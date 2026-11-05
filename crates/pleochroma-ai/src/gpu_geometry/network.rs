//! Numeric preservation network adapted from cut-core meet preservation.
//!
//! Keep IDENTIFY, basis selection, row ordering,
//! imported-mast targets and gauge arithmetic in agreement with that reference.
use glam::{DVec3, Vec3};
use pleochroma::geometry::{
    cuts::{StandardGemCuts, normals_coincide},
    meet_solver::{MeetConstraint, SolveControl, SolveStrategy, SolvedTier},
    plane::{GpuFacetPlane, tier_is_crown_side},
    stone_metrics::{SolidMesh, SolidStatus, build_solid_mesh, feasible_plane_triples},
};
use pleochroma_cut_core::Design;

pub(super) type Plane = (DVec3, f64);

#[derive(Debug, Clone)]
pub(super) struct Probe {
    pub rows: Option<Vec<f64>>,
    /// False only when outward-rounded necessary conditions prove impossibility.
    pub possible: bool,
}

pub(super) fn scratch_per_probe(network: &Network) -> usize {
    network.surfaces * 4 + network.rows + 2 + network.junctions.len() * 7
}

#[derive(Debug, Clone)]
pub(super) struct Junction {
    pub point: DVec3,
    pub support: Vec<usize>,
    pub basis: [usize; 3],
    pub targets: Vec<f64>,
    pub bound: f64,
    /// Determinant of the basis normals in the original stone. It bounds how far the
    /// basis point can sit from any point that is within `bound` of the basis planes.
    pub basis_det: f64,
}

#[derive(Debug, Clone)]
pub(super) struct Network {
    pub junctions: Vec<Junction>,
    pub scale: f64,
    pub radial: f64,
    pub gauge: Vec<usize>,
    pub height: f64,
    pub surfaces: usize,
    pub rows: usize,
    pub width: f64,
    pub half_length: f64,
    pub reaches: Vec<Option<f64>>,
}

/// Exact production plane conversion for pinned masts.
///
/// No authoring solve is needed:
/// every SolvedTier value is the supplied ScaleReference, as in solve/context.rs.
/// Named meets and dimensional targets retain their exact authored CPU solve;
/// the resulting plane batches use the same GPU residual network.
pub(super) fn surfaces(design: &Design) -> Option<Vec<Plane>> {
    if !design.concave_tiers.is_empty() {
        return None;
    }
    let solved: Vec<_> = if direct_masts(design) {
        design
            .tiers
            .iter()
            .map(|tier| {
                let MeetConstraint::ScaleReference(mast) = tier.constraint else {
                    return None;
                };
                (mast.is_finite() && mast != 0.0).then_some(SolvedTier {
                    mast: mast.abs(),
                    strategy: SolveStrategy::ScaleReference,
                    detail: String::new(),
                })
            })
            .collect::<Option<_>>()?
    } else {
        checked_solve(design)?
    };
    surfaces_from_solved(design, &solved)
}

fn surfaces_from_solved(design: &Design, solved: &[SolvedTier]) -> Option<Vec<Plane>> {
    let planes = design.planes_from_solved(solved);
    let mut all = design.preform.planes_offset(design.preform_y_offset);
    let offset = all.len();
    let schedule = design.to_asc_schedule_from_solved(solved);
    for aliases in StandardGemCuts::asc_plane_provenance(&schedule) {
        for plane in aliases {
            all.push(*planes.get(offset + plane)?);
        }
    }
    all.iter()
        .all(|(n, d)| n.is_finite() && d.is_finite())
        .then_some(all)
}

