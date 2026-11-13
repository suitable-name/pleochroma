use super::{SHADER_SOURCE, WhittedGpuRenderer, env_cache::uploads_on_this_thread};
use crate::{
    geometry::{StandardGemCuts, tool::ToolPrimitive},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            Camera, EnvironmentSource, LightingModel, LightingPreset, build_plane_soa,
            environment::{
                head_shadow_cosines, sample_studio_environment_with_rig_spec_power_shadow_tray,
            },
        },
        studio_rig::StudioRig,
    },
    renderer::{
        buffers::whitted::GpuWhittedParams,
        env_map::{EnvRotation, EnvironmentMap},
        frame_scene::FrameScene,
        gpu::{
            compute,
            environment_check::{STUDIO_ENV_ABS_FLOOR, STUDIO_ENV_ULP_BUDGET},
            event_wavefront::compare::{FrameDiff, compare_frames},
            test_support::require_gpu,
            ulp::within_tolerance,
        },
        whitted::{
            WhittedPrep,
            fresnel::{fresnel_unpolarised_f32, refract},
            whitted_pixels_interleaved_geom,
        },
    },
};
use glam::Vec3;
use std::sync::Arc;

#[test]
fn whitted_cpu_gpu_frames_are_close_and_two_dispatches_are_identical() {
    let Some(context) = require_gpu() else {
        return;
    };
    let mut renderer = pollster::block_on(WhittedGpuRenderer::from_context_async(context))
        .expect("compile Ray-Traced");
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let env = EnvironmentMap::uniform(8, 4, [0.7, 0.5, 0.3]);
    for material in [
        GemMaterial::diamond(),
        GemMaterial::new_custom("clear", 1.5, 0.0, 0.0, [0.0; 3]),
    ] {
        // The last case lights the map at a scene exposure: the GPU leaf scales it as the CPU
        // leaf does (`wt_hdr_at_exposure`, `hdr_rgb_at_exposure`).
        for (tools, exposure) in [
            (Vec::new(), 1.0),
            (
                vec![ToolPrimitive::ball(Vec3::new(0.0, 0.5, 0.0), 0.2)],
                1.0,
            ),
            (Vec::new(), 2.5),
        ] {
            let scene = FrameScene {
                camera: &camera,
                width: 24,
                height: 16,
                planes: &planes,
                facet_finishes: &[],
                material: &material,
                max_bounces: 64,
                environment: EnvironmentSource::hdr_at(&env, exposure),
            };
            let prep = WhittedPrep::new(
                &scene,
                Fluorescence::none(),
                crate::renderer::engine::RayOptions {
                    aa_taps: 4,
                    fire: true,
                },
            )
            .expect("prep");
            let uploaded = renderer.prepare(&scene, &tools, &prep).expect("upload");
            let gpu = renderer
                .trace_chunk(&uploaded, 0, 24 * 16, 0, 4)
                .expect("GPU frame");
            let again = renderer
                .trace_chunk(&uploaded, 0, 24 * 16, 0, 4)
                .expect("repeat frame");
            assert_eq!(gpu, again);
            let cpu = whitted_pixels_interleaved_geom(
                &scene,
                &tools,
                &build_plane_soa(&planes),
                &prep,
                0,
                1,
                0,
                4,
            );
            let difference = compare_frames(&cpu, &gpu, 4);
            assert!(difference.tol_passed(cpu.len()), "{difference:?}");
        }
    }
}

/// Pixels of the 384-pixel lit-tray frame allowed beyond the tolerance (facet-edge ties).
const LIT_TRAY_OUTLIERS: usize = 6;

/// ULP distance the GPU `exp` may have from `simd::exp_lane` (both results are positive, so
/// the bit patterns order like the values).
const EXP_ULP_BUDGET: u32 = 2;

