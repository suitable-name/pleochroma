//! The Material Reference dialog: a compact table of every material (Design > Material
//! Reference...) and, behind a row or the Design settings' Details... button, one column of
//! curves, spectra, colors and gem data for one material.
//!
//! The rows, the sort and filter, and the details page come from the editor's shared view model
//! through `pleochroma_web_core::material_reference`; this module keeps the dialog's sort,
//! filter and selection, and copies the finished text and chart paths into
//! `MaterialReferenceModel`. The facts line under the Material combo in Design Settings reads its
//! text through the model's pure `gem-summary-for` and `gem-warns-for` callbacks, so it follows
//! the material combo without a push. Custom materials read "No gem data yet": the browser stores
//! no gem data for them.

use crate::{
    AppWindow, MaterialReferenceModel, RefBand, RefColorRow, RefFilterChip, RefLine, RefSeries,
    RefSourceLine, RefTableRow, RefTick, RefVariant, app::Ctx,
};
use pleochroma_editor::material_reference::{
    ReferenceRow,
    detail::{DetailRow, MaterialDetail, Swatch},
};
use pleochroma_web_core::{
    design_color::stone_width_mm,
    material_reference::{self as reference, TableState, TableView},
};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;

/// The separator of the header's figures: " · ".
const MIDDLE_DOT: &str = " \u{b7} ";

/// The dialog's state between pushes.
#[derive(Default)]
struct Runtime {
    /// Every material's shared row, in the catalogue's order.
    rows: Vec<ReferenceRow>,
    /// The chip, search and sort the table shows.
    table: TableState,
    /// The selected material's name.
    selected: Option<String>,
    /// The names of the rows the table shows, top to bottom.
    visible: Vec<String>,
    /// The keys of the variants the details page lists, in order.
    variant_keys: Vec<String>,
    /// The key of the variant the details page shows (empty: the Material Default).
    variant_key: String,
}

thread_local! {
    static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::default());
}

/// A model of `items`.
fn model_of<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// The Slint color of an sRGB triple.
fn color_of(rgb: [u8; 3]) -> Color {
    Color::from_rgb_u8(rgb[0], rgb[1], rgb[2])
}

/// The Slint color of a swatch.
fn swatch_color(swatch: Swatch) -> Color {
    color_of(swatch.srgb)
}

/// A card line of a detail row.
fn line_of(row: &DetailRow) -> RefLine {
    RefLine {
        label: row.label.as_str().into(),
        value: row.value.as_str().into(),
        note: row.note.as_str().into(),
        missing: row.missing,
    }
}

/// The table's rows, chips, sort and selection.
fn table_into(model: &MaterialReferenceModel<'_>, view: &TableView, state: &RuntimeView<'_>) {
    model.set_subtitle(view.subtitle.as_str().into());
    model.set_footer(view.footer.as_str().into());
    model.set_rows(model_of(
        view.rows
            .iter()
            .map(|row| RefTableRow {
                name: row.name.as_str().into(),
                tag: row.tag.as_str().into(),
                swatch: color_of(row.swatch),
                ri: row.ri.as_str().into(),
                biref: row.birefringence.as_str().into(),
                disp: row.dispersion.as_str().into(),
                sg: row.sg.as_str().into(),
                hardness: row.hardness.as_str().into(),
                hardness_fill: row.hardness_fill,
                toughness: row.toughness.as_str().into(),
                cleavage: row.cleavage.as_str().into(),
            })
            .collect(),
    ));
    model.set_chips(model_of(
        view.chips
            .iter()
            .map(|chip| RefFilterChip {
                id: chip.id,
                label: chip.label.as_str().into(),
            })
            .collect(),
    ));
    model.set_sort_column(reference::index_for_column(state.table.column));
    model.set_sort_descending(state.table.descending);
    model.set_filter(filter_id(state.table));
    model.set_selected(state.selected_index);
}

/// What `table_into` reads besides the view.
struct RuntimeView<'a> {
    table: &'a TableState,
    selected_index: i32,
}

/// The chip id of the table's filter.
fn filter_id(table: &TableState) -> i32 {
    (0..5)
        .find(|&id| reference::filter_for_id(id) == table.filter)
        .unwrap_or(0)
}

/// Rebuilds the table from the runtime state and pushes it.
fn push_table(ui: &AppWindow) {
    let model = ui.global::<MaterialReferenceModel>();
    let (view, table, selected_index) = RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        let view = reference::table_view(&rt.rows, &rt.table);
        rt.visible = view.rows.iter().map(|row| row.name.clone()).collect();
        let selected_index = rt
            .selected
            .as_ref()
            .and_then(|name| rt.visible.iter().position(|visible| visible == name))
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1);
        (view, rt.table.clone(), selected_index)
    });
    table_into(
        &model,
        &view,
        &RuntimeView {
            table: &table,
            selected_index,
        },
    );
}

