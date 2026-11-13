// ---------------------------------------------------------------------------------
// optics::dispersion::DispersionModel::evaluate
// ---------------------------------------------------------------------------------

struct DispersionCase {
    model_type: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    param_a: vec4<f32>,
    param_b: vec4<f32>,
    lambda_nm: f32,
    _pad3: f32,
    _pad4: f32,
    _pad5: f32,
}

@group(0) @binding(12) var<storage, read> dispersion_cases: array<DispersionCase>;
@group(0) @binding(13) var<storage, read_write> dispersion_out: array<f32>;

@compute @workgroup_size(64)
fn dispersion_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&dispersion_cases)) {
        return;
    }
    let c = dispersion_cases[idx];
    dispersion_out[idx] = dispersion_evaluate(c.model_type, c.param_a, c.param_b, c.lambda_nm);
}

// ---------------------------------------------------------------------------------
// optics::raytracer::spectral_absorption
// ---------------------------------------------------------------------------------

struct AbsorptionCase {
    band_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    bands: array<AbsorptionBand, 8>,
    lambda_nm: f32,
    _pad3: f32,
    _pad4: f32,
    _pad5: f32,
}

@group(0) @binding(14) var<storage, read> absorption_cases: array<AbsorptionCase>;
@group(0) @binding(15) var<storage, read_write> absorption_out: array<f32>;

@compute @workgroup_size(64)
fn absorption_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&absorption_cases)) {
        return;
    }
    let c = absorption_cases[idx];
    absorption_out[idx] = spectral_absorption(c.bands, c.band_count, c.lambda_nm);
}

// ---------------------------------------------------------------------------------
// optics::birefringence::pleochroic_channel_alpha (end to end: electric_field_direction,
// ordinary/extraordinary eigen polarization inputs supplied directly, AbsorptionTensor3
// quadratic form, effective_pleochroic_alpha combination).
// ---------------------------------------------------------------------------------

struct PleochroicCase {
    alpha_o: f32,
    alpha_e: f32,
    _pad0: f32,
    _pad1: f32,
    c_axis: vec3<f32>,
    _pad2: f32,
    s_axis: vec3<f32>,
    _pad3: f32,
    propagation_dir: vec3<f32>,
    _pad4: f32,
    eigen_a: vec3<f32>,
    _pad5: f32,
    eigen_b: vec3<f32>,
    _pad6: f32,
    stokes: vec4<f32>,
}

@group(0) @binding(16) var<storage, read> pleochroic_cases: array<PleochroicCase>;
@group(0) @binding(17) var<storage, read_write> pleochroic_out: array<f32>;

@compute @workgroup_size(64)
fn pleochroic_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&pleochroic_cases)) {
        return;
    }
    let cs = pleochroic_cases[idx];
    pleochroic_out[idx] = pleochroic_channel_alpha(
        cs.alpha_o, cs.alpha_e, cs.c_axis, cs.s_axis, cs.propagation_dir, cs.eigen_a, cs.eigen_b, cs.stokes,
    );
}

// Isotropic-material absorption: the midpoint of the two eigenmode quadratic forms
// (optics::raytracer::absorption::channel_absorption_alphas_assigned's isotropic branch).
// Shares `PleochroicCase` and its bindings; the Stokes inputs are ignored.
@compute @workgroup_size(64)
fn isotropic_alpha_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&pleochroic_cases)) {
        return;
    }
    let cs = pleochroic_cases[idx];
    pleochroic_out[idx] = isotropic_channel_alpha(
        cs.alpha_o, cs.alpha_e, cs.c_axis, cs.eigen_a, cs.eigen_b,
    );
}

// ---------------------------------------------------------------------------------
// optics::birefringence::{BirefringenceParams::ordinary_eigen_polarization,
// BirefringenceParams::extraordinary_eigen_polarization}
// ---------------------------------------------------------------------------------

struct EigenPolarizationCase {
    wave_normal: vec3<f32>,
    _pad0: f32,
    c_axis: vec3<f32>,
    _pad1: f32,
}

@group(0) @binding(18) var<storage, read> eigen_polarization_cases: array<EigenPolarizationCase>;
@group(0) @binding(19) var<storage, read_write> eigen_polarization_out: array<f32>;

