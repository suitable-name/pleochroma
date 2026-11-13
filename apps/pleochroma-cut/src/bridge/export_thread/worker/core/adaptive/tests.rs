//! Tests of the adaptive export: the threaded CPU pass against the reference pass, the render
//! loop's determinism, its image against the reference driver's, cancellation, the refusals, the
//! raw-image tap and the path into the tone map. All CPU only (no adapter needed).

use super::{
    super::{
        super::{
            final_picture::{RenderedImage, render_image_rgba},
            taps::{LinearImage, RawTapSink},
            types::AccumulationCarry,
        },
        render_accumulation, with_export_ctx,
    },
    cpu_pass::trace_live_pass,
    *,
};
use crate::{
    bridge::{
        export_thread::{
            params::{ComputeTarget, ExportParams, RemoteSelection},
            scene_snapshot::SceneSnapshot,
        },
        render_thread::RenderContext,
    },
    settings::LocalComputeTarget,
};
use pleochroma::{
    color::ColorSpace,
    optics::raytracer::build_plane_soa,
    renderer::{
        adaptive::{LiveTiles, NoiseMetric, Pass},
        cpu_frame::{render_adaptive_surface, trace_live_tiles_surface},
        engine::{EngineSpec, RayOptions, RenderEngine},
        frame_scene::FrameScene,
        gpu_backend::GpuBackend,
        tonemap::tonemap_accumulation,
    },
};
use std::sync::{Mutex, atomic::AtomicBool};

fn capture() -> SceneSnapshot {
    SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("Diamond resolves")
}

/// Eight-pixel tiles and eight-sample passes, so a small frame has many tiles and a few checks.
fn settings(max_spp: u32, target_noise: f32) -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise,
        metric: NoiseMetric::RelativeLstar,
        min_spp: 16,
        max_spp,
        pass_spp: 8,
        tile: 8,
    }
}

fn taps_with(settings: AdaptiveSettings) -> RenderTaps {
    RenderTaps {
        adaptive: Some(settings),
        ..RenderTaps::default()
    }
}

fn params(scene: &SceneSnapshot, size: u32, spp: u32) -> ExportParams {
    ExportParams {
        engine: scene.engine,
        width: size,
        height: size,
        samples_per_pixel: spp,
        max_bounces: scene.max_bounces,
    }
}

/// An adaptive CPU render of `scene` at `(0.3, 0.2)`.
fn render(
    scene: &SceneSnapshot,
    size: u32,
    spp: u32,
    taps: RenderTaps,
    compute_target: ComputeTarget,
    cancel: &AtomicBool,
    report_progress: impl FnMut(ExportProgress),
) -> AccumulationOutcome {
    render_accumulation(
        scene,
        0.3,
        0.2,
        params(scene, size, spp),
        compute_target,
        None,
        &GpuBackend::disabled(),
        LocalComputeTarget::Cpu,
        &mut AccumulationCarry::with_taps(taps),
        cancel,
        report_progress,
    )
}

fn completed(outcome: AccumulationOutcome) -> Accumulation {
    match outcome {
        AccumulationOutcome::Completed(done) => done,
        AccumulationOutcome::Cancelled => panic!("the render was cancelled"),
        AccumulationOutcome::Failed(message) => panic!("the render failed: {message}"),
    }
}

fn failure(outcome: AccumulationOutcome) -> String {
    match outcome {
        AccumulationOutcome::Failed(message) => message,
        AccumulationOutcome::Completed(_) => panic!("the render should have been refused"),
        AccumulationOutcome::Cancelled => panic!("the render was cancelled"),
    }
}

