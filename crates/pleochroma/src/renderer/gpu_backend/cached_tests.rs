//! GPU backend tests of live passes with a path cache (`gpu` feature). Like the tests next door
//! they skip with a printed note when no adapter is present (`PLEOCHROMA_REQUIRE_GPU=1` turns the
//! skip into a failure).
//!
//! The contract under test: a cached pass adds the samples `sample_offset..sample_offset + spp`
//! to the live pixels of the live tiles, into the summed accumulator and, for the odd absolute
//! indices, into half-buffer B; pixels outside the live tiles are untouched. The cached stone
//! pixels shade in the phase B kernel (the twin of `Shader::shade_sample`), the others on the
//! ordinary live path tracer. GPU-vs-CPU and cached-vs-uncached comparisons use tolerances;
//! the determinism test compares bits (nothing in a pass depends on timing).
//!
//! The residual completion runs on the GPU too; the completion twin tests compare every cached
//! pixel of every sample with the CPU shader (`PathCacheFrame::shade_sample`) at a coarse cutoff
//! where most samples fire. The cached-mean test compares 4096 samples per pixel in 16 batches
//! against the path tracer through the cache's own positions, with the standard errors taken from
//! the batches' spread; the difference to the uncached pass (the positions' quadrature) is printed.
//!
//! Tolerances are estimates. The tests print the worst cases and measured shares so the bounds
//! can be tightened.

use std::{sync::atomic::AtomicBool, time::Duration};

use glam::Vec3;

use super::{
    CacheTier, GpuAccumulate, GpuBackend, GpuPathCache, GpuSceneRef,
    tests::{acquire_or_skip, bits},
};
use crate::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts, tool::StoneGeometry},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            Camera, EnvironmentSource, LightingPreset, build_plane_soa, pixel_rotations,
            sample_draws,
            surface::SurfaceScene,
            transport::{PreparedMaterial, trace_spectral_ray_prepared},
        },
    },
    renderer::{
        adaptive::LiveTiles,
        frame_scene::FrameScene,
        gpu::frame::classify_material,
        path_cache::{
            BuildSettings, CacheOptions, DISPERSION_GATE_BG, PathCacheFrame,
            build::dispersion_b_minus_g, layout::tile_rect, shade::pick_position,
        },
    },
    simd::PlanesSoA32,
};

const MAX_BOUNCES: u32 = 6;

/// 32 x 24 pixels in 8-pixel tiles: 4 x 3 tiles.
const FRAME: (u32, u32) = (32, 24);
const TILE: u32 = 8;

/// A value no pass writes, to tell untouched pixels from zero.
const SENTINEL: Vec3 = Vec3::new(7.25, -3.5, 1.0e-3);

/// Per-sample twin tolerance: relative, plus an absolute floor for dark samples.
const SAMPLE_REL_TOL: f32 = 2.0e-3;
const SAMPLE_ABS_TOL: f32 = 1.0e-4;

/// The share of (pixel, sample) pairs allowed outside the twin tolerance (a position pick that
/// rounds the other way, a contracted multiply-add at a sharp lighting edge).
const SAMPLE_OUTLIER_SHARE: f64 = 0.02;

/// How far the cached and the reference tile totals may differ (two unbiased estimators of the
/// same mean, the path tracer through the cache's positions), relative to the larger total or a
/// fifth of the mean, on top of [`TILE_NOISE_SIGMAS`] standard errors of the difference.
const TILE_TOTAL_REL_TOL: f64 = 0.08;

/// The noise allowance of the relative rule, in standard errors of the difference. A
/// heavy-tailed tile can have a standard error of 5 % of its total, so the 8 % alone would
/// fail on ordinary noise.
const TILE_NOISE_SIGMAS: f64 = 3.0;

/// Branches lighter than this are dropped in the completion fixtures (the default is
/// `whitted::CUTOFF`, 1e-3). Most positions then drop more than an eighth of their light and
/// fire on every sample (`p = min(1, 8 r)`), so the completion runs on most samples.
const COARSE_CUTOFF: f32 = 0.05;

/// The share of FIRED samples allowed outside the twin tolerance. A traced path is a chain of
/// draws against computed probabilities and lighting edges, and an f32 difference at one of
/// them sends the GPU and the CPU path different ways. An estimate, printed with its measured
/// share.
const COMPLETION_OUTLIER_SHARE: f64 = 0.03;

/// The share of compared samples that must fire for a completion twin to test anything.
const MIN_FIRED_SHARE: f64 = 0.25;

/// Batches of the cached-mean comparison (sample offsets `MEAN_BATCH_SPP * b`): 4096 samples per
/// pixel in all.
const MEAN_BATCHES: u32 = 16;

/// Samples per pixel of one cached-mean batch.
const MEAN_BATCH_SPP: u32 = 256;

/// Samples per pixel of one batch of the CPU reference through the cache's positions (16 batches,
/// 1024 samples per cached pixel).
const REF_BATCH_SPP: u32 = 64;

/// The first sample index of the reference, past every sample of the cached pass, so the two
/// share no draw and the z statistic's independence holds.
const REF_FIRST_SAMPLE: u32 = MEAN_BATCHES * MEAN_BATCH_SPP;

/// A tile whose cached-minus-reference total lies further than this many standard errors (from the
/// batch-to-batch spread of both) is not noise.
const TILE_Z_LIMIT: f64 = 5.0;

struct Fixture {
    camera: Camera,
    planes: Vec<GpuFacetPlane>,
    material: GemMaterial,
    soa: PlanesSoA32,
    environment: EnvironmentSource<'static>,
}

