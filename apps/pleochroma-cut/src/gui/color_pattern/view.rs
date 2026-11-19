//! Pushes the session into the Color Pattern window's `ColorPatternModel`.
//!
//! Everything the window shows comes from here; it holds only what is typed. [`present`] sets
//! every property from the session, so a handler changes the session and calls it once. The
//! stage's pictures, the render state and the handles are set by `preview.rs`, the picker's values
//! by `picker.rs` and the gallery's tiles by `gallery.rs`.
//!
//! The layer list shows the top layer first and the base last, the mirror of the stored order
//! (`order.rs`); `selected_row` and every row index the window sends are list rows.

use super::{
    colors::{ZoneSwatch, pattern_swatches},
    form::{is_legacy, rank_text, tile_specs},
    gallery::typical_section,
    model::{
        ADD_ZONE_HINT, axis_labels, f32_of, kind_of, param_specs, shape_label, size_spec,
        softness_to_slider, subtitle, tilt_turn_deg, zone_summary, zone_title,
    },
    order::{flip, stored_to_row},
    session::Session,
    variants::variant_menu,
};
use crate::{
    ColorPatternModel, ColorPatternWindow, PatternColorTile, PatternParamRow, PatternZoneRow,
    gui::rough_colour::wizard::zone_rows::ShapeKind,
};
use pleochroma_cut_core::material::pattern::{MAX_PATTERN_ZONES, PatternColor};
use slint::{Color, ComponentHandle, Model, ModelRc, SharedString, VecModel};

/// The window is `narrow` below this width, logical pixels.
const NARROW_BELOW: f64 = 1060.0;

/// The model of a list of texts.
fn strings<S: AsRef<str>>(items: impl IntoIterator<Item = S>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(|item| SharedString::from(item.as_ref()))
            .collect::<Vec<_>>(),
    ))
}

/// Puts `rows` into the list `current` holds: row by row while the length is unchanged (only
/// the rows that differ), a new model when it changed or there is none yet.
///
/// A `for` loop keeps its row components while its model stays the same, so a re-present does
/// not rebuild the layer cards. That keeps an open list, such as From variant, and its keyboard
/// focus alive while the arrow keys pick one entry after another.
fn update_rows<T: Clone + PartialEq + 'static>(
    current: &ModelRc<T>,
    rows: Vec<T>,
    set: impl FnOnce(ModelRc<T>),
) {
    if current.row_count() != rows.len() || rows.is_empty() {
        set(ModelRc::new(VecModel::from(rows)));
        return;
    }
    for (index, row) in rows.into_iter().enumerate() {
        if current.row_data(index).as_ref() != Some(&row) {
            current.set_row_data(index, row);
        }
    }
}

/// A swatch color as Slint paints it.
const fn paint(rgb: [u8; 3]) -> Color {
    Color::from_rgb_u8(rgb[0], rgb[1], rgb[2])
}

/// Whether a window `width_px` physical pixels wide at `scale` is narrow (below 1060 logical
/// pixels). A window that has no size yet is not.
pub(super) fn is_narrow(width_px: u32, scale: f32) -> bool {
    width_px > 0 && f64::from(width_px) / f64::from(scale.max(0.1)) < NARROW_BELOW
}

/// Whether `window` is narrow right now.
pub(super) fn window_narrow(window: &ColorPatternWindow) -> bool {
    is_narrow(window.window().size().width, window.window().scale_factor())
}

/// The position of the shape of `zone` among `ShapeKind::ALL`; -1 for the base and for a shape
/// the editor has no icon for.
fn shape_icon(index: usize, zone: &pleochroma_cut_core::material::pattern::PatternZone) -> i32 {
    if index == 0 {
        return -1;
    }
    kind_of(&zone.shape)
        .and_then(|kind| {
            ShapeKind::ALL
                .iter()
                .position(|candidate| *candidate == kind)
        })
        .and_then(|at| i32::try_from(at).ok())
        .unwrap_or(-1)
}

