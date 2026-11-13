//! [`run_remote_batch`]: dispatching one `RenderRequest` to a remote worker, with the
//! export-side liveness and cancellation timeouts.
//!
//! # Timeouts and liveness
//!
//! [`run_remote_batch`] polls its channel with the same two-tier deadline the connection thread
//! applies to the socket. The lane does nothing else while it waits, so `last_update` is always
//! fresh. The first wait gets the longer [`FIRST_EVENT_TIMEOUT`] because a coarse worker cadence
//! can make it slower than later waits.

use super::super::{capability::RemoteCapability, rate::remote_request_rate};
use crate::bridge::remote::remote_render::{self, RemoteRenderRequest, RemoteUpdate};
use pleochroma_net::{SceneState, client::Accumulator};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    time::{Duration, Instant},
};

/// The next request id. Consecutive chunks reuse one parked socket, so ids must differ to tell
/// a late reply to the previous chunk from the current one.
static NEXT_REQUEST_ID: AtomicU32 = AtomicU32::new(1);

/// Idle limit once a dispatch has seen an update. Without it a worker that accepted a request
/// and went silent would hang the whole remote lane. Kept separate from the connection's
/// constant of the same value because this is a one-shot dispatch.
///
/// Applies after the first [`RemoteUpdate`]; see [`FIRST_EVENT_TIMEOUT`] before it.
pub(super) const LIVENESS_TIMEOUT: Duration = Duration::from_secs(8);

/// Idle limit before the first [`RemoteUpdate`] of a dispatch.
///
/// # Why the first wait needs a longer grace
///
/// A calibration probe at 4K can legitimately take longer than [`LIVENESS_TIMEOUT`] to report
/// while still computing, so it must not be taken for a silent worker.
pub(super) const FIRST_EVENT_TIMEOUT: Duration = Duration::from_secs(30);

/// The slowest link assumed when budgeting one full-resolution `FRAME`: 4 MiB/s (~34 Mbit/s).
/// It only sizes the allowance; a dead worker is still detected, at most `frame_bytes / this`
/// later.
const MIN_ASSUMED_LINK_BYTES_PER_SEC: u64 = 4 * 1024 * 1024;

/// Extra wait allowed while one `frame_bytes` payload is in flight.
///
/// The connection reports a `FRAME` only after the whole payload is read, so nothing reaches
/// the channel during a large transfer. Without this allowance a slow 4K transfer would look
/// like a silent worker.
pub(super) const fn transfer_allowance(frame_bytes: u64) -> Duration {
    Duration::from_secs(frame_bytes.div_ceil(MIN_ASSUMED_LINK_BYTES_PER_SEC))
}

/// The idle deadline for a wait on `rx`: the base deadline plus the [`transfer_allowance`] for
/// one full-resolution `FRAME` of `frame_bytes`.
pub(in crate::bridge::export_thread) const fn liveness_deadline(
    seen_first_update: bool,
    frame_bytes: u64,
) -> Duration {
    let base = if seen_first_update {
        LIVENESS_TIMEOUT
    } else {
        FIRST_EVENT_TIMEOUT
    };
    base.saturating_add(transfer_allowance(frame_bytes))
}

/// How long to wait for the worker's `DONE { cancelled: true }` after a cancel is sent. A
/// worker that never confirms must not hang the export.
pub(in crate::bridge::export_thread) const CANCEL_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// The first and latest progress reports of one dispatch, which bound a plain worker's rate
/// (see [`remote_request_rate`]).
#[derive(Debug, Default)]
pub(super) struct ProgressSpan {
    /// When the first report with `samples_done > 0` arrived, and what it said.
    first: Option<(Instant, u32)>,
    /// When the latest report that ADVANCED `samples_done` arrived, and what it said.
    last: Option<(Instant, u32)>,
}

impl ProgressSpan {
    /// Records a `Frame`/`Progress` report of `samples_done` that arrived at `at`. A
    /// report that does not raise `samples_done` above the latest one recorded (a repeat,
    /// or a heartbeat with nothing new) changes nothing.
    pub(super) fn note(&mut self, at: Instant, samples_done: u32) {
        let latest = self.last.map_or(0, |(_, samples)| samples);
        if samples_done <= latest {
            return;
        }
        if self.first.is_none() {
            self.first = Some((at, samples_done));
        }
        self.last = Some((at, samples_done));
    }

    /// The samples gained and the time elapsed between the first and the latest
    /// recorded report; `None` until two reports with different `samples_done` have
    /// been seen.
    pub(super) fn advance(&self) -> Option<(u32, Duration)> {
        let (first_at, first_samples) = self.first?;
        let (last_at, last_samples) = self.last?;
        if last_samples <= first_samples {
            return None;
        }
        Some((
            last_samples - first_samples,
            last_at.saturating_duration_since(first_at),
        ))
    }
}