@compute @workgroup_size(64)
fn eigen_polarization_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&eigen_polarization_cases)) {
        return;
    }
    let c = eigen_polarization_cases[idx];
    let o_hat = ordinary_eigen_polarization(c.wave_normal, c.c_axis);
    let e_hat = extraordinary_eigen_polarization(c.wave_normal, c.c_axis);
    eigen_polarization_out[idx * 6u + 0u] = o_hat.x;
    eigen_polarization_out[idx * 6u + 1u] = o_hat.y;
    eigen_polarization_out[idx * 6u + 2u] = o_hat.z;
    eigen_polarization_out[idx * 6u + 3u] = e_hat.x;
    eigen_polarization_out[idx * 6u + 4u] = e_hat.y;
    eigen_polarization_out[idx * 6u + 5u] = e_hat.z;
}

// ---------------------------------------------------------------------------------
// optics::raytracer::theta_c_for_bounce (the theta_c fixed-point iteration). `is_biaxial`
// is not passed: GPU materials are never biaxial here (see `transport_physics.wgsl`).
// ---------------------------------------------------------------------------------

struct ThetaCCase {
    normal: vec3<f32>,
    _pad0: f32,
    ray_dir: vec3<f32>,
    _pad1: f32,
    c_axis: vec3<f32>,
    _pad2: f32,
    cos_i: f32,
    inside_gem: u32,
    is_anisotropic: u32,
    n_o_hero_seed: f32,
    birefringence_delta: f32,
    _pad3: f32,
    _pad4: f32,
    _pad5: f32,
}

@group(0) @binding(20) var<storage, read> theta_c_cases: array<ThetaCCase>;
@group(0) @binding(21) var<storage, read_write> theta_c_out: array<f32>;

@compute @workgroup_size(64)
fn theta_c_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&theta_c_cases)) {
        return;
    }
    let c = theta_c_cases[idx];
    // `theta_c_for_bounce` takes the precomputed hero e-index. This case bank has no
    // independent extraordinary-ray curve, so the constant-offset seed used here matches
    // `GemMaterial::extraordinary_index_at`'s fallback. Curve-carrying cases are covered
    // by the `per_mode_index` and `dispersion` checks.
    let n_e_hero_seed_case = c.n_o_hero_seed + c.birefringence_delta;
    theta_c_out[idx] = theta_c_for_bounce(
        c.normal, c.ray_dir, c.cos_i, c.inside_gem != 0u, c.is_anisotropic != 0u, c.c_axis,
        c.n_o_hero_seed, n_e_hero_seed_case,
    );
}

// ---------------------------------------------------------------------------------
// optics::birefringence::BirefringenceParams::extraordinary_poynting_dir (the
// extraordinary ray's walk-off direction).
// ---------------------------------------------------------------------------------

struct WalkOffCase {
    wave_normal: vec3<f32>,
    _pad0: f32,
    c_axis: vec3<f32>,
    _pad1: f32,
    n_o: f32,
    n_e: f32,
    _pad2: f32,
    _pad3: f32,
}

@group(0) @binding(22) var<storage, read> walk_off_cases: array<WalkOffCase>;
@group(0) @binding(23) var<storage, read_write> walk_off_out: array<f32>;

@compute @workgroup_size(64)
fn walk_off_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&walk_off_cases)) {
        return;
    }
    let c = walk_off_cases[idx];
    let dir = extraordinary_poynting_dir(c.wave_normal, c.c_axis, c.n_o, c.n_e);
    walk_off_out[idx * 3u + 0u] = dir.x;
    walk_off_out[idx * 3u + 1u] = dir.y;
    walk_off_out[idx * 3u + 2u] = dir.z;
}

// ---------------------------------------------------------------------------------
// optics::raytracer::per_channel_uniaxial_indices: one channel's per-mode (n_o, n_eff)
// pair via `per_channel_uniaxial_index`. The per-channel loop stays with the caller.
// ---------------------------------------------------------------------------------

struct PerModeIndexCase {
    model_type: u32,
    is_anisotropic: u32,
    _pad0: u32,
    _pad1: u32,
    param_a: vec4<f32>,
    param_b: vec4<f32>,
    lambda_nm: f32,
    birefringence_delta: f32,
    theta_c: f32,
    _pad2: f32,
}

@group(0) @binding(24) var<storage, read> per_mode_index_cases: array<PerModeIndexCase>;
@group(0) @binding(25) var<storage, read_write> per_mode_index_out: array<f32>;

@compute @workgroup_size(64)
fn per_mode_index_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&per_mode_index_cases)) {
        return;
    }
    let c = per_mode_index_cases[idx];
    let pair = per_channel_uniaxial_index(
        c.model_type, c.param_a, c.param_b, c.lambda_nm, c.birefringence_delta, c.is_anisotropic != 0u, c.theta_c,
    );
    per_mode_index_out[idx * 2u + 0u] = pair.x;
    per_mode_index_out[idx * 2u + 1u] = pair.y;
}

