//! [`run_remote_lane`]: the remote lane's own claim/dispatch/merge loop for the whole
//! concurrent phase, plus its consecutive-failure backoff.

use super::{
    super::{
        capability::RemoteCapability,
        rate::{remote_request_samples, shortfall},
    },
    progress::RemoteProgress,
    run_batch::run_remote_batch,
};
use crate::bridge::export_thread::sample_cursor::SampleCursor;
use pleochroma_net::{SceneState, client::Accumulator};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Consecutive failed chunks before [`run_remote_lane`] pauses remote (`Both` only).
///
/// A single dropped connection is a transient blip; pausing on it would waste remote capacity.
pub(super) const MAX_CONSECUTIVE_REMOTE_FAILURES: u32 = 2;

/// The first pause once remote has failed [`MAX_CONSECUTIVE_REMOTE_FAILURES`] chunks in a row.
/// Each further consecutive failure doubles it, up to [`REMOTE_RETRY_BACKOFF_MAX`].
pub(super) const REMOTE_RETRY_BACKOFF_INITIAL: Duration = Duration::from_secs(15);
pub(super) const REMOTE_RETRY_BACKOFF_MAX: Duration = Duration::from_secs(120);

/// Pause length after `consecutive_failures` failed chunks in a row, once that count has
/// reached [`MAX_CONSECUTIVE_REMOTE_FAILURES`]: 15 s, 30 s, 60 s, then 120 s.
///
/// Remote is never written off for the rest of an export; it is offered work again when the
/// pause ends. Local keeps claiming during the pause, so only remote's share is lost.
pub(super) fn remote_retry_backoff(consecutive_failures: u32) -> Duration {
    let doublings = consecutive_failures.saturating_sub(MAX_CONSECUTIVE_REMOTE_FAILURES);
    let scaled = REMOTE_RETRY_BACKOFF_INITIAL.saturating_mul(1u32 << doublings.min(8));
    scaled.min(REMOTE_RETRY_BACKOFF_MAX)
}

/// Sleeps for `total`, returning early with `false` the moment `cancel` is raised or
/// `cursor`'s shared pool runs dry (local has claimed everything that was left -- no
/// point holding the export open for a pause with nothing to hand remote afterwards).
/// `true` means the full pause elapsed with work still available.
pub(super) fn pause_remote_lane(
    cursor: &SampleCursor,
    cancel: &AtomicBool,
    total: Duration,
) -> bool {
    let started = Instant::now();
    while started.elapsed() < total {
        if cancel.load(Ordering::Relaxed) || cursor.shared_pool_exhausted() {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
    true
}

/// [`run_remote_lane`]'s result once its claim loop has permanently ended.
pub(in crate::bridge::export_thread) struct RemoteLaneOutcome {
    /// `Some` iff the lane ended in a state that must fail the WHOLE export outright
    /// -- only possible when `fallback_to_local` was `false` (`ComputeTarget::RemoteOnly`)
    /// and a chunk came back short: no local lane is running to pick up the unfinished
    /// remainder, so finishing anyway would produce a silently incomplete image.
    /// `run_export` turns this into `ExportOutcome::Failed` rather than writing a
    /// partial image.
    pub(in crate::bridge::export_thread) fatal: Option<String>,
    /// The lane's best throughput estimate (samples/sec) when its claim loop ended. A
    /// multi-frame render passes it on as `AccumulationCarry::remote_rate`, so the next
    /// frame's lane starts from it instead of calibrating again.
    pub(in crate::bridge::export_thread) final_rate: Option<f64>,
}

/// Runs the remote lane for the whole concurrent phase: claims a chunk sized from the current
/// rate estimate (see [`remote_request_samples`]), dispatches it via [`run_remote_batch`], merges
/// the completed prefix into `progress`, and repeats until the cursor is empty, the export is
/// cancelled, remote fails too often in a row (`Both`), or a failed chunk is unrecoverable
/// (`RemoteOnly`).
///
/// # Why every chunk gets its own `Accumulator`
///
/// `Accumulator::begin_request` zeroes the buffer on every dispatch, so reusing one would
/// discard the radiance of earlier chunks. Each chunk's result is summed into `progress` once,
/// when the chunk ends.
///
/// # `fallback_to_local` and `remote_lane_done`
///
/// `fallback_to_local` is `true` only for `Both`. It decides whether a short chunk's remainder
/// goes back to local through `cursor.return_to_local`, or fails the export (`RemoteOnly`).
///
/// `remote_lane_done` is stored last, after every `return_to_local` call, because
/// `batch::run_local_batches` relies on that order to stop waiting for retries.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is a distinct piece of one remote lane's own identity \
              (the shared cursor, worker capability, scene, resolution, the initial \
              rate estimate, the cross-thread progress/notes state, whether a failed \
              chunk may fall back to local, and cancellation/completion signalling) -- \
              bundling them into a struct would just move the same count into field \
              access, not reduce it"
)]
pub(in crate::bridge::export_thread) fn run_remote_lane(
    cursor: &SampleCursor,
    capability: &RemoteCapability,
    scene_state: &SceneState,
    width: u32,
    height: u32,
    initial_rate: f64,
    progress: &RemoteProgress,
    fallback_to_local: bool,
    cancel: &AtomicBool,
    remote_lane_done: &AtomicBool,
) -> RemoteLaneOutcome {
    let outcome = run_remote_lane_claim_loop(
        cursor,
        capability,
        scene_state,
        width,
        height,
        initial_rate,
        progress,
        fallback_to_local,
        cancel,
    );
    // See "`fallback_to_local` and `remote_lane_done`" above for why this must be the
    // LAST thing this function does.
    remote_lane_done.store(true, Ordering::Release);
    outcome
}

