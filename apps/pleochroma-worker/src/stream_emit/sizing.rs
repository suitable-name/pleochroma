//! Adaptive sub-batch sizing: [`next_batch_size`] adapts the tracer's sub-batch sample
//! count toward a target duration ([`TARGET_SUBBATCH`], or [`TARGET_SUBBATCH_FINAL_ONLY`] for
//! a request that streams no intermediate frames), given how long the previous sub-batch
//! actually took.
//!
//! # Why two targets
//!
//! Every sub-batch ends with the whole frame read back from the GPU and added into the
//! pending delta, while that request's GPU work waits. A streamed request needs fresh deltas
//! at its cadence, so it keeps short sub-batches. A `FinalOnly` request (every chunk a
//! coordinator sends a joined worker, and the coordinator's own lane) sends its radiance once,
//! at the end, so short sub-batches only add readbacks: at export sizes they idled the GPU for
//! a quarter of the time. Cancellation does not depend on the target: the GPU dispatch checks
//! the cancel flag mid-dispatch.
//!
//! # Carrying the converged size across jobs
//!
//! Each job's ramp starts at `1` sample and grows by at most 4x per step toward the size
//! that fills its target on this machine. Jobs are frequent (one per request, lane chunk, or
//! live request), and the target size depends only on this process's resource profile and
//! the target duration, so [`seeded_batch_size`]/[`remember_converged_subbatch`] keep the
//! last converged size per [`crate::render_core::hybrid::JobKey`] (the key
//! `calibrate_cached` also uses) and target. A new job then starts near steady state instead
//! of at `1`.

use crate::render_core::hybrid::JobKey;
use pleochroma_net::messages::TransferMode;
use std::{
    collections::BTreeMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

/// The wall-clock duration [`next_batch_size`] adapts a streamed request's sub-batches
/// toward: short, so each cadence tick has fresh samples to send, on any hardware.
pub(super) const TARGET_SUBBATCH: Duration = Duration::from_millis(100);

/// The sub-batch target of a request that streams no intermediate frames (see the module
/// doc comment's "Why two targets").
pub(super) const TARGET_SUBBATCH_FINAL_ONLY: Duration = Duration::from_millis(500);

/// The sub-batch target for a request sent back with `mode`.
#[must_use]
pub(super) const fn subbatch_target(mode: TransferMode) -> Duration {
    match mode {
        TransferMode::FinalOnly => TARGET_SUBBATCH_FINAL_ONLY,
        TransferMode::LiveProgressive | TransferMode::DisplayOnly => TARGET_SUBBATCH,
    }
}

/// Absolute ceiling on a single sub-batch, whatever timing [`next_batch_size`] is given.
/// The 4x-per-step clamp alone compounds when timings keep landing well under the target,
/// for example when dispatch overhead dominates. The cap stays far below
/// `crate::validate::MAX_SAMPLES_PER_REQUEST`, so it binds before one sub-batch takes a
/// whole request.
pub(super) const MAX_SUBBATCH: u32 = 2048;

/// Adapts the next sub-batch's sample count toward `target`, given how long `prev` samples
/// actually took to trace. Grows (up to 4x, never past [`MAX_SUBBATCH`]) when the previous
/// batch finished well under budget, shrinks toward 1 when it ran over, and never returns
/// 0 -- converges on a sub-batch size that fits the worker's own actual throughput rather
/// than a single hardcoded sample count.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "value clamped to 1.0..=max_growth (at most MAX_SUBBATCH) before the cast"
)]
pub(super) fn next_batch_size(prev: u32, elapsed: Duration, target: Duration) -> u32 {
    if elapsed.is_zero() {
        return prev.saturating_mul(4).clamp(1, MAX_SUBBATCH);
    }
    let ratio = target.as_secs_f64() / elapsed.as_secs_f64();
    let scaled = f64::from(prev) * ratio;
    let max_growth = f64::from(prev.saturating_mul(4).clamp(1, MAX_SUBBATCH));
    scaled.clamp(1.0, max_growth).round() as u32
}

