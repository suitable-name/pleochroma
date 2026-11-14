//! The GPU shader cache, built in the background as soon as the app starts.
//!
//! Path-Traced and Ray-Traced render on the graphics card, whose kernels must compile before the
//! first picture: one Ray-Traced kernel, and a set of Path-Traced kernels per material type.
//! Without this job the first render of each waits for that compile. This job compiles them up
//! front: Ray-Traced first, because its one kernel is quick, then Path-Traced.
//!
//! The live viewport renders on the backends this job warms ([`take_ray_backend`],
//! [`path_backend`]) instead of acquiring its own. On that shared device the job builds only what
//! the live view needs: the open stone's material type, in the variant its scene uses first (with
//! or without the tray), and only the kernel families of the viewport's own routes. The live view
//! finds them built, or waits at most one step behind the job.
//!
//! Every other kernel compiles on a second device of the job's own, so it never holds the live
//! render: every material type of the catalogue (`GpuBackend::warm_up_materials`), with and
//! without the tray, with every kernel family. These only fill the disk cache, from which the
//! viewport's device loads them when it needs them. A device without a disk cache (anything but
//! Vulkan with the folder `main` sets) skips them, because that work is lost at exit.
//!
//! Nothing runs while the processor alone renders (Local Compute set to the CPU).
//!
//! The card (`ShaderCacheModel`) and a status-strip activity show the progress. While the job
//! runs, the Live Render stage names its step too ([`waiting_stage`]).

use crate::{
    ActivityModel, MainWindow, ShaderCacheModel, ViewportModel,
    bridge::render_thread::{
        RenderContext, progress::PREPARING_TRACE, resolve_material_with_override,
    },
    settings::model::LocalComputeTarget,
};
use pleochroma::{
    geometry::{cuts::StandardGemCuts, tool::StoneGeometry},
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, DEFAULT_FOV_DEG, DEFAULT_POSE, LightingPreset, surface::SurfaceScene},
    },
    render_setup::tray_mirror_for,
    renderer::gpu_backend::{
        GpuAccumulate, GpuBackend, GpuSceneRef, GpuWarmFamilies, whitted::WhittedGpuBackend,
    },
};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc, Condvar, LazyLock, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Every kernel family this build has: the production kernels, the path cache's and, with
/// adaptive sampling built in, the live ones.
const ALL_FAMILIES: GpuWarmFamilies = GpuWarmFamilies {
    live: cfg!(feature = "adaptive-sampling"),
    path_cache: true,
};
/// Pixels per side of a stand-in scene; the kernels do not depend on the size.
const STAND_IN_SIZE: u32 = 16;
/// Head-on reflectance of the stand-in tray.
const STAND_IN_REFLECTION: f32 = 0.5;
/// How long the job runs before the card appears, so a start that only loads a finished cache
/// shows nothing.
const SHOW_AFTER: Duration = Duration::from_millis(600);
/// How long the card stays up, saying how the build ended, after the job.
const LINGER: Duration = Duration::from_secs(4);
/// How often the UI reads the job's progress.
const TICK: Duration = Duration::from_millis(200);
/// The Ray-Traced kernel's share of the status-strip activity.
const RAY_SHARE: f32 = 0.1;
/// What every Live Render stage this module writes begins with.
const STAGE_PREFIX: &str = "Building shader cache";
const NOTE_ONCE: &str = "Path-Traced and Ray-Traced render on the graphics card, which needs its \
                         shaders compiled first. This runs in the background after an install or \
                         an update; later starts load the finished cache.";
const NOTE_EVERY_START: &str = "Path-Traced and Ray-Traced render on the graphics card, which \
                                needs its shaders compiled first. This graphics driver keeps no \
                                cache between starts, so the shaders for the current material are \
                                built at every start.";
const NOTE_PARTIAL: &str = "Path-Traced and Ray-Traced render on the graphics card, which needs \
                            its shaders compiled first. The build stopped early; the remaining \
                            shaders compile when a renderer first needs them.";

