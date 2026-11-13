//! Tests of the leaf buffers, the fragment plan and the reduction.
//!
//! The pure CPU logic (plan, per-pixel CSR, halving rule, sub-batches, the fixed-order CPU sum)
//! and the naga check run everywhere; the hardware tests skip cleanly without an adapter
//! (`PLEOCHROMA_REQUIRE_GPU=1` turns the skip into a failure).

use std::sync::atomic::AtomicBool;

use glam::Vec3;
use naga::{
    front::wgsl,
    valid::{Capabilities, ValidationFlags, Validator},
};

use super::buffers::{
    FragmentPlan, PixelCsr, REDUCE_BEAM_FRAGMENTS_SRC, REDUCE_ENTRY, ReduceScratch, bounding_rect,
    reduce_beam_fragments, reduce_on_cpu, reduction_fits, reduction_max_buffer_bytes,
    split_into_batches, sub_batch, submit_reduction,
};
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset},
    },
    renderer::{
        beam::beam::{GpuLeafHeader, GpuLeafNode, GpuLeafVertex, PackedLeaves},
        gpu::{GpuContext, compute},
        gpu_backend::{BeamFragmentParams, GpuBackend, GpuBeamFragments, GpuSceneRef},
    },
};

/// Environment variable that turns a missing adapter into a failure.
const REQUIRE_GPU_ENV: &str = "PLEOCHROMA_REQUIRE_GPU";

/// The lowest and highest binding the beam units may use (guide §14.7 item 5).
const BEAM_BINDINGS: std::ops::RangeInclusive<u32> = 200..=219;

fn vertex(x: f32, y: f32, first_node: u32) -> GpuLeafVertex {
    GpuLeafVertex {
        image: [x, y],
        hero_dir: [0.0, 0.0, -1.0],
        first_node,
    }
}

/// A leaf with the triangle `points` (one node per vertex), appended to `packed`.
fn push_leaf(packed: &mut PackedLeaves, points: [[f32; 2]; 3]) {
    let first_vertex = u32::try_from(packed.vertices.len()).expect("small test batch");
    let first_node = u32::try_from(packed.nodes.len()).expect("small test batch");
    for ([x, y], node) in points.into_iter().zip(first_node..) {
        packed.vertices.push(vertex(x, y, node));
        packed.nodes.push(GpuLeafNode {
            weight_dx: node,
            dy_dz: 0,
        });
    }
    // `first_node` of a header is the leaf's first spectral node (as `pack_leaves` writes it),
    // not a position in the node array: every test leaf carries node 0.
    packed.headers.push(GpuLeafHeader {
        first_vertex,
        vertex_count: 3,
        first_node: 0,
        node_count: 1,
        ..GpuLeafHeader::default()
    });
}

/// `tile_count` tiles with one leaf each; every leaf covers the 2 x 2 pixels at the origin.
fn four_square_tiles(tile_count: u32) -> PackedLeaves {
    let mut packed = PackedLeaves::default();
    for tile in 0..tile_count {
        push_leaf(&mut packed, [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]);
        packed.tiles.push(10 + tile);
        packed.tile_offsets.push(tile);
    }
    packed.tile_offsets.push(tile_count);
    packed
}

/// The fragment parameters of the entry tests: the defaults of `BeamSettings`.
const PARAMS: BeamFragmentParams = BeamFragmentParams {
    max_levels: 6,
    n_lambda: 48,
    tolerance: 1e-3,
};

#[test]
fn the_plan_has_half_open_clipped_rects_and_an_exclusive_prefix_sum() {
    let mut packed = PackedLeaves::default();
    push_leaf(&mut packed, [[0.2, 0.2], [3.4, 0.2], [3.4, 2.6]]);
    push_leaf(&mut packed, [[8.6, 8.6], [12.0, 9.0], [9.0, 9.4]]);
    push_leaf(&mut packed, [[-5.0, -5.0], [-2.0, -1.0], [-3.0, -4.0]]);
    push_leaf(&mut packed, [[f32::NAN, 1.0], [2.0, 2.0], [3.0, 1.0]]);
    let plan = FragmentPlan::from_leaves(&packed, (10, 10));
    assert_eq!(
        plan.pixel_rects,
        vec![[0, 0, 4, 4], [9, 9, 10, 10], [0, 0, 0, 0], [0, 0, 0, 0]]
    );
    assert_eq!(plan.first_fragment, vec![0, 16, 17, 17, 17]);
    assert_eq!(plan.total, 17);
}

