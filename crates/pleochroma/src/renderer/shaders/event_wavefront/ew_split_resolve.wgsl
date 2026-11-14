@compute @workgroup_size(64)
// No shared copy of the planes: the only plane reads are the exit splits' re-entry probes,
// which read the binding (`try_split_exit_channel`).
fn ew_split_resolve(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= ew.chunk_rays) {
        return;
    }
    let core = ray_core[i];
    if (core.ray_meta.z != TAG_SPLIT) {
        return;
    }

    var s = ew_load(i);
    var h = ew_hit_load(i);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let observer = core.observer.xyz;

    let r = bounce_resolve_splits(&s, &h, rc, observer);
    ew_store(i, s);
    ew_hit_store(i, h);

    ray_core[i].ray_meta.z = TAG_TAIL;
}
