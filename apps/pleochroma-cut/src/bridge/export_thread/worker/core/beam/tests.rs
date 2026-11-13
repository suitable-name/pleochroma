//! Tests of the beam branch of the export: every local compute choice, the fallback for a stone
//! beams cannot render, the notes, the two-phase progress, cancel and the tilt-video frames. All
//! CPU only unless a test says it needs an adapter.
//!
//! The tolerances are named constants at the top of the file.

use super::{
    super::{
        super::types::{AccumulationCarry, AccumulationOutcome},
        render_accumulation, with_export_ctx,
    },
    *,
};
use crate::bridge::{
    export_thread::{ComputeTarget, params::ExportParams, scene_snapshot::SceneSnapshot},
    render_thread::RenderContext,
};
use pleochroma::renderer::{accel::PathCacheSettings, adaptive::NoiseMetric};
use std::sync::{Mutex, atomic::AtomicBool};

/// The frame edge of the small renders, in pixels (nine 8-pixel tiles).
const SIZE: u32 = 24;
/// The samples per pixel of the small renders (the residual's cap).
const CAP: u32 = 16;
/// The most two images of the same render may differ, relative to the mean pixel, when they were
/// made by different compute choices.
const CHOICE_TOLERANCE: f32 = 1e-3;
/// The most the frame means of a CPU render and a GPU render may differ, as a share of the CPU's.
#[cfg(feature = "gpu")]
const GPU_MEAN_TOLERANCE: f32 = 0.05;
/// How long a GUI cancel may take to stop a beam export (guide section 14.11).
const CANCEL_BOUND: Duration = Duration::from_millis(100);

/// The options of one render: where the camera is, how large, and who computes.
#[derive(Clone, Copy)]
struct Options {
    pose: (f32, f32),
    size: u32,
    spp: u32,
    compute_target: ComputeTarget,
    local: LocalComputeTarget,
}

impl Options {
    /// A small CPU render on this computer only.
    const DEFAULT: Self = Self {
        pose: (0.3, 0.2),
        size: SIZE,
        spp: CAP,
        compute_target: ComputeTarget::LocalOnly,
        local: LocalComputeTarget::Cpu,
    };
}

fn capture() -> SceneSnapshot {
    SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("Diamond resolves")
}

/// Coarse beam settings: four tiles per batch (three batches on the small frame), a small residual
/// minimum and a coarse tree. The tests check structure (bit identity, notes, progress, routing),
/// which holds at any tree resolution; the defaults make the tree build dominate the run.
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
        residual_min_spp: 8,
        batch_tiles: 4,
        ..BeamSettings::default()
    }
}

/// Eight-pixel tiles and eight-sample passes, capped at `max_spp`.
fn adaptive_settings(max_spp: u32) -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise: 0.5,
        metric: NoiseMetric::RelativeLstar,
        min_spp: 8,
        max_spp,
        pass_spp: 8,
        tile: 8,
    }
}

/// Adaptive taps with the given beam choice.
fn taps(beam: Option<BeamSettings>) -> RenderTaps {
    RenderTaps {
        adaptive: Some(adaptive_settings(CAP)),
        beam,
        ..RenderTaps::default()
    }
}

