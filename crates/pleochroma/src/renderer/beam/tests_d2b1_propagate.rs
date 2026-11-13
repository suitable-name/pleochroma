//! Partition, projection and state replay.

use glam::{DMat3, DVec2, DVec3, Vec3};

use super::{
    BeamSettings,
    beam::{Beam, BranchPrefix, PartitionPiece, SampleStates, SeamStore, SpectralInterval},
    geom::{
        Tolerances,
        homography::{PlaneBasis, apply},
        polygon::ConvexPolygon,
    },
    interact::{InteractKit, IntervalHeroes},
    primary::{TileRect, camera_state, primary_beams, primary_beams_counted},
    propagate::{
        VERTEX_SNAP_REL, max_model_distortion, partition, project_to, reset_model_distortion,
        retrace_state, snapped_polygon,
    },
    retrace::{RetraceKit, camera_direction, exact_ray},
    snap::snap_partition_with,
    stats::{BeamStats, TILE_ALLOWANCE},
    stone::BeamStone,
    tests_support::{BeamRig, MAX_BOUNCES, diamond, glass, sapphire},
};
use crate::{
    geometry::GpuFacetPlane,
    optics::{materials::GemMaterial, raytracer::transport::PreparedMaterial},
    renderer::path_cache::{
        bounce::{EventKind, ExplicitHit, PathState, expand_at},
        build::HeroContext,
        signature::BRANCH_SIG_SEED,
    },
};

/// The frame size and node count of the tests.
const SIZE: u32 = 64;
const N_LAMBDA: u32 = 48;
/// The tolerance of the conservation checks.
const CONSERVATION: f64 = 1e-9;
/// The plane-distance tolerance of the projection.
const PLANE_TOLERANCE: f64 = 1e-12;
/// The tolerance of the image mapping of a projected polygon.
const MAPPING_TOLERANCE: f64 = 1e-9;
/// The tolerance of f32 state comparisons.
const STATE_TOLERANCE: f32 = 1e-4;
/// The cube's original facet indices.
const PLUS_X: u32 = 0;
const MINUS_X: u32 = 1;
const TOP: u32 = 2;
const BOTTOM: u32 = 3;

/// Runs `f` with an interact kit over the rig built by `make` for `material`.
fn with_kit<R>(
    make: fn(GemMaterial) -> BeamRig,
    material: GemMaterial,
    f: impl FnOnce(&InteractKit<'_, '_>, &BeamRig) -> R,
) -> R {
    let rig = make(material);
    let stone = BeamStone::new(&rig.planes, false).expect("a stone");
    let prepared = PreparedMaterial::new(&rig.material);
    let full = SpectralInterval::all(N_LAMBDA);
    let heroes: Vec<HeroContext<'_>> = (0..full.groups())
        .map(|g| {
            let (ctx, cache) = prepared.ray_context(full.group_lambdas(g, N_LAMBDA), 3);
            HeroContext { ctx, cache }
        })
        .collect();
    let settings = BeamSettings::default();
    let kit = InteractKit {
        retrace: RetraceKit {
            camera: &rig.camera,
            size: (SIZE, SIZE),
            stone: &stone,
            material: &rig.material,
            n_lambda: N_LAMBDA,
        },
        planes: &rig.planes,
        prepared: &prepared,
        heroes: &heroes,
        settings: &settings,
        tile: TileRect {
            x0: 0,
            y0: 0,
            x1: SIZE,
            y1: SIZE,
        },
        tau_omega: 0.01,
        max_bounces: MAX_BOUNCES,
        subnodes: None,
        interval_heroes: IntervalHeroes::default(),
        cancel: None,
        allowance: TILE_ALLOWANCE,
    };
    f(&kit, &rig)
}

/// A beam with the given polygon, apex and no states (enough for the partition).
fn bare_beam(stone: &BeamStone, facet: u32, apex: DVec3, polygon: &ConvexPolygon) -> Beam {
    Beam {
        facet,
        polygon: *polygon,
        apex,
        image_to_plane: DMat3::IDENTITY,
        inside: true,
        sig: BRANCH_SIG_SEED,
        prefix: BranchPrefix::EMPTY,
        bounces: 0,
        spectrum: SpectralInterval::all(N_LAMBDA),
        state: SampleStates::default(),
    }
    .with_frame_of(stone)
}

