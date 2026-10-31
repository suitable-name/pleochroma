//! Deep Solve in the solve Worker.
//!
//! This holds the plain-data request and result types that cross the Worker boundary, and
//! the handler that runs the desktop's verified repair search.
//!
//! # Same search as the desktop and the mobile core
//!
//! [`run_deep_solve`] is `pleochroma-mobile-core`'s `DeepSolveJob::run` without its policy
//! types (no compute permit, no operation token): the plain solve of the design (the
//! baseline the page compares against), the design's resolved meet inputs, then
//! `solve_meet_points_verified_with` with no adjustable anchors. The printed proportions
//! are not part of the design TOML, so the request carries the five figures itself
//! ([`PrintedTargetsData`]), and the Worker re-checks `deep_solve::availability` on them
//! (the page greys the button out, so this only guards a race with a concurrent edit).
//!
//! # Progress and cancel
//!
//! The search has no honest completion fraction: every pipeline run forwards its own
//! plain-solve-shaped [`SolveProgress`], so the Worker shows the phase and the elapsed time
//! only (see [`deep_progress_status`]). The Worker's progress hook looks at the job's cancel
//! URL and sets the cancel flag the solver polls at each of its checkpoints, as for
//! Optimize; a cancelled run has no partial result and answers
//! [`SolveResponse::Cancelled`] within the poll interval.

use std::sync::atomic::{AtomicBool, Ordering};

use pleochroma::geometry::{
    meet_solver::{
        SolveControl, SolveError, SolvePhase, SolveProgress, VerifiedSolveReport,
        solve_meet_points_verified_with,
    },
    stone_metrics::ExternalProportions,
};
use pleochroma_cut_core::{Design, DesignSolveError};
use pleochroma_editor::deep_solve::availability;
use serde::{Deserialize, Serialize};

use super::{SolveResponse, SolvedTierData};

/// `ExternalProportions` as plain data (it has no serde of its own): the five printed
/// figures, each independently optional.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PrintedTargetsData {
    /// Printed `Vol/W^3`.
    pub vol_w3: Option<f64>,
    /// Printed `L/W`.
    pub lw: Option<f64>,
    /// Printed `C/W`.
    pub cw: Option<f64>,
    /// Printed `P/W`.
    pub pw: Option<f64>,
    /// Printed `H/W`.
    pub hw: Option<f64>,
}

impl From<&ExternalProportions> for PrintedTargetsData {
    fn from(targets: &ExternalProportions) -> Self {
        Self {
            vol_w3: targets.vol_w3,
            lw: targets.lw,
            cw: targets.cw,
            pw: targets.pw,
            hw: targets.hw,
        }
    }
}

impl PrintedTargetsData {
    /// The `ExternalProportions` this came from; converts back losslessly.
    #[must_use]
    pub const fn to_external(self) -> ExternalProportions {
        ExternalProportions {
            vol_w3: self.vol_w3,
            lw: self.lw,
            cw: self.cw,
            pw: self.pw,
            hw: self.hw,
        }
    }
}

/// What a Deep Solve is asked to verify against.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct DeepSolveParams {
    /// The design's printed proportions (the page's `printed_proportions`).
    pub targets: PrintedTargetsData,
}

/// `VerifiedSolveReport` as plain data (it has no serde of its own).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VerifiedReportData {
    /// Combined deviation of the plain solve; `INFINITY` when nothing could be measured.
    pub initial_score: f64,
    /// Combined deviation after the anchor-calibration grid.
    pub score_after_calibration: f64,
    /// Combined deviation of the returned configuration.
    pub final_score: f64,
    /// The returned configuration reproduces the printed figures to verification accuracy.
    pub accepted: bool,
    /// Vertex-level decision overrides the search committed.
    pub overrides_applied: u32,
    /// Anchor-value moves the search committed.
    pub anchor_moves_applied: u32,
    /// Pipeline runs spent.
    pub pipeline_runs: u32,
}

impl From<&VerifiedSolveReport> for VerifiedReportData {
    fn from(report: &VerifiedSolveReport) -> Self {
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        Self {
            initial_score: report.initial_score,
            score_after_calibration: report.score_after_calibration,
            final_score: report.final_score,
            accepted: report.accepted,
            overrides_applied: count(report.overrides_applied),
            anchor_moves_applied: count(report.anchor_moves_applied),
            pipeline_runs: count(report.pipeline_runs),
        }
    }
}

