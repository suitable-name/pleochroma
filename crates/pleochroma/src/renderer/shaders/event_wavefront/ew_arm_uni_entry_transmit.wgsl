// Uniaxial entry arm, transmit half (see `ew_arm_uni_entry.wgsl` for the split). It takes the rays
// the reflect half parked (`TAG_UNI_ENTRY_TRANSMIT`), repeats the hero's entry solve (a pure
// function of the stored ray, so the same numbers) and runs the transmit branch of
// `bounce_arm_uni_entry_core` followed by the surface-finish correction.
//
// The per-channel entry pair is solved once per channel for both of the original's uses (the
// direction-mismatch bookkeeping and the matched transmission); it is a pure function of the
// channel's indices, so hoisting it above the match test changes no value, and the kernel keeps
// two solver call sites (hero + loop) instead of three.

// The transmit branch of `bounce_arm_uni_entry_core`.
fn ew_uni_entry_transmit(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
) {
    let idx = bounce_index_setup(s, h, rc);
    let hit_point = (*h).hit_point;
    let normal = (*h).shading_normal;
    let wave_dir_at_bounce = (*h).wave_dir_at_bounce;
    let c_axis = rc.c_axis;
    let n_o_hoisted = rc.n_o_hoisted;
    let cos_i = idx.cos_i;
    let uframe = idx.uframe;
    let uinc_frame = idx.uinc_frame;
    let n_o_hero = idx.n_o_hero;
    let n_e_hero = idx.n_e_hero;
    let n_e_hero_solve = idx.n_e_raw_ch[0];
    let n_e_raw_ch = idx.n_e_raw_ch;
    let n_eff_ch = idx.n_eff_ch;
    let n1 = idx.n1;
    let n2 = idx.n2;

    // ---- ENTRY: mirrors apply_uniaxial_entry_bounce ----
    let pair_hero = entry_solve_pair_with_incidence(uinc_frame, 1.0, n_o_hero, n_e_hero_solve, c_axis, uframe);
    let sol_s_hero = pair_hero.s_sol;
    let sol_p_hero = pair_hero.p_sol;
    let reflect_mueller_hero = jones_to_mueller(sol_s_hero.r_s, sol_p_hero.r_s, sol_s_hero.r_p, sol_p_hero.r_p);
    let entry_r_branch = clamp(reflect_mueller_hero[0][0], R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
    let entry_inc_flux = 0.5 * uframe.cos_i;
    let p_o_raw = mode_power(sol_s_hero.t_o, sol_p_hero.t_o, sol_s_hero.flux_o, entry_inc_flux, (*s).stokes[0]);
    let p_e_raw = mode_power(sol_s_hero.t_e, sol_p_hero.t_e, sol_s_hero.flux_e, entry_inc_flux, (*s).stokes[0]);
    let p_o_hero = clamp(p_o_raw / max(p_o_raw + p_e_raw, 1e-12), R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
    let entry_mode_split_rand = f32(hash_u32(seed0 ^ hash_u32(bounce ^ BIREFRINGENT_SPLIT_STREAM))) / 4294967295.0;
    let use_extraordinary_entry = entry_mode_split_rand < (1.0 - p_o_hero);
    var p_mode_hero_frac = p_o_hero;
    if (use_extraordinary_entry) {
        p_mode_hero_frac = 1.0 - p_o_hero;
    }

    let n2_hero_dir_e = select(n_o_hero, n2, use_extraordinary_entry);
    let eta_dir_e = n1 / n2_hero_dir_e;
    let sin2_t_dir_e = min(eta_dir_e * eta_dir_e * fma(-cos_i, cos_i, 1.0), 1.0);
    let cos_t_dir_e = sqrt(max(1.0 - sin2_t_dir_e, 0.0));
    let refr_wave_dir_e = normalize(eta_dir_e * wave_dir_at_bounce + fma(eta_dir_e, cos_i, -cos_t_dir_e) * normal);
    var final_refr_dir_e = refr_wave_dir_e;
    if (use_extraordinary_entry) {
        final_refr_dir_e = extraordinary_poynting_dir(refr_wave_dir_e, c_axis, n_o_hero, n_e_hero);
    }
    let p_axis_transmitted = cross(refr_wave_dir_e, uframe.s_axis);

    // this is an INTERIOR dispersive event (air->crystal entry) --
    // mismatch keeps `path_pdf` accumulating (via this event's own
    // `t_unpol_k`) and narrows `compat` below, no split -- see
    // `bounce_arm_uni_entry_core` and
    // `apply_uniaxial_entry_transmit_channels`'s CPU-side doc comment.
    var entry_dirs: array<vec3<f32>, 8>;
    var entry_dirs_valid: array<bool, 8>;
    var entry_hero_match: array<bool, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        entry_dirs[k] = vec3<f32>(0.0, 0.0, 0.0);
        entry_dirs_valid[k] = false;
        entry_hero_match[k] = false;
    }
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        let n_o_k = n_o_hoisted[k];
        var n2k_dir = n_o_k;
        if (use_extraordinary_entry) {
            n2k_dir = n_eff_ch[k];
        }
        let eta_dir_k = n1 / n2k_dir;
        let sin2_t_k = eta_dir_k * eta_dir_k * fma(-cos_i, cos_i, 1.0);
        if (sin2_t_k > 1.0) {
            (*s).stokes[k] = (*s).stokes[k] * 0.0;
            (*s).path_pdf[k] = 0.0;
            continue;
        }
        let cos_t_k = sqrt(max(1.0 - sin2_t_k, 0.0));
        let refr_wave_dir_k = normalize(eta_dir_k * wave_dir_at_bounce + fma(eta_dir_k, cos_i, -cos_t_k) * normal);
        var final_dir_k = refr_wave_dir_k;
        if (use_extraordinary_entry) {
            let n_e_eff_k = n_e_raw_ch[k];
            final_dir_k = extraordinary_poynting_dir(refr_wave_dir_k, c_axis, n_o_k, n_e_eff_k);
        }
        entry_dirs[k] = final_dir_k;
        entry_dirs_valid[k] = true;
        let direction_matches_entry = dot(final_dir_k, final_refr_dir_e) >= DIRECTION_MATCH_COS_TOL;
        entry_hero_match[k] = direction_matches_entry;
        // The entry pair of channel k (channel 0 reuses the hero's solution, PERF-11): the
        // mismatch branch and the matched transmission below both read it.
        var pair_k = pair_hero;
        if (k != 0u) {
            pair_k = entry_solve_pair_with_incidence(uinc_frame, 1.0, n_o_k, n_e_raw_ch[k], c_axis, uframe);
        }
        if (!direction_matches_entry) {
            (*s).stokes[k] = (*s).stokes[k] * 0.0;
            // technique k stays a live member of every compatible
            // channel's MIS family -- keep its own entry-transmit
            // density accumulating (the SAME t_unpol_k the matching
            // branch below folds in); only its radiance ends here.
            let mueller_k_mm = jones_to_mueller(pair_k.s_sol.r_s, pair_k.p_sol.r_s, pair_k.s_sol.r_p, pair_k.p_sol.r_p);
            let t_unpol_k_mm = clamp(1.0 - mueller_k_mm[0][0], 1e-4, 1.0 - 1e-4);
            (*s).path_pdf[k] = (*s).path_pdf[k] * t_unpol_k_mm;
            continue;
        }
        var t_s_row = pair_k.s_sol.t_o;
        var t_p_row = pair_k.p_sol.t_o;
        var flux_k = pair_k.s_sol.flux_o;
        var mode_dir = pair_k.s_sol.o_hat;
        if (use_extraordinary_entry) {
            t_s_row = pair_k.s_sol.t_e;
            t_p_row = pair_k.p_sol.t_e;
            flux_k = pair_k.s_sol.flux_e;
            mode_dir = pair_k.s_sol.e_hat;
        }
        let p_mode_k = mode_power(t_s_row, t_p_row, flux_k, entry_inc_flux, (*s).stokes[k]);
        let deposit_i = p_mode_k / p_mode_hero_frac;
        let azimuth = azimuth2_in_frame(mode_dir, uframe.s_axis, p_axis_transmitted);
        (*s).stokes[k] = vec4<f32>(deposit_i, deposit_i * azimuth.x, deposit_i * azimuth.y, 0.0) * (1.0 / (1.0 - entry_r_branch));

        let mueller_k = jones_to_mueller(pair_k.s_sol.r_s, pair_k.p_sol.r_s, pair_k.s_sol.r_p, pair_k.p_sol.r_p);
        let t_unpol_k = clamp(1.0 - mueller_k[0][0], 1e-4, 1.0 - 1e-4);
        (*s).path_pdf[k] = (*s).path_pdf[k] * t_unpol_k;
    }
    var local_compat = (*s).compat;
    narrow_compat(&local_compat, entry_dirs, entry_dirs_valid, entry_hero_match);
    (*s).compat = local_compat;
    (*s).origin = hit_point + final_refr_dir_e * RAY_EPS;
    (*s).dir = final_refr_dir_e;
    (*s).k = refr_wave_dir_e;
    (*s).inside_gem = true;
    (*s).is_extraordinary = use_extraordinary_entry;
}

@compute @workgroup_size(64)
fn ew_arm_uni_entry_transmit(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= ew.chunk_rays) {
        return;
    }
    let core = ray_core[i];
    if (core.ray_meta.z != TAG_UNI_ENTRY_TRANSMIT) {
        return;
    }

    var s = ew_load(i);
    var h = ew_hit_load(i);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let seed0 = core.ray_meta.y;

    ew_uni_entry_transmit(&s, &h, rc, ew.bounce, seed0);
    surface_finish_event(&s, &h, rc, ew.bounce, seed0);
    ew_store(i, s);
    ew_hit_store(i, h);

    if (h.split_mask != 0u) {
        ray_core[i].ray_meta.z = TAG_SPLIT;
    } else {
        ray_core[i].ray_meta.z = TAG_TAIL;
    }
}
