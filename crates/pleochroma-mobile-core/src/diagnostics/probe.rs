//! The GPU probe: builds every shader of both renderers on its own thread and traces one tiny picture with each.
//!
//! The probe takes its own graphics contexts and never touches the live render's. Every stage is
//! a breadcrumb step, so a crash inside the driver names the stage in the next start's crash log.
//! A probe stage sets no engine mark and no cache count: the probe is a test, and its result is in
//! its own file, `probe-<stamp>.log`, which is written whether or not the tracer log is on.
//!
//! The "probe running" state is process-wide, not part of a `Session`, so a relaunched activity
//! in the same process still sees it.

use super::{
    session::Session,
    store::{LogKind, LogStore},
};
use std::{
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Write as _},
    panic::AssertUnwindSafe,
    path::Path,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

/// The group titles, in the order the page shows them.
const TITLES: [&str; 8] = [
    "Path-Traced · isotropic shaders",
    "Path-Traced · uniaxial shaders",
    "Path-Traced · biaxial shaders",
    "Path-Traced · tiny picture",
    "Ray-Traced · start the graphics card",
    "Ray-Traced · build the shader",
    "Ray-Traced · tiny picture",
    "Shader cache",
];

/// The index of the first Path-Traced shader group; the next two follow it.
#[cfg(feature = "device-gpu")]
const PT_FIRST: usize = 0;
/// The index of the Path-Traced tiny-picture group.
#[cfg(feature = "device-gpu")]
const PT_PICTURE: usize = 3;
/// The index of the Ray-Traced "start the graphics card" group.
#[cfg(feature = "device-gpu")]
const RT_START: usize = 4;
/// The index of the Ray-Traced "build the shader" group.
#[cfg(feature = "device-gpu")]
const RT_BUILD: usize = 5;
/// The index of the Ray-Traced tiny-picture group.
#[cfg(feature = "device-gpu")]
const RT_PICTURE: usize = 6;
/// The index of the shader-cache group.
#[cfg(feature = "device-gpu")]
const CACHE: usize = 7;

/// The note of a group the probe never reached because an earlier stage failed.
#[cfg(any(test, feature = "device-gpu"))]
const NOT_REACHED: &str = "Not run: an earlier stage failed";

/// How far one group of the probe has come.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeGroup {
    /// The group's title, for example "Path-Traced · isotropic shaders".
    pub title: String,
    /// Stages that finished well.
    pub done: u32,
    /// Stages the group has.
    pub total: u32,
    /// Milliseconds spent in the group so far, or in all of it once it ended.
    pub ms: u64,
    /// Where the group stands.
    pub state: GroupState,
    /// The first rejection or the reason the group did not run; empty when there is none.
    pub note: String,
}

/// Where one probe group stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupState {
    /// Not started.
    Waiting,
    /// In progress.
    Running,
    /// Every stage of the group finished well.
    Ok,
    /// A stage was rejected, or the group did not run.
    Failed,
}

/// What the developer page shows while the probe runs and after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeSnapshot {
    /// True while the probe thread runs.
    pub running: bool,
    /// The probe file's name (no folder), once the run has started.
    pub file: Option<String>,
    /// The eight groups, empty before the first run in this process.
    pub groups: Vec<ProbeGroup>,
    /// Stages that finished well, over all groups.
    pub ok: u32,
    /// Stages the probe has, over all groups.
    pub total: u32,
}

/// What the page reads while the probe runs and after: the file name and the groups.
struct State {
    file: Option<String>,
    groups: Vec<ProbeGroup>,
}

/// True from `start` until the probe thread ends. Process-wide on purpose (section 4.8).
static RUNNING: AtomicBool = AtomicBool::new(false);
/// Set by `cancel`; the probe checks it between stages.
static CANCEL: AtomicBool = AtomicBool::new(false);
/// The state of the newest run in this process.
static STATE: Mutex<State> = Mutex::new(State {
    file: None,
    groups: Vec::new(),
});
/// The hook that runs once when the probe ends.
static HOOK: Mutex<Option<Arc<dyn Fn(u32, u32) + Send + Sync>>> = Mutex::new(None);

/// Runs `f` on the shared state; the lock is held only for `f`.
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    f(&mut STATE.lock().unwrap_or_else(PoisonError::into_inner))
}

/// True while the probe thread runs, also after the activity that started it was recreated.
#[must_use]
pub fn is_running() -> bool {
    RUNNING.load(Ordering::Acquire)
}

/// The groups of the newest run in this process, with their live counts.
#[must_use]
pub fn snapshot() -> ProbeSnapshot {
    let running = is_running();
    with_state(|state| ProbeSnapshot {
        running,
        file: state.file.clone(),
        ok: state.groups.iter().map(|group| group.done).sum(),
        total: state.groups.iter().map(|group| group.total).sum(),
        groups: state.groups.clone(),
    })
}

/// Starts the probe thread. The caller has stopped the live render.
///
/// # Errors
///
/// A text for the toast when a probe already runs or the thread could not be created.
pub fn start(session: &Arc<Session>) -> Result<(), String> {
    if RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("The GPU probe is already running.".to_owned());
    }
    CANCEL.store(false, Ordering::Release);
    reset_state();
    let session = Arc::clone(session);
    let spawned = std::thread::Builder::new()
        .name("gpu-probe".to_owned())
        .spawn(move || thread_main(&session));
    match spawned {
        Ok(_detached) => Ok(()),
        Err(error) => {
            RUNNING.store(false, Ordering::Release);
            Err(format!("The GPU probe could not start: {error}"))
        }
    }
}

