//! Tests of Copy to Materials: the names, the outputs and the order of the copies.

use super::copy::{VideoRun, copy_names, distinct_path, placeholder_curves, wanted_output};
use crate::{
    bridge::{
        export_thread::{ExportParams, RemoteSelection, SceneSnapshot},
        render_thread::RenderContext,
    },
    gui::render_jobs::convert::{self, StillJobInputs},
};
use pleochroma::color::ColorSpace;
use pleochroma_render_jobs::{
    RenderJobFile,
    job::{DesignInfo, JobColorSpace, JobKind, OverlaySelection, TiltVideoJob},
};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

/// A small still job of the default stone.
fn still_job(label: &str) -> RenderJobFile {
    let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
        .expect("the material resolves");
    let remote = RemoteSelection::local_only();
    let inputs = StillJobInputs {
        scene: &scene,
        params: ExportParams {
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            width: 16,
            height: 16,
            samples_per_pixel: 1,
            max_bounces: 4,
        },
        color_space: ColorSpace::Srgb,
        remote: &remote,
        output: Path::new("D:/Renders/rb57_quartz_4k.png"),
        label: label.to_string(),
        design: DesignInfo::default(),
        preset_label: String::new(),
        adaptive: None,
        path_cache: None,
        beam: None,
    };
    convert::build_still_job(&inputs, 1_791_302_580).expect("the job builds")
}

/// The still job turned into a tilt video in `D:/Renders/<name>`.
fn video_job(name: &str) -> RenderJobFile {
    let mut job = still_job("Spin");
    job.output = format!("D:/Renders/{name}");
    job.kind = JobKind::TiltVideo(TiltVideoJob {
        axis_index: 0,
        more_axes: vec![1],
        start_deg: -60.0,
        end_deg: 60.0,
        step_deg: 1.0,
        total_frames: 121,
        fps: 30,
        samples_per_pixel: 1,
        color_space: JobColorSpace::Srgb,
        overlay: OverlaySelection::default(),
        curves: None,
        more_curves: Vec::new(),
        keep_frames: false,
        video_name: name.to_string(),
        accel: pleochroma::renderer::accel::RenderAccel::default(),
        keep_path_cache: false,
    });
    job
}

fn known() -> Vec<String> {
    ["Quartz", "Sapphire", "Spinel"].map(String::from).to_vec()
}

#[test]
fn the_label_suffix_is_replaced_not_stacked() {
    let source = still_job("Round Brilliant 57 · Quartz");
    let (label, video_name) = copy_names(&source, "Sapphire", &known());
    assert_eq!(label, "Round Brilliant 57 · Sapphire");
    assert_eq!(video_name, None);

    let plain = still_job("Curves job");
    assert_eq!(
        copy_names(&plain, "Sapphire", &known()).0,
        "Curves job · Sapphire"
    );
}

#[test]
fn a_video_copy_gets_its_own_video_name() {
    let source = video_job("spin · Quartz");
    let (_, video_name) = copy_names(&source, "Spinel", &known());
    assert_eq!(video_name.as_deref(), Some("spin · Spinel"));
}

#[test]
fn a_video_copy_gets_its_own_folder_beside_the_source() {
    let source = video_job("spin · Quartz");
    let first = wanted_output(&video_job("spin · Sapphire"), "");
    let second = wanted_output(&video_job("spin · Spinel"), "");
    assert_eq!(first, PathBuf::from("D:/Renders/spin · Sapphire"));
    assert_ne!(first, second);
    assert_ne!(first, PathBuf::from(&source.output));
    assert_eq!(first.parent(), Path::new(&source.output).parent());
}

#[test]
fn a_picture_takes_the_template_name_not_a_numbered_one() {
    let source = still_job("Round Brilliant 57");
    assert_eq!(
        wanted_output(&source, "rb57_sapphire_4k.png"),
        PathBuf::from("D:/Renders/rb57_sapphire_4k.png")
    );
    assert_eq!(
        wanted_output(&source, "rb57_sapphire_4k"),
        PathBuf::from("D:/Renders/rb57_sapphire_4k.png")
    );
}

#[test]
fn n_copies_have_n_distinct_outputs_in_the_order_given() {
    let source = still_job("Round Brilliant 57");
    let materials = ["sapphire", "spinel", "beryl"];
    let mut taken: Vec<PathBuf> = Vec::new();
    for material in materials {
        let wanted = wanted_output(&source, &format!("rb57_{material}_4k.png"));
        let path = distinct_path(wanted.clone(), &taken);
        assert_eq!(path, wanted, "names from the template never need a number");
        taken.push(path);
    }
    assert_eq!(taken.len(), 3);
    assert!(taken[0].ends_with("rb57_sapphire_4k.png"));
    assert!(taken[1].ends_with("rb57_spinel_4k.png"));
    assert!(taken[2].ends_with("rb57_beryl_4k.png"));
}

#[test]
fn a_clash_inside_one_list_is_numbered_before_the_extension() {
    let path = PathBuf::from("D:/Renders/same.png");
    let taken = vec![path.clone()];
    assert_eq!(
        distinct_path(path, &taken),
        PathBuf::from("D:/Renders/same (2).png")
    );
}

#[test]
fn placeholder_curves_have_one_full_set_per_axis() {
    let curves = placeholder_curves(2);
    assert_eq!(curves.len(), 2);
    for curve in &curves {
        assert_eq!(curve.brilliance.len(), 181);
        assert_eq!(curve.windowing.len(), 181);
        assert_eq!(curve.extinction.len(), 181);
    }
}

#[test]
fn each_copy_goes_after_the_one_before() {
    let mut run = VideoRun::new(
        1,
        7,
        video_job("spin"),
        known(),
        vec!["Sapphire".to_string(), "Spinel".to_string()],
    );
    assert_eq!(run.current(), Some("Sapphire"));
    run.record(8);
    assert_eq!(run.current(), Some("Spinel"));
    run.record(9);
    assert_eq!(run.current(), None);
}
