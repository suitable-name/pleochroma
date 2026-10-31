//! Extended stream-event and batch wire-layout tests.
use super::*;

/// Every v14 event round-trips through `write_stream_event`/`read_stream_event`, the
/// payload-carrying ones together with their raw payload frame.
#[test]
fn v14_stream_events_round_trip_with_and_without_payloads() {
    use crate::messages::{DisplayEncoding, PayloadEncoding, RenderCapability};

    let payload = vec![7u8; 40];
    let with_payload = [
        StreamEvent::Frame(FrameHeader {
            request_id: 2,
            first_sample: 0,
            samples: 8,
            payload_len: 40,
            encoding: PayloadEncoding::ShuffleZstd { level: 1 },
            raw_len: 48,
        }),
        StreamEvent::Preview(PreviewHeader {
            request_id: 2,
            width: 2,
            height: 2,
            samples_done: 8,
            payload_len: 40,
            encoding: PayloadEncoding::ShuffleLz4,
            raw_len: 48,
        }),
        StreamEvent::DisplayFrame(DisplayFrameHeader {
            request_id: 2,
            samples_done: 8,
            width: 5,
            height: 2,
            encoding: DisplayEncoding::Rgba8,
            payload_len: 40,
        }),
        StreamEvent::FinalImage(FinalImageHeader {
            request_id: 2,
            width: 5,
            height: 2,
            samples_done: 8,
            encoding: DisplayEncoding::Png,
            payload_len: 40,
        }),
    ];
    for event in with_payload {
        assert_eq!(event.payload_len(), Some(40));
        assert_eq!(event.request_id(), Some(2));
        let mut buf = Vec::new();
        write_stream_event(&mut buf, &event, Some(&payload)).unwrap();
        let (decoded, decoded_payload) = read_stream_event(&mut std::io::Cursor::new(buf)).unwrap();
        assert_eq!(decoded, event);
        assert_eq!(decoded_payload.as_deref(), Some(payload.as_slice()));
    }

    let without_payload = [
        StreamEvent::Pong {
            nonce: 0xDEAD_BEEF_0000_0001,
        },
        StreamEvent::CapabilityChanged { render: None },
        StreamEvent::CapabilityChanged {
            render: Some(RenderCapability {
                backend: crate::messages::Backend::Coordinator {
                    workers: 2,
                    threads: 32,
                    gpus: 1,
                },
                max_pixels: 33_177_600,
                min_cadence_ms: 100,
                hdr: false,
            }),
        },
        StreamEvent::NeedAsset {
            content_hash: [0xAB; 32],
        },
    ];
    for event in without_payload {
        assert_eq!(event.payload_len(), None);
        assert_eq!(event.request_id(), None);
        let mut buf = Vec::new();
        write_stream_event(&mut buf, &event, None).unwrap();
        let (decoded, decoded_payload) = read_stream_event(&mut std::io::Cursor::new(buf)).unwrap();
        assert_eq!(decoded, event);
        assert_eq!(decoded_payload, None);
    }
}

/// Pins the `StreamEvent` discriminants: the five v13 variants keep 0..=4 and the v14
/// ones are appended in order.
#[test]
fn stream_event_discriminants_are_append_only() {
    let first_byte = |e: &StreamEvent| postcard::to_allocvec(e).unwrap()[0];
    assert_eq!(
        first_byte(&StreamEvent::Frame(FrameHeader::for_payload(1, 0, 1, &[]))),
        0
    );
    assert_eq!(
        first_byte(&StreamEvent::Preview(PreviewHeader::for_payload(
            1,
            0,
            0,
            0,
            &[]
        ))),
        1
    );
    assert_eq!(
        first_byte(&StreamEvent::Progress(Progress {
            request_id: 1,
            samples_done: 0
        })),
        2
    );
    assert_eq!(
        first_byte(&StreamEvent::Error(ErrorMsg {
            code: 1,
            message: String::new(),
            request_id: None,
        })),
        4
    );
    assert_eq!(first_byte(&StreamEvent::Pong { nonce: 1 }), 5);
    assert_eq!(
        first_byte(&StreamEvent::CapabilityChanged { render: None }),
        8
    );
    assert_eq!(
        first_byte(&StreamEvent::NeedAsset {
            content_hash: [0; 32]
        }),
        9
    );
}

