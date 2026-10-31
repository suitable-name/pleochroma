//! Walks of the lessons in `viewing/color_variants.rs`.

use super::{Sim, act, event, guide_named, read, viewing_events as events, walk};
use crate::guide::{
    EVENTS, GuideCategory, HIGHLIGHT_TARGETS, StartingState, WebClass, WebFeatures,
    is_offered_on_web, web_lesson,
};
use pleochroma::optics::materials::variants::built_in_variants;
use pleochroma_cut_core::{
    Edit, MaterialSelection,
    material::{
        CUSTOM_VARIANT_KEY, CUSTOM_VARIANT_LABEL, VariantChoice,
        pattern::{PatternAxis, PatternColor, TemplateId, template},
    },
};

/// The two lessons of this file.
const IDS: [&str; 2] = [
    "materials-color-variants",
    "materials-zone-colors-from-variants",
];

/// The key of a variant the cutter saved, as the Custom editor's Save as Variant... makes one.
const SAVED_KEY: &str = "user.00000000-0000-4000-8000-000000000001";

/// Sets the design's material to `name` with `edit` applied to its selection, as the Stone tab
/// does.
fn set_selection(
    sim: &mut Sim,
    name: &str,
    edit: impl FnOnce(MaterialSelection) -> MaterialSelection,
) {
    let mut material = MaterialSelection::none();
    material.name = Some(name.to_owned());
    sim.session
        .apply(Edit::SetMaterial {
            material: Box::new(edit(material)),
        })
        .expect("the material is set");
}

/// Sets the design's material to `name`, with its own color.
fn set_material(sim: &mut Sim, name: &str) {
    set_selection(sim, name, |material| material);
}

/// The built-in Sapphire variant `key`, as a design stores it.
fn built_in(key: &str) -> VariantChoice {
    built_in_variants("Sapphire")
        .iter()
        .find(|variant| variant.key == key)
        .map_or_else(
            || panic!("Sapphire has no built-in variant {key:?}"),
            VariantChoice::from_variant,
        )
}

/// The learner picks Yellow in the Body Color list of a Sapphire design.
fn pick_yellow(sim: &mut Sim) {
    let yellow = built_in("sapphire.yellow");
    set_selection(sim, "Sapphire", |material| {
        material.with_color_variant(Some(yellow))
    });
}

/// The learner applies a Custom Color: the editor's choice has the key `custom`.
fn apply_custom(sim: &mut Sim) {
    let custom = VariantChoice {
        key: CUSTOM_VARIANT_KEY.to_owned(),
        label: CUSTOM_VARIANT_LABEL.to_owned(),
        ..built_in("sapphire.pink")
    };
    set_selection(sim, "Sapphire", |material| {
        material.with_color_variant(Some(custom))
    });
}

/// The learner saves the Custom Color as a variant of their own: the design then holds the saved
/// variant under its new key and name.
fn save_variant(sim: &mut Sim) {
    let saved = VariantChoice {
        key: SAVED_KEY.to_owned(),
        label: "Sunset Sapphire".to_owned(),
        ..built_in("sapphire.pink")
    };
    set_selection(sim, "Sapphire", |material| {
        material.with_color_variant(Some(saved))
    });
}

/// The learner applies the Watermelon pattern with the core colored from a variant.
fn apply_variant_pattern(sim: &mut Sim) {
    let mut pattern = template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL);
    let pink = built_in_variants("Sapphire")
        .into_iter()
        .find(|variant| variant.key == "sapphire.pink")
        .expect("Sapphire has a Pink variant");
    pattern.zones[0].color = PatternColor::from_variant(&pink);
    let pattern = pattern.with_required_version();
    set_selection(sim, "Tourmaline", |material| {
        material.with_color_pattern(Some(pattern))
    });
    sim.fire(events::PATTERN_SAVED);
}

/// The learner applies the Watermelon pattern with picked colors only.
fn apply_plain_pattern(sim: &mut Sim) {
    let pattern = template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL);
    set_selection(sim, "Tourmaline", |material| {
        material.with_color_pattern(Some(pattern))
    });
    sim.fire(events::PATTERN_SAVED);
}

#[test]
fn the_color_variants_lesson_picks_makes_and_saves_a_variant() {
    walk(
        "materials-color-variants",
        vec![
            read(),
            act(|sim| set_material(sim, "Sapphire")),
            // The Body Color list of the Stone tab.
            act(pick_yellow),
            // The Path-Traced pill of the viewport toolbar.
            act(Sim::click_path),
            // The color editor's Apply Color.
            act(apply_custom),
            // Save as Variant... and Save in the name row.
            act(save_variant),
            // The Material Editor's Color Variants section.
            read(),
        ],
    );
}

#[test]
fn the_zone_lesson_opens_the_window_picks_a_template_and_applies_a_variant() {
    walk(
        "materials-zone-colors-from-variants",
        vec![
            read(),
            act(|sim| set_material(sim, "Tourmaline")),
            event(events::PATTERN_EDITOR_OPENED),
            event(events::PATTERN_TEMPLATE_PICKED),
            act(apply_variant_pattern),
            // The Path-Traced pill of the viewport toolbar.
            act(Sim::click_path),
            read(),
        ],
    );
}

