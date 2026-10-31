//! Trials: the designs a cutter kept to compare or come back to, with their figures. The History
//! tab's third view (`ui/components/editor_inspector/trials_view.slint`, state in
//! `ui/models/trials.slint`); the list, its rules and its table rows are
//! `pleochroma_editor::trials`.
//!
//! A trial is the design with its solved masts, its figures (the Baseline strip's score, under
//! the Grading tray), the material those figures were scored in and the design it belongs to.
//! The list lives only in this session: [`clear`] empties it whenever another design opens
//! (`EditorState::replace_wholesale`), [`release_window_state`] lets go of it with the window, and
//! "Save as Variant..." is the way to keep one.
//!
//! # Recording
//!
//! - Keep as Trial: the button on the Baseline strip and in the view, Ctrl+Shift+K and the
//!   palette ([`actions`]).
//! - The hooks: an Optimize apply records the design before it and the candidate applied, a
//!   Retarget apply records the design before it ([`hold`], [`record_held`], [`record_design`]),
//!   and Use This Angle records the design after its edit ([`record_after_apply`]).
//! - Record Every Change (off by default): the timer below looks at the history for a revision it
//!   has not seen, which is what a committed edit is and what an Undo, a Redo and a jump never
//!   make ([`record::EditWatch`]), and records at most one trial a second.
//!
//! A trial recorded before the strip scored its design has no figures yet: the timer files them
//! when the strip delivers them for that design generation. Trials scored in another material, or
//! not at all, show grey until "Re-score Trials" ([`rescore`]).
//!
//! # Threads
//!
//! Everything runs on the UI thread except the re-scoring, which has a thread of its own that the
//! timer polls. The state is one [`Hub`] per window in a `thread_local!`, like the Baseline's; no
//! borrow of it is held across a call into the editor or into Slint, except the timer's own
//! pushes, which call nothing that comes back here.

mod actions;
mod default_axis;
mod present;
mod record;
mod rescore;
#[cfg(test)]
mod tests;

pub(in crate::gui::editor) use default_axis::register_vault;
pub(in crate::gui::editor) use pleochroma_editor::trials::TrialOrigin;
pub(in crate::gui::editor) use record::{Held, hold, hold_design};

use crate::{
    BaselineModel, EditorModel, MainWindow, VariantsModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            baseline, callbacks::baseline_clone, state::EditorState, view::submit_preview_replan,
        },
        solid_preview::preview_state::{SolidLastSolved, SolidPreviewState},
    },
};
use pleochroma::geometry::meet_solver::SolvedTier;
use pleochroma_cut_core::Design;
use pleochroma_editor::trials::{Trial, TrialDraft, TrialList, trial_label, trial_rows_sorted};
use present::View;
use record::{EditWatch, RecordGate};
use slint::{ComponentHandle, Timer, TimerMode, Weak};
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashSet},
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};

/// How often the timer looks at the history, the strip's figures and the re-scoring.
const TICK: Duration = Duration::from_millis(250);

/// `EditorModel.inspector_tab` of the History tab, which hosts the Trials view.
const HISTORY_TAB: i32 = 4;

/// `VariantsModel.view` of the Trials view.
const TRIALS_VIEW: i32 = 2;

thread_local! {
    /// The window's trials; see the module doc comment for why it is a `thread_local!`.
    static HUB: RefCell<Hub> = RefCell::new(Hub::new());
}

/// The handles the actions and the timer need.
struct Deps {
    state: Rc<RefCell<EditorState>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    preview_state: Arc<SolidPreviewState>,
    solid_last_solved: SolidLastSolved,
}

/// The editor as one look at it found it.
struct Snap {
    /// The design epoch (another one is another design).
    epoch: u64,
    generation: u64,
    /// The material key of the design now.
    key: String,
    /// The history's `(revision, undone)` pairs.
    revisions: Vec<(u64, bool)>,
    has_design: bool,
}

