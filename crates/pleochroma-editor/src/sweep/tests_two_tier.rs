//! Tests for the two-tier sweep: the grid (the current pair is a cell, the refusals), the
//! run (order across worker counts, a 1 x N grid against the one-tier rows, cancel), the
//! one-step apply, the refinement and the CSV.

use super::*;
use crate::solve_policy::SolveCostEstimate;
use pleochroma::optics::raytracer::LightingPreset;
use pleochroma_cut_core::{ConstraintTier, History, PreformSpec, ScheduleMeta};

/// Positions in the standard round brilliant template.
const CROWN_MAIN: usize = 2;
const PAVILION_MAIN: usize = 5;
const LOWER_GIRDLE: usize = 6;

fn rbc() -> Design {
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    design.ensure_tier_ids();
    design
}

fn session_of(design: Design) -> EditorSession {
    EditorSession::with_history(design, History::new())
}

fn range(from_deg: f64, to_deg: f64, step_deg: f64) -> SweepRange {
    SweepRange {
        from_deg,
        to_deg,
        step_deg,
    }
}

fn scene(material: &GemMaterial) -> SweepScene<'_> {
    SweepScene {
        material,
        environment: LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
    }
}

fn options(workers: usize) -> SweepOptions {
    SweepOptions {
        tilt_average: false,
        workers,
    }
}

/// Crown Main (across) by Pavilion Main (down).
fn plan_of(design: &Design, across: SweepRange, down: SweepRange) -> Sweep2dPlan {
    plan_sweep_2d(design, CROWN_MAIN, &across, PAVILION_MAIN, &down).expect("a valid grid")
}

fn run(design: &Design, plan: &Sweep2dPlan, workers: usize) -> Sweep2dOutcome {
    let material = GemMaterial::diamond();
    sweep_two_tier_angles(
        design,
        plan,
        &options(workers),
        &scene(&material),
        &AtomicBool::new(false),
        &|_, _| {},
    )
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn the_current_pair_is_always_a_cell() {
    let design = rbc();
    // The grids miss the current pair (34.5 and -41), so each axis gains it.
    let plan = plan_of(&design, range(36.0, 37.0, 0.5), range(43.0, 44.0, 1.0));
    assert_eq!(plan.angles_a, vec![34.5, 36.0, 36.5, 37.0]);
    assert_eq!(plan.angles_b, vec![-41.0, -43.0, -44.0]);
    assert_eq!((plan.current_col, plan.current_row), (0, 0));
    assert_eq!(plan.len(), 12);
    assert_eq!(plan.tier_a_name, "Crown Main");
    assert_eq!(plan.tier_b_name, "Pavilion Main");
    assert!(near(plan.current_a_deg, 34.5) && near(plan.current_b_deg, -41.0));

    let outcome = run(&design, &plan, 1);
    let current = outcome.current_cell().expect("the current cell finished");
    assert_eq!((current.col, current.row), (0, 0));
    assert!(near(current.angle_a, 34.5) && near(current.angle_b, -41.0));
    assert!(current.is_valid(), "{:?}", current.notes);
    assert_eq!(outcome.cells.iter().filter(|c| c.is_current).count(), 1);
}

#[test]
fn the_cells_come_back_in_row_major_order_whatever_the_workers() {
    let design = rbc();
    let plan = plan_of(&design, range(34.0, 35.0, 0.5), range(40.0, 42.0, 1.0));
    assert_eq!((plan.columns(), plan.rows()), (3, 3));
    let alone = run(&design, &plan, 1);
    let together = run(&design, &plan, 3);
    let many = run(&design, &plan, 64);
    assert_eq!(alone.cells.len(), 9);
    assert_eq!(alone, together);
    assert_eq!(alone, many);
    for (index, cell) in alone.cells.iter().enumerate() {
        assert_eq!((cell.col, cell.row), (index % 3, index / 3));
        assert!(near(cell.angle_a, plan.angles_a[cell.col]));
        assert!(near(cell.angle_b, plan.angles_b[cell.row]));
        assert_eq!(alone.cell_at(cell.col, cell.row), Some(cell));
    }
    assert!(!alone.cancelled);
    assert_eq!(alone.requested, 9);
}

#[test]
fn a_sweep_does_not_touch_the_design_it_was_given() {
    let design = rbc();
    let before = design.clone();
    let plan = plan_of(&design, range(34.0, 35.0, 1.0), range(40.0, 41.0, 1.0));
    let _ = run(&design, &plan, 2);
    assert_eq!(design, before);
}

#[test]
fn a_tier_that_follows_a_relation_is_refused_on_its_axis() {
    let mut session = session_of(rbc());
    session
        .set_tier_relation(LOWER_GIRDLE, "[Pavilion Main] + 1.5")
        .expect("a relation");
    let design = session.design.clone();
    // The Lower Girdle reads the Pavilion Main: one drives the other.
    let error = plan_sweep_2d(
        &design,
        PAVILION_MAIN,
        &range(40.0, 42.0, 1.0),
        LOWER_GIRDLE,
        &range(41.0, 43.0, 1.0),
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            Sweep2dError::Axis {
                across: false,
                error: SweepError::Driven { .. }
            }
        ),
        "{error:?}"
    );
    assert!(error.to_string().contains("follows a relation"));
    // The same pair the other way round names the other axis.
    assert!(matches!(
        plan_sweep_2d(
            &design,
            LOWER_GIRDLE,
            &range(41.0, 43.0, 1.0),
            PAVILION_MAIN,
            &range(40.0, 42.0, 1.0),
        ),
        Err(Sweep2dError::Axis { across: true, .. })
    ));
}