/// Process-wide cache of the last converged sub-batch size for a given [`JobKey`] and
/// target duration -- see this module's top doc comment. A `BTreeMap`, not a hash map,
/// purely to keep iteration/debug output deterministic; lookups are by exact key equality
/// either way.
static CONVERGED_SUBBATCH: LazyLock<Mutex<BTreeMap<(JobKey, Duration), u32>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// The size a job's adaptive ramp toward `target` should start from for `key`: `1` (the
/// original cold-start value) if this resource profile has never converged on that target
/// before, otherwise the last size [`remember_converged_subbatch`] recorded for it.
#[must_use]
pub(super) fn seeded_batch_size(key: JobKey, target: Duration) -> u32 {
    CONVERGED_SUBBATCH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&(key, target))
        .copied()
        .unwrap_or(1)
}

/// Records `size` -- the sub-batch size a job's adaptive ramp toward `target` last reached,
/// whether the job finished, was cancelled, or is simply between sub-batches -- as the new
/// seed for `key` and `target`, so the next job/chunk with the same resource profile and
/// target starts near steady-state instead of ramping from `1` again.
pub(super) fn remember_converged_subbatch(key: JobKey, target: Duration, size: u32) {
    CONVERGED_SUBBATCH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert((key, target), size);
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
            camera_pan: [0.0, 0.0],
        }
    }

    #[test]
    fn seeded_batch_size_defaults_to_one_before_anything_is_recorded() {
        let gpu = GpuBackend::disabled();
        let key = job_key(&gpu, &tiny_scene(), 11, ComputeMode::Hybrid);
        assert_eq!(seeded_batch_size(key, TARGET_SUBBATCH), 1);
    }

    #[test]
    fn remember_then_seed_round_trips_the_converged_size() {
        let gpu = GpuBackend::disabled();
        let key = job_key(&gpu, &tiny_scene(), 12, ComputeMode::Hybrid);
        remember_converged_subbatch(key, TARGET_SUBBATCH, 512);
        assert_eq!(seeded_batch_size(key, TARGET_SUBBATCH), 512);

        // A later job for the SAME key starts where this one left off, not at 1.
        remember_converged_subbatch(key, TARGET_SUBBATCH, 640);
        assert_eq!(seeded_batch_size(key, TARGET_SUBBATCH), 640);
    }

    #[test]
    fn different_keys_never_share_a_converged_size() {
        let gpu = GpuBackend::disabled();
        let scene = tiny_scene();
        let a = job_key(&gpu, &scene, 13, ComputeMode::Hybrid);
        let b = job_key(&gpu, &scene, 14, ComputeMode::Hybrid);
        remember_converged_subbatch(a, TARGET_SUBBATCH, 900);
        assert_eq!(
            seeded_batch_size(b, TARGET_SUBBATCH),
            1,
            "an unrelated key must not see `a`'s size"
        );
        assert_eq!(seeded_batch_size(a, TARGET_SUBBATCH), 900);
        assert_eq!(
            seeded_batch_size(a, TARGET_SUBBATCH_FINAL_ONLY),
            1,
            "a converged size belongs to its own target duration"
        );
    }

    /// A request that sends its radiance once takes the long target; a streamed one keeps
    /// the short one, so every cadence tick has fresh samples.
    #[test]
    fn only_a_final_only_request_takes_the_long_sub_batch_target() {
        assert_eq!(
            subbatch_target(TransferMode::FinalOnly),
            TARGET_SUBBATCH_FINAL_ONLY
        );
        assert_eq!(
            subbatch_target(TransferMode::LiveProgressive),
            TARGET_SUBBATCH
        );
        assert_eq!(subbatch_target(TransferMode::DisplayOnly), TARGET_SUBBATCH);
    }
}
