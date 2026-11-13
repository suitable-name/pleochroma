//! Tests of the GPU beam engine.
//!
//! The batch-layout, settings and metric helpers, the cancel flag and the declines (a disabled
//! backend, a retired engine, a batch off the tile grid) run everywhere. The hardware tests (the
//! fragments against the CPU, the residual against the CPU `run_batch_residual`, the determinism
//! of the pooled buffers, the device-loss failover) skip cleanly without an adapter;
//! `PLEOCHROMA_REQUIRE_GPU=1` turns the skip into a failure.
//!
//! The thresholds and sample counts are named constants; the tests print the measured values they
//! are compared with.

use std::{borrow::Cow, sync::atomic::AtomicBool};

use glam::{DVec2, DVec3, Vec3};

use super::engine::{
    GpuBeamEngine, PixelSlots, batch_layout, batch_order, batch_tile_noises, clear_rects,
    compact_tile_noises, residual_settings,
};
use crate::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            Camera, EnvironmentSource, LightingPreset, build_plane_soa, surface::SurfaceScene,
            transport::PreparedMaterial,
        },
        studio_rig::StudioRig,
    },
    renderer::{
        adaptive::{
            AdaptiveSettings, LiveTiles, NoiseMetric, PendingCheck, metric::live_tile_noises,
        },
        beam::{
            BeamSettings,
            beam::{
                GpuLeafHeader, GpuLeafNode, GpuLeafVertex, LEAF_FLAG_REFLECTION, Leaf, PackedKept,
                PackedLeaves, SpectralInterval,
            },
            engines::{Batch, BatchFragments, BatchResidual, BeamEngine, EngineOutcome},
            geom::polygon::ConvexPolygon,
            lighting::LightingScene,
            primary::TileRect,
            raster::shade_tile,
            stats::BeamStats,
            tests_support::{BeamRig, MAX_BOUNCES, glass},
            tile_residual::{ResidualScene, run_batch_residual},
        },
        frame_scene::FrameScene,
        gpu_backend::GpuBackend,
        path_cache::record::{f16_bits_to_f32, f32_to_f16_bits},
        test_stats::{assert_z_stats, compare_streams, smooth_environment},
    },
};

/// Environment variable that turns a missing adapter into a failure.
const REQUIRE_GPU_ENV: &str = "PLEOCHROMA_REQUIRE_GPU";

/// Relative GPU-vs-CPU tolerance of a fragment's XYZ (the value of `FRAGMENT_TWIN_REL` in
/// `tests_fragments`).
const FRAGMENT_TWIN_REL: f32 = 2e-3;

/// The side of the fragment test frame in pixels (one partial tile of 32).
const FRAGMENT_FRAME: u32 = 24;

/// Spectral nodes of the test leaf and of `ResidualScene::n_lambda` (one group).
const NODES: u32 = 8;

/// The side of the residual test frame in pixels.
const RESIDUAL_FRAME: u32 = 32;

/// The adaptive tile edge of the tests.
const TILE: u32 = 16;

/// Residual samples per pixel of one block (also the fixed adaptive minimum and maximum).
const RESIDUAL_SPP: u32 = 64;

/// `RESIDUAL_SPP` as a float, for the means.
const RESIDUAL_SPP_F32: f32 = 64.0;

/// Blocks (independent sample ranges) per side of the z comparison.
const BLOCKS: u32 = 8;

/// `PIXELS.len()` as a float, for the means.
const PIXEL_COUNT_F64: f64 = 9.0;

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

fn rect(x0: u32, y0: u32, x1: u32, y1: u32) -> TileRect {
    TileRect { x0, y0, x1, y1 }
}

/// `value` rounded to single precision.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the test narrows an f64 weight to the f32 the GPU buffers hold"
)]
const fn to_f32(value: f64) -> f32 {
    value as f32
}

// ---- pure CPU: layout, order, settings, metric -------------------------------------------------

#[test]
fn the_batch_order_is_tiles_in_order_each_row_major() {
    let width = 8;
    let image: Vec<Vec3> = (0..48_u8).map(|i| Vec3::splat(f32::from(i))).collect();
    let tiles = [rect(4, 0, 8, 3), rect(0, 3, 4, 6)];
    let ordered = batch_order(&image, width, &tiles).expect("both tiles lie inside the image");
    let values: Vec<f32> = ordered.iter().map(|p| p.x).collect();
    let expected: Vec<f32> = [
        [4_u8, 5, 6, 7],
        [12, 13, 14, 15],
        [20, 21, 22, 23],
        [24, 25, 26, 27],
        [32, 33, 34, 35],
        [40, 41, 42, 43],
    ]
    .into_iter()
    .flatten()
    .map(f32::from)
    .collect();
    assert_eq!(values, expected);
    assert_eq!(ordered.len(), 6 * 4);
}

