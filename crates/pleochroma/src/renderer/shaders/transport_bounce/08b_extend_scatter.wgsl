// Extend phase: MIS carry, intersect_ray, miss branch including environment sampling.
fn bounce_extend(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    let phase_pdf_this_check = (*s).pending_light_mis;
    let phase_dir_this_check = (*s).pending_light_mis_dir;
    (*h).phase_pdf_this_check = phase_pdf_this_check;
    (*h).phase_dir_this_check = phase_dir_this_check;
    (*s).pending_light_mis = 0.0;

    let hit = intersect_ray((*s).origin, (*s).dir);
    (*h).hit = hit;
    // Concave stones only (`side` is `HIT_SIDE_NONE` for a stone without tools, so the
    // planar path is untouched): the stretch of ray that ends at this hit is in the
    // material exactly when the hit is an exit. The toggle in the arms agrees with that
    // except when the `1e-4` restart stepped over a second surface within `1e-4` of the one
    // it crossed (a tool's rim on a facet, the crease of two tools): the carve skipped that
    // boundary as the ray's own origin, so the ray is in the other medium and the
    // intersection's own state is the one to trace by. Twin of `run_bounce`'s
    // `self.inside_gem = !entering`; it runs here, before the scatter and surface phases
    // read `inside_gem`, in the megakernel, the old wavefront and `ew_extend` alike (the
    // event wavefront stores the adopted flag with the ray).
    if (hit.hit && hit.side != HIT_SIDE_NONE) {
        (*s).inside_gem = hit.side == HIT_SIDE_EXIT;
    }
    if (!hit.hit) {
        // A primary miss (bounce 0) and an escape at any later bounce take the same
        // environment lookup below, so the backdrop tray (`params.backdrop`, read by the
        // studio arms) looks identical around the stone and through its windows.
        var mis_weight: f32 = 1.0;
        if (phase_pdf_this_check > 0.0 && nee_light_available()) {
            let light_pdf = nee_light_pdf(phase_dir_this_check, rc.studio_key_dir);
            mis_weight = balance_heuristic(phase_pdf_this_check, light_pdf);
        }
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            let env = sample_environment_with_rig((*s).dir, (*s).lambdas[k], rc.studio_key_dir, rc.studio_fill_dir, rc.studio_sin_lp, observer);
            // `max((*stokes)[k].x, 0.0)` clamps `I` to >= 0 before the environment
            // lookup, matching `accumulate_miss_radiance`'s `StokesVector::intensity`
            // on the CPU side -- negative `I` is unphysical on either side.
            (*s).radiance[k] = fma(max((*s).stokes[k].x, 0.0) * mis_weight, env, (*s).radiance[k]);
        }
        // The only site that sets this: staged exit-split contributions commit
        // only when the shared/hero path itself reaches this environment lookup.
        (*s).path_escaped = true;
        return PHASE_TERMINATE;
    }
    return PHASE_NEXT;
}

// Scatter phase, composed of four shared pieces so that the megakernel, the old wavefront and the
// event wavefront run the SAME physics. The event wavefront runs the pieces across two kernels
// when the HG NEE needs the HDR buffers (`ew_scatter` then `ew_nee_hg`), keeping this order:
//   alphas -> sample (Beer-Lambert + scatter decision) -> HG NEE -> commit (new direction + RR).

// One interior scattering decision: whether the free flight ended in a scatter, where, and the
// phase-sampled outgoing direction.
struct BounceScatterEvent {
    scattered: u32,
    scatter_point: vec3<f32>,
    new_dir: vec3<f32>,
    // 1 for a Bragg reflection (opal, labradorite): no scattering-point NEE, no phase MIS
    // carry and no phase ratio, as `maybe_scatter_oriented_bragg`'s `bragg` flag on the CPU.
    // The event wavefront never parks a Bragg event, so `ew_nee_hg` rebuilds it with 0.
    bragg: u32,
}

// True when the stone has oriented inclusions or a domain structure (the encoded block has a
// family or the structure flag): the scatter phase then runs the oriented estimator
// (`maybe_scatter_oriented`) in place of the plain haze's. Constant false in a pipeline built
// with `ORIENTED_INCLUSIONS = false` (a stone without either).
fn material_oriented_active() -> bool {
    return ORIENTED_INCLUSIONS && (material.inclusions.header.x > 0u || material.inclusions.header.y != 0u);
}

