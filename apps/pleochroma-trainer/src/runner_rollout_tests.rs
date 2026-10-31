//! Collection policy regressions using synthetic outcomes without candidate scoring.

use super::*;
use crate::runner::tests::fixture_runner_with;
use pleochroma_ai::env::{StepInfo, StepRejection};

fn pending_step(runner: &TrainingRunner) -> PendingStep {
    let observation = &runner.slots[0].obs;
    let tiers = observation.tiers.len();
    PendingStep {
        free: observation.free_tier_mask(),
        raw: vec![0.0; tiers],
        log_prob: 0.0,
        value: 0.0,
        action: ContinuousTierAction::new(vec![0.0; tiers]),
        state: observation.try_flatten(runner.split.max_tiers).unwrap(),
        labelled: true,
    }
}

fn rejected(runner: &TrainingRunner, labelled: bool, fresh_label: bool) -> SlotOutcome {
    SlotOutcome {
        step: Step {
            observation: runner.slots[0].obs.clone(),
            reward: -0.1,
            terminated: false,
            truncated: false,
            info: StepInfo {
                rejection: Some(StepRejection::NotImproved),
                unchanged: true,
                is_closed: true,
                ..StepInfo::default()
            },
        },
        label: labelled.then(|| TeacherLabel {
            tiers: vec![None; runner.slots[0].obs.tiers.len()],
            oracle_gain: Some(0.25),
            oracle_scale: Some(1.0),
            action_target: None,
            evaluations: usize::from(fresh_label),
        }),
        fresh_label,
    }
}

#[test]
fn cached_labels_keep_telemetry_without_repeating_replay_and_stalled_slots_restart() {
    let mut runner = fixture_runner_with(|cfg| cfg.max_rejected_steps = 3);
    let mut rollout = RolloutStats::default();
    for fresh in [true, false] {
        let pending = pending_step(&runner);
        let outcome = rejected(&runner, true, fresh);
        assert!(
            runner
                .finish_step(0, pending, outcome, &mut rollout, None)
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(rollout.labels, 2);
    assert_eq!(rollout.label_evaluations, 1);
    assert_eq!(rollout.diagnostics.labels_added, 1);
    assert_eq!(rollout.diagnostics.labels_reused, 1);
    assert_eq!(rollout.diagnostics.unchanged_proposals, 2);
    assert_eq!(runner.aggregate.len(), 1);
    assert_eq!(runner.slots[0].rejected_steps, 2);

    let pending = pending_step(&runner);
    let mut outcome = rejected(&runner, false, false);
    outcome.step.info.rejection = None;
    outcome.step.info.unchanged = false;
    assert!(
        runner
            .finish_step(0, pending, outcome, &mut rollout, None)
            .unwrap()
            .is_none()
    );
    assert_eq!(runner.slots[0].rejected_steps, 0);

    for index in 0..3 {
        let pending = pending_step(&runner);
        let mut outcome = rejected(&runner, false, false);
        outcome.step.info.rejection = Some(StepRejection::PreservationViolation);
        outcome.step.info.unchanged = false;
        let respawn = runner
            .finish_step(0, pending, outcome, &mut rollout, None)
            .unwrap();
        assert_eq!(respawn.is_some(), index == 2);
        if let Some(respawn) = respawn {
            runner.respawn_slots(vec![respawn]).unwrap();
        }
    }
    assert_eq!(rollout.diagnostics.stalled_episodes, 1);
    assert_eq!(rollout.diagnostics.preservation_rejected, 3);
    assert_eq!(rollout.rej_unsafe, 3);
    assert_eq!(runner.slots[0].rejected_steps, 0);

    let pending = pending_step(&runner);
    let outcome = rejected(&runner, true, true);
    assert!(
        runner
            .finish_step(0, pending, outcome, &mut rollout, None)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        runner.aggregate.len(),
        2,
        "a new episode admits its own state"
    );
    assert_eq!(rollout.diagnostics.labels_added, 2);
}

#[test]
fn disabled_limit_and_ppo_do_not_restart_and_normal_ends_are_not_stalls() {
    for (algorithm, limit) in [(Algorithm::Imitation, 0), (Algorithm::Ppo, 1)] {
        let mut runner = fixture_runner_with(|cfg| {
            cfg.algo = algorithm;
            cfg.max_rejected_steps = limit;
        });
        let mut rollout = RolloutStats::default();
        for _ in 0..3 {
            let pending = pending_step(&runner);
            let outcome = rejected(&runner, true, true);
            assert!(
                runner
                    .finish_step(0, pending, outcome, &mut rollout, None)
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(rollout.diagnostics.stalled_episodes, 0);
        if algorithm == Algorithm::Ppo {
            assert_eq!(rollout.diagnostics.labels_added, 0);
            assert_eq!(runner.aggregate.len(), 0);
        }
        assert_eq!(runner.export_stats(&[], &[]).max_rejected_steps, 0);
    }

    let mut runner = fixture_runner_with(|cfg| cfg.max_rejected_steps = 1);
    let mut rollout = RolloutStats::default();
    let pending = pending_step(&runner);
    let mut outcome = rejected(&runner, true, true);
    outcome.step.truncated = true;
    outcome.label.as_mut().unwrap().oracle_gain = None;
    assert!(
        runner
            .finish_step(0, pending, outcome, &mut rollout, None)
            .unwrap()
            .is_some()
    );
    assert_eq!(rollout.diagnostics.stalled_episodes, 0);
    assert_eq!(rollout.diagnostics.labels_added, 0);
    assert_eq!(runner.aggregate.len(), 0);
}
