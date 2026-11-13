//! Profile-guided-optimization training workload.
//!
//! Run by `scripts/pgo-build.ps1` and `scripts/pgo-bolt-build.sh` inside an instrumented
//! build. It exercises the CPU code paths that a production binary (`pleochroma-worker`,
//! `pleochroma-cut`) runs, across the optical material classes:
//!
//! - `train_tracer`: the spectral path tracer (`optics::raytracer::transport`) over
//!   isotropic, uniaxial and biaxial built-in materials, with plain and frosted finishes,
//!   inclusion scattering and edge rounding. Also covers `PlanesSoA32` intersection, the
//!   A-Trous denoiser, and both the linear HDR and the 8-bit sRGB outputs.
//! - `train_lighting`: every `LightingPreset::ALL` entry, which covers all four
//!   `LightingModel`s (the analytic studio rig and the `IsoHemisphere`, `LightTent` and
//!   `DaylightDome` models), through the same `TraceJob` path. Also one synthetic HDR
//!   equirectangular map through `EnvironmentSource::HdrMap`, the second branch of the
//!   ray-miss lookup. `train_tracer` visits only two presets, chosen for material coverage.
//! - `train_brep`: the `.asc` writer round trip (`to_asc_string`, reparsed) and B-Rep
//!   reconstruction from real cutting instructions (`reconstruct_validated_brep_from_asc`).
//! - `train_solver`: the meet-point solver, plain and verified (`geometry::meet_solver`), on
//!   three synthetic schedules and five real `.asc` designs. Then the CAD-preview
//!   measurement and mesh build (`stone_metrics::{measure_solid, build_solid_mesh}`).
//! - `train_tilt`: the tilt sweep (`color::metrics::evaluate_full_axis_profile_at_azimuth`)
//!   over every `PROFILE_AZIMUTHS_DEG` axis, as used by the worker's `TILT_CURVES` request
//!   and the cut app's tilt panel.
//! - `train_gpu`: only with `--features gpu` and an adapter present. The CPU-side chunked
//!   dispatch and readback in `renderer::gpu::hybrid`. The WGSL compute shader itself is
//!   not trained, since LLVM PGO cannot see it.
//! - `train_guide`: the guide prepass with the path signature (`n_d = 1.76`) at 800x600 and
//!   1080p (`stages_extra`).
//! - `train_denoise_signature`: the A-Trous denoiser with a path signature at 800x600
//!   (`stages_extra`). `train_tracer` also denoises every second preset with a signature.
//! - `train_tilt_sweep`: `evaluate_all_axes_profiles_geom` and `_stepped_geom`, 724
//!   evaluations each (`stages_extra`).
//!
//! `train_lighting`'s HDR map is a procedural 1024x512 panorama (sun, softboxes, sky), not
//! a toy 8x4 gradient. `--print-simd-tier` prints the dispatcher's tier and exits.
//!
//! Not trained here: the database (`pleochroma-vault` is never opened), file I/O, and the
//! WGSL compute shader. `pleochroma-net`'s wire protocol cannot be trained from this crate
//! without a dependency cycle. `scripts/pgo-bolt-build.sh` covers it instead, by running
//! `pleochroma-net`'s `scene_roundtrip` test under instrumentation as a second training
//! binary.
//!
//! Deterministic and self-contained. About 25 s at the default scale (`PGO_SCALE=1`,
//! `--profile probe`). The `[tracer]` and `[solver]` stages dominate, the solver mostly
//! from CrackOtto-Step's 205 facet-plane instances. Honours:
//!
//! - `PLEOCHROMA_SIMD=scalar|avx2|avx512` so a portable (scalar) build can be trained on
//!   an AVX machine -- see `simd::simd_level`.
//! - `PLEOCHROMA_SAMPLES=<n>`: overrides the tracer's samples-per-pixel directly (what
//!   `scripts/pgo-bolt-build.sh --samples` sets); takes priority over `PGO_SCALE` for
//!   that one number.
//! - `PGO_SCALE=<factor>` (default `1.0`): scales every stage's frame size, sample
//!   count, and iteration count by this factor, for a bounded total runtime. `0.05` is
//!   small enough to prove every stage executes in a few seconds; values above `1.0`
//!   are honoured for a deliberately heavier training run.
//! - `PGO_TRAIN_SKIP_GPU=1`: skips `train_gpu` even in a `gpu`-feature build (the
//!   stage already skips itself gracefully when no adapter is present; this is for
//!   deliberately excluding GPU dispatch code from the profile instead).
//!
//! ```text
//! cargo run --release -p pleochroma --all-features --example pgo_train
//! PGO_SCALE=0.05 cargo run --release -p pleochroma --features hdr,serde --example pgo_train
//! ```

