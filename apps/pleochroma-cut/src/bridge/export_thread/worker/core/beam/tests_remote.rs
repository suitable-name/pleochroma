//! Tests of the remote half of a beam render: the split between this computer and a worker, the
//! merge in tile order, the failover of a lost worker and the lane's opening rules.
//!
//! The worker is a scripted stand-in that renders its tiles on this process's CPU with the same
//! entry the local share uses, so every comparison is exact and needs no socket; the wire itself
//! is tested in `remote::beam_tiles` and in the worker crate. All CPU only.

use super::{
    super::{
        super::types::{AccumulationCarry, AccumulationOutcome},
        adaptive::assignment::TileAssignment,
        lanes::RenderJob,
        render_accumulation, with_export_ctx,
    },
    remote::{
        BeamLane, RemoteBeamEngine, Share, Tally, copy_tiles, empty_image, next_work, scatter_tiles,
    },
    *,
};
use crate::{
    bridge::{
        export_thread::{
            params::ComputeTarget,
            remote::{BeamTileFailure, BeamTileOrder, BeamTiles, scene_state_from_snapshot},
            scene_snapshot::SceneSnapshot,
        },
        render_thread::RenderContext,
    },
    settings::LocalComputeTarget,
};
use pleochroma::renderer::{adaptive::NoiseMetric, gpu_backend::GpuBackend};
use pleochroma_net::messages::{BeamTilePixel, BeamTileStats};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// The frame edge of the small renders, in pixels (nine 8-pixel tiles).
const SIZE: u32 = 24;
/// The samples per pixel: one pass, so no tile stops early and the image does not depend on how
/// the tiles are cut into batches.
const CAP: u32 = 8;
/// The share the bar shows once this computer's five of the nine tiles are done and no worker tile
/// is in yet (tile i goes to engine i % 2): `Tally::mixed(1.0, 0)`.
const LOCAL_SHARE: f32 = 5.0 / 9.0;
/// The most a scripted worker holds its answer for its gate; then it answers anyway and the
/// timing assertion fails with the timeline.
const HOLD_BOUND: Duration = Duration::from_secs(600);

fn capture() -> SceneSnapshot {
    SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("Diamond resolves")
}

/// A coarse tree, as in `tests.rs`: these rows check the split, the merge order, the failover and
/// the notes, which hold at any tree resolution. The defaults make one 24-pixel render take over
/// ten minutes.
fn beam_settings() -> BeamSettings {
    BeamSettings {
        dir_tolerance_rad: 1e-2,
        chord_tolerance_px: 1e-2,
        weight_tolerance: 1e-2,
        lighting_tolerance: 1e-2,
        cutoff: 1e-2,
        spectral_nodes: 8,
        absorption_subnodes: 1,
        max_split_depth: 8,
        max_fragment_levels: 3,
        residual_min_spp: CAP,
        batch_tiles: 4,
        ..BeamSettings::default()
    }
}

/// One 8-sample pass over 8-pixel tiles, capped at `CAP`.
fn fixed() -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise: 0.5,
        metric: NoiseMetric::RelativeLstar,
        min_spp: CAP,
        max_spp: CAP,
        pass_spp: CAP,
        tile: 8,
    }
}

fn taps() -> RenderTaps {
    RenderTaps {
        adaptive: Some(fixed()),
        beam: Some(beam_settings()),
        ..RenderTaps::default()
    }
}

/// The tiles a scripted worker was asked for, one list per call.
type CallLog = Arc<Mutex<Vec<Vec<u32>>>>;

