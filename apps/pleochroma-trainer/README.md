# pleochroma-trainer

Standalone Slint GUI and headless CLI for training the two `pleochroma-ai` policies. Training is
not wired into the editor, web app, mobile app, or worker services. A trained checkpoint must be
integrated separately. The GUI and CLI share the trainer loop and model format.

## Tasks and data

The two tasks have distinct checkpoint identities: **Optimizer** changes tier angles while
scoring in the sampled source material; **Retarget** scores in a target material and applies the
retarget preservation policy. Retarget runs may include same-material episodes, but those are a
robustness mixture and do not certify cross-material capability. Loaders reject a checkpoint
whose mode, tier, or observation layout does not match.

Scores are lower-is-better and combine optical quality, measured yield when available, optional
tone goals, and sampled shape context. Seven objective presets are trainable: Balanced,
Brilliance, Low Windowing, Low Extinction, Keep Weight, Lighten Dark Rough, and Intensify Pale
Rough. The two tone tasks use catalog materials with real absorption; episode-seeded material,
lighting, and shape context are recorded in evaluation case artifacts.

The CLI reads the design library read-only through `DIAGRAM_DB_PATH`, or searches the working
directory and up to three parents for `facet_diagrams.sqlite`. It loads `.asc` and `.psdd`
attachments. `--design-dir PATH` loads the regular files directly inside a directory
(no recursion, no symbolic links; sibling files with the same stem share one ID) instead of the
library; add `--include-library` to use both. Native files keep their authored rough, physical
size, relations, dimensional targets, concave tools, crystal-axis effects, frosting and material
(an embedded custom material is restored; an unknown custom name is refused rather than replaced
by Diamond), and draft files are refused. Legacy ASC files carry none of that: the trainer infers
an enclosing block rough and draws one physical width per record from a fixed list, labelled as
augmentation in the evaluation artifacts. Invalid, oversized or unrepresentable designs are
reported as skips (an observation capacity is exceeded by a tier with more than 128 index
positions, an angle relation of more than 32 nodes, more than 16 named-meet participants, more than
32 custom rough planes or more than eight concave tools). Each run writes a corpus coverage
sidecar next to `--save-stats` (`<stats-stem>.coverage.json`): ASC/native counts, geometric
families, tier bands, dimensional targets, native materials, per-split capability counts and the
analytical-scope notes of effects the objective does not evaluate. Retarget targets are drawn from every built-in material of the renderer's catalogue
with a sampled crystal frame; native sources keep their authored material. Records sharing an entry ID or normalized geometry are connected into one
group before train/validation/test assignment, preventing attachments or duplicate cuts from
crossing splits; the grouping key is a conservative geometric family that ignores tier order,
equivalent gears, uniform scale, rotation and reflection. Validation uses a fixed cohort stratified
by format, tier band, concavity and relation programs, always drawn from the validation split.
The split seed is recorded. `--fixture` is a smoke test that deliberately shares
the same fixture across splits; it is not evidence of generalization and cannot produce a
best-validation checkpoint.

## Training behavior

The model predicts seven sensitivity components per tier: windowing, extinction, brilliance loss,
yield loss, lighter-tone loss, deeper-tone loss, and shape distance. Unavailable labels are masked.
It also learns a zero-initialized per-tier action residual, so a useful verified move need not
be represented by a false derivative. The teacher compares its coordinate improvements with
gradient line-search candidates and, if all fail, tries at most eight additional coordinate/paired
attempts on two deterministically ranked tiers. This bounded search is not a global oracle.
Only improving, evaluated actions become behavior targets; failure supplies no action label.
The loss combines component Huber, verified squashed-action Huber (`--action-coeff`, default 1),
verified-action cosine, and gain Huber. Statistics include `action_loss` and `action_samples`
separately, so zero loss with no verified action labels is distinguishable from accurate fitting.
The shared observation includes the seven normalized objective coefficients and source/target
material, lighting, geometry, task, and fidelity context. Observation widths are exported by the
AI crate; the trainer does not duplicate their numeric dimensions.

