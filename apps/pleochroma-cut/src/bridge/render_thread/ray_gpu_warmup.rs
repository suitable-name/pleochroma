//! At most one retained background Ray compiler/upload worker.
use super::{PreparedRay, frame_scene};
use crate::bridge::render_thread::gpu_backend::{BackendFrame, owned_frame::OwnedFrame};
use pleochroma::renderer::{
    env_map::EnvironmentMap,
    gpu_backend::{
        GpuAccumulate,
        whitted::{WhittedGpuBackend, WhittedGpuPrepared},
    },
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// How long a lost device is left alone before preparation is tried again. The backend
/// refuses recovery for its own 30 s cool-down (`gpu_backend::recovery::COOL_DOWN`,
/// private to the library), so an earlier attempt could only decline again.
const LOST_RETRY: Duration = Duration::from_secs(30);

/// Keeps a lost or poisoned device from being re-prepared on every frame, which would
/// either spawn a worker per frame or stall the viewport waiting for one.
#[derive(Default)]
struct RetryGate {
    retry_at: Option<Instant>,
}

impl RetryGate {
    fn arm(&mut self, now: Instant) {
        self.retry_at = Some(now + LOST_RETRY);
    }

    /// Whether preparation must still wait; reopens the gate once the deadline passed.
    fn cooling(&mut self, now: Instant) -> bool {
        match self.retry_at {
            Some(at) if now < at => true,
            Some(_) => {
                self.retry_at = None;
                false
            }
            None => false,
        }
    }
}

/// Records a declined upload. A healthy backend's decline is final for the key (the CPU
/// route serves it); a lost one is not, so it arms the retry gate instead and the CPU
/// route serves the frames until the deadline.
fn settle_decline(gate: &mut RetryGate, key_declined: &mut bool, lost: bool, now: Instant) {
    if lost {
        gate.arm(now);
    } else {
        *key_declined = true;
    }
}

type Key = (u64, u32, u32);
struct Prepared {
    key: Key,
    backend: WhittedGpuBackend,
    upload: Result<WhittedGpuPrepared, GpuAccumulate>,
}
struct Pending {
    key: Key,
    cancel: Arc<AtomicBool>,
    task: JoinHandle<Prepared>,
}

#[derive(Default)]
pub(super) struct RayGpu {
    backend: Option<WhittedGpuBackend>,
    pending: Option<Pending>,
    retired: bool,
    retry: RetryGate,
}
impl Drop for RayGpu {
    fn drop(&mut self) {
        if let Some(pending) = &self.pending {
            pending.cancel.store(true, Ordering::Release);
        }
    }
}
impl RayGpu {
    /// Returns false while preparing, and keeps a compiled backend after a stale
    /// scene is cancelled. No worker owns any live output buffers. Returns true with
    /// `prepared.gpu == None` when the CPU route must serve the frame: a declined key,
    /// a retired worker, or a lost device waiting out its retry deadline.
    pub(super) fn poll(
        &mut self,
        prepared: &mut PreparedRay,
        frame: &BackendFrame<'_>,
        hdr: Option<&Arc<EnvironmentMap>>,
    ) -> bool {
        if self.retired || prepared.gpu.is_some() || prepared.gpu_declined {
            return true;
        }
        if let Some(pending) = &self.pending {
            if pending.key != prepared.key {
                pending.cancel.store(true, Ordering::Release);
            }
            if !pending.task.is_finished() {
                return false;
            }
        }
        if let Some(pending) = self.pending.take() {
            if let Ok(result) = pending.task.join() {
                if result.key == prepared.key {
                    match result.upload {
                        Ok(upload) => prepared.gpu = Some(upload),
                        Err(GpuAccumulate::Declined) => settle_decline(
                            &mut self.retry,
                            &mut prepared.gpu_declined,
                            result.backend.is_lost(),
                            Instant::now(),
                        ),
                        Err(_) => {}
                    }
                }
                self.backend = Some(result.backend);
            } else {
                self.retired = true;
                return true;
            }
        }
        if prepared.gpu.is_some() || prepared.gpu_declined || self.retry.cooling(Instant::now()) {
            return true;
        }
        let owned = OwnedFrame::new(frame, hdr);
        let prep = Arc::clone(&prepared.prep);
        let key = prepared.key;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        // A poisoned backend never serves again; the retry starts from a fresh device.
        let backend = self.backend.take().filter(|backend| !backend.is_poisoned());
        let task = std::thread::spawn(move || {
            let backend = backend.unwrap_or_else(|| {
                #[cfg(feature = "ui-harness")]
                if crate::harness_support::viewport_rendering_disabled() {
                    return WhittedGpuBackend::disabled();
                }
                WhittedGpuBackend::acquire()
            });
            let frame = owned.as_frame();
            let upload =
                backend.prepare_cancellable(&frame_scene(&frame), frame.tools, &prep, &|| {
                    worker_cancel.load(Ordering::Acquire)
                });
            Prepared {
                key,
                backend,
                upload,
            }
        });
        self.pending = Some(Pending { key, cancel, task });
        false
    }

    #[cfg(feature = "render-bench")]
    pub(super) fn drain_background_preparation(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.cancel.store(true, Ordering::Release);
            match pending.task.join() {
                Ok(result) => self.backend = Some(result.backend),
                Err(_) => self.retired = true,
            }
        }
    }

    pub(super) const fn backend(&self) -> Option<&WhittedGpuBackend> {
        self.backend.as_ref()
    }
    pub(super) const fn retired(&self) -> bool {
        self.retired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::{
        geometry::cuts::StandardGemCuts,
        optics::{
            fluorescence::Fluorescence,
            materials::GemMaterial,
            raytracer::{
                Camera, DEFAULT_FOV_DEG, DEFAULT_POSE, LightingPreset, build_plane_soa,
                surface::SurfaceScene,
            },
        },
        renderer::{engine::RayOptions, whitted::WhittedPrep},
    };

    fn at(gate: &RetryGate, seconds: u64) -> Instant {
        gate.retry_at.expect("armed") + Duration::from_secs(seconds)
    }

    #[test]
    fn a_lost_decline_arms_the_gate_and_a_healthy_one_is_final_for_the_key() {
        let now = Instant::now();
        let mut gate = RetryGate::default();
        let mut declined = false;
        settle_decline(&mut gate, &mut declined, false, now);
        assert!(declined, "a healthy backend's decline stays with the key");
        assert!(!gate.cooling(now));

        declined = false;
        settle_decline(&mut gate, &mut declined, true, now);
        assert!(!declined, "a lost device is retried, not written off");
        assert!(gate.cooling(now), "the CPU route serves until the deadline");
        let just_before = (now + LOST_RETRY)
            .checked_sub(Duration::from_millis(1))
            .expect("the deadline is far from the clock origin");
        assert!(gate.cooling(just_before));
    }

    #[test]
    fn the_gate_reopens_once_after_the_deadline() {
        let mut gate = RetryGate::default();
        gate.arm(Instant::now());
        let after = at(&gate, 1);
        assert!(!gate.cooling(after));
        assert!(
            gate.retry_at.is_none(),
            "one retry per loss, not a standing bypass"
        );
        assert!(!gate.cooling(after));
    }

    fn frame_parts() -> (
        Camera,
        Vec<pleochroma::geometry::plane::GpuFacetPlane>,
        GemMaterial,
    ) {
        (
            Camera::new(
                DEFAULT_POSE.yaw,
                DEFAULT_POSE.pitch,
                DEFAULT_POSE.distance,
                DEFAULT_FOV_DEG,
            ),
            StandardGemCuts::standard_round_brilliant(),
            GemMaterial::diamond(),
        )
    }

    fn frame<'a>(
        camera: &'a Camera,
        planes: &'a [pleochroma::geometry::plane::GpuFacetPlane],
        material: &'a GemMaterial,
    ) -> BackendFrame<'a> {
        BackendFrame {
            width: 4,
            height: 4,
            yaw: DEFAULT_POSE.yaw,
            pitch: DEFAULT_POSE.pitch,
            distance: DEFAULT_POSE.distance,
            camera,
            planes,
            tools: &[],
            fluorescence: Fluorescence::none(),
            facet_finishes: &[],
            surface: SurfaceScene::IDEAL,
            material,
            max_bounces: 2,
            environment: LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            spp: 1,
            sample_offset: 0,
        }
    }

    fn prepared(frame: &BackendFrame<'_>) -> PreparedRay {
        let prep = WhittedPrep::new(
            &frame_scene(frame),
            frame.fluorescence,
            RayOptions::default(),
        )
        .expect("a plain scene")
        .into_owned_with_hdr(None);
        PreparedRay {
            key: (1, frame.width, frame.height),
            prep: Arc::new(prep),
            planes: build_plane_soa(frame.planes),
            gpu: None,
            gpu_declined: false,
        }
    }

    /// A lost device must hand the frame to the CPU route at once: `poll` returns true
    /// without a worker, so Local Compute = GPU never spins waiting for one.
    #[test]
    fn a_lost_device_waiting_out_its_deadline_is_ready_without_a_worker() {
        let (camera, planes, material) = frame_parts();
        let frame = frame(&camera, &planes, &material);
        let mut prepared = prepared(&frame);
        let mut gpu = RayGpu::default();
        gpu.retry.arm(Instant::now());
        assert!(gpu.poll(&mut prepared, &frame, None));
        assert!(gpu.pending.is_none(), "no worker per frame while lost");
        assert!(prepared.gpu.is_none());
        assert!(
            !prepared.gpu_declined,
            "the key is retried after the deadline"
        );
    }

    #[test]
    fn preparation_resumes_after_the_deadline_and_settles_a_healthy_decline() {
        let (camera, planes, material) = frame_parts();
        let frame = frame(&camera, &planes, &material);
        let mut prepared = prepared(&frame);
        let mut gpu = RayGpu::default();
        // A disabled backend declines without touching a GPU and is not "lost".
        gpu.backend = Some(WhittedGpuBackend::disabled());
        gpu.retry.retry_at = Instant::now().checked_sub(Duration::from_secs(1));
        assert!(gpu.retry.retry_at.is_some());
        assert!(
            !gpu.poll(&mut prepared, &frame, None),
            "the retry starts a worker"
        );
        assert!(gpu.pending.is_some());
        let deadline = Instant::now() + Duration::from_secs(30);
        while !gpu.poll(&mut prepared, &frame, None) {
            assert!(Instant::now() < deadline, "the worker must finish");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(prepared.gpu.is_none());
        assert!(prepared.gpu_declined);
        assert!(gpu.retry.retry_at.is_none());
    }
}
