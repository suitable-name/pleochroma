use super::*;
use crate::runner::{CPU_LABEL, EvalFidelity, EvalPasses, TrainingRunner};

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

#[test]
fn reference_identity_ignores_model_and_training_batch_but_binds_episode_inputs() {
    let runner = crate::runner::tests::fixture_runner_with(|_| {});
    let evaluation = job(&runner);
    let original = identity(evaluation, 0, None).unwrap();
    let mut cfg = runner.cfg.clone();
    cfg.workers += 1;
    cfg.rollout_size *= 2;
    let other = crate::runner::tests::fixture_runner_with(|c| c.seed += 1);
    assert_eq!(
        original,
        identity(
            EvalJob {
                cfg: &cfg,
                net: &other.net,
                ..evaluation
            },
            0,
            None
        )
        .unwrap(),
    );
    for changed in [
        EvalJob {
            seed: 8,
            ..evaluation
        },
        EvalJob {
            fidelity: EvalFidelity::Full,
            ..evaluation
        },
    ] {
        assert_ne!(original, identity(changed, 0, None).unwrap());
    }
    cfg.max_step_deg = Some(0.25);
    assert_ne!(
        original,
        identity(
            EvalJob {
                cfg: &cfg,
                ..evaluation
            },
            0,
            None
        )
        .unwrap()
    );
    let mut split = runner.split.clone();
    split.entries[0].design.girdle_diameter_mm = Some(12.75);
    assert_ne!(
        original,
        identity(
            EvalJob {
                split: &split,
                ..evaluation
            },
            0,
            None
        )
        .unwrap()
    );
}

#[test]
fn committed_reference_work_survives_incomplete_cases_and_later_iterations() {
    let runner = crate::runner::tests::fixture_runner_with(|_| {});
    let evaluation = job(&runner);
    let root = tempfile::tempdir().unwrap();
    let first = Journal::open(
        &root.path().join("iteration-5"),
        evaluation,
        &[0],
        &[(0, None)],
    )
    .unwrap();
    let reference = first.reference_case(evaluation, 0, None).unwrap();
    reference.save("pass-oracle", &vec![3usize, 7, 11]).unwrap();
    // No whole-case result is committed: another pass may have failed or been interrupted.
    assert!(first.load(0, 0, None).unwrap().is_none());
    let second = Journal::open(
        &root.path().join("iteration-10"),
        evaluation,
        &[0],
        &[(0, None)],
    )
    .unwrap();
    let restored = second.reference_case(evaluation, 0, None).unwrap();
    assert_eq!(
        restored.load::<Vec<usize>>("pass-oracle").unwrap(),
        Some(vec![3, 7, 11]),
    );
    let changed = EvalJob {
        seed: 8,
        ..evaluation
    };
    let different = second.reference_case(changed, 0, None).unwrap();
    assert!(
        different
            .load::<Vec<usize>>("pass-oracle")
            .unwrap()
            .is_none()
    );
}

#[test]
fn reference_namespaces_compare_the_full_corpus_identity() {
    let root = tempfile::tempdir().unwrap();
    let iteration = root.path().join("iteration-5");
    let corpus = "full corpus identity";
    let directory = open_corpus(&iteration, corpus).unwrap();
    std::fs::write(directory.join("identity.mpk"), b"changed corpus").unwrap();
    assert!(open_corpus(&iteration, corpus).is_err());
}
