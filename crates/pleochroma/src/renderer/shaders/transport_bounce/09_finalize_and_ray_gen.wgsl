
// The per-ray tail: the staged exit-split commit, integration to XYZ, the white balance and
// clamp, and the output writes. `out_xyz` is always written. The per-channel debug buffers
// are written only when bit 0 of `params.write_debug_buffers` is set. The megakernel
// (`transport_main`, at the end of its bounce loop) and the wavefront finalize
// (`wavefront_transport.wgsl`) both call this, writing the same output bindings. `idx` is
// the dispatch-local `pixel_in_chunk * camera.num_samples + sample_in_chunk` in both.
//
// Modifies `*radiance` in place, folding in `split_radiance` when `path_escaped`. The
// debug write below reads the post-commit `radiance`.
fn transport_finalize_ray(
    idx: u32,
    lambdas: array<f32, 8>,
    radiance: ptr<function, array<f32, 8>>,
    split_radiance: array<f32, 8>,
    path_pdf: array<f32, 8>,
    compat: array<u32, 8>,
    path_escaped: bool,
    // Sum of the scattering NEE deposits so far, each already integrated to XYZ with the
    // `path_pdf`/`compat` of its own event (see `nee_contribution_hg_scatter` in
    // `transport_bounce_step`). Added unconditionally, not gated on `path_escaped`, as the CPU
    // `trace_spectral_ray_inner` does. Under exit collection it also holds the collected leaves.
    nee_xyz: vec3<f32>,
) {
    // Commit the staged exit splits into `radiance` only if the hero path reached its
    // environment lookup (`path_escaped`). This all-or-nothing gate matches the CPU's
    // `ExitSplitCtx::split_radiance`. A no-op when nothing split.
    if (path_escaped) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            (*radiance)[k] = (*radiance)[k] + split_radiance[k];
        }
    }

    // Each live channel is normalised over the techniques under which it would have stayed
    // alive on this path: its own family, `compat[k]`, not the hero's.
    var xyz = integrate_channels_to_xyz_family(*radiance, lambdas, path_pdf, compat);
    // `nee_xyz` is already XYZ, so it is added directly. It holds the HG scatter-point NEE,
    // the exit-collection leaves and the tray-flush deposits, added ahead of the white
    // balance as the CPU's `integrate_collected_to_xyz` sum does.
    xyz = xyz + nee_xyz;
    if (params.env_mode == 1u) {
        // Bradford-LMS von Kries adaptation, not a raw XYZ scale. Matches
        // `optics::raytracer::apply_von_kries_white_balance`.
        xyz = apply_von_kries_white_balance(xyz, params.white_balance);
    }
    // Non-negative clamp, applied outside the `env_mode` branch as on the CPU. Before the
    // white balance `xyz` is a sum of non-negative terms, so the clamp changes nothing there.
    xyz = max(xyz, vec3<f32>(0.0));

    out_xyz[idx * 3u + 0u] = xyz.x;
    out_xyz[idx * 3u + 1u] = xyz.y;
    out_xyz[idx * 3u + 2u] = xyz.z;
    // Production reads back only `out_xyz`. The per-channel debug buffers serve the Tier 2
    // self-tests. Guarding them on `params.write_debug_buffers` (bit 0, set for every
    // self-test) lets production bind small dummy buffers instead, which cuts write traffic.
    if ((params.write_debug_buffers & 1u) != 0u) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            out_radiance[idx * 8u + k] = (*radiance)[k];
            out_lambdas[idx * 8u + k] = lambdas[k];
            out_path_pdf[idx * 8u + k] = path_pdf[k];
            out_compat[idx * 8u + k] = compat[k];
        }
    }
}

// Camera ray and hero wavelengths for `idx`. `transport_main` and `wavefront_generate`
// (`wavefront_transport.wgsl`) both call it and draw the same RNG sequence. `idx` is the
// dispatch-local `(pixel_in_chunk, sample_in_chunk)` index. `params.pixel_offset` is added
// to get the global pixel for camera rays and the per-pixel rotations. Output slots stay
// indexed by the caller's `idx`.
//
// The hoisted per-ray constants (dispersion, absorption, biaxial frame, studio directions)
// are not computed here. They depend only on `lambdas`, so they are recomputed each bounce
// rather than stored in the wavefront ray state. See `wavefront_transport.wgsl`.
struct GeneratedRay {
    origin: vec3<f32>,
    dir: vec3<f32>,
    lambdas: array<f32, 8>,
    seed0: u32,
}

