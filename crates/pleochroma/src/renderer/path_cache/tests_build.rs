//! Tests of the branch tree: energy, the first-surface Fresnel term, dispersion and determinism.

use glam::Vec3;

use super::{
    bounce::principal_weights,
    build::{DISPERSION_GATE_BG, LOWER_SLOT, PruneBy, TraceKit, UPPER_SLOT, principal_alphas},
    record::{ExitRecord, FLAG_GLARE, REFERENCE_SLOT},
    signature::fold_sig,
    tests_support::{Rig, angle_between, dispersive_glass, glass, uniaxial_glass},
};
use crate::{
    optics::{
        materials::GemMaterial,
        raytracer::{
            Ray, absorption::channel_absorption_alphas_assigned,
            intersect_stone::intersect_stone_soa_side, transport::PreparedMaterial,
        },
    },
    renderer::whitted::{CUTOFF, fresnel::fresnel_unpolarised_f32},
};

const SIZE: u32 = 16;

/// The ray through the centre of pixel `(x, y)` of the 16 x 16 test frame.
fn pixel_ray(rig: &Rig, x: u32, y: u32) -> Ray {
    rig.camera
        .generate_ray(x as f32, y as f32, SIZE as f32, SIZE as f32, 0.0, 0.0)
}

fn with_kit<R>(
    rig: &Rig,
    max_bounces: u32,
    gate: f32,
    f: impl FnOnce(&TraceKit<'_, '_>) -> R,
) -> R {
    let prepared = PreparedMaterial::new(&rig.material);
    let kit = TraceKit::new(
        &rig.planes,
        &rig.soa,
        &prepared,
        (max_bounces, CUTOFF),
        gate,
    );
    f(&kit)
}

/// Leaves, dropped branches and bounce-capped branches must add up to the unit of camera light.
fn accounted_energy(rig: &Rig, ray: Ray) -> f32 {
    with_kit(rig, 12, DISPERSION_GATE_BG, |kit| {
        let tree = kit.trace(ray, REFERENCE_SLOT, PruneBy::MaxChannel);
        let leaves: f32 = tree.leaves.iter().map(|leaf| leaf.weight[0]).sum();
        let pruned: f32 = tree.pruned.iter().map(|p| p.mean).sum();
        leaves + pruned + tree.truncated
    })
}

#[test]
fn white_stone_accounts_for_all_the_light_it_receives() {
    let rig = Rig::brilliant(glass());
    for (x, y) in [(8, 8), (5, 9), (10, 6), (7, 3)] {
        let total = accounted_energy(&rig, pixel_ray(&rig, x, y));
        assert!((total - 1.0).abs() < 2.0e-3, "pixel ({x}, {y}): {total}");
    }
}

#[test]
fn uniaxial_stone_accounts_for_the_light_it_receives_through_the_closed_form_solves() {
    let rig = Rig::brilliant(uniaxial_glass());
    for (x, y) in [(8, 8), (5, 9), (10, 6)] {
        let total = accounted_energy(&rig, pixel_ray(&rig, x, y));
        assert!((total - 1.0).abs() < 2.0e-2, "pixel ({x}, {y}): {total}");
    }
}

#[test]
fn the_first_surface_reflection_is_the_unpolarized_fresnel_reflectance() {
    let rig = Rig::brilliant(glass());
    let ray = pixel_ray(&rig, 8, 8);
    let (hit, _) = intersect_stone_soa_side(ray, &rig.soa, rig.planes.len(), &[])
        .expect("the centre ray hits the stone");
    let cos_i = (-ray.dir.dot(hit.normal)).clamp(0.0, 1.0);
    let index = rig.material.dispersion.evaluate(560.0);
    let expected = fresnel_unpolarised_f32(cos_i, 1.0 / index);
    with_kit(&rig, 12, DISPERSION_GATE_BG, |kit| {
        let tree = kit.trace(ray, REFERENCE_SLOT, PruneBy::MaxChannel);
        let glare: Vec<_> = tree
            .leaves
            .iter()
            .filter(|leaf| leaf.flags & FLAG_GLARE != 0)
            .collect();
        assert_eq!(glare.len(), 1, "exactly one first-surface reflection");
        for weight in glare[0].weight {
            assert!(
                (weight - expected).abs() < 2.0e-3,
                "reflectance {weight} vs Fresnel {expected}"
            );
        }
    });
}