impl Fixture {
    /// A colorless uniaxial stone (the sapphire-like case of the phase B equivalence tests).
    fn new() -> Self {
        Self::with_material(GemMaterial::new_custom(
            "cached gpu test sapphire",
            1.768,
            0.0106,
            0.008,
            [0.0; 3],
        ))
    }

    /// The standard round brilliant in `material`.
    fn with_material(material: GemMaterial) -> Self {
        let planes = StandardGemCuts::standard_round_brilliant();
        let soa = build_plane_soa(&planes);
        Self {
            camera: Camera::new(0.35, 0.28, 5.0, 18.0),
            planes,
            material,
            soa,
            environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
        }
    }

    /// The s2 benchmark sapphire under its Daylight studio (head shadow 16, surface glare 1).
    fn colored() -> Self {
        Self {
            environment: crate::renderer::test_stats::bench_daylight(),
            ..Self::with_material(crate::renderer::test_stats::bench_sapphire())
        }
    }

    fn scene(&self) -> FrameScene<'_> {
        FrameScene {
            camera: &self.camera,
            width: FRAME.0,
            height: FRAME.1,
            planes: &self.planes,
            facet_finishes: &[],
            material: &self.material,
            max_bounces: MAX_BOUNCES,
            environment: self.environment,
        }
    }

    fn gpu_scene(&self) -> GpuSceneRef<'_> {
        GpuSceneRef {
            camera: &self.camera,
            width: FRAME.0,
            height: FRAME.1,
            planes: &self.planes,
            facet_finishes: &[],
            material: &self.material,
            max_bounces: MAX_BOUNCES,
            environment: self.environment,
            estimator: None,
        }
    }
}

fn options() -> CacheOptions {
    options_with_cutoff(BuildSettings::default().cutoff)
}

/// The test options with branches lighter than `cutoff` dropped.
fn options_with_cutoff(cutoff: f32) -> CacheOptions {
    CacheOptions {
        settings: BuildSettings {
            tile: TILE,
            k_max: 8,
            cutoff,
            ..BuildSettings::default()
        },
        ..CacheOptions::default()
    }
}

fn cache_frame<'s, 'a>(scene: &'s FrameScene<'a>, soa: &'s PlanesSoA32) -> PathCacheFrame<'s, 'a> {
    cache_frame_with(scene, soa, options())
}

/// The cache frame of `scene` built with `options`.
fn cache_frame_with<'s, 'a>(
    scene: &'s FrameScene<'a>,
    soa: &'s PlanesSoA32,
    options: CacheOptions,
) -> PathCacheFrame<'s, 'a> {
    PathCacheFrame::new(
        scene,
        soa,
        (&[], Fluorescence::none(), SurfaceScene::IDEAL),
        options,
    )
    .expect("the stone may use the cache")
}

/// Per frame pixel (row-major): true when the cache has no positions for it, so the live path
/// tracer shades it.
fn rest_pixels(frame: &PathCacheFrame<'_, '_>) -> Vec<bool> {
    let mut rest = Vec::with_capacity(frame_len());
    for y in 0..FRAME.1 {
        for x in 0..FRAME.0 {
            let index = frame.tile_of((x, y));
            let (x0, y0, x1, _) = tile_rect(FRAME.0, FRAME.1, TILE, index);
            let local = (y - y0) as usize * (x1 - x0) as usize + (x - x0) as usize;
            rest.push(
                frame
                    .tile(index)
                    .is_none_or(|tile| tile.pixel_positions(local).is_empty()),
            );
        }
    }
    rest
}

const fn frame_len() -> usize {
    (FRAME.0 * FRAME.1) as usize
}

/// One cached pass over `live` into buffers pre-filled with `fill`.
fn cached_pass(
    backend: &GpuBackend,
    fixture: &Fixture,
    cache: &GpuPathCache<'_, '_, '_>,
    live: &LiveTiles,
    (sample_offset, spp): (u32, u32),
    fill: Vec3,
) -> (GpuAccumulate, Vec<Vec3>, Vec<Vec3>) {
    let mut accum = vec![fill; frame_len()];
    let mut half_b = vec![fill; frame_len()];
    let never_cancel = AtomicBool::new(false);
    let outcome = backend.try_accumulate_cached_live_cancellable(
        &fixture.gpu_scene(),
        &[],
        &SurfaceScene::IDEAL,
        sample_offset,
        spp,
        live,
        cache,
        &mut accum,
        &mut half_b,
        &never_cancel,
    );
    (outcome, accum, half_b)
}

/// One uncached live pass over `live` into zeroed buffers.
fn uncached_pass(
    backend: &GpuBackend,
    fixture: &Fixture,
    live: &LiveTiles,
    (sample_offset, spp): (u32, u32),
) -> (Vec<Vec3>, Vec<Vec3>) {
    let mut accum = vec![Vec3::ZERO; frame_len()];
    let mut half_b = vec![Vec3::ZERO; frame_len()];
    let never_cancel = AtomicBool::new(false);
    let outcome = backend.try_accumulate_geom_surface_live_cancellable(
        &fixture.gpu_scene(),
        &[],
        &SurfaceScene::IDEAL,
        sample_offset,
        spp,
        live,
        &mut accum,
        &mut half_b,
        &never_cancel,
    );
    assert_eq!(outcome, GpuAccumulate::Done);
    (accum, half_b)
}

/// Channel totals of the cached pixels (those not in `rest`) of tile `index` of `live` in `pixels`.
fn cached_tile_total(pixels: &[Vec3], live: &LiveTiles, index: u32, rest: &[bool]) -> [f64; 3] {
    let (x0, y0, x1, y1) = live.tile_rect(index);
    let mut total = [0.0_f64; 3];
    for y in y0..y1 {
        for x in x0..x1 {
            let at = (y * live.width() + x) as usize;
            if rest[at] {
                continue;
            }
            let p = pixels[at];
            total[0] += f64::from(p.x);
            total[1] += f64::from(p.y);
            total[2] += f64::from(p.z);
        }
    }
    total
}

