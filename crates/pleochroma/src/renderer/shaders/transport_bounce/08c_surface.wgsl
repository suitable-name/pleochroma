// Surface phase: shading normal, frame rotation, interior absorption, facet finish lookup, and arm selection.
//
// Surface finish (lane RP-G2), the WGSL twin of `optics::raytracer::surface`. The per-facet word
// of binding 8 (`FacetSurface`) is resolved here, before the plane-of-incidence frame is
// built, into what the CPU's `begin_facet_surface` decides:
//
// - a ground facet (word kind 1 or 3, or a point inside the abraded-edge band): the arm is
//   `ARM_FROSTED`; `BounceHit::finish` keeps the word (the band's synthetic word
//   `SURF_EDGE_GROUND_WORD` for the band), from which the ground arm re-derives its lobe;
// - a polish with roughness (word kind 2, possibly after the edge-wear boost): with the
//   probability of the total integrated scatter the shading normal becomes a micro-normal
//   drawn from the visible-normal distribution and the unchanged polished arm runs at it;
//   `BounceHit::finish` keeps the boosted word, and `surface_finish_event` (called after
//   the arm, see `08f`) re-derives the draw to correct the throughput;
// - anything else: untouched.
//
// Every value is re-derived from the ray's state by pure functions of (word, hit point, wave
// normal, wavelengths, seed, bounce), so nothing but `BounceHit::finish` has to survive
// between the event wavefront's kernels.

// The energy-compensation table (`compensation::ground_albedo`) is the tail of the `dist_func`
// buffer, `GROUND_TABLE_WORDS` f32 values long (`env_map_gpu::flatten_distribution` appends it
// behind the distribution, whose indexing never reaches it): the megakernel and the ground arm
// kernel are already at the storage-buffer limit, and both bind `dist_func`. A buffer without
// the table (shorter than that) compensates nothing.
fn ground_albedo(alpha: f32, cos_i: f32, eta_rel: f32) -> f32 {
    let total = arrayLength(&dist_func);
    if (total < GROUND_TABLE_WORDS) {
        return 1.0;
    }
    let base = total - GROUND_TABLE_WORDS;
    let cell = ground_albedo_cell(alpha, cos_i, eta_rel);
    return ground_albedo_blend(
        cell,
        dist_func[base + ground_cell_index(cell, 0u, 0u, 0u)],
        dist_func[base + ground_cell_index(cell, 0u, 0u, 1u)],
        dist_func[base + ground_cell_index(cell, 0u, 1u, 0u)],
        dist_func[base + ground_cell_index(cell, 0u, 1u, 1u)],
        dist_func[base + ground_cell_index(cell, 1u, 0u, 0u)],
        dist_func[base + ground_cell_index(cell, 1u, 0u, 1u)],
        dist_func[base + ground_cell_index(cell, 1u, 1u, 0u)],
        dist_func[base + ground_cell_index(cell, 1u, 1u, 1u)],
    );
}

// The faceting machine of this render: `(arm_offset_deg, a, b)` (see `surf_machine_angle_deg`),
// carried in the spare slots of the material's inclusion block (`edge.w`, `haze.zw`); the abraded
// edge itself is `edge = [width, alpha, halo_rq_nm, arm_offset_deg]`.
fn surface_machine() -> vec3<f32> {
    let spare = material.inclusions.haze;
    return vec3<f32>(material.inclusions.edge.w, spare.z, spare.w);
}

struct SurfaceBegin {
    finish: u32,
    normal: vec3<f32>,
}

