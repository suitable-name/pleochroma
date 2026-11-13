//! Material lookup helpers for the editor (RI-override-aware resolution, the traced-material rule,
//! the nearest-built-in search).
//!
//! The implementation lives in `pleochroma_editor::material_lookup`, shared with the web app; this
//! module re-exports it.

pub(in crate::gui) use pleochroma_editor::material_lookup::{
    EditorMaterialLookup, MATERIAL_MATCH_TOLERANCE, applied_material_guess,
    material_for_refractive_index, material_guess, nearest_built_in_material,
    resolved_gem_material, resolved_gem_material_for_design, traced_gem_material,
    traced_gem_material_at_width, traced_material_for,
};
// Only the identity pins name the candidate list directly; the guess badge uses
// `material_guess`.
#[cfg(test)]
pub(in crate::gui) use pleochroma_editor::material_lookup::material_guess_candidates;
