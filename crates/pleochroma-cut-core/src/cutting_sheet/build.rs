//! Builds a [`super::CuttingSheet`] from a [`Design`] and its already-solved
//! masts ([`Design::cutting_sheet`] and siblings), and [`Design::facet_meets`]:
//! which other tiers a tier's own `MeetConstraint::MeetNamed` resolves to,
//! using the same [`MeetNameResolver`] the solver itself uses.
//!
//! A tier placed by an explicit depth (`MeetConstraint::ScaleReference` without recorded
//! notes, or `MeetConstraint::MeetExisting`) names nothing it meets, so its instruction is
//! worked out from the finished stone ([`DepthPlacements`]) in the fantasy-cut template's
//! convention:
//! - the pavilion mains ending in one point are cut to the centerpoint;
//! - the first girdle tier sets the stone size ([`Design::sizing_tier`], shared with the
//!   `.asc` export);
//! - the first crown tier, where it reaches the girdle, sets the girdle width;
//! - every other one meets the earlier-cut tiers whose facets share a vertex with its own.

use super::sheet::{ConcaveRowInfo, CutSheetRow, CuttingSheet};
use crate::{
    crystal_axis::CrystalOrientation,
    design::{
        ConcaveTier, ConstraintTier, Design, SolveMismatch, TierLabelInfo, TierRef,
        cutting_order::meet_inputs,
    },
    edit::EditError,
    manufacturability::facet_plane_boundaries,
};
use glam::DVec3;
use pleochroma::{
    geometry::{
        meet_solver::{MeetConstraint, MeetNameResolver, SolvedTier, TokenResolution},
        plane::tier_is_crown_side,
        stone_metrics::{SolidStatus, build_solid_mesh},
    },
    optics::materials::GemMaterial,
};

/// Two facet corners closer than this fraction of the stone's size are one vertex: the
/// meets [`DepthPlacements`] lists. The solid mesh already welds its vertices (faces that
/// meet in a vertex carry the same position), so this only absorbs rounding.
const SHARED_VERTEX_FRACTION: f64 = 1e-6;

/// A vertex closer to the stone's axis than this fraction of the stone's size lies on the
/// axis, for "Cut to centerpoint.". Looser than [`SHARED_VERTEX_FRACTION`]: the apex where
/// a tier's facets meet is solved from `f32` facet normals, so it sits a few `1e-7` of the
/// size off the axis; `1e-4` of a 7 mm stone is under a micrometre.
const ON_AXIS_FRACTION: f64 = 1e-4;

/// One name of a `MeetNamed` instruction as the sheet prints it: the code of the tier it
/// names (`Pavilion Main` becomes `P1`), `-`-joined for a compound vertex spec
/// (`1-2-G1` becomes `P1-P2-G1`). A word that names no tier (a meet-point word such as
/// `PCP`, connective prose, a name that does not resolve) stays as the designer wrote it.
fn meet_name_as_code(
    name: &str,
    resolver: &MeetNameResolver<'_>,
    codes: &[TierLabelInfo],
) -> String {
    if let TokenResolution::Tiers(tiers) = resolver.resolve_token(name) {
        let parts: Option<Vec<&str>> = tiers
            .iter()
            .map(|&tier| codes.get(tier).map(|label| label.code.as_str()))
            .collect();
        if let Some(parts) = parts.filter(|parts| !parts.is_empty()) {
            return parts.join("-");
        }
    }
    name.trim().to_owned()
}

/// The recorded `.asc` notes a tier prints verbatim: non-blank notes of a tier whose
/// constraint is still the [`MeetConstraint::ScaleReference`] its import pinned. `None` for
/// every other tier.
fn recorded_notes(tier: &ConstraintTier) -> Option<&str> {
    match (&tier.constraint, &tier.original_notes) {
        (MeetConstraint::ScaleReference(_), Some(notes)) if !notes.trim().is_empty() => {
            Some(notes.as_str())
        }
        _ => None,
    }
}

/// `true` for a tier whose instruction the sheet works out itself ([`DepthPlacement`]): a
/// [`MeetConstraint::MeetExisting`] tier, or a [`MeetConstraint::ScaleReference`] one without
/// recorded notes. Such a tier names nothing it meets.
fn placed_by_depth(tier: &ConstraintTier) -> bool {
    !matches!(tier.constraint, MeetConstraint::MeetNamed(_)) && recorded_notes(tier).is_none()
}