/// What one renderer's row on the card says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BarState {
    /// Not started.
    #[default]
    Waiting,
    /// Compiling.
    Running,
    /// Every step done.
    Ready,
    /// Ended before the last step (the app closed, or the job's own device failed); the rest
    /// compiles when a renderer first needs it.
    Stopped,
    /// No graphics card renderer: the device could not be acquired, or it was lost.
    Unavailable,
}

/// One renderer's progress: `done` of `total` steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bar {
    /// Where the renderer is.
    pub state: BarState,
    /// Steps done.
    pub done: u32,
    /// Steps in all; `0` while unknown.
    pub total: u32,
}

impl Bar {
    const UNAVAILABLE: Self = Self {
        state: BarState::Unavailable,
        done: 0,
        total: 0,
    };

    const fn running(done: u32, total: u32) -> Self {
        Self {
            state: BarState::Running,
            done,
            total,
        }
    }

    const fn ready(total: u32) -> Self {
        Self {
            state: BarState::Ready,
            done: total,
            total,
        }
    }

    const fn stopped(done: u32, total: u32) -> Self {
        Self {
            state: BarState::Stopped,
            done,
            total,
        }
    }

    /// The filled share of the bar.
    #[must_use]
    pub const fn fraction(self) -> f32 {
        match self.state {
            BarState::Ready => 1.0,
            BarState::Running | BarState::Stopped if self.total > 0 => {
                self.done as f32 / self.total as f32
            }
            _ => 0.0,
        }
    }
}

/// The job's progress, as the card and the Live Render stage read it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// The job runs.
    pub running: bool,
    /// The Ray-Traced kernel.
    pub ray: Bar,
    /// The Path-Traced steps.
    pub path: Bar,
    /// Whether the Path-Traced device keeps compiled kernels between starts; `None` until known,
    /// and without a usable device.
    pub persistent: Option<bool>,
}

impl Progress {
    /// The Live Render stage while the job builds: the renderer and the step it is on.
    #[must_use]
    pub fn stage_text(&self) -> Option<String> {
        if !self.running {
            return None;
        }
        Some(match (self.ray.state, self.path) {
            (BarState::Waiting | BarState::Running, _) => format!("{STAGE_PREFIX}: Ray-Traced"),
            (
                _,
                Bar {
                    state: BarState::Running,
                    done,
                    total,
                },
            ) if total > 0 => format!(
                "{STAGE_PREFIX}: Path-Traced {} of {total}",
                (done + 1).min(total)
            ),
            _ => STAGE_PREFIX.to_owned(),
        })
    }

    /// The share of the build done, for the status-strip activity: the Ray-Traced kernel is its
    /// first [`RAY_SHARE`], the Path-Traced steps the rest. It never falls back when the
    /// Path-Traced total becomes known.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        RAY_SHARE.mul_add(
            self.ray.fraction(),
            (1.0 - RAY_SHARE) * self.path.fraction(),
        )
    }
}

/// The job's state and the backends it hands to the live viewport.
#[derive(Default)]
struct Handoff {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    progress: Progress,
    /// The Ray-Traced backend is being acquired; its kernel compiles inside the acquisition.
    ray_pending: bool,
    /// The warmed Ray-Traced backend, until the viewport takes it.
    ray: Option<WhittedGpuBackend>,
    /// The Path-Traced device is being acquired.
    path_pending: bool,
    /// The Path-Traced backend the job warms, shared with the viewport.
    path: Option<Arc<GpuBackend>>,
}

impl Handoff {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Changes the state and wakes every waiter.
    fn update(&self, change: impl FnOnce(&mut State)) {
        change(&mut self.lock());
        self.changed.notify_all();
    }

    /// Waits while `pending` holds, then reads or takes with `then`.
    fn after<T>(&self, pending: impl Fn(&State) -> bool, then: impl FnOnce(&mut State) -> T) -> T {
        let mut state = self.lock();
        while pending(&state) {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        let value = then(&mut state);
        drop(state);
        value
    }

    fn progress(&self) -> Progress {
        self.lock().progress
    }
}

/// Clears what the job left pending, also when it unwinds, so no viewport waits forever.
struct Finish<'a>(&'a Handoff);

