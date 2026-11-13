//! Progressive streaming: the emitter/tracer split described in `serve`'s module
//! docs.
//!
//! This module holds every piece that's testable as pure logic, independent of any
//! actual socket: [`PendingDelta`] (delta coalescing), [`next_batch_size`] (adaptive
//! sub-batch sizing), and [`downsample_preview`] (the cumulative reduced-resolution
//! snapshot `PREVIEW` sends). [`run_stream`] wires these together with a tracer thread
//! and drives the actual `Read + Write` stream; see its own doc comment and `serve`'s
//! module docs for the full architecture.

use std::{
    io::{Read, Write},
    time::Duration,
};

mod contribution;
mod downsample;
mod emitter;
mod producer;
mod sizing;
#[cfg(test)]
mod tests;
mod tracer;

pub use contribution::{Awaited, ContributionSlot, DEFAULT_CONTRIBUTION_WAIT};
pub use emitter::{RawPoll, poll_raw_client_message, run_stream, run_stream_with};
pub use producer::{Output, ProducerOutcome, ProducerSink, local_tracer};
pub use tracer::trace_range;

/// What [`run_stream_with`] streams: the request, the negotiated payload encoding, what
/// the request becomes at the end ([`Output`]), and the viewer's reserved share of a final
/// picture, if any.
#[derive(Debug, Clone, Copy)]
pub struct StreamSpec<'a> {
    /// The request being streamed (a `FinalImageRequest` is carried as an equivalent
    /// `FinalOnly` request with no preview; see [`Output::FinalImage`]).
    pub request: &'a pleochroma_net::messages::RenderRequest,
    /// The encoding negotiated in `WELCOME`: what a request without a [`Self::link`] sends
    /// for every `FRAME`/`PREVIEW` (and, derived from it, every `DISPLAY_FRAME`).
    pub payload_encoding: pleochroma_net::messages::PayloadEncoding,
    /// The connection's adaptive-compression state (one per peer connection, shared by
    /// every request on it): chooses the encoding of each frame from the measured write
    /// speed and times each frame's blocking write. `None` pins every frame to
    /// [`Self::payload_encoding`], unmeasured.
    pub link: Option<&'a std::sync::Arc<pleochroma_net::messages::adaptive::PeerLink>>,
    /// Radiance or a final picture.
    pub output: Output,
    /// The viewer's reserved tail of a `FinalImageRequest`, if it asked to
    /// contribute one -- `None` for every `RENDER` and for a `FinalImageRequest` with
    /// `viewer_samples == 0`. The emitter loop routes an incoming `CONTRIBUTION` here
    /// (see `emitter::run_stream_loop`); the coordinator job's producer thread waits on
    /// it once its own lanes finish (see `crate::coordinator::job::producer`).
    pub contribution: Option<&'a ContributionSlot>,
    /// How long the producer may go without adding a sample before the emitter fails the
    /// request with `error_codes::PRODUCER_STALLED` (see `emitter::run_stream_loop`).
    ///
    /// `None` for a coordinator job, which legitimately waits in its viewer's FIFO or for
    /// a joined worker with no sample landing; `Some` for a request served directly by
    /// the local tracer (a plain worker, or a coordinator's own lane), where a producer
    /// that stops advancing is wedged -- and the emitter's own `PROGRESS` heartbeat would
    /// otherwise keep the client's silence-based liveness deadline satisfied forever.
    pub stall_timeout: Option<Duration>,
}

/// The stall window a directly served request gets: `StreamSpec::stall_timeout` for a
/// plain worker and a coordinator's own-lane route.
///
/// Generous on purpose. The first sample lands only after the tracer's calibration probe
/// and, on a cold GPU, a pipeline compile, and a large sub-batch is sized to about a second
/// of work. Two minutes is well past either, yet short enough that a wedged lane does not
/// hold a batch client's slot for long.
pub const PRODUCER_STALL_TIMEOUT: Duration = Duration::from_secs(120);

/// This worker's cadence floor, advertised in `WELCOME::min_cadence_ms`.
///
/// It matches [`TARGET_SUBBATCH`]: emitting faster than one sub-batch completes is not
/// meaningful, since there is nothing new to send in between.
pub const MIN_CADENCE_FLOOR_MS: u32 = 100;

/// How long `emitter::poll_for_client_message` and `crate::serve::tilt::poll_for_cancel`
/// wait for the remainder of a message once its first byte has arrived.
///
/// Shared so the two implementations of this pattern cannot drift to different bounds.
///
/// Both read a message's length-prefix first byte with a raw, timeout-tolerant `read()`
/// so a timeout can only land BEFORE any byte has arrived. Once a byte arrives, the
/// remainder gets this bounded window instead of `None`, so a peer that sends a partial
/// frame and then goes silent hits a protocol error (tearing down the connection)
/// instead of hanging the reading thread forever.
///
/// 5s is generous relative to both functions' millisecond-scale first-byte poll
/// interval: a real message (tiny -- a `CANCEL` or pipelined `RenderRequest`) completes
/// well under a second even on a slow link, leaving slack for congestion without
/// leaving a stalled connection blocking indefinitely.
pub const FRAME_REMAINDER_TIMEOUT: Duration = Duration::from_secs(5);