#[test]
fn an_empty_batch_has_an_empty_plan() {
    let plan = FragmentPlan::from_leaves(&PackedLeaves::default(), (10, 10));
    assert_eq!(plan.first_fragment, vec![0]);
    assert_eq!(plan.pixel_rects, Vec::<[u32; 4]>::new());
    assert_eq!(plan.total, 0);
}

#[test]
fn the_pixel_csr_lists_slots_in_leaf_then_row_major_order() {
    let plan = FragmentPlan {
        first_fragment: vec![0, 4, 6],
        pixel_rects: vec![[0, 0, 2, 2], [1, 1, 3, 2]],
        total: 6,
    };
    let csr = PixelCsr::from_plan(&plan);
    assert_eq!(csr.origin, (0, 0));
    assert_eq!(csr.size, (3, 2));
    assert_eq!(csr.offsets, vec![0, 1, 2, 2, 3, 5, 6]);
    assert_eq!(csr.slots, vec![0, 1, 2, 3, 4, 5]);
    // Pixel (1, 1) is covered by leaf 0 (slot 3) and then leaf 1 (slot 4).
    assert_eq!(csr.pixel_slots(4), &[3, 4]);
    assert_eq!(csr.pixel_slots(2), &[] as &[u32]);
    assert_eq!(csr.pixel_slots(99), &[] as &[u32]);
}

#[test]
fn the_csr_of_an_offset_rect_is_relative_to_the_bounding_box() {
    let plan = FragmentPlan {
        first_fragment: vec![0, 2],
        pixel_rects: vec![[5, 7, 7, 8]],
        total: 2,
    };
    let csr = PixelCsr::from_plan(&plan);
    assert_eq!(csr.origin, (5, 7));
    assert_eq!(csr.size, (2, 1));
    assert_eq!(csr.offsets, vec![0, 1, 2]);
    assert_eq!(csr.slots, vec![0, 1]);
}

#[test]
fn an_empty_plan_gives_an_empty_csr() {
    let csr = PixelCsr::from_plan(&FragmentPlan::default());
    assert_eq!(csr.pixel_count(), 0);
    assert_eq!(csr.offsets, vec![0]);
    assert_eq!(csr.slots, Vec::<u32>::new());
}

#[test]
fn the_halving_rule_splits_by_tiles_until_the_buffers_fit() {
    let packed = four_square_tiles(4);
    let plan = FragmentPlan::from_leaves(&packed, (8, 8));
    assert_eq!(plan.total, 16);
    // One tile needs 72 bytes (three 24-byte vertices); four need 288.
    let whole = 0..4;
    assert_eq!(
        split_into_batches(&packed, &plan, 1000),
        Some(std::iter::once(whole).collect::<Vec<_>>())
    );
    assert_eq!(
        split_into_batches(&packed, &plan, 150),
        Some(vec![0..2, 2..4])
    );
    assert_eq!(
        split_into_batches(&packed, &plan, 80),
        Some(vec![0..1, 1..2, 2..3, 3..4])
    );
    // A single tile that does not fit declines.
    assert_eq!(split_into_batches(&packed, &plan, 71), None);
}

#[test]
fn the_halving_rule_counts_the_fragment_slots() {
    let packed = four_square_tiles(2);
    let plan = FragmentPlan::from_leaves(&packed, (8, 8));
    // Four pixels a tile: 64 slot bytes a tile, 128 for both; the vertices need 72 and 144.
    let both = 0..2;
    assert_eq!(
        split_into_batches(&packed, &plan, 144),
        Some(std::iter::once(both).collect::<Vec<_>>())
    );
    assert_eq!(
        split_into_batches(&packed, &plan, 143),
        Some(vec![0..1, 1..2])
    );
}

/// Two small leaves in opposite corners of a 7680 x 4320 frame, as `tiles` tiles (`1` puts both
/// in one tile, `2` gives each its own).
fn corner_leaves(tiles: u32) -> PackedLeaves {
    let mut packed = PackedLeaves::default();
    push_leaf(&mut packed, [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]);
    push_leaf(
        &mut packed,
        [[7678.0, 4318.0], [7679.0, 4318.0], [7679.0, 4319.0]],
    );
    if tiles == 1 {
        packed.tiles.push(10);
        packed.tile_offsets.extend([0, 2]);
    } else {
        packed.tiles.extend([10, 11]);
        packed.tile_offsets.extend([0, 1, 2]);
    }
    packed
}

