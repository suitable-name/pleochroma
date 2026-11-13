//! Beam tracing in the live viewport (Preferences > Rendering > "Beam Tracing in Still Views"):
//! a background job that draws the stone of a still pose from beams.
//!
//! # When a job runs
//!
//! While the stone moves there is no job. When a still view (the final phase, the path-traced
//! engine, no remote lane) has refined for [`VIEWPORT_BEAMS_AFTER_SPP`] progressive samples and the
//! stone is eligible (`BeamFrame::accepts` passes it; no tools, fluorescence or surface finish),
//! [`StillBeams`] starts one background job that runs `render_beam_hybrid_surface` with `on_batch`.
//! The progressive loop keeps running meanwhile.
//!
//! # What is shown
//!
//! Every finished batch of tiles is copied into an overlay of final pixels (beam part plus residual
//! mean). The viewport shows the overlay where a batch has landed and the ordinary progressive image
//! elsewhere ([`StillBeams::compose_into`]), handed to the display thread as a running sum of one
//! sample, exactly as the adaptive viewport hands over its means. When the job ends, the whole
//! image the entry returned replaces the overlay.
//!
//! # When a job ends
//!
//! Any pose, design, material, resolution or engine change (a new `pose_generation` or frame size),
//! a preview phase, a remote lane, a different engine, or the preference going away cancels the job
//! at once through its cancel token, which the renderer polls per beam. The token is raised by the
//! render worker itself and, through a small watcher, by the frame's own token, which the GUI raises
//! the instant it edits the scene. A lighting-only change (a new scene generation on the same pose)
//! goes to `LeafCache::relight` when "Instant Lighting Changes" is on and the leaves of the finished
//! render are retained, and otherwise restarts the job.
//!
//! # CPU share
//!
//! The viewport's CPU passes hold the tracing lease and use the tracing partition of
//! `live_cpu_budget`. The job takes no lease: it limits its own threads to half of that partition
//! (`live_cpu_budget::with_limit`), so the progressive passes keep their cores and the job uses the
//! rest. On a machine with a single core no job is started.
//!
//! # Coexistence
//!
//! On an eligible stone beams win over the viewport path cache ([`StillBeams::owns_stone`]), and the
//! adaptive viewport keeps running underneath. With the preference off [`StillBeams`] stays inert:
//! no thread, no overlay and no change to the display path. Nothing is ever written to disk.

use super::{
    adaptive_view::{standard_target_noise, viewport_settings},
    engine::frame_scene,
    gpu_backend::BackendFrame,
    local_preview::RenderPhase,
};
use glam::Vec3;
use pleochroma::{
    geometry::plane::GpuFacetPlane,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            Camera, EnvironmentSource, FacetFinish, build_plane_soa, surface::SurfaceScene,
        },
    },
    renderer::{
        accel_beam::{BeamOff, BeamSettings, VIEWPORT_LEAF_CACHE_BYTES},
        adaptive::AdaptiveImage,
        beam::{
            BeamProgress, BeamRun,
            engines::EngineSet,
            frame::BeamFrame,
            leaf_cache::{LeafCache, RetainedBatch},
            pipeline::with_thread_limit,
            primary::TileRect,
            stats::BeamStats,
        },
        cpu_frame::render_beam_hybrid_surface,
        env_map::{EnvRotation, EnvironmentMap},
        frame_scene::FrameScene,
        path_cache::eligibility::{StoneTraits, check_eligibility},
        tonemap::live_cpu_budget,
    },
};
use std::{
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// Progressive samples a still pose needs before its beam job starts.
pub const VIEWPORT_BEAMS_AFTER_SPP: u32 = 32;

/// How often the watcher of a job looks at the frame's cancel token.
const WATCH_INTERVAL: Duration = Duration::from_millis(4);

/// The residual's convergence-check scale: 1, as the renderer's values already carry the
/// viewport's exposure.
const CHECK_SCALE: f32 = 1.0;

/// Whether the viewport may use beams this frame: the preference is on, the engine is the path
/// tracer, no remote lane is involved and the view is still (the final phase).
#[must_use]
pub(super) const fn applies(
    enabled: bool,
    ray_traced: bool,
    remote: bool,
    phase: RenderPhase,
) -> bool {
    enabled && !ray_traced && !remote && matches!(phase, RenderPhase::Final)
}

/// How a lighting-only change reaches the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LightingRoute {
    /// The retained leaves are re-lit.
    Relight,
    /// The job starts again.
    Restart,
}

