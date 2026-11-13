//! The batch pipeline, the CPU engine and the hybrid entry
//! `cpu_frame::render_beam_hybrid_surface` (beam guide §12.4, §14.5, §16.3).
//!
//! The rows: full equivalence of the hybrid image with the plain reference render (the CPU engine,
//! glass, diamond and sapphire on the cube and the brilliant), the production tracer against the
//! reference (printed), the end clamp, memory caps, thread counts, cancellation, progress, ineligible
//! stones, engine failover and sharing, tile restriction and the hooks. Thresholds and sample counts
//! are named constants. The numbers a run produces are printed with `eprintln!` and asserted only
//! where the guide fixes them.

use std::{
    borrow::Cow,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use glam::Vec3;

use super::{
    BeamOff, BeamPhase, BeamProgress, BeamRun, BeamSettings, ResidualMode,
    engines::{
        Batch, BatchFragments, BatchResidual, BeamEngine, CpuBeamEngine, EngineOutcome, EngineSet,
    },
    frame::BeamFrame,
    leaf::node_count,
    leaf_cache::{LeafCache, RetainedBatch},
    pipeline::{
        HybridInput, ProbeOutcome, batch_share, combine_pixel, consumer_threads,
        gpu_producer_threads, probe_time_bound, probe_trees, probe_verdict, producer_threads,
        render_hybrid_tiles_with_threads, render_hybrid_with_threads, run_jobs, runs_for_share,
        runs_in_turns, shared_preparation, split_for_cap, tile_budget, tile_cost, tile_list,
        with_thread_limit,
    },
    primary::TileRect,
    residual::kept_memberships,
    stats::{BeamStats, TILE_ALLOWANCE},
    tests_support::{BeamRig, MAX_BOUNCES, diamond, glass, sapphire},
    tile_residual::ResidualScene,
};
use crate::{
    geometry::tool::StoneGeometry,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            LightingPreset, add_finite_sample, pixel_rotations, sample_draws,
            surface::SurfaceScene,
            transport::{PreparedMaterial, reference::trace_spectral_ray_reference_prepared},
        },
        tables::OpticsTables,
    },
    renderer::{
        accel_beam::{BEAM_MIN_TILE_BUDGET, BEAM_TREE_PROBE_MILLIS, BEAM_TREE_PROBE_TILES},
        adaptive::{AdaptiveImage, AdaptiveSettings, LiveTiles, NoiseMetric},
        cpu_frame::{render_adaptive_surface, render_beam_hybrid_surface},
        frame_scene::FrameScene,
    },
};

/// The side of the square test frame in pixels.
///
/// The rows check frame totals, bit identity across batchings and threads, hooks, progress and
/// failover, which need only a few tiles; a 32 pixel render costs four times as much.
const FRAME: u32 = 16;

/// The adaptive tile and the beam tile in pixels (4 tiles).
const TILE: u32 = 8;

/// The tiles of the 2 x 2 grid.
const TILE_COUNT: usize = 4;

/// The residual samples per pixel of the hybrid renders (the cap equals the minimum, so no tile
/// stops early and the image does not depend on how the tiles are batched).
const HYBRID_SPP: u32 = 32;

/// Reference samples per pixel of the plain stream the z-tests compare with.
const PLAIN_SPP: u32 = 64;

/// Samples per pixel of the production tracer's printed row.
const PRODUCTION_SPP: u32 = 32;

/// The first absolute sample index of the plain stream (independent of the residual's stream).
const PLAIN_FIRST_SAMPLE: u32 = 100_000;

/// The largest |z| the frame total may show.
const Z_LIMIT: f64 = 5.0;

/// The relative slack of the frame total for the beams' own checked tolerances (guide §12.3: about
/// 0.2 % of a branch in a boundary pixel, plus the spectral and weight tolerances).
const RELATIVE_SLACK: f64 = 0.03;

/// Milliseconds before the watcher thread of the cancel test sets the token.
const CANCEL_DELAY_MS: u64 = 40;

/// The generous bound on the time from the token being set to the entry returning (the guide's
/// figure is 100 ms; a loaded test machine gets ten times that).
const CANCEL_BOUND_MS: u64 = 1000;

/// A cap so high that the cancel test cannot finish before the token is set.
const ENDLESS_SPP: u32 = 1 << 20;

/// One tile per batch: four batches of the 4-tile frame.
const SMALL_BATCH: u32 = 1;

/// Settings whose tiles never converge: every pixel gets exactly `spp` samples, in one pass (the
/// pass is the residual minimum, `tile_residual::residual_settings`, and the fixtures' minimum is
/// `spp`).
const fn fixed_settings(spp: u32) -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise: 1.0e-6,
        metric: NoiseMetric::AbsoluteDeltaE,
        min_spp: spp,
        max_spp: spp,
        pass_spp: spp,
        tile: TILE,
    }
}

/// The beam settings of the tests with `batch_tiles` tiles per batch.
fn beam_settings(batch_tiles: u32) -> BeamSettings {
    BeamSettings {
        residual_min_spp: HYBRID_SPP,
        batch_tiles,
        ..BeamSettings::default()
    }
}

/// The three materials of the equivalence rows.
fn materials() -> [(&'static str, GemMaterial); 3] {
    [
        ("glass", glass()),
        ("diamond", diamond()),
        ("sapphire", sapphire()),
    ]
}

/// The extras of a plain stone.
fn plain_extras() -> (
    &'static [crate::geometry::tool::ToolPrimitive],
    &'static Fluorescence,
    SurfaceScene<'static>,
) {
    (&[], Fluorescence::none(), SurfaceScene::IDEAL)
}

/// An engine boxed for an `EngineSet`.
fn boxed<'e>(engine: impl BeamEngine + 'e) -> Box<dyn BeamEngine + 'e> {
    Box::new(engine)
}

/// A frame with its settings and the prepared material the engines borrow.
///
/// Counters and flags the test engines point at must be declared before the harness, so they
/// outlive the engines it builds.
struct Harness<'r> {
    /// The rig (the stone, its arena and camera).
    rig: &'r BeamRig,
    /// The frame.
    scene: &'r FrameScene<'r>,
    /// The prepared material of the residual.
    prepared: PreparedMaterial<'r>,
    /// The beam settings.
    beam: BeamSettings,
    /// The adaptive settings.
    adaptive: AdaptiveSettings,
}

impl<'r> Harness<'r> {
    /// A harness for `scene` with the settings of the tests: `batch_tiles` tiles per batch and
    /// `HYBRID_SPP` fixed residual samples.
    fn new(rig: &'r BeamRig, scene: &'r FrameScene<'r>, batch_tiles: u32) -> Self {
        Self {
            rig,
            scene,
            prepared: PreparedMaterial::new(&rig.material),
            beam: beam_settings(batch_tiles),
            adaptive: fixed_settings(HYBRID_SPP),
        }
    }

    /// A CPU engine of the frame on `threads` workers.
    fn engine(&self, threads: usize) -> CpuBeamEngine<'_> {
        let residual = ResidualScene {
            frame: self.scene,
            extras: plain_extras(),
            plane_soa: &self.rig.soa,
            prepared: &self.prepared,
            n_lambda: node_count(&self.beam),
            sample_offset: 0,
            residual_min_spp: self.beam.residual_min_spp,
        };
        CpuBeamEngine::new(&residual, &self.beam, &self.adaptive).with_threads(threads)
    }

    /// The entry under test with `engines`, never cancelled, without hooks.
    fn render(&self, engines: &EngineSet<'_>) -> Result<(AdaptiveImage, BeamStats), BeamOff> {
        render_plain(self.rig, self.scene, (&self.beam, &self.adaptive), engines)
    }
}

/// What a render with hooks returned.
struct Rendered {
    /// The image.
    image: AdaptiveImage,
    /// The counters.
    stats: BeamStats,
    /// Every progress report, in order.
    progress: Vec<BeamProgress>,
    /// Every `on_batch` call: the tiles and their final pixels.
    batches: Vec<(Vec<TileRect>, Vec<Vec3>)>,
    /// Every retained batch.
    retained: Vec<RetainedBatch>,
}

