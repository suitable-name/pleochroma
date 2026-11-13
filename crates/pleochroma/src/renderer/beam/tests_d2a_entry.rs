//! The apex fit and the entry refraction.

use glam::{DVec2, DVec3, Vec3};

use super::{
    BeamSettings,
    beam::{Beam, SpectralInterval},
    entry::{fit_apex, fit_error, refract_entry},
    geom::{homography::camera_direction_map, polygon::ConvexPolygon},
    interact::{InteractKit, IntervalHeroes},
    leaf::GROUP_HERO,
    primary::{TileRect, beam_for_image, camera_state, image_polygon, primary_beams},
    retrace::RetraceKit,
    stats::{BeamStats, DropReason, TILE_ALLOWANCE},
    stone::BeamStone,
    tests_support::{BeamRig, cube_planes, glass},
};
use crate::{
    optics::raytracer::{Camera, Ray, transport::PreparedMaterial},
    renderer::path_cache::{
        bounce::{EventKind, ExplicitHit, PathState, expand_at},
        build::HeroContext,
        signature::branch_step,
    },
};

/// The frame size in pixels.
const SIZE: u32 = 64;
/// The spectral node count.
const N_LAMBDA: u32 = 48;
/// The cube's top facet (original index of the +Y plane).
const TOP: u32 = 2;
/// The first group of nodes.
const GROUP_ZERO: SpectralInterval = SpectralInterval { first: 0, last: 7 };
/// The pitch of 30 degrees incidence on the top facet.
const PITCH_30: f32 = std::f32::consts::FRAC_PI_2 - std::f32::consts::FRAC_PI_6;
/// The guide's tolerance on the ratio 2 of the O(h) convergence (§16.1).
const RATIO_BAND: f64 = 0.2;
/// The direction-error floor of `f32` rays, in radians.
const F32_DIRECTION_FLOOR: f64 = 1.0e-6;

fn rig_at(pitch: f32) -> BeamRig {
    BeamRig::new(cube_planes(), glass(), Camera::new(0.0, pitch, 8.0, 24.0))
}

fn with_kit<R>(
    rig: &BeamRig,
    settings: &BeamSettings,
    f: impl FnOnce(&InteractKit<'_, '_>, &BeamStone) -> R,
) -> R {
    let stone = BeamStone::new(&rig.planes, false).expect("the cube is a stone");
    let prepared = PreparedMaterial::new(&rig.material);
    let full = SpectralInterval::all(N_LAMBDA);
    let heroes: Vec<HeroContext<'_>> = (0..full.groups())
        .map(|g| {
            let (ctx, cache) = prepared.ray_context(full.group_lambdas(g, N_LAMBDA), GROUP_HERO);
            HeroContext { ctx, cache }
        })
        .collect();
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
        settings,
        tile: TileRect {
            x0: 0,
            y0: 0,
            x1: SIZE,
            y1: SIZE,
        },
        tau_omega: 0.0,
        max_bounces: 12,
        subnodes: None,
        interval_heroes: IntervalHeroes::default(),
        cancel: None,
        allowance: TILE_ALLOWANCE,
    };
    f(&kit, &stone)
}

/// The primary beam of the top facet over the whole frame.
fn top_beam(rig: &BeamRig, stone: &BeamStone) -> Beam {
    let top = stone.facet_of_original(TOP).expect("top facet");
    let tile = TileRect {
        x0: 0,
        y0: 0,
        x1: SIZE,
        y1: SIZE,
    };
    primary_beams(stone, &rig.camera, (SIZE, SIZE), tile, GROUP_ZERO)
        .into_iter()
        .find(|b| b.facet == top)
        .expect("the top facet is seen")
}

