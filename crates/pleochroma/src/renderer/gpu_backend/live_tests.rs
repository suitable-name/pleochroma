//! GPU backend tests of adaptive (live-tile) passes (`gpu` feature). Like the tests next door
//! they skip with a printed note when no adapter is present (`PLEOCHROMA_REQUIRE_GPU=1` turns
//! the skip into a failure).
//!
//! The pass contract under test (`renderer::adaptive`): a pass adds the samples
//! `sample_offset..sample_offset + spp` to every pixel of the live tiles, into the summed
//! accumulator and into half-buffer B, which receives only the samples with an odd absolute
//! index; pixels outside the live tiles are untouched in both.
//!
//! GPU-vs-GPU comparisons use the event-wavefront golden tolerance (`compare_frames`, never
//! bit-exactness: WGSL may contract to fma); GPU-vs-CPU comparisons are tile totals within a
//! deliberately loose bound, like `zoned_tests`.

use std::sync::atomic::AtomicBool;

use glam::Vec3;

use super::{
    GpuAccumulate, GpuBackend, GpuSceneRef,
    tests::{acquire_or_skip, bits},
};
use crate::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset, build_plane_soa, surface::SurfaceScene},
    },
    renderer::{
        adaptive::LiveTiles,
        cpu_frame::trace_live_tiles_surface,
        frame_scene::FrameScene,
        gpu::{event_wavefront::compare::compare_frames, frame::live_pixel_indices},
    },
};

const MAX_BOUNCES: u32 = 6;

/// A frame whose tiles do not divide it: 40 x 36 at tile 16 is 3 x 3 tiles, the right column 8
/// pixels wide and the bottom row 4 pixels high.
const FRAME: (u32, u32) = (40, 36);
const TILE: u32 = 16;

/// The live tiles of the partial-set tests: an interior tile, the clipped right edge tile
/// (index 5), the clipped bottom row tile (7) and the clipped corner tile (8); tiles 0, 2, 3, 4
/// and 6 stay dead.
const PARTIAL: [u32; 4] = [1, 5, 7, 8];

/// How far the GPU and the CPU twin tile totals may differ, relative to the larger of the CPU
/// tile total and a fifth of the mean tile total (so a dark clipped tile does not divide by
/// almost nothing). An estimate, not a measured budget: two unbiased estimators at 192 samples
/// per pixel, with the half-buffer holding half the samples. At 24 samples per pixel a clipped
/// tile differed by several percent, which is estimator noise, so the test uses more samples.
const TILE_TOTAL_REL_TOL: f64 = 0.05;

/// A value no pass writes, to tell untouched pixels from zero.
const SENTINEL: Vec3 = Vec3::new(7.25, -3.5, 1.0e-3);

fn camera() -> Camera {
    Camera::new(0.35, 0.28, 5.0, 18.0)
}

const fn environment() -> crate::optics::raytracer::EnvironmentSource<'static> {
    LightingPreset::Daylight.studio(1.0, 0.4, 0.35)
}

const fn gpu_scene<'a>(
    camera: &'a Camera,
    planes: &'a [GpuFacetPlane],
    material: &'a GemMaterial,
) -> GpuSceneRef<'a> {
    GpuSceneRef {
        camera,
        width: FRAME.0,
        height: FRAME.1,
        planes,
        facet_finishes: &[],
        material,
        max_bounces: MAX_BOUNCES,
        environment: environment(),
        estimator: None,
    }
}

fn partial_tiles() -> LiveTiles {
    LiveTiles::from_indices(FRAME.0, FRAME.1, TILE, PARTIAL.to_vec())
}

const fn frame_len() -> usize {
    (FRAME.0 * FRAME.1) as usize
}

/// One GPU pass over `live` into buffers pre-filled with `fill`.
fn gpu_pass(
    backend: &GpuBackend,
    live: &LiveTiles,
    (sample_offset, spp): (u32, u32),
    fill: Vec3,
) -> (GpuAccumulate, Vec<Vec3>, Vec<Vec3>) {
    let camera = camera();
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let scene = gpu_scene(&camera, &planes, &material);
    let mut accum = vec![fill; frame_len()];
    let mut half_b = vec![fill; frame_len()];
    let never_cancel = AtomicBool::new(false);
    let outcome = backend.try_accumulate_geom_surface_live_cancellable(
        &scene,
        &[],
        &SurfaceScene::IDEAL,
        sample_offset,
        spp,
        live,
        &mut accum,
        &mut half_b,
        &never_cancel,
    );
    (outcome, accum, half_b)
}

