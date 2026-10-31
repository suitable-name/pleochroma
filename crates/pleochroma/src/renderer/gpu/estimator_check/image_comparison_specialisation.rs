//! Tier 3: the GENERIC-vs-specialised statistical image comparison, split out of
//! `image_comparison` for length.
//!
//! Two different things are checked here, and they are separate functions on purpose:
//!
//! - [`run_specialisation_image_comparison`]: the KIND of the specialised pipeline
//!   (isotropic, uniaxial, biaxial). Its GENERIC side already compiles the material's own
//!   feature bits in (`dispatch_transport_for_class` derives them from the uploaded
//!   material), so it cannot see a wrong feature bit.
//! - [`run_specialisation_feature_image_comparison`]: the FEATURE bits
//!   (`NO_ORIENTED`, `NO_ZONED_ORIENTED`, `NO_SCATTER_NEE`, `NO_EXTRAORDINARY_DISPERSION`).
//!   Its GENERIC side is the pipeline with every feature compiled in, the `override`
//!   defaults, so a bit that wrongly compiles out something the material needs shows as a
//!   difference between the two images.

use crate::{
    optics::{
        materials::GemMaterial,
        raytracer::{
            LightingPreset, environment::environment_white_balance, illuminant_temperature_k,
        },
    },
    renderer::{
        buffers::{GpuGemMaterial, GpuTransportParams, encode_facet_finishes, transport_env_mode},
        env_map_gpu::HdrEnvGpuData,
        gpu::{
            frame::{self, classify_material},
            z_stats::{
                NULL_OVER_3_SIGMA_RATE, SIGMA_THRESHOLD, connected_components, count_non_finite,
            },
        },
    },
};

use super::{
    MaterialForDispatch, SceneBuffersForDispatch, Welford, camera_params_for,
    dispatch_transport_for_class, dispatch_transport_with_pipeline_class,
    image_comparison::{ImageComparisonResult, synthetic_image_comparison_hdr_map},
    image_comparison_oriented::with_preset,
    quartz_material, round_brilliant_planes, test_camera, z_score,
};

/// The GENERIC-vs-specialised correctness gate for the pipeline KIND.
///
/// `renderer::gpu::frame::run_specialisation_equivalence` documents why the GENERIC and
/// specialised pipelines are not guaranteed byte-identical on the same input
/// (dead-code elimination can flip a stochastic-branch threshold comparison by 1 ULP).
/// That is the same class of divergence this module already tolerates statistically
/// between the CPU and GPU estimators, so this function verifies it the same way: two
/// GPU dispatches of `material` over DISJOINT sample ranges (mirroring
/// [`super::image_comparison::run_image_comparison_for`]'s CPU/GPU split, both sides now GPU) -- one through the
/// GENERIC pipeline, one through whichever specialised pipeline [`classify_material`]
/// picks -- compared via the same Welford mean/M2, per-pixel z-score, and
/// connected-component clustering [`ImageComparisonResult`] already carries. A wrong
/// specialisation guard would bias the whole image, not flip isolated pixels, so
/// [`ImageComparisonResult::passed`]'s existing criteria apply unchanged.
///
/// The GENERIC side carries the material's own feature bits (see the module doc), so a wrong
/// FEATURE bit is [`run_specialisation_feature_image_comparison`]'s to catch, not this
/// function's.
///
/// # Panics
///
/// Panics if `material.gpu_supported()` is false for `material` -- self-test scaffolding
/// callers always pass a GPU-supported built-in. Also panics if a pixel index does not fit
/// `u32`, which `width * height` in `u32` rules out.
#[must_use]
pub fn run_specialisation_image_comparison(
    ctx: &crate::renderer::gpu::GpuContext,
    material: &GemMaterial,
) -> ImageComparisonResult {
    let comparison = SpecialisationComparison {
        material,
        hdr: false,
        generic_has_every_feature: false,
    };
    comparison.run(ctx, 1)
}

/// The materials [`run_specialisation_feature_image_comparison`] checks: one per compile-time
/// feature of the `transport_main` pipelines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialisationFeatureCase {
    /// Quartz: a genuine extraordinary-ray dispersion curve (`EXTRAORDINARY_DISPERSION`),
    /// and no inclusions or zones (so `ORIENTED_INCLUSIONS` and `ZONED_ORIENTED` are off).
    ///
    /// The all-features GENERIC side of this case is the pipeline shape that broke uniaxial
    /// stones on AMD's RADV (reachable oriented code, 2026-10-22): a failure there on such a
    /// driver is that fault, not a drifted feature bit; compare with an NVIDIA adapter.
    ExtraordinaryCurve,
    /// Ruby with Star Silk under an HDR map: the scattering-point next-event estimate
    /// (`SCATTER_NEE`) and the oriented estimator are needed, only `ZONED_ORIENTED` is off.
    ScatterNee,
    /// The zoned dichroic rim with Star Silk under an HDR map: the zoned oriented estimator is
    /// needed, and `SCATTER_NEE` is compiled out because such a host never takes that estimate.
    ZonedOriented,
}

