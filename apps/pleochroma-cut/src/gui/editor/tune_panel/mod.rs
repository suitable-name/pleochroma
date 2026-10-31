//! The Optimize tab's Tune view: the design's tiers tuned by hand, one at a time
//! (`ui/components/editor_inspector/tune_tab.slint`, state in `ui/models/tune.slint`).
//!
//! Tune is the hand-driven partner of Optimize. It lists the tiers in a tuning order (pavilion
//! main, crown main, then the other crown tiers, the other pavilion tiers and the girdle tiers;
//! Advanced can reorder them for the session), and for the picked tier it draws a mini sweep of
//! the tier's angle, offers the nudge buttons, Pair With (the Angle Sweep dialog in Two Tiers
//! mode, prefilled), Accept, Revert This Tier and Next Tier.
//!
//! - [`order`] -- the tier list: which tiers, in which order, what "Next Tier" picks and the
//!   Baseline's angle of each tier. Slint-free.
//! - [`mini_sweep`] -- the coarse one-tier sweep: its plan, the worker thread, the cache and the
//!   reading of a drawn sweep (best, plateau, Jump to Best). Slint-free except the thread.
//! - [`present`] -- the rows, the chart's marks and the sentences, and the pushes into
//!   `TuneModel`.
//! - [`simple_mode`] -- what the Simple interface hides that keeps a value.
//! - [`actions`] -- the answers to the model's callbacks: every angle change goes through the
//!   same edit path the tier table and the Angle Sweep dialog use, one undo step each.
//! - [`selection`] -- the shared design-tier selection, mirrored from the editor and updated
//!   by Tune row clicks and Next Tier without changing the active inspector tab.
//!
//! # Threads and state
//!
//! Everything runs on the UI thread except the mini sweep's worker, which a timer polls. The
//! state is one [`Hub`] per window in a `thread_local!`, like the Baseline's and the Trials';
//! no borrow of it is held across a call into the editor or into Slint, except the timer's own
//! pushes, which call nothing that comes back here.
//!
//! The mini sweep is drawn for a [`mini_sweep::Basis`]: the design without the picked tier's own
//! angle. A nudge of the picked tier keeps the chart valid (its ring moves along the line); an
//! edit of any other tier makes it stale, and it is drawn again once the design has stayed
//! still for [`SETTLE`]. All sweeps run under the Grading tray, never the viewport's lighting.

mod actions;
mod metrics;
mod mini_sweep;
mod order;
mod present;
mod selection;
mod simple_mode;
#[cfg(test)]
mod tests;

use self::{
    mini_sweep::{ANGLE_EPSILON_DEG, Basis, Cache, Job, Poll, RunHandle, jump_target},
    order::{
        TierInfo, apply_custom_order, baseline_angles, next_untuned, tier_infos, tuning_order,
    },
    present::{
        ListInput, angle_display, chart_marks, clear_chart, figure_names, readout, title_text,
    },
};
use crate::{
    EditorModel, MainWindow, TuneModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            callbacks::{baseline_marker, snapshot_for_compare},
            state::EditorState,
        },
        show_toast,
        solid_preview::preview_state::{SolidLastSolved, SolidPreviewState},
    },
};
use pleochroma_cut_core::Design;
use pleochroma_editor::{
    optimize_view::default_optimize_material_ri,
    sweep::{SweepOutcome, default_worker_count, sweepable_tiers},
};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError, atomic::Ordering},
    time::{Duration, Instant},
};

/// How often the timer looks at the editor, the Tune view and the running mini sweep.
const TICK: Duration = Duration::from_millis(250);

/// How long the design must stay still before a stale mini sweep is drawn again.
const SETTLE: Duration = Duration::from_millis(500);

/// `EditorModel.inspector_tab` of the Optimize tab, which hosts the Tune view.
const OPTIMIZE_TAB: i32 = 2;

/// The heading of the Pair With combo.
const PAIR_HEADING: &str = "Pair With...";

thread_local! {
    /// The window's Tune state; see the module doc comment for why it is a `thread_local!`.
    static HUB: RefCell<Hub> = RefCell::new(Hub::new());
}