// `GemMaterial::scatters()`: the haze is on, or oriented inclusions are. The single gate every
// scattering-aware site reads (instead of `scattering_sigma_s > 0.0` alone).
fn material_scatters() -> bool {
    return material.scattering_sigma_s > 0.0 || material_oriented_active();
}

// The stone's medium for the oriented estimator: the families of the inclusion block, the haze
// that scatters alongside them and the absorption path scale.
fn oriented_medium() -> OrientedMedium {
    return OrientedMedium(
        material.inclusions.header.x,
        material.inclusions.haze.x,
        material.inclusions.haze.y,
        material.absorption_path_scale,
        material.inclusions.fam_dir[0], material.inclusions.fam_dir[1],
        material.inclusions.fam_dir[2], material.inclusions.fam_dir[3],
        material.inclusions.fam_params[0], material.inclusions.fam_params[1],
        material.inclusions.fam_params[2], material.inclusions.fam_params[3],
    );
}

// Scattering-point next-event estimation is off for oriented inclusions in a zoned host (the
// shadow ray's transmittance would need the zone lengths of its own path), as on the CPU.
fn scatter_nee_zone_free() -> bool {
    return !(material_oriented_active() && material.zones.header.zone_count > 0u);
}

// True when the ray is inside a scattering medium and so takes the scatter phase at all.
fn bounce_scatter_applies(s: ptr<function, BounceRay>) -> bool {
    return (*s).inside_gem && material_scatters();
}

// The assigned eigenmode's absorption coefficient of channel `k` at wave normal `k_hat`, for
// an anisotropic material (`rc.is_anisotropic`). `bounce_scatter_alphas` evaluates it at the
// ray's own direction for the segment it travels; `nee_contribution_hg_scatter` evaluates it
// at the SHADOW direction, which is what a phase-sampled continuation in that direction
// pays (`optics::raytracer::scattering::hg_nee::ShadowAlphas::AssignedMode`).
fn scatter_alpha_anisotropic(rc: RayConstants, k: u32, k_hat: vec3<f32>, is_extraordinary: bool) -> f32 {
    if (rc.is_biaxial && material.has_beta_ray != 0u) {
        return assigned_mode_alpha_biaxial(
            rc.alpha_o_hoisted[k], rc.alpha_beta_hoisted[k], rc.alpha_e_hoisted[k],
            rc.n_alpha_hero, rc.n_beta_hero, rc.n_gamma_hero, rc.biax_ax0, rc.biax_ax1, rc.biax_ax2,
            rc.c_axis, k_hat, is_extraordinary,
        );
    }
    return assigned_mode_alpha_uniaxial(
        rc.alpha_o_hoisted[k], rc.alpha_e_hoisted[k], rc.c_axis, k_hat, is_extraordinary,
        rc.n_o_hero_seed, rc.n_e_hero_seed,
    );
}

// The per-channel absorption alphas for this ray's current direction and mode.
fn bounce_scatter_alphas(
    s: ptr<function, BounceRay>,
    rc: RayConstants,
    alphas: ptr<function, array<f32, 8>>,
) {
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        (*alphas)[k] = 0.0;
    }
    if (rc.is_anisotropic) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            (*alphas)[k] = scatter_alpha_anisotropic(rc, k, (*s).k, (*s).is_extraordinary);
        }
    } else {
        let eigen_a = ordinary_eigen_polarization((*s).k, rc.c_axis);
        let eigen_b = extraordinary_eigen_polarization((*s).k, rc.c_axis);
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            (*alphas)[k] = isotropic_channel_alpha(
                rc.alpha_o_hoisted[k], rc.alpha_e_hoisted[k], rc.c_axis, eigen_a, eigen_b,
            );
        }
    }
}

