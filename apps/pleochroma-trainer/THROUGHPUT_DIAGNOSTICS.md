# Remote training throughput diagnostics

These diagnostics measure the production training and projection paths, and a separate optical
workload that does not depend on the trainer feeding it. They do not modify an existing run or its
checkpoints.

## Bottleneck

Geometry and scheduling dominate, upstream of the optics. Most CPU projection time goes to the
original meet-preservation gate, which refuses most of its calls, and to CPU projection fallbacks
that rarely succeed. Optical batches stay small because little optical work is ready. More VRAM or
a larger neural minibatch alone does not address this. The original acceptance gate remains
authoritative. CPU projection rescue is kept only for unsupported networks; on supported networks
rare rescued proposals are given up to avoid repeated expensive failed searches.

Cumulative service counters overlap across workers and nested calls. They are not elapsed time.
Compare the difference of a counter within one process and phase.

## Current design

The current environment revision is `cuda-env-v7` (`CUDA_ENVIRONMENT_MATH`). Its behavior:

- The conservative interval prefilter intersects Cartesian and dual-basis enclosures. It skips the
  CPU gate only when impossibility is proved. Distinct possible trials still need full gate
  evaluations.
- Active-bound QR pins blocked coordinates and re-solves the free subspace, including activation
  cascades of any length. Accepted results still pass the exact original meet gate and the
  requested motion bounds.
- Direct-mast planes rebuild deduplication and provenance per candidate with production f32
  arithmetic. Matching plane counts alone never reuse an alias map.
- Rollout streams are independent. Shared stage admission and the CPU pool stay bounded; a busy
  stream borrows spare capacity. Completed probe values hold no GPU tickets, and a round still waits
  for its fixed per-slot quotas.
- Inference and imitation use bucketed tier and row shapes. Zeroed padding never reaches a masked
  computation, and small batches and epoch tails keep their statistical weight.
- Optical and construction width caches are separate, so they do not evict one another. Optical
  collection drains queued requests without a fill delay.
- Neural updates are short compared with rollout work once compiled. Initialization, warmup and
  steady-state iterations are reported separately.
- Possible line trials without residual improvement are deferred, not discarded, and exact duplicate
  deferred trials are removed within a request. Every remaining possible trial still needs original
  certification before a numerical failure is returned. Deferred and proof-avoided counters are kept
  apart, so postponed checks are not counted as saved CPU work. Failing searches can stay expensive.

Resuming an older CUDA checkpoint keeps the model, optimizer, iteration, RNG and accepted episodes,
but clears its derived teacher, replay and oracle labels. The commands below are for the operator to
run; they are not evidence of a completed validation.

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
  --output /notebooks/throughput-a100-v6-01
