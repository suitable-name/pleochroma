use std::sync::{
    Arc, Mutex, MutexGuard, PoisonError, RwLock,
    atomic::{AtomicBool, Ordering},
};

/// The tracing target of every tracer event.
pub const TRACER_TARGET: &str = "pleochroma::tracer";

/// The two renderers whose preparation the apps show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Engine {
    /// The event-wavefront Path-Traced renderer.
    PathTraced = 0,
    /// The four-buffer Ray-Traced renderer.
    RayTraced = 1,
}

impl Engine {
    /// The name a person reads.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PathTraced => "Path-Traced",
            Self::RayTraced => "Ray-Traced",
        }
    }

    /// Position in per-engine arrays such as `session_cpu_only`.
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// The step of a GPU preparation that is in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PrepareStep {
    /// Nothing is being prepared.
    Idle = 0,
    /// The adapter and device are requested.
    StartingGpu = 1,
    /// The kernels are compiled.
    BuildingShaders = 2,
    /// The scene is copied to the device.
    Uploading = 3,
    /// The first dispatch and its readback run.
    FirstTrace = 4,
}

/// A snapshot of the preparation in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrepareProgress {
    /// The renderer being prepared.
    pub engine: Engine,
    /// The step in flight, or `Idle`.
    pub step: PrepareStep,
    /// Units finished in this step (kernels built).
    pub done: u32,
    /// Units in this step.
    pub total: u32,
    /// `process_ms()` when this step began.
    pub start_ms: u64,
}

impl PrepareProgress {
    /// The snapshot of a process that prepares nothing.
    const IDLE: Self = Self {
        engine: Engine::PathTraced,
        step: PrepareStep::Idle,
        done: 0,
        total: 0,
        start_ms: 0,
    };
}

/// The step in flight behind a mutex; the rules of `set_prepare` and `clear_prepare`.
struct PrepareCell {
    /// The current snapshot.
    state: Mutex<PrepareProgress>,
}

impl PrepareCell {
    /// A cell that prepares nothing.
    const fn new() -> Self {
        Self {
            state: Mutex::new(PrepareProgress::IDLE),
        }
    }

    /// The current snapshot.
    fn get(&self) -> PrepareProgress {
        *lock(&self.state)
    }

    /// Stores a step; `start_ms` restarts at `now_ms` when the engine or the step differs.
    fn set(&self, engine: Engine, step: PrepareStep, done: u32, total: u32, now_ms: u64) {
        let mut state = lock(&self.state);
        let start_ms = if state.engine == engine && state.step == step {
            state.start_ms
        } else {
            now_ms
        };
        *state = PrepareProgress {
            engine,
            step,
            done,
            total,
            start_ms,
        };
    }

    /// Goes back to `Idle` when `engine` is the stored engine.
    fn clear(&self, engine: Engine) {
        let mut state = lock(&self.state);
        if state.engine == engine {
            *state = PrepareProgress {
                engine,
                ..PrepareProgress::IDLE
            };
        }
    }
}

/// The preparation of this process.
static PREPARE: PrepareCell = PrepareCell::new();
/// Whether [`OVERRIDE`] holds a value; read first so the common case is one relaxed load.
static OVERRIDE_SET: AtomicBool = AtomicBool::new(false);
/// The frozen counts of the preview override and the time they were set.
static OVERRIDE: Mutex<Option<(u32, u32, u64)>> = Mutex::new(None);

/// Locks a mutex whose holders cannot leave it half-written.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Milliseconds since the first call in this process (0 on wasm32).
#[must_use]
#[cfg_attr(
    target_arch = "wasm32",
    allow(
        clippy::missing_const_for_fn,
        reason = "the native arm reads a clock; one signature for both targets"
    )
)]
pub fn process_ms() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let start = *START.get_or_init(std::time::Instant::now);
        u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

/// The step in flight, or `step == Idle`; returns the override while one is set.
#[must_use]
pub fn prepare_progress() -> PrepareProgress {
    let frozen = if OVERRIDE_SET.load(Ordering::Relaxed) {
        *lock(&OVERRIDE)
    } else {
        None
    };
    frozen.map_or_else(
        || PREPARE.get(),
        |(done, total, start_ms)| PrepareProgress {
            engine: Engine::PathTraced,
            step: PrepareStep::BuildingShaders,
            done,
            total,
            start_ms,
        },
    )
}