/// Construct production-equivalent planes without an ASC schedule.
///
/// Deduplication remains quadratic in the number of distinct planes, as in
/// production. Aliases use the rotated first retained representative, including
/// when their own tier has a cheater offset.
pub(super) fn direct_surfaces(design: &Design) -> Option<Vec<Plane>> {
    let gear = design.meta.gear_teeth.unsigned_abs().max(1) as f32;
    let reference = design.meta.gear_reference_angle as f32;
    let mut retained: Vec<(GpuFacetPlane, Plane)> = Vec::new();
    let mut result = design.preform.planes_offset(design.preform_y_offset);
    for (tier_index, tier) in design.tiers.iter().enumerate() {
        let MeetConstraint::ScaleReference(mast) = tier.constraint else {
            return None;
        };
        if !mast.is_finite() || mast == 0.0 {
            return None;
        }
        let theta = (tier.angle_deg.abs() as f32).to_radians();
        let (sin_theta, cos_theta) = (theta.sin(), theta.cos());
        let y = if tier_is_crown_side(tier.angle_deg) {
            cos_theta
        } else {
            -cos_theta
        };
        let d = -(mast.abs() as f32);
        for position in 0..tier.indices.len().max(1) {
            let normal = if let Some(&index) = tier.indices.get(position) {
                let phi = StandardGemCuts::index_to_azimuth(index as f32, gear, reference);
                let (sin_phi, cos_phi) = (phi.sin(), phi.cos());
                Vec3::new(sin_theta * cos_phi, y, sin_theta * sin_phi)
            } else {
                Vec3::new(0.0, y, sin_theta)
            };
            let raw = GpuFacetPlane::new(normal, d);
            let raw_normal = raw.to_halfspace_f64().0;
            let representative = retained.iter().position(|(other, _)| {
                let scale = other.d.abs().max(raw.d.abs()).max(1.0);
                normals_coincide(other.to_halfspace_f64().0, raw_normal)
                    && (other.d - raw.d).abs() <= (1.0 / 2048.0) * scale
            });
            let plane = if let Some(index) = representative {
                retained[index].1
            } else {
                let mut plane = raw.to_halfspace_f64();
                if let Some(offset) = design.cheater_offset_deg(tier_index)
                    && offset != 0.0
                {
                    plane.0 = glam::DMat3::from_rotation_y(-offset.to_radians()) * plane.0;
                }
                retained.push((raw, plane));
                plane
            };
            result.push(plane);
        }
    }
    result
        .iter()
        .all(|(n, d)| n.is_finite() && d.is_finite())
        .then_some(result)
}

pub(super) fn direct_masts(design: &Design) -> bool {
    (0..design.tiers.len()).all(|i| design.tier_target(i).is_none())
        && design
            .tiers
            .iter()
            .all(|t| matches!(t.constraint, MeetConstraint::ScaleReference(_)))
}

/// Exact checked authored solve from meet_preservation/mod.rs; a failed strategy
/// is an invalid residual probe, never a usable approximate mast.
fn checked_solve(design: &Design) -> Option<Vec<SolvedTier>> {
    let solved = design.solve_with(&SolveControl::default()).ok()?;
    (solved.len() == design.tiers.len()
        && solved
            .iter()
            .all(|s| s.mast.is_finite() && !matches!(s.strategy, SolveStrategy::Failed)))
    .then_some(solved)
}

fn best_basis(surfaces: &[Plane], support: &[usize]) -> Option<[usize; 3]> {
    let mut result = None;
    let mut best = 1e-8;
    for (i, &a) in support.iter().enumerate() {
        for (k, &b) in support.iter().enumerate().skip(i + 1) {
            for &c in support.iter().skip(k + 1) {
                let det = surfaces[a].0.dot(surfaces[b].0.cross(surfaces[c].0)).abs();
                if det > best {
                    best = det;
                    result = Some([a, b, c]);
                }
            }
        }
    }
    result
}

fn extent(mesh: &SolidMesh) -> Option<(DVec3, DVec3, f64)> {
    let low = mesh
        .rings
        .iter()
        .flat_map(|(_, r)| r)
        .copied()
        .fold(DVec3::splat(f64::INFINITY), DVec3::min);
    let high = mesh
        .rings
        .iter()
        .flat_map(|(_, r)| r)
        .copied()
        .fold(DVec3::splat(f64::NEG_INFINITY), DVec3::max);
    let scale = (high - low).max_element();
    (scale.is_finite() && scale > 0.0).then_some((low, high, scale))
}