/// The reduction is sized by the bounding rectangle, so two small leaves in opposite corners are
/// split (or declined) from computed sizes; nothing big is allocated.
#[test]
fn a_sparse_piece_is_split_or_declined_by_the_reduction_bounds() {
    const MIB: u64 = 1 << 20;
    let packed = corner_leaves(2);
    let plan = FragmentPlan::from_leaves(&packed, (7680, 4320));
    assert_eq!(
        plan.pixel_rects,
        vec![[0, 0, 2, 2], [7678, 4318, 7680, 4320]]
    );
    // The leaves' own rectangles are 8 pixels, the bounding rectangle the whole frame.
    assert_eq!(plan.total, 8);
    assert_eq!(bounding_rect(&plan.pixel_rects), Some([0, 0, 7680, 4320]));
    let bytes = reduction_max_buffer_bytes(bounding_rect(&plan.pixel_rects), u64::from(plan.total));
    assert_eq!(bytes, 7680 * 4320 * 16);
    assert_eq!(bytes * 4, 2025 * MIB);
    assert!(!reduction_fits(&plan, 128 * MIB));
    assert!(reduction_fits(&plan, 512 * MIB + 64 * MIB));
    // Two tiles: each fits alone, so the batch splits instead of allocating the gap.
    assert_eq!(
        split_into_batches(&packed, &plan, 128 * MIB),
        Some(vec![0..1, 1..2])
    );
    // A roomy limit keeps one piece: one range over both tiles.
    let both_tiles = 0..2;
    assert_eq!(
        split_into_batches(&packed, &plan, 1024 * MIB),
        Some(vec![both_tiles])
    );
    // One tile holding both leaves cannot be split by tiles: it declines to the CPU.
    let together = corner_leaves(1);
    let plan = FragmentPlan::from_leaves(&together, (7680, 4320));
    assert_eq!(split_into_batches(&together, &plan, 128 * MIB), None);
    // Each half is small once split.
    let half = sub_batch(&packed, 1..2).expect("tile 1 exists");
    let half_plan = FragmentPlan::from_leaves(&half, (7680, 4320));
    assert!(reduction_fits(&half_plan, 1024));
}

#[test]
fn malformed_tile_tables_and_empty_batches_are_handled() {
    let mut packed = four_square_tiles(2);
    let plan = FragmentPlan::from_leaves(&packed, (8, 8));
    packed.tile_offsets.pop();
    assert_eq!(split_into_batches(&packed, &plan, 1000), None);
    assert_eq!(sub_batch(&packed, 0..2), None);
    let empty = PackedLeaves::default();
    let empty_plan = FragmentPlan::from_leaves(&empty, (8, 8));
    assert_eq!(split_into_batches(&empty, &empty_plan, 1000), None);
    let no_tiles = PackedLeaves {
        tile_offsets: vec![0],
        ..PackedLeaves::default()
    };
    assert_eq!(
        split_into_batches(&no_tiles, &empty_plan, 1000),
        Some(vec![])
    );
}

#[test]
fn a_sub_batch_rebases_vertices_nodes_and_the_tile_csr() {
    let packed = four_square_tiles(4);
    let sub = sub_batch(&packed, 1..3).expect("tiles 1..3 exist");
    assert_eq!(sub.tiles, vec![11, 12]);
    assert_eq!(sub.tile_offsets, vec![0, 1, 2]);
    assert_eq!(sub.headers.len(), 2);
    assert_eq!(sub.headers[0].first_vertex, 0);
    assert_eq!(sub.headers[1].first_vertex, 3);
    // The spectral node of a header is copied, never rebased.
    assert_eq!(sub.headers[0].first_node, packed.headers[1].first_node);
    assert_eq!(sub.headers[1].first_node, packed.headers[2].first_node);
    let rebased: Vec<GpuLeafVertex> = packed.vertices[3..9]
        .iter()
        .zip(0_u32..)
        .map(|(v, first_node)| GpuLeafVertex { first_node, ..*v })
        .collect();
    assert_eq!(sub.vertices, rebased);
    assert_eq!(sub.nodes, packed.nodes[3..9].to_vec());
    // The whole range is the batch itself.
    assert_eq!(sub_batch(&packed, 0..4), Some(packed));
}

