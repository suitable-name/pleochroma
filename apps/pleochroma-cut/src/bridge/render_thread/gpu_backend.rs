//! The GPU backend wrapper and frame dispatch: `ViewportGpu` (the shared `GpuBackend`
//! plus the guide buffers it cannot produce itself), the hybrid CPU+GPU pacing state,
//! and the per-frame dispatch that picks between them.

#[cfg(feature = "render-bench")]
mod benchmark_observation;
mod calibration;
mod dispatch;
#[path = "owned_backend_frame.rs"]
pub(super) mod owned_frame;
mod warmup;
#[cfg(feature = "render-bench")]
use super::engine::observation::{BenchmarkObservation, PhaseTimer};
#[cfg(test)]
use crate::settings::model::LocalComputeTarget;
#[cfg(test)]
use dispatch::accumulate_frame_samples;
pub(super) use dispatch::{HybridPacing, accumulate_frame_samples_cancellable};
use glam::Vec3;
use pleochroma::{
    geometry::{
        plane::GpuFacetPlane,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, FacetFinish, surface::SurfaceScene},
    },
    renderer::gpu_backend::{
        GpuAccumulate, GpuBackend, GpuSceneRef, scene_routes_to_gpu_with_tools,
    },
};
use std::sync::atomic::{AtomicBool, Ordering};

/// One frame's scene, as both backends need it. Bundled so [`ViewportGpu::try_accumulate_cancellable`]
/// and the CPU path take the same description rather than a dozen loose parameters.
#[derive(Clone, Copy)]
pub(super) struct BackendFrame<'a> {
    pub(super) width: u32,
    pub(super) height: u32,
    /// Pose, carried separately from `camera` because the guide-buffer cache keys on
    /// it (see `bridge::frame_cache::guide_pass`) and cannot recover it from a built
    /// `Camera`.
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) distance: f32,
    pub(super) camera: &'a Camera,
    pub(super) planes: &'a [GpuFacetPlane],
    /// The concave tools cut out of `planes`; empty for a planar stone. The GPU kernels
    /// trace them (`GpuBackend::try_accumulate_geom`), up to `GPU_MAX_TOOL_PRIMITIVES`
    /// (see [`BackendFrame::routes_to_gpu`]).
    pub(super) tools: &'a [ToolPrimitive],
    /// The material's fluorescent emitters (`RenderContext::active_fluorescence`), empty for
    /// every non-fluorescent material. A frame with emitters never reaches the GPU either.
    pub(super) fluorescence: &'a Fluorescence,
    /// Frosted girdle: `&[]` when `RenderContext::girdle_frosted` is off, which both
    /// backends treat as all-polished.
    pub(super) facet_finishes: &'a [FacetFinish],
    /// The facet surfaces, faceting machine and abraded edge; [`SurfaceScene::IDEAL`] for a
    /// perfect polish. Both tracers read it: the CPU tracer directly, the GPU kernels through
    /// `GpuBackend::try_accumulate_geom_surface`.
    pub(super) surface: SurfaceScene<'a>,
    pub(super) material: &'a GemMaterial,
    pub(super) max_bounces: u32,
    pub(super) environment: EnvironmentSource<'a>,
    pub(super) spp: u32,
    /// Samples already in the accumulation buffer. Seeds each sample's jitter/RNG on
    /// both backends, so this must keep advancing across frames or samples repeat.
    pub(super) sample_offset: u32,
}

impl BackendFrame<'_> {
    /// The frame's stone: the polyhedron with its tools.
    pub(super) const fn stone(&self) -> StoneGeometry<'_> {
        StoneGeometry {
            planes: self.planes,
            tools: self.tools,
        }
    }

    /// Whether the GPU may trace this frame at all: a supported material, at most
    /// `GPU_MAX_TOOL_PRIMITIVES` concave tools, no fluorescence and no UV lamp
    /// ([`scene_routes_to_gpu_with_tools`]). Every GPU entry point below asks it first, so a
    /// frame the GPU cannot trace takes the CPU tracer for every compute target.
    pub(super) fn routes_to_gpu(&self) -> bool {
        scene_routes_to_gpu_with_tools(
            self.material,
            self.stone(),
            self.fluorescence,
            self.environment.lighting_preset().unwrap_or_default(),
        )
    }
}

