//! Tier 3: statistical image equivalence -- Welford mean/M2 on CPU and GPU DISJOINT
//! sample ranges (as production renders would split work), per-pixel z-score, and
//! connected-component clustering of failing pixels (a structured bias clusters; noise
//! salts-and-peppers).

use crate::{
    optics::{
        materials::GemMaterial,
        raytracer::{
            BACKDROP_GREY, FacetFinish, LightingPreset, environment::environment_white_balance,
            illuminant_temperature_k,
        },
    },
    renderer::{
        buffers::{GpuGemMaterial, GpuTransportParams, encode_facet_finishes, transport_env_mode},
        env_map::EnvironmentMap,
        env_map_gpu::HdrEnvGpuData,
    },
};
use glam::Vec3;

use super::{
    CpuScene, MaterialForDispatch, SceneBuffersForDispatch, Welford, alexandrite_material,
    bruted_girdle_finishes, camera_params_for, cpu_sample_xyz, cpu_samples,
    dispatch_transport_for_class, quartz_material, round_brilliant_planes, rutile_material,
    tanzanite_material, test_camera, tier3_material, topaz_material, tourmaline_material, z_score,
    zircon_material,
};

use crate::renderer::gpu::{
    frame::classify_material,
    z_stats::{
        NULL_OVER_3_SIGMA_RATE, SIGMA_THRESHOLD, connected_components, count_non_finite,
        passes_z_criteria,
    },
};

/// Outcome of the image comparison check.
#[derive(Debug, Clone)]
pub struct ImageComparisonResult {
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Cpu samples per pixel.
    pub cpu_samples_per_pixel: u32,
    /// Gpu samples per pixel.
    pub gpu_samples_per_pixel: u32,
    /// Image-aggregate mean z (averaged over every pixel's per-pixel z, luminance
    /// channel).
    pub mean_z: f64,
    /// Over 3 sigma count.
    pub over_3_sigma_count: usize,
    /// Total number of pixels compared.
    pub total_pixels: usize,
    /// Binomial expectation for `|z| > 3` under the null (two-sided): ~0.27%.
    pub over_3_sigma_expected: f64,
    /// Sizes of every connected component (4-connectivity) of `|z| > 3` pixels,
    /// largest first.
    pub cluster_sizes: Vec<usize>,
    /// Max abs z.
    pub max_abs_z: f64,
    /// Max abs z pixel.
    pub max_abs_z_pixel: (u32, u32),
    /// NaN or infinite XYZ components among the CPU and GPU samples; any fails the check.
    pub non_finite_samples: usize,
}

impl ImageComparisonResult {
    /// Passes if the observed `|z|>3` rate is within a generous multiple of its
    /// binomial expectation AND no cluster is larger than a handful of pixels --
    /// see this module's doc comment: a structured bias clusters, noise does not.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.non_finite_samples == 0
            && passes_z_criteria(
                self.over_3_sigma_count,
                self.total_pixels,
                self.over_3_sigma_expected,
                &self.cluster_sizes,
            )
    }
}

/// Runs the image comparison check against the CPU reference.
#[must_use]
pub fn run_image_comparison(ctx: &crate::renderer::gpu::GpuContext) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &tier3_material(), &[], LightingPreset::Daylight)
}

/// Runs the image comparison zircon check against the CPU reference.
#[must_use]
pub fn run_image_comparison_zircon(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &zircon_material(), &[], LightingPreset::Daylight)
}

/// Runs the image comparison tourmaline check against the CPU reference.
#[must_use]
pub fn run_image_comparison_tourmaline(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &tourmaline_material(), &[], LightingPreset::Daylight)
}

/// Quartz: a built-in whose extraordinary ray disperses with a genuine independent
/// curve (`uniaxial_extraordinary_dispersion.is_some()`; Amethyst, Citrine and Rutile
/// set it too).
///
/// Rather than the constant-offset approximation the other uniaxial materials here use,
/// this exercises `has_extraordinary_dispersion != 0` through the real megakernel
/// dispatch -- see [`super::quartz_material`]'s doc comment.
#[must_use]
pub fn run_image_comparison_quartz(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &quartz_material(), &[], LightingPreset::Daylight)
}

