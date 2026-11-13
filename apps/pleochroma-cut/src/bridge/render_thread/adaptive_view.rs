//! Adaptive sampling in the live viewport (Preferences > Rendering > "Adaptive Sampling in the
//! Viewport"). Compiled only with the optional `adaptive-sampling` feature; a build without it
//! never starts a run, whatever the saved switch says.
//!
//! # What it does
//!
//! While the stone stands still and the viewport refines progressively, the picture normally
//! gives every pixel the same number of samples, up to the viewport's sample cap. With the
//! preference on, the first pass of a still view is the ordinary one-sample pass (it produces the
//! first image and the denoiser's guide buffers); from then on the view runs an
//! [`AdaptiveRun`]: passes of 16 samples per pixel over the *live* tiles only, a tile stopping as
//! soon as its two half-images agree (`TARGET_NOISE_PRESETS[DEFAULT_TARGET_NOISE_PRESET]`, the
//! "Standard" target, after [`VIEWPORT_MIN_SPP`] samples). The samples that would have gone to
//! clean tiles go to the noisy ones, so the picture cleans up sooner.
//!
//! # What is shown
//!
//! Each pixel shows its own mean (its sum divided by its own sample count, `AdaptiveRun::resolve`),
//! handed to the display thread as a running sum of *one* sample, so the denoiser and the
//! tone-map see exactly what they see today. The progress bar shows the samples per pixel the live
//! tiles have, over the viewport's cap; when every tile has stopped (or the cap is reached) it
//! shows the full cap, which reads "Complete".
//!
//! # When a run starts and ends
//!
//! A run lives exactly as long as one accumulation: any reset of the progressive accumulation (a
//! pose, design, material, lighting, exposure, resolution or engine change, a preview phase, the
//! preference itself) drops it, and the next still frame starts a new one from zero. A change of
//! the viewport's sample cap restarts it too. The Ray-Traced engine, the camera preview phases, a
//! remote lane (an epoch the viewport combines with, a remote that owns the image) and a cap below
//! [`MIN_ADAPTIVE_CAP`] keep the ordinary accumulation.
//!
//! # One engine per pass
//!
//! A pass runs on the GPU when the compute choice allows it and the backend takes it, and on the
//! CPU otherwise. The viewport's CPU+GPU split (`live_split`, the hybrid frame) is not used for
//! adaptive passes: a pass is traced by one engine. When the GPU declines a pass, the rest of the
//! run stays on the CPU.

mod cpu_pass;
#[cfg(test)]
mod tests;

use super::{
    gpu_backend::{BackendFrame, ViewportGpu},
    local_preview::RenderPhase,
};
use crate::settings::model::LocalComputeTarget;
use glam::Vec3;
use pleochroma::renderer::{
    adaptive::{
        AdaptiveRun, AdaptiveSettings, AdaptiveSettingsError, DEFAULT_TARGET_NOISE_PRESET,
        TARGET_NOISE_PRESETS,
    },
    gpu_backend::GpuAccumulate,
    path_cache::PathCacheFrame,
};
use std::sync::atomic::AtomicBool;

/// No tile of the viewport stops before this many samples per pixel. A viewport refines under
/// a cap of a few hundred, so the check starts early.
pub const VIEWPORT_MIN_SPP: u32 = 32;

/// The smallest viewport sample cap that runs adaptively. Below it, the minimum leaves too little
/// to save and the ordinary accumulation is the cheaper way to finish.
pub const MIN_ADAPTIVE_CAP: u32 = 64;

/// The scale of the convergence check's tone map: 1, because the renderer's values already hold
/// the viewport's exposure (it sits in the lighting rig) and the display tone-map applies no
/// exposure of its own.
const CHECK_SCALE: f32 = 1.0;

/// The adaptive settings of a viewport capped at `cap` samples per pixel: the provisional
/// defaults (relative `L*`, 16-sample passes, 16-pixel tiles) with `target_noise` and a minimum
/// of [`VIEWPORT_MIN_SPP`] (less for a tiny cap).
#[must_use]
pub fn viewport_settings(cap: u32, target_noise: f32) -> AdaptiveSettings {
    let base = AdaptiveSettings::provisional(cap);
    let min_spp = (VIEWPORT_MIN_SPP.min(cap) / base.pass_spp * base.pass_spp).max(base.pass_spp);
    AdaptiveSettings {
        target_noise,
        min_spp,
        ..base
    }
}

/// The relative `L*` target of the "Standard" preset, what the viewport uses.
#[must_use]
pub const fn standard_target_noise() -> f32 {
    TARGET_NOISE_PRESETS[DEFAULT_TARGET_NOISE_PRESET].1
}

