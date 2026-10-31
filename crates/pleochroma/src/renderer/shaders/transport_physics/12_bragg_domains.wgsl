// ---------------------------------------------------------------------------------
// Bragg domains GPU port (precious opal play-of-color, labradorite schiller)
// (optics::inclusions::{domains, bragg}, optics::raytracer::scattering::delta_tracking).
//
// The CPU is the reference: every constant, RNG stream and arithmetic order below mirrors it.
// The stone's domain map is a jittered grid (a seed point in every cell, the domain at x is the one
// with the nearest seed point among the 27 cells around x) whose per-domain quantities are hashes
// of the cell and the design seed, so it is rebuilt here bit for bit: the hash floats are
// `(h >> 8) / 2^24`. A ray travelling through it is tracked by spectral delta tracking with the
// hero wavelength (channel 0): a majorant drives tentative collisions, the hero accepts a real
// reflection with probability sigma_hero / majorant, and every channel owes the ratio of its own
// collision probability to the hero's. A reflection is a specular event with a mosaic lobe; its
// direction does not depend on the wavelength.
//
// The structure travels in the inclusion block (`renderer::buffers::structure_rows`): header.z is
// the seed, row 0 = [cell_size, lambda_low, lambda_high, purity], row 1 = [coverage, mosaic,
// kappa, majorant], row 2 = [normal xyz, 0] (zero normal: a random one per domain).
//
// Ported here (not in `transport_bounce`) so the megakernel, both wavefronts and Tier 2's
// standalone kernel call the SAME function objects. Nothing in this file reads `material`.
//
// WGSL shape rules: no function returns an array, no loop-indexed local array carries data (the
// eight channels travel as vec4 pairs inside the tracker), every function-scope `var` is
// initialised explicitly. Needs `11_oriented_scattering.wgsl` (OrientedMedium, ori_*).
// ---------------------------------------------------------------------------------

// delta_tracking::{BRAGG_STEP_STREAM, BRAGG_ACCEPT_STREAM, BRAGG_MOSAIC_U_STREAM,
// BRAGG_MOSAIC_V_STREAM, MAX_TENTATIVE_COLLISIONS}.
const BRAGG_STEP_STREAM: u32 = 0xB5AD4ECEu;
const BRAGG_ACCEPT_STREAM: u32 = 0xE3D28F51u;
const BRAGG_MOSAIC_U_STREAM: u32 = 0x1D8E4E27u;
const BRAGG_MOSAIC_V_STREAM: u32 = 0x8CC70208u;
const BRAGG_MAX_COLLISIONS: u32 = 1024u;

// optics::inclusions::bragg::{SECOND_ORDER_STRENGTH, MIN_SECOND_ORDER_NM}.
const BRAGG_SECOND_ORDER_STRENGTH: f32 = 0.3;
const BRAGG_MIN_SECOND_ORDER_NM: f32 = 380.0;

// optics::inclusions::domains hash salts.
const BRAGG_SALT_JITTER_X: u32 = 0x9E3779B9u;
const BRAGG_SALT_JITTER_Y: u32 = 0x7F4A7C15u;
const BRAGG_SALT_JITTER_Z: u32 = 0xF39CC060u;
const BRAGG_SALT_ACTIVE: u32 = 0x68E31DA4u;
const BRAGG_SALT_COLOR_A: u32 = 0xB5297A4Du;
const BRAGG_SALT_COLOR_B: u32 = 0x1B56C4E9u;
const BRAGG_SALT_NORMAL_A: u32 = 0x2C1B3C6Du;
const BRAGG_SALT_NORMAL_B: u32 = 0x297A2D39u;

const BRAGG_TAU: f32 = 6.2831855;

// optics::inclusions::bragg::BraggStructure, in the stone frame.
struct BraggStructure {
    seed: u32,
    cell_size: f32,
    lambda_low: f32,
    lambda_high: f32,
    purity: f32,
    coverage: f32,
    mosaic: f32,
    // -ln(1 - R0) / cell_size, and its majorant over every domain, direction and wavelength.
    kappa: f32,
    majorant: f32,
    // The shared lattice normal, or zero for a random normal per domain.
    normal: vec3<f32>,
}

