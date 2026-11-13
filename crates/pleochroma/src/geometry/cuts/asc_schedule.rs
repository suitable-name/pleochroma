//! Reconstructs facet planes directly from the real per-tier mast (depth) values
//! in a parsed `.asc` file's cutting instructions -- the accurate counterpart to
//! [`super::database_angles`]'s fabricated proportions.

use glam::{DVec3, Vec3};
use pleochroma_formats::asc::AscSchedule;

use super::{CutError, StandardGemCuts};
use crate::geometry::{
    brep::GemPolyhedron,
    plane::{GpuFacetPlane, tier_is_crown_side},
};

impl StandardGemCuts {
    /// Generates facet planes directly from a parsed `GemCAD` `.asc` file's cutting instructions.
    ///
    /// Uses [`pleochroma_formats::asc::parse_asc`]'s output and its real per-tier mast
    /// (depth) values as plane offsets, instead of the fabricated proportions
    /// [`Self::from_database_angles`] has to invent when only `angle_settings` (angle
    /// + index, no depth) is available.
    ///
    /// # Conventions
    ///
    /// - **Angle sign** decides crown vs. pavilion, via
    ///   [`tier_is_crown_side`](crate::geometry::plane::tier_is_crown_side):
    ///   negative is pavilion, positive is crown, and a zero angle is the table
    ///   unless it is a sign-negative zero (the culet). `parse_asc` already turned
    ///   the file's documented culet encoding (angle `0`, NEGATIVE distance) into
    ///   that sign-negative zero, so file order never matters.
    /// - **Magnitude**: `theta` uses `angle_deg.abs()` -- the sign has already been
    ///   consumed above to pick crown vs. pavilion, so re-applying it to `sin`/`cos`
    ///   would rotate the facet to the wrong azimuthal quadrant.
    /// - **Plane offset**: `d = -mast.abs()`. `GemPolyhedron::from_planes` requires
    ///   every `d < 0` (the origin must lie inside every half-space). After parsing,
    ///   a mast is only negative on a nonzero-angle tier the manual does not
    ///   describe (no real catalogue file has one; the parser warns), and the
    ///   magnitude is still the physical depth.
    /// - **Azimuth**: `phi = 2*pi*(index + gear_reference_angle)/gear_teeth_abs()`
    ///   via [`Self::index_to_azimuth`] -- see that function's doc for the
    ///   reference-angle convention (tooth-denominated and additive; it never flips
    ///   the handedness of index growth). A tier with no listed index (rare -- a bare
    ///   `angle mast` with only a name) still produces one plane, with normal
    ///   `(0, +-cos(theta), sin(theta))` (the reference angle is not applied to an
    ///   absent index). That normal points at +Z, i.e. azimuth `phi = 90` degrees,
    ///   not at index 0 (+X, `phi = 0`); for a table or culet (`theta = 0`) the
    ///   horizontal component vanishes, so the azimuth is immaterial there.
    /// - **`mirror` is not applied here.** `schedule.mirror` is a symmetry *descriptor*,
    ///   not a geometric transform: every `a` record already lists each index-wheel
    ///   position its facet occurs at, mirror image included (see the tier records in
    ///   `pleochroma_formats::asc`). Applying it here would double-reflect schedules
    ///   that already list both mirror images, and fabricate facets for designs that
    ///   do not. `pleochroma_cut_core::orbit` uses it only to predict orbit-mates for
    ///   editing, never to rewrite a tier's `indices`.
    #[must_use]
    pub fn from_asc_schedule(schedule: &AscSchedule) -> Vec<GpuFacetPlane> {
        let (planes, _) = Self::raw_asc_planes(schedule, schedule.gear_reference_angle);
        dedup_planes(planes)
    }

    /// For every authored tier/index instance, the plane retained by production dedup.
    /// Indices address [`Self::from_asc_schedule`]'s output. A tier without indices has
    /// one entry. Aliases refer to the first retained plane, including near duplicates.
    /// Consumers applying later transforms must transform that retained plane once,
    /// rather than transforming each alias independently.
    #[must_use]
    pub fn asc_plane_provenance(schedule: &AscSchedule) -> Vec<Vec<usize>> {
        let (planes, ends) = Self::raw_asc_planes(schedule, schedule.gear_reference_angle);
        let representatives = dedup_representatives(&planes);
        let mut output_indices = vec![0; planes.len()];
        let mut count = 0;
        for (index, &representative) in representatives.iter().enumerate() {
            if index == representative {
                output_indices[index] = count;
                count += 1;
            } else {
                output_indices[index] = output_indices[representative];
            }
        }
        let mut start = 0;
        ends.into_iter()
            .map(|end| {
                let tier = output_indices[start..end].to_vec();
                start = end;
                tier
            })
            .collect()
    }

