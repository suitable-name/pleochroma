use std::sync::{Arc, Mutex};

use pleochroma::optics::{
    absorption::AbsorptionBand,
    materials::{AbsorptionUnit, variants::VariantProvenance},
};
use pleochroma_cut_core::material::CUSTOM_VARIANT_KEY;
use pleochroma_editor::lch_color::{self, LchSolve};

use super::*;
use crate::bridge::render_thread::PlanesOwner;

const YELLOW: [f32; 3] = BODY_COLOR_PRESETS[5].absorption_rgb;

/// A pleochroic uniaxial tensor with different o and e bands, light enough that a 7 mm stone is
/// clearly not black.
fn pleochroic_tensor() -> AbsorptionTensor {
    AbsorptionTensor::uniaxial(
        vec![AbsorptionBand::energy(550.0, 1500.5, 0.08)],
        vec![AbsorptionBand::new(430.0, 40.0, 0.05)],
    )
}

/// A choice of the built-in kind (not a Custom Color) with a pleochroic tensor.
fn named_choice() -> VariantChoice {
    VariantChoice {
        key: "user.test".to_owned(),
        label: "Mine".to_owned(),
        absorption: pleochroic_tensor(),
        unit: AbsorptionUnit::PerMm,
        reference_path_mm: 7.0,
    }
}

/// The variant a cutter saved for Sapphire (what `saved_variants` of the port returns).
fn saved_variant() -> ColorVariant {
    ColorVariant {
        key: "user.test".to_owned(),
        label: "Mine".to_owned(),
        material: "Sapphire".to_owned(),
        absorption: pleochroic_tensor(),
        unit: AbsorptionUnit::PerMm,
        reference_path_mm: 7.0,
        provenance: VariantProvenance::User,
    }
}

/// A solve of one direction that never ran the solver: one energy band at `centre_nm`.
fn fake_solve(centre_nm: f32) -> LchSolve {
    LchSolve {
        lch: [50.0, 30.0, 265.0],
        path_mm: 5.0,
        delta_e: 0.0,
        reachable: true,
        bands: vec![[centre_nm, 1500.0, 0.2]],
        triple: [0.2, 0.4, 1.0],
        swatches: vec![[0.5, 0.5, 0.5]],
    }
}

fn sapphire() -> GemMaterial {
    GemMaterial::sapphire()
}

fn diamond() -> GemMaterial {
    GemMaterial::diamond()
}

/// The list of `host` with the saved variant of Sapphire and the `shown` color.
fn list_of(host: &GemMaterial, shown: &MaterialSelection) -> ColorList {
    color_list(Some(host), &[saved_variant()], shown)
}

fn input<'a>(
    target: ColorTarget,
    shown: &'a MaterialSelection,
    list: &'a ColorList,
) -> ControlInput<'a> {
    ControlInput {
        target,
        physics: false,
        zoned: false,
        shown,
        items: &list.items,
        host: Some(&list.host),
        stone_width_mm: 0.0,
    }
}

fn position_of(list: &ColorList, wanted: &ColorEntry) -> usize {
    list.items
        .iter()
        .position(|item| item.entry == *wanted)
        .expect("the entry is listed")
}

fn with_variant(choice: VariantChoice) -> MaterialSelection {
    MaterialSelection::none().with_color_variant(Some(choice))
}

fn with_triple(rgb: [f32; 3]) -> MaterialSelection {
    MaterialSelection::none().with_body_color(Some(rgb))
}

// --- Picks ---

