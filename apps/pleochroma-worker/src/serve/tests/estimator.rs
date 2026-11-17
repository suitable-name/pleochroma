//! A scene that asks for exit collection (protocol 42, `scene.estimator`) is served with it
//! through the real request loop: the frame equals the one-shot collect trace of the same samples
//! and differs from the branching frame of the same scene.

use super::fixtures::{DuplexHalf, final_only, read_stream_until_done, test_db, tiny_scene};
use crate::{
    cli::ComputeMode,
    render_core,
    serve::connection::{ViewerContext, handle_viewer_connection},
};
use glam::Vec3;
use pleochroma::{optics::raytracer::Estimator, renderer::gpu_backend::GpuBackend};
use pleochroma_net::{
    SceneState, handshake,
    messages::{
        ClientMessage, LOOPBACK_SERVER_PREFERENCE, RenderRequest, RequestIntent, StreamEvent,
        Welcome, write_message,
    },
    radiance,
};
use std::{io::Cursor, sync::Arc};

/// Samples per request.
const SAMPLES: u32 = 4;

/// A frame with enough stone pixels for the two estimators to differ.
fn scene(estimator: Estimator) -> SceneState {
    SceneState {
        width: 16,
        height: 16,
        estimator,
        ..tiny_scene()
    }
}

/// Serves one `FinalOnly` request for `scene` on a CPU-only worker and returns its frame's sums.
fn served_frame(scene: &SceneState) -> Vec<Vec3> {
    let mut input = Vec::new();
    write_message(&mut input, &handshake::local_hello()).unwrap();
    write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(RenderRequest {
            request_id: 1,
            scene: scene.clone(),
            first_sample: 0,
            samples: SAMPLES,
            stream: final_only(0),
            intent: RequestIntent::Batch,
        })),
    )
    .unwrap();
    let mut duplex = DuplexHalf::new(input);
    let db = test_db();
    handle_viewer_connection(
        &mut duplex,
        &ViewerContext {
            threads: 2,
            gpu: &Arc::new(GpuBackend::disabled()),
            db: &db,
            compute_mode: ComputeMode::OnlyCpu,
            encodings: &LOOPBACK_SERVER_PREFERENCE,
            link: None,
            own_lane: true,
            registry: None,
            cert_role: None,
            coordinator: None,
            assets: None,
            jobs: None,
            admitted: None,
        },
    )
    .unwrap();
    let mut out = Cursor::new(duplex.out);
    let _welcome: Welcome = pleochroma_net::messages::read_message(&mut out).unwrap();
    let events = read_stream_until_done(&mut out);
    let (_, payload) = events
        .iter()
        .find(|(event, _)| matches!(event, StreamEvent::Frame(_)))
        .expect("a FRAME");
    radiance::decode(payload.as_deref().unwrap(), scene.width, scene.height).unwrap()
}

/// Whether two frames agree to a relative 1e-6 in every pixel.
fn agree(a: &[Vec3], b: &[Vec3]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            let scale = x.abs().max(y.abs()).max(Vec3::splat(1e-6));
            ((*x - *y).abs() / scale).max_element() <= 1e-6
        })
}

#[test]
fn a_collect_scene_is_served_with_exit_collection() {
    let collect = scene(Estimator::ExitCollection);
    let branching = scene(Estimator::Branching);
    let served = served_frame(&collect);
    let reference = render_core::trace_samples(&collect, 0, SAMPLES, 2);
    assert!(
        agree(&served, &reference),
        "the served frame is the collect trace"
    );
    let plain = render_core::trace_samples(&branching, 0, SAMPLES, 2);
    assert!(
        !agree(&served, &plain),
        "the served frame is not the branching trace"
    );
    assert!(
        agree(&served_frame(&branching), &plain),
        "a branching scene is still served as branching"
    );
}