/// Stores the step in flight; `start_ms` restarts when `engine` or `step` differs from the
/// stored one.
pub fn set_prepare(engine: Engine, step: PrepareStep, done: u32, total: u32) {
    PREPARE.set(engine, step, done, total, process_ms());
}

/// Goes back to `Idle`, only if `engine` is the stored engine.
pub fn clear_prepare(engine: Engine) {
    PREPARE.clear(engine);
}

/// Preview only (`--hold-preparing`).
///
/// `Some((done, total))` freezes [`prepare_progress`] at Path-Traced / `BuildingShaders` with
/// these counts; `None` lifts it.
pub fn set_prepare_override(counts: Option<(u32, u32)>) {
    let mut held = lock(&OVERRIDE);
    *held = counts.map(|(done, total)| (done, total, process_ms()));
    OVERRIDE_SET.store(held.is_some(), Ordering::Relaxed);
}

/// The counts set by [`set_prepare_override`], if any.
#[must_use]
pub fn prepare_override() -> Option<(u32, u32)> {
    if !OVERRIDE_SET.load(Ordering::Relaxed) {
        return None;
    }
    (*lock(&OVERRIDE)).map(|(done, total, _)| (done, total))
}

/// The identity of one open breadcrumb step, chosen by the sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StepId(pub u64);

/// What a breadcrumb step is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepKind {
    /// `request_adapter`.
    AdapterRequest,
    /// `request_device`.
    DeviceRequest,
    /// A shader cache blob is read.
    CacheLoad,
    /// A driver pipeline cache object is created.
    CacheCreate,
    /// A shader cache blob is written.
    CacheSave,
    /// A pipeline is compiled.
    Compile,
    /// The first dispatch of a session.
    FirstDispatch,
    /// The first readback of a session.
    FirstReadback,
    /// One stage of the GPU probe.
    ProbeStage,
    /// The forced crash of the debug build.
    DebugCrash,
}

impl StepKind {
    /// Whether this step touches the shader cache (load, creation or save).
    #[must_use]
    pub const fn is_cache(self) -> bool {
        matches!(self, Self::CacheLoad | Self::CacheCreate | Self::CacheSave)
    }

    /// The words a person reads: "building the Ray-Traced shader", "loading saved shaders", ...
    #[must_use]
    pub fn phrase(self, engine: Option<Engine>) -> String {
        match (self, engine) {
            (Self::AdapterRequest, _) => "looking for the graphics card".to_owned(),
            (Self::DeviceRequest, _) => "starting the graphics card".to_owned(),
            (Self::CacheLoad, _) => "loading saved shaders".to_owned(),
            (Self::CacheCreate, _) => "preparing the shader cache".to_owned(),
            (Self::CacheSave, _) => "saving shaders".to_owned(),
            (Self::Compile, Some(Engine::RayTraced)) => "building the Ray-Traced shader".to_owned(),
            (Self::Compile, Some(Engine::PathTraced)) => {
                "building the Path-Traced shaders".to_owned()
            }
            (Self::Compile, None) => "building shaders".to_owned(),
            (Self::FirstDispatch, Some(engine)) => {
                format!("starting the first {} trace", engine.name())
            }
            (Self::FirstDispatch, None) => "starting the first trace".to_owned(),
            (Self::FirstReadback, Some(engine)) => {
                format!("reading back the first {} result", engine.name())
            }
            (Self::FirstReadback, None) => "reading back the first result".to_owned(),
            (Self::ProbeStage, _) => "running the graphics card probe".to_owned(),
            (Self::DebugCrash, _) => "a forced test crash".to_owned(),
        }
    }
}

/// One step as a sink records it.
pub struct Step<'a> {
    /// What the step does.
    pub kind: StepKind,
    /// The renderer it belongs to, if any.
    pub engine: Option<Engine>,
    /// Kernel name, entry point or probe stage; "" when none. Never a path or a design name.
    pub label: &'a str,
    /// Pass class ("isotropic", "uniaxial", "biaxial") or "".
    pub class: &'a str,
}

