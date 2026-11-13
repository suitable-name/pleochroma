//! The material model of a design: built-in and custom materials, their optical and
//! render-physics data, and the specific-gravity table behind the carat estimate.
//!
//! Specific gravity feeds [`crate::yield_metrics`]' carat estimate. It is kept apart from
//! volumetric yield, which needs no material data.
//!
//! # Provenance
//!
//! Neither `pleochroma::optics::materials` (dispersion, absorption, birefringence) nor the
//! `.asc` catalogue carries specific gravity, so the SG table is its own data. Unlike the
//! single-paper dispersion fits, SG has no primary literature for most gem species; the
//! values are textbook constants from the Handbook of Mineralogy and the International Gem
//! Society property tables. Keys are the exact `GemMaterial::name` strings, so a caller
//! holding one of those names can look its SG up directly.
//!
//! # Resolving a selection to a real material and refractive index
//!
//! [`MaterialSelection::resolve`] turns a preset name plus overrides into a
//! `pleochroma::optics::materials::GemMaterial`, its refractive index at the sodium D line
//! (`n_d`), and the critical angle that follows from it. The lookup goes through a
//! [`MaterialLookup`] trait object, so this crate does not depend on `pleochroma-vault`;
//! [`BuiltinMaterials`] covers the built-ins only.
//!
//! # Ranges
//!
//! Unlike refractive index, SG often varies substantially with substitution across a
//! solid-solution series. Two entries carry a cited range for that reason: **Zircon**
//! (crystalline "high" ~4.6-4.7; metamict "low" down to ~3.90) and **Tourmaline** (the
//! elbaite entry the optics model uses is one composition in the dravite/schorl family,
//! and SG rises with iron). [`SpecificGravity::representative`] is the midpoint of the
//! cited spread, not an average of the family's theoretical extremes.
//!
//! The table covers fourteen species, a subset of `GemMaterial::all_materials`. Optical
//! entries without an SG row (the individual garnets among them) are left out on purpose:
//! an SG row with no optical entry would let a picker offer a species it cannot render.
//! [`MaterialSelection::specific_gravity_override`] lets a user enter the SG of a species
//! this table does not cover.

mod catalogue;
pub mod color;
mod custom_physics;
mod library_row;
pub mod pattern;
mod selection;
mod specific_gravity;
mod variant_choice;

#[cfg(test)]
mod tests;

pub use catalogue::{MaterialCatalogue, MaterialEntry, MaterialKind};
pub use color::{ActiveColor, ColorMode, RecipeHistory};
pub use custom_physics::{
    CustomStructure, PerAxisDispersion, custom_structure_from_json, custom_structure_to_json,
    inclusion_families_from_json, inclusion_families_to_json, per_axis_dispersion_from_json,
    per_axis_dispersion_of_material, per_axis_dispersion_of_snapshot, per_axis_dispersion_to_json,
    structure_spec_from_json, structure_spec_to_json, with_custom_physics, with_custom_structure,
};
pub use library_row::{
    LibraryMaterial, RayBandRows, absorption_bands_from_json, absorption_bands_to_json,
    absorption_ray_bands_from_json, absorption_ray_bands_to_json, biaxial_character_from_deltas,
    crystal_system_from_name, crystal_system_name, is_biaxial, optical_character_from_name,
    optical_character_name, with_default_orientation, with_library_bands, with_library_ray_bands,
};
pub use pattern::{
    ColorPattern, ColorRef, ColorSlot, PATTERN_FORMAT_VERSION, PatternAxis, PatternColor,
    PatternError, PatternZone, TemplateId, ZoneColor, decode_pattern, encode_pattern,
};
pub use selection::{
    BuiltinMaterials, MaterialLookup, MaterialSelection, ResolvedMaterial,
    built_in_material_by_exact_name, built_in_refractive_index,
};
pub use specific_gravity::{SpecificGravity, built_in_specific_gravity};
pub use variant_choice::{CUSTOM_VARIANT_KEY, CUSTOM_VARIANT_LABEL, VariantChoice};
