//! The tier-list row builders.
//!
//! [`tier_items`]/[`tier_items_stale`]/
//! [`tier_items_stale_with_last_solved`]/[`tier_items_from_solved`] -- and the smaller
//! per-tier helpers ([`build_tier_row`], the manufacturability-warning grouping, the
//! `MissingAnchor` block-to-tier mapping) they share.

use super::row_format::{
    constraint_kind_and_text, format_angle_cell, format_index_value, imported_meet_text,
    orbit_status_text, representative_crown_and_pavilion_angles_deg, tier_label,
    tier_margin_and_risk,
};

/// Unrounded `{}` text, for a form or inline cell to re-save; the display strings of
/// [`format_angle_cell`] and [`format_index_value`] would lose digits.
fn full_precision(value: f64) -> String {
    format!("{value}")
}
use super::{TierRow, TierRowKind};
use pleochroma::geometry::meet_solver::{Block, SolveStrategy, SolvedTier, classify_blocks};
use pleochroma_cut_core::{
    ConstraintTier, Design, DesignSolveError, ManufacturabilityWarning, MissingAnchor,
    design::ConcaveTier,
};
use std::collections::BTreeMap;

mod warnings;

use warnings::TierWarnings;
pub use warnings::{
    manufacturability_warnings_tagged, manufacturability_warnings_tagged_with,
    solved_manufacturability_warnings,
};

/// The per-tier solve results of one row, passed to [`build_tier_row`] as one value to keep its
/// argument count within clippy's limit.
struct TierSolveInfo {
    mast: String,
    /// The mast in millimetres, empty when nothing anchors a real scale -- see
    /// [`TierRow::mast_mm`].
    mast_mm: String,
    /// The same mast unrounded -- see [`TierRow::mast_full`].
    mast_full: String,
    strategy: String,
    strategy_is_uncertain: bool,
    strategy_detail: String,
    needs_anchor: bool,
}

/// The per-design inputs [`build_tier_row`] reads, computed once per row list.
struct RowContext<'a> {
    tier_blocks: &'a [Block],
    warnings: &'a BTreeMap<usize, String>,
    /// See [`tier_margin_and_risk`]'s own doc comment.
    pavilion_partner_deg: Option<f64>,
    tier_labels: &'a [pleochroma_cut_core::TierLabelInfo],
}

/// Builds one [`TierRow`] from `tier` and its resolved [`TierSolveInfo`].
///
/// `multi_selected` is always `false` here: the callers have no access to the editor's
/// selection. [`super::row_format::apply_multi_selection`] sets it afterwards.
fn build_tier_row(
    design: &Design,
    index: usize,
    tier: &ConstraintTier,
    n_d: f64,
    info: TierSolveInfo,
    row_context: &RowContext<'_>,
) -> TierRow {
    let tier_blocks = row_context.tier_blocks;
    let warnings = row_context.warnings;
    let pavilion_partner_deg = row_context.pavilion_partner_deg;
    let (constraint_kind, constraint_text) =
        constraint_kind_and_text(&tier.constraint, design.tier_target(index));
    let units = pleochroma_cut_core::orbit_units(&tier.indices, &design.meta);
    let (orbit_status, orbit_incomplete) = orbit_status_text(&units);
    let (margin_text, risk_level) = tier_margin_and_risk(
        tier.angle_deg,
        tier_blocks[index],
        n_d,
        pavilion_partner_deg,
    );
    let code = row_context
        .tier_labels
        .get(index)
        .map(|label| label.code.clone());
    let name = if tier.name.trim().is_empty() || pleochroma_cut_core::is_legacy_123_abc(&tier.name)
    {
        code.clone().unwrap_or_else(|| tier.name.clone())
    } else {
        tier.name.clone()
    };
    TierRow {
        index: i32::try_from(index).unwrap_or(i32::MAX),
        angle_deg: format_angle_cell(tier.angle_deg.abs()),
        angle_full: full_precision(tier.angle_deg.abs()),
        name,
        code: code.unwrap_or_default(),
        indices: tier
            .indices
            .iter()
            .copied()
            .map(format_index_value)
            .collect::<Vec<_>>()
            .join(", "),
        indices_full: tier
            .indices
            .iter()
            .copied()
            .map(full_precision)
            .collect::<Vec<_>>()
            .join(", "),
        constraint_kind,
        constraint_text,
        mast: info.mast,
        mast_mm: info.mast_mm,
        mast_full: info.mast_full,
        strategy: info.strategy,
        strategy_is_uncertain: info.strategy_is_uncertain,
        strategy_detail: info.strategy_detail,
        needs_anchor: info.needs_anchor,
        imported_meet_text: imported_meet_text(tier.imported_meet.as_ref()),
        orbit_status,
        orbit_incomplete,
        is_detached: !tier.detached.is_empty(),
        block: block_label(tier_blocks[index]).to_string(),
        margin_text,
        risk_level,
        meet_partners_text: meet_partners_text(design, index),
        warning_text: warnings.get(&index).cloned().unwrap_or_default(),
        multi_selected: false,
        // Set by `apply_proposed_angles` once the whole row list exists.
        proposed_angle: String::new(),
        kind: TierRowKind::Flat,
        tool_line: String::new(),
        relation_text: design.relation_text(index).unwrap_or_default(),
        frosting_text: design.tier_frosting_text(index),
    }
}