impl Beam {
    /// Test helper: asserts the polygon lies in the stone facet's plane frame.
    fn with_frame_of(self, stone: &BeamStone) -> Self {
        let basis = PlaneBasis::new(stone.plane(self.facet));
        assert_eq!(*self.polygon.frame(), basis.frame());
        self
    }
}

/// A deterministic pseudo-random number in `[0, 1)`.
fn next_unit(seed: &mut u64) -> f64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    f64::from(u32::try_from(*seed >> 40).unwrap_or(0)) / f64::from(1_u32 << 24)
}

/// The polygon, its quadrisection and a few random cuts through it.
fn sub_polygons(poly: &ConvexPolygon, seed: &mut u64) -> Vec<ConvexPolygon> {
    let mut out = vec![*poly];
    out.extend(poly.quadrisect());
    let size = poly.area().sqrt();
    for _ in 0..3 {
        let angle = next_unit(seed) * std::f64::consts::TAU;
        let frame = poly.frame();
        let n = frame.u * angle.cos() + frame.v * angle.sin();
        let m = ((next_unit(seed) - 0.5) * 0.5).mul_add(size, n.dot(poly.centroid()));
        out.extend(poly.clip_capped(n, m, 0.0).0);
    }
    out.retain(|p| p.area() > 1e-9);
    out
}

/// The perimeter of `poly`, in carrier-plane units.
fn perimeter(poly: &ConvexPolygon) -> f64 {
    let pts = poly.vertices();
    pts.iter()
        .zip(pts.iter().cycle().skip(1))
        .map(|(a, b)| a.distance(*b))
        .sum()
}

/// The count of piece vertices outside `snapped` and the largest gap between near-duplicate piece
/// vertices, computed only for a failure message.
fn tiling_diagnostic(
    pieces: &[PartitionPiece],
    snapped: &ConvexPolygon,
    radius: f64,
) -> (usize, f64) {
    let verts: Vec<DVec3> = pieces
        .iter()
        .flat_map(|p| p.polygon.vertices().iter().copied())
        .collect();
    let outside = verts
        .iter()
        .filter(|v| !snapped.contains(**v, 1e-12 * radius))
        .count();
    let max_gap = verts
        .iter()
        .map(|v| {
            verts
                .iter()
                .filter(|w| *w != v)
                .map(|w| v.distance(*w))
                .fold(f64::INFINITY, f64::min)
        })
        .filter(|d| *d > 1e-13 * radius && *d < 1e-5 * radius)
        .fold(0.0, f64::max);
    (outside, max_gap)
}

