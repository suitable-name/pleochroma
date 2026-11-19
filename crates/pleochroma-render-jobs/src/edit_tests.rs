use crate::{
    ComputeChoice, JobColorSpace, RenderJobFile, TransferChoice,
    codec::to_text,
    edit::{
        EditMode, EditRefusal, JobEdit, color_space_of, edit_rule, label_after_edit, samples_of,
        set_samples, video_of,
    },
    format_for,
    job::{
        JOB_FORMAT_VERSION, JobKind, OverlaySelection, TiltVideoJob,
        test_support::{curves, posed_still_job, still_job, video_job, zoned_still_job},
    },
    state::JobState,
};
use pleochroma::{
    optics::{fluorescence::Fluorescence, materials::GemMaterial},
    renderer::engine::{EngineSpec, RayOptions, RenderEngine},
};
use std::path::PathBuf;

fn ray_job() -> RenderJobFile {
    let engine = EngineSpec {
        engine: RenderEngine::RayTraced,
        ray: RayOptions {
            aa_taps: 8,
            fire: true,
        },
    };
    let mut job = still_job();
    job.engine = engine;
    job.scene.engine = engine;
    job.compute.target = ComputeChoice::Local;
    set_samples(&mut job, 8);
    job.format = format_for(&job);
    job
}

fn other_material() -> GemMaterial {
    GemMaterial::all_materials()
        .into_iter()
        .find(|material| *material != GemMaterial::diamond())
        .expect("the built-in table holds more than one material")
}

fn video(job: &RenderJobFile) -> &TiltVideoJob {
    video_of(job).expect("the test job is a video")
}

#[test]
fn an_empty_edit_keeps_every_job_byte_for_byte() {
    let none = JobEdit::default();
    for job in [
        still_job(),
        video_job(),
        ray_job(),
        posed_still_job(),
        zoned_still_job(),
    ] {
        let after = none.apply(&job).expect("an untouched job stays valid");
        assert_eq!(after, job);
        assert_eq!(to_text(&after), to_text(&job));
        assert!(!none.changes_pixels(&job));
        assert!(!none.resets_progress(&job));
        assert!(none.only_label_or_name());
        assert!(!none.needs_curves(&job));
    }
}

#[test]
fn the_format_stays_where_the_job_puts_it() {
    assert_eq!(JOB_FORMAT_VERSION, 8);
    let posed = posed_still_job();
    let edit = JobEdit {
        label: Some("Posed".to_string()),
        ..JobEdit::default()
    };
    let after = edit.apply(&posed).expect("a label edit applies");
    assert_eq!(after.format, 8);
    assert_eq!(after.token, posed.token);
}

#[test]
fn every_still_field_applies() {
    let before = still_job();
    let edit = JobEdit {
        target: Some(ComputeChoice::Remote),
        transfer: Some(TransferChoice::FinalPicture),
        size: Some((1920, 1080)),
        samples_per_pixel: Some(128),
        max_bounces: Some(9),
        color_space: Some(JobColorSpace::Rec2020),
        output: Some(PathBuf::from("renders/new.png")),
        label: Some("A new label".to_string()),
        material: Some(other_material()),
        fluorescence: Some(Fluorescence::default()),
        ..JobEdit::default()
    };
    let after = edit.apply(&before).expect("the edit applies");
    assert_eq!(after.compute.target, ComputeChoice::Remote);
    assert_eq!(after.compute.transfer, TransferChoice::FinalPicture);
    assert_eq!((after.scene.width, after.scene.height), (1920, 1080));
    assert_eq!(samples_of(&after), 128);
    assert_eq!(after.scene.max_bounces, 9);
    assert_eq!(color_space_of(&after), JobColorSpace::Rec2020);
    assert_eq!(
        after.output,
        PathBuf::from("renders/new.png").to_string_lossy()
    );
    assert_eq!(after.label, "A new label");
    assert_eq!(after.scene.material, other_material());
    assert_eq!(after.token, before.token);
    assert_eq!(after.created_at, before.created_at);
}