fn transport_generate_ray(idx: u32) -> GeneratedRay {
    let pixel = idx / camera.num_samples + params.pixel_offset;
    let local_sample = idx % camera.num_samples;
    let sample_num = local_sample + params.sample_offset;

    let seed0 = hash_u32((pixel * 0x9e3779b9u) ^ (sample_num * 0x85ebca6bu));
    // The estimator of this invocation (branching unless bit 2 of
    // `params.write_debug_buffers` is set).
    collect_bind(params.write_debug_buffers);

    // Stratified pixel jitter and hero wavelength (optics::raytracer::
    // {low_discrepancy_base2, cranley_patterson_rotate}). `seed0` still seeds the hashed
    // per-bounce draws in `transport_bounce_step`.
    let rot_jx = low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_X_ROTATION_STREAM));
    let rot_jy = low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_Y_ROTATION_STREAM));
    let rot_hero = low_discrepancy_base2(hash_u32(pixel ^ HERO_WAVELENGTH_ROTATION_STREAM));
    let jx = cranley_patterson_rotate(low_discrepancy_base2(sample_num), rot_jx) - 0.5;
    let jy = cranley_patterson_rotate(radical_inverse_base(sample_num, 3u), rot_jy) - 0.5;
    let hero_rand = cranley_patterson_rotate(radical_inverse_base(sample_num, 5u), rot_hero);
    let raygen = generate_camera_ray(pixel, jx, jy);

    let channel_width = SPECTRUM_SPAN / f32(NUM_CHANNELS);
    let lambda_hero = fma(hero_rand, SPECTRUM_SPAN, SPECTRUM_MIN);
    var lambdas: array<f32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        let offset = fma(f32(k), channel_width, lambda_hero - SPECTRUM_MIN);
        lambdas[k] = SPECTRUM_MIN + (offset % SPECTRUM_SPAN);
    }

    var result: GeneratedRay;
    result.origin = raygen.origin;
    result.dir = raygen.dir;
    result.lambdas = lambdas;
    result.seed0 = seed0;
    return result;
}

// Per-ray constants that depend only on `lambdas`, `material`, `MATERIAL_CLASS` and `params`,
// not on anything that varies per bounce. `transport_main` calls this once per ray.
// `wavefront_bounce` calls it on every bounce instead of storing the result. That is
// bit-identical, since it is a pure function of the stored `lambdas` (see
// `wavefront_transport.wgsl`).
struct RayConstants {
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
}

