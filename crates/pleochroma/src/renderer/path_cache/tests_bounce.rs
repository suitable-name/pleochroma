//! Tests of the bounce expansion: `expand_reporting` against `expand`, `expand`'s optics-table
//! Fresnel, and `split_modes`' probability floor.

use glam::Vec3;

use super::{
    bounce::{
        BounceInput, Child, EventKind, MIN_PROBABILITY, PathState, expand, expand_reporting,
        prepare_bounce, split_modes,
    },
    build::HeroContext,
    record::{COMB_LEN, COMB_NM, REFERENCE_SLOT},
    tests_support::{Rig, glass, uniaxial_glass},
};
use crate::optics::{
    polarization::{MuellerMatrix, StokesVector},
    raytracer::{
        Ray, camera::HitRecord, intersect_stone::intersect_stone_soa_side,
        transport::PreparedMaterial,
    },
    tables::{FRESNEL_REL_TOLERANCE, OpticsTables, fresnel_amplitudes, reflection_amplitudes},
};

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

/// The reporting variant expands the same children as `expand`.
#[test]
fn expand_reporting_returns_the_children_of_expand() {
    let rig = Rig::cube_from_above(uniaxial_glass());
    let prepared = PreparedMaterial::new(&rig.material);
    let hero = hero_of(&prepared);
    let ray = inside_ray();
    let (hit, side) = hit_for(&rig, ray);
    let (input, _) = prepare_bounce(
        &hero,
        &rig.planes,
        state_of(ray, true),
        ray,
        &hit,
        side,
        None,
    );
    let (children, dropped) = expand_reporting(&input);
    assert_children_close(&children, &expand(&input), "reporting");
    assert_eq!(dropped, 0.0);
}

/// The channel-`k` reflection the tracer's table-aware helper gives for the bounce in `input`.
fn tracer_reflection(
    hero: &HeroContext<'_>,
    input: &BounceInput<'_, '_>,
    k: usize,
) -> StokesVector {
    let geo = &input.geo;
    let cos_t = (1.0 - geo.sin2_t_ch[k]).max(0.0).sqrt();
    let (r_s, r_p) = reflection_amplitudes(
        hero.cache.tables.as_deref(),
        geo.n1_ch[k],
        geo.n2_ch[k],
        geo.cos_i,
        cos_t,
    );
    input.state.stokes[k].apply_matrix(&MuellerMatrix::fresnel_reflection(r_s, r_p))
}

/// The channel-`k` transmission the tracer's table-aware helper gives for the bounce in `input`.
fn tracer_transmission(
    hero: &HeroContext<'_>,
    input: &BounceInput<'_, '_>,
    k: usize,
) -> StokesVector {
    let geo = &input.geo;
    let (n1, n2) = (geo.n1_ch[k], geo.n2_ch[k]);
    let sin2_t = (n1 / n2).powi(2) * geo.cos_i.mul_add(-geo.cos_i, 1.0);
    let cos_t = (1.0 - sin2_t).max(0.0).sqrt();
    let amplitudes = fresnel_amplitudes(hero.cache.tables.as_deref(), n1, n2, geo.cos_i, cos_t);
    let matrix = MuellerMatrix::fresnel_transmission(
        n1,
        n2,
        geo.cos_i,
        cos_t,
        amplitudes.t_s,
        amplitudes.t_p,
    );
    input.state.stokes[k].apply_matrix(&matrix).scale(1.0)
}