// `begin_facet_surface`: the finish word the event runs with and the (outward) normal it runs
// at. `n_out` is the macroscopic shading normal, `k` the incident wave normal, `lambda0`/`n0`
// the hero wavelength and its base index. The returned normal is `n_out` itself (bit for bit)
// unless the polish's halo branch drew a micro-normal.
fn surface_begin(
    word: u32,
    hit_point: vec3<f32>,
    facet_idx: u32,
    n_out: vec3<f32>,
    k: vec3<f32>,
    inside_gem: bool,
    lambda0: f32,
    n0: f32,
    rng_seed: u32,
    bounce: u32,
) -> SurfaceBegin {
    var result: SurfaceBegin;
    result.finish = word;
    result.normal = n_out;
    if (surf_is_ground(word)) {
        return result;
    }
    var w = word;
    let edge = material.inclusions.edge;
    if (edge.x > 0.0 && params.num_tools == 0u) {
        if (edge.y > 0.0 && surface_in_edge_band(hit_point, facet_idx, edge.x)) {
            result.finish = SURF_EDGE_GROUND_WORD;
            return result;
        }
        if (edge.z > 0.0) {
            // The TIS boost: the polish next to the band is duller than the facet's own.
            let distance = surface_edge_distance(hit_point, facet_idx);
            let fall = clamp((distance - edge.x) / edge.x, 0.0, 1.0);
            w = surf_with_rq_at_least(w, edge.z * (1.0 - fall));
        }
    }
    result.finish = w;
    if (!surf_has_roughness(w)) {
        return result;
    }
    // `roughness_event`: the halo draw at the hero's reflection scatter.
    let to_eye = -k;
    var n_side = -n_out;
    if (dot(to_eye, n_out) > 0.0) {
        n_side = n_out;
    }
    let cos_i = clamp(dot(to_eye, n_side), 0.0, 1.0);
    var n1 = 1.0;
    if (inside_gem) {
        n1 = n0;
    }
    let tis_hero = surf_tis_reflection(surf_rq_nm(w), n1, cos_i, lambda0);
    let p_hero = clamp(tis_hero, SURF_PDF_MIN, SURF_PDF_MAX);
    if (surface_unit_draw(rng_seed, bounce, SURFACE_HALO_STREAM) >= p_hero) {
        return result;
    }
    let across = surf_across_scratches(w, surface_machine(), n_out, rng_seed, bounce);
    let frame = ground_frame_new(n_side, across, true);
    var wi = ground_to_local(frame, to_eye);
    wi.z = max(wi.z, SURF_MIN_COSINE);
    let m = ggx_sample_visible(
        surf_halo_lobe(w),
        normalize(wi),
        surface_unit_draw(rng_seed, bounce, SURFACE_VNDF_U_STREAM),
        surface_unit_draw(rng_seed, bounce, SURFACE_VNDF_V_STREAM),
    );
    // The micro-normal faces the incident side; the dispatch wants it outward like `n_out`.
    let m_world = ground_to_world(frame, m);
    if (dot(n_side, n_out) > 0.0) {
        result.normal = m_world;
    } else {
        result.normal = -m_world;
    }
    return result;
}

struct SurfGroundParams {
    lobe: vec2<f32>,
    across: vec3<f32>,
    has_across: bool,
}

// What the ground arm's lobe is: `ground_event`. `normal` is the facing shading normal the arm
// runs with (the macroscopic one: a ground event never takes a micro-normal first).
fn surface_ground_params(
    word: u32,
    normal: vec3<f32>,
    inside_before: bool,
    rng_seed: u32,
    bounce: u32,
) -> SurfGroundParams {
    var gp: SurfGroundParams;
    gp.lobe = ggx_lobe_isotropic(SURF_DEFAULT_GROUND_ALPHA);
    gp.across = vec3<f32>(0.0, 0.0, 0.0);
    gp.has_across = false;
    if (word == SURF_EDGE_GROUND_WORD) {
        gp.lobe = ggx_lobe_isotropic(material.inclusions.edge.y);
        return gp;
    }
    gp.lobe = surf_ground_lobe(word);
    if (surf_anisotropy(word) > 1.0) {
        var n_out = normal;
        if (inside_before) {
            n_out = -normal;
        }
        gp.across = surf_across_scratches(word, surface_machine(), n_out, rng_seed, bounce);
        gp.has_across = true;
    }
    return gp;
}

