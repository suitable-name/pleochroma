//! Tests for the pick-mode ring layout, hit test, keyboard order and wording.

use super::{
    meet_pick::{
        PickRing, PickRingKind, accessible_ring, blocked_text, hint_armed, hint_none_visible,
        hint_ring, hit_ring, join_names, pick_rings, plate_detail, step_ring, tier_name,
        toast_cleared, toast_picked,
    },
    projection::{ScreenPoint, ScreenSize, project},
    target::camera_for_pose,
};
use glam::{DVec3, Vec3};
use pleochroma_cut_core::{MeetPickBlock, PickCorner, PickedFacet, TierId, TierMeetPick};
use pleochroma_solid::preview::{CameraPose, FrameGeometry, PickBuffer};
use std::sync::Arc;

const SIZE: u32 = 200;

fn pose() -> CameraPose {
    CameraPose {
        yaw: 0.0,
        pitch: 0.0,
        roll: 0.0,
        distance: 4.0,
        pan: [0.0, 0.0],
    }
}

fn frame(corner_points: Vec<Vec3>) -> FrameGeometry {
    FrameGeometry {
        corner_points: Arc::new(corner_points),
        facet_centroids: Arc::new(Vec::new()),
        bounding_radius: 1.0,
        camera: pose(),
        size: (SIZE, SIZE),
        diagram: None,
        visible_tiers: None,
    }
}

fn pixel_of(p: Vec3) -> ScreenPoint {
    let size = ScreenSize::new(200.0, 200.0);
    project(&camera_for_pose(pose()), p, size).expect("point is in front of the camera")
}

/// A pick buffer that holds `facet_id` in a 5 x 5 patch around each given pixel.
fn buffer_with(patches: &[(ScreenPoint, u32)]) -> PickBuffer {
    let mut pick = vec![0_u32; (SIZE * SIZE) as usize];
    for &(at, id) in patches {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the test points lie inside the small test frame"
        )]
        let (cx, cy) = (at.x.round() as i64, at.y.round() as i64);
        for dy in -2..=2 {
            for dx in -2..=2 {
                let (x, y) = (cx + dx, cy + dy);
                if (0..i64::from(SIZE)).contains(&x) && (0..i64::from(SIZE)).contains(&y) {
                    let cell = usize::try_from(y * i64::from(SIZE) + x).expect("inside the frame");
                    pick[cell] = id + 1;
                }
            }
        }
    }
    PickBuffer {
        width: SIZE,
        height: SIZE,
        pick,
    }
}

fn facet(tier: u64, index: f64) -> PickedFacet {
    PickedFacet {
        tier: TierId(tier),
        index,
    }
}

fn corner(point: Vec3, tiers: &[usize], unnamed: Option<usize>) -> PickCorner {
    PickCorner {
        point: DVec3::new(f64::from(point.x), f64::from(point.y), f64::from(point.z)),
        tiers: tiers.to_vec(),
        pick: TierMeetPick {
            facets: [facet(1, 0.0), facet(2, 0.0), facet(3, 0.0)],
        },
        mast: 1.0,
        unnamed_tier: unnamed,
    }
}

fn names() -> Vec<String> {
    ["G1", "P1", "P2", "P3"].map(str::to_owned).to_vec()
}

/// Facet id `n` belongs to tier `n`.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the shape of the tier lookup `pick_rings` takes, where a facet may have no tier"
)]
fn tier_of(id: u32) -> Option<usize> {
    Some(id as usize)
}

fn ring(at: (f32, f32), depth: f32) -> PickRing {
    PickRing {
        corner: 0,
        at: ScreenPoint::new(at.0, at.1),
        depth,
        kind: PickRingKind::Valid,
        tiers: vec![0, 1, 2],
        names: vec!["G1".into(), "P1".into(), "P2".into()],
        unnamed_tier: None,
    }
}

#[test]
fn rings_sit_on_the_projected_corner_and_follow_the_screen_angle_order() {
    let a = Vec3::new(0.5, 0.0, 0.0); // right of centre
    let b = Vec3::new(-0.5, 0.0, 0.0); // left of centre
    let c = Vec3::new(0.0, -0.5, 0.0); // below centre
    let corners = [
        corner(a, &[0, 1, 2], None),
        corner(b, &[1, 2, 3], None),
        corner(c, &[0, 2, 3], None),
    ];
    let pick = buffer_with(&[(pixel_of(a), 1), (pixel_of(b), 2), (pixel_of(c), 3)]);
    let rings = pick_rings(
        &corners,
        &frame(vec![a, b, c]),
        &pick,
        &names(),
        tier_of,
        None,
    );
    assert_eq!(rings.len(), 3);
    // Angle runs from the left (-pi) through the bottom (+pi/2 with y down) to the right.
    let order: Vec<usize> = rings.iter().map(|r| r.corner).collect();
    let expected = {
        let mut v = vec![0_usize, 1, 2];
        let centre = pixel_of(Vec3::ZERO);
        v.sort_by(|&i, &j| {
            let at = |k: usize| pixel_of([a, b, c][k]);
            let ang = |k: usize| {
                let (dx, dy) = centre.delta_to(at(k));
                dy.atan2(dx)
            };
            ang(i).total_cmp(&ang(j))
        });
        v
    };
    assert_eq!(order, expected);
    for r in &rings {
        let want = pixel_of([a, b, c][r.corner]);
        assert!(r.at.distance(want) < 1e-3);
        assert!(r.depth > 0.0);
    }
    // Repeated calls give the same order.
    let again = pick_rings(
        &corners,
        &frame(vec![a, b, c]),
        &pick,
        &names(),
        tier_of,
        None,
    );
    assert_eq!(rings, again);
}