/// What the sheet prints for a tier placed by an explicit depth ([`placed_by_depth`]), as
/// [`DepthPlacements::placement`] works it out from the finished stone.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum DepthPlacement {
    /// The tier [`Design::sizing_tier`] names, the first explicit-depth girdle tier in cutting
    /// order: its depth sets the size of the stone. `"Set stone size."`
    SetStoneSize,
    /// The first tier in cutting order whose facets (at least three) all end in one point on
    /// the stone's axis: the pavilion mains meeting at the culet point, or a crown closing to
    /// a point. `"Cut to centerpoint."`
    CutToCenterpoint,
    /// The first crown tier in cutting order (the table excepted), when its facets reach the
    /// girdle: its depth sets how wide the girdle is. `"Set girdle width."`
    SetGirdleWidth,
    /// The tiers (indices into `Design::tiers`) cut before this one whose facets share a
    /// vertex with one of its facets in the finished stone, in cutting order. Never empty.
    /// `"Meet P1, P2, G1"`, without a full stop, as the cutting-instructions template writes it.
    Meets(Vec<usize>),
    /// Nothing cut earlier touches the tier (or the stone does not close): its depth.
    /// `"Cut to depth 2.81 mm."` when the girdle diameter gives millimetres, else
    /// `"Set to mast depth 0.6700"`.
    Depth {
        /// The solved mast.
        mast: f64,
        /// The mast in millimetres, when the design's girdle diameter is set.
        depth_mm: Option<f64>,
    },
}

