//! The progressive CPU render.
//!
//! It is a pool of worker threads that trace the solved design, render epochs that replace
//! each other when the scene changes, the tone-map and denoise schedule, the battery and
//! thermal governor, and the local PNG export.
//!
//! # How the UI uses it
//!
//! - [`start`] begins an epoch for the design, material, lighting, HDR map and camera as
//!   they are now and cancels the one before. It returns at once; the scene is built and
//!   traced on the pool's threads.
//! - [`needs_restart`] says whether those inputs changed since the running epoch began (the
//!   UI calls [`start`] again, coalescing camera drags).
//! - [`RenderState::take_frame`] hands out the newest picture once; the UI polls it from a
//!   timer. [`RenderState::status`] says what the render is doing.
//! - [`RenderState::govern`] (every ten seconds) and [`RenderState::lifecycle`] keep the
//!   device cool and the background quiet.
//! - [`export_job`] snapshots the scene for a picture export; [`ExportJob::run`] traces it
//!   on a thread of the caller's.
//!
//! # Modules
//!
//! - `pool`: the shared state, the epoch and the worker loop.
//! - `scene`: the scene description built from [`AppState`], and what makes two of them
//!   equal.
//! - `governor`: the pure thermal and battery rules.
//! - `export`: [`ExportJob`].
//! - `remote_session`: the interactive remote session of an epoch, with reconnection after
//!   a lost transport or a stay in the background.

mod backend;
mod design_scene;
mod export;
mod governor;
mod pool;
mod remote_session;
mod scene;

#[cfg(test)]
mod failure_tests;
#[cfg(test)]
mod geometry_tests;
#[cfg(test)]
mod tests;

use std::{
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    thread::JoinHandle,
    time::Duration,
};

use pleochroma::{color::ColorSpace, diag};
use pleochroma_web_core::{
    display::{MAX_EXPORT_EDGE, export_size},
    render::{EXPORT_MAX_SPP, EXPORT_MIN_SPP},
    scene::GeometrySceneSpec,
};

pub use design_scene::{DesignScene, wire_effects};
pub use export::{CANCELLED, ExportJob};
pub use governor::{
    BATTERY_SAVER_WORKERS, GovernorDecision, GovernorInputs, MAX_WORKERS, base_workers, govern,
};

use crate::{
    app_state::{AppState, Lifecycle},
    layout::WindowClass,
    settings::{RenderBackend, RenderQuality},
    tasks::{BackgroundQueue, join_briefly},
};
use pool::Shared;
use scene::{Prepared, SceneKey};

/// What the status line says while the GPU probe holds renders back and no picture shows.
pub const PROBE_WAIT_SENTENCE: &str =
    "The GPU probe is running. The render starts when it finishes.";

/// The sample target of Draft quality (and of battery saver).
const DRAFT_SPP: u32 = 32;

/// The smallest and largest live sample targets, whatever the settings say.
const MIN_LIVE_SPP: u32 = 8;
const MAX_LIVE_SPP: u32 = 2048;

/// The shortest export edge in pixels.
const MIN_EXPORT_EDGE: u32 = 16;

/// The stack of a render thread (the tracer is iterative; this is generous).
const WORKER_STACK_BYTES: usize = 4 * 1024 * 1024;

/// What a closed renderer answers to [`start`].
const CLOSED: &str = "The renderer has been shut down.";

/// The raster the render traces, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderView {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// One picture of the render.
#[derive(Clone, PartialEq, Eq)]
pub struct RenderFrame {
    /// The epoch ([`start`]'s return value) this picture belongs to.
    pub epoch: u64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Tone-mapped sRGB, 8-bit RGBA, row-major, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    /// Samples per pixel merged into the picture.
    pub samples: u32,
    /// The epoch's sample target.
    pub target: u32,
    /// Whether the picture went through the denoiser.
    pub denoised: bool,
}

impl fmt::Debug for RenderFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RenderFrame")
            .field("epoch", &self.epoch)
            .field("size", &(self.width, self.height))
            .field("rgba_len", &self.rgba.len())
            .field("samples", &self.samples)
            .field("target", &self.target)
            .field("denoised", &self.denoised)
            .finish()
    }
}

