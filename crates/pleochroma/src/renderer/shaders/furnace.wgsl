// Furnace-anchor kernel, driven by `renderer::gpu::furnace_check`.
//
// Runs camera ray generation, the hero-wavelength comb and CIE 1931 CMF integration against a
// uniform constant-radiance environment (independent of direction and wavelength) with no
// facets. The XYZ is then analytic from the CMF integral alone, so the pipeline is checked
// against true values and not only against the CPU, where a shared porting mistake would hide.
//
// The kernel never calls `intersect_polyhedron`: with zero planes the CPU returns a sentinel hit
// at `t = 1e30` (the "inside" exit branch), not a miss. Every ray therefore reaches the
// environment, so the environment is sampled unconditionally. The CPU behavior is pinned by
// `empty_planes_intersect_returns_the_sentinel_hit_not_a_miss` in `renderer::gpu::furnace_check`.

struct GpuCameraParams {
    origin: vec3<f32>,
    fov_tan: f32,
    forward: vec3<f32>,
    width: f32,
    right: vec3<f32>,
    height: f32,
    up: vec3<f32>,
    num_samples: u32,
}

struct FurnaceExtra {
    l0: f32,
    num_pixels: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> camera: GpuCameraParams;
@group(0) @binding(1) var<uniform> extra: FurnaceExtra;
@group(0) @binding(2) var<storage, read_write> out_persample: array<f32>;
@group(0) @binding(3) var<storage, read_write> out_pixel_sum: array<f32>;

const SPECTRUM_MIN: f32 = 380.0;
const SPECTRUM_SPAN: f32 = 400.0;
const NUM_CHANNELS: u32 = 8u;
// `norm_factor` of `optics::raytracer::integrate_channels_to_xyz`. Written as the same division
// rather than a decimal literal, so the f32 value is exactly the one the CPU computes.
const NORM_FACTOR: f32 = (400.0 / 8.0) / 106.856;

fn hash_u32(x_in: u32) -> u32 {
    var x = x_in;
    x = x * 0x85ebca6bu;
    x = x ^ (x >> 13u);
    x = x * 0xc2b2ae35u;
    x = x ^ (x >> 16u);
    return x;
}

// CIE 1931 2-degree observer, tabulated at 5 nm (CIE 15:2004 Table T.4, 380-780 nm), linearly
// interpolated. The real tabulated observer is used, not a Gaussian fit (1-3 % XYZ error).
// The same table is copied by hand into `shaders/environment.wgsl` and
// `transport_physics/05_nee_env_sampling.wgsl` (from `color::cie1931::CIE_1931_TABLE`); keep all
// copies identical. Plain f32 in the same order
// as the Rust function (no fma, no f64), so the results match up to driver-level rounding.

const CIE_15_2004_CMF_START_NM: f32 = 380.0;
const CIE_15_2004_CMF_STEP_NM: f32 = 5.0;
const CIE_15_2004_CMF_LAST_INDEX: u32 = 80u;
// START + 80 * STEP, written out because WGSL const initializers cannot take the table length.
const CIE_15_2004_CMF_END_NM: f32 = 780.0;

const CIE_15_2004_CMF_TABLE: array<vec3<f32>, 81> = array<vec3<f32>, 81>(
    vec3<f32>(0.0014, 0.0000, 0.0065), // 380nm
    vec3<f32>(0.0022, 0.0001, 0.0105), // 385nm
    vec3<f32>(0.0042, 0.0001, 0.0201), // 390nm
    vec3<f32>(0.0076, 0.0002, 0.0362), // 395nm
    vec3<f32>(0.0143, 0.0004, 0.0679), // 400nm
    vec3<f32>(0.0232, 0.0006, 0.1102), // 405nm
    vec3<f32>(0.0435, 0.0012, 0.2074), // 410nm
    vec3<f32>(0.0776, 0.0022, 0.3713), // 415nm
    vec3<f32>(0.1344, 0.0040, 0.6456), // 420nm
    vec3<f32>(0.2148, 0.0073, 1.0391), // 425nm
    vec3<f32>(0.2839, 0.0116, 1.3856), // 430nm
    vec3<f32>(0.3285, 0.0168, 1.6230), // 435nm
    vec3<f32>(0.3483, 0.0230, 1.7471), // 440nm
    vec3<f32>(0.3481, 0.0298, 1.7826), // 445nm
    vec3<f32>(0.3362, 0.0380, 1.7721), // 450nm
    vec3<f32>(0.3187, 0.0480, 1.7441), // 455nm
    vec3<f32>(0.2908, 0.0600, 1.6692), // 460nm
    vec3<f32>(0.2511, 0.0739, 1.5281), // 465nm
    vec3<f32>(0.1954, 0.0910, 1.2876), // 470nm
    vec3<f32>(0.1421, 0.1126, 1.0419), // 475nm
    vec3<f32>(0.0956, 0.1390, 0.8130), // 480nm
    vec3<f32>(0.0580, 0.1693, 0.6162), // 485nm
    vec3<f32>(0.0320, 0.2080, 0.4652), // 490nm
    vec3<f32>(0.0147, 0.2586, 0.3533), // 495nm
    vec3<f32>(0.0049, 0.3230, 0.2720), // 500nm
    vec3<f32>(0.0024, 0.4073, 0.2123), // 505nm
    vec3<f32>(0.0093, 0.5030, 0.1582), // 510nm
    vec3<f32>(0.0291, 0.6082, 0.1117), // 515nm
    vec3<f32>(0.0633, 0.7100, 0.0782), // 520nm
    vec3<f32>(0.1096, 0.7932, 0.0573), // 525nm
    vec3<f32>(0.1655, 0.8620, 0.0422), // 530nm
    vec3<f32>(0.2257, 0.9149, 0.0298), // 535nm
    vec3<f32>(0.2904, 0.9540, 0.0203), // 540nm
    vec3<f32>(0.3597, 0.9803, 0.0134), // 545nm
    vec3<f32>(0.4334, 0.9950, 0.0087), // 550nm
    vec3<f32>(0.5121, 1.0000, 0.0057), // 555nm
    vec3<f32>(0.5945, 0.9950, 0.0039), // 560nm
    vec3<f32>(0.6784, 0.9786, 0.0027), // 565nm
    vec3<f32>(0.7621, 0.9520, 0.0021), // 570nm
    vec3<f32>(0.8425, 0.9154, 0.0018), // 575nm
    vec3<f32>(0.9163, 0.8700, 0.0017), // 580nm
    vec3<f32>(0.9786, 0.8163, 0.0014), // 585nm
    vec3<f32>(1.0263, 0.7570, 0.0011), // 590nm
    vec3<f32>(1.0567, 0.6949, 0.0010), // 595nm
    vec3<f32>(1.0622, 0.6310, 0.0008), // 600nm
    vec3<f32>(1.0456, 0.5668, 0.0006), // 605nm
    vec3<f32>(1.0026, 0.5030, 0.0003), // 610nm
    vec3<f32>(0.9384, 0.4412, 0.0002), // 615nm
    vec3<f32>(0.8544, 0.3810, 0.0002), // 620nm
    vec3<f32>(0.7514, 0.3210, 0.0001), // 625nm
    vec3<f32>(0.6424, 0.2650, 0.0000), // 630nm
    vec3<f32>(0.5419, 0.2170, 0.0000), // 635nm
    vec3<f32>(0.4479, 0.1750, 0.0000), // 640nm
    vec3<f32>(0.3608, 0.1382, 0.0000), // 645nm
    vec3<f32>(0.2835, 0.1070, 0.0000), // 650nm
    vec3<f32>(0.2187, 0.0816, 0.0000), // 655nm
    vec3<f32>(0.1649, 0.0610, 0.0000), // 660nm
    vec3<f32>(0.1212, 0.0446, 0.0000), // 665nm
    vec3<f32>(0.0874, 0.0320, 0.0000), // 670nm
    vec3<f32>(0.0636, 0.0232, 0.0000), // 675nm
    vec3<f32>(0.0468, 0.0170, 0.0000), // 680nm
    vec3<f32>(0.0329, 0.0119, 0.0000), // 685nm
    vec3<f32>(0.0227, 0.0082, 0.0000), // 690nm
    vec3<f32>(0.0158, 0.0057, 0.0000), // 695nm
    vec3<f32>(0.0114, 0.0041, 0.0000), // 700nm
    vec3<f32>(0.0081, 0.0029, 0.0000), // 705nm
    vec3<f32>(0.0058, 0.0021, 0.0000), // 710nm
    vec3<f32>(0.0041, 0.0015, 0.0000), // 715nm
    vec3<f32>(0.0029, 0.0010, 0.0000), // 720nm
    vec3<f32>(0.0020, 0.0007, 0.0000), // 725nm
    vec3<f32>(0.0014, 0.0005, 0.0000), // 730nm
    vec3<f32>(0.0010, 0.0004, 0.0000), // 735nm
    vec3<f32>(0.0007, 0.0002, 0.0000), // 740nm
    vec3<f32>(0.0005, 0.0002, 0.0000), // 745nm
    vec3<f32>(0.0003, 0.0001, 0.0000), // 750nm
    vec3<f32>(0.0002, 0.0001, 0.0000), // 755nm
    vec3<f32>(0.0002, 0.0001, 0.0000), // 760nm
    vec3<f32>(0.0001, 0.0000, 0.0000), // 765nm
    vec3<f32>(0.0001, 0.0000, 0.0000), // 770nm
    vec3<f32>(0.0001, 0.0000, 0.0000), // 775nm
    vec3<f32>(0.0000, 0.0000, 0.0000), // 780nm
);

fn cie_1931_cmf(l: f32) -> vec3<f32> {
    if (!(CIE_15_2004_CMF_START_NM <= l && l <= CIE_15_2004_CMF_END_NM)) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    let position = (l - CIE_15_2004_CMF_START_NM) / CIE_15_2004_CMF_STEP_NM;
    let index0 = min(u32(floor(position)), CIE_15_2004_CMF_LAST_INDEX - 1u);
    let index1 = index0 + 1u;
    let t = position - f32(index0);
    let lo = CIE_15_2004_CMF_TABLE[index0];
    let hi = CIE_15_2004_CMF_TABLE[index1];
    return lo + (hi - lo) * t;
}

/// XYZ estimate of one (pixel, sample) tuple: jittered camera ray, hero-wavelength comb and CMF
/// integration against the uniform environment. Used by both entry points below.
fn furnace_sample_xyz(pixel: u32, sample: u32) -> vec3<f32> {
    let x = f32(pixel % u32(camera.width));
    let y = f32(pixel / u32(camera.width));

    let seed = hash_u32((pixel * 0x9e3779b9u) ^ (sample * 0x85ebca6bu));
    let jx = f32(hash_u32(seed) % 10000u) / 10000.0 - 0.5;
    let jy = f32(hash_u32(seed + 0x7feb352du) % 10000u) / 10000.0 - 0.5;

    // Camera ray generation as in the real path, kept for pipeline fidelity; the environment
    // ignores `dir`.
    let aspect = camera.width / camera.height;
    let u = ((x + jx) / camera.width - 0.5) * 2.0 * aspect * camera.fov_tan;
    let v = (0.5 - (y + jy) / camera.height) * 2.0 * camera.fov_tan;
    let dir = normalize(camera.forward + camera.right * u + camera.up * v);
    _ = dir;

    let hero_hash = hash_u32(seed);
    let hero_rand = f32(hero_hash) / 4294967295.0;
    let channel_width = SPECTRUM_SPAN / f32(NUM_CHANNELS);
    let lambda_hero = fma(hero_rand, SPECTRUM_SPAN, SPECTRUM_MIN);

    var xyz = vec3<f32>(0.0, 0.0, 0.0);
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        let offset = fma(f32(k), channel_width, lambda_hero - SPECTRUM_MIN);
        let wrapped = offset % SPECTRUM_SPAN;
        let lambda = SPECTRUM_MIN + wrapped;
        // Uniform environment: `extra.l0` for every `lambda`. The spectral MIS weight is 1.0,
        // as in `integrate_channels_to_xyz` when all channels share one path pdf.
        xyz = xyz + cie_1931_cmf(lambda) * (extra.l0 * NORM_FACTOR);
    }
    return xyz;
}

