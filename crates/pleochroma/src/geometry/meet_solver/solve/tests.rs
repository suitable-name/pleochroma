//! Tests for the three-phase pipeline: phase-1 rank-1/named-meet selection,
//! determinism, cancellation/error contracts, and progress reporting.

use std::{collections::BTreeMap, sync::atomic::AtomicBool};

use glam::{DMat3, DVec3};

use super::{
    super::{
        MAX_PLANES, MeetConstraint, MeetPick, MeetPickPlane, MeetTierInput, NonFiniteField,
        SolveControl, SolveError, SolvePhase, SolveProgress, SolveStrategy,
        solve_meet_points_verified, tier_instance_normals,
    },
    context::SolveContext,
    solve_meet_points, solve_meet_points_with,
};
use crate::geometry::stone_metrics::ExternalProportions;

/// Hand-verifiable case: a square girdle wall plus flat table/culet (all three
/// given as scale references) fully cap a box `[-1,1] x [-0.6,0.6] x [-1,1]`. A
/// fourth facet at 45 degrees, azimuth 45 degrees (pointed straight at one of
/// the box's corner edges) must land on a *vertex level* of the box's corner
/// arrangement. Under the rank-1 prior that is the second-highest corner value
/// in its normal direction: for unit normal `n = (0.5, 1/sqrt2, 0.5)` the box
/// corners give values `+-0.5 +- 0.6/sqrt2 +- 0.5`, whose distinct levels are
/// `1 + 0.6/sqrt2` (first touch), then `1 - 0.6/sqrt2` (the rank-1 level this
/// solver must select), then lower ones.
#[test]
fn selects_the_rank1_vertex_level_against_a_capped_box() {
    let gear = 4;
    let tiers = vec![
        MeetTierInput {
            angle_deg: 90.0,
            indices: vec![0.0, 1.0, 2.0, 3.0],
            constraint: MeetConstraint::ScaleReference(1.0),
            names: vec![],
            pick: None,
        },
        MeetTierInput {
            angle_deg: 0.0,
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(0.6),
            names: vec![],
            pick: None,
        },
        MeetTierInput {
            angle_deg: -0.0,
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(0.6),
            names: vec![],
            pick: None,
        },
        MeetTierInput {
            angle_deg: 45.0,
            indices: vec![0.5],
            constraint: MeetConstraint::MeetExisting,
            names: vec![],
            pick: None,
        },
    ];

    let solved = solve_meet_points(gear, &tiers);
    assert_eq!(solved.len(), 4);
    for s in &solved[..3] {
        assert_eq!(s.strategy, SolveStrategy::ScaleReference);
    }
    assert_eq!(
        solved[3].strategy,
        SolveStrategy::DependencyOrder,
        "detail: {}",
        solved[3].detail
    );
    let expected = std::f64::consts::FRAC_1_SQRT_2.mul_add(-0.6, 1.0);
    assert!(
        (solved[3].mast - expected).abs() < 1e-6,
        "expected the rank-1 vertex level {expected}, solver produced {} (detail: {})",
        solved[3].mast,
        solved[3].detail
    );
}

