//! Tests of live passes through the real connection handler.
//!
//! The reply carries the pass total
//! and the odd-sample total of every live pixel and equals the reference live pass, a zoned scene
//! keeps its zones, and a request that breaks a rule or names an unsupported scene is refused.

use super::fixtures::{DuplexHalf, test_db, tiny_scene};
use crate::render_core::live::reference_live_pass;
use pleochroma::{
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::GemMaterial,
        zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
    },
    renderer::{
        adaptive::LiveTiles,
        engine::{EngineSpec, RayOptions, RenderEngine},
    },
};
use pleochroma_net::{
    SceneState, handshake,
    messages::{
        ClientMessage, ErrorMsg, LivePassHeader, LivePassRequest, LiveTileSet, StreamEvent,
        ZoningPayload, decode_live_sums, error_codes, write_hello_message, write_message,
        write_zoning_payload,
    },
};
use std::io::Cursor;

/// A `width x height` diamond scene.
fn scene(width: u32, height: u32) -> SceneState {
    SceneState {
        width,
        height,
        ..tiny_scene()
    }
}

fn request(request_id: u32, scene: SceneState, tiles: Vec<u32>) -> LivePassRequest {
    let live = LiveTiles::from_indices(scene.width, scene.height, 8, tiles);
    LivePassRequest {
        request_id,
        scene,
        first_sample: 3,
        samples: 6,
        live: LiveTileSet::from_live(&live),
    }
}

/// The connection's replies to `messages` sent after a zoning `HELLO` (past the `WELCOME`).
///
/// Every message is on the wire before the server starts, so no request may follow one that is
/// traced: a live pass has no pipelining, and a request read while a pass traces is dropped
/// unanswered. Two traced passes go over a socket instead (see
/// `consecutive_passes_on_one_connection_add_up_to_the_longer_pass`).
fn run(messages: impl FnOnce(&mut Vec<u8>)) -> Cursor<Vec<u8>> {
    let mut input = Vec::new();
    write_hello_message(&mut input, &handshake::local_hello_zoning()).unwrap();
    messages(&mut input);
    let mut duplex = DuplexHalf::new(input);
    crate::serve::handle_connection(&mut duplex, 2, &test_db()).unwrap();
    let mut out = Cursor::new(duplex.out);
    // The WELCOME frame (and the zoning marker after it) is in its own frame.
    pleochroma_net::framing::read_frame(&mut out).unwrap();
    out
}

fn send<W: std::io::Write>(input: &mut W, request: &LivePassRequest) {
    write_message(
        input,
        &ClientMessage::LivePassRequest(Box::new(request.clone())),
    )
    .unwrap();
}

/// The one terminal event of a live pass and its payload, skipping heartbeats.
fn reply<R: std::io::Read>(out: &mut R) -> (StreamEvent, Option<Vec<u8>>) {
    loop {
        let (event, payload) = pleochroma_net::messages::read_stream_event(out).unwrap();
        if !matches!(event, StreamEvent::Progress(_)) {
            return (event, payload);
        }
    }
}

fn refusal(out: &mut Cursor<Vec<u8>>) -> ErrorMsg {
    match reply(out).0 {
        StreamEvent::Error(error) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_live_pass_reply_equals_the_reference_pass_of_every_live_pixel() {
    let request = request(11, scene(24, 16), vec![1, 4]);
    let mut out = run(|input| send(input, &request));
    let (event, payload) = reply(&mut out);
    let StreamEvent::LivePass(header) = event else {
        panic!("expected LIVE_PASS, got {event:?}");
    };
    assert_eq!(
        header,
        LivePassHeader {
            request_id: 11,
            first_sample: 3,
            samples: 6,
            live_pixels: 128,
            payload_len: 128 * 24,
        }
    );
    let got = decode_live_sums(&payload.unwrap(), 128).unwrap();
    let live = request.live_tiles().unwrap();
    assert_eq!(live.indices(), [1, 4]);
    assert_eq!(got, reference_live_pass(&request.scene, &live, (3, 6)));
    assert!(got.iter().any(|(sum, _)| sum.max_element() > 0.0));
    // First sample 3 is odd: the odd-sample total of a six-sample pass is samples 3, 5 and 7.
    assert!(got.iter().any(|(_, odd)| odd.max_element() > 0.0));
}

/// Two passes on one connection, sent one at a time.
///
/// The desktop's `LivePassClient` sends the next request only after the reply to the last one: a
/// live pass has no pipelining, and a request that arrives while a pass traces is dropped
/// (`connection::live_pass::absorb`). A loopback socket keeps that order, which the scripted
/// `DuplexHalf` cannot. Each reply equals its own reference pass.
#[test]
fn consecutive_passes_on_one_connection_add_up_to_the_longer_pass() {
    let first = request(1, scene(16, 8), vec![0, 1]);
    let second = LivePassRequest {
        request_id: 2,
        first_sample: 9,
        ..first.clone()
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        crate::serve::handle_connection(stream, 2, &test_db()).unwrap();
    });
    let mut client = std::net::TcpStream::connect(addr).unwrap();
    // A server that stops answering fails the test instead of hanging the step.
    client
        .set_read_timeout(Some(std::time::Duration::from_secs(60)))
        .unwrap();
    write_hello_message(&mut client, &handshake::local_hello_zoning()).unwrap();
    // The WELCOME frame (and the zoning marker after it) is in its own frame.
    pleochroma_net::framing::read_frame(&mut client).unwrap();
    let mut pairs = Vec::new();
    for pass in [&first, &second] {
        send(&mut client, pass);
        let (event, payload) = reply(&mut client);
        assert!(
            matches!(&event, StreamEvent::LivePass(h) if h.request_id == pass.request_id),
            "{event:?}"
        );
        pairs.push(decode_live_sums(&payload.unwrap(), 128).unwrap());
    }
    // Closing the connection ends the server's request loop cleanly.
    drop(client);
    server.join().unwrap();
    let live = first.live_tiles().unwrap();
    assert_eq!(pairs[0], reference_live_pass(&first.scene, &live, (3, 6)));
    assert_eq!(pairs[1], reference_live_pass(&first.scene, &live, (9, 6)));
}

fn zoned_material() -> GemMaterial {
    let band = |center: f32| {
        ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
            center, 60.0, 1.0,
        )]))
    };
    let mut zoned = ZonedAbsorption::new(band(640.0));
    zoned.zones.push(Zone {
        shape: ZoneShape::HalfSpace {
            normal: glam::DVec3::X,
            offset: 0.0,
        },
        absorption: band(450.0),
    });
    GemMaterial::diamond().with_zoning(zoned)
}

