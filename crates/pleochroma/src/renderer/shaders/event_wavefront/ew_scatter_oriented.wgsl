// Interior scatter decision of a stone with oriented inclusions or a domain structure (opal,
// labradorite), `material_oriented_active()`. This is `ew_scatter` as it was before the plain haze
// was split off it: the same shared pieces in the same order (alphas -> sample -> park for the HG
// NEE or commit), so the rendered result is unchanged; only the dispatch partition differs.
// `ew_extend` routes such a stone here by `TAG_SCATTER_ORIENTED`; the kernel is in the dispatch
// list only when the pipeline keeps the oriented estimators (`kernels_for`: class bit
// `NO_ORIENTED` clear), so the browser, which never renders oriented inclusions, does not compile
// it.
//
// With an HDR environment the scatter-point NEE needs five more storage buffers than this kernel
// may bind (plan 4.3: at most 8), so the event is parked on the ray and `ew_nee_hg` finishes it
// exactly as for the plain haze. Parked state: `prev_plane_normal` holds the scatter point and
// `pending_mis_dir.xyz` the sampled outgoing direction.
@compute @workgroup_size(64)
fn ew_scatter_oriented(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    ew_fill_planes_shared(local_index);
    let i = gid.x;
    if (i >= ew.chunk_rays) {
        return;
    }
    let core = ray_core[i];
    if (core.ray_meta.z != TAG_SCATTER_ORIENTED) {
        return;
    }

    var s = ew_load(i);
    var h = ew_hit_load(i);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let seed0 = core.ray_meta.y;

    var tag: u32 = TAG_SURFACE;
    if (bounce_scatter_applies(&s)) {
        var alphas: array<f32, 8>;
        bounce_scatter_alphas(&s, rc, &alphas);
        let ev = bounce_scatter_sample(&s, &h, rc, ew.bounce, seed0, alphas);
        if (ev.scattered != 0u) {
            // A Bragg reflection (`ev.bragg`) takes no scattering-point NEE and leaves no MIS
            // carry, so it is never parked: it commits here, in this kernel.
            if (SCATTER_NEE && params.env_mode == 2u && scatter_nee_zone_free() && ev.bragg == 0u) {
                s.prev_plane_normal = ev.scatter_point;
                s.pending_light_mis_dir = ev.new_dir;
                tag = TAG_NEE_HG;
            } else {
                let r = bounce_scatter_commit(&s, ew.bounce, seed0, ev);
                tag = select(TAG_EXTEND, TAG_FINALIZE, r == PHASE_TERMINATE);
            }
        }
    }
    ew_store(i, s);
    ew_hit_store(i, h);
    ray_core[i].ray_meta.z = tag;
}
