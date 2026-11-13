//! The editor panel refresh after a solve.
//!
//! [`refresh_editor_panel`] and [`refresh_editor_panel_from_solve`] derive every panel field
//! from one solve result. [`super::panel_stale`] is the counterpart that does not solve.

use super::{
    design_label::design_label,
    inspector::refresh_design_settings,
    panel_stale::refresh_deep_solve_availability,
    state::{
        EditorState, ScratchDelta, apply_multi_selection, apply_proposed_angles,
        cutting_instructions_rows, manufacturability_warnings_tagged, preform_y_offset_mm_text,
        proportion_verdicts, push_multi_selected_count, push_rows, push_tiers,
        tier_items_from_result,
    },
};
// The `_from_solved` text helpers are re-exported here and through `view`.
use crate::{
    AngleItem, EditorModel, EditorTierItem, MainWindow, UndoRedoLabels,
    bridge::render_thread::RenderContext,
};
use pleochroma::geometry::meet_solver::SolvedTier;
use pleochroma_cut_core::{Design, DesignSolveError, PreformShape};
pub(in crate::gui::editor) use pleochroma_editor::view_model::yield_report::{
    girdle_and_ratio_texts_from_solved, preform_mm_texts_from_solved, proportions_texts_from_solved,
};
use pleochroma_editor::{
    loading::preform_source_note,
    view_model::{
        solid_status::status_text_and_is_problem_from_result,
        yield_report::{proportions_texts_from_result, yield_report_texts_from_result},
    },
};
use slint::{ComponentHandle, SharedString};
use std::sync::{Arc, Mutex, PoisonError, atomic::Ordering as AtomicOrdering};

/// Refreshes the editor's panels from `state`, solving `state.design` once. The shared viewport
/// is not touched; see [`super::viewport::refresh_all`].
///
/// Every other panel field is derived from that one solve result, so a refresh costs one solve.
/// The startup paint is the only caller of this function. A user action with a design goes
/// through [`refresh_editor_panel_from_solve`] with a solve the caller has gated itself, and an
/// edit that must not solve goes through [`super::panel_stale::refresh_editor_panel_stale`].
///
/// Returns the solve result, so [`super::viewport::refresh_all`] can reuse it for the viewport.
pub(in crate::gui::editor) fn refresh_editor_panel(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
) -> Option<Vec<SolvedTier>> {
    let solved_result = state.design.solve();
    refresh_editor_panel_from_solve(ui, render_ctx, state, solved_result)
}

/// [`refresh_editor_panel`] with the solve already done. A caller that has solved `state.design`
/// itself passes the result in, so it is not solved twice. [`super::viewport::refresh_all`]
/// is the caller: it times the solve and reuses the result here.
pub(in crate::gui::editor) fn refresh_editor_panel_from_solve(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    solved_result: Result<Vec<SolvedTier>, DesignSolveError>,
) -> Option<Vec<SolvedTier>> {
    // Computed once per refresh. Each group below is gated on its own `ScratchDelta` flag.
    let delta = state.record_scratch_push();
    let n_d = refresh_design_settings(ui, render_ctx, state, &delta);
    // Derived from the same solve everything else on this refresh uses, never a
    // second one. Zero when the design does not currently solve, which reads as
    // "nothing to count yet" rather than as a stale figure.
    let facet_count = solved_result.as_ref().ok().map_or(0, |solved| {
        i32::try_from(super::solve_results::facet_count_from_solved(
            &state.design,
            solved,
        ))
        .unwrap_or(i32::MAX)
    });
    ui.global::<EditorModel>().set_facet_count(facet_count);
    let solved = solved_result.as_ref().ok();

    // The rows come from the held result, failed or not: a failed solve is not repeated.
    let tiers = tier_items_from_result(&state.design, &solved_result, n_d);
    push_tier_list_and_undo_redo(ui, state, tiers);
    // The inspector form follows the design the list now shows: an adopt, retarget
    // apply, optimize apply or deep-solve pin changes the selected tier's values.
    super::panel_stale::sync_tier_form_with_design(ui, state);

    // Read from the held result: `Design::status` would solve the design again.
    let (status_text, is_problem) =
        status_text_and_is_problem_from_result(&state.design, &solved_result);
    ui.global::<EditorModel>()
        .set_status_text(status_text.into());
    ui.global::<EditorModel>().set_status_is_problem(is_problem);
    ui.global::<EditorModel>()
        .set_solve_state(if is_problem { "failed" } else { "solved" }.into());

    // Opens the anchor explainer once per session, for a design that lacks an anchor.
    if super::state::should_open_anchor_explainer(solved_result.is_err()) {
        ui.global::<EditorModel>().set_anchor_explainer_open(true);
    }

    push_solve_dependent_panel_fields(ui, render_ctx, state, &delta, &solved_result, n_d);

    ui.global::<EditorModel>()
        .set_design_label(design_label(state).into());
    let rows: Vec<AngleItem> = solved.map_or_else(Vec::new, |solved| {
        cutting_instructions_rows(&state.design, solved)
    });
    push_rows(
        &ui.global::<EditorModel>().get_cutting_rows(),
        rows,
        |model| {
            ui.global::<EditorModel>().set_cutting_rows(model);
        },
    );

    refresh_deep_solve_availability(ui, state);
    super::panel_stale::refresh_optimize_availability(ui, state);
    solved_result.ok()
}

