//! WebGPU admission gate (section 6.5): per-adapter qualification, compilation benchmarking,
//! and admission state machine for GPU rendering in the browser.
//!
//! State transitions (`RenderModel.gpu-state`, [`GpuState`]):
//! - Off (`0`): GPU rendering is off, or the browser has none.
//! - Unknown (`1`): Adapter detected, never tested. "Test GPU" button visible.
//! - Testing (`2`): Benchmarking compilation of the kernels of one material class + 16x16
//!   self-check render (or, for an adapter that passed earlier, only the compile).
//! - Ok (`3`): Adapter passed testing. Eligible scenes route to GPU + CPU once the
//!   kernels of their class were compiled in this page's worker (`admit`, below).
//! - Failed (`4`): Adapter failed compile benchmark, self-check, or was lost, or the last
//!   run was lost before it finished. Shows the reason + "Forget GPU Test Result" button
//!   when the worker can still be used.
//! - Starting (`5`): GPU rendering is on and the worker is starting.
//!
//! # The start-up run
//!
//! [`init`] runs the test while the page loads (the `startup` module): the GPU worker is
//! started, asked for its adapter, the isotropic kernels are compiled and a 16x16 test render
//! runs, with a progress overlay and a message at the end that says what the app renders
//! with. The pure decisions are in `pleochroma_web_core::gpu_startup` and tested natively; the
//! eligibility, records, adapter keys and deadlines are in
//! `pleochroma_web_core::gpu_admission`.
//!
//! # Material classes and kernel features
//!
//! Each material class (isotropic, uniaxial, biaxial) has its own base kernel list, admission
//! record and crash marker (`gpu_admission::class_adapter_key`; the key carries the compiled
//! variant, so a record of another variant is never read). The page names a class by its kind
//! (1, 2, 3); the GPU Worker compiles and traces the browser variant of it
//! (`gpu_admission::browser_pipeline_class`: no oriented inclusions, no zoning, which a browser
//! scene never reaches, and no extraordinary curve unless uniaxial), so the all-features
//! variant, whose `Scatter` kernel exceeds the compile deadline, is never built. The isotropic
//! class is compiled at start-up. Another class is compiled and
//! tested on first use: when a stone of an admitted class (`gpu_admission::ADMITTED_CLASSES`)
//! shows its first image, the [`classes`] module starts the same compile-and-self-check run
//! for that class, and the Worker is told the class is usable (`set_gpu_class_compiled`) only
//! after it passed. A class that fails alone (a kernel does not compile, the self-check
//! disagrees with the CPU) is switched off by itself; the other classes keep the GPU.
//!
//! The base list leaves out the kernels only some stones reach, the
//! `gpu_admission::KernelFeature`s: scattering (`Scatter`), scattering under an HDR map
//! (`NeeHg`) and frosted facets (`ArmFrosted`). They are the slowest to compile, so a feature
//! is compiled the first time a scene needs it (the pool refuses the scene with the feature's
//! reason, `gpu_admission::feature_for_reason`; [`classes::ensure_feature`] compiles it) under
//! its own crash marker and record, with no self-check render. A feature whose compile fails
//! sends only the stones that need it to the CPU.
//!
//! # Compile units, steps and crash markers
//!
//! A compile unit is the base set of a class (named by the class value) or one feature of a
//! class (`gpu_admission::feature_unit`). The page's records, keys, the `GpuCompile`
//! request and its answers, the compiled set and the pool's mask all use the unit, so a
//! feature behaves like a small class of its own.
//!
//! The start-up run, the "Test GPU" run and the `admit` step all send `GpuCompile` for the
//! whole kernel list of one unit. Each writes the `Probing` marker of that unit BEFORE the
//! request and rewrites the record when every kernel answered; a marker still there on the
//! next page load means the browser lost the GPU process, and [`init`] turns it into `Failed`
//! (for that unit only), which no run repeats by itself. The adapter is not known before the
//! worker answers, so the start-up run first writes a marker under
//! `gpu_startup::STARTUP_KEY` and replaces it with the adapter's own once it is known. All
//! are watched by a page-side deadline that gives the worker up (terminates it) when it stops
//! answering.

mod admit;
mod classes;
mod ray;
pub use admit::admit_if_ready;
mod runs;
mod selfcheck;
mod startup;
mod status;

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use pleochroma_web_core::{
    design_meta::iso8601_utc_from_epoch_ms,
    gpu_admission::{self, AdapterFacts, ISOTROPIC_CLASS, SELF_CHECK_SCENE_ID, STAGE_DEADLINE_MS},
    gpu_startup::{self, AdapterPlan, GpuState, Stage},
    host::GpuEvent,
};
use runs::{begin_test, on_compiled, on_test_success, start_admit_compile};
use slint::{ComponentHandle, Timer, TimerMode};