/// Renders the tiles `live` as a worker would, on this process's CPU.
fn worker_tiles(ctx: &ExportCtx<'_>, order: &BeamTileOrder<'_>, live: &LiveTiles) -> BeamTiles {
    let frame = frame_of(ctx);
    let extras: Extras<'_> = (&ctx.scene.tools, &ctx.scene.fluorescence, ctx.surface);
    let soa = build_plane_soa(&ctx.scene.active_planes);
    let prepared = PreparedMaterial::new(frame.material);
    let residual = ResidualScene {
        frame: &frame,
        extras,
        plane_soa: &soa,
        prepared: &prepared,
        n_lambda: node_count(&order.beam),
        sample_offset: order.sample_offset,
        residual_min_spp: order.beam.residual_min_spp,
    };
    let (list, _) = local_engines(
        LocalComputeTarget::Cpu,
        ctx.gpu,
        &residual,
        (&order.beam, &order.adaptive),
        false,
    );
    let engines = EngineSet::new(list);
    let inputs = Inputs {
        frame: &frame,
        extras,
        plane_soa: &soa,
        settings: order.beam,
        adaptive: order.adaptive,
        engines: &engines,
        host_convergence: false,
    };
    let never = AtomicBool::new(false);
    let (image, stats) = render_local(
        &inputs,
        (&never, order.sample_offset),
        Some(live),
        LocalHooks {
            progress: &mut |_| {},
            on_batch: &mut |_, _| {},
            retain: None,
        },
    )
    .expect("the worker renders its tiles");
    let mut pixels = Vec::new();
    for &index in live.indices() {
        let (x0, y0, x1, y1) = live.tile_rect(index);
        for y in y0..y1 {
            for x in x0..x1 {
                let at = (y * SIZE + x) as usize;
                pixels.push(BeamTilePixel {
                    xyz: image.pixels[at],
                    samples: image.counts[at],
                });
            }
        }
    }
    BeamTiles {
        pixels,
        stats: BeamTileStats {
            beams: stats.beams,
            leaves: stats.leaves,
            fragments: stats.fragments,
            drops: 0,
            residual_samples: image.samples_traced,
        },
    }
}

/// A worker that answers like `pleochroma-worker` on the CPU, and from its `fail_from_call`-th
/// call on (1-based) breaks.
struct ScriptedWorker<'a> {
    scene: &'a SceneSnapshot,
    calls: CallLog,
    fail_from_call: Option<usize>,
    /// The worker holds its finished tiles until this is raised (`None`: it answers at once).
    hold_until: Option<Arc<AtomicBool>>,
    /// How long the worker takes to answer after the hold.
    delay: Duration,
    /// When the worker last answered.
    returned: Arc<Mutex<Option<Instant>>>,
}

impl RemoteBeamEngine for ScriptedWorker<'_> {
    fn render_tiles(
        &mut self,
        order: &BeamTileOrder<'_>,
        live: &LiveTiles,
        cancel: &AtomicBool,
    ) -> Result<BeamTiles, BeamTileFailure> {
        let call = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(live.indices().to_vec());
            calls.len()
        };
        if self.fail_from_call.is_some_and(|from| call >= from) {
            return Err(BeamTileFailure::Broken(
                "the scripted worker went away".to_owned(),
            ));
        }
        let tiles = with_export_ctx(
            self.scene,
            0.3,
            0.2,
            SIZE,
            SIZE,
            &GpuBackend::disabled(),
            None,
            cancel,
            |ctx| worker_tiles(ctx.expect("the context builds"), order, live),
        );
        if let Some(gate) = &self.hold_until {
            let deadline = Instant::now() + HOLD_BOUND;
            while !gate.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        std::thread::sleep(self.delay);
        *self.returned.lock().unwrap() = Some(Instant::now());
        Ok(tiles)
    }
}

/// A lane with one scripted worker.
fn lane<'a>(
    scene: &'a SceneSnapshot,
    calls: &CallLog,
    fail_from_call: Option<usize>,
    remote_only: bool,
) -> BeamLane<'a> {
    slow_lane(
        scene,
        calls,
        (fail_from_call, remote_only),
        (None, Duration::ZERO, &Arc::new(Mutex::new(None))),
    )
}

/// When a scripted worker answers: the flag it holds its answer for (if any), the delay after
/// that, and where it notes the instant it answered.
type AnswerTiming<'h> = (
    Option<&'h Arc<AtomicBool>>,
    Duration,
    &'h Arc<Mutex<Option<Instant>>>,
);

