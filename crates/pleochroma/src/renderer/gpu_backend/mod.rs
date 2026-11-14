//! The decline-and-fall-back GPU wrapper, shared by every application in this
//! workspace.
//!
//! [`GpuBackend`] wraps [`GpuFrameRenderer`](super::gpu::GpuFrameRenderer) with the one
//! policy every caller needs: try the GPU, fall back to the CPU tracer whenever it
//! declines, and never pretend the GPU ran when it didn't.
//!
//! Lives here, shared by `apps/pleochroma-cut` and `apps/pleochroma-worker`, because the
//! decline reasons below are correctness rules -- a drifted copy would render a
//! plausible-looking wrong image rather than fail.
//!
//! Deliberately NOT `#[cfg(feature = "gpu")]`: [`GpuBackend`] exists in *both*
//! configurations (the real thing with the feature on, a stand-in that always declines
//! with it off), so an application has exactly one call site and no `#[cfg]` of its own.
//!
//! # Declining is normal, and decided per call
//!
//! A decline is not an error: it happens when this build has no `gpu` feature,
//! [`GpuBackend::disabled`] was chosen explicitly, or the machine has no usable adapter.
//! (Which environment/material kinds the GPU can dispatch is its own evolving list --
//! see `renderer::gpu::frame`'s module doc comment for the current picture.) Re-made on
//! every call, so a declining scene moves only its own samples to the CPU.
//!
//! # Sample-range additivity
//!
//! [`GpuBackend::try_accumulate`] ADDS into the caller's buffer, exactly as the CPU
//! tracers do. `sample_offset` must be the number of samples already folded into those
//! pixels: both the CPU formula and the GPU shader derive each sample's jitter and RNG
//! from the *absolute* sample index, so reusing an offset redraws identical samples and
//! biases the average instead of extending it. This keeps a GPU worker's samples
//! mergeable with a CPU viewer's.
//!
//! # Concurrency: chunk-level fairness, not per-frame monopoly
//!
//! Acquire once and share. The renderer lives behind a `Mutex` and every method takes
//! `&self`, so a `GpuBackend` can be wrapped in an `Arc` and handed to many threads --
//! `pleochroma-worker`'s `serve` module does exactly this, handing the SAME
//! `Arc<GpuBackend>` to every connection's own thread (see
//! `apps/pleochroma-worker/src/serve/mod.rs::run`).
//!
//! [`GpuBackend::try_accumulate_cancellable`] does not hold the renderer lock for a whole
//! request. It takes a FIFO ticket ([`turnstile::Turnstile`]) before each
//! [`turnstile::CHUNKS_PER_TURN`]-chunk turn through
//! [`super::gpu::GpuFrameRenderer::accumulate_turn`], releases the ticket and the renderer
//! lock when the turn ends, and rejoins the back of the queue if pixels remain. Concurrent
//! requests therefore interleave turn by turn. A plain `Mutex` does not guarantee this: the
//! OS may let one thread relock it repeatedly while others wait (starvation).
//!
//! ## Why draining before yielding a turn is required
//!
//! `GpuFrameRenderer`'s chunk pipeline is double-buffered and at most ONE chunk deep in
//! flight (see `renderer::gpu::frame`'s "Overlapped chunk pipeline" section).
//! `accumulate_turn` always drains that pending chunk before returning, so a finished turn
//! leaves the output and staging slots idle. That is what lets the SAME renderer serve a
//! different request's next turn: nothing is in flight for it to race with.
//!
//! This invariant is for turn boundaries. A batch
//! ([`GpuBackend::try_accumulate_batch_cancellable`]) keeps one chunk in flight across
//! picture boundaries while it holds its turn, and drains when it gives the turn up, so the
//! idle-renderer invariant still holds for other requests.
//!
//! ## Scene re-upload: the fairness cost
//!
//! `GpuFrameRenderer::scene_buffers` (camera, material, planes, facet finishes) is one
//! persistent set shared by all requests. A turn of another request overwrites it, so
//! `accumulate_turn` re-uploads those four buffers at the start of every turn. That keeps a
//! resumed request correct. The cost is a few hundred bytes per resumed turn. The large
//! per-chunk buffers (`outputs`, `staging`, `params_buffers`) are not re-uploaded on resume.
//! A single uncontended request pays the same small cost once per turn.
//!
//! ## Batches: a queue that stays fed between pictures
//!
//! [`GpuBackend::try_accumulate_batch_cancellable`] traces a list of [`GpuBatchItem`]s as
//! ONE chunk stream (see `renderer::gpu::frame`'s `batch` module): the next picture's
//! first chunk is queued before the previous picture's last chunk is read back, its scene
//! is written into the persistent scene buffers while that chunk may still run
//! (`write_buffer`/`submit` are ordered by `wgpu`, the same argument the per-slot params
//! buffers rely on, so no second scene-buffer set is needed), and pictures with different
//! material classes simply use their own already-compiled pipelines -- there is no
//! "current pipeline" to switch.
//!
//! Fairness: a batch keeps its turn while no other ticket is queued, with no drain between
//! chunks or pictures. After every [`turnstile::CHUNKS_PER_TURN`] chunks it checks the
//! turnstile for a waiting ticket. If one is queued, it drains its in-flight chunk, gives
//! the turn up and rejoins the back of the queue. An uncontended batch never drains.
//! `on_done` runs while the turn is held, with the next chunk already queued, so it should
//! only hand the picture off (for example a channel send).
//!
//! Memory: the output ring (two slots of output, reduced-pixel, staging and params buffers)
//! is sized once for the largest picture and reused for every picture. Per slot the output
//! buffer is bounded by the chunk byte budget; the reduced-pixel and staging buffers take
//! `12 B * pixels` each. On the host each picture in flight has its own `12 B * pixels`
//! scratch (at most two, pooled).
//!
//! ## Chunk sample offsets stay deterministic regardless of interleaving
//!
//! A chunk derives each sample's RNG seed and stratified jitter from the absolute
//! `(pixel, sample_offset + local_sample)` index alone (see "Sample-range additivity" above).
//! Interleaving turns changes only the wall-clock order of dispatches, so a request's result
//! is bit-identical to running it alone. `run_chunk_equivalence` checks the
//! non-interleaved case.
//!
//! ## `accum` is written only on `Done`
//!
//! Turns run against a backend-owned scratch buffer (zeroed per request, pooled across
//! requests), and [`GpuBackend::try_accumulate_cancellable`] adds it into the caller's
//! `accum` once the last turn completes. A decline or cancellation after some turns
//! already ran (a device lost mid-request, an uncaptured wgpu error caught at the end of a
//! turn, a cancel between turns) discards the scratch instead, so a caller that falls back
//! to the CPU for the full `spp` never double counts the samples the GPU had finished.
//! Cost: one zero-fill and one add pass over the frame per request.
//!
//! ## Desktop viewport and export: no contention to be fair about
//!
//! The live viewport (`ViewportGpu` in `bridge::render_thread::gpu_backend`) and the export
//! worker (`bridge::export_thread::worker::run_export`) in `apps/pleochroma-cut` each own a
//! separate [`GpuBackend`]. They never share a [`turnstile::Turnstile`] or renderer, so they
//! do not contend. Pausing the viewport during an export (`RenderContext::export_active`) is
//! a separate mechanism.
//!
//! The batch preview (`gui::batch::preview::wiring`) shares one [`GpuBackend`] across
//! `local_lane_count()` worker lanes, and `pleochroma-worker`'s `serve` module shares one
//! across connection threads. Both are the multi-caller case this turnstile exists for.
//!
//! # Device loss and recovery
//!
//! A [`GpuFrameError::DeviceLost`](super::gpu::GpuFrameError::DeviceLost) (or a renderer
//! mutex poisoned by a panic) is a decline like any other for the request that hits it,
//! and additionally marks the backend lost: [`GpuBackend::is_lost`] turns `true`,
//! [`GpuBackend::adapter_label`] returns `None`, and every call declines without touching
//! the renderer. It is not permanent. Once 30 s have passed since the loss (or since the
//! previous failed attempt), the next request re-acquires an adapter and compiles a fresh
//! renderer; on success the lost flag clears and the GPU serves again, with the pipeline
//! kind carried over. At most 6 attempts start in any hour, so a device that keeps
//! failing is left alone (the policy lives in [`recovery`]). Only a backend that HAD a
//! renderer can recover: one that found no adapter at start-up stays declining, as does
//! [`GpuBackend::disabled`]. A caller that advertises the backend (a worker's `HELLO`)
//! calls [`GpuBackend::try_recover`] first so it reports the current state.
//!
//! # Module layout
//!
//! [`turnstile`] is the FIFO ticket lock ([`turnstile::Turnstile`]/its RAII turn guard)
//! `feature = "gpu"`'s [`backend::GpuBackend`] drives it through; the non-`gpu` build
//! instead compiles [`stub::GpuBackend`], a signature-compatible always-decline stand-in.
//! [`GpuSceneRef`]/[`GpuAccumulate`]/[`GpuPipelineKind`] below are defined in both
//! configurations so a caller needs no `#[cfg]` of its own.

