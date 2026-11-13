//! Leader labels: a facet too small to hold its label gets it outside the stone, joined to the
//! facet by a thin [`LineWeight::Leader`] line.
//!
//! The parent module's "Labels" section states the rules; [`place_leader_labels`] applies them.
//! Every search walks rays from a point inside the facet in a fixed order of turns and steps, and
//! every list is kept in facet order, so the result is the same from run to run.

use super::{
    LABEL_EM_PER_CHAR, LABEL_SHRINK, LABEL_SIZE, LONG_TICK_OUTER, LabelSpot, LineWeight, Piece,
    TICK_INNER, VectorLabel, VectorLine, VectorView, text_half,
};
use crate::diagram2d::PanelLayout;
use std::collections::BTreeSet;

/// The gap a leader label keeps to the stone's silhouette and to the index wheel's ticks, in ems
/// of the label.
const LEADER_CLEARANCE_EMS: f32 = 0.15;
/// The gap a leader label keeps to any other text, in ems of the label.
const LEADER_TEXT_GAP_EMS: f32 = 0.2;
/// How far short of its label's box a leader line stops, in ems of the label.
pub(super) const LEADER_END_GAP_EMS: f32 = 0.15;
/// How far a leader label moves out along a ray between two tries in the band, in ems of the
/// label: a fine step, since the band is not much taller than a label.
const BAND_STEP_EMS: f32 = 0.1;
/// How far a leader label moves out along a ray between two tries around the stone, in ems of
/// the label.
const AROUND_STEP_EMS: f32 = 0.25;
/// The most steps tried along one ray, far past any view's diagonal.
const MAX_RAY_STEPS: u16 = 4096;
/// The turn between two directions tried in the band, in degrees.
const BAND_TURN_DEG: f32 = 4.0;
/// How many turns the band tries to each side of the outward direction.
const BAND_TURNS: u16 = 6;
/// The turn between two directions tried around the stone, in degrees.
const AROUND_TURN_DEG: f32 = 10.0;
/// How many turns to each side are tried around the stone first: a quarter turn.
const AROUND_NEAR_TURNS: u16 = 9;
/// How many turns to each side are tried around the stone in all: the whole circle.
const AROUND_TURNS: u16 = 18;
/// The caption's estimated width per character, in ems: its capitals are wider than a tier code's
/// letter and digit.
pub(super) const CAPTION_EM_PER_CHAR: f32 = 0.8;

/// A label placed outside its facet, joined to it by a leader line.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct LeaderLabel {
    /// The facet the label names.
    pub(super) facet_id: usize,
    /// The label, at the smaller label size.
    pub(super) label: VectorLabel,
    /// The leader, from a point inside the facet to just short of the label's box; `None` when
    /// the label covers that point (only on a spot that may sit on the stone).
    pub(super) line: Option<VectorLine>,
}

/// How tidy a leader label's spot must be, from the tidiest to any spot at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Strictness {
    /// Inside the view, clear of the stone, the ticks and every text; the leader crosses no
    /// text and no other leader.
    Clear,
    /// As [`Self::Clear`], but the leader may cross other leaders.
    CrossLeaders,
    /// Inside the view and clear of every text only: the label may sit on the stone or the ticks.
    OverStone,
    /// Anywhere: the last resort that gives a tier its only label in a view with no room left.
    Forced,
}

impl Strictness {
    /// Every level, tidiest first.
    const ALL: [Self; 4] = [
        Self::Clear,
        Self::CrossLeaders,
        Self::OverStone,
        Self::Forced,
    ];
}

/// An axis-aligned box of text, in drawing units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TextBox {
    /// The box's centre.
    centre: (f32, f32),
    /// Half its width and half its height.
    half: (f32, f32),
}

impl TextBox {
    /// The box of `label`, `em_per_char` ems wide per character.
    pub(super) fn of_label(label: &VectorLabel, em_per_char: f32) -> Self {
        Self {
            centre: (label.x, label.y),
            half: text_half(&label.text, label.size, em_per_char),
        }
    }