#[test]
fn the_batch_order_refuses_tiles_outside_the_image_and_accepts_no_tiles() {
    let image = vec![Vec3::ONE; 48];
    assert_eq!(batch_order(&image, 8, &[rect(6, 0, 10, 2)]), None);
    assert_eq!(batch_order(&image, 8, &[rect(0, 5, 4, 7)]), None);
    assert_eq!(batch_order(&image, 8, &[]), Some(Vec::new()));
}

#[test]
fn clearing_rectangles_zeroes_exactly_those_pixels() {
    let mut image = vec![Vec3::ONE; 16];
    clear_rects(&mut image, 4, [[1, 1, 3, 2]]);
    for (index, pixel) in image.iter().enumerate() {
        let inside = index == 5 || index == 6;
        assert_eq!(*pixel == Vec3::ZERO, inside, "pixel {index}");
    }
    // A rectangle that leaves the image changes nothing and does not panic.
    let before = image.clone();
    clear_rects(&mut image, 4, [[0, 5, 4, 7]]);
    assert_eq!(image, before);
}

#[test]
fn the_residual_settings_are_valid_runs_with_the_residual_minimum() {
    let adaptive = AdaptiveSettings::provisional(512);
    assert_eq!(
        (adaptive.min_spp, adaptive.max_spp, adaptive.pass_spp),
        (512, 512, 16)
    );
    // (residual minimum, expected minimum, expected pass): the pass is the minimum, whatever the
    // adaptive pass was.
    for (wanted, min_spp, pass_spp) in [
        (64, 64, 64),
        (100, 100, 100),
        (10, 10, 10),
        (0, 1, 1),
        (9999, 512, 512),
    ] {
        let settings = residual_settings(&adaptive, wanted);
        assert_eq!(
            (settings.min_spp, settings.max_spp, settings.pass_spp),
            (min_spp, 512, pass_spp),
            "residual minimum {wanted}"
        );
        assert_eq!(settings.validate(), Ok(()), "residual minimum {wanted}");
        assert_eq!(settings.tile, adaptive.tile);
        assert_eq!(settings.metric, adaptive.metric);
        assert_eq!(
            settings.target_noise.to_bits(),
            adaptive.target_noise.to_bits()
        );
    }
}

#[test]
fn a_two_tile_request_at_opposite_corners_of_8k_plans_compact_sizes() {
    // The batch is two tiles of the 64-pixel grid; its compact slots are those pixels, not the
    // frame's 33 million. 4320 rows are 67 full tile rows and a last one of
    // 32 rows from 4288, so the bottom-right grid tile is 64 x 32.
    let size = (7680, 4320);
    let tiles = [rect(0, 0, 64, 64), rect(7616, 4288, 7680, 4320)];
    let layout = batch_layout(&tiles, size, 64).expect("grid tiles");
    let plan = super::engine::scratch_plan(&layout, size);
    assert_eq!(plan.compact, 64 * 64 + 64 * 32);
    assert_eq!(plan.frame, 7680 * 4320);
    assert_eq!(plan.band, plan.frame);
    assert_eq!(layout[1].start, 64 * 64);
    // One tile: its band is its own rows, and an empty batch plans to nothing.
    let one = batch_layout(&tiles[..1], size, 64).expect("a grid tile");
    let plan = super::engine::scratch_plan(&one, size);
    assert_eq!((plan.compact, plan.band), (64 * 64, 64 * 7680));
    assert_eq!(
        super::engine::scratch_plan(&[], size),
        super::engine::ScratchPlan {
            compact: 0,
            band: 0,
            frame: 7680 * 4320
        }
    );
}

#[test]
fn the_batch_layout_numbers_the_tiles_of_the_adaptive_grid() {
    // 40 x 40 with 16-pixel tiles: a 3 x 3 grid whose last row and column are 8 pixels wide.
    let tiles = [rect(16, 16, 32, 32), rect(32, 32, 40, 40)];
    let layout = batch_layout(&tiles, (40, 40), 16).expect("both tiles are grid tiles");
    assert_eq!(layout.len(), 2);
    assert_eq!((layout[0].index, layout[0].start), (4, 0));
    assert_eq!((layout[1].index, layout[1].start), (8, 256));
    assert_eq!(layout[1].rect, tiles[1]);
    assert_eq!(batch_layout(&[], (40, 40), 16), Some(Vec::new()));
}

