//! Beam tiles: [`serve_beam_tiles`] answers one `ClientMessage::BeamTileRequest` on this
//! machine's own lane.
//!
//! # What a request is
//!
//! A set of tiles of a scene's frame (`pleochroma_net::messages::beam_tiles`). The reply is one
//! `StreamEvent::BeamTiles` carrying the finished XYZ and the residual sample count of every pixel
//! of the tiles, or an `ERROR`, or -- after a `CANCEL` -- `DONE { cancelled: true }`. Nothing is
//! streamed: a coordinator merges whole tiles.
//!
//! # Threads
//!
//! Like a live pass: the render runs on a helper thread (`render_core::beam::render_beam_tiles`)
//! while this thread heartbeats `PROGRESS` every `HEARTBEAT_INTERVAL` and watches the socket for a
//! `CANCEL`, which raises the render's cancel token.
//!
//! # What is refused
//!
//! The Ray-Traced engine and an HDR environment (`UNSUPPORTED_REQUEST`), anything that breaks a
//! limit (`VALIDATION_FAILED`, see `validate::validate_beam_tiles`), and a stone or lighting the
//! beams do not take (`UNSUPPORTED_REQUEST` with the `BeamOff` sentence; the coordinator then
//! renders those tiles itself).

use super::{
    live_pass::{write_cancelled, write_error},
    requests::{RequestContext, TRACE_PANIC_CODE, VALIDATION_FAILED_CODE, write_pong},
};
use crate::{
    assets, render_core,
    render_core::beam::BeamTilesOutcome,
    stream_emit::{
        HEARTBEAT_INTERVAL, RawPoll, TimeoutCache, TimeoutRead, TimeoutWrite,
        poll_raw_client_message,
    },
    validate,
};
use pleochroma_net::messages::{
    BeamTileRequest, ClientMessage, NetError, Progress, StreamEvent, beam_tiles_reply, error_codes,
};
use std::{
    io::{Read, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

/// How long one look at the socket for a `CANCEL` may block.
const SOCKET_POLL: Duration = Duration::from_millis(20);

/// Bounds every write of the request: a peer that stops reading ends the connection.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// How the helper thread's render ended.
enum Traced {
    /// The render's outcome.
    Rendered(BeamTilesOutcome),
    /// The renderer panicked on a request that passed validation.
    Panicked,
}

/// Handles one message that arrived while the tiles render; only a `CANCEL` for this request
/// changes anything.
fn absorb<S: Read + Write>(
    stream: &mut S,
    message: ClientMessage,
    request_id: u32,
    cancel: &AtomicBool,
) -> Result<(), NetError> {
    match message {
        ClientMessage::Cancel(cancelled) => {
            if cancelled.request_id == request_id {
                cancel.store(true, Ordering::Relaxed);
            }
            Ok(())
        }
        ClientMessage::Ping { nonce } => write_pong(stream, nonce),
        ClientMessage::ZoningPayload(payload) => {
            crate::serve::zoning::stash(*payload);
            Ok(())
        }
        ClientMessage::Asset(header) => assets::discard_asset(stream, &header),
        ClientMessage::Contribution(header) => {
            pleochroma_net::messages::discard_contribution_payload(stream, &header)
        }
        ClientMessage::Job(job) => crate::jobs::ignore_in_flight(stream, &job),
        other => {
            tracing::debug!(
                "ignoring a message pipelined while beam tiles {request_id} render (beam tiles \
                 have no pipelining): {other:?}"
            );
            Ok(())
        }
    }
}

/// Renders the tiles on a helper thread and keeps the connection alive meanwhile.
///
/// Returns how the render ended, or the transport error that ended the wait (the helper is then
/// cancelled and joined first).
fn render_while_watching<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    request: &BeamTileRequest,
    live: &pleochroma::renderer::adaptive::LiveTiles,
    ctx: &RequestContext<'_>,
    cancel: &AtomicBool,
) -> Result<Traced, NetError> {
    let gpu: &pleochroma::renderer::gpu_backend::GpuBackend = ctx.gpu;
    let request_id = request.request_id;
    thread::scope(|scope| {
        let renderer = scope.spawn(|| {
            catch_unwind(AssertUnwindSafe(|| {
                render_core::beam::render_beam_tiles(gpu, request, live, cancel)
            }))
        });
        let mut timeouts = TimeoutCache::new();
        let mut last_write = Instant::now();
        let mut failure: Option<NetError> = None;
        while !renderer.is_finished() {
            if last_write.elapsed() >= HEARTBEAT_INTERVAL {
                let beat = pleochroma_net::messages::write_stream_event(
                    stream,
                    &StreamEvent::Progress(Progress {
                        request_id,
                        samples_done: 0,
                    }),
                    None,
                );
                if let Err(e) = beat {
                    failure = Some(e);
                    break;
                }
                last_write = Instant::now();
            }
            match poll_raw_client_message(stream, SOCKET_POLL, &mut timeouts) {
                Ok(RawPoll::Pending) => {}
                Ok(RawPoll::Closed) => {
                    cancel.store(true, Ordering::Relaxed);
                    break;
                }
                Ok(RawPoll::Message(message)) => {
                    if let Err(e) = absorb(stream, message, request_id, cancel) {
                        failure = Some(e);
                        break;
                    }
                }
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
        }
        if failure.is_some() {
            cancel.store(true, Ordering::Relaxed);
        }
        let joined = renderer.join();
        if let Some(e) = failure {
            return Err(e);
        }
        Ok(match joined {
            Ok(Ok(outcome)) => Traced::Rendered(outcome),
            Ok(Err(_)) | Err(_) => Traced::Panicked,
        })
    })
}

/// The sentence a `BeamOff` refusal carries (a silent fallback still names itself).
fn off_message(off: pleochroma::renderer::accel_beam::BeamOff) -> String {
    let message = off.message();
    if message.is_empty() {
        "beam tracing is not used for this stone or lighting".to_owned()
    } else {
        message.to_owned()
    }
}

/// Writes the reply of a finished render.
fn write_outcome<S: Write>(
    stream: &mut S,
    request: &BeamTileRequest,
    outcome: BeamTilesOutcome,
) -> Result<(), NetError> {
    let request_id = request.request_id;
    match outcome {
        BeamTilesOutcome::Done(done) => {
            match beam_tiles_reply(request_id, request.sample_offset, done.stats, &done.pixels) {
                Ok((event, payload)) => {
                    pleochroma_net::messages::write_stream_event(stream, &event, Some(&payload))
                }
                Err(e) => write_error(stream, VALIDATION_FAILED_CODE, e.to_string(), request_id),
            }
        }
        BeamTilesOutcome::Cancelled => write_cancelled(stream, request_id),
        BeamTilesOutcome::Refused(off) => write_error(
            stream,
            error_codes::UNSUPPORTED_REQUEST,
            off_message(off),
            request_id,
        ),
        BeamTilesOutcome::Failed(why) => {
            tracing::warn!(request_id, why, "a beam-tile render could not be sent");
            write_error(stream, TRACE_PANIC_CODE, why.to_owned(), request_id)
        }
    }
}

/// Serves one `BeamTileRequest` on this machine's own lane (see the module doc comment).
///
/// # Errors
///
/// [`NetError`] for a transport-level failure. A refused request or a caught renderer panic is
/// answered with a `StreamEvent::Error` and is not an error return.
pub(super) fn serve_beam_tiles<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    mut request: BeamTileRequest,
    ctx: &RequestContext<'_>,
) -> Result<(), NetError> {
    let request_id = request.request_id;
    if let Err(message) = crate::serve::zoning::attach_to_render(request_id, &mut request.scene) {
        return write_error(stream, VALIDATION_FAILED_CODE, message, request_id);
    }
    let live = match validate::validate_beam_tiles(&request) {
        Ok(live) => live,
        Err(refusal) => {
            tracing::info!(
                request_id,
                reason = refusal.message(),
                "refusing beam tiles"
            );
            return write_error(
                stream,
                refusal.code(),
                refusal.message().to_owned(),
                request_id,
            );
        }
    };

    // Validated: the scene is rendered as its posed scene.
    crate::render_core::pose::apply_pose(&mut request.scene);

    let started = Instant::now();
    let cancel = AtomicBool::new(false);
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let result = match render_while_watching(stream, &request, &live, ctx, &cancel) {
        Err(e) => Err(e),
        Ok(Traced::Rendered(outcome)) => write_outcome(stream, &request, outcome),
        Ok(Traced::Panicked) => {
            tracing::warn!(
                request_id,
                "rendering beam tiles panicked for a request that passed validation"
            );
            write_error(
                stream,
                TRACE_PANIC_CODE,
                "internal error while rendering these beam tiles".to_owned(),
                request_id,
            )
        }
    };
    let _ = stream.set_write_timeout(None);
    tracing::info!(
        request_id,
        size = %format_args!("{}x{}", request.scene.width, request.scene.height),
        tiles = live.indices().len(),
        elapsed = ?started.elapsed(),
        "worker: served beam tiles"
    );
    result
}
