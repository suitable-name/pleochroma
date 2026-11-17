//! Tests of the job core: the snapshot round trip, resumable videos, the frame-folder
//! guards, the HDR checks and the still executor. Every render is CPU-only and tiny.

use super::{
    convert::{self, StillJobInputs},
    execute::{ExecContext, execute_job, remote_selection_for},
};
use crate::{
    bridge::{
        export_thread::{ExportOutcome, ExportParams, RemoteSelection, SceneSnapshot, run_export},
        render_thread::RenderContext,
    },
    gui::tilt::video_export::{frames, metrics, run::VideoExportRequest},
    settings::LocalComputeTarget,
};
use pleochroma::{
    color::ColorSpace,
    optics::{dispersion::DispersionModel, materials::GemMaterial},
};
use pleochroma_net::{
    messages::hash_hex,
    scene::{HdrEnvironment, SceneEnvironment},
};
use pleochroma_render_jobs::{
    ComputeChoice, FailureKind, JobOutcome, JobProgress, JobSink, RenderJobFile, TransferChoice,
    codec,
    job::{DesignInfo, HdrSource},
    paths::{FRAME_MARKER_FILE, marker_text},
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

/// A fresh, empty folder for one test.
pub(super) fn test_dir(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pleochroma_render_jobs_{name}_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn capture(ctx: RenderContext) -> SceneSnapshot {
    let mut snapshot = SceneSnapshot::capture(&Mutex::new(ctx)).expect("the material resolves");
    snapshot.max_bounces = 4;
    snapshot
}

/// A custom Sellmeier material with a frosted girdle and dimmed surface glare: the parts of
/// a scene that a naive conversion would lose.
fn custom_context() -> RenderContext {
    let material = GemMaterial::new_custom_with_dispersion(
        "Custom",
        DispersionModel::Sellmeier1 { b1: 1.4, c1: 0.012 },
        0.0,
        [0.0; 3],
    );
    RenderContext {
        material_override: Some(material),
        girdle_frosted: true,
        surface_glare: 0.25,
        ..RenderContext::default()
    }
}

fn still_inputs<'a>(
    scene: &'a SceneSnapshot,
    params: ExportParams,
    remote: &'a RemoteSelection,
    output: &'a Path,
) -> StillJobInputs<'a> {
    StillJobInputs {
        scene,
        params,
        color_space: ColorSpace::Srgb,
        remote,
        output,
        label: "Test job".to_string(),
        design: DesignInfo::default(),
        preset_label: String::new(),
        adaptive: None,
        path_cache: None,
        beam: None,
    }
}

fn tiny_params() -> ExportParams {
    ExportParams {
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        width: 16,
        height: 16,
        samples_per_pixel: 2,
        max_bounces: 4,
    }
}

fn assert_snapshots_equal(a: &SceneSnapshot, b: &SceneSnapshot) {
    let bits = |label: &str, x: f32, y: f32| {
        assert_eq!(x.to_bits(), y.to_bits(), "{label} differs: {x} vs {y}");
    };
    bits("yaw", a.yaw, b.yaw);
    bits("pitch", a.pitch, b.pitch);
    bits("pan x", a.camera_pan[0], b.camera_pan[0]);
    bits("pan y", a.camera_pan[1], b.camera_pan[1]);
    assert_eq!(a.stone_pose, b.stone_pose);
    bits("distance", a.distance, b.distance);
    bits("light_yaw", a.light_yaw, b.light_yaw);
    bits("light_pitch", a.light_pitch, b.light_pitch);
    bits("exposure", a.exposure, b.exposure);
    bits("backdrop", a.backdrop, b.backdrop);
    bits("surface_glare", a.surface_glare, b.surface_glare);
    assert_eq!(a.max_bounces, b.max_bounces);
    assert_eq!(a.lighting_preset, b.lighting_preset);
    assert_eq!(a.material, b.material);
    // `Debug` prints the shortest text of each float, so equal text means equal bits.
    assert_eq!(format!("{:?}", a.material), format!("{:?}", b.material));
    assert_eq!(a.active_planes, b.active_planes);
    assert_eq!(a.tools, b.tools);
    assert_eq!(a.facet_finishes, b.facet_finishes);
    assert_eq!(*a.fluorescence, *b.fluorescence);
    assert_eq!(a.env_map.is_some(), b.env_map.is_some());
}

pub(super) fn render_png(scene: &SceneSnapshot, path: &Path) -> Vec<u8> {
    let outcome = run_export(
        scene,
        tiny_params(),
        ColorSpace::Srgb,
        path,
        &RemoteSelection::local_only(),
        LocalComputeTarget::Cpu,
        &AtomicBool::new(false),
        |_| {},
    );
    assert!(
        matches!(outcome, ExportOutcome::Completed(_)),
        "the export must finish"
    );
    std::fs::read(path).unwrap()
}

