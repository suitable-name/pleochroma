//! Tests of `prepare_bounce` and `expand_at`, built only with `--cfg beam_tests`. The tests of the
//! shared `expand`, `expand_reporting` and `split_modes` are in `tests_bounce`.

use glam::{DVec3, Vec3};

use super::{
    bounce::{
        Child, EventKind, ExplicitHit, PathState, SegmentDepths, ZONE_SLOTS, expand, expand_at,
        prepare_bounce,
    },
    build::HeroContext,
    record::{COMB_LEN, COMB_NM, REFERENCE_SLOT},
    tests_support::{Rig, glass, uniaxial_glass},
};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    materials::GemMaterial,
    polarization::StokesVector,
    raytracer::{
        Ray, camera::HitRecord, intersect_stone::intersect_stone_soa_side,
        transport::PreparedMaterial,
    },
    zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
};

/// Model-to-millimetre scale of the zoned test material.
const PATH_SCALE: f32 = 3.0;

/// The hero context of the reference comb slot.
fn hero_of<'a>(prepared: &PreparedMaterial<'a>) -> HeroContext<'a> {
    let (ctx, cache) = prepared.ray_context(COMB_NM, REFERENCE_SLOT);
    HeroContext { ctx, cache }
}

/// A camera ray from above that enters the unit cube through its top face.
fn outside_ray() -> Ray {
    Ray {
        origin: Vec3::new(0.2, 3.0, 0.1),
        dir: Vec3::new(-0.05, -1.0, 0.02).normalize(),
    }
}

/// A ray inside the unit cube that leaves through its +X face.
fn inside_ray() -> Ray {
    Ray {
        origin: Vec3::new(-0.5, 0.1, 0.1),
        dir: Vec3::new(1.0, 0.1, 0.05).normalize(),
    }
}

/// An unpolarized unit-intensity state travelling along `ray`.
fn state_of(ray: Ray, inside: bool) -> PathState {
    PathState {
        k: ray.dir,
        inside,
        is_ext: false,
        stokes: [StokesVector::unpolarized(1.0); COMB_LEN],
    }
}

/// The intersector's hit of `ray` and its verdict on entering or leaving.
fn hit_for(rig: &Rig, ray: Ray) -> (HitRecord, Option<bool>) {
    intersect_stone_soa_side(ray, &rig.soa, rig.planes.len(), &[]).expect("the ray meets the cube")
}

fn close(a: f32, b: f32, tolerance: f32) -> bool {
    (a - b).abs() <= tolerance
}

fn stokes_close(a: &StokesVector, b: &StokesVector, tolerance: f32) -> bool {
    close(a.i, b.i, tolerance)
        && close(a.q, b.q, tolerance)
        && close(a.u, b.u, tolerance)
        && close(a.v, b.v, tolerance)
}

fn assert_children_close(a: &[Child], b: &[Child], label: &str) {
    assert_eq!(a.len(), b.len(), "{label}: child count");
    for (x, y) in a.iter().zip(b) {
        assert_eq!(x.kind, y.kind, "{label}");
        assert_eq!(x.state.inside, y.state.inside, "{label}");
        assert_eq!(x.state.is_ext, y.state.is_ext, "{label}");
        assert_eq!(x.diverged, y.diverged, "{label}");
        assert!((x.state.k - y.state.k).length() < 1.0e-5, "{label}: k");
        assert!((x.s - y.s).length() < 1.0e-5, "{label}: s");
        for (p, q) in x.state.stokes.iter().zip(&y.state.stokes) {
            assert!(stokes_close(p, q, 1.0e-5), "{label}: {p:?} vs {q:?}");
        }
    }
}

/// `expand_at` on the hit the intersector found equals `prepare_bounce` + `expand`.
#[test]
fn expand_at_equals_expand_on_a_hit_the_intersector_agrees_with() {
    for (name, material) in [("isotropic", glass()), ("uniaxial", uniaxial_glass())] {
        let rig = Rig::cube_from_above(material);
        let prepared = PreparedMaterial::new(&rig.material);
        let hero = hero_of(&prepared);
        for (where_, ray, inside) in [
            ("entry", outside_ray(), false),
            ("internal", inside_ray(), true),
        ] {
            let label = format!("{name} {where_}");
            let (hit, side) = hit_for(&rig, ray);
            let state = state_of(ray, inside);
            let (input, depths) = prepare_bounce(&hero, &rig.planes, state, ray, &hit, side, None);
            let reference = expand(&input);
            let explicit = ExplicitHit {
                point: ray.origin + hit.t * ray.dir,
                facet: hit.facet_idx,
                normal: hit.normal,
            };
            let at = expand_at(&hero, &rig.planes, state, ray, explicit, side, None);
            assert_children_close(&at.children, &reference, &label);
            assert_eq!(at.plane_normal, input.plane_normal, "{label}");
            assert_eq!(at.dropped, 0.0, "{label}: nothing is below the floor here");
            for (p, q) in at.depths.lengths_mm.iter().zip(&depths.lengths_mm) {
                assert!(close(*p, *q, 1.0e-5), "{label}: lengths");
            }
        }
    }
}