/// Rutile: full uniaxial Fresnel (Lekner 1991), Tier 3.
///
/// See [`super::rutile_material`]'s own doc comment for why this is the strongest
/// single end-to-end CPU-vs-GPU check the closed-form uniaxial solve's WGSL mirror has.
#[must_use]
pub fn run_image_comparison_rutile(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &rutile_material(), &[], LightingPreset::Daylight)
}

/// Exit-event spectral splitting, Tier 3: Synthetic Moissanite.
///
/// The strongest single end-to-end CPU-vs-GPU check the WGSL exit-event splitting port
/// has: a real 57-facet Standard Round Brilliant with every facet polished, so refractive
/// non-TIR-only paths dominate -- exactly where a companion channel's own Snell
/// direction is most likely to diverge from the hero's. Synthetic Moissanite's
/// `birefringence_delta = +0.0415` combined with its high Abbe dispersion makes that
/// divergence large and common, under the analytic studio lighting rig every other
/// Tier 3 scene in this module uses.
///
/// # Panics
///
/// Panics if `"Synthetic Moissanite"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_synthetic_moissanite(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    // Bare built-in on purpose (scale 1.0, no `material_for_stone`): this compares CPU
    // against GPU on the same material, so only parity matters, not the render's colour scale.
    let material = GemMaterial::by_name("Synthetic Moissanite")
        .expect("\"Synthetic Moissanite\" is a built-in material in GemMaterial::all_materials()");
    run_image_comparison_for(ctx, &material, &[], LightingPreset::Daylight)
}

/// Runs the image comparison alexandrite check against the CPU reference.
#[must_use]
pub fn run_image_comparison_alexandrite(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &alexandrite_material(), &[], LightingPreset::Daylight)
}

/// Runs the image comparison topaz check against the CPU reference.
#[must_use]
pub fn run_image_comparison_topaz(ctx: &crate::renderer::gpu::GpuContext) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &topaz_material(), &[], LightingPreset::Daylight)
}

/// Runs the image comparison tanzanite check against the CPU reference.
#[must_use]
pub fn run_image_comparison_tanzanite(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &tanzanite_material(), &[], LightingPreset::Daylight)
}

/// Tier 3 statistical image comparison on a scene with a frosted girdle.
///
/// GPU mirror of `tests/raytracer_tests/material_equivalence_finish.rs`'s
/// `frosted_girdle_changes_face_up_appearance_measurably`, compared statistically
/// (variance-scaled z-score, connected-component clustering) rather than against a
/// polished CPU baseline. Diamond, girdle band bruted via [`bruted_girdle_finishes`].
///
/// # Panics
///
/// Panics if `"Diamond"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_frosted_girdle(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Diamond")
        .expect("\"Diamond\" is a built-in material in GemMaterial::all_materials()");
    let num_planes = round_brilliant_planes().len();
    let finishes = bruted_girdle_finishes(num_planes);
    run_image_comparison_for(ctx, &material, &finishes, LightingPreset::Daylight)
}

/// Tier 3 statistical image comparison on a scene with Henyey-Greenstein scattering.
///
/// A forward-biased (`g = 0.3`) scattering medium (`sigma_s = 1.5`) on built-in Ruby, whose
/// chromatic absorption and scattering are both active, so `sigma_t_k` differs per channel.
/// GPU mirror of the scene of `scattering_measurably_changes_face_up_appearance` in
/// `optics::raytracer::scattering::tests`, compared statistically rather than against a
/// clear-stone CPU baseline.
///
/// # Panics
///
/// Panics if `"Ruby"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_scattering(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Ruby")
        .expect("\"Ruby\" is a built-in material in GemMaterial::all_materials()")
        .with_scattering(1.5, 0.3);
    run_image_comparison_for(ctx, &material, &[], LightingPreset::Daylight)
}

