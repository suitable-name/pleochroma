//! Refitting the table and culet heights after the angles move.
//!
//! The table and the culet are flat: their angle never changes, but their HEIGHT decides how
//! big they are. A steeper crown pushes the crown's apex up, so a table left at its old
//! height gets larger; a shallower pavilion pulls the apex up too and can lift it past the
//! culet plane, which then no longer touches the stone. A cutter fixes both by re-polishing
//! the flat at the height that gives the same size again.
//!
//! [`refit_flats`] does that for every pinned flat tier that had its own facet: a bisection on
//! the tier's mast, over the candidate's planes with every other mast held, until the flat's
//! facet has the area it had in the original stone. It never re-solves the design inside the
//! loop (only the plane set is rebuilt), so the caller solves the refitted design once at the
//! end and judges that.
//!
//! The search bracket is cut to the stone's own scale (its largest mast), so a design authored
//! at another size -- a girdle mast of 4 rather than the usual 1 -- is refitted as well.
//!
//! # Measuring a probe
//!
//! A probe needs ONE facet's area. Building the whole solid for it (`build_solid_mesh`) costs
//! about 3 ms on a 170-plane design and a bisection makes 37 of them per flat, so by default only
//! the first probe (the low end of the bracket) builds the mesh and every other probe cuts the
//! flat's facet out of its own plane ([`AreaMethod::Clipped`], see the `clip` module). The first
//! mesh also shows that the stone closes. Wherever the clipped figure cannot be vouched for to
//! the mesh's own tolerance -- a sliver of a facet, a plane lying on another, a solid that does
//! not close -- the probe builds the mesh after all, so those answers are the mesh's own.
//! [`AreaMethod::FullMesh`] builds it at every probe, the way the refit always did.
//!
//! Rebuilding the plane list (`Design::planes_from_solved`: the schedule, every plane's trig and
//! the plane builder's quadratic duplicate scan) costs a good part of a millisecond itself, more
//! than a clipped probe. A flat's mast changes one plane's offset and nothing else, so after one
//! rebuild the list is patched instead: checked once against a second rebuild, bit for bit, and
//! rebuilt every time where it cannot be (another near-flat tier on the same side).

mod clip;

use self::clip::Reference;
use super::{
    anchors::scale_reference_mast,
    validity::{FlatRing, StoneAnalysis, polygon_area},
};
use glam::DVec3;
use pleochroma::geometry::{
    meet_solver::SolvedTier,
    plane::tier_is_crown_side,
    stone_metrics::{SolidStatus, build_solid_mesh},
};
use pleochroma_cut_core::{
    Design, design::hinge::tier_plane_ranges, optics_hints::is_horizontal_angle_deg,
};

/// A flat facet whose linear size is within this fraction of the original is left alone.
const REFIT_TOLERANCE: f64 = 0.02;

/// The lowest mast the bisection tries, as a fraction of the stone's scale: a flat just off
/// the girdle plane.
const BRACKET_LOW_FRACTION: f64 = 1e-4;

/// The highest mast the bisection tries, as a multiple of the stone's scale: far above any
/// crown apex or below any culet.
const BRACKET_HIGH_FRACTION: f64 = 3.0;

/// The bisection stops when its bracket is narrower than this fraction of the stone's scale.
const STOP_WIDTH_FRACTION: f64 = 1e-10;

/// Upper bound on bisection steps (the bracket halves every step).
const MAX_STEPS: usize = 60;

// How many probes `Probes::area` has run on this thread, how many of them the clipper answered and
// how many built the mesh: the test-only witnesses of what a stopped bisection still paid for
// and of how many whole meshes the clipped route avoids.
#[cfg(test)]
thread_local! {
    static AREA_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CLIPPED_ANSWERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static MESH_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How a probe measures the area of the flat's facet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaMethod {
    /// Cut the facet out of its own plane where that can be vouched for to the mesh's tolerance,
    /// and build the mesh where it cannot. The first probe always builds it.
    Clipped,
    /// Build the whole solid's mesh at every probe: what the refit did before the clipper.
    FullMesh,
}

/// The new mast of one flat tier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlatRefit {
    /// The tier's position in `design.tiers`.
    pub tier_index: usize,
    /// The mast that gives the flat facet its original size again.
    pub new_mast: f64,
}

/// The flat tier a bisection moves and the plane whose facet it measures.
#[derive(Debug, Clone, Copy)]
struct FlatPlane {
    /// The tier's position in `design.tiers`.
    tier_index: usize,
    /// The plane's position in `planes_from_solved`'s list.
    plane_index: usize,
}

