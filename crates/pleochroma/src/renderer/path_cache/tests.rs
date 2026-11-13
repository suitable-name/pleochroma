//! Tests of the record encoding and the tile layout.

use glam::{Vec2, Vec3};

use super::{
    layout::{LayoutError, TileCache, TileCacheHeader, tile_count, tile_rect},
    record::{
        COMB_NM, ExitRecord, FLAG_PER_BIN, bin_of, f16_bits_to_f32, f32_to_f16_bits, oct_decode,
        oct_encode,
    },
    tests_support::angle_between,
};

fn plain(dir: Vec3, weight: [f32; 8]) -> ExitRecord {
    ExitRecord::new(dir, Vec2::ZERO, weight, 0, 0)
}

#[test]
fn record_is_44_bytes_and_the_header_16() {
    assert_eq!(size_of::<ExitRecord>(), 44);
    assert_eq!(size_of::<TileCacheHeader>(), 16);
}

#[test]
fn the_depth_coefficients_round_trip_and_give_the_optical_depth() {
    let record = plain(Vec3::Z, [0.5; 8]);
    assert!(!record.has_depth());
    assert_eq!(record.depths(), [0.0; 3]);
    assert_eq!(
        record.optical_depth([3.0, 2.0, 1.0]).to_bits(),
        0.0_f32.to_bits()
    );

    let deep = record.with_depth([0.25, 1.5, 0.125]);
    assert!(deep.has_depth());
    assert_eq!(deep.depths(), [0.25, 1.5, 0.125]);
    // 0.25 * 2 + 1.5 * 0.5 + 0.125 * 4, exact in f32.
    assert_eq!(
        deep.optical_depth([2.0, 0.5, 4.0]).to_bits(),
        1.75_f32.to_bits()
    );
    // The weight is untouched: the transmittance is the shader's.
    assert_eq!(deep.weights(), record.weights());
    assert_eq!(
        deep.weight_at(585.0).to_bits(),
        record.weight_at(585.0).to_bits()
    );

    // A coefficient that is not a positive finite number is stored as zero.
    let odd = record.with_depth([f32::NAN, -1.0, f32::INFINITY]);
    assert!(!odd.has_depth());
}

#[test]
fn octahedral_round_trip_stays_below_1e4_rad_over_a_sphere_sweep() {
    let mut worst = 0.0_f32;
    for i in 0..=200 {
        let theta = std::f32::consts::PI * i as f32 / 200.0;
        for j in 0..400 {
            let phi = std::f32::consts::TAU * j as f32 / 400.0;
            let dir = Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            );
            let back = plain(dir, [0.0; 8]).direction();
            worst = worst.max(angle_between(dir, back));
        }
    }
    for axis in [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ] {
        worst = worst.max(angle_between(axis, plain(axis, [0.0; 8]).direction()));
    }
    assert!(worst < 1.0e-4, "worst octahedral error {worst} rad");
}

#[test]
fn unquantized_octahedral_mapping_is_exact_to_f32() {
    for dir in [
        Vec3::new(0.3, -0.5, 0.8),
        Vec3::new(-0.7, 0.1, -0.2),
        Vec3::new(0.0, 0.0, -1.0),
    ] {
        let dir = dir.normalize();
        assert!(angle_between(dir, oct_decode(oct_encode(dir))) < 1.0e-6);
    }
}

#[test]
fn f16_conversion_round_trips_every_half_value() {
    for bits in 0..=u16::MAX {
        let value = f16_bits_to_f32(bits);
        if value.is_nan() {
            continue;
        }
        assert_eq!(f32_to_f16_bits(value), bits, "half {bits:#06x} = {value}");
    }
    assert_eq!(f32_to_f16_bits(1.0), 0x3c00);
    assert_eq!(f32_to_f16_bits(0.5), 0x3800);
    assert_eq!(f32_to_f16_bits(65504.0), 0x7bff);
    assert_eq!(f32_to_f16_bits(1.0e9), 0x7c00);
    assert_eq!(f32_to_f16_bits(0.0), 0);
    assert_eq!(f16_bits_to_f32(0x0001), 2.0_f32.powi(-24));
}