pub use status::{format_backend_text, update_backend_text};

use crate::{
    RenderModel,
    app::Ctx,
    storage::admission::{self, AdmissionRecord, AdmissionState},
};

#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent phase flags of the gate (testing, admitting, starting, ...), each read and written on its own"
)]
struct GateState {
    /// The reduced device can run Ray-Traced but not the eight-buffer event kernels.
    ray_only_device: bool,
    /// The admission key of this adapter per compile unit (`classes::tracked_units`: a class's
    /// base set or one kernel feature of it), once the adapter is known.
    keys: BTreeMap<u32, String>,
    /// The record of each compile unit: what is stored, and the running compile's marker.
    records: BTreeMap<u32, AdmissionRecord>,
    /// The compile unit the running (or the last) test or `admit` compile is for; the isotropic
    /// class before any ran. Named `run_class` as the units began as classes.
    run_class: Option<u32>,
    /// "Test GPU" is running (compile, then the self-check).
    testing: bool,
    /// The `admit` compile after a reload is running.
    admitting: bool,
    /// The compile units (class base sets and kernel features) whose whole kernel list this
    /// page's GPU worker compiled (by a test or by `admit`).
    compiled_classes: BTreeSet<u32>,
    /// The compile units that failed on their own in this page session, with the reason. The
    /// worker refuses compiles after a failure, so only a reload tests them again.
    class_failed: BTreeMap<u32, String>,
    /// A compile failed in this page's worker, which refuses every later compile.
    compile_blocked: bool,
    compiled_kernels: BTreeSet<String>,
    failed_in_session: bool,
    /// The page-side deadline of the running step.
    deadline: Timer,
    /// The GPU worker was spawned and has not reported its adapter yet.
    starting: bool,
    /// The run may start the test by itself (page load, setting switched on).
    auto: bool,
    /// The end of the run says what the app renders with (a message).
    announce_pending: bool,
    /// The browser has no WebGPU.
    no_webgpu: bool,
    /// The run was blocked before the adapter was known (the start-up marker of the last
    /// run was still there); "Forget GPU Test Result" then starts it again.
    blocked_at_start: bool,
    /// The readable name of the adapter, once known.
    adapter_name: Option<String>,
    /// The running compile ends with the 16x16 test render.
    with_self_check: bool,
}

impl GateState {
    /// The compile unit of the running (or the last) compile.
    const fn run(&self) -> u32 {
        match self.run_class {
            Some(class) => class,
            None => ISOTROPIC_CLASS,
        }
    }

    /// The admission key of [`Self::run`]'s class, when the adapter is known.
    fn run_key(&self) -> Option<String> {
        self.keys.get(&self.run()).cloned()
    }

    /// The record of [`Self::run`]'s class.
    fn run_record(&self) -> Option<AdmissionRecord> {
        self.records.get(&self.run()).cloned()
    }

    /// A run (start, test or `admit` compile) is going.
    const fn busy(&self) -> bool {
        self.testing || self.admitting || self.starting
    }

    /// This page's worker compiled the kernels of compile unit `class` (a class's base set or a
    /// kernel feature).
    fn compiled(&self, class: u32) -> bool {
        self.compiled_classes.contains(&class)
    }
}

thread_local! {
    static GATE: RefCell<GateState> = RefCell::new(GateState::default());
}

/// Computes the unique admission storage key of compile `unit` (a material class's base set, or
/// one kernel feature of a class) for this adapter and browser version (section 6.5): FNV-1a 64
/// over the adapter's fields, the Chrome major version and the kernel set hash, with the unit's
/// suffix (`pleochroma_web_core::gpu_admission::unit_adapter_key`).
#[must_use]
pub fn compute_unit_key(facts: &AdapterFacts<'_>, unit: u32) -> String {
    gpu_admission::unit_adapter_key(
        facts,
        &get_chrome_major_version(),
        pleochroma::renderer::buffers::KERNEL_SET_HASH,
        unit,
    )
}

fn get_chrome_major_version() -> String {
    web_sys::window()
        .and_then(|window| window.navigator().user_agent().ok())
        .map_or_else(
            || "0".to_string(),
            |ua| gpu_admission::chrome_major_from_user_agent(&ua),
        )
}

