//! Audit rows of the beam renderer (beam guide §12.4, §14.5, §14.7): cancel at random instants,
//! monotone progress, forced small caps, and the preloaded re-render.
//!
//! All rows render a 32 x 32 brilliant in glass with a fixed sample count (no tile stops early),
//! so "the same image" is bit for bit whatever the batching, the engine mix or the cut of a batch.

use std::{
    fmt::Debug,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use glam::Vec3;
use tracing::{
    Event, Metadata, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};

use super::{
    BeamPhase, BeamProgress, BeamRun,
    engines::EngineSet,
    engines_test_hooks::{
        Bench, ImageBits, Injected, InjectionLog, Run, assert_written_at_most_once, beam_settings,
        boxed, fixed_settings, image_bits, next_random, plain_extras,
    },
    leaf_cache::{LeafCache, RetainedBatch},
    pipeline::tile_cost,
    primary::TileRect,
    tests_support::{BeamRig, glass},
};
use crate::renderer::{cpu_frame::render_beam_hybrid_surface, frame_scene::FrameScene};

/// The bounce limit of every render. The rows check bit identity across batchings, caps and
/// caches, progress and cancellation, none of which depends on tree depth; 4 bounces keep each
/// render cheap.
const MAX_BOUNCES: u32 = 4;

/// The side of the square test frame in pixels.
const FRAME: u32 = 32;

/// The size of the frame as a pair.
const SIZE: (u32, u32) = (FRAME, FRAME);

/// The adaptive tile and the beam tile in pixels (16 tiles).
const TILE: u32 = 8;

/// Samples per pixel (fixed: no tile stops early).
const SPP: u32 = 16;

/// Samples per pass: the residual minimum (`SPP`), the pass the beam residual runs
/// (`tile_residual::residual_settings`).
const PASS_SPP: u32 = SPP;

/// Tiles per batch of most rows: four batches of the 16-tile frame.
const BATCH_TILES: u32 = 4;

/// The batches of the 16-tile frame at `BATCH_TILES` tiles per batch.
const BATCHES: usize = 4;

/// The tiles of the frame.
const TILES: usize = 16;

/// The guide's bound on the time from the token being set to the entry returning.
const CANCEL_BOUND: Duration = Duration::from_millis(100);

/// What a loaded test machine (the suite runs rows in parallel) may add to the bound.
const SCHEDULER_SLACK: Duration = Duration::from_millis(150);

/// The random cancel points of the cancel row.
const CANCEL_POINTS: usize = 20;

/// The seed of the cancel row's random points.
const CANCEL_SEED: u64 = 0x5EED_0D7B;

/// The cancel points range over this multiple of an uncancelled render's time, so a few arrive
/// after the render is done.
const CANCEL_SPAN: f64 = 1.1;

/// The log target of the beam renderer.
const TARGET: &str = "pleochroma::beam";

// ---- capturing the renderer's log lines ----

/// The messages a [`Capture`] saw.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<String>>>);

impl Lines {
    /// A copy of the messages so far.
    fn all(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// A subscriber that keeps the message of every event of the beam target.
struct Capture {
    /// Where the messages go.
    lines: Lines,
}

/// Reads the `message` field of an event.
struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl Subscriber for Capture {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.target() == TARGET
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut message = Message(String::new());
        event.record(&mut message);
        self.lines
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(message.0);
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

/// Runs `body` with the beam log of this thread captured; returns its result and the messages. The
/// pipeline hands this thread's subscriber to its producer thread and tree workers, so the renders
/// of a row run inside `body` and all their log lines arrive (the producer's early hand-over line
/// among them).
fn captured<R>(body: impl FnOnce() -> R) -> (R, Vec<String>) {
    let lines = Lines::default();
    let result = tracing::subscriber::with_default(
        Capture {
            lines: lines.clone(),
        },
        body,
    );
    (result, lines.all())
}

/// How many of `lines` contain `needle`.
fn count_lines(lines: &[String], needle: &str) -> usize {
    lines.iter().filter(|line| line.contains(needle)).count()
}

// ---- the harness ----

/// The bench of `rig` with `batch_tiles` tiles per batch and the beam settings `beam`.
fn bench_of<'r>(
    rig: &'r BeamRig,
    scene: &'r FrameScene<'r>,
    beam: super::BeamSettings,
) -> Bench<'r> {
    Bench::new(rig, scene, beam, fixed_settings(SPP, PASS_SPP, TILE))
}

/// The bench with the default memory cap and `batch_tiles` tiles per batch.
fn plain_bench<'r>(rig: &'r BeamRig, scene: &'r FrameScene<'r>, batch_tiles: u32) -> Bench<'r> {
    bench_of(rig, scene, beam_settings(batch_tiles, SPP))
}

