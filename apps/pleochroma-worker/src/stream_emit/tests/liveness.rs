//! Tests for stream-timeout classification and the heartbeat backstop.
//!
//! [`is_stream_timeout`] and [`poll_for_client_message`] treat `WriteZero` as a timeout.
//! `run_stream` applies the same heartbeat gate, but only with a real tracer thread, so these
//! tests drive [`wait_for_tracer_to_stop`] directly with a plain `Write` and an injectable bound.

use super::fixtures::{
    decode_events, finish_after, heartbeat_test_request, shared_state, tiny_scene,
};
use crate::stream_emit::{
    TimeoutCache, TimeoutRead,
    emitter::{ClientPoll, poll_for_client_message, wait_for_tracer_to_stop},
    is_stream_timeout,
};
use pleochroma_net::messages::StreamEvent;
use std::{
    io::ErrorKind,
    sync::{Arc, Mutex},
    time::Duration,
};

#[test]
fn is_stream_timeout_recognizes_would_block_timed_out_and_write_zero() {
    for kind in [
        ErrorKind::WouldBlock,
        ErrorKind::TimedOut,
        ErrorKind::WriteZero,
    ] {
        assert!(
            is_stream_timeout(&std::io::Error::new(kind, "scripted")),
            "expected {kind:?} to be recognized as a stream timeout"
        );
    }
}

#[test]
fn is_stream_timeout_rejects_unrelated_error_kinds() {
    for kind in [
        ErrorKind::ConnectionReset,
        ErrorKind::UnexpectedEof,
        ErrorKind::Other,
    ] {
        assert!(
            !is_stream_timeout(&std::io::Error::new(kind, "scripted")),
            "expected {kind:?} to NOT be recognized as a stream timeout"
        );
    }
}

/// A `Read + TimeoutRead` double whose only read returns the scripted `io::Error`.
struct ErroringRead(ErrorKind);

impl std::io::Read for ErroringRead {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(self.0, "scripted"))
    }
}

impl TimeoutRead for ErroringRead {
    fn set_read_timeout(&mut self, _duration: Option<Duration>) -> std::io::Result<()> {
        Ok(())
    }
}

/// A TLS stream can report a socket timeout as `WriteZero` on a read, because
/// `rustls::Stream::read` may need to flush outgoing bytes first. It must count as a
/// timeout, not as a fatal protocol error.
#[test]
fn poll_for_client_message_tolerates_a_write_zero_on_the_first_byte() {
    let mut stream = ErroringRead(ErrorKind::WriteZero);
    let mut timeouts = TimeoutCache::new();
    let result = poll_for_client_message(&mut stream, 1, Duration::from_millis(1), &mut timeouts);
    assert!(matches!(result, Ok(ClientPoll::Pending)));
}

/// A silent tracer must still get at least two `StreamEvent::Progress` heartbeats over
/// more than two heartbeat intervals. A long calibration probe, GPU sub-batch or
/// wide-cadence cancellation wind-down would otherwise go silent.
#[test]
fn wait_for_tracer_to_stop_heartbeats_at_least_twice_when_the_tracer_produces_nothing() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let request = heartbeat_test_request();
    let state = Arc::new(Mutex::new(shared_state(pixel_count, 0)));
    let (_progress_tx, progress_rx) = std::sync::mpsc::channel::<()>();
    let mut out = Vec::new();
    let mut last_emit = std::time::Instant::now();
    let mut emission_count: u32 = 0;

    let finisher = finish_after(&state, Duration::from_millis(70));

    wait_for_tracer_to_stop(
        &mut out,
        &request,
        &state,
        &progress_rx,
        Duration::from_millis(2), // heartbeat_bound: tiny, so this test stays fast
        &mut last_emit,
        &mut emission_count,
    );
    finisher.join().unwrap();

    let events = decode_events(&out);
    let progress_count = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::Progress(_)))
        .count();
    assert!(
        progress_count >= 2,
        "expected at least 2 heartbeats over > 2 heartbeat intervals, got {progress_count}: {events:?}"
    );
}

/// Within `heartbeat_bound` no heartbeat is written, since recent progress already proves
/// liveness.
#[test]
fn wait_for_tracer_to_stop_suppresses_the_heartbeat_within_the_interval() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let request = heartbeat_test_request();
    let state = Arc::new(Mutex::new(shared_state(pixel_count, 0)));
    let (_progress_tx, progress_rx) = std::sync::mpsc::channel::<()>();
    let mut out = Vec::new();
    let mut last_emit = std::time::Instant::now();
    let mut emission_count: u32 = 0;

    // Finishes well before the bound below could possibly elapse.
    let finisher = finish_after(&state, Duration::from_millis(30));

    wait_for_tracer_to_stop(
        &mut out,
        &request,
        &state,
        &progress_rx,
        Duration::from_secs(10),
        &mut last_emit,
        &mut emission_count,
    );
    finisher.join().unwrap();

    assert!(
        out.is_empty(),
        "no heartbeat should have been written within the interval: {:?}",
        decode_events(&out)
    );
}

/// A heartbeat reports the `samples_done` current when it fires, never a stale value.
#[test]
fn wait_for_tracer_to_stop_heartbeat_carries_the_latest_samples_done() {
    let scene = tiny_scene();
    let pixel_count = scene.width as usize * scene.height as usize;
    let request = heartbeat_test_request();
    let state = Arc::new(Mutex::new(shared_state(pixel_count, 42)));
    let (_progress_tx, progress_rx) = std::sync::mpsc::channel::<()>();
    let mut out = Vec::new();
    let mut last_emit = std::time::Instant::now();
    let mut emission_count: u32 = 0;

    let finisher = finish_after(&state, Duration::from_millis(60));

    wait_for_tracer_to_stop(
        &mut out,
        &request,
        &state,
        &progress_rx,
        Duration::from_millis(2),
        &mut last_emit,
        &mut emission_count,
    );
    finisher.join().unwrap();

    let events = decode_events(&out);
    let last_progress = events.iter().rev().find_map(|e| match e {
        StreamEvent::Progress(p) => Some(p.samples_done),
        _ => None,
    });
    assert_eq!(
        last_progress,
        Some(42),
        "heartbeat must carry the current samples_done: {events:?}"
    );
}