/// The CPU twin of [`gpu_pass`], into zeroed buffers.
fn cpu_pass(live: &LiveTiles, (sample_offset, spp): (u32, u32)) -> (Vec<Vec3>, Vec<Vec3>) {
    let camera = camera();
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let scene = FrameScene {
        camera: &camera,
        width: FRAME.0,
        height: FRAME.1,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: MAX_BOUNCES,
        environment: environment(),
    };
    let soa = build_plane_soa(&planes);
    let mut sum = vec![Vec3::ZERO; frame_len()];
    let mut half_b = vec![Vec3::ZERO; frame_len()];
    trace_live_tiles_surface(
        &scene,
        (&[], Fluorescence::none(), SurfaceScene::IDEAL),
        &soa,
        live,
        sample_offset,
        spp,
        &mut sum,
        &mut half_b,
    );
    (sum, half_b)
}

/// Channel totals of tile `index` of `live` in `pixels`.
fn tile_total(pixels: &[Vec3], live: &LiveTiles, index: u32) -> [f64; 3] {
    let (x0, y0, x1, y1) = live.tile_rect(index);
    let mut total = [0.0f64; 3];
    for y in y0..y1 {
        for x in x0..x1 {
            let p = pixels[(y * live.width() + x) as usize];
            total[0] += f64::from(p.x);
            total[1] += f64::from(p.y);
            total[2] += f64::from(p.z);
        }
    }
    total
}

/// Asserts the GPU and CPU tile totals of every live tile agree within [`TILE_TOTAL_REL_TOL`].
fn assert_tile_totals_close(label: &str, live: &LiveTiles, gpu: &[Vec3], cpu: &[Vec3]) {
    let totals: Vec<([f64; 3], [f64; 3])> = live
        .indices()
        .iter()
        .map(|&t| (tile_total(gpu, live, t), tile_total(cpu, live, t)))
        .collect();
    for channel in 0..3 {
        let mean_cpu = totals.iter().map(|(_, c)| c[channel]).sum::<f64>() / totals.len() as f64;
        for (&tile, (g, c)) in live.indices().iter().zip(&totals) {
            let scale = c[channel].abs().max(0.2 * mean_cpu.abs()).max(1.0e-9);
            let rel = (g[channel] - c[channel]).abs() / scale;
            println!(
                "{label}: tile {tile} channel {channel}: GPU {:.4} CPU {:.4} rel {rel:.4}",
                g[channel], c[channel]
            );
            assert!(
                rel <= TILE_TOTAL_REL_TOL,
                "{label}: tile {tile}, channel {channel}: GPU {} vs CPU {} (relative {rel:.4}, \
                 limit {TILE_TOTAL_REL_TOL})",
                g[channel],
                c[channel]
            );
        }
    }
}

/// Copies `pixels` at the live pixel indices only.
fn live_values(pixels: &[Vec3], live: &LiveTiles) -> Vec<Vec3> {
    live_pixel_indices(live)
        .iter()
        .map(|&p| pixels[p as usize])
        .collect()
}

/// One adaptive pass on the GPU against its CPU twin, over a partial tile set that includes the
/// clipped edge tiles: tile totals of the summed accumulator and of half-buffer B agree.
#[test]
fn a_gpu_live_pass_matches_the_cpu_twin_on_a_partial_tile_set() {
    let Some(backend) =
        acquire_or_skip("a_gpu_live_pass_matches_the_cpu_twin_on_a_partial_tile_set")
    else {
        return;
    };
    let live = partial_tiles();
    // An odd offset, so the half-buffer takes the first sample of the pass. 192 samples per
    // pixel keeps the estimator noise of the GPU and CPU twins, which trace different float
    // paths, under the tolerance. The all-tiles, outside-untouched and parity tests pin the
    // mapping itself.
    let pass = (5, 192);
    let (outcome, gpu_sum, gpu_half) = gpu_pass(&backend, &live, pass, Vec3::ZERO);
    assert_eq!(outcome, GpuAccumulate::Done);
    let (cpu_sum, cpu_half) = cpu_pass(&live, pass);
    assert_tile_totals_close("sum", &live, &gpu_sum, &cpu_sum);
    assert_tile_totals_close("half B", &live, &gpu_half, &cpu_half);
}