/// The handles the actions and the timer need.
struct Deps {
    state: Rc<RefCell<EditorState>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    preview_state: Arc<SolidPreviewState>,
    solid_last_solved: SolidLastSolved,
}

/// What the editor looked like at the last rebuild of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ListKey {
    epoch: u64,
    generation: u64,
    /// The Baseline's epoch and whether one is held.
    baseline: (u64, bool),
    advanced: bool,
    /// The shared selection, in design order (never a position in Tune's reordered list).
    selected: i32,
}

/// The mini sweep chart as last pushed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChartKey {
    /// Counts the sweeps shown.
    version: u64,
    figure: usize,
    /// The tier's angle, as a bit pattern.
    angle: u64,
}

/// What the window's Tune view knows.
struct Hub {
    ui: Option<Weak<MainWindow>>,
    deps: Option<Rc<Deps>>,
    /// Calls [`tick_now`]; stopped (dropped) with the window.
    timer: Timer,
    /// The design epoch last seen.
    epoch: u64,
    /// The design generation last seen, and when it last moved.
    generation: u64,
    changed_at: Instant,
    /// The listed tiers, in design order.
    infos: Vec<TierInfo>,
    /// Their design positions in the order they are listed.
    order: Vec<usize>,
    /// The order the cutter gave them (empty: the tuning order), kept for the design's session.
    custom_order: Vec<usize>,
    /// The Baseline's angle of every tier of the design.
    baseline: Vec<Option<f64>>,
    /// The tiers accepted in this session.
    accepted: BTreeSet<usize>,
    /// The shared editor selection, when that design tier is present in the Tune list.
    selected: Option<usize>,
    /// The other tiers Pair With offers, as the combo lists them after its heading.
    partners: Vec<usize>,
    /// The list or the selected panel has to be built again.
    dirty: bool,
    list_key: Option<ListKey>,
    // --- The mini sweep ---
    cache: Cache,
    /// The sweep on show (possibly stale) and what it is a sweep of.
    shown: Option<(Basis, SweepOutcome)>,
    /// Counts the sweeps shown.
    shown_version: u64,
    run: Option<(RunHandle, Basis)>,
    /// A basis whose sweep could not run: not tried again until the design changes.
    failed: Option<Basis>,
    chart_key: Option<ChartKey>,
    /// The chart row under the pointer.
    hover: Option<usize>,
    /// Presentation only: independent of the objective, sweep basis and cache.
    visible_metrics: metrics::Visibility,
}

impl Hub {
    fn new() -> Self {
        Self {
            ui: None,
            deps: None,
            timer: Timer::default(),
            epoch: 0,
            generation: 0,
            changed_at: Instant::now(),
            infos: Vec::new(),
            order: Vec::new(),
            custom_order: Vec::new(),
            baseline: Vec::new(),
            accepted: BTreeSet::new(),
            selected: None,
            partners: Vec::new(),
            dirty: true,
            list_key: None,
            cache: Cache::default(),
            shown: None,
            shown_version: 0,
            run: None,
            failed: None,
            chart_key: None,
            hover: None,
            visible_metrics: metrics::Visibility::default(),
        }
    }

    /// Another design opened: nothing of the one before applies.
    fn reset_for_design(&mut self, epoch: u64) {
        self.epoch = epoch;
        self.infos.clear();
        self.order.clear();
        self.custom_order.clear();
        self.baseline.clear();
        self.accepted.clear();
        self.selected = None;
        self.partners.clear();
        self.dirty = true;
        self.list_key = None;
        self.forget_sweeps();
    }

    /// Drops every drawn sweep and the running one.
    fn forget_sweeps(&mut self) {
        self.cache.clear();
        self.shown = None;
        self.run = None;
        self.failed = None;
        self.chart_key = None;
        self.hover = None;
    }