/// Runs one `RenderRequest` against `capability.worker` for samples `[first_sample,
/// first_sample + samples)` at `width x height`, blocking until it finishes, fails, or `cancel`
/// is observed. On cancel it sends the cancel and waits for the worker's confirmation.
///
/// `accumulator` is shared so the caller can read its progress from another thread while this
/// call runs.
///
/// Returns `(samples_done, cancelled, error, rate)`. `samples_done` is the valid prefix the
/// accumulator holds, however this ended. `error` is set only for an outright failure.
///
/// `rate` is the worker's measured throughput (see `super::super::rate::remote_request_rate`).
/// A coordinator is measured over the whole request, setup included. A plain worker is measured
/// between its first and last progress reports, or over the whole request if it reported once.
/// `None` when there was no `DONE` or nothing was traced.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is a distinct piece of one RenderRequest's own identity \
              (worker capability, scene, sample range, resolution, the shared \
              accumulator, cancellation) -- bundling them into a struct would just move \
              the same count into field access, not reduce it, and `RemoteRenderRequest` \
              already plays that role one level down in `bridge::remote::remote_render`"
)]
pub(in crate::bridge::export_thread) fn run_remote_batch(
    capability: &RemoteCapability,
    scene: SceneState,
    first_sample: u32,
    samples: u32,
    width: u32,
    height: u32,
    accumulator: &Arc<Mutex<Accumulator>>,
    cancel: &AtomicBool,
) -> (u32, bool, Option<String>, Option<f64>) {
    let (tx, rx) = mpsc::channel::<RemoteUpdate>();
    // Start of the whole-request span that the coordinator rate is measured over.
    let request_sent_at = Instant::now();
    let handle = remote_render::spawn_remote_render(
        RemoteRenderRequest {
            worker: capability.worker.clone(),
            request_id: NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed),
            scene,
            first_sample,
            samples,
            width,
            height,
            // Exports and tilt videos (the only callers) are throughput work.
            intent: pleochroma_net::messages::RequestIntent::Batch,
            display_only: false,
        },
        Arc::clone(accumulator),
        move |update| {
            let _ = tx.send(update);
        },
    );

    let mut span = ProgressSpan::default();
    let mut cancel_sent = false;
    // Set when the cancel is sent; idle polls then use `CANCEL_WAIT_TIMEOUT`.
    let mut cancel_sent_at: Option<Instant> = None;
    let mut last_update = Instant::now();
    // Selects `FIRST_EVENT_TIMEOUT` until the first update arrives.
    let mut first_update_seen = false;
    // Size of one full-resolution `FRAME` at this request's dimensions.
    let frame_bytes =
        u64::from(width) * u64::from(height) * pleochroma_net::radiance::BYTES_PER_PIXEL as u64;
    loop {
        if !cancel_sent && cancel.load(Ordering::Relaxed) {
            handle.cancel();
            cancel_sent = true;
            cancel_sent_at = Some(Instant::now());
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(update) => {
                let received_at = Instant::now();
                last_update = received_at;
                first_update_seen = true;
                match update {
                    RemoteUpdate::Done { cancelled, .. } => {
                        let samples_done = accumulator
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .samples_done();
                        let rate = remote_request_rate(
                            capability.coordinator,
                            samples_done,
                            received_at.saturating_duration_since(request_sent_at),
                            span.advance(),
                        );
                        return (samples_done, cancelled, None, rate);
                    }
                    RemoteUpdate::Failed { message, .. }
                    | RemoteUpdate::Unsupported { message, .. } => {
                        let acc = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
                        return (acc.samples_done(), false, Some(message), None);
                    }
                    RemoteUpdate::Frame { samples_done, .. }
                    | RemoteUpdate::Progress { samples_done, .. } => {
                        span.note(received_at, samples_done);
                    }
                    // Not terminal. The accumulator already holds every applied `Frame`.
                    _ => {}
                }
            }
            // Nothing arrived in this poll.
            Err(RecvTimeoutError::Timeout) => match cancel_sent_at {
                Some(sent_at) if sent_at.elapsed() > CANCEL_WAIT_TIMEOUT => {
                    // No confirmation: report the cancel with the samples completed so far.
                    tracing::warn!(
                        "remote render: worker never confirmed cancellation within \
                         {CANCEL_WAIT_TIMEOUT:?} -- returning with samples completed so \
                         far"
                    );
                    let acc = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
                    return (acc.samples_done(), true, None, None);
                }
                None if last_update.elapsed()
                    > liveness_deadline(first_update_seen, frame_bytes) =>
                {
                    // Silent past the applicable deadline with no cancel: presume it dead.
                    let message = format!(
                        "worker silent for {:.0?} -- treating as failed",
                        last_update.elapsed()
                    );
                    tracing::warn!("remote render: {message}");
                    let acc = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
                    return (acc.samples_done(), false, Some(message), None);
                }
                // Still within the deadline: keep polling.
                Some(_) | None => {}
            },
            Err(RecvTimeoutError::Disconnected) => {
                // The worker thread ended without a terminal update, likely from a panic.
                let acc = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
                return (
                    acc.samples_done(),
                    false,
                    Some("remote render worker thread ended unexpectedly".to_string()),
                    None,
                );
            }
        }
    }
}