With `--algo imitation`, labelled states are appended to a bounded aggregate and optimized by
mini-batches. The default `--updates-per-round` bounds work per update; `0` retains full-aggregate
epoch behavior. A deterministic bounded preparation path streams oversized full epochs in
windows instead of silently reducing requested training work. The imitation update intentionally
trains on the previous rollout while the next one is collected. With `--algo ppo`, the policy
trains on scalar rollout rewards. `--label-every` can still request teacher probes for diagnostics
in PPO mode, but those labels do not enter PPO's loss and add environment work.

An unchanged episode state reuses its last measured teacher label instead of repeating the
finite-difference probes and oracle search. Reuse requires the exact same design, probe size,
and ordered line-search scales, within the same episode and scoring context. It changes neither
the label sampling schedule nor the targets; the policy action still runs through the full
meet-preservation and scoring path. The cache holds at most one label per slot and starts empty
when the slot gets a new episode. Reused labels add no simulator evaluations to telemetry.
Only fresh, informative labels are added to the imitation aggregate: repeatedly rejecting
actions at the same state no longer fills replay with copies of that state. Cached labels
still contribute to teacher-request telemetry, so `labels_this_round` is not a count of new
replay entries. `diagnostics.labels_added` reports those entries separately.

Imitation training restarts an episode after eight consecutive rejected actions by default.
`--max-rejected-steps N` changes this sampling limit; zero disables it. Acceptance resets the
streak. This spends collection on new states rather than an entire episode stuck on one
design. It does not relax meet preservation, claim the stone is optimal, or shorten held-out
evaluation or PPO episodes. More fresh labels can cost more CPU time per step; the intended
benefit is more distinct supervised states, not a guaranteed increase in raw steps/second.

Environment slots are capped by rollout size and are scheduled on one persistent bounded Rayon
pool. Line searches start serial while enough slots remain to occupy the pool. As slots finish,
unstarted candidate scales may share spare threads in that same pool. Results are consumed in
scale order, preserving first-improvement acceptance without creating per-slot OS threads. A
single geometry solve already in progress cannot be split by this scheduling change. The
banner and telemetry report configured workers, effective slots, actual pool width, backend, and
debug-build status. Timings are host phase measurements, not GPU utilization: they include rollout
forward, data packing, update bodies, and weight export, with asynchronous device work potentially
crossing phase boundaries. Component support counts show how many selected labels train each of
the seven heads. Candidate evaluation counts retain their sequential-prefix definition;
speculative candidates after the accepted scale can consume additional physical work.

The scorer shares planar mesh construction and measurement and reuses carved meshes for
volume. A proposal that projects exactly onto the current stone reuses its known evaluation
and remains valid geometry; it still passes all gates and is not accepted as an improvement.
Teacher probes use the same coupled projection radius as real actions. A feasible identity
probe supplies a measured zero response rather than a missing derivative side.

Iteration records and stats include `diagnostics`: preservation failures (also included in
the legacy unsafe bucket), exact unchanged proposals, new replay labels, cached label requests,
and stalled episode restarts. Proposal diagnostics describe the accepted candidate or the
first attempted scale when all scales fail; they do not enumerate every speculative candidate.

Training and validation fidelity are separate CLI settings. Validation uses a fixed cohort;
`--objective-sweep` evaluates each selected design under all seven named objectives. Coverage
reports show missing objectives and spectrum materials rather than implying that a small random
cohort covered every material. `--classical-baseline N` optionally runs the public coordinate
optimizer with a separate nominal per-design candidate budget (a candidate pair can exceed it),
task-appropriate retarget preservation,
and matching baseline material orientation/size, objective, shape, and lighting context. Core
search fixes absorption scale at that baseline and uses its own Fast-search/Full-ranking schedule;
AI episodes recalibrate size at each candidate and use their configured fidelity. The sidecar
records these comparator differences explicitly, and both endpoints are rescored by the same AI
evaluator at report fidelity. This is a comparator, not an identical-search benchmark.

Cases that actually run the teacher reference also record `oracle_final_score`,
`oracle_evaluations`, `oracle_steps`, and `oracle_seconds`. The count includes teacher probes,
search, and accepted-action replay, but excludes reset scoring; elapsed time includes reset. These fields remain null when only a cached mean oracle result is reused.

## Progress, artifacts, and checkpoints

