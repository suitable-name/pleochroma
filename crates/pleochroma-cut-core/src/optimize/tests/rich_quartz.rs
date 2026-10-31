//! The Rich Teaching Design in Quartz: the stone the Optimize lesson starts from, and the
//! questions that say where a run that finds nothing goes wrong.
//!
//! Its pavilion mains sit at 40 degrees, below Quartz's critical angle (about 40.4 degrees), so
//! the rays that reach them straight down leave through the pavilion (windowing). Turned about
//! the girdle edge to 41 degrees the same rays are totally reflected twice and go back up. So:
//!
//! 1. the quick (table-up) score must reward the steeper pavilion,
//! 2. the full (tilt sweep) score, which gates every candidate, must reward it too,
//! 3. the two disagree about the quick optimum, which is why the gate needs fallbacks, and
//! 4. a short Low Windowing search must therefore end with a candidate.
//!
//! When the last fails, the first three say whether the objective, the gate or the search is
//! at fault. Every figure the first three read is printed (`cargo test -p pleochroma-cut-core
//! rich_quartz -- --nocapture`).

use super::{
    super::{
        FinishedScore, ObjectiveFidelity, ObjectivePreset, OptimizeConfig, OptimizeOptions,
        ScoringSetup, SearchHooks, objective::CANONICAL_LIGHTING_PRESET, optimize_design_with,
        score_finished_design,
    },
    anchored::space_for,
};
use crate::{
    design::{Design, tier_hinge_points},
    templates::TEMPLATES,
};
use pleochroma::optics::materials::GemMaterial;

/// The pavilion main of the Rich Teaching Design, first in its cutting order.
const PAVILION: usize = 0;

/// The crown main of the Rich Teaching Design, the other tier the lesson's run may turn.
const CROWN: usize = 2;

/// The Rich Teaching Design (template 5), in its own material, Quartz.
fn rich_in_quartz() -> Design {
    let spec = &TEMPLATES[4];
    let mut design = Design::fresh_from_spec(spec.fresh_spec(spec.default_material_selection()));
    design.tiers = spec.tiers();
    design.ensure_tier_ids();
    assert_eq!(design.tiers[PAVILION].name, "Pavilion Main");
    assert_eq!(design.tiers[CROWN].name, "Crown Main");
    design
}

fn quartz() -> GemMaterial {
    GemMaterial::by_name("Quartz").expect("Quartz resolves")
}

/// The options of the lesson's run: anchored tiers varied about their girdle edges, every
/// movable tier within five degrees of where it is, two candidates half a degree apart, and the
/// girdle kept to half its thickness when `keep_girdle`.
fn lesson_options(design: &Design, keep_girdle: bool) -> OptimizeOptions {
    let solved = design.solve().expect("the lesson's stone solves");
    let mut options = OptimizeOptions {
        vary_anchored: true,
        anchor_hinges: tier_hinge_points(design, &solved),
        keep_candidates: 2,
        candidate_separation_deg: Some(0.5),
        min_girdle_fraction: keep_girdle.then_some(0.5),
        ..OptimizeOptions::default()
    };
    for index in super::super::free_tier_indices_with(design, &options) {
        let angle = design.tiers[index].angle_deg;
        options
            .angle_bounds
            .insert(index, (angle - 5.0, angle + 5.0));
    }
    options
}

/// `design` with each listed tier turned to its angle about the girdle edge, exactly as the
/// search writes a proposal (the coupled projection a run adds on top moves the other pinned
/// depths a little, so a run's own state measures slightly differently).
fn turned_to(design: &Design, angles: &[(usize, f64)]) -> Design {
    let options = lesson_options(design, true);
    let (space, free) = space_for(design, &options);
    let mut turned = design.clone();
    for &(tier, angle_deg) in angles {
        assert!(free.contains(&tier), "tier {tier} is free to turn");
        assert!(
            space.write_angle(&mut turned, tier, angle_deg),
            "tier {tier} has a mast through its hinge at {angle_deg}"
        );
    }
    turned
}

/// `design` with its pavilion main turned to `angle_deg` about the girdle edge.
fn pavilion_turned_to(design: &Design, angle_deg: f64) -> Design {
    turned_to(design, &[(PAVILION, angle_deg)])
}

/// The Low Windowing score of `design` at `fidelity`, as the search measures it.
fn measured(design: &Design, fidelity: ObjectiveFidelity) -> FinishedScore {
    let solved = design.solve().expect("the stone solves");
    let planes = design.planes_from_solved(&solved);
    score_finished_design(
        design,
        &solved,
        &planes,
        &quartz(),
        ScoringSetup {
            weights: &ObjectivePreset::LowWindowing.weights(),
            fidelity,
            lighting: CANONICAL_LIGHTING_PRESET,
        },
        None,
    )
}

fn show(label: &str, angle_deg: f64, score: &FinishedScore) {
    eprintln!(
        "{label} pavilion {angle_deg:>6.2}: score {:>7.3}  windowing {:>6.2} %  extinction {:>6.2} %  brilliance {:>6.2} %",
        score.score,
        score.after.windowing_pct,
        score.after.extinction_pct,
        score.after.tilt_brilliance_pct
    );
}

