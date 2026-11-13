//! Tests of the batch lifetime of the kept index and of the CPU batch residual
//! `tile_residual::run_batch_residual` (beam guide §12.3, §12.4, §16.3).
//!
//! The residual of a batch must equal the plain reference adaptive run when nothing is kept, and
//! must add up with the beam part to the plain reference when the beams' kept index is used (a
//! z-test on the frame total). It polls the cancel token and does not depend on the thread count.
//! The thresholds and sample counts are named constants; the numbers a run produces are printed
//! with `eprintln!` and never asserted.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use glam::{DVec2, Vec3};

use super::{
    BeamSettings,
    beam::{BranchPrefix, Leaf, PackedKept, PackedLeaves, SpectralInterval},
    engines::{Batch, BatchResidual},
    frame::{BeamFrame, BeamImage},
    geom::polygon::ConvexPolygon,
    leaf::node_count,
    primary::TileRect,
    residual::{KeptIndex, batch_kept_indices, pack_kept},
    tests_support::{BeamRig, MAX_BOUNCES, glass},
    tile_residual::{ResidualScene, run_batch_residual, run_batch_residual_with},
};
use crate::{
    geometry::tool::StoneGeometry,
    optics::{
        fluorescence::Fluorescence,
        raytracer::{
            add_finite_sample, pixel_rotations, sample_draws,
            surface::SurfaceScene,
            transport::{PreparedMaterial, reference::trace_spectral_ray_reference_prepared},
        },
    },
    renderer::{
        adaptive::{AdaptiveRun, AdaptiveSettings, LiveTiles, NoiseMetric, Pass},
        frame_scene::FrameScene,
    },
};

/// The side of the square test frame in pixels.
const FRAME: u32 = 32;

/// The adaptive tile and the beam tile in pixels (16 tiles).
const TILE: u32 = 8;

/// The tiles of the small batch: the middle 2 x 2 tiles of the 4 x 4 grid.
const MIDDLE_TILES: [u32; 4] = [5, 6, 9, 10];

/// The minimum residual samples of the equivalence tests, which is also their pass size.
const RESIDUAL_MIN_SPP: u32 = 16;

/// The cap of the equivalence tests: with a target no tile reaches, every pixel gets exactly this.
const FIXED_SPP: u32 = 64;

/// Samples per pass: the residual minimum, as `tile_residual::residual_settings` makes it, so the
/// plain baseline loops sum in the same passes as the batch residual.
const PASS_SPP: u32 = RESIDUAL_MIN_SPP;

/// Reference samples per pixel of the plain stream the z-test compares with.
const PLAIN_SPP: u32 = 128;

/// The first absolute sample index of the plain stream (an independent stream from the residual).
const PLAIN_FIRST_SAMPLE: u32 = 100_000;

/// The largest |z| the frame total may show.
const Z_LIMIT: f64 = 5.0;

/// The relative slack of the frame total for the beams' own checked tolerances (guide §12.3: about
/// 0.2 % of a branch in a boundary pixel, plus the spectral and weight tolerances).
const RELATIVE_SLACK: f64 = 0.02;

/// Milliseconds before the watcher thread of the cancel test sets the token.
const CANCEL_DELAY_MS: u64 = 40;

/// A cap so high that the cancel test cannot finish before the token is set.
const ENDLESS_SPP: u32 = 1 << 20;

/// Settings whose tiles never converge: every pixel runs to `max_spp`.
const fn fixed_settings(min_spp: u32, max_spp: u32) -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise: 1.0e-6,
        metric: NoiseMetric::AbsoluteDeltaE,
        min_spp,
        max_spp,
        pass_spp: PASS_SPP,
        tile: TILE,
    }
}

/// Settings that let quiet tiles stop at the minimum.
const fn converging_settings() -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise: 0.5,
        metric: NoiseMetric::AbsoluteDeltaE,
        min_spp: 512,
        max_spp: FIXED_SPP,
        pass_spp: PASS_SPP,
        tile: TILE,
    }
}

/// The frame scene of a rig.
fn frame_scene(rig: &BeamRig) -> FrameScene<'_> {
    rig.scene(FRAME, MAX_BOUNCES)
}

