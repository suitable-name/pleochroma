//! Tests of the GPU-resident beam residual.
//!
//! The engine flag, the declines and the cancel flag run everywhere. The hardware tests (the
//! resident convergence check with a beam part against the host metric on the same sums, the
//! resident engine against the host engine within z, the restriction to a batch's tiles) skip
//! cleanly without an adapter; `PLEOCHROMA_REQUIRE_GPU=1` turns the skip into a failure. The
//! unit-level checks of the patched `adaptive_tile_error.wgsl` (naga, binding 5, the unchanged
//! block without a beam part) live in `gpu::frame::resident`'s own test module.

use std::sync::atomic::AtomicBool;

use glam::Vec3;

use super::engine::{GpuBeamEngine, batch_layout, batch_tile_noises};
use crate::{
    geometry::GpuFacetPlane,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            Camera, EnvironmentSource, build_plane_soa, surface::SurfaceScene,
            transport::PreparedMaterial,
        },
    },
    renderer::{
        adaptive::{AdaptiveRun, AdaptiveSettings, LiveTiles, NoiseMetric},
        beam::{
            BeamSettings,
            beam::{PackedKept, PackedLeaves},
            engines::{Batch, BeamEngine, EngineOutcome},
            primary::TileRect,
            tests_support::{BeamRig, MAX_BOUNCES, glass},
            tile_residual::ResidualScene,
        },
        frame_scene::FrameScene,
        gpu_backend::{GpuAccumulate, GpuBackend, GpuSceneRef},
        test_stats::{assert_z_stats, compare_streams, smooth_environment},
    },
};

/// Environment variable that turns a missing adapter into a failure.
const REQUIRE_GPU_ENV: &str = "PLEOCHROMA_REQUIRE_GPU";

/// The side of the test frame in pixels (four 16-pixel tiles).
const FRAME: u32 = 32;

/// The adaptive tile edge of the tests.
const TILE: u32 = 16;

/// Spectral nodes of `ResidualScene::n_lambda` (one group).
const NODES: u32 = 8;

/// Samples per pass of the resident check test, and its adaptive minimum.
const PASS_SPP: u32 = 16;

/// Residual samples per pixel of one engine block (also the fixed adaptive minimum and maximum).
const RESIDUAL_SPP: u32 = 64;

/// `RESIDUAL_SPP` as a float, for the means.
const RESIDUAL_SPP_F32: f32 = 64.0;

/// Blocks (independent sample ranges) per side of the z comparison.
const BLOCKS: u32 = 8;

/// The largest |z| allowed.
const Z_LIMIT: f64 = 5.0;

/// The pixels compared: a 3 x 3 grid well inside the top face of the cube.
const PIXELS: [(u32, u32); 9] = [
    (12, 12),
    (16, 12),
    (20, 12),
    (12, 16),
    (16, 16),
    (20, 16),
    (12, 20),
    (16, 20),
    (20, 20),
];

/// The relative part of the tolerance between the GPU's f32 noise and the host's f64 chain.
const NOISE_REL: f32 = 2e-2;

/// The absolute part of the same tolerance (the noise unit is a CIEDE2000 value or a ratio).
const NOISE_ABS: f32 = 1e-3;

/// The beam part's base value in the offset tests (a dim, neutral XYZ).
const OFFSET_BASE: Vec3 = Vec3::new(0.05, 0.06, 0.07);

/// How many distinct multiples of [`OFFSET_BASE`] the test offset cycles through.
const OFFSET_CYCLE: usize = 7;

fn rect(x0: u32, y0: u32, x1: u32, y1: u32) -> TileRect {
    TileRect { x0, y0, x1, y1 }
}

/// The four tiles of the test frame in row-major order.
fn tiles() -> Vec<TileRect> {
    let mut tiles = Vec::new();
    for y in (0..FRAME).step_by(TILE as usize) {
        for x in (0..FRAME).step_by(TILE as usize) {
            tiles.push(rect(x, y, x + TILE, y + TILE));
        }
    }
    tiles
}

