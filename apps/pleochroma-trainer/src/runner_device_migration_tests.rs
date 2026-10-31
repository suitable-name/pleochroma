//! Source regressions for atomic device and environment migration.
use super::*;
use crate::runner::{CPU_LABEL, imitation::LabelledSample, tests::fixture_runner_with};
use pleochroma_ai::env::{ContinuousTierAction, OracleOutcome, assemble_label};

fn env_address(env: &EnvKind) -> *const () {
    match env {
        EnvKind::Optimizer(env) => std::ptr::from_ref(env.as_ref()).cast(),
        EnvKind::Retarget(env) => std::ptr::from_ref(env.as_ref()).cast(),
    }
}

fn step_count(env: &EnvKind) -> usize {
    match env {
        EnvKind::Optimizer(env) => env.step_count(),
        EnvKind::Retarget(env) => env.step_count(),
    }
}

fn add_reference_label(runner: &mut TrainingRunner) {
    let slot = &mut runner.slots[0];
    let label = assemble_label(
        vec![None; slot.obs.tiers.len()],
        0,
        OracleOutcome::default(),
    );
    slot.label_cache
        .put(slot.env.current_design(), 0.1, &[1.0], label.clone());
    runner.aggregate.push(LabelledSample::new(
        slot.obs
            .try_flatten(runner.split.max_tiers)
            .expect("observation"),
        slot.obs.free_tier_mask(),
        &label,
    ));
    runner.stats.labelled_samples = runner.aggregate.len();
}

#[test]
fn cpu_device_switch_keeps_episode_allocation_and_reference_labels() {
    let mut runner = fixture_runner_with(|_| {});
    add_reference_label(&mut runner);
    let address = env_address(&runner.slots[0].env);
    let observation = runner.slots[0].obs.clone();
    let current = runner.slots[0].env.current_design().clone();
    assert_eq!(
        runner.set_device(DeviceChoice::Cpu).expect("CPU switch"),
        CPU_LABEL
    );
    assert_eq!(env_address(&runner.slots[0].env), address);
    assert_eq!(runner.slots[0].obs, observation);
    assert_eq!(*runner.slots[0].env.current_design(), current);
    assert_eq!(runner.aggregate.len(), 1);
    assert!(
        runner.slots[0]
            .label_cache
            .get(&current, 0.1, &[1.0])
            .is_some()
    );
    assert!(runner.execution.is_none());
    assert!(runner.progress.snapshot().environment.is_none());
}

#[test]
fn migration_reconstruction_keeps_accepted_episode_and_step_progress() {
    let mut runner = fixture_runner_with(|_| {});
    let action = ContinuousTierAction::new(vec![0.0; runner.slots[0].obs.tiers.len()]);
    let step = runner.slots[0].env.step_scaled(&action, &[1.0]);
    runner.slots[0].obs = step.observation;
    let before = &runner.slots[0];
    let staged = runner
        .reconstruct_device_episode(before)
        .expect("staged episode");
    assert_eq!(staged.build_observation(), before.obs);
    assert_eq!(staged.current_design(), before.env.current_design());
    assert_eq!(
        staged.current_score().to_bits(),
        before.env.current_score().to_bits()
    );
    assert_eq!(
        staged.base_score().to_bits(),
        before.env.base_score().to_bits()
    );
    assert_eq!(step_count(&staged), step_count(&before.env));
    assert!(step_count(&staged) > 0);
}

#[cfg(not(feature = "cuda"))]
#[test]
fn unavailable_cuda_keeps_current_episode_device_and_labels() {
    let mut runner = fixture_runner_with(|_| {});
    add_reference_label(&mut runner);
    let address = env_address(&runner.slots[0].env);
    let observation = runner.slots[0].obs.clone();
    let warnings = runner.warnings.clone();
    assert!(runner.set_device(DeviceChoice::Cuda).is_err());
    assert_eq!(runner.device_label, CPU_LABEL);
    assert_eq!(runner.cfg.device, DeviceChoice::Cpu);
    assert_eq!(env_address(&runner.slots[0].env), address);
    assert_eq!(runner.slots[0].obs, observation);
    assert_eq!(runner.warnings, warnings);
    assert_eq!(runner.aggregate.len(), 1);
    assert!(runner.execution.is_none());
    assert!(runner.progress.snapshot().environment.is_none());
}

#[cfg(feature = "cuda")]
#[test]
#[ignore = "requires explicitly authorized NVIDIA CUDA/NVRTC execution"]
fn cuda_round_trip_detaches_old_service_and_invalidates_arithmetic_labels() {
    let mut runner = fixture_runner_with(|_| {});
    add_reference_label(&mut runner);
    let observation = runner.slots[0].obs.clone();
    let design = runner.slots[0].env.current_design().clone();
    let score = runner.slots[0].env.current_score().to_bits();
    runner.set_device(DeviceChoice::Cuda).expect("CUDA switch");
    assert!(runner.execution.is_some());
    assert!(runner.progress.snapshot().environment.is_some());
    assert!(runner.aggregate.is_empty());
    assert!(
        runner.slots[0]
            .label_cache
            .get(&design, 0.1, &[1.0])
            .is_none()
    );
    assert_eq!(runner.slots[0].obs, observation);
    assert_eq!(*runner.slots[0].env.current_design(), design);
    assert_eq!(runner.slots[0].env.current_score().to_bits(), score);
    let old = runner.execution.clone().expect("CUDA service");
    let address = env_address(&runner.slots[0].env);
    runner
        .set_device(DeviceChoice::Cuda)
        .expect("same CUDA route");
    assert!(Arc::ptr_eq(
        runner.execution.as_ref().expect("same service"),
        &old
    ));
    assert_eq!(env_address(&runner.slots[0].env), address);
    runner.set_device(DeviceChoice::Cpu).expect("CPU detach");
    assert!(runner.execution.is_none());
    assert!(runner.progress.snapshot().environment.is_none());
    assert_eq!(
        Arc::strong_count(&old),
        1,
        "no slot or progress observer retains the old accelerator"
    );
    assert_eq!(runner.slots[0].obs, observation);
    assert_eq!(*runner.slots[0].env.current_design(), design);
    assert_eq!(runner.slots[0].env.current_score().to_bits(), score);
}

#[cfg(feature = "cuda")]
#[test]
#[ignore = "requires explicitly authorized NVIDIA CUDA/NVRTC execution"]
fn later_episode_staging_failure_discards_trainers_without_partial_attachment() {
    let mut runner = fixture_runner_with(|cfg| cfg.workers = 2);
    add_reference_label(&mut runner);
    let first = env_address(&runner.slots[0].env);
    let second = env_address(&runner.slots[1].env);
    let valid_entry = runner.slots[1].entry;
    runner.slots[1].entry = usize::MAX;
    assert!(runner.set_device(DeviceChoice::Cuda).is_err());
    assert_eq!(runner.device_label, CPU_LABEL);
    assert_eq!(runner.cfg.device, DeviceChoice::Cpu);
    assert_eq!(env_address(&runner.slots[0].env), first);
    assert_eq!(env_address(&runner.slots[1].env), second);
    assert!(runner.execution.is_none());
    assert!(runner.progress.snapshot().environment.is_none());
    assert_eq!(runner.aggregate.len(), 1);
    runner.slots[1].entry = valid_entry;
    runner
        .set_device(DeviceChoice::Cpu)
        .expect("staged CUDA trainer was discarded");
}
