//! Instant lighting changes with beams in the Ray-Traced view (beam guide section 14.10,
//! Preferences > Rendering > "Instant Lighting Changes With Beams"): the stone drawn from the kept
//! beam leaves, deterministic and noise-free, over the Whitted engine's own background.
//!
//! # When it runs
//!
//! While the stone moves there is nothing. When a still Ray-Traced view (the final phase, no
//! remote lane) has finished its taps and the stone is eligible (`BeamFrame::new` accepts
//! it, with no tools, fluorescence or surface finish), [`RayBeams`] starts one background job. The
//! job calls `LeafCache::relight_ray_traced` with the finished Whitted picture as the background.
//! The first call of a pose builds the stone's leaves itself (the tree of every tile, retained up
//! to `VIEWPORT_LEAF_CACHE_BYTES`, with no residual: the Ray-Traced view has none); every later
//! call of the pose only refines, shades and composes them.
//!
//! # A lighting-only change
//!
//! A new scene generation on the same pose (the preset, the light, the exposure, the backdrop, the
//! glare, the head shadow, the HDR map) keeps the leaves, drops the shown picture and cancels the
//! job. The Ray-Traced picture of the new lighting then finishes normally (when the
//! "Instant Lighting Changes in the Ray-Traced View" Preference is on, that is the re-light of the
//! stored light paths, at once), and its sum of taps is the background of the next call.
//!
//! # What is shown
//!
//! The whole frame the call returns, as a running sum of one sample. Inside the stone it is the
//! beams; at the silhouette and outside it is the Whitted engine's shading of the uncovered share.
//! Any pose, design, material, resolution or engine change, a preview phase, a remote lane or the
//! Preference going away cancels the job at once and drops the leaves. With the Preference off
//! [`RayBeams`] stays inert: no thread, no overlay and no change to the display path. Nothing is
//! ever written to disk.

use super::{
    gpu_backend::BackendFrame,
    local_preview::RenderPhase,
    still_beams::{
        BeamJobInputs, Overlay, Tokens, lock, viewport_beam_settings, with_combined_token,
    },
};
use glam::Vec3;
use pleochroma::renderer::{
    accel_beam::{BeamOff, VIEWPORT_LEAF_CACHE_BYTES},
    beam::{leaf_cache::LeafCache, pipeline::with_thread_limit},
    env_map::EnvironmentMap,
    tonemap::live_cpu_budget,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

/// Whether the Ray-Traced view may use beams this frame: the Preference is on, the engine is the
/// Ray-Traced one, no remote lane is involved and the view is still (the final phase).
#[must_use]
pub(super) const fn applies(
    enabled: bool,
    ray_traced: bool,
    remote: bool,
    phase: RenderPhase,
) -> bool {
    enabled && ray_traced && !remote && matches!(phase, RenderPhase::Final)
}

/// What [`RayBeams::sync`] needs to know about this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RayBeamWanted {
    /// [`applies`] for this frame, and the viewport is not suspended.
    pub(super) applies: bool,
    /// `RenderContext::pose_generation`: everything but the lighting.
    pub(super) pose: u64,
    /// `RenderContext::scene_generation`: the pose and the lighting.
    pub(super) scene: u64,
    /// The frame size.
    pub(super) size: (u32, u32),
    /// The Ray-Traced picture of this scene is finished (all its taps are in).
    pub(super) converged: bool,
}

/// What the render loop should do with the beam job this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RayBeamStep {
    /// Nothing: no job is due and the Ray-Traced picture carries on.
    Idle,
    /// Build the leaves of this pose and show the stone from them.
    Build,
    /// Show the stone from the kept leaves under the new lighting.
    Relight,
}

/// The step of a due job: a re-light when the leaves of this pose are kept, a build otherwise.
#[must_use]
pub(super) const fn due_step(cache_built: bool) -> RayBeamStep {
    if cache_built {
        RayBeamStep::Relight
    } else {
        RayBeamStep::Build
    }
}