/// The tool line a concave row carries: the four
/// [`ConcaveTier::second_line_fields`] columns, joined with two spaces.
#[must_use]
pub fn concave_tool_line(tier: &ConcaveTier) -> String {
    tier.second_line_fields().join("  ")
}

/// One [`TierRow`] per concave tier, in storage order, after the flat rows.
///
/// Listed as authored, like the flat rows, not in cutting order. Each `code` comes from
/// [`Design::tier_codes`]. A concave tier has no mast, strategy, meet constraint or margin,
/// so those cells read as "not applicable" (`"-"`, an empty string, risk `-1`).
fn concave_tier_rows(design: &Design, warnings: &BTreeMap<usize, String>) -> Vec<TierRow> {
    let codes = design.tier_codes().concave;
    design
        .concave_tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let units = pleochroma_cut_core::orbit_units(&tier.indices, &design.meta);
            let (orbit_status, orbit_incomplete) = orbit_status_text(&units);
            TierRow {
                index: i32::try_from(index).unwrap_or(i32::MAX),
                angle_deg: format_angle_cell(tier.angle_deg.abs()),
                angle_full: full_precision(tier.angle_deg.abs()),
                name: tier.name.clone(),
                code: codes
                    .get(index)
                    .map(|label| label.code.clone())
                    .unwrap_or_default(),
                indices: tier
                    .indices
                    .iter()
                    .copied()
                    .map(format_index_value)
                    .collect::<Vec<_>>()
                    .join(", "),
                indices_full: tier
                    .indices
                    .iter()
                    .copied()
                    .map(full_precision)
                    .collect::<Vec<_>>()
                    .join(", "),
                mast: "-".to_string(),
                strategy: "Tool cut".to_string(),
                strategy_detail: tier.instructions.clone(),
                orbit_status,
                orbit_incomplete,
                block: if tier.is_crown_side() {
                    "Crown"
                } else {
                    "Pavilion"
                }
                .to_string(),
                risk_level: -1,
                warning_text: warnings.get(&index).cloned().unwrap_or_default(),
                kind: TierRowKind::Concave,
                tool_line: concave_tool_line(tier),
                ..TierRow::default()
            }
        })
        .collect()
}

