//! Tests of the opt-in GPU-resident convergence path of the adaptive export, as far as they need
//! no adapter: when it is asked for, when it declines, and that a declined run renders the very
//! image the standard adaptive path renders. (The GPU side is tested in
//! `renderer::gpu_backend::resident_tests`.)

use super::{
    super::{
        super::{taps::RenderTaps, types::AccumulationCarry},
        render_accumulation, with_export_ctx,
    },
    remote_lane::RemoteLane,
    resident_wanted, *,
};
use crate::{
    bridge::{
        export_thread::{
            params::{ComputeTarget, ExportParams},
            scene_snapshot::SceneSnapshot,
        },
        render_thread::RenderContext,
    },
    settings::LocalComputeTarget,
};
use pleochroma::renderer::{adaptive::NoiseMetric, gpu_backend::GpuBackend};
use std::sync::{Mutex, atomic::AtomicBool};

fn capture() -> SceneSnapshot {
    SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("Diamond resolves")
}

fn settings() -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise: 0.5,
        metric: NoiseMetric::RelativeLstar,
        min_spp: 16,
        max_spp: 48,
        pass_spp: 8,
        tile: 8,
    }
}

fn taps(gpu_convergence: bool) -> RenderTaps {
    RenderTaps {
        adaptive: Some(settings()),
        gpu_convergence,
        ..RenderTaps::default()
    }
}

/// One adaptive render of a 24 x 24 frame on a disabled GPU backend.
fn render(
    scene: &SceneSnapshot,
    taps: RenderTaps,
    local: LocalComputeTarget,
    notes: &mut Vec<String>,
) -> Accumulation {
    let cancel = AtomicBool::new(false);
    let outcome = render_accumulation(
        scene,
        0.3,
        0.2,
        ExportParams {
            engine: scene.engine,
            width: 24,
            height: 24,
            samples_per_pixel: 48,
            max_bounces: scene.max_bounces,
        },
        ComputeTarget::LocalOnly,
        None,
        &GpuBackend::disabled(),
        local,
        &mut AccumulationCarry::with_taps(taps),
        &cancel,
        |progress| notes.extend(progress.note),
    );
    match outcome {
        AccumulationOutcome::Completed(done) => done,
        AccumulationOutcome::Cancelled => panic!("the render was cancelled"),
        AccumulationOutcome::Failed(message) => panic!("the render failed: {message}"),
    }
}

/// With no usable GPU the flag changes nothing but a note: the image and the counts are the
/// standard adaptive render's.
#[test]
fn a_declined_resident_run_renders_the_standard_adaptive_image() {
    let scene = capture();
    let (mut plain_notes, mut flagged_notes) = (Vec::new(), Vec::new());
    let plain = render(
        &scene,
        taps(false),
        LocalComputeTarget::CpuGpu,
        &mut plain_notes,
    );
    let flagged = render(
        &scene,
        taps(true),
        LocalComputeTarget::CpuGpu,
        &mut flagged_notes,
    );
    assert_eq!(flagged.accum, plain.accum);
    assert_eq!(flagged.samples_per_pixel, plain.samples_per_pixel);
    assert_eq!(
        flagged.adaptive.as_ref().map(|s| &s.counts),
        plain.adaptive.as_ref().map(|s| &s.counts)
    );
    assert!(
        flagged_notes
            .iter()
            .any(|note| note.contains("GPU-resident")),
        "the fall back is said once: {flagged_notes:?}"
    );
    assert!(
        !plain_notes.iter().any(|note| note.contains("GPU-resident")),
        "an ordinary render says nothing about it: {plain_notes:?}"
    );
}

/// Without the flag the render is untouched, and the CPU-only choice never offers the GPU.
#[test]
fn the_resident_path_is_asked_for_only_when_the_flag_and_the_gpu_allow_it() {
    let scene = capture();
    let cancel = AtomicBool::new(false);
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
            let lane = RemoteLane::local_only();
            assert!(!resident_wanted(
                ctx,
                &taps(false),
                LocalComputeTarget::CpuGpu,
                &lane
            ));
            assert!(resident_wanted(
                ctx,
                &taps(true),
                LocalComputeTarget::CpuGpu,
                &lane
            ));
            assert!(
                !resident_wanted(ctx, &taps(true), LocalComputeTarget::Cpu, &lane),
                "a CPU-only render never offers the GPU"
            );
        },
    );
}