#[test]
fn f16_weight_error_is_below_1e3_relative() {
    let mut worst = 0.0_f32;
    for step in 0..4000 {
        // 1e-3 .. 1 on a geometric grid.
        let value = 10.0_f32.powf(-3.0 + 3.0 * step as f32 / 4000.0);
        let stored = f16_bits_to_f32(f32_to_f16_bits(value));
        worst = worst.max((stored - value).abs() / value);
    }
    assert!(worst < 1.0e-3, "worst relative weight error {worst}");
    let record = plain(Vec3::Z, [0.9, 0.5, 0.25, 0.125, 0.0625, 0.03, 0.01, 0.002]);
    for (stored, wanted) in record
        .weights()
        .into_iter()
        .zip([0.9, 0.5, 0.25, 0.125, 0.0625, 0.03, 0.01, 0.002])
    {
        assert!(
            (stored - wanted).abs() <= 1.0e-3 * wanted,
            "{stored} vs {wanted}"
        );
    }
}

#[test]
fn the_derivative_moves_the_direction_linearly_in_the_octahedral_domain() {
    let dir = Vec3::new(0.3, 0.6, 0.74).normalize();
    let slope = Vec2::new(0.01, -0.02);
    let record = ExitRecord::new(dir, slope, [1.0; 8], 0, 0);
    assert!(angle_between(record.direction(), dir) < 1.0e-4);
    for lambda in [410.0, 510.0, 560.0, 610.0, 760.0] {
        let wanted = oct_decode(oct_encode(dir) + slope * ((lambda - 560.0) / 100.0));
        let got = record.direction_at(lambda);
        assert!(angle_between(got, wanted) < 1.5e-4, "{lambda} nm");
    }
}

#[test]
fn weights_interpolate_across_the_comb_and_per_bin_records_stay_in_their_bin() {
    let weights = [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7];
    let record = plain(Vec3::Z, weights);
    for (lambda, wanted) in [
        (380.0, 0.0),
        (410.0, 0.0),
        (435.0, 0.05),
        (560.0, 0.3),
        (585.0, 0.35),
        (760.0, 0.7),
        (800.0, 0.7),
    ] {
        assert!(
            (record.weight_at(lambda) - wanted).abs() < 2.0e-3,
            "{lambda} nm: {} vs {wanted}",
            record.weight_at(lambda)
        );
    }

    let mut only_bin = [0.0; 8];
    only_bin[2] = 0.8;
    let per_bin = ExitRecord::new(Vec3::Z, Vec2::ZERO, only_bin, FLAG_PER_BIN | (2 << 8), 0);
    assert!(per_bin.is_per_bin());
    assert_eq!(per_bin.bin(), 2);
    assert!((per_bin.weight_at(COMB_NM[2]) - 0.8).abs() < 1.0e-3);
    assert_eq!(per_bin.weight_at(COMB_NM[1]), 0.0);
    assert_eq!(per_bin.weight_at(COMB_NM[3]), 0.0);
}

#[test]
fn bins_are_fifty_nanometres_wide_and_clamp_to_the_comb() {
    assert_eq!(bin_of(300.0), 0);
    assert_eq!(bin_of(385.0), 0);
    assert_eq!(bin_of(434.9), 0);
    assert_eq!(bin_of(435.0), 1);
    assert_eq!(bin_of(560.0), 3);
    assert_eq!(bin_of(784.0), 7);
    assert_eq!(bin_of(900.0), 7);
    for (index, nm) in COMB_NM.into_iter().enumerate() {
        assert_eq!(bin_of(nm), index);
    }
}