/// The route of a lighting-only change: a re-light when "Instant Lighting Changes" is on and the
/// leaves of a finished render are retained, a restart otherwise.
#[must_use]
pub(super) const fn lighting_route(instant_lighting: bool, cache_ready: bool) -> LightingRoute {
    if instant_lighting && cache_ready {
        LightingRoute::Relight
    } else {
        LightingRoute::Restart
    }
}

/// What [`StillBeams::sync`] needs to know about this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BeamWanted {
    /// [`applies`] for this frame, and the viewport is not suspended.
    pub(super) applies: bool,
    /// `RenderContext::pose_generation`: everything but the lighting.
    pub(super) pose: u64,
    /// `RenderContext::scene_generation`: the pose and the lighting.
    pub(super) scene: u64,
    /// The frame size.
    pub(super) size: (u32, u32),
    /// The progressive samples the still pose has.
    pub(super) samples: u32,
    /// The beam "Instant Lighting Changes" preference.
    pub(super) instant_lighting: bool,
}

/// What the render loop should do with the beam job this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BeamStep {
    /// Nothing: no job is due and the ordinary image carries on.
    Idle,
    /// Start the render job of this pose.
    Start,
    /// Re-light the retained leaves under the new lighting.
    Relight,
}

/// What a job did.
#[derive(Debug)]
enum Outcome {
    /// The whole frame is in the overlay.
    Done,
    /// The entry refused the scene; the ordinary image carries on.
    Declined(BeamOff),
    /// The cancel token was raised.
    Cancelled,
}

/// What a job computes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JobKind {
    /// A full beam render.
    Render,
    /// A re-light of the retained leaves.
    Relight,
}

/// A running background job.
struct Job {
    kind: JobKind,
    /// The job's own token, raised by the render loop.
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<Outcome>,
}

/// Where the beams of the current pose stand.
enum Stage {
    /// No job and nothing shown.
    Idle,
    /// A job is running; the overlay fills batch by batch.
    Running(Job),
    /// The whole frame is in the overlay.
    Shown,
    /// Refused for this pose: ineligible stone, a declined entry or no spare core.
    Refused,
}

/// The final pixels of the batches that have landed.
#[derive(Default)]
pub(super) struct Overlay {
    /// Changes whenever the overlay is reset, so that a job started earlier cannot write into it.
    pub(super) generation: u64,
    /// Changes whenever a pixel or the clearing changes what the overlay shows.
    pub(super) version: u64,
    size: (u32, u32),
    pixels: Vec<Vec3>,
    done: Vec<bool>,
    /// The number of pixels set.
    pub(super) filled: usize,
}

impl Overlay {
    /// The frame size the overlay was last reset for.
    pub(super) const fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Empties the overlay for a frame of `size`, ending the writes of every earlier job.
    pub(super) fn reset(&mut self, size: (u32, u32)) {
        self.generation += 1;
        self.version += 1;
        self.size = size;
        self.pixels.clear();
        self.done.clear();
        self.filled = 0;
    }

    /// Allocates the pixel buffers on the first write.
    fn ensure(&mut self) {
        let count = self.size.0 as usize * self.size.1 as usize;
        if self.pixels.len() != count {
            self.pixels = vec![Vec3::ZERO; count];
            self.done = vec![false; count];
            self.filled = 0;
        }
    }

    /// Copies a batch (`tiles` in batch order, each tile row-major) into the overlay; ignored when
    /// `generation` is no longer the overlay's.
    fn write_batch(&mut self, generation: u64, tiles: &[TileRect], means: &[Vec3]) {
        if generation != self.generation {
            return;
        }
        self.ensure();
        let (width, height) = self.size;
        let mut at = 0_usize;
        for tile in tiles {
            for y in tile.y0..tile.y1 {
                for x in tile.x0..tile.x1 {
                    let mean = means.get(at).copied();
                    at += 1;
                    let (Some(mean), true) = (mean, x < width && y < height) else {
                        continue;
                    };
                    let index = y as usize * width as usize + x as usize;
                    self.pixels[index] = mean;
                    if !self.done[index] {
                        self.done[index] = true;
                        self.filled += 1;
                    }
                }
            }
        }
        self.version += 1;
    }