#[test]
fn entries_map_to_the_picks_they_stand_for() {
    assert_eq!(
        pick_for_entry(&ColorEntry::MaterialDefault, &[]),
        Some(Pick::MaterialDefault)
    );
    for (index, preset) in BODY_COLOR_PRESETS.iter().enumerate() {
        assert_eq!(
            pick_for_entry(&ColorEntry::Preset(index), &[]),
            Some(Pick::Triple(preset.absorption_rgb)),
            "{}",
            preset.label
        );
    }
    assert_eq!(
        pick_for_entry(&ColorEntry::Preset(BODY_COLOR_PRESETS.len()), &[]),
        None
    );
    assert_eq!(pick_for_entry(&ColorEntry::Custom, &[]), None);

    let variants = vec![saved_variant()];
    assert_eq!(
        pick_for_entry(
            &ColorEntry::Variant {
                key: "user.test".to_owned()
            },
            &variants
        ),
        Some(Pick::Variant {
            material: "Sapphire".to_owned(),
            choice: VariantChoice::from_variant(&saved_variant()),
        })
    );
    assert_eq!(
        pick_for_entry(
            &ColorEntry::Variant {
                key: "user.gone".to_owned()
            },
            &variants
        ),
        None,
        "a key the list does not hold is not a pick"
    );
}

#[test]
fn a_pick_carries_exactly_the_change_and_the_view_color_it_stands_for() {
    assert_eq!(Pick::MaterialDefault.change(), ColorChange::Legacy(None));
    assert_eq!(Pick::MaterialDefault.view_color(), None);

    let triple = Pick::Triple(YELLOW);
    assert_eq!(triple.change(), ColorChange::Legacy(Some(YELLOW)));
    assert_eq!(triple.view_color(), Some(ViewColor::triple_only(YELLOW)));

    let choice = named_choice();
    let variant = Pick::Variant {
        material: "Sapphire".to_owned(),
        choice: choice.clone(),
    };
    assert_eq!(variant.change(), ColorChange::Variant(choice.clone()));
    assert_eq!(
        variant.view_color(),
        Some(ViewColor::from_variant("Sapphire", choice))
    );
}

#[test]
fn a_finished_view_color_is_the_pick_the_design_takes_when_it_drives() {
    let choice = named_choice();
    assert_eq!(
        pick_of_view_color(ViewColor::from_variant("Sapphire", choice.clone())),
        Pick::Variant {
            material: "Sapphire".to_owned(),
            choice
        }
    );
    assert_eq!(
        pick_of_view_color(ViewColor::triple_only(YELLOW)),
        Pick::Triple(YELLOW)
    );
}

// --- The list ---

#[test]
fn a_pleochroic_host_lists_its_variants_and_no_generic_presets() {
    let none = MaterialSelection::none();
    let list = list_of(&sapphire(), &none);
    let view = list_view(&list, 0.0);
    assert!(view.presets.is_empty(), "no one-color presets for Sapphire");
    assert_eq!(view.legacy, None);
    assert_eq!(view.rows[0].label, "Material Default");
    assert!(
        view.rows.iter().any(|row| row.label == "Mine"),
        "the saved variant is listed after the built-in ones"
    );
    assert!(
        view.rows.iter().all(|row| row.swatch.is_some()),
        "every row has a dot"
    );
    let custom = view.custom.expect("Custom is always listed");
    assert_eq!(custom.index, list.items.len() - 1, "Custom is last");
    assert_eq!(custom.swatch, None);
    for row in &view.rows {
        assert!(!row.tip.is_empty(), "{} has a tooltip", row.label);
    }
}

#[test]
fn an_isotropic_host_lists_the_nine_presets_between_the_variants_and_custom() {
    let list = list_of(&diamond(), &MaterialSelection::none());
    let view = list_view(&list, 0.0);
    assert_eq!(view.presets.len(), BODY_COLOR_PRESETS.len());
    assert_eq!(view.legacy, None);
    assert!(
        view.rows.iter().all(|row| row.label != "Mine"),
        "a Sapphire variant is not listed for Diamond"
    );
    for (row, preset) in view.presets.iter().zip(&BODY_COLOR_PRESETS) {
        assert_eq!(row.label, preset.label);
        assert_eq!(
            row.swatch,
            Some(display_rgb_for_stone(preset.absorption_rgb, 0.0))
        );
    }
    // The presets are contiguous, so the popup can number them from the first.
    for (n, row) in view.presets.iter().enumerate() {
        assert_eq!(row.index, view.presets[0].index + n);
    }
    assert!(view.custom.expect("Custom").index > view.presets[8].index);
}