/// The area of the facet on `planes[plane_index]`: the mesh's figure, or the clipper's where it
/// vouches for the same one. `0.0` when the solid is not closed.
///
/// `reference` stays `None` until `method` has built a closed mesh, and is set from the first
/// one; the clipper answers only once it is set.
fn area_of_planes(
    planes: &[(DVec3, f64)],
    plane_index: usize,
    method: AreaMethod,
    reference: &mut Option<Reference>,
) -> f64 {
    if method == AreaMethod::Clipped
        && let Some(reference) = reference.as_ref()
        && let Some(area) = clip::facet_area(planes, plane_index, reference)
    {
        #[cfg(test)]
        CLIPPED_ANSWERS.with(|answers| answers.set(answers.get() + 1));
        return area;
    }
    #[cfg(test)]
    MESH_BUILDS.with(|builds| builds.set(builds.get() + 1));
    match build_solid_mesh(planes) {
        SolidStatus::Closed(mesh) => {
            if method == AreaMethod::Clipped && reference.is_none() {
                *reference = Reference::from_mesh(&mesh);
            }
            mesh.rings
                .iter()
                .filter(|(plane, ring)| *plane == plane_index && ring.len() >= 3)
                .map(|(_, ring)| polygon_area(ring))
                .sum()
        }
        _ => 0.0,
    }
}

/// Facets of other tiers leaning less than this from the horizontal, on the flat's own side, can
/// share its normal: the plane builder's duplicate rule then relates them to the flat's plane.
const NEAR_FLAT_DEG: f64 = 0.5;

/// The offset the plane builder gives a tier of mast `mast`: its magnitude, through `f32`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the plane builder stores a facet plane's offset as f32; this is that rounding"
)]
fn flat_offset(mast: f64) -> f64 {
    f64::from(mast.abs() as f32)
}

/// Whether two plane lists are the same bit for bit.
fn same_planes(a: &[(DVec3, f64)], b: &[(DVec3, f64)]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(&(normal_a, offset_a), &(normal_b, offset_b))| {
                normal_a.to_array().map(f64::to_bits) == normal_b.to_array().map(f64::to_bits)
                    && offset_a.to_bits() == offset_b.to_bits()
            })
}

/// Where a bisection's plane list comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Patch {
    /// No probe has run.
    Unbuilt,
    /// The flat's plane is the only thing a mast changes (see [`Probes::can_patch`]); the next
    /// probe rebuilds the list once more to check the patched one against it.
    Unverified,
    /// A patched list equalled a rebuilt one bit for bit: later probes set the flat's offset
    /// instead of rebuilding.
    Verified,
    /// Every probe rebuilds the list from the design.
    Off,
}

/// What one bisection knows between its probes: the plane list, a point inside the solid, and how
/// each probe measures.
struct Probes<'a> {
    design: &'a Design,
    flat: FlatPlane,
    method: AreaMethod,
    /// A point inside a closed solid with these normals, from the first probe that built a
    /// closed mesh; the clipper answers only once it has one.
    reference: Option<Reference>,
    /// The planes of the last probe.
    planes: Vec<(DVec3, f64)>,
    patch: Patch,
}

impl<'a> Probes<'a> {
    const fn new(design: &'a Design, flat: FlatPlane, method: AreaMethod) -> Self {
        Self {
            design,
            flat,
            method,
            reference: None,
            planes: Vec::new(),
            patch: Patch::Unbuilt,
        }
    }

    /// `design`'s planes with tier `flat.tier_index` at mast `mast` and every other mast as it is
    /// in `solved`.
    fn rebuilt(&self, solved: &mut [SolvedTier], mast: f64) -> Vec<(DVec3, f64)> {
        let previous = solved[self.flat.tier_index].mast;
        solved[self.flat.tier_index].mast = mast;
        let planes = self.design.planes_from_solved(solved);
        solved[self.flat.tier_index].mast = previous;
        planes
    }

    /// Whether the flat's mast moves nothing but the offset of `self.planes[plane_index]`,
    /// given `self.planes` built at mast `mast`.
    ///
    /// The plane builder takes a tier's planes from that tier alone, gives a horizontal tier
    /// the offset [`flat_offset`], and drops a plane only for an earlier one with a coinciding
    /// normal and a close offset. So a rebuild at another mast has the same planes in the same
    /// order but for that offset, unless another tier can share this normal: a tier on the same
    /// side that is (nearly) flat too.
    fn can_patch(&self, mast: f64) -> bool {
        let Some(tier) = self.design.tiers.get(self.flat.tier_index) else {
            return false;
        };
        let side = tier_is_crown_side(tier.angle_deg);
        is_horizontal_angle_deg(tier.angle_deg)
            && self
                .planes
                .get(self.flat.plane_index)
                .is_some_and(|&(_, offset)| offset.to_bits() == flat_offset(mast).to_bits())
            && !self.design.tiers.iter().enumerate().any(|(index, other)| {
                index != self.flat.tier_index
                    && other.angle_deg.abs() < NEAR_FLAT_DEG
                    && tier_is_crown_side(other.angle_deg) == side
            })
    }

