// The four non-frosted arms, each followed by the surface-finish correction of a polish with
// roughness (`surface_finish_event`, `08c_surface.wgsl`). The correction is a no-op for every
// other event, so an ideal scene is untouched. The arms themselves are the `*_core` functions;
// the megakernel's dispatcher below and the event wavefront's arm kernels both call these
// wrappers.
fn bounce_arm_tir(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    let r = bounce_arm_tir_core(s, h, rc, bounce, seed0, observer);
    surface_finish_event(s, h, rc, bounce, seed0);
    return r;
}

fn bounce_arm_uni_entry(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    let r = bounce_arm_uni_entry_core(s, h, rc, bounce, seed0, observer);
    surface_finish_event(s, h, rc, bounce, seed0);
    return r;
}

fn bounce_arm_uni_internal(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    let r = bounce_arm_uni_internal_core(s, h, rc, bounce, seed0, observer);
    surface_finish_event(s, h, rc, bounce, seed0);
    return r;
}

fn bounce_arm_generic(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    let r = bounce_arm_generic_core(s, h, rc, bounce, seed0, observer);
    surface_finish_event(s, h, rc, bounce, seed0);
    return r;
}

// Arm dispatcher based on h.arm selected in surface phase.
fn bounce_run_arm(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    if ((*h).arm == ARM_FROSTED) {
        return bounce_arm_frosted(s, h, rc, bounce, seed0, observer);
    } else if ((*h).arm == ARM_TIR) {
        return bounce_arm_tir(s, h, rc, bounce, seed0, observer);
    } else if ((*h).arm == ARM_UNI_ENTRY) {
        return bounce_arm_uni_entry(s, h, rc, bounce, seed0, observer);
    } else if ((*h).arm == ARM_UNI_INTERNAL) {
        return bounce_arm_uni_internal(s, h, rc, bounce, seed0, observer);
    } else if ((*h).arm == ARM_GENERIC) {
        return bounce_arm_generic(s, h, rc, bounce, seed0, observer);
    }
    return PHASE_NEXT;
}

// Resolve deferred exit splits.
fn bounce_resolve_splits(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    observer: vec3<f32>,
) -> u32 {
    let mask = (*h).split_mask;
    if (mask != 0u) {
        var local_split_radiance = (*s).split_radiance;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            if ((mask & (1u << k)) != 0u) {
                // Exit collection: the slot holds a collected leaf, not a staged split. It is
                // resolved through the same lookup into an emptied slot and added to `nee_xyz`
                // at once as a plain comb term (`integrate_collected_to_xyz`): a leaf is final,
                // whatever happens to the path afterwards.
                let staged_k = local_split_radiance[k];
                if (collect_on) {
                    local_split_radiance[k] = 0.0;
                }
                try_split_exit_channel(
                    &local_split_radiance,
                    (*h).hit_point,
                    k,
                    (*s).lambdas[k],
                    (*h).split_dir[k].xyz,
                    (*h).split_dir[k].w,
                    rc.studio_key_dir,
                    rc.studio_fill_dir,
                    rc.studio_sin_lp,
                    observer,
                    (*h).split_mis_weight,
                );
                if (collect_on) {
                    (*s).nee_xyz = (*s).nee_xyz
                        + cie_1931_cmf((*s).lambdas[k]) * (local_split_radiance[k] * NORM_FACTOR);
                    local_split_radiance[k] = staged_k;
                }
            }
        }
        (*s).split_radiance = local_split_radiance;
    }
    return PHASE_NEXT;
}