/// With tables on `expand`'s per-channel Fresnel is the tracer's helper's, within the table
/// tolerance of the closed form; with tables off it is the closed form, bit for bit.
#[test]
fn expand_follows_the_optics_tables_like_the_tracer() {
    let rig = Rig::cube_from_above(glass());
    let ray = outside_ray();
    let (hit, side) = hit_for(&rig, ray);
    let state = state_of(ray, false);
    let off = PreparedMaterial::new(&rig.material).with_optics_tables(OpticsTables::Off);
    let on = PreparedMaterial::new(&rig.material).with_optics_tables(OpticsTables::On);
    let (hero_off, hero_on) = (hero_of(&off), hero_of(&on));
    assert!(hero_off.cache.tables.is_none());
    assert!(hero_on.cache.tables.is_some());

    let (input_off, _) = prepare_bounce(&hero_off, &rig.planes, state, ray, &hit, side, None);
    let (input_on, _) = prepare_bounce(&hero_on, &rig.planes, state, ray, &hit, side, None);
    let (children_off, children_on) = (expand(&input_off), expand(&input_on));
    assert_eq!(children_off.len(), 2);
    assert_eq!(children_on.len(), 2);
    assert_eq!(children_on[0].kind, EventKind::Reflect);
    assert_eq!(children_on[1].kind, EventKind::Transmit);

    let tolerance = 8.0 * FRESNEL_REL_TOLERANCE;
    for k in 0..COMB_LEN {
        // Tables on: exactly the tracer's helper.
        assert_eq!(
            children_on[0].state.stokes[k],
            tracer_reflection(&hero_on, &input_on, k),
            "reflection, channel {k}"
        );
        assert_eq!(
            children_on[1].state.stokes[k],
            tracer_transmission(&hero_on, &input_on, k),
            "transmission, channel {k}"
        );
        // Tables off: the closed form `expand` computes inline.
        let geo = &input_off.geo;
        let (n1, n2) = (geo.n1_ch[k], geo.n2_ch[k]);
        let cos_t = (1.0 - geo.sin2_t_ch[k]).max(0.0).sqrt();
        let r_s =
            f32::mul_add(n2, -cos_t, n1 * geo.cos_i) / f32::mul_add(n2, cos_t, n1 * geo.cos_i);
        let r_p =
            f32::mul_add(n1, -cos_t, n2 * geo.cos_i) / f32::mul_add(n1, cos_t, n2 * geo.cos_i);
        let inline =
            input_off.state.stokes[k].apply_matrix(&MuellerMatrix::fresnel_reflection(r_s, r_p));
        assert_eq!(
            children_off[0].state.stokes[k], inline,
            "closed form, channel {k}"
        );
        assert_eq!(
            children_off[1].state.stokes[k],
            tracer_transmission(&hero_off, &input_off, k),
            "closed-form transmission, channel {k}"
        );
        // And the two agree to the table tolerance.
        for child in 0..2 {
            let (a, b) = (
                &children_on[child].state.stokes[k],
                &children_off[child].state.stokes[k],
            );
            assert!(
                stokes_close(a, b, tolerance),
                "child {child}, channel {k}: {a:?} vs {b:?}"
            );
        }
    }
}

/// A mode below the probability floor is not a child; its energy is reported.
#[test]
fn a_near_zero_mode_is_dropped_and_its_energy_reported() {
    let ray = inside_ray();
    let base = Child {
        state: state_of(ray, true),
        s: ray.dir,
        kind: EventKind::Reflect,
        diverged: false,
    };
    let tiny = 1.0e-7_f32;
    assert!(tiny < MIN_PROBABILITY);

    let mut dropped = 0.0;
    let children = split_modes(base, tiny, &mut dropped);
    assert_eq!(children.len(), 1, "only the extraordinary mode survives");
    assert!(children[0].state.is_ext);
    assert!(
        close(dropped, tiny, 1.0e-12),
        "the ordinary share is reported: {dropped}"
    );

    // Both modes expanded: nothing dropped.
    let mut dropped = 0.0;
    assert_eq!(split_modes(base, 0.5, &mut dropped).len(), 2);
    assert_eq!(dropped, 0.0);

    // A mode of probability zero carries no energy to report.
    let mut dropped = 0.0;
    assert_eq!(split_modes(base, 0.0, &mut dropped).len(), 1);
    assert_eq!(dropped, 0.0);
}
