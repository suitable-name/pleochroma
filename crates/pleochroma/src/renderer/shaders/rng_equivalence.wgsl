// RNG and integer bit-exactness self-test kernel, driven by `renderer::gpu::rng_check`. Not a
// physics kernel. It ports the deterministic sampling draws to WGSL, bit for bit against the Rust
// source (WGSL `u32` arithmetic wraps modulo 2^32, as Rust's `wrapping_*` does):
//   1. `optics::raytracer::hash_u32`, the integer hash everything else is built from.
//   2. The per-sample seed formula of `render_core.rs::trace_samples`.
//   3. The stratified pixel-jitter and hero-wavelength draw (`low_discrepancy_base2`,
//      `cranley_patterson_rotate` and their rotation salts).
//   4. The four salted per-bounce draws of `optics::raytracer::trace_spectral_ray`.
//   5. The hero-wavelength comb of `optics::raytracer::wrapped_hero_wavelengths`.
//
// `%` vs `rem_euclid`: WGSL's `%` truncates, so it differs from `rem_euclid` for a negative
// dividend. Here `offset` is always >= 0.0 (`lambda_hero >= 380.0`, `k * channel_width >= 0.0`),
// so plain `%` agrees with the CPU. This holds only because of that invariant.
//
// `fma`, not `a * b + c`: the CPU uses `f32::mul_add`, a single-rounding fused multiply-add, and
// WGSL's `fma()` gives the same guarantee. Some drivers lower `fma()` to a non-fused multiply-add,
// so the `fma`-built floats (`lambdas`, `jx`, `jy`, `hero_rand`) can differ from the CPU by 1 ULP,
// while integer fields stay exact. `renderer::gpu::rng_check` uses zero tolerance for integer
// fields and 1 ULP for the `fma`-built floats.

struct RngRecord {
    seed: u32,
    rot_jx_hash: u32,
    rot_jy_hash: u32,
    rot_hero_hash: u32,
    sample_reversed: u32,
    jx: f32,
    jy: f32,
    hero_rand: f32,
    lambdas: array<f32, 8>,
    fresnel_draws: array<u32, 4>,
    rr_draws: array<u32, 4>,
    biref_draws: array<u32, 4>,
    mode_coupling_draws: array<u32, 4>,
    // Integer draws of the frosted bounce's cosine-weighted hemisphere direction sample.
    frosted_dir_u_draws: array<u32, 4>,
    frosted_dir_v_draws: array<u32, 4>,
}

