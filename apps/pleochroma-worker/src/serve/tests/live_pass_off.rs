//! A build without the `adaptive-sampling` feature refuses protocol v37 live passes through the
//! real connection handler: each request is answered with `UNSUPPORTED_REQUEST` naming the missing
//! feature, and the connection keeps serving the next one.

use super::fixtures::{DuplexHalf, test_db, tiny_scene};
use pleochroma::renderer::adaptive::LiveTiles;
use pleochroma_net::{
    SceneState, handshake,
    messages::{
        ClientMessage, LivePassRequest, LiveTileSet, StreamEvent, error_codes, write_hello_message,
        write_message,
    },
};
use std::io::Cursor;

/// A live pass of one tile of a 16 x 8 diamond scene.
fn request(request_id: u32) -> LivePassRequest {
    let scene = SceneState {
        width: 16,
        height: 8,
        ..tiny_scene()
    };
    let live = LiveTiles::from_indices(scene.width, scene.height, 8, vec![0]);
    LivePassRequest {
        request_id,
        scene,
        first_sample: 0,
        samples: 4,
        live: LiveTileSet::from_live(&live),
    }
}

/// Two live passes sent back to back are both refused with `UNSUPPORTED_REQUEST` (code 5) naming
/// their request ids and the feature, one frame each, and the connection then ends normally.
#[test]
fn a_build_without_adaptive_sampling_refuses_every_live_pass_and_keeps_serving() {
    let mut input = Vec::new();
    write_hello_message(&mut input, &handshake::local_hello_zoning()).unwrap();
    for request_id in [21, 22] {
        write_message(
            &mut input,
            &ClientMessage::LivePassRequest(Box::new(request(request_id))),
        )
        .unwrap();
    }
    let mut duplex = DuplexHalf::new(input);
    crate::serve::handle_connection(&mut duplex, 2, &test_db()).unwrap();
    let mut out = Cursor::new(duplex.out);
    // The WELCOME frame (and the zoning marker after it) is in its own frame.
    pleochroma_net::framing::read_frame(&mut out).unwrap();
    for request_id in [21, 22] {
        let (event, payload) = pleochroma_net::messages::read_stream_event(&mut out).unwrap();
        let StreamEvent::Error(error) = event else {
            panic!("expected an error, got {event:?}");
        };
        assert_eq!(error.code, error_codes::UNSUPPORTED_REQUEST);
        assert_eq!(error.code, 5, "UNSUPPORTED_REQUEST keeps its wire value");
        assert_eq!(error.request_id, Some(request_id));
        assert!(
            error.message.contains("adaptive-sampling"),
            "{}",
            error.message
        );
        assert!(payload.is_none());
    }
    assert_eq!(
        usize::try_from(out.position()).unwrap(),
        out.get_ref().len(),
        "exactly one frame per request was written"
    );
}