/// Asks a running probe to stop at the next stage boundary; does nothing when none runs.
pub fn cancel() {
    if is_running() {
        CANCEL.store(true, Ordering::Release);
    }
}

/// The result of the newest probe file that holds one, as `(ok, total)`; `None` without one.
///
/// A file of a crashed or cancelled run holds no result line and is skipped.
#[must_use]
pub fn last_result(store: &LogStore) -> Option<(u32, u32)> {
    store
        .list()
        .into_iter()
        .filter(|entry| entry.kind == LogKind::Probe)
        .find_map(|entry| {
            let (text, _whole) = store.read_tail(&entry.name, 4096).ok()?;
            parse_result(&text)
        })
}

/// Sets the hook that runs once on the probe thread when it ends, after the running flag was
/// cleared; `None` removes it. The hook must not call this function.
pub fn set_finished_hook(hook: Option<Box<dyn Fn(u32, u32) + Send + Sync>>) {
    *HOOK.lock().unwrap_or_else(PoisonError::into_inner) = hook.map(Arc::from);
}

/// Dies in a GPU step on purpose, so the next start's crash log can be checked (debug builds).
///
/// The step stays open and the process aborts, which Android reports as a native crash.
#[cfg(debug_assertions)]
pub fn crash_in_gpu_step(engine: pleochroma::diag::Engine) -> ! {
    use pleochroma::diag::{Step, StepKind, enter};
    let _step = enter(&Step {
        kind: StepKind::DebugCrash,
        engine: Some(engine),
        label: "debug crash",
        class: "",
    });
    std::process::abort()
}

/// How a run ended.
enum Outcome {
    /// Every stage was tried (some may have failed).
    Finished,
    /// The user stopped the probe.
    #[cfg_attr(
        not(feature = "device-gpu"),
        expect(dead_code, reason = "only the device GPU stages can be stopped")
    )]
    Cancelled,
}

/// Why the stages stopped before the end.
#[cfg(feature = "device-gpu")]
enum Halt {
    /// `cancel` was called.
    Cancelled,
    /// No graphics context could be acquired again.
    Lost(String),
}

/// The probe thread: runs the stages, then clears the flag and calls the hook.
fn thread_main(session: &Session) {
    let ran = std::panic::catch_unwind(AssertUnwindSafe(|| run(session)));
    if ran.is_err() {
        settle("The probe stopped unexpectedly", true);
    }
    let (ok, total) = totals_now();
    RUNNING.store(false, Ordering::Release);
    let hook = HOOK.lock().unwrap_or_else(PoisonError::into_inner).clone();
    if let Some(hook) = hook {
        hook(ok, total);
    }
}

/// One whole run: header, stages, result line.
fn run(session: &Session) {
    let mut rep = Reporter::create(session.store().dir());
    rep.header(session);
    let outcome = run_stages(&mut rep);
    let (ok, total) = totals_now();
    match outcome {
        Outcome::Finished => rep.line(&format!("probe result {ok} of {total} ok")),
        Outcome::Cancelled => rep.line(&format!("probe cancelled after {ok} of {total}")),
    }
}

/// The stages of a build without device GPU support: none can run.
#[cfg(not(feature = "device-gpu"))]
fn run_stages(rep: &mut Reporter) -> Outcome {
    const NOT_BUILT: &str = "device GPU support is not built into this app";
    rep.line(NOT_BUILT);
    settle(NOT_BUILT, true);
    Outcome::Finished
}

/// Runs the stages unless the GPU start gate stays busy for ten seconds.
#[cfg(feature = "device-gpu")]
fn run_stages(rep: &mut Reporter) -> Outcome {
    const BLOCKED: &str = "GPU start is blocked by an earlier hung step";
    if pleochroma::diag::gpu_start_blocked(std::time::Duration::from_secs(10)) {
        rep.line(BLOCKED);
        settle(BLOCKED, true);
        return Outcome::Finished;
    }
    match stages::run_all(rep) {
        Ok(()) => {
            settle(NOT_REACHED, true);
            Outcome::Finished
        }
        Err(Halt::Cancelled) => {
            settle("Stopped", false);
            Outcome::Cancelled
        }
        Err(Halt::Lost(why)) => {
            rep.line(&format!("probe stopped: {why}"));
            settle(&why, true);
            Outcome::Finished
        }
    }
}

/// The group counts of a run that starts now.
#[cfg(feature = "device-gpu")]
fn planned_totals() -> [u32; 8] {
    stages::totals()
}

/// The group counts of a build without device GPU support: one stage each, none can run.
#[cfg(not(feature = "device-gpu"))]
const fn planned_totals() -> [u32; 8] {
    [1; 8]
}

/// Puts the eight groups in the waiting state for a new run.
fn reset_state() {
    let totals = planned_totals();
    with_state(|state| {
        state.file = None;
        state.groups = TITLES
            .iter()
            .zip(totals)
            .map(|(title, total)| ProbeGroup {
                title: (*title).to_owned(),
                done: 0,
                total,
                ms: 0,
                state: GroupState::Waiting,
                note: String::new(),
            })
            .collect();
    });
}

