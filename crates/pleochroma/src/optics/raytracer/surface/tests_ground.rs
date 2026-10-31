//! Tests of the ground facet, its energy compensation, and the polished halo hooks.

use glam::Vec3;

use super::{
    EdgeAbrasion, FacetSurface, FacetingMachine, GgxLobe, GroundCtx, GroundInterface, GroundParams,
    PolishPreset, SurfaceScene,
    compensation::{albedo_by_quadrature, ground_albedo},
    dispatch::{SurfaceOutcome, SurfaceQuery, begin_facet_surface, finish_facet_surface},
    edge::in_edge_band,
    micro::reflect_about,
    tis::tis_reflection,
};
use crate::{
    geometry::plane::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        polarization::StokesVector,
        raytracer::{camera::FacetFinish, hash_u32},
    },
};

/// A deterministic uniform number in `[0, 1]` for the index `i` of the stream `salt`.
fn uniform(i: u32, salt: u32) -> f32 {
    hash_u32(i.wrapping_mul(0x9E37_79B9) ^ salt) as f32 / 4_294_967_295.0
}

/// A ground event for a ray meeting a facet with normal `+Z` at incidence cosine `cos_i`,
/// travelling from medium `n1` to `n2`.
fn ground_ctx(alpha: f32, cos_i: f32, n1: f32, n2: f32) -> GroundCtx {
    let sin_i = cos_i.mul_add(-cos_i, 1.0).sqrt();
    let k_in = Vec3::new(-sin_i, 0.0, -cos_i);
    let eta = n1 / n2;
    GroundCtx::new(
        Vec3::Z,
        k_in,
        GroundParams {
            lobe: GgxLobe::isotropic(alpha),
            across: None,
        },
        GroundInterface {
            n1,
            n2,
            cos_i,
            sin2_t: eta * eta * cos_i.mul_add(-cos_i, 1.0),
        },
    )
}

/// A lossless rough interface under the white furnace keeps its energy: the mean throughput of
/// an event is one, from outside and from inside, at every preset roughness.
///
/// The mean is taken over a fine grid of the micro-normal draw with the two branches weighted by
/// their selection probabilities, which is the expectation of the sampler without its Monte Carlo
/// noise.
#[test]
fn the_compensated_ground_event_conserves_energy() {
    let grid = 200u32;
    for (n1, n2) in [(1.0f32, 1.5f32), (1.5, 1.0)] {
        for alpha in [0.1f32, 0.3, 0.5] {
            for degrees in [10.0f32, 30.0, 50.0, 70.0] {
                let ctx = ground_ctx(alpha, degrees.to_radians().cos(), n1, n2);
                let mut total = 0.0f64;
                for a in 0..grid {
                    for b in 0..grid {
                        let u1 = (a as f32 + 0.5) / grid as f32;
                        let u2 = (b as f32 + 0.5) / grid as f32;
                        // A branch draw of 0 reflects, of 1 transmits (the selection
                        // probability never reaches 1).
                        for u_branch in [0.0f32, 1.0] {
                            let s = ctx.sample(u1, u2, u_branch);
                            total += f64::from(s.branch_prob * s.weight);
                        }
                    }
                }
                let mean = total / f64::from(grid * grid);
                assert!(
                    (mean - 1.0).abs() < 0.01,
                    "n {n1}->{n2}, alpha {alpha}, {degrees} deg: mean throughput {mean}"
                );
            }
        }
    }
}

/// The uncompensated single-scatter loss at alpha 0.5 is real (the number D11 records), and the
/// table is bounded and smooth.
#[test]
fn the_uncompensated_loss_at_alpha_half_is_recorded() {
    for (cos_i, eta) in [(0.9f32, 1.5f32), (0.5, 1.5), (0.5, 1.0 / 1.5)] {
        let albedo = albedo_by_quadrature(0.5, cos_i, eta);
        assert!(albedo > 0.6 && albedo < 0.999, "{cos_i} {eta}: {albedo}");
        // The table, at a node of the roughness axis, agrees with the quadrature it holds.
        let tabulated = ground_albedo(0.5, cos_i, eta);
        assert!((tabulated - albedo).abs() < 0.01, "{tabulated} vs {albedo}");
    }
    // A nearly smooth lobe loses nothing.
    assert!(albedo_by_quadrature(0.05, 0.9, 1.5) > 0.98);
}