/// The panel text once a kernel failed to compile in this page's worker: it refuses every later
/// compile, so no retest (and no "Forget GPU Test Result") can help before a reload.
const COMPILE_BLOCKED_TEXT: &str = "Path-Traced GPU kernels are qualified. A GPU kernel failed to \
     compile in this session; reload the page to test again.";

/// Shows the gate's state in the Render panel.
fn show(ctx: &Ctx, state: GpuState, text: &str, can_forget: bool) {
    if let Some(ui) = ctx.ui.upgrade() {
        let model = ui.global::<RenderModel>();
        model.set_gpu_state(state.code());
        model.set_gpu_status_text(text.into());
        model.set_gpu_can_forget(can_forget);
    }
}

/// Stores the record of the running class with `state` and `reason`, when the adapter is
/// known.
fn save_record(state: AdmissionState, reason: Option<String>) -> f64 {
    let class = GATE.with(|gate| gate.borrow().run());
    save_record_for(class, state, reason)
}

/// Stores the record of compile `unit` with `state` and `reason`, when the adapter is known.
/// A verdict about the adapter itself (a lost device) belongs to the isotropic base set, never
/// to whichever unit compiled last.
fn save_record_for(unit: u32, state: AdmissionState, reason: Option<String>) -> f64 {
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        let Some(key) = g.keys.get(&unit).cloned() else {
            return 0.0;
        };
        let Some(rec) = g.records.get_mut(&unit) else {
            return 0.0;
        };
        rec.state = state;
        rec.reason = reason;
        rec.when = iso8601_utc_from_epoch_ms(js_sys::Date::now());
        admission::save(&key, rec);
        rec.total_ms
    })
}

/// (Re)starts the page-side deadline of the running step.
fn restart_deadline(ctx: &Ctx) {
    let c = ctx.clone();
    GATE.with(|gate| {
        gate.borrow().deadline.start(
            TimerMode::SingleShot,
            Duration::from_millis(u64::from(STAGE_DEADLINE_MS)),
            move || on_deadline(&c),
        );
    });
}

fn stop_deadline() {
    GATE.with(|gate| gate.borrow().deadline.stop());
}

/// The worker stopped answering during a start, a test or an admission compile.
fn on_deadline(ctx: &Ctx) {
    let (starting, testing, admitting) = GATE.with(|gate| {
        let g = gate.borrow();
        (g.starting, g.testing, g.admitting)
    });
    if starting {
        startup::fail_start(ctx, "the GPU worker did not answer in time", true);
    } else if testing {
        fail_gpu(ctx, "GPU test timed out");
    } else if admitting {
        fail_gpu(ctx, "GPU compile timed out");
    }
}

/// A test or compile failed: the verdict is stored, the worker is given up for this page
/// session, and the GPU is off.
fn fail_gpu(ctx: &Ctx, reason: &str) {
    stop_deadline();
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.testing = false;
        g.admitting = false;
        g.starting = false;
        g.compiled_classes.clear();
        g.failed_in_session = true;
    });
    save_record(AdmissionState::Failed, Some(reason.to_string()));
    show(
        ctx,
        GpuState::Failed,
        &format!("GPU test failed: {reason}. Reload the page to try again."),
        false,
    );
    if let Some(pool) = crate::workers::existing() {
        pool.render().set_gpu_admitted(false);
        pool.render().terminate_gpu();
    }
    startup::end_run(ctx);
}

/// Fails the running test or compile.
///
/// The isotropic base set takes the whole GPU down with it ([`fail_gpu`]); another class, and
/// every kernel feature, fails alone ([`fail_class`]).
fn fail_run(ctx: &Ctx, reason: &str) {
    let class = GATE.with(|gate| gate.borrow().run());
    if class == ISOTROPIC_CLASS {
        fail_gpu(ctx, reason);
    } else {
        fail_class(ctx, class, reason);
    }
}