use glam::Vec3;
mod cache;
#[cfg(test)]
mod cache_tests;
mod limits;
mod tool_routing;
pub mod whitted;
pub use cache::{
    CacheTier, GpuPathCache, HostSlab, MemoryHints, RAM_SLAB_BYTES, SlabSpan, VRAM_SLAB_BYTES,
    memory_budget, plan_span,
};
pub use limits::GpuWorkLimits;
pub use tool_routing::{
    GpuRouteDecline, scene_decline_with_tools, scene_decline_with_tray_mirror,
    scene_decline_with_tray_shadow, scene_routes_to_gpu_with_tools,
};

use crate::{
    geometry::{
        GpuFacetPlane,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            Camera, EnvironmentSource, FacetFinish, LightingPreset, surface::SurfaceScene,
        },
    },
};

#[cfg(feature = "gpu")]
mod backend;
#[cfg(all(test, feature = "gpu"))]
mod cached_tests;
#[cfg(all(test, feature = "gpu"))]
mod estimator_tests;
#[cfg(all(test, feature = "gpu"))]
mod live_tests;
#[cfg(feature = "gpu")]
mod recovery;
#[cfg(all(test, feature = "gpu"))]
mod resident_tests;
#[cfg(not(feature = "gpu"))]
mod stub;
#[cfg(feature = "gpu")]
mod surface_entry;
#[cfg(all(test, feature = "gpu"))]
mod tests;
#[cfg(all(test, feature = "gpu"))]
mod tool_tests;
#[cfg(feature = "gpu")]
mod turnstile;
#[cfg(all(test, feature = "gpu"))]
mod zoned_tests;