impl Snap {
    fn of(state: &EditorState) -> Self {
        Self {
            epoch: state.design_epoch.load(Ordering::Relaxed),
            generation: state.current_generation(),
            key: record::trial_key(
                &state.design.material,
                &state.design.render_effects,
                default_axis::of(&state.design.material),
            ),
            revisions: record::revisions_of(state),
            has_design: state.has_design,
        }
    }
}

/// The design the stone shows as a preview of a selected trial, and when it stops being the
/// right thing to show.
#[derive(Debug, Clone, Copy)]
struct PreviewMark {
    epoch: u64,
    generation: u64,
}

/// What the window's Trials view knows.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent UI-session flags, not a state machine"
)]
struct Hub {
    ui: Option<Weak<MainWindow>>,
    deps: Option<Rc<Deps>>,
    /// Calls [`tick_now`]; stopped (dropped) with the window.
    timer: Timer,
    started: Instant,
    list: TrialList,
    /// The drop-down's entry.
    sort_index: i32,
    /// The selected trial's number.
    selected: Option<u32>,
    /// Set while the stone shows the selected trial.
    preview: Option<PreviewMark>,
    every_change: bool,
    watch: EditWatch,
    gate: RecordGate,
    /// An edit landed that "Record Every Change" has not recorded yet (the gate was shut).
    owed: bool,
    /// The next edit the watch sees is one a hook recorded itself (an Optimize or Retarget
    /// apply), not one to record again.
    skip_edit: bool,
    /// Trials waiting for the strip's figures: `(number, design generation)`.
    pending: Vec<(u32, u64)>,
    /// Trials a re-score found to give no stone: scored, with nothing to show.
    no_stone: HashSet<u32>,
    rescoring: Option<rescore::Run>,
    /// The material key of the design now, as last seen.
    current_key: String,
    design_open: bool,
    last_view: Option<View>,
}

impl Hub {
    fn new() -> Self {
        Self {
            ui: None,
            deps: None,
            timer: Timer::default(),
            started: Instant::now(),
            list: TrialList::new(),
            sort_index: 0,
            selected: None,
            preview: None,
            every_change: false,
            watch: EditWatch::default(),
            gate: RecordGate::default(),
            owed: false,
            skip_edit: false,
            pending: Vec::new(),
            no_stone: HashSet::new(),
            rescoring: None,
            current_key: String::new(),
            design_open: false,
            last_view: None,
        }
    }

    /// Another design opened: the trials of the one before are gone. The stone is about to show
    /// the new design, so nothing here puts the old one back.
    fn clear_for_new_design(&mut self) {
        self.list.clear();
        self.selected = None;
        self.preview = None;
        self.watch.reset();
        self.owed = false;
        self.skip_edit = false;
        self.pending.clear();
        self.no_stone.clear();
        self.rescoring = None;
    }

    /// Adds `held` to the list and returns its number.
    fn push(&mut self, held: Held) -> u32 {
        let label = held.label.unwrap_or_else(|| self.auto_label(&held.design));
        let number = self.list.push(TrialDraft {
            design: held.design,
            solved: held.solved,
            metrics: held.metrics,
            material_key: held.key,
            design_epoch: held.epoch,
            label,
            origin: held.origin,
        });
        if let Some(generation) = held.awaiting {
            self.pending.push((number, generation));
        }
        if held.scored_no_stone {
            // Scored already, and the design gives no stone: "Re-score Trials" has nothing to do.
            self.no_stone.insert(number);
        }
        // A hook records its own change (an apply); the edit it made is not another trial. Tune's
        // Accept and a hand-kept trial make no edit of their own.
        if !matches!(held.origin, TrialOrigin::Manual | TrialOrigin::Tune) {
            self.skip_edit = true;
        }
        number
    }