/// `eval` of the direction a sample produced returns that sample's weight and density: the
/// half-vector and Jacobian algebra of the light sampling agrees with the sampler.
#[test]
fn the_lobe_evaluation_agrees_with_the_sampler() {
    for (n1, n2) in [(1.0f32, 1.7f32), (1.7, 1.0)] {
        let ctx = ground_ctx(0.45, 0.6, n1, n2);
        let (mut reflected, mut transmitted) = (0, 0);
        for i in 0..60_000u32 {
            let s = ctx.sample(uniform(i, 5), uniform(i, 6), uniform(i, 7));
            if !s.valid {
                continue;
            }
            let eval = ctx
                .eval(s.dir, s.reflect_branch)
                .expect("a sampled direction has a density");
            assert!(
                (eval.pdf / s.pdf - 1.0).abs() < 2e-3,
                "pdf {} vs {} (reflect {})",
                eval.pdf,
                s.pdf,
                s.reflect_branch
            );
            assert!(
                (eval.weight - s.weight).abs() <= 2e-3 * s.weight.max(1e-3),
                "weight {} vs {} (reflect {})",
                eval.weight,
                s.weight,
                s.reflect_branch
            );
            if s.reflect_branch {
                reflected += 1;
            } else {
                transmitted += 1;
            }
        }
        assert!(
            reflected > 100 && transmitted > 20,
            "{reflected} {transmitted}"
        );
    }
}

/// The density of a branch integrates to the fraction of valid samples of that branch: a Monte
/// Carlo integral over uniformly drawn directions against the sampler's own count.
#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn the_branch_densities_integrate_to_the_valid_fraction() {
    let ctx = ground_ctx(0.5, 0.7, 1.0, 1.5);
    let draws = 200_000u32;
    let (mut reflect_valid, mut reflect_total) = (0u32, 0u32);
    let (mut transmit_valid, mut transmit_total) = (0u32, 0u32);
    for i in 0..draws {
        // The branch is drawn by the third number alone, so use both branches explicitly.
        let r = ctx.sample(uniform(i, 21), uniform(i, 22), 0.0);
        let t = ctx.sample(uniform(i, 21), uniform(i, 22), 1.0);
        reflect_total += 1;
        transmit_total += 1;
        reflect_valid += u32::from(r.valid);
        transmit_valid += u32::from(t.valid);
    }
    for (reflect, valid, total) in [
        (true, reflect_valid, reflect_total),
        (false, transmit_valid, transmit_total),
    ] {
        // Uniform directions on the sphere; the density is zero on the wrong side.
        let points = 400_000u32;
        let mut sum = 0.0f64;
        for i in 0..points {
            let z = 2.0 * uniform(i, 31) - 1.0;
            let phi = std::f32::consts::TAU * uniform(i, 32);
            let r = z.mul_add(-z, 1.0).max(0.0).sqrt();
            let o = Vec3::new(r * phi.cos(), r * phi.sin(), z);
            if let Some(eval) = ctx.eval(o, reflect) {
                sum += f64::from(eval.pdf);
            }
        }
        let integral = sum / f64::from(points) * 4.0 * std::f64::consts::PI;
        let fraction = f64::from(valid) / f64::from(total);
        assert!(
            (integral - fraction).abs() < 0.02,
            "reflect {reflect}: integral {integral}, valid fraction {fraction}"
        );
    }
}

/// A very smooth ground lobe is the mirror: the reflected direction is the specular one and the
/// event is the Fresnel split.
#[test]
fn a_nearly_smooth_ground_lobe_reflects_specularly() {
    let ctx = ground_ctx(0.001, 0.8, 1.0, 1.5);
    let sin_i = 0.6f32;
    // The ray travels along (-0.6, 0, -0.8) and leaves along (-0.6, 0, 0.8).
    let specular = Vec3::new(-sin_i, 0.0, 0.8);
    let s = ctx.sample(0.3, 0.6, 0.0);
    assert!(s.valid && s.reflect_branch);
    assert!((s.dir - specular).length() < 5e-3, "{:?}", s.dir);
}

