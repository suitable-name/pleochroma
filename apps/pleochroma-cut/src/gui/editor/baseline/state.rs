//! What the Baseline strip remembers between ticks, and what it decides at each one.
//!
//! One [`Hub`] per window, in a `thread_local!` like the other holders of the editor (the
//! callbacks that feed it have no handle to anything else). A timer on the UI thread calls
//! [`Hub::tick`] every [`TICK`]:
//!
//! 1. it notices a Baseline that was replaced behind its back (the guide's reference design);
//! 2. it looks at the running job and files its figures, when they still describe what is shown;
//! 3. it starts the next job when the design now has masts the figures were not scored from,
//!    300 ms after the last solve (so a drag scores at its end), and neither Optimize nor an
//!    Angle Sweep holds the cores;
//! 4. it works out the strip and pushes it when it changed.
//!
//! The strip never solves. The masts come from the solves the editor already runs
//! (`baseline::on_solved`); a design whose solve does not come (auto-solve off or over its budget) leaves
//! the strip showing its last figures, dimmed, with "Out of Date: Press Solve".

use super::{
    present::{self, CellView, View},
    worker::{self, Job, Part, Poll, RunHandle, Scored},
};
use crate::{
    EditorModel, MainWindow, SweepModel, TuneModel,
    bridge::render_thread::RenderContext,
    gui::editor::{
        callbacks::{baseline_clone, baseline_marker, fill_baseline_masts, install_baseline},
        state::EditorState,
        view::design_label,
    },
};
use pleochroma::geometry::meet_solver::SolvedTier;
use pleochroma_cut_core::{Design, MaterialSelection};
use pleochroma_editor::{
    live_deltas::figure_deltas, optimize_view::default_optimize_material_ri,
    snapshot::DesignSnapshot, sweep::RowMetrics,
};
use slint::{ComponentHandle, Timer, TimerMode, Weak};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

/// How often the UI thread looks at the strip's worker.
const TICK: Duration = Duration::from_millis(100);

/// How long after the last solve the design is scored: a drag or a burst of edits scores once,
/// at its end.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// How long the design may be ahead of its last solve before the figures are called out of date:
/// the auto-solve normally catches up within this.
const STALE_GRACE: Duration = Duration::from_secs(1);

/// How long a one-off sentence ("Solve the design first ...") stays under the figures.
const NOTE_HOLD: Duration = Duration::from_secs(6);

thread_local! {
    /// The strip's state; see the module doc comment for why it is a `thread_local!`.
    static HUB: RefCell<Hub> = RefCell::new(Hub::new());
}

/// How the Baseline came to be, for the strip's label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::gui::editor) enum Origin {
    /// A design was opened (New, Open, Load Selected).
    Opened,
    /// An Optimize result was applied.
    Optimize,
    /// A Retarget was applied.
    Retarget,
    /// Set Baseline: the cutter chose it.
    Set,
}

impl Origin {
    /// The word the label uses: `opened`, `after Optimize`, `after Retarget`, `set`.
    const fn label(self) -> &'static str {
        match self {
            Self::Opened => "opened",
            Self::Optimize => "after Optimize",
            Self::Retarget => "after Retarget",
            Self::Set => "set",
        }
    }
}

/// The masts of the design now, as the last solve produced them.
struct Fed {
    design: Design,
    solved: Vec<SolvedTier>,
    /// The design generation the masts belong to.
    generation: u64,
    /// Counts the feeds, so figures scored from an older feed are told from these.
    serial: u64,
    /// When the feed arrived (the debounce counts from it).
    at: Instant,
}

/// The figures of the design now.
struct DesignFigures {
    /// The feed they were scored from.
    serial: u64,
    /// The design generation they describe.
    generation: u64,
    /// The material they were scored under.
    key: MaterialSelection,
    /// `None`: the design gives no stone.
    metrics: Option<RowMetrics>,
}

