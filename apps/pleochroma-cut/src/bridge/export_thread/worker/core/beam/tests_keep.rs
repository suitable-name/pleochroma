//! Tests of kept beams in the export: the rule that decides whether a render keeps them, the
//! store after a render, the load that skips the tree on a re-render and its bit-identical image.
//! All CPU only, in a temporary cache folder.

use super::{
    super::{
        super::types::{AccumulationCarry, AccumulationOutcome},
        render_accumulation,
    },
    keep::wants_keep,
    remote::NOTE_KEEP_LOCAL,
    *,
};
use crate::bridge::{
    export_thread::{
        params::{ComputeTarget, ExportParams},
        scene_snapshot::SceneSnapshot,
    },
    render_thread::RenderContext,
};
use pleochroma::renderer::{accel::PathCacheSettings, adaptive::NoiseMetric};
use std::{
    path::PathBuf,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

/// The frame edge of the small renders, in pixels (nine 8-pixel tiles).
const SIZE: u32 = 24;
/// The samples per pixel: one pass, which is what a re-light from kept beams runs.
const CAP: u32 = 8;
/// The disk limit of the test folders: far above anything they hold.
const LIMIT: u64 = 1 << 40;

fn capture() -> SceneSnapshot {
    SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("Diamond resolves")
}

/// A coarse tree: the tests check the store, the load and bit identity, which hold at any tree
/// resolution. The defaults make the tree build dominate the run.
fn beam_settings(keep: bool) -> BeamSettings {
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
        residual_min_spp: CAP,
        batch_tiles: 4,
        keep_leaves: keep,
        ..BeamSettings::default()
    }
}