/// Parses and validates the reduction kernel; returns the bindings its entry point reaches.
fn reduce_bindings() -> Vec<u32> {
    let module = wgsl::parse_str(REDUCE_BEAM_FRAGMENTS_SRC).unwrap_or_else(|e| {
        panic!(
            "reduce_beam_fragments.wgsl does not parse: {}",
            e.emit_to_string(REDUCE_BEAM_FRAGMENTS_SRC)
        )
    });
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .unwrap_or_else(|e| {
            panic!(
                "reduce_beam_fragments.wgsl does not validate: {}",
                e.emit_to_string(REDUCE_BEAM_FRAGMENTS_SRC)
            )
        });
    let index = module
        .entry_points
        .iter()
        .position(|ep| ep.name == REDUCE_ENTRY)
        .expect("the reduction entry point exists");
    let uses = info.get_entry_point(index);
    let mut bindings: Vec<u32> = module
        .global_variables
        .iter()
        .filter(|(handle, _)| !uses[*handle].is_empty())
        .filter_map(|(_, var)| var.binding.as_ref().map(|b| b.binding))
        .collect();
    bindings.sort_unstable();
    bindings
}

#[test]
fn the_reduction_kernel_is_naga_clean_and_binds_only_the_beam_range() {
    let bindings = reduce_bindings();
    assert_eq!(bindings, vec![200, 201, 202, 203, 204]);
    assert!(bindings.iter().all(|b| BEAM_BINDINGS.contains(b)));
}

#[test]
fn the_cpu_reduction_sums_in_the_csr_order() {
    let slots = [
        [1.0e8, 0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0, 0.0],
        [-1.0e8, 0.0, 0.0, 0.0],
    ];
    let csr = PixelCsr {
        origin: (0, 0),
        size: (2, 1),
        offsets: vec![0, 3, 6],
        slots: vec![0, 1, 2, 0, 2, 1],
    };
    let sums = reduce_on_cpu(&slots, &csr);
    // ((0 + 1e8) + 1) - 1e8 loses the 1 in f32; the other order keeps it.
    assert_eq!(sums[0][0].to_bits(), 0.0_f32.to_bits());
    assert_eq!(sums[1][0].to_bits(), 1.0_f32.to_bits());
    assert_eq!(sums[0][3].to_bits(), 0.0_f32.to_bits());
}

fn scene_parts() -> (Camera, Vec<crate::geometry::GpuFacetPlane>, GemMaterial) {
    (
        Camera::new(0.35, 0.28, 5.0, 18.0),
        StandardGemCuts::standard_round_brilliant(),
        GemMaterial::diamond(),
    )
}

#[test]
fn a_disabled_backend_declines_a_batch_and_touches_nothing() {
    let (camera, planes, material) = scene_parts();
    let scene = GpuSceneRef {
        camera: &camera,
        width: 8,
        height: 8,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 4,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
        estimator: None,
    };
    let backend = GpuBackend::disabled();
    let sentinel = Vec3::new(1.5, -2.5, 3.5);
    let mut out = vec![sentinel; 64];
    let never = AtomicBool::new(false);
    let packed = four_square_tiles(2);
    let outcome = backend.try_beam_fragments(&scene, &packed, PARAMS, &mut out, &never);
    assert!(matches!(outcome, GpuBeamFragments::Declined(_)));
    assert!(out.iter().all(|p| *p == sentinel));
    // An empty batch is trivially done; a set cancel flag wins.
    let empty = PackedLeaves::default();
    assert_eq!(
        backend.try_beam_fragments(&scene, &empty, PARAMS, &mut out, &never),
        GpuBeamFragments::Done
    );
    let cancelled = AtomicBool::new(true);
    assert_eq!(
        backend.try_beam_fragments(&scene, &packed, PARAMS, &mut out, &cancelled),
        GpuBeamFragments::Cancelled
    );
}

/// A deterministic spread of f32 values with mixed magnitudes and signs (no denormals).
fn pseudo_slots(count: usize) -> Vec<[f32; 4]> {
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let unit = f32::from(u16::try_from(state >> 48).expect("16 bits")) / 65536.0;
        let exponent = i32::try_from((state >> 8) % 7).expect("below 7") - 3;
        (unit - 0.5) * 10.0_f32.powi(exponent)
    };
    (0..count).map(|_| [next(), next(), next(), 0.0]).collect()
}