#[test]
fn the_threaded_cpu_pass_equals_the_reference_live_pass_bit_for_bit() {
    let scene = capture();
    let (width, height) = (24_u32, 24_u32);
    let cancel = AtomicBool::new(false);
    with_export_ctx(
        &scene,
        0.3,
        0.2,
        width,
        height,
        &GpuBackend::disabled(),
        None,
        &cancel,
        |ctx| {
            let ctx = ctx.expect("the context builds");
            // The middle tile, the first and the last: an odd first sample, so the half-buffer
            // takes the first sample of the pass.
            let live = LiveTiles::from_indices(width, height, 8, vec![0, 4, 8]);
            let pass = Pass {
                first_sample: 5,
                spp: 6,
                live: live.clone(),
            };
            let pixels = (width * height) as usize;
            let (mut sum, mut half_b) = (vec![Vec3::ZERO; pixels], vec![Vec3::ZERO; pixels]);
            assert!(!trace_live_pass(ctx, &pass, &mut sum, &mut half_b, &cancel));

            let frame = FrameScene {
                camera: ctx.camera,
                width,
                height,
                planes: &scene.active_planes,
                facet_finishes: &scene.facet_finishes,
                material: ctx.gpu_scene.material,
                max_bounces: scene.max_bounces,
                environment: ctx.gpu_scene.environment,
            };
            let plane_soa = build_plane_soa(&scene.active_planes);
            let (mut want_sum, mut want_half) =
                (vec![Vec3::ZERO; pixels], vec![Vec3::ZERO; pixels]);
            trace_live_tiles_surface(
                &frame,
                (&scene.tools, &scene.fluorescence, ctx.surface),
                &plane_soa,
                &live,
                5,
                6,
                &mut want_sum,
                &mut want_half,
            );
            assert_eq!(
                sum, want_sum,
                "the sums are the reference pass's, bit for bit"
            );
            assert_eq!(half_b, want_half, "so is the odd-sample half-buffer");
            assert!(sum.iter().any(|v| *v != Vec3::ZERO), "something was traced");
            // Tile 1 (x 8..16, y 0..8) is not live: its pixels are untouched.
            assert_eq!(sum[8], Vec3::ZERO);
            assert_eq!(half_b[8], Vec3::ZERO);

            // A second pass adds to the first, as one pass of twice the length would.
            let again = Pass {
                first_sample: 11,
                spp: 6,
                live: live.clone(),
            };
            assert!(!trace_live_pass(
                ctx,
                &again,
                &mut sum,
                &mut half_b,
                &cancel
            ));
            trace_live_tiles_surface(
                &frame,
                (&scene.tools, &scene.fluorescence, ctx.surface),
                &plane_soa,
                &live,
                11,
                6,
                &mut want_sum,
                &mut want_half,
            );
            assert_eq!(sum, want_sum);
            assert_eq!(half_b, want_half);
        },
    );
}

#[test]
fn a_cancelled_cpu_pass_leaves_the_buffers_alone() {
    let scene = capture();
    let cancel = AtomicBool::new(true);
    with_export_ctx(
        &scene,
        0.3,
        0.2,
        16,
        16,
        &GpuBackend::disabled(),
        None,
        &cancel,
        |ctx| {
            let ctx = ctx.expect("the context builds");
            let pass = Pass {
                first_sample: 0,
                spp: 4,
                live: LiveTiles::all(16, 16, 8),
            };
            let (mut sum, mut half_b) = (vec![Vec3::ZERO; 256], vec![Vec3::ZERO; 256]);
            assert!(trace_live_pass(ctx, &pass, &mut sum, &mut half_b, &cancel));
            assert!(sum.iter().chain(&half_b).all(|v| *v == Vec3::ZERO));
        },
    );
}

