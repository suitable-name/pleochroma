//! What a finished Deep Solve shows: the verdict line, the per-tier rows and the masts a
//! Pin may use. Pure functions over the Worker answer; no window, no state.

use crate::solve::{DeepSolveResultData, to_solved_tiers};
use pleochroma_cut_core::Design;
use pleochroma_editor::deep_solve::{
    DeepSolveRowText, deep_solve_row_texts, format_deep_solve_report_for, tier_mast_deltas,
};

/// One row of the table: the tier, its display text and the mast Pin would use.
#[derive(Debug, Clone, PartialEq)]
pub struct RowEntry {
    /// The tier's 0-based position in `Design::tiers`.
    pub tier_index: usize,
    /// The row as display text.
    pub text: DeepSolveRowText,
    /// Deep Solve's verified mast for this tier.
    pub verified_mast: f64,
}

/// A finished run as the tab shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// The verdict line.
    pub status: String,
    /// The verdict is "not accepted": shown in the problem color.
    pub problem: bool,
    /// The tiers whose verified mast differs from the plain solve's.
    pub rows: Vec<RowEntry>,
}

/// The verdict and the per-tier rows for `data`, computed against `design` (the design the
/// run solved, so the names line up with the masts). `stale` says an edit landed meanwhile.
#[must_use]
pub fn summarize(data: &DeepSolveResultData, design: &Design, stale: bool) -> Summary {
    let baseline = to_solved_tiers(&data.baseline);
    let solved = to_solved_tiers(&data.solved);
    let deltas = tier_mast_deltas(&baseline, &solved);
    let texts = deep_solve_row_texts(&deltas, design);
    let rows = deltas
        .iter()
        .zip(texts)
        .map(|(delta, text)| RowEntry {
            tier_index: delta.tier_index,
            text,
            verified_mast: delta.after_mast,
        })
        .collect::<Vec<_>>();
    let mut status =
        format_deep_solve_report_for(&data.report.to_report(), stale, design.concave_tiers.len());
    if rows.is_empty() {
        status.push_str(" No tier's mast moved against the plain solve.");
    }
    Summary {
        status,
        problem: !data.report.accepted,
        rows,
    }
}

/// The status line for a Deep Solve the Worker declined or could not finish.
///
/// `message` is the availability hint or the solver's text; `missing_anchor` says the plain
/// solve found a tier it could not place.
#[must_use]
pub fn failure_status(message: &str, missing_anchor: bool) -> String {
    if missing_anchor {
        format!(
            "Deep Solve failed: {message} -- add an Exact scale value tier there and Solve first."
        )
    } else if message.ends_with('.') {
        message.to_owned()
    } else {
        format!("Deep Solve failed: {message}.")
    }
}

/// Whether a Pin may go ahead, and with which mast.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PinCheck {
    /// Pin the tier to this mast.
    Ready(f64),
    /// The design changed since the result was computed (or there is no design).
    Stale,
    /// The result names no verified mast for this tier any more.
    Missing,
}