#[test]
fn whitted_cpu_gpu_frames_agree_with_stone_shadow() {
    use crate::optics::raytracer::environment::tray_shadow::{
        TRAY_SHADOW_BANDS, TrayShadowKey, TrayShadowMap,
    };
    let Some(context) = require_gpu() else {
        return;
    };
    let mut renderer = pollster::block_on(WhittedGpuRenderer::from_context_async(context))
        .expect("compile Ray-Traced");
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let factors: Vec<f32> = (0..16)
        .flat_map(|i| std::iter::repeat_n((i as f32).mul_add(0.04, 0.2), TRAY_SHADOW_BANDS))
        .collect();
    let rgb = (0..16)
        .map(|i| [(i as f32).mul_add(0.04, 0.2), 0.4, 0.7])
        .collect();
    let map = TrayShadowMap::from_parts(-1.0, [0.0, 0.0], 3.0, 4, factors, rgb, TrayShadowKey(7));
    let env = LightingPreset::LightTent
        .studio(1.0, 0.85, 0.95)
        .with_backdrop(0.81);
    let scene = FrameScene {
        camera: &camera,
        width: 24,
        height: 16,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 64,
        environment: env.with_tray_shadow(Some(&map)),
    };
    let options = crate::renderer::engine::RayOptions {
        aa_taps: 4,
        fire: true,
    };
    let prep = WhittedPrep::new(&scene, Fluorescence::none(), options).expect("prep");
    let plane_soa = build_plane_soa(&planes);
    let cpu = whitted_pixels_interleaved_geom(&scene, &[], &plane_soa, &prep, 0, 1, 0, 4);
    let uploaded = renderer.prepare(&scene, &[], &prep).expect("upload");
    let gpu = renderer
        .trace_chunk(&uploaded, 0, 24 * 16, 0, 4)
        .expect("GPU frame");
    let unshadowed = FrameScene {
        environment: env,
        ..scene
    };
    let plain_prep =
        WhittedPrep::new(&unshadowed, Fluorescence::none(), options).expect("plain prep");
    let plain =
        whitted_pixels_interleaved_geom(&unshadowed, &[], &plane_soa, &plain_prep, 0, 1, 0, 4);
    // Purpose of the test: the shadow must reach the frame. It must move more pixels beyond the
    // rounding bound than the rounding tier allows, or a GPU frame without the shadow would pass.
    let reach = compare_frames(&cpu, &plain, 4).rounding_failing_pixels;
    assert!(
        reach > FrameDiff::allowed_rounding_outliers(cpu.len()),
        "the shadow moves only {reach} pixels beyond the rounding bound"
    );
    // The same scene without the shadow on the GPU (the lit-tray test's LightTent frame): its
    // differences are the chain's own ties.
    let plain_uploaded = renderer
        .prepare(&unshadowed, &[], &plain_prep)
        .expect("plain upload");
    let plain_gpu = renderer
        .trace_chunk(&plain_uploaded, 0, 24 * 16, 0, 4)
        .expect("plain GPU frame");
    let plain_difference = compare_frames(&plain, &plain_gpu, 4);
    // The lookup mirrors the CPU operation for operation and is linear in the tray, so only
    // rounding separates the frames. Near a critical angle a few ulp move the Fresnel weights
    // (`FrameDiff::ROUNDING_RELATIVE_BOUND`) and can tip a TIR-or-exit, facet-edge or cutoff
    // decision. The map exposes more of these ties than the uniform tray, because the two
    // leaves of a tie can land on different factors. The rounding tier bounds exactly this. A
    // GPU that drops or misplaces the shadow changes pixels by percent, not ulp, and fails it.
    let difference = compare_frames(&cpu, &gpu, 4);
    assert!(
        difference.rounding_passed(cpu.len()),
        "{difference:?}; without the shadow: {plain_difference:?}"
    );
}

/// A glossy tray under the stone: the GPU frame (floor escapes at the exits and at the primary
/// misses, reflected branches on the 4-slot stack) matches the CPU frame, and the reflection
/// really reaches the frame.
#[test]
fn whitted_cpu_gpu_frames_agree_over_a_glossy_tray() {
    use crate::optics::raytracer::environment::TrayMirror;
    let Some(context) = require_gpu() else {
        return;
    };
    let mut renderer = pollster::block_on(WhittedGpuRenderer::from_context_async(context))
        .expect("compile Ray-Traced");
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let plane_soa = build_plane_soa(&planes);
    let options = crate::renderer::engine::RayOptions {
        aa_taps: 4,
        fire: true,
    };
    let env = LightingPreset::LightTent
        .studio(1.0, 0.85, 0.95)
        .with_backdrop(0.81);
    for reflectance in [0.04_f32, 0.6] {
        let mirror = TrayMirror::new(-1.0, reflectance).expect("a valid mirror");
        let scene = FrameScene {
            camera: &camera,
            width: 24,
            height: 16,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 64,
            environment: env.with_tray_mirror(Some(mirror)),
        };
        let prep = WhittedPrep::new(&scene, Fluorescence::none(), options).expect("prep");
        let cpu = whitted_pixels_interleaved_geom(&scene, &[], &plane_soa, &prep, 0, 1, 0, 4);
        let uploaded = renderer.prepare(&scene, &[], &prep).expect("upload");
        let gpu = renderer
            .trace_chunk(&uploaded, 0, 24 * 16, 0, 4)
            .expect("GPU frame");
        let matte = FrameScene {
            environment: env,
            ..scene
        };
        let matte_prep =
            WhittedPrep::new(&matte, Fluorescence::none(), options).expect("matte prep");
        let plain =
            whitted_pixels_interleaved_geom(&matte, &[], &plane_soa, &matte_prep, 0, 1, 0, 4);
        // Purpose of the test: the reflection reaches the frame. It must move more pixels
        // beyond the rounding bound than the rounding tier lets go, or a GPU frame without it
        // would pass.
        let reach = compare_frames(&cpu, &plain, 4).rounding_failing_pixels;
        assert!(
            reach > FrameDiff::allowed_rounding_outliers(cpu.len()),
            "R0 {reflectance}: the reflection moves only {reach} pixels beyond the rounding bound"
        );
        let difference = compare_frames(&cpu, &gpu, 4);
        assert!(
            difference.rounding_passed(cpu.len()),
            "R0 {reflectance}: {difference:?}"
        );
    }
}

