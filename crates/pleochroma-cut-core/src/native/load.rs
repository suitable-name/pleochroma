//! Shared helpers for loading a design file: [`MaterialResolution`] and the `restore` helpers.
//!
//! The helpers put per-tier state from a file's tier tables onto a fresh
//! [`crate::design::Design`] (see [`super::design_from_file`]).

use std::fmt;

mod restore;

pub(super) use restore::{
    apply_tier_frosting, apply_tier_ids_and_targets, apply_tier_relations,
    cheater_offsets_from_native, material_resolution_of, raw_tier_frosting_from_native,
    raw_tier_ids_from_native, raw_tier_relations_from_native, raw_tier_targets_from_native,
    restore_concave_tiers, tier_from_table_with_full_geometry, tier_notes_from_native,
};

/// Whether the loaded design's [`crate::material::MaterialSelection::name`] matches a
/// built-in [`pleochroma::optics::materials::GemMaterial`] by exact name.
///
/// This crate has no custom-material catalogue, so the check covers the built-in presets
/// only. [`Self::Unresolved`] is therefore a heuristic: the caller's own catalogue may still
/// resolve the name (see [`crate::material::MaterialSelection::resolve`]). The variant lets a
/// caller without that catalogue warn, instead of silently falling back to
/// [`pleochroma::optics::materials::GemMaterial::diamond`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialResolution {
    /// `design.material.name` is `None` -- nothing to resolve.
    NoneSelected,
    /// `design.material.name` matches a built-in preset by exact name.
    Known,
    /// `design.material.name` matches no built-in preset. It may be a custom material that
    /// the caller's catalogue supplies.
    Unresolved,
}

impl fmt::Display for MaterialResolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoneSelected => write!(f, "no material selected"),
            Self::Known => write!(f, "material recognized"),
            Self::Unresolved => write!(
                f,
                "material not recognized as a built-in preset -- it may render as \
                 Diamond until a matching custom material is restored"
            ),
        }
    }
}