/// Tier 3 statistical image comparison of the biaxial inclusion-scattering path.
///
/// The scatter block of `spectral_transport.wgsl` mirrors its absorption block: a biaxial
/// material's scattering alphas come from `biaxial_eigen_polarizations` and
/// `pleochroic_channel_alpha_biaxial`, not the uniaxial approximation of
/// `channel_absorption_alphas`. Alexandrite ([`super::alexandrite_material`], biaxial with a
/// populated `beta_ray` band) with scattering is the biaxial counterpart of
/// [`run_image_comparison_scattering`]'s uniaxial Ruby, so the `is_biaxial && has_beta_ray`
/// path is reached.
#[must_use]
pub fn run_image_comparison_biaxial_scattering(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = super::alexandrite_material().with_scattering(1.5, 0.3);
    run_image_comparison_for(ctx, &material, &[], LightingPreset::Daylight)
}

/// Tier 3 statistical image comparison on a scene with a nonzero `edge_rounding_radius`.
///
/// GPU mirror of the scene of
/// `intersect::edge_rounding_tests::edge_rounding_measurably_changes_face_up_appearance`,
/// compared statistically against the CPU tracer rather than against its sharp-edge baseline.
///
/// # Panics
///
/// Panics if `"Diamond"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_edge_rounding(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Diamond")
        .expect("\"Diamond\" is a built-in material in GemMaterial::all_materials()")
        .with_edge_rounding(0.02);
    run_image_comparison_for(ctx, &material, &[], LightingPreset::Daylight)
}

/// Tier 3 statistical image comparison on a
/// colored (chromatically absorbing) stone at a non-1.0 `absorption_path_scale`.
///
/// Ruby (built-in chromatic Beer-Lambert absorption, exactly what
/// `absorption_path_scale` multiplies the path length before) at `scale = 3.0` --
/// large enough that a scale/no-scale mismatch between the CPU and WGSL mirrors would
/// show up as a strong, whole-image color/brightness bias, not just isolated pixels.
///
/// # Panics
///
/// Panics if `"Ruby"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_absorption_path_scale(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Ruby")
        .expect("\"Ruby\" is a built-in material in GemMaterial::all_materials()")
        .with_absorption_path_scale(3.0);
    run_image_comparison_for(ctx, &material, &[], LightingPreset::Daylight)
}

/// Tier 3 statistical image comparison of a ZONED stone.
///
/// The CPU tracer's zoned absorption (`optics::raytracer::zoned`) against the megakernel's zone
/// kernels (`shaders/zoning/*.wgsl`), through the same disjoint-sample-range z-score criteria as
/// every other Tier 3 check ([`ImageComparisonResult::passed`]). A whole-image colour bias
/// from a wrong zone length, boundary, frame or per-zone alpha shows up as a clustered
/// failure.
///
/// See [`crate::renderer::gpu::zoned_cases::ZonedImageCase`] for the stones; the soft cases
/// are the ones that decide whether `GPU_SOFT_SUBDIV` may be lowered.
#[must_use]
pub fn run_image_comparison_zoned(
    ctx: &crate::renderer::gpu::GpuContext,
    case: crate::renderer::gpu::zoned_cases::ZonedImageCase,
) -> ImageComparisonResult {
    run_image_comparison_for(ctx, &case.material(), &[], LightingPreset::Daylight)
}

/// Tier 3 statistical image comparison under the ISO hemisphere lighting model.
///
/// Diamond on the standard 57-facet Round Brilliant.
///
/// # Panics
///
/// Panics if `"Diamond"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_iso_hemisphere(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Diamond")
        .expect("\"Diamond\" is a built-in material in GemMaterial::all_materials()");
    run_image_comparison_with_backdrop(
        ctx,
        &material,
        &[],
        LightingPreset::IsoHemisphere,
        (BACKDROP_GREY, 1.0),
    )
}

/// Tier 3 statistical image comparison under the Light tent + black cards model.
///
/// Diamond on the standard 57-facet Round Brilliant.
///
/// # Panics
///
/// Panics if `"Diamond"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_light_tent(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Diamond")
        .expect("\"Diamond\" is a built-in material in GemMaterial::all_materials()");
    run_image_comparison_with_backdrop(
        ctx,
        &material,
        &[],
        LightingPreset::LightTent,
        (BACKDROP_GREY, 1.0),
    )
}

/// Tier 3 statistical image comparison under the sky-only Daylight dome model.
///
/// Diamond on the standard 57-facet Round Brilliant.
///
/// # Panics
///
/// Panics if `"Diamond"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_daylight_dome(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Diamond")
        .expect("\"Diamond\" is a built-in material in GemMaterial::all_materials()");
    run_image_comparison_with_backdrop(
        ctx,
        &material,
        &[],
        LightingPreset::DaylightDome,
        (BACKDROP_GREY, 1.0),
    )
}

