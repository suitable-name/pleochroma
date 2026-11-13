//! `LIVE_PASS_REQUEST` on a coordinator without a render lane of its own.
//!
//! The request is forwarded whole to ONE idle joined worker and its reply is relayed under the
//! viewer's `request_id`. With a lane of its own the coordinator serves the request itself, like
//! a plain worker (`serve::connection::requests`).
//!
//! # Why whole, to one worker
//!
//! A live pass names the tiles to trace and the viewer has already split its pass between its
//! engines by tile. The coordinator passes the request on unchanged: the scene, the sample range
//! and the tile set are the viewer's, only the `request_id` is the coordinator's own (so a late
//! event of an earlier request on a held connection cannot be mistaken for this one's), and the
//! zones of a zoned scene go ahead of it as a `ZoningPayload`, as for every chunk. The
//! viewer that wants several joined workers' throughput sends several requests, each on a
//! connection of its own; the coordinator gives each the next idle worker.
//!
//! # What is relayed
//!
//! The worker's `PROGRESS` heartbeats (so the viewer's silence deadline holds), then its one
//! `LIVE_PASS` with the payload frame, or `DONE { cancelled: true }` after a `CANCEL`, or an
//! `ERROR` (re-worded, with the worker's code). A viewer `CANCEL` or hang-up is passed on as the
//! worker's `CANCEL`. A worker that stops answering is discarded and the viewer gets
//! `ALL_WORKERS_LOST`, which it handles like any lost remote chunk (those tiles run on its own
//! lane).

use super::{
    LaneNeed, LaneTimeouts, Registry, ViewerSession, WorkerConn, WorkerHandle,
    lanes::{POLL, PatientReader, WRITE_TIMEOUT, next_worker_request_id},
};
use crate::{
    serve::{CancelPoll, poll_for_cancel},
    stream_emit::TimeoutRead,
    validate,
};
use pleochroma_net::messages::{
    ClientMessage, Done, ErrorMsg, LivePassHeader, LivePassRequest, NetError, Progress,
    StreamEvent, ZoningPayload, error_codes,
};
use std::{
    cell::{Cell, RefCell},
    io::{Read, Write},
    sync::Arc,
    time::{Duration, Instant},
};

/// How long a live pass waits for an idle joined worker.
const WORKER_WAIT: Duration = Duration::from_secs(5);

/// A joined worker's deadlines during a pass: it heartbeats at least every 2 s while tracing, so
/// the silence limit is the usual 8 s; the first reply may take longer (scene upload, a cold GPU).
const PASS_TIMEOUTS: LaneTimeouts = LaneTimeouts {
    first_event: Duration::from_secs(30),
    liveness: Duration::from_secs(8),
    cancel_wait: Duration::from_secs(10),
};

/// Writes `StreamEvent::Error` naming `request_id` to the viewer.
fn write_error<S: Write>(
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

/// Serves one `LivePassRequest` by forwarding it to a joined worker (see the module doc).
///
/// # Errors
///
/// [`NetError`] for a transport failure on the viewer connection while answering a refusal.
pub fn serve_live_pass<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    request: &LivePassRequest,
    session: &ViewerSession,
) -> Result<(), NetError> {
    if let Err(refusal) = validate::validate_live_pass(request) {
        return write_error(
            stream,
            refusal.code(),
            refusal.message().to_owned(),
            request.request_id,
        );
    }
    let need = LaneNeed::pixels(request.scene.width.saturating_mul(request.scene.height));
    let Some(handle) = session
        .coordinator
        .registry()
        .and_then(|registry| wait_for_idle_worker(registry, need))
    else {
        return write_error(
            stream,
            error_codes::NO_RENDER_CAPACITY,
            "no idle joined worker to trace the live pass on, and this coordinator has no own \
             render lane (--render)"
                .to_owned(),
            request.request_id,
        );
    };
    let result = forward(stream, request, handle);
    let _ = stream.set_read_timeout(None);
    result
}

