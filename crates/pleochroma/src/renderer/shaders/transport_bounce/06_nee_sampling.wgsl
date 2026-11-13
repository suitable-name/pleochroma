// ---------------------------------------------------------------------------------
// GPU Next-Event Estimation & Multiple Importance Sampling
// ---------------------------------------------------------------------------------

fn dist1d_find_bucket(cdf_start: u32, n: u32, u: f32) -> u32 {
    var first: u32 = 0u;
    var len: u32 = n + 1u;
    while (len > 0u) {
        let half = len / 2u;
        let middle = first + half;
        if (dist_cdf[cdf_start + middle] <= u) {
            first = middle + 1u;
            len = len - (half + 1u);
        } else {
            len = half;
        }
    }
    var offset: u32 = 0u;
    if (first > 0u) {
        offset = first - 1u;
    }
    return min(offset, n - 1u);
}

fn dist1d_bucket_pdf(func_start: u32, offset: u32, func_int: f32) -> f32 {
    if (func_int > 0.0) {
        return max(dist_func[func_start + offset], 0.0) / func_int;
    }
    return 1.0;
}

fn dist1d_sample_continuous(cdf_start: u32, func_start: u32, n: u32, func_int: f32, u_in: f32) -> Dist1dSample {
    let u = clamp(u_in, 0.0, 0.99999994);
    let offset = dist1d_find_bucket(cdf_start, n, u);
    let cdf0 = dist_cdf[cdf_start + offset];
    let cdf1 = dist_cdf[cdf_start + offset + 1u];
    let span = cdf1 - cdf0;
    var du: f32 = 0.0;
    if (span > 0.0) {
        du = (u - cdf0) / span;
    }
    let sample = clamp((f32(offset) + du) / f32(n), 0.0, 0.99999994);
    let pdf = dist1d_bucket_pdf(func_start, offset, func_int);
    var res: Dist1dSample;
    res.sample = sample;
    res.pdf = pdf;
    res.offset = offset;
    return res;
}

fn dist1d_pdf(func_start: u32, n: u32, func_int: f32, x: f32) -> f32 {
    let offset = min(u32(clamp(x, 0.0, 0.99999994) * f32(n)), n - 1u);
    return dist1d_bucket_pdf(func_start, offset, func_int);
}

fn dist2d_sample(u0: f32, u1: f32) -> Dist2dSample {
    let width = dist_dims.width;
    let height = dist_dims.height;
    let marginal_cdf_start = height * (width + 1u);
    let marginal_func_start = width * height;
    let marginal_func_int = dist_dims.marginal_func_int;

    let s_v = dist1d_sample_continuous(marginal_cdf_start, marginal_func_start, height, marginal_func_int, u1);
    let row = s_v.offset;
    let v = s_v.sample;
    let pdf_v = s_v.pdf;

    let cond_cdf_start = row * (width + 1u);
    let cond_func_start = row * width;
    let cond_func_int = dist_func[marginal_func_start + row];

    let s_u = dist1d_sample_continuous(cond_cdf_start, cond_func_start, width, cond_func_int, u0);
    let u = s_u.sample;
    let pdf_u = s_u.pdf;

    let dir = hdr_map_to_world(hdr_uv_to_direction(u, v), hdr_rot());
    let rgb = hdr_env_sample_bilinear(u, v);
    let pdf = pdf_uv_to_solid_angle(pdf_u * pdf_v, v);

    var res: Dist2dSample;
    res.dir = dir;
    res.rgb = rgb;
    res.pdf = pdf;
    return res;
}

fn dist2d_pdf_uv(u: f32, v: f32) -> f32 {
    let width = dist_dims.width;
    let height = dist_dims.height;
    let marginal_func_start = width * height;
    let marginal_func_int = dist_dims.marginal_func_int;

    let row = min(u32(clamp(v, 0.0, 0.99999994) * f32(height)), height - 1u);
    let pdf_v = dist1d_pdf(marginal_func_start, height, marginal_func_int, v);

    let cond_func_start = row * width;
    let cond_func_int = dist_func[marginal_func_start + row];
    let pdf_u = dist1d_pdf(cond_func_start, width, cond_func_int, u);

    return pdf_u * pdf_v;
}