/// A beam part for `count` pixels that differs from pixel to pixel.
fn beam_part(count: usize) -> Vec<Vec3> {
    (0..count)
        .map(|i| OFFSET_BASE * ((i % OFFSET_CYCLE) as f32 + 1.0))
        .collect()
}

/// Settings under which no tile converges before `max_spp`.
const fn never_converging(min_spp: u32, max_spp: u32) -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise: 1e-4,
        metric: NoiseMetric::RelativeLstar,
        min_spp,
        max_spp,
        pass_spp: PASS_SPP,
        tile: TILE,
    }
}

/// The one-group residual scene over the cube rig's planes.
fn with_scene<R>(
    planes: &[GpuFacetPlane],
    camera: &Camera,
    material: &GemMaterial,
    environment: EnvironmentSource<'_>,
    run: impl FnOnce(&ResidualScene<'_>) -> R,
) -> R {
    let soa = build_plane_soa(planes);
    let prepared = PreparedMaterial::new(material);
    let frame = FrameScene {
        camera,
        width: FRAME,
        height: FRAME,
        planes,
        facet_finishes: &[],
        material,
        max_bounces: MAX_BOUNCES,
        environment,
    };
    let scene = ResidualScene {
        frame: &frame,
        extras: (&[], Fluorescence::none(), SurfaceScene::IDEAL),
        plane_soa: &soa,
        prepared: &prepared,
        n_lambda: NODES,
        sample_offset: 0,
        residual_min_spp: RESIDUAL_SPP,
    };
    run(&scene)
}

/// The frame of `scene` as the GPU entries take it.
fn gpu_scene<'a>(scene: &ResidualScene<'a>) -> GpuSceneRef<'a> {
    let frame = scene.frame;
    GpuSceneRef {
        camera: frame.camera,
        width: frame.width,
        height: frame.height,
        planes: frame.planes,
        facet_finishes: frame.facet_finishes,
        material: frame.material,
        max_bounces: frame.max_bounces,
        environment: frame.environment,
        estimator: None,
    }
}

/// A backend with a renderer, or `None` (the caller returns, skipping its test) without an
/// adapter.
fn backend_or_skip(test: &str) -> Option<GpuBackend> {
    let backend = GpuBackend::acquire();
    if backend.adapter_label().is_some() {
        return Some(backend);
    }
    assert!(
        !std::env::var(REQUIRE_GPU_ENV).is_ok_and(|value| value == "1"),
        "{test}: {REQUIRE_GPU_ENV}=1 but no GPU adapter could be acquired"
    );
    println!("skipping {test}: no GPU adapter");
    None
}

/// A batch of `tiles` with no leaves and an empty kept set.
fn empty_batch<'b>(
    tiles: &'b [TileRect],
    packed: &'b PackedLeaves,
    kept: &'b PackedKept,
) -> Batch<'b> {
    Batch {
        index: 0,
        tiles,
        leaves: &[],
        kept: &[],
        packed_leaves: packed,
        packed_kept: kept,
    }
}