/// Builds the tier list's rows without calling [`Design::solve`]: every mast and strategy
/// cell reads `"-"`/`"not solved"` (flagged uncertain).
///
/// Used after every edit except the explicit "Solve". A previous solve's masts would be wrong
/// once an edit changes the tier count or order, and a solve can take seconds.
#[must_use]
pub fn tier_items_stale(design: &Design, n_d: f64) -> Vec<TierRow> {
    let tier_blocks = classify_blocks(&design.meet_tier_inputs());
    // No solve here, so only the mast-free manufacturability checks run, tagged "(pre-solve)".
    let warnings = TierWarnings::of(design, None);
    let (_, pavilion_partner_deg) = representative_crown_and_pavilion_angles_deg(design);
    let tier_labels = pleochroma_cut_core::compute_tier_labels(&design.tiers);
    let row_context = RowContext {
        tier_blocks: &tier_blocks,
        warnings: &warnings.flat,
        pavilion_partner_deg,
        tier_labels: &tier_labels,
    };
    let mut rows: Vec<TierRow> = design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let info = TierSolveInfo {
                mast: "-".to_string(),
                mast_mm: String::new(),
                mast_full: String::new(),
                strategy: "not solved".to_string(),
                strategy_is_uncertain: true,
                // No solve here, so no detail or missing-anchor block; `tier_items` supplies them.
                strategy_detail: String::new(),
                needs_anchor: false,
            };
            build_tier_row(design, index, tier, n_d, info, &row_context)
        })
        .collect();
    rows.extend(concave_tier_rows(design, &warnings.concave));
    rows
}

/// [`tier_items_stale`] for a caller that still holds the last completed solve's masts.
///
/// Keeps the previous readings, so an edit such as a rename does not wipe the figures a cutter
/// compares against a gauge on a slow design. Rows whose index is in `dirty` (the tiers the
/// edit touched, or every tier for a structural edit that can move masts) get
/// [`tier_items_stale`]'s `"-"`/`"not solved"` treatment. Every other row keeps `last_solved`'s
/// mast and detail, with strategy prefixed `"stale"`.
///
/// Falls back to [`tier_items_stale`] when `last_solved` is `None` or its length differs from
/// `design.tiers.len()`: a list from another tier layout would show the wrong mast on a row.
#[must_use]
pub fn tier_items_stale_with_last_solved(
    design: &Design,
    n_d: f64,
    last_solved: Option<&[SolvedTier]>,
    dirty: &std::collections::BTreeSet<usize>,
) -> Vec<TierRow> {
    let last_solved = last_solved.filter(|solved| solved.len() == design.tiers.len());
    let Some(last_solved) = last_solved else {
        return tier_items_stale(design, n_d);
    };
    let tier_blocks = classify_blocks(&design.meet_tier_inputs());
    // The cached solve may be one edit old, so the mast-free checks are tagged "(pre-solve)".
    let warnings = TierWarnings::of(design, None);
    let mm_per_unit = design.yield_report(last_solved).mm_per_unit;
    let (_, pavilion_partner_deg) = representative_crown_and_pavilion_angles_deg(design);
    let tier_labels = pleochroma_cut_core::compute_tier_labels(&design.tiers);
    let row_context = RowContext {
        tier_blocks: &tier_blocks,
        warnings: &warnings.flat,
        pavilion_partner_deg,
        tier_labels: &tier_labels,
    };
    let mut rows: Vec<TierRow> = design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let info = if dirty.contains(&index) {
                TierSolveInfo {
                    mast: "-".to_string(),
                    mast_mm: String::new(),
                    mast_full: String::new(),
                    strategy: "not solved".to_string(),
                    strategy_is_uncertain: true,
                    strategy_detail: String::new(),
                    needs_anchor: false,
                }
            } else {
                let (label, _) = strategy_label(last_solved[index].strategy);
                TierSolveInfo {
                    mast: format!("{:.4}", last_solved[index].mast),
                    mast_mm: mm_per_unit.map_or_else(String::new, |mm| {
                        format!("{:.3} mm", last_solved[index].mast * mm)
                    }),
                    mast_full: format!("{}", last_solved[index].mast),
                    strategy: format!("stale ({label})"),
                    strategy_is_uncertain: true,
                    strategy_detail: last_solved[index].detail.clone(),
                    needs_anchor: false,
                }
            };
            build_tier_row(design, index, tier, n_d, info, &row_context)
        })
        .collect();
    rows.extend(concave_tier_rows(design, &warnings.concave));
    rows
}