/// The warnings, preform, yield and proportion pushes of [`refresh_editor_panel_from_solve`].
/// Split out to stay under the function-length lint. An `Err` in `solved_result` is a design
/// that does not solve, and these fields show the failed-solve text from it.
fn push_solve_dependent_panel_fields(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    delta: &ScratchDelta,
    solved_result: &Result<Vec<SolvedTier>, DesignSolveError>,
    n_d: f64,
) {
    push_manufacturability_and_preform_scratch(ui, state, delta, solved_result.as_ref().ok());
    push_yield_and_proportions(ui, render_ctx, state, delta, solved_result, n_d);
}

/// The warnings and preform push half of [`push_solve_dependent_panel_fields`]. Split out to
/// stay under the function-length lint.
fn push_manufacturability_and_preform_scratch(
    ui: &MainWindow,
    state: &EditorState,
    delta: &ScratchDelta,
    solved: Option<&Vec<SolvedTier>>,
) {
    // Kept as tagged pairs so each warning keeps its tier index.
    let tagged_warnings =
        manufacturability_warnings_tagged(&state.design, solved.map(Vec::as_slice));
    let warning_tiers: Vec<i32> = tagged_warnings
        .iter()
        .map(|(index, _)| i32::try_from(*index).unwrap_or(i32::MAX))
        .collect();
    let warnings: Vec<SharedString> = tagged_warnings
        .into_iter()
        .map(|(_, text)| SharedString::from(text))
        .collect();
    push_rows(
        &ui.global::<EditorModel>().get_manufacturability_warnings(),
        warnings,
        |model| {
            ui.global::<EditorModel>()
                .set_manufacturability_warnings(model);
        },
    );
    push_rows(
        &ui.global::<EditorModel>()
            .get_manufacturability_warning_tiers(),
        warning_tiers,
        |model| {
            ui.global::<EditorModel>()
                .set_manufacturability_warning_tiers(model);
        },
    );

    if delta.preform {
        let preform = &state.design.preform;
        // A planned piece shows its bounding box in the Block form; the note under the shape
        // says where it came from (empty for a block or cylinder).
        ui.global::<EditorModel>()
            .set_preform_shape_index(match preform.shape {
                PreformShape::Block | PreformShape::Planes(_) => 0,
                PreformShape::Cylinder { .. } => 1,
            });
        ui.global::<EditorModel>()
            .set_preform_source_note(preform_source_note(preform).into());
        ui.global::<EditorModel>()
            .set_preform_half_width(format!("{:.4}", preform.half_width).into());
        ui.global::<EditorModel>()
            .set_preform_length_over_width(format!("{:.4}", preform.length_over_width).into());
        ui.global::<EditorModel>()
            .set_preform_depth(format!("{:.4}", preform.depth).into());
        // The mm equivalents of the fields above. Without a solve there is no scale, so both
        // are blank.
        let (preform_half_width_mm, preform_depth_mm) = solved.map_or_else(
            || (String::new(), String::new()),
            |solved| preform_mm_texts_from_solved(&state.design, solved),
        );
        ui.global::<EditorModel>()
            .set_preform_half_width_mm_text(preform_half_width_mm.into());
        ui.global::<EditorModel>()
            .set_preform_depth_mm_text(preform_depth_mm.into());
        // From the same solve as the mm texts above.
        let mm_per_unit = solved.and_then(|solved| state.design.yield_report(solved).mm_per_unit);
        ui.global::<EditorModel>().set_preform_y_offset_mm(
            preform_y_offset_mm_text(state.design.preform_y_offset, mm_per_unit).into(),
        );
    }
}