// `finish_facet_surface`: after the arm, corrects the throughput of a polish with roughness for
// the branch that happened (halo or specular, reflected or transmitted), channel by channel,
// and kills a halo direction that ends on the wrong side of the macroscopic surface. A
// transmission's deferred exit split (`split_dir[k].w`, the intensity the split will carry) is
// scaled the same way. A no-op for every other event.
fn surface_finish_event(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
) {
    let word = (*h).finish;
    if (!surf_has_roughness(word)) {
        return;
    }
    // The macroscopic outward shading normal `bounce_surface` left in `hit.normal`.
    let n_out = (*h).hit.normal;
    let to_eye = -(*h).wave_dir_at_bounce;
    var n_side = -n_out;
    if (dot(to_eye, n_out) > 0.0) {
        n_side = n_out;
    }
    let cos_i = clamp(dot(to_eye, n_side), 0.0, 1.0);
    let rq = surf_rq_nm(word);
    let inside_before = (*h).pre_bounce_inside_gem;
    let reflected = inside_before == (*s).inside_gem;

    var n1_hero = 1.0;
    if (inside_before) {
        n1_hero = rc.n_o_hoisted[0];
    }
    let p_hero = clamp(surf_tis_reflection(rq, n1_hero, cos_i, (*s).lambdas[0]), SURF_PDF_MIN, SURF_PDF_MAX);
    let halo = surface_unit_draw(seed0, bounce, SURFACE_HALO_STREAM) < p_hero;
    // The outgoing masking of a halo direction; 1 for the specular branch.
    var masking: f32 = 1.0;
    if (halo) {
        let across = surf_across_scratches(word, surface_machine(), n_out, seed0, bounce);
        let frame = ground_frame_new(n_side, across, true);
        let along = dot(frame.n, (*s).k);
        var right_side = along < 0.0;
        if (reflected) {
            right_side = along > 0.0;
        }
        masking = 0.0;
        if (right_side) {
            masking = ggx_g1(surf_halo_lobe(word), ground_to_local(frame, (*s).k));
        }
    }
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        let n_c = rc.n_o_hoisted[k];
        var n1 = 1.0;
        var n2 = n_c;
        if (inside_before) {
            n1 = n_c;
            n2 = 1.0;
        }
        let lambda = (*s).lambdas[k];
        let tis_r = surf_tis_reflection(rq, n1, cos_i, lambda);
        var tis = tis_r;
        if (!reflected) {
            tis = surf_tis_transmission(rq, n1, n2, cos_i, lambda);
        }
        var scale: f32 = 1.0;
        var technique: f32 = 1.0;
        if (halo) {
            scale = tis / p_hero * masking;
            technique = clamp(tis_r, SURF_PDF_MIN, SURF_PDF_MAX);
        } else {
            scale = (1.0 - tis) / (1.0 - p_hero);
            technique = clamp(1.0 - tis_r, SURF_PDF_MIN, SURF_PDF_MAX);
        }
        (*s).stokes[k] = (*s).stokes[k] * scale;
        (*s).path_pdf[k] = (*s).path_pdf[k] * technique;
        if (!reflected && ((*h).split_mask & (1u << k)) != 0u) {
            (*h).split_dir[k].w = (*h).split_dir[k].w * scale;
        }
    }
}

