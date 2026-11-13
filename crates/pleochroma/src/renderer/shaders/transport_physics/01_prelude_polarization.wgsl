// Shared physics prelude: the single definition of every ported function consumed by
// `spectral_transport.wgsl` (the megakernel) and `transport_functions.wgsl` (the standalone
// kernels that `renderer::gpu::transport_check` tests one function at a time).
//
// # Why this file exists
//
// WGSL has no `#include`. Two copies of the same physics can drift apart, and a
// function-level check then tests a copy that never ships. One definition keeps the checks
// on the shipped code.
//
// # How it's wired in
//
// `build.rs` concatenates the pieces of `transport_physics/` (01 to 13, in
// `TRANSPORT_PHYSICS_PIECES` order, no separator) ahead of each transport unit, into
// `$OUT_DIR/*.generated.wgsl`. `renderer::gpu::estimator_check` and
// `renderer::gpu::transport_check` `include_str!` those generated files, not the sources.
// No unit is valid WGSL alone: each assumes every symbol defined here is in scope.
//
// `build.rs`'s `PLEOCHROMA_BUILD_ID` hash walks `src/**/*.wgsl` on disk, this file included,
// not the generated output, so it covers the physics text that ships.
//
// # Rules for editing this file
//
// Only functions and types shared verbatim by the megakernel and the standalone kernels
// belong here. No bindings (`@group`/`@binding`) and no entry points (`@compute`): those
// differ per file. A function that only one file needs stays local to it.
//
// Exit-event spectral splitting: `compute_channel_transmission` and
// `compute_uniaxial_exit_transmission` are defined in `08_exit_splitting.wgsl`. The
// `renderer::gpu::transport_check::p6_exit_splitting` check covers them.

const PI: f32 = 3.14159265358979323846;

// ---------------------------------------------------------------------------------
// optics::raytracer::hash_u32, the per-bounce stream salts
// (FRESNEL_BRANCH_STREAM/RUSSIAN_ROULETTE_STREAM/BIREFRINGENT_SPLIT_STREAM/
// MODE_COUPLING_STREAM/FROSTED_DIR_U_STREAM/FROSTED_DIR_V_STREAM) and the frosted
// r_unpol clamp bounds (R_UNPOL_MIN/R_UNPOL_MAX).
//
// Defined here because `apply_frosted_bounce` below needs them in every unit that
// concatenates this file. WGSL rejects a duplicate top-level identifier, so each constant
// has exactly one definition.
const FRESNEL_BRANCH_STREAM: u32 = 0x9e3779b1u;
const RUSSIAN_ROULETTE_STREAM: u32 = 0x517cc1b7u;
const BIREFRINGENT_SPLIT_STREAM: u32 = 0x2545f491u;
const MODE_COUPLING_STREAM: u32 = 0xcc9e2d51u;
// Frosted girdle finish: the 2D cosine-weighted-hemisphere direction draw at a frosted
// bounce -- two independent streams for (u, v), mirroring
// optics::raytracer::{FROSTED_DIR_U_STREAM, FROSTED_DIR_V_STREAM}.
const FROSTED_DIR_U_STREAM: u32 = 0x27d4eb2fu;
const FROSTED_DIR_V_STREAM: u32 = 0x165667b1u;
// Next-event estimation environment direction draws.
// Mirrors optics::raytracer::sampling::{NEE_ENV_DIR_U_STREAM, NEE_ENV_DIR_V_STREAM,
// FROSTED_NEE_ENV_DIR_U_STREAM, FROSTED_NEE_ENV_DIR_V_STREAM}.
const NEE_ENV_DIR_U_STREAM: u32 = 0x4b725c19u;
const NEE_ENV_DIR_V_STREAM: u32 = 0x2f1e8a4du;
const FROSTED_NEE_ENV_DIR_U_STREAM: u32 = 0x6a09e667u;
const FROSTED_NEE_ENV_DIR_V_STREAM: u32 = 0xbb67ae85u;

const R_UNPOL_MIN: f32 = 1e-4;
const R_UNPOL_MAX: f32 = 1.0 - 1e-4;
const RAY_EPS: f32 = 1e-4;

fn hash_u32(x_in: u32) -> u32 {
    var x = x_in;
    x = x * 0x85ebca6bu;
    x = x ^ (x >> 13u);
    x = x * 0xc2b2ae35u;
    x = x ^ (x >> 16u);
    return x;
}

