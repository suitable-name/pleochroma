//! The cancellable "Optimize" action, run off the UI thread.
//!
//! It follows the shape of `deep_solve.rs`: `thread::spawn`, an `Arc<AtomicBool>` cancel
//! flag and `Weak::upgrade_in_event_loop` progress.
//!
//! [`spawn_optimize_run`] is the Optimize tab's entry point. Retarget's Optimize mode has
//! its own worker in `callbacks::retarget_actions::optimize_run`.
//!
//! # Why not the `gui::batch` shape
//!
//! `gui::batch::tilt` and `gui::batch::batch_queue` distribute independent whole-design
//! work items across local lanes and a remote dispatcher. One design's coordinate search
//! is a single evolving state whose sweeps depend on each other, so it reuses only what
//! lives in `pleochroma_cut_core::optimize`: `evaluate_candidate_pair`'s two-thread
//! evaluation of a tier's `+step`/`-step` candidates. Remote dispatch would need
//! `pleochroma-net`'s wire protocol extended.
//!
//! # Real progress, not just elapsed time
//!
//! `deep_solve` reports only elapsed time, because its repair search has no loop this
//! crate controls. `optimize_design` runs this crate's own loop, so
//! [`OptimizeSolveProgress`] carries the real evaluation count (from
//! [`pleochroma_cut_core::optimize::SearchHooks::on_progress`]) alongside elapsed time.
//!
//! A ticker thread throttles how often the UI thread is asked to redraw. `on_progress` runs
//! after every tier decision and only stores into a shared [`AtomicUsize`]; the ticker
//! alone calls `Weak::upgrade_in_event_loop`.
//!
//! # Hinges are measured on the worker
//!
//! A run that varies anchored tiers needs each tier's hinge, which needs a solved design.
//! The worker solves once and measures them, because the UI thread must never solve
//! synchronously and its cached solve may be a frame behind the design.
//!
//! # Cancellation is a real mid-search checkpoint
//!
//! `optimize_design` polls `cancel` once per tier decision, so cancelling takes effect
//! within one `evaluate_candidate_pair` call. Unlike `deep_solve`, a cancelled run keeps
//! its partial [`OptimizeRunOutcome::Cancelled`] result rather than discarding it.

// Started by the `setup_optimize_*` callbacks in `callbacks::solve_actions`. A kept result
// goes through `pleochroma_editor::optimize_view::apply_candidate` (one undoable
// `Edit::ModifyTier` per changed tier), never straight into `Design`, so it stays in the
// undo history like every other edit.

use super::material_lookup::{EditorMaterialLookup, resolved_gem_material};
use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::{
    Design, DesignSolveError, MaterialSelection, OptimizeConfig, OptimizeOptions, OptimizeResult,
    SearchHooks, free_tier_indices_with,
    optimize::{SearchStage, StartProgress, inclusive_max_evaluations_for},
    optimize_design_with,
};
use pleochroma_editor::{
    material_lookup::{optimize_needs_model_width, sized_material_for_optimize},
    optimize_view::measure_anchor_hinges,
    solve_policy::design_to_gpu_planes_from_solved,
};
use slint::{ComponentHandle, Weak};
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Progress redraw interval. Cheap next to a search that takes hundreds of milliseconds to
/// hundreds of seconds (see `pleochroma_cut_core::optimize`'s "Cost first" section).
const TICK_INTERVAL: Duration = Duration::from_millis(250);

/// Handle returned by [`spawn_optimize_run`]. Cancelling is a checkpoint inside the search, see
/// the module docs.
pub struct OptimizeSolveHandle {
    cancel: Arc<AtomicBool>,
}

impl OptimizeSolveHandle {
    /// Requests cancellation. The search stops at its next checkpoint.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// One progress tick of an optimize run.
///
/// `max_evaluations` is [`inclusive_max_evaluations_for`]'s figure, which includes the polish
/// stages' caps, so the count never passes it. `stage` lets a caller show a stage name instead
/// of a stalled fraction during [`SearchStage::BaselineFull`] and [`SearchStage::FinalFull`].
#[derive(Debug, Clone, Copy)]
pub struct OptimizeSolveProgress {
    pub evaluations: usize,
    pub max_evaluations: usize,
    pub stage: SearchStage,
    /// The several-starts search's latest "start i of n, best ..." report; `None` for a
    /// single start and before the first report.
    pub start: Option<StartProgress>,
    pub elapsed: Duration,
}

/// The latest [`StartProgress`] as atomics the worker's hook writes and the ticker reads.
/// `count == 0` means no report yet.
#[derive(Default)]
struct StartCell {
    index: AtomicUsize,
    count: AtomicUsize,
    best_bits: AtomicU32,
}

impl StartCell {
    fn store(&self, progress: StartProgress) {
        self.index.store(progress.index, Ordering::Relaxed);
        self.best_bits
            .store(progress.best_fast_score.to_bits(), Ordering::Relaxed);
        // Written last: a reader that sees a count sees an index no older than it.
        self.count.store(progress.count, Ordering::Relaxed);
    }