#[test]
fn an_adaptive_export_is_deterministic_and_equals_the_reference_driver() {
    let scene = capture();
    let (size, cap) = (40_u32, 64_u32);
    let settings = settings(cap, 0.5);
    let cancel = AtomicBool::new(false);
    let run = || {
        completed(render(
            &scene,
            size,
            cap,
            taps_with(settings),
            ComputeTarget::LocalOnly,
            &cancel,
            |_| {},
        ))
    };
    let first = run();
    let second = run();
    assert_eq!(first.accum, second.accum, "two runs are byte-identical");
    assert_eq!(first.adaptive, second.adaptive);
    assert_eq!(first.samples_per_pixel, 1, "the buffer holds means");

    let stats = first.adaptive.as_ref().expect("an adaptive render says so");
    assert_eq!(stats.counts.len(), (size * size) as usize);
    assert!(
        stats
            .counts
            .iter()
            .all(|&count| (16..=cap).contains(&count)),
        "no pixel stops before the minimum or passes the cap"
    );
    assert!(
        stats.counts.iter().any(|&count| count < cap),
        "some tiles stop before the cap"
    );
    assert_eq!(
        stats.samples_traced,
        stats
            .counts
            .iter()
            .map(|&count| u64::from(count))
            .sum::<u64>()
    );
    assert_eq!(stats.max_spp, cap);
    assert_eq!(stats.tiles_total, 25);
    assert!(stats.tiles_converged >= 1 && stats.tiles_converged <= 25);

    // The resolved image is the reference driver's: every pixel is its own sum over its own
    // count, and the decisions between the passes agree.
    let cancel_ref = AtomicBool::new(false);
    with_export_ctx(
        &scene,
        0.3,
        0.2,
        size,
        size,
        &GpuBackend::disabled(),
        None,
        &cancel_ref,
        |ctx| {
            let ctx = ctx.expect("the context builds");
            let frame = FrameScene {
                camera: ctx.camera,
                width: size,
                height: size,
                planes: &scene.active_planes,
                facet_finishes: &scene.facet_finishes,
                material: ctx.gpu_scene.material,
                max_bounces: scene.max_bounces,
                environment: ctx.gpu_scene.environment,
            };
            let plane_soa = build_plane_soa(&scene.active_planes);
            let reference = render_adaptive_surface(
                &frame,
                (&scene.tools, &scene.fluorescence, ctx.surface),
                &plane_soa,
                &settings,
                0,
                1.0,
            )
            .expect("valid settings");
            assert_eq!(reference.counts, stats.counts);
            assert_eq!(reference.pixels, first.accum);
        },
    );
}

#[test]
fn progress_counts_samples_per_pixel_over_the_cap_and_the_last_report_carries_the_log_line() {
    let scene = capture();
    let cancel = AtomicBool::new(false);
    let mut reports = Vec::new();
    let done = completed(render(
        &scene,
        24,
        64,
        taps_with(settings(64, 0.5)),
        ComputeTarget::LocalOnly,
        &cancel,
        |progress| reports.push(progress),
    ));
    assert!(
        reports.len() >= 2,
        "at least the passes and the closing report"
    );
    let mut previous = 0.0;
    for report in &reports {
        assert!(report.fraction >= previous, "progress never goes back");
        assert!(report.fraction <= 1.0);
        assert_eq!(report.samples_total, 64);
        previous = report.fraction;
    }
    let last = reports.last().expect("reports");
    assert!((last.fraction - 1.0).abs() < f32::EPSILON);
    let note = last
        .note
        .as_deref()
        .expect("the closing report has the note");
    assert!(note.starts_with("adaptive sampling: "), "{note}");
    assert!(note.contains("samples traced"), "{note}");
    assert!(note.contains("tiles converged"), "{note}");
    assert!(note.contains("24 x 24 x 64"), "{note}");
    let stats = done.adaptive.expect("stats");
    assert_eq!(last.samples_done, stats.samples_done);
    assert!(
        reports[..reports.len() - 1]
            .iter()
            .all(|r| r.note.is_none()),
        "the note comes once, at the end"
    );
}

#[test]
fn a_cancelled_adaptive_render_ends_cancelled() {
    let scene = capture();
    let cancel = AtomicBool::new(true);
    let outcome = render(
        &scene,
        16,
        32,
        taps_with(settings(32, 0.5)),
        ComputeTarget::LocalOnly,
        &cancel,
        |_| {},
    );
    assert!(matches!(outcome, AccumulationOutcome::Cancelled));
}

#[test]
fn what_an_adaptive_render_cannot_do_is_refused_with_a_message() {
    let cancel = AtomicBool::new(false);
    let scene = capture();

    let message = failure(render(
        &scene,
        16,
        32,
        taps_with(settings(32, 0.5)),
        ComputeTarget::RemoteOnly,
        &cancel,
        |_| {},
    ));
    assert!(message.contains("this computer only"), "{message}");

    // Settings that break a rule are an error, not a panic: 20 is not a whole number of passes.
    let broken = AdaptiveSettings {
        min_spp: 20,
        ..settings(32, 0.5)
    };
    let message = failure(render(
        &scene,
        16,
        32,
        taps_with(broken),
        ComputeTarget::LocalOnly,
        &cancel,
        |_| {},
    ));
    assert!(message.starts_with("Adaptive sampling: "), "{message}");
    assert!(message.contains("multiple of the pass size"), "{message}");

    let mut ray = capture();
    ray.engine = EngineSpec {
        engine: RenderEngine::RayTraced,
        ray: RayOptions {
            aa_taps: 4,
            fire: true,
        },
    };
    let message = failure(render(
        &ray,
        16,
        4,
        taps_with(settings(4, 0.5)),
        ComputeTarget::LocalOnly,
        &cancel,
        |_| {},
    ));
    assert!(message.contains("path-traced"), "{message}");
}

