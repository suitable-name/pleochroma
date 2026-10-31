//! One retained preparation worker; no radiance enters the live accumulator.
use super::{BackendFrame, GpuBackend, GpuSceneRef, ViewportGpu, owned_frame::OwnedFrame};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

#[derive(Default)]
pub(super) struct PathWarmup {
    started: bool,
    task: Option<JoinHandle<(u64, GpuBackend)>>,
    prepared_key: Option<u64>,
    cancel: Arc<AtomicBool>,
}

impl PathWarmup {
    pub(super) fn new(already_acquired: bool) -> Self {
        Self {
            started: already_acquired,
            task: None,
            prepared_key: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Drop for PathWarmup {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl ViewportGpu {
    /// Starts lazily, so selecting CPU creates neither a GPU device nor a compiler.
    pub(super) fn poll_preparation(&mut self, frame: &BackendFrame<'_>) -> bool {
        // The key ignores the pass size, so a ramp step never re-prepares.
        let key = super::dispatch::workload_key(frame);
        if self.gpu_retired {
            return true;
        }
        if self
            .warmup
            .task
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
            && let Some(task) = self.warmup.task.take()
        {
            if let Ok((prepared_key, gpu)) = task.join() {
                self.gpu = gpu;
                self.warmup.prepared_key = Some(prepared_key);
                self.status_message = None;
                if self.gpu.is_lost() {
                    self.note_device_loss();
                }
            } else {
                self.retire();
                return true;
            }
        }
        if self.warmup.task.is_some() {
            return false;
        }
        if self.warmup.prepared_key == Some(key) {
            return true;
        }
        let acquire = !self.warmup.started;
        self.warmup.started = true;
        let owned = OwnedFrame::new(frame, self.shared_hdr.as_ref());
        let cancel = Arc::clone(&self.warmup.cancel);
        let retained = std::mem::replace(&mut self.gpu, GpuBackend::disabled());
        self.status_message = Some(super::PREPARING_STATUS.to_owned());
        self.ray_status_active = false;
        self.warmup.task = Some(std::thread::spawn(move || {
            #[cfg(feature = "ui-harness")]
            if crate::harness_support::viewport_rendering_disabled() {
                return (key, GpuBackend::disabled());
            }
            let gpu = if acquire {
                GpuBackend::acquire()
            } else {
                retained
            };
            if !cancel.load(Ordering::Acquire) {
                let frame = owned.as_frame();
                let scene = GpuSceneRef {
                    camera: frame.camera,
                    width: frame.width,
                    height: frame.height,
                    planes: frame.planes,
                    facet_finishes: frame.facet_finishes,
                    material: frame.material,
                    max_bounces: frame.max_bounces,
                    environment: frame.environment,
                };
                let _ = gpu.prepare_geom_surface_cancellable(
                    &scene,
                    frame.tools,
                    &frame.surface,
                    &cancel,
                );
            }
            (key, gpu)
        }));
        false
    }

    /// Benchmark case boundaries must not overlap detached compiler/probe work.
    #[cfg(feature = "render-bench")]
    pub(in crate::bridge::render_thread) fn drain_background_preparation(
        &mut self,
        pacing: &mut super::HybridPacing,
    ) {
        self.warmup.cancel.store(true, Ordering::Release);
        if let Some(task) = self.warmup.task.take() {
            match task.join() {
                Ok((key, gpu)) => {
                    self.gpu = gpu;
                    self.warmup.prepared_key = Some(key);
                }
                Err(_) => self.retire(),
            }
        }
        self.warmup.cancel.store(false, Ordering::Release);
        pacing.drain_background_calibration();
    }

    pub(in crate::bridge::render_thread) fn set_shared_hdr(
        &mut self,
        hdr: Option<Arc<pleochroma::renderer::env_map::EnvironmentMap>>,
    ) {
        self.shared_hdr = hdr;
    }

    pub(super) fn wait_preparation(
        &mut self,
        frame: &BackendFrame<'_>,
        cancel: &AtomicBool,
    ) -> bool {
        while !self.poll_preparation(frame) {
            if cancel.load(Ordering::Acquire) {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        !cancel.load(Ordering::Acquire)
    }
}
