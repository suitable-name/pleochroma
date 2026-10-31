// Surface phase of the event wavefront: shading normal, surface finish, plane-of-incidence frame
// rotation, plain interior absorption and the interaction arm selection.
//
// This is `bounce_surface` (08c) split along its natural events for the browser's kernel compile
// time, with every operation unchanged and in the same order:
//
// * The zoned interior absorption (`zoned_interior_absorption`, zone geometry and per-zone alpha
//   evaluation) is NOT here: a zoned stone leaves with `TAG_ZONE_ABSORB` and `ew_zoned_absorb`
//   applies it before the arm kernels run (the absorption only scales `stokes`, which the arm
//   selection below does not read). `with_zoned_absorb` puts that kernel in the dispatch list
//   exactly for the stones with zones; the browser refuses zoned stones, so it never compiles it.
// * The arm selection reads only the hero channel (index 0): `ew_select_arm` evaluates the
//   hero-channel expressions of `bounce_index_setup` and nothing of the seven other channels, the
//   biaxial per-axis tables or the uniaxial entry frames, which only the arm kernels use (and
//   compute again for themselves, as before).
//
// The megakernel and the old wavefront keep calling `bounce_surface` in 08c unchanged.

// The arm this hit takes, from the hero channel alone. Every expression is the hero-channel
// (k = 0) one of `bounce_index_setup` (08a), which stays the source of the full per-channel
// values the arm kernels use; keep the two in step. Reads the hit record `bounce_surface` filled.
fn ew_select_arm(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
) -> u32 {
    let wave_dir_at_bounce = (*h).wave_dir_at_bounce;
    let normal = (*h).shading_normal;
    let is_anisotropic = rc.is_anisotropic;
    let is_biaxial = rc.is_biaxial;
    let c_axis = rc.c_axis;
    let n_o_hero_seed = rc.n_o_hero_seed;
    let n_e_hero_seed = rc.n_e_hero_seed;
    let n_o_hero = rc.n_o_hoisted[0];
    let birefringence_delta = rc.birefringence_delta;
    let biax_ax0 = rc.biax_ax0;
    let biax_ax1 = rc.biax_ax1;
    let biax_ax2 = rc.biax_ax2;
    let n_alpha_hero = rc.n_alpha_hero;
    let n_beta_hero = rc.n_beta_hero;
    let n_gamma_hero = rc.n_gamma_hero;

    let cos_i = clamp(dot(-wave_dir_at_bounce, normal), 0.0, 1.0);

    // The hero's effective index of the uniaxial pair (`n_eff_ch[0]`); the ordinary index for a
    // material that is not anisotropic.
    var n_eff_hero = n_o_hero;
    if (is_anisotropic) {
        let theta_c = theta_c_for_bounce(normal, wave_dir_at_bounce, cos_i, (*s).inside_gem, is_anisotropic, c_axis, n_o_hero_seed, n_e_hero_seed);
        var n_e_hero_raw: f32;
        if (EXTRAORDINARY_DISPERSION && material.has_extraordinary_dispersion != 0u) {
            n_e_hero_raw = extraordinary_dispersion_evaluate(material.extraordinary_model_type, material.extraordinary_param_a, material.extraordinary_param_b, (*s).lambdas[0]);
        } else {
            n_e_hero_raw = n_o_hero + birefringence_delta;
        }
        n_eff_hero = effective_extraordinary_index(n_o_hero, n_e_hero_raw, theta_c);
    }

    // `uniaxial_nondegenerate` of `bounce_index_setup`.
    let uniaxial_active = is_anisotropic && !is_biaxial;
    let uniaxial_nondegenerate = uniaxial_active
        && dot(cross(wave_dir_at_bounce, c_axis), cross(wave_dir_at_bounce, c_axis)) > 1e-6;

    // The hero's two biaxial mode indices (`n_biax_a_ch[0]`, `n_biax_b_ch[0]`); zero otherwise.
    var n_biax_a_hero: f32 = 0.0;
    var n_biax_b_hero: f32 = 0.0;
    if (is_biaxial) {
        var wave_dir_a_hero = vec3<f32>(0.0, 0.0, 0.0);
        var wave_dir_b_hero = vec3<f32>(0.0, 0.0, 0.0);
        if ((*s).inside_gem) {
            wave_dir_a_hero = wave_dir_at_bounce;
            wave_dir_b_hero = wave_dir_at_bounce;
        } else {
            let res_a = biaxial_resolve_entry_mode(n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2, wave_dir_at_bounce, normal, cos_i, n_o_hero_seed, false);
            let res_b = biaxial_resolve_entry_mode(n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2, wave_dir_at_bounce, normal, cos_i, n_o_hero_seed, true);
            wave_dir_a_hero = res_a.wave_dir;
            wave_dir_b_hero = res_b.wave_dir;
        }
        let principal_hero = biaxial_principal_indices(
            material.biaxial_curves.types.z != 0u,
            material.biaxial_curves.types.x,
            material.biaxial_curves.alpha_a,
            material.biaxial_curves.alpha_b,
            material.biaxial_curves.types.y,
            material.biaxial_curves.gamma_a,
            material.biaxial_curves.gamma_b,
            n_o_hero,
            material.dispersion.biaxial_delta_beta_alpha,
            birefringence_delta,
            (*s).lambdas[0],
        );
        let ni_a = biaxial_wave_indices(principal_hero.x, principal_hero.y, principal_hero.z, biax_ax0, biax_ax1, biax_ax2, wave_dir_a_hero);
        let ni_b = biaxial_wave_indices(principal_hero.x, principal_hero.y, principal_hero.z, biax_ax0, biax_ax1, biax_ax2, wave_dir_b_hero);
        n_biax_a_hero = ni_a.y;
        n_biax_b_hero = ni_b.x;
    }

    // The medium index this ray is currently in (`n_medium_hero`), then `sin2_t` of the hero.
    let use_mode_a_medium = is_anisotropic && (*s).inside_gem && !(*s).is_extraordinary;
    var mode_a_hero: f32;
    var mode_b_hero: f32;
    if (is_biaxial) {
        mode_a_hero = n_biax_a_hero;
        mode_b_hero = n_biax_b_hero;
    } else {
        mode_a_hero = n_o_hero;
        mode_b_hero = n_eff_hero;
    }
    let n_medium_hero = select(mode_b_hero, mode_a_hero, use_mode_a_medium);
    let n1 = select(1.0, n_medium_hero, (*s).inside_gem);
    let n2 = select(n_medium_hero, 1.0, (*s).inside_gem);
    let eta = n1 / n2;
    let sin2_t = eta * eta * fma(-cos_i, cos_i, 1.0);

    if (surf_is_ground((*h).finish)) {
        return ARM_FROSTED;
    } else if (sin2_t > 1.0) {
        return ARM_TIR;
    } else if (uniaxial_nondegenerate) {
        if (!(*s).inside_gem) {
            return ARM_UNI_ENTRY;
        }
        return ARM_UNI_INTERNAL;
    }
    return ARM_GENERIC;
}