/// Pixels outside the live tiles keep their values bit for bit in both buffers, while the live
/// pixels (including the clipped edge tiles) do receive light.
#[test]
fn pixels_outside_the_live_tiles_are_untouched() {
    let Some(backend) = acquire_or_skip("pixels_outside_the_live_tiles_are_untouched") else {
        return;
    };
    let live = partial_tiles();
    let (outcome, accum, half_b) = gpu_pass(&backend, &live, (0, 4), SENTINEL);
    assert_eq!(outcome, GpuAccumulate::Done);
    let inside: std::collections::BTreeSet<u32> = live_pixel_indices(&live).into_iter().collect();
    let sentinel_bits = bits(&[SENTINEL])[0];
    for (pixel, (a, b)) in accum.iter().zip(&half_b).enumerate() {
        if !inside.contains(&u32::try_from(pixel).expect("a test frame is small")) {
            assert_eq!(
                bits(&[*a])[0],
                sentinel_bits,
                "accum pixel {pixel} was touched"
            );
            assert_eq!(
                bits(&[*b])[0],
                sentinel_bits,
                "half B pixel {pixel} was touched"
            );
        }
    }
    // The pass reached every live tile, clipped ones included.
    for &tile in live.indices() {
        let (x0, y0, x1, y1) = live.tile_rect(tile);
        let touched = (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| (y * FRAME.0 + x) as usize))
            .any(|p| bits(&[accum[p]])[0] != sentinel_bits);
        assert!(touched, "no pixel of live tile {tile} received a sample");
    }
}

/// `LiveTiles::all` over a range equals the ordinary accumulation over the same range: the
/// adaptive path does not change the estimator.
#[test]
fn all_tiles_equal_the_ordinary_accumulation_over_the_same_range() {
    let Some(backend) =
        acquire_or_skip("all_tiles_equal_the_ordinary_accumulation_over_the_same_range")
    else {
        return;
    };
    let (offset, spp) = (3, 8);
    let live = LiveTiles::all(FRAME.0, FRAME.1, TILE);
    let (outcome, live_sum, _) = gpu_pass(&backend, &live, (offset, spp), Vec3::ZERO);
    assert_eq!(outcome, GpuAccumulate::Done);

    let camera = camera();
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let scene = gpu_scene(&camera, &planes, &material);
    let mut ordinary = vec![Vec3::ZERO; frame_len()];
    assert!(backend.try_accumulate_geom_surface(
        &scene,
        &[],
        &SurfaceScene::IDEAL,
        offset,
        spp,
        &mut ordinary
    ));
    let diff = compare_frames(&live_sum, &ordinary, spp);
    println!("all tiles vs ordinary: {diff:?}");
    assert!(
        diff.tol_passed(ordinary.len()),
        "the live pass differs from the ordinary accumulation: {diff:?}"
    );
}

/// A pass starting at an odd `sample_offset` puts the right samples into half-buffer B: single
/// samples land in B exactly when their absolute index is odd, and a longer pass's B is the
/// sum of its odd samples.
#[test]
fn half_buffer_b_holds_exactly_the_odd_absolute_samples() {
    let Some(backend) = acquire_or_skip("half_buffer_b_holds_exactly_the_odd_absolute_samples")
    else {
        return;
    };
    let live = partial_tiles();
    let single = |offset: u32| gpu_pass(&backend, &live, (offset, 1), Vec3::ZERO);

    // One sample at an even index: all in A, none in B.
    let (outcome, even_sum, even_half) = single(2);
    assert_eq!(outcome, GpuAccumulate::Done);
    assert!(
        live_values(&even_sum, &live)
            .iter()
            .any(|p| p.length_squared() > 0.0),
        "the probe must see light, or the parity check proves nothing"
    );
    assert!(
        live_values(&even_half, &live)
            .iter()
            .all(|p| *p == Vec3::ZERO),
        "an even sample leaked into half-buffer B"
    );

    // One sample at an odd index: B equals the whole pass, bit for bit (0 + x is x).
    for offset in [1, 3, 7] {
        let (outcome, odd_sum, odd_half) = single(offset);
        assert_eq!(outcome, GpuAccumulate::Done);
        assert_eq!(bits(&odd_half), bits(&odd_sum), "offset {offset}");
    }

    // Four samples starting at an odd offset (indices 3, 4, 5, 6): B is samples 3 and 5, A =
    // total - B is samples 4 and 6.
    let (outcome, sum, half) = gpu_pass(&backend, &live, (3, 4), Vec3::ZERO);
    assert_eq!(outcome, GpuAccumulate::Done);
    let add =
        |a: &[Vec3], b: &[Vec3]| -> Vec<Vec3> { a.iter().zip(b).map(|(x, y)| *x + *y).collect() };
    let odd_expected = add(&single(3).2, &single(5).2);
    let diff_b = compare_frames(&half, &odd_expected, 2);
    assert!(
        diff_b.tol_passed(half.len()),
        "half B of a 4-sample pass: {diff_b:?}"
    );
    let all_expected = add(
        &add(&single(3).1, &single(4).1),
        &add(&single(5).1, &single(6).1),
    );
    let diff_sum = compare_frames(&sum, &all_expected, 4);
    assert!(
        diff_sum.tol_passed(sum.len()),
        "sum of a 4-sample pass: {diff_sum:?}"
    );
}

