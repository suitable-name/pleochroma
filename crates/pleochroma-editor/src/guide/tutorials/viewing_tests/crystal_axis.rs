//! Walks of the lessons in `viewing/crystal_axis.rs` and `viewing/crystal_axis_rough.rs`.

use super::{Sim, act, event, guide_named, read, viewing_events as events, walk};
use crate::guide::{GuideCategory, StartingState};
use pleochroma_cut_core::{Edit, MaterialSelection, StoneRenderEffects};

/// The five lessons, with the section each sits in.
const LESSONS: [(&str, GuideCategory); 5] = [
    ("materials-orient-sapphire", GuideCategory::Materials),
    (
        "materials-tourmaline-axis-in-table",
        GuideCategory::Materials,
    ),
    ("materials-best-blue-tanzanite", GuideCategory::Materials),
    ("materials-set-crystal-axis", GuideCategory::Materials),
    (
        "planning-override-orientation",
        GuideCategory::RoughPlanning,
    ),
];

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

/// The effects the Stone tab's debounced commit writes after a turn of the axis.
fn turned() -> StoneRenderEffects {
    StoneRenderEffects {
        c_axis_override_enabled: true,
        c_axis_tilt_deg: 40.0,
        c_axis_azimuth_deg: 120.0,
        ..StoneRenderEffects::default()
    }
}

#[test]
fn the_sapphire_lesson_shows_the_axis_sets_it_and_picks_a_preset() {
    walk(
        "materials-orient-sapphire",
        vec![
            read(),
            act(|sim| set_material(sim, "Sapphire")),
            // The Path-Traced pill of the viewport toolbar.
            act(Sim::click_path),
            event(events::CRYSTAL_AXIS_TOGGLED),
            event(events::CRYSTAL_AXIS_SET_FOR_STONE),
            event(events::CRYSTAL_AXIS_PRESET_CHOSEN),
            read(),
        ],
    );
}

#[test]
fn the_tourmaline_lesson_tries_the_two_flat_presets() {
    walk(
        "materials-tourmaline-axis-in-table",
        vec![
            read(),
            act(|sim| set_material(sim, "Tourmaline")),
            act(Sim::click_path),
            event(events::CRYSTAL_AXIS_PRESET_CHOSEN),
            event(events::CRYSTAL_AXIS_PRESET_CHOSEN),
            read(),
        ],
    );
}

#[test]
fn the_tanzanite_lesson_sets_the_second_axis_and_two_color_presets() {
    walk(
        "materials-best-blue-tanzanite",
        vec![
            read(),
            act(|sim| set_material(sim, "Tanzanite")),
            act(Sim::click_path),
            event(events::CRYSTAL_SECOND_AXIS_CHANGED),
            event(events::CRYSTAL_AXIS_COLOR_PRESET_CHOSEN),
            event(events::CRYSTAL_AXIS_COLOR_PRESET_CHOSEN),
            read(),
        ],
    );
}

#[test]
fn the_handle_lesson_drags_cancels_undoes_and_returns_to_the_material() {
    let sim = walk(
        "materials-set-crystal-axis",
        vec![
            read(),
            act(|sim| set_material(sim, "Sapphire")),
            event(events::CRYSTAL_AXIS_TOGGLED),
            // The drag reports its event when it is released, and the debounced commit that
            // follows writes the axis into the design.
            act(|sim| {
                sim.fire(events::CRYSTAL_AXIS_DRAGGED);
                sim.session
                    .apply(Edit::SetRenderEffects { effects: turned() })
                    .expect("the turned axis is written");
            }),
            event(events::CRYSTAL_AXIS_DRAG_CANCELLED),
            act(Sim::undo),
            event(events::CRYSTAL_AXIS_AS_MATERIAL),
        ],
    );
    assert!(
        !sim.session.design.render_effects.c_axis_override_enabled,
        "the lesson ends on the material's own axis"
    );
}

#[test]
fn the_planning_lesson_sets_the_axis_plans_overrides_locks_and_opens_the_stone() {
    walk(
        "planning-override-orientation",
        vec![
            read(),
            event(events::ROUGH_AXIS_SET),
            event(events::ROUGH_AXIS_SET),
            read(),
            event(events::ROUGH_PLAN_FINISHED),
            event(events::ROUGH_STONE_OVERRIDDEN),
            act(|sim| {
                sim.fire(events::ROUGH_STONE_LOCKED);
                sim.fire(events::ROUGH_PLAN_FINISHED);
            }),
            event(events::ROUGH_STONE_OPENED),
        ],
    );
}

