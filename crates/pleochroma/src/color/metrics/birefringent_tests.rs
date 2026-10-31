//! Tests of the two-mode (doubling) metric tracer: the isotropic fast path stays exactly what it
//! was, the ordinary mode is the plain trace, the extraordinary mode's walk-off follows the
//! analytic plate geometry, and the Doubling figure behaves like the optics says it must.

use std::f32::consts::FRAC_PI_2;

use glam::Vec3;

use super::{
    birefringent::{Line, Mode, ModeCtx, is_anisotropic, trace_mode},
    camera::camera_view_basis,
    classify::{ApertureSampleContext, RayClassification, classify_aperture_sample},
    evaluate::{MetricsWanted, evaluate_pose},
    fan::GRID_DISC_RADIUS_SQ,
    prepared::PreparedEval,
    ray_trace::{EntryRay, RayFate, trace_entry},
};
use crate::{
    geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane, tool::StoneGeometry},
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, LightingPreset, Ray},
    },
};

const fn studio() -> EnvironmentSource<'static> {
    LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95)
}

fn sapphire_with_axis(c_axis: Vec3) -> GemMaterial {
    let mut material = GemMaterial::sapphire();
    material.c_axis = c_axis;
    material
}

/// A 1.0-thick plane-parallel plate (top at `y = 0.5`) wide enough that no ray reaches its sides.
fn plate() -> Vec<GpuFacetPlane> {
    [
        (Vec3::Y, -0.5),
        (Vec3::NEG_Y, -0.5),
        (Vec3::X, -5.0),
        (Vec3::NEG_X, -5.0),
        (Vec3::Z, -5.0),
        (Vec3::NEG_Z, -5.0),
    ]
    .iter()
    .map(|&(normal, d)| GpuFacetPlane::new(normal, d))
    .collect()
}

/// The entry of the camera ray `origin + t * dir` into the stone, `None` when it misses.
fn entry_of(prepared: &PreparedEval<'_>, origin: Vec3, dir: Vec3) -> Option<EntryRay> {
    let ray = Ray { origin, dir };
    let hit = prepared.stone().intersect(ray)?;
    Some(EntryRay {
        point: origin + hit.t * dir,
        dir,
        normal: hit.normal,
        cos_i: (-dir).dot(hit.normal).clamp(0.0, 1.0),
        facet: hit.facet_idx,
    })
}

/// Everything about a fate that must agree bit for bit between two equal traces.
fn fate_bits(fate: &RayFate) -> (u8, [u32; 6]) {
    match fate {
        RayFate::EntryBlocked => (0, [0; 6]),
        RayFate::Leaked => (1, [0; 6]),
        RayFate::Absorbed => (3, [0; 6]),
        RayFate::ExitedUpward(path) => (
            2,
            [
                path.dir.x.to_bits(),
                path.dir.y.to_bits(),
                path.dir.z.to_bits(),
                path.transmittance.to_bits(),
                path.path_len.to_bits(),
                u32::try_from(path.facet_idx).unwrap_or(u32::MAX) ^ (path.bounces << 24),
            ],
        ),
    }
}

/// Brilliance / windowing / extinction (percent) the way the integer path counted them before the
/// two-mode tracer existed: a plain loop over `classify_aperture_sample` and integer counters.
fn integer_reference(prepared: &PreparedEval<'_>, yaw: f32, pitch: f32) -> [f32; 3] {
    let (cam_forward, cam_right, cam_up) = camera_view_basis(yaw, pitch);
    let ctx = ApertureSampleContext {
        stone: prepared.stone(),
        nd: prepared.nd,
        n_f: prepared.n_f,
        n_c: prepared.n_c,
        cam_forward,
        cam_right,
        cam_up,
        fan: prepared.fan,
        lighting: &prepared.lighting,
        want_fire: false,
    };
    let samples = [
        (0.0f32, 0.0f32),
        (0.08, 0.0),
        (-0.08, 0.0),
        (0.0, 0.08),
        (0.0, -0.08),
    ];
    let (mut total, mut windowed, mut extinct, mut returned) = (0u32, 0u32, 0u32, 0u32);
    for ix in 0..18 {
        for iz in 0..18 {
            let u = ((ix as f32 + 0.5) / 18.0).mul_add(2.0, -1.0);
            let v = ((iz as f32 + 0.5) / 18.0).mul_add(2.0, -1.0);
            if v.mul_add(v, u * u) > GRID_DISC_RADIUS_SQ {
                continue;
            }
            for &(dx, dz) in &samples {
                let Some(class) = classify_aperture_sample(&ctx, u, v, dx, dz) else {
                    continue;
                };
                total += 1;
                match class {
                    RayClassification::Windowed => windowed += 1,
                    RayClassification::Extinct => extinct += 1,
                    RayClassification::Returned(_) => returned += 1,
                    RayClassification::EntryBlocked | RayClassification::Invalid => {}
                }
            }
        }
    }
    let n = total.max(1) as f32;
    [
        (returned as f32 / n * 100.0).clamp(0.0, 100.0),
        (windowed as f32 / n * 100.0).clamp(0.0, 100.0),
        (extinct as f32 / n * 100.0).clamp(0.0, 100.0),
    ]
}