/// The four per-pixel buffers a frame writes: the radiance running sum, plus the three
/// first-hit guide buffers the A-Trous denoiser keys on.
pub(super) struct FrameOutputs<'a> {
    pub(super) accum: &'a mut [Vec3],
    pub(super) depth: &'a mut [f32],
    pub(super) normal: &'a mut [Vec3],
    pub(super) facet_id: &'a mut [i32],
}

/// The viewport's GPU backend: the shared [`GpuBackend`] plus the guide buffers it
/// cannot produce.
///
/// The megakernel returns radiance only, with no first-hit depth/normal/facet-id, so
/// the A-Trous denoiser has nothing to key on -- the same gap a remote worker's `FRAME`
/// payload has, solved the same way: `bridge::frame_cache::guide_pass`'s local
/// primary-ray prepass, cached on pose plus geometry. When the `gpu` feature is off,
/// `GpuBackend` always declines and the guide cache is never consulted.
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent latches (signature wanted, last request accepted, retired, ray status shown) \
              written at unrelated points of the frame loop; grouping them would only add an indirection"
)]
pub(super) struct ViewportGpu {
    gpu: GpuBackend,
    warmup: warmup::PathWarmup,
    shared_hdr: Option<std::sync::Arc<pleochroma::renderer::env_map::EnvironmentMap>>,
    guides: crate::bridge::frame_cache::guide_pass::GuideCache,
    /// Key of the guide buffers currently copied into the render loop's buffers, so an
    /// unchanged pose copies nothing rather than memcpying ~10 MB every frame.
    applied_guide_key: Option<crate::bridge::frame_cache::guide_pass::GuideKey>,
    /// Key of the path signature currently held by the render loop's signature buffer
    /// (`None`: the buffer is zeroed or unset); see [`Self::sync_path_sig`].
    path_sig_key: Option<crate::bridge::frame_cache::guide_pass::GuideKey>,
    /// Whether this frame's path signature will be consumed (denoiser on and camera at
    /// rest); see [`Self::set_path_sig_wanted`]. When `false` the guide prepass runs with
    /// `n_d = 0.0` (no signature walk), because [`Self::sync_path_sig`] would zero the
    /// signature anyway.
    path_sig_wanted: bool,
    /// Whether the last GPU request was accepted. The guide prepass is only started
    /// speculatively, beside the GPU turn, while this holds, so a backend that keeps
    /// declining (lost device, oversized map) never pays a prepass for a CPU frame.
    last_gpu_ok: bool,
    /// Set once a joined GPU-thread panic is observed (see [`hybrid_frame`])
    /// and never cleared -- `GpuBackend` itself only recovers from a cleanly-reported
    /// [`pleochroma::renderer::gpu::GpuFrameError::DeviceLost`], which
    /// a raw thread panic never reaches (the panic unwinds past the normal return path
    /// entirely). Without this, a panic observed only through `hybrid_frame`'s joined
    /// thread left the single-engine path at [`accumulate_frame_samples_cancellable`] free to call
    /// [`Self::try_accumulate_cancellable`] again next frame, straight back into the wgpu state
    /// that produced the panic. A reported device loss is deliberately NOT tracked here:
    /// `GpuBackend` owns that recovery (see [`Self::try_accumulate_cancellable`]).
    gpu_retired: bool,
    /// User-facing status text for the viewport's own status pill
    /// (`ui/components/gem_viewport.slint`'s `ViewportModel.gpu_status_text`) --
    /// `None` while healthy or on a CPU-only build (nothing to report), `Some` while
    /// the device is lost or once [`Self::gpu_retired`] is set. The
    /// render loop polls [`Self::status_message`] once per frame and only pushes a
    /// Slint property write when it actually changed -- see `spawn_render_thread`'s
    /// own `last_gpu_status` local.
    status_message: Option<String>,
    /// Whether [`Self::status_message`] was last written by the Ray-Traced path
    /// ([`Self::set_ray_status`]), so a switch back to Path-Traced can drop it.
    ray_status_active: bool,
    #[cfg(feature = "render-bench")]
    observation: BenchmarkObservation,
}