/// Tier 3 statistical image comparison under the Daylight sky + direct sun model.
///
/// A polished Diamond: the 0.27 degree sun disc is met only by exit directions, giving
/// deterministic facet flashes, since no NEE is possible at a specular interface.
///
/// # Panics
///
/// Panics if `"Diamond"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_daylight_sun(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Diamond")
        .expect("\"Diamond\" is a built-in material in GemMaterial::all_materials()");
    run_image_comparison_with_backdrop(
        ctx,
        &material,
        &[],
        LightingPreset::DaylightSun,
        (BACKDROP_GREY, 1.0),
    )
}

/// Tier 3 statistical image comparison under the Daylight sky + direct sun model, frosted girdle.
///
/// A bruted (frosted) girdle: the frosted-exterior next-event-estimation toward the sun disc
/// and its MIS-weighted escape (`nee_contribution_frosted_exterior`, CPU and WGSL twin) are
/// both exercised, which a polished stone cannot do.
///
/// # Panics
///
/// Panics if `"Diamond"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_daylight_sun_frosted_girdle(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Diamond")
        .expect("\"Diamond\" is a built-in material in GemMaterial::all_materials()");
    let num_planes = round_brilliant_planes().len();
    let finishes = bruted_girdle_finishes(num_planes);
    run_image_comparison_with_backdrop(
        ctx,
        &material,
        &finishes,
        LightingPreset::DaylightSun,
        (BACKDROP_GREY, 1.0),
    )
}

/// Tier 3 statistical image comparison under the ASET-style contrast view (narrow zone
/// bands, read from the wavelength inside the model).
///
/// Diamond on the standard 57-facet Round Brilliant.
///
/// # Panics
///
/// Panics if `"Diamond"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_aset(ctx: &crate::renderer::gpu::GpuContext) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Diamond")
        .expect("\"Diamond\" is a built-in material in GemMaterial::all_materials()");
    run_image_comparison_with_backdrop(
        ctx,
        &material,
        &[],
        LightingPreset::Aset,
        (BACKDROP_GREY, 1.0),
    )
}

/// Shared Tier 3 statistical image comparison body, parameterized on `material`.
///
/// The isotropic and uniaxial checks all go through it. `facet_finishes` is `&[]` for
/// callers without a frosted finish, which is exactly all-`Polished` (see
/// [`super::CpuScene::facet_finishes`]); the frosted-girdle check passes its own.
pub(super) fn run_image_comparison_for(
    ctx: &crate::renderer::gpu::GpuContext,
    material: &GemMaterial,
    facet_finishes: &[FacetFinish],
    preset: LightingPreset,
) -> ImageComparisonResult {
    run_image_comparison_with_backdrop(ctx, material, facet_finishes, preset, (0.0, 1.0))
}

/// Tier 3 statistical image comparison of the surface-glare scale.
///
/// Diamond under the D65 daylight preset with the given `surface_glare` (`0.0..=1.0`)
/// applied to both the CPU tracer and the megakernel, so the first-surface reflection
/// branch is compared.
///
/// # Panics
///
/// Panics if `"Diamond"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_surface_glare(
    ctx: &crate::renderer::gpu::GpuContext,
    surface_glare: f32,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Diamond")
        .expect("\"Diamond\" is a built-in material in GemMaterial::all_materials()");
    run_image_comparison_with_backdrop(
        ctx,
        &material,
        &[],
        LightingPreset::Daylight,
        (BACKDROP_GREY, surface_glare),
    )
}

/// [`run_image_comparison_for`] with a backdrop card behind the stone and a
/// surface-glare scale, passed as `(backdrop, surface_glare)` (see
/// `EnvironmentSource::Studio::backdrop`/`surface_glare`), so the camera-ray miss branch
/// and the first-surface reflection are compared too; `(0.0, 1.0)` is the plain
/// environment.
fn run_image_comparison_with_backdrop(
    ctx: &crate::renderer::gpu::GpuContext,
    material: &GemMaterial,
    facet_finishes: &[FacetFinish],
    preset: LightingPreset,
    backdrop_and_glare: (f32, f32),
) -> ImageComparisonResult {
    run_image_comparison_with_shadow(
        ctx,
        material,
        facet_finishes,
        preset,
        backdrop_and_glare,
        None,
        None,
    )
}