/// The renderer's predicate: not cubic and `|delta n| > 1e-4`.
#[test]
fn the_anisotropy_predicate_is_the_renderers() {
    assert!(!is_anisotropic(&GemMaterial::diamond()));
    assert!(is_anisotropic(&GemMaterial::sapphire()));
    assert!(is_anisotropic(
        &GemMaterial::by_name("Tourmaline").expect("Tourmaline resolves")
    ));
    let mut weak = GemMaterial::sapphire();
    weak.birefringence_delta = 5e-5;
    assert!(!is_anisotropic(&weak));
    let mut cubic = GemMaterial::diamond();
    cubic.birefringence_delta = 0.01;
    assert!(!is_anisotropic(&cubic));
}

/// An isotropic stone takes the integer path: no modes on the arena, the counters equal an
/// independent integer loop bit for bit, and the Doubling figure is exactly zero.
#[test]
fn the_isotropic_path_is_the_integer_path() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let prepared =
        PreparedEval::new(StoneGeometry::planes_only(&planes), &material, studio()).unwrap();
    assert!(prepared.stone().modes.is_none());
    for (yaw, pitch) in [(0.0, FRAC_PI_2), (0.6, 0.45)] {
        let metrics = evaluate_pose(&prepared, yaw, pitch, MetricsWanted::ALL, None);
        let [brilliance, windowing, extinction] = integer_reference(&prepared, yaw, pitch);
        assert_eq!(metrics.brilliance_pct.to_bits(), brilliance.to_bits());
        assert_eq!(metrics.windowing_pct.to_bits(), windowing.to_bits());
        assert_eq!(metrics.extinction_pct.to_bits(), extinction.to_bits());
        assert_eq!(metrics.doubling_arcmin.to_bits(), 0.0f32.to_bits());
    }
}

/// The ordinary mode is the plain isotropic trace at `n_o`, bit for bit: over a grid of camera
/// rays through the emerald cut, sapphire's ordinary fate equals `trace_entry` at the d-line.
#[test]
fn the_ordinary_mode_is_the_plain_trace() {
    let planes = StandardGemCuts::emerald_cut();
    let material = sapphire_with_axis(Vec3::new(0.3, 0.8, 0.2).normalize());
    let prepared =
        PreparedEval::new(StoneGeometry::planes_only(&planes), &material, studio()).unwrap();
    let stone = prepared.stone();
    assert!(stone.modes.is_some());
    let camera = Camera::new(0.4, 0.9, 2.4, 42.0);
    let mut compared = 0;
    for gy in 0..32 {
        for gx in 0..32 {
            let ray = camera.generate_ray(gx as f32, gy as f32, 32.0, 32.0, 0.0, 0.0);
            let Some(entry) = entry_of(&prepared, ray.origin, ray.dir) else {
                continue;
            };
            let plain = trace_entry(entry, stone, prepared.nd, None);
            let ordinary = trace_mode(entry, stone, Line::D, Mode::Ordinary, prepared.nd, None);
            assert_eq!(fate_bits(&plain), fate_bits(&ordinary), "pixel {gx},{gy}");
            compared += 1;
        }
    }
    assert!(compared > 100, "{compared} rays compared");
}