    /// Replaces the whole overlay by the finished frame; ignored when `generation` is no longer the
    /// overlay's or `pixels` is not a frame.
    pub(super) fn write_frame(&mut self, generation: u64, pixels: &[Vec3]) {
        let count = self.size.0 as usize * self.size.1 as usize;
        if generation != self.generation || pixels.len() != count {
            return;
        }
        self.pixels.clear();
        self.pixels.extend_from_slice(pixels);
        self.done.clear();
        self.done.resize(count, true);
        self.filled = count;
        self.version += 1;
    }

    /// The picture to show: the overlay where it is set, `accum / samples` elsewhere. `false` (and
    /// `out` untouched) when nothing is set or `accum` is not a frame of the overlay's size.
    pub(super) fn compose(&self, accum: &[Vec3], samples: u32, out: &mut Vec<Vec3>) -> bool {
        if self.filled == 0 || accum.len() != self.pixels.len() {
            return false;
        }
        let scale = 1.0 / samples.max(1) as f32;
        out.clear();
        out.extend(
            accum
                .iter()
                .zip(self.pixels.iter().zip(&self.done))
                .map(|(sum, (beam, done))| if *done { *beam } else { *sum * scale }),
        );
        true
    }
}

/// Locks `mutex`, recovering from a poisoned lock (the data are plain values).
pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The production beam settings of the viewport: the defaults with the viewport's leaf-cache cap.
#[must_use]
pub(super) fn viewport_beam_defaults() -> BeamSettings {
    BeamSettings {
        leaf_cache_bytes: VIEWPORT_LEAF_CACHE_BYTES,
        ..BeamSettings::default()
    }
}

/// The beam settings the viewport renders with: [`viewport_beam_defaults`].
///
/// In tests the tree is coarse (8 spectral nodes, 1 absorption sub-node, split depth 8, tolerances
/// 1e-2): the full tree costs minutes per render at the test profile, and the tests check the
/// job's structure, not the tree's accuracy. The leaf-cache cap stays the production one.
#[must_use]
pub(super) fn viewport_beam_settings() -> BeamSettings {
    let settings = viewport_beam_defaults();
    if cfg!(test) {
        BeamSettings {
            spectral_nodes: 8,
            absorption_subnodes: 1,
            max_split_depth: 8,
            dir_tolerance_rad: 1e-2,
            chord_tolerance_px: 1e-2,
            weight_tolerance: 1e-2,
            lighting_tolerance: 1e-2,
            ..settings
        }
    } else {
        settings
    }
}

/// The engines of a viewport beam job.
///
/// An empty set: the batch pipeline then shades and runs the residual on its own CPU engine, which
/// is what the viewport wants (its CPU permits come from the viewport's pool; no GPU contention
/// with the live passes).
#[must_use]
fn viewport_engines() -> EngineSet<'static> {
    EngineSet::new(Vec::new())
}

/// The threads a job may use, or `None` on a machine with a single core (the job would compete with
/// the viewport's only tracing core).
#[must_use]
fn spare_threads() -> Option<usize> {
    let cores = thread::available_parallelism().map_or(1, std::num::NonZero::get);
    (cores > 1).then(|| (live_cpu_budget::partition().0 / 2).max(1))
}

/// Whether the stone of `frame` is beam traced: the viewport frame passes the path-cache
/// eligibility rules with its real tools, fluorescence and surface, and
/// `BeamFrame::accepts` passes it.
///
/// # Errors
///
/// The [`BeamOff`] reason.
fn check(frame: &BackendFrame<'_>) -> Result<(), BeamOff> {
    if frame.environment.tray_shadow().is_some() {
        return Err(BeamOff::StoneShadow);
    }
    if frame.environment.tray_mirror().is_some() {
        return Err(BeamOff::TrayReflection);
    }
    check_eligibility(&StoneTraits {
        material: frame.material,
        facet_finishes: frame.facet_finishes,
        tools: frame.tools,
        fluorescence: frame.fluorescence,
        surface: frame.surface,
        depth_of_field: false,
    })
    .map_err(BeamOff::NotEligible)?;
    // The frame's own refusals, checked without building it: the job builds it once, when it runs.
    BeamFrame::accepts(&frame_scene(frame))
}

