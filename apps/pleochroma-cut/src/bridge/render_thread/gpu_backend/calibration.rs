//! A bounded diagnostic probe, independent of the display's first image.
use super::{BackendFrame, owned_frame::OwnedFrame};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

pub(super) struct Calibration {
    task: Option<JoinHandle<Option<(u64, f64)>>>,
    cancel: Arc<AtomicBool>,
}
impl Calibration {
    pub(super) fn start(
        key: u64,
        frame: &BackendFrame<'_>,
        shared_hdr: Option<&Arc<pleochroma::renderer::env_map::EnvironmentMap>>,
    ) -> Self {
        let owned = OwnedFrame::new(frame, shared_hdr);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let task = std::thread::spawn(move || {
            let _lease = pleochroma::renderer::tonemap::live_cpu_budget::single_core_lease();
            let _trace_lease = pleochroma::renderer::tonemap::live_cpu_budget::trace_lease();
            super::super::scanline::measure_cpu_rate(&owned.as_frame(), &worker_cancel)
                .map(|rate| (key, rate))
        });
        Self {
            task: Some(task),
            cancel,
        }
    }
    pub(super) fn finished(&self) -> bool {
        self.task.as_ref().is_some_and(JoinHandle::is_finished)
    }
    pub(super) fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub(super) fn finish(mut self) -> Option<(u64, f64)> {
        self.task.take()?.join().ok().flatten()
    }
}

impl Drop for Calibration {
    fn drop(&mut self) {
        self.cancel();
    }
}
