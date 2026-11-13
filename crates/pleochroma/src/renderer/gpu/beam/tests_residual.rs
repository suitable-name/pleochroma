//! Tests of the GPU reference residual.
//!
//! The kept reference megakernel text, the storage-buffer arithmetic of the decline, and (with an
//! adapter; they skip cleanly without one, `PLEOCHROMA_REQUIRE_GPU=1` turns the skip into a
//! failure) the residual pass through `GpuBackend::try_accumulate_reference_live_cancellable`.

use std::sync::atomic::AtomicBool;

use glam::Vec3;
use naga::{
    front::wgsl,
    valid::{Capabilities, ValidationFlags, Validator},
};

use super::{
    kept::{
        KEPT_BINDING_INDICES, KEPT_BINDING_PARAMS, KEPT_BINDING_PIXEL_OFFSETS,
        KEPT_BINDING_REGIONS, KEPT_STORAGE_BUFFERS, kept_wgsl,
    },
    reference_source::{
        RAY_SIG_BINDING, REFERENCE_LIVE_STORAGE_BUFFERS, ReferenceUnit,
        reference_kept_megakernel_for_live, reference_kept_variant, reference_live_fits,
        reference_variant,
    },
};
use crate::{
    geometry::tool::StoneGeometry,
    optics::{
        fluorescence::Fluorescence,
        raytracer::{
            Camera, cranley_patterson_rotate, hash_u32, radical_inverse_base,
            surface::SurfaceScene,
            transport::{
                PreparedMaterial,
                reference::{KeptFn, trace_spectral_ray_reference_prepared},
            },
        },
    },
    renderer::{
        adaptive::LiveTiles,
        beam::{
            beam::{GpuKeptRegion, KEPT_MAX_VERTICES, PackedKept},
            tests_support::{BeamRig, MAX_BOUNCES, cube_planes, d1_reflection_signatures, glass},
        },
        gpu::{MEGAKERNEL_STORAGE_BUFFERS, frame::SHADER_SRC},
        gpu_backend::{GpuAccumulate, GpuBackend, GpuSceneRef},
        path_cache::signature::{BRANCH_SIG_SEED, branch_step},
        test_stats::{assert_z_stats, compare_streams, draw},
    },
};

/// Environment variable that turns a missing adapter into a failure.
const REQUIRE_GPU_ENV: &str = "PLEOCHROMA_REQUIRE_GPU";

/// The side of the square test frame in pixels.
const FRAME: u32 = 32;

/// The tile side of the live tiles (the whole frame is four tiles).
const TILE: u32 = 16;

/// Spectral nodes of the kept test.
const N_LAMBDA: u32 = 24;

/// The `@binding` of the live pixel list.
const LIVE_LIST_BINDING: u32 = 140;

/// Samples per pixel of one pass (one block of the z comparison).
const BLOCK_SPP: u32 = 256;

/// Blocks (independent passes) per side of the z comparison.
const BLOCKS: u32 = 8;

/// The largest |z| allowed (`assert_z_stats` also bounds the fraction above 3).
const Z_LIMIT: f64 = 5.0;

/// The pixels compared: a 3 x 3 grid well inside the top face of the test cube.
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

/// The pixels compared under `off_pole_cube_rig`: a 3 x 3 grid inside the top face as the 60
/// degree camera sees it (the face's image sits above the frame centre; row 20 of `PIXELS` would
/// enter the `+Z` face).
const OFF_POLE_PIXELS: [(u32, u32); 9] = [
    (12, 8),
    (16, 8),
    (20, 8),
    (12, 12),
    (16, 12),
    (20, 12),
    (12, 16),
    (16, 16),
    (20, 16),
];

/// The facet index of the cube's top face (the `+Y` plane, third of `cube_planes`).
const TOP_FACET: u32 = 2;

fn validate(name: &str, source: &str) -> naga::Module {
    let module = wgsl::parse_str(source)
        .unwrap_or_else(|e| panic!("{name} does not parse: {}", e.emit_to_string(source)));
    Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name} does not validate: {}", e.emit_to_string(source)));
    module
}

