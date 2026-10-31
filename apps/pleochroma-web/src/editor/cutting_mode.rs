//! Cutting Mode's model for the browser: the steps of the solved design, the current step,
//! and the done marks, with no Slint in it.
//!
//! The model itself touches no window; its screen is the `push` and `screen` child modules
//! (the page, the keys, the stone picture). The plan, the marks' rules and the page texts are
//! the shared ones in `pleochroma_editor::cutting_mode`; this module adds what the page owns:
//! where the solve comes from, where the marks are stored, and when the plan is stale.
//!
//! # Lifecycle
//!
//! - [`availability`] says, without solving, whether Cutting Mode can open (no design, no
//!   tier, a failed solve); its [`Unavailable::reason`] is the text for a disabled entry
//!   point.
//! - [`open`] asks for the solve ([`with_solved`](crate::app::solve::with_solved): at once
//!   when it is cached, else when the solve it starts finishes), builds the plan the way
//!   the cutting-sheet download does (`io::save::download_cutting_sheet`: the design's
//!   solved masts, the page's custom materials and the stored default axis of the design's
//!   custom material), reads the marks, and starts at the step the caller names, else at
//!   the first step not done.
//! - [`check`] compares the open model with the design: [`Freshness::Stale`] when the design
//!   changed (its generation moved), after which [`restart`] builds the plan again and puts
//!   the model on the first step not done (the page says [`DESIGN_CHANGED_MESSAGE`]);
//!   [`Freshness::Rebound`] when only the design's `[meta]` id appeared (the first save
//!   stamps one), which re-files the marks without moving anything.
//! - [`with_mode`] gives the open [`CuttingMode`] to a closure: the reads
//!   ([`CuttingMode::page`], [`CuttingMode::wheel`], [`CuttingMode::axis_picture`],
//!   [`CuttingMode::progress_line`], ...), the moves ([`CuttingMode::next`], ...) and the
//!   marks ([`CuttingMode::toggle_done`], ...). [`close`] drops the model.
//!
//! # Marks
//!
//! Stored through [`crate::storage::marks`] as [`StoredMarks`](pleochroma_web_core::storage_keys::StoredMarks)
//! under the stone key of `pleochroma_web_core::storage_keys::mark_keys`: the design's id
//! when it has one, else a hash of its native text. The marks live in this browser only (a
//! different browser, a private window or cleared site data starts without them); say so on
//! the screen. A storage that refuses the write does not stop the screen: the mark stays in
//! memory and [`MarkOutcome::saved`] is `false`.
//!
//! # The stone picture
//!
//! [`CuttingMode::cut_steps`] is the `cut_steps` of the step showing (`position + 1`, cutting
//! order, concave tiers included) for
//! `pleochroma_solid::preview::view::plan_job_with_cut_steps`;
//! [`CuttingMode::table_row`] is the tier-table row of that step for the selection.
//!
//! The tutorial event `CUTTING_STEP_MARKED` is raised by the screen (`screen::mark`) when
//! [`MarkOutcome::newly_done`] is `true`, whichever route marked the step (the button, Space or
//! D, or the last index chip). Taking a mark back does not raise it.

mod push;
mod screen;

pub use screen::wire;

use crate::{
    app::{
        Ctx,
        solve::with_solved,
        state::{DesignState, SolveState, WebApp},
    },
    storage::marks,
};
use pleochroma::geometry::meet_solver::SolvedTier;
use pleochroma_editor::cutting_mode::{
    CuttingPlan, CuttingStep,
    axis_picture::AxisPicture,
    build_plan_with_default,
    dial::IndexWheel,
    display::{StepPage, progress_fraction, progress_line, stone_caption, wheel_caption},
    progress::{MarkChange, Progress, next_position, previous_position},
};
use pleochroma_web_core::{
    custom_material::stored_default_orientation,
    storage_keys::{MarkKeys, mark_keys},
};
use std::{cell::RefCell, collections::BTreeMap};

/// What the page tells the user after the design changed under an open Cutting Mode.
pub const DESIGN_CHANGED_MESSAGE: &str = "The design changed; Cutting Mode restarted";

/// The sentence that says where the marks are kept.
pub const MARKS_LIVE_HERE: &str = "Your cutting marks are kept in this browser only; another browser or cleared site data starts without them.";

/// Why Cutting Mode cannot open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// No design is loaded.
    NoDesign,
    /// The design has no tier to cut.
    NoTiers,
    /// The design has no usable solve; carries the solver's own message.
    SolveFailed(String),
    /// The steps cannot be listed for the solve the design has.
    NoPlan,
}

impl Unavailable {
    /// The sentence for a disabled entry point or a message.
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::NoDesign => "Open or create a design first.".to_owned(),
            Self::NoTiers => "This design has no tiers to cut.".to_owned(),
            Self::SolveFailed(message) => {
                format!("Cutting Mode needs a solved design: {message}")
            }
            Self::NoPlan => "The cutting steps cannot be listed for this design.".to_owned(),
        }
    }
}

