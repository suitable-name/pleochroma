//! View-model text for the Preform and Yield tabs, read once a design solves.
//!
//! Covers yield and weight, proportions (table %, crown/pavilion depth, girdle thickness,
//! length-to-width), the proportion-guidance verdict chips, and the Edit tab's cut-order
//! schedule rows.

use super::{
    CuttingRow,
    girdle_reading::GirdleReading,
    row_format::{format_angle_cell, representative_crown_and_pavilion_angles_deg},
};
use crate::material_lookup::EditorMaterialLookup;
use pleochroma::geometry::meet_solver::{Block, SolvedTier, classify_blocks};
use pleochroma_cut_core::{Design, DesignSolveError, design::TierRef, format_sheet_indices};

/// `design`'s current yield and weight figures, formatted for `EditorView`'s read-only
/// display fields.
///
/// Called after the explicit "Solve" action, a debounced background solve and a completed
/// solid-preview replan (`view::panel::refresh_editor_panel_from_solve`,
/// `auto_solve::apply::push_solve_dependent_background_fields`,
/// `view::viewport::push_solved_preview`). An edit with no solve clears these fields to `""`
/// instead, via `view::panel_stale::push_stale_preform_and_yield`, because that path has no
/// solved mast list for `Design::yield_report`.
///
/// Returns `(volumetric_yield_text, carat_weight_text, specific_gravity_used_text,
/// preform_fit_warning_text)`. All four are empty when the design does not solve
/// ([`pleochroma_cut_core::MissingAnchor`], already shown by the banner).
///
/// `custom_sg` maps custom catalogue material names to their specific gravity; see
/// [`yield_report_texts_from_solved`].
#[must_use]
pub fn yield_report_texts(
    design: &Design,
    custom_sg: &[(String, f64)],
) -> (String, String, String, String) {
    yield_report_texts_from_result(design, &design.solve(), custom_sg)
}

/// [`yield_report_texts`]'s counterpart for a caller that already ran
/// [`Design::solve`] and holds its `Result` -- a failed solve included.
///
/// `Err` is the four empty strings (the design does not currently solve, already
/// surfaced by the banner); `Ok` is [`yield_report_texts_from_solved`]. Never solves.
#[must_use]
pub fn yield_report_texts_from_result(
    design: &Design,
    solved: &Result<Vec<SolvedTier>, DesignSolveError>,
    custom_sg: &[(String, f64)],
) -> (String, String, String, String) {
    solved.as_ref().map_or_else(
        |_| (String::new(), String::new(), String::new(), String::new()),
        |solved| yield_report_texts_from_solved(design, solved, custom_sg),
    )
}

/// [`yield_report_texts`] for a caller that already holds an up-to-date `solved` mast list.
///
/// Never re-solves. Unlike [`yield_report_texts`], it never returns the all-empty tuple,
/// since a caller holding a `solved` slice already knows the design solves.
///
/// Resolves `design.material`'s specific gravity through [`EditorMaterialLookup`], so a
/// custom catalogue material's recorded SG reaches the carat-weight estimate, not only a
/// built-in preset or a per-design override. `custom_sg` is passed in because this module has
/// no `RenderContext`; the caller clones it before any background thread, so no copy can drift.
pub fn yield_report_texts_from_solved(
    design: &Design,
    solved: &[SolvedTier],
    custom_sg: &[(String, f64)],
) -> (String, String, String, String) {
    let catalogue = EditorMaterialLookup::new(&[]).with_specific_gravity(custom_sg);
    let report = design.yield_report_with(solved, &catalogue);

    let volumetric_yield_text = report
        .volumetric_yield
        .map(|y| format!("{:.2}%", y * 100.0))
        .unwrap_or_default();
    let carat_weight_text = report
        .carat_weight
        .map(|c| format!("{c:.4} ct (est.)"))
        .unwrap_or_default();
    // "(override)" whenever the figure came from the user's typed number rather than
    // the selected preset's table figure.
    let specific_gravity_used_text = report.specific_gravity_used.map_or_else(String::new, |sg| {
        if design.material.specific_gravity_override.is_some() {
            format!("{sg:.3} (override)")
        } else {
            format!("{sg:.3}")
        }
    });
    let preform_fit_warning_text = report
        .preform_fit
        .map(|fit| fit.to_string())
        .unwrap_or_default();

    (
        volumetric_yield_text,
        carat_weight_text,
        specific_gravity_used_text,
        preform_fit_warning_text,
    )
}

