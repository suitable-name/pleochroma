//! Spectral intervals, interval splits and kept regions.

use std::sync::atomic::AtomicBool;

use glam::DVec2;

use super::{
    BeamSettings, RegionMode,
    beam::{BranchPrefix, Classifier, IntervalSplit, Leaf, SpectralInterval, SpectralNode},
    frame::BeamFrame,
    geom::polygon::ConvexPolygon,
    leaf::ReflectionKit,
    primary::TileRect,
    retrace::RetraceKit,
    snap::FacetClassifier,
    spectral::{
        BranchFollower, indices_constant_at, initial_intervals, shrink_leaf, strictly_constant,
    },
    stats::BeamStats,
    tests_support::{BeamRig, MAX_BOUNCES, cube_planes, diamond, glass, sapphire},
};
use crate::optics::{dispersion::DispersionModel, materials::GemMaterial, raytracer::Camera};

/// Samples per bin of the dense coverage check (guide §9.2).
const LAMBDAS_PER_BIN: usize = 8;
/// Points closer than this many pixels to an edge of a kept region are not checked (`ε_chord`
/// scale; the boundaries are exact only to the chord tolerance).
const EDGE_MARGIN_PX: f64 = 8.0e-3;
/// The share of violating samples tolerated (per mille): other stages of the tree have their own
/// tolerances, the measured count is printed.
const VIOLATION_PER_MILLE: usize = 1;
/// The leaves checked at most by the dense test.
const MAX_LEAVES: usize = 400;

/// The side of the frames of the brilliant rows in pixels (one tile).
const SMALL_FRAME: u32 = 16;

/// The settings of the brilliant rows: 16 spectral nodes instead of 48, because a strongly
/// dispersive stone builds one tree per node.
fn small_settings() -> BeamSettings {
    BeamSettings {
        spectral_nodes: 16,
        ..BeamSettings::default()
    }
}

/// An isotropic dispersive material below the dispersion gate (6 intervals of 8).
fn mild() -> GemMaterial {
    GemMaterial::new_custom("beam test mild", 1.768, 0.015, 0.0, [0.0; 3])
}

fn leaves_of(rig: &BeamRig, size: u32, settings: &BeamSettings) -> (Vec<Leaf>, u32) {
    let scene = rig.scene(size, MAX_BOUNCES);
    let frame = BeamFrame::new(&scene, settings).expect("eligible");
    let cancel = AtomicBool::new(false);
    let (leaves, _, _) = frame.tile_leaves(0, size, &cancel).expect("tile");
    (leaves, frame.n_lambda())
}

#[test]
fn the_initial_intervals_follow_the_material_class() {
    let glass = initial_intervals(&glass(), 48);
    assert_eq!(glass, vec![SpectralInterval::all(48)]);
    let mild = initial_intervals(&sapphire(), 48);
    assert_eq!(mild.len(), 6);
    assert!(mild.iter().all(|i| i.len() == 8));
    assert_eq!(mild[0], SpectralInterval { first: 0, last: 7 });
    assert_eq!(
        mild[5],
        SpectralInterval {
            first: 40,
            last: 47
        }
    );
    let strong = initial_intervals(&diamond(), 48);
    assert_eq!(strong.len(), 48);
    assert!(strong.iter().all(|i| i.len() == 1));
    assert!(
        strong
            .iter()
            .enumerate()
            .all(|(k, i)| usize::from(i.first) == k)
    );
}

#[test]
fn only_a_bitwise_constant_index_is_strictly_non_dispersive() {
    assert!(strictly_constant(&glass(), 48));
    assert!(!strictly_constant(&diamond(), 48));
    assert!(!strictly_constant(&mild(), 48));
}

/// A Cauchy curve with the given dispersion term.
const fn cauchy(a: f32, b: f32) -> DispersionModel {
    DispersionModel::Cauchy { a, b, c: 0.0 }
}

/// A custom biaxial material with a constant `n_beta` and the given `n_alpha` and `n_gamma` curves.
fn biaxial_with(alpha: DispersionModel, gamma: DispersionModel) -> GemMaterial {
    let mut material = glass();
    material.biaxial_delta_beta_alpha = Some(0.004);
    material.biaxial_principal_dispersion = Some([alpha, gamma]);
    material
}

