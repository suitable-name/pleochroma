# Render-acceleration benchmark

This folder holds the benchmark that decides how far the render accelerations may go: adaptive
sampling, the path cache and beam tracing. The owner runs it; nothing here is run by the build or
by the tests.

Beam tracing is retired and is an optional build feature (`beam`). The beam and engine-matrix
rows need a CLI built with it: `cargo build -p pleochroma-cli --features beam`. The command
`bash scripts/bench_render_accel.sh --with-beam` builds one into `target/beam`. `run.sh` and
`run.ps1` stop with a message when the CLI they are given has no beam tracer.

Adaptive sampling is an optional build feature too (`adaptive-sampling`). The adaptive rows
(`-Adaptive` / `--adaptive`) need a CLI built with it: `cargo build -p pleochroma-cli --features
adaptive-sampling`. The command `bash scripts/bench_render_accel.sh --with-adaptive` builds one into
`target/adaptive`. The scripts stop with a message when the CLI has no adaptive sampling.

An accelerated render is good when a viewer cannot tell it from a render with far more samples.
So the benchmark renders each scene twice at a very high sample count, to get two independent
references, and then measures how far everything else is from them.

## What is in this folder

| File | What it is |
|---|---|
| `scenes.json` | The six scenes: id, description, job file, reference samples, adaptive cap, extra render flags |
| `jobs/*.job.json` | One render job per scene, ready for `pleochroma-cli render` / `tilt-video` |
| `run.ps1`, `run.sh` | The scripts that run the benchmark (PowerShell, and bash with `jq`) |
| `out/` | Where the scripts write (created by them, ignored by git) |

## The scenes

| Id | What it tests | Job | Reference spp | Adaptive max spp |
|---|---|---|---|---|
| `s1_glints` | A high-dispersion stone under spot lights: sharp glints and fire | `jobs/s1_glints.job.json`, rendered with `--material "Synthetic Moissanite"` | 32768 | 8192 |
| `s2_sapphire` | A dark royal-blue sapphire: deep extinction | `jobs/s2_sapphire.job.json` | 16384 | 8192 |
| `s3_small` | A stone small in the frame (under 15 percent of the pixels) | `jobs/s3_small.job.json` | 16384 | 8192 |
| `s4_tilt` | Three consecutive tilt-video frames of one design | `jobs/s4_tilt.job.json` | 16384 | 8192 |
| `s5_opal` | A scattering material (white opal): the path cache must fall back correctly | `jobs/s5_opal.job.json`, rendered with `--material Opal` | 16384 | 8192 |
| `s6_8k` | An 8K still at 32k samples, the high-end case | `jobs/s6_8k.job.json` | 32768 | 32768 |

The six jobs derive from `designs/Pentalume_Sapphire/jobs/hero.job.json` (the design's planes are
embedded in the job), each with its own token, label and output name, and these changes:

- `s1_glints`: the lighting preset is Dark Spotlight; `scenes.json` adds `--material "Synthetic
  Moissanite"` so the stone is moissanite.
- `s2_sapphire`: only the sample count.
- `s3_small`: the camera distance is three times the original. Check the first render: the stone
  must cover less than 15 percent of the picture; if it covers more, raise `distance` in the job.
- `s4_tilt`: a tilt-video job of 3 frames (0, 1 and 2 degrees on axis 0).
- `s5_opal`: `scenes.json` adds `--material Opal`.
- `s6_8k`: 7680 x 4320 pixels, 32768 samples (the most a job allows).

The sapphire stone is a clipped pentagon, not a round brilliant. The geometry is the same in all
scenes, so the scenes differ in material, lighting, size and frame, which is what the acceleration
features care about. Replace the planes of a job with those of another design if you want another
shape.

## How to run

PowerShell:

