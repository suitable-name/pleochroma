//! The gate's compile runs of one compile unit: the full test of a class (compile, then the
//! 16x16 self-check), the `admit` compile of a unit that passed earlier, the test of a kernel
//! feature (compile only, no self-check render), and what happens when every kernel answered.
//!
//! A unit is the base set of a material class or one kernel feature of it
//! (`gpu_admission::feature_unit`). Every run writes the unit's own `Probing` crash marker
//! BEFORE the compile request and is watched by the page-side deadline; a failure goes to
//! `fail_run` in the parent module.

use pleochroma_web_core::{
    design_meta::iso8601_utc_from_epoch_ms,
    gpu_admission::{self, SELF_CHECK_SCENE_ID},
    gpu_startup::{GpuState, Stage},
    host::WorkerPool,
};

use super::{
    GATE, admission, admit_if_ready, classes, fail_run, ray, restart_deadline, save_record,
    selfcheck, show, startup, stop_deadline,
};
use crate::{
    app::Ctx,
    storage::admission::{AdmissionRecord, AdmissionState},
};

/// One kernel answered a compile request (of a test or of `admit`) for compile unit `class`
/// (the Worker echoes the unit it was asked to compile).
pub(super) fn on_compiled(ctx: &Ctx, kernel: String, class: u32, ms: f64) {
    let (testing, admitting, run) = GATE.with(|gate| {
        let g = gate.borrow();
        (g.testing, g.admitting, g.run())
    });
    if (!testing && !admitting) || class != run {
        return;
    }
    let total = gpu_admission::kernels_for_unit(run).len();
    let count = GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        if let Some(rec) = g.records.get_mut(&run) {
            rec.per_kernel_ms.insert(kernel.clone(), ms);
            rec.total_ms += ms;
            rec.max_ms = rec.max_ms.max(ms);
        }
        g.compiled_kernels.insert(kernel);
        g.compiled_kernels.len()
    });
    restart_deadline(ctx);
    startup::set_stage(ctx, Stage::Compile { done: count, total });
    if count < total {
        return;
    }
    if admitting {
        finish_admit(ctx);
        return;
    }
    // A kernel feature is judged by its compile alone: the self-check scenes do not scatter
    // and have no frosted facet, so a render would not run its kernels.
    if !GATE.with(|gate| gate.borrow().with_self_check) {
        on_test_success(ctx);
        return;
    }
    // All kernels compiled: run the 16x16 self-check. The worker then holds the self-check
    // scene; the pool gives it the real one again before the next real pass.
    let Some(pool) = crate::workers::existing() else {
        return;
    };
    startup::set_stage(ctx, Stage::SelfCheck);
    if let Err(e) = pool.render().run_gpu_self_check(
        SELF_CHECK_SCENE_ID,
        selfcheck::scene_for_class(run),
        selfcheck::spp_for_class(run),
    ) {
        fail_run(ctx, &format!("Self-check dispatch failed: {e}"));
    }
}

/// Starts the full test of compile unit `class` on the known adapter: the unit's crash marker
/// first, then the compile request, then (when every kernel answered) the self-check. A
/// kernel feature has no self-check: its test ends when every kernel compiled.
pub(super) fn begin_test(ctx: &Ctx, pool: &WorkerPool, class: u32) {
    let Some(key) = GATE.with(|gate| gate.borrow().keys.get(&class).cloned()) else {
        return;
    };
    let rec = AdmissionRecord {
        state: AdmissionState::Probing,
        when: iso8601_utc_from_epoch_ms(js_sys::Date::now()),
        ..AdmissionRecord::default()
    };
    // Save probing state into storage BEFORE sending any compile request (section 6.5).
    admission::save(&key, &rec);
    // The adapter's own marker is in place: the start-up one is not needed any more.
    startup::clear_marker();
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.testing = true;
        g.run_class = Some(class);
        g.compiled_kernels.clear();
        g.records.insert(class, rec);
        g.with_self_check = gpu_admission::unit_feature(class).is_none();
    });
    send_class_compile(ctx, pool, class);
}