/// The Baseline's own state: which one it is, and its figures once scored.
struct BaselineFigures {
    /// The held Baseline's epoch (`callbacks::baseline_marker`); another epoch is another
    /// Baseline, whose figures are not these.
    epoch: u64,
    /// Whether a Baseline is held at all.
    present: bool,
    origin: Origin,
    since: Instant,
    /// The design generation the Baseline was taken at, while it still lacks masts: the solve
    /// of that generation fills them in.
    awaiting_generation: Option<u64>,
    /// The material the figures were scored under, and the figures (`None`: no stone).
    scored: Option<(MaterialSelection, Option<RowMetrics>)>,
    /// The material the Baseline was last re-scored for, once the material changed.
    rescored_for: Option<String>,
}

impl BaselineFigures {
    const fn new(
        epoch: u64,
        origin: Origin,
        since: Instant,
        awaiting_generation: Option<u64>,
    ) -> Self {
        Self {
            epoch,
            present: true,
            origin,
            since,
            awaiting_generation,
            scored: None,
            rescored_for: None,
        }
    }
}

/// A job in flight, with what its figures will be filed under.
struct Run {
    handle: RunHandle,
    tilt: bool,
    /// Whether the job scores the Baseline.
    scores_baseline: bool,
    serial: u64,
    generation: u64,
    epoch: u64,
    key: MaterialSelection,
}

/// What the window's strip knows.
pub(super) struct Hub {
    ui: Option<Weak<MainWindow>>,
    editor: Option<Rc<RefCell<EditorState>>>,
    render_ctx: Option<Arc<Mutex<RenderContext>>>,
    /// Calls [`tick`]; stopped (dropped) with the window.
    timer: Timer,
    fed: Option<Fed>,
    next_serial: u64,
    design: Option<DesignFigures>,
    baseline: BaselineFigures,
    run: Option<Run>,
    /// "Measure Tilt" was pressed and no job has taken it up yet.
    tilt_requested: bool,
    /// Since when the design has been ahead of its last solve.
    stale_since: Option<Instant>,
    /// A one-off sentence and when it was said.
    flash: Option<(String, Instant)>,
    last_view: Option<View>,
}

