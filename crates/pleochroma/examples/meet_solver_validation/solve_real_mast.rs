//! Reports A and C: solving with each block's scale anchor bootstrapped from the design's
//! own recorded mast. This is a harness-only crutch; a design without an `.asc` file has no
//! recorded mast.

use pleochroma::geometry::{
    meet_solver::{
        MeetConstraint, meet_tier_inputs_from_asc, solve_meet_points, solve_meet_points_verified,
    },
    stone_metrics::ExternalProportions,
};
use pleochroma_formats::asc;

use crate::{
    scoring::{classify_named_resolution, score_solved_tiers},
    types::{AscRow, ConstraintKind, DesignResult},
};

/// Solves one design end-to-end (parse, bootstrap scale reference if needed, solve,
/// score against the file's own real masts) and returns everything the aggregate
/// report needs. No shared state with any other design -- safe to call from any
/// thread on a disjoint row.
pub fn solve_one(row: &AscRow) -> DesignResult {
    solve_one_impl(row, |gear, tiers| (solve_meet_points(gear, tiers), None))
}

/// Report C: like [`solve_one`] (identical anchoring and scoring conventions),
/// but solving via [`solve_meet_points_verified`] with the design's printed
/// proportions as the external repair/verification targets.
pub fn solve_one_verified(row: &AscRow) -> DesignResult {
    let targets = ExternalProportions {
        vol_w3: row.volume,
        lw: row.lw_ratio,
        cw: row.cw_ratio,
        pw: row.pw_ratio,
        hw: row.hw_ratio,
    };
    solve_one_impl(row, move |gear, tiers| {
        let (solved, report) = solve_meet_points_verified(gear, tiers, &targets, &[]);
        (solved, Some(report))
    })
}

fn solve_one_impl(
    row: &AscRow,
    solve: impl FnOnce(
        u32,
        &[pleochroma::geometry::meet_solver::MeetTierInput],
    ) -> (
        Vec<pleochroma::geometry::meet_solver::SolvedTier>,
        Option<pleochroma::geometry::meet_solver::VerifiedSolveReport>,
    ),
) -> DesignResult {
    let mut out = DesignResult::default();

    let text = String::from_utf8_lossy(&row.content);
    let Ok(schedule) = asc::parse_asc(&text) else {
        return out; // parse_ok stays false
    };
    out.parse_ok = true;
    if schedule.tiers.is_empty() {
        return out;
    }

    let mut tiers = meet_tier_inputs_from_asc(&schedule);
    let original_kinds: Vec<ConstraintKind> = tiers
        .iter()
        .map(|t| match &t.constraint {
            MeetConstraint::ScaleReference(_) => ConstraintKind::ScaleReference,
            MeetConstraint::MeetNamed(_) => ConstraintKind::MeetNamed,
            MeetConstraint::MeetExisting => ConstraintKind::MeetExisting,
        })
        .collect();
    // Captured *before* the tier-0 scale-reference bootstrap below can overwrite a
    // `MeetNamed` tier 0's `.constraint` (and with it, its name list) -- so a
    // bootstrapped design's original stated names are never lost.
    let original_named_names: Vec<Option<Vec<String>>> = tiers
        .iter()
        .map(|t| match &t.constraint {
            MeetConstraint::MeetNamed(names) => Some(names.clone()),
            _ => None,
        })
        .collect();
    out.has_meet_named = original_kinds.contains(&ConstraintKind::MeetNamed);
    out.has_meet_existing = original_kinds.contains(&ConstraintKind::MeetExisting);
    out.has_scale_reference = original_kinds.contains(&ConstraintKind::ScaleReference);

    // Per-block scale anchors. Each block (crown, pavilion, girdle; classified by facet-normal
    // y-sign) has a continuous freedom that preserves every vertex incidence, so each block
    // needs one stated dimension. A block without one gets its first tier bootstrapped from
    // that tier's recorded mast. Bootstrapped tiers are excluded from scoring.
    let mut bootstrapped: Vec<bool> = vec![false; tiers.len()];
    {
        // Side per tier (crown = +1, pavilion = -1, girdle = 0), as the solver classifies
        // it. A negative zero angle is the culet.
        let side: Vec<i8> = tiers
            .iter()
            .map(|t| {
                let crown = pleochroma::geometry::plane::tier_is_crown_side(t.angle_deg);
                let y = if crown {
                    t.angle_deg.abs().to_radians().cos()
                } else {
                    -t.angle_deg.abs().to_radians().cos()
                };
                if y.abs() <= 1e-6 {
                    0
                } else if y > 0.0 {
                    1
                } else {
                    -1
                }
            })
            .collect();
        for block in [1i8, -1, 0] {
            let members: Vec<usize> = (0..tiers.len()).filter(|&i| side[i] == block).collect();
            let has_anchor = members
                .iter()
                .any(|&i| matches!(tiers[i].constraint, MeetConstraint::ScaleReference(_)));
            if let (false, Some(&first)) = (has_anchor, members.first()) {
                tiers[first].constraint =
                    MeetConstraint::ScaleReference(schedule.tiers[first].mast);
                bootstrapped[first] = true;
                out.no_scale_reference = true; // >=1 block needed a bootstrap
            }
        }
    }

    // Resolved against `tiers` in the state `solve_meet_points` receives (after the bootstrap
    // above), so the result matches the solver's own resolution. See `classify_named_resolution`.
    let named_resolution = classify_named_resolution(&tiers, &original_named_names);

    let (solved, verify_report) = solve(schedule.gear_teeth_abs(), &tiers);
    if solved.len() != schedule.tiers.len() {
        return out;
    }
    out.verify = verify_report;

    score_solved_tiers(
        &schedule,
        &solved,
        &original_kinds,
        &named_resolution,
        |i| original_kinds[i] == ConstraintKind::ScaleReference || bootstrapped[i],
        &mut out,
    );
    out
}