During rollout/update the CLI reports the current phase, completed steps and tasks, fresh/reused
labels, and the oldest active task with its age. It reports at phase changes and every ten seconds
during longer work, and writes the same live snapshot to `<save-stats path>.progress.json` (for
example, `retarget_m_imitation.json.progress.json`). This small diagnostic file is available
before the first iteration completes; it is not a checkpoint and does not save the replay buffer.
TTY output keeps a bounded multi-line dashboard with the latest completed loss and acceptance
figures. Redirected output retains full iteration records and periodic live snapshots without
cursor-control sequences. Initialization and validation also retain their heartbeat messages.
The live phase follows the main thread: an imitation iteration may finish while its queued
update continues in the background. Waiting for that update is reported separately when the
runner next needs its weights.

The CLI saves a full recovery snapshot before training starts, at the `--save-every` cadence,
before and after validation, and before final evaluation. `--save-every 0` disables only the
regular cadence. Each snapshot synchronizes the pending neural update, then saves the model,
Adam moments, update counter, shuffle/sampling RNGs, reward normalization, labelled replay,
active episodes, rollout policy copy, corpus identity, configuration and accumulated telemetry.
The learning-rate schedule and original total iteration count continue unchanged.

For `--save-weights /notebooks/retarget_m_layout7.ckpt.json`, the stable recovery target is
`/notebooks/retarget_m_layout7`. A small `retarget_m_layout7.latest.json` manifest points to the
last fully committed generation inside `retarget_m_layout7.run/`; that generation contains the
`resume.json` run state and its model/optimizer/runner payloads. The latest two complete
generations are retained. Keep the manifest and run directory together on persistent storage.
A lock prevents two trainer processes from writing the same run simultaneously.

After a remote restart, only this is needed:

```sh
./pleochroma-trainer-cli --no-update-check --device cuda \
  --resume /notebooks/retarget_m_layout7
```

`--resume /notebooks/retarget_m_layout7.latest.json` and the `.run` directory are accepted too.
The saved absolute database/directory/output paths are reused; `DIAGRAM_DB_PATH` need not be set
again. The corpus must still be available and unchanged. Only the device may be overridden;
changing training flags requires a new run. Resuming iteration 20 of a 300-iteration run starts
at iteration 21 and stops at 300, with the same validation and saving cadence. A pending validation
or test evaluation is retried before advancing. Resuming a completed run exits successfully
without training again. The dashboard also keeps the original iteration numbering.

Recovery is exact at committed iteration boundaries: work since the last save is repeated after
a crash, and a partially completed rollout/update is not a checkpoint. Device changes preserve
saved training state but do not promise bitwise identical floating-point results across CPU,
CUDA, WGPU, hardware or drivers. A snapshot is published only after its payload files are
written and flushed; interrupted saves do not replace the previously committed target.

Numbered model-only checkpoints remain available for inference and warm starts. The exact
`--save-weights` path receives final weights; when omitted, it defaults to `<save-stats stem>.ckpt.json`
in the statistics directory, with the same stable resume prefix. Statistics go to `--save-stats`, with per-design
validation/test results in `<stats-stem>-evaluations.json`.

Older model-only checkpoints cannot restore optimizer, replay or environment state. Use
`--warm-start PATH` with the desired training flags to begin a new run from those weights; its
iteration counter, optimizer, replay, RNG and reward scale start fresh. Choose new output names.
Model checkpoints still use schema v6 and observation layout v7; schema 5 and earlier are
incompatible with the larger embeddings. Do not alter schema or layout identifiers to force a load.

Each replay row holds a full flat observation (`3781 + padded_tiers * 384` floats, about 113 KB at 64
tiers and 212 KB at 128). Padding follows the largest retained design, within `--max-tiers`. The aggregate therefore keeps at most `--label-capacity` rows and at most
512 MiB (`MAX_LABEL_BYTES`), whichever is smaller (roughly 4,600 rows at 64 padded tiers). In
bounded-update mode, the snapshot is at most 64 MiB (roughly 570 rows at 64 tiers), with batches
clamped to available rows. Full-epoch mode (`--updates-per-round 0`) copies the retained aggregate
and streams prepared windows of at most 32 MiB; it deliberately visits all retained rows. The
512 MiB limit covers retained row payload and headers, not total process or device memory. The
banner prints the byte cap next to the row capacity.

