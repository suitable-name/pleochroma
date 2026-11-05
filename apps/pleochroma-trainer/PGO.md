# Profile-guided trainer builds

`scripts/pgo-bolt-trainer.sh` builds the headless trainer with Rust/LLVM PGO and,
optionally, BOLT. Run it from a checkout on the **Linux machine that can actually
run the chosen training backend**. It supports native GNU x86-64 and AArch64 PGO;
the optional BOLT path is limited to GNU x86-64. Run the script on the GPU server
itself for a CUDA profile. It does not cross-compile, build the trainer GUI, or use
an existing training run. After all requested stages succeed, it publishes the
optimized executable to the checkout's `bin/pleochroma-trainer-cli`.

PGO can improve branch prediction, inlining and code layout in CPU-side proposal
projection, meet certification, data preparation, teacher scheduling, validation,
and training orchestration. BOLT can improve the linked CPU executable's layout.
Neither rewrites CUDA kernels, Burn/CubeCL runtime-generated kernels, NVIDIA's
driver, or dynamically loaded libraries. This cannot fix a serial algorithm or
make a GPU busy while it waits for CPU proposals. A throughput improvement is
plausible for the current CPU-bound pipeline, but is not established until measured.
GPU memory occupancy is not the success metric; completed useful training work
per second is.

## Build on the A100/A6000 training host

The CUDA driver/runtime and NVRTC required by a normal CUDA trainer run must be
available. The script enables the standalone trainer's `cuda` feature when
`--device cuda` is selected. It refuses backend fallback. Install the selected
Rust toolchain's LLVM profiling tools first:

```bash
rustup component add llvm-tools-preview

bash scripts/pgo-bolt-trainer.sh \
  --db /notebooks/facet_diagrams.sqlite \
  --device cuda --jobs 1
```

For optional BOLT, install `llvm-bolt` and its matching `merge-fdata` companion
from the same LLVM installation, including BOLT's instrumentation runtime. They
must be on `PATH`, or set `BOLT_BIN` to their directory:

```bash
bash scripts/pgo-bolt-trainer.sh \
  --db /notebooks/facet_diagrams.sqlite \
  --device cuda --jobs 1 --bolt
```

`llvm-profdata` must match rustc's LLVM major/minor version. The script looks in
the Rust sysroot and standard LLVM locations; `PGO_LLVM_BIN` can select an explicit
matching directory. BOLT tooling is a separate installation from Rust's profiling
tools. BOLT failure stops the script and retains the PGO artifact and diagnostics;
it leaves an existing `bin/pleochroma-trainer-cli` untouched and does not silently
label an unmodified executable as BOLT optimized.

Compilation is sequential by default (`--jobs 1`). Thin LTO and one codegen unit
are applied to both instrumented and profile-use builds. They use the identical
manifest, target, target directory, binary and feature set, and the trainer's own
lockfile. Both builds select only `pleochroma-trainer-cli` and use
`--no-default-features` to exclude the optional GUI dependencies and Slint UI
generation. The CUDA feature is added for `--device cuda`; AI backend features
are otherwise unchanged. Main-application profiling stages are never run.
No application-wide Cargo settings or physics code are changed.

The default CPU target is `native` on both x86-64 and AArch64. Both the
instrumented and profile-use builds use `-C target-cpu=native`, and BOLT rewrites
that same native PGO executable. This tunes the result for the current **CPU**;
it may not run on another cloud host. An A100 or A6000 identifies the GPU, not
the host CPU's instruction set. Override with `--cpu x86-64-v3` for a shared
x86-64 baseline only if all deployment CPUs support it. For older hosts,
explicitly select `--cpu x86-64-v2`; this loses FMA and can make
the tracer's fused operations substantially slower. Compare ordinary release
and PGO builds with the same CPU target so an ISA change is not mistaken for a
PGO improvement or regression.

## Profiling coverage

The profiling code is the native `pgo_workload` module under
`apps/pleochroma-trainer/src/`. The build script invokes the **actual trainer CLI
executable** with `--pgo`; the workload then spawns that same executable for normal
training and resume operations. There is no separate synthetic training binary,
no Python dependency and no main-application workload. Using the same executable
profiles the CLI's real call paths without introducing a different Cargo binary
or feature identity. Four cases run with fixed seeds:

| Case | Teacher and update path |
| --- | --- |
| Retarget, imitation | Finite-difference labels, preservation, replay and imitation updates |
| Optimizer, imitation | Same-source-material optimization, labels and imitation updates |
| Retarget, PPO | Rollouts and PPO updates, without unused imitation probes |
| Optimizer, PPO | Same-source-material rollouts and PPO updates |

Every case uses tier M, mixed objectives, spectrum sampling, real design loading,
checkpoint serialization, final held-out validation and an actual recovery
continuation. Defaults are four worker slots, 32 rollout steps, 16-row
mini-batches, two iterations, four imitation updates per round, two PPO epochs,
a 1,024-label replay cap, one validation design and a 128-tier cap. At least two iterations are required so
the asynchronous imitation update is reached. A real corpus with a nonempty
held-out validation split is required; fixture-only training is not used.

This is finite **work**, not a wall-clock deadline. Geometry, the teacher and full
validation episodes may still be expensive, particularly in an instrumented
build. The driver reports its phase every 30 seconds and leaves each trainer's
full log and progress JSON available during the run. Interrupting the script
does not yield a valid completed profile; rerun into a fresh output directory.

The defaults are deliberately a small initial profile, not proof that all
designs, optical materials, model tiers, rejection branches or GPU batching
regimes were represented. For a production-quality profile, increase rollout
coverage to match the intended run and include your native design corpus:

