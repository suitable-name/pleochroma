// ---------------------------------------------------------------------------------
// Inclusion and subsurface scattering: optics::raytracer::{henyey_greenstein_phase,
// sample_henyey_greenstein_direction, maybe_scatter_or_extinguish}. Ported here so the
// megakernel and the standalone kernels call the same functions (see
// `03_dispersion_absorption_frosted.wgsl`).
//
// `maybe_scatter_or_extinguish` takes the per-channel absorption coefficients (`alphas`)
// as an explicit `array<f32, 8>`, like `dispersion_evaluate`. The caller has already
// computed them (`spectral_absorption` and `pleochroic_channel_alpha`). The CPU function's
// doc comment has the estimator derivation. This is a line-for-line translation of its body.
// ---------------------------------------------------------------------------------

// optics::raytracer::{DISTANCE_SAMPLE_STREAM, PHASE_DIR_U_STREAM, PHASE_DIR_V_STREAM}.
const DISTANCE_SAMPLE_STREAM: u32 = 0xa24baed4u;
const PHASE_DIR_U_STREAM: u32 = 0x9fb21c65u;
const PHASE_DIR_V_STREAM: u32 = 0x1ce4e5b9u;

// optics::raytracer::scattering::hg_nee::SHADOW_PROBE_OFFSET -- how far along the sampled
// direction the Henyey-Greenstein NEE shadow probe starts from the scattering point. The
// probe's origin lies that far INSIDE the material, so every in-material length it reports
// is that much short of the path a phase-sampled continuation from the scattering point
// itself travels; the NEE twins add it back (`nee_contribution_hg_scatter` and
// `hg_walk_exits` in `transport_bounce/06_nee_sampling.wgsl`, `tf_nee_contribution_hg_scatter`
// in `transport_functions/08_nee_and_intersect.wgsl`).
const HG_SHADOW_PROBE_OFFSET: f32 = 1e-4;

// optics::raytracer::henyey_greenstein_phase
fn henyey_greenstein_phase(cos_theta: f32, g: f32) -> f32 {
    let g2 = g * g;
    let base = max(fma(2.0 * g, -cos_theta, 1.0 + g2), 1e-6);
    let denom = base * sqrt(base);
    return (1.0 - g2) / (4.0 * PI * denom);
}

// optics::raytracer::sample_henyey_greenstein_direction
fn sample_henyey_greenstein_direction(u1: f32, u2: f32, g: f32, forward: vec3<f32>) -> vec3<f32> {
    var cos_theta: f32;
    if (abs(g) < 1e-3) {
        cos_theta = fma(-2.0, u1, 1.0);
    } else {
        let one_minus_g2 = fma(-g, g, 1.0);
        let denom = fma(2.0 * g, u1, 1.0 - g);
        let sq = one_minus_g2 / denom;
        cos_theta = fma(-sq, sq, fma(g, g, 1.0)) / (2.0 * g);
    }
    cos_theta = clamp(cos_theta, -1.0, 1.0);
    let sin_theta = sqrt(max(fma(cos_theta, -cos_theta, 1.0), 0.0));
    let phi = 2.0 * PI * u2;
    let sin_p = sin(phi);
    let cos_p = cos(phi);
    let basis = frosted_orthonormal_basis(forward);
    let dir = basis.t * (sin_theta * cos_p) + basis.b * (sin_theta * sin_p) + forward * cos_theta;
    return normalize_or_zero(dir);
}

// `simd::exp_poly::exp_lane`, the scalar reference that `exp_f32x8`'s AVX2 path is tested
// bit-identical against (`src/simd/exp_poly.rs`). Ported op for op: same Cody-Waite range
// reduction, same `fma` placement, same polynomial order. GPU and CPU then agree up to the
// difference between a hardware transcendental unit and `libm`.
// Used where the CPU twin calls `exp_f32x8`: Beer-Lambert transmittance
// (`maybe_scatter_or_extinguish` below, the NEE medium transmittance, `apply_absorption`
// in `transport_bounce/`).
// NOT used where the CPU calls `f32::exp()`, such as `spectral_absorption`'s Gaussian band
// shape or the blackbody spectrum. Those stay on the `exp()` builtin, and their small ULP gap
// against `f32::exp()` is a property of this adapter.
const EXP_LOG2E: f32 = 1.4426950408889634;
const EXP_C1: f32 = 0.693359375;
const EXP_C2: f32 = -2.1219444e-4;
const EXP_P0: f32 = 1.9875691e-4;
const EXP_P1: f32 = 1.3981999e-3;
const EXP_P2: f32 = 8.333452e-3;
const EXP_P3: f32 = 4.1665796e-2;
const EXP_P4: f32 = 1.6666665e-1;
const EXP_P5: f32 = 5.0000003e-1;
const EXP_HI: f32 = 88.02875;
const EXP_LO: f32 = -87.33654;