/// A stone with a cavity over a glossy tray: leaves bound for the tray are pushed so the
/// cavity can catch them (`emit_outside`'s tools branch), and the GPU frame still matches the
/// CPU frame.
#[test]
fn whitted_cpu_gpu_frames_agree_over_a_glossy_tray_with_a_cavity() {
    use crate::optics::raytracer::environment::TrayMirror;
    let Some(context) = require_gpu() else {
        return;
    };
    let mut renderer = pollster::block_on(WhittedGpuRenderer::from_context_async(context))
        .expect("compile Ray-Traced");
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let plane_soa = build_plane_soa(&planes);
    let options = crate::renderer::engine::RayOptions {
        aa_taps: 4,
        fire: true,
    };
    // A ball cut into the pavilion main facet at azimuth 0 (that facet is at y = -0.45 for
    // x = 0.5).
    let tools = [ToolPrimitive::ball(Vec3::new(0.5, -0.45, 0.0), 0.2)];
    let env = LightingPreset::LightTent
        .studio(1.0, 0.85, 0.95)
        .with_backdrop(0.81);
    let mirror = TrayMirror::new(-1.0, 0.6).expect("a valid mirror");
    let scene = FrameScene {
        camera: &camera,
        width: 24,
        height: 16,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 64,
        environment: env.with_tray_mirror(Some(mirror)),
    };
    let prep = WhittedPrep::new(&scene, Fluorescence::none(), options).expect("prep");
    let cpu = whitted_pixels_interleaved_geom(&scene, &tools, &plane_soa, &prep, 0, 1, 0, 4);
    let uploaded = renderer.prepare(&scene, &tools, &prep).expect("upload");
    let gpu = renderer
        .trace_chunk(&uploaded, 0, 24 * 16, 0, 4)
        .expect("GPU frame");
    // Purpose of the test: the cavity and the reflection both reach the frame, or a GPU frame
    // without either would pass.
    let uncut = whitted_pixels_interleaved_geom(&scene, &[], &plane_soa, &prep, 0, 1, 0, 4);
    let cavity = compare_frames(&cpu, &uncut, 4).rounding_failing_pixels;
    assert!(
        cavity > FrameDiff::allowed_rounding_outliers(cpu.len()),
        "the cavity moves only {cavity} pixels beyond the rounding bound"
    );
    let matte = FrameScene {
        environment: env,
        ..scene
    };
    let matte_prep = WhittedPrep::new(&matte, Fluorescence::none(), options).expect("matte prep");
    let plain =
        whitted_pixels_interleaved_geom(&matte, &tools, &plane_soa, &matte_prep, 0, 1, 0, 4);
    let reach = compare_frames(&cpu, &plain, 4).rounding_failing_pixels;
    assert!(
        reach > FrameDiff::allowed_rounding_outliers(cpu.len()),
        "the reflection moves only {reach} pixels beyond the rounding bound"
    );
    // The same two effects on the device, device against device (no CPU tie between them): a GPU
    // frame without the cavity or without the reflection fails here, whatever the tie allowance
    // below lets through.
    let uncut_gpu = renderer
        .prepare(&scene, &[], &prep)
        .and_then(|uploaded| renderer.trace_chunk(&uploaded, 0, 24 * 16, 0, 4))
        .expect("uncut GPU frame");
    let plain_gpu = renderer
        .prepare(&matte, &tools, &matte_prep)
        .and_then(|uploaded| renderer.trace_chunk(&uploaded, 0, 24 * 16, 0, 4))
        .expect("matte GPU frame");
    let gpu_cavity = compare_frames(&gpu, &uncut_gpu, 4).rounding_failing_pixels;
    let gpu_reach = compare_frames(&gpu, &plain_gpu, 4).rounding_failing_pixels;
    assert!(
        gpu_cavity > FrameDiff::allowed_rounding_outliers(cpu.len())
            && gpu_reach > FrameDiff::allowed_rounding_outliers(cpu.len()),
        "on the GPU the cavity moves {gpu_cavity} and the reflection {gpu_reach} pixels beyond \
         the rounding bound"
    );
    let difference = compare_frames(&cpu, &gpu, 4);
    assert!(
        difference.non_finite_pixels == 0
            && difference.tol_failing_pixels <= CAVITY_MIRROR_TOL_FAILING
            && difference.rounding_failing_pixels <= CAVITY_MIRROR_TIES,
        "{difference:?}; the cavity moves {cavity} and the reflection {reach} pixels on the CPU"
    );
}

/// Pixels of the 384-pixel cavity-over-mirror frame allowed beyond the rounding bound. The
/// cavity's rim, seen directly in the mirror, is a tie that the CPU and the GPU can settle
/// differently.
///
/// The ball's interval uses the same expressions on both sides (`ball_interval`,
/// `ct_ball_interval`), but a GPU driver fuses the dot products into FMAs and the CPU does not.
/// So a ray grazing the rim can enter on one side and miss on the other. The rounding tier's own
/// `max(4, 2 %)` allowance is 7, far below this count.
const CAVITY_MIRROR_TIES: usize = 16;

/// Pixels of the same frame allowed beyond the golden tolerance. Every path that meets the ball
/// carries rounding, so many pixels sit just outside that tolerance. The rounding tier's own
/// 20 % allowance alone would be 76.
const CAVITY_MIRROR_TOL_FAILING: usize = 128;

