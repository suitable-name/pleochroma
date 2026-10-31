//! Tests of oriented inclusions in a zoned host: the zoned estimator against the homogeneous one
//! for a single covering zone, the whole tracer against the unzoned stone, and the unchanged
//! zoned stone without inclusions.

use super::{
    super::{
        NUM_CHANNELS,
        absorption::channel_absorption_alphas_assigned,
        camera::{Camera, Ray},
        environment::LightingPreset,
        refraction::{RayMaterialContext, build_ray_wavelength_cache},
        sampling::hash_u32,
        transport::trace_spectral_ray,
    },
    oriented::{OrientedMedium, maybe_scatter_oriented},
    oriented_zoned::maybe_scatter_oriented_zoned,
};
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        inclusions::{FamilyKind, InclusionSet, ResolvedFamily},
        materials::{AbsorptionUnit, GemMaterial},
        polarization::StokesVector,
        zoning::{Zone, ZoneAbsorption, ZoneFrame, ZoneShape, ZonedAbsorption},
    },
    render_setup::{absorption_path_scale_for, measure_model_width},
};
use glam::{DVec3, Vec3};

const LAMBDAS: [f32; NUM_CHANNELS] = [420.0, 460.0, 500.0, 540.0, 580.0, 620.0, 660.0, 700.0];

fn band_tensor() -> AbsorptionTensor {
    AbsorptionTensor::isotropic(vec![AbsorptionBand::new(540.0, 60.0, 0.10)])
}

fn silk() -> InclusionSet {
    InclusionSet::new(&[ResolvedFamily {
        kind: FamilyKind::Needle,
        dir: Vec3::Y,
        sigma: 0.8,
        p: 2,
        lobe: 0.2,
        rho: 0.3,
    }])
}

fn probe(name: &str) -> GemMaterial {
    GemMaterial::new_custom(name, 1.76, 0.0, 0.0, [0.0, 0.0, 0.0])
}

/// A zoned material whose base zone and one half-space zone carry the same `tensor`.
fn identical_zones(tensor: &AbsorptionTensor, scale: f32) -> GemMaterial {
    probe("identical zones")
        .with_zoning(ZonedAbsorption {
            frame: ZoneFrame::IDENTITY,
            base: ZoneAbsorption::per_mm(tensor.clone()),
            zones: vec![Zone {
                shape: ZoneShape::HalfSpace {
                    normal: DVec3::X,
                    offset: -1.0e6,
                },
                absorption: ZoneAbsorption::per_mm(tensor.clone()),
            }],
            boundary_softness_mm: 0.0,
            follows_axis: false,
        })
        .with_absorption_path_scale(scale)
}

const fn context(material: &GemMaterial) -> RayMaterialContext<'_> {
    RayMaterialContext {
        material,
        lambdas: LAMBDAS,
        hero_idx: 0,
        c_axis: Vec3::Y,
        is_anisotropic: false,
        enable_internal_mode_coupling: true,
    }
}

const fn unit_stokes() -> [StokesVector; NUM_CHANNELS] {
    [StokesVector::unpolarized(1.0); NUM_CHANNELS]
}

