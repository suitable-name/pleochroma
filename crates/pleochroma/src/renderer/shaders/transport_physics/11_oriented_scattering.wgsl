// ---------------------------------------------------------------------------------
// Oriented scattering GPU port (silk, needles, platelets)
// (optics::raytracer::scattering::oriented, optics::inclusions::{needle, platelet, spectral}).
//
// The CPU is the reference: every constant, RNG stream and arithmetic order below mirrors it.
// The medium is the plain Henyey-Greenstein haze plus up to four families; the event picks one
// component with probability sigma_c(w, lambda_hero) / sigma_s(w, lambda_hero) and samples its
// phase function, the direction being shared by every channel. The per-channel ratio of the
// mixture phases is NOT applied here: `bounce_scatter_commit` recomputes it from the old and
// the new direction (`ori_channel_phases`), after the scattering-point next-event estimate has
// used the weight without it, exactly as `try_scatter_step_oriented` orders it.
//
// Ported here (not in `transport_bounce`) for the reason `maybe_scatter_or_extinguish` lives in
// `04_scattering.wgsl`: the megakernel, both wavefronts and Tier 2's standalone kernels call the
// SAME function objects. Nothing in this file reads `material`: the medium is passed in.
//
// WGSL shape rules (see the lane rules): no function returns an array, no loop-indexed local
// array carries data (the four families are four named vec4 pairs picked by `ori_dir` /
// `ori_par`), every function-scope `var` is initialised explicitly.
// ---------------------------------------------------------------------------------

// optics::raytracer::scattering::oriented::ORIENTED_COMPONENT_STREAM.
const ORIENTED_COMPONENT_STREAM: u32 = 0x7F4A7C15u;

// optics::raytracer::scattering::oriented::RANDOM_NEEDLE_AXES.
const ORI_RANDOM_AXES: u32 = 64u;
// The golden angle (radians), the Fibonacci spiral's step: `GOLDEN_ANGLE`.
const ORI_GOLDEN_ANGLE: f32 = 2.3999632;
// 2 pi as an exactly representable high part plus the remainder, for the range reduction of
// `ori_random_axis` (a Cody-Waite split: `k * ORI_TWO_PI_HI` is exact for the k <= 24 used).
const ORI_TWO_PI: f32 = 6.2831855;
const ORI_TWO_PI_HI: f32 = 6.28125;
const ORI_TWO_PI_LO: f32 = 0.0019353072;
// pi / 4: the mean extinction factor of needles of every orientation.
const ORI_FRAC_PI_4: f32 = 0.7853982;
// optics::inclusions::spectral::REFERENCE_WAVELENGTH_NM.
const ORI_REFERENCE_NM: f32 = 550.0;
// optics::inclusions::needle::MAX_RHO.
const ORI_MAX_RHO: f32 = 0.98;

// `fam_dir[i]` is (axis or normal xyz, kind w) and `fam_params[i]` is (sigma, p, lobe, rho) of
// `renderer::buffers::GpuInclusionBlock`; kind 0 = needle, 1 = platelet, 2 = random needle.
struct OrientedMedium {
    n_fam: u32,
    // The plain haze's sigma_s per model unit and its asymmetry.
    sigma_hg: f32,
    g: f32,
    // GemMaterial::absorption_path_scale.
    path_scale: f32,
    dir0: vec4<f32>,
    dir1: vec4<f32>,
    dir2: vec4<f32>,
    dir3: vec4<f32>,
    par0: vec4<f32>,
    par1: vec4<f32>,
    par2: vec4<f32>,
    par3: vec4<f32>,
}

// One value per component: the haze and the four families (unused families are zero).
struct OriComponents {
    h: f32,
    f: vec4<f32>,
}

fn ori_dir(m: OrientedMedium, i: u32) -> vec4<f32> {
    if (i == 0u) {
        return m.dir0;
    }
    if (i == 1u) {
        return m.dir1;
    }
    if (i == 2u) {
        return m.dir2;
    }
    return m.dir3;
}