impl Drop for Finish<'_> {
    fn drop(&mut self) {
        self.0.update(|state| {
            state.ray_pending = false;
            state.path_pending = false;
            state.progress.running = false;
        });
    }
}

static HANDOFF: LazyLock<Handoff> = LazyLock::new(Handoff::default);
/// Set when the main window closes: the job stops after its current step.
static CANCEL: AtomicBool = AtomicBool::new(false);

/// The warmed Ray-Traced backend, once. Waits while the job is still acquiring it. `None` when
/// the job did not start, its device failed, or the backend was taken before.
#[must_use]
pub fn take_ray_backend() -> Option<WhittedGpuBackend> {
    HANDOFF.after(|state| state.ray_pending, |state| state.ray.take())
}

/// The Path-Traced backend the job warms, shared. Waits while the job is still acquiring it.
/// `None` when the job did not start or its device failed.
#[must_use]
pub fn path_backend() -> Option<Arc<GpuBackend>> {
    HANDOFF.after(|state| state.path_pending, |state| state.path.clone())
}

/// The Live Render stage while no sample has landed and the job builds: its step. `None` once it
/// is done (or when it never ran).
#[must_use]
pub fn waiting_stage() -> Option<String> {
    HANDOFF.progress().stage_text()
}

/// Stops the job after its current step. Called when the main window has closed; a compile
/// already in the driver runs to its end.
pub fn cancel() {
    CANCEL.store(true, Ordering::Release);
}

/// What the job builds, read from the render context when it starts.
struct Plan {
    /// The open stone's material, when it resolves.
    open: Option<GemMaterial>,
    /// The studio lighting of the stand-in scenes. Only the tray changes the kernels; the rest
    /// keeps the stand-in close to the live view.
    preset: LightingPreset,
    light_yaw: f32,
    light_pitch: f32,
    backdrop: f32,
    /// The lighting puts the stone on a tray (Stone Shadow or Tray Reflection), whose kernels
    /// are a variant of their own.
    tray: bool,
    /// The live scene is on the tray from its first frame: a Tray Reflection applies at once,
    /// while a Stone Shadow waits for its map and the plain kernels serve until then.
    tray_first: bool,
    /// The kernel families of the viewport's own routes: the path cache's and the live ones
    /// only while their viewport switches are on.
    viewport: GpuWarmFamilies,
    max_bounces: u32,
}

impl Plan {
    fn from_context(ctx: &RenderContext) -> Self {
        let open = resolve_material_with_override(
            GemMaterial::built_in(),
            &ctx.custom_materials,
            ctx.tinted_material_override().as_ref(),
            &ctx.material_name,
        );
        // An ultraviolet lamp sends the stone to the processor; its stand-in takes the default
        // lighting instead.
        let preset = if ctx.lighting_preset.is_uv_lamp() {
            LightingPreset::default()
        } else {
            ctx.lighting_preset
        };
        let backdrop = ctx.backdrop.resolve(preset, ctx.light_yaw, ctx.light_pitch);
        // As the render worker decides it: a studio light (no HDR map, no ASET) with a lit
        // backdrop.
        let tray_lit = ctx.env_map.is_none() && preset != LightingPreset::Aset && backdrop > 0.0;
        let mirror = tray_lit && ctx.tray_reflection > 0.0;
        Self {
            open,
            preset,
            light_yaw: ctx.light_yaw,
            light_pitch: ctx.light_pitch,
            backdrop,
            tray: mirror || (tray_lit && ctx.stone_shadow),
            tray_first: mirror,
            viewport: GpuWarmFamilies {
                live: cfg!(feature = "adaptive-sampling") && ctx.adaptive_viewport,
                path_cache: ctx.path_cache_viewport,
            },
            max_bounces: ctx.max_bounces,
        }
    }