/// `design`'s proportion readouts for the Preform tab's "Proportions" section: table %,
/// crown height, pavilion depth, total depth and length-to-width.
///
/// Lengths are converted to millimetres with [`Design::yield_report`]'s scale factor when one
/// exists (a girdle diameter is set and the design measures). Otherwise they stay in mast units
/// with no suffix, rather than guessing a scale.
///
/// Returns `(table_percent_text, crown_height_text, pavilion_depth_text, total_depth_text,
/// length_to_width_text)`. Each is `"-"` when the design has no tiers, does not solve, is not a
/// closed solid, or (for the two depths and `total_depth`) has no vertical girdle plane with a
/// live facet to measure from. See [`pleochroma::geometry::stone_metrics::StoneProportions`] for
/// why the depths are `Option`. Pushed by the Solve action, a debounced background solve and a
/// completed preview replan, not on every edit (see [`yield_report_texts`]).
///
/// The tierless guard runs before the solve. A tierless design still solves (an empty mast list
/// is a valid, closed, zero-plane solve), and `stone_proportions` would then measure the bare
/// preform block as if it were the stone. `total_depth` is a plain `f64`, so this guard is what
/// keeps it from showing a preform value. [`girdle_and_ratio_texts`] has the same guard.
#[must_use]
pub fn proportions_texts(design: &Design) -> (String, String, String, String, String) {
    let dash = || "-".to_string();
    if design.tiers.is_empty() {
        return (dash(), dash(), dash(), dash(), dash());
    }
    let Ok(solved) = design.solve() else {
        return (dash(), dash(), dash(), dash(), dash());
    };
    proportions_texts_from_solved(design, &solved)
}

/// The readouts [`proportions_texts`] does not return: girdle thickness (no other text home)
/// and the printed `C/W%`, `P/W%` and `G/W%` ratios, as opposed to the absolute depths.
///
/// The girdle figures are read at the girdle LINE ([`GirdleReading`]: the corners where girdle
/// faces meet and the ends of level edges), not across the band, which includes a brilliant's
/// V-scallops. A level line gives one figure; an unlevel one gives a range such as
/// `0.254–0.399 mm (not level)`. A design without a girdle line reads the band.
///
/// Mirrors [`proportions_texts`]: solve, measure, `"-"` fallback, and millimetre or model-unit
/// handling. Only the girdle thickness takes the unit suffix, since the `_to_width_percent`
/// values are scale-invariant. Both families use the same conversion, so they agree on units.
///
/// Returns `(girdle_thickness_text, crown_to_width_percent_text, pavilion_to_width_percent_text,
/// girdle_to_width_percent_text)`, each `"-"` under the conditions [`proportions_texts`] lists,
/// and also when there is no vertical girdle plane with a live facet to measure the girdle from.
#[must_use]
pub fn girdle_and_ratio_texts(design: &Design) -> (String, String, String, String) {
    let dash = || "-".to_string();
    // A tierless design still solves, and `stone_proportions` would then measure the bare
    // preform block (girdle 50% of a cube, crown and pavilion 0%). Those numbers must never
    // reach a readout. The same guard is in `proportions_texts` and the `_from_solved` mirrors.
    if design.tiers.is_empty() {
        return (dash(), dash(), dash(), dash());
    }
    let Ok(solved) = design.solve() else {
        return (dash(), dash(), dash(), dash());
    };
    girdle_and_ratio_texts_from_solved(design, &solved)
}

/// One proportion metric's verdict against [`pleochroma_cut_core::proportions_windows`]'s
/// reference table.
///
/// `level` is `0` (`Verdict::Within`), `1` (`Verdict::Near`), `2` (`Verdict::Outside`), or
/// `-1` when there is nothing to judge: the design does not solve or close, or this metric has
/// no value (e.g. no crown tier). `reason` is the matched window's one-line explanation, empty
/// at level `-1`.
pub struct ProportionVerdict {
    /// Verdict level; `-1` means nothing to judge yet.
    pub level: i32,
    /// One-line explanation of the matched window.
    pub reason: String,
}

