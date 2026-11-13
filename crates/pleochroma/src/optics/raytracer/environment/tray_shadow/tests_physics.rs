//! Physical checks on the irradiance table and traced maps.
use super::{
    build::{BuildSettings, TrayShadowInput, build_with_settings},
    lights::LightTable,
    *,
};
use crate::{
    geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane, tool::StoneGeometry},
    optics::{
        materials::GemMaterial,
        raytracer::environment::{LightingPreset, rig_irradiance_over_pi},
    },
};
use glam::Vec3;
use std::sync::atomic::AtomicBool;

#[test]
fn gather_table_total_is_the_open_irradiance() {
    for preset in LightingPreset::ALL {
        if preset.is_uv_lamp() || preset == LightingPreset::Aset {
            continue;
        }
        for (yaw, pitch) in [(0.0, 0.0), (0.7, 0.3), (-1.1, -0.2)] {
            let actual = LightTable::gather(preset, yaw, pitch).total;
            let expected =
                std::f64::consts::PI * f64::from(rig_irradiance_over_pi(preset, yaw, pitch));
            assert!(
                (actual - expected).abs() <= expected * 1e-6,
                "{preset:?}: {actual} vs {expected}"
            );
        }
    }
}

fn plate() -> Vec<GpuFacetPlane> {
    [
        (Vec3::X, -10.0),
        (Vec3::NEG_X, -10.0),
        (Vec3::Y, -0.5),
        (Vec3::NEG_Y, -0.5),
        (Vec3::Z, -10.0),
        (Vec3::NEG_Z, -10.0),
    ]
    .into_iter()
    .map(|(n, d)| GpuFacetPlane::new(n, d))
    .collect()
}

fn map(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    preset: LightingPreset,
    size: u32,
    dirs: u32,
    photons: u64,
) -> TrayShadowMap {
    let input = TrayShadowInput {
        geom: StoneGeometry::planes_only(planes),
        material,
        preset,
        yaw: 0.0,
        pitch: 0.6,
        max_bounces: 32,
    };
    build_with_settings(
        &input,
        TrayShadowQuality::Preview,
        &AtomicBool::new(false),
        &mut |_| {},
        BuildSettings {
            resolution: size,
            directions: dirs,
            photons,
            threads: 1,
            photon_chunk_size: 65_536,
        },
    )
    .expect("valid shadow")
}

fn diffuse_transmittance() -> f32 {
    let mut sum = 0.0;
    for i in 0..10_000 {
        let theta = (i as f32 + 0.5) * std::f32::consts::FRAC_PI_2 / 10_000.0;
        let r = crate::renderer::whitted::fresnel::fresnel_unpolarised_f32(theta.cos(), 1.0 / 1.5);
        sum +=
            2.0 * (1.0 - r) / (1.0 + r) * theta.cos() * theta.sin() * std::f32::consts::FRAC_PI_2
                / 10_000.0;
    }
    sum
}

/// Centre wavelength of band `b`, in nanometres.
const fn band_nm(b: usize) -> f32 {
    TRAY_SHADOW_BAND_WIDTH_NM.mul_add(b as f32, TRAY_SHADOW_FIRST_CENTER_NM)
}

/// Mean ratio over a small patch under the plate's centre. The plate is wide enough that the
/// map is flat there, so averaging a few lookups only trims the photon noise.
fn centre_mean(m: &TrayShadowMap) -> f32 {
    let mut sum = 0.0;
    for iy in -1..=1 {
        for ix in -1..=1 {
            sum += m.factor_at([ix as f32 * 0.05, iy as f32 * 0.05], 550.0);
        }
    }
    sum / 9.0
}

fn clear_plate_material() -> GemMaterial {
    GemMaterial::new_custom("clear", 1.5, 0.0, 0.0, [0.0; 3])
}