/// The lighting of a job, owned.
enum JobLight {
    /// The analytic studio rig.
    Studio(EnvironmentSource<'static>),
    /// A loaded HDR panorama, the frame's exposure it is lit at and its turn.
    Hdr(Arc<EnvironmentMap>, f32, EnvRotation),
}

/// A camera, owned (`Camera` itself is not `Clone`).
struct CameraParts {
    origin: Vec3,
    forward: Vec3,
    right: Vec3,
    up: Vec3,
    fov_tan: f32,
}

impl CameraParts {
    const fn of(camera: &Camera) -> Self {
        Self {
            origin: camera.origin,
            forward: camera.forward,
            right: camera.right,
            up: camera.up,
            fov_tan: camera.fov_tan,
        }
    }

    const fn build(&self) -> Camera {
        Camera {
            origin: self.origin,
            forward: self.forward,
            right: self.right,
            up: self.up,
            fov_tan: self.fov_tan,
        }
    }
}

/// Everything a job reads, copied out of the render loop's frame so that the job outlives it.
pub(super) struct BeamJobInputs {
    size: (u32, u32),
    camera: CameraParts,
    planes: Vec<GpuFacetPlane>,
    finishes: Vec<FacetFinish>,
    material: GemMaterial,
    max_bounces: u32,
    light: JobLight,
    /// The viewport's sample cap, which bounds the residual's adaptive loop.
    cap: u32,
    /// The threads the job may use.
    threads: usize,
}

impl BeamJobInputs {
    /// The frame description of the job, drawn with `camera` (built by [`CameraParts::build`]).
    fn scene<'a>(&'a self, camera: &'a Camera) -> FrameScene<'a> {
        let environment = match &self.light {
            JobLight::Studio(studio) => *studio,
            JobLight::Hdr(map, exposure, rotation) => {
                EnvironmentSource::hdr_at(map, *exposure).with_hdr_rotation(*rotation)
            }
        };
        FrameScene {
            camera,
            width: self.size.0,
            height: self.size.1,
            planes: &self.planes,
            facet_finishes: &self.finishes,
            material: &self.material,
            max_bounces: self.max_bounces,
            environment,
        }
    }

    /// Runs `run` on the frame description of the job (built with the job's own camera).
    pub(super) fn with_scene<R>(&self, run: impl FnOnce(&FrameScene<'_>) -> R) -> R {
        let camera = self.camera.build();
        run(&self.scene(&camera))
    }

    /// The threads the job may use.
    pub(super) const fn threads(&self) -> usize {
        self.threads
    }

    /// Copies `frame` for a job after checking that its stone is beam traced.
    ///
    /// `hdr` is the loaded panorama when `frame` is lit by one.
    ///
    /// # Errors
    ///
    /// The [`BeamOff`] reason the stone is not beam traced; [`BeamOff::OutOfScopeLighting`] for an
    /// HDR frame without its panorama.
    pub(super) fn capture(
        frame: &BackendFrame<'_>,
        hdr: Option<&Arc<EnvironmentMap>>,
        cap: u32,
        threads: usize,
    ) -> Result<Self, BeamOff> {
        check(frame)?;
        let light = match frame.environment {
            EnvironmentSource::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare,
                head_shadow_deg,
                ..
            } => JobLight::Studio(EnvironmentSource::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare,
                head_shadow_deg,
                tray_shadow: None,
                // `check` refused a frame with a tray mirror, so a job never carries one.
                tray_mirror: None,
            }),
            EnvironmentSource::HdrMap {
                exposure, rotation, ..
            } => JobLight::Hdr(
                Arc::clone(hdr.ok_or(BeamOff::OutOfScopeLighting)?),
                exposure,
                rotation,
            ),
        };
        Ok(Self {
            size: (frame.width, frame.height),
            camera: CameraParts::of(frame.camera),
            planes: frame.planes.to_vec(),
            finishes: frame.facet_finishes.to_vec(),
            material: frame.material.clone(),
            max_bounces: frame.max_bounces,
            light,
            cap,
            threads,
        })
    }
}

