//! A process-wide permit pool bounding the CPU-fallback tracer threads of the batch
//! preview lanes.
//!
//! The batch lanes (`gui::batch::batch_queue::local_lane_count`) share one `GpuBackend`.
//! When the GPU is unavailable or refuses an item, each lane falls back to
//! `export_thread::batch::render_batch_with_threads`. Unbounded, every lane spawns
//! `available_parallelism` threads: on a 16-core host, 15 lanes ran 240 threads that
//! thrash each other's caches.
//!
//! The same design as the worker's `THREAD_PERMITS`
//! (`apps/pleochroma-worker/src/render_core/mod.rs`): a counting semaphore sized to
//! `available_parallelism`, released by an RAII guard so a panicking tracer never leaks
//! permits. A lane asks for its fair share, `cores / lanes_in_the_fallback`, and the renderer
//! spawns exactly as many threads as it was granted, so lanes share the pool instead of
//! queueing behind one another. Per-row results are independent of which thread claims a
//! row, so the output is bit-identical.

use std::{
    sync::{
        Condvar, LazyLock, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

/// A counting semaphore over CPU tracer threads.
struct CpuPermits {
    /// Total capacity, fixed at construction.
    capacity: usize,
    /// Permits currently unclaimed, guarded by the mutex the condvar waits on.
    available: Mutex<usize>,
    /// Notified whenever permits come back, so a blocked acquire re-checks.
    freed: Condvar,
    /// Lanes currently inside (or waiting for) a fallback render; the divisor of the
    /// fair share.
    lanes: AtomicUsize,
}

impl CpuPermits {
    fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            available: Mutex::new(capacity),
            freed: Condvar::new(),
            lanes: AtomicUsize::new(0),
        }
    }

    /// Blocks until at least one permit is free, then claims up to `wanted` (clamped to
    /// `1..=capacity`) of what is free; returns how many were claimed.
    fn acquire(&self, wanted: usize) -> usize {
        let wanted = wanted.clamp(1, self.capacity);
        let mut available = self
            .available
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        while *available == 0 {
            available = self
                .freed
                .wait(available)
                .unwrap_or_else(PoisonError::into_inner);
        }
        let taken = wanted.min(*available);
        *available -= taken;
        drop(available);
        taken
    }

    /// Returns `held` permits and wakes every waiter.
    fn release(&self, held: usize) {
        let mut available = self
            .available
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *available += held;
        drop(available);
        self.freed.notify_all();
    }
}

/// The pool, sized once to this machine's `available_parallelism`.
static CPU_PERMITS: LazyLock<CpuPermits> = LazyLock::new(|| {
    CpuPermits::new(thread::available_parallelism().map_or(8, std::num::NonZero::get))
});

/// RAII guard returning its permits to the pool on drop, including on an unwind.
pub(super) struct CpuPermitGuard(usize);

impl CpuPermitGuard {
    /// How many tracer threads the holder may run.
    pub(super) const fn threads(&self) -> usize {
        self.0
    }
}

impl Drop for CpuPermitGuard {
    fn drop(&mut self) {
        CPU_PERMITS.release(self.0);
        CPU_PERMITS.lanes.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Registers this lane, then blocks until at least one permit is free and claims up to
/// `min(cores, max(1, cores / lanes))` of them.
///
/// Called around one CPU-fallback render. The grant shrinks as lanes arrive and grows as
/// they leave: one active lane gets every core, and 15 lanes on 16 cores get one thread each.
pub(super) fn acquire_share() -> CpuPermitGuard {
    let lanes = CPU_PERMITS.lanes.fetch_add(1, Ordering::AcqRel) + 1;
    let share = (CPU_PERMITS.capacity / lanes).max(1);
    CpuPermitGuard(CPU_PERMITS.acquire(share))
}