#[test]
fn the_same_tier_twice_is_refused() {
    let design = rbc();
    let error = plan_sweep_2d(
        &design,
        CROWN_MAIN,
        &range(33.0, 36.0, 1.0),
        CROWN_MAIN,
        &range(33.0, 36.0, 1.0),
    )
    .unwrap_err();
    assert_eq!(
        error,
        Sweep2dError::SameTier {
            tier: "Crown Main".to_owned()
        }
    );
    assert!(error.to_string().starts_with("Crown Main is named twice"));
}

#[test]
fn more_than_the_cell_cap_is_refused() {
    let design = rbc();
    // 31 angles each way (33.0 to 36.0 holds the current 34.5; 38.0 to 41.0 holds -41).
    let error = plan_sweep_2d(
        &design,
        CROWN_MAIN,
        &range(33.0, 36.0, 0.1),
        PAVILION_MAIN,
        &range(38.0, 41.0, 0.1),
    )
    .unwrap_err();
    assert_eq!(
        error,
        Sweep2dError::TooManyCells {
            cells: 961,
            max: MAX_SWEEP_CELLS
        }
    );
    assert!(error.to_string().contains("961 angle pairs"));
    // Thirty by thirty is the largest grid.
    let plan = plan_sweep_2d(
        &design,
        CROWN_MAIN,
        &range(33.0, 35.9, 0.1),
        PAVILION_MAIN,
        &range(38.1, 41.0, 0.1),
    )
    .expect("thirty by thirty is taken");
    assert_eq!((plan.columns(), plan.rows()), (30, 30));
    assert_eq!(plan.len(), MAX_SWEEP_CELLS);
}

#[test]
fn an_axis_refusal_carries_the_one_tier_message() {
    let design = rbc();
    let error = plan_sweep_2d(
        &design,
        CROWN_MAIN,
        &range(33.0, 36.0, 0.0),
        PAVILION_MAIN,
        &range(40.0, 42.0, 1.0),
    )
    .unwrap_err();
    assert_eq!(
        error,
        Sweep2dError::Axis {
            across: true,
            error: SweepError::StepNotPositive
        }
    );
    assert_eq!(error.to_string(), "The step must be more than 0.");
}

#[test]
fn using_a_pair_is_one_undo_step_that_changes_and_restores_both() {
    let mut session = session_of(rbc());
    session
        .set_tier_relation(LOWER_GIRDLE, "[Pavilion Main] + 1.5")
        .expect("a relation");
    let before = session.design.clone();

    let change = apply_sweep_angles(&mut session, (CROWN_MAIN, 35.0), (PAVILION_MAIN, -40.5))
        .expect("applied");
    assert!(change.is_some());
    assert!(near(session.design.tiers[CROWN_MAIN].angle_deg, 35.0));
    assert!(near(session.design.tiers[PAVILION_MAIN].angle_deg, -40.5));
    assert!(
        near(session.design.tiers[LOWER_GIRDLE].angle_deg, -42.0),
        "the follower moved in the same step"
    );

    // One undo brings both angles (and the follower) back.
    assert!(session.undo().expect("undo").is_some());
    assert_eq!(session.design, before);
    assert!(near(session.design.tiers[CROWN_MAIN].angle_deg, 34.5));
    assert!(near(session.design.tiers[PAVILION_MAIN].angle_deg, -41.0));
}