/// How the open model stands against the design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Cutting Mode is not open.
    Closed,
    /// The model still matches the design.
    Fresh,
    /// Only the design's `[meta]` id changed; the marks were re-filed under it and nothing
    /// else moved.
    Rebound,
    /// The design changed: call [`restart`].
    Stale,
}

/// What a change of the marks did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkOutcome {
    /// Whether any mark changed.
    pub changed: bool,
    /// Whether the number of steps that are done went up: the moment for the tutorial event
    /// `CUTTING_STEP_MARKED`. Taking a mark back, or ticking one index of several, does not.
    pub newly_done: bool,
    /// Whether the browser storage took the new marks. `false` means they last until the
    /// page closes; the screen should say so.
    pub saved: bool,
}

impl MarkOutcome {
    /// Nothing changed (and nothing needed storing).
    const NONE: Self = Self {
        changed: false,
        newly_done: false,
        saved: true,
    };
}

/// The open Cutting Mode: the steps, where it is, and the marks. See the module docs.
#[derive(Debug)]
pub struct CuttingMode {
    /// The design generation the plan was built at.
    generation: u64,
    /// The design's `[meta]` id the marks are filed under (empty before the first save).
    id: String,
    plan: CuttingPlan,
    /// The current step, an index into the plan's steps.
    position: usize,
    keys: MarkKeys,
    /// The stored form of the marks, kept in step with [`Self::progress`].
    marks: BTreeMap<String, String>,
    progress: Progress,
}

thread_local! {
    /// The open model; `None` while Cutting Mode is closed.
    static MODE: RefCell<Option<CuttingMode>> = const { RefCell::new(None) };
}

/// Whether Cutting Mode can open for the current design, judged without solving.
///
/// It can when the design has a tier and its last solve for the current generation did not
/// fail.
///
/// # Errors
///
/// The reason it cannot open; [`Unavailable::reason`] has the text.
pub fn availability(app: &WebApp) -> Result<(), Unavailable> {
    let design = app.design.as_ref().ok_or(Unavailable::NoDesign)?;
    let flat = &design.session.design;
    if flat.tiers.is_empty() && flat.concave_tiers.is_empty() {
        return Err(Unavailable::NoTiers);
    }
    match &app.solve {
        SolveState::Failed {
            generation,
            message,
        } if *generation == design.session.current_generation() => {
            Err(Unavailable::SolveFailed(message.clone()))
        }
        _ => Ok(()),
    }
}

/// Opens Cutting Mode on the design's current solve.
///
/// It starts at step `start` when that is a valid step, else at the first step not done (the
/// last step when everything is done). `then` runs once the model is built (at once when the
/// solve is cached) or when it cannot be; on an error an already open model is left as it was.
pub fn open(
    ctx: &Ctx,
    start: Option<usize>,
    then: impl FnOnce(&Ctx, Result<(), Unavailable>) + 'static,
) {
    if let Err(unavailable) = availability(&ctx.state.borrow()) {
        then(ctx, Err(unavailable));
        return;
    }
    with_solved(ctx, move |ctx, solved| {
        let result = solved
            .map_err(Unavailable::SolveFailed)
            .and_then(|solved| CuttingMode::build(&ctx.state.borrow(), &solved, start))
            .map(|built| MODE.with(|mode| *mode.borrow_mut() = Some(built)));
        then(ctx, result);
    });
}

/// Builds the plan again for the changed design.
///
/// The model goes to the first step not done, or closes when the design cannot be cut any
/// more. `then` gets the result, so the page can say [`DESIGN_CHANGED_MESSAGE`] or close the
/// screen.
pub fn restart(ctx: &Ctx, then: impl FnOnce(&Ctx, Result<(), Unavailable>) + 'static) {
    open(ctx, None, move |ctx, result| {
        if result.is_err() {
            close();
        }
        then(ctx, result);
    });
}

/// Closes Cutting Mode by dropping the model. The marks stay stored.
pub fn close() {
    MODE.with(|mode| *mode.borrow_mut() = None);
}

/// Whether Cutting Mode is open.
#[must_use]
pub fn is_open() -> bool {
    MODE.with(|mode| mode.borrow().is_some())
}

/// Compares the open model with the design.
///
/// Cheap: call it from the page's poll while the screen shows. On [`Freshness::Stale`] call
/// [`restart`].
pub fn check(ctx: &Ctx) -> Freshness {
    let Ok(app) = ctx.state.try_borrow() else {
        // A writer holds the state: the next poll looks again.
        return if is_open() {
            Freshness::Fresh
        } else {
            Freshness::Closed
        };
    };
    MODE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(model) = slot.as_mut() else {
            return Freshness::Closed;
        };
        let Some(design) = app.design.as_ref() else {
            return Freshness::Stale;
        };
        if design.session.current_generation() != model.generation {
            Freshness::Stale
        } else if design.metadata.id != model.id {
            model.rebind(design);
            Freshness::Rebound
        } else {
            Freshness::Fresh
        }
    })
}

