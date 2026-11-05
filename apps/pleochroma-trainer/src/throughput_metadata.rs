//! Reproducibility metadata: explicit configuration, corpus identity and host information.

use crate::{
    dataset::{DatasetSplit, DbDesignRecord},
    runner::RunConfig,
    throughput_args::Args,
};
use serde_json::{Value, json};

pub(super) fn initial(args: &Args) -> Value {
    json!({
        "schema_version": 2, "program": "pleochroma-throughput-probe",
        "package_version": env!("CARGO_PKG_VERSION"),
        "build_revision": option_env!("PLEOCHROMA_BUILD_REVISION").unwrap_or("unknown"),
        "build_dirty": option_env!("PLEOCHROMA_BUILD_DIRTY").unwrap_or("unknown"),
        "build_provenance_note": "build-script snapshot watches trainer/AI source inputs and resolved Git HEAD/ref/index; dirty is repository-wide status at that build, not a source-content fingerprint; runtime checkout does not identify binary contents",
        "runtime_source_checkout": runtime_source_checkout(),
        "cuda_feature": cfg!(feature = "cuda"), "started_utc": crate::stats::rfc3339_utc_now(),
        "args": args, "run_config": config(&args.config()), "validation_enabled": false,
        "learning_rate_horizon_iterations": 300,
        "imitation_objective_revision": pleochroma_ai::training::imitation::IMITATION_OBJECTIVE_REVISION,
        "phase_snapshots": "sampled every 2 seconds; short phases may be absent; rollout forward and update durations come from production counters",
        "checkpoint_writes": false, "database_access": "read_only",
        "host": {"os": std::env::consts::OS, "arch": std::env::consts::ARCH,
            "available_parallelism": std::thread::available_parallelism().map_or(1, usize::from),
            "cpu_model": cpu_model(),
            "cuda_visible_devices": std::env::var("CUDA_VISIBLE_DEVICES").ok()},
        "timing": {"units": "host wall seconds unless field suffix says milliseconds",
            "gpu_utilization_measured": false, "heartbeat_interval_secs": 2,
            "update_schedule": "production overlap; optional explicit per-iteration sync measured separately",
            "update_attribution": "explicit-sync rows use the same iteration's completed update; production_reported_update retains the original lagged report; unsynchronized rows retain production attribution",
            "stage_timings": "host wall only; asynchronous execution and first-use compilation can spill into existing readbacks; compiler and GPU-kernel time are not isolated",
            "shape_policy": "power-of-two tier and row buckets, bounded by input layout and prepared-window budget; dummy rows have zero tier and target masks; real row order is preserved",
            "warmup_boundary": "explicit update drain before the first measured iteration",
            "measurement_window": "measured iterations plus final update drain and intervening logging",
            "soft_deadline": "checked between operations; cannot interrupt initialization or projection"}
    })
}

/// Do not serialize Git output containing paths, remote URLs, identities, or status entries.
/// A relocated standalone binary may have no source checkout; that is explicit rather than
/// borrowing a revision from an unrelated current working directory.
fn runtime_source_checkout() -> Value {
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let revision = git(&["rev-parse", "HEAD"]).filter(|revision| {
        matches!(revision.len(), 40 | 64) && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
    });
    let dirty = revision.as_ref().and_then(|_| {
        git(&["status", "--porcelain", "--untracked-files=normal"]).map(|status| !status.is_empty())
    });
    json!({"available": revision.is_some(), "revision": revision, "dirty": dirty})
}

fn cpu_model() -> Option<String> {
    if cfg!(target_os = "linux") {
        std::fs::read_to_string("/proc/cpuinfo")
            .ok()?
            .lines()
            .find_map(|line| {
                line.strip_prefix("model name")
                    .and_then(|tail| tail.split_once(':'))
                    .map(|(_, value)| value.trim().to_owned())
            })
    } else {
        std::env::var("PROCESSOR_IDENTIFIER").ok()
    }
}

fn config(cfg: &RunConfig) -> Value {
    json!({
        "mode": cfg.mode, "tier": cfg.tier, "device": cfg.device, "workers": cfg.workers,
        "effective_workers": cfg.effective_workers(), "rollout_size": cfg.rollout_size,
        "ppo": cfg.ppo, "corpus_mode": cfg.corpus_mode, "universal_spectrum": cfg.universal_spectrum,
        "fixed_target": cfg.fixed_target, "seed": cfg.seed,
        "validation_designs": cfg.validation_designs, "validation_every": cfg.validation_every,
        "max_step_deg": cfg.max_step_deg, "train_fidelity": format!("{:?}", cfg.train_fidelity),
        "full_fidelity_frac": cfg.full_fidelity_frac, "validation_fidelity": format!("{:?}", cfg.validation_fidelity),
        "objective_sweep": cfg.objective_sweep, "classical_budget": cfg.classical_budget,
        "algo": cfg.algo, "imitation": cfg.imitation, "label_every": cfg.label_every,
        "label_h_deg": cfg.label_h_deg, "label_capacity": cfg.label_capacity, "label_memory_mib": cfg.label_memory_mib,
        "explore_std": cfg.explore_std, "max_rejected_steps": cfg.max_rejected_steps,
        "line_search": cfg.line_search, "objectives": cfg.objectives,
        "same_material_frac": cfg.same_material_frac,
    })
}

pub(super) fn corpus(records: &[DbDesignRecord], split: &DatasetSplit) -> Value {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for record in records {
        let id = record.id.to_le_bytes();
        for data in [
            id.as_slice(),
            record.title.as_bytes(),
            record.filename.as_bytes(),
            &record.asc_bytes,
        ] {
            for byte in (data.len() as u64).to_le_bytes().iter().chain(data) {
                hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    json!({
        "records_fnv1a64": format!("{hash:016x}"), "record_count": records.len(),
        "loaded_entries": split.entries.len(), "train_indices": split.train,
        "validation_indices": split.val, "test_indices": split.test,
        "max_tiers": split.max_tiers, "split_seed": split.split_seed,
        "skipped": split.skipped.iter().map(|entry| json!({"title": entry.title, "reason": entry.reason})).collect::<Vec<_>>(),
    })
}

pub(super) fn entry(split: &DatasetSplit, index: usize) -> Value {
    let entry = &split.entries[index];
    json!({"index": index, "id": entry.id, "title": entry.title,
        "filename": entry.filename, "tiers": entry.design.tiers.len()})
}