/// The phase B kernel against `Shader::shade_sample`: with one sample per pass the value a cached
/// pixel gets IS the sample (plus its residual completion), so every cached pixel of every sample
/// index can be compared with the CPU twin.
#[test]
fn the_phase_b_kernel_matches_the_cpu_shader_sample_for_sample() {
    let Some(backend) =
        acquire_or_skip("the_phase_b_kernel_matches_the_cpu_shader_sample_for_sample")
    else {
        return;
    };
    assert_phase_b_twin(&backend, &Fixture::new());
}

/// The phase B kernel's absorption (`pcb_principal_alphas`, `pcb_optical_depth`) against the CPU
/// shader on the colored benchmark sapphire.
#[test]
fn the_phase_b_kernel_matches_the_cpu_shader_on_a_colored_sapphire() {
    let Some(backend) =
        acquire_or_skip("the_phase_b_kernel_matches_the_cpu_shader_on_a_colored_sapphire")
    else {
        return;
    };
    assert_phase_b_twin(&backend, &Fixture::colored());
}

/// The shared body of the phase B twins: eight single-sample passes of `fixture`'s stone against
/// the CPU shader.
fn assert_phase_b_twin(backend: &GpuBackend, fixture: &Fixture) {
    let scene = fixture.scene();
    let frame = cache_frame(&scene, &fixture.soa);
    let cache = GpuPathCache::new(&frame, CacheTier::Vram);
    let live = LiveTiles::all(FRAME.0, FRAME.1, TILE);

    let (mut compared, mut outliers) = (0_usize, 0_usize);
    let mut worst = 0.0_f32;
    for sample in 0..8_u32 {
        let (outcome, accum, _) =
            cached_pass(backend, fixture, &cache, &live, (sample, 1), Vec3::ZERO);
        assert_eq!(outcome, GpuAccumulate::Done);
        for y in 0..FRAME.1 {
            for x in 0..FRAME.0 {
                let rot = pixel_rotations(y * FRAME.0 + x);
                // Pixels the cache has no positions for are the path tracer's: not a twin.
                let Some(parts) = frame.shade_sample((x, y), sample, &rot) else {
                    continue;
                };
                let cpu = parts.total();
                let gpu = accum[(y * FRAME.0 + x) as usize];
                compared += 1;
                let error = (gpu - cpu).abs();
                let allowed =
                    cpu.abs().max(gpu.abs()) * SAMPLE_REL_TOL + Vec3::splat(SAMPLE_ABS_TOL);
                worst = worst.max((error / allowed).max_element());
                if error.cmpgt(allowed).any() {
                    outliers += 1;
                }
            }
        }
    }
    println!(
        "phase B twin: {compared} samples compared, {outliers} outliers, worst error/tolerance {worst:.2}"
    );
    assert!(compared > 0, "the stone must cover some pixels");
    assert!(
        (outliers as f64) <= SAMPLE_OUTLIER_SHARE * compared as f64,
        "{outliers} of {compared} samples are outside the twin tolerance"
    );
}

/// The batches of a cached and an uncached run of the cached-mean test.
struct MeanRun {
    /// Per live tile, per batch: the cached pass's channel totals over the tile's cached pixels.
    cached: Vec<Vec<[f64; 3]>>,
    /// Per live tile, per batch: the uncached pass's channel totals over the same pixels.
    plain: Vec<Vec<[f64; 3]>>,
    /// The summed accumulator and half B over all batches, cached.
    cached_sum: (Vec<Vec3>, Vec<Vec3>),
    /// The summed accumulator and half B over all batches, uncached.
    plain_sum: (Vec<Vec3>, Vec<Vec3>),
}

/// Runs `MEAN_BATCHES` batches of `MEAN_BATCH_SPP` samples, cached and uncached, and asserts the
/// rest pixels (no cached positions) equal bit for bit in every batch.
fn run_mean_batches(
    backend: &GpuBackend,
    fixture: &Fixture,
    cache: &GpuPathCache<'_, '_, '_>,
    rest: &[bool],
    live: &LiveTiles,
) -> MeanRun {
    let tiles = live.indices().len();
    let mut run = MeanRun {
        cached: vec![Vec::new(); tiles],
        plain: vec![Vec::new(); tiles],
        cached_sum: (vec![Vec3::ZERO; frame_len()], vec![Vec3::ZERO; frame_len()]),
        plain_sum: (vec![Vec3::ZERO; frame_len()], vec![Vec3::ZERO; frame_len()]),
    };
    for batch in 0..MEAN_BATCHES {
        let pass = (batch * MEAN_BATCH_SPP, MEAN_BATCH_SPP);
        let (outcome, cached, cached_half) =
            cached_pass(backend, fixture, cache, live, pass, Vec3::ZERO);
        assert_eq!(outcome, GpuAccumulate::Done);
        let (plain, plain_half) = uncached_pass(backend, fixture, live, pass);
        for (index, &is_rest) in rest.iter().enumerate() {
            if is_rest {
                let at = format!(
                    "pixel {index} (x {}, y {}), batch {batch}",
                    index % FRAME.0 as usize,
                    index / FRAME.0 as usize
                );
                assert_eq!(
                    cached[index].to_array().map(f32::to_bits),
                    plain[index].to_array().map(f32::to_bits),
                    "the sum of a not-cached {at}"
                );
                assert_eq!(
                    cached_half[index].to_array().map(f32::to_bits),
                    plain_half[index].to_array().map(f32::to_bits),
                    "half B of a not-cached {at}"
                );
            }
        }
        for (slot, &t) in live.indices().iter().enumerate() {
            run.cached[slot].push(cached_tile_total(&cached, live, t, rest));
            run.plain[slot].push(cached_tile_total(&plain, live, t, rest));
        }
        for (sums, (a, b)) in [
            (&mut run.cached_sum, (&cached, &cached_half)),
            (&mut run.plain_sum, (&plain, &plain_half)),
        ] {
            for (acc, value) in sums.0.iter_mut().zip(a) {
                *acc += *value;
            }
            for (acc, value) in sums.1.iter_mut().zip(b) {
                *acc += *value;
            }
        }
    }
    run
}

