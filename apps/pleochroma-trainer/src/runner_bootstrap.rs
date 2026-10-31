//! PPO critic bootstraps use the same device and frozen weights as the rollout.

use pleochroma_ai::{
    env::{GemstoneObservation, Step},
    training::TrainError,
};

use super::{Algorithm, RunnerError, TrainingRunner};

impl TrainingRunner {
    pub(super) fn bootstrap_values(
        &self,
        observations: &[&GemstoneObservation],
    ) -> Result<Vec<f32>, RunnerError> {
        if observations.is_empty() {
            return Ok(Vec::new());
        }
        let _task = self.progress.task(format!(
            "device critic bootstrap for {} states",
            observations.len()
        ));
        let states = observations
            .iter()
            .map(|obs| obs.try_flatten(self.split.max_tiers))
            .collect::<Result<Vec<_>, _>>()?;
        let counts: Vec<_> = observations.iter().map(|obs| obs.tiers.len()).collect();
        let values = self.rollout_model.forward_host(&states, &counts)?.values;
        if values.len() != observations.len() || values.iter().any(|v| !v.is_finite()) {
            return Err(RunnerError::Train(TrainError::ShapeMismatch(
                "device critic returned missing or non-finite bootstrap values".into(),
            )));
        }
        Ok(values)
    }

    pub(super) fn truncated_bootstraps<'a>(
        &self,
        steps: impl Iterator<Item = &'a Step<GemstoneObservation>>,
    ) -> Result<Vec<Option<f32>>, RunnerError> {
        let steps: Vec<_> = steps.collect();
        let mut result = vec![None; steps.len()];
        if self.cfg.algo != Algorithm::Ppo {
            return Ok(result);
        }
        let selected: Vec<_> = steps
            .iter()
            .enumerate()
            .filter(|(_, step)| step.truncated && !step.terminated)
            .collect();
        let observations: Vec<_> = selected.iter().map(|(_, step)| &step.observation).collect();
        let values = self.bootstrap_values(&observations)?;
        for ((index, _), value) in selected.into_iter().zip(values) {
            result[index] = Some(value);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::tests::fixture_runner_with;

    #[test]
    fn only_time_limits_bootstrap_in_the_original_slot_order() {
        let runner = fixture_runner_with(|cfg| cfg.algo = Algorithm::Ppo);
        let observation = runner.slots[0].obs.clone();
        let step = |terminated, truncated| Step {
            observation: observation.clone(),
            reward: 0.0,
            terminated,
            truncated,
            info: pleochroma_ai::env::StepInfo::default(),
        };
        let steps = [
            step(false, false),
            step(false, true),
            step(true, true),
            step(false, true),
        ];
        let values = runner.truncated_bootstraps(steps.iter()).expect("critic");
        let expected = runner
            .bootstrap_values(&[&observation, &observation])
            .expect("device batch");
        assert_eq!(values, [None, Some(expected[0]), None, Some(expected[1])]);
        assert!(runner.bootstrap_values(&[]).expect("empty").is_empty());
    }
}