/// The test or compile of compile unit `class` (an anisotropic class, or a kernel feature of
/// any class) failed cleanly (a kernel does not compile, the self-check disagrees with the
/// CPU): the verdict is stored under the unit's own key, the stones that need the unit stay on
/// the CPU, and everything else keeps the GPU. The Worker refuses further compiles after a
/// failure, so the unit is tried again only after a reload.
fn fail_class(ctx: &Ctx, class: u32, reason: &str) {
    stop_deadline();
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.testing = false;
        g.admitting = false;
        g.compiled_classes.remove(&class);
        g.class_failed.insert(class, reason.to_owned());
    });
    save_record(AdmissionState::Failed, Some(reason.to_owned()));
    if let Some(pool) = crate::workers::existing() {
        pool.render().set_gpu_class_compiled(class, false);
    }
    // The adapter itself passed (the isotropic class is compiled): the panel says so again. A
    // compile failure blocked the worker, so "Forget GPU Test Result" cannot help (reload).
    if GATE.with(|gate| gate.borrow().compile_blocked) {
        show(ctx, GpuState::Ok, COMPILE_BLOCKED_TEXT, false);
    } else {
        show(
            ctx,
            GpuState::Ok,
            "Path-Traced GPU kernels are qualified",
            true,
        );
    }
    startup::end_run(ctx);
    if let Some(pool) = crate::workers::existing() {
        admit_if_ready(ctx, &pool);
    }
}

/// Initializes the GPU gate on page load (section 6.5): turns every crash marker left
/// behind in the `Probing` state into `Failed`, then runs the start-up test (or says why
/// not).
pub fn init(ctx: &Ctx) {
    let startup_key = admission::resolve_key(gpu_startup::STARTUP_KEY);
    for probing_key in admission::all_probing() {
        let mut rec = admission::load(&probing_key).unwrap_or_default();
        rec.state = AdmissionState::Failed;
        rec.reason = Some(if probing_key == startup_key {
            gpu_startup::REASON_STARTUP_LOST.to_string()
        } else {
            "the browser lost the GPU during the last test".to_string()
        });
        admission::save(&probing_key, &rec);
    }
    startup::install_pagehide();
    startup::start(ctx, true);
}

/// Whether the pool may start the GPU worker by itself when it is created: not in a browser
/// without WebGPU, and not after a start that was blocked (the worker's adapter request is
/// the step that run lost the GPU in).
#[must_use]
pub fn worker_allowed() -> bool {
    GATE.with(|gate| {
        let g = gate.borrow();
        !g.no_webgpu && !g.blocked_at_start
    })
}

/// Handles toggling "GPU Rendering": switching it on runs the same start-up test, switching
/// it off stops whatever runs and hands the render back to the CPU workers.
pub fn on_gpu_enabled_changed(ctx: &Ctx, enabled: bool) {
    if !enabled {
        ray::abandon();
        stop_deadline();
        startup::abandon(ctx);
        GATE.with(|gate| {
            let mut g = gate.borrow_mut();
            g.testing = false;
            g.admitting = false;
            g.starting = false;
            g.compiled_classes.clear();
            g.class_failed.clear();
            g.compile_blocked = false;
            g.no_webgpu = false;
            g.blocked_at_start = false;
        });
        show(ctx, GpuState::Off, "", true);
        if let Some(pool) = crate::workers::existing() {
            // Turning the GPU off hands a pass in flight back to the CPU workers.
            pool.render().set_gpu_admitted(false);
            pool.render().set_gpu_enabled(false);
        }
        status::refresh(ctx);
        return;
    }
    startup::start(ctx, true);
}

/// Handles events posted from the GPU worker to the host.
pub fn on_gpu_event(ctx: &Ctx, event: GpuEvent) {
    if ray::event(ctx, &event) {
        status::refresh(ctx);
        return;
    }
    handle_event(ctx, event);
    status::refresh(ctx);
}

/// The worker answered cleanly that it cannot be used (or was lost): the verdict is the
/// session's, the worker is given up and the start-up run ends.
fn give_up(ctx: &Ctx, text: &str) {
    stop_deadline();
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.failed_in_session = true;
        g.starting = false;
        g.testing = false;
        g.admitting = false;
        g.compiled_classes.clear();
    });
    // An answer, even a bad one, is not a lost browser: the start-up marker goes.
    startup::clear_marker();
    show(ctx, GpuState::Failed, text, false);
    if let Some(pool) = crate::workers::existing() {
        pool.render().set_gpu_admitted(false);
        pool.render().terminate_gpu();
    }
    startup::end_run(ctx);
}

