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

/// A step of slot 0 that moves every free tier by half the step limit.
fn moving_step(runner: &TrainingRunner, labelled: bool) -> PendingStep {
    let max_step = runner.slots[0].env.max_step_deg();
    let mut pending = pending_step(runner);
    pending.action = ContinuousTierAction::new(
        pending
            .free
            .iter()
            .map(|&free| if free { 0.5 * max_step } else { 0.0 })
            .collect(),
    );
    pending.labelled = labelled;
    pending
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
fn a_labelled_step_moves_as_the_unlabelled_one_and_caches_the_pre_step_label() {
    // The teacher and the policy search of a labelled step run side by side: the step must not
    // depend on the label, and the label must belong to the state before the step.
    let mut labelled = fixture_runner_with(|cfg| cfg.label_every = 1);
    let mut plain = fixture_runner_with(|cfg| cfg.label_every = 1);
    let limits = || pool::StagedLimits {
        threads: pool::pool_threads(),
        pending: MAX_PENDING_WORK,
        shared: None,
    };
    let before = labelled.slots[0].env.current_design().clone();
    let h_deg = probe_size(&labelled.cfg, &labelled.slots[0]);
    let pending = moving_step(&labelled, true);
    let (with_label, _) = step_slot(
        &mut labelled.slots[0],
        &pending,
        &labelled.cfg,
        &labelled.progress,
        0,
        limits(),
    );
    let pending = moving_step(&plain, false);
    let (without, _) = step_slot(
        &mut plain.slots[0],
        &pending,
        &plain.cfg,
        &plain.progress,
        0,
        limits(),
    );
    assert!(with_label.fresh_label);
    let label = with_label.label.as_ref().expect("a fresh label");
    assert!(without.label.is_none() && !without.fresh_label);
    assert_eq!(
        with_label.step.reward.to_bits(),
        without.step.reward.to_bits()
    );
    assert_eq!(with_label.step.info, without.step.info);
    assert_eq!(
        labelled.slots[0].env.current_design(),
        plain.slots[0].env.current_design()
    );
    let cached = labelled.slots[0]
        .label_cache
        .get(&before, h_deg, &labelled.cfg.line_search)
        .expect("the label is cached for the pre-step state");
    assert_eq!(cached.tiers, label.tiers);
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