fn ori_par(m: OrientedMedium, i: u32) -> vec4<f32> {
    if (i == 0u) {
        return m.par0;
    }
    if (i == 1u) {
        return m.par1;
    }
    if (i == 2u) {
        return m.par2;
    }
    return m.par3;
}

// `x` rounded into [0, 1) for use as a remapped random: `unit`.
fn ori_unit(x: f32) -> f32 {
    return clamp(x, 0.0, 1.0 - 1e-6);
}

// `stream_draw`: the unit random of the RNG stream `stream` at (rng_seed, bounce).
fn ori_stream_draw(rng_seed: u32, bounce: u32, stream: u32) -> f32 {
    return f32(hash_u32(rng_seed ^ hash_u32(bounce ^ stream))) / 4294967295.0;
}

// optics::inclusions::spectral::spectral_factor: (550 / lambda)^p for p in {0, 1, 2, 4}, by
// multiplication only. `p` travels as a float.
fn ori_spectral_factor(lambda_nm: f32, p: f32) -> f32 {
    let pe = u32(p + 0.5);
    if (pe == 0u || !(lambda_nm > 0.0)) {
        return 1.0;
    }
    let r = ORI_REFERENCE_NM / lambda_nm;
    if (pe == 1u) {
        return r;
    }
    if (pe == 2u || pe == 3u) {
        return r * r;
    }
    let r2 = r * r;
    return r2 * r2;
}

// oriented::family_extinction: the extinction of one family for the propagation direction `w`
// at `lambda_nm`, per model unit.
fn ori_family_extinction(d: vec4<f32>, par: vec4<f32>, w: vec3<f32>, lambda_nm: f32) -> f32 {
    var shape: f32 = ORI_FRAC_PI_4;
    if (d.w < 0.5) {
        let along = dot(w, d.xyz);
        shape = sqrt(max(1.0 - along * along, 0.0));
    } else if (d.w < 1.5) {
        shape = abs(dot(w, d.xyz));
    }
    return par.x * ori_spectral_factor(lambda_nm, par.y) * shape;
}

// OrientedMedium::extinctions: the haze at `h`, the families in `f`.
fn ori_extinctions(m: OrientedMedium, w: vec3<f32>, lambda_nm: f32) -> OriComponents {
    var f = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    if (m.n_fam > 0u) {
        f.x = ori_family_extinction(m.dir0, m.par0, w, lambda_nm);
    }
    if (m.n_fam > 1u) {
        f.y = ori_family_extinction(m.dir1, m.par1, w, lambda_nm);
    }
    if (m.n_fam > 2u) {
        f.z = ori_family_extinction(m.dir2, m.par2, w, lambda_nm);
    }
    if (m.n_fam > 3u) {
        f.w = ori_family_extinction(m.dir3, m.par3, w, lambda_nm);
    }
    return OriComponents(max(m.sigma_hg, 0.0), f);
}

// The components summed in the CPU's order (haze first, then the families).
fn ori_total(c: OriComponents) -> f32 {
    return (((c.h + c.f.x) + c.f.y) + c.f.z) + c.f.w;
}

// Component `i` (0 = haze, 1..4 = families).
fn ori_component(c: OriComponents, i: u32) -> f32 {
    if (i == 0u) {
        return c.h;
    }
    return c.f[i - 1u];
}

// OrientedMedium::sigma_s: the total scattering coefficient for direction `w` at `lambda_nm`,
// per model unit.
fn ori_sigma_s(m: OrientedMedium, w: vec3<f32>, lambda_nm: f32) -> f32 {
    return ori_total(ori_extinctions(m, w, lambda_nm));
}

// ---- needle (optics::inclusions::needle) ----

struct OriBasis {
    t: vec3<f32>,
    b: vec3<f32>,
}

// inclusions::perpendicular_basis.
fn ori_perp_basis(n: vec3<f32>) -> OriBasis {
    var a = vec3<f32>(1.0, 0.0, 0.0);
    if (abs(n.x) > 0.9) {
        a = vec3<f32>(0.0, 1.0, 0.0);
    }
    let t = normalize_or_zero(a - n * dot(n, a));
    return OriBasis(t, cross(n, t));
}

