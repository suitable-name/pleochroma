//! Tests of the estimator rules of the job executor: the token rule, the scene's estimator, the
//! path cache and beam policy, and the frame-folder guard.
//!
//! They run through the pure functions of [`super::execute`] with the estimator as an argument.
//! None of them sets the process default estimator (`set_default_estimator`): the library
//! default is branching in this crate's tests, and changing it would race with the other tests
//! of the binary.

#[cfg(all(feature = "beam", beam_tests))]
use super::execute::BEAM_OFF_NOTE;
use super::{
    convert::{self, StillJobInputs},
    execute::{
        FolderRule, PATH_CACHE_OFF_NOTE, accel_for_estimator, estimator_change_hint,
        job_for_estimator,
    },
    headless::changed_token,
};
use crate::{
    bridge::{
        export_thread::{ExportParams, RemoteSelection, SceneSnapshot},
        render_thread::RenderContext,
    },
    gui::tilt::video_export::{metrics, run::VideoExportRequest},
    settings::LocalComputeTarget,
};
use pleochroma::{
    color::ColorSpace,
    optics::raytracer::Estimator,
    renderer::{
        accel::{PathCacheSettings, RenderAccel},
        accel_beam::BeamSettings,
    },
};
use pleochroma_render_jobs::{JobKind, RenderJobFile, job::DesignInfo};
use std::{path::Path, sync::Mutex};

const BRANCHING: Estimator = Estimator::Branching;
const COLLECT: Estimator = Estimator::ExitCollection;

/// An accel with the path cache and the beams on or off.
fn accel(path_cache: bool, beam: bool) -> RenderAccel {
    RenderAccel {
        adaptive: None,
        path_cache: path_cache.then(PathCacheSettings::default),
        beam: beam.then(BeamSettings::default),
    }
}

/// A 16 x 16 still job that records the branching estimator.
fn still_job(accel: &RenderAccel) -> RenderJobFile {
    let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
        .expect("the material resolves");
    let remote = RemoteSelection::local_only();
    let inputs = StillJobInputs {
        scene: &scene,
        params: ExportParams {
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            width: 16,
            height: 16,
            samples_per_pixel: 2,
            max_bounces: 4,
        },
        color_space: ColorSpace::Srgb,
        remote: &remote,
        output: Path::new("estimator-job.png"),
        label: "Test job".to_string(),
        design: DesignInfo::default(),
        preset_label: String::new(),
        adaptive: None,
        path_cache: accel.path_cache,
        beam: accel.beam,
    };
    let mut job = convert::build_still_job(&inputs, 1_791_302_580).expect("the job builds");
    job.scene.estimator = BRANCHING;
    job
}

/// A three-frame tilt video job that records branching, with the path cache kept on disk when it
/// has one.
fn video_job(accel: &RenderAccel) -> RenderJobFile {
    let request = VideoExportRequest {
        scene: SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("the material resolves"),
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
        out_dir: Path::new("estimator-clip").to_path_buf(),
        out_name: "clip".to_string(),
        selection: metrics::MetricSelection::default(),
        curves: Vec::new(),
        keep_frames: false,
        remote: RemoteSelection::local_only(),
        local_compute: LocalComputeTarget::Cpu,
        adaptive: None,
        path_cache: accel.path_cache.map(|cache| PathCacheSettings {
            persist: true,
            ..cache
        }),
        beam: accel.beam,
    };
    let mut job = convert::build_video_job(
        &request,
        "Test video".to_string(),
        DesignInfo::default(),
        "clip".to_string(),
        0,
    )
    .expect("the video job builds");
    job.scene.estimator = BRANCHING;
    job
}

/// The accelerations a job carries.
const fn accel_of(job: &RenderJobFile) -> &RenderAccel {
    match &job.kind {
        JobKind::Still(still) => &still.accel,
        JobKind::TiltVideo(video) => &video.accel,
    }
}

#[test]
fn accel_for_estimator_leaves_branching_alone() {
    let asked = accel(true, true);
    let (back, notes) = accel_for_estimator(&asked, BRANCHING);
    assert_eq!(back, asked);
    assert!(notes.is_empty(), "{notes:?}");
    let (back, notes) = accel_for_estimator(&accel(false, false), COLLECT);
    assert_eq!(back, accel(false, false));
    assert!(
        notes.is_empty(),
        "nothing to drop, nothing to say: {notes:?}"
    );
}

