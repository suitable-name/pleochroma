// ---------------------------------------------------------------------------------
// Tier 2 kernel for the oriented scattering port (`transport_physics/11_oriented_scattering.wgsl`):
// per-channel scattering coefficient, the mixture phase of every channel, the mixture direction
// sampler and `maybe_scatter_oriented`, each compared against the CPU functions of
// `optics::raytracer::scattering::oriented` by `renderer::gpu::transport_check::oriented`.
// ---------------------------------------------------------------------------------

// One case: the medium as `GpuInclusionBlock` encodes it, an incoming direction, a scattered
// direction to evaluate the phases at, three randoms for the direction sampler, and the
// per-ray inputs of `maybe_scatter_oriented`. 432 bytes, no padding.
struct OrientedCase {
    // [family_count, rng_seed, bounce, 0]
    header: vec4<u32>,
    dir0: vec4<f32>,
    dir1: vec4<f32>,
    dir2: vec4<f32>,
    dir3: vec4<f32>,
    par0: vec4<f32>,
    par1: vec4<f32>,
    par2: vec4<f32>,
    par3: vec4<f32>,
    // [sigma_hg, g, path_scale, hit_t]
    haze: vec4<f32>,
    w_i: vec4<f32>,
    w_o: vec4<f32>,
    // [u_c, u1, u2, 0]
    u: vec4<f32>,
    lambdas: array<f32, 8>,
    alphas: array<f32, 8>,
    stokes_in: array<vec4<f32>, 8>,
    path_pdf_in: array<f32, 8>,
}

@group(0) @binding(110) var<storage, read> oriented_cases: array<OrientedCase>;
// Layout per case, 65 floats: [0..8) sigma_s per channel (model units), [8] hero phase,
// [9..17) phase per channel, [17..20) sample_direction, [20] scattered (0.0/1.0), [21] t_free,
// [22..25) new_dir, [25..57) stokes (8 vec4s), [57..65) path_pdf.
@group(0) @binding(111) var<storage, read_write> oriented_out: array<f32>;

@compute @workgroup_size(64)
fn oriented_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&oriented_cases)) {
        return;
    }
    let c = oriented_cases[idx];
    let m = OrientedMedium(
        c.header.x, c.haze.x, c.haze.y, c.haze.z,
        c.dir0, c.dir1, c.dir2, c.dir3,
        c.par0, c.par1, c.par2, c.par3,
    );
    let w_i = c.w_i.xyz;
    let w_o = c.w_o.xyz;
    let base = idx * 65u;

    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        oriented_out[base + k] = ori_sigma_s(m, w_i, c.lambdas[k]);
    }

    var per_channel: array<f32, 8>;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        per_channel[k] = 0.0;
    }
    let hero_phase = ori_channel_phases(m, c.lambdas, w_i, w_o, &per_channel);
    oriented_out[base + 8u] = hero_phase;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        oriented_out[base + 9u + k] = per_channel[k];
    }

    let sampled = ori_sample_direction(m, w_i, c.lambdas[0], c.u.x, c.u.y, c.u.z);
    oriented_out[base + 17u] = sampled.x;
    oriented_out[base + 18u] = sampled.y;
    oriented_out[base + 19u] = sampled.z;

    var stokes: array<vec4<f32>, 8> = c.stokes_in;
    var path_pdf: array<f32, 8> = c.path_pdf_in;
    let result = maybe_scatter_oriented(
        m, c.lambdas, c.alphas, w_i, c.haze.w, c.header.y, c.header.z, &stokes, &path_pdf,
    );
    oriented_out[base + 20u] = f32(result.scattered);
    oriented_out[base + 21u] = result.t_free;
    oriented_out[base + 22u] = result.new_dir.x;
    oriented_out[base + 23u] = result.new_dir.y;
    oriented_out[base + 24u] = result.new_dir.z;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        oriented_out[base + 25u + k * 4u + 0u] = stokes[k].x;
        oriented_out[base + 25u + k * 4u + 1u] = stokes[k].y;
        oriented_out[base + 25u + k * 4u + 2u] = stokes[k].z;
        oriented_out[base + 25u + k * 4u + 3u] = stokes[k].w;
    }
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        oriented_out[base + 57u + k] = path_pdf[k];
    }
}
