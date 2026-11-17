//! Tests of the zone weights, the spot grid, Contrast and the accumulator of
//! [`super::aset`].

#![allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "the tests compare exact bit-identical values and build small grid values from indices"
)]

use super::{
    aset::{ASET_GRID, AsetAccumulator, AsetSpotGrid, AsetZoneWeights, pattern_contrast},
    fan::GRID_DISC_RADIUS_SQ,
};
use glam::Vec3;

const SPOTS: usize = ASET_GRID * ASET_GRID;

/// Whether spot `(ix, iz)` lies inside the metrics grid's disc, by the grid pass's own test.
fn in_disc(ix: usize, iz: usize) -> bool {
    let side = ASET_GRID as f32;
    let u = ((ix as f32 + 0.5) / side).mul_add(2.0, -1.0);
    let v = ((iz as f32 + 0.5) / side).mul_add(2.0, -1.0);
    v.mul_add(v, u * u) <= GRID_DISC_RADIUS_SQ
}

/// A grid with `value(ix, iz)` on every spot inside the disc and `None` outside.
fn disc_grid(mut value: impl FnMut(usize, usize) -> f32) -> [Option<f32>; SPOTS] {
    let mut grid = [None; SPOTS];
    for ix in 0..ASET_GRID {
        for iz in 0..ASET_GRID {
            if in_disc(ix, iz) {
                grid[ix * ASET_GRID + iz] = Some(value(ix, iz));
            }
        }
    }
    grid
}

/// A direction at `elevation_deg` above the horizontal plane.
fn direction(elevation_deg: f32) -> Vec3 {
    let e = elevation_deg.to_radians();
    Vec3::new(e.cos(), e.sin(), 0.0)
}

fn weights_sum(w: AsetZoneWeights) -> f32 {
    w.red + w.green + w.blue + w.leak + w.lost
}

#[test]
fn the_full_disc_has_180_spots_and_328_pairs() {
    let grid = disc_grid(|_, _| 0.0);
    assert_eq!(grid.iter().flatten().count(), 180);
    let mut pairs = 0;
    for ix in 0..ASET_GRID {
        for iz in 0..ASET_GRID {
            let here = grid[ix * ASET_GRID + iz].is_some();
            if ix + 1 < ASET_GRID && here && grid[(ix + 1) * ASET_GRID + iz].is_some() {
                pairs += 1;
            }
            if iz + 1 < ASET_GRID && here && grid[ix * ASET_GRID + iz + 1].is_some() {
                pairs += 1;
            }
        }
    }
    assert_eq!(pairs, 328);
}

#[test]
fn contrast_is_zero_without_blue_or_with_one_solid_patch() {
    assert_eq!(pattern_contrast(&[None; SPOTS]), 0.0);
    assert_eq!(pattern_contrast(&disc_grid(|_, _| 0.0)), 0.0);
    assert_eq!(pattern_contrast(&disc_grid(|_, _| 1.0)), 0.0);
    assert_eq!(pattern_contrast(&disc_grid(|_, _| 0.37)), 0.0);
}

#[test]
fn a_checkerboard_reads_one_hundred() {
    let grid = disc_grid(|ix, iz| if (ix + iz) % 2 == 0 { 1.0 } else { 0.0 });
    assert!((pattern_contrast(&grid) - 100.0).abs() < 1e-4);
}

#[test]
fn one_blue_half_crosses_sixteen_of_328_pairs() {
    let grid = disc_grid(|ix, _| if ix < ASET_GRID / 2 { 1.0 } else { 0.0 });
    let expected = 100.0 * 16.0 / 328.0;
    assert!((pattern_contrast(&grid) - expected).abs() < 1e-4);
}

#[test]
fn off_stone_spots_are_in_no_pair() {
    let mut grid = [None; SPOTS];
    // A lone spot has no pair.
    grid[5 * ASET_GRID + 5] = Some(1.0);
    assert_eq!(pattern_contrast(&grid), 0.0);
    // Two neighbours across a gap of `None` do not pair; an adjacent one does.
    grid[5 * ASET_GRID + 7] = Some(0.0);
    assert_eq!(pattern_contrast(&grid), 0.0);
    grid[5 * ASET_GRID + 6] = Some(0.0);
    // Pairs: (5,5)-(5,6) differs by 1, (5,6)-(5,7) differs by 0.
    assert!((pattern_contrast(&grid) - 50.0).abs() < 1e-4);
}

