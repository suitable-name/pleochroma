//! Stress rows of the beam renderer (beam guide §16.4).
//!
//! The rows: the benchmark stone of scene s6 at 7680 x 4320 (a small tile set, and the whole
//! frame's beam part), a generated 200-facet convex design, grazing poses where the exit fans out,
//! and the overflow of an engine's pixel cap (batches are cut under the memory cap, and an engine
//! that still declines hands over to the CPU, same image). Every row asserts no panic, no lost tile
//! (every pixel written exactly once) and finite, non-negative pixels, and prints its numbers. The
//! long rows are `#[ignore]`d: run them with `cargo test -- --ignored`. The time bounds are
//! generous named constants.

use std::{sync::atomic::AtomicBool, time::Duration};

use glam::Vec3;

use super::{
    BeamSettings, ResidualMode,
    engines::EngineSet,
    engines_test_hooks::{
        Bench, Fault, Injected, InjectionLog, Run, Stage, Trigger, assert_written_exactly_once,
        beam_settings, boxed, fixed_settings, image_bits, written_counts,
    },
    frame::BeamFrame,
    pipeline::{batch_share, runs_for_share, tile_cost},
    tests_support::{BeamRig, MAX_BOUNCES, diamond, glass, sapphire},
};
use crate::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::raytracer::Camera,
    renderer::{adaptive::LiveTiles, frame_scene::FrameScene},
};

/// The width of the s6 frame.
const S6_WIDTH: u32 = 7680;

/// The height of the s6 frame.
const S6_HEIGHT: u32 = 4320;

/// The bounce limit of the s6 job.
const S6_BOUNCES: u32 = 32;

/// The tile edge of the s6 rows.
const S6_TILE: u32 = 16;

/// Residual samples per pixel of the s6 tile-set row.
const S6_SPP: u32 = 8;

/// The time the s6 tile-set row may take.
const S6_TILES_BOUND: Duration = Duration::from_mins(15);

/// The time the s6 whole-frame beam part may take.
const S6_FRAME_BOUND: Duration = Duration::from_hours(4);

/// The number of facets of the generated design.
const MANY_FACETS: u16 = 200;

/// The side of the small stress frames in pixels.
const SMALL_FRAME: u32 = 32;

/// The tile edge of the small stress frames.
const SMALL_TILE: u32 = 8;

/// Samples per pixel of the small stress frames.
const SMALL_SPP: u32 = 8;

/// The time a small stress frame may take.
const SMALL_BOUND: Duration = Duration::from_secs(600);

/// The camera elevations above the girdle plane of the grazing rows, in radians (about 2.9 and
/// 1.1 degrees).
const GRAZING_PITCHES: [f32; 2] = [0.05, 0.02];

/// Samples per pixel of the cap-overflow rows.
const CAP_SPP: u32 = 16;

/// The tiles of the 32-pixel, 8-pixel-tile frame.
const CAP_TILES: u32 = 16;

/// The pixels of one tile.
const TILE_PIXELS: usize = (SMALL_TILE * SMALL_TILE) as usize;

/// Asserts the properties every finished stress render has: finite non-negative pixels, `spp`
/// residual samples in every written pixel (none outside), and every pixel of the tiles to render
/// written exactly once. Returns the number of pixels written.
fn check_finished(
    run: &Run,
    size: (u32, u32),
    (tile, spp): (u32, u32),
    restrict: Option<&LiveTiles>,
) -> usize {
    assert_written_exactly_once(run, size, tile, restrict);
    let written = written_counts(run, size);
    let mut pixels = 0_usize;
    for (index, &count) in written.iter().enumerate() {
        let pixel = run.image.pixels[index];
        assert!(pixel.is_finite(), "pixel {index} is not finite");
        assert!(pixel.min_element() >= 0.0, "pixel {index} is negative");
        if count == 1 {
            pixels += 1;
            assert_eq!(run.image.counts[index], spp, "samples of pixel {index}");
        } else {
            assert_eq!(run.image.counts[index], 0, "samples of unwritten {index}");
            assert_eq!(pixel, Vec3::ZERO, "unwritten pixel {index}");
        }
    }
    pixels
}

