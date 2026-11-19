//! Tests of Load Job View: the exposure and tray inverses, what a view shows, what ends it, and
//! that the one disk pin keeps the cutter's own look out of reach of the controls.
//!
//! Everything here runs without a window: the pure decisions, the render context and the settings
//! rewrite. The window half (the banner, the controls, the second Load) is the UI harness's.

use super::{
    load::{
        BackdropShown, apply_exact_light, apply_to_context, backdrop_shown, exposure_shown,
        look_from_job, subtitle_text,
    },
    session::{CtxLook, EndReason},
};
use crate::{
    bridge::{
        export_thread::{ExportParams, RemoteSelection, SceneSnapshot},
        render_thread::{PlanesOwner, RenderContext},
    },
    gui::{
        render::design_lighting::{ViewPins, job_view_pin},
        render_jobs::convert::{self, StillJobInputs},
    },
    settings::{
        SettingsFile,
        model::{AppSettings, Backdrop, EXPOSURE_RANGE, LightingValues},
    },
};
use pleochroma::{
    color::ColorSpace,
    optics::raytracer::{LightingPreset, metered_exposure},
    render_setup::StonePose,
    renderer::engine::EngineSpec,
};
use pleochroma_render_jobs::{RenderJobFile, job::DesignInfo};
use std::{
    path::Path,
    sync::{Arc, Mutex, atomic::AtomicU64},
};

/// A context with a look of its own: camera, pose, material, size, edges and lighting.
fn lit_context(material: &str, yaw: f32) -> RenderContext {
    RenderContext {
        yaw,
        pitch: 0.7,
        camera_roll: 0.2,
        camera_pan: [0.1, -0.05],
        distance: 5.5,
        stone_pose: StonePose::from_array([30.0, 10.0, 5.0]),
        material_name: material.to_owned(),
        exposure: 1.337,
        max_bounces: 24,
        edge_rounding_radius: 0.01,
        stone_width_mm: 9.0,
        stone_shadow: true,
        tray_reflection: 0.04,
        head_shadow_deg: 12.0,
        ..RenderContext::default()
    }
}

/// The scene `ctx` shows, as a still job freezes it, with the scene itself.
fn job_from(ctx: RenderContext) -> (RenderJobFile, SceneSnapshot) {
    let scene = SceneSnapshot::capture(&Mutex::new(ctx)).expect("the scene captures");
    let remote = RemoteSelection::local_only();
    let job = convert::build_still_job(
        &StillJobInputs {
            scene: &scene,
            params: ExportParams {
                engine: EngineSpec::default(),
                width: 16,
                height: 16,
                samples_per_pixel: 2,
                max_bounces: scene.max_bounces,
            },
            color_space: ColorSpace::Srgb,
            remote: &remote,
            output: Path::new("job.png"),
            label: "Test".to_owned(),
            design: DesignInfo {
                title: "Pentalume".to_owned(),
                material: "Sapphire".to_owned(),
                ..DesignInfo::default()
            },
            preset_label: String::new(),
            adaptive: None,
            path_cache: None,
            beam: None,
        },
        0,
    )
    .expect("the still job builds");
    (job, scene)
}

/// What the lighting session puts in the context for the look of a view.
fn show_lighting(ctx: &mut RenderContext, lighting: &LightingValues) {
    ctx.lighting_preset = LightingPreset::from_label(&lighting.lighting_rig);
    ctx.surface_glare = lighting.surface_glare;
    ctx.backdrop = lighting.backdrop;
    ctx.head_shadow_deg = lighting.head_shadow_deg;
    ctx.stone_shadow = lighting.stone_shadow;
    ctx.tray_reflection = lighting.tray_reflection;
    ctx.hdr_follows_camera = lighting.env_map_follows_camera;
}

