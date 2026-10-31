//! Cancellable spectral frame dispatch and measured CPU/GPU work sharing.

use super::{
    super::scanline::{
        render_frame_scanlines_cancellable, render_frame_scanlines_with_threads_cancellable,
    },
    BackendFrame, FrameOutputs, ViewportGpu,
};
#[cfg(feature = "render-bench")]
use super::{BenchmarkObservation, PhaseTimer};
use crate::settings::model::LocalComputeTarget;
use glam::Vec3;
use pleochroma::renderer::{
    buffers::{GpuGemMaterial, GpuSurfaceScene},
    gpu_backend::GpuAccumulate,
};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    sync::atomic::{AtomicBool, Ordering},
};

/// Compatibility entry point for deterministic CPU/GPU equivalence fixtures.
#[cfg(test)]
pub(in crate::bridge::render_thread) fn accumulate_frame_samples(
    backend: &mut ViewportGpu,
    frame: &BackendFrame<'_>,
    outputs: &mut FrameOutputs<'_>,
    hybrid: &mut HybridPacing,
    target: LocalComputeTarget,
) {
    accumulate_frame_samples_cancellable(
        backend,
        frame,
        outputs,
        hybrid,
        target,
        &AtomicBool::new(false),
    );
}

/// Completes exactly the requested sample range, or returns false for cancellation.
///
/// A cancelled GPU request never falls through to the CPU and never marks a healthy
/// device lost. CPU and guide cancellation may leave partial output; the render loop
/// discards that stale accumulation before reading the next camera snapshot.
pub(in crate::bridge::render_thread) fn accumulate_frame_samples_cancellable(
    backend: &mut ViewportGpu,
    frame: &BackendFrame<'_>,
    outputs: &mut FrameOutputs<'_>,
    hybrid: &mut HybridPacing,
    target: LocalComputeTarget,
    cancel: &AtomicBool,
) -> bool {
    #[cfg(feature = "render-bench")]
    {
        backend.observation = BenchmarkObservation::default();
    }
    if cancel.load(Ordering::Relaxed) {
        return false;
    }
    hybrid.prepare_frame(frame);
    if target == LocalComputeTarget::Cpu || !frame.routes_to_gpu() {
        // Nothing polls the preparation worker on this route, so its pill would stay.
        backend.clear_preparing_status();
        #[cfg(feature = "render-bench")]
        if target != LocalComputeTarget::Cpu {
            backend
                .observation
                .fallback("Path scene is unsupported by GPU admission");
        }
        return cpu_frame(backend, frame, outputs, hybrid, cancel);
    }
    if target == LocalComputeTarget::CpuGpu {
        // Compilation never blocks the first useful CPU image. Calibration runs on
        // one bounded worker and never contributes samples to accumulation.
        if !backend.poll_preparation(frame) {
            return cpu_frame(backend, frame, outputs, hybrid, cancel);
        }
        hybrid.poll_calibration(frame, backend.shared_hdr.as_ref());
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        if let Some(gpu_share) = hybrid.gpu_share(frame.spp) {
            return hybrid_frame(backend, frame, outputs, hybrid, gpu_share, cancel);
        }
        if hybrid.prefer_cpu() {
            return cpu_frame(backend, frame, outputs, hybrid, cancel);
        }
    } else if !backend.wait_preparation(frame, cancel) {
        return false;
    }
    let start = std::time::Instant::now();
    #[cfg(feature = "render-bench")]
    {
        backend.observation.route = "gpu";
    }
    match backend.try_accumulate_cancellable(frame, outputs, cancel) {
        GpuAccumulate::Done => {
            hybrid.observe_gpu_only(frame.spp, start.elapsed());
            true
        }
        GpuAccumulate::Cancelled => false,
        GpuAccumulate::Declined => cpu_frame(backend, frame, outputs, hybrid, cancel),
    }
}

fn cpu_frame(
    backend: &mut ViewportGpu,
    frame: &BackendFrame<'_>,
    outputs: &mut FrameOutputs<'_>,
    hybrid: &mut HybridPacing,
    cancel: &AtomicBool,
) -> bool {
    #[cfg(feature = "render-bench")]
    let timer = PhaseTimer::start("path_cpu_radiance_and_guides", false);
    #[cfg(feature = "render-bench")]
    {
        backend.observation.route = "cpu";
    }
    let start = std::time::Instant::now();
    let completed = render_frame_scanlines_cancellable(
        frame,
        frame.spp,
        frame.sample_offset + frame.spp,
        outputs,
        cancel,
    );
    #[cfg(feature = "render-bench")]
    timer.finish(&mut backend.observation);
    backend.invalidate_guide_cache();
    if completed {
        HybridPacing::blend(&mut hybrid.cpu_rate, frame.spp, start.elapsed());
    }
    completed
}

