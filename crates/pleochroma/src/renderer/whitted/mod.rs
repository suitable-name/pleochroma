//! Deterministic Fresnel-weighted image tracer (the Ray-Traced engine).
//!
//! Rays form a linear internal chain. Only concave geometry needs a bounded stack
//! for outside leaves that can enter another part of the stone. Output is summed
//! CIE XYZ, so existing accumulation and display code can consume it unchanged.
//!
//! # Tray reflection
//!
//! With a `TrayMirror` in the environment (the glossy tray under the stone, see
//! `optics::raytracer::environment`), every escape that meets the tray plane splits
//! deterministically: `(1 - F)` of its radiance is the matte-paper lookup and a reflected
//! branch with Schlick factor `F` is traced on, so the stone can appear mirrored.
//! The split lives in one function, `chain::resolve_escape`, used by both escape sites (the
//! branch miss and the direct resolve of an outside leaf); its WGSL twin is `wt_floor_escape`,
//! which calls `wt_floor`, the twin of `TrayMirror::hit`.
//!
//! Not mirrored: frosted facets keep their per-facet mean (`frost.rs` has no position to hit
//! the tray from), the ASET table is a direction-only lookup, and the stone-shadow lookup
//! (`WhittedEnv::leaf_at`) serves only the matte `(1 - F)` part. A reflected leaf reads the
//! environment along the mirrored direction without searching for the stone when the worklist
//! has no room or the depth cap is reached.

mod aa;
pub(crate) mod chain;
pub(crate) mod env;
mod frame;
pub(crate) mod fresnel;
mod frost;
mod prep;
pub(crate) mod tint;

pub use aa::aa_offset;
pub use env::WhittedEnv;
pub use frame::{whitted_pixels_interleaved_geom, whitted_primary_hit, whitted_tap_xyz};
pub use prep::{Chain, WhittedPrep};
pub use tint::{ABSORPTION_BINS, ABSORPTION_MODES, BinnedAbsorption};

/// Increment when an intentional image change should invalidate cached drafts.
pub const WHITTED_VERSION: u32 = 3;
pub const CUTOFF: f32 = 1.0e-3;
pub const WORKLIST_CAPACITY: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhittedDecline {
    UvLamp,
    Fluorescence,
}

impl WhittedDecline {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::UvLamp | Self::Fluorescence => {
                "Ray-Traced cannot show this material's lighting effect. Use Path-Traced."
            }
        }
    }
}

impl std::fmt::Display for WhittedDecline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}
impl std::error::Error for WhittedDecline {}

#[cfg(test)]
mod tests_chain;
#[cfg(test)]
mod tests_frame;
#[cfg(test)]
mod tests_mirror;
#[cfg(test)]
mod tests_prep;
#[cfg(test)]
mod tests_shadow;