#[cfg(feature = "gpu")]
pub use backend::{GpuBackend, GpuResidentRun};
#[cfg(not(feature = "gpu"))]
pub use stub::{GpuBackend, GpuResidentRun};

/// The scene inputs one GPU accumulation call needs.
///
/// Defined in both build configurations, like [`GpuAccumulate`] below, so callers stay
/// free of `#[cfg]`; the non-`gpu` stand-in just ignores the whole bundle.
#[cfg_attr(not(feature = "gpu"), allow(dead_code))]
pub struct GpuSceneRef<'a> {
    /// Camera the frame is rendered from.
    pub camera: &'a Camera,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Facet planes of the stone.
    pub planes: &'a [GpuFacetPlane],
    /// Per-plane surface finish, indexed in step with `planes`. `&[]` means every facet
    /// is polished -- see `GpuFrameScene::facet_finishes` on how a shorter slice is
    /// padded.
    pub facet_finishes: &'a [FacetFinish],
    /// Gem material.
    pub material: &'a GemMaterial,
    /// Maximum number of internal bounces per path.
    pub max_bounces: u32,
    /// Lighting environment.
    pub environment: EnvironmentSource<'a>,
    /// The estimator this frame asks for at the partial Fresnel events. `None` follows the
    /// renderer's own choice, else the process default
    /// ([`crate::optics::raytracer::default_estimator`]); `Some(estimator)` wins over both, so a
    /// worker renders the scene's estimator whatever its own process default is. Where exit
    /// collection does not apply the frame runs
    /// [`crate::optics::raytracer::Estimator::Branching`] whatever is asked.
    pub estimator: Option<crate::optics::raytracer::Estimator>,
}