    /// The families `step` builds. On the viewport's device only its own routes', and no path
    /// cache with the tray, which that route never takes; on the job's own device every family.
    const fn families(&self, step: Step) -> GpuWarmFamilies {
        if step.shared {
            GpuWarmFamilies {
                live: self.viewport.live,
                path_cache: self.viewport.path_cache && !step.tray,
            }
        } else {
            ALL_FAMILIES
        }
    }
}

/// One Path-Traced step: a material type, plain or with the tray variant, on the viewport's
/// device (what the live view needs) or on the job's own (the disk cache only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Step {
    /// Index into the job's material list; `0` is the open stone's.
    material: usize,
    tray: bool,
    /// On the device the viewport shares.
    shared: bool,
}

/// The steps in order. First the open stone's type on the viewport's device, in the variant its
/// live scene uses first, then the other variant when a Stone Shadow will switch to it. Then,
/// with a persistent cache only, every type on the job's own device: plain, then with the tray.
fn steps(materials: usize, tray: bool, tray_first: bool, persistent: bool) -> Vec<Step> {
    let mut order = Vec::new();
    if materials == 0 {
        return order;
    }
    let live: &[bool] = match (tray, tray_first) {
        (false, _) => &[false],
        (true, false) => &[false, true],
        (true, true) => &[true],
    };
    order.extend(live.iter().map(|&tray| Step {
        material: 0,
        tray,
        shared: true,
    }));
    if persistent {
        let variants: &[bool] = if tray { &[false, true] } else { &[false] };
        for &tray in variants {
            order.extend((0..materials).map(|material| Step {
                material,
                tray,
                shared: false,
            }));
        }
    }
    order
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Builds the kernels of `step` on `gpu` with a stand-in scene: the round brilliant at the
/// default pose, the step's material and the plan's lighting.
fn warm_step(
    gpu: &GpuBackend,
    plan: &Plan,
    materials: &[GemMaterial],
    step: Step,
    cancel: &AtomicBool,
) -> GpuAccumulate {
    let Some(material) = materials.get(step.material) else {
        return GpuAccumulate::Declined;
    };
    let camera = Camera::new(
        DEFAULT_POSE.yaw,
        DEFAULT_POSE.pitch,
        DEFAULT_POSE.distance,
        DEFAULT_FOV_DEG,
    );
    let planes = StandardGemCuts::standard_round_brilliant();
    let studio = plan
        .preset
        .studio(1.0, plan.light_yaw, plan.light_pitch)
        .with_backdrop(plan.backdrop);
    let environment = if step.tray {
        // A Tray Reflection stands in for every tray: with the Stone Shadow the kernels have the
        // same sources and constants (`material_class::pipeline_key` folds the two).
        let stone = StoneGeometry {
            planes: &planes,
            tools: &[],
        };
        match tray_mirror_for(STAND_IN_REFLECTION, studio, stone) {
            Ok(Some(mirror)) => studio.with_tray_mirror(Some(mirror)),
            _ => return GpuAccumulate::Declined,
        }
    } else {
        studio
    };
    let scene = GpuSceneRef {
        camera: &camera,
        width: STAND_IN_SIZE,
        height: STAND_IN_SIZE,
        planes: &planes,
        facet_finishes: &[],
        material,
        max_bounces: plan.max_bounces,
        environment,
        estimator: None,
    };
    gpu.warm_pipelines_cancellable(
        &scene,
        &[],
        &SurfaceScene::IDEAL,
        plan.families(step),
        cancel,
    )
}

/// Builds the cache: the Ray-Traced kernel, then the Path-Traced steps of `plan`. Stops between
/// steps once `cancel` is set, or when a device is lost or cannot be acquired.
fn run(handoff: &Handoff, plan: &Plan, cancel: &AtomicBool) {
    let _finish = Finish(handoff);
    handoff.update(|state| state.progress.ray = Bar::running(0, 1));
    let ray = WhittedGpuBackend::acquire();
    let ray_ready = ray.adapter_label().is_some();
    handoff.update(|state| {
        state.ray_pending = false;
        state.ray = ray_ready.then_some(ray);
        state.progress.ray = if ray_ready {
            Bar::ready(1)
        } else {
            Bar::UNAVAILABLE
        };
    });
    if cancel.load(Ordering::Acquire) {
        return;
    }

    handoff.update(|state| state.progress.path = Bar::running(0, 0));
    let materials = GpuBackend::warm_up_materials(plan.open.as_ref());
    let gpu = Arc::new(GpuBackend::acquire());
    let usable = gpu.adapter_label().is_some();
    let persistent = gpu.persists_pipelines();
    let order = if usable {
        steps(materials.len(), plan.tray, plan.tray_first, persistent)
    } else {
        Vec::new()
    };
    let total = count(order.len());
    handoff.update(|state| {
        state.path_pending = false;
        state.path = usable.then(|| Arc::clone(&gpu));
        state.progress.persistent = usable.then_some(persistent);
        state.progress.path = if usable {
            Bar::running(0, total)
        } else {
            Bar::UNAVAILABLE
        };
    });
    let mut own: Option<GpuBackend> = None;
    let mut done = 0;
    for &step in &order {
        if cancel.load(Ordering::Acquire) || gpu.is_lost() {
            break;
        }
        let device: &GpuBackend = if step.shared {
            &gpu
        } else {
            own.get_or_insert_with(GpuBackend::acquire)
        };
        if device.adapter_label().is_none() {
            tracing::warn!(
                "the shader cache build has no usable device; the rest compiles on first use"
            );
            break;
        }
        let started = Instant::now();
        let outcome = warm_step(device, plan, &materials, step, cancel);
        tracing::info!(
            material = materials.get(step.material).map_or("", |m| m.name.as_str()),
            tray = step.tray,
            shared = step.shared,
            ?outcome,
            elapsed_ms = started.elapsed().as_millis(),
            "shader cache step"
        );
        // A decline is final: this device has nothing to build for the step (a tray it cannot
        // bind, say). A cancel or a lost device leaves the step unbuilt.
        if outcome == GpuAccumulate::Cancelled || device.is_lost() {
            break;
        }
        done += 1;
        handoff.update(|state| state.progress.path.done = done);
    }
    let lost = gpu.is_lost();
    handoff.update(|state| {
        state.progress.path = if !usable || lost {
            Bar::UNAVAILABLE
        } else if done == total {
            Bar::ready(total)
        } else {
            Bar::stopped(done, total)
        };
    });
}

/// Starts the job in the background, and the UI that reports it. Once per process, from the
/// main window's setup. A build without the `gpu` feature has no kernels to build, and with Local
/// Compute set to the CPU no device is opened.
pub fn start(ui: &MainWindow, ctx: &Arc<Mutex<RenderContext>>) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if !cfg!(feature = "gpu") {
        return;
    }
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    // The UI harness drives the window itself: a background compile would make its timings and
    // pictures depend on the driver's cache. A harness build started as the app builds it.
    #[cfg(feature = "ui-harness")]
    if crate::harness_support::seams_armed()
        || crate::harness_support::viewport_rendering_disabled()
    {
        return;
    }
    let plan = {
        let ctx = ctx.lock().unwrap_or_else(PoisonError::into_inner);
        if ctx.local_compute_target == LocalComputeTarget::Cpu {
            return;
        }
        let plan = Plan::from_context(&ctx);
        drop(ctx);
        plan
    };
    // Pending from the start: a viewport that asks before the job reaches a renderer waits for
    // the job's device instead of compiling the same kernels on a second one.
    HANDOFF.update(|state| {
        state.progress.running = true;
        state.ray_pending = true;
        state.path_pending = true;
    });
    let spawned = std::thread::Builder::new()
        .name("pleochroma-shader-cache".to_owned())
        .spawn(move || run(&HANDOFF, &plan, &CANCEL));
    if let Err(error) = spawned {
        tracing::warn!(
            %error,
            "could not start the shader cache build; the renderers build their kernels on first use"
        );
        drop(Finish(&HANDOFF));
        return;
    }
    watch(ui);
}

