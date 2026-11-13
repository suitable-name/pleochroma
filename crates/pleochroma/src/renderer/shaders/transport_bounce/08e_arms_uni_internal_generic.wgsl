// Arm 4: Uniaxial internal/exit bounce.
fn bounce_arm_uni_internal_core(
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
    let is_biaxial = rc.is_biaxial;
    let n_o_hoisted = rc.n_o_hoisted;
    let studio_key_dir = rc.studio_key_dir;
    let current_plane_normal = (*s).prev_plane_normal;
    let phase_pdf_this_check = (*h).phase_pdf_this_check;
    let phase_dir_this_check = (*h).phase_dir_this_check;
    let cos_i = idx.cos_i;
    let uframe = idx.uframe;
    let n_o_hero = idx.n_o_hero;
    let n_e_hero_solve = idx.n_e_raw_ch[0];
    let n_e_raw_ch = idx.n_e_raw_ch;
    let n_medium_ch = idx.n_medium_ch;
    let n1 = idx.n1;
    let n2 = idx.n2;

    // ---- INTERNAL: mirrors apply_uniaxial_internal_bounce ----
    let sol_hero_i = internal_solve(n1, n_o_hero, n_e_hero_solve, c_axis, uframe, !(*s).is_extraordinary);
    let flux_inc_hero_i = max(sol_hero_i.flux_inc, 1e-12);
    let ro_pow_hero = cplx_norm_sqr(sol_hero_i.r_o) * sol_hero_i.flux_ro;
    let re_pow_hero = cplx_norm_sqr(sol_hero_i.r_e) * sol_hero_i.flux_re;
    let internal_r_branch = clamp((ro_pow_hero + re_pow_hero) / flux_inc_hero_i, R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
    let internal_p_o_exact = ro_pow_hero / max(ro_pow_hero + re_pow_hero, 1e-12);
    let internal_rng_bounce = f32(discrete_draw(seed0, bounce, FRESNEL_BRANCH_STREAM)) / 4294967295.0;
    // Exit collection: no draw. Each channel that can transmit stages its exit leaf (its
    // transmitted intensity along its refracted direction, undivided). The path takes the
    // reflection undivided too: `internal_r_div` is 1 under collection. `internal_producers`
    // masks the channels not totally reflected here. A leaf from the last iteration earns
    // nothing (`ExitCollect::credit`).
    let internal_r_div = select(internal_r_branch, 1.0, collect_on);
    let internal_leaf_on = collect_on && bounce + 1u < params.max_bounces;
    var internal_producers: u32 = 0u;
    if (internal_leaf_on) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            if (n_medium_ch[k] * n_medium_ch[k] * fma(-cos_i, cos_i, 1.0) <= 1.0) {
                internal_producers = internal_producers | (1u << k);
            }
        }
    }

    if (collect_on || internal_rng_bounce < internal_r_branch) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            var sol = sol_hero_i;
            if (k != 0u) {
                sol = internal_solve(n_medium_ch[k], n_o_hoisted[k], n_e_raw_ch[k], c_axis, uframe, !(*s).is_extraordinary);
            }
            if (internal_leaf_on && (internal_producers & (1u << k)) != 0u) {
                // Leaf from the incident state, as `uniaxial_exit_leaf_intensities` does on the
                // CPU.
                let n_inc_leaf = n_medium_ch[k];
                let cos_t_leaf = sqrt(max(1.0 - n_inc_leaf * n_inc_leaf * fma(-cos_i, cos_i, 1.0), 0.0));
                let et_leaf = compute_uniaxial_exit_transmission(sol, 0.0, (*s).stokes[k].x);
                collect_stage_leaf(
                    h, k,
                    normalize(n_inc_leaf * wave_dir_at_bounce + fma(n_inc_leaf, cos_i, -cos_t_leaf) * normal),
                    et_leaf.transmitted.x,
                    collect_leaf_weight((*s).compat[k], (*s).path_pdf, internal_producers),
                );
            }
            let flux_inc_k = max(sol.flux_inc, 1e-12);
            // `fma` matches `apply_uniaxial_internal_reflect_channels` bit for bit. Without it
            // the result differs by 1 ULP.
            let r_total_k = min(fma(cplx_norm_sqr(sol.r_e), sol.flux_re, cplx_norm_sqr(sol.r_o) * sol.flux_ro) / flux_inc_k, 1.0);
            (*s).stokes[k] = (*s).stokes[k] * (r_total_k / internal_r_div);
            if (!collect_on) {
                (*s).path_pdf[k] = (*s).path_pdf[k] * clamp(r_total_k, 1e-4, 1.0 - 1e-4);
            }
        }
        let new_k_i = wave_dir_at_bounce - 2.0 * dot(wave_dir_at_bounce, normal) * normal;
        (*s).origin = hit_point + new_k_i * RAY_EPS;
        (*s).dir = new_k_i;
        (*s).k = new_k_i;
        (*s).is_extraordinary = internal_mode_coupling_draw(
            c_axis, is_biaxial, current_plane_normal, new_k_i,
            (*s).stokes[0].x, (*s).stokes[0].y, (*s).stokes[0].z, true, internal_p_o_exact, seed0, bounce,
        );
    } else {
        let eta_dir_i = n1 / n2;
        let sin2_t_dir_i = min(eta_dir_i * eta_dir_i * fma(-cos_i, cos_i, 1.0), 1.0);
        let cos_t_dir_i = sqrt(max(1.0 - sin2_t_dir_i, 0.0));
        let hero_refr_dir_i = normalize(eta_dir_i * wave_dir_at_bounce + fma(eta_dir_i, cos_i, -cos_t_dir_i) * normal);

        // Reached only while `inside_gem` is true, so this transmit is always a real exit. The
        // split weight matches `dispatch_bounce`'s `exit.split_mis_weight`: same balance
        // heuristic, same light pdf at the carried interior direction.
        var split_mis_weight_i: f32 = 1.0;
        if (phase_pdf_this_check > 0.0 && nee_light_available()) {
            let light_pdf_i = nee_light_pdf(phase_dir_this_check, studio_key_dir);
            split_mis_weight_i = balance_heuristic(phase_pdf_this_check, light_pdf_i);
        }

        // Uniaxial exact exit (crystal to air). A mismatched channel resolves its own split and
        // keeps `path_pdf` accumulating through this event's exit factor. It never narrows
        // `compat`. See `apply_uniaxial_internal_transmit_channels` on the CPU.
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            // Captured before the mutations below, so the split branch still has the original
            // incident value.
            let original_stokes_i = (*s).stokes[k].x;
            let n_inc_k = n_medium_ch[k];
            let eta_dir_k = n_inc_k;
            let sin2_t_k = eta_dir_k * eta_dir_k * fma(-cos_i, cos_i, 1.0);
            if (sin2_t_k > 1.0) {
                (*s).stokes[k] = (*s).stokes[k] * 0.0;
                (*s).path_pdf[k] = 0.0;
                continue;
            }
            let cos_t_k = sqrt(max(1.0 - sin2_t_k, 0.0));
            let refr_wave_dir_k = normalize(eta_dir_k * wave_dir_at_bounce + fma(eta_dir_k, cos_i, -cos_t_k) * normal);
            let direction_matches_i = directions_match(refr_wave_dir_k, hero_refr_dir_i);

            // Channel 0 reuses the hero solve, which is identical.
            var sol = sol_hero_i;
            if (k != 0u) {
                sol = internal_solve(n_inc_k, n_o_hoisted[k], n_e_raw_ch[k], c_axis, uframe, !(*s).is_extraordinary);
            }

            if (!direction_matches_i) {
                // Chromatic termination.
                let prefix_path_pdf_k = (*s).path_pdf[k];
                (*s).stokes[k] = (*s).stokes[k] * 0.0;
                (*s).path_pdf[k] = 0.0;

                let et_mm = compute_uniaxial_exit_transmission(sol, internal_r_branch, original_stokes_i);
                let t_unpol_k_mm = clamp(et_mm.i_unit, 1e-4, 1.0 - 1e-4);
                (*s).path_pdf[k] = prefix_path_pdf_k * t_unpol_k_mm;
                if (original_stokes_i > 0.0) {
                    (*h).split_mask = (*h).split_mask | (1u << k);
                    (*h).split_dir[k] = vec4<f32>(refr_wave_dir_k, et_mm.transmitted.x);
                    (*h).split_mis_weight = split_mis_weight_i;
                }
                continue;
            }

            // Matching direction: plain transmission.
            let et = compute_uniaxial_exit_transmission(sol, internal_r_branch, original_stokes_i);
            (*s).stokes[k] = et.transmitted;
            let t_unpol_k = clamp(et.i_unit, 1e-4, 1.0 - 1e-4);
            (*s).path_pdf[k] = (*s).path_pdf[k] * t_unpol_k;
        }
        (*s).origin = hit_point + hero_refr_dir_i * RAY_EPS;
        (*s).dir = hero_refr_dir_i;
        (*s).k = hero_refr_dir_i;
        (*s).inside_gem = false;
        // Reached only with `inside_gem` true, so a live incoming NEE carry is a real
        // transmit-out. Pass it through unchanged, as `dispatch_bounce` does for the generic exit.
        (*s).pending_light_mis = phase_pdf_this_check;
        (*s).pending_light_mis_dir = phase_dir_this_check;
    }
    return PHASE_NEXT;
}

