//! Hardware tests of adaptive (live-tile) passes on [`GpuFrameRenderer`] itself: chunking and
//! turn resumption, and the declines of the legacy wavefront. Like `gpu_hardware_tests`, each
//! test acquires its own renderer through
//! [`renderer_or_skip`](crate::renderer::gpu::merge_tests::renderer_or_skip), which skips with
//! a printed note when no adapter exists (`PLEOCHROMA_REQUIRE_GPU=1` turns that into a failure).
//! The backend-level pass contract tests live in `renderer::gpu_backend::live_tests`.

use glam::Vec3;

use super::{
    ChunkCursor, ChunkTurnOutcome, GpuFrameError, GpuFrameRenderer, GpuFrameScene, GpuPipelineKind,
    LivePass, LiveSums, TurnRequest, classify_material,
};
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset},
    },
    renderer::{
        adaptive::LiveTiles,
        gpu::{event_wavefront::compare::compare_frames, merge_tests::renderer_or_skip},
    },
};

const FRAME: (u32, u32) = (40, 36);

/// Interior tile plus the three clipped edge tiles of a 40 x 36 frame at tile 16.
fn live_tiles() -> LiveTiles {
    LiveTiles::from_indices(FRAME.0, FRAME.1, 16, vec![1, 5, 7, 8])
}

/// Runs one live pass to completion, `max_chunks` chunks per turn, and returns the compact
/// summed and half-buffer sums (one entry per live pixel).
fn run_pass(
    renderer: &mut GpuFrameRenderer,
    max_chunks: usize,
    (sample_offset, spp): (u32, u32),
) -> Result<(Vec<Vec3>, Vec<Vec3>), GpuFrameError> {
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let scene = GpuFrameScene {
        camera: &camera,
        width: FRAME.0,
        height: FRAME.1,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 6,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };
    let request = TurnRequest {
        scene: &scene,
        tools: &[],
        pipeline_class: classify_material(&material),
        sample_offset,
        spp,
        cancel: None,
        max_chunks,
        surface: None,
        estimator: None,
    };
    let pass: LivePass = renderer.begin_live_pass(&live_tiles())?;
    let live_count = pass.pixels().len();
    let mut sum = vec![Vec3::ZERO; live_count];
    let mut half_b = vec![Vec3::ZERO; live_count];
    let mut cursor = ChunkCursor::default();
    loop {
        let outcome = renderer.accumulate_live_turn(
            &request,
            &pass,
            &mut LiveSums {
                sum: &mut sum,
                half_b: &mut half_b,
            },
            &mut cursor,
        )?;
        match outcome {
            ChunkTurnOutcome::Done => return Ok((sum, half_b)),
            ChunkTurnOutcome::Cancelled => panic!("nothing cancels this pass"),
            ChunkTurnOutcome::MoreWork => {}
        }
    }
}

/// A live pass cut into many small chunks and resumed one chunk per turn equals the same pass
/// in big chunks: chunk boundaries and turn boundaries change nothing.
#[test]
fn a_live_pass_in_many_chunks_and_turns_equals_one_in_big_chunks() {
    let name = "a_live_pass_in_many_chunks_and_turns_equals_one_in_big_chunks";
    let Some(mut whole) = renderer_or_skip(name) else {
        return;
    };
    let Some(mut pieces) = renderer_or_skip(name) else {
        return;
    };
    let pass = (3, 4);
    let (whole_sum, whole_half) = run_pass(&mut whole, usize::MAX, pass).expect("one big turn");
    // 64 tuples per chunk is 16 pixels at 4 spp: the live set takes dozens of chunks, resumed
    // one per turn.
    pieces.set_chunk_budget_bytes(12 * 64);
    let (piece_sum, piece_half) = run_pass(&mut pieces, 1, pass).expect("many small turns");
    for (label, a, b) in [
        ("sum", &whole_sum, &piece_sum),
        ("half B", &whole_half, &piece_half),
    ] {
        let diff = compare_frames(a, b, pass.1);
        println!("{label}: {diff:?}");
        assert!(
            diff.tol_passed(a.len()),
            "{label} changed with the chunking: {diff:?}"
        );
    }
    assert!(
        whole_sum.iter().any(|p| p.length_squared() > 0.0),
        "the probe must see light"
    );
}

/// The legacy wavefront kernels have no live variant: a renderer set to them declines a live
/// pass with [`GpuFrameError::LivePassUnavailable`] instead of tracing a wrong one.
#[test]
fn the_legacy_wavefront_declines_a_live_pass() {
    let Some(mut renderer) = renderer_or_skip("the_legacy_wavefront_declines_a_live_pass") else {
        return;
    };
    renderer.set_pipeline_kind(GpuPipelineKind::Wavefront);
    if renderer.pipeline_kind() != GpuPipelineKind::Wavefront {
        println!("skipping the wavefront decline: this device stays on the megakernel");
        return;
    }
    let result = run_pass(&mut renderer, usize::MAX, (0, 2));
    assert!(
        matches!(result, Err(GpuFrameError::LivePassUnavailable(_))),
        "expected LivePassUnavailable, got {result:?}"
    );
}