/// The stone shadow and the glossy tray together: the paper part of every escape reads the
/// shadow map, and the GPU frame matches the CPU frame.
#[test]
fn whitted_cpu_gpu_frames_agree_with_stone_shadow_and_a_glossy_tray() {
    use crate::optics::raytracer::environment::{
        TrayMirror,
        tray_shadow::{TRAY_SHADOW_BANDS, TrayShadowKey, TrayShadowMap},
    };
    let Some(context) = require_gpu() else {
        return;
    };
    let mut renderer = pollster::block_on(WhittedGpuRenderer::from_context_async(context))
        .expect("compile Ray-Traced");
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let plane_soa = build_plane_soa(&planes);
    let factors: Vec<f32> = (0..16)
        .flat_map(|i| std::iter::repeat_n((i as f32).mul_add(0.04, 0.2), TRAY_SHADOW_BANDS))
        .collect();
    let rgb = (0..16)
        .map(|i| [(i as f32).mul_add(0.04, 0.2), 0.4, 0.7])
        .collect();
    let map = TrayShadowMap::from_parts(-1.0, [0.0, 0.0], 3.0, 4, factors, rgb, TrayShadowKey(7));
    let options = crate::renderer::engine::RayOptions {
        aa_taps: 4,
        fire: true,
    };
    let env = LightingPreset::LightTent
        .studio(1.0, 0.85, 0.95)
        .with_backdrop(0.81);
    let mirror = TrayMirror::new(-1.0, 0.6).expect("a valid mirror");
    let scene = FrameScene {
        camera: &camera,
        width: 24,
        height: 16,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 64,
        environment: env
            .with_tray_shadow(Some(&map))
            .with_tray_mirror(Some(mirror)),
    };
    let prep = WhittedPrep::new(&scene, Fluorescence::none(), options).expect("prep");
    let cpu = whitted_pixels_interleaved_geom(&scene, &[], &plane_soa, &prep, 0, 1, 0, 4);
    let uploaded = renderer.prepare(&scene, &[], &prep).expect("upload");
    let gpu = renderer
        .trace_chunk(&uploaded, 0, 24 * 16, 0, 4)
        .expect("GPU frame");
    // Purpose of the test: both the shadow and the reflection reach the frame.
    let mirror_only = FrameScene {
        environment: env.with_tray_mirror(Some(mirror)),
        ..scene
    };
    let mirror_prep =
        WhittedPrep::new(&mirror_only, Fluorescence::none(), options).expect("mirror prep");
    let unshadowed =
        whitted_pixels_interleaved_geom(&mirror_only, &[], &plane_soa, &mirror_prep, 0, 1, 0, 4);
    let shadow_reach = compare_frames(&cpu, &unshadowed, 4).rounding_failing_pixels;
    assert!(
        shadow_reach > FrameDiff::allowed_rounding_outliers(cpu.len()),
        "the shadow moves only {shadow_reach} pixels beyond the rounding bound"
    );
    let shadow_only = FrameScene {
        environment: env.with_tray_shadow(Some(&map)),
        ..scene
    };
    let shadow_prep =
        WhittedPrep::new(&shadow_only, Fluorescence::none(), options).expect("shadow prep");
    let unmirrored =
        whitted_pixels_interleaved_geom(&shadow_only, &[], &plane_soa, &shadow_prep, 0, 1, 0, 4);
    let mirror_reach = compare_frames(&cpu, &unmirrored, 4).rounding_failing_pixels;
    assert!(
        mirror_reach > FrameDiff::allowed_rounding_outliers(cpu.len()),
        "the reflection moves only {mirror_reach} pixels beyond the rounding bound"
    );
    let difference = compare_frames(&cpu, &gpu, 4);
    assert!(difference.rounding_passed(cpu.len()), "{difference:?}");
}

/// Under a lit backdrop tray the GPU frame (primary misses included, which take the same
/// lookup as every window) matches the CPU frame for a lit model, a Studio rig and ASET.
#[test]
fn whitted_cpu_gpu_frames_agree_over_a_lit_tray() {
    let Some(context) = require_gpu() else {
        return;
    };
    let mut renderer = pollster::block_on(WhittedGpuRenderer::from_context_async(context))
        .expect("compile Ray-Traced");
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let plane_soa = build_plane_soa(&planes);
    let options = crate::renderer::engine::RayOptions {
        aa_taps: 4,
        fire: true,
    };
    // CPU twin frame for a given camera and tray albedo (the GPU frame is only ever taken
    // at `camera` / 0.81).
    let cpu_frame = |preset: LightingPreset, camera: &Camera, tray: f32| {
        let scene = FrameScene {
            camera,
            width: 24,
            height: 16,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 64,
            environment: preset.studio(1.0, 0.85, 0.95).with_backdrop(tray),
        };
        let prep = WhittedPrep::new(&scene, Fluorescence::none(), options).expect("prep");
        whitted_pixels_interleaved_geom(&scene, &[], &plane_soa, &prep, 0, 1, 0, 4)
    };
    for preset in [
        LightingPreset::IsoHemisphere,
        LightingPreset::LightTent,
        LightingPreset::Daylight,
        LightingPreset::Aset,
    ] {
        let scene = FrameScene {
            camera: &camera,
            width: 24,
            height: 16,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 64,
            environment: preset.studio(1.0, 0.85, 0.95).with_backdrop(0.81),
        };
        let prep = WhittedPrep::new(&scene, Fluorescence::none(), options).expect("prep");
        let uploaded = renderer.prepare(&scene, &[], &prep).expect("upload");
        let gpu = renderer
            .trace_chunk(&uploaded, 0, 24 * 16, 0, 4)
            .expect("GPU frame");
        let cpu = cpu_frame(preset, &camera, 0.81);

        // Purpose of the test: the tray must actually reach pixels in this scene.
        let untrayed = cpu_frame(preset, &camera, 0.0);
        assert!(
            cpu.iter().zip(&untrayed).any(|(a, b)| a != b),
            "{preset:?}: the 0.81 tray changes no CPU pixel relative to tray 0"
        );

        // The frame comparator (`compare.rs`) allows max(2, 0.1 %) outlier pixels, which
        // absorbs facet-edge ties. IsoHemisphere and LightTent use the per-pixel rule below
        // with the fixed `LIT_TRAY_OUTLIERS` instead: edge ties scale with the stone's edges,
        // not with the frame's area.
        //
        // ASET joins the Studio branch although it is not a Studio-model rig. The GPU looks up
        // the CPU-built 1024-row ASET elevation table with its own f32 index arithmetic. At
        // ASET's steep smoothstep zone edges a few ulp of d.y shift the value by up to about
        // 1e-3 relative. The mismatch also occurs at tray 0, so the tray is not the cause.
        if preset.model() != LightingModel::Studio && preset != LightingPreset::Aset {
            let difference = compare_frames(&cpu, &gpu, 4);
            assert!(
                difference.non_finite_pixels == 0
                    && difference.tol_failing_pixels <= LIT_TRAY_OUTLIERS,
                "{preset:?}: {difference:?}"
            );
            continue;
        }

        // Studio-model rigs: CPU and GPU light functions agree pointwise within
        // `STUDIO_ENV_ULP_BUDGET` (see `environment_check/studio_env.rs`), about 9.8e-4
        // relative. The per-pixel tolerance is, per channel,
        // `STUDIO_ENV_ULP_BUDGET * f32::EPSILON * max(|cpu|, |gpu|) + 4e-6`, with the outlier
        // allowance `FrameDiff::allowed_tol_failing` (`compare.rs`). The remaining failures are
        // facet-edge ties at the critical angle, which that allowance absorbs.
        let budget = STUDIO_ENV_ULP_BUDGET as f32 * f32::EPSILON;
        let failing = cpu
            .iter()
            .zip(&gpu)
            .filter(|(c, g)| {
                !c.is_finite()
                    || !g.is_finite()
                    || (0..3).any(|k| {
                        let scaled = budget * c[k].abs().max(g[k].abs());
                        (c[k] - g[k]).abs() > scaled + 4e-6
                    })
            })
            .count();
        assert!(
            failing <= FrameDiff::allowed_tol_failing(cpu.len()),
            "{preset:?}: {failing} of {} pixels beyond the {STUDIO_ENV_ULP_BUDGET}-ULP \
             per-channel tolerance (allowed {})",
            cpu.len(),
            FrameDiff::allowed_tol_failing(cpu.len())
        );
        // Edge ties move energy between neighbouring pixels but not out of the frame.
        let mean = |frame: &[Vec3]| {
            frame
                .iter()
                .map(|p| f64::from(p.element_sum()))
                .sum::<f64>()
                / frame.len() as f64
        };
        let (cpu_mean, gpu_mean) = (mean(&cpu), mean(&gpu));
        assert!(
            (cpu_mean - gpu_mean).abs() <= 1e-3 * cpu_mean.abs().max(gpu_mean.abs()),
            "{preset:?}: frame means differ, CPU {cpu_mean} vs GPU {gpu_mean}"
        );
    }
}