fn ori_logistic_cdf(x: f32) -> f32 {
    return 1.0 / (1.0 + exp(-clamp(x, -40.0, 40.0)));
}

// needle::truncation: (mass of the kernel inside [-1, 1], kernel CDF at the lower end).
struct OriTruncation {
    mass: f32,
    lo: f32,
}

fn ori_truncation(xi: f32, beta: f32) -> OriTruncation {
    let lo = ori_logistic_cdf((-1.0 - xi) / beta);
    let hi = ori_logistic_cdf((1.0 - xi) / beta);
    return OriTruncation(max(hi - lo, 1e-12), lo);
}

fn ori_needle_s_density(xi: f32, s: f32, beta: f32) -> f32 {
    if (beta <= 0.0) {
        return 0.0;
    }
    let tr = ori_truncation(xi, beta);
    let e = exp(-min(abs((s - xi) / beta), 40.0));
    return e / (beta * (1.0 + e) * (1.0 + e) * tr.mass);
}

fn ori_wrapped_cauchy(cos_phi: f32, rho_in: f32) -> f32 {
    let rho = clamp(rho_in, 0.0, ORI_MAX_RHO);
    return (1.0 - rho * rho) / (2.0 * PI * (1.0 + rho * rho - 2.0 * rho * cos_phi));
}

// needle::azimuth_origin: the unit vector the azimuth is measured from, and whether it is
// defined by `w_i` (false when `w_i` is along the axis).
struct OriAzimuth {
    u: vec3<f32>,
    defined: bool,
}

fn ori_azimuth_origin(w_i: vec3<f32>, t: vec3<f32>) -> OriAzimuth {
    let perp = w_i - t * dot(w_i, t);
    let len = length(perp);
    if (len < 1e-6) {
        return OriAzimuth(ori_perp_basis(t).t, false);
    }
    return OriAzimuth(perp / len, true);
}

// needle::needle_sample.
fn ori_needle_sample(w_i: vec3<f32>, t: vec3<f32>, beta: f32, rho_in: f32, u1: f32, u2: f32) -> vec3<f32> {
    let xi = clamp(dot(w_i, t), -1.0, 1.0);
    var s: f32 = xi;
    if (beta > 0.0) {
        let tr = ori_truncation(xi, beta);
        let v = clamp(tr.lo + u1 * tr.mass, 1e-7, 1.0 - 1.2e-7);
        s = clamp(xi + beta * log(v / (1.0 - v)), -1.0, 1.0);
    }
    let rho = clamp(rho_in, 0.0, ORI_MAX_RHO);
    let a = (1.0 - rho) / (1.0 + rho);
    let phi = 2.0 * atan(a * tan(PI * (clamp(u2, 1e-6, 1.0 - 1e-6) - 0.5)));
    let az = ori_azimuth_origin(w_i, t);
    let v2 = cross(t, az.u);
    let sin_phi = sin(phi);
    let cos_phi = cos(phi);
    let radial = sqrt(max(1.0 - s * s, 0.0));
    return normalize_or_zero(t * s + (az.u * cos_phi + v2 * sin_phi) * radial);
}

// needle::needle_pdf.
fn ori_needle_pdf(w_i: vec3<f32>, t: vec3<f32>, beta: f32, rho: f32, w_o: vec3<f32>) -> f32 {
    if (beta <= 0.0) {
        return 0.0;
    }
    let xi = clamp(dot(w_i, t), -1.0, 1.0);
    let s = clamp(dot(w_o, t), -1.0, 1.0);
    var azimuth: f32 = 0.5 / PI;
    let az = ori_azimuth_origin(w_i, t);
    let perp = w_o - t * dot(w_o, t);
    let len = length(perp);
    if (az.defined && len >= 1e-6) {
        azimuth = ori_wrapped_cauchy(dot(perp, az.u) / len, rho);
    }
    return ori_needle_s_density(xi, s, beta) * azimuth;
}

// ---- platelet (optics::inclusions::platelet) ----

fn ori_ggx_d(cos_m: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let d = fma(cos_m * cos_m, a2 - 1.0, 1.0);
    return a2 / (PI * d * d);
}

