//! Shared adapter gate: absence skips locally, `PLEOCHROMA_REQUIRE_GPU=1` makes it a failure.
//!
//! Also the test-side guard against driver stalls. A user-space stall inside the Vulkan driver,
//! hit while several tests create and destroy instances and devices at once, blocks even the
//! libtest main thread (it joins a finished test's thread), so no test-side timeout can fire.
//!
//! - Creation and teardown are serialised in every build by `gpu::lifecycle`.
//!   [`acquire_serialised`], which [`GpuContext::acquire`] runs in a test build, adds the watch.
//!   Nothing is kept alive for the process except the shared context of [`require_gpu`]: a
//!   device that outlives the process can crash the driver's exit handlers (see
//!   `gpu::beam::device_caches`).
//! - [`watch`] names a phase that has no timeout of its own (device creation, pipeline
//!   compilation, and the teardown of a device inside the lifecycle turn). A monitor thread
//!   reports a phase that passes its bound, with the test thread it runs on, and aborts the test
//!   process.
//!
//! Switches:
//! - `PLEOCHROMA_GPU_TEST_GATE=0` makes the lifecycle turn a pass-through (unserialised).
//! - `PLEOCHROMA_GPU_TEST_WATCHDOG=0` switches the monitor off (under a debugger); `=report`
//!   makes it report without aborting, and already when a phase has run for 60 s. The retest
//!   script sets `report`: its stall detector dumps every thread's state before it kills the
//!   step, and the report's thread ids match that dump.

use std::{
    fmt::Write as _,
    io::Write as _,
    sync::{
        Mutex, MutexGuard, OnceLock, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use super::{GpuAcquireError, GpuContext};

/// The bound on waiting for the lifecycle turn, creating an instance, an adapter and a device,
/// and tearing a device down inside the turn (`gpu::lifecycle`).
///
/// A creation takes well under a second on a healthy driver. The bound matches
/// `compute::SELF_TEST_WAIT_TIMEOUT`.
pub const ACQUIRE_BOUND: Duration = Duration::from_secs(120);

/// The bound on compiling one compute pipeline.
///
/// The megakernel compiles in seconds on Vulkan and much slower through FXC on DX12, so this
/// bound only ends a stuck compile.
pub const PIPELINE_COMPILE_BOUND: Duration = Duration::from_mins(30);

/// In report mode the monitor also reports a phase that has run this long.
///
/// The report lists every phase in flight with its kernel thread id, so the log of a stalled
/// step says which tests were creating a device, tearing one down or compiling, and so which
/// were not.
const REPORT_AFTER: Duration = Duration::from_mins(1);

/// `0` makes the lifecycle turn a pass-through.
const GATE_ENV: &str = "PLEOCHROMA_GPU_TEST_GATE";

/// `0` switches the monitor thread off; `report` makes it report without aborting.
const WATCHDOG_ENV: &str = "PLEOCHROMA_GPU_TEST_WATCHDOG";

/// Whether the environment variable `name` is set to `0`.
fn switched_off(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| value == "0")
}

/// Whether `PLEOCHROMA_GPU_TEST_GATE=0` asks for unserialised creation and teardown.
pub(in crate::renderer::gpu) fn gate_switched_off() -> bool {
    switched_off(GATE_ENV)
}

/// The shared GPU context, or `None` (the test skips) when no adapter is available.
///
/// # Panics
///
/// Panics when `PLEOCHROMA_REQUIRE_GPU=1` is set and no adapter can be acquired.
#[must_use]
pub fn require_gpu() -> Option<GpuContext> {
    static CONTEXT: OnceLock<Result<GpuContext, String>> = OnceLock::new();
    match CONTEXT.get_or_init(|| {
        let _watch = watch(
            "waiting for the turn and creating the shared GPU instance, adapter and device",
            ACQUIRE_BOUND,
        );
        pollster::block_on(super::whitted::context::acquire()).map_err(|error| error.to_string())
    }) {
        Ok(context) => Some(context.clone()),
        Err(error) => {
            assert!(
                !std::env::var("PLEOCHROMA_REQUIRE_GPU").is_ok_and(|value| value == "1"),
                "PLEOCHROMA_REQUIRE_GPU=1, but no GPU is available: {error}"
            );
            eprintln!("GPU tests skipped: {error}");
            None
        }
    }
}

/// [`GpuContext::acquire`] of a test build: the same acquisition (it takes the lifecycle turn
/// itself), watched.
///
/// # Errors
///
/// The errors of [`GpuContext::acquire_async`].
pub fn acquire_serialised() -> Result<GpuContext, GpuAcquireError> {
    let _watch = watch(
        "waiting for the turn and creating a GPU instance, adapter and device",
        ACQUIRE_BOUND,
    );
    pollster::block_on(GpuContext::acquire_async())
}

/// One watched phase.
struct Watched {
    /// The guard's identity.
    id: u64,
    /// What the thread is doing.
    what: String,
    /// The name of the thread (libtest names it after the test).
    thread: String,
    /// The kernel's id of the thread ([`kernel_thread_id`]).
    tid: String,
    /// When the phase began.
    started: Instant,
    /// How long it may take.
    bound: Duration,
}

/// What the monitor does once a phase has passed its bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnOverdue {
    /// Report, then abort the test process.
    Abort,
    /// Report each phase once (already after [`REPORT_AFTER`]) and keep running.
    Report,
}