/// A stated `"Meet <names>"` reference must override the rank-1 prior: the tier
/// must land exactly on the vertex where its named references meet, even when
/// that is not the rank-1 level of its own arrangement.
#[test]
fn a_stated_named_meet_overrides_the_rank1_prior() {
    // Box as above, plus Y (a 60-degree facet solved by rank-1 first), plus X,
    // which names Y + girdle + table: X must pass through the vertex where those
    // three planes meet.
    let gear = 4;
    let tiers = vec![
        MeetTierInput {
            angle_deg: 90.0,
            indices: vec![0.0, 1.0, 2.0, 3.0],
            constraint: MeetConstraint::ScaleReference(1.0),
            names: vec!["girdle".to_string()],
            pick: None,
        },
        MeetTierInput {
            angle_deg: 0.0,
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(0.6),
            names: vec!["table".to_string()],
            pick: None,
        },
        MeetTierInput {
            angle_deg: -0.0,
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(0.6),
            names: vec!["culet".to_string()],
            pick: None,
        },
        MeetTierInput {
            angle_deg: 45.0,
            indices: vec![0.5],
            constraint: MeetConstraint::MeetNamed(vec![
                "Y".to_string(),
                "girdle".to_string(),
                "table".to_string(),
            ]),
            names: vec!["X".to_string()],
            pick: None,
        },
        MeetTierInput {
            angle_deg: 60.0,
            indices: vec![0.5],
            constraint: MeetConstraint::MeetExisting,
            names: vec!["Y".to_string()],
            pick: None,
        },
    ];

    let solved = solve_meet_points(gear, &tiers);
    assert_eq!(solved.len(), 5);
    let x = &solved[3];
    assert_eq!(
        x.strategy,
        SolveStrategy::DependencyOrder,
        "X should have solved exactly, got {:?}: {}",
        x.strategy,
        x.detail
    );
    assert!(
        x.detail.contains("named reference"),
        "X should have used its stated named references, detail: {}",
        x.detail
    );

    // Verify the incidence directly: X's plane must pass through a point that
    // also lies on Y's plane, a girdle plane, and the table plane.
    let y = &solved[4];
    let theta_x = 45.0_f64.to_radians();
    let theta_y = 60.0_f64.to_radians();
    let phi = std::f64::consts::FRAC_PI_4; // index 0.5 on a 4-tooth gear
    let n_x = DVec3::new(
        theta_x.sin() * phi.cos(),
        theta_x.cos(),
        theta_x.sin() * phi.sin(),
    );
    let n_y = DVec3::new(
        theta_y.sin() * phi.cos(),
        theta_y.cos(),
        theta_y.sin() * phi.sin(),
    );
    // Vertex of {Y, girdle at azimuth 0 (n = +x), table (n = +y)}:
    let m = glam::DMat3::from_cols(n_y, DVec3::X, DVec3::Y).transpose();
    let v = m.inverse() * DVec3::new(y.mast, 1.0, 0.6);
    assert!(
        (n_x.dot(v) - x.mast).abs() < 1e-6,
        "X's mast {} should equal n_x . v = {} at the named meet vertex",
        x.mast,
        n_x.dot(v)
    );
}

/// Two identical calls produce byte-identical results.
#[test]
fn solving_is_deterministic() {
    let schedule = pleochroma_formats::asc::parse_asc(
        "GemCad 5.0\n\
         g 96 0.0\n\
         y 6 y\n\
         I 1.72\n\
         H PC 45.149  Round Trichecker-12\n\
         a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4\n\
         a -90.000000 1.07325092 92 n 2 84 76 68 60 52 44 36 28 20 12 4\n\
         a 29.730000 0.65249790 4 n A 12 20 28 36 44 52 60 68 76 84 92\n\
         a 25.000000 0.59508784 96 n B 16 32 48 64 80\n\
         a 10.000000 0.48799664 96 n C 16 32 48 64 80\n",
    )
    .expect("must parse");
    let mut tiers = super::super::meet_tier_inputs_from_asc(&schedule);
    // No stated scale references: anchor one tier per block (pavilion 0, girdle 1, crown 2).
    tiers[0].constraint = MeetConstraint::ScaleReference(schedule.tiers[0].mast);
    tiers[1].constraint = MeetConstraint::ScaleReference(schedule.tiers[1].mast);
    tiers[2].constraint = MeetConstraint::ScaleReference(schedule.tiers[2].mast);

    let a = solve_meet_points(schedule.gear_teeth_abs(), &tiers);
    let b = solve_meet_points(schedule.gear_teeth_abs(), &tiers);
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        assert!(
            x.mast.to_bits() == y.mast.to_bits(),
            "nondeterministic mast"
        );
        assert_eq!(x.strategy, y.strategy);
        assert_eq!(x.detail, y.detail);
    }
}

/// The named-meet fixture above, run through [`solve_meet_points_with`]
/// with a plain [`SolveControl::default`], must reproduce
/// [`solve_meet_points`] bit for bit -- an unused control must change
/// nothing about the result.
#[test]
fn solve_meet_points_with_a_default_control_matches_solve_meet_points_bitwise() {
    let gear = 4;
    let tiers = vec![
        MeetTierInput {
            angle_deg: 90.0,
            indices: vec![0.0, 1.0, 2.0, 3.0],
            constraint: MeetConstraint::ScaleReference(1.0),
            names: vec![],
            pick: None,
        },
        MeetTierInput {
            angle_deg: 0.0,
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(0.6),
            names: vec![],
            pick: None,
        },
        MeetTierInput {
            angle_deg: -0.0,
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(0.6),
            names: vec![],
            pick: None,
        },
        MeetTierInput {
            angle_deg: 45.0,
            indices: vec![0.5],
            constraint: MeetConstraint::MeetExisting,
            names: vec![],
            pick: None,
        },
    ];
    let plain = solve_meet_points(gear, &tiers);
    let via_with = solve_meet_points_with(gear, &tiers, &SolveControl::default())
        .expect("a default control never cancels and this fixture is under MAX_PLANES");
    assert_eq!(plain.len(), via_with.len());
    for (a, b) in plain.iter().zip(&via_with) {
        assert_eq!(a.mast.to_bits(), b.mast.to_bits());
        assert_eq!(a.strategy, b.strategy);
        assert_eq!(a.detail, b.detail);
    }
}