/// Why tracing is paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseReason {
    /// The user paused it.
    User,
    /// The app is in the background.
    Background,
    /// The device is too hot.
    Hot,
}

/// What the render is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderStatus {
    /// Nothing runs.
    Idle,
    /// Tracing: `samples` of `target` samples per pixel are merged.
    Tracing {
        /// Merged samples per pixel.
        samples: u32,
        /// The target.
        target: u32,
    },
    /// The target is reached.
    Done {
        /// Samples per pixel in the final picture.
        samples: u32,
    },
    /// Tracing is paused; the last picture stays.
    Paused(PauseReason),
    /// There is nothing to show; the text says why (shown to the user as it is).
    Failed(String),
    /// A GPU step is in flight and nothing is merged yet: the graphics card starts, the
    /// shaders build, the stone uploads or the first pass runs.
    Preparing {
        /// The step in flight.
        step: pleochroma::diag::PrepareStep,
        /// Kernels built so far in this step.
        done: u32,
        /// Kernels this step builds in all, or 0 when it has no count.
        total: u32,
        /// Whole seconds the step has taken so far.
        elapsed_s: u32,
    },
}

/// The color space of an exported picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportColorSpace {
    /// sRGB (untagged PNG).
    Srgb,
    /// Display P3 (PNG with an embedded profile).
    DisplayP3,
}

impl ExportColorSpace {
    const fn space(self) -> ColorSpace {
        match self {
            Self::Srgb => ColorSpace::Srgb,
            Self::DisplayP3 => ColorSpace::DisplayP3,
        }
    }
}

/// What the export sheet asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportRequest {
    /// The long edge of the picture in pixels (capped by `settings.render.export_cap_px`).
    pub long_edge_px: u32,
    /// Samples per pixel (capped by `settings.render.export_cap_spp`).
    pub spp: u32,
    /// The color space of the file.
    pub color_space: ExportColorSpace,
}

/// The render engine's state on the UI thread. See the module docs.
pub struct RenderState {
    shared: Arc<Shared>,
    /// The render threads, spawned by the first [`start`].
    threads: Vec<JoinHandle<()>>,
    /// Builds scenes and re-shows settled pictures off the UI thread.
    aux: Option<BackgroundQueue>,
    cores: usize,
    /// The raster of the last [`start`].
    view: Option<RenderView>,
    /// What the last [`start`] asked for: the scene's key, or why there was none.
    attempt: Option<Result<SceneKey, String>>,
    next_epoch: u64,
    thermal: Option<f32>,
    battery_saver: bool,
    /// Per engine ([`diag::Engine`] as an index): "Use the CPU" was chosen. Not persisted.
    session_cpu_only: [bool; 2],
    /// The next driver step starts the render again though the scene did not change.
    restart_wanted: bool,
    /// The last backend choice written to the tracer log, so only a change is written again.
    logged_backend: Option<(diag::Engine, RenderBackend, Option<CpuReason>)>,
    /// Test view: stands in for the process-wide probe flag.
    #[cfg(test)]
    probe_running: Option<bool>,
}

impl fmt::Debug for RenderState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RenderState")
            .field("status", &self.shared.status())
            .field("threads", &self.threads.len())
            .field("view", &self.view)
            .finish_non_exhaustive()
    }
}

impl Default for RenderState {
    fn default() -> Self {
        let cores = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
        Self {
            shared: Arc::new(Shared::new(base_workers(cores))),
            threads: Vec::new(),
            aux: None,
            cores,
            view: None,
            attempt: None,
            next_epoch: 0,
            thermal: None,
            battery_saver: false,
            session_cpu_only: [false; 2],
            restart_wanted: false,
            logged_backend: None,
            #[cfg(test)]
            probe_running: None,
        }
    }
}

impl Drop for RenderState {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// The pool's state is shared behind an `Arc`, so several methods need no `&mut self`; they
// take it anyway so the owner of the `AppState` serialises every call.
impl RenderState {
    /// Shared budget for long editor operations and exports as well as tracing.
    #[must_use]
    pub fn compute_policy(&self) -> Arc<crate::compute_policy::ComputePolicy> {
        Arc::clone(&self.shared.policy)
    }
    /// Cancels the running epoch and keeps the pool. The last pictures are dropped; the
    /// next [`start`] begins afresh.
    pub fn stop(&mut self) {
        self.shared.stop();
        self.attempt = None;
        tracing::info!(target: diag::TRACER_TARGET, "render stopped");
    }