    /// The box grown by `by` on every side.
    const fn grown(self, by: f32) -> Self {
        Self {
            centre: self.centre,
            half: (self.half.0 + by, self.half.1 + by),
        }
    }

    /// Whether the two boxes share some area (touching is not overlapping).
    pub(super) const fn overlaps(self, other: Self) -> bool {
        (self.centre.0 - other.centre.0).abs() < self.half.0 + other.half.0
            && (self.centre.1 - other.centre.1).abs() < self.half.1 + other.half.1
    }

    /// Whether the box lies inside a drawing `width` x `height` units.
    pub(super) const fn inside(self, width: f32, height: f32) -> bool {
        self.centre.0 - self.half.0 >= 0.0
            && self.centre.1 - self.half.1 >= 0.0
            && self.centre.0 + self.half.0 <= width
            && self.centre.1 + self.half.1 <= height
    }

    /// The distance from `point` to the box (`0` inside it).
    pub(super) fn nearest_distance(self, point: (f32, f32)) -> f32 {
        let dx = ((point.0 - self.centre.0).abs() - self.half.0).max(0.0);
        let dy = ((point.1 - self.centre.1).abs() - self.half.1).max(0.0);
        dx.hypot(dy)
    }

    /// The distance from `point` to the box's farthest corner.
    pub(super) fn farthest_distance(self, point: (f32, f32)) -> f32 {
        ((point.0 - self.centre.0).abs() + self.half.0)
            .hypot((point.1 - self.centre.1).abs() + self.half.1)
    }

    /// Whether the segment `from`-`to` runs through the box (Liang-Barsky clipping).
    fn crossed_by(self, from: (f32, f32), to: (f32, f32)) -> bool {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let (left, top) = (self.centre.0 - self.half.0, self.centre.1 - self.half.1);
        let (right, bottom) = (self.centre.0 + self.half.0, self.centre.1 + self.half.1);
        let mut enter = 0.0f32;
        let mut leave = 1.0f32;
        for (step, room) in [
            (-dx, from.0 - left),
            (dx, right - from.0),
            (-dy, from.1 - top),
            (dy, bottom - from.1),
        ] {
            if step == 0.0 {
                if room < 0.0 {
                    return false;
                }
                continue;
            }
            let at = room / step;
            if step < 0.0 {
                enter = enter.max(at);
            } else {
                leave = leave.min(at);
            }
            if enter >= leave {
                return false;
            }
        }
        true
    }
}

/// `a . b`.
const fn dot(a: (f32, f32), b: (f32, f32)) -> f32 {
    a.0 * b.0 + a.1 * b.1
}

/// Twice the signed area of the triangle `origin`, `a`, `b`: positive when `b` lies to the left
/// of `origin -> a` in the drawing's own axes.
const fn orientation(origin: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - origin.0) * (b.1 - origin.1) - (a.1 - origin.1) * (b.0 - origin.0)
}

/// The point `distance` along the unit `direction` from `origin`.
const fn along_ray(origin: (f32, f32), direction: (f32, f32), distance: f32) -> (f32, f32) {
    (
        origin.0 + direction.0 * distance,
        origin.1 + direction.1 * distance,
    )
}

/// Whether two orientations lie strictly on opposite sides of zero.
const fn opposite(first: f32, second: f32) -> bool {
    (first > 0.0 && second < 0.0) || (first < 0.0 && second > 0.0)
}

/// Whether the segments `first_from`-`first_to` and `second_from`-`second_to` cross at a point
/// inside both.
pub(super) const fn segments_cross(
    first_from: (f32, f32),
    first_to: (f32, f32),
    second_from: (f32, f32),
    second_to: (f32, f32),
) -> bool {
    opposite(
        orientation(second_from, second_to, first_from),
        orientation(second_from, second_to, first_to),
    ) && opposite(
        orientation(first_from, first_to, second_from),
        orientation(first_from, first_to, second_to),
    )
}