fn canonical_junctions(
    planes: &[Plane],
    triples: Vec<([usize; 3], DVec3)>,
    scale: f64,
) -> Option<Vec<Junction>> {
    let identify = scale * 5e-5;
    let mut clusters: Vec<(DVec3, Vec<usize>)> = Vec::new();
    for (triple, p) in triples {
        if planes.iter().any(|(n, d)| n.dot(p) > d + identify) {
            continue;
        }
        if let Some(i) = clusters.iter().position(|(q, _)| q.distance(p) <= identify) {
            clusters[i].1.extend(triple);
        } else {
            clusters.push((p, triple.to_vec()));
        }
    }
    let mut junctions = Vec::new();
    for (p, mut support) in clusters {
        support.extend(
            planes
                .iter()
                .enumerate()
                .filter(|(_, (n, d))| (n.dot(p) - d).abs() <= identify)
                .map(|(i, _)| i),
        );
        support.sort_unstable();
        support.dedup();
        let basis = best_basis(planes, &support)?;
        let at = point(planes, basis)?;
        let targets = support
            .iter()
            .filter(|i| !basis.contains(i))
            .map(|&i| {
                let row = (planes[i].0.dot(at) - planes[i].1) / scale;
                if row.abs() * scale > scale * 2e-6 {
                    row
                } else {
                    0.0
                }
            })
            .collect();
        let bound = (scale * 2e-6).max(
            support
                .iter()
                .map(|&s| (planes[s].0.dot(p) - planes[s].1).abs())
                .fold(0.0, f64::max),
        );
        let [a, b, c] = basis.map(|i| planes[i].0);
        junctions.push(Junction {
            point: p,
            support,
            basis,
            targets,
            bound,
            basis_det: a.dot(b.cross(c)),
        });
    }
    // Canonical drop_covered: duplicate supports retain their first cluster.
    let keep: Vec<_> = junctions
        .iter()
        .enumerate()
        .map(|(i, j)| {
            !junctions.iter().enumerate().any(|(k, other)| {
                i != k
                    && j.support.iter().all(|s| other.support.contains(s))
                    && (j.support.len() < other.support.len() || k < i)
            })
        })
        .collect();
    let junctions: Vec<_> = junctions
        .into_iter()
        .zip(keep)
        .filter_map(|(j, keep)| keep.then_some(j))
        .collect();
    (!junctions.is_empty()).then_some(junctions)
}

fn original_gauge(junctions: &[Junction], identify: f64) -> (f64, Vec<usize>, f64) {
    let radial = junctions
        .iter()
        .map(|j| j.point.x.hypot(j.point.z))
        .fold(0.0, f64::max);
    let gauge: Vec<_> = junctions
        .iter()
        .enumerate()
        .filter(|(_, j)| (j.point.x.hypot(j.point.z) - radial).abs() <= identify)
        .map(|(i, _)| i)
        .collect();
    let height =
        gauge.iter().map(|&i| junctions[i].point.y).sum::<f64>() / gauge.len().max(1) as f64;
    (radial, gauge, height)
}

pub(super) fn point(planes: &[Plane], basis: [usize; 3]) -> Option<DVec3> {
    let [a, b, c] = basis.map(|i| planes[i]);
    let det = a.0.dot(b.0.cross(c.0));
    if det.abs() <= 1e-8 {
        return None;
    }
    let p = (b.0.cross(c.0) * a.1 + c.0.cross(a.0) * b.1 + a.0.cross(b.0) * c.1) / det;
    p.is_finite().then_some(p)
}

impl Network {
    /// The arrangement proof is optional: larger searches use the original gate.
    ///
    /// Limit it to one existing 32K-triple tile and at most 1M worst-case junction
    /// flag/support checks. Bounding both avoids a single tile with many junctions
    /// monopolizing the device. This is a work ceiling, not an invalidity test.
    #[cfg(feature = "cuda")]
    pub(super) fn arrangement_triples_within_budget(&self) -> Option<u64> {
        arrangement_triples_within_budget(
            self.surfaces,
            self.junctions.iter().map(|junction| junction.support.len()),
        )
    }

