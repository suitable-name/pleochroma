//! Filesystem recovery tests; no trainer/backend is needed for manifest publication checks.

use super::*;

fn args() -> CliArgs {
    let crate::cli_args::Parsed::Run(args) = crate::cli_args::parse_args(&[]).unwrap() else {
        panic!("run arguments");
    };
    *args
}

fn state(iteration: usize) -> RunState {
    RunState {
        schema: RESUME_SCHEMA,
        trainer_version: env!("CARGO_PKG_VERSION").to_owned(),
        iteration,
        phase: ResumePhase::Training,
        args: args(),
        database: None,
        records: RunRecords::default(),
    }
}

fn publish_stub(root: &Path, manifest: &Path, iteration: usize) -> PathBuf {
    publish_state_stub(root, manifest, &state(iteration))
}

fn publish_state_stub(root: &Path, manifest: &Path, state: &RunState) -> PathBuf {
    let temp = tempfile::Builder::new()
        .prefix("generation-")
        .tempdir_in(root)
        .unwrap();
    storage::write_json(&temp.path().join("resume.json"), state).unwrap();
    for name in [
        "model.ckpt.json",
        "runner-state.mpk",
        "replay.mpk",
        "trainer.json",
        "trainer-model.bpk",
        "trainer-optimizer.bpk",
    ] {
        fs::write(temp.path().join(name), b"stub payload").unwrap();
    }
    let path = temp.path().to_path_buf();
    storage::publish(temp, root, manifest, state.iteration).unwrap();
    path
}

#[test]
fn latest_only_advances_after_publication_and_keeps_two_generations() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("test.run");
    fs::create_dir(&root).unwrap();
    let manifest = temp.path().join("test.latest.json");
    let first = publish_stub(&root, &manifest, 1);
    assert_eq!(storage::read_committed(&root, &manifest).unwrap(), first);
    let interrupted = tempfile::Builder::new()
        .prefix("generation-")
        .tempdir_in(&root)
        .unwrap();
    fs::write(interrupted.path().join("incomplete"), b"partial").unwrap();
    assert_eq!(storage::read_committed(&root, &manifest).unwrap(), first);
    let second = publish_stub(&root, &manifest, 2);
    assert!(first.is_dir());
    let third = publish_stub(&root, &manifest, 3);
    assert!(!first.exists());
    assert!(second.is_dir());
    assert_eq!(storage::read_committed(&root, &manifest).unwrap(), third);
}

#[test]
fn incomplete_payload_is_refused_and_cannot_replace_the_previous_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("run");
    fs::create_dir(&root).unwrap();
    let manifest = temp.path().join("latest.json");
    let committed = publish_stub(&root, &manifest, 2);
    let incomplete = tempfile::Builder::new()
        .prefix("generation-")
        .tempdir_in(&root)
        .unwrap();
    fs::write(incomplete.path().join("model.ckpt.json"), b"new").unwrap();
    assert!(storage::publish(incomplete, &root, &manifest, 3).is_err());
    assert_eq!(
        storage::read_committed(&root, &manifest).unwrap(),
        committed
    );
    fs::write(committed.join("model.ckpt.json"), b"corrupt").unwrap();
    assert!(storage::read_committed(&root, &manifest).is_err());
}

#[test]
fn model_and_resume_metadata_without_optimizer_cannot_be_published() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("run");
    fs::create_dir(&root).unwrap();
    let manifest = temp.path().join("latest.json");
    let committed = publish_stub(&root, &manifest, 1);
    let incomplete = tempfile::Builder::new()
        .prefix("generation-")
        .tempdir_in(&root)
        .unwrap();
    for name in [
        "resume.json",
        "model.ckpt.json",
        "runner-state.mpk",
        "replay.mpk",
        "trainer.json",
        "trainer-model.bpk",
    ] {
        fs::copy(committed.join(name), incomplete.path().join(name)).unwrap();
    }
    assert!(storage::publish(incomplete, &root, &manifest, 2).is_err());
    assert_eq!(
        storage::read_committed(&root, &manifest).unwrap(),
        committed
    );
}

#[test]
fn resume_accepts_a_stable_prefix_and_rejects_training_overrides() {
    let temp = tempfile::tempdir().unwrap();
    let prefix = temp.path().join("retarget_m_layout7");
    fs::write(suffixed(&prefix, ".latest.json"), b"{}").unwrap();
    for target in [
        prefix.clone(),
        suffixed(&prefix, ".ckpt.json"),
        suffixed(&prefix, ".latest.json"),
        suffixed(&prefix, ".run"),
    ] {
        assert_eq!(resume_prefix(&target).unwrap(), absolute(&prefix).unwrap());
    }
    let accepted: Vec<String> = ["--device", "cuda", "--resume", "xy"]
        .map(str::to_owned)
        .into();
    validate_resume_flags(&accepted).unwrap();
    let refused: Vec<String> = ["--resume", "xy", "--iterations", "280"]
        .map(str::to_owned)
        .into();
    assert!(validate_resume_flags(&refused).is_err());
}