fn conservation_on(make: fn(GemMaterial) -> BeamRig, material: GemMaterial) {
    let rig = make(material);
    let stone = BeamStone::new(&rig.planes, false).expect("a stone");
    let tile = TileRect {
        x0: 0,
        y0: 0,
        x1: SIZE,
        y1: SIZE,
    };
    let primaries = primary_beams(
        &stone,
        &rig.camera,
        (SIZE, SIZE),
        tile,
        SpectralInterval::all(N_LAMBDA),
    );
    assert_ne!(primaries.len(), 0);
    let mut seed = 12345_u64;
    let mut checked = 0;
    for primary in &primaries {
        for polygon in sub_polygons(&primary.polygon, &mut seed) {
            let beam = bare_beam(&stone, primary.facet, primary.apex, &polygon);
            let result = partition(&beam, &stone, &Tolerances::DEFAULT);
            let sum: f64 =
                result.pieces.iter().map(|p| p.polygon.area()).sum::<f64>() + result.sliver_area;
            // The pieces tile the beam polygon with its corners snapped onto the apex rays through
            // the carrier's vertices, not the stored polygon. The snap moves a corner by at most
            // `VERTEX_SNAP_REL * R`, so the areas differ by that times the perimeter per corner.
            let snapped = snapped_polygon(&beam, &stone);
            let area = snapped.area();
            let stored = polygon.area();
            let snap_budget = VERTEX_SNAP_REL
                * stone.bounding_radius()
                * perimeter(&polygon)
                * polygon.len() as f64;
            assert!(
                (stored - area).abs() <= snap_budget,
                "facet {}: the corner snap moved the area by {:e} (budget {snap_budget:e})",
                primary.facet,
                stored - area
            );
            let lost = area - sum;
            let (normal, offset) = stone.plane(primary.facet);
            let ring_offset = stone
                .ring(primary.facet)
                .iter()
                .map(|&id| (normal.dot(stone.vertex(id)) - offset).abs())
                .fold(0.0, f64::max);
            assert!(
                lost.abs() <= CONSERVATION * area,
                "facet {}: {sum} vs {area}: lost {lost:e} ({:e} relative); {} pieces on facets {:?} \
                 with {:e} sliver area; conserved {}; bounding radius {}, side epsilon {:e}, \
                 beam polygon of {} vertices; the facet's ring vertices lie up to {ring_offset:e} \
                 off its plane; stored area {stored}; piece vertices outside the snapped parent / \
                 largest near-duplicate gap: {:?}; pieces (facet, area, vertices): {:?}",
                primary.facet,
                lost / area,
                result.pieces.len(),
                result.pieces.iter().map(|p| p.to).collect::<Vec<_>>(),
                result.sliver_area,
                result.conserved,
                stone.bounding_radius(),
                Tolerances::DEFAULT.side_eps_rel * stone.bounding_radius(),
                polygon.points_2d().1,
                tiling_diagnostic(&result.pieces, &snapped, stone.bounding_radius()),
                result
                    .pieces
                    .iter()
                    .map(|p| (p.to, p.polygon.area(), p.polygon.len()))
                    .collect::<Vec<_>>(),
            );
            assert!(
                result.conserved,
                "facet {}: the partition reports itself not conserved ({sum} vs {area})",
                primary.facet
            );
            assert_ne!(result.pieces.len(), 0);
            assert!(result.pieces.iter().all(|p| p.to != primary.facet));
            checked += 1;
        }
    }
    eprintln!("partition conservation: {checked} beams");
}

#[test]
fn the_partition_conserves_area_on_the_cube() {
    conservation_on(BeamRig::cube, glass());
}

#[test]
fn the_partition_conserves_area_on_the_brilliant() {
    conservation_on(BeamRig::brilliant, diamond());
}

#[test]
fn the_partition_conserves_area_on_the_pentalume() {
    conservation_on(BeamRig::pentalume, sapphire());
}

/// A strip beam on the cube's top whose rays leave through -X, the bottom and +X.
fn strip_beam(stone: &BeamStone) -> Beam {
    let top = stone.facet_of_original(TOP).expect("top");
    let basis = PlaneBasis::new(stone.plane(top));
    let points = [
        DVec3::new(-0.5, 1.0, -0.01),
        DVec3::new(1.0, 1.0, -0.01),
        DVec3::new(1.0, 1.0, 0.01),
        DVec3::new(-0.5, 1.0, 0.01),
    ];
    let polygon = ConvexPolygon::with_frame(&points, basis.frame()).expect("strip");
    bare_beam(stone, top, DVec3::new(0.0, 1.05, 0.0), &polygon)
}

#[test]
fn a_sliver_on_the_way_does_not_hide_the_facet_behind_it() {
    let rig = BeamRig::cube(glass());
    let stone = BeamStone::new(&rig.planes, false).expect("a stone");
    let beam = strip_beam(&stone);
    let plus_x = stone.facet_of_original(PLUS_X).expect("+x");
    let minus_x = stone.facet_of_original(MINUS_X).expect("-x");
    let bottom = stone.facet_of_original(BOTTOM).expect("bottom");
    // Without a sliver threshold the three pieces exist.
    let open = Tolerances {
        sliver_area_px2: 0.0,
        ..Tolerances::DEFAULT
    };
    let all = partition(&beam, &stone, &open);
    let facets: Vec<u32> = all.pieces.iter().map(|p| p.to).collect();
    assert!(facets.contains(&plus_x) && facets.contains(&minus_x) && facets.contains(&bottom));
    // With a threshold above the bottom strip (about 1e-3) only, the bottom is dropped as a sliver
    // but -X, reachable from the seed +X only through the bottom, is still found.
    let radius = stone.bounding_radius();
    let tight = Tolerances {
        sliver_area_px2: 5e-3 / (radius * radius),
        ..Tolerances::DEFAULT
    };
    let result = partition(&beam, &stone, &tight);
    let facets: Vec<u32> = result.pieces.iter().map(|p| p.to).collect();
    assert!(facets.contains(&plus_x), "{facets:?}");
    assert!(facets.contains(&minus_x), "{facets:?}");
    assert!(!facets.contains(&bottom), "{facets:?}");
    assert!(result.sliver_area > 0.0);
    assert!(result.conserved);
}