/// A material that absorbs in the comb's bands, with the zones of `zoning` when given.
fn absorbing(zoning: Option<ZonedAbsorption>) -> GemMaterial {
    let base = GemMaterial::new_custom("depths probe", 1.6, 0.0, 0.0, [1.0, 2.0, 4.0]);
    match zoning {
        Some(zoning) => base
            .with_zoning(zoning)
            .with_absorption_path_scale(PATH_SCALE),
        None => base,
    }
}

fn band_zone(center_nm: f32, width_nm: f32, peak_per_mm: f32) -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
        center_nm,
        width_nm,
        peak_per_mm,
    )]))
}

/// A base zone plus one coloured half-space zone across the cube's mid-plane.
fn half_space_zoning() -> ZonedAbsorption {
    let mut zoning = ZonedAbsorption::new(band_zone(520.0, 60.0, 0.4));
    zoning.zones.push(Zone {
        shape: ZoneShape::HalfSpace {
            normal: DVec3::X,
            offset: 0.0,
        },
        absorption: band_zone(620.0, 50.0, 0.9),
    });
    zoning
}

/// The segment depths `prepare_bounce` returns reproduce the 8-channel absorption it applied.
#[test]
fn segment_depths_reproduce_the_applied_absorption() {
    for (name, material, zones_used) in [
        ("homogeneous", absorbing(None), 1),
        ("zoned", absorbing(Some(half_space_zoning())), 2),
    ] {
        let rig = Rig::cube_from_above(material);
        let prepared = PreparedMaterial::new(&rig.material);
        let hero = hero_of(&prepared);
        let ray = inside_ray();
        let (hit, side) = hit_for(&rig, ray);
        let (input, depths) = prepare_bounce(
            &hero,
            &rig.planes,
            state_of(ray, true),
            ray,
            &hit,
            side,
            None,
        );
        let used = depths.lengths_mm.iter().filter(|&&l| l > 0.0).count();
        assert_eq!(used, zones_used, "{name}: zones the segment crosses");
        let scale = rig.material.absorption_path_scale;
        let total: f32 = depths.lengths_mm.iter().sum();
        assert!(
            close(total, hit.t * scale, 1.0e-4 * hit.t * scale),
            "{name}"
        );

        let transmittance = depths.transmittance();
        let mut darkest = 1.0_f32;
        for (k, &trans) in transmittance.iter().enumerate() {
            // The tracer's exponential is a few-ULP polynomial, not `f32::exp`.
            assert!(
                close(input.state.stokes[k].i, trans, 1.0e-5),
                "{name}: channel {k}: applied {} vs depths {trans}",
                input.state.stokes[k].i
            );
            darkest = darkest.min(trans);
        }
        assert!(darkest < 0.999, "{name}: the probe must absorb");

        let doubled = depths.alphas.map(|zone| zone.map(|a| 2.0 * a));
        let twice = depths.optical_depths_with(&doubled);
        for (once, double) in depths.optical_depths().iter().zip(&twice) {
            assert!(close(2.0 * once, *double, 1.0e-4 * once.max(1.0)), "{name}");
        }
    }
}

/// An exterior segment carries no depth data.
#[test]
fn an_entry_hit_has_no_segment_depths() {
    let rig = Rig::cube_from_above(absorbing(None));
    let prepared = PreparedMaterial::new(&rig.material);
    let hero = hero_of(&prepared);
    let ray = outside_ray();
    let (hit, side) = hit_for(&rig, ray);
    let (input, depths) = prepare_bounce(
        &hero,
        &rig.planes,
        state_of(ray, false),
        ray,
        &hit,
        side,
        None,
    );
    assert_eq!(depths, SegmentDepths::NONE);
    assert_eq!(ZONE_SLOTS, depths.lengths_mm.len());
    assert!(!input.state.inside);
    assert_eq!(input.state.stokes, state_of(ray, false).stokes);
}

/// Pin (guide section 10.1): the ordinary and extraordinary children of one internal reflection
/// share the Poynting direction `s`.
#[test]
fn the_o_and_e_children_of_one_internal_reflection_share_s() {
    let rig = Rig::cube_from_above(uniaxial_glass());
    let prepared = PreparedMaterial::new(&rig.material);
    let hero = hero_of(&prepared);
    let ray = inside_ray();
    let (hit, side) = hit_for(&rig, ray);
    let explicit = ExplicitHit {
        point: ray.origin + hit.t * ray.dir,
        facet: hit.facet_idx,
        normal: hit.normal,
    };
    let expansion = expand_at(
        &hero,
        &rig.planes,
        state_of(ray, true),
        ray,
        explicit,
        side,
        None,
    );
    let reflected: Vec<&Child> = expansion
        .children
        .iter()
        .filter(|c| c.kind != EventKind::Transmit)
        .collect();
    assert_eq!(
        reflected.len(),
        2,
        "one ordinary and one extraordinary child"
    );
    assert!(!reflected[0].state.is_ext);
    assert!(reflected[1].state.is_ext);
    assert_eq!(reflected[0].s, reflected[1].s);
    assert_eq!(reflected[0].state.k, reflected[1].state.k);
    assert_eq!(expansion.children.len(), 3, "plus the transmitted child");
}