### Diagnosing a long first iteration

`--batch-size` counts environment steps, not neural samples. With 64 slots, 4096 steps and
`--label-every 2`, the first round requests 2048 labels before it queues its first optimizer
update. A fresh label tries up to two meet-preserving projections and optical evaluations per
free tier, followed by the verified-action search (gradient scales and up to eight rescue attempts
if none improves). Coordinate results are reused. Each projection can itself need many geometry
solves. With native CUDA, ray tracing, spectral absorption and batched meet residuals use the GPU;
authored solving and final preservation checks still use the CPU. WGPU retains CPU environment
physics. The 64 slots share the CPU pool shown in the banner; on a five-core allocation only five
pool tasks execute at a time.
`--updates-per-round` bounds neural training work, not label collection. Checkpoints scheduled
by `--save-every` require completed iterations, so increasing the save frequency cannot recover
an unfinished first rollout; the initial recovery snapshot restarts that rollout from its beginning.

For an initial diagnosis, use the same backend and corpus with a small workload and separate
artifact names. This checks progress, not model quality:

```sh
./pleochroma-trainer-cli --no-update-check --device wgpu \
  --algo imitation --mode retarget --tier m --iterations 2 \
  --workers 10 --batch-size 32 --mini-batch 16 --updates-per-round 2 \
  --label-every 2 --objectives mixed --same-material-frac 0.25 \
  --max-tiers 64 --seed 42 --train-fidelity fast \
  --validation-designs 1 --validation-every 2 --validation-fidelity fast \
  --save-every 1 --save-stats retarget_probe.json \
  --save-weights retarget_probe.ckpt.json
```

A growing probe completion count means simulator work is advancing. A persistent `device
forward` phase without a completed task points to the forward/host-readback path, which includes
cold kernel compilation. A long `wait for previous update` phase points to the update thread;
it does not by itself distinguish slow training, compilation, or a blocked backend. Retain the
live sidecar and terminal output if a task stops completing. Compare a separate run with
`--device cpu`, or explicit `--device cuda` when built with CUDA support. Native CUDA also accelerates the teacher's optical evaluations and supported projection residuals; WGPU does not. Once phase timings are available, increase collection and neural
batch sizes separately; the large examples below are workload choices, not throughput guarantees.

### Current training commands (layout 7 / schema 6)

Start fresh after rebuilding; earlier weights cannot be warm-started into the larger embeddings.
Omit `--workers` initially: the default is three episode slots per available CPU core, all sharing
one bounded CPU pool (15 slots on five cores, 27 on nine). This is a starting configuration, not a
measured optimum. Native CUDA batches neural forwards, optical rays, spectral absorption and
supported meet residuals. Raising the slot count cannot eliminate the remaining CPU authoring
and preservation checks; inspect the execution counters before increasing workers.

For a corpus that covers authored constraints and custom materials, put solved native designs
in `/notebooks/native-designs` and combine them with the library below. The directory must exist;
files are read directly inside it. If you only have ASC, omit `--design-dir` and `--include-library`:
training works, but cannot learn native constraint/material cases absent from the corpus. Inspect
the generated coverage report rather than treating format support as evidence of actual coverage.

Retarget warm-up, with a checkpoint and statistics after each completed iteration:

```sh
DIAGRAM_DB_PATH=/notebooks/facet_diagrams.sqlite ./pleochroma-trainer-cli \
  --no-update-check --device cuda --algo imitation --mode retarget --tier m \
  --design-dir /notebooks/native-designs --include-library \
  --iterations 300 --batch-size 256 --mini-batch 128 --updates-per-round 16 \
  --label-every 2 --label-capacity 50000 --max-rejected-steps 8 \
  --objectives mixed --same-material-frac 0.25 --max-tiers 128 --seed 42 \
  --train-fidelity fast --full-fidelity-frac 0 \
  --validation-designs 16 --validation-every 20 --validation-fidelity fast \
  --objective-sweep --save-every 1 \
  --save-stats retarget_m_layout7.json --save-weights retarget_m_layout7.ckpt.json
```

Optimizer warm-up uses the same data and cadence, with a separate task/checkpoint:

