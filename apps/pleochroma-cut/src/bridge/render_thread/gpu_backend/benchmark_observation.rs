//! Feature-only diagnostics; ordinary GPU refusals need not change the UI banner.

use super::{BenchmarkObservation, ViewportGpu};

impl ViewportGpu {
    pub(in crate::bridge::render_thread) const fn benchmark_observation(
        &self,
    ) -> &BenchmarkObservation {
        &self.observation
    }

    pub(super) fn note_benchmark_gpu_decline(&mut self) {
        self.observation.adapter_label = self.gpu.adapter_label();
        let reason = if let Some(reason) = self.status_message() {
            reason.to_owned()
        } else if self.gpu_retired {
            "Path GPU retired after a render thread panic".to_owned()
        } else if self.observation.adapter_label.is_none() {
            "Path GPU unavailable or disabled".to_owned()
        } else {
            "Path GPU declined the request; detailed reason unavailable".to_owned()
        };
        self.observation.fallback(reason);
    }
}