// Beer-Lambert survival and the scatter decision. Updates stokes and path_pdf; leaves origin,
// dir and k untouched (the commit piece moves them).
fn bounce_scatter_sample(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    alphas: array<f32, 8>,
) -> BounceScatterEvent {
    if (material_oriented_active()) {
        return bounce_scatter_sample_oriented(s, h, rc, bounce, seed0, alphas);
    }
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

// The Bragg domain tracker of a stone with a domain structure (opal, labradorite;
// `material.inclusions.header.y != 0u`): the twin of `maybe_scatter_oriented_bragg`, which
// decides the whole segment (a reflection, or the homogeneous event with the null-collision
// ratios applied). `handled != 0u` means it did (`scattered`, `t_free` in model units and
// `new_dir` as `OrientedScatterResult`, `bragg` set for a reflection, with `stokes`/`path_pdf`
// updated in place) and the plain oriented estimator is skipped.
struct BraggTrackResult {
    handled: u32,
    scattered: u32,
    t_free: f32,
    new_dir: vec3<f32>,
    bragg: u32,
}

fn bragg_track(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    bounce: u32,
    seed0: u32,
    alphas: array<f32, 8>,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
) -> BraggTrackResult {
    // The structure rows of the inclusion block; the seed travels in `header.z`.
    let st = bragg_structure(
        material.inclusions.header.z,
        material.inclusions.structure[0], material.inclusions.structure[1],
        material.inclusions.structure[2],
    );
    let r = maybe_scatter_oriented_bragg(
        oriented_medium(), st, (*s).lambdas, alphas, (*s).origin, (*s).dir, (*h).hit.t,
        seed0, bounce, stokes, path_pdf,
    );
    return BraggTrackResult(1u, r.scattered, r.t_free, r.new_dir, r.bragg);
}

// The zoned host's per-zone, per-channel absorption coefficients of this segment, slot
// `zone * 8 + channel` (zone 0 is the base zone; zones past `zone_count` are zero):
// `ZonedCache::zone_alphas`, with the same per-zone evaluation `zoned_interior_absorption` uses.
fn zoned_scatter_zone_alphas(
    s: ptr<function, BounceRay>,
    rc: RayConstants,
    za: ptr<function, array<f32, 40>>,
) {
    for (var i: u32 = 0u; i < 40u; i = i + 1u) {
        (*za)[i] = 0.0;
    }
    let k_hat = (*s).k;
    var eigen_a = vec3<f32>(0.0, 0.0, 0.0);
    var eigen_b = vec3<f32>(0.0, 0.0, 0.0);
    if (!rc.is_anisotropic) {
        eigen_a = ordinary_eigen_polarization(k_hat, rc.c_axis);
        eigen_b = extraordinary_eigen_polarization(k_hat, rc.c_axis);
    }
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        (*za)[k] = zone_channel_alpha(
            rc.alpha_o_hoisted[k], rc.alpha_e_hoisted[k], rc.alpha_beta_hoisted[k],
            material.has_beta_ray != 0u,
            rc.is_anisotropic, rc.is_biaxial, rc.c_axis, k_hat, (*s).is_extraordinary,
            rc.n_alpha_hero, rc.n_beta_hero, rc.n_gamma_hero, rc.biax_ax0, rc.biax_ax1, rc.biax_ax2,
            rc.n_o_hero_seed, rc.n_e_hero_seed, eigen_a, eigen_b,
        );
    }
    let count = min(material.zones.header.zone_count, ZONE_MAX);
    for (var zi: u32 = 0u; zi < count; zi = zi + 1u) {
        let has_beta = rc.is_biaxial && material.zones.absorption[zi].has_beta_ray != 0u;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            let lambda = (*s).lambdas[k];
            let a_o = spectral_absorption(
                material.zones.absorption[zi].o_ray_bands,
                material.zones.absorption[zi].o_ray_band_count,
                lambda,
            );
            let a_e = spectral_absorption(
                material.zones.absorption[zi].e_ray_bands,
                material.zones.absorption[zi].e_ray_band_count,
                lambda,
            );
            var a_beta: f32 = 0.0;
            if (has_beta) {
                a_beta = spectral_absorption(
                    material.zones.absorption[zi].beta_ray_bands,
                    material.zones.absorption[zi].beta_ray_band_count,
                    lambda,
                );
            }
            (*za)[(zi + 1u) * 8u + k] = zone_channel_alpha(
                a_o, a_e, a_beta, has_beta,
                rc.is_anisotropic, rc.is_biaxial, rc.c_axis, k_hat, (*s).is_extraordinary,
                rc.n_alpha_hero, rc.n_beta_hero, rc.n_gamma_hero, rc.biax_ax0, rc.biax_ax1, rc.biax_ax2,
                rc.n_o_hero_seed, rc.n_e_hero_seed, eigen_a, eigen_b,
            );
        }
    }
}