// platelet::platelet_sample.
fn ori_platelet_sample(w_i: vec3<f32>, m: vec3<f32>, alpha: f32, u1_in: f32, u2: f32) -> vec3<f32> {
    let a2 = alpha * alpha;
    let u1 = clamp(u1_in, 0.0, 1.0 - 1e-6);
    let tan2 = a2 * u1 / (1.0 - u1);
    let cos_m = 1.0 / sqrt(1.0 + tan2);
    let sin_m = sqrt(max(1.0 - cos_m * cos_m, 0.0));
    let sin_p = sin(2.0 * PI * u2);
    let cos_p = cos(2.0 * PI * u2);
    let bs = ori_perp_basis(m);
    let h = bs.t * (sin_m * cos_p) + bs.b * (sin_m * sin_p) + m * cos_m;
    return normalize_or_zero(w_i - h * (2.0 * dot(w_i, h)));
}

// platelet::platelet_pdf.
fn ori_platelet_pdf(w_i: vec3<f32>, m: vec3<f32>, alpha: f32, w_o: vec3<f32>) -> f32 {
    let hv = w_o - w_i;
    let len = length(hv);
    if (len < 1e-6) {
        return 0.0;
    }
    let h = hv / len;
    let cos_m = abs(dot(h, m));
    let facing = max(abs(dot(w_i, h)), 1e-5);
    return ori_ggx_d(cos_m, alpha) * cos_m / (4.0 * facing);
}

// ---- random needles (oriented::RANDOM_AXES, random_needle_phase / _sample) ----

// Axis `j` of the Fibonacci spiral over the upper hemisphere: z = (j + 0.5) / 64, azimuth
// j * golden angle. The azimuth is range-reduced to [-pi, pi] first (the GPU's sin and cos are
// only specified there; the split constant keeps the reduction accurate to ~1e-7 rad).
fn ori_random_axis(j: u32) -> vec3<f32> {
    let z = (f32(j) + 0.5) / 64.0;
    let r = sqrt(max(1.0 - z * z, 0.0));
    let a = f32(j) * ORI_GOLDEN_ANGLE;
    let k = floor(a / ORI_TWO_PI + 0.5);
    let ar = fma(-k, ORI_TWO_PI_LO, fma(-k, ORI_TWO_PI_HI, a));
    return vec3<f32>(r * cos(ar), r * sin(ar), z);
}

// The sum of the axes' extinction weights sin(theta) for the incoming direction, in axis order.
fn ori_random_total(w_i: vec3<f32>) -> f32 {
    var total: f32 = 0.0;
    for (var j: u32 = 0u; j < ORI_RANDOM_AXES; j = j + 1u) {
        let along = dot(w_i, ori_random_axis(j));
        total = total + sqrt(max(1.0 - along * along, 0.0));
    }
    return total;
}

// random_needle_phase: the discrete mixture of the needle cones weighted by sin(theta).
fn ori_random_needle_phase(w_i: vec3<f32>, beta: f32, rho: f32, w_o: vec3<f32>) -> f32 {
    let total = ori_random_total(w_i);
    if (total <= 0.0) {
        return 0.0;
    }
    var sum: f32 = 0.0;
    for (var j: u32 = 0u; j < ORI_RANDOM_AXES; j = j + 1u) {
        let axis = ori_random_axis(j);
        let along = dot(w_i, axis);
        let weight = sqrt(max(1.0 - along * along, 0.0));
        sum = sum + weight * ori_needle_pdf(w_i, axis, beta, rho, w_o);
    }
    return sum / total;
}

// random_needle_sample: `u_axis` picks the axis, `u1`/`u2` the cone sample.
fn ori_random_needle_sample(w_i: vec3<f32>, beta: f32, rho: f32, u_axis: f32, u1: f32, u2: f32) -> vec3<f32> {
    let total = ori_random_total(w_i);
    let tgt = u_axis * total;
    var acc: f32 = 0.0;
    var chosen: u32 = ORI_RANDOM_AXES - 1u;
    for (var j: u32 = 0u; j < ORI_RANDOM_AXES; j = j + 1u) {
        let along = dot(w_i, ori_random_axis(j));
        acc = acc + sqrt(max(1.0 - along * along, 0.0));
        if (tgt < acc) {
            chosen = j;
            break;
        }
    }
    return ori_needle_sample(w_i, ori_random_axis(chosen), beta, rho, u1, u2);
}