/// Both engines trace disjoint sample ranges. Only a complete pair is merged.
fn hybrid_frame(
    backend: &mut ViewportGpu,
    frame: &BackendFrame<'_>,
    outputs: &mut FrameOutputs<'_>,
    hybrid: &mut HybridPacing,
    gpu_share: u32,
    cancel: &AtomicBool,
) -> bool {
    #[cfg(feature = "render-bench")]
    {
        backend.observation.route = "hybrid";
    }
    let cpu_share = frame.spp - gpu_share;
    hybrid.prepare_scratch(frame.width as usize * frame.height as usize);
    let cpu_threads = pleochroma::renderer::tonemap::live_cpu_budget::partition().0;
    #[cfg(feature = "render-bench")]
    let mut cpu_phase = None;
    let (gpu_result, cpu_done, cpu_time) = std::thread::scope(|scope| {
        let gpu_task = scope.spawn(|| {
            let start = std::time::Instant::now();
            let gpu_frame = BackendFrame {
                spp: gpu_share,
                ..*frame
            };
            let result = backend.try_accumulate_cancellable(&gpu_frame, outputs, cancel);
            (result, start.elapsed())
        });
        #[cfg(feature = "render-bench")]
        tracing::info!(target: "pleochroma_cut::render_bench", phase = "path_hybrid_cpu_radiance_and_guides", overlaps = true, "phase_begin");
        let start = std::time::Instant::now();
        let cpu_done = render_frame_scanlines_with_threads_cancellable(
            frame,
            cpu_share,
            frame.sample_offset + frame.spp,
            &mut FrameOutputs {
                accum: &mut hybrid.cpu_scratch,
                depth: &mut hybrid.scratch_depth,
                normal: &mut hybrid.scratch_normal,
                facet_id: &mut hybrid.scratch_facet,
            },
            cpu_threads,
            cancel,
        );
        let cpu_time = start.elapsed();
        #[cfg(feature = "render-bench")]
        {
            cpu_phase = Some(PhaseTimer::completed(
                "path_hybrid_cpu_radiance_and_guides",
                cpu_time,
                true,
            ));
        }
        (gpu_task.join(), cpu_done, cpu_time)
    });
    #[cfg(feature = "render-bench")]
    backend.observation.phases.extend(cpu_phase);
    let (gpu_outcome, gpu_time) = gpu_result.unwrap_or_else(|_| {
        tracing::error!("GPU render thread panicked mid-frame; retiring the GPU backend");
        backend.retire();
        #[cfg(feature = "render-bench")]
        backend.note_benchmark_gpu_decline();
        (GpuAccumulate::Declined, cpu_time)
    });
    #[cfg(feature = "render-bench")]
    if gpu_outcome == GpuAccumulate::Declined {
        backend.observation.route = "cpu";
    }
    if !cpu_done || gpu_outcome == GpuAccumulate::Cancelled || cancel.load(Ordering::Relaxed) {
        return false;
    }
    for (pixel, extra) in outputs.accum.iter_mut().zip(&hybrid.cpu_scratch) {
        *pixel += *extra;
    }
    if gpu_outcome == GpuAccumulate::Done {
        HybridPacing::blend(&mut hybrid.gpu_rate, gpu_share, gpu_time);
        HybridPacing::blend(&mut hybrid.cpu_rate, cpu_share, cpu_time);
        return true;
    }
    hybrid.gpu_dead = true;
    // The shared GPU backend is transactional on decline, so only its missing sample
    // range is rendered here; the CPU share above must not be duplicated.
    #[cfg(feature = "render-bench")]
    let timer = PhaseTimer::start("path_cpu_missing_gpu_sample_range", false);
    let completed = render_frame_scanlines_cancellable(
        frame,
        gpu_share,
        frame.sample_offset + gpu_share,
        outputs,
        cancel,
    );
    #[cfg(feature = "render-bench")]
    timer.finish(&mut backend.observation);
    backend.invalidate_guide_cache();
    completed
}

