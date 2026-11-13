// Spectral estimator for isotropic, uniaxial-birefringent and biaxial-birefringent media: the GPU
// port of `optics::raytracer::trace_spectral_ray`. Driven by `renderer::gpu::estimator_check`
// (image statistics, energy-conservation furnace, spectral debug comparison) and
// `renderer::gpu::transport_check` (per-function ULP budgets, via `shaders/transport_functions/`).
//
// This file holds only the megakernel entry point `transport_main`: per-ray setup (camera ray,
// hero wavelength, hoisted constants), a loop calling `transport_bounce_step` once per bounce, and
// the output. The scene bindings and bounce helpers live in `shaders/transport_bounce/`. The
// wavefront kernels in `wavefront_transport.wgsl` call the same `transport_bounce_step`, so the
// code is kept in one place that both callers share.

@compute @workgroup_size(64)
fn transport_main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    // Cooperative fill of the workgroup's plane cache. It runs before the out-of-range return,
    // because `workgroupBarrier()` must be reached by every invocation of the workgroup.
    // `plane_count` is uniform across the workgroup, so the barrier is uniform control flow.
    // Only the real planes are cached: concave tool records follow them in `planes` and are read
    // from `scene_tool`.
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

    let idx = gid.x;
    let total = params.num_pixels * camera.num_samples;
    if (idx >= total) {
        return;
    }

    // Camera ray, hero wavelength and per-channel `lambdas` come from `transport_generate_ray`
    // (`shaders/transport_bounce/09_finalize_and_ray_gen.wgsl`), shared with `wavefront_generate`.
    // `idx` is the local (pixel, sample) tuple index; the function adds `params.pixel_offset`.
    let gen = transport_generate_ray(idx);
    // `var`, not `let`: the bounce functions take `&lambdas`, and they treat it as read-only.
    var lambdas: array<f32, 8> = gen.lambdas;
    let seed0 = gen.seed0;

    var stokes: array<vec4<f32>, 8>;
    var radiance: array<f32, 8>;
    var path_pdf: array<f32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        stokes[k] = vec4<f32>(1.0, 0.0, 0.0, 0.0);
        radiance[k] = 0.0;
        path_pdf[k] = 1.0;
    }

    // Per-ray constants from the material (dispersion, absorption, biaxial frame, hero indices,
    // studio rig directions), from `transport_hoist_ray_constants`. `wavefront_bounce` calls it
    // again on every bounce instead of once per ray.
    let rc = transport_hoist_ray_constants(lambdas);

    var current_origin = gen.origin;
    var current_dir = gen.dir;
    // Unit direction from the stone back to the eye, for the head shadow of the lit models.
    let observer = -gen.dir;
    // Wave normal `k`, tracked alongside `current_dir` (the Poynting direction `S`). It starts
    // equal to the ray direction, and in air `k == S` always (no walk-off in air).
    var current_k = gen.dir;
    var inside_gem = false;
    // Eigenmode drawn at the most recent air-to-crystal entry. Meaningless while `!inside_gem`.
    // It is carried across internal bounces, as on the CPU.
    var is_extraordinary = false;
    var prev_plane_normal = vec3<f32>(0.0, 0.0, 0.0);
    var have_prev_plane_normal = false;

    // Exit-event splitting state. `split_radiance` is staged and folded into `radiance` only if the
    // path escapes. Bit `j` of `compat[k]` stays set while channels `k` and `j` agree at every
    // interior dispersive event (all bits set before the first mismatch).
    var split_radiance: array<f32, 8>;
    var compat: array<u32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        split_radiance[k] = 0.0;
        compat[k] = 0xFFu;
    }
    var path_escaped = false;
    // Running NEE XYZ total, see `transport_bounce_step`.
    var nee_xyz = vec3<f32>(0.0, 0.0, 0.0);
    var pending_light_mis: f32 = 0.0;
    // Interior direction at which `pending_light_mis`'s phase pdf was evaluated.
    var pending_light_mis_dir = vec3<f32>(0.0, 0.0, 0.0);

    if (GATED_LOOP) {
        // Gated loop (pipeline constant `GATED_LOOP`, see
        // `transport_bounce/01_header_and_constants`). A bounce with a heavy arm (anything but
        // total internal reflection) parks after its surface phase. Cheap bounces continue in the
        // inner loop, and the parked arms run together once every lane of the warp has parked or
        // finished. Random draws are keyed by `seed0` and the bounce index, so only the
        // interleaving between lanes differs from the plain loop.
        var bounce: u32 = 0u;
        var alive: bool = true;
        var park: BouncePark = bounce_park_blank();
        loop {
            loop {
                if (!alive || park.pending || bounce >= params.max_bounces) {
                    break;
                }
                let st = transport_bounce_front(
                    bounce, seed0, rc, observer, &lambdas,
                    &stokes, &radiance, &path_pdf, &current_origin, &current_dir, &current_k,
                    &inside_gem, &is_extraordinary, &prev_plane_normal, &have_prev_plane_normal,
                    &split_radiance, &compat, &nee_xyz, &path_escaped, &pending_light_mis, &pending_light_mis_dir,
                    &park,
                );
                if (st == BOUNCE_STATUS_TERMINATE) {
                    alive = false;
                } else if (st == BOUNCE_STATUS_CONTINUE) {
                    bounce = bounce + 1u;
                }
            }
            if (!park.pending) {
                break;
            }
            let st_back = transport_bounce_back(
                bounce, seed0, rc, observer, &lambdas,
                &stokes, &radiance, &path_pdf, &current_origin, &current_dir, &current_k,
                &inside_gem, &is_extraordinary, &prev_plane_normal, &have_prev_plane_normal,
                &split_radiance, &compat, &nee_xyz, &path_escaped, &pending_light_mis, &pending_light_mis_dir,
                park,
            );
            park.pending = false;
            if (st_back == BOUNCE_STATUS_TERMINATE) {
                alive = false;
            } else {
                bounce = bounce + 1u;
            }
        }
    } else {
        for (var bounce: u32 = 0u; bounce < params.max_bounces; bounce = bounce + 1u) {
            // One bounce of `transport_bounce_step` (`shaders/transport_bounce/`).
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
        }
    }

    // Per-ray tail: `transport_finalize_ray`
    // (`shaders/transport_bounce/09_finalize_and_ray_gen.wgsl`) commits the staged splits,
    // integrates XYZ, applies white balance and the clamp, and writes the output. The wavefront
    // termination paths share it.
    transport_finalize_ray(idx, lambdas, &radiance, split_radiance, path_pdf, compat, path_escaped, nee_xyz);
}