/// Marks the groups still running as failed, and the waiting ones too when `waiting_too`.
fn settle(note: &str, waiting_too: bool) {
    with_state(|state| {
        for group in &mut state.groups {
            let touched = match group.state {
                GroupState::Running => true,
                GroupState::Waiting => waiting_too,
                GroupState::Ok | GroupState::Failed => false,
            };
            if touched {
                group.state = GroupState::Failed;
                if group.note.is_empty() {
                    note.clone_into(&mut group.note);
                }
            }
        }
    });
}

/// Stages done and stages planned, over all groups.
fn totals_now() -> (u32, u32) {
    with_state(|state| {
        (
            state.groups.iter().map(|group| group.done).sum(),
            state.groups.iter().map(|group| group.total).sum(),
        )
    })
}

/// The first line of `text`, cut to a length one log line can carry.
#[cfg(any(test, feature = "device-gpu"))]
fn first_line(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(300)
        .collect()
}

/// The note of a group whose stages were rejected: the first one, and how many followed.
#[cfg(feature = "device-gpu")]
fn rejected_note(rejected: &[String]) -> String {
    match rejected {
        [] => String::new(),
        [only] => format!("Rejected: {only}"),
        [first, rest @ ..] => format!("Rejected: {first} (and {} more)", rest.len()),
    }
}

/// `(ok, total)` of the last `probe result` line in `text`.
fn parse_result(text: &str) -> Option<(u32, u32)> {
    text.lines().rev().find_map(|line| {
        let (_before, rest) = line.split_once("probe result ")?;
        let mut words = rest.split_whitespace();
        let ok = words.next()?.parse().ok()?;
        (words.next()? == "of").then_some(())?;
        let total = words.next()?.parse().ok()?;
        Some((ok, total))
    })
}

/// Writes the probe file and keeps the shared group state; lives on the probe thread.
struct Reporter {
    /// `None` when the file could not be created or a write failed (storage full).
    file: Option<File>,
    started: Instant,
    /// When the group in progress began.
    #[cfg_attr(
        not(feature = "device-gpu"),
        expect(dead_code, reason = "only the device GPU stages time their groups")
    )]
    group_started: Instant,
}

impl Reporter {
    /// Creates `probe-<stamp>.log` in `logs_dir`; the run goes on without a file when that fails.
    fn create(logs_dir: &Path) -> Self {
        let now = Instant::now();
        let mut rep = Self {
            file: None,
            started: now,
            group_started: now,
        };
        let created = create_probe_file(logs_dir);
        if let Some((file, name)) = created {
            rep.file = Some(file);
            with_state(|state| state.file = Some(name));
        }
        rep
    }

    /// A reporter that writes no file, for the tests.
    #[cfg(test)]
    fn detached() -> Self {
        let now = Instant::now();
        Self {
            file: None,
            started: now,
            group_started: now,
        }
    }

    /// Writes one whole line with the time since the probe started, then flushes. A failed
    /// write ends the file but never the probe.
    fn line(&mut self, text: &str) {
        let Some(file) = &mut self.file else {
            return;
        };
        let line = format!("+{:.3} s {text}\n", self.started.elapsed().as_secs_f64());
        if file
            .write_all(line.as_bytes())
            .and_then(|()| file.flush())
            .is_err()
        {
            self.file = None;
        }
    }

    /// The header: the app and the device, never an account, id or serial.
    fn header(&mut self, session: &Session) {
        let facts = session.facts();
        self.line(&format!("Pleochroma GPU probe · app {}", facts.app_version));
        self.line(&format!(
            "device {} {} · Android {} (SDK {}) · {} · {} MB",
            facts.manufacturer,
            facts.model,
            facts.android_release,
            facts.sdk_int,
            facts.abi,
            facts.total_ram_mb,
        ));
    }

    /// True once `cancel` was called.
    #[cfg(feature = "device-gpu")]
    fn cancelled() -> bool {
        CANCEL.load(Ordering::Acquire)
    }

    /// Group `idx` starts.
    #[cfg(feature = "device-gpu")]
    fn begin(&mut self, idx: usize) {
        self.group_started = Instant::now();
        with_state(|state| {
            if let Some(group) = state.groups.get_mut(idx) {
                group.state = GroupState::Running;
            }
        });
    }

    /// Group `idx` has `done` finished stages.
    #[cfg(feature = "device-gpu")]
    fn count(&self, idx: usize, done: u32) {
        let ms = elapsed_ms(self.group_started);
        with_state(|state| {
            if let Some(group) = state.groups.get_mut(idx) {
                group.done = done;
                group.ms = ms;
            }
        });
    }