    /// One look at everything; see the module doc comment.
    fn tick(&mut self, now: Instant) {
        let Some(ui) = self.ui.as_ref().and_then(Weak::upgrade) else {
            return;
        };
        let Some(deps) = self.deps.clone() else {
            return;
        };
        let model = ui.global::<TuneModel>();
        if !model.get_active() || !tune_showing(&ui) {
            // Nobody looks: the worker stops, the cache stays.
            self.run = None;
            return;
        }
        let advanced = model.get_advanced();
        let (epoch, generation, has_design, design) = {
            let Ok(state) = deps.state.try_borrow() else {
                return;
            };
            (
                state.design_epoch.load(Ordering::Relaxed),
                state.current_generation(),
                state.has_design,
                state.design.clone(),
            )
        };
        if epoch != self.epoch {
            self.reset_for_design(epoch);
        }
        if generation != self.generation {
            self.generation = generation;
            self.changed_at = now;
        }
        let key = ListKey {
            epoch,
            generation,
            baseline: baseline_marker(),
            advanced,
            selected: ui.global::<EditorModel>().get_selected_tier_index(),
        };
        if self.dirty || self.list_key != Some(key) {
            self.rebuild(&ui, &design, has_design);
            self.list_key = Some(key);
            self.dirty = false;
        }
        self.drive_sweep(&ui, &deps, &design, advanced, now);
    }

    /// Builds the tier list and the selected tier's panel again.
    fn rebuild(&mut self, ui: &MainWindow, design: &Design, has_design: bool) {
        let model = ui.global::<TuneModel>();
        model.set_has_design(has_design);
        if !has_design {
            self.infos.clear();
            self.order.clear();
            self.selected = None;
            self.show_selection(ui, design);
            present::push_tiers(ui, Vec::new());
            model.set_status_text("Open a design to tune it.".into());
            return;
        }
        self.infos = tier_infos(design);
        let default_order = tuning_order(&self.infos);
        self.order = apply_custom_order(&default_order, &self.custom_order);
        let order_changed = self.order != default_order;
        let selected = selection::listed_tier(
            &self.infos,
            ui.global::<EditorModel>().get_selected_tier_index(),
        );
        if self.selected != selected {
            self.selected = selected;
            self.hover = None;
        }
        self.baseline = snapshot_for_compare().map_or_else(
            || vec![None; design.tiers.len()],
            |(before, _label)| baseline_angles(&before, design),
        );
        present::push_tiers(
            ui,
            present::tier_rows(&ListInput {
                infos: &self.infos,
                order: &self.order,
                baseline: &self.baseline,
                accepted: &self.accepted,
                selected: self.selected,
                reorderable: model.get_advanced(),
            }),
        );
        let status = if self.infos.iter().any(TierInfo::tunable) {
            ""
        } else {
            "No tier of this design has an angle that can be tuned."
        };
        model.set_status_text(status.into());
        model.set_hidden_in_use(
            self.visible_metrics != metrics::Visibility::default()
                || simple_mode::settings_in_use(
                    order_changed,
                    usize::try_from(model.get_figure_index()).unwrap_or(0),
                ),
        );
        self.show_selection(ui, design);
    }