/// `vector` turned by `angle` radians.
fn rotated(vector: (f32, f32), angle: f32) -> (f32, f32) {
    let (sin, cos) = angle.sin_cos();
    (
        cos.mul_add(vector.0, -(sin * vector.1)),
        sin.mul_add(vector.0, cos * vector.1),
    )
}

/// The turns tried from a label's outward direction, in radians: none, then `step_deg` more to
/// each side in turn, `count` steps to each side (a half turn only once).
fn turn_angles(step_deg: f32, count: u16) -> Vec<f32> {
    let mut angles = vec![0.0];
    for step in 1..=count {
        let degrees = f32::from(step) * step_deg;
        angles.push(degrees.to_radians());
        if degrees < 180.0 {
            angles.push((-degrees).to_radians());
        }
    }
    angles
}

/// The fraction of `along` that `reach` is, or infinity when `along` is about zero.
const fn reach_fraction(reach: f32, along: f32) -> f32 {
    if along.abs() > f32::EPSILON {
        reach / along.abs()
    } else {
        f32::INFINITY
    }
}

/// The leader from `anchor` to a label centred on `centre` (`half` its box's half size), stopping
/// `gap` short of the box. `None` when `anchor` lies within that.
const fn leader_ends(
    anchor: (f32, f32),
    centre: (f32, f32),
    half: (f32, f32),
    gap: f32,
) -> Option<((f32, f32), (f32, f32))> {
    let toward = (anchor.0 - centre.0, anchor.1 - centre.1);
    let fraction =
        reach_fraction(half.0 + gap, toward.0).min(reach_fraction(half.1 + gap, toward.1));
    if fraction >= 1.0 {
        return None;
    }
    Some((
        anchor,
        (
            centre.0 + fraction * toward.0,
            centre.1 + fraction * toward.1,
        ),
    ))
}