// Matches `renderer::env_map::EnvironmentMap::pdf`. `sin(theta)` is `length(vec2(x, z))`
// (the CPU's `x.hypot(z)`), not `sin(acos(y))`.
fn dist2d_pdf(dir: vec3<f32>) -> f32 {
    let m = hdr_world_to_map(dir, hdr_rot());
    let uv = hdr_direction_to_uv(m);
    let pdf_uv = dist2d_pdf_uv(uv.x, uv.y);
    let d = normalize(m);
    let sin_theta = length(vec2<f32>(d.x, d.z));
    return pdf_uv_to_solid_angle_from_sin(pdf_uv, sin_theta);
}

// ---- concave-stone shadow probe of the Henyey-Greenstein NEE ----
// optics::raytracer::scattering::hg_nee::{exit_transmittance, walk_exits}. Only used when
// `params.num_tools > 0`. A convex stone has one crossing and takes the single-exit path in
// `nee_contribution_hg_scatter`.

// The most boundaries the probe crosses before it gives the sample up
// (`HG_NEE_MAX_CROSSINGS`).
const HG_NEE_MAX_CROSSINGS: u32 = 4u;

struct HgExit {
    ok: bool,
    normal: vec3<f32>,
    t_material: f32,
    cos_i: f32,
    cos_t: f32,
    t_unpol: f32,
}

// Unpolarised exit Fresnel transmittance and refracted-angle cosine for a ray leaving an
// interior of index `n_inside` at `cos_i`, as `(t_unpol, cos_t)`; `x < 0` marks total
// internal reflection (the sample is dropped).
fn hg_exit_transmittance(n_inside: f32, cos_i: f32) -> vec2<f32> {
    let sin2_t = min(n_inside * n_inside * fma(-cos_i, cos_i, 1.0), 1.0);
    if (sin2_t >= 1.0) {
        return vec2<f32>(-1.0, 0.0);
    }
    let cos_t = sqrt(max(1.0 - sin2_t, 0.0));
    let r_s = fma(n_inside, cos_i, -cos_t) / fma(n_inside, cos_i, cos_t);
    let r_p = fma(n_inside, -cos_t, cos_i) / fma(n_inside, cos_t, cos_i);
    let r_unpol = clamp(0.5 * fma(r_p, r_p, r_s * r_s), 0.0, 1.0);
    return vec2<f32>(1.0 - r_unpol, cos_t);
}

// Concave-stone probe. Follows the sample direction through up to `HG_NEE_MAX_CROSSINGS`
// boundaries, multiplying in the Fresnel transmittance at each exit and each cavity re-entry.
// It also sums the in-material segment lengths for the Beer-Lambert term. Each segment
// starts `HG_SHADOW_PROBE_OFFSET` inside the material, so that offset is added back. Drops
// the sample above the crossing cap, on TIR or a frosted exit, and when the walk ends on an
// entry. Between exits the direction stays the interior one; only the last exit bends it
// (see the CPU twin).
fn hg_walk_exits(probe_origin: vec3<f32>, dir: vec3<f32>, first: HitInfo, n_inside: f32) -> HgExit {
    var out: HgExit;
    out.ok = false;
    out.normal = vec3<f32>(0.0, 0.0, 0.0);
    out.t_material = 0.0;
    out.cos_i = 0.0;
    out.cos_t = 0.0;
    out.t_unpol = 0.0;
    var origin = probe_origin;
    var hit = first;
    var crossings: u32 = 1u;
    var chain: f32 = 1.0;
    var material_len: f32 = 0.0;
    var have_last = false;
    var last_normal = vec3<f32>(0.0, 0.0, 0.0);
    var last_cos_i: f32 = 0.0;
    var last_cos_t: f32 = 0.0;
    for (var guard: u32 = 0u; guard <= HG_NEE_MAX_CROSSINGS; guard = guard + 1u) {
        if (hit.facet_idx < arrayLength(&facet_finishes) && surf_is_ground(facet_finishes[hit.facet_idx])) {
            return out;
        }
        let cos_d = dot(dir, hit.normal);
        if (cos_d > 0.0) {
            // The segment ending at an exit is in-material, from an origin offset
            // `HG_SHADOW_PROBE_OFFSET` inside it (the probe's own origin, or the restart
            // just past a re-entry).
            material_len = material_len + (hit.t + HG_SHADOW_PROBE_OFFSET);
            let cos_in = clamp(cos_d, 0.0, 1.0);
            let tr = hg_exit_transmittance(n_inside, cos_in);
            if (tr.x < 0.0) {
                return out;
            }
            chain = chain * tr.x;
            have_last = true;
            last_normal = hit.normal;
            last_cos_i = cos_in;
            last_cos_t = tr.y;
        } else {
            // Re-entry through a far cavity wall: the air-gap segment ending here is not
            // absorbing, but the interface transmits only `T`.
            let tr = hg_exit_transmittance(n_inside, clamp(-cos_d, 0.0, 1.0));
            if (tr.x < 0.0) {
                return out;
            }
            chain = chain * tr.x;
            have_last = false;
        }
        origin = origin + hit.t * dir + dir * HG_SHADOW_PROBE_OFFSET;
        let next = intersect_ray(origin, dir);
        if (!next.hit) {
            break;
        }
        crossings = crossings + 1u;
        if (crossings > HG_NEE_MAX_CROSSINGS) {
            return out;
        }
        hit = next;
    }
    if (!have_last) {
        return out;
    }
    out.ok = true;
    out.normal = last_normal;
    out.t_material = material_len;
    out.cos_i = last_cos_i;
    out.cos_t = last_cos_t;
    out.t_unpol = chain;
    return out;
}

