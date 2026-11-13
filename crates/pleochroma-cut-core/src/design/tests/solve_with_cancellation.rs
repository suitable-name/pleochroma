//! `solve_with`/`resolve_dirty_with` (cancellation) and the panic-to-error
//! conversion of the `previous`-alignment check, plus the plane-count-cap
//! error path.

use crate::{
    design::{ConstraintTier, Design, DesignSolveError},
    preform::PreformSpec,
};
use pleochroma::geometry::meet_solver::{
    MeetConstraint, SolveControl, SolveError, SolveStrategy, SolvedTier,
};

/// A small hand-built design for the `solve_with`/`resolve_dirty_with` tests below:
/// anchors `A` (crown) and `C` (pavilion), with `B` (crown) meeting `A` by name.
fn three_anchor_design() -> Design {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(ConstraintTier {
        angle_deg: 30.0,
        name: "A".to_string(),
        indices: vec![0.0, 24.0, 48.0, 72.0],
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });
    design.tiers.push(ConstraintTier {
        angle_deg: 45.0,
        name: "B".to_string(),
        indices: vec![0.0, 24.0, 48.0, 72.0],
        constraint: MeetConstraint::MeetNamed(vec!["A".to_string()]),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });
    design.tiers.push(ConstraintTier {
        angle_deg: -30.0,
        name: "C".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(0.4),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });
    design
}

/// `solve_with` with a default (no-op) [`SolveControl`] must reproduce
/// [`crate::design::Design::solve`] bit for bit -- an unused control changes nothing.
#[test]
fn solve_with_a_default_control_matches_solve_bitwise() {
    let design = three_anchor_design();
    let plain = design.solve().expect("hand-built design must solve");
    let via_with = design
        .solve_with(&SolveControl::default())
        .expect("a default control never cancels");
    assert_eq!(plain.len(), via_with.len());
    for (a, b) in plain.iter().zip(&via_with) {
        assert_eq!(a.mast.to_bits(), b.mast.to_bits());
        assert_eq!(a.strategy, b.strategy);
    }
}

/// `resolve_dirty_with` must return [`DesignSolveError::Mismatch`] (naming
/// both the expected and the actual tier count) instead of panicking when
/// `previous` is the wrong length; `resolve_dirty` itself must still
/// `panic!` on the exact same input.
#[test]
fn resolve_dirty_with_reports_a_mismatch_instead_of_panicking() {
    let design = three_anchor_design();
    let bogus_previous: Vec<SolvedTier> = Vec::new();
    let dirty = std::collections::BTreeSet::from([0]);
    let control = SolveControl::default();
    let err = design
        .resolve_dirty_with(&bogus_previous, &dirty, &control)
        .expect_err("empty previous list must not align with 3 tiers");
    match err {
        DesignSolveError::Mismatch(m) => {
            assert_eq!(m.expected_tiers, design.tiers.len());
            assert_eq!(m.got_tiers, 0);
        }
        other => panic!("expected DesignSolveError::Mismatch, got {other:?}"),
    }
}

#[test]
#[should_panic(expected = "resolve_dirty: `previous`")]
fn resolve_dirty_still_panics_on_a_mismatch() {
    let design = three_anchor_design();
    let bogus_previous: Vec<SolvedTier> = Vec::new();
    let dirty = std::collections::BTreeSet::from([0]);
    let _ = design.resolve_dirty(&bogus_previous, &dirty);
}

/// Above the plane cap, `solve_with` reports an error.
///
/// The hand-built designs stay far below `MAX_PLANES`, so this uses a synthetic design with
/// one massively indexed tier. `solve_with` returns
/// `DesignSolveError::Solve(SolveError::TooManyPlanes)`, while the legacy `solve` falls back
/// to `solve_meet_points` and does not error. Both entry points must agree on what counts as
/// too many planes.
#[test]
fn solve_with_reports_too_many_planes_above_the_cap_while_solve_keeps_the_legacy_fallback() {
    let mut design = Design::fresh(PreformSpec::block(2.0, 1.0, 2.0), 401, 4, 1.62);
    design.tiers.push(ConstraintTier {
        angle_deg: 90.0,
        name: "G".to_string(),
        indices: (0..401).map(f64::from).collect(),
        constraint: MeetConstraint::ScaleReference(1.0),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });

    match design.solve_with(&SolveControl::default()) {
        Err(DesignSolveError::Solve(SolveError::TooManyPlanes { planes, max })) => {
            assert!(planes > max, "planes: {planes}, max: {max}");
        }
        other => panic!("expected DesignSolveError::Solve(TooManyPlanes), got {other:?}"),
    }

    let legacy = design
        .solve()
        .expect("legacy solve never errors on plane cap");
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].strategy, SolveStrategy::ScaleReference);
}
