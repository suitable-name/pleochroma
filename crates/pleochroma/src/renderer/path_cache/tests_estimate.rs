//! Tests of the size estimate.

use glam::Vec3;

use super::{
    estimate::{
        COMPRESSION_RATIO_DEFAULT, K_EDGE_MEAN, Measurements, SizeEstimate, estimate_size,
        format_bytes, measure_prepass,
    },
    positions::{BuildSettings, K0},
    tests_support::{Rig, glass},
};
use crate::{
    geometry::GpuFacetPlane,
    optics::raytracer::{Camera, transport::PreparedMaterial},
};

/// A cube of edge 2 centered 100 units to the side, far outside the camera's view.
fn cube_out_of_view() -> Rig {
    let faces = [
        (Vec3::X, -101.0),
        (Vec3::NEG_X, 99.0),
        (Vec3::Y, -1.0),
        (Vec3::NEG_Y, -1.0),
        (Vec3::Z, -1.0),
        (Vec3::NEG_Z, -1.0),
    ];
    let planes = faces
        .iter()
        .map(|&(normal, d)| GpuFacetPlane::new(normal, d))
        .collect();
    Rig::new(
        planes,
        glass(),
        Camera::new(0.0, std::f32::consts::FRAC_PI_2, 8.0, 24.0),
    )
}

fn estimate_of(rig: &Rig, size: u32) -> SizeEstimate {
    let scene = rig.scene(size, 8);
    let prepared = PreparedMaterial::new(&rig.material);
    estimate_size(&scene, &prepared, &rig.soa, &BuildSettings::default())
}

#[test]
fn the_estimate_follows_the_documented_formula() {
    let measured = Measurements {
        pixels: 100,
        stone_pixels: 50,
        edge_pixels: 10,
        positions: 200,
        records: 400,
        fire_milli: 0,
    };
    let estimate = SizeEstimate::from_measurements(
        &measured,
        (400, 400),
        &BuildSettings::default(),
        COMPRESSION_RATIO_DEFAULT,
    );
    // Half the 160 000 pixels are stone; a fifth of them are edges: 4 * 0.8 + 24 * 0.2 = 8.
    assert_eq!(estimate.frame_pixels, 160_000);
    assert_eq!(estimate.stone_pixels, 80_000);
    assert!((estimate.edge_share - 0.2).abs() < 1.0e-12);
    assert!((estimate.positions_per_stone_pixel - 8.0).abs() < 1.0e-9);
    assert!((estimate.records_per_position - 2.0).abs() < 1.0e-12);
    assert_eq!(estimate.positions, 640_000);
    assert_eq!(estimate.records, 1_280_000);
    // Records 44 B, positions 6 B, pixels 4 B, 625 tiles of 24 B.
    assert_eq!(estimate.raw_bytes, 60_815_000);
    assert_eq!(estimate.compressed_bytes, 30_407_500);
    assert!(estimate.has_stone());
}

#[test]
fn the_edge_mean_is_capped_by_k_max_and_floored_by_k0() {
    let measured = Measurements {
        pixels: 100,
        stone_pixels: 100,
        edge_pixels: 50,
        positions: 400,
        records: 400,
        fire_milli: 0,
    };
    let positions_per_pixel = |k0, k_max| {
        SizeEstimate::from_measurements(
            &measured,
            (16, 16),
            &BuildSettings {
                k0,
                k_max,
                ..BuildSettings::default()
            },
            2.0,
        )
        .positions_per_stone_pixel
    };
    // Half the pixels are edges: the mean of K0 and the edge mean.
    assert!((positions_per_pixel(4, 64) - f64::midpoint(4.0, K_EDGE_MEAN)).abs() < 1.0e-9);
    assert!((positions_per_pixel(4, 8) - 6.0).abs() < 1.0e-9);
    assert!((positions_per_pixel(4, 4) - 4.0).abs() < 1.0e-9);
}

#[test]
fn a_frame_without_stone_is_zero() {
    let none = Measurements {
        pixels: 64,
        ..Measurements::default()
    };
    let estimate =
        SizeEstimate::from_measurements(&none, (256, 256), &BuildSettings::default(), 2.0);
    assert_eq!(estimate.raw_bytes, 0);
    assert_eq!(estimate.compressed_bytes, 0);
    assert_eq!(estimate.positions, 0);
    assert_eq!(estimate.records, 0);
    assert_eq!(estimate.stone_pixels, 0);
    assert!(!estimate.has_stone());

    let rig = cube_out_of_view();
    let measured = measure_prepass(
        &rig.scene(32, 8),
        &PreparedMaterial::new(&rig.material),
        &rig.soa,
        &BuildSettings::default(),
    );
    assert_eq!(measured.stone_pixels, 0);
    assert_eq!(measured.pixels, 1024);
    let scene_estimate = estimate_of(&rig, 32);
    assert_eq!(scene_estimate.raw_bytes, 0);
    assert!(!scene_estimate.has_stone());
}

