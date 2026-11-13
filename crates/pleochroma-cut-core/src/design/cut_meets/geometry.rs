//! The vertex bookkeeping [`super`] shares between the stone cut so far and the finished stone.
//!
//! It holds the stone's bounds, the box that closes it without a preform, welding face corners
//! into vertices, and which tiers of the finished stone touch.

use super::{ON_AXIS_FRACTION, SHARED_VERTEX_FRACTION};
use glam::DVec3;
use pleochroma::geometry::stone_metrics::SolidMesh;
use std::cmp::Ordering;

/// Half the side of the box that closes the stone cut so far when the preform has no planes,
/// in multiples of the finished stone's size: the box is four times the stone's extent.
const CLOSING_BOX_HALF_SIDE: f64 = 2.0;

/// The smallest and largest corner coordinates of `mesh`'s faces: its axis-aligned bounds.
pub(super) fn ring_bounds(mesh: &SolidMesh) -> (DVec3, DVec3) {
    mesh.rings.iter().flat_map(|(_, ring)| ring.iter()).fold(
        (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
        |(lo, hi), &point| (lo.min(point), hi.max(point)),
    )
}

/// The axis-aligned box that closes the stone cut so far when the preform has no planes:
/// centred on the stone's bounds `lo`..`hi`, four times its `size` on a side.
pub(super) fn closing_box(lo: DVec3, hi: DVec3, size: f64) -> Vec<(DVec3, f64)> {
    let centre = (lo + hi) * 0.5;
    let half_side = CLOSING_BOX_HALF_SIDE * size;
    [
        DVec3::X,
        DVec3::NEG_X,
        DVec3::Y,
        DVec3::NEG_Y,
        DVec3::Z,
        DVec3::NEG_Z,
    ]
    .into_iter()
    .map(|normal| (normal, normal.dot(centre) + half_side))
    .collect()
}

/// The distinct vertices among `corners` (a face corner and what its face is: the facet, or
/// `None` for a closing face), each with what every face it is a corner of is, duplicates
/// kept.
///
/// Corners within `tolerance` of each other are one vertex, placed where its first corner is.
/// Sorts `corners` by position first; the vertices come in that order.
pub(super) fn weld<T: Copy>(corners: &mut [(DVec3, T)], tolerance: f64) -> Vec<(DVec3, Vec<T>)> {
    corners.sort_by(|a, b| {
        a.0.x
            .total_cmp(&b.0.x)
            .then_with(|| a.0.y.total_cmp(&b.0.y))
            .then_with(|| a.0.z.total_cmp(&b.0.z))
    });
    let mut parent: Vec<usize> = (0..corners.len()).collect();
    // Swept along `x`: two corners can only be one vertex when their `x` differ by at most
    // `tolerance`.
    for (i, &(p, _)) in corners.iter().enumerate() {
        for (j, &(q, _)) in corners.iter().enumerate().skip(i + 1) {
            if q.x - p.x > tolerance {
                break;
            }
            if p.distance(q) <= tolerance {
                join(&mut parent, i, j);
            }
        }
    }
    let mut vertex_of: Vec<Option<usize>> = vec![None; corners.len()];
    let mut vertices: Vec<(DVec3, Vec<T>)> = Vec::new();
    for (i, &(point, face)) in corners.iter().enumerate() {
        // A set's representative is its smallest index (see `join`), so it comes first.
        let leader = find(&mut parent, i);
        if leader == i {
            vertex_of[i] = Some(vertices.len());
            vertices.push((point, Vec::new()));
        }
        if let Some(vertex) = vertex_of[leader].and_then(|vertex| vertices.get_mut(vertex)) {
            vertex.1.push(face);
        }
    }
    vertices
}

/// The representative of `index`'s set in the union-find forest `parent` (the smallest index of
/// the set, see [`join`]), halving the path on the way.
const fn find(parent: &mut [usize], mut index: usize) -> usize {
    while parent[index] != index {
        parent[index] = parent[parent[index]];
        index = parent[index];
    }
    index
}

/// Joins the sets of `a` and `b` in `parent`; the smaller representative represents the union,
/// so a set's representative is always its smallest index.
fn join(parent: &mut [usize], a: usize, b: usize) {
    let (first, second) = (find(parent, a), find(parent, b));
    match first.cmp(&second) {
        Ordering::Less => parent[second] = first,
        Ordering::Greater => parent[first] = second,
        Ordering::Equal => {}
    }
}

/// What the finished stone says about its tiers: which tiers' facets share a vertex, and
/// which tiers' facets all end in one point on the axis. Built once by [`StoneContacts::of`].
pub(super) struct StoneContacts {
    /// How many flat tiers the design has.
    tier_count: usize,
    /// `touching[a * tier_count + b]`: a facet of tier `a` and a facet of tier `b` share a
    /// vertex. Symmetric.
    touching: Vec<bool>,
    /// Per tier: at least three facets, and all of them share one vertex on the axis.
    converges: Vec<bool>,
}

impl StoneContacts {
    /// The contacts of the finished stone `mesh`, each face given to its tier by `owner` (the
    /// plane index's tier, [`super::owned_planes`]). Preform faces belong to no tier and take
    /// no part. `size` is the stone's size.
    pub(super) fn of(
        mesh: &SolidMesh,
        owner: &[Option<usize>],
        tier_count: usize,
        size: f64,
    ) -> Self {
        let faces: Vec<(usize, &[DVec3])> = mesh
            .rings
            .iter()
            .filter_map(|(plane, ring)| {
                let tier = owner
                    .get(*plane)
                    .copied()
                    .flatten()
                    .filter(|&tier| tier < tier_count)?;
                Some((tier, ring.as_slice()))
            })
            .collect();
        let shared = SHARED_VERTEX_FRACTION * size;
        let on_axis = ON_AXIS_FRACTION * size;

        let mut touching = vec![false; tier_count * tier_count];
        // Every facet corner with its tier, swept along `x`: two corners can only be one
        // vertex when their `x` differ by at most `shared`.
        let mut corners: Vec<(DVec3, usize)> = faces
            .iter()
            .flat_map(|&(tier, ring)| ring.iter().map(move |&p| (p, tier)))
            .collect();
        corners.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
        for (i, &(p, a)) in corners.iter().enumerate() {
            for &(q, b) in corners[i + 1..]
                .iter()
                .take_while(|(next, _)| next.x - p.x <= shared)
            {
                if a != b && p.distance(q) <= shared {
                    touching[a * tier_count + b] = true;
                    touching[b * tier_count + a] = true;
                }
            }
        }

        let mut rings_of: Vec<Vec<&[DVec3]>> = vec![Vec::new(); tier_count];
        for &(tier, ring) in &faces {
            rings_of[tier].push(ring);
        }
        let converges = rings_of
            .iter()
            .map(|rings| meet_in_one_axis_point(rings, shared, on_axis))
            .collect();

        Self {
            tier_count,
            touching,
            converges,
        }
    }

    /// Whether a facet of tier `a` shares a vertex with a facet of tier `b`.
    pub(super) fn touch(&self, a: usize, b: usize) -> bool {
        self.touching
            .get(a * self.tier_count + b)
            .copied()
            .unwrap_or(false)
    }

    /// Whether `tier` has at least three facets and all of them end in one point on the axis.
    pub(super) fn ends_in_axis_point(&self, tier: usize) -> bool {
        self.converges.get(tier).copied().unwrap_or(false)
    }
}

/// `true` when there are at least three `rings` and one corner of the first, within
/// `on_axis` of the stone's axis (`x = z = 0`), is (within `shared`) a corner of every one.
fn meet_in_one_axis_point(rings: &[&[DVec3]], shared: f64, on_axis: f64) -> bool {
    let [first, rest @ ..] = rings else {
        return false;
    };
    rings.len() >= 3
        && first
            .iter()
            .filter(|p| p.x.hypot(p.z) <= on_axis)
            .any(|&p| {
                rest.iter()
                    .all(|ring| ring.iter().any(|&q| p.distance(q) <= shared))
            })
}
