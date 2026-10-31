//! Keeps the render context in step with a stored pattern.
//!
//! Storing or removing a pattern rewrites the material's zoning row in the vault. The render
//! context holds the material list as built at startup (or at the last material save), so the
//! material is rebuilt from its row with the stored zones installed, and put in place of the old
//! one. The live render restarts from the new material.

use super::follow;
use crate::{
    bridge::render_thread::RenderContext,
    gui::{optics::crystal_optics::gem_material_from_row, rough_colour::store::with_stored_zoning},
};
use pleochroma_vault::db::sqlite::Database;
use std::sync::{Arc, Mutex, PoisonError};

/// Rebuilds the saved material `name` from its row with its stored zones, at `stone_width_mm`, and
/// puts it in the render context's list (and in the override that names it).
pub(super) fn refresh_material(
    render_ctx: &Arc<Mutex<RenderContext>>,
    db: &Arc<Mutex<Database>>,
    name: &str,
    stone_width_mm: f64,
) {
    let row = db
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_custom_materials()
        .unwrap_or_default()
        .into_iter()
        .find(|row| row.name.eq_ignore_ascii_case(name));
    let Some(row) = row else {
        return;
    };
    let material = {
        let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
        with_stored_zoning(&guard, gem_material_from_row(&row), stone_width_mm)
    };
    let mut ctx = RenderContext::lock(render_ctx);
    let materials = Arc::make_mut(&mut ctx.custom_materials);
    if let Some(at) = materials
        .iter()
        .position(|candidate| candidate.name.eq_ignore_ascii_case(name))
    {
        materials[at] = material.clone();
    } else {
        return;
    }
    if ctx
        .material_override
        .as_ref()
        .is_some_and(|current| current.name.eq_ignore_ascii_case(name))
    {
        ctx.material_override = Some(material);
    }
    ctx.dirty = true;
    drop(ctx);
    // The stored zones were installed on the material's own axis; a pattern that follows the
    // Crystal Axis takes the design's axis.
    follow::axis_applied(render_ctx);
}
