//! Pick mode for the meet point: where the corner rings of the stone cut before a tier sit on
//! screen, which of them can be seen, which one a pointer or the keyboard is on, and every word
//! the mode shows.
//!
//! The corners come from `Design::pick_corners`; this module only places them. Everything works
//! in the pick frame (see the [`manipulate`](super) module docs), takes the frame the stone was
//! drawn with, and knows nothing about a window, so the desktop and the web app share it.
//!
//! # Which corners get a ring
//!
//! A corner is drawn when
//!
//! 1. it is a corner point of the shown mesh ([`FrameGeometry::corner_points`], within
//!    [`CORNER_MATCH_FRACTION`] of the bounding radius): a corner that a concave tool removed
//!    from the stone is not offered,
//! 2. it projects in front of the camera, and
//! 3. it is not hidden: the pick buffer holds a facet of one of the corner's tiers within
//!    [`VISIBLE_RADIUS_PX`] of its pixel.
//!
//! # Order
//!
//! [`pick_rings`] returns the rings in a fixed order that the arrow keys walk
//! ([`step_ring`]): by screen angle around the projected stone centre, from the left going
//! clockwise on screen, then by distance from that centre. The order depends only on the
//! camera, never on the pointer.

use super::{
    projection::{ScreenPoint, ScreenSize, project},
    target::camera_for_pose,
};
use glam::Vec3;
use pleochroma_cut_core::{MeetPickBlock, PickCorner, TierMeetPick};
use pleochroma_solid::preview::{FrameGeometry, PickBuffer};
use std::cmp::Ordering;

/// How close (pixels) a corner's pixel must be to a pixel of one of its facets in the pick
/// buffer for the corner to count as visible.
pub const VISIBLE_RADIUS_PX: f32 = 3.0;

/// How near a corner of the stone must lie to a corner point of the shown mesh, as a fraction
/// of the mesh's bounding radius, to be a corner of what is on screen.
pub const CORNER_MATCH_FRACTION: f64 = 1e-4;

/// How close (logical pixels, before the larger-handles scale) the pointer must be to a ring.
pub const RING_HIT_RADIUS_PX: f32 = 12.0;

/// What a ring means to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickRingKind {
    /// A corner the tier can be cut to.
    Valid,
    /// A corner where a tier has no name, so the names rule cannot say it. Shown, not pickable.
    Unnamed,
    /// The corner the tier is already cut to.
    Current,
}

impl PickRingKind {
    /// The code the Slint `MeetPickRing.kind` carries: 0 valid, 1 unnamed, 2 current pick.
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::Valid => 0,
            Self::Unnamed => 1,
            Self::Current => 2,
        }
    }
}

/// One corner ring on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct PickRing {
    /// Index of the corner in the slice given to [`pick_rings`].
    pub corner: usize,
    /// The ring's centre in the pick frame.
    pub at: ScreenPoint,
    /// Distance of the corner from the camera plane; smaller is nearer.
    pub depth: f32,
    /// What the ring means.
    pub kind: PickRingKind,
    /// The tiers whose facets meet at the corner, in cutting order.
    pub tiers: Vec<usize>,
    /// The names of `tiers` (an unnamed tier reads `tier N`).
    pub names: Vec<String>,
    /// The tier without a name at this corner, if any.
    pub unnamed_tier: Option<usize>,
}

impl PickRing {
    /// Whether clicking or accepting this ring cuts the tier to it.
    #[must_use]
    pub const fn pickable(&self) -> bool {
        !matches!(self.kind, PickRingKind::Unnamed)
    }

    /// The corner's tier names joined for a plate: `P1 · P2 · G1`.
    #[must_use]
    pub fn plate_title(&self) -> String {
        self.names.join(" \u{b7} ")
    }
}

/// The name of tier `index` for text: its name, or `tier N` when it has none or `tier_names`
/// does not reach it.
#[must_use]
pub fn tier_name(tier_names: &[String], index: usize) -> String {
    match tier_names.get(index) {
        Some(name) if !name.is_empty() => name.clone(),
        _ => format!("tier {}", index + 1),
    }
}

/// Whether `current` is the corner `corner` offers: every facet of the stored pick is one of
/// the corner's three, whatever their order.
fn is_current(corner: &PickCorner, current: Option<&TierMeetPick>) -> bool {
    current.is_some_and(|stored| {
        stored.facets.iter().all(|facet| {
            corner
                .pick
                .facets
                .iter()
                .any(|offered| offered.tier == facet.tier && offered.index == facet.index)
        })
    })
}

