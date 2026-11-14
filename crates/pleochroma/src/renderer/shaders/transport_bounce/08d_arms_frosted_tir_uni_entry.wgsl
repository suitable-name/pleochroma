// Arm 1: Frosted finish bounce.
fn bounce_arm_frosted(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    let idx = bounce_arm_index(s, h, rc);
    let hit_point = (*h).hit_point;
    let normal = (*h).shading_normal;
    let pre_bounce_inside_gem = (*h).pre_bounce_inside_gem;
    let is_anisotropic = rc.is_anisotropic;
    let is_biaxial = rc.is_biaxial;
    let c_axis = rc.c_axis;
    let studio_key_dir = rc.studio_key_dir;
    let current_plane_normal = (*s).prev_plane_normal;
    var local_lambdas = (*s).lambdas;
    let lambdas = &local_lambdas;
    var local_stokes = (*s).stokes;
    let stokes = &local_stokes;
    var local_path_pdf = (*s).path_pdf;
    let path_pdf = &local_path_pdf;
    var local_radiance = (*s).radiance;
    let radiance = &local_radiance;

    // optics::raytracer::surface::apply_ground_bounce (via dispatch_bounce): a GGX rough
    // dielectric. It replaces the TIR, partial-reflect and refract dispatch for frosted facets.
    // One micro-normal is drawn
    // from the visible-normal distribution of the facet's lobe; the exact mirror/Snell law at
    // it gives the direction, and the throughput is the Fresnel and masking ratio divided by
    // the tabulated directional albedo (`ground_albedo`), so a lossless rough interface keeps
    // its energy. The lobe is re-derived from `(*h).finish` (see `08c_surface.wgsl`).
    let gp = surface_ground_params((*h).finish, normal, pre_bounce_inside_gem, seed0, bounce);
    var ground = ground_ctx_new(
        normal, (*h).wave_dir_at_bounce, gp.lobe, gp.across, gp.has_across,
        idx.n1, idx.n2, idx.cos_i, idx.sin2_t,
    );
    ground.compensation = 1.0 / ground_albedo(ggx_alpha_mean(ground.lobe), ground.i.z, ground.n2 / ground.n1);
    let gs = ground_sample(
        ground,
        surface_unit_draw(seed0, bounce, FROSTED_DIR_U_STREAM),
        surface_unit_draw(seed0, bounce, FROSTED_DIR_V_STREAM),
        surface_unit_draw(seed0, bounce, FRESNEL_BRANCH_STREAM),
    );
    // NEE is eligible exactly for the outcomes that end outside the gem: a reflection while
    // outside, a transmission while inside. It runs before the throughput is scaled, like the
    // CPU's `apply_ground_bounce`.
    let exterior = gs.reflect_branch != pre_bounce_inside_gem;
    if (exterior && nee_light_available()) {
        nee_contribution_ground(
            ground, gs.reflect_branch, lambdas, hit_point, seed0, bounce, stokes, radiance, studio_key_dir,
        );
        if (gs.valid) {
            // The density of the BSDF-sampled continuation given its branch, for the MIS weight
            // of its escape (the CPU's `pending_light_mis`).
            (*s).pending_light_mis = gs.pdf;
            (*s).pending_light_mis_dir = gs.dir;
        }
    }
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        // Depolarizing: the micro-scattering scrambles the phase. `path_pdf` takes the
        // probability of the branch drawn; the branch's energy fraction cancels against its
        // selection probability inside `gs.weight`.
        (*stokes)[k] = vec4<f32>(max((*stokes)[k].x, 0.0) * gs.weight, 0.0, 0.0, 0.0);
        (*path_pdf)[k] = (*path_pdf)[k] * gs.branch_prob;
    }
    var fb: FrostedBounceResult;
    fb.new_dir = gs.dir;
    fb.new_inside_gem = select(0u, 1u, pre_bounce_inside_gem != gs.crossed);
    fb.has_extraordinary_update = 0u;
    fb.extraordinary_update = 0u;
    if (gs.crossed && !pre_bounce_inside_gem && is_anisotropic) {
        // Mode SELECTION is a stochastic 50/50 draw with no throughput weighting, as for the
        // polished entry.
        fb.has_extraordinary_update = 1u;
        fb.extraordinary_update = select(
            0u, 1u,
            surface_unit_draw(seed0, bounce, BIREFRINGENT_SPLIT_STREAM) < 0.5,
        );
    }
    (*s).stokes = local_stokes;
    (*s).path_pdf = local_path_pdf;
    (*s).radiance = local_radiance;
    (*s).origin = hit_point + fb.new_dir * RAY_EPS;
    (*s).dir = fb.new_dir;
    // A frosted bounce depolarizes, so k collapses to S, as in `dispatch_bounce` for
    // `FacetFinish::Frosted`.
    (*s).k = fb.new_dir;
    (*s).inside_gem = fb.new_inside_gem != 0u;
    if (fb.has_extraordinary_update != 0u) {
        (*s).is_extraordinary = fb.extraordinary_update != 0u;
    }
    // Same `was_internal_reflection` as `dispatch_bounce`: true for the reflect branch,
    // including TIR (`inside_gem` unchanged, no extraordinary update), false for transmit
    // (`inside_gem` flips).
    let was_internal_reflection = pre_bounce_inside_gem
        && (fb.has_extraordinary_update == 0u)
        && ((*s).inside_gem == pre_bounce_inside_gem);
    if (is_anisotropic && was_internal_reflection) {
        // No stokes or path_pdf scaling. This relabels the eigenmode for the next bounce and
        // does not split the ray (see `internal_mode_coupling_draw`).
        (*s).is_extraordinary = internal_mode_coupling_draw(
            c_axis, is_biaxial, current_plane_normal, fb.new_dir,
            (*s).stokes[0].x, (*s).stokes[0].y, (*s).stokes[0].z, false, 0.0, seed0, bounce,
        );
    }
    return PHASE_NEXT;
}