    /// The area of the flat's facet when its tier has mast `mast` and every other mast stays as
    /// it is in `solved`.
    fn area(&mut self, solved: &mut [SolvedTier], mast: f64) -> f64 {
        #[cfg(test)]
        AREA_PROBES.with(|probes| probes.set(probes.get() + 1));
        match self.patch {
            Patch::Verified => self.planes[self.flat.plane_index].1 = flat_offset(mast),
            Patch::Unbuilt => {
                self.planes = self.rebuilt(solved, mast);
                self.patch = if self.can_patch(mast) {
                    Patch::Unverified
                } else {
                    Patch::Off
                };
            }
            Patch::Unverified => {
                // The previous probe's list with only the flat's offset set, against the real
                // thing at this mast: equal, and later probes skip the rebuild.
                let mut patched = std::mem::take(&mut self.planes);
                patched[self.flat.plane_index].1 = flat_offset(mast);
                self.planes = self.rebuilt(solved, mast);
                self.patch = if same_planes(&patched, &self.planes) {
                    Patch::Verified
                } else {
                    Patch::Off
                };
            }
            Patch::Off => self.planes = self.rebuilt(solved, mast),
        }
        area_of_planes(
            &self.planes,
            self.flat.plane_index,
            self.method,
            &mut self.reference,
        )
    }
}

/// The scale of the stone `solved` describes: its largest mast magnitude (in a design
/// normalised the usual way that is the girdle's mast, 1), or 1 when nothing is solved.
///
/// Every mast scales together when a design is authored at another size, so the refit's
/// search bracket is cut to this instead of to fixed heights.
fn stone_scale(solved: &[SolvedTier]) -> f64 {
    let largest = solved
        .iter()
        .map(|tier| tier.mast.abs())
        .filter(|mast| mast.is_finite())
        .fold(0.0_f64, f64::max);
    if largest > 0.0 { largest } else { 1.0 }
}

/// Finds the mast of one flat tier that gives its facet `target` area, or `None` when the
/// bracket (relative to the stone's `scale`) does not enclose it.
fn bisect_mast(
    design: &Design,
    solved: &mut [SolvedTier],
    flat: FlatPlane,
    target: f64,
    scale: f64,
    method: AreaMethod,
    should_stop: &dyn Fn() -> bool,
) -> Option<f64> {
    let (mut low, mut high) = (BRACKET_LOW_FRACTION * scale, BRACKET_HIGH_FRACTION * scale);
    let mut probes = Probes::new(design, flat, method);
    // The low probe is the first, so it is the one that builds the mesh the clipper leans on.
    // A smaller mast is a flat nearer the girdle plane: a larger facet.
    if probes.area(solved, low) < target || probes.area(solved, high) > target {
        return None;
    }
    // A probe can be a whole mesh build: look at the stop flag before each one, the first right
    // after the two bracket probes.
    for _ in 0..MAX_STEPS {
        if should_stop() {
            return None;
        }
        let mid = f64::midpoint(low, high);
        if probes.area(solved, mid) > target {
            low = mid;
        } else {
            high = mid;
        }
        if high - low < STOP_WIDTH_FRACTION * scale {
            break;
        }
    }
    Some(f64::midpoint(low, high))
}

/// The refits that bring every pinned flat facet of `candidate` back to the size it had in
/// `original`.
///
/// `candidate_solved` is the candidate design's solved masts and `candidate_flats` its flat
/// rings. A flat whose facet is already within [`REFIT_TOLERANCE`] of the original size (in
/// linear size), a flat that is not pinned by a `ScaleReference`, and a flat the bisection
/// cannot bracket are all left out.
#[must_use]
pub fn refit_flats(
    candidate: &Design,
    candidate_solved: &[SolvedTier],
    candidate_flats: &[FlatRing],
    original: &StoneAnalysis,
    should_stop: &dyn Fn() -> bool,
) -> Vec<FlatRefit> {
    refit_flats_by(
        AreaMethod::Clipped,
        candidate,
        candidate_solved,
        candidate_flats,
        original,
        should_stop,
    )
}

