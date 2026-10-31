//! The GPU Worker's `GpuCompile` handling: one test dispatch per kernel under error scopes,
//! so a shader that does not compile, or a driver that stalls, is reported to the page
//! instead of taking the GPU down silently.
//!
//! Split out of `lib.rs` (the message handler grew past the lint's line limit); the shared
//! state, `post` and `timeout_race` live in the parent module.

use std::{
    cell::{Cell, RefCell},
    ops::ControlFlow,
    rc::Rc,
};

use pleochroma::renderer::gpu::event_wavefront::{EwError, EwKernel};
use pleochroma_web_core::{
    gpu_admission,
    protocol::{FromWorker, WorkerRole},
};

use super::{
    COMPILE_BLOCKED, GpuWorkerState, RaceResult, block_compile, now_ms, post, timeout_race,
};

/// The kernel the page (`Tail`) or the shader entry point (`ew_tail`) names.
enum WorkerKernel {
    Event(EwKernel),
    Whitted,
}

fn parse_kernel_name(name: &str) -> Option<WorkerKernel> {
    if name.eq_ignore_ascii_case("whitted") {
        return Some(WorkerKernel::Whitted);
    }
    parse_event_kernel_name(name).map(WorkerKernel::Event)
}

/// The name the page and the Worker's ledger use for `kernel`: its variant name (`Tail`).
///
/// This is the one mapping between [`EwKernel`] and kernel names in the Worker, so a kernel
/// added to the enum needs no list kept here; the page's own name tables are pinned to the
/// desktop's kernel selection by a native test in `pleochroma_web_core::gpu_admission`.
pub(super) fn ew_kernel_name(kernel: EwKernel) -> String {
    format!("{kernel:?}")
}

/// The kernel `name` stands for: its variant name (`Tail`) or its shader entry point
/// (`ew_tail`).
fn parse_event_kernel_name(name: &str) -> Option<EwKernel> {
    EwKernel::ALL
        .into_iter()
        .find(|kernel| kernel.entry_point() == name || ew_kernel_name(*kernel) == name)
}

/// A future that resolves once the queue has finished all work submitted so far.
fn wait_submitted_work_done(
    queue: &wgpu::Queue,
) -> impl std::future::Future<Output = ()> + 'static {
    let (tx, rx) = futures_channel::oneshot::channel();
    queue.on_submitted_work_done(move || {
        let _ = tx.send(());
    });
    async move {
        let _ = rx.await;
    }
}

/// Posts `GpuCompileFailed` for `kernel`.
fn report_failure(kernel: &str, class: u32, reason: String, ms: f64) {
    post(&FromWorker::GpuCompileFailed {
        kernel: kernel.to_string(),
        class,
        reason,
        ms,
    });
}

/// Pushes the validation and the internal error scope on the renderer's device. Posts a
/// failure and gives `None` when there is no renderer.
fn push_error_scopes(
    state_rc: &Rc<RefCell<GpuWorkerState>>,
    kernel_name: &str,
    class: u32,
) -> Option<(wgpu::ErrorScopeGuard, wgpu::ErrorScopeGuard)> {
    let state = state_rc.borrow();
    let Some(renderer) = &state.renderer else {
        report_failure(
            kernel_name,
            class,
            "renderer is not initialized".to_string(),
            0.0,
        );
        return None;
    };
    let device = &renderer.context().device;
    Some((
        device.push_error_scope(wgpu::ErrorFilter::Validation),
        device.push_error_scope(wgpu::ErrorFilter::Internal),
    ))
}

/// Runs the renderer's test dispatch of `kernel` for compile `unit`.
///
/// The pipeline is the browser variant of the unit's material class
/// (`gpu_admission::browser_pipeline_class`: no oriented inclusions, no zoning), the one a pass
/// traces with. This is the only place the Worker compiles an event-wavefront kernel for a
/// class, so the all-features variant, whose `Scatter` kernel exceeds the compile deadline, is
/// never built.
fn test_dispatch(
    state_rc: &Rc<RefCell<GpuWorkerState>>,
    kernel: EwKernel,
    unit: u32,
) -> Result<(), EwError> {
    let pipeline_class = gpu_admission::browser_pipeline_class(gpu_admission::unit_class(unit));
    let mut state = state_rc.borrow_mut();
    state.renderer.as_mut().map_or_else(
        || {
            Err(EwError::DeviceLost(
                "renderer is not initialized".to_string(),
            ))
        },
        |renderer| renderer.test_dispatch_kernel(kernel, pipeline_class),
    )
}