#[test]
fn legacy_weights_do_not_pretend_to_have_replay_and_optimizer_state() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("old.ckpt-iter20.json");
    fs::write(&path, b"{\"schema_version\":6}").unwrap();
    assert!(
        resume_prefix(&path)
            .unwrap_err()
            .to_string()
            .contains("--warm-start")
    );
}

#[test]
fn completed_and_pending_phases_cannot_skip_remaining_training() {
    let mut state = state(20);
    state.phase = ResumePhase::Complete;
    assert!(validate_state(&state).is_err());
    state.phase = ResumePhase::ValidationPending;
    validate_state(&state).unwrap();
    state.iteration = 21;
    assert!(validate_state(&state).is_err());
    state.phase = ResumePhase::FinalEvaluationPending;
    state.iteration = state.args.iterations;
    assert!(validate_state(&state).is_err());
    state.args.eval_test = true;
    validate_state(&state).unwrap();
}

#[test]
fn a_run_lock_is_released_on_drop_and_prevents_concurrent_writers() {
    let temp = tempfile::tempdir().unwrap();
    let prefix = temp.path().join("run");
    let first = open_session(prefix.clone()).unwrap();
    assert!(open_session(prefix.clone()).is_err());
    drop(first);
    assert!(open_session(prefix).is_ok());
}

#[test]
fn preparation_restores_configuration_and_only_overrides_the_device() {
    let temp = tempfile::tempdir().unwrap();
    let mut configured = args();
    configured.fixture = true;
    configured.workers = 7;
    configured.iterations = 300;
    configured.batch_size = 256;
    configured.mini_batch = Some(128);
    configured.updates_per_round = Some(16);
    configured.validation_every = 20;
    configured.save_every = 1;
    configured.stats_path = temp
        .path()
        .join("original.json")
        .to_string_lossy()
        .into_owned();
    let (saved, session) = prepare(configured, &[]).unwrap();
    let prefix = session.prefix.clone();
    let snapshot = RunState {
        args: saved.clone(),
        phase: ResumePhase::ValidationPending,
        iteration: 20,
        ..state(20)
    };
    publish_state_stub(&session.root, &session.manifest, &snapshot);
    drop(session);
    let raw = vec![
        "--resume".into(),
        prefix.to_string_lossy().into_owned(),
        "--device".into(),
        "cuda".into(),
    ];
    let crate::cli_args::Parsed::Run(parsed) = crate::cli_args::parse_args(&raw).unwrap() else {
        panic!("resume arguments");
    };
    let (actual, recovery) = prepare(*parsed, &raw).unwrap();
    let mut expected = saved;
    expected.device = crate::runner::DeviceChoice::Cuda;
    expected.resume = Some(absolute(&prefix).unwrap().to_string_lossy().into_owned());
    assert_eq!(actual, expected);
    assert!(recovery.is_resumed());
    assert_eq!(recovery.phase(), ResumePhase::ValidationPending);
    assert!(recovery.database_path().is_none());
}

#[test]
fn default_output_paths_are_fixed_before_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let mut configured = args();
    configured.fixture = true;
    configured.stats_path = temp.path().join("run.json").to_string_lossy().into_owned();
    let (saved, recovery) = prepare(configured, &[]).unwrap();
    assert_eq!(
        saved.weights_path.as_deref(),
        Some(
            absolute(&temp.path().join("run.ckpt.json"))
                .unwrap()
                .to_str()
                .unwrap()
        )
    );
    assert_eq!(
        crate::periodic_checkpoint_path(&saved, 20),
        temp.path().join("run.ckpt-iter20.json").to_string_lossy()
    );
    assert_eq!(
        crate::validation_checkpoint_path(&saved),
        temp.path()
            .join("run.ckpt-best-validation.json")
            .to_string_lossy()
    );
    assert_eq!(
        recovery.prefix(),
        absolute(&temp.path().join("run")).unwrap()
    );
}

#[test]
fn deserialized_invalid_ranges_are_rejected_before_schedule_arithmetic() {
    let mut snapshot = state(20);
    snapshot.phase = ResumePhase::ValidationPending;
    snapshot.args.validation_every = 0;
    assert!(validate_state(&snapshot).is_err());
    snapshot.args.validation_every = 20;
    snapshot.args.mini_batch = Some(0);
    assert!(validate_state(&snapshot).is_err());
    snapshot.args.mini_batch = Some(128);
    snapshot.args.workers = usize::MAX;
    assert!(validate_state(&snapshot).is_err());
}
