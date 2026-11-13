//! The two connection lifecycles, one-shot and persistent.
//!
//! [`one_shot::spawn_remote_render`] and [`persistent::spawn_remote_connection`] share
//! [`handshake::connect_and_handshake`] and the timeout-tolerant frame readers in
//! [`stream_io`]. The parent module's docs explain why one thread owns the stream.
//!
//! The timeouts and the socket-free decisions built on them ([`liveness_deadline`],
//! [`check_liveness`], [`route_event`]) live here because both lifecycles share them, and
//! so that [`tests`] can run them without a live connection.

mod asset_upload;
mod batch_client;
mod handshake;
mod one_shot;
mod payload_setting;
mod persistent;
mod pool;
mod stream_io;
#[cfg(test)]
mod tests;

pub use batch_client::{
    BatchClient, BatchEvent, BatchWatch, MAX_BATCH_REQUEST_BYTES, items_fitting,
};
pub use handshake::{connect_and_handshake, test_connection};
pub use one_shot::{spawn_final_image_request, spawn_remote_render};
pub use payload_setting::set_payload_choice;
pub use persistent::spawn_remote_connection;
pub use pool::close_idle as close_idle_connections;

use super::types::{CurrentRequest, RemoteError, RemoteUpdate};
use pleochroma_net::{client::ApplyOutcome, messages::StreamEvent};
use std::{
    sync::PoisonError,
    time::{Duration, Instant},
};
use stream_io::to_remote_update;

// Only `tests` (through `use super::*`) needs these; the code of this module does not.
#[cfg(test)]
use std::{
    sync::{Arc, Mutex},
    thread,
};

/// How long a read waits for data before the loops check for a pending command. A trade-off
/// between responsiveness and CPU use, not a protocol requirement.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// # Timeouts and liveness
///
/// Every blocking step has a deadline.
///
/// Without one, a worker that stopped responding after accepting a request (crashed, NAT
/// entry expired, laptop asleep) would leave the connection polling a dead socket forever,
/// with the request still shown as running.
///
/// - [`CONNECT_TIMEOUT`] bounds the TCP connect in `handshake::open_tcp`.
/// - [`HANDSHAKE_TIMEOUT`] bounds the TLS handshake and `HELLO`/`WELCOME` exchange in
///   [`handshake::connect_and_handshake`].
/// - [`WRITE_TIMEOUT`] bounds every write for the connection's life, so a write cannot
///   block forever on a peer that stopped reading.
/// - [`LIVENESS_TIMEOUT`] bounds the silence after a request's first event. It is applied
///   in [`one_shot::run`] and, via [`check_liveness`], in [`persistent::run_connection`].
/// - [`FIRST_EVENT_TIMEOUT`] is the longer wait before a dispatched request's first event,
///   which covers a worker's warm-up at high resolution. See [`liveness_deadline`].
/// - [`FRAME_REMAINDER_TIMEOUT`] bounds the read of the rest of a frame once its first byte
///   has arrived, so a worker that dies mid-frame cannot block the thread in an unbounded
///   read.
///
/// [`KEEPALIVE_TIME`]/[`KEEPALIVE_INTERVAL`] add an OS-level check that notices a dead peer
/// even while nothing is being read.
///
/// Both loops read the socket on every iteration and do little else in between, so the
/// deadlines measure the worker's silence, not local work.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the TLS handshake and the following `HELLO`/`WELCOME` exchange may take. Generous
/// for a slow WAN link, while still bounding a worker that accepts TCP but never completes the
/// handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a single write may block before failing. Set once on the socket by
/// [`handshake::connect_and_handshake`], so it applies to every later write.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// TCP keepalive: idle time before the OS sends the first probe. Paired with
/// [`KEEPALIVE_INTERVAL`] in `handshake::open_tcp`.
const KEEPALIVE_TIME: Duration = Duration::from_secs(30);
/// TCP keepalive: interval between probes after the first, until the peer answers or
/// the OS reports the socket dead.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);

/// How long a request that has already produced an event may go without another before
/// this side gives up on it. The worker's `Progress` heartbeat is time-based and sent at
/// least every `stream_emit::HEARTBEAT_INTERVAL` (2 seconds) in `pleochroma-worker`, so
/// longer silence means the worker or the network path has stopped. The 8 seconds is four
/// heartbeat intervals: room for two lost heartbeats plus jitter.
///
/// This does not bound the worker's cadence-based `FRAME`/`PREVIEW` messages, which can
/// come several heartbeats apart at a coarse cadence.
///
/// Passed through [`one_shot::run`] and [`persistent::run_connection`] as a parameter, so
/// the tests of [`check_liveness`] can use a shorter value.
const LIVENESS_TIMEOUT: Duration = Duration::from_secs(8);

