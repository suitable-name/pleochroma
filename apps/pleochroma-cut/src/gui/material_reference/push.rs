//! Copies the window's plain data ([`Session`], [`MaterialDetail`]) into the properties of
//! `MaterialReferenceModel`. Every function here only sets properties; the decisions are made in
//! `session.rs` and in `pleochroma_editor::material_reference`.

use super::{
    prefs::{column_index, filter_index},
    session::Session,
};
use crate::{
    BandRow, ColorRow, CurveSeries, DataRow, MaterialReferenceModel, RefChip, RefGuide, RefHeader,
    RefHover, RefRow, RefTick, VariantRow,
};
use pleochroma_editor::material_reference::{
    DispersionScale, MaterialDetail, ReferenceRow,
    curves::{AxisTick, FraunhoferGuide},
    detail::{DetailRow, Swatch, WeightDetail},
    filter_counts,
    text::DASH,
};
use slint::{Color, ModelRc, SharedString, VecModel};
use std::fmt::Write as _;

/// A slint color from an sRGB triple.
const fn color(rgb: [u8; 3]) -> Color {
    Color::from_rgb_u8(rgb[0], rgb[1], rgb[2])
}

/// A model of `items`.
fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// `text`, or a dash when it is empty or missing.
fn or_dash(text: Option<&str>) -> SharedString {
    text.filter(|value| !value.is_empty())
        .unwrap_or(DASH)
        .into()
}

/// The table row of `row` for the dispersion convention `scale`.
fn ref_row(row: &ReferenceRow, scale: DispersionScale) -> RefRow {
    let dispersion = match scale {
        DispersionScale::BG => &row.dispersion_bg_text,
        DispersionScale::FC => &row.dispersion_fc_text,
    };
    let (hardness_lo, hardness_hi) = row.gem.hardness_range.unwrap_or((-1.0, -1.0));
    let hardness = or_dash(row.gem.hardness.as_deref());
    let toughness = or_dash(row.gem.toughness.as_deref());
    let cleavage = or_dash(row.gem.cleavage.as_deref());
    let summary = format!(
        "{}, {}, {}, refractive index {}, birefringence {}, dispersion {}, specific gravity {}, \
         hardness {}, toughness {}, cleavage {}",
        row.name,
        row.crystal_system,
        row.optic_character,
        row.ri_text,
        row.birefringence_text,
        dispersion,
        row.sg_text,
        hardness,
        toughness,
        cleavage,
    );
    RefRow {
        name: row.name.as_str().into(),
        tag: row.origin.tag().into(),
        swatch: color(row.swatch),
        crystal_system: row.crystal_system.as_str().into(),
        optic_character: row.optic_character.as_str().into(),
        ri: row.ri_text.as_str().into(),
        birefringence: row.birefringence_text.as_str().into(),
        dispersion: dispersion.as_str().into(),
        sg: row.sg_text.as_str().into(),
        hardness,
        hardness_lo,
        hardness_hi,
        toughness,
        cleavage,
        summary: summary.into(),
    }
}

/// How many materials each filter chip would show for the current search text: the filter is
/// ignored, the search is not, so the chips read "All 3" while the box holds a query.
fn search_counts(session: &Session) -> [usize; 5] {
    let matching: Vec<ReferenceRow> = session.search_matches().into_iter().cloned().collect();
    filter_counts(&matching)
}

/// The one Specific gravity row: "3.53 (3.49\u{2013}3.57) \u{b7} about 1.9 ct at 7 mm". The
/// design's estimate joins it when a design is open.
fn weight_row(weight: &WeightDetail) -> String {
    let estimate = weight
        .design_line
        .as_deref()
        .and_then(|line| line.strip_prefix("This design: "));
    match estimate {
        Some(estimate) if weight.sg.is_some() => {
            format!("{} \u{b7} {estimate}", weight.sg_text)
        }
        _ => weight.sg_text.clone(),
    }
}

/// Pushes the table: the visible rows, the counts on the chips, the view, the selection, the
/// subtitle, the footer and the page.
pub(super) fn push_table(model_global: &MaterialReferenceModel<'_>, session: &Session) {
    let prefs = session.prefs();
    let rows: Vec<RefRow> = session
        .visible()
        .into_iter()
        .map(|row| ref_row(row, prefs.scale))
        .collect();
    let counts: Vec<i32> = search_counts(session)
        .iter()
        .map(|count| i32::try_from(*count).unwrap_or(i32::MAX))
        .collect();
    model_global.set_rows(model(rows));
    model_global.set_filter_counts(model(counts));
    model_global.set_sort_column(column_index(prefs.column));
    model_global.set_sort_descending(prefs.descending);
    model_global.set_filter(filter_index(prefs.filter));
    model_global.set_query(prefs.query.as_str().into());
    model_global.set_dispersion_bg(prefs.scale == DispersionScale::BG);
    model_global.set_selected(
        session
            .selected_index()
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1),
    );
    model_global.set_subtitle(session.subtitle().into());
    model_global.set_footer(session.footer().into());
    model_global.set_page(session.page().index());
}