/// The entry under test, with the progress, `on_batch` and `retain` hooks recording.
fn render_hooked(
    rig: &BeamRig,
    scene: &FrameScene<'_>,
    settings: (&BeamSettings, &AdaptiveSettings),
    engines: &EngineSet<'_>,
    restrict: Option<&LiveTiles>,
    cancel: &AtomicBool,
) -> Result<Rendered, BeamOff> {
    let (beam, adaptive) = settings;
    let mut progress_log = Vec::new();
    let mut batches = Vec::new();
    let mut retained = Vec::new();
    let result = {
        let mut progress = |report: BeamProgress| progress_log.push(report);
        let mut on_batch =
            |tiles: &[TileRect], means: &[Vec3]| batches.push((tiles.to_vec(), means.to_vec()));
        let mut retain = |batch: RetainedBatch| retained.push(batch);
        render_beam_hybrid_surface(
            scene,
            plain_extras(),
            &rig.soa,
            beam,
            adaptive,
            engines,
            (0, 1.0),
            cancel,
            BeamRun {
                tiles: restrict,
                progress: &mut progress,
                on_batch: Some(&mut on_batch),
                retain: Some(&mut retain),
                preloaded: None,
            },
        )
    };
    let (image, stats) = result?;
    Ok(Rendered {
        image,
        stats,
        progress: progress_log,
        batches,
        retained,
    })
}

/// The entry under test without hooks, never cancelled.
fn render_plain(
    rig: &BeamRig,
    scene: &FrameScene<'_>,
    settings: (&BeamSettings, &AdaptiveSettings),
    engines: &EngineSet<'_>,
) -> Result<(AdaptiveImage, BeamStats), BeamOff> {
    let (beam, adaptive) = settings;
    let cancel = AtomicBool::new(false);
    let mut progress = |_: BeamProgress| {};
    render_beam_hybrid_surface(
        scene,
        plain_extras(),
        &rig.soa,
        beam,
        adaptive,
        engines,
        (0, 1.0),
        &cancel,
        BeamRun::new(&mut progress),
    )
}

/// The bit patterns of a vector image.
fn vec_bits(values: &[Vec3]) -> Vec<[u32; 3]> {
    values
        .iter()
        .map(|v| v.to_array().map(f32::to_bits))
        .collect()
}

/// An image as comparable bits: sums, odd sums, counts, pixels and the sample total.
type ImageBits = (Vec<[u32; 3]>, Vec<[u32; 3]>, Vec<u32>, Vec<[u32; 3]>, u64);

/// The bits of every field of `image`.
fn image_bits(image: &AdaptiveImage) -> ImageBits {
    (
        vec_bits(&image.sum),
        vec_bits(&image.half_b),
        image.counts.clone(),
        vec_bits(&image.pixels),
        image.samples_traced,
    )
}

/// The pixels of the row-major frame buffer `buffer` in batch order.
fn gather(buffer: &[Vec3], tiles: &[TileRect]) -> Vec<Vec3> {
    let mut out = Vec::new();
    for tile in tiles {
        for y in tile.y0..tile.y1 {
            for x in tile.x0..tile.x1 {
                out.push(buffer[(y * FRAME + x) as usize]);
            }
        }
    }
    out
}

/// The baseline image: the CPU engine alone, batches of `SMALL_BATCH` tiles.
fn baseline(rig: &BeamRig, scene: &FrameScene<'_>) -> AdaptiveImage {
    let harness = Harness::new(rig, scene, SMALL_BATCH);
    let engines = EngineSet::new(vec![boxed(harness.engine(2))]);
    harness.render(&engines).expect("an eligible stone").0
}

// ---- full equivalence, production vs reference, the clamp ----

/// One sample of the plain reference tracer (`kept = None`) of pixel `(x, y)`.
fn reference_sample(
    rig: &BeamRig,
    scene: &FrameScene<'_>,
    prepared: &PreparedMaterial<'_>,
    pixel: (u32, u32),
    sample_num: u32,
) -> Vec3 {
    let rot = pixel_rotations(pixel.1 * FRAME + pixel.0);
    let draws = sample_draws(pixel.1 * FRAME + pixel.0, sample_num, &rot);
    let ray = rig.camera.generate_ray(
        pixel.0 as f32,
        pixel.1 as f32,
        FRAME as f32,
        FRAME as f32,
        draws.jitter_x,
        draws.jitter_y,
    );
    trace_spectral_ray_reference_prepared(
        ray,
        StoneGeometry::planes_only(&rig.planes),
        &rig.soa,
        &[],
        prepared,
        Fluorescence::none(),
        MAX_BOUNCES,
        scene.environment,
        draws.seed,
        draws.hero_rand,
        None,
        SurfaceScene::IDEAL,
        None,
    )
    .xyz
}

/// The mean and the standard error of the mean of `values`.
fn mean_and_se(values: &[f64]) -> (f64, f64) {
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let variance = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / (n - 1.0);
    (mean, (variance / n).sqrt())
}

/// The frame total of Y of the plain reference tracer and its standard error: per sample index the
/// sum of Y over the frame, then the mean over `PLAIN_SPP` indices.
fn plain_total(rig: &BeamRig, scene: &FrameScene<'_>) -> (f64, f64) {
    let prepared = PreparedMaterial::new(&rig.material);
    let mut aggregate = vec![0.0_f64; PLAIN_SPP as usize];
    for y in 0..FRAME {
        for x in 0..FRAME {
            for i in 0..PLAIN_SPP {
                let sample =
                    reference_sample(rig, scene, &prepared, (x, y), PLAIN_FIRST_SAMPLE + i);
                let mut kept = Vec3::ZERO;
                add_finite_sample(&mut kept, sample);
                aggregate[i as usize] += f64::from(kept.y);
            }
        }
    }
    mean_and_se(&aggregate)
}

/// The frame total of Y of the final pixels of `image`.
fn image_total(image: &AdaptiveImage) -> f64 {
    image.pixels.iter().map(|p| f64::from(p.y)).sum()
}

/// The variance of the frame total's residual part from the half-buffer split (`E[((A - B) / 2)^2]`
/// is the variance of a pixel's mean); the beam part is in both halves and cancels.
fn residual_variance(image: &AdaptiveImage) -> f64 {
    let mut variance = 0.0_f64;
    for index in 0..image.pixels.len() {
        let count = image.counts[index];
        let odd = count / 2;
        let even = count - odd;
        if odd == 0 || even == 0 {
            continue;
        }
        let mean_a = (image.sum[index] - image.half_b[index]) / even as f32;
        let mean_b = image.half_b[index] / odd as f32;
        let half_gap = f64::from(mean_a.y - mean_b.y) / 2.0;
        variance = half_gap.mul_add(half_gap, variance);
    }
    variance
}

