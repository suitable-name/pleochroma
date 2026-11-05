//! Configuration, corpus and fidelity flag regressions.

use super::*;

#[test]
fn full_recovery_and_weight_warm_start_are_distinct() {
    let resume = run_args(&["--device", "cuda", "--resume", "retarget_m_layout7"]);
    assert_eq!(resume.resume.as_deref(), Some("retarget_m_layout7"));
    assert!(resume.warm_start.is_none());
    let warm = run_args(&["--warm-start", "weights.json", "--iterations", "280"]);
    assert!(warm.resume.is_none());
    assert_eq!(warm.warm_start.as_deref(), Some("weights.json"));
    assert_eq!(warm.iterations, 280);
    assert!(
        parse(&["--resume", "run", "--warm-start", "weights.json"])
            .unwrap_err()
            .contains("mutually exclusive")
    );
}

#[test]
fn relabel_is_a_resume_permission_that_is_never_saved() {
    let resume = run_args(&["--resume", "run", "--relabel"]);
    assert!(resume.relabel);
    assert!(
        parse(&["--relabel"])
            .unwrap_err()
            .contains("--relabel applies only to --resume")
    );
    let saved: CliArgs = serde_json::from_value(serde_json::to_value(&*resume).unwrap()).unwrap();
    assert!(!saved.relabel);
    assert!(HELP.contains("Ctrl-C (SIGINT) or SIGTERM"));
}

#[test]
fn the_imitation_gradient_clip_is_range_checked_and_reaches_the_configuration() {
    assert_eq!(build_config(&run_args(&[])).imitation.max_grad_norm, 1.0);
    let args = run_args(&["--max-grad-norm", "0.25"]);
    assert_eq!(args.max_grad_norm, Some(0.25));
    let config = build_config(&args);
    assert_eq!(config.imitation.max_grad_norm, 0.25);
    assert_eq!(config.ppo.max_grad_norm, PpoConfig::default().max_grad_norm);
    for value in ["0", "-1", "nan", "inf", "x"] {
        assert!(parse(&["--max-grad-norm", value]).is_err(), "{value}");
    }
    assert!(parse(&["--max-grad-norm"]).is_err());
    let notes = run_args(&["--algo", "ppo", "--max-grad-norm", "0.5"]).ignored_flag_notes();
    assert_eq!(
        notes,
        ["--max-grad-norm applies to --algo imitation only and is ignored"]
    );
    // Runs saved before the flag existed resume with the built-in clip.
    let mut saved = serde_json::to_value(&*run_args(&[])).unwrap();
    saved.as_object_mut().unwrap().remove("max_grad_norm");
    let restored: CliArgs = serde_json::from_value(saved).unwrap();
    assert_eq!(restored.max_grad_norm, None);
}

#[test]
fn saved_cli_settings_round_trip_without_reapplying_machine_defaults() {
    let args = run_args(&[
        "--mode",
        "optimizer",
        "--algo",
        "ppo",
        "--workers",
        "7",
        "--device",
        "cpu",
        "--iterations",
        "300",
        "--batch-size",
        "256",
        "--mini-batch",
        "128",
        "--design-dir",
        "native",
        "--include-library",
        "--seed",
        "91",
        "--objective-sweep",
        "--eval-test",
        "--save-every",
        "1",
        "--save-stats",
        "stats.json",
        "--save-weights",
        "weights.json",
    ]);
    let bytes = serde_json::to_vec(&*args).unwrap();
    let recovered: CliArgs = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(recovered, *args);
    assert_eq!(recovered.workers, 7);
    assert_eq!(
        build_config(&recovered).ppo.lr_schedule,
        build_config(&args).ppo.lr_schedule
    );
}