// `ZonedCache::lengths_mm`: the per-zone path length in mm of the part of the segment
// `origin + dir * t` between `t0` and `t1` (model units).
fn zoned_scatter_lengths(
    origin: vec3<f32>,
    dir: vec3<f32>,
    t0: f32,
    t1: f32,
    path_scale: f32,
    lens: ptr<function, array<f32, 5>>,
) {
    for (var i: u32 = 0u; i < 5u; i = i + 1u) {
        (*lens)[i] = 0.0;
    }
    let scaled = (t1 - t0) * path_scale;
    if (!(scaled > 0.0)) {
        return;
    }
    let seg_a = (origin + dir * t0) * path_scale;
    let seg_b = (origin + dir * t1) * path_scale;
    let raw = zone_lengths(seg_a, seg_b);
    let total = raw[0] + raw[1] + raw[2] + raw[3] + raw[4];
    if (!(total > 0.0)) {
        (*lens)[0] = scaled;
        return;
    }
    for (var i: u32 = 0u; i < 5u; i = i + 1u) {
        if (raw[i] >= total) {
            (*lens)[i] = scaled;
        } else {
            (*lens)[i] = scaled * (raw[i] / total);
        }
    }
}

// `optical_depths_spectral`: sum over the zones of (alpha_z[k] + sigma_s[k]) * len_z.
fn zoned_scatter_depths(
    lens: ptr<function, array<f32, 5>>,
    za: ptr<function, array<f32, 40>>,
    sigma_s: ptr<function, array<f32, 8>>,
    depth: ptr<function, array<f32, 8>>,
) {
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        (*depth)[k] = 0.0;
    }
    for (var z: u32 = 0u; z < 5u; z = z + 1u) {
        let len_z = (*lens)[z];
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            (*depth)[k] = fma((*za)[z * 8u + k] + (*sigma_s)[k], len_z, (*depth)[k]);
        }
    }
}

// The optical depth of the first `t` model units of the segment, per channel.
fn zoned_scatter_depth_to(
    origin: vec3<f32>,
    dir: vec3<f32>,
    t: f32,
    path_scale: f32,
    za: ptr<function, array<f32, 40>>,
    sigma_s: ptr<function, array<f32, 8>>,
    depth: ptr<function, array<f32, 8>>,
) {
    var lens: array<f32, 5> = array<f32, 5>();
    zoned_scatter_lengths(origin, dir, 0.0, t, path_scale, &lens);
    zoned_scatter_depths(&lens, za, sigma_s, depth);
}

// `zoned_scatter_depth_to`'s channel 0 alone, with the same operations in the same order: the
// bisection reads only the hero, and the seven other channels per step cost registers on a
// kernel that is already at the edge on AMD.
fn zoned_scatter_hero_depth_to(
    origin: vec3<f32>,
    dir: vec3<f32>,
    t: f32,
    path_scale: f32,
    hero_coeff: array<f32, 5>,
) -> f32 {
    var lens: array<f32, 5> = array<f32, 5>();
    zoned_scatter_lengths(origin, dir, 0.0, t, path_scale, &lens);
    var depth: f32 = 0.0;
    for (var z: u32 = 0u; z < 5u; z = z + 1u) {
        depth = fma(hero_coeff[z], lens[z], depth);
    }
    return depth;
}