/// The hybrid image of `rig` through the CPU engine and its comparison with the plain reference.
fn check_equivalence(rig: &BeamRig, scene: &FrameScene<'_>, label: &str) -> AdaptiveImage {
    let image = baseline(rig, scene);
    let (plain, plain_se) = plain_total(rig, scene);
    let total = image_total(&image);
    let spread = residual_variance(&image).sqrt().hypot(plain_se);
    let difference = total - plain;
    eprintln!(
        "{label}: hybrid Y {total:.4}, plain reference Y {plain:.4} +- {plain_se:.4}, residual \
         spread {:.4}, difference {difference:+.4} ({:+.3} %)",
        residual_variance(&image).sqrt(),
        100.0 * difference / plain.max(1.0e-12)
    );
    assert!(plain > 0.0, "{label}: the reference image is empty");
    assert!(
        difference.abs() <= RELATIVE_SLACK.mul_add(plain.abs(), Z_LIMIT * spread),
        "{label}: hybrid {total} against the plain reference {plain} (spread {spread})"
    );
    image
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn the_hybrid_image_matches_the_plain_reference_on_the_cube() {
    for (name, material) in materials() {
        let rig = BeamRig::cube(material);
        let scene = rig.scene(FRAME, MAX_BOUNCES);
        check_equivalence(&rig, &scene, &format!("cube {name}"));
    }
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn the_hybrid_image_matches_the_plain_reference_on_the_brilliant() {
    for (name, material) in materials() {
        let rig = BeamRig::brilliant(material);
        let scene = rig.scene(FRAME, MAX_BOUNCES);
        check_equivalence(&rig, &scene, &format!("brilliant {name}"));
    }
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn the_production_tracer_is_printed_against_the_reference() {
    // The production tracer's own approximation (guide §5.5): reported, not counted against beams.
    for (name, material) in [("glass", glass()), ("diamond", diamond())] {
        let rig = BeamRig::cube(material);
        let scene = rig.scene(FRAME, MAX_BOUNCES);
        let production = render_adaptive_surface(
            &scene,
            plain_extras(),
            &rig.soa,
            &fixed_settings(PRODUCTION_SPP),
            0,
            1.0,
        )
        .expect("valid settings");
        let (plain, plain_se) = plain_total(&rig, &scene);
        let hybrid = image_total(&baseline(&rig, &scene));
        let production_total = image_total(&production);
        eprintln!(
            "cube {name}: production Y {production_total:.4}, reference Y {plain:.4} +- \
             {plain_se:.4}, hybrid Y {hybrid:.4}, production - reference {:+.3} %, hybrid - \
             reference {:+.3} %",
            100.0 * (production_total - plain) / plain.max(1.0e-12),
            100.0 * (hybrid - plain) / plain.max(1.0e-12)
        );
        assert!(production_total.is_finite() && plain.is_finite() && hybrid.is_finite());
    }
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn the_end_clamp_holds_and_the_clamp_row_is_reported_for_a_white_diamond() {
    // A white dispersive stone under the warmest Planckian preset (guide §16.3 clamp row).
    let rig = BeamRig::brilliant(diamond());
    let scene = FrameScene {
        environment: LightingPreset::IlluminantA.studio(1.0, 0.4, 0.35),
        ..rig.scene(FRAME, MAX_BOUNCES)
    };
    let image = baseline(&rig, &scene);
    let (plain, plain_se) = plain_total(&rig, &scene);
    let total = image_total(&image);
    let negative_before_clamp = (0..image.pixels.len())
        .filter(|&i| {
            let unclamped = image.sum[i] / image.counts[i].max(1) as f32;
            unclamped.min_element() < 0.0
        })
        .count();
    eprintln!(
        "brilliant diamond, illuminant A: hybrid Y {total:.4}, reference Y {plain:.4} +- \
         {plain_se:.4}, difference {:+.3} %, pixels negative before the end clamp \
         {negative_before_clamp}",
        100.0 * (total - plain) / plain.max(1.0e-12)
    );
    assert!(image.pixels.iter().all(|p| p.is_finite()));
    assert!(
        image.pixels.iter().all(|p| p.min_element() >= 0.0),
        "a pixel is negative after the end clamp"
    );
}

// ---- memory, batching, threads ----

/// The memory estimate of each tile of a render with `retain`, in tile order.
fn tile_costs(rendered: &Rendered) -> Vec<u64> {
    rendered
        .retained
        .iter()
        .flat_map(|batch| {
            batch
                .tiles
                .iter()
                .zip(&batch.leaves)
                .map(|(&rect, (_, leaves))| tile_cost(rect, leaves))
        })
        .collect()
}

/// A memory cap whose per-batch share is exactly the widest tile of `reference` (a roomy render):
/// no tile is over the share, and the widest tile joins no other.
fn splitting_cap(reference: &Rendered) -> u64 {
    let widest = tile_costs(reference).into_iter().max().expect("some tile");
    widest * 2
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_cap_that_splits_the_batches_by_the_estimate_gives_the_same_image() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let adaptive = fixed_settings(HYBRID_SPP);
    let cancel = AtomicBool::new(false);
    let engines = EngineSet::new(Vec::new());
    let roomy = beam_settings(64);
    let whole = render_hooked(&rig, &scene, (&roomy, &adaptive), &engines, None, &cancel)
        .expect("an eligible stone");
    let cap = splitting_cap(&whole);
    let tight = BeamSettings {
        memory_cap_bytes: cap,
        ..roomy
    };
    let split = render_hooked(&rig, &scene, (&tight, &adaptive), &engines, None, &cancel)
        .expect("an eligible stone");
    eprintln!(
        "memory row: {} piece(s) under the default cap, {} under a cap of {cap} bytes",
        whole.batches.len(),
        split.batches.len()
    );
    assert_eq!(whole.batches.len(), 1);
    assert_eq!(whole.batches[0].0.len(), TILE_COUNT);
    // The widest tile is alone, so there is more than one batch, and every batch is what the
    // estimates in tile order give.
    assert!(split.batches.len() > 1);
    let costs = tile_costs(&whole);
    let lengths: Vec<usize> = split.batches.iter().map(|(tiles, _)| tiles.len()).collect();
    let expected: Vec<usize> = runs_for_share(&costs, batch_share(cap))
        .iter()
        .map(ExactSizeIterator::len)
        .collect();
    assert_eq!(lengths, expected);
    assert_eq!(image_bits(&whole.image), image_bits(&split.image));
    assert_eq!(whole.retained.len(), 1);
    assert_eq!(split.retained.len(), split.batches.len());
}

#[test]
fn a_tiny_cap_still_renders_with_the_residual_covering_every_tile() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let adaptive = fixed_settings(HYBRID_SPP);
    let cancel = AtomicBool::new(false);
    let engines = EngineSet::new(Vec::new());
    let tiny = BeamSettings {
        memory_cap_bytes: 1,
        ..beam_settings(64)
    };
    let rendered = render_hooked(&rig, &scene, (&tiny, &adaptive), &engines, None, &cancel)
        .expect("an eligible stone");
    // Every tile is over the share: residual-only, a batch of its own, never retained, and no
    // fragment was shaded for it.
    assert_eq!(rendered.batches.len(), TILE_COUNT);
    assert!(rendered.batches.iter().all(|(tiles, _)| tiles.len() == 1));
    assert_eq!(rendered.retained.len(), 0);
    assert_eq!(rendered.stats.fragments, 0);
    assert!(rendered.image.counts.iter().all(|&c| c == HYBRID_SPP));
    assert!(rendered.image.pixels.iter().all(|p| p.is_finite()));
    assert!(
        rendered.image.pixels.iter().any(|p| p.y > 0.0),
        "the residual alone must still light the stone"
    );
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn the_estimate_counts_the_kept_indexs_memberships_counts_and_cursors() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let adaptive = fixed_settings(HYBRID_SPP);
    let cancel = AtomicBool::new(false);
    let engines = EngineSet::new(Vec::new());
    let rendered = render_hooked(
        &rig,
        &scene,
        (&beam_settings(64), &adaptive),
        &engines,
        None,
        &cancel,
    )
    .expect("an eligible stone");
    let batch = rendered.retained.first().expect("a retained batch");
    let mut seen = 0_u64;
    for ((&rect, (_, leaves)), (_, index)) in batch.tiles.iter().zip(&batch.leaves).zip(&batch.kept)
    {
        let memberships = kept_memberships(rect, leaves);
        seen += memberships;
        let pixels = u64::try_from(rect.pixel_count()).expect("small");
        // The built index holds at least the memberships and the offsets the estimate counts.
        let regions =
            u64::try_from(index.len() * size_of::<super::beam::KeptRegion>()).expect("small");
        assert!(index.heap_bytes() >= regions + (pixels + 1) * 4 + memberships * 4);
        // The estimate adds the memberships, the three per-pixel tables and the leaves.
        assert!(tile_cost(rect, leaves) >= pixels * 12 + memberships * 4 + regions);
        // Twice the leaves: twice the memberships.
        let doubled: Vec<_> = leaves.iter().chain(leaves).cloned().collect();
        assert_eq!(kept_memberships(rect, &doubled), 2 * memberships);
    }
    assert!(seen > 0, "a cube in view overlaps its leaves' pixel boxes");
    // No leaves: the three per-pixel tables alone.
    let rect = batch.tiles[0];
    assert_eq!(kept_memberships(rect, &[]), 0);
}

#[test]
fn the_tree_and_the_engine_share_one_thread_quota() {
    for (quota, producer, consumer) in [(0, 1, 1), (1, 1, 1), (2, 1, 1), (7, 4, 3), (8, 4, 4)] {
        assert_eq!(producer_threads(quota), producer, "producer of {quota}");
        assert_eq!(consumer_threads(quota), consumer, "consumer of {quota}");
    }
    for quota in 2..40 {
        assert_eq!(producer_threads(quota) + consumer_threads(quota), quota);
    }
}

#[test]
fn without_a_cpu_engine_the_producer_takes_all_but_one_thread() {
    for (quota, producer) in [(0, 1), (1, 1), (2, 2), (3, 2), (8, 7)] {
        assert_eq!(
            gpu_producer_threads(quota),
            producer,
            "producer of {quota} without a CPU engine"
        );
    }
    // The CPU engine says it shares the quota; an engine that does not know the question does not.
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let harness = Harness::new(&rig, &scene, SMALL_BATCH);
    assert!(harness.engine(1).shares_cpu_threads());
    assert!(!Tiny.shares_cpu_threads());
}

/// An engine that answers nothing and leaves every trait default alone.
struct Tiny;

impl BeamEngine for Tiny {
    fn name(&self) -> &'static str {
        "tiny"
    }

    fn fragments(&self, _: &Batch<'_>, _: &AtomicBool) -> EngineOutcome<BatchFragments> {
        EngineOutcome::Declined("tiny")
    }

    fn residual(&self, _: &Batch<'_>, _: &[Vec3], _: &AtomicBool) -> EngineOutcome<BatchResidual> {
        EngineOutcome::Declined("tiny")
    }
}

#[test]
fn a_residual_that_needs_the_whole_cap_abandons_the_beam_render() {
    // The tiles never converge, so every stone pixel gets the cap (32 samples at a minimum of 4,
    // a ratio of 8): the residual share of the first stone batch is 1, over the limit.
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let beam = BeamSettings {
        residual_min_spp: 4,
        ..beam_settings(4)
    };
    let adaptive = AdaptiveSettings {
        target_noise: 1.0e-6,
        min_spp: 4,
        max_spp: 32,
        pass_spp: 4,
        ..fixed_settings(HYBRID_SPP)
    };
    let engines = EngineSet::new(Vec::new());
    let result = render_plain(&rig, &scene, (&beam, &adaptive), &engines);
    assert_eq!(result.err(), Some(BeamOff::ResidualTooLarge));
}

#[test]
fn the_tile_budget_follows_the_plain_render_between_its_bounds() {
    assert_eq!(tile_budget(16, 8192), 4096);
    assert_eq!(tile_budget(16, 1 << 20), 131_072);
    assert_eq!(tile_budget(16, 1 << 30), TILE_ALLOWANCE);
    assert_eq!(tile_budget(16, 32), BEAM_MIN_TILE_BUDGET);
}

/// The tile edge of the probe rows: 2 pixels, so the cube covers well over eight tiles.
const PROBE_TILE: u32 = 2;

#[test]
fn the_tree_rule_is_certain_once_half_spent_or_more_than_half_did_not() {
    let all = BEAM_TREE_PROBE_TILES;
    // Fewer stone tiles than the rule reads: never a decline, whatever they spent.
    for stone in 0..all {
        assert_eq!(probe_verdict(stone, 0, stone), Some(false), "{stone} tiles");
    }
    // Under half spent: undecided until more than half finished within the budget.
    for spent in 0..all / 2 {
        for unspent in 0..=(all - spent) {
            let expected = if (all - unspent) * 2 < all {
                Some(false)
            } else {
                None
            };
            assert_eq!(
                probe_verdict(spent, unspent, all),
                expected,
                "{spent} spent, {unspent} not"
            );
        }
    }
    // Half spent or more: a decline, whatever the rest does.
    for spent in all / 2..=all {
        for unspent in 0..=(all - spent) {
            assert_eq!(probe_verdict(spent, unspent, all), Some(true));
        }
    }
    // With all eight tiles counted, half or more spent declines the probe.
    assert_eq!(probe_verdict(4, 4, 8), Some(true));
    assert_eq!(probe_verdict(3, 5, 8), Some(false));
    assert_eq!(probe_verdict(3, 4, 8), None);
}

#[test]
fn the_probe_time_bound_follows_the_budget_and_the_waves() {
    let base = Duration::from_millis(BEAM_TREE_PROBE_MILLIS);
    assert_eq!(probe_time_bound(BEAM_MIN_TILE_BUDGET, 8), base);
    assert_eq!(probe_time_bound(1, 64), base);
    assert_eq!(probe_time_bound(2 * BEAM_MIN_TILE_BUDGET, 8), 2 * base);
    assert_eq!(probe_time_bound(BEAM_MIN_TILE_BUDGET, 4), 2 * base);
    assert_eq!(probe_time_bound(BEAM_MIN_TILE_BUDGET, 3), 3 * base);
    assert_eq!(probe_time_bound(BEAM_MIN_TILE_BUDGET, 0), 8 * base);
}

#[test]
fn the_probe_declines_on_spent_tiles_and_passes_on_cheap_ones() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let settings = beam_settings(4);
    let tiles = tile_list((FRAME, FRAME), PROBE_TILE, None);
    let cancel = AtomicBool::new(false);
    // A budget of one unit: every stone tile spends it with its first beam.
    let spent = BeamFrame::new(&scene, &settings)
        .expect("an eligible stone")
        .with_tile_allowance(1);
    // The full allowance: no tile of the cube comes near it.
    let cheap = BeamFrame::new(&scene, &settings)
        .expect("an eligible stone")
        .with_tile_allowance(TILE_ALLOWANCE);
    let stone = (0..tiles.len())
        .filter(|&index| cheap.tile_has_primary(index, PROBE_TILE))
        .count();
    assert!(
        stone >= usize::try_from(BEAM_TREE_PROBE_TILES).expect("a small count"),
        "the fixture needs eight stone tiles, it has {stone}"
    );
    assert!(!cheap.tile_has_primary(tiles.len(), PROBE_TILE));
    // The verdicts do not depend on the thread count.
    for threads in [1, 4] {
        assert_eq!(
            probe_trees(&spent, PROBE_TILE, &tiles, threads, None, &cancel),
            ProbeOutcome::Declined,
            "{threads} thread(s)"
        );
        assert_eq!(
            probe_trees(&cheap, PROBE_TILE, &tiles, threads, None, &cancel),
            ProbeOutcome::Go,
            "{threads} thread(s)"
        );
    }
    // The 8 pixel grid has four tiles, fewer than the rule reads: nothing is traced, it passes.
    let coarse = tile_list((FRAME, FRAME), TILE, None);
    assert_eq!(
        probe_trees(&spent, TILE, &coarse, 4, None, &cancel),
        ProbeOutcome::Go
    );
    // A time bound that has passed before the first tree ends the probe undecided.
    assert_eq!(
        probe_trees(&cheap, PROBE_TILE, &tiles, 1, Some(Duration::ZERO), &cancel),
        ProbeOutcome::TimedOut
    );
    // A set token wins over everything.
    let cancelled = AtomicBool::new(true);
    assert_eq!(
        probe_trees(&spent, PROBE_TILE, &tiles, 4, None, &cancelled),
        ProbeOutcome::Cancelled
    );
}

#[test]
fn a_budgeted_render_whose_probe_passes_goes_on_to_the_residual_rule() {
    // 32 pixels: at least eight of the sixteen 8 pixel tiles hold the cube, so the probe stage of
    // the entry traces eight of them. Their trees are far below the budget: the probe passes and
    // the render reaches the residual rule, which declines as in the 16 pixel row
    // (`a_residual_that_needs_the_whole_cap_abandons_the_beam_render`).
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(2 * FRAME, MAX_BOUNCES);
    let beam = BeamSettings {
        residual_min_spp: 4,
        ..beam_settings(4)
    };
    let adaptive = AdaptiveSettings {
        target_noise: 1.0e-6,
        min_spp: 4,
        max_spp: 32,
        pass_spp: 4,
        ..fixed_settings(HYBRID_SPP)
    };
    let frame = BeamFrame::new(&scene, &beam).expect("an eligible stone");
    let tiles = tile_list((2 * FRAME, 2 * FRAME), TILE, None);
    let stone = (0..tiles.len())
        .filter(|&index| frame.tile_has_primary(index, TILE))
        .count();
    assert!(
        stone >= usize::try_from(BEAM_TREE_PROBE_TILES).expect("a small count"),
        "the fixture needs eight stone tiles, it has {stone}"
    );
    let engines = EngineSet::new(Vec::new());
    let result = render_plain(&rig, &scene, (&beam, &adaptive), &engines);
    assert_eq!(result.err(), Some(BeamOff::ResidualTooLarge));
}

#[test]
fn a_quota_of_one_makes_the_trees_and_the_shading_take_turns() {
    assert!(runs_in_turns(0));
    assert!(runs_in_turns(1));
    assert!(!runs_in_turns(2));
    // The turn-taking producer waits for each batch: the image and the batches are those of the
    // overlapping pipeline (results do not depend on the quota).
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let adaptive = fixed_settings(HYBRID_SPP);
    let cancel = AtomicBool::new(false);
    let engines = EngineSet::new(Vec::new());
    let beam = beam_settings(2);
    let overlapped = render_hooked(&rig, &scene, (&beam, &adaptive), &engines, None, &cancel)
        .expect("an eligible stone");
    let serial = with_thread_limit(1, || {
        render_hooked(&rig, &scene, (&beam, &adaptive), &engines, None, &cancel)
    })
    .expect("an eligible stone");
    assert_eq!(image_bits(&overlapped.image), image_bits(&serial.image));
    assert_eq!(overlapped.batches.len(), serial.batches.len());
}

#[test]
fn the_batch_rule_joins_tiles_while_the_running_estimate_fits_the_share() {
    assert_eq!(
        runs_for_share(&[], 10),
        Vec::<std::ops::Range<usize>>::new()
    );
    assert_eq!(runs_for_share(&[10, 10, 10, 10], 100), vec![0..4]);
    assert_eq!(runs_for_share(&[10, 10, 10, 10], 25), vec![0..2, 2..4]);
    assert_eq!(
        runs_for_share(&[10, 10, 10, 10], 5),
        vec![0..1, 1..2, 2..3, 3..4]
    );
    // A tile that does not fit starts the next batch even when a later one would have fit.
    assert_eq!(runs_for_share(&[6, 6, 1], 10), vec![0..1, 1..3]);
    assert_eq!(batch_share(0), 1);
    assert_eq!(batch_share(1), 1);
    assert_eq!(batch_share(9), 4);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn batches_of_any_size_and_no_engine_give_the_same_image_with_fixed_samples() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let adaptive = fixed_settings(HYBRID_SPP);
    let engines = EngineSet::new(Vec::new());
    let reference = baseline(&rig, &scene);
    for batch_tiles in [1, 3, 5, 16, 64] {
        let beam = beam_settings(batch_tiles);
        let (image, _) =
            render_plain(&rig, &scene, (&beam, &adaptive), &engines).expect("an eligible stone");
        assert_eq!(
            image_bits(&image),
            image_bits(&reference),
            "batches of {batch_tiles} tiles"
        );
    }
}

/// The image of `rig` with tree workers and the CPU engine both on `threads` threads.
fn render_on_threads(rig: &BeamRig, scene: &FrameScene<'_>, threads: usize) -> AdaptiveImage {
    let harness = Harness::new(rig, scene, SMALL_BATCH);
    let engines = EngineSet::new(vec![boxed(harness.engine(threads))]);
    let cancel = AtomicBool::new(false);
    let mut progress = |_: BeamProgress| {};
    let input = HybridInput {
        scene: harness.scene,
        extras: plain_extras(),
        plane_soa: &rig.soa,
        beam: &harness.beam,
        adaptive: &harness.adaptive,
        engines: &engines,
        sample_offset: 0,
        scale: 1.0,
    };
    render_hybrid_with_threads(&input, threads, &cancel, BeamRun::new(&mut progress))
        .expect("an eligible stone")
        .0
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn one_and_eight_threads_are_bit_identical() {
    for rig in [BeamRig::cube(glass()), BeamRig::brilliant(diamond())] {
        let scene = rig.scene(FRAME, MAX_BOUNCES);
        let one = render_on_threads(&rig, &scene, 1);
        let eight = render_on_threads(&rig, &scene, 8);
        let again = render_on_threads(&rig, &scene, 8);
        assert!(
            one.pixels.iter().any(|p| p.y > 0.0),
            "an empty image proves nothing"
        );
        assert_eq!(image_bits(&one), image_bits(&eight));
        assert_eq!(image_bits(&eight), image_bits(&again));
    }
}

// ---- the beam part, statistics and the hooks ----

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn without_a_residual_the_image_is_the_clamped_beam_part_of_the_frame() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let beam = BeamSettings {
        residual: ResidualMode::Off,
        ..beam_settings(SMALL_BATCH)
    };
    let adaptive = fixed_settings(HYBRID_SPP);
    let (image, stats) = render_plain(
        &rig,
        &scene,
        (&beam, &adaptive),
        &EngineSet::new(Vec::new()),
    )
    .expect("an eligible stone");
    let frame = BeamFrame::new(&scene, &beam).expect("eligible");
    let cancel = AtomicBool::new(false);
    let tree = frame
        .render_beams_with_threads(TILE, 2, &cancel)
        .expect("not cancelled");
    assert!(tree.stats.leaves > 0, "no leaves: nothing is compared");
    assert!(image.counts.iter().all(|&c| c == 0));
    assert_eq!(image.samples_traced, 0);
    for (index, pixel) in image.pixels.iter().enumerate() {
        assert_eq!(*pixel, tree.xyz[index].max(Vec3::ZERO), "pixel {index}");
    }
    assert_eq!(stats.primary_beams, tree.stats.primary_beams);
    assert_eq!(stats.beams, tree.stats.beams);
    assert_eq!(stats.leaves, tree.stats.leaves);
    assert_eq!(stats.drops, tree.stats.drops);
    assert_eq!(stats.fragments, tree.stats.fragments);
    assert_eq!(stats.fragment_pieces, tree.stats.fragment_pieces);
    assert!(stats.fragments > 0);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn the_hooks_see_every_piece_with_its_final_pixels_and_its_leaves() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let beam = beam_settings(SMALL_BATCH);
    let adaptive = fixed_settings(HYBRID_SPP);
    let cancel = AtomicBool::new(false);
    let rendered = render_hooked(
        &rig,
        &scene,
        (&beam, &adaptive),
        &EngineSet::new(Vec::new()),
        None,
        &cancel,
    )
    .expect("an eligible stone");
    assert_eq!(rendered.batches.len(), TILE_COUNT / SMALL_BATCH as usize);
    assert_eq!(rendered.retained.len(), rendered.batches.len());
    let columns = FRAME / TILE;
    let mut seen = 0_usize;
    for ((tiles, means), retained) in rendered.batches.iter().zip(&rendered.retained) {
        assert_eq!(
            vec_bits(means),
            vec_bits(&gather(&rendered.image.pixels, tiles))
        );
        assert_eq!(&retained.tiles, tiles);
        assert_eq!(retained.leaves.len(), tiles.len());
        assert_eq!(retained.kept.len(), tiles.len());
        for (position, tile) in tiles.iter().enumerate() {
            let index = (tile.y0 / TILE) * columns + tile.x0 / TILE;
            assert_eq!(retained.leaves[position].0, index);
            assert_eq!(retained.kept[position].0, *tile);
            assert!(retained.leaves[position].1.iter().all(|l| l.tile == index));
        }
        seen += tiles.len();
    }
    assert_eq!(seen, TILE_COUNT);
    let leaves: usize = rendered
        .retained
        .iter()
        .flat_map(|batch| batch.leaves.iter())
        .map(|(_, leaves)| leaves.len())
        .sum();
    assert_eq!(leaves as u64, rendered.stats.leaves);
    assert!(rendered.stats.fragments > 0);
    assert!(rendered.image.samples_traced > 0);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_tile_restricted_run_equals_the_full_run_on_its_tiles_and_is_zero_elsewhere() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let beam = beam_settings(2);
    let adaptive = fixed_settings(HYBRID_SPP);
    let cancel = AtomicBool::new(false);
    let engines = EngineSet::new(Vec::new());
    let members = [1_u32, 2];
    let live = LiveTiles::from_indices(FRAME, FRAME, TILE, members.to_vec());
    let restricted = render_hooked(
        &rig,
        &scene,
        (&beam, &adaptive),
        &engines,
        Some(&live),
        &cancel,
    )
    .expect("an eligible stone");
    let full = render_hooked(&rig, &scene, (&beam, &adaptive), &engines, None, &cancel)
        .expect("an eligible stone");
    let mut inside = 0_usize;
    for y in 0..FRAME {
        for x in 0..FRAME {
            let index = (y * FRAME + x) as usize;
            let tile = (y / TILE) * (FRAME / TILE) + x / TILE;
            if members.contains(&tile) {
                inside += 1;
                assert_eq!(restricted.image.pixels[index], full.image.pixels[index]);
                assert_eq!(restricted.image.counts[index], full.image.counts[index]);
                assert_eq!(restricted.image.sum[index], full.image.sum[index]);
            } else {
                assert_eq!(restricted.image.pixels[index], Vec3::ZERO);
                assert_eq!(restricted.image.counts[index], 0);
                assert_eq!(restricted.image.sum[index], Vec3::ZERO);
            }
        }
    }
    assert_eq!(inside, members.len() * (TILE * TILE) as usize);
    let rendered_tiles: usize = restricted
        .batches
        .iter()
        .map(|(tiles, _)| tiles.len())
        .sum();
    assert_eq!(rendered_tiles, members.len());
    assert_eq!(
        restricted.image.samples_traced,
        inside as u64 * u64::from(HYBRID_SPP)
    );
    // A restriction built for another frame leaves nothing to render.
    let foreign = LiveTiles::from_indices(32, 32, TILE, vec![0, 1]);
    let nothing = render_hooked(
        &rig,
        &scene,
        (&beam, &adaptive),
        &engines,
        Some(&foreign),
        &cancel,
    )
    .expect("an eligible stone");
    assert!(nothing.image.counts.iter().all(|&c| c == 0));
    assert_eq!(nothing.batches.len(), 0);
}

// ---- cancel and progress ----

#[test]
fn a_token_set_before_the_call_returns_at_once_with_nothing_rendered() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let beam = beam_settings(SMALL_BATCH);
    let adaptive = fixed_settings(HYBRID_SPP);
    let cancel = AtomicBool::new(true);
    let rendered = render_hooked(
        &rig,
        &scene,
        (&beam, &adaptive),
        &EngineSet::new(Vec::new()),
        None,
        &cancel,
    )
    .expect("an eligible stone");
    assert!(rendered.image.counts.iter().all(|&c| c == 0));
    assert!(rendered.image.pixels.iter().all(|p| *p == Vec3::ZERO));
    assert_eq!(rendered.image.samples_traced, 0);
    assert_eq!(rendered.batches.len(), 0);
    assert!(
        rendered.progress.is_empty(),
        "a cancelled render reports no end"
    );
}