// The structure from the inclusion block's seed (`header.z`) and its three rows.
fn bragg_structure(seed: u32, r0: vec4<f32>, r1: vec4<f32>, r2: vec4<f32>) -> BraggStructure {
    return BraggStructure(seed, r0.x, r0.y, r0.z, r0.w, r1.x, r1.y, r1.z, r1.w, r2.xyz);
}

// domains::Domain without its id and cell (nothing on the GPU reads them).
struct BraggDomain {
    // Whether the patch reflects at all.
    on: bool,
    // The Bragg wavelength at normal incidence, nanometres.
    lambda_b0: f32,
    // The unit lattice normal.
    normal: vec3<f32>,
}

// domains::unit_float: the top 24 bits over 2^24, exact in f32.
fn bragg_unit_float(h: u32) -> f32 {
    return f32(h >> 8u) * (1.0 / 16777216.0);
}

// domains::cell_hash.
fn bragg_cell_hash(cell: vec3<i32>, seed: u32, salt: u32) -> u32 {
    var h = hash_u32(seed ^ salt);
    h = hash_u32(h ^ bitcast<u32>(cell.x));
    h = hash_u32(h ^ bitcast<u32>(cell.y));
    h = hash_u32(h ^ bitcast<u32>(cell.z));
    return h;
}

// cos(2 pi u) and sin(2 pi u) for u in [0, 1), with the angle reduced to [-pi, pi] first (the
// GPU's sin and cos are only specified there; `u - 1` is exact for u >= 0.5).
fn bragg_cos_tau(u: f32) -> f32 {
    return cos(BRAGG_TAU * select(u, u - 1.0, u > 0.5));
}

fn bragg_sin_tau(u: f32) -> f32 {
    return sin(BRAGG_TAU * select(u, u - 1.0, u > 0.5));
}

// domains::seed_point.
fn bragg_seed_point(st: BraggStructure, cell: vec3<i32>) -> vec3<f32> {
    let jitter = vec3<f32>(
        bragg_unit_float(bragg_cell_hash(cell, st.seed, BRAGG_SALT_JITTER_X)),
        bragg_unit_float(bragg_cell_hash(cell, st.seed, BRAGG_SALT_JITTER_Y)),
        bragg_unit_float(bragg_cell_hash(cell, st.seed, BRAGG_SALT_JITTER_Z)),
    );
    return (vec3<f32>(cell) + jitter) * st.cell_size;
}

// domains::domain_of_cell.
fn bragg_domain_of_cell(st: BraggStructure, cell: vec3<i32>) -> BraggDomain {
    let seed = st.seed;
    let on = bragg_unit_float(bragg_cell_hash(cell, seed, BRAGG_SALT_ACTIVE)) < st.coverage;

    // Gaussian about the middle of the color range, a quarter of its width as the standard
    // deviation (Box-Muller), clamped to the range.
    let u1 = max(bragg_unit_float(bragg_cell_hash(cell, seed, BRAGG_SALT_COLOR_A)), 1e-7);
    let u2 = bragg_unit_float(bragg_cell_hash(cell, seed, BRAGG_SALT_COLOR_B));
    let normal_draw = sqrt(-2.0 * log(u1)) * bragg_cos_tau(u2);
    let low = st.lambda_low;
    let high = st.lambda_high;
    let lambda_b0 = clamp(0.5 * (low + high) + 0.25 * (high - low) * normal_draw, low, high);

    var normal = st.normal;
    if (st.normal.x == 0.0 && st.normal.y == 0.0 && st.normal.z == 0.0) {
        // Opal: a uniformly random lattice orientation.
        let z = 1.0 - 2.0 * bragg_unit_float(bragg_cell_hash(cell, seed, BRAGG_SALT_NORMAL_A));
        let ph = bragg_unit_float(bragg_cell_hash(cell, seed, BRAGG_SALT_NORMAL_B));
        let r = sqrt(max(1.0 - z * z, 0.0));
        normal = vec3<f32>(r * bragg_cos_tau(ph), r * bragg_sin_tau(ph), z);
    }
    return BraggDomain(on, lambda_b0, normal);
}