/// Compares a shadowed tray under the existing statistical frame thresholds.
#[must_use]
pub fn run_image_comparison_tray_shadow(
    ctx: &crate::renderer::gpu::GpuContext,
    map: &crate::optics::raytracer::environment::TrayShadowMap,
) -> ImageComparisonResult {
    run_image_comparison_with_shadow(
        ctx,
        &GemMaterial::diamond(),
        &[],
        LightingPreset::LightTent,
        (0.23, 1.0),
        Some(map),
        None,
    )
}

/// Compares a glossy tray under the existing statistical frame thresholds.
///
/// The tray is the Tray Reflection, with or without the Stone Shadow on the same plane; the
/// CPU path tracer's floor bounce and Rule M are compared against the megakernel's tray
/// variant.
#[must_use]
pub fn run_image_comparison_tray_mirror(
    ctx: &crate::renderer::gpu::GpuContext,
    mirror: crate::optics::raytracer::environment::TrayMirror,
    shadow: Option<&crate::optics::raytracer::environment::TrayShadowMap>,
) -> ImageComparisonResult {
    run_image_comparison_with_shadow(
        ctx,
        &GemMaterial::diamond(),
        &[],
        LightingPreset::LightTent,
        (0.23, 1.0),
        shadow,
        Some(mirror),
    )
}

/// Shared frame comparison with an optional map and mirror held throughout both CPU and GPU
/// sampling.
fn run_image_comparison_with_shadow(
    ctx: &crate::renderer::gpu::GpuContext,
    material: &GemMaterial,
    facet_finishes: &[FacetFinish],
    preset: LightingPreset,
    backdrop_and_glare: (f32, f32),
    shadow: Option<&crate::optics::raytracer::environment::TrayShadowMap>,
    mirror: Option<crate::optics::raytracer::environment::TrayMirror>,
) -> ImageComparisonResult {
    let (backdrop, surface_glare) = backdrop_and_glare;
    let camera = test_camera();
    let (width, height) = (48u32, 48u32);
    let planes = round_brilliant_planes();
    let gpu_material = GpuGemMaterial::encode(material);
    let gpu_finishes = encode_facet_finishes(facet_finishes, planes.len());
    let max_bounces = 10u32;
    let (exposure, light_yaw, light_pitch) = (1.0f32, 0.4f32, 0.35f32);
    let temp_k = illuminant_temperature_k(preset);
    let environment = preset
        .studio(exposure, light_yaw, light_pitch)
        .with_backdrop(backdrop)
        .with_surface_glare(surface_glare)
        .with_tray_shadow(shadow)
        .with_tray_mirror(mirror);
    let wb = environment_white_balance(environment);

    let cpu_samples_per_pixel = 500u32;
    let gpu_samples_per_pixel = 500u32;

    let scene = CpuScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes,
        material,
        max_bounces,
    };
    let cpu_flat = cpu_samples(width, height, cpu_samples_per_pixel, 0, |pixel, sample| {
        cpu_sample_xyz(&scene, pixel, sample, environment)
    });

    let camera_params = camera_params_for(&camera, width, height, gpu_samples_per_pixel);
    let params = GpuTransportParams::new(
        width * height,
        max_bounces,
        cpu_samples_per_pixel,
        transport_env_mode::STUDIO_RIG,
        0.0,
        temp_k,
        preset.params().spot_mult,
        exposure,
        light_yaw,
        light_pitch,
        wb.to_array(),
    )
    // The CPU side (`cpu_sample_xyz` above, via `sample_studio_environment_with_rig`)
    // already routes presets through uses_d65() / model() -- the GPU
    // dispatch must match or this comparison reintroduces CPU/GPU divergence.
    .with_studio_use_d65(preset.uses_d65())
    .with_studio_model(preset.model().gpu_id())
    .with_tent(preset.params().tent)
    .with_backdrop(backdrop)
    .with_surface_glare(surface_glare);
    let total_gpu = (width * height * gpu_samples_per_pixel) as usize;
    // Dispatch through whichever pipeline `GpuFrameRenderer::accumulate` would actually
    // pick for `material` (via `frame::classify_material`), not the GENERIC one every
    // other check in this module uses -- see `dispatch_transport_for_class`'s doc
    // comment for why Tier 3's image comparisons need the production path.
    let gpu_dispatch = dispatch_transport_for_class(
        ctx,
        &camera_params,
        &params,
        MaterialForDispatch {
            encoded: &gpu_material,
            class: classify_material(material),
        },
        SceneBuffersForDispatch {
            planes: &planes,
            facet_finishes: &gpu_finishes,
            hdr_env: None,
            tray_shadow: shadow,
            tray_mirror: mirror,
        },
        total_gpu,
    );

    compute_image_comparison_result(
        width,
        height,
        cpu_samples_per_pixel,
        gpu_samples_per_pixel,
        &cpu_flat,
        &gpu_dispatch.xyz,
    )
}

