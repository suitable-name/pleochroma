//! Tests of `geom::side`.

use glam::DVec3;

use super::{
    side::{Side, cell_contains, cell_side, side},
    tests_polygon::Rng,
};

#[test]
fn side_classifies_with_the_tolerance() {
    let n = DVec3::Z;
    let eps = 1e-3;
    assert_eq!(side(n, 2.0, DVec3::new(5.0, 7.0, 1.0), eps), Side::Inside);
    assert_eq!(side(n, 2.0, DVec3::new(5.0, 7.0, 3.0), eps), Side::Outside);
    assert_eq!(side(n, 2.0, DVec3::new(5.0, 7.0, 2.0), eps), Side::On);
    assert_eq!(
        side(n, 2.0, DVec3::new(0.0, 0.0, 2.0 + 5e-4), eps),
        Side::On
    );
    assert_eq!(
        side(n, 2.0, DVec3::new(0.0, 0.0, 2.0 - 5e-4), eps),
        Side::On
    );
    // The thresholds themselves: Inside is `<= -eps`, Outside is `>= eps`.
    assert_eq!(side(n, 0.0, DVec3::new(0.0, 0.0, -0.5), 0.5), Side::Inside);
    assert_eq!(side(n, 0.0, DVec3::new(0.0, 0.0, 0.5), 0.5), Side::Outside);
}

#[test]
fn side_is_permutation_consistent_and_antisymmetric() {
    let mut rng = Rng::new(21);
    let n = rng.unit_vec3();
    let m = 0.25;
    let points: Vec<DVec3> = (0..300)
        .map(|_| rng.unit_vec3() * rng.range(0.0, 2.0))
        .collect();
    let forward: Vec<Side> = points.iter().map(|&p| side(n, m, p, 1e-3)).collect();
    // Classifying in any order gives the same class per point.
    let mut order: Vec<usize> = (0..points.len()).collect();
    for i in (1..order.len()).rev() {
        let j = rng.index(i + 1);
        order.swap(i, j);
    }
    for &i in &order {
        assert_eq!(side(n, m, points[i], 1e-3), forward[i]);
    }
    // The negated plane swaps inside and outside and keeps `On`.
    for (&p, &class) in points.iter().zip(&forward) {
        let flipped = match class {
            Side::Inside => Side::Outside,
            Side::Outside => Side::Inside,
            Side::On => Side::On,
        };
        assert_eq!(side(-n, -m, p, 1e-3), flipped);
    }
    assert!(forward.contains(&Side::Inside));
    assert!(forward.contains(&Side::Outside));
}

#[test]
fn cell_side_is_half_open() {
    let eps = 1e-9;
    // Upper edge: On counts as outside the cell.
    assert_eq!(cell_side(0.5, 1.0, eps), Side::Inside);
    assert_eq!(cell_side(1.0, 1.0, eps), Side::On);
    assert_eq!(cell_side(1.0 + 1e-10, 1.0, eps), Side::On);
    assert_eq!(cell_side(1.5, 1.0, eps), Side::Outside);
    assert!(cell_contains(0.0, 0.0, 1.0, eps));
    assert!(!cell_contains(1.0, 0.0, 1.0, eps));
    assert!(!cell_contains(1.0 - 1e-10, 0.0, 1.0, eps));
    assert!(cell_contains(-1e-10, 0.0, 1.0, eps));
    assert!(!cell_contains(-1e-6, 0.0, 1.0, eps));
    assert!(cell_contains(0.5, 0.0, 1.0, eps));
    assert!(!cell_contains(1.5, 0.0, 1.0, eps));
}

#[test]
fn adjacent_cells_partition_the_line() {
    let eps = 1e-9;
    let mut rng = Rng::new(22);
    let count = |x: f64| {
        (-4..4)
            .filter(|&i| cell_contains(x, f64::from(i), f64::from(i + 1), eps))
            .count()
    };
    for _ in 0..2000 {
        assert_eq!(count(rng.range(-3.9, 3.9)), 1);
    }
    // On and next to every integer boundary exactly one cell owns the coordinate.
    for k in -3..=3 {
        for offset in [-5e-10, -1e-12, 0.0, 1e-12, 5e-10] {
            assert_eq!(count(f64::from(k) + offset), 1, "k {k} offset {offset}");
        }
    }
}