// ---- the mixture ----

// family_phase: the phase function of one family, per steradian.
fn ori_family_phase(d: vec4<f32>, par: vec4<f32>, w_i: vec3<f32>, w_o: vec3<f32>) -> f32 {
    if (d.w < 0.5) {
        return ori_needle_pdf(w_i, d.xyz, par.z, par.w, w_o);
    }
    if (d.w < 1.5) {
        return ori_platelet_pdf(w_i, d.xyz, par.z, w_o);
    }
    return ori_random_needle_phase(w_i, par.z, par.w, w_o);
}

// OrientedMedium::phases: the phase function of every component for the turn w_i -> w_o.
fn ori_phases(m: OrientedMedium, w_i: vec3<f32>, w_o: vec3<f32>) -> OriComponents {
    var h: f32 = 0.0;
    if (m.sigma_hg > 0.0) {
        h = henyey_greenstein_phase(dot(w_i, w_o), m.g);
    }
    var f = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    if (m.n_fam > 0u) {
        f.x = ori_family_phase(m.dir0, m.par0, w_i, w_o);
    }
    if (m.n_fam > 1u) {
        f.y = ori_family_phase(m.dir1, m.par1, w_i, w_o);
    }
    if (m.n_fam > 2u) {
        f.z = ori_family_phase(m.dir2, m.par2, w_i, w_o);
    }
    if (m.n_fam > 3u) {
        f.w = ori_family_phase(m.dir3, m.par3, w_i, w_o);
    }
    return OriComponents(h, f);
}

// oriented::mixture: the normalised mixture phase for component extinctions `sig`.
fn ori_mixture(sig: OriComponents, phases: OriComponents) -> f32 {
    let total = ori_total(sig);
    if (total <= 0.0) {
        return 0.0;
    }
    let weighted = (((sig.h * phases.h + sig.f.x * phases.f.x) + sig.f.y * phases.f.y)
        + sig.f.z * phases.f.z) + sig.f.w * phases.f.w;
    return weighted / total;
}

// OrientedMedium::channel_phases: the mixture phase of every channel (each with its own
// wavelength's weights) into `per_channel`, returning the hero's (channel 0).
fn ori_channel_phases(
    m: OrientedMedium,
    lambdas: array<f32, 8>,
    w_i: vec3<f32>,
    w_o: vec3<f32>,
    per_channel: ptr<function, array<f32, 8>>,
) -> f32 {
    let phases = ori_phases(m, w_i, w_o);
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        (*per_channel)[k] = ori_mixture(ori_extinctions(m, w_i, lambdas[k]), phases);
    }
    return (*per_channel)[0];
}

// OrientedMedium::sample_direction: draws the scattered direction from the mixture at the
// hero wavelength; `u_c` picks the component, `u1`/`u2` drive its sampler.
fn ori_sample_direction(m: OrientedMedium, w_i: vec3<f32>, lambda_hero: f32, u_c: f32, u1: f32, u2: f32) -> vec3<f32> {
    let sig = ori_extinctions(m, w_i, lambda_hero);
    let total = ori_total(sig);
    if (total <= 0.0) {
        // No component can scatter here: fall back to the haze's own distribution.
        return sample_henyey_greenstein_direction(u1, u2, m.g, w_i);
    }
    let tgt = u_c * total;
    var acc: f32 = 0.0;
    var chosen: u32 = 0u;
    var u_local: f32 = 0.0;
    for (var c: u32 = 0u; c < 5u; c = c + 1u) {
        let s = ori_component(sig, c);
        if (s <= 0.0) {
            continue;
        }
        chosen = c;
        u_local = ori_unit((tgt - acc) / s);
        acc = acc + s;
        if (tgt < acc) {
            break;
        }
    }
    if (chosen == 0u) {
        return sample_henyey_greenstein_direction(u1, u2, m.g, w_i);
    }
    let d = ori_dir(m, chosen - 1u);
    let par = ori_par(m, chosen - 1u);
    if (d.w < 0.5) {
        return ori_needle_sample(w_i, d.xyz, par.z, par.w, u1, u2);
    }
    if (d.w < 1.5) {
        return ori_platelet_sample(w_i, d.xyz, par.z, u1, u2);
    }
    return ori_random_needle_sample(w_i, par.z, par.w, u_local, u1, u2);
}

