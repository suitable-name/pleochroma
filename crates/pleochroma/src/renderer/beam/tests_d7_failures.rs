//! Failure-injection rows of the beam renderer (beam guide §12.5, §14.5, §16.4).
//!
//! Engines from `engines_test_hooks` decline, lose their device or cancel at a chosen batch or
//! dispatch. The rows assert that the next engine (or the pipeline's own CPU engine) takes over
//! with the same image, that no render panics, that every pixel of a finished render is written
//! exactly once and that a cancelled render returns within the bound with nothing written twice.
//! The engines wrap the CPU engine, so "the same image" is bit for bit (fixed sample counts, one
//! platform). A real GPU device loss or a real remote disconnect is the same event to the
//! pipeline: a `Declined` answer.

use std::{sync::atomic::AtomicBool, time::Duration};

use super::{
    BeamPhase,
    engines::EngineSet,
    engines_test_hooks::{
        Bench, Fault, ImageBits, Injected, InjectionLog, Stage, Trigger, beam_settings, boxed,
        fixed_settings, image_bits, next_random,
    },
    tests_support::{BeamRig, glass},
};
use crate::renderer::adaptive::LiveTiles;

/// The bounce limit of every render. The rows check bit identity, deterministic failover and
/// cancellation, none of which depends on tree depth; 4 bounces keep each render cheap.
const MAX_BOUNCES: u32 = 4;

/// The side of the square test frame in pixels.
const FRAME: u32 = 32;

/// The adaptive tile and the beam tile in pixels (16 tiles).
const TILE: u32 = 8;

/// Samples per pixel (fixed: no tile stops early, so the image does not depend on the batching).
const SPP: u32 = 16;

/// Samples per pass: the residual minimum (`SPP`), the pass the beam residual runs.
const PASS_SPP: u32 = SPP;

/// Tiles per batch of most rows: four batches of the 16-tile frame.
const BATCH_TILES: u32 = 4;

/// The batches of the 16-tile frame at `BATCH_TILES` tiles per batch.
const BATCHES: u32 = 4;

/// The generous bound on the time from the token being set to the entry returning (the guide's
/// figure is 100 ms; a loaded test machine gets ten times that).
const CANCEL_BOUND: Duration = Duration::from_millis(1000);

/// The random cancel points of the cancel row.
const CANCEL_TRIALS: usize = 12;

/// The seed of the cancel row's random points.
const CANCEL_SEED: u64 = 0x5EED_0D7A;

/// The tiles of the restricted rows (a remote worker's request).
const WORKER_TILES: [u32; 4] = [5, 6, 9, 10];

/// The size of the frame as a pair.
const SIZE: (u32, u32) = (FRAME, FRAME);

/// The bench of the brilliant rig with `batch_tiles` tiles per batch.
fn bench_of<'r>(
    rig: &'r BeamRig,
    scene: &'r crate::renderer::frame_scene::FrameScene<'r>,
    batch_tiles: u32,
) -> Bench<'r> {
    Bench::new(
        rig,
        scene,
        beam_settings(batch_tiles, SPP),
        fixed_settings(SPP, PASS_SPP, TILE),
    )
}