/// Prints the numbers of a stress row.
fn report(label: &str, run: &Run) {
    eprintln!(
        "{label}: {:?}; primary beams {}, beams {}, leaves {}, drops {:?}, fragments {}, pieces {}, \
         residual samples {}, {} batch piece(s)",
        run.elapsed,
        run.stats.primary_beams,
        run.stats.beams,
        run.stats.leaves,
        run.stats.drops,
        run.stats.fragments,
        run.stats.fragment_pieces,
        run.image.samples_traced,
        run.batches.len()
    );
}

/// Renders with the CPU engine alone, never cancelled; panics when the frame is not beam traced.
fn render_cpu(bench: &Bench<'_>, restrict: Option<&LiveTiles>, label: &str) -> Run {
    let cancel = AtomicBool::new(false);
    let engines = EngineSet::new(vec![boxed(bench.cpu(super::pipeline::default_threads()))]);
    match bench.render(&engines, restrict, &cancel) {
        Ok(run) => run,
        Err(off) => panic!("{label}: the frame is not beam traced: {off:?}"),
    }
}

// ---- s6 at 8K ----

/// The s6 frame: the Pentalume Sapphire at 7680 x 4320 with the job's bounce limit.
fn s6_scene(rig: &BeamRig) -> FrameScene<'_> {
    FrameScene {
        width: S6_WIDTH,
        height: S6_HEIGHT,
        max_bounces: S6_BOUNCES,
        ..rig.scene(SMALL_FRAME, S6_BOUNCES)
    }
}

#[test]
#[ignore = "stress: run with --ignored"]
fn the_s6_stone_at_8k_renders_a_tile_set_spread_over_the_frame() {
    // The whole 8K frame is too long for a test with a residual; this is the job's stone at the
    // job's size on a small tile set: a block at the center, and the first and last tile.
    let rig = BeamRig::pentalume(sapphire());
    let scene = s6_scene(&rig);
    let columns = S6_WIDTH.div_ceil(S6_TILE);
    let rows = S6_HEIGHT.div_ceil(S6_TILE);
    let mut members = vec![0_u32];
    for row in (rows / 2 - 1)..=(rows / 2 + 1) {
        for column in (columns / 2 - 1)..=(columns / 2 + 1) {
            members.push(row * columns + column);
        }
    }
    members.push(columns * rows - 1);
    let live = LiveTiles::from_indices(S6_WIDTH, S6_HEIGHT, S6_TILE, members.clone());
    let bench = Bench::new(
        &rig,
        &scene,
        beam_settings(4, S6_SPP),
        fixed_settings(S6_SPP, S6_SPP, S6_TILE),
    );
    let run = render_cpu(&bench, Some(&live), "s6 tile set");
    report("s6 8K tile set", &run);
    let pixels = check_finished(&run, (S6_WIDTH, S6_HEIGHT), (S6_TILE, S6_SPP), Some(&live));
    assert_eq!(pixels, members.len() * (S6_TILE * S6_TILE) as usize);
    assert!(run.stats.leaves > 0, "the center tiles hold the stone");
    assert!(run.elapsed <= S6_TILES_BOUND, "took {:?}", run.elapsed);
}