#[test]
fn the_dots_of_the_presets_and_variants_follow_the_stone_size() {
    let list = list_of(&diamond(), &MaterialSelection::none());
    let small = list_view(&list, 5.0);
    let large = list_view(&list, 14.0);
    assert_ne!(
        small.presets[1].swatch, large.presets[1].swatch,
        "a preset dot darkens with the stone"
    );
    let list = list_of(&sapphire(), &MaterialSelection::none());
    let mine = |view: &ListView| {
        view.rows
            .iter()
            .find(|row| row.label == "Mine")
            .and_then(|row| row.swatch)
    };
    assert_ne!(
        mine(&list_view(&list, 5.0)),
        mine(&list_view(&list, 14.0)),
        "so does a variant dot"
    );
    // Review finding 9: the Material Default dot follows the Stone Size like the other rows.
    let default = |view: &ListView| view.rows[0].swatch;
    assert_eq!(view_label(&list_view(&list, 5.0)), "Material Default");
    assert_ne!(
        default(&list_view(&list, 5.0)),
        default(&list_view(&list, 14.0)),
        "so does the Material Default dot"
    );
    // At the 7 mm default (and with no size set) it is the swatch of the combo row.
    assert_eq!(
        default(&list_view(&list, 0.0)),
        default(&list_view(&list, 7.0))
    );
    assert_eq!(
        default(&list_view(&list, 7.0)),
        list.items[0].swatch_rgb(),
        "the 7 mm dot is the combo's swatch"
    );
}

/// The label of the first row of the popup (Material Default).
fn view_label(view: &ListView) -> &str {
    view.rows[0].label.as_str()
}

#[test]
fn an_old_single_color_on_a_pleochroic_host_gets_the_legacy_row_and_note() {
    let shown = with_triple(BODY_COLOR_PRESETS[1].absorption_rgb);
    let list = list_of(&sapphire(), &shown);
    let view = list_view(&list, 0.0);
    let legacy = view.legacy.expect("a legacy row");
    assert!(legacy.label.starts_with("Legacy:"), "{}", legacy.label);
    assert!(view.presets.is_empty());
    assert_eq!(legacy_note(&shown, Some(&sapphire())), LEGACY_NOTE);

    let control = control_view(&input(ColorTarget::View, &shown, &list));
    assert_eq!(control.choice, legacy.index);
    assert_eq!(control.summary, legacy.label);
    assert!(control.swatch.is_some(), "it keeps rendering as saved");

    // Neither an isotropic host, a variant, no color nor an unresolved host has the note.
    assert_eq!(legacy_note(&shown, Some(&diamond())), "");
    assert_eq!(
        legacy_note(&with_variant(named_choice()), Some(&sapphire())),
        ""
    );
    assert_eq!(
        legacy_note(&MaterialSelection::none(), Some(&sapphire())),
        ""
    );
    assert_eq!(legacy_note(&shown, None), "");
}

#[test]
fn a_view_with_no_resolvable_material_lists_the_one_color_choices() {
    let list = color_list(None, &[], &MaterialSelection::none());
    let view = list_view(&list, 0.0);
    assert_eq!(list.rank, TensorRank::Isotropic);
    assert_eq!(view.presets.len(), BODY_COLOR_PRESETS.len());
    assert!(view.custom.is_some());
}

// --- What the control shows ---