/// The self-check render of the running class came back valid.
pub(super) fn on_test_success(ctx: &Ctx) {
    stop_deadline();
    let class = GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.testing = false;
        // The whole kernel list of the class was just compiled in this page's worker.
        let class = g.run();
        g.compiled_classes.insert(class);
        class
    });
    finish_run(ctx, class, true);
}

/// The `admit(scene)` step (section 6.5): with a stored `ok` record, the worker compiles
/// the kernel list of `class` under the same `Probing` marker and deadlines as "Test GPU"
/// before the first real pass. Only then is the GPU admitted.
pub(super) fn start_admit_compile(ctx: &Ctx, pool: &WorkerPool, class: u32) {
    let key = GATE.with(|gate| gate.borrow().keys.get(&class).cloned());
    let Some(key) = key else {
        return;
    };
    let rec = GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        let mut rec = g.records.get(&class).cloned().unwrap_or_default();
        rec.state = AdmissionState::Probing;
        rec.per_kernel_ms.clear();
        rec.max_ms = 0.0;
        rec.total_ms = 0.0;
        rec.reason = None;
        rec.when = iso8601_utc_from_epoch_ms(js_sys::Date::now());
        g.admitting = true;
        g.run_class = Some(class);
        g.compiled_kernels.clear();
        g.records.insert(class, rec.clone());
        g.with_self_check = false;
        rec
    });
    // The crash marker first, then the request.
    admission::save(&key, &rec);
    startup::clear_marker();
    send_class_compile(ctx, pool, class);
}

/// Shows the compile at 0 and sends the request for the kernels of compile unit `class` (a
/// base set or one feature); a request that cannot be sent fails the run.
fn send_class_compile(ctx: &Ctx, pool: &WorkerPool, class: u32) {
    startup::set_stage(
        ctx,
        Stage::Compile {
            done: 0,
            total: gpu_admission::kernels_for_unit(class).len(),
        },
    );
    restart_deadline(ctx);
    if let Err(e) = pool.render().send_gpu_compile(
        class,
        classes::kernel_names(class),
        gpu_admission::COMPILE_SOFT_MS,
        gpu_admission::COMPILE_HARD_MS,
    ) {
        fail_run(ctx, &format!("Could not send compile request: {e}"));
    }
}

/// Every kernel answered the `admit` compile: the marker goes back to `ok`.
fn finish_admit(ctx: &Ctx) {
    stop_deadline();
    let class = GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.admitting = false;
        let class = g.run();
        g.compiled_classes.insert(class);
        class
    });
    finish_run(ctx, class, false);
}

/// A test or an `admit` compile of `class` passed: the record is `ok`, the Worker is told
/// the class is usable, and the scene is admitted when it is of that class. A test (`sync`)
/// also asks the live render to compare its scene again.
fn finish_run(ctx: &Ctx, class: u32, sync: bool) {
    let total_ms = save_record(AdmissionState::Ok, None);
    startup::set_stage(ctx, Stage::Done);
    show(
        ctx,
        GpuState::Ok,
        &classes::passed_text(class, total_ms),
        true,
    );
    if let Some(pool) = crate::workers::existing() {
        pool.render().set_gpu_class_compiled(class, true);
        admit_if_ready(ctx, &pool);
    }
    // `admit_if_ready` may have started the next compile this stone needs (another kernel
    // feature): that run keeps the progress card up and says what the app renders with when it
    // ends.
    if !GATE.with(|gate| gate.borrow().busy()) {
        startup::end_run(ctx);
    }
    if let Some(pool) = crate::workers::existing() {
        ray::begin(ctx, &pool, false);
    }
    if sync {
        crate::render::request_sync(ctx);
    }
}