/// The exposure inverse gives the compensation back under a tray rig, under a UV lamp (which the
/// meter leaves alone) and for an HDR map (which is unmetered); a value outside the slider is shown
/// clamped and flagged.
#[test]
fn the_exposure_inverse_round_trips_for_a_tray_rig_a_uv_lamp_and_an_hdr_map() {
    for preset in LightingPreset::ALL {
        for (yaw, pitch) in [(0.84f32, 1.2f32), (5.24, 0.45)] {
            let factor = metered_exposure(1.0, preset, yaw, pitch);
            for compensation in [0.2f32, 0.75, 1.0, 1.337, 2.5, 5.0] {
                let frozen = metered_exposure(compensation, preset, yaw, pitch);
                let shown = exposure_shown(frozen, factor);
                assert!(!shown.clamped, "{preset:?} at {compensation}");
                assert!(
                    (shown.value - compensation).abs() <= 1.0e-4,
                    "{preset:?} at {compensation}: {}",
                    shown.value
                );
            }
        }
    }
    let hdr = exposure_shown(1.337, 1.0);
    assert!((hdr.value - 1.337).abs() <= 1.0e-4 && !hdr.clamped);

    let high = exposure_shown(20.0, 1.0);
    assert!(high.clamped);
    assert_eq!(high.value.to_bits(), EXPOSURE_RANGE.1.to_bits());
    let low = exposure_shown(0.01, 1.0);
    assert!(low.clamped);
    assert_eq!(low.value.to_bits(), EXPOSURE_RANGE.0.to_bits());
}

/// A frozen tray radiance gives its tray back; a literal value picks the nearest tray and says so;
/// As Lit is only ever the exact zero.
#[test]
fn the_backdrop_picks_the_nearest_tray_and_never_falls_back_to_as_lit() {
    for preset in LightingPreset::ALL {
        let (yaw, pitch) = (0.84f32, 1.2f32);
        for tray in [Backdrop::Grey, Backdrop::White, Backdrop::Black] {
            let level = tray.resolve(preset, yaw, pitch);
            if level <= 0.0 {
                continue;
            }
            assert_eq!(
                backdrop_shown(level, preset, yaw, pitch),
                BackdropShown {
                    choice: tray,
                    exact: true
                },
                "{preset:?} {tray:?}"
            );
        }
        let white = Backdrop::White.resolve(preset, yaw, pitch);
        if white > 0.0 {
            let literal = backdrop_shown(white * 0.5, preset, yaw, pitch);
            assert_ne!(literal.choice, Backdrop::AsLit, "{preset:?}");
            assert!(!literal.exact, "{preset:?}");
            // Far below every tray: still a tray, not As Lit.
            assert_ne!(
                backdrop_shown(white * 1.0e-6, preset, yaw, pitch).choice,
                Backdrop::AsLit
            );
        }
    }
    let preset = LightingPreset::default();
    assert_eq!(
        backdrop_shown(0.0, preset, 0.8, 1.2),
        BackdropShown {
            choice: Backdrop::AsLit,
            exact: true
        }
    );
    let broken = backdrop_shown(f32::NAN, preset, 0.8, 1.2);
    assert_eq!(broken.choice, Backdrop::White);
    assert!(!broken.exact);
}

/// The banner's subtitle names the design and the material, and a video what it shows.
#[test]
fn the_subtitle_names_the_design_the_material_and_a_videos_pose() {
    assert_eq!(
        subtitle_text("Pentalume", "Sapphire", false),
        "Pentalume \u{b7} Sapphire"
    );
    assert_eq!(
        subtitle_text("Pentalume", "Sapphire", true),
        "Pentalume \u{b7} Sapphire \u{b7} tilt video, shown face up"
    );
    assert_eq!(subtitle_text("", "Sapphire", false), "Sapphire");
    assert_eq!(subtitle_text("  ", "", true), "tilt video, shown face up");
    assert_eq!(subtitle_text("", "", false), "");
}