/// Decides a Pin of `tier`.
///
/// The result must still describe the design (`shown` is the generation it was computed for
/// or last pinned at, `current` the design's now) and name the tier. The verdict itself is not consulted, as on the desktop: a Pin is the cutter's
/// own opt-in.
#[must_use]
pub fn pin_check(
    shown: Option<u64>,
    current: Option<u64>,
    masts: &[(usize, f64)],
    tier: usize,
) -> PinCheck {
    let Some(mast) = masts
        .iter()
        .find(|(index, _)| *index == tier)
        .map(|(_, mast)| *mast)
    else {
        return PinCheck::Missing;
    };
    match (shown, current) {
        (Some(shown), Some(current)) if shown == current => PinCheck::Ready(mast),
        _ => PinCheck::Stale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solve::{SolvedTierData, StrategyData, VerifiedReportData};
    use pleochroma_editor::{
        loading::{parse_new_design_form, parse_preform_form},
        session::EditorSession,
    };

    fn empty_design() -> Design {
        let preform = parse_preform_form(1, "1.50", "1.00", "1.50", 96).expect("preform");
        let spec = parse_new_design_form(96, preform, "8", true, 0).expect("spec");
        EditorSession::from_template(spec, 0).design
    }

    fn tier(mast: f64) -> SolvedTierData {
        SolvedTierData {
            mast,
            strategy: StrategyData::DependencyOrder,
            detail: String::new(),
        }
    }

    fn report(accepted: bool, initial: f64) -> VerifiedReportData {
        VerifiedReportData {
            initial_score: initial,
            score_after_calibration: 0.5,
            final_score: 0.1,
            accepted,
            overrides_applied: 2,
            anchor_moves_applied: 1,
            pipeline_runs: 40,
        }
    }

    fn data(baseline: &[f64], solved: &[f64], accepted: bool) -> DeepSolveResultData {
        DeepSolveResultData {
            solved: solved.iter().copied().map(tier).collect(),
            baseline: baseline.iter().copied().map(tier).collect(),
            report: report(accepted, 1.0),
        }
    }

    #[test]
    fn only_tiers_whose_mast_moved_get_a_row() {
        let summary = summarize(
            &data(&[1.0, 2.0, 3.0], &[1.0, 2.5, 3.0], true),
            &empty_design(),
            false,
        );
        assert_eq!(summary.rows.len(), 1);
        let row = &summary.rows[0];
        assert_eq!(row.tier_index, 1);
        assert_eq!(row.text.tier_number, "#2");
        assert_eq!(row.text.before_mast, "2.0000");
        assert_eq!(row.text.after_mast, "2.5000");
        assert_eq!(row.text.delta, "+0.5000");
        assert!((row.verified_mast - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn a_pure_move_down_has_a_signed_delta() {
        let summary = summarize(&data(&[2.0], &[1.5], true), &empty_design(), false);
        assert_eq!(summary.rows[0].text.delta, "-0.5000");
    }

    #[test]
    fn an_accepted_verdict_is_not_a_problem_and_a_rejected_one_is() {
        let accepted = summarize(&data(&[1.0], &[1.5], true), &empty_design(), false);
        assert!(!accepted.problem);
        assert!(accepted.status.contains("ACCEPTED"));
        let rejected = summarize(&data(&[1.0], &[1.5], false), &empty_design(), false);
        assert!(rejected.problem);
        assert!(rejected.status.contains("not accepted"));
    }

    #[test]
    fn a_run_that_moved_nothing_says_so() {
        let summary = summarize(
            &data(&[1.0, 2.0], &[1.0, 2.0], true),
            &empty_design(),
            false,
        );
        assert!(summary.rows.is_empty());
        assert!(
            summary
                .status
                .ends_with("No tier's mast moved against the plain solve.")
        );
    }

    #[test]
    fn a_stale_run_carries_the_shared_note() {
        let fresh = summarize(&data(&[1.0], &[1.5], true), &empty_design(), false);
        let stale = summarize(&data(&[1.0], &[1.5], true), &empty_design(), true);
        assert!(!fresh.status.contains("the design changed"));
        assert!(stale.status.contains("the design changed while this ran"));
    }

    #[test]
    fn nothing_measurable_reads_as_unverifiable() {
        let mut result = data(&[1.0], &[1.5], false);
        result.report = report(false, f64::INFINITY);
        let summary = summarize(&result, &empty_design(), false);
        assert!(summary.status.contains("unverifiable"));
    }

    #[test]
    fn the_lists_are_paired_up_to_the_shorter_one() {
        // A design edited between the plain solve and the verified one can change the tier
        // count; the shared pairing stops at the shorter list instead of panicking.
        let summary = summarize(&data(&[1.0, 2.0], &[1.5], true), &empty_design(), false);
        assert_eq!(summary.rows.len(), 1);
    }

    #[test]
    fn a_declined_run_shows_the_availability_hint_as_it_is() {
        let hint = "Deep Solve needs this design's printed proportions. Enter the figures.";
        assert_eq!(failure_status(hint, false), hint);
    }

    #[test]
    fn a_solver_failure_gets_a_prefix_and_a_full_stop() {
        assert_eq!(
            failure_status("too many planes", false),
            "Deep Solve failed: too many planes."
        );
    }

    #[test]
    fn a_missing_anchor_names_the_fix() {
        let text = failure_status("tier 3 has no anchor", true);
        assert!(text.starts_with("Deep Solve failed: tier 3 has no anchor"));
        assert!(text.ends_with("add an Exact scale value tier there and Solve first."));
    }

    #[test]
    fn a_pin_needs_the_tier_in_the_result() {
        let masts = [(1, 2.5), (4, 3.5)];
        assert_eq!(pin_check(Some(7), Some(7), &masts, 4), PinCheck::Ready(3.5));
        assert_eq!(pin_check(Some(7), Some(7), &masts, 2), PinCheck::Missing);
    }

    #[test]
    fn a_pin_after_an_unrelated_edit_is_stale() {
        let masts = [(1, 2.5)];
        assert_eq!(pin_check(Some(7), Some(8), &masts, 1), PinCheck::Stale);
        assert_eq!(pin_check(Some(7), None, &masts, 1), PinCheck::Stale);
        assert_eq!(pin_check(None, Some(7), &masts, 1), PinCheck::Stale);
    }

    #[test]
    fn a_missing_tier_wins_over_a_stale_result() {
        // Nothing to pin is the more useful answer than "stale".
        assert_eq!(pin_check(Some(7), Some(8), &[], 0), PinCheck::Missing);
    }
}
