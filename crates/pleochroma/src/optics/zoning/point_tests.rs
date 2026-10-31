//! Tests for `ZoneKernel::zone_index_at` and its f32 twin (written, not run, by lane CP-C1).
//!
//! Property tests use a fixed seed (a splitmix64 generator, no dependency).

use super::{
    MAX_ZONES, Zone, ZoneAbsorption, ZoneFrame, ZoneKernel, ZoneKernelF32, ZoneShape,
    ZonedAbsorption,
};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    materials::ametrine_zoning,
};
use glam::{DQuat, DVec3, Vec3};
use std::f64::consts::FRAC_PI_2;

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn point(&mut self, half: f64) -> DVec3 {
        DVec3::new(
            (2.0 * half).mul_add(self.unit(), -half),
            (2.0 * half).mul_add(self.unit(), -half),
            (2.0 * half).mul_add(self.unit(), -half),
        )
    }

    fn direction(&mut self) -> DVec3 {
        loop {
            let v = self.point(1.0);
            let l = v.length();
            if l > 0.1 && l <= 1.0 {
                return v / l;
            }
        }
    }
}

fn unit(x: f64, y: f64, z: f64) -> DVec3 {
    DVec3::new(x, y, z).normalize()
}

fn absorb(peak: f32) -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
        550.0, 1.0e6, peak,
    )]))
}

fn test_frame() -> ZoneFrame {
    ZoneFrame {
        rotation: DQuat::from_axis_angle(unit(1.0, 2.0, 3.0), 0.7),
        translation: DVec3::new(0.3, -0.2, 0.5),
    }
}

/// A base plus the given shapes in order, with `frame` and `softness`.
fn build(shapes: Vec<ZoneShape>, frame: ZoneFrame, softness: f32) -> ZonedAbsorption {
    let mut z = ZonedAbsorption::new(absorb(0.1));
    z.frame = frame;
    z.boundary_softness_mm = softness;
    for (i, shape) in shapes.into_iter().enumerate() {
        z.zones.push(Zone {
            shape,
            absorption: absorb(0.2 * (i + 1) as f32),
        });
    }
    assert_eq!(z.validate(), Ok(()), "fixture must validate");
    z
}

fn cube_shape(centre: DVec3, half: f64) -> ZoneShape {
    let mut vertices = Vec::new();
    for (sx, sy, sz) in [
        (-1.0, -1.0, -1.0),
        (1.0, -1.0, -1.0),
        (1.0, 1.0, -1.0),
        (-1.0, 1.0, -1.0),
        (-1.0, -1.0, 1.0),
        (1.0, -1.0, 1.0),
        (1.0, 1.0, 1.0),
        (-1.0, 1.0, 1.0),
    ] {
        vertices.push([
            f64::mul_add(sx, half, centre.x),
            f64::mul_add(sy, half, centre.y),
            f64::mul_add(sz, half, centre.z),
        ]);
    }
    let triangles = vec![
        [0, 3, 2],
        [0, 2, 1],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [3, 7, 6],
        [3, 6, 2],
        [0, 4, 7],
        [0, 7, 3],
        [1, 2, 6],
        [1, 6, 5],
    ];
    ZoneShape::MeshShell {
        vertices,
        triangles,
    }
}