/// A view shows what the capture froze: showing a job on a fresh context and capturing again finds
/// the same scene, so Update from the unchanged view has nothing to write.
#[test]
fn a_view_shows_the_frozen_scene_and_a_new_capture_finds_it_unchanged() {
    let (job, frozen) = job_from(lit_context("Sapphire", 1.1));
    let look = look_from_job(&job, 7);
    assert_eq!(look.title, "Showing the view of job 7");
    assert_eq!(look.subtitle, "Pentalume \u{b7} Sapphire");
    assert!(look.notes.is_empty(), "{:?}", look.notes);

    let mut shown = lit_context("Ruby", 2.2);
    shown.stone_width_mm = 0.0;
    shown.edge_rounding_radius = 0.0;
    assert!(apply_to_context(&mut shown, 7, &job, &look));
    show_lighting(&mut shown, &look.lighting);
    let owner = apply_exact_light(&mut shown, &look);
    assert_eq!(owner, PlanesOwner::Job { job_id: 7 });
    assert_eq!(shown.planes_owner, owner);

    let again = SceneSnapshot::capture(&Mutex::new(shown)).expect("the view captures");
    assert_eq!(again.yaw.to_bits(), frozen.yaw.to_bits());
    assert_eq!(again.pitch.to_bits(), frozen.pitch.to_bits());
    assert_eq!(again.camera_roll.to_bits(), frozen.camera_roll.to_bits());
    assert_eq!(again.camera_pan, frozen.camera_pan);
    assert_eq!(again.distance.to_bits(), frozen.distance.to_bits());
    assert_eq!(again.stone_pose.to_array(), frozen.stone_pose.to_array());
    assert_eq!(again.max_bounces, frozen.max_bounces);
    assert_eq!(again.light_yaw.to_bits(), frozen.light_yaw.to_bits());
    assert_eq!(again.light_pitch.to_bits(), frozen.light_pitch.to_bits());
    assert_eq!(again.exposure.to_bits(), frozen.exposure.to_bits());
    assert_eq!(again.backdrop.to_bits(), frozen.backdrop.to_bits());
    assert_eq!(
        again.head_shadow_deg.to_bits(),
        frozen.head_shadow_deg.to_bits()
    );
    assert_eq!(again.stone_shadow, frozen.stone_shadow);
    assert_eq!(
        again.tray_reflection.to_bits(),
        frozen.tray_reflection.to_bits()
    );
    assert_eq!(again.active_planes, frozen.active_planes);
    assert_eq!(again.tools, frozen.tools);
    // The material is the job's own, with the stone size read back from its absorption scale.
    let (mut got, mut want) = (again.material, frozen.material);
    let scale_gap = (got.absorption_path_scale - want.absorption_path_scale).abs();
    assert!(
        scale_gap <= want.absorption_path_scale * 1.0e-3,
        "scale gap {scale_gap}"
    );
    got.absorption_path_scale = want.absorption_path_scale;
    want.name.clone_from(&got.name);
    assert_eq!(got, want);
}

/// A frozen exposure outside the slider's range is shown clamped, with a note in the banner.
#[test]
fn an_exposure_outside_the_slider_is_clamped_with_a_note() {
    let (mut job, _) = job_from(lit_context("Sapphire", 1.1));
    job.scene.exposure = 60.0;
    let look = look_from_job(&job, 3);
    assert_eq!(look.exposure.to_bits(), EXPOSURE_RANGE.1.to_bits());
    assert_eq!(look.notes.len(), 1, "{:?}", look.notes);
    assert!(
        look.notes[0].contains("Exposure slider"),
        "{}",
        look.notes[0]
    );
}

/// A tray radiance that is none of the trays shows the nearest tray and says so.
#[test]
fn a_literal_tray_radiance_picks_the_nearest_tray_with_a_note() {
    let (mut job, _) = job_from(lit_context("Sapphire", 1.1));
    job.scene.backdrop *= 0.55;
    let look = look_from_job(&job, 3);
    assert_ne!(look.lighting.backdrop, Backdrop::AsLit);
    assert_eq!(look.notes.len(), 1, "{:?}", look.notes);
    assert!(look.notes[0].contains("tray"), "{}", look.notes[0]);
}