// optics::raytracer::scattering::hg_nee::nee_contribution_hg_scatter: direct light at a
// scattering point. `alphas` are the segment's per-channel coefficients at the arrival
// direction. An isotropic material uses them as they are (the CPU's `ShadowAlphas::Fixed`
// arm). An anisotropic material (`ShadowAlphas::AssignedMode`) re-evaluates them per channel
// at the shadow direction, which is what the continuation in that direction pays.
// `sigma_s`, `absorption_path_scale` and `facet_finishes` are read from the global bindings,
// as the scatter call in `transport_bounce_step` does.
// The deposit is a self-contained sample. It is combined into XYZ with the `path_pdf` and
// `compat` as they stand at this event, not the final ones. The inputs are immutable
// snapshots and the XYZ increment is returned, as in the event-local integration of
// `optics::raytracer::scattering::try_scatter_step`.
fn nee_contribution_hg_scatter(
    lambdas: array<f32, 8>,
    n_inside_hero: f32,
    scatter_point: vec3<f32>,
    scatter_dir_in: vec3<f32>,
    g: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: array<vec4<f32>, 8>,
    path_pdf: array<f32, 8>,
    compat: array<u32, 8>,
    alphas: array<f32, 8>,
    rc: RayConstants,
    is_extraordinary: bool,
) -> vec3<f32> {
    let u0 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_U_STREAM))) / 4294967295.0;
    let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ NEE_ENV_DIR_V_STREAM))) / 4294967295.0;
    let sample = dist2d_sample(u0, u1);
    if (sample.pdf <= 0.0) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }

    let probe_origin = scatter_point + sample.dir * HG_SHADOW_PROBE_OFFSET;
    let hit = intersect_ray(probe_origin, sample.dir);
    if (!hit.hit) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }

    // The exit the shadow ray finally leaves through. A convex stone has one crossing (`hit`).
    // A concave stone can cross into a cavity and back, so the probe walks its boundaries
    // (`hg_walk_exits`). The in-material length counts from the scattering point. The probe
    // origin is `HG_SHADOW_PROBE_OFFSET` inside the material, so `hit.t` alone is short by that.
    var exit_normal = hit.normal;
    var exit_t = hit.t + HG_SHADOW_PROBE_OFFSET;
    var cos_i: f32 = 0.0;
    var cos_t: f32 = 0.0;
    var t_unpol: f32 = 0.0;
    if (params.num_tools == 0u) {
        // A ground (frosted) exit has no specular Fresnel term for this ray.
        // `nee_contribution_frosted_exterior` covers that surface point instead.
        if (hit.facet_idx < arrayLength(&facet_finishes) && surf_is_ground(facet_finishes[hit.facet_idx])) {
            return vec3<f32>(0.0, 0.0, 0.0);
        }
        // An exit inside an abraded edge band is a ground event, so it is dropped like a
        // frosted exit (the twin of `NeeContext::edge_band`).
        let nee_edge = material.inclusions.edge;
        if (nee_edge.x > 0.0 && nee_edge.y > 0.0
            && surface_in_edge_band(probe_origin + hit.t * sample.dir, hit.facet_idx, nee_edge.x)) {
            return vec3<f32>(0.0, 0.0, 0.0);
        }

        cos_i = clamp(dot(sample.dir, hit.normal), 0.0, 1.0);
        let sin2_t = min(n_inside_hero * n_inside_hero * fma(-cos_i, cos_i, 1.0), 1.0);
        if (sin2_t >= 1.0) {
            return vec3<f32>(0.0, 0.0, 0.0);
        }
        cos_t = sqrt(max(1.0 - sin2_t, 0.0));
        let r_s = fma(n_inside_hero, cos_i, -cos_t) / fma(n_inside_hero, cos_i, cos_t);
        let r_p = fma(n_inside_hero, -cos_t, cos_i) / fma(n_inside_hero, cos_t, cos_i);
        let r_unpol = clamp(0.5 * fma(r_p, r_p, r_s * r_s), 0.0, 1.0);
        t_unpol = 1.0 - r_unpol;
    } else {
        let walked = hg_walk_exits(probe_origin, sample.dir, hit, n_inside_hero);
        if (!walked.ok) {
            return vec3<f32>(0.0, 0.0, 0.0);
        }
        exit_normal = walked.normal;
        exit_t = walked.t_material;
        cos_i = walked.cos_i;
        cos_t = walked.cos_t;
        t_unpol = walked.t_unpol;
    }

    let phase_cos = dot(sample.dir, scatter_dir_in);
    var phase_val = henyey_greenstein_phase(phase_cos, g);
    // Oriented inclusions give each channel its own mixture phase. `phase_ch` holds every
    // channel's value and `phase_val` the hero's, the density a phase-sampled continuation
    // uses. Plain haze gives one Henyey-Greenstein value for all channels.
    var phase_ch: array<f32, 8>;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        phase_ch[k] = phase_val;
    }
    let oriented = material_oriented_active();
    let medium = oriented_medium();
    if (oriented) {
        phase_val = ori_channel_phases(medium, lambdas, scatter_dir_in, sample.dir, &phase_ch);
    }
    let mis_weight = balance_heuristic(sample.pdf, phase_val);
    if (mis_weight <= 0.0) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }

    // Exterior direction of the light sample: Snell's law at the exit facet, in the vector
    // form `eta*k_hat + (eta*cos_i - cos_t)*normal`. `sample.pdf` stays in the interior
    // measure that `dist2d_sample` drew from. Only the radiance lookup uses the refracted
    // direction.
    let refracted_dir = normalize(n_inside_hero * sample.dir - fma(n_inside_hero, cos_i, -cos_t) * exit_normal);
    let refracted_uv = hdr_direction_to_uv(hdr_world_to_map(refracted_dir, hdr_rot()));
    // At the map's exposure, as every HDR lookup (CPU twin: `hdr_rgb_at_exposure` in
    // `nee_contribution_scatter`).
    let env_rgb = hdr_rgb_at_exposure(hdr_env_sample_bilinear(refracted_uv.x, refracted_uv.y));

    // Medium transmittance to the same boundary, as a phase-sampled continuation would pay it:
    // per channel `exp_poly(-(alpha_k + sigma_s) * d * path_scale)`, the same term as the
    // survive branch of `maybe_scatter_or_extinguish`. `exp_poly` is used, not `exp()` (see
    // `transport_physics.wgsl`). `alpha_k` is taken at the shadow direction, and `d` is the
    // full distance from the scattering point.
    let hit_t_scaled = exit_t * material.absorption_path_scale;
    // `scattering_sigma_s` is per model unit; convert to absorption-length units.
    let sigma_s_abs = material.scattering_sigma_s / material.absorption_path_scale;

    var nee_deposit: array<f32, 8>;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        // Shadow-ray extinction. Isotropic: the segment's `alphas[k]`. Anisotropic: the
        // assigned mode's coefficient at the shadow direction (`scatter_alpha_anisotropic`).
        var alpha_k: f32 = alphas[k];
        if (rc.is_anisotropic) {
            alpha_k = scatter_alpha_anisotropic(rc, k, sample.dir, is_extraordinary);
        }
        // An oriented medium's coefficient depends on the shadow direction and on the channel.
        var sigma_s_abs_k: f32 = sigma_s_abs;
        if (oriented) {
            sigma_s_abs_k = ori_sigma_s(medium, sample.dir, lambdas[k]) / material.absorption_path_scale;
        }
        let transmittance_k = exp_poly(-(alpha_k + sigma_s_abs_k) * hit_t_scaled);
        let env_k = rgb_to_spectral_radiance(env_rgb.x, env_rgb.y, env_rgb.z, lambdas[k]);
        let nee_common = t_unpol * phase_ch[k] * mis_weight / sample.pdf;
        nee_deposit[k] = fma(stokes[k].x * transmittance_k * nee_common * env_k, 1.0, 0.0);
    }
    return integrate_channels_to_xyz_family(nee_deposit, lambdas, path_pdf, compat);
}