/// The five [`ProportionVerdict`]s of the Preform tab's proportion guidance, one per metric in
/// [`pleochroma_cut_core::proportions_windows::Metric`].
pub struct ProportionVerdicts {
    /// Verdict for the table width percentage.
    pub table_pct: ProportionVerdict,
    /// Verdict for the crown angle.
    pub crown_angle: ProportionVerdict,
    /// Verdict for the pavilion angle.
    pub pavilion_angle: ProportionVerdict,
    /// Verdict for the total depth percentage.
    pub total_depth_pct: ProportionVerdict,
    /// Verdict for the girdle percentage.
    pub girdle_pct: ProportionVerdict,
}

/// The "nothing to judge yet" verdict, level `-1` (see [`ProportionVerdict`]).
const fn verdict_none() -> ProportionVerdict {
    ProportionVerdict {
        level: -1,
        reason: String::new(),
    }
}

/// Looks `value` up in the reference window for `shape`, `material` and `metric`
/// (via [`pleochroma_cut_core::proportions_windows::verdict_for`]). Gives [`verdict_none`]
/// when `value` is `None` or no window matches, which does not happen for a real metric.
fn verdict_from(
    value: Option<f64>,
    shape: pleochroma_cut_core::ShapeClass,
    material: pleochroma_cut_core::MaterialClass,
    metric: pleochroma_cut_core::ProportionMetric,
) -> ProportionVerdict {
    let Some(v) = value else {
        return verdict_none();
    };
    let Some((verdict, window)) =
        pleochroma_cut_core::proportions_windows::verdict_for(shape, material, metric, v)
    else {
        return verdict_none();
    };
    let level = match verdict {
        pleochroma_cut_core::Verdict::Within => 0,
        pleochroma_cut_core::Verdict::Near => 1,
        pleochroma_cut_core::Verdict::Outside => 2,
    };
    ProportionVerdict {
        level,
        reason: window.reason.to_string(),
    }
}

/// The [`pleochroma_cut_core::ShapeClass`] of `meta`'s schedule, for the proportion verdicts.
/// [`pleochroma_cut_core::ShapeClass::Round`] needs a symmetry order of 6 or more with mirroring
/// (the round brilliant family, the only shape with verified templates, see
/// [`pleochroma_cut_core::templates`]). Every other schedule is
/// [`pleochroma_cut_core::ShapeClass::Other`], which has no cushion, oval, step or trillion
/// classifier and uses the generic lapidary window.
const fn shape_class_for(
    meta: &pleochroma_cut_core::ScheduleMeta,
) -> pleochroma_cut_core::ShapeClass {
    if meta.symmetry_order >= 6 && meta.mirror {
        pleochroma_cut_core::ShapeClass::Round
    } else {
        pleochroma_cut_core::ShapeClass::Other
    }
}

/// The Preform tab's five proportion-verdict chips, judged from `proportions`.
///
/// `proportions` is the same [`pleochroma::geometry::stone_metrics::StoneProportions`] the
/// number readouts show, so a chip and its number cannot disagree. `n_d` is the design's
/// effective refractive index.
///
/// Total depth is judged as the sum of the crown, pavilion and girdle percentages. That is the
/// honest percentage-of-width equivalent: `StoneProportions` has only the absolute
/// `total_depth`, and total depth is crown plus pavilion plus girdle by construction. The sum
/// uses the whole girdle band.
///
/// The girdle chip judges the girdle at its LINE ([`GirdleReading::judged_percent`], the
/// thinnest line thickness) where `solved` gives one, and the band otherwise. A correctly cut
/// brilliant's band includes the V-scallops between the girdle corners and never fits a
/// girdle window.
#[must_use]
pub fn proportion_verdicts(
    design: &Design,
    solved: &[SolvedTier],
    proportions: &pleochroma::geometry::stone_metrics::StoneProportions,
    n_d: f64,
) -> ProportionVerdicts {
    let girdle = GirdleReading::of(design, solved, proportions);
    proportion_verdicts_with_girdle(design, proportions, girdle, n_d)
}