impl Hub {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            ui: None,
            editor: None,
            render_ctx: None,
            timer: Timer::default(),
            fed: None,
            next_serial: 0,
            design: None,
            baseline: BaselineFigures {
                present: false,
                ..BaselineFigures::new(0, Origin::Set, now, None)
            },
            run: None,
            tilt_requested: false,
            stale_since: None,
            flash: None,
            last_view: None,
        }
    }

    /// A new Baseline was installed (`epoch`): its figures are not the old ones. A design that
    /// was just opened also drops everything about the design before it.
    fn baseline_installed(&mut self, epoch: u64, origin: Origin, awaiting: Option<u64>) {
        let now = Instant::now();
        self.baseline = BaselineFigures::new(epoch, origin, now, awaiting);
        if origin == Origin::Opened {
            self.fed = None;
            self.design = None;
            self.run = None;
            self.tilt_requested = false;
            self.stale_since = None;
            self.flash = None;
        }
    }

    /// The masts of a solve of `design` at `generation`.
    fn feed(&mut self, design: &Design, solved: &[SolvedTier], generation: u64, now: Instant) {
        if self.ui.is_none() || solved.len() != design.tiers.len() {
            return;
        }
        if let Some(awaiting) = self.baseline.awaiting_generation
            && self.baseline.present
        {
            if generation == awaiting {
                fill_baseline_masts(self.baseline.epoch, solved.to_vec());
            }
            if generation >= awaiting {
                self.baseline.awaiting_generation = None;
            }
        }
        self.next_serial += 1;
        self.fed = Some(Fed {
            design: design.clone(),
            solved: solved.to_vec(),
            generation,
            serial: self.next_serial,
            at: now,
        });
        // A tilt measurement of an older design says nothing about this one.
        if let Some(run) = &self.run
            && run.tilt
            && run.generation != generation
        {
            run.handle.cancel();
        }
    }

    /// "Measure Tilt": asks for a tilt measurement, or stops the one that runs.
    fn toggle_tilt(&mut self) {
        match &self.run {
            Some(run) if run.tilt => {
                run.handle.cancel();
                self.tilt_requested = false;
            }
            _ => self.tilt_requested = !self.tilt_requested,
        }
    }

    /// One look at everything; see the module doc comment.
    fn tick(&mut self, now: Instant) {
        let Some(ui) = self.ui.as_ref().and_then(Weak::upgrade) else {
            return;
        };
        let Some(editor) = self.editor.clone() else {
            return;
        };
        // A Baseline replaced by a path that did not come through `install` (the guide's
        // reference design) or released with its window: its figures are not the old ones.
        let (epoch, present) = baseline_marker();
        if epoch != self.baseline.epoch {
            self.baseline = BaselineFigures::new(epoch, Origin::Set, now, None);
        }
        self.baseline.present = present;
        let editor_model = ui.global::<EditorModel>();
        // "Compare" in the command bar waits for it.
        editor_model.set_has_snapshot(present);
        let Ok(editor_state) = editor.try_borrow() else {
            return;
        };
        let generation = editor_state.current_generation();
        drop(editor_state);
        // The Tune mini sweep (sweep_state 1 = drawing) holds cores too.
        let busy_elsewhere = editor_model.get_optimize_running()
            || editor_model.get_deep_solve_running()
            || ui.global::<SweepModel>().get_running()
            || ui.global::<TuneModel>().get_sweep_state() == 1;
        let solving = editor_model.get_solve_running();

        self.poll_run();
        if self.run.is_none() && !busy_elsewhere {
            self.start_next(now, generation);
        }

        let behind = self
            .fed
            .as_ref()
            .is_none_or(|fed| fed.generation != generation);
        if !behind {
            self.stale_since = None;
        } else if self.stale_since.is_none() {
            self.stale_since = Some(now);
        }
        let stale = !solving
            && self
                .stale_since
                .is_some_and(|since| now.duration_since(since) >= STALE_GRACE);
        if stale && self.tilt_requested && self.run.is_none() {
            self.tilt_requested = false;
            self.flash = Some((
                "Solve the design first, then measure the tilt.".to_owned(),
                now,
            ));
        }
        if self
            .flash
            .as_ref()
            .is_some_and(|(_, at)| now.duration_since(*at) >= NOTE_HOLD)
        {
            self.flash = None;
        }

        let view = self.view(now, stale);
        if self.last_view.as_ref() != Some(&view) {
            present::push(&ui, &view);
            self.last_view = Some(view);
        }
    }

    /// Files the running job's figures when it is over.
    fn poll_run(&mut self) {
        let Some(mut run) = self.run.take() else {
            return;
        };
        match run.handle.poll() {
            Poll::Running => self.run = Some(run),
            Poll::Finished(scored) => self.land(&run, scored),
            Poll::Died => {
                self.tilt_requested = false;
            }
        }
    }

    /// Files `scored`, the figures of `run`.
    fn land(&mut self, run: &Run, scored: Scored) {
        if run.tilt {
            self.tilt_requested = false;
        }
        if scored.baseline == Part::Cancelled || scored.design == Part::Cancelled {
            return;
        }
        if run.scores_baseline
            && run.epoch == self.baseline.epoch
            && let Part::Scored(metrics) = scored.baseline
        {
            let moved_material = self
                .baseline
                .scored
                .as_ref()
                .is_some_and(|(key, _)| *key != run.key);
            if moved_material {
                self.baseline.rescored_for = Some(present::material_label(&run.key));
            }
            self.baseline.scored = Some((run.key.clone(), metrics));
        }
        if let Part::Scored(metrics) = scored.design {
            self.design = Some(DesignFigures {
                serial: run.serial,
                generation: run.generation,
                key: run.key.clone(),
                metrics,
            });
        }
    }

    /// Starts a job when the design now (or the Baseline, or a tilt measurement) needs one.
    fn start_next(&mut self, now: Instant, generation: u64) {
        let Some(fed) = self.fed.as_ref() else {
            return;
        };
        // The masts are those of an earlier design: wait for the solve of this one.
        if fed.generation != generation {
            return;
        }
        let tilt = self.tilt_requested;
        if !tilt && now.duration_since(fed.at) < DEBOUNCE {
            return;
        }
        let key = fed.design.material.clone();
        let design_done = self
            .design
            .as_ref()
            .is_some_and(|figures| figures.serial == fed.serial && figures.key == key);
        let baseline_done = !self.baseline.present
            || self
                .baseline
                .scored
                .as_ref()
                .is_some_and(|(scored_key, _)| *scored_key == key);
        if design_done && baseline_done && !tilt {
            return;
        }
        let baseline_part = (self.baseline.present && (!baseline_done || tilt))
            .then(baseline_clone)
            .flatten()
            .filter(|(epoch, _)| *epoch == self.baseline.epoch)
            .map(|(_, snapshot): (u64, DesignSnapshot)| (snapshot.design, snapshot.solved));
        let design_part = (!design_done || tilt).then(|| (fed.design.clone(), fed.solved.clone()));
        if baseline_part.is_none() && design_part.is_none() {
            return;
        }
        let Some(render_ctx) = self.render_ctx.as_ref() else {
            return;
        };
        let custom_materials = render_ctx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .custom_materials
            .as_ref()
            .clone();
        // A design that names no material is scored at its own refractive index, not as
        // diamond: the same defaulting Optimize and the sweep do.
        let mut material = key.clone();
        let _ = default_optimize_material_ri(&fed.design, &mut material, &custom_materials);
        let (serial, fed_generation) = (fed.serial, fed.generation);
        let scores_baseline = baseline_part.is_some();
        let job = Job {
            design: design_part,
            baseline: baseline_part,
            material,
            custom_materials,
            tilt,
        };
        // A refused thread leaves the strip as it is; the next tick tries again.
        if let Ok(handle) = worker::start(job) {
            self.run = Some(Run {
                handle,
                tilt,
                scores_baseline,
                serial,
                generation: fed_generation,
                epoch: self.baseline.epoch,
                key,
            });
        }
    }

    /// What the strip shows now.
    fn view(&self, now: Instant, stale: bool) -> View {
        let (cells, note) = self.cells_and_note();
        let note = match &self.flash {
            Some((text, _)) => text.clone(),
            None => note,
        };
        let label = if self.baseline.present {
            present::label_text(
                self.baseline.origin.label(),
                now.duration_since(self.baseline.since).as_secs(),
            )
        } else {
            String::new()
        };
        View {
            has_baseline: self.baseline.present,
            label,
            cells,
            stale,
            busy: self.run.is_some() || self.tilt_requested,
            measuring_tilt: self.tilt_requested || self.run.as_ref().is_some_and(|run| run.tilt),
            note,
        }
    }

    /// The figure cells, and the sentence under them: a re-scored Baseline, or why there are none.
    fn cells_and_note(&self) -> (Vec<CellView>, String) {
        let Some(design) = &self.design else {
            return (Vec::new(), String::new());
        };
        let Some((baseline_key, baseline_metrics)) = &self.baseline.scored else {
            let note = if self.baseline.present {
                "Scoring the Baseline...".to_owned()
            } else {
                String::new()
            };
            return (Vec::new(), note);
        };
        if *baseline_key != design.key {
            let note = format!(
                "Scoring the Baseline for {}...",
                present::material_label(&design.key)
            );
            return (Vec::new(), note);
        }
        match (baseline_metrics, &design.metrics) {
            (Some(was), Some(now)) => {
                let note = self
                    .baseline
                    .rescored_for
                    .as_ref()
                    .map(|material| format!("Baseline re-scored for {material}."))
                    .unwrap_or_default();
                (present::cells_of(&figure_deltas(was, now)), note)
            }
            (None, _) => (
                Vec::new(),
                "The Baseline gives no stone, so there is nothing to compare with.".to_owned(),
            ),
            (_, None) => (
                Vec::new(),
                "This design gives no stone, so there are no figures.".to_owned(),
            ),
        }
    }
}

