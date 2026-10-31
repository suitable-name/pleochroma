# Live render latency benchmark

This diagnostic separates the work that delays a new Library preview's first
rendered image. It uses the desktop renderer's actual pass scheduler, engine
dispatch, guide generation, accumulation commit, denoiser and tone mapper.
It does not change those algorithms.

The benchmark is an opt-in example behind `render-bench`. No window or library
database is opened. Run it on the affected machine, with an optimized build and
the same GPU and power settings used by the app. Run one benchmark process at a
time, and avoid competing renders while comparing results.

## Start here

Run these commands from the repository root on Windows or Linux. `probe` is the
workspace's optimized diagnostic profile. Compilation time is outside the
reported measurements. Keep the profile consistent between comparisons; use
`--release` instead of `--profile probe` when comparing release builds.

Ray Traced, two resolutions, first use and warm design switches:

```text
cargo run --profile probe -p pleochroma-cut --features gpu,render-bench --example live_render_bench -- --engines ray --backend gpu --resolutions 800x600,1920x1080 --ray-aa 8 --passes 3 --repeats 2 --json live-ray.json
```

Path Traced, the reported sample targets, with and without denoising:

```text
cargo run --profile probe -p pleochroma-cut --features gpu,render-bench --example live_render_bench -- --engines path --backend gpu --resolutions 800x600,1920x1080 --path-targets 128,1024 --denoise both --passes 3 --repeats 2 --json live-path.json
```

These runs normally stop after three progressive passes per case. They measure
first-image latency without requiring every case to converge to 1024 samples.
The default ten-second case budget is checked between passes: **one slow pass
or shader compilation can exceed it**. It is not a watchdog and does not kill a
GPU operation.

Phase starts and completions are flushed to stderr as they happen. The JSON
report is updated after each completed case. Capture stderr alongside the report
if a run stalls: the last phase start identifies the operation in progress,
even when that case has not returned. Pipeline compilation and backend logs also
appear there; avoid enabling verbose Naga/WGPU debugging during timing runs.

## Reproduce the problematic design and material

Export the two designs as ASC or PSDD files, then supply them instead of the
built-in round brilliant and emerald-cut geometry:

```text
cargo run --profile probe -p pleochroma-cut --features gpu,render-bench --example live_render_bench -- --engines path --backend gpu --design-a slow-a.psdd --design-b slow-b.asc --material Rutile --resolutions 1920x1080 --path-targets 1024 --passes 3 --json live-rutile.json
```

The files supply geometry; the explicit built-in `--material` supplies the
material for both designs. This controls material changes while measuring design
switches. Saved material overrides, lighting, inclusions and other authored
render effects are not a full application-session replay. The default material
is Diamond. Use `--max-bounces N` to match the app's bounce limit. Keep camera
settings reported by the benchmark in mind when comparing it with an interactive
session. Check `visible_hit_pixels`: an empty/off-camera stone is not a useful
rendering throughput comparison.

## Measure interruption of old work

This requests cancellation during the first pass of the initial A in each
sequence and reports how long the renderer takes to return. The subsequent B
and A cases render normally, retaining the backend but using fresh cancellation
tokens. This measures stopping old work and then starting another design.

```text
cargo run --profile probe -p pleochroma-cut --features gpu,render-bench --example live_render_bench -- --engines both --backend gpu --resolutions 1920x1080 --material Rutile --cancel-after-ms 100 --passes 1 --json live-cancellation.json
```

A pass that completes before the timer has no cancellation-latency measurement.
Cancelled work must not increase the committed sample count. A shader compiler
or an already-submitted GPU dispatch may return well after cancellation was
requested; measuring that delay is the purpose of this case.

## Check sustained progress and CPU comparison

To measure convergence rather than startup, use a single case configuration and
remove the soft case budget deliberately:

```text
cargo run --profile probe -p pleochroma-cut --features gpu,render-bench --example live_render_bench -- --engines path --backend gpu --resolutions 1920x1080 --path-targets 1024 --scenarios cold --full-convergence --case-seconds 0 --json live-convergence.json
```

This can take substantially longer. The per-pass records show committed sample
counts and costs throughout the run. Ray Traced converges to `--ray-aa` (1, 4, 8
or 16), independently of the Path Traced target.

For a CPU reference, repeat a small case with `--backend cpu`. Use
`--backend cpu-gpu` to exercise the desktop's hybrid policy. Read the **observed
route and fallback reason**, not just the requested backend: a GPU request can
be declined and served by the CPU. A one-sample hybrid pass normally uses one
backend; this alone is not a failure to exercise the policy.