    /// Group `idx` ends well or badly, with its note.
    #[cfg(feature = "device-gpu")]
    fn end(&mut self, idx: usize, ok: bool, note: &str) {
        let ms = elapsed_ms(self.group_started);
        let summary = with_state(|state| {
            let group = state.groups.get_mut(idx)?;
            if ok {
                group.done = group.total;
            }
            group.ms = ms;
            group.state = if ok {
                GroupState::Ok
            } else {
                GroupState::Failed
            };
            note.clone_into(&mut group.note);
            Some((group.title.clone(), group.done, group.total))
        });
        if let Some((title, done, total)) = summary {
            let verdict = if ok { "ok" } else { "failed" };
            let tail = if note.is_empty() {
                String::new()
            } else {
                format!(" · {note}")
            };
            self.line(&format!(
                "probe group {title}: {done} of {total} {verdict} {ms} ms{tail}"
            ));
        }
    }

    /// A stage finished well.
    #[cfg(feature = "device-gpu")]
    fn stage_ok(&mut self, stage: &str, started: Instant) {
        self.line(&format!("probe {stage} ok {} ms", elapsed_ms(started)));
    }

    /// A stage was rejected, with the device's own message.
    #[cfg(feature = "device-gpu")]
    fn stage_rejected(&mut self, stage: &str, why: &str) {
        self.line(&format!("probe {stage} rejected: {}", first_line(why)));
    }
}

