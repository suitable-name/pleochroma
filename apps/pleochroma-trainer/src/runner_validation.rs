//! Fixed-cohort evaluation and cached validation references.

use super::eval::OracleKey;
use super::*;
use std::path::Path;

impl TrainingRunner {
    /// Identity of the cohort, physics, and evaluation settings used for model selection.
    #[must_use]
    pub fn validation_identity(&self, count: usize) -> String {
        let key = OracleKey::new(
            &self.cfg,
            &self.validation_entries(count),
            self.cfg.seed,
            self.max_step_deg(),
        )
        .with_environment(self.execution.is_some());
        format!("{}:{key:?}", self.device_label)
    }
    /// Fixed coverage-oriented cohort from validation only. The same stones every pass;
    /// its stratified mean is not a population-unbiased generalization estimate.
    #[must_use]
    pub fn validation_entries(&self, count: usize) -> Vec<usize> {
        self.split.validation_cohort(count)
    }

    /// Plays one model episode per entry (the policy mean, projected, stepped with the run's
    /// line search) and averages the results; `passes` adds one stochastic episode per design
    /// ([`EvalReport::mean_improvement_sampled`]) and/or one finite-difference oracle episode
    /// ([`EvalReport::mean_improvement_oracle`]). Retarget targets and objective weights are
    /// drawn from `seed` and the entry, so every pass scores the same stone against the same
    /// target under the same weights.
    ///
    /// The designs are handed one at a time to a pool of up to `available_parallelism` scoped
    /// threads. Every outcome lands in the slot of its entry and the reduction runs serially
    /// in entry order afterwards. GPU/CUDA policy requests share one bounded batching worker
    /// on the resolved training backend, initialized once from the last installed network.
    /// CPU evaluation uses the pure-Rust reference. Native CUDA also accelerates attached
    /// environment geometry/scoring; the oracle search stays on the bounded CPU pool.
    /// Floating point policy values can differ across backends and batch shapes.
    ///
    /// It plays the network as of the last installed update: [`Self::validate`] and
    /// [`Self::sync_update`] install the update in flight first, a bare `evaluate` (which only
    /// borrows the runner) does not.
    ///
    /// # Errors
    ///
    /// [`RunnerError`] for a material or network failure (the first one in entry order); a
    /// design whose environment cannot be built is counted in [`EvalReport::failed`] instead.
    pub fn evaluate(
        &self,
        entries: &[usize],
        fidelity: EvalFidelity,
        seed: u64,
        passes: EvalPasses,
    ) -> Result<EvalReport, RunnerError> {
        self.evaluate_with_recovery(entries, fidelity, seed, passes, None)
    }

    fn evaluate_with_recovery(
        &self,
        entries: &[usize],
        fidelity: EvalFidelity,
        seed: u64,
        passes: EvalPasses,
        directory: Option<&Path>,
    ) -> Result<EvalReport, RunnerError> {
        eval::evaluate_designs(
            eval::EvalJob {
                net: &self.net,
                backend: self.device_label,
                inference: None,
                execution: self.execution.as_ref(),
                split: &self.split,
                cfg: &self.cfg,
                fidelity,
                seed,
                passes,
                progress: &self.progress,
            },
            entries,
            directory,
        )
    }

    /// Mean score drop over the recent training episodes minus the validation drop; `None` until
    /// both exist, fidelities match and validation covered at least [`MIN_GAP_DESIGNS`] families.
    ///
    /// Like with like: the training drop comes from episodes that draw their actions, so it is
    /// compared with the drop of the sampled validation episodes
    /// ([`EvalReport::mean_improvement_sampled`]), which carry the same exploration noise. A
    /// report without a sampled pass falls back to the deterministic drop, and the gap then also
    /// contains the exploration-noise effect. Different training and validation fidelities are
    /// not comparable and produce no gap.
    #[must_use]
    pub fn generalization_gap(&self) -> Option<f32> {
        if !matches!(
            (self.cfg.train_fidelity, self.cfg.validation_fidelity),
            (ObjectiveFidelity::Fast, EvalFidelity::Fast)
                | (ObjectiveFidelity::Full, EvalFidelity::Full)
        ) {
            return None;
        }
        if self.cfg.train_fidelity == ObjectiveFidelity::Fast && self.cfg.full_fidelity_frac > 0.0 {
            return None; // A mixture of Fast/Full training drops is not a matched estimator.
        }
        let train = self.stats.mean_improvement()?;
        let val = self.stats.last_validation.as_ref()?;
        let val_drop = val.mean_improvement_sampled.unwrap_or(val.mean_improvement);
        (val.distinct_families >= MIN_GAP_DESIGNS && val.failed == 0).then_some(train - val_drop)
    }

    /// Validates on the fixed subset of `count` held-out designs at configured validation fidelity
    /// (callers pass [`RunConfig::validation_designs`]) and remembers the result as
    /// [`RunnerStats::last_validation`]. Each design is played by the model and sampled
    /// exploration policy. Fast validation also computes a teacher reference once; Full
    /// validation omits that expensive reference. The retarget target and the objective
    /// weights of each design are fixed by the run seed, so passes differ only by what the
    /// model learned.
    ///
    /// The oracle does not depend on the model, so it is played once: its drop is cached until
    /// something it depends on changes (the validation designs, the seed, the step limit, the
    /// task, the target settings, the line search, the objective sampling, the probe size), and
    /// a later report carries the cached figure with [`EvalReport::oracle_cached`] set.
    /// `None` when the validation split is empty.
    ///
    /// # Errors
    ///
    /// As [`Self::evaluate`].
    pub fn validate(&mut self, count: usize) -> Result<Option<EvalReport>, RunnerError> {
        self.validate_inner(count, None)
    }

