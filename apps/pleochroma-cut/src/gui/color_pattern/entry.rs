//! The material dialog's Color Pattern section: Add Pattern, Edit Pattern and Remove Pattern.

use super::{
    colors::lch_of_rows,
    context::refresh_material,
    session::{Deps, deps, set_deps},
    store,
    wiring::{Opening, open_session},
};
use crate::{
    MainWindow, SettingsModel, ViewportModel, Zoning,
    bridge::render_thread::RenderContext,
    gui::{
        editor::native_io::ask_write_confirm,
        optics::crystal_optics::gem_material_from_row,
        rough_colour::store::{StoredZoning, load_material_zoning},
        show_toast,
    },
};
use pleochroma::{
    optics::materials::{CrystalSystem, GemMaterial},
    render_setup::PHYSICS_DEFAULT_STONE_WIDTH_MM,
};
use pleochroma_cut_core::material::{
    ColorMode, absorption_bands_from_json,
    pattern::{ColorPattern, PatternAxis, PatternColor, TemplateId, ZoneColor, template},
};
use pleochroma_editor::lch_color::reference_path_mm;
use pleochroma_vault::{db::sqlite::Database, model::material::CustomMaterialRow};
use slint::{ComponentHandle, Model};
use std::sync::{Arc, Mutex, PoisonError};

/// The sentence when Add Pattern is pressed on a material without band rows.
const NEEDS_COLOR: &str = "Pick a color for this material first.";

/// The name of the material the dialog shows (the viewport's material selection).
fn selected_name(ui: &MainWindow) -> String {
    let model = ui.global::<ViewportModel>();
    let index = usize::try_from(model.get_selected_material_index()).unwrap_or(0);
    model
        .get_material_options()
        .row_data(index)
        .map(|name| name.to_string())
        .unwrap_or_default()
}

/// The stone width setting, millimeters (the default width for `0`).
fn stone_width_mm(ui: &MainWindow) -> f64 {
    let width = ui.global::<SettingsModel>().get_stone_width_mm();
    if width > 0.0 {
        f64::from(width)
    } else {
        f64::from(PHYSICS_DEFAULT_STONE_WIDTH_MM)
    }
}

/// The saved row of material `name`.
fn load_row(db: &Arc<Mutex<Database>>, name: &str) -> Option<CustomMaterialRow> {
    db.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_custom_materials()
        .unwrap_or_default()
        .into_iter()
        .find(|row| row.name.eq_ignore_ascii_case(name))
}

/// Whether the material can show two colors: some birefringence and a non-cubic crystal system.
fn two_colors_allowed(row: &CustomMaterialRow, material: &GemMaterial) -> bool {
    row.birefringence.abs() > f32::EPSILON && material.crystal_system != CrystalSystem::Cubic
}

/// The pattern an Add Pattern starts from: Uniform, its base the material's own band color.
fn uniform_from_bands(bands: Vec<[f32; 3]>, path_mm: f64) -> ColorPattern {
    let mut pattern = template(TemplateId::Uniform, PatternAxis::TABLE_NORMAL);
    let lch = lch_of_rows(&bands, path_mm);
    pattern.base.color = PatternColor::Single(ZoneColor::solved(lch, bands));
    pattern
}

/// The pattern of a material that has Rough Color wizard zones but no stored pattern.
fn pattern_from_zones(stored: &StoredZoning, path_mm: f64) -> ColorPattern {
    ColorPattern::from_zoned(&stored.zoning, stored.relative_to_stone, |rows| {
        lch_of_rows(rows, path_mm)
    })
}

