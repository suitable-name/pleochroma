//! Beam tiles through the real connection handler.
//!
//! The reply equals the local render of the
//! same tiles, and a request that breaks a rule, goes past the pixel cap, names an unsupported
//! scene or a stone the beams refuse is answered with its reason.

use super::fixtures::{DuplexHalf, test_db, tiny_scene};
use crate::render_core::beam::{BeamTilesOutcome, reference_beam_tiles};
use pleochroma::renderer::{
    accel_beam::{BeamOff, BeamSettings},
    adaptive::{AdaptiveSettings, LiveTiles, NoiseMetric},
    engine::{EngineSpec, RayOptions, RenderEngine},
};
use pleochroma_net::{
    SceneState, handshake,
    messages::{
        BeamTileRequest, ClientMessage, ErrorMsg, LiveTileSet, MAX_BEAM_TILE_PIXELS, StreamEvent,
        decode_beam_tiles_reply, error_codes, write_hello_message, write_message,
    },
};
use std::io::Cursor;

/// The tile edge of the requests below (also the adaptive tile).
const TILE: u32 = 8;

/// A `width x height` diamond scene.
fn scene(width: u32, height: u32) -> SceneState {
    SceneState {
        width,
        height,
        ..tiny_scene()
    }
}

/// Small adaptive settings with an eight-pixel tile.
const fn adaptive() -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise: 0.5,
        metric: NoiseMetric::RelativeLstar,
        min_spp: 8,
        max_spp: 16,
        pass_spp: 8,
        tile: TILE,
    }
}

/// A coarse tree: these tests assert bit identity and refusals, which hold at any tree
/// resolution, while the defaults (48 spectral nodes, depth 24) make the tree build dominate
/// the run. The node count stays a multiple of 8 so the request is valid.
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

/// The connection's replies to `messages` sent after a zoning `HELLO` (past the `WELCOME`).
fn run(messages: impl FnOnce(&mut Vec<u8>)) -> Cursor<Vec<u8>> {
    let mut input = Vec::new();
    write_hello_message(&mut input, &handshake::local_hello_zoning()).unwrap();
    messages(&mut input);
    let mut duplex = DuplexHalf::new(input);
    crate::serve::handle_connection(&mut duplex, 2, &test_db()).unwrap();
    let mut out = Cursor::new(duplex.out);
    pleochroma_net::framing::read_frame(&mut out).unwrap();
    out
}

fn send(input: &mut Vec<u8>, request: &BeamTileRequest) {
    write_message(
        input,
        &ClientMessage::BeamTileRequest(Box::new(request.clone())),
    )
    .unwrap();
}