    /// Cancels a socket session without scheduling an automatic re-upload. Explicit
    /// Retry or a changed scene starts a fresh remote request at sample zero.
    pub fn cancel_remote(&mut self) {
        self.shared.stop();
    }

    /// Pauses or resumes tracing at the user's request. A paused render keeps its picture.
    pub fn set_user_paused(&mut self, paused: bool) {
        let before = self.shared.control().user_paused;
        self.shared
            .update_control(|control| control.user_paused = paused);
        if before != paused {
            tracing::info!(
                target: diag::TRACER_TARGET,
                "render {}",
                if paused { "paused by the user" } else { "resumed by the user" }
            );
        }
    }

    /// Whether the user paused tracing.
    #[must_use]
    pub fn user_paused(&self) -> bool {
        self.shared.control().user_paused
    }

    /// The newest picture since the last call, once. Pictures of an older epoch are never
    /// returned.
    pub fn take_frame(&mut self) -> Option<RenderFrame> {
        self.shared.take_frame()
    }

    /// What the render is doing.
    #[must_use]
    pub fn status(&self) -> RenderStatus {
        self.shared.status()
    }

    /// Whether the last epoch started fine and then died while running (a render thread
    /// panicked). Not the "nothing to render" failures, which a restart repeats.
    #[must_use]
    pub fn run_failed(&self) -> bool {
        matches!(self.shared.status(), RenderStatus::Failed(_))
            && matches!(self.attempt, Some(Ok(_)))
    }

    /// The raster of the last [`start`].
    #[must_use]
    pub const fn view(&self) -> Option<RenderView> {
        self.view
    }

    /// Turns denoising of the settled picture on or off. A finished render shows the
    /// matching picture at once (off the UI thread); a running one decides when it is done.
    pub fn set_denoise(&mut self, on: bool) {
        if let Some(settled) = self.shared.set_denoise(on) {
            let shared = Arc::clone(&self.shared);
            self.aux().submit(move || shared.show_settled(&settled, on));
        }
    }

    /// Applies the thermal and battery rules; call it every ten seconds with the platform's
    /// thermal headroom. Battery saver changes the sample target from the next [`start`].
    pub fn govern(&mut self, thermal_headroom: Option<f32>, battery_saver: bool) {
        // Stay paused until a valid reading drops below the recovery threshold.
        // Missing readings do not prove that a previously hot device cooled down.
        self.thermal = if self.thermal.is_some_and(|value| value > 1.0)
            && !thermal_headroom.is_some_and(|value| value.is_finite() && value < 0.90)
        {
            self.thermal
        } else {
            thermal_headroom
        };
        self.battery_saver = battery_saver;
        let hot_before = self.shared.control().hot;
        self.apply_governor();
        let hot_after = self.shared.control().hot;
        if hot_before != hot_after {
            tracing::info!(
                target: diag::TRACER_TARGET,
                "render {}",
                if hot_after { "thermal pause" } else { "thermal resume" }
            );
        }
    }

    /// Reacts to a lifecycle event: leaving the foreground pauses tracing (the running
    /// chunks stop within milliseconds and are handed back); coming back resumes the epoch.
    pub fn lifecycle(&mut self, event: Lifecycle) {
        let background = !matches!(event, Lifecycle::Resumed);
        let before = self.shared.control().background;
        self.shared
            .update_control(|control| control.background = background);
        self.apply_governor();
        if before != background {
            tracing::info!(
                target: diag::TRACER_TARGET,
                "render {}",
                if background { "paused: app in background" } else { "resumed: app in foreground" }
            );
        }
    }

    /// Stops the render, joins the worker threads (waiting at most about two seconds for
    /// each) and ends the pool. Called by `AppState::shutdown`; the state refuses to start
    /// again afterwards.
    pub fn shutdown(&mut self) {
        self.shared.close();
        for handle in self.threads.drain(..) {
            join_briefly(handle, "render-worker");
        }
        self.aux = None;
    }