// Arm 2: Forced TIR bounce.
fn bounce_arm_tir_core(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    let idx = bounce_arm_index(s, h, rc);
    let hit_point = (*h).hit_point;
    let normal = (*h).shading_normal;
    let wave_dir_at_bounce = (*h).wave_dir_at_bounce;
    let is_anisotropic = rc.is_anisotropic;
    let is_biaxial = rc.is_biaxial;
    let c_axis = rc.c_axis;
    let n_o_hoisted = rc.n_o_hoisted;
    let n_alpha_hero = rc.n_alpha_hero;
    let n_beta_hero = rc.n_beta_hero;
    let n_gamma_hero = rc.n_gamma_hero;
    let biax_ax0 = rc.biax_ax0;
    let biax_ax1 = rc.biax_ax1;
    let biax_ax2 = rc.biax_ax2;
    let current_plane_normal = (*s).prev_plane_normal;
    let cos_i = idx.cos_i;
    let sin_i = idx.sin_i;
    let uframe = idx.uframe;
    let n_o_hero = idx.n_o_hero;
    let n_e_hero = idx.n_e_hero;
    let n_medium_ch = idx.n_medium_ch;
    let n_e_raw_ch = idx.n_e_raw_ch;
    let uniaxial_nondegenerate = idx.uniaxial_nondegenerate;

    // Hero forced TIR has probability 1, so no pdf division. The uniaxial branch is the
    // Lekner 1991 closed-form o/e-coupled TIR reflectance, matching `apply_tir_bounce` on the
    // CPU. At a wave normal parallel to the optic axis `uniaxial_nondegenerate` is false, and
    // the scalar branch below runs, as in `apply_tir_bounce`. `tir_exact_p_o` is the hero's
    // `R_o/(R_o+R_e)`, passed to `internal_mode_coupling_draw` as the CPU's `exact_p_o`.
    var tir_exact_p_o: f32 = 0.5;
    if (uniaxial_nondegenerate) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            let n_inc_k = select(1.0, n_medium_ch[k], (*s).inside_gem);
            let n2k_dbg = select(n_medium_ch[k], 1.0, (*s).inside_gem);
            let etak_dbg = n_inc_k / n2k_dbg;
            let sin2_t_k = etak_dbg * etak_dbg * fma(-cos_i, cos_i, 1.0);
            // Past its own critical angle a channel reflects all of its energy: the transmitted
            // wave is evanescent, so the solve's total reflectance is exactly 1 and is not
            // computed (Stokes and path_pdf are left as they are). The hero still solves, for
            // the mode-coupling probability `tir_exact_p_o`. Mirrors `apply_tir_bounce`.
            if (sin2_t_k > 1.0 && k != 0u) {
                continue;
            }
            let sol = internal_solve(n_inc_k, n_o_hoisted[k], n_e_raw_ch[k], c_axis, uframe, !(*s).is_extraordinary);
            let flux_inc = max(sol.flux_inc, 1e-12);
            let ro_pow = cplx_norm_sqr(sol.r_o) * sol.flux_ro;
            let re_pow = cplx_norm_sqr(sol.r_e) * sol.flux_re;
            if (sin2_t_k <= 1.0) {
                let r_total_k = min((ro_pow + re_pow) / flux_inc, 1.0);
                (*s).stokes[k] = (*s).stokes[k] * r_total_k;
                // Exit collection follows its chain with probability one: no branch factor
                // (`dispatch_bounce` puts the densities back after `apply_tir_bounce`).
                if (!collect_on) {
                    (*s).path_pdf[k] = (*s).path_pdf[k] * clamp(r_total_k, 1e-4, 1.0 - 1e-4);
                }
            }
            if (k == 0u) {
                tir_exact_p_o = ro_pow / max(ro_pow + re_pow, 1e-12);
            }
        }
    } else {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            let n1k = select(1.0, n_medium_ch[k], (*s).inside_gem);
            let n2k = select(n_medium_ch[k], 1.0, (*s).inside_gem);
            let etak = n1k / n2k;
            let sin2_t_k = etak * etak * fma(-cos_i, cos_i, 1.0);
            if (sin2_t_k > 1.0) {
                let delta_k = tir_phase_delta(n1k, cos_i, sin_i);
                (*s).stokes[k] = mueller_tir_retardation(delta_k) * (*s).stokes[k];
            } else {
                let cos_t_k = sqrt(max(1.0 - sin2_t_k, 0.0));
                let r_s_k = fma(n2k, -cos_t_k, n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
                let r_p_k = fma(n1k, -cos_t_k, n2k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
                (*s).stokes[k] = mueller_fresnel_reflection(r_s_k, r_p_k) * (*s).stokes[k];
                let r_unpol_k = clamp(0.5 * fma(r_p_k, r_p_k, r_s_k * r_s_k), R_UNPOL_MIN, R_UNPOL_MAX);
                // No branch factor under exit collection, as in the uniaxial loop above.
                if (!collect_on) {
                    (*s).path_pdf[k] = (*s).path_pdf[k] * r_unpol_k;
                }
            }
        }
    }
    // Reflects the wave normal `k`, then re-derives the Poynting direction `S'` for `k'` via
    // `poynting_dir_for_mode`.
    let k_prime = wave_dir_at_bounce - 2.0 * dot(wave_dir_at_bounce, normal) * normal;
    let s_prime = poynting_dir_for_mode(
        is_anisotropic, is_biaxial, (*s).inside_gem, (*s).is_extraordinary, k_prime, c_axis,
        n_o_hero, n_e_hero, n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2,
    );
    (*s).origin = hit_point + s_prime * RAY_EPS;
    (*s).dir = s_prime;
    (*s).k = k_prime;

    // TIR is always an internal reflection (`n1 > n2` for the hero
    // channel implies `(*inside_gem)`, exactly as on the CPU side).
    if (is_anisotropic) {
        // `has_exact_p_o` is `uniaxial_nondegenerate`. `tir_exact_p_o` is solved only when that
        // branch ran. At the degenerate limit it keeps its default 0.5, which must not count as
        // exact. The CPU returns `exact_p_o: None` there, so the polarization heuristic runs.
        // No stokes or path_pdf scaling: this is a relabeling, not a split.
        (*s).is_extraordinary = internal_mode_coupling_draw(
            c_axis, is_biaxial, current_plane_normal, k_prime,
            (*s).stokes[0].x, (*s).stokes[0].y, (*s).stokes[0].z, uniaxial_nondegenerate, tir_exact_p_o, seed0, bounce,
        );
    }
    return PHASE_NEXT;
}