#[test]
fn fractional_shares_give_fractional_contrast() {
    let mut grid = [None; SPOTS];
    grid[3 * ASET_GRID + 3] = Some(0.25);
    grid[4 * ASET_GRID + 3] = Some(0.75);
    assert!((pattern_contrast(&grid) - 50.0).abs() < 1e-4);
}

#[test]
fn contrast_stays_within_zero_and_one_hundred() {
    let mut state = 0x2545_f491_u32;
    for _ in 0..50 {
        let grid = disc_grid(|_, _| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / (1_u32 << 24) as f32
        });
        let c = pattern_contrast(&grid);
        assert!((0.0..=100.0).contains(&c), "contrast {c}");
    }
    // Out-of-range inputs are clamped, so the bound still holds.
    let wild = disc_grid(|ix, iz| if (ix + iz) % 2 == 0 { 7.0 } else { -3.0 });
    assert!((pattern_contrast(&wild) - 100.0).abs() < 1e-4);
}

#[test]
fn contrast_is_deterministic() {
    let grid = disc_grid(|ix, iz| ((ix * 7 + iz * 3) % 5) as f32 / 4.0);
    assert_eq!(
        pattern_contrast(&grid).to_bits(),
        pattern_contrast(&grid.clone()).to_bits()
    );
}

#[test]
fn from_exit_puts_the_below_horizon_part_in_leak() {
    let down = AsetZoneWeights::from_exit(Vec3::NEG_Y);
    assert_eq!(down.leak, 1.0);
    assert_eq!(
        (down.red, down.green, down.blue, down.lost),
        (0.0, 0.0, 0.0, 0.0)
    );

    let up = AsetZoneWeights::from_exit(Vec3::Y);
    assert!(up.blue > 0.999);
    assert_eq!(up.leak, 0.0);

    let red = AsetZoneWeights::from_exit(direction(60.0));
    assert!(red.red > 0.999);
    let green = AsetZoneWeights::from_exit(direction(20.0));
    assert!(green.green > 0.999);

    for elevation in [
        -30.0, -3.0, 0.0, 10.0, 44.0, 45.0, 46.0, 73.0, 76.0, 80.0, 89.0,
    ] {
        let w = AsetZoneWeights::from_exit(direction(elevation));
        assert!((weights_sum(w) - 1.0).abs() < 1e-5, "elevation {elevation}");
        assert_eq!(w.lost, 0.0);
    }
}

#[test]
fn leak_and_lost_are_unit_weights() {
    assert_eq!(weights_sum(AsetZoneWeights::LEAK), 1.0);
    assert_eq!(AsetZoneWeights::LEAK.leak, 1.0);
    assert_eq!(weights_sum(AsetZoneWeights::LOST), 1.0);
    assert_eq!(AsetZoneWeights::LOST.lost, 1.0);
    assert_eq!(
        AsetZoneWeights::default(),
        AsetZoneWeights {
            red: 0.0,
            green: 0.0,
            blue: 0.0,
            leak: 0.0,
            lost: 0.0,
        }
    );
}

#[test]
fn an_empty_accumulator_finishes_to_zeros() {
    let acc = AsetAccumulator::default();
    let shares = acc.finish(1.0);
    assert_eq!(shares.red_pct, 0.0);
    assert_eq!(shares.green_pct, 0.0);
    assert_eq!(shares.blue_pct, 0.0);
    assert_eq!(shares.leak_pct, 0.0);
    assert_eq!(shares.lost_pct, 0.0);
    assert_eq!(shares.contrast, 0.0);
    assert_eq!(shares.dark_pct(), 0.0);
    assert!(acc.spots().blue.iter().all(Option::is_none));
}

/// Folds a fixed mixed set of rays into an accumulator; returns it and the ray weight.
fn mixed_accumulator() -> (AsetAccumulator, f32) {
    let mut acc = AsetAccumulator::default();
    let rays = [
        (0, AsetZoneWeights::from_exit(direction(85.0))),
        (1, AsetZoneWeights::from_exit(direction(60.0))),
        (2, AsetZoneWeights::from_exit(direction(20.0))),
        (3, AsetZoneWeights::from_exit(direction(45.0))),
        (4, AsetZoneWeights::from_exit(direction(77.0))),
        (5, AsetZoneWeights::LEAK),
        (6, AsetZoneWeights::LOST),
        (19, AsetZoneWeights::from_exit(direction(85.0))),
        (19, AsetZoneWeights::from_exit(direction(10.0))),
    ];
    for (spot, weights) in rays {
        acc.add(spot, weights, 1.0);
    }
    (acc, rays.len() as f32)
}