#[test]
fn a_non_dispersive_stone_has_no_derivative_and_no_per_bin_records() {
    let rig = Rig::brilliant(glass());
    with_kit(&rig, 12, DISPERSION_GATE_BG, |kit| {
        assert!(!kit.all_per_bin());
        let mut records = 0;
        for y in (4..12).step_by(2) {
            for x in (4..12).step_by(2) {
                let output = kit.position(pixel_ray(&rig, x, y));
                for record in &output.records {
                    records += 1;
                    assert!(!record.is_per_bin(), "pixel ({x}, {y})");
                    assert_eq!(record.ddir_dlambda, [0, 0]);
                }
            }
        }
        assert!(records > 0);
    });
}

#[test]
fn the_dispersion_derivative_reproduces_the_per_bin_directions_on_sapphire() {
    let sapphire = GemMaterial::by_name("Sapphire").expect("Sapphire is a built-in material");
    let rig = Rig::brilliant(sapphire);
    with_kit(&rig, 12, DISPERSION_GATE_BG, |kit| {
        assert!(!kit.all_per_bin(), "sapphire is below the dispersion gate");
        let mut compared = 0;
        let mut worst = 0.0_f32;
        let mut curvature = 0.0_f32;
        for (x, y) in [(8, 8), (6, 9), (10, 7), (9, 5)] {
            let ray = pixel_ray(&rig, x, y);
            let output = kit.position(ray);
            let reference = kit.trace(ray, REFERENCE_SLOT, PruneBy::MaxChannel);
            let lower = kit.trace(ray, LOWER_SLOT, PruneBy::Hero(LOWER_SLOT));
            let upper = kit.trace(ray, UPPER_SLOT, PruneBy::Hero(UPPER_SLOT));
            for record in output.records.iter().filter(|r| !r.is_per_bin()) {
                let at = |tree: &super::build::Tree| {
                    tree.leaves
                        .iter()
                        .find(|leaf| fold_sig(leaf.branch) == record.signature)
                        .map(|leaf| leaf.dir)
                };
                if let (Some(low), Some(high), Some(middle)) =
                    (at(&lower), at(&upper), at(&reference))
                {
                    compared += 1;
                    // What a straight line through the end points cannot follow: the distance of
                    // the 560 nm direction from the chord midpoint (half the second difference).
                    curvature = curvature.max(angle_between((low + high).normalize(), middle));
                    worst = worst
                        .max(angle_between(record.direction_at(510.0), low))
                        .max(angle_between(record.direction_at(610.0), high));
                }
            }
        }
        assert!(compared > 0, "no merged record had a per-wavelength twin");
        // A straight line through the 510 and 610 nm points misses the curved dispersion at those
        // two points by half the second difference at 560 nm (`curvature`, a few tenths of a
        // milliradian on sapphire). Beyond that only the 16-bit octahedral and half-float
        // quantization may remain, well below 1e-4 rad.
        assert!(
            worst < curvature + 1.0e-4,
            "worst direction error {worst} rad over {compared} records, curvature {curvature}"
        );
    });
}

#[test]
fn the_material_gate_makes_every_record_per_bin() {
    let rig = Rig::brilliant(dispersive_glass());
    with_kit(&rig, 12, DISPERSION_GATE_BG, |kit| {
        assert!(kit.all_per_bin());
        let output = kit.position(pixel_ray(&rig, 8, 8));
        assert_ne!(output.records, [] as [ExitRecord; 0]);
        let mut bins = Vec::new();
        for record in &output.records {
            assert!(record.is_per_bin());
            let weights = record.weights();
            assert!(weights[record.bin()] > 0.0);
            for (slot, weight) in weights.iter().enumerate() {
                if slot != record.bin() {
                    assert_eq!(*weight, 0.0);
                }
            }
            bins.push(record.bin());
        }
        bins.sort_unstable();
        bins.dedup();
        assert!(bins.len() >= 2, "bins seen: {bins:?}");
    });
}