#[test]
fn compute_both_without_a_worker_renders_locally() {
    let scene = capture();
    let cancel = AtomicBool::new(false);
    let done = completed(render(
        &scene,
        16,
        16,
        taps_with(AdaptiveSettings {
            max_spp: 16,
            ..settings(16, 0.5)
        }),
        ComputeTarget::Both,
        &cancel,
        |_| {},
    ));
    assert!(done.adaptive.is_some());
}

/// One published image: kind, samples, counts and means.
type SeenImage = (LinearImageKind, u32, Vec<u32>, Vec<[f32; 3]>);

/// Records the images a render publishes: kind, samples, counts and means.
#[derive(Debug, Default)]
struct Recorder {
    seen: Mutex<Vec<SeenImage>>,
}

impl RawTapSink for Recorder {
    fn linear_image(&self, image: &LinearImage<'_>) {
        self.seen.lock().unwrap().push((
            image.kind,
            image.samples_per_pixel,
            image.pixel_counts(),
            image.mean_xyz(),
        ));
    }
}

#[test]
fn snapshots_follow_the_passes_and_the_final_image_carries_each_pixels_own_count() {
    let scene = capture();
    let cancel = AtomicBool::new(false);
    let recorder = std::sync::Arc::new(Recorder::default());
    // A target no stone tile meets, so the live tiles run to the cap.
    let taps = RenderTaps {
        sink: Some(recorder.clone()),
        snapshots: true,
        adaptive: Some(settings(64, 1.0e-6)),
        ..RenderTaps::default()
    };
    let size = 24;
    let done = completed(render(
        &scene,
        size,
        64,
        taps,
        ComputeTarget::LocalOnly,
        &cancel,
        |_| {},
    ));
    let stats = done.adaptive.clone().expect("stats");
    assert_eq!(stats.samples_done, 64, "the stone tiles ran to the cap");

    // The render itself publishes snapshots only; the final image goes out through
    // `render_image_rgba` (see the next test), so here: snapshots at 16, 32 and 64.
    let seen = recorder.seen.lock().unwrap();
    let snapshot_samples: Vec<u32> = seen.iter().map(|(_, samples, _, _)| *samples).collect();
    assert_eq!(snapshot_samples, vec![16, 32, 64]);
    for (kind, samples, counts, means) in seen.iter() {
        assert_eq!(*kind, LinearImageKind::Snapshot);
        assert_eq!(counts.len(), (size * size) as usize);
        assert!(counts.iter().all(|count| count <= samples));
        assert!(counts.iter().all(|&count| count >= 16));
        assert_eq!(means.len(), counts.len());
    }
    // The last snapshot is the finished image: same counts, same means.
    let last = seen.last().expect("snapshots").clone();
    drop(seen);
    let (_, _, last_counts, last_means) = &last;
    assert_eq!(*last_counts, stats.counts);
    let finished: Vec<[f32; 3]> = done.accum.iter().map(glam::Vec3::to_array).collect();
    assert_eq!(*last_means, finished);
}

#[test]
fn the_picture_goes_through_the_exports_own_tone_map() {
    let scene = capture();
    let cancel = AtomicBool::new(false);
    let (size, cap) = (24_u32, 32_u32);
    let settings = settings(cap, 0.5);
    let done = completed(render(
        &scene,
        size,
        cap,
        taps_with(settings),
        ComputeTarget::LocalOnly,
        &cancel,
        |_| {},
    ));
    let recorder = std::sync::Arc::new(Recorder::default());
    let rgba = render_image_rgba(
        &scene,
        0.3,
        0.2,
        params(&scene, size, cap),
        ColorSpace::Srgb,
        &RemoteSelection::local_only(),
        &GpuBackend::disabled(),
        LocalComputeTarget::Cpu,
        &mut AccumulationCarry::with_taps(RenderTaps {
            sink: Some(recorder.clone()),
            adaptive: Some(settings),
            ..RenderTaps::default()
        }),
        &cancel,
        |_| {},
    );
    let RenderedImage::Rgba(rgba) = rgba else {
        panic!("the picture renders")
    };
    assert_eq!(
        rgba,
        tonemap_accumulation(size, size, 1, &done.accum, ColorSpace::Srgb),
        "the means go through `tonemap_accumulation` with a count of 1"
    );
    // The final image reached the sink with the real counts.
    let stats = done.adaptive.expect("stats");
    let seen = recorder.seen.lock().unwrap();
    let (kind, samples, counts, means) = seen.last().expect("the final image is published");
    assert_eq!(*kind, LinearImageKind::Final);
    assert_eq!(*samples, stats.samples_done);
    assert_eq!(*counts, stats.counts);
    assert_eq!(means.len(), (size * size) as usize);
    drop(seen);
}