#[test]
fn a_flag_set_from_another_thread_stops_the_render_within_the_bound() {
    // The residual would run for minutes; the watcher sets the token after a short delay.
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let beam = beam_settings(SMALL_BATCH);
    let adaptive = AdaptiveSettings {
        max_spp: ENDLESS_SPP,
        ..fixed_settings(HYBRID_SPP)
    };
    let cancel = AtomicBool::new(false);
    let engines = EngineSet::new(Vec::new());
    let (result, latency) = std::thread::scope(|scope| {
        let watcher = scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(CANCEL_DELAY_MS));
            cancel.store(true, Ordering::Relaxed);
            Instant::now()
        });
        let result = render_plain_cancellable(&rig, &scene, (&beam, &adaptive), &engines, &cancel);
        let returned = Instant::now();
        let set_at = watcher.join().expect("the watcher thread");
        (result, returned.saturating_duration_since(set_at))
    });
    eprintln!("cancel latency: {latency:?}");
    assert!(result.is_ok(), "a cancelled render still returns its image");
    assert!(
        latency <= Duration::from_millis(CANCEL_BOUND_MS),
        "the render took {latency:?} to notice the token"
    );
}

/// The entry under test with a caller-owned token.
fn render_plain_cancellable(
    rig: &BeamRig,
    scene: &FrameScene<'_>,
    settings: (&BeamSettings, &AdaptiveSettings),
    engines: &EngineSet<'_>,
    cancel: &AtomicBool,
) -> Result<(AdaptiveImage, BeamStats), BeamOff> {
    let (beam, adaptive) = settings;
    let mut progress = |_: BeamProgress| {};
    render_beam_hybrid_surface(
        scene,
        plain_extras(),
        &rig.soa,
        beam,
        adaptive,
        engines,
        (0, 1.0),
        cancel,
        BeamRun::new(&mut progress),
    )
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn progress_is_monotone_phased_and_ends_at_one() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let beam = beam_settings(SMALL_BATCH);
    let adaptive = fixed_settings(HYBRID_SPP);
    let cancel = AtomicBool::new(false);
    let rendered = render_hooked(
        &rig,
        &scene,
        (&beam, &adaptive),
        &EngineSet::new(Vec::new()),
        None,
        &cancel,
    )
    .expect("an eligible stone");
    let reports = &rendered.progress;
    eprintln!("progress: {} report(s)", reports.len());
    assert_ne!(reports.len(), 0);
    assert!(reports.iter().all(|r| (0.0..=1.0).contains(&r.done)));
    assert!(reports.windows(2).all(|pair| pair[0].done <= pair[1].done));
    assert_eq!(reports[0].eta, None, "no ETA before the first batch");
    assert!(reports.iter().any(|r| r.eta.is_some()));
    assert!(reports.iter().any(|r| r.phase == BeamPhase::Finishing));
    let last = reports[reports.len() - 1];
    assert_eq!(last.done, 1.0);
    assert_eq!(last.phase, BeamPhase::Finishing);
    assert_eq!(last.eta, Some(Duration::ZERO));
}