// `bounce_surface` without the zoned interior absorption (returned as `true` when the stone needs
// it) and with the hero-only arm selection. Fills the hit record and `h.arm`.
fn ew_surface_event(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
) -> bool {
    let hit = (*h).hit;
    var local_stokes = (*s).stokes;
    let is_anisotropic = rc.is_anisotropic;
    let is_biaxial = rc.is_biaxial;
    let c_axis = rc.c_axis;
    let n_alpha_hero = rc.n_alpha_hero;
    let n_beta_hero = rc.n_beta_hero;
    let n_gamma_hero = rc.n_gamma_hero;
    let biax_ax0 = rc.biax_ax0;
    let biax_ax1 = rc.biax_ax1;
    let biax_ax2 = rc.biax_ax2;
    let n_o_hero_seed = rc.n_o_hero_seed;
    let n_e_hero_seed = rc.n_e_hero_seed;
    let alpha_o_hoisted = rc.alpha_o_hoisted;
    let alpha_e_hoisted = rc.alpha_e_hoisted;
    let alpha_beta_hoisted = rc.alpha_beta_hoisted;

    let hit_point = (*s).origin + hit.t * (*s).dir;
    // See shading_normal_near_edge's own doc comment. `shading_normal_for_hit` is that
    // function unchanged for a planar stone and the concave-stone rule otherwise.
    var normal = shading_normal_for_hit(hit_point, hit.facet_idx, hit.normal, material.edge_rounding_radius);
    // Surface finish (see 08c's header): the facet word, and the micro-normal of a polish's halo
    // draw in place of the macroscopic normal, resolved before the plane-of-incidence frame.
    var finish: u32 = 0u;
    if (hit.facet_idx < arrayLength(&facet_finishes)) {
        finish = facet_finishes[hit.facet_idx];
    }
    let begun = surface_begin(
        finish, hit_point, hit.facet_idx, normal, (*s).k, (*s).inside_gem,
        (*s).lambdas[0], rc.n_o_hoisted[0], seed0, bounce,
    );
    // `hit.normal` (the unrounded plane normal) is dead after this phase; it carries the
    // macroscopic outward shading normal on to `surface_finish_event`.
    (*h).hit.normal = normal;
    normal = begun.normal;
    finish = begun.finish;
    // The wave normal `k` serves every index lookup, cos_i/sin_i, Snell/Fresnel evaluation, TIR
    // decision and the Stokes frame below (see `bounce_surface`).
    let wave_dir_at_bounce = (*s).k;

    // Plane-of-incidence frame rotation (signed psi via atan2).
    let cpn_raw = cross(wave_dir_at_bounce, normal);
    let cpn_len2 = dot(cpn_raw, cpn_raw);
    var current_plane_normal: vec3<f32>;
    if (cpn_len2 > 0.0) {
        current_plane_normal = cpn_raw / sqrt(cpn_len2);
    } else {
        current_plane_normal = vec3<f32>(0.0, 0.0, 0.0);
    }
    if ((*s).have_prev_plane_normal && cpn_len2 > 1e-6 && dot((*s).prev_plane_normal, (*s).prev_plane_normal) > 1e-6) {
        let psi = signed_frame_rotation_psi((*s).prev_plane_normal, current_plane_normal, wave_dir_at_bounce);
        let rot = mueller_frame_rotation(psi);
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            local_stokes[k] = rot * local_stokes[k];
            (*s).stokes[k] = local_stokes[k];
        }
    }
    (*s).prev_plane_normal = current_plane_normal;
    (*s).have_prev_plane_normal = true;

    var zone_absorb_pending = false;
    if ((*s).inside_gem) {
        normal = -normal;
        // A scattering-active material's extinction for this segment was already applied by the
        // scatter phase; `!material_scatters()` is the pre-scattering case where this is the ONLY
        // absorption application (see `bounce_surface`).
        if (!material_scatters()) {
            if (material.zones.header.zone_count > 0u) {
                // Zoned stone: `ew_zoned_absorb` applies the per-zone absorption of this segment.
                zone_absorb_pending = true;
            } else {
                // P1 (assigned-mode absorption): optics::raytracer::absorption::
                // channel_absorption_alphas_assigned, evaluated at the current wave normal.
                var alphas_interior: array<f32, 8>;
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    alphas_interior[k] = 0.0;
                }
                if (is_anisotropic) {
                    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                        if (is_biaxial && material.has_beta_ray != 0u) {
                            alphas_interior[k] = assigned_mode_alpha_biaxial(
                                alpha_o_hoisted[k], alpha_beta_hoisted[k], alpha_e_hoisted[k],
                                n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2,
                                c_axis, wave_dir_at_bounce, (*s).is_extraordinary,
                            );
                        } else {
                            alphas_interior[k] = assigned_mode_alpha_uniaxial(
                                alpha_o_hoisted[k], alpha_e_hoisted[k], c_axis, wave_dir_at_bounce, (*s).is_extraordinary,
                                n_o_hero_seed, n_e_hero_seed,
                            );
                        }
                    }
                } else {
                    let eigen_a = ordinary_eigen_polarization(wave_dir_at_bounce, c_axis);
                    let eigen_b = extraordinary_eigen_polarization(wave_dir_at_bounce, c_axis);
                    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                        alphas_interior[k] = isotropic_channel_alpha(
                            alpha_o_hoisted[k], alpha_e_hoisted[k], c_axis, eigen_a, eigen_b,
                        );
                    }
                }
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    // (absorption path scale): mirrors apply_absorption's own
                    // `path_len * ctx.material.absorption_path_scale` multiply exactly.
                    let scaled_hit_t = hit.t * material.absorption_path_scale;
                    // `exp_poly`, not the `exp()` builtin -- `apply_absorption` calls
                    // `crate::simd::exp_f32x8`, not `f32::exp`.
                    let trans_factor = exp_poly(-alphas_interior[k] * scaled_hit_t);
                    local_stokes[k] = local_stokes[k] * trans_factor;
                }
            }
        }
        (*s).stokes = local_stokes;
    }

    // `finish` was looked up, and resolved by `surface_begin`, above. The arm kernels read the
    // pre-bounce `inside_gem` from the hit record.
    (*h).hit_point = hit_point;
    (*h).shading_normal = normal;
    (*h).wave_dir_at_bounce = wave_dir_at_bounce;
    (*h).finish = finish;
    (*h).pre_bounce_inside_gem = (*s).inside_gem;

    (*h).arm = ew_select_arm(s, h, rc);
    return zone_absorb_pending;
}

@compute @workgroup_size(64)
fn ew_surface(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    ew_fill_planes_shared(local_index);
    let i = gid.x;
    if (i >= ew.chunk_rays) {
        return;
    }
    let core = ray_core[i];
    if (core.ray_meta.z != TAG_SURFACE) {
        return;
    }

    var s = ew_load(i);
    var h = ew_hit_load(i);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let seed0 = core.ray_meta.y;

    let zone_absorb_pending = ew_surface_event(&s, &h, rc, ew.bounce, seed0);
    ew_store(i, s);
    ew_hit_store(i, h);

    ray_core[i].ray_meta.z = select(TAG_ARM_BASE + h.arm, TAG_ZONE_ABSORB, zone_absorb_pending);
}