/// The pipeline families a shader warm-up builds besides the production ones
/// ([`GpuBackend::warm_pipelines_cancellable`]).
///
/// The default builds none of them: exactly what
/// [`GpuBackend::prepare_geom_surface_cancellable`] builds. Defined in both build
/// configurations, like [`GpuSceneRef`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuWarmFamilies {
    /// The live megakernel and its reduce, which adaptive live passes run on
    /// ([`GpuBackend::try_accumulate_geom_surface_live_cancellable`]).
    pub live: bool,
    /// The path cache's phase B, its completion and the live reduce, which cached passes run on
    /// ([`GpuBackend::try_accumulate_cached_live_cancellable`]).
    pub path_cache: bool,
}

/// One picture of a batch: its scene, sample range and its own output buffer.
///
/// The unit of [`GpuBackend::try_accumulate_batch_cancellable`]; `out` receives exactly
/// what [`GpuBackend::try_accumulate_cancellable`] would have ADDED into its `accum` for
/// this scene (`first_sample` is the absolute sample offset, `samples` the count).
/// Defined in both build configurations, like [`GpuSceneRef`].
#[cfg_attr(not(feature = "gpu"), allow(dead_code))]
pub struct GpuBatchItem<'a> {
    /// The scene to trace.
    pub scene: GpuSceneRef<'a>,
    /// Concave tools of the stone (`&[]` for a convex one), at most
    /// [`GPU_MAX_TOOL_PRIMITIVES`](crate::geometry::tool::GPU_MAX_TOOL_PRIMITIVES); see
    /// [`scene_routes_to_gpu_with_tools`]. Tool `k` has facet id `scene.planes.len() + k`.
    pub tools: &'a [ToolPrimitive],
    /// The stone's surface finish (`SurfaceScene::IDEAL` for a polished one, which traces
    /// exactly as a batch without the field did).
    pub surface: SurfaceScene<'a>,
    /// Absolute index of the first sample (see "Sample-range additivity").
    pub first_sample: u32,
    /// Number of samples to trace.
    pub samples: u32,
    /// Output buffer, `scene.width * scene.height` pixels; written only on `Done`.
    pub out: &'a mut [Vec3],
}

/// Whether a scene may be dispatched to the GPU at all, for a caller that CANNOT hand the
/// GPU a tool list.
///
/// The material must be one the GPU supports, the stone must be convex (no tools), the
/// material must not fluoresce (`fluorescence` empty) and the lighting must not be a UV
/// lamp.
///
/// Fluorescence is single-wavelength CPU transport (`optics::fluorescence`), and the UV
/// lamps' spectra and the excitation paths below 380 nm exist only on the CPU, so such
/// a scene is traced entirely by the CPU.
///
/// A concave stone is declined here because [`GpuSceneRef`] carries planes only. A caller
/// that passes tools must use [`scene_routes_to_gpu_with_tools`] instead. The GPU kernels
/// do trace tools (`GpuBackend::try_accumulate_geom`, `GpuBatchItem::tools`). `lighting` is
/// the scene's analytic preset (`EnvironmentSource::lighting_preset`); for an HDR panorama
/// pass the preset's default, since no lamp is in play. A pure function, so it is unit-tested
/// without an adapter.
///
/// Not `const`: `GemMaterial::gpu_supported` inspects the zone list of a zoned material.
/// Mesh-shell, scattering and oversized band zones are CPU only. Declining them keeps a zoned
/// material from silently rendering its base zone on the GPU.
#[must_use]
pub fn scene_routes_to_gpu(
    material: &GemMaterial,
    geom: StoneGeometry<'_>,
    fluorescence: &Fluorescence,
    lighting: LightingPreset,
) -> bool {
    material.gpu_supported()
        && geom.is_convex()
        && fluorescence.is_empty()
        && !lighting.is_uv_lamp()
}