/// The card's own state between ticks.
struct Card {
    started: Instant,
    /// The card was put up: the job ran past [`SHOW_AFTER`].
    shown: bool,
    /// The user pressed Hide.
    dismissed: bool,
    /// When the job ended.
    finished: Option<Instant>,
    /// The status-strip activity, while one is up.
    activity: Option<i32>,
}

thread_local! {
    static TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

/// Installs the UI-thread timer that reports the job, and the card's Hide.
fn watch(ui: &MainWindow) {
    let card = Rc::new(RefCell::new(Card {
        started: Instant::now(),
        shown: false,
        dismissed: false,
        finished: None,
        activity: None,
    }));
    let hide_card = Rc::clone(&card);
    let hide_ui = ui.as_weak();
    ui.global::<ShaderCacheModel>().on_hide(move || {
        hide_card.borrow_mut().dismissed = true;
        if let Some(ui) = hide_ui.upgrade() {
            ui.global::<ShaderCacheModel>().set_visible(false);
        }
    });
    let weak = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, TICK, move || {
        let again = weak.upgrade().map_or_else(
            || {
                cancel();
                false
            },
            |ui| tick(&ui, &mut card.borrow_mut(), Instant::now()),
        );
        if !again {
            TIMER.with(|slot| {
                if let Some(timer) = slot.borrow().as_ref() {
                    timer.stop();
                }
            });
        }
    });
    TIMER.with(|slot| *slot.borrow_mut() = Some(timer));
}