/// A data row of a card.
fn data_row(row: &DetailRow) -> DataRow {
    DataRow {
        label: row.label.as_str().into(),
        value: row.value.as_str().into(),
        note: row.note.as_str().into(),
        tooltip: row.tooltip.as_str().into(),
        missing: row.missing,
    }
}

/// A plain label and value row.
fn text_row(label: &str, value: &str) -> DataRow {
    DataRow {
        label: label.into(),
        value: value.into(),
        note: SharedString::default(),
        tooltip: SharedString::default(),
        missing: false,
    }
}

/// A chart tick.
fn tick(tick: &AxisTick) -> RefTick {
    RefTick {
        text: tick.text.as_str().into(),
        position: tick.position,
    }
}

/// A Fraunhofer guide line.
fn guide(guide: &FraunhoferGuide) -> RefGuide {
    RefGuide {
        label: guide.label.into(),
        x: guide.x,
    }
}

/// One color of the Colors card with its out-of-gamut flag.
const fn swatch_color(swatch: Swatch) -> Color {
    color(swatch.srgb)
}

/// The hover cards of the index chart: one per sampled wavelength.
fn hover_cards(detail: &MaterialDetail) -> Vec<RefHover> {
    detail
        .ri
        .samples
        .iter()
        .map(|sample| {
            let mut text = format!("\u{3bb} {:.0} nm", sample.wavelength_nm);
            for (series, index) in detail.ri.series.iter().zip(&sample.indices) {
                let _ = write!(text, " \u{b7} {} {index:.4}", series.label);
            }
            RefHover {
                x: sample.x,
                text: text.into(),
            }
        })
        .collect()
}

/// The refractive-index card.
fn push_index(model_global: &MaterialReferenceModel<'_>, detail: &MaterialDetail) {
    let ri = &detail.ri;
    let series: Vec<CurveSeries> = ri
        .series
        .iter()
        .map(|series| CurveSeries {
            label: series.label.as_str().into(),
            value: series.nd_text.as_str().into(),
            path: series.path.as_str().into(),
            ink: i32::try_from(series.ink).unwrap_or(0),
        })
        .collect();
    model_global.set_ri_series(model(series));
    model_global.set_ri_y_ticks(model(ri.y_ticks.iter().map(tick).collect()));
    model_global.set_ri_x_ticks(model(ri.x_ticks.iter().map(tick).collect()));
    model_global.set_ri_guides(model(ri.guides.iter().map(guide).collect()));
    model_global.set_ri_readout(ri.readout.as_str().into());
    model_global.set_ri_model_line(ri.model_line.as_str().into());
    model_global.set_ri_hover(model(hover_cards(detail)));
}

/// The spectrum card: the chart of the chosen mode, the swatches of its rays and the band table.
fn push_spectrum(
    model_global: &MaterialReferenceModel<'_>,
    detail: &MaterialDetail,
    transmission: bool,
) {
    let spectrum = &detail.spectrum;
    let series: Vec<CurveSeries> = spectrum
        .series
        .iter()
        .map(|ray| CurveSeries {
            label: ray.label.as_str().into(),
            value: SharedString::default(),
            path: if transmission {
                ray.transmission_path.as_str().into()
            } else {
                ray.absorption_path.as_str().into()
            },
            ink: i32::try_from(ray.ink).unwrap_or(0),
        })
        .collect();
    let swatches: Vec<Color> = spectrum
        .series
        .iter()
        .map(|ray| color(ray.swatch))
        .collect();
    let bands: Vec<BandRow> = spectrum
        .series
        .iter()
        .flat_map(|ray| ray.bands.iter())
        .map(|band| BandRow {
            ray: band.ray.as_str().into(),
            center: format!("{:.0} nm", band.center_nm).into(),
            width: band.width_text.as_str().into(),
            peak: band.peak_text.as_str().into(),
            shape: band.shape.as_str().into(),
        })
        .collect();
    model_global.set_spectrum_series(model(series));
    model_global.set_spectrum_swatches(model(swatches));
    model_global.set_spectrum_bands(model(bands));
    model_global.set_spectrum_x_ticks(model(spectrum.x_ticks.iter().map(tick).collect()));
    model_global.set_spectrum_summary(spectrum.summary.as_str().into());
    model_global.set_spectrum_provenance(spectrum.provenance.as_str().into());
    model_global.set_spectrum_variant(spectrum.variant_label.as_str().into());
    model_global.set_spectrum_estimated(spectrum.estimated);
    model_global.set_spectrum_tooltip(spectrum.transmission_tooltip.as_str().into());
    model_global.set_spectrum_transmission(transmission);
}