/// The phases in flight.
static WATCHED: Mutex<Vec<Watched>> = Mutex::new(Vec::new());

/// The next guard identity.
static NEXT_WATCH: AtomicU64 = AtomicU64::new(0);

/// Set once the monitor thread was started (or found switched off).
static MONITOR: OnceLock<()> = OnceLock::new();

/// The list of phases in flight.
fn watched() -> MutexGuard<'static, Vec<Watched>> {
    WATCHED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A watched phase; dropping it ends the watch.
#[must_use = "the phase is watched until the guard is dropped"]
pub struct Watch(u64);

impl Drop for Watch {
    fn drop(&mut self) {
        watched().retain(|phase| phase.id != self.0);
    }
}

/// The calling thread's id as the kernel and `/proc` know it on Linux, else `?`.
///
/// The stall dump of `scripts/retest.sh` lists threads by this id.
fn kernel_thread_id() -> String {
    std::fs::read_link("/proc/thread-self")
        .ok()
        .and_then(|link| {
            link.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "?".to_owned())
}

/// Watches the calling thread's phase `what` until the returned guard is dropped.
///
/// When the phase runs longer than `bound`, the monitor thread prints every phase in flight to the
/// real standard error (not the test's captured output) and, unless
/// `PLEOCHROMA_GPU_TEST_WATCHDOG=report`, aborts the process: a stuck driver call cannot be
/// cancelled, and a stalled test process otherwise blocks its whole step.
///
/// # Panics
///
/// Panics when the monitor thread cannot be started.
pub fn watch(what: &str, bound: Duration) -> Watch {
    MONITOR.get_or_init(|| {
        let mode = match std::env::var_os(WATCHDOG_ENV) {
            Some(value) if value == "0" => return,
            Some(value) if value == "report" => OnOverdue::Report,
            _ => OnOverdue::Abort,
        };
        // Never joined and never exits: it must outlive every test.
        std::thread::Builder::new()
            .name("gpu test watchdog".to_owned())
            .spawn(move || monitor(mode))
            .expect("the GPU test watchdog thread starts");
    });
    let id = NEXT_WATCH.fetch_add(1, Ordering::Relaxed);
    let thread = std::thread::current()
        .name()
        .unwrap_or("unnamed thread")
        .to_owned();
    watched().push(Watched {
        id,
        what: what.to_owned(),
        thread,
        tid: kernel_thread_id(),
        started: Instant::now(),
        bound,
    });
    Watch(id)
}

/// The monitor thread: checks the phases once a second, reports a phase that is due and, in
/// [`OnOverdue::Abort`], aborts the process.
fn monitor(mode: OnOverdue) {
    let mut reported = Vec::new();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let report = fresh_report(&watched(), &mut reported, Instant::now(), mode);
        let Some(report) = report else {
            continue;
        };
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(report.as_bytes());
        let _ = stderr.flush();
        drop(stderr);
        if mode == OnOverdue::Abort {
            std::process::abort();
        }
    }
}

/// How long `phase` runs before the monitor reports it in `mode`.
fn report_threshold(phase: &Watched, mode: OnOverdue) -> Duration {
    match mode {
        OnOverdue::Abort => phase.bound,
        OnOverdue::Report => phase.bound.min(REPORT_AFTER),
    }
}

/// The report when a phase of `phases` is due at `now` and not in `reported` yet, else `None`.
///
/// The newly due identities are added to `reported`: each phase is reported once.
fn fresh_report(
    phases: &[Watched],
    reported: &mut Vec<u64>,
    now: Instant,
    mode: OnOverdue,
) -> Option<String> {
    let fresh: Vec<u64> = phases
        .iter()
        .filter(|phase| {
            now.saturating_duration_since(phase.started) >= report_threshold(phase, mode)
                && !reported.contains(&phase.id)
        })
        .map(|phase| phase.id)
        .collect();
    if fresh.is_empty() {
        return None;
    }
    reported.extend(fresh);
    Some(report_text(phases, now, mode))
}