#[test]
fn zoning_can_be_removed_and_a_material_change_keeps_the_rest() {
    let zoned = zoned_still_job();
    assert!(zoned.zoning.is_some());
    let edit = JobEdit {
        zoning: Some(None),
        ..JobEdit::default()
    };
    let after = edit.apply(&zoned).expect("removing zones applies");
    assert!(after.zoning.is_none());
    assert!(edit.changes_pixels(&zoned));
    assert_eq!(after.format, format_for(&after));
}

#[test]
fn the_remote_worker_is_refused_for_a_ray_job() {
    let ray = ray_job();
    for target in [ComputeChoice::Remote, ComputeChoice::Both] {
        let edit = JobEdit {
            target: Some(target),
            ..JobEdit::default()
        };
        assert_eq!(edit.apply(&ray), Err(EditRefusal::RayRemote));
    }
    let local = JobEdit {
        target: Some(ComputeChoice::Local),
        ..JobEdit::default()
    };
    assert!(local.apply(&ray).is_ok());
}

#[test]
fn ray_taps_set_both_engine_copies_and_the_samples() {
    let ray = ray_job();
    let edit = JobEdit {
        ray_taps: Some(16),
        ray_fire: Some(false),
        ..JobEdit::default()
    };
    let after = edit.apply(&ray).expect("taps and fire apply");
    assert_eq!(after.engine.ray.aa_taps, 16);
    assert_eq!(after.scene.engine.ray.aa_taps, 16);
    assert!(!after.engine.ray.fire);
    assert!(!after.scene.engine.ray.fire);
    assert_eq!(samples_of(&after), 16);
    assert!(edit.changes_pixels(&ray));
}

#[test]
fn a_ray_job_refuses_samples_and_a_path_job_refuses_taps() {
    let samples = JobEdit {
        samples_per_pixel: Some(64),
        ..JobEdit::default()
    };
    assert!(matches!(
        samples.apply(&ray_job()),
        Err(EditRefusal::Invalid(_))
    ));
    for edit in [
        JobEdit {
            ray_taps: Some(4),
            ..JobEdit::default()
        },
        JobEdit {
            ray_fire: Some(false),
            ..JobEdit::default()
        },
    ] {
        assert!(matches!(
            edit.apply(&still_job()),
            Err(EditRefusal::Invalid(_))
        ));
    }
    let odd = JobEdit {
        ray_taps: Some(5),
        ..JobEdit::default()
    };
    assert!(matches!(
        odd.apply(&ray_job()),
        Err(EditRefusal::Invalid(_))
    ));
}

#[test]
fn a_video_field_on_a_still_is_refused() {
    let edit = JobEdit {
        fps: Some(24),
        ..JobEdit::default()
    };
    assert!(matches!(
        edit.apply(&still_job()),
        Err(EditRefusal::Invalid(_))
    ));
}

#[test]
fn an_invalid_value_shows_the_validation_sentence() {
    let edit = JobEdit {
        size: Some((4, 4)),
        ..JobEdit::default()
    };
    let Err(EditRefusal::Invalid(sentence)) = edit.apply(&still_job()) else {
        panic!("a four pixel picture is refused");
    };
    assert!(sentence.contains("picture size"));
}

#[test]
fn every_video_field_applies() {
    let before = video_job();
    let edit = JobEdit {
        fps: Some(24),
        keep_frames: Some(false),
        video_name: Some("Renamed".to_string()),
        size: Some((128, 96)),
        ..JobEdit::default()
    };
    let after = edit.apply(&before).expect("the edit applies");
    let v = video(&after);
    assert_eq!(v.fps, 24);
    assert!(!v.keep_frames);
    assert_eq!(v.video_name, "Renamed");
    assert_eq!((after.scene.width, after.scene.height), (128, 96));
    assert_eq!(v.curves, video(&before).curves);
}