/// One render through the hybrid entry with the recording hooks of the harness, plus the optional
/// `retain` hook and `preloaded` cache that `Bench::render` does not offer.
fn render_with(
    bench: &Bench<'_>,
    engines: &EngineSet<'_>,
    cancel: &AtomicBool,
    retain: Option<&mut dyn FnMut(RetainedBatch)>,
    preloaded: Option<&LeafCache>,
) -> Run {
    let started = Instant::now();
    let mut progress_log = Vec::new();
    let mut batches = Vec::new();
    let result = {
        let mut progress = |report: BeamProgress| progress_log.push(report);
        let mut on_batch = |tiles: &[TileRect], means: &[Vec3]| {
            batches.push((tiles.to_vec(), means.to_vec()));
        };
        render_beam_hybrid_surface(
            bench.scene,
            plain_extras(),
            &bench.rig.soa,
            &bench.beam,
            &bench.adaptive,
            engines,
            (0, 1.0),
            cancel,
            BeamRun {
                tiles: None,
                progress: &mut progress,
                on_batch: Some(&mut on_batch),
                // Coerced to the run's lifetime: an `Option<&mut dyn ..>` is invariant, so
                // passed as it is it would pin the run (and the local hooks above) to the
                // caller's lifetime.
                retain: retain.map(|retain| retain as &mut dyn FnMut(RetainedBatch)),
                preloaded,
            },
        )
    };
    let returned_at = Instant::now();
    let (image, stats) = result.expect("an eligible stone");
    Run {
        image,
        stats,
        progress: progress_log,
        batches,
        elapsed: returned_at.saturating_duration_since(started),
        returned_at,
    }
}

/// A render of the bench on one CPU engine, with nothing special.
fn plain_render(bench: &Bench<'_>) -> Run {
    let cancel = AtomicBool::new(false);
    let engines = EngineSet::new(vec![boxed(bench.cpu(2))]);
    render_with(bench, &engines, &cancel, None, None)
}

/// The image bits of a plain render, checked to be a real image.
fn reference(bench: &Bench<'_>) -> (ImageBits, Run) {
    let run = plain_render(bench);
    assert!(
        run.image.pixels.iter().any(|p| p.y > 0.0),
        "an empty reference proves nothing"
    );
    (image_bits(&run.image), run)
}

/// Checks the progress contract over a whole render: every value in 0 to 1 and never decreasing,
/// the phase going back to tracing at most once per batch, the time left present once it appears,
/// and the last report exactly `{ Finishing, 1, 0 s }`.
fn assert_progress_contract(progress: &[BeamProgress], batches: usize, label: &str) {
    assert!(!progress.is_empty(), "{label}: no report at all");
    let mut last = 0.0_f32;
    let mut eta_seen = false;
    let mut returns_to_tracing = 0_usize;
    let mut previous: Option<BeamPhase> = None;
    for (position, report) in progress.iter().enumerate() {
        assert!(
            (0.0..=1.0).contains(&report.done),
            "{label}: report {position} is {}",
            report.done
        );
        assert!(
            report.done >= last,
            "{label}: report {position} went back from {last} to {}",
            report.done
        );
        last = report.done;
        if report.eta.is_some() {
            eta_seen = true;
        } else {
            assert!(
                !eta_seen,
                "{label}: report {position} lost the time left after it appeared"
            );
        }
        if previous == Some(BeamPhase::Finishing) && report.phase == BeamPhase::Tracing {
            returns_to_tracing += 1;
        }
        previous = Some(report.phase);
    }
    assert!(
        returns_to_tracing <= batches,
        "{label}: the phase went back to tracing {returns_to_tracing} times for {batches} batches"
    );
    let end = progress.last().copied().expect("a last report");
    assert_eq!(end.phase, BeamPhase::Finishing, "{label}");
    assert!(
        (end.done - 1.0).abs() < f32::EPSILON,
        "{label}: ends at {}",
        end.done
    );
    assert_eq!(end.eta, Some(Duration::ZERO), "{label}");
}