// ---- eligibility ----

/// A CPU engine that counts the batches it is given.
struct Counting<'e> {
    inner: CpuBeamEngine<'e>,
    calls: &'e AtomicUsize,
}

impl BeamEngine for Counting<'_> {
    fn name(&self) -> &'static str {
        "counting cpu"
    }

    fn fragments(&self, batch: &Batch<'_>, cancel: &AtomicBool) -> EngineOutcome<BatchFragments> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inner.fragments(batch, cancel)
    }

    fn residual(
        &self,
        batch: &Batch<'_>,
        offset: &[Vec3],
        cancel: &AtomicBool,
    ) -> EngineOutcome<BatchResidual> {
        self.inner.residual(batch, offset, cancel)
    }

    fn fragment_stats(&self) -> BeamStats {
        self.inner.fragment_stats()
    }

    fn needs_packed(&self) -> bool {
        false
    }
}

#[test]
fn an_ineligible_stone_returns_the_beam_off_of_the_frame_and_touches_no_engine() {
    let calls = AtomicUsize::new(0);
    // A material that rounds its edges.
    let mut rounded = glass();
    rounded.edge_rounding_radius = 0.1;
    let rig = BeamRig::cube(rounded);
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    // Lighting outside the product scope.
    let lamp_rig = BeamRig::cube(glass());
    let lamp = FrameScene {
        environment: LightingPreset::UvLamp365.studio(1.0, 0.4, 0.35),
        ..lamp_rig.scene(FRAME, MAX_BOUNCES)
    };
    for (rig, scene) in [(&rig, &scene), (&lamp_rig, &lamp)] {
        let harness = Harness::new(rig, scene, SMALL_BATCH);
        let expected = BeamFrame::new(scene, &harness.beam).err();
        assert!(expected.is_some(), "the frame must be ineligible");
        let counting = Counting {
            inner: harness.engine(2),
            calls: &calls,
        };
        let engines = EngineSet::new(vec![boxed(counting)]);
        assert_eq!(harness.render(&engines).err(), expected);
    }
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    // Settings the adaptive loop refuses are `Unavailable`, never a panic.
    let eligible = BeamRig::cube(glass());
    let eligible_scene = eligible.scene(FRAME, MAX_BOUNCES);
    let beam = beam_settings(SMALL_BATCH);
    let bad = AdaptiveSettings {
        tile: 3,
        ..fixed_settings(HYBRID_SPP)
    };
    let refused = render_plain(
        &eligible,
        &eligible_scene,
        (&beam, &bad),
        &EngineSet::new(Vec::new()),
    );
    assert_eq!(refused.err(), Some(BeamOff::Unavailable));
}