/// The residual scene of a rig with `residual_min_spp`.
fn residual_scene<'a>(
    rig: &'a BeamRig,
    frame: &'a FrameScene<'a>,
    prepared: &'a PreparedMaterial<'a>,
    residual_min_spp: u32,
) -> ResidualScene<'a> {
    ResidualScene {
        frame,
        extras: (&[], Fluorescence::none(), SurfaceScene::IDEAL),
        plane_soa: &rig.soa,
        prepared,
        n_lambda: node_count(&BeamSettings::default()),
        sample_offset: 0,
        residual_min_spp,
    }
}

/// The beam tree of the frame.
fn beam_image(frame: &FrameScene<'_>) -> BeamImage {
    let beams = BeamFrame::new(frame, &BeamSettings::default()).expect("eligible");
    let cancel = AtomicBool::new(false);
    beams
        .render_beams_with_threads(TILE, 4, &cancel)
        .expect("not cancelled")
}

/// The tile `index` of the 4 x 4 grid.
fn tile_rect(index: u32) -> TileRect {
    TileRect::of_index((FRAME, FRAME), TILE, index).expect("a tile of the grid")
}

/// A batch's owned parts: tiles, kept indices, the (unused) GPU layouts and the beam part.
struct Fixture {
    tiles: Vec<TileRect>,
    kept: Vec<(TileRect, KeptIndex)>,
    packed_leaves: PackedLeaves,
    packed_kept: PackedKept,
    offset: Vec<Vec3>,
}

impl Fixture {
    /// The batch of `tile_ids`; `real_kept` takes the beams' kept indices, else empty ones;
    /// `real_offset` takes the beam image as the offset, else zeros.
    fn new(image: &BeamImage, tile_ids: &[u32], real_kept: bool, real_offset: bool) -> Self {
        let tiles: Vec<TileRect> = tile_ids.iter().map(|&id| tile_rect(id)).collect();
        let kept: Vec<(TileRect, KeptIndex)> = tile_ids
            .iter()
            .zip(&tiles)
            .map(|(&id, &rect)| {
                if real_kept {
                    image.kept[id as usize].clone()
                } else {
                    (rect, KeptIndex::from_leaves(rect, &[]))
                }
            })
            .collect();
        let offset = if real_offset {
            gather(&image.xyz, &tiles)
        } else {
            vec![Vec3::ZERO; tiles.iter().map(|t| t.pixel_count()).sum::<usize>()]
        };
        let packed_kept = pack_kept(&kept);
        Self {
            tiles,
            kept,
            packed_leaves: PackedLeaves::default(),
            packed_kept,
            offset,
        }
    }

    /// The batch view.
    fn batch(&self) -> Batch<'_> {
        Batch {
            index: 0,
            tiles: &self.tiles,
            leaves: &[],
            kept: &self.kept,
            packed_leaves: &self.packed_leaves,
            packed_kept: &self.packed_kept,
        }
    }
}

/// The pixels of the row-major frame buffer `buffer` in batch order (tiles in order, each tile
/// row-major).
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

/// The same for a per-pixel count buffer.
fn gather_counts(buffer: &[u32], tiles: &[TileRect]) -> Vec<u32> {
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

/// One sample of the plain reference tracer (`kept = None`) of pixel `(x, y)`.
fn reference_sample(
    rig: &BeamRig,
    frame: &FrameScene<'_>,
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
        frame.environment,
        draws.seed,
        draws.hero_rand,
        None,
        SurfaceScene::IDEAL,
        None,
    )
    .xyz
}