#[test]
fn the_flags_reach_the_run_configuration() {
    let args = run_args(&[
        "--algo",
        "ppo",
        "--iterations",
        "200",
        "--batch-size",
        "1024",
        "--mini-batch",
        "128",
        "--epochs",
        "4",
        "--lr",
        "2e-4",
        "--entropy-coeff",
        "0.005",
        "--label-every",
        "4",
        "--max-step-deg",
        "0.25",
        "--validation-every",
        "7",
    ]);
    let config = build_config(&args);
    assert_eq!(config.algo, Algorithm::Ppo);
    assert_eq!(config.validation_every, 7);
    assert_eq!(config.label_every, 4);
    assert_eq!(config.max_step_deg, Some(0.25));
    assert!((config.ppo.learning_rate - 2e-4).abs() < 1e-9);
    assert!((config.ppo.entropy_coeff - 0.005).abs() < 1e-9);
    // 200 iterations x 4 epochs x (1024 / 128 = 8 mini-batches).
    assert_eq!(
        config.ppo.lr_schedule,
        LrSchedule::Linear {
            total_updates: 6400
        }
    );

    // Without the flags the PPO defaults stay.
    let defaults = build_config(&run_args(&["--algo", "ppo"]));
    let ppo = PpoConfig::default();
    assert!((defaults.ppo.learning_rate - ppo.learning_rate).abs() < 1e-12);
    assert!((defaults.ppo.entropy_coeff - ppo.entropy_coeff).abs() < 1e-12);
    assert_eq!(defaults.max_step_deg, None);
    assert_eq!(defaults.label_every, 0);
}

#[test]
fn flags_of_the_other_algorithm_are_reported_as_ignored() {
    assert_eq!(run_args(&[]).ignored_flag_notes(), Vec::<String>::new());
    assert_eq!(
        run_args(&["--algo", "ppo", "--lr", "1e-4"]).ignored_flag_notes(),
        Vec::<String>::new()
    );
    let notes = run_args(&["--lr", "1e-4", "--entropy-coeff", "0.01"]).ignored_flag_notes();
    assert_eq!(
        notes,
        vec![
            "--lr applies to --algo ppo only and is ignored".to_owned(),
            "--entropy-coeff applies to --algo ppo only and is ignored".to_owned(),
        ]
    );
    let notes = run_args(&[
        "--algo",
        "ppo",
        "--imitation-lr",
        "1e-3",
        "--cosine-coeff",
        "0.5",
        "--gain-coeff",
        "0.1",
    ])
    .ignored_flag_notes();
    assert_eq!(notes.len(), 3);
    assert!(notes.iter().all(|n| n.contains("--algo imitation only")));
    let notes = run_args(&["--algo", "ppo", "--label-every", "2"]).ignored_flag_notes();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("PPO does not use these labels in its loss"));
}

#[test]
fn rejected_step_limit_reaches_config_and_is_reported_as_imitation_only() {
    assert_eq!(build_config(&run_args(&[])).max_rejected_steps, 8);
    for limit in [0, 1, 12] {
        let text = limit.to_string();
        let args = run_args(&["--max-rejected-steps", &text]);
        assert_eq!(build_config(&args).max_rejected_steps, limit);
    }
    assert!(parse(&["--max-rejected-steps", "-1"]).is_err());
    assert!(parse(&["--max-rejected-steps", "100001"]).is_err());
    assert!(parse(&["--max-rejected-steps"]).is_err());
    let notes = run_args(&["--algo", "ppo", "--max-rejected-steps", "8"]).ignored_flag_notes();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("--algo imitation only"));
}

#[test]
fn verified_action_coefficient_reaches_imitation_and_is_ignored_by_ppo() {
    assert_eq!(build_config(&run_args(&[])).imitation.action_coeff, 1.0);
    for coefficient in [0.0, 0.25, 2.0] {
        let text = coefficient.to_string();
        let args = run_args(&["--action-coeff", &text]);
        assert_eq!(build_config(&args).imitation.action_coeff, coefficient);
    }
    assert!(parse(&["--action-coeff"]).is_err());
    let notes = run_args(&["--algo", "ppo", "--action-coeff", "0.5"]).ignored_flag_notes();
    assert_eq!(
        notes,
        ["--action-coeff applies to --algo imitation only and is ignored"]
    );
}

