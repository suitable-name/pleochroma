//! The shared bounce loop every public entry point delegates to.
//!
//! [`trace_spectral_ray_inner`], plus its own small per-bounce helpers (primary-hit
//! capture, facet finish lookup, interior-segment absorption, and the per-sample ray
//! context builders).

use super::{
    PathTermination,
    path_events::PathEvents,
    reference::{KeptFn, ReferenceSample},
};

use super::super::{
    NUM_CHANNELS,
    absorption::apply_segment_absorption,
    camera::{FacetFinish, HitRecord, Ray},
    environment::{EnvironmentSource, environment_supports_nee},
    estimator::Estimator,
    intersect::shading_normal_near_edge,
    refraction::{
        DirectionMatch, RayMaterialContext, RayWavelengthCache, build_ray_wavelength_cache,
        build_ray_wavelength_cache_prepared, build_ray_wavelength_cache_tabled,
    },
    surface::{SurfaceScene, merged_facet_finishes},
    zoned::PreparedZoneKernels,
};
use crate::{
    geometry::{
        plane::GpuFacetPlane,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::{
        fluorescence::Fluorescence,
        materials::{CrystalSystem, GemMaterial},
        polarization::StokesVector,
        studio_rig::StudioRig,
        tables::OpticsTableSet,
    },
};
use glam::Vec3;
use std::sync::Arc;

mod bounce_body;
use bounce_body::{trace_spectral_ray_core, trace_spectral_ray_core_reference};

/// Records the primary ray's first hit for the denoiser's guide buffers. Only bounce 0 writes:
/// there `current_ray` is still the camera ray, and the path is driven by the hero channel, so
/// the slot receives the hero's own first hit.
///
/// A stone seen in a Tray Reflection is reached only after a floor bounce, so it has no bounce-0
/// hit. Its pixel keeps no primary hit (facet id -1 in the guide buffers) and the denoiser passes
/// it through unfiltered. This is a documented approximation (`docs/physics.md` section 9).
fn capture_primary_hit(
    primary_hit_out: &mut Option<&mut Option<HitRecord>>,
    bounce: u32,
    hit: Option<HitRecord>,
) {
    if bounce == 0
        && let Some(slot) = primary_hit_out.as_deref_mut()
    {
        *slot = hit;
    }
}

/// The facet's finish, with `Polished` for any index `facet_finishes` doesn't cover.
fn facet_finish_for(facet_finishes: &[FacetFinish], facet_idx: usize) -> FacetFinish {
    facet_finishes.get(facet_idx).copied().unwrap_or_default()
}

/// Edge-rounded shading normal for a hit on a (possibly concave) stone.
///
/// A convex stone returns [`shading_normal_near_edge`] unchanged, so a planar result is
/// bit-identical. With tools, rounding is skipped for a hit on a tool surface, and for a plane
/// hit within `rounding_radius` of a tool: `shading_normal_near_edge` blends toward the nearest
/// *plane* normal, the wrong neighbour there. The tool-proximity test probes four in-plane
/// points one radius away. That is exact when a tool crosses the facet, and cheap since tools
/// are few.
fn shading_normal_for_hit(
    geom: StoneGeometry<'_>,
    hit_point: Vec3,
    hit_facet_idx: usize,
    hit_normal: Vec3,
    rounding_radius: f32,
) -> Vec3 {
    if geom.is_convex() || rounding_radius <= 0.0 {
        return shading_normal_near_edge(
            geom.planes,
            hit_point,
            hit_facet_idx,
            hit_normal,
            rounding_radius,
        );
    }
    if hit_facet_idx >= geom.planes.len() {
        return hit_normal;
    }
    let (u, v) = crate::optics::raytracer::scattering::frosted_orthonormal_basis(hit_normal);
    let near_tool = [u, -u, v, -v].into_iter().any(|dir| {
        let probe = hit_point + dir * rounding_radius;
        geom.tools.iter().any(|tool| tool.contains(probe))
    });
    if near_tool {
        return hit_normal;
    }
    shading_normal_near_edge(
        geom.planes,
        hit_point,
        hit_facet_idx,
        hit_normal,
        rounding_radius,
    )
}

/// `true` when `p` is in the stone's material: inside every plane and outside every tool.
#[cfg(debug_assertions)]
fn point_in_material(geom: StoneGeometry<'_>, p: Vec3) -> bool {
    geom.planes
        .iter()
        .all(|pl| Vec3::from_array(pl.normal).dot(p) + pl.d <= 0.0)
        && !geom.tools.iter().any(|tool| tool.contains(p))
}

/// Debug check of the `inside_gem` state against the geometry (concave stones only).
///
/// Every boundary of the material is a crossing, so the point just before the next hit lies in
/// the material exactly when `inside_gem`. A probe within a few `1e-4` of another surface can
/// classify either way, so the check fires only when two probes straddling the `1e-4` hit
/// offset agree with each other.
///
/// `inside_gem` is the side the intersection reports (`intersect_stone_soa_side`), and the
/// classification shares no code with the intersection. The toggle alone is not enough on a
/// concave stone: a `1e-4` restart can step over a second surface (a tool's rim on a facet),
/// and the toggle would then name the wrong medium for the rest of the stretch.
#[cfg(debug_assertions)]
fn debug_assert_inside_gem_matches_geometry(
    geom: StoneGeometry<'_>,
    ray: Ray,
    t: f32,
    inside_gem: bool,
) {
    if geom.is_convex() || t < 1e-3 {
        return;
    }
    let a = point_in_material(geom, ray.origin + ray.dir * (t - 1e-4));
    let b = point_in_material(geom, ray.origin + ray.dir * (t - 2e-4));
    debug_assert!(
        a != b || a == inside_gem,
        "inside_gem = {inside_gem} disagrees with the point classification {a} at t = {t}"
    );
}

/// Interior-side handling of one facet hit: flips the geometric normal to face the interior
/// ray, then applies the segment's Beer-Lambert absorption. A no-op outside the gem.
///
/// A scattering material's extinction for the segment is already applied by `try_scatter_step`,
/// so absorption is applied here only for a non-scattering material.
///
/// `ray_ctx` bundles the material context and wavelength cache, to stay within clippy's
/// argument limit.
///
/// `k_hat` is the WAVE NORMAL `k`, not the Poynting direction: the assigned-mode E-field of the
/// absorption derives from `k`, while the segment length stays geometric. See the `refraction`
/// module docs.
///
/// `segment` is the ray the path travelled (origin and direction, model units) and its length
/// `hit_t`. Only a zoned material reads the ray, since its absorption depends on where the
/// segment lies (`absorption::apply_segment_absorption`).
///
/// `is_extraordinary` is the eigenmode assigned at the path's latest air->crystal entry (see
/// `absorption::channel_absorption_alphas_assigned`).
fn apply_interior_segment(
    ray_ctx: (&RayMaterialContext, &RayWavelengthCache),
    k_hat: Vec3,
    is_extraordinary: bool,
    segment: (Ray, f32),
    inside_gem: bool,
    normal: &mut Vec3,
    stokes: &mut [StokesVector; NUM_CHANNELS],
) {
    let (ctx, cache) = ray_ctx;
    if !inside_gem {
        return;
    }
    *normal = -*normal;
    if !ctx.material.scatters() {
        apply_segment_absorption(ctx, cache, k_hat, is_extraordinary, segment, stokes);
    }
}

/// Builds the per-sample [`RayMaterialContext`].
pub(super) fn build_ray_material_context(
    material: &GemMaterial,
    lambdas: [f32; NUM_CHANNELS],
    hero_idx: usize,
    enable_internal_mode_coupling: bool,
) -> RayMaterialContext<'_> {
    // Per-material optical c-axis for anisotropic birefringence.
    let c_axis = material.c_axis;
    let is_anisotropic = material.crystal_system != CrystalSystem::Cubic
        && material.birefringence_delta.abs() > 1e-4;
    RayMaterialContext {
        material,
        lambdas,
        hero_idx,
        c_axis,
        is_anisotropic,
        enable_internal_mode_coupling,
    }
}