fn transport_hoist_ray_constants(lambdas: array<f32, 8>) -> RayConstants {
    let c_axis = material.dispersion.c_axis_and_birefringence.xyz;
    let birefringence_delta = material.dispersion.c_axis_and_birefringence.w;
    // ANDed with the pipeline-overridable MATERIAL_CLASS -- a no-op for
    // MATERIAL_CLASS_GENERIC, forced `false` for MATERIAL_CLASS_ISOTROPIC regardless of
    // the material buffer's own flag.
    let is_anisotropic = (material.dispersion.is_anisotropic != 0u)
        && (MATERIAL_CLASS != MATERIAL_CLASS_ISOTROPIC);
    // Biaxial anisotropy (three distinct principal indices), as opposed to the uniaxial
    // approximation. Constant per ray, like `c_axis`. ANDed with `MATERIAL_CLASS` only to
    // drop branches that cannot be reached for that class (see
    // `renderer::gpu::frame::classify_material`).
    let is_biaxial = (material.dispersion.has_biaxial_delta != 0u)
        && (MATERIAL_CLASS == MATERIAL_CLASS_GENERIC || MATERIAL_CLASS == MATERIAL_CLASS_BIAXIAL);
    // The biaxial principal-axis frame (alpha, beta, gamma world directions) depends
    // only on `c_axis` (and the optional second direction `a_axis_and_flag`), so it is
    // computed once per ray rather than every bounce. Flag 0 is the gamma-only completion.
    let a_axis_and_flag = material.dispersion.a_axis_and_flag;
    let biax_axes = biaxial_axes(c_axis, a_axis_and_flag.xyz, a_axis_and_flag.w);
    let biax_ax0 = biax_axes.ax0;
    let biax_ax1 = biax_axes.ax1;
    let biax_ax2 = biax_axes.ax2;
    // Hero principal indices, as `GemMaterial::biaxial_indicatrix` defines them:
    // `n_beta := dispersion.evaluate(lambda)`, `n_alpha := n_beta - biaxial_delta_beta_alpha`,
    // `n_gamma := n_alpha + birefringence_delta`. Computed unconditionally. For non-biaxial
    // materials the delta is 0, so all three equal `n_o_hero_seed`. The biaxial functions run
    // only under `is_biaxial`.
    let n_o_hero_seed = dispersion_evaluate(material.dispersion.model_type, material.dispersion.param_a, material.dispersion.param_b, lambdas[0]);
    // With per-axis curves (`GpuBiaxialCurves`) `n_alpha`/`n_gamma` come from their own
    // curves at the hero wavelength and the three are sorted by magnitude, exactly as
    // `biaxial_indicatrix` does; without curves this is the achromatic offsets above.
    let hero_principal = biaxial_principal_indices(
        material.biaxial_curves.types.z != 0u,
        material.biaxial_curves.types.x,
        material.biaxial_curves.alpha_a,
        material.biaxial_curves.alpha_b,
        material.biaxial_curves.types.y,
        material.biaxial_curves.gamma_a,
        material.biaxial_curves.gamma_b,
        n_o_hero_seed,
        material.dispersion.biaxial_delta_beta_alpha,
        birefringence_delta,
        lambdas[0],
    );
    let n_alpha_hero = hero_principal.x;
    let n_beta_hero = hero_principal.y;
    let n_gamma_hero = hero_principal.z;
    // Hero extraordinary index, as `GemMaterial::extraordinary_index_at`: the material's own
    // e-ray dispersion curve when it has one (Quartz/Amethyst/Citrine/Rutile), else
    // `n_o_hero_seed + birefringence_delta`.
    var n_e_hero_seed: f32;
    if (EXTRAORDINARY_DISPERSION && material.has_extraordinary_dispersion != 0u) {
        n_e_hero_seed = extraordinary_dispersion_evaluate(material.extraordinary_model_type, material.extraordinary_param_a, material.extraordinary_param_b, lambdas[0]);
    } else {
        n_e_hero_seed = n_o_hero_seed + birefringence_delta;
    }

    // `n_o` and the per-channel absorption coefficients depend only on `lambdas[k]` and the
    // material's dispersion and band data.
    var n_o_hoisted: array<f32, 8>;
    var alpha_o_hoisted: array<f32, 8>;
    var alpha_e_hoisted: array<f32, 8>;
    var alpha_beta_hoisted: array<f32, 8>;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        n_o_hoisted[k] = dispersion_evaluate(material.dispersion.model_type, material.dispersion.param_a, material.dispersion.param_b, lambdas[k]);
        alpha_o_hoisted[k] = spectral_absorption(material.o_ray_bands, material.o_ray_band_count, lambdas[k]);
        alpha_e_hoisted[k] = spectral_absorption(material.e_ray_bands, material.e_ray_band_count, lambdas[k]);
        alpha_beta_hoisted[k] = 0.0;
    }
    if (is_biaxial) {
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            alpha_beta_hoisted[k] = spectral_absorption(material.beta_ray_bands, material.beta_ray_band_count, lambdas[k]);
        }
    }

    // Studio rig key/fill/ring directions: a pure function of `params` alone. Unused
    // when `params.env_mode != transport_env_mode::STUDIO_RIG`.
    let studio_key_dir = studio_rig_key_dir(params.studio_light_yaw, params.studio_light_pitch);
    let studio_fill_dir = studio_rig_fill_dir(params.studio_light_yaw, params.studio_light_pitch);
    let studio_sin_lp = sin(params.studio_light_pitch);

    var result: RayConstants;
    result.c_axis = c_axis;
    result.birefringence_delta = birefringence_delta;
    result.is_anisotropic = is_anisotropic;
    result.is_biaxial = is_biaxial;
    result.biax_ax0 = biax_ax0;
    result.biax_ax1 = biax_ax1;
    result.biax_ax2 = biax_ax2;
    result.n_o_hero_seed = n_o_hero_seed;
    result.n_beta_hero = n_beta_hero;
    result.n_alpha_hero = n_alpha_hero;
    result.n_gamma_hero = n_gamma_hero;
    result.n_e_hero_seed = n_e_hero_seed;
    result.n_o_hoisted = n_o_hoisted;
    result.alpha_o_hoisted = alpha_o_hoisted;
    result.alpha_e_hoisted = alpha_e_hoisted;
    result.alpha_beta_hoisted = alpha_beta_hoisted;
    result.studio_key_dir = studio_key_dir;
    result.studio_fill_dir = studio_fill_dir;
    result.studio_sin_lp = studio_sin_lp;
    return result;
}