/// Workload identity excludes orbit angle so comparable measured work survives camera
/// movement, and the pass size (`spp`): every `PassBudget` ramp step would otherwise be a
/// new workload, dropping the measured rates and the calibration on each doubling.
pub(super) fn workload_key(frame: &BackendFrame<'_>) -> u64 {
    let mut hash = DefaultHasher::new();
    (frame.width, frame.height, frame.max_bounces).hash(&mut hash);
    pleochroma::render_setup::hash_geometry(frame.stone()).hash(&mut hash);
    let surface = GpuSurfaceScene::from_scene(&frame.surface);
    let material = surface.apply_to_material(GpuGemMaterial::encode(frame.material));
    bytemuck::bytes_of(&material).hash(&mut hash);
    surface
        .encode_finishes(frame.facet_finishes, frame.planes.len() + frame.tools.len())
        .hash(&mut hash);
    // Illumination and camera coverage change path costs even for the same mesh.
    (frame.distance.to_bits(), frame.camera.fov_tan.to_bits()).hash(&mut hash);
    match frame.environment {
        pleochroma::optics::raytracer::EnvironmentSource::Studio {
            preset,
            exposure,
            light_yaw,
            light_pitch,
            backdrop,
            surface_glare,
            head_shadow_deg,
        } => {
            format!("{preset:?}").hash(&mut hash);
            [
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare,
                head_shadow_deg,
            ]
            .map(f32::to_bits)
            .hash(&mut hash);
        }
        pleochroma::optics::raytracer::EnvironmentSource::HdrMap(map) => {
            std::ptr::from_ref(map).hash(&mut hash);
        }
    }
    hash.finish()
}

/// A lane changes only after a 15% throughput improvement over the retained lane.
const BACKEND_HYSTERESIS: f64 = 1.15;

/// Rates are samples per full image per second; both lanes use the same resolution.
pub(in crate::bridge::render_thread) struct HybridPacing {
    cost_key: Option<u64>,
    gpu_rate: Option<f64>,
    cpu_rate: Option<f64>,
    gpu_dead: bool,
    cpu_preferred: bool,
    calibration: Option<super::calibration::Calibration>,
    costs: std::collections::VecDeque<(u64, Option<f64>, Option<f64>)>,
    cpu_scratch: Vec<Vec3>,
    scratch_depth: Vec<f32>,
    scratch_normal: Vec<Vec3>,
    scratch_facet: Vec<i32>,
}

impl HybridPacing {
    pub(in crate::bridge::render_thread) const fn new() -> Self {
        Self {
            cost_key: None,
            gpu_rate: None,
            cpu_rate: None,
            gpu_dead: false,
            cpu_preferred: false,
            calibration: None,
            costs: std::collections::VecDeque::new(),
            cpu_scratch: Vec::new(),
            scratch_depth: Vec::new(),
            scratch_normal: Vec::new(),
            scratch_facet: Vec::new(),
        }
    }

    /// Retain useful timings while orbiting, but never carry a cheap material's
    /// CPU/GPU split into a different resolution, optical material or bounce limit.
    pub(in crate::bridge::render_thread) fn prepare_frame(&mut self, frame: &BackendFrame<'_>) {
        let key = workload_key(frame);
        if self.cost_key != Some(key) {
            if let Some(previous) = self.cost_key {
                self.costs.retain(|(key, _, _)| *key != previous);
                self.costs
                    .push_front((previous, self.gpu_rate, self.cpu_rate));
                self.costs.truncate(8);
            }
            self.cost_key = Some(key);
            let cached = self.costs.iter().find(|(cached, _, _)| *cached == key);
            self.gpu_rate = cached.and_then(|(_, gpu, _)| *gpu);
            self.cpu_rate = cached.and_then(|(_, _, cpu)| *cpu);
            self.cpu_preferred = false;
            if let Some(calibration) = &self.calibration {
                calibration.cancel();
            }
            self.gpu_dead = false;
        }
    }

    fn gpu_share(&self, spp: u32) -> Option<u32> {
        if self.gpu_dead || spp < 2 {
            return None;
        }
        let (gpu, cpu) = (self.gpu_rate?, self.cpu_rate?);
        let fastest_alone = f64::from(spp) / gpu.max(cpu);
        // Whole-sample lanes have a join/merge cost. Require a measurable benefit;
        // a slow GPU should not drag a fast CPU into an unhelpful shared pass.
        (1..spp)
            .filter_map(|gpu_share| {
                let predicted =
                    (f64::from(gpu_share) / gpu).max(f64::from(spp - gpu_share) / cpu) + 0.005;
                (predicted < fastest_alone * 0.9).then_some((gpu_share, predicted))
            })
            .min_by(|(_, left), (_, right)| left.total_cmp(right))
            .map(|(share, _)| share)
    }

