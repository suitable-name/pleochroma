// Standalone kernel -- NOT concatenated with any other shader file, like `reduce_xyz_live.wgsl`.
// `renderer::gpu::frame` pulls it in directly via `include_str!`.
//
// # Why this exists
//
// The adaptive-sampling convergence check (`renderer::adaptive::metric`) compares, per pixel, the
// two half-image means of a tile after the render's own sRGB tone-map, in CIE L*a*b* with
// CIEDE2000, and takes the tile's WORST pixel error. This kernel is that check on the GPU: one
// thread per live tile reads the run's resident summed XYZ and half-buffer B
// (`reduce_xyz_resident.wgsl` keeps them there) and writes ONE float per live tile -- the tile's
// noise in the unit of the chosen metric. The host then reads back 4 bytes per live tile instead
// of 24 bytes per live PIXEL, and runs the unchanged decision (`ConvergenceMap::converge`).
//
// # The CPU twin
//
// A line-for-line port, in f32, of
//   `renderer::tonemap::tonemap_pixel_unquantized`   (`ColorSpace::encode_unquantized_with`,
//        `ToneMap::AcesFilmic { exposure: 1.0 }`: ACES luminance curve, `gamut::project_to_gamut`
//        / `project_to_gamut_bounded` / `fit_keeping_color`, the sRGB transfer function),
//   `color::body_color::srgb_to_lab` and `delta_e_2000`  (f64 on the CPU),
//   `adaptive::metric::{pixel_error, tile_measure, tile_noise}`.
// Everything that the CPU takes from a shared constant arrives through the uniform block below
// (the sRGB XYZ -> linear matrix, the white point, the keep-color knee and white level, the
// luminance floor, the highlight rule, the display scale, the metric). The constants with no
// shared CPU name (the ACES fit, the sRGB transfer curve, the Lab matrix and white, the CIEDE2000
// weights) are literals on both sides, pinned against each other by a hardware test.
//
// Results agree with the CPU within f32 rounding of the whole chain (the test states the
// tolerance); they are NOT bit-identical (f32 against f64, `pow` and trigonometry differ).
//
// # No local arrays, every `var` initialised
//
// Nothing here uses a function-scope array; every `var` has an explicit initial value.

struct TileErrorParams {
    // Frame size and tile edge in pixels, tiles per row.
    width: u32,
    height: u32,
    tile: u32,
    tiles_x: u32,
    // Live tiles in `tile_list` (= threads that do work).
    num_tiles: u32,
    // Samples per pixel so far, and how many of them had an odd absolute index.
    n_total: u32,
    n_odd: u32,
    // 0: relative L* (error / max(mean L*, l_floor)); 1: absolute Delta E.
    metric: u32,
    // 1: `HighlightRule::KeepColor`; 0: `HighlightRule::Whiten`.
    keep_color: u32,
    // 1: `tile_offset` holds a beam part (binding 5); 0: no beam part, the plain metric.
    has_offset: u32,
    // First word of the beam part's floats in `tile_offset` (the length of its per-slot header).
    offset_base: u32,
    pad2: u32,
    // Multiplier applied to the means before the tone-map.
    scale: f32,
    // `adaptive::metric::L_FLOOR`.
    l_floor: f32,
    // `gamut::KEEP_COLOR_KNEE` and `KEEP_COLOR_WHITE`.
    knee: f32,
    white_level: f32,
    // Rows of `ColorSpace::Srgb.xyz_to_rgb_matrix()` (w unused).
    rgb_r: vec4<f32>,
    rgb_g: vec4<f32>,
    rgb_b: vec4<f32>,
    // `ColorSpace::Srgb.white_point_xy()` in x and y.
    white_xy: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: TileErrorParams;
// Tile indices (row-major), ascending: the live tiles of the pass.
@group(0) @binding(1) var<storage, read> tile_list: array<u32>;
// Three floats per frame pixel: summed XYZ and half-buffer B (`reduce_xyz_resident.wgsl`).
@group(0) @binding(2) var<storage, read> resident_sum: array<f32>;
@group(0) @binding(3) var<storage, read> resident_half_b: array<f32>;
// One noise value per entry of `tile_list`.
@group(0) @binding(4) var<storage, read_write> tile_noise_out: array<f32>;
// The beam part of a beam batch's residual (beam guide section 12.4), as raw words. Words
// `0..num_tiles` are, per slot of `tile_list`, the position (in pixels) of that tile's first pixel
// in the beam part, or `NO_OFFSET` for a tile the beam part does not cover (unmeasurable). From
// word `offset_base` on come three floats (XYZ bit patterns) per pixel, the batch's tiles in
// order, each tile row-major. Read only when `params.has_offset == 1u`; otherwise any small
// buffer is bound and the shipped metric runs unchanged.
@group(0) @binding(5) var<storage, read> tile_offset: array<u32>;

const NO_OFFSET: u32 = 0xffffffffu;
const MAX_FINITE: f32 = 3.0e38;
const EXPONENT_MASK: u32 = 0x7f800000u;
const DEG: f32 = 57.29577951308232;
const RAD: f32 = 0.017453292519943295;
const POW25_7: f32 = 6103515625.0;

fn finite(v: f32) -> bool {
    return (bitcast<u32>(v) & EXPONENT_MASK) != EXPONENT_MASK;
}

fn max3(v: vec3<f32>) -> f32 {
    return max(v.x, max(v.y, v.z));
}

// `ColorSpace::xyz_to_linear` for sRGB.
fn xyz_to_linear(xyz: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        fma(params.rgb_r.x, xyz.x, fma(params.rgb_r.y, xyz.y, params.rgb_r.z * xyz.z)),
        fma(params.rgb_g.x, xyz.x, fma(params.rgb_g.y, xyz.y, params.rgb_g.z * xyz.z)),
        fma(params.rgb_b.x, xyz.x, fma(params.rgb_b.y, xyz.y, params.rgb_b.z * xyz.z)),
    );
}

