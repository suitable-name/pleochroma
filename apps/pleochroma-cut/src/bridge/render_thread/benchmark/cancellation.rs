//! Cooperative interruption of an active pass, as when another design is selected.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{RecvTimeoutError, channel},
    },
    thread,
    time::{Duration, Instant},
};

/// Requests cancellation after the deadline, then measures time until work returns.
/// This does not impose a hard timeout on shader compilation or a GPU wait. A fast
/// operation wakes the timer immediately rather than waiting out the deadline.
pub(super) fn with_deadline<T>(
    cancel: &AtomicBool,
    after_ms: Option<u64>,
    work: impl FnOnce() -> T,
) -> (T, Option<f64>) {
    let Some(after_ms) = after_ms else {
        return (work(), None);
    };
    thread::scope(|scope| {
        let (done_tx, done_rx) = channel();
        let timer =
            scope.spawn(
                move || match done_rx.recv_timeout(Duration::from_millis(after_ms)) {
                    Err(RecvTimeoutError::Timeout) => {
                        let requested = Instant::now();
                        cancel.store(true, Ordering::Release);
                        tracing::info!(
                            target: "pleochroma_cut::render_bench",
                            after_ms,
                            "benchmark cancellation requested"
                        );
                        Some(requested)
                    }
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => None,
                },
            );
        let result = work();
        let returned = Instant::now();
        let _ = done_tx.send(());
        let requested = match timer.join() {
            Ok(requested) => requested,
            Err(payload) => std::panic::resume_unwind(payload),
        };
        let latency_ms = requested
            .map(|instant| returned.saturating_duration_since(instant).as_secs_f64() * 1000.0);
        (result, latency_ms)
    })
}
