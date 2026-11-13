// Interior absorption of a zoned stone: applies the per-zone absorption of the segment the ray
// travelled inside the stone. It is the call `bounce_surface` makes in place, split into its own
// kernel so that `ew_surface` stays free of zone code. It runs after the surface kernel and
// before the arm kernels, on the `stokes` the surface phase left. The segment is the stored ray
// (`origin`, `dir`, wave normal `k`) up to `hit.t`. The arm chosen by the surface kernel waits in
// the hit record (`misc.w`), so only that word of `ray_hit` is read.
//
// Dispatched only for stones with zones (`with_zoned_absorb`). The browser build has no zones
// and never compiles this kernel.
@compute @workgroup_size(64)
fn ew_zoned_absorb(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= ew.chunk_rays) {
        return;
    }
    let core = ray_core[i];
    if (core.ray_meta.z != TAG_ZONE_ABSORB) {
        return;
    }

    var s = ew_load(i);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let hit_t = ray_hit[i].hit_point.w;

    var local_lambdas = s.lambdas;
    var local_stokes = s.stokes;
    zoned_interior_absorption(
        hit_t, s.origin, s.dir, s.k, &local_lambdas,
        rc.is_anisotropic, rc.is_biaxial, rc.c_axis, rc.n_alpha_hero, rc.n_beta_hero, rc.n_gamma_hero,
        rc.biax_ax0, rc.biax_ax1, rc.biax_ax2, rc.n_o_hero_seed, rc.n_e_hero_seed, s.is_extraordinary,
        rc.alpha_o_hoisted, rc.alpha_e_hoisted, rc.alpha_beta_hoisted, &local_stokes,
    );
    s.stokes = local_stokes;
    ew_store(i, s);

    ray_core[i].ray_meta.z = TAG_ARM_BASE + ray_hit[i].misc.w;
}