/// A control whose cancel flag is already set must return [`SolveError::Cancelled`]
/// immediately, without a slow fixture or a background thread. Mid-solve latency is
/// tested in `pleochroma-cut-core` (`design::tests::cancel_stops_a_large_real_solve_quickly`).
#[test]
fn solve_meet_points_with_a_precancelled_control_returns_cancelled() {
    let gear = 4;
    let tiers = vec![
        MeetTierInput {
            angle_deg: 90.0,
            indices: vec![0.0, 1.0, 2.0, 3.0],
            constraint: MeetConstraint::ScaleReference(1.0),
            names: vec![],
            pick: None,
        },
        MeetTierInput {
            angle_deg: 45.0,
            indices: vec![0.5],
            constraint: MeetConstraint::MeetExisting,
            names: vec![],
            pick: None,
        },
    ];
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let control = SolveControl::with_cancel(&cancel);
    let result = solve_meet_points_with(gear, &tiers, &control);
    assert!(matches!(result, Err(SolveError::Cancelled)), "{result:?}");
}

/// Above [`MAX_PLANES`], `solve_meet_points_with` must return a distinct
/// [`SolveError::TooManyPlanes`] naming the actual plane count and the
/// cap, while `solve_meet_points` silently keeps its
/// behavior (every tier `SolveStrategy::Failed`, except the anchor,
/// which keeps its given mast) -- the two must never diverge on what
/// counts as "too many planes".
#[test]
fn solve_meet_points_with_reports_too_many_planes_above_the_cap() {
    let gear = 400;
    // One tier with 401 index instances -- one plane instance over
    // MAX_PLANES all by itself (`total_planes = 6 blank + 401`).
    let indices: Vec<f64> = (0..401).map(f64::from).collect();
    let tiers = vec![MeetTierInput {
        angle_deg: 90.0,
        indices,
        constraint: MeetConstraint::ScaleReference(1.0),
        names: vec![],
        pick: None,
    }];

    let via_with = solve_meet_points_with(gear, &tiers, &SolveControl::default());
    match via_with {
        Err(SolveError::TooManyPlanes { planes, max }) => {
            assert_eq!(max, MAX_PLANES);
            assert!(planes > MAX_PLANES, "planes: {planes}");
        }
        other => panic!("expected SolveError::TooManyPlanes, got {other:?}"),
    }

    let legacy = solve_meet_points(gear, &tiers);
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].strategy, SolveStrategy::ScaleReference);
    assert!((legacy[0].mast - 1.0).abs() < 1e-12);
}

/// A NaN `ScaleReference` mast must be rejected up front, never
/// silently folded away by the scale-prior/domination-limit computation and
/// never reach candidate-vertex geometry as a NaN plane offset.
#[test]
fn solve_meet_points_with_rejects_a_nan_mast() {
    let gear = 96;
    let tiers = vec![
        MeetTierInput {
            angle_deg: 90.0,
            indices: vec![0.0, 24.0, 48.0, 72.0],
            constraint: MeetConstraint::ScaleReference(f64::NAN),
            names: vec![],
            pick: None,
        },
        MeetTierInput {
            angle_deg: 41.0,
            indices: vec![12.0, 36.0, 60.0, 84.0],
            constraint: MeetConstraint::MeetExisting,
            names: vec![],
            pick: None,
        },
    ];
    let result = solve_meet_points_with(gear, &tiers, &SolveControl::default());
    match result {
        Err(SolveError::NonFiniteInput { tier, field }) => {
            assert_eq!(tier, 0);
            assert_eq!(field, NonFiniteField::Mast);
        }
        other => panic!(
            "expected Err(SolveError::NonFiniteInput), got {other:?} (never a solved/Closed result)"
        ),
    }
}