/// The yield and proportion push half of [`push_solve_dependent_panel_fields`]. Split out to
/// stay under the function-length lint.
///
/// `render_ctx` supplies the custom materials' specific gravities, so a custom catalogue
/// material's own gravity reaches the carat-weight estimate.
fn push_yield_and_proportions(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    delta: &ScratchDelta,
    solved_result: &Result<Vec<SolvedTier>, DesignSolveError>,
    n_d: f64,
) {
    let solved = solved_result.as_ref().ok();
    // Seeds the Yield form's scratch buffers when the design changed, then pushes the
    // read-only figures, which are always pushed.
    push_yield_material_scratch(ui, state, delta);
    let custom_sg = Arc::clone(
        &render_ctx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .custom_material_specific_gravity,
    );
    let (vol_yield_text, carat_text, sg_used_text, fit_text) =
        yield_report_texts_from_result(&state.design, solved_result, &custom_sg);
    ui.global::<EditorModel>()
        .set_volumetric_yield_text(vol_yield_text.into());
    ui.global::<EditorModel>()
        .set_carat_weight_text(carat_text.into());
    ui.global::<EditorModel>()
        .set_specific_gravity_used_text(sg_used_text.into());
    ui.global::<EditorModel>()
        .set_preform_fit_warning(fit_text.into());

    let (table_pct, crown_height, pavilion_depth, total_depth, length_to_width) =
        proportions_texts_from_result(&state.design, solved_result);
    ui.global::<EditorModel>()
        .set_proportions_table_pct(table_pct.into());
    ui.global::<EditorModel>()
        .set_proportions_crown_height(crown_height.into());
    ui.global::<EditorModel>()
        .set_proportions_pavilion_depth(pavilion_depth.into());
    ui.global::<EditorModel>()
        .set_proportions_total_depth(total_depth.into());
    ui.global::<EditorModel>()
        .set_proportions_length_to_width(length_to_width.into());

    // Girdle thickness and the crown, pavilion and girdle ratios. A design that does not solve
    // reads "-" for all four.
    let (girdle_thickness, crown_to_width, pavilion_to_width, girdle_to_width) = solved
        .map_or_else(
            || {
                let dash = || "-".to_string();
                (dash(), dash(), dash(), dash())
            },
            |solved| girdle_and_ratio_texts_from_solved(&state.design, solved),
        );
    ui.global::<EditorModel>()
        .set_girdle_thickness_text(girdle_thickness.into());
    ui.global::<EditorModel>()
        .set_crown_to_width_text(crown_to_width.into());
    ui.global::<EditorModel>()
        .set_pavilion_to_width_text(pavilion_to_width.into());
    ui.global::<EditorModel>()
        .set_girdle_to_width_text(girdle_to_width.into());

    push_proportion_verdicts_from_solved(ui, &state.design, solved.map(Vec::as_slice), n_d);
}

/// Computes and pushes the Preform tab's five proportion verdict chips, using the reference
/// table of `pleochroma_cut_core::proportions_windows`. Every path that publishes a solve's
/// numbers calls this, so the chips never lag behind the numbers: the synchronous solve, the
/// background solve's completion and a finished solid-preview replan. It also feeds the
/// overall verdict through `verdict::on_solved`.
///
/// The chips show "nothing to judge" (level `-1`, empty reason) when `solved` is `None` or the
/// design has no tiers, since a bare preform has no crown or pavilion angles to judge.
///
/// The verdict type is private to `state`, so it is not named here. It is computed and
/// consumed inside this function.
pub(in crate::gui::editor) fn push_proportion_verdicts_from_solved(
    ui: &MainWindow,
    design: &Design,
    solved: Option<&[SolvedTier]>,
    n_d: f64,
) {
    // The overall Good / Check / Problem verdict follows the same solves these chips do
    // (see `gui::editor::verdict`): one call here covers the synchronous Solve, a background
    // solve's completion and a finished solid-preview replan.
    crate::gui::editor::verdict::on_solved(ui, design, solved, n_d);
    let verdicts = (!design.tiers.is_empty())
        .then_some(solved)
        .flatten()
        .and_then(|solved| {
            design
                .stone_proportions(solved)
                .map(|proportions| proportion_verdicts(design, solved, &proportions, n_d))
        });
    let push = |level: i32,
                reason: &str,
                set_level: fn(&MainWindow, i32),
                set_reason: fn(&MainWindow, SharedString)| {
        set_level(ui, level);
        set_reason(ui, reason.into());
    };
    let (level, reason) = verdicts.as_ref().map_or((-1, String::new()), |v| {
        (v.table_pct.level, v.table_pct.reason.clone())
    });
    push(
        level,
        &reason,
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_table_level(v);
        },
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_table_reason(v);
        },
    );
    let (level, reason) = verdicts.as_ref().map_or((-1, String::new()), |v| {
        (v.crown_angle.level, v.crown_angle.reason.clone())
    });
    push(
        level,
        &reason,
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_crown_angle_level(v);
        },
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_crown_angle_reason(v);
        },
    );
    let (level, reason) = verdicts.as_ref().map_or((-1, String::new()), |v| {
        (v.pavilion_angle.level, v.pavilion_angle.reason.clone())
    });
    push(
        level,
        &reason,
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_pavilion_angle_level(v);
        },
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_pavilion_angle_reason(v);
        },
    );
    let (level, reason) = verdicts.as_ref().map_or((-1, String::new()), |v| {
        (v.total_depth_pct.level, v.total_depth_pct.reason.clone())
    });
    push(
        level,
        &reason,
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_total_depth_level(v);
        },
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_total_depth_reason(v);
        },
    );
    let (level, reason) = verdicts.as_ref().map_or((-1, String::new()), |v| {
        (v.girdle_pct.level, v.girdle_pct.reason.clone())
    });
    push(
        level,
        &reason,
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_girdle_level(v);
        },
        |ui, v| {
            ui.global::<EditorModel>()
                .set_proportion_verdict_girdle_reason(v);
        },
    );
}

