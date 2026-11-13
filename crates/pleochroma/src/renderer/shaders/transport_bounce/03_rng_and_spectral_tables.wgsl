
// `hash_u32` is defined in `transport_physics/01_prelude_polarization.wgsl`.
//
// The pixel-jitter and hero-wavelength draws and their rotation streams are bit-exact with
// `optics::raytracer`. `shaders/rng_equivalence.wgsl` tests them against the CPU. jx, jy and
// hero_rand use the prime bases 2, 3 and 5. One shared base gave higher variance on the noisiest
// pixels.

const PIXEL_JITTER_X_ROTATION_STREAM: u32 = 0xA511E9B3u;
const PIXEL_JITTER_Y_ROTATION_STREAM: u32 = 0x63D81B23u;
const HERO_WAVELENGTH_ROTATION_STREAM: u32 = 0x1B873593u;

fn low_discrepancy_base2(n: u32) -> f32 {
    return f32(reverseBits(n)) / 4294967296.0;
}

// Prime-base radical inverse, as `optics::raytracer::radical_inverse_base`. Base 2 uses the
// bit-reversal path above. `fma` matches the CPU's `mul_add`.
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

// The CIE 1931 color matching table, `cie_1931_cmf` and `integrate_channels_to_xyz_family` are in
// `transport_physics/05_nee_env_sampling.wgsl`, shared with the transport-function kernels.
//
// The uniform furnace environment (env_mode 0) is grey but still wavelength-dependent, so its
// radiance comes from `rgb_to_spectral_radiance` (`transport_physics/05_nee_env_sampling.wgsl`)
// rather than a bare `l0`.

fn powi_u(base: f32, exp: u32) -> f32 {
    var result: f32 = 1.0;
    var b: f32 = base;
    var e: u32 = exp;
    loop {
        if (e == 0u) {
            break;
        }
        if ((e & 1u) == 1u) {
            result = result * b;
        }
        b = b * b;
        e = e >> 1u;
    }
    return result;
}

fn blackbody_spectrum(lambda_nm: f32, temp_k: f32) -> f32 {
    let t_k = max(temp_k, 1000.0);
    let h_c_k: f32 = 14388000.0;
    let exp_val = exp(min(h_c_k / (lambda_nm * t_k), 80.0));
    let exp_560 = exp(min(h_c_k / (560.0 * t_k), 80.0));
    let denom = max(exp_val - 1.0, 1e-6);
    let denom_560 = max(exp_560 - 1.0, 1e-6);
    let ratio = denom_560 / denom;
    return clamp(powi_u(560.0 / lambda_nm, 5u) * ratio, 0.01, 20.0);
}

// CIE 15:2004 D65 table of the "D65 Daylight" preset, used instead of `blackbody_spectrum` when
// `studio_use_d65` is set. A `const` array: 41 `f32`s need no bind-group slot or upload.
const CIE_D65_SPD_380_780_10NM: array<f32, 41> = array<f32, 41>(
    49.9755, 54.6482, 82.7549, 91.4860, 93.4318, 86.6823, 104.865, 117.008, 117.812, 114.861,
    115.923, 108.811, 109.354, 107.802, 104.790, 107.689, 104.405, 104.046, 100.000, 96.3342,
    95.7880, 88.6856, 90.0062, 89.5991, 87.6987, 83.2886, 83.6992, 80.0268, 80.2146, 82.2778,
    78.2842, 69.7213, 71.6091, 74.3496, 61.6045, 69.8856, 75.0870, 63.5928, 46.4182, 66.8054,
    63.3828,
);

// Ported op for op from `d65_relative_spectral_power` in
// `optics::raytracer::environment::spectral`. Normalized by the 100.000 entry, which is at 560 nm,
// and clamped to the table edges outside 380-780 nm.
fn d65_relative_spectral_power(lambda_nm: f32) -> f32 {
    let start_nm: f32 = 380.0;
    let step_nm: f32 = 10.0;
    let last_index: u32 = 40u; // CIE_D65_SPD_380_780_10NM.len() - 1

    let clamped = clamp(lambda_nm, start_nm, fma(step_nm, f32(last_index), start_nm));
    let position = (clamped - start_nm) / step_nm;
    let index0 = min(u32(floor(position)), last_index - 1u);
    let index1 = index0 + 1u;
    let frac = position - f32(index0);

    let v0 = CIE_D65_SPD_380_780_10NM[index0];
    let v1 = CIE_D65_SPD_380_780_10NM[index1];
    return fma(frac, v1 - v0, v0) / 100.0;
}