/// An infinite index must be rejected the same way a NaN mast is --
/// checked before `SolveContext::new` derives anything from it.
#[test]
fn solve_meet_points_with_rejects_an_infinite_index() {
    let gear = 96;
    let tiers = vec![
        MeetTierInput {
            angle_deg: 90.0,
            indices: vec![0.0, 24.0, 48.0, f64::INFINITY],
            constraint: MeetConstraint::ScaleReference(1.0),
            names: vec![],
            pick: None,
        },
        MeetTierInput {
            angle_deg: 41.0,
            indices: vec![12.0, 36.0, 60.0, 84.0],
            constraint: MeetConstraint::MeetExisting,
            names: vec![],
            pick: None,
        },
    ];
    let result = solve_meet_points_with(gear, &tiers, &SolveControl::default());
    match result {
        Err(SolveError::NonFiniteInput { tier, field }) => {
            assert_eq!(tier, 0);
            assert_eq!(field, NonFiniteField::Index);
        }
        other => panic!(
            "expected Err(SolveError::NonFiniteInput), got {other:?} (never a solved/Closed result)"
        ),
    }
}

/// A non-finite `angle_deg` is rejected the same way.
#[test]
fn solve_meet_points_with_rejects_a_nan_angle() {
    let gear = 96;
    let tiers = vec![MeetTierInput {
        angle_deg: f64::NAN,
        indices: vec![0.0],
        constraint: MeetConstraint::ScaleReference(1.0),
        names: vec![],
        pick: None,
    }];
    let result = solve_meet_points_with(gear, &tiers, &SolveControl::default());
    match result {
        Err(SolveError::NonFiniteInput { tier, field }) => {
            assert_eq!(tier, 0);
            assert_eq!(field, NonFiniteField::AngleDeg);
        }
        other => panic!("expected Err(SolveError::NonFiniteInput), got {other:?}"),
    }
}

/// The progress callback must report [`SolvePhase::Constructive`] before
/// [`SolvePhase::LeastSquares`] before [`SolvePhase::Refine`] (the fixed
/// phase order the module docs describe), `sweep` must never decrease
/// within one phase's own run of reports, and every phase must reach
/// `blocks_done == blocks_total` at least once (each phase fully
/// processes every tier, or reports zero-work-to-do, before moving on).
#[test]
fn solve_meet_points_with_reports_monotonic_progress_reaching_every_phases_total() {
    let schedule = pleochroma_formats::asc::parse_asc(
        "GemCad 5.0\n\
         g 96 0.0\n\
         y 6 y\n\
         I 1.72\n\
         H PC 45.149  Round Trichecker-12\n\
         a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4\n\
         a -90.000000 1.07325092 92 n 2 84 76 68 60 52 44 36 28 20 12 4\n\
         a 29.730000 0.65249790 4 n A 12 20 28 36 44 52 60 68 76 84 92\n\
         a 25.000000 0.59508784 96 n B 16 32 48 64 80\n\
         a 10.000000 0.48799664 96 n C 16 32 48 64 80\n",
    )
    .expect("must parse");
    let mut tiers = super::super::meet_tier_inputs_from_asc(&schedule);
    tiers[0].constraint = MeetConstraint::ScaleReference(schedule.tiers[0].mast);
    tiers[1].constraint = MeetConstraint::ScaleReference(schedule.tiers[1].mast);
    tiers[2].constraint = MeetConstraint::ScaleReference(schedule.tiers[2].mast);

    let reports: std::cell::RefCell<Vec<SolveProgress>> = std::cell::RefCell::new(Vec::new());
    let record = |p: SolveProgress| reports.borrow_mut().push(p);
    let control = SolveControl::default().reporting(&record);
    let _ = solve_meet_points_with(schedule.gear_teeth_abs(), &tiers, &control)
        .expect("under MAX_PLANES, never cancelled");

    let reports = reports.into_inner();
    assert!(!reports.is_empty(), "no progress reported at all");

    // Fixed phase order, no phase revisited once left.
    let phase_rank = |p: SolvePhase| match p {
        SolvePhase::Constructive => 0,
        SolvePhase::LeastSquares => 1,
        SolvePhase::Refine => 2,
    };
    let mut last_rank = 0;
    let mut last_sweep_in_phase = 0u32;
    let mut reached_total: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();
    for r in &reports {
        let rank = phase_rank(r.phase);
        assert!(
            rank >= last_rank,
            "phase went backwards: {:?} after rank {last_rank}",
            r.phase
        );
        if rank != last_rank {
            last_sweep_in_phase = 0;
        }
        assert!(
            r.sweep >= last_sweep_in_phase,
            "sweep decreased within {:?}: {} after {last_sweep_in_phase}",
            r.phase,
            r.sweep
        );
        last_sweep_in_phase = r.sweep;
        last_rank = rank;
        assert!(
            r.blocks_done <= r.blocks_total,
            "blocks_done exceeded blocks_total: {r:?}"
        );
        assert_eq!(
            r.blocks_total,
            u32::try_from(tiers.len()).expect("the tier count fits u32")
        );
        if r.blocks_done == r.blocks_total {
            reached_total.insert(rank);
        }
    }
    assert!(
        reached_total.contains(&0),
        "Constructive phase never reported blocks_done == blocks_total: {reports:?}"
    );
    assert!(
        reached_total.contains(&2),
        "Refine phase never reported blocks_done == blocks_total: {reports:?}"
    );
}

