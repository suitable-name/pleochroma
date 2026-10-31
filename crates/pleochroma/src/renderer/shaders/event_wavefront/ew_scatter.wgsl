// Interior scatter decision of the plain haze (`scattering_sigma_s > 0`, Henyey-Greenstein). A
// stone with oriented inclusions or a domain structure scatters in `ew_scatter_oriented` instead
// (`ew_extend` routes by tag), so nothing reachable from this kernel is an oriented, Bragg or
// zoned estimator: that code is what made this kernel the slowest to compile in the browser.
//
// `ew_scatter_sample_plain` and `ew_scatter_commit_plain` are the non-oriented tails of the shared
// `bounce_scatter_sample` and `bounce_scatter_commit` (08b): the same statements in the same
// order, without the `material_oriented_active()` branches that are false for every stone that
// reaches this kernel. The event-vs-megakernel equivalence tests keep them in step.
//
// With an HDR environment the scatter-point NEE needs five more storage buffers than this kernel
// may bind (plan 4.3: at most 8), so the event is parked on the ray and `ew_nee_hg` finishes it:
// NEE first, then the same commit (new direction + roulette) the megakernel runs. Parked state:
// `prev_plane_normal` holds the scatter point and `pending_mis_dir.xyz` the sampled outgoing
// direction (both are dead until the commit rewrites them: `have_prev_plane_normal` is cleared
// there, and the MIS carry is set by the NEE).

// `bounce_scatter_sample` for a stone without oriented inclusions: Beer-Lambert survival and the
// scatter decision. Updates stokes and path_pdf; leaves origin, dir and k untouched.
fn ew_scatter_sample_plain(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    bounce: u32,
    seed0: u32,
    alphas: array<f32, 8>,
) -> BounceScatterEvent {
    var local_stokes = (*s).stokes;
    var local_path_pdf = (*s).path_pdf;
    let sc = maybe_scatter_or_extinguish(
        alphas, material.scattering_sigma_s, material.scattering_g, (*s).dir, (*h).hit.t,
        material.absorption_path_scale, seed0, bounce, &local_stokes, &local_path_pdf,
    );
    (*s).stokes = local_stokes;
    (*s).path_pdf = local_path_pdf;
    return BounceScatterEvent(sc.scattered, (*s).origin + sc.t_free * (*s).dir, sc.new_dir, 0u);
}

// `bounce_scatter_commit` for a stone without oriented inclusions: moves the ray to the scatter
// point along the sampled direction, then Russian roulette.
fn ew_scatter_commit_plain(
    s: ptr<function, BounceRay>,
    bounce: u32,
    seed0: u32,
    ev: BounceScatterEvent,
) -> u32 {
    (*s).origin = ev.scatter_point;
    (*s).dir = ev.new_dir;
    (*s).k = ev.new_dir;
    (*s).have_prev_plane_normal = false;

    if (bounce > 4u) {
        var max_intensity: f32 = 0.0;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            max_intensity = max(max_intensity, max((*s).stokes[k].x, 0.0));
        }
        let q = clamp(max_intensity, RR_FLOOR, 1.0);
        let rr_rand = f32(hash_u32(seed0 ^ hash_u32(bounce ^ RUSSIAN_ROULETTE_STREAM))) / 4294967295.0;
        if (rr_rand > q) {
            return PHASE_TERMINATE;
        }
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            (*s).stokes[k] = (*s).stokes[k] * (1.0 / q);
            (*s).split_radiance[k] = (*s).split_radiance[k] / q;
        }
    }
    return PHASE_END_STEP;
}

@compute @workgroup_size(64)
fn ew_scatter(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    ew_fill_planes_shared(local_index);
    let i = gid.x;
    if (i >= ew.chunk_rays) {
        return;
    }
    let core = ray_core[i];
    if (core.ray_meta.z != TAG_SCATTER) {
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
        let ev = ew_scatter_sample_plain(&s, &h, ew.bounce, seed0, alphas);
        if (ev.scattered != 0u) {
            // The plain haze has no Bragg reflection (`ev.bragg` is 0), so every scatter event of
            // an HDR scene is parked for `ew_nee_hg`.
            if (SCATTER_NEE && params.env_mode == 2u && scatter_nee_zone_free() && ev.bragg == 0u) {
                s.prev_plane_normal = ev.scatter_point;
                s.pending_light_mis_dir = ev.new_dir;
                tag = TAG_NEE_HG;
            } else {
                let r = ew_scatter_commit_plain(&s, ew.bounce, seed0, ev);
                tag = select(TAG_EXTEND, TAG_FINALIZE, r == PHASE_TERMINATE);
            }
        }
    }
    ew_store(i, s);
    ew_hit_store(i, h);
    ray_core[i].ray_meta.z = tag;
}