    /// "Use the CPU": gives up a hung preparation, keeps `engine` off the GPU for the rest of
    /// this session and renders again.
    ///
    /// The graphics card slot of a hung step stays locked for the session; the CPU path never
    /// touches it. The choice is not persisted. The next driver step starts the render over.
    pub fn use_cpu_for_session(&mut self, engine: diag::Engine) {
        self.session_cpu_only[engine as usize] = true;
        tracing::info!(
            target: diag::TRACER_TARGET,
            engine = engine_name(engine),
            "the CPU is used for this session"
        );
        self.abandon_preparation(engine);
        // The step the abandoned thread was in no longer shows as this render's progress.
        diag::clear_prepare(engine);
        self.stop();
        self.restart_wanted = true;
    }

    /// How long the latest GPU preparation took, and where it stopped if it was abandoned.
    ///
    /// Read for Render Details; [`Self::status`] is what notes a preparation's start and end,
    /// so it is as exact as the UI's polling of the status.
    #[must_use]
    pub fn preparation_time(&self) -> Option<crate::viewport::render_view::PreparationTime> {
        self.shared.last_preparation()
    }

    /// Whether "Use the CPU" was chosen for `engine` in this session.
    #[must_use]
    pub const fn session_cpu_only(&self, engine: diag::Engine) -> bool {
        self.session_cpu_only[engine as usize]
    }

    /// True while the probe holds renders back; a start asked for meanwhile is remembered.
    #[must_use]
    pub fn held_by_probe(&self) -> bool {
        #[cfg(test)]
        {
            if let Some(running) = self.probe_running {
                return running;
            }
        }
        crate::diagnostics::probe::is_running()
    }

    /// Runs the remembered start. The UI thread calls it when it sees the probe ended; the
    /// next driver step then starts the render, whether or not the scene changed.
    pub fn resume_after_probe(&mut self) {
        if !self.held_by_probe() && self.shared.take_remembered_start() {
            self.restart_wanted = true;
        }
    }

    /// Test view: pretends the probe runs (`Some(true)`), does not, or reads the real flag.
    #[cfg(test)]
    pub(crate) const fn set_probe_running(&mut self, running: Option<bool>) {
        self.probe_running = running;
    }

    /// Remembers that a start was asked for while the probe runs (the driver calls it).
    pub(crate) fn note_start_asked(&self) {
        self.shared.remember_start();
    }

    /// Whether the driver must start the render though the scene did not change; clears it.
    pub(crate) const fn take_restart_wanted(&mut self) -> bool {
        let wanted = self.restart_wanted;
        self.restart_wanted = false;
        wanted
    }

    /// Gives up a preparation that does not end, without waiting for it. Returns whether
    /// there was one.
    ///
    /// - `render-aux` inside a scene preparation: its queue is replaced and the old one is
    ///   dropped on a thread of its own, so nothing here joins it.
    /// - A `render-N` thread inside a GPU trace (a kernel compile that cannot be interrupted):
    ///   a new thread takes the index over, the old one leaves when its call returns.
    ///
    /// Each abandoned thread's compute permit is returned with `ComputePolicy::forget_one`.
    fn abandon_preparation(&mut self, engine: diag::Engine) -> bool {
        let mut abandoned = false;
        if self.shared.abandon_aux_prepare() {
            let old = self.aux.replace(BackgroundQueue::new("render-aux"));
            self.shared.policy.forget_one();
            if let Some(old) = old {
                release_without_waiting(old);
            }
            abandoned = true;
        }
        for index in self.shared.gpu_workers() {
            let generation = self.shared.supersede_worker(index);
            match self.spawn_worker(index) {
                Ok(handle) => {
                    if let Some(slot) = self.threads.get_mut(index) {
                        // Dropping a join handle detaches the thread; it ends by itself.
                        drop(std::mem::replace(slot, handle));
                    }
                }
                Err(error) => {
                    tracing::error!(%error, generation, "a replacement render thread could not be started");
                }
            }
            self.shared.policy.forget_one();
            abandoned = true;
        }
        if abandoned {
            self.shared.note_preparation_abandoned();
            let progress = diag::prepare_progress();
            tracing::warn!(
                target: diag::TRACER_TARGET,
                engine = engine_name(engine),
                step = prepare_step_name(progress.step),
                "preparation abandoned"
            );
        }
        abandoned
    }

