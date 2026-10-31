//! Shared Deep Solve availability and diagnostic presentation. No UI or mutation.
use crate::optimize_view::tier_name_for_row;
use pleochroma::geometry::{
    meet_solver::{MeetConstraint, SolvedTier, VerifiedSolveReport},
    stone_metrics::ExternalProportions,
};
use pleochroma_cut_core::Design;

/// Whether Deep Solve can run on `design`, and the hint shown next to its button.
///
/// Only externally supplied targets can verify a design. A concave design is
/// available: the verified solver measures the flat stone (the planar solve), which is
/// what printed proportions describe, and concave tools are not part of it.
pub fn availability(design: &Design, targets: Option<&ExternalProportions>) -> (bool, String) {
    let Some(targets) = targets else {
        return (false, "Deep Solve needs this design's printed proportions. Enter the figures from its published source.".into());
    };
    let values = [
        targets.vol_w3,
        targets.lw,
        targets.cw,
        targets.pw,
        targets.hw,
    ];
    if values.iter().all(Option::is_none)
        || values
            .iter()
            .flatten()
            .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return (
            false,
            "Enter at least one finite positive printed proportion from the source.".into(),
        );
    }
    if design.meta.gear_teeth == 0 || design.tiers.is_empty() {
        return (
            false,
            "Add valid tiers and a positive index gear before Deep Solve.".into(),
        );
    }
    if !design
        .tiers
        .iter()
        .any(|tier| !matches!(tier.constraint, MeetConstraint::ScaleReference(_)))
    {
        return (false, "Every tier is pinned. Use Adopt on an imported meet before Deep Solve has anything to repair.".into());
    }
    let mut hint = String::from(
        "Slow: runs locally off the UI thread and can be cancelled. Verification never edits constraints.",
    );
    if !design.concave_tiers.is_empty() {
        hint.push_str(
            " Verifies the flat stone; concave tools are not part of printed proportions.",
        );
    }
    (true, hint)
}

/// One tier's mast disagreement between a plain solve and Deep Solve's externally
/// verified repair.
///
/// This is the per-tier breakdown behind
/// [`VerifiedSolveReport`]'s aggregate verdict. Display-only, like the report itself:
/// nothing here is ever written back to a `Design`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TierMastDelta {
    /// This tier's index in `Design::tiers` (and in `Design::solve`'s parallel
    /// output).
    pub tier_index: usize,
    /// The plain solve's mast for this tier, before verification.
    pub before_mast: f64,
    /// Deep Solve's verified mast for this tier.
    pub after_mast: f64,
}

impl TierMastDelta {
    /// The signed movement `after_mast - before_mast` -- positive means Deep Solve
    /// pushed the mast further from the girdle, negative means closer.
    #[must_use]
    pub fn delta(&self) -> f64 {
        self.after_mast - self.before_mast
    }
}

/// A mast movement below this (in the same units as `SolvedTier::mast`, i.e. mast
/// order) is treated as "unchanged" -- filters out the floating-point noise a tier
/// Deep Solve genuinely left alone would otherwise show as a spurious sub-millitier
/// disagreement.
const MAST_DELTA_EPSILON: f64 = 1e-6;

/// Zips `current` (a plain [`pleochroma_cut_core::Design::solve`]) against `verified`
/// (a Deep Solve's own [`DeepSolveOutcome::Completed::solved`]) tier by tier.
///
/// Keeps only the tiers whose mast actually moved by more than [`MAST_DELTA_EPSILON`] --
/// the per-tier breakdown [`DeepSolveOutcome`]'s own doc comment describes. Both
/// slices are expected in the same tier order (`Design::meet_tier_inputs`'s own
/// convention); this pairs them positionally and silently stops at the shorter of the
/// two lengths rather than panicking, since a design edited between the plain solve
/// and Deep Solve's completion can change tier count -- the caller already flags that
/// case `stale` independently (`EditorState::generation`).
#[must_use]
pub fn tier_mast_deltas(current: &[SolvedTier], verified: &[SolvedTier]) -> Vec<TierMastDelta> {
    current
        .iter()
        .zip(verified.iter())
        .enumerate()
        .filter_map(|(tier_index, (before, after))| {
            let delta = after.mast - before.mast;
            (delta.abs() > MAST_DELTA_EPSILON).then_some(TierMastDelta {
                tier_index,
                before_mast: before.mast,
                after_mast: after.mast,
            })
        })
        .collect()
}

