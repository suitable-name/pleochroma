//! Native GPU tests of the HDR map's yaw on the event wavefront renderer, which builds its own
//! transport uniform (`accumulate.rs`, `GpuTransportParams::with_hdr_of`).
//!
//! Skips cleanly if no GPU adapter is available, unless `PLEOCHROMA_REQUIRE_GPU=1` is set.

use std::f32::consts::TAU;

use glam::Vec3;

use super::EventWavefrontRenderer;
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, FacetFinish},
    },
    renderer::{
        env_map::{EnvRotation, EnvironmentMap},
        frame_scene::FrameScene,
        gpu::{
            GpuContext,
            z_stats::{Welford, batch_frames_agree, batch_mean_stats},
        },
    },
};

/// Frame edge in pixels.
const SIZE: u32 = 16;

/// Samples per pixel of every frame.
const SPP: u32 = 4;

/// Batches of a frosted frame compared in expectation: 48 batch means per pixel keep the
/// z-test's Student tails close to the normal ones.
const FROSTED_BATCHES: u32 = 48;

/// Samples per pixel of one such batch.
const FROSTED_BATCH_SPP: u32 = 8;

/// The renderer, or `None` (a clean skip) when no adapter is available.
fn renderer_or_skip(test_name: &str) -> Option<EventWavefrontRenderer> {
    match GpuContext::acquire() {
        Ok(ctx) => Some(EventWavefrontRenderer::new(ctx)),
        Err(err) => {
            assert!(
                !std::env::var("PLEOCHROMA_REQUIRE_GPU").is_ok_and(|v| v == "1"),
                "{test_name}: PLEOCHROMA_REQUIRE_GPU=1 was set, but GPU acquisition failed: {err}"
            );
            println!("skipping {test_name}: no GPU adapter ({err})");
            None
        }
    }
}

/// A sky whose radiance changes with the azimuth everywhere, so any wrong turn shows.
fn sky_map() -> EnvironmentMap {
    let (width, height) = (32_usize, 16_usize);
    let texels = (0..width * height)
        .map(|i| {
            let u = (i % width) as f32 / width as f32;
            let v = (i / width) as f32 / height as f32;
            [0.2 + u, 0.5f32.mul_add(v, 0.3), u.mul_add(v, 0.1)]
        })
        .collect();
    EnvironmentMap::from_rgb(width, height, texels).expect("self-consistent texels")
}

/// `map` with every row moved `columns` texels toward larger `u` (wrapping).
fn shifted(map: &EnvironmentMap, columns: usize) -> EnvironmentMap {
    let (width, height) = (map.width(), map.height());
    let texels = (0..height)
        .flat_map(|y| (0..width).map(move |x| (y, x)))
        .map(|(y, x)| map.texels()[y * width + (x + width - columns) % width])
        .collect();
    EnvironmentMap::from_rgb(width, height, texels).expect("self-consistent texels")
}

/// The event wavefront frame of samples `0..SPP` of a diamond under `environment`.
fn frame(
    renderer: &mut EventWavefrontRenderer,
    finishes: &[FacetFinish],
    environment: EnvironmentSource<'_>,
) -> Vec<Vec3> {
    frame_range(renderer, finishes, environment, 0, SPP)
}

/// Per-pixel batch statistics of samples `first..first + FROSTED_BATCHES * FROSTED_BATCH_SPP`,
/// one `accumulate` call per batch.
fn batch_stats(
    renderer: &mut EventWavefrontRenderer,
    finishes: &[FacetFinish],
    environment: EnvironmentSource<'_>,
    first: u32,
) -> Vec<[Welford; 3]> {
    let batches: Vec<Vec<Vec3>> = (0..FROSTED_BATCHES)
        .map(|k| {
            frame_range(
                renderer,
                finishes,
                environment,
                first + k * FROSTED_BATCH_SPP,
                FROSTED_BATCH_SPP,
            )
        })
        .collect();
    batch_mean_stats(&batches, FROSTED_BATCH_SPP)
}