    fn load(&self) -> Option<StartProgress> {
        let count = self.count.load(Ordering::Relaxed);
        (count > 0).then(|| StartProgress {
            index: self.index.load(Ordering::Relaxed),
            count,
            best_fast_score: f32::from_bits(self.best_bits.load(Ordering::Relaxed)),
        })
    }
}

/// The "start K of N" report worth showing at `stage`: dropped once the polish (and the final
/// scoring after it) is running, where the stored report is stale; kept for screening and the
/// coordinate stage.
const fn start_for_stage(
    stage: SearchStage,
    start: Option<StartProgress>,
) -> Option<StartProgress> {
    match stage {
        SearchStage::Polish | SearchStage::FinalFull => None,
        _ => start,
    }
}

/// What a run of the Optimize tab produced: the whole [`OptimizeResult`] (the best outcome,
/// the ranked candidates and the mast changes).
pub enum OptimizeRunOutcome {
    /// The search finished.
    Completed { result: OptimizeResult },
    /// Cancelled. `result` is the partial result found before the checkpoint fired.
    Cancelled { result: OptimizeResult },
    /// `design` did not solve: a block lacks a
    /// [`pleochroma_cut_core::design::MeetConstraint::ScaleReference`] anchor, or a
    /// [`pleochroma_cut_core::TierTarget`] cannot be resolved. The search never started.
    Failed { error: DesignSolveError },
}

/// Everything [`spawn_optimize_run`]'s worker thread needs, moved onto it. Bundled so the
/// function signature stays short.
pub struct OptimizeRunRequest {
    pub design: Design,
    pub material_selection: MaterialSelection,
    /// The catalogue's custom materials when Optimize was launched. The objective uses the
    /// same resolved material as the viewport (via [`resolved_gem_material`]), including any
    /// `refractive_index_override`.
    pub custom_materials: Vec<GemMaterial>,
    pub config: OptimizeConfig,
    /// What may change beyond the search knobs. With `vary_anchored` on, the worker fills
    /// `anchor_hinges` itself.
    pub options: OptimizeOptions,
    /// The tiers anchored variation is limited to ("Only selected tiers"); `None` means
    /// every tier that may vary.
    pub only_tiers: Option<BTreeSet<usize>>,
}

/// Spawns the search worker and its progress ticker off the UI thread.
///
/// `on_progress` runs on the UI event loop about every [`TICK_INTERVAL`] while the search runs;
/// `on_done` runs once with the final outcome. Generic over the UI handle so this module does not
/// depend on `crate::MainWindow`, as in `deep_solve::spawn_deep_solve`.
pub fn spawn_optimize_run<T, P, D>(
    ui_weak: Weak<T>,
    request: OptimizeRunRequest,
    on_progress: P,
    on_done: D,
) -> OptimizeSolveHandle
where
    T: ComponentHandle + 'static,
    P: Fn(&T, OptimizeSolveProgress) + Send + 'static + Clone,
    D: FnOnce(&T, OptimizeRunOutcome) + Send + 'static,
{
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_ticker = Arc::clone(&cancel);
    let cancel_worker = Arc::clone(&cancel);
    let done_flag = Arc::new(AtomicBool::new(false));
    let done_ticker = Arc::clone(&done_flag);
    let evaluations_done = Arc::new(AtomicUsize::new(0));
    let evaluations_ticker = Arc::clone(&evaluations_done);
    // Shown until the worker's first report arrives.
    let stage_done = Arc::new(AtomicU8::new(SearchStage::BaselineFull.to_code()));
    let stage_ticker = Arc::clone(&stage_done);
    let start_done = Arc::new(StartCell::default());
    let start_ticker = Arc::clone(&start_done);
    let ticker_ui = ui_weak.clone();
    // Covers screening, the coordinate stage and each kept start's polish. The worker
    // refines the free-tier count once the hinges are known.
    let max_evaluations = Arc::new(AtomicUsize::new(inclusive_max_evaluations_for(
        &request.config,
        request.options.keep_candidates,
        free_tier_indices_with(&request.design, &request.options).len(),
    )));
    let max_evaluations_ticker = Arc::clone(&max_evaluations);

    // Ticker: throttles UI redraws. It reads the real evaluation count, never an estimate.
    thread::spawn(move || {
        let start = Instant::now();
        loop {
            thread::sleep(TICK_INTERVAL);
            if done_ticker.load(Ordering::Relaxed) || cancel_ticker.load(Ordering::Relaxed) {
                break;
            }
            let stage = SearchStage::from_code(stage_ticker.load(Ordering::Relaxed));
            let progress = OptimizeSolveProgress {
                evaluations: evaluations_ticker.load(Ordering::Relaxed),
                max_evaluations: max_evaluations_ticker.load(Ordering::Relaxed),
                stage,
                start: start_for_stage(stage, start_ticker.load()),
                elapsed: start.elapsed(),
            };
            let on_progress = on_progress.clone();
            let _ = ticker_ui.upgrade_in_event_loop(move |ui| {
                on_progress(&ui, progress);
            });
        }
    });

    thread::spawn(move || {
        let OptimizeRunRequest {
            design,
            material_selection,
            custom_materials,
            config,
            mut options,
            only_tiers,
        } = request;
        let lookup = EditorMaterialLookup::new(&custom_materials);
        let material = resolved_gem_material(&material_selection, &lookup);
        let hooks = SearchHooks {
            cancel: Some(&cancel_worker),
            on_progress: Some(&|evaluations: usize, stage: SearchStage| {
                evaluations_done.store(evaluations, Ordering::Relaxed);
                stage_done.store(stage.to_code(), Ordering::Relaxed);
            }),
            on_start: Some(&|progress: StartProgress| start_done.store(progress)),
        };
        let result = prepare_run_inputs(&design, material, &mut options, only_tiers.as_ref())
            .and_then(|material| {
                max_evaluations.store(
                    inclusive_max_evaluations_for(
                        &config,
                        options.keep_candidates,
                        free_tier_indices_with(&design, &options).len(),
                    ),
                    Ordering::Relaxed,
                );
                optimize_design_with(&design, &material, &config, &options, &hooks)
            });
        done_flag.store(true, Ordering::Relaxed);
        let cancelled = cancel_worker.load(Ordering::Relaxed);
        let outcome = run_outcome_for(cancelled, result);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            on_done(&ui, outcome);
        });
    });

    OptimizeSolveHandle { cancel }
}

