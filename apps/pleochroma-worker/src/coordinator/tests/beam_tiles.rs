//! Beam tiles through a real coordinator on loopback with a joined CPU worker.
//!
//! The coordinator holds no lane of its own, so every request is forwarded unchanged to the worker
//! and the reply relayed under the viewer's `request_id`.

use super::{
    fixtures::{bundle, coordinator_args, pki_with_server, start, tls_client, wait_for},
    support::{ViewerStream, scene, send, spawn_worker, viewer},
};
use crate::{
    coordinator::LivenessConfig,
    render_core::beam::{BeamTilesOutcome, reference_beam_tiles},
};
use pleochroma::renderer::{
    accel_beam::BeamSettings,
    adaptive::{AdaptiveSettings, LiveTiles, NoiseMetric},
};
use pleochroma_net::{
    SceneState,
    client::handshake_with_hello,
    handshake,
    messages::{
        Backend, BeamTilePixel, BeamTileRequest, Cancel, ClientMessage, Done, ErrorMsg,
        LiveTileSet, MAX_BEAM_TILE_PIXELS, PeerRole, Progress, RenderCapability, Stats,
        StreamEvent, decode_beam_tiles_reply, error_codes,
    },
};
use std::{
    net::{Shutdown, SocketAddr},
    path::Path,
    sync::Arc,
    thread,
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(20);

/// The beam tile and adaptive tile edge of every request below.
const TILE: u32 = 8;

fn adaptive() -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise: 0.5,
        metric: NoiseMetric::RelativeLstar,
        min_spp: 8,
        max_spp: 16,
        pass_spp: 8,
        tile: TILE,
    }
}

/// A coarse tree. These tests check bit identity, relaying and refusals, which hold at any
/// tree resolution; the default 48 spectral nodes and depth 24 would make the tree build
/// dominate the run.
fn beam_settings() -> BeamSettings {
    BeamSettings {
        dir_tolerance_rad: 1e-2,
        chord_tolerance_px: 1e-2,
        weight_tolerance: 1e-2,
        lighting_tolerance: 1e-2,
        cutoff: 1e-2,
        spectral_nodes: 8,
        absorption_subnodes: 1,
        max_split_depth: 8,
        max_fragment_levels: 3,
        residual_min_spp: 8,
        ..BeamSettings::default()
    }
}

fn request(request_id: u32, scene: SceneState, tiles: Vec<u32>) -> BeamTileRequest {
    let live = LiveTiles::from_indices(scene.width, scene.height, TILE, tiles);
    BeamTileRequest {
        request_id,
        scene,
        tiles: LiveTileSet::from_live(&live),
        beam: beam_settings(),
        adaptive: adaptive(),
        sample_offset: 0,
    }
}

fn send_request(client: &mut ViewerStream, request: &BeamTileRequest) {
    send(
        client,
        &ClientMessage::BeamTileRequest(Box::new(request.clone())),
    );
}

/// What a beam-tile request got back.
enum Reply {
    /// The finished pixels, with the id the reply carried.
    Tiles(u32, Vec<BeamTilePixel>),
    /// A refusal or a loss.
    Error(ErrorMsg),
}

/// Reads events until the request's terminal one. Heartbeats and capability broadcasts are
/// skipped: a worker joining or dropping changes the capacity at any time, so a broadcast can
/// come before the reply.
fn read_reply(client: &mut ViewerStream) -> Reply {
    loop {
        let (event, payload) = pleochroma_net::messages::read_stream_event(client).unwrap();
        match event {
            StreamEvent::Progress(_) | StreamEvent::CapabilityChanged { .. } => {}
            StreamEvent::BeamTiles(_) => {
                let (header, pixels) = decode_beam_tiles_reply(&event, &payload.unwrap()).unwrap();
                return Reply::Tiles(header.request_id, pixels);
            }
            StreamEvent::Error(error) => return Reply::Error(error),
            other => panic!("unexpected reply to beam tiles: {other:?}"),
        }
    }
}