/// The resident check of one first pass over all four tiles with `offset` as the beam part (empty:
/// none): the GPU's tile noises, and the host metric's over the very sums the GPU read back.
/// `None` when the resident run could not start or the pass declined (the callers then fail).
fn resident_check(backend: &GpuBackend, offset: &[Vec3]) -> Option<(Vec<f32>, Vec<f32>)> {
    let rig = BeamRig::cube(glass());
    let tiles = tiles();
    let layout = batch_layout(&tiles, (FRAME, FRAME), TILE).expect("the tiles are the grid");
    let starts: Vec<(u32, u32)> = layout
        .iter()
        .map(|tile| (tile.index, u32::try_from(tile.start).expect("few pixels")))
        .collect();
    let settings = never_converging(PASS_SPP, 2 * PASS_SPP);
    let kept = PackedKept::default();
    let never = AtomicBool::new(false);
    with_scene(
        &rig.planes,
        &rig.camera,
        &rig.material,
        smooth_environment(),
        |scene| {
            let live = LiveTiles::from_indices(
                FRAME,
                FRAME,
                TILE,
                layout.iter().map(|tile| tile.index).collect(),
            );
            let mut run = AdaptiveRun::new(settings, FRAME, FRAME)
                .expect("valid settings")
                .restricted_to(&live);
            let frame = gpu_scene(scene);
            let resident = backend.try_adaptive_resident(
                &frame,
                scene.extras.0,
                &scene.extras.2,
                settings.pass_spp,
            )?;
            let mut resident = resident
                .restricted_to(&starts, offset, (0, FRAME))?
                .with_reference_kept(&kept, NODES);
            let pass = run.next_pass().expect("a first pass");
            if resident.pass(&pass, &never) != GpuAccumulate::Done {
                return None;
            }
            let check = run.pending_check().expect("the minimum is one pass");
            let gpu = resident.tile_errors(&check, 1.0, settings.metric)?;
            let (sum, half_b) = resident.finish_rows()?;
            let host = batch_tile_noises(
                &check,
                &layout,
                offset,
                (&sum, &half_b),
                FRAME,
                (1.0, settings.metric),
            );
            Some((gpu, host))
        },
    )
}

// ---- no adapter needed ----

#[test]
fn the_resident_path_is_off_until_asked_for() {
    let rig = BeamRig::cube(glass());
    let backend = GpuBackend::disabled();
    with_scene(
        &rig.planes,
        &rig.camera,
        &rig.material,
        smooth_environment(),
        |scene| {
            let engine = GpuBeamEngine::new(
                &backend,
                scene,
                &BeamSettings::default(),
                &never_converging(PASS_SPP, PASS_SPP),
            );
            assert!(!engine.uses_gpu_convergence());
            let engine = engine.with_gpu_convergence(true);
            assert!(engine.uses_gpu_convergence());
            let engine = engine.with_gpu_convergence(false);
            assert!(!engine.uses_gpu_convergence());
        },
    );
}

#[test]
fn a_resident_engine_on_a_disabled_backend_declines_and_a_set_cancel_flag_cancels() {
    let rig = BeamRig::cube(glass());
    let backend = GpuBackend::disabled();
    let tiles = tiles();
    let packed = PackedLeaves::default();
    let kept = PackedKept::default();
    let batch = empty_batch(&tiles, &packed, &kept);
    let offset = beam_part(batch.pixel_count());
    with_scene(
        &rig.planes,
        &rig.camera,
        &rig.material,
        smooth_environment(),
        |scene| {
            let engine = GpuBeamEngine::new(
                &backend,
                scene,
                &BeamSettings::default(),
                &never_converging(PASS_SPP, PASS_SPP),
            )
            .with_gpu_convergence(true);
            let never = AtomicBool::new(false);
            // The resident run cannot start; the host path then declines too (no device), and the
            // batch goes to the next engine.
            match engine.residual(&batch, &offset, &never) {
                EngineOutcome::Declined(reason) => assert_ne!(reason, ""),
                other => panic!("a disabled backend must decline the residual, got {other:?}"),
            }
            let cancelled = AtomicBool::new(true);
            assert_eq!(
                engine.residual(&batch, &offset, &cancelled),
                EngineOutcome::Cancelled
            );
        },
    );
}

// ---- hardware ----

