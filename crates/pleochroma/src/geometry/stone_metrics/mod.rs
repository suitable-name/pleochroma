//! Deterministic physical measurement of a faceted stone from its plane
//! arrangement.
//!
//! Wrong mast configurations are *self-consistent*: every tier of a wrong solve is
//! still vertex-incident, so nothing inside the arrangement separates right from
//! wrong. The proportions printed on a real diagram (`Vol/W^3`, `L/W`, `C/W`,
//! `P/W`, `H/W`) are **external** constraints: a candidate configuration either
//! reproduces them or it does not. [`measure_solid`] computes those same figures
//! from a plane arrangement -- deterministically, in `f64`, with no convex-hull
//! library. [`super::brep`] uses this module's [`vertices`] walk for the same
//! reason; the meet solver must not depend on `brep`.
//!
//! The mechanism: enumerate the solid's vertices as feasible triple
//! intersections of the planes (the same primitive `meet_solver` uses), then
//! reconstruct each facet's polygon by collecting the vertices on its plane and
//! ordering them by angle. Volume comes from the divergence theorem
//! (`V = (1/3) * sum over faces of offset * area`, exact for outward-oriented
//! planes), heights from vertex `y` extents against the girdle band, and
//! width/length from the girdle outline's `x`/`z` extents (axis-aligned, and
//! rotating-caliper as a cross-check; the printed figures use the axis convention,
//! see [`ExternalProportions`]).
//!
//! Entry points: [`measure_solid`] / [`measure_solid_with_vertices`] for the figures,
//! [`build_solid_mesh`] for the mesh, and [`build_solid_mesh_and_measure`] /
//! [`build_solid_mesh_and_measure_with_vertices`] for both from one arrangement
//! (one vertex enumeration, one face ring per plane; results bit-identical to the
//! separate calls). Optimizer candidate evaluation, result scoring and Retarget
//! analysis want both and should use the combined pair.
//!
//! Split into: [`types`] (the reported data shapes), [`vertices`] (plane-
//! arrangement vertex enumeration and dedup, shared by both entry points),
//! `arrangement` (the shared dedup + normalise + enumerate step and the per-plane
//! face rings), [`measure`] ([`measure_solid`] itself), [`mesh`] ([`build_solid_mesh`] and
//! face reconstruction), [`concave`] (the tool-carved mesh behind
//! [`build_solid_mesh_geom`]), [`caliper`] (the rotating-caliper cross-check), and
//! [`proportions`] ([`StoneProportions`], the cutter-facing readout).

mod arrangement;
mod caliper;
mod concave;
mod measure;
mod mesh;
mod proportions;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_arrangement;
mod types;
mod vertices;

pub use caliper::{CaliperFrame, caliper_frame};
pub use concave::ToolBounds;
pub use measure::{measure_solid, measure_solid_with_vertices};
pub use mesh::{
    TOOL_ICOSPHERE_LEVEL, TOOL_SEGMENTS, build_solid_mesh, build_solid_mesh_geom, mesh_volume,
    tessellate_tool, tool_bounds,
};
pub use proportions::StoneProportions;
pub use types::{ExternalProportions, SolidMesh, SolidMetrics, SolidStatus};

// Shared with `geometry::brep`, which reconstructs its B-rep from the same
// deterministic arrangement walk (see that module's docs).
pub(super) use measure::max_abs_offset;
pub(super) use vertices::{escaping_plane_indices, for_each_feasible_triple};

use super::girdle::GIRDLE_NORMAL_Y_EPSILON;

/// The mesh and the measures of one plane arrangement, from ONE arrangement and ONE
/// set of face rings.
///
/// Bit for bit equal to `(build_solid_mesh(planes), measure_solid(planes))`, but the
/// plane dedup, the vertex enumeration (the expensive step) and each plane's face ring
/// run once. The second element is `None` exactly
/// when [`measure_solid`] would return `None`.
///
/// Use this wherever both are wanted for the same planes: optimizer candidate
/// evaluation, result scoring, Retarget analysis. A caller that needs only one of the
/// two should keep calling [`build_solid_mesh`] or [`measure_solid`].
#[must_use]
pub fn build_solid_mesh_and_measure(
    planes: &[(glam::DVec3, f64)],
) -> (SolidStatus, Option<SolidMetrics>) {
    let (status, measured) = build_solid_mesh_and_measure_with_vertices(planes);
    (status, measured.map(|(metrics, _)| metrics))
}

/// [`build_solid_mesh_and_measure`] plus the solid's distinct vertices.
///
/// For callers of [`measure_solid_with_vertices`]: bit for bit equal to
/// `(build_solid_mesh(planes), measure_solid_with_vertices(planes))` from one
/// arrangement.
#[must_use]
pub fn build_solid_mesh_and_measure_with_vertices(
    planes: &[(glam::DVec3, f64)],
) -> (SolidStatus, Option<(SolidMetrics, Vec<glam::DVec3>)>) {
    let arr = match arrangement::arrange(planes) {
        Ok(arr) => arr,
        Err(status) => return (*status, None),
    };
    let rings = arrangement::face_rings(&arr);
    let measured = measure::measure_from_arrangement(&arr, Some(&rings));
    let status = mesh::mesh_from_arrangement(&arr, &rings);
    (status, measured)
}