/// The Whitted image of a finished Ray-Traced picture: its sum of `taps` taps as a mean.
#[must_use]
pub(super) fn whitted_image(accum: &[Vec3], taps: u32) -> Vec<Vec3> {
    // A tap count is a handful (the engine's `aa_taps` is a `u8`).
    let scale = 1.0 / f32::from(u16::try_from(taps.max(1)).unwrap_or(u16::MAX));
    accum.iter().map(|sum| *sum * scale).collect()
}

/// What a job did.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The whole frame is in the overlay.
    Done,
    /// The leaf cache refused the scene; the Ray-Traced picture carries on.
    Declined,
    /// The cancel token was raised.
    Cancelled,
}

/// A running background job.
struct Job {
    /// The job's own token, raised by the render loop.
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<Outcome>,
}

/// Where the beams of the current pose stand.
enum Stage {
    /// No job and nothing shown.
    Idle,
    /// A job is running; nothing is shown yet.
    Running(Job),
    /// The whole frame is in the overlay.
    Shown,
    /// Refused for this scene: an ineligible stone or a declined cache.
    Refused,
}

/// What [`RayBeams::launch`] reads from the render loop besides the frame.
pub(super) struct RayLaunch<'a> {
    /// The loaded HDR panorama, when there is one.
    pub(super) hdr: Option<&'a Arc<EnvironmentMap>>,
    /// The frame's cancel token.
    pub(super) frame_cancel: &'a Arc<AtomicBool>,
    /// The running sum of the finished Ray-Traced picture.
    pub(super) accum: &'a [Vec3],
    /// The taps in `accum`.
    pub(super) taps: u32,
}

/// The beam job of the Ray-Traced view of the render loop.
pub(super) struct RayBeams {
    stage: Stage,
    /// The pose and the size the stage describes.
    key: Option<(u64, (u32, u32))>,
    /// The scene generation the stage describes.
    scene: u64,
    /// The leaves of this pose are retained and can be re-lit.
    cache_built: bool,
    overlay: Arc<Mutex<Overlay>>,
    cache: Arc<Mutex<LeafCache>>,
    /// The overlay version the render loop has seen.
    seen_version: u64,
    /// The picture [`Self::compose_into`] builds.
    composite: Vec<Vec3>,
}

impl Default for RayBeams {
    fn default() -> Self {
        Self {
            stage: Stage::Idle,
            key: None,
            scene: 0,
            cache_built: false,
            overlay: Arc::new(Mutex::new(Overlay::default())),
            cache: Arc::new(Mutex::new(LeafCache::new(VIEWPORT_LEAF_CACHE_BYTES))),
            seen_version: 0,
            composite: Vec::new(),
        }
    }
}

/// The body of a job: the stone from the leaves of `cache` under the lighting of `inputs`, over
/// `whitted`.
fn run_relight(
    inputs: &BeamJobInputs,
    whitted: &[Vec3],
    tokens: &Tokens<'_>,
    shared: (&Mutex<Overlay>, &Mutex<LeafCache>),
    generation: u64,
) -> Outcome {
    let (overlay, cache) = shared;
    let (pixels, cancelled) = with_combined_token(tokens, |cancel| {
        live_cpu_budget::with_limit(inputs.threads(), || {
            // The beam pipeline's own worker count follows the same quota.
            with_thread_limit(inputs.threads(), || {
                inputs.with_scene(|scene| {
                    lock(cache).relight_ray_traced(
                        scene,
                        &viewport_beam_settings(),
                        whitted,
                        cancel,
                    )
                })
            })
        })
    });
    match pixels {
        _ if cancelled => Outcome::Cancelled,
        Some(pixels) => {
            lock(overlay).write_frame(generation, &pixels);
            Outcome::Done
        }
        None => Outcome::Declined,
    }
}