/// Solved masts scale exactly with the absolute `ScaleReference` anchors, and every tier
/// keeps its [`SolveStrategy`] at any absolute scale. The geometric thresholds are tuned
/// for masts of order 1, so this guards `SolveContext::scale_norm`'s normalisation.
#[test]
fn solve_meet_points_scales_masts_proportionally_with_scale_reference() {
    const GIRDLE: [f64; 16] = [
        0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
        90.0,
    ];
    const BREAK: [f64; 16] = [
        95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0, 83.0,
        85.0,
    ];
    const MAIN: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
    const STAR: [f64; 8] = [6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0];

    let rbc = |k: f64| -> Vec<MeetTierInput> {
        let t = |angle_deg: f64, indices: &[f64], constraint: MeetConstraint| MeetTierInput {
            angle_deg,
            indices: indices.to_vec(),
            constraint,
            names: vec![],
            pick: None,
        };
        vec![
            t(0.0, &[], MeetConstraint::ScaleReference(0.32 * k)),
            t(15.0, &STAR, MeetConstraint::MeetExisting),
            t(34.5, &MAIN, MeetConstraint::MeetExisting),
            t(41.0, &BREAK, MeetConstraint::MeetExisting),
            t(90.0, &GIRDLE, MeetConstraint::ScaleReference(1.0 * k)),
            t(-41.0, &MAIN, MeetConstraint::MeetExisting),
            t(-42.5, &BREAK, MeetConstraint::MeetExisting),
            t(-0.0, &[], MeetConstraint::ScaleReference(0.88 * k)),
        ]
    };

    let gear = 96;
    let baseline = solve_meet_points(gear, &rbc(1.0));
    for &k in &[0.001, 0.01, 0.1, 10.0, 20.0, 40.0, 70.0, 100.0] {
        let solved = solve_meet_points(gear, &rbc(k));
        assert_eq!(solved.len(), baseline.len());
        for (i, (s, b)) in solved.iter().zip(&baseline).enumerate() {
            assert_eq!(
                s.strategy, b.strategy,
                "tier {i}: strategy changed at k={k} ({:?} vs baseline {:?})",
                s.strategy, b.strategy
            );
            let rel = (s.mast / k - b.mast).abs() / b.mast.abs().max(1e-9);
            assert!(
                rel < 1e-6,
                "tier {i}: mast/k {} vs baseline {} at k={k} (rel {rel})",
                s.mast / k,
                b.mast
            );
        }
    }
}

// ---- Picked corners (`MeetTierInput::pick`). ----

const PICK_GEAR: u32 = 8;
const PICK_G1: usize = 0;
const PICK_P1: usize = 1;
const PICK_P2: usize = 2;
const PICK_P3: usize = 3;

