// ---------------------------------------------------------------------------------
// Tier 2 kernel for the Bragg domain port (`transport_physics/12_bragg_domains.wgsl`): the domain
// at a point, the reflection coefficient of every channel there, the delta tracker, the mosaic
// mirror and `maybe_scatter_oriented_bragg` (haze-only medium), each compared against the CPU
// functions of `optics::inclusions::{domains, bragg}` and
// `optics::raytracer::scattering::delta_tracking` by `renderer::gpu::transport_check::bragg`.
// ---------------------------------------------------------------------------------

// One case: the structure as `structure_rows` encodes it, a point to query the domain map at, a
// segment to track and a ray for the whole estimator. 384 bytes, no padding.
struct BraggCase {
    // [domain_seed, rng_seed, bounce, 0]
    header: vec4<u32>,
    row0: vec4<f32>,
    row1: vec4<f32>,
    row2: vec4<f32>,
    // The point whose domain is queried.
    p: vec4<f32>,
    // The segment's origin (xyz) and the facet distance `hit_t` of the estimator (w).
    origin: vec4<f32>,
    // The segment's unit direction (xyz) and the tracked length `t_max` (w).
    dir: vec4<f32>,
    // [sigma_hg, g, path_scale, 0]: the haze of the medium.
    haze: vec4<f32>,
    lambdas: array<f32, 8>,
    alphas: array<f32, 8>,
    stokes_in: array<vec4<f32>, 8>,
    path_pdf_in: array<f32, 8>,
    // Synthetic zoned optical depths per channel at the tracked `t_model`, for
    // `bragg_reflect_weights4`.
    depth_in: array<f32, 8>,
}

@group(0) @binding(130) var<storage, read> bragg_cases: array<BraggCase>;
// Layout per case, 83 floats: [0] domain on (0.0/1.0), [1] lambda_b0, [2..5) normal,
// [5..13) sigma per channel along `dir` in that domain, [13] reflect (0.0/1.0), [14] t_model,
// [15..18) tracked normal, [18..26) channel weights, [26..29) mirrored direction (of `dir` in
// the queried domain's normal), [29] scattered, [30] bragg, [31] t_free, [32..35) new_dir,
// [35..67) stokes (8 vec4s), [67..75) path_pdf, [75..83) the zoned reflection weights
// (`bragg_reflect_weights4` of `depth_in` and the tracked weights).
@group(0) @binding(131) var<storage, read_write> bragg_out: array<f32>;

@compute @workgroup_size(64)
fn bragg_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&bragg_cases)) {
        return;
    }
    let c = bragg_cases[idx];
    let st = bragg_structure(c.header.x, c.row0, c.row1, c.row2);
    let base = idx * 83u;
    let dir = c.dir.xyz;

    let dom = bragg_domain_at(st, c.p.xyz);
    bragg_out[base] = select(0.0, 1.0, dom.on);
    bragg_out[base + 1u] = dom.lambda_b0;
    bragg_out[base + 2u] = dom.normal.x;
    bragg_out[base + 3u] = dom.normal.y;
    bragg_out[base + 4u] = dom.normal.z;
    let cos_theta = abs(dot(dir, dom.normal));
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        bragg_out[base + 5u + k] = bragg_sigma(st, dom, cos_theta, c.lambdas[k]);
    }

    let lam_lo = vec4<f32>(c.lambdas[0], c.lambdas[1], c.lambdas[2], c.lambdas[3]);
    let lam_hi = vec4<f32>(c.lambdas[4], c.lambdas[5], c.lambdas[6], c.lambdas[7]);
    let tr = bragg_track_hero(st, c.origin.xyz, dir, c.dir.w, lam_lo, lam_hi, c.header.y, c.header.z);
    bragg_out[base + 13u] = f32(tr.reflect);
    bragg_out[base + 14u] = tr.t_model;
    bragg_out[base + 15u] = tr.normal.x;
    bragg_out[base + 16u] = tr.normal.y;
    bragg_out[base + 17u] = tr.normal.z;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        bragg_out[base + 18u + k] = bragg_channel_weight(tr, k);
    }

    let mirrored = bragg_reflect_dir(dir, dom.normal, st.mosaic, c.header.y, c.header.z);
    bragg_out[base + 26u] = mirrored.x;
    bragg_out[base + 27u] = mirrored.y;
    bragg_out[base + 28u] = mirrored.z;

    let zero4 = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    let m = OrientedMedium(
        0u, c.haze.x, c.haze.y, c.haze.z,
        zero4, zero4, zero4, zero4,
        zero4, zero4, zero4, zero4,
    );
    var stokes: array<vec4<f32>, 8> = c.stokes_in;
    var path_pdf: array<f32, 8> = c.path_pdf_in;
    let r = maybe_scatter_oriented_bragg(
        m, st, c.lambdas, c.alphas, c.origin.xyz, dir, c.origin.w, c.header.y, c.header.z,
        &stokes, &path_pdf,
    );
    bragg_out[base + 29u] = f32(r.scattered);
    bragg_out[base + 30u] = f32(r.bragg);
    bragg_out[base + 31u] = r.t_free;
    bragg_out[base + 32u] = r.new_dir.x;
    bragg_out[base + 33u] = r.new_dir.y;
    bragg_out[base + 34u] = r.new_dir.z;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        bragg_out[base + 35u + k * 4u + 0u] = stokes[k].x;
        bragg_out[base + 35u + k * 4u + 1u] = stokes[k].y;
        bragg_out[base + 35u + k * 4u + 2u] = stokes[k].z;
        bragg_out[base + 35u + k * 4u + 3u] = stokes[k].w;
    }
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        bragg_out[base + 67u + k] = path_pdf[k];
    }

    let depth_lo = vec4<f32>(c.depth_in[0], c.depth_in[1], c.depth_in[2], c.depth_in[3]);
    let depth_hi = vec4<f32>(c.depth_in[4], c.depth_in[5], c.depth_in[6], c.depth_in[7]);
    let zw = bragg_reflect_weights4(depth_lo, depth_hi, tr);
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        bragg_out[base + 75u + k] = bragg_channel_weight(zw, k);
    }
}