/// The two cancel tokens of a job: the frame's (raised by the GUI the instant it edits the scene)
/// and the job's own (raised by the render loop).
pub(super) struct Tokens<'a> {
    pub(super) frame: &'a AtomicBool,
    pub(super) own: &'a AtomicBool,
}

impl Tokens<'_> {
    fn raised(&self) -> bool {
        self.frame.load(Ordering::Acquire) || self.own.load(Ordering::Acquire)
    }
}

/// Copies a raised token into `combined` until `finished`, polled every [`WATCH_INTERVAL`]: the
/// renderer polls one token, and this makes it see either of two.
fn watch(tokens: &Tokens<'_>, combined: &AtomicBool, finished: &AtomicBool) {
    while !finished.load(Ordering::Acquire) {
        if tokens.raised() {
            combined.store(true, Ordering::Release);
        }
        thread::sleep(WATCH_INTERVAL);
    }
}

/// The sink of finished batches: their tiles and final pixels.
type OnBatch<'a> = &'a mut dyn FnMut(&[TileRect], &[Vec3]);

/// The beam hybrid render of `inputs`: the viewport's lighting, the residual bounded by the
/// viewport's cap, `on_batch` told of every finished batch.
///
/// # Errors
///
/// The [`BeamOff`] reason the entry declines the frame.
fn render_frame(
    inputs: &BeamJobInputs,
    cancel: &AtomicBool,
    on_batch: Option<OnBatch<'_>>,
    retain: Option<&mut dyn FnMut(RetainedBatch)>,
) -> Result<(AdaptiveImage, BeamStats), BeamOff> {
    let camera = inputs.camera.build();
    let scene = inputs.scene(&camera);
    let soa = build_plane_soa(&inputs.planes);
    let adaptive = viewport_settings(inputs.cap, standard_target_noise());
    let mut progress = |progress: BeamProgress| {
        tracing::trace!(
            phase = progress.phase.label(),
            done = progress.done,
            "viewport beams: progress"
        );
    };
    render_beam_hybrid_surface(
        &scene,
        (&[], Fluorescence::none(), SurfaceScene::IDEAL),
        &soa,
        &viewport_beam_settings(),
        &adaptive,
        &viewport_engines(),
        (0, CHECK_SCALE),
        cancel,
        BeamRun {
            tiles: None,
            progress: &mut progress,
            // Coerced to the run's lifetime: `&mut` is invariant, so the caller's lifetime would
            // otherwise pin the run and the local `progress` could not join it.
            on_batch: on_batch.map(|on_batch| on_batch as &mut dyn FnMut(&[TileRect], &[Vec3])),
            retain: retain.map(|retain| retain as &mut dyn FnMut(RetainedBatch)),
            preloaded: None,
        },
    )
}

/// Raises its flag when dropped, on a normal return and on unwinding alike.
struct SetOnDrop<'a>(&'a AtomicBool);

impl Drop for SetOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Runs `work` with a watcher that turns either of `tokens` into the one token `work` polls.
///
/// The watcher ends when `work` returns or panics, so a panic propagates instead of leaving the
/// thread scope waiting on the watcher.
pub(super) fn with_combined_token<R>(
    tokens: &Tokens<'_>,
    work: impl FnOnce(&AtomicBool) -> R,
) -> (R, bool) {
    let combined = AtomicBool::new(false);
    let finished = AtomicBool::new(false);
    let result = thread::scope(|scope| {
        scope.spawn(|| watch(tokens, &combined, &finished));
        let _signal = SetOnDrop(&finished);
        work(&combined)
    });
    (result, combined.load(Ordering::Acquire))
}