pub(super) fn synthetic_image_comparison_hdr_map() -> EnvironmentMap {
    let width = 16;
    let height = 8;
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let r = (((x * 3 + y * 5) % 7) as f32).mul_add(0.4, 0.1);
            let g = (((x * 2 + y * 3) % 5) as f32).mul_add(0.3, 0.1);
            let b = (((x * 5 + y * 2) % 6) as f32).mul_add(0.5, 0.1);
            pixels.push([r, g, b]);
        }
    }
    EnvironmentMap::from_rgb(width, height, pixels).expect("valid synthetic env map")
}

/// Tier 3 statistical image comparison on a scene under a non-uniform HDR environment
/// map with next-event estimation (NEE) and balance-heuristic MIS active on both CPU and GPU.
///
/// Exercises light importance sampling, Henyey-Greenstein shadow ray probing, exit
/// transmittance, and balance heuristic MIS.
///
/// # Panics
///
/// Panics if `"Ruby"` is ever removed from `GemMaterial::all_materials()`.
#[must_use]
pub fn run_image_comparison_hdr_nee(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = GemMaterial::by_name("Ruby")
        .expect("\"Ruby\" is a built-in material in GemMaterial::all_materials()")
        .with_scattering(1.0, 0.3);
    run_image_comparison_hdr_for(ctx, &material)
}

/// [`run_image_comparison_hdr_nee`]'s body for any `material`: the synthetic non-uniform HDR map
/// with NEE and balance-heuristic MIS active on both CPU and GPU, 48 x 48 pixels at 500 samples
/// per backend on disjoint ranges.
pub(super) fn run_image_comparison_hdr_for(
    ctx: &crate::renderer::gpu::GpuContext,
    material: &GemMaterial,
) -> ImageComparisonResult {
    let camera = test_camera();
    let (width, height) = (48u32, 48u32);
    let planes = round_brilliant_planes();
    let gpu_material = GpuGemMaterial::encode(material);
    let gpu_finishes = encode_facet_finishes(&[], planes.len());
    let max_bounces = 10u32;

    let env_map = synthetic_image_comparison_hdr_map();
    let environment = crate::optics::raytracer::EnvironmentSource::hdr(&env_map);
    let scene = CpuScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material,
        max_bounces,
    };
    let cpu_samples_per_pixel = 500u32;
    let gpu_samples_per_pixel = 500u32;

    let cpu_flat = cpu_samples(width, height, cpu_samples_per_pixel, 0, |pixel, sample| {
        cpu_sample_xyz(&scene, pixel, sample, environment)
    });

    let camera_params = camera_params_for(&camera, width, height, gpu_samples_per_pixel);
    let params = GpuTransportParams::new(
        width * height,
        max_bounces,
        cpu_samples_per_pixel,
        transport_env_mode::HDR_MAP,
        1.0,
        6500.0,
        1.0,
        1.0,
        0.0,
        0.0,
        [1.0, 1.0, 1.0],
    );
    let total_gpu = (width * height * gpu_samples_per_pixel) as usize;
    let gpu_hdr = HdrEnvGpuData::upload(&ctx.device, &env_map);
    let gpu_dispatch = dispatch_transport_for_class(
        ctx,
        &camera_params,
        &params,
        MaterialForDispatch {
            encoded: &gpu_material,
            class: classify_material(material),
        },
        SceneBuffersForDispatch {
            planes: &planes,
            facet_finishes: &gpu_finishes,
            hdr_env: Some(&gpu_hdr),
            tray_shadow: None,
            tray_mirror: None,
        },
        total_gpu,
    );

    compute_image_comparison_result(
        width,
        height,
        cpu_samples_per_pixel,
        gpu_samples_per_pixel,
        &cpu_flat,
        &gpu_dispatch.xyz,
    )
}