```powershell
.\run.ps1                                    # step 1, 3 and 5 for all scenes
.\run.ps1 -Scene s2_sapphire,s3_small        # only these scenes
.\run.ps1 -Cli C:\tools\pleochroma-cli.exe -OutDir D:\bench
.\run.ps1 -RenderArgs '--local','gpu'        # extra flags for every render
.\run.ps1 -Adaptive                          # also step 2, the adaptive runs (needs a CLI built with adaptive-sampling)
.\run.ps1 -Beam                              # also the beam step (needs a CLI built with the beam feature)
.\run.ps1 -EngineMatrix                      # the beam step on CPU, GPU and hybrid (implies -Beam)
.\run.ps1 -Beam -BeamArgs '--beam-residual-min-spp','64'   # extra flags for the beam runs only
.\run.ps1 -EngineMatrix -DryRun              # print the render commands, run nothing
```

bash (needs bash 4 or newer and `jq`):

```bash
./run.sh
./run.sh --scene s2_sapphire --scene s3_small
./run.sh --cli /opt/pleochroma/pleochroma-cli --out-dir /data/bench
./run.sh --render-arg --local --render-arg gpu
./run.sh --adaptive
./run.sh --adaptive --path-cache
./run.sh --beam
./run.sh --engine-matrix
./run.sh --beam --beam-arg --beam-residual-min-spp --beam-arg 64
./run.sh --engine-matrix --dry-run
```

The CLI is `pleochroma-cli` on the PATH unless you pass it. The output folder is `out/` next to the
script unless you pass one. The scripts start no build, and they delete and recreate the folder of
each scene they run (`out/<scene id>/`). For `-Adaptive` / `--adaptive` the CLI must come from a
build with the `adaptive-sampling` feature, and the scripts stop with a message before any render
when it has none. For `-Beam` / `--beam` and `-EngineMatrix` /
`--engine-matrix` the CLI must come from a build with the `beam` feature; without a beam tracer
the scripts stop with a message before any render.

Run it on a quiet machine. The reference renders at 16k samples (32k for `s1_glints`, whose glints
need the extra samples for a usable reference; 32768 is the most a render allows) are hours of work, and the 8K scene is
much longer; use `-Scene` / `--scene` to do a few at a time. Timings taken while the machine is
busy are provisional.

## What the scripts do

Every render runs the CLI's own estimator, exit collection, the estimator of the apps. The
yardsticks (REF, REF2 and the baselines) are exit-collection renders. The path-cache and beam rows
render branching through their own flags (`--path-cache`, `--persist-cache` and the `--beam*` flags
need it).

Per scene (stills; a tilt scene does the same for each frame and reports the worst frame):

1. **Two references.** `render JOB --raw-out ref.plraw --progress-raw progress/ --sample-offset 0`
   and `render JOB --raw-out ref2.plraw --sample-offset <reference spp>`. The renderer has no
   seed: its sample sequence is the seed, so starting REF2 where REF ended gives two independent
   renders of the same scene. For a tilt scene `--raw-out` is a folder that receives one
   `.plraw` per rendered frame, named like that frame's PNG (`frame_1.plraw`, ...). Frames copied
   from an earlier identical pose, or already on disk when a run resumes, get no raw file.
2. **Noise floor.** `image-compare REF REF2`: how different two correct renders are.
3. **Convergence curve.** Each file of `progress/` (a snapshot at every power of two from 16 spp;
   a local render ends its chunks exactly there while it takes snapshots) is compared with REF and
   with REF2. The first snapshot below the noise floor tells how many
   samples `min_spp` must be. A snapshot is a prefix of REF, so it is closer to REF than to REF2:
   read `vs_ref2` as the honest curve and `vs_ref` as the optimistic one.