// oriented_zoned::zoned_hero_free_flight: the hero's free flight through the zoned segment by a
// 48-step bisection of its piecewise-linear optical depth, `t_free` in model units, or `hit_t`
// when `depth_target` is not inside the segment's total hero depth `depth_full_hero` (the hero
// survives). Shared by `bounce_scatter_sample_zoned` and `bounce_scatter_sample_zoned_bragg`.
fn zoned_hero_free_flight(
    origin: vec3<f32>,
    dir: vec3<f32>,
    hit_t: f32,
    path_scale: f32,
    za: ptr<function, array<f32, 40>>,
    sigma_s: ptr<function, array<f32, 8>>,
    depth_target: f32,
    depth_full_hero: f32,
) -> f32 {
    if (depth_target < depth_full_hero) {
        var hero_coeff: array<f32, 5> = array<f32, 5>();
        for (var z: u32 = 0u; z < 5u; z = z + 1u) {
            hero_coeff[z] = (*za)[z * 8u] + (*sigma_s)[0];
        }
        var lo: f32 = 0.0;
        var hi: f32 = hit_t;
        for (var it: u32 = 0u; it < 48u; it = it + 1u) {
            let mid = 0.5 * (lo + hi);
            if (zoned_scatter_hero_depth_to(origin, dir, mid, path_scale, hero_coeff) < depth_target) {
                lo = mid;
            } else {
                hi = mid;
            }
            if (hi - lo <= 1.1920929e-7 * hi) {
                break;
            }
        }
        return 0.5 * (lo + hi);
    }
    return hit_t;
}

// oriented_zoned::maybe_scatter_oriented_zoned: the oriented estimator for a zoned host. The
// hero's piecewise-linear optical depth is inverted by a 48-step bisection; the decisions,
// weights and result are those of `maybe_scatter_oriented` (the phase ratio is applied by the
// commit piece). `lam` are the ray's wavelengths, channel 0 the hero.
fn bounce_scatter_sample_zoned(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    m: OrientedMedium,
    bounce: u32,
    seed0: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
) -> OrientedScatterResult {
    let origin = (*s).origin;
    let dir = (*s).dir;
    let hit_t = (*h).hit.t;
    let path_scale = m.path_scale;
    // Per absorption-length unit, like the zone alphas.
    var sigma_s: array<f32, 8> = array<f32, 8>();
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        sigma_s[k] = ori_sigma_s(m, dir, (*s).lambdas[k]) / path_scale;
    }
    var za: array<f32, 40> = array<f32, 40>();
    zoned_scatter_zone_alphas(s, rc, &za);

    let dist_rand = ori_stream_draw(seed0, bounce, DISTANCE_SAMPLE_STREAM);
    let one_minus_u = max(1.0 - dist_rand, 1e-7);
    let depth_target = -(log(one_minus_u));

    var depth_full: array<f32, 8> = array<f32, 8>();
    zoned_scatter_depth_to(origin, dir, hit_t, path_scale, &za, &sigma_s, &depth_full);

    var result: OrientedScatterResult;
    result.scattered = 0u;
    result.t_free = 0.0;
    result.new_dir = vec3<f32>(0.0, 0.0, 0.0);

    if (depth_target < depth_full[0]) {
        // Invert the hero's piecewise-linear optical depth.
        let t_free = zoned_hero_free_flight(
            origin, dir, hit_t, path_scale, &za, &sigma_s, depth_target, depth_full[0],
        );

        // `local_extinction_spectral`: the extinction at the event point, averaged over a short
        // window about it.
        let delta = max(1e-4 * hit_t, 1e-7);
        let t_a = max(t_free - delta, 0.0);
        let t_b = min(t_free + delta, hit_t);
        var lens_ab: array<f32, 5> = array<f32, 5>();
        zoned_scatter_lengths(origin, dir, t_a, t_b, path_scale, &lens_ab);
        let lens_total = lens_ab[0] + lens_ab[1] + lens_ab[2] + lens_ab[3] + lens_ab[4];
        var depth_ab: array<f32, 8> = array<f32, 8>();
        zoned_scatter_depths(&lens_ab, &za, &sigma_s, &depth_ab);
        var sigma_t_local: array<f32, 8> = array<f32, 8>();
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            if (lens_total > 0.0) {
                sigma_t_local[k] = depth_ab[k] / lens_total;
            } else {
                sigma_t_local[k] = za[k] + sigma_s[k];
            }
        }

        let pdf_hero = max(sigma_t_local[0] * one_minus_u, 1e-12);
        var depth_free: array<f32, 8> = array<f32, 8>();
        zoned_scatter_depth_to(origin, dir, t_free, path_scale, &za, &sigma_s, &depth_free);
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            let tr_k = exp_poly(-depth_free[k]);
            let weight = tr_k * sigma_s[k] / pdf_hero;
            let intensity = max((*stokes)[k].x, 0.0) * weight;
            (*stokes)[k] = vec4<f32>(intensity, 0.0, 0.0, 0.0);
            (*path_pdf)[k] = (*path_pdf)[k] * (sigma_t_local[k] * tr_k);
        }
        let u_c = ori_stream_draw(seed0, bounce, ORIENTED_COMPONENT_STREAM);
        let u1 = ori_stream_draw(seed0, bounce, PHASE_DIR_U_STREAM);
        let u2 = ori_stream_draw(seed0, bounce, PHASE_DIR_V_STREAM);
        result.scattered = 1u;
        result.t_free = t_free;
        result.new_dir = ori_sample_direction(m, dir, (*s).lambdas[0], u_c, u1, u2);
        return result;
    }

    var survive: array<f32, 8> = array<f32, 8>();
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        survive[k] = exp_poly(-depth_full[k]);
    }
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        (*stokes)[k] = (*stokes)[k] * (survive[k] / max(survive[0], 1e-30));
        (*path_pdf)[k] = (*path_pdf)[k] * survive[k];
    }
    return result;
}