/// The body of a render job: the cache starts empty for the new pose and keeps the finished
/// batches' leaves (up to its cap) for a later re-light.
fn run_render(
    inputs: &BeamJobInputs,
    tokens: &Tokens<'_>,
    shared: (&Mutex<Overlay>, &Mutex<LeafCache>),
    generation: u64,
) -> Outcome {
    let (overlay, cache) = shared;
    *lock(cache) = LeafCache::new(VIEWPORT_LEAF_CACHE_BYTES);
    let (result, cancelled) = with_combined_token(tokens, |cancel| {
        let mut on_batch = |tiles: &[TileRect], means: &[Vec3]| {
            lock(overlay).write_batch(generation, tiles, means);
        };
        let mut retain = |batch: RetainedBatch| {
            lock(cache).retain(batch);
        };
        live_cpu_budget::with_limit(inputs.threads, || {
            // The beam pipeline's own worker count follows the same quota.
            with_thread_limit(inputs.threads, || {
                render_frame(inputs, cancel, Some(&mut on_batch), Some(&mut retain))
            })
        })
    });
    match result {
        _ if cancelled => Outcome::Cancelled,
        Ok((image, stats)) => {
            lock(overlay).write_frame(generation, &image.pixels);
            tracing::debug!(summary = %stats.summary(), "viewport beams: finished");
            Outcome::Done
        }
        Err(off) => Outcome::Declined(off),
    }
}

/// The body of a re-light job: the retained leaves under the lighting of `inputs`.
fn run_relight(
    inputs: &BeamJobInputs,
    tokens: &Tokens<'_>,
    shared: (&Mutex<Overlay>, &Mutex<LeafCache>),
    generation: u64,
) -> Outcome {
    let (overlay, cache) = shared;
    let (pixels, cancelled) = with_combined_token(tokens, |cancel| {
        let camera = inputs.camera.build();
        let scene = inputs.scene(&camera);
        live_cpu_budget::with_limit(inputs.threads, || {
            with_thread_limit(inputs.threads, || {
                lock(cache).relight(
                    &scene,
                    &viewport_beam_settings(),
                    &viewport_engines(),
                    cancel,
                )
            })
        })
    });
    match pixels {
        Some(pixels) if !cancelled => {
            lock(overlay).write_frame(generation, &pixels);
            Outcome::Done
        }
        _ => Outcome::Cancelled,
    }
}

/// What [`StillBeams::launch`] reads from the render loop besides the frame.
pub(super) struct Launch<'a> {
    /// The loaded HDR panorama, when there is one.
    pub(super) hdr: Option<&'a Arc<EnvironmentMap>>,
    /// The viewport's sample cap.
    pub(super) cap: u32,
    /// The frame's cancel token.
    pub(super) frame_cancel: &'a Arc<AtomicBool>,
}

/// An empty leaf cache behind its own handle: one per pose, never shared between poses.
fn fresh_cache() -> Arc<Mutex<LeafCache>> {
    Arc::new(Mutex::new(LeafCache::new(VIEWPORT_LEAF_CACHE_BYTES)))
}

/// The beam job of the viewport's render loop.
pub(super) struct StillBeams {
    stage: Stage,
    /// The pose and the size the stage describes.
    key: Option<(u64, (u32, u32))>,
    /// The scene generation the stage describes.
    scene: u64,
    /// A lighting-only change is waiting for its re-light to be launched.
    relight_pending: bool,
    /// The leaves of a finished render are retained and can be re-lit.
    cache_valid: bool,
    overlay: Arc<Mutex<Overlay>>,
    cache: Arc<Mutex<LeafCache>>,
    /// The overlay version the render loop has seen.
    seen_version: u64,
    /// The picture [`Self::compose_into`] builds.
    composite: Vec<Vec3>,
}

impl Default for StillBeams {
    fn default() -> Self {
        Self {
            stage: Stage::Idle,
            key: None,
            scene: 0,
            relight_pending: false,
            cache_valid: false,
            overlay: Arc::new(Mutex::new(Overlay::default())),
            cache: fresh_cache(),
            seen_version: 0,
            composite: Vec::new(),
        }
    }
}