#[test]
fn the_design_directory_flags_parse_and_are_cross_checked() {
    let defaults = run_args(&[]);
    assert!(defaults.design_dir.is_none() && !defaults.include_library);
    let directory = run_args(&["--design-dir", "corpus"]);
    assert_eq!(directory.design_dir.as_deref(), Some("corpus"));
    assert!(!directory.include_library);
    let both = run_args(&["--design-dir", "corpus", "--include-library"]);
    assert!(both.include_library);
    assert!(parse(&["--design-dir"]).is_err());
    assert!(
        parse(&["--include-library"])
            .unwrap_err()
            .contains("--design-dir")
    );
    for flags in [
        &["--fixture", "--design-dir", "corpus"][..],
        &["--fixture", "--design-dir", "corpus", "--include-library"],
    ] {
        assert!(parse(flags).unwrap_err().contains("--fixture"), "{flags:?}");
    }
}

#[test]
fn the_max_tiers_cap_is_limited_by_the_trainers_padding_limit() {
    let limit = pleochroma_ai::training::MAX_TIERS_LIMIT;
    assert_eq!(run_args(&["--max-tiers", "64"]).max_tiers, 64);
    assert_eq!(
        run_args(&["--max-tiers", &limit.to_string()]).max_tiers,
        limit
    );
    assert!(parse(&["--max-tiers", "0"]).is_err());
    assert!(parse(&["--max-tiers", &(limit + 1).to_string()]).is_err());
}

#[test]
fn the_long_cuda_imitation_command_parses() {
    let args = run_args(&[
        "--device",
        "cuda",
        "--algo",
        "imitation",
        "--mode",
        "retarget",
        "--tier",
        "m",
        "--iterations",
        "1200",
        "--batch-size",
        "1024",
        "--mini-batch",
        "512",
        "--updates-per-round",
        "16",
        "--label-every",
        "2",
        "--label-capacity",
        "50000",
        "--max-rejected-steps",
        "8",
        "--objectives",
        "mixed",
        "--same-material-frac",
        "0.25",
        "--max-tiers",
        "64",
        "--seed",
        "42",
        "--train-fidelity",
        "fast",
        "--validation-designs",
        "16",
        "--validation-every",
        "80",
        "--validation-fidelity",
        "full",
        "--objective-sweep",
        "--eval-test",
        "--eval-fidelity",
        "full",
        "--save-every",
        "1",
        "--save-stats",
        "retarget_m_v4.json",
        "--save-weights",
        "retarget_m_v4.ckpt.json",
    ]);
    assert_eq!(args.device, DeviceChoice::Cuda);
    assert_eq!(args.max_tiers, 64);
    let config = build_config(&args);
    assert_eq!(config.rollout_size, 1024);
    assert_eq!(config.imitation.mini_batch_size, 512);
    assert_eq!(config.imitation.updates_per_round, 16);
    assert_eq!(config.label_capacity, 50_000);
    assert!(config.objective_sweep);
}

#[test]
fn full_fidelity_mixture_is_explicit_bounded_and_forwarded() {
    assert_eq!(build_config(&run_args(&[])).full_fidelity_frac, 0.0);
    for value in ["0", "0.125", "1"] {
        let cfg = build_config(&run_args(&["--full-fidelity-frac", value]));
        assert_eq!(cfg.full_fidelity_frac, value.parse::<f32>().unwrap());
    }
    for value in ["-0.01", "1.01", "NaN", "inf", "oops"] {
        assert!(parse(&["--full-fidelity-frac", value]).is_err(), "{value}");
    }
    assert!(parse(&["--full-fidelity-frac"]).is_err());
}
