# pleochroma — the GPU port and its equivalence harness

How `renderer::gpu` is verified against the CPU tracer, tier by tier. For the
public API and feature-flag summary see [the README](../README.md); for the
physics simplifications and golden tests see [physics.md](physics.md).

**There is a real, verified GPU port of the spectral transport physics.**
`renderer/shaders/spectral_transport.wgsl`'s `transport_main` compute kernel is
a complete megakernel covering camera-ray generation, polyhedron intersection
(both entry and exit branches), Sellmeier dispersion, Fresnel/TIR with
PDF-divided throughput, full Stokes–Mueller polarized transport, pleochroic
Beer–Lambert absorption, Russian roulette, spectral MIS with per-channel
`path_pdf` and chromatic termination, and uniaxial birefringence (θc
iteration, walk-off, per-mode indices) — this is not a stub or a partial port.
It renders real frames today, and every ported function is checked against its
CPU counterpart by the harness below, which reports the maximum genuine ULP per
check and per tier (see [The recorded run](#the-recorded-run)); a further tier
compares real rendered images. The measured values are in the recorded run, not
in this prose. This is the kernel the README's "What it physically models"
section means by "uniaxial birefringence runs on both CPU and GPU."

## What is and isn't true about its state

- **The physics is ported in full**, covering camera-ray generation through
  biaxial birefringence as described above; whether it currently matches the CPU is
  what the harness and its recorded run show.
- **It is wired into the viewer, behind a feature flag.**
  `renderer::gpu::frame::GpuFrameRenderer` is the general entry point: hand it a
  scene (camera, planes, material, environment) and it accumulates samples into
  a caller-owned buffer. `apps/pleochroma-cut` uses it when built with its own
  `gpu` feature (`cargo build -p pleochroma-cut --features gpu`), falling back to
  the CPU tracer per frame whenever the GPU declines — no adapter, a lost device,
  or an HDR environment map too large for the device's storage-buffer limit (HDR
  maps otherwise render on the GPU through their own environment mode).
  `apps/pleochroma-worker` can enable it too (its own `gpu` feature), but never for
  a viewport — that binary has none, only `render`, `serve --render` and `join` (see
  that app's README).
- **`gpu` is off by default, everywhere.** Nothing in the workspace turns it on
  for you, so an ordinary `cargo build` still pulls neither `wgpu` nor
  `pollster`, and the CPU tracer remains the reference implementation that every
  GPU result is checked against.
- **The chunked dispatch path has its own check.** A frame too large for
  `frame::CHUNK_BUDGET_BYTES` is split into pixel chunks via
  `GpuTransportParams::pixel_offset`; `frame::run_chunk_equivalence` (run by the
  harness below) requires a chunked render to be *bit-identical* to the same
  frame rendered in one dispatch. Every other GPU check dispatches a whole frame
  at once and so leaves that path unexercised.
- **Biaxial materials render on the GPU.** The `BiaxialIndicatrix` machinery is
  ported to WGSL and verified at the same Tier 2 / Tier 3 bar as every other
  material, so `GemMaterial::gpu_supported()` (`optics/materials/optics.rs`) is a
  `const fn` that returns `true` — including for the three biaxial built-ins
  (Alexandrite, Topaz, Tanzanite). It is an API seam that a caller assembling a
  scene can keep calling so a future incompatible material can opt out; today it
  does not discriminate between materials.

## Running the harness

```
cargo run --profile probe -p pleochroma --features gpu --example gpu_equivalence_harness
```

Add `-- --json <path> --date <YYYY-MM-DD>` to record the run (see
[The recorded run](#the-recorded-run)).

Needs a real GPU adapter — this is why it's an example with `required-features =
["gpu"]`, not a `cargo test` target: `cargo check`/`cargo test` without
`--features gpu` skip building it entirely. It prints `pleochroma::BUILD_ID` first for
traceability, and **exits nonzero on any check failure, or if no GPU adapter is
available at all** (a distinct exit code for "clean skip, nothing was tested" vs.
an actual divergence — read the printed message rather than just the exit code):
0 all checks passed, 1 a check failed, 2 no adapter or a bad command line, 3 the
requested summary file could not be written.

It runs through several phases, roughly in increasing order of what's being
compared:

- **Bit-exact integer checks** — GPU self-determinism (two dispatches of the same
  input must produce byte-identical output), struct-layout echo tests (a value
  round-tripped through a GPU buffer and back must match exactly — this is what
  catches WGSL's stricter `vec3`/`vec4` alignment rules silently misplacing a
  field relative to Rust's `#[repr(C)]` layout), and RNG bit-exactness.
- **Per-function ULP budgets** — individual physics functions (camera ray
  generation, CIE color-matching, Fresnel reflection/transmission, TIR
  retardation, dispersion, absorption, eigen-polarization, ...) compared CPU vs.
  GPU against a small, explicit floating-point tolerance, since CPU and GPU
  floating point are not required to agree bit-for-bit even when both are
  IEEE-754 compliant. The harness prints, per check, the **maximum genuine ULP**
  (over comparisons not exempted by the check's absolute-difference floor), the
  maximum raw ULP (including the exempted ones) and the budget, and aggregates the
  maximum genuine ULP per tier. The budget exists for honesty about what CPU/GPU
  floating point is and isn't guaranteed to do; what a given build actually
  measures is in the recorded run below, not in this prose.
- **A furnace anchor** — a uniform environment with zero (or trivial) geometry,
  checked against an *analytically computable* expected result, not merely
  CPU-vs-GPU agreement. This is what catches the case where CPU and GPU agree with
  each other but both disagree with the actual physics.
- **Statistical image comparison of real rendered frames** — the full
  `transport_main` spectral estimator, run end to end on real materials (Spinel, the
  uniaxial and biaxial built-ins, and the scattering, frosted-girdle, edge-rounding,
  lighting-model and HDR-NEE variants), each at a small pixel grid with disjoint
  CPU-traced and GPU-traced sample ranges (mirroring how a real distributed render
  actually splits work — see `pleochroma-net`'s README; every comparison prints its own
  size and sample counts), compared
  per-pixel via Welford mean/variance, a z-score threshold, and connected-component
  clustering of failing pixels — because at this level, exact or even ULP-level
  agreement isn't the right bar; a statistically consistent image is.

Related always-available modules: `renderer::gpu::determinism_check` (the
self-determinism tier) and `renderer::gpu::polyhedron_check` (a discrete
facet-index comparison for intersection results, with a documented allowance for
legitimate edge-grazing rays where two facets are within tolerance of the same
hit distance).

## The recorded run

The harness is not a `cargo test` target, so nothing in a default `cargo test` run
proves the GPU still matches the CPU. The repository therefore keeps one artefact of
the last harness run, `crates/pleochroma/docs/gpu_harness_last_run.json`, and **the
owner commits a fresh one with each physics change** (a change to anything under
`optics/raytracer/`, `renderer/shaders/`, or the color pipeline). Only that file's
recorded verdict (`passed`) and its `build_id` count as evidence for a build.

```
cargo run --profile probe -p pleochroma --features gpu --example gpu_equivalence_harness \
    -- --json crates/pleochroma/docs/gpu_harness_last_run.json --date 2026-09-30
```

`--date` is required with `--json` and is recorded verbatim (the harness never reads
the clock). The file is written even when a check failed, with `"passed": false`, and
is never written when no adapter was found. It contains:

| Key | Content |
|---|---|
| `schema`, `date`, `build_id`, `adapter` | format version, the `--date` value, `pleochroma::BUILD_ID` (Rust and WGSL source hash), the adapter's name / device type / backend |
| `passed` | the overall verdict (every check group passed) |
| `groups` | one `{name, passed}` entry per check group (Phase 0/1, Phase 2, uniaxial, biaxial, inclusion scattering, frosted girdle, edge rounding, absorption path scale, lighting models, chunked dispatch, wavefront pipeline, kernel specialisation, HDR NEE) |
| `tiers` | per ULP family (`tier2_per_function`, `furnace_per_tuple`): number of checks, `max_genuine_ulp`, `max_raw_ulp`, `within_budget` |
| `ulp_checks` | every ULP comparison: tier, label, comparisons, `max_genuine_ulp`, `max_raw_ulp`, exempted count, budget (`null` when the check reports none), passed |
| `image_comparisons` | every Tier 3 comparison: label, size, samples per pixel per side, mean z, `\|z\|>3` pixel count and its binomial expectation, max `\|z\|`, largest connected cluster, passed |

Bit-exact tiers (Tier 0 determinism, Tier 1 integer and layout checks) appear only as
pass/fail in `groups`. "Genuine" ULP excludes comparisons the check's absolute-difference
floor exempts (values that legitimately cross zero); the raw figure includes them.
Review the `tiers` block, not this prose, to know what the port measures.

## Hardware unit tests

A few `#[test]` functions need a real adapter: the seven in
`renderer/gpu/frame/gpu_hardware_tests.rs`, the merge test in
`renderer/gpu/merge_tests.rs`, and the ones in `renderer/gpu_backend/tests.rs`. On a
machine without an adapter they print `skipping <test>: no GPU adapter` and pass. Set
`PLEOCHROMA_REQUIRE_GPU=1` and a missing adapter fails them instead, which is how a CI
machine that is supposed to have a GPU must run them:

```
PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --features gpu -- --test-threads=1 gpu_hardware_tests
```

Every build creates and tears down one instance and device at a time (`renderer/gpu/lifecycle.rs`): an acquisition takes a process-wide turn, and the last clone of a
`GpuContext` drops its handles inside the same turn. In a test build of `pleochroma` (its own
`cargo test`) a watchdog thread also aborts the test process with the name of the stuck phase when
waiting for the turn and creating a device take over 120 s or one pipeline compilation over 30
minutes (`renderer/gpu/test_support.rs`). There `PLEOCHROMA_GPU_TEST_GATE=0` makes the turn a
pass-through; `PLEOCHROMA_GPU_TEST_WATCHDOG=0` switches the watchdog off, for example under a
debugger, and `=report` makes it report (already after 60 s) without aborting;
`PLEOCHROMA_GPU_TEST_PIN=1` keeps one instance alive for the process.

What they assert with an adapter is narrow (no error, some non-zero radiance, the
renderer's poisoning rules, the reduction's non-finite rule, a statistical merge test);
the CPU-versus-GPU equivalence evidence is the recorded harness run above, and the
whole-frame `cpu_accumulate` identity pins in `renderer/gpu/pin_tests.rs` are compiled
only with `--features gpu`.

**Concave stones run on the megakernel (up to 128 tools).** The WGSL twin of the tool geometry is `shaders/transport_physics/10_concave_tools.wgsl` (`ct_*`: the convex pieces of a tool, closed-form ray intervals, signed-distance normals, containment, and a sort-free carve that finds the first change of the "inside the stone" state past `1e-4`); the megakernel calls it from `intersect_ray` (`intersect_ray_tools`), from the edge-rounding rule (`shading_normal_for_hit`: no rounding on a tool hit or a plane hit within the radius of a tool) and from the NEE probes (the frosted-exterior shadow probe, the HG-scatter exit walk of up to four crossings). The tool records ride in the existing `planes` storage binding, five `vec4`s per tool after the planes, with the count in `params.num_tools` (the former `_pad_head_shadow1` slot), so `MEGAKERNEL_STORAGE_BUFFERS` is unchanged; `geometry::tool::pack_planes_and_tools` is the Rust producer and documents the layout. A stone with more than `GPU_MAX_TOOL_PRIMITIVES` (128, equal to the CPU's `MAX_TOOL_PRIMITIVES`) tools is declined to the CPU tracer with `GpuRouteDecline::TooManyTools`. Callers route with `renderer::gpu_backend::scene_routes_to_gpu_with_tools` and pass the tools through `GpuBackend::try_accumulate_geom` / `GpuBatchItem::tools`. The plane-only `scene_routes_to_gpu` still declines a stone with tools, for callers that do not pass the tools along. The mobile backend (`pleochroma-mobile-core`, `render/backend.rs`) does pass them: it routes with `cpu_only_reason` over the planes and the tools and calls `GpuBackend::try_accumulate_geom_cancellable` with `scene.tools()`. The event-wavefront kernels read the same `planes` binding and `scene_plane_count()` but are not yet bound to tool records. Verification: `geometry/tool/gpu_twin_tests.rs` (host: the scan algorithm against `intersect_stone`) and, with an adapter, `renderer/gpu/tool_check.rs` (probe kernel against the CPU per tool kind and sweep, and a CPU-vs-GPU frame).

**The event-wavefront kernels trace the same concave stones.** `EventWavefrontRenderer::accumulate_with_tools` (and `accumulate_via_class_with_tools`; `accumulate` is the empty-tools case) packs the planes and the tool records with `pack_planes_and_tools` into the existing `planes` binding (slot 3), sizes `facet_finishes` to the packed entry count, sets `GpuTransportParams::num_tools` per chunk and binds both at `SceneBuffers::planes_len` entries (`chunk::scene_binding_bytes`), never at the buffer's grown capacity. No kernel gained a binding slot (`chunk/bindings.rs::kernel_slots` is unchanged; every kernel still reaches at most 8 storage buffers, so the browser's `>= 8 per stage` gate holds and `WAVEFRONT_STORAGE_BUFFERS` stays 27); the tool logic is the shared prelude and bounce text, so it is the megakernel's. More than `GPU_MAX_TOOL_PRIMITIVES` tools is `EwError::TooManyTools`; the browser's eligibility check (`pleochroma_web_core::gpu_admission::gpu_ineligibility_reason_with_tools`) declines it first. Tests: `event_wavefront/tool_tests.rs` (limit, binding sizes and text invariants on any host; per primitive EW-vs-megakernel and a CPU-effect image on hardware).

**The medium after a seam hit follows the carve, on the CPU and the GPU.** The bounce loop restarts a ray `1e-4` past the surface it crossed, and the carve skips every boundary at or before `1e-4` as the ray's own origin. On a concave stone two surfaces can lie that close (the rim where a tool meets a facet, the crease of two tools), so a restart can step across the second one and the blind `inside_gem` toggle would name the wrong medium for the whole next stretch. The carve therefore also reports whether its hit is an entry into the material or an exit: `intersect_stone_soa_side` on the CPU (adopted by `run_bounce` as `inside_gem = !entering`), `CtCarve::entering` in `10_concave_tools.wgsl` and `HitInfo::side` out of `intersect_ray` on the GPU (`HIT_SIDE_NONE` for a stone without tools, `HIT_SIDE_ENTRY`, `HIT_SIDE_EXIT`), adopted by `bounce_extend` (`08b_extend_scatter.wgsl`) before the scatter and surface phases read `inside_gem`. `bounce_extend` is shared by the megakernel, the old wavefront and `ew_extend` (which stores the adopted flag with the ray), so every kernel that toggles the medium afterwards starts from the carve's state. A stone without tools never sets `side`, so planar results do not move. The NEE probes (`hg_walk_exits`, the shadow probes, `try_split_exit_channel`) only read `hit`, as on the CPU. The host twins are `geometry/tool/gpu_twin_tests.rs` (the scan with the flag against the CPU carve, plus the rim-wedge case) and the `a[3]` side code of the tool probe in `renderer/gpu/tool_check.rs`.

**The render-physics layouts: `GpuGemMaterial` is 3104 bytes.** The zone table still ends the old material at byte 2720 (`zones` 592..2720). After it come `GpuBiaxialCurves` (80 B, 2720..2800: `types: vec4<u32>` = alpha type, gamma type, has, 0; then `alpha_a`, `alpha_b`, `gamma_a`, `gamma_b` as `vec4<f32>`, the Sellmeier or Cauchy parameters of the principal-index curves of a biaxial stone) and `GpuInclusionBlock` (224 B, 2800..3024: `header` `vec4<u32>` = family count, structure flag, domain seed, flags at 2800; `fam_dir[4]` at 2816, direction xyz and kind in w; `fam_params[4]` at 2880, sigma, p, lobe, rho; `haze` at 2944; `structure[3]` at 2960; `edge` at 3008). The block that can grow, the inclusion block, is last, so nothing after it ever moves. The Rust side is `renderer/buffers/material.rs` (the `offset_of!` and `size_of` asserts), `buffers/inclusions.rs` (the encoder and the `with_structure`, `with_edge` setters) and `gpu/layout_check.rs` (`field_name_at_offset` and the echo of the new blocks, with `layout_echo.wgsl` extended in lockstep). The default WGSL `GpuGemMaterial` stops at `extraordinary_param_b`; `build.rs::zoned_bounce_shared` is the patch that adds `zones`, then `biaxial_curves` and `inclusions`, to the megakernel and wavefront text, so a block past byte 2720 is reachable only in those variants and is pinned by the text tests of `shader_validation_tests.rs` and `shader_validation_ew_tests.rs`. The storage buffer may be larger than the WGSL struct, so unpatched shaders keep working. The inclusion block is filled as: structure row 0 = cell size, `lambda_low`, `lambda_high`, purity; row 1 = coverage, mosaic, kappa, majorant; row 2 = the lamella normal. `haze.zw` carries the sweep range (a, b) of the faceting machine and `edge` the abraded edge: width, alpha, `halo_rq_nm` (z) and the arm offset in degrees (w). The megakernel's oriented path reads only `haze.xy`, so the machine in `haze.zw` and `edge.w` collides with nothing. **No storage binding was added** (`MEGAKERNEL_STORAGE_BUFFERS` is unchanged; the blocks ride in the existing material buffer).

**The packed facet-surface word.** The per-facet `u32` of binding 8 (`facet_finishes`) keeps `0` (polished) and `1` (frosted, a Ground facet of the default roughness) and gains: bits 0 to 1 = kind (0 ideal or polished, 1 frosted, 2 polished with roughness, 3 ground), bits 2 to 11 = the level (`Rq` in 0.1 nm for kind 2, so up to 102.3 nm; the ground `alpha` in 0.001 for kind 3, 0 meaning the default 0.45), bits 12 to 19 = the anisotropy code (`16 x ratio`, 0 = round), bits 20 to 29 = the scratch angle code (degrees x 1023/180, used with the Custom Angle bit), bit 30 = Custom Angle, bit 31 = No Sweep. The CPU decodes the same quantised word (`FacetSurface::from_packed`), so both twins see identical parameters; Scratch Strength is folded into the anisotropy when the word is built and is not stored. The WGSL sites that test frosting (`06_nee_sampling.wgsl`, `08c_surface.wgsl`, `08f_dispatch_split_tail_step.wgsl`) read the kind bits of the word (`surf_kind` and `surf_is_ground` in `13_surface_ggx.wgsl`; `FACET_FINISH_KIND_MASK` in `02_scene_bindings.wgsl` is the mask), never compare the whole word, since the kind is a field of it; `girdle_finish.rs::encode_facet_surfaces` is the encoder.

**The surface scene is uploaded per request.** `GpuSurfaceScene` (`renderer/buffers/surface_upload.rs`) carries the packed words, the machine and the edge setting. Both GPU renderers take it with `set_surface_scene`, and a batch carries it on the request: `TurnRequest.surface` (falling back to the renderer's own), `GpuBackend::try_accumulate_geom_surface` (and its cancellable twin) in `gpu_backend/surface_entry.rs`, `GpuBatchItem.surface`. It is re-encoded every turn; there is no change cache. The event-wavefront path schedules the ground-arm kernel only when the scene needs it (`needed_kernels_with_surface`). **The compensation table** of the ground arm is appended to the tail of the existing `dist_func` binding, not given a binding of its own (a `facet_finishes`-sized extra binding would push `ew_arm_frosted` to nine storage buffers); the shader falls back to 1.0 when the table is absent. The ground arm replaces the Lambertian frosted arm; the arms are named `*_core` and are wrapped by `surface_finish_event` in `08f`. The scene routes to the GPU like any other: the web viewer's verdict still declines a scene with effects (`pleochroma-web-core/src/host/gpu_slot.rs`), the worker and the desktop route a surface to the GPU.

**The new units and their Tier 2 checks.** `transport_physics/11_oriented_scattering.wgsl` (needle and plate sampling, pdf and the mixture), `12_bragg_domains.wgsl` (the domain lookup with an unrolled 27-cell search over scalar offsets, no arrays, and `bragg_track_hero`) and `13_surface_ggx.wgsl` (GGX, VNDF, TIS, the micro-normal dispatch, the ground arm with its compensation lookup, the sweep angle); `transport_bounce/08c_surface.wgsl` (kinds 2 and 3, the abraded-edge band and its wear); the biaxial principal indices (`biaxial_principal_indices` in `02_biaxial_eigenmodes.wgsl`, `IndexSetup.n_beta_ch`, used per channel in `08a` and for the hero in `09`; `08e` and `08d` take the per-channel extraordinary index for the walk-off). Tier 2 test kernels: `transport_functions/09_oriented_scattering.wgsl` (bindings 110 and 111; `gpu/transport_check/oriented.rs`, 1400 cases), `10_bragg.wgsl` (130 and 131; `bragg.rs`, 432 cases), `11_surface.wgsl` (110 to 120; `surface.rs`: lobe, TIS, word, albedo and ground against the CPU twins, with the flags and words exact) and the biaxial-curves kernel (100 and 101, `eigenmodes_biaxial/curves.rs`, 32 ULP). The ULP budgets of the oriented, Bragg and surface checks (1024 to 2048, with an absolute floor of 1e-4) are provisional. No Tier 2 case exists for zoned silk, for the Bragg reflection actually appearing in the table, or for the abraded-edge shadow drop in `06_nee_sampling.wgsl`. GPU-versus-GPU frames are held to 1e-4 relative; WGSL cannot forbid fused multiply-add contraction, so bit equality is not claimed. Each unit changes the generated shader text, so the shader hashes and the event-wavefront `KERNEL_SET_HASH` and `SOURCE_HASH` are re-pinned with it (see [The recorded run](#the-recorded-run)).

**The sorted wavefront (opt-in).** `PLEOCHROMA_SORTED=1` (or a ray pool in MiB, 16 to 4096; unset, empty or `0` is off) makes every chunk of `GpuFrameRenderer`'s chunk loop (final pictures, the worker, the batch stepper and the viewport's progressive passes; the first chunk of a class compiles its kernels) run `renderer/gpu/frame/sorted.rs` instead of the megakernel; it is asked before the staged megakernel (`renderer/gpu/frame/staged.rs`, the default). A bounce of `transport_bounce_step` is cut after its surface phase: `sw_front_main` runs extend, scatter and surface for new rays and rays whose bounce ended in the medium, one kernel per arm (`sw_arm_*`) runs the arm, the exit splits, the tail and the next bounce's front, and every kernel files each ray into the queue of its next step with an atomic counter, so each dispatch is indirect over its own rays only. A pool of ray slots (the event wavefront's records, 712 bytes a slot, 256 MiB by default) takes new tuples every round; a one-thread kernel per round (`shaders/sorted_prepare.wgsl`) writes the indirect arguments, and after the rounds a drain kernel finishes the rest one thread per path. One submission per chunk, no readback inside it. `out_xyz`, the reduction and the readback are the megakernel's, so a pixel's samples are added in the same order; a sample differs from the megakernel's by compiler rounding only (the twin rule of `event_wavefront::compare`), and two sorted runs agree bit for bit. Not covered, with one warning per reason: the adaptive passes, the optics tables, the stone shadow, the tray reflection, the legacy wavefront, a device that binds fewer than 16 storage buffers (21 in all) per stage or cannot dispatch indirectly; the path cache's passes and the browser build never use it. Units: `sorted_wavefront.generated.wgsl` (the megakernel's prelude and bounce text, `event_wavefront/00_common.wgsl`, `event_wavefront/ew_surface.wgsl`, `sorted_queues.wgsl`, `sorted_wavefront.wgsl`) and `sorted_prepare.generated.wgsl`; neither enters `KERNEL_SET_HASH`. Tests: `renderer/gpu/frame/sorted_tests.rs`, `renderer/gpu/shader_validation_sorted_tests.rs`; speed: part E of `scripts/eng_speed_experiments.sh`.

**The staged megakernel (the default).** Every chunk of `GpuFrameRenderer`'s chunk loop for a plain megakernel class (final pictures, the worker, the batch stepper and the viewport's progressive passes) runs `renderer/gpu/frame/staged.rs`: the megakernel's bounce loop cut at stage ends, by default after bounces 5 and 12. A ray still alive at a stage end stores its state in a survivor buffer and ends its thread. The buffer holds 352 bytes a ray: half the chunk's tuples for the first end and a quarter for the second, grown per chunk and never shrunk, at most 512 MiB and the device's binding limit. A one-thread kernel (`shaders/stage_prepare.wgsl`) turns the survivor count into the indirect arguments of the next stage, which runs one thread per survivor; a full region leaves its rays in their threads. Each chunk is one submission, and `out_xyz`, the reduction and the readback are the megakernel's. A sample's result does not depend on the stages, so the image differs from the megakernel's by compiler rounding only (`frame::staged::tests`). Ends at or over the bounce cap drop out: the default cap of 12 runs one end, at 5, and a cap of 5 or less runs the megakernel. `PLEOCHROMA_STAGED=0` keeps the single megakernel, `1` (or unset) the default ends, and a comma list of up to three ascending bounce counts sets other ends. Android and Apple builds keep the megakernel unless the variable is set to `1` or a list (they were never measured), and the browser build always does (it cannot set one), as do the optics tables, the stone shadow, the tray reflection, the adaptive passes, the path cache, the beam reference kernels, the legacy wavefront and a device that binds fewer than 13 storage buffers (18 in all) per stage or cannot dispatch indirectly. It is the default because part S of `scripts/eng_speed_experiments.sh` (GPU only, M samples/s after the first pass) measured it faster than the megakernel on an A100 and a Radeon 780M, with every staged 8-bit picture byte-equal to the megakernel's. The first chunk of a class builds one more megakernel-sized pipeline, which makes the first pass 70-160 ms slower with a warm pipeline cache.

**The tray reflection rides the tray variant of the megakernel.** The variant that carries the Stone Shadow's map in storage binding 190 is selected when either the shadow or the "Tray Reflection" is on (`material_class::TRAY_MIRROR` is a scene bit beside the shadow's). The binding's header words: word 0 stays the shadow flag; **words 10 to 12 carry the mirror** (word 10 = on, 1.0 or 0.0; word 11 = the head-on reflectance `R0`; word 12 = the mirror's `plane_y`), words 13 to 15 stay 0. A mirror-only scene uploads a header-only buffer (shadow off, `N = 0`) with the mirror words set; `optics/raytracer/environment/tray_shadow/pack.rs` (`pack_gpu_words_with_mirror`) packs it and the upload cache key includes the mirror. The WGSL appended to the variant (`tray_shadow.wgsl`) has `tm_on()` (`env_mode == 1`, `backdrop > 0`, word 10 non-zero) and `tm_floor`, the twin of `TrayMirror::hit` stating the same plain multiplies and adds in the same order as the CPU (no explicit `fma`; compiler contraction is covered by the twin's ULP budget). `bounce_extend`'s miss branch first calls `tm_floor_bounce`, the twin of the CPU floor bounce: the same MIS weight, the `(1 − F)` tray deposit, the flush of `radiance` and `split_radiance` into `nee_xyz` through the final family integration, the Stokes vector depolarized and scaled by `F`, and the ray restarted along the reflection. Rule M applies exactly as on the CPU, to every mismatched exit channel whose own direction, or the hero's exit direction, meets the floor, whatever the channel's intensity (a zero-intensity companion too): it gets `stokes_k = 0`, `path_pdf[k] = 0` and no probe. It is routed into the tray variant's bounce arms, at the generic arm's chromatic termination at an exit and at the uniaxial internal exit (`08e_arms_uni_internal_generic.wgsl`, patched by `tray_shadow_source.rs`), not into `bounce_resolve_splits`. A floor bounce uses one of `max_bounces`, as on the CPU. Exit collection is off while the mirror is on. The unshadowed, unmirrored shader sources stay byte-identical; every anchor of the variant must occur exactly once or the variant is `None`, and a device or pipeline that cannot run the variant declines the scene to the CPU (`GpuRouteDecline::TrayReflection`, "Tray reflection is traced on the CPU on this graphics card."; `GpuBackend::supports_tray_mirror()`). The event wavefront and the legacy wavefront decline a mirrored scene; the shared decline variant is named `TrayVariant` in `GpuFrameError`, `EwError` and `SortedDecline`; in `GpuFrameError` and `EwError` its message reads "the Stone Shadow and the Tray Reflection need the megakernel's tray variant; rendering on the CPU" (the sorted wavefront logs its own reason). Binding 190 is present in a bind group whenever the variant is. Tests: `renderer/gpu/environment_check/tray_mirror.rs` (the twin against `TrayMirror::hit`, within 64 ULP or an absolute 2e-6, whichever is wider), the variant tests in `tray_shadow_source.rs` and `shader_validation_tests.rs`, the ignored `dump_the_tray_variants_for_naga_cli` (set `PLEOCHROMA_WGSL_DUMP_DIR` to write the variants for `naga`), `estimator_check::run_image_comparison_tray_mirror` and, for Ray-Traced, `gpu/whitted/tests.rs::whitted_cpu_gpu_frames_agree_over_a_glossy_tray`.

**The estimator a frame asks for.** `GpuSceneRef::estimator` and `TurnRequest::estimator` are `Option<Estimator>`: `None` is the path a build without the field took (the renderer's own `set_estimator` choice, else the process default, which the library keeps at `Branching`); `Some(..)` wins over both, so a worker renders the scene's estimator whatever its own process default is. `renderer/gpu/frame/renderer.rs::resolve_estimator(asked, own, process, kind)` is the pure precedence rule (the legacy wavefront always resolves to `Branching`), `GpuFrameRenderer::frame_estimator_for(asked)` applies it, and `GpuTransportParams::with_estimator` still decides per dispatch whether collection applies. `prepare_turn` forces `Branching` for a frame with a Tray Reflection. A legacy-wavefront renderer whose frame would collect (the request's estimator, else the renderer's, else the process default, which the apps set to exit collection) where collection applies (`collect_applies`: ideal surface, no HDR map, no Daylight + Sun, no tools, no Tray Reflection) returns `GpuFrameError::LegacyWavefrontCollect`, which the backend treats like any other decline (the CPU tracer, which collects, traces the frame). The reference pass of the backend always asks for `Branching`. Tests: `gpu_backend/estimator_tests.rs`.

**Ray-Traced on the GPU** gets one new last uniform row for the reflection: `GpuWhittedParams.tray_mirror = (on 0/1, plane_y, R0, 0)`, so the parameter block grows by 16 bytes, from 2464 to 2480, with the row at offset 2464. `wt_floor` is the WGSL twin of `TrayMirror::hit` and `wt_floor_escape` the twin of `resolve_escape`; at the emit site and at the branch miss the kernel adds `(1 − F)` times the leaf and pushes the reflected branch with factor `F` onto its worklist. A leaf leaving a stone with tools is still traced for re-entry; only a convex stone's leaf (or an overflowing worklist's) resolves directly through the floor split. The child is pushed only while its depth is within the cap and more than the reserved last slot is free (`count < 3u`); otherwise it resolves as the environment along the reflected direction, without searching the stone, as on the CPU. The kernel hash (`WHITTED_KERNEL_HASH`) changes with the shader text.