#[test]
fn the_partition_is_deterministic() {
    let rig = BeamRig::pentalume(sapphire());
    let stone = BeamStone::new(&rig.planes, false).expect("a stone");
    let tile = TileRect {
        x0: 0,
        y0: 0,
        x1: SIZE,
        y1: SIZE,
    };
    let primaries = primary_beams(
        &stone,
        &rig.camera,
        (SIZE, SIZE),
        tile,
        SpectralInterval::all(N_LAMBDA),
    );
    for primary in &primaries {
        let first = partition(primary, &stone, &Tolerances::DEFAULT);
        let second = partition(primary, &stone, &Tolerances::DEFAULT);
        assert_eq!(first, second);
    }
}

/// The point closest to the lines `points[i] + t dirs[i]` (least squares).
fn closest_point(points: &[DVec3], dirs: &[DVec3]) -> DVec3 {
    let mut m = DMat3::ZERO;
    let mut b = DVec3::ZERO;
    for (p, d) in points.iter().zip(dirs) {
        let proj = DMat3::IDENTITY - DMat3::from_cols(*d * d.x, *d * d.y, *d * d.z);
        m += proj;
        b += proj * *p;
    }
    m.inverse() * b
}

/// The prefix word `(facet << 8) | (code << 1) | is_ext`.
const fn word(facet: u32, kind: EventKind) -> u32 {
    let code = match kind {
        EventKind::Reflect | EventKind::TotalReflect => 1,
        EventKind::Transmit => 3,
    };
    (facet << 8) | (code << 1)
}

/// An entry beam on the cube's top: a small image patch at the frame centre, the exact refracted
/// rays and their least-squares apex.
fn entry_beam(kit: &InteractKit<'_, '_>, half: f64) -> Beam {
    let stone = kit.retrace.stone;
    let top = stone.facet_of_original(TOP).expect("top");
    let spectrum = SpectralInterval::all(N_LAMBDA);
    let c = f64::from(SIZE) * 0.5;
    let patch = ConvexPolygon::from_image_points(&[
        DVec2::new(c - half, c - half),
        DVec2::new(c + half, c - half),
        DVec2::new(c + half, c + half),
        DVec2::new(c - half, c + half),
    ])
    .expect("patch");
    let primary = super::primary::beam_for_image(
        stone,
        kit.retrace.camera,
        (SIZE, SIZE),
        top,
        &patch,
        spectrum,
    )
    .expect("primary");
    let prefix = BranchPrefix::EMPTY
        .push(word(TOP, EventKind::Transmit))
        .expect("prefix");
    let mut beam = Beam {
        inside: true,
        prefix,
        bounces: 1,
        ..primary
    };
    let (points, _) = patch.points_2d();
    let mut origins = Vec::new();
    let mut dirs = Vec::new();
    for q in &points[..4] {
        let ray = exact_ray(
            *q,
            super::beam::SpectralNode(spectrum.centre()),
            &prefix,
            &kit.retrace,
        )
        .expect("entry ray");
        origins.push(ray.origin);
        dirs.push(ray.dir);
    }
    beam.apex = closest_point(&origins, &dirs);
    beam
}

/// The pieces of the entry beam, projected.
fn projected_pieces(kit: &InteractKit<'_, '_>, beam: &Beam) -> Vec<(PartitionPiece, Beam)> {
    let stone = kit.retrace.stone;
    let result = partition(beam, stone, &Tolerances::DEFAULT);
    assert!(result.conserved);
    let mut stats = BeamStats::default();
    // As in production (`expand_beam`): the model boundaries are snapped onto the exact curves
    // before the pieces are projected; the projection check relies on it.
    let pieces = snap_partition_with(
        beam,
        &result,
        &kit.retrace,
        kit.tile,
        &mut SeamStore::default(),
        &mut stats,
    )
    .expect("the partition snaps");
    pieces
        .into_iter()
        .map(|piece| {
            let child = project_to(beam, &piece, kit, &mut stats).expect("projects");
            (piece, child)
        })
        .collect()
}