/// Checks out an idle worker that takes the pass, waiting up to [`WORKER_WAIT`].
fn wait_for_idle_worker(registry: &Arc<Registry>, need: LaneNeed) -> Option<WorkerHandle> {
    let deadline = Instant::now() + WORKER_WAIT;
    loop {
        if let Some(handle) = Registry::checkout(registry, |info| need.accepts(info)) {
            return Some(handle);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(POLL);
    }
}

/// Arms the worker socket's deadlines and writes the pass: the zones of a zoned scene first,
/// then the request under the coordinator-local `local_id`.
fn send_request(
    mut conn: &mut dyn WorkerConn,
    request: &LivePassRequest,
    local_id: u32,
) -> Result<(), String> {
    conn.set_timeouts(Some(POLL), Some(WRITE_TIMEOUT))
        .map_err(|e| e.to_string())?;
    if let Some(payload) = ZoningPayload::for_scene(local_id, &request.scene) {
        pleochroma_net::messages::write_message(
            &mut conn,
            &ClientMessage::ZoningPayload(Box::new(payload)),
        )
        .map_err(|e| e.to_string())?;
    }
    let message = ClientMessage::LivePassRequest(Box::new(LivePassRequest {
        request_id: local_id,
        scene: request.scene.clone(),
        first_sample: request.first_sample,
        samples: request.samples,
        live: request.live.clone(),
    }));
    pleochroma_net::messages::write_message(&mut conn, &message).map_err(|e| e.to_string())
}

/// Sends the pass to `handle`'s worker and relays its reply to the viewer; a viewer `CANCEL` (or
/// hang-up) is passed on as the worker's `CANCEL`.
fn forward<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    request: &LivePassRequest,
    mut handle: WorkerHandle,
) -> Result<(), NetError> {
    let worker_id = handle.info().worker_id;
    // The worker's id for this pass carries the viewer's path-cache request along
    // (`PATH_CACHE_FLAG`); the worker echoes it, so the reader below matches it as it is.
    let local_id = pleochroma_net::messages::request_id_with_path_cache(
        next_worker_request_id(),
        request.path_cache(),
    );
    let viewer_gone = Cell::new(false);
    let viewer = RefCell::new(&mut *stream);
    let exchange = {
        let conn = handle.stream();
        send_request(conn, request, local_id).and_then(|()| {
            let mut reader = PatientReader::new(conn, local_id, PASS_TIMEOUTS, || {
                viewer_gone.get()
                    || match poll_for_cancel(&mut **viewer.borrow_mut(), request.request_id) {
                        Ok(CancelPoll::Pending) => false,
                        Ok(CancelPoll::Cancelled | CancelPoll::Closed) | Err(_) => {
                            viewer_gone.set(true);
                            true
                        }
                    }
            });
            relay_events(
                &mut reader,
                &viewer,
                &viewer_gone,
                request.request_id,
                (local_id, worker_id),
            )
        })
    };
    match exchange {
        Ok(()) => {
            let _ = handle.stream().set_timeouts(None, None);
            drop(handle);
            Ok(())
        }
        Err(why) => {
            handle.discard(&format!("live-pass request failed: {why}"));
            if viewer_gone.get() {
                return Ok(());
            }
            let mut viewer = viewer.borrow_mut();
            write_error(
                &mut **viewer,
                error_codes::ALL_WORKERS_LOST,
                format!("the joined worker tracing the live pass was lost ({why})"),
                request.request_id,
            )
        }
    }
}

/// Reads the worker's events until the pass ends and relays each to the viewer under the viewer's
/// `request_id`.
///
/// `Ok` means the exchange ended cleanly (a `LIVE_PASS`, a cancelled `DONE` or an `ERROR`, all
/// relayed) and the connection is in sync; `Err` is why the connection can no longer be used. A
/// write that fails marks the viewer gone (the worker is then cancelled by the reader's next
/// idle tick and drained to its end).
fn relay_events<F: FnMut() -> bool, S: Write>(
    reader: &mut PatientReader<'_, F>,
    viewer: &RefCell<&mut S>,
    viewer_gone: &Cell<bool>,
    request_id: u32,
    (local_id, worker_id): (u32, u32),
) -> Result<(), String> {
    let relay = |event: &StreamEvent, payload: Option<&[u8]>| {
        if viewer_gone.get() {
            return;
        }
        let mut viewer = viewer.borrow_mut();
        if pleochroma_net::messages::write_stream_event(&mut **viewer, event, payload).is_err() {
            viewer_gone.set(true);
        }
    };
    loop {
        let (event, payload) = pleochroma_net::messages::read_stream_event(reader)
            .map_err(|e| format!("worker #{worker_id}: {e}"))?;
        if event.request_id().is_some_and(|id| id != local_id) {
            continue; // a late event of an earlier request on this connection
        }
        match event {
            StreamEvent::Progress(progress) => relay(
                &StreamEvent::Progress(Progress {
                    request_id,
                    ..progress
                }),
                None,
            ),
            StreamEvent::LivePass(header) => {
                relay(
                    &StreamEvent::LivePass(LivePassHeader {
                        request_id,
                        ..header
                    }),
                    payload.as_deref(),
                );
                return Ok(());
            }
            StreamEvent::Done(done) => {
                relay(&StreamEvent::Done(Done { request_id, ..done }), None);
                return Ok(());
            }
            StreamEvent::Error(error) => {
                relay(
                    &StreamEvent::Error(ErrorMsg {
                        code: error.code,
                        message: format!(
                            "joined worker #{worker_id} could not trace the live pass (worker \
                             error code {})",
                            error.code
                        ),
                        request_id: Some(request_id),
                    }),
                    None,
                );
                return Ok(());
            }
            _ => {}
        }
    }
}