fn exp_poly(x_in: f32) -> f32 {
    let x0 = clamp(x_in, EXP_LO, EXP_HI);
    let n = floor(fma(x0, EXP_LOG2E, 0.5));
    let x1 = fma(n, -EXP_C1, x0);
    let x2 = fma(n, -EXP_C2, x1);
    let z = x2 * x2;
    var p = EXP_P0;
    p = fma(p, x2, EXP_P1);
    p = fma(p, x2, EXP_P2);
    p = fma(p, x2, EXP_P3);
    p = fma(p, x2, EXP_P4);
    p = fma(p, x2, EXP_P5);
    let poly = fma(p, z, x2) + 1.0;
    // 2^n by exponent-bit construction -- mirrors `exp_lane`'s
    // `f32::from_bits((((n as i32) + 127) << 23) as u32)` exactly; `n` is integral and
    // within the f32 exponent range after the clamp above.
    let pow2n = bitcast<f32>(u32(i32(n) + 127) << 23u);
    return poly * pow2n;
}

// The `Option<(f32, Vec3)>` `maybe_scatter_or_extinguish` returns, encoded for WGSL:
// `scattered == 0u` is the CPU's `None` (`t_free`/`new_dir` unset, ignored by the
// caller); `!= 0u` is `Some((t_free, new_dir))`.
struct ScatterOrExtinguishResult {
    scattered: u32,
    t_free: f32,
    new_dir: vec3<f32>,
}

// optics::raytracer::maybe_scatter_or_extinguish. `stokes`/`path_pdf` are
// `ptr<function, ...>`, mirroring `apply_frosted_bounce`'s identical in-place-mutation
// convention above.
fn maybe_scatter_or_extinguish(
    alphas: array<f32, 8>,
    sigma_s_model: f32,
    g: f32,
    ray_dir: vec3<f32>,
    hit_t: f32,
    path_scale: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
) -> ScatterOrExtinguishResult {
    // `sigma_s_model` is per MODEL unit (size-independent); `alphas` are per absorption-length
    // unit. Same conversion as the CPU twin: `sigma_s * (hit_t * path_scale) == sigma_s_model *
    // hit_t`. `path_scale == 1.0` is an exact no-op division.
    let sigma_s = sigma_s_model / path_scale;
    let sigma_t_hero = alphas[0] + sigma_s;

    let dist_rand = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ DISTANCE_SAMPLE_STREAM))) / 4294967295.0;
    let one_minus_u = max(1.0 - dist_rand, 1e-7);
    let t_free = -(log(one_minus_u)) / sigma_t_hero;

    // Model units -> absorption-length units (`GemMaterial::absorption_path_scale`).
    // `path_scale == 1.0`, every built-in, is an exact no-op.
    let hit_t_scaled = hit_t * path_scale;

    var result: ScatterOrExtinguishResult;

    if (t_free < hit_t_scaled) {
        let pdf_hero = sigma_t_hero * one_minus_u;
        for (var k: u32 = 0u; k < 8u; k = k + 1u) {
            let sigma_t_k = alphas[k] + sigma_s;
            let tr_k = exp_poly(-sigma_t_k * t_free);
            let weight = tr_k * sigma_s / pdf_hero;
            let intensity = max((*stokes)[k].x, 0.0) * weight;
            (*stokes)[k] = vec4<f32>(intensity, 0.0, 0.0, 0.0);
            (*path_pdf)[k] = (*path_pdf)[k] * (sigma_t_k * tr_k);
        }
        let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ PHASE_DIR_U_STREAM))) / 4294967295.0;
        let u2 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ PHASE_DIR_V_STREAM))) / 4294967295.0;
        result.scattered = 1u;
        // Convert the sampled free-path distance back to MODEL units -- see the CPU
        // function's own doc comment for why this unit conversion preserves the
        // estimator's unbiasedness. `path_scale == 1.0` is an exact no-op division.
        result.t_free = t_free / path_scale;
        result.new_dir = sample_henyey_greenstein_direction(u1, u2, g, ray_dir);
        return result;
    }

    let survive_hero = exp_poly(-sigma_t_hero * hit_t_scaled);
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        let sigma_t_k = alphas[k] + sigma_s;
        let survive_k = exp_poly(-sigma_t_k * hit_t_scaled);
        (*stokes)[k] = (*stokes)[k] * (survive_k / max(survive_hero, 1e-30));
        (*path_pdf)[k] = (*path_pdf)[k] * survive_k;
    }
    result.scattered = 0u;
    result.t_free = 0.0;
    result.new_dir = vec3<f32>(0.0, 0.0, 0.0);
    return result;
}