```sh
DIAGRAM_DB_PATH=/notebooks/facet_diagrams.sqlite ./pleochroma-trainer-cli \
  --no-update-check --device cuda --algo imitation --mode optimizer --tier m \
  --design-dir /notebooks/native-designs --include-library \
  --iterations 300 --batch-size 256 --mini-batch 128 --updates-per-round 16 \
  --label-every 2 --label-capacity 50000 --max-rejected-steps 8 \
  --objectives mixed --max-tiers 128 --seed 42 \
  --train-fidelity fast --full-fidelity-frac 0 \
  --validation-designs 16 --validation-every 20 --validation-fidelity fast \
  --objective-sweep --save-every 1 \
  --save-stats optimizer_m_layout7.json --save-weights optimizer_m_layout7.ckpt.json
```

Fast labels evaluate a single pose. They do not teach the Full tilt objective merely because a
final report uses Full. `--full-fidelity-frac X` explicitly samples Full episodes while retaining
Fast ones; fidelity is an input feature, and each episode's labels and acceptance use that same
fidelity. The default is zero because Full evaluates 724 poses per objective evaluation. Even a
small fraction can dominate physics work and delay a synchronized round. `--train-fidelity full`
always uses Full, regardless of the fraction. Fast/Full mixtures suppress the pooled
train/validation gap because the underlying score averages are not directly comparable.

For deliberate Full fine-tuning after warm-up, use a small workload first. The following example
is for five CPU cores; change `--workers 5` to your actual allocation. This still may be expensive:

```sh
DIAGRAM_DB_PATH=/notebooks/facet_diagrams.sqlite ./pleochroma-trainer-cli \
  --no-update-check --device cuda --algo imitation --mode retarget --tier m \
  --design-dir /notebooks/native-designs --include-library \
  --warm-start retarget_m_layout7.ckpt.json --workers 5 \
  --iterations 100 --batch-size 16 --mini-batch 8 --updates-per-round 4 \
  --label-every 2 --label-capacity 50000 --objectives mixed \
  --same-material-frac 0.25 --max-tiers 128 --seed 42 \
  --train-fidelity full --validation-fidelity full \
  --validation-designs 4 --validation-every 20 \
  --eval-test --eval-fidelity full --save-every 1 \
  --save-stats retarget_m_layout7_full.json --save-weights retarget_m_layout7_full.ckpt.json
```

For optimizer fine-tuning, change `--mode`, the compatible `--warm-start` checkpoint and both output
names to optimizer. This fine-tuning command deliberately starts a new run: replay, optimizer and RNG states start anew.
Full validation/test now run the entire policy episode with Full scoring, not just the endpoint;
the automatic Full teacher-reference pass is omitted. Final `--eval-test` visits the entire held-out
test split and can be lengthy. The four-design fine-tuning cohort above is only a progress check;
use a broader cohort for a quality assessment. Preserve the final test split for model selection's end.

The initial recovery checkpoint precedes the first rollout; subsequent checkpoints include
completed rollout/update boundaries. Ten-second diagnostic progress snapshots remain available
between checkpoints. Native files preserve their authored material; ASC episodes use the imported
index in one quarter of cases and real catalog materials in the rest. Mixed objectives include
continuous lighter/deeper tone weights and varied shape targets, not just seven isolated presets.

The earlier source/algebra reviews are historical context, not a claim of empirical verification:
[mathematical review](../../crates/pleochroma-ai/docs/training-math-review-2026-10-26.md) and
[representation and teacher review](../../crates/pleochroma-ai/docs/representation-review-2026-10-26.md).

## Native CUDA (Linux CLI)