/// [`proportion_verdicts`] with the girdle reading already worked out (`None`: no girdle to
/// judge), for a caller that has it, so the girdle is not measured twice.
#[must_use]
pub fn proportion_verdicts_with_girdle(
    design: &Design,
    proportions: &pleochroma::geometry::stone_metrics::StoneProportions,
    girdle: Option<GirdleReading>,
    n_d: f64,
) -> ProportionVerdicts {
    let shape = shape_class_for(&design.meta);
    let material = pleochroma_cut_core::MaterialClass::from_ri(n_d);
    let (crown_angle_deg, pavilion_angle_deg) =
        representative_crown_and_pavilion_angles_deg(design);
    let total_depth_pct = match (
        proportions.crown_to_width_percent,
        proportions.pavilion_to_width_percent,
        proportions.girdle_to_width_percent,
    ) {
        (Some(c), Some(p), Some(g)) => Some(c + p + g),
        _ => None,
    };
    ProportionVerdicts {
        table_pct: verdict_from(
            proportions.table_percent,
            shape,
            material,
            pleochroma_cut_core::ProportionMetric::TablePercent,
        ),
        crown_angle: verdict_from(
            crown_angle_deg,
            shape,
            material,
            pleochroma_cut_core::ProportionMetric::CrownAngle,
        ),
        pavilion_angle: verdict_from(
            pavilion_angle_deg,
            shape,
            material,
            pleochroma_cut_core::ProportionMetric::PavilionAngle,
        ),
        total_depth_pct: verdict_from(
            total_depth_pct,
            shape,
            material,
            pleochroma_cut_core::ProportionMetric::TotalDepthPercent,
        ),
        girdle_pct: verdict_from(
            girdle.map(GirdleReading::judged_percent),
            shape,
            material,
            pleochroma_cut_core::ProportionMetric::GirdleThicknessPercent,
        ),
    }
}

/// Millimetre text for the Preform tab's Half-Width and Depth fields, using the same
/// [`Design::yield_report`] scale factor as [`proportions_texts`].
///
/// Those fields are stored in mast units (girdle half-width = 1), and `PreformSpec` has no mm
/// concept. The mm text is shown ALONGSIDE the editable model-unit value, never in place of it.
/// Each is `""` when the design does not solve or no girdle diameter anchors `mm_per_unit`.
#[must_use]
pub fn preform_mm_texts(design: &Design) -> (String, String) {
    let Ok(solved) = design.solve() else {
        return (String::new(), String::new());
    };
    let Some(mm_per_unit) = design.yield_report(&solved).mm_per_unit else {
        return (String::new(), String::new());
    };
    let preform = &design.preform;
    (
        format!("\u{2248} {:.3} mm", preform.half_width * mm_per_unit),
        format!("\u{2248} {:.3} mm", preform.depth * mm_per_unit),
    )
}

/// The seed text for `EditorModel.preform_y_offset_mm`: `design.preform_y_offset` (mast units)
/// in millimetres, via the same `mm_per_unit` anchor as [`preform_mm_texts`].
///
/// Pure, with no internal solve. The caller passes `mm_per_unit` from a solved mast list, or
/// `None` on the stale path, which gives `""`. The text is a bare decimal with no unit or
/// approximation glyph, because the field is editable and round-trips through
/// `Edit::SetPreformYOffset`; a suffix would not parse back.
#[must_use]
pub fn preform_y_offset_mm_text(preform_y_offset: f64, mm_per_unit: Option<f64>) -> String {
    mm_per_unit.map_or_else(String::new, |mm_per_unit| {
        format!("{:.2}", preform_y_offset * mm_per_unit)
    })
}

/// The label for the design under edit: the paired `.asc`'s bare file name when it was loaded
/// from or saved to one, else `"Untitled design"`.
///
/// Shown in the editor status strip.
pub fn design_label_text(asc_filename: Option<&str>) -> String {
    asc_filename.map_or_else(|| "Untitled design".to_string(), str::to_string)
}

