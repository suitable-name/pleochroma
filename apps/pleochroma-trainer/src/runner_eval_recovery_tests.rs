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

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct PartialEpisode {
    step: usize,
    measurements: Vec<f64>,
    completed_probes: Vec<Option<bool>>,
}

#[test]
fn fragments_round_trip_and_replace_without_changing_legacy_cases() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open_identity(directory.path(), b"version-three job").unwrap();
    let completed = completed();
    journal.save(0, 9, None, &completed).unwrap();
    let identity_before = fs::read(directory.path().join("identity.mpk")).unwrap();
    let case_before = fs::read(journal.path(0)).unwrap();
    let parts = journal.case(0, 9, None);
    assert!(
        parts
            .load::<PartialEpisode>("oracle-step")
            .unwrap()
            .is_none()
    );
    let mut state = PartialEpisode {
        step: 1,
        measurements: vec![0.123_456_789_012_345_66, -0.0],
        completed_probes: vec![Some(false), None, Some(true)],
    };
    parts.save("oracle-step", &state).unwrap();
    state.step = 2;
    state.completed_probes[1] = Some(false);
    parts.save("oracle-step", &state).unwrap();
    fs::write(directory.path().join("pending-partial-probe"), b"partial").unwrap();
    let reopened = Journal::open_identity(directory.path(), b"version-three job").unwrap();
    let restored = reopened
        .case(0, 9, None)
        .load::<PartialEpisode>("oracle-step")
        .unwrap()
        .unwrap();
    assert_eq!(restored, state);
    assert_eq!(restored.measurements[1].to_bits(), (-0.0f64).to_bits());
    assert_eq!(
        fs::read(directory.path().join("identity.mpk")).unwrap(),
        identity_before
    );
    assert_eq!(fs::read(journal.path(0)).unwrap(), case_before);
    assert_eq!(
        encode(&reopened.load(0, 9, None).unwrap().unwrap()).unwrap(),
        encode(&completed).unwrap()
    );
}

#[test]
fn fragments_reject_invalid_names_and_wrong_case_or_part_binding() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open_identity(directory.path(), b"job").unwrap();
    let parts = journal.case(0, 9, None);
    for name in [
        "",
        ".",
        "..",
        "../outside",
        "a/b",
        "a\\b",
        "C:outside",
        "a\0b",
    ] {
        assert!(parts.save(name, &7usize).is_err());
        assert!(parts.load::<usize>(name).is_err());
    }
    assert!(parts.save(&"x".repeat(97), &7usize).is_err());
    parts.save("model", &7usize).unwrap();
    assert!(journal.case(0, 8, None).load::<usize>("model").is_err());
    assert!(
        journal
            .case(1, 9, None)
            .load::<usize>("model")
            .unwrap()
            .is_none()
    );
    let bytes = fs::read(parts.path("model").unwrap()).unwrap();
    fs::write(parts.path("sampled").unwrap(), bytes).unwrap();
    assert!(parts.load::<usize>("sampled").is_err());
}

#[test]
fn corrupt_fragment_versions_payloads_and_trailing_bytes_are_refused() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open_identity(directory.path(), b"job").unwrap();
    let parts = journal.case(0, 9, None);
    for mutation in 0..3 {
        parts.save("oracle", &7usize).unwrap();
        let path = parts.path("oracle").unwrap();
        let mut record: PartRecord = decode(&fs::read(&path).unwrap()).unwrap();
        if mutation == 0 {
            record.version += 1;
        } else {
            record.payload[0] ^= 1;
        }
        let mut bytes = encode(&record).unwrap();
        if mutation == 2 {
            bytes.push(0);
        }
        fs::write(path, bytes).unwrap();
        assert!(parts.load::<usize>("oracle").is_err());
    }
}

#[test]
fn continuation_write_failures_are_not_hidden() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open_identity(directory.path(), b"job").unwrap();
    let parts = journal.case(0, 9, None);
    fs::create_dir(parts.path("oracle").unwrap()).unwrap();
    assert!(parts.save("oracle", &1usize).is_err());
    assert!(parts.load::<usize>("oracle").is_err());
}

#[cfg(unix)]
#[test]
fn journal_directory_and_payload_symlinks_are_refused() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().unwrap();
    let actual = directory.path().join("actual");
    fs::create_dir(&actual).unwrap();
    let linked = directory.path().join("linked");
    symlink(&actual, &linked).unwrap();
    assert!(Journal::open_identity(&linked.join("nested"), b"job").is_err());
    let journal = Journal::open_identity(&actual, b"job").unwrap();
    let parts = journal.case(0, 9, None);
    let outside = directory.path().join("untouched");
    fs::write(&outside, b"keep").unwrap();
    symlink(&outside, parts.path("oracle").unwrap()).unwrap();
    assert!(parts.save("oracle", &1usize).is_err());
    assert!(parts.load::<usize>("oracle").is_err());
    assert_eq!(fs::read(outside).unwrap(), b"keep");
}
