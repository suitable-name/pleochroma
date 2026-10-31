//! The one scoring path of a finished design: solve, check it closes, measure it, count
//! what is hard to cut, score it under the lighting and (on request) average the tilt.
//!
//! The one-tier sweep, the two-tier sweep and the live deltas all score through here, so a
//! figure never depends on which of them asked for it.
//!
//! The stone is scored as the render shows it: its concave tools cut out, and its frosted
//! facets and frosted girdle ([`Design::facet_finishes_from_solved`] of the design being
//! scored) scattering the light. A polished design has no finishes and scores exactly as it
//! always did.

use super::{RowMetrics, SweepOptions, SweepScene, TiltAverages};
use crate::{
    metric_deltas::average_axis_profiles,
    solve_policy::{design_to_gpu_planes_from_solved, solve_cancellably},
};
use pleochroma::{
    color::metrics::{
        FinishedStone, evaluate_all_axes_profiles_stepped_finished_threaded,
        evaluate_gem_optical_metrics_finished,
    },
    geometry::{
        StoneGeometry,
        meet_solver::{SolveError, SolveStrategy, SolvedTier},
        stone_metrics::{SolidStatus, build_solid_mesh, measure_solid},
    },
};
use pleochroma_cut_core::{
    DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, Design, DesignSolveError, check_manufacturability,
    volumetric_yield,
};
use std::sync::atomic::{AtomicBool, Ordering};

use super::rows::count_warnings;

/// The camera pitch of the fast score (90 degrees): looking straight down on the table,
/// as Optimize's own fast score does.
const TABLE_UP_PITCH_RAD: f32 = std::f32::consts::FRAC_PI_2;

/// The scoring was stopped by the cancel flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

/// Solves and scores `scratch` (a clone already set to the angles to try).
///
/// `Ok(None)` is a design that gives no stone (the reason is pushed to `notes`);
/// `Err(Cancelled)` is a cancelled solve or tilt average. `tilt_threads` caps the threads
/// of the tilt average (`None` leaves it to the tracer, `Some(1)` runs it on the calling
/// thread, which a grid of cells uses so the cell workers are the only pool).
pub(super) fn score_clone(
    scratch: &Design,
    scene: &SweepScene<'_>,
    options: &SweepOptions,
    preform_volume: Option<f64>,
    tilt_threads: Option<usize>,
    cancel: &AtomicBool,
    notes: &mut Vec<String>,
) -> Result<Option<RowMetrics>, Cancelled> {
    let ctx = Context {
        scene,
        options,
        preform_volume,
        tilt_threads,
        cancel,
    };
    score_inner(scratch, None, &ctx, notes)
}

/// Scores one finished design as it stands: no angle is changed.
///
/// When `solved` is `Some` its masts are used and nothing is solved (a worker that only
/// scores never solves); when it is `None` the design is solved first. The tilt average
/// (when `options` asks for it) is left to the tracer's own threads.
///
/// # Errors
///
/// [`Cancelled`] when `cancel` was set during the solve or the tilt average.
pub fn score_design(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    scene: &SweepScene<'_>,
    options: &SweepOptions,
    cancel: &AtomicBool,
) -> Result<Option<RowMetrics>, Cancelled> {
    let ctx = Context {
        scene,
        options,
        preform_volume: measure_solid(&design.preform.planes()).map(|solid| solid.volume),
        tilt_threads: None,
        cancel,
    };
    score_inner(design, solved, &ctx, &mut Vec::new())
}

/// What one scoring run shares: the scene and options, the rough's volume, the tilt
/// threads and the cancel flag.
struct Context<'a> {
    scene: &'a SweepScene<'a>,
    options: &'a SweepOptions,
    preform_volume: Option<f64>,
    tilt_threads: Option<usize>,
    cancel: &'a AtomicBool,
}