/// `design`'s cut order as the Edit tab's schedule rows: angle, facet name, index positions and
/// notes, in cut order ([`Design::cutting_order`], planar and concave alike).
///
/// Built with [`Design::try_to_asc_schedule_from_solved`], the conversion behind "Export Edited
/// .asc", so the rows match the export. The fallible form is used so that a `solved` which does
/// not match `design` logs and gives an empty schedule instead of panicking the editor.
pub fn cutting_instructions_rows(design: &Design, solved: &[SolvedTier]) -> Vec<CuttingRow> {
    // `CuttingRow::side` comes from the solver's block classification, not the sign of the
    // angle, because only the classification tells a girdle facet (neither side) from a crown one.
    let tier_blocks = classify_blocks(&design.meet_tier_inputs());
    // The label is the tier code (`P1`, `G1`, `C1`, `T`), numbered in cutting order as the
    // printed sheet writes it. The index list uses the sheet's dashed, zero-padded form.
    let tier_codes = design.tier_codes();
    let schedule = match design.try_to_asc_schedule_from_solved(solved) {
        Ok(schedule) => schedule,
        Err(mismatch) => {
            tracing::warn!(%mismatch, "cutting_instructions_rows: solved masts do not match this design's tier count");
            return Vec::new();
        }
    };
    // `tier_index` is the stored position (it indexes `tier_blocks`); `order_idx` is the
    // position in cutting order, shown in the row and in an unnamed tier's placeholder label.
    // They differ whenever the stored order is not the cutting order.
    let flat_row =
        |tier_index: usize, order_idx: usize, tier: pleochroma_formats::asc::AscTier| CuttingRow {
            order_idx: i32::try_from(order_idx).unwrap_or(i32::MAX),
            side: match tier_blocks.get(tier_index) {
                Some(Block::Crown) => 1,
                Some(Block::Pavilion) => -1,
                _ => 0,
            },
            facet: match tier_codes.flat.get(tier_index) {
                Some(label) if !label.code.is_empty() => label.code.clone(),
                _ if tier.name.is_empty() => format!("#{}", order_idx + 1),
                _ => tier.name,
            },
            angle: format_angle_cell(tier.angle_deg.abs()),
            index_val: format_sheet_indices(&tier.indices),
            notes: tier.notes,
            second_line: None,
        };
    // Rows follow `cutting_order()`, the order the printed sheet and cutting mode use: pavilion
    // and girdle tiers, each concave tool at the end of its section, then the crown tiers, then
    // the table. A flat row's `side` uses its stored index (its position in `design.tiers`),
    // which is kept apart from `order_idx`.
    let mut flat_tiers: Vec<Option<pleochroma_formats::asc::AscTier>> =
        schedule.tiers.into_iter().map(Some).collect();
    design
        .cutting_order()
        .into_iter()
        .enumerate()
        .filter_map(|(order_idx, tier_ref)| match tier_ref {
            TierRef::Flat(i) => {
                let tier = flat_tiers.get_mut(i)?.take()?;
                Some(flat_row(i, order_idx, tier))
            }
            TierRef::Concave(i) => {
                let tier = design.concave_tiers.get(i)?;
                Some(CuttingRow {
                    order_idx: i32::try_from(order_idx).unwrap_or(i32::MAX),
                    side: if tier.is_crown_side() { 1 } else { -1 },
                    facet: match tier_codes.concave.get(i) {
                        Some(label) if !label.code.is_empty() => label.code.clone(),
                        _ if tier.name.is_empty() => format!("#{}", order_idx + 1),
                        _ => tier.name.clone(),
                    },
                    angle: format_angle_cell(tier.angle_deg.abs()),
                    index_val: format_sheet_indices(&tier.indices),
                    notes: tier.instructions.clone(),
                    second_line: Some(tier.second_line_fields()),
                })
            }
        })
        .collect()
}

/// [`proportions_texts`] for a caller that already holds an up-to-date `solved` mast list,
/// so the design is not solved again.
///
/// Keeps the `tiers.is_empty()` guard: a tierless design solves to an empty mast list, and
/// without the guard `stone_proportions` would measure the bare preform block as the stone.
#[must_use]
pub fn proportions_texts_from_solved(
    design: &Design,
    solved: &[SolvedTier],
) -> (String, String, String, String, String) {
    let dash = || "-".to_string();
    if design.tiers.is_empty() {
        return (dash(), dash(), dash(), dash(), dash());
    }
    let Some(proportions) = design.stone_proportions(solved) else {
        return (dash(), dash(), dash(), dash(), dash());
    };
    let mm_per_unit = design.yield_report(solved).mm_per_unit;
    let proportions = mm_per_unit.map_or(proportions, |mm| proportions.to_mm(mm));
    let unit = if mm_per_unit.is_some() { " mm" } else { "" };
    let table_percent_text = proportions
        .table_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    let crown_height_text = proportions
        .crown_height
        .map_or_else(dash, |v| format!("{v:.3}{unit}"));
    let pavilion_depth_text = proportions
        .pavilion_depth
        .map_or_else(dash, |v| format!("{v:.3}{unit}"));
    // `total_depth` is a plain `f64`, so it is gated on `girdle_thickness` as in
    // `proportions_texts`, with a "-" fallback.
    let total_depth_text = if proportions.girdle_thickness.is_some() {
        format!("{:.3}{unit}", proportions.total_depth)
    } else {
        dash()
    };
    let length_to_width_text = proportions
        .length_to_width
        .map_or_else(dash, |v| format!("{v:.3}"));
    (
        table_percent_text,
        crown_height_text,
        pavilion_depth_text,
        total_depth_text,
        length_to_width_text,
    )
}