#[test]
fn the_control_shows_the_designs_variant_by_label_and_dot_never_material_default() {
    // The editor's port reports the design's selection; the control names its variant.
    let reported = with_variant(named_choice());
    let port = DesignColorPort {
        current: Box::new(move || Some(reported.clone())),
        apply: Box::new(|_| Ok(())),
        saved_variants: Box::new(|_| vec![saved_variant()]),
    };
    let design = (port.current)().expect("readable");
    let shown = shown_selection(
        ColorTarget::Design,
        false,
        &design,
        &MaterialSelection::none(),
    );
    let list = color_list(
        Some(&sapphire()),
        &(port.saved_variants)("Sapphire"),
        &shown,
    );
    let view = control_view(&input(ColorTarget::Design, &shown, &list));
    assert_eq!(view.summary, "Mine");
    assert_ne!(view.summary, "Material Default");
    assert_eq!(
        view.choice,
        position_of(
            &list,
            &ColorEntry::Variant {
                key: "user.test".to_owned()
            }
        ),
        "the variant's own row is selected"
    );
    assert_eq!(
        view.swatch,
        Some(variant_swatch(&named_choice(), 0.0, &sapphire())),
        "the dot is the variant's face-up color"
    );
    assert!(view.targets_design && view.enabled && view.colors_enabled);
}

#[test]
fn a_custom_color_is_named_custom_color_and_selects_the_custom_row() {
    let choice = VariantChoice {
        key: CUSTOM_VARIANT_KEY.to_owned(),
        label: CUSTOM_VARIANT_LABEL.to_owned(),
        ..named_choice()
    };
    let shown = with_variant(choice.clone());
    let list = list_of(&sapphire(), &shown);
    let view = control_view(&input(ColorTarget::View, &shown, &list));
    assert_eq!(view.summary, "Custom Color");
    assert_eq!(view.choice, position_of(&list, &ColorEntry::Custom));
    assert_eq!(view.swatch, Some(variant_swatch(&choice, 0.0, &sapphire())));
    assert!(!view.targets_design);
}

#[test]
fn a_saved_variant_that_is_no_longer_listed_is_still_named_and_shown_on_the_custom_row() {
    let shown = with_variant(named_choice());
    let list = color_list(Some(&sapphire()), &[], &shown);
    let view = control_view(&input(ColorTarget::Design, &shown, &list));
    assert_eq!(
        view.summary, "Mine",
        "the design's snapshot still has its name"
    );
    assert_eq!(view.choice, position_of(&list, &ColorEntry::Custom));
}

#[test]
fn no_color_is_the_default_row_with_a_hollow_dot_and_an_unlisted_triple_is_custom_color() {
    let none = MaterialSelection::none();
    let list = list_of(&diamond(), &none);
    let view = control_view(&input(ColorTarget::View, &none, &list));
    assert_eq!((view.choice, view.swatch), (0, None));
    assert_eq!(view.summary, "Material Default");

    let odd = with_triple([0.3, 0.9, 1.4]);
    let list = list_of(&diamond(), &odd);
    let view = control_view(&input(ColorTarget::View, &odd, &list));
    assert_eq!(view.choice, position_of(&list, &ColorEntry::Custom));
    assert_eq!(view.summary, "Custom Color");
    assert!(view.swatch.is_some());
}

#[test]
fn a_preset_triple_on_an_isotropic_host_selects_its_preset_row() {
    let blue = with_triple(BODY_COLOR_PRESETS[1].absorption_rgb);
    let list = list_of(&diamond(), &blue);
    let view = control_view(&input(ColorTarget::Design, &blue, &list));
    assert_eq!(view.choice, position_of(&list, &ColorEntry::Preset(1)));
    assert_eq!(view.summary, "Blue");
}

#[test]
fn the_shown_color_is_the_designs_while_picks_go_to_the_design_and_the_views_otherwise() {
    let design = with_triple(BODY_COLOR_PRESETS[1].absorption_rgb);
    let view = with_triple(YELLOW);
    assert_eq!(
        shown_selection(ColorTarget::Design, false, &design, &view),
        design
    );
    assert_eq!(
        shown_selection(ColorTarget::View, false, &design, &view),
        view
    );
    // A view color never applies to a material with its own color (a physics recipe, zones).
    assert_eq!(
        shown_selection(ColorTarget::View, true, &design, &view),
        MaterialSelection::none()
    );
    // The design's own color still shows, greyed out, for such a material.
    assert_eq!(
        shown_selection(ColorTarget::Design, true, &design, &view),
        design
    );
}