/// The status pill's text while a GPU compiler/upload worker is running.
pub(super) const PREPARING_STATUS: &str = "Preparing GPU renderer";

impl ViewportGpu {
    pub(super) fn acquire() -> Self {
        #[cfg(not(test))]
        let gpu = GpuBackend::disabled();
        #[cfg(test)]
        let gpu = {
            #[cfg(feature = "ui-harness")]
            if crate::harness_support::viewport_rendering_disabled() {
                return Self::from_backend(GpuBackend::disabled(), true);
            }
            GpuBackend::acquire()
        };
        Self::from_backend(gpu, cfg!(test))
    }

    fn from_backend(gpu: GpuBackend, already_acquired: bool) -> Self {
        Self {
            gpu,
            warmup: warmup::PathWarmup::new(already_acquired),
            shared_hdr: None,
            guides: crate::bridge::frame_cache::guide_pass::GuideCache::new(),
            applied_guide_key: None,
            path_sig_key: None,
            path_sig_wanted: true,
            last_gpu_ok: true,
            gpu_retired: false,
            status_message: None,
            ray_status_active: false,
            #[cfg(feature = "render-bench")]
            observation: BenchmarkObservation::default(),
        }
    }

    /// Declares whether the coming frame's path signature will be used (denoiser on and
    /// camera at rest). The render loop calls it before every frame; with `false` the
    /// guide prepass skips the signature walk (1-3 extra intersections per pixel while
    /// the camera is moving, all of it discarded by [`Self::sync_path_sig`]). The guide
    /// key carries the effective `n_d`, so the first frame after the camera stops sees a
    /// different key and regenerates the guides with the signature.
    pub(super) const fn set_path_sig_wanted(&mut self, wanted: bool) {
        self.path_sig_wanted = wanted;
    }

    /// The `n_d` the guide prepass runs with: the material's, or `0.0` ("no signature")
    /// while [`Self::path_sig_wanted`] is off. Depth, normal and facet id do not depend
    /// on it.
    fn guide_n_d(&self, frame: &BackendFrame<'_>) -> f32 {
        if self.path_sig_wanted {
            frame.material.dispersion.n_d()
        } else {
            0.0
        }
    }

    /// Permanently stops this session's GPU frames after a joined GPU-thread panic --
    /// mirrors the policy `GpuBackend::try_accumulate_cancellable`
    /// already applies to its own `lost` flag (`Declined` forever, never retried), just
    /// tracked here too since a raw thread panic never reaches that code path.
    fn retire(&mut self) {
        self.gpu_retired = true;
        self.status_message = Some("CPU fallback (GPU render thread panicked)".to_string());
    }

    /// The viewport's current GPU status text, if there is anything to show -- see
    /// [`Self::status_message`]'s own doc comment.
    pub(super) fn status_message(&self) -> Option<&str> {
        self.status_message.as_deref()
    }

    pub(super) fn set_ray_status(&mut self, status: Option<&str>) {
        self.status_message = status.map(str::to_owned);
        self.ray_status_active = status.is_some();
    }

    /// Drops a status the Ray-Traced path left behind once the frame is Path-Traced again;
    /// the Path-Traced route writes its own status when it has one.
    pub(super) fn clear_ray_status(&mut self) {
        if std::mem::take(&mut self.ray_status_active) {
            self.status_message = None;
        }
    }

