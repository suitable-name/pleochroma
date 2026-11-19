//! PPO critic bootstraps use the same device and frozen weights as the rollout.

use pleochroma_ai::{env::GemstoneObservation, training::TrainError};

use super::{RunnerError, TrainingRunner};

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
}

#[cfg(test)]
mod tests {
    use crate::runner::tests::fixture_runner_with;

    #[test]
    fn rollout_cut_evaluates_only_the_requested_critic_rows() {
        let runner = fixture_runner_with(|cfg| cfg.algo = crate::runner::Algorithm::Ppo);
        let observation = runner.slots[0].obs.clone();
        let expected = runner
            .bootstrap_values(&[&observation, &observation])
            .expect("device batch");
        assert_eq!(expected.len(), 2);
        assert_eq!(expected[0], expected[1]);
        assert!(runner.bootstrap_values(&[]).expect("empty").is_empty());
    }
}