// ---- engines: failover, sharing, packed layouts ----

/// An engine that declines everything.
struct Declining<'e> {
    calls: &'e AtomicUsize,
}

impl BeamEngine for Declining<'_> {
    fn name(&self) -> &'static str {
        "declining"
    }

    fn fragments(&self, _: &Batch<'_>, _: &AtomicBool) -> EngineOutcome<BatchFragments> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        EngineOutcome::Declined("test: no device")
    }

    fn residual(&self, _: &Batch<'_>, _: &[Vec3], _: &AtomicBool) -> EngineOutcome<BatchResidual> {
        EngineOutcome::Declined("test: no device")
    }
}

/// An engine whose fragments are the CPU's and whose residual declines (a lost device between the
/// two stages).
struct DecliningResidual<'e> {
    inner: CpuBeamEngine<'e>,
    calls: &'e AtomicUsize,
}

impl BeamEngine for DecliningResidual<'_> {
    fn name(&self) -> &'static str {
        "declining residual"
    }

    fn fragments(&self, batch: &Batch<'_>, cancel: &AtomicBool) -> EngineOutcome<BatchFragments> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inner.fragments(batch, cancel)
    }

    fn residual(&self, _: &Batch<'_>, _: &[Vec3], _: &AtomicBool) -> EngineOutcome<BatchResidual> {
        EngineOutcome::Declined("test: device lost")
    }

    fn fragment_stats(&self) -> BeamStats {
        self.inner.fragment_stats()
    }

    fn needs_packed(&self) -> bool {
        false
    }
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_declining_engine_fails_over_to_the_cpu_with_the_same_image() {
    let first_calls = AtomicUsize::new(0);
    let half_calls = AtomicUsize::new(0);
    let only_calls = AtomicUsize::new(0);
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let reference = baseline(&rig, &scene);
    let harness = Harness::new(&rig, &scene, SMALL_BATCH);

    // A declining engine first, the CPU second: asked once, then marked failed for the render.
    let engines = EngineSet::new(vec![
        boxed(Declining {
            calls: &first_calls,
        }),
        boxed(harness.engine(2)),
    ]);
    let (image, _) = harness.render(&engines).expect("an eligible stone");
    assert_eq!(first_calls.load(Ordering::Relaxed), 1);
    assert_eq!(image_bits(&image), image_bits(&reference));

    // An engine that declines only the residual: its fragments are redone with the batch.
    let flaky = DecliningResidual {
        inner: harness.engine(2),
        calls: &half_calls,
    };
    let engines = EngineSet::new(vec![boxed(flaky)]);
    let (image, _) = harness.render(&engines).expect("an eligible stone");
    assert_eq!(half_calls.load(Ordering::Relaxed), 1);
    assert_eq!(image_bits(&image), image_bits(&reference));

    // Every engine failed: the pipeline's own CPU engine takes the batches.
    let engines = EngineSet::new(vec![boxed(Declining { calls: &only_calls })]);
    let (image, _) = harness.render(&engines).expect("an eligible stone");
    assert_eq!(only_calls.load(Ordering::Relaxed), 1);
    assert_eq!(image_bits(&image), image_bits(&reference));
}

/// The reason of the size-cap decline of [`CapDeclining`].
const CAP_REASON: &str = "test: over the size cap";