#[test]
fn a_corner_the_pick_buffer_does_not_show_is_dropped() {
    let a = Vec3::new(0.4, 0.1, 0.0);
    let b = Vec3::new(-0.4, 0.1, 0.0);
    let corners = [corner(a, &[0, 1, 2], None), corner(b, &[0, 1, 2], None)];
    // Only `a` has a facet of its tiers beside it; `b` shows a facet of tier 3.
    let pick = buffer_with(&[(pixel_of(a), 1), (pixel_of(b), 3)]);
    let rings = pick_rings(&corners, &frame(vec![a, b]), &pick, &names(), tier_of, None);
    assert_eq!(rings.len(), 1);
    assert_eq!(rings[0].corner, 0);
}

#[test]
fn a_facet_just_outside_the_visibility_radius_does_not_count() {
    let a = Vec3::new(0.0, 0.0, 0.0);
    let at = pixel_of(a);
    let far = ScreenPoint::new(at.x + 9.0, at.y);
    let pick = buffer_with(&[(far, 1)]);
    let rings = pick_rings(
        &[corner(a, &[1], None)],
        &frame(vec![a]),
        &pick,
        &names(),
        tier_of,
        None,
    );
    assert!(rings.is_empty());
}

#[test]
fn a_corner_that_is_not_a_corner_of_the_shown_mesh_is_dropped() {
    let a = Vec3::new(0.4, 0.1, 0.0);
    let pick = buffer_with(&[(pixel_of(a), 1)]);
    let elsewhere = Vec3::new(0.4, 0.1, 0.01);
    let rings = pick_rings(
        &[corner(a, &[1], None)],
        &frame(vec![elsewhere]),
        &pick,
        &names(),
        tier_of,
        None,
    );
    assert!(rings.is_empty());
}

#[test]
fn a_corner_behind_the_camera_is_dropped() {
    let behind = Vec3::new(0.0, 0.0, 10.0);
    let pick = buffer_with(&[]);
    let rings = pick_rings(
        &[corner(behind, &[1], None)],
        &frame(vec![behind]),
        &pick,
        &names(),
        tier_of,
        None,
    );
    assert!(rings.is_empty());
}

#[test]
fn unnamed_and_current_corners_get_their_own_kind() {
    let a = Vec3::new(0.4, 0.0, 0.0);
    let b = Vec3::new(-0.4, 0.0, 0.0);
    let unnamed = corner(a, &[1, 2, 3], Some(3));
    let picked = corner(b, &[0, 1, 2], None);
    let current = picked.pick;
    let pick = buffer_with(&[(pixel_of(a), 1), (pixel_of(b), 1)]);
    let rings = pick_rings(
        &[unnamed, picked],
        &frame(vec![a, b]),
        &pick,
        &names(),
        tier_of,
        Some(&current),
    );
    let kind_of = |corner: usize| rings.iter().find(|r| r.corner == corner).unwrap().kind;
    assert_eq!(kind_of(0), PickRingKind::Unnamed);
    assert_eq!(kind_of(1), PickRingKind::Current);
    assert!(!rings.iter().find(|r| r.corner == 0).unwrap().pickable());
    assert_eq!(PickRingKind::Unnamed.code(), 1);
    assert_eq!(PickRingKind::Current.code(), 2);
    assert_eq!(PickRingKind::Valid.code(), 0);
}

#[test]
fn a_current_pick_matches_in_any_facet_order() {
    let a = Vec3::new(0.3, 0.0, 0.0);
    let c = corner(a, &[0, 1, 2], None);
    let shuffled = TierMeetPick {
        facets: [c.pick.facets[2], c.pick.facets[0], c.pick.facets[1]],
    };
    let pick = buffer_with(&[(pixel_of(a), 1)]);
    let rings = pick_rings(
        &[c],
        &frame(vec![a]),
        &pick,
        &names(),
        tier_of,
        Some(&shuffled),
    );
    assert_eq!(rings[0].kind, PickRingKind::Current);
}