/// The worker's one solve. Measures the hinges a run varying anchored tiers needs, and sizes
/// `material` by the design's girdle diameter as the render does (see
/// [`sized_material_for_optimize`]), so the tone objective's face-up color matches the render.
/// The solve is skipped when neither is needed.
///
/// # Errors
///
/// [`DesignSolveError`] when the design does not solve, the same error the search would give.
fn prepare_run_inputs(
    design: &Design,
    material: GemMaterial,
    options: &mut OptimizeOptions,
    only: Option<&BTreeSet<usize>>,
) -> Result<GemMaterial, DesignSolveError> {
    options.only_tiers = only.cloned();
    let wants_size = design
        .girdle_diameter_mm
        .is_some_and(|mm| mm > 0.0 && mm.is_finite());
    if !options.vary_anchored && !wants_size {
        // Skip the solve unless the material reads the model width. Every unit reads it today,
        // so `optimize_needs_model_width` is always true and this shortcut is never taken; the
        // gate stays for a unit that stops needing the width. A color pattern makes the
        // material per-mm, so it is judged after the pattern.
        if !optimize_needs_model_width(&material, design) {
            return Ok(sized_material_for_optimize(material, design, &[]));
        }
    }
    let solved = design.solve()?;
    if options.vary_anchored {
        measure_anchor_hinges(options, design, &solved, only);
    }
    let planes = design_to_gpu_planes_from_solved(design, &solved);
    Ok(sized_material_for_optimize(material, design, &planes))
}