    /// Cumulative facet-plane count of `schedule.tiers[0..=i]` for every `i`: entry `i` is
    /// exactly `from_asc_schedule` of the schedule cut down to its first `i + 1` tiers,
    /// length only, with `gear_reference_angle` standing in for the schedule's own.
    ///
    /// One pass instead of `n` prefix schedules. Equal by construction, not by
    /// measurement: each tier's raw planes depend on that tier alone, and
    /// [`dedup_planes`] keeps a plane or drops it from the planes kept before it
    /// alone, so the planes kept from the first `i + 1` tiers are the same set whether
    /// or not later tiers follow. The count is how many survive among the raw planes of
    /// those tiers.
    #[must_use]
    pub fn asc_tier_plane_boundaries(
        schedule: &AscSchedule,
        gear_reference_angle: f64,
    ) -> Vec<usize> {
        let (planes, raw_ends) = Self::raw_asc_planes(schedule, gear_reference_angle);
        let keep = dedup_keep_mask(&planes);
        let mut kept_before = Vec::with_capacity(keep.len() + 1);
        kept_before.push(0usize);
        for &kept in &keep {
            kept_before.push(kept_before[kept_before.len() - 1] + usize::from(kept));
        }
        raw_ends.into_iter().map(|end| kept_before[end]).collect()
    }

    /// Every tier's planes in tier order, before deduplication, and the end position in
    /// that list of each tier's run. The azimuth uses `gear_reference_angle`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the schedule's f64 angles, masts and indices become the f32 GPU plane data; all are small normal-range values"
    )]
    fn raw_asc_planes(
        schedule: &AscSchedule,
        gear_reference_angle: f64,
    ) -> (Vec<GpuFacetPlane>, Vec<usize>) {
        let gear_teeth = (schedule.gear_teeth_abs().max(1)) as f32;
        let gear_reference_angle = gear_reference_angle as f32;
        let mut planes = Vec::with_capacity(schedule.facet_plane_count());
        let mut ends = Vec::with_capacity(schedule.tiers.len());

        for tier in &schedule.tiers {
            let is_crown = tier_is_crown_side(tier.angle_deg);

            let theta = (tier.angle_deg.abs() as f32).to_radians();
            let sin_theta = theta.sin();
            let cos_theta = theta.cos();
            let d = -(tier.mast.abs() as f32);

            if tier.indices.is_empty() {
                let n = if is_crown {
                    Vec3::new(0.0, cos_theta, sin_theta)
                } else {
                    Vec3::new(0.0, -cos_theta, sin_theta)
                };
                planes.push(GpuFacetPlane::new(n, d));
                ends.push(planes.len());
                continue;
            }

            for &idx in &tier.indices {
                let phi = Self::index_to_azimuth(idx as f32, gear_teeth, gear_reference_angle);
                let (sin_phi, cos_phi) = (phi.sin(), phi.cos());
                let n = if is_crown {
                    Vec3::new(sin_theta * cos_phi, cos_theta, sin_theta * sin_phi)
                } else {
                    Vec3::new(sin_theta * cos_phi, -cos_theta, sin_theta * sin_phi)
                };
                planes.push(GpuFacetPlane::new(n, d));
            }
            ends.push(planes.len());
        }

        (planes, ends)
    }

    /// Builds a validated B-Rep solid directly from a parsed `.asc` schedule.
    ///
    /// Uses [`Self::from_asc_schedule`] plus [`GemPolyhedron::from_planes`] and a
    /// check of [`GemPolyhedron::untouched_planes`] as the correctness oracle --
    /// exactly the same two-part gate [`Self::reconstruct_validated_brep`] uses for
    /// `angle_settings`-derived reconstructions.
    ///
    /// Unlike that path, there's no good "fabricated shape" fallback available here:
    /// the whole point of this function is that its offsets are the file's own real
    /// mast values, not invented proportions, so a validation failure means either the
    /// parse, the source file itself (e.g. a near-duplicate tier left in from a design
    /// revision), or the crown/pavilion sign convention above is off *for this
    /// particular file* -- silently substituting `standard_round_brilliant()` would
    /// hide that. Callers that need a guaranteed-renderable result on failure should
    /// fall back to [`Self::reconstruct_validated_brep`] (via `angle_settings`)
    /// themselves.
    ///
    /// # Errors
    ///
    /// Returns [`GemPolyhedron::from_planes`]'s error verbatim if the planes don't
    /// reconstruct into a valid, closed, finite solid at all, or a descriptive error
    /// if they do but leave one or more planes untouched (over-constrained --
    /// most often a near-duplicate tier revision in the source file, occasionally a
    /// zero-angle tier that resolved to the wrong side).
    pub fn reconstruct_validated_brep_from_asc(
        schedule: &AscSchedule,
    ) -> Result<GemPolyhedron, CutError> {
        let planes = Self::from_asc_schedule(schedule);
        let plane_count = planes.len();
        let hull = GemPolyhedron::from_planes(planes)?;
        let untouched = hull.untouched_planes();
        if untouched.is_empty() {
            Ok(hull)
        } else {
            Err(CutError::OverConstrained {
                untouched_count: untouched.len(),
                plane_count,
                untouched,
            })
        }
    }
}

