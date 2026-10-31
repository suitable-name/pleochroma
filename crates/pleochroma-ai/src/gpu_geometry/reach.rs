//! Adapted from cut-core meet_preservation/{reach,network_topology}. The exact
//! original face cycles decide which facets have a measurable inward reach.
use super::network::{Junction, Plane};
use glam::DVec3;

pub(super) fn tier_reaches(
    tiers: usize,
    planes: &[Plane],
    owners: &[Option<usize>],
    junctions: &[Junction],
    scale: f64,
    identify: f64,
) -> Vec<Option<f64>> {
    let mut result: Vec<Option<f64>> = vec![None; tiers];
    for (surface, (normal, _)) in planes.iter().enumerate() {
        let Some(tier) = owners[surface] else {
            continue;
        };
        let members: Vec<_> = (0..junctions.len())
            .filter(|&j| junctions[j].support.contains(&surface))
            .collect();
        if members.len() < 3 {
            continue;
        }
        let cycle = ordered(*normal, &members, junctions);
        let points: Vec<_> = cycle.iter().map(|&i| junctions[i].point).collect();
        if !non_collinear(&points, identify) {
            continue;
        }
        let first = points[0];
        let area = points
            .windows(2)
            .skip(1)
            .map(|p| (p[0] - first).cross(p[1] - first))
            .sum::<DVec3>()
            .length()
            * 0.5;
        if area <= scale * scale * 1e-12 {
            continue;
        }
        let horizontal = normal.x.hypot(normal.z);
        if horizontal <= 1e-9 {
            continue;
        }
        let azimuth = DVec3::new(normal.x / horizontal, 0.0, normal.z / horizontal);
        let outer = junctions
            .iter()
            .map(|j| azimuth.dot(j.point))
            .fold(f64::NEG_INFINITY, f64::max);
        let inner = points
            .iter()
            .map(|p| azimuth.dot(*p))
            .fold(f64::INFINITY, f64::min);
        let reach = outer - inner;
        if reach.is_finite() {
            result[tier] = Some(result[tier].map_or(reach.max(0.0), |old| old.max(reach.max(0.0))));
        }
    }
    result
}

fn non_collinear(points: &[DVec3], tolerance: f64) -> bool {
    let Some(&first) = points.first() else {
        return false;
    };
    let far = points.iter().copied().fold(first, |best, p| {
        if p.distance(first) > best.distance(first) {
            p
        } else {
            best
        }
    });
    let axis = far - first;
    let length = axis.length();
    length > tolerance
        && points
            .iter()
            .any(|p| axis.cross(*p - first).length() / length > tolerance)
}

fn ordered(normal: DVec3, members: &[usize], junctions: &[Junction]) -> Vec<usize> {
    let seed = if normal.x.abs() <= normal.y.abs() && normal.x.abs() <= normal.z.abs() {
        DVec3::X
    } else if normal.y.abs() <= normal.z.abs() {
        DVec3::Y
    } else {
        DVec3::Z
    };
    let u = (seed - normal * normal.dot(seed)).normalize();
    let v = normal.cross(u);
    let center = members.iter().map(|&i| junctions[i].point).sum::<DVec3>() / members.len() as f64;
    let mut keyed: Vec<_> = members
        .iter()
        .map(|&i| {
            let d = junctions[i].point - center;
            (v.dot(d).atan2(u.dot(d)), i)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    keyed.into_iter().map(|(_, i)| i).collect()
}