    /// Drops "Preparing GPU renderer" when the frame runs on the CPU and nothing polls
    /// the preparation worker (CPU target, or a scene the GPU does not trace). A device
    /// loss or retirement message stays: it still describes the GPU.
    pub(super) fn clear_preparing_status(&mut self) {
        if self.status_message.as_deref() == Some(PREPARING_STATUS) {
            self.status_message = None;
            self.ray_status_active = false;
        }
    }

    /// Invalidates the cached guide-buffer key so the NEXT GPU frame recopies its guide
    /// buffers unconditionally, even if the pose/geometry key it would compute matches
    /// the one last applied.
    ///
    /// Must be called whenever something OTHER than [`Self::try_accumulate_cancellable`] has just
    /// written `depth`/`normal`/`facet_id` -- a CPU scanline frame
    /// (`render_frame_scanlines`, taking any of [`accumulate_frame_samples_cancellable`]'s non-GPU
    /// branches) or a resize/dirty reallocation
    /// (`frame_helpers::update_accumulation_state`, called from the render loop) -- since
    /// either leaves guide data in the buffers that does NOT match
    /// `self.applied_guide_key`'s cached pose/geometry association, which
    /// [`Self::try_accumulate_cancellable`]'s `self.applied_guide_key.as_ref() != Some(&key)` check
    /// would otherwise treat as already up to date and skip recopying, serving a stale
    /// depth/normal/facet-id triple to the denoiser on the next GPU frame.
    pub(super) const fn invalidate_guide_cache(&mut self) {
        self.applied_guide_key = None;
    }

    /// Accumulates one frame's samples on the GPU and refreshes the guide buffers.
    ///
    /// Returns `false` without touching `out` if the GPU declines OR this backend was
    /// already [`Self::retire`]d, in which case the caller must run the CPU path for this
    /// frame.
    ///
    /// # Lost devices
    ///
    /// Recovery belongs to [`GpuBackend`] alone: a lost device makes it decline, and at
    /// the start of each later request it re-acquires a fresh device once its cool-down
    /// has passed (`GpuBackend::try_recover`, within an hourly attempt budget). This
    /// wrapper never swaps the backend itself; it only reports. An ORDINARY decline
    /// (unsupported material/environment, or no adapter at all -- neither sets `lost`)
    /// falls back to the CPU for this one frame with no status change. A decline caused
    /// by a genuine loss also runs the frame on the CPU and sets [`Self::status_message`]
    /// for the viewport's status pill; the message clears the moment a GPU frame
    /// succeeds again.
    #[cfg(all(test, feature = "gpu"))]
    fn try_accumulate(&mut self, frame: &BackendFrame<'_>, out: &mut FrameOutputs<'_>) -> bool {
        let cancel = AtomicBool::new(false);
        self.wait_preparation(frame, &cancel)
            && self.try_accumulate_cancellable(frame, out, &cancel) == GpuAccumulate::Done
    }