#[test]
fn a_better_compression_ratio_shrinks_the_disk_size_only() {
    let measured = Measurements {
        pixels: 16,
        stone_pixels: 8,
        edge_pixels: 2,
        positions: 32,
        records: 64,
        fire_milli: 0,
    };
    let at = |ratio| {
        SizeEstimate::from_measurements(&measured, (64, 64), &BuildSettings::default(), ratio)
    };
    assert_eq!(at(2.0).raw_bytes, at(4.0).raw_bytes);
    assert!(at(4.0).compressed_bytes < at(2.0).compressed_bytes);
    // A ratio below one never grows the file.
    assert_eq!(at(0.5).compressed_bytes, at(1.0).compressed_bytes);
    assert_eq!(at(1.0).compressed_bytes, at(1.0).raw_bytes);
}

#[test]
fn the_prepass_is_deterministic_and_measures_the_stone() {
    let rig = Rig::cube_from_above(glass());
    let first = estimate_of(&rig, 32);
    let second = estimate_of(&rig, 32);
    assert_eq!(first, second);

    assert!(first.has_stone());
    assert!(first.stone_pixels > 0 && first.stone_pixels < first.frame_pixels);
    assert!((0.0..=1.0).contains(&first.edge_share));
    assert!(first.positions_per_stone_pixel >= f64::from(K0));
    assert!(first.positions_per_stone_pixel <= K_EDGE_MEAN);
    assert!(first.records_per_position >= 1.0);
    assert!(first.compressed_bytes < first.raw_bytes);
}

#[test]
fn the_prepass_probes_a_lattice_of_at_most_1024_pixels() {
    use super::estimate::{PROBE_PIXELS, probe_step};
    assert_eq!(probe_step(32, 32), 1);
    assert_eq!(probe_step(64, 64), 2);
    assert_eq!(probe_step(512, 512), 16);
    assert_eq!(probe_step(1920, 1080), 46);
    assert!(1920_u32.div_ceil(46) * 1080_u32.div_ceil(46) <= PROBE_PIXELS);

    let rig = Rig::cube_from_above(glass());
    for size in [32, 64] {
        let measured = measure_prepass(
            &rig.scene(size, 8),
            &PreparedMaterial::new(&rig.material),
            &rig.soa,
            &BuildSettings::default(),
        );
        assert_eq!(measured.pixels, 1024, "size {size}");
        // First level only: every stone pixel has exactly K0 positions.
        assert_eq!(measured.positions, measured.stone_pixels * u64::from(K0));
        assert!(measured.records >= measured.positions);
        assert!(measured.edge_pixels <= measured.stone_pixels);
    }
    // At 32 pixels every pixel is probed: the cube's silhouette crosses pixels, which therefore
    // disagree between positions.
    let all = measure_prepass(
        &rig.scene(32, 8),
        &PreparedMaterial::new(&rig.material),
        &rig.soa,
        &BuildSettings::default(),
    );
    assert!(all.edge_pixels > 0);
}

#[test]
fn the_estimate_scales_with_the_resolution() {
    let rig = Rig::cube_from_above(glass());
    let small = estimate_of(&rig, 32);
    let large = estimate_of(&rig, 64);
    assert_eq!(large.frame_pixels, 4 * small.frame_pixels);
    // Four times the pixels, but the edge band is one pixel wide at either size, so its share of
    // the stone falls as the resolution rises and the bytes grow by less than the pixel count.
    assert!(large.edge_share < small.edge_share, "{large:?} {small:?}");
    assert!(large.raw_bytes > small.raw_bytes, "{large:?} {small:?}");
    assert!(large.raw_bytes < small.raw_bytes * 6, "{large:?} {small:?}");
}

#[test]
fn sizes_read_well() {
    assert_eq!(format_bytes(0), "under 1 MB");
    assert_eq!(format_bytes(999_999), "under 1 MB");
    assert_eq!(format_bytes(850_000_000), "850 MB");
    assert_eq!(format_bytes(4_200_000_000), "4.2 GB");
    assert_eq!(format_bytes(11_400_000_000), "11 GB");

    let measured = Measurements {
        pixels: 1,
        stone_pixels: 1,
        edge_pixels: 0,
        positions: 4,
        records: 4,
        fire_milli: 0,
    };
    let estimate =
        SizeEstimate::from_measurements(&measured, (8000, 4000), &BuildSettings::default(), 2.0);
    assert!(estimate.size_text().starts_with("about "));
    assert!(estimate.size_text().ends_with(" GB"));
}
