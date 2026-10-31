//! Whole-pass Ray lane selection; measured useful work, including one-tap passes.
use super::{BackendFrame, HybridPacing, WhittedPrep};
use std::time::Duration;

type Options = (u32, u8, u8, [u32; 3]);
pub(super) struct RayPacing {
    pacing: HybridPacing,
    options: Option<Options>,
}
impl Default for RayPacing {
    fn default() -> Self {
        Self {
            pacing: HybridPacing::new(),
            options: None,
        }
    }
}
impl RayPacing {
    pub(super) fn prepare(&mut self, frame: &BackendFrame<'_>, prep: &WhittedPrep<'_>) {
        let options = (
            prep.max_depth,
            prep.taps,
            prep.chain_count,
            prep.chains.map(|chain| chain.n.to_bits()),
        );
        if self.options != Some(options) {
            self.pacing = HybridPacing::new();
            self.options = Some(options);
        }
        self.pacing.prepare_frame(frame);
    }
    pub(super) fn cpu_wanted(&mut self) -> bool {
        self.pacing.needs_cpu_timing() || self.pacing.prefer_cpu()
    }
    pub(super) fn observe_cpu(&mut self, spp: u32, elapsed: Duration) {
        self.pacing.observe_cpu_only(spp, elapsed);
    }
    pub(super) fn observe_gpu(&mut self, spp: u32, elapsed: Duration) {
        self.pacing.observe_gpu_only(spp, elapsed);
    }
}
