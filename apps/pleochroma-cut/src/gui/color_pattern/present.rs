//! The Color Pattern section of the material dialog: what `Zoning` shows about the selected
//! material's pattern (the summary, the zone strip and which buttons are offered).

use super::{
    colors::{SwatchStone, pattern_swatches},
    model::pattern_summary,
    store::load_pattern,
};
use crate::{
    MainWindow, SettingsModel, Zoning,
    gui::rough_colour::{
        material_zones::{built_in_zoning, rows_for_material},
        store::load_material_zoning,
    },
};
use pleochroma_cut_core::material::{ColorMode, pattern::PatternColor};
use pleochroma_editor::lch_color::reference_path_mm;
use pleochroma_vault::db::sqlite::Database;
use slint::{Color, ComponentHandle, ModelRc, VecModel};
use std::sync::{Arc, Mutex, PoisonError};

/// `Zoning.pattern_state`: no pattern (Add Pattern is offered).
pub(super) const STATE_NONE: i32 = 0;
/// A stored pattern, or zones of the Rough Color wizard (Edit and Remove).
pub(super) const STATE_HAS_PATTERN: i32 = 1;
/// A built-in material's pattern (read-only).
pub(super) const STATE_BUILT_IN: i32 = 2;
/// The material's color is not band rows yet (pick a color first).
pub(super) const STATE_NEEDS_COLOR: i32 = 3;
/// Not offered (the material's color is an active physics recipe): the section is hidden.
pub(super) const STATE_HIDDEN: i32 = 4;

/// What the section shows for one material.
struct Section {
    state: i32,
    summary: String,
    swatches: Vec<[u8; 3]>,
    base_locked: bool,
    show_relative: bool,
}

impl Section {
    const fn plain(state: i32) -> Self {
        Self {
            state,
            summary: String::new(),
            swatches: Vec::new(),
            base_locked: false,
            show_relative: false,
        }
    }
}

/// The swatches of the zones `rows_for_material` lists (wizard zones and built-ins).
fn zone_list_swatches(db: &Database, name: &str, width: f32) -> Vec<[u8; 3]> {
    rows_for_material(db, name, width)
        .into_iter()
        .map(|entry| entry.swatch)
        .collect()
}

/// Works out the section for material `name` at the stone width `width` (millimeters).
fn section_for(db: &Database, name: &str, width: f32) -> Section {
    let row = db
        .get_custom_materials()
        .unwrap_or_default()
        .into_iter()
        .find(|row| row.name.eq_ignore_ascii_case(name));
    let Some(row) = row else {
        // A built-in: only Ametrine has zones, and they are shown read-only.
        return if built_in_zoning(name).is_some() {
            Section {
                state: STATE_BUILT_IN,
                summary: "Built-in color zones. Save a copy under a new name to change them."
                    .to_owned(),
                swatches: zone_list_swatches(db, name, width),
                base_locked: false,
                show_relative: false,
            }
        } else {
            Section::plain(STATE_HIDDEN)
        };
    };
    // Only an active physics recipe owns the color; a stored Fantasy mode is a plain color.
    if row
        .color_recipe_json
        .as_deref()
        .and_then(ColorMode::from_json)
        .is_some_and(|mode| mode.is_physics())
    {
        return Section::plain(STATE_HIDDEN);
    }
    let pattern = load_pattern(db, name).ok().flatten();
    if let Some(pattern) = pattern {
        let path = reference_path_mm(Some(f64::from(width)));
        return Section {
            state: STATE_HAS_PATTERN,
            summary: pattern_summary(&pattern),
            // The library row's own material is not at hand here: a variant zone shows its
            // unpolarised color at the face-up path of this stone width.
            swatches: pattern_swatches(
                &pattern,
                &SwatchStone {
                    path_mm: path,
                    width_mm: f64::from(width),
                    host: None,
                },
            )
            .into_iter()
            .map(|swatch| swatch.blend)
            .collect(),
            // Two or three colors, or a variant: edited in the pattern editor only.
            base_locked: !matches!(pattern.base.color, PatternColor::Single(_)),
            show_relative: false,
        };
    }
    if let Ok(Some(stored)) = load_material_zoning(db, name) {
        let swatches = zone_list_swatches(db, name, width);
        return Section {
            state: STATE_HAS_PATTERN,
            summary: format!(
                "Zones from the Rough Color Wizard \u{b7} {} \u{b7} {}",
                zones_text(stored.zoning.zones.len() + 1),
                if stored.relative_to_stone {
                    "scales with stone size"
                } else {
                    "fixed in mm"
                }
            ),
            swatches,
            base_locked: false,
            show_relative: true,
        };
    }
    let has_bands = row
        .absorption_bands_json
        .as_deref()
        .and_then(pleochroma_cut_core::material::absorption_bands_from_json)
        .is_some_and(|bands| !bands.is_empty());
    Section::plain(if has_bands {
        STATE_NONE
    } else {
        STATE_NEEDS_COLOR
    })
}

/// "1 zone" or "3 zones".
fn zones_text(count: usize) -> String {
    format!("{count} zone{}", if count == 1 { "" } else { "s" })
}

/// Fills the dialog's Color Pattern section for the selected material `name`.
pub fn present_section(ui: &MainWindow, db: &Arc<Mutex<Database>>, name: &str) {
    let width = ui.global::<SettingsModel>().get_stone_width_mm();
    let section = section_for(
        &db.lock().unwrap_or_else(PoisonError::into_inner),
        name,
        width,
    );
    let zoning = ui.global::<Zoning>();
    zoning.set_pattern_state(section.state);
    zoning.set_pattern_summary(section.summary.into());
    zoning.set_pattern_swatches(ModelRc::new(VecModel::from(
        section
            .swatches
            .into_iter()
            .map(|rgb| Color::from_rgb_u8(rgb[0], rgb[1], rgb[2]))
            .collect::<Vec<_>>(),
    )));
    zoning.set_base_color_locked(section.base_locked);
    zoning.set_base_color_locked_note(if section.base_locked {
        "Edit this color in the pattern editor.".into()
    } else {
        "".into()
    });
    zoning.set_show_relative_to_stone(section.show_relative);
}