#[test]
fn each_crystal_axis_step_waits_for_its_own_event() {
    // (lesson, step, the event the step waits for)
    let own = [
        ("materials-orient-sapphire", 3, events::CRYSTAL_AXIS_TOGGLED),
        (
            "materials-orient-sapphire",
            4,
            events::CRYSTAL_AXIS_SET_FOR_STONE,
        ),
        (
            "materials-orient-sapphire",
            5,
            events::CRYSTAL_AXIS_PRESET_CHOSEN,
        ),
        (
            "materials-best-blue-tanzanite",
            3,
            events::CRYSTAL_SECOND_AXIS_CHANGED,
        ),
        (
            "materials-best-blue-tanzanite",
            4,
            events::CRYSTAL_AXIS_COLOR_PRESET_CHOSEN,
        ),
        (
            "materials-set-crystal-axis",
            3,
            events::CRYSTAL_AXIS_DRAGGED,
        ),
        (
            "materials-set-crystal-axis",
            4,
            events::CRYSTAL_AXIS_DRAG_CANCELLED,
        ),
        (
            "materials-set-crystal-axis",
            6,
            events::CRYSTAL_AXIS_AS_MATERIAL,
        ),
        ("planning-override-orientation", 1, events::ROUGH_AXIS_SET),
        (
            "planning-override-orientation",
            5,
            events::ROUGH_STONE_OVERRIDDEN,
        ),
    ];
    for (id, step, name) in own {
        let guide = guide_named(id);
        // Every event of the crystal axis group except this step's own.
        for other in events::ALL.iter().filter(|other| {
            (other.starts_with("crystal_") || other.starts_with("rough_")) && **other != name
        }) {
            let mut sim = Sim::start(&guide.starting_state);
            sim.fire(other);
            assert!(
                !sim.met(&guide.steps[step].goal),
                "{id}: {:?} must not finish on {other}",
                guide.steps[step].title
            );
        }
        let mut sim = Sim::start(&guide.starting_state);
        sim.fire(name);
        assert!(
            sim.met(&guide.steps[step].goal),
            "{id}: {:?} must finish on {name}",
            guide.steps[step].title
        );
    }
}

#[test]
fn the_look_steps_need_path_traced_and_not_solid() {
    for id in [
        "materials-orient-sapphire",
        "materials-tourmaline-axis-in-table",
        "materials-best-blue-tanzanite",
    ] {
        let guide = guide_named(id);
        let mut sim = Sim::start(&guide.starting_state);
        sim.show(0);
        assert!(!sim.met(&guide.steps[2].goal), "{id}");
        sim.click_path();
        assert!(sim.met(&guide.steps[2].goal), "{id}");
    }
}

#[test]
fn another_material_does_not_finish_a_material_step() {
    for (id, material, other) in [
        ("materials-orient-sapphire", "Sapphire", "Tanzanite"),
        (
            "materials-tourmaline-axis-in-table",
            "Tourmaline",
            "Sapphire",
        ),
        ("materials-best-blue-tanzanite", "Tanzanite", "Tourmaline"),
        ("materials-set-crystal-axis", "Sapphire", "Tanzanite"),
    ] {
        let guide = guide_named(id);
        let mut sim = Sim::start(&guide.starting_state);
        set_material(&mut sim, other);
        assert!(!sim.met(&guide.steps[1].goal), "{id} with {other}");
        set_material(&mut sim, material);
        assert!(sim.met(&guide.steps[1].goal), "{id} with {material}");
    }
}

#[test]
fn the_undo_step_waits_for_the_axis_to_leave_the_design() {
    let guide = guide_named("materials-set-crystal-axis");
    let mut sim = Sim::start(&guide.starting_state);
    // A stone that has the material's own axis already has nothing to undo.
    assert!(sim.met(&guide.steps[5].goal));
    sim.session
        .apply(Edit::SetRenderEffects { effects: turned() })
        .expect("the turned axis is written");
    assert!(!sim.met(&guide.steps[5].goal));
    sim.undo();
    assert!(sim.met(&guide.steps[5].goal));
}

#[test]
fn locking_alone_does_not_finish_the_lock_step() {
    let guide = guide_named("planning-override-orientation");
    let mut sim = Sim::start(&guide.starting_state);
    sim.fire(events::ROUGH_STONE_LOCKED);
    assert!(
        !sim.met(&guide.steps[6].goal),
        "a new plan is still to come"
    );
    sim.fire(events::ROUGH_PLAN_FINISHED);
    assert!(sim.met(&guide.steps[6].goal));
}

#[test]
fn the_axis_pill_is_outlined_and_the_lessons_sit_in_their_sections() {
    for (id, category) in LESSONS {
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
    for id in ["materials-orient-sapphire", "materials-set-crystal-axis"] {
        let guide = guide_named(id);
        assert!(
            guide
                .steps
                .iter()
                .any(|step| step.highlight_target == "crystal_axis_toggle"),
            "{id} outlines the Crystal Axis pill"
        );
    }
}

#[test]
fn no_crystal_axis_lesson_mentions_what_the_manual_keeps_out_of_the_interface() {
    for (id, _) in LESSONS {
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
