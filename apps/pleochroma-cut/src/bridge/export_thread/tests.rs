//! `run_export` end-to-end tests: a valid PNG for a tiny scene, HDR-environment-map wiring,
//! pre-set cancellation, and the wide-gamut export's byte-identity and ICC-profile output.

use super::*;
use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_tests_on_request.sh"]
fn run_export_produces_a_valid_png_for_a_tiny_scene() {
    let scene = SceneSnapshot {
        camera_roll: 0.0,
        camera_pan: [0.0, 0.0],
        effects: pleochroma_net::scene::effects::SceneEffects::default(),
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        optical_diagnostics: None,
        yaw: 0.60,
        pitch: 0.45,
        distance: 2.4,
        light_yaw: 0.85,
        light_pitch: 0.95,
        material: GemMaterial::diamond(),
        lighting_preset: LightingPreset::RingLights,
        max_bounces: 4,
        exposure: 1.0,
        backdrop: 0.0,
        active_planes: StandardGemCuts::standard_round_brilliant(),
        tools: Vec::new(),
        fluorescence: std::sync::Arc::default(),
        facet_finishes: Vec::new(),
        env_map: None,
        hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
        stone_pose: pleochroma::render_setup::StonePose::IDENTITY,
        surface_glare: 1.0,
        head_shadow_deg: 16.0,
        stone_shadow: false,
        tray_reflection: 0.0,
        stone_closed: true,
    };
    let params = ExportParams {
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        width: 8,
        height: 8,
        samples_per_pixel: 1,
        max_bounces: 4,
    };
    let cancel = AtomicBool::new(false);

    let dir =
        std::env::temp_dir().join(format!("pleochroma-cut-export-test-{}", std::process::id()));
    let output_path = dir.join("tiny.png");

    let mut progress_calls = 0;
    let outcome = run_export(
        &scene,
        params,
        ColorSpace::Srgb,
        &output_path,
        &RemoteSelection::local_only(),
        LocalComputeTarget::CpuGpu,
        &cancel,
        |_frac| {
            progress_calls += 1;
        },
    );

    match outcome {
        ExportOutcome::Completed(path) => {
            assert_eq!(path, output_path);
            let img = image::open(&path).expect("exported file must be a valid, readable image");
            assert_eq!(img.width(), 8);
            assert_eq!(img.height(), 8);
        }
        other => panic!("expected a completed export, got {other:?}"),
    }
    assert!(
        progress_calls > 0,
        "progress must be reported at least once"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A `SceneSnapshot` with an HDR environment map (`env_map: Some(..)`) must still produce a
/// valid, completed export. `LocalComputeTarget::CpuGpu` exercises the same path as a hybrid
/// export: with a GPU, `GpuBackend::try_accumulate` declines every batch for `HdrMap`, and the
/// samples fall through to the CPU tracer.
#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_tests_on_request.sh"]
fn run_export_with_an_hdr_environment_map_still_produces_a_valid_png() {
    let scene = SceneSnapshot {
        camera_roll: 0.0,
        camera_pan: [0.0, 0.0],
        effects: pleochroma_net::scene::effects::SceneEffects::default(),
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        optical_diagnostics: None,
        yaw: 0.60,
        pitch: 0.45,
        distance: 2.4,
        light_yaw: 0.85,
        light_pitch: 0.95,
        material: GemMaterial::diamond(),
        lighting_preset: LightingPreset::RingLights,
        max_bounces: 4,
        exposure: 1.0,
        backdrop: 0.0,
        active_planes: StandardGemCuts::standard_round_brilliant(),
        tools: Vec::new(),
        fluorescence: std::sync::Arc::default(),
        facet_finishes: Vec::new(),
        env_map: Some(std::sync::Arc::new(
            pleochroma::renderer::env_map::EnvironmentMap::uniform(4, 4, [0.5, 0.5, 0.5]),
        )),
        hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
        stone_pose: pleochroma::render_setup::StonePose::IDENTITY,
        surface_glare: 1.0,
        head_shadow_deg: 16.0,
        stone_shadow: false,
        tray_reflection: 0.0,
        stone_closed: true,
    };
    let params = ExportParams {
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        width: 8,
        height: 8,
        samples_per_pixel: 1,
        max_bounces: 4,
    };
    let cancel = AtomicBool::new(false);
    let dir = std::env::temp_dir().join(format!(
        "pleochroma-cut-export-hdr-test-{}",
        std::process::id()
    ));
    let output_path = dir.join("hdr.png");

    let outcome = run_export(
        &scene,
        params,
        ColorSpace::Srgb,
        &output_path,
        &RemoteSelection::local_only(),
        LocalComputeTarget::CpuGpu,
        &cancel,
        |_frac| {},
    );

    match outcome {
        ExportOutcome::Completed(path) => {
            let img = image::open(&path).expect("exported file must be a valid, readable image");
            assert_eq!(img.width(), 8);
            assert_eq!(img.height(), 8);
        }
        other => panic!("expected a completed export, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn run_export_honors_pre_set_cancellation_and_writes_no_file() {
    let scene = SceneSnapshot {
        camera_roll: 0.0,
        camera_pan: [0.0, 0.0],
        effects: pleochroma_net::scene::effects::SceneEffects::default(),
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        optical_diagnostics: None,
        yaw: 0.60,
        pitch: 0.45,
        distance: 2.4,
        light_yaw: 0.85,
        light_pitch: 0.95,
        material: GemMaterial::diamond(),
        lighting_preset: LightingPreset::RingLights,
        max_bounces: 4,
        exposure: 1.0,
        backdrop: 0.0,
        active_planes: StandardGemCuts::standard_round_brilliant(),
        tools: Vec::new(),
        fluorescence: std::sync::Arc::default(),
        facet_finishes: Vec::new(),
        env_map: None,
        hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
        stone_pose: pleochroma::render_setup::StonePose::IDENTITY,
        surface_glare: 1.0,
        head_shadow_deg: 16.0,
        stone_shadow: false,
        tray_reflection: 0.0,
        stone_closed: true,
    };
    let params = ExportParams {
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        width: 8,
        height: 8,
        samples_per_pixel: 64,
        max_bounces: 4,
    };
    let cancel = AtomicBool::new(true); // already cancelled before starting

    let dir = std::env::temp_dir().join(format!(
        "pleochroma-cut-export-cancel-test-{}",
        std::process::id()
    ));
    let output_path = dir.join("never.png");

    let outcome = run_export(
        &scene,
        params,
        ColorSpace::Srgb,
        &output_path,
        &RemoteSelection::local_only(),
        LocalComputeTarget::CpuGpu,
        &cancel,
        |_frac| {},
    );
    assert!(matches!(outcome, ExportOutcome::Cancelled));
    assert!(!output_path.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

// ---- Wide-gamut export -----------------------------------------------------------

fn tiny_scene() -> SceneSnapshot {
    SceneSnapshot {
        camera_roll: 0.0,
        camera_pan: [0.0, 0.0],
        effects: pleochroma_net::scene::effects::SceneEffects::default(),
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        optical_diagnostics: None,
        yaw: 0.60,
        pitch: 0.45,
        distance: 2.4,
        light_yaw: 0.85,
        light_pitch: 0.95,
        material: GemMaterial::diamond(),
        lighting_preset: LightingPreset::RingLights,
        max_bounces: 4,
        exposure: 1.0,
        backdrop: 0.0,
        active_planes: StandardGemCuts::standard_round_brilliant(),
        tools: Vec::new(),
        fluorescence: std::sync::Arc::default(),
        facet_finishes: Vec::new(),
        env_map: None,
        hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
        stone_pose: pleochroma::render_setup::StonePose::IDENTITY,
        surface_glare: 1.0,
        head_shadow_deg: 16.0,
        stone_shadow: false,
        tray_reflection: 0.0,
        stone_closed: true,
    }
}

/// Two `Srgb` exports of the same scene must produce identical bytes, exercising the `Srgb`
/// branch of `save_png`.
#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_tests_on_request.sh"]
fn srgb_export_is_byte_identical_regardless_of_which_save_png_path_runs() {
    let scene = tiny_scene();
    let params = ExportParams {
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        width: 6,
        height: 5,
        samples_per_pixel: 2,
        max_bounces: 4,
    };

    // Two deterministic renders through `run_export` must agree byte for byte.
    let dir = std::env::temp_dir().join(format!(
        "pleochroma-cut-srgb-identity-test-{}",
        std::process::id()
    ));
    let path_a = dir.join("a.png");
    let path_b = dir.join("b.png");
    let cancel = AtomicBool::new(false);

    let outcome_a = run_export(
        &scene,
        params,
        ColorSpace::Srgb,
        &path_a,
        &RemoteSelection::local_only(),
        LocalComputeTarget::CpuGpu,
        &cancel,
        |_| {},
    );
    let outcome_b = run_export(
        &scene,
        params,
        ColorSpace::Srgb,
        &path_b,
        &RemoteSelection::local_only(),
        LocalComputeTarget::CpuGpu,
        &cancel,
        |_| {},
    );
    assert!(matches!(outcome_a, ExportOutcome::Completed(_)));
    assert!(matches!(outcome_b, ExportOutcome::Completed(_)));

    let bytes_a = std::fs::read(&path_a).unwrap();
    let bytes_b = std::fs::read(&path_b).unwrap();
    assert_eq!(
        bytes_a, bytes_b,
        "two Srgb exports of the identical scene must match exactly"
    );
    // No `iCCP` chunk anywhere in the file -- an sRGB export must stay untagged.
    assert!(
        !contains_bytes(&bytes_a, b"iCCP"),
        "an Srgb export must never carry an ICC profile chunk"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A non-`Srgb` export must carry an embedded ICC profile (`iCCP` PNG chunk). Without one,
/// viewers read a wide-gamut PNG as sRGB. See `bridge::icc_profile`.
#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_tests_on_request.sh"]
fn wide_gamut_export_embeds_an_icc_profile_chunk() {
    for color_space in [ColorSpace::DisplayP3, ColorSpace::Rec2020] {
        let scene = tiny_scene();
        let params = ExportParams {
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            width: 6,
            height: 5,
            samples_per_pixel: 2,
            max_bounces: 4,
        };
        let cancel = AtomicBool::new(false);
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-cut-wide-gamut-test-{color_space:?}-{}",
            std::process::id()
        ));
        let output_path = dir.join("wide.png");

        let outcome = run_export(
            &scene,
            params,
            color_space,
            &output_path,
            &RemoteSelection::local_only(),
            LocalComputeTarget::CpuGpu,
            &cancel,
            |_| {},
        );
        match outcome {
            ExportOutcome::Completed(path) => {
                let bytes = std::fs::read(&path).unwrap();
                assert!(
                    contains_bytes(&bytes, b"iCCP"),
                    "{color_space:?}: expected an iCCP chunk in the exported PNG"
                );
                let img = image::open(&path)
                    .unwrap_or_else(|e| panic!("{color_space:?}: not a readable PNG: {e}"));
                assert_eq!(img.width(), 6);
                assert_eq!(img.height(), 5);
            }
            other => panic!("{color_space:?}: expected a completed export, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Byte substring search -- enough to confirm a chunk signature is present
/// somewhere in a small test PNG without pulling in a PNG-chunk-parsing
/// dependency just for this test.
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