#[test]
fn a_physics_material_greys_the_control_out_and_a_zoned_one_keeps_it_open() {
    let none = MaterialSelection::none();
    let list = list_of(&diamond(), &none);
    let mut state = input(ColorTarget::View, &none, &list);
    state.physics = true;
    let view = control_view(&state);
    assert!(!view.enabled && !view.colors_enabled && view.physics);

    state.physics = false;
    state.zoned = true;
    let view = control_view(&state);
    assert!(
        view.enabled,
        "the Color Zoning section is reached through it"
    );
    assert!(!view.colors_enabled);
    assert!(view.zoned && !view.physics);
}

// --- Dots ---

#[test]
fn every_per_millimetre_variant_dot_follows_the_stone_width_a_custom_color_included() {
    let host = sapphire();
    let named = named_choice();
    let sum = |rgb: [u8; 3]| rgb.iter().map(|c| u32::from(*c)).sum::<u32>();
    assert_eq!(
        variant_swatch(&named, 0.0, &host),
        variant_swatch(&named, 7.0, &host),
        "no size set is the 7 mm calibration"
    );
    assert!(
        sum(variant_swatch(&named, 14.0, &host)) < sum(variant_swatch(&named, 7.0, &host)),
        "a stone twice as wide is darker"
    );
    // Review finding 4: the dot does not change meaning when a Custom Color is saved under a
    // name (only the key and the label change, the tensor and the width stay).
    let custom = VariantChoice {
        key: CUSTOM_VARIANT_KEY.to_owned(),
        ..named.clone()
    };
    for width in [0.0_f32, 5.0, 7.0, 14.0] {
        assert_eq!(
            variant_swatch(&custom, width, &host),
            variant_swatch(&named, width, &host),
            "a Custom Color and the same tensor saved: one dot at {width} mm"
        );
    }
    assert!(
        sum(variant_swatch(&custom, 14.0, &host)) < sum(variant_swatch(&custom, 5.0, &host)),
        "a Custom Color darkens with the stone like any other variant"
    );
}

#[test]
fn a_per_model_unit_variant_dot_is_the_legacy_dot_at_every_size() {
    for preset in &BODY_COLOR_PRESETS {
        let choice = VariantChoice {
            key: "x.legacy".to_owned(),
            label: preset.label.to_owned(),
            absorption: AbsorptionTensor::isotropic(legacy_rgb_bands(preset.absorption_rgb)),
            unit: AbsorptionUnit::ModelUnit,
            reference_path_mm: 7.0,
        };
        for width in [0.0_f32, 5.0, 7.0, 10.87] {
            assert_eq!(
                variant_swatch(&choice, width, &diamond()),
                display_rgb_for_stone(preset.absorption_rgb, width),
                "{} at {width} mm",
                preset.label
            );
        }
    }
}

#[test]
fn the_swatch_colours_look_like_their_names() {
    let [cr, cg, cb] = display_rgb_for_stone(BODY_COLOR_PRESETS[0].absorption_rgb, 0.0);
    assert!(
        cr > 200 && cg > 200 && cb > 200,
        "Clear is near white: {cr} {cg} {cb}"
    );
    let [r, _, b] = display_rgb_for_stone(BODY_COLOR_PRESETS[1].absorption_rgb, 0.0);
    assert!(b > r, "Blue is bluer than it is red: r={r} b={b}");
    let [r, g, b] = display_rgb_for_stone(BODY_COLOR_PRESETS[5].absorption_rgb, 0.0);
    assert!(r > b && g > b, "Yellow is warm: {r} {g} {b}");
    let [r, g, b] = display_rgb_for_stone(BODY_COLOR_PRESETS[2].absorption_rgb, 0.0);
    assert!(r > g && r > b, "Red is red: {r} {g} {b}");
}