CUDA is an explicit opt-in backend and does not participate in `--device auto`; Auto keeps its
existing WGPU-then-CPU behavior. `--device gpu` and `--device wgpu` select the existing WGPU
backend. Native CUDA explicitly targets CUDA device index 0 and requires the trainer's `cuda` Cargo feature, a matching NVIDIA driver, and
the CUDA toolkit/development environment used by the pinned `cudarc 0.19.10` and
`cubecl-cuda 0.11` dependencies. Ensure `nvcc` is on `PATH`; set `CUDA_PATH` when the toolkit is
installed in a nonstandard location. `cudarc` detects `nvcc --version` and has its own fallback
when `nvcc` is unavailable, so install the toolkit rather than relying on that fallback. The
CubeCL CUDA backend defaults to its bundled LLVM compiler path, which needs a C++ compiler at build
time but not a system LLVM installation. The CUDA toolkit and libdevice must remain available at
runtime: CubeCL searches `CUDA_PATH`, `/usr/local/cuda`, `/opt/cuda`, `/usr/lib/cuda`, and `/usr`
for `nvvm/libdevice/libdevice.10.bc`. Install the toolkit for both build and runtime, including
NVRTC (`libnvrtc`), which compiles the AI crate's native physics kernels. Native kernels target
the selected device's compute capability (A100: 8.0; RTX A6000: 8.6), with precise math rather
than fast-math substitutions. CUDA performs neural updates, batched rollout/validation policy
forwards and PPO critic bootstraps, optical ray tracing and lighting classification, spectral
absorption, plane-pair pruning and feasible-vertex enumeration, tool-face clipping, and batched
meet-projection residuals. Exact authored mast solving and the original final preservation gate
still run on CPU; that gate includes its own arrangement and topology checks. Ordered vertex
deduplication, mesh rings, calipers, manufacturability checks, irregular concave splitting,
small correction solves and reporting also remain on CPU. The reported
device name, CLI banner, telemetry, tier changes, and update/rollout trainers retain the actual CUDA
backend label. Checkpoints remain backend-independent. The existing GUI
switch only selects between CPU and WGPU; CUDA is treated as GPU by the switch, so toggling from
CUDA moves to CPU. A successful device switch also changes environment execution. Crossing the
CUDA boundary retains accepted episode state and saved scores, clears arithmetic-specific replay
and partial PPO samples, and reports that change; a failed switch keeps the old run intact.

From `apps/pleochroma-trainer`, build the standalone CLI with CUDA enabled on a Linux CUDA host:

```sh
cargo build --release --features cuda --bin pleochroma-trainer-cli
```

Use the layout-7 commands above with the newly built executable. For PPO, select `--algo ppo`,
set `--label-every 0` unless teacher diagnostics are needed, and configure PPO epochs/mini-batches.
PPO does not train the teacher's sensitivity loss. An explicit CUDA choice fails on a missing
feature/runtime/device rather than silently selecting WGPU or CPU. Checkpoints are backend-independent.

Training quality should be assessed from fixed validation and final test cohorts, objective and
material coverage, per-case outcomes, component support, and the chosen endpoint fidelity. This
README sets no numeric success threshold; quality depends on the corpus, task mixture, fidelity,
and measured evaluation coverage.

