//! Native GPU tests of adaptive (live-tile) passes on the event wavefront renderer.
//!
//! Skips cleanly if no GPU adapter is available, unless `PLEOCHROMA_REQUIRE_GPU=1` is set. The
//! pass contract under test (`renderer::adaptive`): samples `sample_offset..sample_offset + spp`
//! are added to every pixel of the live tiles, into the summed accumulator and into half-buffer
//! B, which receives only the samples with an odd absolute index; other pixels are untouched.
//! GPU-vs-GPU comparisons use the golden tolerance (`compare_frames`), never bit-exactness.

use glam::Vec3;

use super::{EventWavefrontRenderer, EwError, compare::compare_frames};
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset},
    },
    renderer::{
        adaptive::LiveTiles,
        frame_scene::FrameScene,
        gpu::{GpuContext, frame::live_pixel_indices},
    },
};

const FRAME: (u32, u32) = (40, 36);
const TILE: u32 = 16;
const SENTINEL: Vec3 = Vec3::new(7.25, -3.5, 1.0e-3);

fn renderer_or_skip(test: &str) -> Option<EventWavefrontRenderer> {
    match GpuContext::acquire() {
        Ok(ctx) => Some(EventWavefrontRenderer::new(ctx)),
        Err(error) => {
            assert!(
                !std::env::var("PLEOCHROMA_REQUIRE_GPU").is_ok_and(|v| v == "1"),
                "{test}: PLEOCHROMA_REQUIRE_GPU=1 but no GPU adapter could be acquired ({error})"
            );
            println!("skipping {test}: no GPU adapter ({error})");
            None
        }
    }
}

/// Four tiles of a 40 x 36 frame at tile 16, three of them clipped at the right or bottom edge.
fn partial_tiles() -> LiveTiles {
    LiveTiles::from_indices(FRAME.0, FRAME.1, TILE, vec![1, 5, 7, 8])
}

fn frame_len() -> usize {
    (FRAME.0 * FRAME.1) as usize
}

/// One live pass into buffers pre-filled with `fill`.
fn live_pass(
    renderer: &mut EventWavefrontRenderer,
    live: &LiveTiles,
    (sample_offset, spp): (u32, u32),
    fill: Vec3,
) -> Result<(Vec<Vec3>, Vec<Vec3>), EwError> {
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in material");
    let scene = FrameScene {
        camera: &camera,
        width: FRAME.0,
        height: FRAME.1,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 6,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };
    let mut sum = vec![fill; frame_len()];
    let mut half_b = vec![fill; frame_len()];
    pollster::block_on(renderer.accumulate_live_with_tools(
        &scene,
        &[],
        live,
        sample_offset,
        spp,
        &mut sum,
        &mut half_b,
    ))?;
    Ok((sum, half_b))
}

/// `LiveTiles::all` over a range equals the ordinary accumulation over the same range.
#[test]
fn ew_all_tiles_equal_the_ordinary_accumulation() {
    let Some(mut renderer) = renderer_or_skip("ew_all_tiles_equal_the_ordinary_accumulation")
    else {
        return;
    };
    let (offset, spp) = (3, 4);
    let live = LiveTiles::all(FRAME.0, FRAME.1, TILE);
    let (live_sum, _) =
        live_pass(&mut renderer, &live, (offset, spp), Vec3::ZERO).expect("live pass");

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in material");
    let scene = FrameScene {
        camera: &camera,
        width: FRAME.0,
        height: FRAME.1,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 6,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };
    let mut ordinary = vec![Vec3::ZERO; frame_len()];
    pollster::block_on(renderer.accumulate(&scene, offset, spp, &mut ordinary))
        .expect("ordinary pass");
    let diff = compare_frames(&live_sum, &ordinary, spp);
    println!("event wavefront, all tiles vs ordinary: {diff:?}");
    assert!(
        diff.tol_passed(ordinary.len()),
        "the live pass differs from the ordinary accumulation: {diff:?}"
    );
}