```bash
bash scripts/pgo-bolt-trainer.sh \
  --db /notebooks/facet_diagrams.sqlite \
  --design-dir /notebooks/native-designs \
  --device cuda --jobs 1 --bolt \
  --workers 27 --batch-size 256 --mini-batch 128 \
  --iterations 8 --validation-designs 2 --max-tiers 128
```

Use `--full-fidelity` if production primarily uses Full scoring; it changes both
training and validation to Full and can be much slower. The default Fast profile
does not cover the 724-pose Full path, every objective-specific validation sweep,
classical baselines or the final held-out **test** split. Optional native inputs
provide relations, dimensional targets, custom materials and concave designs
only when such files are actually supplied. The same physics gates and
acceptance rules used by ordinary training remain in force.

## Genuine recovery profiling

Simply resuming a completed run would immediately return without restoring the
runner. The driver therefore copies a committed, immutable **Training** generation
while the first process runs, preferring the latest captured iteration below the
configured total. The original process still finishes normally so instrumentation
can flush. Copy/prune races are retried; failure to capture a generation is fatal.

The exact committed manifest and payloads are retained, without editing the
phase, iteration, model, optimizer, RNG, replay, arguments or hashes. After the
first process finishes, its JSON outputs and entire recovery directory are
archived inside this private profiling case. The captured generation is restored
at the same private prefix and the CLI is invoked with only `--device --resume`.
The trainer validates all original payload hashes and restores its ordinary
state. A fresh recovery tree prevents later-model validation journals from being
reused after the rewind. The resumed process must finish the remaining training
iterations and validation normally.

The driver refuses a reused output directory. Database/design inputs are read by
the ordinary read-only loaders. All stats, snapshots, recovery rewinds, model
files and logs belong to newly allocated profiling directories; the production
training checkpoint is never supplied to the workload.

Success requires zero child exit codes, the requested backend, expected step and
iteration counts, completed progress and recovery state, finite held-out
validation results, a final model checkpoint and verified action labels in the
imitation cases. A too-small or unsuitable corpus fails explicitly instead of
producing an apparently successful profile of startup/no-op code.

## Running just the profiling workload

`--pgo` is an intentionally hidden development entry point, so ordinary trainer
help stays focused on training. Use `--pgo --help` to inspect its options. It
bypasses the update check, while its child runs use the ordinary trainer CLI
paths. To exercise an already-built binary directly:

```bash
./bin/pleochroma-trainer-cli --pgo \
  --db /notebooks/facet_diagrams.sqlite \
  --output-dir /notebooks/trainer-profile-new \
  --device cuda --workers 4 \
  --batch-size 32 --mini-batch 16 --iterations 2 \
  --validation-designs 1 --max-tiers 128
```

The output directory must not already exist. An ordinary release binary can run
this workload, but LLVM `.profraw` files are produced only by a PGO-instrumented
build; the build script handles instrumentation and collection automatically.
`LLVM_PROFILE_FILE` is inherited by all self-spawned trainer processes, and BOLT
profiling likewise executes the same instrumented binary for every scenario.

The old `scripts/profile-workload.py --binary PATH ...` interface remains as a
small compatibility wrapper. It forwards arguments to `PATH --pgo` and contains
no workload logic. Python is needed only when invoking that legacy wrapper.

## Outputs and using the result

Every build gets a unique directory under `target/pgo-trainer/`, or under the
parent supplied with `--output-dir`. Nothing from a previous invocation is reused.
The script prints the exact paths:

```text
target/pgo-trainer/run-<timestamp>-<random>/
  provenance.txt
  logs/
  raw/
  merged.profdata
  profile-functions.txt
  pgo-workload/workload-summary.json
  pgo-workload/<mode>-<algorithm>/
  bolt-workload/workload-summary.json       # --bolt only
  artifacts/pleochroma-trainer-cli-pgo
  artifacts/pleochroma-trainer-cli-pgo-bolt  # --bolt only
```

`provenance.txt` records compiler/tool versions, selected device/CPU, exact Cargo
and workload arguments and the repository revision/dirty status. Per-case command
JSON, fresh/resume logs and first-pass snapshots make the coverage inspectable.
The workload summary's durations are **instrumented profiling durations**, not
speedup measurements. BOLT profiling runs the same workload again against its
instrumented copy of the PGO executable.

The final selected executable is atomically copied to `bin/pleochroma-trainer-cli`
only after the entire requested pipeline succeeds: PGO by default, or PGO+BOLT
with `--bolt`. Use `--bin-dir DIR` to select another publication directory;
`--output-dir DIR` independently selects the parent for retained diagnostics.
Previous profiling artifacts remain available. A failure before publication
leaves the existing final executable unchanged.

Normal training and recovery arguments remain unchanged. From the project root:

```bash
./bin/pleochroma-trainer-cli \
  --device cuda --resume /notebooks/retarget_m_cuda_v2
```

These optimizations do not intentionally change model/checkpoint formats or
training arithmetic. Benchmark an ordinary release against PGO and PGO+BOLT on
the same hardware, corpus, configuration and fresh output directories. Separate
the first CUDA compilation/warmup iteration from steady state, compare rollout,
teacher, update and validation timings, and check that useful accepted work and
the selected backend remain comparable. Keep the ordinary release available
until the resulting executable has been validated on the training host.

Implementation validation here is static only: no builds, workloads, benchmarks,
tests or clippy were run while adding these files.

## Toolchain references

- [Rust's PGO guide](https://doc.rust-lang.org/rustc/profile-guided-optimization.html)
  describes instrumentation, profile collection/merging, and rebuilding with
  consistent compiler settings.
- [LLVM BOLT documentation](https://github.com/llvm/llvm-project/blob/main/bolt/README.md)
  describes post-link optimization, executable requirements, and profiling.