    /// The test view of the running epoch's merged sum.
    #[cfg(test)]
    pub(crate) fn debug_sum(&self) -> Option<(u32, Vec<glam::Vec3>)> {
        self.shared.debug_sum()
    }

    fn aux(&mut self) -> &BackgroundQueue {
        self.aux
            .get_or_insert_with(|| BackgroundQueue::new("render-aux"))
    }

    /// Recomputes how many threads may trace from the last thermal reading and the
    /// battery preference.
    fn apply_governor(&self) {
        let (cores, thermal_headroom, battery_saver) =
            (self.cores, self.thermal, self.battery_saver);
        self.shared.update_control(|control| {
            let decision = govern(&GovernorInputs {
                cores,
                thermal_headroom,
                foreground: !control.background,
                battery_saver,
            });
            control.workers_allowed = decision.workers.min(control.max_workers);
            self.shared.policy.set_capacity(decision.workers);
            control.hot = decision.paused == Some(PauseReason::Hot);
        });
    }

    /// Spawns the render threads on the first start.
    fn ensure_workers(&mut self) -> Result<(), String> {
        if !self.threads.is_empty() {
            return Ok(());
        }
        for index in 0..base_workers(self.cores) {
            let handle = self
                .spawn_worker(index)
                .map_err(|error| format!("The render threads could not be started: {error}"))?;
            self.threads.push(handle);
        }
        Ok(())
    }

    /// Starts render thread `index` for the generation that owns the index now.
    fn spawn_worker(&self, index: usize) -> std::io::Result<JoinHandle<()>> {
        let shared = Arc::clone(&self.shared);
        let generation = shared.worker_generation(index);
        std::thread::Builder::new()
            .name(format!("render-{index}"))
            .stack_size(WORKER_STACK_BYTES)
            .spawn(move || pool::worker_loop(&shared, index, generation))
    }

    /// Begins an epoch for `prepared`, or reports why there is nothing to render.
    fn begin(
        &mut self,
        view: RenderView,
        prepared: Result<Prepared, String>,
        denoise: bool,
        battery_saver: bool,
        remote: Option<crate::remote::rendering::RemoteConfig>,
    ) -> Result<u64, String> {
        if self.shared.is_closed() {
            return Err(CLOSED.to_owned());
        }
        self.view = Some(view);
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(message) => {
                self.attempt = Some(Err(message.clone()));
                self.shared.fail_now(&message);
                return Err(message);
            }
        };
        self.ensure_workers()?;
        self.battery_saver = battery_saver;
        self.apply_governor();
        self.next_epoch += 1;
        let id = self.next_epoch;
        let Prepared { key, hdr } = prepared;
        let engine = key.engine;
        // Facts for the tracer log: counts and the material class, never a design name.
        let facts = SceneFacts::of(&key.spec);
        let spec = DesignScene::new(
            GeometrySceneSpec {
                scene: key.spec.clone(),
                tools: key.tools.clone(),
            },
            key.effects.clone(),
        );
        let customs = key.customs.clone();
        let target = key.target;
        self.attempt = Some(Ok(key));
        self.shared.begin_epoch(id, target, denoise);
        let shared = Arc::clone(&self.shared);
        self.aux().submit(move || {
            if let Some(remote) = remote {
                remote_session::run(
                    &shared,
                    id,
                    &remote,
                    &spec,
                    &customs,
                    engine,
                    hdr.as_ref(),
                    target,
                    denoise,
                );
                return;
            }
            let Some(_permit) = shared.policy.acquire(&|| !shared.is_current(id)) else {
                return;
            };
            if !shared.is_current(id) {
                return;
            }
            // A panic while the scene is built or the GPU is acquired would be swallowed by
            // the queue and leave the epoch "tracing" with no scene for good; fail it
            // instead, so the next start begins a fresh epoch.
            // The token lets "Use the CPU" give up this preparation if the GPU never answers.
            let token = shared.begin_aux_prepare();
            let prepared = catch_unwind(AssertUnwindSafe(|| {
                let started = std::time::Instant::now();
                let built = spec
                    .build_with_customs(hdr, &customs)
                    .and_then(|mut scene| {
                        scene
                            .set_engine(engine)
                            .map_err(|reason| reason.message().to_owned())?;
                        Ok(scene)
                    });
                if let Ok(scene) = &built {
                    facts.log_built(scene.material(), started.elapsed());
                }
                shared.install_scene(id, built, target);
            }));
            shared.end_aux_prepare(token);
            if prepared.is_err() {
                tracing::error!("the render scene could not be prepared: a panic");
                shared.fail(id, pool::RUN_FAILED);
            }
        });
        Ok(id)
    }
}