/// `Asset` (HDR maps) is appended after `FinalImageRequest` (index 6), and the v14
/// scene environment keeps `Studio` at 0 with `Hdr` appended at 1.
#[cfg(feature = "render")]
#[test]
fn asset_and_scene_environment_discriminants_are_pinned() {
    use crate::{
        messages::AssetHeader,
        scene::{HdrEnvironment, SceneEnvironment},
    };
    let asset = ClientMessage::Asset(AssetHeader {
        content_hash: [3; 32],
        len: 1024,
    });
    assert_eq!(postcard::to_allocvec(&asset).unwrap()[0], 6);
    let mut buf = Vec::new();
    write_message(&mut buf, &asset).unwrap();
    let decoded: ClientMessage = read_message(&mut std::io::Cursor::new(buf)).unwrap();
    assert_eq!(decoded, asset);

    assert_eq!(
        postcard::to_allocvec(&SceneEnvironment::Studio).unwrap(),
        [0]
    );
    let hdr = SceneEnvironment::Hdr(HdrEnvironment {
        content_hash: [5; 32],
        width: 2048,
        height: 1024,
    });
    assert_eq!(postcard::to_allocvec(&hdr).unwrap()[0], 1);
}

/// `Ping` and `FinalImageRequest` are appended after `TiltCurvesRequest` (indices 4/5)
/// and round-trip.
#[cfg(feature = "render")]
#[test]
fn ping_and_final_image_request_round_trip_at_discriminants_4_and_5() {
    use crate::messages::{FinalImageRequest, FinalOutput, WireColorSpace};
    use pleochroma::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };

    let ping = ClientMessage::Ping { nonce: 77 };
    let ping_bytes = postcard::to_allocvec(&ping).unwrap();
    assert_eq!(ping_bytes[0], 4, "Ping must be discriminant 4");
    let mut buf = Vec::new();
    write_message(&mut buf, &ping).unwrap();
    let decoded: ClientMessage = read_message(&mut std::io::Cursor::new(buf)).unwrap();
    assert_eq!(decoded, ping);

    let scene = crate::scene::SceneState {
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        width: 2,
        height: 2,
        yaw: 0.0,
        camera_roll: 0.0,
        pitch: 0.0,
        distance: 1.0,
        light_yaw: 0.0,
        light_pitch: 0.0,
        exposure: 1.0,
        max_bounces: 1,
        lighting_preset: LightingPreset::Daylight,
        material: GemMaterial::diamond(),
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: 0.0,
        environment: crate::scene::SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: pleochroma::optics::fluorescence::Fluorescence::default(),
        head_shadow_deg: 16.0,
        frosted_planes: Vec::new(),
        effects: crate::scene::effects::SceneEffects::default(),
    };
    let request = ClientMessage::FinalImageRequest(Box::new(FinalImageRequest {
        request_id: 9,
        scene,
        first_sample: 0,
        samples: 16,
        width: 2,
        height: 2,
        color_space: WireColorSpace::DisplayP3,
        output: FinalOutput::PngRgba8,
        viewer_samples: 0,
    }));
    let request_bytes = postcard::to_allocvec(&request).unwrap();
    assert_eq!(
        request_bytes[0], 5,
        "FinalImageRequest must be discriminant 5"
    );
    let mut buf = Vec::new();
    write_message(&mut buf, &request).unwrap();
    let decoded: ClientMessage = read_message(&mut std::io::Cursor::new(buf)).unwrap();
    assert_eq!(decoded, request);
}

/// v16: `Contribution` is appended after `Asset` (index 6), at discriminant 7, and
/// round-trips through the tagged `ClientMessage` envelope.
#[cfg(feature = "render")]
#[test]
fn contribution_is_appended_at_discriminant_7_and_round_trips() {
    use crate::messages::ContributionHeader;

    let header = ContributionHeader {
        request_id: 4,
        first_sample: 10,
        samples: 2,
        width: 2,
        height: 2,
        encoding: crate::messages::PayloadEncoding::Raw,
        payload_len: 48,
        raw_len: 48,
    };
    let contribution = ClientMessage::Contribution(header);
    let bytes = postcard::to_allocvec(&contribution).unwrap();
    assert_eq!(bytes[0], 7, "Contribution must be discriminant 7");

    let mut buf = Vec::new();
    write_message(&mut buf, &contribution).unwrap();
    let decoded: ClientMessage = read_message(&mut std::io::Cursor::new(buf)).unwrap();
    assert_eq!(decoded, contribution);
}

