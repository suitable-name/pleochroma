//! Adaptive live passes through a real coordinator on loopback with a joined CPU worker.
//!
//! The coordinator holds no lane of its own, so every pass is forwarded whole to the worker and the
//! reply relayed under the viewer's `request_id`.

use super::{
    fixtures::{bundle, coordinator_args, pki_with_server, start, tls_client, wait_for},
    support::{ViewerStream, close, scene, send, spawn_worker, viewer},
};
use crate::{coordinator::LivenessConfig, render_core::live::reference_live_pass};
use glam::Vec3;
use pleochroma::{
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::GemMaterial,
        zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
    },
    renderer::adaptive::{AdaptiveRun, AdaptiveSettings, NoiseMetric, Pass},
};
use pleochroma_net::{
    SceneState,
    client::handshake_with_hello,
    handshake,
    messages::{
        ClientMessage, ErrorMsg, LivePassRequest, LiveTileSet, PeerRole, StreamEvent,
        ZoningPayload, decode_live_sums, error_codes, scatter_live, write_zoning_payload,
    },
};
use std::{sync::Arc, time::Duration};

const WAIT: Duration = Duration::from_secs(20);

/// Eight-pixel tiles and eight-sample passes: a small frame has many tiles and a few checks.
fn settings() -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise: 0.5,
        metric: NoiseMetric::RelativeLstar,
        min_spp: 16,
        max_spp: 32,
        pass_spp: 8,
        tile: 8,
    }
}

/// Sends `pass` of `scene` as request `request_id` and returns the live pixels' pairs, or the
/// worker's refusal.
fn remote_pass(
    client: &mut ViewerStream,
    request_id: u32,
    scene: &SceneState,
    pass: &Pass,
) -> Result<Vec<(Vec3, Vec3)>, ErrorMsg> {
    send(
        client,
        &ClientMessage::LivePassRequest(Box::new(LivePassRequest {
            request_id,
            scene: scene.clone(),
            first_sample: pass.first_sample,
            samples: pass.spp,
            live: LiveTileSet::from_live(&pass.live),
        })),
    );
    loop {
        let (event, payload) = pleochroma_net::messages::read_stream_event(client).unwrap();
        match event {
            StreamEvent::Progress(_) => {}
            StreamEvent::LivePass(header) => {
                assert_eq!(header.request_id, request_id, "the viewer's own id");
                assert_eq!(header.first_sample, pass.first_sample);
                assert_eq!(header.samples, pass.spp);
                let pixels = usize::try_from(pass.live.pixel_count()).unwrap();
                assert_eq!(header.live_pixels as usize, pixels);
                return Ok(decode_live_sums(&payload.unwrap(), pixels).unwrap());
            }
            StreamEvent::Error(error) => return Err(error),
            other => panic!("unexpected reply to a live pass: {other:?}"),
        }
    }
}

/// An adaptive render driven the way the export thread drives it, with every pass traced by
/// `trace`: the live-tile sequence, the per-pixel means and the per-pixel counts.
fn adaptive(
    scene: &SceneState,
    mut trace: impl FnMut(&Pass) -> Vec<(Vec3, Vec3)>,
) -> (Vec<Vec<u32>>, Vec<Vec3>, Vec<u32>) {
    let mut run = AdaptiveRun::new(settings(), scene.width, scene.height).unwrap();
    let pixels = (scene.width * scene.height) as usize;
    let (mut sum, mut half_b) = (vec![Vec3::ZERO; pixels], vec![Vec3::ZERO; pixels]);
    let mut sequence = Vec::new();
    while let Some(pass) = run.next_pass() {
        sequence.push(pass.live.indices().to_vec());
        let traced = trace(&pass);
        assert!(scatter_live(&pass.live, &traced, &mut sum, &mut half_b));
        run.finish_pass(&sum, &half_b, 1.0);
    }
    (sequence, run.resolve(&sum), run.pixel_counts())
}