/// Runs `f` on the open model; `None` when Cutting Mode is closed.
pub fn with_mode<R>(f: impl FnOnce(&mut CuttingMode) -> R) -> Option<R> {
    MODE.with(|slot| slot.borrow_mut().as_mut().map(f))
}

/// The marks stored for `keys`: the current slot, else the legacy one.
///
/// The legacy slot is the one the design had before it got an id. `None` when neither holds
/// any.
fn stored_marks(keys: &MarkKeys) -> Option<BTreeMap<String, String>> {
    marks::load_marks(&keys.primary).or_else(|| keys.legacy.as_deref().and_then(marks::load_marks))
}

impl CuttingMode {
    /// The model of `app`'s design from its solved masts `solved`.
    ///
    /// It starts at `start` when that is a valid step, else at the first step not done.
    fn build(
        app: &WebApp,
        solved: &[SolvedTier],
        start: Option<usize>,
    ) -> Result<Self, Unavailable> {
        let state = app.design.as_ref().ok_or(Unavailable::NoDesign)?;
        let design = &state.session.design;
        if design.tiers.is_empty() && design.concave_tiers.is_empty() {
            return Err(Unavailable::NoTiers);
        }
        let plan = build_plan_with_default(
            design,
            solved,
            &app.custom_materials,
            stored_default_orientation(state.custom_snapshot()),
        )
        .ok_or(Unavailable::NoPlan)?;
        let keys = mark_keys(&state.metadata.id, design);
        let marks = stored_marks(&keys).unwrap_or_default();
        let progress = Progress::from_marks(marks.clone());
        let position = match start {
            Some(wanted) if wanted < plan.steps.len() => wanted,
            _ => progress.resume_position(&plan.steps),
        };
        Ok(Self {
            generation: state.session.current_generation(),
            id: state.metadata.id.clone(),
            plan,
            position,
            keys,
            marks,
            progress,
        })
    }

    /// Files the marks under the keys of `state`'s design, whose id changed since the build.
    ///
    /// The marks held here stay: a design that just got its id still has the text it had, and
    /// a stored copy under the new keys only wins when there is one.
    fn rebind(&mut self, state: &DesignState) {
        self.keys = mark_keys(&state.metadata.id, &state.session.design);
        self.id.clone_from(&state.metadata.id);
        if let Some(stored) = stored_marks(&self.keys) {
            self.progress = Progress::from_marks(stored.clone());
            self.marks = stored;
        } else if !self.marks.is_empty() {
            // Held in memory only (storage refused them before): file them now.
            marks::save_marks(&self.keys.primary, &self.marks);
        }
    }

    // ---- reads ------------------------------------------------------------------------

    /// The plan: the steps in cutting order, the gear and the orientation block.
    #[must_use]
    pub const fn plan(&self) -> &CuttingPlan {
        &self.plan
    }

    /// The marks, for the shared page builders.
    #[must_use]
    pub const fn progress(&self) -> &Progress {
        &self.progress
    }

    /// The current step, counting from 0.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }

    /// The number of steps.
    #[must_use]
    pub const fn step_count(&self) -> usize {
        self.plan.steps.len()
    }

    /// The current step.
    #[must_use]
    pub fn step(&self) -> Option<&CuttingStep> {
        self.plan.steps.get(self.position)
    }

    /// Everything the page shows about the current step.
    #[must_use]
    pub fn page(&self) -> Option<StepPage> {
        StepPage::new(&self.plan.steps, self.position, &self.progress)
    }

    /// The index wheel with the current step's indices marked.
    #[must_use]
    pub fn wheel(&self) -> Option<IndexWheel> {
        Some(IndexWheel::new(
            self.plan.gear_teeth,
            self.step()?,
            &self.progress,
        ))
    }

    /// The crystal-axis picture of the first step; `None` on every other step and for a stone
    /// without a set axis.
    #[must_use]
    pub fn axis_picture(&self) -> Option<AxisPicture> {
        if self.position != 0 {
            return None;
        }
        self.plan.orientation.as_ref().map(AxisPicture::new)
    }

    /// The orientation sentence of the first step (how to orient the rough for the crystal
    /// axis); `None` on every other step and for a stone without a set axis.
    #[must_use]
    pub fn orientation_note(&self) -> Option<String> {
        if self.position == 0 {
            self.plan.orientation_note()
        } else {
            None
        }
    }

