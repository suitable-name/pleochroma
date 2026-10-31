use super::super::{DesignFigures, EvalCase, Optics};
use super::*;

fn completed() -> DesignResult {
    DesignResult::Figures(DesignFigures {
        case: EvalCase {
            design_id: 9,
            final_score: Some(0.123_456_79),
            ..EvalCase::default()
        },
        base_score: 10.1,
        final_score: 0.123_456_79,
        base: Optics::default(),
        fin: Optics::default(),
        sampled_final_score: Some(1.5),
        oracle_final_score: Some(0.1),
        model_evaluations: 7,
        model_steps: 4,
    })
}

#[test]
fn reopening_reuses_only_completed_cases_and_retains_the_exact_result() {
    let directory = tempfile::tempdir().unwrap();
    let result = completed();
    {
        let journal = Journal::open_identity(directory.path(), b"same job").unwrap();
        assert!(journal.load(0, 9, None).unwrap().is_none());
        journal.save(0, 9, None, &result).unwrap();
        // A write that never reached publication is not a completed case.
        fs::write(directory.path().join("pending-unpublished"), b"partial").unwrap();
    }
    let restored = Journal::open_identity(directory.path(), b"same job").unwrap();
    let got = restored.load(0, 9, None).unwrap().unwrap();
    assert_eq!(encode(&got).unwrap(), encode(&result).unwrap());
    assert!(restored.load(1, 9, None).unwrap().is_none());
}

#[test]
fn changed_identity_or_case_binding_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open_identity(directory.path(), b"original job").unwrap();
    journal.save(0, 9, None, &completed()).unwrap();
    assert!(Journal::open_identity(directory.path(), b"different job").is_err());
    assert!(journal.load(0, 8, None).is_err());
    let mut record: CaseRecord = decode(&fs::read(journal.path(0)).unwrap()).unwrap();
    record.index = 1;
    fs::write(journal.path(0), encode(&record).unwrap()).unwrap();
    assert!(journal.load(0, 9, None).is_err());
}

#[test]
fn corrupt_or_trailing_payload_bytes_are_refused() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open_identity(directory.path(), b"job").unwrap();
    journal.save(0, 9, None, &completed()).unwrap();
    let mut record: CaseRecord = decode(&fs::read(journal.path(0)).unwrap()).unwrap();
    record.payload[0] ^= 1;
    fs::write(journal.path(0), encode(&record).unwrap()).unwrap();
    assert!(journal.load(0, 9, None).is_err());
    journal.save(0, 9, None, &completed()).unwrap();
    let mut bytes = fs::read(journal.path(0)).unwrap();
    bytes.push(0);
    fs::write(journal.path(0), bytes).unwrap();
    assert!(journal.load(0, 9, None).is_err());
}

#[test]
fn an_identity_cannot_be_recreated_beside_existing_case_results() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("case-000000.mpk"), b"orphan").unwrap();
    assert!(Journal::open_identity(directory.path(), b"job").is_err());
}

#[test]
fn an_old_case_schema_cannot_be_reused_under_a_current_identity() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open_identity(directory.path(), b"job").unwrap();
    journal.save(0, 9, None, &completed()).unwrap();
    let mut record: CaseRecord = decode(&fs::read(journal.path(0)).unwrap()).unwrap();
    record.version = 1;
    fs::write(journal.path(0), encode(&record).unwrap()).unwrap();
    assert!(journal.load(0, 9, None).is_err());
}

fn job<'a>(runner: &'a crate::runner::TrainingRunner, backend: &'static str) -> EvalJob<'a> {
    EvalJob {
        net: &runner.net,
        backend,
        inference: None,
        execution: runner.execution.as_ref(),
        split: &runner.split,
        cfg: &runner.cfg,
        fidelity: super::super::EvalFidelity::Fast,
        seed: 7,
        passes: super::super::EvalPasses::default(),
        progress: &runner.progress,
    }
}

#[test]
fn completed_cases_are_bound_to_the_resolved_policy_backend() {
    let runner = crate::runner::tests::fixture_runner_with(|_| {});
    let directory = tempfile::tempdir().unwrap();
    let cpu = job(&runner, crate::runner::CPU_LABEL);
    Journal::open(directory.path(), cpu, &[0], &[(0, None)]).unwrap();
    for backend in [crate::runner::GPU_LABEL, crate::runner::CUDA_LABEL] {
        assert!(
            Journal::open(
                directory.path(),
                EvalJob { backend, ..cpu },
                &[0],
                &[(0, None)]
            )
            .is_err()
        );
    }
}

#[test]
fn restoring_a_complete_cuda_journal_does_not_initialize_or_call_a_gpu() {
    let runner = crate::runner::tests::fixture_runner_with(|_| {});
    let directory = tempfile::tempdir().unwrap();
    let job = job(&runner, crate::runner::CUDA_LABEL);
    let journal = Journal::open(directory.path(), job, &[0], &[(0, None)]).unwrap();
    journal.save(0, 0, None, &completed()).unwrap();
    let report = super::super::evaluate_designs(job, &[0], Some(directory.path())).unwrap();
    assert_eq!(report.designs, 1);
    assert_eq!(report.cases[0].final_score, Some(0.123_456_79));
}