struct Params {
    num_samples: u32,
    // Must be <= 4, the capacity of `RngRecord`'s per-bounce arrays. The host checks this before
    // dispatch.
    num_bounces: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> out_records: array<RngRecord>;

const SPECTRUM_MIN: f32 = 380.0;
const SPECTRUM_SPAN: f32 = 400.0; // 780.0 - 380.0
const NUM_CHANNELS: u32 = 8u;

// Per-bounce stream salts of `optics::raytracer`: `FRESNEL_BRANCH_STREAM`,
// `RUSSIAN_ROULETTE_STREAM`, `BIREFRINGENT_SPLIT_STREAM`, `MODE_COUPLING_STREAM`.
const FRESNEL_BRANCH_STREAM: u32 = 0x9E3779B1u;
const RUSSIAN_ROULETTE_STREAM: u32 = 0x517CC1B7u;
const BIREFRINGENT_SPLIT_STREAM: u32 = 0x2545F491u;
const MODE_COUPLING_STREAM: u32 = 0xCC9E2D51u;
// Stream salts `FROSTED_DIR_U_STREAM` and `FROSTED_DIR_V_STREAM` of `optics::raytracer`.
const FROSTED_DIR_U_STREAM: u32 = 0x27D4EB2Fu;
const FROSTED_DIR_V_STREAM: u32 = 0x165667B1u;

// Rotation salts of the pixel jitter and hero wavelength, as in `optics::raytracer`.
const PIXEL_JITTER_X_ROTATION_STREAM: u32 = 0xA511E9B3u;
const PIXEL_JITTER_Y_ROTATION_STREAM: u32 = 0x63D81B23u;
const HERO_WAVELENGTH_ROTATION_STREAM: u32 = 0x1B873593u;

fn hash_u32(x_in: u32) -> u32 {
    var x = x_in;
    x = x * 0x85ebca6bu;
    x = x ^ (x >> 13u);
    x = x * 0xc2b2ae35u;
    x = x ^ (x >> 16u);
    return x;
}

// `low_discrepancy_base2`, `radical_inverse_base` and `cranley_patterson_rotate` of
// `optics::raytracer`. jx, jy and hero_rand use bases 2, 3 and 5: one shared base gave higher
// variance on the noisiest pixels.
fn low_discrepancy_base2(n: u32) -> f32 {
    return f32(reverseBits(n)) / 4294967296.0;
}

fn radical_inverse_base(n_in: u32, base: u32) -> f32 {
    var n = n_in;
    var val: f32 = 0.0;
    var inv_base: f32 = 1.0 / f32(base);
    loop {
        if (n == 0u) {
            break;
        }
        let digit = n % base;
        val = fma(f32(digit), inv_base, val);
        inv_base = inv_base / f32(base);
        n = n / base;
    }
    return val;
}

fn cranley_patterson_rotate(x: f32, offset: f32) -> f32 {
    let sum = x + offset;
    return sum - floor(sum);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&out_records)) {
        return;
    }
    let pixel = idx / params.num_samples;
    let sample = idx % params.num_samples;

    // Per-sample seed (`render_core.rs::trace_samples`); it seeds every draw below.
    let seed = hash_u32((pixel * 0x9e3779b9u) ^ (sample * 0x85ebca6bu));

    // Stratified pixel jitter and hero wavelength.
    let rot_jx_hash = hash_u32(pixel ^ PIXEL_JITTER_X_ROTATION_STREAM);
    let rot_jy_hash = hash_u32(pixel ^ PIXEL_JITTER_Y_ROTATION_STREAM);
    let rot_hero_hash = hash_u32(pixel ^ HERO_WAVELENGTH_ROTATION_STREAM);
    let sample_reversed = reverseBits(sample);

    var rec: RngRecord = RngRecord();
    rec.seed = seed;
    rec.rot_jx_hash = rot_jx_hash;
    rec.rot_jy_hash = rot_jy_hash;
    rec.rot_hero_hash = rot_hero_hash;
    rec.sample_reversed = sample_reversed;

    let rot_jx = low_discrepancy_base2(rot_jx_hash);
    let rot_jy = low_discrepancy_base2(rot_jy_hash);
    let rot_hero = low_discrepancy_base2(rot_hero_hash);
    let jx = cranley_patterson_rotate(low_discrepancy_base2(sample), rot_jx) - 0.5;
    let jy = cranley_patterson_rotate(radical_inverse_base(sample, 3u), rot_jy) - 0.5;
    let hero_rand = cranley_patterson_rotate(radical_inverse_base(sample, 5u), rot_hero);
    rec.jx = jx;
    rec.jy = jy;
    rec.hero_rand = hero_rand;

    let channel_width = SPECTRUM_SPAN / f32(NUM_CHANNELS);
    let lambda_hero = fma(hero_rand, SPECTRUM_SPAN, SPECTRUM_MIN);
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        let offset = fma(f32(k), channel_width, lambda_hero - SPECTRUM_MIN);
        // `offset` is always >= 0.0 here, so plain `%` agrees with the CPU's `rem_euclid`.
        let wrapped = offset % SPECTRUM_SPAN;
        rec.lambdas[k] = SPECTRUM_MIN + wrapped;
    }

    for (var b: u32 = 0u; b < params.num_bounces; b = b + 1u) {
        rec.fresnel_draws[b] = hash_u32(seed ^ hash_u32(b ^ FRESNEL_BRANCH_STREAM));
        rec.rr_draws[b] = hash_u32(seed ^ hash_u32(b ^ RUSSIAN_ROULETTE_STREAM));
        rec.biref_draws[b] = hash_u32(seed ^ hash_u32(b ^ BIREFRINGENT_SPLIT_STREAM));
        rec.mode_coupling_draws[b] = hash_u32(seed ^ hash_u32(b ^ MODE_COUPLING_STREAM));
        rec.frosted_dir_u_draws[b] = hash_u32(seed ^ hash_u32(b ^ FROSTED_DIR_U_STREAM));
        rec.frosted_dir_v_draws[b] = hash_u32(seed ^ hash_u32(b ^ FROSTED_DIR_V_STREAM));
    }

    out_records[idx] = rec;
}