fn in_range(v: vec3<f32>, upper: f32) -> bool {
    return v.x >= 0.0 && v.y >= 0.0 && v.z >= 0.0 && v.x <= upper && v.y <= upper && v.z <= upper;
}

// `optics::raytracer::aces_tonemap`.
fn aces(y: f32) -> f32 {
    return (y * fma(2.51, y, 0.03)) / fma(y, fma(2.43, y, 0.59), 0.14);
}

// The chromaticity walked towards the white point by `t`, at constant luminance.
fn mapped_xyz(x: f32, y: f32, luminance: f32, t: f32) -> vec3<f32> {
    let wx = params.white_xy.x;
    let wy = params.white_xy.y;
    let xp = fma(wx - x, t, x);
    let yp = fma(wy - y, t, y);
    if (yp > 1e-6) {
        return vec3<f32>((xp / yp) * luminance, luminance, ((1.0 - xp - yp) / yp) * luminance);
    }
    return vec3<f32>(
        (wx / wy) * luminance,
        luminance,
        ((1.0 - wx - wy) / wy) * luminance,
    );
}

// `gamut::project_to_gamut_bounded` (and `project_to_gamut` with `upper` = MAX_FINITE).
fn project_to_gamut_bounded(xyz: vec3<f32>, upper: f32) -> vec3<f32> {
    let total = xyz.x + xyz.y + xyz.z;
    if (!finite(total) || total <= 1e-6) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    let x = xyz.x / total;
    let y = xyz.y / total;
    let luminance = max(xyz.y, 0.0);

    let linear = xyz_to_linear(xyz);
    if (in_range(linear, upper)) {
        return linear;
    }

    // The walk length solved exactly, as `gamut::project_to_gamut_bounded` does: per-channel
    // numerators at the color (t = 0) and at the white point (t = 1), one linear bound each.
    let wx = params.white_xy.x;
    let wy = params.white_xy.y;
    let n0 = xyz_to_linear(vec3<f32>(x, y, 1.0 - x - y));
    let n1 = xyz_to_linear(vec3<f32>(wx, wy, 1.0 - wx - wy));
    let capped = upper < MAX_FINITE;
    var t = 0.0;
    for (var c: u32 = 0u; c < 3u; c = c + 1u) {
        t = max(t, first_nonnegative(n0[c], n1[c]));
        if (capped) {
            let h0 = fma(-luminance, n0[c], upper * y);
            let h1 = fma(-luminance, n1[c], upper * wy);
            t = max(t, first_nonnegative(h0, h1));
        }
    }
    if (t <= 1.0) {
        return clamp(
            xyz_to_linear(mapped_xyz(x, y, luminance, t)),
            vec3<f32>(0.0, 0.0, 0.0),
            vec3<f32>(upper, upper, upper),
        );
    }
    return max(xyz_to_linear(mapped_xyz(x, y, luminance, 1.0)), vec3<f32>(0.0, 0.0, 0.0));
}

