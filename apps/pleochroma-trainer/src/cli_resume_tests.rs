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

const STUB_PAYLOADS: [&str; 6] = [
    "model.ckpt.json",
    "runner-state.mpk",
    "replay.mpk",
    "trainer.json",
    "trainer-model.bpk",
    "trainer-optimizer.bpk",
];

/// A generation directory named as `RecoverySession::save` names it.
fn pending_generation(root: &Path, iteration: usize) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("generation-{iteration:08}-"))
        .tempdir_in(root)
        .unwrap()
}

fn publish_state_stub(root: &Path, manifest: &Path, state: &RunState) -> PathBuf {
    let temp = pending_generation(root, state.iteration);
    let resume = storage::write_json_stamped(&temp.path().join("resume.json"), state).unwrap();
    for name in STUB_PAYLOADS {
        fs::write(temp.path().join(name), b"stub payload").unwrap();
    }
    let path = temp.path().to_path_buf();
    storage::publish(temp, root, manifest, state.iteration, Some(&resume)).unwrap();
    path
}

fn run_root(temp: &tempfile::TempDir) -> (PathBuf, PathBuf) {
    let root = temp.path().join("run.run");
    fs::create_dir(&root).unwrap();
    (root, temp.path().join("run.latest.json"))
}

/// Byte-wise FNV-1a 64, the stamp of the first manifest schema.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
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
    assert!(storage::publish(incomplete, &root, &manifest, 3, None).is_err());
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
    assert!(storage::publish(incomplete, &root, &manifest, 2, None).is_err());
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
        crate::validation_checkpoint_path(&saved, None),
        temp.path()
            .join("run.ckpt-best-validation-v3.json")
            .to_string_lossy()
    );
    assert_eq!(
        crate::validation_checkpoint_path(&saved, Some("0123456789abcdef")),
        temp.path()
            .join("run.ckpt-best-validation-v3.0123456789abcdef.json")
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

#[test]
fn a_damaged_latest_generation_falls_back_to_the_verified_previous_one() {
    let temp = tempfile::tempdir().unwrap();
    let (root, manifest) = run_root(&temp);
    let first = publish_stub(&root, &manifest, 1);
    let second = publish_stub(&root, &manifest, 2);
    fs::write(second.join("replay.mpk"), b"bit rot").unwrap();
    assert_eq!(storage::read_committed(&root, &manifest).unwrap(), first);
    // The previous generation became the latest one; the damaged one is kept aside.
    let damaged = root.join(format!(
        "damaged-{}",
        second.file_name().unwrap().to_str().unwrap()
    ));
    assert!(!second.exists() && damaged.is_dir());
    assert_eq!(storage::read_committed(&root, &manifest).unwrap(), first);
    // The next commit keeps the generation the run continued from as its own fallback.
    let third = publish_stub(&root, &manifest, 2);
    assert_eq!(storage::read_committed(&root, &manifest).unwrap(), third);
    fs::write(third.join("trainer.json"), b"corrupt").unwrap();
    assert_eq!(storage::read_committed(&root, &manifest).unwrap(), first);
    assert!(damaged.is_dir(), "damaged generations are never swept");
    // Without a verifiable previous generation the damage is an error, never silent.
    fs::write(first.join("model.ckpt.json"), b"corrupt").unwrap();
    assert!(storage::read_committed(&root, &manifest).is_err());
}

#[test]
fn first_release_manifests_still_verify_and_serve_as_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let (root, manifest) = run_root(&temp);
    let generation = pending_generation(&root, 4).keep();
    let resume = serde_json::to_vec(&state(4)).unwrap();
    fs::write(generation.join("resume.json"), &resume).unwrap();
    let mut files = serde_json::Map::new();
    files.insert(
        "resume.json".into(),
        serde_json::json!({"bytes": resume.len(), "fnv1a": fnv1a(&resume)}),
    );
    for name in STUB_PAYLOADS {
        fs::write(generation.join(name), b"stub payload").unwrap();
        files.insert(
            name.into(),
            serde_json::json!({"bytes": 12, "fnv1a": fnv1a(b"stub payload")}),
        );
    }
    let name = generation.file_name().unwrap().to_str().unwrap();
    let legacy = serde_json::json!({
        "schema": 1, "iteration": 4, "generation": name, "previous": null, "files": files,
    });
    fs::write(&manifest, serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert_eq!(
        storage::read_committed(&root, &manifest).unwrap(),
        generation
    );
    // A new commit records the old generation, with its byte-wise stamps, as the fallback.
    let next = publish_stub(&root, &manifest, 5);
    assert_eq!(storage::read_committed(&root, &manifest).unwrap(), next);
    fs::write(next.join("replay.mpk"), b"bit rot").unwrap();
    assert_eq!(
        storage::read_committed(&root, &manifest).unwrap(),
        generation
    );
}

#[test]
fn stale_pending_generations_are_swept_without_touching_retained_ones() {
    let temp = tempfile::tempdir().unwrap();
    let (root, manifest) = run_root(&temp);
    let killed = pending_generation(&root, 0).keep();
    fs::write(killed.join("replay.mpk"), b"partial").unwrap();
    let unrelated = root.join("generation-notes");
    fs::create_dir(&unrelated).unwrap();
    // A new run's start removes what a killed first save left behind.
    assert!(storage::sweep_stale_generations(&root, &manifest).is_empty());
    assert!(!killed.exists());
    assert!(
        unrelated.is_dir(),
        "only tempfile's own generation names are swept"
    );
    let first = publish_stub(&root, &manifest, 1);
    let second = publish_stub(&root, &manifest, 2);
    let interrupted = pending_generation(&root, 3).keep();
    fs::write(interrupted.join("replay.mpk"), b"partial").unwrap();
    let third = publish_stub(&root, &manifest, 3);
    assert!(!interrupted.exists() && !first.exists());
    assert!(second.is_dir() && third.is_dir());
    assert!(storage::sweep_stale_generations(&root, &manifest).is_empty());
    assert!(second.is_dir() && third.is_dir());
}

#[test]
fn a_second_commit_of_one_iteration_links_identical_payloads() {
    let temp = tempfile::tempdir().unwrap();
    let (root, manifest) = run_root(&temp);
    let pending = publish_stub(&root, &manifest, 20);
    let mut validated = state(20);
    validated.phase = ResumePhase::ValidationPending;
    let committed = publish_state_stub(&root, &manifest, &validated);
    assert_eq!(
        storage::read_committed(&root, &manifest).unwrap(),
        committed
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let inode =
            |directory: &Path, name: &str| fs::metadata(directory.join(name)).unwrap().ino();
        assert_eq!(
            inode(&committed, "replay.mpk"),
            inode(&pending, "replay.mpk")
        );
        assert_ne!(
            inode(&committed, "resume.json"),
            inode(&pending, "resume.json")
        );
    }
    // After a training step nothing is linked, and removing the older twin keeps the
    // payloads the retained one shares with it.
    let later = publish_stub(&root, &manifest, 21);
    assert!(!pending.exists());
    fs::write(later.join("replay.mpk"), b"bit rot").unwrap();
    assert_eq!(
        storage::read_committed(&root, &manifest).unwrap(),
        committed
    );
}

#[test]
fn only_validation_journals_before_the_latest_finished_one_are_pruned() {
    let temp = tempfile::tempdir().unwrap();
    let (root, _) = run_root(&temp);
    let journal = |name: &str| {
        let path = root.join(name);
        fs::create_dir(&path).unwrap();
        fs::write(path.join("identity.mpk"), b"identity").unwrap();
        path
    };
    let older = journal("validation-v3-cpu-env-v1-00000010");
    let older_cuda = journal(&format!(
        "validation-v3-{}-00000010",
        pleochroma_ai::env::CUDA_ENVIRONMENT_MATH
    ));
    let finished = journal("validation-v3-cpu-env-v1-00000020");
    let finished_wgpu = journal("validation-v3-wgpu-cpu-env-v1-00000020");
    let pending = journal("validation-v3-cpu-env-v1-00000030");
    let references = journal("reference-cases-v1");
    assert!(storage::prune_validation_journals(&root, 20).is_empty());
    assert!(!older.exists() && !older_cuda.exists());
    assert!(finished.is_dir() && finished_wgpu.is_dir() && pending.is_dir());
    assert!(references.is_dir());
}

/// A saved fixture run named `name` at iteration 20; returns its arguments and prefix.
fn saved_run(temp: &tempfile::TempDir, name: &str, algo: Algorithm) -> (CliArgs, PathBuf) {
    let mut configured = args();
    configured.fixture = true;
    configured.algo = algo;
    configured.stats_path = temp
        .path()
        .join(format!("{name}.json"))
        .to_string_lossy()
        .into_owned();
    let (saved, session) = prepare(configured, &[]).unwrap();
    let snapshot = RunState {
        args: saved.clone(),
        ..state(20)
    };
    publish_state_stub(&session.root, &session.manifest, &snapshot);
    (saved, session.prefix.clone())
}

fn resume_with(prefix: &Path, extra: &[&str]) -> Result<CliArgs, String> {
    let mut raw = vec!["--resume".to_owned(), prefix.to_string_lossy().into_owned()];
    raw.extend(extra.iter().map(|flag| (*flag).to_owned()));
    let crate::cli_args::Parsed::Run(parsed) = crate::cli_args::parse_args(&raw)? else {
        panic!("resume arguments");
    };
    prepare(*parsed, &raw)
        .map(|(args, _)| args)
        .map_err(|error| error.to_string())
}

#[test]
fn resume_overrides_reach_the_restored_run_and_other_training_flags_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    let (saved, prefix) = saved_run(&temp, "imitation", Algorithm::Imitation);
    let resumed = resume_with(
        &prefix,
        &[
            "--save-every",
            "3",
            "--imitation-lr",
            "1e-4",
            "--max-grad-norm",
            "0.5",
            "--relabel",
        ],
    )
    .unwrap();
    let mut expected = saved.clone();
    expected.save_every = 3;
    expected.imitation_lr = resumed.imitation_lr;
    expected.max_grad_norm = Some(0.5);
    expected.relabel = true;
    expected.resume = Some(absolute(&prefix).unwrap().to_string_lossy().into_owned());
    assert_eq!(resumed, expected, "nothing but the overrides changes");
    assert!((resumed.imitation_lr.unwrap() - 1e-4).abs() < 1e-12);
    let config = build_config(&resumed);
    assert!((config.imitation.learning_rate - 1e-4).abs() < 1e-12);
    assert_eq!(config.imitation.max_grad_norm, 0.5);
    // `--relabel` is a permission of one invocation, never saved with the run.
    let encoded = serde_json::to_value(&resumed).unwrap();
    assert!(encoded.get("relabel").is_none());
    for refused in [
        &["--iterations", "5"][..],
        &["--lr", "1e-4"],
        &["--batch-size", "64"],
        &["--seed", "1"],
        &["--validation-every", "5"],
        &["--save-stats", "other.json"],
    ] {
        let error = resume_with(&prefix, refused).unwrap_err();
        assert!(error.contains(refused[0]), "{refused:?}: {error}");
    }
    let (_, ppo) = saved_run(&temp, "ppo", Algorithm::Ppo);
    for refused in [&["--imitation-lr", "1e-4"][..], &["--max-grad-norm", "0.5"]] {
        let error = resume_with(&ppo, refused).unwrap_err();
        assert!(error.contains("--algo imitation"), "{refused:?}: {error}");
    }
    assert_eq!(
        resume_with(&ppo, &["--save-every", "1"])
            .unwrap()
            .save_every,
        1
    );
}