/// Patches `state.pending_optimize`'s
/// `AngleChange`s onto `tiers` via [`apply_proposed_angles`] -- but only when
/// that pending result still applies to `state`'s CURRENT generation, the same
/// check [`super::panel_stale::refresh_optimize_availability`] already makes
/// for `EditorModel.optimize_can_apply`. A superseded result (an edit landed
/// since Optimize last ran) must not paint a ghost angle for a design it no
/// longer describes.
fn apply_pending_optimize_ghost(tiers: &mut [EditorTierItem], state: &EditorState) {
    let current_generation = state.generation.load(AtomicOrdering::Relaxed);
    let pending = state
        .pending_optimize
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some((outcome, started_generation)) = pending.as_ref()
        && *started_generation == current_generation
    {
        apply_proposed_angles(tiers, &outcome.changes);
    }
}

/// Applies the multi-select highlight, pushes `tiers` into `EditorModel.tiers` with the
/// multi-select count and the selected tier's chips, and refreshes the undo and redo state.
/// Shared by [`refresh_editor_panel_from_solve`] and [`super::panel_stale::push_stale_content`],
/// which differ only in how `tiers` is built.
pub(super) fn push_tier_list_and_undo_redo(
    ui: &MainWindow,
    state: &EditorState,
    mut tiers: Vec<EditorTierItem>,
) {
    apply_multi_selection(&mut tiers, &state.multi_selected);
    apply_pending_optimize_ghost(&mut tiers, state);
    push_tiers(ui, tiers);
    // Pushed from the `&EditorState` in hand, not from the `recompute_dirty` callback that
    // `changed tiers` fires inside `push_tiers`. That callback runs while the caller holds the
    // state, so reading it there would panic on the `RefCell`.
    ui.global::<EditorModel>().set_is_dirty(state.is_dirty());
    push_multi_selected_count(ui, state.multi_selected.len());
    super::inspector::push_selected_tier_chips(ui, state);
    ui.global::<EditorModel>()
        .set_can_undo(state.history.can_undo());
    ui.global::<EditorModel>()
        .set_can_redo(state.history.can_redo());
    // A step with words of its own ("Open variant ...") keeps them in the hints.
    ui.global::<UndoRedoLabels>()
        .set_undo_label(state.undo_hint().into());
    ui.global::<UndoRedoLabels>()
        .set_redo_label(state.redo_hint().into());
}

/// Seeds the Yield form's girdle-diameter and material scratch fields from the design. Each
/// field is gated on its own [`ScratchDelta`] flag. Shared with the stale panel, which seeds
/// the same fields whether or not a solve ran.
pub(super) fn push_yield_material_scratch(
    ui: &MainWindow,
    state: &EditorState,
    delta: &ScratchDelta,
) {
    let girdle_text = state
        .design
        .girdle_diameter_mm
        .map_or_else(String::new, |mm| format!("{mm:.4}"));
    // The design's own value is pushed on every refresh (not gated on the scratch delta): the
    // Stone tab's "Saved in the design" note reads it, never the scratch.
    ui.global::<EditorModel>()
        .set_design_girdle_diameter_text(girdle_text.as_str().into());
    if delta.girdle {
        ui.global::<EditorModel>()
            .set_girdle_diameter_mm(girdle_text.into());
    }
    if delta.material {
        // The material itself is named by the Design Settings material combo
        // (`EditorModel.material_combo_index`), not here.
        ui.global::<EditorModel>().set_specific_gravity_override(
            state
                .design
                .material
                .specific_gravity_override
                .map_or_else(String::new, |sg| format!("{sg:.4}"))
                .into(),
        );
    }
}
