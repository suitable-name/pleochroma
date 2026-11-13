//! Builds a [`super::CuttingSheet`] from a [`Design`] and its already-solved
//! masts ([`Design::cutting_sheet`] and siblings), and [`Design::facet_meets`]:
//! which other tiers a tier's own `MeetConstraint::MeetNamed` resolves to,
//! using the same [`MeetNameResolver`] the solver itself uses.
//!
//! A tier placed by an explicit depth (`MeetConstraint::ScaleReference` without recorded
//! notes, or `MeetConstraint::MeetExisting`) names nothing it meets, so its instruction is
//! worked out from the stone as it is when the tier is cut ([`Design::cut_meets`]), in the
//! fantasy-cut template's convention:
//! - the first girdle tier sets the stone size ([`Design::sizing_tier`], shared with the
//!   `.asc` export);
//! - the pavilion mains ending in one point are cut to the centerpoint, unless they meet
//!   points already on the stone;
//! - the first crown tier (or, in a design without one, the table), where it reaches the girdle,
//!   sets the girdle width;
//! - the table, when the stone has no point a table could meet (only the crown's apex and the
//!   girdle's corners), sets the table size;
//! - a pavilion tier cut while the stone has no point to meet yet, which reaches a girdle tier
//!   cut before it, sets the girdle line;
//! - every other one meets the earlier-cut tiers whose facets form the points its own facets
//!   pass through, or, when one of its facets passes through none, gives its depth alone (the
//!   editor's Checks, not the sheet, say that it has no meet point).
//!
//! Only the sheet prints these computed instructions. The `.asc` and `.gcs` exports write
//! "Set stone size." on the sizing tier alone: a written "Meet P1" would come back from the
//! next import as a named meet to adopt (`ConstraintTier::imported_meet`).

use super::sheet::{ConcaveRowInfo, CutSheetRow, CuttingSheet};
use crate::{
    crystal_axis::CrystalOrientation,
    design::{
        ConcaveTier, ConstraintTier, CutMeet, CutMeetAnchor, Design, SolveMismatch, TierLabelInfo,
        TierRef, cutting_order::meet_inputs,
    },
    edit::EditError,
};
use pleochroma::{
    geometry::meet_solver::{MeetConstraint, MeetNameResolver, SolvedTier, TokenResolution},
    optics::materials::GemMaterial,
};

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

/// What the sheet prints for a tier placed by an explicit depth ([`placed_by_depth`]), from its
/// meet on the stone cut so far ([`Design::cut_meets`], [`DepthPlacement::of`]).
#[derive(Debug, Clone, PartialEq)]
pub(super) enum DepthPlacement {
    /// The tier [`Design::sizing_tier`] names, the first explicit-depth girdle tier in cutting
    /// order: its depth sets the size of the stone. `"Set stone size."`
    SetStoneSize,
    /// The first tier in cutting order whose facets (at least three) all end in one point on
    /// the stone's axis: the pavilion mains meeting at the culet point, or a crown closing to
    /// a point. Unless it meets points already on the stone, which it then prints instead.
    /// `"Cut to centerpoint."`
    CutToCenterpoint,
    /// The first crown tier in cutting order (the table excepted), or the table of a design with
    /// no other flat crown tier, when its facets reach the girdle: its depth sets how wide the
    /// girdle is. `"Set girdle width."`
    SetGirdleWidth,
    /// The table, when the stone cut so far has no point a table could meet (every point is on
    /// the axis or on the girdle line) and it meets none: its depth sets the size of the table.
    /// `"Set table size."`
    SetTableSize,
    /// A pavilion tier cut while the stone has no point to meet yet, whose facets reach a girdle
    /// tier cut before it: its depth sets where the girdle's lower edge runs.
    /// `"Set girdle line."`
    SetGirdleLine,
    /// The tiers (indices into `Design::tiers`) cut before this one whose facets form the
    /// vertices its facets pass through on the stone cut so far, in cutting order. Never empty.
    /// `"Meet P1, P2, G1"`, without a full stop, as the cutting-instructions template writes it.
    Meets(Vec<usize>),
    /// The stone cut so far has vertices, but a facet of the tier passes through none of them,
    /// so the cutter has no point to stop at: its depth, worded as [`Self::Depth`]'s. The
    /// editor's Checks name these tiers; the sheet does not warn on each row.
    NoMeetPoint {
        /// The solved mast.
        mast: f64,
        /// The mast in millimetres, when the design's girdle diameter is set.
        depth_mm: Option<f64>,
    },
    /// The stone cut so far has no vertex to meet yet (or the stone does not close): its depth.
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
    /// The placement of a tier whose cut-so-far meet is `meet` ([`Design::cut_meets`]; `None`
    /// when the stone does not close or nothing needed it) and whose solved mast is `mast`
    /// (`depth_mm` in millimetres when known).
    fn of(meet: Option<&CutMeet>, mast: f64, depth_mm: Option<f64>) -> Self {
        match meet {
            Some(CutMeet::Anchor(CutMeetAnchor::SetStoneSize)) => Self::SetStoneSize,
            Some(CutMeet::Anchor(CutMeetAnchor::CutToCenterpoint)) => Self::CutToCenterpoint,
            Some(CutMeet::Anchor(CutMeetAnchor::SetGirdleWidth)) => Self::SetGirdleWidth,
            Some(CutMeet::Anchor(CutMeetAnchor::SetTableSize)) => Self::SetTableSize,
            Some(CutMeet::Anchor(CutMeetAnchor::SetGirdleLine)) => Self::SetGirdleLine,
            Some(CutMeet::Meets(tiers)) => Self::Meets(tiers.clone()),
            Some(CutMeet::Floats { .. }) => Self::NoMeetPoint { mast, depth_mm },
            Some(CutMeet::Anchor(CutMeetAnchor::NothingToMeet)) | None => {
                Self::Depth { mast, depth_mm }
            }
        }
    }