/// The event wavefront frame of samples `first..first + spp` of a diamond under `environment`.
fn frame_range(
    renderer: &mut EventWavefrontRenderer,
    finishes: &[FacetFinish],
    environment: EnvironmentSource<'_>,
    first: u32,
    spp: u32,
) -> Vec<Vec3> {
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let scene = FrameScene {
        camera: &camera,
        width: SIZE,
        height: SIZE,
        planes: &planes,
        facet_finishes: finishes,
        material: &material,
        max_bounces: 8,
        environment,
    };
    let mut accum = vec![Vec3::ZERO; (SIZE * SIZE) as usize];
    pollster::block_on(renderer.accumulate(&scene, first, spp, &mut accum))
        .expect("a small HDR frame renders");
    accum
}

/// How many pixels of `a` differ from `b` by more than `relative` of the larger component.
fn differing(a: &[Vec3], b: &[Vec3], relative: f32) -> usize {
    a.iter()
        .zip(b)
        .filter(|(x, y)| {
            let scale = y.abs().max_element().max(1e-3);
            (**x - **y).abs().max_element() > relative * scale
        })
        .count()
}

/// A map turned by a whole number of texel columns is the map with its columns shifted, on the
/// event wavefront renderer.
///
/// A polished stone reads the map only where its paths escape, so the two frames match pixel
/// for pixel. A frosted stone also draws its exterior NEE directions out of the map's importance
/// distribution, and the shifted map's distribution hands the same random numbers other texels:
/// its two frames match in expectation only, which per-pixel z-tests over disjoint sample ranges
/// check (the turned map's draws and pdfs must turn with it, or the frames part).
#[test]
fn ew_a_whole_column_turn_equals_shifted_columns() {
    let Some(mut renderer) = renderer_or_skip("ew_a_whole_column_turn_equals_shifted_columns")
    else {
        return;
    };
    let map = sky_map();
    let columns = 5;
    let yaw = TAU * columns as f32 / map.width() as f32;
    let shifted_map = shifted(&map, columns);
    let planes = StandardGemCuts::standard_round_brilliant();
    let frosted = vec![FacetFinish::Frosted; planes.len()];
    let turned = EnvironmentSource::hdr(&map).with_hdr_rotation(EnvRotation::from_yaw(yaw));
    let moved = EnvironmentSource::hdr(&shifted_map);
    let unturned = EnvironmentSource::hdr(&map);

    let a = frame(&mut renderer, &[], turned);
    let b = frame(&mut renderer, &[], moved);
    let plain = frame(&mut renderer, &[], unturned);
    let outliers = differing(&a, &b, 2e-3);
    assert!(
        outliers <= a.len() / 20,
        "(polished): {outliers} of {} pixels differ between the turned map and the shifted \
         columns",
        a.len()
    );
    assert!(
        differing(&plain, &b, 1e-2) > a.len() / 10,
        "the fixture must show the turn: the unturned map renders differently"
    );

    let span = FROSTED_BATCHES * FROSTED_BATCH_SPP;
    let a = batch_stats(&mut renderer, &frosted, turned, 0);
    let b = batch_stats(&mut renderer, &frosted, moved, span);
    let plain = batch_stats(&mut renderer, &frosted, unturned, 2 * span);
    let (agree, summary) = batch_frames_agree(&a, &b, SIZE, SIZE);
    assert!(
        agree,
        "(frosted): the turned map and the shifted columns differ beyond noise: {summary}"
    );
    let (alike, summary) = batch_frames_agree(&plain, &b, SIZE, SIZE);
    assert!(
        !alike,
        "(frosted): the fixture must show the turn, yet the unturned map is within noise: \
         {summary}"
    );
}

/// An identity yaw renders the default event wavefront frame bit for bit.
#[test]
fn ew_an_identity_yaw_renders_the_default_frame_bit_for_bit() {
    let Some(mut renderer) = renderer_or_skip("ew_an_identity_yaw_renders_the_default_frame")
    else {
        return;
    };
    let map = sky_map();
    let base = frame(&mut renderer, &[], EnvironmentSource::hdr(&map));
    for yaw in [0.0f32, -0.0, TAU] {
        let source = EnvironmentSource::hdr(&map).with_hdr_rotation(EnvRotation::from_yaw(yaw));
        let same = frame(&mut renderer, &[], source);
        assert!(
            same.iter()
                .zip(&base)
                .all(|(a, b)| a.to_array().map(f32::to_bits) == b.to_array().map(f32::to_bits)),
            "yaw {yaw} changed the frame"
        );
    }
}