/// Ending a view puts back the stone, the material, the camera and the render settings the cutter
/// had, also when the editor owned the stone, and the editor's own material comes back with it.
#[test]
fn ending_a_view_puts_the_cutters_stone_material_and_camera_back() {
    let (job, _) = job_from(lit_context("Ruby", 2.2));
    let mut ctx = lit_context("Sapphire", 1.1);
    ctx.planes_owner = PlanesOwner::Editor { generation: 3 };
    ctx.editor_generation = Some(Arc::new(AtomicU64::new(3)));
    let planes = Arc::clone(&ctx.active_planes);
    let had = CtxLook::capture(&mut ctx);

    let look = look_from_job(&job, 9);
    assert!(apply_to_context(&mut ctx, 9, &job, &look));
    // A late background solve of the editor's design must not replace the job's stone.
    assert!(ctx.library_took_over_since(3));
    assert_eq!(ctx.planes_owner, PlanesOwner::Job { job_id: 9 });
    assert_eq!(ctx.material_name, "Ruby");
    assert!(ctx.material_override.is_some());
    assert!(ctx.job_design.is_some());

    had.restore(&mut ctx);
    assert_eq!(ctx.planes_owner, PlanesOwner::Editor { generation: 3 });
    assert!(Arc::ptr_eq(&ctx.active_planes, &planes));
    assert_eq!(ctx.material_name, "Sapphire");
    assert!(ctx.material_override.is_none());
    assert!(ctx.editor_material_stash.is_none());
    assert!(ctx.job_design.is_none());
    assert!(!ctx.library_took_over_since(3));
    assert_eq!(ctx.yaw.to_bits(), 1.1f32.to_bits());
    assert_eq!(ctx.distance.to_bits(), 5.5f32.to_bits());
    assert_eq!(ctx.stone_pose.to_array(), [30.0, 10.0, 5.0]);
    assert_eq!(ctx.max_bounces, 24);
    assert_eq!(ctx.stone_width_mm.to_bits(), 9.0f32.to_bits());
    assert_eq!(ctx.edge_rounding_radius.to_bits(), 0.01f32.to_bits());
    assert!(ctx.dirty);
}

/// A second job's view replaces the first on the same context, and one restore still brings back
/// the look the cutter had before the first.
#[test]
fn a_second_view_replaces_the_first_and_one_restore_brings_back_the_cutters_look() {
    let (first, _) = job_from(lit_context("Ruby", 2.2));
    let (second, _) = job_from(lit_context("Emerald", 0.4));
    let mut ctx = lit_context("Sapphire", 1.1);
    let had = CtxLook::capture(&mut ctx);
    for (id, job) in [(1, &first), (2, &second)] {
        let look = look_from_job(job, id);
        assert!(apply_to_context(&mut ctx, id, job, &look));
    }
    assert_eq!(ctx.planes_owner, PlanesOwner::Job { job_id: 2 });
    assert_eq!(ctx.material_name, "Emerald");
    had.restore(&mut ctx);
    assert_eq!(ctx.material_name, "Sapphire");
    assert_eq!(ctx.yaw.to_bits(), 1.1f32.to_bits());
}

/// The view's body color must not tint the job's own material, and it comes back after the view.
#[test]
fn the_cutters_view_color_stays_off_the_jobs_stone_and_returns() {
    use crate::bridge::render_thread::ViewColor;
    let (job, _) = job_from(lit_context("Ruby", 2.2));
    let mut ctx = lit_context("Sapphire", 1.1);
    ctx.view_body_color = Some(ViewColor::triple_only([0.4, 0.2, 0.1]));
    let had = CtxLook::capture(&mut ctx);
    let look = look_from_job(&job, 4);
    assert!(apply_to_context(&mut ctx, 4, &job, &look));
    assert!(ctx.view_body_color.is_none());
    had.restore(&mut ctx);
    assert_eq!(
        ctx.view_body_color,
        Some(ViewColor::triple_only([0.4, 0.2, 0.1]))
    );
}