// Tail phase: surface glare and Russian roulette.
fn bounce_tail(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
) -> u32 {
    // Surface glare: at the camera path's first event on a polished facet the ray is outside
    // the stone, so only the specular reflection arrives. Skipped at 1.0 and for frosted
    // facets, which keeps those cases bit-identical.
    if (bounce == 0u && !(*s).inside_gem && !surf_is_ground((*h).finish) && params.surface_glare < 1.0) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            (*s).stokes[k] = (*s).stokes[k] * params.surface_glare;
        }
    }

    if (collect_on) {
        // Exit collection (`apply_collect_roulette`): the chain's weights are physical, so
        // nothing happens while the largest channel intensity is at least the floor; below it
        // the path survives with probability `intensity / floor`, at every bounce.
        var collect_max: f32 = 0.0;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            collect_max = max(collect_max, max((*s).stokes[k].x, 0.0));
        }
        if (collect_max >= COLLECT_ROULETTE_FLOOR) {
            return PHASE_NEXT;
        }
        let collect_q = collect_max / COLLECT_ROULETTE_FLOOR;
        let collect_rand = f32(discrete_draw(seed0, bounce, RUSSIAN_ROULETTE_STREAM)) / 4294967295.0;
        if (collect_q <= 0.0 || collect_rand > collect_q) {
            return PHASE_TERMINATE;
        }
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            (*s).stokes[k] = (*s).stokes[k] * (1.0 / collect_q);
            (*s).split_radiance[k] = (*s).split_radiance[k] / collect_q;
        }
        return PHASE_NEXT;
    }

    if (bounce > 4u) {
        var max_intensity: f32 = 0.0;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            max_intensity = max(max_intensity, max((*s).stokes[k].x, 0.0));
        }
        let q = clamp(max_intensity, RR_FLOOR, 1.0);
        let rr_rand = f32(discrete_draw(seed0, bounce, RUSSIAN_ROULETTE_STREAM)) / 4294967295.0;
        if (rr_rand > q) {
            return PHASE_TERMINATE;
        }
        // `split_radiance` gets the same `1/q` rescale as `stokes`, as in CPU
        // `apply_russian_roulette`.
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            (*s).stokes[k] = (*s).stokes[k] * (1.0 / q);
            (*s).split_radiance[k] = (*s).split_radiance[k] / q;
        }
    }

    return PHASE_NEXT;
}

// One bounce of the shared transport bounce loop. Composition of the phase functions.
fn transport_bounce_step(
    bounce: u32,
    seed0: u32,
    lambdas: ptr<function, array<f32, 8>>,
    c_axis: vec3<f32>,
    birefringence_delta: f32,
    is_anisotropic: bool,
    is_biaxial: bool,
    biax_ax0: vec3<f32>,
    biax_ax1: vec3<f32>,
    biax_ax2: vec3<f32>,
    n_o_hero_seed: f32,
    n_beta_hero: f32,
    n_alpha_hero: f32,
    n_gamma_hero: f32,
    n_e_hero_seed: f32,
    n_o_hoisted: array<f32, 8>,
    alpha_o_hoisted: array<f32, 8>,
    alpha_e_hoisted: array<f32, 8>,
    alpha_beta_hoisted: array<f32, 8>,
    studio_key_dir: vec3<f32>,
    studio_fill_dir: vec3<f32>,
    studio_sin_lp: f32,
    observer: vec3<f32>,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
    current_origin: ptr<function, vec3<f32>>,
    current_dir: ptr<function, vec3<f32>>,
    current_k: ptr<function, vec3<f32>>,
    inside_gem: ptr<function, bool>,
    is_extraordinary: ptr<function, bool>,
    prev_plane_normal: ptr<function, vec3<f32>>,
    have_prev_plane_normal: ptr<function, bool>,
    split_radiance: ptr<function, array<f32, 8>>,
    compat: ptr<function, array<u32, 8>>,
    nee_xyz: ptr<function, vec3<f32>>,
    path_escaped: ptr<function, bool>,
    pending_light_mis: ptr<function, f32>,
    pending_light_mis_dir: ptr<function, vec3<f32>>,
) -> u32 {
    var s = bounce_ray_from_pointers(
        current_origin, current_dir, current_k, prev_plane_normal, pending_light_mis_dir, nee_xyz,
        pending_light_mis, inside_gem, is_extraordinary, have_prev_plane_normal, path_escaped,
        lambdas, stokes, radiance, path_pdf, split_radiance, compat,
    );
    var h = bounce_hit_blank();
    var rc: RayConstants;
    rc.c_axis = c_axis;
    rc.birefringence_delta = birefringence_delta;
    rc.is_anisotropic = is_anisotropic;
    rc.is_biaxial = is_biaxial;
    rc.biax_ax0 = biax_ax0;
    rc.biax_ax1 = biax_ax1;
    rc.biax_ax2 = biax_ax2;
    rc.n_o_hero_seed = n_o_hero_seed;
    rc.n_beta_hero = n_beta_hero;
    rc.n_alpha_hero = n_alpha_hero;
    rc.n_gamma_hero = n_gamma_hero;
    rc.n_e_hero_seed = n_e_hero_seed;
    rc.n_o_hoisted = n_o_hoisted;
    rc.alpha_o_hoisted = alpha_o_hoisted;
    rc.alpha_e_hoisted = alpha_e_hoisted;
    rc.alpha_beta_hoisted = alpha_beta_hoisted;
    rc.studio_key_dir = studio_key_dir;
    rc.studio_fill_dir = studio_fill_dir;
    rc.studio_sin_lp = studio_sin_lp;

    var r = bounce_extend(&s, &h, rc, bounce, seed0, observer);
    if (r == PHASE_NEXT) { r = bounce_scatter(&s, &h, rc, bounce, seed0, observer); }
    if (r == PHASE_NEXT) { r = bounce_surface(&s, &h, rc, bounce, seed0, observer); }
    if (r == PHASE_NEXT) { r = bounce_run_arm(&s, &h, rc, bounce, seed0, observer); }
    if (r == PHASE_NEXT) { r = bounce_resolve_splits(&s, &h, rc, observer); }
    if (r == PHASE_NEXT) { r = bounce_tail(&s, &h, rc, bounce, seed0); }

    bounce_ray_to_pointers(
        s, current_origin, current_dir, current_k, prev_plane_normal, pending_light_mis_dir, nee_xyz,
        pending_light_mis, inside_gem, is_extraordinary, have_prev_plane_normal, path_escaped,
        stokes, radiance, path_pdf, split_radiance, compat,
    );

    if (r == PHASE_TERMINATE) {
        return BOUNCE_STATUS_TERMINATE;
    }
    return BOUNCE_STATUS_CONTINUE;
}