/// The width of the open design's stone, or 0 (the default 7 mm) without a design.
fn design_width_mm(ctx: &Ctx) -> f64 {
    ctx.state
        .borrow()
        .design
        .as_ref()
        .map_or(0.0, |design| stone_width_mm(&design.session.design))
}

/// Rebuilds every material's row from the session's custom materials.
fn rebuild_rows(ctx: &Ctx) {
    let rows = reference::reference_table(&ctx.state.borrow().custom_materials);
    RUNTIME.with(|rt| rt.borrow_mut().rows = rows);
}

/// Pushes the details page of the selected material for the variant the runtime holds. Returns
/// whether there was a page to push.
fn push_detail(ctx: &Ctx, ui: &AppWindow) -> bool {
    let (name, variant_key) = RUNTIME.with(|rt| {
        let rt = rt.borrow();
        (rt.selected.clone(), rt.variant_key.clone())
    });
    let Some(name) = name else {
        return false;
    };
    let width = design_width_mm(ctx);
    let detail = reference::detail(
        &name,
        &ctx.state.borrow().custom_materials,
        width,
        &variant_key,
    );
    let Some(detail) = detail else {
        return false;
    };
    RUNTIME.with(|rt| {
        rt.borrow_mut().variant_keys = detail.variants.iter().map(|v| v.key.clone()).collect();
    });
    detail_into(&ui.global::<MaterialReferenceModel>(), &detail);
    true
}

/// The header chips that are plain figures, joined into one line.
fn chips_line(detail: &MaterialDetail) -> String {
    detail
        .header
        .chips
        .iter()
        .filter(|chip| !chip.highlight)
        .map(|chip| chip.text.as_str())
        .collect::<Vec<_>>()
        .join(MIDDLE_DOT)
}

/// The source-difference notes and the care text of the Durability card, one per line.
fn care_text(detail: &MaterialDetail) -> String {
    let mut lines: Vec<&str> = detail.durability.notes.iter().map(String::as_str).collect();
    if !detail.durability.care.is_empty() {
        lines.push(detail.durability.care.as_str());
    }
    lines.join("\n")
}

/// The specific-gravity line, its basis note and the design's weight line, one per line.
fn weight_text(detail: &MaterialDetail) -> String {
    let mut lines: Vec<&str> = Vec::new();
    if !detail.weight.line.is_empty() {
        lines.push(detail.weight.line.as_str());
    }
    if let Some(note) = detail.weight.basis_note.as_deref() {
        lines.push(note);
    }
    if let Some(design) = detail.weight.design_line.as_deref() {
        lines.push(design);
    }
    lines.join("\n")
}

/// The ray's color, direction and chart paths of the spectrum card.
fn spectrum_series(detail: &MaterialDetail) -> Vec<RefSeries> {
    detail
        .spectrum
        .series
        .iter()
        .map(|series| RefSeries {
            label: series.label.as_str().into(),
            value: SharedString::new(),
            path: series.absorption_path.as_str().into(),
            path_alt: series.transmission_path.as_str().into(),
            swatch: color_of(series.swatch),
            ink: i32::try_from(series.ink).unwrap_or(0),
        })
        .collect()
}

/// One axis tick of a chart.
fn tick_of(text: &str, position: f32) -> RefTick {
    RefTick {
        text: text.into(),
        position,
    }
}

/// Copies one material's details into the model.
fn detail_into(model: &MaterialReferenceModel<'_>, detail: &MaterialDetail) {
    let header = &detail.header;
    model.set_detail_name(detail.name.as_str().into());
    model.set_detail_family(header.family_line.as_str().into());
    model.set_detail_chips(chips_line(detail).into());
    model.set_detail_color_change(header.color_change);
    model.set_detail_swatch_daylight(color_of(header.swatch_daylight));
    model.set_detail_swatch_incandescent(color_of(header.swatch_incandescent));
    ri_into(model, detail);
    model.set_optics_rows(model_of(detail.optics.rows.iter().map(line_of).collect()));
    spectrum_into(model, detail);
    colors_into(model, detail);
    model.set_effects_summary(detail.effects.summary.as_str().into());
    model.set_durability_rows(model_of(
        detail.durability.rows.iter().map(line_of).collect(),
    ));
    model.set_weight_line(weight_text(detail).into());
    model.set_care(care_text(detail).into());
    model.set_source_lines(model_of(
        detail
            .sources
            .iter()
            .flat_map(|group| {
                std::iter::once(RefSourceLine {
                    text: group.title.as_str().into(),
                    heading: true,
                })
                .chain(group.lines.iter().map(|line| RefSourceLine {
                    text: line.as_str().into(),
                    heading: false,
                }))
            })
            .collect(),
    ));
}