/// The GPU's convergence check with a beam part against the host metric over the same sums: per
/// tile within `NOISE_REL` / `NOISE_ABS`; the beam part changes the noise the check sees.
#[test]
fn the_gpu_check_with_a_beam_part_matches_the_host_metric() {
    let Some(backend) = backend_or_skip("the_gpu_check_with_a_beam_part_matches_the_host_metric")
    else {
        return;
    };
    let offset = beam_part(usize::try_from(FRAME * FRAME).expect("small"));
    // An adapter was acquired, so the resident path must run: no silent skip.
    let (gpu, host) = resident_check(&backend, &offset)
        .expect("the resident beam residual must run on an acquired adapter");
    let (gpu_plain, _) = resident_check(&backend, &[])
        .expect("the resident run without a beam part must run on an acquired adapter");
    eprintln!(
        "tile noise with a beam part: GPU {gpu:?}, host {host:?}; without: GPU {gpu_plain:?}"
    );
    assert_eq!(gpu.len(), host.len());
    assert_eq!(gpu.len(), tiles().len());
    for (index, (g, h)) in gpu.iter().zip(&host).enumerate() {
        assert!(
            (g - h).abs() <= NOISE_REL.mul_add(h.abs(), NOISE_ABS),
            "tile {index}: GPU {g} against host {h}"
        );
    }
    // A beam part brightens both halves alike, so it lowers the relative noise of a lit tile.
    assert!(
        gpu.iter()
            .zip(&gpu_plain)
            .any(|(with, without)| with != without),
        "the beam part left every tile's noise unchanged"
    );
}

/// With no beam part the kernel's block is the shipped one: the GPU check agrees with the host
/// metric, and an all-zero beam part gives the very same noises bit for bit.
#[test]
fn without_a_beam_part_the_gpu_check_is_the_shipped_metric() {
    let Some(backend) = backend_or_skip("without_a_beam_part_the_gpu_check_is_the_shipped_metric")
    else {
        return;
    };
    let (gpu, host) = resident_check(&backend, &[])
        .expect("the resident beam residual must run on an acquired adapter");
    for (index, (g, h)) in gpu.iter().zip(&host).enumerate() {
        assert!(
            (g - h).abs() <= NOISE_REL.mul_add(h.abs(), NOISE_ABS),
            "tile {index}: GPU {g} against host {h}"
        );
    }
    let zeros = vec![Vec3::ZERO; usize::try_from(FRAME * FRAME).expect("small")];
    let (zero_gpu, _) = resident_check(&backend, &zeros)
        .expect("the resident run with a zero beam part must run on an acquired adapter");
    assert_eq!(
        gpu.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        zero_gpu.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        "a zero beam part changed the shipped metric's output"
    );
}

/// A run restricted to one tile refuses a pass over other tiles, and is dead afterwards.
#[test]
fn a_restricted_run_refuses_tiles_outside_its_batch() {
    let Some(backend) = backend_or_skip("a_restricted_run_refuses_tiles_outside_its_batch") else {
        return;
    };
    let rig = BeamRig::cube(glass());
    let settings = never_converging(PASS_SPP, PASS_SPP);
    let never = AtomicBool::new(false);
    with_scene(
        &rig.planes,
        &rig.camera,
        &rig.material,
        smooth_environment(),
        |scene| {
            let mut all = AdaptiveRun::new(settings, FRAME, FRAME).expect("valid settings");
            let pass = all.next_pass().expect("a first pass over every tile");
            let frame = gpu_scene(scene);
            let resident = backend
                .try_adaptive_resident(&frame, scene.extras.0, &scene.extras.2, PASS_SPP)
                .expect("a resident run must start on an acquired adapter");
            let mut resident = resident
                .restricted_to(&[(0, 0)], &[], (0, TILE))
                .expect("a one-tile batch");
            assert_eq!(resident.pass(&pass, &never), GpuAccumulate::Declined);
            assert_eq!(resident.pass(&pass, &never), GpuAccumulate::Declined);
            assert!(resident.finish_rows().is_none());
        },
    );
}