#[test]
#[ignore = "stress: run with --ignored (the whole 8K frame, beam part only, about 1.2 GB of image)"]
fn the_s6_stone_at_8k_renders_the_whole_frame_beam_part() {
    let rig = BeamRig::pentalume(sapphire());
    let scene = s6_scene(&rig);
    let beam = BeamSettings {
        residual: ResidualMode::Off,
        ..beam_settings(64, S6_SPP)
    };
    let bench = Bench::new(&rig, &scene, beam, fixed_settings(S6_SPP, S6_SPP, S6_TILE));
    let run = render_cpu(&bench, None, "s6 whole frame");
    report("s6 8K whole frame, beams only", &run);
    assert_written_exactly_once(&run, (S6_WIDTH, S6_HEIGHT), S6_TILE, None);
    assert!(run.image.pixels.iter().all(|p| p.is_finite()));
    assert!(run.image.pixels.iter().all(|p| p.min_element() >= 0.0));
    assert!(run.stats.leaves > 0);
    assert!(run.elapsed <= S6_FRAME_BOUND, "took {:?}", run.elapsed);
}

// ---- a 200-facet design ----

/// A convex polyhedron of `count` facets: planes tangent to the unit sphere at Fibonacci points,
/// so every plane bounds a face of the solid.
fn tangent_polyhedron(count: u16) -> Vec<GpuFacetPlane> {
    let golden_angle = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    (0..count)
        .map(|i| {
            let fraction = (f32::from(i) + 0.5) / f32::from(count);
            let z = 2.0_f32.mul_add(-fraction, 1.0);
            let ring = z.mul_add(-z, 1.0).max(0.0).sqrt();
            let phi = golden_angle * f32::from(i);
            let normal = Vec3::new(ring * phi.cos(), z, ring * phi.sin());
            GpuFacetPlane::new(normal, -1.0)
        })
        .collect()
}

#[test]
fn the_generated_polyhedron_has_the_facets_it_was_asked_for() {
    let planes = tangent_polyhedron(MANY_FACETS);
    assert_eq!(planes.len(), usize::from(MANY_FACETS));
    let mut sorted: Vec<[u32; 3]> = planes.iter().map(|p| p.normal.map(f32::to_bits)).collect();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        usize::from(MANY_FACETS),
        "two facets share a normal"
    );
    assert!(planes.iter().all(|p| p.d == -1.0));
}

#[test]
#[ignore = "stress: run with --ignored"]
fn a_200_facet_design_renders_every_tile_once_and_deterministically() {
    let rig = BeamRig::new(
        tangent_polyhedron(MANY_FACETS),
        glass(),
        Camera::new(0.35, 0.28, 5.0, 30.0),
    );
    let scene = rig.scene(SMALL_FRAME, MAX_BOUNCES);
    let bench = Bench::new(
        &rig,
        &scene,
        beam_settings(4, SMALL_SPP),
        fixed_settings(SMALL_SPP, SMALL_SPP, SMALL_TILE),
    );
    let first = render_cpu(&bench, None, "200 facets");
    report("200 facets", &first);
    let pixels = check_finished(
        &first,
        (SMALL_FRAME, SMALL_FRAME),
        (SMALL_TILE, SMALL_SPP),
        None,
    );
    assert_eq!(pixels, (SMALL_FRAME * SMALL_FRAME) as usize);
    assert!(first.stats.leaves > 0);
    assert!(first.elapsed <= SMALL_BOUND, "took {:?}", first.elapsed);
    let again = render_cpu(&bench, None, "200 facets, again");
    assert_eq!(image_bits(&first.image), image_bits(&again.image));
}

// ---- grazing fan-out ----

#[test]
#[ignore = "stress: run with --ignored"]
fn a_camera_a_few_degrees_above_the_girdle_plane_renders_every_tile_once() {
    for pitch in GRAZING_PITCHES {
        let rig = BeamRig::new(
            StandardGemCuts::standard_round_brilliant(),
            diamond(),
            Camera::new(0.35, pitch, 5.0, 18.0),
        );
        let scene = rig.scene(SMALL_FRAME, MAX_BOUNCES);
        let bench = Bench::new(
            &rig,
            &scene,
            beam_settings(4, SMALL_SPP),
            fixed_settings(SMALL_SPP, SMALL_SPP, SMALL_TILE),
        );
        let label = format!("grazing pitch {pitch}");
        let run = render_cpu(&bench, None, &label);
        report(&label, &run);
        let pixels = check_finished(
            &run,
            (SMALL_FRAME, SMALL_FRAME),
            (SMALL_TILE, SMALL_SPP),
            None,
        );
        assert_eq!(pixels, (SMALL_FRAME * SMALL_FRAME) as usize);
        assert!(
            run.elapsed <= SMALL_BOUND,
            "{label}: took {:?}",
            run.elapsed
        );
    }
}

