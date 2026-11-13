//! Tests of the model circle, the snapped TIR split and the internal hit.

use std::sync::OnceLock;

use glam::{DVec2, DVec3, Vec3};

use super::{
    BeamSettings,
    beam::{Beam, SampleState, SeamStore, SpectralInterval, SpectralNode},
    entry::refract_entry,
    geom::{Tolerances, polygon::ConvexPolygon},
    interact::{
        InteractKit, IntervalHeroes, internal_hit, mirror_apex, model_circle, split_tir, tir_flags,
    },
    primary::{TileRect, image_polygon, primary_beams},
    propagate::{partition, project_to},
    retrace::{RetraceKit, exact_ray, exact_ray_at, is_total_reflection},
    snap::snap_partition,
    stats::{BeamStats, TILE_ALLOWANCE},
    stone::BeamStone,
    tests_support::{BeamRig, MAX_BOUNCES, glass},
};
use crate::{
    optics::{
        materials::GemMaterial,
        raytracer::{Ray, transport::PreparedMaterial},
    },
    renderer::path_cache::{
        bounce::{EventKind, Expansion, ExplicitHit, PathState, expand_at},
        build::HeroContext,
        signature::branch_step,
    },
};

/// The frame size in pixels.
const SIZE: u32 = 64;
/// The spectral node count.
const N_LAMBDA: u32 = 48;
/// The first group of nodes.
const GROUP_ZERO: SpectralInterval = SpectralInterval { first: 0, last: 7 };
/// The cube's bottom facet (original index of the -Y plane).
const BOTTOM: u32 = 3;
/// The side in pixels of the tiles the search walks, centre tiles first.
///
/// The rays of one entry facet are nearly parallel, so an entry facet reaches the critical angle
/// on a given internal facet only by chance. The search therefore widens tile by tile over the
/// whole frame until it has enough candidates.
const SEARCH_TILE_SIDE: u32 = 16;
/// How many straddling candidates the search collects before it keeps the largest.
const CANDIDATES: usize = 12;
/// How many straddling beams the tests use.
const WANTED_MIXED: usize = 3;
/// How many beams with one class the tests use.
const WANTED_UNIFORM: usize = 3;
/// The relative tolerance of the tiling checks.
const TILING: f64 = 1e-12;
/// `ε_chord` in pixels (the default setting).
const CHORD_PX: f64 = 1e-3;
/// `τ_dir` of the fixtures: 1e-3, a fine setting.
///
/// The default of 1e-2 cuts a brilliant's primary beam into only three or four entry children. The
/// fine setting cuts them into many small pieces, so a critical curve that crosses an entry facet
/// crosses many of them. The entry tolerance is not under test here, so the fixtures pin it.
const FINE_TAU_DIR: f64 = 1e-3;
/// The tolerance of the exact-reflection comparisons.
const REFLECTION: f64 = 1e-12;
/// The tolerance of the `f32` state directions against the exact ones.
const STATE_DIRECTION: f64 = 1e-5;
/// The tolerance of the per-hit energy conservation.
const CONSERVATION: f32 = 1e-6;
/// Random points per check.
const POINTS: usize = 1000;
/// The most random points drawn while looking for `POINTS` on each side.
const MAX_DRAWS: usize = 400_000;

/// Runs `f` with an interact kit over the rig built by `make` for `material`.
fn with_kit<R>(
    make: fn(GemMaterial) -> BeamRig,
    material: GemMaterial,
    max_bounces: u32,
    f: impl FnOnce(&InteractKit<'_, '_>, &BeamStone) -> R,
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
    let settings = BeamSettings {
        dir_tolerance_rad: FINE_TAU_DIR,
        ..BeamSettings::default()
    };
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
        max_bounces,
        subnodes: None,
        interval_heroes: IntervalHeroes::default(),
        cancel: None,
        allowance: TILE_ALLOWANCE,
    };
    f(&kit, &stone)
}

/// A deterministic pseudo-random number in `[0, 1)`.
fn next_unit(seed: &mut u64) -> f64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    f64::from(u32::try_from(*seed >> 40).unwrap_or(0)) / f64::from(1_u32 << 24)
}

/// Beams on internal facets found by running the pipeline on the brilliant.
struct Found {
    /// Beams whose sample points disagree on total reflection, largest first.
    mixed: Vec<Beam>,
    /// Beams whose sample points agree.
    uniform: Vec<Beam>,
    /// What the search looked at, for the failure message.
    visited: Visited,
}

