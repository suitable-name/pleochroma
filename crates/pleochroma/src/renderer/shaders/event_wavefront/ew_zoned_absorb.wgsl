// Interior absorption of a zoned stone: the per-zone absorption of the segment the ray just
// travelled inside the stone, split off `ew_surface` (see its header). It runs between the surface
// and the arm kernels in the same bounce round, and applies exactly the call `bounce_surface`
// makes in place (`zoned_interior_absorption`, patched into 08c by build.rs) on the same
// `stokes` that the surface phase's frame rotation left: the segment is the stored ray
// (`origin`, `dir`, wave normal `k`), its length `hit.t` the `w` of the hit point. The arm was
// chosen by the surface kernel and waits in the hit record (`misc.w`), so only that word of
// `ray_hit` is read.
//
// In the dispatch list only for a stone with zones (`with_zoned_absorb`); the browser refuses
// zoned stones, so it never compiles this kernel.
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