/// Patches [`TierRow::proposed_angle`] onto the rows that `changes` targets.
///
/// Runs after the row list is built, as [`super::row_format::apply_multi_selection`] does.
/// Rows `changes` does not name keep the empty default. The value is formatted like
/// `angle_deg`, so the ghost reads like the real cell beside it.
pub fn apply_proposed_angles(tiers: &mut [TierRow], changes: &[pleochroma_cut_core::AngleChange]) {
    for change in changes {
        if let Some(row) = tiers.get_mut(change.index) {
            row.proposed_angle = format_angle_cell(change.to_deg.abs());
        }
    }
}

/// Maps each tier index that [`MissingAnchor::block_details`] names to its [`Block`].
///
/// Lets [`tier_items`] mark only the tiers that lack an anchor, not every tier in the design.
fn missing_anchor_tier_blocks(missing: &MissingAnchor, design: &Design) -> BTreeMap<usize, Block> {
    missing
        .block_details(design)
        .into_iter()
        .flat_map(|(block, indices)| indices.into_iter().map(move |index| (index, block)))
        .collect()
}

/// The meet partners that [`Design::facet_meets`] resolves tier `index`'s constraint against,
/// as text (e.g. `"meets tier 3 (C1)"`), or `""` when none resolves, as for a `ScaleReference`
/// tier. Uses the solver's own name resolver, so it shows the partners the solver used.
/// It needs no solve, so [`tier_items`] and [`tier_items_stale`] both fill it.
fn meet_partners_text(design: &Design, index: usize) -> String {
    let Ok(targets) = design.facet_meets(index) else {
        return String::new();
    };
    if targets.is_empty() {
        return String::new();
    }
    format!(
        "meets {}",
        targets
            .into_iter()
            .map(|target| tier_label(design, target))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// [`SolveStrategy`]'s label for the SOLVE column, and whether that label is flagged uncertain.
/// `LeastSquaresFallback` is an estimate and `Failed` is not to be trusted; the others are not
/// flagged.
const fn strategy_label(strategy: SolveStrategy) -> (&'static str, bool) {
    match strategy {
        SolveStrategy::ScaleReference => ("Scale reference", false),
        SolveStrategy::DependencyOrder => ("Dependency order", false),
        SolveStrategy::JointGroup => ("Joint group", false),
        SolveStrategy::LeastSquaresFallback => ("Least-squares est.", true),
        SolveStrategy::Failed => ("FAILED (untrusted)", true),
    }
}

/// Converts `design`'s tier list into the rows the editor's list renders, with each tier's
/// solved mast and [`SolveStrategy`] label.
///
/// `index` is the tier's position in `design.tiers`; the UI sends it back on save and remove.
///
/// Solves `design` once. Only a tier that the [`MissingAnchor`] error names (see
/// [`missing_anchor_tier_blocks`]) reads `"?"` as having no anchor. Other tiers, blocked by
/// another block's missing anchor, read `"-"`/`"blocked"`.
///
/// For a full solve (New, Load, an explicit Solve); after an edit use [`tier_items_stale`].
#[must_use]
pub fn tier_items(design: &Design, n_d: f64) -> Vec<TierRow> {
    tier_items_from_result(design, &design.solve(), n_d)
}

/// [`tier_items`] for a caller that already ran [`Design::solve`].
///
/// An `Err` result gives the same rows [`tier_items`] builds for a failed solve, without
/// solving a second time.
///
/// # Panics
///
/// As [`tier_items_from_solved`] for the `Ok` arm: `solved` must have one entry per
/// tier of `design`.
#[must_use]
pub fn tier_items_from_result(
    design: &Design,
    solved: &Result<Vec<SolvedTier>, DesignSolveError>,
    n_d: f64,
) -> Vec<TierRow> {
    match solved {
        Ok(rows) => tier_items_from_solved(design, rows, n_d),
        Err(err) => {
            // Only `MissingAnchor` names individual tiers. Any other failure blocks the whole
            // design, so every tier takes the generic "blocked" arm.
            let missing_tier_blocks = match err {
                DesignSolveError::MissingAnchor(missing) => {
                    missing_anchor_tier_blocks(missing, design)
                }
                DesignSolveError::Mismatch(_)
                | DesignSolveError::Solve(_)
                | DesignSolveError::Preservation(_)
                | DesignSolveError::Target(_) => BTreeMap::new(),
            };
            let blocked_detail = if matches!(err, DesignSolveError::MissingAnchor(_)) {
                "Another block in this design has no anchor -- see the validation banner above."
                    .to_string()
            } else {
                err.to_string()
            };
            let tier_blocks = classify_blocks(&design.meet_tier_inputs());
            // Tagged "(pre-solve)": there is no completed solve to report warnings from.
            let warnings = TierWarnings::of(design, None);
            let (_, pavilion_partner_deg) = representative_crown_and_pavilion_angles_deg(design);
            let tier_labels = pleochroma_cut_core::compute_tier_labels(&design.tiers);
            let row_context = RowContext {
                tier_blocks: &tier_blocks,
                warnings: &warnings.flat,
                pavilion_partner_deg,
                tier_labels: &tier_labels,
            };
            let mut rows: Vec<TierRow> = design
                .tiers
                .iter()
                .enumerate()
                .map(|(index, tier)| {
                    let info = missing_tier_blocks.get(&index).map_or_else(
                        || TierSolveInfo {
                            mast: "-".to_string(),
                            mast_mm: String::new(),
                            mast_full: String::new(),
                            strategy: "blocked".to_string(),
                            strategy_is_uncertain: true,
                            strategy_detail: blocked_detail.clone(),
                            needs_anchor: false,
                        },
                        |&block| TierSolveInfo {
                            mast: "?".to_string(),
                            mast_mm: String::new(),
                            mast_full: String::new(),
                            strategy: "no anchor yet".to_string(),
                            strategy_is_uncertain: true,
                            strategy_detail: MissingAnchor::block_sentence(block),
                            needs_anchor: true,
                        },
                    );
                    build_tier_row(design, index, tier, n_d, info, &row_context)
                })
                .collect();
            rows.extend(concave_tier_rows(design, &warnings.concave));
            rows
        }
    }
}

/// [`tier_items`] for a caller that already has an up-to-date `solved` mast list.
///
/// Builds each row from `solved` instead of solving again, so the solid-preview replan and
/// the background auto-solve feed the same masts to the tier table without a second solve.
///
/// # Panics
///
/// `solved` must have one entry per tier `design` currently has, in the same
/// order -- the same alignment contract [`Design::to_asc_schedule_from_solved`]
/// documents; indexing out of that range panics.
#[must_use]
pub fn tier_items_from_solved(design: &Design, solved: &[SolvedTier], n_d: f64) -> Vec<TierRow> {
    tier_items_from_solved_and(design, solved, n_d, &TierWarnings::of(design, Some(solved)))
}

/// [`tier_items_from_solved`] for a caller on the UI thread, which must not run the pass itself.
///
/// The manufacturability pass builds meshes, slow for a design with concave tools, so `warnings`
/// comes from the worker's pass for this same `design` and `solved`
/// ([`solved_manufacturability_warnings`]). `None` (no worker pass) gives the full inline pass
/// for a design without concave tools; with concave tools it shows only the mast-free findings,
/// tagged `(pre-solve)`.
///
/// # Panics
///
/// As [`tier_items_from_solved`]: `solved` must have one entry per tier of `design`.
#[must_use]
pub fn tier_items_from_solved_with_warnings(
    design: &Design,
    solved: &[SolvedTier],
    n_d: f64,
    warnings: Option<&[ManufacturabilityWarning]>,
) -> Vec<TierRow> {
    tier_items_from_solved_and(
        design,
        solved,
        n_d,
        &TierWarnings::from_precomputed(design, solved, warnings),
    )
}

/// The inline pass's input when no worker pass exists: the masts for a design without concave
/// tools, where the pass is cheap; `None` (mast-free checks only, no mesh) for one with tools,
/// whose check 6 builds a solid.
fn without_tool_checks<'a>(design: &Design, solved: &'a [SolvedTier]) -> Option<&'a [SolvedTier]> {
    design.concave_tiers.is_empty().then_some(solved)
}

