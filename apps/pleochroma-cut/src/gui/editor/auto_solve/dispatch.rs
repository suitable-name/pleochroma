//! Spawning a background solve of a `Design` snapshot and ticking its "Solving..."
//! banner. Also re-exports the cancellable-solve and plane-cap helpers.

use super::{
    apply::{BackgroundSolveResult, apply_background_solve_result},
    runtime::{PendingDispatch, RUNTIME},
};
use crate::{
    EditorModel, EditorTierItem, MainWindow,
    bridge::render_thread::RenderContext,
    gui::editor::state::{
        apply_multi_selection, design_to_gpu_planes, manufacturability_warnings_tagged,
        status_text_and_is_problem, status_text_and_is_problem_from_solved, tier_items,
        tier_items_from_solved, yield_report_texts, yield_report_texts_from_solved,
    },
};
use pleochroma::{
    geometry::{
        GpuFacetPlane, ToolPrimitive,
        meet_solver::{SolveError, SolvedTier},
    },
    optics::materials::GemMaterial,
};
use pleochroma_cut_core::{Design, DesignSolveError};
use pleochroma_editor::solve_policy::SOLVING_TICK_INTERVAL as TICK_INTERVAL;
use slint::{ComponentHandle, Weak};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

// The cancellable solve, the plane-cap diagnosis and the "Solving..." banner text
// live in `pleochroma_editor::solve_policy`, shared with the web app. `TICK_INTERVAL`
// is the banner's refresh period.
pub(super) use pleochroma_editor::solve_policy::solving_banner;
pub(in crate::gui::editor) use pleochroma_editor::solve_policy::{
    likely_hit_plane_cap, solve_cancellably, too_many_planes_message,
};

/// Invalidates the background solve in flight or queued, and stops its worker.
///
/// Bumping `current_seq` makes the worker's completion fail its `is_current(seq)`
/// check, so [`super::apply::apply_background_solve_result`] only frees the in-flight
/// slot. Setting `Runtime::current_cancel` makes the worker stop at its next cancel
/// point in `pleochroma::geometry::meet_solver`, typically within milliseconds.
/// Drops any queued [`PendingDispatch`], the debounced auto-solve timer, and the idle
/// replan timer, so no stale request restarts the solve that was just cancelled.
///
/// `last_solve` and `current_design` are kept: cancelling does not change which design
/// is loaded.
///
/// The UI half of "Abandon Solve" is `gui::editor::setup::setup_solve_cancel_callback`,
/// which calls this function first.
pub(in crate::gui::editor) fn cancel_in_flight_solve() {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        if let Some(cancel) = rt.current_cancel.as_ref() {
            cancel.store(true, Ordering::Relaxed);
        }
        rt.current_seq += 1;
        rt.debounce = None;
        rt.pending_dispatch = None;
        rt.idle_replan = None;
    });
}

/// [`cancel_in_flight_solve`] plus the `EditorModel` reset that "Abandon Solve" applies.
/// This is the cancel closure of the background-solve activity, so cancelling from the
/// status strip behaves the same as the command bar button.
fn cancel_in_flight_solve_from_activity(ui_weak: &Weak<MainWindow>) {
    cancel_in_flight_solve();
    let Some(ui) = ui_weak.upgrade() else {
        return;
    };
    let model = ui.global::<EditorModel>();
    model.set_solve_running(false);
    model.set_solve_state("stale".into());
    model.set_status_text("Solve abandoned -- click Solve when you are ready.".into());
    model.set_status_is_problem(true);
}

/// `true` iff `seq` is still `Runtime::current_seq`, i.e. this dispatch still owns the
/// "Solving..." banner.
pub(super) fn is_current(seq: u64) -> bool {
    RUNTIME.with(|cell| cell.borrow().current_seq == seq)
}