fn bounce_surface(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    let hit = (*h).hit;
    var local_origin = (*s).origin;
    let current_origin = &local_origin;
    var local_dir = (*s).dir;
    let current_dir = &local_dir;
    var local_is_extraordinary = (*s).is_extraordinary;
    let is_extraordinary = &local_is_extraordinary;
    var local_stokes = (*s).stokes;
    let stokes = &local_stokes;
    var local_lambdas = (*s).lambdas;
    let lambdas = &local_lambdas;
    let is_anisotropic = rc.is_anisotropic;
    let is_biaxial = rc.is_biaxial;
    let c_axis = rc.c_axis;
    let n_alpha_hero = rc.n_alpha_hero;
    let n_beta_hero = rc.n_beta_hero;
    let n_gamma_hero = rc.n_gamma_hero;
    let biax_ax0 = rc.biax_ax0;
    let biax_ax1 = rc.biax_ax1;
    let biax_ax2 = rc.biax_ax2;
    let n_o_hero_seed = rc.n_o_hero_seed;
    let n_e_hero_seed = rc.n_e_hero_seed;
    let alpha_o_hoisted = rc.alpha_o_hoisted;
    let alpha_e_hoisted = rc.alpha_e_hoisted;
    let alpha_beta_hoisted = rc.alpha_beta_hoisted;

    let hit_point = (*current_origin) + hit.t * (*current_dir);
    // See shading_normal_near_edge's own doc comment. `shading_normal_for_hit` is that
    // function unchanged for a planar stone and the concave-stone rule otherwise.
    var normal = shading_normal_for_hit(hit_point, hit.facet_idx, hit.normal, material.edge_rounding_radius);
    // Surface finish (see this file's header): the facet word, and the micro-normal of a polish's
    // halo draw in place of the macroscopic normal. Resolved before the plane-of-incidence frame
    // below is built, exactly as the CPU's `facet_event` does.
    var finish: u32 = 0u;
    if (hit.facet_idx < arrayLength(&facet_finishes)) {
        finish = facet_finishes[hit.facet_idx];
    }
    let begun = surface_begin(
        finish, hit_point, hit.facet_idx, normal, (*s).k, (*s).inside_gem,
        (*s).lambdas[0], rc.n_o_hoisted[0], seed0, bounce,
    );
    // `hit.normal` (the unrounded plane normal) is dead after this phase; it carries the
    // macroscopic outward shading normal on to `surface_finish_event`, which needs it after the
    // arm has run, whether or not the event took a micro-normal.
    (*h).hit.normal = normal;
    normal = begun.normal;
    finish = begun.finish;
    // `wave_dir_at_bounce` is the wave normal `k` (== (*current_k)), used for
    // EVERY index lookup, cos_i/sin_i, Snell/Fresnel evaluation, TIR decision, and
    // the Stokes plane-of-incidence frame below -- see
    // optics::raytracer::refraction's "wave normal vs Poynting direction" design
    // note. The Poynting/energy direction `S` (== (*current_dir)) is used directly
    // (geometric origin-advance/intersection only) where still needed.
    // `wave_dir_at_bounce == (*current_dir)` trivially outside the crystal and for the
    // uniaxial ordinary eigenmode, so every such case is bit-identical to the plain
    // all-`(*current_dir)` code.
    let wave_dir_at_bounce = (*s).k;

    // Plane-of-incidence frame rotation (signed psi via atan2).
    let cpn_raw = cross(wave_dir_at_bounce, normal);
    let cpn_len2 = dot(cpn_raw, cpn_raw);
    var current_plane_normal: vec3<f32>;
    if (cpn_len2 > 0.0) {
        current_plane_normal = cpn_raw / sqrt(cpn_len2);
    } else {
        current_plane_normal = vec3<f32>(0.0, 0.0, 0.0);
    }
    if ((*s).have_prev_plane_normal && cpn_len2 > 1e-6 && dot((*s).prev_plane_normal, (*s).prev_plane_normal) > 1e-6) {
        let psi = signed_frame_rotation_psi((*s).prev_plane_normal, current_plane_normal, wave_dir_at_bounce);
        let rot = mueller_frame_rotation(psi);
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            local_stokes[k] = rot * local_stokes[k];
            (*s).stokes[k] = local_stokes[k];
        }
    }
    (*s).prev_plane_normal = current_plane_normal;
    (*s).have_prev_plane_normal = true;

    if ((*s).inside_gem) {
        normal = -normal;
        // A scattering-active material's extinction for this segment was
        // already applied above (in the `maybe_scatter_or_extinguish` no-scatter
        // branch) -- applying the plain absorption loop here too would charge
        // this segment's absorption TWICE. `!material_scatters()` (no haze and no oriented
        // inclusions, the CPU's `GemMaterial::scatters()`) is exactly the pre-Task-1 case
        // (including every existing scene), where the block above never ran and this is the
        // ONLY absorption application, matching the CPU's identical guard.
        if (!material_scatters()) {
            // P1 (assigned-mode absorption): optics::raytracer::absorption::
            // channel_absorption_alphas_assigned. `(*is_extraordinary)` names which
            // eigenmode this path was assigned to at its most recent air->crystal
            // entry -- computed fresh from the CURRENT wave normal
            // (`wave_dir_at_bounce`) every bounce, not read off `(*stokes)[k]`'s
            // (possibly azimuth-drifted) Stokes state. The isotropic branch is the
            // Stokes-independent eigenmode midpoint -- see the scatter block's own
            // comment above.
            var alphas_interior: array<f32, 8>;
            for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                alphas_interior[k] = 0.0;
            }
            if (is_anisotropic) {
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    if (is_biaxial && material.has_beta_ray != 0u) {
                        alphas_interior[k] = assigned_mode_alpha_biaxial(
                            alpha_o_hoisted[k], alpha_beta_hoisted[k], alpha_e_hoisted[k],
                            n_alpha_hero, n_beta_hero, n_gamma_hero, biax_ax0, biax_ax1, biax_ax2,
                            c_axis, wave_dir_at_bounce, (*is_extraordinary),
                        );
                    } else {
                        alphas_interior[k] = assigned_mode_alpha_uniaxial(
                            alpha_o_hoisted[k], alpha_e_hoisted[k], c_axis, wave_dir_at_bounce, (*is_extraordinary),
                            n_o_hero_seed, n_e_hero_seed,
                        );
                    }
                }
            } else {
                let eigen_a = ordinary_eigen_polarization(wave_dir_at_bounce, c_axis);
                let eigen_b = extraordinary_eigen_polarization(wave_dir_at_bounce, c_axis);
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    alphas_interior[k] = isotropic_channel_alpha(
                        alpha_o_hoisted[k], alpha_e_hoisted[k], c_axis, eigen_a, eigen_b,
                    );
                }
            }
            for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                // (absorption path scale): mirrors
                // optics::raytracer::absorption::apply_absorption's own
                // `path_len * ctx.material.absorption_path_scale` multiply exactly.
                let scaled_hit_t = hit.t * material.absorption_path_scale;
                // `exp_poly`, not the `exp()` builtin --
                // `apply_absorption` calls `crate::simd::exp_f32x8`, not `f32::exp`.
                let trans_factor = exp_poly(-alphas_interior[k] * scaled_hit_t);
                local_stokes[k] = local_stokes[k] * trans_factor;
            }
        }
        (*s).stokes = local_stokes;
    }

    // Which specular/diffuse treatment
    // this facet gets. Bounds-checked (mirrors optics::raytracer::trace_spectral_ray_inner's
    // `facet_finishes.get(hit_rec.facet_idx).copied().unwrap_or_default()`): an
    // index past the end of a shorter-than-`planes` buffer defaults to
    // `facet_finish::POLISHED`, exactly as the CPU's `Default` does.
    // `select`'s two value arguments are BOTH evaluated in WGSL (no short-circuit),
    // so an `if` guard is used instead of `select` here -- indexing
    // `facet_finishes[hit.facet_idx]` unconditionally would attempt an
    // out-of-bounds storage-buffer read whenever `facet_idx >= arrayLength(...)`
    // (harmless per WGSL's robustness guarantees, but its RESULT is
    // implementation-defined, so this must never be allowed to influence `finish`).
    // (`finish` was looked up, and resolved by `surface_begin`, at the top of this function.)
    // Captured before any of the three bounce-dispatch arms below run -- see
    // optics::raytracer::dispatch_bounce's doc comment for why `was_internal_reflection`
    // is always computed against the PRE-bounce `(*inside_gem)`. The TIR/reflect/refract
    // arms below never need this explicitly (they either never touch `(*inside_gem)` at
    // all, or only flip it once at their very end, after which nothing else in that
    // arm reads it this bounce) -- only the frosted arm, which can update `(*inside_gem)`
    // itself, needs the pre-bounce value spelled out separately.
    let pre_bounce_inside_gem = (*s).inside_gem;

    (*h).hit_point = hit_point;
    (*h).shading_normal = normal;
    (*h).wave_dir_at_bounce = wave_dir_at_bounce;
    (*h).finish = finish;
    (*h).pre_bounce_inside_gem = pre_bounce_inside_gem;

    let idx = bounce_index_setup(s, h, rc);

    if (surf_is_ground(finish)) {
        (*h).arm = ARM_FROSTED;
    } else if (idx.sin2_t > 1.0) {
        (*h).arm = ARM_TIR;
    } else if (idx.uniaxial_nondegenerate) {
        if (!(*s).inside_gem) {
            (*h).arm = ARM_UNI_ENTRY;
        } else {
            (*h).arm = ARM_UNI_INTERNAL;
        }
    } else {
        (*h).arm = ARM_GENERIC;
    }

    return PHASE_NEXT;
}

