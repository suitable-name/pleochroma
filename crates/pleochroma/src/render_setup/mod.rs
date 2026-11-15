//! Pure render-setup helpers shared by every renderer built on this crate.
//!
//! The desktop viewer (`apps/pleochroma-cut`, through thin re-exports) and the browser app
//! (`apps/pleochroma-web`) both use them, so both build the same scene/material for the same
//! settings and produce byte-identical output.
//!
//! Everything here is pure (no `Instant::now`, no filesystem access) except the
//! `stone-shadow` precompute, which uses threads; the rest compiles for
//! `wasm32-unknown-unknown`. State that must persist across frames (a plane-hash-keyed
//! measurement cache, for instance) stays on the caller's side -- see
//! [`stone_width::measure_model_width`]'s own doc comment.
//!
//! Submodules: [`materials`] (by-name material resolution and the render-time
//! override stack), [`backdrop`] (the `Backdrop` level/index mapping), [`plane_hash`]
//! (the cheap plane-set identity every per-design cache keys on), [`stone_width`] (the
//! pure girdle-width measurement `apply_material_overrides`'s stone-size override
//! needs), [`icc_profile`] (the embedded-profile byte builder for wide-gamut PNG
//! export) and, behind the `hdr` feature (which already pulls in `image`),
//! [`png_encode`] (in-memory PNG+ICC encoding), and [`sample_scale`] (the target
//! samples slider's exponent<->count mapping) and [`inclusion_scale`] (the Inclusion Haze
//! slider's position<->coefficient mapping).

pub mod backdrop;
pub mod icc_profile;
pub mod inclusion_scale;
pub mod materials;
pub mod plane_hash;
#[cfg(feature = "hdr")]
pub mod png_encode;
pub mod sample_scale;
mod stone_pose;
#[cfg(feature = "stone-shadow")]
mod stone_shadow;
pub mod stone_width;
mod tray_mirror;
#[cfg(feature = "stone-shadow")]
pub use stone_shadow::{stone_shadow_for, stone_shadow_for_with_threads};
pub use tray_mirror::{TrayMirrorOff, tray_mirror_for};

pub use backdrop::Backdrop;
pub use materials::{
    MODEL_UNIT_FACE_UP_PATH, MaterialOverrides, PHYSICS_DEFAULT_STONE_WIDTH_MM,
    REFERENCE_MODEL_WIDTH, absorption_path_scale_for, apply_material_overrides,
    apply_material_overrides_with_inclusions, effective_stone_width_mm, material_for_stone,
    material_for_stone_oriented, needs_model_width, resolve_material,
    resolve_material_with_override, stone_width_for_scale,
};
pub use plane_hash::{hash_geometry, hash_planes};
#[cfg(feature = "hdr")]
pub use png_encode::{encode_png_with_icc, encode_preview_png};
pub use stone_pose::{PosedStone, StonePose, pose_stone};
pub use stone_width::measure_model_width;