/// Three pixels: the first has two positions, the second none, the third one. The positions hold
/// 2, 1 and 3 records.
fn sample_tile() -> TileCache {
    let records: Vec<ExitRecord> = (0..6)
        .map(|i| plain(Vec3::new(i as f32, 1.0, 2.0), [0.1 * (i as f32 + 1.0); 8]))
        .collect();
    let residual = [
        f32_to_f16_bits(0.5),
        f32_to_f16_bits(0.25),
        f32_to_f16_bits(0.125),
    ];
    TileCache::from_parts(7, &[0, 2, 2, 3], &[0, 2, 3, 6], &residual, &records)
}

#[test]
fn layout_offsets_are_consistent_and_views_read_back() {
    let tile = sample_tile();
    let header = tile.header();
    assert_eq!(
        header,
        TileCacheHeader {
            tile_index: 7,
            pixel_count: 3,
            position_count: 3,
            record_count: 6,
        }
    );
    assert_eq!(tile.pixel_offsets(), &[0, 2, 2, 3]);
    assert_eq!(tile.position_offsets(), &[0, 2, 3, 6]);
    assert_eq!(tile.records().len(), 6);
    assert_eq!(tile.pixel_positions(0), 0..2);
    assert_eq!(tile.pixel_positions(1), 2..2);
    assert_eq!(tile.pixel_positions(2), 2..3);
    assert!(tile.pixel_positions(3).is_empty());
    assert_eq!(tile.position_records(0).len(), 2);
    assert_eq!(tile.position_records(1).len(), 1);
    assert_eq!(tile.position_records(2).len(), 3);
    assert_eq!(tile.position_records(3), [] as [ExitRecord; 0]);
    assert_eq!(tile.position_residual(0), 0.5);
    assert_eq!(tile.position_residual(1), 0.25);
    assert_eq!(tile.position_residual(2), 0.125);
    // 4 header + 4 pixel offsets + 4 position offsets + 2 residual words + 6 * 11 record words.
    assert_eq!(tile.as_words().len(), 4 + 4 + 4 + 2 + 66);
    assert_eq!(tile.byte_len(), tile.as_words().len() * 4);
    let again = TileCache::from_words(tile.as_words().to_vec()).expect("a valid buffer");
    assert_eq!(again, tile);
}

#[test]
fn layout_rejects_corrupt_buffers() {
    let words = sample_tile().into_words();
    assert_eq!(
        TileCache::from_words(vec![1, 2]),
        Err(LayoutError::TooShort { words: 2 })
    );
    let mut short = words.clone();
    short.pop();
    assert!(matches!(
        TileCache::from_words(short),
        Err(LayoutError::SizeMismatch { .. })
    ));
    let mut pixels = words.clone();
    pixels[4 + 3] = 9;
    assert_eq!(
        TileCache::from_words(pixels),
        Err(LayoutError::PixelOffsets)
    );
    let mut positions = words;
    positions[4 + 4 + 1] = 99;
    assert_eq!(
        TileCache::from_words(positions),
        Err(LayoutError::PositionOffsets)
    );
}

#[test]
fn an_empty_tile_is_just_a_header_and_two_zero_offsets() {
    let tile = TileCache::from_parts(3, &[0], &[0], &[], &[]);
    assert_eq!(tile.header().pixel_count, 0);
    assert_eq!(tile.as_words().len(), 4 + 1 + 1);
    assert!(TileCache::from_words(tile.as_words().to_vec()).is_ok());
}

#[test]
fn tile_rectangles_clip_at_the_frame_edge() {
    assert_eq!(tile_count(40, 20, 16), 6);
    assert_eq!(tile_rect(40, 20, 16, 0), (0, 0, 16, 16));
    assert_eq!(tile_rect(40, 20, 16, 2), (32, 0, 40, 16));
    assert_eq!(tile_rect(40, 20, 16, 5), (32, 16, 40, 20));
    let (x0, y0, x1, y1) = tile_rect(40, 20, 16, 6);
    assert!(
        x1 <= x0 || y1 <= y0,
        "past the last tile the rectangle is empty"
    );
}
