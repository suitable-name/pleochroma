// The staged megakernel: the loop of `transport_main` cut into bounce stages, with survivor
// compaction between them. The host side is `renderer::gpu::frame::staged`.
//
// Stage 0 runs the megakernel up to bounce `stage.end_bounce`, one thread per (pixel, sample)
// tuple. A ray still alive there appends its state to `stage_rays` and its thread ends without
// finalizing, so a few long paths do not hold up the warp. `transport_stage_prepare`
// (`stage_prepare.wgsl`) turns the survivor count into indirect dispatch arguments. The next stage
// runs one thread per survivor: it reloads the state, rebuilds the per-ray constants as
// `wavefront_bounce` does, runs its bounces, and either parks again or finalizes the tuple.
//
// A sample does not depend on the stages: the state is stored exactly (bit casts), every draw is
// keyed by the tuple's seed and its bounce index, and `out_xyz[idx]` is written once by the stage
// that finalizes the ray. A full survivor region is not an error: the ray keeps tracing in its own
// thread, so the region size is a speed setting only. The survivor slot order depends on
// scheduling, and nothing reads it.

struct StageParams {
    // 0: fresh rays, one thread per tuple. 1: resume, one thread per survivor record.
    mode: u32,
    // The first bounce this stage runs (0 for stage 0).
    first_bounce: u32,
    // A ray alive with `bounce == end_bounce` parks. 0xFFFFFFFF: the last stage, never parks.
    end_bounce: u32,
    // Resume: the word offset of the records this stage reads, their counter and capacity.
    in_base: u32,
    in_counter: u32,
    in_capacity: u32,
    // The word offset of the records this stage writes, their counter and capacity.
    out_base: u32,
    out_counter: u32,
    out_capacity: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
}

// Words per survivor record (352 bytes). Word 0 is the tuple index, word 1 the flags, words 2-19
// six vec3 (origin, direction, wave normal, previous plane normal, light-MIS direction, NEE total),
// word 20 the pending light-MIS weight, words 24-55 the Stokes vectors, and words 56-87 the
// radiance, path pdf, split radiance and compat (8 words each). Words 21-23 are unused.
const STAGE_WORDS: u32 = 88u;
const STAGE_NEVER_PARKS: u32 = 0xFFFFFFFFu;

@group(0) @binding(150) var<uniform> stage: StageParams;
@group(0) @binding(151) var<storage, read_write> stage_rays: array<u32>;
@group(0) @binding(152) var<storage, read_write> stage_counters: array<atomic<u32>>;

fn stage_put_f(at: u32, v: f32) {
    stage_rays[at] = bitcast<u32>(v);
}

fn stage_put_v3(at: u32, v: vec3<f32>) {
    stage_rays[at] = bitcast<u32>(v.x);
    stage_rays[at + 1u] = bitcast<u32>(v.y);
    stage_rays[at + 2u] = bitcast<u32>(v.z);
}

fn stage_get_f(at: u32) -> f32 {
    return bitcast<f32>(stage_rays[at]);
}

fn stage_get_v3(at: u32) -> vec3<f32> {
    return vec3<f32>(
        bitcast<f32>(stage_rays[at]),
        bitcast<f32>(stage_rays[at + 1u]),
        bitcast<f32>(stage_rays[at + 2u]),
    );
}

const STAGE_FLAG_INSIDE_GEM: u32 = 0x1u;
const STAGE_FLAG_IS_EXTRAORDINARY: u32 = 0x2u;
const STAGE_FLAG_HAVE_PREV_PLANE_NORMAL: u32 = 0x4u;
const STAGE_FLAG_PATH_ESCAPED: u32 = 0x8u;