#[test]
fn whitted_white_furnace_matches_the_analytic_background() {
    let Some(context) = require_gpu() else {
        return;
    };
    let mut renderer =
        pollster::block_on(WhittedGpuRenderer::from_context_async(context)).expect("pipeline");
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::new_custom("clear", 1.5, 0.0, 0.0, [0.0; 3]);
    let env = EnvironmentMap::uniform(8, 4, [1.0; 3]);
    let scene = FrameScene {
        camera: &camera,
        width: 24,
        height: 16,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 64,
        environment: EnvironmentSource::hdr(&env),
    };
    let prep = WhittedPrep::new(
        &scene,
        Fluorescence::none(),
        crate::renderer::engine::RayOptions {
            aa_taps: 4,
            fire: false,
        },
    )
    .expect("prep");
    let expected = prep.rgb_to_xyz * Vec3::ONE;
    let uploaded = renderer.prepare(&scene, &[], &prep).expect("upload");
    for value in renderer
        .trace_chunk(&uploaded, 0, 24 * 16, 0, 4)
        .expect("frame")
    {
        assert!(
            ((value / 4.0 - expected) / expected).abs().max_element() < 0.002,
            "{value:?}"
        );
    }
}

/// 4096 sphere directions with an exp input, each followed by a refraction case.
fn probe_cases() -> Vec<[f32; 4]> {
    let mut cases = Vec::with_capacity(8192);
    for index in 0..4096 {
        let y = 1.0 - 2.0 * (index as f32 + 0.5) / 4096.0;
        let phi = index as f32 * 2.399_963_1;
        let radius = y.mul_add(-y, 1.0).sqrt();
        let dir = Vec3::new(radius * phi.cos(), y, radius * phi.sin());
        let exp_input = (index as f32).mul_add(97.0 / 4095.0, -87.0);
        cases.push([dir.x, dir.y, dir.z, exp_input]);
        cases.push([0.0, 0.0, 1.0, ((index % 22) as f32).mul_add(0.1, 0.4)]);
    }
    cases
}

/// Compare the GPU probe output for one preset/shadow/tray against the CPU twins.
fn check_probe_results(
    cases: &[[f32; 4]],
    results: &[f32],
    rig: &StudioRig,
    preset: LightingPreset,
    head_shadow_deg: f32,
    tray: f32,
) {
    let shadow = head_shadow_cosines(head_shadow_deg);
    for index in 0..4096 {
        let dir = Vec3::new(
            cases[2 * index][0],
            cases[2 * index][1],
            cases[2 * index][2],
        );
        let x = cases[2 * index][3];
        let eta = cases[2 * index + 1][3];
        let result = &results[index * 8..index * 8 + 8];
        assert!((result[0] - fresnel_unpolarised_f32(x, eta)).abs() <= 1e-6);
        // Within 2 ULP of the CPU twin, not bitwise: WGSL cannot forbid a driver from
        // contracting the polynomial's multiply-adds.
        let (gpu_bits, cpu_bits) = (result[1].to_bits(), crate::simd::exp_lane(x).to_bits());
        assert!(
            gpu_bits.abs_diff(cpu_bits) <= EXP_ULP_BUDGET,
            "exp at {x}: GPU {gpu_bits} vs CPU {cpu_bits}"
        );
        let refracted = refract(dir, Vec3::Z, eta);
        assert_eq!(result[5] != 0.0, refracted.is_some());
        if let Some(expected) = refracted {
            assert!(
                (Vec3::new(result[2], result[3], result[4]) - expected)
                    .abs()
                    .max_element()
                    < 1e-6
            );
        }
        let g_at = |direction: Vec3| {
            sample_studio_environment_with_rig_spec_power_shadow_tray(
                direction,
                preset,
                0.9,
                rig,
                Vec3::Z,
                1.0,
                shadow,
                tray,
            )
        };
        let g = g_at(dir);
        // The studio function chains `pow` lobes (`powi` on the CPU, exp2/log2 on the GPU), which
        // amplify a few ulp of `dot(d, light)` by the exponent. The budget is
        // `STUDIO_ENV_ULP_BUDGET` with its absolute floor.
        assert!(
            within_tolerance(g, result[6], STUDIO_ENV_ULP_BUDGET, STUDIO_ENV_ABS_FLOOR),
            "{preset:?}, shadow {head_shadow_deg}, tray {tray}, direction {dir:?}: {g} vs {}",
            result[6]
        );
    }
}