/// The one terminal event of a request and its payload, skipping heartbeats.
fn reply(out: &mut Cursor<Vec<u8>>) -> (StreamEvent, Option<Vec<u8>>) {
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

/// The reply is the local render of the same tiles, bit for bit (CPU on both sides).
#[test]
fn a_beam_tile_reply_equals_the_local_render_of_the_same_tiles() {
    let request = request(11, scene(24, 16), vec![1, 4]);
    let mut out = run(|input| send(input, &request));
    let (event, payload) = reply(&mut out);
    let (header, pixels) = decode_beam_tiles_reply(&event, &payload.unwrap()).unwrap();
    assert_eq!(header.request_id, 11);
    assert_eq!(header.sample_offset, 0);
    assert_eq!(header.pixels, 128);
    let live = request.live_tiles().unwrap();
    let BeamTilesOutcome::Done(local) = reference_beam_tiles(&request, &live) else {
        panic!("the local render must succeed too");
    };
    assert_eq!(pixels, local.pixels, "bit for bit");
    assert!(pixels.iter().any(|p| p.xyz.max_element() > 0.0));
}

/// A request over the pixel cap is refused before any work, with the cap in the reason.
#[test]
fn a_request_over_the_pixel_cap_is_refused_and_the_connection_keeps_serving() {
    // 2304 x 2048 = 4.5 Mi pixels, every tile requested: past the 4 Mi cap.
    let wide = scene(2304, 2048);
    let live = LiveTiles::all(wide.width, wide.height, TILE);
    assert!(live.pixel_count() > MAX_BEAM_TILE_PIXELS);
    let over = BeamTileRequest {
        request_id: 1,
        tiles: LiveTileSet::from_live(&live),
        ..request(1, wide, vec![0])
    };
    let good = request(2, scene(16, 8), vec![0]);
    let mut out = run(|input| {
        send(input, &over);
        send(input, &good);
    });
    let error = refusal(&mut out);
    assert_eq!(error.code, error_codes::VALIDATION_FAILED);
    assert_eq!(error.request_id, Some(1));
    // The next request is answered (rendered or refused by the beams, never dropped).
    let (event, _) = reply(&mut out);
    let id = match &event {
        StreamEvent::BeamTiles(header) => header.request_id,
        StreamEvent::Error(error) => error.request_id.unwrap_or(0),
        other => panic!("unexpected reply {other:?}"),
    };
    assert_eq!(id, 2);
}

/// Settings, tile sets and tile edges that break a rule are refused as invalid.
#[test]
fn a_request_that_breaks_a_rule_is_refused_with_validation_failed() {
    let good = request(3, scene(16, 8), vec![0]);
    let mut bad_nodes = good.clone();
    bad_nodes.request_id = 1;
    bad_nodes.beam.spectral_nodes = 12;
    let mut bad_mask = good.clone();
    bad_mask.request_id = 2;
    bad_mask.tiles.mask = vec![0xFF, 0xFF, 0xFF];
    let mut other_tile = good.clone();
    other_tile.request_id = 4;
    other_tile.adaptive.tile = 16;
    // The residual loop is bounded by the live-pass sample cap, so an over-cap `max_spp` is
    // refused. A residual minimum above `max_spp` is clamped, as in a local render, not refused.
    let mut endless = good;
    endless.request_id = 5;
    endless.adaptive.max_spp = crate::validate::MAX_SAMPLES_PER_REQUEST + 1;
    let mut out = run(|input| {
        send(input, &bad_nodes);
        send(input, &bad_mask);
        send(input, &other_tile);
        send(input, &endless);
    });
    for expected_id in [1, 2, 4, 5] {
        let error = refusal(&mut out);
        assert_eq!(error.code, error_codes::VALIDATION_FAILED);
        assert_eq!(error.request_id, Some(expected_id));
    }
}

/// The Ray-Traced engine has no residual: refused as unsupported with the beam sentence.
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
    let ray_request = request(5, ray, vec![0]);
    let mut out = run(|input| send(input, &ray_request));
    let error = refusal(&mut out);
    assert_eq!(error.code, error_codes::UNSUPPORTED_REQUEST);
    assert_eq!(error.request_id, Some(5));
    assert_eq!(error.message, BeamOff::RayTracedResidual.message());
}

/// A scene with the Stone Shadow is traced without beams: the tiles are refused with
/// `BeamOff::StoneShadow`'s sentence, so the coordinator renders them itself (with the shadow).
#[test]
fn a_scene_with_the_stone_shadow_is_refused_with_the_beam_off_message() {
    let mut shadowed = scene(16, 8);
    shadowed.stone_shadow = true;
    shadowed.backdrop = 1.0;
    let shadowed_request = request(7, shadowed, vec![0]);
    let mut out = run(|input| send(input, &shadowed_request));
    let error = refusal(&mut out);
    assert_eq!(error.code, error_codes::UNSUPPORTED_REQUEST);
    assert_eq!(error.request_id, Some(7));
    assert_eq!(error.message, BeamOff::StoneShadow.message());
}

/// A scene with the Tray Reflection is traced without beams too: the tiles are refused with
/// `BeamOff::TrayReflection`'s sentence, so the coordinator renders them itself.
#[test]
fn a_scene_with_the_tray_reflection_is_refused_with_the_beam_off_message() {
    let mut mirrored = scene(16, 8);
    mirrored.tray_reflection = 0.04;
    mirrored.backdrop = 1.0;
    let mirrored_request = request(8, mirrored, vec![0]);
    let mut out = run(|input| send(input, &mirrored_request));
    let error = refusal(&mut out);
    assert_eq!(error.code, error_codes::UNSUPPORTED_REQUEST);
    assert_eq!(error.request_id, Some(8));
    assert_eq!(error.message, BeamOff::TrayReflection.message());
}

/// A stone the beams refuse (here: a frosted girdle) is answered with its `BeamOff` sentence, so
/// the coordinator renders those tiles itself. Each `BeamOff` sentence starts with
/// "Beam tracing".
#[test]
fn a_stone_the_beams_refuse_gets_the_beam_off_message() {
    let mut frosted = scene(16, 8);
    frosted.girdle_frosted = true;
    let frosted_request = request(6, frosted, vec![0]);
    let mut out = run(|input| send(input, &frosted_request));
    let error = refusal(&mut out);
    assert_eq!(error.code, error_codes::UNSUPPORTED_REQUEST);
    assert_eq!(error.request_id, Some(6));
    assert!(
        error.message.starts_with("Beam tracing"),
        "the BeamOff sentence: {}",
        error.message
    );
}