    /// The label from the tier diff against the trial before, or the Baseline when there is
    /// none.
    fn auto_label(&self, design: &Design) -> String {
        let previous = self
            .list
            .iter()
            .last()
            .map(|trial| trial.design.clone())
            .or_else(|| baseline_clone().map(|(_, snapshot)| snapshot.design));
        previous.map_or_else(
            || "First trial".to_owned(),
            |previous| trial_label(&previous, design),
        )
    }

    /// Whether any trial would change by "Re-score Trials": scored in another material than
    /// `key`, or not scored at all (and not waiting for the strip's figures).
    fn needs_rescore(&self, key: &str) -> bool {
        !key.is_empty() && self.list.iter().any(|trial| self.wants_rescore(trial, key))
    }

    /// Whether `trial`'s figures are for another material than `key`, or are missing and not
    /// on their way.
    fn wants_rescore(&self, trial: &Trial, key: &str) -> bool {
        // Only the material part: a trial is scored in its own orientation, whatever the design's
        // is now, so another Crystal Axis alone never makes a trial wait for a re-score.
        record::material_part(&trial.material_key) != record::material_part(key)
            || (trial.metrics.is_none()
                && !self.no_stone.contains(&trial.number)
                && !self
                    .pending
                    .iter()
                    .any(|(number, _)| *number == trial.number))
    }

    /// The trials "Re-score Trials" scores: the ones [`Self::needs_rescore`] names.
    fn items_to_rescore(&self, key: &str) -> Vec<rescore::Item> {
        self.list
            .iter()
            .filter(|trial| self.wants_rescore(trial, key))
            .map(|trial| rescore::Item {
                number: trial.number,
                design: trial.design.clone(),
                solved: trial.solved.clone(),
            })
            .collect()
    }

    /// One look at everything; see the module doc comment.
    fn tick(&mut self, now: Instant) {
        let Some(ui) = self.ui.as_ref().and_then(Weak::upgrade) else {
            return;
        };
        let Some(deps) = self.deps.clone() else {
            return;
        };
        let snap = deps.state.try_borrow().ok().map(|state| Snap::of(&state));
        self.keep_preview_honest(&ui, &deps, snap.as_ref());
        self.fill_pending(snap.as_ref());
        self.poll_rescore();
        self.record_edits(&deps, snap.as_ref(), now);
        self.refresh(&ui, snap.as_ref());
    }

    /// Ends the stone's preview when it stopped being right: the design changed under it, or
    /// the Trials view is not on screen any more.
    fn keep_preview_honest(&mut self, ui: &MainWindow, deps: &Deps, snap: Option<&Snap>) {
        let Some(mark) = self.preview else {
            return;
        };
        let moved =
            snap.is_some_and(|snap| snap.epoch != mark.epoch || snap.generation != mark.generation);
        if !moved && trials_showing(ui) {
            return;
        }
        self.preview = None;
        self.selected = None;
        // An edit's own replan already put the real design back.
        if !moved {
            show_real_design(ui, deps);
        }
    }

    /// Files the figures the strip scored for trials that were recorded before it had them.
    fn fill_pending(&mut self, snap: Option<&Snap>) {
        let Some(snap) = snap else {
            return;
        };
        let list = &mut self.list;
        self.pending.retain(|&(number, generation)| {
            if let Some((selection, metrics)) = baseline::design_figures(generation) {
                // Scored in the orientation of the design the trial recorded.
                let key = list.get(number).map_or_else(
                    || record::material_key(&selection),
                    |trial| {
                        record::trial_key(
                            &selection,
                            &trial.design.render_effects,
                            default_axis::of(&selection),
                        )
                    },
                );
                list.set_metrics(number, metrics, &key);
                return false;
            }
            // The design moved on without the strip scoring that generation: "Re-score Trials"
            // does it.
            snap.generation == generation && list.get(number).is_some()
        });
    }

