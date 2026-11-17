//! Pins for `aset_zone_amplitudes`: the renderer's ASET radiance is built from it bit for bit,
//! and its zone bounds and blends are the ones the Contrast view and the zone shares rely on.

#![allow(
    clippy::cast_precision_loss,
    reason = "sweep indices are far below 2^24"
)]

use super::{
    LightingPreset, aset_zone_amplitudes, d65_relative_spectral_power,
    sample_studio_environment_with_rig_shadow_tray,
};
use crate::optics::studio_rig::StudioRig;
use glam::Vec3;

const RED_NM: f32 = 610.0;
const GREEN_NM: f32 = 540.0;
const BLUE_NM: f32 = 460.0;
const SIGMA_NM: f32 = 8.493_218;
const RED_GAIN: f32 = 8.0;
const GREEN_GAIN: f32 = 5.0;
const BLUE_GAIN: f32 = 16.0;

/// Unit direction at elevation `elevation_deg`.
fn at_elevation(elevation_deg: f32) -> Vec3 {
    let e = elevation_deg.to_radians();
    Vec3::new(e.cos(), e.sin(), 0.0)
}

/// Unit direction with `d.y == y` and azimuth `phi`.
fn with_y(y: f32, phi: f32) -> Vec3 {
    let r = y.mul_add(-y, 1.0).max(0.0).sqrt();
    Vec3::new(r * phi.cos(), y, r * phi.sin())
}

fn band(lambda_nm: f32, centre_nm: f32) -> f32 {
    let z = (lambda_nm - centre_nm) / SIGMA_NM;
    (-0.5 * z * z).exp()
}

/// 20 000 directions: a Fibonacci sphere plus six dense bands of `d.y` around the zone edges.
fn sweep_directions() -> Vec<Vec3> {
    const FIBONACCI: usize = 14_000;
    const PER_BAND: usize = 1_000;
    let golden_angle = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    let mut out = Vec::with_capacity(FIBONACCI + 6 * PER_BAND);
    for i in 0..FIBONACCI {
        let y = 1.0 - 2.0 * (i as f32 + 0.5) / FIBONACCI as f32;
        out.push(with_y(y, i as f32 * golden_angle));
    }
    let bands = [
        (-0.06f32, 0.0f32),
        (0.0, 0.06),
        (0.68, 0.71),
        (0.71, 0.74),
        (0.94, 0.965),
        (0.965, 0.99),
    ];
    for (lo, hi) in bands {
        for k in 0..PER_BAND {
            let y = lo + (hi - lo) * (k as f32 + 0.5) / PER_BAND as f32;
            out.push(with_y(y, k as f32 * golden_angle));
        }
    }
    out
}

/// The renderer's value at `d`, from the public sampler.
fn rendered(rig: &StudioRig, d: Vec3, lambda_nm: f32, exposure: f32, tray: f32) -> f32 {
    sample_studio_environment_with_rig_shadow_tray(
        d,
        lambda_nm,
        LightingPreset::Aset,
        exposure,
        rig,
        Vec3::ZERO,
        super::DEFAULT_HEAD_SHADOW_COSINES,
        tray,
    )
}

/// The same value rebuilt from `aset_zone_amplitudes` with the radiance's own operations. The
/// sampler normalizes its direction first; near a blend's end one ulp of `d.y` moves the
/// fading zone's amplitude by far more than one ulp, so the rebuild normalizes too.
fn rebuilt(d: Vec3, lambda_nm: f32, exposure: f32, tray: f32) -> f32 {
    let [red, green, blue, below] = aset_zone_amplitudes(d.normalize());
    let r = (red * band(lambda_nm, RED_NM)) * RED_GAIN;
    let g = (green * band(lambda_nm, GREEN_NM)) * GREEN_GAIN;
    let b = (blue * band(lambda_nm, BLUE_NM)) * BLUE_GAIN;
    let zones = ((r + g) + b) * exposure;
    let tray_ground = if tray > 0.0 { below * tray } else { 0.0 };
    if tray_ground > 0.0 {
        (tray_ground * d65_relative_spectral_power(lambda_nm)).mul_add(exposure, zones)
    } else {
        zones
    }
}