/// Maps the worker's result to the UI outcome. A cancelled run keeps its partial
/// [`OptimizeResult`]. A solve failure wins even when cancelled, since it was never searched.
fn run_outcome_for(
    cancelled: bool,
    result: Result<OptimizeResult, DesignSolveError>,
) -> OptimizeRunOutcome {
    match result {
        Err(error) => OptimizeRunOutcome::Failed { error },
        Ok(result) if cancelled => OptimizeRunOutcome::Cancelled { result },
        Ok(result) => OptimizeRunOutcome::Completed { result },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::{AngleChange, MissingAnchor, ObjectiveComponents, OptimizeOutcome};

    #[test]
    fn the_start_prefix_is_dropped_during_polish_and_kept_for_coordinate() {
        let start = Some(StartProgress {
            index: 2,
            count: 5,
            best_fast_score: 1.0,
        });
        assert!(start_for_stage(SearchStage::Coordinate, start).is_some());
        assert!(start_for_stage(SearchStage::Screening, start).is_some());
        assert!(start_for_stage(SearchStage::Polish, start).is_none());
        assert!(start_for_stage(SearchStage::FinalFull, start).is_none());
    }

    fn dummy_outcome(changed: bool) -> OptimizeOutcome {
        let components = ObjectiveComponents {
            windowing_pct: 10.0,
            extinction_pct: 10.0,
            tilt_brilliance_pct: 80.0,
        };
        OptimizeOutcome {
            provenance: None,
            before: components,
            before_score: 10.0,
            before_yield_loss_pct: 0.0,
            after: components,
            after_score: 10.0,
            after_yield_loss_pct: 0.0,
            evaluations: 4,
            changes: if changed {
                vec![AngleChange {
                    index: 0,
                    from_deg: 30.0,
                    to_deg: 31.0,
                }]
            } else {
                Vec::new()
            },
            cancelled: false,
            polish_evaluations: 0,
            polish_improvement: 0.0,
        }
    }

    fn dummy_result(changed: bool) -> OptimizeResult {
        OptimizeResult {
            outcome: dummy_outcome(changed),
            mast_changes: Vec::new(),
            candidates: Vec::new(),
            tone_before: None,
            tone_goal: None,
            lighting: pleochroma_cut_core::CANONICAL_LIGHTING_PRESET,
            starts_run: 1,
            best_start: 0,
        }
    }

    // --- OptimizeSolveHandle::cancel ---

    #[test]
    fn cancel_sets_the_flag_the_worker_and_ticker_threads_poll() {
        let flag = Arc::new(AtomicBool::new(false));
        let handle = OptimizeSolveHandle {
            cancel: Arc::clone(&flag),
        };
        assert!(!flag.load(Ordering::Relaxed));
        handle.cancel();
        assert!(flag.load(Ordering::Relaxed));
    }

    // --- run_outcome_for ---

    #[test]
    fn run_outcome_for_reports_failed_regardless_of_cancelled_when_the_design_does_not_solve() {
        // A `MissingAnchor` can happen whether or not the user also cancelled --
        // solve failure takes priority since there is no partial search outcome to
        // report at all in that case.
        let error = DesignSolveError::MissingAnchor(MissingAnchor {
            blocks: vec![pleochroma::geometry::meet_solver::Block::Crown],
        });
        for cancelled in [true, false] {
            let outcome = run_outcome_for(cancelled, Err(error.clone()));
            assert!(matches!(outcome, OptimizeRunOutcome::Failed { .. }));
        }
    }

    #[test]
    fn run_outcome_for_preserves_the_real_partial_result_when_cancelled() {
        let outcome = run_outcome_for(true, Ok(dummy_result(true)));
        match outcome {
            OptimizeRunOutcome::Cancelled { result } => {
                assert_eq!(result.outcome.changes.len(), 1);
                assert_eq!(result.outcome.evaluations, 4);
            }
            _ => panic!("expected Cancelled"),
        }
    }

    #[test]
    fn run_outcome_for_reports_completed_when_not_cancelled() {
        let outcome = run_outcome_for(false, Ok(dummy_result(false)));
        assert!(matches!(outcome, OptimizeRunOutcome::Completed { .. }));
    }

    // The hinge measurement itself (`prepare_anchor_hinges`) is shared with the command line
    // and is tested in `pleochroma_editor::optimize_view`.

    // --- the girdle-diameter size rule ---

    #[test]
    fn optimize_job_sizes_the_material_by_the_girdle_diameter() {
        let mut design = pleochroma_editor::EditorSession::fresh().design;
        let coloured = || {
            GemMaterial::sapphire().with_body_color(
                pleochroma::optics::materials::body_color::BODY_COLOR_PRESETS[1].absorption_rgb,
            )
        };
        let mut options = OptimizeOptions::default();
        let unsized_material = prepare_run_inputs(&design, coloured(), &mut options, None)
            .expect("the fresh design solves");
        let face_up = f64::from(pleochroma::render_setup::MODEL_UNIT_FACE_UP_PATH);
        // A per-model-unit scale is `(mm / 7) / K * (REFERENCE_MODEL_WIDTH / w)`. The measured
        // width `w` (near the reference 2.0) sets the expectation, not a bare `1 / K`.
        let solved = design.solve().expect("the fresh design solves");
        let planes = design_to_gpu_planes_from_solved(&design, &solved);
        let width = pleochroma::render_setup::measure_model_width(&planes)
            .expect("the fresh design measures");
        let ratio = pleochroma::render_setup::REFERENCE_MODEL_WIDTH / width;
        let close = |actual: f32, wanted: f64| (f64::from(actual) - wanted).abs() <= 1e-6 * wanted;
        // No girdle diameter: the unset size is the 7 mm the colour was tuned for, so `7 / 7`.
        assert!(
            close(unsized_material.absorption_path_scale, ratio / face_up),
            "{}",
            unsized_material.absorption_path_scale
        );

        design.girdle_diameter_mm = Some(6.5);
        let sized = prepare_run_inputs(&design, coloured(), &mut options, None)
            .expect("the fresh design solves");
        // A per-model-unit colour is tuned for 7 mm: 6.5 mm renders 6.5/7 as dense, on top of the
        // face-up calibration `1 / K` and the width rescale.
        assert!(
            close(sized.absorption_path_scale, 6.5 / 7.0 / face_up * ratio),
            "{}",
            sized.absorption_path_scale
        );
    }

    #[test]
    fn a_patterned_stone_is_sized_by_the_model_width_even_without_a_girdle_diameter() {
        use pleochroma_cut_core::material::pattern::{PatternAxis, TemplateId, template};
        let mut design = pleochroma_editor::EditorSession::fresh().design;
        let coloured = || {
            GemMaterial::sapphire().with_body_color(
                pleochroma::optics::materials::body_color::BODY_COLOR_PRESETS[1].absorption_rgb,
            )
        };
        // The pattern's zones are per-mm, so the stone is sized like a per-mm material although
        // the base material above is per-model-unit.
        let mut pattern = template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL);
        for at in pattern.unsolved_colors() {
            assert!(pattern.set_color_bands(at, vec![[480.0, 2100.0, 0.22], [620.0, 38.0, 0.05]]));
        }
        design.material.color_pattern_override = Some(pattern);
        design.girdle_diameter_mm = None;
        let mut options = OptimizeOptions::default();
        assert!(
            !options.vary_anchored,
            "the no-solve shortcut is the path under test"
        );
        assert!(optimize_needs_model_width(&coloured(), &design));

        let solved = design.solve().expect("the fresh design solves");
        let planes = design_to_gpu_planes_from_solved(&design, &solved);
        let expected = sized_material_for_optimize(coloured(), &design, &planes);
        let got = prepare_run_inputs(&design, coloured(), &mut options, None)
            .expect("the fresh design solves");
        assert!(
            (got.absorption_path_scale - expected.absorption_path_scale).abs() < 1e-7,
            "{} against {}",
            got.absorption_path_scale,
            expected.absorption_path_scale
        );
        // Not the per-model-unit scale the unpatterned base would have got.
        let model_unit = 1.0 / pleochroma::render_setup::MODEL_UNIT_FACE_UP_PATH;
        assert!((got.absorption_path_scale - model_unit).abs() > 1e-6);
    }

    // --- OptimizeSolveProgress ---

    #[test]
    fn progress_is_copy_not_just_clone() {
        // `spawn_optimize_run`'s ticker thread hands each tick a fresh
        // `OptimizeSolveProgress` by value across the `upgrade_in_event_loop`
        // boundary -- `Copy` (not just `Clone`) is load-bearing there, same as
        // `deep_solve::DeepSolveProgress`.
        let progress = OptimizeSolveProgress {
            evaluations: 12,
            max_evaluations: 200,
            stage: SearchStage::Coordinate,
            start: None,
            elapsed: Duration::from_secs(1),
        };
        let copied = progress;
        assert_eq!(copied.evaluations, progress.evaluations);
    }

    // --- material resolution (via `super::material_lookup::resolved_gem_material`) ---

    #[test]
    fn optimize_job_resolves_no_material_selection_to_diamond() {
        // A new design's `MaterialSelection::none()` has no name, so the objective must still
        // resolve to a real material: diamond, via `MaterialSelection::resolve`'s fallback.
        // The run must not fail over this.
        let lookup = EditorMaterialLookup::new(&[]);
        let material = resolved_gem_material(&MaterialSelection::none(), &lookup);
        assert_eq!(material.name, GemMaterial::diamond().name);
    }
}
