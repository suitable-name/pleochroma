//! The production entry point: [`PreparedMaterial`] and [`trace_spectral_ray_prepared`].
//!
//! A zoned material's path-length kernel depends only on the zones' geometry, so a frame builds it
//! once ([`PreparedMaterial::new`]) and every sample it traces borrows it. The per-zone tensors
//! depend on each sample's wavelength comb and are still built per sample. The radiance is
//! bit-identical to the unprepared entry points.

use super::{
    super::{
        NUM_CHANNELS,
        camera::{FacetFinish, HitRecord, Ray},
        environment::{EnvironmentSource, environment_supports_nee},
        estimator::{Estimator, default_estimator},
        refraction::{RayMaterialContext, RayWavelengthCache},
        surface::SurfaceScene,
        zoned::PreparedZoneKernels,
    },
    PathEvents, PathTermination, inner,
};
use crate::{
    geometry::tool::StoneGeometry,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        tables::{OpticsTableSet, OpticsTables, default_optics_tables},
    },
};
use glam::Vec3;
use std::sync::Arc;

/// A [`GemMaterial`] with the per-frame set-up of its zoning done once.
///
/// Build one next to the frame's `PlanesSoA32` arena and merged facet finishes, and trace every
/// sample through [`trace_spectral_ray_prepared`]. A material without zones carries nothing extra.
///
/// The kernels are held through an [`Arc`], so they live as long as any clone does, even after
/// the borrow of the material ends. Validation of the zones stays with whoever installs them;
/// [`PreparedMaterial::new`] only debug-asserts it.
///
/// The optics lookup tables (`optics::tables`) are built once per frame when they are on.
/// [`PreparedMaterial::new`] follows the process default
/// ([`crate::optics::tables::set_default_optics_tables`], off unless set), and
/// [`PreparedMaterial::with_optics_tables`] overrides it for this material. With them off nothing
/// is built and every trace is bit-identical to one without the feature.
///
/// The [`Estimator`] follows the process default
/// ([`crate::optics::raytracer::set_default_estimator`], [`Estimator::Branching`] unless set), and
/// [`PreparedMaterial::with_estimator`] overrides it for this material. Only
/// [`trace_spectral_ray_prepared`] and its attributed twin honour it, and only where the estimator
/// module documents that exit collection applies (no next-event estimation, no fluorescence, an
/// ideal surface, a stone without tools). Everywhere else the trace runs
/// [`Estimator::Branching`].
#[derive(Clone)]
pub struct PreparedMaterial<'a> {
    /// The material every prepared trace reads.
    pub material: &'a GemMaterial,
    zone_kernels: Option<Arc<PreparedZoneKernels>>,
    optics_tables: Option<Arc<OpticsTableSet>>,
    estimator: Estimator,
}

impl<'a> PreparedMaterial<'a> {
    /// Builds the zone kernels of `material` (once), or nothing when it has no zones, the
    /// optics lookup tables when the process default switches them on, and takes the process
    /// default estimator.
    #[must_use]
    pub fn new(material: &'a GemMaterial) -> Self {
        Self {
            material,
            zone_kernels: material
                .zoning
                .as_ref()
                .map(|zoned| Arc::new(PreparedZoneKernels::new(zoned))),
            optics_tables: None,
            estimator: default_estimator(),
        }
        .with_optics_tables(default_optics_tables())
    }

    /// Selects the estimator the prepared traces of this material run.
    ///
    /// [`Estimator::Branching`] gives traces bit-identical to the unprepared entry points.
    #[must_use]
    pub const fn with_estimator(mut self, estimator: Estimator) -> Self {
        self.estimator = estimator;
        self
    }

    /// The estimator the prepared traces of this material run.
    #[must_use]
    pub const fn estimator(&self) -> Estimator {
        self.estimator
    }

    /// Switches the optics lookup tables on (building them for this material) or off.
    ///
    /// [`OpticsTables::Off`] drops them, so the traces of this material run the closed forms.
    #[must_use]
    pub fn with_optics_tables(mut self, mode: OpticsTables) -> Self {
        self.optics_tables = match mode {
            OpticsTables::On => Some(Arc::new(OpticsTableSet::for_material(self.material))),
            OpticsTables::Off => None,
        };
        self
    }

