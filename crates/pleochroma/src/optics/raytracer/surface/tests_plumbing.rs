//! Tests of the surface plumbing: the merged finish list, the abraded-edge TIS boost, and the
//! Henyey-Greenstein shadow probe's view of the ground band.

use glam::Vec3;

use super::{
    EdgeAbrasion, FacetSurface, FacetingMachine, PolishPreset, SurfaceScene,
    dispatch::{SurfaceQuery, begin_facet_surface},
    edge::{EdgeBand, edge_distance},
    merged_facet_finishes,
    tis::tis_reflection,
};
use crate::{
    geometry::plane::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        polarization::StokesVector,
        raytracer::{
            EnvironmentSource, NUM_CHANNELS, build_plane_soa,
            camera::FacetFinish,
            hash_u32,
            scattering::{
                NeeContext,
                hg_nee::{ShadowAlphas, nee_contribution_hg_scatter},
            },
        },
    },
    renderer::env_map::EnvironmentMap,
};

const LAMBDAS: [f32; NUM_CHANNELS] = [450.0, 480.0, 510.0, 540.0, 570.0, 600.0, 630.0, 660.0];

fn cube(half: f32) -> Vec<GpuFacetPlane> {
    [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ]
    .into_iter()
    .map(|n| GpuFacetPlane::new(n, -half))
    .collect()
}

#[test]
fn ground_words_are_reported_as_frosted_in_the_finish_list() {
    let words = [
        FacetSurface::IDEAL,
        FacetSurface::ground(0.3, 1.0),
        FacetSurface::from_preset(PolishPreset::Fine, 0.3),
        FacetSurface::frosted(),
    ];
    let merged = merged_facet_finishes(&[], &words);
    assert!(matches!(merged, std::borrow::Cow::Owned(_)));
    assert_eq!(
        &*merged,
        &[
            FacetFinish::Polished,
            FacetFinish::Frosted,
            FacetFinish::Polished,
            FacetFinish::Frosted
        ]
    );

    // A list that is longer than the words keeps its own entries.
    let list = [FacetFinish::Frosted; 6];
    let merged = merged_facet_finishes(&list, &words);
    assert_eq!(&*merged, &list);
    assert!(
        matches!(merged, std::borrow::Cow::Borrowed(_)),
        "nothing to add: no allocation"
    );

    // Merging a merged list changes nothing and allocates nothing.
    let once = merged_facet_finishes(&[FacetFinish::Polished], &words).into_owned();
    assert!(matches!(
        merged_facet_finishes(&once, &words),
        std::borrow::Cow::Borrowed(_)
    ));

    // No words at all: the list passes through untouched.
    let plain = [FacetFinish::Frosted, FacetFinish::Polished];
    assert!(matches!(
        merged_facet_finishes(&plain, &[]),
        std::borrow::Cow::Borrowed(p) if p == plain
    ));
}

#[test]
fn the_wear_falls_from_the_boost_to_nothing_over_one_more_width() {
    let edge = EdgeAbrasion {
        width: 0.02,
        alpha: 0.0,
        halo_rq_nm: 20.0,
    };
    assert!(edge.boost_active() && !edge.ground_active() && edge.active());
    assert_eq!(edge.wear_rq_nm(0.0), 20.0);
    assert_eq!(edge.wear_rq_nm(0.02), 20.0);
    assert!((edge.wear_rq_nm(0.03) - 10.0).abs() < 1e-4);
    assert_eq!(edge.wear_rq_nm(0.04), 0.0);
    assert_eq!(edge.wear_rq_nm(5.0), 0.0);
    assert_eq!(EdgeAbrasion::NONE.wear_rq_nm(0.0), 0.0);
    assert!(!EdgeAbrasion::NONE.active());
}

#[test]
fn the_edge_distance_is_the_nearest_other_plane() {
    let planes = cube(0.5);
    assert!((edge_distance(&planes, Vec3::new(0.0, 0.5, 0.0), 2) - 0.5).abs() < 1e-6);
    assert!((edge_distance(&planes, Vec3::new(0.495, 0.5, 0.0), 2) - 0.005).abs() < 1e-6);
    assert_eq!(edge_distance(&planes[..1], Vec3::ZERO, 0), f32::INFINITY);
}