#[test]
fn the_gpu_reduction_equals_the_cpu_sum_bit_for_bit() {
    let ctx = match GpuContext::acquire() {
        Ok(ctx) => ctx,
        Err(error) => {
            assert!(
                std::env::var(REQUIRE_GPU_ENV).is_err(),
                "{REQUIRE_GPU_ENV} is set but no adapter is available: {error:?}"
            );
            eprintln!("skipping the GPU reduction test: no adapter ({error:?})");
            return;
        }
    };
    let plan = FragmentPlan {
        first_fragment: vec![0, 20, 32, 37],
        pixel_rects: vec![[0, 0, 5, 4], [2, 1, 8, 3], [3, 0, 4, 5]],
        total: 37,
    };
    let csr = PixelCsr::from_plan(&plan);
    let slots = pseudo_slots(plan.total as usize);
    let buffer = compute::upload(
        &ctx.device,
        "test fragment slots",
        &slots,
        wgpu::BufferUsages::STORAGE,
    );
    let gpu = reduce_beam_fragments(&ctx, &buffer, &csr).expect("the reduction runs");
    let cpu = reduce_on_cpu(&slots, &csr);
    assert_eq!(gpu.len(), cpu.len());
    for (pixel, (g, c)) in gpu.iter().zip(&cpu).enumerate() {
        let gbits = g.map(f32::to_bits);
        let cbits = c.map(f32::to_bits);
        assert_eq!(gbits, cbits, "pixel {pixel}: gpu {g:?} cpu {c:?}");
    }
}

/// The per-device caches die with the context: after a reduction, dropping the last clone of the
/// context drops its pipelines and with them the device.
#[test]
fn the_beam_caches_die_with_the_last_context_clone() {
    let ctx = match GpuContext::acquire() {
        Ok(ctx) => ctx,
        Err(error) => {
            assert!(
                std::env::var(REQUIRE_GPU_ENV).is_err(),
                "{REQUIRE_GPU_ENV} is set but no adapter is available: {error:?}"
            );
            eprintln!("skipping the beam caches lifetime test: no adapter ({error:?})");
            return;
        }
    };
    // A reduction compiles and caches the reduce pipeline.
    let plan = FragmentPlan {
        first_fragment: vec![0, 3],
        pixel_rects: vec![[4, 4, 7, 5]],
        total: 3,
    };
    let csr = PixelCsr::from_plan(&plan);
    let slots = pseudo_slots(3);
    let buffer = compute::upload(
        &ctx.device,
        "test fragment slots",
        &slots,
        wgpu::BufferUsages::STORAGE,
    );
    let sums = reduce_beam_fragments(&ctx, &buffer, &csr).expect("the reduction runs");
    assert_eq!(sums.len(), csr.pixel_count());
    drop(buffer);
    let caches = std::sync::Arc::downgrade(&ctx.beam_caches);
    assert!(caches.upgrade().is_some());
    drop(ctx);
    assert!(caches.upgrade().is_none(), "the caches outlive the context");
}

/// Pieces of different sizes share one grow-only buffer set and give the very bits the per-piece
/// sets give; the set allocates only when a piece outgrows it.
#[test]
fn pieces_of_different_sizes_reuse_one_reduction_buffer_set() {
    let ctx = match GpuContext::acquire() {
        Ok(ctx) => ctx,
        Err(error) => {
            assert!(
                std::env::var(REQUIRE_GPU_ENV).is_err(),
                "{REQUIRE_GPU_ENV} is set but no adapter is available: {error:?}"
            );
            eprintln!("skipping the reduction reuse test: no adapter ({error:?})");
            return;
        }
    };
    let big = FragmentPlan {
        first_fragment: vec![0, 20, 32, 37],
        pixel_rects: vec![[0, 0, 5, 4], [2, 1, 8, 3], [3, 0, 4, 5]],
        total: 37,
    };
    let small = FragmentPlan {
        first_fragment: vec![0, 3],
        pixel_rects: vec![[4, 4, 7, 5]],
        total: 3,
    };
    let slots = pseudo_slots(37);
    let buffer = compute::upload(
        &ctx.device,
        "test fragment slots",
        &slots,
        wgpu::BufferUsages::STORAGE,
    );
    let mut shared = ReduceScratch::default();
    let mut allocations = Vec::new();
    for plan in [&big, &small, &big, &small] {
        let csr = PixelCsr::from_plan(plan);
        let reused = submit_reduction(&ctx, &mut shared, &buffer, &csr)
            .and_then(|pending| pending.finish(&ctx.device))
            .expect("the shared-set reduction runs");
        let separate = reduce_beam_fragments(&ctx, &buffer, &csr).expect("the reduction runs");
        let cpu = reduce_on_cpu(&slots, &csr);
        let bits = |sums: &[[f32; 4]]| -> Vec<[u32; 4]> {
            sums.iter().map(|sum| sum.map(f32::to_bits)).collect()
        };
        assert_eq!(bits(&reused), bits(&separate));
        assert_eq!(bits(&reused), bits(&cpu));
        allocations.push(shared.allocations());
    }
    // The big piece allocates the four buffers once; nothing later outgrows them.
    assert_eq!(allocations, vec![4, 4, 4, 4]);
}

