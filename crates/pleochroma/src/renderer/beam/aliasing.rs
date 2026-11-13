//! The spectral aliasing check (beam guide §9.3).
//!
//! A fixed node set renders a sharp light seen through strong dispersion as 48 discrete colored
//! copies. The copies blend when the exit-direction step between neighbouring nodes is below the
//! lighting's angular feature size `σ_L` (§11.3). [`worst_node_step`] measures that step per leaf
//! (the frame driver folds the worst one into `BeamStats::worst_node_step`) and [`aliasing_risk`]
//! compares it with `σ_L / 2`. The check is a measurement only: the node count stays a setting, and
//! rendering is not changed by it.

use glam::DVec3;

use super::beam::Leaf;

/// The angle between two vectors in radians, `None` when either has no direction (zero or not
/// finite).
fn angle_between(a: DVec3, b: DVec3) -> Option<f64> {
    if !(a.is_finite() && b.is_finite()) || a.length_squared() <= 0.0 || b.length_squared() <= 0.0 {
        return None;
    }
    Some(a.cross(b).length().atan2(a.dot(b)))
}

/// The largest angle, in radians, between the exit directions of two neighbouring nodes at any
/// vertex of `leaf` (§9.3).
///
/// The directions are the leaf's per-vertex, per-node `dirs` (vertex-major). A leaf of one node, or
/// whose directions do not depend on the node (a non-dispersive stone, a reflection leaf), gives
/// `0.0`; a direction that is zero or not finite is skipped.
#[must_use]
pub fn worst_node_step(leaf: &Leaf) -> f64 {
    let nodes = leaf.nodes();
    if nodes < 2 {
        return 0.0;
    }
    let mut worst = 0.0_f64;
    for vertex in leaf.dirs.chunks_exact(nodes) {
        for pair in vertex.windows(2) {
            if let Some(angle) = angle_between(pair[0].as_dvec3(), pair[1].as_dvec3()) {
                worst = worst.max(angle);
            }
        }
    }
    worst
}

/// `true` when the node step of `leaf` exceeds half the lighting's angular feature size `sigma_l`
/// (radians): the 48 colored copies of a sharp light would then stay apart (§9.3).
#[must_use]
pub fn aliasing_risk(leaf: &Leaf, sigma_l: f64) -> bool {
    worst_node_step(leaf) > 0.5 * sigma_l
}