#[test]
fn using_the_pair_a_design_already_has_changes_nothing() {
    let mut session = session_of(rbc());
    let generation = session.current_generation();
    assert!(matches!(
        apply_sweep_angles(&mut session, (CROWN_MAIN, 34.5), (PAVILION_MAIN, -41.0)),
        Ok(None)
    ));
    assert_eq!(session.current_generation(), generation);
    assert!(matches!(
        apply_sweep_angles(&mut session, (CROWN_MAIN, 34.5), (99, -41.0)),
        Err(SessionEditError::Edit(_))
    ));
    assert!(matches!(
        apply_sweep_angles(&mut session, (CROWN_MAIN, 35.0), (CROWN_MAIN, 36.0)),
        Err(SessionEditError::Edit(_))
    ));
}

#[test]
fn a_one_by_n_grid_equals_the_one_tier_rows() {
    let design = rbc();
    // The across axis is just the current angle; the down axis is the one-tier sweep's.
    let plan = plan_of(&design, range(34.5, 34.5, 1.0), range(43.0, 39.0, 1.0));
    assert_eq!((plan.columns(), plan.rows()), (1, 5));
    let grid = run(&design, &plan, 2);

    let one_tier_plan =
        plan_sweep(&design, PAVILION_MAIN, range(43.0, 39.0, 1.0)).expect("a valid plan");
    let material = GemMaterial::diamond();
    let rows = sweep_tier_angle(
        &design,
        &one_tier_plan,
        &scene(&material),
        &options(2),
        &AtomicBool::new(false),
        &|_, _| {},
    );

    assert_eq!(grid.cells.len(), rows.rows.len());
    for (cell, row) in grid.cells.iter().zip(&rows.rows) {
        assert_eq!(cell.angle_b.to_bits(), row.angle_deg.to_bits());
        assert_eq!(cell.is_current, row.is_current);
        assert_eq!(cell.metrics, row.metrics);
        assert_eq!(cell.notes, row.notes);
    }
}

#[test]
fn score_design_matches_the_current_cell_with_and_without_the_solved_masts() {
    let design = rbc();
    let plan = plan_of(&design, range(34.5, 34.5, 1.0), range(41.0, 41.0, 1.0));
    let outcome = run(&design, &plan, 1);
    let cell = outcome.current_cell().expect("finished").metrics;

    let material = GemMaterial::diamond();
    let cancel = AtomicBool::new(false);
    let alone = score_design(&design, None, &scene(&material), &options(1), &cancel);
    assert_eq!(alone, Ok(cell));
    let solved = design.solve().expect("the template solves");
    let given = score_design(
        &design,
        Some(&solved),
        &scene(&material),
        &options(1),
        &cancel,
    );
    assert_eq!(given, Ok(cell));
    assert_eq!(
        score_design(
            &design,
            None,
            &scene(&material),
            &options(1),
            &AtomicBool::new(true)
        ),
        Err(Cancelled)
    );
}