// ---- cancel ----

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_cancel_at_random_instants_returns_within_the_bound_and_finishes_nothing() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let bench = plain_bench(&rig, &scene, BATCH_TILES);
    // The time of an uncancelled render sets the range of the random points.
    let (_, calm) = reference(&bench);
    let whole = calm.elapsed.as_secs_f64();
    eprintln!("an uncancelled render takes {whole:.3} s");
    let mut state = CANCEL_SEED;
    let mut worst = Duration::ZERO;
    let mut cancelled_runs = 0_usize;
    for point in 0..CANCEL_POINTS {
        state = next_random(state);
        // A fraction of the span in 0..=1 from the top 32 bits of the random word.
        let top = u32::try_from(state >> 32).unwrap_or(u32::MAX);
        let fraction = f64::from(top) / f64::from(u32::MAX);
        let delay = Duration::from_secs_f64(whole * CANCEL_SPAN * fraction);
        let cancel = AtomicBool::new(false);
        let set_at: Mutex<Option<Instant>> = Mutex::new(None);
        let engines = EngineSet::new(vec![boxed(bench.cpu(2))]);
        let run = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(delay);
                *set_at.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
                cancel.store(true, Ordering::SeqCst);
            });
            captured(|| render_with(&bench, &engines, &cancel, None, None))
        });
        // The pipeline's own log says how long the producer thread took to stop after the calling
        // thread did; a failure prints it, as the part of the latency a tree loop would own.
        let (run, log) = run;
        let stopped: Vec<&String> = log
            .iter()
            .filter(|line| line.contains("stopped by the cancel token"))
            .collect();
        let set = *set_at.lock().unwrap_or_else(PoisonError::into_inner);
        let latency = set.map_or(Duration::ZERO, |at| {
            run.returned_at.saturating_duration_since(at)
        });
        worst = worst.max(latency);
        let finished = run.progress.last().is_some_and(|report| {
            report.phase == BeamPhase::Finishing
                && report.done >= 1.0
                && report.eta == Some(Duration::ZERO)
        });
        eprintln!(
            "point {point}: cancel after {:.3} s, returned {} ms later, finished {finished}, \
             batches written {}, pipeline log {stopped:?}",
            delay.as_secs_f64(),
            latency.as_millis(),
            run.batches.len()
        );
        assert!(
            latency <= CANCEL_BOUND + SCHEDULER_SLACK,
            "point {point}: the entry took {latency:?} to return after the cancel \
             (pipeline log {stopped:?})"
        );
        if finished {
            assert_progress_contract(&run.progress, BATCHES, "a render the cancel missed");
        } else {
            cancelled_runs += 1;
            // Nothing is written twice and what was not finished stays zero; there is no closing
            // report of a render that did not finish.
            let written = assert_written_at_most_once(&run, SIZE);
            assert!(written <= FRAME as usize * FRAME as usize);
        }
    }
    eprintln!("worst cancel latency {worst:?}, {cancelled_runs} of {CANCEL_POINTS} were cancelled");
    assert!(
        cancelled_runs > 0,
        "no random point landed inside the render"
    );
}

// ---- progress ----

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn progress_never_decreases_over_a_whole_render_for_any_batching_and_engine_mix() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let cancel = AtomicBool::new(false);
    for batch_tiles in [1_u32, BATCH_TILES, 16] {
        let bench = plain_bench(&rig, &scene, batch_tiles);
        let batches = TILES.div_ceil(batch_tiles as usize);
        // One CPU engine, two CPU engines, and an engine that fails halfway.
        let single = EngineSet::new(vec![boxed(bench.cpu(2))]);
        let run = render_with(&bench, &single, &cancel, None, None);
        assert_progress_contract(
            &run.progress,
            batches,
            &format!("one engine, {batches} batches"),
        );

        let pair = EngineSet::new(vec![boxed(bench.cpu(2)), boxed(bench.cpu(3))]);
        let run = render_with(&bench, &pair, &cancel, None, None);
        assert_progress_contract(
            &run.progress,
            batches,
            &format!("two engines, {batches} batches"),
        );

        let log = InjectionLog::new();
        let flaky = Injected::new(bench.cpu(2), "flaky", &log).with_pixel_cap(1);
        let failing = EngineSet::new(vec![boxed(flaky), boxed(bench.cpu(3))]);
        let run = render_with(&bench, &failing, &cancel, None, None);
        assert_progress_contract(
            &run.progress,
            batches,
            &format!("a declining engine, {batches} batches"),
        );
    }
}