/// One look at the job: the card, the activity and the Live Render stage. Returns whether to
/// look again.
fn tick(ui: &MainWindow, card: &mut Card, now: Instant) -> bool {
    let progress = HANDOFF.progress();
    if !progress.running && card.finished.is_none() {
        card.finished = Some(now);
    }
    if progress.running && !card.shown && now.duration_since(card.started) >= SHOW_AFTER {
        card.shown = true;
        card.activity = Some(ui.global::<ActivityModel>().invoke_start_external(
            "shader_cache".into(),
            "Building shader cache".into(),
            false,
        ));
    }
    let lingering = card
        .finished
        .is_some_and(|at| now.duration_since(at) < LINGER);
    let visible = card.shown && !card.dismissed && (progress.running || lingering);
    publish(ui, &progress, visible);
    if let Some(id) = card.activity {
        let activity = ui.global::<ActivityModel>();
        if progress.running {
            activity.invoke_progress_external(id, progress.fraction());
        } else {
            activity.invoke_finish_external(id);
            card.activity = None;
        }
    }
    refresh_stage(ui, &progress);
    progress.running || visible
}

/// The card's texts.
struct CardText {
    title: &'static str,
    note: &'static str,
    ray: String,
    path: String,
}

const fn state_word(state: BarState) -> &'static str {
    match state {
        BarState::Waiting => "Waiting",
        BarState::Running => "Compiling",
        BarState::Ready => "Ready",
        BarState::Stopped => "Stopped",
        BarState::Unavailable => "Not Available",
    }
}

fn card_text(progress: &Progress) -> CardText {
    let stopped = progress.path.state == BarState::Stopped;
    let title = if progress.running {
        "Building the Shader Cache"
    } else if stopped {
        "Shader Cache Partly Built"
    } else if progress.ray.state == BarState::Ready || progress.path.state == BarState::Ready {
        "Shader Cache Ready"
    } else {
        "Shader Cache Not Built"
    };
    let note = if progress.persistent == Some(false) {
        NOTE_EVERY_START
    } else if stopped {
        NOTE_PARTIAL
    } else {
        NOTE_ONCE
    };
    let path = match progress.path {
        Bar {
            state: BarState::Running,
            total: 0,
            ..
        } => "Starting".to_owned(),
        Bar {
            state: BarState::Running,
            done,
            total,
        } => format!("{done} of {total}"),
        Bar {
            state: BarState::Stopped,
            done,
            total,
        } => format!("Stopped at {done} of {total}"),
        bar => state_word(bar.state).to_owned(),
    };
    CardText {
        title,
        note,
        ray: state_word(progress.ray.state).to_owned(),
        path,
    }
}