/// v24: the batch events are appended after `NeedAsset` (indices 10 to 13), carry their
/// batch's `request_id`, and only `BatchItemDone` has a payload frame (the PNG).
#[test]
fn v24_batch_events_are_appended_and_round_trip() {
    use crate::messages::{BatchDone, BatchItemDoneHeader, BatchItemFailed, BatchItemProgress};

    let first_byte = |e: &StreamEvent| postcard::to_allocvec(e).unwrap()[0];
    let png = vec![0x89, b'P', b'N', b'G', 1, 2, 3];
    let done = StreamEvent::BatchItemDone(BatchItemDoneHeader {
        request_id: 6,
        item_id: 3,
        samples_done: 64,
        payload_len: u32::try_from(png.len()).unwrap(),
    });
    let progress = StreamEvent::BatchItemProgress(BatchItemProgress {
        request_id: 6,
        item_id: 3,
        samples_done: 64,
    });
    let failed = StreamEvent::BatchItemFailed(BatchItemFailed {
        request_id: 6,
        item_id: 4,
        reason: "the scene names an HDR environment".to_owned(),
    });
    let batch_done = StreamEvent::BatchDone(BatchDone {
        request_id: 6,
        cancelled: true,
    });
    assert_eq!(first_byte(&progress), 10);
    assert_eq!(first_byte(&done), 11);
    assert_eq!(first_byte(&failed), 12);
    assert_eq!(first_byte(&batch_done), 13);

    for (event, payload) in [
        (&progress, None),
        (&done, Some(png.as_slice())),
        (&failed, None),
        (&batch_done, None),
    ] {
        assert_eq!(event.request_id(), Some(6));
        assert_eq!(
            event.payload_len(),
            payload.map(|p| u32::try_from(p.len()).unwrap())
        );
        let mut buf = Vec::new();
        write_stream_event(&mut buf, event, payload).unwrap();
        let (decoded, decoded_payload) = read_stream_event(&mut std::io::Cursor::new(buf)).unwrap();
        assert_eq!(&decoded, event);
        assert_eq!(decoded_payload.as_deref(), payload);
    }

    // A payload handed to a payload-less batch event is refused before anything is written.
    let mut buf = Vec::new();
    assert!(write_stream_event(&mut buf, &batch_done, Some(png.as_slice())).is_err());
    assert_eq!(buf, [] as [u8; 0]);
}

/// v24: `BatchRenderRequest` is appended after `Contribution` (index 7), at discriminant
/// 8, and round-trips through the tagged `ClientMessage` envelope.
#[cfg(feature = "render")]
#[test]
fn batch_render_request_is_appended_at_discriminant_8_and_round_trips() {
    use crate::messages::{BatchItem, BatchRenderRequest, BatchReply};
    use pleochroma::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };

    let scene = crate::scene::SceneState {
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        width: 2,
        height: 2,
        yaw: 0.0,
        camera_roll: 0.0,
        pitch: 0.0,
        distance: 1.0,
        light_yaw: 0.0,
        light_pitch: 0.0,
        exposure: 1.0,
        max_bounces: 1,
        lighting_preset: LightingPreset::Daylight,
        material: GemMaterial::diamond(),
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: 0.0,
        environment: crate::scene::SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: pleochroma::optics::fluorescence::Fluorescence::default(),
        head_shadow_deg: 16.0,
        frosted_planes: Vec::new(),
        effects: crate::scene::effects::SceneEffects::default(),
    };
    let request = ClientMessage::BatchRenderRequest(Box::new(BatchRenderRequest {
        request_id: 12,
        reply: BatchReply::FinalPng,
        items: vec![BatchItem {
            item_id: 0,
            scene,
            first_sample: 0,
            samples: 16,
            width: 2,
            height: 2,
        }],
    }));
    assert_eq!(
        postcard::to_allocvec(&request).unwrap()[0],
        8,
        "BatchRenderRequest must be discriminant 8"
    );
    let mut buf = Vec::new();
    write_message(&mut buf, &request).unwrap();
    let decoded: ClientMessage = read_message(&mut std::io::Cursor::new(buf)).unwrap();
    assert_eq!(decoded, request);
}

/// v16: `Stats.reclaimed_samples` round-trips through `DONE` for a nonzero value (a
/// zero one is already covered by `done_round_trips_both_cancelled_states`).
#[test]
fn done_stats_round_trip_with_reclaimed_samples() {
    let done = Done {
        request_id: 3,
        cancelled: false,
        stats: Stats {
            samples_done: 40,
            requested_cadence_ms: 250,
            effective_cadence_ms: 260,
            reclaimed_samples: 6,
        },
    };
    let mut buf = Vec::new();
    write_message(&mut buf, &done).unwrap();
    let decoded: Done = read_message(&mut std::io::Cursor::new(buf)).unwrap();
    assert_eq!(decoded, done);
    assert_eq!(decoded.stats.reclaimed_samples, 6);
}