/// Traces one adaptive pass of the plain reference tracer into the full-frame buffers, in the
/// order of the pass contract (a pixel's pass total is formed first, then added).
fn reference_pass(
    rig: &BeamRig,
    frame: &FrameScene<'_>,
    prepared: &PreparedMaterial<'_>,
    pass: &Pass,
    buffers: (&mut [Vec3], &mut [Vec3]),
) {
    let (sum, half_b) = buffers;
    for &tile in pass.live.indices() {
        let (x0, y0, x1, y1) = pass.live.tile_rect(tile);
        for y in y0..y1 {
            for x in x0..x1 {
                let mut pass_sum = Vec3::ZERO;
                let mut pass_odd = Vec3::ZERO;
                for local in 0..pass.spp {
                    let sample_num = pass.first_sample + local;
                    let sample = reference_sample(rig, frame, prepared, (x, y), sample_num);
                    add_finite_sample(&mut pass_sum, sample);
                    if !sample_num.is_multiple_of(2) {
                        add_finite_sample(&mut pass_odd, sample);
                    }
                }
                let pixel = (y * FRAME + x) as usize;
                sum[pixel] += pass_sum;
                half_b[pixel] += pass_odd;
            }
        }
    }
}

/// The baseline: the plain reference adaptive run over the frame, restricted to `tiles` (all tiles
/// when `None`), with the beam part `offset` (a full-frame buffer) in the metric. Returns the
/// full-frame sums, odd sums and counts.
fn baseline(
    rig: &BeamRig,
    settings: AdaptiveSettings,
    tiles: Option<&[u32]>,
    offset: Option<&[Vec3]>,
) -> (Vec<Vec3>, Vec<Vec3>, Vec<u32>) {
    let frame = frame_scene(rig);
    let prepared = PreparedMaterial::new(&rig.material);
    let mut run = AdaptiveRun::new(settings, FRAME, FRAME).expect("valid settings");
    if let Some(ids) = tiles {
        run = run.restricted_to(&LiveTiles::from_indices(FRAME, FRAME, TILE, ids.to_vec()));
    }
    let pixels = (FRAME * FRAME) as usize;
    let mut sum = vec![Vec3::ZERO; pixels];
    let mut half_b = vec![Vec3::ZERO; pixels];
    while let Some(pass) = run.next_pass() {
        reference_pass(rig, &frame, &prepared, &pass, (&mut sum, &mut half_b));
        run.finish_pass_with_offset(&sum, &half_b, 1.0, offset);
    }
    let counts = run.pixel_counts();
    (sum, half_b, counts)
}

/// The bit patterns of a vector image.
fn vec_bits(values: &[Vec3]) -> Vec<[u32; 3]> {
    values
        .iter()
        .map(|v| v.to_array().map(f32::to_bits))
        .collect()
}

/// A residual as comparable bits: the sums, the half sums, the counts and the traced samples.
type ResidualBits = (Vec<[u32; 3]>, Vec<[u32; 3]>, Vec<u32>, u64);