    /// Files the figures a re-score has delivered.
    fn poll_rescore(&mut self) {
        let Some(run) = &self.rescoring else {
            return;
        };
        let over = run.is_over();
        for (number, metrics, key) in run.drain() {
            if metrics.is_none() {
                self.no_stone.insert(number);
            }
            // Filed under the key of the orientation the trial was scored in (its own).
            self.list.set_metrics(number, metrics, &key);
        }
        if over {
            self.rescoring = None;
        }
    }

    /// "Record Every Change": records the design when a committed edit landed.
    fn record_edits(&mut self, deps: &Deps, snap: Option<&Snap>, now: Instant) {
        let Some(snap) = snap else {
            return;
        };
        let landed = self.watch.observe(snap.epoch, &snap.revisions);
        if self.skip_edit && landed {
            // A hook recorded this change itself.
            self.skip_edit = false;
            return;
        }
        if !self.every_change || !snap.has_design {
            return;
        }
        self.owed |= landed;
        if !self.owed {
            return;
        }
        let millis =
            u64::try_from(now.duration_since(self.started).as_millis()).unwrap_or(u64::MAX);
        let Ok(state) = deps.state.try_borrow() else {
            return;
        };
        if self.gate.allow(millis) {
            self.owed = false;
            let held = record::hold(&state, TrialOrigin::Manual, None);
            self.push(held);
        }
    }

    /// Works out what the view shows now and pushes what changed.
    fn refresh(&mut self, ui: &MainWindow, snap: Option<&Snap>) {
        if let Some(snap) = snap {
            self.current_key.clone_from(&snap.key);
            self.design_open = snap.has_design;
        }
        let view = self.build_view(ui);
        if self.last_view.as_ref() != Some(&view) {
            present::push(ui, &view, self.last_view.as_ref());
            self.last_view = Some(view);
        }
    }

    /// The view: the rows against the Baseline, and the selected trial's details.
    fn build_view(&self, ui: &MainWindow) -> View {
        let key = self.current_key.as_str();
        // The Baseline's figures count only in the material they were scored in; the strip
        // re-scores it when the material changes. They are scored in the Baseline's own
        // orientation, so the material part of the key decides, not the axis.
        let baseline_metrics = baseline::baseline_figures()
            .filter(|(selection, _)| record::material_key(selection) == record::material_part(key))
            .and_then(|(_, metrics)| metrics);
        let rows = trial_rows_sorted(
            &self.list,
            baseline_metrics.as_ref(),
            present::sort_for(self.sort_index),
        )
        .iter()
        .map(|row| present::row_of(row, key))
        .collect();
        let strip = ui.global::<BaselineModel>();
        let label = if strip.get_has_baseline() {
            strip.get_label().to_string()
        } else {
            String::new()
        };
        let selected = self.selected.and_then(|number| self.list.get(number));
        View {
            rows,
            baseline: present::baseline_row_of(&label, baseline_metrics.as_ref()),
            selected: selected.map_or(-1, |trial| i32::try_from(trial.number).unwrap_or(-1)),
            selected_label: selected.map_or_else(String::new, |trial| {
                format!("Trial {}: {}", trial.number, trial.label)
            }),
            selected_note: selected.map_or_else(String::new, |trial| trial.note.clone()),
            empty_text: if self.list.is_empty() {
                present::EMPTY_TEXT.to_owned()
            } else {
                String::new()
            },
            design_open: self.design_open,
            rescore_needed: self.needs_rescore(key),
            rescoring: self.rescoring.is_some(),
        }
    }
}

/// Runs `action` on the window's [`Hub`]; `None` when it is in use (which cannot happen on one
/// thread unless a callback of ours pumped the event loop).
fn with_hub<R>(action: impl FnOnce(&mut Hub) -> R) -> Option<R> {
    HUB.with(|cell| cell.try_borrow_mut().ok().map(|mut hub| action(&mut hub)))
}

/// The timer's callback.
fn tick_now() {
    let _ = with_hub(|hub| hub.tick(Instant::now()));
}