    /// Whether this material's traces use the optics lookup tables.
    #[must_use]
    pub const fn optics_tables(&self) -> OpticsTables {
        if self.optics_tables.is_some() {
            OpticsTables::On
        } else {
            OpticsTables::Off
        }
    }

    /// The frame's shared zone kernels, `None` when the material has no zones (for the beam
    /// tracer's reference entry, which must trace exactly like the prepared production entry).
    #[must_use]
    pub(super) const fn zone_kernel_set(&self) -> Option<&Arc<PreparedZoneKernels>> {
        self.zone_kernels.as_ref()
    }

    /// The optics lookup tables of this material, when they are on (for the GPU buffer).
    #[must_use]
    pub const fn optics_table_set(&self) -> Option<&Arc<OpticsTableSet>> {
        self.optics_tables.as_ref()
    }

    /// The per-ray material context and wavelength cache for a fixed channel comb.
    ///
    /// `lambdas` are the eight channel wavelengths and `hero_idx` the channel that drives the
    /// geometry, exactly as a traced sample's hero comb would. Internal mode coupling is on, like
    /// every production trace. The path cache's deterministic branch tree reads the path tracer's
    /// own refraction geometry and absorption through this pair.
    #[must_use]
    pub(crate) fn ray_context(
        &self,
        lambdas: [f32; NUM_CHANNELS],
        hero_idx: usize,
    ) -> (RayMaterialContext<'a>, RayWavelengthCache) {
        inner::build_ray_context(
            self.material,
            lambdas,
            hero_idx,
            true,
            self.zone_kernels.as_ref(),
            self.optics_tables.as_ref(),
        )
    }
}

/// `trace_spectral_ray_with_finish_soa_geom_surface` with the material prepared once by the caller
/// per frame, pass or job ([`PreparedMaterial::new`]).
///
/// This is the production entry point. Its radiance is bit-identical to the unprepared function's,
/// which prepares the material per call. `prepared` must have been built from the material the
/// trace uses.
///
/// This is the only entry point that honours [`PreparedMaterial::estimator`]. With
/// [`Estimator::ExitCollection`], a trace without next-event estimation (a studio rig or daylight,
/// not an HDR map), without fluorescence, without a Tray Reflection, over an ideal surface and for
/// a stone without tools collects the exit leaves instead of drawing a branch at each partial
/// Fresnel event. The GPU declines the same scenes. Any other trace runs [`Estimator::Branching`].
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
        (
            (None, None),
            surface,
            prepared.zone_kernels.as_ref(),
            prepared.optics_tables.as_ref(),
            prepared.estimator == Estimator::ExitCollection,
        ),
    )
}

/// [`trace_spectral_ray_prepared`] that also returns what the path did.
///
/// The `Vec3` is that function's result bit for bit (same switches, same loop; the recorder is
/// only written to). The [`PathEvents`] carry the labelling data of the hero path: how many
/// interior reflections it made before its first exit and the direction it finally escaped in.
/// The path cache tags the paths its records do not cover with them.
#[expect(
    clippy::too_many_arguments,
    reason = "see trace_spectral_ray_prepared's own reason"
)]
#[must_use]
pub fn trace_spectral_ray_attributed_prepared(
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
    surface: SurfaceScene<'_>,
) -> (Vec3, PathEvents) {
    let mut events = PathEvents::default();
    let mut termination = (max_bounces, PathTermination::HitCap);
    let xyz = inner::trace_spectral_ray_inner_geom_events_surface_prepared(
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
        None,
        true,
        true,
        environment_supports_nee(environment),
        (
            (Some(&mut termination), Some(&mut events)),
            surface,
            prepared.zone_kernels.as_ref(),
            prepared.optics_tables.as_ref(),
            // The attribution labels describe one branch per event, which is what the path
            // cache reads them as: always the branching estimator.
            false,
        ),
    );
    events.terminated(termination.1);
    (xyz, events)
}