// Arm 5: Generic (isotropic/biaxial/degenerate-uniaxial) bounce.
fn bounce_arm_generic_core(
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
    let birefringence_delta = rc.birefringence_delta;
    let studio_key_dir = rc.studio_key_dir;
    let current_plane_normal = (*s).prev_plane_normal;
    let phase_pdf_this_check = (*h).phase_pdf_this_check;
    let phase_dir_this_check = (*h).phase_dir_this_check;
    let n_alpha_hero = rc.n_alpha_hero;
    let n_beta_hero = rc.n_beta_hero;
    let n_gamma_hero = rc.n_gamma_hero;
    let biax_ax0 = rc.biax_ax0;
    let biax_ax1 = rc.biax_ax1;
    let biax_ax2 = rc.biax_ax2;
    let cos_i = idx.cos_i;
    let sin_i = idx.sin_i;
    let n_o_hero = idx.n_o_hero;
    let n_e_hero = idx.n_e_hero;
    let n_medium_ch = idx.n_medium_ch;
    let n_biax_a_hero = idx.n_biax_a_hero;
    let n_biax_b_hero = idx.n_biax_b_hero;
    let n_biax_a_ch = idx.n_biax_a_ch;
    let n_alpha_ch = idx.n_alpha_ch;
    let n_beta_ch = idx.n_beta_ch;
    let n_gamma_ch = idx.n_gamma_ch;
    let n1 = idx.n1;
    let n2 = idx.n2;

    // On an air-to-crystal entry into an anisotropic material the eigenmode (ordinary, mode A,
    // or extraordinary, mode B) is chosen here, before the reflect-or-transmit draw, and both
    // branches share the choice. First, the choice is polarization-weighted
    // (`entry_eigenmode_selection`), so a beam aligned with the ordinary axis also reflects
    // more readily at the ordinary index. Second, the reflect Fresnel coefficients use the same
    // mode's index as the transmit branch, so `R + T == 1` for the selected mode. See
    // `apply_partial_fresnel_bounce` on the CPU. A biaxial material has no uniaxial ordinary
    // mode, so both steps are gated on `!is_biaxial`.
    let entering_anisotropic = !(*s).inside_gem && is_anisotropic;
    var entry_valid = 0u;
    var entry_cos_2psi_o = 0.0;
    var entry_sin_2psi_o = 0.0;
    var p_o = 0.5;
    if (entering_anisotropic && !is_biaxial) {
        let sel = entry_eigenmode_selection(
            c_axis, current_plane_normal, wave_dir_at_bounce,
            (*s).stokes[0].x, (*s).stokes[0].y, (*s).stokes[0].z,
        );
        entry_valid = sel.valid;
        entry_cos_2psi_o = sel.cos_2psi_o;
        entry_sin_2psi_o = sel.sin_2psi_o;
        if (sel.valid != 0u) {
            p_o = sel.p_o;
        }
    }
    var use_extraordinary = (*s).is_extraordinary;
    if (entering_anisotropic) {
        // The draw is read only in the comparison below, so it runs only on an anisotropic entry.
        let mode_split_rand = f32(discrete_draw(seed0, bounce, BIREFRINGENT_SPLIT_STREAM)) / 4294967295.0;
        // Must match the CPU's `mode_split_rand < (1.0 - p_o)` exactly, same operator and sense.
        // `rand >= p_o` has the same probability but picks the other half of the draw, which
        // assigns the opposite mode. Do not simplify.
        use_extraordinary = mode_split_rand < (1.0 - p_o);
    }
    // The chosen mode's own doubled polarization azimuth, for the per-channel
    // eigenmode projection in the REFRACT arm below -- meaningless unless
    // `entry_valid != 0u`. Extraordinary is perpendicular to ordinary
    // (psi_e == psi_o + 90deg), so its doubled azimuth is the negation of the
    // ordinary one.
    var entry_cos_2psi_x = entry_cos_2psi_o;
    var entry_sin_2psi_x = entry_sin_2psi_o;
    if (use_extraordinary) {
        entry_cos_2psi_x = -entry_cos_2psi_o;
        entry_sin_2psi_x = -entry_sin_2psi_o;
    }

    // `n2_decision` is the selected mode's index at this interface: `n_o_hero` for the
    // ordinary mode on a uniaxial anisotropic entry, otherwise `n2`. Recomputing the ratio
    // here gives the same bits as the hero-level `sin2_t` when the index is unchanged.
    let n2_decision = select(n2, n_o_hero, entering_anisotropic && !is_biaxial && !use_extraordinary);
    let eta_decision = n1 / n2_decision;
    let cos_t = sqrt(max(1.0 - eta_decision * eta_decision * fma(-cos_i, cos_i, 1.0), 0.0));
    let r_s = fma(n2_decision, -cos_t, n1 * cos_i) / fma(n2_decision, cos_t, n1 * cos_i);
    let r_p = fma(n1, -cos_t, n2_decision * cos_i) / fma(n1, cos_t, n2_decision * cos_i);
    let r_unpol_raw = 0.5 * fma(r_p, r_p, r_s * r_s);
    // Clamped to [0.02, 0.98], a wider range than the per-channel one used for `path_pdf`.
    // This caps the `1/r_unpol` and `1/(1-r_unpol)` divisions below at 50x instead of
    // 10,000x at grazing incidence, which reduces fireflies.
    let r_unpol = clamp(r_unpol_raw, R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
    let rng_bounce = f32(discrete_draw(seed0, bounce, FRESNEL_BRANCH_STREAM)) / 4294967295.0;
    // Exit collection: no draw. Inside the gem, each transmitting channel stages its exit leaf
    // and the path takes the reflection undivided (`r_div` is 1). At an entry the reflect loop
    // only stages the mirror leaf of each channel, scaled by the surface glare at bounce 0. The
    // refraction then runs undivided (`r_t` is 0). Neither loop changes the densities. A leaf
    // from the last iteration earns nothing (`ExitCollect::credit`).
    let collect_entry = collect_on && !(*s).inside_gem;
    let r_div = select(r_unpol, 1.0, collect_on);
    let r_t = select(r_unpol, 0.0, collect_on);
    let generic_leaf_on = collect_on && bounce + 1u < params.max_bounces;
    var generic_producers: u32 = 0xFFu;
    if (generic_leaf_on && (*s).inside_gem) {
        generic_producers = 0u;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            if (n_medium_ch[k] * n_medium_ch[k] * fma(-cos_i, cos_i, 1.0) <= 1.0) {
                generic_producers = generic_producers | (1u << k);
            }
        }
    }
    var generic_leaf_scale: f32 = 1.0;
    if (bounce == 0u && params.surface_glare < 1.0) {
        generic_leaf_scale = params.surface_glare;
    }
    let generic_mirror_dir = wave_dir_at_bounce - 2.0 * dot(wave_dir_at_bounce, normal) * normal;
    let run_reflect = collect_on || rng_bounce < r_unpol;

    if (run_reflect) {
        // REFLECT: each channel applies its own Fresnel or TIR matrix, divided by the hero's
        // selection probability `r_unpol`.
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            let n1k = select(1.0, n_medium_ch[k], (*s).inside_gem);
            let n2k = select(n_medium_ch[k], 1.0, (*s).inside_gem);
            let etak = n1k / n2k;
            let sin2_t_k = etak * etak * fma(-cos_i, cos_i, 1.0);
            var refl: mat4x4<f32>;
            if (sin2_t_k > 1.0) {
                let delta_k = tir_phase_delta(n1k, cos_i, sin_i);
                refl = mueller_tir_retardation(delta_k);
            } else {
                let cos_t_k = sqrt(max(1.0 - sin2_t_k, 0.0));
                let r_s_k = fma(n2k, -cos_t_k, n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
                let r_p_k = fma(n1k, -cos_t_k, n2k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
                let r_unpol_k = clamp(0.5 * fma(r_p_k, r_p_k, r_s_k * r_s_k), R_UNPOL_MIN, R_UNPOL_MAX);
                if (!collect_on) {
                    (*s).path_pdf[k] = (*s).path_pdf[k] * r_unpol_k;
                }
                refl = mueller_fresnel_reflection(r_s_k, r_p_k);
                if (generic_leaf_on && (*s).inside_gem) {
                    // `scalar_exit_leaf_intensities` and `exit_leaf_dirs`, from the incident state.
                    let ct_leaf = compute_channel_transmission(
                        n1k, n2k, cos_i, cos_t_k, 0.0, false, false, 0.0, 0.0, (*s).stokes[k],
                    );
                    collect_stage_leaf(
                        h, k,
                        normalize(etak * wave_dir_at_bounce + fma(etak, cos_i, -cos_t_k) * normal),
                        ct_leaf.transmitted.x,
                        collect_leaf_weight((*s).compat[k], (*s).path_pdf, generic_producers),
                    );
                }
            }
            let reflected_k = refl * (*s).stokes[k];
            if (collect_entry) {
                // The mirror leaf of an entry; the incident state goes on to the refract loop.
                if (generic_leaf_on) {
                    collect_stage_leaf(
                        h, k, generic_mirror_dir, reflected_k.x,
                        collect_leaf_weight((*s).compat[k], (*s).path_pdf, 0xFFu) * generic_leaf_scale,
                    );
                }
            } else {
                (*s).stokes[k] = reflected_k * (1.0 / r_div);
            }
        }
        // Reflects the wave normal `k`, then re-derives the Poynting direction `S'` via
        // `poynting_dir_for_mode`.
        let k_prime = wave_dir_at_bounce - 2.0 * dot(wave_dir_at_bounce, normal) * normal;
        let s_prime = poynting_dir_for_mode(
            is_anisotropic, is_biaxial, (*s).inside_gem, (*s).is_extraordinary, k_prime, c_axis,
            n_o_hero, n_e_hero, n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2,
        );
        (*s).origin = hit_point + s_prime * RAY_EPS;
        (*s).dir = s_prime;
        (*s).k = k_prime;

        // This arm never changes `(*inside_gem)` (unlike the refract arm
        // below, which always flips it), so `(*inside_gem)` here still holds its
        // pre-bounce value -- an internal reflection iff it was already true.
        if (is_anisotropic && (*s).inside_gem) {
            // No stokes or path_pdf scaling: a relabeling, not a split. This arm handles biaxial
            // and degenerate-axis uniaxial materials, where no exact p_o exists, so the
            // polarization heuristic runs.
            (*s).is_extraordinary = internal_mode_coupling_draw(
                c_axis, is_biaxial, current_plane_normal, k_prime,
                (*s).stokes[0].x, (*s).stokes[0].y, (*s).stokes[0].z, false, 0.0, seed0, bounce,
            );
        }
    }
    // The refraction: the drawn branch, or the undivided continuation of a collected entry (the
    // reflect block above then only staged the mirror leaf; the ray it wrote is overwritten below).
    if (!run_reflect || collect_entry) {
        // REFRACT, as `optics::raytracer::apply_refract_bounce`. `entering_anisotropic` and
        // `use_extraordinary` are resolved above, with no new draw. On an air-to-crystal entry
        // the selected mode's Fresnel transmittance applies to its projected Stokes state (see
        // the channel loop) at unscaled intensity. The selection probability equals that mode's
        // energy fraction, so the multiply and the divide cancel. Otherwise `use_extraordinary`
        // keeps `is_extraordinary`.
        //
        // Hero direction, computed before the channel loop so each channel's own direction can
        // be compared with it. The ordinary ray of a uniaxial material is not walked off. The
        // extraordinary Poynting direction is displaced by the walk-off angle. For a biaxial
        // entry both modes walk off via `biaxial_mode_poynting_dir`, using `n_biax_a_hero` and
        // `n_biax_b_hero`.
        // `refr_wave_dir` is the Snell-refracted wave normal `k'`, from `wave_dir_at_bounce`
        // and not S. It goes into `new_k` alongside the Poynting direction `final_refr_dir`
        // (`S'`), since both are needed later.
        var new_k: vec3<f32>;
        var final_refr_dir: vec3<f32>;
        if (entering_anisotropic && is_biaxial) {
            let n2_hero_dir = select(n_biax_a_hero, n_biax_b_hero, use_extraordinary);
            let eta_dir = n1 / n2_hero_dir;
            let sin2_t_dir = min(eta_dir * eta_dir * fma(-cos_i, cos_i, 1.0), 1.0);
            let cos_t_dir = sqrt(max(1.0 - sin2_t_dir, 0.0));
            let refr_wave_dir = normalize(
                eta_dir * wave_dir_at_bounce + fma(eta_dir, cos_i, -cos_t_dir) * normal,
            );
            new_k = refr_wave_dir;
            final_refr_dir = biaxial_mode_poynting_dir(n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2, refr_wave_dir, use_extraordinary);
        } else {
            let n2_hero_dir = select(n2, n_o_hero, entering_anisotropic && !use_extraordinary);
            let eta_dir = n1 / n2_hero_dir;
            let sin2_t_dir = min(eta_dir * eta_dir * fma(-cos_i, cos_i, 1.0), 1.0);
            let cos_t_dir = sqrt(max(1.0 - sin2_t_dir, 0.0));
            let refr_wave_dir = normalize(
                eta_dir * wave_dir_at_bounce + fma(eta_dir, cos_i, -cos_t_dir) * normal,
            );
            new_k = refr_wave_dir;
            final_refr_dir = refr_wave_dir;
            if (entering_anisotropic && use_extraordinary) {
                final_refr_dir = extraordinary_poynting_dir(refr_wave_dir, c_axis, n_o_hero, n_e_hero);
            }
        }

        // `is_exit_event` is `inside_gem` before the flip. Since `entering_anisotropic` requires
        // `!inside_gem`, this matches `apply_refract_channel`'s exit test. Interior mismatches
        // only narrow `compat` and never split.
        let is_exit_event = (*s).inside_gem;
        // Matches `dispatch_bounce`'s `exit.split_mis_weight`. The split branch is reachable only
        // from the hero-driven transmit, so the weight can be computed before the channel loop.
        var split_mis_weight: f32 = 1.0;
        if (is_exit_event && phase_pdf_this_check > 0.0 && nee_light_available()) {
            let light_pdf = nee_light_pdf(phase_dir_this_check, studio_key_dir);
            split_mis_weight = balance_heuristic(phase_pdf_this_check, light_pdf);
        }
        var scalar_dirs: array<vec3<f32>, 8>;
        var scalar_dirs_valid: array<bool, 8>;
        var scalar_hero_match: array<bool, 8>;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            scalar_dirs[k] = vec3<f32>(0.0, 0.0, 0.0);
            scalar_dirs_valid[k] = false;
            scalar_hero_match[k] = false;
        }
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            // captured before any mutation below, so the mismatch/split
            // branch (which needs the ORIGINAL incident value after `(*stokes)[k]`
            // has already been zeroed by the unchanged chromatic-termination
            // bookkeeping) can still compute k's own transmission.
            let original_stokes_k = (*s).stokes[k];
            let n1k = select(1.0, n_medium_ch[k], (*s).inside_gem);
            var n2k = select(n_medium_ch[k], 1.0, (*s).inside_gem);
            if (entering_anisotropic && !use_extraordinary) {
                if (is_biaxial) {
                    n2k = n_biax_a_ch[k];
                } else {
                    n2k = n_o_hoisted[k];
                }
            }
            let ratio_k = n1k / n2k;
            let sin2_t_k = (ratio_k * ratio_k) * fma(-cos_i, cos_i, 1.0);
            if (sin2_t_k > 1.0) {
                // Chromatic termination: channel k cannot transmit at this angle, though the
                // hero can. Its Stokes and `path_pdf` go to exactly 0, not just down-weighted.
                (*s).stokes[k] = (*s).stokes[k] * 0.0;
                (*s).path_pdf[k] = 0.0;
                continue;
            }
            let cos_t_k = sqrt(max(1.0 - sin2_t_k, 0.0));
            let refr_wave_dir_k = normalize(
                ratio_k * wave_dir_at_bounce + fma(ratio_k, cos_i, -cos_t_k) * normal,
            );
            // Compare against the stored `final_refr_dir`, not a recomputed hero direction. A
            // recomputation differs by a few ULP and would terminate the hero channel against
            // itself. Biaxial: channel k walks off with its own `n_alpha_ch`, `n_beta_ch` and
            // `n_gamma_ch`, at its own refracted wave direction. Uniaxial extraordinary:
            // `extraordinary_poynting_dir` below.
            var final_dir_k = refr_wave_dir_k;
            if (entering_anisotropic && is_biaxial) {
                final_dir_k = biaxial_mode_poynting_dir(n_alpha_ch[k], n_beta_ch[k], n_gamma_ch[k], biax_ax0, biax_ax1, biax_ax2, refr_wave_dir_k, use_extraordinary);
            } else if (entering_anisotropic && use_extraordinary) {
                // The channel's true n_e(lambda) (`bounce_index_setup`'s `n_e_raw_ch`:
                // the extraordinary curve when the material carries one, else
                // `n_o + birefringence_delta`, the identical f32 sum).
                let n_e_k = idx.n_e_raw_ch[k];
                final_dir_k = extraordinary_poynting_dir(refr_wave_dir_k, c_axis, n_o_hoisted[k], n_e_k);
            }
            scalar_dirs[k] = final_dir_k;
            scalar_dirs_valid[k] = true;
            let direction_matches = directions_match(final_dir_k, final_refr_dir);
            scalar_hero_match[k] = direction_matches;
            if (direction_matches) {
                let t_s_k = (2.0 * n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
                let t_p_k = (2.0 * n1k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
                let trans = mueller_fresnel_transmission(n1k, n2k, cos_i, cos_t_k, t_s_k, t_p_k);
                // Project the channel's incident Stokes onto the selected eigenmode before
                // transmission (see `entry_eigenmode_selection`). The result is fully linear
                // along the mode axis (`w` and `V` zeroed), at the channel's unscaled intensity.
                // `entry_valid == 0u` (biaxial, or negligible linear polarization) skips the
                // projection.
                var incident_k = (*s).stokes[k];
                if (entering_anisotropic && entry_valid != 0u) {
                    let i_k = (*s).stokes[k].x;
                    incident_k = vec4<f32>(i_k, i_k * entry_cos_2psi_x, i_k * entry_sin_2psi_x, 0.0);
                }
                // No division by the selection probability (see above).
                (*s).stokes[k] = (trans * incident_k) * (1.0 / (1.0 - r_t));

                let r_s_k = fma(n2k, -cos_t_k, n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
                let r_p_k = fma(n1k, -cos_t_k, n2k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
                let r_unpol_k = clamp(0.5 * fma(r_p_k, r_p_k, r_s_k * r_s_k), R_UNPOL_MIN, R_UNPOL_MAX);
                // No extra factor: a uniform per-channel factor cancels in the MIS weight. See
                // refraction.rs.
                if (!collect_on) {
                    (*s).path_pdf[k] = (*s).path_pdf[k] * (1.0 - r_unpol_k);
                }
            } else {
                // Chromatic termination. The Stokes contribution is lost either way, while
                // `path_pdf` keeps accumulating this event's transmit factor. At an exit event
                // the channel also stages its transmitted radiance along its refracted direction
                // for `try_split_exit_channel`. See the `else` branch of `apply_refract_channel`.
                let prefix_path_pdf_k = (*s).path_pdf[k];
                (*s).stokes[k] = (*s).stokes[k] * 0.0;
                (*s).path_pdf[k] = 0.0;

                let azimuth_valid_here = entering_anisotropic && entry_valid != 0u;
                let ct = compute_channel_transmission(
                    n1k, n2k, cos_i, cos_t_k, r_unpol, entering_anisotropic,
                    azimuth_valid_here, entry_cos_2psi_x, entry_sin_2psi_x, original_stokes_k,
                );
                (*s).path_pdf[k] = prefix_path_pdf_k * (1.0 - ct.r_unpol_k);
                if (collect_on) {
                    // No branch factor under exit collection (`restore_densities`).
                    (*s).path_pdf[k] = prefix_path_pdf_k;
                }
                if (is_exit_event && original_stokes_k.x > 0.0) {
                    (*h).split_mask = (*h).split_mask | (1u << k);
                    (*h).split_dir[k] = vec4<f32>(refr_wave_dir_k, ct.transmitted.x);
                    (*h).split_mis_weight = split_mis_weight;
                }
            }
        }
        // An interior dispersive event (entry) narrows every channel's MIS family. An exit never
        // does. See `narrow_compat` in `transport_physics.wgsl`.
        if (!(*s).inside_gem) {
            var local_compat = (*s).compat;
            narrow_compat(&local_compat, scalar_dirs, scalar_dirs_valid, scalar_hero_match);
            (*s).compat = local_compat;
        }
        (*s).origin = hit_point + final_refr_dir * RAY_EPS;
        (*s).dir = final_refr_dir;
        (*s).k = new_k;
        // `inside_gem` still has its pre-bounce value here (flipped below). A live incoming NEE
        // carry passes through unchanged, as `dispatch_bounce` does for this generic exit.
        if ((*s).inside_gem) {
            (*s).pending_light_mis = phase_pdf_this_check;
            (*s).pending_light_mis_dir = phase_dir_this_check;
        }
        (*s).inside_gem = !(*s).inside_gem;
        if (entering_anisotropic) {
            (*s).is_extraordinary = use_extraordinary;
        }
    }
    return PHASE_NEXT;
}