// `gamut::first_nonnegative`: the smallest t >= 0 with g0 + (g1 - g0) t >= 0. Where none exists
// it returns 2.0 (the CPU returns infinity): any value above 1 selects the white-point fallback.
fn first_nonnegative(g0: f32, g1: f32) -> f32 {
    if (g0 >= 0.0) {
        return 0.0;
    }
    if (g1 > g0) {
        return -g0 / (g1 - g0);
    }
    return 2.0;
}

// `gamut::fit_keeping_color`.
fn fit_keeping_color(xyz: vec3<f32>) -> vec3<f32> {
    let rgb = project_to_gamut_bounded(xyz, MAX_FINITE);
    let peak = max3(rgb);
    if (peak <= 1.0) {
        return rgb;
    }
    let luminance = max(xyz.y, 0.0);
    let t = clamp((luminance - params.knee) / (params.white_level - params.knee), 0.0, 1.0);
    let whiten = t * t * fma(2.0, -t, 3.0);
    let fit = 1.0 / peak;
    let scale = fma(whiten, 1.0 - fit, fit);
    let dimmed = rgb * scale;
    let level = luminance * scale;
    let top = max3(dimmed);
    if (top <= 1.0) {
        return dimmed;
    }
    if (level >= 1.0) {
        return vec3<f32>(level, level, level);
    }
    let share = (top - 1.0) / (top - level);
    return dimmed + (vec3<f32>(level, level, level) - dimmed) * share;
}

// The sRGB transfer function of `TransferFunction::Srgb::encode`, with the clamps of
// `encode_unquantized_with`.
fn encode_channel(c: f32) -> f32 {
    var linear = 0.0;
    if (finite(c)) {
        linear = clamp(c, 0.0, 1.0);
    }
    var encoded = linear * 12.92;
    if (linear > 0.0031308) {
        encoded = fma(1.055, pow(linear, 1.0 / 2.4), -0.055);
    }
    return clamp(encoded, 0.0, 1.0);
}

// `tonemap_pixel_unquantized`: a mean XYZ to the sRGB channels in 0..=1, before 8-bit rounding.
fn tone_map(mean: vec3<f32>) -> vec3<f32> {
    let xyz = mean * params.scale;
    let total = xyz.x + xyz.y + xyz.z;
    if (!finite(total) || total <= 1e-6) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    let exposed = max(xyz.y, 0.0);
    let luminance_scale = aces(exposed) / max(exposed, 1e-5);
    let toned = xyz * luminance_scale;
    var rgb = vec3<f32>(0.0, 0.0, 0.0);
    if (params.keep_color == 1u) {
        rgb = fit_keeping_color(toned);
    } else {
        rgb = project_to_gamut_bounded(toned, 1.0);
    }
    return vec3<f32>(encode_channel(rgb.x), encode_channel(rgb.y), encode_channel(rgb.z));
}

