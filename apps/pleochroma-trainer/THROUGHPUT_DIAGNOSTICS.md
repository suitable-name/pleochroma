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
will improve throughput. The original acceptance gate remains authoritative. The
current implementation deliberately retains CPU projection rescue only for
unsupported networks, accepting the loss of rare rescued proposals on supported
networks to avoid repeated expensive failed searches.

The changes to compare against that baseline are:

- The conservative interval prefilter now intersects Cartesian and dual-basis
  enclosures. It may skip the CPU gate only when impossibility is proved. Distinct
  possible trials can still require distinct full gate evaluations.
- Active-bound QR now pins blocked coordinates and re-solves the free subspace,
  including activation cascades longer than six passes. Accepted results still
  pass the exact original meet gate and requested motion bounds.
- Direct-mast planes rebuild deduplication/provenance per candidate with production
  f32 arithmetic. Matching plane counts alone no longer reuse an alias map.
- Independent rollout streams remove index-lane blocking. Shared stage admission
  and CPU pool limits remain bounded; tail streams borrow spare capacity and
  environment respawns cannot bypass the pool. Completed probe values contain no
  live GPU tickets. Final iteration completion still waits for all fixed quotas.
- Inference row/tier buckets and imitation tier buckets reduce shape variation;
  zeroed padding prevents unused values contaminating masked computations.
  Small training batches and epoch tails retain their original statistical weight.
- Optical and construction width caches no longer evict one another. Optical
  collection drains already queued requests without a deliberate fill delay.
- Neural updates are much shorter than rollout work after first-use compilation.
  Cold initialization, warmup and steady-state iterations must be kept separate.

The v5 arithmetic identity was `cuda-env-v5`. In addition to the v4 changes
above, possible line trials without residual improvement are deferred, not
discarded, and exact duplicate deferred trials are deduplicated within a request.
All remaining possible trials still require original certification before a
numerical failure. Separate deferred and proof-avoided counters prevent treating
postponed checks as saved CPU work. Failed searches can remain expensive.

Resuming older CUDA checkpoints
keeps model, optimizer, iteration, RNG and accepted episodes, but clears derived
teacher/replay/oracle labels. Keep workload settings identical for the first new
comparison. The following changes have only been statically reviewed; the commands
below are for the operator to run, not evidence of completed validation.

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
  `total_tasks: 0` means the staged task total is unknown; it is not a zero-work
  claim. Human task counts show `completed/unknown`.
- Projection reports compare success/refusal, returned geometry and exact-gate
  behavior. Different valid CPU/CUDA solutions are a diagnostic difference, not
  automatically an error. Repeated exact requests expose positive/negative cache reuse.
- The new geometry breakdown separates exact-gate rejection kinds from the reason
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

## Acceptance evidence for this revision

Run the numerical, recovery and scheduler tests before timing on both GPU models.
Then collect the same corpus, seed and workload on each machine. Inspect:

- No accepted candidate violates original meet constraints or requested bounds.
  Unsupported networks retain explicit CPU fallback; supported projection misses
  do not enter CPU rescue. Miss reasons stay visible.
- Zero CUDA failures and no unexplained CPU optical fallbacks. Gate call counts
  and service time should fall on the troublesome designs without using an unsafe
  residual-only threshold to reject potentially valid moves.
- At least two **completed measured iterations** after warmup. The original archive
  timed out during the first measured iteration; its partial step counts cannot
  serve as a completed steady-state throughput baseline. Retain its partial events
  and report timeouts rather than treating missing summaries as zero elapsed work.
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

The original `diagnostic_active_bound` fixture demonstrates why scaling an
unconstrained direction can collapse. New bounded-QR tests exercise re-solving
the free subspace and sequential activation beyond six coordinates. These focused
tests complement the existing numerical CUDA parity suite in the trainer README.

## Cold shapes and update attribution (v6)

The v5 A100 pipeline's first rollout forward accumulated roughly 80.5 seconds,
followed by 4.50 and 3.81 seconds. Its explicitly drained updates took roughly
58.8, 4.79 and 0.72 seconds. These host durations show a substantial cold-path
cost; they do not isolate compiler time. Initial backend/model construction is
reported separately as `initialization_wall_secs`. Rollout scheduling diagnostics
now split validation, inference-view creation, packing/upload, forward submission,
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
As before, a configured mini-batch larger than the budget remains one update.

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
packing diagnostics provide no new CPU/CUDA numerical-parity guarantee; retain
the numerical and independent-validation comparisons when assessing parity.

Recovery snapshots now store `imitation_objective_revision` independently of the
last update's telemetry. A legacy revision 1 snapshot intentionally retains
learned weights, Adam moments and replay, and emits a migration warning before
subsequent revision 2 gradients. This continues training under the new objective;
it does not claim the old objective's bitwise optimizer trajectory. New saves
record revision 2 so later resumes retain the warning history without adding the
same migration warning again.

## Geometry and rollout admission (v6)

The current environment identity is `cuda-env-v6`. The v5 archive completed its
pipeline measurement, but still spent 2,837 summed service seconds in line-trial
CPU gates, with 47,268 detached-junction refusals across the pipeline process.
These are overlapping service durations, not elapsed wall time. The optical
service received a mean batch of about 1.004 candidates, despite the independent
optical workload demonstrating capacity for much larger batches.

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

Inspect the new geometry counters alongside the existing gate-phase counters:

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
described below. An old unfinished case has no intermediate records to recover;
its first execution with the updated binary starts creating those records.
The same CUDA resume command can use this fix without discarding model, optimizer,
replay or compatible completed-case journals. This bounds proof cost, not total
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

The new recovery regressions are supplied for remote execution; they have not been
run locally. Verify interruption/reopening at pass, step and teacher boundaries,
stochastic RNG continuation, changed identity/fidelity refusal, per-case reference
reuse across changed model weights, and legacy completed-case compatibility before
relying on unattended recovery. Compare scores, selected actions and evaluation
counts, not wall-clock durations. Cross-backend floating-point equivalence is not
promised.

Shared staged-work admission now permits a busy stream to borrow unused capacity,
reserving admission for other registered idle streams. Total pending work remains
bounded by 128. Per-slot sample quotas, real-sample selection and checkpoint
boundaries are unchanged. A round still waits for its slowest unfinished quota;
allowing faster designs to steal those samples would bias the training data.
`rollout_scheduling` reports the inference stages and the final tail, including a
histogram by coordinator-observed slots with steps remaining. Those observations
include event-delivery delays and final respawn drain; they are not CPU utilization.

Run the new ignored CUDA safety regressions before collecting the v6 timings:

```sh
cargo test -j 1 --manifest-path crates/pleochroma-ai/Cargo.toml \
  --target-dir target/throughput-diagnostics --features cuda --lib \
  gpu_geometry::prefilter_tests -- --ignored --test-threads=1

cargo test -j 1 --manifest-path crates/pleochroma-ai/Cargo.toml \
  --target-dir target/throughput-diagnostics --features cuda --lib \
  gpu_geometry::projection_tests -- --ignored --test-threads=1
```

Then run the training, recovery, scheduler and diagnostic tests listed above and
rebuild both throughput executables. Use the same database, seed, sample counts
and training parameters as v5 for the first comparison. Check that accepted moves
still pass independent preservation validation, CUDA failures remain zero, and
completed measured iterations improve. Do not increase minibatches or worker
counts in that comparison: it would obscure the effect of these changes.