/// An engine that declines the first batch for a size cap and runs the others on the CPU.
struct CapDeclining<'e> {
    inner: CpuBeamEngine<'e>,
    calls: &'e AtomicUsize,
}

impl BeamEngine for CapDeclining<'_> {
    fn name(&self) -> &'static str {
        "cap declining"
    }

    fn fragments(&self, batch: &Batch<'_>, cancel: &AtomicBool) -> EngineOutcome<BatchFragments> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if batch.index == 0 {
            return EngineOutcome::Declined(CAP_REASON);
        }
        self.inner.fragments(batch, cancel)
    }

    fn residual(
        &self,
        batch: &Batch<'_>,
        offset: &[Vec3],
        cancel: &AtomicBool,
    ) -> EngineOutcome<BatchResidual> {
        self.inner.residual(batch, offset, cancel)
    }

    fn fragment_stats(&self) -> BeamStats {
        self.inner.fragment_stats()
    }

    fn needs_packed(&self) -> bool {
        false
    }

    fn decline_is_per_batch(&self, reason: &'static str) -> bool {
        reason == CAP_REASON
    }
}

#[test]
fn a_size_cap_decline_costs_the_engine_one_batch_not_the_render() {
    let calls = AtomicUsize::new(0);
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let reference = baseline(&rig, &scene);
    let harness = Harness::new(&rig, &scene, SMALL_BATCH);
    let engines = EngineSet::new(vec![boxed(CapDeclining {
        inner: harness.engine(2),
        calls: &calls,
    })]);
    let (image, _) = harness.render(&engines).expect("an eligible stone");
    // Four batches: the first is declined and goes to the pipeline's CPU engine, the engine is
    // asked again for each of the other three (a retired engine would have been asked once).
    assert_eq!(calls.load(Ordering::Relaxed), TILE_COUNT);
    assert_eq!(image_bits(&image), image_bits(&reference));
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn two_cpu_engines_share_the_batches_in_turn_and_give_the_same_image() {
    let calls = [AtomicUsize::new(0), AtomicUsize::new(0)];
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let reference = baseline(&rig, &scene);
    let harness = Harness::new(&rig, &scene, SMALL_BATCH);
    let engines = EngineSet::new(vec![
        boxed(Counting {
            inner: harness.engine(2),
            calls: &calls[0],
        }),
        boxed(Counting {
            inner: harness.engine(3),
            calls: &calls[1],
        }),
    ]);
    let (image, _) = harness.render(&engines).expect("an eligible stone");
    // Four batches dealt in turn: two each.
    assert_eq!(calls[0].load(Ordering::Relaxed), 2);
    assert_eq!(calls[1].load(Ordering::Relaxed), 2);
    assert_eq!(image_bits(&image), image_bits(&reference));
}

/// A CPU engine that records whether the GPU layouts of its batches were built.
struct Probe<'e> {
    inner: CpuBeamEngine<'e>,
    wants: bool,
    packed_seen: &'e AtomicBool,
}

impl BeamEngine for Probe<'_> {
    fn name(&self) -> &'static str {
        "probe"
    }

    fn fragments(&self, batch: &Batch<'_>, cancel: &AtomicBool) -> EngineOutcome<BatchFragments> {
        if batch.packed_leaves.tiles.len() == batch.tiles.len() && !batch.tiles.is_empty() {
            self.packed_seen.store(true, Ordering::Relaxed);
        }
        self.inner.fragments(batch, cancel)
    }

    fn residual(
        &self,
        batch: &Batch<'_>,
        offset: &[Vec3],
        cancel: &AtomicBool,
    ) -> EngineOutcome<BatchResidual> {
        self.inner.residual(batch, offset, cancel)
    }

    fn fragment_stats(&self) -> BeamStats {
        self.inner.fragment_stats()
    }

    fn needs_packed(&self) -> bool {
        self.wants
    }
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn the_gpu_layouts_are_built_only_for_an_engine_that_wants_them() {
    let seen = [AtomicBool::new(false), AtomicBool::new(false)];
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let reference = baseline(&rig, &scene);
    let harness = Harness::new(&rig, &scene, SMALL_BATCH);
    for (wants, flag) in [false, true].into_iter().zip(&seen) {
        let probe = Probe {
            inner: harness.engine(2),
            wants,
            packed_seen: flag,
        };
        let engines = EngineSet::new(vec![boxed(probe)]);
        let (image, _) = harness.render(&engines).expect("an eligible stone");
        assert_eq!(flag.load(Ordering::Relaxed), wants, "wants = {wants}");
        assert_eq!(image_bits(&image), image_bits(&reference));
    }
}

/// A CPU engine that counts the tiles of its batches whose leaves are borrowed and owned.
struct LeafLender<'e> {
    inner: CpuBeamEngine<'e>,
    borrowed: &'e AtomicUsize,
    owned: &'e AtomicUsize,
}

impl BeamEngine for LeafLender<'_> {
    fn name(&self) -> &'static str {
        "leaf lender"
    }

    fn fragments(&self, batch: &Batch<'_>, cancel: &AtomicBool) -> EngineOutcome<BatchFragments> {
        for (_, leaves) in batch.leaves {
            let counter = match leaves {
                Cow::Borrowed(_) => self.borrowed,
                Cow::Owned(_) => self.owned,
            };
            counter.fetch_add(1, Ordering::Relaxed);
        }
        self.inner.fragments(batch, cancel)
    }

    fn residual(
        &self,
        batch: &Batch<'_>,
        offset: &[Vec3],
        cancel: &AtomicBool,
    ) -> EngineOutcome<BatchResidual> {
        self.inner.residual(batch, offset, cancel)
    }

    fn fragment_stats(&self) -> BeamStats {
        self.inner.fragment_stats()
    }
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_preloaded_render_lends_the_cached_leaves_and_equals_the_copying_render() {
    let borrowed = AtomicUsize::new(0);
    let owned = AtomicUsize::new(0);
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let harness = Harness::new(&rig, &scene, SMALL_BATCH);
    let engines = EngineSet::new(vec![boxed(LeafLender {
        inner: harness.engine(2),
        borrowed: &borrowed,
        owned: &owned,
    })]);
    let cancel = AtomicBool::new(false);
    // A fresh render traces its trees: every batch owns its leaves.
    let fresh = render_hooked(
        &rig,
        &scene,
        (&harness.beam, &harness.adaptive),
        &engines,
        None,
        &cancel,
    )
    .expect("an eligible stone");
    assert!(owned.load(Ordering::Relaxed) > 0, "no batch was shaded");
    assert_eq!(borrowed.load(Ordering::Relaxed), 0, "a traced batch owns");
    let mut cache = LeafCache::new(u64::MAX);
    for batch in fresh.retained {
        assert!(cache.retain(batch));
    }
    assert!(!cache.is_empty());
    owned.store(0, Ordering::Relaxed);

    // The preloaded render takes its leaves from the cache and copies none of them.
    let mut progress = |_: BeamProgress| {};
    let (image, stats) = render_beam_hybrid_surface(
        &scene,
        plain_extras(),
        &rig.soa,
        &harness.beam,
        &harness.adaptive,
        &engines,
        (0, 1.0),
        &cancel,
        BeamRun {
            tiles: None,
            progress: &mut progress,
            on_batch: None,
            retain: None,
            preloaded: Some(&cache),
        },
    )
    .expect("an eligible stone");
    assert_eq!(stats.beams, 0, "a preloaded render traces no beam");
    assert!(borrowed.load(Ordering::Relaxed) > 0, "nothing was lent");
    assert_eq!(owned.load(Ordering::Relaxed), 0, "a leaf was copied");
    assert_eq!(image_bits(&image), image_bits(&fresh.image));
}

// ---- the pure pieces ----

#[test]
fn halving_by_tiles_covers_the_tiles_in_order_and_never_splits_a_single_tile() {
    assert_eq!(split_for_cap(&[], 10), (Vec::new(), 0));
    let whole = 0..4;
    assert_eq!(
        split_for_cap(&[10, 10, 10, 10], 100),
        (std::iter::once(whole).collect::<Vec<_>>(), 0)
    );
    assert_eq!(split_for_cap(&[10, 10, 10, 10], 25), (vec![0..2, 2..4], 1));
    assert_eq!(
        split_for_cap(&[10, 10, 10, 10], 5),
        (vec![0..1, 1..2, 2..3, 3..4], 3)
    );
    assert_eq!(
        split_for_cap(&[10, 10, 10], 15),
        (vec![0..1, 1..2, 2..3], 2)
    );
    let single = 0..1;
    assert_eq!(
        split_for_cap(&[100], 1),
        (std::iter::once(single).collect::<Vec<_>>(), 0)
    );
    // Uneven tiles: the heavy one ends alone, the light ones stay together.
    assert_eq!(
        split_for_cap(&[1, 1, 1, 50], 10),
        (vec![0..2, 2..3, 3..4], 2)
    );
}