fn query<'a>(
    scene: &'a SurfaceScene<'a>,
    material: &'a GemMaterial,
    lambdas: &'a [f32; 8],
    seed: u32,
) -> SurfaceQuery<'a> {
    SurfaceQuery {
        scene,
        planes: &[],
        tools: &[],
        material,
        finish: FacetFinish::Polished,
        facet_idx: 0,
        hit_point: Vec3::ZERO,
        normal_out: Vec3::Y,
        k: Vec3::new(0.3, -1.0, 0.1).normalize(),
        inside_gem: false,
        lambdas,
        hero_idx: 0,
        split_radiance: &[0.0; 8],
        rng_seed: seed,
        bounce: 0,
    }
}

const LAMBDAS: [f32; 8] = [450.0, 480.0, 510.0, 540.0, 570.0, 600.0, 630.0, 660.0];

#[test]
fn an_ideal_scene_passes_the_event_through_bit_for_bit() {
    let material = GemMaterial::new_custom("surface", 1.8, 0.02, 0.0, [0.0; 3]);
    let ideal = SurfaceScene::IDEAL;
    assert!(ideal.is_ideal());
    let q = query(&ideal, &material, &LAMBDAS, 17);
    let event = begin_facet_surface(&q);
    assert_eq!(event.finish, FacetFinish::Polished);
    assert_eq!(
        event.normal_out.to_array().map(f32::to_bits),
        Vec3::Y.to_array().map(f32::to_bits)
    );
    let mut stokes = [StokesVector::unpolarized(0.7); 8];
    let mut path_pdf = [0.4f32; 8];
    let mut split = [0.1f32; 8];
    finish_facet_surface(
        &event,
        SurfaceOutcome {
            inside_before: false,
            inside_after: true,
            new_k: Vec3::NEG_Y,
        },
        &mut stokes,
        &mut path_pdf,
        &mut split,
    );
    assert!(stokes.iter().all(|s| s.intensity() == 0.7));
    assert!(path_pdf.iter().all(|&p| p == 0.4) && split.iter().all(|&s| s == 0.1));

    // An all-ideal surface list or a surface list with only ideal words is the same.
    let words = [FacetSurface::IDEAL];
    let listed = SurfaceScene {
        facet_surfaces: &words,
        machine: FacetingMachine::DEFAULT,
        edge: EdgeAbrasion::NONE,
    };
    let event = begin_facet_surface(&query(&listed, &material, &LAMBDAS, 17));
    assert_eq!(event.normal_out, Vec3::Y);
    assert_eq!(event.finish, FacetFinish::Polished);
}

