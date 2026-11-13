//! The conversions behind the `.psdd` design file (Pleochroma Suite Design Document), and the
//! catalogue `.asc` text a save stores next to it.
//!
//! The on-disk shape (schema, TOML encode/decode, path rules) lives in
//! [`pleochroma_formats::native`], which has no editor types. This module holds the parts that
//! need this crate's types: [`convert`]'s free functions between the on-disk mirror types and
//! [`crate::design::Design`], [`crate::preform::PreformSpec`],
//! [`crate::material::MaterialSelection`] and `pleochroma`'s
//! [`pleochroma::geometry::meet_solver::MeetConstraint`]. They are free functions because
//! most of these pairs are foreign to this crate, which the orphan rules do not allow to
//! implement `From`/`Into`. The module also holds [`design_to_file`]/[`design_from_file`]
//! and [`save`]'s [`catalogue_asc`].
//!
//! # The self-contained design file
//!
//! [`design_to_file`]/[`design_from_file`] convert a design to and from the single-file
//! `.psdd` format ([`pleochroma_formats::native::design`]). It carries the whole design and
//! needs no paired `.asc`.
//!
//! # Preserving the original `.asc` text
//!
//! [`pleochroma_formats::asc::to_asc_string`] is round-trip stable but not byte-identical to
//! hand-authored `GemCAD` output. So [`catalogue_asc`] keeps the caller's loaded original `.asc`
//! text byte for byte when the design's current [`crate::design::Design::to_asc_schedule`]
//! equals what that text parses to, with one field masked: `refractive_index` is compared
//! against the original's own value, not [`crate::design::Design::effective_refractive_index`].
//! A different built-in material changes the effective value alone, and the design file's
//! `[material]` table already records the real selection, so keeping the `I` line costs
//! nothing. Any real tier or preform edit breaks the equality and regenerates the `.asc`.
//! The comparison is the private `schedules_equal_ignoring_refractive_index` in `save`.

mod axis_curves;
mod convert;
mod design_file;
mod dispersion_dto;
mod effects_io;
mod load;
mod save;
#[cfg(test)]
mod tests;
mod unknown_keys;

pub use axis_curves::{axis_dispersion_dtos, with_snapshot_physics};
pub use dispersion_dto::{
    dispersion_dto_from_json, dispersion_model_dto, dispersion_model_from_dto,
    dispersion_model_from_json, dispersion_model_to_json,
};
pub use unknown_keys::UnknownFileKeys;

pub use convert::{
    SaveExtras, SnapshotColor, color_recipe_dto, gem_material_from_custom_snapshot,
    gem_material_from_custom_snapshot_keeping_recipe, snapshot_color,
};
pub use design_file::{
    DESIGN_HISTORY_LIMIT, DesignExtras, DesignLoadError, LoadedDesign, design_from_file,
    design_from_str, design_to_file, design_to_string,
};
pub use load::MaterialResolution;
pub use save::{
    CatalogueAsc, DraftReason, SaveError, catalogue_asc, catalogue_asc_extended,
    catalogue_asc_extended_from_solved,
};

pub use pleochroma_formats::native::{
    CustomMaterialSnapshot, DispersionModelDto, HistoryTable, MaterialTable, NativeMeetConstraint,
    NativePreformShape, PreformTable, RayBandsTable, SourceTable, TierTable,
    design::{
        AttachmentBlob, AttachmentRole, DesignFile, DesignFileError, DesignMetadata, FileKind,
    },
    sha256_hex,
};