/// The image bits of the CPU engine alone on `restrict`.
fn reference(bench: &Bench<'_>, restrict: Option<&LiveTiles>) -> ImageBits {
    let cancel = AtomicBool::new(false);
    let engines = EngineSet::new(vec![boxed(bench.cpu(2))]);
    let run = bench
        .render(&engines, restrict, &cancel)
        .expect("an eligible stone");
    assert!(
        run.image.pixels.iter().any(|p| p.y > 0.0),
        "an empty reference proves nothing"
    );
    image_bits(&run.image)
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn an_engine_declining_at_batch_k_hands_the_batch_over_with_the_same_image() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let bench = bench_of(&rig, &scene, BATCH_TILES);
    let expected = reference(&bench, None);
    let cancel = AtomicBool::new(false);
    for stage in [Stage::Fragments, Stage::Residual] {
        for k in 0..BATCHES {
            let log = InjectionLog::new();
            let trigger = Trigger::Batch { batch: k, stage };
            let flaky = Injected::new(bench.cpu(2), "flaky", &log)
                .with_fault(trigger, Fault::Decline("test: refused"));
            let engines = EngineSet::new(vec![boxed(flaky), boxed(bench.cpu(3))]);
            let run = bench
                .render(&engines, None, &cancel)
                .expect("an eligible stone");
            // Two live engines deal the batches in turn: engine 0 sees batches 0 and 2 only.
            let reached = k <= 2;
            eprintln!(
                "decline at batch {k} ({stage:?}): fired {}, declines {}",
                log.fired(),
                log.declines()
            );
            assert_eq!(log.fired(), reached, "{stage:?} at batch {k}");
            assert_eq!(log.declines(), u32::from(reached));
            assert_eq!(image_bits(&run.image), expected, "{stage:?} at batch {k}");
            super::engines_test_hooks::assert_written_exactly_once(&run, SIZE, TILE, None);
        }
    }
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_decline_is_deterministic_run_to_run() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let bench = bench_of(&rig, &scene, BATCH_TILES);
    let cancel = AtomicBool::new(false);
    let mut images = Vec::new();
    for _ in 0..2 {
        let log = InjectionLog::new();
        let trigger = Trigger::Batch {
            batch: 1,
            stage: Stage::Residual,
        };
        let flaky = Injected::new(bench.cpu(2), "flaky", &log)
            .with_fault(trigger, Fault::Decline("test: refused"));
        let engines = EngineSet::new(vec![boxed(flaky), boxed(bench.cpu(3))]);
        let run = bench
            .render(&engines, None, &cancel)
            .expect("an eligible stone");
        images.push(image_bits(&run.image));
    }
    assert_eq!(images[0], images[1]);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_device_lost_between_the_stages_reruns_the_batch_on_the_cpu() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let bench = bench_of(&rig, &scene, BATCH_TILES);
    let expected = reference(&bench, None);
    let cancel = AtomicBool::new(false);
    for k in 0..BATCHES {
        // The only engine: the pipeline's own CPU engine takes over from the lost device.
        let log = InjectionLog::new();
        let trigger = Trigger::Batch {
            batch: k,
            stage: Stage::Residual,
        };
        let gpu = Injected::new(bench.cpu(2), "gpu", &log).with_fault(trigger, Fault::LoseDevice);
        let engines = EngineSet::new(vec![boxed(gpu)]);
        let run = bench
            .render(&engines, None, &cancel)
            .expect("an eligible stone");
        assert!(log.fired() && log.lost(), "device loss at batch {k}");
        // The engine is asked once per batch up to the loss, never again afterwards.
        assert_eq!(log.fragment_calls(), k + 1);
        assert_eq!(log.residual_calls(), k + 1);
        assert_eq!(log.declines(), 1);
        assert_eq!(image_bits(&run.image), expected, "device loss at batch {k}");
        super::engines_test_hooks::assert_written_exactly_once(&run, SIZE, TILE, None);

        // With a second engine alive, the survivor takes the rest.
        let log = InjectionLog::new();
        let gpu = Injected::new(bench.cpu(2), "gpu", &log).with_fault(trigger, Fault::LoseDevice);
        let engines = EngineSet::new(vec![boxed(gpu), boxed(bench.cpu(3))]);
        let run = bench
            .render(&engines, None, &cancel)
            .expect("an eligible stone");
        assert_eq!(log.lost(), k <= 2, "device loss at batch {k}, two engines");
        assert_eq!(image_bits(&run.image), expected);
    }
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_remote_worker_that_disconnects_fails_over_with_the_same_image() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    // One tile per batch: four batches for the worker's four tiles.
    let bench = bench_of(&rig, &scene, 1);
    let live = LiveTiles::from_indices(FRAME, FRAME, TILE, WORKER_TILES.to_vec());
    let expected = reference(&bench, Some(&live));
    let cancel = AtomicBool::new(false);
    for nth in 0..3 {
        let log = InjectionLog::new();
        let trigger = Trigger::Call {
            nth,
            stage: Stage::Fragments,
        };
        let remote = Injected::new(bench.cpu(2), "remote 10.0.0.7", &log)
            .with_fault(trigger, Fault::Decline("test: the worker disconnected"));
        let engines = EngineSet::new(vec![boxed(remote), boxed(bench.cpu(2))]);
        let run = bench
            .render(&engines, Some(&live), &cancel)
            .expect("an eligible stone");
        // The remote engine is dealt batches 0 and 2 while it lives.
        assert_eq!(log.fired(), nth <= 1, "disconnect at dispatch {nth}");
        assert_eq!(image_bits(&run.image), expected, "dispatch {nth}");
        super::engines_test_hooks::assert_written_exactly_once(&run, SIZE, TILE, Some(&live));
    }
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_chain_of_failing_engines_ends_on_the_pipelines_cpu_with_the_same_image() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let bench = bench_of(&rig, &scene, BATCH_TILES);
    let expected = reference(&bench, None);
    let cancel = AtomicBool::new(false);
    let logs = [
        InjectionLog::new(),
        InjectionLog::new(),
        InjectionLog::new(),
    ];
    // Engine 0 declines batch 0 at once; engine 1 loses its device at the residual of batch 1;
    // engine 2 declines its first fragment request. Every engine is gone by batch 1 and the
    // pipeline's own CPU engine finishes the render.
    let first = Injected::new(bench.cpu(2), "first", &logs[0]).with_fault(
        Trigger::Batch {
            batch: 0,
            stage: Stage::Fragments,
        },
        Fault::Decline("test: no device"),
    );
    let second = Injected::new(bench.cpu(3), "second", &logs[1]).with_fault(
        Trigger::Batch {
            batch: 1,
            stage: Stage::Residual,
        },
        Fault::LoseDevice,
    );
    let third = Injected::new(bench.cpu(1), "third", &logs[2]).with_fault(
        Trigger::Call {
            nth: 0,
            stage: Stage::Fragments,
        },
        Fault::Decline("test: the worker disconnected"),
    );
    let engines = EngineSet::new(vec![boxed(first), boxed(second), boxed(third)]);
    let run = bench
        .render(&engines, None, &cancel)
        .expect("an eligible stone");
    for (index, log) in logs.iter().enumerate() {
        assert!(log.fired(), "engine {index} never failed");
        assert_eq!(log.declines(), 1, "engine {index} was asked after failing");
    }
    assert_eq!(image_bits(&run.image), expected);
    super::engines_test_hooks::assert_written_exactly_once(&run, SIZE, TILE, None);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_cancel_at_a_random_point_returns_at_once_and_writes_nothing_twice() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let bench = bench_of(&rig, &scene, BATCH_TILES);
    let mut rng_state = CANCEL_SEED;
    for trial in 0..CANCEL_TRIALS {
        rng_state = next_random(rng_state);
        // The first trial cancels before anything is finished; the others at a random batch.
        let batch = if trial == 0 {
            0
        } else {
            u32::try_from(rng_state % u64::from(BATCHES)).expect("below the batch count")
        };
        let stage = if (rng_state >> 8) & 1 == 0 {
            Stage::Fragments
        } else {
            Stage::Residual
        };
        // Half the trials name the point by batch, half by the dispatch count (one request per
        // stage and batch, so the same point).
        let trigger = if (rng_state >> 16) & 1 == 0 {
            Trigger::Batch { batch, stage }
        } else {
            Trigger::Call { nth: batch, stage }
        };
        let token = AtomicBool::new(false);
        let log = InjectionLog::new();
        let canceller = Injected::new(bench.cpu(2), "cancelling", &log)
            .with_fault(trigger, Fault::Cancel)
            .with_cancel_token(&token);
        let engines = EngineSet::new(vec![boxed(canceller)]);
        let run = bench
            .render(&engines, None, &token)
            .expect("a cancelled render still returns its image");
        let fired_at = log.fired_at().expect("the cancel fired");
        let latency = run.returned_at.saturating_duration_since(fired_at);
        eprintln!("cancel trial {trial}: {trigger:?}, latency {latency:?}");
        assert!(latency <= CANCEL_BOUND, "trial {trial}: {latency:?}");
        // Batches 0..batch were finished and stored, once each; nothing after them.
        assert_eq!(run.batches.len(), batch as usize, "trial {trial}");
        let written = super::engines_test_hooks::assert_written_at_most_once(&run, SIZE);
        assert_eq!(
            written,
            batch as usize * BATCH_TILES as usize * (TILE * TILE) as usize
        );
        if let Some(last) = run.progress.last() {
            assert!(last.done < 1.0, "a cancelled render reports no end");
            assert_ne!(
                (last.phase, last.done),
                (BeamPhase::Finishing, 1.0),
                "trial {trial}"
            );
        }
        assert!(run.progress.windows(2).all(|p| p[0].done <= p[1].done));
    }
}

#[test]
fn a_token_set_by_a_fault_in_the_first_dispatch_leaves_the_image_empty() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let bench = bench_of(&rig, &scene, BATCH_TILES);
    let token = AtomicBool::new(false);
    let log = InjectionLog::new();
    let trigger = Trigger::Call {
        nth: 0,
        stage: Stage::Fragments,
    };
    let canceller = Injected::new(bench.cpu(2), "cancelling", &log)
        .with_fault(trigger, Fault::Cancel)
        .with_cancel_token(&token);
    let engines = EngineSet::new(vec![boxed(canceller)]);
    let run = bench
        .render(&engines, None, &token)
        .expect("a cancelled render still returns its image");
    assert_eq!(run.batches.len(), 0);
    assert_eq!(run.image.samples_traced, 0);
    assert!(run.image.counts.iter().all(|&c| c == 0));
    assert!(run.image.pixels.iter().all(|p| p.length() == 0.0));
    assert_eq!(
        super::engines_test_hooks::assert_written_at_most_once(&run, SIZE),
        0
    );
}