#[test]
fn hit_ring_respects_the_radius() {
    let rings = [ring((100.0, 100.0), 4.0)];
    assert_eq!(
        hit_ring(&rings, ScreenPoint::new(108.0, 100.0), 12.0),
        Some(0)
    );
    assert_eq!(hit_ring(&rings, ScreenPoint::new(113.0, 100.0), 12.0), None);
    assert_eq!(hit_ring(&[], ScreenPoint::new(0.0, 0.0), 12.0), None);
}

#[test]
fn hit_ring_prefers_the_ring_nearest_the_camera_then_the_pointer() {
    let rings = [
        ring((100.0, 100.0), 5.0),
        ring((104.0, 100.0), 3.0),
        ring((101.0, 100.0), 3.0),
    ];
    // Rings 1 and 2 are nearer the camera than ring 0; ring 2 is nearer the pointer.
    assert_eq!(
        hit_ring(&rings, ScreenPoint::new(100.0, 100.0), 12.0),
        Some(2)
    );
    // A radius that reaches only ring 0 picks it whatever the depth.
    assert_eq!(
        hit_ring(&rings, ScreenPoint::new(89.0, 100.0), 11.5),
        Some(0)
    );
}

#[test]
fn step_ring_wraps_in_both_directions() {
    let rings = [
        ring((0.0, 0.0), 1.0),
        ring((1.0, 0.0), 1.0),
        ring((2.0, 0.0), 1.0),
    ];
    assert_eq!(step_ring(&rings, None, 1), Some(0));
    assert_eq!(step_ring(&rings, None, -1), Some(2));
    assert_eq!(step_ring(&rings, Some(0), 1), Some(1));
    assert_eq!(step_ring(&rings, Some(2), 1), Some(0));
    assert_eq!(step_ring(&rings, Some(0), -1), Some(2));
    assert_eq!(step_ring(&rings, Some(1), -1), Some(0));
    assert_eq!(step_ring(&rings, Some(9), 1), Some(0));
    assert_eq!(step_ring(&rings, None, 0), None);
    assert_eq!(step_ring(&[], None, 1), None);
}

#[test]
fn names_join_for_sentences() {
    let n = |s: &[&str]| s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    assert_eq!(join_names(&n(&["P1"])), "P1");
    assert_eq!(join_names(&n(&["P1", "P2"])), "P1 and P2");
    assert_eq!(join_names(&n(&["P1", "P2", "G1"])), "P1, P2 and G1");
    assert_eq!(join_names(&n(&["A", "B", "C", "D"])), "A, B, C and D");
    assert_eq!(tier_name(&names(), 1), "P1");
    assert_eq!(tier_name(&names(), 9), "tier 10");
    assert_eq!(tier_name(&[String::new()], 0), "tier 1");
}

#[test]
fn texts_match_the_contract() {
    let mut r = ring((0.0, 0.0), 1.0);
    r.names = vec!["P1".into(), "P2".into(), "G1".into()];
    assert_eq!(r.plate_title(), "P1 \u{b7} P2 \u{b7} G1");
    assert_eq!(
        hint_ring(&r, "P3", &names()),
        "Corner of P1, P2 and G1. Click to cut P3 to it."
    );
    assert_eq!(plate_detail(&r, "P3"), "Click to cut P3 to this corner");
    r.kind = PickRingKind::Current;
    assert_eq!(plate_detail(&r, "P3"), "P3 meets here");
    r.kind = PickRingKind::Unnamed;
    r.unnamed_tier = Some(4);
    assert_eq!(
        hint_ring(&r, "P3", &names()),
        "Tier 5 has no name. Name it to pick this corner."
    );
    assert_eq!(
        hint_armed("P3"),
        "Pick the corner P3 is cut to: click a ringed corner. The stone is shown as it is \
         before P3 is cut. Esc cancels."
    );
    assert_eq!(
        hint_none_visible("P3"),
        "No corner P3 can meet is on this side. Orbit the stone, or press Esc."
    );
    assert_eq!(
        toast_picked("P3", &["P1".into(), "P2".into(), "G1".into()]),
        "P3 now meets the corner of P1, P2 and G1."
    );
    assert_eq!(toast_cleared("P3"), "P3 no longer has a picked corner.");
    assert_eq!(accessible_ring(2, 9, &r), "Corner 3 of 9: P1, P2 and G1.");
    assert_eq!(
        blocked_text(MeetPickBlock::NothingCutBefore, "P1", "crown"),
        "Nothing is cut before P1, so there is no corner to pick."
    );
    assert_eq!(
        blocked_text(MeetPickBlock::SoleBlockAnchor, "P1", "crown"),
        "P1 is the only tier that sets the size of the crown. Give another crown tier an \
         exact scale value first."
    );
    assert_eq!(
        blocked_text(MeetPickBlock::SizingTier, "P1", "crown"),
        "P1 sets the stone's size and cannot meet a corner."
    );
}
