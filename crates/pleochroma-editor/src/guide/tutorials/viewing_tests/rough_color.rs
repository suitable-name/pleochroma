//! Walks of the lessons in `viewing/rough_color.rs`.

use super::{Sim, act, event, guide_named, read, viewing_events as events, walk};
use crate::guide::{GuideCategory, StartingState};
use pleochroma_cut_core::{Edit, MaterialSelection};

/// Sets the design's material to `name`, as the Stone tab's Material list does.
fn set_material(sim: &mut Sim, name: &str) {
    let mut material = MaterialSelection::none();
    material.name = Some(name.to_owned());
    sim.session
        .apply(Edit::SetMaterial {
            material: Box::new(material),
        })
        .expect("the material is set");
}

#[test]
fn the_wizard_lesson_opens_the_window_and_reads_the_rest() {
    walk(
        "library-rough-color-wizard",
        vec![
            read(),
            event(events::ROUGH_COLOR_WIZARD_OPENED),
            read(),
            read(),
            read(),
            read(),
            read(),
        ],
    );
}

#[test]
fn the_planner_lesson_chooses_best_color_then_uses_a_color() {
    walk(
        "library-rough-color-in-planner",
        vec![
            read(),
            event(events::ROUGH_COLOR_BEST_CHOSEN),
            event(events::ROUGH_COLOR_USED),
            read(),
        ],
    );
}

#[test]
fn the_ametrine_lesson_picks_the_material_and_looks_at_it_path_traced() {
    walk(
        "viewing-ametrine",
        vec![
            read(),
            act(|sim| set_material(sim, "Ametrine")),
            // The Path-Traced pill of the viewport toolbar.
            act(Sim::click_path),
            read(),
        ],
    );
}

#[test]
fn another_material_is_not_ametrine_and_solid_is_not_path_traced() {
    let guide = guide_named("viewing-ametrine");
    let mut sim = Sim::start(&guide.starting_state);
    set_material(&mut sim, "Quartz");
    assert!(!sim.met(&guide.steps[1].goal));
    set_material(&mut sim, "Ametrine");
    assert!(sim.met(&guide.steps[1].goal));
    sim.show(0);
    assert!(!sim.met(&guide.steps[2].goal));
    sim.click_path();
    assert!(sim.met(&guide.steps[2].goal));
}

#[test]
fn opening_the_wizard_is_not_choosing_a_color() {
    let guide = guide_named("library-rough-color-in-planner");
    let mut sim = Sim::start(&guide.starting_state);
    sim.fire(events::ROUGH_COLOR_WIZARD_OPENED);
    assert!(!sim.met(&guide.steps[1].goal));
    assert!(!sim.met(&guide.steps[2].goal));
    sim.fire(events::ROUGH_COLOR_BEST_CHOSEN);
    assert!(sim.met(&guide.steps[1].goal));
    assert!(
        !sim.met(&guide.steps[2].goal),
        "Best Color is not Use Color"
    );
}

#[test]
fn the_color_lessons_sit_in_their_sections_and_start_from_the_open_design() {
    for (id, category) in [
        ("library-rough-color-wizard", GuideCategory::RoughPlanning),
        (
            "library-rough-color-in-planner",
            GuideCategory::RoughPlanning,
        ),
        ("viewing-ametrine", GuideCategory::Materials),
    ] {
        let guide = guide_named(id);
        assert_eq!(guide.category, category, "{id}");
        assert!(
            matches!(
                guide.starting_state,
                StartingState::CurrentDesign | StartingState::Template(5)
            ),
            "{id} works with the open design or the Rich Teaching Design"
        );
    }
}

#[test]
fn no_color_lesson_mentions_what_the_manual_keeps_out_of_the_interface() {
    for id in [
        "library-rough-color-wizard",
        "library-rough-color-in-planner",
        "viewing-ametrine",
    ] {
        let guide = guide_named(id);
        for step in &guide.steps {
            let text = format!(
                "{} {} {} {} {}",
                step.title,
                step.intro,
                step.actions.join(" "),
                step.check,
                step.why
            )
            .to_lowercase();
            for word in ["colour", "chromophore", "fluorescen", "uv lamp", " ai "] {
                assert!(
                    !text.contains(word),
                    "{id}, {:?}: the text contains {word:?}",
                    step.title
                );
            }
        }
    }
}
