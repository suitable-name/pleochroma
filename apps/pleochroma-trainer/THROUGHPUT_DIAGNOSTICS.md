# Remote training throughput diagnostics

These diagnostics investigate the A100 run reported on 2026-10-30. They exercise the
production training and projection paths, and a separate optical workload that does
not depend on the trainer feeding it. They do not modify an existing run or its
checkpoints. The implementation was reviewed from source; it has **not** been built
or executed by the implementation agent.

## What the supplied measurements establish

The resumed run switched from `cuda-env-v2` to `cuda-env-v3` after iteration 5. The
first five entries in its statistics are historical. The replay count falling at
iteration 6 is consistent with the reported invalidation of derived labels after
the arithmetic change; model weights and optimizer state were retained.

In the later v3 snapshot:

| Measurement | Observed value | Interpretation |
| --- | ---: | --- |
| Optical batch capacity | 128 candidates | An upper bound, not actual supplied work |
| Mean / maximum optical batch | 1.048 / 3 candidates | Optical service receives very small batches |
| Optical queue / active requests | 0 / 0 | No optical backlog at that instant |
| CPU projection fallback calls | 4,385 | Frequent reference searches remain |
| Completed fallback successes / completions | 7 / 4,383 | About 0.16%; two calls were still in flight |
| Summed CPU fallback service | 6,038.7 seconds | Concurrent service time, not elapsed time |
| Exact CPU gate calls / original refusals | 171,771 / 168,505 | About 98.1% refused by the original gate |
| Summed exact CPU gate service | 1,786.7 seconds | Nested in other projection timings |
| Summed CUDA residual service | 24.24 seconds | Host service timing, not kernel event timing |
| Optical service | 3.09 seconds | Small compared with the geometry work |

Two snapshots within iteration 10 are particularly useful because they cover the
same running process: over **87.65 seconds**, 81 steps completed, another 294 CPU
fallbacks started, and the number of successful fallbacks did not increase.
Optical service increased by only **0.244 seconds**. CPU fallback service increased
by 294.5 seconds summed across concurrent calls.

This is evidence of geometry and scheduling starvation upstream of optics. It is
not evidence that allocating more VRAM or increasing neural minibatch size alone
will improve throughput. Nor does it justify removing the exact gate or all CPU
fallbacks: some fallback searches still produce valid solutions.

Source review identified additional questions to measure:

- Expanded projection backtracking may call the exact CPU gate for many distinct
  trial states before finding progress. Exact-state caches cannot reuse different
  states safely. The new refusal breakdown identifies which invariant rejects them.
- A correction can be stopped by an active bound even when a different feasible
  direction may exist. Projection probes expose bound violations and compare both
  solvers; they do not change the solver or relax meet preservation.
- Preparation of speculative candidates and the next batched-forward pass can
  still wait for a slow slot. Progress events record the oldest active task and
  completion counters alongside the phase and wall time.
- Neural updates are much shorter than rollout work after first-use compilation.
  Cold initialization, warmup and steady-state iterations must be kept separate.

## Build on the remote machine

From the repository root, build **sequentially**. These commands use a single Cargo
build job and the same explicit target directory so the executable paths below are
unambiguous. They require the CUDA toolchain/runtime used for the existing trainer.

```sh
cargo build --release -j 1 \
  --manifest-path apps/pleochroma-trainer/Cargo.toml \
  --target-dir target/throughput-diagnostics \
  --features cuda --bin pleochroma-throughput-probe

cargo build --release -j 1 \
  --manifest-path crates/pleochroma-ai/Cargo.toml \
  --target-dir target/throughput-diagnostics \
  --features cuda --example optical_throughput
```

The Python collector uses only the standard library. `nvidia-smi` is optional but
strongly useful for the utilization and device-memory record. It never invokes
Cargo or builds anything itself.

## First collection on the A100

Stop other training on that GPU so device-wide telemetry is interpretable. Run:

```sh
python3 apps/pleochroma-trainer/scripts/collect_throughput.py \
  --probe target/throughput-diagnostics/release/pleochroma-throughput-probe \
  --optics target/throughput-diagnostics/release/examples/optical_throughput \
  --database /notebooks/facet_diagrams.sqlite \
  --output /notebooks/throughput-a100-01
```

Use a new output directory for each collection. Existing directories and archives
are refused. Send back **`/notebooks/throughput-a100-01.zip`**, including failed or
timed-out cases. It contains reports and logs, not the database, model weights or
training checkpoints. Design titles/filenames and local paths are included for
matching a slow case to the corpus.

The cases run one at a time:

1. **CUDA pipeline:** fresh model M, imitation, retarget, seed 42; 256 rollout
   steps, minibatch 128, 16 updates, labels every second step. One warmup iteration
   followed by two measured iterations. Synchronizing each iteration mirrors the
   training-update wait at a checkpoint boundary; checkpoint disk I/O is excluded.
   The learning-rate schedule retains the reported 300-iteration horizon.
   Validation is not run.
2. **Paired projection:** loaded corpus entries 69, 215 and 283, using the same
   explicit actions for the CPU reference and CUDA-assisted route, repeated twice.
   It uses the production Retarget preservation policy and a 0.5-degree step limit.
   These indices came from the supplied slow-task records. Reports include their
   resolved identities; changed corpus ordering can point to different designs.
