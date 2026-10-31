//! The production entry point of a frame: [`PreparedMaterial`] and [`trace_spectral_ray_prepared`].
//!
//! A zoned material's path-length kernel depends only on the zones' geometry, so a frame builds it
//! ONCE ([`PreparedMaterial::new`]) and every sample it traces borrows it, instead of every traced
//! sample rebuilding it (which, for a mesh-shell zone, also cloned the shell's vertices and
//! triangles each time). The per-zone tensors depend on the sample's wavelength comb and are still
//! built per sample. The radiance is bit-identical to the unprepared entry points.

use super::{
    super::{
        camera::{FacetFinish, HitRecord, Ray},
        environment::{EnvironmentSource, environment_supports_nee},
        surface::SurfaceScene,
        zoned::PreparedZoneKernels,
    },
    inner,
};
use crate::{
    geometry::tool::StoneGeometry,
    optics::{fluorescence::Fluorescence, materials::GemMaterial},
};
use glam::Vec3;
use std::sync::Arc;

/// A [`GemMaterial`] with the per-frame set-up of its zoning done once.
///
/// Build one next to the frame's `PlanesSoA32` arena and merged facet finishes and trace every
/// sample through [`trace_spectral_ray_prepared`]. A material without zones carries nothing extra.
///
/// The kernels are held through an [`Arc`]; a zoned frame therefore retains exactly one copy of
/// the mesh-shell bytes (one per frame), and the kernels stay alive for as long as any clone does,
/// even after the borrow of the material ends. Validation of the zones stays with whoever
/// installs them; [`PreparedMaterial::new`] only debug-asserts it.
#[derive(Clone)]
pub struct PreparedMaterial<'a> {
    /// The material every prepared trace reads.
    pub material: &'a GemMaterial,
    zone_kernels: Option<Arc<PreparedZoneKernels>>,
}

impl<'a> PreparedMaterial<'a> {
    /// Builds the zone kernels of `material` (once), or nothing when it has no zones.
    #[must_use]
    pub fn new(material: &'a GemMaterial) -> Self {
        Self {
            material,
            zone_kernels: material
                .zoning
                .as_ref()
                .map(|zoned| Arc::new(PreparedZoneKernels::new(zoned))),
        }
    }
}

/// [`trace_spectral_ray_with_finish_soa_geom_surface`] with the material prepared by the CALLER
/// once per frame/pass/job ([`PreparedMaterial::new`]).
///
/// This is the production entry point: its radiance is bit-identical to the unprepared function's
/// (which is this function with a freshly prepared material per call); only the zone kernels are
/// built once instead of once per sample. `prepared` must have been built from the material the
/// trace is meant to use (it is that material).
///
/// [`trace_spectral_ray_with_finish_soa_geom_surface`]: super::trace_spectral_ray_with_finish_soa_geom_surface
#[expect(
    clippy::too_many_arguments,
    reason = "see trace_spectral_ray_with_finish_soa_geom_surface's own reason; the prepared \
              material takes the place of the plain one"
)]
#[must_use]
pub fn trace_spectral_ray_prepared(
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
) -> Vec3 {
    inner::trace_spectral_ray_inner_geom_events_surface_prepared(
        initial_ray,
        geom.planes,
        geom.tools,
        plane_soa,
        facet_finishes,
        prepared.material,
        fluorescence,
        max_bounces,
        environment,
        rng_seed,
        hero_rand,
        primary_hit_out,
        true,
        true,
        environment_supports_nee(environment),
        ((None, None), surface, prepared.zone_kernels.as_ref()),
    )
}