// oriented::maybe_scatter_oriented's event; `scattered == 0u` is the CPU's `None`.
struct OrientedScatterResult {
    scattered: u32,
    // Where along the old direction the event is, model units.
    t_free: f32,
    new_dir: vec3<f32>,
}

// oriented::maybe_scatter_oriented: the per-bounce extinction/scatter estimator for a medium
// with oriented inclusions. The same decisions as `maybe_scatter_or_extinguish` with the
// per-channel coefficient sigma_s,k(w) in place of the scalar and the mixture sampler in place
// of Henyey-Greenstein; channel 0 is the hero. `lambdas` are the ray's eight wavelengths.
// `stokes`/`path_pdf` are updated in place like `maybe_scatter_or_extinguish`'s.
fn maybe_scatter_oriented(
    m: OrientedMedium,
    lambdas: array<f32, 8>,
    alphas: array<f32, 8>,
    ray_dir: vec3<f32>,
    hit_t: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
) -> OrientedScatterResult {
    // Per absorption-length unit, like `alphas`.
    var sigma_s: array<f32, 8>;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        sigma_s[k] = ori_sigma_s(m, ray_dir, lambdas[k]) / m.path_scale;
    }
    let sigma_t_hero = max(alphas[0] + sigma_s[0], 1e-12);

    let dist_rand = ori_stream_draw(rng_seed, bounce, DISTANCE_SAMPLE_STREAM);
    let one_minus_u = max(1.0 - dist_rand, 1e-7);
    let t_free = -(log(one_minus_u)) / sigma_t_hero;
    let hit_t_scaled = hit_t * m.path_scale;

    var result: OrientedScatterResult;
    result.scattered = 0u;
    result.t_free = 0.0;
    result.new_dir = vec3<f32>(0.0, 0.0, 0.0);

    if (t_free < hit_t_scaled) {
        let pdf_hero = sigma_t_hero * one_minus_u;
        for (var k: u32 = 0u; k < 8u; k = k + 1u) {
            let sigma_t_k = alphas[k] + sigma_s[k];
            let tr_k = exp_poly(-sigma_t_k * t_free);
            let weight = tr_k * sigma_s[k] / pdf_hero;
            let intensity = max((*stokes)[k].x, 0.0) * weight;
            (*stokes)[k] = vec4<f32>(intensity, 0.0, 0.0, 0.0);
            (*path_pdf)[k] = (*path_pdf)[k] * (sigma_t_k * tr_k);
        }
        let u_c = ori_stream_draw(rng_seed, bounce, ORIENTED_COMPONENT_STREAM);
        let u1 = ori_stream_draw(rng_seed, bounce, PHASE_DIR_U_STREAM);
        let u2 = ori_stream_draw(rng_seed, bounce, PHASE_DIR_V_STREAM);
        result.scattered = 1u;
        result.t_free = t_free / m.path_scale;
        result.new_dir = ori_sample_direction(m, ray_dir, lambdas[0], u_c, u1, u2);
        return result;
    }

    let survive_hero = exp_poly(-sigma_t_hero * hit_t_scaled);
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        let sigma_t_k = alphas[k] + sigma_s[k];
        let survive_k = exp_poly(-sigma_t_k * hit_t_scaled);
        (*stokes)[k] = (*stokes)[k] * (survive_k / max(survive_hero, 1e-30));
        (*path_pdf)[k] = (*path_pdf)[k] * survive_k;
    }
    return result;
}