/// A square patch beam of half-width `half` pixels around `centre` on the beam's facet.
fn patch(rig: &BeamRig, stone: &BeamStone, facet: u32, centre: DVec2, half: f64) -> Beam {
    let corners = [
        centre + DVec2::new(-half, -half),
        centre + DVec2::new(half, -half),
        centre + DVec2::new(half, half),
        centre + DVec2::new(-half, half),
    ];
    let image = ConvexPolygon::from_image_points(&corners).expect("patch polygon");
    beam_for_image(stone, &rig.camera, (SIZE, SIZE), facet, &image, GROUP_ZERO).expect("patch")
}

#[test]
fn fit_apex_recovers_the_apex_of_lines_through_one_point() {
    let apex = DVec3::new(0.3, -0.2, 2.5);
    let points: Vec<DVec3> = [
        (0.0, 0.0),
        (1.0, 0.2),
        (-0.7, 0.9),
        (0.4, -1.1),
        (-1.2, -0.5),
        (0.1, 0.45),
    ]
    .iter()
    .map(|&(x, y)| DVec3::new(x, y, 0.0))
    .collect();
    let dirs: Vec<DVec3> = points.iter().map(|&p| (p - apex) * 3.7).collect();
    let fitted = fit_apex(&points, &dirs).expect("a fit");
    assert!((fitted - apex).length() < 1e-12, "apex {fitted}");
    let flipped: Vec<DVec3> = dirs.iter().map(|&d| -d).collect();
    let fitted = fit_apex(&points, &flipped).expect("a fit");
    assert!((fitted - apex).length() < 1e-12, "apex {fitted}");
}

#[test]
fn fit_apex_is_stable_for_near_parallel_lines() {
    let radius = 1.0e3;
    let apex = DVec3::new(0.0, 0.0, radius);
    let points: Vec<DVec3> = (0..9)
        .map(|i| DVec3::new(f64::from(i % 3) - 1.0, f64::from(i / 3) - 1.0, 0.0))
        .collect();
    let dirs: Vec<DVec3> = points.iter().map(|&p| p - apex).collect();
    let fitted = fit_apex(&points, &dirs).expect("a fit");
    assert!(fitted.is_finite());
    assert!((fitted - apex).length() < 1e-8 * radius, "apex {fitted}");
}

#[test]
fn fit_apex_refuses_degenerate_sets() {
    let p = [DVec3::ZERO, DVec3::X, DVec3::Y];
    assert_eq!(fit_apex(&p[..1], &[DVec3::Z]), None);
    assert_eq!(fit_apex(&p, &[DVec3::Z, DVec3::Z]), None);
    // Parallel lines have no apex.
    assert_eq!(fit_apex(&p, &[DVec3::Z; 3]), None);
    // A zero direction is no line.
    assert_eq!(fit_apex(&p, &[DVec3::Z, DVec3::ZERO, DVec3::Z]), None);
    // Two crossing lines meet at their crossing.
    let two = fit_apex(
        &[DVec3::ZERO, DVec3::X],
        &[DVec3::Z, DVec3::new(-1.0, 0.0, 1.0)],
    )
    .expect("two lines");
    assert!((two - DVec3::new(0.0, 0.0, 1.0)).length() < 1e-12, "{two}");
}

#[test]
fn fit_apex_gives_up_on_nearly_exactly_parallel_lines() {
    let apex = DVec3::new(0.0, 0.0, 1.0e9);
    let points: Vec<DVec3> = (0..9)
        .map(|i| DVec3::new(f64::from(i % 3) - 1.0, f64::from(i / 3) - 1.0, 0.0))
        .collect();
    let dirs: Vec<DVec3> = points.iter().map(|&p| p - apex).collect();
    assert_eq!(fit_apex(&points, &dirs), None);
}

