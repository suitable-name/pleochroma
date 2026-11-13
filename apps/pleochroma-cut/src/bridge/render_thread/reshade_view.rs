//! Instant lighting changes in the Ray-Traced view (Preferences > Rendering >
//! "Instant Lighting Changes in the Ray-Traced View"): memory only, built for a pose that has
//! stood still.
//!
//! # When it lives
//!
//! While the stone moves there is no cache. When a still Ray-Traced view has finished its taps,
//! [`ReshadeView`] builds the path cache of that pose in system RAM, with the viewport path
//! cache's own code ([`ViewportCache`]: the same eligibility, size estimate and placement rules,
//! the same 4 GB cap, phase A on the viewport's tracing threads). The Ray-Traced picture stays on
//! screen meanwhile. While the cache exists, a change of the LIGHTING only (the preset, the light
//! direction, the exposure, the backdrop, the glare, the head shadow or the HDR map: everything
//! `RenderContext::pose_generation` leaves out) shows [`ReshadeView::relight`] at once instead of
//! a new Ray-Traced trace. Any other change (the pose, the design, the material, the size, the
//! engine) drops it, as does leaving the final phase or switching the preference off. Nothing is
//! ever written to disk.
//!
//! # What is drawn
//!
//! The stone pixels the cache covers come from `path_cache::reshade` (the path tracer's physics,
//! evaluated on a fixed grid, no noise), scaled to the sum of the frame's taps the display
//! expects. Every other pixel (the background, a stone the cache cannot serve) is drawn by the
//! Ray-Traced engine as always.

use super::{
    engine::{self, EngineAccumulator, frame_scene},
    gpu_backend::BackendFrame,
    local_preview::RenderPhase,
    viewport_cache::{CacheWanted, VIEWPORT_CACHE_AFTER_SPP, ViewportCache},
};
use glam::Vec3;
use pleochroma::{
    geometry::{plane::GpuFacetPlane, tool::ToolPrimitive},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, FacetFinish},
    },
    renderer::{
        engine::EngineSpec,
        env_map::EnvironmentMap,
        path_cache::{ReshadeError, reshade},
        tonemap::live_cpu_budget,
    },
};
use pleochroma_net::scene::effects::SceneEffects;
use std::sync::{Arc, atomic::AtomicBool};

/// Whether the feature can work at all this frame: the preference is on, the engine is the
/// Ray-Traced one and no remote lane is involved.
#[must_use]
pub(super) const fn possible(enabled: bool, ray_traced: bool, remote: bool) -> bool {
    enabled && ray_traced && !remote
}

/// Whether the view is in the phase that holds a cache: the final one.
#[must_use]
pub(super) const fn in_final(phase: RenderPhase) -> bool {
    matches!(phase, RenderPhase::Final)
}

/// What [`ReshadeView::sync`] needs to know about this frame.
#[derive(Clone, Copy, Debug)]
pub(super) struct ReshadeWanted {
    /// [`possible`] and the final phase.
    pub(super) applies: bool,
    /// `RenderContext::pose_generation` of the frame.
    pub(super) pose: u64,
    /// The Ray-Traced picture of this pose is finished (all its taps are in).
    pub(super) converged: bool,
    /// The frame size.
    pub(super) size: (u32, u32),
}

/// How a re-light ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Relight {
    /// `accum` holds the re-lit picture, as the sum of `taps` taps.
    Done,
    /// The frame's cancellation fired; `accum` is partly written.
    Cancelled,
    /// There is nothing to re-light from (no cache, or the engine declined); `accum` is
    /// untouched, and the ordinary Ray-Traced pass runs.
    Unavailable,
}

/// The frame a Ray-Traced view traces, before its effects and sample range are applied.
pub(super) struct RayView<'a> {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) distance: f32,
    pub(super) camera: &'a Camera,
    pub(super) planes: &'a [GpuFacetPlane],
    pub(super) tools: &'a [ToolPrimitive],
    pub(super) fluorescence: &'a Fluorescence,
    pub(super) facet_finishes: &'a [FacetFinish],
    pub(super) material: &'a GemMaterial,
    pub(super) max_bounces: u32,
    pub(super) environment: EnvironmentSource<'a>,
}

impl RayView<'_> {
    /// Runs `run` with the backend frame of this view for `spp` taps from `sample_offset`. The
    /// Ray-Traced engine ignores the design's render physics, so the frame carries none, exactly
    /// as the live loop builds it for that engine.
    pub(super) fn with_frame<R>(
        &self,
        (spp, sample_offset): (u32, u32),
        run: impl FnOnce(&BackendFrame<'_>) -> R,
    ) -> R {
        let no_effects = SceneEffects::default();
        let material = no_effects.apply_to_material(self.material);
        let surfaces = no_effects.surfaces();
        let frame = BackendFrame {
            width: self.width,
            height: self.height,
            yaw: self.yaw,
            pitch: self.pitch,
            distance: self.distance,
            camera: self.camera,
            planes: self.planes,
            tools: self.tools,
            fluorescence: self.fluorescence,
            facet_finishes: self.facet_finishes,
            surface: no_effects.surface_scene(&surfaces),
            material: &material,
            max_bounces: self.max_bounces,
            environment: self.environment,
            spp,
            sample_offset,
        };
        run(&frame)
    }
}