/// The standard error of the total of `MEAN_BATCHES` batches: `sqrt(B * s^2)` with `s^2` the
/// unbiased sample variance of the batch totals.
fn total_standard_error(batch_totals: &[f64]) -> f64 {
    let n = batch_totals.len() as f64;
    let mean = batch_totals.iter().sum::<f64>() / n;
    let squares: f64 = batch_totals.iter().map(|v| (v - mean) * (v - mean)).sum();
    (n * squares / (n - 1.0)).sqrt()
}

/// One channel of one tile of the cached-mean comparison, over the tile's cached pixels.
struct TileChannel {
    /// The cached pass's total (`C`).
    cached: f64,
    /// The path tracer's total through the cache's positions, scaled to the cached pass's
    /// sample count (`R`).
    reference: f64,
    /// The uncached pass's total (`U`), printed only.
    plain: f64,
    /// The standard error of `cached`.
    se_cached: f64,
    /// The standard error of `reference`.
    se_reference: f64,
}

impl TileChannel {
    /// Totals and standard errors of channel `channel` from the per-batch totals; the reference
    /// batches (`REF_BATCH_SPP` samples each) are scaled to `MEAN_BATCH_SPP`.
    fn new(
        cached: &[[f64; 3]],
        reference: &[[f64; 3]],
        plain: &[[f64; 3]],
        channel: usize,
    ) -> Self {
        let scale = f64::from(MEAN_BATCH_SPP / REF_BATCH_SPP);
        let column = |batches: &[[f64; 3]], factor: f64| -> Vec<f64> {
            batches.iter().map(|t| factor * t[channel]).collect()
        };
        let (c, r, p) = (
            column(cached, 1.0),
            column(reference, scale),
            column(plain, 1.0),
        );
        Self {
            cached: c.iter().sum(),
            reference: r.iter().sum(),
            plain: p.iter().sum(),
            se_cached: total_standard_error(&c),
            se_reference: total_standard_error(&r),
        }
    }

    /// The standard error of `C - R`.
    fn spread(&self) -> f64 {
        self.se_cached.hypot(self.se_reference)
    }

    /// `(C - R)` in standard errors; zero when neither total has a spread and they are equal.
    fn z(&self) -> f64 {
        let spread = self.spread();
        let difference = self.cached - self.reference;
        if spread > 0.0 {
            difference / spread
        } else {
            let scale = self.cached.abs().max(self.reference.abs()).max(1.0);
            assert!(
                difference.abs() <= 1.0e-6 * scale,
                "no spread in either total, yet cached {} and reference {} differ",
                self.cached,
                self.reference
            );
            0.0
        }
    }

    /// The larger of the two totals' magnitudes, or `floor`: the scale of the relative rule.
    const fn relative_scale(&self, floor: f64) -> f64 {
        self.reference.abs().max(self.cached.abs()).max(floor)
    }
}

/// Per live tile, per batch: the channel totals of the path tracer through the cache's own
/// positions, over the tile's cached pixels. The cached pass estimates exactly this mean.
fn position_reference(
    fixture: &Fixture,
    frame: &PathCacheFrame<'_, '_>,
    live: &LiveTiles,
) -> Vec<Vec<[f64; 3]>> {
    let prepared = PreparedMaterial::new(&fixture.material);
    let scene = fixture.scene();
    let geom = StoneGeometry::planes_only(&fixture.planes);
    let mut out = Vec::new();
    for &index in live.indices() {
        let (x0, y0, x1, y1) = live.tile_rect(index);
        let mut batches = vec![[0.0_f64; 3]; MEAN_BATCHES as usize];
        for y in y0..y1 {
            for x in x0..x1 {
                let local = (y - y0) as usize * (x1 - x0) as usize + (x - x0) as usize;
                let count = frame
                    .tile(index)
                    .map_or(0, |tile| tile.pixel_positions(local).len());
                if count == 0 {
                    continue;
                }
                let pixel = y * FRAME.0 + x;
                let rot = pixel_rotations(pixel);
                for (b, total) in (0_u32..).zip(batches.iter_mut()) {
                    for s in 0..REF_BATCH_SPP {
                        let sample = REF_FIRST_SAMPLE + b * REF_BATCH_SPP + s;
                        let at = pick_position(pixel, sample, count);
                        let jitter =
                            sample_draws(pixel, u32::try_from(at).expect("a small index"), &rot);
                        let ray = fixture.camera.generate_ray(
                            x as f32,
                            y as f32,
                            FRAME.0 as f32,
                            FRAME.1 as f32,
                            jitter.jitter_x,
                            jitter.jitter_y,
                        );
                        let draws = sample_draws(pixel, sample, &rot);
                        let xyz = trace_spectral_ray_prepared(
                            ray,
                            geom,
                            &fixture.soa,
                            &[],
                            &prepared,
                            Fluorescence::none(),
                            MAX_BOUNCES,
                            scene.environment,
                            draws.seed,
                            draws.hero_rand,
                            None,
                            SurfaceScene::IDEAL,
                        );
                        for (acc, value) in total.iter_mut().zip(xyz.to_array()) {
                            *acc += f64::from(value);
                        }
                    }
                }
            }
        }
        out.push(batches);
    }
    out
}