fn render(
    scene: &SceneSnapshot,
    options: Options,
    taps: RenderTaps,
    gpu: &GpuBackend,
    cancel: &AtomicBool,
    report_progress: impl FnMut(ExportProgress),
) -> AccumulationOutcome {
    render_accumulation(
        scene,
        options.pose.0,
        options.pose.1,
        ExportParams {
            engine: scene.engine,
            width: options.size,
            height: options.size,
            samples_per_pixel: options.spp,
            max_bounces: scene.max_bounces,
        },
        options.compute_target,
        None,
        gpu,
        options.local,
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

/// A finished CPU-backend render and every progress report it made.
fn run(
    scene: &SceneSnapshot,
    options: Options,
    taps: RenderTaps,
) -> (Accumulation, Vec<ExportProgress>) {
    let cancel = AtomicBool::new(false);
    let mut reports = Vec::new();
    let done = completed(render(
        scene,
        options,
        taps,
        &GpuBackend::disabled(),
        &cancel,
        |progress| reports.push(progress),
    ));
    (done, reports)
}

/// The notes of the reports, in order.
fn notes(reports: &[ExportProgress]) -> Vec<&str> {
    reports
        .iter()
        .filter_map(|report| report.note.as_deref())
        .collect()
}

/// The largest per-channel difference between two images, relative to the mean pixel of `a`.
fn max_relative_difference(a: &[Vec3], b: &[Vec3]) -> f32 {
    assert_eq!(a.len(), b.len(), "images of one size");
    let mean = a.iter().map(|p| p.element_sum()).sum::<f32>() / (3 * a.len().max(1)) as f32;
    let worst = a
        .iter()
        .zip(b)
        .map(|(p, q)| (*p - *q).abs().max_element())
        .fold(0.0, f32::max);
    worst / mean.max(f32::MIN_POSITIVE)
}

/// The mean Y of an image.
#[cfg(feature = "gpu")]
fn mean_y(image: &[Vec3]) -> f32 {
    image.iter().map(|p| p.y).sum::<f32>() / image.len().max(1) as f32
}

#[test]
fn the_default_taps_have_no_beams_and_an_ordinary_render_is_unchanged() {
    assert!(RenderTaps::default().beam.is_none());
    let scene = capture();
    let plain = RenderTaps {
        adaptive: Some(adaptive_settings(CAP)),
        ..RenderTaps::default()
    };
    let (first, reports) = run(&scene, Options::DEFAULT, plain.clone());
    let (second, _) = run(&scene, Options::DEFAULT, plain);
    assert_eq!(
        first.accum, second.accum,
        "an ordinary render is repeatable"
    );
    assert!(
        reports.iter().all(|report| report.beam.is_none()),
        "no beam status without beams"
    );
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 04b runs it"]
fn an_eligible_stone_renders_from_beams_with_two_phase_progress() {
    let scene = capture();
    let (done, reports) = run(&scene, Options::DEFAULT, taps(Some(beam_settings())));
    let pixels = (SIZE * SIZE) as usize;
    assert_eq!(done.accum.len(), pixels);
    assert_eq!(done.samples_per_pixel, 1, "the buffer holds means");
    assert!(
        done.accum
            .iter()
            .all(|p| p.is_finite() && p.min_element() >= 0.0),
        "final pixels are finite and clamped at zero"
    );
    assert!(done.accum.iter().any(|p| p.y > 0.0), "the stone is drawn");
    let adaptive_stats = done.adaptive.as_ref().expect("the residual's counts");
    assert_eq!(adaptive_stats.counts.len(), pixels);
    assert!(adaptive_stats.counts.iter().all(|&count| count <= CAP));
    assert_eq!(adaptive_stats.max_spp, CAP);
    assert_eq!(adaptive_stats.tiles_total, 9);

    let first = reports.first().expect("reports");
    assert!(first.fraction.abs() < f32::EPSILON, "the first report is 0");
    assert_eq!(first.beam.map(|b| b.phase), Some(BeamPhase::Tracing));
    let mut previous = 0.0;
    let mut finishing = false;
    for report in &reports {
        let status = report.beam.expect("every report carries the beam status");
        assert!(report.fraction >= previous, "the share never goes back");
        assert!(report.fraction <= 1.0);
        assert_eq!(report.samples_total, CAP);
        assert!(
            !(finishing && status.phase == BeamPhase::Tracing),
            "the phase never goes back from finishing to tracing"
        );
        finishing |= status.phase == BeamPhase::Finishing;
        previous = report.fraction;
    }
    let last = reports.last().expect("reports");
    assert!((last.fraction - 1.0).abs() < f32::EPSILON);
    assert_eq!(last.beam.map(|b| b.phase), Some(BeamPhase::Finishing));
    let note = last.note.as_deref().expect("the closing note");
    assert!(note.starts_with("Beams: "), "{note}");
    assert!(
        note.contains(" leaves, ") && note.ends_with(" % residual"),
        "{note}"
    );
    eprintln!("beam export note: {note}");
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 04b runs it"]
fn every_local_compute_choice_gives_the_same_image() {
    let scene = capture();
    let (cpu, _) = run(&scene, Options::DEFAULT, taps(Some(beam_settings())));
    for local in [LocalComputeTarget::CpuGpu, LocalComputeTarget::Gpu] {
        let (other, _) = run(
            &scene,
            Options {
                local,
                ..Options::DEFAULT
            },
            taps(Some(beam_settings())),
        );
        let difference = max_relative_difference(&cpu.accum, &other.accum);
        eprintln!("compute choice {local:?}: relative difference {difference:e}");
        assert!(difference <= CHOICE_TOLERANCE, "{local:?}: {difference}");
        assert_eq!(
            cpu.adaptive.as_ref().map(|s| &s.counts),
            other.adaptive.as_ref().map(|s| &s.counts)
        );
    }
}

/// A real adapter renders the same frame as the CPU within the GPU twin tolerance. Skips
/// cleanly where there is no adapter.
#[cfg(feature = "gpu")]
#[test]
fn a_real_adapter_matches_the_cpu_choice_on_the_frame_mean() {
    let backend = GpuBackend::acquire();
    if backend.adapter_label().is_none() {
        eprintln!("no GPU adapter: skipping the GPU rows");
        return;
    }
    let scene = capture();
    let (cpu, _) = run(&scene, Options::DEFAULT, taps(Some(beam_settings())));
    let cpu_mean = mean_y(&cpu.accum);
    for local in [LocalComputeTarget::Gpu, LocalComputeTarget::CpuGpu] {
        let cancel = AtomicBool::new(false);
        let gpu = completed(render(
            &scene,
            Options {
                local,
                ..Options::DEFAULT
            },
            taps(Some(beam_settings())),
            &backend,
            &cancel,
            |_| {},
        ));
        let gpu_mean = mean_y(&gpu.accum);
        eprintln!("compute choice {local:?}: mean Y {gpu_mean} against the CPU's {cpu_mean}");
        assert!(
            (gpu_mean - cpu_mean).abs() <= GPU_MEAN_TOLERANCE * cpu_mean,
            "{local:?}: {gpu_mean} against {cpu_mean}"
        );
    }
}

#[test]
fn an_ineligible_stone_renders_as_without_beams_and_says_why() {
    let mut scene = capture();
    scene.material.edge_rounding_radius = 0.2;
    let (without, _) = run(&scene, Options::DEFAULT, taps(None));
    let (with, reports) = run(&scene, Options::DEFAULT, taps(Some(beam_settings())));
    assert_eq!(with.accum, without.accum, "byte-identical to no beams");
    assert_eq!(with.adaptive, without.adaptive);
    assert!(
        reports.iter().all(|report| report.beam.is_none()),
        "the ordinary render reports no beam status"
    );
    let said = notes(&reports);
    assert!(
        said.iter()
            .any(|note| note.contains("Beam tracing off: the material rounds its edges")),
        "{said:?}"
    );
}

/// Settings a worker would refuse are not rendered locally either: the ordinary render follows
/// with the reason as a note.
#[test]
fn invalid_local_beam_settings_render_as_without_beams_and_say_why() {
    let scene = capture();
    let (without, _) = run(&scene, Options::DEFAULT, taps(None));
    let invalid = BeamSettings {
        spectral_nodes: 12,
        ..beam_settings()
    };
    let (with, reports) = run(&scene, Options::DEFAULT, taps(Some(invalid)));
    assert_eq!(with.accum, without.accum, "byte-identical to no beams");
    assert!(
        reports.iter().all(|report| report.beam.is_none()),
        "the ordinary render reports no beam status"
    );
    let said = notes(&reports);
    assert!(
        said.iter()
            .any(|note| note.contains("Beam tracing off: spectral_nodes must be")),
        "{said:?}"
    );
}

#[test]
fn eligibility_follows_the_renderers_rules() {
    let cancel = AtomicBool::new(false);
    let check = |scene: &SceneSnapshot| {
        with_export_ctx(
            scene,
            0.3,
            0.2,
            SIZE,
            SIZE,
            &GpuBackend::disabled(),
            None,
            &cancel,
            |ctx| {
                let ctx = ctx.expect("the context builds");
                let frame = frame_of(ctx);
                let extras: Extras<'_> = (&scene.tools, &scene.fluorescence, ctx.surface);
                (
                    eligible(&frame, extras, &beam_settings()),
                    renders_stone(Some(&beam_settings()), &frame, extras),
                    renders_stone(None, &frame, extras),
                )
            },
        )
    };
    assert_eq!(check(&capture()), (Ok(()), true, false));
    let mut rounded = capture();
    rounded.material.edge_rounding_radius = 0.2;
    assert_eq!(
        check(&rounded),
        (Err(BeamOff::EdgeRounding), false, false),
        "a rounding material is not eligible"
    );
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 04b runs it"]
fn remote_and_both_render_on_this_computer_and_say_so() {
    let scene = capture();
    let (local, local_reports) = run(&scene, Options::DEFAULT, taps(Some(beam_settings())));
    for compute_target in [ComputeTarget::Both, ComputeTarget::RemoteOnly] {
        let (other, reports) = run(
            &scene,
            Options {
                compute_target,
                ..Options::DEFAULT
            },
            taps(Some(beam_settings())),
        );
        assert_eq!(other.accum, local.accum, "{compute_target:?}");
        // The first report is the bare one before the lane opens: read the first note.
        let first = notes(&reports).first().copied();
        assert!(
            first.is_some_and(|note| note.contains(NOTE_BEAMS_LOCAL_ONLY)),
            "{compute_target:?}: {first:?}"
        );
    }
    assert!(
        notes(&local_reports)
            .iter()
            .all(|note| !note.contains(NOTE_BEAMS_LOCAL_ONLY)),
        "a local render says nothing about it"
    );
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 04b runs it"]
fn beams_win_over_the_path_cache_and_the_gpu_resident_check_on_an_eligible_stone() {
    let scene = capture();
    let (beams_only, _) = run(&scene, Options::DEFAULT, taps(Some(beam_settings())));
    let both = RenderTaps {
        path_cache: Some(PathCacheSettings::default()),
        gpu_convergence: true,
        ..taps(Some(beam_settings()))
    };
    let (with_cache, reports) = run(&scene, Options::DEFAULT, both);
    assert_eq!(
        with_cache.accum, beams_only.accum,
        "the path cache is not used for the stone"
    );
    // The first report is the bare 0 % one made before the lane opens; the opening note
    // comes with the second one, and it is the first note said.
    assert_eq!(
        reports.iter().position(|r| r.note.is_some()),
        Some(1),
        "the opening note follows the bare starting report"
    );
    let first = notes(&reports).first().copied().expect("the opening note");
    assert!(first.contains(NOTE_BEAMS_REPLACE_PATH_CACHE), "{first}");
    assert!(first.contains(NOTE_BEAMS_HOST_CONVERGENCE), "{first}");
}

#[test]
fn the_path_cache_stays_for_an_ineligible_stone() {
    let mut scene = capture();
    scene.material.edge_rounding_radius = 0.2;
    let cache = RenderTaps {
        path_cache: Some(PathCacheSettings::default()),
        ..taps(None)
    };
    let with_beams = RenderTaps {
        beam: Some(beam_settings()),
        ..cache.clone()
    };
    let (without, _) = run(&scene, Options::DEFAULT, cache);
    let (with, reports) = run(&scene, Options::DEFAULT, with_beams);
    assert_eq!(with.accum, without.accum, "the cache render is untouched");
    assert!(
        notes(&reports)
            .iter()
            .all(|note| !note.contains(NOTE_BEAMS_REPLACE_PATH_CACHE)),
        "beams did not replace anything"
    );
}

#[test]
fn a_cancel_before_the_call_ends_cancelled() {
    let scene = capture();
    let cancel = AtomicBool::new(true);
    let outcome = render(
        &scene,
        Options::DEFAULT,
        taps(Some(beam_settings())),
        &GpuBackend::disabled(),
        &cancel,
        |_| {},
    );
    assert!(matches!(outcome, AccumulationOutcome::Cancelled));
}

#[test]
fn a_gui_cancel_stops_a_beam_export_within_the_bound() {
    let scene = capture();
    let cancel = AtomicBool::new(false);
    let raised: Mutex<Option<Instant>> = Mutex::new(None);
    let options = Options {
        size: 192,
        spp: 4096,
        ..Options::DEFAULT
    };
    let long = RenderTaps {
        adaptive: Some(adaptive_settings(4096)),
        beam: Some(beam_settings()),
        ..RenderTaps::default()
    };
    let outcome = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(40));
            *raised.lock().expect("the lock") = Some(Instant::now());
            cancel.store(true, Ordering::Release);
        });
        render(
            &scene,
            options,
            long,
            &GpuBackend::disabled(),
            &cancel,
            |_| {},
        )
    });
    let returned = Instant::now();
    assert!(matches!(outcome, AccumulationOutcome::Cancelled));
    let raised = raised
        .lock()
        .expect("the lock")
        .expect("the cancel was raised");
    let latency = returned.saturating_duration_since(raised);
    eprintln!("beam export cancel latency: {latency:?}");
    assert!(latency <= CANCEL_BOUND, "{latency:?}");
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 04b runs it"]
fn every_tilt_frame_runs_its_own_tree() {
    let scene = capture();
    let cancel = AtomicBool::new(false);
    let poses = [(0.3_f32, 0.2_f32), (0.6, 0.2)];
    let mut carry = AccumulationCarry::with_taps(taps(Some(beam_settings())));
    let mut images = Vec::new();
    for (frame, pose) in poses.into_iter().enumerate() {
        carry.set_frame_name(Some(format!("frame_{frame:04}")));
        let mut reports = Vec::new();
        let done = completed(render_accumulation(
            &scene,
            pose.0,
            pose.1,
            ExportParams {
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
            &mut carry,
            &cancel,
            |progress| reports.push(progress),
        ));
        let last = reports.last().expect("reports");
        assert!(
            last.note
                .as_deref()
                .is_some_and(|n| n.starts_with("Beams: ")),
            "frame {frame} built its own tree: {:?}",
            last.note
        );
        assert_eq!(last.beam.map(|b| b.phase), Some(BeamPhase::Finishing));
        images.push(done.accum);
    }
    assert_ne!(images[0], images[1], "the poses differ");
    // No state leaks from one frame into the next: the second frame equals a fresh render.
    let (fresh, _) = run(
        &scene,
        Options {
            pose: poses[1],
            ..Options::DEFAULT
        },
        taps(Some(beam_settings())),
    );
    assert_eq!(images[1], fresh.accum);
}

#[test]
fn the_residual_settings_are_the_requests_with_the_renders_cap_or_the_provisional_ones() {
    assert_eq!(
        residual_settings(64, None),
        AdaptiveSettings::provisional(64)
    );
    let requested = AdaptiveSettings {
        max_spp: 999,
        ..adaptive_settings(999)
    };
    assert_eq!(
        residual_settings(64, Some(requested)),
        AdaptiveSettings {
            max_spp: 64,
            ..requested
        }
    );
}

#[test]
fn the_opening_note_names_what_beams_overrode() {
    assert_eq!(start_note(false, false, false), None);
    assert_eq!(
        start_note(true, false, false),
        Some(format!("{NOTE_BEAMS_REPLACE_PATH_CACHE}."))
    );
    assert_eq!(
        start_note(true, true, true),
        Some(format!(
            "{NOTE_BEAMS_REPLACE_PATH_CACHE}. {NOTE_BEAMS_HOST_CONVERGENCE}. \
             {NOTE_BEAMS_LOCAL_ONLY}."
        ))
    );
}

#[test]
fn shares_and_samples_convert_without_leaving_their_range() {
    assert_eq!(samples_at(64, 0.5), 32);
    assert_eq!(samples_at(64, -1.0), 0);
    assert_eq!(samples_at(64, 2.0), 64);
    assert_eq!(samples_at(64, f32::NAN), 0);
    assert!((residual_share(50, (10, 10), 10) - 0.05).abs() < 1e-12);
    assert!(residual_share(50, (10, 10), 0).abs() < f64::EPSILON);
}

#[test]
fn the_throttle_forwards_the_first_report_a_change_of_phase_and_one_per_interval() {
    let base = Instant::now();
    let mut throttle = Throttle::default();
    let step = Duration::from_millis(10);
    assert!(throttle.admits(BeamPhase::Tracing, base));
    assert!(!throttle.admits(BeamPhase::Tracing, base + step));
    assert!(throttle.admits(BeamPhase::Finishing, base + step));
    assert!(!throttle.admits(BeamPhase::Finishing, base + step * 2));
    assert!(throttle.admits(BeamPhase::Finishing, base + step + PROGRESS_INTERVAL));
}

#[test]
fn a_finished_batch_lands_on_its_tiles_in_tile_order() {
    let mut frame = vec![Vec3::ZERO; 4 * 2];
    let tiles = [TileRect {
        x0: 1,
        y0: 0,
        x1: 3,
        y1: 2,
    }];
    let pixels: Vec<Vec3> = (1..=4).map(|v| Vec3::splat(v as f32)).collect();
    paste_batch(&mut frame, 4, &tiles, &pixels);
    let want = [0.0, 1.0, 2.0, 0.0, 0.0, 3.0, 4.0, 0.0];
    assert_eq!(frame.iter().map(|p| p.x).collect::<Vec<_>>(), want);

    // A short pixel list stops at its end and never panics.
    let mut short = vec![Vec3::ZERO; 8];
    paste_batch(&mut short, 4, &tiles, &pixels[..1]);
    assert_eq!(short.iter().filter(|p| **p != Vec3::ZERO).count(), 1);
}

#[test]
fn the_tile_figures_count_the_tiles_that_stopped_before_the_cap() {
    let (width, height, cap) = (20_u32, 10_u32, 16_u32);
    let mut counts = vec![cap; (width * height) as usize];
    counts[0] = 4;
    counts[(8 * width + 16) as usize] = 5;
    assert_eq!(tile_figures((width, height), 8, &counts, cap), (6, 2));
    assert_eq!(tile_figures((width, height), 8, &[], cap), (6, 0));
}

#[test]
fn the_accumulation_of_a_beam_image_keeps_its_pixels_and_counts() {
    let image = AdaptiveImage {
        width: 2,
        height: 1,
        sum: vec![Vec3::ZERO; 2],
        half_b: vec![Vec3::ZERO; 2],
        counts: vec![16, 8],
        pixels: vec![Vec3::ONE, Vec3::splat(2.0)],
        samples_traced: 24,
    };
    let done = accumulation(image, 16, 8);
    assert_eq!(done.accum, vec![Vec3::ONE, Vec3::splat(2.0)]);
    assert_eq!(done.samples_per_pixel, 1);
    assert_eq!(
        done.adaptive,
        Some(AdaptiveStats {
            counts: vec![16, 8],
            samples_done: 16,
            samples_traced: 24,
            max_spp: 16,
            tiles_total: 1,
            tiles_converged: 0,
        })
    );
}