/// The rows of the layer list: the top layer first, the base last.
fn zone_rows(session: &Session, swatches: &[ZoneSwatch]) -> Vec<PatternZoneRow> {
    let pattern = &session.pattern;
    let selected = session.selected();
    let zones = pattern.zones.len();
    (0..=zones)
        .map(|row| {
            let index = flip(row, zones);
            let zone = pattern.zone(index).unwrap_or(&pattern.base);
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
                swatch_mid: paint(swatch.beta),
                dichroic: swatch.dichroic,
                selected: index == selected,
                hidden: session.hidden.contains(index),
                shape_icon: shape_icon(index, zone),
                is_base: index == 0,
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

/// The header: title, subtitle, the swatch chip, mode flags and what the window may offer.
fn present_header(
    model: &ColorPatternModel<'_>,
    window: &ColorPatternWindow,
    session: &Session,
    swatches: &[ZoneSwatch],
    advanced: bool,
) {
    // The bare name: the window adds "Color Pattern \u{2014}" itself.
    model.set_title(session.material_name.as_str().into());
    model.set_subtitle(subtitle(&session.pattern).into());
    // The chip: the base color and the first zone's.
    let base = swatches.first().map_or([128; 3], |swatch| swatch.blend);
    let first = swatches.get(1).map_or(base, |swatch| swatch.blend);
    model.set_header_along(paint(base));
    model.set_header_across(paint(first));
    model.set_show_advanced(advanced);
    model.set_dirty(session.dirty());
    model.set_solving_colors(session.solving);
    model.set_confirm_discard(session.confirm_discard);
    model.set_narrow(window_narrow(window));
    model.set_stone_target(session.stone_apply.is_some());
    model.set_targets_design(session.targets_design);
    model.set_design_preview_available(session.preview_design.is_some());
}

/// The gallery's state (its tile lists are `gallery.rs`'s) and the stage's view and hint.
fn present_gallery_and_stage(model: &ColorPatternModel<'_>, session: &Session) {
    model.set_gallery_open(session.gallery_open);
    model.set_preset_naming(session.naming_preset);
    model.set_typical_section(typical_section(&session.base_material.name).into());
    model.set_view(session.view);
    model.set_slice_pos(session.slice_pos);
    model.set_render_hint(session.render_hint().into());
}

/// The layer list and the buttons that change it.
fn present_layers(model: &ColorPatternModel<'_>, session: &Session, swatches: &[ZoneSwatch]) {
    let pattern = &session.pattern;
    let zones = pattern.zones.len();
    update_rows(
        &model.get_zone_rows(),
        zone_rows(session, swatches),
        |rows| model.set_zone_rows(rows),
    );
    let row = stored_to_row(session.selected(), zones);
    model.set_selected_row(i32::try_from(row).unwrap_or(0));
    let can_add = zones < MAX_PATTERN_ZONES;
    model.set_can_add_zone(can_add);
    let add_hint = if can_add { "" } else { ADD_ZONE_HINT };
    model.set_add_zone_hint(add_hint.into());
}

/// The shape and size controls of the selected zone.
fn present_zone_geometry(model: &ColorPatternModel<'_>, session: &Session) {
    let pattern = &session.pattern;
    let relative = pattern.relative_to_stone;
    let selected = session.selected();
    let zone = pattern.zone(selected).unwrap_or(&pattern.base);
    model.set_shape_options(strings(ShapeKind::ALL.iter().map(|kind| kind.name())));
    model.set_shape_index(shape_icon(selected, zone).max(0));
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

/// The color tiles of the selected zone, by what its color has.
fn color_tiles(color: &PatternColor, swatch: ZoneSwatch) -> Vec<PatternColorTile> {
    let from_variant = matches!(color, PatternColor::Variant { .. });
    tile_specs(color)
        .into_iter()
        .map(|spec| PatternColorTile {
            caption: spec.caption.into(),
            color: paint(swatch.part(spec.part)),
            slot: spec.slot,
            from_variant,
        })
        .collect()
}

/// The color of the selected zone: its tiles, its variant and the legacy notice.
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
    let swatch = swatch.unwrap_or(ZoneSwatch::uniform([128; 3]));
    model.set_color_tiles(ModelRc::new(VecModel::from(color_tiles(color, swatch))));
    model.set_rank_text(rank_text(session.rank).into());
    model.set_legacy_notice(is_legacy(session.rank, color));
    // "Custom color" first, then the material's variants; the entry the zone has is selected.
    let (labels, index) = variant_menu(&session.variants, color);
    update_rows(
        &model.get_variant_labels(),
        std::iter::once("Custom color".to_owned())
            .chain(labels)
            .map(SharedString::from)
            .collect(),
        |rows| model.set_variant_labels(rows),
    );
    model.set_variant_index(if index < 0 { 0 } else { index + 1 });
    let caption = match color {
        PatternColor::Variant { label, .. } => format!("From {label}"),
        _ => String::new(),
    };
    model.set_variant_caption(caption.into());
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
/// `advanced` is whether the Advanced controls are shown (`GuideModel.shows_advanced()` of the
/// main window).
pub(super) fn present(window: &ColorPatternWindow, session: &Session, advanced: bool) {
    let model = window.global::<ColorPatternModel>();
    let swatches = pattern_swatches(&session.pattern, &session.swatch_stone());
    present_header(&model, window, session, &swatches, advanced);
    present_gallery_and_stage(&model, session);
    present_layers(&model, session, &swatches);
    present_zone_geometry(&model, session);
    present_zone_color(&model, session, swatches.get(session.selected()).copied());
    present_pattern_settings(&model, session);
    model.set_picker_open(session.picker.is_some());

    // Validation and history.
    model.set_message(session.message().into());
    model.set_can_undo(session.undo.can_undo());
    model.set_can_redo(session.undo.can_redo());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_below_1060_logical_pixels_is_narrow() {
        assert!(is_narrow(960, 1.0));
        assert!(!is_narrow(1120, 1.0));
        assert!(!is_narrow(1060, 1.0));
        // At a scale of 2 the window is half as wide in logical pixels.
        assert!(is_narrow(2000, 2.0));
        assert!(!is_narrow(2200, 2.0));
        // A window with no size yet is not narrow.
        assert!(!is_narrow(0, 1.0));
    }

    /// The same length keeps the model (only changed rows are written); a new length replaces it.
    #[test]
    fn rows_of_the_same_length_update_the_model_in_place() {
        let first: ModelRc<i32> = ModelRc::new(VecModel::from(vec![1, 2, 3]));
        let mut replaced = None;
        update_rows(&first, vec![1, 5, 3], |rows| replaced = Some(rows));
        assert!(replaced.is_none(), "same length: no new model");
        assert_eq!(first.iter().collect::<Vec<_>>(), vec![1, 5, 3]);

        update_rows(&first, vec![1, 5], |rows| replaced = Some(rows));
        let replaced = replaced.expect("a new length gets a new model");
        assert_eq!(replaced.iter().collect::<Vec<_>>(), vec![1, 5]);
    }
}