/// [`refit_flats`] measuring its probes by `method`: [`AreaMethod::FullMesh`] is the reference
/// the clipped refit is tested against and benched next to.
#[must_use]
pub fn refit_flats_by(
    method: AreaMethod,
    candidate: &Design,
    candidate_solved: &[SolvedTier],
    candidate_flats: &[FlatRing],
    original: &StoneAnalysis,
    should_stop: &dyn Fn() -> bool,
) -> Vec<FlatRefit> {
    let ranges = tier_plane_ranges(candidate, candidate_solved);
    let scale = stone_scale(candidate_solved);
    let mut work = candidate_solved.to_vec();
    let mut refits = Vec::new();
    for flat in &original.flats {
        if should_stop() {
            break;
        }
        if scale_reference_mast(candidate, flat.tier_index).is_none() {
            continue;
        }
        let target = flat.area;
        if target.is_nan() || target <= 0.0 {
            continue;
        }
        let current = candidate_flats
            .iter()
            .find(|other| other.tier_index == flat.tier_index)
            .map_or(0.0, |other| other.area);
        if ((current / target).sqrt() - 1.0).abs() < REFIT_TOLERANCE {
            continue;
        }
        let Some(range) = ranges.get(flat.tier_index) else {
            continue;
        };
        let Some(new_mast) = bisect_mast(
            candidate,
            &mut work,
            FlatPlane {
                tier_index: flat.tier_index,
                plane_index: range.start,
            },
            target,
            scale,
            method,
            should_stop,
        ) else {
            continue;
        };
        work[flat.tier_index].mast = new_mast;
        refits.push(FlatRefit {
            tier_index: flat.tier_index,
            new_mast,
        });
    }
    refits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{retarget::validity::analyze, templates::gallery_design};
    use pleochroma::geometry::meet_solver::{MeetConstraint, SolveStrategy};
    use pleochroma_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta, templates::TEMPLATES};

    fn brilliant() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    fn tier_named(design: &Design, name: &str) -> usize {
        design.tiers.iter().position(|t| t.name == name).unwrap()
    }

    #[test]
    fn an_unchanged_stone_needs_no_refit() {
        let design = brilliant();
        let original = analyze(&design, false).unwrap();
        let refits = refit_flats(
            &design,
            &original.solved,
            &original.flats,
            &original,
            &|| false,
        );
        assert_eq!(refits, Vec::new());
    }

    #[test]
    fn a_table_that_grew_is_raised_back_to_its_original_size() {
        let design = brilliant();
        let original = analyze(&design, false).unwrap();
        let table = tier_named(&design, "Table");
        let original_area = original
            .flats
            .iter()
            .find(|flat| flat.tier_index == table)
            .unwrap()
            .area;

        // A lower table is a bigger table.
        let mut lowered = design;
        lowered.tiers[table].constraint = MeetConstraint::ScaleReference(0.2);
        let candidate = analyze(&lowered, false).unwrap();
        let grown = candidate
            .flats
            .iter()
            .find(|flat| flat.tier_index == table)
            .unwrap()
            .area;
        assert!(grown > original_area * 1.05, "{grown} vs {original_area}");

        let refits = refit_flats(
            &lowered,
            &candidate.solved,
            &candidate.flats,
            &original,
            &|| false,
        );
        let refit = refits
            .iter()
            .find(|refit| refit.tier_index == table)
            .expect("the table must be refitted");
        // Back to the original table height (0.32 in the template).
        assert!((refit.new_mast - 0.32).abs() < 1e-3, "{}", refit.new_mast);

        // And refitting really gives the size back.
        lowered.tiers[table].constraint = MeetConstraint::ScaleReference(refit.new_mast);
        let fitted = analyze(&lowered, false).unwrap();
        let area = fitted
            .flats
            .iter()
            .find(|flat| flat.tier_index == table)
            .unwrap()
            .area;
        assert!(
            (area / original_area - 1.0).abs() < 1e-3,
            "{area} vs {original_area}"
        );
    }

    /// The brilliant authored at another size: every `ScaleReference` mast multiplied by
    /// `scale`, in a preform block big enough to hold it.
    fn scaled_brilliant(scale: f64) -> Design {
        let mut tiers = ConstraintTier::standard_round_brilliant();
        for tier in &mut tiers {
            if let MeetConstraint::ScaleReference(mast) = &mut tier.constraint {
                *mast *= scale;
            }
        }
        Design::new(
            PreformSpec::block(2.0 * scale, 1.0, 4.0 * scale),
            ScheduleMeta::standard_round_brilliant(),
            tiers,
        )
    }

    #[test]
    fn a_stone_authored_at_a_larger_scale_still_gets_its_culet_refitted() {
        // The culet's own mast is 0.88 * 4 = 3.52 here, beyond the fixed 3.0 the bracket used
        // to stop at, so the old absolute bracket found nothing to refit.
        let scale = 4.0;
        let design = scaled_brilliant(scale);
        let original = analyze(&design, false).unwrap();
        let culet = tier_named(&design, "Culet");
        let original_area = original
            .flats
            .iter()
            .find(|flat| flat.tier_index == culet)
            .unwrap()
            .area;

        // A culet nearer the girdle is a bigger culet.
        let mut shallow = design;
        shallow.tiers[culet].constraint = MeetConstraint::ScaleReference(0.75 * scale);
        let candidate = analyze(&shallow, false).unwrap();
        let grown = candidate
            .flats
            .iter()
            .find(|flat| flat.tier_index == culet)
            .unwrap()
            .area;
        assert!(grown > original_area * 1.05, "{grown} vs {original_area}");

        let refits = refit_flats(
            &shallow,
            &candidate.solved,
            &candidate.flats,
            &original,
            &|| false,
        );
        let refit = refits
            .iter()
            .find(|refit| refit.tier_index == culet)
            .expect("the culet of a large-scale stone must be refitted");
        let original_culet_mast = 0.88 * scale;
        assert!(
            (refit.new_mast - original_culet_mast).abs() < 1e-3 * scale,
            "{}",
            refit.new_mast
        );
    }

    #[test]
    fn the_stone_scale_is_the_largest_mast_or_one() {
        let tier = |mast: f64| SolvedTier {
            mast,
            strategy: SolveStrategy::ScaleReference,
            detail: String::new(),
        };
        assert!((stone_scale(&[tier(0.3), tier(-4.0), tier(1.0)]) - 4.0).abs() < 1e-12);
        assert!((stone_scale(&[tier(f64::NAN), tier(0.5)]) - 0.5).abs() < 1e-12);
        assert!((stone_scale(&[]) - 1.0).abs() < 1e-12);
        assert!((stone_scale(&[tier(0.0)]) - 1.0).abs() < 1e-12);
    }

    /// The bisection used to look at the stop flag every eighth probe, so a cancelled Retarget
    /// paid for up to eight more whole-mesh builds per flat. It looks before every probe: a
    /// flag that rises at the second look costs the two bracket probes and one more.
    #[test]
    fn a_stop_request_costs_at_most_one_probe_after_the_bracket() {
        let design = brilliant();
        let original = analyze(&design, false).unwrap();
        let table = tier_named(&design, "Table");
        let target = original
            .flats
            .iter()
            .find(|flat| flat.tier_index == table)
            .unwrap()
            .area;
        let mut lowered = design;
        lowered.tiers[table].constraint = MeetConstraint::ScaleReference(0.2);
        let candidate = analyze(&lowered, false).unwrap();
        let ranges = tier_plane_ranges(&lowered, &candidate.solved);
        let flat = FlatPlane {
            tier_index: table,
            plane_index: ranges[table].start,
        };

        for method in [AreaMethod::Clipped, AreaMethod::FullMesh] {
            let mut work = candidate.solved.clone();
            let looks = std::cell::Cell::new(0_usize);
            let stop_from_the_second_look = || {
                looks.set(looks.get() + 1);
                looks.get() >= 2
            };
            AREA_PROBES.with(|probes| probes.set(0));
            let found = bisect_mast(
                &lowered,
                &mut work,
                flat,
                target,
                stone_scale(&candidate.solved),
                method,
                &stop_from_the_second_look,
            );
            assert_eq!(found, None);
            assert_eq!(looks.get(), 2);
            let probes = AREA_PROBES.with(std::cell::Cell::get);
            assert!(
                probes <= 3,
                "{method:?}: {probes} probes after the stop request"
            );
        }
    }

    #[test]
    fn a_stop_request_ends_the_refit_early() {
        let design = brilliant();
        let original = analyze(&design, false).unwrap();
        let table = tier_named(&design, "Table");
        let mut lowered = design;
        lowered.tiers[table].constraint = MeetConstraint::ScaleReference(0.2);
        let candidate = analyze(&lowered, false).unwrap();
        let refits = refit_flats(
            &lowered,
            &candidate.solved,
            &candidate.flats,
            &original,
            &|| true,
        );
        assert_eq!(refits, Vec::new());
    }

    // The clipped probe against the mesh probe. The clipper is a different algorithm, so the
    // figures agree to rounding (the mesh merges vertices closer than 1e-6 of the stone's scale
    // and gathers those within 2e-5 of the plane; a facet that is not an ordinary polygon is
    // given back to the mesh and agrees exactly), not bit for bit.

    const ROUND: &str = "Standard Round Brilliant";
    const OVAL: &str = "Oval Brilliant";
    const CUSHION: &str = "Cushion Brilliant";
    const EMERALD: &str = "Emerald Step Cut";
    const FIXTURES: [&str; 4] = [ROUND, OVAL, CUSHION, EMERALD];

    /// The template's design and its solved analysis: the four designs of the retarget bench.
    fn template_fixture(name: &str) -> (Design, StoneAnalysis) {
        let position = TEMPLATES
            .iter()
            .position(|template| template.name == name)
            .unwrap_or_else(|| panic!("template {name:?} is not in TEMPLATES"));
        let design = gallery_design(i32::try_from(position + 1).expect("index fits i32"));
        let analysis = analyze(&design, false).expect("the template must solve and close");
        (design, analysis)
    }

    fn assert_same_area(clipped: f64, mesh: f64, scale: f64, what: &str) {
        let slack = 1e-8_f64.mul_add(mesh, 1e-8 * scale * scale);
        assert!(
            (clipped - mesh).abs() <= slack,
            "{what}: clipped {clipped} vs mesh {mesh}"
        );
    }

    /// Every flat of every fixture, probed at masts from the bracket's low end to its high end
    /// (and the authored one): the clipped route against the mesh. Returns
    /// `(probes, clipped answers)`.
    fn scan(name: &str) -> (usize, usize) {
        let (design, analysis) = template_fixture(name);
        let scale = stone_scale(&analysis.solved);
        let mut probes = 0;
        let mut clipped_answers = 0;
        for ring in &analysis.flats {
            let flat = FlatPlane {
                tier_index: ring.tier_index,
                plane_index: analysis.ranges[ring.tier_index].start,
            };
            let authored = analysis.solved[flat.tier_index].mast;
            let masts: Vec<f64> = std::iter::once(BRACKET_LOW_FRACTION * scale)
                .chain((1..=40).map(|step| 0.04 * scale * f64::from(step)))
                .chain([BRACKET_HIGH_FRACTION * scale, authored])
                .collect();
            let mut work = analysis.solved.clone();
            let mut clipped = Probes::new(&design, flat, AreaMethod::Clipped);
            let mut mesh = Probes::new(&design, flat, AreaMethod::FullMesh);
            for mast in masts {
                let before = CLIPPED_ANSWERS.with(std::cell::Cell::get);
                let by_clip = clipped.area(&mut work, mast);
                let answered = CLIPPED_ANSWERS.with(std::cell::Cell::get) - before;
                let by_mesh = mesh.area(&mut work, mast);
                assert_same_area(
                    by_clip,
                    by_mesh,
                    scale,
                    &format!("{name} tier {} at mast {mast}", flat.tier_index),
                );
                if answered > 0 {
                    // A vanished flat is exactly the mesh's zero, never a rounding of it.
                    if by_mesh == 0.0 {
                        assert_eq!(by_clip, 0.0, "{name} tier {} at {mast}", flat.tier_index);
                    }
                    clipped_answers += 1;
                }
                probes += 1;
            }
        }
        (probes, clipped_answers)
    }

    #[test]
    fn clipped_probes_equal_mesh_probes_on_the_four_bench_designs() {
        let (mut probes, mut clipped) = (0, 0);
        for name in FIXTURES {
            let (design_probes, design_clipped) = scan(name);
            // The clipper must actually carry the bisection, not hand every probe back.
            assert!(
                design_clipped * 4 >= design_probes * 3,
                "{name}: the clipper answered {design_clipped} of {design_probes} probes"
            );
            probes += design_probes;
            clipped += design_clipped;
        }
        assert!(clipped * 4 >= probes * 3, "{clipped} of {probes}");
    }

    #[test]
    fn a_flat_that_vanishes_is_exactly_zero_and_one_that_spans_the_stone_matches_the_mesh() {
        let design = brilliant();
        let analysis = analyze(&design, false).unwrap();
        let scale = stone_scale(&analysis.solved);
        for name in ["Table", "Culet"] {
            let tier = tier_named(&design, name);
            let flat = FlatPlane {
                tier_index: tier,
                plane_index: analysis.ranges[tier].start,
            };
            let mut work = analysis.solved.clone();
            let mut prober = Probes::new(&design, flat, AreaMethod::Clipped);
            let mut mesh = Probes::new(&design, flat, AreaMethod::FullMesh);
            // The low end of the bracket: the flat covers the whole outline (and builds the
            // mesh the other probes lean on).
            let low = BRACKET_LOW_FRACTION * scale;
            let spanning = prober.area(&mut work, low);
            let spanning_mesh = mesh.area(&mut work, low);
            assert!(spanning > 1.0, "{name}: {spanning}");
            assert_eq!(spanning, spanning_mesh);
            // Far beyond the apex: gone, and gone without a mesh.
            MESH_BUILDS.with(|builds| builds.set(0));
            let gone = prober.area(&mut work, BRACKET_HIGH_FRACTION * scale);
            assert_eq!(gone, 0.0, "{name}");
            assert_eq!(MESH_BUILDS.with(std::cell::Cell::get), 0, "{name}");
            // Just beyond the authored flat's neighbourhood, up to a hair above it: sliver to
            // nothing, always the mesh's figure.
            let authored = analysis.solved[tier].mast;
            for factor in [0.9, 0.99, 1.0, 1.01, 1.05, 1.1, 1.2, 1.5] {
                let mast = authored * factor;
                let by_clip = prober.area(&mut work, mast);
                let by_mesh = mesh.area(&mut work, mast);
                assert_same_area(by_clip, by_mesh, scale, &format!("{name} at {mast}"));
            }
        }
    }

    #[test]
    fn a_solid_that_does_not_close_stays_zero_and_never_reaches_the_clipper() {
        // The box `[-1, 1]^3` without its top: the mesh calls it unbounded, so its floor has no
        // area, and with no closed solid to lean on the clipper stays out of it.
        let open = vec![
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::NEG_Y, 1.0),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
        ];
        for method in [AreaMethod::Clipped, AreaMethod::FullMesh] {
            let mut reference = None;
            for plane in 0..open.len() {
                assert_eq!(area_of_planes(&open, plane, method, &mut reference), 0.0);
            }
            assert!(reference.is_none());
        }
        // Closed again, the same reference slot gets its solid and the floor its area.
        let mut closed = open;
        closed.push((DVec3::Y, 1.0));
        let mut reference = None;
        let floor = area_of_planes(&closed, 2, AreaMethod::Clipped, &mut reference);
        assert!((floor - 4.0).abs() < 1e-12);
        assert!(reference.is_some());
        // And the clipped probe of a plane that stops closing it gives the mesh's zero.
        let mut opened = closed.clone();
        opened.pop();
        assert_eq!(
            area_of_planes(&opened, 2, AreaMethod::Clipped, &mut reference),
            0.0
        );
    }

    /// A probe's patched plane list is the list `planes_from_solved` builds, bit for bit.
    #[test]
    fn patched_planes_are_the_rebuilt_planes_on_the_four_bench_designs() {
        for name in FIXTURES {
            let (design, analysis) = template_fixture(name);
            let scale = stone_scale(&analysis.solved);
            for ring in &analysis.flats {
                let flat = FlatPlane {
                    tier_index: ring.tier_index,
                    plane_index: analysis.ranges[ring.tier_index].start,
                };
                let mut work = analysis.solved.clone();
                let mut probes = Probes::new(&design, flat, AreaMethod::Clipped);
                for step in 1..=12 {
                    let mast = 0.13 * scale * f64::from(step);
                    let _ = probes.area(&mut work, mast);
                    let mut at_mast = analysis.solved.clone();
                    at_mast[flat.tier_index].mast = mast;
                    assert!(
                        same_planes(&probes.planes, &design.planes_from_solved(&at_mast)),
                        "{name} tier {} at mast {mast} ({:?})",
                        flat.tier_index,
                        probes.patch
                    );
                    // Two rebuilds (the first and the check) and then patches, for a design with
                    // one flat per side.
                    if step >= 2 && name == ROUND {
                        assert_eq!(probes.patch, Patch::Verified, "{name} at step {step}");
                    }
                }
            }
        }
    }

    /// Another tier that is flat on the same side can share the flat's normal, so the plane
    /// builder may drop or keep a plane as the mast moves: the list is rebuilt at every probe.
    #[test]
    fn a_second_flat_on_the_same_side_turns_patching_off() {
        let design = brilliant();
        let analysis = analyze(&design, false).unwrap();
        let table = tier_named(&design, "Table");
        let flat = FlatPlane {
            tier_index: table,
            plane_index: analysis.ranges[table].start,
        };
        let mut work = analysis.solved.clone();
        let mut alone = Probes::new(&design, flat, AreaMethod::Clipped);
        let _ = alone.area(&mut work, 0.3);
        assert_eq!(alone.patch, Patch::Unverified);

        let mut crowded = design.clone();
        let mut second = crowded.tiers[table].clone();
        second.name = "Second table".to_string();
        second.angle_deg = 0.2;
        crowded.tiers.push(second);
        let mut crowded_solved = analysis.solved;
        crowded_solved.push(crowded_solved[table].clone());
        let mut probes = Probes::new(&crowded, flat, AreaMethod::Clipped);
        let _ = probes.area(&mut crowded_solved, 0.3);
        assert_eq!(probes.patch, Patch::Off);
    }

    #[test]
    fn a_bisection_builds_the_mesh_once_instead_of_at_every_probe() {
        let design = brilliant();
        let original = analyze(&design, false).unwrap();
        let table = tier_named(&design, "Table");
        let target = original
            .flats
            .iter()
            .find(|flat| flat.tier_index == table)
            .unwrap()
            .area;
        let mut lowered = design;
        lowered.tiers[table].constraint = MeetConstraint::ScaleReference(0.2);
        let candidate = analyze(&lowered, false).unwrap();
        let ranges = tier_plane_ranges(&lowered, &candidate.solved);
        let flat = FlatPlane {
            tier_index: table,
            plane_index: ranges[table].start,
        };
        let run = |method: AreaMethod| {
            let mut work = candidate.solved.clone();
            AREA_PROBES.with(|probes| probes.set(0));
            MESH_BUILDS.with(|builds| builds.set(0));
            let mast = bisect_mast(
                &lowered,
                &mut work,
                flat,
                target,
                stone_scale(&candidate.solved),
                method,
                &|| false,
            )
            .expect("the table brackets");
            (
                mast,
                AREA_PROBES.with(std::cell::Cell::get),
                MESH_BUILDS.with(std::cell::Cell::get),
            )
        };
        let (by_mesh, mesh_probes, mesh_builds) = run(AreaMethod::FullMesh);
        let (by_clip, clip_probes, clip_builds) = run(AreaMethod::Clipped);
        assert_eq!(mesh_probes, mesh_builds);
        // The same bracket and the same decisions, bar a probe that lands a rounding from the
        // target.
        assert!(
            clip_probes.abs_diff(mesh_probes) <= 1,
            "{clip_probes} vs {mesh_probes}"
        );
        assert!(clip_probes >= 30, "{clip_probes}");
        assert!(
            clip_builds <= 3,
            "{clip_builds} meshes for {clip_probes} probes"
        );
        assert!((by_clip - by_mesh).abs() < 1e-8, "{by_clip} vs {by_mesh}");
    }

    #[test]
    fn the_clipped_refit_gives_the_refits_of_the_mesh_on_the_four_bench_designs() {
        let mut compared = 0;
        for name in FIXTURES {
            let (design, original) = template_fixture(name);
            let scale = stone_scale(&original.solved);
            // Every pinned flat lower (bigger), and every one higher (smaller, a culet may go).
            for factor in [0.7, 1.3] {
                let mut candidate_design = design.clone();
                for flat in &original.flats {
                    if let Some(mast) = scale_reference_mast(&design, flat.tier_index) {
                        candidate_design.tiers[flat.tier_index].constraint =
                            MeetConstraint::ScaleReference(mast * factor);
                    }
                }
                let Ok(candidate) = analyze(&candidate_design, false) else {
                    continue;
                };
                let run = |method| {
                    refit_flats_by(
                        method,
                        &candidate_design,
                        &candidate.solved,
                        &candidate.flats,
                        &original,
                        &|| false,
                    )
                };
                let by_mesh = run(AreaMethod::FullMesh);
                let by_clip = run(AreaMethod::Clipped);
                assert_eq!(by_clip.len(), by_mesh.len(), "{name} x{factor}");
                for (clipped, mesh) in by_clip.iter().zip(&by_mesh) {
                    assert_eq!(clipped.tier_index, mesh.tier_index, "{name} x{factor}");
                    assert!(
                        (clipped.new_mast - mesh.new_mast).abs() < 1e-7 * scale,
                        "{name} x{factor}: {} vs {}",
                        clipped.new_mast,
                        mesh.new_mast
                    );
                    compared += 1;
                }
            }
        }
        assert!(compared > 0, "no design needed a refit to compare");
    }
}
