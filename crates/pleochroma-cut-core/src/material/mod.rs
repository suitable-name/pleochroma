//! The material model of a design: built-in and custom materials, their optical and
//! render-physics data, the specific-gravity table behind the carat estimate, and the gem
//! reference table (hardness, toughness, cleavage, care).
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
//! The table covers all 40 built-in materials of `GemMaterial::all_materials`; each has an
//! optical entry, so no SG row offers a species that cannot be rendered.
//! [`MaterialSelection::specific_gravity_override`] lets a user enter the SG of a species this
//! table does not cover, such as the garnet group as a whole.
//!
//! Each representative figure and range is cited in a comment beside its row, from the Handbook
//! of Mineralogy (measured density), the GIA gem encyclopedia, the International Gem Society
//! property pages and its table of specific-gravity values, webmineral.com, Gems & Gemology
//! Winter 1997 (Table 1, for YAG and GGG), the Schott optical glass datasheets (for the two
//! glasses) and Gems & Gemology Summer 2026 (the measured range of the glass-ceramic Nanosital). The garnet rows hold the values of the gem (Pyrope 3.74, Almandine 4.19), not the
//! pure end-members (Handbook of Mineralogy: Pyrope 3.582, Almandine 4.318).
//!
//! Three rows are derived, not printed by any source: Morganite (the midpoint of the GIA range
//! 2.80-2.91), the Color-Change Garnet (the midpoint of the range merged from two Gems &
//! Gemology studies) and Nanosital (the mean of the three faceted stones of the Gems & Gemology
//! 2026 study, inside its measured 2.85-3.42). They are marked [`SgBasis::Derived`] and carry a basis note; every other
//! row is [`SgBasis::Cited`]. A material with a single cited value stores it as both ends of its
//! range. For Opal the sources differ (webmineral 1.9-2.3; the IGS table prints 1.25-2.23
//! including porous material); webmineral is kept.
//!
//! # Gem data
//!
//! [`built_in_gem_data`] holds what a cutter reads beside the optics: Mohs hardness (a range),
//! toughness (the GIA word scale), cleavage, parting, fracture and short care notes for each of
//! the 40 built-in materials. Keys are the exact `GemMaterial::name` strings, like the SG table.
//!
//! - **Sources.** Every stored value carries the sources it comes from ([`Cited`], rendered
//!   by [`GemSource::citation`] and [`GemSource::url`]). Mohs and care come from the GIA
//!   gem-encyclopedia care pages, where GIA has no page from the GIA Gem Cleaning and Display
//!   Chart (rev. Oct 2018), and for the laboratory-grown materials from Gems & Gemology,
//!   Winter 1997, Table 1. Cleavage, parting and fracture come from the Handbook of
//!   Mineralogy, which gives Miller indices; webmineral and Wikipedia are used only where the
//!   Handbook has no page (opal, labradorite and the synthetics). The glasses use Schott's
//!   optical-glass datasheets, and care text comes from the International Gem Society only
//!   where GIA has no page. Nanosital, a glass-ceramic, takes its fracture and care from the
//!   Gems & Gemology 2026 study and its Mohs figure from the manufacturer.
//! - **Differing sources.** The GIA care page is shown for toughness and Mohs, widened by the
//!   Handbook of Mineralogy where it differs (spinel 7.5-8). A disagreement is kept as a
//!   [`SourceNote`] ("Sources differ: ...") and never resolved silently. A lone outlier is not
//!   used.
//! - **Typical values.** The figures describe the species, not a single stone. A value taken
//!   over from the species of a variety or from end-members ([`GemBasis::Inferred`]) and a
//!   value computed from cited figures ([`GemBasis::Derived`]) are shown with their basis
//!   note.
//! - **No source, no value.** A value no gemological or mineralogical source gives is left
//!   out rather than guessed: the toughness of the glasses, Benitoite, Titanite and Nanosital,
//!   the cleavage of Nanosital, and the cleavage and fracture of GGG, are empty and read "Not
//!   graded" or "Not recorded".
//!   Retail pages and search snippets are not sources.
//! - **Durability only.** The table holds hardness, toughness, cleavage, parting, fracture and
//!   care text. Remarks that belong to the physics-color side of the app are never stored
//!   here.

mod catalogue;
pub mod color;
mod custom_physics;
mod gem_data;
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
pub use gem_data::{
    CareTopic, Cited, Cleavage, CleavageQuality, Confidence, CrystalForm, Fracture, GemBasis,
    GemData, GemField, GemSource, MaterialOrigin, SourceNote, Toughness, ToughnessRange,
    built_in_gem_data, cleavage_summary, fracture_summary, mohs_label, parting_summary,
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
pub use specific_gravity::{SgBasis, SpecificGravity, built_in_specific_gravity};
pub use variant_choice::{CUSTOM_VARIANT_KEY, CUSTOM_VARIANT_LABEL, VariantChoice};