fn srgb_channel_to_linear(c: f32) -> f32 {
    let v = clamp(c, 0.0, 1.0);
    if (v <= 0.04045) {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}

fn lab_f(t: f32) -> f32 {
    if (t > 0.008856451679035631) {
        return pow(t, 1.0 / 3.0);
    }
    return (903.2962962962963 * t + 16.0) / 116.0;
}

// `color::body_color::srgb_to_lab` (D65).
fn srgb_to_lab(srgb: vec3<f32>) -> vec3<f32> {
    let r = srgb_channel_to_linear(srgb.x);
    let g = srgb_channel_to_linear(srgb.y);
    let b = srgb_channel_to_linear(srgb.z);
    let x = 0.4124564 * r + 0.3575761 * g + 0.1804375 * b;
    let y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
    let z = 0.0193339 * r + 0.1191920 * g + 0.9503041 * b;
    let xr = lab_f(x / 0.95047);
    let yr = lab_f(y);
    let zr = lab_f(z / 1.08883);
    return vec3<f32>(116.0 * yr - 16.0, 500.0 * (xr - yr), 200.0 * (yr - zr));
}

fn pow7(v: f32) -> f32 {
    let v2 = v * v;
    let v4 = v2 * v2;
    return v4 * v2 * v;
}

fn hue_degrees(a_prime: f32, b: f32) -> f32 {
    if (abs(a_prime) < 1e-12 && abs(b) < 1e-12) {
        return 0.0;
    }
    let deg = atan2(b, a_prime) * DEG;
    if (deg < 0.0) {
        return deg + 360.0;
    }
    return deg;
}

// `color::body_color::delta_e_2000` (CIEDE2000, k_L = k_C = k_H = 1).
fn delta_e_2000(lab1: vec3<f32>, lab2: vec3<f32>) -> f32 {
    let l1 = lab1.x;
    let a1 = lab1.y;
    let b1 = lab1.z;
    let l2 = lab2.x;
    let a2 = lab2.y;
    let b2 = lab2.z;

    let c1_ab = sqrt(a1 * a1 + b1 * b1);
    let c2_ab = sqrt(a2 * a2 + b2 * b2);
    let c_bar_ab = 0.5 * (c1_ab + c2_ab);
    let c_bar_ab_7 = pow7(c_bar_ab);
    let g = 0.5 * (1.0 - sqrt(c_bar_ab_7 / (c_bar_ab_7 + POW25_7)));

    let a1_prime = (1.0 + g) * a1;
    let a2_prime = (1.0 + g) * a2;
    let c1_prime = sqrt(a1_prime * a1_prime + b1 * b1);
    let c2_prime = sqrt(a2_prime * a2_prime + b2 * b2);
    let h1_prime = hue_degrees(a1_prime, b1);
    let h2_prime = hue_degrees(a2_prime, b2);

    let d_lightness = l2 - l1;
    let d_chroma = c2_prime - c1_prime;

    let h_diff = abs(h2_prime - h1_prime);
    let chroma_product = c1_prime * c2_prime;
    var delta_h_deg = 0.0;
    if (chroma_product < 1e-12) {
        delta_h_deg = 0.0;
    } else if (h_diff <= 180.0) {
        delta_h_deg = h2_prime - h1_prime;
    } else if (h2_prime - h1_prime > 180.0) {
        delta_h_deg = h2_prime - h1_prime - 360.0;
    } else {
        delta_h_deg = h2_prime - h1_prime + 360.0;
    }
    let d_hue = 2.0 * sqrt(chroma_product) * sin(delta_h_deg * RAD * 0.5);

    let l_bar_prime = 0.5 * (l1 + l2);
    let c_bar_prime = 0.5 * (c1_prime + c2_prime);

    var h_bar_prime = 0.0;
    if (chroma_product < 1e-12) {
        h_bar_prime = h1_prime + h2_prime;
    } else if (h_diff <= 180.0) {
        h_bar_prime = 0.5 * (h1_prime + h2_prime);
    } else if (h1_prime + h2_prime < 360.0) {
        h_bar_prime = 0.5 * (h1_prime + h2_prime + 360.0);
    } else {
        h_bar_prime = 0.5 * (h1_prime + h2_prime - 360.0);
    }

    let t = 1.0
        - 0.17 * cos((h_bar_prime - 30.0) * RAD)
        + 0.24 * cos(2.0 * h_bar_prime * RAD)
        + 0.32 * cos((3.0 * h_bar_prime + 6.0) * RAD)
        - 0.20 * cos((4.0 * h_bar_prime - 63.0) * RAD);

    let hue_term = (h_bar_prime - 275.0) / 25.0;
    let delta_theta = 30.0 * exp(-(hue_term * hue_term));
    let c_bar_prime_7 = pow7(c_bar_prime);
    let r_c = 2.0 * sqrt(c_bar_prime_7 / (c_bar_prime_7 + POW25_7));

    let l_bar_minus_50 = l_bar_prime - 50.0;
    let l_bar_minus_50_sq = l_bar_minus_50 * l_bar_minus_50;
    let s_l = 1.0 + (0.015 * l_bar_minus_50_sq) / sqrt(20.0 + l_bar_minus_50_sq);
    let s_c = 1.0 + 0.045 * c_bar_prime;
    let s_h = 1.0 + 0.015 * c_bar_prime * t;

    let r_t = -sin(2.0 * delta_theta * RAD) * r_c;

    let term_l = d_lightness / s_l;
    let term_c = d_chroma / s_c;
    let term_h = d_hue / s_h;
    return sqrt(max(term_l * term_l + term_c * term_c + term_h * term_h + r_t * term_c * term_h, 0.0));
}

@compute @workgroup_size(64)
fn adaptive_tile_error_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let slot = gid.x;
    if (slot >= params.num_tiles) {
        return;
    }

    // A half with no samples cannot be compared: the tile is unmeasurable and never converges
    // (the CPU reports an infinite error there).
    if (params.n_odd == 0u || params.n_odd >= params.n_total) {
        tile_noise_out[slot] = MAX_FINITE;
        return;
    }
    let count_a = f32(params.n_total - params.n_odd);
    let count_b = f32(params.n_odd);

    let tile_index = tile_list[slot];
    let x0 = (tile_index % params.tiles_x) * params.tile;
    let y0 = (tile_index / params.tiles_x) * params.tile;
    let x1 = min(x0 + params.tile, params.width);
    let y1 = min(y0 + params.tile, params.height);

    // The beam part of this tile: where its first pixel sits in `tile_offset`. A tile the beam
    // part does not cover is unmeasurable, like the host metric's.
    var offset_pixel: u32 = 0u;
    if (params.has_offset == 1u) {
        offset_pixel = tile_offset[slot];
        if (offset_pixel == NO_OFFSET) {
            tile_noise_out[slot] = MAX_FINITE;
            return;
        }
    }

    var max_error = 0.0;
    var lightness_sum = 0.0;
    var pixels = 0.0;
    for (var y: u32 = y0; y < y1; y = y + 1u) {
        for (var x: u32 = x0; x < x1; x = x + 1u) {
            let base = (y * params.width + x) * 3u;
            let total = vec3<f32>(
                resident_sum[base + 0u],
                resident_sum[base + 1u],
                resident_sum[base + 2u],
            );
            let half_b = vec3<f32>(
                resident_half_b[base + 0u],
                resident_half_b[base + 1u],
                resident_half_b[base + 2u],
            );
            var mean_a = (total - half_b) / count_a;
            var mean_b = half_b / count_b;
            if (params.has_offset == 1u) {
                // `offset + mean_a` against `offset + mean_b`, as `pixel_error_with_offset`.
                let local = (y - y0) * (x1 - x0) + (x - x0);
                let at = params.offset_base + (offset_pixel + local) * 3u;
                let beam = vec3<f32>(
                    bitcast<f32>(tile_offset[at + 0u]),
                    bitcast<f32>(tile_offset[at + 1u]),
                    bitcast<f32>(tile_offset[at + 2u]),
                );
                mean_a = mean_a + beam;
                mean_b = mean_b + beam;
            }
            let lab_a = srgb_to_lab(tone_map(mean_a));
            let lab_b = srgb_to_lab(tone_map(mean_b));
            let pixel_error = delta_e_2000(lab_a, lab_b) * 0.5;
            if (pixel_error > max_error) {
                max_error = pixel_error;
            }
            lightness_sum = lightness_sum + 0.5 * (lab_a.x + lab_b.x);
            pixels = pixels + 1.0;
        }
    }

    if (pixels < 0.5) {
        tile_noise_out[slot] = MAX_FINITE;
        return;
    }
    var noise = max_error;
    if (params.metric == 0u) {
        noise = max_error / max(lightness_sum / pixels, params.l_floor);
    }
    tile_noise_out[slot] = noise;
}