/// Hard ceiling, independent of `cadence_ms`, on the gap between two `StreamEvent`s while a
/// request streams.
///
/// A bare `Progress` heartbeat fills the gap when nothing else is due.
///
/// The editor's remote-render connection declares a worker dead after its own
/// `LIVENESS_TIMEOUT` (8s) with no `StreamEvent` received (30s grace before the first
/// event); this is a quarter of that, leaving slack for one lost heartbeat without the
/// next also missing the deadline.
///
/// Can't just be `request.stream.cadence_ms`: that's client-chosen with only a floor
/// ([`MIN_CADENCE_FLOOR_MS`]), and a large export legitimately requests a large cadence
/// for fewer, bigger payloads that could otherwise go silent past `LIVENESS_TIMEOUT`.
/// The emitter loop wakes at least this often unconditionally and heartbeats with the
/// latest `samples_done` whenever nothing else went out; a due cadence tick is never
/// delayed by this.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);

/// True if `e` is a stream-level timeout on this crate's transports (a raw `TcpStream`, or
/// TLS via `rustls::StreamOwned`).
///
/// Both `emitter::poll_for_client_message` and `crate::serve::tilt::poll_for_cancel` use
/// this to tell "nothing pending yet" from a genuine protocol error.
///
/// A raw socket's timeout elapsing surfaces as [`std::io::ErrorKind::WouldBlock`] or
/// [`std::io::ErrorKind::TimedOut`] depending on platform.
/// [`std::io::ErrorKind::WriteZero`] is a third, non-obvious member once TLS is
/// involved: `rustls`'s `Stream::read`/`write` drive `complete_io()` internally, and a
/// single call can require both a socket read and write under the hood. When that
/// inner I/O times out, rustls reports `WriteZero` instead of propagating the original
/// `WouldBlock`/`TimedOut` -- without treating it the same, a merely-slow-but-alive
/// peer's poll could be misclassified as a fatal protocol error.
#[must_use]
pub fn is_stream_timeout(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::WriteZero
    )
}

/// A stream that can have a short read timeout applied, so [`run_stream`]'s emitter loop
/// can poll for an incoming `CANCEL` without ever blocking longer than the timeout.
///
/// Implemented for the real transports (`TcpStream`, and `rustls::StreamOwned` wrapping
/// one) by delegating to the underlying socket. A test double can implement this to
/// toggle its own "exhausted input means `WouldBlock`, not EOF" behavior -- see
/// `serve::tests::fixtures::DuplexHalf`.
pub trait TimeoutRead {
    /// # Errors
    ///
    /// Returns whatever the underlying transport's own timeout-setting call returns.
    fn set_read_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()>;
}

impl TimeoutRead for std::net::TcpStream {
    fn set_read_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()> {
        Self::set_read_timeout(self, duration)
    }
}

impl<C, T: TimeoutRead + Read + Write> TimeoutRead for rustls::StreamOwned<C, T> {
    fn set_read_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()> {
        self.sock.set_read_timeout(duration)
    }
}

/// Mirrors `std::io::Read`/`Write`'s blanket impls for `&mut T`, so a test can drive
/// `run_stream` through a `&mut` test double and still inspect it afterward.
impl<T: TimeoutRead + ?Sized> TimeoutRead for &mut T {
    fn set_read_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()> {
        (**self).set_read_timeout(duration)
    }
}

/// Whether [`TimeoutCache`] has applied a read timeout yet, and if so, the last value
/// it applied -- a named alternative to `Option<Option<Duration>>` (flagged by
/// `clippy::option_option`, and less self-explanatory at every call site than a type
/// whose variants say what they mean).
#[derive(Debug, Default, PartialEq, Eq)]
enum LastTimeout {
    /// [`TimeoutCache::apply`] has never been called (or never succeeded) yet -- the
    /// next call always reaches the real `set_read_timeout`, whatever value it's given,
    /// rather than assuming the socket already matches some value.
    #[default]
    Unknown,
    /// The last value successfully applied -- itself `Option<Duration>` (`None` is a
    /// meaningful, cacheable value here: "no timeout").
    Applied(Option<Duration>),
}