    /// The printed instruction. Tiers are written as their `codes` (`P1`, `G1`, `T`).
    pub(super) fn text(&self, codes: &[TierLabelInfo]) -> String {
        match self {
            Self::SetStoneSize => "Set stone size.".to_string(),
            Self::CutToCenterpoint => "Cut to centerpoint.".to_string(),
            Self::SetGirdleWidth => "Set girdle width.".to_string(),
            Self::SetTableSize => "Set table size.".to_string(),
            Self::SetGirdleLine => "Set girdle line.".to_string(),
            Self::Meets(tiers) => format!(
                "Meet {}",
                tiers
                    .iter()
                    .filter_map(|&tier| codes.get(tier).map(|label| label.code.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::NoMeetPoint {
                depth_mm: Some(mm), ..
            }
            | Self::Depth {
                depth_mm: Some(mm), ..
            } => format!("Cut to depth {mm:.2} mm."),
            Self::NoMeetPoint {
                mast,
                depth_mm: None,
            }
            | Self::Depth {
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
/// centerpoint."`, `"Set girdle width."`, `"Set table size."`, `"Set girdle line."`, the
/// computed `"Meet P1, P2, G1"`, or its depth (`"Cut to depth 2.81 mm."`).
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

/// A concave tier's facet-line row. It has no mast and no depth (its cut is bounded by the
/// tool, not by a solved plane), so both read as zero or absent. The meet column prints the
/// tier's own instructions, else "cut to depth"; the tool line's values ride in
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
        // Carat weight, with its caveat on the same line: the estimate assumes the facets reach
        // the preform's walls, so a stone left smaller than its rough reads lighter than it
        // (see `crate::yield_metrics::report`).
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
        // The sizing tier (with or without notes, so the sheet and the `.asc` export name the
        // same tier) needs no stone. The stone cut so far is worked out only when another tier
        // prints a placement, one solid per tier.
        let sizing_tier = self.sizing_tier();
        let needs_stone = order.iter().any(|tier_ref| {
            matches!(tier_ref, TierRef::Flat(i)
                if Some(*i) != sizing_tier && placed_by_depth(&self.tiers[*i]))
        });
        let cut_meets = if needs_stone {
            self.cut_meets(solved)
        } else {
            None
        };
        let flat_row = |i: usize, sequence: usize| {
            let (tier, solved_tier) = (&self.tiers[i], &solved[i]);
            let depth_mm = mm_per_unit.map(|scale| solved_tier.mast * scale);
            let placement = if sizing_tier == Some(i) {
                DepthPlacement::SetStoneSize
            } else {
                DepthPlacement::of(
                    cut_meets.as_ref().and_then(|meets| meets.tiers.get(i)),
                    solved_tier.mast,
                    depth_mm,
                )
            };
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
