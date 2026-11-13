//! `BEAM_TILE_REQUEST` on a coordinator without a render lane of its own.
//!
//! The request is forwarded unchanged to ONE idle joined worker and its reply is relayed under the
//! viewer's `request_id`. With a lane of its own the coordinator serves the request itself, like a
//! plain worker (`serve::connection::requests`).
//!
//! # Whole, to one worker
//!
//! The viewer splits the frame's tiles between its engines and the workers before it sends,
//! so a request names exactly the tiles one worker renders. The coordinator passes it on
//! unchanged, except for the `request_id`: its own id keeps a late event of an earlier request
//! on a held connection from being mistaken for this one's. The zones of a zoned scene go
//! ahead of it as a `ZoningPayload`, as for every chunk. A viewer that wants several
//! workers sends several requests, each on a connection of its own; the coordinator gives each
//! the next idle worker.
//!
//! # What is relayed
//!
//! The worker's `PROGRESS` heartbeats, then its one `BEAM_TILES` with the payload frame, or
//! `DONE { cancelled: true }` after a `CANCEL`, or an `ERROR` (re-worded, with the worker's code).
//! A viewer `CANCEL` or hang-up is passed on as the worker's `CANCEL`; after a `CANCEL` the viewer
//! is still connected and receives the worker's terminal event. After a hang-up (or a viewer
//! write that fails or takes over 10 s) nothing is written. A worker that stops answering is
//! discarded and the viewer gets `ALL_WORKERS_LOST`; the viewer then assigns those tiles to
//! another engine, as it does for any lost remote chunk.

use super::{
    LaneNeed, LaneTimeouts, Registry, ViewerSession, WorkerConn, WorkerHandle,
    lanes::{POLL, PatientReader, WRITE_TIMEOUT, next_worker_request_id},
};
use crate::{
    serve::{CancelPoll, poll_for_cancel},
    stream_emit::{TimeoutRead, TimeoutWrite},
    validate,
};
use pleochroma_net::messages::{
    BeamTileRequest, BeamTilesHeader, ClientMessage, Done, ErrorMsg, NetError, Progress,
    StreamEvent, ZoningPayload, error_codes,
};
use std::{
    cell::{Cell, RefCell},
    io::{Read, Write},
    sync::Arc,
    time::{Duration, Instant},
};

/// How long beam tiles wait for an idle joined worker.
const WORKER_WAIT: Duration = Duration::from_secs(5);

/// A joined worker's deadlines: it heartbeats at least every 2 s while rendering, so the silence
/// limit is the usual 8 s; the first reply may take longer (scene upload, a cold GPU).
const TILE_TIMEOUTS: LaneTimeouts = LaneTimeouts {
    first_event: Duration::from_secs(30),
    liveness: Duration::from_secs(8),
    cancel_wait: Duration::from_secs(10),
};

/// How long one write to the viewer may block: the direct worker connection's deadline.
const VIEWER_WRITE_TIMEOUT: Duration = WRITE_TIMEOUT;

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

/// Serves one `BeamTileRequest` by forwarding it to a joined worker (see the module doc).
///
/// # Errors
///
/// [`NetError`] for a transport failure on the viewer connection while answering a refusal.
pub fn serve_beam_tiles<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    request: &BeamTileRequest,
    session: &ViewerSession,
) -> Result<(), NetError> {
    if let Err(refusal) = validate::validate_beam_tiles(request) {
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
            "no idle joined worker to render the beam tiles on, and this coordinator has no own \
             render lane (--render)"
                .to_owned(),
            request.request_id,
        );
    };
    let result = forward(stream, request, handle);
    let _ = stream.set_read_timeout(None);
    let _ = stream.set_write_timeout(None);
    result
}

/// Checks out an idle worker that takes the tiles, waiting up to [`WORKER_WAIT`].
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

/// Arms the worker socket's deadlines and writes the request: the zones of a zoned scene first,
/// then the request under the coordinator-local `local_id`.
fn send_request(
    mut conn: &mut dyn WorkerConn,
    request: &BeamTileRequest,
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
    let message = ClientMessage::BeamTileRequest(Box::new(BeamTileRequest {
        request_id: local_id,
        ..request.clone()
    }));
    pleochroma_net::messages::write_message(&mut conn, &message).map_err(|e| e.to_string())
}

