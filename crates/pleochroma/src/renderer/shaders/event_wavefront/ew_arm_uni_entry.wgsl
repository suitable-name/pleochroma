// Uniaxial entry arm, reflect half. `bounce_arm_uni_entry_core` (08d) has four Lekner solver call
// sites, which made it the slowest arm to compile in the browser. The event wavefront runs it as
// two kernels along its one real branch, with every operation unchanged and in the same order:
//
// * this kernel solves the hero's entry pair, draws the reflect-vs-transmit decision
//   (`entry_rng_bounce < entry_r_branch`) and, on the reflect branch, runs the per-channel reflect
//   loop and the surface-finish correction. On the transmit branch it changes nothing and parks
//   the ray (`TAG_UNI_ENTRY_TRANSMIT`);
// * `ew_arm_uni_entry_transmit` repeats the (pure) hero solve and runs the transmit branch.
//
// Nothing is written before the decision in the original either, so a parked ray is exactly the
// ray the original function started from. The reflect loop reuses the hero's solution for channel
// 0 (PERF-11), so each kernel has two solver call sites.

// The reflect branch of `bounce_arm_uni_entry_core`. Returns true, leaving `s` and `h` untouched,
// when the transmit branch was drawn instead.
fn ew_uni_entry_reflect(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
) -> bool {
    let idx = bounce_index_setup(s, h, rc);
    let hit_point = (*h).hit_point;
    let normal = (*h).shading_normal;
    let wave_dir_at_bounce = (*h).wave_dir_at_bounce;
    let c_axis = rc.c_axis;
    let n_o_hoisted = rc.n_o_hoisted;
    let uframe = idx.uframe;
    let uinc_frame = idx.uinc_frame;
    let n_o_hero = idx.n_o_hero;
    let n_e_hero_solve = idx.n_e_raw_ch[0];
    let n_e_raw_ch = idx.n_e_raw_ch;

    // ---- ENTRY: mirrors apply_uniaxial_entry_bounce ----
    let pair_hero = entry_solve_pair_with_incidence(uinc_frame, 1.0, n_o_hero, n_e_hero_solve, c_axis, uframe);
    let sol_s_hero = pair_hero.s_sol;
    let sol_p_hero = pair_hero.p_sol;
    let reflect_mueller_hero = jones_to_mueller(sol_s_hero.r_s, sol_p_hero.r_s, sol_s_hero.r_p, sol_p_hero.r_p);
    let entry_r_branch = clamp(reflect_mueller_hero[0][0], R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
    let entry_rng_bounce = f32(hash_u32(seed0 ^ hash_u32(bounce ^ FRESNEL_BRANCH_STREAM))) / 4294967295.0;

    if (!(entry_rng_bounce < entry_r_branch)) {
        return true;
    }
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        // Channel 0 is the hero: `pair_hero` was solved with the identical
        // arguments (`n_o_hero == n_o_hoisted[0]`, `n_e_hero_solve ==
        // n_e_raw_ch[0]`), so it is reused (PERF-11).
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
    return false;
}

@compute @workgroup_size(64)
fn ew_arm_uni_entry(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= ew.chunk_rays) {
        return;
    }
    let core = ray_core[i];
    if (core.ray_meta.z != TAG_ARM_BASE + ARM_UNI_ENTRY) {
        return;
    }

    var s = ew_load(i);
    var h = ew_hit_load(i);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let seed0 = core.ray_meta.y;

    if (ew_uni_entry_reflect(&s, &h, rc, ew.bounce, seed0)) {
        // The transmit branch: nothing was changed, `ew_arm_uni_entry_transmit` takes the ray.
        ray_core[i].ray_meta.z = TAG_UNI_ENTRY_TRANSMIT;
        return;
    }
    surface_finish_event(&s, &h, rc, ew.bounce, seed0);
    ew_store(i, s);
    ew_hit_store(i, h);

    // The reflect branch ends the bounce step like the original arm's `PHASE_NEXT`; the exit-split
    // mask is always empty here, but the routing is the other arms' routing.
    if (h.split_mask != 0u) {
        ray_core[i].ray_meta.z = TAG_SPLIT;
    } else {
        ray_core[i].ray_meta.z = TAG_TAIL;
    }
}