// Arm 3: Uniaxial entry bounce.
fn bounce_arm_uni_entry_core(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    let idx = bounce_arm_index(s, h, rc);
    let hit_point = (*h).hit_point;
    let normal = (*h).shading_normal;
    let wave_dir_at_bounce = (*h).wave_dir_at_bounce;
    let c_axis = rc.c_axis;
    let n_o_hoisted = rc.n_o_hoisted;
    let birefringence_delta = rc.birefringence_delta;
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
    let entry_mode_split_rand = f32(discrete_draw(seed0, bounce, BIREFRINGENT_SPLIT_STREAM)) / 4294967295.0;
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

    let entry_rng_bounce = f32(discrete_draw(seed0, bounce, FRESNEL_BRANCH_STREAM)) / 4294967295.0;
    // Exit collection (`apply_uniaxial_entry_collect`): no reflect-vs-transmit draw. The mirror
    // reflection of every channel is staged as a leaf (its own `R_k`-weighted state, nothing
    // divided out, times the surface glare at bounce 0) and the path takes the transmit branch
    // undivided: `entry_t_div` is the `r_branch` of the transmit loop, zero under collection, and
    // the loop leaves the densities as they are. A leaf of the last iteration earns nothing, as
    // on the CPU (`ExitCollect::credit`).
    let entry_t_div = select(entry_r_branch, 0.0, collect_on);
    let entry_leaf_on = collect_on && bounce + 1u < params.max_bounces;
    var entry_leaf_scale: f32 = 1.0;
    if (bounce == 0u && params.surface_glare < 1.0) {
        entry_leaf_scale = params.surface_glare;
    }
    let entry_mirror_dir = wave_dir_at_bounce - 2.0 * dot(wave_dir_at_bounce, normal) * normal;

    if (!collect_on && entry_rng_bounce < entry_r_branch) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            // Channel 0 is the hero. `pair_hero` was solved with the same arguments, so it is
            // reused.
            var pair_k = pair_hero;
            if (k != 0u) {
                pair_k = entry_solve_pair_with_incidence(uinc_frame, 1.0, n_o_hoisted[k], n_e_raw_ch[k], c_axis, uframe);
            }
            let mueller_k = jones_to_mueller(pair_k.s_sol.r_s, pair_k.p_sol.r_s, pair_k.s_sol.r_p, pair_k.p_sol.r_p);
            let r_unpol_k = clamp(mueller_k[0][0], 1e-4, 1.0 - 1e-4);
            (*s).stokes[k] = (mueller_k * (*s).stokes[k]) * (1.0 / entry_r_branch);
            (*s).path_pdf[k] = (*s).path_pdf[k] * r_unpol_k;
        }
        let new_k_e = wave_dir_at_bounce - 2.0 * dot(wave_dir_at_bounce, normal) * normal;
        (*s).origin = hit_point + new_k_e * RAY_EPS;
        (*s).dir = new_k_e;
        (*s).k = new_k_e;
    } else {
        // Interior dispersive event (air to crystal). A channel whose direction disagrees with
        // the hero keeps `path_pdf` accumulating through its own `t_unpol_k`, and `compat` is
        // narrowed below. No split. See `apply_uniaxial_entry_transmit_channels` on the CPU.
        var entry_dirs: array<vec3<f32>, 8>;
        // Bit k: channel k refracted (`entry_dirs[k]` holds its direction); that direction
        // matches the hero's.
        var entry_dirs_valid: u32 = 0u;
        var entry_hero_match: u32 = 0u;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            entry_dirs[k] = vec3<f32>(0.0, 0.0, 0.0);
        }
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            let n_o_k = n_o_hoisted[k];
            // The incident state, for the mirror leaf of exit collection.
            let entry_stokes_k = (*s).stokes[k];
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
            entry_dirs_valid = entry_dirs_valid | (1u << k);
            let direction_matches_entry = directions_match(final_dir_k, final_refr_dir_e);
            entry_hero_match = entry_hero_match | (u32(direction_matches_entry) << k);
            if (!direction_matches_entry) {
                (*s).stokes[k] = (*s).stokes[k] * 0.0;
                // technique k stays a live member of every compatible
                // channel's MIS family -- keep its own entry-transmit
                // density accumulating (the SAME t_unpol_k the matching
                // branch below folds in); only its radiance ends here.
                var pair_k_mm = pair_hero;
                if (k != 0u) {
                    pair_k_mm = entry_solve_pair_with_incidence(uinc_frame, 1.0, n_o_k, n_e_raw_ch[k], c_axis, uframe);
                }
                let mueller_k_mm = jones_to_mueller(pair_k_mm.s_sol.r_s, pair_k_mm.p_sol.r_s, pair_k_mm.s_sol.r_p, pair_k_mm.p_sol.r_p);
                let t_unpol_k_mm = clamp(1.0 - mueller_k_mm[0][0], 1e-4, 1.0 - 1e-4);
                if (!collect_on) {
                    (*s).path_pdf[k] = (*s).path_pdf[k] * t_unpol_k_mm;
                }
                if (entry_leaf_on) {
                    collect_stage_leaf(
                        h, k, entry_mirror_dir, (mueller_k_mm * entry_stokes_k).x,
                        collect_leaf_weight((*s).compat[k], (*s).path_pdf, 0xFFu) * entry_leaf_scale,
                    );
                }
                continue;
            }
            var pair_k = pair_hero;
            if (k != 0u) {
                pair_k = entry_solve_pair_with_incidence(uinc_frame, 1.0, n_o_k, n_e_raw_ch[k], c_axis, uframe);
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
            (*s).stokes[k] = vec4<f32>(deposit_i, deposit_i * azimuth.x, deposit_i * azimuth.y, 0.0) * (1.0 / (1.0 - entry_t_div));

            let mueller_k = jones_to_mueller(pair_k.s_sol.r_s, pair_k.p_sol.r_s, pair_k.s_sol.r_p, pair_k.p_sol.r_p);
            let t_unpol_k = clamp(1.0 - mueller_k[0][0], 1e-4, 1.0 - 1e-4);
            if (!collect_on) {
                (*s).path_pdf[k] = (*s).path_pdf[k] * t_unpol_k;
            }
            if (entry_leaf_on) {
                collect_stage_leaf(
                    h, k, entry_mirror_dir, (mueller_k * entry_stokes_k).x,
                    collect_leaf_weight((*s).compat[k], (*s).path_pdf, 0xFFu) * entry_leaf_scale,
                );
            }
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
    return PHASE_NEXT;
}