impl VerifiedReportData {
    /// The `VerifiedSolveReport` this came from, for the editor's report text.
    #[must_use]
    pub const fn to_report(self) -> VerifiedSolveReport {
        VerifiedSolveReport {
            initial_score: self.initial_score,
            score_after_calibration: self.score_after_calibration,
            final_score: self.final_score,
            accepted: self.accepted,
            overrides_applied: self.overrides_applied as usize,
            anchor_moves_applied: self.anchor_moves_applied as usize,
            pipeline_runs: self.pipeline_runs as usize,
        }
    }
}

/// A finished Deep Solve.
///
/// It holds the verified masts, the plain solve they are compared with, and the verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeepSolveResultData {
    /// The verified repair search's masts, in `Design::tiers` order.
    pub solved: Vec<SolvedTierData>,
    /// The design's plain solve, in the same order (the "before" of the per-tier rows).
    pub baseline: Vec<SolvedTierData>,
    /// The aggregate verdict.
    pub report: VerifiedReportData,
}

/// The status line of a running Deep Solve.
///
/// It names the phase of the pipeline run in flight and the elapsed time. There is no
/// fraction to show (see the module doc comment).
#[must_use]
pub fn deep_progress_status(progress: SolveProgress, elapsed_secs: f32) -> String {
    let stage = match progress.phase {
        SolvePhase::Constructive => format!(
            "placing tiers, {} of {}",
            progress.blocks_done, progress.blocks_total
        ),
        SolvePhase::LeastSquares => "estimating unsettled blocks".to_owned(),
        SolvePhase::Refine => format!(
            "refining, sweep {} of up to {}",
            progress.sweep, progress.max_sweeps
        ),
    };
    format!(
        "Deep Solve... checking the stone against its printed proportions ({stage}), \
         {elapsed_secs:.1}s elapsed"
    )
}

/// A failed Deep Solve as the answer: `message` is the reason.
const fn failed(message: String, missing_anchor: bool) -> SolveResponse {
    SolveResponse::AnalysisFailed {
        message,
        missing_anchor,
    }
}

/// The answer for a plain solve or meet-input resolution that did not finish.
fn failed_solve(error: &DesignSolveError) -> SolveResponse {
    match error {
        DesignSolveError::Solve(SolveError::Cancelled) => SolveResponse::Cancelled,
        other => failed(
            other.to_string(),
            matches!(other, DesignSolveError::MissingAnchor(_)),
        ),
    }
}

/// Runs Deep Solve on `design` against `params.targets`.
///
/// See the module doc comment. `cancel` is polled at every solver checkpoint; a set flag answers
/// [`SolveResponse::Cancelled`]. `on_progress` receives every checkpoint's report (the
/// Worker throttles and formats them; pass `&|_| {}` for none). A design Deep Solve
/// declines (no usable printed figures, nothing to repair, no gear) answers
/// [`SolveResponse::AnalysisFailed`] with `deep_solve::availability`'s hint.
#[must_use]
pub fn run_deep_solve(
    design: &Design,
    params: &DeepSolveParams,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(SolveProgress),
) -> SolveResponse {
    let targets = params.targets.to_external();
    let (available, hint) = availability(design, Some(&targets));
    if !available {
        return failed(hint, false);
    }
    let control = SolveControl::with_cancel(cancel).reporting(on_progress);
    let baseline = match design.solve_with(&control) {
        Ok(baseline) => baseline,
        Err(error) => return failed_solve(&error),
    };
    let tiers = match design.resolved_meet_tier_inputs() {
        Ok(tiers) => tiers,
        Err(error) => return failed_solve(&DesignSolveError::from(error)),
    };
    let (solved, report) = match solve_meet_points_verified_with(
        design.meta.gear_teeth_abs(),
        &tiers,
        &targets,
        &[],
        &control,
    ) {
        Ok(result) => result,
        Err(SolveError::Cancelled) => return SolveResponse::Cancelled,
        Err(error) => return failed(error.to_string(), false),
    };
    // A flag raised after the last checkpoint still means the page gave up.
    if cancel.load(Ordering::Relaxed) {
        return SolveResponse::Cancelled;
    }
    SolveResponse::DeepSolved(DeepSolveResultData {
        solved: solved.iter().map(SolvedTierData::from).collect(),
        baseline: baseline.iter().map(SolvedTierData::from).collect(),
        report: VerifiedReportData::from(&report),
    })
}
