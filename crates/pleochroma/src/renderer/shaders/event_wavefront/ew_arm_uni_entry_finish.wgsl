// The surface-finish correction of the uniaxial entry's transmit half, then the arms' routing. It
// is split off `ew_arm_uni_entry_transmit`, which the Adreno 740 driver does not link with it:
// that kernel stores the ray after the arm (`TAG_UNI_ENTRY_FINISH`) and this one goes on from the
// stored ray, which holds everything the correction reads, so the numbers are the same.
@compute @workgroup_size(64)
fn ew_arm_uni_entry_finish(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= ew.chunk_rays) {
        return;
    }
    let core = ray_core[i];
    if (core.ray_meta.z != TAG_UNI_ENTRY_FINISH) {
        return;
    }

    var s = ew_load(i);
    var h = ew_hit_load(i);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let seed0 = core.ray_meta.y;

    surface_finish_event(&s, &h, rc, ew.bounce, seed0);
    ew_store(i, s);
    ew_hit_store(i, h);

    if (h.split_mask != 0u) {
        ray_core[i].ray_meta.z = TAG_SPLIT;
    } else {
        ray_core[i].ray_meta.z = TAG_TAIL;
    }
}