// domains::domain_at: the domain with the nearest seed point among the 27 cells around `p`
// (a tie goes to the cell met first in the fixed x, y, z loop order).
fn bragg_domain_at(st: BraggStructure, p: vec3<f32>) -> BraggDomain {
    let home = vec3<i32>(floor(p / st.cell_size));
    var best = home;
    var best_d2: f32 = 3.0e38;
    for (var dx: i32 = -1; dx <= 1; dx = dx + 1) {
        for (var dy: i32 = -1; dy <= 1; dy = dy + 1) {
            for (var dz: i32 = -1; dz <= 1; dz = dz + 1) {
                let cell = vec3<i32>(home.x + dx, home.y + dy, home.z + dz);
                let d = bragg_seed_point(st, cell) - p;
                let d2 = dot(d, d);
                if (d2 < best_d2) {
                    best_d2 = d2;
                    best = cell;
                }
            }
        }
    }
    return bragg_domain_of_cell(st, best);
}

// bragg::spectral_profile: a Gaussian around the first-order peak plus the second order while
// that stays at or above 380 nm.
fn bragg_gaussian(lambda_nm: f32, center: f32, purity: f32) -> f32 {
    let d = (lambda_nm - center) / (purity * center);
    return exp(-0.5 * d * d);
}

fn bragg_spectral_profile(lambda_nm: f32, lambda_b: f32, purity: f32) -> f32 {
    if (lambda_b <= 0.0) {
        return 0.0;
    }
    var value = bragg_gaussian(lambda_nm, lambda_b, purity);
    let second = 0.5 * lambda_b;
    if (second >= BRAGG_MIN_SECOND_ORDER_NM) {
        value = value + BRAGG_SECOND_ORDER_STRENGTH * bragg_gaussian(lambda_nm, second, purity);
    }
    return value;
}

// BraggStructure::sigma: the reflection coefficient of `dom` for a ray at cosine `cos_theta` to
// its normal, at `lambda_nm`, per model unit.
fn bragg_sigma(st: BraggStructure, dom: BraggDomain, cos_theta: f32, lambda_nm: f32) -> f32 {
    if (!dom.on) {
        return 0.0;
    }
    let lambda_b = dom.lambda_b0 * abs(cos_theta);
    return st.kappa * bragg_spectral_profile(lambda_nm, lambda_b, st.purity);
}

// `bragg_sigma` for four wavelengths at once.
fn bragg_sigma4(st: BraggStructure, dom: BraggDomain, cos_theta: f32, lam: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(
        bragg_sigma(st, dom, cos_theta, lam.x),
        bragg_sigma(st, dom, cos_theta, lam.y),
        bragg_sigma(st, dom, cos_theta, lam.z),
        bragg_sigma(st, dom, cos_theta, lam.w),
    );
}

// delta_tracking::draw: the unit random of `stream` at (rng_seed, bounce) and tentative
// collision `index`, limited to [0, 1).
fn bragg_draw(rng_seed: u32, bounce: u32, stream: u32, index: u32) -> f32 {
    let salt = hash_u32(index * 0x9E3779B9u + stream);
    let u = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ salt))) / 4294967295.0;
    return min(u, 1.0 - 1e-6);
}

// delta_tracking::BraggTrack: no reflection (`reflect == 0u`) or the hero's first accepted one,
// with the per-channel null-collision products before it (a reflection's own ratio included
// for the non-hero channels, as the CPU folds it into the same weight). Channel k is
// `w_lo[k]` (k < 4) or `w_hi[k - 4]`.
struct BraggTrackOut {
    reflect: u32,
    t_model: f32,
    normal: vec3<f32>,
    w_lo: vec4<f32>,
    w_hi: vec4<f32>,
}