// ---- caps ----

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_forced_small_memory_cap_gives_the_same_image_and_one_log_line_per_early_hand_over() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let (expected, _) = reference(&plain_bench(&rig, &scene, BATCH_TILES));
    // A cap whose batch share is the widest tile: no tile is over the share (none becomes
    // residual-only, so the image stays the same), and the widest tile joins no other.
    let (kept, _) = kept_batches(&plain_bench(&rig, &scene, BATCH_TILES));
    let widest = kept
        .iter()
        .flat_map(|batch| {
            batch
                .tiles
                .iter()
                .zip(&batch.leaves)
                .map(|(&rect, (_, leaves))| tile_cost(rect, leaves))
        })
        .max()
        .expect("some tile");
    let mut beam = beam_settings(BATCH_TILES, SPP);
    beam.memory_cap_bytes = widest * 2;
    let bench = bench_of(&rig, &scene, beam);
    let (run, lines) = captured(|| plain_render(&bench));
    for line in &lines {
        eprintln!("log: {line}");
    }
    assert_eq!(image_bits(&run.image), expected);
    // The producer hands a batch over early once per extra part: one log line each, and no engine
    // declined.
    assert!(run.batches.len() > BATCHES, "the cap must split a batch");
    assert_eq!(
        count_lines(&lines, "memory cap"),
        run.batches.len() - BATCHES
    );
    assert_eq!(count_lines(&lines, "declined"), 0);
    // A batch that fits logs no cut.
    let (calm, calm_lines) = captured(|| plain_render(&plain_bench(&rig, &scene, BATCH_TILES)));
    assert_eq!(image_bits(&calm.image), expected);
    assert_eq!(count_lines(&calm_lines, "memory cap"), 0);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_forced_small_engine_binding_routes_the_batch_to_the_cpu_with_one_log_line() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    // One batch of all 16 tiles: 1024 pixels against an engine that takes 256.
    let bench = plain_bench(&rig, &scene, 16);
    let (expected, _) = reference(&bench);
    let log = InjectionLog::new();
    let capped = Injected::new(bench.cpu(2), "gpu-like", &log).with_pixel_cap(256);
    let engines = EngineSet::new(vec![boxed(capped)]);
    let cancel = AtomicBool::new(false);
    let (run, lines) = captured(|| render_with(&bench, &engines, &cancel, None, None));
    for line in &lines {
        eprintln!("log: {line}");
    }
    assert_eq!(image_bits(&run.image), expected);
    assert_eq!(log.declines(), 1);
    assert_eq!(count_lines(&lines, "declined"), 1);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_forced_small_leaf_cache_keeps_less_and_never_changes_the_image() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let bench = plain_bench(&rig, &scene, BATCH_TILES);
    let (expected, _) = reference(&bench);
    let cancel = AtomicBool::new(false);
    for cap in [0_u64, 1] {
        let mut cache = LeafCache::new(cap);
        let mut refused = 0_usize;
        let run = {
            let mut keep = |batch: RetainedBatch| {
                if !cache.retain(batch) {
                    refused += 1;
                }
            };
            let engines = EngineSet::new(vec![boxed(bench.cpu(2))]);
            render_with(&bench, &engines, &cancel, Some(&mut keep), None)
        };
        assert_eq!(image_bits(&run.image), expected, "cap {cap}");
        assert!(cache.is_empty(), "cap {cap}");
        assert_eq!(refused, BATCHES, "cap {cap}");
    }
}

// ---- the preloaded re-render ----

/// A cache holding every batch of a fresh render of `bench`, and that render.
fn kept_batches(bench: &Bench<'_>) -> (Vec<RetainedBatch>, Run) {
    let cancel = AtomicBool::new(false);
    let mut kept = Vec::new();
    let run = {
        let mut keep = |batch: RetainedBatch| kept.push(batch);
        let engines = EngineSet::new(vec![boxed(bench.cpu(2))]);
        render_with(bench, &engines, &cancel, Some(&mut keep), None)
    };
    (kept, run)
}