3. **CUDA optics capacity:** 4,096 independent requests per timing case at in-flight
   counts 1, 8, 32 and 128, with first-use timing, two warmed repeats and a
   mixed-fixture identity audit. The ticket count remains bounded by the selected
   in-flight limit. This establishes
   what the optical service does when work is available, without waiting for
   training projection.

The collector imposes a **30-minute hard timeout per child**, including initialization.
It terminates only the process group it launched, retains partial artifacts, and
continues with the other cases. Three default cases can therefore take up to about
90 minutes if all hit the limit. Override with `--timeout-seconds 600` for a shorter
initial diagnostic. A timeout is not a numerical failure or proof of deadlock.
The binaries' own time limits are cooperative; a synchronous geometry/CUDA call
may outlast them. The collector provides the outer process timeout.

While running, the collector prints a status every 30 seconds. Case directories
contain stdout/stderr logs, flushed diagnostic events and `hardware.jsonl` samples.
Ctrl+C stops the current child and packages the partial collection.

## Comparisons after the first collection

Repeat the same command on A6000 with a different output name and the same database
and source revision. Preserve the workload before tuning worker count. To compare
worker counts on the same machine, repeat a fresh collection with `--workers 9`
and then `--workers 27`; these are experiments, not claims that either is optimal.

`--include-cpu` adds a full CPU pipeline run using the same configuration. It can
be slow. The default targeted projection case already includes the CPU reference,
so a complete CPU pipeline is not required for the initial report.

For a longer warmed sample, use `--iterations 5 --warmup 1`. Warmup actually runs
training and changes the model; it is excluded from steady-state timing rather
than rolled back. Each fresh case starts from the configured seed. CUDA and CPU
training need not produce bitwise identical optimization trajectories.

## Interpreting the output

- `host.json` records executable SHA-256 hashes, CPU affinity/quota when available,
  configuration, database path/size and relevant device/thread environment variables.
  Runner metadata records the loaded corpus identity and configuration.
- Pipeline events preserve initialization, per-iteration data and live task
  progress. They distinguish warmup and measured work, plus explicit update waits.
- Projection reports compare success/refusal, returned geometry and exact-gate
  behavior. Different valid CPU/CUDA solutions are a diagnostic difference, not
  automatically an error. Repeated exact requests expose positive/negative cache reuse.
- The new geometry breakdown separates exact-gate rejection kinds from the reason
  the accelerated projector chose fallback. Bounded examples retain useful error
  detail without collecting unbounded unique messages.
- Optical output records request throughput, service counters and numerical
  consistency across queue depths. A queue depth below device capacity may still
  be optimal; actual batch size and wall time decide.
- `hardware.jsonl` records process CPU use and GPU utilization/memory/power. CPU
  100% means one logical CPU. GPU metrics are device-wide and sampled, so very short
  kernels can be missed and other GPU processes contaminate the result. Even 4,096
  requests may finish quickly at high concurrency: use measured request throughput
  and batch counters rather than judging those short cases by a utilization sample.
- Cumulative service counters overlap across workers and nested operations.
  Do not sum them into an elapsed-time estimate or interpret them as CUDA event
  timings. Compare counter differences within the same process and phase.
- A targeted probe starts from the imported design, not an evolved episode from
  the resumed checkpoint. Failure to reproduce that episode's slow projection
  does not rule out the original bottleneck. The real pipeline case complements it.
- The optical capacity fixture does not measure full training, model convergence,
  every material, or full-fidelity validation. Successful execution cannot establish
  production readiness by itself.

## Tests for the diagnostic tools

The collector tests do not launch training or require CUDA:

```sh
python3 -m unittest discover -s apps/pleochroma-trainer/scripts -p 'test_collect_throughput.py' -v
```

The following filters exercise the diagnostic contracts, gate-counter attribution,
bounded-direction reproducer, CLI/output safety and optical accounting. They do not
start the benchmark binaries' main functions. Run the commands sequentially and
separately from timing collection:

```sh
cargo test -j 1 --manifest-path crates/pleochroma-ai/Cargo.toml \
  --target-dir target/throughput-diagnostics --features cuda --lib \
  env::projection_benchmark::tests -- --test-threads=1

cargo test -j 1 --manifest-path crates/pleochroma-ai/Cargo.toml \
  --target-dir target/throughput-diagnostics --features cuda --lib \
  gpu_geometry::projection_diagnostics::tests -- --test-threads=1

cargo test -j 1 --manifest-path crates/pleochroma-ai/Cargo.toml \
  --target-dir target/throughput-diagnostics --features cuda --lib \
  diagnostic_active_bound -- --test-threads=1

cargo test -j 1 --manifest-path crates/pleochroma-ai/Cargo.toml \
  --target-dir target/throughput-diagnostics --features cuda \
  --example optical_throughput -- --test-threads=1

cargo test -j 1 --manifest-path apps/pleochroma-trainer/Cargo.toml \
  --target-dir target/throughput-diagnostics --features cuda \
  --bin pleochroma-throughput-probe throughput_ -- --test-threads=1
```

The active-bound test documents the current solver limitation; it does not claim
to repair it. These focused tests complement the existing numerical CUDA parity
suite in the trainer README rather than replacing that suite.