/// Bundles [`build_ray_material_context`] and [`build_ray_wavelength_cache_prepared`] into one
/// call -- see [`RayWavelengthCache`]'s doc comment for what it caches. `zone_kernels` is the
/// frame's shared zone kernels (`None` for an unprepared trace, which builds its own). `tables` is
/// the render's optics lookup tables (`None`, the default, keeps every site on its closed form).
pub(super) fn build_ray_context<'m>(
    material: &'m GemMaterial,
    lambdas: [f32; NUM_CHANNELS],
    hero_idx: usize,
    enable_internal_mode_coupling: bool,
    zone_kernels: Option<&Arc<PreparedZoneKernels>>,
    tables: Option<&Arc<OpticsTableSet>>,
) -> (RayMaterialContext<'m>, RayWavelengthCache) {
    let mat_ctx =
        build_ray_material_context(material, lambdas, hero_idx, enable_internal_mode_coupling);
    let wavelength_cache = match (zone_kernels, tables) {
        (None, None) => build_ray_wavelength_cache(&mat_ctx),
        (kernels, None) => build_ray_wavelength_cache_prepared(&mat_ctx, kernels),
        (kernels, tables) => build_ray_wavelength_cache_tabled(&mat_ctx, kernels, tables),
    };
    (mat_ctx, wavelength_cache)
}