#[test]
fn whitted_physics_and_every_studio_rig_match_the_cpu_twins() {
    let Some(context) = require_gpu() else {
        return;
    };
    let source = format!(
        "{SHADER_SOURCE}\n{}",
        include_str!("../../shaders/whitted/probe.wgsl")
    );
    let pipeline = compute::create_compute_pipeline(
        &context.device,
        "Whitted physics probes",
        &source,
        "whitted_probe",
    );
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let material = GemMaterial::diamond();
    let cases = probe_cases();
    let input = compute::upload(
        &context.device,
        "Whitted probe inputs",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let rig = StudioRig::new(0.7, 0.8);
    // 16 degrees is the default shadow; 0 (off) and 30 exercise the `wt.shadow` uniform.
    // `tray` is the backdrop tray radiance (`values.z`): 0 = As Lit, then a dim, a white-tray and
    // a Studio-rig-bright tray, so every arm's `tray > 0.0` branch is probed below, in and above
    // the horizon band (the directions cover the whole sphere).
    for (preset, head_shadow_deg, tray) in LightingPreset::ALL
        .into_iter()
        .filter(|preset| !preset.is_uv_lamp() && *preset != LightingPreset::Aset)
        .flat_map(|preset| {
            [0.0_f32, 16.0, 30.0].into_iter().flat_map(move |deg| {
                [0.0_f32, 0.05, 0.81, 3.2]
                    .into_iter()
                    .map(move |tray| (preset, deg, tray))
            })
        })
    {
        let scene = FrameScene {
            camera: &camera,
            width: 1,
            height: 1,
            planes: &[],
            facet_finishes: &[],
            material: &material,
            max_bounces: 12,
            environment: EnvironmentSource::Studio {
                preset,
                exposure: 0.9,
                light_yaw: 0.7,
                light_pitch: 0.8,
                backdrop: tray,
                surface_glare: 1.0,
                tray_shadow: None,
                head_shadow_deg,
                tray_mirror: None,
            },
        };
        let prep = WhittedPrep::new(
            &scene,
            Fluorescence::none(),
            crate::renderer::engine::RayOptions::default(),
        )
        .expect("prep");
        let mut params = GpuWhittedParams::new(&scene, 0, &prep);
        params.range[1] = 4096;
        let uniform = compute::upload(
            &context.device,
            "Whitted probe params",
            &[params],
            wgpu::BufferUsages::UNIFORM,
        );
        let output = compute::zeroed_buffer::<f32>(
            &context.device,
            "Whitted probe output",
            4096 * 8,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let bindings = compute::bind_buffers(
            &context.device,
            "Whitted probe bindings",
            &pipeline,
            &[(1, &uniform), (2, &input), (6, &output)],
        );
        compute::dispatch_and_wait(
            &context.device,
            &context.queue,
            &pipeline,
            &bindings,
            (64, 1, 1),
        );
        let results = compute::readback::<f32>(&context.device, &context.queue, &output, 4096 * 8);
        check_probe_results(&cases, &results, &rig, preset, head_shadow_deg, tray);
    }
}

#[test]
fn large_raster_coordinates_and_public_prep_counts_match_cpu() {
    let Some(context) = require_gpu() else {
        return;
    };
    let mut renderer =
        pollster::block_on(WhittedGpuRenderer::from_context_async(context)).expect("pipeline");
    let camera = Camera::new(0.0, 0.0, 3.0, 42.0);
    let material = GemMaterial::diamond();
    let env = EnvironmentMap::from_rgb(
        4,
        1,
        vec![
            [0.1, 0.2, 0.3],
            [0.8, 0.2, 0.1],
            [0.3, 0.7, 0.4],
            [0.2, 0.4, 0.9],
        ],
    )
    .expect("directional map");
    let scene = FrameScene {
        camera: &camera,
        width: (1 << 24) + 1,
        height: 1,
        planes: &[],
        facet_finishes: &[],
        material: &material,
        max_bounces: 1,
        environment: EnvironmentSource::hdr(&env),
    };
    let soa = build_plane_soa(&[]);
    let mut prep = WhittedPrep::new(
        &scene,
        Fluorescence::none(),
        crate::renderer::engine::RayOptions::default(),
    )
    .expect("prep");
    // Prep is public. GPU upload must follow the CPU's defensive count handling.
    prep.chain_count = u8::MAX;
    for (taps, tap) in [(0, 0), (u8::MAX, 15)] {
        prep.taps = taps;
        let uploaded = renderer.prepare(&scene, &[], &prep).expect("upload");
        let actual = renderer
            .trace_chunk(&uploaded, scene.width - 1, 1, tap, 1)
            .expect("pixel")[0];
        let expected = crate::renderer::whitted::whitted_tap_xyz(
            &scene,
            &[],
            &soa,
            &prep,
            scene.width - 1,
            tap,
            None,
        );
        let difference = compare_frames(&[expected], &[actual], 1);
        assert_eq!(difference.non_finite_pixels, 0, "{difference:?}");
        assert_eq!(difference.tol_failing_pixels, 0, "{difference:?}");
    }
}

const fn hdr_scene<'a>(
    camera: &'a Camera,
    planes: &'a [crate::geometry::GpuFacetPlane],
    material: &'a GemMaterial,
    map: &'a EnvironmentMap,
) -> FrameScene<'a> {
    FrameScene {
        camera,
        width: 8,
        height: 8,
        planes,
        facet_finishes: &[],
        material,
        max_bounces: 4,
        environment: EnvironmentSource::hdr(map),
    }
}

