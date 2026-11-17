//! The window's buttons that act on the rest of the app: Preview As, Duplicate as Custom and
//! Use for Design. Each one goes through the path the Stone tab and the Material Editor already
//! use, so undo, the preview badge and the toasts behave exactly as they do there.

use crate::{
    EditorModel, MainWindow, ViewportModel,
    gui::{
        render::material::{ExplicitApply, apply_explicit_material},
        show_toast,
    },
};
use pleochroma_editor::material_reference::MaterialDetail;
use slint::{ComponentHandle, Model, ModelRc, SharedString};

/// Whether an editable design is open on the viewport: the only case in which Use for Design
/// can change anything.
#[must_use]
pub(super) fn design_is_open(ui: &MainWindow) -> bool {
    ui.global::<EditorModel>().get_has_design()
        && ui.global::<ViewportModel>().get_shows_editor_design()
}

/// The stone width of the open design in millimeters, 0 when no design is open or its girdle
/// diameter is not a plain positive number.
#[must_use]
pub(super) fn design_width_mm(ui: &MainWindow) -> f64 {
    if !design_is_open(ui) {
        return 0.0;
    }
    ui.global::<EditorModel>()
        .get_girdle_diameter_mm()
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|width| width.is_finite() && *width > 0.0)
        .unwrap_or(0.0)
}

/// The material the Material Reference opens on when none is named: the open design's own
/// material, else the one the viewport shows.
#[must_use]
pub(super) fn current_material(ui: &MainWindow) -> String {
    let viewport = ui.global::<ViewportModel>();
    let design = viewport.get_design_material_name();
    if design_is_open(ui) && !design.is_empty() {
        return design.to_string();
    }
    viewport.get_traced_material_name().to_string()
}

/// "Preview As": shows `name` in the viewport without touching the design. A library design
/// takes it as its view selection, as the Stone tab's material pick does.
pub(super) fn preview_as(ui: &MainWindow, name: &str) {
    let viewport = ui.global::<ViewportModel>();
    let name = SharedString::from(name);
    if viewport.get_shows_editor_design() {
        viewport.invoke_preview_material_chosen(name);
    } else {
        viewport.invoke_library_material_chosen(name);
    }
}

/// The entries of the Material Editor's Load Preset Template picker that start from a built-in
/// material of the same name (the picker's "Custom" entry starts from nothing).
const EDITOR_TEMPLATES: [&str; 11] = [
    "Cubic Zirconia",
    "Diamond",
    "Emerald",
    "Quartz",
    "Ruby",
    "Sapphire",
    "Spinel",
    "Synthetic Moissanite",
    "Tanzanite",
    "Topaz",
    "Zircon",
];

/// The Load Preset Template entry that starts from the material `name`, if the picker has one.
#[must_use]
fn editor_template_for(name: &str) -> Option<&'static str> {
    EDITOR_TEMPLATES
        .iter()
        .copied()
        .find(|template| template.eq_ignore_ascii_case(name.trim()))
}

/// "Duplicate as Custom...": opens the Material Editor with the Load Preset Template entry of the
/// material selected and loaded, as if it had been picked. A material the picker has no entry
/// for opens the editor as usual, with a note to pick the closest template.
pub(super) fn duplicate_as_custom(ui: &MainWindow, name: &str) {
    let viewport = ui.global::<ViewportModel>();
    if let Some(template) = editor_template_for(name) {
        viewport.set_material_editor_template(template.into());
        viewport.set_material_editor_open(true);
        return;
    }
    viewport.set_material_editor_template(SharedString::default());
    viewport.set_material_editor_open(true);
    show_toast(
        ui,
        &format!(
            "{name} has no Load Preset Template. Pick the closest one to start from its values."
        ),
        "info",
    );
}

/// The row of the Stone tab's color list that shows `label`, compared without regard to case.
fn color_row_of(options: &ModelRc<SharedString>, label: &str) -> Option<i32> {
    options
        .iter()
        .position(|option| option.as_str().eq_ignore_ascii_case(label))
        .and_then(|index| i32::try_from(index).ok())
}

/// "Use for Design": sets the open design's color to variant `index` of `detail`, the same way
/// a tile of the Stone tab's color list does (`EditorModel.apply_design_material`, one undo
/// step). When the design is made of another material, that material is chosen first.
pub(super) fn use_variant(ui: &MainWindow, detail: &MaterialDetail, index: usize) {
    if !design_is_open(ui) {
        show_toast(ui, "Open a design to use this color for it.", "info");
        return;
    }
    let Some(variant) = detail.variants.get(index) else {
        return;
    };
    let viewport = ui.global::<ViewportModel>();
    let same_material = viewport
        .get_design_material_name()
        .as_str()
        .eq_ignore_ascii_case(&detail.name);
    if !same_material && apply_explicit_material(ui, &detail.name, None) == ExplicitApply::Busy {
        return;
    }
    let editor = ui.global::<EditorModel>();
    // Row 0 is the material's own color; the variants follow under their labels.
    let row = if index == 0 {
        Some(0)
    } else {
        color_row_of(&editor.get_body_color_options(), &variant.label)
    };
    let Some(row) = row else {
        show_toast(
            ui,
            "This color is not offered for the design's material.",
            "info",
        );
        return;
    };
    editor.set_body_color_index(row);
    editor.invoke_apply_design_material(
        editor.get_material_combo_index(),
        editor.get_ri_override_text(),
        row,
    );
}