/// Outcome of [`GpuBackend::try_accumulate_cancellable`].
///
/// Gives a cancelled dispatch its OWN outcome, distinct from "done" and "declined": a
/// decline means fall back to the CPU for every requested sample, while a cancellation
/// means the caller no longer wants this work and usually retries neither.
///
/// Defined in both build configurations (like [`GpuSceneRef`] above).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuAccumulate {
    /// Every requested sample was traced and added into `accum`.
    Done,
    /// The GPU did not produce this request -- same meaning as
    /// [`GpuBackend::try_accumulate`] returning `false`: `accum` untouched, fall back to
    /// the CPU tracer for the FULL `spp`. See the module doc's "Declining is normal"
    /// section; also covers a lost device (see `GpuFrameError::DeviceLost`, the `lost`
    /// field on the `gpu`-gated `GpuBackend`, and the module doc's "Device loss and
    /// recovery" section). The guarantee holds however late the decline happens: a
    /// failure on the last turn of a many-turn request leaves `accum` exactly as it was
    /// passed in.
    Declined,
    /// `cancel` was observed set before every sample was traced.
    ///
    /// `accum` is untouched, however many turns had already run. Either fall back to the
    /// CPU for the full `spp`, or simply stop, exactly as if this call had never been
    /// made.
    Cancelled,
}

/// The fragment-stage parameters of [`GpuBackend::try_beam_fragments`] (beam guide §11.4).
#[cfg(feature = "beam")]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeamFragmentParams {
    /// The deepest fragment subdivision (`BeamSettings::max_fragment_levels`).
    pub max_levels: u32,
    /// The spectral node count `N_λ` the leaves' node indices refer to.
    pub n_lambda: u32,
    /// The smoothness tolerance `τ_L` (`BeamSettings::lighting_tolerance`).
    pub tolerance: f32,
}

/// Outcome of [`GpuBackend::try_beam_fragments`].
///
/// Defined in both build configurations (like [`GpuAccumulate`]), so callers stay free of `#[cfg]`
/// beyond the `beam` feature.
#[cfg(feature = "beam")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuBeamFragments {
    /// Every fragment of the batch was shaded and its pixel sums were added into `out`.
    Done,
    /// The GPU did not produce the batch (no device, a lost device, a tile larger than the
    /// storage binding, a kernel that declined). `out` is untouched; the caller shades the
    /// batch on the CPU. The text gives the reason for the log.
    Declined(&'static str),
    /// `cancel` was observed set: `out` is untouched.
    Cancelled,
}

/// Which transport kernel a render dispatches every chunk through.
///
/// The megakernel (`spectral_transport.wgsl`) or the wavefront pipeline
/// (`wavefront_transport.wgsl`); see `renderer::gpu::frame`'s module doc for the trade-off.
/// Both produce the same output for the same scene up to compiler rounding
/// (`renderer::gpu::frame::run_pipeline_equivalence`).
///
/// Defined here, not in the `gpu`-gated `renderer::gpu::frame`, so that
/// [`GpuBackend::set_pipeline_kind`] accepts it in both build configurations. It is
/// re-exported as `renderer::gpu::frame::GpuPipelineKind`.
///
/// The default is `Megakernel`. Change it only with [`GpuBackend::set_pipeline_kind`] or
/// [`super::gpu::GpuFrameRenderer::set_pipeline_kind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GpuPipelineKind {
    /// One monolithic kernel traces each path to completion.
    #[default]
    Megakernel,
    /// Separate kernels advance all paths one bounce at a time.
    Wavefront,
}