/// The renderer's ASET radiance equals the amplitudes pushed through the band arithmetic,
/// bit for bit, over a dense direction sweep, three band centers, and with and without a tray.
#[test]
fn zone_amplitudes_reproduce_the_rendered_radiance_bit_for_bit() {
    let rig = StudioRig::new(0.4, 0.35);
    let dirs = sweep_directions();
    assert_eq!(dirs.len(), 20_000);
    for &d in &dirs {
        for lambda in [RED_NM, GREEN_NM, BLUE_NM, 500.0] {
            for (exposure, tray) in [(1.0f32, 0.0f32), (1.7, 0.0), (1.0, 0.6), (0.8, 0.25)] {
                let got = rendered(&rig, d, lambda, exposure, tray);
                let want = rebuilt(d, lambda, exposure, tray);
                assert_eq!(
                    got.to_bits(),
                    want.to_bits(),
                    "d = {d:?}, lambda = {lambda}, exposure = {exposure}, tray = {tray}"
                );
            }
        }
    }
}

/// The four amplitudes sum to 1 and each zone is exactly full inside its limits.
#[test]
fn zone_amplitudes_sum_to_one_and_fill_their_zones() {
    for d in sweep_directions() {
        let [r, g, b, below] = aset_zone_amplitudes(d);
        let sum = r + g + b + below;
        assert!((sum - 1.0).abs() < 1e-6, "sum {sum} at {d:?}");
        for amplitude in [r, g, b, below] {
            assert!((0.0..=1.0).contains(&amplitude), "{amplitude} at {d:?}");
        }
    }
    let close = |got: [f32; 4], want: [f32; 4], what: &str| {
        for (a, b) in got.into_iter().zip(want) {
            assert!((a - b).abs() < 1e-6, "{what}: {got:?} vs {want:?}");
        }
    };
    close(
        aset_zone_amplitudes(at_elevation(60.0)),
        [1.0, 0.0, 0.0, 0.0],
        "red at 60",
    );
    close(
        aset_zone_amplitudes(at_elevation(20.0)),
        [0.0, 1.0, 0.0, 0.0],
        "green at 20",
    );
    close(
        aset_zone_amplitudes(at_elevation(85.0)),
        [0.0, 0.0, 1.0, 0.0],
        "blue at 85",
    );
    close(
        aset_zone_amplitudes(at_elevation(-10.0)),
        [0.0, 0.0, 0.0, 1.0],
        "below at -10",
    );
}

/// At the centers of the two blended edges the neighbouring zones share the light equally.
#[test]
fn zone_amplitudes_are_half_at_the_edge_centers() {
    // `sin(elevation)` midway between each edge's lo and hi.
    let [r, g, b, below] = aset_zone_amplitudes(with_y(std::f32::consts::FRAC_1_SQRT_2, 0.3));
    assert!(
        (r - 0.5).abs() < 1e-5 && (g - 0.5).abs() < 1e-5,
        "45: {r} {g}"
    );
    assert!(b.abs() < 1e-6 && below.abs() < 1e-6);
    let [r, g, b, below] = aset_zone_amplitudes(with_y(0.965_925_8, 0.3));
    assert!(
        (r - 0.5).abs() < 1e-5 && (b - 0.5).abs() < 1e-5,
        "75: {r} {b}"
    );
    assert!(g.abs() < 1e-6 && below.abs() < 1e-6);
    let [r, g, b, below] = aset_zone_amplitudes(with_y(0.0, 0.3));
    assert!(
        (below - 0.5).abs() < 1e-5 && (g - 0.5).abs() < 1e-5,
        "0: {g} {below}"
    );
    assert!(r.abs() < 1e-6 && b.abs() < 1e-6);
}

/// `below` is exactly zero wherever the horizon blend has finished, and exactly one under it.
#[test]
fn below_is_exactly_zero_above_the_horizon_blend() {
    for d in sweep_directions() {
        let below = aset_zone_amplitudes(d)[3];
        if d.y > 0.05 {
            assert_eq!(below.to_bits(), 0.0f32.to_bits(), "below at y = {}", d.y);
        }
        if d.y < -0.05 {
            assert_eq!(below.to_bits(), 1.0f32.to_bits(), "below at y = {}", d.y);
        }
    }
}