#[test]
fn project_to_maps_the_piece_onto_the_next_facet() {
    reset_model_distortion();
    with_kit(BeamRig::cube, glass(), |kit, _| {
        let beam = entry_beam(kit, 2.0);
        let stone = kit.retrace.stone;
        let pairs = projected_pieces(kit, &beam);
        assert_ne!(pairs.len(), 0);
        for (piece, child) in &pairs {
            assert_eq!(child.facet, piece.to);
            assert_eq!(child.apex, beam.apex);
            assert_eq!(child.prefix, beam.prefix);
            assert_eq!(child.sig, beam.sig);
            assert!(child.inside);
            let (n, m) = stone.plane(piece.to);
            let n = n.normalize();
            let m = m / stone.plane(piece.to).0.length();
            for v in child.polygon.vertices() {
                assert!((n.dot(*v) - m).abs() < PLANE_TOLERANCE, "off plane");
            }
            // Each vertex lies on the ray from the apex through a vertex of the piece.
            for v in child.polygon.vertices() {
                let along = (*v - beam.apex).normalize();
                let best = piece
                    .polygon
                    .vertices()
                    .iter()
                    .map(|p| (*p - beam.apex).normalize().distance(along))
                    .fold(f64::INFINITY, f64::min);
                assert!(best < 1e-9, "vertex not on a piece ray: {best}");
            }
            // The image mapping sends the states' image vertices onto the polygon.
            let basis = PlaneBasis::new(stone.plane(piece.to));
            for i in 0..child.state.vertices {
                let q = child.state.vertex(i).expect("vertex").image;
                let world = basis.to_world(apply(child.image_to_plane, q).expect("finite"));
                let near = child
                    .polygon
                    .vertices()
                    .iter()
                    .map(|p| p.distance(world))
                    .fold(f64::INFINITY, f64::min);
                assert!(near < MAPPING_TOLERANCE, "image vertex {i}: {near}");
            }
            assert_eq!(
                child.state.points.len(),
                2 * child.state.vertices + 1,
                "vertices, centroid, midpoints"
            );
        }
        let total: f64 = pairs.iter().map(|(_, c)| c.polygon.area()).sum();
        assert!(total > 0.0);
        // The model's position error of this test's own children stays within the budget
        // `2 · τ_dir · 2R`. The global statistic is only printed: other tests running in parallel
        // feed it too.
        let budget =
            2.0 * kit.settings.dir_tolerance_rad * 2.0 * kit.retrace.stone.bounding_radius();
        let mut worst = 0.0_f64;
        for (piece, child) in &pairs {
            let (n, m) = stone.plane(piece.to);
            let basis = PlaneBasis::new((n, m));
            for state in &child.state.points {
                let (o, d) = (state.ray.origin.as_dvec3(), state.ray.dir.as_dvec3());
                let exact = d.mul_add(DVec3::splat((m - n.dot(o)) / n.dot(d)), o);
                let model =
                    basis.to_world(apply(child.image_to_plane, state.image).expect("finite"));
                worst = worst.max(exact.distance(model));
            }
        }
        eprintln!(
            "model distortion: {worst:e} of a budget of {budget:e} (global maximum {:e})",
            max_model_distortion()
        );
        assert!(worst <= budget, "{worst:e} > {budget:e}");
    });
}

/// At the default `τ_dir`, a piece labelled with a facet its rays do not leave through is refused
/// whatever the slack. The model's position budget (`0.04 R` at the default) does not disable the
/// topology check.
#[test]
fn a_piece_with_the_wrong_exit_facet_is_refused_at_the_default_tolerance() {
    with_kit(BeamRig::cube, glass(), |kit, _| {
        assert!((kit.settings.dir_tolerance_rad - 1e-2).abs() < 1e-15);
        let stone = kit.retrace.stone;
        let beam = entry_beam(kit, 2.0);
        let pairs = projected_pieces(kit, &beam);
        assert_ne!(pairs.len(), 0);
        let mut refused = 0;
        for (piece, _) in &pairs {
            for wrong in 0..u32::try_from(stone.facet_count()).expect("count") {
                if wrong == piece.to || wrong == beam.facet {
                    continue;
                }
                let relabelled = PartitionPiece {
                    to: wrong,
                    polygon: piece.polygon,
                };
                let mut stats = BeamStats::default();
                assert!(
                    project_to(&beam, &relabelled, kit, &mut stats).is_none(),
                    "a piece for facet {} was accepted as facet {wrong}",
                    piece.to
                );
                assert_eq!(stats.bracket_failures, 1);
                refused += 1;
            }
        }
        assert_ne!(refused, 0);
    });
}