/// The counts of what the search walked through.
#[derive(Debug, Default)]
struct Visited {
    /// Tiles searched.
    tiles: usize,
    /// Primary beams refracted.
    primaries: usize,
    /// Entry children produced.
    children: usize,
    /// Partition pieces projected onto their internal facet.
    pieces: usize,
    /// Pieces whose projection was refused.
    refused: usize,
    /// Projected beams that classified as total reflection at every sample point.
    all_total: usize,
    /// Projected beams that classified as partial reflection at every sample point.
    all_partial: usize,
}

/// The tiles of the frame, the ones nearest the centre first.
fn search_tiles() -> Vec<TileRect> {
    let per_side = SIZE.div_ceil(SEARCH_TILE_SIDE);
    let mut tiles: Vec<TileRect> = (0..per_side * per_side)
        .filter_map(|index| TileRect::of_index((SIZE, SIZE), SEARCH_TILE_SIDE, index))
        .collect();
    let centre = f64::from(SIZE) * 0.5;
    let distance = |t: &TileRect| {
        let dx = f64::from(t.x0 + t.x1).mul_add(0.5, -centre);
        let dy = f64::from(t.y0 + t.y1).mul_add(0.5, -centre);
        dx.mul_add(dx, dy * dy)
    };
    tiles.sort_by(|a, b| distance(a).total_cmp(&distance(b)));
    tiles
}

impl Found {
    /// `true` once the search has all the candidates it wants.
    fn is_full(&self) -> bool {
        self.mixed.len() >= CANDIDATES && self.uniform.len() >= WANTED_UNIFORM
    }

    /// Classifies `beam` by its sample points and keeps it when it is wanted.
    fn record(&mut self, beam: Beam, kit: &InteractKit<'_, '_>) {
        let flags = tir_flags(&beam, kit);
        let any_total = flags.iter().any(|&f| f);
        let any_partial = flags.iter().any(|&f| !f);
        let mixed = any_total && any_partial;
        if any_total && !any_partial {
            self.visited.all_total += 1;
        } else if any_partial && !any_total {
            self.visited.all_partial += 1;
        }
        if mixed && self.mixed.len() < CANDIDATES {
            self.mixed.push(beam);
        } else if !mixed && self.uniform.len() < WANTED_UNIFORM {
            self.uniform.push(beam);
        }
    }

    /// Partitions the entry `child` and records every piece that projects onto its internal facet.
    fn record_child(
        &mut self,
        child: &Beam,
        kit: &InteractKit<'_, '_>,
        stone: &BeamStone,
        stats: &mut BeamStats,
    ) {
        self.visited.children += 1;
        let result = partition(child, stone, &Tolerances::DEFAULT);
        // As in production (`expand_beam`): the model boundaries are snapped onto the exact
        // curves before the pieces are projected; the projection check relies on it.
        let Some(pieces) = snap_partition(child, &result, kit, &mut SeamStore::default(), stats)
        else {
            self.visited.pieces += result.pieces.len();
            self.visited.refused += result.pieces.len();
            return;
        };
        for piece in &pieces {
            self.visited.pieces += 1;
            match project_to(child, piece, kit, stats) {
                Some(beam) => self.record(beam, kit),
                None => self.visited.refused += 1,
            }
        }
    }
}

/// The beams on internal facets of the frame, tile by tile from the centre: primary, entry
/// refraction, partition and projection, as the tree driver does for the first internal hit.
fn search(kit: &InteractKit<'_, '_>, stone: &BeamStone) -> Found {
    let mut stats = BeamStats::default();
    let mut found = Found {
        mixed: Vec::new(),
        uniform: Vec::new(),
        visited: Visited::default(),
    };
    'tiles: for tile in search_tiles() {
        found.visited.tiles += 1;
        for primary in primary_beams(stone, kit.retrace.camera, (SIZE, SIZE), tile, GROUP_ZERO) {
            found.visited.primaries += 1;
            for child in refract_entry(&primary, kit, &mut stats) {
                found.record_child(&child, kit, stone, &mut stats);
                if found.is_full() {
                    break 'tiles;
                }
            }
        }
    }
    let area = |b: &Beam| image_polygon(b).map_or(0.0, |p| p.area());
    found.mixed.sort_by(|a, b| area(b).total_cmp(&area(a)));
    found.mixed.truncate(WANTED_MIXED);
    found
}

