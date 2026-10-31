// ---------------------------------------------------------------------------------
// Surface finish GPU port (lane RP-G2): Tier 2 kernels for `transport_physics/13_surface_ggx.wgsl`,
// the WGSL twin of `optics::raytracer::surface`. Each kernel calls the SAME physics function
// the megakernel and the event wavefront call; only the energy-compensation table is read
// from this file's own binding (the shipped kernels read it from the tail of `dist_func`).
// ---------------------------------------------------------------------------------

// ---- surf_lobe_main: the GGX lobe (ggx.rs) ----

struct SurfLobeCase {
    alpha_x: f32,
    alpha_y: f32,
    u1: f32,
    u2: f32,
    wi: vec3<f32>,
    _pad0: f32,
    wo: vec3<f32>,
    _pad1: f32,
    m: vec3<f32>,
    _pad2: f32,
}

@group(0) @binding(110) var<storage, read> surf_lobe_cases: array<SurfLobeCase>;
// 8 floats per case: [0] d(m), [1] lambda(wi), [2] g1(wi), [3] g2(wi, wo),
// [4] visible_density(wi, m), [5..8) sample_visible(wi, u1, u2).
@group(0) @binding(111) var<storage, read_write> surf_lobe_out: array<f32>;

@compute @workgroup_size(64)
fn surf_lobe_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&surf_lobe_cases)) {
        return;
    }
    let c = surf_lobe_cases[idx];
    let lobe = vec2<f32>(c.alpha_x, c.alpha_y);
    let sampled = ggx_sample_visible(lobe, c.wi, c.u1, c.u2);
    let base = idx * 8u;
    surf_lobe_out[base + 0u] = ggx_d(lobe, c.m);
    surf_lobe_out[base + 1u] = ggx_lambda(lobe, c.wi);
    surf_lobe_out[base + 2u] = ggx_g1(lobe, c.wi);
    surf_lobe_out[base + 3u] = ggx_g2(lobe, c.wi, c.wo);
    surf_lobe_out[base + 4u] = ggx_visible_density(lobe, c.wi, c.m);
    surf_lobe_out[base + 5u] = sampled.x;
    surf_lobe_out[base + 6u] = sampled.y;
    surf_lobe_out[base + 7u] = sampled.z;
}

// ---- surf_tis_main: total integrated scatter and the halo width (tis.rs, facet_surface.rs) ----

struct SurfTisCase {
    rq_nm: f32,
    n1: f32,
    n2: f32,
    cos_i: f32,
    lambda_nm: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(112) var<storage, read> surf_tis_cases: array<SurfTisCase>;
// 4 floats per case: [0] tis_reflection, [1] tis_transmission, [2] halo_alpha(rq), [3] fresnel.
@group(0) @binding(113) var<storage, read_write> surf_tis_out: array<f32>;

@compute @workgroup_size(64)
fn surf_tis_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&surf_tis_cases)) {
        return;
    }
    let c = surf_tis_cases[idx];
    let base = idx * 4u;
    surf_tis_out[base + 0u] = surf_tis_reflection(c.rq_nm, c.n1, c.cos_i, c.lambda_nm);
    surf_tis_out[base + 1u] = surf_tis_transmission(c.rq_nm, c.n1, c.n2, c.cos_i, c.lambda_nm);
    surf_tis_out[base + 2u] = surf_halo_alpha(c.rq_nm);
    surf_tis_out[base + 3u] = surf_fresnel(c.n1, c.n2, c.cos_i);
}

// ---- surf_word_main: the packed word and the scratch direction (facet_surface.rs, sweep.rs) ----

struct SurfWordCase {
    word: u32,
    rq_floor: f32,
    sweep_u: f32,
    _pad0: f32,
    machine: vec3<f32>,
    _pad1: f32,
    normal: vec3<f32>,
    _pad2: f32,
}

@group(0) @binding(114) var<storage, read> surf_word_cases: array<SurfWordCase>;
// 12 floats per case: [0] kind, [1] rq_nm, [2] ground_alpha, [3] anisotropy, [4] custom angle (deg),
// [5] scratch angle (deg), [6..9) across-scratches direction, [9] low 16 bits and [10] high 16 bits
// of `with_rq_at_least(word, rq_floor)`, [11] 1 if the word has roughness.
@group(0) @binding(115) var<storage, read_write> surf_word_out: array<f32>;

@compute @workgroup_size(64)
fn surf_word_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&surf_word_cases)) {
        return;
    }
    let c = surf_word_cases[idx];
    let base = idx * 12u;
    let angle = surf_scratch_angle_deg(c.word, c.machine, c.sweep_u);
    let across = cross(c.normal, surf_scratch_direction(c.normal, angle));
    let raised = surf_with_rq_at_least(c.word, c.rq_floor);
    surf_word_out[base + 0u] = f32(surf_kind(c.word));
    surf_word_out[base + 1u] = surf_rq_nm(c.word);
    surf_word_out[base + 2u] = surf_ground_alpha(c.word);
    surf_word_out[base + 3u] = surf_anisotropy(c.word);
    surf_word_out[base + 4u] = surf_custom_angle_deg(c.word);
    surf_word_out[base + 5u] = angle;
    surf_word_out[base + 6u] = across.x;
    surf_word_out[base + 7u] = across.y;
    surf_word_out[base + 8u] = across.z;
    surf_word_out[base + 9u] = f32(raised & 0xffffu);
    surf_word_out[base + 10u] = f32(raised >> 16u);
    surf_word_out[base + 11u] = select(0.0, 1.0, surf_has_roughness(c.word));
}

