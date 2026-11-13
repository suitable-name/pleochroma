//! Tests of leaf retention and instant re-light (beam guide §14.10, §16.3).
//!
//! The rows: `retain` counts honestly and refuses past the cap; a re-light equals a fresh beam
//! render under the new lighting, bit for bit on the CPU engine, for a new Studio pose, a Studio
//! to HDR-map change (leaf refinement through the stored prefixes), a glare change and a
//! head-shadow change; a cache that holds only some batches re-lights to the same image (the other
//! tiles are traced fresh); the Ray-Traced view's re-light is deterministic, equals beams plus
//! residual on fully covered pixels within a loose bound and the Whitted image within a looser one
//! (both printed); a raised cancel token gives `None`.
//!
//! "A fresh render" is `render_beam_hybrid_surface` with the short residual the cache runs: one
//! pass of `residual_min_spp` samples per pixel (`min_spp == max_spp == pass_spp`), so the image
//! does not depend on how the tiles are batched. The tolerances below are named constants; the
//! numbers a run produces are printed with `eprintln!`.

use std::sync::atomic::AtomicBool;

use glam::Vec3;

use super::{
    BeamProgress, BeamRun, BeamSettings,
    engines::EngineSet,
    frame::BeamFrame,
    leaf::ReflectionKit,
    leaf_cache::{LeafCache, RetainedBatch, retained_bytes},
    lighting::tau_omega,
    primary::TileRect,
    retrace::{RetraceKit, refine_leaf},
    stats::BeamStats,
    tests_support::{BeamRig, MAX_BOUNCES, diamond, glass, show_beam_log},
};
use crate::{
    optics::{
        fluorescence::Fluorescence,
        raytracer::{EnvironmentSource, LightingPreset, surface::SurfaceScene},
    },
    renderer::{
        adaptive::{AdaptiveSettings, NoiseMetric},
        cpu_frame::render_beam_hybrid_surface,
        engine::RayOptions,
        env_map::EnvironmentMap,
        frame_scene::FrameScene,
        whitted::{WhittedPrep, whitted_pixels_interleaved_geom},
    },
};

/// The side of the square test frame in pixels.
///
/// Every property tested here is exact at any size, and 16 keeps a render at a quarter of the
/// cost of 32.
const FRAME: u32 = 16;

/// The adaptive tile and the beam tile in pixels (4 tiles).
const TILE: u32 = 8;

/// One tile per batch: four batches of the 4-tile frame.
const BATCH_TILES: u32 = 1;

/// The residual samples per pixel of every render (one pass).
const SPP: u32 = 16;

/// The width of the HDR map in texels: `τ_ω = π / (20 · width)` is about 1.9e-5, a fifth of the
/// Studio value 1e-4, so the leaves of a Studio render are stale under it.
const HDR_WIDTH: usize = 8192;

/// The height of the HDR map in texels.
const HDR_HEIGHT: usize = 16;

/// A pixel with at least this share covered counts as fully covered.
const FULLY_COVERED: f64 = 1.0 - 1.0e-6;

/// The largest mean relative difference between the Ray-Traced re-light and beams plus residual on
/// fully covered pixels (the residual adds the dropped share and its noise).
const COVERED_TOLERANCE: f64 = 0.15;

/// The Ray-Traced image's mean may differ from the Whitted image's by at most this factor either
/// way (a different model, guide §14.10).
const WHITTED_FACTOR: f64 = 4.0;

/// The settings of every render: the short residual and four tiles per batch.
fn beam_settings() -> BeamSettings {
    BeamSettings {
        residual_min_spp: SPP,
        batch_tiles: BATCH_TILES,
        ..BeamSettings::default()
    }
}

/// The adaptive settings of the fresh renders: the single pass the cache runs.
const fn adaptive_settings() -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise: 1.0e-6,
        metric: NoiseMetric::AbsoluteDeltaE,
        min_spp: SPP,
        max_spp: SPP,
        pass_spp: SPP,
        tile: TILE,
    }
}

/// The frame of `rig` lit by `environment`.
fn scene_with<'a>(rig: &'a BeamRig, environment: EnvironmentSource<'a>) -> FrameScene<'a> {
    FrameScene {
        environment,
        ..rig.scene(FRAME, MAX_BOUNCES)
    }
}

/// The rig of the tests: the standard brilliant in glass.
fn rig() -> BeamRig {
    BeamRig::brilliant(glass())
}