/// Records why/where the bounce loop stopped into the caller's `termination_out` slot;
/// a no-op if it is `None`.
fn record_termination(
    termination_out: &mut Option<&mut (u32, PathTermination)>,
    bounce: u32,
    reason: PathTermination,
) {
    if let Some(out) = termination_out.as_deref_mut() {
        *out = (bounce, reason);
    }
}

/// The shared body of [`trace_spectral_ray`](super::trace_spectral_ray) and
/// [`trace_spectral_ray_with_finish`](super::trace_spectral_ray_with_finish). See those for the
/// parameters.
///
/// `enable_internal_mode_coupling = false` fixes the eigenmode at entry for the whole interior
/// traversal. Only this module's tests pass `false`.
///
/// `plane_soa` is the [`crate::simd::PlanesSoA32`] arena that `intersect_polyhedron_soa` scans,
/// built once by the caller. `planes` is still needed, since `shading_normal_near_edge` reads the
/// per-facet plane records.
///
/// `pub(super)`: called by every public entry point in `transport::mod`, and by the A/B sweeps of
/// `transport::tests` over `enable_internal_mode_coupling`, `enable_exit_splitting` and
/// `enable_nee`. The public entry points fix those switches, so the tests need this.
#[expect(
    clippy::too_many_arguments,
    reason = "the shared bounce loop every public entry point delegates to -- see \
              trace_spectral_ray's own reason, plus the debug/instrumentation output \
              hooks (primary_hit_out, termination_out) and the caller-supplied \
              plane_soa arena alongside the planes slice it was built from"
)]
pub(super) fn trace_spectral_ray_inner(
    initial_ray: Ray,
    planes: &[GpuFacetPlane],
    plane_soa: &crate::simd::PlanesSoA32,
    facet_finishes: &[FacetFinish],
    material: &GemMaterial,
    max_bounces: u32,
    environment: EnvironmentSource<'_>,
    rng_seed: u32,
    hero_rand: f32,
    primary_hit_out: Option<&mut Option<HitRecord>>,
    enable_internal_mode_coupling: bool,
    enable_exit_splitting: bool,
    // `true` at the public entry points when `environment` is an HDR map (the studio rig has no
    // importance distribution for NEE samples, see `NeeContext::enabled`), or when a test forces
    // it for an on/off comparison.
    enable_nee: bool,
    termination_out: Option<&mut (u32, PathTermination)>,
) -> Vec3 {
    trace_spectral_ray_inner_geom(
        initial_ray,
        planes,
        &[],
        plane_soa,
        facet_finishes,
        material,
        Fluorescence::none(),
        max_bounces,
        environment,
        rng_seed,
        hero_rand,
        primary_hit_out,
        enable_internal_mode_coupling,
        enable_exit_splitting,
        enable_nee,
        termination_out,
    )
}

