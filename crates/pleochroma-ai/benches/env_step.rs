//! Benchmarks one RL environment step, the unit the trainer's throughput depends on.
//!
//! A step is a full candidate evaluation: apply the angle deltas, solve, build the mesh,
//! manufacturability gate, girdle gate, then the objective at the configured fidelity
//! (`Fast`: one pose; `Full`: the 724-pose tilt sweep), plus building the observation.
//! The fixture is the concave test design (`Design::concave_fixture`, a closed stone with
//! four movable tiers). The environment is `reset` before each timed step so every sample
//! starts from the same state.
//!
//! Run from `crates/pleochroma-ai` with `cargo bench --bench env_step`.

use divan::black_box;
use pleochroma_ai::env::{ContinuousTierAction, GemstoneRlEnv, OptimizerEnv, OptimizerEnvConfig};
use pleochroma_cut_core::{Design, optimize::ObjectiveFidelity};

fn main() {
    divan::main();
}

/// A deterministic action: small alternating-sign deltas on every tier.
fn action_for(design: &Design) -> ContinuousTierAction {
    let deltas = (0..design.tiers.len())
        .map(|i| if i % 2 == 0 { 0.25 } else { -0.25 })
        .collect();
    ContinuousTierAction::new(deltas)
}

/// One `step` from a freshly reset environment at `fidelity`.
fn run(bencher: divan::Bencher, fidelity: ObjectiveFidelity) {
    let design = Design::concave_fixture();
    let action = action_for(&design);
    let config = OptimizerEnvConfig {
        fidelity,
        ..OptimizerEnvConfig::default()
    };
    bencher
        .with_inputs(|| {
            let mut env = OptimizerEnv::new(design.clone(), config.clone())
                .expect("the fixture is a closed, movable stone");
            let _ = env.reset(0);
            env
        })
        .bench_local_refs(|env| black_box(env.step(black_box(&action))));
}

/// One step at `Fast` fidelity.
#[divan::bench(sample_count = 100)]
fn step_fast(bencher: divan::Bencher) {
    run(bencher, ObjectiveFidelity::Fast);
}

/// One step at `Full` fidelity.
#[divan::bench(sample_count = 5)]
fn step_full(bencher: divan::Bencher) {
    run(bencher, ObjectiveFidelity::Full);
}