/// The engine an engine choice traces with, as the engine mark and "Use the CPU" name it.
fn engine_of(spec: pleochroma::renderer::engine::EngineSpec) -> diag::Engine {
    if spec.is_ray() {
        diag::Engine::RayTraced
    } else {
        diag::Engine::PathTraced
    }
}

/// The engine's name in the tracer log.
const fn engine_name(engine: diag::Engine) -> &'static str {
    match engine {
        diag::Engine::PathTraced => "Path-Traced",
        diag::Engine::RayTraced => "Ray-Traced",
    }
}

/// The step's name in the tracer log.
const fn prepare_step_name(step: diag::PrepareStep) -> &'static str {
    match step {
        diag::PrepareStep::Idle => "idle",
        diag::PrepareStep::StartingGpu => "starting the graphics card",
        diag::PrepareStep::BuildingShaders => "building shaders",
        diag::PrepareStep::Uploading => "uploading the stone",
        diag::PrepareStep::FirstTrace => "first trace",
    }
}

/// Why Automatic or Device GPU is kept off the graphics card for an engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CpuReason {
    /// "Use the CPU" was chosen in this session.
    Session,
    /// The app closed while preparing this engine's graphics card (the engine mark).
    EngineMark,
}

/// Why `engine` renders on the CPU though the settings ask for `requested`, if it does.
///
/// "Use the CPU" holds for every local backend; the engine mark holds only for Automatic (a
/// manual Device GPU choice clears it). The CPU and Remote choices are never changed.
fn cpu_reason(
    state: &AppState,
    engine: diag::Engine,
    requested: RenderBackend,
) -> Option<CpuReason> {
    if matches!(requested, RenderBackend::Remote | RenderBackend::DeviceCpu) {
        return None;
    }
    if state.render.session_cpu_only(engine) {
        return Some(CpuReason::Session);
    }
    (requested == RenderBackend::Automatic && !state.diagnostics.state().gpu_allowed(engine))
        .then_some(CpuReason::EngineMark)
}

/// Writes the tracer line for a backend choice (no address, no name).
fn log_backend_chosen(engine: diag::Engine, effective: RenderBackend, reason: Option<CpuReason>) {
    let (backend, why) = match (effective, reason) {
        (_, Some(CpuReason::Session)) => ("Device CPU", "Use the CPU was chosen for this session"),
        (_, Some(CpuReason::EngineMark)) => (
            "Device CPU",
            "the app closed while preparing the graphics card earlier",
        ),
        (RenderBackend::Automatic, None) => ("Automatic", "the settings say Automatic"),
        (RenderBackend::DeviceCpu, None) => ("Device CPU", "chosen in the settings"),
        (RenderBackend::DeviceGpu, None) => ("Device GPU", "chosen in the settings"),
        (RenderBackend::Remote, None) => ("Remote", "chosen in the settings"),
    };
    tracing::info!(
        target: diag::TRACER_TARGET,
        engine = engine_name(engine),
        backend,
        reason = why,
        "backend chosen"
    );
}

/// The facts of a scene the tracer log may state: counts and a class, never a design name.
struct SceneFacts {
    facets: usize,
    /// The built-in material's name, or "custom".
    material: String,
    bounces: u32,
    width: u32,
    height: u32,
}