#[test]
fn cancelling_keeps_the_finished_cells_in_order() {
    let design = rbc();
    let plan = plan_of(&design, range(34.0, 35.0, 0.5), range(40.0, 42.0, 1.0));
    let material = GemMaterial::diamond();
    let cancel = AtomicBool::new(false);
    let progress = |done: usize, _total: usize| {
        if done >= 2 {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let outcome = sweep_two_tier_angles(
        &design,
        &plan,
        &options(1),
        &scene(&material),
        &cancel,
        &progress,
    );
    assert!(outcome.cancelled);
    assert_eq!(outcome.requested, 9);
    assert_eq!(outcome.cells.len(), 2);
    let places: Vec<(usize, usize)> = outcome.cells.iter().map(|c| (c.col, c.row)).collect();
    assert_eq!(
        places,
        vec![(0, 0), (1, 0)],
        "the first row, flattest first"
    );
    assert!(outcome.cell_at(2, 2).is_none());

    let stopped = sweep_two_tier_angles(
        &design,
        &plan,
        &options(2),
        &scene(&material),
        &AtomicBool::new(true),
        &|_, _| {},
    );
    assert!(stopped.cancelled);
    assert_eq!(
        stopped.cells,
        [] as [crate::sweep::two_tier::Sweep2dCell; 0]
    );
}

#[test]
fn refining_makes_a_finer_grid_around_the_cell_that_keeps_the_current_pair() {
    let design = rbc();
    let plan = plan_of(&design, range(33.5, 35.5, 0.5), range(39.0, 43.0, 1.0));
    assert_eq!((plan.columns(), plan.rows()), (5, 5));
    // Around (column 3 = 35.0, row 3 = -42.0), five times finer.
    let finer = refine_around(&plan, &design, (3, 3), 5.0).expect("a refined grid");
    assert_eq!((finer.tier_a, finer.tier_b), (CROWN_MAIN, PAVILION_MAIN));
    let a: Vec<f64> = finer.angles_a.iter().map(|angle| angle.abs()).collect();
    assert!(a.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(a.iter().any(|angle| near(*angle, 35.0)), "{a:?}");
    assert!(
        a.iter().any(|angle| near(*angle, 34.5)),
        "the current angle stays"
    );
    assert!(near(a[1] - a[0], 0.1) || near(a[2] - a[1], 0.1), "{a:?}");
    assert!(finer.len() >= plan.len() && finer.len() <= plan.len() + 11);
    assert!(near(finer.angles_b[finer.current_row], -41.0));
    // A factor that is not positive is refused.
    assert!(refine_around(&plan, &design, (0, 0), 0.0).is_err());
    // Near the flat end the range slides rather than leaving the allowed angles.
    let flat = plan_sweep_2d(
        &design,
        CROWN_MAIN,
        &range(0.2, 0.6, 0.2),
        PAVILION_MAIN,
        &range(40.0, 42.0, 1.0),
    )
    .expect("a valid grid");
    let refined = refine_around(&flat, &design, (0, 0), 0.5).expect("a refined grid");
    assert!(
        refined
            .angles_a
            .iter()
            .all(|angle| angle.abs() >= MIN_SWEEP_ANGLE_DEG)
    );
}

#[test]
fn the_csv_has_one_line_per_cell_in_row_major_order() {
    let design = rbc();
    let plan = plan_of(&design, range(34.0, 35.0, 1.0), range(40.0, 42.0, 1.0));
    let outcome = run(&design, &plan, 2);
    let csv = sweep2d_csv(&outcome);
    let lines: Vec<&str> = csv.split("\r\n").collect();
    // A header, one line per cell, and the empty remainder after the final line end.
    assert_eq!(lines.len(), 1 + outcome.cells.len() + 1);
    assert!(lines[0].starts_with("tier_a,tier_b,angle_a_deg,angle_b_deg,current,valid,"));
    assert!(lines[0].ends_with(",facet_warnings,notes"));
    assert!(!lines[0].contains("tilt_"));
    assert!(lines[1].starts_with("Crown Main,Pavilion Main,34.0000,40.0000,"));
    assert_eq!(lines[outcome.cells.len() + 1], "");
}

#[test]
fn the_best_cells_are_flagged_over_bare_figures_like_the_rows_are() {
    let design = rbc();
    let plan = plan_of(&design, range(34.0, 35.0, 0.5), range(40.0, 42.0, 1.0));
    let outcome = run(&design, &plan, 2);
    let rows: Vec<SweepRow> = outcome
        .cells
        .iter()
        .map(|cell| SweepRow {
            angle_deg: cell.angle_a,
            is_current: cell.is_current,
            metrics: cell.metrics,
            notes: Vec::new(),
        })
        .collect();
    assert_eq!(best_flags_of(&outcome.metrics()), best_flags(&rows));
    assert!(
        best_flags_of(&[None, None])
            .iter()
            .all(|f| *f == BestFlags::default())
    );
}

#[test]
fn the_estimate_is_the_one_tier_estimate_per_cell() {
    let cost = SolveCostEstimate {
        planes: 100,
        meet_derived_tiers: 0,
        has_tier_targets: false,
    };
    assert_eq!(
        estimate_seconds_2d(100, false, 4, cost).to_bits(),
        estimate_seconds(100, false, 4, cost).to_bits()
    );
}