#[test]
fn accel_for_estimator_drops_the_path_cache_under_exit_collection_with_its_note() {
    assert_eq!(
        PATH_CACHE_OFF_NOTE,
        "Path cache off: exit collection, the default estimator, does not use it."
    );
    let (back, notes) = accel_for_estimator(&accel(true, false), COLLECT);
    assert_eq!(back.path_cache, None);
    assert_eq!(notes, [PATH_CACHE_OFF_NOTE]);
}

/// A build with the `beam` feature drops the beams too, each with its own note.
#[cfg(all(feature = "beam", beam_tests))]
#[test]
fn accel_for_estimator_drops_the_beams_under_exit_collection_in_a_beam_build() {
    assert_eq!(
        BEAM_OFF_NOTE,
        "Beam tracing off: exit collection, the default estimator, does not use it."
    );
    let (back, notes) = accel_for_estimator(&accel(true, true), COLLECT);
    assert_eq!((back.path_cache, back.beam), (None, None));
    assert_eq!(notes, [PATH_CACHE_OFF_NOTE, BEAM_OFF_NOTE]);
    let (back, notes) = accel_for_estimator(&accel(false, true), COLLECT);
    assert_eq!(back.beam, None);
    assert_eq!(notes, [BEAM_OFF_NOTE]);
}

/// A build without the `beam` feature leaves `accel.beam` as it is, so the render's own note
/// ("Beam tracing off: this build has no beam tracer") is the only beam note.
#[cfg(not(feature = "beam"))]
#[test]
fn accel_for_estimator_leaves_the_beams_to_the_builds_own_note_without_the_beam_feature() {
    let (back, notes) = accel_for_estimator(&accel(true, true), COLLECT);
    assert_eq!(back.path_cache, None);
    assert_eq!(
        back.beam,
        Some(BeamSettings::default()),
        "the beams are untouched"
    );
    assert_eq!(notes, [PATH_CACHE_OFF_NOTE]);
    let (back, notes) = accel_for_estimator(&accel(false, true), COLLECT);
    assert_eq!(back.beam, Some(BeamSettings::default()));
    assert!(notes.is_empty(), "no second beam note: {notes:?}");
}

/// The Ray-Traced engine has no estimator and no light-path cache: its job keeps its token, its
/// estimator and its accelerations, and gets no note.
#[test]
fn a_ray_traced_job_is_left_alone_by_the_estimator_rule() {
    use pleochroma::renderer::engine::{EngineSpec, RenderEngine};
    let ray = EngineSpec {
        engine: RenderEngine::RayTraced,
        ..EngineSpec::default()
    };
    for mut job in [still_job(&accel(true, true)), video_job(&accel(true, true))] {
        job.engine = ray;
        let ruled = job_for_estimator(&job, COLLECT);
        assert_eq!(
            ruled.job, job,
            "token, estimator and caches are as recorded"
        );
        assert!(ruled.change.is_none());
        assert!(ruled.notes.is_empty(), "{:?}", ruled.notes);
    }
}

#[test]
fn an_old_branching_job_run_with_exit_collection_gets_the_estimator_token() {
    let job = still_job(&accel(false, false));
    let ruled = job_for_estimator(&job, COLLECT);
    assert_eq!(
        ruled.job.token,
        changed_token(&job.token, "estimator=collect")
    );
    assert_ne!(ruled.job.token, job.token);
    assert_eq!(ruled.job.token.len(), 32);
    let change = ruled
        .change
        .expect("the estimator differs from the recorded one");
    assert_eq!(change.recorded, BRANCHING);
    assert_eq!(change.old_token, job.token);
    // Everything else of the job is as it was.
    let mut expected = job;
    expected.token = ruled.job.token.clone();
    expected.scene.estimator = COLLECT;
    assert_eq!(ruled.job, expected);
}

#[test]
fn a_job_recorded_with_exit_collection_keeps_its_token_under_exit_collection() {
    let mut job = still_job(&accel(false, false));
    job.scene.estimator = COLLECT;
    let ruled = job_for_estimator(&job, COLLECT);
    assert_eq!(ruled.job, job);
    assert!(ruled.change.is_none());
    assert_eq!(ruled.notes, [] as [&str; 0]);
}

#[test]
fn a_job_recorded_with_exit_collection_run_with_branching_gets_the_branching_token() {
    let mut job = still_job(&accel(false, false));
    job.scene.estimator = COLLECT;
    let ruled = job_for_estimator(&job, BRANCHING);
    assert_eq!(
        ruled.job.token,
        changed_token(&job.token, "estimator=branching")
    );
    assert_eq!(ruled.job.scene.estimator, BRANCHING);
    assert_eq!(ruled.change.map(|change| change.recorded), Some(COLLECT));
    // The two estimators never share a token.
    assert_ne!(
        changed_token(&job.token, "estimator=branching"),
        changed_token(&job.token, "estimator=collect")
    );
}