// optics::raytracer::sampling::discrete_draw: the raw u32 draw of a polished-path stream at
// `bounce`, the salted hash.
fn discrete_draw(rng_seed: u32, bounce: u32, stream: u32) -> u32 {
    return hash_u32(rng_seed ^ hash_u32(bounce ^ stream));
}

// ---------------------------------------------------------------------------------
// Exit collection: the twin of optics::raytracer::Estimator::ExitCollection (CPU:
// `refraction::dispatch::apply_collect_bounce`). At a partial Fresnel event no branch is drawn.
// The arm stages the leaf (an exit's transmitted light, an entry's mirror reflection) in the
// bounce's split slots with its spectral-MIS weight folded in. The split resolution adds the
// leaf, unscaled, to the ray's `nee_xyz`, and the path continues undivided along the other branch.
//
// `collect_on` is set once per invocation by `collect_bind`. It is off by default, where every
// arm is the branching estimator (self-tests and function kernels). The host turns it on only
// for a convex stone over an ideal surface without next-event estimation, e.g. the uniform
// furnace. Then `nee_xyz` holds nothing else, so a leaf needs no re-entry probe.
// ---------------------------------------------------------------------------------
// Bit of `GpuTransportParams::write_debug_buffers` that turns exit collection on
// (`renderer::buffers::GpuTransportParams::COLLECT_BIT`).
const PARAMS_COLLECT_BIT: u32 = 4u;
// optics::raytracer::transport::bounce::COLLECT_ROULETTE_FLOOR.
const COLLECT_ROULETTE_FLOOR: f32 = 0.02;

var<private> collect_on: bool = false;

// Binds the estimator of this invocation. `flags` is `params.write_debug_buffers`.
fn collect_bind(flags: u32) {
    collect_on = (flags & PARAMS_COLLECT_BIT) != 0u;
}

// ---------------------------------------------------------------------------------
// optics::polarization -- Stokes/Mueller matrix constructors. Matrices are applied with the
// WGSL `mat4x4<f32> * vec4<f32>` operator, so no helper is needed for that.
// ---------------------------------------------------------------------------------

// WGSL's `mat4x4<f32>(col0, col1, col2, col3)` is column-major. Column 1 holds `-s2` and
// column 2 holds `s2`, which builds the textbook R(psi). A row-major fill of the CPU's array
// gives R(-psi).
fn mueller_frame_rotation(psi: f32) -> mat4x4<f32> {
    let c2 = cos(2.0 * psi);
    let s2 = sin(2.0 * psi);
    return mat4x4<f32>(
        vec4<f32>(1.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, c2, -s2, 0.0),
        vec4<f32>(0.0, s2, c2, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 1.0),
    );
}

fn mueller_fresnel_reflection(r_s: f32, r_p: f32) -> mat4x4<f32> {
    let rs2 = r_s * r_s;
    let rp2 = r_p * r_p;
    let a = 0.5 * (rs2 + rp2);
    let b = 0.5 * (rs2 - rp2);
    let c = r_s * r_p;
    return mat4x4<f32>(
        vec4<f32>(a, b, 0.0, 0.0),
        vec4<f32>(b, a, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, c, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, c),
    );
}

fn mueller_fresnel_transmission(n1: f32, n2: f32, cos_i: f32, cos_t: f32, t_s: f32, t_p: f32) -> mat4x4<f32> {
    let factor = (n2 * cos_t) / max(n1 * cos_i, 1e-6);
    let ts2 = t_s * t_s * factor;
    let tp2 = t_p * t_p * factor;
    let a = 0.5 * (ts2 + tp2);
    let b = 0.5 * (ts2 - tp2);
    let c = t_s * t_p * factor;
    return mat4x4<f32>(
        vec4<f32>(a, b, 0.0, 0.0),
        vec4<f32>(b, a, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, c, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, c),
    );
}

fn mueller_tir_retardation(delta: f32) -> mat4x4<f32> {
    let cos_d = cos(delta);
    let sin_d = sin(delta);
    return mat4x4<f32>(
        vec4<f32>(1.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 1.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, cos_d, -sin_d),
        vec4<f32>(0.0, 0.0, sin_d, cos_d),
    );
}