/// Drops planes that are near-duplicates of an earlier one in the list (same normal
/// and offset within a coarse tolerance), keeping the first occurrence.
///
/// Real `.asc` files can list the same index twice in separate tier rows with the same angle
/// and mast. Such planes are one half-space, and `GemPolyhedron::from_planes` rejects
/// coincident planes, so the redundant copy is dropped here without changing the solid.
///
/// The test is an `O(P^2)` tolerance compare against every kept plane, not a quantized-bin
/// lookup: two planes whose values straddle a bin edge would land in different bins and both
/// be kept, which `from_planes` then rejects as coincident.
///
/// The **first** occurrence of a duplicate survives; a later one is dropped in place, never
/// substituted in. Callers attribute planes to tiers by position, and substituting a later
/// tier's plane into an earlier slot would silently reattribute it. The result can depend on
/// the order of a chain of mutually close planes, but real schedules list a duplicate row
/// next to the row it duplicates.
pub(super) fn dedup_planes(planes: Vec<GpuFacetPlane>) -> Vec<GpuFacetPlane> {
    let keep = dedup_keep_mask(&planes);
    planes
        .into_iter()
        .zip(keep)
        .filter_map(|(plane, kept)| kept.then_some(plane))
        .collect()
}

/// Which of `planes` [`dedup_planes`] keeps, position for position: a plane is kept
/// exactly when no earlier KEPT plane coincides with it, so the answer for a prefix of
/// `planes` is the prefix of the answer for all of them.
fn dedup_keep_mask(planes: &[GpuFacetPlane]) -> Vec<bool> {
    dedup_representatives(planes)
        .into_iter()
        .enumerate()
        .map(|(index, representative)| index == representative)
        .collect()
}

/// Raw index of the first retained plane representing each raw input plane.
fn dedup_representatives(planes: &[GpuFacetPlane]) -> Vec<usize> {
    // Relative to the larger of the two offsets, floored at `1.0`: a schedule at a larger scale
    // has proportionally larger masts, so an absolute quantum would either miss genuine
    // duplicates or merge distinct facets. Below the floor this is an absolute quantum of
    // about 5e-4.
    const OFFSET_REL_EPSILON: f32 = 1.0 / 2048.0;

    let mut kept: Vec<usize> = Vec::with_capacity(planes.len());
    let mut representatives = Vec::with_capacity(planes.len());
    for (index, plane) in planes.iter().enumerate() {
        let pn = plane_normal_f64(plane);
        let representative = kept.iter().copied().find(|&i| {
            let k = &planes[i];
            let offset_scale = k.d.abs().max(plane.d.abs()).max(1.0);
            normals_coincide(plane_normal_f64(k), pn)
                && (k.d - plane.d).abs() <= OFFSET_REL_EPSILON * offset_scale
        });
        let representative = representative.unwrap_or_else(|| {
            kept.push(index);
            index
        });
        representatives.push(representative);
    }
    representatives
}

/// The plane's stored `f32` normal widened to `f64`.
fn plane_normal_f64(plane: &GpuFacetPlane) -> DVec3 {
    DVec3::new(
        f64::from(plane.normal[0]),
        f64::from(plane.normal[1]),
        f64::from(plane.normal[2]),
    )
}

/// Whether two facet normals point the same way, to within `1 - cos(angle) <= 2e-7`
/// (about 0.036 degrees).
///
/// The one normal-equality rule shared by every plane dedup that has to agree with
/// the `.asc` plane builder: this module's `dedup_planes`, the solid measurer's
/// plane filter and the facet-provenance map. Both inputs are normalised in `f64`
/// first, because a normal that was unit-normalised in `f32` has a squared length
/// of `1 +- ~1.5e-7`, so the raw dot product of two bit-identical copies can fall
/// short of `1` by more than any bound tight enough to tell real neighbours apart.
/// After the `f64` normalisation, rounding noise from independent `sin`/`cos`/
/// `normalize` calls in `f32` (well under `1e-12` in `1 - cos`) is far below the
/// bound, while the closest facets real schedules keep distinct, one degree apart
/// (`1 - cos` of about `1.5e-4`), are far above it.
///
/// A zero or non-finite normal never coincides with anything, itself included.
#[must_use]
pub fn normals_coincide(a: DVec3, b: DVec3) -> bool {
    const NORMAL_COINCIDE_EPSILON: f64 = 2e-7;
    let (a_hat, b_hat) = (a.normalize_or_zero(), b.normalize_or_zero());
    1.0 - a_hat.dot(b_hat) <= NORMAL_COINCIDE_EPSILON
}