/// Whether the viewport samples adaptively this frame: the preference is on, the engine is the
/// path tracer, no remote lane is involved, the view is still (the final phase) and the cap is at
/// least [`MIN_ADAPTIVE_CAP`].
#[must_use]
pub(super) const fn applies(
    enabled: bool,
    ray_traced: bool,
    remote: bool,
    phase: RenderPhase,
    cap: u32,
) -> bool {
    enabled
        && !ray_traced
        && !remote
        && matches!(phase, RenderPhase::Final)
        && cap >= MIN_ADAPTIVE_CAP
}

/// What [`AdaptiveView::sync`] needs to know about this frame.
#[derive(Clone, Copy, Debug)]
pub(super) struct Wanted {
    /// [`applies`] for this frame.
    pub(super) applies: bool,
    /// The progressive accumulation was reset this frame (a scene change or a new size).
    pub(super) reset: bool,
    /// The ordinary first pass of this accumulation is done (its image and guides exist).
    pub(super) first_pass_done: bool,
    /// The frame size.
    pub(super) size: (u32, u32),
    /// The viewport's sample cap.
    pub(super) cap: u32,
}

/// How one adaptive pass ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PassOutcome {
    /// The pass was traced and the convergence check ran.
    Traced,
    /// The run had no pass left.
    Finished,
    /// The frame's cancellation fired; the buffers are untouched.
    Cancelled,
}

/// One engaged run with its buffers.
struct Engaged {
    run: AdaptiveRun,
    size: (u32, u32),
    cap: u32,
    sum: Vec<Vec3>,
    half_b: Vec<Vec3>,
    /// Each pixel's sum over its own count; empty until the first pass.
    means: Vec<Vec3>,
    /// `false` once the GPU declined a pass of this run.
    gpu_usable: bool,
}

impl Engaged {
    fn start(
        target_noise: f32,
        (width, height): (u32, u32),
        cap: u32,
    ) -> Result<Self, AdaptiveSettingsError> {
        let run = AdaptiveRun::new(viewport_settings(cap, target_noise), width, height)?;
        let pixels = width as usize * height as usize;
        Ok(Self {
            run,
            size: (width, height),
            cap,
            sum: vec![Vec3::ZERO; pixels],
            half_b: vec![Vec3::ZERO; pixels],
            means: Vec::new(),
            gpu_usable: true,
        })
    }

    /// Every tile has stopped, or the cap is reached.
    fn finished(&self) -> bool {
        self.run.samples_done() >= self.cap || self.run.map().is_done()
    }
}

/// The adaptive state of the viewport's render loop.
pub(super) struct AdaptiveView {
    engaged: Option<Engaged>,
    /// The settings were refused for this accumulation; the ordinary one carries on.
    refused: bool,
    target_noise: f32,
}

impl Default for AdaptiveView {
    fn default() -> Self {
        Self::with_target_noise(standard_target_noise())
    }
}

impl AdaptiveView {
    /// A view whose tiles stop at `target_noise` (relative `L*`).
    pub(super) const fn with_target_noise(target_noise: f32) -> Self {
        Self {
            engaged: None,
            refused: false,
            target_noise,
        }
    }

    /// Brings the run in step with this frame and says whether the adaptive path drives it.
    ///
    /// Call once per frame, after the progressive accumulation was reset or kept. A reset (or a
    /// frame the adaptive path does not apply to) drops the run; a run starts on the first frame
    /// after the ordinary first pass; a changed cap or size restarts it.
    pub(super) fn sync(&mut self, want: Wanted) -> bool {
        if !want.applies || want.reset {
            self.engaged = None;
            self.refused = false;
        }
        if !want.applies {
            return false;
        }
        if self
            .engaged
            .as_ref()
            .is_some_and(|engaged| engaged.cap != want.cap || engaged.size != want.size)
        {
            self.engaged = None;
        }
        if self.engaged.is_some() {
            return true;
        }
        if self.refused || !want.first_pass_done {
            return false;
        }
        match Engaged::start(self.target_noise, want.size, want.cap) {
            Ok(engaged) => {
                self.engaged = Some(engaged);
                true
            }
            Err(error) => {
                tracing::warn!(%error, "adaptive viewport sampling refused; sampling as usual");
                self.refused = true;
                false
            }
        }
    }

    /// Whether a run is engaged.
    #[cfg(test)]
    pub(super) const fn is_engaged(&self) -> bool {
        self.engaged.is_some()
    }

