//! Entry refraction: exact points, fitted apex, subdivision, drop accounting (beam guide §8.3).
//!
//! [`refract_entry`] turns an outside primary beam into the beams inside the stone that its
//! transmitted rays form. Each sample point is expanded by `path_cache::bounce::expand_at` at its
//! exact hit on the carrier facet (§8.1, §13.5). The refracted rays of a plane interface have no
//! common point, so [`fit_apex`] fits the apex closest to all of them. The beam is bisected
//! (`primary::split_beam`) until the direction error is below `τ_dir` and the weight interpolation
//! check of §8.4 passes.
//!
//! The geometry (rays, apex) of a beam comes from the group of 8 nodes that holds the interval's
//! centre node, with that group's hero. The other groups contribute their Stokes vectors only; the
//! interval validity checks of §9.2 are made by the tree driver.

use glam::{DMat3, DVec3, Vec3};

use super::{
    beam::{Beam, GROUP, SampleState, SampleStates},
    birefringence::{beam_near_axis, is_anisotropic},
    geom::polygon::{ConvexPolygon, MAX_VERTICES},
    interact::{InteractKit, KitHeroes},
    leaf::{SPLIT_FLOOR_PX2, WEIGHT_ABS_TOLERANCE, peak_weight},
    primary::{image_polygon, split_beam},
    propagate::retrace_state,
    stats::{BeamStats, DropReason},
};
use crate::{
    optics::raytracer::Ray,
    renderer::path_cache::{
        bounce::{Child, EventKind, Expansion, ExplicitHit, PathState, expand_at},
        signature::branch_step,
    },
};

/// The condition number above which [`fit_apex`] falls back (guide §8.3).
pub const CONDITION_LIMIT: f64 = 1.0e12;

/// The code of a transmit event in a branch step word (`branch_step`).
const TRANSMIT_CODE: u32 = 3;

/// The condition number `λ_max / λ_min` of a symmetric positive semi-definite 3x3 matrix, from
/// its closed-form eigenvalues; infinite when the smallest is not positive.
fn condition_number(m: DMat3) -> f64 {
    let (a00, a11, a22) = (m.x_axis.x, m.y_axis.y, m.z_axis.z);
    let (a01, a02, a12) = (m.y_axis.x, m.z_axis.x, m.z_axis.y);
    let q = (a00 + a11 + a22) / 3.0;
    let off = a01.mul_add(a01, a02.mul_add(a02, a12 * a12));
    let diagonal = (a22 - q).mul_add(a22 - q, (a11 - q).mul_add(a11 - q, (a00 - q) * (a00 - q)));
    let spread = off.mul_add(2.0, diagonal);
    let p = (spread / 6.0).sqrt();
    if p <= 0.0 {
        return if q > 0.0 { 1.0 } else { f64::INFINITY };
    }
    let shifted = (m - DMat3::IDENTITY * q) * (1.0 / p);
    let half_det = (shifted.determinant() * 0.5).clamp(-1.0, 1.0);
    let phi = half_det.acos() / 3.0;
    let largest = (2.0 * p).mul_add(phi.cos(), q);
    let smallest = (2.0 * p).mul_add((phi + std::f64::consts::TAU / 3.0).cos(), q);
    if smallest > 0.0 && largest > 0.0 {
        largest / smallest
    } else {
        f64::INFINITY
    }
}

/// The least-squares point closest to the lines `points[i] + t · dirs[i]` and the condition number
/// of the normal equations. `None` for fewer than two lines, a zero direction or a singular
/// system.
fn least_squares_apex(points: &[DVec3], dirs: &[DVec3]) -> Option<(DVec3, f64)> {
    if points.len() != dirs.len() || points.len() < 2 {
        return None;
    }
    let mut sum = DVec3::ZERO;
    for p in points {
        sum += *p;
    }
    let centre = sum / count_f64(points.len());
    let mut m = DMat3::ZERO;
    let mut b = DVec3::ZERO;
    for (&p, &d) in points.iter().zip(dirs) {
        let d = d.try_normalize()?;
        let projector = DMat3::IDENTITY - DMat3::from_cols(d * d.x, d * d.y, d * d.z);
        m += projector;
        b += projector * (p - centre);
    }
    let cond = condition_number(m);
    if !m.determinant().is_finite() || m.determinant() == 0.0 {
        return None;
    }
    let apex = centre + m.inverse() * b;
    apex.is_finite().then_some((apex, cond))
}