#[test]
fn a_roughness_floor_raises_a_polish_and_keeps_the_rest() {
    let ideal = FacetSurface::IDEAL.with_rq_at_least(12.0);
    assert!(ideal.has_roughness());
    assert!((ideal.rq_nm() - 12.0).abs() < 0.06);
    // Already rougher: unchanged. A zero floor: unchanged. Ground: unchanged.
    let rough = FacetSurface::from_preset(PolishPreset::Prepolish, 0.3);
    assert_eq!(rough.with_rq_at_least(5.0), rough);
    assert_eq!(
        FacetSurface::IDEAL.with_rq_at_least(0.0),
        FacetSurface::IDEAL
    );
    let ground = FacetSurface::ground(0.3, 2.0);
    assert_eq!(ground.with_rq_at_least(50.0), ground);
    // The scratch bits and the anisotropy survive.
    let kept = FacetSurface::from_preset(PolishPreset::Fine, 1.0)
        .with_custom_angle(40.0)
        .with_no_sweep()
        .with_rq_at_least(30.0);
    assert!((kept.rq_nm() - 30.0).abs() < 0.06);
    assert!(kept.no_sweep() && kept.custom_angle_deg().is_some());
    assert!((kept.anisotropy() - 2.0).abs() < 0.07);
}

fn query<'a>(
    scene: &'a SurfaceScene<'a>,
    planes: &'a [GpuFacetPlane],
    material: &'a GemMaterial,
    seed: u32,
    hit_point: Vec3,
) -> SurfaceQuery<'a> {
    SurfaceQuery {
        scene,
        planes,
        tools: &[],
        material,
        finish: FacetFinish::Polished,
        facet_idx: 2,
        hit_point,
        normal_out: Vec3::Y,
        k: Vec3::new(0.3, -1.0, 0.1).normalize(),
        inside_gem: false,
        lambdas: &LAMBDAS,
        hero_idx: 0,
        split_radiance: &[0.0; 8],
        rng_seed: seed,
        bounce: 0,
    }
}

/// An ideal facet next to an abraded edge scatters a halo of the worn roughness: the halo
/// fraction at the rim is the TIS of the boost, it is smaller farther out, and the middle of the
/// facet is untouched bit for bit.
#[test]
fn the_tis_boost_dulls_the_polish_next_to_the_band() {
    let material = GemMaterial::new_custom("surface", 1.8, 0.02, 0.0, [0.0; 3]);
    let planes = cube(0.5);
    let scene = SurfaceScene {
        facet_surfaces: &[],
        machine: FacetingMachine::DEFAULT,
        edge: EdgeAbrasion {
            width: 0.01,
            alpha: 0.0,
            halo_rq_nm: 25.0,
        },
    };
    assert!(!scene.is_ideal());
    let k = Vec3::new(0.3, -1.0, 0.1).normalize();
    let cos_i = (-k).dot(Vec3::Y);
    let seeds = 40_000u32;
    let fraction_at = |hit_point: Vec3| {
        let halo = (0..seeds)
            .filter(|&seed| {
                let q = query(&scene, &planes, &material, hash_u32(seed), hit_point);
                let event = begin_facet_surface(&q);
                assert_eq!(event.finish, FacetFinish::Polished);
                event.normal_out != Vec3::Y
            })
            .count();
        halo as f64 / f64::from(seeds)
    };

    // 5 mm from the +X face: the full boost.
    let rim = fraction_at(Vec3::new(0.495, 0.5, 0.0));
    let expected = f64::from(tis_reflection(25.0, 1.0, cos_i, LAMBDAS[0]));
    let sigma = (expected * (1.0 - expected) / f64::from(seeds)).sqrt();
    assert!(
        (rim - expected).abs() < 5.0 * sigma,
        "rim halo fraction {rim}, expected {expected}"
    );
    // 15 mm: half the boost, so less scatter.
    let outer = fraction_at(Vec3::new(0.485, 0.5, 0.0));
    let expected_outer = f64::from(tis_reflection(12.5, 1.0, cos_i, LAMBDAS[0]));
    let sigma_outer = (expected_outer * (1.0 - expected_outer) / f64::from(seeds)).sqrt();
    assert!(
        (outer - expected_outer).abs() < 5.0 * sigma_outer,
        "outer halo fraction {outer}, expected {expected_outer}"
    );
    assert!(outer < rim);
    // The middle of the facet takes no halo draw at all.
    let q = query(&scene, &planes, &material, 7, Vec3::new(0.0, 0.5, 0.0));
    let event = begin_facet_surface(&q);
    assert_eq!(event.finish, FacetFinish::Polished);
    assert_eq!(event.normal_out, Vec3::Y);
}