/// An adaptive render through a coordinator and a joined worker is the local adaptive render of
/// the same scene: the same live tiles in every pass, so the same per-pixel counts, and the same
/// image (CPU on both sides, so no GPU twin tolerance is in play).
#[test]
fn an_adaptive_render_through_a_joined_worker_equals_the_local_adaptive_render() {
    let pki = pki_with_server("live-pass-e2e");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    spawn_worker(handle.worker_addr.unwrap(), &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));
    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);

    let image = scene(40, 24);
    let mut next_id = 100;
    let (remote_sequence, remote_image, remote_counts) = adaptive(&image, |pass| {
        next_id += 1;
        remote_pass(&mut client, next_id, &image, pass).expect("the worker traces the pass")
    });
    let (local_sequence, local_image, local_counts) = adaptive(&image, |pass| {
        reference_live_pass(&image, &pass.live, (pass.first_sample, pass.spp))
    });

    assert_eq!(
        remote_sequence, local_sequence,
        "the same tiles are live in every pass"
    );
    assert!(remote_sequence.len() >= 2, "at least the minimum's passes");
    assert_eq!(remote_counts, local_counts);
    assert!(close(&remote_image, &local_image), "the images agree");
    assert!(
        remote_image.iter().any(|v| *v != Vec3::ZERO),
        "something was traced"
    );
}

/// A bicolor stone: the base absorbs red, a half space (+X side) absorbs blue.
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

/// A zoned live pass through a render-less coordinator keeps its zones: the coordinator
/// re-attaches the zones the viewer sent ahead of the request and sends them again ahead of
/// the joined worker's copy. The pass equals the zoned reference and differs from the unzoned.
#[test]
fn a_zoned_live_pass_through_a_joined_worker_keeps_its_zones() {
    let pki = pki_with_server("live-pass-zoned");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    spawn_worker(handle.worker_addr.unwrap(), &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));

    let mut client = tls_client(handle.viewer_addr, &viewer_bundle);
    let welcome = handshake_with_hello(&mut client, &handshake::local_hello_zoning()).unwrap();
    assert!(welcome.zoning, "a coordinator advertises zoning");

    let zoned = SceneState {
        material: zoned_material(),
        ..scene(16, 16)
    };
    let live = pleochroma::renderer::adaptive::LiveTiles::all(16, 16, 8);
    let pass = Pass {
        first_sample: 4,
        spp: 6,
        live: live.clone(),
    };
    write_zoning_payload(&mut client, &ZoningPayload::for_scene(31, &zoned).unwrap()).unwrap();
    let got = remote_pass(&mut client, 31, &zoned, &pass).expect("the worker traces the pass");
    assert_eq!(got.len(), 256);
    // CPU on both sides: the worker's pass is the reference pass, bit for bit.
    assert_eq!(got, reference_live_pass(&zoned, &live, (4, 6)));
    let mut stripped = zoned;
    stripped.material.zoning = None;
    assert_ne!(
        got,
        reference_live_pass(&stripped, &live, (4, 6)),
        "the zones change the pass"
    );
}

/// A request the coordinator can see is wrong is refused by the coordinator itself, with the
/// viewer's id, and the connection stays usable.
#[test]
fn a_bad_live_pass_is_refused_by_the_coordinator_and_the_connection_keeps_serving() {
    let pki = pki_with_server("live-pass-refused");
    let viewer_bundle = bundle(&pki, "laptop", PeerRole::Viewer);
    let worker_bundle = bundle(&pki, "box", PeerRole::Worker);
    let handle = start(&coordinator_args(&pki), LivenessConfig::default());
    let registry = Arc::clone(handle.registry.as_ref().unwrap());
    spawn_worker(handle.worker_addr.unwrap(), &worker_bundle);
    assert!(wait_for(WAIT, || registry.capacity().workers == 1));
    let (_, mut client) = viewer(handle.viewer_addr, &viewer_bundle);

    let image = scene(16, 16);
    let mut bad = LivePassRequest {
        request_id: 41,
        scene: image.clone(),
        first_sample: 0,
        samples: 4,
        live: LiveTileSet::from_live(&pleochroma::renderer::adaptive::LiveTiles::all(16, 16, 8)),
    };
    bad.live.mask = vec![0xFF; 5];
    send(&mut client, &ClientMessage::LivePassRequest(Box::new(bad)));
    let (event, _) = pleochroma_net::messages::read_stream_event(&mut client).unwrap();
    let StreamEvent::Error(error) = event else {
        panic!("expected an ERROR, got {event:?}");
    };
    assert_eq!(error.code, error_codes::VALIDATION_FAILED);
    assert_eq!(error.request_id, Some(41));

    // The connection and the worker are still good.
    let pass = Pass {
        first_sample: 0,
        spp: 2,
        live: pleochroma::renderer::adaptive::LiveTiles::all(16, 16, 8),
    };
    let ok = remote_pass(&mut client, 42, &image, &pass).expect("the next pass is served");
    assert_eq!(ok.len(), 256);
}