/// The point closest, in the least-squares sense, to the lines `points[i] + t · dirs[i]` (guide
/// §8.3): a closed-form 3x3 solve on the centred points.
///
/// When the condition number of the system exceeds [`CONDITION_LIMIT`] the fit uses only the line
/// nearest to the centroid of the points and the two lines of the two points farthest apart (the
/// centroid and the extreme vertices). `None` for fewer than two lines, mismatched slices, a zero
/// direction, or a set that is still ill-conditioned or singular (parallel lines have no finite
/// apex).
#[must_use]
pub fn fit_apex(points: &[DVec3], dirs: &[DVec3]) -> Option<DVec3> {
    let (apex, cond) = least_squares_apex(points, dirs)?;
    if cond <= CONDITION_LIMIT {
        return Some(apex);
    }
    let subset = reproduction_subset(points);
    let sub_points: Vec<DVec3> = subset.iter().map(|&i| points[i]).collect();
    let sub_dirs: Vec<DVec3> = subset.iter().map(|&i| dirs[i]).collect();
    let (apex, cond) = least_squares_apex(&sub_points, &sub_dirs)?;
    (cond <= CONDITION_LIMIT).then_some(apex)
}

/// The indices (ascending, distinct) of the point nearest the centroid and of the two points
/// farthest apart; the first of equals wins.
fn reproduction_subset(points: &[DVec3]) -> Vec<usize> {
    let mut sum = DVec3::ZERO;
    for p in points {
        sum += *p;
    }
    let centre = sum / count_f64(points.len().max(1));
    let mut nearest = (f64::INFINITY, 0);
    let mut farthest = (-1.0, 0, 0);
    for (i, p) in points.iter().enumerate() {
        let d = p.distance_squared(centre);
        if d < nearest.0 {
            nearest = (d, i);
        }
        for (j, q) in points.iter().enumerate().skip(i + 1) {
            let e = p.distance_squared(*q);
            if e > farthest.0 {
                farthest = (e, i, j);
            }
        }
    }
    let mut indices = vec![nearest.1, farthest.1, farthest.2];
    indices.sort_unstable();
    indices.dedup();
    indices
}