/// What the editor opens on for `name`, or the sentence for why it cannot open.
fn opening_for(ui: &MainWindow, deps: &Deps, name: &str, adding: bool) -> Result<Opening, String> {
    let Some(row) = load_row(&deps.db, name) else {
        return Err("Save the material first, then add a pattern.".to_owned());
    };
    if row
        .color_recipe_json
        .as_deref()
        .and_then(ColorMode::from_json)
        .is_some_and(|mode| mode.is_physics())
    {
        return Err("This material's color cannot have a pattern.".to_owned());
    }
    let width = stone_width_mm(ui);
    let path_mm = reference_path_mm(Some(width));
    let material = gem_material_from_row(&row);
    let allowed = two_colors_allowed(&row, &material);
    let base_material = RenderContext::lock(&deps.render_ctx)
        .custom_materials
        .iter()
        .find(|candidate| candidate.name.eq_ignore_ascii_case(name))
        .cloned()
        .unwrap_or(material);
    let (pattern, import_note) = if adding {
        let bands = row
            .absorption_bands_json
            .as_deref()
            .and_then(absorption_bands_from_json)
            .filter(|bands| !bands.is_empty())
            .ok_or_else(|| NEEDS_COLOR.to_owned())?;
        (uniform_from_bands(bands, path_mm), None)
    } else {
        let db = deps.db.lock().unwrap_or_else(PoisonError::into_inner);
        match store::load_pattern(&db, name) {
            Ok(Some(pattern)) => (pattern, None),
            Ok(None) => match load_material_zoning(&db, name) {
                Ok(Some(stored)) => (
                    pattern_from_zones(&stored, path_mm),
                    Some("These zones come from the Rough Color Wizard.".to_owned()),
                ),
                Ok(None) => return Err("This material has no pattern to edit.".to_owned()),
                Err(error) => return Err(format!("{error:#}")),
            },
            Err(error) => return Err(format!("{error:#}")),
        }
    };
    Ok(Opening {
        material_name: row.name,
        stone_apply: None,
        targets_design: false,
        pattern,
        import_note,
        stone_width_mm: width,
        base_material,
        two_colors_allowed: allowed,
        own: None,
    })
}

/// Opens the editor for the dialog's selected material.
fn open_for_selected(ui: &MainWindow, adding: bool) {
    let Some(deps) = deps() else {
        return;
    };
    let name = selected_name(ui);
    match opening_for(ui, &deps, &name, adding) {
        Ok(opening) => open_session(ui, &deps, opening),
        Err(sentence) => show_toast(ui, &sentence, "warning"),
    }
}

/// Removes the stored pattern of `name` after the cutter confirmed.
fn remove_confirmed(ui: &MainWindow, deps: &Deps, name: &str) {
    let result = {
        let guard = deps.db.lock().unwrap_or_else(PoisonError::into_inner);
        store::remove_pattern(&guard, name)
    };
    if let Err(error) = result {
        show_toast(ui, &format!("{error:#}"), "error");
        return;
    }
    refresh_material(&deps.render_ctx, &deps.db, name, stone_width_mm(ui));
    ui.global::<ViewportModel>()
        .invoke_selected_material_changed_for_editor_prefill();
    show_toast(
        ui,
        &format!("Removed the color pattern of '{name}'."),
        "success",
    );
}

/// Wires the section's three buttons and remembers the vault and render context the editor saves
/// into.
pub fn setup_pattern_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    db: &Arc<Mutex<Database>>,
) {
    set_deps(Deps {
        db: Arc::clone(db),
        render_ctx: Arc::clone(render_ctx),
    });
    let zoning = ui.global::<Zoning>();
    let weak = ui.as_weak();
    zoning.on_add_pattern(move || {
        if let Some(ui) = weak.upgrade() {
            open_for_selected(&ui, true);
        }
    });
    let weak = ui.as_weak();
    zoning.on_edit_pattern(move || {
        if let Some(ui) = weak.upgrade() {
            open_for_selected(&ui, false);
        }
    });
    let weak = ui.as_weak();
    zoning.on_remove_pattern(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Some(deps) = deps() else {
            return;
        };
        let name = selected_name(&ui);
        let message =
            format!("Remove the color pattern of '{name}'? The material keeps its base color.");
        ask_write_confirm(
            &ui,
            "Remove Color Pattern",
            message,
            "Remove",
            None,
            move |ui| remove_confirmed(ui, &deps, &name),
        );
    });
}