#[test]
fn a_divergent_facet_sequence_produces_per_bin_records() {
    let rig = Rig::brilliant(dispersive_glass());
    // The gate is lifted so only the facet-sequence test can ask for per-bin records.
    with_kit(&rig, 12, 10.0, |kit| {
        assert!(!kit.all_per_bin());
        let (mut per_bin, mut merged) = (0, 0);
        for y in 4..12 {
            for x in 4..12 {
                for record in kit.position(pixel_ray(&rig, x, y)).records {
                    if record.is_per_bin() {
                        per_bin += 1;
                    } else {
                        merged += 1;
                    }
                }
            }
        }
        assert!(
            per_bin > 0,
            "a stone this dispersive must split some branch"
        );
        assert!(merged > 0, "most branches are still merged");
    });
}

#[test]
fn a_position_is_deterministic() {
    for material in [glass(), uniaxial_glass(), dispersive_glass()] {
        let rig = Rig::brilliant(material);
        let ray = pixel_ray(&rig, 7, 8);
        let first = with_kit(&rig, 12, DISPERSION_GATE_BG, |kit| kit.position(ray));
        let second = with_kit(&rig, 12, DISPERSION_GATE_BG, |kit| kit.position(ray));
        assert_eq!(first.records, second.records);
        assert_eq!(first.residual.to_bits(), second.residual.to_bits());
        assert_eq!(first.signature, second.signature);
    }
}

#[test]
fn a_ray_that_misses_the_stone_is_one_direct_record() {
    let rig = Rig::brilliant(glass());
    let ray = Ray {
        origin: Vec3::new(0.0, 5.0, 0.0),
        dir: Vec3::new(1.0, 0.2, 0.0).normalize(),
    };
    with_kit(&rig, 12, DISPERSION_GATE_BG, |kit| {
        let output = kit.position(ray);
        assert_eq!(output.records.len(), 1);
        assert!(output.records[0].is_direct());
        assert_eq!(output.residual, 0.0);
        assert_eq!(output.signature, 0);
        assert!(angle_between(output.records[0].direction(), ray.dir) < 1.0e-4);
        for weight in output.records[0].weights() {
            assert_eq!(weight, 1.0);
        }
    });
}

/// `principal_weights` times the band sums is the tracer's per-channel absorption coefficient, at
/// wavelengths off the comb, for an isotropic, a uniaxial and a biaxial material with three
/// spectra.
#[test]
fn the_principal_weights_reproduce_the_tracers_absorption_coefficients() {
    let lambdas = [397.0, 431.0, 487.0, 533.0, 585.0, 612.0, 701.0, 779.0];
    let materials = [
        GemMaterial::new_custom("isotropic probe", 1.62, 0.01, 0.0, [0.3, 0.6, 0.9]),
        GemMaterial::sapphire(),
        GemMaterial::by_name("Tanzanite").expect("Tanzanite is a built-in material"),
    ];
    let directions = [
        Vec3::new(0.3, -0.9, 0.2).normalize(),
        Vec3::new(-0.6, 0.1, 0.75).normalize(),
        Vec3::new(0.05, 0.99, -0.1).normalize(),
    ];
    assert!(
        materials[2].absorption.beta_ray.is_some(),
        "the biaxial case has three spectra"
    );
    for material in &materials {
        let prepared = PreparedMaterial::new(material);
        let spectra = principal_alphas(material, &lambdas);
        for hero in [0, 4] {
            let (ctx, cache) = prepared.ray_context(lambdas, hero);
            for k_hat in directions {
                for is_ext in [false, true] {
                    let alphas = channel_absorption_alphas_assigned(&ctx, &cache, k_hat, is_ext);
                    let weights = principal_weights(&ctx, &cache, k_hat, is_ext);
                    for (k, &alpha) in alphas.iter().enumerate() {
                        let rebuilt = weights[0].mul_add(
                            spectra[k][0],
                            weights[1].mul_add(spectra[k][1], weights[2] * spectra[k][2]),
                        );
                        assert!(
                            (rebuilt - alpha).abs() <= 2.0e-5 * alpha.abs().max(1.0),
                            "{} hero {hero} ext {is_ext} channel {k}: {rebuilt} vs {alpha}",
                            material.name
                        );
                    }
                }
            }
        }
    }
}