impl RayBeams {
    /// Brings the job in step with this frame and says what the loop should start.
    ///
    /// Call once per frame. A frame the beams do not apply to, a new pose or size drops the job,
    /// the picture and the leaves at once; a lighting-only change cancels the job and drops the
    /// picture but keeps the leaves. The step repeats every frame until [`Self::launch`] has taken
    /// it.
    pub(super) fn sync(&mut self, want: RayBeamWanted) -> RayBeamStep {
        self.collect();
        if !want.applies {
            self.drop_all();
            return RayBeamStep::Idle;
        }
        let key = (want.pose, want.size);
        if self.key != Some(key) {
            self.drop_all();
            self.key = Some(key);
            self.scene = want.scene;
            lock(&self.overlay).reset(want.size);
        } else if self.scene != want.scene {
            self.scene = want.scene;
            self.on_lighting_change();
        }
        match self.stage {
            Stage::Idle if want.converged => due_step(self.cache_built),
            _ => RayBeamStep::Idle,
        }
    }

    /// Whether the picture of the beams is shown.
    pub(super) const fn shows(&self) -> bool {
        matches!(self.stage, Stage::Shown)
    }

    /// Whether the overlay changed since the last call: the picture needs showing again.
    pub(super) fn take_news(&mut self) -> bool {
        let version = lock(&self.overlay).version;
        let news = version != self.seen_version;
        self.seen_version = version;
        news
    }

    /// Builds the picture to show and says whether there is one; read it with [`Self::composite`].
    /// `false` leaves the Ray-Traced picture to show.
    pub(super) fn compose_into(&mut self, accum: &[Vec3], samples: u32) -> bool {
        lock(&self.overlay).compose(accum, samples, &mut self.composite)
    }

    /// The picture [`Self::compose_into`] built, a running sum of one sample.
    pub(super) fn composite(&self) -> &[Vec3] {
        &self.composite
    }

    /// Starts the job of `step` for `frame`, when the stone is beam traced; a stone that is not is
    /// refused until the pose or the lighting changes, with the Ray-Traced picture carrying on.
    pub(super) fn launch(
        &mut self,
        step: RayBeamStep,
        frame: &BackendFrame<'_>,
        with: &RayLaunch<'_>,
    ) {
        if step == RayBeamStep::Idle || !matches!(self.stage, Stage::Idle) {
            return;
        }
        // The Ray-Traced picture is finished, so the viewport's tracing threads are idle.
        let threads = live_cpu_budget::partition().0.max(1);
        let inputs = match BeamJobInputs::capture(frame, with.hdr, with.taps, threads) {
            Ok(inputs) => inputs,
            Err(off) => {
                self.refuse(off);
                return;
            }
        };
        let whitted = whitted_image(with.accum, with.taps);
        let generation = lock(&self.overlay).generation;
        let overlay = Arc::clone(&self.overlay);
        let cache = Arc::clone(&self.cache);
        let frame_cancel = Arc::clone(with.frame_cancel);
        self.spawn_job(move |own| {
            let tokens = Tokens {
                frame: &frame_cancel,
                own,
            };
            run_relight(&inputs, &whitted, &tokens, (&overlay, &cache), generation)
        });
    }

    /// Runs `run` on a background thread as the job of this scene.
    fn spawn_job<F>(&mut self, run: F)
    where
        F: FnOnce(&AtomicBool) -> Outcome + Send + 'static,
    {
        let cancel = Arc::new(AtomicBool::new(false));
        let own = Arc::clone(&cancel);
        let spawned = thread::Builder::new()
            .name("ray-beams".to_owned())
            .spawn(move || run(own.as_ref()));
        match spawned {
            Ok(handle) => self.stage = Stage::Running(Job { cancel, handle }),
            Err(error) => {
                tracing::warn!(%error, "ray-traced beams: the job thread did not start");
                self.stage = Stage::Refused;
            }
        }
    }

