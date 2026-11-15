//! Adaptive live passes: [`serve_live_pass`] answers one `ClientMessage::LivePassRequest` on
//! this machine's own lane.
//!
//! # What a request is
//!
//! One chunk of an adaptive pass: `samples` absolute samples from `first_sample` for the pixels of
//! a set of live tiles (`pleochroma_net::messages::live_pass`). The reply is one
//! `StreamEvent::LivePass` carrying the pass total and the odd-sample total of every live pixel,
//! or an `ERROR`, or -- after a `CANCEL` -- `DONE { cancelled: true }`. Nothing is streamed: the
//! convergence check needs the whole pass, so a partial reply would be of no use.
//!
//! # Threads
//!
//! TLS state cannot be shared, so the connection thread owns the stream. The pass runs on a
//! helper thread ([`render_core::live::trace_live_pass`], the GPU when it takes the scene and the
//! threaded CPU tracer otherwise) while this thread heartbeats `PROGRESS` every
//! [`HEARTBEAT_INTERVAL`] -- the viewer's silence deadline holds however long the pass takes --
//! and watches the socket for a `CANCEL`.
//!
//! # What is refused
//!
//! The Ray-Traced engine and an HDR environment (`UNSUPPORTED_REQUEST`; the pass is studio-lit
//! and the viewer then keeps those tiles on its own lane), and anything that breaks a limit
//! (`VALIDATION_FAILED`, see `validate::validate_live_pass`). A zoned scene needs the
//! `ZoningPayload` the viewer sends just ahead of the request.

use super::requests::{RequestContext, TRACE_PANIC_CODE, VALIDATION_FAILED_CODE, write_pong};
use crate::{
    assets, render_core,
    stream_emit::{
        HEARTBEAT_INTERVAL, RawPoll, TimeoutCache, TimeoutRead, TimeoutWrite,
        poll_raw_client_message,
    },
    validate,
};
use glam::Vec3;
use pleochroma_net::messages::{
    ClientMessage, Done, ErrorMsg, LivePassHeader, LivePassRequest, NetError, Progress, Stats,
    StreamEvent, encode_live_sums,
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

/// Bounds every write of the request: a peer that stops reading ends the connection instead of
/// hanging it (the bound `stream_emit` and the batch session apply too).
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// How the helper thread's pass ended.
enum Traced {
    /// Every live pixel's pair, in payload order.
    Done(Vec<(Vec3, Vec3)>),
    /// `cancel` was raised first.
    Cancelled,
    /// The tracer panicked on a scene that passed validation.
    Panicked,
    /// The scene's Stone Shadow cannot be built; the request is refused with this sentence.
    Refused(&'static str),
}

/// Writes `StreamEvent::Error` naming `request_id`.
pub(super) fn write_error<S: Write>(
    stream: &mut S,
    code: u32,
    message: String,
    request_id: u32,
) -> Result<(), NetError> {
    pleochroma_net::messages::write_stream_event(
        stream,
        &StreamEvent::Error(ErrorMsg {
            code,
            message,
            request_id: Some(request_id),
        }),
        None,
    )
}

/// Handles one message that arrived while the pass traces; only a `CANCEL` for this request
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
        // The zones of a request pipelined behind this one; kept for it.
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
                "ignoring a message pipelined while live pass {request_id} traces (a live pass \
                 has no pipelining): {other:?}"
            );
            Ok(())
        }
    }
}

/// The reply to a pass that was cancelled: `DONE { cancelled: true }`.
pub(super) fn write_cancelled<S: Write>(stream: &mut S, request_id: u32) -> Result<(), NetError> {
    pleochroma_net::messages::write_stream_event(
        stream,
        &StreamEvent::Done(Done {
            request_id,
            cancelled: true,
            stats: Stats {
                samples_done: 0,
                requested_cadence_ms: 0,
                effective_cadence_ms: 0,
                reclaimed_samples: 0,
            },
        }),
        None,
    )
}