fn handle_event(ctx: &Ctx, event: GpuEvent) {
    match event {
        GpuEvent::Adapter {
            vendor,
            architecture,
            device,
            description,
            backend,
            max_storage_buffers_per_stage,
            max_storage_binding,
            max_buffer,
        } => on_adapter(
            ctx,
            &AdapterFacts {
                vendor: &vendor,
                architecture: &architecture,
                device: &device,
                description: &description,
                backend: &backend,
                max_storage_buffers_per_stage,
                max_storage_binding,
                max_buffer,
            },
        ),
        GpuEvent::Unavailable { reason } => give_up(
            ctx,
            &format!("GPU unavailable: {reason}. Reload the page to try again."),
        ),
        GpuEvent::Compiled { kernel, class, ms } => on_compiled(ctx, kernel, class, ms),
        GpuEvent::CompileFailed {
            kernel,
            class,
            reason,
            ..
        } => {
            let (busy, run) = GATE.with(|gate| {
                let mut g = gate.borrow_mut();
                let busy = g.testing || g.admitting;
                // The worker refuses every compile after a failed one.
                g.compile_blocked |= busy;
                (busy, g.run())
            });
            if busy && class == run {
                fail_run(ctx, &format!("Kernel {kernel} failed to compile: {reason}"));
            }
        }
        GpuEvent::FullPassResult {
            scene_id,
            sums_valid,
            ..
        } => {
            let (testing, run) = GATE.with(|gate| {
                let g = gate.borrow();
                (g.testing, g.run())
            });
            if testing && scene_id == SELF_CHECK_SCENE_ID {
                if sums_valid {
                    on_test_success(ctx);
                } else {
                    fail_run(ctx, &selfcheck::invalid_frame_reason(run));
                }
            }
        }
        GpuEvent::PassDropped { scene_id } | GpuEvent::SceneRejected { scene_id, .. } => {
            let testing = GATE.with(|gate| gate.borrow().testing);
            if testing && scene_id == SELF_CHECK_SCENE_ID {
                fail_run(ctx, "The GPU worker could not run the self-check scene");
            }
        }
        GpuEvent::Lost { reason } => {
            stop_deadline();
            let (compiling, run) = GATE.with(|gate| {
                let mut g = gate.borrow_mut();
                let state = (g.testing || g.admitting, g.run());
                g.failed_in_session = true;
                g.starting = false;
                g.testing = false;
                g.admitting = false;
                g.compiled_classes.clear();
                state
            });
            // A lost device is a verdict on the adapter: the isotropic base set carries it, so
            // the whole GPU stays blocked on the next load. The unit that was compiling when the
            // device went (a kernel feature, say) is marked as well, so its crash marker does not
            // outlive the run.
            let lost = Some(format!("GPU device lost: {reason}"));
            save_record_for(ISOTROPIC_CLASS, AdmissionState::Failed, lost.clone());
            if compiling && run != ISOTROPIC_CLASS {
                save_record_for(run, AdmissionState::Failed, lost);
            }
            startup::clear_marker();
            show(
                ctx,
                GpuState::Failed,
                &format!("GPU lost: {reason}. Reload the page to try again."),
                false,
            );
            if let Some(pool) = crate::workers::existing() {
                pool.render().set_gpu_admitted(false);
                pool.render().terminate_gpu();
            }
            startup::end_run(ctx);
        }
    }
}

/// The GPU worker reported its adapter: check its limits, look up what is stored for it
/// and do what the start-up run is to do with it (`gpu_startup::adapter_plan`).
fn on_adapter(ctx: &Ctx, facts: &AdapterFacts<'_>) {
    ray::remember(facts);
    GATE.with(|gate| gate.borrow_mut().ray_only_device = facts.max_storage_buffers_per_stage < 8);
    let name = gpu_startup::adapter_display_name(facts);
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.starting = false;
        g.adapter_name = Some(name);
    });
    stop_deadline();
    startup::set_stage(ctx, Stage::Limits);

    if facts.max_storage_buffers_per_stage < 4 {
        give_up(
            ctx,
            &format!(
                "GPU unavailable (max storage buffers {} < 4)",
                facts.max_storage_buffers_per_stage
            ),
        );
        return;
    }

    // A record and a key per compile unit (each class's base set and each kernel feature of
    // it); the isotropic base set decides the start-up run.
    let mut keys = BTreeMap::new();
    let mut records = BTreeMap::new();
    for unit in classes::tracked_units() {
        let key = compute_unit_key(facts, unit);
        records.insert(unit, admission::load(&key).unwrap_or_default());
        keys.insert(unit, key);
    }
    let rec = records.get(&ISOTROPIC_CLASS).cloned().unwrap_or_default();
    let auto = GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.keys = keys;
        g.records = records;
        g.run_class = Some(ISOTROPIC_CLASS);
        g.compiled_classes.clear();
        g.class_failed.clear();
        g.compile_blocked = false;
        g.testing = false;
        g.admitting = false;
        g.auto
    });

    let Some(pool) = crate::workers::existing() else {
        return;
    };
    if ray::selected(ctx) {
        startup::clear_marker();
        ray::begin(ctx, &pool, false);
        return;
    }
    if GATE.with(|gate| gate.borrow().ray_only_device) {
        show(
            ctx,
            GpuState::Off,
            "This GPU device supports Ray-Traced; Path-Traced needs eight storage buffers.",
            false,
        );
        set_admitted(false);
        startup::clear_marker();
        startup::end_run(ctx);
        return;
    }
    match gpu_startup::adapter_plan(&rec, auto) {
        AdapterPlan::RunTest => begin_test(ctx, &pool, ISOTROPIC_CLASS),
        AdapterPlan::CompileOnly => start_admit_compile(ctx, &pool, ISOTROPIC_CLASS),
        AdapterPlan::WaitForUser => {
            show(ctx, GpuState::Unknown, "GPU not tested yet", true);
            set_admitted(false);
            startup::clear_marker();
            startup::end_run(ctx);
        }
        AdapterPlan::Blocked(reason) => {
            show(
                ctx,
                GpuState::Failed,
                &format!("GPU test failed: {reason}"),
                true,
            );
            set_admitted(false);
            startup::clear_marker();
            startup::end_run(ctx);
        }
    }
}