    /// Every tile has stopped, or the cap is reached (`false` without a run).
    pub(super) fn is_finished(&self) -> bool {
        self.engaged.as_ref().is_some_and(Engaged::finished)
    }

    /// The samples per pixel to report against the viewport's cap: the live tiles' count, the full
    /// cap once the run is finished, and at least one (the ordinary first pass).
    pub(super) fn samples_shown(&self) -> u32 {
        self.engaged.as_ref().map_or(0, |engaged| {
            if engaged.finished() {
                engaged.cap
            } else {
                engaged.run.samples_done().max(1)
            }
        })
    }

    /// Each pixel's own mean, `None` until the run has traced a pass (the ordinary image is shown
    /// until then).
    pub(super) fn means(&self) -> Option<&[Vec3]> {
        self.engaged
            .as_ref()
            .map(|engaged| engaged.means.as_slice())
            .filter(|means| !means.is_empty())
    }

    /// Traces the next pass of the run for `frame` with one engine: the GPU when `compute` allows
    /// it, the CPU otherwise. With the viewport's path cache (`Some`) the stone pixels of the live
    /// tiles shade from the cache, on the GPU's phase B kernel or the CPU; `None` is the plain
    /// pass.
    pub(super) fn trace_pass_cached(
        &mut self,
        backend: &mut ViewportGpu,
        compute: LocalComputeTarget,
        frame: &BackendFrame<'_>,
        cache: Option<&PathCacheFrame<'_, '_>>,
        cancel: &AtomicBool,
    ) -> PassOutcome {
        // An explicit GPU choice waits for the shader; the combined choice never holds up a pass
        // for it and traces on the CPU until the GPU is ready.
        let gpu = match compute {
            LocalComputeTarget::Cpu => None,
            LocalComputeTarget::Gpu => Some((backend, true)),
            LocalComputeTarget::CpuGpu => Some((backend, false)),
        };
        self.trace_pass_on_cached(gpu, frame, cache, cancel)
    }

    /// Traces the next pass without a cache; see [`Self::trace_pass_on_cached`].
    #[cfg(test)]
    pub(super) fn trace_pass_on(
        &mut self,
        gpu: Option<(&mut ViewportGpu, bool)>,
        frame: &BackendFrame<'_>,
        cancel: &AtomicBool,
    ) -> PassOutcome {
        self.trace_pass_on_cached(gpu, frame, None, cancel)
    }

    /// Traces the next pass: on the GPU when `gpu` is given (with whether to wait for its
    /// preparation) and it takes the pass, else on the CPU; then runs the convergence check and
    /// refreshes the per-pixel means. With `cache` the stone pixels shade from the path cache.
    pub(super) fn trace_pass_on_cached(
        &mut self,
        gpu: Option<(&mut ViewportGpu, bool)>,
        frame: &BackendFrame<'_>,
        cache: Option<&PathCacheFrame<'_, '_>>,
        cancel: &AtomicBool,
    ) -> PassOutcome {
        let Some(engaged) = self.engaged.as_mut() else {
            return PassOutcome::Finished;
        };
        let Some(pass) = engaged.run.next_pass() else {
            return PassOutcome::Finished;
        };
        let mut traced = false;
        if engaged.gpu_usable
            && let Some((backend, wait)) = gpu
        {
            let buffers = (engaged.sum.as_mut_slice(), engaged.half_b.as_mut_slice());
            let outcome = match cache {
                Some(cache) => backend.try_accumulate_cached_live_cancellable(
                    frame,
                    (pass.first_sample, pass.spp),
                    &pass.live,
                    cache,
                    buffers,
                    wait,
                    cancel,
                ),
                None => {
                    backend.try_accumulate_live_cancellable(frame, &pass, buffers, wait, cancel)
                }
            };
            match outcome {
                Some(GpuAccumulate::Done) => traced = true,
                Some(GpuAccumulate::Cancelled) => return PassOutcome::Cancelled,
                Some(GpuAccumulate::Declined) => engaged.gpu_usable = false,
                // The combined choice's GPU is still preparing: this pass is the CPU's.
                None => {}
            }
        }
        if !traced
            && cpu_pass::trace_live_pass_cached(
                frame,
                &pass,
                (&mut engaged.sum, &mut engaged.half_b),
                cache,
                cancel,
            )
        {
            return PassOutcome::Cancelled;
        }
        engaged
            .run
            .finish_pass(&engaged.sum, &engaged.half_b, CHECK_SCALE);
        engaged.means = engaged.run.resolve(&engaged.sum);
        PassOutcome::Traced
    }
}