/// The entry children of every front facet, with the fitted apex production uses, partition into
/// pieces that tile their polygon. A camera apex lies on the rays of the primary corners and would
/// hide the strip a fitted apex loses.
fn fitted_apex_conservation_on(make: fn(GemMaterial) -> BeamRig, material: GemMaterial) {
    with_kit(make, material, |kit, rig| {
        let stone = kit.retrace.stone;
        let spectrum = SpectralInterval::all(N_LAMBDA);
        let tile = TileRect {
            x0: 0,
            y0: 0,
            x1: SIZE,
            y1: SIZE,
        };
        let primaries = primary_beams(stone, &rig.camera, (SIZE, SIZE), tile, spectrum);
        assert_ne!(primaries.len(), 0);
        let mut seed = 54321_u64;
        let mut checked = 0;
        for primary in &primaries {
            let prefix = BranchPrefix::EMPTY
                .push(word(
                    stone.original_facet(primary.facet),
                    EventKind::Transmit,
                ))
                .expect("prefix");
            let mut origins = Vec::new();
            let mut dirs = Vec::new();
            for i in 0..primary.state.vertices {
                let Some(state) = primary.state.vertex(i) else {
                    continue;
                };
                let node = super::beam::SpectralNode(spectrum.centre());
                if let Ok(ray) = exact_ray(state.image, node, &prefix, &kit.retrace) {
                    origins.push(ray.origin);
                    dirs.push(ray.dir);
                }
            }
            if origins.len() < 3 {
                continue;
            }
            let apex = closest_point(&origins, &dirs);
            let (normal, offset) = stone.plane(primary.facet);
            // The fitted apex of an entry child lies outside the stone, behind its facet.
            if !apex.is_finite() || normal.dot(apex) - offset <= 0.0 {
                continue;
            }
            for polygon in sub_polygons(&primary.polygon, &mut seed) {
                let beam = bare_beam(stone, primary.facet, apex, &polygon);
                let result = partition(&beam, stone, &Tolerances::DEFAULT);
                let sum: f64 = result.pieces.iter().map(|p| p.polygon.area()).sum::<f64>()
                    + result.sliver_area;
                // The pieces tile the beam polygon with its corners snapped onto the apex
                // rays through the carrier's vertices, not the stored polygon.
                let snapped = snapped_polygon(&beam, stone);
                let area = snapped.area();
                let stored = polygon.area();
                let snap_budget = VERTEX_SNAP_REL
                    * stone.bounding_radius()
                    * perimeter(&polygon)
                    * polygon.len() as f64;
                assert!(
                    (stored - area).abs() <= snap_budget,
                    "facet {}: the corner snap moved the area by {:e} (budget {snap_budget:e})",
                    primary.facet,
                    stored - area
                );
                assert!(
                    result.conserved && (sum - area).abs() <= CONSERVATION * area,
                    "facet {}: {sum} vs {area} ({:e} relative), conserved {}; stored area {stored}; \
                     piece vertices outside the snapped parent / largest near-duplicate gap: {:?}; \
                     pieces (facet, area, vertices): {:?}",
                    primary.facet,
                    (sum - area) / area,
                    result.conserved,
                    tiling_diagnostic(&result.pieces, &snapped, stone.bounding_radius()),
                    result
                        .pieces
                        .iter()
                        .map(|p| (p.to, p.polygon.area(), p.polygon.len()))
                        .collect::<Vec<_>>()
                );
                checked += 1;
            }
        }
        assert_ne!(
            checked, 0,
            "no entry child had a fitted apex behind its facet"
        );
        eprintln!("fitted-apex partition conservation: {checked} beams");
    });
}