// First half of a bounce in the gated loop (`GATED_LOOP`, `transport_main`): extend, scatter
// and surface, as `transport_bounce_step` runs them. A total internal reflection finishes
// the bounce here. Any other arm is parked: the bounce copies what the arm needs into `park`,
// writes the ray back and returns `BOUNCE_STATUS_PARKED`. This lets the parked lanes of a
// warp reach `transport_bounce_back` together. `rc` carries the per-ray constants that
// `transport_bounce_step` takes as separate arguments.
fn transport_bounce_front(
    bounce: u32,
    seed0: u32,
    rc: RayConstants,
    observer: vec3<f32>,
    lambdas: ptr<function, array<f32, 8>>,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
    current_origin: ptr<function, vec3<f32>>,
    current_dir: ptr<function, vec3<f32>>,
    current_k: ptr<function, vec3<f32>>,
    inside_gem: ptr<function, bool>,
    is_extraordinary: ptr<function, bool>,
    prev_plane_normal: ptr<function, vec3<f32>>,
    have_prev_plane_normal: ptr<function, bool>,
    split_radiance: ptr<function, array<f32, 8>>,
    compat: ptr<function, array<u32, 8>>,
    nee_xyz: ptr<function, vec3<f32>>,
    path_escaped: ptr<function, bool>,
    pending_light_mis: ptr<function, f32>,
    pending_light_mis_dir: ptr<function, vec3<f32>>,
    park: ptr<function, BouncePark>,
) -> u32 {
    var s = bounce_ray_from_pointers(
        current_origin, current_dir, current_k, prev_plane_normal, pending_light_mis_dir, nee_xyz,
        pending_light_mis, inside_gem, is_extraordinary, have_prev_plane_normal, path_escaped,
        lambdas, stokes, radiance, path_pdf, split_radiance, compat,
    );
    var h = bounce_hit_blank();

    var r = bounce_extend(&s, &h, rc, bounce, seed0, observer);
    if (r == PHASE_NEXT) { r = bounce_scatter(&s, &h, rc, bounce, seed0, observer); }
    if (r == PHASE_NEXT) { r = bounce_surface(&s, &h, rc, bounce, seed0, observer); }
    var parked = false;
    if (r == PHASE_NEXT) {
        if (h.arm != ARM_TIR) {
            (*park).pending = true;
            (*park).hit = h.hit;
            (*park).hit_point = h.hit_point;
            (*park).shading_normal = h.shading_normal;
            (*park).wave_dir_at_bounce = h.wave_dir_at_bounce;
            (*park).phase_pdf_this_check = h.phase_pdf_this_check;
            (*park).phase_dir_this_check = h.phase_dir_this_check;
            (*park).pre_bounce_inside_gem = h.pre_bounce_inside_gem;
            (*park).finish = h.finish;
            (*park).arm = h.arm;
            parked = true;
        } else {
            let r_arm = bounce_run_arm(&s, &h, rc, bounce, seed0, observer);
            r = r_arm;
            if (r == PHASE_NEXT) { r = bounce_resolve_splits(&s, &h, rc, observer); }
            if (r == PHASE_NEXT) { r = bounce_tail(&s, &h, rc, bounce, seed0); }
        }
    }

    bounce_ray_to_pointers(
        s, current_origin, current_dir, current_k, prev_plane_normal, pending_light_mis_dir, nee_xyz,
        pending_light_mis, inside_gem, is_extraordinary, have_prev_plane_normal, path_escaped,
        stokes, radiance, path_pdf, split_radiance, compat,
    );

    if (parked) {
        return BOUNCE_STATUS_PARKED;
    }
    if (r == PHASE_TERMINATE) {
        return BOUNCE_STATUS_TERMINATE;
    }
    return BOUNCE_STATUS_CONTINUE;
}