pub(super) fn compute_image_comparison_result(
    width: u32,
    height: u32,
    cpu_samples_per_pixel: u32,
    gpu_samples_per_pixel: u32,
    cpu_flat: &[Vec3],
    gpu_xyz: &[f32],
) -> ImageComparisonResult {
    compute_image_comparison_result_channel(
        width,
        height,
        (cpu_samples_per_pixel, gpu_samples_per_pixel),
        (cpu_flat, gpu_xyz),
        1,
    )
}

/// [`compute_image_comparison_result`] on one XYZ component, `channel` 0 = X, 1 = Y (the
/// luminance every other check reads), 2 = Z. A color-only error (an index error that moves the
/// hue and leaves Y alone) shows in X or Z, so a check that must catch it runs all three.
///
/// `samples_per_pixel` and `samples` are `(cpu, gpu)` pairs.
pub(super) fn compute_image_comparison_result_channel(
    width: u32,
    height: u32,
    samples_per_pixel: (u32, u32),
    samples: (&[Vec3], &[f32]),
    channel: usize,
) -> ImageComparisonResult {
    assert!(channel < 3, "XYZ has three components, not {channel}");
    let (cpu_samples_per_pixel, gpu_samples_per_pixel) = samples_per_pixel;
    let (cpu_flat, gpu_xyz) = samples;
    let num_pixels = (width * height) as usize;
    let mut z_grid = vec![0.0f64; num_pixels];
    let mut mean_z_sum = 0.0f64;
    let mut max_abs_z = 0.0f64;
    let mut max_abs_z_pixel = (0u32, 0u32);

    for pixel in 0..num_pixels {
        let mut cpu_acc = Welford::default();
        for s in 0..cpu_samples_per_pixel as usize {
            let v = cpu_flat[pixel * cpu_samples_per_pixel as usize + s];
            cpu_acc.update(f64::from(v[channel]));
        }
        let mut gpu_acc = Welford::default();
        for s in 0..gpu_samples_per_pixel as usize {
            let idx = (pixel * gpu_samples_per_pixel as usize + s) * 3;
            gpu_acc.update(f64::from(gpu_xyz[idx + channel]));
        }
        let z = z_score(&cpu_acc, &gpu_acc);
        z_grid[pixel] = z;
        mean_z_sum += z;
        if z.abs() > max_abs_z {
            max_abs_z = z.abs();
            let pixel = u32::try_from(pixel).expect("the grid has width * height u32 pixels");
            max_abs_z_pixel = (pixel % width, pixel / width);
        }
    }

    let over_3_sigma_count = z_grid.iter().filter(|z| z.abs() > SIGMA_THRESHOLD).count();
    let cluster_sizes = connected_components(&z_grid, width, height, SIGMA_THRESHOLD);

    ImageComparisonResult {
        width,
        height,
        cpu_samples_per_pixel,
        gpu_samples_per_pixel,
        mean_z: mean_z_sum / num_pixels as f64,
        over_3_sigma_count,
        total_pixels: num_pixels,
        over_3_sigma_expected: NULL_OVER_3_SIGMA_RATE,
        cluster_sizes,
        max_abs_z,
        max_abs_z_pixel,
        non_finite_samples: cpu_flat
            .iter()
            .map(|v| count_non_finite(&v.to_array()))
            .sum::<usize>()
            + count_non_finite(gpu_xyz),
    }
}

#[cfg(test)]
mod tray_shadow_tests {
    use super::*;
    use crate::{
        optics::raytracer::{
            EnvironmentSource,
            environment::{TrayMirror, tray_shadow::TrayShadowMap},
        },
        renderer::gpu::environment_check::build_tray_shadow_check_map,
    };