/// The cached pixels of a tile (those with positions) among its pixels.
fn cached_pixels_of(rest: &[bool], live: &LiveTiles, index: u32) -> (usize, usize) {
    let (x0, y0, x1, y1) = live.tile_rect(index);
    let mut cached = 0;
    let mut not_cached = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            if rest[(y * FRAME.0 + x) as usize] {
                not_cached += 1;
            } else {
                cached += 1;
            }
        }
    }
    (cached, not_cached)
}

/// A cached pass has the mean of the path tracer through the cache's positions, over 4096
/// samples per pixel in 16 batches of 256.
///
/// The pixels the cache does not shade are traced by the same live pipeline in both passes and
/// must be identical bit for bit in every batch. The cached pixels are compared with the path
/// tracer through the cache's own positions (`position_reference`, 1024 samples per pixel, no
/// draw shared with the cached pass): their tile totals agree to within 8 % plus three standard
/// errors of the difference, and in standard errors taken from the batch-to-batch spread of
/// both (`|z| <= TILE_Z_LIMIT`). The uncached pass's total `U` and `U - R`, the sub-pixel
/// quadrature of the positions, are printed per tile, not asserted.
#[test]
fn a_cached_pass_has_the_uncached_passs_mean_on_a_small_sapphire_scene() {
    let Some(backend) =
        acquire_or_skip("a_cached_pass_has_the_uncached_passs_mean_on_a_small_sapphire_scene")
    else {
        return;
    };
    let _ = assert_cached_mean(&backend, &Fixture::new());
}

/// The cached-mean proof on the colored benchmark sapphire: the tile rules of the colorless test,
/// and the frame totals within 1.5 % plus three standard errors per channel. The rule bounds a
/// bias to about 5 %; the tight proof of the absorption is the CPU equivalence and paired tests
/// plus the sample-for-sample twin.
#[test]
fn a_cached_pass_has_the_uncached_passs_mean_on_a_colored_sapphire() {
    let Some(backend) =
        acquire_or_skip("a_cached_pass_has_the_uncached_passs_mean_on_a_colored_sapphire")
    else {
        return;
    };
    let totals = assert_cached_mean(&backend, &Fixture::colored());
    for (channel, (cached, reference, spread)) in totals.into_iter().enumerate() {
        println!(
            "colored frame total, channel {channel}: C {cached:.4} R {reference:.4} ratio {:.4} se {spread:.4}",
            cached / reference.max(1e-12)
        );
        assert!(
            (cached - reference).abs() <= 0.015_f64.mul_add(reference.abs(), 3.0 * spread),
            "channel {channel}: cached frame total {cached} against the reference {reference} (se {spread})"
        );
    }
}

/// The shared body of the cached-mean tests: the tile rules on `fixture`'s stone. Returns the frame
/// totals per channel: (cached, reference, standard error of the difference).
fn assert_cached_mean(backend: &GpuBackend, fixture: &Fixture) -> [(f64, f64, f64); 3] {
    let scene = fixture.scene();
    let frame = cache_frame(&scene, &fixture.soa);
    let cache = GpuPathCache::new(&frame, CacheTier::Vram);
    let live = LiveTiles::all(FRAME.0, FRAME.1, TILE);
    let rest = rest_pixels(&frame);

    let run = run_mean_batches(backend, fixture, &cache, &rest, &live);
    backend.release_path_cache(&cache);
    let reference = position_reference(fixture, &frame, &live);

    // Tiles with no cached pixel have nothing to compare.
    let tiles: Vec<(u32, [TileChannel; 3])> = live
        .indices()
        .iter()
        .enumerate()
        .filter(|&(_, &index)| cached_pixels_of(&rest, &live, index).0 > 0)
        .map(|(slot, &index)| {
            let channels = std::array::from_fn(|channel| {
                TileChannel::new(
                    &run.cached[slot],
                    &reference[slot],
                    &run.plain[slot],
                    channel,
                )
            });
            (index, channels)
        })
        .collect();
    let mean_tile = tiles.iter().map(|(_, t)| t[1].reference).sum::<f64>() / tiles.len() as f64;
    let mut worst = (0.0_f64, 0.0_f64);
    for (index, channels) in &tiles {
        let (cached_px, rest_px) = cached_pixels_of(&rest, &live, *index);
        println!("tile {index}: {cached_px} cached pixels, {rest_px} rest pixels");
        for (channel, t) in channels.iter().enumerate() {
            let relative = (t.cached - t.reference).abs() / t.relative_scale(mean_tile / 5.0);
            let z = t.z();
            worst = (worst.0.max(relative), worst.1.max(z.abs()));
            println!(
                "  channel {channel}: C {:.4} R {:.4} relative {relative:.4} se(C) {:.4} se(R) {:.4} z {z:+.2}; uncached U {:.4}, U - R {:+.4} (sub-pixel quadrature of the fixture, k_max 8)",
                t.cached,
                t.reference,
                t.se_cached,
                t.se_reference,
                t.plain,
                t.plain - t.reference
            );
        }
    }
    println!(
        "cached vs position-reference tile totals over {} samples per pixel: worst relative difference {:.4}, worst |z| {:.2}",
        MEAN_BATCHES * MEAN_BATCH_SPP,
        worst.0,
        worst.1
    );
    for (index, channels) in &tiles {
        for (channel, t) in channels.iter().enumerate() {
            let allowed = TILE_TOTAL_REL_TOL.mul_add(
                t.relative_scale(mean_tile / 5.0),
                TILE_NOISE_SIGMAS * t.spread(),
            );
            assert!(
                (t.cached - t.reference).abs() <= allowed,
                "tile {index} channel {channel}: cached {} vs reference {} differ by more than {} % plus {TILE_NOISE_SIGMAS} standard errors ({allowed})",
                t.cached,
                t.reference,
                TILE_TOTAL_REL_TOL * 100.0
            );
            assert!(
                t.z().abs() <= TILE_Z_LIMIT,
                "tile {index} channel {channel}: cached {} vs reference {} is {:+.2} standard errors",
                t.cached,
                t.reference,
                t.z()
            );
        }
    }

    // The half buffer holds half the samples of the sum, in mean, for both.
    let half_share = |(sum, half): &(Vec<Vec3>, Vec<Vec3>)| -> f64 {
        let s: f64 = sum.iter().map(|p| f64::from(p.y)).sum();
        let h: f64 = half.iter().map(|p| f64::from(p.y)).sum();
        h / s.max(1e-12)
    };
    let (a, b) = (half_share(&run.cached_sum), half_share(&run.plain_sum));
    assert!(
        (a - 0.5).abs() < 0.05 && (b - 0.5).abs() < 0.05,
        "half shares {a} and {b}"
    );

    // Frame totals per channel: (cached, reference, standard error of the difference).
    std::array::from_fn(|channel| {
        let (mut cached, mut reference, mut variance) = (0.0_f64, 0.0_f64, 0.0_f64);
        for (_, channels) in &tiles {
            let t = &channels[channel];
            cached += t.cached;
            reference += t.reference;
            variance = t.spread().mul_add(t.spread(), variance);
        }
        (cached, reference, variance.sqrt())
    })
}