#[test]
fn a_dispersion_on_any_independent_principal_axis_is_not_strictly_constant() {
    // Control: constant `n_beta`, constant `n_alpha` and `n_gamma` curves.
    let flat = biaxial_with(cauchy(1.59, 0.0), cauchy(1.61, 0.0));
    assert!(strictly_constant(&flat, 48));
    // Constant `n_beta`, varying `n_alpha`, constant `n_gamma`; and the other way round.
    let alpha = biaxial_with(cauchy(1.59, 0.01), cauchy(1.61, 0.0));
    let gamma = biaxial_with(cauchy(1.59, 0.0), cauchy(1.61, 0.01));
    assert!(!strictly_constant(&alpha, 48));
    assert!(!strictly_constant(&gamma, 48));
    assert!(!indices_constant_at(&alpha, [450.0, 650.0]));
    // A uniaxial material with a constant ordinary index and a dispersive extraordinary curve.
    let mut uniaxial = glass();
    uniaxial.uniaxial_extraordinary_dispersion = Some(cauchy(1.62, 0.01));
    assert!(!strictly_constant(&uniaxial, 48));
    assert!(!indices_constant_at(&uniaxial, [450.0, 650.0]));
    // The one-wavelength set is always constant.
    assert!(indices_constant_at(&uniaxial, [500.0]));
    // None of the dispersive ones starts as one all-node interval.
    for material in [&alpha, &gamma, &uniaxial] {
        assert!(initial_intervals(material, 48).len() > 1);
    }
}

#[test]
#[ignore = "over 60 s in the retest (2026-11-09); run on request: scripts/run_beam_tests.sh step 03"]
fn a_transmitted_leaf_whose_centroid_leaves_its_branch_is_not_kept_whole() {
    let rig = BeamRig::brilliant(diamond());
    let settings = small_settings();
    let (leaves, n_lambda) = leaves_of(&rig, SMALL_FRAME, &settings);
    let leaf = leaves
        .iter()
        .find(|l| !l.reflection)
        .expect("a transmitted leaf")
        .clone();
    let scene = rig.scene(SMALL_FRAME, MAX_BOUNCES);
    let frame = BeamFrame::new(&scene, &settings).expect("eligible");
    let reflection = ReflectionKit::new(
        &rig.material,
        &rig.planes,
        &rig.camera,
        (SMALL_FRAME, SMALL_FRAME),
        &scene.environment,
        n_lambda,
    );
    let retrace = RetraceKit {
        camera: &rig.camera,
        size: (SMALL_FRAME, SMALL_FRAME),
        stone: frame.stone(),
        material: &rig.material,
        n_lambda,
    };
    let tile = TileRect {
        x0: 0,
        y0: 0,
        x1: SMALL_FRAME,
        y1: SMALL_FRAME,
    };
    let kit = reflection.interact_kit(retrace, &settings, tile, MAX_BOUNCES);
    // The same leaf moved far outside the stone's image: its centroid cannot follow the branch.
    let far = [
        DVec2::new(1.0e4, 1.0e4),
        DVec2::new(1.0e4 + 1.0, 1.0e4),
        DVec2::new(1.0e4, 1.0e4 + 1.0),
    ];
    let mut lost = leaf;
    lost.image = ConvexPolygon::from_image_points(&far).expect("a triangle");
    let mut stats = BeamStats::default();
    let verdict = shrink_leaf(&lost, &kit, settings.region_mode, &mut stats);
    assert!(
        matches!(verdict, Err(IntervalSplit::Topology)),
        "a transmitted leaf off its branch must split or drop, not succeed"
    );
    // A reflection keeps its early return.
    lost.reflection = true;
    let verdict = shrink_leaf(&lost, &kit, settings.region_mode, &mut stats);
    assert!(matches!(verdict, Ok(ref parts) if parts.len() == 1));
}

#[test]
fn the_initial_intervals_and_their_halves_are_deterministic() {
    for material in [glass(), mild(), diamond()] {
        assert_eq!(
            initial_intervals(&material, 48),
            initial_intervals(&material, 48)
        );
    }
    let interval = SpectralInterval { first: 0, last: 7 };
    let (a, b) = interval.halves().expect("splits");
    assert_eq!(interval.halves(), Some((a, b)));
    assert_eq!(a, SpectralInterval { first: 0, last: 3 });
    assert_eq!(b, SpectralInterval { first: 4, last: 7 });
    assert_eq!(SpectralInterval::single(5).halves(), None);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_grazing_entry_splits_its_interval_and_the_render_is_deterministic() {
    let rig = BeamRig::new(cube_planes(), mild(), Camera::new(0.0, 0.15, 8.0, 24.0));
    let scene = rig.scene(64, MAX_BOUNCES);
    let frame = BeamFrame::new(&scene, &BeamSettings::default()).expect("eligible");
    let cancel = AtomicBool::new(false);
    let first = frame
        .render_beams_with_threads(64, 1, &cancel)
        .expect("image");
    let second = frame
        .render_beams_with_threads(64, 1, &cancel)
        .expect("image");
    eprintln!(
        "interval splits by cause: {:?}",
        first.stats.interval_splits
    );
    assert!(
        first.stats.interval_splits.iter().sum::<u64>() > 0,
        "a grazing entry must split an 8-node interval"
    );
    assert_eq!(first, second);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn a_non_dispersive_stone_never_splits() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(32, MAX_BOUNCES);
    let frame = BeamFrame::new(&scene, &BeamSettings::default()).expect("eligible");
    let cancel = AtomicBool::new(false);
    let image = frame
        .render_beams_with_threads(32, 1, &cancel)
        .expect("image");
    assert_eq!(image.stats.interval_splits, [0; 4]);
    assert_eq!(image.stats.residual_band_area.to_bits(), 0.0_f64.to_bits());
}

/// Random points of a convex polygon, by a fixed generator.
fn interior_points(points: &[DVec2], count: usize, seed: &mut u64) -> Vec<DVec2> {
    let mut next = || {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (*seed >> 11) as f64 / (1_u64 << 53) as f64
    };
    let mut out = Vec::new();
    for _ in 0..count {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the product is a non-negative float below the fan count, floored on purpose"
        )]
        let fan = 1 + (next() * (points.len() - 2) as f64) as usize % (points.len() - 2);
        let (mut wu, mut wv) = (next(), next());
        if wu + wv > 1.0 {
            wu = 1.0 - wu;
            wv = 1.0 - wv;
        }
        let (apex, second, third) = (points[0], points[fan], points[fan + 1]);
        out.push(apex + (second - apex) * wu + (third - apex) * wv);
    }
    out
}

