//! The exposure of an HDR-lit job: it survives the job file round trip and lights the map.
//!
//! Two jobs that differ only in their exposure must render different pictures, and the same
//! job must render the same bytes twice. Every render is CPU-only and tiny, through the export
//! the job executor runs.

use super::{
    convert,
    tests::{hdr_job, render_png, test_dir, write_test_hdr},
};
use crate::bridge::{
    export_thread::SceneSnapshot, remote::hdr_asset, render_thread::RenderContext,
};
use pleochroma::renderer::env_map::HdrOrientation;
use pleochroma_render_jobs::{codec, job::format_for};
use std::sync::{Arc, Mutex};

/// The mean 8-bit channel value of an encoded picture.
fn mean_level(png: &[u8]) -> f64 {
    let image = image::load_from_memory(png)
        .expect("the export is a PNG")
        .to_rgb8();
    let raw = image.as_raw();
    raw.iter().map(|&c| f64::from(c)).sum::<f64>() / raw.len() as f64
}

#[test]
fn an_hdr_job_keeps_its_exposure_and_is_lit_by_it() {
    let dir = test_dir("hdr_exposure");
    let path = write_test_hdr(&dir);
    let hash = pleochroma_net::messages::content_hash(&std::fs::read(&path).unwrap());
    let snapshot_at = |exposure: f32| -> SceneSnapshot {
        let mut job = hdr_job(&path, &hash);
        job.scene.exposure = exposure;
        let job = codec::from_text(&codec::to_text(&job).expect("the job encodes"))
            .expect("the job decodes");
        assert_eq!(
            job.scene.exposure.to_bits(),
            exposure.to_bits(),
            "the job file keeps the exposure"
        );
        let map = convert::load_hdr(&job, &dir).expect("the map loads");
        assert!(map.is_some(), "the job is lit by its map");
        let snapshot = convert::snapshot_from_scene(&job.scene, map);
        assert_eq!(snapshot.exposure.to_bits(), exposure.to_bits());
        snapshot
    };
    let dim = render_png(&snapshot_at(0.5), &dir.join("dim.png"));
    let dim_again = render_png(&snapshot_at(0.5), &dir.join("dim_again.png"));
    let bright = render_png(&snapshot_at(4.0), &dir.join("bright.png"));
    assert_eq!(dim, dim_again, "the same job renders the same picture");
    assert_ne!(dim, bright, "the exposure changes an HDR-lit picture");
    let (dim_level, bright_level) = (mean_level(&dim), mean_level(&bright));
    assert!(
        bright_level > dim_level,
        "exposure 4.0 renders brighter than 0.5: {bright_level} vs {dim_level}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The map's orientation travels with a job: captured from the context, written into the job
/// file (format 6), and rebuilt into the runner's snapshot, which renders the same picture as
/// the original. An unturned job keeps its format and writes no orientation.
#[test]
fn a_turned_hdr_map_survives_the_capture_job_file_and_runner_path() {
    let dir = test_dir("hdr_turn");
    let path = write_test_hdr(&dir);
    let hash = pleochroma_net::messages::content_hash(&std::fs::read(&path).unwrap());
    let (map, _asset) = hdr_asset::load_hdr_file(&path).expect("the map loads");
    let capture = |yaw: f32, follows: bool| -> SceneSnapshot {
        let ctx = RenderContext {
            env_map: Some(Arc::clone(&map)),
            hdr_yaw: yaw,
            hdr_follows_camera: follows,
            ..RenderContext::default()
        };
        let mut snapshot = SceneSnapshot::capture(&Mutex::new(ctx)).expect("the material resolves");
        snapshot.max_bounces = 4;
        snapshot
    };
    // The job a capture makes: the writer's scene, the map's file and hash, the format it needs.
    let job_of = |snapshot: &SceneSnapshot| {
        let mut job = hdr_job(&path, &hash);
        job.scene = convert::scene_from_snapshot(snapshot, 16, 16);
        job.hdr = convert::hdr_source(snapshot).expect("the map has a file");
        job.format = format_for(&job);
        job.validate().expect("the job is consistent");
        codec::from_text(&codec::to_text(&job).expect("the job encodes")).expect("the job decodes")
    };

    let turned = capture(1.1, true);
    let back = job_of(&turned);
    assert_eq!(back.format, 6, "a turned map needs format 6");
    assert_eq!(back.scene.hdr_orientation.yaw.to_bits(), 1.1_f32.to_bits());
    assert!(back.scene.hdr_orientation.follows_camera);
    let text = codec::to_text(&back).expect("the job encodes");
    assert!(text.contains("hdr_orientation"), "the turn is in the file");

    let map_again = convert::load_hdr(&back, &dir).expect("the map loads");
    let rebuilt = convert::snapshot_from_job(&back, map_again);
    assert_eq!(rebuilt.hdr_orientation, turned.hdr_orientation);
    let original_png = render_png(&turned, &dir.join("turned.png"));
    let rebuilt_png = render_png(&rebuilt, &dir.join("rebuilt.png"));
    assert_eq!(
        original_png, rebuilt_png,
        "the runner renders the captured picture"
    );

    let flat = capture(0.0, false);
    assert_eq!(flat.hdr_orientation, HdrOrientation::default());
    let flat_job = job_of(&flat);
    assert_eq!(flat_job.format, 1, "an unturned map keeps the job's format");
    let flat_text = codec::to_text(&flat_job).expect("the job encodes");
    assert!(
        !flat_text.contains("hdr_orientation"),
        "no key for an unturned map"
    );
    let flat_png = render_png(&flat, &dir.join("flat.png"));
    assert_ne!(
        flat_png, original_png,
        "the turned map lights the stone differently"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
