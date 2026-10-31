//! Pushes the session into the Color Pattern window's `ColorPatternModel`.
//!
//! Everything the window shows comes from here; it holds only what is typed. [`present`] sets every
//! property from the session, so a handler changes the session and calls it once. The preview's
//! picture, busy flag and handles are set by `preview.rs`.

use super::{
    colors::{ZoneSwatch, pattern_swatches},
    form::{form_hint, form_options, kind_of as color_kind_of},
    model::{
        ADD_ZONE_HINT, axis_labels, f32_of, kind_of, param_specs, shape_label, size_spec,
        softness_to_slider, tilt_turn_deg, zone_summary, zone_title,
    },
    presets::menu_labels,
    session::{PREVIEW_NOTE, Session},
    variants::variant_menu,
};
use crate::{
    ColorPatternModel, ColorPatternWindow, PatternParamRow, PatternZoneRow,
    gui::rough_colour::wizard::zone_rows::ShapeKind,
};
use pleochroma_cut_core::material::pattern::{
    EXACT_COLOR_KEY, MAX_PATTERN_ZONES, PatternColor, TemplateId, typical_patterns,
};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

/// The model of a list of texts.
fn strings<S: AsRef<str>>(items: impl IntoIterator<Item = S>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(|item| SharedString::from(item.as_ref()))
            .collect::<Vec<_>>(),
    ))
}

/// A swatch color as Slint paints it.
const fn paint(rgb: [u8; 3]) -> Color {
    Color::from_rgb_u8(rgb[0], rgb[1], rgb[2])
}

/// The label of the row of typical patterns ("Typical for Sapphire:"); empty when the species
/// has none.
fn typical_title(species: &str, none: bool) -> String {
    if none {
        String::new()
    } else {
        format!("Typical for {}:", species.trim())
    }
}

/// The sentence under the Variant menu of a zone holding `color`: what the copy means; empty for
/// a picked color.
fn variant_hint(session: &Session, color: &PatternColor) -> String {
    let PatternColor::Variant { key, .. } = color else {
        return String::new();
    };
    if key == EXACT_COLOR_KEY {
        return "This is the color the stone has, kept exactly. Pick a variant to replace it, or Custom to pick a color.".to_owned();
    }
    let Some(listed) = session.variants.iter().find(|variant| variant.key == *key) else {
        return "This variant is not listed for this material any more. The zone keeps its own copy."
            .to_owned();
    };
    let copy = "The zone keeps its own copy of this variant, so it does not change when the variant is edited later.";
    // An estimated built-in variant says where its numbers come from.
    super::variants::estimated_line(listed).map_or_else(
        || copy.to_owned(),
        |estimated| format!("{copy} {estimated}"),
    )
}

/// The rows of the zone list.
fn zone_rows(session: &Session, swatches: &[ZoneSwatch]) -> Vec<PatternZoneRow> {
    let pattern = &session.pattern;
    let selected = session.selected();
    std::iter::once(&pattern.base)
        .chain(&pattern.zones)
        .enumerate()
        .map(|(index, zone)| {
            let swatch = swatches
                .get(index)
                .copied()
                .unwrap_or(ZoneSwatch::uniform([128; 3]));
            PatternZoneRow {
                title: zone_title(index).into(),
                name: zone.name.as_str().into(),
                shape: shape_label(index, zone).into(),
                summary: zone_summary(index, zone, pattern.relative_to_stone).into(),
                swatch: paint(swatch.primary),
                swatch_across: paint(swatch.across),
                dichroic: swatch.dichroic,
                selected: index == selected,
            }
        })
        .collect()
}

/// The Advanced parameter rows of the selected zone.
fn param_rows(session: &Session) -> Vec<PatternParamRow> {
    let selected = session.selected();
    if selected == 0 {
        return Vec::new();
    }
    let Some(zone) = session.pattern.zone(selected) else {
        return Vec::new();
    };
    param_specs(
        zone,
        session.pattern.relative_to_stone,
        session.stone_width_mm,
    )
    .into_iter()
    .map(|spec| PatternParamRow {
        text: spec.text().into(),
        label: spec.label.into(),
        value: f32_of(spec.value),
        min: f32_of(spec.min),
        max: f32_of(spec.max),
        step: f32_of(spec.step),
        unit: spec.unit.into(),
        enabled: true,
        hint: spec.hint.into(),
    })
    .collect()
}

/// The header: title, mode flags and what the window may offer.
fn present_header(model: &ColorPatternModel<'_>, session: &Session, advanced: bool) {
    model.set_title(format!("Color Pattern \u{2014} {}", session.material_name).into());
    model.set_show_advanced(advanced);
    model.set_dirty(session.dirty());
    model.set_solving_colors(session.solving);
    model.set_confirm_discard(session.confirm_discard);
    model.set_preset_naming(session.naming_preset);
    let is_stone = session.stone_apply.is_some();
    model.set_stone_target(is_stone);
    model.set_targets_design(session.targets_design);
    model.set_design_preview_available(session.preview_design.is_some());
    model.set_picker_open(is_stone && session.pending_pick.is_some());
}

/// The templates, the stone's typical patterns, the presets and the preview's mode (the picture
/// itself is `preview.rs`'s).
fn present_templates_and_preview(model: &ColorPatternModel<'_>, session: &Session) {
    model.set_template_labels(strings(TemplateId::ALL.iter().map(|id| id.label())));
    let typical = typical_patterns(&session.base_material.name);
    model.set_typical_title(typical_title(&session.base_material.name, typical.is_empty()).into());
    model.set_typical_labels(strings(typical.iter().map(|entry| entry.label)));
    model.set_preset_labels(strings(menu_labels(&session.saved_presets)));

    model.set_preview_mode(session.preview_mode);
    model.set_slice_pos(session.slice_pos);
    let note = session.import_note.as_ref().map_or_else(
        || PREVIEW_NOTE.to_owned(),
        |note| format!("{note} {PREVIEW_NOTE}"),
    );
    model.set_preview_note(note.into());
}

