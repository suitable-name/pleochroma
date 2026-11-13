//! [`RemoteProgress`]: the cross-thread state a running remote lane publishes for
//! `run_export`'s progress-reporting closure to read while the lane is still running.

use glam::Vec3;
use pleochroma_net::client::Accumulator;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU32, Ordering},
    },
};

/// Cross-thread state that [`super::lane::run_remote_lane`] publishes for `run_export`'s
/// progress closure. The lane runs many chunk dispatches, each with its own `Accumulator`.
///
/// Fields use interior mutability, so the lane thread and the local thread share one
/// `&RemoteProgress` inside the `thread::scope` of `run_export`.
pub(in crate::bridge::export_thread) struct RemoteProgress {
    /// Sum of every fully merged chunk. Folded into the export's `accum` only once, after the
    /// concurrent phase, because local's CPU threads write the same pixels.
    pub(super) accum: Mutex<Vec<Vec3>>,
    /// Samples folded into `accum`, so progress is one atomic load per tick.
    pub(super) traced: AtomicU32,
    /// The chunk in flight, `Some` only during a `super::run_batch::run_remote_batch` call.
    /// Gives sub-chunk progress between chunk completions.
    pub(super) in_flight: Mutex<Option<Arc<Mutex<Accumulator>>>>,
    /// Notes raised mid-export, queued because several can occur. The progress closure drains
    /// one per tick via [`take_note`](Self::take_note).
    pub(super) notes: Mutex<VecDeque<String>>,
}

impl RemoteProgress {
    pub(in crate::bridge::export_thread) fn new(pixel_count: usize) -> Self {
        Self {
            accum: Mutex::new(vec![Vec3::ZERO; pixel_count]),
            traced: AtomicU32::new(0),
            in_flight: Mutex::new(None),
            notes: Mutex::new(VecDeque::new()),
        }
    }

    /// Samples traced so far: merged chunks plus what the in-flight chunk has streamed back.
    /// May under-report by one tick during a chunk hand-over, never over-report, because
    /// `run_remote_lane` clears `in_flight` before it adds to `traced`.
    pub(in crate::bridge::export_thread) fn samples_done(&self) -> u32 {
        let in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let live = in_flight.as_ref().map_or(0, |acc| {
            acc.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .samples_done()
        });
        drop(in_flight);
        self.traced.load(Ordering::Relaxed) + live
    }

    /// Live preview buffer: merged chunks plus the in-flight chunk, pixel-summed. Always one
    /// full pixel buffer; all zero before any remote contribution.
    pub(in crate::bridge::export_thread) fn preview_buffer(&self) -> Vec<Vec3> {
        let mut buf = self
            .accum
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(acc) = in_flight.as_ref() {
            let acc = acc.lock().unwrap_or_else(PoisonError::into_inner);
            for (dst, src) in buf.iter_mut().zip(acc.buffer()) {
                *dst += *src;
            }
        }
        drop(in_flight);
        buf
    }

    /// Pops the oldest queued note, if any. Called once per progress tick.
    pub(in crate::bridge::export_thread) fn take_note(&self) -> Option<String> {
        self.notes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }

    /// Consumes `self` and returns all fully merged samples, to fold into the export's `accum`
    /// once, after the concurrent phase.
    pub(in crate::bridge::export_thread) fn into_buffer(self) -> Vec<Vec3> {
        self.accum
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
    }
}