#[test]
fn a_sweep_sets_the_frame_count() {
    let before = video_job();
    assert_eq!(video(&before).total_frames, 181);
    let edit = JobEdit {
        sweep: Some((0.0, 90.0, 5.0)),
        ..JobEdit::default()
    };
    let after = edit.apply(&before).expect("a sweep alone keeps the curves");
    let v = video(&after);
    assert_eq!(v.total_frames, 19);
    assert_eq!((v.start_deg, v.end_deg, v.step_deg), (0.0, 90.0, 5.0));
    assert!(v.curves.is_some());
    assert!(edit.changes_pixels(&before));
    assert!(!edit.needs_curves(&before));
}

#[test]
fn axes_need_curves_and_count_the_rocking_poses() {
    let before = video_job();
    let edit = JobEdit {
        sweep: Some((-10.0, 10.0, 10.0)),
        axes: Some(vec![0, 1]),
        ..JobEdit::default()
    };
    assert!(edit.needs_curves(&before));
    assert_eq!(edit.apply(&before), Err(EditRefusal::CurvesMissing));
    let with_curves = JobEdit {
        curves: Some(vec![curves(), curves()]),
        ..edit
    };
    let after = with_curves.apply(&before).expect("the curves are given");
    let v = video(&after);
    assert_eq!(v.axis_index, 0);
    assert_eq!(v.more_axes, vec![1]);
    assert_eq!(v.more_curves.len(), 1);
    assert_eq!(v.total_frames, 9);
}

#[test]
fn a_wrong_number_of_curve_sets_is_refused() {
    let edit = JobEdit {
        axes: Some(vec![0, 1]),
        curves: Some(vec![curves()]),
        ..JobEdit::default()
    };
    assert!(matches!(
        edit.apply(&video_job()),
        Err(EditRefusal::Invalid(_))
    ));
}

#[test]
fn a_material_change_needs_curves_while_an_overlay_is_on() {
    let before = video_job();
    let edit = JobEdit {
        material: Some(other_material()),
        ..JobEdit::default()
    };
    assert!(edit.needs_curves(&before));
    assert_eq!(edit.apply(&before), Err(EditRefusal::CurvesMissing));
    let same = JobEdit {
        material: Some(GemMaterial::diamond()),
        ..JobEdit::default()
    };
    assert!(!same.needs_curves(&before));
}

#[test]
fn switching_the_overlay_off_removes_the_curves() {
    let before = video_job();
    let edit = JobEdit {
        overlay: Some(OverlaySelection::default()),
        ..JobEdit::default()
    };
    assert!(!edit.needs_curves(&before));
    let after = edit.apply(&before).expect("the overlay goes off");
    let v = video(&after);
    assert!(v.curves.is_none());
    assert!(v.more_curves.is_empty());
    assert!(edit.changes_pixels(&before));
}

#[test]
fn an_overlay_switched_on_for_a_job_without_curves_needs_them() {
    let mut bare = video_job();
    if let JobKind::TiltVideo(v) = &mut bare.kind {
        v.overlay = OverlaySelection::default();
        v.curves = None;
    }
    let edit = JobEdit {
        overlay: Some(OverlaySelection {
            brilliance: true,
            ..OverlaySelection::default()
        }),
        ..JobEdit::default()
    };
    assert!(edit.needs_curves(&bare));
    assert_eq!(edit.apply(&bare), Err(EditRefusal::CurvesMissing));
    let with_curves = JobEdit {
        curves: Some(vec![curves()]),
        ..edit
    };
    assert!(with_curves.apply(&bare).is_ok());
}

#[test]
fn only_the_picture_fields_change_pixels() {
    let still = still_job();
    let quiet = [
        JobEdit {
            target: Some(ComputeChoice::Local),
            ..JobEdit::default()
        },
        JobEdit {
            transfer: Some(TransferChoice::FinalPicture),
            ..JobEdit::default()
        },
        JobEdit {
            label: Some("x".to_string()),
            ..JobEdit::default()
        },
        JobEdit {
            output: Some(PathBuf::from("a.png")),
            ..JobEdit::default()
        },
        JobEdit {
            samples_per_pixel: Some(64),
            ..JobEdit::default()
        },
    ];
    for edit in &quiet {
        assert!(!edit.changes_pixels(&still));
    }
    let loud = [
        JobEdit {
            size: Some((100, 100)),
            ..JobEdit::default()
        },
        JobEdit {
            samples_per_pixel: Some(65),
            ..JobEdit::default()
        },
        JobEdit {
            max_bounces: Some(5),
            ..JobEdit::default()
        },
        JobEdit {
            color_space: Some(JobColorSpace::DisplayP3),
            ..JobEdit::default()
        },
        JobEdit {
            material: Some(other_material()),
            ..JobEdit::default()
        },
    ];
    for edit in &loud {
        assert!(edit.changes_pixels(&still));
    }
    let video = video_job();
    for edit in [
        JobEdit {
            fps: Some(60),
            ..JobEdit::default()
        },
        JobEdit {
            keep_frames: Some(false),
            ..JobEdit::default()
        },
    ] {
        assert!(!edit.changes_pixels(&video));
    }
}

