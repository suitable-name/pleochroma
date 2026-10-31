//! Walks of the lesson in `viewing/stone_effects.rs`.

use super::{Sim, act, event, guide_named, read, viewing_events as events, walk};
use pleochroma_cut_core::{Edit, StoneRenderEffects};

/// The effects the lesson sets, as the Stone tab's commit writes them into the design.
fn effects(haze: f64, frosted: bool, rounding: f64) -> StoneRenderEffects {
    StoneRenderEffects {
        inclusion_sigma_s: haze,
        girdle_frosted: frosted,
        edge_rounding_radius: rounding,
        ..StoneRenderEffects::default()
    }
}

#[test]
fn the_four_effects_are_set_one_after_the_other_and_looked_at_in_path_traced() {
    walk(
        "viewing-stone-effects",
        vec![
            read(),
            // The Path-Traced pill of the viewport toolbar.
            act(Sim::click_path),
            event(events::STONE_HAZE_CHANGED),
            event(events::STONE_GIRDLE_FROSTED_CHANGED),
            event(events::STONE_EDGE_ROUNDING_CHANGED),
            event(events::STONE_WIDTH_SET),
            read(),
        ],
    );
}

#[test]
fn each_effect_step_waits_for_its_own_event() {
    let guide = guide_named("viewing-stone-effects");
    let own = [
        (2, events::STONE_HAZE_CHANGED),
        (3, events::STONE_GIRDLE_FROSTED_CHANGED),
        (4, events::STONE_EDGE_ROUNDING_CHANGED),
        (5, events::STONE_WIDTH_SET),
    ];
    for (step, name) in own {
        for (_, other) in own.iter().filter(|(_, other)| *other != name) {
            let mut sim = Sim::start(&guide.starting_state);
            sim.fire(other);
            assert!(
                !sim.met(&guide.steps[step].goal),
                "{:?} must not finish on {other}",
                guide.steps[step].title
            );
        }
    }
}

#[test]
fn the_look_step_needs_path_traced_and_not_solid() {
    let guide = guide_named("viewing-stone-effects");
    let mut sim = Sim::start(&guide.starting_state);
    sim.show(0);
    assert!(!sim.met(&guide.steps[1].goal));
    sim.click_path();
    assert!(sim.met(&guide.steps[1].goal));
}

#[test]
fn every_effect_is_one_undo_step_and_the_design_keeps_them() {
    let guide = guide_named("viewing-stone-effects");
    let mut sim = Sim::start(&guide.starting_state);
    let width_before = sim.session.design.girdle_diameter_mm;
    let steps = [
        effects(0.5, false, 0.0),
        effects(0.5, true, 0.0),
        effects(0.5, true, 0.01),
    ];
    for effects in steps {
        sim.session
            .apply(Edit::SetRenderEffects { effects })
            .expect("the effects apply");
    }
    sim.session
        .apply(Edit::SetGirdleDiameterMm {
            girdle_diameter_mm: Some(8.0),
        })
        .expect("the width applies");
    assert_eq!(sim.session.design.girdle_diameter_mm, Some(8.0));
    sim.undo();
    assert_eq!(sim.session.design.girdle_diameter_mm, width_before);
    assert_eq!(sim.session.design.render_effects, steps[2]);
    sim.undo();
    assert_eq!(sim.session.design.render_effects, steps[1]);
    sim.undo();
    assert_eq!(sim.session.design.render_effects, steps[0]);
    sim.undo();
    assert!(sim.session.design.render_effects.is_default());
}