#[test]
fn a_pixel_is_the_beam_part_plus_the_residual_mean_clamped_once() {
    let pixel = combine_pixel(
        Vec3::new(1.0, 2.0, 3.0),
        (Vec3::splat(8.0), Vec3::splat(4.0)),
        (8, 4),
    );
    assert_eq!(pixel.sum, Vec3::new(16.0, 24.0, 32.0));
    assert_eq!(pixel.half_b, Vec3::new(8.0, 12.0, 16.0));
    assert_eq!(pixel.mean, Vec3::new(2.0, 3.0, 4.0));
    // A negative beam part is clamped once, after the residual is added; the sums stay unclamped.
    let negative = combine_pixel(
        Vec3::new(-5.0, 1.0, 1.0),
        (Vec3::splat(8.0), Vec3::splat(4.0)),
        (8, 4),
    );
    assert_eq!(negative.mean, Vec3::new(0.0, 2.0, 2.0));
    assert_eq!(negative.sum.x, -32.0);
    // No samples: the beam part alone, and the sums are the residual's.
    let bare = combine_pixel(Vec3::new(1.0, 1.0, 1.0), (Vec3::ZERO, Vec3::ZERO), (0, 0));
    assert_eq!(bare.mean, Vec3::ONE);
    assert_eq!(bare.sum, Vec3::ZERO);
    // A non-finite beam part counts as zero.
    let broken = combine_pixel(
        Vec3::new(f32::NAN, 0.0, 0.0),
        (Vec3::splat(8.0), Vec3::splat(4.0)),
        (8, 4),
    );
    assert_eq!(broken.mean, Vec3::ONE);
}

#[test]
fn the_tile_list_is_the_grid_or_the_members_of_a_matching_restriction() {
    let size = (FRAME, FRAME);
    let tile_count = u32::try_from(TILE_COUNT).expect("the tile count fits in u32");
    let all: Vec<u32> = (0..tile_count).collect();
    assert_eq!(tile_list(size, TILE, None), all);
    let live = LiveTiles::from_indices(FRAME, FRAME, TILE, vec![3, 1, 99, 2]);
    assert_eq!(tile_list(size, TILE, Some(&live)), vec![1, 2, 3]);
    let other_frame = LiveTiles::from_indices(32, 32, TILE, vec![0]);
    assert_eq!(tile_list(size, TILE, Some(&other_frame)), Vec::<u32>::new());
    let other_tile = LiveTiles::from_indices(FRAME, FRAME, 16, vec![0]);
    assert_eq!(tile_list(size, TILE, Some(&other_tile)), Vec::<u32>::new());
    // A partial last column and row are tiles too.
    assert_eq!(tile_list((20, 12), TILE, None).len(), 3 * 2);
}

#[test]
fn a_tile_without_leaves_costs_its_per_pixel_tables() {
    let rect = TileRect {
        x0: 0,
        y0: 0,
        x1: TILE,
        y1: TILE,
    };
    // The per-pixel tables: the offsets, the counts and the insertion cursors (3 tables of `u32`
    // per pixel).
    assert_eq!(tile_cost(rect, &[]), u64::from(TILE * TILE) * 12);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_tile_ordered_render_holds_only_its_tiles_and_equals_the_frame_render_there() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let harness = Harness::new(&rig, &scene, SMALL_BATCH);
    let engines = EngineSet::new(vec![boxed(harness.engine(2))]);
    let live = LiveTiles::from_indices(FRAME, FRAME, TILE, vec![3, 1]);
    let cancel = AtomicBool::new(false);
    let input = HybridInput {
        scene: harness.scene,
        extras: plain_extras(),
        plane_soa: &rig.soa,
        beam: &harness.beam,
        adaptive: &harness.adaptive,
        engines: &engines,
        sample_offset: 0,
        scale: 1.0,
    };
    let mut progress = |_: BeamProgress| {};
    let (full, full_stats) = render_hybrid_with_threads(
        &input,
        2,
        &cancel,
        BeamRun {
            tiles: Some(&live),
            ..BeamRun::new(&mut progress)
        },
    )
    .expect("an eligible stone");
    let mut progress = |_: BeamProgress| {};
    let (tiled, tiled_stats) = render_hybrid_tiles_with_threads(
        &input,
        2,
        &cancel,
        BeamRun {
            tiles: Some(&live),
            ..BeamRun::new(&mut progress)
        },
    )
    .expect("an eligible stone");
    let rects: Vec<TileRect> = [1_u32, 3]
        .iter()
        .map(|&index| TileRect::of_index((FRAME, FRAME), TILE, index).expect("a tile"))
        .collect();
    // Two tiles of 8 x 8 pixels, not the 16 x 16 frame; tile 1 before tile 3.
    assert_eq!(tiled.tiles, vec![1, 3]);
    assert_eq!(tiled.pixels.len(), 2 * (TILE * TILE) as usize);
    assert_eq!(tiled.counts.len(), tiled.pixels.len());
    assert_eq!(
        vec_bits(&tiled.pixels),
        vec_bits(&gather(&full.pixels, &rects))
    );
    assert_eq!(vec_bits(&tiled.sum), vec_bits(&gather(&full.sum, &rects)));
    assert_eq!(
        vec_bits(&tiled.half_b),
        vec_bits(&gather(&full.half_b, &rects))
    );
    assert!(tiled.counts.iter().all(|&count| count == HYBRID_SPP));
    assert_eq!(tiled.samples_traced, full.samples_traced);
    assert_eq!(tiled_stats, full_stats);
}

#[test]
fn the_job_runner_returns_results_in_index_order_for_any_thread_count() {
    let cancel = AtomicBool::new(false);
    for threads in [1, 2, 7] {
        let squares = run_jobs(20, threads, &cancel, |i| Some(i * i)).expect("not cancelled");
        assert_eq!(squares, (0..20).map(|i| i * i).collect::<Vec<usize>>());
    }
    // A job without a result fails the batch; a set token gives nothing.
    assert_eq!(
        run_jobs(4, 2, &cancel, |i| (i != 2).then_some(i)),
        None::<Vec<usize>>
    );
    let raised = AtomicBool::new(true);
    assert_eq!(run_jobs(4, 2, &raised, Some), None::<Vec<usize>>);
    assert_eq!(run_jobs(0, 2, &cancel, Some), Some(Vec::<usize>::new()));
}

/// The engines, the beam frame and the pipeline's fallback share one preparation of the material,
/// so a render builds its optics tables once (checked by `Arc` identity, not by timing).
#[test]
fn one_preparation_and_one_set_of_optics_tables_serve_the_engines_the_frame_and_the_fallback() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let prepared = PreparedMaterial::new(&rig.material).with_optics_tables(OpticsTables::On);
    let tables = Arc::clone(prepared.optics_table_set().expect("the tables are on"));
    let beam = beam_settings(SMALL_BATCH);
    let adaptive = fixed_settings(HYBRID_SPP);
    let engines = EngineSet::with_prepared(Vec::new(), &prepared);
    let input = HybridInput {
        scene: &scene,
        extras: plain_extras(),
        plane_soa: &rig.soa,
        beam: &beam,
        adaptive: &adaptive,
        engines: &engines,
        sample_offset: 0,
        scale: 1.0,
    };
    // The pipeline takes the engines' preparation, tables included, and hands it to the frame.
    let shared = shared_preparation(&input);
    assert!(Arc::ptr_eq(
        shared.optics_table_set().expect("shared tables"),
        &tables
    ));
    let frame = BeamFrame::with_prepared(&scene, &beam, shared).expect("an eligible stone");
    assert!(Arc::ptr_eq(
        frame.prepared().optics_table_set().expect("frame tables"),
        &tables
    ));
    // A frame for another material object, or a set that carries nothing, prepares its own.
    let other = glass();
    assert!(engines.prepared_for(&rig.material).is_some());
    assert!(engines.prepared_for(&other).is_none());
    assert!(
        EngineSet::new(Vec::new())
            .prepared_for(&rig.material)
            .is_none()
    );
    // The eligibility check answers like the frame without building one.
    assert!(BeamFrame::accepts(&scene).is_ok());
}

/// Reusing the engines' preparation changes nothing in the image.
#[test]
fn a_render_with_the_carried_preparation_equals_one_that_prepares_again() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let harness = Harness::new(&rig, &scene, SMALL_BATCH);
    let carried = EngineSet::with_prepared(vec![boxed(harness.engine(2))], &harness.prepared);
    let separate = EngineSet::new(vec![boxed(harness.engine(2))]);
    let with = harness.render(&carried).expect("an eligible stone").0;
    let without = harness.render(&separate).expect("an eligible stone").0;
    assert_eq!(image_bits(&with), image_bits(&without));
}