/// Everything [`dispatch_background_solve`]'s worker derives from one solve, so the
/// two branches live in one named place rather than inline in an already-long
/// function.
struct PanelInputs {
    tiers: Vec<EditorTierItem>,
    status_text: String,
    status_is_problem: bool,
    /// `(tier index, warning text)` pairs, so each warning can be tied to its tier row.
    warnings: Vec<(usize, String)>,
    yield_texts: (String, String, String, String),
    planes: Vec<GpuFacetPlane>,
    /// The frosted planes among `planes` (`Design::frosted_planes_from_solved`); empty for
    /// every design without frosting, and for a design that does not solve.
    frosted: Vec<u32>,
    /// The concave tools and placements that go with `planes`; empty for a planar design.
    tools: Vec<ToolPrimitive>,
    placements: Vec<(usize, usize)>,
    /// See [`BackgroundSolveResult::too_many_planes`]. Computed on the worker thread.
    too_many_planes: bool,
}

/// Builds every panel readout from one `Design::solve`, so the helpers are not each
/// solving again.
///
/// `solved` is `None` only when the design does not solve. The helpers' own solving
/// forms then run, and they fail early at the missing anchor, so this costs little.
///
/// `custom_materials` is a dispatch-time snapshot of `RenderContext::custom_materials`.
/// `Design::effective_refractive_index_with` uses it so a custom material (for example
/// "Garnet 1.74") gives the margin badges its real index; the built-in-only lookup
/// would give the wrong one.
///
/// `custom_sg` is the matching snapshot of `RenderContext::custom_material_specific_gravity`,
/// passed on to the yield report helpers.
fn panel_inputs(
    design: &Design,
    solved: Option<&Vec<SolvedTier>>,
    custom_materials: &[GemMaterial],
    custom_sg: &[(String, f64)],
) -> PanelInputs {
    let n_d = design.effective_refractive_index_with(custom_materials);
    // The same cheap-then-
    // authoritative check `state::status_text_and_is_problem*` runs internally for
    // its OWN status text, run once more here purely to hand
    // `apply::apply_background_solve_result` a plain `bool` for the once-per-design
    // toast -- see `BackgroundSolveResult::too_many_planes`'s own doc comment.
    // Safe to call `too_many_planes_message` (a real `solve_with`) unconditionally
    // behind the cheap pre-filter here: this whole function runs on the
    // background worker thread (`solve_and_build_result`'s own caller), never the
    // UI thread.
    let too_many_planes = solved.is_some_and(|solved| likely_hit_plane_cap(solved))
        && too_many_planes_message(design).is_some();
    let (tools, placements) =
        super::replan::design_to_gpu_tools_from_solved(design, solved.map(Vec::as_slice));
    let Some(solved) = solved else {
        let (status_text, status_is_problem) = status_text_and_is_problem(design);
        return PanelInputs {
            tiers: tier_items(design, n_d),
            status_text,
            status_is_problem,
            warnings: manufacturability_warnings_tagged(design, None),
            yield_texts: yield_report_texts(design, custom_sg),
            planes: design_to_gpu_planes(design),
            frosted: Vec::new(),
            tools,
            placements,
            too_many_planes,
        };
    };
    let (status_text, status_is_problem) = status_text_and_is_problem_from_solved(design, solved);
    PanelInputs {
        tiers: tier_items_from_solved(design, solved, n_d),
        status_text,
        status_is_problem,
        warnings: manufacturability_warnings_tagged(design, Some(solved)),
        yield_texts: yield_report_texts_from_solved(design, solved, custom_sg),
        planes: super::replan::design_to_gpu_planes_from_solved(design, solved),
        frosted: design.frosted_planes_from_solved(solved),
        tools,
        placements,
        too_many_planes,
    }
}

/// Spawns the thread that refreshes the "Solving..." banner's elapsed time. Returns
/// the `done` flag the caller sets when the solve finishes; the thread stops on its
/// next tick.
fn spawn_solving_ticker(ui: &MainWindow, seq: u64, tier_count: usize) -> Arc<AtomicBool> {
    let done_flag = Arc::new(AtomicBool::new(false));
    let done_ticker = Arc::clone(&done_flag);
    let ticker_ui = ui.as_weak();
    thread::spawn(move || {
        let start = Instant::now();
        loop {
            thread::sleep(TICK_INTERVAL);
            if done_ticker.load(Ordering::Relaxed) {
                break;
            }
            let elapsed = start.elapsed();
            let _ = ticker_ui.upgrade_in_event_loop(move |ui| {
                // A superseded dispatch's ticker has nothing left worth touching --
                // the newer dispatch owns the banner now.
                if is_current(seq) {
                    ui.global::<EditorModel>()
                        .set_status_text(solving_banner(tier_count, elapsed).into());
                }
            });
        }
    });
    done_flag
}