fn analytic_shapes() -> Vec<(&'static str, Vec<ZoneShape>)> {
    vec![
        (
            "half space",
            vec![ZoneShape::HalfSpace {
                normal: unit(0.3, 0.5, 0.8),
                offset: 0.2,
            }],
        ),
        (
            "slab",
            vec![ZoneShape::Slab {
                normal: unit(-0.6, 0.2, 0.7),
                offset_min: -0.4,
                offset_max: 0.9,
            }],
        ),
        (
            "tube",
            vec![ZoneShape::CoaxialCylinder {
                axis_point: DVec3::new(0.3, 0.1, -0.2),
                axis_dir: unit(0.2, -0.5, 0.9),
                r_in: 0.5,
                r_out: 1.4,
            }],
        ),
        (
            "prism 3",
            vec![ZoneShape::CoaxialPrism {
                axis_point: DVec3::new(0.1, -0.2, 0.0),
                axis_dir: unit(-0.4, 0.3, 0.8),
                n_sides: 3,
                r_in: 0.0,
                r_out: 1.0,
                phase: 0.4,
            }],
        ),
        (
            "prism 6 tube",
            vec![ZoneShape::CoaxialPrism {
                axis_point: DVec3::ZERO,
                axis_dir: unit(0.7, 0.1, -0.3),
                n_sides: 6,
                r_in: 0.5,
                r_out: 1.5,
                phase: 0.1,
            }],
        ),
        (
            "sector narrow",
            vec![ZoneShape::Sector {
                axis_point: DVec3::ZERO,
                axis_dir: unit(0.1, 0.9, 0.4),
                angle_from: 0.3,
                angle_to: 1.3,
            }],
        ),
        (
            "sector wide",
            vec![ZoneShape::Sector {
                axis_point: DVec3::new(0.2, 0.0, 0.1),
                axis_dir: unit(0.5, -0.4, 0.6),
                angle_from: -1.0,
                angle_to: 3.2,
            }],
        ),
        (
            "three overlapping",
            vec![
                ZoneShape::HalfSpace {
                    normal: unit(0.1, 0.2, 1.0),
                    offset: -0.3,
                },
                ZoneShape::CoaxialCylinder {
                    axis_point: DVec3::ZERO,
                    axis_dir: unit(1.0, 0.3, 0.2),
                    r_in: 0.0,
                    r_out: 1.2,
                },
                ZoneShape::Slab {
                    normal: unit(0.0, 1.0, 0.2),
                    offset_min: -0.5,
                    offset_max: 0.6,
                },
            ],
        ),
    ]
}

/// The index of the largest entry of `lengths[..=zones]` (the first on a tie).
fn argmax(lengths: &[f64; MAX_ZONES + 1]) -> usize {
    let mut best = 0;
    for (i, &l) in lengths.iter().enumerate() {
        if l > lengths[best] {
            best = i;
        }
    }
    best
}

/// For random points, the index equals the zone the whole of a very short segment lands in.
/// Returns how many samples were decisive (the segment stayed in one zone).
fn check_against_lengths(name: &str, z: &ZonedAbsorption, samples: usize, seed: u64) -> usize {
    const SEGMENT: f64 = 1.0e-5;
    let kernel = ZoneKernel::new(z);
    let mut rng = Rng(seed);
    let mut decisive = 0;
    for _ in 0..samples {
        let p = rng.point(2.0);
        let to = p + rng.direction() * SEGMENT;
        let lengths = kernel.lengths(p, to);
        let k = argmax(&lengths);
        if lengths[k] < SEGMENT * (1.0 - 1.0e-6) {
            continue; // the segment crosses a boundary
        }
        decisive += 1;
        assert_eq!(
            kernel.zone_index_at(p),
            k,
            "{name}: point {p:?} lengths {lengths:?}"
        );
    }
    decisive
}

#[test]
fn index_agrees_with_lengths_on_a_short_segment_inside_one_zone() {
    for (name, shapes) in analytic_shapes() {
        for frame in [ZoneFrame::IDENTITY, test_frame()] {
            let z = build(shapes.clone(), frame, 0.0);
            let decisive = check_against_lengths(name, &z, 400, 7);
            assert!(decisive >= 360, "{name}: only {decisive} decisive samples");
        }
    }
}