// optics::raytracer::tir_phase_delta -- TIR phase retardation delta = delta_p - delta_s.
fn tir_phase_delta(n1k: f32, cos_i: f32, sin_i: f32) -> f32 {
    let a = n1k * n1k * sin_i;
    let inner = max(fma(a, sin_i, -1.0), 0.0);
    let tan_half_delta_k = (cos_i * sqrt(inner)) / max(n1k * sin_i * sin_i, 1e-6);
    return 2.0 * atan(tan_half_delta_k);
}

fn normalize_or_zero(v: vec3<f32>) -> vec3<f32> {
    let l2 = dot(v, v);
    if (l2 > 1e-30) {
        return v / sqrt(l2);
    }
    return vec3<f32>(0.0, 0.0, 0.0);
}

// optics::raytracer::absorption::signed_frame_rotation_psi: signed plane-of-incidence
// rotation angle between consecutive bounces, via atan2.
fn signed_frame_rotation_psi(prev: vec3<f32>, curr: vec3<f32>, axis: vec3<f32>) -> f32 {
    let cos_psi = clamp(dot(prev, curr), -1.0, 1.0);
    let sin_psi = dot(cross(prev, curr), normalize_or_zero(axis));
    return atan2(sin_psi, cos_psi);
}

fn degree_of_polarization(s: vec4<f32>) -> f32 {
    if (s.x <= 1e-7) {
        return 0.0;
    }
    let mag = sqrt(fma(s.w, s.w, fma(s.z, s.z, s.y * s.y)));
    return clamp(mag / s.x, 0.0, 1.0);
}

fn polarization_azimuth(s: vec4<f32>) -> f32 {
    return 0.5 * atan2(s.z, s.y);
}

fn arbitrary_perpendicular(n: vec3<f32>) -> vec3<f32> {
    var a: vec3<f32>;
    if (abs(n.x) > 0.9) {
        a = vec3<f32>(0.0, 1.0, 0.0);
    } else {
        a = vec3<f32>(1.0, 0.0, 0.0);
    }
    return normalize_or_zero(a - n * dot(n, a));
}

// optics::polarization::electric_field_direction
fn electric_field_direction(s: vec4<f32>, s_axis: vec3<f32>, propagation_dir: vec3<f32>) -> vec3<f32> {
    let k_hat = normalize_or_zero(propagation_dir);
    let s_raw = s_axis - k_hat * dot(k_hat, s_axis);
    var s_hat: vec3<f32>;
    if (dot(s_raw, s_raw) > 1e-8) {
        s_hat = normalize(s_raw);
    } else {
        s_hat = arbitrary_perpendicular(k_hat);
    }
    let p_hat = cross(k_hat, s_hat);
    let psi = polarization_azimuth(s);
    let e = cos(psi) * s_hat + sin(psi) * p_hat;
    if (dot(e, e) > 1e-8) {
        return normalize(e);
    }
    return s_hat;
}

// ---------------------------------------------------------------------------------
// optics::birefringence -- the uniaxial pleochroic absorption path (exercised even
// for an isotropic material -- see `spectral_transport.wgsl`'s header comment).
// ---------------------------------------------------------------------------------

// The same construction as `arbitrary_perpendicular` (CPU `stable_orthonormal_basis`). Kept as
// its own function to mirror the CPU name at each call site.
fn stable_orthonormal_basis_t(n: vec3<f32>) -> vec3<f32> {
    var a: vec3<f32>;
    if (abs(n.x) > 0.9) {
        a = vec3<f32>(0.0, 1.0, 0.0);
    } else {
        a = vec3<f32>(1.0, 0.0, 0.0);
    }
    return normalize_or_zero(a - n * dot(n, a));
}

fn ordinary_eigen_polarization(wave_normal: vec3<f32>, c_axis: vec3<f32>) -> vec3<f32> {
    let crs = cross(wave_normal, c_axis);
    if (dot(crs, crs) > 1e-8) {
        return normalize(crs);
    }
    return stable_orthonormal_basis_t(normalize_or_zero(wave_normal));
}

fn extraordinary_eigen_polarization(wave_normal: vec3<f32>, c_axis: vec3<f32>) -> vec3<f32> {
    let o_hat = ordinary_eigen_polarization(wave_normal, c_axis);
    return normalize_or_zero(cross(wave_normal, o_hat));
}