/// Connects the window: the handles the tick reads, and the timer that calls it.
pub(super) fn attach(
    ui: &MainWindow,
    editor: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    HUB.with(|cell| {
        let mut hub = cell.borrow_mut();
        hub.ui = Some(ui.as_weak());
        hub.editor = Some(Rc::clone(editor));
        hub.render_ctx = Some(Arc::clone(render_ctx));
        // The first tick pushes a Baseline installed before the window was wired (the design a
        // startup restore opens).
        hub.timer.start(TimerMode::Repeated, TICK, tick_now);
    });
}

/// The timer's callback.
fn tick_now() {
    HUB.with(|cell| {
        // A tick that lands while the hub is borrowed (it is not, on one thread, unless a
        // callback of ours pumped the event loop) is skipped; the next one catches up.
        if let Ok(mut hub) = cell.try_borrow_mut() {
            hub.tick(Instant::now());
        }
    });
}

/// Lets go of the window and everything scored for it. See `gui::window_release`.
pub(super) fn release() {
    HUB.with(|cell| {
        if let Ok(mut hub) = cell.try_borrow_mut() {
            *hub = Hub::new();
        }
    });
}

/// Feeds the masts of a solve of `design` at `generation`: the strip scores them. A no-op until
/// the window is wired.
pub(super) fn feed_solved(design: &Design, solved: &[SolvedTier], generation: u64) {
    HUB.with(|cell| {
        if let Ok(mut hub) = cell.try_borrow_mut() {
            hub.feed(design, solved, generation, Instant::now());
        }
    });
}