/// The refractive-index card: the curves, the axes, the guides and the readout.
fn ri_into(model: &MaterialReferenceModel<'_>, detail: &MaterialDetail) {
    let ri = &detail.ri;
    model.set_ri_series(model_of(
        ri.series
            .iter()
            .map(|series| RefSeries {
                label: series.label.as_str().into(),
                value: series.nd_text.as_str().into(),
                path: series.path.as_str().into(),
                path_alt: SharedString::new(),
                swatch: Color::default(),
                ink: i32::try_from(series.ink).unwrap_or(0),
            })
            .collect(),
    ));
    model.set_ri_y_ticks(model_of(
        ri.y_ticks
            .iter()
            .map(|t| tick_of(&t.text, t.position))
            .collect(),
    ));
    model.set_ri_x_ticks(model_of(
        ri.x_ticks
            .iter()
            .map(|t| tick_of(&t.text, t.position))
            .collect(),
    ));
    model.set_ri_guides(model_of(
        ri.guides.iter().map(|g| tick_of(g.label, g.x)).collect(),
    ));
    model.set_ri_readout(ri.readout.as_str().into());
    model.set_ri_model(ri.model_line.as_str().into());
}

/// The spectrum card: the series, the axis, the summary and the absorption bands.
fn spectrum_into(model: &MaterialReferenceModel<'_>, detail: &MaterialDetail) {
    let spectrum = &detail.spectrum;
    model.set_spectrum_series(model_of(spectrum_series(detail)));
    model.set_spectrum_x_ticks(model_of(
        spectrum
            .x_ticks
            .iter()
            .map(|t| tick_of(&t.text, t.position))
            .collect(),
    ));
    model.set_spectrum_summary(spectrum.summary.as_str().into());
    model.set_spectrum_provenance(spectrum.provenance.as_str().into());
    model.set_spectrum_variant(spectrum.variant_label.as_str().into());
    model.set_transmission_note(spectrum.transmission_tooltip.as_str().into());
    model.set_spectrum_bands(model_of(
        spectrum
            .series
            .iter()
            .flat_map(|series| series.bands.iter())
            .map(|band| RefBand {
                ray: band.ray.as_str().into(),
                center: format!("{:.0} nm", band.center_nm).into(),
                width: band.width_text.as_str().into(),
                peak: band.peak_text.as_str().into(),
                shape: band.shape.as_str().into(),
            })
            .collect(),
    ));
}

/// The color card: the rows under both lights and the color variants.
fn colors_into(model: &MaterialReferenceModel<'_>, detail: &MaterialDetail) {
    model.set_color_rows(model_of(
        detail
            .colors
            .rows
            .iter()
            .map(|row| RefColorRow {
                label: row.label.as_str().into(),
                daylight: swatch_color(row.daylight),
                incandescent: swatch_color(row.incandescent),
                face_up: row.face_up,
            })
            .collect(),
    ));
    model.set_color_note(detail.colors.note.as_str().into());
    model.set_variants(model_of(
        detail
            .variants
            .iter()
            .map(|variant| RefVariant {
                label: variant.label.as_str().into(),
                swatch: color_of(variant.swatch),
                source: variant.source_line.as_str().into(),
                estimated: variant.estimated,
                selected: variant.selected,
            })
            .collect(),
    ));
}

/// Design > Material Reference...: opens the table with the design's material selected.
fn open(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    rebuild_rows(ctx);
    let design_material = ctx
        .state
        .borrow()
        .design
        .as_ref()
        .and_then(|design| design.session.design.material.name.clone());
    RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        if design_material.is_some() {
            rt.selected = design_material;
        }
        rt.variant_key.clear();
    });
    push_table(&ui);
    let model = ui.global::<MaterialReferenceModel>();
    model.set_page(0);
    model.set_is_open(true);
}

