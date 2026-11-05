use super::super::{EvalFidelity, EvalPasses, pool, recovery::Journal};
use super::*;
use crate::runner::{CPU_LABEL, CUDA_LABEL, TrainingRunner};
use pleochroma_ai::{env::RetargetEnv, model::Prng};
use std::sync::atomic::AtomicUsize;

fn job(runner: &TrainingRunner) -> EvalJob<'_> {
    EvalJob {
        net: &runner.net,
        backend: CPU_LABEL,
        inference: None,
        execution: None,
        split: &runner.split,
        cfg: &runner.cfg,
        fidelity: EvalFidelity::Fast,
        seed: 7,
        passes: EvalPasses::default(),
        progress: &runner.progress,
    }
}

fn short_env(runner: &TrainingRunner, steps: usize) -> EnvKind {
    let EnvKind::Retarget(source) = &runner.slots[0].env else {
        panic!("retarget fixture")
    };
    let mut config = source.config().clone();
    config.max_steps = steps;
    EnvKind::Retarget(Box::new(
        RetargetEnv::new(source.base_design().clone(), config).unwrap(),
    ))
}

fn clone_env(env: &EnvKind) -> EnvKind {
    let EnvKind::Retarget(source) = env else {
        panic!("retarget fixture")
    };
    EnvKind::Retarget(Box::new(
        RetargetEnv::from_episode(
            source.base_design().clone(),
            source.config().clone(),
            &source.snapshot_episode().unwrap(),
        )
        .unwrap(),
    ))
}

fn tail() -> pool::Tail {
    pool::Tail {
        unfinished: AtomicUsize::new(1),
        pool_width: 1,
        shared_pool: false,
    }
}

fn progress<'a>(runner: &TrainingRunner, tail: &'a pool::Tail) -> CaseProgress<'a> {
    CaseProgress {
        task: runner.progress.task("recovery regression".into()),
        case: 1,
        entry: 0,
        tail,
    }
}

fn bytes(value: &impl Serialize) -> Vec<u8> {
    rmp_serde::to_vec_named(value).unwrap()
}

#[test]
fn terminal_pass_restores_without_a_policy_worker_or_step_reexecution() {
    let runner = crate::runner::tests::fixture_runner_with(|_| {});
    let evaluation = job(&runner);
    let mut env = short_env(&runner, 1);
    let mut restored_env = clone_env(&env);
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(directory.path(), evaluation, &[0], &[(0, None)]).unwrap();
    let part = journal.case(0, 0, None);
    let tail = tail();
    let progress = progress(&runner, &tail);
    assert!(needs_policy(&part, false).unwrap());
    let played = evaluation
        .play(
            &mut env,
            Pilot::Model,
            &mut InferenceScratch::default(),
            &progress,
            Some(&part),
        )
        .unwrap();
    assert_eq!(played.steps, 1);
    assert!(!needs_policy(&part, false).unwrap());
    assert!(
        needs_policy(&part, true).unwrap(),
        "sampled pass is still missing"
    );
    let no_worker = EvalJob {
        backend: CUDA_LABEL,
        ..evaluation
    };
    let restored = no_worker
        .play(
            &mut restored_env,
            Pilot::Model,
            &mut InferenceScratch::default(),
            &progress,
            Some(&part),
        )
        .unwrap();
    assert!(restored.restored_complete);
    assert_eq!(restored.obs, played.obs);
    assert_eq!(restored.evaluations, played.evaluations);
    assert_eq!(restored.seconds.to_bits(), played.seconds.to_bits());
    assert_eq!(restored_env.current_design(), env.current_design());
    assert_eq!(restored_env.step_count(), 1);
}