```

Use a new output directory for each collection. Existing directories and archives
are refused. Send back **`/notebooks/throughput-a100-v6-01.zip`**, including failed or
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

The supported training device is the A100-80G (`sm_80`). A6000 is not a supported comparison point,
because the GPU gate-avoidance proof is directed-rounding `f64` (A6000 FP64 is 1/64 of FP32).
Preserve the workload before tuning worker count. To compare worker counts on the same machine, repeat a fresh collection with `--workers 9` and then `--workers 27`; these are
experiments, not claims that either is optimal.

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
  `total_tasks: 0` means the staged task total is unknown; it is not a zero-work
  claim. Human task counts show `completed/unknown`.
- Projection reports compare success/refusal, returned geometry and exact-gate
  behavior. Different valid CPU/CUDA solutions are a diagnostic difference, not
  automatically an error. Repeated exact requests expose positive/negative cache reuse.
- The geometry breakdown separates exact-gate rejection kinds from the reason
  the accelerated projector chose fallback. Bounded examples retain useful error
  detail without collecting unbounded unique messages.
- Optical output records request throughput, service counters and numerical
  consistency across CUDA queue depths. Its scalar-versus-batched comparison is
  CUDA-versus-CUDA, not evidence of CPU agreement. The separate numerical parity
  tests in the trainer README compare reference and CUDA behavior.
  A queue depth below device capacity may still
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

## Acceptance checks

Run the numerical, recovery and scheduler tests before timing on both GPU models.
Then collect the same corpus, seed and workload on each machine. Inspect:

- No accepted candidate violates original meet constraints or requested bounds.
  Unsupported networks retain explicit CPU fallback; supported projection misses
  do not enter CPU rescue. Miss reasons stay visible.
- Zero CUDA failures and no unexplained CPU optical fallbacks. Gate call counts
  and service time should fall on the troublesome designs without using an unsafe
  residual-only threshold to reject potentially valid moves.
- At least two **completed measured iterations** after warmup. A timed-out case
  cannot serve as a steady-state baseline. Keep its partial events and report the timeout rather
  than treating missing summaries as zero elapsed work.
- Lower wall time to completed rollout/update boundaries with the same settings.
  Compare per-phase counter differences; nested or concurrent service seconds must
  not be converted into percentages of elapsed time.
- Useful optical batching when multiple candidates are ready, fewer scheduling
  stalls, and bounded host/device memory. High GPU memory allocation or utilization
  alone is not success; do not enlarge batches solely to occupy 80 GB.
- Recovery resumes a valid committed boundary. Older revisions invalidate derived
  labels once, and a failed rollout cannot publish partially mutated runner state.

See the README for the full CPU/CUDA tests and strict Clippy commands. Additional
focused coverage for these changes is:

```sh
cargo test -j 1 --manifest-path crates/pleochroma-ai/Cargo.toml --lib training::
cargo test -j 1 --manifest-path apps/pleochroma-trainer/Cargo.toml --bin pleochroma-trainer-cli pool::
cargo test -j 1 --manifest-path apps/pleochroma-trainer/Cargo.toml --bin pleochroma-trainer-cli recovery::
cargo test -j 1 --manifest-path apps/pleochroma-trainer/Cargo.toml --bin pleochroma-trainer-cli rollout_stream::
```

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

The `diagnostic_active_bound` fixture shows why scaling an unconstrained direction can
collapse. The bounded-QR tests exercise re-solving the free subspace and sequential
activation beyond six coordinates. These focused tests complement the numerical CUDA
parity suite in the trainer README.

## Cold shapes and update attribution

The first rollout forward and the first updates are dominated by a cold-path cost (first-use
compilation and setup), so their host durations are not steady-state. They do not isolate compiler
time. Initial backend/model construction is
reported separately as `initialization_wall_secs`. Rollout scheduling diagnostics
split validation, inference-view creation, packing/upload, forward submission,
the existing bundled host readback, and unpacking. Imitation `stages` split batch
validation, gather, forward/loss submission, the existing loss readback, and
backward/optimizer work (including the existing gradient-norm readback). Lazy
execution and compilation may occur during a later readback. No diagnostic adds
a synchronization or a duplicate prewarm/training pass.

Rollout tier/row shapes use bounded power-of-two buckets. Imitation also buckets
prepared upload rows and gathered mini-batch rows, so an early replay growing by
one sample does not require a new exact row shape every round. Small replays stay
small. A reserved dummy row has zero features, tier masks and every label mask;
it is gathered only to fill padding. Loss denominators still count real target
support, including the per-sample action and direction denominators. Row selection,
row order, real sample weights and the number of optimizer steps are unchanged.
Prepared upload bucket capacity includes padding under the existing 32 MiB budget;
if a bucket/sentinel would exceed it, that window uses exact row counts instead.
A configured mini-batch larger than the budget remains one update.

Schema 2 explicitly distinguishes update attribution. With
`--sync-every-iteration`, the row's update metrics and zero-based update identifier
refer to the update completed by that sync. `production_reported_update` preserves
the original lagged production report. Without explicit sync, the row keeps that
production lag and says `update_attribution: production_install_lag`.
`sync_complete.installed_update`, `warmup_drained_update`, `final_drained_update`
and `final_stats` retain the completed update after a drain, including the last
update when no following rollout exists to report it. Sync wait time remains a
separate wall duration and must not be added to the update thread duration as
independent work.

Build metadata records a build-script Git revision and dirty-state snapshot,
with `unknown` when Git/explicit build provenance is unavailable. Release builders
can supply `PLEOCHROMA_BUILD_REVISION` (full SHA) and `PLEOCHROMA_BUILD_DIRTY`
(`true`, `false` or `unknown`). Trainer/dependency source inputs and Git's resolved
HEAD, current ref, index and existing packed refs are watched; the full `.git`
directory is not. Dirty state is a repository-wide snapshot at that build, rather
than a cryptographic source fingerprint or proof about later checkout changes.
`runtime_source_checkout` separately reports the original source checkout when
available; it does not prove the contents of an already built binary. Metadata
contains no Git paths, remotes, status entries or user identity. These timing and
packing diagnostics provide no CPU/CUDA numerical-parity guarantee; retain
the numerical and independent-validation comparisons when assessing parity.

Recovery snapshots store `imitation_objective_revision` independently of the
last update's telemetry. A revision 1 snapshot keeps its learned weights, Adam moments and
replay, and emits a migration warning before revision 2 gradients. Training continues under
the revision 2 objective; the old objective's bitwise optimizer trajectory is not
reproduced. Saves record revision 2, so later resumes keep the warning history without
repeating the warning.

## Geometry and rollout admission

The environment revision is `cuda-env-v7`. Line-trial CPU gates are the largest service cost, and
optical batches stay near one candidate, although the independent optical workload shows capacity
for much larger batches. Service durations overlap and are not elapsed wall time.

The additional GPU check addresses the discrete junction requirement that a slab
overlap test cannot decide. It enumerates every triple of candidate surface planes
and encloses the production CPU triple solver's rounded operations with directed
interval arithmetic. Production's feasible, clustered arrangement representatives
are a subset of those points: clustering retains a representative rather than
averaging coordinates. If no enclosed point can satisfy an original junction's
support bounds, that junction cannot be mapped by the original gate. Only that
proven absence bypasses the CPU gate. A possible witness does not certify a move;
the original CPU gate still checks every accepted result, including topology,
girdle, shape and displacement constraints.

The proof uses an independently CPU-captured baseline and exact production
candidate planes. The older cheap prefilter is allowed to reject only when its
baseline metadata and candidate planes match the production equivalents exactly.
Uncertain arithmetic or metadata retains the CPU gate. The optional triple pass
has a deterministic work ceiling: at most 32,768 triples and 1,048,576 worst-case
junction flag/support checks per candidate, in addition to the 256-plane storage
limit. Larger cases go directly to the original CPU gate, before proof input
preparation or device uploads. A budget bypass is never a rejection or an
acceptance. The pass returns only small witness flags rather than the entire
triple arrangement. Completed refutations are cached within one immutable
episode projection by exact authored state and by ordered production-plane bits;
uncertain, interrupted, failed and capacity-bypassed proofs are not cached as
refutations. The plane cache retains at most eight keys (64 KiB coefficient
payload). Runtime health is checked before cache use.
CPU baseline preparation and GPU proof work have a cost, so the next measurement
must establish the net benefit rather than infer it from the reduction in calls.

Inspect the geometry counters alongside the gate-phase counters:

| Counter | Meaning |
| --- | --- |
| `arrangement_proof_policy` | `bounded-exact-refutations-v1` identifies this routing/cache fix independently of the unchanged environment arithmetic and recovery identity |
| `arrangement_probes` | Uncached gate candidates considered for the proof |
| `arrangement_refusals` | Candidates proven to lack a required junction witness |
| `arrangement_witnessed` | Every junction has a possible witness; CPU certification still required |
| `arrangement_uncertain` | Numerical or metadata uncertainty; CPU gate retained |
| `arrangement_capacity_bypasses` | Plane-count or triple/support-work ceilings bypassed the optional proof; original CPU certification remains required |
| `arrangement_tiles`, `arrangement_triples` | Submitted CUDA workload, including work before an interruption |
| `arrangement_service_ms` | Summed call wall time including preparation, dispatch, waiting and readback; concurrent calls overlap, so this is neither elapsed run time nor GPU kernel time |

When resuming a `ValidationPending` checkpoint, the generation stays unchanged
until that validation finishes. With 16 designs and the seven-objective sweep,
progress counts 112 cases; no rollout steps are expected during this phase.
Completed journal cases are reused without entering the work pool. New validation
journals also retain completed passes, episode steps and teacher-search work as
described below. An unfinished case from an older binary has no intermediate records;
it starts creating them on its first run. The same CUDA resume command keeps model, optimizer,
replay and compatible completed-case journals. This bounds proof cost, not total
validation duration; measure the remaining CPU gate and oracle work remotely.

### Incremental validation recovery

Training generations still contain the model, optimizer, replay and rollout state.
Validation progress is written separately, so saving a teacher probe never rewrites
the replay buffer or publishes a different training generation.

| Recovery boundary | Durable state |
| --- | --- |
| Completed validation case | Original ordered result, including all requested passes |
| Baseline environment | Exact episode snapshot and measured baseline, avoiding repeated optical scoring on restore |
| Each completed episode step | Accepted design, evaluation values, pilot/RNG, counters, accumulated timing and terminal state |
| Each completed teacher probe | Explicit completion, including unsuccessful/empty probes |
| Each completed gradient/rescue candidate | Result in canonical search order, with the search context needed to reconstruct the same winner and evaluation count |
| Completed noise/oracle pass | Model-independent reference, retained separately from iteration-specific policy results |

Partial records are versioned and bound to their exact evaluation identity. Each
replacement is written to a temporary file, flushed, and atomically published;
Unix directory synchronization makes the published boundary durable. Incomplete
temporary files are not treated as completed work. Corrupt or incompatible records
produce an error rather than silently mixing results.

The `reference-cases-v1` directory beside the iteration journals stores independent
case identities. A changed model can reuse a completed noise/oracle reference when
the design, material/context, seed, scoring fidelity, search settings and environment
revision still match. One failed case does not discard other cases' durable
references. The existing complete-cohort cache remains compatible. Per-case
`oracle_reference_cached` and `noise_reference_cached` distinguish partial cache
reuse; report-wide cached flags mean every successful case reused that reference.

Validation restoration explicitly uses validation fidelity, including when training
uses another fidelity. Geometry and meet-preservation acceptance checks remain
unchanged. Snapshots contain portable logical state; CUDA handles, pending kernels
and driver allocations are rebuilt, never deserialized. An interrupted probe or
candidate that did not finish publication is retried. Concurrent completions are
serialized for journal publication while decisions reduce in canonical order.
The policy worker is only initialized if a pending model or sampled pass still
needs inference; reference-only continuation avoids that cold policy setup.
Step timing retains elapsed time at completed step boundaries. Work time inside
an interrupted step is not reconstructed from saved teacher units, so timings
across interrupted runs are diagnostic rather than an exact accounting of total
compute time. Scores, RNG state, counters and search decisions are preserved.

This does not introduce background validation: a `ValidationPending` generation
still completes its pending validation before training advances. Persistence avoids
repeating completed work; it does not remove calculations that have not finished.
Keep the complete recovery directory, including validation and reference journals,
when moving a run to another machine. The `--device cuda --resume <prefix>` command
and existing training checkpoint format are unchanged.

Verify interruption/reopening at pass, step and teacher boundaries,
stochastic RNG continuation, changed identity/fidelity refusal, per-case reference
reuse across changed model weights, and legacy completed-case compatibility before
relying on unattended recovery. Compare scores, selected actions and evaluation
counts, not wall-clock durations. Cross-backend floating-point equivalence is not
promised.

Shared staged-work admission lets a busy stream borrow unused capacity,
while reserving admission for other registered idle streams. Total pending work remains
bounded by 128. Per-slot sample quotas, real-sample selection and checkpoint
boundaries are unchanged. A round still waits for its slowest unfinished quota;
allowing faster designs to steal those samples would bias the training data.
`rollout_scheduling` reports the inference stages and the final tail, including a
histogram by coordinator-observed slots with steps remaining. Those observations
include event-delivery delays and final respawn drain; they are not CPU utilization.

Run the ignored CUDA safety regressions before timing:

```sh
cargo test -j 1 --manifest-path crates/pleochroma-ai/Cargo.toml \
  --target-dir target/throughput-diagnostics --features cuda --lib \
  gpu_geometry::prefilter_tests -- --ignored --test-threads=1

cargo test -j 1 --manifest-path crates/pleochroma-ai/Cargo.toml \
  --target-dir target/throughput-diagnostics --features cuda --lib \
  gpu_geometry::projection_tests -- --ignored --test-threads=1
```

Then run the training, recovery, scheduler and diagnostic tests listed above and rebuild both
throughput executables. A comparison uses the same database, seed, sample counts and training
parameters as the previous collection. Check that accepted moves still pass independent preservation
validation, CUDA failures remain zero, and completed measured iterations improve. Do not increase
minibatches or worker counts in that comparison: it would obscure the effect of the change.