/// Along the optic axis the two modes are one: `c` parallel to `k` gives the same refracted
/// wave normal, index, transmittance and travel direction, and a plate traced along `c` returns
/// the same fate for both modes.
#[test]
fn c_parallel_to_k_gives_two_identical_fates() {
    let planes = plate();
    let material = sapphire_with_axis(Vec3::Y);
    let prepared =
        PreparedEval::new(StoneGeometry::planes_only(&planes), &material, studio()).unwrap();
    let stone = prepared.stone();
    let modes = stone.modes.expect("sapphire is anisotropic");
    let entry = entry_of(&prepared, Vec3::new(0.3, 3.0, 0.1), Vec3::NEG_Y).expect("hits the plate");
    let ordinary = ModeCtx::new(modes, Line::D, Mode::Ordinary)
        .enter(entry)
        .unwrap();
    let extra_ctx = ModeCtx::new(modes, Line::D, Mode::Extraordinary);
    let extraordinary = extra_ctx.enter(entry).unwrap();
    assert_eq!(ordinary.k.to_array(), extraordinary.k.to_array());
    assert_eq!(ordinary.index.to_bits(), extraordinary.index.to_bits());
    assert_eq!(
        ordinary.transmittance.to_bits(),
        extraordinary.transmittance.to_bits()
    );
    assert_eq!(
        extra_ctx.ray_dir(extraordinary.k).to_array(),
        extraordinary.k.to_array()
    );
    let fate_o = trace_mode(entry, stone, Line::D, Mode::Ordinary, prepared.nd, None);
    let fate_e = trace_mode(
        entry,
        stone,
        Line::D,
        Mode::Extraordinary,
        prepared.nd,
        None,
    );
    assert!(matches!(fate_o, RayFate::Leaked));
    assert!(matches!(fate_e, RayFate::Leaked));
}

/// A tourmaline plate with `c` in the plate (along `+X`) and light incident at 30 degrees in the
/// plane of `c`: the extraordinary mode refracts with `n_e(theta)` (Snell with the effective
/// index), walks off the wave normal by the analytic `tan(rho) = (n_o^2 - n_e^2) sin(theta)
/// cos(theta) / (n_e^2 cos^2(theta) + n_o^2 sin^2(theta))`, away from `c`, and lands on the
/// bottom face `t * (tan(phi) - tan(phi - rho))` from where the wave normal would: all to 1e-4.
#[test]
fn the_extraordinary_walk_off_matches_the_analytic_plate() {
    let planes = plate();
    let mut material = GemMaterial::by_name("Tourmaline").expect("Tourmaline resolves");
    material.c_axis = Vec3::X;
    let prepared =
        PreparedEval::new(StoneGeometry::planes_only(&planes), &material, studio()).unwrap();
    let stone = prepared.stone();
    let modes = stone.modes.expect("tourmaline is anisotropic");
    let incidence = 30.0f32.to_radians();
    let dir = Vec3::new(incidence.sin(), -incidence.cos(), 0.0);
    let entry = entry_of(&prepared, Vec3::new(-2.0, 3.0, 0.0), dir).expect("hits the plate");

    let n_o = f64::from(material.dispersion.evaluate(589.3).max(1.1));
    let n_e = f64::from(
        material
            .extraordinary_index_at(589.3, material.dispersion.evaluate(589.3))
            .max(1.001),
    );
    let ctx = ModeCtx::new(modes, Line::D, Mode::Extraordinary);
    let start = ctx.enter(entry).expect("refracts");

    // The refracted wave normal lies in the plane of incidence and `c`: k = (sin phi, -cos phi, 0).
    let phi = f64::from(start.k.x).atan2(f64::from(-start.k.y));
    let sin_theta = phi.cos();
    let cos_theta = phi.sin();
    let denom = (n_e * cos_theta).mul_add(n_e * cos_theta, (n_o * sin_theta).powi(2));
    let index = n_o * n_e / denom.sqrt();
    assert!((f64::from(start.index) - index).abs() < 1e-4, "n_eff");
    // Snell with the effective index.
    assert!(
        index.mul_add(-phi.sin(), f64::from(incidence.sin())).abs() < 1e-4,
        "Snell"
    );

    let tan_rho = n_e.mul_add(-n_e, n_o * n_o) * sin_theta * cos_theta / denom;
    let rho = tan_rho.atan();
    let travel = ctx.ray_dir(start.k);
    let phi_s = f64::from(travel.x).atan2(f64::from(-travel.y));
    assert!(
        (phi_s - (phi - rho)).abs() < 1e-4,
        "S angle {phi_s} vs {}",
        phi - rho
    );

    // Where the energy lands on the bottom face (the plate is 1.0 thick).
    let from = entry.point + travel * 1e-4;
    let bottom = stone
        .intersect(Ray {
            origin: from,
            dir: travel,
        })
        .expect("reaches the bottom");
    let landed = f64::from(bottom.t.mul_add(travel.x, from.x) - entry.point.x);
    assert!((landed - (phi - rho).tan()).abs() < 1e-4, "landing");
    let separation = phi.tan() - landed;
    assert!(separation.abs() > 1e-3, "tourmaline must walk off visibly");
    assert!(
        (separation - (phi.tan() - (phi - rho).tan())).abs() < 1e-4,
        "walk-off separation"
    );

    // The ordinary mode never walks off.
    let ordinary = ModeCtx::new(modes, Line::D, Mode::Ordinary);
    let o_start = ordinary.enter(entry).expect("refracts");
    assert_eq!(ordinary.ray_dir(o_start.k).to_array(), o_start.k.to_array());
}

