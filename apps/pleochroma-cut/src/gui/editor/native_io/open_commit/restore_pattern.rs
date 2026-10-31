//! Restoring the color pattern a design file's material snapshot carries.
//!
//! [`restore_pattern`] runs from `restore_custom_material` after the snapshot's plain material
//! is registered in the session. A material the library does not have is created there from
//! the snapshot, together with its pattern, zones and base color (one transaction,
//! `gui::color_pattern::save_pattern`); a material the library already has keeps the library's
//! pattern, and a file pattern that differs is reported.

use crate::{
    bridge::render_thread::RenderContext,
    gui::{
        color_pattern::{load_pattern, save_pattern},
        rough_colour::store::with_stored_zoning,
    },
};
use pleochroma_cut_core::{
    material::{
        decode_pattern, encode_pattern, inclusion_families_from_json, inclusion_families_to_json,
        per_axis_dispersion_of_snapshot, per_axis_dispersion_to_json, structure_spec_from_json,
        structure_spec_to_json,
    },
    native::{
        CustomMaterialSnapshot, dispersion_model_from_dto, dispersion_model_to_json,
        gem_material_from_custom_snapshot,
    },
};
use pleochroma_vault::db::sqlite::{CustomMaterialParams, Database};
use std::{
    cell::RefCell,
    sync::{Arc, Mutex, PoisonError, Weak},
};

/// The sentence for a file pattern that is not used because the library's own pattern differs.
const DIFFERS: &str =
    "This design's material pattern differs from your library's; your library's is used.";

/// The sentence for a file pattern this build cannot read (damaged, or written by a newer build).
const UNREADABLE: &str =
    "This design's material pattern cannot be read by this version; the plain material is used.";

/// The sentence for a pattern that could not be stored in the library.
const NOT_STORED: &str =
    "This design's material pattern could not be restored; the plain material is used.";

thread_local! {
    /// The vault the main window was built with, held weakly so it never keeps the database
    /// connection open past the window (`gui::window_release`). Set by [`register_vault`].
    static VAULT: RefCell<Weak<Mutex<Database>>> = const { RefCell::new(Weak::new()) };
}

/// Remembers the main window's vault for [`restore_pattern`] (the open callbacks carry none).
pub(in crate::gui::editor::native_io) fn register_vault(db: &Arc<Mutex<Database>>) {
    VAULT.with(|cell| *cell.borrow_mut() = Arc::downgrade(db));
}

/// `value` as the `f32` the library row stores.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the library stores optics as f32; the file's f64 text came from an f32"
)]
const fn narrow(value: f64) -> f32 {
    value as f32
}

/// Creates the library row of custom material `name` from `snapshot`: its optics only. The base
/// color is the pattern's, written by the pattern save that follows.
fn create_row(db: &Database, name: &str, snapshot: &CustomMaterialSnapshot) -> anyhow::Result<()> {
    let model_json = snapshot
        .dispersion_model
        .as_ref()
        .and_then(dispersion_model_from_dto)
        .map(|model| dispersion_model_to_json(&model));
    // The curves, families and structure the file carries, as the canonical text the columns
    // hold; one this build cannot read is left out, like an unreadable dispersion model.
    let per_axis_json = per_axis_dispersion_of_snapshot(snapshot)
        .and_then(|curves| per_axis_dispersion_to_json(&curves));
    let inclusions_json = snapshot
        .inclusions
        .as_deref()
        .and_then(inclusion_families_from_json)
        .and_then(|families| inclusion_families_to_json(&families));
    let structure_json = snapshot
        .structure
        .as_deref()
        .and_then(structure_spec_from_json)
        .and_then(|spec| structure_spec_to_json(&spec));
    let text = |value: &str| (!value.is_empty()).then_some(value.to_string());
    let crystal_system = text(&snapshot.crystal_system);
    let optical_character = text(&snapshot.optical_character);
    db.save_custom_material(&CustomMaterialParams {
        name,
        refractive_index: narrow(snapshot.mean_ri),
        dispersion: narrow(snapshot.dispersion_delta),
        birefringence: narrow(snapshot.birefringence_delta),
        absorption_rgb: [0.0; 3],
        crystal_system: crystal_system.as_deref(),
        optical_character: optical_character.as_deref(),
        biaxial_delta_beta_alpha: snapshot.biaxial_delta_beta_alpha.map(narrow),
        per_axis_dispersion_json: per_axis_json.as_deref(),
        specific_gravity: snapshot.specific_gravity.map(narrow),
        color_recipe_json: None,
        dispersion_model_json: model_json.as_deref(),
        absorption_bands_json: None,
        c_axis_tilt_deg: snapshot.c_axis_tilt_deg.map(narrow),
        c_axis_azimuth_deg: snapshot.c_axis_azimuth_deg.map(narrow),
        c_axis_second_azimuth_deg: snapshot.c_axis_second_azimuth_deg.map(narrow),
        inclusion_families_json: inclusions_json.as_deref(),
        structure_json: structure_json.as_deref(),
    })
}