/// The rows of [`tier_items_from_solved`] with `warnings` already split by row.
fn tier_items_from_solved_and(
    design: &Design,
    solved: &[SolvedTier],
    n_d: f64,
    warnings: &TierWarnings,
) -> Vec<TierRow> {
    let tier_blocks = classify_blocks(&design.meet_tier_inputs());
    // `None` without a girdle diameter, the only anchor to a real size. Measured once per
    // list, not per row, since `yield_report` measures the whole solid.
    let mm_per_unit = design.yield_report(solved).mm_per_unit;
    let (_, pavilion_partner_deg) = representative_crown_and_pavilion_angles_deg(design);
    let tier_labels = pleochroma_cut_core::compute_tier_labels(&design.tiers);
    let row_context = RowContext {
        tier_blocks: &tier_blocks,
        warnings: &warnings.flat,
        pavilion_partner_deg,
        tier_labels: &tier_labels,
    };
    let mut rows: Vec<TierRow> = design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let (label, uncertain) = strategy_label(solved[index].strategy);
            let info = TierSolveInfo {
                mast: format!("{:.4}", solved[index].mast),
                mast_mm: mm_per_unit.map_or_else(String::new, |mm| {
                    format!("{:.3} mm", solved[index].mast * mm)
                }),
                mast_full: format!("{}", solved[index].mast),
                strategy: label.to_string(),
                strategy_is_uncertain: uncertain,
                strategy_detail: solved[index].detail.clone(),
                needs_anchor: false,
            };
            build_tier_row(design, index, tier, n_d, info, &row_context)
        })
        .collect();
    rows.extend(concave_tier_rows(design, &warnings.concave));
    rows
}