/// Nothing in a pass may depend on when an upload happens, on the slab size or on the tier: one
/// run with a slab per tile and an artificial delay in every upload equals a run with one big
/// resident slab, bit for bit.
#[test]
fn the_output_does_not_depend_on_upload_timing_slab_size_or_tier() {
    let Some(backend) =
        acquire_or_skip("the_output_does_not_depend_on_upload_timing_slab_size_or_tier")
    else {
        return;
    };
    let fixture = Fixture::new();
    let scene = fixture.scene();
    let frame = cache_frame(&scene, &fixture.soa);
    let live = LiveTiles::all(FRAME.0, FRAME.1, TILE);
    let pass = (5, 24);

    let streaming = GpuPathCache::new(&frame, CacheTier::SystemRam)
        .with_slab_budget_bytes(1)
        .with_stage_delay(Duration::from_millis(3));
    let (outcome, slow, slow_half) =
        cached_pass(&backend, &fixture, &streaming, &live, pass, Vec3::ZERO);
    assert_eq!(outcome, GpuAccumulate::Done);
    assert!(streaming.planned_slabs() > 1, "one slab per tile streams");
    assert_eq!(
        slabs_on_gpu(&backend),
        0,
        "the system-RAM tier drops slabs after use"
    );

    let resident = GpuPathCache::new(&frame, CacheTier::Vram);
    let (outcome, fast, fast_half) =
        cached_pass(&backend, &fixture, &resident, &live, pass, Vec3::ZERO);
    assert_eq!(outcome, GpuAccumulate::Done);
    assert_eq!(resident.planned_slabs(), 1);
    assert!(
        slabs_on_gpu(&backend) > 0,
        "the video-memory tier keeps its slab"
    );
    assert_eq!(bits(&slow), bits(&fast));
    assert_eq!(bits(&slow_half), bits(&fast_half));

    // A second pass over the resident slab uploads nothing and gives the same bits again.
    let (_, again, again_half) =
        cached_pass(&backend, &fixture, &resident, &live, pass, Vec3::ZERO);
    assert_eq!(bits(&fast), bits(&again));
    assert_eq!(bits(&fast_half), bits(&again_half));

    backend.release_path_cache(&resident);
    assert_eq!(slabs_on_gpu(&backend), 0);
}

/// Slabs of any cache currently on the GPU.
fn slabs_on_gpu(backend: &GpuBackend) -> usize {
    let mutex = backend.renderer.as_ref().expect("a GPU backend");
    let renderer = mutex.lock().expect("the renderer lock");
    renderer.cached_slab_count()
}

/// Whether the GPU completion of material class `class` is available on this device, as the
/// renderer reports it (`None` when it has no answer for the class).
fn completes_on_gpu(backend: &GpuBackend, class: u32) -> Option<bool> {
    let mutex = backend.renderer.as_ref().expect("a GPU backend");
    let renderer = mutex.lock().expect("the renderer lock");
    renderer.completes_on_gpu(class)
}

/// Pixels outside the live tiles are untouched, and the half buffer follows the sample parity.
#[test]
fn only_live_tiles_change_and_half_b_follows_the_sample_parity() {
    let Some(backend) =
        acquire_or_skip("only_live_tiles_change_and_half_b_follows_the_sample_parity")
    else {
        return;
    };
    let fixture = Fixture::new();
    let scene = fixture.scene();
    let frame = cache_frame(&scene, &fixture.soa);
    let cache = GpuPathCache::new(&frame, CacheTier::Vram);
    let live = LiveTiles::from_indices(FRAME.0, FRAME.1, TILE, vec![1, 5, 6]);

    let (outcome, accum, half_b) = cached_pass(&backend, &fixture, &cache, &live, (3, 1), SENTINEL);
    assert_eq!(outcome, GpuAccumulate::Done);
    let in_live = |index: usize| {
        let index = u32::try_from(index).expect("a frame index fits a u32");
        let (x, y) = (index % FRAME.0, index / FRAME.0);
        live.indices().contains(&frame.tile_of((x, y)))
    };
    for index in 0..frame_len() {
        if in_live(index) {
            // One odd sample: the sum IS the odd half, bitwise.
            assert_eq!(
                accum[index].to_array().map(f32::to_bits),
                half_b[index].to_array().map(f32::to_bits)
            );
        } else {
            assert_eq!(
                accum[index], SENTINEL,
                "pixel {index} is outside the live tiles"
            );
            assert_eq!(half_b[index], SENTINEL);
        }
    }

    // One even sample: half-buffer B receives nothing.
    let (outcome, _, half_b) = cached_pass(&backend, &fixture, &cache, &live, (4, 1), Vec3::ZERO);
    assert_eq!(outcome, GpuAccumulate::Done);
    assert!(half_b.iter().all(|p| *p == Vec3::ZERO));
    backend.release_path_cache(&cache);
}