/// The renderer lock is free while the reduction readback is awaited.
#[test]
fn the_renderer_lock_is_free_during_the_reduction_readback() {
    let backend = GpuBackend::acquire();
    if backend.adapter_label().is_none() {
        assert!(
            std::env::var(REQUIRE_GPU_ENV).is_err(),
            "{REQUIRE_GPU_ENV} is set but no adapter is available"
        );
        eprintln!("skipping the lock test: no adapter");
        return;
    }
    let (camera, planes, material) = scene_parts();
    let scene = GpuSceneRef {
        camera: &camera,
        width: 8,
        height: 8,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 4,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
        estimator: None,
    };
    let packed = four_square_tiles(2);
    let mut out = vec![Vec3::ZERO; 64];
    let never = AtomicBool::new(false);
    let mut waits = Vec::new();
    let outcome = backend.try_beam_fragments_observed(
        &scene,
        &packed,
        PARAMS,
        &mut out,
        &never,
        &mut |free| waits.push(free),
    );
    assert_eq!(outcome, GpuBeamFragments::Done);
    assert_eq!(waits, vec![true]);
}

/// The compact entry adds the same sums, bit for bit, at the slots of the batch's tiles (here two
/// column halves in reverse order, covering the whole frame).
#[test]
fn the_compact_fragment_entry_equals_the_frame_entry_in_batch_order() {
    use crate::renderer::{
        beam::primary::TileRect,
        gpu::beam::engine::{PixelSlots, batch_order},
    };
    let backend = GpuBackend::acquire();
    if backend.adapter_label().is_none() {
        assert!(
            std::env::var(REQUIRE_GPU_ENV).is_err(),
            "{REQUIRE_GPU_ENV} is set but no adapter is available"
        );
        eprintln!("skipping the compact fragment test: no adapter");
        return;
    }
    let (camera, planes, material) = scene_parts();
    let scene = GpuSceneRef {
        camera: &camera,
        width: 8,
        height: 8,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 4,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
        estimator: None,
    };
    let packed = four_square_tiles(2);
    let never = AtomicBool::new(false);
    let mut frame = vec![Vec3::ZERO; 64];
    let outcome = backend.try_beam_fragments(&scene, &packed, PARAMS, &mut frame, &never);
    assert_eq!(outcome, GpuBeamFragments::Done);
    let tiles = [
        TileRect {
            x0: 4,
            y0: 0,
            x1: 8,
            y1: 8,
        },
        TileRect {
            x0: 0,
            y0: 0,
            x1: 4,
            y1: 8,
        },
    ];
    let slots = PixelSlots::new(&tiles).expect("disjoint tiles");
    let mut compact = vec![Vec3::ZERO; slots.len()];
    let outcome =
        backend.try_beam_fragments_compact(&scene, &packed, PARAMS, (&slots, &mut compact), &never);
    assert_eq!(outcome, GpuBeamFragments::Done);
    let expected = batch_order(&frame, 8, &tiles).expect("inside the frame");
    let bits = |pixels: &[Vec3]| -> Vec<[u32; 3]> {
        pixels
            .iter()
            .map(|p| p.to_array().map(f32::to_bits))
            .collect()
    };
    assert_eq!(bits(&compact), bits(&expected));
    assert!(compact.iter().any(|p| *p != Vec3::ZERO));
    // A slice of another length declines and touches nothing.
    let mut short = vec![Vec3::ZERO; slots.len() - 1];
    let outcome =
        backend.try_beam_fragments_compact(&scene, &packed, PARAMS, (&slots, &mut short), &never);
    assert!(matches!(outcome, GpuBeamFragments::Declined(_)));
    assert!(short.iter().all(|p| *p == Vec3::ZERO));
}