// ---- GPU-cap overflow ----

/// The cube bench with `memory_cap_bytes` and one batch of every tile.
fn cap_bench<'r>(
    rig: &'r BeamRig,
    scene: &'r FrameScene<'r>,
    memory_cap_bytes: Option<u64>,
) -> Bench<'r> {
    let mut beam = beam_settings(CAP_TILES, CAP_SPP);
    if let Some(cap) = memory_cap_bytes {
        beam.memory_cap_bytes = cap;
    }
    Bench::new(
        rig,
        scene,
        beam,
        fixed_settings(CAP_SPP, CAP_SPP, SMALL_TILE),
    )
}

/// The memory estimate (`pipeline::tile_cost`) of each tile of the cap bench's frame, in order.
fn cap_tile_costs(scene: &FrameScene<'_>) -> Vec<u64> {
    let beam = beam_settings(CAP_TILES, CAP_SPP);
    let frame = BeamFrame::new(scene, &beam).expect("an eligible stone");
    let cancel = AtomicBool::new(false);
    (0..CAP_TILES as usize)
        .map(|index| {
            let (leaves, _, rect) = frame
                .tile_leaves(index, SMALL_TILE, &cancel)
                .expect("not cancelled");
            tile_cost(rect, &leaves)
        })
        .collect()
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn an_engine_over_its_cap_declines_to_the_cpu_and_the_batches_that_fit_stay_on_it() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(SMALL_FRAME, MAX_BOUNCES);
    let cancel = AtomicBool::new(false);
    let size = (SMALL_FRAME, SMALL_FRAME);
    let roomy = cap_bench(&rig, &scene, None);
    let expected = image_bits(&render_cpu(&roomy, None, "cap reference").image);

    // The whole batch (16 tiles) is over the engine's cap and fits the memory cap: one decline,
    // the pipeline's CPU engine takes the batch.
    let log = InjectionLog::new();
    let capped = Injected::new(roomy.cpu(2), "gpu", &log).with_pixel_cap(4 * TILE_PIXELS);
    let run = roomy
        .render(&EngineSet::new(vec![boxed(capped)]), None, &cancel)
        .expect("an eligible stone");
    assert_eq!(log.declines(), 1);
    assert_eq!(log.fragment_calls(), 1);
    assert_eq!(run.batches.len(), 1);
    assert_eq!(image_bits(&run.image), expected);
    assert_written_exactly_once(&run, size, SMALL_TILE, None);

    // A memory cap whose batch share is the widest tile: no tile is over the share, so none
    // becomes residual-only and the image stays the same. The batch is cut where the running
    // estimate passes the share; the engine's cap holds the widest piece, so it serves every piece
    // and never declines.
    let costs = cap_tile_costs(&scene);
    let cap = costs.iter().copied().max().expect("some tile") * 2;
    let runs = runs_for_share(&costs, batch_share(cap));
    let pieces = u32::try_from(runs.len()).expect("a few pieces");
    let widest_piece = runs
        .iter()
        .map(ExactSizeIterator::len)
        .max()
        .expect("a piece");
    assert!(pieces > 1, "the cap must cut the batch");
    let tight = cap_bench(&rig, &scene, Some(cap));
    let log = InjectionLog::new();
    let capped =
        Injected::new(tight.cpu(2), "gpu", &log).with_pixel_cap(widest_piece * TILE_PIXELS);
    let run = tight
        .render(&EngineSet::new(vec![boxed(capped)]), None, &cancel)
        .expect("an eligible stone");
    eprintln!(
        "cap overflow: {} piece(s), engine asked {} / {} time(s), {} decline(s)",
        run.batches.len(),
        log.fragment_calls(),
        log.residual_calls(),
        log.declines()
    );
    assert_eq!(run.batches.len(), pieces as usize);
    assert_eq!(log.declines(), 0);
    assert_eq!(log.fragment_calls(), pieces);
    assert_eq!(log.residual_calls(), pieces);
    assert_eq!(image_bits(&run.image), expected);
    assert_written_exactly_once(&run, size, SMALL_TILE, None);

    // A cap below one tile: cutting cannot help; the first piece declines and the CPU takes the
    // rest, asked once.
    let log = InjectionLog::new();
    let capped = Injected::new(tight.cpu(2), "gpu", &log).with_pixel_cap(TILE_PIXELS - 1);
    let run = tight
        .render(&EngineSet::new(vec![boxed(capped)]), None, &cancel)
        .expect("an eligible stone");
    assert_eq!(run.batches.len(), pieces as usize);
    assert_eq!(log.declines(), 1);
    assert_eq!(log.fragment_calls(), 1);
    assert_eq!(image_bits(&run.image), expected);
    assert_written_exactly_once(&run, size, SMALL_TILE, None);
}