#[test]
fn adaptive_sampling_with_a_remote_lane_is_refused_before_a_picture_renders() {
    let scene = capture();
    let cancel = AtomicBool::new(false);
    let remote = RemoteSelection {
        compute_target: ComputeTarget::RemoteOnly,
        ..RemoteSelection::local_only()
    };
    let outcome = render_image_rgba(
        &scene,
        0.3,
        0.2,
        params(&scene, 16, 16),
        ColorSpace::Srgb,
        &remote,
        &GpuBackend::disabled(),
        LocalComputeTarget::Cpu,
        &mut AccumulationCarry::with_taps(taps_with(settings(16, 0.5))),
        &cancel,
        |_| {},
    );
    let RenderedImage::Failed(message) = outcome else {
        panic!("a remote lane is refused")
    };
    assert!(message.contains("this computer only"), "{message}");
}

// ---- Protocol v37: a remote worker takes part in an adaptive render. ----
//
// The worker is a scripted stand-in that traces its chunks on this process's CPU with the same
// code the local share uses, so every comparison below is exact (no GPU twin tolerance) and needs
// no socket; the wire itself is tested in `remote::live_pass` and in the worker crate.

use super::{
    remote_lane::{RemoteEngine, RemoteLane},
    split_pass::PassEngines,
};
use crate::bridge::export_thread::remote::{LivePassFailure, scene_state_from_snapshot};
use pleochroma_net::{SceneState, messages::gather_live};
use std::sync::Arc;

/// The chunks a scripted worker was asked for: first sample and tile indices, one per call.
type CallLog = Arc<Mutex<Vec<(u32, Vec<u32>)>>>;

/// A worker that answers like `pleochroma-worker` on the CPU, and from its `fail_from_call`-th
/// call on (1-based) breaks.
struct ScriptedWorker<'a> {
    scene: &'a SceneSnapshot,
    size: u32,
    calls: CallLog,
    fail_from_call: Option<usize>,
}

impl RemoteEngine for ScriptedWorker<'_> {
    fn trace(
        &mut self,
        _scene: &SceneState,
        (first_sample, spp): (u32, u32),
        live: &LiveTiles,
        cancel: &AtomicBool,
    ) -> Result<Vec<(Vec3, Vec3)>, LivePassFailure> {
        let call = {
            let mut calls = self.calls.lock().unwrap();
            calls.push((first_sample, live.indices().to_vec()));
            calls.len()
        };
        if self.fail_from_call.is_some_and(|from| call >= from) {
            return Err(LivePassFailure::Broken(
                "the scripted worker went away".to_owned(),
            ));
        }
        let size = self.size;
        let pixels = (size * size) as usize;
        Ok(with_export_ctx(
            self.scene,
            0.3,
            0.2,
            size,
            size,
            &GpuBackend::disabled(),
            None,
            cancel,
            |ctx| {
                let ctx = ctx.expect("the context builds");
                let pass = Pass {
                    first_sample,
                    spp,
                    live: live.clone(),
                };
                let (mut sum, mut half_b) = (vec![Vec3::ZERO; pixels], vec![Vec3::ZERO; pixels]);
                trace_live_pass(ctx, &pass, &mut sum, &mut half_b, cancel);
                gather_live(live, &sum, &half_b)
            },
        ))
    }
}