/// [`proportions_texts`] for a caller that holds the `Result` of [`Design::solve`], including a
/// failed one.
///
/// The tierless guard runs first, then `Err` gives five dashes and `Ok` goes to
/// [`proportions_texts_from_solved`]. Never solves.
#[must_use]
pub fn proportions_texts_from_result(
    design: &Design,
    solved: &Result<Vec<SolvedTier>, DesignSolveError>,
) -> (String, String, String, String, String) {
    let dash = || "-".to_string();
    if design.tiers.is_empty() {
        return (dash(), dash(), dash(), dash(), dash());
    }
    solved.as_ref().map_or_else(
        |_| (dash(), dash(), dash(), dash(), dash()),
        |solved| proportions_texts_from_solved(design, solved),
    )
}

/// [`girdle_and_ratio_texts`] for a caller that already holds an up-to-date `solved` mast list.
///
/// Same body and `tiers.is_empty()` guard, minus the internal solve. The guard is needed here
/// too, since an empty mast list is a valid solve and would otherwise measure the bare preform.
/// Preferred on the UI's hot path once a design has solved.
#[must_use]
pub fn girdle_and_ratio_texts_from_solved(
    design: &Design,
    solved: &[SolvedTier],
) -> (String, String, String, String) {
    let dash = || "-".to_string();
    if design.tiers.is_empty() {
        return (dash(), dash(), dash(), dash());
    }
    let Some(proportions) = design.stone_proportions(solved) else {
        return (dash(), dash(), dash(), dash());
    };
    // Read in the design's own units; only the length is scaled to millimetres (the
    // percentages are the same in either).
    let girdle = GirdleReading::of(design, solved, &proportions);
    let mm_per_unit = design.yield_report(solved).mm_per_unit;
    let girdle_thickness_text =
        girdle.map_or_else(dash, |reading| reading.thickness_text(mm_per_unit));
    let crown_to_width_percent_text = proportions
        .crown_to_width_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    let pavilion_to_width_percent_text = proportions
        .pavilion_to_width_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    let girdle_to_width_percent_text = girdle.map_or_else(dash, GirdleReading::percent_text);
    (
        girdle_thickness_text,
        crown_to_width_percent_text,
        pavilion_to_width_percent_text,
        girdle_to_width_percent_text,
    )
}

/// [`preform_mm_texts`] for a caller that already holds an up-to-date `solved` mast list.
#[must_use]
pub fn preform_mm_texts_from_solved(design: &Design, solved: &[SolvedTier]) -> (String, String) {
    let Some(mm_per_unit) = design.yield_report(solved).mm_per_unit else {
        return (String::new(), String::new());
    };
    let preform = &design.preform;
    (
        format!("\u{2248} {:.3} mm", preform.half_width * mm_per_unit),
        format!("\u{2248} {:.3} mm", preform.depth * mm_per_unit),
    )
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

    #[test]
    fn from_result_helpers_blank_a_failed_solve_and_defer_to_from_solved_otherwise() {
        let failing = design_with(MeetConstraint::MeetExisting);
        let failed = failing.solve();
        assert!(failed.is_err(), "an unanchored design must fail to solve");
        assert_eq!(
            yield_report_texts_from_result(&failing, &failed, &[]),
            (String::new(), String::new(), String::new(), String::new())
        );
        let dash = || "-".to_string();
        assert_eq!(
            proportions_texts_from_result(&failing, &failed),
            (dash(), dash(), dash(), dash(), dash())
        );

        let solving = design_with(MeetConstraint::ScaleReference(0.65));
        let solved = solving.solve();
        let ok = solved.as_ref().unwrap();
        assert_eq!(
            yield_report_texts_from_result(&solving, &solved, &[]),
            yield_report_texts_from_solved(&solving, ok, &[])
        );
        assert_eq!(
            proportions_texts_from_result(&solving, &solved),
            proportions_texts_from_solved(&solving, ok)
        );
        assert_eq!(
            proportions_texts_from_result(&solving, &solved),
            proportions_texts(&solving)
        );
    }
}