/// What a re-light needs besides the view.
pub(super) struct RelightJob<'a> {
    /// The image engine (the Ray-Traced taps and Fire).
    pub(super) engine: EngineSpec,
    /// The scene generation of the frame, which keys the Ray-Traced engine's prepared scene.
    pub(super) generation: u64,
    /// The loaded HDR map, if the lighting is one.
    pub(super) shared_hdr: Option<&'a Arc<EnvironmentMap>>,
    /// The taps of the finished picture: the count the display divides by.
    pub(super) taps: u32,
}

/// The viewport's instant-lighting cache.
#[derive(Default)]
pub(super) struct ReshadeView {
    cache: ViewportCache,
    /// The pose generation of the last frame that was synced.
    pose: Option<u64>,
}

impl ReshadeView {
    /// Whether a cache of exactly this pose is held.
    #[must_use]
    pub(super) const fn holds(&self, pose: u64) -> bool {
        self.cache.is_built() && matches!(self.pose, Some(held) if held == pose)
    }

    /// Whether any cache is held.
    #[cfg(test)]
    #[must_use]
    pub(super) const fn is_built(&self) -> bool {
        self.cache.is_built()
    }

    /// The bytes of the built tiles; zero without a cache.
    #[cfg(test)]
    pub(super) const fn built_bytes(&self) -> u64 {
        self.cache.built_bytes()
    }

    /// Whether the stone was refused for this pose.
    #[cfg(test)]
    pub(super) const fn is_refused(&self) -> bool {
        self.cache.is_refused()
    }

    /// Brings the cache in step with this frame and says whether it should be built now: the view
    /// applies, its picture is finished, the stone was not refused and nothing is built yet.
    ///
    /// A different pose, a new size or a frame the feature does not apply to drops whatever was
    /// built, and a refusal with it. A change of lighting alone is the same pose: it keeps both.
    pub(super) fn sync(&mut self, want: ReshadeWanted) -> bool {
        let pose_changed = self.pose != Some(want.pose);
        self.pose = Some(want.pose);
        let wanted = self.cache.sync(CacheWanted {
            applies: want.applies,
            reset: pose_changed,
            samples: if want.converged {
                VIEWPORT_CACHE_AFTER_SPP
            } else {
                0
            },
            size: want.size,
        });
        wanted && !self.cache.is_built()
    }

    /// Builds the cache of the view's pose (phase A of every tile). Returns whether one is built;
    /// a raised `cancel` ends the build and leaves nothing, a stone that cannot use the cache or
    /// does not fit is refused until the pose changes.
    pub(super) fn build(&mut self, view: &RayView<'_>, cancel: &AtomicBool) -> bool {
        view.with_frame((1, 0), |frame| self.cache.ensure_built(frame, cancel))
    }

    /// Writes the picture of `view` under its (new) lighting into `accum`, as the sum of
    /// `job.taps` taps: the stone from the stored light paths, every other pixel from the
    /// Ray-Traced engine.
    pub(super) fn relight(
        &mut self,
        view: &RayView<'_>,
        ray_engine: &mut EngineAccumulator,
        job: &RelightJob<'_>,
        accum: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> Relight {
        let taps = job.taps.max(1);
        view.with_frame((taps, 0), |frame| {
            let shaded = self.cache.with_pass_frame(frame, |cache| {
                let cache = cache?;
                let _lease = live_cpu_budget::single_core_lease();
                let _trace_lease = live_cpu_budget::trace_lease();
                let threads = live_cpu_budget::partition().0.max(1);
                Some(reshade(cache, &frame_scene(frame), threads, cancel))
            });
            let image = match shaded {
                Some(Ok(image)) if image.xyz.len() == accum.len() => image,
                Some(Err(ReshadeError::Cancelled)) => return Relight::Cancelled,
                _ => return Relight::Unavailable,
            };
            let outcome = engine::accumulate_uncovered(
                ray_engine,
                job.engine,
                (job.generation, job.shared_hdr),
                frame,
                (&image.covered, &mut *accum),
                cancel,
            );
            match outcome {
                Ok(true) => {}
                Ok(false) => return Relight::Cancelled,
                Err(reason) => {
                    tracing::debug!(
                        reason = reason.message(),
                        "instant lighting: engine declined"
                    );
                    return Relight::Unavailable;
                }
            }
            // A tap count is a handful (the engine's `aa_taps` is a `u8`).
            let scale = f32::from(u16::try_from(taps).unwrap_or(u16::MAX));
            for ((slot, xyz), &covered) in accum.iter_mut().zip(&image.xyz).zip(&image.covered) {
                if covered {
                    *slot = *xyz * scale;
                }
            }
            Relight::Done
        })
    }
}

#[cfg(test)]
mod tests;