#[test]
fn the_fit_error_is_first_order_in_the_patch_size() {
    let rig = rig_at(PITCH_30);
    with_kit(&rig, &BeamSettings::default(), |kit, stone| {
        let whole = top_beam(&rig, stone);
        let centre = image_polygon(&whole).expect("image").centroid().truncate();
        let errors: Vec<f64> = [2.0, 1.0, 0.5, 0.25]
            .iter()
            .map(|&half| {
                let beam = patch(&rig, stone, whole.facet, centre, half);
                fit_error(&beam, kit).expect("a fit")
            })
            .collect();
        eprintln!("entry fit errors at 30 degrees, h = 2, 1, 0.5, 0.25 px: {errors:?}");
        eprintln!("astigmatic constant (error / h): {}", errors[3] / 0.25);
        for pair in errors.windows(2) {
            let ratio = pair[0] / pair[1];
            assert!(
                (ratio - 2.0).abs() <= RATIO_BAND,
                "ratio {ratio} in {errors:?}"
            );
        }
    });
}

#[test]
fn normal_incidence_has_no_fit_error() {
    let rig = rig_at(std::f32::consts::FRAC_PI_2);
    with_kit(&rig, &BeamSettings::default(), |kit, stone| {
        let top = stone.facet_of_original(TOP).expect("top facet");
        let beam = patch(&rig, stone, top, DVec2::new(32.0, 32.0), 0.5);
        let error = fit_error(&beam, kit).expect("a fit");
        assert!(error < F32_DIRECTION_FLOOR, "error {error}");
    });
}

/// The children of the top beam at 30 degrees under the kit's settings.
fn refracted_top(
    rig: &BeamRig,
    kit: &InteractKit<'_, '_>,
    stone: &BeamStone,
    stats: &mut BeamStats,
) -> (Beam, Vec<Beam>) {
    let parent = top_beam(rig, stone);
    let children = refract_entry(&parent, kit, stats);
    (parent, children)
}

#[test]
fn the_children_tile_the_parent_and_carry_the_branch() {
    let rig = rig_at(PITCH_30);
    // The entry must split for this check, which needs a tighter tolerance than the default 1e-2.
    let settings = BeamSettings {
        dir_tolerance_rad: 1.0e-3,
        ..BeamSettings::default()
    };
    with_kit(&rig, &settings, |kit, stone| {
        let mut stats = BeamStats::default();
        let (parent, children) = refracted_top(&rig, kit, stone, &mut stats);
        assert!(children.len() >= 2, "{} children", children.len());
        assert_eq!(stats.drops.iter().sum::<u64>(), 0, "{stats:?}");
        let sum: f64 = children.iter().map(|c| c.polygon.area()).sum();
        let whole = parent.polygon.area();
        assert!((sum - whole).abs() <= 1e-12 * whole, "{sum} vs {whole}");
        assert_eq!(stats.entry_pieces, children.len() as u64);
        assert_eq!(stats.apex_splits + 1, children.len() as u64);
        let original = stone.original_facet(parent.facet);
        let (n, m) = stone.plane(parent.facet);
        for child in &children {
            assert!(child.inside);
            assert_eq!(child.facet, parent.facet);
            assert_eq!(child.bounces, 1);
            assert_eq!(
                child.sig,
                branch_step(parent.sig, original, EventKind::Transmit, false)
            );
            assert_eq!(child.prefix.steps(), &[(original << 8) | (3 << 1)]);
            assert!(
                n.dot(child.apex) > m,
                "the apex is outside the carrier plane"
            );
            assert_eq!(child.state.points.len(), 2 * child.state.vertices + 1);
        }
    });
}