mod stages;
mod stages_extra;

#[cfg(feature = "gpu")]
use stages::train_gpu;
use stages::{train_brep, train_lighting, train_solver, train_tilt, train_tracer};
use stages_extra::{train_denoise_signature, train_guide, train_tilt_sweep};
use std::time::Instant;

/// `PGO_SCALE` environment variable (default `1.0`, clamped to a sane positive range),
/// read once and shared by every stage below.
fn pgo_scale() -> f32 {
    std::env::var("PGO_SCALE")
        .ok()
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(1.0)
        .clamp(0.01, 20.0)
}

/// The dispatcher's chosen tier as the `PLEOCHROMA_SIMD` spelling, for
/// `--print-simd-tier`.
const fn tier_name(level: pleochroma::simd::SimdLevel) -> &'static str {
    match level {
        pleochroma::simd::SimdLevel::Scalar => "scalar",
        pleochroma::simd::SimdLevel::Avx2 => "avx2",
        pleochroma::simd::SimdLevel::Avx512 => "avx512",
    }
}

fn main() {
    // `scripts/pgo-build.ps1` runs this with `PLEOCHROMA_SIMD` unset to learn the host's
    // native tier, then trains the tier build again at that tier.
    if std::env::args().any(|a| a == "--print-simd-tier") {
        println!("{}", tier_name(pleochroma::simd::simd_level()));
        return;
    }
    let scale = pgo_scale();
    println!(
        "pgo_train: simd level {:?}, PGO_SCALE={scale}",
        pleochroma::simd::simd_level()
    );
    let start = Instant::now();

    let t0 = Instant::now();
    let tracer_checksum = train_tracer(scale);
    println!("pgo_train: [tracer] stage done in {:.2?}", t0.elapsed());

    let t0 = Instant::now();
    let lighting_checksum = train_lighting(scale);
    println!("pgo_train: [lighting] stage done in {:.2?}", t0.elapsed());

    let t0 = Instant::now();
    let brep_checksum = train_brep();
    println!("pgo_train: [brep]   stage done in {:.2?}", t0.elapsed());

    let t0 = Instant::now();
    let solver_checksum = train_solver(scale);
    println!("pgo_train: [solver] stage done in {:.2?}", t0.elapsed());

    let t0 = Instant::now();
    let tilt_checksum = train_tilt(scale);
    println!("pgo_train: [tilt]   stage done in {:.2?}", t0.elapsed());

    let t0 = Instant::now();
    let guide_checksum = train_guide(scale);
    println!("pgo_train: [guide]  stage done in {:.2?}", t0.elapsed());

    let t0 = Instant::now();
    let denoise_checksum = train_denoise_signature(scale);
    println!(
        "pgo_train: [denoise_sig] stage done in {:.2?}",
        t0.elapsed()
    );

    let t0 = Instant::now();
    let sweep_checksum = train_tilt_sweep();
    println!("pgo_train: [tilt_sweep] stage done in {:.2?}", t0.elapsed());

    #[cfg(feature = "gpu")]
    let gpu_checksum = {
        let t0 = Instant::now();
        let c = train_gpu(scale);
        println!("pgo_train: [gpu]    stage done in {:.2?}", t0.elapsed());
        c
    };
    #[cfg(not(feature = "gpu"))]
    let gpu_checksum = 0.0f32;

    println!(
        "pgo_train done in {:.1?} (checksums tracer={tracer_checksum:.3} lighting={lighting_checksum:.3} \
         brep={brep_checksum:.6} solver={solver_checksum:.6} tilt={tilt_checksum:.3} guide={guide_checksum:.3} \
         denoise_sig={denoise_checksum:.3} tilt_sweep={sweep_checksum:.3} gpu={gpu_checksum:.3})",
        start.elapsed()
    );
}