/// An orbit step changes the camera, not the map: the owned map is repacked and uploaded
/// once, a different map replaces the entry, and a borrowed map is never cached.
#[test]
fn an_owned_hdr_map_is_uploaded_once_across_camera_changes() {
    let Some(context) = require_gpu() else {
        return;
    };
    let renderer =
        pollster::block_on(WhittedGpuRenderer::from_context_async(context)).expect("pipeline");
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let options = crate::renderer::engine::RayOptions::default();
    let owned_prep = |camera: &Camera, map: &Arc<EnvironmentMap>| {
        WhittedPrep::new(
            &hdr_scene(camera, &planes, &material, map),
            Fluorescence::none(),
            options,
        )
        .expect("prep")
        .into_owned_with_hdr(Some(Arc::clone(map)))
    };
    let first = Arc::new(EnvironmentMap::uniform(8, 4, [0.7, 0.5, 0.3]));
    let base = uploads_on_this_thread();

    let camera_a = Camera::new(0.2, 0.3, 3.0, 42.0);
    let scene_a = renderer
        .prepare(
            &hdr_scene(&camera_a, &planes, &material, &first),
            &[],
            &owned_prep(&camera_a, &first),
        )
        .expect("first upload");
    assert_eq!(uploads_on_this_thread(), base + 1);
    assert_eq!(renderer.cached_environment_bytes(), 8 * 4 * 16);

    let camera_b = Camera::new(0.9, 0.3, 3.0, 42.0);
    let scene_b = renderer
        .prepare(
            &hdr_scene(&camera_b, &planes, &material, &first),
            &[],
            &owned_prep(&camera_b, &first),
        )
        .expect("orbit step");
    assert_eq!(
        uploads_on_this_thread(),
        base + 1,
        "camera-only change hits"
    );
    // Buffer equality is wgpu object identity: the same GPU buffer, not a copy.
    assert_eq!(scene_a.environment, scene_b.environment);

    let second = Arc::new(EnvironmentMap::uniform(16, 8, [0.1, 0.2, 0.3]));
    let scene_c = renderer
        .prepare(
            &hdr_scene(&camera_a, &planes, &material, &second),
            &[],
            &owned_prep(&camera_a, &second),
        )
        .expect("different map");
    assert_eq!(uploads_on_this_thread(), base + 2);
    assert_ne!(scene_a.environment, scene_c.environment);
    assert_eq!(
        renderer.cached_environment_bytes(),
        16 * 8 * 16,
        "one entry: the old map was released from the cache"
    );
    assert_eq!(Arc::strong_count(&first), 1, "the cache dropped its pin");

    // A borrowed `Hdr(&map)` has no owner to pin, so every prepare uploads again.
    let borrowed = WhittedPrep::new(
        &hdr_scene(&camera_a, &planes, &material, &second),
        Fluorescence::none(),
        options,
    )
    .expect("prep");
    for step in 1..=2 {
        renderer
            .prepare(
                &hdr_scene(&camera_a, &planes, &material, &second),
                &[],
                &borrowed,
            )
            .expect("borrowed upload");
        assert_eq!(uploads_on_this_thread(), base + 2 + step);
    }
}

/// A map whose texels differ in every column, so a turn of the lookup changes the picture.
fn gradient_map(width: u8, height: u8) -> EnvironmentMap {
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height));
    for y in 0..height {
        for x in 0..width {
            let (fx, fy) = (f32::from(x), f32::from(y));
            pixels.push([
                fx.mul_add(0.3, 1.0),
                fy.mul_add(0.2, 0.5),
                fx.mul_add(-0.1, 2.0),
            ]);
        }
    }
    EnvironmentMap::from_rgb(usize::from(width), usize::from(height), pixels).expect("gradient map")
}

/// The CPU and the device frame of the 24 x 16 clear-stone scene lit by `environment`.
fn hdr_cpu_and_gpu_frames(
    renderer: &mut WhittedGpuRenderer,
    environment: EnvironmentSource<'_>,
) -> (Vec<Vec3>, Vec<Vec3>) {
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::new_custom("clear", 1.5, 0.0, 0.0, [0.0; 3]);
    let scene = FrameScene {
        camera: &camera,
        width: 24,
        height: 16,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 64,
        environment,
    };
    let prep = WhittedPrep::new(
        &scene,
        Fluorescence::none(),
        crate::renderer::engine::RayOptions {
            aa_taps: 4,
            fire: true,
        },
    )
    .expect("prep");
    let uploaded = renderer.prepare(&scene, &[], &prep).expect("upload");
    let gpu = renderer
        .trace_chunk(&uploaded, 0, 24 * 16, 0, 4)
        .expect("GPU frame");
    let cpu =
        whitted_pixels_interleaved_geom(&scene, &[], &build_plane_soa(&planes), &prep, 0, 1, 0, 4);
    (cpu, gpu)
}