#[test]
fn a_batch_off_the_grid_or_out_of_order_has_no_layout() {
    let size = (40, 40);
    // Origin off the grid, wrong edge, empty, unordered and repeated tiles.
    assert_eq!(batch_layout(&[rect(8, 0, 24, 16)], size, 16), None);
    assert_eq!(batch_layout(&[rect(0, 0, 8, 16)], size, 16), None);
    assert_eq!(batch_layout(&[rect(32, 0, 32, 16)], size, 16), None);
    assert_eq!(
        batch_layout(&[rect(16, 0, 32, 16), rect(0, 0, 16, 16)], size, 16),
        None
    );
    assert_eq!(
        batch_layout(&[rect(0, 0, 16, 16), rect(0, 0, 16, 16)], size, 16),
        None
    );
}

/// 8 x 8 residual sums whose two halves differ: means 0.1 and 0.2 over 64 samples, 32 odd.
fn noisy_tile() -> (Vec<Vec3>, Vec<Vec3>, PendingCheck) {
    let sum = vec![Vec3::splat(9.6); 64];
    let half_b = vec![Vec3::splat(6.4); 64];
    let check = PendingCheck {
        live: LiveTiles::all(8, 8, 8),
        samples: 64,
        odd_samples: 32,
    };
    (sum, half_b, check)
}

fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|v| v.to_bits()).collect()
}

#[test]
fn a_zero_offset_gives_the_noises_of_the_plain_metric_bit_for_bit() {
    let (sum, half_b, check) = noisy_tile();
    let layout = batch_layout(&[rect(0, 0, 8, 8)], (8, 8), 8).expect("one tile");
    let zero = vec![Vec3::ZERO; 64];
    for metric in [NoiseMetric::RelativeLstar, NoiseMetric::AbsoluteDeltaE] {
        let plain = live_tile_noises(&sum, &half_b, &check.live, (64, 32), 1.0, metric);
        let with = batch_tile_noises(&check, &layout, &zero, (&sum, &half_b), 8, (1.0, metric));
        assert_eq!(with.len(), 1);
        assert!(plain[0] > 0.0, "the halves differ, so the tile has noise");
        assert_eq!(bits(&with), bits(&plain), "{metric:?}");
        // An empty offset slice means no beam part at all.
        let none = batch_tile_noises(&check, &layout, &[], (&sum, &half_b), 8, (1.0, metric));
        assert_eq!(bits(&none), bits(&plain), "{metric:?}");
    }
}

#[test]
fn the_beam_part_changes_the_noise_the_check_sees() {
    let (sum, half_b, check) = noisy_tile();
    let layout = batch_layout(&[rect(0, 0, 8, 8)], (8, 8), 8).expect("one tile");
    let metric = (1.0, NoiseMetric::AbsoluteDeltaE);
    let zero = vec![Vec3::ZERO; 64];
    let beams = vec![Vec3::splat(0.05); 64];
    let without = batch_tile_noises(&check, &layout, &zero, (&sum, &half_b), 8, metric);
    let with = batch_tile_noises(&check, &layout, &beams, (&sum, &half_b), 8, metric);
    assert_ne!(bits(&without), bits(&with));
}

#[test]
fn a_live_tile_outside_the_layout_reads_as_unmeasured() {
    let (sum, half_b, check) = noisy_tile();
    let layout = batch_layout(&[], (8, 8), 8).expect("no tiles");
    let noises = batch_tile_noises(
        &check,
        &layout,
        &[],
        (&sum, &half_b),
        8,
        (1.0, NoiseMetric::RelativeLstar),
    );
    assert_eq!(noises, vec![f32::INFINITY]);
}

#[test]
fn pixel_slots_of_a_two_tile_request_at_opposite_8k_corners_are_the_tile_pixels() {
    // The engine's scratch is one slot per batch pixel. The bottom-right tile of the 64-pixel grid
    // over 4320 rows is the 64 x 32 one from row 4288.
    let size = (7680, 4320);
    let tiles = [rect(0, 0, 64, 64), rect(7616, 4288, 7680, 4320)];
    let layout = batch_layout(&tiles, size, 64).expect("grid tiles");
    let plan = super::engine::scratch_plan(&layout, size);
    let slots = PixelSlots::new(&tiles).expect("disjoint tiles");
    assert_eq!(slots.len() as u64, plan.compact);
    assert_eq!(slots.slot(0, 0), Some(0));
    assert_eq!(slots.slot(63, 63), Some(64 * 64 - 1));
    assert_eq!(slots.slot(7616, 4288), Some(64 * 64));
    assert_eq!(slots.slot(7679, 4319), Some(64 * 64 + 64 * 32 - 1));
    // The slot of a frame-order index is the same pixel's slot.
    assert_eq!(slots.slot_of_index(7680, 4288 * 7680 + 7616), Some(64 * 64));
    for (x, y) in [
        (64, 0),
        (0, 64),
        (100, 100),
        (7615, 4288),
        (7616, 4287),
        (7680, 4320),
    ] {
        assert_eq!(slots.slot(x, y), None, "pixel ({x}, {y})");
    }
    assert_eq!(slots.slot_of_index(0, 5), None);
    assert!(PixelSlots::new(&[]).expect("no tiles").is_empty());
}