/// Sends the request to `handle`'s worker and relays its reply to the viewer; a viewer `CANCEL`
/// (or hang-up) is passed on as the worker's `CANCEL`.
fn forward<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    request: &BeamTileRequest,
    mut handle: WorkerHandle,
) -> Result<(), NetError> {
    let worker_id = handle.info().worker_id;
    let local_id = next_worker_request_id();
    // A cancelled viewer is still connected and gets the terminal acknowledgement; only a
    // hang-up or a failed write sets `viewer_gone`.
    let viewer_gone = Cell::new(false);
    let viewer_cancelled = Cell::new(false);
    // Bounds every downstream write like the direct worker connection, so a viewer that stops
    // reading cannot hold the checked-out worker (the write then fails and `viewer_gone` is set).
    let _ = stream.set_write_timeout(Some(VIEWER_WRITE_TIMEOUT));
    let viewer = RefCell::new(&mut *stream);
    let exchange = {
        let conn = handle.stream();
        send_request(conn, request, local_id).and_then(|()| {
            let mut reader = PatientReader::new(conn, local_id, TILE_TIMEOUTS, || {
                viewer_gone.get()
                    || viewer_cancelled.get()
                    || match poll_for_cancel(&mut **viewer.borrow_mut(), request.request_id) {
                        Ok(CancelPoll::Pending) => false,
                        Ok(CancelPoll::Cancelled) => {
                            viewer_cancelled.set(true);
                            true
                        }
                        Ok(CancelPoll::Closed) | Err(_) => {
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
            handle.discard(&format!("beam-tile request failed: {why}"));
            if viewer_gone.get() {
                return Ok(());
            }
            let mut viewer = viewer.borrow_mut();
            write_error(
                &mut **viewer,
                error_codes::ALL_WORKERS_LOST,
                format!("the joined worker rendering the beam tiles was lost ({why})"),
                request.request_id,
            )
        }
    }
}

/// Reads the worker's events until the request ends and relays each to the viewer under the
/// viewer's `request_id`.
///
/// `Ok` means the exchange ended cleanly (a `BEAM_TILES`, a cancelled `DONE` or an `ERROR`, all
/// relayed) and the connection is in sync; `Err` is why the connection can no longer be used.
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
            StreamEvent::BeamTiles(header) => {
                relay(
                    &StreamEvent::BeamTiles(BeamTilesHeader {
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
                            "joined worker #{worker_id} could not render the beam tiles \
                             (worker error: {})",
                            error.message
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

#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests {
    use super::*;
    use pleochroma_net::messages::Stats;
    use std::io::Cursor;

    /// A worker connection that replays a prepared byte stream.
    struct Replay(Cursor<Vec<u8>>);

    impl Read for Replay {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(buf)
        }
    }

    impl Write for Replay {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl WorkerConn for Replay {
        fn set_timeouts(
            &mut self,
            _read: Option<Duration>,
            _write: Option<Duration>,
        ) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A viewer whose every write hits the write deadline.
    struct Stalled;

    impl Write for Stalled {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::TimedOut.into())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// The bytes of a worker's cancelled `DONE` for request 5.
    fn cancelled_done() -> Vec<u8> {
        let mut bytes = Vec::new();
        let done = Done {
            request_id: 5,
            cancelled: true,
            stats: Stats {
                samples_done: 0,
                requested_cadence_ms: 0,
                effective_cadence_ms: 0,
                reclaimed_samples: 0,
            },
        };
        pleochroma_net::messages::write_stream_event(&mut bytes, &StreamEvent::Done(done), None)
            .unwrap();
        bytes
    }

    #[test]
    fn a_connected_viewer_receives_the_terminal_acknowledgement() {
        let mut conn = Replay(Cursor::new(cancelled_done()));
        let mut reader = PatientReader::new(&mut conn, 5, TILE_TIMEOUTS, || false);
        let mut sink = Vec::new();
        let viewer = RefCell::new(&mut sink);
        let gone = Cell::new(false);
        relay_events(&mut reader, &viewer, &gone, 9, (5, 1)).unwrap();
        assert!(!gone.get());
        let (event, _) =
            pleochroma_net::messages::read_stream_event(&mut Cursor::new(sink)).unwrap();
        assert!(matches!(
            event,
            StreamEvent::Done(Done {
                request_id: 9,
                cancelled: true,
                ..
            })
        ));
    }

    #[test]
    fn a_viewer_write_that_hits_its_deadline_ends_the_relay_and_marks_the_viewer_gone() {
        let mut conn = Replay(Cursor::new(cancelled_done()));
        let mut reader = PatientReader::new(&mut conn, 5, TILE_TIMEOUTS, || false);
        let mut stalled = Stalled;
        let viewer = RefCell::new(&mut stalled);
        let gone = Cell::new(false);
        relay_events(&mut reader, &viewer, &gone, 9, (5, 1)).unwrap();
        assert!(gone.get(), "the failed write must not hold the worker");
    }
}