#[test]
fn the_partition_conserves_area_with_a_fitted_apex_on_the_cube() {
    fitted_apex_conservation_on(BeamRig::cube, glass());
}

#[test]
fn the_partition_conserves_area_with_a_fitted_apex_on_the_brilliant() {
    fitted_apex_conservation_on(BeamRig::brilliant, diamond());
}

#[test]
fn the_partition_conserves_area_with_a_fitted_apex_on_the_pentalume() {
    fitted_apex_conservation_on(BeamRig::pentalume, sapphire());
}

/// The counted primaries equal the plain ones, and the cube leaves no silent gap.
#[test]
fn the_counted_primaries_equal_the_plain_ones_and_the_cube_has_no_gap() {
    let rig = BeamRig::cube(glass());
    let stone = BeamStone::new(&rig.planes, false).expect("a stone");
    let tile = TileRect {
        x0: 0,
        y0: 0,
        x1: SIZE,
        y1: SIZE,
    };
    let spectrum = SpectralInterval::all(N_LAMBDA);
    let plain = primary_beams(&stone, &rig.camera, (SIZE, SIZE), tile, spectrum);
    let mut stats = BeamStats::default();
    let counted = primary_beams_counted(
        &stone,
        &rig.camera,
        (SIZE, SIZE),
        tile,
        spectrum,
        &mut stats,
    );
    assert_eq!(plain, counted);
    assert_eq!(stats.silent_gaps, 0);
    assert_eq!(stats.silent_gap_area.to_bits(), 0);
}

/// The hero context of group 0, for hand replays.
fn hero0<'k, 'a>(kit: &'k InteractKit<'_, 'a>) -> &'k HeroContext<'a> {
    &kit.heroes[0]
}

/// The first transmit or reflect child, ordinary mode.
fn pick(
    children: &[crate::renderer::path_cache::bounce::Child],
    reflect: bool,
) -> crate::renderer::path_cache::bounce::Child {
    *children
        .iter()
        .find(|c| !c.state.is_ext && (c.kind != EventKind::Transmit) == reflect)
        .expect("child")
}

/// One hand-made bounce of `path` at the facet `original`, with `ray` as the incoming ray.
fn hand_bounce(
    kit: &InteractKit<'_, '_>,
    path: PathState,
    ray: crate::optics::raytracer::Ray,
    original: u32,
    prev: Option<Vec3>,
) -> crate::renderer::path_cache::bounce::Expansion {
    let (plane_normal, plane_offset) = kit.retrace.stone.plane(
        kit.retrace
            .stone
            .facet_of_original(original)
            .expect("facet"),
    );
    let origin = ray.origin.as_dvec3();
    let direction = ray.dir.as_dvec3();
    let t = (plane_offset - plane_normal.dot(origin)) / plane_normal.dot(direction);
    let point = (origin + direction * t).as_vec3();
    expand_at(
        hero0(kit),
        kit.planes,
        path,
        ray,
        ExplicitHit {
            point,
            facet: original as usize,
            normal: Vec3::from(kit.planes[original as usize].normal),
        },
        Some(!path.inside),
        prev,
    )
}

#[test]
fn retrace_state_equals_the_entry_state_at_a_vertex() {
    with_kit(BeamRig::cube, glass(), |kit, rig| {
        let spectrum = SpectralInterval::single(3);
        let mut beam = entry_beam(kit, 1.0);
        beam.spectrum = spectrum;
        let image = DVec2::new(30.25, 33.5);
        let state = retrace_state(image, &beam, kit).expect("state");
        let map =
            crate::renderer::beam::geom::homography::camera_direction_map(&rig.camera, SIZE, SIZE);
        let camera = camera_state(&rig.camera, map, image, spectrum);
        let path = PathState {
            k: camera.k,
            inside: false,
            is_ext: false,
            stokes: camera.stokes[0],
        };
        let expansion = hand_bounce(kit, path, camera.ray, TOP, None);
        let child = pick(&expansion.children, false);
        assert!(state.inside);
        assert!((state.ray.dir - child.s).length() < STATE_TOLERANCE);
        assert!((state.k - child.state.k).length() < STATE_TOLERANCE);
        assert_eq!(state.segment_origin.distance(state.ray.origin), 0.0);
        let prev = state.prev_plane.expect("plane of incidence");
        assert!((prev - expansion.plane_normal).length() < STATE_TOLERANCE);
        for (a, b) in state.stokes[0].iter().zip(&child.state.stokes) {
            assert!((a.i - b.i).abs() < STATE_TOLERANCE, "{} vs {}", a.i, b.i);
        }
        // The origin is the exact hit on the top plane.
        assert!((f64::from(state.ray.origin.y) - 1.0).abs() < 1e-6);
    });
}

