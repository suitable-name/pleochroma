//! Atomic service counters and the first failure; timings include host processing.
#[cfg(feature = "cuda")]
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

/// Cumulative optical workload and approximate current worker queue state.
///
/// Counters are sampled independently without synchronizing CUDA; they are not an atomic
/// snapshot of one instant and do not measure GPU utilization.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct OpticalDiagnostics {
    /// Device properties and calculated limits captured when the service was created.
    pub capacity: super::OpticalCapacity,
    /// Successfully submitted optical, reduction and tone kernels, even if they later fail.
    pub launches: u64,
    /// Submitted optical ray lanes, including misses, aperture samples and optical modes.
    pub rays: u64,
    /// Path-by-wavelength attenuation evaluations, excluding Gaussian band evaluations.
    pub spectral_exponentials: u64,
    /// Requests finished by the worker, including processed errors and cancellations.
    pub completed_requests: u64,
    /// Fast batches started by the worker, including batches that subsequently fail.
    pub fast_batches: u64,
    /// Largest Fast batch started so far, measured in candidate requests.
    pub max_fast_batch_candidates: usize,
    /// Mean candidates per started Fast batch, or zero before the first batch.
    pub mean_fast_batch_candidates: f64,
    /// Approximate waiting requests, including staged batches and transient admission attempts.
    pub queued_requests: usize,
    /// Requests being processed by the worker, including host work and CUDA waits.
    pub active_requests: usize,
    /// Sum of processed request wait times from admission to service, including batch collection.
    /// Requests cancelled before service are excluded.
    pub queue_wait_micros: u64,
    /// Worker wall time spent collecting already queued Fast requests.
    pub batch_collection_micros: u64,
    /// Cumulative worker service time in microseconds, excluding queue wait and batch collection.
    ///
    /// Includes host processing and CUDA waits; this is not measured GPU kernel time.
    pub service_wall_micros: u64,
    /// First fatal worker or execution error, retained for the lifetime of the service.
    pub first_error: Option<String>,
}

#[derive(Default)]
#[cfg(feature = "cuda")]
pub(super) struct Counters {
    pub capacity: OnceLock<super::OpticalCapacity>,
    pub launches: AtomicU64,
    pub rays: AtomicU64,
    pub exponentials: AtomicU64,
    pub completed: AtomicU64,
    pub batches: AtomicU64,
    pub batch_candidates: AtomicU64,
    pub max_batch: AtomicUsize,
    pub queued: AtomicUsize,
    pub active: AtomicUsize,
    pub service_micros: AtomicU64,
    pub queue_micros: AtomicU64,
    pub collection_micros: AtomicU64,
    pub(super) error: Arc<Mutex<Option<String>>>,
}
#[cfg(feature = "cuda")]
impl Counters {
    pub fn dequeue(&self, count: usize) {
        let _ = self
            .queued
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |queued| {
                Some(queued.saturating_sub(count))
            });
    }
    pub fn error(&self) -> Option<String> {
        self.error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    pub fn fail(&self, error: String) {
        self.error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert(error);
    }
    pub fn snapshot(&self) -> OpticalDiagnostics {
        let batches = self.batches.load(Ordering::Relaxed);
        OpticalDiagnostics {
            capacity: self.capacity.get().cloned().unwrap_or_default(),
            launches: self.launches.load(Ordering::Relaxed),
            rays: self.rays.load(Ordering::Relaxed),
            spectral_exponentials: self.exponentials.load(Ordering::Relaxed),
            completed_requests: self.completed.load(Ordering::Relaxed),
            fast_batches: batches,
            max_fast_batch_candidates: self.max_batch.load(Ordering::Relaxed),
            mean_fast_batch_candidates: if batches == 0 {
                0.0
            } else {
                self.batch_candidates.load(Ordering::Relaxed) as f64 / batches as f64
            },
            queued_requests: self.queued.load(Ordering::Relaxed),
            active_requests: self.active.load(Ordering::Relaxed),
            queue_wait_micros: self.queue_micros.load(Ordering::Relaxed),
            batch_collection_micros: self.collection_micros.load(Ordering::Relaxed),
            service_wall_micros: self.service_micros.load(Ordering::Relaxed),
            first_error: self.error(),
        }
    }
}