/// The declines: another frame size, and a cancelled pass.
#[test]
fn a_mismatched_or_cancelled_pass_leaves_the_buffers_untouched() {
    let Some(backend) =
        acquire_or_skip("a_mismatched_or_cancelled_pass_leaves_the_buffers_untouched")
    else {
        return;
    };
    let fixture = Fixture::new();
    let scene = fixture.scene();
    let frame = cache_frame(&scene, &fixture.soa);
    let cache = GpuPathCache::new(&frame, CacheTier::Vram);

    let wrong_size = LiveTiles::all(FRAME.0 + TILE, FRAME.1, TILE);
    let (outcome, accum, half_b) =
        cached_pass(&backend, &fixture, &cache, &wrong_size, (0, 2), SENTINEL);
    assert_eq!(outcome, GpuAccumulate::Declined);
    assert!(accum.iter().chain(&half_b).all(|p| *p == SENTINEL));

    let wrong_tile = LiveTiles::all(FRAME.0, FRAME.1, TILE * 2);
    let (outcome, accum, _) =
        cached_pass(&backend, &fixture, &cache, &wrong_tile, (0, 2), SENTINEL);
    assert_eq!(outcome, GpuAccumulate::Declined);
    assert!(accum.iter().all(|p| *p == SENTINEL));

    let live = LiveTiles::all(FRAME.0, FRAME.1, TILE);
    let mut accum = vec![SENTINEL; frame_len()];
    let mut half_b = vec![SENTINEL; frame_len()];
    let cancelled = AtomicBool::new(true);
    let outcome = backend.try_accumulate_cached_live_cancellable(
        &fixture.gpu_scene(),
        &[],
        &SurfaceScene::IDEAL,
        0,
        4,
        &live,
        &cache,
        &mut accum,
        &mut half_b,
        &cancelled,
    );
    assert_eq!(outcome, GpuAccumulate::Cancelled);
    assert!(accum.iter().chain(&half_b).all(|p| *p == SENTINEL));
    backend.release_path_cache(&cache);
}

/// What a sample-for-sample comparison of a cached pass with its CPU twin found.
#[derive(Default)]
struct TwinCounts {
    compared: usize,
    fired: usize,
    fired_outliers: usize,
    kept_outliers: usize,
    worst: f32,
}

/// Runs `samples` single-sample cached passes over every tile and compares each cached pixel with
/// `PathCacheFrame::shade_sample(...).total()` (the CPU kept part plus the CPU completion), under
/// the file's per-sample tolerance. Also asserts the half buffer bit for bit against the sample
/// parity.
fn compare_with_cpu_twin(
    backend: &GpuBackend,
    fixture: &Fixture,
    frame: &PathCacheFrame<'_, '_>,
    cache: &GpuPathCache<'_, '_, '_>,
    samples: u32,
) -> TwinCounts {
    let live = LiveTiles::all(FRAME.0, FRAME.1, TILE);
    let mut counts = TwinCounts::default();
    for sample in 0..samples {
        let (outcome, accum, half_b) =
            cached_pass(backend, fixture, cache, &live, (sample, 1), Vec3::ZERO);
        assert_eq!(outcome, GpuAccumulate::Done);
        if sample.is_multiple_of(2) {
            assert!(
                half_b.iter().all(|p| *p == Vec3::ZERO),
                "half B must stay empty on the even sample {sample}"
            );
        } else {
            assert_eq!(
                bits(&half_b),
                bits(&accum),
                "half B is the sum on the odd sample {sample}"
            );
        }
        for y in 0..FRAME.1 {
            for x in 0..FRAME.0 {
                let rot = pixel_rotations(y * FRAME.0 + x);
                let Some(parts) = frame.shade_sample((x, y), sample, &rot) else {
                    continue;
                };
                let cpu = parts.total();
                let gpu = accum[(y * FRAME.0 + x) as usize];
                let error = (gpu - cpu).abs();
                let allowed =
                    cpu.abs().max(gpu.abs()) * SAMPLE_REL_TOL + Vec3::splat(SAMPLE_ABS_TOL);
                counts.worst = counts.worst.max((error / allowed).max_element());
                counts.compared += 1;
                counts.fired += usize::from(parts.traced);
                if error.cmpgt(allowed).any() {
                    if parts.traced {
                        counts.fired_outliers += 1;
                    } else {
                        counts.kept_outliers += 1;
                    }
                }
            }
        }
    }
    counts
}

/// Prints the counts of a twin run.
fn print_twin_counts(name: &str, counts: &TwinCounts) {
    let share = |part: usize, whole: usize| part as f64 / whole.max(1) as f64;
    let unfired = counts.compared - counts.fired;
    println!(
        "{name}: {} samples compared, {} fired ({:.1} %), {} fired outliers ({:.2} % of fired), {} unfired outliers ({:.2} % of unfired), worst error/tolerance {:.2}",
        counts.compared,
        counts.fired,
        100.0 * share(counts.fired, counts.compared),
        counts.fired_outliers,
        100.0 * share(counts.fired_outliers, counts.fired),
        counts.kept_outliers,
        100.0 * share(counts.kept_outliers, unfired),
        counts.worst
    );
}