/// Milliseconds since `started`.
#[cfg(feature = "device-gpu")]
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Creates the probe file and returns it with its name. A second probe in the same minute gets
/// the seconds in its stamp, and a third a counter.
fn create_probe_file(logs_dir: &Path) -> Option<(File, String)> {
    fs::create_dir_all(logs_dir).ok()?;
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let [year, month, day, hour, minute, second] = utc_fields(secs);
    let base = format!("probe-{year:04}{month:02}{day:02}-{hour:02}{minute:02}");
    let candidates = [
        format!("{base}.log"),
        format!("{base}{second:02}.log"),
        format!("{base}{second:02}-2.log"),
        format!("{base}{second:02}-3.log"),
    ];
    for name in candidates {
        match OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(logs_dir.join(&name))
        {
            Ok(file) => return Some((file, name)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
    }
    None
}

/// Year, month, day, hour, minute and second (UTC) of `secs` since the Unix epoch.
fn utc_fields(secs: u64) -> [u64; 6] {
    let days = secs / 86_400;
    let rest = secs % 86_400;
    // Civil-from-days, valid for every date from 1970 on.
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    [year, month, day, rest / 3600, rest % 3600 / 60, rest % 60]
}

/// The stages that need the device GPU: one function per group.
#[cfg(feature = "device-gpu")]
mod stages {
    use super::{
        CACHE, Halt, PT_FIRST, PT_PICTURE, RT_BUILD, RT_PICTURE, RT_START, Reporter, first_line,
        rejected_note,
    };
    use glam::Vec3;
    use pleochroma::{
        diag::{self, Engine, Step, StepGuard, StepKind},
        geometry::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
        renderer::{
            engine::{EngineSpec, RayOptions, RenderEngine},
            gpu::{
                GpuContext, compute,
                event_wavefront::{EventWavefrontRenderer, EwKernel, kernels::arms_for},
                frame::{classify_material, material_class},
                whitted::{
                    CHUNK_PIXELS, WhittedGpuRange, WhittedGpuRenderer, WhittedGpuTurnLimits,
                },
            },
        },
    };
    use pleochroma_web_core::{
        gpu_admission::browser_pass_class,
        scene::{
            CameraSpec, FinishSpec, LightingSpec, MaterialSpec, OwnedScene, SceneSpec,
            planes_to_data,
        },
    };
    use std::time::Instant;

    /// Width of the tiny picture.
    const TINY_WIDTH: u32 = 16;
    /// Height of the tiny picture.
    const TINY_HEIGHT: u32 = 12;

    /// One pass class the Path-Traced groups build, with the material that has it.
    struct Class {
        name: &'static str,
        material: &'static str,
        /// The pass class value, `None` when the material has no browser pass class.
        value: Option<u32>,
    }

    /// The three pass classes the phone traces with, as `ew_kernel_probe` builds them: a plain
    /// stone of each kind (isotropic, uniaxial, biaxial) and the class the renderer picks for it.
    fn classes() -> Vec<Class> {
        [
            ("isotropic", "Diamond"),
            ("uniaxial", "Sapphire"),
            ("biaxial", "Tanzanite"),
        ]
        .into_iter()
        .map(|(name, material)| Class {
            name,
            material,
            value: GemMaterial::by_name(material)
                .and_then(|gem| browser_pass_class(classify_material(&gem))),
        })
        .collect()
    }

    /// The kernels a pass class builds: every shared kernel and the arms of that class.
    fn kernels_for(class: u32) -> Vec<EwKernel> {
        let arms = arms_for(class, true);
        EwKernel::ALL
            .into_iter()
            .filter(|kernel| !is_arm(*kernel) || arms.contains(kernel))
            .collect()
    }

    /// Whether `kernel` is one of the surface interaction arm kernels, which `arms_for` selects
    /// per class.
    const fn is_arm(kernel: EwKernel) -> bool {
        matches!(
            kernel,
            EwKernel::ArmFrosted
                | EwKernel::ArmTir
                | EwKernel::ArmUniEntry
                | EwKernel::ArmUniEntryTransmit
                | EwKernel::ArmUniEntryFinish
                | EwKernel::ArmUniInternal
                | EwKernel::ArmUniInternalTransmit
                | EwKernel::ArmGeneric
        )
    }

    /// The stage counts of the eight groups.
    pub(super) fn totals() -> [u32; 8] {
        let count = |class: &Class| {
            class.value.map_or(1, |value| {
                u32::try_from(kernels_for(value).len()).unwrap_or(1)
            })
        };
        let classes = classes();
        let [iso, uni, bi] = [0, 1, 2].map(|i| classes.get(i).map_or(1, count));
        [iso, uni, bi, 3, 1, 1, 1, 3]
    }

    /// A graphics context the stages build on, replaced after a rejection.
    struct Device {
        ctx: GpuContext,
    }

    impl Device {
        /// A fresh context, or the reason there is none.
        fn acquire() -> Result<Self, String> {
            GpuContext::acquire()
                .map(|ctx| Self { ctx })
                .map_err(|error| format!("no GPU device: {error}"))
        }

        /// The first failure the device recorded, `None` while it has none.
        fn failure(&self) -> Option<String> {
            diag::device_failure(&self.ctx)
        }
    }

    /// The breadcrumb step of one probe stage.
    fn step(engine: Option<Engine>, label: &str, class: &str) -> StepGuard {
        diag::enter(&Step {
            kind: StepKind::ProbeStage,
            engine,
            label,
            class,
        })
    }

    /// The adapter line of the probe file.
    fn adapter_line(dev: &Device) -> String {
        let info = dev.ctx.adapter.get_info();
        format!(
            "adapter {} ({:?}; driver {}, {}) · pipeline cache {}",
            info.name,
            info.backend,
            info.driver,
            info.driver_info,
            if dev
                .ctx
                .device
                .features()
                .contains(wgpu::Features::PIPELINE_CACHE)
            {
                "offered"
            } else {
                "not offered"
            },
        )
    }

    /// Runs every group in order.
    pub(super) fn run_all(rep: &mut Reporter) -> Result<(), Halt> {
        let classes = classes();
        let mut dev = Device::acquire().map_err(Halt::Lost)?;
        rep.line(&adapter_line(&dev));
        for (offset, class) in classes.iter().enumerate() {
            shader_group(rep, &mut dev, PT_FIRST + offset, class)?;
        }
        path_picture_group(rep, &mut dev, &classes)?;
        drop(dev);
        ray_groups(rep)?;
        cache_group(rep, classes.first().and_then(|class| class.value))
    }

    /// Builds every kernel of one pass class, each fresh with no cache.
    fn shader_group(
        rep: &mut Reporter,
        dev: &mut Device,
        idx: usize,
        class: &Class,
    ) -> Result<(), Halt> {
        rep.begin(idx);
        let Some(value) = class.value else {
            rep.end(idx, false, "No pass class for this kind of stone");
            return Ok(());
        };
        let constants = material_class::pipeline_constants(value);
        let mut done = 0;
        let mut rejected = Vec::new();
        for kernel in kernels_for(value) {
            if Reporter::cancelled() {
                return Err(Halt::Cancelled);
            }
            let entry = kernel.entry_point();
            let stage = format!("path-traced/{}/{entry}", class.name);
            let guard = step(Some(Engine::PathTraced), entry, class.name);
            let started = Instant::now();
            let built = build(dev, entry, kernel.source(), entry, &constants, None);
            drop(guard);
            match built {
                Ok(()) => {
                    done += 1;
                    rep.count(idx, done);
                    rep.stage_ok(&stage, started);
                }
                Err(why) => {
                    rep.stage_rejected(&stage, &why);
                    rejected.push(format!("{entry} ({}): {}", class.name, first_line(&why)));
                    // A rejected pipeline loses the whole device: take a fresh one.
                    *dev = Device::acquire().map_err(Halt::Lost)?;
                }
            }
        }
        rep.end(idx, rejected.is_empty(), &rejected_note(&rejected));
        Ok(())
    }

    /// Builds `entry` of `source` with the pipeline-overridable `constants`, through `cache`
    /// when one is given, and reports the device's failure when the driver rejected it.
    fn build(
        dev: &Device,
        label: &str,
        source: &str,
        entry: &str,
        constants: &[(&str, f64)],
        cache: Option<&wgpu::PipelineCache>,
    ) -> Result<(), String> {
        let module = dev
            .ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
        let pipeline = dev
            .ctx
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants,
                    zero_initialize_workgroup_memory: true,
                },
                cache,
            });
        drop(pipeline);
        drop(module);
        dev.failure().map_or(Ok(()), Err)
    }

    /// A 16 by 12 scene of a plain round brilliant of `material`.
    fn tiny_scene(material: &str, ray: bool) -> Result<OwnedScene, String> {
        let spec = SceneSpec {
            width: TINY_WIDTH,
            height: TINY_HEIGHT,
            planes: planes_to_data(&StandardGemCuts::standard_round_brilliant()),
            finishes: FinishSpec::AllPolished,
            material: MaterialSpec::catalogue(material),
            camera: CameraSpec {
                yaw: 0.6,
                pitch: 0.45,
                distance: 3.0,
            },
            lighting: LightingSpec {
                preset_index: LightingPreset::Daylight.index(),
                exposure: 1.0,
                light_yaw: 0.7,
                light_pitch: 0.8,
                backdrop_index: crate::design_props::backdrop_index(
                    crate::settings::LightingSettings::default().backdrop,
                ),
                head_shadow_deg: 16.0,
                surface_glare: 1.0,
                hdr_yaw_deg: 0.0,
                hdr_follows_camera: false,
            },
            max_bounces: 12,
            hdr_id: None,
        };
        let mut scene = OwnedScene::build(&spec, None).map_err(|error| error.to_string())?;
        if ray {
            scene
                .set_engine(EngineSpec {
                    engine: RenderEngine::RayTraced,
                    ray: RayOptions::default(),
                })
                .map_err(|decline| decline.message().to_owned())?;
        }
        Ok(scene)
    }

    /// True when every value is finite and not negative, the rule a render applies to a pass.
    fn sums_are_valid(sums: &[Vec3]) -> bool {
        sums.iter()
            .all(|sum| sum.is_finite() && sum.min_element() >= 0.0)
    }

    /// One sample of the tiny picture of `class` on the event wavefront.
    fn path_picture(dev: &Device, class: &Class) -> Result<(), String> {
        let value = class
            .value
            .ok_or_else(|| "No pass class for this kind of stone".to_owned())?;
        let scene = tiny_scene(class.material, false)?;
        let mut renderer = EventWavefrontRenderer::new(dev.ctx.clone());
        renderer.set_surface_scene(&scene.surface_scene());
        let frame = scene.frame_scene();
        let mut sums = vec![Vec3::ZERO; (TINY_WIDTH * TINY_HEIGHT) as usize];
        let traced = pollster::block_on(renderer.accumulate_via_class_with_tools(
            &frame,
            scene.tools(),
            value,
            0,
            1,
            &mut sums,
        ));
        if let Some(why) = dev.failure() {
            return Err(why);
        }
        traced.map_err(|error| error.to_string())?;
        if sums_are_valid(&sums) {
            Ok(())
        } else {
            Err("the picture holds non-finite or negative sums".to_owned())
        }
    }

    /// The Path-Traced tiny pictures, one per pass class, on the probe's device.
    fn path_picture_group(
        rep: &mut Reporter,
        dev: &mut Device,
        classes: &[Class],
    ) -> Result<(), Halt> {
        rep.begin(PT_PICTURE);
        let mut done = 0;
        let mut rejected = Vec::new();
        for class in classes {
            if Reporter::cancelled() {
                return Err(Halt::Cancelled);
            }
            let stage = format!("path-traced/{}/picture", class.name);
            let guard = step(Some(Engine::PathTraced), "picture", class.name);
            let started = Instant::now();
            let traced = path_picture(dev, class);
            drop(guard);
            match traced {
                Ok(()) => {
                    done += 1;
                    rep.count(PT_PICTURE, done);
                    rep.stage_ok(&stage, started);
                }
                Err(why) => {
                    rep.stage_rejected(&stage, &why);
                    rejected.push(format!("{}: {}", class.name, first_line(&why)));
                    *dev = Device::acquire().map_err(Halt::Lost)?;
                }
            }
        }
        rep.end(PT_PICTURE, rejected.is_empty(), &rejected_note(&rejected));
        Ok(())
    }

    /// The three Ray-Traced groups: the device, the `whitted_main` build, one tiny picture.
    fn ray_groups(rep: &mut Reporter) -> Result<(), Halt> {
        if Reporter::cancelled() {
            return Err(Halt::Cancelled);
        }
        rep.begin(RT_START);
        let guard = step(Some(Engine::RayTraced), "start", "");
        let started = Instant::now();
        let acquired = Device::acquire();
        drop(guard);
        let dev = match acquired {
            Ok(dev) => {
                rep.stage_ok("ray-traced/start", started);
                rep.end(RT_START, true, "");
                dev
            }
            Err(why) => {
                rep.stage_rejected("ray-traced/start", &why);
                rep.end(RT_START, false, &first_line(&why));
                return Ok(());
            }
        };
        if Reporter::cancelled() {
            return Err(Halt::Cancelled);
        }
        let Some(mut renderer) = ray_build(rep, &dev) else {
            return Ok(());
        };
        if Reporter::cancelled() {
            return Err(Halt::Cancelled);
        }
        rep.begin(RT_PICTURE);
        let guard = step(Some(Engine::RayTraced), "picture", "");
        let started = Instant::now();
        let traced = ray_picture(&mut renderer, &dev);
        drop(guard);
        match traced {
            Ok(()) => {
                rep.stage_ok("ray-traced/picture", started);
                rep.end(RT_PICTURE, true, "");
            }
            Err(why) => {
                rep.stage_rejected("ray-traced/picture", &why);
                rep.end(RT_PICTURE, false, &first_line(&why));
            }
        }
        Ok(())
    }

    /// Builds the Ray-Traced renderer on `dev`; `None` when the build was rejected.
    fn ray_build(rep: &mut Reporter, dev: &Device) -> Option<WhittedGpuRenderer> {
        rep.begin(RT_BUILD);
        let guard = step(Some(Engine::RayTraced), "whitted_main", "");
        let started = Instant::now();
        let built = pollster::block_on(WhittedGpuRenderer::from_context_async(dev.ctx.clone()))
            .map_err(|error| error.to_string())
            .and_then(|renderer| dev.failure().map_or(Ok(renderer), Err));
        drop(guard);
        match built {
            Ok(renderer) => {
                rep.stage_ok("ray-traced/build", started);
                rep.end(RT_BUILD, true, "");
                Some(renderer)
            }
            Err(why) => {
                rep.stage_rejected("ray-traced/build", &why);
                rep.end(RT_BUILD, false, &first_line(&why));
                None
            }
        }
    }

    /// Uploads the tiny scene and traces one sample of it.
    fn ray_picture(renderer: &mut WhittedGpuRenderer, dev: &Device) -> Result<(), String> {
        let scene = tiny_scene("Diamond", true)?;
        let prep = scene
            .whitted_prep()
            .map_err(|decline| decline.message().to_owned())?;
        let uploaded = renderer
            .prepare(&scene.frame_scene(), scene.tools(), prep)
            .map_err(|error| error.to_string())?;
        let range = WhittedGpuRange {
            first_pixel: 0,
            pixels: TINY_WIDTH * TINY_HEIGHT,
            sample_offset: 0,
            spp: 1,
        };
        let limits = WhittedGpuTurnLimits {
            max_chunk_pixels: CHUNK_PIXELS,
            max_chunks: 2,
        };
        let traced = renderer.trace_turn(&uploaded, range, limits, &|| false);
        if let Some(why) = dev.failure() {
            return Err(why);
        }
        match traced {
            Ok(Some(values)) if sums_are_valid(&values) => Ok(()),
            Ok(Some(_)) => Err("the picture holds non-finite or negative sums".to_owned()),
            Ok(None) => Err("the trace was cancelled".to_owned()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// The shader-cache group (B-T1): create an empty cache and build with it, read its blob,
    /// create a second cache from the blob and build again. In memory only, no folder.
    fn cache_group(rep: &mut Reporter, class: Option<u32>) -> Result<(), Halt> {
        if Reporter::cancelled() {
            return Err(Halt::Cancelled);
        }
        rep.begin(CACHE);
        let dev = match Device::acquire() {
            Ok(dev) => dev,
            Err(why) => {
                rep.end(CACHE, false, &first_line(&why));
                return Ok(());
            }
        };
        let info = dev.ctx.adapter.get_info();
        let Some(cache) = compute::create_pipeline_cache(&dev.ctx.device, &info) else {
            rep.line("probe cache not offered by this graphics card");
            rep.end(CACHE, true, "Not offered by this graphics card");
            return Ok(());
        };
        let Some(class) = class else {
            rep.end(CACHE, false, "No pass class for the small kernel");
            return Ok(());
        };
        let kernel = EwKernel::Finalize;
        let constants = material_class::pipeline_constants(class);
        match cache_create(rep, &dev, &cache, kernel, &constants) {
            Ok(()) => rep.count(CACHE, 1),
            Err(why) => {
                rep.end(CACHE, false, &first_line(&why));
                return Ok(());
            }
        }
        if Reporter::cancelled() {
            return Err(Halt::Cancelled);
        }
        let blob = cache_read(rep, &cache);
        rep.count(CACHE, 2);
        if Reporter::cancelled() {
            return Err(Halt::Cancelled);
        }
        match cache_reload(rep, &dev, blob.as_deref(), kernel, &constants) {
            Ok(()) => rep.end(CACHE, true, ""),
            Err(why) => rep.end(CACHE, false, &first_line(&why)),
        }
        Ok(())
    }

    /// Stage one: the small kernel, built through the empty cache.
    fn cache_create(
        rep: &mut Reporter,
        dev: &Device,
        cache: &wgpu::PipelineCache,
        kernel: EwKernel,
        constants: &[(&str, f64)],
    ) -> Result<(), String> {
        let entry = kernel.entry_point();
        let guard = step(None, "cache-create", "");
        let started = Instant::now();
        let built = build(dev, entry, kernel.source(), entry, constants, Some(cache));
        drop(guard);
        match &built {
            Ok(()) => rep.stage_ok("shader-cache/create", started),
            Err(why) => rep.stage_rejected("shader-cache/create", why),
        }
        built
    }

    /// Stage two: the blob the driver handed back, and its size.
    fn cache_read(rep: &mut Reporter, cache: &wgpu::PipelineCache) -> Option<Vec<u8>> {
        let guard = step(None, "cache-read", "");
        let blob = cache.get_data();
        drop(guard);
        match &blob {
            Some(bytes) => rep.line(&format!(
                "probe shader-cache/read blob {} bytes",
                bytes.len()
            )),
            None => rep.line("probe shader-cache/read the driver returned no blob"),
        }
        blob
    }

    /// Stage three: a second cache from the blob, with `fallback`, and the kernel built again.
    fn cache_reload(
        rep: &mut Reporter,
        dev: &Device,
        blob: Option<&[u8]>,
        kernel: EwKernel,
        constants: &[(&str, f64)],
    ) -> Result<(), String> {
        let entry = kernel.entry_point();
        let guard = step(None, "cache-reload", "");
        let started = Instant::now();
        // SAFETY: the blob comes from `get_data` of a cache on this same device a moment ago;
        // `fallback: true` makes wgpu start an empty cache when the driver refuses it, and the
        // shader and pipeline validation stay on.
        let second = unsafe {
            dev.ctx
                .device
                .create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
                    label: Some("pleochroma probe cache from blob"),
                    data: blob,
                    fallback: true,
                })
        };
        let built = build(dev, entry, kernel.source(), entry, constants, Some(&second));
        drop(guard);
        match &built {
            Ok(()) => rep.stage_ok("shader-cache/reload", started),
            Err(why) => rep.stage_rejected("shader-cache/reload", why),
        }
        built
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_utc_fields_follow_the_calendar() {
        assert_eq!(utc_fields(0), [1970, 1, 1, 0, 0, 0]);
        assert_eq!(utc_fields(1_700_000_000), [2023, 11, 14, 22, 13, 20]);
        // The day after a leap day.
        assert_eq!(utc_fields(1_709_251_200), [2024, 3, 1, 0, 0, 0]);
    }

    #[test]
    fn a_result_line_gives_ok_and_total_and_the_last_one_wins() {
        let text = "+0.1 s probe stage ok 3 ms\n+9.0 s probe result 20 of 25 ok\n\
                    +9.5 s probe result 24 of 25 ok\n";
        assert_eq!(parse_result(text), Some((24, 25)));
        assert_eq!(parse_result("+1.0 s probe cancelled after 3 of 25"), None);
        assert_eq!(parse_result("+1.0 s probe result many of 25 ok"), None);
        assert_eq!(parse_result(""), None);
    }

    #[test]
    fn the_first_line_is_cut_to_one_short_line() {
        assert_eq!(first_line("a\nb"), "a");
        assert_eq!(first_line(&"x".repeat(500)).len(), 300);
        assert_eq!(first_line(""), "");
    }

    #[test]
    fn a_probe_file_name_carries_the_stamp_and_never_repeats() {
        let dir =
            std::env::temp_dir().join(format!("pleochroma-probe-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (first, first_name) = create_probe_file(&dir).expect("first probe file");
        let (second, second_name) = create_probe_file(&dir).expect("second probe file");
        assert!(first_name.starts_with("probe-") && first_name.ends_with(".log"));
        assert_ne!(first_name, second_name);
        drop((first, second));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_last_result_comes_from_a_probe_file_that_holds_one() {
        let dir =
            std::env::temp_dir().join(format!("pleochroma-probe-last-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("logs folder");
        let store = LogStore::new(dir.clone());
        assert_eq!(last_result(&store), None);
        fs::write(
            dir.join("probe-20261117-1540.log"),
            "+0.1 s probe started\n",
        )
        .expect("a crashed run");
        assert_eq!(last_result(&store), None);
        fs::write(
            dir.join("probe-20261117-1530.log"),
            "+0.1 s start\n+8.0 s probe result 24 of 25 ok\n",
        )
        .expect("a finished run");
        assert_eq!(last_result(&store), Some((24, 25)));
        let _ = fs::remove_dir_all(&dir);
    }

    /// One test owns the shared state, so parallel tests never see each other's groups.
    #[test]
    fn the_groups_settle_and_the_snapshot_adds_them_up() {
        reset_state();
        let fresh = snapshot();
        assert_eq!(fresh.groups.len(), 8);
        assert!(fresh.groups.iter().all(|g| g.state == GroupState::Waiting));
        assert_eq!(fresh.groups[0].title, "Path-Traced · isotropic shaders");
        assert_eq!(fresh.groups[7].title, "Shader cache");
        assert_eq!(fresh.ok, 0);
        assert_eq!(
            fresh.total,
            fresh.groups.iter().map(|g| g.total).sum::<u32>()
        );

        with_state(|state| {
            state.groups[0].state = GroupState::Ok;
            state.groups[0].done = state.groups[0].total;
            state.groups[1].state = GroupState::Running;
        });
        settle("Stopped", false);
        let after = snapshot();
        assert_eq!(after.groups[0].state, GroupState::Ok);
        assert_eq!(after.groups[1].state, GroupState::Failed);
        assert_eq!(after.groups[1].note, "Stopped");
        assert_eq!(after.groups[2].state, GroupState::Waiting);
        assert_eq!(after.ok, after.groups[0].total);

        settle(NOT_REACHED, true);
        let last = snapshot();
        assert_eq!(last.groups[2].state, GroupState::Failed);
        assert_eq!(last.groups[2].note, NOT_REACHED);
        // A note that was set stays.
        assert_eq!(last.groups[1].note, "Stopped");

        let mut rep = Reporter::detached();
        rep.line("a line without a file does nothing");
    }

    #[test]
    fn the_finished_hook_can_be_set_and_removed() {
        set_finished_hook(Some(Box::new(|_ok, _total| {})));
        assert!(
            HOOK.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_some()
        );
        set_finished_hook(None);
        assert!(
            HOOK.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_none()
        );
    }

    #[cfg(feature = "device-gpu")]
    #[test]
    fn rejections_read_as_the_first_one_and_a_count() {
        assert_eq!(rejected_note(&[]), "");
        assert_eq!(
            rejected_note(&["ew_tail (uniaxial): lost".to_owned()]),
            "Rejected: ew_tail (uniaxial): lost"
        );
        assert_eq!(
            rejected_note(&["a".to_owned(), "b".to_owned(), "c".to_owned()]),
            "Rejected: a (and 2 more)"
        );
    }
}