impl StillBeams {
    /// Brings the job in step with this frame and says what the loop should start.
    ///
    /// Call once per frame. A frame the beams do not apply to, a new pose or size drops the job, the
    /// overlay and the retained leaves at once; a lighting-only change cancels the job and routes to
    /// a re-light or a restart ([`lighting_route`]). The step repeats every frame until
    /// [`Self::launch`] has taken it.
    pub(super) fn sync(&mut self, want: BeamWanted) -> BeamStep {
        self.collect();
        if !want.applies {
            self.drop_all();
            return BeamStep::Idle;
        }
        let key = (want.pose, want.size);
        if self.key != Some(key) {
            self.drop_all();
            self.key = Some(key);
            self.scene = want.scene;
            lock(&self.overlay).reset(want.size);
        } else if self.scene != want.scene {
            self.scene = want.scene;
            self.on_lighting_change(want.instant_lighting);
        }
        match self.stage {
            Stage::Idle if self.relight_pending => BeamStep::Relight,
            Stage::Idle if want.samples >= VIEWPORT_BEAMS_AFTER_SPP => BeamStep::Start,
            _ => BeamStep::Idle,
        }
    }

    /// Whether beams draw this pose (a job is running or its picture is shown): the viewport's path
    /// cache is then not built.
    pub(super) const fn owns_stone(&self) -> bool {
        matches!(self.stage, Stage::Running(_) | Stage::Shown)
    }

    /// Whether the overlay changed since the last call: the picture needs showing again.
    pub(super) fn take_news(&mut self) -> bool {
        let version = lock(&self.overlay).version;
        let news = version != self.seen_version;
        self.seen_version = version;
        news
    }

    /// Builds the picture to show from the overlay and `accum / samples` and says whether there is
    /// one; read it with [`Self::composite`]. `false` leaves the ordinary picture to show.
    pub(super) fn compose_into(&mut self, accum: &[Vec3], samples: u32) -> bool {
        lock(&self.overlay).compose(accum, samples, &mut self.composite)
    }

    /// The picture [`Self::compose_into`] built, a running sum of one sample.
    pub(super) fn composite(&self) -> &[Vec3] {
        &self.composite
    }

    /// Starts the job of `step` for `frame`, when the stone is beam traced; a stone that is not
    /// (or a machine without a spare core) is refused until the pose or the lighting changes, with
    /// the ordinary image carrying on.
    pub(super) fn launch(&mut self, step: BeamStep, frame: &BackendFrame<'_>, with: &Launch<'_>) {
        let kind = match step {
            BeamStep::Idle => return,
            BeamStep::Start => JobKind::Render,
            BeamStep::Relight => JobKind::Relight,
        };
        self.relight_pending = false;
        if !matches!(self.stage, Stage::Idle) {
            return;
        }
        let Some(threads) = spare_threads() else {
            tracing::debug!("viewport beams: no spare core");
            self.stage = Stage::Refused;
            return;
        };
        let inputs = match BeamJobInputs::capture(frame, with.hdr, with.cap, threads) {
            Ok(inputs) => inputs,
            Err(off) => {
                self.refuse(off);
                return;
            }
        };
        let generation = lock(&self.overlay).generation;
        if kind == JobKind::Render {
            // A render starts on a cache no other job can reach (a cancelled job may still hold
            // the old handle and write to it).
            self.invalidate_cache();
        }
        let overlay = Arc::clone(&self.overlay);
        let cache = Arc::clone(&self.cache);
        let frame_cancel = Arc::clone(with.frame_cancel);
        match kind {
            JobKind::Render => self.spawn_job(kind, move |own| {
                let tokens = Tokens {
                    frame: &frame_cancel,
                    own,
                };
                run_render(&inputs, &tokens, (&overlay, &cache), generation)
            }),
            JobKind::Relight => self.spawn_job(kind, move |own| {
                let tokens = Tokens {
                    frame: &frame_cancel,
                    own,
                };
                run_relight(&inputs, &tokens, (&overlay, &cache), generation)
            }),
        }
    }

