//! Feature-only wall-clock observations of the existing live render path.

use std::time::{Duration, Instant};

/// Routes describe radiance execution; GPU frames may still use CPU guides.
#[derive(Clone, serde::Serialize)]
pub(in crate::bridge::render_thread) struct BenchmarkObservation {
    pub route: &'static str,
    pub fallback: bool,
    pub cancelled: bool,
    pub reason: Option<String>,
    pub adapter_label: Option<String>,
    /// Inclusive wall times. Overlapping phases must not be added together.
    pub phases: Vec<BenchmarkPhase>,
}

impl Default for BenchmarkObservation {
    fn default() -> Self {
        Self {
            route: "none",
            fallback: false,
            cancelled: false,
            reason: None,
            adapter_label: None,
            phases: Vec::new(),
        }
    }
}

impl BenchmarkObservation {
    pub fn fallback(&mut self, reason: impl Into<String>) {
        self.fallback = true;
        self.reason = Some(reason.into());
    }
}

#[derive(Clone, serde::Serialize)]
pub(in crate::bridge::render_thread) struct BenchmarkPhase {
    pub name: &'static str,
    pub elapsed_ms: f64,
    pub overlaps: bool,
}

pub(in crate::bridge::render_thread) struct PhaseTimer {
    name: &'static str,
    start: Instant,
    overlaps: bool,
}

impl PhaseTimer {
    pub fn start(name: &'static str, overlaps: bool) -> Self {
        tracing::info!(target: "pleochroma_cut::render_bench", phase = name, overlaps, "phase_begin");
        Self {
            name,
            start: Instant::now(),
            overlaps,
        }
    }

    pub fn finish(self, observation: &mut BenchmarkObservation) {
        observation.phases.push(Self::completed(
            self.name,
            self.start.elapsed(),
            self.overlaps,
        ));
    }

    pub fn completed(name: &'static str, elapsed: Duration, overlaps: bool) -> BenchmarkPhase {
        let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
        tracing::info!(target: "pleochroma_cut::render_bench", phase = name, elapsed_ms, overlaps, "phase_end");
        BenchmarkPhase {
            name,
            elapsed_ms,
            overlaps,
        }
    }
}
