//! Starting in the background, shutting down, and running without a design library.
//!
//! The coordinators here use `--insecure-no-tls` on loopback with ephemeral ports: nothing
//! in these tests is about TLS, and it keeps them free of certificate setup.

use super::fixtures::{
    DuplexHalf, final_only, read_stream_until_done, test_db_file, tiny_scene, unique_temp_dir,
};
use crate::{
    cli::{ComputeMode, ServeArgs},
    serve::{
        self, ServeHandle, handle_connection,
        library::{LIBRARY_ERROR_CODE, LibraryHandle},
    },
};
use pleochroma_net::{
    handshake,
    library::{LibraryRequest, LibraryResponse},
    messages::{
        ClientMessage, Done, RenderRequest, RequestIntent, StreamEvent, Welcome, read_message,
        write_message,
    },
};
use std::{
    io::{Cursor, Read},
    net::TcpStream,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

/// A coordinator on loopback without TLS, workers or enrollment, serving the library at `db`.
fn insecure_args(bind: &str, db: PathBuf) -> ServeArgs {
    ServeArgs {
        bind: bind.to_string(),
        threads: 2,
        allow_remote: false,
        ca: None,
        cert: None,
        key: None,
        allowlist: None,
        trust_any_client_cert: false,
        insecure_no_tls: true,
        compute_mode: ComputeMode::Hybrid,
        render: false,
        enroll_bind: None,
        no_enroll: true,
        worker_bind: None,
        worker_enroll_bind: None,
        no_workers: true,
        worker_allowlist: None,
        db: Some(db),
        max_connections: 16,
        max_preauth_per_ip: crate::cli::DEFAULT_MAX_PREAUTH_PER_IP,
        interactive_workers: 0,
        pin_interactive_worker: None,
        max_job_memory_mib: crate::cli::DEFAULT_MAX_JOB_MEMORY_MIB,
        whole_image_secs: crate::cli::DEFAULT_WHOLE_IMAGE_SECS,
        whole_image_pixel_samples: crate::cli::DEFAULT_WHOLE_IMAGE_PIXEL_SAMPLES,
        jobs_per_viewer: crate::cli::DEFAULT_JOBS_PER_VIEWER,
        payload_encoding: pleochroma_net::messages::adaptive::PayloadChoice::Auto,
        allow_remote_admin: false,
        no_library: false,
    }
}

const DEADLINE: Duration = Duration::from_secs(10);

/// A path no database exists at.
fn missing_database() -> PathBuf {
    unique_temp_dir("lifecycle").join("absent.sqlite")
}

fn start_insecure(bind: &str, db: PathBuf) -> ServeHandle {
    serve::start(&insecure_args(bind, db)).expect("the coordinator starts")
}

#[test]
fn shutdown_stops_the_listeners_within_the_deadline_and_frees_the_port() {
    let handle = start_insecure("127.0.0.1:0", test_db_file());
    let addr = handle.viewer_addr;
    TcpStream::connect(addr).expect("the viewer port accepts while running");

    let begun = Instant::now();
    let report = handle.shutdown(DEADLINE);
    assert!(report.is_clean(), "{report:?}");
    assert!(begun.elapsed() < DEADLINE);

    assert!(
        TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_err(),
        "nothing listens after shutdown"
    );
    // A second call waits for nothing and reports the same.
    assert!(handle.shutdown(DEADLINE).is_clean());
}

#[test]
fn start_shutdown_start_works_on_the_same_port() {
    let first = start_insecure("127.0.0.1:0", test_db_file());
    let addr = first.viewer_addr;
    assert!(first.shutdown(DEADLINE).is_clean());

    let second = start_insecure(&addr.to_string(), test_db_file());
    assert_eq!(second.viewer_addr, addr);
    assert!(second.shutdown(DEADLINE).is_clean());
}

#[test]
fn shutdown_closes_a_live_connection() {
    let handle = start_insecure("127.0.0.1:0", test_db_file());
    let mut client = TcpStream::connect(handle.viewer_addr).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    // Say HELLO and read WELCOME, so the connection is fully served.
    write_message(&mut client, &handshake::local_hello()).unwrap();
    let _welcome: Welcome = read_message(&mut client).unwrap();

    assert!(handle.shutdown(DEADLINE).is_clean());
    let mut byte = [0_u8; 1];
    assert!(
        matches!(client.read(&mut byte), Ok(0) | Err(_)),
        "the server end is closed"
    );
}

#[test]
fn start_returns_before_the_listeners_are_ready() {
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let args = insecure_args("127.0.0.1:0", test_db_file());
    let mut starting = serve::lifecycle::spawn_start(args, move || {
        let _ = release_rx.recv();
    });
    // The start-up thread is held before it binds anything, yet the call has returned.
    assert!(starting.poll().is_none(), "still starting");
    assert!(starting.poll().is_none(), "polling never blocks");

    release_tx.send(()).unwrap();
    let handle = loop {
        if let Some(result) = starting.poll() {
            break result.expect("the coordinator starts");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    TcpStream::connect(handle.viewer_addr).expect("ready after the start finished");
    assert!(starting.poll().is_none(), "the outcome is delivered once");
    assert!(handle.shutdown(DEADLINE).is_clean());
}

#[test]
fn a_cancelled_start_leaves_nothing_running() {
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let args = insecure_args("127.0.0.1:0", test_db_file());
    let starting = serve::lifecycle::spawn_start(args, move || {
        let _ = release_rx.recv();
    });
    starting.cancel();
    release_tx.send(()).unwrap();
    assert!(starting.wait().is_err());
}

#[test]
fn a_missing_library_does_not_fail_the_start_and_library_requests_get_a_plain_error() {
    let handle = start_insecure("127.0.0.1:0", missing_database());
    let mut client = TcpStream::connect(handle.viewer_addr).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write_message(&mut client, &handshake::local_hello()).unwrap();
    let _welcome: Welcome = read_message(&mut client).unwrap();
    write_message(
        &mut client,
        &ClientMessage::Library(Box::new(LibraryRequest::FilterOptions)),
    )
    .unwrap();
    let response: LibraryResponse = read_message(&mut client).unwrap();
    let LibraryResponse::Error(error) = response else {
        panic!("expected LibraryResponse::Error, got {response:?}");
    };
    assert_eq!(error.code, LIBRARY_ERROR_CODE);
    assert!(error.message.contains("no design library"), "{error:?}");
    drop(client);
    assert!(handle.shutdown(DEADLINE).is_clean());
}

#[test]
fn a_library_less_connection_still_renders() {
    let library = LibraryHandle::absent(None);
    let mut input = Vec::new();
    write_message(&mut input, &handshake::local_hello()).unwrap();
    write_message(
        &mut input,
        &ClientMessage::Library(Box::new(LibraryRequest::FilterOptions)),
    )
    .unwrap();
    write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(RenderRequest {
            intent: RequestIntent::Batch,
            request_id: 3,
            scene: tiny_scene(),
            first_sample: 0,
            samples: 2,
            stream: final_only(0),
        })),
    )
    .unwrap();
    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 1, &library).unwrap();

    let mut out = Cursor::new(duplex.out);
    let _welcome: Welcome = read_message(&mut out).unwrap();
    let response: LibraryResponse = read_message(&mut out).unwrap();
    assert!(
        matches!(&response, LibraryResponse::Error(e) if e.code == LIBRARY_ERROR_CODE),
        "{response:?}"
    );
    let events = read_stream_until_done(&mut out);
    assert!(matches!(
        events.last().unwrap().0,
        StreamEvent::Done(Done {
            cancelled: false,
            ..
        })
    ));
    assert!(!library.is_open());
}