/// The real swatch path (`display_rgb_for_stone`, via `swatch_path_units`) against the
/// render's own rule (`material_for_stone` scale times `MODEL_UNIT_FACE_UP_PATH`), at 5, 7
/// and 10.87 mm, for every preset: the dot is the color the render shows. D65 on both sides,
/// 1/255 per channel.
#[test]
fn the_desktop_swatch_equals_the_sized_render_material_at_every_size() {
    use pleochroma::render_setup::{MODEL_UNIT_FACE_UP_PATH, material_for_stone};
    for preset in &BODY_COLOR_PRESETS {
        let material = GemMaterial::by_name("Cubic Zirconia")
            .expect("built-in")
            .with_body_color(preset.absorption_rgb);
        for width_mm in [5.0_f32, 7.0, 10.87] {
            let stone = material_for_stone(material.clone(), width_mm, &[]);
            let render = body_colors(
                &stone.absorption,
                f64::from(MODEL_UNIT_FACE_UP_PATH) * f64::from(stone.absorption_path_scale),
                Illuminant::D65,
            )
            .unpolarised
            .srgb;
            let swatch = display_rgb_for_stone(preset.absorption_rgb, width_mm);
            for (r, s) in render.iter().zip(swatch) {
                assert!(
                    r.abs_diff(s) <= 1,
                    "{} at {width_mm} mm: render {render:?} vs swatch {swatch:?}",
                    preset.label
                );
            }
        }
    }
}

// --- The view color renders pleochroic ---

#[test]
fn a_per_axis_custom_color_on_a_pleochroic_host_renders_pleochroic() {
    let solves = [fake_solve(460.0), fake_solve(580.0)];
    let choice = lch_color::custom_choice(TensorRank::Uniaxial, &solves, 5.0)
        .expect("two rows for a uniaxial host");
    let mut ctx = RenderContext {
        material_name: "Sapphire".to_string(),
        material_linked: false,
        ..Default::default()
    };
    ctx.view_body_color = Some(ViewColor::from_variant("Sapphire", choice.clone()));
    let tinted = ctx.tinted_material_override().expect("Sapphire resolves");
    assert!(
        tinted.absorption.is_pleochroic,
        "the view's Custom Color keeps o and e apart"
    );
    assert_eq!(tinted.absorption, choice.absorption);
    assert_eq!(tinted.absorption_unit, AbsorptionUnit::PerMm);
    assert_eq!(tinted.name, "Sapphire");
}

#[test]
fn a_legacy_view_color_on_a_pleochroic_host_flattens_exactly_as_saved() {
    let mut ctx = RenderContext {
        material_name: "Sapphire".to_string(),
        material_linked: false,
        ..Default::default()
    };
    ctx.view_body_color = Some(ViewColor::triple_only(YELLOW));
    let tinted = ctx.tinted_material_override().expect("Sapphire resolves");
    assert!(!tinted.absorption.is_pleochroic);
    assert_eq!(
        tinted.absorption,
        sapphire().with_body_color(YELLOW).absorption
    );
}

#[test]
fn the_view_hosts_and_selection_read_back_for_the_per_axis_editor() {
    let choice = named_choice();
    let ctx = RenderContext {
        material_name: "Sapphire".to_string(),
        material_linked: false,
        view_body_color: Some(ViewColor::from_variant("Sapphire", choice.clone())),
        ..Default::default()
    };
    let shared = Arc::new(Mutex::new(ctx));
    assert_eq!(
        view_host(&shared).expect("Sapphire resolves").name,
        "Sapphire"
    );
    assert_eq!(
        view_color_selection(&shared).color_variant_override,
        Some(choice)
    );
    RenderContext::lock(&shared).view_body_color = None;
    assert_eq!(view_color_selection(&shared), MaterialSelection::none());
}