/// The beams of the search, computed once.
fn found() -> &'static Found {
    static FOUND: OnceLock<Found> = OnceLock::new();
    FOUND.get_or_init(|| {
        with_kit(BeamRig::brilliant, glass(), MAX_BOUNCES, |kit, stone| {
            let found = search(kit, stone);
            eprintln!(
                "search: {} straddling beams, {} uniform beams, {:?}",
                found.mixed.len(),
                found.uniform.len(),
                found.visited
            );
            found
        })
    })
}

/// Fails, naming what the search did find, when it found no beam that straddles the critical curve.
fn require_straddling(found: &Found) {
    assert!(
        !found.mixed.is_empty(),
        "no straddling beam found in a {SIZE} px frame at tau_dir {FINE_TAU_DIR} ({} uniform \
         beams kept; searched {:?})",
        found.uniform.len(),
        found.visited
    );
}

/// The distance of `p` from the segment `a b`.
fn segment_distance(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    let edge = b - a;
    let len2 = edge.length_squared();
    let along = if len2 > 0.0 {
        ((p - a).dot(edge) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    edge.mul_add(DVec2::splat(along), a).distance(p)
}

/// The distance of `q` from the boundary of the image polygon.
fn edge_distance(image: &ConvexPolygon, q: DVec2) -> f64 {
    let (points, n) = image.points_2d();
    (0..n)
        .map(|i| segment_distance(q, points[i], points[(i + 1) % n]))
        .fold(f64::INFINITY, f64::min)
}

// ---- the model ----

#[test]
fn the_model_circle_is_the_critical_curve_of_rays_through_the_apex() {
    with_kit(BeamRig::cube, glass(), MAX_BOUNCES, |kit, stone| {
        let facet = stone.facet_of_original(BOTTOM).expect("bottom");
        let plane = stone.plane(facet);
        let lambda = SpectralNode(23).centre_nm(N_LAMBDA);
        let index = f64::from(kit.retrace.material.dispersion.evaluate(lambda));
        let apex = DVec3::new(0.2, 0.9, -0.1);
        let circle = model_circle(apex, plane, index).expect("a circle");
        let unit = plane.0.normalize();
        let height = (unit.dot(apex) - plane.1 / plane.0.length()).abs();
        let expected = height / index.mul_add(index, -1.0).sqrt();
        assert!((circle.radius - expected).abs() < 1e-12, "radius");
        let mut seed = 7_u64;
        let mut checked = 0;
        for _ in 0..POINTS {
            let x = next_unit(&mut seed).mul_add(6.0, -3.0);
            let z = next_unit(&mut seed).mul_add(6.0, -3.0);
            let y = plane.0.z.mul_add(-z, plane.0.x.mul_add(-x, plane.1)) / plane.0.y;
            let p = DVec3::new(x, y, z);
            let distance = p.distance(circle.centre);
            if (distance - circle.radius).abs() < 1e-9 {
                continue;
            }
            let tir = is_total_reflection((p - apex).normalize(), facet, lambda, &kit.retrace);
            assert_eq!(tir, distance > circle.radius, "point {p:?}");
            checked += 1;
        }
        assert!(checked > POINTS / 2, "{checked} points checked");
        assert!(model_circle(apex, plane, 1.0).is_none(), "no TIR at n = 1");
    });
}

#[test]
fn the_mirror_apex_reproduces_the_exact_reflection() {
    let plane = (DVec3::new(0.3, -0.9, 0.2), 0.7);
    let unit = plane.0.normalize();
    let apex = DVec3::new(0.1, 0.4, -0.2);
    let mirror = mirror_apex(apex, plane);
    // The mirror lies on the other side at the same distance.
    let side = |p: DVec3| unit.dot(p) - plane.1 / plane.0.length();
    assert!((side(apex) + side(mirror)).abs() < 1e-12);
    let mut seed = 11_u64;
    for _ in 0..POINTS {
        let du = next_unit(&mut seed).mul_add(4.0, -2.0);
        let dv = next_unit(&mut seed).mul_add(4.0, -2.0);
        // A point on the plane: foot of the origin plus two in-plane offsets.
        let helper = if unit.x.abs() < 0.9 {
            DVec3::X
        } else {
            DVec3::Y
        };
        let tangent_u = unit.cross(helper).normalize();
        let tangent_v = unit.cross(tangent_u);
        let p = unit * (plane.1 / plane.0.length()) + tangent_u * du + tangent_v * dv;
        let incident = (p - apex).normalize();
        let reflected = incident - 2.0 * incident.dot(unit) * unit;
        let from_mirror = (p - mirror).normalize();
        assert!((reflected - from_mirror).length() < REFLECTION);
    }
}

// ---- the split ----

#[test]
fn a_beam_whose_points_agree_is_returned_whole() {
    let found = found();
    assert!(!found.uniform.is_empty(), "no uniform beam found");
    with_kit(BeamRig::brilliant, glass(), MAX_BOUNCES, |kit, _| {
        for beam in &found.uniform {
            let mut stats = BeamStats::default();
            let mut seams = SeamStore::default();
            let (partial, tir) = split_tir(beam, kit, &mut seams, &mut stats);
            assert_eq!(partial.len() + tir.len(), 1);
            let whole = partial.first().or_else(|| tir.first()).expect("a beam");
            assert_eq!(whole, beam);
            assert_eq!(stats.tir_splits, 0);
            assert_eq!(stats.drops.iter().sum::<u64>(), 0);
        }
    });
}

/// Checks that random points in `parent` are classified by the exact critical curve as the piece
/// containing them says, except within a few `ε_chord` of a piece's edge.
fn check_classification(kit: &InteractKit<'_, '_>, parent: &Beam, partial: &[Beam], tir: &[Beam]) {
    let whole = image_polygon(parent).expect("parent image");
    let pieces: Vec<(ConvexPolygon, bool)> = partial
        .iter()
        .map(|b| (image_polygon(b).expect("piece image"), false))
        .chain(
            tir.iter()
                .map(|b| (image_polygon(b).expect("piece image"), true)),
        )
        .collect();
    let (points, n) = whole.points_2d();
    let lo = points[..n]
        .iter()
        .fold(DVec2::splat(f64::INFINITY), |a, p| a.min(*p));
    let hi = points[..n]
        .iter()
        .fold(DVec2::splat(f64::NEG_INFINITY), |a, p| a.max(*p));
    let lambda = SpectralNode(parent.spectrum.centre()).centre_nm(N_LAMBDA);
    let mut seed = 99_u64;
    let (mut in_partial, mut in_tir) = (0, 0);
    for _ in 0..MAX_DRAWS {
        if in_partial >= POINTS && in_tir >= POINTS {
            break;
        }
        let q = lo + (hi - lo) * DVec2::new(next_unit(&mut seed), next_unit(&mut seed));
        if !whole.contains_2d(q, 0.0) {
            continue;
        }
        let Some((piece, expected)) = pieces.iter().find(|(p, _)| p.contains_2d(q, 0.0)) else {
            continue;
        };
        if *expected {
            in_tir += 1;
        } else {
            in_partial += 1;
        }
        let ray = exact_ray_at(q, lambda, &parent.prefix, &kit.retrace).expect("exact ray");
        let actual = is_total_reflection(ray.dir, parent.facet, lambda, &kit.retrace);
        if actual != *expected {
            let near = edge_distance(piece, q);
            assert!(
                near <= 4.0 * CHORD_PX,
                "point {q:?} is {actual} but its piece says {expected}, {near} px from the edge"
            );
        }
    }
    eprintln!("classified {in_partial} partial and {in_tir} TIR points");
    assert!(in_partial > 0 && in_tir > 0);
}

#[test]
fn the_pieces_tile_the_beam_and_the_snapped_curve_classifies_correctly() {
    let found = found();
    require_straddling(found);
    with_kit(BeamRig::brilliant, glass(), MAX_BOUNCES, |kit, _| {
        for beam in &found.mixed {
            let mut stats = BeamStats::default();
            let mut seams = SeamStore::default();
            let (partial, tir) = split_tir(beam, kit, &mut seams, &mut stats);
            assert!(!partial.is_empty() && !tir.is_empty(), "{stats:?}");
            assert_eq!(stats.drops.iter().sum::<u64>(), 0, "{stats:?}");
            assert!(stats.tir_splits >= 1, "{stats:?}");
            eprintln!(
                "split: {} partial, {} TIR pieces, {} snapped vertices, {} splits",
                partial.len(),
                tir.len(),
                stats.snapped_vertices,
                stats.tir_splits
            );
            let whole = beam.polygon.area();
            let sum: f64 = partial.iter().chain(&tir).map(|b| b.polygon.area()).sum();
            assert!(
                (sum - whole).abs() <= TILING * whole,
                "pieces {sum} vs beam {whole}"
            );
            for piece in partial.iter().chain(&tir) {
                assert_eq!(piece.facet, beam.facet);
                assert_eq!(piece.prefix, beam.prefix);
                assert_eq!(piece.sig, beam.sig);
                assert_eq!(piece.state.points.len(), 2 * piece.state.vertices + 1);
            }
            for piece in &partial {
                assert!(tir_flags(piece, kit).iter().any(|&f| !f));
            }
            check_classification(kit, beam, &partial, &tir);
        }
    });
}

// ---- the hit ----

/// The expansion of a state at its exact hit on the carrier facet, with group 0's hero.
fn expansion_at(kit: &InteractKit<'_, '_>, beam: &Beam, state: &SampleState) -> Expansion {
    let stone = kit.retrace.stone;
    let original = stone.original_facet(beam.facet) as usize;
    let (n, m) = stone.plane(beam.facet);
    let origin = state.segment_origin.as_dvec3();
    let dir = state.ray.dir.as_dvec3();
    let t = (m - n.dot(origin)) / n.dot(dir);
    let point = dir.mul_add(DVec3::splat(t), origin).as_vec3();
    let path = PathState {
        k: state.k,
        inside: state.inside,
        is_ext: state.is_ext,
        stokes: state.stokes[0],
    };
    expand_at(
        &kit.heroes[0],
        kit.planes,
        path,
        Ray {
            origin: state.segment_origin,
            dir: state.ray.dir,
        },
        ExplicitHit {
            point,
            facet: original,
            normal: Vec3::from(kit.planes[original].normal),
        },
        Some(!state.inside),
        state.prev_plane,
    )
}

#[test]
fn the_children_of_every_sample_point_conserve_the_incoming_energy() {
    let found = found();
    require_straddling(found);
    with_kit(BeamRig::brilliant, glass(), MAX_BOUNCES, |kit, _| {
        let mut points = 0;
        for beam in &found.mixed {
            let mut stats = BeamStats::default();
            let mut seams = SeamStore::default();
            let (partial, tir) = split_tir(beam, kit, &mut seams, &mut stats);
            for piece in partial.iter().chain(&tir) {
                for state in &piece.state.points {
                    let expansion = expansion_at(kit, piece, state);
                    for slot in 0..8 {
                        let total: f32 = expansion
                            .children
                            .iter()
                            .map(|c| c.state.stokes[slot].i)
                            .sum();
                        let incoming = state.stokes[0][slot].i;
                        let error = (total + expansion.dropped - incoming).abs();
                        assert!(
                            error <= CONSERVATION * incoming.max(1.0),
                            "node {slot}: children {total} + dropped {} vs incoming {incoming}",
                            expansion.dropped
                        );
                    }
                    points += 1;
                }
            }
        }
        assert!(points > 0);
    });
}

#[test]
fn partial_and_total_reflection_share_one_signature_code() {
    let sig = [0x1234_5678, 0x9ABC_DEF0];
    for (facet, ext) in [(0, false), (7, true), (31, false)] {
        assert_eq!(
            branch_step(sig, facet, EventKind::Reflect, ext),
            branch_step(sig, facet, EventKind::TotalReflect, ext)
        );
        assert_ne!(
            branch_step(sig, facet, EventKind::Reflect, ext),
            branch_step(sig, facet, EventKind::Transmit, ext)
        );
    }
    let found = found();
    require_straddling(found);
    with_kit(BeamRig::brilliant, glass(), MAX_BOUNCES, |kit, stone| {
        for beam in &found.mixed {
            let mut stats = BeamStats::default();
            let mut seams = SeamStore::default();
            let hit = internal_hit(beam, kit, &mut seams, &mut stats);
            assert!(!hit.reflected.is_empty(), "{stats:?}");
            let original = stone.original_facet(beam.facet);
            let expected = branch_step(beam.sig, original, EventKind::Reflect, false);
            for child in hit.reflected.iter().filter(|c| !c.state.points[0].is_ext) {
                assert_eq!(child.sig, expected, "TIR and partial children differ");
            }
        }
    });
}

#[test]
fn reflected_children_carry_the_mirror_apex_and_the_exact_reflection() {
    let found = found();
    require_straddling(found);
    with_kit(BeamRig::brilliant, glass(), MAX_BOUNCES, |kit, stone| {
        for beam in &found.mixed {
            let mut stats = BeamStats::default();
            let mut seams = SeamStore::default();
            let hit = internal_hit(beam, kit, &mut seams, &mut stats);
            assert_ne!(hit.reflected.len(), 0);
            let original = stone.original_facet(beam.facet);
            let unit = stone.plane(beam.facet).0.normalize();
            let mut area = 0.0;
            for child in &hit.reflected {
                assert!(child.inside);
                assert_eq!(child.facet, beam.facet);
                assert_eq!(child.bounces, beam.bounces + 1);
                assert_eq!(child.prefix.len, beam.prefix.len + 1);
                assert_eq!(
                    &child.prefix.steps()[..usize::from(beam.prefix.len)],
                    beam.prefix.steps()
                );
                let step = *child.prefix.steps().last().expect("a step");
                assert_eq!(step >> 8, original);
                assert_eq!((step >> 1) & 0x7F, 1, "reflect code");
                assert_eq!(child.apex, mirror_apex(beam.apex, stone.plane(beam.facet)));
                area += child.polygon.area();
                let node = SpectralNode(child.spectrum.centre());
                for state in &child.state.points {
                    let incoming = exact_ray(state.image, node, &beam.prefix, &kit.retrace)
                        .expect("incoming ray");
                    let outgoing = exact_ray(state.image, node, &child.prefix, &kit.retrace)
                        .expect("reflected ray");
                    let expected = incoming.dir - 2.0 * incoming.dir.dot(unit) * unit;
                    assert!(
                        (outgoing.dir - expected).length() < REFLECTION,
                        "exact reflection"
                    );
                    assert!(
                        (state.ray.dir.as_dvec3() - outgoing.dir).length() < STATE_DIRECTION,
                        "state direction"
                    );
                }
            }
            // The two modes of a uniaxial stone would each cover the beam; glass has one.
            assert!(area <= beam.polygon.area() * (1.0 + 1e-9), "{area}");
        }
    });
}

#[test]
fn the_max_bounces_rule_stops_a_branch_without_a_drop() {
    let found = found();
    require_straddling(found);
    let beam = &found.mixed[0];
    assert_eq!(beam.bounces, 1, "an entry child has one event");
    // This hit is the second event; it is shaded only when 2 <= max_bounces - 1.
    with_kit(BeamRig::brilliant, glass(), 2, |kit, _| {
        let mut stats = BeamStats::default();
        let mut seams = SeamStore::default();
        let hit = internal_hit(beam, kit, &mut seams, &mut stats);
        assert!(hit.reflected.is_empty() && hit.transmitted.is_empty());
        assert_eq!(stats, BeamStats::default(), "uncounted energy");
        assert!(seams.is_empty());
    });
    // The hit may emit its exit, but a reflected child could not exit any more.
    with_kit(BeamRig::brilliant, glass(), 3, |kit, _| {
        let mut stats = BeamStats::default();
        let mut seams = SeamStore::default();
        let hit = internal_hit(beam, kit, &mut seams, &mut stats);
        assert!(hit.reflected.is_empty(), "no reflected child at the cap");
    });
    // With room for one more bounce the reflected children exist.
    with_kit(BeamRig::brilliant, glass(), 4, |kit, _| {
        let mut stats = BeamStats::default();
        let mut seams = SeamStore::default();
        let hit = internal_hit(beam, kit, &mut seams, &mut stats);
        assert!(!hit.reflected.is_empty(), "{stats:?}");
    });
}

#[test]
fn an_internal_hit_is_deterministic() {
    let found = found();
    require_straddling(found);
    with_kit(BeamRig::brilliant, glass(), MAX_BOUNCES, |kit, _| {
        let beam = &found.mixed[0];
        let run = || {
            let mut stats = BeamStats::default();
            let mut seams = SeamStore::default();
            let hit = internal_hit(beam, kit, &mut seams, &mut stats);
            (hit, stats)
        };
        let (first, first_stats) = run();
        let (second, second_stats) = run();
        assert_eq!(first, second);
        assert_eq!(first_stats, second_stats);
    });
}