// delta_tracking::track_bragg, the hero being channel 0 (`lam_lo.x`): tracks the segment
// `origin + dir * t` for `t < t_max_model`.
fn bragg_track_hero(
    st: BraggStructure,
    origin: vec3<f32>,
    dir: vec3<f32>,
    t_max_model: f32,
    lam_lo: vec4<f32>,
    lam_hi: vec4<f32>,
    rng_seed: u32,
    bounce: u32,
) -> BraggTrackOut {
    let sigma_bar = st.majorant;
    var w_lo = vec4<f32>(1.0, 1.0, 1.0, 1.0);
    var w_hi = vec4<f32>(1.0, 1.0, 1.0, 1.0);
    var t: f32 = 0.0;
    for (var i: u32 = 0u; i < BRAGG_MAX_COLLISIONS; i = i + 1u) {
        let u_step = bragg_draw(rng_seed, bounce, BRAGG_STEP_STREAM, i);
        t = t + (-log(1.0 - u_step)) / sigma_bar;
        if (t >= t_max_model) {
            break;
        }
        let dom = bragg_domain_at(st, origin + dir * t);
        let cos_theta = abs(dot(dir, dom.normal));
        let s_lo = bragg_sigma4(st, dom, cos_theta, lam_lo);
        let s_hi = bragg_sigma4(st, dom, cos_theta, lam_hi);
        let s_hero = s_lo.x;
        let p_hero = clamp(s_hero / sigma_bar, 0.0, 1.0);
        if (bragg_draw(rng_seed, bounce, BRAGG_ACCEPT_STREAM, i) < p_hero) {
            // The real reflection's own ratio, sigma_k / sigma_hero; the hero's weight is one.
            let r_lo = vec4<f32>(1.0, w_lo.y * (s_lo.y / s_hero), w_lo.z * (s_lo.z / s_hero), w_lo.w * (s_lo.w / s_hero));
            let r_hi = w_hi * (s_hi / s_hero);
            return BraggTrackOut(1u, t, dom.normal, r_lo, r_hi);
        }
        let null_hero = max(1.0 - p_hero, 1e-6);
        let zero = vec4<f32>(0.0, 0.0, 0.0, 0.0);
        let one = vec4<f32>(1.0, 1.0, 1.0, 1.0);
        let n_lo = w_lo * (max(one - s_lo / sigma_bar, zero) / null_hero);
        w_lo = vec4<f32>(1.0, n_lo.y, n_lo.z, n_lo.w);
        w_hi = w_hi * (max(one - s_hi / sigma_bar, zero) / null_hero);
    }
    return BraggTrackOut(0u, 0.0, vec3<f32>(0.0, 0.0, 0.0), w_lo, w_hi);
}

// `bragg_track_hero`s weight of channel `k`.
fn bragg_channel_weight(tr: BraggTrackOut, k: u32) -> f32 {
    if (k < 4u) {
        return tr.w_lo[k & 3u];
    }
    return tr.w_hi[k & 3u];
}

// delta_tracking::bragg_reflect_weights, the hero being channel 0: a reflection's per-channel
// weight in a host whose optical depth is not linear in the distance (a zoned host),
// `exp(-(depth_k - depth_0))` times the tracking weight of `tr`. `depth_lo`/`depth_hi` are the
// optical depths of channels 0..4 and 4..8 at the reflection. Returns `tr` with `w_lo`/`w_hi`
// replaced, so `bragg_channel_weight` reads the result.
fn bragg_reflect_weights4(depth_lo: vec4<f32>, depth_hi: vec4<f32>, tr: BraggTrackOut) -> BraggTrackOut {
    let d0 = depth_lo.x;
    let w_lo = vec4<f32>(
        exp_poly(-(depth_lo.x - d0)) * tr.w_lo.x,
        exp_poly(-(depth_lo.y - d0)) * tr.w_lo.y,
        exp_poly(-(depth_lo.z - d0)) * tr.w_lo.z,
        exp_poly(-(depth_lo.w - d0)) * tr.w_lo.w,
    );
    let w_hi = vec4<f32>(
        exp_poly(-(depth_hi.x - d0)) * tr.w_hi.x,
        exp_poly(-(depth_hi.y - d0)) * tr.w_hi.y,
        exp_poly(-(depth_hi.z - d0)) * tr.w_hi.z,
        exp_poly(-(depth_hi.w - d0)) * tr.w_hi.w,
    );
    return BraggTrackOut(tr.reflect, tr.t_model, tr.normal, w_lo, w_hi);
}