/// The monitor's message: a heading for `mode`, then every phase of `phases` with its state at
/// `now`.
fn report_text(phases: &[Watched], now: Instant, mode: OnOverdue) -> String {
    let ran = |phase: &Watched| now.saturating_duration_since(phase.started);
    let mut text = String::from(match mode {
        OnOverdue::Abort => {
            "\nGPU test watchdog: a watched GPU phase passed its bound; aborting the test process.\n"
        }
        OnOverdue::Report => {
            "\nGPU test watchdog (report only; the test process keeps running): a watched GPU \
             phase has run for 60 s or passed its bound.\n"
        }
    });
    for phase in phases {
        let state = if ran(phase) >= phase.bound {
            "OVER ITS BOUND"
        } else {
            "in flight"
        };
        let _ = writeln!(
            text,
            "  {state}: {} on thread `{}` (thread id {}), running for {} s (bound {} s)",
            phase.what,
            phase.thread,
            phase.tid,
            ran(phase).as_secs(),
            phase.bound.as_secs()
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{OnOverdue, Watched, fresh_report, watch, watched};
    use std::time::{Duration, Instant};

    fn phase(id: u64, what: &str, started: Instant, bound_seconds: u64) -> Watched {
        Watched {
            id,
            what: what.to_owned(),
            thread: "test thread".to_owned(),
            tid: "7".to_owned(),
            started,
            bound: Duration::from_secs(bound_seconds),
        }
    }

    #[test]
    fn nothing_is_reported_while_every_phase_is_within_its_bound() {
        let started = Instant::now();
        let phases = [phase(1, "compiling a pipeline", started, 10)];
        let early = started + Duration::from_secs(9);
        assert_eq!(
            fresh_report(&phases, &mut Vec::new(), early, OnOverdue::Abort),
            None
        );
        assert_eq!(
            fresh_report(&phases, &mut Vec::new(), early, OnOverdue::Report),
            None
        );
        assert_eq!(
            fresh_report(&[], &mut Vec::new(), started, OnOverdue::Abort),
            None
        );
    }

    #[test]
    fn an_overdue_phase_is_reported_with_the_phases_still_in_flight() {
        let started = Instant::now();
        let phases = [
            phase(1, "creating a device", started, 10),
            phase(2, "compiling a pipeline", started, 600),
        ];
        let report = fresh_report(
            &phases,
            &mut Vec::new(),
            started + Duration::from_secs(10),
            OnOverdue::Abort,
        )
        .expect("one phase is overdue");
        assert!(report.contains("aborting the test process"), "{report}");
        assert!(
            report.contains(
                "OVER ITS BOUND: creating a device on thread `test thread` (thread id 7), \
                 running for 10 s (bound 10 s)"
            ),
            "{report}"
        );
        assert!(
            report.contains(
                "in flight: compiling a pipeline on thread `test thread` (thread id 7), \
                 running for 10 s (bound 600 s)"
            ),
            "{report}"
        );
    }

    #[test]
    fn an_overdue_phase_is_reported_once() {
        let started = Instant::now();
        let phases = [phase(7, "creating a device", started, 10)];
        let late = started + Duration::from_secs(11);
        let mut reported = Vec::new();
        let first = fresh_report(&phases, &mut reported, late, OnOverdue::Report)
            .expect("the phase is overdue");
        assert!(first.contains("report only"), "{first}");
        assert_eq!(reported, vec![7]);
        assert_eq!(
            fresh_report(&phases, &mut reported, late, OnOverdue::Report),
            None
        );
    }

    #[test]
    fn in_report_mode_a_slow_phase_is_reported_before_its_bound() {
        let started = Instant::now();
        let phases = [phase(3, "compiling a pipeline", started, 1800)];
        let before = started + Duration::from_secs(59);
        let after = started + Duration::from_secs(60);
        assert_eq!(
            fresh_report(&phases, &mut Vec::new(), before, OnOverdue::Report),
            None
        );
        assert_eq!(
            fresh_report(&phases, &mut Vec::new(), after, OnOverdue::Abort),
            None
        );
        let report = fresh_report(&phases, &mut Vec::new(), after, OnOverdue::Report)
            .expect("the phase has run for 60 s");
        assert!(
            report.contains(
                "in flight: compiling a pipeline on thread `test thread` (thread id 7), \
                 running for 60 s (bound 1800 s)"
            ),
            "{report}"
        );
    }

    #[test]
    fn a_watch_is_listed_until_its_guard_is_dropped() {
        let guard = watch("a test phase", Duration::from_secs(3600));
        let id = guard.0;
        assert!(watched().iter().any(|phase| phase.id == id));
        drop(guard);
        assert!(!watched().iter().any(|phase| phase.id == id));
    }
}