    /// Renders the CPU frame of the comparison scene (a few samples per pixel) with the tray
    /// feature `attach` adds to the rig (its flag says whether to attach it) and without it, and
    /// reports whether any pixel differs.
    fn tray_changes_the_cpu_frame<'t>(
        attach: impl Fn(EnvironmentSource<'static>, bool) -> EnvironmentSource<'t>,
    ) -> bool {
        let camera = test_camera();
        let (width, height) = (48u32, 48u32);
        let planes = round_brilliant_planes();
        let material = GemMaterial::diamond();
        let scene = CpuScene {
            camera: &camera,
            width,
            height,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 10,
        };
        let render = |on: bool| {
            let rig = LightingPreset::LightTent
                .studio(1.0, 0.4, 0.35)
                .with_backdrop(0.23)
                .with_surface_glare(1.0);
            let environment = attach(rig, on);
            cpu_samples(width, height, 8, 0, |pixel, sample| {
                cpu_sample_xyz(&scene, pixel, sample, environment)
            })
        };
        let attached = render(true);
        let plain = render(false);
        attached
            .iter()
            .zip(&plain)
            .any(|(a, b)| a.to_array() != b.to_array())
    }

    /// Whether the shadow `map` changes any pixel of the CPU frame.
    fn shadow_changes_the_cpu_frame(map: &TrayShadowMap) -> bool {
        tray_changes_the_cpu_frame(|rig, on| rig.with_tray_shadow(on.then_some(map)))
    }

    /// Whether the `mirror` changes any pixel of the CPU frame.
    fn mirror_changes_the_cpu_frame(mirror: TrayMirror) -> bool {
        tray_changes_the_cpu_frame(|rig, on| rig.with_tray_mirror(on.then_some(mirror)))
    }

    #[test]
    #[ignore = "statistical GPU frame comparison; run with --ignored"]
    fn a_shadowed_tray_matches_the_cpu_frame() {
        let Some(ctx) = crate::renderer::gpu::merge_tests::context_or_skip(
            "a_shadowed_tray_matches_the_cpu_frame",
        ) else {
            return;
        };
        // The plane sits at the stone's culet, so the shadow is physical and reaches the frame.
        let map = build_tray_shadow_check_map();
        assert!(
            shadow_changes_the_cpu_frame(&map),
            "the shadow must change at least one CPU pixel, or the twin proves nothing"
        );
        let result = run_image_comparison_tray_shadow(&ctx, &map);
        assert!(result.passed(), "{result:?}");
    }

    /// The mirror plane of the comparison scene: the shadow check map's plane, at the culet.
    fn check_plane_y() -> f32 {
        build_tray_shadow_check_map().plane_y()
    }

    #[test]
    #[ignore = "statistical GPU frame comparison; run with --ignored"]
    fn a_glossy_tray_matches_the_cpu_frame() {
        let Some(ctx) = crate::renderer::gpu::merge_tests::context_or_skip(
            "a_glossy_tray_matches_the_cpu_frame",
        ) else {
            return;
        };
        for r0 in [0.04_f32, 0.5] {
            let mirror = TrayMirror::new(check_plane_y(), r0).expect("valid mirror");
            assert!(
                mirror_changes_the_cpu_frame(mirror),
                "R0 {r0}: the mirror must change at least one CPU pixel, or the twin proves nothing"
            );
            let result = run_image_comparison_tray_mirror(&ctx, mirror, None);
            assert!(result.passed(), "R0 {r0}: {result:?}");
        }
    }

    #[test]
    #[ignore = "statistical GPU frame comparison; run with --ignored"]
    fn a_glossy_shadowed_tray_matches_the_cpu_frame() {
        let Some(ctx) = crate::renderer::gpu::merge_tests::context_or_skip(
            "a_glossy_shadowed_tray_matches_the_cpu_frame",
        ) else {
            return;
        };
        let map = build_tray_shadow_check_map();
        let mirror = TrayMirror::new(map.plane_y(), 0.04).expect("valid mirror");
        let result = run_image_comparison_tray_mirror(&ctx, mirror, Some(&map));
        assert!(result.passed(), "{result:?}");
    }
}