#[test]
fn a_cap_decline_and_a_later_device_loss_in_one_render_still_give_the_same_image() {
    // Two engines: the first over its cap on the whole batch, the second losing its device after
    // its first fragments; the pipeline's CPU engine finishes. No panic, nothing lost.
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(SMALL_FRAME, MAX_BOUNCES);
    let cancel = AtomicBool::new(false);
    let bench = cap_bench(&rig, &scene, Some(1));
    let expected = image_bits(&render_cpu(&bench, None, "cap reference").image);
    let logs = [InjectionLog::new(), InjectionLog::new()];
    let first = Injected::new(bench.cpu(2), "first", &logs[0]).with_pixel_cap(TILE_PIXELS - 1);
    let second = Injected::new(bench.cpu(3), "second", &logs[1]).with_fault(
        Trigger::Call {
            nth: 1,
            stage: Stage::Residual,
        },
        Fault::LoseDevice,
    );
    let run = bench
        .render(
            &EngineSet::new(vec![boxed(first), boxed(second)]),
            None,
            &cancel,
        )
        .expect("an eligible stone");
    assert_eq!(logs[0].declines(), 1);
    assert!(logs[1].lost());
    assert_eq!(image_bits(&run.image), expected);
    assert_written_exactly_once(&run, (SMALL_FRAME, SMALL_FRAME), SMALL_TILE, None);
}

#[test]
fn an_ineligible_frame_stays_a_beam_off_under_every_injection() {
    // A rounded edge is refused before any engine is asked: the injection never fires.
    let mut rounded = glass();
    rounded.edge_rounding_radius = 0.1;
    let rig = BeamRig::cube(rounded);
    let scene = rig.scene(SMALL_FRAME, MAX_BOUNCES);
    let bench = cap_bench(&rig, &scene, None);
    // `bench.beam` is the setting the frame check reads.
    let log = InjectionLog::new();
    let flaky = Injected::new(bench.cpu(1), "flaky", &log).with_fault(
        Trigger::Call {
            nth: 0,
            stage: Stage::Fragments,
        },
        Fault::LoseDevice,
    );
    let cancel = AtomicBool::new(false);
    let result = bench.render(&EngineSet::new(vec![boxed(flaky)]), None, &cancel);
    let expected = BeamFrame::new(&scene, &bench.beam).err();
    assert!(expected.is_some(), "the frame must be ineligible");
    assert_eq!(result.err(), expected);
    assert_eq!(log.fragment_calls(), 0);
    assert!(!log.fired());
}