#[test]
fn a_new_frame_folder_resets_a_video_but_not_a_still() {
    let video = video_job();
    let name = JobEdit {
        video_name: Some("Other".to_string()),
        ..JobEdit::default()
    };
    let folder = JobEdit {
        output: Some(PathBuf::from("elsewhere")),
        ..JobEdit::default()
    };
    for edit in [&name, &folder] {
        assert!(!edit.changes_pixels(&video));
        assert!(edit.resets_progress(&video));
    }
    let same_folder = JobEdit {
        output: Some(PathBuf::from(&video.output)),
        ..JobEdit::default()
    };
    assert!(!same_folder.resets_progress(&video));
    assert!(!folder.resets_progress(&still_job()));
}

#[test]
fn only_a_label_or_name_is_only_a_label_or_name() {
    assert!(
        JobEdit {
            label: Some("x".to_string()),
            output: Some(PathBuf::from("a.png")),
            ..JobEdit::default()
        }
        .only_label_or_name()
    );
    for edit in [
        JobEdit {
            target: Some(ComputeChoice::Both),
            ..JobEdit::default()
        },
        JobEdit {
            transfer: Some(TransferChoice::FullData),
            ..JobEdit::default()
        },
        JobEdit {
            hdr_path: Some(PathBuf::from("map.hdr")),
            ..JobEdit::default()
        },
        JobEdit {
            video_name: Some("v".to_string()),
            ..JobEdit::default()
        },
        JobEdit {
            size: Some((64, 48)),
            ..JobEdit::default()
        },
    ] {
        assert!(!edit.only_label_or_name());
    }
}

#[test]
fn locating_an_hdr_map_changes_the_path_only() {
    let before = video_job();
    let edit = JobEdit {
        hdr_path: Some(PathBuf::from("maps/studio.hdr")),
        ..JobEdit::default()
    };
    let after = edit.apply(&before).expect("the path applies");
    let source = after.hdr.expect("the map stays");
    assert_eq!(
        source.path,
        PathBuf::from("maps/studio.hdr").to_string_lossy()
    );
    assert_eq!(
        source.sha256_hex,
        before.hdr.expect("the video has a map").sha256_hex
    );
    assert!(!edit.changes_pixels(&before));
    assert!(matches!(
        edit.apply(&still_job()),
        Err(EditRefusal::Invalid(_))
    ));
}

#[test]
fn edit_rule_has_six_rows() {
    assert_eq!(edit_rule(JobState::Queued), Ok(EditMode::InPlace));
    assert_eq!(edit_rule(JobState::Paused), Ok(EditMode::InPlace));
    assert_eq!(edit_rule(JobState::Failed), Ok(EditMode::KeepFramesRequeue));
    assert_eq!(edit_rule(JobState::Done), Ok(EditMode::RequeueWithConfirm));
    assert_eq!(
        edit_rule(JobState::Cancelled),
        Ok(EditMode::RequeueWithConfirm)
    );
    assert!(edit_rule(JobState::Running).is_err());
}

#[test]
fn the_label_follows_only_while_it_is_automatic() {
    assert_eq!(label_after_edit("Auto A", "Auto A", "Auto B"), "Auto B");
    assert_eq!(label_after_edit("Typed", "Auto A", "Auto B"), "Typed");
}