/// A turned map: the kernel's `hdr_world_to_map` agrees with `EnvRotation::to_map`, so the CPU
/// and device frames stay close, and the turn really changes the picture.
///
/// The kernel's `atan2` and `acos` are not bit-identical to the CPU's. Over a gradient map each
/// escape can land a few 1e-5 off, beyond the golden tolerance but within the rounding bound. A
/// wrong turn moves the texel a direction reads, which changes a pixel by percent, so the
/// rounding bound separates the two cases.
#[test]
fn whitted_cpu_gpu_frames_agree_with_a_turned_hdr_map() {
    let Some(context) = require_gpu() else {
        return;
    };
    let mut renderer = pollster::block_on(WhittedGpuRenderer::from_context_async(context))
        .expect("compile Ray-Traced");
    let env = gradient_map(16, 8);
    let turned = EnvironmentSource::hdr_at(&env, 1.0).with_hdr_rotation(EnvRotation::from_yaw(0.7));
    let (cpu, gpu) = hdr_cpu_and_gpu_frames(&mut renderer, turned);
    let (cpu_unturned, unturned) =
        hdr_cpu_and_gpu_frames(&mut renderer, EnvironmentSource::hdr_at(&env, 1.0));
    let difference = compare_frames(&cpu, &gpu, 4);
    let unturned_difference = compare_frames(&cpu_unturned, &unturned, 4);
    let allowed = FrameDiff::allowed_rounding_outliers(cpu.len());
    assert!(
        difference.non_finite_pixels == 0 && difference.rounding_failing_pixels <= allowed,
        "{difference:?}; the unturned map: {unturned_difference:?}"
    );
    let moved = compare_frames(&cpu, &unturned, 4).rounding_failing_pixels;
    assert!(
        moved > allowed,
        "the turn moves only {moved} pixels beyond the rounding bound"
    );
}

/// An identity rotation is the default: the same bits on the same device, however it is spelled.
#[test]
fn whitted_identity_hdr_rotation_is_bitwise_the_default_frame() {
    let Some(context) = require_gpu() else {
        return;
    };
    let mut renderer = pollster::block_on(WhittedGpuRenderer::from_context_async(context))
        .expect("compile Ray-Traced");
    let env = gradient_map(16, 8);
    let (_, default) = hdr_cpu_and_gpu_frames(&mut renderer, EnvironmentSource::hdr_at(&env, 1.0));
    for rotation in [EnvRotation::IDENTITY, EnvRotation::from_yaw(0.0)] {
        let environment = EnvironmentSource::hdr_at(&env, 1.0).with_hdr_rotation(rotation);
        let (_, frame) = hdr_cpu_and_gpu_frames(&mut renderer, environment);
        assert_eq!(frame, default);
    }
}

/// Turning the map by whole columns (2 pi k / W) equals shifting its columns by k: the device
/// frame of the turned map matches the device frame of the shifted map.
#[test]
fn whitted_gpu_column_shift_equals_a_turned_hdr_map() {
    let Some(context) = require_gpu() else {
        return;
    };
    let mut renderer = pollster::block_on(WhittedGpuRenderer::from_context_async(context))
        .expect("compile Ray-Traced");
    let (width, height, shift) = (16_usize, 8_usize, 3_usize);
    let env = gradient_map(16, 8);
    // Texel `x` of the shifted map is texel `x - shift` of the original (wrapping): the turn
    // puts the map's u = 0 at azimuth `shift` columns on.
    let mut shifted_pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            shifted_pixels.push(env.texels()[y * width + (x + width - shift) % width]);
        }
    }
    let shifted = EnvironmentMap::from_rgb(width, height, shifted_pixels).expect("shifted map");
    // 2 pi * 3 / 16.
    let yaw = std::f32::consts::TAU * 3.0 / 16.0;
    let turned = EnvironmentSource::hdr_at(&env, 1.0).with_hdr_rotation(EnvRotation::from_yaw(yaw));
    let (_, turned_frame) = hdr_cpu_and_gpu_frames(&mut renderer, turned);
    let (_, shifted_frame) =
        hdr_cpu_and_gpu_frames(&mut renderer, EnvironmentSource::hdr_at(&shifted, 1.0));
    let difference = compare_frames(&shifted_frame, &turned_frame, 4);
    assert!(difference.tol_passed(turned_frame.len()), "{difference:?}");
}

/// The uniform carries the map's yaw in `key_dir.xy` for the HDR mode and [1, 0] unturned.
#[test]
fn whitted_hdr_uniform_carries_the_rotation_in_key_dir() {
    let camera = Camera::new(0.6, 0.45, 3.0, 42.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let env = gradient_map(16, 8);
    let rotation = EnvRotation::from_yaw(0.7);
    for (environment, expected) in [
        (EnvironmentSource::hdr_at(&env, 1.0), [1.0, 0.0, 0.0, 0.0]),
        (
            EnvironmentSource::hdr_at(&env, 1.0).with_hdr_rotation(rotation),
            [rotation.cos_sin()[0], rotation.cos_sin()[1], 0.0, 0.0],
        ),
    ] {
        let scene = FrameScene {
            camera: &camera,
            width: 8,
            height: 8,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 4,
            environment,
        };
        let prep = WhittedPrep::new(
            &scene,
            Fluorescence::none(),
            crate::renderer::engine::RayOptions::default(),
        )
        .expect("prep");
        let params = GpuWhittedParams::new(&scene, 0, &prep);
        assert_eq!(params.modes[1], 2);
        assert_eq!(params.key_dir.map(f32::to_bits), expected.map(f32::to_bits));
    }
}