/// The "solve already in flight" guard of [`dispatch_background_solve`]. When a solve
/// is running, stores `design`/`generation`/`multi_selected` as the [`PendingDispatch`]
/// (replacing any earlier one) and returns `true`. Otherwise claims the in-flight slot
/// for `cancel` and returns `false`.
///
/// The queued entry keeps the caller's `started_generation`, not a fresh reload of
/// `generation`. Reloading would pick up edits made after the snapshot and replay a
/// stale design as if it were current. See [`PendingDispatch::started_generation`].
pub(super) fn queue_or_claim_solve_slot(
    design: &Design,
    generation: &Arc<AtomicU64>,
    started_generation: u64,
    multi_selected: &BTreeSet<usize>,
    cancel: &Arc<AtomicBool>,
) -> bool {
    RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        if rt.solve_in_flight {
            rt.pending_dispatch = Some(PendingDispatch {
                design: design.clone(),
                started_generation,
                generation: Arc::clone(generation),
                multi_selected: multi_selected.clone(),
            });
            true
        } else {
            rt.solve_in_flight = true;
            rt.current_cancel = Some(Arc::clone(cancel));
            false
        }
    })
}

/// Registers this dispatch with the shared `ActivityRegistry`, so the status strip lists
/// it next to Deep Solve and Optimize. The progress is indeterminate, since
/// `meet_solver` reports only elapsed time. The cancel closure goes through
/// [`cancel_in_flight_solve_from_activity`], so it also resets the `EditorModel` fields.
fn register_solve_activity(ui: &MainWindow, tier_count: usize) {
    let activity_id = RUNTIME
        .with(|cell| cell.borrow().activity.clone())
        .map(|a| {
            let ui_weak = ui.as_weak();
            a.start(
                "solve",
                // The chip appends its own live "-- N.Ns", so the label has no elapsed time of its own.
                format!(
                    "Solving... ({tier_count} tier{})",
                    if tier_count == 1 { "" } else { "s" }
                ),
                Some(Box::new(move || {
                    cancel_in_flight_solve_from_activity(&ui_weak);
                })),
            )
        });
    RUNTIME.with(|cell| cell.borrow_mut().activity_id = activity_id);
}

/// Worker-thread body: solves `design` once and builds the panel inputs from that
/// solve. Returns a cancelled placeholder if the solve was cancelled.
fn solve_and_build_result(
    design: Design,
    cancel: &AtomicBool,
    multi_selected: &BTreeSet<usize>,
    custom_materials: &[GemMaterial],
    custom_sg: &[(String, f64)],
    start: Instant,
) -> BackgroundSolveResult {
    // `design` is solved once here. Every other value is built from that `solved`
    // list through its `_from_solved` helper, so no helper solves again.
    //
    // `solve_cancellably` checks `cancel` during the solve, so "Abandon Solve" stops
    // the work instead of only discarding its result.
    let solved_result = solve_cancellably(&design, cancel);
    let multi_selected_count = multi_selected.len();
    // The same effective RI `panel_inputs` uses; kept on the result for the
    // proportion-verdict chips.
    let n_d = design.effective_refractive_index_with(custom_materials);
    // A cancelled solve skips `panel_inputs`, whose extra solves cost as much as the
    // solve itself. `cancel` is also checked directly, so a cancel that arrives after
    // `solve_cancellably` returned still short-circuits.
    if matches!(
        solved_result,
        Err(DesignSolveError::Solve(SolveError::Cancelled))
    ) || cancel.load(Ordering::Relaxed)
    {
        return BackgroundSolveResult::cancelled(design, start.elapsed(), multi_selected_count);
    }
    let PanelInputs {
        mut tiers,
        status_text,
        status_is_problem,
        warnings,
        yield_texts,
        planes,
        frosted,
        tools,
        placements,
        too_many_planes,
    } = panel_inputs(
        &design,
        solved_result.as_ref().ok(),
        custom_materials,
        custom_sg,
    );
    apply_multi_selection(&mut tiers, multi_selected);
    let solved = solved_result.ok();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "gear reference angle (degrees) is stored as f32 in the panel model"
    )]
    let gear = (
        design.meta.gear_teeth_abs(),
        design.meta.gear_reference_angle as f32,
    );
    BackgroundSolveResult {
        elapsed: start.elapsed(),
        tiers,
        status_text,
        status_is_problem,
        warnings,
        yield_texts,
        planes,
        frosted,
        tools,
        placements,
        solved,
        too_many_planes,
        gear,
        multi_selected_count,
        n_d,
        panicked: false,
        // Moved in last, after every borrow of it is done.
        design,
    }
}