fn set_admitted(admitted: bool) {
    if let Some(pool) = crate::workers::existing() {
        pool.render().set_gpu_admitted(admitted);
    }
}

/// Called when the user clicks "Test GPU".
pub fn on_gpu_test_clicked(ctx: &Ctx) {
    if ray::selected(ctx) {
        if let Some(pool) = crate::workers::existing() {
            ray::begin(ctx, &pool, true);
        }
        return;
    }
    if ray::busy() || GATE.with(|gate| gate.borrow().ray_only_device) {
        return;
    }
    let Some(pool) = crate::workers::existing() else {
        show(ctx, GpuState::Unknown, "Workers not ready", true);
        return;
    };
    let (known, busy, blocked) = GATE.with(|gate| {
        let g = gate.borrow();
        (
            g.keys.contains_key(&ISOTROPIC_CLASS),
            g.testing || g.admitting,
            g.compile_blocked,
        )
    });
    if !known {
        show(ctx, GpuState::Unknown, "Waiting for GPU adapter...", true);
        return;
    }
    if busy || !pool.render().is_gpu_ready() {
        return;
    }
    if blocked {
        // The worker refuses every compile after a failed one (see `on_gpu_forget_clicked`).
        show(ctx, GpuState::Ok, COMPILE_BLOCKED_TEXT, false);
        return;
    }
    // The test needs no scene of its own (it renders the built-in self-check scene), so it
    // does not wait for the first image as it did when it was only a button.
    startup::begin_run(ctx, true);
    begin_test(ctx, &pool, ISOTROPIC_CLASS);
}

/// Called when the user clicks "Forget GPU Test Result".
pub fn on_gpu_forget_clicked(ctx: &Ctx) {
    if GATE.with(|gate| gate.borrow().blocked_at_start) {
        // The adapter was never reached: forgetting the start-up marker starts the run
        // again (it reaches the adapter and stops at "GPU not tested yet" if it is new).
        startup::forget_and_restart(ctx);
        return;
    }
    if ray::selected(ctx) {
        ray::forget(ctx);
        return;
    }
    let (usable, running, blocked) = GATE.with(|gate| {
        let g = gate.borrow();
        (
            !g.failed_in_session,
            g.testing || g.admitting,
            g.compile_blocked,
        )
    });
    if !usable || running {
        // The worker is gone for this page session (or busy); only a reload can test again.
        return;
    }
    if blocked {
        // A kernel failed to compile: the worker refuses every later compile, so a retest would
        // fail on the refusal and store a Failed verdict for a kernel that was never tried.
        show(ctx, GpuState::Ok, COMPILE_BLOCKED_TEXT, false);
        return;
    }
    let forgotten = GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        for key in g.keys.values() {
            admission::forget(key);
        }
        for record in g.records.values_mut() {
            *record = AdmissionRecord::default();
        }
        std::mem::take(&mut g.compiled_classes)
    });
    show(ctx, GpuState::Unknown, "GPU not tested yet", true);
    if let Some(pool) = crate::workers::existing() {
        for class in forgotten {
            pool.render().set_gpu_class_compiled(class, false);
        }
    }
    set_admitted(false);
    status::refresh(ctx);
}
