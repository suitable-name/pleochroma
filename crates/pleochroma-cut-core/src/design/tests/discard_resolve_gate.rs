//! The discard-and-rederive property, checked at real-fixture scale.
//!
//! It measures how well meet constraints alone recover a design's real recorded masts, and
//! the separate (exact, not tolerance-bound) property that a pinned import
//! reproduces its original masts exactly.

use super::fixtures::real_fixtures;
use crate::{design::Design, preform::PreformSpec};
use pleochroma::geometry::meet_solver::{
    Block, MeetConstraint, MeetTierInput, SolveStrategy, SolvedTier, classify_blocks,
    meet_tier_inputs_from_asc, solve_meet_points,
};

/// The measured `worst_meet_derived_rel_err` of each expected-FAIL fixture, pinned within 10%.
///
/// The pass/fail check alone would miss the solver getting much worse while still failing.
/// A real regression should change this number with an explanation, never silence the
/// check (see [`discard_and_resolve_gate_on_real_fixtures`]).
const EXPECTED_FAIL_WORST_ERR: &[(&str, f64)] = &[
    ("Mini Square Barion #4", 0.6428),
    ("Six Main Hilite LB", 0.2954),
    ("RBC-445", 0.5670),
];

/// The discard-and-rederive property on five real `.asc` fixtures.
///
/// The fixtures are embedded in [`super::fixtures::real_fixtures`], so no external file is
/// needed. They have 2, 8, 9, 12 and 20 tiers.
///
/// Each design's recorded masts are discarded and `Design::solve` rederives them. Every
/// meet-derived tier's relative error against its recorded mast must be within a
/// **10% tolerance**, the bar `crates/pleochroma/examples/meet_solver_validation/main.rs`
/// reports against.
///
/// The corpus-wide figure (about 10.8% of 2,881 designs over that bar) comes from an
/// external catalogue that is not in this repository, so it is not asserted here. The
/// assertions encode which fixtures pass and which are pinned as known limitations at
/// today's solver behavior. A real change in either direction should change this test,
/// not the tolerance.
#[test]
fn discard_and_resolve_gate_on_real_fixtures() {
    let mut passed = 0;
    for (name, text, expect_pass) in real_fixtures::ALL {
        let worst_err = worst_meet_derived_rel_err(name, text);
        // Octahedron's crown and pavilion each bootstrap as their own anchor, so the
        // solver derives nothing. `None` is a vacuous pass, not a real one.
        let Some(worst_err) = worst_err else {
            assert_eq!(
                name, "Octahedron",
                "{name}: zero meet-derived tiers -- every tier bootstrapped as its own anchor, \
                 so this fixture tests nothing; pick a different fixture or explain why zero is \
                 expected here"
            );
            println!("{name}: zero meet-derived tiers -- vacuous PASS, tests nothing");
            assert!(
                expect_pass,
                "{name}: a vacuous case must be marked expect_pass"
            );
            passed += 1;
            continue;
        };
        let this_passes = worst_err <= real_fixtures::TOLERANCE;
        println!(
            "{name}: worst meet-derived tier rel. err {worst_err:.4} ({})",
            if this_passes { "PASS" } else { "FAIL" }
        );
        assert_eq!(
            this_passes,
            expect_pass,
            "{name}: expected {} at {} tolerance, got worst rel. err {worst_err:.4} -- the \
             solver's real-design behavior changed; update the expectation only with a \
             genuine explanation of why, never to silence this",
            if expect_pass { "PASS" } else { "FAIL" },
            real_fixtures::TOLERANCE,
        );
        if !expect_pass {
            let &(_, expected) = EXPECTED_FAIL_WORST_ERR
                .iter()
                .find(|&&(n, _)| n == name)
                .unwrap_or_else(|| {
                    panic!("{name}: no pinned expected worst_err for a FAIL fixture")
                });
            let rel_diff = (worst_err - expected).abs() / expected.abs().max(1e-6);
            assert!(
                rel_diff <= 0.10,
                "{name}: worst_err {worst_err:.4} drifted more than 10% from the pinned \
                 {expected:.4} -- a real regression (in either direction), not a reason to loosen \
                 this bound"
            );
        }
        passed += usize::from(this_passes);
    }

    println!(
        "\ndiscard-and-resolve gate: {passed}/{} real fixtures pass at {} tolerance ({:.0}%) \
         -- corpus-wide (full 2,881 designs): 10.8%",
        real_fixtures::ALL.len(),
        real_fixtures::TOLERANCE,
        100.0 * passed as f64 / real_fixtures::ALL.len() as f64
    );
}