/// Runs the pass on a helper thread and keeps the connection alive meanwhile.
///
/// Returns how the pass ended, or the transport error that ended the wait (the helper is then
/// cancelled and joined first).
fn trace_while_watching<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    request: &LivePassRequest,
    live: &pleochroma::renderer::adaptive::LiveTiles,
    ctx: &RequestContext<'_>,
    cancel: &AtomicBool,
) -> Result<Traced, NetError> {
    let gpu: &pleochroma::renderer::gpu_backend::GpuBackend = ctx.gpu;
    let threads = ctx.threads;
    let request_id = request.request_id;
    thread::scope(|scope| {
        let tracer = scope.spawn(|| {
            catch_unwind(AssertUnwindSafe(|| {
                // The Stone Shadow and the Tray Reflection, resolved once for the pass. A live
                // pass is a finished picture, so it takes the Final map (no intent). It is
                // built on this helper thread so the heartbeats cover the build.
                let tray = match assets::resolve_tray(&request.scene, None, cancel) {
                    Ok(tray) => tray,
                    Err(assets::TrayStop::Cancelled) => return Ok(None),
                    Err(assets::TrayStop::Refused(message)) => return Err(message),
                };
                // A request that asks for a path cache (the flag bit of its id) gets this
                // worker's own cache of the tiles it is given; without the flag the pass is the
                // plain one.
                let trace = if request.path_cache() {
                    render_core::live_cache::trace_live_pass_cached
                } else {
                    render_core::live::trace_live_pass
                };
                Ok(trace(
                    gpu,
                    &request.scene,
                    live,
                    (request.first_sample, request.samples),
                    threads,
                    cancel,
                    tray.view(),
                ))
            }))
        });
        let mut timeouts = TimeoutCache::new();
        let mut last_write = Instant::now();
        let mut failure: Option<NetError> = None;
        while !tracer.is_finished() {
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
        let joined = tracer.join();
        if let Some(e) = failure {
            return Err(e);
        }
        Ok(match joined {
            Ok(Ok(Ok(Some(pixels)))) => Traced::Done(pixels),
            Ok(Ok(Ok(None))) => Traced::Cancelled,
            Ok(Ok(Err(message))) => Traced::Refused(message),
            Ok(Err(_)) | Err(_) => Traced::Panicked,
        })
    })
}

/// Serves one `LivePassRequest` on this machine's own lane (see the module doc comment).
///
/// # Errors
///
/// [`NetError`] for a transport-level failure. A refused request or a caught tracer panic is
/// answered with a `StreamEvent::Error` and is not an error return.
pub(super) fn serve_live_pass<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    mut request: LivePassRequest,
    ctx: &RequestContext<'_>,
) -> Result<(), NetError> {
    let request_id = request.request_id;
    // Re-attach the zones the viewer sent ahead of this request (they are `serde(skip)` in the
    // scene). A payload that does not fit refuses the request.
    if let Err(message) = crate::serve::zoning::attach_to_render(request_id, &mut request.scene) {
        return write_error(stream, VALIDATION_FAILED_CODE, message, request_id);
    }
    let live = match validate::validate_live_pass(&request) {
        Ok(live) => live,
        Err(refusal) => {
            tracing::info!(
                request_id,
                reason = refusal.message(),
                "refusing a live pass"
            );
            return write_error(
                stream,
                refusal.code(),
                refusal.message().to_owned(),
                request_id,
            );
        }
    };

    // Validated: the scene is traced as its posed scene.
    crate::render_core::pose::apply_pose(&mut request.scene);

    let started = Instant::now();
    let cancel = AtomicBool::new(false);
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let traced = trace_while_watching(stream, &request, &live, ctx, &cancel);
    let result = match traced {
        Err(e) => Err(e),
        Ok(Traced::Done(pixels)) => {
            let payload = encode_live_sums(&pixels);
            let header = LivePassHeader {
                request_id,
                first_sample: request.first_sample,
                samples: request.samples,
                live_pixels: u32::try_from(pixels.len()).unwrap_or(u32::MAX),
                payload_len: u32::try_from(payload.len()).unwrap_or(u32::MAX),
            };
            pleochroma_net::messages::write_stream_event(
                stream,
                &StreamEvent::LivePass(header),
                Some(&payload),
            )
        }
        Ok(Traced::Cancelled) => write_cancelled(stream, request_id),
        Ok(Traced::Refused(message)) => {
            tracing::info!(request_id, reason = message, "refusing a live pass");
            write_error(
                stream,
                VALIDATION_FAILED_CODE,
                message.to_owned(),
                request_id,
            )
        }
        Ok(Traced::Panicked) => {
            tracing::warn!(
                request_id,
                first_sample = request.first_sample,
                samples = request.samples,
                "tracing a live pass panicked for a request that passed validation"
            );
            write_error(
                stream,
                TRACE_PANIC_CODE,
                "internal error while tracing this live pass".to_owned(),
                request_id,
            )
        }
    };
    let _ = stream.set_write_timeout(None);
    tracing::info!(
        request_id,
        size = %format_args!("{}x{}", request.scene.width, request.scene.height),
        live_tiles = live.indices().len(),
        samples = request.samples,
        elapsed = ?started.elapsed(),
        "worker: served a live pass"
    );
    result
}