// The gated megakernel loop's second half of a bounce: the arm, the deferred exit splits and the
// tail of the bounce `transport_bounce_front` parked. Rebuilds the `BounceHit` from `park`; the
// arm recomputes its index setup (`bounce_arm_index`). Returns `BOUNCE_STATUS_CONTINUE` or
// `BOUNCE_STATUS_TERMINATE`, as `transport_bounce_step` does.
fn transport_bounce_back(
    bounce: u32,
    seed0: u32,
    rc: RayConstants,
    observer: vec3<f32>,
    lambdas: ptr<function, array<f32, 8>>,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
    current_origin: ptr<function, vec3<f32>>,
    current_dir: ptr<function, vec3<f32>>,
    current_k: ptr<function, vec3<f32>>,
    inside_gem: ptr<function, bool>,
    is_extraordinary: ptr<function, bool>,
    prev_plane_normal: ptr<function, vec3<f32>>,
    have_prev_plane_normal: ptr<function, bool>,
    split_radiance: ptr<function, array<f32, 8>>,
    compat: ptr<function, array<u32, 8>>,
    nee_xyz: ptr<function, vec3<f32>>,
    path_escaped: ptr<function, bool>,
    pending_light_mis: ptr<function, f32>,
    pending_light_mis_dir: ptr<function, vec3<f32>>,
    park: BouncePark,
) -> u32 {
    var s = bounce_ray_from_pointers(
        current_origin, current_dir, current_k, prev_plane_normal, pending_light_mis_dir, nee_xyz,
        pending_light_mis, inside_gem, is_extraordinary, have_prev_plane_normal, path_escaped,
        lambdas, stokes, radiance, path_pdf, split_radiance, compat,
    );
    var h = bounce_hit_blank();
    h.hit = park.hit;
    h.hit_point = park.hit_point;
    h.shading_normal = park.shading_normal;
    h.wave_dir_at_bounce = park.wave_dir_at_bounce;
    h.phase_pdf_this_check = park.phase_pdf_this_check;
    h.phase_dir_this_check = park.phase_dir_this_check;
    h.pre_bounce_inside_gem = park.pre_bounce_inside_gem;
    h.finish = park.finish;
    h.arm = park.arm;

    let r_arm = bounce_run_arm(&s, &h, rc, bounce, seed0, observer);
    var r = r_arm;
    if (r == PHASE_NEXT) { r = bounce_resolve_splits(&s, &h, rc, observer); }
    if (r == PHASE_NEXT) { r = bounce_tail(&s, &h, rc, bounce, seed0); }

    bounce_ray_to_pointers(
        s, current_origin, current_dir, current_k, prev_plane_normal, pending_light_mis_dir, nee_xyz,
        pending_light_mis, inside_gem, is_extraordinary, have_prev_plane_normal, path_escaped,
        stokes, radiance, path_pdf, split_radiance, compat,
    );

    if (r == PHASE_TERMINATE) {
        return BOUNCE_STATUS_TERMINATE;
    }
    return BOUNCE_STATUS_CONTINUE;
}