// oriented_zoned::maybe_scatter_oriented_zoned_bragg: the zoned estimator for a zoned host that
// also holds a Bragg domain structure. The zones change only the absorption, so the Bragg
// composition of `maybe_scatter_oriented_bragg` holds with the zoned optical depth in place of
// the homogeneous one: the hero's zoned free flight (the very draw and bisection of
// `bounce_scatter_sample_zoned`) bounds the tracking, and whichever event comes first wins. A
// reflection weighs channel k by exp(-(depth_k - depth_0)) at the reflection times its tracking
// weight; with nothing found `bounce_scatter_sample_zoned` decides and the null-collision
// weights multiply the result. `bragg` is set for a reflection only.
fn bounce_scatter_sample_zoned_bragg(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    m: OrientedMedium,
    bounce: u32,
    seed0: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    path_pdf: ptr<function, array<f32, 8>>,
) -> BraggTrackResult {
    let origin = (*s).origin;
    let dir = (*s).dir;
    let hit_t = (*h).hit.t;
    let path_scale = m.path_scale;
    let st = bragg_structure(
        material.inclusions.header.z,
        material.inclusions.structure[0], material.inclusions.structure[1],
        material.inclusions.structure[2],
    );
    // Per absorption-length unit, like the zone alphas.
    var sigma_s: array<f32, 8> = array<f32, 8>();
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        sigma_s[k] = ori_sigma_s(m, dir, (*s).lambdas[k]) / path_scale;
    }
    var za: array<f32, 40> = array<f32, 40>();
    zoned_scatter_zone_alphas(s, rc, &za);

    let dist_rand = ori_stream_draw(seed0, bounce, DISTANCE_SAMPLE_STREAM);
    let one_minus_u = max(1.0 - dist_rand, 1e-7);
    let depth_target = -(log(one_minus_u));

    var depth_full: array<f32, 8> = array<f32, 8>();
    zoned_scatter_depth_to(origin, dir, hit_t, path_scale, &za, &sigma_s, &depth_full);
    // The bisection runs only when the hero's flight ends inside the segment.
    var t_h: f32 = hit_t;
    if (depth_target < depth_full[0]) {
        t_h = zoned_hero_free_flight(
            origin, dir, hit_t, path_scale, &za, &sigma_s, depth_target, depth_full[0],
        );
    }

    let lam = (*s).lambdas;
    let lam_lo = vec4<f32>(lam[0], lam[1], lam[2], lam[3]);
    let lam_hi = vec4<f32>(lam[4], lam[5], lam[6], lam[7]);
    let tr = bragg_track_hero(st, origin, dir, min(t_h, hit_t), lam_lo, lam_hi, seed0, bounce);

    if (tr.reflect != 0u) {
        // The zoned medium's survival to the reflection, relative to the hero's, times the
        // tracking weight.
        var depth: array<f32, 8> = array<f32, 8>();
        zoned_scatter_depth_to(origin, dir, tr.t_model, path_scale, &za, &sigma_s, &depth);
        let depth_lo = vec4<f32>(depth[0], depth[1], depth[2], depth[3]);
        let depth_hi = vec4<f32>(depth[4], depth[5], depth[6], depth[7]);
        let tw = bragg_reflect_weights4(depth_lo, depth_hi, tr);
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            let w = bragg_channel_weight(tw, k);
            (*stokes)[k] = vec4<f32>(max((*stokes)[k].x, 0.0) * w, 0.0, 0.0, 0.0);
            (*path_pdf)[k] = (*path_pdf)[k] * w;
        }
        let new_dir = bragg_reflect_dir(dir, tr.normal, st.mosaic, seed0, bounce);
        return BraggTrackResult(1u, 1u, tr.t_model, new_dir, 1u);
    }

    // Nothing found: the zoned event decides as before, and the null-collision weights multiply
    // the result.
    let sc = bounce_scatter_sample_zoned(s, h, rc, m, bounce, seed0, stokes, path_pdf);
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        let w = bragg_channel_weight(tr, k);
        (*stokes)[k] = (*stokes)[k] * w;
        (*path_pdf)[k] = (*path_pdf)[k] * w;
    }
    return BraggTrackResult(1u, sc.scattered, sc.t_free, sc.new_dir, 0u);
}