// Deposits into the shared `radiance`, weighted by the final post-loop `path_pdf` at
// `transport_finalize_ray`, unlike `nee_contribution_hg_scatter` above. No separate `nee_xyz`
// is needed: a deposit is made only for a direction in the outward half-space that the
// concave-stone probe did not hit, so the next bounce escapes directly with no `path_pdf`
// update in between. See the "Why the exterior NEE
// deposit needs no separate `nee_xyz` accumulator" section of
// `optics::raytracer::scattering::frosted`.
//
// Two light-sampling techniques share this body, mirroring `sample_environment_for_nee`:
// the HDR map's importance distribution (`env_mode == 2u`) and the analytic sun of
// `LightingModel::DaylightSun` (`env_mode == 1u`, `studio_model == 5u`: a direction uniform
// over the 0.27 degree disc about `key_dir`, `pdf = 1 / SUN_SOLID_ANGLE`, radiance
// `daylight_sun_factor * (spd * exposure)`). `key_dir` is only read for the sun.
fn nee_contribution_frosted_exterior(
    lambdas: ptr<function, array<f32, 8>>,
    // The surface point of this bounce, the origin of the concave-stone shadow probe.
    origin: vec3<f32>,
    ext_normal: vec3<f32>,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
    key_dir: vec3<f32>,
) {
    let u0 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_U_STREAM))) / 4294967295.0;
    let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_V_STREAM))) / 4294967295.0;
    let sun_mode = params.env_mode != 2u;
    var light_dir = vec3<f32>(0.0, 0.0, 0.0);
    var light_pdf: f32 = 0.0;
    var hdr_rgb = vec3<f32>(0.0, 0.0, 0.0);
    if (sun_mode) {
        light_dir = daylight_sun_cone_direction(key_dir, u0, u1);
        light_pdf = 1.0 / SUN_SOLID_ANGLE;
    } else {
        let sample = dist2d_sample(u0, u1);
        light_dir = sample.dir;
        light_pdf = sample.pdf;
        // `sample_environment_for_nee`: the texel at the map's exposure.
        hdr_rgb = hdr_rgb_at_exposure(sample.rgb);
    }
    if (light_pdf <= 0.0) {
        return;
    }

    let cos_light = dot(light_dir, ext_normal);
    if (cos_light <= 0.0) {
        return;
    }

    // Concave stone: the sampled direction can run into the opposite wall of a cavity, so
    // cast a shadow probe and deposit only on a miss. Any hit is an occluder, which keeps the
    // two MIS techniques consistent. The probe runs after the RNG draws, so the stream use
    // matches the convex case. See the "Concave stones" section of
    // `optics::raytracer::scattering::frosted`.
    if (params.num_tools != 0u) {
        if (intersect_ray(origin + light_dir * 1e-4, light_dir).hit) {
            return;
        }
    }

    let brdf_pdf = cos_light / PI;
    let mis_weight = balance_heuristic(light_pdf, brdf_pdf);
    if (mis_weight <= 0.0) {
        return;
    }

    let nee_common = brdf_pdf * mis_weight / light_pdf;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        var env_k: f32;
        if (sun_mode) {
            env_k = daylight_sun_factor(key_dir) * (studio_spectral_power((*lambdas)[k]) * params.studio_exposure);
        } else {
            env_k = rgb_to_spectral_radiance(hdr_rgb.x, hdr_rgb.y, hdr_rgb.z, (*lambdas)[k]);
        }
        (*radiance)[k] = fma((*stokes)[k].x * nee_common * env_k, 1.0, (*radiance)[k]);
    }
}