@compute @workgroup_size(64)
fn furnace_samples_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    let total = extra.num_pixels * camera.num_samples;
    if (idx >= total) {
        return;
    }
    let pixel = idx / camera.num_samples;
    let sample = idx % camera.num_samples;
    let xyz = furnace_sample_xyz(pixel, sample);
    out_persample[idx * 3u + 0u] = xyz.x;
    out_persample[idx * 3u + 1u] = xyz.y;
    out_persample[idx * 3u + 2u] = xyz.z;
}

// Sequential per-thread sum over `camera.num_samples`, the pattern of `self_determinism.wgsl`:
// no atomics or cross-thread reduction, so the sum is bit-reproducible under any scheduling.
@compute @workgroup_size(64)
fn furnace_accumulate_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let pixel = gid.x;
    if (pixel >= extra.num_pixels) {
        return;
    }
    var sum = vec3<f32>(0.0, 0.0, 0.0);
    for (var s: u32 = 0u; s < camera.num_samples; s = s + 1u) {
        sum = sum + furnace_sample_xyz(pixel, s);
    }
    out_pixel_sum[pixel * 3u + 0u] = sum.x;
    out_pixel_sum[pixel * 3u + 1u] = sum.y;
    out_pixel_sum[pixel * 3u + 2u] = sum.z;
}