/// Pops both error scopes after a finished dispatch and judges the compile: an unresolved
/// scope, a shader error or a compile slower than `soft_ms` fails (and blocks every later
/// compile); otherwise the kernel is reported compiled.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn settle_compile(
    kernel_name: &str,
    class: u32,
    soft_ms: u32,
    start_ms: f64,
    (int_guard, val_guard): (wgpu::ErrorScopeGuard, wgpu::ErrorScopeGuard),
) -> ControlFlow<()> {
    let popped = timeout_race(
        async {
            let int_err = int_guard.pop().await;
            let val_err = val_guard.pop().await;
            (int_err, val_err)
        },
        1000.0,
    )
    .await;
    let elapsed_ms = now_ms() - start_ms;
    let RaceResult::Completed((int_err, val_err)) = popped else {
        block_compile();
        report_failure(
            kernel_name,
            class,
            "error scope did not resolve".to_string(),
            elapsed_ms,
        );
        return ControlFlow::Break(());
    };

    if let Some(err) = val_err.or(int_err) {
        block_compile();
        report_failure(
            kernel_name,
            class,
            format!("shader compile error: {err}"),
            elapsed_ms,
        );
        return ControlFlow::Break(());
    }

    if elapsed_ms > f64::from(soft_ms) {
        block_compile();
        report_failure(
            kernel_name,
            class,
            format!("compile duration {elapsed_ms:.1} ms exceeded soft limit of {soft_ms} ms"),
            elapsed_ms,
        );
        return ControlFlow::Break(());
    }

    post(&FromWorker::GpuCompiled {
        kernel: kernel_name.to_string(),
        class,
        ms: elapsed_ms,
    });
    ControlFlow::Continue(())
}

/// Test-compiles one kernel. `Break` means no further kernel is tried (a failure was
/// reported, or the renderer is gone).
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn compile_kernel(
    state_rc: &Rc<RefCell<GpuWorkerState>>,
    kernel_name: &str,
    class: u32,
    soft_ms: u32,
    hard_ms: u32,
) -> ControlFlow<()> {
    let Some(kernel) = parse_kernel_name(kernel_name) else {
        report_failure(
            kernel_name,
            class,
            format!("unknown kernel name: {kernel_name}"),
            0.0,
        );
        return ControlFlow::Break(());
    };
    let WorkerKernel::Event(kernel) = kernel else {
        return if super::ray::compile(state_rc, class, soft_ms, hard_ms).await {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(())
        };
    };

    let start_ms = now_ms();
    let Some((val_guard, int_guard)) = push_error_scopes(state_rc, kernel_name, class) else {
        return ControlFlow::Break(());
    };

    if let Err(e) = test_dispatch(state_rc, kernel, class) {
        // Report first; the scope pops may never resolve on a sick device.
        block_compile();
        report_failure(
            kernel_name,
            class,
            format!("test dispatch failed: {e}"),
            now_ms() - start_ms,
        );
        let _ = timeout_race(
            async {
                let _ = int_guard.pop().await;
                let _ = val_guard.pop().await;
            },
            1000.0,
        )
        .await;
        return ControlFlow::Break(());
    }

    let done_fut = {
        let state = state_rc.borrow();
        let Some(renderer) = state.renderer.as_ref() else {
            return ControlFlow::Break(());
        };
        wait_submitted_work_done(&renderer.context().queue)
    };

    match timeout_race(done_fut, f64::from(hard_ms)).await {
        RaceResult::TimedOut => {
            // Report at once. The scope pops resolve in queue order and
            // would never resolve behind a stuck compile, so the guards
            // are leaked instead of awaited; no further compile starts.
            block_compile();
            report_failure(
                kernel_name,
                class,
                format!("compile timed out after {hard_ms} ms"),
                f64::from(hard_ms),
            );
            std::mem::forget(int_guard);
            std::mem::forget(val_guard);
            ControlFlow::Break(())
        }
        RaceResult::Completed(()) => {
            let flow = settle_compile(
                kernel_name,
                class,
                soft_ms,
                start_ms,
                (int_guard, val_guard),
            )
            .await;
            if flow.is_continue() {
                // Remembered for the kind of the unit's class: a pass is traced only when every
                // kernel it builds was compiled here (`trace::take_pass_inputs`).
                state_rc
                    .borrow_mut()
                    .compiled
                    .record(gpu_admission::unit_class(class), &ew_kernel_name(kernel));
            }
            flow
        }
    }
}

/// Handles `GpuCompile`: test-compiles `kernels` in order, stopping at the first failure.
/// Refused before a successful `Init`, and after an earlier failure blocked compiling.
///
/// `class` is the compile unit (`gpu_admission::feature_unit`): a material class for a base
/// set, or a feature of a class. It is echoed in every answer; each kernel that compiled is
/// recorded in the Worker's ledger for the unit's material kind.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub(super) async fn handle_compile(
    state_rc: &Rc<RefCell<GpuWorkerState>>,
    class: u32,
    kernels: &[String],
    soft_ms: u32,
    hard_ms: u32,
) {
    if state_rc.borrow().role != Some(WorkerRole::Gpu) {
        post(&FromWorker::Error {
            message: "GpuCompile before a successful Init".to_string(),
        });
        return;
    }
    if COMPILE_BLOCKED.with(Cell::get) {
        report_failure(
            kernels.first().map_or("", String::as_str),
            class,
            "compilation disabled after an earlier failure".to_string(),
            0.0,
        );
        return;
    }

    for kernel_name in kernels {
        if compile_kernel(state_rc, kernel_name, class, soft_ms, hard_ms)
            .await
            .is_break()
        {
            return;
        }
    }
}