/// A fake worker that answers `PING`s and dies (closes its connection) on its first
/// `BeamTileRequest`, counting nothing: the test reads the viewer's `ALL_WORKERS_LOST`.
fn spawn_dying_beam_worker(worker_addr: SocketAddr, bundle_dir: &Path) {
    let capability = RenderCapability {
        backend: Backend::Cpu { threads: 2 },
        max_pixels: 1 << 20,
        min_cadence_ms: 100,
        hdr: false,
    };
    let mut tls = tls_client(worker_addr, bundle_dir);
    thread::spawn(move || {
        if handshake_with_hello(&mut tls, &handshake::local_worker_hello(capability)).is_err() {
            return;
        }
        let _ = tls.sock.set_read_timeout(Some(Duration::from_secs(300)));
        loop {
            match pleochroma_net::messages::read_message::<_, ClientMessage>(&mut tls) {
                Ok(ClientMessage::Ping { nonce }) => {
                    let _ = pleochroma_net::messages::write_stream_event(
                        &mut tls,
                        &StreamEvent::Pong { nonce },
                        None,
                    );
                }
                Ok(ClientMessage::BeamTileRequest(_)) => {
                    let _ = tls.sock.shutdown(Shutdown::Both);
                    return;
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    });
}

/// Beam tiles through a coordinator and a joined worker equal the local render of the same
/// tiles, bit for bit (CPU on both sides, one platform), under the viewer's own `request_id`.
#[test]
fn beam_tiles_through_a_joined_worker_equal_the_local_render_of_the_same_tiles() {
    let pki = pki_with_server("beam-tiles-e2e");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    spawn_worker(handle.worker_addr.unwrap(), &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));
    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);

    let asked = request(101, scene(40, 24), vec![0, 3, 7, 14]);
    send_request(&mut client, &asked);
    let Reply::Tiles(id, pixels) = read_reply(&mut client) else {
        panic!("the worker renders the tiles");
    };
    assert_eq!(id, 101, "the viewer's own id");
    let live = asked.live_tiles().unwrap();
    let BeamTilesOutcome::Done(local) = reference_beam_tiles(&asked, &live) else {
        panic!("the local render succeeds too");
    };
    assert_eq!(pixels.len() as u64, live.pixel_count());
    assert_eq!(pixels, local.pixels, "bit for bit");
}

/// A request past the pixel cap is refused by the coordinator itself with the viewer's id. The
/// connection keeps serving: the next request reaches the worker and is answered.
#[test]
fn a_request_over_the_pixel_cap_is_refused_and_the_connection_keeps_serving() {
    let pki = pki_with_server("beam-tiles-cap");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    spawn_worker(handle.worker_addr.unwrap(), &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));
    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);

    let wide = scene(2304, 2048);
    let live = LiveTiles::all(wide.width, wide.height, TILE);
    assert!(live.pixel_count() > MAX_BEAM_TILE_PIXELS);
    let over = BeamTileRequest {
        tiles: LiveTileSet::from_live(&live),
        ..request(41, wide, vec![0])
    };
    send_request(&mut client, &over);
    let Reply::Error(error) = read_reply(&mut client) else {
        panic!("over the cap is refused");
    };
    assert_eq!(error.code, error_codes::VALIDATION_FAILED);
    assert_eq!(error.request_id, Some(41));

    send_request(&mut client, &request(42, scene(16, 16), vec![0, 1]));
    match read_reply(&mut client) {
        Reply::Tiles(id, _) => assert_eq!(id, 42),
        Reply::Error(error) => {
            // The beams refuse this stone; the worker's refusal is relayed under the viewer's
            // id, never as a loss.
            assert_eq!(
                error.code,
                error_codes::UNSUPPORTED_REQUEST,
                "{}",
                error.message
            );
            assert_eq!(error.request_id, Some(42));
        }
    }
}

/// A stone the beams refuse gets the worker's `BeamOff` sentence, relayed with the viewer's
/// id, so the viewer's export thread renders those tiles itself.
#[test]
fn a_stone_the_beams_refuse_is_relayed_with_the_beam_off_message() {
    let pki = pki_with_server("beam-tiles-off");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    spawn_worker(handle.worker_addr.unwrap(), &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));
    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);

    // A frosted girdle is a stone the beams refuse.
    let mut frosted = scene(16, 16);
    frosted.girdle_frosted = true;
    send_request(&mut client, &request(51, frosted, vec![0]));
    let Reply::Error(error) = read_reply(&mut client) else {
        panic!("the beams refuse a frosted stone");
    };
    assert_eq!(error.code, error_codes::UNSUPPORTED_REQUEST);
    assert_eq!(error.request_id, Some(51));
    assert!(
        error.message.contains("Beam tracing"),
        "the BeamOff sentence: {}",
        error.message
    );
}