impl SpecialisationFeatureCase {
    /// Every case.
    pub const ALL: [Self; 3] = [
        Self::ExtraordinaryCurve,
        Self::ScatterNee,
        Self::ZonedOriented,
    ];

    /// The label the harness prints.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ExtraordinaryCurve => "Quartz, extraordinary curve (studio)",
            Self::ScatterNee => "Ruby, Star Silk, scattering NEE (HDR map)",
            Self::ZonedOriented => "zoned dichroic rim, Star Silk, zoned oriented (HDR map)",
        }
    }

    /// Whether the case renders under the HDR map (else the Daylight studio rig).
    const fn hdr(self) -> bool {
        !matches!(self, Self::ExtraordinaryCurve)
    }

    /// The case's material.
    ///
    /// # Panics
    ///
    /// Panics if `"Ruby"` or a corundum `"Star Silk"` preset is ever removed.
    #[must_use]
    pub fn material(self) -> GemMaterial {
        match self {
            Self::ExtraordinaryCurve => quartz_material(),
            Self::ScatterNee => {
                let ruby = GemMaterial::by_name("Ruby")
                    .expect("\"Ruby\" is a built-in material in GemMaterial::all_materials()");
                with_preset(ruby, "Ruby", "Star Silk")
            }
            Self::ZonedOriented => {
                let rim = crate::renderer::gpu::zoned_cases::ZonedImageCase::DichroicRim.material();
                with_preset(rim, "Sapphire", "Star Silk")
            }
        }
    }
}

/// The FEATURE-bit gate: `case`'s material through the pipeline [`classify_material`] picks.
///
/// The pipeline is picked by kind and feature bits, and compared
/// against the GENERIC pipeline with EVERY feature compiled in, over
/// disjoint sample ranges, on the X, Y and Z components in that order.
///
/// All three components are compared because a feature that is wrongly compiled out (a missing
/// extraordinary curve, an index error) can move the hue and leave the luminance alone. Each
/// result passes on [`ImageComparisonResult::passed`]; the check passes when all three do.
#[must_use]
pub fn run_specialisation_feature_image_comparison(
    ctx: &crate::renderer::gpu::GpuContext,
    case: SpecialisationFeatureCase,
) -> [ImageComparisonResult; 3] {
    let material = case.material();
    let comparison = SpecialisationComparison {
        material: &material,
        hdr: case.hdr(),
        generic_has_every_feature: true,
    };
    comparison.run_all_components(ctx)
}

/// One GENERIC-vs-specialised comparison of `material`.
struct SpecialisationComparison<'a> {
    material: &'a GemMaterial,
    /// Render under the synthetic HDR map (else the Daylight studio rig).
    hdr: bool,
    /// The GENERIC side compiles every feature in (class 0) instead of the material's own
    /// feature bits.
    generic_has_every_feature: bool,
}

/// Both dispatches of a comparison.
struct SpecialisationImages {
    width: u32,
    height: u32,
    samples_per_pixel: u32,
    generic_xyz: Vec<f32>,
    specialised_xyz: Vec<f32>,
}