/// [`trace_spectral_ray_inner`] for a stone with tools: `tools` are subtracted from the
/// polyhedron `planes` define, and `plane_soa` is still the arena built from `planes` alone.
/// With `tools` empty this is `trace_spectral_ray_inner`, bit for bit.
#[expect(
    clippy::too_many_arguments,
    reason = "the shared bounce loop every public entry point delegates to -- see \
              trace_spectral_ray's own reason, plus the debug/instrumentation output \
              hooks (primary_hit_out, termination_out) and the caller-supplied \
              plane_soa arena alongside the planes slice it was built from"
)]
pub(super) fn trace_spectral_ray_inner_geom(
    initial_ray: Ray,
    planes: &[GpuFacetPlane],
    tools: &[ToolPrimitive],
    plane_soa: &crate::simd::PlanesSoA32,
    facet_finishes: &[FacetFinish],
    material: &GemMaterial,
    fluorescence: &Fluorescence,
    max_bounces: u32,
    environment: EnvironmentSource<'_>,
    rng_seed: u32,
    hero_rand: f32,
    primary_hit_out: Option<&mut Option<HitRecord>>,
    enable_internal_mode_coupling: bool,
    enable_exit_splitting: bool,
    // `true` at the public entry points when `environment` is an HDR map (the studio rig has no
    // importance distribution for NEE samples, see `NeeContext::enabled`), or when a test forces
    // it for an on/off comparison.
    enable_nee: bool,
    termination_out: Option<&mut (u32, PathTermination)>,
) -> Vec3 {
    trace_spectral_ray_inner_geom_events(
        initial_ray,
        planes,
        tools,
        plane_soa,
        facet_finishes,
        material,
        fluorescence,
        max_bounces,
        environment,
        rng_seed,
        hero_rand,
        primary_hit_out,
        enable_internal_mode_coupling,
        enable_exit_splitting,
        enable_nee,
        (termination_out, None),
    )
}

/// [`trace_spectral_ray_inner_geom`] with an optional [`PathEvents`] recorder beside the
/// termination slot (`outputs.1`). With `None` for the recorder it is the function above, bit
/// for bit; the recorder is only ever written by the bounce loop's hooks, never read.
#[expect(
    clippy::too_many_arguments,
    reason = "see trace_spectral_ray_inner_geom's own reason, plus the attribution recorder"
)]
pub(super) fn trace_spectral_ray_inner_geom_events(
    initial_ray: Ray,
    planes: &[GpuFacetPlane],
    tools: &[ToolPrimitive],
    plane_soa: &crate::simd::PlanesSoA32,
    facet_finishes: &[FacetFinish],
    material: &GemMaterial,
    fluorescence: &Fluorescence,
    max_bounces: u32,
    environment: EnvironmentSource<'_>,
    rng_seed: u32,
    hero_rand: f32,
    primary_hit_out: Option<&mut Option<HitRecord>>,
    enable_internal_mode_coupling: bool,
    enable_exit_splitting: bool,
    enable_nee: bool,
    outputs: (Option<&mut (u32, PathTermination)>, Option<&mut PathEvents>),
) -> Vec3 {
    trace_spectral_ray_inner_geom_events_surface(
        initial_ray,
        planes,
        tools,
        plane_soa,
        facet_finishes,
        material,
        fluorescence,
        max_bounces,
        environment,
        rng_seed,
        hero_rand,
        primary_hit_out,
        enable_internal_mode_coupling,
        enable_exit_splitting,
        enable_nee,
        (outputs, SurfaceScene::IDEAL),
    )
}