    /// The cheap rejection kernel may use this network only when every value
    /// governing its proof agrees bit-for-bit with production CPU capture.
    #[cfg(feature = "cuda")]
    pub(super) fn same_proof_metadata(&self, other: &Self) -> bool {
        self.surfaces == other.surfaces
            && self.scale.to_bits() == other.scale.to_bits()
            && self.radial.to_bits() == other.radial.to_bits()
            && self.height.to_bits() == other.height.to_bits()
            && self.gauge == other.gauge
            && self.junctions.len() == other.junctions.len()
            && self.junctions.iter().zip(&other.junctions).all(|(a, b)| {
                a.support == b.support
                    && a.basis == b.basis
                    && a.point.x.to_bits() == b.point.x.to_bits()
                    && a.point.y.to_bits() == b.point.y.to_bits()
                    && a.point.z.to_bits() == b.point.z.to_bits()
                    && a.bound.to_bits() == b.bound.to_bits()
            })
    }

    #[cfg(test)]
    pub fn capture(design: &Design) -> Option<Self> {
        Self::capture_with_solid(design, None).ok().flatten()
    }

    pub fn capture_with_solid(
        design: &Design,
        solid: Option<&crate::gpu_solid::GpuSolid>,
    ) -> Result<Option<Self>, String> {
        if !design.concave_tiers.is_empty() {
            return Ok(None);
        }
        let Some(solved) = checked_solve(design) else {
            return Ok(None);
        };
        Self::capture_prepared(design, solid, &solved)
    }

    /// `solved` must belong to this immutable design, retained while MoveSpace
    /// measures its hinges/features. Never reuse this across an authored edit.
    pub fn capture_prepared(
        design: &Design,
        solid: Option<&crate::gpu_solid::GpuSolid>,
        solved: &[SolvedTier],
    ) -> Result<Option<Self>, String> {
        if solved.len() != design.tiers.len()
            || solved
                .iter()
                .any(|s| !s.mast.is_finite() || matches!(s.strategy, SolveStrategy::Failed))
        {
            return Ok(None);
        }
        let Some(planes) = surfaces_from_solved(design, solved) else {
            return Ok(None);
        };
        let production = design.planes_from_solved(solved);
        let status = if let Some(solid) = solid {
            solid.build_and_measure(&production, &[])?.status
        } else {
            build_solid_mesh(&production)
        };
        let SolidStatus::Closed(mesh) = status else {
            return Ok(None);
        };
        let Some((low, high, scale)) = extent(&mesh) else {
            return Ok(None);
        };
        let identify = scale * 5e-5;
        let triples = if let Some(solid) = solid {
            solid.feasible_plane_triples(&planes, scale)?
        } else {
            feasible_plane_triples(&planes, scale)
        };
        let Some(triples) = triples else {
            return Ok(None);
        };
        let Some(junctions) = canonical_junctions(&planes, triples, scale) else {
            return Ok(None);
        };
        let (radial, gauge, height) = original_gauge(&junctions, identify);
        let rows = junctions.iter().map(|j| j.targets.len() + 1).sum::<usize>() + 2;
        let preform_planes = design.preform.planes_offset(design.preform_y_offset);
        let offset = preform_planes.len();
        let mut owners = vec![None; offset];
        let schedule = design.to_asc_schedule_from_solved(solved);
        for (tier, aliases) in StandardGemCuts::asc_plane_provenance(&schedule)
            .iter()
            .enumerate()
        {
            owners.extend(aliases.iter().map(|_| Some(tier)));
        }
        let reaches = super::reach::tier_reaches(
            design.tiers.len(),
            &planes,
            &owners,
            &junctions,
            scale,
            identify,
        );
        Ok(Some(Self {
            junctions,
            scale,
            radial,
            gauge,
            height,
            surfaces: planes.len(),
            rows,
            width: (high.x - low.x).min(high.z - low.z),
            half_length: 0.5 * (high.x - low.x).max(high.z - low.z),
            reaches,
        }))
    }