/// [`TierRow::block`]'s one-word label for a [`Block`]. The side comes from the solver's block,
/// so a zero angle's side is never guessed from its formatted text.
const fn block_label(block: Block) -> &'static str {
    match block {
        Block::Crown => "Crown",
        Block::Pavilion => "Pavilion",
        Block::Girdle => "Girdle",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditorSession;
    use pleochroma::geometry::meet_solver::MeetConstraint;
    use pleochroma_cut_core::{ConstraintTier, Edit};

    /// A one-tier design; `MeetExisting` leaves its block without an anchor (the solve
    /// fails with `MissingAnchor`), `ScaleReference` makes it solve.
    fn design_with(constraint: MeetConstraint) -> Design {
        let mut session = EditorSession::fresh();
        session
            .apply(Edit::AddTier {
                index: 0,
                tier: ConstraintTier {
                    angle_deg: -41.0,
                    name: "P1".to_string(),
                    indices: vec![0.0, 24.0],
                    constraint,
                    imported_meet: None,
                    original_notes: None,
                    detached: Vec::new(),
                },
            })
            .unwrap();
        session.design
    }

    /// The `Err` rows from a held failed solve equal `tier_items`' rows; the `Ok` rows equal
    /// `tier_items_from_solved`.
    #[test]
    fn tier_items_from_result_matches_tier_items_for_failed_and_solved_designs() {
        let failing = design_with(MeetConstraint::MeetExisting);
        let n_d = failing.effective_refractive_index();
        let failed = failing.solve();
        assert!(failed.is_err(), "an unanchored design must fail to solve");
        let rows = tier_items_from_result(&failing, &failed, n_d);
        assert_eq!(rows, tier_items(&failing, n_d));
        assert_eq!(rows[0].strategy.as_str(), "no anchor yet");

        let solving = design_with(MeetConstraint::ScaleReference(0.65));
        let solved = solving.solve();
        let solved_rows = tier_items_from_result(&solving, &solved, n_d);
        assert_eq!(
            solved_rows,
            tier_items_from_solved(&solving, solved.as_ref().unwrap(), n_d)
        );
    }
}