/// The optional termination and path-event outputs of a trace.
type TraceOutputs<'a, 'b> = (
    Option<&'a mut (u32, PathTermination)>,
    Option<&'b mut PathEvents>,
);

/// [`trace_spectral_ray_inner_geom_events`] with the surface finish of the facets and edges
/// (`surface`, see [`crate::optics::raytracer::surface`]). With [`SurfaceScene::IDEAL`] the
/// result equals the function above; a frosted facet takes the ground lobe in both.
#[expect(
    clippy::too_many_arguments,
    reason = "see trace_spectral_ray_inner_geom_events, plus the surface scene"
)]
pub(in crate::optics::raytracer) fn trace_spectral_ray_inner_geom_events_surface(
    initial_ray: Ray,
    planes: &[GpuFacetPlane],
    tools: &[ToolPrimitive],
    plane_soa: &crate::simd::PlanesSoA32,
    facet_finishes: &[FacetFinish],
    material: &GemMaterial,
    fluorescence: &Fluorescence,
    max_bounces: u32,
    environment: EnvironmentSource<'_>,
    rng_seed: u32,
    hero_rand: f32,
    primary_hit_out: Option<&mut Option<HitRecord>>,
    enable_internal_mode_coupling: bool,
    enable_exit_splitting: bool,
    enable_nee: bool,
    (outputs, surface): (TraceOutputs<'_, '_>, SurfaceScene<'_>),
) -> Vec3 {
    trace_spectral_ray_inner_geom_events_surface_prepared(
        initial_ray,
        planes,
        tools,
        plane_soa,
        facet_finishes,
        material,
        fluorescence,
        max_bounces,
        environment,
        rng_seed,
        hero_rand,
        primary_hit_out,
        enable_internal_mode_coupling,
        enable_exit_splitting,
        enable_nee,
        (outputs, surface, None, None, false),
    )
}

/// The extras of [`trace_spectral_ray_inner_geom_events_surface_prepared`]: the trace outputs, the
/// surface scene, the prepared material's zone kernels, the render's optics tables and whether
/// the exit-collection estimator is asked for.
type PreparedExtras<'a, 'b, 'c> = (
    TraceOutputs<'a, 'b>,
    SurfaceScene<'c>,
    Option<&'c Arc<PreparedZoneKernels>>,
    Option<&'c Arc<OpticsTableSet>>,
    bool,
);