/// A girdle `G1` (8 indices, scale `scale`), two pavilion tiers `P1` (even indices) and `P2`
/// (odd indices) anchored at `0.8 * scale`, and a pavilion tier `P3` that meets both.
fn pick_fixture(
    scale: f64,
    p2_angle: f64,
    p3_indices: &[f64],
    pick: Option<MeetPick>,
) -> Vec<MeetTierInput> {
    let tier =
        |angle_deg: f64, indices: &[f64], constraint: MeetConstraint, name: &str| MeetTierInput {
            angle_deg,
            indices: indices.to_vec(),
            constraint,
            names: vec![name.to_string()],
            pick: None,
        };
    vec![
        tier(
            90.0,
            &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0],
            MeetConstraint::ScaleReference(scale),
            "G1",
        ),
        tier(
            -40.0,
            &[0.0, 2.0, 4.0, 6.0],
            MeetConstraint::ScaleReference(0.8 * scale),
            "P1",
        ),
        tier(
            p2_angle,
            &[1.0, 3.0, 5.0, 7.0],
            MeetConstraint::ScaleReference(0.8 * scale),
            "P2",
        ),
        MeetTierInput {
            pick,
            ..tier(
                -60.0,
                p3_indices,
                MeetConstraint::MeetNamed(vec!["P1".to_string(), "P2".to_string()]),
                "P3",
            )
        },
    ]
}

/// A pick of three `(tier position, index)` planes.
fn pick_of(planes: [(usize, f64); 3]) -> MeetPick {
    MeetPick {
        planes: planes.map(|(tier, index)| MeetPickPlane { tier, index }),
    }
}

/// The corner where the three anchored planes `planes` meet, solved with a matrix inverse
/// independent of the solver's own formula.
fn pick_corner(tiers: &[MeetTierInput], planes: [(usize, f64); 3]) -> DVec3 {
    let normals = tier_instance_normals(PICK_GEAR, tiers);
    let rows = planes.map(|(t, index)| {
        let instance = tiers[t]
            .indices
            .iter()
            .position(|&i| (i - index).abs() < 1e-9)
            .expect("the plane's index exists");
        let MeetConstraint::ScaleReference(mast) = tiers[t].constraint else {
            panic!("a corner plane of the fixture is anchored");
        };
        (normals[t][instance], mast)
    });
    let matrix = DMat3::from_cols(rows[0].0, rows[1].0, rows[2].0).transpose();
    matrix.inverse() * DVec3::new(rows[0].1, rows[1].1, rows[2].1)
}

/// The mast of `P3` through `corner`: the deepest of its instances.
fn pick_mast_through(tiers: &[MeetTierInput], corner: DVec3) -> f64 {
    tier_instance_normals(PICK_GEAR, tiers)[PICK_P3]
        .iter()
        .map(|n| n.dot(corner))
        .fold(f64::NEG_INFINITY, f64::max)
}

const P3_ALL: [f64; 4] = [0.0, 2.0, 4.0, 6.0];
const GIRDLE_CORNER: [(usize, f64); 3] = [(PICK_P1, 0.0), (PICK_P2, 1.0), (PICK_G1, 0.0)];
const CULET_CORNER: [(usize, f64); 3] = [(PICK_P1, 0.0), (PICK_P2, 1.0), (PICK_P1, 2.0)];

/// Without a pick the tier takes the shallowest vertex its named facets share (the girdle
/// corner here). A pick at the girdle gives that same mast, a pick at the culet gives a
/// different one, and each is the mast at which the tier's plane passes through the corner.
#[test]
fn a_picked_corner_sets_the_mast_where_the_shallowest_rule_would_not() {
    let plain = solve_meet_points(PICK_GEAR, &pick_fixture(1.0, -40.0, &P3_ALL, None));
    let reference = pick_fixture(1.0, -40.0, &P3_ALL, None);

    let girdle_corner = pick_corner(&reference, GIRDLE_CORNER);
    let culet_corner = pick_corner(&reference, CULET_CORNER);
    let girdle_mast = pick_mast_through(&reference, girdle_corner);
    let culet_mast = pick_mast_through(&reference, culet_corner);
    assert!(
        (girdle_mast - culet_mast).abs() > 0.1,
        "the two corners must give different masts ({girdle_mast} vs {culet_mast})"
    );
    assert!(
        (plain[PICK_P3].mast - girdle_mast).abs() < 1e-6,
        "without a pick the shallowest level is the girdle corner: {} vs {girdle_mast} ({})",
        plain[PICK_P3].mast,
        plain[PICK_P3].detail
    );

    for (planes, expected, label) in [
        (GIRDLE_CORNER, girdle_mast, "(P1@0, P2@1, G1@0)"),
        (CULET_CORNER, culet_mast, "(P1@0, P2@1, P1@2)"),
    ] {
        let tiers = pick_fixture(1.0, -40.0, &P3_ALL, Some(pick_of(planes)));
        let solved = solve_meet_points(PICK_GEAR, &tiers);
        let p3 = &solved[PICK_P3];
        assert_eq!(p3.strategy, SolveStrategy::DependencyOrder, "{}", p3.detail);
        assert!(
            (p3.mast - expected).abs() < 1e-9,
            "picked {label}: mast {} vs expected {expected}",
            p3.mast
        );
        assert_eq!(p3.detail, format!("picked corner {label}"));
    }
}