/// The one disk pin writes the normal lighting and every pinned field over whatever the controls
/// wrote during the view: a camera field and the material, not only the lighting.
#[test]
fn the_one_disk_pin_keeps_the_lighting_the_camera_and_the_material_out_of_the_file() {
    let app = AppSettings {
        camera_yaw: 0.4,
        camera_pitch: 0.9,
        camera_roll: 0.1,
        camera_pan: [0.2, 0.1],
        camera_distance: 3.3,
        stone_pose: [10.0, 20.0, 30.0],
        selected_material: "Ruby".to_owned(),
        max_bounces: 24,
        live_view_mode: 1,
        exposure: 1.5,
        light_yaw_deg: 100.0,
        ..AppSettings::default()
    };
    let normal = LightingValues::from_app_settings(&app);
    let pin = job_view_pin(&normal, &ViewPins::of(&app));

    // What the controls and the handlers wrote while the job's view showed.
    let mut file = SettingsFile {
        settings: AppSettings {
            camera_yaw: 2.0,
            camera_pitch: 0.2,
            camera_roll: 0.0,
            camera_pan: [0.0, 0.0],
            camera_distance: 9.0,
            stone_pose: [0.0, 0.0, 0.0],
            selected_material: "Diamond".to_owned(),
            max_bounces: 128,
            live_view_mode: 2,
            girdle_frosted: true,
            stone_width_mm: 11.0,
            edge_rounding_radius: 0.02,
            exposure: 3.0,
            light_yaw_deg: 10.0,
            ..app.clone()
        },
        ..SettingsFile::default()
    };
    pin(&mut file);
    let kept = &file.settings;
    assert_eq!(kept.camera_yaw.to_bits(), 0.4f32.to_bits());
    assert_eq!(kept.camera_distance.to_bits(), 3.3f32.to_bits());
    assert_eq!(kept.camera_pan, [0.2, 0.1]);
    assert_eq!(kept.stone_pose, [10.0, 20.0, 30.0]);
    assert_eq!(kept.selected_material, "Ruby");
    assert_eq!(kept.max_bounces, 24);
    assert_eq!(kept.live_view_mode, 1);
    assert!(!kept.girdle_frosted);
    assert_eq!(kept.stone_width_mm.to_bits(), app.stone_width_mm.to_bits());
    assert_eq!(
        kept.edge_rounding_radius.to_bits(),
        app.edge_rounding_radius.to_bits()
    );
    assert_eq!(kept.exposure.to_bits(), 1.5f32.to_bits());
    assert_eq!(kept.light_yaw_deg.to_bits(), 100.0f32.to_bits());
}

/// The pins keep the settings of the first Load: a pin made later from other settings is another
/// pin, and the first one still writes the first values.
#[test]
fn a_pin_keeps_the_settings_it_was_made_from() {
    let first = AppSettings {
        camera_yaw: 0.4,
        ..AppSettings::default()
    };
    let pins = ViewPins::of(&first);
    let later = AppSettings {
        camera_yaw: 3.0,
        ..AppSettings::default()
    };
    let mut written = later.clone();
    pins.write_into(&mut written);
    assert_eq!(written.camera_yaw.to_bits(), 0.4f32.to_bits());
}

/// A design's opening and a chosen lighting show no saved lighting afterwards (the opening shows
/// its own, the choice is the new normal look); every other ending does.
#[test]
fn the_endings_that_show_the_designs_saved_lighting_again() {
    for reason in [
        EndReason::BackToMyView,
        EndReason::LibrarySelected,
        EndReason::EditorEdit,
        EndReason::JobDeleted,
    ] {
        assert!(reason.shows_design_lighting_again(), "{reason:?}");
    }
    for reason in [EndReason::DesignOpened, EndReason::LightingChosen] {
        assert!(!reason.shows_design_lighting_again(), "{reason:?}");
    }
}