/// Renders [`tier_mast_deltas`]'s result as a status-line suffix naming which tiers
/// Deep Solve's verified repair actually disagreed with the plain solve on.
///
/// It also gives by how much -- the minimal consumer [`DeepSolveOutcome`]'s own doc comment describes,
/// so this per-tier data is never dead: `format_deep_solve_report`
/// (`gui::editor::view`) still owns the aggregate verdict line; this is appended
/// after it. Returns `""` when `deltas` is empty (report already reads as full
/// agreement, and an empty sentence would just be noise).
#[must_use]
pub fn format_tier_mast_deltas(deltas: &[TierMastDelta]) -> String {
    if deltas.is_empty() {
        return String::new();
    }
    let tiers: Vec<String> = deltas
        .iter()
        .map(|d| format!("tier {} ({:+.4})", d.tier_index, d.delta()))
        .collect();
    format!(" Disagrees with the last solve on: {}.", tiers.join(", "))
}

/// One row of the per-tier table behind a Deep Solve verdict, as display text.
///
/// Plain strings so every front end (the desktop's Slint struct, the web page) maps it
/// without repeating the number formatting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepSolveRowText {
    /// `#N`, N being the tier's 1-based position in `Design::tiers`.
    pub tier_number: String,
    /// The tier's table name, `""` when the tier no longer exists.
    pub name: String,
    /// The plain solve's mast, four decimals.
    pub before_mast: String,
    /// Deep Solve's verified mast, four decimals.
    pub after_mast: String,
    /// The signed movement, four decimals.
    pub delta: String,
}

/// Formats [`tier_mast_deltas`]'s output as one [`DeepSolveRowText`] per delta.
///
/// `deltas` is expected to be already filtered to the tiers whose mast moved. A tier
/// index past the end of `design.tiers` (an edit landed mid-run) gets a blank name.
#[must_use]
pub fn deep_solve_row_texts(deltas: &[TierMastDelta], design: &Design) -> Vec<DeepSolveRowText> {
    deltas
        .iter()
        .map(|d| DeepSolveRowText {
            tier_number: format!("#{}", d.tier_index + 1),
            name: tier_name_for_row(design, d.tier_index),
            before_mast: format!("{:.4}", d.before_mast),
            after_mast: format!("{:.4}", d.after_mast),
            delta: format!("{:+.4}", d.delta()),
        })
        .collect()
}

/// [`format_deep_solve_report_for`] for a design without concave tiers.
#[must_use]
pub fn format_deep_solve_report(report: &VerifiedSolveReport, stale: bool) -> String {
    format_deep_solve_report_for(report, stale, 0)
}

/// Renders a Deep Solve verdict as the status text.
///
/// Honest by construction: it always shows the score movement and the run count, and never
/// claims that `accepted` proves the geometry. `concave_tiers` is the design's concave
/// tier count; when it is positive the text says the verdict is about the flat stone.
#[must_use]
pub fn format_deep_solve_report_for(
    report: &VerifiedSolveReport,
    stale: bool,
    concave_tiers: usize,
) -> String {
    let verdict = if report.accepted {
        "ACCEPTED -- reproduces the printed figures to verification accuracy (not a \
         correctness proof, only a strong external signal)"
    } else {
        "not accepted -- still deviates from the printed figures"
    };
    let scores = if report.initial_score.is_finite() {
        format!(
            "combined deviation {:.4} -> {:.4} ({} vertex-level repair(s), {} anchor \
             calibration move(s), {} pipeline run(s))",
            report.initial_score,
            report.final_score,
            report.overrides_applied,
            report.anchor_moves_applied,
            report.pipeline_runs
        )
    } else {
        "none of this design's printed figures overlapped what could be measured -- \
         unverifiable"
            .to_string()
    };
    let concave_note = if concave_tiers > 0 {
        format!(
            " (flat stone; {concave_tiers} concave tier(s) are not part of the printed \
             proportions)"
        )
    } else {
        String::new()
    };
    let stale_note = if stale {
        " NOTE: the design changed while this ran -- re-run Deep Solve for a result that \
         reflects the current schedule."
    } else {
        ""
    };
    format!("Deep Solve: {verdict}. {scores}.{concave_note}{stale_note}")
}

#[cfg(test)]
mod tests;