impl SceneFacts {
    fn of(spec: &pleochroma_web_core::scene::SceneSpec) -> Self {
        let material = &spec.material;
        let built_in = material.linked_design.is_none()
            && !material
                .custom_materials
                .iter()
                .any(|custom| custom.name == material.name);
        Self {
            facets: spec.planes.len(),
            material: if built_in {
                material.name.clone()
            } else {
                "custom".to_owned()
            },
            bounces: spec.max_bounces,
            width: spec.width,
            height: spec.height,
        }
    }

    /// The line for a scene that was built in `took`.
    fn log_built(&self, material: &pleochroma::optics::materials::GemMaterial, took: Duration) {
        // The renderer's `beam::birefringence` rule, which is behind the `beam` feature here.
        let anisotropic = material.crystal_system
            != pleochroma::optics::materials::CrystalSystem::Cubic
            && material.birefringence_delta.abs() > 1e-4;
        let class = if material.biaxial_delta_beta_alpha.is_some() {
            "biaxial"
        } else if anisotropic {
            "uniaxial"
        } else {
            "isotropic"
        };
        tracing::info!(
            target: diag::TRACER_TARGET,
            facets = self.facets,
            material = %self.material,
            class,
            bounces = self.bounces,
            width = self.width,
            height = self.height,
            ms = u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
            "scene built"
        );
    }
}

/// Drops a queue on a thread of its own: dropping joins it for up to two seconds, and an
/// abandoned queue's thread may never come back.
fn release_without_waiting(queue: BackgroundQueue) {
    let _ = std::thread::Builder::new()
        .name("render-abandon".to_owned())
        .spawn(move || drop(queue));
}

/// The live render's sample target for the quality setting and the window class.
///
/// Draft is 32; Normal is the compact or large count of the settings; Fine is twice that;
/// battery saver caps the target at Draft (a lower choice stays lower).
#[must_use]
pub fn live_target_spp(state: &AppState) -> u32 {
    let render = &state.settings.render;
    let engine = render.engine_spec();
    if engine.is_ray() {
        return engine.target_samples(0);
    }
    let normal = if state.window_class == WindowClass::Compact {
        render.live_spp_compact
    } else {
        render.live_spp_large
    };
    let spp = match render.quality {
        RenderQuality::Draft => DRAFT_SPP,
        RenderQuality::Normal => normal,
        RenderQuality::Fine => normal.saturating_mul(2),
    };
    // Battery saver caps the target at the draft count; a lower choice (16) stays lower.
    let spp = if state.settings.battery_saver {
        spp.min(DRAFT_SPP)
    } else {
        spp
    };
    spp.clamp(MIN_LIVE_SPP, MAX_LIVE_SPP)
}

/// Starts a render epoch at `view`'s size from the current design, solve, material,
/// lighting, HDR map and camera, cancelling the epoch before.
///
/// Returns the new epoch's id (the `epoch` of its frames) at once; the scene is built and
/// traced on the pool's threads.
///
/// # Errors
///
/// A sentence for the user when there is nothing to render ("Solve the design to render
/// it", a failed solve, no facets, an unresolvable material) or the renderer was shut
/// down. The running epoch is cancelled and [`RenderState::status`] reports the same
/// sentence as [`RenderStatus::Failed`].
pub fn start(state: &mut AppState, view: RenderView) -> Result<u64, String> {
    // While the GPU probe runs nothing starts; the start is remembered and runs afterwards.
    if state.render.held_by_probe() {
        state.render.note_start_asked();
        return Err(PROBE_WAIT_SENTENCE.to_owned());
    }
    // The preview's `--hold-preparing` shows a frozen "Preparing" status and launches nothing.
    if diag::prepare_override().is_some() {
        state.render.view = Some(view);
        state.render.attempt = None;
        state.render.next_epoch += 1;
        return Ok(state.render.next_epoch);
    }
    let requested = state.settings.render.backend;
    let engine = engine_of(state.settings.render.engine_spec());
    let reason = cpu_reason(state, engine, requested);
    let effective = if reason.is_some() {
        RenderBackend::DeviceCpu
    } else {
        requested
    };
    state.render.select_backend(effective);
    let chosen = (engine, effective, reason);
    if state.render.logged_backend != Some(chosen) {
        log_backend_chosen(engine, effective, reason);
        state.render.logged_backend = Some(chosen);
    }
    let prepared = scene::prepare(state, view);
    let denoise = state.settings.render.denoise && state.settings.render.engine_spec().denoises();
    let battery_saver =
        state.settings.battery_saver || state.platform().system_power_save().unwrap_or(false);
    let remote =
        (state.settings.render.backend == crate::settings::RenderBackend::Remote).then(|| {
            crate::remote::rendering::RemoteConfig {
                platform: Arc::clone(state.platform()),
                settings: state.settings.remote.clone(),
            }
        });
    state
        .render
        .begin(view, prepared, denoise, battery_saver, remote)
}