    /// `4 of 12 steps done`, with the steps that changed since they were marked.
    #[must_use]
    pub fn progress_line(&self) -> String {
        progress_line(&self.plan.steps, &self.progress)
    }

    /// How much of the progress bar is full, from 0 to 1.
    #[must_use]
    pub fn progress_fraction(&self) -> f32 {
        progress_fraction(&self.plan.steps, &self.progress)
    }

    /// The caption under the stone picture.
    #[must_use]
    pub fn stone_caption(&self) -> Option<String> {
        Some(stone_caption(self.step()?, self.step_count()))
    }

    /// The caption under the index wheel.
    #[must_use]
    pub fn wheel_caption(&self) -> Option<String> {
        Some(wheel_caption(self.plan.gear_teeth, self.step()?))
    }

    /// How many steps are done.
    #[must_use]
    pub fn done_count(&self) -> usize {
        self.progress.done_count(&self.plan.steps)
    }

    /// Whether anything is marked.
    ///
    /// A done mark, a tick, or a mark of a step that changed counts: a Reset has something
    /// to clear exactly when this is true.
    #[must_use]
    pub fn has_marks(&self) -> bool {
        !self.marks.is_empty()
    }

    /// The `cut_steps` of the stone picture of the current step.
    ///
    /// The number of steps cut so far, this one included, in cutting order (concave tiers
    /// included). For `plan_job_with_cut_steps(inputs, Some(mode.cut_steps()))`.
    #[must_use]
    pub const fn cut_steps(&self) -> usize {
        self.position + 1
    }

    /// The tier-table row of the current step (flat tiers first, concave after), the row to
    /// select.
    #[must_use]
    pub fn table_row(&self) -> Option<usize> {
        self.step().map(|step| step.table_row)
    }

    // ---- moves ------------------------------------------------------------------------

    /// Moves to the next step. `false` at the last step.
    pub const fn next(&mut self) -> bool {
        self.go(next_position(self.position, self.step_count()))
    }

    /// Moves to the previous step. `false` at the first step.
    pub const fn previous(&mut self) -> bool {
        self.go(previous_position(self.position))
    }

    /// Moves to `step`. `false` when there is no such step or it is the current one.
    pub fn jump(&mut self, step: usize) -> bool {
        self.go((step < self.step_count()).then_some(step))
    }

    const fn go(&mut self, target: Option<usize>) -> bool {
        match target {
            Some(target) if target != self.position => {
                self.position = target;
                true
            }
            _ => false,
        }
    }

    // ---- marks ------------------------------------------------------------------------

    /// The "Mark step done" button.
    ///
    /// It marks a step that is not done and takes the mark of a done one back, with its index
    /// ticks.
    pub fn toggle_done(&mut self) -> MarkOutcome {
        let Some(step) = self.step() else {
            return MarkOutcome::NONE;
        };
        let changes = self.progress.toggle_done(step);
        self.commit(&changes)
    }

    /// Space or D: marks the step done.
    ///
    /// A step already done needs nothing, so the key can be pressed again to move on. Moving
    /// on is [`Self::next`].
    pub fn mark_done(&mut self) -> MarkOutcome {
        let Some(step) = self.step() else {
            return MarkOutcome::NONE;
        };
        let changes = self.progress.mark_done(step);
        self.commit(&changes)
    }

    /// A click on index chip `chip` of the current step.
    ///
    /// It ticks or un-ticks the chip; ticking the last one also marks the step done.
    pub fn toggle_chip(&mut self, chip: usize) -> MarkOutcome {
        let Some(step) = self.step() else {
            return MarkOutcome::NONE;
        };
        let changes = self.progress.toggle_chip(step, chip);
        self.commit(&changes)
    }

    /// Removes every mark of the design (both stored slots) and goes back to the first step.
    pub fn reset(&mut self) -> MarkOutcome {
        let had_marks = !self.marks.is_empty();
        self.marks.clear();
        self.progress.clear();
        for stem in self.keys.clone().into_stems() {
            marks::remove(&stem);
        }
        self.position = 0;
        MarkOutcome {
            changed: had_marks,
            newly_done: false,
            saved: true,
        }
    }

    /// Applies `changes` to the marks held here and stores the result.
    fn commit(&mut self, changes: &[MarkChange]) -> MarkOutcome {
        if changes.is_empty() {
            return MarkOutcome::NONE;
        }
        let before = self.done_count();
        self.progress.apply(changes);
        for change in changes {
            match change {
                MarkChange::Mark { key, signature } => {
                    self.marks.insert(key.clone(), signature.clone());
                }
                MarkChange::Unmark { key } => {
                    self.marks.remove(key);
                }
            }
        }
        let saved = marks::save_marks(&self.keys.primary, &self.marks);
        MarkOutcome {
            changed: true,
            newly_done: self.done_count() > before,
            saved,
        }
    }
}