/// A scene frozen into a job file and rebuilt renders the same picture, byte for byte, for
/// the default material and for a custom one with a frosted girdle.
#[test]
fn j3_t1_the_snapshot_round_trip_renders_identically() {
    let dir = test_dir("t1");
    for (name, ctx) in [
        ("default", RenderContext::default()),
        ("custom", custom_context()),
    ] {
        let original = capture(ctx);
        if name == "custom" {
            assert!(
                !original.facet_finishes.is_empty(),
                "the custom scene must carry a frosted girdle"
            );
        }
        let remote = RemoteSelection::local_only();
        let output = dir.join(format!("{name}-job.png"));
        let job = convert::build_still_job(
            &still_inputs(&original, tiny_params(), &remote, &output),
            1_791_302_580,
        )
        .expect("the job builds");
        let back = codec::from_text(&codec::to_text(&job).expect("the job encodes"))
            .expect("the job decodes");
        assert_eq!(back, job, "{name}: the job file must survive its own text");

        let rebuilt = convert::snapshot_from_scene(&back.scene, None);
        assert_snapshots_equal(&original, &rebuilt);

        let a = render_png(&original, &dir.join(format!("{name}-a.png")));
        let b = render_png(&rebuilt, &dir.join(format!("{name}-b.png")));
        assert_eq!(
            a, b,
            "{name}: the rebuilt scene must render the same picture"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The stone's orientation travels with a job: a biaxial stone with a Crystal Axis and a Second
/// Axis is frozen into the job file and rebuilt with both directions, and a stone without either
/// keeps the material's own (so a job saved without an axis is unchanged).
#[test]
fn the_crystal_axis_and_the_second_axis_survive_the_job_round_trip() {
    let dir = test_dir("axis");
    let gamma = glam::Vec3::X;
    let alpha = glam::Vec3::Z;
    let oriented = capture(RenderContext {
        material_name: "Tanzanite".to_string(),
        c_axis_override: Some(gamma),
        alpha_axis_override: Some(alpha),
        ..RenderContext::default()
    });
    assert_eq!(oriented.material.c_axis, gamma);
    assert_eq!(oriented.material.alpha_axis, Some(alpha));
    let plain = capture(RenderContext {
        material_name: "Tanzanite".to_string(),
        ..RenderContext::default()
    });
    assert_eq!(plain.material.alpha_axis, None);

    for (name, original) in [("oriented", &oriented), ("plain", &plain)] {
        let remote = RemoteSelection::local_only();
        let output = dir.join(format!("{name}-job.png"));
        let job = convert::build_still_job(
            &still_inputs(original, tiny_params(), &remote, &output),
            1_791_302_580,
        )
        .expect("the job builds");
        let back = codec::from_text(&codec::to_text(&job).expect("the job encodes"))
            .expect("the job decodes");
        let rebuilt = convert::snapshot_from_scene(&back.scene, None);
        assert_eq!(rebuilt.material.c_axis, original.material.c_axis, "{name}");
        assert_eq!(
            rebuilt.material.alpha_axis, original.material.alpha_axis,
            "{name}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The viewport's pan is frozen into the job file and rebuilt with it, so a queued render frames
/// the stone where the view did.
#[test]
fn the_pan_survives_the_job_round_trip() {
    let dir = test_dir("pan");
    let original = capture(RenderContext {
        camera_pan: [0.3, -0.45],
        ..RenderContext::default()
    });
    let remote = RemoteSelection::local_only();
    let output = dir.join("pan-job.png");
    let job = convert::build_still_job(
        &still_inputs(&original, tiny_params(), &remote, &output),
        1_791_302_580,
    )
    .expect("the job builds");
    assert_eq!(job.scene.camera_pan, [0.3, -0.45]);
    assert_eq!(job.format, 7, "a pan makes the job format 7");
    let back =
        codec::from_text(&codec::to_text(&job).expect("the job encodes")).expect("the job decodes");
    let rebuilt = convert::snapshot_from_scene(&back.scene, None);
    assert_snapshots_equal(&original, &rebuilt);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The Stone Pose is frozen into the job file (format 8) and rebuilt with it, so a queued or
/// command-line render stands the stone as the view did; a job without one rebuilds unposed.
#[test]
fn the_stone_pose_survives_the_job_round_trip() {
    use pleochroma::render_setup::StonePose;
    let dir = test_dir("stone_pose");
    let mut original = capture(RenderContext::default());
    original.stone_pose = StonePose::from_array([90.0, 30.0, 10.0]);
    let remote = RemoteSelection::local_only();
    let output = dir.join("pose-job.png");
    let job = convert::build_still_job(
        &still_inputs(&original, tiny_params(), &remote, &output),
        1_791_302_580,
    )
    .expect("the job builds");
    assert_eq!(job.scene.stone_pose, [90.0, 30.0, 10.0]);
    assert_eq!(job.format, 8, "a Stone Pose makes the job format 8");
    let back =
        codec::from_text(&codec::to_text(&job).expect("the job encodes")).expect("the job decodes");
    let rebuilt = convert::snapshot_from_scene(&back.scene, None);
    assert_snapshots_equal(&original, &rebuilt);

    let unposed = capture(RenderContext::default());
    let plain = convert::build_still_job(
        &still_inputs(&unposed, tiny_params(), &remote, &output),
        1_791_302_580,
    )
    .expect("the job builds");
    assert_eq!(plain.scene.stone_pose, [0.0, 0.0, 0.0]);
    assert_eq!(
        convert::snapshot_from_scene(&plain.scene, None).stone_pose,
        StonePose::IDENTITY
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The queue row names the pose only when the stone is posed.
#[test]
fn the_job_detail_names_a_stone_pose_only_when_posed() {
    use pleochroma::render_setup::StonePose;
    assert_eq!(convert::stone_pose_detail(StonePose::IDENTITY), "");
    assert_eq!(
        convert::stone_pose_detail(StonePose::from_array([90.0, 30.0, 10.0])),
        " \u{b7} Stone Pose: tilt 90\u{b0}, turn 30\u{b0}, spin 10\u{b0}"
    );
}

/// The stone's render physics (surface finish, abraded edge and the faceting machine) is frozen
/// into the job file and rebuilt with it.
#[test]
fn the_render_physics_survive_the_job_round_trip() {
    use pleochroma_net::scene::effects::{EdgeSpec, SceneEffects};
    let dir = test_dir("effects");
    let mut original = capture(RenderContext::default());
    original.effects = SceneEffects {
        edge: EdgeSpec {
            width: 0.02,
            alpha: 0.3,
            halo_rq_nm: 0.0,
        },
        ..SceneEffects::default()
    };
    let remote = RemoteSelection::local_only();
    let output = dir.join("effects-job.png");
    let job = convert::build_still_job(
        &still_inputs(&original, tiny_params(), &remote, &output),
        1_791_302_580,
    )
    .expect("the job builds");
    let back =
        codec::from_text(&codec::to_text(&job).expect("the job encodes")).expect("the job decodes");
    let rebuilt = convert::snapshot_from_scene(&back.scene, None);
    assert_eq!(rebuilt.effects, original.effects);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The stone's color zones are frozen into the job file (format 5), come back from its text, and
/// the runner's snapshot carries them again, so it renders the picture the zoned capture does.
#[test]
fn the_color_zones_survive_the_capture_job_file_and_runner_path() {
    let dir = test_dir("zoning");
    let zoning = pleochroma::optics::materials::ametrine_zoning();
    let mut original = capture(RenderContext::default());
    original.material = original.material.with_zoning(zoning.clone());
    let remote = RemoteSelection::local_only();
    let output = dir.join("zoned-job.png");
    let job = convert::build_still_job(
        &still_inputs(&original, tiny_params(), &remote, &output),
        1_791_302_580,
    )
    .expect("the job builds");
    assert_eq!(job.format, 5, "zones make the job format 5");
    assert_eq!(job.zoning.as_ref(), Some(&zoning));
    assert!(job.scene.material.zoning.is_none(), "the scene holds none");
    let back =
        codec::from_text(&codec::to_text(&job).expect("the job encodes")).expect("the job decodes");
    assert_eq!(back, job, "the zones survive the file text");

    let rebuilt = convert::snapshot_from_job(&back, None);
    assert_eq!(rebuilt.material.zoning.as_ref(), Some(&zoning));
    assert_snapshots_equal(&original, &rebuilt);
    assert!(
        convert::snapshot_from_scene(&back.scene, None)
            .material
            .zoning
            .is_none(),
        "the bare scene is the base color alone"
    );
    let a = render_png(&original, &dir.join("zoned-a.png"));
    let b = render_png(&rebuilt, &dir.join("zoned-b.png"));
    assert_eq!(a, b, "the runner renders the zoned picture");

    // A stone without zones stays the job it was.
    let plain = convert::build_still_job(
        &still_inputs(
            &capture(RenderContext::default()),
            tiny_params(),
            &remote,
            &output,
        ),
        1_791_302_580,
    )
    .expect("the job builds");
    assert_eq!(plain.format, 1);
    assert!(plain.zoning.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A queue row written before format 5 holds its zones only in the side table: the job queue
/// (when it starts the row) and the script export complete the job from it with
/// `complete_job_zoning`, and a job that already holds zones is left alone.
#[test]
fn a_row_queued_before_format_five_gets_its_zones_from_the_side_table() {
    use crate::gui::rough_colour::store::{complete_job_zoning, save_job_zoning};
    let db = pleochroma_vault::db::sqlite::Database::new(Some(":memory:")).expect("vault");
    let zoning = pleochroma::optics::materials::ametrine_zoning();
    let mut zoned = capture(RenderContext::default());
    zoned.material = zoned.material.with_zoning(zoning.clone());
    let remote = RemoteSelection::local_only();
    let dir = test_dir("legacy-zoning");
    let output = dir.join("legacy-job.png");
    let current = convert::build_still_job(
        &still_inputs(&zoned, tiny_params(), &remote, &output),
        1_791_302_580,
    )
    .expect("the job builds");

    // The row as an older version wrote it: no zones in the file, format 1, zones beside it.
    let mut legacy = current.clone();
    legacy.zoning = None;
    legacy.format = pleochroma_render_jobs::format_for(&legacy);
    assert_eq!(legacy.format, 1);
    assert!(save_job_zoning(&db, 5, &zoned.material).expect("saves"));

    let mut completed = legacy.clone();
    assert!(complete_job_zoning(&db, 5, &mut completed).expect("completes"));
    assert_eq!(completed.zoning.as_ref(), Some(&zoning));
    assert_eq!(completed.format, 5);
    assert_eq!(completed.validate(), Ok(()));
    let snapshot = convert::snapshot_from_job(&completed, None);
    assert_eq!(snapshot.material.zoning.as_ref(), Some(&zoning));
    assert_eq!(
        codec::from_text(&codec::to_text(&completed).expect("encodes")).expect("decodes"),
        completed,
        "an exported script's job file carries the zones"
    );

    // A row with no side-table entry renders as it is, and a current job is not touched.
    let mut bare = legacy.clone();
    assert!(!complete_job_zoning(&db, 6, &mut bare).expect("completes"));
    assert_eq!(bare, legacy);
    let mut whole = current.clone();
    assert!(!complete_job_zoning(&db, 5, &mut whole).expect("completes"));
    assert_eq!(whole, current);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The dialog's adaptive-sampling choice is frozen into a still job and a video job, comes back
/// from the file, and a job without it is the job it was.
#[test]
fn adaptive_sampling_is_frozen_into_still_and_video_jobs_and_off_adds_nothing() {
    use pleochroma::renderer::adaptive::{AdaptiveSettings, TARGET_NOISE_PRESETS};
    let dir = test_dir("adaptive-freeze");
    let scene = capture(RenderContext::default());
    let remote = RemoteSelection::local_only();
    let output = dir.join("adaptive-job.png");
    let plain = convert::build_still_job(
        &still_inputs(&scene, tiny_params(), &remote, &output),
        1_791_302_580,
    )
    .expect("the job builds");
    let plain_text = codec::to_text(&plain).expect("the job encodes");
    assert!(
        !plain_text.contains("accel"),
        "off writes no acceleration key"
    );

    let draft = AdaptiveSettings {
        target_noise: TARGET_NOISE_PRESETS[2].1,
        ..AdaptiveSettings::provisional(2)
    };
    let mut inputs = still_inputs(&scene, tiny_params(), &remote, &output);
    inputs.adaptive = Some(draft);
    let job = convert::build_still_job(&inputs, 1_791_302_580).expect("the job builds");
    let back = codec::from_text(&codec::to_text(&job).expect("the job encodes")).expect("decodes");
    let pleochroma_render_jobs::JobKind::Still(still) = &back.kind else {
        unreachable!("a still stays a still")
    };
    assert_eq!(still.accel.adaptive, Some(draft));

    // The executor's own reading of the file caps the stored settings at the job's samples.
    assert_eq!(
        super::execute::job_adaptive_settings(&back).map(|settings| settings.max_spp),
        Some(2)
    );
    assert_eq!(super::execute::job_adaptive_settings(&plain), None);

    // A video, through its request and back into the request the executor builds.
    let video = video_job(&dir.join("clip-off"), false);
    assert_eq!(super::execute::job_adaptive_settings(&video), None);
    let mut request = VideoExportRequest {
        scene: capture(RenderContext::default()),
        axes: vec![0],
        start_deg: -2.0,
        end_deg: 2.0,
        step_deg: 2.0,
        total_frames: 3,
        fps: 30,
        width: 16,
        height: 16,
        samples_per_pixel: 2,
        color_space: ColorSpace::Srgb,
        out_dir: dir.join("clip-on"),
        out_name: "clip".to_string(),
        selection: metrics::MetricSelection::default(),
        curves: Vec::new(),
        keep_frames: false,
        remote: RemoteSelection::local_only(),
        local_compute: LocalComputeTarget::Cpu,
        adaptive: Some(draft),
        path_cache: None,
        beam: None,
    };
    let video = convert::build_video_job(
        &request,
        "Test video".to_string(),
        DesignInfo::default(),
        "clip".to_string(),
        0,
    )
    .expect("the video job builds");
    let video = codec::from_text(&codec::to_text(&video).expect("encodes")).expect("decodes");
    let pleochroma_render_jobs::JobKind::TiltVideo(frozen) = &video.kind else {
        unreachable!("a video stays a video")
    };
    assert_eq!(frozen.accel.adaptive, Some(draft));
    assert!(!frozen.keep_path_cache);
    let rebuilt = convert::video_request_from_job(
        &video,
        frozen,
        capture(RenderContext::default()),
        dir.join("clip-on"),
        RemoteSelection::local_only(),
        LocalComputeTarget::Cpu,
    );
    assert_eq!(
        rebuilt.adaptive,
        Some(AdaptiveSettings {
            max_spp: 2,
            ..draft
        })
    );
    request.adaptive = None;
    let off = convert::build_video_job(
        &request,
        "Test video".to_string(),
        DesignInfo::default(),
        "clip".to_string(),
        0,
    )
    .expect("the video job builds");
    assert!(!codec::to_text(&off).expect("encodes").contains("accel"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The job list's detail line says "adaptive" with the Target Noise entry, and says nothing for
/// an ordinary job.
#[cfg(feature = "adaptive-sampling")]
#[test]
fn the_detail_line_names_adaptive_sampling_and_its_target_noise() {
    use pleochroma::renderer::adaptive::{AdaptiveSettings, NoiseMetric, TARGET_NOISE_PRESETS};
    assert_eq!(convert::adaptive_detail(None), "");
    for (label, target) in TARGET_NOISE_PRESETS {
        let settings = AdaptiveSettings {
            target_noise: target,
            ..AdaptiveSettings::provisional(1024)
        };
        assert_eq!(
            convert::adaptive_detail(Some(&settings)),
            format!(" \u{b7} adaptive ({label})")
        );
    }
    let odd = AdaptiveSettings {
        target_noise: 0.0123,
        ..AdaptiveSettings::provisional(1024)
    };
    assert_eq!(
        convert::adaptive_detail(Some(&odd)),
        " \u{b7} adaptive (Custom)"
    );
    let absolute = AdaptiveSettings {
        metric: NoiseMetric::AbsoluteDeltaE,
        target_noise: 1.0,
        ..AdaptiveSettings::provisional(1024)
    };
    assert_eq!(
        convert::adaptive_detail(Some(&absolute)),
        " \u{b7} adaptive (Custom)"
    );
}

/// Without the `adaptive-sampling` feature no adaptive pass renders, so the job list never
/// says "adaptive", even for a job file that asks for it.
#[cfg(not(feature = "adaptive-sampling"))]
#[test]
fn the_detail_line_says_nothing_about_adaptive_sampling_in_a_build_without_it() {
    use pleochroma::renderer::adaptive::AdaptiveSettings;
    assert_eq!(convert::adaptive_detail(None), "");
    let settings = AdaptiveSettings::provisional(1024);
    assert_eq!(convert::adaptive_detail(Some(&settings)), "");
}

/// The dialog's path-cache choice is frozen into a still job and a video job, comes back from
/// the file as the taps the executor builds, and a job without it adds nothing.
#[test]
fn the_path_cache_is_frozen_into_still_and_video_jobs_and_off_adds_nothing() {
    use pleochroma::renderer::accel::PathCacheSettings;
    let dir = test_dir("cache-freeze");
    let scene = capture(RenderContext::default());
    let remote = RemoteSelection::local_only();
    let output = dir.join("cache-job.png");
    let plain = convert::build_still_job(
        &still_inputs(&scene, tiny_params(), &remote, &output),
        1_791_302_580,
    )
    .expect("the job builds");
    assert_eq!(super::execute::job_path_cache_settings(&plain), None);
    assert!(
        !codec::to_text(&plain)
            .expect("the job encodes")
            .contains("path_cache")
    );

    let kept = PathCacheSettings {
        persist: true,
        ..PathCacheSettings::default()
    };
    let mut inputs = still_inputs(&scene, tiny_params(), &remote, &output);
    inputs.path_cache = Some(kept);
    let job = convert::build_still_job(&inputs, 1_791_302_580).expect("the job builds");
    let back = codec::from_text(&codec::to_text(&job).expect("the job encodes")).expect("decodes");
    assert_eq!(super::execute::job_path_cache_settings(&back), Some(kept));

    // A video: the frame caches reach the disk only with "Keep Path Cache for This Video".
    let mut request = VideoExportRequest {
        scene: capture(RenderContext::default()),
        axes: vec![0],
        start_deg: -2.0,
        end_deg: 2.0,
        step_deg: 2.0,
        total_frames: 3,
        fps: 30,
        width: 16,
        height: 16,
        samples_per_pixel: 2,
        color_space: ColorSpace::Srgb,
        out_dir: dir.join("clip"),
        out_name: "clip".to_string(),
        selection: metrics::MetricSelection::default(),
        curves: Vec::new(),
        keep_frames: false,
        remote: RemoteSelection::local_only(),
        local_compute: LocalComputeTarget::Cpu,
        adaptive: None,
        path_cache: Some(PathCacheSettings::default()),
        beam: None,
    };
    for keep in [false, true] {
        request.path_cache = Some(PathCacheSettings {
            persist: keep,
            ..PathCacheSettings::default()
        });
        let video = convert::build_video_job(
            &request,
            "Test video".to_string(),
            DesignInfo::default(),
            "clip".to_string(),
            0,
        )
        .expect("the video job builds");
        let video = codec::from_text(&codec::to_text(&video).expect("encodes")).expect("decodes");
        let pleochroma_render_jobs::JobKind::TiltVideo(frozen) = &video.kind else {
            unreachable!("a video stays a video")
        };
        assert_eq!(frozen.keep_path_cache, keep);
        assert!(frozen.accel.path_cache.is_some());
        assert_eq!(
            super::execute::job_path_cache_settings(&video).map(|cache| cache.persist),
            Some(keep)
        );
        let rebuilt = convert::video_request_from_job(
            &video,
            frozen,
            capture(RenderContext::default()),
            dir.join("clip"),
            RemoteSelection::local_only(),
            LocalComputeTarget::Cpu,
        );
        assert_eq!(rebuilt.path_cache.map(|cache| cache.persist), Some(keep));
    }
    request.path_cache = None;
    let off = convert::build_video_job(
        &request,
        "Test video".to_string(),
        DesignInfo::default(),
        "clip".to_string(),
        0,
    )
    .expect("the video job builds");
    assert!(!codec::to_text(&off).expect("encodes").contains("accel"));
    assert_eq!(super::execute::job_path_cache_settings(&off), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The job list's detail line says "path cache" when it is on and nothing when it is off.
#[test]
fn the_detail_line_names_the_path_cache() {
    use pleochroma::renderer::accel::PathCacheSettings;
    assert_eq!(convert::path_cache_detail(None), "");
    assert_eq!(
        convert::path_cache_detail(Some(&PathCacheSettings::default())),
        " \u{b7} path cache"
    );
}

/// The dialog's beam-tracing choice is frozen into a still job and a video job (`accel.beam`),
/// comes back from the file as the taps the executor builds, and a job without it adds nothing.
#[cfg(all(feature = "beam", beam_tests))]
#[test]
fn the_beam_choice_is_frozen_into_still_and_video_jobs_and_off_adds_nothing() {
    use pleochroma::renderer::accel_beam::BeamSettings;
    // The job text is checked through its structure (`kind.accel`), not by searching it: the
    // output path is part of the text, and its folder name must not decide the check.
    let dir = test_dir("freeze-choice");
    let has_accel = |job: &RenderJobFile| {
        let text = codec::to_text(job).expect("the job encodes");
        let value: serde_json::Value = serde_json::from_str(&text).expect("the text is JSON");
        value
            .get("kind")
            .and_then(|kind| kind.get("accel"))
            .is_some()
    };
    let scene = capture(RenderContext::default());
    let remote = RemoteSelection::local_only();
    let output = dir.join("freeze-job.png");
    let plain = convert::build_still_job(
        &still_inputs(&scene, tiny_params(), &remote, &output),
        1_791_302_580,
    )
    .expect("the job builds");
    assert_eq!(super::execute::job_beam_settings(&plain), None);
    assert!(
        !has_accel(&plain),
        "a job without beams writes no accel record"
    );

    let mut inputs = still_inputs(&scene, tiny_params(), &remote, &output);
    inputs.beam = Some(BeamSettings::default());
    let job = convert::build_still_job(&inputs, 1_791_302_580).expect("the job builds");
    let back = codec::from_text(&codec::to_text(&job).expect("the job encodes")).expect("decodes");
    assert_eq!(
        super::execute::job_beam_settings(&back),
        Some(BeamSettings::default())
    );

    // A video: the same setting, and back into the request the frame loop runs.
    let mut request = VideoExportRequest {
        scene: capture(RenderContext::default()),
        axes: vec![0],
        start_deg: -2.0,
        end_deg: 2.0,
        step_deg: 2.0,
        total_frames: 3,
        fps: 30,
        width: 16,
        height: 16,
        samples_per_pixel: 2,
        color_space: ColorSpace::Srgb,
        out_dir: dir.join("clip"),
        out_name: "clip".to_string(),
        selection: metrics::MetricSelection::default(),
        curves: Vec::new(),
        keep_frames: false,
        remote: RemoteSelection::local_only(),
        local_compute: LocalComputeTarget::Cpu,
        adaptive: None,
        path_cache: None,
        beam: Some(BeamSettings::default()),
    };
    let video = convert::build_video_job(
        &request,
        "Test video".to_string(),
        DesignInfo::default(),
        "clip".to_string(),
        0,
    )
    .expect("the video job builds");
    let video = codec::from_text(&codec::to_text(&video).expect("encodes")).expect("decodes");
    let pleochroma_render_jobs::JobKind::TiltVideo(frozen) = &video.kind else {
        unreachable!("a video stays a video")
    };
    assert_eq!(frozen.accel.beam, Some(BeamSettings::default()));
    assert_eq!(
        super::execute::job_beam_settings(&video),
        Some(BeamSettings::default())
    );
    let rebuilt = convert::video_request_from_job(
        &video,
        frozen,
        capture(RenderContext::default()),
        dir.join("clip"),
        RemoteSelection::local_only(),
        LocalComputeTarget::Cpu,
    );
    assert_eq!(rebuilt.beam, Some(BeamSettings::default()));

    request.beam = None;
    let off = convert::build_video_job(
        &request,
        "Test video".to_string(),
        DesignInfo::default(),
        "clip".to_string(),
        0,
    )
    .expect("the video job builds");
    assert!(
        !has_accel(&off),
        "a video job without beams writes no accel record"
    );
    assert_eq!(super::execute::job_beam_settings(&off), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// "Keep Beams for Re-Renders" is frozen into the job as `accel.beam.keep_leaves` and comes back
/// from the file; without it the flag stays off.
#[cfg(all(feature = "beam", beam_tests))]
#[test]
fn keep_beams_is_frozen_into_the_job_as_keep_leaves() {
    use pleochroma::renderer::accel_beam::BeamSettings;
    let dir = test_dir("beam-keep-freeze");
    let scene = capture(RenderContext::default());
    let remote = RemoteSelection::local_only();
    let output = dir.join("beam-keep-job.png");
    for keep in [false, true] {
        let mut inputs = still_inputs(&scene, tiny_params(), &remote, &output);
        inputs.beam = Some(BeamSettings {
            keep_leaves: keep,
            ..BeamSettings::default()
        });
        let job = convert::build_still_job(&inputs, 1_791_302_580).expect("the job builds");
        let back =
            codec::from_text(&codec::to_text(&job).expect("the job encodes")).expect("decodes");
        let beam = super::execute::job_beam_settings(&back).expect("the job keeps its beams");
        assert_eq!(beam.keep_leaves, keep);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The job list's detail line says "beams" when beam tracing is on and nothing when it is off.
#[cfg(all(feature = "beam", beam_tests))]
#[test]
fn the_detail_line_names_the_beams() {
    use pleochroma::renderer::accel_beam::BeamSettings;
    assert_eq!(convert::beam_detail(None), "");
    assert_eq!(
        convert::beam_detail(Some(&BeamSettings::default())),
        " \u{b7} beams"
    );
}

#[test]
fn compute_choices_and_engines_map_both_ways() {
    for target in [
        crate::bridge::export_thread::ComputeTarget::LocalOnly,
        crate::bridge::export_thread::ComputeTarget::RemoteOnly,
        crate::bridge::export_thread::ComputeTarget::Both,
    ] {
        for transfer in [
            crate::settings::ExportTransfer::FullData,
            crate::settings::ExportTransfer::FinalPicture,
        ] {
            let selection = RemoteSelection {
                compute_target: target,
                worker: None,
                transfer,
                contribute_local: true,
            };
            let compute = convert::job_compute(&selection);
            assert_eq!(convert::remote_selection(compute, None), selection);
        }
    }
    for target in [
        LocalComputeTarget::Cpu,
        LocalComputeTarget::CpuGpu,
        LocalComputeTarget::Gpu,
    ] {
        assert_eq!(
            convert::local_target_of(convert::engines_of(target)),
            target
        );
    }
    for space in [ColorSpace::Srgb, ColorSpace::DisplayP3, ColorSpace::Rec2020] {
        let job_space = convert::job_color_space(space).unwrap();
        assert_eq!(convert::color_space_of(job_space), space);
    }
    assert_eq!(
        convert::job_color_space(ColorSpace::AcesCg).unwrap_err(),
        "ACEScg cannot be saved in an 8-bit picture."
    );
}

fn context(dir: &Path) -> ExecContext {
    ExecContext {
        base_dir: dir.to_path_buf(),
        local_compute: LocalComputeTarget::Cpu,
        worker: None,
        compute_override: None,
        transfer_override: None,
        contribute_local_override: None,
        output_override: None,
        restart_frames: false,
    }
}

/// A three-frame, 16 x 16, one-sample video job writing into `dir`.
fn video_job(dir: &Path, keep_frames: bool) -> RenderJobFile {
    video_job_with(dir, keep_frames, RemoteSelection::local_only())
}

/// [`video_job`] with the compute choices of `remote`.
fn video_job_with(dir: &Path, keep_frames: bool, remote: RemoteSelection) -> RenderJobFile {
    let request = VideoExportRequest {
        scene: capture(RenderContext::default()),
        axes: vec![0],
        start_deg: -2.0,
        end_deg: 2.0,
        step_deg: 2.0,
        total_frames: 3,
        fps: 30,
        width: 16,
        height: 16,
        samples_per_pixel: 1,
        color_space: ColorSpace::Srgb,
        out_dir: dir.to_path_buf(),
        out_name: "clip".to_string(),
        selection: metrics::MetricSelection::default(),
        curves: Vec::new(),
        keep_frames,
        remote,
        local_compute: LocalComputeTarget::Cpu,
        adaptive: None,
        path_cache: None,
        beam: None,
    };
    convert::build_video_job(
        &request,
        "Test video".to_string(),
        DesignInfo::default(),
        "clip".to_string(),
        0,
    )
    .expect("the video job builds")
}

/// A video request for the tray effect tests: the default Live Render scene with `shadow` and
/// `reflection` set the way the section's switches would leave them.
fn effects_video_request(shadow: bool, reflection: f32) -> VideoExportRequest {
    let mut scene = capture(RenderContext::default());
    scene.stone_shadow = shadow;
    scene.tray_reflection = reflection;
    VideoExportRequest {
        scene,
        axes: vec![0],
        start_deg: -2.0,
        end_deg: 2.0,
        step_deg: 2.0,
        total_frames: 3,
        fps: 30,
        width: 16,
        height: 16,
        samples_per_pixel: 1,
        color_space: ColorSpace::Srgb,
        out_dir: std::env::temp_dir(),
        out_name: "clip".to_string(),
        selection: metrics::MetricSelection::default(),
        curves: Vec::new(),
        keep_frames: false,
        remote: RemoteSelection::local_only(),
        local_compute: LocalComputeTarget::Cpu,
        adaptive: None,
        path_cache: None,
        beam: None,
    }
}

/// A tilt video job freezes the Stone Shadow its request draws, as the job format that carries it,
/// and the job rebuilds the same scene for the frame loop. A request without it is the format 1
/// file it always was.
#[test]
fn a_tilt_video_job_keeps_the_stone_shadow_its_request_draws() {
    let request = effects_video_request(true, 0.0);
    let job = convert::build_video_job(
        &request,
        "Shadow video".to_string(),
        DesignInfo::default(),
        "clip".to_string(),
        0,
    )
    .expect("the tilt video job builds");
    assert!(job.scene.stone_shadow);
    assert_eq!(job.format, 3, "the stone shadow is job format 3");
    let text = codec::to_text(&job).expect("encodes");
    let back = codec::from_text(&text).expect("decodes");
    assert!(convert::snapshot_from_job(&back, None).stone_shadow);

    let plain = convert::build_video_job(
        &effects_video_request(false, 0.0),
        "Plain video".to_string(),
        DesignInfo::default(),
        "clip".to_string(),
        0,
    )
    .expect("the tilt video job builds");
    assert!(!plain.scene.stone_shadow);
    assert!(plain.format <= 2, "no stone-shadow job format");
}

/// A tilt video job freezes the Tray Reflection its request draws, in the format that carries it,
/// and reads it back; a request without it freezes none.
#[test]
fn a_tilt_video_job_keeps_the_tray_reflection_its_request_draws() {
    let request = effects_video_request(false, 0.04);
    let job = convert::build_video_job(
        &request,
        "Reflection video".to_string(),
        DesignInfo::default(),
        "clip".to_string(),
        0,
    )
    .expect("the tilt video job builds");
    assert_eq!(job.scene.tray_reflection.to_bits(), 0.04f32.to_bits());
    assert_eq!(job.format, 4, "the tray reflection is job format 4");
    let text = codec::to_text(&job).expect("encodes");
    let back = codec::from_text(&text).expect("decodes");
    assert_eq!(
        convert::snapshot_from_job(&back, None)
            .tray_reflection
            .to_bits(),
        0.04f32.to_bits()
    );

    let plain = convert::build_video_job(
        &effects_video_request(false, 0.0),
        "Plain video".to_string(),
        DesignInfo::default(),
        "clip".to_string(),
        0,
    )
    .expect("the tilt video job builds");
    assert_eq!(plain.scene.tray_reflection.to_bits(), 0);
    assert!(plain.format <= 3, "no tray-reflection job format");
}

/// A still job keeps the Tray Reflection through the frozen scene and back into a snapshot.
#[test]
fn a_still_jobs_scene_carries_the_tray_reflection_both_ways() {
    let snapshot = capture(RenderContext {
        tray_reflection: 0.04,
        ..RenderContext::default()
    });
    let scene = convert::scene_from_snapshot(&snapshot, 16, 16);
    assert_eq!(scene.tray_reflection.to_bits(), 0.04f32.to_bits());
    let restored = convert::snapshot_from_scene(&scene, None);
    assert_eq!(restored.tray_reflection.to_bits(), 0.04f32.to_bits());

    // The built job file is the format that carries the reflection, and it keeps the value.
    let remote = RemoteSelection::local_only();
    let output = std::env::temp_dir().join("tray-reflection-job.png");
    let job = convert::build_still_job(
        &still_inputs(&snapshot, tiny_params(), &remote, &output),
        1_791_302_580,
    )
    .expect("the job builds");
    assert_eq!(job.format, 4, "the tray reflection is job format 4");
    assert_eq!(job.scene.tray_reflection.to_bits(), 0.04f32.to_bits());
}

/// Records what the executor reports, and can raise `cancel` after a given frame.
struct Recorder<'a> {
    finished: Vec<u32>,
    progress_ticks: usize,
    cancel_after: Option<(u32, &'a AtomicBool)>,
}

impl<'a> Recorder<'a> {
    const fn new() -> Self {
        Self {
            finished: Vec::new(),
            progress_ticks: 0,
            cancel_after: None,
        }
    }

    const fn cancelling_after(frames: u32, cancel: &'a AtomicBool) -> Self {
        Self {
            finished: Vec::new(),
            progress_ticks: 0,
            cancel_after: Some((frames, cancel)),
        }
    }
}

impl JobSink for Recorder<'_> {
    fn progress(&mut self, _progress: &JobProgress) {
        self.progress_ticks += 1;
    }

    fn frame_finished(&mut self, frames_done: u32, _frames_total: u32) {
        self.finished.push(frames_done);
        if let Some((after, cancel)) = self.cancel_after
            && frames_done == after
        {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// A video stopped after one frame keeps it, and the next run renders only the missing
/// frames, leaving the first frame's file untouched.
#[test]
fn j3_t2_a_resumed_video_skips_the_frames_already_on_disk() {
    let dir = test_dir("t2");
    let job = video_job(&dir, true);
    let ctx = context(&dir);

    let cancel = AtomicBool::new(false);
    let mut first = Recorder::cancelling_after(1, &cancel);
    let outcome = execute_job(&job, &ctx, &cancel, &mut first);
    assert_eq!(outcome, JobOutcome::Stopped { frames_done: 1 });
    assert_eq!(frames::existing_frames(&dir, 3), vec![true, false, false]);
    assert!(
        dir.join(FRAME_MARKER_FILE).is_file(),
        "the marker names the job"
    );
    let frame_one = frames::frame_path(&dir, 0, 3);
    let bytes = std::fs::read(&frame_one).unwrap();
    let modified = std::fs::metadata(&frame_one).unwrap().modified().unwrap();

    let mut second = Recorder::new();
    let outcome = execute_job(&job, &ctx, &AtomicBool::new(false), &mut second);
    let JobOutcome::Done { result, .. } = outcome else {
        panic!("the resumed video must finish, got {outcome:?}");
    };
    assert_eq!(
        second.finished,
        vec![2, 3],
        "only the missing frames render"
    );
    assert!(second.progress_ticks > 0);
    assert!(
        result.exists(),
        "the video, the GIF or the frame folder is reported"
    );
    assert_eq!(std::fs::read(&frame_one).unwrap(), bytes);
    assert_eq!(
        std::fs::metadata(&frame_one).unwrap().modified().unwrap(),
        modified,
        "the first frame must not be written again"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The frame folder guards and the restart.
#[test]
fn j3_t3_the_frame_folder_guards_protect_other_exports() {
    // Another job's marker.
    let dir = test_dir("t3_marker");
    let job = video_job(&dir, true);
    std::fs::write(
        dir.join(FRAME_MARKER_FILE),
        marker_text("ffffffffffffffffffffffffffffffff"),
    )
    .unwrap();
    let outcome = execute_job(
        &job,
        &context(&dir),
        &AtomicBool::new(false),
        &mut Recorder::new(),
    );
    assert!(
        matches!(&outcome, JobOutcome::Failed { kind: FailureKind::Input, message, .. }
            if message.contains("belongs to another render job")),
        "{outcome:?}"
    );

    // Frames from another export, no marker.
    let dir = test_dir("t3_foreign");
    let job = video_job(&dir, true);
    let foreign = frames::frame_path(&dir, 0, 3);
    std::fs::write(&foreign, b"not a picture").unwrap();
    let outcome = execute_job(
        &job,
        &context(&dir),
        &AtomicBool::new(false),
        &mut Recorder::new(),
    );
    assert!(
        matches!(&outcome, JobOutcome::Failed { kind: FailureKind::Input, message, .. }
            if message.contains("already holds frames from another export")),
        "{outcome:?}"
    );
    assert_eq!(std::fs::read(&foreign).unwrap(), b"not a picture");

    // The same folder with a restart: the old frames go, the video renders.
    let restart = ExecContext {
        restart_frames: true,
        ..context(&dir)
    };
    let outcome = execute_job(
        &job,
        &restart,
        &AtomicBool::new(false),
        &mut Recorder::new(),
    );
    assert!(matches!(outcome, JobOutcome::Done { .. }), "{outcome:?}");
    assert!(
        image::open(&foreign).is_ok(),
        "the foreign frame was replaced"
    );

    // A stray partial frame of this job's own folder is removed.
    let dir = test_dir("t3_partial");
    let job = video_job(&dir, true);
    std::fs::write(dir.join(FRAME_MARKER_FILE), marker_text(&job.token)).unwrap();
    let partial = dir.join("frame_1.png.partial");
    std::fs::write(&partial, b"half a frame").unwrap();
    let stopped = AtomicBool::new(true);
    let outcome = execute_job(&job, &context(&dir), &stopped, &mut Recorder::new());
    assert_eq!(outcome, JobOutcome::Stopped { frames_done: 0 });
    assert!(!partial.exists(), "the partial frame must be removed");
    let _ = std::fs::remove_dir_all(&dir);
}

pub(super) fn write_test_hdr(dir: &Path) -> PathBuf {
    let pixels: Vec<image::Rgb<f32>> = (0..64)
        .map(|i| image::Rgb([(i as f32).mul_add(0.01, 0.2), 0.5, 0.25]))
        .collect();
    let mut bytes = Vec::new();
    image::codecs::hdr::HdrEncoder::new(&mut bytes)
        .encode(&pixels, 16, 4)
        .unwrap();
    let path = dir.join("studio.hdr");
    std::fs::write(&path, &bytes).unwrap();
    path
}

/// A still job whose scene names the HDR map `hash`, stored at `path`.
pub(super) fn hdr_job(path: &Path, hash: &[u8; 32]) -> RenderJobFile {
    let remote = RemoteSelection::local_only();
    let output = path.with_extension("png");
    let mut job = convert::build_still_job(
        &still_inputs(
            &capture(RenderContext::default()),
            tiny_params(),
            &remote,
            &output,
        ),
        0,
    )
    .unwrap();
    job.scene.environment = SceneEnvironment::Hdr(HdrEnvironment {
        content_hash: *hash,
        width: 16,
        height: 4,
    });
    job.hdr = Some(HdrSource {
        path: path.to_string_lossy().into_owned(),
        sha256_hex: hash_hex(hash),
    });
    job.validate().expect("the HDR job is consistent");
    job
}

/// A missing HDR map and a changed one fail the job with a plain sentence.
#[test]
fn j3_t4_a_missing_or_changed_hdr_map_fails_the_job() {
    let dir = test_dir("t4");

    let missing = dir.join("missing.hdr");
    let job = hdr_job(&missing, &[7; 32]);
    let (kind, message) = convert::load_hdr(&job, &dir).unwrap_err();
    assert_eq!(kind, FailureKind::Input);
    assert_eq!(
        message,
        format!("The HDR map {} is missing.", missing.display())
    );
    let outcome = execute_job(
        &job,
        &context(&dir),
        &AtomicBool::new(false),
        &mut Recorder::new(),
    );
    assert!(
        matches!(&outcome, JobOutcome::Failed { kind: FailureKind::Input, message, .. }
            if message.ends_with("is missing.")),
        "{outcome:?}"
    );

    let path = write_test_hdr(&dir);
    let bytes = std::fs::read(&path).unwrap();
    let hash = pleochroma_net::messages::content_hash(&bytes);
    let job = hdr_job(&path, &hash);
    let loaded = convert::load_hdr(&job, &dir).expect("the unchanged map loads");
    assert!(loaded.is_some());

    let mut changed = bytes;
    changed.push(0);
    std::fs::write(&path, changed).unwrap();
    let (kind, message) = convert::load_hdr(&job, &dir).unwrap_err();
    assert_eq!(kind, FailureKind::Input);
    assert!(
        message.contains("changed since the job was added"),
        "{message}"
    );

    let mut no_hdr = job;
    no_hdr.hdr = None;
    no_hdr.scene.environment = SceneEnvironment::Studio;
    assert!(convert::load_hdr(&no_hdr, &dir).unwrap().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_still_job_renders_and_never_overwrites_an_existing_picture() {
    let dir = test_dir("still");
    let remote = RemoteSelection::local_only();
    let output = dir.join("gem.png");
    let job = convert::build_still_job(
        &still_inputs(
            &capture(RenderContext::default()),
            tiny_params(),
            &remote,
            &output,
        ),
        0,
    )
    .unwrap();
    let ctx = context(&dir);

    let mut recorder = Recorder::new();
    let outcome = execute_job(&job, &ctx, &AtomicBool::new(false), &mut recorder);
    assert_eq!(
        outcome,
        JobOutcome::Done {
            result: output.clone(),
            note: None
        }
    );
    assert!(recorder.progress_ticks > 0);

    let outcome = execute_job(&job, &ctx, &AtomicBool::new(false), &mut Recorder::new());
    let JobOutcome::Done { result, .. } = outcome else {
        panic!("the second run must finish");
    };
    assert_eq!(result, dir.join("gem (2).png"));
    assert!(output.is_file() && result.is_file());

    let stopped = execute_job(&job, &ctx, &AtomicBool::new(true), &mut Recorder::new());
    assert_eq!(stopped, JobOutcome::Stopped { frames_done: 0 });
    let _ = std::fs::remove_dir_all(&dir);
}

/// A video queued with "Remote only" freezes that choice, keeps it through the job file's
/// text, and runs as `RemoteOnly` with the worker read at run time (never stored in the job).
#[test]
fn a_queued_video_keeps_its_compute_choice_through_the_job_file() {
    use crate::bridge::export_thread::ComputeTarget;
    let dir = test_dir("video_compute");
    let remote = RemoteSelection {
        compute_target: ComputeTarget::RemoteOnly,
        worker: None,
        transfer: crate::settings::ExportTransfer::FullData,
        contribute_local: false,
    };
    let job = video_job_with(&dir, false, remote);
    assert_eq!(job.compute.target, ComputeChoice::Remote);
    let back =
        codec::from_text(&codec::to_text(&job).expect("the job encodes")).expect("the job decodes");
    assert_eq!(back.compute.target, ComputeChoice::Remote);

    let worker = crate::settings::WorkerSettings {
        address: "render-box.local:7878".to_string(),
        ..crate::settings::WorkerSettings::default()
    };
    let ctx = ExecContext {
        worker: Some(worker.clone()),
        ..context(&dir)
    };
    let selection = remote_selection_for(&back, &ctx);
    assert_eq!(selection.compute_target, ComputeTarget::RemoteOnly);
    assert_eq!(selection.worker, Some(worker));

    // The default for a local video stays local.
    assert_eq!(video_job(&dir, false).compute.target, ComputeChoice::Local);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn overrides_replace_the_jobs_compute_choices_and_the_worker_comes_from_the_context() {
    let dir = test_dir("override");
    let remote = RemoteSelection::local_only();
    let output = dir.join("gem.png");
    let job = convert::build_still_job(
        &still_inputs(
            &capture(RenderContext::default()),
            tiny_params(),
            &remote,
            &output,
        ),
        0,
    )
    .unwrap();
    assert_eq!(job.compute.target, ComputeChoice::Local);

    let plain = remote_selection_for(&job, &context(&dir));
    assert_eq!(plain, remote);

    let worker = crate::settings::WorkerSettings {
        address: "render-box.local:7878".to_string(),
        ..crate::settings::WorkerSettings::default()
    };
    let ctx = ExecContext {
        worker: Some(worker.clone()),
        compute_override: Some(ComputeChoice::Remote),
        transfer_override: Some(TransferChoice::FinalPicture),
        contribute_local_override: Some(true),
        ..context(&dir)
    };
    let overridden = remote_selection_for(&job, &ctx);
    assert_eq!(
        overridden.compute_target,
        crate::bridge::export_thread::ComputeTarget::RemoteOnly
    );
    assert_eq!(
        overridden.transfer,
        crate::settings::ExportTransfer::FinalPicture
    );
    assert!(overridden.contribute_local);
    assert_eq!(overridden.worker, Some(worker));
    let _ = std::fs::remove_dir_all(&dir);
}