/// Whether the pick buffer holds a facet of `tiers` within [`VISIBLE_RADIUS_PX`] of `at`.
fn is_visible(
    pick: &PickBuffer,
    at: ScreenPoint,
    tiers: &[usize],
    tier_of: &impl Fn(u32) -> Option<usize>,
) -> bool {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a pixel coordinate of a frame that fits in memory is far below i32::MAX"
    )]
    let (cx, cy) = (at.x.round() as i32, at.y.round() as i32);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the window radius is a small whole number of pixels"
    )]
    let reach = VISIBLE_RADIUS_PX.ceil() as i32;
    let limit = VISIBLE_RADIUS_PX * VISIBLE_RADIUS_PX;
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            #[expect(
                clippy::cast_precision_loss,
                reason = "the offsets are within a few pixels"
            )]
            let dist2 = (dx * dx + dy * dy) as f32;
            if dist2 > limit {
                continue;
            }
            let (Ok(x), Ok(y)) = (u32::try_from(cx + dx), u32::try_from(cy + dy)) else {
                continue;
            };
            if let Some(facet) = pick.facet_at(x, y)
                && tier_of(facet).is_some_and(|tier| tiers.contains(&tier))
            {
                return true;
            }
        }
    }
    false
}

/// The rings of `corners` on `frame`, in the stable keyboard order (see the module docs).
///
/// - `pick` is the pick buffer of the frame; a corner it does not show is dropped.
/// - `tier_names[i]` is the name of tier `i` (empty for none).
/// - `tier_of` maps a facet id of the frame to its flat tier (`FacetMap::tier_of`).
/// - `current` is the pick the tier being edited already has; its corner gets
///   [`PickRingKind::Current`].
///
/// A corner whose tier has no name gets [`PickRingKind::Unnamed`] and is not pickable.
#[must_use]
pub fn pick_rings(
    corners: &[PickCorner],
    frame: &FrameGeometry,
    pick: &PickBuffer,
    tier_names: &[String],
    tier_of: impl Fn(u32) -> Option<usize>,
    current: Option<&TierMeetPick>,
) -> Vec<PickRing> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "frame sizes are far below the f32 integer limit"
    )]
    let size = ScreenSize::new(frame.size.0 as f32, frame.size.1 as f32);
    let camera = camera_for_pose(frame.camera);
    let corner_tol = CORNER_MATCH_FRACTION * frame.bounding_radius;
    let mut rings: Vec<PickRing> = corners
        .iter()
        .enumerate()
        .filter_map(|(corner, c)| {
            let point: Vec3 = c.point.as_vec3();
            let on_mesh = frame
                .corner_points
                .iter()
                .any(|p| f64::from(p.distance(point)) <= corner_tol);
            if !on_mesh {
                return None;
            }
            let at = project(&camera, point, size)?;
            if !is_visible(pick, at, &c.tiers, &tier_of) {
                return None;
            }
            let kind = if c.unnamed_tier.is_some() {
                PickRingKind::Unnamed
            } else if is_current(c, current) {
                PickRingKind::Current
            } else {
                PickRingKind::Valid
            };
            Some(PickRing {
                corner,
                at,
                depth: (point - camera.origin).dot(camera.forward),
                kind,
                tiers: c.tiers.clone(),
                names: c.tiers.iter().map(|&t| tier_name(tier_names, t)).collect(),
                unnamed_tier: c.unnamed_tier,
            })
        })
        .collect();
    let centre = project(&camera, Vec3::ZERO, size)
        .unwrap_or_else(|| ScreenPoint::new(size.width * 0.5, size.height * 0.5));
    let key = |ring: &PickRing| {
        let (dx, dy) = centre.delta_to(ring.at);
        (dy.atan2(dx), dx.hypot(dy))
    };
    rings.sort_by(|a, b| {
        let (angle_a, dist_a) = key(a);
        let (angle_b, dist_b) = key(b);
        angle_a
            .total_cmp(&angle_b)
            .then_with(|| dist_a.total_cmp(&dist_b))
            .then_with(|| a.corner.cmp(&b.corner))
    });
    rings
}

/// The ring under `p`: the ones within `radius_px`, the one nearest the camera first, then the
/// one nearest the pointer. `None` when no ring is in reach.
#[must_use]
pub fn hit_ring(rings: &[PickRing], p: ScreenPoint, radius_px: f32) -> Option<usize> {
    rings
        .iter()
        .enumerate()
        .filter_map(|(i, ring)| {
            let dist = ring.at.distance(p);
            (dist <= radius_px).then_some((i, ring.depth, dist))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.2.total_cmp(&b.2)))
        .map(|(i, _, _)| i)
}