4. **Adaptive runs (only with `-Adaptive` / `--adaptive`).** First a **baseline**: the scene at a
   fixed sample count equal to the adaptive max spp (the script writes a copy of the job with that
   sample count), without adaptive sampling. Then one untimed warm-up render (`adaptive_warmup`,
   1024 spp, absolute target 10: it pays the live megakernel's compile; its seconds stay in
   `adaptive_warmup.seconds`) and, for each target noise of both metrics (relative: 10 and 1
   percent, passed as the fractions 0.1 and 0.01; absolute: Delta E 10 and 1; two per metric), the
   same job with `--adaptive --target-noise X --noise-metric M --verbose`
   (the `--verbose` log carries the `adaptive passes finished` line with the pass timing). The
   baseline and every adaptive run are compared with **REF2** (`--ref2 REF` sets the threshold):
   they start at sample 0 like REF, so REF2 is the reference that shares no samples with them. This step needs a CLI built with the `adaptive-sampling`
   feature (it has the adaptive flags; the scripts stop with a message when it has none). Check
   that `render --help` takes `--target-noise` as
   a fraction (0.01 = 1 percent) before relying on the relative results. The adaptive runs come
   right after the baseline, before the path-cache and beam runs, which may each take up to ten
   times as long.
5. **Path-cache runs (bash only, with `--path-cache`).** The scene at the adaptive cap with
   `--path-cache`, and with `--adaptive` too one adaptive run per metric (relative 0.01, absolute
   1) with the cache, each judged against the baseline by the same rule. With `--cache-keep`
   (implies `--path-cache`) a kept-cache pair follows: `cached_keep_first` builds and keeps the
   cache (`--persist-cache --cache-dir <scene>/path_cache_kept`), `cached_keep_again` loads it;
   rows `keep_first` and `keep_again` of `path_cache`. Each run's log keeps the
   cache's `Path cache: ...` note: its size and memory when the cache was used, or why it was not
   (for example `Path cache: about 2.0 GB, needs 2.4 GB with a safety margin but only 1.0 GB is
   free, rendering without it`). A cache that is used also says how many of its samples trace a
   path again, for example `Path cache: about 1.2 GB, 31.0 % of samples traced again, in video
   memory`. A run whose cache was off is a plain render, and `results.json` marks it with `stats.used: false`. The cache
   plans its memory with the free video memory in `PLEOCHROMA_CACHE_VRAM_GB` (gigabytes); without
   it the cache can only use system RAM. Like a beam run, a path-cache run is stopped after
   10 x the baseline's seconds + 120 s and recorded as `timed_out`. Path-cache runs also run with
   `--verbose` and a `RUST_LOG` that enables the cache's debug lines (lower-case `path cache: ...`);
   the status parsed into `stats` is still only the `note: Path cache` line.

6. **Beam runs (only with `-Beam` / `--beam`).** The scene at the adaptive cap with `--beam` (beam
   tracing plus the adaptive residual, whose defaults the benchmark leaves alone: residual minimum
   64 spp, region mode end-intersection, batch of 256 tiles), after the same fixed-spp baseline as
   step 4, compared with REF2 and judged by the same rule. It needs a CLI built with the `beam`
   feature (without it `--beam` is an unknown flag, and the script stops with a message); the
   build then has `--beam` on `render` / `tilt-video`. `-BeamArgs` / `--beam-arg` adds flags to
   the beam runs only, for example `--beam-residual-min-spp 64`, `--beam-batch-tiles 64`,
   `--beam-region-mode end-intersection|per-node` or `--beam-nodes N` (a multiple of 8 from 8 to 256; see `render
   --help`). Beams replace the path cache: where beams apply the cache is not used, and the
   cache still serves a stone the beams cannot. `--keep-beams` (store the beams for re-renders)
   is not part of the benchmark: it changes a re-render, not the first picture.
   Every render's CLI output is kept as `<scene>/<run>.log`; the `note:` lines of a beam run
   (the `Beams: N leaves, X % residual` line, and the notes that say why beams were refused, or
   that a stone fell back to the path tracer) are copied into `results.json` as `stats`. The
   other beam counters (beams, fragments, drops by reason, depth) are debug lines of the
   renderer, one per batch: they reach the log only with `--verbose` (put it in `-RenderArgs` /
   `--render-arg`; the default filter is `warn,pleochroma::beam=debug`, and `RUST_LOG` changes
   it). Without `--verbose` the log holds the progress and `note:` lines only.
   `run.sh` adds `--verbose` to every beam run and stops a beam run after 10 x the baseline's
   seconds + 120 s (recorded as `timed_out`, the scene goes on). A row whose notes include `Beam
   tracing off` is recorded with `declined: true` and the beam tables have a `declined` column: its
   picture is a plain render and its seconds over the baseline's are the price of asking, so
   `scripts/bench_render_accel.sh` prints "beams declined" for it instead of a beam speed-up.