#[cfg(all(test, feature = "gpu"))]
impl GpuBackend {
    /// Raises the real lost flag, as a device loss would (test hook).
    ///
    /// [`Self::is_lost`] turns `true`, every request declines until [`Self::try_recover`]
    /// succeeds, and the recovery cool-down starts now. The remembered reason stays unset.
    /// Lets a test drive the backend's own lost flag.
    pub fn mark_lost_for_test(&self) {
        self.recovery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record_loss(std::time::Instant::now());
        self.lost.store(true, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(all(test, feature = "gpu"))]
mod lost_hook_tests {
    use super::GpuBackend;

    #[test]
    fn the_test_hook_raises_the_real_lost_flag() {
        let backend = GpuBackend::disabled();
        assert!(!backend.is_lost());
        backend.mark_lost_for_test();
        assert!(backend.is_lost());
        assert_eq!(
            backend.adapter_label(),
            None,
            "a lost backend claims no adapter"
        );
        // The cool-down has only just started, so no recovery is attempted yet.
        assert!(!backend.try_recover());
        assert!(backend.is_lost());
    }
}

#[cfg(test)]
mod routing_tests {
    use super::*;
    use crate::{
        geometry::cuts::StandardGemCuts,
        optics::{
            absorption::AbsorptionBand,
            fluorescence::{EmissionBand, FluorescentEmitter},
        },
    };

    #[test]
    fn scene_routes_to_gpu_is_false_for_fluorescence_or_a_uv_lamp() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let stone = StoneGeometry::planes_only(&planes);
        let material = GemMaterial::diamond();
        let emitter = FluorescentEmitter {
            excitation: vec![AbsorptionBand::new(410.0, 20.0, 1.0)],
            emission: vec![EmissionBand::new(694.0, 2.0, 1.0)],
            quantum_yield: 0.9,
        };
        let fluorescent = Fluorescence::new(vec![emitter]);
        // Without fluorescence, only the UV lamp presets route to the CPU.
        for preset in LightingPreset::ALL {
            assert_eq!(
                scene_routes_to_gpu(&material, stone, Fluorescence::none(), preset),
                !preset.is_uv_lamp(),
                "{preset:?}"
            );
        }
        // A non-empty fluorescence routes to the CPU under any lighting.
        for preset in LightingPreset::ALL {
            assert!(!scene_routes_to_gpu(&material, stone, &fluorescent, preset));
        }
    }

    /// A zoned stone the GPU zone kernels can render routes to the GPU; one
    /// with a mesh-shell zone, or a scattering one, stays on the CPU tracer instead of
    /// silently rendering its base zone on the GPU.
    #[test]
    fn scene_routes_to_gpu_declines_a_zoned_stone_the_kernels_cannot_render() {
        use crate::optics::{
            absorption::AbsorptionTensor,
            zoning::{Zone, ZoneAbsorption, ZoneFrame, ZoneShape, ZonedAbsorption},
        };
        use glam::DVec3;

        let zone_absorption = || {
            ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
                560.0, 60.0, 0.2,
            )]))
        };
        let zoning = |shape: ZoneShape| ZonedAbsorption {
            frame: ZoneFrame::IDENTITY,
            base: ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(Vec::new())),
            zones: vec![Zone {
                shape,
                absorption: zone_absorption(),
            }],
            boundary_softness_mm: 0.0,
            follows_axis: false,
        };
        let planes = StandardGemCuts::standard_round_brilliant();
        let stone = StoneGeometry::planes_only(&planes);
        let routes = |material: &GemMaterial| {
            scene_routes_to_gpu(
                material,
                stone,
                Fluorescence::none(),
                LightingPreset::Daylight,
            )
        };

        let plane_zone = GemMaterial::diamond().with_zoning(zoning(ZoneShape::HalfSpace {
            normal: DVec3::X,
            offset: 0.0,
        }));
        assert!(routes(&plane_zone), "a half-space zone is GPU-renderable");

        let mesh_zone = GemMaterial::diamond().with_zoning(zoning(ZoneShape::MeshShell {
            vertices: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            triangles: vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
        }));
        assert!(!routes(&mesh_zone), "a mesh-shell zone is CPU only");

        let mut scattering = plane_zone;
        scattering.scattering_sigma_s = 0.4;
        assert!(!routes(&scattering), "a zoned scattering stone is CPU only");
    }
}