/// Caches the last read timeout applied through [`TimeoutCache::apply`], so repeat settings
/// skip the `set_read_timeout` syscall.
///
/// # Why this exists
///
/// `run_stream`'s emitter calls [`emitter::poll_for_client_message`] every
/// [`emitter::EMITTER_POLL`] (20ms), which calls `set_read_timeout` up to twice per
/// call (once for the poll window, once more after the first byte of a message
/// arrives). On an idle streaming connection (no `CANCEL`, no pipelined
/// `RenderRequest` ever arriving) that would be ~100 setsockopt syscalls/s for no
/// behavioral benefit, since the value being set is identical to what's already on the
/// socket. [`Self::apply`] makes the second call a no-op whenever the requested value
/// hasn't changed since the last one this cache actually applied.
///
/// Owned by `run_stream` for the life of one request and passed by `&mut` into
/// `poll_for_client_message`; a fresh cache per request means a new request never
/// trusts an assumption inherited from a previous one about what's currently on the
/// socket -- see [`Self::new`].
#[derive(Debug, Default)]
pub struct TimeoutCache {
    last: LastTimeout,
}

impl TimeoutCache {
    /// A fresh cache with nothing recorded yet -- the next [`Self::apply`] call always
    /// reaches the real `set_read_timeout`, whatever value it's given.
    pub(crate) const fn new() -> Self {
        Self {
            last: LastTimeout::Unknown,
        }
    }

    /// Applies `duration` as `stream`'s read timeout via [`TimeoutRead::set_read_timeout`],
    /// skipping the call entirely if `duration` is exactly what this cache last applied
    /// successfully.
    ///
    /// A failed `set_read_timeout` call does NOT update the cached value: whatever was
    /// last successfully applied is presumably still what the socket actually has, so a
    /// later retry of the same `duration` correctly tries again rather than being
    /// skipped as a false no-op.
    pub(crate) fn apply<S: TimeoutRead + ?Sized>(
        &mut self,
        stream: &mut S,
        duration: Option<Duration>,
    ) -> std::io::Result<()> {
        if self.last == LastTimeout::Applied(duration) {
            return Ok(());
        }
        stream.set_read_timeout(duration)?;
        self.last = LastTimeout::Applied(duration);
        Ok(())
    }
}

/// A stream that can have a write timeout applied.
///
/// It lets [`run_stream`]'s emitter stop blocking on one `write()` after
/// [`emitter::WRITE_TIMEOUT`]. The emitter writes payloads and polls for `CANCEL` in one
/// loop, so a peer that stops reading would block it indefinitely. The timeout ends the
/// connection instead, since a partly written length-prefixed frame cannot be abandoned
/// without desyncing the peer. Implemented the same way as [`TimeoutRead`].
pub trait TimeoutWrite {
    /// # Errors
    ///
    /// Returns whatever the underlying transport's own timeout-setting call returns.
    fn set_write_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()>;
}

impl TimeoutWrite for std::net::TcpStream {
    fn set_write_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()> {
        Self::set_write_timeout(self, duration)
    }
}

impl<C, T: TimeoutWrite + Read + Write> TimeoutWrite for rustls::StreamOwned<C, T> {
    fn set_write_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()> {
        self.sock.set_write_timeout(duration)
    }
}

impl<T: TimeoutWrite + ?Sized> TimeoutWrite for &mut T {
    fn set_write_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()> {
        (**self).set_write_timeout(duration)
    }
}

/// How [`run_stream`] ended, so the caller knows what (if anything) is still to be written.
///
/// For [`Completed`](StreamOutcome::Completed), every `StreamEvent` is already sent (a normal
/// finish and a cancellation both end with a `DONE`). Only [`TracePanicked`] and [`Failed`]
/// need the caller to send `StreamEvent::Error`. [`run_stream`] returns this alongside an
/// `Option<RenderRequest>` for a pipelined request.
///
/// [`TracePanicked`]: StreamOutcome::TracePanicked
/// [`Failed`]: StreamOutcome::Failed
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamOutcome {
    /// The request ran to completion (`DONE { cancelled: false }`) or was cancelled
    /// (`DONE { cancelled: true }`) -- either way, every [`StreamEvent`] this request
    /// will ever produce has already been written.
    Completed,
    /// `pleochroma`'s tracer panicked on this (validation-passing but pathological) scene.
    /// Nothing beyond whatever `FRAME`/`PREVIEW`/`PROGRESS` had already gone out before
    /// the panic has been written -- no `DONE`, since there is no valid outcome to
    /// report. The caller is expected to send `StreamEvent::Error` itself.
    TracePanicked,
    /// The producer could not complete the request (a coordinator that lost every lane:
    /// `error_codes::ALL_WORKERS_LOST`; a producer that made no progress for
    /// [`StreamSpec::stall_timeout`]: `error_codes::PRODUCER_STALLED`), or its final
    /// picture could not be encoded. No `DONE` was written; the caller sends this
    /// `ErrorMsg` as `StreamEvent::Error`.
    Failed(pleochroma_net::messages::ErrorMsg),
}

/// The short label the per-request `info!` lines use: `completed`, `trace panicked` or
/// `failed code N`.
impl std::fmt::Display for StreamOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Completed => f.write_str("completed"),
            Self::TracePanicked => f.write_str("trace panicked"),
            Self::Failed(error) => write!(f, "failed code {}", error.code),
        }
    }
}