/// A Studio environment: the pose, the exposure and the preset.
fn studio(preset: LightingPreset, yaw: f32, pitch: f32) -> EnvironmentSource<'static> {
    preset.studio(1.0, yaw, pitch)
}

/// A texel-sharp HDR map: a gradient with narrow bright bands, `HDR_WIDTH` wide.
fn hdr_map() -> EnvironmentMap {
    let texels = (0..HDR_WIDTH * HDR_HEIGHT)
        .map(|i| {
            let x = i % HDR_WIDTH;
            let ramp = f32::from(u16::try_from(x % 1024).unwrap_or(0)) / 1024.0;
            let band = if (x / 48) % 7 == 3 { 5.0 } else { 0.0 };
            let value = 0.6_f32.mul_add(ramp, 0.2) + band;
            [value, 0.9 * value, 0.7 * value]
        })
        .collect();
    EnvironmentMap::from_rgb(HDR_WIDTH, HDR_HEIGHT, texels).expect("a consistent map")
}

/// A fresh beam render of `scene` on the pipeline's own CPU engine; its finished batches go to
/// `cache` when one is given. Returns the final pixels.
fn fresh(rig: &BeamRig, scene: &FrameScene<'_>, cache: Option<&mut LeafCache>) -> Vec<Vec3> {
    let beam = beam_settings();
    let adaptive = adaptive_settings();
    let engines = EngineSet::new(Vec::new());
    let cancel = AtomicBool::new(false);
    let mut progress = |_: BeamProgress| {};
    let mut sink = cache;
    let mut keep = |batch: RetainedBatch| {
        if let Some(cache) = sink.as_deref_mut() {
            cache.retain(batch);
        }
    };
    let (image, _) = render_beam_hybrid_surface(
        scene,
        (&[], Fluorescence::none(), SurfaceScene::IDEAL),
        &rig.soa,
        &beam,
        &adaptive,
        &engines,
        (0, 1.0),
        &cancel,
        BeamRun {
            tiles: None,
            progress: &mut progress,
            on_batch: None,
            retain: Some(&mut keep),
            preloaded: None,
        },
    )
    .expect("an eligible stone");
    image.pixels
}

/// The batches of a fresh render, in order.
fn batches_of(rig: &BeamRig, scene: &FrameScene<'_>) -> Vec<RetainedBatch> {
    let mut all = Vec::new();
    let beam = beam_settings();
    let adaptive = adaptive_settings();
    let engines = EngineSet::new(Vec::new());
    let cancel = AtomicBool::new(false);
    let mut progress = |_: BeamProgress| {};
    let mut keep = |batch: RetainedBatch| all.push(batch);
    render_beam_hybrid_surface(
        scene,
        (&[], Fluorescence::none(), SurfaceScene::IDEAL),
        &rig.soa,
        &beam,
        &adaptive,
        &engines,
        (0, 1.0),
        &cancel,
        BeamRun {
            tiles: None,
            progress: &mut progress,
            on_batch: None,
            retain: Some(&mut keep),
            preloaded: None,
        },
    )
    .expect("an eligible stone");
    all
}

/// The bit patterns of a vector image.
fn bits(values: &[Vec3]) -> Vec<[u32; 3]> {
    values
        .iter()
        .map(|v| v.to_array().map(f32::to_bits))
        .collect()
}

/// Asserts that two images are bit-identical, printing how they differ when they are not.
fn assert_same_image(label: &str, relit: &[Vec3], reference: &[Vec3]) {
    assert_eq!(relit.len(), reference.len(), "{label}: image sizes");
    let different = bits(relit)
        .iter()
        .zip(bits(reference))
        .filter(|(a, b)| **a != *b)
        .count();
    let worst = relit
        .iter()
        .zip(reference)
        .map(|(a, b)| (*a - *b).abs().max_element())
        .fold(0.0_f32, f32::max);
    eprintln!(
        "{label}: {different} of {} pixels differ, largest difference {worst:e}",
        relit.len()
    );
    assert_eq!(bits(relit), bits(reference), "{label}: not bit-identical");
}