/// A count as `f64` (counts here are tiny).
fn count_f64(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// The angle between two vectors, in radians.
fn angle_between(a: DVec3, b: DVec3) -> f64 {
    a.cross(b).length().atan2(a.dot(b))
}

/// `max_p angle(dirs[p], normalize(hits[p] − apex))`; infinite when a hit coincides with the apex.
fn direction_error(hits: &[DVec3], dirs: &[DVec3], apex: DVec3) -> f64 {
    hits.iter()
        .zip(dirs)
        .map(|(&p, &d)| {
            (p - apex)
                .try_normalize()
                .map_or(f64::INFINITY, |v| angle_between(d, v))
        })
        .fold(0.0, f64::max)
}

/// The group that supplies a beam's geometry: the one holding the interval's centre node.
fn geometry_group(beam: &Beam) -> usize {
    usize::from(beam.spectrum.centre() - beam.spectrum.first) / GROUP
}

/// The exact hit of one sample point and the expansion of every group there.
struct PointExpansion {
    /// The hit point on the carrier plane, in `f64`.
    hit_f64: DVec3,
    /// One expansion per group of the beam's interval.
    groups: Vec<Expansion>,
}

/// Expands the sample `state` of `beam` at its exact hit on the carrier plane.
fn expand_point(
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
    heroes: &KitHeroes<'_, '_>,
    state: &SampleState,
) -> Option<PointExpansion> {
    let stone = kit.retrace.stone;
    let original = stone.original_facet(beam.facet);
    let (n, m) = stone.plane(beam.facet);
    let origin = state.segment_origin.as_dvec3();
    let dir = state.ray.dir.as_dvec3();
    let denominator = n.dot(dir);
    if denominator.abs() <= f64::EPSILON {
        return None;
    }
    let t = (m - n.dot(origin)) / denominator;
    if !(t.is_finite() && t > 0.0) {
        return None;
    }
    let hit_f64 = dir.mul_add(DVec3::splat(t), origin);
    let hit = hit_f64.as_vec3();
    let explicit = ExplicitHit {
        point: hit,
        facet: original as usize,
        normal: Vec3::from(kit.planes.get(original as usize)?.normal),
    };
    let ray = Ray {
        origin: state.segment_origin,
        dir: state.ray.dir,
    };
    let groups = (0..beam.spectrum.groups())
        .map(|g| {
            let path = PathState {
                k: state.k,
                inside: state.inside,
                is_ext: state.is_ext,
                stokes: *state.stokes.get(g)?,
            };
            Some(expand_at(
                heroes.get(g)?,
                kit.planes,
                path,
                ray,
                explicit,
                Some(!state.inside),
                state.prev_plane,
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(PointExpansion { hit_f64, groups })
}

/// The expansions of all sample points of `beam`, in the order of its states.
fn expand_points(
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
    heroes: &KitHeroes<'_, '_>,
) -> Option<Vec<PointExpansion>> {
    beam.state
        .points
        .iter()
        .map(|state| expand_point(beam, kit, heroes, state))
        .collect()
}

/// Child `index` of group `g` at a point.
fn mode_child(point: &PointExpansion, g: usize, index: usize) -> Option<&Child> {
    point.groups.get(g)?.children.get(index)
}

/// The kinds and modes of an expansion's children, in order: the "child set" of §8.1.
fn child_shape(expansion: &Expansion) -> Vec<(EventKind, bool)> {
    expansion
        .children
        .iter()
        .map(|c| (c.kind, c.state.is_ext))
        .collect()
}

/// `true` when every point and group returns the reference child set and no child diverged.
fn same_child_sets(points: &[PointExpansion], reference: &Expansion) -> bool {
    let shape = child_shape(reference);
    points
        .iter()
        .flat_map(|p| p.groups.iter())
        .all(|e| child_shape(e) == shape && e.children.iter().all(|c| !c.diverged))
}

/// The apex fit of transmitted child `index` of the geometry group `g` and its direction error.
fn mode_fit(points: &[PointExpansion], g: usize, index: usize) -> Option<(DVec3, f64)> {
    let mut hits = Vec::with_capacity(points.len());
    let mut dirs = Vec::with_capacity(points.len());
    for point in points {
        hits.push(point.hit_f64);
        dirs.push(mode_child(point, g, index)?.s.as_dvec3());
    }
    let apex = fit_apex(&hits, &dirs)?;
    Some((apex, direction_error(&hits, &dirs, apex)))
}

/// The number of nodes of group `g` of an interval of `nodes` nodes.
fn group_nodes(nodes: usize, g: usize) -> usize {
    nodes.saturating_sub(g * GROUP).min(GROUP)
}

/// `true` when the exact Stokes intensity at the centroid and the edge midpoints matches the
/// Wachspress interpolation from the vertices within `τ_w · max(I_vertices) + τ_abs`, for every
/// node of mode `index` (§8.4).
fn interpolates(
    beam: &Beam,
    image: &ConvexPolygon,
    points: &[PointExpansion],
    index: usize,
    tau_w: f32,
) -> bool {
    let vertices = beam.state.vertices;
    if points.len() != beam.state.points.len() || points.len() <= vertices {
        return false;
    }
    let checks: Vec<[f64; MAX_VERTICES]> = beam.state.points[vertices..]
        .iter()
        .map(|s| image.wachspress(s.image))
        .collect();
    let nodes = beam.spectrum.len();
    for g in 0..beam.spectrum.groups() {
        for slot in 0..group_nodes(nodes, g) {
            let value = |p: &PointExpansion| {
                mode_child(p, g, index)
                    .and_then(|c| c.state.stokes.get(slot))
                    .map(|s| f64::from(s.i))
            };
            let Some(corner): Option<Vec<f64>> = points[..vertices].iter().map(value).collect()
            else {
                return false;
            };
            let peak = corner.iter().copied().fold(0.0, f64::max);
            let tolerance = f64::from(tau_w).mul_add(peak, f64::from(WEIGHT_ABS_TOLERANCE));
            for (weights, point) in checks.iter().zip(&points[vertices..]) {
                let Some(exact) = value(point) else {
                    return false;
                };
                let mut interpolated = 0.0;
                for (w, v) in weights.iter().zip(&corner) {
                    interpolated = w.mul_add(*v, interpolated);
                }
                if (exact - interpolated).abs() > tolerance {
                    return false;
                }
            }
        }
    }
    true
}

/// The child beam of transmitted mode `index` with the fitted `apex`.
fn child_beam(
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
    points: &[PointExpansion],
    (g, index): (usize, usize),
    apex: DVec3,
) -> Option<Beam> {
    let original = kit.retrace.stone.original_facet(beam.facet);
    let is_ext = mode_child(points.first()?, g, index)?.state.is_ext;
    let word = (original << 8) | (TRANSMIT_CODE << 1) | u32::from(is_ext);
    let prefix = beam.prefix.push(word)?;
    let mut child = Beam {
        facet: beam.facet,
        polygon: beam.polygon,
        apex,
        image_to_plane: beam.image_to_plane,
        inside: true,
        sig: branch_step(beam.sig, original, EventKind::Transmit, is_ext),
        prefix,
        bounces: beam.bounces + 1,
        spectrum: beam.spectrum,
        state: SampleStates::default(),
    };
    // The child's states are the retraced ones, so an inherited vertex equals the same image point
    // retraced later bit for bit (§7.3). The entry segment is outside the stone, so it adds no
    // depth; `add_depth` only creates the zero-filled depth vector the interior segments fill.
    let states = beam
        .state
        .points
        .iter()
        .zip(points)
        .map(|(state, point)| {
            let mut next = retrace_state(state.image, &child, kit)?;
            let segment = point.groups.get(g)?.depths;
            kit.add_depth(&mut next, &segment, (state.k, state.is_ext), beam.spectrum)
                .then_some(next)
        })
        .collect::<Option<Vec<_>>>()?;
    child.state = SampleStates {
        points: states,
        vertices: beam.state.vertices,
    };
    Some(child)
}

/// What one beam turned into.
enum Verdict {
    /// The child beams (one per transmitted mode) and the spectral-mean intensity of the
    /// outcomes `expand` dropped below `MIN_PROBABILITY`.
    Accept {
        /// The child beams.
        children: Vec<Beam>,
        /// The largest dropped intensity over the sample points.
        dropped: f64,
    },
    /// The beam must be split; the reason is its drop reason at the depth cap.
    Split(DropReason),
}

/// The child beam of one mode, or the reason it needs a split.
fn mode_beam(
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
    image: &ConvexPolygon,
    points: &[PointExpansion],
    mode: (usize, usize),
) -> Result<Beam, DropReason> {
    let (apex, error) = mode_fit(points, mode.0, mode.1).ok_or(DropReason::DepthCap)?;
    if !error.is_finite() || error > kit.settings.dir_tolerance_rad {
        return Err(DropReason::DepthCap);
    }
    if !interpolates(beam, image, points, mode.1, kit.settings.weight_tolerance) {
        return Err(DropReason::Interpolation);
    }
    child_beam(beam, kit, points, mode, apex).ok_or(DropReason::DepthCap)
}

/// The largest per-point spectral-mean intensity `expand` dropped below `MIN_PROBABILITY`.
fn dropped_intensity(points: &[PointExpansion]) -> f64 {
    points
        .iter()
        .map(|p| {
            let sum: f32 = p.groups.iter().map(|e| e.dropped).sum();
            f64::from(sum) / count_f64(p.groups.len().max(1))
        })
        .fold(0.0, f64::max)
}

/// The largest transmitted Stokes intensity over points, groups and nodes.
fn max_weight(beam: &Beam, points: &[PointExpansion]) -> f64 {
    let nodes = beam.spectrum.len();
    let mut best = 0.0_f64;
    for point in points {
        for (g, expansion) in point.groups.iter().enumerate() {
            for child in expansion
                .children
                .iter()
                .filter(|c| c.kind == EventKind::Transmit)
            {
                for s in child.state.stokes.iter().take(group_nodes(nodes, g)) {
                    best = best.max(f64::from(s.i));
                }
            }
        }
    }
    best
}

/// Judges `beam`: the child beams of its transmitted modes, or a split.
fn assess(
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
    image: &ConvexPolygon,
    points: &[PointExpansion],
) -> Verdict {
    let g = geometry_group(beam);
    let Some(reference) = points.first().and_then(|p| p.groups.get(g)) else {
        return Verdict::Split(DropReason::DepthCap);
    };
    if points.len() != beam.state.points.len() || !same_child_sets(points, reference) {
        return Verdict::Split(DropReason::DepthCap);
    }
    let mut children = Vec::new();
    for (index, child) in reference.children.iter().enumerate() {
        if child.kind != EventKind::Transmit {
            continue;
        }
        match mode_beam(beam, kit, image, points, (g, index)) {
            Ok(mode) => children.push(mode),
            Err(reason) => return Verdict::Split(reason),
        }
    }
    Verdict::Accept {
        children,
        dropped: dropped_intensity(points),
    }
}

/// Drops the entry children whose refracted wave normal lies within the optic-axis guard, as
/// [`DropReason::OpticAxis`] with energy `image_area × peak weight` (§10.2, §10.3).
///
/// The interaction guard tests only a beam's incoming `k`, so a child next to an optic axis would
/// be caught at its first internal hit only. The sample states' `k` are tested here instead,
/// before the child is counted or traced. Isotropic stones have no axis and keep every child.
fn drop_axis_children(
    children: Vec<Beam>,
    kit: &InteractKit<'_, '_>,
    image_area: f64,
    stats: &mut BeamStats,
) -> Vec<Beam> {
    let material = kit.retrace.material;
    let n_lambda = kit.retrace.n_lambda;
    let (guarded, kept): (Vec<Beam>, Vec<Beam>) = children
        .into_iter()
        .partition(|child| beam_near_axis(child, material, n_lambda));
    for child in &guarded {
        stats.record_drop(DropReason::OpticAxis, image_area * peak_weight(child));
    }
    kept
}

/// Refracts `beam` into `out`, bisecting it while the fit or a check fails.
///
/// Stops at once when the kit is cancelled, and drops the piece as `DepthCap` once the tile's
/// attempted work reaches the kit's allowance.
fn refract_into(
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
    heroes: &KitHeroes<'_, '_>,
    depth: u32,
    stats: &mut BeamStats,
    out: &mut Vec<Beam>,
) {
    // A cancelled render never uses a partial tree: stop at once.
    if kit.cancelled() {
        return;
    }
    let Some(image) = image_polygon(beam) else {
        stats.record_drop(DropReason::Sliver, 0.0);
        return;
    };
    stats.record_attempt();
    if stats.allowance_spent_at(kit.allowance) {
        // The tile's allowance is spent: the piece goes to the residual like a depth-cap drop.
        stats.record_drop(DropReason::DepthCap, image.area() * peak_weight(beam));
        return;
    }
    let points = expand_points(beam, kit, heroes);
    let verdict = points
        .as_deref()
        .map_or(Verdict::Split(DropReason::DepthCap), |p| {
            assess(beam, kit, &image, p)
        });
    match verdict {
        Verdict::Accept { children, dropped } => {
            let children = drop_axis_children(children, kit, image.area(), stats);
            let count = u64::try_from(children.len()).unwrap_or(u64::MAX);
            stats.entry_pieces += count;
            if is_anisotropic(kit.retrace.material) {
                // The o and e (or resolved A and B) children, each with its own fitted apex
                // (§10.1, §10.3).
                stats.mode_children += count;
            }
            if dropped > 0.0 {
                stats.record_drop(DropReason::MinProbability, image.area() * dropped);
            }
            out.extend(children);
        }
        Verdict::Split(_) if beam_near_axis(beam, kit.retrace.material, kit.retrace.n_lambda) => {
            // Not subdivided further near an optic axis: the residual covers it (§10.2, §10.3).
            let weight = points.as_deref().map_or(0.0, |p| max_weight(beam, p));
            stats.record_drop(DropReason::OpticAxis, image.area() * weight);
        }
        Verdict::Split(reason) => {
            let halves = (depth < kit.settings.max_split_depth && image.area() > SPLIT_FLOOR_PX2)
                .then(|| {
                    split_beam(
                        beam,
                        kit.retrace.stone,
                        kit.retrace.camera,
                        kit.retrace.size,
                    )
                })
                .flatten();
            if let Some((first, second)) = halves {
                stats.apex_splits += 1;
                refract_into(&first, kit, heroes, depth + 1, stats, out);
                refract_into(&second, kit, heroes, depth + 1, stats, out);
            } else {
                let weight = points.as_deref().map_or(0.0, |p| max_weight(beam, p));
                stats.record_drop(reason, image.area() * weight);
            }
        }
    }
}

/// Refracts an outside primary beam into the stone (beam guide §8.3).
///
/// `beam` lies on a front facet (`inside == false`, no interaction yet). Per transmitted mode the
/// result holds child beams on the same facet and polygon, inside, with the fitted apex, the same
/// `image_to_plane`, `sig = branch_step(.., Transmit, is_ext)`, the prefix plus the step word
/// `(facet << 8) | (3 << 1) | is_ext`, `bounces + 1`, and the exact sample states of the
/// expansion (`prev_plane` is the expansion's plane of incidence). Beams are bisected while the
/// direction error exceeds `τ_dir`, while the sample points disagree on the child set (or a child
/// diverged), or while the weight interpolation check fails; past `max_split_depth` the piece is
/// dropped (`DepthCap`, or `Interpolation`) with energy `image area × max weight`. Outcomes below
/// `MIN_PROBABILITY` are recorded as `MinProbability` drops of the pieces that are kept. The
/// reflection at the first surface is `leaf::reflection_leaf`'s. Counts `entry_pieces` (child
/// beams) and `apex_splits`; recording the new beams in `stats.beams` is the tree driver's job.
///
/// A child whose refracted wave normal lies within the optic-axis guard of a birefringent stone
/// (uniaxial or biaxial axes) is not returned: it is a [`DropReason::OpticAxis`] drop, which the
/// residual covers (`drop_axis_children`).
///
/// Children come in canonical order: ordinary mode first, then the extraordinary one, each in the
/// order of the bisection. A beam that is not an outside primary one gives nothing.
#[must_use]
pub fn refract_entry(beam: &Beam, kit: &InteractKit<'_, '_>, stats: &mut BeamStats) -> Vec<Beam> {
    if beam.inside || beam.bounces != 0 {
        return Vec::new();
    }
    let heroes = kit.group_heroes(beam.spectrum);
    let mut out = Vec::new();
    refract_into(beam, kit, &heroes, 0, stats, &mut out);
    out.sort_by_key(|b| b.state.points.first().is_some_and(|s| s.is_ext));
    out
}

/// The direction error (radians) of the apex fit of `beam`'s first transmitted mode, without any
/// subdivision: `max_p angle(ray_p, p − A')` over all sample points (guide §8.3).
///
/// A diagnostic for the tests and for the astigmatic-constant measurement. `None` for a beam that
/// is not an outside primary one, has no transmitted child, or cannot be fitted.
#[must_use]
pub fn fit_error(beam: &Beam, kit: &InteractKit<'_, '_>) -> Option<f64> {
    if beam.inside || beam.bounces != 0 {
        return None;
    }
    let heroes = kit.group_heroes(beam.spectrum);
    let points = expand_points(beam, kit, &heroes)?;
    let g = geometry_group(beam);
    let index = points
        .first()?
        .groups
        .get(g)?
        .children
        .iter()
        .position(|c| c.kind == EventKind::Transmit)?;
    mode_fit(&points, g, index).map(|(_, error)| error)
}

#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::{
        optics::materials::GemMaterial,
        renderer::beam::{
            BeamSettings,
            beam::{OPTIC_AXIS_GUARD_RAD, SpectralInterval, SpectralNode},
            birefringence::{biaxial_optic_axes, is_biaxial_crystal},
            leaf::ReflectionKit,
            primary::{TileRect, primary_beams},
            retrace::RetraceKit,
            stone::BeamStone,
            tests_support::{BeamRig, MAX_BOUNCES, glass, sapphire},
        },
    };

    /// The side of the test frame in pixels.
    const FRAME: u32 = 16;

    /// The spectral node count of the tests.
    const N_LAMBDA: u32 = 48;

    /// The area handed to the guard (the energy scale only).
    const TEST_AREA: f64 = 0.25;

    /// Runs `f` with the interaction kit of a cube rig of `material` and its primary beams.
    fn with_kit<R>(
        material: GemMaterial,
        f: impl FnOnce(&InteractKit<'_, '_>, Vec<Beam>) -> R,
    ) -> R {
        with_cancel_kit(material, None, f)
    }

    /// [`with_kit`] with the kit polling `cancel`, if given.
    fn with_cancel_kit<R>(
        material: GemMaterial,
        cancel: Option<&AtomicBool>,
        f: impl FnOnce(&InteractKit<'_, '_>, Vec<Beam>) -> R,
    ) -> R {
        let rig = BeamRig::cube(material);
        let stone = BeamStone::new(&rig.planes, false).expect("a closed stone");
        let settings = BeamSettings::default();
        let scene = rig.scene(FRAME, MAX_BOUNCES);
        let reflection = ReflectionKit::new(
            &rig.material,
            &rig.planes,
            &rig.camera,
            (FRAME, FRAME),
            &scene.environment,
            N_LAMBDA,
        );
        let retrace = RetraceKit {
            camera: &rig.camera,
            size: (FRAME, FRAME),
            stone: &stone,
            material: &rig.material,
            n_lambda: N_LAMBDA,
        };
        let tile = TileRect {
            x0: 0,
            y0: 0,
            x1: FRAME,
            y1: FRAME,
        };
        let mut kit = reflection.interact_kit(retrace, &settings, tile, MAX_BOUNCES);
        kit.cancel = cancel;
        let primaries = primary_beams(
            &stone,
            &rig.camera,
            (FRAME, FRAME),
            tile,
            SpectralInterval::all(N_LAMBDA),
        );
        f(&kit, primaries)
    }

    /// `axis` tilted by `angle` radians towards an arbitrary perpendicular.
    fn tilted(axis: Vec3, angle: f64) -> Vec3 {
        let perp = axis.any_orthonormal_vector().as_dvec3();
        let k = axis.as_dvec3() * angle.cos() + perp * angle.sin();
        k.normalize().as_vec3()
    }

    /// `child` with the wave normal `k` at every sample point.
    fn with_wave_normal(child: &Beam, k: Vec3) -> Beam {
        let mut out = child.clone();
        for state in &mut out.state.points {
            state.k = k;
        }
        out
    }

    /// The optic axes of `material` at the centre wavelength of `beam`'s interval.
    fn axes(material: &GemMaterial, beam: &Beam) -> Vec<Vec3> {
        if is_biaxial_crystal(material) {
            let lambda = SpectralNode(beam.spectrum.centre()).centre_nm(N_LAMBDA);
            let indicatrix = material.biaxial_indicatrix(lambda).expect("biaxial");
            biaxial_optic_axes(&indicatrix).expect("two axes").to_vec()
        } else {
            vec![material.c_axis]
        }
    }

    /// An entry child next to an optic axis is dropped as `OpticAxis`, one outside the guard kept.
    fn check_guard(material: GemMaterial) {
        let reference = material.clone();
        with_kit(material, |kit, primaries| {
            let mut stats = BeamStats::default();
            let child = primaries
                .iter()
                .find_map(|primary| refract_entry(primary, kit, &mut stats).into_iter().next())
                .expect("an entry child");
            let n_lambda = kit.retrace.n_lambda;
            for axis in axes(&reference, &child) {
                let near = with_wave_normal(&child, tilted(axis, 0.5 * OPTIC_AXIS_GUARD_RAD));
                let far = with_wave_normal(&child, tilted(axis, 4.0 * OPTIC_AXIS_GUARD_RAD));
                assert!(beam_near_axis(&near, &reference, n_lambda));
                assert!(!beam_near_axis(&far, &reference, n_lambda));
                let mut guard_stats = BeamStats::default();
                let kept =
                    drop_axis_children(vec![near, far.clone()], kit, TEST_AREA, &mut guard_stats);
                assert_eq!(kept.len(), 1);
                assert_eq!(kept[0].state.points[0].k, far.state.points[0].k);
                let slot = DropReason::OpticAxis.slot();
                assert_eq!(guard_stats.drops[slot], 1);
                assert!(guard_stats.dropped_energy[slot] > 0.0);
            }
        });
    }

    /// A cancelled kit refracts nothing and records nothing.
    #[test]
    fn a_cancelled_kit_stops_the_entry_recursion_at_once() {
        let cancel = AtomicBool::new(true);
        with_cancel_kit(glass(), Some(&cancel), |kit, primaries| {
            assert!(kit.cancelled());
            let mut stats = BeamStats::default();
            for primary in &primaries {
                assert_eq!(refract_entry(primary, kit, &mut stats).len(), 0);
            }
            assert_eq!(stats, BeamStats::default());
        });
    }

    /// A tile whose allowance is spent hands the entry piece to the residual as a depth-cap drop
    /// and refracts nothing.
    #[test]
    fn a_spent_allowance_drops_the_entry_piece_as_depth_cap() {
        with_kit(glass(), |kit, primaries| {
            let mut stats = BeamStats {
                attempted_pieces: crate::renderer::beam::stats::TILE_ALLOWANCE,
                ..BeamStats::default()
            };
            let slot = DropReason::DepthCap.slot();
            for primary in &primaries {
                assert_eq!(refract_entry(primary, kit, &mut stats).len(), 0);
            }
            assert_eq!(stats.drops[slot], primaries.len() as u64);
            assert!(stats.dropped_energy[slot] > 0.0);
            assert_eq!(stats.entry_pieces, 0);
        });
    }

    #[test]
    fn a_uniaxial_entry_child_within_the_guard_angle_is_dropped() {
        // The cube rig looks straight down +Y, which is also the default c axis: the centre sample
        // of the only primary beam is then inside the guard, `refract_entry` drops the whole beam
        // as `OpticAxis` and there is no entry child to test. The c axis is tilted 45 degrees so
        // the beam is refracted normally and the guard is exercised on its child.
        let mut tilted_axis = sapphire();
        tilted_axis.c_axis = Vec3::new(1.0, 1.0, 0.0).normalize();
        check_guard(tilted_axis);
    }

    #[test]
    fn a_biaxial_entry_child_within_the_guard_angle_of_either_axis_is_dropped() {
        check_guard(GemMaterial::by_name("Tanzanite").expect("Tanzanite resolves"));
    }

    #[test]
    fn an_isotropic_stone_loses_no_entry_child_to_the_guard() {
        with_kit(glass(), |kit, primaries| {
            let mut stats = BeamStats::default();
            let mut children = 0_usize;
            for primary in &primaries {
                let made = refract_entry(primary, kit, &mut stats);
                children += made.len();
                let mut guard_stats = BeamStats::default();
                let kept = drop_axis_children(made.clone(), kit, TEST_AREA, &mut guard_stats);
                assert_eq!(kept.len(), made.len());
                assert_eq!(guard_stats.drops[DropReason::OpticAxis.slot()], 0);
            }
            assert!(children > 0, "glass must refract");
            assert_eq!(stats.drops[DropReason::OpticAxis.slot()], 0);
        });
    }
}