    pub(in crate::bridge::render_thread) fn prefer_cpu(&mut self) -> bool {
        if let (Some(gpu), Some(cpu)) = (self.gpu_rate, self.cpu_rate) {
            if cpu > gpu * BACKEND_HYSTERESIS {
                self.cpu_preferred = true;
            }
            if gpu > cpu * BACKEND_HYSTERESIS {
                self.cpu_preferred = false;
            }
        }
        self.cpu_preferred
    }

    fn poll_calibration(
        &mut self,
        frame: &BackendFrame<'_>,
        shared_hdr: Option<&std::sync::Arc<pleochroma::renderer::env_map::EnvironmentMap>>,
    ) {
        if self
            .calibration
            .as_ref()
            .is_some_and(super::calibration::Calibration::finished)
            && let Some(calibration) = self.calibration.take()
            && let Some((key, rate)) = calibration.finish()
            && self.cost_key == Some(key)
            && self.cpu_rate.is_none()
        {
            self.cpu_rate = Some(rate);
        }
        if self.cpu_rate.is_none()
            && self.calibration.is_none()
            && let Some(key) = self.cost_key
        {
            self.calibration = Some(super::calibration::Calibration::start(
                key, frame, shared_hdr,
            ));
        }
    }

    #[cfg(feature = "render-bench")]
    pub(in crate::bridge::render_thread) fn drain_background_calibration(&mut self) {
        if let Some(calibration) = self.calibration.take() {
            calibration.cancel();
            let _ = calibration.finish();
        }
    }

    fn prepare_scratch(&mut self, pixels: usize) {
        self.cpu_scratch.clear();
        self.cpu_scratch.resize(pixels, Vec3::ZERO);
        self.scratch_depth.resize(pixels, 1.0e6);
        self.scratch_normal.resize(pixels, Vec3::ZERO);
        self.scratch_facet.resize(pixels, -1);
    }

    pub(in crate::bridge::render_thread) fn observe_gpu_only(
        &mut self,
        spp: u32,
        elapsed: std::time::Duration,
    ) {
        Self::blend(&mut self.gpu_rate, spp, elapsed);
        self.gpu_dead = false;
    }

    pub(in crate::bridge::render_thread) fn observe_cpu_only(
        &mut self,
        spp: u32,
        elapsed: std::time::Duration,
    ) {
        Self::blend(&mut self.cpu_rate, spp, elapsed);
    }

    pub(in crate::bridge::render_thread) const fn needs_cpu_timing(&self) -> bool {
        self.cpu_rate.is_none()
    }

    fn blend(slot: &mut Option<f64>, spp: u32, elapsed: std::time::Duration) {
        if spp != 0 {
            let rate = f64::from(spp) / elapsed.as_secs_f64().max(1e-9);
            *slot = Some(slot.map_or(rate, |previous| previous.mul_add(0.7, rate * 0.3)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_single_sample_never_splits() {
        let mut pacing = HybridPacing::new();
        pacing.gpu_rate = Some(10.0);
        pacing.cpu_rate = Some(10.0);
        assert_eq!(pacing.gpu_share(1), None);
    }

    #[test]
    fn one_sample_can_choose_cpu_with_hysteresis() {
        let mut pacing = HybridPacing::new();
        pacing.gpu_rate = Some(1.0);
        pacing.cpu_rate = Some(2.0);
        assert!(pacing.prefer_cpu());
        pacing.cpu_rate = Some(0.95);
        assert!(
            pacing.prefer_cpu(),
            "small fluctuations retain the previous lane"
        );
        pacing.cpu_rate = Some(0.5);
        assert!(!pacing.prefer_cpu());
        assert_eq!(pacing.gpu_share(1), None);
    }

    #[test]
    fn slow_gpu_does_not_force_unhelpful_sharing() {
        let mut pacing = HybridPacing::new();
        pacing.gpu_rate = Some(1.0);
        pacing.cpu_rate = Some(20.0);
        assert_eq!(pacing.gpu_share(2), None);
        assert!(pacing.prefer_cpu());
    }

    #[test]
    fn a_slow_cpu_probe_does_not_force_a_full_cpu_pass() {
        let mut pacing = HybridPacing::new();
        pacing.gpu_rate = Some(20.0);
        pacing.cpu_rate = Some(1.0);
        assert_eq!(pacing.gpu_share(8), None);
        pacing.cpu_rate = Some(20.0);
        assert_eq!(pacing.gpu_share(8), Some(4));
    }
}
