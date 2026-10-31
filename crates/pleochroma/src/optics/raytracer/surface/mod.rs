//! Surface finish: polish quality, scratches and ground facets.
//!
//! A facet is perfectly polished (the exact interface physics, a Dirac-delta specular event),
//! really polished (the same, plus a halo of total integrated scatter, stretched perpendicular to
//! the polishing scratches), or ground (a GGX rough dielectric). The pieces:
//!
//! - [`ggx`]: the anisotropic GGX distribution, masking functions and the visible-normal sampler;
//! - [`tis`]: the total integrated scatter of a polish of RMS roughness `Rq`;
//! - [`facet_surface`]: [`FacetSurface`], the packed per-facet word the GPU reads too, and the
//!   polish presets;
//! - [`sweep`]: the faceting machine and the sweep of the stone across the lap, which turn the
//!   facet's orientation into a scratch direction;
//! - [`ground`] and [`compensation`]: the ground facet's bounce, its next-event estimation, and
//!   the tabulated energy compensation of its single-scatter lobe;
//! - [`dispatch`]: the hooks the bounce loop calls before and after a facet's interface event;
//! - [`edge`]: the abraded-edge band and its TIS boost;
//! - [`finishes`]: the finish list a scene's ground surfaces are reported in.
//!
//! With the default [`SurfaceScene`] (no facet surfaces, no abrasion) the hooks return their
//! inputs unchanged and a trace is bit-identical to one without this module; only a frosted
//! facet differs, because the ground lobe replaced the Lambertian bounce (decision D11).
//!
//! The WGSL twins (lane RP-G2) are written against these expressions and draw from the same RNG
//! streams, named below.

pub mod compensation;
pub mod dispatch;
pub mod edge;
pub mod facet_surface;
pub mod finishes;
pub mod ggx;
pub mod ground;
pub mod micro;
pub mod sweep;
pub mod tis;

#[cfg(test)]
mod tests_ground;
#[cfg(test)]
mod tests_lobe;
#[cfg(test)]
mod tests_plumbing;

pub use edge::EdgeAbrasion;
pub use facet_surface::{
    DEFAULT_GROUND_ALPHA, DEFAULT_SCRATCH_STRENGTH, FacetSurface, PolishPreset, SurfaceKind,
    effective_anisotropy, halo_alpha,
};
pub use finishes::merged_facet_finishes;
pub use ggx::GgxLobe;
pub use ground::{GroundCtx, GroundFrame, GroundInterface, GroundParams, GroundSample};
pub use sweep::{FacetingMachine, SweepPreset, SweepRange};

pub(crate) use edge::EdgeBand;
pub(crate) use ground::apply_ground_bounce;

use crate::optics::raytracer::sampling::hash_u32;

/// The draw that decides whether a polished event takes the halo branch.
pub(crate) const SURFACE_HALO_STREAM: u32 = 0x85EB_CA6B;

/// The first uniform number of the halo's visible-normal draw.
pub(crate) const SURFACE_VNDF_U_STREAM: u32 = 0xC2B2_AE35;

/// The second uniform number of the halo's visible-normal draw.
pub(crate) const SURFACE_VNDF_V_STREAM: u32 = 0xD6E8_FEB8;

/// The draw of the contact radius in the sweep, which turns into the scratch angle. The CPU and
/// the GPU twin draw the angle from this one stream.
pub(crate) const SURFACE_SWEEP_STREAM: u32 = 0xA136_AAAD;

/// A `[0, 1]` draw from `rng_seed`'s stream `stream` at `bounce`, in the form every per-bounce
/// draw of the tracer uses.
pub(crate) fn unit_draw(rng_seed: u32, bounce: u32, stream: u32) -> f32 {
    (hash_u32(rng_seed ^ hash_u32(bounce ^ stream)) as f32) / 4_294_967_295.0
}

/// The surface inputs of one trace: the facet surfaces parallel to the planes, the faceting
/// machine, and the abraded-edge setting.
///
/// Empty `facet_surfaces` and no abrasion is the ideal scene: every hook is skipped.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceScene<'a> {
    /// The surface of every facet, indexed like the planes; a missing entry is ideal.
    pub facet_surfaces: &'a [FacetSurface],
    /// The machine the scratch directions are derived from.
    pub machine: FacetingMachine,
    /// The abraded-edge band.
    pub edge: EdgeAbrasion,
}

impl SurfaceScene<'static> {
    /// No facet surfaces and no abrasion.
    pub const IDEAL: Self = Self {
        facet_surfaces: &[],
        machine: FacetingMachine::DEFAULT,
        edge: EdgeAbrasion::NONE,
    };
}

impl SurfaceScene<'_> {
    /// `true` when nothing in the scene departs from a perfect polish.
    #[must_use]
    pub fn is_ideal(&self) -> bool {
        self.facet_surfaces.is_empty() && !self.edge.active()
    }
}