/// The halo branch is taken with the hero's scatter fraction, draws visible micro-normals, and
/// the throughput corrections leave the mean at one up to the masking loss.
#[test]
fn the_polished_halo_is_drawn_with_the_scatter_fraction_and_stays_unbiased() {
    let material = GemMaterial::new_custom("surface", 1.8, 0.02, 0.0, [0.0; 3]);
    let words = [FacetSurface::from_preset(PolishPreset::Prepolish, 0.3)];
    let scene = SurfaceScene {
        facet_surfaces: &words,
        machine: FacetingMachine::DEFAULT,
        edge: EdgeAbrasion::NONE,
    };
    let k = Vec3::new(0.3, -1.0, 0.1).normalize();
    let cos_i = (-k).dot(Vec3::Y);
    let expected = tis_reflection(25.0, 1.0, cos_i, LAMBDAS[0]);
    let seeds = 100_000u32;
    let (mut halo, mut total_scale) = (0u32, 0.0f64);
    let mut mean_hero = 0.0f64;
    for seed in 0..seeds {
        let q = query(&scene, &material, &LAMBDAS, hash_u32(seed));
        let event = begin_facet_surface(&q);
        assert_eq!(event.finish, FacetFinish::Polished);
        let taken = event.normal_out != Vec3::Y;
        if taken {
            halo += 1;
            assert!((event.normal_out.length() - 1.0).abs() < 1e-4);
            assert!((-k).dot(event.normal_out) > -1e-5, "a hidden micro-normal");
        }
        // The polished reflection at the (micro-)normal, then the correction.
        let new_k = reflect_about(-k, event.normal_out) * 1.0;
        let mut stokes = [StokesVector::unpolarized(1.0); 8];
        let mut path_pdf = [1.0f32; 8];
        let mut split = [0.0f32; 8];
        finish_facet_surface(
            &event,
            SurfaceOutcome {
                inside_before: false,
                inside_after: false,
                new_k,
            },
            &mut stokes,
            &mut path_pdf,
            &mut split,
        );
        mean_hero += f64::from(stokes[0].intensity());
        total_scale += f64::from(stokes[7].intensity());
        assert!(path_pdf.iter().all(|&p| p > 0.0 && p < 1.0));
    }
    let fraction = f64::from(halo) / f64::from(seeds);
    let sigma = (f64::from(expected) * (1.0 - f64::from(expected)) / f64::from(seeds)).sqrt();
    assert!(
        (fraction - f64::from(expected)).abs() < 5.0 * sigma,
        "halo fraction {fraction}, expected {expected}"
    );
    let mean_hero = mean_hero / f64::from(seeds);
    let mean_red = total_scale / f64::from(seeds);
    // Unbiased up to the masking and wrong-side losses of the halo (alpha 0.04, ~28 % of events).
    assert!((mean_hero - 1.0).abs() < 0.01, "hero {mean_hero}");
    assert!((mean_red - 1.0).abs() < 0.02, "red {mean_red}");
}

/// A frosted facet and an abraded edge select the ground arm; the band follows the edge.
#[test]
fn ground_facets_and_abraded_edges_take_the_ground_arm() {
    let material = GemMaterial::new_custom("surface", 1.8, 0.02, 0.0, [0.0; 3]);
    let words = [FacetSurface::ground(0.3, 2.0)];
    let scene = SurfaceScene {
        facet_surfaces: &words,
        machine: FacetingMachine::DEFAULT,
        edge: EdgeAbrasion::NONE,
    };
    let event = begin_facet_surface(&query(&scene, &material, &LAMBDAS, 3));
    assert_eq!(event.finish, FacetFinish::Frosted);
    assert!((event.ground.lobe.alpha_mean() - 0.3).abs() < 1e-3);
    assert!(event.ground.lobe.alpha_x > event.ground.lobe.alpha_y);
    assert!(event.ground.across.is_some());

    // A cube: a point 5 mm from the +X face on the top face is in a 10 mm band, one in the middle
    // is not.
    let cube: Vec<GpuFacetPlane> = [
        (Vec3::X, -0.5),
        (Vec3::NEG_X, -0.5),
        (Vec3::Y, -0.5),
        (Vec3::NEG_Y, -0.5),
        (Vec3::Z, -0.5),
        (Vec3::NEG_Z, -0.5),
    ]
    .into_iter()
    .map(|(n, d)| GpuFacetPlane::new(n, d))
    .collect();
    assert!(in_edge_band(&cube, Vec3::new(0.495, 0.5, 0.0), 2, 0.01));
    assert!(!in_edge_band(&cube, Vec3::new(0.0, 0.5, 0.0), 2, 0.01));

    let banded = SurfaceScene {
        facet_surfaces: &[],
        machine: FacetingMachine::DEFAULT,
        edge: EdgeAbrasion {
            width: 0.01,
            alpha: 0.3,
            ..EdgeAbrasion::NONE
        },
    };
    let mut near = query(&banded, &material, &LAMBDAS, 3);
    near.planes = &cube;
    near.facet_idx = 2;
    near.hit_point = Vec3::new(0.495, 0.5, 0.0);
    let event = begin_facet_surface(&near);
    assert_eq!(event.finish, FacetFinish::Frosted);
    assert!((event.ground.lobe.alpha_mean() - 0.3).abs() < 1e-3);
    near.hit_point = Vec3::new(0.0, 0.5, 0.0);
    assert_eq!(begin_facet_surface(&near).finish, FacetFinish::Polished);
}