/// A lane with one scripted worker; `remote_only` makes this computer sit out.
fn scripted_lane<'a>(
    scene: &'a SceneSnapshot,
    size: u32,
    calls: &CallLog,
    fail_from_call: Option<usize>,
    remote_only: bool,
) -> RemoteLane<'a> {
    RemoteLane {
        engines: vec![Box::new(ScriptedWorker {
            scene,
            size,
            calls: Arc::clone(calls),
            fail_from_call,
        })],
        scene: Some(scene_state_from_snapshot(scene, size, size, 0.3, 0.2)),
        note: None,
        remote_only,
    }
}

/// An adaptive render driven through `lane`, returning the outcome and every progress report.
fn render_with_lane(
    scene: &SceneSnapshot,
    (size, cap): (u32, u32),
    lane: RemoteLane<'_>,
    compute_target: ComputeTarget,
) -> (
    Result<Accumulation, AccumulationOutcome>,
    Vec<ExportProgress>,
) {
    let cancel = AtomicBool::new(false);
    let taps = taps_with(settings(cap, 0.5));
    let mut reports = Vec::new();
    let outcome = with_export_ctx(
        scene,
        0.3,
        0.2,
        size,
        size,
        &GpuBackend::disabled(),
        None,
        &cancel,
        |ctx| {
            let ctx = ctx.expect("the context builds");
            let job = RenderJob {
                scene,
                cam_yaw: 0.3,
                cam_pitch: 0.2,
                width: size,
                height: size,
                samples_per_pixel: cap,
                sample_offset: 0,
                compute_target,
                worker: None,
                cancel: &cancel,
            };
            render_adaptive_with_lane(
                ctx,
                &job,
                LocalComputeTarget::Cpu,
                settings(cap, 0.5),
                &taps,
                lane,
                |progress| reports.push(progress),
            )
        },
    );
    (outcome, reports)
}

fn new_log() -> CallLog {
    Arc::new(Mutex::new(Vec::new()))
}

#[test]
fn an_adaptive_render_with_a_worker_is_the_local_render_and_the_split_is_sticky() {
    let scene = capture();
    let (size, cap) = (40_u32, 64_u32);
    let (local, _) = render_with_lane(
        &scene,
        (size, cap),
        RemoteLane::local_only(),
        ComputeTarget::LocalOnly,
    );
    let local = local.ok().expect("the local render completes");

    let calls = new_log();
    let lane = scripted_lane(&scene, size, &calls, None, false);
    let (split, _) = render_with_lane(&scene, (size, cap), lane, ComputeTarget::Both);
    let split = split.ok().expect("the split render completes");

    assert_eq!(split.accum, local.accum, "the same image, bit for bit");
    assert_eq!(
        split.adaptive, local.adaptive,
        "the same counts, so the same tiles were live in every pass"
    );

    let calls = calls.lock().unwrap();
    assert!(calls.len() >= 2, "the worker took part in several passes");
    let odd_tiles: Vec<u32> = (0..25).filter(|tile| tile % 2 == 1).collect();
    assert_eq!(calls[0].1, odd_tiles, "tile i goes to engine i % 2");
    for pair in calls.windows(2) {
        assert!(pair[1].0 > pair[0].0, "the passes advance");
        assert!(
            pair[1].1.iter().all(|tile| pair[0].1.contains(tile)),
            "a tile that stays live stays with its worker"
        );
    }
    assert!(
        calls
            .iter()
            .all(|(_, tiles)| tiles.iter().all(|t| t % 2 == 1))
    );
    drop(calls);
}

#[test]
fn a_remote_only_render_sends_every_tile_to_the_worker() {
    let scene = capture();
    let (size, cap) = (24_u32, 32_u32);
    let (local, _) = render_with_lane(
        &scene,
        (size, cap),
        RemoteLane::local_only(),
        ComputeTarget::LocalOnly,
    );
    let local = local.ok().expect("the local render completes");

    let calls = new_log();
    let lane = scripted_lane(&scene, size, &calls, None, true);
    let (remote, _) = render_with_lane(&scene, (size, cap), lane, ComputeTarget::RemoteOnly);
    let remote = remote.ok().expect("the remote render completes");

    assert_eq!(remote.accum, local.accum);
    assert_eq!(remote.adaptive, local.adaptive);
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls[0].1,
        (0..9).collect::<Vec<u32>>(),
        "nothing stays here"
    );
}