#[test]
fn a_branching_job_run_with_branching_is_exactly_the_job() {
    for job in [still_job(&accel(true, true)), video_job(&accel(true, true))] {
        let ruled = job_for_estimator(&job, BRANCHING);
        assert_eq!(ruled.job, job, "off means identical");
        assert!(ruled.change.is_none());
        assert!(ruled.notes.is_empty(), "{:?}", ruled.notes);
    }
}

#[test]
fn the_scene_of_the_job_carries_the_estimator_it_is_rendered_with() {
    for job in [
        still_job(&accel(false, false)),
        video_job(&accel(false, false)),
    ] {
        for estimator in [BRANCHING, COLLECT] {
            let ruled = job_for_estimator(&job, estimator);
            assert_eq!(ruled.job.scene.estimator, estimator);
        }
        // A job recorded with the other estimator is rewritten, so remote parts carry it too.
        let mut other = job.clone();
        other.scene.estimator = COLLECT;
        assert_eq!(
            job_for_estimator(&other, BRANCHING).job.scene.estimator,
            BRANCHING
        );
    }
}

#[test]
fn exit_collection_applies_the_cache_policy_to_stills_and_tilt_videos_alike() {
    let asked = accel(true, false);
    for job in [still_job(&asked), video_job(&asked)] {
        assert!(
            accel_of(&job).path_cache.is_some(),
            "the fixture asks for the cache"
        );
        let ruled = job_for_estimator(&job, COLLECT);
        assert_eq!(accel_of(&ruled.job).path_cache, None);
        assert_eq!(ruled.notes, [PATH_CACHE_OFF_NOTE]);
        if let JobKind::TiltVideo(video) = &ruled.job.kind {
            assert!(!video.keep_path_cache, "a video keeps no cache that is off");
        }
        // Branching keeps the cache (and a video keeps asking to persist it).
        let kept = job_for_estimator(&job, BRANCHING);
        assert_eq!(accel_of(&kept.job).path_cache, accel_of(&job).path_cache);
        if let (JobKind::TiltVideo(before), JobKind::TiltVideo(after)) = (&job.kind, &kept.job.kind)
        {
            assert!(before.keep_path_cache, "the fixture keeps the cache");
            assert_eq!(after.keep_path_cache, before.keep_path_cache);
        }
    }
}

#[test]
fn a_frame_folder_of_the_recorded_estimator_is_refused_with_the_way_out() {
    let old = "0123456789abcdef0123456789abcdef";
    let job = still_job(&accel(false, false));
    let ruled = job_for_estimator(&job, COLLECT);
    let change = ruled.change.expect("the token changed");
    let rule = |restart: bool| FolderRule {
        token: &ruled.job.token,
        restart,
        change: Some(&change),
        estimator: COLLECT,
    };
    // The folder of the job's own token before the rule: refused, naming both ways out.
    let owner = Some(job.token.as_str());
    assert!(rule(false).refuses(owner));
    let hint = rule(false).hint(owner);
    assert!(hint.contains("--restart"), "{hint}");
    assert!(hint.contains("Restart"), "{hint}");
    assert!(!hint.contains("--estimator"), "{hint}");
    assert!(!hint.contains("--path-cache"), "{hint}");
    assert_eq!(hint, estimator_change_hint(BRANCHING, COLLECT));
    // Frames of exit collection that a branching run met are continued without the cache and
    // beam flags.
    let flags = estimator_change_hint(COLLECT, BRANCHING);
    assert!(flags.contains("--path-cache"), "{flags}");
    assert!(flags.contains("--restart"), "{flags}");
    assert!(flags.contains("Restart"), "{flags}");
    assert!(!flags.contains("--estimator"), "{flags}");
    // A restart takes that folder over.
    assert!(!rule(true).refuses(owner));
    // The run's own folder is never refused.
    assert!(!rule(false).refuses(Some(ruled.job.token.as_str())));
    // Another job's folder is refused with no estimator advice, restart or not.
    assert!(rule(false).refuses(Some(old)));
    assert!(rule(true).refuses(Some(old)));
    assert_eq!(rule(false).hint(Some(old)), "");
    assert!(rule(false).refuses(None));
    // Without a token change the guard is the plain one.
    let plain = FolderRule {
        token: old,
        restart: true,
        change: None,
        estimator: BRANCHING,
    };
    assert!(plain.refuses(Some(job.token.as_str())));
    assert!(!plain.refuses(Some(old)));
    assert_eq!(plain.hint(Some(job.token.as_str())), "");
}