/// How long the wait for the first event of a dispatched request may go, longer than
/// [`LIVENESS_TIMEOUT`]. The worker's calibration and warm-up happen inside this wait, at high
/// resolution and with a coarse cadence, and can legitimately take longer than a steady-state
/// heartbeat gap. A dead worker is still reported within 30 seconds.
const FIRST_EVENT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long [`stream_io`] waits for the rest of a frame (the remaining length-prefix bytes
/// and the payload) once its first byte has arrived. Same bound as the worker's
/// `FRAME_REMAINDER_TIMEOUT` (`apps/pleochroma-worker/src/stream_emit/mod.rs`).
///
/// Without it, a worker that died mid-frame would block this thread in `read_exact` until
/// TCP keepalive fired, far past every other deadline, since those are only checked between
/// reads.
const FRAME_REMAINDER_TIMEOUT: Duration = Duration::from_secs(5);

/// Which deadline currently applies to an idle wait for the next event: the generous
/// [`FIRST_EVENT_TIMEOUT`] before `seen_first_event`, or the tighter, heartbeat-derived
/// `liveness_timeout` once at least one event has arrived.
///
/// `liveness_timeout` is threaded in as a parameter so this pure decision is
/// unit-testable with a short value; production callers always pass
/// [`LIVENESS_TIMEOUT`] itself.
const fn liveness_deadline(seen_first_event: bool, liveness_timeout: Duration) -> Duration {
    if seen_first_event {
        liveness_timeout
    } else {
        FIRST_EVENT_TIMEOUT
    }
}

/// How long a one-shot connection waits, after writing `CANCEL`, for the worker to end
/// the request (`DONE`/`ERROR`) or close the stream before giving up on it. Unlike
/// [`LIVENESS_TIMEOUT`] this is a wall-clock bound from the moment the cancel was sent:
/// a coordinator whose tracer is wedged keeps heartbeating `PROGRESS`, which resets the
/// silence clock forever, so only a deadline that heartbeats cannot move ends the wait.
const CANCEL_ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether a `CANCEL` written at `cancel_sent_at` has gone unanswered for longer than
/// `timeout` as of `now`. `None` (no cancel sent) is never overdue. Pure, with both
/// instants and the timeout passed in, so it is unit-testable without a socket or sleeping.
fn cancel_ack_overdue(cancel_sent_at: Option<Instant>, now: Instant, timeout: Duration) -> bool {
    cancel_sent_at.is_some_and(|sent| now.saturating_duration_since(sent) > timeout)
}

/// Checks whether `current` (if any) has gone longer than `timeout` since `last_event`.
/// If so, delivers [`RemoteUpdate::Failed`] to it and returns `true`, so the caller drops
/// the connection and the next dispatch reconnects. A no-op (`false`) when nothing is
/// current or the deadline has not passed.
///
/// Kept apart from the socket so it can be tested without one: [`super::types::RemoteStream`]
/// cannot be replaced by a fake. The tests pin that a silent request is failed and cleared,
/// and that a request whose clock keeps resetting never is.
fn check_liveness(
    current: &mut Option<CurrentRequest>,
    last_event: Instant,
    timeout: Duration,
) -> bool {
    if current.is_none() {
        return false;
    }
    let elapsed = last_event.elapsed();
    if elapsed <= timeout {
        return false;
    }
    let mut cur = current.take().expect("just checked Some above");
    (cur.on_update)(RemoteUpdate::Failed {
        request_id: cur.request_id,
        message: RemoteError::WorkerSilent(elapsed).to_string(),
    });
    true
}

/// Applies one [`StreamEvent`] to the request in flight on a persistent connection. Like
/// [`one_shot::run`]'s loop body: apply to the accumulator, translate with
/// [`stream_io::to_remote_update`], and clear `current` on a terminal outcome. An event
/// arriving while `current` is `None` (a reply to a request already given up) is dropped.
///
/// The request id is not compared here. `Accumulator::apply` checks the epoch, so a reply
/// to a superseded request never reaches the new accumulator, even when its best-effort
/// `CANCEL` arrived too late to stop it.
fn route_event(current: &mut Option<CurrentRequest>, event: &StreamEvent, payload: Option<&[u8]>) {
    let Some(cur) = current.as_mut() else {
        return;
    };

    let outcome = {
        let mut acc = cur
            .accumulator
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        acc.apply(event, payload)
    };
    let request_id = cur.request_id;
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(e) => {
            (cur.on_update)(RemoteUpdate::Failed {
                request_id,
                message: RemoteError::Client(e.into()).to_string(),
            });
            *current = None;
            return;
        }
    };

    let is_terminal = matches!(
        outcome,
        ApplyOutcome::Done { .. } | ApplyOutcome::WorkerError
    );
    if let Some(update) = to_remote_update(request_id, event, outcome) {
        (cur.on_update)(update);
    }
    if is_terminal {
        *current = None;
    }
}
