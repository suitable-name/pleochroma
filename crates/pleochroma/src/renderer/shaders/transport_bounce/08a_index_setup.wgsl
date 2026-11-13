// ---------------------------------------------------------------------------------
// Per-bounce index setup. The extend, scatter, surface, arm, split and tail phases are in the
// neighbouring 08 files, and `transport_bounce_step` (08f) composes them.
// ---------------------------------------------------------------------------------

struct IndexSetup {
    cos_i: f32,
    sin_i: f32,
    theta_c: f32,
    n_eff_ch: array<f32, 8>,
    n_e_raw_ch: array<f32, 8>,
    n_o_hero: f32,
    n_e_hero: f32,
    uframe: UniaxialFrameW,
    uinc_frame: EntryIncidenceFrameW,
    uniaxial_active: bool,
    uniaxial_nondegenerate: bool,
    wave_dir_a_hero: vec3<f32>,
    wave_dir_b_hero: vec3<f32>,
    n_biax_a_ch: array<f32, 8>,
    n_biax_b_ch: array<f32, 8>,
    n_alpha_ch: array<f32, 8>,
    n_beta_ch: array<f32, 8>,
    n_gamma_ch: array<f32, 8>,
    n_biax_a_hero: f32,
    n_biax_b_hero: f32,
    use_mode_a_medium: bool,
    n_medium_ch: array<f32, 8>,
    n_medium_hero: f32,
    n1: f32,
    n2: f32,
    eta: f32,
    sin2_t: f32,
}