    /// Adopts the outcome of a job that has ended by itself.
    fn collect(&mut self) {
        let finished = matches!(&self.stage, Stage::Running(job) if job.handle.is_finished());
        if !finished {
            return;
        }
        let Stage::Running(job) = std::mem::replace(&mut self.stage, Stage::Idle) else {
            return;
        };
        match job.handle.join() {
            Ok(Outcome::Done) => {
                self.stage = Stage::Shown;
                self.cache_built = self.cache_built || !lock(&self.cache).is_empty();
            }
            Ok(Outcome::Declined) => {
                tracing::debug!("ray-traced beams: the leaf cache declined the scene");
                self.clear_overlay();
                self.stage = Stage::Refused;
            }
            Ok(Outcome::Cancelled) => {
                // A token the GUI raised for an edit that left the scene alone: start again.
                self.clear_overlay();
            }
            Err(_) => {
                tracing::warn!("ray-traced beams: the job thread panicked");
                self.clear_overlay();
                self.stage = Stage::Refused;
            }
        }
    }

    /// A lighting-only change: stops the job and drops the stale picture; the leaves stay.
    fn on_lighting_change(&mut self) {
        self.cancel_job();
        self.clear_overlay();
        self.stage = Stage::Idle;
    }

    /// Raises the token of a running job and lets go of it.
    fn cancel_job(&mut self) {
        if let Stage::Running(job) = &self.stage {
            job.cancel.store(true, Ordering::Release);
            self.stage = Stage::Idle;
        }
    }

    /// Refuses the scene for the cache's reason.
    fn refuse(&mut self, off: BeamOff) {
        if off.is_silent() {
            tracing::debug!("ray-traced beams: stone out of scope");
        } else {
            tracing::debug!(reason = off.message(), "ray-traced beams: refused");
        }
        self.clear_overlay();
        self.stage = Stage::Refused;
    }

    /// Empties the overlay (ending the writes of any job still winding down).
    fn clear_overlay(&self) {
        let mut overlay = lock(&self.overlay);
        let size = overlay.size();
        overlay.reset(size);
    }

    /// Stops everything and forgets the pose: the view is not one beams apply to.
    ///
    /// The old leaf cache is left to the job that may still hold it (it is let go, not locked).
    fn drop_all(&mut self) {
        let idle = self.key.is_none() && matches!(self.stage, Stage::Idle) && !self.cache_built;
        if idle {
            return;
        }
        self.cancel_job();
        self.stage = Stage::Idle;
        self.key = None;
        self.cache_built = false;
        self.clear_overlay();
        self.cache = Arc::new(Mutex::new(LeafCache::new(VIEWPORT_LEAF_CACHE_BYTES)));
    }

    /// Whether a job is running.
    #[cfg(test)]
    #[cfg(all(feature = "beam", beam_tests))]
    const fn is_running(&self) -> bool {
        matches!(self.stage, Stage::Running(_))
    }

    /// Whether the scene was refused.
    #[cfg(test)]
    #[cfg(all(feature = "beam", beam_tests))]
    const fn is_refused(&self) -> bool {
        matches!(self.stage, Stage::Refused)
    }

    /// Whether nothing is held: no job, no pose, no overlay, no leaves.
    #[cfg(test)]
    #[cfg(all(feature = "beam", beam_tests))]
    fn is_inactive(&self) -> bool {
        matches!(self.stage, Stage::Idle)
            && self.key.is_none()
            && !self.cache_built
            && lock(&self.overlay).filled == 0
    }

    /// The cancel token of the running job.
    #[cfg(test)]
    #[cfg(all(feature = "beam", beam_tests))]
    fn job_token(&self) -> Option<Arc<AtomicBool>> {
        match &self.stage {
            Stage::Running(job) => Some(Arc::clone(&job.cancel)),
            _ => None,
        }
    }

    /// The cache handle the next job would be launched with.
    #[cfg(test)]
    #[cfg(all(feature = "beam", beam_tests))]
    fn cache_handle(&self) -> Arc<Mutex<LeafCache>> {
        Arc::clone(&self.cache)
    }

    /// Pretends the leaves of this pose are retained (the fake jobs of the tests keep nothing).
    #[cfg(test)]
    #[cfg(all(feature = "beam", beam_tests))]
    const fn mark_cache_built(&mut self) {
        self.cache_built = true;
    }
}

#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests;
