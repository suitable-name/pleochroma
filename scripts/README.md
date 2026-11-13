# Scripts

Every script states its usage and output in its own header. Run them from the repository root
unless the header says otherwise. Shell scripts are kept LF (`.gitattributes`). They call cargo
with full commands, never the aliases of `.cargo/config.toml`, because `.cargo` is not in git:
a clone has neither the aliases nor that file's x86-64-v3 and mold flags.

## Test and measurement rounds (Linux GPU box)

| Script | What it does |
| --- | --- |
| `run_full_round.sh` | The whole test and measurement round in one command, and the only check script on Linux: retest, bench, cache-note, speed, ui and beam-check. `--with-m1` adds the m1 rows (plain rows with exit collection against kept-cache rows, which render branching through `--path-cache`). It verifies only what changed since the last run and writes one summary. Options: `--gate-repro N`, `--hang-repro N` and `--exit-repro N` (the stall reproduction arms of the retest), `--bench-typical` (the bench runs s2_sapphire and s3_small on the default engines, for a machine whose frames split between the CPU and the GPU), `--speed-parts LIST` (the parts of `eng_speed_experiments.sh`, for example `S`), `--with-adaptive` (the adaptive sampling tests and bench rows, see below), and `--with-beam` and the on-request tests. |
| `retest.sh` | Part retest: the tests that the current fixes need, with the stall reproduction arms (`--gate-repro N`, `--hang-repro N`, `--exit-repro N`) and a stall dump. It lints the `adaptive-sampling` feature builds every run; `--with-adaptive` adds the tests of those features. |
| `bench_render_accel.sh` | Part bench: builds the CLI and runs `benchmarks/render_accel/run.sh` on the still scenes, then writes one verdict table per optimization. |
| `eng_speed_experiments.sh` | Part speed: engine measurements on s1_glints; by default parts S (the megakernel against the staged kernel) and E (the sorted wavefront against both, each image compared with the megakernel's); part H (hybrid rule) on request. Every row renders with exit collection. |
| `run_tests_on_request.sh` | The tests that only run on request: those ignored as over 60 s in the workspace run or in retest step 02d, and the statistical tray tests (GPU against CPU frames, the stone over a mirror). |
| `fresh_sources.sh` | Sourced by every entry script that runs cargo on the Linux box, not run by hand. Before the first cargo command it compares the hash of the source files' content with the one stored at the last run (`target/.pleochroma-sources-stamp`). When they differ it gives every source file the current time, so cargo rebuilds what changed even when the files keep older timestamps (cargo does not then rerun a build script). It also provides `sources_hash`, the `sources:` figure of the `Repo:` line. It does nothing off Linux or on a `/mnt/` checkout. |

## Retired beam tracing (on request only)

Beam tracing is retired. Its code compiles only with the `beam` features and its tests only with
`--cfg beam_tests`. No regular script runs its tests; `run_full_round.sh` only compiles and lints the
beam build (the beam-check part runs `run_beam_tests.sh --steps 00,00c`).

| Script | What it does |
| --- | --- |
| `run_beam_tests.sh` | The one place the beam tests run, in `target/beam-tests`. |
| `bench_beam_decline.sh` | The cost of a declined beam render. Builds its own beam CLI in `target/beam`. |

`run_full_round.sh --with-beam` adds the whole `run_beam_tests.sh`; `bench_beam_decline.sh` runs only
by hand. `bench_render_accel.sh --with-beam` adds the beam benchmark rows.

## Optional adaptive sampling

Adaptive sampling is an optional build feature (`adaptive-sampling`, off by default) of `pleochroma-cut`,
`pleochroma-cli` and `pleochroma-worker`. The regular rounds build without it and check what a standard build
does: a job file that asks for it renders without it and prints a note, and a worker refuses live passes. Retest
step 01f lints the feature builds every round. `run_full_round.sh --with-adaptive` (or `retest.sh --with-adaptive`)
adds their tests (04a, 05c, 06e) and `bench_render_accel.sh --with-adaptive` the adaptive rows, built with the
feature into `target/adaptive`.

## Builds and packaging

| Script | What it does |
| --- | --- |
| `pgo-bolt-build.sh` | PGO, optionally with BOLT, for pleochroma-worker and pleochroma-cut (Linux). |
| `pgo-bolt-trainer.sh` | Profiles the trainer CLI for PGO/BOLT. |
| `pgo-build.ps1` | PGO release builds of pleochroma-cut and pleochroma-worker on Windows. |
| `make-store-package.ps1` | Stages and packs the MSIX from the scalar GPU build. |
| `make-icons.py`, `make-store-icons.py` | Generate the Windows icons and the store and Play assets. |
| `package-macos.sh`, `prepare-macos-workflow.py`, `verify-macos-archive.py` | The macOS workflow (`.github/workflows/build-macos.yml`): preflight, packaging, archive check. |

## Windows and shader tools

| Script | What it does |
| --- | --- |
| `run_windows_pins.ps1` | The Windows pins, one GPU build of the round's changed kernels and, with the same build, the staged megakernel's and the sorted wavefront's tests on this machine's default adapter (Vulkan). |
| `wgsl-bisect.sh`, `wgsl-bisect.ps1` | Rebuild the shaders with one `PLEOCHROMA_WGSL_BISECT` revert at a time and run the GPU equivalence harness for each. |
| `merge.ps1` | Muxes an SRT subtitle file into an MP4. |