/// The ring one step from `lit` through `rings` in their order, wrapping at both ends.
///
/// With nothing lit (or a stale `lit` past the end) a forward step lights the first ring and a
/// backward step the last. `None` when there are no rings or `delta` is 0 with nothing lit.
#[must_use]
pub fn step_ring(rings: &[PickRing], lit: Option<usize>, delta: i32) -> Option<usize> {
    let len = isize::try_from(rings.len()).ok()?;
    if len == 0 {
        return None;
    }
    let lit = lit.filter(|&i| i < rings.len());
    let Some(current) = lit else {
        return match delta.cmp(&0) {
            Ordering::Greater => Some(0),
            Ordering::Less => Some(rings.len() - 1),
            Ordering::Equal => None,
        };
    };
    let from = isize::try_from(current).ok()?;
    let step = isize::try_from(delta).ok()?;
    usize::try_from((from + step).rem_euclid(len)).ok()
}

/// `A`, `A and B`, `A, B and C`: names joined for a sentence.
#[must_use]
pub fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [only] => only.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// `tier N` with its first letter capitalized, for the start of a sentence.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The hint strip while armed and a ring is on screen.
#[must_use]
pub fn hint_armed(tier: &str) -> String {
    format!(
        "Pick the corner {tier} is cut to: click a ringed corner. The stone is shown as it is \
         before {tier} is cut. Esc cancels."
    )
}

/// The hint strip while armed and no ring is on screen.
#[must_use]
pub fn hint_none_visible(tier: &str) -> String {
    format!("No corner {tier} can meet is on this side. Orbit the stone, or press Esc.")
}

/// The hint strip while `ring` is lit: what the corner is and what a click does, or why the
/// corner cannot be picked.
#[must_use]
pub fn hint_ring(ring: &PickRing, tier: &str, tier_names: &[String]) -> String {
    ring.unnamed_tier.map_or_else(
        || {
            format!(
                "Corner of {}. Click to cut {tier} to it.",
                join_names(&ring.names)
            )
        },
        |unnamed| {
            format!(
                "{} has no name. Name it to pick this corner.",
                capitalized(&tier_name(tier_names, unnamed))
            )
        },
    )
}

/// The second line of the plate over a lit ring (the first is [`PickRing::plate_title`]).
#[must_use]
pub fn plate_detail(ring: &PickRing, tier: &str) -> String {
    match ring.kind {
        PickRingKind::Current => format!("{tier} meets here"),
        PickRingKind::Valid => format!("Click to cut {tier} to this corner"),
        PickRingKind::Unnamed => "Name the tier to pick this corner".to_owned(),
    }
}

/// The toast after `tier` was cut to the corner of `names`.
#[must_use]
pub fn toast_picked(tier: &str, names: &[String]) -> String {
    format!("{tier} now meets the corner of {}.", join_names(names))
}

/// The toast after the picked corner of `tier` was cleared.
#[must_use]
pub fn toast_cleared(tier: &str) -> String {
    format!("{tier} no longer has a picked corner.")
}

/// The accessible label of the lit ring: `Corner k of n: A, B and C.` with `k` counted from 1.
#[must_use]
pub fn accessible_ring(position: usize, count: usize, ring: &PickRing) -> String {
    format!(
        "Corner {} of {count}: {}.",
        position + 1,
        join_names(&ring.names)
    )
}

/// Why `tier` cannot be armed. `block` names the block whose size the tier alone sets.
#[must_use]
pub fn blocked_text(reason: MeetPickBlock, tier: &str, block: &str) -> String {
    match reason {
        MeetPickBlock::NothingCutBefore => {
            format!("Nothing is cut before {tier}, so there is no corner to pick.")
        }
        MeetPickBlock::SoleBlockAnchor => format!(
            "{tier} is the only tier that sets the size of the {block}. Give another {block} \
             tier an exact scale value first."
        ),
        MeetPickBlock::SizingTier => {
            format!("{tier} sets the stone's size and cannot meet a corner.")
        }
    }
}

/// The tooltip of the *Pick in View* button.
#[must_use]
pub fn pick_button_tooltip(tier: &str) -> String {
    format!(
        "Show the stone as it is before {tier} is cut and click the corner {tier} should meet. \
         Shortcut: M."
    )
}

/// The tooltip of the *Clear* button.
#[must_use]
pub fn clear_button_tooltip(tier: &str) -> String {
    format!(
        "Forget the picked corner. {tier} keeps its named facets and meets their shallowest \
         point."
    )
}