#[test]
fn retrace_state_along_two_steps_equals_a_hand_replay() {
    with_kit(BeamRig::cube, glass(), |kit, rig| {
        let spectrum = SpectralInterval::single(3);
        let mut beam = entry_beam(kit, 1.0);
        beam.spectrum = spectrum;
        beam.prefix = beam
            .prefix
            .push(word(BOTTOM, EventKind::Reflect))
            .expect("prefix");
        let image = DVec2::new(31.0, 32.5);
        let state = retrace_state(image, &beam, kit).expect("state");
        let map =
            crate::renderer::beam::geom::homography::camera_direction_map(&rig.camera, SIZE, SIZE);
        let camera = camera_state(&rig.camera, map, image, spectrum);
        let first_path = PathState {
            k: camera.k,
            inside: false,
            is_ext: false,
            stokes: camera.stokes[0],
        };
        let first = hand_bounce(kit, first_path, camera.ray, TOP, None);
        let entered = pick(&first.children, false);
        let hit = {
            let (n, m) = kit
                .retrace
                .stone
                .plane(kit.retrace.stone.facet_of_original(TOP).expect("top"));
            let o = camera.ray.origin.as_dvec3();
            let d = camera.ray.dir.as_dvec3();
            (o + d * ((m - n.dot(o)) / n.dot(d))).as_vec3()
        };
        let ray = crate::optics::raytracer::Ray {
            origin: hit,
            dir: entered.s,
        };
        let second = hand_bounce(kit, entered.state, ray, BOTTOM, Some(first.plane_normal));
        let reflected = pick(&second.children, true);
        assert!(state.inside);
        assert!((state.ray.dir - reflected.s).length() < STATE_TOLERANCE);
        assert!((f64::from(state.ray.origin.y) + 1.0).abs() < 1e-6);
        let prev = state.prev_plane.expect("plane");
        assert!((prev - second.plane_normal).length() < STATE_TOLERANCE);
        for (a, b) in state.stokes[0].iter().zip(&reflected.state.stokes) {
            assert!((a.i - b.i).abs() < STATE_TOLERANCE, "{} vs {}", a.i, b.i);
        }
        assert!(state.path_length > 0.0);
    });
}

#[test]
fn retrace_state_refuses_a_prefix_with_no_exact_ray() {
    with_kit(BeamRig::cube, glass(), |kit, _| {
        let mut beam = entry_beam(kit, 1.0);
        let centre = DVec2::new(f64::from(SIZE) * 0.5, f64::from(SIZE) * 0.5);
        assert!(
            retrace_state(centre, &beam, kit).is_some(),
            "the entry prefix retraces at the frame centre"
        );
        // A ray reflected off the top of the cube from above travels up and away: the bottom plane
        // lies behind it, a provable miss. The refusal is a clean `None`.
        beam.prefix = BranchPrefix::EMPTY
            .push(word(TOP, EventKind::Reflect))
            .and_then(|prefix| prefix.push(word(BOTTOM, EventKind::Reflect)))
            .expect("prefix");
        assert!(retrace_state(centre, &beam, kit).is_none());
        // The camera ray itself exists there, so the refusal comes from the prefix.
        let camera_dir = camera_direction(kit.retrace.camera, kit.retrace.size, centre);
        assert!(camera_dir.length() > 0.0);
    });
}

#[test]
fn the_cube_has_the_facets_the_tests_assume() {
    let rig = BeamRig::cube(glass());
    let stone = BeamStone::new(&rig.planes, false).expect("a stone");
    assert_eq!(stone.facet_count(), 6);
    let planes: &[GpuFacetPlane] = &rig.planes;
    assert_eq!(planes[TOP as usize].normal, [0.0, 1.0, 0.0]);
}