## Interpret the measurements

| Slow measurement | What to investigate |
| --- | --- |
| File loading / solving | Design reconstruction before rendering; not GPU execution |
| Backend acquisition or first-use trace | Adapter/device initialization, lazy shader compilation, initial uploads |
| Ray preparation or GPU preparation | Per-design CPU work and scene uploads |
| Radiance / GPU work | Rendering, dispatch/readback overhead, GPU waits; this is wall time, not a GPU timestamp |
| Visibility diagnostic | Explicit Ray hit-information pass after timed rendering; excluded from first-image and case latency |
| Path GPU with overlapping guides | Inclusive wall time of concurrent GPU and CPU work; do not sum overlapping phases |
| Signature synchronization | Additional guide/signature work before accumulation can commit |
| Display preparation | Snapshot copying, denoising and tone mapping |
| Cancellation latency | Old work delaying a new design even after cancellation is requested |

### GPU timing breakdown (opt-in)

Set `PLEOCHROMA_GPU_TIMING=1` (any value but empty or `0`, read once at start) to split
a Path Traced GPU pass into its dispatch, wait, readback, turn-boundary and scratch
costs. Each pass record then carries a `gpu_timing` object (totals in milliseconds and
the decision percentages), and stderr carries one `gpu_timing chunk`, one
`gpu_timing turn` and one `gpu_timing pass` line per chunk, turn and pass on the
`pleochroma_cut::render_bench` target. Off by default; it adds clock reads around the
GPU calls, so compare `trace_ms` only between runs that both have it on or both off.
It covers the megakernel Path route (not Ray Traced or Whitted) and assumes one GPU
request in flight, as in this harness. The field list is the `PassSummary` struct in
`crates/pleochroma/src/renderer/gpu/timing.rs`.

`first_use` means first use in that benchmark session, **not an empty driver or
on-disk shader cache**. By default the harness does not enable the app-managed
pipeline-cache directory; driver-managed caches may still be warm. Add
`--pipeline-cache-dir benchmark-pipeline-cache` to enable persistence in an
explicit directory, then repeat the same command to compare process starts with
that cache retained. This option requires `gpu`; backend support for persistent
pipeline caches varies. No cache is deleted. Warm cases retain renderer state and
switch A → B → A. With `--scenarios warm`, the initial A is recorded as a warm-up;
do not include that primer in warm comparisons. Separate first-use and warm
records when comparing target counts, resolutions or revisions.

GPU acquisition and preparation are now lazy. In GPU-only mode their waiting time
belongs to first-use tracing; the session-creation measurement no longer includes
device acquisition. CPU+GPU mode may return useful CPU images while the background
GPU preparation continues. Compare the reported route on each pass, not just the
requested backend. Ray's `visibility_diagnostic_ms` reports its separate diagnostic
pass; it is excluded from `total_case_ms` and all rendered-pass measurements.
Each sequence drains outstanding preparation/calibration workers before the next
session starts. This separate `drain_background_preparation` stage may wait for
an uncancellable driver compilation; it is outside the rendered-case timings.

Compare the same case configuration across reports. A 1080p image contains 4.32
times as many pixels as 800×600; compare both absolute latency and per-pixel
cost. Raising the final Path Traced target should not enlarge a fresh view's
first one-sample pass. If first-pass latency changes with target alone, compare
the recorded route, cache state and stage timings before attributing the change
to the target.

## What this does not measure

The first-image measurement ends when CPU RGBA pixels are ready. It does not
measure Slint texture upload, window presentation, display-thread queueing,
concurrent metrics work, Library database access, or rapid selection events in
the actual app. Display processing here is synchronous so its cost can be
isolated; intermediate-frame skipping and the app's reduced-resolution initial
preview phase are not reproduced. These cases measure the requested final
resolution, not the latency of that smaller preview.

If this benchmark is fast but the app still takes thirty seconds to show a
design, capture the current app's existing `GPU pipeline compilation started`,
`GPU pipeline compilation finished`, `live render pass completed` and
`live display pass completed` log events. That points the next investigation at
the actual scene handoff, concurrent work or image-delivery path. It does not
prove the interactive issue is fixed.

Use `--help` for the complete option list. Benchmark implementation and results
must be validated on the user's machines; no performance claims follow merely
from adding the harness.