#[test]
fn the_shares_sum_to_one_hundred() {
    let (acc, n) = mixed_accumulator();
    let s = acc.finish(n);
    let total = s.red_pct + s.green_pct + s.blue_pct + s.leak_pct + s.lost_pct;
    assert!((total - 100.0).abs() < 1e-3, "total {total}");
    assert!((s.dark_pct() - (s.leak_pct + s.lost_pct)).abs() < 1e-6);
    // One leak ray and one lost ray out of nine.
    assert!((s.leak_pct - 100.0 / 9.0).abs() < 1e-3);
    assert!((s.lost_pct - 100.0 / 9.0).abs() < 1e-3);
}

#[test]
fn the_same_inputs_in_the_same_order_give_the_same_bits() {
    let (a, n) = mixed_accumulator();
    let (b, _) = mixed_accumulator();
    let (sa, sb) = (a.finish(n), b.finish(n));
    assert_eq!(sa.red_pct.to_bits(), sb.red_pct.to_bits());
    assert_eq!(sa.green_pct.to_bits(), sb.green_pct.to_bits());
    assert_eq!(sa.blue_pct.to_bits(), sb.blue_pct.to_bits());
    assert_eq!(sa.leak_pct.to_bits(), sb.leak_pct.to_bits());
    assert_eq!(sa.lost_pct.to_bits(), sb.lost_pct.to_bits());
    assert_eq!(sa.contrast.to_bits(), sb.contrast.to_bits());
    assert_eq!(a.spots(), b.spots());
}

#[test]
fn spot_shares_are_mode_weighted_in_numerator_and_denominator() {
    let mut acc = AsetAccumulator::default();
    // Two eigenmodes of one ray: one fully blue, one fully green, half weight each.
    acc.add(0, AsetZoneWeights::from_exit(Vec3::Y), 0.5);
    acc.add(0, AsetZoneWeights::from_exit(direction(20.0)), 0.5);
    // A single full-weight blue ray in another spot.
    acc.add(1, AsetZoneWeights::from_exit(Vec3::Y), 1.0);
    let grid: AsetSpotGrid = acc.spots();
    let first = grid.blue[0].expect("spot 0 was hit");
    assert!((first - 0.5).abs() < 1e-3);
    assert!(grid.blue[1].expect("spot 1 was hit") > 0.999);
    assert!(grid.blue[2].is_none());
}

/// The leak percentage the Windowing figure computes from a count of windowed rays.
fn windowing_expression(windowed: f32, n_total: f32) -> f32 {
    (windowed / n_total * 100.0).clamp(0.0, 100.0)
}

#[test]
fn the_leak_percentage_has_the_bits_of_the_windowing_expression() {
    // Whole counts (isotropic stone) and half counts (two modes of one half each).
    for (rays, n_total) in [
        (0_u32, 900.0_f32),
        (1, 900.0),
        (37, 713.0),
        (900, 900.0),
        (123, 777.0),
    ] {
        let mut acc = AsetAccumulator::default();
        for i in 0..rays {
            acc.add(i as usize % SPOTS, AsetZoneWeights::LEAK, 1.0);
        }
        assert_eq!(
            acc.finish(n_total).leak_pct.to_bits(),
            windowing_expression(rays as f32, n_total).to_bits(),
            "whole count {rays} of {n_total}"
        );

        let mut halves = AsetAccumulator::default();
        for i in 0..rays {
            halves.add(i as usize % SPOTS, AsetZoneWeights::LEAK, 0.5);
            halves.add(i as usize % SPOTS, AsetZoneWeights::LEAK, 0.5);
        }
        assert_eq!(
            halves.finish(n_total).leak_pct.to_bits(),
            windowing_expression(rays as f32, n_total).to_bits(),
            "half counts {rays} of {n_total}"
        );
    }
    // A half-integer count: an odd number of single-mode half weights.
    let mut odd = AsetAccumulator::default();
    for _ in 0..7 {
        odd.add(0, AsetZoneWeights::LEAK, 0.5);
    }
    assert_eq!(
        odd.finish(100.0).leak_pct.to_bits(),
        windowing_expression(3.5, 100.0).to_bits()
    );
}

#[test]
fn finish_contrast_comes_from_the_spot_grid() {
    let (acc, n) = mixed_accumulator();
    assert_eq!(
        acc.finish(n).contrast.to_bits(),
        pattern_contrast(&acc.spots().blue).to_bits()
    );
}