/// [`trace_spectral_ray_inner_geom_events_surface`] with the frame's shared zone kernels as the
/// third element of the last parameter (from `PreparedMaterial`, see `transport::prepared`). With
/// `None` the kernels of a zoned material are built for this one trace. With `Some` they must
/// come from `material`'s own zoning. The radiance is bit-identical either way.
///
/// The fourth element is the render's optics lookup tables (`optics::tables`). With `None` no
/// table is read.
///
/// The fifth element requests the exit-collection estimator ([`Estimator::ExitCollection`](
/// crate::optics::raytracer::Estimator)); `false` is the branching estimator. The request is
/// honoured only without next-event estimation and fluorescence, over an ideal surface, without
/// a Tray Reflection (`environment.tray_mirror()` is `None`) and for a stone without tools. The
/// GPU declines the same scenes, so a hybrid frame never mixes estimators.
#[expect(
    clippy::too_many_arguments,
    reason = "see trace_spectral_ray_inner_geom_events_surface, plus the shared zone kernels"
)]
pub(in crate::optics::raytracer) fn trace_spectral_ray_inner_geom_events_surface_prepared(
    initial_ray: Ray,
    planes: &[GpuFacetPlane],
    tools: &[ToolPrimitive],
    plane_soa: &crate::simd::PlanesSoA32,
    facet_finishes: &[FacetFinish],
    material: &GemMaterial,
    fluorescence: &Fluorescence,
    max_bounces: u32,
    environment: EnvironmentSource<'_>,
    rng_seed: u32,
    hero_rand: f32,
    primary_hit_out: Option<&mut Option<HitRecord>>,
    enable_internal_mode_coupling: bool,
    enable_exit_splitting: bool,
    enable_nee: bool,
    (outputs, surface, zone_kernels, tables, exit_collection): PreparedExtras<'_, '_, '_>,
) -> Vec3 {
    let (termination_out, events) = outputs;
    // A ground facet of the surface list is a frosted facet in the finish list too, so every
    // reader of the list (the shadow probes, the glare rule) sees one answer. Borrowed, and free,
    // unless a ground word is missing from the list.
    let merged_finishes = merged_facet_finishes(facet_finishes, surface.facet_surfaces);
    let facet_finishes = &*merged_finishes;
    let scene = TraceScene {
        planes,
        tools,
        plane_soa,
        facet_finishes,
        material,
        fluorescence,
        max_bounces,
        environment,
        surface_glare: environment.surface_glare(),
        surface,
        zone_kernels,
        tables,
    };
    let switches = TraceSwitches {
        internal_mode_coupling: enable_internal_mode_coupling,
        exit_splitting: enable_exit_splitting,
        nee: enable_nee,
        direction_match: DirectionMatch::default(),
        estimator: if exit_collection {
            Estimator::ExitCollection
        } else {
            Estimator::Branching
        },
    };
    trace_spectral_ray_core(
        &scene,
        initial_ray,
        rng_seed,
        hero_rand,
        switches,
        (primary_hit_out, termination_out, events),
    )
}

/// The reference tracer's extras: the surface scene, the zone kernels, the optics tables and the
/// `kept` callback.
type ReferenceExtras<'a> = (
    SurfaceScene<'a>,
    Option<&'a Arc<PreparedZoneKernels>>,
    Option<&'a Arc<OpticsTableSet>>,
    Option<&'a KeptFn<'a>>,
);

/// The reference tracer's shared body: the production switches, except `direction_match = Exact`
/// and `exit_splitting = false`, with the signature accumulator and the `kept` zeroing on. `ids`
/// is `(rng_seed, hero_rand)`, and `extras` holds the surface scene, the render's optics tables
/// and the `kept` callback. See
/// See [`super::reference::trace_spectral_ray_reference_prepared`].
///
/// The zone kernels are the prepared material's, as in the production prepared entry. No
/// production entry point reaches this function.
#[expect(
    clippy::too_many_arguments,
    reason = "see trace_spectral_ray_inner_geom_events_surface_prepared, plus the kept callback"
)]
pub(super) fn trace_spectral_ray_reference_inner(
    initial_ray: Ray,
    geom: StoneGeometry<'_>,
    plane_soa: &crate::simd::PlanesSoA32,
    facet_finishes: &[FacetFinish],
    material: &GemMaterial,
    fluorescence: &Fluorescence,
    max_bounces: u32,
    environment: EnvironmentSource<'_>,
    ids: (u32, f32),
    primary_hit_out: Option<&mut Option<HitRecord>>,
    extras: ReferenceExtras<'_>,
) -> ReferenceSample {
    let (rng_seed, hero_rand) = ids;
    let (surface, zone_kernels, tables, kept) = extras;
    let merged_finishes = merged_facet_finishes(facet_finishes, surface.facet_surfaces);
    let facet_finishes = &*merged_finishes;
    let scene = TraceScene {
        planes: geom.planes,
        tools: geom.tools,
        plane_soa,
        facet_finishes,
        material,
        fluorescence,
        max_bounces,
        environment,
        surface_glare: environment.surface_glare(),
        surface,
        zone_kernels,
        tables,
    };
    let switches = TraceSwitches {
        internal_mode_coupling: true,
        exit_splitting: false,
        nee: environment_supports_nee(environment),
        direction_match: DirectionMatch::Exact,
        estimator: Estimator::Branching,
    };
    trace_spectral_ray_core_reference(
        &scene,
        initial_ray,
        rng_seed,
        hero_rand,
        switches,
        (primary_hit_out, kept),
    )
}