#[test]
fn pixel_slots_are_the_batch_order() {
    let width = 8;
    let image: Vec<Vec3> = (0..48_u8).map(|i| Vec3::splat(f32::from(i))).collect();
    let tiles = [rect(4, 0, 8, 3), rect(0, 3, 4, 6)];
    let ordered = batch_order(&image, width, &tiles).expect("both tiles lie inside the image");
    let slots = PixelSlots::new(&tiles).expect("disjoint tiles");
    assert_eq!(slots.len(), ordered.len());
    let mut found = 0;
    for (index, pixel) in image.iter().enumerate() {
        if let Some(slot) = slots.slot_of_index(width, index) {
            assert_eq!(ordered[slot], *pixel, "image index {index}");
            found += 1;
        }
    }
    assert_eq!(found, ordered.len());
}

#[test]
fn overlapping_tiles_have_no_pixel_slots() {
    assert_eq!(
        PixelSlots::new(&[rect(0, 0, 8, 8), rect(4, 4, 12, 12)]),
        None
    );
    assert!(PixelSlots::new(&[rect(0, 0, 8, 8), rect(8, 0, 16, 8)]).is_some());
}

#[test]
fn compact_noises_equal_the_frame_noises_bit_for_bit() {
    // Two tiles of a 16 x 8 frame with distinct sums per pixel.
    let tiles = [rect(0, 0, 8, 8), rect(8, 0, 16, 8)];
    let layout = batch_layout(&tiles, (16, 8), 8).expect("grid tiles");
    let frame_sum: Vec<Vec3> = (0..128_u8)
        .map(|i| Vec3::splat(f32::from(i).mul_add(0.05, 5.0)))
        .collect();
    let frame_half: Vec<Vec3> = frame_sum.iter().map(|s| *s * 0.45).collect();
    let compact_sum = batch_order(&frame_sum, 16, &tiles).expect("inside");
    let compact_half = batch_order(&frame_half, 16, &tiles).expect("inside");
    let check = PendingCheck {
        live: LiveTiles::all(16, 8, 8),
        samples: 64,
        odd_samples: 32,
    };
    let beams = vec![Vec3::splat(0.02); 128];
    for metric in [NoiseMetric::RelativeLstar, NoiseMetric::AbsoluteDeltaE] {
        let display = (1.0, metric);
        let frame = batch_tile_noises(
            &check,
            &layout,
            &beams,
            (&frame_sum, &frame_half),
            16,
            display,
        );
        let compact = compact_tile_noises(
            &check,
            &layout,
            &beams,
            (&compact_sum, &compact_half),
            display,
        );
        assert_eq!(frame.len(), 2);
        assert_eq!(bits(&compact), bits(&frame), "{metric:?}");
    }
}

// ---- the fixtures ------------------------------------------------------------------------------