/// A lane with one scripted worker that holds its answer for `hold_until`, then answers after
/// `delay` and notes when it did.
fn slow_lane<'a>(
    scene: &'a SceneSnapshot,
    calls: &CallLog,
    (fail_from_call, remote_only): (Option<usize>, bool),
    (hold_until, delay, returned): AnswerTiming<'_>,
) -> BeamLane<'a> {
    BeamLane {
        engines: vec![Box::new(ScriptedWorker {
            scene,
            calls: Arc::clone(calls),
            fail_from_call,
            hold_until: hold_until.map(Arc::clone),
            delay,
            returned: Arc::clone(returned),
        })],
        scene: Some(scene_state_from_snapshot(scene, SIZE, SIZE, 0.3, 0.2)),
        note: None,
        not_local: false,
        remote_only,
    }
}

/// A beam render of `scene` through `lane` (`None`: the job's own, which is local).
fn render_through(
    scene: &SceneSnapshot,
    lane: Option<BeamLane<'_>>,
    compute_target: ComputeTarget,
) -> (BeamAttempt, Vec<ExportProgress>) {
    let (attempt, reports) = render_timed(scene, lane, compute_target, None);
    (
        attempt,
        reports.into_iter().map(|(_, report)| report).collect(),
    )
}

/// [`render_through`], with the time of each report; `local_done` is raised once the bar shows
/// this computer's share as done.
fn render_timed(
    scene: &SceneSnapshot,
    lane: Option<BeamLane<'_>>,
    compute_target: ComputeTarget,
    local_done: Option<&AtomicBool>,
) -> (BeamAttempt, Vec<(Instant, ExportProgress)>) {
    let cancel = AtomicBool::new(false);
    let taps = taps();
    let mut reports = Vec::new();
    let attempt = with_export_ctx(
        scene,
        0.3,
        0.2,
        SIZE,
        SIZE,
        &GpuBackend::disabled(),
        None,
        &cancel,
        |ctx| {
            let job = RenderJob {
                scene,
                cam_yaw: 0.3,
                cam_pitch: 0.2,
                width: SIZE,
                height: SIZE,
                samples_per_pixel: CAP,
                sample_offset: 0,
                compute_target,
                worker: None,
                cancel: &cancel,
            };
            render_beams_with(
                ctx.expect("the context builds"),
                &job,
                LocalComputeTarget::Cpu,
                &taps,
                lane,
                |progress| {
                    if let Some(gate) = local_done
                        && progress.fraction >= LOCAL_SHARE
                    {
                        gate.store(true, Ordering::Release);
                    }
                    reports.push((Instant::now(), progress));
                },
            )
        },
    );
    (attempt, reports)
}

fn done(attempt: BeamAttempt) -> Accumulation {
    match attempt {
        BeamAttempt::Done(done) => done,
        BeamAttempt::Cancelled => panic!("the render was cancelled"),
        BeamAttempt::Failed(message) => panic!("the render failed: {message}"),
        BeamAttempt::Off => panic!("the stone was not rendered from beams"),
    }
}