/// The per-trace inputs of one traced ray, bundled to keep the loop's argument count down.
/// `surface_glare` is read from the environment (`EnvironmentSource::surface_glare`).
#[derive(Clone, Copy)]
struct TraceScene<'a> {
    planes: &'a [GpuFacetPlane],
    /// Convex volumes subtracted from the polyhedron; empty for a planar stone.
    tools: &'a [ToolPrimitive],
    plane_soa: &'a crate::simd::PlanesSoA32,
    facet_finishes: &'a [FacetFinish],
    material: &'a GemMaterial,
    /// Fluorescent emitters beside the material; empty (the usual case) leaves the
    /// whole trace exactly as it was without the field.
    fluorescence: &'a Fluorescence,
    max_bounces: u32,
    environment: EnvironmentSource<'a>,
    surface_glare: f32,
    /// The surface finish of the facets and edges; [`SurfaceScene::IDEAL`] (the usual case)
    /// leaves the whole trace exactly as it was without the field.
    surface: SurfaceScene<'a>,
    /// The frame's shared zone kernels, built once by `PreparedMaterial`; `None` builds them
    /// per trace (unprepared callers) or means the material has no zones.
    zone_kernels: Option<&'a Arc<PreparedZoneKernels>>,
    /// The render's optics lookup tables (`optics::tables`); `None` (the usual case) leaves the
    /// whole trace exactly as it was without the field.
    tables: Option<&'a Arc<OpticsTableSet>>,
}

thread_local! {
    /// The last [`StudioRig`] this thread built, keyed by the exact bits of its
    /// `(light_yaw, light_pitch)`. A render batch traces many samples under one light pose, so
    /// the rig is built once per thread and copied per sample.
    static STUDIO_RIG_CACHE: std::cell::Cell<Option<(u32, u32, StudioRig)>> =
        const { std::cell::Cell::new(None) };
}

/// The [`StudioRig`] for a light pose, from this thread's one-entry cache.
///
/// Value-identical to `StudioRig::new(light_yaw, light_pitch)`: the cache returns a copy of
/// that function's own output for the same input bits, so nothing downstream can tell the
/// difference. NaN poses key on their bits too and rebuild to the same NaN rig.
fn cached_studio_rig(light_yaw: f32, light_pitch: f32) -> StudioRig {
    let key = (light_yaw.to_bits(), light_pitch.to_bits());
    if let Some((yaw_bits, pitch_bits, rig)) = STUDIO_RIG_CACHE.get()
        && (yaw_bits, pitch_bits) == key
    {
        return rig;
    }
    let rig = StudioRig::new(light_yaw, light_pitch);
    STUDIO_RIG_CACHE.set(Some((key.0, key.1, rig)));
    rig
}

/// The A/B switches of the bounce loop. The public entry points hardcode every one
/// (`nee` follows the environment kind); only this module's tests vary them.
#[derive(Clone, Copy)]
struct TraceSwitches {
    internal_mode_coupling: bool,
    exit_splitting: bool,
    nee: bool,
    /// The dispersive-event match rule; the default (the cosine test) everywhere except the
    /// beam tracer's reference tracer, which sets `Exact` (beam guide §5.5).
    direction_match: DirectionMatch,
    /// The estimator asked for. The bounce loop honours [`Estimator::ExitCollection`] only
    /// without next-event estimation and fluorescence, over an ideal surface, with exit
    /// splitting on, without a Tray Reflection and for a stone without tools;
    /// [`Estimator::Branching`] everywhere but the prepared production entry.
    estimator: Estimator,
}