/// Renders `from` into a cache, re-lights it to `to` and compares with a fresh render of `to`.
/// Returns the leaf counts before and after the re-light.
fn check_relight(
    label: &str,
    rig: &BeamRig,
    from: EnvironmentSource<'_>,
    to: EnvironmentSource<'_>,
) -> (usize, usize) {
    let scene_from = scene_with(rig, from);
    let scene_to = scene_with(rig, to);
    let mut cache = LeafCache::new(u64::MAX);
    let _ = fresh(rig, &scene_from, Some(&mut cache));
    assert!(!cache.is_empty(), "{label}: the render retained nothing");
    let before = cache.leaf_count();
    let engines = EngineSet::new(Vec::new());
    let cancel = AtomicBool::new(false);
    let relit = cache
        .relight(&scene_to, &beam_settings(), &engines, &cancel)
        .expect("retained leaves");
    let after = cache.leaf_count();
    eprintln!("{label}: {before} leaves before, {after} after the re-light");
    let reference = fresh(rig, &scene_to, None);
    assert_same_image(label, &relit, &reference);
    (before, after)
}

// ---- retention ----

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn retain_counts_the_batch_honestly_and_refuses_past_the_cap() {
    let rig = rig();
    let scene = scene_with(&rig, studio(LightingPreset::RingLights, 0.3, 0.9));
    let batches = batches_of(&rig, &scene);
    assert_eq!(batches.len(), 4, "four batches of one tile");
    let sizes: Vec<u64> = batches.iter().map(retained_bytes).collect();
    let leaf_bytes: usize = batches
        .iter()
        .flat_map(|b| &b.leaves)
        .flat_map(|(_, leaves)| leaves)
        .map(|leaf| leaf.dirs.len() * size_of::<Vec3>() + leaf.weights.len() * size_of::<f32>())
        .sum();
    let total: u64 = sizes.iter().sum();
    eprintln!("retained sizes {sizes:?}, total {total}, leaf vectors alone {leaf_bytes}");
    assert!(total >= u64::try_from(leaf_bytes).unwrap_or(u64::MAX));

    let mut all = LeafCache::new(u64::MAX);
    for batch in batches.clone() {
        assert!(all.retain(batch));
    }
    assert_eq!(all.bytes(), total);
    assert_eq!(all.batch_count(), batches.len());

    // A cap one byte short of the total keeps every batch but the last.
    let mut short = LeafCache::new(total - 1);
    let kept: Vec<bool> = batches.iter().map(|b| short.retain(b.clone())).collect();
    assert_eq!(kept, vec![true, true, true, false]);
    assert!(short.bytes() < total);
    assert_eq!(short.batch_count(), 3);

    let mut none = LeafCache::new(0);
    assert!(!none.retain(batches[0].clone()));
    assert!(none.is_empty());
    assert_eq!(none.bytes(), 0);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn an_empty_cache_has_nothing_to_relight_but_the_ray_traced_call_builds() {
    // The build traces every tile at the default tree: the beam log shows its progress.
    show_beam_log();
    let rig = rig();
    let scene = scene_with(&rig, studio(LightingPreset::RingLights, 0.3, 0.9));
    let mut cache = LeafCache::new(u64::MAX);
    let engines = EngineSet::new(Vec::new());
    let cancel = AtomicBool::new(false);
    assert_eq!(
        cache.relight(&scene, &beam_settings(), &engines, &cancel),
        None
    );
    assert!(cache.is_empty());
    // The Ray-Traced view's first call of a pose is its build: it traces every tile and retains
    // the leaves, so the next lighting change re-lights them.
    let background = vec![Vec3::ZERO; (FRAME * FRAME) as usize];
    let built = cache
        .relight_ray_traced(&scene, &beam_settings(), &background, &cancel)
        .expect("the first call builds");
    assert_eq!(built.len(), background.len());
    assert!(built.iter().all(|pixel| pixel.is_finite()));
    assert!(!cache.is_empty(), "the build retains its leaves");
}

// ---- re-light = a fresh render ----

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_new_studio_pose_relights_to_the_fresh_render() {
    let rig = rig();
    let (before, after) = check_relight(
        "studio pose",
        &rig,
        studio(LightingPreset::RingLights, 0.3, 0.9),
        studio(LightingPreset::RingLights, 1.7, 0.5),
    );
    // The same `τ_ω`: nothing is refined.
    assert_eq!(before, after);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_new_studio_preset_relights_to_the_fresh_render() {
    let rig = rig();
    let (before, after) = check_relight(
        "studio preset",
        &rig,
        studio(LightingPreset::RingLights, 0.3, 0.9),
        studio(LightingPreset::Incandescent, 0.3, 0.9),
    );
    assert_eq!(before, after);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn studio_to_an_hdr_map_refines_the_leaves_and_relights_to_the_fresh_render() {
    let rig = rig();
    let map = hdr_map();
    let hdr = EnvironmentSource::hdr(&map);
    let studio_env = studio(LightingPreset::RingLights, 0.3, 0.9);
    let (studio_tau, hdr_tau) = (tau_omega(&studio_env), tau_omega(&hdr));
    eprintln!("tau_omega: studio {studio_tau:e}, hdr {hdr_tau:e}");
    assert!(
        hdr_tau < studio_tau,
        "the HDR map must need finer leaves than the Studio render built"
    );
    let (before, after) = check_relight("studio to hdr", &rig, studio_env, hdr);
    eprintln!(
        "leaf refinement changed the count by {}",
        after.abs_diff(before)
    );
}

/// A dispersive stone: the retained leaves of a diamond keep their pre-clip polygons, so a
/// refinement bisects the unclipped polygon and re-clips the pieces like the tree, and the
/// re-light equals the fresh HDR render bit for bit.
#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn studio_to_an_hdr_map_relights_a_dispersive_stone_to_the_fresh_render_bit_for_bit() {
    let rig = BeamRig::brilliant(diamond());
    let map = hdr_map();
    let hdr = EnvironmentSource::hdr(&map);
    let studio_env = studio(LightingPreset::RingLights, 0.3, 0.9);
    assert!(tau_omega(&hdr) < tau_omega(&studio_env));
    let (before, after) = check_relight("diamond studio to hdr", &rig, studio_env, hdr);
    eprintln!(
        "diamond leaf refinement changed the count by {}",
        after.abs_diff(before)
    );
}

/// Turning the HDR map is a lighting change like a new Studio pose: the retained leaves are the
/// same (the map's width, hence the leaf size, is unchanged) and the re-light equals the fresh
/// render of the turned map bit for bit.
#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn turning_the_hdr_map_relights_to_the_fresh_render_bit_for_bit() {
    let rig = rig();
    let map = hdr_map();
    let unturned = EnvironmentSource::hdr(&map);
    let turned = unturned.with_hdr_rotation(crate::renderer::env_map::EnvRotation::from_yaw(1.1));
    let (before, after) = check_relight("hdr turn", &rig, unturned, turned);
    assert_eq!(before, after, "a turn needs no finer leaves");
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_second_relight_under_the_same_lighting_changes_nothing() {
    let rig = rig();
    let map = hdr_map();
    let scene_from = scene_with(&rig, studio(LightingPreset::RingLights, 0.3, 0.9));
    let scene_to = scene_with(&rig, EnvironmentSource::hdr(&map));
    let mut cache = LeafCache::new(u64::MAX);
    let _ = fresh(&rig, &scene_from, Some(&mut cache));
    let engines = EngineSet::new(Vec::new());
    let cancel = AtomicBool::new(false);
    let first = cache
        .relight(&scene_to, &beam_settings(), &engines, &cancel)
        .expect("retained leaves");
    let leaves = cache.leaf_count();
    let bytes = cache.bytes();
    let second = cache
        .relight(&scene_to, &beam_settings(), &engines, &cancel)
        .expect("retained leaves");
    assert_eq!(cache.leaf_count(), leaves);
    assert_eq!(cache.bytes(), bytes);
    assert_same_image("second re-light", &second, &first);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_glare_change_relights_to_the_fresh_render() {
    let rig = rig();
    let (before, after) = check_relight(
        "glare",
        &rig,
        studio(LightingPreset::RingLights, 0.3, 0.9),
        studio(LightingPreset::RingLights, 0.3, 0.9).with_surface_glare(0.25),
    );
    assert_eq!(before, after);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_head_shadow_change_relights_to_the_fresh_render() {
    let rig = rig();
    let (before, after) = check_relight(
        "head shadow",
        &rig,
        studio(LightingPreset::RingLights, 0.3, 0.9),
        studio(LightingPreset::RingLights, 0.3, 0.9).with_head_shadow(40.0),
    );
    assert_eq!(before, after);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn batches_over_the_cap_are_retraced_and_give_the_same_image() {
    let rig = rig();
    let scene_from = scene_with(&rig, studio(LightingPreset::RingLights, 0.3, 0.9));
    let scene_to = scene_with(&rig, studio(LightingPreset::RingLights, 1.7, 0.5));
    let mut full = LeafCache::new(u64::MAX);
    let _ = fresh(&rig, &scene_from, Some(&mut full));
    let cap = full.bytes() / 4 * 3;
    let mut capped = LeafCache::new(cap);
    let _ = fresh(&rig, &scene_from, Some(&mut capped));
    eprintln!(
        "full {} batches / {} bytes, capped {} batches / {} bytes (cap {cap})",
        full.batch_count(),
        full.bytes(),
        capped.batch_count(),
        capped.bytes()
    );
    assert!(capped.bytes() <= cap);
    assert!(capped.batch_count() > 0, "the cap kept nothing");
    assert!(
        capped.batch_count() < full.batch_count(),
        "the cap refused nothing"
    );

    let engines = EngineSet::new(Vec::new());
    let cancel = AtomicBool::new(false);
    let from_full = full
        .relight(&scene_to, &beam_settings(), &engines, &cancel)
        .expect("retained leaves");
    let from_capped = capped
        .relight(&scene_to, &beam_settings(), &engines, &cancel)
        .expect("retained leaves");
    let reference = fresh(&rig, &scene_to, None);
    assert_same_image("full cache", &from_full, &reference);
    assert_same_image("capped cache", &from_capped, &reference);
    assert!(capped.bytes() <= cap, "the re-light overran the cap");
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_cancelled_relight_returns_none() {
    let rig = rig();
    let scene = scene_with(&rig, studio(LightingPreset::RingLights, 0.3, 0.9));
    let mut cache = LeafCache::new(u64::MAX);
    let _ = fresh(&rig, &scene, Some(&mut cache));
    let engines = EngineSet::new(Vec::new());
    let raised = AtomicBool::new(true);
    assert_eq!(
        cache.relight(&scene, &beam_settings(), &engines, &raised),
        None
    );
    let background = vec![Vec3::ZERO; (FRAME * FRAME) as usize];
    assert_eq!(
        cache.relight_ray_traced(&scene, &beam_settings(), &background, &raised),
        None
    );
    assert!(!cache.is_empty(), "a cancelled re-light keeps the leaves");
}

// ---- the Ray-Traced view's re-light ----

/// The Whitted engine's image of `scene` (the mean of its anti-aliasing taps per pixel).
fn whitted_image(rig: &BeamRig, scene: &FrameScene<'_>) -> Vec<Vec3> {
    let prep = WhittedPrep::new(scene, Fluorescence::none(), RayOptions::default())
        .expect("a plain stone is a Whitted scene");
    let taps = u32::from(prep.taps).max(1);
    let sums = whitted_pixels_interleaved_geom(scene, &[], &rig.soa, &prep, 0, 1, 0, taps);
    let scale = f32::from(u16::try_from(taps).unwrap_or(1));
    sums.into_iter().map(|sum| sum / scale).collect()
}

/// The mean of the three channels over `pixels`.
fn mean_of(pixels: impl Iterator<Item = Vec3>) -> f64 {
    let (sum, count) = pixels.fold((0.0_f64, 0_u32), |(sum, count), p| {
        (sum + f64::from(p.element_sum()) / 3.0, count + 1)
    });
    sum / f64::from(count.max(1))
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn the_ray_traced_relight_is_deterministic_and_the_same_for_a_capped_cache() {
    let rig = rig();
    let scene_from = scene_with(&rig, studio(LightingPreset::RingLights, 0.3, 0.9));
    let scene_to = scene_with(&rig, studio(LightingPreset::RingLights, 1.7, 0.5));
    let background = whitted_image(&rig, &scene_to);
    let cancel = AtomicBool::new(false);

    let mut full = LeafCache::new(u64::MAX);
    let _ = fresh(&rig, &scene_from, Some(&mut full));
    let first = full
        .relight_ray_traced(&scene_to, &beam_settings(), &background, &cancel)
        .expect("retained leaves");
    let again = full
        .relight_ray_traced(&scene_to, &beam_settings(), &background, &cancel)
        .expect("retained leaves");
    assert_same_image("ray-traced twice", &again, &first);

    let mut capped = LeafCache::new(full.bytes() / 2);
    let _ = fresh(&rig, &scene_from, Some(&mut capped));
    assert!(capped.batch_count() < full.batch_count());
    if capped.is_empty() {
        eprintln!("the half cap kept nothing; the capped row is skipped");
        return;
    }
    let from_capped = capped
        .relight_ray_traced(&scene_to, &beam_settings(), &background, &cancel)
        .expect("retained leaves");
    assert_same_image("ray-traced capped cache", &from_capped, &first);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn the_ray_traced_relight_matches_beams_plus_residual_on_covered_pixels() {
    let rig = rig();
    let scene_from = scene_with(&rig, studio(LightingPreset::RingLights, 0.3, 0.9));
    let scene_to = scene_with(&rig, studio(LightingPreset::Incandescent, 1.7, 0.5));
    let background = whitted_image(&rig, &scene_to);
    let cancel = AtomicBool::new(false);
    let mut cache = LeafCache::new(u64::MAX);
    let _ = fresh(&rig, &scene_from, Some(&mut cache));
    let coverage = cache.coverage((FRAME, FRAME));
    let traced = cache
        .relight_ray_traced(&scene_to, &beam_settings(), &background, &cancel)
        .expect("retained leaves");
    let hybrid = fresh(&rig, &scene_to, None);

    let covered: Vec<usize> = coverage
        .iter()
        .enumerate()
        .filter(|(_, share)| **share >= FULLY_COVERED)
        .map(|(at, _)| at)
        .collect();
    assert!(!covered.is_empty(), "no pixel is fully covered");
    let difference = mean_of(covered.iter().map(|&at| (traced[at] - hybrid[at]).abs()));
    let level = mean_of(covered.iter().map(|&at| hybrid[at]));
    let relative = difference / level.max(f64::MIN_POSITIVE);
    eprintln!(
        "ray-traced vs beams + residual on {} covered pixels: mean relative difference {relative:.4} (bound {COVERED_TOLERANCE})",
        covered.len()
    );
    assert!(relative <= COVERED_TOLERANCE);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn the_ray_traced_relight_is_close_to_the_whitted_image_in_level() {
    let rig = rig();
    let scene_from = scene_with(&rig, studio(LightingPreset::RingLights, 0.3, 0.9));
    let scene_to = scene_with(&rig, studio(LightingPreset::RingLights, 1.7, 0.5));
    let background = whitted_image(&rig, &scene_to);
    let cancel = AtomicBool::new(false);
    let mut cache = LeafCache::new(u64::MAX);
    let _ = fresh(&rig, &scene_from, Some(&mut cache));
    let traced = cache
        .relight_ray_traced(&scene_to, &beam_settings(), &background, &cancel)
        .expect("retained leaves");
    assert_eq!(traced.len(), background.len());
    assert!(
        traced
            .iter()
            .all(|p| p.is_finite() && p.min_element() >= 0.0)
    );
    let traced_mean = mean_of(traced.iter().copied());
    let whitted_mean = mean_of(background.iter().copied());
    let difference = mean_of(traced.iter().zip(&background).map(|(a, b)| (*a - *b).abs()));
    eprintln!(
        "ray-traced mean {traced_mean:.4}, Whitted mean {whitted_mean:.4}, mean absolute difference {difference:.4} (level ratio bound {WHITTED_FACTOR})"
    );
    let ratio = traced_mean / whitted_mean.max(f64::MIN_POSITIVE);
    assert!((1.0 / WHITTED_FACTOR..=WHITTED_FACTOR).contains(&ratio));
}

/// A fresh beam render of `scene` under `beam` (no cache), the final pixels.
fn render_with(rig: &BeamRig, scene: &FrameScene<'_>, beam: &BeamSettings) -> Vec<Vec3> {
    let adaptive = adaptive_settings();
    let engines = EngineSet::new(Vec::new());
    let cancel = AtomicBool::new(false);
    let mut progress = |_: BeamProgress| {};
    let (image, _) = render_beam_hybrid_surface(
        scene,
        (&[], Fluorescence::none(), SurfaceScene::IDEAL),
        &rig.soa,
        beam,
        &adaptive,
        &engines,
        (0, 1.0),
        &cancel,
        BeamRun::new(&mut progress),
    )
    .expect("an eligible stone");
    image.pixels
}

/// A tile over the memory cap renders residual-only and has no retained batch; a hybrid re-light
/// still copies its pixels.
#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_residual_only_missing_tile_is_not_black_after_a_hybrid_relight() {
    let rig = rig();
    let scene = scene_with(&rig, studio(LightingPreset::RingLights, 0.3, 0.9));
    let capped = BeamSettings {
        memory_cap_bytes: 1,
        ..beam_settings()
    };
    let mut batches = batches_of(&rig, &scene);
    assert_eq!(batches.len(), 4, "one tile per batch");
    let missing = batches.pop().expect("a last batch").tiles[0];
    let mut cache = LeafCache::new(u64::MAX);
    for batch in batches {
        assert!(cache.retain(batch));
    }
    let engines = EngineSet::new(Vec::new());
    let cancel = AtomicBool::new(false);
    let relit = cache
        .relight(&scene, &capped, &engines, &cancel)
        .expect("retained leaves");
    assert_eq!(cache.batch_count(), 3, "a residual-only tile is not kept");
    let reference = render_with(&rig, &scene, &capped);
    let width = FRAME as usize;
    let mut lit = false;
    for y in missing.y0..missing.y1 {
        for x in missing.x0..missing.x1 {
            let at = y as usize * width + x as usize;
            assert_eq!(
                relit[at].to_array().map(f32::to_bits),
                reference[at].to_array().map(f32::to_bits),
                "pixel ({x}, {y}) of the residual-only tile"
            );
            lit |= relit[at] != Vec3::ZERO;
        }
    }
    assert!(lit, "the residual-only tile came back black");
}

/// A refined leaf that dropped a piece is never restored whole. A parent that passed whole
/// records no drop, so a single part with the parent's polygon and a recorded drop would be the
/// stale parent standing in for a dropped half.
#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_refined_leaf_that_dropped_a_piece_is_not_restored_whole() {
    let rig = rig();
    let scene_from = scene_with(&rig, studio(LightingPreset::RingLights, 0.3, 0.9));
    let map = hdr_map();
    let scene_to = scene_with(&rig, EnvironmentSource::hdr(&map));
    let mut cache = LeafCache::new(u64::MAX);
    let _ = fresh(&rig, &scene_from, Some(&mut cache));
    // A shallow split depth forces drops at the cap.
    let settings = BeamSettings {
        max_split_depth: 2,
        ..beam_settings()
    };
    let Ok(frame) = BeamFrame::new(&scene_to, &settings) else {
        panic!("a beam frame");
    };
    let size = (scene_to.width, scene_to.height);
    let reflection = ReflectionKit::new(
        scene_to.material,
        scene_to.planes,
        scene_to.camera,
        size,
        &scene_to.environment,
        frame.n_lambda(),
    );
    let retrace = RetraceKit {
        camera: scene_to.camera,
        size,
        stone: frame.stone(),
        material: scene_to.material,
        n_lambda: frame.n_lambda(),
    };
    let (mut refined, mut with_drops) = (0_usize, 0_usize);
    for batch in cache.retained_batches() {
        for (index, leaves) in &batch.leaves {
            let rect = TileRect::of_index(size, TILE, *index).expect("a tile of the frame");
            let kit = reflection.interact_kit(retrace, &settings, rect, scene_to.max_bounces);
            for leaf in leaves.iter().filter(|leaf| !leaf.reflection) {
                let mut stats = BeamStats::default();
                let Some(parts) = refine_leaf(leaf, &kit, &mut stats) else {
                    continue;
                };
                refined += 1;
                if stats.drops.iter().sum::<u64>() == 0 {
                    continue;
                }
                with_drops += 1;
                let restored = parts.len() == 1
                    && parts[0].image.area().to_bits() == leaf.image.area().to_bits();
                assert!(!restored, "a dropped piece was covered by the stale parent");
            }
        }
    }
    eprintln!("{refined} leaves refined, {with_drops} with a drop");
    assert!(refined > 0, "nothing was refined");
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_background_of_the_wrong_size_is_refused() {
    let rig = rig();
    let scene = scene_with(&rig, studio(LightingPreset::RingLights, 0.3, 0.9));
    let mut cache = LeafCache::new(u64::MAX);
    let _ = fresh(&rig, &scene, Some(&mut cache));
    let cancel = AtomicBool::new(false);
    let background = vec![Vec3::ZERO; 3];
    assert_eq!(
        cache.relight_ray_traced(&scene, &beam_settings(), &background, &cancel),
        None
    );
}