fn quadratic_form(alpha_o: f32, alpha_e: f32, a1: vec3<f32>, a2: vec3<f32>, c: vec3<f32>, e_hat: vec3<f32>) -> f32 {
    let l0 = dot(a1, e_hat);
    let l1 = dot(a2, e_hat);
    let l2 = dot(c, e_hat);
    return alpha_o * (l0 * l0) + alpha_o * (l1 * l1) + alpha_e * (l2 * l2);
}

fn pleochroic_channel_alpha(
    alpha_o: f32,
    alpha_e: f32,
    c_axis: vec3<f32>,
    s_axis: vec3<f32>,
    propagation_dir: vec3<f32>,
    eigen_a: vec3<f32>,
    eigen_b: vec3<f32>,
    s: vec4<f32>,
) -> f32 {
    let c = normalize_or_zero(c_axis);
    let a1 = stable_orthonormal_basis_t(c);
    let a2 = cross(c, a1);
    let e_hat = electric_field_direction(s, s_axis, propagation_dir);
    let alpha_polarized = quadratic_form(alpha_o, alpha_e, a1, a2, c, e_hat);
    let alpha_unpolarized = 0.5 * (quadratic_form(alpha_o, alpha_e, a1, a2, c, eigen_a) + quadratic_form(alpha_o, alpha_e, a1, a2, c, eigen_b));
    let p = clamp(degree_of_polarization(s), 0.0, 1.0);
    return fma(p, alpha_polarized - alpha_unpolarized, alpha_unpolarized);
}

// optics::raytracer::absorption::channel_absorption_alphas_assigned's isotropic branch:
// the plain midpoint of the two eigenmode quadratic forms, with no dependence on the
// Stokes state (the unpolarized limit of `pleochroic_channel_alpha`).
fn isotropic_channel_alpha(
    alpha_o: f32,
    alpha_e: f32,
    c_axis: vec3<f32>,
    eigen_a: vec3<f32>,
    eigen_b: vec3<f32>,
) -> f32 {
    let c = normalize_or_zero(c_axis);
    let a1 = stable_orthonormal_basis_t(c);
    let a2 = cross(c, a1);
    return 0.5 * (quadratic_form(alpha_o, alpha_e, a1, a2, c, eigen_a) + quadratic_form(alpha_o, alpha_e, a1, a2, c, eigen_b));
}

// ---------------------------------------------------------------------------------
// optics::birefringence::{effective_extraordinary_index, walk_off_angle,
// extraordinary_poynting_dir} and optics::raytracer::{theta_c_for_bounce,
// per_channel_uniaxial_indices}. `per_channel_uniaxial_index` is one channel of the CPU's
// per-channel loop; WGSL callers loop over the channels themselves.
//
// `theta_c_for_bounce` omits the CPU's `is_biaxial` parameter and always takes the
// `!inside_gem && is_anisotropic` branch. For a biaxial material its result, and that of
// `per_channel_uniaxial_index`, is never read: `transport_main` takes the medium index from
// the biaxial mode arrays instead. The value stays finite, so the unused result cannot carry
// a NaN.
// ---------------------------------------------------------------------------------

fn effective_extraordinary_index(n_o: f32, n_e: f32, theta: f32) -> f32 {
    if (abs(n_o - n_e) < 1e-5) {
        return n_o;
    }
    return effective_extraordinary_index_sc(n_o, n_e, sin(theta), cos(theta));
}

// `effective_extraordinary_index` from the sine and cosine of theta directly, so a caller that
// evaluates several channels at one theta computes the trigonometry once.
// `BirefringenceParams::effective_extraordinary_index_sc` is the CPU twin.
fn effective_extraordinary_index_sc(n_o: f32, n_e: f32, sin_t: f32, cos_t: f32) -> f32 {
    if (abs(n_o - n_e) < 1e-5) {
        return n_o;
    }
    let ne_cos = n_e * cos_t;
    let no_sin = n_o * sin_t;
    let denom_sq = fma(ne_cos, ne_cos, no_sin * no_sin);
    if (denom_sq <= 1e-8) {
        return n_o;
    }
    return (n_o * n_e) / sqrt(denom_sq);
}

