//! Reports B and D: the production ratio-anchoring path.
//!
//! Each block is anchored from the design's printed `C/W` and `P/W` proportions, not from recorded
//! masts, because most catalogued designs have no `.asc` file and so no recorded masts.

use pleochroma::geometry::{
    meet_solver::{
        MeetConstraint, apply_ratio_anchors, meet_tier_inputs_from_asc, solve_meet_points,
        solve_meet_points_verified,
    },
    stone_metrics::ExternalProportions,
};
use pleochroma_formats::asc;

use crate::{
    scoring::{classify_named_resolution, score_solved_tiers},
    types::{AscRow, ConstraintKind, DesignResult},
};

/// Report B: the production ratio-anchoring path.
///
/// Like [`solve_one`], but each block is anchored from the design's printed `C/W` and `P/W`
/// proportions ([`apply_ratio_anchors`]) instead of its recorded tier-0 mast. Only this path
/// measures production usability.
///
/// A block whose ratio is missing (a partial diagram) falls back to its tier-0 mast, as in
/// `solve_one`, so every design stays solvable. The fallback is recorded in
/// `DesignResult::fully_ratio_anchored`. Every anchor tier, whichever path supplied it, is
/// excluded from scoring: an anchor is given, not solved.
pub fn solve_one_ratio_anchored(row: &AscRow) -> DesignResult {
    solve_one_ratio_anchored_impl(row, false)
}

/// Report D: [`solve_one_ratio_anchored`] solved via [`solve_meet_points_verified`].
///
/// The ratio-derived crown and pavilion anchors are *adjustable*, so the search calibrates them
/// against the printed figures, and the printed proportions are the repair targets. Anchoring and
/// scoring otherwise match Report B.
pub fn solve_one_ratio_anchored_verified(row: &AscRow) -> DesignResult {
    solve_one_ratio_anchored_impl(row, true)
}

fn solve_one_ratio_anchored_impl(row: &AscRow, verified: bool) -> DesignResult {
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

    apply_ratio_anchors(&mut tiers, row.cw_ratio, row.pw_ratio);

    // Anything the ratio path newly turned into a ScaleReference (i.e. wasn't
    // already one before the call) is a ratio-derived anchor -- given, not solved.
    let ratio_anchored: Vec<bool> = (0..tiers.len())
        .map(|i| {
            original_kinds[i] != ConstraintKind::ScaleReference
                && matches!(tiers[i].constraint, MeetConstraint::ScaleReference(_))
        })
        .collect();

    // Any block that still has no anchor at all (its ratio was `None`, or it had no
    // tiers for `apply_ratio_anchors` to pick from) falls back to its own tier-0
    // real mast, same convention as `solve_one`, so the design stays solvable.
    let mut fallback_anchored: Vec<bool> = vec![false; tiers.len()];
    let mut fully_ratio_anchored = true;
    {
        use pleochroma::geometry::meet_solver::classify_blocks;
        let blocks = classify_blocks(&tiers);
        for block in [
            pleochroma::geometry::meet_solver::Block::Crown,
            pleochroma::geometry::meet_solver::Block::Pavilion,
            pleochroma::geometry::meet_solver::Block::Girdle,
        ] {
            let members: Vec<usize> = (0..tiers.len()).filter(|&i| blocks[i] == block).collect();
            let has_anchor = members
                .iter()
                .any(|&i| matches!(tiers[i].constraint, MeetConstraint::ScaleReference(_)));
            if let (false, Some(&first)) = (has_anchor, members.first()) {
                tiers[first].constraint =
                    MeetConstraint::ScaleReference(schedule.tiers[first].mast);
                fallback_anchored[first] = true;
                fully_ratio_anchored = false;
            }
        }
    }
    out.fully_ratio_anchored = fully_ratio_anchored;

    let named_resolution = classify_named_resolution(&tiers, &original_named_names);

    let solved = if verified {
        let targets = ExternalProportions {
            vol_w3: row.volume,
            lw: row.lw_ratio,
            cw: row.cw_ratio,
            pw: row.pw_ratio,
            hw: row.hw_ratio,
        };
        // Ratio-derived crown/pavilion anchors are estimates the search may
        // calibrate; the girdle reference is pure unit choice (every target is
        // scale-invariant), and real-mast fallback anchors are exact.
        let blocks = pleochroma::geometry::meet_solver::classify_blocks(&tiers);
        let adjustable: Vec<usize> = (0..tiers.len())
            .filter(|&i| {
                ratio_anchored[i] && blocks[i] != pleochroma::geometry::meet_solver::Block::Girdle
            })
            .collect();
        let (solved, report) =
            solve_meet_points_verified(schedule.gear_teeth_abs(), &tiers, &targets, &adjustable);
        out.verify = Some(report);
        solved
    } else {
        solve_meet_points(schedule.gear_teeth_abs(), &tiers)
    };
    if solved.len() != schedule.tiers.len() {
        return out;
    }

    score_solved_tiers(
        &schedule,
        &solved,
        &original_kinds,
        &named_resolution,
        |i| {
            original_kinds[i] == ConstraintKind::ScaleReference
                || ratio_anchored[i]
                || fallback_anchored[i]
        },
        &mut out,
    );
    out
}