/// A picked corner follows its planes: after an angle of a picked facet's tier changes, the
/// tier's plane still passes through the recomputed corner.
#[test]
fn a_picked_corner_follows_its_planes_when_an_angle_changes() {
    for p2_angle in [-40.0, -37.0, -35.0] {
        let tiers = pick_fixture(1.0, p2_angle, &P3_ALL, Some(pick_of(GIRDLE_CORNER)));
        let corner = pick_corner(&tiers, GIRDLE_CORNER);
        let solved = solve_meet_points(PICK_GEAR, &tiers);
        let mast = solved[PICK_P3].mast;
        let on_plane = tier_instance_normals(PICK_GEAR, &tiers)[PICK_P3]
            .iter()
            .any(|n| (n.dot(corner) - mast).abs() < 1e-9);
        assert!(
            on_plane,
            "P2 at {p2_angle}: no P3 instance passes through the corner (mast {mast}, {})",
            solved[PICK_P3].detail
        );
        assert!((mast - pick_mast_through(&tiers, corner)).abs() < 1e-9);
    }
}

/// A pick that cannot be used solves exactly like the same input without a pick, and the
/// tier's `detail` names the cause.
#[test]
fn an_unusable_pick_solves_as_if_there_were_none() {
    /// A case name, the tier angles, the three picked facets and the expected refusal.
    type Case<'a> = (&'a str, Vec<f64>, [(usize, f64); 3], &'a str);
    let missing = "a picked facet does not exist";
    let parallel = "the picked facets do not meet in one point";
    let range = "the picked point is out of range";
    let cases: Vec<Case> = vec![
        (
            "tier out of range",
            P3_ALL.to_vec(),
            [(9, 0.0), (PICK_P2, 1.0), (PICK_G1, 0.0)],
            missing,
        ),
        (
            "self reference",
            P3_ALL.to_vec(),
            [(PICK_P3, 0.0), (PICK_P2, 1.0), (PICK_G1, 0.0)],
            missing,
        ),
        (
            "missing index",
            P3_ALL.to_vec(),
            [(PICK_P1, 3.0), (PICK_P2, 1.0), (PICK_G1, 0.0)],
            missing,
        ),
        (
            "parallel planes",
            P3_ALL.to_vec(),
            [(PICK_P1, 0.0), (PICK_P1, 0.0), (PICK_P2, 1.0)],
            parallel,
        ),
        (
            "non-positive target",
            vec![0.0],
            [(PICK_P1, 4.0), (PICK_P2, 5.0), (PICK_G1, 4.0)],
            range,
        ),
    ];
    for (label, p3_indices, planes, cause) in cases {
        let plain = solve_meet_points(PICK_GEAR, &pick_fixture(1.0, -40.0, &p3_indices, None));
        let picked = solve_meet_points(
            PICK_GEAR,
            &pick_fixture(1.0, -40.0, &p3_indices, Some(pick_of(planes))),
        );
        for (i, (a, b)) in plain.iter().zip(&picked).enumerate() {
            assert_eq!(a.mast.to_bits(), b.mast.to_bits(), "{label}: tier {i} mast");
            assert_eq!(a.strategy, b.strategy, "{label}: tier {i} strategy");
        }
        assert!(
            picked[PICK_P3].detail.starts_with(&plain[PICK_P3].detail),
            "{label}: {}",
            picked[PICK_P3].detail
        );
        assert!(
            picked[PICK_P3]
                .detail
                .ends_with(&format!(" (picked corner not usable: {cause})")),
            "{label}: {}",
            picked[PICK_P3].detail
        );
    }
}