7. **Engine matrix (only with `-EngineMatrix` / `--engine-matrix`, implies `-Beam`).** The beam
   render again with `--compute local` and `--local cpu`, `--local gpu` and `--local cpu+gpu`
   (the hybrid), each next to a fixed-spp render of the same cap on the same engines, so the
   speed-up is beam seconds against baseline seconds on the same hardware. Do not put `--local` or
   `--compute` in `-RenderArgs` / `--render-arg` with it. The GPU rows need a build with the `gpu`
   feature and a usable adapter; without one the CLI says so and renders on the processor, and
   the row is then a second CPU row. A table of the rows is printed at the end. The matrix also
   needs the CLI with the `beam` feature (add `gpu` for the GPU rows).
8. **Dry run (`-DryRun` / `--dry-run`).** Prints the render commands of every scene, in the order a
   real run issues them, and renders, writes and deletes nothing; it does not need the CLI.

**Reading the beam rows.** The beam rows exist only for a CLI built with the `beam` feature;
`bash scripts/bench_render_accel.sh --with-beam` builds one into `target/beam`. A beam row is
judged exactly like the adaptive rows: its `figures` (against REF2) must satisfy the selection
rule below with the scene's `baseline` as the yardstick, so `passes: true` says the beam picture is as close to the references as the fixed-spp render at
the same cap. The speed is in `seconds`: for the default row compare it with the baseline's
`seconds`; for a matrix row (`cpu`, `gpu`, `hybrid`) compare it with that row's own
`engine_baseline` (speed-up = baseline seconds / beam seconds on the same engines). `stats`
says what the beams did: `leaves` (the pieces of the stone drawn from beams) and `residual_pct`
(the share of samples left to ordinary tracing) come from the `Beams: N leaves, X % residual`
note, and a `notes` entry such as `Beam tracing off: the material rounds its edges` means the
row did not use beams at all, so its time is a baseline time and must not be read as a beam
result. A `Beam tracing renders on this computer` note on a row that was meant to use a remote
worker means no worker took part. The `s5_opal` scene (a scattering material) is expected to
carry the `Beam tracing off: the material scatters light` note; it is the fallback check.

**The s6 8K scene.** Its adaptive cap equals its reference count (32768), so the baseline is a
third 32k render and every beam row is another one: with `-EngineMatrix` that is eight 8K renders
at 32k samples on top of the two references (and `--beam` keeps the leaf cache of an 8K picture in
memory, up to the cache bound). Run `s6_8k` on its own (`--scene s6_8k`), on a quiet
machine, and treat its rows as the end-to-end case rather than a quick check. The other scenes
are small enough to run together.

## Reading `results.json`

```
{
  "generated": "...", "cli": "...",
  "rule": { "against": "REF2; baseline = fixed spp at adaptive_max_spp",
            "changed_area_pct_at_most_baseline_plus": 0.1, "worst_tile_de_at_most_baseline_times": 1.1,
            "glint_recall_at_least": 0.99 },
  "scenes": [
    {
      "id": "s2_sapphire", "kind": "still", "reference_spp": 16384,
      "noise_floor": { ...figures of REF against REF2... },
      "convergence": [ { "file": "spp_000016.plraw", "spp": 16, "vs_ref": {...}, "vs_ref2": {...} }, ... ],
      "baseline": { ...figures of the fixed-spp render at the adaptive cap against REF2... },
      "adaptive": [ { "metric": "relative", "target": 0.01, "passes": true, "figures": {...} }, ... ],
      "path_cache": [ { "mode": "fixed", "passes": true, "timed_out": false, "figures": {..., "seconds": 240},
                        "stats": { "cache_line": "Path cache: about 6.7 GB, in video memory", "used": true } },
                      { "mode": "adaptive_relative_0.01", ... }, { "mode": "adaptive_absolute_1", ... } ],
      "beam": [ { "mode": "fixed", "engine": "default", "passes": true, "figures": {..., "seconds": 812},
                  "engine_baseline": null,
                  "stats": { "beams_line": "note: Beams: 142000 leaves, 0.8 % residual", "leaves": 142000,
                             "residual_pct": 0.8, "notes": ["note: ..."] } },
                { "mode": "gpu", "engine": "gpu", ..., "engine_baseline": { ...figures, "seconds": ... } } ]
    }
  ],
  "selection": { "relative": { "loosest_target": 0.01, "samples_saving": { "s1_glints": 0.62, ... } }, "absolute": {...} }
}
```