/// The zone list and the buttons that change it.
fn present_zone_list(model: &ColorPatternModel<'_>, session: &Session, swatches: &[ZoneSwatch]) {
    let pattern = &session.pattern;
    let selected = session.selected();
    model.set_zone_rows(ModelRc::new(VecModel::from(zone_rows(session, swatches))));
    model.set_selected_zone(i32::try_from(selected).unwrap_or(0));
    let can_add = pattern.zones.len() < MAX_PATTERN_ZONES;
    model.set_can_add_zone(can_add);
    let add_hint = if can_add { "" } else { ADD_ZONE_HINT };
    model.set_add_zone_hint(add_hint.into());
    model.set_can_edit_zone(selected > 0);
}

/// The name, shape and size controls of the selected zone.
fn present_zone_geometry(model: &ColorPatternModel<'_>, session: &Session) {
    let pattern = &session.pattern;
    let relative = pattern.relative_to_stone;
    let selected = session.selected();
    let zone = pattern.zone(selected).unwrap_or(&pattern.base);
    model.set_zone_name(zone.name.as_str().into());
    model.set_shape_options(strings(ShapeKind::ALL.iter().map(|kind| kind.name())));
    let shape_index = kind_of(&zone.shape)
        .and_then(|kind| {
            ShapeKind::ALL
                .iter()
                .position(|candidate| *candidate == kind)
        })
        .and_then(|at| i32::try_from(at).ok())
        .unwrap_or(0);
    model.set_shape_index(shape_index);
    model.set_shape_enabled(selected > 0);
    model.set_param_rows(ModelRc::new(VecModel::from(param_rows(session))));
    if let Some(size) = (selected > 0)
        .then(|| size_spec(&zone.shape, relative, session.stone_width_mm))
        .flatten()
    {
        model.set_size_min(f32_of(size.min));
        model.set_size_max(f32_of(size.max));
        model.set_size_step(f32_of(size.step));
        model.set_size_value(f32_of(size.value));
        model.set_size_text(size.text.into());
        model.set_size_enabled(true);
    } else {
        model.set_size_text("".into());
        model.set_size_enabled(false);
    }
}

/// The color of the selected zone: its form, its variant and its swatches.
fn present_zone_color(
    model: &ColorPatternModel<'_>,
    session: &Session,
    swatch: Option<ZoneSwatch>,
) {
    let pattern = &session.pattern;
    let color = &pattern
        .zone(session.selected())
        .unwrap_or(&pattern.base)
        .color;
    model.set_color_kind(color_kind_of(color));
    // A picked color: the forms the material's rank offers, and the one the zone has.
    let options = form_options(session.rank, color);
    model.set_color_form_options(strings(
        options.iter().flat_map(|options| options.labels.iter()),
    ));
    model.set_color_form_index(
        options
            .as_ref()
            .and_then(|options| i32::try_from(options.index).ok())
            .unwrap_or(0),
    );
    model.set_color_form_hint(form_hint(session.rank, color).into());
    // A variant of the material.
    let (variant_labels, variant_index) = variant_menu(&session.variants, color);
    model.set_variants_available(!session.variants.is_empty() || !color.is_custom());
    model.set_color_source_index(i32::from(!color.is_custom()));
    model.set_variant_labels(strings(variant_labels));
    model.set_variant_index(variant_index);
    model.set_variant_hint(variant_hint(session, color).into());
    if let Some(swatch) = swatch {
        model.set_single_swatch(paint(swatch.blend));
        model.set_along_swatch(paint(swatch.primary));
        model.set_beta_swatch(paint(swatch.beta));
        model.set_across_swatch(paint(swatch.across));
    }
}

/// The pattern's own settings: axis, size rule and softness.
fn present_pattern_settings(model: &ColorPatternModel<'_>, session: &Session) {
    let pattern = &session.pattern;
    let relative = pattern.relative_to_stone;
    model.set_axis_options(strings(axis_labels(&pattern.axis)));
    model.set_axis_index(session.axis_menu_index());
    let (tilt, turn) = tilt_turn_deg(&pattern.axis);
    model.set_tilt_deg(f32_of(tilt));
    model.set_turn_deg(f32_of(turn));
    model.set_scales_with_stone(relative);
    model.set_softness(softness_to_slider(
        pattern.softness,
        relative,
        session.stone_width_mm,
    ));
}

/// Sets every property of `window`'s `ColorPatternModel` that follows from `session`.
/// `advanced` is whether the Advanced controls are shown (`GuideModel.shows_advanced()` of the main
/// window).
pub(super) fn present(window: &ColorPatternWindow, session: &Session, advanced: bool) {
    let model = window.global::<ColorPatternModel>();
    present_header(&model, session, advanced);
    present_templates_and_preview(&model, session);
    let swatches = pattern_swatches(&session.pattern, &session.swatch_stone());
    present_zone_list(&model, session, &swatches);
    present_zone_geometry(&model, session);
    present_zone_color(&model, session, swatches.get(session.selected()).copied());
    present_pattern_settings(&model, session);

    // Validation and history.
    model.set_message(session.message().into());
    model.set_can_undo(session.undo.can_undo());
    model.set_can_redo(session.undo.can_redo());
}