/// A folder of its own under the system temp folder, removed by the test.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-keep-{name}-{}-{nanos}",
            std::process::id()
        ));
        Self(dir)
    }

    /// The number of kept folders in it.
    fn kept(&self) -> usize {
        std::fs::read_dir(&self.0).map_or(0, Iterator::count)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn taps(keep: bool, dir: &TempDir) -> RenderTaps {
    RenderTaps {
        adaptive: Some(AdaptiveSettings {
            target_noise: 0.5,
            metric: NoiseMetric::RelativeLstar,
            min_spp: CAP,
            max_spp: CAP,
            pass_spp: CAP,
            tile: 8,
        }),
        beam: Some(beam_settings(keep)),
        cache_dir: Some(dir.0.clone()),
        cache_disk_limit_bytes: Some(LIMIT),
        ..RenderTaps::default()
    }
}

fn render(
    scene: &SceneSnapshot,
    taps: RenderTaps,
    compute_target: ComputeTarget,
) -> (Accumulation, Vec<ExportProgress>) {
    let cancel = AtomicBool::new(false);
    let mut reports = Vec::new();
    let outcome = render_accumulation(
        scene,
        0.3,
        0.2,
        ExportParams {
            engine: scene.engine,
            width: SIZE,
            height: SIZE,
            samples_per_pixel: CAP,
            max_bounces: scene.max_bounces,
        },
        compute_target,
        None,
        &GpuBackend::disabled(),
        LocalComputeTarget::Cpu,
        &mut AccumulationCarry::with_taps(taps),
        &cancel,
        |progress| reports.push(progress),
    );
    let AccumulationOutcome::Completed(done) = outcome else {
        panic!("the render completes");
    };
    (done, reports)
}

fn last_note(reports: &[ExportProgress]) -> String {
    reports
        .last()
        .and_then(|report| report.note.clone())
        .unwrap_or_default()
}

#[test]
fn a_render_keeps_its_beams_only_when_asked_and_a_tilt_video_only_with_its_path_cache() {
    let dir = TempDir::new("rule");
    let ask = beam_settings(true);
    let no = beam_settings(false);
    let still = taps(true, &dir);
    assert!(wants_keep(&ask, &still));
    assert!(!wants_keep(&no, &still));

    let frame = RenderTaps {
        frame_name: Some("frame_0001".to_owned()),
        ..still
    };
    assert!(
        !wants_keep(&ask, &frame),
        "a tilt frame keeps nothing without the video's path cache"
    );
    let transient = RenderTaps {
        path_cache: Some(PathCacheSettings {
            persist: false,
            ..PathCacheSettings::default()
        }),
        ..frame.clone()
    };
    assert!(!wants_keep(&ask, &transient));
    let persisting = RenderTaps {
        path_cache: Some(PathCacheSettings {
            persist: true,
            ..PathCacheSettings::default()
        }),
        ..frame
    };
    assert!(wants_keep(&ask, &persisting));
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 04b runs it"]
fn a_render_that_does_not_keep_writes_nothing() {
    let scene = capture();
    let dir = TempDir::new("none");
    let (_, reports) = render(&scene, taps(false, &dir), ComputeTarget::LocalOnly);
    assert_eq!(dir.kept(), 0);
    assert!(!last_note(&reports).contains("kept beams"));
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 04b runs it"]
fn a_re_render_loads_the_kept_beams_and_is_bit_identical() {
    let scene = capture();
    let dir = TempDir::new("reload");
    let (plain, _) = render(&scene, taps(false, &dir), ComputeTarget::LocalOnly);

    let (first, first_reports) = render(&scene, taps(true, &dir), ComputeTarget::LocalOnly);
    assert_eq!(dir.kept(), 1, "the beams were stored under one key");
    assert!(
        last_note(&first_reports).starts_with("Beams: "),
        "{}",
        last_note(&first_reports)
    );
    assert!(
        !last_note(&first_reports).contains("kept beams"),
        "the first render traced the facets"
    );
    assert_eq!(first.accum, plain.accum, "keeping changes no pixel");

    let (second, second_reports) = render(&scene, taps(true, &dir), ComputeTarget::LocalOnly);
    let note = last_note(&second_reports);
    eprintln!("re-render note: {note}");
    assert!(note.contains("kept beams"), "the tree was skipped: {note}");
    assert_eq!(second.accum, first.accum, "the same image, bit for bit");
    assert_eq!(dir.kept(), 1, "the same key");
    let figures = second.adaptive.as_ref().expect("the residual's figures");
    assert!(figures.counts.iter().all(|&count| count == CAP));

    // The re-render reports like a fresh one: the share grows, the thumbnail comes with it.
    assert!(second_reports.len() > 2, "{} reports", second_reports.len());
    let fractions: Vec<f32> = second_reports.iter().map(|r| r.fraction).collect();
    assert!(
        fractions.windows(2).all(|pair| pair[0] <= pair[1]),
        "the share never goes back: {fractions:?}"
    );
    assert!(
        fractions.iter().any(|&f| f > 0.0 && f < 1.0),
        "a share between the first and the last report: {fractions:?}"
    );
    assert!(
        second_reports.iter().any(|r| r.preview.is_some()),
        "a preview before the end"
    );
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 04b runs it"]
fn a_finer_lighting_tolerance_traces_afresh() {
    let scene = capture();
    let dir = TempDir::new("tolerance");
    render(&scene, taps(true, &dir), ComputeTarget::LocalOnly);
    assert_eq!(dir.kept(), 1);
    let finer = RenderTaps {
        beam: Some(BeamSettings {
            lighting_tolerance: 5e-3,
            ..beam_settings(true)
        }),
        ..taps(true, &dir)
    };
    let (_, reports) = render(&scene, finer, ComputeTarget::LocalOnly);
    assert!(
        !last_note(&reports).contains("kept beams"),
        "the coarser leaves were not loaded"
    );
    assert_eq!(dir.kept(), 2, "a finer lighting tolerance is another key");
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 04b runs it"]
fn another_pose_is_another_key() {
    let scene = capture();
    let dir = TempDir::new("pose");
    render(&scene, taps(true, &dir), ComputeTarget::LocalOnly);
    let cancel = AtomicBool::new(false);
    let mut moved = Vec::new();
    let outcome = render_accumulation(
        &scene,
        0.6,
        0.2,
        ExportParams {
            engine: scene.engine,
            width: SIZE,
            height: SIZE,
            samples_per_pixel: CAP,
            max_bounces: scene.max_bounces,
        },
        ComputeTarget::LocalOnly,
        None,
        &GpuBackend::disabled(),
        LocalComputeTarget::Cpu,
        &mut AccumulationCarry::with_taps(taps(true, &dir)),
        &cancel,
        |progress| moved.push(progress),
    );
    assert!(matches!(outcome, AccumulationOutcome::Completed(_)));
    assert!(
        !last_note(&moved).contains("kept beams"),
        "a new pose traces its own tree"
    );
    assert_eq!(dir.kept(), 2);
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 04b runs it"]
fn a_tilt_frame_without_the_videos_path_cache_stores_nothing() {
    let scene = capture();
    let dir = TempDir::new("tilt");
    let frame = RenderTaps {
        frame_name: Some("frame_0000".to_owned()),
        ..taps(true, &dir)
    };
    render(&scene, frame.clone(), ComputeTarget::LocalOnly);
    assert_eq!(dir.kept(), 0);
    let persisting = RenderTaps {
        path_cache: Some(PathCacheSettings {
            persist: true,
            ..PathCacheSettings::default()
        }),
        ..frame
    };
    render(&scene, persisting, ComputeTarget::LocalOnly);
    assert_eq!(dir.kept(), 1);
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 04b runs it"]
fn kept_beams_render_here_and_say_so_under_remote_compute() {
    let scene = capture();
    let dir = TempDir::new("local");
    let (_, reports) = render(&scene, taps(true, &dir), ComputeTarget::Both);
    // The first report is the bare one before the lane is settled: the first note says it.
    let first = reports.iter().find_map(|r| r.note.as_deref()).unwrap_or("");
    assert!(first.contains(NOTE_KEEP_LOCAL), "{first}");
    // A re-render from the kept beams stays on this computer too, and says so as well.
    let (_, again) = render(&scene, taps(true, &dir), ComputeTarget::Both);
    let first = again.iter().find_map(|r| r.note.as_deref()).unwrap_or("");
    assert!(first.contains(NOTE_KEEP_LOCAL), "{first}");
    assert!(
        last_note(&again).contains("kept beams"),
        "{}",
        last_note(&again)
    );
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 04b runs it"]
fn a_corrupt_kept_folder_is_rebuilt_silently() {
    let scene = capture();
    let dir = TempDir::new("corrupt");
    let (first, _) = render(&scene, taps(true, &dir), ComputeTarget::LocalOnly);
    // Damage every chunk file of the one kept folder.
    let folder = std::fs::read_dir(&dir.0)
        .expect("the cache folder")
        .next()
        .expect("one kept folder")
        .expect("readable")
        .path();
    let chunks = folder.join("tiles");
    for entry in std::fs::read_dir(&chunks).expect("the chunks") {
        let path = entry.expect("readable").path();
        let mut bytes = std::fs::read(&path).expect("a chunk");
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0xff;
        std::fs::write(&path, bytes).expect("rewritten");
    }
    let (again, reports) = render(&scene, taps(true, &dir), ComputeTarget::LocalOnly);
    assert!(
        !last_note(&reports).contains("kept beams"),
        "the damaged beams were not used"
    );
    assert_eq!(again.accum, first.accum, "the render traced them afresh");
}