#[test]
fn every_child_state_is_a_fresh_expand_at_its_own_point() {
    let rig = rig_at(PITCH_30);
    with_kit(&rig, &BeamSettings::default(), |kit, stone| {
        let mut stats = BeamStats::default();
        let (_, children) = refracted_top(&rig, kit, stone, &mut stats);
        let map = camera_direction_map(&rig.camera, SIZE, SIZE);
        for child in &children {
            let original = stone.original_facet(child.facet);
            for s in &child.state.points {
                let outside = camera_state(&rig.camera, map, s.image, GROUP_ZERO);
                let hit = ExplicitHit {
                    point: s.segment_origin,
                    facet: original as usize,
                    normal: Vec3::from(rig.planes[original as usize].normal),
                };
                let path = PathState {
                    k: outside.k,
                    inside: false,
                    is_ext: false,
                    stokes: outside.stokes[0],
                };
                let ray = Ray {
                    origin: outside.segment_origin,
                    dir: outside.ray.dir,
                };
                let expansion = expand_at(
                    &kit.heroes[0],
                    &rig.planes,
                    path,
                    ray,
                    hit,
                    Some(true),
                    None,
                );
                let fresh = expansion
                    .children
                    .iter()
                    .find(|c| c.kind == EventKind::Transmit && c.state.is_ext == s.is_ext)
                    .expect("a transmitted child");
                assert_eq!(s.ray.dir, fresh.s);
                assert_eq!(s.ray.origin, s.segment_origin);
                assert_eq!(s.k, fresh.state.k);
                assert_eq!(s.stokes[0], fresh.state.stokes);
                assert_eq!(s.prev_plane, Some(expansion.plane_normal));
                assert!(s.inside);
            }
        }
    });
}

#[test]
fn the_children_meet_the_direction_tolerance() {
    let rig = rig_at(PITCH_30);
    let settings = BeamSettings::default();
    with_kit(&rig, &settings, |kit, stone| {
        let mut stats = BeamStats::default();
        let (_, children) = refracted_top(&rig, kit, stone, &mut stats);
        for child in &children {
            for s in &child.state.points {
                let to_point = (s.segment_origin.as_dvec3() - child.apex).normalize();
                let ray = s.ray.dir.as_dvec3();
                let angle = ray.cross(to_point).length().atan2(ray.dot(to_point));
                assert!(
                    angle <= settings.dir_tolerance_rad + F32_DIRECTION_FLOOR,
                    "angle {angle}"
                );
            }
        }
    });
}

#[test]
fn refraction_is_deterministic() {
    let rig = rig_at(PITCH_30);
    with_kit(&rig, &BeamSettings::default(), |kit, stone| {
        let mut a = BeamStats::default();
        let mut b = BeamStats::default();
        let (_, first) = refracted_top(&rig, kit, stone, &mut a);
        let (_, second) = refracted_top(&rig, kit, stone, &mut b);
        assert_eq!(first, second);
        assert_eq!(a, b);
    });
}

#[test]
fn the_depth_cap_drops_with_the_energy_counted() {
    let rig = rig_at(PITCH_30);
    let settings = BeamSettings {
        max_split_depth: 0,
        dir_tolerance_rad: 1.0e-9,
        ..BeamSettings::default()
    };
    with_kit(&rig, &settings, |kit, stone| {
        let mut stats = BeamStats::default();
        let (parent, children) = refracted_top(&rig, kit, stone, &mut stats);
        assert_eq!(children.len(), 0);
        let slot = DropReason::DepthCap.slot();
        assert_eq!(stats.drops[slot], 1);
        assert_eq!(stats.drops.iter().sum::<u64>(), 1);
        assert_eq!(stats.apex_splits, 0);
        let area = image_polygon(&parent).expect("image").area();
        let weight = stats.dropped_energy[slot] / area;
        // Glass at 30 degrees transmits most of the light and never more than all of it.
        assert!((0.8..=1.0).contains(&weight), "mean weight {weight}");
    });
}

#[test]
fn a_beam_inside_or_past_its_first_event_is_not_refracted() {
    let rig = rig_at(PITCH_30);
    with_kit(&rig, &BeamSettings::default(), |kit, stone| {
        let mut stats = BeamStats::default();
        let (_, children) = refracted_top(&rig, kit, stone, &mut stats);
        let again = refract_entry(&children[0], kit, &mut stats);
        assert_eq!(again.len(), 0);
    });
}