#[test]
fn a_worker_that_fails_hands_its_tiles_back_and_is_not_asked_again() {
    let scene = capture();
    let (size, cap) = (40_u32, 64_u32);
    let (local, _) = render_with_lane(
        &scene,
        (size, cap),
        RemoteLane::local_only(),
        ComputeTarget::LocalOnly,
    );
    let local = local.ok().expect("the local render completes");

    let calls = new_log();
    // The worker answers the first pass and breaks on the second.
    let lane = scripted_lane(&scene, size, &calls, Some(2), false);
    let (split, reports) = render_with_lane(&scene, (size, cap), lane, ComputeTarget::Both);
    let split = split.ok().expect("the render survives the loss");

    assert_eq!(split.accum, local.accum, "the image does not notice");
    assert_eq!(split.adaptive, local.adaptive);
    assert_eq!(calls.lock().unwrap().len(), 2, "never asked again");
    let notes: Vec<&str> = reports.iter().filter_map(|r| r.note.as_deref()).collect();
    assert_eq!(notes.len(), 2, "the loss and the closing line: {notes:?}");
    assert!(notes[0].contains("remote worker stopped"), "{notes:?}");
    assert!(notes[0].contains("this computer traces"), "{notes:?}");
}

#[test]
fn without_this_computer_a_failed_worker_fails_the_render() {
    let scene = capture();
    let calls = new_log();
    let lane = scripted_lane(&scene, 16, &calls, Some(1), true);
    let (outcome, _) = render_with_lane(&scene, (16, 32), lane, ComputeTarget::RemoteOnly);
    let Err(AccumulationOutcome::Failed(message)) = outcome else {
        panic!("a Remote Only render has nothing to fall back on");
    };
    assert!(message.contains("remote worker failed"), "{message}");
}

#[test]
fn a_split_pass_merges_into_the_buffers_of_a_local_pass_and_failover_is_deterministic() {
    let scene = capture();
    let (size, pixels) = (24_u32, 24_usize * 24);
    let cancel = AtomicBool::new(false);
    with_export_ctx(
        &scene,
        0.3,
        0.2,
        size,
        size,
        &GpuBackend::disabled(),
        None,
        &cancel,
        |ctx| {
            let ctx = ctx.expect("the context builds");
            // Two passes of the whole frame (an odd first sample, so half-buffer B has work).
            let passes = [(5_u32, 6_u32), (11, 6)];
            let pass = |(first_sample, spp): (u32, u32)| Pass {
                first_sample,
                spp,
                live: LiveTiles::all(size, size, 8),
            };
            let run = |lane: RemoteLane<'_>| {
                let mut engines = PassEngines::new(ctx, &cancel, false, lane, 9);
                let (mut sum, mut half_b) = (vec![Vec3::ZERO; pixels], vec![Vec3::ZERO; pixels]);
                let notes: Vec<Option<String>> = passes
                    .iter()
                    .map(|&p| {
                        engines
                            .trace(&pass(p), &mut sum, &mut half_b)
                            .ok()
                            .expect("the pass completes")
                    })
                    .collect();
                (sum, half_b, notes, engines.assignment().clone())
            };

            let (want_sum, want_half, _, _) = run(RemoteLane::local_only());
            assert!(want_half.iter().any(|v| *v != Vec3::ZERO));

            let calls = new_log();
            let (sum, half_b, notes, assignment) =
                run(scripted_lane(&scene, size, &calls, None, false));
            assert_eq!(sum, want_sum, "the merged sums are the local pass's");
            assert_eq!(half_b, want_half, "so is the merged odd-sample half-buffer");
            assert_eq!(notes, [None, None]);
            assert!(assignment.is_alive(1));
            assert_eq!(calls.lock().unwrap().len(), 2);

            // The worker dies on the second pass: same buffers, and the tiles are all here now.
            let calls = new_log();
            let (sum, half_b, notes, assignment) =
                run(scripted_lane(&scene, size, &calls, Some(2), false));
            assert_eq!(sum, want_sum);
            assert_eq!(half_b, want_half);
            assert_eq!(notes[0], None);
            assert!(notes[1].as_deref().is_some_and(|n| n.contains("stopped")));
            assert!(!assignment.is_alive(1));
            assert!((0..9).all(|tile| assignment.owner(tile) == Some(0)));
        },
    );
}