// ---- surf_albedo_main / surf_ground_main: the energy compensation and the ground event (ground.rs) ----

// The compensation table, as `compensation::table_values` lays it out.
@group(0) @binding(116) var<storage, read> surf_ground_table: array<f32>;

// `ground_albedo` against this file's table binding (the shipped kernels read the tail of
// `dist_func`); the cell and the blend are the shared physics functions.
fn tf_ground_albedo(alpha: f32, cos_i: f32, eta_rel: f32) -> f32 {
    let cell = ground_albedo_cell(alpha, cos_i, eta_rel);
    return ground_albedo_blend(
        cell,
        surf_ground_table[ground_cell_index(cell, 0u, 0u, 0u)],
        surf_ground_table[ground_cell_index(cell, 0u, 0u, 1u)],
        surf_ground_table[ground_cell_index(cell, 0u, 1u, 0u)],
        surf_ground_table[ground_cell_index(cell, 0u, 1u, 1u)],
        surf_ground_table[ground_cell_index(cell, 1u, 0u, 0u)],
        surf_ground_table[ground_cell_index(cell, 1u, 0u, 1u)],
        surf_ground_table[ground_cell_index(cell, 1u, 1u, 0u)],
        surf_ground_table[ground_cell_index(cell, 1u, 1u, 1u)],
    );
}

struct SurfAlbedoCase {
    alpha: f32,
    cos_i: f32,
    eta_rel: f32,
    _pad0: f32,
}

@group(0) @binding(117) var<storage, read> surf_albedo_cases: array<SurfAlbedoCase>;
@group(0) @binding(118) var<storage, read_write> surf_albedo_out: array<f32>;

@compute @workgroup_size(64)
fn surf_albedo_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&surf_albedo_cases)) {
        return;
    }
    let c = surf_albedo_cases[idx];
    surf_albedo_out[idx] = tf_ground_albedo(c.alpha, c.cos_i, c.eta_rel);
}

struct SurfGroundCase {
    normal: vec3<f32>,
    alpha_x: f32,
    k_in: vec3<f32>,
    alpha_y: f32,
    across: vec3<f32>,
    has_across: u32,
    o_dir: vec3<f32>,
    reflect_eval: u32,
    n1: f32,
    n2: f32,
    cos_i: f32,
    sin2_t: f32,
    u1: f32,
    u2: f32,
    u_branch: f32,
    _pad0: f32,
}

@group(0) @binding(119) var<storage, read> surf_ground_cases: array<SurfGroundCase>;
// 12 floats per case: [0..3) sampled direction, [3] crossed, [4] reflect branch, [5] weight,
// [6] branch probability, [7] pdf, [8] valid, [9] eval ok, [10] eval weight, [11] eval pdf.
@group(0) @binding(120) var<storage, read_write> surf_ground_out: array<f32>;

@compute @workgroup_size(64)
fn surf_ground_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&surf_ground_cases)) {
        return;
    }
    let c = surf_ground_cases[idx];
    var ctx = ground_ctx_new(
        c.normal, c.k_in, vec2<f32>(c.alpha_x, c.alpha_y), c.across, c.has_across != 0u,
        c.n1, c.n2, c.cos_i, c.sin2_t,
    );
    ctx.compensation = 1.0 / tf_ground_albedo(ggx_alpha_mean(ctx.lobe), ctx.i.z, ctx.n2 / ctx.n1);
    let sample = ground_sample(ctx, c.u1, c.u2, c.u_branch);
    let ev = ground_eval(ctx, c.o_dir, c.reflect_eval != 0u);
    let base = idx * 12u;
    surf_ground_out[base + 0u] = sample.dir.x;
    surf_ground_out[base + 1u] = sample.dir.y;
    surf_ground_out[base + 2u] = sample.dir.z;
    surf_ground_out[base + 3u] = select(0.0, 1.0, sample.crossed);
    surf_ground_out[base + 4u] = select(0.0, 1.0, sample.reflect_branch);
    surf_ground_out[base + 5u] = sample.weight;
    surf_ground_out[base + 6u] = sample.branch_prob;
    surf_ground_out[base + 7u] = sample.pdf;
    surf_ground_out[base + 8u] = select(0.0, 1.0, sample.valid);
    surf_ground_out[base + 9u] = select(0.0, 1.0, ev.ok);
    surf_ground_out[base + 10u] = ev.weight;
    surf_ground_out[base + 11u] = ev.pdf;
}
