// Uniaxial internal arm, reflect half. The hero's solve and the reflect-vs-transmit draw, and on
// the reflect branch the arm and the surface-finish correction. A ray that draws the transmit
// branch is parked unchanged (`TAG_UNI_INTERNAL_TRANSMIT`) for `ew_arm_uni_internal_transmit`.
@compute @workgroup_size(64)
fn ew_arm_uni_internal(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= ew.chunk_rays) {
        return;
    }
    let core = ray_core[i];
    if (core.ray_meta.z != TAG_ARM_BASE + ARM_UNI_INTERNAL) {
        return;
    }

    var s = ew_load(i);
    var h = ew_hit_load(i);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let seed0 = core.ray_meta.y;
    let observer = core.observer.xyz;

    let r = bounce_arm_uni_internal_branches(&s, &h, rc, ew.bounce, seed0, observer, true, false);
    if (r == PHASE_PARK) {
        // The transmit branch: nothing was changed, `ew_arm_uni_internal_transmit` takes the ray.
        ray_core[i].ray_meta.z = TAG_UNI_INTERNAL_TRANSMIT;
        return;
    }
    surface_finish_event(&s, &h, rc, ew.bounce, seed0);
    ew_store(i, s);
    ew_hit_store(i, h);

    if (r == PHASE_TERMINATE) {
        ray_core[i].ray_meta.z = TAG_FINALIZE;
    } else if (h.split_mask != 0u) {
        ray_core[i].ray_meta.z = TAG_SPLIT;
    } else {
        ray_core[i].ray_meta.z = TAG_TAIL;
    }
}