/// 1. The quick score: the figure every trial stone of the search is compared on.
#[test]
fn the_quick_score_rewards_a_pavilion_turned_past_the_critical_angle() {
    let design = rich_in_quartz();
    let start = measured(&design, ObjectiveFidelity::Fast);
    show("fast", -40.0, &start);
    let mut best = start;
    for angle in [-41.0, -42.0, -43.0, -44.0] {
        let turned = measured(&pavilion_turned_to(&design, angle), ObjectiveFidelity::Fast);
        show("fast", angle, &turned);
        if turned.score < best.score {
            best = turned;
        }
    }
    assert!(
        best.score < start.score,
        "no pavilion between 41 and 44 degrees beats 40 on the quick score ({})",
        start.score
    );
    assert!(
        best.after.windowing_pct < start.after.windowing_pct,
        "windowing does not fall as the pavilion turns past the critical angle"
    );
}

/// 2. The full score: a candidate is only kept when this does not read worse than the start.
///
/// It is the mean over the whole tilt sweep (four axes, -90 to 90 degrees), which the extreme
/// tilts dominate: for Quartz it stays within a point of the start over the whole 37 to 43
/// degree range (windowing 50 to 53 % everywhere) while the table-up figure swings by 15. Just
/// past the critical angle, at 41 degrees, it does read better than the start; at 42 it is
/// already worse again (measured: 45.94 at 41, 46.09 at 40, 46.28 at 42), so the claim is for
/// 41, not for every steeper pavilion.
#[test]
fn the_full_score_rewards_a_pavilion_turned_just_past_the_critical_angle() {
    let design = rich_in_quartz();
    let start = measured(&design, ObjectiveFidelity::Full);
    show("full", -40.0, &start);
    let turned = measured(&pavilion_turned_to(&design, -41.0), ObjectiveFidelity::Full);
    show("full", -41.0, &turned);
    show(
        "full",
        -42.0,
        &measured(&pavilion_turned_to(&design, -42.0), ObjectiveFidelity::Full),
    );
    assert!(
        turned.score <= start.score,
        "the full score of a 41 degree pavilion ({}) is worse than the 40 degree start ({})",
        turned.score,
        start.score
    );
}

/// 3. The two scores disagree about the quick optimum: the state the lesson's search descends
///    to (pavilion about 42.5 degrees, the crown main lowered nearly to the edge of its range) is far
///    better than the start table-up and worse than the start over the tilt sweep. A pool that
///    held only the states the search likes best would give the full-fidelity gate nothing it
///    can pass, which is what the gate's reserve of fallbacks is for.
#[test]
fn the_quick_and_full_scores_disagree_on_the_quick_optimum() {
    let design = rich_in_quartz();
    let crown = design.tiers[CROWN].angle_deg;
    let optimum = turned_to(&design, &[(PAVILION, -42.5), (CROWN, crown - 4.9)]);
    let fast = (
        measured(&design, ObjectiveFidelity::Fast),
        measured(&optimum, ObjectiveFidelity::Fast),
    );
    let full = (
        measured(&design, ObjectiveFidelity::Full),
        measured(&optimum, ObjectiveFidelity::Full),
    );
    show("fast start   ", -40.0, &fast.0);
    show("fast optimum ", -42.5, &fast.1);
    show("full start   ", -40.0, &full.0);
    show("full optimum ", -42.5, &full.1);
    assert!(
        fast.1.score < fast.0.score - 10.0,
        "the quick score must prefer the lowered crown by a wide margin"
    );
    assert!(
        full.1.score > full.0.score,
        "the full score must read the quick optimum as worse than the start"
    );
}

/// 4. The search.
fn lesson_run(keep_girdle: bool) -> usize {
    let design = rich_in_quartz();
    let options = lesson_options(&design, keep_girdle);
    let config = OptimizeConfig {
        weights: ObjectivePreset::LowWindowing.weights(),
        seed: 0,
        max_evaluations: 100,
        starts: 8,
        lighting: CANONICAL_LIGHTING_PRESET,
        ..OptimizeConfig::default()
    };
    let result = optimize_design_with(
        &design,
        &quartz(),
        &config,
        &options,
        &SearchHooks::default(),
    )
    .expect("the lesson's stone optimizes");
    eprintln!(
        "search (girdle kept: {keep_girdle}): {} evaluation(s), {} candidate(s); start score {:.3}",
        result.outcome.evaluations,
        result.candidates.len(),
        result.outcome.before_score
    );
    if let Some(p) = &result.outcome.provenance {
        eprintln!("diagnostics: {:?}", p.diagnostics);
    }
    for candidate in &result.candidates {
        eprintln!(
            "candidate: score {:.3}, changes {:?}",
            candidate.score, candidate.changes
        );
        assert!(
            candidate.score <= result.outcome.before_score,
            "a candidate never reads worse than the start at full fidelity"
        );
    }
    assert!(
        result.candidates.len() <= options.keep_candidates,
        "never more candidates than asked for"
    );
    result.candidates.len()
}

#[test]
fn the_lessons_short_search_finds_a_candidate() {
    assert_ne!(lesson_run(true), 0, "no candidate with the girdle kept");
}

/// Without the girdle guard: when only this one finds a candidate the guard refuses the
/// pavilion turn.
#[test]
fn the_lessons_short_search_finds_a_candidate_without_the_girdle_guard() {
    assert_ne!(lesson_run(false), 0, "no candidate without the girdle kept");
}