/// One zone covering the segment: the zoned estimator makes the homogeneous one's decision,
/// free-flight distance, direction, weights and phase ratios.
#[test]
fn a_single_covering_zone_matches_the_homogeneous_oriented_estimator() {
    let scale = 3.0;
    let tensor = band_tensor();
    let plain = probe("plain")
        .with_chromophore_absorption(tensor.clone())
        .with_absorption_path_scale(scale);
    let covering = identical_zones(&tensor, scale);
    let set = silk();
    let ctx_plain = context(&plain);
    let ctx_zoned = context(&covering);
    let cache_plain = build_ray_wavelength_cache(&ctx_plain);
    let cache_zoned = build_ray_wavelength_cache(&ctx_zoned);
    let zoned_cache = cache_zoned.zoned.as_ref().expect("zoned cache");
    let k_hat = Vec3::new(1.0, 0.2, 0.3).normalize();
    let zone_alphas = zoned_cache.zone_alphas(&ctx_zoned, &cache_zoned, k_hat, false);
    let alphas = channel_absorption_alphas_assigned(&ctx_plain, &cache_plain, k_hat, false);
    let medium = OrientedMedium {
        set: &set,
        sigma_hg_model: 0.1,
        g: 0.3,
        lambdas: &LAMBDAS,
        hero_idx: 0,
        path_scale: scale,
    };
    let ray = Ray {
        origin: Vec3::new(-0.2, 0.1, -0.9),
        dir: k_hat,
    };
    let hit_t = 1.8;

    let (mut scattered, mut survived) = (0, 0);
    for rng_seed in 0..96_u32 {
        let (mut stokes_h, mut stokes_z) = (unit_stokes(), unit_stokes());
        let (mut pdf_h, mut pdf_z) = ([1.0_f32; NUM_CHANNELS], [1.0_f32; NUM_CHANNELS]);
        let homogeneous = maybe_scatter_oriented(
            &medium,
            &alphas,
            ray.dir,
            hit_t,
            rng_seed,
            0,
            &mut stokes_h,
            &mut pdf_h,
        );
        let zoned = maybe_scatter_oriented_zoned(
            &medium,
            zoned_cache,
            &zone_alphas,
            ray,
            hit_t,
            rng_seed,
            0,
            &mut stokes_z,
            &mut pdf_z,
        );
        assert_eq!(
            homogeneous.is_some(),
            zoned.is_some(),
            "seed {rng_seed}: scatter-or-survive must agree"
        );
        if let (Some(h), Some(z)) = (&homogeneous, &zoned) {
            scattered += 1;
            assert!(
                (h.t_free_model - z.t_free_model).abs() <= 1e-4 * h.t_free_model.max(1e-3),
                "seed {rng_seed}: {} vs {}",
                h.t_free_model,
                z.t_free_model
            );
            assert!((h.new_dir - z.new_dir).length() <= 1e-5, "seed {rng_seed}");
            for k in 0..NUM_CHANNELS {
                assert!(
                    (h.phase_ratio[k] - z.phase_ratio[k]).abs() <= 1e-4,
                    "seed {rng_seed}, channel {k}: phase ratio"
                );
            }
        } else {
            survived += 1;
        }
        for k in 0..NUM_CHANNELS {
            let (a, b) = (stokes_h[k].intensity(), stokes_z[k].intensity());
            assert!(
                (a - b).abs() <= 2e-3 * a.abs().max(1e-3),
                "seed {rng_seed}, channel {k}: weight {a} vs {b}"
            );
            let (a, b) = (pdf_h[k], pdf_z[k]);
            assert!(
                (a - b).abs() <= 2e-3 * a.abs().max(1e-3),
                "seed {rng_seed}, channel {k}: path pdf {a} vs {b}"
            );
        }
    }
    assert!(
        scattered > 0 && survived > 0,
        "the seeds must exercise both branches ({scattered} scattered, {survived} survived)"
    );
}

/// The mean of a grid of rays through the whole tracer (the studio rig has no NEE).
fn mean_radiance(material: &GemMaterial) -> Vec3 {
    const GRID: usize = 10;
    const SAMPLES: u32 = 24;
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let mut sum = Vec3::ZERO;
    for iy in 0..GRID {
        for ix in 0..GRID {
            let ray = camera.generate_ray(ix as f32, iy as f32, GRID as f32, GRID as f32, 0.5, 0.5);
            for s in 0..SAMPLES {
                let pixel_id = u32::try_from(iy * GRID + ix).expect("a small grid");
                let seed = hash_u32(pixel_id ^ hash_u32(s ^ 0x5CA7));
                sum += trace_spectral_ray(
                    ray,
                    &planes,
                    material,
                    16,
                    LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
                    seed,
                    (hash_u32(seed) as f32) / 4_294_967_295.0,
                    None,
                );
            }
        }
    }
    sum / (GRID * GRID) as f32 / SAMPLES as f32
}

fn path_scale() -> f32 {
    let planes = StandardGemCuts::standard_round_brilliant();
    absorption_path_scale_for(AbsorptionUnit::PerMm, 7.0, measure_model_width(&planes))
        .expect("measurable width")
}

/// A zoned stone with silk whose zones share the host absorption scatters like the unzoned stone
/// with the same silk.
#[test]
fn a_zoned_stone_with_silk_renders_like_the_unzoned_one() {
    let scale = path_scale();
    let tensor = band_tensor();
    let plain = probe("plain")
        .with_chromophore_absorption(tensor.clone())
        .with_absorption_path_scale(scale)
        .with_inclusions(silk());
    let zoned = identical_zones(&tensor, scale).with_inclusions(silk());
    assert!(zoned.oriented_inclusions().is_some() && zoned.scatters());
    let (a, b) = (mean_radiance(&plain), mean_radiance(&zoned));
    let rel = ((a - b).abs() / a.abs().max(Vec3::splat(1e-6))).max_element();
    assert!(
        rel < 0.03,
        "identical zones with silk must render like no zones: {a:?} vs {b:?} (relative {rel})"
    );
    // The silk is visible at all: the stone differs from the silk-free zoned one.
    let clear = mean_radiance(&identical_zones(&tensor, scale));
    assert!(
        ((a - clear).abs() / clear.abs().max(Vec3::splat(1e-6))).max_element() > 0.01,
        "silk must change the render: {a:?} vs {clear:?}"
    );
}

/// With no active inclusion set a zoned stone is the stone it was: clearing the set changes
/// nothing, bit for bit.
#[test]
fn a_zoned_stone_without_inclusions_is_unchanged() {
    let scale = path_scale();
    let tensor = band_tensor();
    let base = identical_zones(&tensor, scale);
    let cleared = base.clone().with_inclusions(InclusionSet::EMPTY);
    assert!(cleared.inclusions.is_none() && !cleared.scatters());
    assert_eq!(mean_radiance(&base), mean_radiance(&cleared));
}