/// What an adapter reports about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdapterFacts {
    /// The adapter name.
    pub name: String,
    /// The graphics API, such as `Vulkan`.
    pub backend: String,
    /// The driver name.
    pub driver: String,
    /// The driver's own version text.
    pub driver_info: String,
    /// The device type, such as `IntegratedGpu`.
    pub device_type: String,
    /// The limits that matter on a mobile device, as `name=value` pairs.
    pub limits: String,
    /// Whether the adapter offers the driver pipeline cache.
    pub pipeline_cache_offered: bool,
}

/// Receives the open steps of the GPU path.
pub trait BreadcrumbSink: Send + Sync {
    /// A step begins; the returned id comes back in `leave`.
    fn enter(&self, step: &Step<'_>) -> StepId;
    /// The step with this id ended.
    fn leave(&self, id: StepId);
    /// An adapter became known.
    fn adapter(&self, facts: &AdapterFacts);
}

/// Whether [`SINK`] holds a sink; read first so the common case is one relaxed load.
static SINK_SET: AtomicBool = AtomicBool::new(false);
/// The installed sink.
static SINK: RwLock<Option<Arc<dyn BreadcrumbSink>>> = RwLock::new(None);

/// Installs the breadcrumb sink; `None` removes it.
pub fn set_breadcrumb_sink(sink: Option<Arc<dyn BreadcrumbSink>>) {
    let mut slot = SINK.write().unwrap_or_else(PoisonError::into_inner);
    SINK_SET.store(sink.is_some(), Ordering::Relaxed);
    let old = std::mem::replace(&mut *slot, sink);
    drop(slot);
    drop(old);
}

/// The installed sink, or `None` after one relaxed load when there is none.
fn current_sink() -> Option<Arc<dyn BreadcrumbSink>> {
    if !SINK_SET.load(Ordering::Relaxed) {
        return None;
    }
    SINK.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// A step that is open until this guard drops.
pub struct StepGuard {
    /// The sink that saw `enter`, with the id it returned; `None` when no sink was installed.
    held: Option<(Arc<dyn BreadcrumbSink>, StepId)>,
}

impl StepGuard {
    /// The id the sink returned, or `None` when no sink was installed.
    #[must_use]
    pub fn id(&self) -> Option<StepId> {
        self.held.as_ref().map(|(_, id)| *id)
    }
}

impl Drop for StepGuard {
    fn drop(&mut self) {
        if let Some((sink, id)) = self.held.take() {
            sink.leave(id);
        }
    }
}

/// Opens a step; its `Drop` calls `leave`. Without a sink the guard holds nothing.
#[must_use]
pub fn enter(step: &Step<'_>) -> StepGuard {
    StepGuard {
        held: current_sink().map(|sink| {
            let id = sink.enter(step);
            (sink, id)
        }),
    }
}

/// One `(kind, label, class)` already asked for.
struct Seen {
    /// The step kind.
    kind: StepKind,
    /// The kernel or stage name.
    label: Box<str>,
    /// The pass class.
    class: Box<str>,
}

/// Every key `first_time` has answered.
static SEEN: Mutex<Vec<Seen>> = Mutex::new(Vec::new());

/// True the first time this `(kind, label, class)` is asked in this process.
///
/// A caller decides "first compile of each kernel and class" or "first dispatch of a session" with
/// it. The list is short and a repeat question allocates nothing.
#[must_use]
pub fn first_time(kind: StepKind, label: &str, class: &str) -> bool {
    let mut seen = lock(&SEEN);
    if seen
        .iter()
        .any(|key| key.kind == kind && &*key.label == label && &*key.class == class)
    {
        return false;
    }
    seen.push(Seen {
        kind,
        label: label.into(),
        class: class.into(),
    });
    true
}

/// Called once an adapter is known; forwards to the sink and writes one tracer event.
pub fn record_adapter(facts: &AdapterFacts) {
    if let Some(sink) = current_sink() {
        sink.adapter(facts);
    }
    tracing::info!(
        target: TRACER_TARGET,
        adapter = %facts.name,
        backend = %facts.backend,
        driver = %facts.driver,
        driver_info = %facts.driver_info,
        device_type = %facts.device_type,
        pipeline_cache_offered = facts.pipeline_cache_offered,
        limits = %facts.limits,
        "adapter found"
    );
}

/// What an acquired adapter reports about itself.
#[cfg(feature = "gpu")]
pub(crate) fn adapter_facts(adapter: &wgpu::Adapter) -> AdapterFacts {
    let info = adapter.get_info();
    let limits = adapter.limits();
    AdapterFacts {
        name: info.name,
        backend: format!("{:?}", info.backend),
        driver: info.driver,
        driver_info: info.driver_info,
        device_type: format!("{:?}", info.device_type),
        limits: format!(
            "max_storage_buffers_per_shader_stage={} \
             max_buffers_and_acceleration_structures_per_shader_stage={} \
             max_storage_buffer_binding_size={} max_buffer_size={} \
             max_compute_workgroup_storage_size={} max_compute_invocations_per_workgroup={}",
            limits.max_storage_buffers_per_shader_stage,
            limits.max_buffers_and_acceleration_structures_per_shader_stage,
            limits.max_storage_buffer_binding_size,
            limits.max_buffer_size,
            limits.max_compute_workgroup_storage_size,
            limits.max_compute_invocations_per_workgroup,
        ),
        pipeline_cache_offered: adapter.features().contains(wgpu::Features::PIPELINE_CACHE),
    }
}

/// The device's failure text, for the probe.
#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
#[must_use]
pub fn device_failure(ctx: &crate::renderer::gpu::context::GpuContext) -> Option<String> {
    ctx.failure()
}

/// True when the GPU lifecycle gate stayed busy for the whole `wait`.
#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
#[must_use]
pub fn gpu_start_blocked(wait: std::time::Duration) -> bool {
    crate::renderer::gpu::context::lifecycle_busy_for(wait)
}

#[cfg(test)]
mod tests {
    use super::{
        AdapterFacts, BreadcrumbSink, Engine, PrepareCell, PrepareStep, Step, StepId, StepKind,
        enter, first_time, prepare_override, prepare_progress, record_adapter, set_breadcrumb_sink,
        set_prepare_override,
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    /// Prefix of every label and adapter name these tests use; other tests' steps are ignored.
    const MINE: &str = "diag-test";

    /// Serialises the tests that touch the process-wide sink or override.
    static GLOBALS: Mutex<()> = Mutex::new(());

    /// Counts the calls that carry this module's own label.
    #[derive(Default)]
    struct CountingSink {
        entered: AtomicUsize,
        left: AtomicUsize,
        adapters: AtomicUsize,
    }

    impl BreadcrumbSink for CountingSink {
        fn enter(&self, step: &Step<'_>) -> StepId {
            if step.label.starts_with(MINE) {
                self.entered.fetch_add(1, Ordering::SeqCst);
                StepId(7)
            } else {
                StepId(0)
            }
        }

        fn leave(&self, id: StepId) {
            if id == StepId(7) {
                self.left.fetch_add(1, Ordering::SeqCst);
            }
        }

        fn adapter(&self, facts: &AdapterFacts) {
            if facts.name.starts_with(MINE) {
                self.adapters.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    fn mine(label: &str) -> Step<'_> {
        Step {
            kind: StepKind::Compile,
            engine: Some(Engine::PathTraced),
            label,
            class: "uniaxial",
        }
    }

    fn counts(sink: &CountingSink) -> (usize, usize, usize) {
        (
            sink.entered.load(Ordering::SeqCst),
            sink.left.load(Ordering::SeqCst),
            sink.adapters.load(Ordering::SeqCst),
        )
    }

    #[test]
    fn the_hooks_reach_a_sink_only_while_one_is_installed() {
        let _serial = GLOBALS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let sink = Arc::new(CountingSink::default());
        let facts = AdapterFacts {
            name: format!("{MINE}-adapter"),
            ..AdapterFacts::default()
        };

        set_breadcrumb_sink(None);
        let guard = enter(&mine("diag-test-before"));
        assert!(guard.id().is_none(), "a guard without a sink holds nothing");
        drop(guard);
        record_adapter(&facts);
        assert_eq!(counts(&sink), (0, 0, 0));

        let dynamic: Arc<dyn BreadcrumbSink> = sink.clone();
        set_breadcrumb_sink(Some(dynamic));
        let guard = enter(&mine("diag-test-during"));
        assert_eq!(guard.id(), Some(StepId(7)));
        assert_eq!(counts(&sink), (1, 0, 0));
        drop(guard);
        record_adapter(&facts);
        assert_eq!(counts(&sink), (1, 1, 1));

        set_breadcrumb_sink(None);
        drop(enter(&mine("diag-test-after")));
        record_adapter(&facts);
        assert_eq!(counts(&sink), (1, 1, 1));
    }

    #[test]
    fn a_guard_leaves_the_sink_that_saw_it_enter() {
        let _serial = GLOBALS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let first = Arc::new(CountingSink::default());
        let second = Arc::new(CountingSink::default());
        let dynamic: Arc<dyn BreadcrumbSink> = first.clone();
        set_breadcrumb_sink(Some(dynamic));
        let guard = enter(&mine("diag-test-swap"));
        let dynamic: Arc<dyn BreadcrumbSink> = second.clone();
        set_breadcrumb_sink(Some(dynamic));
        drop(guard);
        set_breadcrumb_sink(None);
        assert_eq!(counts(&first), (1, 1, 0));
        assert_eq!(counts(&second), (0, 0, 0));
    }

    #[test]
    fn first_time_is_true_once_per_kind_label_and_class() {
        assert!(first_time(
            StepKind::Compile,
            "diag-test-kernel",
            "uniaxial"
        ));
        assert!(!first_time(
            StepKind::Compile,
            "diag-test-kernel",
            "uniaxial"
        ));
        assert!(first_time(StepKind::Compile, "diag-test-kernel", "biaxial"));
        assert!(first_time(
            StepKind::FirstDispatch,
            "diag-test-kernel",
            "uniaxial"
        ));
        assert!(first_time(StepKind::Compile, "diag-test-other", "uniaxial"));
    }

    #[test]
    fn a_step_keeps_its_start_until_the_engine_or_the_step_changes() {
        let cell = PrepareCell::new();
        assert_eq!(cell.get().step, PrepareStep::Idle);
        cell.set(Engine::PathTraced, PrepareStep::BuildingShaders, 0, 18, 100);
        cell.set(Engine::PathTraced, PrepareStep::BuildingShaders, 3, 18, 900);
        let progress = cell.get();
        assert_eq!(
            (progress.done, progress.total, progress.start_ms),
            (3, 18, 100)
        );
        cell.set(Engine::PathTraced, PrepareStep::Uploading, 0, 1, 1000);
        assert_eq!(cell.get().start_ms, 1000);
        cell.set(Engine::RayTraced, PrepareStep::Uploading, 0, 1, 2000);
        assert_eq!(cell.get().start_ms, 2000);
    }

    #[test]
    fn clearing_goes_idle_only_for_the_stored_engine() {
        let cell = PrepareCell::new();
        cell.set(Engine::RayTraced, PrepareStep::BuildingShaders, 0, 1, 5);
        cell.clear(Engine::PathTraced);
        assert_eq!(cell.get().step, PrepareStep::BuildingShaders);
        cell.clear(Engine::RayTraced);
        assert_eq!(cell.get().step, PrepareStep::Idle);
        assert_eq!((cell.get().done, cell.get().total), (0, 0));
    }

    #[test]
    fn the_override_freezes_the_progress_until_it_is_lifted() {
        let _serial = GLOBALS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        set_prepare_override(Some((3, 18)));
        let progress = prepare_progress();
        assert_eq!(progress.engine, Engine::PathTraced);
        assert_eq!(progress.step, PrepareStep::BuildingShaders);
        assert_eq!((progress.done, progress.total), (3, 18));
        assert_eq!(prepare_override(), Some((3, 18)));
        set_prepare_override(None);
        assert_eq!(prepare_override(), None);
    }

    #[test]
    fn the_phrases_name_the_step_in_plain_words() {
        assert_eq!(
            StepKind::Compile.phrase(Some(Engine::RayTraced)),
            "building the Ray-Traced shader"
        );
        assert_eq!(StepKind::CacheLoad.phrase(None), "loading saved shaders");
        assert!(StepKind::CacheLoad.is_cache());
        assert!(StepKind::CacheCreate.is_cache());
        assert!(StepKind::CacheSave.is_cache());
        assert!(!StepKind::Compile.is_cache());
        assert_eq!(Engine::RayTraced.index(), 1);
    }
}