#[test]
fn every_shape_kind_has_hand_checked_points() {
    let k = |shape: ZoneShape| ZoneKernel::new(&build(vec![shape], ZoneFrame::IDENTITY, 0.0));
    let at = |x: f64, y: f64, z: f64| DVec3::new(x, y, z);

    let half = k(ZoneShape::HalfSpace {
        normal: DVec3::X,
        offset: 0.5,
    });
    assert_eq!(half.zone_index_at(at(1.0, 3.0, -2.0)), 1);
    assert_eq!(half.zone_index_at(at(0.0, 0.0, 0.0)), 0);

    let slab = k(ZoneShape::Slab {
        normal: DVec3::Y,
        offset_min: -0.5,
        offset_max: 1.0,
    });
    assert_eq!(slab.zone_index_at(at(9.0, 0.0, 9.0)), 1);
    assert_eq!(slab.zone_index_at(at(0.0, 1.5, 0.0)), 0);
    assert_eq!(slab.zone_index_at(at(0.0, -0.6, 0.0)), 0);

    // A tube about Z: base inside the bore and outside the wall.
    let tube = k(ZoneShape::CoaxialCylinder {
        axis_point: DVec3::ZERO,
        axis_dir: DVec3::Z,
        r_in: 0.5,
        r_out: 1.0,
    });
    assert_eq!(tube.zone_index_at(at(0.2, 0.0, 5.0)), 0);
    assert_eq!(tube.zone_index_at(at(0.0, 0.7, -5.0)), 1);
    assert_eq!(tube.zone_index_at(at(2.0, 0.0, 0.0)), 0);

    // A solid triangular prism about Z, apothem 1: the side normal at angle 0 is +X.
    let prism = k(ZoneShape::CoaxialPrism {
        axis_point: DVec3::ZERO,
        axis_dir: DVec3::Z,
        n_sides: 3,
        r_in: 0.0,
        r_out: 1.0,
        phase: 0.0,
    });
    assert_eq!(prism.zone_index_at(at(0.0, 0.0, 3.0)), 1);
    assert_eq!(prism.zone_index_at(at(0.9, 0.0, 0.0)), 1);
    assert_eq!(prism.zone_index_at(at(1.1, 0.0, 0.0)), 0);
    // The corner side: a point at radius 1.1 towards a corner (angle 180 degrees) is inside.
    assert_eq!(prism.zone_index_at(at(-1.5, 0.0, 0.0)), 1);

    // A quarter wedge about Z from the reference direction (+X) towards +Y.
    let sector = k(ZoneShape::Sector {
        axis_point: DVec3::ZERO,
        axis_dir: DVec3::Z,
        angle_from: 0.0,
        angle_to: FRAC_PI_2,
    });
    assert_eq!(sector.zone_index_at(at(1.0, 1.0, 4.0)), 1);
    assert_eq!(sector.zone_index_at(at(-1.0, 1.0, 0.0)), 0);
    assert_eq!(sector.zone_index_at(at(1.0, -1.0, 0.0)), 0);
}

#[test]
fn later_zones_override_earlier_ones() {
    let z = build(
        vec![
            ZoneShape::HalfSpace {
                normal: DVec3::X,
                offset: 0.0,
            },
            ZoneShape::HalfSpace {
                normal: DVec3::X,
                offset: 1.0,
            },
        ],
        ZoneFrame::IDENTITY,
        0.0,
    );
    let k = ZoneKernel::new(&z);
    assert_eq!(k.zone_index_at(DVec3::new(-1.0, 0.0, 0.0)), 0);
    assert_eq!(k.zone_index_at(DVec3::new(0.5, 0.0, 0.0)), 1);
    assert_eq!(k.zone_index_at(DVec3::new(2.0, 0.0, 0.0)), 2);
}

#[test]
fn ametrine_half_space_splits_amethyst_and_citrine() {
    let z = ametrine_zoning();
    let k = ZoneKernel::new(&z);
    assert_eq!(k.zone_index_at(DVec3::new(-1.0, 0.3, 0.2)), 0, "amethyst");
    assert_eq!(k.zone_index_at(DVec3::new(1.0, -0.3, 2.0)), 1, "citrine");
    let k32 = ZoneKernelF32::new(&z).expect("no mesh shell");
    assert_eq!(k32.zone_index_at(Vec3::new(-1.0, 0.3, 0.2)), 0);
    assert_eq!(k32.zone_index_at(Vec3::new(1.0, -0.3, 2.0)), 1);
}