#[test]
fn a_variant_of_another_material_is_dormant_and_shows_nothing() {
    let named = ViewColor::from_variant("Sapphire", named_choice());
    assert_eq!(
        applicable_view_color(Some(named.clone()), Some(&sapphire())),
        Some(named.clone())
    );
    assert_eq!(
        applicable_view_color(Some(named.clone()), Some(&diamond())),
        None,
        "a pleochroic variant does not fit an isotropic host"
    );
    assert_eq!(
        applicable_view_color(Some(named.clone()), None),
        Some(named),
        "with no host to check against it stays"
    );
    let legacy = ViewColor::triple_only(YELLOW);
    assert_eq!(
        applicable_view_color(Some(legacy.clone()), Some(&diamond())),
        Some(legacy),
        "a legacy color applies to every host"
    );
    assert_eq!(applicable_view_color(None, Some(&sapphire())), None);
}

// --- Dirty flag and persistence ---

#[test]
fn picking_a_view_colour_marks_the_render_dirty_only_when_it_applies() {
    let mut ctx = RenderContext {
        material_linked: false,
        dirty: false,
        ..Default::default()
    };
    let yellow = ViewColor::triple_only(YELLOW);
    update_context(&mut ctx, |c| c.view_body_color = Some(yellow.clone()));
    assert!(ctx.dirty, "a new colour on a free view restarts the render");
    assert_eq!(ctx.view_body_color, Some(yellow.clone()));

    // The same colour again changes nothing.
    ctx.dirty = false;
    update_context(&mut ctx, |c| c.view_body_color = Some(yellow.clone()));
    assert!(!ctx.dirty);

    // The same triple with band rows is a different colour: the render restarts.
    let banded = ViewColor {
        triple: YELLOW,
        bands: Some(vec![[460.0, 45.0, 0.25]]),
        variant: None,
    };
    update_context(&mut ctx, |c| c.view_body_color = Some(banded));
    assert!(ctx.dirty, "the rows changed what the render shows");
    ctx.dirty = false;
    update_context(&mut ctx, |c| c.view_body_color = Some(yellow));

    // A variant is another colour again, and the same variant twice is not a change.
    ctx.dirty = false;
    let variant = ViewColor::from_variant("Sapphire", named_choice());
    update_context(&mut ctx, |c| c.view_body_color = Some(variant.clone()));
    assert!(ctx.dirty, "a variant replaced the triple");
    ctx.dirty = false;
    update_context(&mut ctx, |c| c.view_body_color = Some(variant.clone()));
    assert!(!ctx.dirty);
    let mut other = named_choice();
    other.absorption.o_ray[0].peak = 0.09;
    update_context(&mut ctx, |c| {
        c.view_body_color = Some(ViewColor::from_variant("Sapphire", other));
    });
    assert!(ctx.dirty, "another tensor is another color");

    // While the linked open design drives the colour the setting is dormant: changing
    // it, or the link, only restarts the render when what is shown really changes.
    ctx.dirty = false;
    ctx.planes_owner = PlanesOwner::Editor { generation: 1 };
    update_context(&mut ctx, |c| c.material_linked = true);
    assert!(ctx.dirty, "the view colour stopped applying");
    ctx.dirty = false;
    update_context(&mut ctx, |c| c.view_body_color = None);
    assert!(!ctx.dirty, "a dormant colour is not a scene change");
}