// True for a zoned host with oriented inclusions or a domain structure. A pipeline built with
// `SCATTER_NEE = false` is such a stone by construction (`material_class::feature_bits` sets
// `NO_SCATTER_NEE` only for a zoned stone with an active inclusion block), so the test is then a
// pipeline constant and the unzoned routes below (`bragg_track`, `maybe_scatter_oriented`)
// are compiled out instead of staying reachable but dead. RADV (Mesa) produced wrong or
// non-finite radiance for exactly these zoned-oriented classes while that code was reachable
// (2026-10-27: zoned opal +14 %, zoned Star Silk NaN). The generic pipeline keeps the
// run-time `zone_count` test, as before.
fn zoned_oriented_stone() -> bool {
    return ZONED_ORIENTED && (!SCATTER_NEE || material.zones.header.zone_count > 0u);
}

// The scatter decision for a stone with oriented inclusions: the zoned estimator in a zoned
// host (with the Bragg tracker on top for a zoned stone that has a domain structure), otherwise
// the Bragg tracker (a stone with a domain structure) or the plain oriented estimator.
// Same event shape as `bounce_scatter_sample`.
fn bounce_scatter_sample_oriented(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    alphas: array<f32, 8>,
) -> BounceScatterEvent {
    let m = oriented_medium();
    let lam = (*s).lambdas;
    var local_stokes = (*s).stokes;
    var local_path_pdf = (*s).path_pdf;
    var sc: OrientedScatterResult;
    sc.scattered = 0u;
    sc.t_free = 0.0;
    sc.new_dir = vec3<f32>(0.0, 0.0, 0.0);
    var bragg: u32 = 0u;
    // `ZONED_ORIENTED` is false only in a pipeline built for an unzoned stone, where the zone
    // count is 0 anyway: the estimator is then compiled out (see `01_header_and_constants`).
    if (zoned_oriented_stone()) {
        // A zoned stone with a domain structure (opal, labradorite) reflects on top of its zones.
        let zoned_structure = material.inclusions.header.y != 0u;
        if (zoned_structure) {
            let bt = bounce_scatter_sample_zoned_bragg(s, h, rc, m, bounce, seed0, &local_stokes, &local_path_pdf);
            sc = OrientedScatterResult(bt.scattered, bt.t_free, bt.new_dir);
            bragg = bt.bragg;
        } else {
            sc = bounce_scatter_sample_zoned(s, h, rc, m, bounce, seed0, &local_stokes, &local_path_pdf);
        }
    } else {
        var handled = false;
        if (material.inclusions.header.y != 0u) {
            let bt = bragg_track(s, h, bounce, seed0, alphas, &local_stokes, &local_path_pdf);
            if (bt.handled != 0u) {
                sc = OrientedScatterResult(bt.scattered, bt.t_free, bt.new_dir);
                bragg = bt.bragg;
                handled = true;
            }
        }
        if (!handled) {
            sc = maybe_scatter_oriented(
                m, lam, alphas, (*s).dir, (*h).hit.t, seed0, bounce, &local_stokes, &local_path_pdf,
            );
        }
    }
    (*s).stokes = local_stokes;
    (*s).path_pdf = local_path_pdf;
    return BounceScatterEvent(sc.scattered, (*s).origin + sc.t_free * (*s).dir, sc.new_dir, bragg);
}