    /// Returns cancellation separately from decline; only a decline requests CPU
    /// fallback. A cancelled guide pass may follow completed GPU radiance, so the
    /// caller must discard its staged output rather than count those samples.
    fn try_accumulate_cancellable(
        &mut self,
        frame: &BackendFrame<'_>,
        out: &mut FrameOutputs<'_>,
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        if cancel.load(Ordering::Relaxed) {
            return GpuAccumulate::Cancelled;
        }
        // A frame the GPU cannot trace (fluorescence, a UV lamp, more tools than the kernels
        // trace) is the CPU tracer's alone, and that is no fault of the device: decline
        // without touching `status_message` so the pill stays quiet.
        if self.gpu_retired || !frame.routes_to_gpu() {
            #[cfg(feature = "render-bench")]
            self.note_benchmark_gpu_decline();
            return GpuAccumulate::Declined;
        }
        if !self.poll_preparation(frame) {
            return GpuAccumulate::Declined;
        }
        let scene = GpuSceneRef {
            camera: frame.camera,
            width: frame.width,
            height: frame.height,
            planes: frame.planes,
            facet_finishes: frame.facet_finishes,
            material: frame.material,
            max_bounces: frame.max_bounces,
            environment: frame.environment,
        };
        // The guides do not depend on the GPU's result, so for a changed pose the prepass
        // runs beside the GPU turn instead of after it (UI-1 b); an unchanged pose has
        // nothing to compute.
        let key = Self::guide_key(frame, self.guide_n_d(frame));
        let overlap = self.last_gpu_ok && self.applied_guide_key.as_ref() != Some(&key);
        #[cfg(feature = "render-bench")]
        let timer = PhaseTimer::start(
            if overlap {
                "path_gpu_radiance_and_parallel_guides_inclusive"
            } else {
                "path_gpu_radiance_inclusive"
            },
            overlap || self.observation.route == "hybrid",
        );
        let accepted = if overlap {
            self.accumulate_beside_prepass(&scene, frame, out.accum, cancel)
        } else {
            self.gpu.try_accumulate_geom_surface_cancellable(
                &scene,
                frame.tools,
                &frame.surface,
                frame.sample_offset,
                frame.spp,
                out.accum,
                cancel,
            )
        };
        #[cfg(feature = "render-bench")]
        {
            timer.finish(&mut self.observation);
            self.observation.adapter_label = self.gpu.adapter_label();
        }
        if accepted == GpuAccumulate::Cancelled || cancel.load(Ordering::Relaxed) {
            return GpuAccumulate::Cancelled;
        }
        self.last_gpu_ok = accepted == GpuAccumulate::Done;
        if self.last_gpu_ok {
            self.status_message = None;
            #[cfg(feature = "render-bench")]
            let timer = PhaseTimer::start(
                "path_guides_ensure_and_copy",
                self.observation.route == "hybrid",
            );
            let copied = self.copy_guides(frame, out, cancel);
            #[cfg(feature = "render-bench")]
            timer.finish(&mut self.observation);
            return if copied {
                GpuAccumulate::Done
            } else {
                GpuAccumulate::Cancelled
            };
        }
        if self.gpu.is_lost() {
            self.note_device_loss();
        }
        #[cfg(feature = "render-bench")]
        self.note_benchmark_gpu_decline();
        GpuAccumulate::Declined
    }

    /// Records a genuine device loss in the status pill. The backend recovers itself
    /// (cool-down, attempt budget) at the start of a later request, so this only reports:
    /// the frame that hit the loss, and every frame until recovery, runs on the CPU
    /// tracer. The log line is written once per loss, on the transition into the lost
    /// state, not on every declined frame.
    fn note_device_loss(&mut self) {
        if self.status_message.is_none() {
            let reason = self
                .gpu
                .last_lost_reason()
                .unwrap_or_else(|| "unknown reason".to_string());
            tracing::warn!(%reason, "GPU renderer lost; rendering on the CPU tracer until it recovers");
            self.status_message = Some(format!("CPU fallback (GPU lost: {reason})"));
        }
    }

    /// Copies the guide buffers for `frame`'s pose/geometry into `out` if they are not
    /// already the ones last applied -- see [`Self::applied_guide_key`]'s own doc
    /// comment. Split out of [`Self::try_accumulate_cancellable`] to keep it short.
    fn copy_guides(
        &mut self,
        frame: &BackendFrame<'_>,
        out: &mut FrameOutputs<'_>,
        cancel: &AtomicBool,
    ) -> bool {
        let key = Self::guide_key(frame, self.guide_n_d(frame));
        if self.applied_guide_key.as_ref() != Some(&key) {
            let Some(guides) = live_guides(|| {
                self.guides
                    .ensure_geom_cancellable(key.clone(), frame.stone(), cancel)
            }) else {
                return false;
            };
            out.depth.copy_from_slice(&guides.depth);
            out.normal.copy_from_slice(&guides.normal);
            out.facet_id.copy_from_slice(&guides.facet_id);
            self.applied_guide_key = Some(key);
        }
        !cancel.load(Ordering::Relaxed)
    }