/// The colors card, the variant list and the effects line.
fn push_colors_and_variants(model_global: &MaterialReferenceModel<'_>, detail: &MaterialDetail) {
    let colors: Vec<ColorRow> = detail
        .colors
        .rows
        .iter()
        .map(|row| ColorRow {
            label: row.label.as_str().into(),
            daylight: swatch_color(row.daylight),
            incandescent: swatch_color(row.incandescent),
            daylight_clipped: row.daylight.out_of_gamut,
            incandescent_clipped: row.incandescent.out_of_gamut,
            face_up: row.face_up,
        })
        .collect();
    model_global.set_color_rows(model(colors));
    model_global.set_color_note(detail.colors.note.as_str().into());
    model_global.set_color_change(detail.colors.color_change);
    let variants: Vec<VariantRow> = detail
        .variants
        .iter()
        .map(|variant| VariantRow {
            label: variant.label.as_str().into(),
            swatch: color(variant.swatch),
            source: variant.source_line.as_str().into(),
            estimated: variant.estimated,
            selected: variant.selected,
            user: variant.user,
        })
        .collect();
    let selected = detail
        .variants
        .iter()
        .position(|variant| variant.selected)
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(0);
    model_global.set_variant_rows(model(variants));
    model_global.set_selected_variant(selected);
    model_global.set_effects_summary(detail.effects.summary.as_str().into());
}

/// The durability, weight and source cards.
fn push_facts(model_global: &MaterialReferenceModel<'_>, detail: &MaterialDetail) {
    let durability = &detail.durability;
    model_global.set_durability_rows(model(durability.rows.iter().map(data_row).collect()));
    let (mohs_lo, mohs_hi) = durability.mohs_range.unwrap_or((-1.0, -1.0));
    model_global.set_mohs_lo(mohs_lo);
    model_global.set_mohs_hi(mohs_hi);
    model_global.set_durability_notes(durability.notes.join("\n").into());
    model_global.set_care(durability.care.as_str().into());
    model_global.set_sources(durability.sources.as_str().into());
    let weight = &detail.weight;
    model_global.set_weight_line(weight_row(weight).into());
    model_global.set_weight_basis(weight.basis_note.as_deref().unwrap_or_default().into());
    // The estimate is part of the Specific gravity row (`weight_line`).
    model_global.set_weight_design(SharedString::default());
    let groups: Vec<DataRow> = detail
        .sources
        .iter()
        .map(|group| text_row(&group.title, &group.lines.join("\n")))
        .collect();
    model_global.set_source_groups(model(groups));
}

/// The header: names, family line, the split swatch and the chips.
fn push_header(model_global: &MaterialReferenceModel<'_>, detail: &MaterialDetail) {
    let header = &detail.header;
    let chips: Vec<RefChip> = header
        .chips
        .iter()
        .map(|chip| RefChip {
            text: chip.text.as_str().into(),
            tooltip: chip.tooltip.as_str().into(),
            highlight: chip.highlight,
        })
        .collect();
    model_global.set_header(RefHeader {
        name: header.name.as_str().into(),
        family: header.family_line.as_str().into(),
        swatch_day: color(header.swatch_daylight),
        swatch_night: color(header.swatch_incandescent),
        chips: model(chips),
        color_change: header.color_change,
        custom: detail.is_custom,
    });
}

/// Pushes every card of the details page. `design_open` shows the Use for Design button.
pub(super) fn push_detail(
    model_global: &MaterialReferenceModel<'_>,
    detail: &MaterialDetail,
    transmission: bool,
    design_open: bool,
) {
    push_header(model_global, detail);
    push_index(model_global, detail);
    model_global.set_optics_rows(model(detail.optics.rows.iter().map(data_row).collect()));
    push_spectrum(model_global, detail, transmission);
    push_colors_and_variants(model_global, detail);
    push_facts(model_global, detail);
    model_global.set_can_use_for_design(design_open);
}

/// Empties the details page (no material is selected).
pub(super) fn clear_detail(model_global: &MaterialReferenceModel<'_>) {
    model_global.set_header(RefHeader::default());
    model_global.set_ri_series(ModelRc::default());
    model_global.set_spectrum_series(ModelRc::default());
    model_global.set_spectrum_bands(ModelRc::default());
    model_global.set_color_rows(ModelRc::default());
    model_global.set_variant_rows(ModelRc::default());
    model_global.set_optics_rows(ModelRc::default());
    model_global.set_durability_rows(ModelRc::default());
    model_global.set_source_groups(ModelRc::default());
    model_global.set_can_use_for_design(false);
}
