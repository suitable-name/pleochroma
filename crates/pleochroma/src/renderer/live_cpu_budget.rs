//! Thread limits applied only by live-view callers; exports retain their own policy.
use std::cell::Cell;

thread_local! {
    static LIMIT: Cell<usize> = const { Cell::new(usize::MAX) };
}

#[cfg(any(not(target_arch = "wasm32"), test))]
pub(super) fn limit(requested: usize) -> usize {
    LIMIT.with(|limit| requested.min(limit.get()).max(1))
}

/// Run a synchronous pipeline with a cap shared by its nested filter/tonemap stages.
/// The previous limit is restored on every exit, including unwinding.
pub fn with_limit<T>(threads: usize, work: impl FnOnce() -> T) -> T {
    struct Restore(usize);
    impl Drop for Restore {
        fn drop(&mut self) {
            LIMIT.with(|limit| limit.set(self.0));
        }
    }
    let _restore = Restore(LIMIT.with(|limit| limit.replace(limit.get().min(threads.max(1)))));
    work()
}

/// Reserve display capacity while the tracer is runnable. A single-core machine
/// serializes the two stages through the shared lease below instead of oversubscribing.
#[must_use]
pub fn partition() -> (usize, usize) {
    let total = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    partition_for(total)
}

fn partition_for(total: usize) -> (usize, usize) {
    let display = (total / 4).max(1);
    (total.saturating_sub(display).max(1), display)
}

static TRACE_STAGE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Tracing and calibration use the same reserved CPU lane rather than spawning
/// a probe beside an already saturated tracer after a GPU decline.
pub fn trace_lease() -> std::sync::MutexGuard<'static, ()> {
    TRACE_STAGE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

static SINGLE_CORE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// On one core, stages share one lease. On larger machines the fixed partition
/// allows display and tracing to overlap without exceeding available capacity.
pub fn single_core_lease() -> Option<std::sync::MutexGuard<'static, ()>> {
    (std::thread::available_parallelism().map_or(1, std::num::NonZero::get) == 1).then(|| {
        SINGLE_CORE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partitions_fit_available_capacity() {
        for cores in 2..=256 {
            let (trace, display) = partition_for(cores);
            assert!(trace > 0 && display > 0);
            assert_eq!(trace + display, cores);
        }
    }
    #[test]
    fn nested_limits_restore_after_unwind() {
        with_limit(3, || {
            assert_eq!(limit(8), 3);
            let _ = std::panic::catch_unwind(|| with_limit(1, || panic!("test unwind")));
            assert_eq!(limit(8), 3);
        });
        assert_eq!(limit(8), 8);
    }
}