"figures" are the numbers of `image-compare`:

| Key | Meaning |
|---|---|
| `changed_area_pct` | Percent of pixels whose Delta E (CIEDE2000, on the tone-mapped display colors) to REF is above max(1.0, the 99th percentile of the noise floor) |
| `worst_tile_de` | The largest mean Delta E of any 16 x 16 tile |
| `glint_recall` | The share of REF's glint pixels that are also glints in the run (1 means none lost). `null` ("no glints" in the text report) when REF has no glint pixels, which happens when the 99.9th percentile of the display luminance is the image maximum (for example 6.5 % of `s1_glints` clipped to white): there is nothing to recall, so the old 1.0 said nothing |
| `mean_de`, `p99_de` | Mean and 99th percentile Delta E to REF |
| `max_spp`, `samples_used` | The run's largest per-pixel sample count, and the sum of all counts |
| `seconds` | (added by the scripts) the wall time of the render, in seconds |
| `samples_saving` | `1 - samples_used / (width x height x max_spp)`: how many samples adaptive sampling saved |

For the noise floor the same keys describe REF2 against REF. For a tilt scene every key is the
worst over the frames (the largest, the smallest for `glint_recall` and `samples_saving`, the sum
for `samples_used`; a `glint_recall` that is `null` on some frames is ignored, and `null` on all
of them stays `null`). `image-compare --help` has the exact definitions.

## The selection rule

Choose the **loosest** target noise for which, on **every** scene, the adaptive run is as good as
the scene's fixed-spp baseline at the same cap (both against REF2):

- `changed_area_pct` is at most the baseline's plus 0.1 (percentage points),
- `worst_tile_de` is at most 1.1 times the baseline's, and
- `glint_recall` is at least 0.99, or `null` (REF has no glints, so nothing can be lost; the scripts
  write `glint_recall_null_passes: true` in `rule`).

The rule is relative because fixed limits cannot be met: even REF2, a second render at the full
reference count, has about 1 percent of its pixels
"changed" against REF (the threshold is the noise floor's own 99th percentile) and a worst tile of
up to 8.7 (`s1_glints`). A run capped at half the reference count is noisier still, so the honest
question is whether adaptive sampling at a cap looks as good as spending that cap on every pixel.

Do this once for each metric (relative, absolute). `selection` in `results.json` has the result:
the loosest passing target of each metric and the samples saved per scene at that target; a
metric with `"loosest_target": null` has no target that passes everywhere. Pick the metric that
saves more, and use the convergence curve to choose `min_spp`.

Because the rule needs every scene to pass, a single failing scene (usually `s1_glints` for a glint
that stopped too early, or `s2_sapphire` for dark areas) sets the target. Look at the error maps the
script leaves next to the renders (`<scene>/<run>_error.png`, `noise_floor.png`): black is no
difference, blue about 1, green 2, yellow 5, red 10 or more.

## Notes

- All images are `PLRAW1` files: the mean CIE XYZ of every pixel before the tone map, with the
  number of samples behind it. The format is documented in `apps/pleochroma-cli/src/raw_image.rs`.
- `image-compare` tone-maps with the render's own tone map and the job's color space, so Delta E is
  what a viewer would see. The job's exposure is already in the XYZ values.
- The `out/` folder is large (an 8K `PLRAW1` image is about 530 MB) and is ignored by git.