/// Whether the Trials view is on screen: the History tab is selected, not collapsed, and showing
/// its Trials.
fn trials_showing(ui: &MainWindow) -> bool {
    let editor = ui.global::<EditorModel>();
    editor.get_inspector_tab() == HISTORY_TAB
        && !editor.get_inspector_collapsed()
        && ui.global::<VariantsModel>().get_view() == TRIALS_VIEW
}

/// Puts the real design back on the stone after a trial's preview.
fn show_real_design(ui: &MainWindow, deps: &Deps) {
    let Ok(state) = deps.state.try_borrow() else {
        return;
    };
    submit_preview_replan(
        ui,
        &deps.render_ctx,
        &deps.preview_state,
        &deps.solid_last_solved,
        &state,
        BTreeSet::new(),
        false,
    );
}

/// Shows what changed in the list, with `snap` as the editor now (`None`: the last look).
fn refresh_with(ui: &MainWindow, snap: Option<&Snap>) {
    let _ = with_hub(|hub| hub.refresh(ui, snap));
}

/// Wires the Trials view's callbacks and starts the timer that watches the history, files the
/// strip's figures and polls the re-scoring.
pub(super) fn setup_trials(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let _ = with_hub(|hub| {
        *hub = Hub::new();
        hub.ui = Some(ui.as_weak());
        hub.deps = Some(Rc::new(Deps {
            state: Rc::clone(state),
            render_ctx: Arc::clone(render_ctx),
            preview_state: Arc::clone(preview_state),
            solid_last_solved: Arc::clone(solid_last_solved),
        }));
        hub.timer.start(TimerMode::Repeated, TICK, tick_now);
    });
    present::push_static(ui);
    actions::wire(ui);
}

/// Lets go of the window's trials (timer, list, the running re-score). See
/// `gui::window_release`.
pub(super) fn release_window_state() {
    let _ = with_hub(|hub| *hub = Hub::new());
}

/// Names this module's holder in `out` while it is set. See `gui::window_release::live_holders`.
#[cfg(feature = "ui-harness")]
pub(super) fn note_holders(out: &mut Vec<&'static str>) {
    if with_hub(|hub| hub.deps.is_some()).unwrap_or(false) {
        out.push("editor::trials HUB (editor state, render context, timer)");
    }
}

/// Empties the list: another design opened (`EditorState::replace_wholesale`).
pub(in crate::gui::editor) fn clear() {
    let _ = with_hub(Hub::clear_for_new_design);
}

/// Records `held` as a trial and shows the list; the trial's number.
pub(in crate::gui::editor) fn record_held(
    ui: &MainWindow,
    state: &EditorState,
    held: Held,
) -> Option<u32> {
    let number = with_hub(|hub| hub.push(held))?;
    refresh_with(ui, Some(&Snap::of(state)));
    Some(number)
}

/// Records the design `state` holds now as a trial of `origin`; `label` replaces the
/// automatic one from the tier diff.
pub(in crate::gui::editor) fn record_trial(
    ui: &MainWindow,
    state: &EditorState,
    origin: TrialOrigin,
    label: Option<String>,
) -> Option<u32> {
    record_held(ui, state, hold(state, origin, label))
}

/// Records the design `state` holds now, after a change that was applied to it (Use This
/// Angle, Use These Angles), as a trial of `origin`.
pub(in crate::gui::editor) fn record_after_apply(
    ui: &MainWindow,
    state: &EditorState,
    origin: TrialOrigin,
) {
    record_trial(ui, state, origin, None);
}

/// Records another design of the open document as a trial of `origin` (the design before a
/// Retarget Apply). Its figures are not known: "Re-score Trials" scores them.
pub(in crate::gui::editor) fn record_design(
    ui: &MainWindow,
    state: &EditorState,
    design: Design,
    solved: Option<Vec<SolvedTier>>,
    origin: TrialOrigin,
    label: Option<String>,
) -> Option<u32> {
    record_held(ui, state, hold_design(state, design, solved, origin, label))
}