/// A worker that drops while it holds the request leaves the viewer `ALL_WORKERS_LOST` under
/// its own id, the signal on which the export thread re-assigns those tiles. The same request
/// is then served by the next worker that joins.
#[test]
fn a_worker_dropping_mid_request_is_reported_lost_and_the_request_can_be_reassigned() {
    let pki = pki_with_server("beam-tiles-failover");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    spawn_dying_beam_worker(handle.worker_addr.unwrap(), &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));
    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);

    let asked = request(61, scene(16, 16), vec![0, 1]);
    send_request(&mut client, &asked);
    let Reply::Error(error) = read_reply(&mut client) else {
        panic!("the dead worker answers nothing");
    };
    assert_eq!(
        error.code,
        error_codes::ALL_WORKERS_LOST,
        "{}",
        error.message
    );
    assert_eq!(error.request_id, Some(61));
    assert!(wait_for(WAIT, || registry.capacity().workers == 0));

    // Re-assignment: a real worker joins and takes the same tiles.
    spawn_worker(handle.worker_addr.unwrap(), &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));
    send_request(&mut client, &request(62, scene(16, 16), vec![0, 1]));
    match read_reply(&mut client) {
        Reply::Tiles(id, pixels) => {
            assert_eq!(id, 62);
            assert_eq!(pixels.len(), 2 * 64);
        }
        Reply::Error(error) => {
            assert_eq!(
                error.code,
                error_codes::UNSUPPORTED_REQUEST,
                "{}",
                error.message
            );
            assert_eq!(error.request_id, Some(62));
        }
    }
}

/// A fake worker that heartbeats on its first `BeamTileRequest`, waits for the coordinator's
/// `CANCEL` and acknowledges it with `DONE { cancelled: true }` under the request's id.
fn spawn_cancel_acknowledging_worker(worker_addr: SocketAddr, bundle_dir: &Path) {
    let capability = RenderCapability {
        backend: Backend::Cpu { threads: 2 },
        max_pixels: 1 << 20,
        min_cadence_ms: 100,
        hdr: false,
    };
    let mut tls = tls_client(worker_addr, bundle_dir);
    thread::spawn(move || {
        if handshake_with_hello(&mut tls, &handshake::local_worker_hello(capability)).is_err() {
            return;
        }
        let _ = tls.sock.set_read_timeout(Some(Duration::from_secs(300)));
        let mut asked = None;
        loop {
            match pleochroma_net::messages::read_message::<_, ClientMessage>(&mut tls) {
                Ok(ClientMessage::Ping { nonce }) => {
                    let _ = pleochroma_net::messages::write_stream_event(
                        &mut tls,
                        &StreamEvent::Pong { nonce },
                        None,
                    );
                }
                Ok(ClientMessage::BeamTileRequest(request)) => {
                    asked = Some(request.request_id);
                    let _ = pleochroma_net::messages::write_stream_event(
                        &mut tls,
                        &StreamEvent::Progress(Progress {
                            request_id: request.request_id,
                            samples_done: 0,
                        }),
                        None,
                    );
                }
                Ok(ClientMessage::Cancel(cancel)) if asked == Some(cancel.request_id) => {
                    let done = Done {
                        request_id: cancel.request_id,
                        cancelled: true,
                        stats: Stats {
                            samples_done: 0,
                            requested_cadence_ms: 0,
                            effective_cadence_ms: 0,
                            reclaimed_samples: 0,
                        },
                    };
                    let _ = pleochroma_net::messages::write_stream_event(
                        &mut tls,
                        &StreamEvent::Done(done),
                        None,
                    );
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    });
}

/// A viewer that cancels stays connected and receives the worker's terminal
/// `DONE { cancelled: true }` under its own id; the connection then serves the next request.
#[test]
fn a_cancelling_viewer_that_stays_connected_gets_the_terminal_acknowledgement() {
    let pki = pki_with_server("beam-tiles-cancel");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    spawn_cancel_acknowledging_worker(handle.worker_addr.unwrap(), &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));
    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);

    send_request(&mut client, &request(71, scene(16, 16), vec![0, 1]));
    send(
        &mut client,
        &ClientMessage::Cancel(Cancel { request_id: 71 }),
    );
    loop {
        let (event, _) = pleochroma_net::messages::read_stream_event(&mut client).unwrap();
        match event {
            StreamEvent::Progress(_) => {}
            StreamEvent::Done(done) => {
                assert_eq!(done.request_id, 71, "the viewer's own id");
                assert!(done.cancelled);
                break;
            }
            other => panic!("expected the cancel acknowledgement, got {other:?}"),
        }
    }
}
