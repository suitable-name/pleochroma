//! Walks of the lessons in `viewing/surface_finish.rs` and `viewing/render_effects.rs`.

use super::{Sim, act, event, guide_named, read, viewing_events as events, walk};
use crate::guide::{Group, GuideCategory, StartingState};
use pleochroma::optics::inclusions::bragg::{StructurePattern, StructureSpec};
use pleochroma_cut_core::{
    Edit, InclusionEffects, MaterialSelection, StoneRenderEffects, StructureEffects, SurfaceFinish,
    SurfaceGrade, TierSurface,
};

/// The six lessons.
const LESSONS: [&str; 6] = [
    "materials-polish-quality",
    "materials-scratch-direction",
    "materials-silk-sapphire",
    "materials-opal-play-of-color",
    "materials-labradorite-schiller",
    "materials-axis-curves",
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

/// Gives the tier called `tier` the finish `finish`, as the inspector's Surface Finish list does.
fn set_finish(sim: &mut Sim, tier: &str, finish: SurfaceFinish) {
    let index = sim.row(tier);
    let surface = TierSurface::of_tier(finish).normalised();
    sim.session
        .apply(Edit::SetTierSurface { index, surface })
        .expect("the finish is set");
}

/// Gives the tier called `tier` the grade `grade`.
fn set_grade(sim: &mut Sim, tier: &str, grade: SurfaceGrade) {
    set_finish(sim, tier, SurfaceFinish::of_grade(grade));
}

/// Writes the Stone tab's debounced commit: the design's effects after `change`.
fn with_effects(sim: &mut Sim, change: impl FnOnce(&mut StoneRenderEffects)) {
    let mut effects = sim.session.design.render_effects;
    change(&mut effects);
    sim.session
        .apply(Edit::SetRenderEffects { effects })
        .expect("the effects are written");
}

/// The play-of-color the design's material starts from.
fn default_structure(sim: &Sim) -> StructureEffects {
    let name = sim
        .session
        .design
        .material
        .name
        .clone()
        .expect("the lesson has set a material");
    StructureEffects::from_spec(
        &StructureSpec::default_for(&name).expect("the material has a play-of-color"),
    )
}

/// Switches play-of-color on, as the Stone tab's switch does.
fn structure_on(sim: &mut Sim) {
    let structure = default_structure(sim);
    with_effects(sim, |effects| effects.structure = Some(structure));
}

/// Writes the inclusion preset `name`.
fn inclusions(sim: &mut Sim, name: &str) {
    let chosen = InclusionEffects::of_preset(name).expect("the catalogue has the preset");
    with_effects(sim, |effects| effects.inclusions = Some(chosen));
}

#[test]
fn the_polish_lesson_grades_the_crown_adds_haze_and_goes_back_to_ideal() {
    let sim = walk(
        "materials-polish-quality",
        vec![
            read(),
            act(Sim::click_path),
            act(|sim| set_grade(sim, "Crown Main", SurfaceGrade::Commercial)),
            act(|sim| set_grade(sim, "Crown Main", SurfaceGrade::Prepolish)),
            event(events::STONE_HAZE_CHANGED),
            act(|sim| set_grade(sim, "Crown Main", SurfaceGrade::Ideal)),
        ],
    );
    assert!(
        !sim.session.design.has_surface(),
        "an Ideal tier states nothing, so the design holds no finish"
    );
}

#[test]
fn the_scratch_lesson_polishes_the_pavilion_and_fixes_one_direction() {
    let sim = walk(
        "materials-scratch-direction",
        vec![
            read(),
            act(Sim::click_path),
            act(|sim| set_grade(sim, "Pavilion Main", SurfaceGrade::Prepolish)),
            // The Surface Finish pill and the Faceting Machine leave nothing in the design.
            read(),
            read(),
            act(|sim| {
                set_finish(
                    sim,
                    "Pavilion Main",
                    SurfaceFinish {
                        lap_deg: Some(30.0),
                        ..SurfaceFinish::of_grade(SurfaceGrade::Prepolish)
                    },
                );
            }),
            read(),
        ],
    );
    let at = sim.row("Pavilion Main");
    assert_eq!(
        sim.session
            .design
            .tier_surface(at)
            .and_then(|surface| surface.finish.lap_deg),
        Some(30.0)
    );
}

#[test]
fn the_silk_lesson_sets_the_axis_a_preset_a_turn_and_the_cats_eye() {
    walk(
        "materials-silk-sapphire",
        vec![
            read(),
            act(|sim| set_material(sim, "Sapphire")),
            act(Sim::click_path),
            event(events::CRYSTAL_AXIS_SET_FOR_STONE),
            act(|sim| inclusions(sim, "Star Silk")),
            act(|sim| {
                with_effects(sim, |effects| {
                    if let Some(chosen) = effects.inclusions.as_mut() {
                        chosen.turn_deg = Some(40.0);
                    }
                });
            }),
            act(|sim| {
                set_material(sim, "Chrysoberyl (Yellow)");
                inclusions(sim, "Cat's-Eye Silk");
            }),
            read(),
        ],
    );
}

#[test]
fn the_opal_lesson_switches_the_flashes_on_and_works_the_controls() {
    walk(
        "materials-opal-play-of-color",
        vec![
            read(),
            act(|sim| set_material(sim, "Precious Opal (White)")),
            act(Sim::click_path),
            act(structure_on),
            act(|sim| {
                with_effects(sim, |effects| {
                    if let Some(structure) = effects.structure.as_mut() {
                        structure.pattern = StructurePattern::Pinfire;
                    }
                });
            }),
            act(|sim| {
                with_effects(sim, |effects| {
                    if let Some(structure) = effects.structure.as_mut() {
                        structure.color_low_nm += 60.0;
                    }
                });
            }),
            act(|sim| {
                with_effects(sim, |effects| {
                    if let Some(structure) = effects.structure.as_mut() {
                        structure.seed = StructureEffects::reseeded(structure.seed);
                    }
                });
            }),
            act(|sim| set_material(sim, "Precious Opal (Black)")),
        ],
    );
}

#[test]
fn the_labradorite_lesson_switches_the_schiller_on_and_changes_the_coverage() {
    walk(
        "materials-labradorite-schiller",
        vec![
            read(),
            act(|sim| set_material(sim, "Labradorite")),
            act(Sim::click_path),
            act(structure_on),
            act(|sim| {
                with_effects(sim, |effects| {
                    if let Some(structure) = effects.structure.as_mut() {
                        // Away from whatever the material starts with.
                        structure.coverage = if structure.coverage > 0.5 { 0.2 } else { 0.9 };
                    }
                });
            }),
            read(),
        ],
    );
}

#[test]
fn the_curves_lesson_waits_for_a_saved_material() {
    walk(
        "materials-axis-curves",
        vec![read(), event(events::CUSTOM_MATERIAL_SAVED), read(), read()],
    );
}

#[test]
fn a_grade_of_another_tier_or_another_grade_does_not_finish_a_polish_step() {
    let guide = guide_named("materials-polish-quality");
    let mut sim = Sim::start(&guide.starting_state);
    // Steps 3 and 4 (indices 2 and 3) wait for the crown, Commercial and then Prepolish.
    set_grade(&mut sim, "Pavilion Main", SurfaceGrade::Commercial);
    assert!(
        !sim.met(&guide.steps[2].goal),
        "the pavilion is not the crown"
    );
    set_grade(&mut sim, "Crown Main", SurfaceGrade::Prepolish);
    assert!(
        !sim.met(&guide.steps[2].goal),
        "Prepolish is not Commercial"
    );
    assert!(sim.met(&guide.steps[3].goal));
    set_grade(&mut sim, "Crown Main", SurfaceGrade::Commercial);
    assert!(sim.met(&guide.steps[2].goal));
    assert!(!sim.met(&guide.steps[3].goal));
}

#[test]
fn only_a_custom_angle_finishes_the_angle_step() {
    let guide = guide_named("materials-scratch-direction");
    let mut sim = Sim::start(&guide.starting_state);
    set_grade(&mut sim, "Pavilion Main", SurfaceGrade::Prepolish);
    assert!(sim.met(&guide.steps[2].goal));
    assert!(!sim.met(&guide.steps[5].goal), "a grade alone has no angle");
    set_finish(
        &mut sim,
        "Pavilion Main",
        SurfaceFinish {
            lap_deg: Some(10.0),
            ..SurfaceFinish::of_grade(SurfaceGrade::Prepolish)
        },
    );
    assert!(sim.met(&guide.steps[5].goal));
}

#[test]
fn a_silk_turn_needs_the_preset_and_the_cats_eye_needs_its_own_preset() {
    let guide = guide_named("materials-silk-sapphire");
    let mut sim = Sim::start(&guide.starting_state);
    inclusions(&mut sim, "Star Silk");
    assert!(sim.met(&guide.steps[4].goal));
    assert!(!sim.met(&guide.steps[5].goal), "no turn is set yet");
    assert!(
        !sim.met(&guide.steps[6].goal),
        "Star Silk is not the Cat's-Eye"
    );
    with_effects(&mut sim, |effects| {
        if let Some(chosen) = effects.inclusions.as_mut() {
            chosen.turn_deg = Some(90.0);
        }
    });
    assert!(sim.met(&guide.steps[5].goal));
    set_material(&mut sim, "Chrysoberyl (Yellow)");
    inclusions(&mut sim, "Cat's-Eye Silk");
    assert!(sim.met(&guide.steps[6].goal));
}

#[test]
fn the_play_of_color_steps_tell_the_controls_apart() {
    let guide = guide_named("materials-opal-play-of-color");
    let mut sim = Sim::start(&guide.starting_state);
    set_material(&mut sim, "Precious Opal (White)");
    // Nothing is on yet.
    for step in [3, 4, 5, 6] {
        assert!(!sim.met(&guide.steps[step].goal), "step {}", step + 1);
    }
    structure_on(&mut sim);
    assert!(sim.met(&guide.steps[3].goal));
    // The material's own pattern, range and seed finish none of the others.
    for step in [4, 5, 6] {
        assert!(!sim.met(&guide.steps[step].goal), "step {}", step + 1);
    }
}

#[test]
fn the_lessons_sit_in_the_materials_section_and_start_from_the_rich_design() {
    for id in LESSONS {
        let guide = guide_named(id);
        assert_eq!(guide.category, GuideCategory::Materials, "{id}");
        assert!(
            matches!(guide.starting_state, StartingState::Template(5)),
            "{id} starts from the Rich Teaching Design"
        );
    }
}

#[test]
fn the_steps_that_need_an_advanced_control_unlock_the_advanced_group() {
    // The inspector's Surface Finish section and the Preferences dialog are Advanced controls.
    let guide = guide_named("materials-polish-quality");
    for step in [2, 3, 5] {
        assert!(guide.steps[step].allow.contains(&Group::Advanced), "{step}");
        assert!(guide.steps[step].allow.contains(&Group::TierForm), "{step}");
    }
    let guide = guide_named("materials-scratch-direction");
    for step in [2, 4, 5] {
        assert!(guide.steps[step].allow.contains(&Group::Advanced), "{step}");
    }
}

#[test]
fn no_render_effects_lesson_mentions_what_the_manual_keeps_out_of_the_interface() {
    for id in LESSONS {
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
            for word in [
                "colour",
                "chromophore",
                "fluorescen",
                "uv lamp",
                " ai ",
                "physics-color",
            ] {
                assert!(
                    !text.contains(word),
                    "{id}, {:?}: the text contains {word:?}",
                    step.title
                );
            }
        }
    }
}