    /// Runs the GPU turn on this thread while a scoped thread generates `frame`'s guide
    /// buffers into the cache (same call, same inputs as [`Self::copy_guides`] would make
    /// afterwards, so the buffers are bit-identical; only the time differs). The prepass
    /// is joined when the scope ends, before anything reads the guides. A panic in either
    /// half propagates out of the scope exactly as the sequential code's would.
    fn accumulate_beside_prepass(
        &mut self,
        scene: &GpuSceneRef<'_>,
        frame: &BackendFrame<'_>,
        accum: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        let key = Self::guide_key(frame, self.guide_n_d(frame));
        let (gpu, guides) = (&mut self.gpu, &mut self.guides);
        std::thread::scope(|scope| {
            scope.spawn(move || {
                let _ = live_guides(|| guides.ensure_geom_cancellable(key, frame.stone(), cancel));
            });
            gpu.try_accumulate_geom_surface_cancellable(
                scene,
                frame.tools,
                &frame.surface,
                frame.sample_offset,
                frame.spp,
                accum,
                cancel,
            )
        })
    }

    /// The guide-cache key for `frame`: pose, geometry and `n_d` (`0.0` = no signature).
    fn guide_key(
        frame: &BackendFrame<'_>,
        n_d: f32,
    ) -> crate::bridge::frame_cache::guide_pass::GuideKey {
        crate::bridge::frame_cache::guide_pass::GuideCache::key_for_camera(
            frame.width,
            frame.height,
            frame.camera,
            frame.stone(),
            n_d,
        )
    }

    /// Brings `sig` (the render loop's first-hit path-signature buffer, `width * height`
    /// long) in line with `frame`'s pose, geometry and material, for CPU and GPU frames
    /// alike: both take the signature from the same [`GuideCache`] the GPU copy above
    /// uses, so a GPU frame has already paid for it. Does nothing while the key is
    /// unchanged.
    ///
    /// With `compute == false` (denoiser off, or the camera moving, when a prepass per
    /// pose would cost more than the filter gains) the buffer is zeroed instead, which
    /// the denoiser treats as one constant region, i.e. no signature term.
    pub(super) fn sync_path_sig(
        &mut self,
        frame: &BackendFrame<'_>,
        sig: &mut Vec<u32>,
        compute: bool,
        cancel: &AtomicBool,
    ) -> bool {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let pixels = frame.width as usize * frame.height as usize;
        if !compute {
            if self.path_sig_key.take().is_some() || sig.len() != pixels {
                sig.clear();
                sig.resize(pixels, 0);
            }
            return true;
        }
        let n_d = frame.material.dispersion.n_d();
        let key = Self::guide_key(frame, n_d);
        if self.path_sig_key.as_ref() == Some(&key) && sig.len() == pixels {
            return true;
        }
        let Some(guides) = live_guides(|| {
            self.guides
                .ensure_geom_cancellable(key.clone(), frame.stone(), cancel)
        }) else {
            return false;
        };
        sig.clear();
        sig.extend_from_slice(&guides.path_sig);
        self.path_sig_key = Some(key);
        !cancel.load(Ordering::Relaxed)
    }
}

/// Guide prepasses share the tracing partition, including the hybrid GPU helper.
fn live_guides<T>(work: impl FnOnce() -> T) -> T {
    use pleochroma::renderer::tonemap::live_cpu_budget;
    let _lease = live_cpu_budget::single_core_lease();
    let _trace_lease = live_cpu_budget::trace_lease();
    live_cpu_budget::with_limit(live_cpu_budget::partition().0, work)
}

#[cfg(test)]
mod concave_routing_tests;
#[cfg(test)]
mod status_tests;