/// The Design settings' Details...: opens the details page of `name`.
fn open_for(ctx: &Ctx, name: &str) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    rebuild_rows(ctx);
    let previous = RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        let previous = (rt.selected.take(), std::mem::take(&mut rt.variant_key));
        rt.selected = Some(name.to_owned());
        previous
    });
    push_table(&ui);
    // A name the page does not know leaves the dialog, and the selection, where they were.
    if !push_detail(ctx, &ui) {
        RUNTIME.with(|rt| {
            let mut rt = rt.borrow_mut();
            rt.selected = previous.0;
            rt.variant_key = previous.1;
        });
        push_table(&ui);
        return;
    }
    let model = ui.global::<MaterialReferenceModel>();
    model.set_page(1);
    model.set_is_open(true);
}

/// A column heading: sorts by it, and a second click on the same column reverses the order.
fn sort_clicked(ctx: &Ctx, index: i32) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let column = reference::column_for_index(index);
    RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        if rt.table.column == column {
            rt.table.descending = !rt.table.descending;
        } else {
            rt.table.column = column;
            rt.table.descending = false;
        }
    });
    push_table(&ui);
}

/// A filter chip: narrows the table to it.
fn filter_chosen(ctx: &Ctx, id: i32) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    RUNTIME.with(|rt| rt.borrow_mut().table.filter = reference::filter_for_id(id));
    push_table(&ui);
}

/// The search text changed.
fn query_edited(ctx: &Ctx, text: &str) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    RUNTIME.with(|rt| rt.borrow_mut().table.query = text.to_owned());
    push_table(&ui);
}

/// The name of the visible row `index`.
fn visible_name(index: i32) -> Option<String> {
    let index = usize::try_from(index).ok()?;
    RUNTIME.with(|rt| rt.borrow().visible.get(index).cloned())
}

/// A row was clicked: selects it.
fn row_selected(ctx: &Ctx, index: i32) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let Some(name) = visible_name(index) else {
        return;
    };
    RUNTIME.with(|rt| rt.borrow_mut().selected = Some(name));
    ui.global::<MaterialReferenceModel>().set_selected(index);
}

/// A row was opened: shows its details page.
fn row_opened(ctx: &Ctx, index: i32) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let Some(name) = visible_name(index) else {
        return;
    };
    RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        rt.selected = Some(name);
        rt.variant_key.clear();
    });
    push_detail(ctx, &ui);
    let model = ui.global::<MaterialReferenceModel>();
    model.set_selected(index);
    model.set_page(1);
}

/// Up or Down: moves the selection by `delta` rows, staying inside the table.
fn move_selection(ctx: &Ctx, delta: i32) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<MaterialReferenceModel>();
    let count = RUNTIME.with(|rt| rt.borrow().visible.len());
    if count == 0 {
        return;
    }
    let last = i32::try_from(count - 1).unwrap_or(0);
    let current = model.get_selected();
    let target = if current < 0 {
        if delta < 0 { last } else { 0 }
    } else {
        (current + delta).clamp(0, last)
    };
    row_selected(ctx, target);
}

/// A variant row of the details page: shows that variant's spectrum, colors and sources.
fn variant_selected(ctx: &Ctx, index: i32) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let Some(key) = usize::try_from(index)
        .ok()
        .and_then(|i| RUNTIME.with(|rt| rt.borrow().variant_keys.get(i).cloned()))
    else {
        return;
    };
    RUNTIME.with(|rt| rt.borrow_mut().variant_key = key);
    push_detail(ctx, &ui);
}

/// Registers the dialog's callbacks.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<MaterialReferenceModel>();
    model.on_gem_summary_for(|name| reference::gem_summary_for_name(&name).into());
    model.on_gem_warns_for(|name| reference::gem_cleavage_warns_for_name(&name));
    let c = ctx.clone();
    model.on_open(move || open(&c));
    let c = ctx.clone();
    model.on_open_for(move |name| open_for(&c, &name));
    let c = ctx.clone();
    model.on_close(move || {
        if let Some(ui) = c.ui.upgrade() {
            ui.global::<MaterialReferenceModel>().set_is_open(false);
        }
    });
    let c = ctx.clone();
    model.on_sort_clicked(move |index| sort_clicked(&c, index));
    let c = ctx.clone();
    model.on_filter_chosen(move |id| filter_chosen(&c, id));
    let c = ctx.clone();
    model.on_query_edited(move |text| query_edited(&c, &text));
    let c = ctx.clone();
    model.on_row_selected(move |index| row_selected(&c, index));
    let c = ctx.clone();
    model.on_row_opened(move |index| row_opened(&c, index));
    let c = ctx.clone();
    model.on_move_selection(move |delta| move_selection(&c, delta));
    let c = ctx.clone();
    model.on_variant_selected(move |index| variant_selected(&c, index));
}