#[test]
fn soft_boundaries_pick_the_larger_blended_weight() {
    let z = build(
        vec![ZoneShape::HalfSpace {
            normal: DVec3::X,
            offset: 0.0,
        }],
        ZoneFrame::IDENTITY,
        0.2,
    );
    let k = ZoneKernel::new(&z);
    // Past the band on either side, and just inside it on either side of the plane.
    assert_eq!(k.zone_index_at(DVec3::new(0.5, 0.0, 0.0)), 1);
    assert_eq!(k.zone_index_at(DVec3::new(-0.5, 0.0, 0.0)), 0);
    assert_eq!(k.zone_index_at(DVec3::new(0.01, 0.0, 0.0)), 1);
    assert_eq!(k.zone_index_at(DVec3::new(-0.01, 0.0, 0.0)), 0);

    // Over a short segment the zone with the most length is the index, unless two zones tie.
    for (name, shapes) in analytic_shapes() {
        let z = build(shapes, test_frame(), 0.3);
        let kernel = ZoneKernel::new(&z);
        let mut rng = Rng(11);
        let mut checked = 0;
        for _ in 0..300 {
            let p = rng.point(2.0);
            let lengths = kernel.lengths(p, p + rng.direction() * 1.0e-5);
            let best = argmax(&lengths);
            let runner_up = lengths
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != best)
                .map(|(_, &l)| l)
                .fold(0.0, f64::max);
            if lengths[best] < 1.05 * runner_up {
                continue;
            }
            checked += 1;
            assert_eq!(kernel.zone_index_at(p), best, "{name}: {p:?} {lengths:?}");
        }
        assert!(checked >= 200, "{name}: only {checked} decisive samples");
    }
}

#[test]
fn mesh_cube_contains_by_crossing_parity() {
    let centre = DVec3::new(0.2, -0.1, 0.3);
    let z = build(vec![cube_shape(centre, 1.1)], ZoneFrame::IDENTITY, 0.0);
    let k = ZoneKernel::new(&z);
    assert_eq!(k.zone_index_at(centre), 1);
    assert_eq!(k.zone_index_at(centre + DVec3::new(1.0, 0.9, -1.0)), 1);
    assert_eq!(k.zone_index_at(centre + DVec3::new(1.3, 0.0, 0.0)), 0);
    assert_eq!(k.zone_index_at(centre + DVec3::new(0.0, -1.3, 0.0)), 0);
    assert_eq!(k.zone_index_at(DVec3::new(9.0, 9.0, 9.0)), 0);

    // A later analytic zone overrides the shell where they overlap.
    let z = build(
        vec![
            cube_shape(centre, 1.1),
            ZoneShape::CoaxialCylinder {
                axis_point: centre,
                axis_dir: DVec3::Z,
                r_in: 0.0,
                r_out: 0.5,
            },
        ],
        ZoneFrame::IDENTITY,
        0.0,
    );
    let k = ZoneKernel::new(&z);
    assert_eq!(k.zone_index_at(centre), 2);
    assert_eq!(k.zone_index_at(centre + DVec3::new(0.9, 0.0, 0.0)), 1);
    assert_eq!(k.zone_index_at(centre + DVec3::new(1.5, 0.0, 0.0)), 0);
}

#[test]
fn mesh_cube_index_agrees_with_lengths_under_a_frame() {
    let z = build(
        vec![cube_shape(DVec3::new(0.2, -0.1, 0.3), 1.1)],
        test_frame(),
        0.0,
    );
    let decisive = check_against_lengths("cube shell", &z, 400, 23);
    assert!(decisive >= 360, "only {decisive} decisive samples");
}

#[test]
fn f32_twin_matches_f64_away_from_boundaries() {
    for (name, shapes) in analytic_shapes() {
        for softness in [0.0, 0.3] {
            let z = build(shapes.clone(), test_frame(), softness);
            let k64 = ZoneKernel::new(&z);
            let k32 = ZoneKernelF32::new(&z).expect("no mesh shell");
            let mut rng = Rng(5);
            let mut differ = 0;
            for _ in 0..500 {
                let p = rng.point(2.0);
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "the f32 twin is queried at the nearest f32 point"
                )]
                let p32 = Vec3::new(p.x as f32, p.y as f32, p.z as f32);
                if k64.zone_index_at(p) != k32.zone_index_at(p32) {
                    differ += 1;
                }
            }
            // Only points within f32 rounding of a boundary may differ.
            assert!(
                differ <= 5,
                "{name} softness {softness}: {differ} of 500 differ"
            );
        }
    }
}

#[test]
fn no_zones_is_the_base() {
    let z = ZonedAbsorption::new(absorb(0.1));
    let k = ZoneKernel::new(&z);
    assert_eq!(k.zone_index_at(DVec3::new(1.0, 2.0, 3.0)), 0);
    let k32 = ZoneKernelF32::new(&z).expect("no mesh shell");
    assert_eq!(k32.zone_index_at(Vec3::new(1.0, 2.0, 3.0)), 0);
}