/// Whether [`start`] would trace something other than what runs now.
///
/// That is so when the design (its solve), the material, the lighting, the HDR map, the
/// camera, the sample target or the window class changed since the last start, or nothing
/// was started (or it was stopped).
///
/// An unchanged answer ("nothing to render" again) is not a change, so the UI does not
/// loop on a design that cannot be rendered.
#[must_use]
pub fn needs_restart(state: &AppState) -> bool {
    let Some(view) = state.render.view else {
        return true;
    };
    let now = scene::prepare(state, view).map(|prepared| prepared.key);
    state.render.attempt.as_ref() != Some(&now)
}

/// The view size in whole pixels.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "both sizes are checked to be at least 1.0 by the caller, so they are positive; `as` saturates for huge values"
)]
const fn pixel_size(width: f32, height: f32) -> (u32, u32) {
    (width as u32, height as u32)
}

/// Snapshots the scene for a picture export.
///
/// The size follows the live view's aspect (or the viewport's, or 4:3) with the requested
/// long edge. The long edge and the samples are capped by `settings.render.export_cap_px`
/// and `export_cap_spp`.
///
/// # Errors
///
/// A sentence for the user when there is nothing to render (see [`start`]).
pub fn export_job(state: &AppState, request: &ExportRequest) -> Result<ExportJob, String> {
    // No export is created while the GPU probe runs; the ask is remembered like a render's.
    if state.render.held_by_probe() {
        state.render.note_start_asked();
        return Err(PROBE_WAIT_SENTENCE.to_owned());
    }
    let settings = &state.settings.render;
    let mut engine = settings.engine_spec();
    if engine.is_ray() {
        engine.ray.aa_taps = 16;
    }
    // The same engine mark and "Use the CPU" choice as the live render.
    let backend = if cpu_reason(state, engine_of(engine), settings.backend).is_some() {
        RenderBackend::DeviceCpu
    } else {
        settings.backend
    };
    let (view_width, view_height) = state.render.view.map_or_else(
        || {
            let (width, height) = state.viewport.view_size;
            if width >= 1.0 && height >= 1.0 {
                pixel_size(width, height)
            } else {
                (4, 3)
            }
        },
        |view| (view.width, view.height),
    );
    let cap = settings
        .export_cap_px
        .clamp(MIN_EXPORT_EDGE, MAX_EXPORT_EDGE);
    let long_edge = request.long_edge_px.clamp(MIN_EXPORT_EDGE, cap);
    let (width, height) = export_size(view_width, view_height, long_edge);
    let path_spp = request
        .spp
        .clamp(EXPORT_MIN_SPP, EXPORT_MAX_SPP)
        .min(settings.export_cap_spp.max(1));
    let spp = engine.target_samples(path_spp);
    let prepared = scene::prepare(state, RenderView { width, height })?;
    Ok(ExportJob::new(
        DesignScene::new(
            GeometrySceneSpec {
                scene: prepared.key.spec,
                tools: prepared.key.tools,
            },
            prepared.key.effects,
        ),
        prepared.key.customs,
        engine,
        prepared.hdr,
        spp,
        request.color_space.space(),
        base_workers(state.render.cores),
        state.render.shared.rate_per_thread(),
        Arc::clone(&state.render.shared.policy),
        backend,
        state.render.shared.backend.probe(),
        (settings.backend == crate::settings::RenderBackend::Remote).then(|| {
            crate::remote::rendering::RemoteConfig {
                platform: Arc::clone(state.platform()),
                settings: state.settings.remote.clone(),
            }
        }),
    ))
}