/// Sapphire round brilliant, table up: with `c` along the stone axis every table ray runs along
/// `c` and the returned light leaves close to it, so the two images barely separate; with `c`
/// across the table the returned light is perpendicular to `c` and the full birefringence shows.
#[test]
fn doubling_follows_the_axis_orientation() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let along = evaluate(&planes, Vec3::Y);
    let across = evaluate(&planes, Vec3::X);
    assert!(across.doubling_arcmin > 0.0, "{across:?}");
    assert!(along.doubling_arcmin.is_finite() && along.doubling_arcmin >= 0.0);
    assert!(
        along.doubling_arcmin < 0.5 * across.doubling_arcmin,
        "c = Y: {}, c = X: {}",
        along.doubling_arcmin,
        across.doubling_arcmin
    );
}

fn evaluate(planes: &[GpuFacetPlane], c_axis: Vec3) -> super::types::GemOpticalMetrics {
    super::evaluate_gem_optical_metrics(
        planes,
        &sapphire_with_axis(c_axis),
        0.0,
        FRAC_PI_2,
        studio(),
    )
}

/// The weighted shares are shares: in range, summing to at most the whole, deterministic, and the
/// Fire-less pass reports the same Doubling and the same three percentages.
#[test]
fn the_weighted_shares_are_consistent() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = sapphire_with_axis(Vec3::new(1.0, 0.4, 0.2).normalize());
    let a = super::evaluate_gem_optical_metrics(&planes, &material, 0.6, 0.45, studio());
    let b = super::evaluate_gem_optical_metrics(&planes, &material, 0.6, 0.45, studio());
    assert_eq!(a.brilliance_pct.to_bits(), b.brilliance_pct.to_bits());
    assert_eq!(a.doubling_arcmin.to_bits(), b.doubling_arcmin.to_bits());
    let sum = a.brilliance_pct + a.windowing_pct + a.extinction_pct;
    assert!(sum <= 100.0 + 1e-3, "{a:?}");
    for share in [a.brilliance_pct, a.windowing_pct, a.extinction_pct] {
        assert!((0.0..=100.0).contains(&share), "{a:?}");
    }
    assert!(a.fire_index.is_finite() && a.scintillation_pct.is_finite());
    let core = super::evaluate_gem_optical_metrics_geom_core(
        StoneGeometry::planes_only(&planes),
        &material,
        0.6,
        0.45,
        studio(),
    );
    assert_eq!(a.brilliance_pct.to_bits(), core.brilliance_pct.to_bits());
    assert_eq!(a.windowing_pct.to_bits(), core.windowing_pct.to_bits());
    assert_eq!(a.extinction_pct.to_bits(), core.extinction_pct.to_bits());
    assert_eq!(a.doubling_arcmin.to_bits(), core.doubling_arcmin.to_bits());
}