/// Pixels outside the live tiles keep their values bit for bit in both buffers.
#[test]
fn ew_pixels_outside_the_live_tiles_are_untouched() {
    let Some(mut renderer) = renderer_or_skip("ew_pixels_outside_the_live_tiles_are_untouched")
    else {
        return;
    };
    let live = partial_tiles();
    let (sum, half_b) = live_pass(&mut renderer, &live, (0, 2), SENTINEL).expect("live pass");
    let inside: std::collections::BTreeSet<u32> = live_pixel_indices(&live).into_iter().collect();
    let same = |p: Vec3| p.to_array().map(f32::to_bits) == SENTINEL.to_array().map(f32::to_bits);
    for (pixel, (a, b)) in sum.iter().zip(&half_b).enumerate() {
        let live_pixel = inside.contains(&u32::try_from(pixel).expect("a test frame is small"));
        if !live_pixel {
            assert!(same(*a), "sum pixel {pixel} was touched");
            assert!(same(*b), "half B pixel {pixel} was touched");
        }
    }
    assert!(
        inside.iter().any(|&p| !same(sum[p as usize])),
        "no live pixel received a sample"
    );
}

/// A single sample lands in half-buffer B exactly when its absolute index is odd, including for
/// a pass that starts at an odd `sample_offset`.
#[test]
fn ew_half_buffer_b_holds_exactly_the_odd_absolute_samples() {
    let Some(mut renderer) =
        renderer_or_skip("ew_half_buffer_b_holds_exactly_the_odd_absolute_samples")
    else {
        return;
    };
    let live = partial_tiles();
    let (even_sum, even_half) =
        live_pass(&mut renderer, &live, (2, 1), Vec3::ZERO).expect("even sample");
    assert!(
        even_sum.iter().any(|p| p.length_squared() > 0.0),
        "the probe must see light"
    );
    assert!(
        even_half.iter().all(|p| *p == Vec3::ZERO),
        "an even sample leaked into half-buffer B"
    );
    for offset in [1, 3] {
        let (odd_sum, odd_half) =
            live_pass(&mut renderer, &live, (offset, 1), Vec3::ZERO).expect("odd sample");
        let bits = |pixels: &[Vec3]| -> Vec<[u32; 3]> {
            pixels
                .iter()
                .map(|p| p.to_array().map(f32::to_bits))
                .collect()
        };
        assert_eq!(bits(&odd_half), bits(&odd_sum), "offset {offset}");
    }
}

/// Two identical live passes agree within the GPU-vs-GPU tolerance.
#[test]
fn ew_two_identical_live_passes_agree() {
    let Some(mut renderer) = renderer_or_skip("ew_two_identical_live_passes_agree") else {
        return;
    };
    let live = partial_tiles();
    let first = live_pass(&mut renderer, &live, (6, 8), Vec3::ZERO).expect("first pass");
    let second = live_pass(&mut renderer, &live, (6, 8), Vec3::ZERO).expect("second pass");
    for (label, a, b) in [
        ("sum", &first.0, &second.0),
        ("half B", &first.1, &second.1),
    ] {
        let diff = compare_frames(a, b, 8);
        assert!(
            diff.tol_passed(a.len()),
            "{label} differs between runs: {diff:?}"
        );
    }
}

/// Buffers of the wrong size and tiles of another frame are refused before anything is traced.
#[test]
fn ew_live_pass_refuses_mismatched_sizes() {
    let Some(mut renderer) = renderer_or_skip("ew_live_pass_refuses_mismatched_sizes") else {
        return;
    };
    let other_frame = LiveTiles::all(FRAME.0 + 8, FRAME.1, TILE);
    let error = live_pass(&mut renderer, &other_frame, (0, 1), Vec3::ZERO)
        .expect_err("tiles of another frame size");
    assert!(
        matches!(error, EwError::InvalidBufferSize { .. }),
        "{error:?}"
    );
}
