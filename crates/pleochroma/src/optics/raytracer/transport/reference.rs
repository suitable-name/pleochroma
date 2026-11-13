//! The reference tracer: exact direction matching, exit splitting off, one branch signature per
//! sample and the `kept` zeroing.
//!
//! [`trace_spectral_ray_reference_prepared`] is [`trace_spectral_ray_prepared`] with two
//! switches changed: a channel stays live at a dispersive event only if its refractive data equal
//! the hero's bit for bit ([`DirectionMatch::Exact`](super::super::refraction::DirectionMatch)),
//! and exit splitting is off. Under that rule every live channel rides the hero's own path, so one
//! branch signature per sample describes all of them, and a per-channel live mask says which
//! channels the sample carries. The `kept` callback lets a caller remove the light the beams
//! already account for: for each live channel it is asked whether the sample's branch is kept at
//! that wavelength, and a kept channel's radiance is zeroed before the spectral integration.
//!
//! [`trace_spectral_ray_prepared`]: super::trace_spectral_ray_prepared

use super::{
    super::{
        camera::{FacetFinish, HitRecord, Ray},
        environment::EnvironmentSource,
        surface::SurfaceScene,
    },
    PreparedMaterial, inner,
};
use crate::{
    geometry::tool::StoneGeometry,
    optics::{fluorescence::Fluorescence, raytracer::NUM_CHANNELS},
};
use glam::Vec3;

/// One sample of the reference tracer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReferenceSample {
    /// The sample's XYZ, after the `kept` zeroing, white balance and the floor at zero.
    pub xyz: Vec3,
    /// The branch signature of the hero's path (two lanes, see `branch_step`). A path that met no
    /// facet carries `BRANCH_SIG_SEED`.
    pub signature: [u32; 2],
    /// Bit `k` is set when channel `k` is live, that is when its path density is positive at the
    /// end of the path. The hero is channel 0.
    pub live_mask: u8,
}

// The mask holds one bit per channel.
const _: () = assert!(NUM_CHANNELS <= u8::BITS as usize);

/// The `kept` callback of the reference tracer: `(packed signature, wavelength in nm) -> kept`.
///
/// The caller binds the sample's image point into the closure; the signature is
/// [`pack_sig`](crate::renderer::path_cache::signature::pack_sig) of the branch signature.
pub type KeptFn<'k> = dyn Fn(u64, f32) -> bool + 'k;

/// [`trace_spectral_ray_prepared`](super::trace_spectral_ray_prepared) as the reference estimator
/// of the beam tracer (see `renderer::beam`).
///
/// Direction matching is exact (bitwise equal indices; the hero always matches) and exit splitting
/// is off, so the result is the single-wavelength hero estimator. The returned
/// [`ReferenceSample`] carries one branch signature for the whole sample and the live mask. The
/// other arguments are those of the production entry point.
///
/// `kept`, when given, is called once per live channel `k` with the packed signature and the
/// channel's wavelength; a `true` answer zeroes that channel's radiance before the integration.
/// A path that met no facet is never offered to `kept`. With `kept = None` nothing is zeroed.
/// The Henyey-Greenstein NEE deposit of a scattering material is not per channel and is not
/// zeroed. The zone kernels are the prepared material's, as in the production entry.
#[expect(
    clippy::too_many_arguments,
    reason = "see trace_spectral_ray_prepared's own reason, plus the kept callback"
)]
#[must_use]
pub fn trace_spectral_ray_reference_prepared(
    initial_ray: Ray,
    geom: StoneGeometry<'_>,
    plane_soa: &crate::simd::PlanesSoA32,
    facet_finishes: &[FacetFinish],
    prepared: &PreparedMaterial<'_>,
    fluorescence: &Fluorescence,
    max_bounces: u32,
    environment: EnvironmentSource<'_>,
    rng_seed: u32,
    hero_rand: f32,
    primary_hit_out: Option<&mut Option<HitRecord>>,
    surface: SurfaceScene<'_>,
    kept: Option<&KeptFn<'_>>,
) -> ReferenceSample {
    inner::trace_spectral_ray_reference_inner(
        initial_ray,
        geom,
        plane_soa,
        facet_finishes,
        prepared.material,
        fluorescence,
        max_bounces,
        environment,
        (rng_seed, hero_rand),
        primary_hit_out,
        (
            surface,
            prepared.zone_kernel_set(),
            prepared.optics_table_set(),
            kept,
        ),
    )
}
