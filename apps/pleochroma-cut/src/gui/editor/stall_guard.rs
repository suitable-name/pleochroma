//! UI-thread stall detector. [`stall_guard`] logs a `tracing::warn!` naming the callback
//! when its body runs longer than [`STALL_THRESHOLD`].
//!
//! The threshold is one frame of a 60 Hz UI. The app's `tracing` subscriber writes
//! `pleochroma-cut.log` beside the executable, so a stall shows up there with no new UI.
//!
//! # Why a plain wrapper function
//!
//! Wrapping a callback body in `stall_guard::stall_guard("name", || { ... })` needs one
//! added call per site and no new syntax. The closure keeps its own early returns, since
//! `stall_guard` only measures how long the body took.
//!
//! # Why a spawned worker is not measured
//!
//! A callback that hands work to a background thread (`SolveService`, `deep_solve`,
//! `optimize_solve`, `auto_solve::dispatch_background_solve`) returns once the dispatch is
//! queued. `stall_guard` measures that return only, not the later
//! `Weak::upgrade_in_event_loop` completion closure.

use std::time::{Duration, Instant};

/// A callback body longer than this on the UI thread is a visible stall at 60 Hz (one frame
/// is about 16.7 ms).
const STALL_THRESHOLD: Duration = Duration::from_millis(16);

/// Runs `body`, logging a `tracing::warn!` naming `name` and the elapsed time if it took
/// longer than [`STALL_THRESHOLD`]. Returns `body`'s own result unchanged.
pub(super) fn stall_guard<R>(name: &str, body: impl FnOnce() -> R) -> R {
    let start = Instant::now();
    let result = body();
    let elapsed = start.elapsed();
    if is_stall(elapsed) {
        tracing::warn!(
            callback = name,
            elapsed_ms = elapsed.as_millis(),
            "UI-thread callback exceeded the 16ms stall threshold"
        );
    }
    result
}

/// The threshold check itself, pulled out so a test can drive it directly without
/// needing a closure that actually sleeps [`STALL_THRESHOLD`] or longer.
const fn is_stall(elapsed: Duration) -> bool {
    elapsed.as_millis() > STALL_THRESHOLD.as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stall_guard_returns_the_bodys_own_result() {
        let result = stall_guard("test_callback", || 42);
        assert_eq!(result, 42);
    }

    #[test]
    fn stall_guard_runs_the_body_exactly_once() {
        let mut calls = 0;
        stall_guard("test_callback", || {
            calls += 1;
        });
        assert_eq!(calls, 1);
    }

    #[test]
    fn is_stall_is_false_at_and_under_the_threshold() {
        assert!(!is_stall(Duration::from_millis(0)));
        assert!(!is_stall(Duration::from_millis(16)));
    }

    #[test]
    fn is_stall_is_true_over_the_threshold() {
        assert!(is_stall(Duration::from_millis(17)));
        assert!(is_stall(Duration::from_secs(1)));
    }
}
