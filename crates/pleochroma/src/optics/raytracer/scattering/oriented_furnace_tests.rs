//! White-furnace energy conservation of oriented scatterers through the public
//! `trace_spectral_ray`: a lossless stone in a uniform environment must render at that
//! environment's own radiance, whatever the phase function. Grey silk (a needle family with no
//! spectral slope) and a Bragg structure (opal) are the two estimators the plain-haze furnace
//! tests in `tests.rs` do not reach.
//!
//! Each case traces 6 400 rays (10 x 10 pixels, 64 samples) at 64 bounces, which keeps a test to
//! seconds. At that budget the per-sample spread of a lossless stone in a uniform map is small
//! (the 12 x 12 x 96 haze furnace in `tests.rs` measures 0.2 percent noise), so the tolerance
//! below is dominated by the few percent of paths a bounce cap cuts off, and it is set to
//! several standard errors above that.

use super::super::{
    camera::Camera, color::cie_1931_cmf, environment::EnvironmentSource, sampling::hash_u32,
    transport::trace_spectral_ray,
};
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        inclusions::{
            FamilyKind, InclusionSet, ResolvedFamily,
            bragg::{StructureKind, StructureSpec},
        },
        materials::GemMaterial,
    },
    renderer::env_map::{EnvironmentMap, rgb_to_spectral_radiance},
};
use glam::Vec3;

const L0: f32 = 2.5;
const GRID: usize = 10;
const SAMPLES_PER_PIXEL: u32 = 64;
const MAX_BOUNCES: u32 = 64;

/// The mean radiance of `material` over the face-up view in a uniform map of radiance [`L0`],
/// and the radiance the map itself integrates to (the furnace target), both as XYZ.
fn furnace_mean_and_target(material: &GemMaterial, salt: u32) -> (Vec3, Vec3) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let env_map = EnvironmentMap::uniform(1, 1, [L0, L0, L0]);
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let mut sum = Vec3::ZERO;
    let mut count = 0u32;
    for iy in 0..GRID {
        for ix in 0..GRID {
            let ray = camera.generate_ray(ix as f32, iy as f32, GRID as f32, GRID as f32, 0.5, 0.5);
            for s in 0..SAMPLES_PER_PIXEL {
                let pixel_id = u32::try_from(iy * GRID + ix).expect("the pixel index fits u32");
                let seed = hash_u32(pixel_id ^ hash_u32(s ^ salt));
                sum += trace_spectral_ray(
                    ray,
                    &planes,
                    material,
                    MAX_BOUNCES,
                    EnvironmentSource::HdrMap(&env_map),
                    seed,
                    (hash_u32(seed) as f32) / 4_294_967_295.0,
                    None,
                );
                count += 1;
            }
        }
    }
    let mut target = Vec3::ZERO;
    for step in 0..=(780 - 380) {
        let lambda = 380.0f32 + step as f32;
        target += cie_1931_cmf(lambda) * rgb_to_spectral_radiance([L0, L0, L0], lambda);
    }
    target /= crate::color::cie1931::CIE_1931_Y_INTEGRAL_5NM;
    (sum / count as f32, target)
}

fn assert_furnace(label: &str, mean: Vec3, target: Vec3, tolerance: f32) {
    let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
    let errs = [
        rel_err(mean.x, target.x),
        rel_err(mean.y, target.y),
        rel_err(mean.z, target.z),
    ];
    println!("[{label} furnace] mean={mean:?} target={target:?} rel_err={errs:?}");
    assert!(
        errs.iter().all(|e| *e <= tolerance),
        "{label}: a lossless stone must render at the uniform environment's radiance \
         (mean={mean:?}, target={target:?}, rel_err={errs:?}, tolerance={tolerance})"
    );
}

/// The colorless, non-dispersive probe stone of the other furnace tests.
fn lossless_stone() -> GemMaterial {
    GemMaterial::new_custom("oriented furnace probe", 1.5, 0.0, 0.0, [0.0, 0.0, 0.0])
}

/// Grey silk (`p = 0` needles, two directions, forward-leaning azimuth) in a lossless stone:
/// the oriented estimator redirects light, it must neither create nor destroy any.
#[test]
fn grey_silk_in_a_lossless_stone_passes_the_white_furnace() {
    let needle = |dir: Vec3| ResolvedFamily {
        kind: FamilyKind::Needle,
        dir,
        sigma: 0.9,
        p: 0,
        lobe: 0.08,
        rho: 0.4,
    };
    let set = InclusionSet::new(&[
        needle(Vec3::new(1.0, 0.0, 0.0)),
        needle(Vec3::new(0.3, 0.9, 0.1).normalize()),
    ]);
    let material = lossless_stone().with_inclusions(set);
    assert!(material.oriented_inclusions().is_some());
    let (mean, target) = furnace_mean_and_target(&material, 0x5117_4B01);
    assert_furnace("grey silk", mean, target, 0.03);
}

/// An opal's Bragg domains in a lossless stone: a reflection takes light out of one direction and
/// puts the same amount into another, so the furnace still reads the environment's radiance.
#[test]
fn a_bragg_structure_in_a_lossless_stone_passes_the_white_furnace() {
    let spec = StructureSpec::default_for("Precious Opal (White)").expect("opal default");
    let material = lossless_stone().with_structure_spec_as(StructureKind::Opal, &spec);
    assert!(material.oriented_inclusions().is_some());
    let (mean, target) = furnace_mean_and_target(&material, 0xB4A6_6001);
    assert_furnace("bragg opal", mean, target, 0.04);
}