#[test]
fn a_zoned_live_pass_with_its_payload_keeps_its_zones() {
    let zoned = SceneState {
        material: zoned_material(),
        ..scene(16, 16)
    };
    let request = request(21, zoned.clone(), vec![0, 1, 2, 3]);
    let mut out = run(|input| {
        // The zones go first (they are `serde(skip)` in the scene), then the request.
        write_zoning_payload(input, &ZoningPayload::for_scene(21, &zoned).unwrap()).unwrap();
        send(input, &request);
    });
    let (event, payload) = reply(&mut out);
    assert!(matches!(event, StreamEvent::LivePass(_)), "{event:?}");
    let got = decode_live_sums(&payload.unwrap(), 256).unwrap();
    let live = request.live_tiles().unwrap();
    assert_eq!(got, reference_live_pass(&zoned, &live, (3, 6)));
    // Control: the same request without the zones renders a different picture.
    let mut stripped = zoned;
    stripped.material.zoning = None;
    assert_ne!(
        got,
        reference_live_pass(&stripped, &live, (3, 6)),
        "the zones changed the pass"
    );
}

#[test]
fn a_request_that_breaks_a_rule_is_refused_and_the_connection_keeps_serving() {
    let good = request(3, scene(16, 8), vec![0]);
    let mut no_samples = good.clone();
    no_samples.request_id = 1;
    no_samples.samples = 0;
    let mut bad_mask = good.clone();
    bad_mask.request_id = 2;
    bad_mask.live.mask = vec![0xFF, 0xFF, 0xFF];
    let mut out = run(|input| {
        send(input, &no_samples);
        send(input, &bad_mask);
        send(input, &good);
    });
    for expected_id in [1, 2] {
        let error = refusal(&mut out);
        assert_eq!(error.code, error_codes::VALIDATION_FAILED);
        assert_eq!(error.request_id, Some(expected_id));
    }
    let (event, _) = reply(&mut out);
    assert!(
        matches!(event, StreamEvent::LivePass(h) if h.request_id == 3),
        "the connection serves the next request: {event:?}"
    );
}

#[test]
fn the_ray_traced_engine_is_refused_as_unsupported() {
    let mut ray = scene(16, 8);
    ray.engine = EngineSpec {
        engine: RenderEngine::RayTraced,
        ray: RayOptions {
            aa_taps: 4,
            fire: true,
        },
    };
    let mut ray_request = request(5, ray, vec![0]);
    ray_request.samples = 2;
    ray_request.first_sample = 0;
    let mut out = run(|input| send(input, &ray_request));
    let error = refusal(&mut out);
    assert_eq!(error.code, error_codes::UNSUPPORTED_REQUEST);
    assert_eq!(error.request_id, Some(5));
}

/// Guards the beam gating through the real connection handler.
///
/// A build without `beam` answers a `BeamTileRequest` with `UNSUPPORTED_REQUEST` naming its
/// request id, and the connection serves the next request.
#[cfg(not(feature = "beam"))]
#[test]
fn a_beam_tile_request_is_refused_and_the_connection_keeps_serving() {
    let beams = pleochroma_net::messages::BeamTileRequest {
        request_id: 41,
        scene: scene(16, 16),
        tiles: LiveTileSet::from_live(&LiveTiles::from_indices(16, 16, 8, vec![0])),
        beam: pleochroma::renderer::accel_beam::BeamSettings::default(),
        adaptive: pleochroma::renderer::adaptive::AdaptiveSettings::provisional(16),
        sample_offset: 0,
    };
    let next = request(42, scene(16, 16), vec![0]);
    let mut out = run(|input| {
        write_message(input, &ClientMessage::BeamTileRequest(Box::new(beams))).unwrap();
        send(input, &next);
    });
    let error = refusal(&mut out);
    assert_eq!(error.code, error_codes::UNSUPPORTED_REQUEST);
    assert_eq!(error.request_id, Some(41));
    let (event, _) = reply(&mut out);
    assert!(
        matches!(&event, StreamEvent::LivePass(header) if header.request_id == 42),
        "the next request is served: {event:?}"
    );
}