/// Restores the color pattern of the snapshot of custom material `name`, and installs the
/// stored zones on the session's copy of the material. `design_width_mm` is the design's girdle
/// width (a relative pattern resolves at it; none or not positive: the render's stone width).
///
/// Returns the sentence to show when the file's pattern was not used as it is: the library has
/// the material with a different pattern (the library's is kept), or the file's pattern cannot
/// be read or stored (the plain material stays). `None` when the snapshot has no pattern, when
/// no vault is registered, and when everything went as saved.
pub(super) fn restore_pattern(
    render_ctx: &Arc<Mutex<RenderContext>>,
    name: &str,
    snapshot: &CustomMaterialSnapshot,
    design_width_mm: Option<f64>,
) -> Option<&'static str> {
    let text = snapshot.pattern.as_deref()?;
    let vault = VAULT.with(|cell| cell.borrow().upgrade())?;
    let render_width_mm = RenderContext::lock(render_ctx).stone_width_mm;
    let width_mm = design_width_mm
        .filter(|width| width.is_finite() && *width > 0.0)
        .unwrap_or_else(|| f64::from(render_width_mm));

    let guard = vault.lock().unwrap_or_else(PoisonError::into_inner);
    let exists = guard
        .get_custom_materials()
        .ok()?
        .iter()
        .any(|row| row.name.eq_ignore_ascii_case(name));
    let mut sentence = None;
    match decode_pattern(text) {
        Err(_) => sentence = Some(UNREADABLE),
        Ok(pattern) if exists => {
            let ours = load_pattern(&guard, name)
                .ok()
                .flatten()
                .and_then(|stored| encode_pattern(&stored).ok());
            if ours != encode_pattern(&pattern).ok() {
                sentence = Some(DIFFERS);
            }
        }
        Ok(pattern) => {
            let stored = create_row(&guard, name, snapshot)
                .and_then(|()| save_pattern(&guard, name, &pattern, width_mm));
            if let Err(error) = stored {
                tracing::warn!("The pattern of '{name}' was not restored: {error:#}");
                // The row was only created for the pattern: leave no half-restored material.
                let _ = guard.delete_custom_material(name);
                sentence = Some(NOT_STORED);
            }
        }
    }
    // The session's copy of the material gets the library's zones (none when nothing was stored).
    let gem = with_stored_zoning(
        &guard,
        gem_material_from_custom_snapshot(name, snapshot),
        f64::from(render_width_mm),
    );
    drop(guard);
    if gem.zoning.is_some() {
        let mut ctx = RenderContext::lock(render_ctx);
        let materials = Arc::make_mut(&mut ctx.custom_materials);
        if let Some(entry) = materials
            .iter_mut()
            .find(|material| material.name.eq_ignore_ascii_case(name))
        {
            *entry = gem;
        }
        drop(ctx);
    }
    sentence
}