impl SpecialisationComparison<'_> {
    /// The comparison on the `channel` component (0 = X, 1 = Y, 2 = Z).
    fn run(&self, ctx: &crate::renderer::gpu::GpuContext, channel: usize) -> ImageComparisonResult {
        let images = self.dispatch(ctx);
        compare_dispatch_images(&images, channel)
    }

    /// The comparison on X, Y and Z from one pair of dispatches.
    fn run_all_components(
        &self,
        ctx: &crate::renderer::gpu::GpuContext,
    ) -> [ImageComparisonResult; 3] {
        let images = self.dispatch(ctx);
        [0, 1, 2].map(|channel| compare_dispatch_images(&images, channel))
    }

    fn dispatch(&self, ctx: &crate::renderer::gpu::GpuContext) -> SpecialisationImages {
        let material = self.material;
        let camera = test_camera();
        let (width, height) = (48u32, 48u32);
        let planes = round_brilliant_planes();
        let gpu_material = GpuGemMaterial::encode(material);
        let gpu_finishes = encode_facet_finishes(&[], planes.len());
        let max_bounces = 10u32;
        let preset = LightingPreset::Daylight;
        let (exposure, light_yaw, light_pitch) = (1.0f32, 0.4f32, 0.35f32);
        let temp_k = illuminant_temperature_k(preset);
        let wb = environment_white_balance(preset.studio(exposure, light_yaw, light_pitch));

        let samples_per_pixel = 500u32;
        let camera_params = camera_params_for(&camera, width, height, samples_per_pixel);
        let total = (width * height * samples_per_pixel) as usize;

        // Shared by both dispatches below (`sample_offset` is the only thing that differs)
        // so the D65 flag can't independently drift between the GENERIC and specialised
        // params the way two separate call sites could.
        let hdr = self.hdr;
        let build_params = |sample_offset: u32| {
            if hdr {
                GpuTransportParams::new(
                    width * height,
                    max_bounces,
                    sample_offset,
                    transport_env_mode::HDR_MAP,
                    1.0,
                    6500.0,
                    1.0,
                    1.0,
                    0.0,
                    0.0,
                    [1.0, 1.0, 1.0],
                )
            } else {
                GpuTransportParams::new(
                    width * height,
                    max_bounces,
                    sample_offset,
                    transport_env_mode::STUDIO_RIG,
                    0.0,
                    temp_k,
                    preset.params().spot_mult,
                    exposure,
                    light_yaw,
                    light_pitch,
                    wb.to_array(),
                )
                .with_studio_use_d65(preset.uses_d65())
                .with_studio_model(preset.model().gpu_id())
                .with_tent(preset.params().tent)
            }
        };
        let env_map = hdr.then(synthetic_image_comparison_hdr_map);
        let gpu_hdr = env_map
            .as_ref()
            .map(|map| HdrEnvGpuData::upload(&ctx.device, map));
        let scene = SceneBuffersForDispatch {
            planes: &planes,
            facet_finishes: &gpu_finishes,
            hdr_env: gpu_hdr.as_ref(),
        };

        // GENERIC pipeline: samples [0, samples_per_pixel).
        let generic_params = build_params(0);
        let generic_dispatch = if self.generic_has_every_feature {
            // No feature bits: every estimator compiled in, the `override` defaults.
            dispatch_transport_with_pipeline_class(
                ctx,
                &camera_params,
                &generic_params,
                (&gpu_material, frame::material_class::GENERIC),
                scene,
                total,
            )
        } else {
            dispatch_transport_for_class(
                ctx,
                &camera_params,
                &generic_params,
                MaterialForDispatch {
                    encoded: &gpu_material,
                    class: frame::material_class::GENERIC,
                },
                scene,
                total,
            )
        };

        // Specialised pipeline: samples [samples_per_pixel, 2*samples_per_pixel) -- a
        // DISJOINT range from the GENERIC dispatch above, like
        // `run_image_comparison_for`'s CPU/GPU split.
        let specialised_params = build_params(samples_per_pixel);
        let specialised_dispatch = dispatch_transport_for_class(
            ctx,
            &camera_params,
            &specialised_params,
            MaterialForDispatch {
                encoded: &gpu_material,
                class: classify_material(material),
            },
            scene,
            total,
        );

        SpecialisationImages {
            width,
            height,
            samples_per_pixel,
            generic_xyz: generic_dispatch.xyz,
            specialised_xyz: specialised_dispatch.xyz,
        }
    }
}

/// Compares the two disjoint GPU sample sets using the shared image criteria, on one XYZ
/// component.
fn compare_dispatch_images(images: &SpecialisationImages, channel: usize) -> ImageComparisonResult {
    let (width, height, samples_per_pixel) =
        (images.width, images.height, images.samples_per_pixel);
    let num_pixels = (width * height) as usize;
    let z_grid: Vec<f64> = (0..num_pixels)
        .map(|pixel| {
            pixel_specialisation_z_score(
                pixel,
                samples_per_pixel,
                (&images.generic_xyz, &images.specialised_xyz),
                channel,
            )
        })
        .collect();

    let mut mean_z_sum = 0.0f64;
    let mut max_abs_z = 0.0f64;
    let mut max_abs_z_pixel = (0u32, 0u32);
    for (pixel, &z) in z_grid.iter().enumerate() {
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
        cpu_samples_per_pixel: samples_per_pixel,
        gpu_samples_per_pixel: samples_per_pixel,
        mean_z: mean_z_sum / num_pixels as f64,
        over_3_sigma_count,
        total_pixels: num_pixels,
        over_3_sigma_expected: NULL_OVER_3_SIGMA_RATE,
        cluster_sizes,
        max_abs_z,
        max_abs_z_pixel,
        non_finite_samples: count_non_finite(&images.generic_xyz)
            + count_non_finite(&images.specialised_xyz),
    }
}

/// One pixel's z-score between a GENERIC-pipeline and a specialised-pipeline sample set
/// on the XYZ component `channel`
/// -- factored out of [`run_specialisation_image_comparison`] so that function's own
/// per-pixel loop stays a plain `enumerate()` over the result (clippy's function-length
/// and index-loop lints both want this split out, and it also documents the per-pixel
/// Welford construction as its own named step).
///
/// `xyz` is `(generic, specialised)`.
fn pixel_specialisation_z_score(
    pixel: usize,
    samples_per_pixel: u32,
    xyz: (&[f32], &[f32]),
    channel: usize,
) -> f64 {
    let (generic_xyz, specialised_xyz) = xyz;
    let mut generic_acc = Welford::default();
    let mut specialised_acc = Welford::default();
    for s in 0..samples_per_pixel as usize {
        let idx = (pixel * samples_per_pixel as usize + s) * 3 + channel;
        generic_acc.update(f64::from(generic_xyz[idx]));
        specialised_acc.update(f64::from(specialised_xyz[idx]));
    }
    z_score(&generic_acc, &specialised_acc)
}