/// The convex hull of `points` (sorted by `x`, then `y`, without repeats), counter-clockwise in
/// the drawing's own axes (Andrew's monotone chain); fewer than three corners when the points
/// are collinear.
fn convex_ring(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut lower: Vec<(f32, f32)> = Vec::with_capacity(points.len());
    for &point in points {
        while lower.len() >= 2
            && orientation(lower[lower.len() - 2], lower[lower.len() - 1], point) <= 0.0
        {
            lower.pop();
        }
        lower.push(point);
    }
    let mut upper: Vec<(f32, f32)> = Vec::with_capacity(points.len());
    for &point in points.iter().rev() {
        while upper.len() >= 2
            && orientation(upper[upper.len() - 2], upper[upper.len() - 1], point) <= 0.0
        {
            upper.pop();
        }
        upper.push(point);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// One edge of a [`Hull`].
struct HullEdge {
    /// The edge's outward unit normal.
    normal: (f32, f32),
    /// The hull's largest projection onto `normal`: the edge's own line.
    high: f32,
    /// The hull's smallest projection onto `normal`.
    low: f32,
}

/// The convex hull of a view's drawn pieces: the stone's silhouette (a hull around it, for a
/// concave stone), which a leader label keeps clear of.
struct Hull {
    /// Every edge, counter-clockwise; none when the corners are collinear.
    edges: Vec<HullEdge>,
    /// The lowest and the highest corner coordinate on each axis; `None` without any corner.
    bounds: Option<((f32, f32), (f32, f32))>,
}

impl Hull {
    /// The hull of every corner of `pieces`.
    fn of(pieces: &[Piece]) -> Self {
        let mut points: Vec<(f32, f32)> = pieces
            .iter()
            .flat_map(|piece| piece.points.iter().copied())
            .filter(|point| point.0.is_finite() && point.1.is_finite())
            .collect();
        points.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        points.dedup();
        let bounds = points.first().map(|&first| {
            points.iter().fold((first, first), |(low, high), &point| {
                (
                    (low.0.min(point.0), low.1.min(point.1)),
                    (high.0.max(point.0), high.1.max(point.1)),
                )
            })
        });
        let ring = convex_ring(&points);
        let mut edges = Vec::with_capacity(ring.len());
        if ring.len() >= 3 {
            for (index, &from) in ring.iter().enumerate() {
                let to = ring[(index + 1) % ring.len()];
                let length = (to.0 - from.0).hypot(to.1 - from.1);
                if length <= 0.0 {
                    continue;
                }
                let normal = ((to.1 - from.1) / length, (from.0 - to.0) / length);
                let low = ring
                    .iter()
                    .map(|&point| dot(normal, point))
                    .fold(f32::INFINITY, f32::min);
                edges.push(HullEdge {
                    normal,
                    high: dot(normal, from),
                    low,
                });
            }
        }
        Self { edges, bounds }
    }

    /// Whether `area` keeps at least `gap` from the hull: some axis separates them, either axis
    /// of the box or an edge's normal.
    fn clear_of(&self, area: TextBox, gap: f32) -> bool {
        let Some((low, high)) = self.bounds else {
            return true;
        };
        let (left, top) = (area.centre.0 - area.half.0, area.centre.1 - area.half.1);
        let (right, bottom) = (area.centre.0 + area.half.0, area.centre.1 + area.half.1);
        if left >= high.0 + gap
            || right <= low.0 - gap
            || top >= high.1 + gap
            || bottom <= low.1 - gap
        {
            return true;
        }
        self.edges.iter().any(|edge| {
            let middle = dot(edge.normal, area.centre);
            let reach = area
                .half
                .0
                .mul_add(edge.normal.0.abs(), area.half.1 * edge.normal.1.abs());
            middle - reach >= edge.high + gap || middle + reach <= edge.low - gap
        })
    }

    /// How far a ray from `from` along the unit `direction` runs before it leaves the hull; `0`
    /// from outside it or without edges.
    fn exit_distance(&self, from: (f32, f32), direction: (f32, f32)) -> f32 {
        let mut exit = f32::INFINITY;
        for edge in &self.edges {
            let along = dot(edge.normal, direction);
            if along > 1e-6 {
                exit = exit.min((edge.high - dot(edge.normal, from)) / along);
            }
        }
        if exit.is_finite() { exit.max(0.0) } else { 0.0 }
    }
}

/// The index wheel's ticks: a ring around the view's centre.
#[derive(Debug, Clone, Copy)]
struct TickRing {
    /// Where the ticks start, in drawing units from the centre.
    inner: f32,
    /// Where the long ticks end.
    outer: f32,
}

impl TickRing {
    /// Whether `area` keeps `gap` from the ticks around `centre`: wholly inside the ring or
    /// wholly outside it.
    fn clear_of(self, area: TextBox, centre: (f32, f32), gap: f32) -> bool {
        area.farthest_distance(centre) <= self.inner - gap
            || area.nearest_distance(centre) >= self.outer + gap
    }
}

/// What a new leader label and its leader must keep clear of.
struct Taken {
    /// The box of every text so far: the labels inside their facets, the tooth numbers, the
    /// caption and the leader labels placed before.
    boxes: Vec<TextBox>,
    /// The ends of every leader line placed before.
    leaders: Vec<((f32, f32), (f32, f32))>,
}

impl Taken {
    /// The boxes of the labels of `view` so far and of its caption.
    fn of(view: &VectorView) -> Self {
        let mut boxes: Vec<TextBox> = view
            .labels
            .iter()
            .map(|label| TextBox::of_label(label, LABEL_EM_PER_CHAR))
            .collect();
        boxes.push(TextBox::of_label(
            &view.caption_label(),
            CAPTION_EM_PER_CHAR,
        ));
        Self {
            boxes,
            leaders: Vec::new(),
        }
    }
}

/// One leader label being placed.
#[derive(Debug, Clone, Copy)]
struct Seek {
    /// Where its leader starts, inside the facet.
    anchor: (f32, f32),
    /// The unit direction its spot is first looked for in.
    outward: (f32, f32),
    /// Half its box's width and height.
    half: (f32, f32),
    /// How tidy the spot must be.
    strictness: Strictness,
}

/// Where the leader labels of one view may go: the drawing, the stone's hull, the wheel's ticks.
struct LeaderSpace {
    /// The drawing's width, in drawing units.
    width: f32,
    /// The drawing's height.
    height: f32,
    /// The view's centre: the stone's axis, or the middle of its girdle on a side view.
    centre: (f32, f32),
    /// The stone's hull.
    hull: Hull,
    /// The index wheel's ticks, on the crown and pavilion views only.
    ticks: Option<TickRing>,
    /// The leader labels' font size: the smaller label size.
    size: f32,
}

impl LeaderSpace {
    /// The space of `view`, its other labels and its wheel already drawn, around `pieces`.
    fn new(view: &VectorView, layout: &PanelLayout, pieces: &[Piece]) -> Self {
        let ticks = if layout.kind.has_index_wheel() {
            Some(TickRing {
                inner: layout.wheel_radius_px * TICK_INNER,
                outer: layout.wheel_radius_px * LONG_TICK_OUTER,
            })
        } else {
            None
        };
        Self {
            width: view.width,
            height: view.height,
            centre: (layout.center_x, layout.center_y),
            hull: Hull::of(pieces),
            ticks,
            size: view.width.min(view.height) * LABEL_SIZE * LABEL_SHRINK,
        }
    }

    /// The gap a leader label keeps to the stone and the ticks.
    const fn clearance(&self) -> f32 {
        self.size * LEADER_CLEARANCE_EMS
    }

    /// The gap a leader label keeps to other text.
    const fn text_gap(&self) -> f32 {
        self.size * LEADER_TEXT_GAP_EMS
    }

    /// How far short of its label's box a leader stops.
    const fn end_gap(&self) -> f32 {
        self.size * LEADER_END_GAP_EMS
    }

    /// Whether a label centred on `centre` has left the drawing entirely.
    const fn left_view(&self, centre: (f32, f32), half: (f32, f32)) -> bool {
        centre.0 < -half.0
            || centre.0 > self.width + half.0
            || centre.1 < -half.1
            || centre.1 > self.height + half.1
    }

    /// The direction the label of a facet at `anchor` is first looked for in. On a side view
    /// straight up or down from the girdle, on the facet's own side; elsewhere outward from the
    /// centre through the facet (straight up for a facet on the axis).
    fn outward(&self, anchor: (f32, f32)) -> (f32, f32) {
        let away = (anchor.0 - self.centre.0, anchor.1 - self.centre.1);
        if self.ticks.is_none() {
            return if away.1 >= 0.0 {
                (0.0, 1.0)
            } else {
                (0.0, -1.0)
            };
        }
        let length = away.0.hypot(away.1);
        if length > self.size * 1e-3 {
            (away.0 / length, away.1 / length)
        } else {
            (0.0, -1.0)
        }
    }

    /// The leader label of `spot` at `strictness`, its box and its leader added to `taken`;
    /// `None` when no spot that tidy is free (never at [`Strictness::Forced`]).
    fn place(
        &self,
        taken: &mut Taken,
        spot: &LabelSpot<'_>,
        strictness: Strictness,
    ) -> Option<LeaderLabel> {
        let seek = Seek {
            anchor: spot.anchor,
            outward: self.outward(spot.anchor),
            half: text_half(spot.text, self.size, LABEL_EM_PER_CHAR),
            strictness,
        };
        let centre = if strictness == Strictness::Forced {
            self.forced_spot(seek)
        } else {
            self.band_spot(taken, seek)
                .or_else(|| self.around_spot(taken, seek))?
        };
        let line = leader_ends(seek.anchor, centre, seek.half, self.end_gap());
        taken.boxes.push(TextBox {
            centre,
            half: seek.half,
        });
        taken.leaders.extend(line);
        Some(LeaderLabel {
            facet_id: spot.facet_id,
            label: VectorLabel {
                text: spot.text.to_string(),
                x: centre.0,
                y: centre.1,
                size: self.size,
            },
            line: line.map(|(from, to)| VectorLine {
                from,
                to,
                weight: LineWeight::Leader,
            }),
        })
    }

    /// The first free spot of the band between the stone and the wheel's ticks, along the
    /// outward direction or a few turns from it; `None` on a side view and for a label that may
    /// sit on the stone.
    fn band_spot(&self, taken: &Taken, seek: Seek) -> Option<(f32, f32)> {
        let ticks = self.ticks?;
        if seek.strictness > Strictness::CrossLeaders {
            return None;
        }
        let limit = ticks.inner - self.clearance();
        let step = self.size * BAND_STEP_EMS;
        for angle in turn_angles(BAND_TURN_DEG, BAND_TURNS) {
            let direction = rotated(seek.outward, angle);
            let start = self.hull.exit_distance(seek.anchor, direction);
            for index in 0..MAX_RAY_STEPS {
                let centre = along_ray(
                    seek.anchor,
                    direction,
                    f32::from(index).mul_add(step, start),
                );
                let area = TextBox {
                    centre,
                    half: seek.half,
                };
                if area.farthest_distance(self.centre) > limit {
                    break;
                }
                if self.is_free(taken, seek, centre) {
                    return Some(centre);
                }
            }
        }
        None
    }

    /// The free spot nearest the facet around the stone: along the directions within a quarter
    /// turn of the outward one first, then round the rest of the circle.
    fn around_spot(&self, taken: &Taken, seek: Seek) -> Option<(f32, f32)> {
        let near = turn_angles(AROUND_TURN_DEG, AROUND_NEAR_TURNS);
        let all = turn_angles(AROUND_TURN_DEG, AROUND_TURNS);
        self.nearest_spot(taken, seek, &near)
            .or_else(|| self.nearest_spot(taken, seek, &all[near.len()..]))
    }

    /// The free spot nearest the facet along the turns `angles` from the outward direction: the
    /// first free step of each ray, the nearest of those.
    fn nearest_spot(&self, taken: &Taken, seek: Seek, angles: &[f32]) -> Option<(f32, f32)> {
        let step = self.size * AROUND_STEP_EMS;
        let mut best: Option<(f32, (f32, f32))> = None;
        for &angle in angles {
            let direction = rotated(seek.outward, angle);
            let start = if seek.strictness <= Strictness::CrossLeaders {
                self.hull.exit_distance(seek.anchor, direction)
            } else {
                0.0
            };
            for index in 0..MAX_RAY_STEPS {
                let distance = f32::from(index).mul_add(step, start);
                if best.is_some_and(|(nearest, _)| distance >= nearest) {
                    break;
                }
                let centre = along_ray(seek.anchor, direction, distance);
                if self.left_view(centre, seek.half) {
                    break;
                }
                if self.is_free(taken, seek, centre) {
                    best = Some((distance, centre));
                    break;
                }
            }
        }
        best.map(|(_, centre)| centre)
    }

    /// The last-resort spot: just outside the stone along the outward direction, moved into the
    /// drawing.
    fn forced_spot(&self, seek: Seek) -> (f32, f32) {
        let distance = self.hull.exit_distance(seek.anchor, seek.outward)
            + seek.half.0.hypot(seek.half.1)
            + self.clearance();
        let centre = along_ray(seek.anchor, seek.outward, distance);
        (
            centre.0.min(self.width - seek.half.0).max(seek.half.0),
            centre.1.min(self.height - seek.half.1).max(seek.half.1),
        )
    }

    /// Whether a label centred on `centre` is free at `seek.strictness`.
    fn is_free(&self, taken: &Taken, seek: Seek, centre: (f32, f32)) -> bool {
        let area = TextBox {
            centre,
            half: seek.half,
        };
        if !area.inside(self.width, self.height) {
            return false;
        }
        let keeps_off_stone = seek.strictness <= Strictness::CrossLeaders;
        if keeps_off_stone && !self.clear_of_stone(area) {
            return false;
        }
        let padded = area.grown(self.text_gap());
        if taken.boxes.iter().any(|&other| padded.overlaps(other)) {
            return false;
        }
        !keeps_off_stone || self.leader_is_clear(taken, seek, centre, padded)
    }

    /// Whether `area` keeps clear of the stone's hull and the wheel's ticks.
    fn clear_of_stone(&self, area: TextBox) -> bool {
        let gap = self.clearance();
        self.hull.clear_of(area, gap)
            && self
                .ticks
                .is_none_or(|ticks| ticks.clear_of(area, self.centre, gap))
    }

    /// Whether no earlier leader runs through the label (`padded`, its box grown) and its own
    /// leader crosses no text (nor, at [`Strictness::Clear`], another leader).
    fn leader_is_clear(
        &self,
        taken: &Taken,
        seek: Seek,
        centre: (f32, f32),
        padded: TextBox,
    ) -> bool {
        if taken
            .leaders
            .iter()
            .any(|&(from, to)| padded.crossed_by(from, to))
        {
            return false;
        }
        let Some((from, to)) = leader_ends(seek.anchor, centre, seek.half, self.end_gap()) else {
            return true;
        };
        if taken.boxes.iter().any(|other| other.crossed_by(from, to)) {
            return false;
        }
        seek.strictness != Strictness::Clear
            || !taken
                .leaders
                .iter()
                .any(|&(start, end)| segments_cross(from, to, start, end))
    }

    /// One leader label for the tier `text`: at the tidiest [`Strictness`] any of its facets in
    /// `waiting` allows, the earlier facet first, with that facet's index in `waiting`. `None`
    /// only when no facet of the tier is waiting.
    fn place_tier(
        &self,
        taken: &mut Taken,
        waiting: &[&LabelSpot<'_>],
        text: &str,
    ) -> Option<(usize, LeaderLabel)> {
        for strictness in Strictness::ALL {
            for (index, spot) in waiting.iter().enumerate() {
                if spot.text != text {
                    continue;
                }
                if let Some(leader) = self.place(taken, spot, strictness) {
                    return Some((index, leader));
                }
            }
        }
        None
    }
}

/// The leader labels of every spot whose label did not fit inside its facet (`fitted` false),
/// in facet order.
///
/// First every tier (the facets sharing one label text) without a label inside a facet gets one,
/// tier by tier in the order of their first facet, at the tidiest [`Strictness`] that has room.
/// Then each remaining facet gets one where a [`Strictness::Clear`] spot is still free, so a view
/// that runs out of room drops the extra labels of tiers that are labelled already, never a
/// tier's only label.
pub(super) fn place_leader_labels(
    view: &VectorView,
    layout: &PanelLayout,
    pieces: &[Piece],
    spots: &[LabelSpot<'_>],
    fitted: &[bool],
) -> Vec<LeaderLabel> {
    let waiting: Vec<&LabelSpot<'_>> = spots
        .iter()
        .zip(fitted)
        .filter(|&(_, &fits)| !fits)
        .map(|(spot, _)| spot)
        .collect();
    if waiting.is_empty() {
        return Vec::new();
    }
    let labelled: BTreeSet<&str> = spots
        .iter()
        .zip(fitted)
        .filter(|&(_, &fits)| fits)
        .map(|(spot, _)| spot.text)
        .collect();
    let space = LeaderSpace::new(view, layout, pieces);
    let mut taken = Taken::of(view);
    let mut placed: Vec<LeaderLabel> = Vec::new();
    let mut done = vec![false; waiting.len()];
    let mut tiers_seen: BTreeSet<&str> = BTreeSet::new();
    for spot in &waiting {
        if labelled.contains(spot.text) || !tiers_seen.insert(spot.text) {
            continue;
        }
        if let Some((index, leader)) = space.place_tier(&mut taken, &waiting, spot.text) {
            done[index] = true;
            placed.push(leader);
        }
    }
    for (spot, &already) in waiting.iter().zip(&done) {
        if already {
            continue;
        }
        if let Some(leader) = space.place(&mut taken, spot, Strictness::Clear) {
            placed.push(leader);
        }
    }
    placed.sort_by_key(|leader| leader.facet_id);
    placed
}