/// The `@binding` numbers of the globals `entry` reaches in `source`, sorted.
fn reached_bindings(source: &str, entry: &str) -> Vec<u32> {
    let module = validate(entry, source);
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .expect("validated above");
    let index = module
        .entry_points
        .iter()
        .position(|ep| ep.name == entry)
        .expect("entry point exists");
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

const KEPT_BINDINGS: [u32; 4] = [
    KEPT_BINDING_REGIONS,
    KEPT_BINDING_PIXEL_OFFSETS,
    KEPT_BINDING_INDICES,
    KEPT_BINDING_PARAMS,
];

fn kept_megakernel() -> String {
    reference_kept_variant(ReferenceUnit::Megakernel, SHADER_SRC)
        .expect("the megakernel has a kept reference variant")
}

/// [`kept_megakernel`] as the live variant makes it: the two pixel lines through the list, the
/// generator's own lookup swapped, the list declared (what `live_source::live_variant` does,
/// repeated here because that module is private).
fn live_kept_megakernel() -> String {
    let text =
        reference_kept_megakernel_for_live().expect("the kept megakernel has a live-ready variant");
    let lookup = "let pixel = idx / camera.num_samples + params.pixel_offset;";
    assert_eq!(text.matches(lookup).count(), 1);
    let mut live = text.replacen(
        lookup,
        "let pixel = live_pixels[idx / camera.num_samples + params.pixel_offset];",
        1,
    );
    live.push_str("\n@group(0) @binding(140) var<storage, read> live_pixels: array<u32>;\n");
    live
}

#[test]
fn the_kept_variant_swaps_the_stub_and_appends_the_kept_unit() {
    let text = kept_megakernel();
    let plain = reference_variant(ReferenceUnit::Megakernel, SHADER_SRC).expect("variant");
    assert!(plain.contains("-> bool { return false; }"));
    assert!(!text.contains(
        "fn beam_kept(pixel: u32, image_point: vec2<f32>, sig: vec2<u32>, lambda: f32) -> bool { return false; }"
    ));
    assert_eq!(text.matches("fn beam_kept(").count(), 1);
    assert_eq!(text.matches("fn beam_kept_test(").count(), 1);
    assert_eq!(text.matches(kept_wgsl()).count(), 1);
    // The shipped text and the plain reference variant hold none of it.
    assert!(!SHADER_SRC.contains("beam_kept_test"));
    assert!(!plain.contains("beam_kept_test"));
}

#[test]
fn a_kept_variant_of_a_drifted_source_declines() {
    assert!(reference_kept_variant(ReferenceUnit::Megakernel, "fn nothing() {}").is_none());
}

#[test]
fn the_kept_megakernel_validates_and_adds_only_the_kept_bindings() {
    let plain = reached_bindings(SHADER_SRC, "transport_main");
    let kept = reached_bindings(&kept_megakernel(), "transport_main");
    let mut expected = plain;
    expected.extend(KEPT_BINDINGS);
    expected.sort_unstable();
    assert_eq!(kept, expected);
    assert!(!kept.contains(&RAY_SIG_BINDING));
}

#[test]
fn the_live_kept_megakernel_validates_and_adds_the_list_and_the_kept_bindings() {
    let text = live_kept_megakernel();
    // Both reference pixel lines read the real pixel through the list.
    assert_eq!(
        text.matches("let pixel = live_pixels[params.pixel_offset + idx / camera.num_samples];")
            .count(),
        2
    );
    assert!(!text.contains("let pixel = params.pixel_offset + idx / camera.num_samples;"));
    let plain = reached_bindings(SHADER_SRC, "transport_main");
    let live = reached_bindings(&text, "transport_main");
    let mut expected = plain;
    expected.push(LIVE_LIST_BINDING);
    expected.extend(KEPT_BINDINGS);
    expected.sort_unstable();
    assert_eq!(live, expected);
}

#[test]
fn every_new_binding_of_the_kept_megakernel_is_in_the_beam_range() {
    let plain = reached_bindings(SHADER_SRC, "transport_main");
    for binding in reached_bindings(&live_kept_megakernel(), "transport_main") {
        if !plain.contains(&binding) && binding != LIVE_LIST_BINDING {
            assert!(
                (200..=219).contains(&binding),
                "binding {binding} is outside 200-219"
            );
        }
    }
}

#[test]
fn the_storage_buffer_arithmetic_counts_the_list_and_the_kept_buffers() {
    assert_eq!(KEPT_STORAGE_BUFFERS, 3);
    assert_eq!(
        REFERENCE_LIVE_STORAGE_BUFFERS,
        MEGAKERNEL_STORAGE_BUFFERS + 1 + KEPT_STORAGE_BUFFERS
    );
    assert!(reference_live_fits(REFERENCE_LIVE_STORAGE_BUFFERS));
    assert!(reference_live_fits(REFERENCE_LIVE_STORAGE_BUFFERS + 1));
    assert!(reference_live_fits(u32::MAX));
}

#[test]
fn an_adapter_with_too_few_storage_buffers_is_declined() {
    for available in 0..REFERENCE_LIVE_STORAGE_BUFFERS {
        assert!(
            !reference_live_fits(available),
            "{available} storage buffers cannot hold the reference pass"
        );
    }
    // The baseline WebGPU limit of eight is far below the megakernel's own eleven.
    assert!(!reference_live_fits(8));
}

// ---- hardware tests ----------------------------------------------------------------------------

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

/// A rectangle region over the whole frame, kept for `sig` at every node.
fn frame_region(sig: [u32; 2]) -> GpuKeptRegion {
    let mut polygon = [[0.0_f32; 2]; KEPT_MAX_VERTICES];
    let side = FRAME as f32 + 1.0;
    polygon[0] = [-1.0, -1.0];
    polygon[1] = [side, -1.0];
    polygon[2] = [side, side];
    polygon[3] = [-1.0, side];
    GpuKeptRegion {
        polygon,
        sig,
        len: 4,
        first_node: 0,
        last_node: N_LAMBDA - 1,
        flags: 0,
        pad: [0; 2],
    }
}

/// The kept set of one region over the whole frame: every pixel lists region 0.
fn frame_kept(sig: [u32; 2]) -> PackedKept {
    let pixels = (FRAME * FRAME) as usize;
    PackedKept {
        regions: vec![frame_region(sig)],
        origin: (0, 0),
        size: (FRAME, FRAME),
        pixel_offsets: (0..=u32::try_from(pixels).expect("small")).collect(),
        indices: vec![0; pixels],
        span_count: 0,
    }
}

/// The kept set of one region per signature over the whole frame: every pixel lists them all.
fn frame_kept_all(sigs: &[[u32; 2]]) -> PackedKept {
    let pixels = (FRAME * FRAME) as usize;
    let regions = u32::try_from(sigs.len()).expect("few regions");
    PackedKept {
        regions: sigs.iter().map(|&sig| frame_region(sig)).collect(),
        origin: (0, 0),
        size: (FRAME, FRAME),
        pixel_offsets: (0..=u32::try_from(pixels).expect("small"))
            .map(|pixel| pixel * regions)
            .collect(),
        indices: (0..pixels).flat_map(|_| 0..regions).collect(),
        span_count: 0,
    }
}

/// The signature of the depth-1 reflection off the cube's top face.
fn top_reflection() -> [u32; 2] {
    branch_step(
        BRANCH_SIG_SEED,
        TOP_FACET,
        crate::renderer::path_cache::bounce::EventKind::Reflect,
        false,
    )
}

/// One residual pass over the whole frame; `(outcome, accum, half_b)` from zeroed buffers.
fn residual_pass(
    backend: &GpuBackend,
    rig: &BeamRig,
    kept: &PackedKept,
    (sample_offset, spp): (u32, u32),
) -> (GpuAccumulate, Vec<Vec3>, Vec<Vec3>) {
    let frame_scene = rig.scene(FRAME, MAX_BOUNCES);
    let scene = GpuSceneRef {
        camera: frame_scene.camera,
        width: FRAME,
        height: FRAME,
        planes: frame_scene.planes,
        facet_finishes: &[],
        material: frame_scene.material,
        max_bounces: frame_scene.max_bounces,
        environment: frame_scene.environment,
        estimator: None,
    };
    let live = LiveTiles::all(FRAME, FRAME, TILE);
    let len = (FRAME * FRAME) as usize;
    let mut accum = vec![Vec3::ZERO; len];
    let mut half_b = vec![Vec3::ZERO; len];
    let never_cancel = AtomicBool::new(false);
    let outcome = backend.try_accumulate_reference_live_cancellable(
        &scene,
        &[],
        &SurfaceScene::IDEAL,
        sample_offset,
        spp,
        &live,
        kept,
        N_LAMBDA,
        &mut accum,
        &mut half_b,
        &never_cancel,
    );
    (outcome, accum, half_b)
}

fn bits(pixels: &[Vec3]) -> Vec<[u32; 3]> {
    pixels
        .iter()
        .map(|p| p.to_array().map(f32::to_bits))
        .collect()
}

fn total_y(pixels: &[Vec3]) -> f64 {
    pixels.iter().map(|p| f64::from(p.y)).sum()
}

/// With nothing kept the pass is the plain reference pass, and a kept set that can match nothing
/// (a signature no path carries) changes not one bit of it.
#[test]
fn an_empty_kept_set_equals_a_kept_set_that_matches_nothing_bit_for_bit() {
    let Some(backend) = backend_or_skip("an_empty_kept_set_equals_a_kept_set_that_matches_nothing")
    else {
        return;
    };
    let rig = BeamRig::cube(glass());
    let pass = (0, 16);
    let (outcome, empty, empty_half) = residual_pass(&backend, &rig, &PackedKept::default(), pass);
    assert_eq!(outcome, GpuAccumulate::Done);
    assert!(
        total_y(&empty) > 0.0,
        "the reference pass produced no light"
    );
    let (outcome, inert, inert_half) = residual_pass(
        &backend,
        &rig,
        &frame_kept([0xDEAD_BEEF, 0x0BAD_F00D]),
        pass,
    );
    assert_eq!(outcome, GpuAccumulate::Done);
    assert_eq!(bits(&empty), bits(&inert));
    assert_eq!(bits(&empty_half), bits(&inert_half));
    // Deterministic: the same pass twice gives the same bits.
    let (_, again, _) = residual_pass(&backend, &rig, &PackedKept::default(), pass);
    assert_eq!(bits(&empty), bits(&again));
}

/// Keeping the depth-1 reflection of the top face removes light, and only light, at the pixels
/// the top face covers.
#[test]
fn keeping_the_top_reflection_removes_light() {
    let Some(backend) = backend_or_skip("keeping_the_top_reflection_removes_light") else {
        return;
    };
    let rig = off_pole_cube_rig();
    for &pixel in &OFF_POLE_PIXELS {
        assert_eq!(
            first_entered_facet(&rig, pixel),
            Some(TOP_FACET as usize),
            "pixel {pixel:?} does not hit the top face first: fixture error"
        );
    }
    let pass = (0, BLOCK_SPP);
    let (_, whole, _) = residual_pass(&backend, &rig, &PackedKept::default(), pass);
    let (outcome, residual, _) = residual_pass(&backend, &rig, &frame_kept(top_reflection()), pass);
    assert_eq!(outcome, GpuAccumulate::Done);
    let at = |image: &[Vec3]| -> f64 {
        OFF_POLE_PIXELS
            .iter()
            .map(|&(x, y)| f64::from(image[(y * FRAME + x) as usize].y))
            .sum()
    };
    eprintln!(
        "top face pixels: whole Y {:.4}, residual Y {:.4}",
        at(&whole),
        at(&residual)
    );
    // The values that decide a failure: whether any first-surface reflection signature of the six
    // cube faces matches on the GPU, and whether the CPU reference removes light at all.
    let first_surface: Vec<[u32; 2]> = (0..6)
        .map(|facet| {
            branch_step(
                BRANCH_SIG_SEED,
                facet,
                crate::renderer::path_cache::bounce::EventKind::Reflect,
                false,
            )
        })
        .collect();
    let (_, all_faces, _) = residual_pass(&backend, &rig, &frame_kept_all(&first_surface), pass);
    let prepared = PreparedMaterial::new(&rig.material);
    let cpu_y = |keep: bool| -> f64 {
        OFF_POLE_PIXELS
            .iter()
            .map(|&pixel| {
                f64::from(cpu_block_keeping(&rig, &prepared, pixel, 0, keep).y)
                    * f64::from(BLOCK_SPP)
            })
            .sum()
    };
    let (cpu_whole, cpu_residual) = (cpu_y(false), cpu_y(true));
    eprintln!(
        "GPU with the reflections of all six faces kept: Y {:.4}; CPU block 0 (its own sample \
         stream, not the GPU's samples): nothing kept Y {cpu_whole:.4}, top reflection kept Y \
         {cpu_residual:.4}",
        at(&all_faces),
    );
    // Precondition: the kept branch carries light on the CPU, else the fixture is dark (a camera
    // inside the head shadow, say) and the GPU assertions below prove nothing.
    assert!(
        cpu_whole > cpu_residual,
        "fixture error: the CPU reference loses no light by keeping the top reflection"
    );
    for &(x, y) in &OFF_POLE_PIXELS {
        let index = (y * FRAME + x) as usize;
        assert!(
            residual[index].y <= whole[index].y,
            "pixel ({x}, {y}) gained light by keeping a reflection"
        );
    }
    assert!(
        at(&residual) < at(&whole),
        "the kept reflection was not removed"
    );
    // The GPU drop is at least half of the CPU's relative drop (the sample streams differ, so
    // not equal): a branch that matched on one side only would fail this.
    let gpu_drop = 1.0 - at(&residual) / at(&whole);
    let cpu_drop = 1.0 - cpu_residual / cpu_whole;
    assert!(
        gpu_drop > 0.5 * cpu_drop,
        "GPU relative drop {gpu_drop:.4} is far below the CPU's {cpu_drop:.4}"
    );
    assert!(at(&residual) >= 0.0);
}

/// The unit cube seen from 60 degrees elevation, so the depth-1 reflection of the top face leaves
/// the eye's head-shadow cone and carries light.
fn off_pole_cube_rig() -> BeamRig {
    BeamRig::new(cube_planes(), glass(), Camera::new(0.5, 1.05, 8.0, 24.0))
}

/// The facet a pixel's primary ray enters the stone through (largest entry distance over the
/// planes it faces), `None` when it misses; a pure CPU check.
fn first_entered_facet(rig: &BeamRig, pixel: (u32, u32)) -> Option<usize> {
    let ray = rig.pixel_ray(FRAME, pixel);
    let (mut enter, mut leave) = (f32::NEG_INFINITY, f32::INFINITY);
    let mut facet = None;
    for (index, plane) in rig.planes.iter().enumerate() {
        let normal = Vec3::from(plane.normal);
        let denominator = normal.dot(ray.dir);
        let distance = -(normal.dot(ray.origin) + plane.d);
        if denominator.abs() < 1e-9 {
            if distance > 0.0 {
                return None;
            }
            continue;
        }
        let t = distance / denominator;
        if denominator < 0.0 {
            if t > enter {
                enter = t;
                facet = Some(index);
            }
        } else {
            leave = leave.min(t);
        }
    }
    if enter < leave { facet } else { None }
}

/// The CPU reference tracer's mean over `BLOCK_SPP` samples of pixel `pixel`, block `block`, with
/// the top reflection kept.
fn cpu_block(
    rig: &BeamRig,
    prepared: &PreparedMaterial<'_>,
    pixel: (u32, u32),
    block: u32,
) -> Vec3 {
    cpu_block_keeping(rig, prepared, pixel, block, true)
}

/// [`cpu_block`] with the top reflection kept (`keep`) or nothing kept.
fn cpu_block_keeping(
    rig: &BeamRig,
    prepared: &PreparedMaterial<'_>,
    pixel: (u32, u32),
    block: u32,
    keep: bool,
) -> Vec3 {
    let environment = rig.scene(FRAME, MAX_BOUNCES).environment;
    let top = crate::renderer::path_cache::signature::pack_sig(top_reflection());
    let kept: &KeptFn<'_> = &move |sig: u64, _lambda: f32| sig == top;
    let salt = pixel.1 * FRAME + pixel.0;
    let unit = |h: u32| hash_u32(h) as f32 / 4_294_967_295.0;
    let mut sum = Vec3::ZERO;
    for s in 0..BLOCK_SPP {
        let i = block * BLOCK_SPP + s;
        let (seed, hero_rand) = draw(i, salt);
        let jitter_x =
            cranley_patterson_rotate(radical_inverse_base(i, 2), unit(salt ^ 0x1357_9BDF)) - 0.5;
        let jitter_y =
            cranley_patterson_rotate(radical_inverse_base(i, 3), unit(salt ^ 0x2468_ACE0)) - 0.5;
        let ray = rig.camera.generate_ray(
            pixel.0 as f32,
            pixel.1 as f32,
            FRAME as f32,
            FRAME as f32,
            jitter_x,
            jitter_y,
        );
        sum += trace_spectral_ray_reference_prepared(
            ray,
            StoneGeometry::planes_only(&rig.planes),
            &rig.soa,
            &[],
            prepared,
            Fluorescence::none(),
            MAX_BOUNCES,
            environment,
            seed,
            hero_rand,
            None,
            SurfaceScene::IDEAL,
            keep.then_some(kept),
        )
        .xyz;
    }
    sum / BLOCK_SPP as f32
}

/// The GPU residual against the CPU reference tracer with the same kept set (the top-face
/// reflection kept at every pixel and node): the per-pixel block means agree within z.
#[test]
fn the_gpu_residual_matches_the_cpu_reference_with_the_same_kept_set() {
    let Some(backend) =
        backend_or_skip("the_gpu_residual_matches_the_cpu_reference_with_the_same_kept_set")
    else {
        return;
    };
    let rig = BeamRig::cube(glass());
    let prepared = PreparedMaterial::new(&rig.material);
    let kept = frame_kept(top_reflection());
    let mut gpu_blocks: Vec<Vec<Vec3>> = Vec::new();
    for block in 0..BLOCKS {
        let (outcome, accum, _) =
            residual_pass(&backend, &rig, &kept, (block * BLOCK_SPP, BLOCK_SPP));
        assert_eq!(outcome, GpuAccumulate::Done);
        gpu_blocks.push(
            PIXELS
                .iter()
                .map(|&(x, y)| accum[(y * FRAME + x) as usize] / BLOCK_SPP as f32)
                .collect(),
        );
    }
    let cpu_blocks: Vec<Vec<Vec3>> = (0..BLOCKS)
        .map(|block| {
            PIXELS
                .iter()
                .map(|&pixel| cpu_block(&rig, &prepared, pixel, block))
                .collect()
        })
        .collect();
    let zs = compare_streams(
        PIXELS.len(),
        (BLOCKS, BLOCKS),
        |k, i| cpu_blocks[i as usize][k],
        |k, i| gpu_blocks[i as usize][k],
    );
    let gpu_mean: f64 = gpu_blocks
        .iter()
        .flatten()
        .map(|p| f64::from(p.y))
        .sum::<f64>()
        / f64::from(BLOCKS);
    let cpu_mean: f64 = cpu_blocks
        .iter()
        .flatten()
        .map(|p| f64::from(p.y))
        .sum::<f64>()
        / f64::from(BLOCKS);
    eprintln!(
        "residual Y over {} pixels: GPU {gpu_mean:.5}, CPU {cpu_mean:.5}",
        PIXELS.len()
    );
    assert!(gpu_mean > 0.0, "the GPU residual is empty");
    assert_z_stats(&zs, Z_LIMIT, "GPU residual against the CPU reference");
    // The kept signature must be one of the `d1_reflection_signatures` set.
    assert!(d1_reflection_signatures(6).contains(
        &crate::renderer::path_cache::signature::pack_sig(top_reflection())
    ));
}

/// A device with fewer storage buffers per stage than the pass needs is declined by the pure
/// check; the hardware here must satisfy it for the tests above to mean anything.
#[test]
fn this_adapter_binds_the_reference_pass() {
    let Some(backend) = backend_or_skip("this_adapter_binds_the_reference_pass") else {
        return;
    };
    let rig = BeamRig::cube(glass());
    let (outcome, _, _) = residual_pass(&backend, &rig, &PackedKept::default(), (0, 1));
    assert_eq!(
        outcome,
        GpuAccumulate::Done,
        "the adapter declined the reference pass (storage buffers: need {REFERENCE_LIVE_STORAGE_BUFFERS})"
    );
}

// ---- the uploaded kept set ----

/// [`residual_pass`] with a kept set uploaded beforehand.
fn uploaded_pass(
    backend: &GpuBackend,
    rig: &BeamRig,
    kept: &super::kept::KeptBufferSet,
    n_lambda: u32,
    (sample_offset, spp): (u32, u32),
) -> (GpuAccumulate, Vec<Vec3>, Vec<Vec3>) {
    let frame_scene = rig.scene(FRAME, MAX_BOUNCES);
    let scene = GpuSceneRef {
        camera: frame_scene.camera,
        width: FRAME,
        height: FRAME,
        planes: frame_scene.planes,
        facet_finishes: &[],
        material: frame_scene.material,
        max_bounces: frame_scene.max_bounces,
        environment: frame_scene.environment,
        estimator: None,
    };
    let live = LiveTiles::all(FRAME, FRAME, TILE);
    let len = (FRAME * FRAME) as usize;
    let mut accum = vec![Vec3::ZERO; len];
    let mut half_b = vec![Vec3::ZERO; len];
    let never_cancel = AtomicBool::new(false);
    let outcome = backend.try_accumulate_reference_live_uploaded_cancellable(
        &scene,
        &[],
        &SurfaceScene::IDEAL,
        sample_offset,
        spp,
        &live,
        kept,
        n_lambda,
        &mut accum,
        &mut half_b,
        &never_cancel,
    );
    (outcome, accum, half_b)
}

/// A batch uploads its kept regions once however many passes it runs, and an uploaded pass is the
/// packed pass bit for bit.
#[test]
fn a_batch_uploads_its_kept_regions_once_for_any_number_of_passes() {
    let Some(backend) =
        backend_or_skip("a_batch_uploads_its_kept_regions_once_for_any_number_of_passes")
    else {
        return;
    };
    let rig = BeamRig::cube(glass());
    let kept = frame_kept(top_reflection());
    let pass = (0, 16);
    let (outcome, packed, packed_half) = residual_pass(&backend, &rig, &kept, pass);
    assert_eq!(outcome, GpuAccumulate::Done);

    let before = super::kept::upload_count();
    let set = backend
        .upload_reference_kept(&kept, N_LAMBDA)
        .expect("the kept set fits");
    assert_eq!(super::kept::upload_count(), before + 1);
    for _ in 0..3 {
        let (outcome, accum, half_b) = uploaded_pass(&backend, &rig, &set, N_LAMBDA, pass);
        assert_eq!(outcome, GpuAccumulate::Done);
        assert_eq!(bits(&accum), bits(&packed));
        assert_eq!(bits(&half_b), bits(&packed_half));
    }
    assert_eq!(
        super::kept::upload_count(),
        before + 1,
        "three uploaded passes must not upload again"
    );

    // The packed entry uploads for every pass.
    let _ = residual_pass(&backend, &rig, &kept, pass);
    assert_eq!(super::kept::upload_count(), before + 2);

    // A set for another node count is declined, and nothing is added to the sums.
    let (outcome, accum, _) = uploaded_pass(&backend, &rig, &set, N_LAMBDA + 1, pass);
    assert_eq!(outcome, GpuAccumulate::Declined);
    assert!(accum.iter().all(|p| *p == Vec3::ZERO));
}