fn publish(ui: &MainWindow, progress: &Progress, visible: bool) {
    let text = card_text(progress);
    let model = ui.global::<ShaderCacheModel>();
    model.set_visible(visible);
    model.set_running(progress.running);
    model.set_title(text.title.into());
    model.set_note(text.note.into());
    model.set_ray_text(text.ray.into());
    model.set_ray_fraction(progress.ray.fraction());
    model.set_ray_busy(progress.ray.state == BarState::Running);
    model.set_path_text(text.path.into());
    model.set_path_fraction(progress.path.fraction());
}

/// Keeps the Live Render stage on the job's step while no sample has landed, and gives
/// [`PREPARING_TRACE`] back once the job ends. The render loop writes the stage when a frame
/// starts, and a viewport waiting for its kernels starts none.
fn refresh_stage(ui: &MainWindow, progress: &Progress) {
    let model = ui.global::<ViewportModel>();
    if model.get_render_samples_done() > 0 {
        return;
    }
    let stage = model.get_render_progress_stage();
    if stage.as_str() != PREPARING_TRACE && !stage.starts_with(STAGE_PREFIX) {
        return;
    }
    let text = progress
        .stage_text()
        .unwrap_or_else(|| PREPARING_TRACE.to_owned());
    if stage.as_str() != text {
        model.set_render_progress_stage(text.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn shared(tray: bool) -> Step {
        Step {
            material: 0,
            tray,
            shared: true,
        }
    }

    const fn own(material: usize, tray: bool) -> Step {
        Step {
            material,
            tray,
            shared: false,
        }
    }

    #[test]
    fn the_viewport_device_builds_the_open_stone_in_the_variant_its_scene_uses_first() {
        // No tray: the plain kernels, then every type on the job's own device.
        assert_eq!(
            steps(3, false, false, true),
            vec![shared(false), own(0, false), own(1, false), own(2, false)]
        );
        // A Stone Shadow: plain until its map is built, then the tray variant.
        assert_eq!(
            steps(2, true, false, true),
            vec![
                shared(false),
                shared(true),
                own(0, false),
                own(1, false),
                own(0, true),
                own(1, true),
            ]
        );
        // A Tray Reflection: on the tray from the first frame.
        assert_eq!(steps(2, true, true, true)[0], shared(true));
        assert_eq!(steps(2, true, true, true).len(), 5);
    }

    #[test]
    fn a_device_without_a_persistent_cache_builds_the_open_stone_only() {
        assert_eq!(
            steps(3, true, false, false),
            vec![shared(false), shared(true)]
        );
        assert_eq!(steps(3, false, false, false), vec![shared(false)]);
        assert!(steps(0, true, true, true).is_empty());
    }

    #[test]
    fn the_viewport_device_builds_only_the_viewport_routes_families() {
        let plan = Plan {
            open: None,
            preset: LightingPreset::default(),
            light_yaw: 0.0,
            light_pitch: 0.0,
            backdrop: 1.0,
            tray: true,
            tray_first: false,
            viewport: GpuWarmFamilies {
                live: false,
                path_cache: true,
            },
            max_bounces: 8,
        };
        assert_eq!(
            plan.families(shared(false)),
            GpuWarmFamilies {
                live: false,
                path_cache: true,
            }
        );
        // The cached route never takes a scene on the tray.
        assert!(!plan.families(shared(true)).path_cache);
        assert_eq!(plan.families(own(1, true)), ALL_FAMILIES);
    }

    #[test]
    fn a_warmed_ray_backend_is_taken_once() {
        let handoff = Handoff::default();
        handoff.update(|state| state.ray = Some(WhittedGpuBackend::disabled()));
        let take = || handoff.after(|state| state.ray_pending, |state| state.ray.take());
        assert!(take().is_some());
        assert!(take().is_none(), "the viewport owns it now");
    }

    #[test]
    fn the_viewport_waits_while_the_job_acquires_the_device() {
        let handoff = Arc::new(Handoff::default());
        handoff.update(|state| state.path_pending = true);
        let waiter = {
            let handoff = Arc::clone(&handoff);
            std::thread::spawn(move || {
                handoff
                    .after(|state| state.path_pending, |state| state.path.clone())
                    .is_some()
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !waiter.is_finished(),
            "it waits while the device is pending"
        );
        handoff.update(|state| {
            state.path_pending = false;
            state.path = Some(Arc::new(GpuBackend::disabled()));
        });
        assert!(waiter.join().expect("the waiting thread"));
    }

    #[test]
    fn a_job_that_ends_early_releases_every_waiter() {
        let handoff = Handoff::default();
        handoff.update(|state| {
            state.progress.running = true;
            state.ray_pending = true;
            state.path_pending = true;
        });
        drop(Finish(&handoff));
        let (ray, path, running) = {
            let state = handoff.lock();
            (
                state.ray_pending,
                state.path_pending,
                state.progress.running,
            )
        };
        assert!(!ray && !path && !running);
        assert!(
            handoff
                .after(|state| state.path_pending, |state| state.path.clone())
                .is_none()
        );
    }

    #[test]
    fn the_stage_names_the_renderer_and_the_step_in_progress() {
        let mut progress = Progress {
            running: true,
            ray: Bar::running(0, 1),
            ..Progress::default()
        };
        assert_eq!(
            progress.stage_text().as_deref(),
            Some("Building shader cache: Ray-Traced")
        );
        progress.ray = Bar::ready(1);
        progress.path = Bar::running(2, 9);
        assert_eq!(
            progress.stage_text().as_deref(),
            Some("Building shader cache: Path-Traced 3 of 9")
        );
        progress.path = Bar::running(0, 0);
        assert_eq!(
            progress.stage_text().as_deref(),
            Some("Building shader cache")
        );
        progress.running = false;
        assert_eq!(progress.stage_text(), None);
    }

    #[test]
    fn the_activity_share_never_falls_back_while_the_steps_are_counted() {
        let mut progress = Progress {
            running: true,
            ray: Bar::ready(1),
            path: Bar::running(0, 0),
            persistent: None,
        };
        let before = progress.fraction();
        assert!((before - RAY_SHARE).abs() < 1e-6, "Ray-Traced is done");
        progress.path = Bar::running(0, 9);
        assert!(
            progress.fraction() >= before,
            "the total arriving takes nothing back"
        );
        progress.path = Bar::running(1, 9);
        assert!(
            (progress.fraction() - 0.2).abs() < 1e-6,
            "one tenth plus one ninth of the rest"
        );
    }

    #[test]
    fn the_card_counts_the_steps_and_says_how_the_build_ended() {
        let progress = Progress {
            running: true,
            ray: Bar::ready(1),
            path: Bar::running(3, 9),
            persistent: Some(true),
        };
        let text = card_text(&progress);
        assert_eq!(text.title, "Building the Shader Cache");
        assert_eq!(text.ray, "Ready");
        assert_eq!(text.path, "3 of 9");
        assert_eq!(text.note, NOTE_ONCE);

        let every_start = Progress {
            running: false,
            path: Bar::ready(2),
            persistent: Some(false),
            ..progress
        };
        let text = card_text(&every_start);
        assert_eq!(text.title, "Shader Cache Ready");
        assert_eq!(text.path, "Ready");
        assert_eq!(text.note, NOTE_EVERY_START);

        let stopped = Progress {
            running: false,
            path: Bar::stopped(4, 9),
            ..progress
        };
        let text = card_text(&stopped);
        assert_eq!(text.title, "Shader Cache Partly Built");
        assert_eq!(text.path, "Stopped at 4 of 9");
        assert_eq!(text.note, NOTE_PARTIAL);

        let none = Progress {
            running: false,
            ray: Bar::UNAVAILABLE,
            path: Bar::UNAVAILABLE,
            persistent: None,
        };
        assert_eq!(card_text(&none).title, "Shader Cache Not Built");
        assert_eq!(card_text(&none).path, "Not Available");
        assert_eq!(
            card_text(&none).note,
            NOTE_ONCE,
            "no driver claim without a device"
        );
    }
}