// bragg::bragg_reflect with the mosaic perturbation's two randoms (`BRAGG_MOSAIC_U/V_STREAM` at
// index 0): `w` mirrored in the lattice normal perturbed by the mosaic spread, falling back to
// `w` when the mirror degenerates.
fn bragg_reflect_dir(w: vec3<f32>, normal: vec3<f32>, mosaic: f32, rng_seed: u32, bounce: u32) -> vec3<f32> {
    let u1 = bragg_draw(rng_seed, bounce, BRAGG_MOSAIC_U_STREAM, 0u);
    let u2 = bragg_draw(rng_seed, bounce, BRAGG_MOSAIC_V_STREAM, 0u);
    let reflected = ori_platelet_sample(w, normal, mosaic, u1, u2);
    if (reflected.x == 0.0 && reflected.y == 0.0 && reflected.z == 0.0) {
        return w;
    }
    return reflected;
}

// delta_tracking::BraggScatter: `scattered == 0u` is the CPU's `None`; `bragg != 0u` marks a
// Bragg reflection (no scattering-point NEE, no phase MIS partner, no phase ratio), `t_free`
// is in model units.
struct BraggScatterResult {
    scattered: u32,
    bragg: u32,
    t_free: f32,
    new_dir: vec3<f32>,
}

// delta_tracking::maybe_scatter_oriented_bragg: `maybe_scatter_oriented` for a medium that also
// holds a Bragg domain structure. The homogeneous free flight is drawn first (the very draw
// `maybe_scatter_oriented` makes), tracking runs over [0, min(t_homogeneous, hit_t)), and
// whichever event comes first wins. `stokes`/`path_pdf` are updated in place; the hero is
// channel 0.
fn maybe_scatter_oriented_bragg(
    m: OrientedMedium,
    st: BraggStructure,
    lambdas: array<f32, 8>,
    alphas: array<f32, 8>,
    origin: vec3<f32>,
    ray_dir: vec3<f32>,
    hit_t: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
) -> BraggScatterResult {
    // The homogeneous extinction per channel, per absorption-length unit, and the hero's free
    // flight.
    var sigma_s: array<f32, 8>;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        sigma_s[k] = ori_sigma_s(m, ray_dir, lambdas[k]) / m.path_scale;
    }
    let sigma_t_hero = max(alphas[0] + sigma_s[0], 1e-12);
    let dist_rand = ori_stream_draw(rng_seed, bounce, DISTANCE_SAMPLE_STREAM);
    let t_homogeneous = -(log(max(1.0 - dist_rand, 1e-7))) / sigma_t_hero / m.path_scale;

    let lam_lo = vec4<f32>(lambdas[0], lambdas[1], lambdas[2], lambdas[3]);
    let lam_hi = vec4<f32>(lambdas[4], lambdas[5], lambdas[6], lambdas[7]);
    let tr = bragg_track_hero(st, origin, ray_dir, min(t_homogeneous, hit_t), lam_lo, lam_hi, rng_seed, bounce);

    if (tr.reflect != 0u) {
        // The homogeneous medium's survival to the reflection, relative to the hero's, times the
        // tracking ratio.
        let t_abs = tr.t_model * m.path_scale;
        for (var k: u32 = 0u; k < 8u; k = k + 1u) {
            let arg = -((alphas[k] + sigma_s[k]) - (alphas[0] + sigma_s[0])) * t_abs;
            let w = exp_poly(arg) * bragg_channel_weight(tr, k);
            (*stokes)[k] = vec4<f32>(max((*stokes)[k].x, 0.0) * w, 0.0, 0.0, 0.0);
            (*path_pdf)[k] = (*path_pdf)[k] * w;
        }
        let new_dir = bragg_reflect_dir(ray_dir, tr.normal, st.mosaic, rng_seed, bounce);
        return BraggScatterResult(1u, 1u, tr.t_model, new_dir);
    }

    // Nothing found: the homogeneous event decides as before, and the null-collision ratios
    // multiply the result.
    let sc = maybe_scatter_oriented(m, lambdas, alphas, ray_dir, hit_t, rng_seed, bounce, stokes, path_pdf);
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        let w = bragg_channel_weight(tr, k);
        (*stokes)[k] = (*stokes)[k] * w;
        (*path_pdf)[k] = (*path_pdf)[k] * w;
    }
    return BraggScatterResult(sc.scattered, 0u, sc.t_free, sc.new_dir);
}