#[test]
fn a_view_colour_is_saved_and_restored_as_triple_and_band_rows() {
    let rows = vec![[460.0f32, 45.0, 0.25], [580.0, 60.0, 0.1]];
    let banded = ViewColor {
        triple: YELLOW,
        bands: Some(rows.clone()),
        variant: None,
    };
    let (triple, bands, variant) = persisted_parts(Some(&banded));
    assert_eq!(
        (triple, bands.clone(), variant.clone()),
        (Some(YELLOW), Some(rows), None)
    );
    assert_eq!(restored_view_color(triple, bands, variant), Some(banded));

    let plain = ViewColor::triple_only(YELLOW);
    let (triple, bands, variant) = persisted_parts(Some(&plain));
    assert_eq!(
        (triple, bands.clone(), variant.clone()),
        (Some(YELLOW), None, None)
    );
    assert_eq!(restored_view_color(triple, bands, variant), Some(plain));

    assert_eq!(persisted_parts(None), (None, None, None));
    // Rows with no triple beside them, or an empty list, are not a colour.
    assert_eq!(
        restored_view_color(None, Some(vec![[500.0, 40.0, 0.5]]), None),
        None
    );
    assert_eq!(
        restored_view_color(Some(YELLOW), Some(Vec::new()), None),
        Some(ViewColor::triple_only(YELLOW))
    );
}

#[test]
fn a_view_variant_is_saved_as_the_variant_alone_and_wins_over_old_entries() {
    let color = ViewColor::from_variant("Sapphire", named_choice());
    let (triple, bands, variant) = persisted_parts(Some(&color));
    assert_eq!(triple, None, "the legacy entries are cleared with it");
    assert_eq!(bands, None);
    assert_eq!(variant, Some(("Sapphire".to_owned(), named_choice())));
    assert_eq!(
        restored_view_color(triple, bands, variant.clone()),
        Some(color.clone())
    );
    // A file that holds both (an older legacy color and a newer variant) restores the variant.
    assert_eq!(
        restored_view_color(Some(YELLOW), Some(vec![[460.0, 45.0, 0.25]]), variant),
        Some(color)
    );
}

#[test]
fn a_view_variant_survives_a_settings_file_round_trip() {
    use crate::settings::model::AppSettings;

    let color = ViewColor::from_variant("Sapphire", named_choice());
    let (triple, bands, variant) = persisted_parts(Some(&color));
    let mut settings = AppSettings {
        render_body_color_override: triple,
        render_body_color_bands: bands,
        ..AppSettings::default()
    };
    settings.set_render_view_variant(
        variant
            .as_ref()
            .map(|(material, choice)| (material.as_str(), choice)),
    );
    let text = toml::to_string(&settings).expect("settings must serialize");
    let restored: AppSettings = toml::from_str(&text).expect("settings must parse");
    assert_eq!(
        restored_view_color(
            restored.render_body_color_override,
            restored.render_body_color_bands.clone(),
            restored.render_view_variant()
        ),
        Some(color)
    );
}

#[test]
fn a_settings_file_with_only_the_old_fields_restores_the_legacy_color_as_saved() {
    use crate::settings::model::AppSettings;

    let rows = vec![[460.0f32, 45.0, 0.25]];
    let old = AppSettings {
        render_body_color_override: Some(YELLOW),
        render_body_color_bands: Some(rows.clone()),
        ..AppSettings::default()
    };
    let text = toml::to_string(&old).expect("settings must serialize");
    let restored: AppSettings = toml::from_str(&text).expect("an old file must parse");
    let color = restored_view_color(
        restored.render_body_color_override,
        restored.render_body_color_bands.clone(),
        restored.render_view_variant(),
    )
    .expect("the legacy color restores");
    assert_eq!(color.triple, YELLOW);
    assert_eq!(color.bands, Some(rows));
    assert_eq!(color.variant, None);

    // And it renders as before: the bands, per millimetre, on the pleochroic host.
    let ctx = RenderContext {
        material_name: "Sapphire".to_string(),
        material_linked: false,
        view_body_color: Some(color),
        ..Default::default()
    };
    let tinted = ctx.tinted_material_override().expect("Sapphire resolves");
    assert_eq!(tinted.absorption_unit, AbsorptionUnit::PerMm);
    assert!(!tinted.absorption.is_pleochroic, "as saved: no pleochroism");
}