The AI crate's embedded NVRTC kernels use device math built-ins and exact numeric constants,
without including host `math.h` or `float.h`. An older executable can fail at startup with
`could not open source file "math.h" (no directories in search list)` even when NVRTC is installed.
Rebuild and replace the trainer executable to pick up the corrected embedded sources; the
training command is unchanged. Do not add host C-library include paths to work around it.
NVIDIA documents the available built-ins in the
[NVRTC guide](https://docs.nvidia.com/cuda/archive/12.8.1/nvrtc/index.html#builtin-functions).


### CUDA environment execution and recovery

`--device cuda` enables the native environment services automatically; no additional training
flags are needed. They use the same device ordinal and primary CUDA context as the neural
backend. The implementation lives entirely in `pleochroma-ai` and `pleochroma-trainer`; it does
not route training through the application's rendering worker.

A bounded persistent optical worker combines ready Fast candidates. Its batch and Full pose-tile
capacities depend on the selected device's SM count, thread capacity and free VRAM, with a ceiling
of 128 Fast candidates and a scratch budget of at most 512 MiB or one eighth of free VRAM.
These are capacity limits, not measured optimal batch sizes: the worker never waits indefinitely
to fill them. Neural mini-batch size and rollout size remain explicit training settings.
Planar and concave rays use separate compiled entry points, so ordinary faceted stones do not
pay for the CSG intersection stack. CUDA classifies lighting and reduces mode counts on-device;
Full scoring reads back four floats per pose. Face-up tone paths remain resident through
histogram/principal-path compaction, Gaussian absorption-band evaluation and spectral attenuation.
Only metadata and final spectra return to the host for the existing CIE conversion.

The solid service prunes plane pairs on CUDA, scans the surviving work and enumerates only
surviving-pair triples in bounded tiles. Ordered compaction retains canonical triple order and
provenance. It caches tool-face meshes and plane-only widths, including the quantized width used
by the analytical ray fan. Mesh measurement reuses those vertices instead of enumerating the
same arrangement again. Tool-face polygon clipping runs on CUDA; dynamic concave splitting and
visibility remain a counted CPU stage.

Projection uploads immutable episode networks once, then evaluates finite-difference and
line-search candidates in batches on independent streams. An outward-rounded interval prefilter
can reject impossible candidates; it never certifies a candidate. Every accepted output still
passes the original production meet-preservation gate. A small exact-state certificate cache
reuses that gate's solved tiers and facts rather than solving the accepted design again.
Candidate preparation runs on the existing bounded CPU pool, and all services reuse CUDA buffers.

`<save-stats path>.progress.json` includes an `environment` object. Its `optics` counters report
ray launches, rays, completed requests, queue/active counts and mean/max Fast batch size, alongside
the queried device details and calculated capacity limits. Its `solid` counters report GPU
arrangements, tiles, surviving triples, readback/scratch bytes, width/tool cache hits and CPU
concave fallback reasons. Its `geometry` counters distinguish CUDA residual probes and interval
refusals, CPU authoring probes, CPU projection fallbacks, unsupported-network reasons, original
gate calls/time and certificate-cache hits. The terminal progress view also summarizes the CUDA
work and CPU preservation checks. Service timings are cumulative host wall times and may overlap;
they are **not GPU utilization or CUDA event measurements**. Device/compiler/numeric
failures stop the run; only explicitly unsupported inputs may use the counted CPU reference.

A6000 and A100 have different FP64 capabilities. Batching and buffer reuse help avoid launch
and transfer overhead, but small geometry workloads and exact CPU checks can still limit
throughput. There is no source-only basis for promising 100% GPU usage or an optimal worker
count. Keep the saved worker configuration for recovery, inspect the counters, and compare
measured phase times before changing a fresh run's workload.

The existing recovery command remains:

```sh
./pleochroma-trainer-cli --device cuda --resume /notebooks/retarget_m_layout7
```

Recovery retains saved model/optimizer/RNG/episode state. Replay and cached teacher labels
are retained when the environment arithmetic matches. When it changes, those labels and
the oracle cache are cleared with an explicit warning: an action may project differently
under the new arithmetic. Training keeps its saved iteration, configuration and optimizer;
cross-revision bitwise replay is not promised. The execution revision is `cuda-env-v3`.
Validation caches and completed-case journals bind the physics
and policy backend identities. The CLI uses separate versioned journal directories for the execution route, preserving older
journals. Inside a route, incompatible or corrupt identities are refused rather than silently
mixed into current validation.
Completed compatible validation cases are restored before any policy inference is scheduled.

The new CUDA physics path requires numerical validation on both A100 and A6000 before long
training. Source review and formatting do not establish runtime parity, speed or learned quality.

From the repository root, run the following sequentially on **each** training GPU. The native
fixtures are ignored by default, so `--include-ignored` is intentional. These cover optical
counts, continuous tone tolerances, Full tilt ordering, scratch reuse, cancellation, solid meshes
and widths, canonical triples, projection bounds and the original meet-preservation gate.
The trainer suite covers evaluation, recovery and PPO bootstrap selection.

```sh
cargo test --manifest-path crates/pleochroma-ai/Cargo.toml --features cuda --lib gpu_optics:: -- --include-ignored --test-threads=1
cargo test --manifest-path crates/pleochroma-ai/Cargo.toml --features cuda --lib gpu_solid:: -- --include-ignored --test-threads=1
cargo test --manifest-path crates/pleochroma-ai/Cargo.toml --features cuda --lib gpu_geometry:: -- --include-ignored --test-threads=1
cargo test --manifest-path crates/pleochroma-ai/Cargo.toml --features cuda --lib env:: -- --test-threads=1
cargo test --manifest-path apps/pleochroma-trainer/Cargo.toml --features cuda --bin pleochroma-trainer-cli
cargo test --manifest-path apps/pleochroma-trainer/Cargo.toml --features cuda --bin pleochroma-trainer-cli device_migration_tests:: -- --include-ignored --test-threads=1
```

Also check the CPU configuration and lint both standalone crates before deployment:

```sh
cargo test --manifest-path crates/pleochroma-ai/Cargo.toml --lib
cargo clippy --manifest-path crates/pleochroma-ai/Cargo.toml --all-targets --all-features -- -D warnings
cargo clippy --manifest-path apps/pleochroma-trainer/Cargo.toml --all-targets --all-features -- -D warnings
```

Only after these pass, resume with the newly built CUDA CLI. Check that `environment.optics`,
`environment.solid` and `environment.geometry` counters advance, inspect fallback reasons, and retain the next
progress/statistics files for comparing phase times. A quiet GPU during exact geometry work is
possible; a CUDA label alone does not show how much work reached the accelerator. Do not infer
speedup from kernel-launch counts or reduce worker counts without that measurement.

### CUDA throughput changes in `cuda-env-v3`

For remote CPU/GPU measurements, targeted projection comparisons and an independent
optical capacity probe, see [Throughput diagnostics](THROUGHPUT_DIAGNOSTICS.md).
The collector packages flushed reports and hardware telemetry into one archive.

The A100 report showed 796 CPU projection fallbacks for 2,679 accelerated requests,
with about 2,224 seconds of summed fallback service time. Optical batches averaged
1.12 candidates despite capacity for 128. This points to geometry and scheduling
starvation; increasing optical buffer capacity or the neural minibatch alone cannot
remove that wait. Service times from concurrent workers overlap and are not GPU utilization.

The accelerated projector now chooses represented finite differences inside the actual
angle bounds, clips coupled corrections to both authored bounds and the original trust
region, and tries smaller ordered backtracking fractions when the first four fail.
Every accepted result still passes the original meet-preservation gate. Exact bounded
episode-local caches retain deterministic failed requests, failed gates and CPU fallback
results. Request bounds are part of the key; cancellation and infrastructure failures are
not cached. Unsupported inputs retain the original reference fallback.

Rollout probes, teacher finalization and policy steps now use per-slot dependencies.
An unrelated slow tier no longer prevents a ready slot from finishing its own work.
Optical submissions have owned tickets and bounded admission: the caller coordinates
readiness while CPU pool threads prepare other geometry. Spare threads in that same
pool can prepare both probe sides or remaining search candidates. No extra CPU pool
or thread per request is created. Sampling, candidate reduction, replay insertion and
episode seed assignment keep canonical order. There is still a pass boundary before
the next batched policy forward, so a difficult last slot can remain a bottleneck.

The progress file adds `environment.geometry.projection_miss_reasons` and
`cpu_fallback_by_reason` (actual calls, successful/refused completions, summed service
milliseconds, and cached outcomes), plus `original_gate_refusals`,
`requested_limit_refusals`, `negative_gate_cache_hits` and `projection_miss_cache_hits`.
These distinguish an accelerated correction miss from a design rejected by the exact gate.
`label_secs` measures the overlapping interval until all probes in a pass complete; it
must not be added to rollout time as independent work.

No new launch flags are required. Keep the same workload for the first comparison:
256 rollout steps, 128 minibatch, 16 updates, default automatic worker count, seed 42,
and the same corpus/material settings. A fresh comparison should use a new output prefix.
The existing run resumes with:

```sh
./pleochroma-trainer-cli --device cuda --resume /notebooks/retarget_m_cuda_v2
```

The prefix is a path, not an arithmetic version selector. Upgrading this run to v3 rebuilds
its old teacher labels but retains learned weights and Adam/RNG/episode state. Legacy snapshots
without an arithmetic marker also rebuild their derived labels once. Checkpoints
remain synchronized iteration boundaries; an interrupted partial iteration is repeated.
Compare warmed iterations separately from first-use compilation. Required evidence is
lower rollout/fallback wall time and unchanged validity behavior, not merely higher GPU
occupancy. Exact CPU authoring, preservation validation and remaining reference fallbacks
still exist. This change has not been benchmarked or executed by the implementation agent.