@compute @workgroup_size(64)
fn transport_stage_main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    // The workgroup-shared plane fill of `transport_main`, before any early return.
    let plane_count = scene_plane_count();
    if (plane_count <= PLANES_SHARED_CAPACITY) {
        var fill_i = local_index;
        loop {
            if (fill_i >= plane_count) {
                break;
            }
            planes_shared[fill_i] = planes[fill_i];
            fill_i = fill_i + 64u;
        }
    }
    workgroupBarrier();

    let resume = stage.mode == 1u;
    var idx: u32 = gid.x;
    var in_at: u32 = 0u;
    if (resume) {
        let survivors = min(atomicLoad(&stage_counters[stage.in_counter]), stage.in_capacity);
        if (gid.x >= survivors) {
            return;
        }
        in_at = stage.in_base + gid.x * STAGE_WORDS;
        idx = stage_rays[in_at];
    } else {
        let total = params.num_pixels * camera.num_samples;
        if (idx >= total) {
            return;
        }
    }

    // Deterministic generation of the tuple: the same draws in every stage. It also binds the
    // estimator of the tuple, so it must come before `transport_bounce_step`.
    let gen = transport_generate_ray(idx);
    var lambdas: array<f32, 8> = gen.lambdas;
    let seed0 = gen.seed0;
    let rc = transport_hoist_ray_constants(lambdas);
    let observer = -gen.dir;

    var stokes: array<vec4<f32>, 8> = array<vec4<f32>, 8>();
    var radiance: array<f32, 8> = array<f32, 8>();
    var path_pdf: array<f32, 8> = array<f32, 8>();
    var split_radiance: array<f32, 8> = array<f32, 8>();
    var compat: array<u32, 8> = array<u32, 8>();
    var current_origin = gen.origin;
    var current_dir = gen.dir;
    var current_k = gen.dir;
    var inside_gem = false;
    var is_extraordinary = false;
    var prev_plane_normal = vec3<f32>(0.0, 0.0, 0.0);
    var have_prev_plane_normal = false;
    var path_escaped = false;
    var nee_xyz = vec3<f32>(0.0, 0.0, 0.0);
    var pending_light_mis: f32 = 0.0;
    var pending_light_mis_dir = vec3<f32>(0.0, 0.0, 0.0);

    if (resume) {
        let flags = stage_rays[in_at + 1u];
        inside_gem = (flags & STAGE_FLAG_INSIDE_GEM) != 0u;
        is_extraordinary = (flags & STAGE_FLAG_IS_EXTRAORDINARY) != 0u;
        have_prev_plane_normal = (flags & STAGE_FLAG_HAVE_PREV_PLANE_NORMAL) != 0u;
        path_escaped = (flags & STAGE_FLAG_PATH_ESCAPED) != 0u;
        current_origin = stage_get_v3(in_at + 2u);
        current_dir = stage_get_v3(in_at + 5u);
        current_k = stage_get_v3(in_at + 8u);
        prev_plane_normal = stage_get_v3(in_at + 11u);
        pending_light_mis_dir = stage_get_v3(in_at + 14u);
        nee_xyz = stage_get_v3(in_at + 17u);
        pending_light_mis = stage_get_f(in_at + 20u);
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            let s_at = in_at + 24u + k * 4u;
            stokes[k] = vec4<f32>(
                stage_get_f(s_at),
                stage_get_f(s_at + 1u),
                stage_get_f(s_at + 2u),
                stage_get_f(s_at + 3u),
            );
            radiance[k] = stage_get_f(in_at + 56u + k);
            path_pdf[k] = stage_get_f(in_at + 64u + k);
            split_radiance[k] = stage_get_f(in_at + 72u + k);
            compat[k] = stage_rays[in_at + 80u + k];
        }
    } else {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            stokes[k] = vec4<f32>(1.0, 0.0, 0.0, 0.0);
            radiance[k] = 0.0;
            path_pdf[k] = 1.0;
            split_radiance[k] = 0.0;
            compat[k] = 0xFFu;
        }
    }

    var may_park = stage.end_bounce != STAGE_NEVER_PARKS;
    var bounce: u32 = stage.first_bounce;
    loop {
        if (bounce >= params.max_bounces) {
            break;
        }
        if (may_park && bounce >= stage.end_bounce) {
            // One attempt per ray. A full region leaves the ray in this thread to the end.
            may_park = false;
            let slot = atomicAdd(&stage_counters[stage.out_counter], 1u);
            if (slot < stage.out_capacity) {
                let out_at = stage.out_base + slot * STAGE_WORDS;
                var flags: u32 = 0u;
                if (inside_gem) {
                    flags = flags | STAGE_FLAG_INSIDE_GEM;
                }
                if (is_extraordinary) {
                    flags = flags | STAGE_FLAG_IS_EXTRAORDINARY;
                }
                if (have_prev_plane_normal) {
                    flags = flags | STAGE_FLAG_HAVE_PREV_PLANE_NORMAL;
                }
                if (path_escaped) {
                    flags = flags | STAGE_FLAG_PATH_ESCAPED;
                }
                stage_rays[out_at] = idx;
                stage_rays[out_at + 1u] = flags;
                stage_put_v3(out_at + 2u, current_origin);
                stage_put_v3(out_at + 5u, current_dir);
                stage_put_v3(out_at + 8u, current_k);
                stage_put_v3(out_at + 11u, prev_plane_normal);
                stage_put_v3(out_at + 14u, pending_light_mis_dir);
                stage_put_v3(out_at + 17u, nee_xyz);
                stage_put_f(out_at + 20u, pending_light_mis);
                for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
                    let s_at = out_at + 24u + k * 4u;
                    stage_put_f(s_at, stokes[k].x);
                    stage_put_f(s_at + 1u, stokes[k].y);
                    stage_put_f(s_at + 2u, stokes[k].z);
                    stage_put_f(s_at + 3u, stokes[k].w);
                    stage_put_f(out_at + 56u + k, radiance[k]);
                    stage_put_f(out_at + 64u + k, path_pdf[k]);
                    stage_put_f(out_at + 72u + k, split_radiance[k]);
                    stage_rays[out_at + 80u + k] = compat[k];
                }
                return;
            }
        }
        let status = transport_bounce_step(
            bounce, seed0, &lambdas, rc.c_axis, rc.birefringence_delta, rc.is_anisotropic, rc.is_biaxial,
            rc.biax_ax0, rc.biax_ax1, rc.biax_ax2, rc.n_o_hero_seed, rc.n_beta_hero, rc.n_alpha_hero, rc.n_gamma_hero,
            rc.n_e_hero_seed, rc.n_o_hoisted, rc.alpha_o_hoisted, rc.alpha_e_hoisted, rc.alpha_beta_hoisted,
            rc.studio_key_dir, rc.studio_fill_dir, rc.studio_sin_lp, observer,
            &stokes, &radiance, &path_pdf, &current_origin, &current_dir, &current_k,
            &inside_gem, &is_extraordinary, &prev_plane_normal, &have_prev_plane_normal,
            &split_radiance, &compat, &nee_xyz, &path_escaped, &pending_light_mis, &pending_light_mis_dir,
        );
        if (status == BOUNCE_STATUS_TERMINATE) {
            break;
        }
        bounce = bounce + 1u;
    }

    transport_finalize_ray(idx, lambdas, &radiance, split_radiance, path_pdf, compat, path_escaped, nee_xyz);
}