/// A picked tier that comes before one of its corner's tiers in file order waits for it and
/// then settles on the pick.
#[test]
fn a_picked_tier_waits_for_later_tiers_of_its_corner() {
    let tier =
        |angle_deg: f64, indices: &[f64], constraint: MeetConstraint, name: &str| MeetTierInput {
            angle_deg,
            indices: indices.to_vec(),
            constraint,
            names: vec![name.to_string()],
            pick: None,
        };
    let tiers = vec![
        tier(
            90.0,
            &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0],
            MeetConstraint::ScaleReference(1.0),
            "G1",
        ),
        tier(
            -40.0,
            &[0.0, 2.0, 4.0, 6.0],
            MeetConstraint::ScaleReference(0.8),
            "P1",
        ),
        MeetTierInput {
            pick: Some(pick_of([(1, 0.0), (3, 1.0), (0, 0.0)])),
            ..tier(
                -60.0,
                &P3_ALL,
                MeetConstraint::MeetNamed(vec!["P1".to_string(), "P2".to_string()]),
                "P3",
            )
        },
        tier(
            -40.0,
            &[1.0, 3.0, 5.0, 7.0],
            MeetConstraint::MeetExisting,
            "P2",
        ),
    ];
    let solved = solve_meet_points(PICK_GEAR, &tiers);
    assert_eq!(solved[2].strategy, SolveStrategy::DependencyOrder);
    assert_eq!(solved[2].detail, "picked corner (P1@0, P2@1, G1@0)");
    assert!(solved[2].mast.is_finite() && solved[2].mast > 0.0);
}

/// The repair search never overrides a picked tier: its phase-1 pick is not a vertex level.
#[test]
fn the_repair_search_leaves_a_picked_tier_alone() {
    let tiers = pick_fixture(1.0, -40.0, &P3_ALL, Some(pick_of(GIRDLE_CORNER)));
    let ctx = SolveContext::new(PICK_GEAR, &tiers);
    let result = ctx
        .run_pipeline(&BTreeMap::new(), &BTreeMap::new(), &SolveControl::default())
        .expect("an unused control never cancels");
    assert!(
        result.last_pick[PICK_P3].is_none(),
        "a picked tier carries no vertex-level pick"
    );

    let (solved, _report) =
        solve_meet_points_verified(PICK_GEAR, &tiers, &ExternalProportions::default(), &[]);
    let plain = solve_meet_points(PICK_GEAR, &tiers);
    assert_eq!(
        solved[PICK_P3].mast.to_bits(),
        plain[PICK_P3].mast.to_bits()
    );
}

/// Scaling every anchor by 8 scales the picked tier's mast by 8.
#[test]
fn a_picked_mast_scales_with_the_anchors() {
    let base = solve_meet_points(
        PICK_GEAR,
        &pick_fixture(1.0, -40.0, &P3_ALL, Some(pick_of(GIRDLE_CORNER))),
    );
    let scaled = solve_meet_points(
        PICK_GEAR,
        &pick_fixture(8.0, -40.0, &P3_ALL, Some(pick_of(GIRDLE_CORNER))),
    );
    for (i, (a, b)) in base.iter().zip(&scaled).enumerate() {
        assert!(
            a.mast.mul_add(8.0, -b.mast).abs() < 1e-9 * b.mast.abs().max(1.0),
            "tier {i}: {} * 8 vs {}",
            a.mast,
            b.mast
        );
    }
}

/// A cancel flag that is already set stops a solve that carries picks.
#[test]
fn a_cancelled_solve_with_picks_returns_cancelled() {
    let tiers = pick_fixture(1.0, -40.0, &P3_ALL, Some(pick_of(GIRDLE_CORNER)));
    let cancel = AtomicBool::new(true);
    let result = solve_meet_points_with(PICK_GEAR, &tiers, &SolveControl::with_cancel(&cancel));
    assert!(matches!(result, Err(SolveError::Cancelled)), "{result:?}");
}

/// Two runs over the same picked input are bit-identical.
#[test]
fn two_picked_solves_are_bit_identical() {
    let tiers = pick_fixture(1.0, -37.0, &P3_ALL, Some(pick_of(GIRDLE_CORNER)));
    let first = solve_meet_points(PICK_GEAR, &tiers);
    let second = solve_meet_points(PICK_GEAR, &tiers);
    for (a, b) in first.iter().zip(&second) {
        assert_eq!(a.mast.to_bits(), b.mast.to_bits());
        assert_eq!(a.strategy, b.strategy);
        assert_eq!(a.detail, b.detail);
    }
}