/// The body of [`score_clone`] and [`score_design`].
fn score_inner(
    scratch: &Design,
    given: Option<&[SolvedTier]>,
    ctx: &Context<'_>,
    notes: &mut Vec<String>,
) -> Result<Option<RowMetrics>, Cancelled> {
    let Context {
        scene,
        options,
        preform_volume,
        tilt_threads,
        cancel,
    } = *ctx;
    let own: Vec<SolvedTier>;
    let solved: &[SolvedTier] = if let Some(masts) = given {
        masts
    } else {
        own = match solve_cancellably(scratch, cancel) {
            Ok(solved) => solved,
            Err(DesignSolveError::Solve(SolveError::Cancelled)) => return Err(Cancelled),
            Err(error) => {
                notes.push(format!("The design does not solve: {error}"));
                return Ok(None);
            }
        };
        &own
    };
    let failed = solved
        .iter()
        .filter(|tier| matches!(tier.strategy, SolveStrategy::Failed))
        .count();
    if failed > 0 {
        notes.push(format!(
            "The solver could not place {failed} tier{}.",
            if failed == 1 { "" } else { "s" }
        ));
        return Ok(None);
    }
    let planes = scratch.planes_from_solved(solved);
    if !matches!(build_solid_mesh(&planes), SolidStatus::Closed(_)) {
        notes.push("The facets do not close into a stone at this angle.".to_owned());
        return Ok(None);
    }
    let Some(solid) = measure_solid(&planes) else {
        notes.push("The stone cannot be measured at this angle.".to_owned());
        return Ok(None);
    };
    let warning_count = count_warnings(
        &check_manufacturability(scratch, solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2),
        notes,
    );
    let gpu_planes = design_to_gpu_planes_from_solved(scratch, solved);
    // The stone with its concave tools (none for a flat design) and its frosted facets and
    // girdle (none for a polished design), as the Compare window measures it: the same
    // placement rule, the same tracer entry points.
    let tools = scratch.tools_for_scoring(&planes);
    let finishes = scratch.facet_finishes_from_solved(solved, &gpu_planes);
    let stone = FinishedStone::new(
        StoneGeometry {
            planes: &gpu_planes,
            tools: &tools,
        },
        &finishes,
    );
    let fast = evaluate_gem_optical_metrics_finished(
        stone,
        scene.material,
        0.0,
        TABLE_UP_PITCH_RAD,
        scene.environment,
    );
    // The yield reads the carved volume (the flat one without tools, or when the tools leave
    // no closed stone), the same measure the yield report and carat weight use.
    let volume =
        Design::carved_metrics(Some(solid), &planes, &tools).map_or(solid.volume, |m| m.volume);
    let tilt = if options.tilt_average {
        // The same measurement the compare window takes, stopped by the cancel flag.
        let Some(averages) = measure_tilt(stone, scene, tilt_threads, cancel) else {
            return Err(Cancelled);
        };
        Some(averages)
    } else {
        None
    };
    Ok(Some(RowMetrics {
        brilliance_pct: fast.brilliance_pct,
        windowing_pct: fast.windowing_pct,
        extinction_pct: fast.extinction_pct,
        fire_index: fast.fire_index,
        scintillation_pct: fast.scintillation_pct,
        yield_pct: preform_volume
            .and_then(|preform| volumetric_yield(volume, preform))
            .map(|fraction| fraction * 100.0),
        warning_count,
        has_girdle: solid
            .girdle_thickness
            .is_some_and(|thickness| thickness > 0.0),
        tilt,
    }))
}

/// The tilt average of `stone`, or `None` when `cancel` stopped it. With `threads` as
/// `None` this is `measure_tilt_average_finished`, bit for bit.
fn measure_tilt(
    stone: FinishedStone<'_>,
    scene: &SweepScene<'_>,
    threads: Option<usize>,
    cancel: &AtomicBool,
) -> Option<TiltAverages> {
    evaluate_all_axes_profiles_stepped_finished_threaded(
        stone,
        scene.material,
        scene.environment,
        threads,
        &mut |_| !cancel.load(Ordering::Relaxed),
    )
    .map(|axes| average_axis_profiles(&axes))
}

#[cfg(test)]
#[path = "tests_frosted.rs"]
mod frosted_tests;