#[test]
fn resuming_a_completed_step_preserves_stochastic_rng_geometry_and_counts() {
    let runner = crate::runner::tests::fixture_runner_with(|_| {});
    let evaluation = job(&runner);
    let baseline = short_env(&runner, 3);
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(directory.path(), evaluation, &[0], &[(0, None)]).unwrap();
    let expected_parts = journal.case(0, 0, None);
    let resumed_parts = journal.case(1, 0, None);
    let tail = tail();
    let progress = progress(&runner, &tail);
    for pilot in [
        Pilot::Noise {
            rng: Prng::new(17),
            std: 0.3,
        },
        Pilot::Sampled {
            rng: Prng::new(17),
            explore_std: Some(0.3),
        },
    ] {
        let part = format!("pass-{}", pilot.key());
        let mut expected_env = clone_env(&baseline);
        evaluation
            .play(
                &mut expected_env,
                pilot.clone(),
                &mut InferenceScratch::default(),
                &progress,
                Some(&expected_parts),
            )
            .unwrap();
        let mut interrupted = clone_env(&baseline);
        let obs = interrupted.reset(evaluation.seed);
        let mut snapshot = PassSnapshot::start(
            &interrupted,
            pilot.clone(),
            Optics::from_global(&obs.global),
        )
        .unwrap();
        evaluation
            .play_step(
                &mut interrupted,
                &mut snapshot,
                &mut InferenceScratch::default(),
                &progress,
                None,
            )
            .unwrap();
        snapshot.episode = interrupted.snapshot_episode().unwrap();
        resumed_parts.save(&part, &snapshot).unwrap();
        let mut resumed = clone_env(&baseline);
        evaluation
            .play(
                &mut resumed,
                pilot,
                &mut InferenceScratch::default(),
                &progress,
                Some(&resumed_parts),
            )
            .unwrap();
        let mut expected: PassSnapshot = expected_parts.load(&part).unwrap().unwrap();
        let mut actual: PassSnapshot = resumed_parts.load(&part).unwrap().unwrap();
        expected.elapsed_seconds = 0.0;
        actual.elapsed_seconds = 0.0;
        assert_eq!(bytes(&actual), bytes(&expected));
        assert_eq!(
            resumed.build_observation(),
            expected_env.build_observation()
        );
    }
}

#[test]
fn only_terminal_and_oracle_steps_commit_before_the_interval() {
    let runner = crate::runner::tests::fixture_runner_with(|_| {});
    let env = short_env(&runner, 3);
    let pilot = Pilot::Noise {
        rng: Prng::new(17),
        std: 0.3,
    };
    let mut state = PassSnapshot::start(&env, pilot, Optics::default()).unwrap();
    assert!(
        !commit_due(&state, Duration::ZERO),
        "replayable steps are batched"
    );
    assert!(commit_due(&state, COMMIT_INTERVAL));
    state.done = true;
    assert!(
        commit_due(&state, Duration::ZERO),
        "the completed-pass record is never batched"
    );
    let oracle = PassSnapshot::start(&env, Pilot::Oracle, Optics::default()).unwrap();
    assert!(
        commit_due(&oracle, Duration::ZERO),
        "teacher journals are keyed by step"
    );
}

#[test]
fn malformed_pass_state_and_changed_distribution_are_refused() {
    let runner = crate::runner::tests::fixture_runner_with(|_| {});
    let mut env = short_env(&runner, 1);
    let pilot = Pilot::Noise {
        rng: Prng::new(17),
        std: 0.3,
    };
    let state = PassSnapshot::start(&env, pilot.clone(), Optics::default()).unwrap();
    let mut corrupt = state.clone();
    corrupt.steps = 1;
    assert!(corrupt.restore(&mut env, &pilot).is_err());
    corrupt = state.clone();
    corrupt.elapsed_seconds = f64::NAN;
    assert!(corrupt.restore(&mut env, &pilot).is_err());
    assert!(
        state
            .restore(
                &mut env,
                &Pilot::Noise {
                    rng: Prng::new(17),
                    std: 0.5
                }
            )
            .is_err()
    );
}

#[test]
fn validation_baseline_restore_uses_evaluation_fidelity_not_training_mixture() {
    let runner = crate::runner::tests::fixture_runner_with(|_| {});
    let mut cfg = runner.cfg.clone();
    cfg.train_fidelity = pleochroma_cut_core::optimize::ObjectiveFidelity::Full;
    let item = &runner.split.entries[0];
    let seed = 7;
    let target =
        super::super::pick_target(&cfg, seed, &item.design, item.authored_material.as_ref())
            .unwrap();
    let weights = super::super::draw_weights(cfg.objectives, seed);
    let fidelity = pleochroma_cut_core::optimize::ObjectiveFidelity::Fast;
    let env = super::super::build_env(
        &cfg,
        &item.design,
        target.as_ref(),
        weights,
        (Some(fidelity), None),
        seed,
        item.authored_material.as_ref(),
    )
    .unwrap();
    let restored = crate::runner::envs::build_eval_env_from_episode(
        &cfg,
        &item.design,
        target.as_ref(),
        weights,
        seed,
        item.authored_material.as_ref(),
        &env.snapshot_episode().unwrap(),
        (fidelity, None),
    )
    .unwrap();
    assert_eq!(restored.build_observation(), env.build_observation());
}