/// A residual as comparable bits.
fn residual_bits(r: &BatchResidual) -> ResidualBits {
    (
        vec_bits(&r.sum),
        vec_bits(&r.half_b),
        r.counts.clone(),
        r.samples_traced,
    )
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn an_empty_kept_index_gives_the_plain_reference_adaptive_run() {
    let rig = BeamRig::cube(glass());
    let frame = frame_scene(&rig);
    let prepared = PreparedMaterial::new(&rig.material);
    let image = beam_image(&frame);
    let all: Vec<u32> = (0..16).collect();
    let fixture = Fixture::new(&image, &all, false, false);
    let scene = residual_scene(&rig, &frame, &prepared, RESIDUAL_MIN_SPP);
    let cancel = AtomicBool::new(false);
    // The adaptive minimum of 512 is replaced by the residual's 16; the cap and pass stay.
    let settings = fixed_settings(512, FIXED_SPP);
    let result = run_batch_residual(
        &scene,
        &fixture.batch(),
        &fixture.offset,
        &settings,
        &cancel,
    )
    .expect("not cancelled");
    let expected = baseline(
        &rig,
        fixed_settings(RESIDUAL_MIN_SPP, FIXED_SPP),
        None,
        None,
    );
    assert_eq!(result.sum.len(), 1024);
    assert_eq!(
        vec_bits(&result.sum),
        vec_bits(&gather(&expected.0, &fixture.tiles))
    );
    assert_eq!(
        vec_bits(&result.half_b),
        vec_bits(&gather(&expected.1, &fixture.tiles))
    );
    assert_eq!(result.counts, gather_counts(&expected.2, &fixture.tiles));
    assert_eq!(result.samples_traced, 1024 * u64::from(FIXED_SPP));
    // An empty offset means no beam part: the same result.
    let bare = run_batch_residual(&scene, &fixture.batch(), &[], &settings, &cancel)
        .expect("not cancelled");
    assert_eq!(residual_bits(&bare), residual_bits(&result));
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_subset_batch_with_a_beam_offset_equals_the_restricted_reference_run() {
    // Quiet tiles stop at the minimum, noisy ones run on: the decisions read the beam part.
    let rig = BeamRig::cube(glass());
    let frame = frame_scene(&rig);
    let prepared = PreparedMaterial::new(&rig.material);
    let image = beam_image(&frame);
    let fixture = Fixture::new(&image, &MIDDLE_TILES, false, true);
    let scene = residual_scene(&rig, &frame, &prepared, RESIDUAL_MIN_SPP);
    let cancel = AtomicBool::new(false);
    let result = run_batch_residual(
        &scene,
        &fixture.batch(),
        &fixture.offset,
        &converging_settings(),
        &cancel,
    )
    .expect("not cancelled");
    let expected = baseline(
        &rig,
        AdaptiveSettings {
            min_spp: RESIDUAL_MIN_SPP,
            ..converging_settings()
        },
        Some(MIDDLE_TILES.as_slice()),
        Some(image.xyz.as_slice()),
    );
    let mut spread: Vec<u32> = result.counts.clone();
    spread.sort_unstable();
    spread.dedup();
    eprintln!("subset batch: distinct sample counts {spread:?}");
    assert_eq!(result.sum.len(), fixture.offset.len());
    assert_eq!(
        vec_bits(&result.sum),
        vec_bits(&gather(&expected.0, &fixture.tiles))
    );
    assert_eq!(
        vec_bits(&result.half_b),
        vec_bits(&gather(&expected.1, &fixture.tiles))
    );
    assert_eq!(result.counts, gather_counts(&expected.2, &fixture.tiles));
    // The other tiles of the frame got nothing.
    assert_eq!(
        result.samples_traced,
        result.counts.iter().map(|&c| u64::from(c)).sum::<u64>()
    );
}

/// The mean and the standard error of the mean of `values`.
fn mean_and_se(values: &[f64]) -> (f64, f64) {
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let variance = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / (n - 1.0);
    (mean, (variance / n).sqrt())
}

/// The frame total of Y (beam part plus residual mean) and the variance estimate of its residual
/// part, from the half-buffer split (`E[((A - B) / 2)^2]` is the variance of the mean).
fn total_and_variance(offset: &[Vec3], result: &BatchResidual) -> (f64, f64) {
    let mut total = 0.0_f64;
    let mut variance = 0.0_f64;
    for (index, beam) in offset.iter().enumerate() {
        let count = result.counts[index];
        let odd = count / 2;
        let even = count - odd;
        let mean = result.sum[index] / count as f32;
        let mean_a = (result.sum[index] - result.half_b[index]) / even as f32;
        let mean_b = result.half_b[index] / odd as f32;
        total += f64::from(beam.y) + f64::from(mean.y);
        let half_gap = f64::from(mean_a.y - mean_b.y) / 2.0;
        variance = half_gap.mul_add(half_gap, variance);
    }
    (total, variance)
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn the_residual_with_the_kept_index_plus_the_beams_matches_the_plain_reference() {
    let rig = BeamRig::cube(glass());
    let frame = frame_scene(&rig);
    let prepared = PreparedMaterial::new(&rig.material);
    let image = beam_image(&frame);
    assert!(image.stats.leaves > 0, "no leaves: nothing is kept");
    let all: Vec<u32> = (0..16).collect();
    let cancel = AtomicBool::new(false);
    let scene = residual_scene(&rig, &frame, &prepared, FIXED_SPP);
    let settings = fixed_settings(FIXED_SPP, FIXED_SPP);

    let kept = Fixture::new(&image, &all, true, true);
    let with_kept = run_batch_residual(&scene, &kept.batch(), &kept.offset, &settings, &cancel)
        .expect("not cancelled");
    let (total, residual_variance) = total_and_variance(&kept.offset, &with_kept);

    // What the same residual would be if nothing were kept: it must hold much more light.
    let nothing = Fixture::new(&image, &all, false, false);
    let without = run_batch_residual(
        &scene,
        &nothing.batch(),
        &nothing.offset,
        &settings,
        &cancel,
    )
    .expect("not cancelled");
    let (residual_only, _) = total_and_variance(&nothing.offset, &without);
    let (with_residual, _) = total_and_variance(&[Vec3::ZERO; 1024], &with_kept);

    // The plain reference stream: per sample the frame's Y.
    let mut aggregate = vec![0.0_f64; PLAIN_SPP as usize];
    for y in 0..FRAME {
        for x in 0..FRAME {
            for i in 0..PLAIN_SPP {
                let sample =
                    reference_sample(&rig, &frame, &prepared, (x, y), PLAIN_FIRST_SAMPLE + i);
                if sample.is_finite() {
                    aggregate[i as usize] += f64::from(sample.y);
                }
            }
        }
    }
    let (plain, plain_se) = mean_and_se(&aggregate);
    let spread = residual_variance.sqrt().hypot(plain_se);
    let difference = total - plain;
    eprintln!(
        "cube glass: beams + residual Y {total:.4}, plain reference Y {plain:.4} +- {plain_se:.4}, \
         residual spread {:.4}, difference {difference:+.4} ({:+.3} %), residual alone with kept \
         {with_residual:.4}, without kept {residual_only:.4}, leaves {}",
        residual_variance.sqrt(),
        100.0 * difference / plain.max(1.0e-12),
        image.stats.leaves
    );
    assert!(
        difference.abs() <= RELATIVE_SLACK.mul_add(plain.abs(), Z_LIMIT * spread),
        "beams + residual {total} against the plain reference {plain} (spread {spread})"
    );
    assert!(
        with_residual < residual_only,
        "the kept index removed no light: {with_residual} against {residual_only}"
    );
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_cancelled_token_returns_none_at_once() {
    let rig = BeamRig::cube(glass());
    let frame = frame_scene(&rig);
    let prepared = PreparedMaterial::new(&rig.material);
    let image = beam_image(&frame);
    let fixture = Fixture::new(&image, &MIDDLE_TILES, true, true);
    let scene = residual_scene(&rig, &frame, &prepared, RESIDUAL_MIN_SPP);
    let cancel = AtomicBool::new(true);
    let result = run_batch_residual(
        &scene,
        &fixture.batch(),
        &fixture.offset,
        &fixed_settings(RESIDUAL_MIN_SPP, FIXED_SPP),
        &cancel,
    );
    assert_eq!(result, None);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_token_set_during_the_run_stops_it_within_a_pass() {
    // The run would take minutes; the watcher sets the token after a short delay and the loop
    // (polled per pass and per tile row) returns `None` instead of finishing.
    let rig = BeamRig::cube(glass());
    let frame = frame_scene(&rig);
    let prepared = PreparedMaterial::new(&rig.material);
    let image = beam_image(&frame);
    let fixture = Fixture::new(&image, &MIDDLE_TILES, true, true);
    let scene = residual_scene(&rig, &frame, &prepared, RESIDUAL_MIN_SPP);
    let cancel = AtomicBool::new(false);
    let settings = fixed_settings(RESIDUAL_MIN_SPP, ENDLESS_SPP);
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(CANCEL_DELAY_MS));
            cancel.store(true, Ordering::Relaxed);
        });
        run_batch_residual_with(
            &scene,
            &fixture.batch(),
            &fixture.offset,
            &settings,
            (2, 1.0),
            &cancel,
            &mut |_| {},
        )
    });
    assert_eq!(result, None);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn one_and_eight_threads_are_bit_identical() {
    let rig = BeamRig::cube(glass());
    let frame = frame_scene(&rig);
    let prepared = PreparedMaterial::new(&rig.material);
    let image = beam_image(&frame);
    let fixture = Fixture::new(&image, &MIDDLE_TILES, true, true);
    let scene = residual_scene(&rig, &frame, &prepared, RESIDUAL_MIN_SPP);
    let cancel = AtomicBool::new(false);
    let settings = converging_settings();
    let run = |threads: usize| {
        run_batch_residual_with(
            &scene,
            &fixture.batch(),
            &fixture.offset,
            &settings,
            (threads, 1.0),
            &cancel,
            &mut |_| {},
        )
        .expect("not cancelled")
    };
    let one = run(1);
    let eight = run(8);
    let again = run(8);
    assert_eq!(one.sum.len(), fixture.offset.len());
    assert_eq!(residual_bits(&one), residual_bits(&eight));
    assert_eq!(residual_bits(&eight), residual_bits(&again));
    assert!(
        one.sum.iter().any(|p| p.y > 0.0),
        "an empty image proves nothing"
    );
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn an_empty_batch_gives_an_empty_residual_and_a_foreign_tile_gives_none() {
    let rig = BeamRig::cube(glass());
    let frame = frame_scene(&rig);
    let prepared = PreparedMaterial::new(&rig.material);
    let image = beam_image(&frame);
    let scene = residual_scene(&rig, &frame, &prepared, RESIDUAL_MIN_SPP);
    let cancel = AtomicBool::new(false);
    let settings = fixed_settings(RESIDUAL_MIN_SPP, FIXED_SPP);
    let empty = Fixture::new(&image, &[], false, false);
    let result = run_batch_residual(&scene, &empty.batch(), &[], &settings, &cancel);
    assert_eq!(result, Some(BatchResidual::default()));
    // A rectangle that is not a tile of the adaptive grid is refused.
    let mut odd = Fixture::new(&image, &[5], false, false);
    odd.tiles[0].x0 += 1;
    assert_eq!(
        run_batch_residual(&scene, &odd.batch(), &odd.offset, &settings, &cancel),
        None
    );
}

// ---- the batch lifetime of the kept index ----

/// A leaf over the image polygon `points` (counter-clockwise).
fn leaf_over(points: &[DVec2], sig: [u32; 2]) -> Leaf {
    let spectrum = SpectralInterval { first: 0, last: 47 };
    let count = points.len() * spectrum.len();
    Leaf {
        tile: 0,
        image: ConvexPolygon::from_image_points(points).expect("a polygon"),
        dirs: vec![Vec3::Z; count],
        weights: vec![1.0; count],
        spectrum,
        sig,
        prefix: BranchPrefix::EMPTY,
        glare: false,
        reflection: false,
        tau_omega: 1.0e-4,
        pre_clip: None,
    }
}

#[test]
fn the_batch_kept_indices_follow_the_batch_tiles_and_the_leaf_tile_indices() {
    let square = [
        DVec2::new(9.0, 1.0),
        DVec2::new(12.0, 1.0),
        DVec2::new(12.0, 4.0),
        DVec2::new(9.0, 4.0),
    ];
    let tiles = [tile_rect(0), tile_rect(1), tile_rect(6)];
    // The leaf lies in tile 1 (pixels 8..16 x 0..8) of the 4 x 4 grid.
    let leaves = vec![(1_u32, vec![leaf_over(&square, [3, 4])])];
    let indices = batch_kept_indices(&tiles, &leaves, (FRAME, TILE));
    assert_eq!(indices.len(), 3);
    assert_eq!(indices[0].0, tiles[0]);
    assert_eq!(indices[1].0, tiles[1]);
    assert_eq!(indices[2].0, tiles[2]);
    assert!(indices[0].1.is_empty());
    assert!(indices[2].1.is_empty());
    assert_eq!(indices[1].1.len(), 1);
    let direct = KeptIndex::from_leaves(tiles[1], &leaves[0].1);
    assert_eq!(indices[1].1, direct);
    // The packed form of the batch covers exactly the batch's tiles.
    let packed = pack_kept(&indices);
    assert_eq!(packed.regions.len(), 1);
    assert_eq!(packed.origin, (0, 0));
    drop(indices);
}
