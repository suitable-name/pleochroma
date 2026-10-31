//! The cancellable `Full` scoring is the ungated one when nothing cancels, and stops early
//! (with nothing scored) when something does.

use super::*;
use crate::{
    design::{ConstraintTier, Design, ScheduleMeta},
    optimize::scored_stone::ScoredStone,
    preform::PreformSpec,
};
use pleochroma::{color::metrics::total_evaluations, geometry::meet_solver::SolvedTier};
use std::cell::Cell;

/// The standard round brilliant in a rough deep enough that the preform never touches it.
fn brilliant() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

/// The brilliant's solved tiers and planes: the stone every test here scores (as diamond).
fn solved_planes() -> (Vec<SolvedTier>, Vec<(DVec3, f64)>) {
    let design = brilliant();
    let solved = design.solve().expect("the brilliant solves");
    let planes = design.planes_from_solved(&solved);
    (solved, planes)
}

fn bits(components: &ObjectiveComponents) -> [u32; 3] {
    [
        components.windowing_pct.to_bits(),
        components.extinction_pct.to_bits(),
        components.tilt_brilliance_pct.to_bits(),
    ]
}

#[test]
fn a_cancellable_full_scoring_that_never_cancels_is_the_ungated_scoring_bit_for_bit() {
    let design = brilliant();
    let (solved, planes) = solved_planes();
    let stone = ScoredStone::of(&design, &solved, &planes);
    let material = GemMaterial::diamond();

    let ungated = evaluate_objective_geom_under(
        stone.geometry(),
        &material,
        ObjectiveFidelity::Full,
        LightingPreset::Daylight,
    );
    let gated = evaluate_objective_finished_under_cancellable(
        stone.finished(),
        &material,
        ObjectiveFidelity::Full,
        LightingPreset::Daylight,
        None,
        &|| false,
    )
    .expect("nothing cancelled");
    assert_eq!(bits(&gated), bits(&ungated));
}

#[test]
fn the_cancellable_fast_scoring_ignores_the_flag_and_equals_the_plain_one() {
    let design = brilliant();
    let (solved, planes) = solved_planes();
    let stone = ScoredStone::of(&design, &solved, &planes);
    let material = GemMaterial::diamond();

    let plain = evaluate_objective_with_tone_geom_under_threaded(
        stone.geometry(),
        &material,
        ObjectiveFidelity::Fast,
        LightingPreset::Daylight,
        None,
    );
    let cancellable = evaluate_objective_with_tone_finished_under_cancellable(
        stone.finished(),
        &material,
        ObjectiveFidelity::Fast,
        LightingPreset::Daylight,
        None,
        &|| true,
    )
    .expect("Fast is one pose and is not cancellable");
    assert_eq!(bits(&cancellable.0), bits(&plain.0));
    assert_eq!(cancellable.1, plain.1);
}

#[test]
fn a_cancel_that_rises_after_the_first_look_stops_the_sweep_early() {
    let design = brilliant();
    let (solved, planes) = solved_planes();
    let stone = ScoredStone::of(&design, &solved, &planes);
    let material = GemMaterial::diamond();

    let looks = Cell::new(0_usize);
    let cancel_from_the_second_look = || {
        looks.set(looks.get() + 1);
        looks.get() > 1
    };
    let scored = evaluate_objective_finished_under_cancellable(
        stone.finished(),
        &material,
        ObjectiveFidelity::Full,
        LightingPreset::Daylight,
        None,
        &cancel_from_the_second_look,
    );
    assert!(scored.is_none(), "a stopped sweep scores nothing");
    assert!(
        looks.get() < total_evaluations(),
        "the sweep asked {} times of {} poses: it ran on after the stop",
        looks.get(),
        total_evaluations()
    );
}

#[test]
fn a_full_tone_scoring_that_is_already_cancelled_runs_no_sweep() {
    let design = brilliant();
    let (solved, planes) = solved_planes();
    let stone = ScoredStone::of(&design, &solved, &planes);
    let material = GemMaterial::diamond();

    let before = full_sweep_runs::count();
    let scored = evaluate_objective_with_tone_finished_under_cancellable(
        stone.finished(),
        &material,
        ObjectiveFidelity::Full,
        LightingPreset::Daylight,
        None,
        &|| true,
    );
    assert!(scored.is_none());
    assert_eq!(full_sweep_runs::count(), before);
}