/// The Baseline's figures and the material they were scored under; the figures are `None` for a
/// Baseline that gives no stone. `None` while the Baseline is not scored yet.
#[must_use]
pub(in crate::gui::editor) fn baseline_figures() -> Option<(MaterialSelection, Option<RowMetrics>)>
{
    HUB.with(|cell| {
        cell.try_borrow()
            .ok()
            .and_then(|hub| hub.baseline.scored.clone())
    })
}

/// The figures of the design as it is at `generation` and the material they were scored under;
/// `None` when the strip has not scored that generation (yet).
#[must_use]
pub(in crate::gui::editor) fn design_figures(
    generation: u64,
) -> Option<(MaterialSelection, Option<RowMetrics>)> {
    HUB.with(|cell| {
        let hub = cell.try_borrow().ok()?;
        hub.design
            .as_ref()
            .filter(|figures| figures.generation == generation)
            .map(|figures| (figures.key.clone(), figures.metrics))
    })
}

/// The masts the last solve produced for the design at `generation`; `None` for another
/// generation.
#[must_use]
pub(in crate::gui::editor) fn solved_masts(generation: u64) -> Option<Vec<SolvedTier>> {
    HUB.with(|cell| {
        let hub = cell.try_borrow().ok()?;
        hub.fed
            .as_ref()
            .filter(|fed| fed.generation == generation)
            .map(|fed| fed.solved.clone())
    })
}

/// "Measure Tilt" (or "Stop").
pub(super) fn toggle_tilt() {
    HUB.with(|cell| {
        if let Ok(mut hub) = cell.try_borrow_mut() {
            hub.toggle_tilt();
        }
    });
}

/// Holds `design` as the Baseline: `solved` are its masts when known, `generation` is the design
/// generation it was taken at (so the solve of that generation fills missing masts in), `label`
/// names it in the comparison.
pub(in crate::gui::editor) fn install_parts(
    origin: Origin,
    design: Design,
    solved: Option<Vec<SolvedTier>>,
    generation: Option<u64>,
    label: &str,
) {
    let awaiting = if solved.is_none() { generation } else { None };
    let epoch = install_baseline(DesignSnapshot {
        design,
        solved,
        label: label.to_string(),
        from_retarget_apply: false,
    });
    HUB.with(|cell| {
        if let Ok(mut hub) = cell.try_borrow_mut() {
            hub.baseline_installed(epoch, origin, awaiting);
        }
    });
}

/// Holds the design of `state`, as it is now, as the Baseline. Its masts arrive with the next
/// solve of this generation.
pub(in crate::gui::editor) fn install(origin: Origin, state: &EditorState) {
    install_parts(
        origin,
        state.design.clone(),
        None,
        Some(state.current_generation()),
        &design_label(state),
    );
}