// Henyey-Greenstein NEE at the scatter point and the MIS carry for the phase-sampled
// continuation. HDR environments only. Runs BEFORE the commit, on the pre-roulette stokes.
// `alphas` are this segment's own (evaluated at `(*s).k`, the direction the ray ARRIVED
// along); the shadow ray uses them as they are only for an isotropic material, and
// re-evaluates an anisotropic material's at its own direction (see
// `nee_contribution_hg_scatter`).
fn bounce_scatter_nee(
    s: ptr<function, BounceRay>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    ev: BounceScatterEvent,
    alphas: array<f32, 8>,
) {
    let old_dir = (*s).dir;
    let hg_lambdas = (*s).lambdas;
    let hg_stokes = (*s).stokes;
    let hg_path_pdf = (*s).path_pdf;
    let hg_compat = (*s).compat;
    let nee_deposit = nee_contribution_hg_scatter(
        hg_lambdas, rc.n_o_hero_seed, ev.scatter_point, old_dir,
        material.scattering_g, seed0, bounce, hg_stokes, hg_path_pdf, hg_compat,
        alphas, rc, (*s).is_extraordinary,
    );
    (*s).nee_xyz = (*s).nee_xyz + nee_deposit;
    var mis_phase = henyey_greenstein_phase(dot(ev.new_dir, old_dir), material.scattering_g);
    if (material_oriented_active()) {
        // The density the oriented estimator draws this direction with: the hero's mixture phase.
        var phase_per_channel: array<f32, 8>;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            phase_per_channel[k] = 0.0;
        }
        mis_phase = ori_channel_phases(oriented_medium(), hg_lambdas, old_dir, ev.new_dir, &phase_per_channel);
    }
    // The competing technique still exists when this particular light sample was rejected.
    // Keep its MIS carry even when the returned direct contribution is zero.
    (*s).pending_light_mis = mis_phase;
    (*s).pending_light_mis_dir = ev.new_dir;
}

// Moves the ray to the scatter point along the sampled direction, then Russian roulette.
fn bounce_scatter_commit(
    s: ptr<function, BounceRay>,
    bounce: u32,
    seed0: u32,
    ev: BounceScatterEvent,
) -> u32 {
    if (material_oriented_active() && ev.bragg == 0u) {
        // (A Bragg reflection's ratio is one: `OrientedScatter::phase_ratio` of the CPU.)
        // Each channel owes the ratio of its own mixture phase to the hero's at the sampled
        // direction (`OrientedScatter::phase_ratio`); applied AFTER the scattering-point NEE,
        // which used the weight without it. `(*s).dir` is still the direction the ray arrived along.
        var phase_per_channel: array<f32, 8>;
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            phase_per_channel[k] = 0.0;
        }
        let hero_phase = ori_channel_phases(oriented_medium(), (*s).lambdas, (*s).dir, ev.new_dir, &phase_per_channel);
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            var ratio: f32 = 1.0;
            if (hero_phase > 1e-12) {
                ratio = phase_per_channel[k] / hero_phase;
            }
            (*s).stokes[k] = (*s).stokes[k] * ratio;
        }
    }
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

// Scatter phase: interior scatter (HG NEE + RR).
fn bounce_scatter(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    if (bounce_scatter_applies(s)) {
        var alphas: array<f32, 8> = array<f32, 8>();
        bounce_scatter_alphas(s, rc, &alphas);
        let ev = bounce_scatter_sample(s, h, rc, bounce, seed0, alphas);
        if (ev.scattered != 0u) {
            // A Bragg reflection takes no scattering-point NEE and leaves no MIS carry.
            if (SCATTER_NEE && params.env_mode == 2u && scatter_nee_zone_free() && ev.bragg == 0u) {
                bounce_scatter_nee(s, rc, bounce, seed0, ev, alphas);
            }
            return bounce_scatter_commit(s, bounce, seed0, ev);
        }
    }
    return PHASE_NEXT;
}