fn new_log() -> CallLog {
    Arc::new(Mutex::new(Vec::new()))
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 04b runs it"]
fn a_frame_split_with_a_worker_equals_the_local_render_bit_for_bit() {
    let scene = capture();
    let local = done(render_through(&scene, None, ComputeTarget::LocalOnly).0);

    let calls = new_log();
    // The worker holds its finished tiles until this computer's share is done, then answers 300 ms
    // later: the bar must keep moving in between, whichever share is the slower one.
    let local_done = Arc::new(AtomicBool::new(false));
    let returned = Arc::new(Mutex::new(None));
    let (attempt, timed) = render_timed(
        &scene,
        Some(slow_lane(
            &scene,
            &calls,
            (None, false),
            (Some(&local_done), Duration::from_millis(300), &returned),
        )),
        ComputeTarget::Both,
        Some(&local_done),
    );
    let reports: Vec<&ExportProgress> = timed.iter().map(|(_, report)| report).collect();
    let answered = returned.lock().unwrap().expect("the worker answered");
    assert!(
        timed.iter().any(|(at, report)| {
            report.fraction < 1.0
                && *at < answered
                && answered.saturating_duration_since(*at) < Duration::from_millis(250)
        }),
        "the progress keeps moving while the worker's share runs: {:?}",
        timed
            .iter()
            .map(|(at, report)| (answered.saturating_duration_since(*at), report.fraction))
            .collect::<Vec<_>>()
    );
    let split = done(attempt);
    assert_eq!(split.accum, local.accum, "the same image, bit for bit");
    assert_eq!(
        split.adaptive.as_ref().map(|s| &s.counts),
        local.adaptive.as_ref().map(|s| &s.counts),
        "the same residual counts"
    );
    let calls = calls.lock().unwrap();
    assert_eq!(
        *calls,
        vec![vec![1, 3, 5, 7]],
        "tile i goes to engine i % 2"
    );
    let last = reports.last().and_then(|r| r.note.as_deref());
    eprintln!("split render closing note: {last:?}");
    assert!(
        last.is_some_and(|note| note.starts_with("Beams: ")),
        "{last:?}"
    );
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 04b runs it"]
fn a_remote_only_render_sends_every_tile_to_the_worker() {
    let scene = capture();
    let local = done(render_through(&scene, None, ComputeTarget::LocalOnly).0);
    let calls = new_log();
    let (attempt, reports) = render_through(
        &scene,
        Some(lane(&scene, &calls, None, true)),
        ComputeTarget::RemoteOnly,
    );
    assert_eq!(done(attempt).accum, local.accum);
    assert_eq!(*calls.lock().unwrap(), vec![(0..9).collect::<Vec<u32>>()]);
    let mut previous = 0.0;
    for report in &reports {
        assert!(report.fraction >= previous, "the share never goes back");
        previous = report.fraction;
    }
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 04b"]
fn a_worker_that_drops_hands_its_tiles_to_this_computer_with_the_same_image() {
    let scene = capture();
    let local = done(render_through(&scene, None, ComputeTarget::LocalOnly).0);
    let calls = new_log();
    let (attempt, reports) = render_through(
        &scene,
        Some(lane(&scene, &calls, Some(1), false)),
        ComputeTarget::Both,
    );
    assert_eq!(
        done(attempt).accum,
        local.accum,
        "the failover is invisible"
    );
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "the worker got no second chance"
    );
    let last = reports.last().and_then(|r| r.note.as_deref()).unwrap_or("");
    assert!(last.contains("The remote worker stopped"), "{last}");
    assert!(last.contains("this computer renders its tiles"), "{last}");
}

#[test]
fn two_workers_failing_in_one_round_queue_their_tiles_for_the_survivor_only() {
    // Three engines over nine tiles; slot 1 fails first (its heir is slot 2), then slot 2 does.
    let mut assignment = TileAssignment::new(9, 3);
    let all: Vec<u32> = (0..9).collect();
    let shares: Vec<Share> = assignment
        .split(&all)
        .into_iter()
        .enumerate()
        .map(|(slot, tiles)| Share {
            slot,
            live: LiveTiles::from_indices(SIZE, SIZE, 8, tiles),
        })
        .collect();
    assert_eq!(assignment.fail(1), Some(2));
    assert_eq!(assignment.fail(2), Some(0));
    let next = next_work(&assignment, &[&shares[1], &shares[2]]);
    assert_eq!(next, vec![(0, vec![1, 2, 4, 5, 7, 8])]);
}

#[test]
fn a_remote_only_worker_that_drops_fails_the_render() {
    let scene = capture();
    let calls = new_log();
    let (attempt, _) = render_through(
        &scene,
        Some(lane(&scene, &calls, Some(1), true)),
        ComputeTarget::RemoteOnly,
    );
    let BeamAttempt::Failed(message) = attempt else {
        panic!("nobody is left to take the tiles");
    };
    assert!(message.contains("The remote worker failed"), "{message}");
}

#[test]
fn a_render_without_a_configured_worker_stays_here_and_says_so() {
    let scene = capture();
    let cancel = AtomicBool::new(false);
    let job = |compute_target| RenderJob {
        scene: &scene,
        cam_yaw: 0.3,
        cam_pitch: 0.2,
        width: SIZE,
        height: SIZE,
        samples_per_pixel: CAP,
        sample_offset: 0,
        compute_target,
        worker: None,
        cancel: &cancel,
    };
    let plain = remote::open(&job(ComputeTarget::LocalOnly)).expect("opens");
    assert!(plain.engines.is_empty() && !plain.not_local && plain.note.is_none());
    for target in [ComputeTarget::Both, ComputeTarget::RemoteOnly] {
        let lane = remote::open(&job(target)).expect("opens");
        assert!(lane.engines.is_empty() && lane.not_local, "{target:?}");
    }
    let kept = BeamLane::kept(ComputeTarget::Both);
    assert_eq!(kept.note.as_deref(), Some(remote::NOTE_KEEP_LOCAL));
    assert_eq!(BeamLane::kept(ComputeTarget::LocalOnly).note, None);
}

#[test]
fn the_lane_with_a_remote_only_worker_leaves_this_computer_out() {
    let scene = capture();
    let calls = new_log();
    assert!(lane(&scene, &calls, None, false).local_participates());
    assert!(!lane(&scene, &calls, None, true).local_participates());
    assert!(BeamLane::local_only().local_participates());
}

#[test]
fn a_workers_pixels_land_on_their_tiles_and_a_wrong_count_is_refused() {
    let mut image = empty_image(16, 8);
    let live = LiveTiles::from_indices(16, 8, 8, vec![1]);
    let pixels: Vec<BeamTilePixel> = (0..64_u16)
        .map(|i| BeamTilePixel {
            xyz: Vec3::splat(f32::from(i) + 1.0),
            samples: 7,
        })
        .collect();
    assert!(scatter_tiles(&mut image, &live, &pixels));
    assert_eq!(
        image.pixels[8],
        Vec3::splat(1.0),
        "row 0 of tile 1 starts at x 8"
    );
    assert_eq!(image.pixels[16 + 8], Vec3::splat(9.0));
    assert_eq!(image.pixels[0], Vec3::ZERO, "tile 0 is untouched");
    assert_eq!(image.counts.iter().filter(|&&c| c == 7).count(), 64);
    assert!(!scatter_tiles(&mut image, &live, &pixels[..63]));
}

#[test]
fn the_tiles_of_a_local_share_are_copied_and_the_others_left_alone() {
    let mut source = empty_image(16, 8);
    source.pixels.fill(Vec3::ONE);
    source.counts.fill(3);
    let mut target = empty_image(16, 8);
    copy_tiles(
        &mut target,
        &source,
        &LiveTiles::from_indices(16, 8, 8, vec![0]),
    );
    assert_eq!(target.pixels[0], Vec3::ONE);
    assert_eq!(target.pixels[8], Vec3::ZERO);
    assert_eq!(target.counts.iter().filter(|&&c| c == 3).count(), 64);
}

#[test]
fn the_share_done_mixes_the_local_fraction_with_the_settled_and_remote_tiles() {
    let tally = Tally {
        total: 10,
        settled: std::cell::Cell::new(2),
        local: std::cell::Cell::new(4),
    };
    assert!((tally.mixed(0.0, 0) - 0.2).abs() < 1e-6);
    assert!((tally.mixed(0.5, 2) - 0.6).abs() < 1e-6);
    assert!((tally.mixed(9.0, 9) - 1.0).abs() < f32::EPSILON, "clamped");
    let empty = Tally {
        total: 0,
        settled: std::cell::Cell::new(0),
        local: std::cell::Cell::new(0),
    };
    assert!((empty.mixed(0.0, 0) - 1.0).abs() < f32::EPSILON);
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 04b runs it"]
fn a_carry_with_these_taps_still_renders_without_a_lane() {
    // The ordinary entry (no injected lane) is the local render: the same bits as the seam's.
    let scene = capture();
    let cancel = AtomicBool::new(false);
    let outcome = render_accumulation(
        &scene,
        0.3,
        0.2,
        crate::bridge::export_thread::params::ExportParams {
            engine: scene.engine,
            width: SIZE,
            height: SIZE,
            samples_per_pixel: CAP,
            max_bounces: scene.max_bounces,
        },
        ComputeTarget::LocalOnly,
        None,
        &GpuBackend::disabled(),
        LocalComputeTarget::Cpu,
        &mut AccumulationCarry::with_taps(taps()),
        &cancel,
        |_| {},
    );
    let AccumulationOutcome::Completed(entry) = outcome else {
        panic!("the render completes");
    };
    let seam = done(render_through(&scene, None, ComputeTarget::LocalOnly).0);
    assert_eq!(entry.accum, seam.accum);
}