// `BirefringenceParams::walk_off_from_sin_cos`: the walk-off from sin/cos of theta
// directly, so callers that already hold cos(theta) skip the lossy acos -> sin/cos trip.
fn walk_off_from_sin_cos(n_o: f32, n_e: f32, sin_t: f32, cos_t: f32) -> f32 {
    if (abs(n_o - n_e) < 1e-5) {
        return 0.0;
    }
    let n_o2 = n_o * n_o;
    let n_e2 = n_e * n_e;
    let numer = (n_o2 - n_e2) * sin_t * cos_t;
    let denom = max(fma(n_e2 * cos_t, cos_t, n_o2 * sin_t * sin_t), 1e-6);
    let tan_rho = numer / denom;
    return atan(tan_rho);
}

fn walk_off_angle(n_o: f32, n_e: f32, theta: f32) -> f32 {
    return walk_off_from_sin_cos(n_o, n_e, sin(theta), cos(theta));
}

// The optic axis is a director (S(c) == S(-c)), so `c_axis` is folded onto the wave normal's
// hemisphere via `sign` and the tilt is negated on that branch. `theta`/`delta` use the
// unsigned `|cos_theta|` and do not depend on the branch; `c_proj` is built from the signed
// `cos_theta`.
fn extraordinary_poynting_dir(wave_normal: vec3<f32>, c_axis: vec3<f32>, n_o: f32, n_e: f32) -> vec3<f32> {
    let cos_theta = clamp(dot(wave_normal, c_axis), -1.0, 1.0);
    let abs_cos = abs(cos_theta);
    // sin(theta) from the cosine directly (no acos round trip), factored to avoid
    // cancellation near 1.
    let sin_t = sqrt(max((1.0 - abs_cos) * (1.0 + abs_cos), 0.0));
    let delta = walk_off_from_sin_cos(n_o, n_e, sin_t, abs_cos);

    if (abs(delta) < 1e-5) {
        return wave_normal;
    }

    let c_proj = normalize_or_zero(c_axis - cos_theta * wave_normal);
    if (dot(c_proj, c_proj) < 1e-6) {
        return wave_normal;
    }
    var sign: f32 = 1.0;
    if (cos_theta < 0.0) {
        sign = -1.0;
    }

    return normalize(wave_normal * cos(delta) - sign * c_proj * sin(delta));
}

// optics::raytracer::theta_c_for_bounce. `is_biaxial` is omitted; see the section header above.
//
// Takes the hero e-index `n_e_hero_seed`, computed once per ray by the caller: via
// `extraordinary_dispersion_evaluate` when the material has an independent e-ray curve, else
// `n_o_hero_seed + birefringence_delta` (as `GemMaterial::extraordinary_index_at`).
fn theta_c_for_bounce(
    normal: vec3<f32>,
    ray_dir: vec3<f32>,
    cos_i: f32,
    inside_gem: bool,
    is_anisotropic: bool,
    c_axis: vec3<f32>,
    n_o_hero_seed: f32,
    n_e_hero_seed: f32,
) -> f32 {
    if (!inside_gem && is_anisotropic) {
        var n_guess = n_o_hero_seed;
        var theta: f32 = 0.0;
        for (var i: u32 = 0u; i < 2u; i = i + 1u) {
            let eta_guess = 1.0 / n_guess;
            let sin2_t_guess = eta_guess * eta_guess * fma(-cos_i, cos_i, 1.0);
            if (sin2_t_guess > 1.0) {
                break;
            }
            let cos_t_guess = sqrt(max(1.0 - sin2_t_guess, 0.0));
            let wave_dir_guess = normalize(eta_guess * ray_dir + fma(eta_guess, cos_i, -cos_t_guess) * normal);
            let cos_theta_wave = abs(clamp(dot(wave_dir_guess, c_axis), -1.0, 1.0));
            theta = atan2(sqrt(max((1.0 - cos_theta_wave) * (1.0 + cos_theta_wave), 0.0)), cos_theta_wave);
            n_guess = effective_extraordinary_index(n_o_hero_seed, n_e_hero_seed, theta);
        }
        return theta;
    } else {
        let cos_ray = abs(clamp(dot(ray_dir, c_axis), -1.0, 1.0));
        return atan2(sqrt(max((1.0 - cos_ray) * (1.0 + cos_ray), 0.0)), cos_ray);
    }
}