/// Spawns a background solve of `design` (a snapshot taken when this is called), plus
/// a ticker for the "Solving..." banner. Called by `view::refresh_all` for large designs
/// and by `scheduling::on_edit` for auto-solve.
///
/// `multi_selected` is a dispatch-time snapshot, so the worker can apply the multi-select
/// highlight to the tier rows. A toggle made after dispatch shows on the next refresh.
///
/// `started_generation` is the `generation` value at the moment `design` was
/// snapshotted. A replayed [`PendingDispatch`] passes its own captured value, because
/// the live counter may have moved on since.
pub(in crate::gui::editor) fn dispatch_background_solve(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    design: Design,
    generation: &Arc<AtomicU64>,
    multi_selected: BTreeSet<usize>,
    started_generation: u64,
) {
    // While a solve is in flight, this request is queued instead of starting a second
    // worker. `apply_background_solve_result` replays it when that solve completes.
    // `cancel` is stored only when a worker is actually spawned.
    let cancel = Arc::new(AtomicBool::new(false));
    if queue_or_claim_solve_slot(
        &design,
        generation,
        started_generation,
        &multi_selected,
        &cancel,
    ) {
        return;
    }

    let tier_count = design.tiers.len();
    let seq = RUNTIME.with(|cell| {
        let mut rt = cell.borrow_mut();
        rt.current_seq += 1;
        rt.current_seq
    });

    ui.global::<EditorModel>().set_solve_running(true);
    ui.global::<EditorModel>().set_solve_state("solving".into());
    ui.global::<EditorModel>()
        .set_status_text(solving_banner(tier_count, Duration::ZERO).into());
    ui.global::<EditorModel>().set_status_is_problem(false);
    register_solve_activity(ui, tier_count);

    let done_flag = spawn_solving_ticker(ui, seq, tier_count);

    // Snapshotted here on the UI thread, so the lock is not held across the solve.
    // `Arc` clones are cheap.
    let (custom_materials, custom_sg) = {
        let ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
        (
            Arc::clone(&ctx.custom_materials),
            Arc::clone(&ctx.custom_material_specific_gravity),
        )
    };

    let ui_weak = ui.as_weak();
    let render_ctx_for_apply = Arc::clone(render_ctx);
    let generation = Arc::clone(generation);
    let multi_selected_count = multi_selected.len();
    thread::spawn(move || {
        let start = Instant::now();
        // A panic must still free `Runtime::solve_in_flight`, or every later dispatch
        // would queue forever. The clone lets the panic path build a placeholder result.
        let design_for_panic = design.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            solve_and_build_result(
                design,
                &cancel,
                &multi_selected,
                &custom_materials,
                &custom_sg,
                start,
            )
        }))
        .unwrap_or_else(|_| {
            BackgroundSolveResult::panicked(design_for_panic, start.elapsed(), multi_selected_count)
        });
        // Always set, and always followed by a completion post, so every outcome frees
        // the in-flight slot.
        done_flag.store(true, Ordering::Relaxed);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            apply_background_solve_result(
                &ui,
                &render_ctx_for_apply,
                seq,
                started_generation,
                &generation,
                result,
            );
        });
    });
}