/// Hardware tests driving the app's OWN [`ViewportGpu::
/// try_accumulate`] directly -- `pleochroma`'s own
/// `renderer::gpu_backend::tests::viewport_frames_with_camera_changes_never_poison_the_backend`
/// covers the crate-level `GpuBackend` entry point this wraps, but not this wrapper's
/// own self-healing/guide-buffer logic. Reaches `ViewportGpu`'s private fields
/// directly, which is why these live here rather than in `pleochroma`'s own test
/// module. Each test acquires its OWN `ViewportGpu` and prints a note and returns (a
/// clean skip, not a failure) when no GPU adapter is available.
#[cfg(all(test, feature = "gpu"))]
mod gpu_hardware_tests {
    use super::*;
    use pleochroma::{
        geometry::cuts::StandardGemCuts, optics::raytracer::LightingPreset,
        renderer::gpu_backend::GpuPipelineKind,
    };

    /// Drives `ViewportGpu::try_accumulate` across 12 simulated viewport frames, each
    /// with a DIFFERENT camera pose (as `on_camera_orbit` produces on every drag
    /// `moved` event) and an advancing `sample_offset` (as the render loop's
    /// `accum_samples` does), for the given pipeline kind. Every call must succeed
    /// (`true`), and `status_message()` must stay `None` throughout -- any `Some` means
    /// a turn was declared `DeviceLost` and the self-healing path in
    /// `ViewportGpu::try_accumulate` had to report a loss, which is
    /// exactly the poisoning this bug report describes.
    fn assert_twelve_rotated_frames_never_lose_the_device(pipeline_kind: GpuPipelineKind) {
        let mut viewport = ViewportGpu::acquire();
        if viewport.gpu.adapter_label().is_none() {
            println!(
                "skipping assert_twelve_rotated_frames_never_lose_the_device({pipeline_kind:?}): \
                 no GPU adapter"
            );
            return;
        }
        viewport.gpu.set_pipeline_kind(pipeline_kind);

        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material");
        let environment = LightingPreset::Daylight.studio(1.0, 0.4, 0.35);
        let (width, height) = (480u32, 360u32);
        let spp = 2u32;
        let pixel_count = (width * height) as usize;
        let mut accum = vec![Vec3::ZERO; pixel_count];
        let mut depth = vec![1.0e6; pixel_count];
        let mut normal = vec![Vec3::ZERO; pixel_count];
        let mut facet_id = vec![-1i32; pixel_count];

        for frame in 0..12u32 {
            let yaw = (frame as f32).mul_add(0.29, 0.1);
            let pitch = (frame as f32).mul_add(0.13, 0.2).clamp(-1.4, 1.4);
            let camera = Camera::new(yaw, pitch, 5.0, 18.0);
            let backend_frame = BackendFrame {
                width,
                height,
                yaw,
                pitch,
                distance: 5.0,
                camera: &camera,
                planes: &planes,
                tools: &[],
                fluorescence: Fluorescence::none(),
                facet_finishes: &[],
                surface: SurfaceScene::IDEAL,
                material: &material,
                max_bounces: 4,
                environment,
                spp,
                sample_offset: frame * spp,
            };
            let ok = viewport.try_accumulate(
                &backend_frame,
                &mut FrameOutputs {
                    accum: &mut accum,
                    depth: &mut depth,
                    normal: &mut normal,
                    facet_id: &mut facet_id,
                },
            );
            assert!(
                ok,
                "frame {frame} ({pipeline_kind:?}, yaw={yaw}, pitch={pitch}) declined -- \
                 status: {:?}",
                viewport.status_message()
            );
            assert_eq!(
                viewport.status_message(),
                None,
                "frame {frame} ({pipeline_kind:?}) left a self-healing status behind -- \
                 the device was lost at some point during this run"
            );
        }
        assert!(
            accum.iter().any(|v| v.length_squared() > 0.0),
            "a lit studio-rig scene traced over 12 frames must leave SOME nonzero radiance"
        );
    }

    #[test]
    fn twelve_rotated_frames_never_lose_the_device_megakernel() {
        assert_twelve_rotated_frames_never_lose_the_device(GpuPipelineKind::Megakernel);
    }

    #[test]
    fn twelve_rotated_frames_never_lose_the_device_wavefront() {
        assert_twelve_rotated_frames_never_lose_the_device(GpuPipelineKind::Wavefront);
    }
}