#[test]
fn each_color_step_waits_for_its_own_kind_of_color() {
    let guide = guide_named("materials-color-variants");
    let (picked, custom, saved) = (&guide.steps[2], &guide.steps[4], &guide.steps[5]);
    let mut sim = Sim::start(&guide.starting_state);
    set_material(&mut sim, "Sapphire");
    assert!(
        !sim.met(&picked.goal) && !sim.met(&custom.goal) && !sim.met(&saved.goal),
        "a sapphire with its own color has no variant, no Custom Color and no saved variant"
    );
    pick_yellow(&mut sim);
    assert!(
        sim.met(&picked.goal),
        "a built-in variant is a picked variant"
    );
    assert!(!sim.met(&custom.goal) && !sim.met(&saved.goal));
    apply_custom(&mut sim);
    assert!(sim.met(&custom.goal));
    assert!(
        !sim.met(&picked.goal),
        "a Custom Color is not a variant picked from the list"
    );
    assert!(!sim.met(&saved.goal));
    save_variant(&mut sim);
    assert!(sim.met(&saved.goal));
    assert!(
        sim.met(&picked.goal),
        "a saved variant is picked from the list too"
    );
    assert!(!sim.met(&custom.goal));
    // The material's own color again: nothing is met.
    set_material(&mut sim, "Sapphire");
    assert!(!sim.met(&picked.goal) && !sim.met(&custom.goal) && !sim.met(&saved.goal));
}

#[test]
fn the_zone_step_needs_the_apply_event_and_a_variant_in_the_pattern() {
    let guide = guide_named("materials-zone-colors-from-variants");
    let step = &guide.steps[4];
    // Picked colors only, however many zones and however often applied.
    let mut plain = Sim::start(&guide.starting_state);
    set_material(&mut plain, "Tourmaline");
    apply_plain_pattern(&mut plain);
    assert!(
        !plain.met(&step.goal),
        "a watermelon of picked colors is the older lesson, not this one"
    );
    // A variant in the pattern, but nothing reported yet: the window may still be unsaved.
    let mut unreported = Sim::start(&guide.starting_state);
    set_material(&mut unreported, "Tourmaline");
    apply_variant_pattern(&mut unreported);
    unreported.events.clear();
    assert!(!unreported.met(&step.goal));
    // Both together.
    let mut done = Sim::start(&guide.starting_state);
    set_material(&mut done, "Tourmaline");
    apply_variant_pattern(&mut done);
    assert!(done.met(&step.goal));
    // The Save event alone is not the template step before it.
    let template_step = &guide.steps[3];
    assert!(!done.met(&template_step.goal));
}

#[test]
fn both_lessons_sit_in_materials_start_from_the_rich_design_and_are_desktop_only() {
    for id in IDS {
        let guide = guide_named(id);
        assert_eq!(guide.category, GuideCategory::Materials, "{id}");
        assert_eq!(guide.starting_state, StartingState::Template(5), "{id}");
        assert_eq!(
            web_lesson(id).map(|lesson| lesson.class),
            Some(WebClass::C),
            "{id} is classified desktop-only"
        );
        assert!(
            !is_offered_on_web(id, WebFeatures::ALL),
            "{id} must not be offered on the web"
        );
    }
}

#[test]
fn the_color_variant_lessons_use_only_known_targets_and_events() {
    for id in IDS {
        let guide = guide_named(id);
        for step in &guide.steps {
            assert!(
                HIGHLIGHT_TARGETS.contains(&step.highlight_target.as_str()),
                "{id}, {:?}: {:?} is not a known highlight target",
                step.title,
                step.highlight_target
            );
            for name in step.goal.events() {
                assert!(
                    EVENTS.contains(&name),
                    "{id}, {:?}: {name} is not a known event",
                    step.title
                );
            }
        }
    }
}

#[test]
fn the_color_variant_lessons_name_the_controls_the_window_has() {
    let color_variants = guide_named("materials-color-variants");
    let text = |index: usize| {
        let step = &color_variants.steps[index];
        format!("{} {}", step.intro, step.actions.join(" "))
    };
    // The Body Color list of a pleochroic stone: no generic colors, Custom... last.
    assert!(text(2).contains("Material Default") && text(2).contains("Custom..."));
    // The per-direction rows, the swatches under the sliders and the two buttons of the editor.
    for needle in ["Along the Axis", "Across the Axis", "Apply Color"] {
        assert!(text(4).contains(needle), "step 5 does not say {needle:?}");
    }
    for needle in ["Choose Color...", "Save as Variant...", "Save"] {
        assert!(text(5).contains(needle), "step 6 does not say {needle:?}");
    }
    // The Color Variants section of the Material Editor and its buttons.
    for needle in [
        "New/Edit Material...",
        "Color Variants",
        "Estimated",
        "Duplicate",
        "Rename",
        "Delete",
    ] {
        assert!(text(6).contains(needle), "step 7 does not say {needle:?}");
    }
    let zones = guide_named("materials-zone-colors-from-variants");
    let all: String = zones
        .steps
        .iter()
        .flat_map(|step| step.actions.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    for needle in [
        "Color Pattern...",
        "Watermelon",
        "Source",
        "Variant",
        "Rubellite",
        "Verdelite",
        "Apply",
        "Done",
    ] {
        assert!(
            all.contains(needle),
            "the zone lesson does not say {needle:?}"
        );
    }
}

#[test]
fn the_lessons_say_colors_menu_and_not_the_old_two_colors_switch() {
    for id in IDS {
        let guide = guide_named(id);
        for step in &guide.steps {
            let text = format!(
                "{} {} {} {} {}",
                step.title,
                step.intro,
                step.actions.join(" "),
                step.check,
                step.why
            );
            assert!(
                !text.contains("Two Colors"),
                "{id}, {:?}: the Two Colors switch is now the Colors menu",
                step.title
            );
        }
    }
    let zones = guide_named("materials-zone-colors-from-variants");
    let last = zones.steps.last().expect("a last step");
    assert!(
        last.actions
            .join(" ")
            .contains("Colors says how many colors")
    );
}

#[test]
fn no_color_variant_lesson_mentions_what_the_manual_keeps_out_of_the_interface() {
    for id in IDS {
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