/// Folds one finished chunk's radiance into `progress`'s merged buffer and adds its
/// `done` samples to the traced count. A chunk with `done == 0` contributes nothing
/// and is skipped; `done` is always the valid prefix `run_remote_batch` reports.
fn merge_chunk_into_progress(
    progress: &RemoteProgress,
    chunk_accumulator: &Mutex<Accumulator>,
    done: u32,
) {
    if done == 0 {
        return;
    }
    let chunk = chunk_accumulator
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut merged = progress
        .accum
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    for (dst, src) in merged.iter_mut().zip(chunk.buffer()) {
        *dst += *src;
    }
    drop(merged);
    drop(chunk);
    progress.traced.fetch_add(done, Ordering::Relaxed);
}

#[expect(
    clippy::too_many_arguments,
    reason = "see `run_remote_lane`'s own identical `#[expect]` -- this is that \
              function's claim loop, split out only so the `remote_lane_done` \
              store-on-every-exit-path guarantee lives in exactly one place (the \
              wrapper) rather than being duplicated at every `return` this loop has"
)]
fn run_remote_lane_claim_loop(
    cursor: &SampleCursor,
    capability: &RemoteCapability,
    scene_state: &SceneState,
    width: u32,
    height: u32,
    initial_rate: f64,
    progress: &RemoteProgress,
    fallback_to_local: bool,
    cancel: &AtomicBool,
) -> RemoteLaneOutcome {
    let mut rate = initial_rate;
    let mut consecutive_failures = 0u32;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return RemoteLaneOutcome {
                fatal: None,
                final_rate: Some(rate),
            };
        }

        let Some((start, count)) =
            cursor.claim(remote_request_samples(rate, capability.coordinator))
        else {
            // Shared pool empty: a normal end. The local-only retry pile is not checked here.
            return RemoteLaneOutcome {
                fatal: None,
                final_rate: Some(rate),
            };
        };

        let chunk_accumulator = Arc::new(Mutex::new(Accumulator::new(width, height)));
        *progress
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&chunk_accumulator));
        let (done, cancelled, error, measured_rate) = run_remote_batch(
            capability,
            scene_state.clone(),
            start,
            count,
            width,
            height,
            &chunk_accumulator,
            cancel,
        );
        *progress
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;

        merge_chunk_into_progress(progress, &chunk_accumulator, done);

        if let Some(measured) = measured_rate {
            rate = measured;
        }

        if cancelled {
            // The export's top-level cancel check discards the result anyway.
            return RemoteLaneOutcome {
                fatal: None,
                final_rate: Some(rate),
            };
        }

        let missing = shortfall(count, done);
        if missing == 0 {
            consecutive_failures = 0;
            continue;
        }

        let message = error.as_deref().unwrap_or("connection ended early");
        if !fallback_to_local {
            // `RemoteOnly`: no local lane can take the remainder; see `RemoteLaneOutcome::fatal`.
            return RemoteLaneOutcome {
                fatal: Some(format!(
                    "Remote worker failed ({message}) -- {missing} of {count} samples in \
                     this chunk were never traced, and this export has no local fallback \
                     (Compute: Remote)."
                )),
                final_rate: Some(rate),
            };
        }

        cursor.return_to_local(start + done, missing);
        progress
            .notes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(format!(
                "Remote worker failed ({message}) partway through a chunk -- the \
                 remaining {missing} samples will finish locally. Every sample it \
                 completed first is still included."
            ));
        consecutive_failures += 1;
        if consecutive_failures >= MAX_CONSECUTIVE_REMOTE_FAILURES {
            let pause = remote_retry_backoff(consecutive_failures);
            progress
                .notes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back(format!(
                    "Remote worker failed {consecutive_failures} chunks in a row -- \
                     pausing it for {}s before offering it more work; rendering \
                     continues locally in the meantime.",
                    pause.as_secs()
                ));
            if !pause_remote_lane(cursor, cancel, pause) {
                // Cancelled, or local took the rest during the pause.
                return RemoteLaneOutcome {
                    fatal: None,
                    final_rate: Some(rate),
                };
            }
        }
    }
}