/// Every feasible plane triple of the arrangement `planes` bounds, with the point the
/// three planes meet at, in `planes`' own units and in the walk's lexicographic order.
///
/// This is the public face of the crate-private `for_each_feasible_triple` walk (whose
/// callers hand it already-normalised planes): it applies the same power-of-two offset
/// normalisation as [`build_solid_mesh`], then scales every point back, so a point is
/// bit-for-bit one the mesh derives its vertices from. Unlike the mesh it neither
/// welds nearby points nor snaps them to a face, so a caller can cluster the points at
/// an accuracy of its own choosing.
///
/// `scale_floor` is a length (the stone's extent) the normalisation scale never falls
/// below. The walk accepts a point that violates a plane by up to `EPS_FEAS` (`1e-5`)
/// in normalised units, so a scale far below the stone's extent would make that slack
/// smaller than the caller's own incidence tolerance. Pass `0.0` to reproduce the
/// mesh's scale exactly.
///
/// Returns `None` when a well-conditioned feasible vertex reaches the bounding blank
/// (the planes enclose no finite stone). Triples that involve a blank-box plane are
/// never reported.
#[must_use]
pub fn feasible_plane_triples(
    planes: &[(glam::DVec3, f64)],
    scale_floor: f64,
) -> Option<Vec<([usize; 3], glam::DVec3)>> {
    let floor = if scale_floor.is_finite() {
        scale_floor
    } else {
        0.0
    };
    let scale = pow2_scale_norm(max_abs_offset(planes).max(floor));
    let normalised: Vec<(glam::DVec3, f64)> = planes.iter().map(|&(n, m)| (n, m / scale)).collect();
    let mut triples = Vec::new();
    for_each_feasible_triple(&normalised, MIN_TRIPLE_DET, true, |triple, _, vertex| {
        if triple.iter().all(|&index| index < planes.len()) {
            triples.push((triple, vertex * scale));
        }
    })?;
    Some(triples)
}

// One definition of the blank box half-extent (a solid that reaches it is
// unbounded -- a schedule missing its closing planes -- which [`measure_solid`]
// reports as `None`), the feasibility slack (a vertex may poke this far beyond a
// plane and still count as part of the solid) and the minimum triple
// determinant, shared with the meet solver's own candidate enumeration.
pub(super) use super::meet_solver::{BLANK_HALF_EXTENT, EPS_FEAS, MIN_TRIPLE_DET};

/// A vertex within this absolute distance of a plane counts as lying on its
/// face. Sized to cover [`EPS_FEAS`] feasibility drift and `VERTEX_DEDUP`
/// merging; the resulting polygon-corner error is quadratically small in it.
const EPS_FACE: f64 = 2e-5;

/// Two candidate vertices within this distance (per axis) are one vertex.
const VERTEX_DEDUP: f64 = 1e-6;

/// `|normal.y|` at or below this means a vertical (girdle) plane.
///
/// Classifies `f32`-derived normals widened to `f64`, hence the `1e-3` noise
/// floor. The meet solver classifies exact `f64` schedule angles with `1e-6`.
///
/// Shares [`super::girdle::GIRDLE_NORMAL_Y_EPSILON`]: this module's planes carry
/// the same `f32` normalisation noise, so both must classify a facet alike.
const GIRDLE_NY: f64 = GIRDLE_NORMAL_Y_EPSILON as f64;

/// `normal.y` at or above this counts as a flat table facet: an outward
/// normal within this of straight up (`+Y`) -- the horizontal-plane
/// counterpart to [`GIRDLE_NY`]'s vertical-plane threshold.
const TABLE_NY: f64 = 1.0 - 1e-6;

/// Rounds a design's representative absolute scale (the largest `|m|`
/// among its planes) to the nearest power of two. [`measure_solid`] and
/// [`build_solid_mesh`] divide every plane offset by this before running any
/// vertex-arrangement geometry, and multiply their length/volume results back
/// by it afterward -- [`EPS_FEAS`], [`EPS_FACE`], [`VERTEX_DEDUP`] and
/// `BLANK_HALF_EXTENT` are all absolute constants tuned for masts of order 1,
/// so a design at a very different absolute scale would otherwise make them too
/// tight or too
/// loose. Non-finite or non-positive input (an empty plane list, or every
/// offset exactly `0.0`) falls back to `1.0`, i.e. no normalisation.
/// `scale == 1.0` (or already an exact power of two) makes this `1.0` exactly,
/// so every division/multiplication by it is a bit-exact no-op.
///
/// Computed from the IEEE exponent and mantissa (`scale = m * 2^k`, `m` in
/// `[1, 2)`; the result is `2^k` when `m < sqrt(2)`, else `2^(k+1)`), with no
/// platform `log2`/`powi`. The exponent is clamped to the normal range.
pub(super) fn pow2_scale_norm(scale: f64) -> f64 {
    const MANTISSA: u64 = (1 << 52) - 1;
    /// 2^64, lifts a subnormal into the normal range.
    const SUBNORMAL_LIFT: f64 = 18_446_744_073_709_551_616.0;
    if !(scale.is_finite() && scale > 0.0) {
        return 1.0;
    }
    // `scale` is positive, so the sign bit is clear and `bits >> 52` is the biased exponent.
    let raw = scale.to_bits();
    let (bits, lift) = if raw >> 52 == 0 {
        ((scale * SUBNORMAL_LIFT).to_bits(), 64_i64)
    } else {
        (raw, 0_i64)
    };
    let biased = i64::from(u16::try_from(bits >> 52).unwrap_or(1023));
    let mantissa = f64::from_bits((bits & MANTISSA) | (1023_u64 << 52));
    let mut exponent = biased - 1023 - lift;
    if mantissa >= std::f64::consts::SQRT_2 {
        exponent += 1;
    }
    let biased_out = u64::try_from((exponent + 1023).clamp(1, 2046)).unwrap_or(1023);
    f64::from_bits(biased_out << 52)
}