/// A residual scene over `planes` and `material`, and the closure's result; the frame is a square
/// of `size` pixels.
fn with_scene<R>(
    planes: &[GpuFacetPlane],
    camera: &Camera,
    material: &GemMaterial,
    (environment, size): (EnvironmentSource<'_>, u32),
    run: impl FnOnce(&ResidualScene<'_>) -> R,
) -> R {
    let soa = build_plane_soa(planes);
    let prepared = PreparedMaterial::new(material);
    let frame = FrameScene {
        camera,
        width: size,
        height: size,
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

/// The adaptive settings of the residual tests: no tile converges before `RESIDUAL_SPP`, which is
/// also the cap, so every pixel gets exactly `RESIDUAL_SPP` samples.
const fn fixed_adaptive() -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise: 1e-4,
        metric: NoiseMetric::RelativeLstar,
        min_spp: RESIDUAL_SPP,
        max_spp: RESIDUAL_SPP,
        pass_spp: RESIDUAL_SPP,
        tile: TILE,
    }
}

/// [`fixed_adaptive`] with one 32-pixel tile, so the 24-pixel fragment frame is one grid tile.
const fn fragment_adaptive() -> AdaptiveSettings {
    AdaptiveSettings {
        tile: 32,
        ..fixed_adaptive()
    }
}

/// An unnormalised mirror-reflected camera field, affine in the image like a reflection leaf's.
struct Field {
    forward: DVec3,
    right: DVec3,
    up: DVec3,
    fov_tan: f64,
    mirror: DVec3,
}

impl Field {
    fn towards(camera: &Camera, target: Vec3) -> Self {
        let forward = camera.forward.as_dvec3();
        Self {
            forward,
            right: camera.right.as_dvec3(),
            up: camera.up.as_dvec3(),
            fov_tan: f64::from(camera.fov_tan),
            mirror: (forward - target.as_dvec3()).normalize(),
        }
    }

    fn direction(&self, p: DVec2) -> DVec3 {
        let size = f64::from(FRAGMENT_FRAME);
        let u = (p.x / size - 0.5) * 2.0 * self.fov_tan;
        let v = (0.5 - p.y / size) * 2.0 * self.fov_tan;
        let d = self.forward + self.right * u + self.up * v;
        d - self.mirror * (2.0 * d.dot(self.mirror))
    }
}

/// The weight of node `node` at `p`: a Fresnel-like level with a node slope and a spatial ramp,
/// rounded to f16 (both sides of the twin see the rounded value).
fn weight(node: u32, p: DVec2) -> f32 {
    let w = 0.0012_f64.mul_add(f64::from(node), 0.17) + 0.01 * (p.x - 12.0) / 8.0;
    f16_bits_to_f32(f32_to_f16_bits(to_f32(w)))
}

/// The CPU leaf of tile 0 and the same leaf in the GPU layout.
fn build_leaf(field: &Field) -> (Leaf, PackedLeaves) {
    let corners = [
        DVec2::new(7.3, 7.9),
        DVec2::new(16.6, 7.2),
        DVec2::new(16.1, 16.4),
        DVec2::new(8.0, 15.8),
    ];
    let image = ConvexPolygon::from_image_points(&corners).expect("a convex leaf");
    let mut dirs = Vec::new();
    let mut weights = Vec::new();
    let mut packed = PackedLeaves::default();
    for v in image.vertices() {
        let p = DVec2::new(v.x, v.y);
        let direction = field.direction(p).as_vec3();
        packed.vertices.push(GpuLeafVertex {
            image: p.as_vec2().to_array(),
            hero_dir: direction.to_array(),
            first_node: u32::try_from(packed.nodes.len()).expect("few nodes"),
        });
        for node in 0..NODES {
            let w = weight(node, p);
            dirs.push(direction);
            weights.push(w);
            packed.nodes.push(GpuLeafNode {
                weight_dx: u32::from(f32_to_f16_bits(w)),
                dy_dz: 0,
            });
        }
    }
    packed.headers.push(GpuLeafHeader {
        sig: [0, 0],
        first_vertex: 0,
        vertex_count: u32::try_from(image.len()).expect("few vertices"),
        first_node: 0,
        node_count: NODES,
        flags: LEAF_FLAG_REFLECTION,
        tau_omega: 1e-4,
    });
    packed.tiles = vec![0];
    packed.tile_offsets = vec![0, 1];
    let leaf = Leaf {
        tile: 0,
        image,
        dirs,
        weights,
        spectrum: SpectralInterval::all(NODES),
        sig: [0, 0],
        prefix: crate::renderer::beam::beam::BranchPrefix::EMPTY,
        glare: false,
        reflection: true,
        tau_omega: 1e-4,
        pre_clip: None,
    };
    (leaf, packed)
}

/// What the fragment tests share: the brilliant under ring lights seen from the twin pose.
struct FragmentRig {
    planes: Vec<GpuFacetPlane>,
    material: GemMaterial,
    camera: Camera,
    environment: EnvironmentSource<'static>,
}

impl FragmentRig {
    const YAW: f32 = 0.4;
    const PITCH: f32 = 0.6;

    fn new() -> Self {
        Self {
            planes: StandardGemCuts::standard_round_brilliant(),
            material: GemMaterial::diamond(),
            // 1.5 degrees over 24 pixels: 0.0011 rad per pixel, the scale tau_L = 1e-3 is designed
            // for (a 40 degree field forced full refinement of every fragment).
            camera: Camera::new(0.6, 0.45, 3.0, 1.5),
            environment: LightingPreset::RingLights.studio(1.0, Self::YAW, Self::PITCH),
        }
    }

    fn leaf(&self) -> (Leaf, PackedLeaves) {
        let target = Vec3::new(0.3, 0.8, 0.2).normalize();
        build_leaf(&Field::towards(&self.camera, target))
    }

    fn with_scene<R>(&self, run: impl FnOnce(&ResidualScene<'_>) -> R) -> R {
        with_scene(
            &self.planes,
            &self.camera,
            &self.material,
            (self.environment, FRAGMENT_FRAME),
            run,
        )
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

/// A one-tile batch of the fragment frame with the given leaves.
fn fragment_batch<'b>(
    tiles: &'b [TileRect],
    leaves: &'b [(u32, Cow<'b, [Leaf]>)],
    packed: &'b PackedLeaves,
    kept: &'b PackedKept,
) -> Batch<'b> {
    Batch {
        index: 0,
        tiles,
        leaves,
        kept: &[],
        packed_leaves: packed,
        packed_kept: kept,
    }
}

// ---- declines and the cancel flag (no adapter needed) ------------------------------------------

#[test]
fn a_set_cancel_flag_cancels_both_stages_before_any_work() {
    let rig = FragmentRig::new();
    let (leaf, packed) = rig.leaf();
    let backend = GpuBackend::disabled();
    let tiles = [rect(0, 0, 24, 24)];
    let leaves = [(0, Cow::Owned(vec![leaf]))];
    let kept = PackedKept::default();
    let batch = fragment_batch(&tiles, &leaves, &packed, &kept);
    let cancelled = AtomicBool::new(true);
    rig.with_scene(|scene| {
        let engine = GpuBeamEngine::new(
            &backend,
            scene,
            &BeamSettings::default(),
            &fragment_adaptive(),
        );
        assert_eq!(
            engine.fragments(&batch, &cancelled),
            EngineOutcome::Cancelled
        );
        let offset = vec![Vec3::ZERO; batch.pixel_count()];
        assert_eq!(
            engine.residual(&batch, &offset, &cancelled),
            EngineOutcome::Cancelled
        );
    });
}

#[test]
fn a_disabled_backend_declines_both_stages_with_a_reason() {
    let rig = FragmentRig::new();
    let (leaf, packed) = rig.leaf();
    let backend = GpuBackend::disabled();
    let tiles = [rect(0, 0, 24, 24)];
    let leaves = [(0, Cow::Owned(vec![leaf]))];
    let kept = PackedKept::default();
    let batch = fragment_batch(&tiles, &leaves, &packed, &kept);
    let never = AtomicBool::new(false);
    rig.with_scene(|scene| {
        let engine = GpuBeamEngine::new(
            &backend,
            scene,
            &BeamSettings::default(),
            &fragment_adaptive(),
        );
        assert_eq!(engine.name(), "gpu");
        match engine.fragments(&batch, &never) {
            EngineOutcome::Declined(reason) => assert_ne!(reason, ""),
            other => panic!("a disabled backend must decline fragments, got {other:?}"),
        }
        let offset = vec![Vec3::ZERO; batch.pixel_count()];
        match engine.residual(&batch, &offset, &never) {
            EngineOutcome::Declined(reason) => assert_ne!(reason, ""),
            other => panic!("a disabled backend must decline the residual, got {other:?}"),
        }
    });
}

#[test]
fn a_retired_engine_declines_every_request_and_never_panics() {
    let rig = FragmentRig::new();
    let (leaf, packed) = rig.leaf();
    let backend = GpuBackend::disabled();
    let tiles = [rect(0, 0, 24, 24)];
    let leaves = [(0, Cow::Owned(vec![leaf]))];
    let kept = PackedKept::default();
    let batch = fragment_batch(&tiles, &leaves, &packed, &kept);
    let never = AtomicBool::new(false);
    rig.with_scene(|scene| {
        let engine = GpuBeamEngine::new(
            &backend,
            scene,
            &BeamSettings::default(),
            &fragment_adaptive(),
        );
        assert!(!engine.is_retired());
        engine.retire();
        assert!(engine.is_retired());
        assert_eq!(
            engine.fragments(&batch, &never),
            EngineOutcome::Declined("GPU device lost")
        );
        let offset = vec![Vec3::ZERO; batch.pixel_count()];
        assert_eq!(
            engine.residual(&batch, &offset, &never),
            EngineOutcome::Declined("GPU device lost")
        );
    });
}

#[test]
fn an_off_grid_batch_or_a_wrong_offset_is_declined_and_an_empty_batch_is_done() {
    let rig = FragmentRig::new();
    let backend = GpuBackend::disabled();
    let packed = PackedLeaves::default();
    let kept = PackedKept::default();
    let never = AtomicBool::new(false);
    rig.with_scene(|scene| {
        let engine = GpuBeamEngine::new(
            &backend,
            scene,
            &BeamSettings::default(),
            &fragment_adaptive(),
        );
        // The frame is 24 pixels: its one grid tile is (0, 0, 24, 24) at tile 32.
        let on_grid = [rect(0, 0, 24, 24)];
        let batch = fragment_batch(&on_grid, &[], &packed, &kept);
        let wrong_offset = vec![Vec3::ZERO; batch.pixel_count() - 1];
        assert!(matches!(
            engine.residual(&batch, &wrong_offset, &never),
            EngineOutcome::Declined(_)
        ));
        let off_grid = [rect(0, 0, 16, 16)];
        let batch = fragment_batch(&off_grid, &[], &packed, &kept);
        let offset = vec![Vec3::ZERO; batch.pixel_count()];
        assert!(matches!(
            engine.residual(&batch, &offset, &never),
            EngineOutcome::Declined(_)
        ));
        // No tiles and no leaves: nothing to do, no device needed.
        let batch = fragment_batch(&[], &[], &packed, &kept);
        assert_eq!(
            engine.fragments(&batch, &never),
            EngineOutcome::Done(BatchFragments { xyz: Vec::new() })
        );
        assert_eq!(
            engine.residual(&batch, &[], &never),
            EngineOutcome::Done(BatchResidual::default())
        );
    });
}

// ---- hardware ----------------------------------------------------------------------------------

/// The GPU engine's fragments of the one-leaf batch against `shade_tile`, the CPU fragments of the
/// same batch: per pixel within `FRAGMENT_TWIN_REL`. Run twice, the second run must equal the
/// first bit for bit (the pooled image buffer is clean again).
#[test]
fn the_gpu_engine_fragments_equal_the_cpu_fragments_of_the_same_batch() {
    let Some(backend) =
        backend_or_skip("the_gpu_engine_fragments_equal_the_cpu_fragments_of_the_same_batch")
    else {
        return;
    };
    let rig = FragmentRig::new();
    let (leaf, packed) = rig.leaf();
    let tile = rect(0, 0, FRAGMENT_FRAME, FRAGMENT_FRAME);
    let tiles = [tile];
    let leaves = [(0, Cow::Owned(vec![leaf.clone()]))];
    let kept = PackedKept::default();
    let batch = fragment_batch(&tiles, &leaves, &packed, &kept);
    let never = AtomicBool::new(false);
    // The same depth cap on both sides, low enough that a steep fixture still terminates.
    let settings = BeamSettings {
        max_fragment_levels: 3,
        ..BeamSettings::default()
    };
    rig.with_scene(|scene| {
        let engine = GpuBeamEngine::new(&backend, scene, &settings, &fragment_adaptive());
        let outcome = engine.fragments(&batch, &never);
        let EngineOutcome::Done(first) = outcome else {
            panic!("the GPU did not shade the batch: {outcome:?}");
        };
        let outcome = engine.fragments(&batch, &never);
        let EngineOutcome::Done(second) = outcome else {
            panic!("the GPU did not shade the batch the second time: {outcome:?}");
        };
        assert_eq!(first, second, "two runs differ (a dirty pooled buffer?)");
        let lighting = LightingScene {
            environment: rig.environment,
            rig: Some(StudioRig::new(FragmentRig::YAW, FragmentRig::PITCH)),
            camera: &rig.camera,
            size: (FRAGMENT_FRAME, FRAGMENT_FRAME),
            tolerance: settings.lighting_tolerance,
            max_levels: settings.max_fragment_levels,
            n_lambda: NODES,
        };
        let cpu = shade_tile(&[leaf], tile, &lighting, &mut BeamStats::default());
        assert_eq!(first.xyz.len(), cpu.xyz.len());
        let mut worst = 0.0_f32;
        let mut total_y = 0.0_f64;
        for (got, expected) in first.xyz.iter().zip(&cpu.xyz) {
            let expected = expected.as_vec3();
            total_y += f64::from(expected.y);
            let rel = (*got - expected).abs().max_element() / expected.max_element().max(1e-6);
            worst = worst.max(rel);
        }
        eprintln!(
            "GPU engine fragments: worst rel {worst:.3e} (FRAGMENT_TWIN_REL {FRAGMENT_TWIN_REL})"
        );
        assert!(total_y > 0.0, "the CPU fragments are empty");
        assert!(worst <= FRAGMENT_TWIN_REL, "worst relative error {worst}");
    });
}

/// The index of pixel `(x, y)` of the residual frame in the batch layout of the four tiles.
const fn batch_index(x: u32, y: u32) -> usize {
    let tile = (y / TILE) * (RESIDUAL_FRAME / TILE) + x / TILE;
    (tile * TILE * TILE + (y % TILE) * TILE + x % TILE) as usize
}

/// The four tiles of the residual frame in row-major order.
fn residual_tiles() -> Vec<TileRect> {
    let mut tiles = Vec::new();
    for y in (0..RESIDUAL_FRAME).step_by(TILE as usize) {
        for x in (0..RESIDUAL_FRAME).step_by(TILE as usize) {
            tiles.push(rect(x, y, x + TILE, y + TILE));
        }
    }
    tiles
}

/// The per-pixel mean of `result` at the compared pixels.
fn pixel_means(result: &BatchResidual) -> Vec<Vec3> {
    PIXELS
        .iter()
        .map(|&(x, y)| result.sum[batch_index(x, y)] / RESIDUAL_SPP_F32)
        .collect()
}

/// The GPU engine's residual against the CPU `run_batch_residual` of the same batch (an empty kept
/// set, a zero offset): per-pixel block means agree within z; the counts are the fixed spp; a
/// second run equals the first bit for bit.
#[test]
fn the_gpu_engine_residual_equals_the_cpu_batch_residual_within_z() {
    let Some(backend) =
        backend_or_skip("the_gpu_engine_residual_equals_the_cpu_batch_residual_within_z")
    else {
        return;
    };
    let rig = BeamRig::cube(glass());
    let tiles = residual_tiles();
    let packed = PackedLeaves::default();
    let kept = PackedKept::default();
    let batch = fragment_batch(&tiles, &[], &packed, &kept);
    let offset = vec![Vec3::ZERO; batch.pixel_count()];
    let adaptive = fixed_adaptive();
    let never = AtomicBool::new(false);
    let mut gpu_blocks: Vec<Vec<Vec3>> = Vec::new();
    let mut cpu_blocks: Vec<Vec<Vec3>> = Vec::new();
    for block in 0..BLOCKS {
        with_scene(
            &rig.planes,
            &rig.camera,
            &rig.material,
            (smooth_environment(), RESIDUAL_FRAME),
            |base| {
                let scene = ResidualScene {
                    sample_offset: block * RESIDUAL_SPP,
                    ..*base
                };
                let engine =
                    GpuBeamEngine::new(&backend, &scene, &BeamSettings::default(), &adaptive);
                let EngineOutcome::Done(gpu) = engine.residual(&batch, &offset, &never) else {
                    panic!("the GPU did not run the residual of block {block}");
                };
                assert_eq!(gpu.counts.len(), batch.pixel_count());
                assert!(gpu.counts.iter().all(|&c| c == RESIDUAL_SPP));
                assert_eq!(
                    gpu.samples_traced,
                    u64::from(RESIDUAL_SPP) * batch.pixel_count() as u64
                );
                if block == 0 {
                    let EngineOutcome::Done(again) = engine.residual(&batch, &offset, &never)
                    else {
                        panic!("the GPU did not run the residual twice");
                    };
                    assert_eq!(gpu, again, "two runs differ (a dirty pooled buffer?)");
                }
                let Some(cpu) = run_batch_residual(&scene, &batch, &offset, &adaptive, &never)
                else {
                    panic!("the CPU batch residual did not run for block {block}");
                };
                assert!(cpu.counts.iter().all(|&c| c == RESIDUAL_SPP));
                gpu_blocks.push(pixel_means(&gpu));
                cpu_blocks.push(pixel_means(&cpu));
            },
        );
    }
    let zs = compare_streams(
        PIXELS.len(),
        (BLOCKS, BLOCKS),
        |k, i| cpu_blocks[i as usize][k],
        |k, i| gpu_blocks[i as usize][k],
    );
    let mean_y = |blocks: &[Vec<Vec3>]| -> f64 {
        blocks.iter().flatten().map(|p| f64::from(p.y)).sum::<f64>()
            / (f64::from(BLOCKS) * PIXEL_COUNT_F64)
    };
    eprintln!(
        "residual Y over {} pixels: GPU {:.5}, CPU {:.5}",
        PIXELS.len(),
        mean_y(&gpu_blocks),
        mean_y(&cpu_blocks)
    );
    assert!(mean_y(&gpu_blocks) > 0.0, "the GPU residual is empty");
    assert_z_stats(
        &zs,
        Z_LIMIT,
        "GPU engine residual against the CPU batch residual",
    );
}

/// Marking the device lost between the stages gives a decline, never a panic, and the engine
/// stays declined afterwards.
///
/// The loss is the backend's real flag, raised by `GpuBackend::mark_lost_for_test`; the engine
/// declines through its check of `GpuBackend::is_lost`.
#[test]
fn a_device_lost_between_fragments_and_residual_declines_without_panicking() {
    let Some(backend) =
        backend_or_skip("a_device_lost_between_fragments_and_residual_declines_without_panicking")
    else {
        return;
    };
    let rig = FragmentRig::new();
    let (leaf, packed) = rig.leaf();
    let tiles = [rect(0, 0, FRAGMENT_FRAME, FRAGMENT_FRAME)];
    let leaves = [(0, Cow::Owned(vec![leaf]))];
    let kept = PackedKept::default();
    let batch = fragment_batch(&tiles, &leaves, &packed, &kept);
    let never = AtomicBool::new(false);
    rig.with_scene(|scene| {
        let engine = GpuBeamEngine::new(
            &backend,
            scene,
            &BeamSettings::default(),
            &fragment_adaptive(),
        );
        let outcome = engine.fragments(&batch, &never);
        let EngineOutcome::Done(fragments) = outcome else {
            panic!("the GPU did not shade the batch: {outcome:?}");
        };
        assert!(!engine.is_retired());
        backend.mark_lost_for_test();
        assert!(backend.is_lost());
        assert!(matches!(
            engine.residual(&batch, &fragments.xyz, &never),
            EngineOutcome::Declined(_)
        ));
        assert!(matches!(
            engine.fragments(&batch, &never),
            EngineOutcome::Declined(_)
        ));
    });
}