/// The distance of `q` from the nearest edge line of the counter-clockwise polygon.
fn edge_distance(points: &[DVec2], q: DVec2) -> f64 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            (b - a).perp_dot(q - a) / a.distance(b)
        })
        .fold(f64::INFINITY, f64::min)
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn kept_regions_lie_inside_the_exact_region_of_every_wavelength_of_the_bin() {
    // A diamond disperses strongly, so every node is its own tree: the 16 pixel frame and 16
    // nodes (a rate check, `MAX_LEAVES` and the per-mille bound do not depend on the size).
    let rig = BeamRig::brilliant(diamond());
    let settings = small_settings();
    let (leaves, n_lambda) = leaves_of(&rig, SMALL_FRAME, &settings);
    let scene = rig.scene(SMALL_FRAME, MAX_BOUNCES);
    let frame = BeamFrame::new(&scene, &settings).expect("eligible");
    let kit = RetraceKit {
        camera: &rig.camera,
        size: (SMALL_FRAME, SMALL_FRAME),
        stone: frame.stone(),
        material: &rig.material,
        n_lambda,
    };
    let (mut samples, mut violations) = (0_usize, 0_usize);
    // Violations by cause: the follower leaves the branch, the classifier finds no facet, the
    // classifier finds another exit facet (a sample can have several).
    let mut causes = [0_usize; 3];
    let mut seed = 0x5EED_u64;
    for leaf in leaves.iter().filter(|l| !l.reflection).take(MAX_LEAVES) {
        let (pts, n) = leaf.image.points_2d();
        let outline = &pts[..n];
        let follower = BranchFollower::new(kit, leaf.prefix);
        let steps = leaf.prefix.steps();
        let mut shorter = BranchPrefix::EMPTY;
        for &w in &steps[..steps.len() - 1] {
            shorter = shorter.push(w).expect("room");
        }
        let exit = kit.stone.facet_of_original(steps[steps.len() - 1] >> 8);
        for q in interior_points(outline, 6, &mut seed) {
            if edge_distance(outline, q) < EDGE_MARGIN_PX {
                continue;
            }
            for node in leaf.spectrum.first..=leaf.spectrum.last {
                let (lo, hi) = SpectralNode(node).bin_nm(n_lambda);
                for i in 0..LAMBDAS_PER_BIN {
                    let lambda = lo + (hi - lo) * (i as f32 + 0.5) / LAMBDAS_PER_BIN as f32;
                    samples += 1;
                    let class = FacetClassifier::new(kit, shorter, lambda).classify(q);
                    let off_branch = !follower.follows(q, lambda);
                    let no_class = class.is_none();
                    let other_exit = !no_class && class != exit;
                    causes[0] += usize::from(off_branch);
                    causes[1] += usize::from(no_class);
                    causes[2] += usize::from(other_exit);
                    violations += usize::from(off_branch || no_class || other_exit);
                }
            }
        }
    }
    eprintln!("kept-region coverage: {violations} violations in {samples} samples");
    assert!(samples > 0, "no samples from {} leaves", leaves.len());
    assert!(
        violations * 1000 <= samples * VIOLATION_PER_MILLE,
        "{violations} of {samples} samples follow another branch (off the branch {}, no facet {}, \
         another exit facet {}; {} leaves)",
        causes[0],
        causes[1],
        causes[2],
        leaves.len()
    );
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn per_node_regions_cover_at_least_what_the_end_intersection_covers() {
    let rig = BeamRig::brilliant(mild());
    let area = |mode: RegionMode| {
        let settings = BeamSettings {
            region_mode: mode,
            ..small_settings()
        };
        let (leaves, _) = leaves_of(&rig, SMALL_FRAME, &settings);
        leaves
            .iter()
            .map(|l| l.image.area() * l.spectrum.len() as f64)
            .sum::<f64>()
    };
    let (end, per_node) = (area(RegionMode::EndIntersection), area(RegionMode::PerNode));
    eprintln!("kept node-area: end intersection {end}, per node {per_node}");
    assert!(per_node >= 0.99 * end);
}