/// With a ground band the boost does not apply inside it (the band is ground), and the band
/// alone (no boost) is unchanged by the new field.
#[test]
fn the_ground_band_still_wins_inside_the_band() {
    let material = GemMaterial::new_custom("surface", 1.8, 0.02, 0.0, [0.0; 3]);
    let planes = cube(0.5);
    let scene = SurfaceScene {
        facet_surfaces: &[],
        machine: FacetingMachine::DEFAULT,
        edge: EdgeAbrasion {
            width: 0.01,
            alpha: 0.3,
            halo_rq_nm: 25.0,
        },
    };
    let q = query(&scene, &planes, &material, 3, Vec3::new(0.495, 0.5, 0.0));
    assert_eq!(begin_facet_surface(&q).finish, FacetFinish::Frosted);
    // Just outside the band the boost applies: still the polished dispatch.
    let q = query(&scene, &planes, &material, 3, Vec3::new(0.485, 0.5, 0.0));
    assert_eq!(begin_facet_surface(&q).finish, FacetFinish::Polished);
}

/// A shadow ray of the scattering-point estimator that leaves through the ground band deposits
/// nothing (the bounce loop treats that exit as a ground bounce); a band too thin to catch any
/// exit leaves every deposit as it was.
#[test]
fn the_hg_shadow_probe_drops_exits_through_the_ground_band() {
    let half = 0.25;
    let planes = cube(half);
    let soa = build_plane_soa(&planes);
    let env_map = EnvironmentMap::uniform(4, 2, [2.0, 2.0, 2.0]);
    let material =
        GemMaterial::new_custom("band", 1.5, 0.0, 0.0, [0.0; 3]).with_scattering(1.0, 0.2);
    let stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
    let alphas = [0.0f32; NUM_CHANNELS];
    let k_in = Vec3::new(0.05, -1.0, 0.02).normalize();
    let deposit = |edge_band: Option<EdgeBand<'_>>, seed: u32| {
        let nee = NeeContext {
            environment: EnvironmentSource::HdrMap(&env_map),
            plane_soa: &soa,
            tools: &[],
            enabled: true,
            edge_band,
        };
        let mut out = [0.0f32; NUM_CHANNELS];
        nee_contribution_hg_scatter(
            nee,
            &LAMBDAS,
            1.5,
            Vec3::ZERO,
            k_in,
            material.scattering_g,
            seed,
            1,
            &stokes,
            &mut out,
            ShadowAlphas::Fixed(&alphas),
            material.scattering_sigma_s,
            material.absorption_path_scale,
            &[],
        );
        out
    };
    let everywhere = EdgeBand::of(
        &EdgeAbrasion {
            width: 10.0,
            alpha: 0.3,
            halo_rq_nm: 0.0,
        },
        &planes,
        false,
    );
    let hairline = EdgeBand::of(
        &EdgeAbrasion {
            width: 1e-7,
            alpha: 0.3,
            halo_rq_nm: 0.0,
        },
        &planes,
        false,
    );
    assert!(everywhere.is_some() && hairline.is_some());
    // Tools switch the band off; so does a band with no ground.
    assert!(EdgeBand::of(&EdgeAbrasion::NONE, &planes, false).is_none());
    assert!(
        EdgeBand::of(
            &EdgeAbrasion {
                width: 1.0,
                alpha: 0.3,
                halo_rq_nm: 0.0
            },
            &planes,
            true
        )
        .is_none()
    );

    let mut deposited = 0;
    for seed in 0..32u32 {
        let seed = 91_000 + seed;
        let open = deposit(None, seed);
        if open.iter().any(|&v| v > 0.0) {
            deposited += 1;
        }
        assert!(
            deposit(everywhere, seed).iter().all(|&v| v == 0.0),
            "seed {seed}: an exit inside the band deposits nothing"
        );
        let thin = deposit(hairline, seed);
        assert_eq!(
            thin.map(f32::to_bits),
            open.map(f32::to_bits),
            "seed {seed}: a hairline band changes nothing"
        );
    }
    assert!(deposited >= 8, "premise: {deposited} of 32 draws deposit");
}