impl DepthPlacement {
    /// The printed instruction. Tiers are written as their `codes` (`P1`, `G1`, `T`).
    pub(super) fn text(&self, codes: &[TierLabelInfo]) -> String {
        match self {
            Self::SetStoneSize => "Set stone size.".to_string(),
            Self::CutToCenterpoint => "Cut to centerpoint.".to_string(),
            Self::SetGirdleWidth => "Set girdle width.".to_string(),
            Self::Meets(tiers) => format!(
                "Meet {}",
                tiers
                    .iter()
                    .filter_map(|&tier| codes.get(tier).map(|label| label.code.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Depth {
                depth_mm: Some(mm), ..
            } => format!("Cut to depth {mm:.2} mm."),
            Self::Depth {
                mast,
                depth_mm: None,
            } => format!("Set to mast depth {mast:.4}"),
        }
    }
}

/// Builds one tier's printable meet instruction -- see
/// [`super::sheet::CutSheetRow::meet_instruction`]. Mirrors
/// `design::export`'s own notes-vs-synthesized precedence (recorded notes
/// text while the constraint is still the pinned import value, else
/// synthesized from the constraint), but only ever accepts non-blank
/// recorded notes and never prints an empty string.
///
/// A recorded note is the designer's own text and prints verbatim. `MeetNamed`
/// targets print as their tiers' `codes` (`Meet P1, P2, G1`), in the order the
/// constraint stores them. A tier placed by an explicit depth
/// ([`MeetConstraint::ScaleReference`] without notes, [`MeetConstraint::MeetExisting`])
/// prints `placement` ([`DepthPlacement::text`]): `"Set stone size."`, `"Cut to
/// centerpoint."`, `"Set girdle width."`, the computed `"Meet P1, P2, G1"`, or its depth.
pub(super) fn meet_instruction(
    tier: &ConstraintTier,
    resolver: &MeetNameResolver<'_>,
    codes: &[TierLabelInfo],
    placement: &DepthPlacement,
) -> String {
    if let Some(notes) = recorded_notes(tier) {
        return notes.to_owned();
    }
    match &tier.constraint {
        MeetConstraint::MeetNamed(names) => format!(
            "Meet {}",
            names
                .iter()
                .map(|name| meet_name_as_code(name, resolver, codes))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        MeetConstraint::MeetExisting | MeetConstraint::ScaleReference(_) => placement.text(codes),
    }
}

/// What the finished stone says about its tiers: which tiers' facets share a vertex, and
/// which tiers' facets all end in one point on the axis. Built once per sheet by
/// [`StoneContacts::of`].
struct StoneContacts {
    /// How many flat tiers the design has.
    tier_count: usize,
    /// `touching[a * tier_count + b]`: a facet of tier `a` and a facet of tier `b` share a
    /// vertex. Symmetric.
    touching: Vec<bool>,
    /// Per tier: at least three facets, and all of them share one vertex on the axis.
    converges: Vec<bool>,
}

impl StoneContacts {
    /// The contacts of `design`'s finished stone: the solid
    /// [`Design::planes_from_solved`] bounds, each face given to the tier that cut it the
    /// way [`Design::tier_for_plane_index`] does. Preform faces belong to no tier and take no
    /// part. `None` when the planes do not close into a solid.
    ///
    /// `solved` must be aligned with `design.tiers` (the caller checks it).
    fn of(design: &Design, solved: &[SolvedTier]) -> Option<Self> {
        let SolidStatus::Closed(mesh) = build_solid_mesh(&design.planes_from_solved(solved)) else {
            return None;
        };
        let preform_len = design.preform.planes().len();
        let boundaries = facet_plane_boundaries(&design.to_asc_schedule_from_solved(solved));
        let tier_of = |plane: usize| {
            let local = plane.checked_sub(preform_len)?;
            let tier = boundaries.partition_point(|&end| end <= local);
            (tier < boundaries.len()).then_some(tier)
        };
        let faces: Vec<(usize, &[DVec3])> = mesh
            .rings
            .iter()
            .filter_map(|(plane, ring)| Some((tier_of(*plane)?, ring.as_slice())))
            .collect();

        let (lo, hi) = faces.iter().flat_map(|(_, ring)| ring.iter()).fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(lo, hi), &p| (lo.min(p), hi.max(p)),
        );
        let size = (hi - lo).max_element();
        let size = if size.is_finite() && size > 0.0 {
            size
        } else {
            1.0
        };
        let shared = SHARED_VERTEX_FRACTION * size;
        let on_axis = ON_AXIS_FRACTION * size;

        let tier_count = design.tiers.len();
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

        Some(Self {
            tier_count,
            touching,
            converges,
        })
    }

    /// Whether a facet of tier `a` shares a vertex with a facet of tier `b`.
    fn touch(&self, a: usize, b: usize) -> bool {
        self.touching
            .get(a * self.tier_count + b)
            .copied()
            .unwrap_or(false)
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

/// The instruction of every tier placed by an explicit depth ([`placed_by_depth`]), worked
/// out once per sheet: which tier sets the stone size, which is cut to the centerpoint, which
/// sets the girdle width, and which earlier-cut tiers each one meets in the finished stone.
struct DepthPlacements {
    /// [`Design::sizing_tier`] (with or without notes, so the sheet and the `.asc` export name
    /// the same tier).
    sizing_tier: Option<usize>,
    /// The first tier in cutting order whose facets all end in one point on the axis.
    centerpoint_tier: Option<usize>,
    /// The first crown tier in cutting order (the table excepted), when one of its facets
    /// shares a vertex with a girdle tier's (or the design has no girdle tier, so the
    /// preform's walls are the girdle).
    girdle_width_tier: Option<usize>,
    /// Per tier (index into `Design::tiers`): the tiers cut before it whose facets share a
    /// vertex with its own, in cutting order. All empty when nothing needed the stone or the
    /// stone does not close.
    earlier_meets: Vec<Vec<usize>>,
}

impl DepthPlacements {
    /// Works the placements out for `design` cut in `order` (its [`Design::cutting_order`]; the
    /// concave tiers in it take no part). Builds the finished solid once, and only when some
    /// tier other than the one setting the stone size needs it.
    ///
    /// `solved` must be aligned with `design.tiers` (the caller checks it).
    fn new(design: &Design, solved: &[SolvedTier], order: &[TierRef]) -> Self {
        let flat_order: Vec<usize> = order
            .iter()
            .filter_map(|tier_ref| match tier_ref {
                TierRef::Flat(i) => Some(*i),
                TierRef::Concave(_) => None,
            })
            .collect();
        let sizing_tier = design.sizing_tier();
        let mut placements = Self {
            sizing_tier,
            centerpoint_tier: None,
            girdle_width_tier: None,
            earlier_meets: vec![Vec::new(); design.tiers.len()],
        };
        let needs_stone = flat_order
            .iter()
            .any(|&i| Some(i) != sizing_tier && placed_by_depth(&design.tiers[i]));
        if !needs_stone {
            return placements;
        }
        let Some(contacts) = StoneContacts::of(design, solved) else {
            return placements;
        };
        placements.centerpoint_tier = flat_order
            .iter()
            .copied()
            .find(|&i| contacts.converges.get(i).copied().unwrap_or(false));
        let girdles: Vec<usize> = flat_order
            .iter()
            .copied()
            .filter(|&i| design.tiers[i].is_girdle())
            .collect();
        placements.girdle_width_tier = flat_order
            .iter()
            .copied()
            .find(|&i| {
                let tier = &design.tiers[i];
                !tier.is_table() && !tier.is_girdle() && tier_is_crown_side(tier.angle_deg)
            })
            .filter(|&crown| {
                girdles.is_empty() || girdles.iter().any(|&girdle| contacts.touch(crown, girdle))
            });
        for (position, &tier) in flat_order.iter().enumerate() {
            placements.earlier_meets[tier] = flat_order[..position]
                .iter()
                .copied()
                .filter(|&earlier| contacts.touch(tier, earlier))
                .collect();
        }
        placements
    }

    /// The placement of tier `tier` (index into `Design::tiers`), whose solved mast is `mast`
    /// (`depth_mm` in millimetres when known). Only printed for a tier [`placed_by_depth`].
    fn placement(&self, tier: usize, mast: f64, depth_mm: Option<f64>) -> DepthPlacement {
        if self.sizing_tier == Some(tier) {
            return DepthPlacement::SetStoneSize;
        }
        if self.centerpoint_tier == Some(tier) {
            return DepthPlacement::CutToCenterpoint;
        }
        if self.girdle_width_tier == Some(tier) {
            return DepthPlacement::SetGirdleWidth;
        }
        match self.earlier_meets.get(tier) {
            Some(meets) if !meets.is_empty() => DepthPlacement::Meets(meets.clone()),
            _ => DepthPlacement::Depth { mast, depth_mm },
        }
    }
}

/// A concave tier's facet-line row. It has no mast and no depth (its cut is
/// bounded by the tool, not by a solved plane), so both read as zero or absent
/// and the meet column prints the tier's own instructions, else "cut to depth"; the tool line's values ride in
/// [`CutSheetRow::concave`]. `code` is the tier's code (`P4`).
fn concave_row(tier: &ConcaveTier, code: &str, sequence: usize) -> CutSheetRow {
    CutSheetRow {
        sequence,
        code: code.to_owned(),
        name: tier.name.clone(),
        angle_deg: tier.angle_deg.abs(),
        indices: tier.indices.clone(),
        mast: 0.0,
        meet_instruction: if tier.instructions.trim().is_empty() {
            "cut to depth".to_string()
        } else {
            tier.instructions.clone()
        },
        meets_tiers: Vec::new(),
        cheater_offset_deg: None,
        angle_of_elevation_deg: tier.angle_deg.abs(),
        depth_mm: None,
        concave: Some(ConcaveRowInfo {
            tool: tier.tool,
            tool_azimuth_deg: tier.tool_azimuth_deg,
            displacement: tier.displacement,
            diameter_ratio: tier.diameter_ratio,
            tool_angle_deg: tier.tool_angle_deg,
            motion: tier.motion,
        }),
        // A concave tier cannot be frosted.
        frosting: None,
    }
}

/// Resolves `tier`'s own [`MeetConstraint::MeetNamed`] against an
/// already-built `resolver` -- the shared body of [`Design::facet_meets`]
/// and [`Design::cutting_sheet`], so the latter builds one
/// [`MeetNameResolver`] for the whole design instead of one per row.
/// `MeetExisting`/`ScaleReference` both resolve to an empty list, same as
/// [`Design::facet_meets`] documents.
fn resolve_meets(tier: &ConstraintTier, resolver: &MeetNameResolver<'_>) -> Vec<usize> {
    let MeetConstraint::MeetNamed(names) = &tier.constraint else {
        return Vec::new();
    };
    resolver.resolve_names(names).refs
}

impl Design {
    /// Which other tiers (by index into [`Self::tiers`]) the tier at
    /// `tier_index` actually meets, resolved the same way [`Self::solve`]
    /// itself resolves a `MeetNamed` instruction -- [`MeetNameResolver`], not
    /// a naive name match, so girdle/culet/table fallbacks, side-prefix and
    /// plural stripping, and compound vertex specs all behave identically to
    /// the solver.
    ///
    /// [`MeetConstraint::MeetExisting`] (the solver picks a candidate vertex
    /// without ever naming facets) and [`MeetConstraint::ScaleReference`]
    /// (an authored dimension, not a meet at all) both resolve to an empty
    /// list -- there is nothing named to report for either. Only
    /// [`MeetConstraint::MeetNamed`] has a real answer.
    ///
    /// # Errors
    ///
    /// [`EditError`] if `tier_index` is out of range.
    pub fn facet_meets(&self, tier_index: usize) -> Result<Vec<usize>, EditError> {
        let tier_count = self.tiers.len();
        let tier = self.tiers.get(tier_index).ok_or(EditError {
            index: tier_index,
            tier_count,
        })?;
        let inputs = meet_inputs(&self.tiers);
        let resolver = MeetNameResolver::new(&inputs);
        Ok(resolve_meets(tier, &resolver))
    }

    /// Builds this design's printable cutting sequence from an
    /// already-[`Self::solve`]'d (or [`Self::resolve_dirty`]'d) mast list --
    /// see [`CuttingSheet`] for what it carries and why a cutter needs it.
    /// Never solves again itself, same reasoning as
    /// [`Self::to_asc_schedule_from_solved`].
    ///
    /// Built-ins-only for the printed "Refractive index" line -- see
    /// [`Self::effective_refractive_index`]'s own doc comment. A caller with a
    /// resolved custom catalogue on hand should call [`Self::cutting_sheet_with`]
    /// instead.
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::to_asc_schedule_from_solved`]:
    /// `solved` must have one entry per tier `self` currently has, in the
    /// same order.
    #[must_use]
    pub fn cutting_sheet(&self, solved: &[SolvedTier]) -> CuttingSheet {
        self.cutting_sheet_with(solved, &[])
    }

    /// Catalogue-aware sibling of [`Self::cutting_sheet`]: also consults `custom` --
    /// see [`Self::effective_refractive_index_with`] -- so the printed "Refractive
    /// index" line reflects a CUSTOM catalogue material's own `n_D` instead of
    /// silently falling back to the legacy schedule RI. Passing `&[]` behaves
    /// exactly like [`Self::cutting_sheet`] (that method's own implementation, in
    /// fact).
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::cutting_sheet`].
    #[must_use]
    pub fn cutting_sheet_with(
        &self,
        solved: &[SolvedTier],
        custom: &[GemMaterial],
    ) -> CuttingSheet {
        self.try_cutting_sheet_with(solved, custom)
            .unwrap_or_else(|e| {
                panic!(
                    "cutting_sheet: `solved` ({} masts) is not aligned with this design's current \
                 {} tier(s)",
                    e.got_tiers, e.expected_tiers
                )
            })
    }

    /// Fallible sibling of [`Self::cutting_sheet`]: returns [`SolveMismatch`]
    /// instead of panicking when `solved` is not aligned with
    /// [`Self::tiers`]. Identical construction otherwise.
    ///
    /// # Errors
    ///
    /// [`SolveMismatch`] when `solved.len() != self.tiers.len()`.
    pub fn try_cutting_sheet(&self, solved: &[SolvedTier]) -> Result<CuttingSheet, SolveMismatch> {
        self.try_cutting_sheet_with(solved, &[])
    }

    /// Catalogue-aware, fallible sibling of [`Self::cutting_sheet_with`] -- returns
    /// [`SolveMismatch`] instead of panicking, exactly like
    /// [`Self::try_cutting_sheet`] but also consulting `custom` for the printed
    /// "Refractive index" line (see [`Self::effective_refractive_index_with`]).
    /// Every other cutting-sheet entry point in this impl ultimately calls through
    /// here, so this is the one place that line is actually decided.
    ///
    /// # Errors
    ///
    /// [`SolveMismatch`] when `solved.len() != self.tiers.len()`.
    pub fn try_cutting_sheet_with(
        &self,
        solved: &[SolvedTier],
        custom: &[GemMaterial],
    ) -> Result<CuttingSheet, SolveMismatch> {
        self.try_cutting_sheet_with_default(solved, custom, None)
    }

    /// [`Self::cutting_sheet_with`] with the stored default orientation of the design's
    /// material (`LibraryMaterial::default_orientation` of its library row), so a custom
    /// material whose cutter explicitly set "tilt 0" still prints the Orientation block. With
    /// `None` it is [`Self::cutting_sheet_with`].
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::cutting_sheet`].
    #[must_use]
    pub fn cutting_sheet_with_default(
        &self,
        solved: &[SolvedTier],
        custom: &[GemMaterial],
        stored_default: Option<CrystalOrientation>,
    ) -> CuttingSheet {
        self.try_cutting_sheet_with_default(solved, custom, stored_default)
            .unwrap_or_else(|e| {
                panic!(
                    "cutting_sheet: `solved` ({} masts) is not aligned with this design's current \
                 {} tier(s)",
                    e.got_tiers, e.expected_tiers
                )
            })
    }

    /// [`Self::try_cutting_sheet_with`] with the stored default orientation of the design's
    /// material, as for [`Self::cutting_sheet_with_default`].
    ///
    /// # Errors
    ///
    /// [`SolveMismatch`] when `solved.len() != self.tiers.len()`.
    pub fn try_cutting_sheet_with_default(
        &self,
        solved: &[SolvedTier],
        custom: &[GemMaterial],
        stored_default: Option<CrystalOrientation>,
    ) -> Result<CuttingSheet, SolveMismatch> {
        if solved.len() != self.tiers.len() {
            return Err(SolveMismatch {
                expected_tiers: self.tiers.len(),
                got_tiers: solved.len(),
            });
        }

        let mut header = vec![format!(
            "Material: {}",
            self.material.name.as_deref().unwrap_or("(unset)")
        )];
        header.push(format!(
            "Refractive index: {:.3}",
            self.effective_refractive_index_with(custom)
        ));
        header.push(format!(
            "Index gear: {} teeth, symmetry {}{}",
            self.meta.gear_teeth_abs(),
            self.meta.symmetry_order,
            if self.meta.mirror { ", mirrored" } else { "" }
        ));
        if let Some(mm) = self.girdle_diameter_mm {
            header.push(format!("Girdle diameter: {mm:.3} mm"));
        }
        // Carat weight, with the same understated-weight caveat
        // `crate::yield_metrics::report`'s own module doc comment documents
        // (facets that don't reach the preform's own walls leave `width_axis`
        // reading as the PREFORM's width, not the actual narrower girdle) --
        // shown next to the number instead of only in a doc comment nobody
        // printing a sheet ever reads.
        let yield_report = self.yield_report(solved);
        if let Some(carat) = yield_report.carat_weight {
            header.push(format!(
                "Carat weight (estimate): {carat:.3} ct -- assumes the authored facets reach \
                 the preform's own walls; a stone left smaller than its rough reads lighter \
                 than this"
            ));
        }
        let mm_per_unit = yield_report.mm_per_unit;
        // Where the crystal axis must run in the rough, for a stone whose axis is set; the block
        // is also part of the text header. Nothing for an isotropic or unset stone.
        let orientation = self.sheet_orientation_with_default(custom, stored_default);
        if let Some(block) = &orientation {
            header.extend(block.lines());
        }

        let inputs = meet_inputs(&self.tiers);
        let resolver = MeetNameResolver::new(&inputs);
        let codes = self.tier_codes();
        // Every design -- planar or not -- is printed in `cutting_order`: the pavilion
        // section first, each tool line at the end of its section, the table last.
        let order = self.cutting_order();
        let placements = DepthPlacements::new(self, solved, &order);
        let flat_row = |i: usize, sequence: usize| {
            let (tier, solved_tier) = (&self.tiers[i], &solved[i]);
            let depth_mm = mm_per_unit.map(|scale| solved_tier.mast * scale);
            let placement = placements.placement(i, solved_tier.mast, depth_mm);
            CutSheetRow {
                sequence,
                code: codes.flat[i].code.clone(),
                name: tier.name.clone(),
                angle_deg: tier.angle_deg.abs(),
                indices: tier.indices.clone(),
                mast: solved_tier.mast,
                meet_instruction: meet_instruction(tier, &resolver, &codes.flat, &placement),
                meets_tiers: resolve_meets(tier, &resolver),
                cheater_offset_deg: self.cheater_offset_deg(i),
                angle_of_elevation_deg: tier.angle_deg.abs(),
                depth_mm,
                concave: None,
                frosting: self.tier_frosting(i).cloned(),
            }
        };
        let rows = order
            .into_iter()
            .enumerate()
            .map(|(position, tier_ref)| match tier_ref {
                TierRef::Flat(i) => flat_row(i, position + 1),
                TierRef::Concave(i) => {
                    concave_row(&self.concave_tiers[i], &codes.concave[i].code, position + 1)
                }
            })
            .collect();

        Ok(CuttingSheet {
            header,
            rows,
            orientation,
        })
    }
}