fn check_clear_plate(size: u32, dirs: u32, photons: u64) {
    let m = map(
        &plate(),
        &clear_plate_material(),
        LightingPreset::IsoHemisphere,
        size,
        dirs,
        photons,
    );
    let value = centre_mean(&m);
    let expected = diffuse_transmittance();
    assert!(
        (value - expected).abs() < 0.02 * expected,
        "centre {value}, diffuse transmittance {expected}"
    );
}

#[test]
fn a_clear_plate_transmits_its_diffuse_transmittance() {
    // 800 000 photons keep the 2 % gate beyond 3 sigma of the centre-patch photon noise.
    check_clear_plate(32, 64, 800_000);
}

#[test]
#[ignore = "full-resolution photon reference is expensive; run with --ignored"]
fn a_clear_plate_transmits_its_diffuse_transmittance_full() {
    check_clear_plate(512, 1024, 12_000_000);
}

#[test]
fn light_reaches_the_paper_under_a_clear_plate() {
    // A scattering body deposits nothing (C is zero) but blocks identically, so the
    // difference of the two maps is the redirected fraction C itself.
    let clear = clear_plate_material();
    let mut matte = clear.clone();
    matte.scattering_sigma_s = 1.0;
    let build = |material: &GemMaterial| {
        map(
            &plate(),
            material,
            LightingPreset::IsoHemisphere,
            32,
            64,
            200_000,
        )
    };
    let redirected = centre_mean(&build(&clear)) - centre_mean(&build(&matte));
    let expected = diffuse_transmittance();
    assert!(
        redirected > 0.5 * expected,
        "redirected fraction {redirected} under a clear plate, diffuse transmittance {expected}"
    );
}

#[test]
fn an_opaque_plate_casts_a_full_shadow() {
    // `new_custom` converts RGB peaks to three Gaussian absorption bands; the
    // finite peaks below extinguish paths across the visible spectrum without
    // switching to the scattering shortcut.
    let material = GemMaterial::new_custom("opaque", 1.5, 0.0, 0.0, [1.0e6; 3]);
    let m = map(
        &plate(),
        &material,
        LightingPreset::IsoHemisphere,
        32,
        64,
        200_000,
    );
    assert!(m.factor_at([0.0, 0.0], 550.0) < 0.02);
    assert!(m.factor_at([0.95, 0.0], 550.0) > 0.98);
}

#[test]
fn the_stone_never_creates_light() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let input = TrayShadowInput {
        geom: StoneGeometry::planes_only(&planes),
        material: &material,
        preset: LightingPreset::DaylightDome,
        yaw: 0.0,
        pitch: 0.6,
        max_bounces: 32,
    };
    let (_, energy) = super::build::build_with_energy(
        &input,
        TrayShadowQuality::Preview,
        &AtomicBool::new(false),
        &mut |_| {},
        BuildSettings {
            resolution: 32,
            directions: 64,
            photons: 200_000,
            threads: 1,
            photon_chunk_size: 65_536,
        },
    )
    .expect("valid shadow");
    for (band, balance) in energy.iter().enumerate() {
        assert!(
            *balance >= 0.0,
            "band {band}: net absorbed irradiance {balance}"
        );
    }
}

#[test]
fn an_opaque_body_deposits_no_photons() {
    use super::{
        geometry::{StoneFrame, StoneHit},
        photons::{PhotonCtx, trace_chunk},
    };
    use crate::renderer::whitted::tint::BinnedAbsorption;
    let planes = StandardGemCuts::standard_round_brilliant();
    let geom = StoneGeometry::planes_only(&planes);
    let frame = StoneFrame::new(geom).expect("valid stone");
    let stone = StoneHit::new(geom);
    assert!(stone.hits(
        frame.sphere_center + Vec3::Y * (2.0 * frame.sphere_radius),
        Vec3::NEG_Y
    ));
    let lights = LightTable::emission(LightingPreset::DaylightDome, 0.0, 0.6);
    let absorption = BinnedAbsorption::NONE;
    let context = PhotonCtx {
        frame: &frame,
        stone: &stone,
        table: &lights,
        n: [1.5; TRAY_SHADOW_BANDS],
        absorption: &absorption,
        opaque_body: true,
        convex: true,
        max_bounces: 8,
        weight: 1.0,
    };
    let mut deposits = Vec::new();
    trace_chunk(&context, 0, 0, 4096, &mut deposits);
    assert!(deposits.is_empty());
}