/// Malformed batches never start a restricted run: tiles out of order, empty or oversized rows.
#[test]
fn a_malformed_batch_gives_no_restricted_run() {
    let Some(backend) = backend_or_skip("a_malformed_batch_gives_no_restricted_run") else {
        return;
    };
    let rig = BeamRig::cube(glass());
    with_scene(
        &rig.planes,
        &rig.camera,
        &rig.material,
        smooth_environment(),
        |scene| {
            let frame = gpu_scene(scene);
            let start =
                || backend.try_adaptive_resident(&frame, scene.extras.0, &scene.extras.2, PASS_SPP);
            let first = start().expect("a resident run must start on an acquired adapter");
            assert!(
                first
                    .restricted_to(&[(1, 0), (0, 4)], &[], (0, FRAME))
                    .is_none()
            );
            let second = start().expect("a second resident run must start");
            assert!(second.restricted_to(&[(0, 0)], &[], (4, 4)).is_none());
            let third = start().expect("a third resident run must start");
            assert!(
                third
                    .restricted_to(&[(0, 0)], &[], (0, FRAME + 1))
                    .is_none()
            );
        },
    );
}

/// The pixel index of `(x, y)` of the test frame in the batch layout of the four tiles.
const fn batch_index(x: u32, y: u32) -> usize {
    let tile = (y / TILE) * (FRAME / TILE) + x / TILE;
    (tile * TILE * TILE + (y % TILE) * TILE + x % TILE) as usize
}

/// The resident engine against the host engine on the same batch (an empty kept set, a beam part):
/// per-pixel block means agree within z, and the counts are the fixed spp.
///
/// The engine records which path ran (`resident_completions`, `resident_fallbacks`), and the
/// test requires resident completion, so a silent host fallback fails instead of comparing host
/// against host.
#[test]
fn the_resident_engine_equals_the_host_engine_within_z() {
    let Some(backend) = backend_or_skip("the_resident_engine_equals_the_host_engine_within_z")
    else {
        return;
    };
    let rig = BeamRig::cube(glass());
    let tiles = tiles();
    let packed = PackedLeaves::default();
    let kept = PackedKept::default();
    let batch = empty_batch(&tiles, &packed, &kept);
    let offset = beam_part(batch.pixel_count());
    let adaptive = never_converging(RESIDUAL_SPP, RESIDUAL_SPP);
    let never = AtomicBool::new(false);
    let mut resident_blocks: Vec<Vec<Vec3>> = Vec::new();
    let mut host_blocks: Vec<Vec<Vec3>> = Vec::new();
    for block in 0..BLOCKS {
        with_scene(
            &rig.planes,
            &rig.camera,
            &rig.material,
            smooth_environment(),
            |base| {
                let scene = ResidualScene {
                    sample_offset: block * RESIDUAL_SPP,
                    ..*base
                };
                let mut means = Vec::new();
                for resident in [true, false] {
                    let engine =
                        GpuBeamEngine::new(&backend, &scene, &BeamSettings::default(), &adaptive)
                            .with_gpu_convergence(resident);
                    let EngineOutcome::Done(result) = engine.residual(&batch, &offset, &never)
                    else {
                        panic!("the engine did not run the residual of block {block}");
                    };
                    // Record which path ran: the resident engine must complete resident, the
                    // host engine never takes the resident path.
                    if resident {
                        assert_eq!(engine.resident_completions(), 1, "block {block}");
                        assert_eq!(engine.resident_fallbacks(), 0, "block {block}");
                    } else {
                        assert_eq!(engine.resident_completions(), 0, "block {block}");
                    }
                    assert_eq!(result.counts.len(), batch.pixel_count());
                    assert!(result.counts.iter().all(|&c| c == RESIDUAL_SPP));
                    means.push(
                        PIXELS
                            .iter()
                            .map(|&(x, y)| result.sum[batch_index(x, y)] / RESIDUAL_SPP_F32)
                            .collect::<Vec<Vec3>>(),
                    );
                }
                host_blocks.push(means.pop().expect("host means"));
                resident_blocks.push(means.pop().expect("resident means"));
            },
        );
    }
    let zs = compare_streams(
        PIXELS.len(),
        (BLOCKS, BLOCKS),
        |k, i| host_blocks[i as usize][k],
        |k, i| resident_blocks[i as usize][k],
    );
    assert_z_stats(
        &zs,
        Z_LIMIT,
        "resident beam residual against the host-convergence residual",
    );
}