    /// Construct probe planes with production provenance.
    ///
    /// Direct masts use the production f32 plane and first-representative rules.
    /// Rebuild provenance on every probe: even an unchanged plane count does not
    /// imply unchanged alias ownership. Other authoring uses the checked solver.
    pub(super) fn surfaces_for_probe(design: &Design) -> Option<Vec<Plane>> {
        if !design.concave_tiers.is_empty() {
            return None;
        }
        if direct_masts(design) {
            direct_surfaces(design)
        } else {
            surfaces(design)
        }
    }

    /// Independent CPU reference for the exact GPU numeric contract. A numerical
    /// residual is never a preservation certificate; the original gate still decides.
    pub fn residuals(&self, planes: &[Plane]) -> Option<Vec<f64>> {
        if planes.len() != self.surfaces {
            return None;
        }
        let mut rows = Vec::with_capacity(self.rows);
        let mut points = Vec::with_capacity(self.junctions.len());
        for j in &self.junctions {
            let p = point(planes, j.basis)?;
            points.push(p);
            rows.extend(
                j.support
                    .iter()
                    .filter(|s| !j.basis.contains(s))
                    .zip(&j.targets)
                    .map(|(&s, &target)| (planes[s].0.dot(p) - planes[s].1) / self.scale - target),
            );
            rows.push(
                planes
                    .iter()
                    .enumerate()
                    .filter(|(s, _)| !j.support.contains(s))
                    .map(|(_, (n, d))| (n.dot(p) - d) / self.scale)
                    .fold(0.0, f64::max),
            );
        }
        let radial = points.iter().map(|p| p.x.hypot(p.z)).fold(0.0, f64::max);
        let height =
            self.gauge.iter().map(|&i| points[i].y).sum::<f64>() / self.gauge.len().max(1) as f64;
        rows.extend([
            (radial - self.radial) / self.scale,
            (height - self.height) / self.scale,
        ]);
        rows.iter().all(|r| r.is_finite()).then_some(rows)
    }

    pub fn turn_travel(&self, original: &Design, candidate: &Design) -> f64 {
        self.reaches
            .iter()
            .zip(pleochroma_cut_core::meet_preservation::tangent_changes(
                original, candidate,
            ))
            .filter(|(_, change)| *change > 0.0)
            .map(|(reach, change)| reach.unwrap_or(self.half_length) * change)
            .fold(0.0, f64::max)
    }
}

#[cfg(any(feature = "cuda", test))]
fn arrangement_triples_within_budget(
    surfaces: usize,
    mut support_counts: impl Iterator<Item = usize>,
) -> Option<u64> {
    if !(3..=256).contains(&surfaces) {
        return None;
    }
    let triples = surfaces * (surfaces - 1) * (surfaces - 2) / 6;
    if triples > 32_768 {
        return None;
    }
    let checks_per_triple = support_counts.try_fold(0usize, |checks, supports| {
        checks.checked_add(supports.checked_add(1)?)
    })?;
    let checks = triples.checked_mul(checks_per_triple)?;
    (checks <= 1_048_576).then_some(triples as u64)
}

#[cfg(test)]
mod arrangement_budget_tests {
    use super::arrangement_triples_within_budget;

    #[test]
    fn proof_budget_bounds_triples_and_support_checks_independently() {
        assert_eq!(
            arrangement_triples_within_budget(59, [3].into_iter()),
            Some(32_509)
        );
        assert_eq!(arrangement_triples_within_budget(60, [3].into_iter()), None);
        assert_eq!(
            arrangement_triples_within_budget(32, [3; 52].into_iter()),
            Some(4_960)
        );
        assert_eq!(
            arrangement_triples_within_budget(32, [3; 53].into_iter()),
            None
        );
        assert_eq!(
            arrangement_triples_within_budget(4, [usize::MAX].into_iter()),
            None
        );
        assert_eq!(
            arrangement_triples_within_budget(257, [3].into_iter()),
            None
        );
    }
}