    /// Continues validation from its durable case, pass, episode-step and teacher records.
    ///
    /// Use an exclusively owned directory for this validation iteration. Its full job identity
    /// must match before any work is reused. Model-independent reference records live beside
    /// the iteration directory and remain reusable when only the evaluated model changes.
    ///
    /// # Errors
    /// Returns evaluation errors, IO failures, or an incompatible/corrupt journal error.
    pub fn validate_with_recovery(
        &mut self,
        count: usize,
        directory: &Path,
    ) -> Result<Option<EvalReport>, RunnerError> {
        self.validate_inner(count, Some(directory))
    }

    fn validate_inner(
        &mut self,
        count: usize,
        directory: Option<&Path>,
    ) -> Result<Option<EvalReport>, RunnerError> {
        // The update in flight is part of the model being judged.
        self.sync_update()?;
        self.progress.phase("validation", 0);
        let entries = self.validation_entries(count);
        if entries.is_empty() {
            return Ok(None);
        }
        let key = OracleKey::new(&self.cfg, &entries, self.cfg.seed, self.max_step_deg())
            .with_environment(self.execution.is_some());
        let cached = self
            .oracle_cache
            .as_ref()
            .filter(|cache| cache.key == key && complete_references(cache))
            .cloned();
        let passes = EvalPasses {
            sampled: true,
            oracle: cached.as_ref().and_then(|cache| cache.drop).is_none()
                && self.cfg.validation_fidelity == EvalFidelity::Fast,
            noise: self.cfg.algo == Algorithm::Imitation
                && cached.as_ref().and_then(|cache| cache.noise_drop).is_none(),
        };
        let mut report = self.evaluate_with_recovery(
            &entries,
            self.cfg.validation_fidelity,
            self.cfg.seed,
            passes,
            directory,
        )?;
        if report.designs > 0 && report.failed == 0 {
            if let Some(cache) = &cached {
                restore_references(&mut report, cache);
            }
            self.oracle_cache = Some(OracleCache {
                key,
                drop: report.mean_improvement_oracle,
                noise_drop: report.mean_improvement_noise,
                cases: report.cases.clone(),
            });
        }
        self.stats.val_oracle = report.mean_improvement_oracle;
        self.stats.last_validation = Some(report.clone());
        Ok(Some(report))
    }
}

/// A mean without all of its paired reference endpoints cannot suppress a fresh pass.
fn complete_references(cache: &OracleCache) -> bool {
    !cache.cases.is_empty()
        && (cache.drop.is_some() || cache.noise_drop.is_some())
        && cache.drop.is_none_or(f32::is_finite)
        && cache.noise_drop.is_none_or(f32::is_finite)
        && cache.cases.iter().all(|case| {
            case.failure.is_none()
                && case.base_score.is_some_and(f32::is_finite)
                && (cache.drop.is_none() || case.oracle_final_score.is_some_and(f32::is_finite))
                && (cache.noise_drop.is_none()
                    || case.noise_final_score.is_some_and(f32::is_finite))
        })
}

/// Restore only when every ordered case still has the identical context and baseline.
pub(super) fn restore_references(report: &mut EvalReport, cache: &OracleCache) {
    if report.failed > 0
        || report.designs == 0
        || report.designs != report.cases.len()
        || !complete_references(cache)
        || report.cases.len() != cache.cases.len()
        || !report.cases.iter().zip(&cache.cases).all(|(case, saved)| {
            case.failure.is_none()
                && case.design_id == saved.design_id
                && case.seed == saved.seed
                && case.objective == saved.objective
                && case.target_material == saved.target_material
                && case.source_material == saved.source_material
                && case.material_augmentation == saved.material_augmentation
                && case.target_augmentation == saved.target_augmentation
                && case.lighting == saved.lighting
                && case.shape_target == saved.shape_target
                && case.base_score.map(f32::to_bits) == saved.base_score.map(f32::to_bits)
        })
    {
        return;
    }
    report.mean_improvement_oracle = cache.drop.or(report.mean_improvement_oracle);
    report.mean_improvement_noise = cache.noise_drop.or(report.mean_improvement_noise);
    report.oracle_cached = cache.drop.is_some();
    report.noise_cached = cache.noise_drop.is_some();
    for (case, saved) in report.cases.iter_mut().zip(&cache.cases) {
        if report.oracle_cached {
            case.oracle_final_score = saved.oracle_final_score;
            case.oracle_evaluations = saved.oracle_evaluations;
            case.oracle_steps = saved.oracle_steps;
            case.oracle_seconds = saved.oracle_seconds;
            case.oracle_reference_cached = true;
        }
        if report.noise_cached {
            case.noise_final_score = saved.noise_final_score;
            case.noise_reference_cached = true;
        }
        case.references_cached = case.oracle_reference_cached || case.noise_reference_cached;
    }
}