/// Checks the same five fixtures for a different property: exact reproduction on import.
///
/// Importing a real `.asc` file via [`crate::design::Design::from_asc_schedule`] and
/// re-solving must reproduce **every** recorded mast exactly. Every tier is pinned to a
/// [`MeetConstraint::ScaleReference`], so only parse round-trip float error is possible,
/// hence the `1e-9` bound instead of the gate's 10%.
#[test]
fn importing_pins_every_tier_and_resolving_reproduces_the_original_masts_exactly() {
    for (name, text) in real_fixtures::ALL
        .iter()
        .map(|&(name, text, _)| (name, text))
    {
        let schedule = pleochroma_formats::asc::parse_asc(text)
            .unwrap_or_else(|e| panic!("{name}: must parse: {e}"));
        let design = Design::from_asc_schedule(PreformSpec::block(1.0, 1.0, 1.0), &schedule);
        assert!(
            design
                .tiers
                .iter()
                .all(|t| matches!(t.constraint, MeetConstraint::ScaleReference(_))),
            "{name}: import must pin every tier, no exceptions"
        );
        let solved = design
            .solve()
            .unwrap_or_else(|e| panic!("{name}: every tier is its own anchor: {e}"));
        assert_eq!(
            solved.len(),
            schedule.tiers.len(),
            "{name}: tier count must match"
        );
        for (i, (solved, original)) in solved.iter().zip(&schedule.tiers).enumerate() {
            assert!(
                (solved.mast - original.mast).abs() < 1e-9,
                "{name}: tier {i} mast {} != original {} (exact reproduction required)",
                solved.mast,
                original.mast
            );
            assert_eq!(solved.strategy, SolveStrategy::ScaleReference);
        }
    }
}

/// Re-derives a file's masts from its constraints alone.
///
/// It calls `pleochroma`'s `meet_solver` API directly, not `Design::from_asc_schedule`,
/// which pins every tier's mast.
///
/// Each crown, pavilion and girdle block is anchored at its first tier's recorded mast
/// when the file states no anchor of its own. Every other tier's recorded mast is
/// discarded and solved by `solve_meet_points`.
///
/// Returns the worst relative error among the meet-derived tiers against their recorded
/// masts, or `None` when there are none (see the `Octahedron` case in
/// [`discard_and_resolve_gate_on_real_fixtures`]). `ScaleReference` tiers are the anchors
/// and are excluded.
///
/// Checks that each meet-derived mast is finite. A plain `f64::max` fold would drop a
/// `NaN` and score a solver failure as `0.0`.
///
/// # Panics
///
/// If any meet-derived tier's solved mast, or its relative error, is not finite.
fn worst_meet_derived_rel_err(name: &str, text: &str) -> Option<f64> {
    let schedule = pleochroma_formats::asc::parse_asc(text)
        .unwrap_or_else(|e| panic!("{name}: must parse: {e}"));
    let mut inputs: Vec<MeetTierInput> = meet_tier_inputs_from_asc(&schedule);
    let blocks = classify_blocks(&inputs);
    for block in [Block::Crown, Block::Pavilion, Block::Girdle] {
        let anchored = inputs
            .iter()
            .zip(&blocks)
            .any(|(t, &b)| b == block && matches!(t.constraint, MeetConstraint::ScaleReference(_)));
        if anchored {
            continue;
        }
        if let Some(i) = (0..inputs.len()).find(|&i| blocks[i] == block) {
            inputs[i].constraint = MeetConstraint::ScaleReference(schedule.tiers[i].mast);
        }
    }

    let solved: Vec<SolvedTier> = solve_meet_points(schedule.gear_teeth_abs(), &inputs);

    let mut worst: Option<f64> = None;
    for ((input, solved), original) in inputs.iter().zip(&solved).zip(&schedule.tiers) {
        if matches!(input.constraint, MeetConstraint::ScaleReference(_)) {
            continue;
        }
        assert!(
            solved.mast.is_finite(),
            "{name}: a meet-derived tier's solved mast is not finite ({}) -- a real solver \
             failure, not something an `f64::max` fold should ever hide as `0.0`",
            solved.mast
        );
        let rel_err = (solved.mast - original.mast).abs() / original.mast.abs().max(1e-6);
        assert!(
            rel_err.is_finite(),
            "{name}: a meet-derived tier's relative error is not finite ({rel_err})"
        );
        worst = Some(worst.map_or(rel_err, |w: f64| w.max(rel_err)));
    }
    worst
}