#[test]
fn ruby_absorption_transmits_more_red_than_green() {
    let material = GemMaterial::ruby();
    let environment = LightingPreset::LightTent.studio(1.0, 0.0, 0.6);
    let absorption = crate::renderer::whitted::tint::binned_absorption_for(&material, environment);
    let transmission = super::photons::band_transmittance(&absorption, Vec3::Y, 1.0);
    let red = transmission[12..20].iter().sum::<f32>() / 8.0;
    let green = transmission[6..=8].iter().sum::<f32>() / 3.0;
    assert!(red > green, "red {red}, green {green}");
}

#[test]
fn diamond_has_normal_dispersion_between_shadow_bands() {
    let material = GemMaterial::diamond();
    assert!(material.dispersion.evaluate(390.0) > material.dispersion.evaluate(770.0));
}

#[test]
fn a_ruby_casts_a_red_shadow() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let m = map(
        &planes,
        &GemMaterial::ruby(),
        LightingPreset::LightTent,
        32,
        64,
        200_000,
    );
    let red = (12..20)
        .map(|b| m.factor_at([0.0, 0.0], band_nm(b)))
        .sum::<f32>()
        / 8.0;
    let green = (6..=8)
        .map(|b| m.factor_at([0.0, 0.0], band_nm(b)))
        .sum::<f32>()
        / 3.0;
    assert!(red >= 1.2 * green, "red {red}, green {green}");
}

/// A texel map under a symmetry of the grid: `(n, i, j) -> (i', j')`.
type Symmetry = fn(usize, usize, usize) -> (usize, usize);

fn check_symmetry(size: u32, dirs: u32, photons: u64) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let m = map(
        &planes,
        &GemMaterial::diamond(),
        LightingPreset::IsoHemisphere,
        size,
        dirs,
        photons,
    );
    // Texel values are compared under the quarter turn (i, j) -> (j, n-1-i), which is exact on the
    // centred square grid and a symmetry of the round brilliant's 8-fold rotation whatever its
    // azimuth. (A mirror is left out: whether a mirror line lies on a map axis depends on how the
    // cut is turned.) Every third texel in each direction is used because the sigma 0.75
    // blur correlates neighbours within two texels, so each difference is an independent draw; the
    // border band, where the fade enters, is skipped.
    let n = usize::try_from(m.resolution()).expect("resolution fits usize");
    let value = |i: usize, j: usize| m.factors()[(j * n + i) * TRAY_SHADOW_BANDS + 8];
    let margin = (n * 3 / 32).max(3);
    let mut report = Vec::new();
    let symmetries: [(&str, Symmetry); 1] = [("quarter turn", |n, i, j| (j, n - 1 - i))];
    for (name, map_texel) in symmetries {
        let mut differences = Vec::new();
        for j in (margin..n - margin).step_by(3) {
            for i in (margin..n - margin).step_by(3) {
                let (pi, pj) = map_texel(n, i, j);
                differences.push(value(i, j) - value(pi, pj));
            }
        }
        let count = differences.len() as f32;
        let mean = differences.iter().sum::<f32>() / count;
        let variance = differences
            .iter()
            .map(|d| (d - mean) * (d - mean))
            .sum::<f32>()
            / (count - 1.0);
        let sigma = (variance / count).sqrt();
        report.push((name, mean, sigma));
    }
    let failures: Vec<_> = report
        .iter()
        .filter(|(_, mean, sigma)| mean.abs() > 3.0 * sigma)
        .collect();
    assert!(
        failures.is_empty(),
        "symmetries off by more than 3 sigma: {failures:?}; all (name, mean, sigma): {report:?}"
    );
}