    /// Fills the selected tier's panel: its title, angle box, buttons and Pair With entries.
    fn show_selection(&mut self, ui: &MainWindow, design: &Design) {
        let model = ui.global::<TuneModel>();
        let info = self
            .selected
            .and_then(|tier| self.infos.iter().find(|info| info.index == tier))
            .filter(|info| info.tunable());
        let Some(info) = info else {
            model.set_has_selection(false);
            model.set_title("".into());
            model.set_angle_text("".into());
            model.set_can_accept(false);
            model.set_can_revert(false);
            model.set_can_next(false);
            self.partners.clear();
            model.set_pair_names(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            return;
        };
        let angle = design
            .tiers
            .get(info.index)
            .map_or(info.angle_deg, |tier| tier.angle_deg);
        model.set_has_selection(true);
        model.set_title(title_text(info).into());
        model.set_angle_text(angle_display(angle).into());
        model.set_can_accept(true);
        let start = self.baseline.get(info.index).copied().flatten();
        model.set_can_revert(start.is_some_and(|start| (start - angle).abs() > ANGLE_EPSILON_DEG));
        let infos = &self.infos;
        model.set_can_next(
            next_untuned(
                &self.order,
                self.selected,
                &|tier| is_tunable(infos, tier),
                &self.accepted,
            )
            .is_some(),
        );
        let selected = info.index;
        self.partners = sweepable_tiers(design)
            .into_iter()
            .filter(|&tier| tier != selected)
            .collect();
        let mut names = vec![SharedString::from(PAIR_HEADING)];
        names.extend(self.partners.iter().map(|&tier| {
            self.infos
                .iter()
                .find(|other| other.index == tier)
                .map_or_else(
                    || SharedString::from(format!("Tier {}", tier + 1)),
                    |other| {
                        if other.name == other.code || other.code.is_empty() {
                            SharedString::from(other.name.as_str())
                        } else {
                            SharedString::from(format!("{} {}", other.code, other.name))
                        }
                    },
                )
        }));
        model.set_pair_names(ModelRc::new(VecModel::from(names)));
        model.set_pair_index(0);
    }

    /// Polls the running mini sweep, starts one when the picked tier needs it and shows what
    /// there is: the chart, its marks and the sweep's state.
    fn drive_sweep(
        &mut self,
        ui: &MainWindow,
        deps: &Deps,
        design: &Design,
        advanced: bool,
        now: Instant,
    ) {
        let model = ui.global::<TuneModel>();
        let tier = self
            .selected
            .filter(|&tier| advanced && is_tunable(&self.infos, tier));
        let Some(tier) = tier else {
            self.run = None;
            self.shown = None;
            model.set_sweep_state(0);
            model.set_sweep_stale(false);
            model.set_can_jump(false);
            if self.chart_key.take().is_some() || !model.get_chart_commands().is_empty() {
                clear_chart(ui, self.visible_metrics);
            }
            return;
        };
        let basis = Basis::of(design, self.epoch, tier);
        let progress = self.poll_run(ui);
        if self.shown.as_ref().map(|(shown, _)| shown) != Some(&basis)
            && let Some(outcome) = self.cache.get(&basis).cloned()
        {
            self.shown = Some((basis.clone(), outcome));
            self.shown_version += 1;
        }
        let fresh = self
            .shown
            .as_ref()
            .is_some_and(|(shown, _)| *shown == basis);
        let running = self.run.as_ref().is_some_and(|(_, run)| *run == basis);
        if !fresh && !running && self.failed.as_ref() != Some(&basis) {
            // A stale chart waits for the design to settle; no chart at all starts at once.
            let waiting = self.shown.is_some() && now.duration_since(self.changed_at) < SETTLE;
            if !waiting {
                self.start_sweep(ui, deps, design, &basis);
            }
        }
        self.show_sweep(ui, design, tier, fresh, progress);
    }

    /// Looks at the running sweep: its progress, or its outcome, which goes to the cache. Only
    /// a sweep of the basis now is shown; another one waits in the cache.
    fn poll_run(&mut self, ui: &MainWindow) -> f32 {
        let polled = self
            .run
            .as_mut()
            .map(|(run, basis)| (run.poll(), basis.clone(), run.total()));
        match polled {
            None => 0.0,
            Some((Poll::Running { done }, _, total)) => done as f32 / total.max(1) as f32,
            Some((Poll::Finished(outcome), basis, _)) => {
                self.run = None;
                if !outcome.cancelled {
                    self.cache.put(basis, *outcome);
                }
                1.0
            }
            Some((Poll::Died, basis, _)) => {
                self.run = None;
                self.failed = Some(basis);
                show_toast(ui, "The sweep stopped unexpectedly.", "error");
                0.0
            }
        }
    }

    /// Starts the mini sweep of `basis` on a worker thread.
    fn start_sweep(&mut self, ui: &MainWindow, deps: &Deps, design: &Design, basis: &Basis) {
        let plan = match mini_sweep::plan(design, basis.tier) {
            Ok(plan) => plan,
            Err(_refused) => {
                self.failed = Some(basis.clone());
                return;
            }
        };
        let custom_materials = deps
            .render_ctx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .custom_materials
            .as_ref()
            .clone();
        // A design that names no material is scored at its own refractive index, not as
        // diamond -- the same defaulting Optimize and the Angle Sweep do.
        let mut material = design.material.clone();
        let _ = default_optimize_material_ri(design, &mut material, &custom_materials);
        let job = Job {
            design: design.clone(),
            plan,
            material,
            custom_materials,
            // The window keeps its cores: half of what a dialog sweep takes.
            workers: (default_worker_count() / 2).max(1),
        };
        match mini_sweep::start(job) {
            Ok(handle) => self.run = Some((handle, basis.clone())),
            Err(message) => {
                self.failed = Some(basis.clone());
                show_toast(ui, &message, "error");
            }
        }
    }

    /// Pushes the sweep's state and, when it changed, the chart.
    fn show_sweep(
        &mut self,
        ui: &MainWindow,
        design: &Design,
        tier: usize,
        fresh: bool,
        progress: f32,
    ) {
        let model = ui.global::<TuneModel>();
        let figure = usize::try_from(model.get_figure_index())
            .unwrap_or(0)
            .min(mini_sweep::FIGURES.len() - 1);
        let current = design.tiers.get(tier).map_or(0.0, |found| found.angle_deg);
        let Some((_, outcome)) = &self.shown else {
            let drawing = self.run.is_some();
            model.set_sweep_state(i32::from(drawing));
            model.set_sweep_progress(progress);
            model.set_sweep_stale(false);
            model.set_can_jump(false);
            if self.chart_key.take().is_some() || !model.get_chart_commands().is_empty() {
                clear_chart(ui, self.visible_metrics);
            }
            return;
        };
        let valid = outcome.valid_count() > 0;
        model.set_sweep_state(if valid { 2 } else { 3 });
        model.set_sweep_progress(progress);
        model.set_sweep_stale(!fresh);
        model.set_can_jump(fresh && jump_target(&outcome.rows, figure, current).is_some());
        let key = ChartKey {
            version: self.shown_version,
            figure,
            angle: current.to_bits(),
        };
        if self.chart_key != Some(key) {
            self.chart_key = Some(key);
            self.hover = None;
            present::push_chart(ui, &chart_marks(&outcome.rows, figure, current));
            metrics::push(ui, &outcome.rows, self.visible_metrics);
            model.set_hover_x(-1.0);
            model.set_readout_text(
                readout(&outcome.rows, figure, None, self.visible_metrics).into(),
            );
        }
    }
}

/// Whether `tier` is a listed tier the cutter can tune.
fn is_tunable(infos: &[TierInfo], tier: usize) -> bool {
    infos
        .iter()
        .any(|info| info.index == tier && info.tunable())
}

/// Whether the Tune view is on screen: the Optimize tab is selected and not collapsed.
fn tune_showing(ui: &MainWindow) -> bool {
    let editor = ui.global::<EditorModel>();
    editor.get_inspector_tab() == OPTIMIZE_TAB && !editor.get_inspector_collapsed()
}

/// Runs `action` on the hub; `None` when the hub is borrowed (a callback of ours pumped the
/// event loop).
fn with_hub<R>(action: impl FnOnce(&mut Hub) -> R) -> Option<R> {
    HUB.with(|cell| cell.try_borrow_mut().ok().map(|mut hub| action(&mut hub)))
}

/// The timer's callback, and the actions' way to show what they changed at once.
fn tick_now() {
    let _ = with_hub(|hub| hub.tick(Instant::now()));
}

/// The editor table, Solid and Diagram share a selection; update Tune after the normal handler.
pub(super) fn selection_changed() {
    tick_now();
}

/// The window and the handles of the hub, for an action; the hub is not borrowed afterwards.
fn handles() -> Option<(MainWindow, Rc<Deps>)> {
    with_hub(|hub| {
        let ui = hub.ui.as_ref().and_then(Weak::upgrade)?;
        Some((ui, hub.deps.clone()?))
    })
    .flatten()
}

/// Wires the Tune view's callbacks and starts the timer that watches the editor and the mini
/// sweep.
pub(super) fn setup_tune(
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
    let model = ui.global::<TuneModel>();
    model.set_figure_names(ModelRc::new(VecModel::from(figure_names())));
    metrics::push(ui, &[], metrics::Visibility::default());
    actions::wire(ui);
}

/// Lets go of the window's Tune state (timer, the running sweep, the cache). See
/// `gui::window_release`.
pub(super) fn release_window_state() {
    let _ = with_hub(|hub| *hub = Hub::new());
}