    /// Runs `run` on a background thread as the job of this pose.
    fn spawn_job<F>(&mut self, kind: JobKind, run: F)
    where
        F: FnOnce(&AtomicBool) -> Outcome + Send + 'static,
    {
        let cancel = Arc::new(AtomicBool::new(false));
        let own = Arc::clone(&cancel);
        let spawned = thread::Builder::new()
            .name("viewport-beams".to_owned())
            .spawn(move || run(own.as_ref()));
        match spawned {
            Ok(handle) => {
                self.relight_pending = false;
                self.stage = Stage::Running(Job {
                    kind,
                    cancel,
                    handle,
                });
            }
            Err(error) => {
                tracing::warn!(%error, "viewport beams: the job thread did not start");
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
                self.cache_valid = match job.kind {
                    JobKind::Render => !lock(&self.cache).is_empty(),
                    JobKind::Relight => true,
                };
            }
            Ok(Outcome::Declined(off)) => self.refuse(off),
            Ok(Outcome::Cancelled) => {
                // A token the GUI raised for an edit that left the pose alone: start again.
                self.clear_overlay();
                match job.kind {
                    JobKind::Render => self.invalidate_cache(),
                    // The leaves are still good: retry the re-light, not a render.
                    JobKind::Relight => self.relight_pending = true,
                }
            }
            Err(_) => {
                tracing::warn!("viewport beams: the job thread panicked");
                self.clear_overlay();
                self.stage = Stage::Refused;
            }
        }
    }

    /// A lighting-only change: stops the job, drops the stale picture and routes the change.
    fn on_lighting_change(&mut self, instant_lighting: bool) {
        let route = lighting_route(instant_lighting, self.cache_valid);
        self.cancel_job();
        self.clear_overlay();
        self.stage = Stage::Idle;
        match route {
            LightingRoute::Relight => self.relight_pending = true,
            LightingRoute::Restart => {
                self.relight_pending = false;
                self.invalidate_cache();
            }
        }
    }

    /// Raises the token of a running job and lets go of it.
    fn cancel_job(&mut self) {
        if let Stage::Running(job) = &self.stage {
            job.cancel.store(true, Ordering::Release);
            self.stage = Stage::Idle;
        }
    }

    /// Refuses the pose for the entry's reason.
    fn refuse(&mut self, off: BeamOff) {
        if off.is_silent() {
            tracing::debug!("viewport beams: stone out of scope");
        } else {
            tracing::debug!(reason = off.message(), "viewport beams: refused");
        }
        self.clear_overlay();
        self.stage = Stage::Refused;
    }

    /// Empties the overlay (ending the writes of any job still winding down).
    fn clear_overlay(&self) {
        let mut overlay = lock(&self.overlay);
        let size = overlay.size;
        overlay.reset(size);
    }

    /// Forgets the retained leaves by handing the next pose a fresh cache handle.
    ///
    /// A job that is still winding down keeps the handle it was launched with, so its reset, retain
    /// or re-light can neither touch the new pose's leaves nor block this thread on the mutex.
    fn invalidate_cache(&mut self) {
        self.cache_valid = false;
        self.cache = fresh_cache();
    }

    /// Stops everything and forgets the pose: the view is not one beams apply to.
    fn drop_all(&mut self) {
        let idle = self.key.is_none()
            && matches!(self.stage, Stage::Idle)
            && !self.relight_pending
            && !self.cache_valid;
        if idle {
            return;
        }
        self.cancel_job();
        self.stage = Stage::Idle;
        self.key = None;
        self.relight_pending = false;
        self.clear_overlay();
        self.invalidate_cache();
    }

    /// Whether a job is running.
    #[cfg(test)]
    #[cfg(all(feature = "beam", beam_tests))]
    const fn is_running(&self) -> bool {
        matches!(self.stage, Stage::Running(_))
    }

    /// Whether the pose was refused.
    #[cfg(test)]
    #[cfg(all(feature = "beam", beam_tests))]
    const fn is_refused(&self) -> bool {
        matches!(self.stage, Stage::Refused)
    }

    /// Whether nothing is held: no job, no pose, no overlay.
    #[cfg(test)]
    #[cfg(all(feature = "beam", beam_tests))]
    fn is_inactive(&self) -> bool {
        matches!(self.stage, Stage::Idle)
            && self.key.is_none()
            && !self.relight_pending
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

    /// Pretends the leaves of the finished render are retained (the retaining stubs keep nothing).
    #[cfg(test)]
    #[cfg(all(feature = "beam", beam_tests))]
    const fn mark_cache_ready(&mut self) {
        self.cache_valid = true;
    }
}

#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests;