/// Two identical adaptive passes give the same result within the GPU-vs-GPU tolerance (no
/// bit-exactness, see the module doc).
#[test]
fn two_identical_live_passes_agree() {
    let Some(backend) = acquire_or_skip("two_identical_live_passes_agree") else {
        return;
    };
    let live = partial_tiles();
    let (first_outcome, first_sum, first_half) = gpu_pass(&backend, &live, (6, 16), Vec3::ZERO);
    let (second_outcome, second_sum, second_half) = gpu_pass(&backend, &live, (6, 16), Vec3::ZERO);
    assert_eq!(first_outcome, GpuAccumulate::Done);
    assert_eq!(second_outcome, GpuAccumulate::Done);
    for (label, a, b) in [
        ("sum", &first_sum, &second_sum),
        ("half B", &first_half, &second_half),
    ] {
        let diff = compare_frames(a, b, 16);
        println!("{label}: {diff:?}");
        assert!(
            diff.tol_passed(a.len()),
            "{label} differs between runs: {diff:?}"
        );
    }
}

/// A cancelled pass leaves both buffers untouched, like the ordinary cancellable entry.
#[test]
fn a_cancelled_live_pass_leaves_the_buffers_untouched() {
    let Some(backend) = acquire_or_skip("a_cancelled_live_pass_leaves_the_buffers_untouched")
    else {
        return;
    };
    let camera = camera();
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let scene = gpu_scene(&camera, &planes, &material);
    let live = partial_tiles();
    let mut accum = vec![SENTINEL; frame_len()];
    let mut half_b = vec![SENTINEL; frame_len()];
    let cancelled = AtomicBool::new(true);
    let outcome = backend.try_accumulate_geom_surface_live_cancellable(
        &scene,
        &[],
        &SurfaceScene::IDEAL,
        0,
        4,
        &live,
        &mut accum,
        &mut half_b,
        &cancelled,
    );
    assert_eq!(outcome, GpuAccumulate::Cancelled);
    assert!(accum.iter().chain(&half_b).all(|p| *p == SENTINEL));
}

/// An empty live set is a finished pass that touches nothing; tiles of another frame size are
/// declined, not traced.
#[test]
fn empty_and_mismatched_live_tiles_are_handled_without_touching_the_buffers() {
    let Some(backend) =
        acquire_or_skip("empty_and_mismatched_live_tiles_are_handled_without_touching_the_buffers")
    else {
        return;
    };
    let empty = LiveTiles::from_indices(FRAME.0, FRAME.1, TILE, Vec::new());
    let (outcome, accum, half_b) = gpu_pass(&backend, &empty, (0, 4), SENTINEL);
    assert_eq!(outcome, GpuAccumulate::Done);
    assert!(accum.iter().chain(&half_b).all(|p| *p == SENTINEL));

    let other_frame = LiveTiles::all(FRAME.0 + 8, FRAME.1, TILE);
    let (outcome, accum, half_b) = gpu_pass(&backend, &other_frame, (0, 4), SENTINEL);
    assert_eq!(outcome, GpuAccumulate::Declined);
    assert!(accum.iter().chain(&half_b).all(|p| *p == SENTINEL));
}

/// A disabled backend (no adapter needed) declines a live pass and leaves the buffers alone.
#[test]
fn a_disabled_backend_declines_a_live_pass() {
    let backend = GpuBackend::disabled();
    let live = partial_tiles();
    let (outcome, accum, half_b) = gpu_pass(&backend, &live, (0, 4), SENTINEL);
    assert_eq!(outcome, GpuAccumulate::Declined);
    assert!(accum.iter().chain(&half_b).all(|p| *p == SENTINEL));
}