// optics::raytracer::surface::ground::nee_contribution_ground: next-event estimation for the
// two ground outcomes that land on the exterior side of the gem (a reflection while outside,
// a transmission while inside). It replaces `nee_contribution_frosted_exterior` at a ground
// facet. The light direction is drawn as there (HDR distribution or sun cone). The ground
// lobe is evaluated at it (`ground_eval`: throughput `w` and branch density `p`), and the
// deposit is `intensity * w * p / p_light * MIS`, with the balance heuristic of the light
// density against `p`. A concave stone casts a shadow probe. Deposits into the shared
// `radiance`, as the frosted exterior NEE does.
fn nee_contribution_ground(
    ground: GroundCtx,
    is_refl: bool,
    lambdas: ptr<function, array<f32, 8>>,
    origin: vec3<f32>,
    rng_seed: u32,
    bounce: u32,
    stokes: ptr<function, array<vec4<f32>, 8>>,
    radiance: ptr<function, array<f32, 8>>,
    key_dir: vec3<f32>,
) {
    let u0 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_U_STREAM))) / 4294967295.0;
    let u1 = f32(hash_u32(rng_seed ^ hash_u32(bounce ^ FROSTED_NEE_ENV_DIR_V_STREAM))) / 4294967295.0;
    let sun_mode = params.env_mode != 2u;
    var light_dir = vec3<f32>(0.0, 0.0, 0.0);
    var light_pdf: f32 = 0.0;
    var hdr_rgb = vec3<f32>(0.0, 0.0, 0.0);
    if (sun_mode) {
        light_dir = daylight_sun_cone_direction(key_dir, u0, u1);
        light_pdf = 1.0 / SUN_SOLID_ANGLE;
    } else {
        let sample = dist2d_sample(u0, u1);
        light_dir = sample.dir;
        light_pdf = sample.pdf;
        // `sample_environment_for_nee`: the texel at the map's exposure.
        hdr_rgb = hdr_rgb_at_exposure(sample.rgb);
    }
    if (light_pdf <= 0.0) {
        return;
    }

    // A direction behind the surface (or off the lobe) has no density here; the BSDF-sampled
    // continuation still gets its own chance to reach it.
    let ev = ground_eval(ground, light_dir, is_refl);
    if (!ev.ok) {
        return;
    }
    // Concave stone: any hit of the shadow probe is an occluder (see
    // `nee_contribution_frosted_exterior`).
    if (params.num_tools != 0u) {
        if (intersect_ray(origin + light_dir * 1e-4, light_dir).hit) {
            return;
        }
    }
    let mis_weight = balance_heuristic(light_pdf, ev.pdf);
    if (mis_weight <= 0.0) {
        return;
    }

    let nee_common = ev.weight * ev.pdf * mis_weight / light_pdf;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        var env_k: f32;
        if (sun_mode) {
            env_k = daylight_sun_factor(key_dir) * (studio_spectral_power((*lambdas)[k]) * params.studio_exposure);
        } else {
            env_k = rgb_to_spectral_radiance(hdr_rgb.x, hdr_rgb.y, hdr_rgb.z, (*lambdas)[k]);
        }
        (*radiance)[k] = fma(max((*stokes)[k].x, 0.0) * nee_common * env_k, 1.0, (*radiance)[k]);
    }
}

// Whether the scene offers a light-sampling technique: an HDR map, or the analytic sun
// (`environment_supports_nee` on the CPU). Gates the frosted-exterior NEE and the MIS weight
// of a frosted continuation's escape. Henyey-Greenstein scattering NEE is HDR-only
// (`params.env_mode == 2u`).
fn nee_light_available() -> bool {
    return params.env_mode == 2u || (params.env_mode == 1u && params.studio_model == 5u);
}

// `environment_nee_pdf`: the light-sampling technique's solid-angle pdf at `dir` -- the HDR
// distribution's, or `daylight_sun_nee_pdf`'s (zero outside the disc).
fn nee_light_pdf(dir: vec3<f32>, key_dir: vec3<f32>) -> f32 {
    if (params.env_mode == 2u) {
        return dist2d_pdf(dir);
    }
    return daylight_sun_nee_pdf(normalize(dir), key_dir);
}