/// The shared body of the completion twins: 16 single-sample passes at the coarse cutoff, GPU
/// completion (or, with `host_completion`, the host fallback) against the CPU shader.
fn run_twin(name: &str, fixture: &Fixture, host_completion: bool) {
    let Some(backend) = acquire_or_skip(name) else {
        return;
    };
    let scene = fixture.scene();
    let frame = cache_frame_with(&scene, &fixture.soa, options_with_cutoff(COARSE_CUTOFF));
    let mut cache = GpuPathCache::new(&frame, CacheTier::Vram);
    if host_completion {
        cache = cache.with_cpu_completion();
    }
    let counts = compare_with_cpu_twin(&backend, fixture, &frame, &cache, 16);
    print_twin_counts(name, &counts);
    if !host_completion {
        assert_eq!(
            completes_on_gpu(&backend, classify_material(&fixture.material)),
            Some(true),
            "the completion must be available wherever the live megakernel runs; check the device storage-buffer limit"
        );
    }
    assert!(
        counts.fired as f64 >= MIN_FIRED_SHARE * counts.compared as f64,
        "only {} of {} samples fired; raise the cutoff of the fixture",
        counts.fired,
        counts.compared
    );
    // With the host fallback the completion is the same CPU code on both sides.
    let outlier_share = if host_completion {
        SAMPLE_OUTLIER_SHARE
    } else {
        COMPLETION_OUTLIER_SHARE
    };
    assert!(
        counts.fired_outliers as f64 <= outlier_share * counts.fired as f64,
        "{} of {} fired samples are outside the twin tolerance",
        counts.fired_outliers,
        counts.fired
    );
    let unfired = counts.compared - counts.fired;
    assert!(
        counts.kept_outliers as f64 <= SAMPLE_OUTLIER_SHARE * unfired.max(1) as f64,
        "{} of {unfired} unfired samples are outside the twin tolerance",
        counts.kept_outliers
    );
    backend.release_path_cache(&cache);
}

/// The GPU completion is the CPU completion: every cached pixel of 16 samples at a coarse cutoff
/// (most samples fire) matches `PathCacheFrame::shade_sample` on a sapphire-like stone.
#[test]
fn the_gpu_completion_matches_the_cpu_completion_sample_for_sample() {
    run_twin(
        "the_gpu_completion_matches_the_cpu_completion_sample_for_sample",
        &Fixture::new(),
        false,
    );
}

/// The same twin on a moissanite-like stone, where every branch is a per-bin record.
#[test]
fn the_gpu_completion_matches_the_cpu_completion_on_a_dispersive_birefringent_stone() {
    let material = GemMaterial::new_custom(
        "cached gpu test moissanite-like",
        2.65,
        0.07,
        0.043,
        [0.0; 3],
    );
    assert!(
        dispersion_b_minus_g(&material) > DISPERSION_GATE_BG,
        "every branch must be a per-bin record"
    );
    run_twin(
        "the_gpu_completion_matches_the_cpu_completion_on_a_dispersive_birefringent_stone",
        &Fixture::with_material(material),
        false,
    );
}

/// The host completion fallback still equals the CPU shader (the same code on both sides).
#[test]
fn the_host_completion_fallback_still_matches_the_cpu_twin() {
    run_twin(
        "the_host_completion_fallback_still_matches_the_cpu_twin",
        &Fixture::new(),
        true,
    );
}

/// A GPU-completed pass depends neither on the order fired samples are appended in nor on the
/// slab layout: a streaming cache, a resident one and a second resident run give equal bits in the
/// sum and half B at a coarse cutoff.
#[test]
fn a_gpu_completed_pass_does_not_depend_on_the_fired_order_or_the_slab_layout() {
    let Some(backend) = acquire_or_skip(
        "a_gpu_completed_pass_does_not_depend_on_the_fired_order_or_the_slab_layout",
    ) else {
        return;
    };
    let fixture = Fixture::new();
    let scene = fixture.scene();
    let frame = cache_frame_with(&scene, &fixture.soa, options_with_cutoff(COARSE_CUTOFF));
    let live = LiveTiles::all(FRAME.0, FRAME.1, TILE);
    let pass = (5, 24);

    let streaming = GpuPathCache::new(&frame, CacheTier::SystemRam)
        .with_slab_budget_bytes(1)
        .with_stage_delay(Duration::from_millis(3));
    let (outcome, slow, slow_half) =
        cached_pass(&backend, &fixture, &streaming, &live, pass, Vec3::ZERO);
    assert_eq!(outcome, GpuAccumulate::Done);
    assert!(streaming.planned_slabs() > 1, "one slab per tile streams");

    let resident = GpuPathCache::new(&frame, CacheTier::Vram);
    let (outcome, fast, fast_half) =
        cached_pass(&backend, &fixture, &resident, &live, pass, Vec3::ZERO);
    assert_eq!(outcome, GpuAccumulate::Done);
    assert_eq!(bits(&slow), bits(&fast));
    assert_eq!(bits(&slow_half), bits(&fast_half));

    let (_, again, again_half) =
        cached_pass(&backend, &fixture, &resident, &live, pass, Vec3::ZERO);
    assert_eq!(bits(&fast), bits(&again));
    assert_eq!(bits(&fast_half), bits(&again_half));

    backend.release_path_cache(&streaming);
    backend.release_path_cache(&resident);
    assert_eq!(slabs_on_gpu(&backend), 0);
}