// Pure per-bounce optical and index setup. Reads ray and hit state; writes nothing.
fn bounce_index_setup(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
) -> IndexSetup {
    let wave_dir_at_bounce = (*h).wave_dir_at_bounce;
    let normal = (*h).shading_normal;
    let is_anisotropic = rc.is_anisotropic;
    let is_biaxial = rc.is_biaxial;
    let c_axis = rc.c_axis;
    let n_o_hero_seed = rc.n_o_hero_seed;
    let n_e_hero_seed = rc.n_e_hero_seed;
    let n_o_hoisted = rc.n_o_hoisted;
    let birefringence_delta = rc.birefringence_delta;
    let biax_ax0 = rc.biax_ax0;
    let biax_ax1 = rc.biax_ax1;
    let biax_ax2 = rc.biax_ax2;
    let n_alpha_hero = rc.n_alpha_hero;
    let n_beta_hero = rc.n_beta_hero;
    let n_gamma_hero = rc.n_gamma_hero;

    // Incidence angle is measured against the wave normal `k` (`wave_dir_at_bounce`), not the
    // Poynting direction `S`.
    let cos_i = clamp(dot(-wave_dir_at_bounce, normal), 0.0, 1.0);
    let sin_i = sqrt(max(fma(-cos_i, cos_i, 1.0), 0.0));

    // theta_c fixed-point iteration (optics::raytracer::theta_c_for_bounce). Per-channel
    // effective extraordinary indices come from `per_channel_uniaxial_index` in
    // transport_physics.wgsl. An isotropic material gives n_eff_ch[k] == n_o_ch[k]. For a
    // biaxial material this result is unused. Fed `wave_dir_at_bounce` (`k`), not `S`.
    let theta_c = theta_c_for_bounce(normal, wave_dir_at_bounce, cos_i, (*s).inside_gem, is_anisotropic, c_axis, n_o_hero_seed, n_e_hero_seed);

    // `n_o_ch[k]` equals the hoisted `n_o_hoisted[k]`, so only the theta_c-dependent half of
    // `per_channel_uniaxial_index` runs per bounce. It is bit-identical to calling the full
    // function.
    //
    // n_e_k matches `GemMaterial::extraordinary_index_at`: the material's own extraordinary
    // curve when `material.has_extraordinary_dispersion != 0` (Quartz/Amethyst/Citrine),
    // else `n_o_hoisted[k] + birefringence_delta`.
    var n_eff_ch: array<f32, 8>;
    // Raw per-channel extraordinary index, before the theta_c projection. The uniaxial
    // arms read it directly.
    var n_e_raw_ch: array<f32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        n_eff_ch[k] = 0.0;
        n_e_raw_ch[k] = 0.0;
    }
    // sin and cos of theta_c, computed once for the channel loop.
    var sin_theta_c: f32 = 0.0;
    var cos_theta_c: f32 = 1.0;
    if (is_anisotropic) {
        sin_theta_c = sin(theta_c);
        cos_theta_c = cos(theta_c);
    }
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        var n_eff_k = n_o_hoisted[k];
        if (is_anisotropic) {
            // Gated on the anisotropy flag. Every read of `n_e_raw_ch` sits under
            // `uniaxial_nondegenerate`, which implies `is_anisotropic`.
            var n_e_k: f32;
            if (EXTRAORDINARY_DISPERSION && material.has_extraordinary_dispersion != 0u) {
                n_e_k = extraordinary_dispersion_evaluate(material.extraordinary_model_type, material.extraordinary_param_a, material.extraordinary_param_b, (*s).lambdas[k]);
            } else {
                n_e_k = n_o_hoisted[k] + birefringence_delta;
            }
            n_e_raw_ch[k] = n_e_k;
            n_eff_k = effective_extraordinary_index_sc(n_o_hoisted[k], n_e_k, sin_theta_c, cos_theta_c);
        }
        n_eff_ch[k] = n_eff_k;
    }
    let n_o_hero = n_o_hoisted[0];
    // Hero extraordinary index: `n_e_hero_seed` (the material's e-ray curve when it has one,
    // else `n_o + birefringence_delta`). It drives the walk-off direction
    // (`extraordinary_poynting_dir`), so the doubled images separate by color. Mirrors
    // `BounceRefractionGeometry::n_e_hero` on the CPU.
    var n_e_hero = n_o_hero + birefringence_delta;
    if (EXTRAORDINARY_DISPERSION && material.has_extraordinary_dispersion != 0u) {
        n_e_hero = n_e_hero_seed;
    }

    // Local frame and optic-axis direction cosines for the closed-form uniaxial Fresnel solver
    // (Lekner 1991). Built only under `uniaxial_nondegenerate`, since every read sits behind
    // that guard. `entry_incidence_frame` (with `n1 == 1.0` for air to crystal) is built only
    // for rays outside the gem, because only the entry arm uses it.
    var uframe: UniaxialFrameW;
    uframe.that = vec3<f32>(0.0, 0.0, 0.0);
    uframe.zhat = vec3<f32>(0.0, 0.0, 0.0);
    uframe.s_axis = vec3<f32>(0.0, 0.0, 0.0);
    uframe.p_axis = vec3<f32>(0.0, 0.0, 0.0);
    uframe.cos_i = 0.0;
    uframe.sin_i = 0.0;
    uframe.alpha = 0.0;
    uframe.beta = 0.0;
    uframe.gamma = 0.0;

    var uinc_frame: EntryIncidenceFrameW;
    for (var i: u32 = 0u; i < 4u; i = i + 1u) {
        uinc_frame.incident_s[i] = cplx_zero();
        uinc_frame.incident_p[i] = cplx_zero();
        uinc_frame.reflected_s[i] = cplx_zero();
        uinc_frame.reflected_p[i] = cplx_zero();
    }

    // Mirrors `BounceRefractionGeometry::uniaxial_frame`'s `Some` condition
    // exactly -- the plain uniaxial-vs-biaxial-vs-isotropic material classification,
    // with no degenerate-axis check of its own (see `uniaxial_nondegenerate` below
    // for that).
    let uniaxial_active = is_anisotropic && !is_biaxial;
    // Degenerate limit where the wave normal is parallel to the optic axis. Mirrors the
    // cross-product guard in `apply_partial_fresnel_bounce` and `apply_tir_bounce`
    // (`refraction/tir.rs`). Falling back to the scalar-at-`n_o` path is exact there, for
    // every dispatch arm including forced TIR.
    let uniaxial_nondegenerate = uniaxial_active
        && dot(cross(wave_dir_at_bounce, c_axis), cross(wave_dir_at_bounce, c_axis)) > 1e-6;
    if (uniaxial_nondegenerate) {
        uframe = uniaxial_frame_build(wave_dir_at_bounce, normal, c_axis, cos_i, sin_i);
        if (!(*s).inside_gem) {
            uinc_frame = entry_incidence_frame(1.0, uframe);
        }
    }

    // optics::raytracer::{hero_biaxial_wave_dirs, per_channel_biaxial_indices}. Mode A is the
    // faster (lower-index) root, mode B the slower one. The arrays stay zero unless
    // `is_biaxial`, and are read only under that guard.
    var wave_dir_a_hero = vec3<f32>(0.0, 0.0, 0.0);
    var wave_dir_b_hero = vec3<f32>(0.0, 0.0, 0.0);
    var n_biax_a_ch: array<f32, 8>;
    var n_biax_b_ch: array<f32, 8>;
    var n_alpha_ch: array<f32, 8>;
    var n_beta_ch: array<f32, 8>;
    var n_gamma_ch: array<f32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        n_biax_a_ch[k] = 0.0;
        n_biax_b_ch[k] = 0.0;
        n_alpha_ch[k] = 0.0;
        n_beta_ch[k] = 0.0;
        n_gamma_ch[k] = 0.0;
    }
    if (is_biaxial) {
        if ((*s).inside_gem) {
            // Inside the gem both biaxial modes walk off, so the wave direction is `k`, not S.
            wave_dir_a_hero = wave_dir_at_bounce;
            wave_dir_b_hero = wave_dir_at_bounce;
        } else {
            let res_a = biaxial_resolve_entry_mode(n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2, wave_dir_at_bounce, normal, cos_i, n_o_hero_seed, false);
            let res_b = biaxial_resolve_entry_mode(n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2, wave_dir_at_bounce, normal, cos_i, n_o_hero_seed, true);
            wave_dir_a_hero = res_a.wave_dir;
            wave_dir_b_hero = res_b.wave_dir;
        }
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            // Per-channel n_alpha, n_beta and n_gamma, as `GemMaterial::biaxial_indicatrix`
            // gives them. With per-axis curves the sorted middle value can differ from
            // `n_o_hoisted[k]`, so `n_beta_k` is read from the triple.
            let principal_k = biaxial_principal_indices(
                material.biaxial_curves.types.z != 0u,
                material.biaxial_curves.types.x,
                material.biaxial_curves.alpha_a,
                material.biaxial_curves.alpha_b,
                material.biaxial_curves.types.y,
                material.biaxial_curves.gamma_a,
                material.biaxial_curves.gamma_b,
                n_o_hoisted[k],
                material.dispersion.biaxial_delta_beta_alpha,
                birefringence_delta,
                (*s).lambdas[k],
            );
            let n_alpha_k = principal_k.x;
            let n_beta_k = principal_k.y;
            let n_gamma_k = principal_k.z;
            n_alpha_ch[k] = n_alpha_k;
            n_beta_ch[k] = n_beta_k;
            n_gamma_ch[k] = n_gamma_k;
            let ni_a = biaxial_wave_indices(n_alpha_k, n_beta_k, n_gamma_k, biax_ax0, biax_ax1, biax_ax2, wave_dir_a_hero);
            let ni_b = biaxial_wave_indices(n_alpha_k, n_beta_k, n_gamma_k, biax_ax0, biax_ax1, biax_ax2, wave_dir_b_hero);
            n_biax_a_ch[k] = ni_a.y;
            n_biax_b_ch[k] = ni_b.x;
        }
    }
    let n_biax_a_hero = n_biax_a_ch[0];
    let n_biax_b_hero = n_biax_b_ch[0];

    // Inside an anisotropic crystal the medium index is mode A or mode B, set by the eigenmode
    // chosen at the most recent entry (`is_extraordinary`). Otherwise it is the mode-B array,
    // which for a cubic material equals n_o_hoisted. `is_biaxial` selects the biaxial pair
    // computed above, or the uniaxial n_o_hoisted / n_eff_ch pair.
    let use_mode_a_medium = is_anisotropic && (*s).inside_gem && !(*s).is_extraordinary;
    var n_medium_ch: array<f32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        var mode_a_k: f32;
        var mode_b_k: f32;
        if (is_biaxial) {
            mode_a_k = n_biax_a_ch[k];
            mode_b_k = n_biax_b_ch[k];
        } else {
            mode_a_k = n_o_hoisted[k];
            mode_b_k = n_eff_ch[k];
        }
        n_medium_ch[k] = select(mode_b_k, mode_a_k, use_mode_a_medium);
    }
    let n_medium_hero = n_medium_ch[0];
    let n1 = select(1.0, n_medium_hero, (*s).inside_gem);
    let n2 = select(n_medium_hero, 1.0, (*s).inside_gem);
    let eta = n1 / n2;
    let sin2_t = eta * eta * fma(-cos_i, cos_i, 1.0);

    var idx: IndexSetup;
    idx.cos_i = cos_i;
    idx.sin_i = sin_i;
    idx.theta_c = theta_c;
    idx.n_eff_ch = n_eff_ch;
    idx.n_e_raw_ch = n_e_raw_ch;
    idx.n_o_hero = n_o_hero;
    idx.n_e_hero = n_e_hero;
    idx.uframe = uframe;
    idx.uinc_frame = uinc_frame;
    idx.uniaxial_active = uniaxial_active;
    idx.uniaxial_nondegenerate = uniaxial_nondegenerate;
    idx.wave_dir_a_hero = wave_dir_a_hero;
    idx.wave_dir_b_hero = wave_dir_b_hero;
    idx.n_biax_a_ch = n_biax_a_ch;
    idx.n_biax_b_ch = n_biax_b_ch;
    idx.n_alpha_ch = n_alpha_ch;
    idx.n_beta_ch = n_beta_ch;
    idx.n_gamma_ch = n_gamma_ch;
    idx.n_biax_a_hero = n_biax_a_hero;
    idx.n_biax_b_hero = n_biax_b_hero;
    idx.use_mode_a_medium = use_mode_a_medium;
    idx.n_medium_ch = n_medium_ch;
    idx.n_medium_hero = n_medium_hero;
    idx.n1 = n1;
    idx.n2 = n2;
    idx.eta = eta;
    idx.sin2_t = sin2_t;
    return idx;
}

// The index setup an arm works from: the one `bounce_surface` stored in `h` when the megakernel's
// surface phase ran (the same inputs, so the same value), else a fresh `bounce_index_setup` (the
// event wavefront's arm kernels, and a parked bounce resumed by `transport_bounce_back`).
fn bounce_arm_index(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
) -> IndexSetup {
    if ((*h).idx_valid) {
        return (*h).idx;
    }
    return bounce_index_setup(s, h, rc);
}