#[test]
fn a_round_brilliant_casts_a_symmetric_shadow() {
    check_symmetry(32, 64, 200_000);
}

#[test]
#[ignore = "full-resolution rotational-symmetry reference is expensive; run with --ignored"]
fn a_round_brilliant_casts_a_symmetric_shadow_full() {
    check_symmetry(512, 1024, 12_000_000);
}

#[test]
fn a_diamond_spreads_its_caustics_by_wavelength() {
    // A wedge gives a known refraction direction, unlike a many-facet centroid whose
    // sign depends on its mix of paths. It is still a diamond dispersion curve.
    let slope = 0.25_f32;
    let normal = Vec3::new(0.0, 1.0, -slope).normalize();
    let planes: Vec<_> = [
        (Vec3::X, -2.0),
        (Vec3::NEG_X, -2.0),
        (Vec3::NEG_Y, -0.5),
        (Vec3::Z, -2.0),
        (Vec3::NEG_Z, -2.0),
        (normal, -0.5 / slope.mul_add(slope, 1.0).sqrt()),
    ]
    .into_iter()
    .map(|(n, d)| GpuFacetPlane::new(n, d))
    .collect();
    let diamond = GemMaterial::diamond();
    let m = map(
        &planes,
        &diamond,
        LightingPreset::DaylightSun,
        32,
        64,
        200_000,
    );
    let mut absorber = diamond.clone();
    absorber.scattering_sigma_s = 1.0;
    let blocked = map(
        &planes,
        &absorber,
        LightingPreset::DaylightSun,
        32,
        64,
        200_000,
    );
    let centroid = |band: usize| {
        let mut weighted = Vec3::ZERO;
        let mut sum = 0.0;
        for y in 0..32 {
            for x in 0..32 {
                let index = (y * 32 + x) * 20 + band;
                let value = (m.factors()[index] - blocked.factors()[index]).max(0.0);
                weighted += Vec3::new(x as f32, 0.0, y as f32) * value;
                sum += value;
            }
        }
        if sum > 0.0 { weighted / sum } else { weighted }
    };
    let incoming = -crate::optics::studio_rig::StudioRig::new(0.0, 0.6).key_dir;
    let predicted = |wavelength| {
        let n = diamond.dispersion.evaluate(wavelength);
        let inside =
            crate::renderer::whitted::fresnel::refract(incoming, normal, 1.0 / n).expect("entry");
        let out = crate::renderer::whitted::fresnel::refract(inside, Vec3::Y, n).expect("exit");
        let entry = Vec3::new(0.0, 0.5, 0.0);
        let exit = entry + inside * ((-0.5 - entry.y) / inside.y);
        out.z.mul_add((-0.51 - exit.y) / out.y, exit.z)
    };
    let expected = predicted(390.0) - predicted(770.0);
    let actual = centroid(0).z - centroid(19).z;
    assert!(
        actual * expected > 0.0,
        "centroid shift {actual}, predicted {expected}"
    );
}

#[test]
fn a_round_brilliant_separates_extreme_band_caustics() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    let m = map(
        &planes,
        &diamond,
        LightingPreset::DaylightSun,
        32,
        64,
        200_000,
    );
    let mut absorber = diamond;
    absorber.scattering_sigma_s = 1.0;
    let blocked = map(
        &planes,
        &absorber,
        LightingPreset::DaylightSun,
        32,
        64,
        200_000,
    );
    let centroid = |band: usize| {
        let mut weighted = Vec3::ZERO;
        let mut sum = 0.0;
        for y in 0..32 {
            for x in 0..32 {
                let index = (y * 32 + x) * 20 + band;
                let value = (m.factors()[index] - blocked.factors()[index]).max(0.0);
                weighted += Vec3::new(x as f32, 0.0, y as f32) * value;
                sum += value;
            }
        }
        if sum > 0.0 { weighted / sum } else { weighted }
    };
    assert!((centroid(0) - centroid(19)).length() > 1e-5);
}