/// A cache of `batches`.
fn cache_of(batches: Vec<RetainedBatch>) -> LeafCache {
    let mut cache = LeafCache::new(u64::MAX);
    for batch in batches {
        assert!(cache.retain(batch));
    }
    cache
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_preloaded_render_equals_a_fresh_render_bit_for_bit_and_traces_nothing() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let bench = plain_bench(&rig, &scene, BATCH_TILES);
    let (kept, fresh) = kept_batches(&bench);
    assert_eq!(kept.len(), BATCHES);
    let cache = cache_of(kept);
    let cancel = AtomicBool::new(false);
    let mut retained_again = 0_usize;
    let mut keep = |_: RetainedBatch| retained_again += 1;
    let engines = EngineSet::new(vec![boxed(bench.cpu(2))]);
    let loaded = render_with(&bench, &engines, &cancel, Some(&mut keep), Some(&cache));
    assert_eq!(image_bits(&loaded.image), image_bits(&fresh.image));
    // No tree was traced and nothing the cache already holds is kept a second time.
    assert_eq!(loaded.stats.beams, 0);
    assert_eq!(loaded.stats.primary_beams, 0);
    assert_eq!(
        loaded.stats.leaves,
        u64::try_from(cache.leaf_count()).unwrap_or(u64::MAX)
    );
    eprintln!(
        "fresh leaves {}, preloaded leaves {}",
        fresh.stats.leaves, loaded.stats.leaves
    );
    assert_eq!(retained_again, 0);
    assert_progress_contract(&loaded.progress, BATCHES, "a preloaded render");
    assert_eq!(loaded.batches.len(), fresh.batches.len());
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_cache_with_missing_batches_is_completed_by_tracing_them() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let bench = plain_bench(&rig, &scene, BATCH_TILES);
    let (mut kept, fresh) = kept_batches(&bench);
    // Keep only the first two batches: the other two must be traced.
    kept.truncate(2);
    let cache = cache_of(kept);
    let cancel = AtomicBool::new(false);
    let mut traced_tiles = 0_usize;
    let mut keep = |batch: RetainedBatch| traced_tiles += batch.tiles.len();
    let engines = EngineSet::new(vec![boxed(bench.cpu(2))]);
    let run = render_with(&bench, &engines, &cancel, Some(&mut keep), Some(&cache));
    assert_eq!(image_bits(&run.image), image_bits(&fresh.image));
    assert_eq!(traced_tiles, 2 * BATCH_TILES as usize);
    assert!(run.stats.beams > 0, "the missing batches were traced");
    assert_progress_contract(&run.progress, BATCHES, "a half preloaded render");
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_cache_cut_into_other_batches_still_serves_every_tile() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    // The cache comes from batches of two tiles, the render cuts batches of four.
    let (kept, _) = kept_batches(&plain_bench(&rig, &scene, 2));
    let cache = cache_of(kept);
    let bench = plain_bench(&rig, &scene, BATCH_TILES);
    let (expected, _) = reference(&bench);
    let cancel = AtomicBool::new(false);
    let engines = EngineSet::new(vec![boxed(bench.cpu(3))]);
    let run = render_with(&bench, &engines, &cancel, None, Some(&cache));
    assert_eq!(image_bits(&run.image), expected);
    assert_eq!(run.stats.beams, 0);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_cache_of_another_tile_edge_loads_nothing() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let bench = plain_bench(&rig, &scene, BATCH_TILES);
    let (expected, _) = reference(&bench);
    // Batches cut on 16-pixel tiles do not name the 8-pixel tiles of the render.
    let other = Bench::new(
        &rig,
        &scene,
        beam_settings(BATCH_TILES, SPP),
        fixed_settings(SPP, PASS_SPP, 16),
    );
    let (kept, _) = kept_batches(&other);
    let cache = cache_of(kept);
    let cancel = AtomicBool::new(false);
    let engines = EngineSet::new(vec![boxed(bench.cpu(2))]);
    let run = render_with(&bench, &engines, &cancel, None, Some(&cache));
    assert_eq!(image_bits(&run.image), expected);
    assert!(run.stats.beams > 0, "every batch was traced");
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_cancelled_preloaded_render_returns_promptly_and_finishes_nothing() {
    let rig = BeamRig::brilliant(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let bench = plain_bench(&rig, &scene, BATCH_TILES);
    let (kept, _) = kept_batches(&bench);
    let cache = cache_of(kept);
    let cancel = AtomicBool::new(true);
    let engines = EngineSet::new(vec![boxed(bench.cpu(2))]);
    let run = render_with(&bench, &engines, &cancel, None, Some(&cache));
    assert_eq!(run.batches.len(), 0);
    assert!(run.progress.iter().all(|report| report.done < 1.0));
    assert_eq!(assert_written_at_most_once(&run, SIZE), 0);
}
