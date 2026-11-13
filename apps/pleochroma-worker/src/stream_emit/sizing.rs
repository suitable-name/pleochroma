//! Adaptive sub-batch sizing: [`next_batch_size`] adapts the tracer's sub-batch sample
//! count toward [`TARGET_SUBBATCH`], given how long the previous sub-batch actually took.
//!
//! # Carrying the converged size across jobs
//!
//! Each job's ramp starts at `1` sample and grows by at most 4x per step toward the size
//! that fills [`TARGET_SUBBATCH`] on this machine. Jobs are frequent (one per request, lane
//! chunk, or live request), and the target size depends only on this process's resource
//! profile, so [`seeded_batch_size`]/[`remember_converged_subbatch`] keep the last converged
//! size per [`crate::render_core::hybrid::JobKey`], the key `calibrate_cached` also uses. A
//! new job then starts near steady state instead of at `1`.

use crate::render_core::hybrid::JobKey;
use std::{
    collections::BTreeMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

/// The wall-clock duration [`next_batch_size`] adapts sub-batch sizes toward. It bounds
/// cancellation latency (the tracer checks its cancel flag only between sub-batches) on
/// any hardware, so no fixed sample count is needed.
pub(super) const TARGET_SUBBATCH: Duration = Duration::from_millis(100);

/// Absolute ceiling on a single sub-batch, whatever timing [`next_batch_size`] is given.
/// The 4x-per-step clamp alone compounds when timings keep landing well under
/// [`TARGET_SUBBATCH`], for example when dispatch overhead dominates. The cap stays far
/// below `crate::validate::MAX_SAMPLES_PER_REQUEST`, so it binds before one sub-batch
/// takes a whole request.
pub(super) const MAX_SUBBATCH: u32 = 2048;

/// Adapts the next sub-batch's sample count toward [`TARGET_SUBBATCH`], given how long
/// `prev` samples actually took to trace. Grows (up to 4x, never past [`MAX_SUBBATCH`])
/// when the previous batch finished well under budget, shrinks toward 1 when it ran
/// over, and never returns 0 -- converges on a sub-batch size that fits the worker's
/// own actual throughput rather than a single hardcoded sample count.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "value clamped to 1.0..=max_growth (at most MAX_SUBBATCH) before the cast"
)]
pub(super) fn next_batch_size(prev: u32, elapsed: Duration) -> u32 {
    if elapsed.is_zero() {
        return prev.saturating_mul(4).clamp(1, MAX_SUBBATCH);
    }
    let ratio = TARGET_SUBBATCH.as_secs_f64() / elapsed.as_secs_f64();
    let scaled = f64::from(prev) * ratio;
    let max_growth = f64::from(prev.saturating_mul(4).clamp(1, MAX_SUBBATCH));
    scaled.clamp(1.0, max_growth).round() as u32
}

/// Process-wide cache of the last converged sub-batch size for a given
/// [`JobKey`] -- see this module's top doc comment. A `BTreeMap`, not a hash map, purely
/// to keep iteration/debug output deterministic; lookups are by exact key equality
/// either way.
static CONVERGED_SUBBATCH: LazyLock<Mutex<BTreeMap<JobKey, u32>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// The size a job's adaptive ramp should start from for `key`: `1` (the original
/// cold-start value) if this resource profile has never converged before, otherwise the
/// last size [`remember_converged_subbatch`] recorded for it.
#[must_use]
pub(super) fn seeded_batch_size(key: JobKey) -> u32 {
    CONVERGED_SUBBATCH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .copied()
        .unwrap_or(1)
}

/// Records `size` -- the sub-batch size a job's adaptive ramp last reached, whether the
/// job finished, was cancelled, or is simply between sub-batches -- as `key`'s new seed,
/// so the next job/chunk with the same resource profile starts near steady-state instead
/// of ramping from `1` again.
pub(super) fn remember_converged_subbatch(key: JobKey, size: u32) {
    CONVERGED_SUBBATCH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, size);
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::{cli::ComputeMode, render_core::hybrid::job_key};
    use pleochroma::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
        renderer::gpu_backend::GpuBackend,
    };
    use pleochroma_net::SceneState;

    /// A minimal valid scene -- this module only needs `job_key` to produce distinct,
    /// real keys (varying `threads` below), not a scene that traces to anything
    /// meaningful.
    fn tiny_scene() -> SceneState {
        SceneState {
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            width: 8,
            height: 8,
            yaw: 0.4,
            camera_roll: 0.0,
            stone_shadow: false,
            tray_reflection: 0.0,
            hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
            estimator: pleochroma::optics::raytracer::Estimator::Branching,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
            backdrop: 0.0,
            environment: pleochroma_net::scene::SceneEnvironment::Studio,
            surface_glare: 1.0,
            tools: Vec::new(),
            fluorescence: pleochroma::optics::fluorescence::Fluorescence::default(),
            head_shadow_deg: 16.0,
            frosted_planes: Vec::new(),
            effects: pleochroma_net::scene::effects::SceneEffects::default(),
        }
    }

    #[test]
    fn seeded_batch_size_defaults_to_one_before_anything_is_recorded() {
        let gpu = GpuBackend::disabled();
        let key = job_key(&gpu, &tiny_scene(), 11, ComputeMode::Hybrid);
        assert_eq!(seeded_batch_size(key), 1);
    }

    #[test]
    fn remember_then_seed_round_trips_the_converged_size() {
        let gpu = GpuBackend::disabled();
        let key = job_key(&gpu, &tiny_scene(), 12, ComputeMode::Hybrid);
        remember_converged_subbatch(key, 512);
        assert_eq!(seeded_batch_size(key), 512);

        // A later job for the SAME key starts where this one left off, not at 1.
        remember_converged_subbatch(key, 640);
        assert_eq!(seeded_batch_size(key), 640);
    }

    #[test]
    fn different_keys_never_share_a_converged_size() {
        let gpu = GpuBackend::disabled();
        let scene = tiny_scene();
        let a = job_key(&gpu, &scene, 13, ComputeMode::Hybrid);
        let b = job_key(&gpu, &scene, 14, ComputeMode::Hybrid);
        remember_converged_subbatch(a, 900);
        assert_eq!(
            seeded_batch_size(b),
            1,
            "an unrelated key must not see `a`'s size"
        );
        assert_eq!(seeded_batch_size(a), 900);
    }
}
