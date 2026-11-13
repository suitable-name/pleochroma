//! The editor panel refresh without a solve.
//!
//! [`refresh_editor_panel_stale`] runs after every edit that is not the explicit "Solve". It is
//! the counterpart of [`super::panel::refresh_editor_panel_from_solve`]. The Deep Solve and
//! Optimize availability pushes are shared by both paths.

use super::{
    design_label::design_label,
    panel::{push_tier_list_and_undo_redo, push_yield_material_scratch},
    solve_results::{deep_solve_hint, optimize_hint},
    state::{
        EditorState, ScratchDelta, TOOLTIP_LINE_CHARS, manufacturability_warnings_tagged,
        preform_y_offset_mm_text, result_is_stale, tier_items_stale_with_last_solved,
        wrap_tooltip_text,
    },
};
use crate::{
    AngleItem, EditorModel,
    bridge::render_thread::RenderContext,
    gui::editor::{auto_solve, stale},
};
use pleochroma_cut_core::PreformShape;
use pleochroma_editor::loading::preform_source_note;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::Cell,
    collections::BTreeSet,
    sync::{Arc, Mutex, PoisonError, atomic::Ordering as AtomicOrdering},
};

/// The design state the Tier inspector form was last reconciled against: the edit
/// generation and the tier count. Two equal marks mean no edit landed in between,
/// so the form has nothing to re-seed from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FormSyncMark {
    /// `EditorState::generation` at the last refresh.
    generation: u64,
    /// `design.tiers.len()` at the last refresh.
    tier_count: usize,
}

thread_local! {
    /// The [`FormSyncMark`] the last refresh recorded (`None` before the first). UI
    /// thread only: every caller is a Slint callback, and keeping the mark here leaves
    /// `EditorState` untouched by a purely presentational bookkeeping value.
    static LAST_FORM_SYNC: Cell<Option<FormSyncMark>> = const { Cell::new(None) };
}

/// Whether the Tier form owes a re-seed after a refresh, and if so whether the tier
/// COUNT changed since the previous one (the payload of
/// `EditorModel.form_reseed_tier_count_changed`).
///
/// `None` when nothing changed since `last` (a refresh that follows no edit, e.g.
/// the availability pushes, must never disturb a form the cutter is typing into) or
/// when the selection does not survive (`selection_survives == false`: the form is
/// blanked through `form_reset_pulse` instead, and re-seeding a row that is gone
/// would resurrect it). A missing `last` re-seeds once, since nothing is known about
/// what the form was built from.
fn form_reseed_request(
    last: Option<FormSyncMark>,
    now: FormSyncMark,
    selection_survives: bool,
) -> Option<bool> {
    if !selection_survives {
        return None;
    }
    match last {
        Some(previous) if previous == now => None,
        Some(previous) => Some(previous.tier_count != now.tier_count),
        None => Some(false),
    }
}

/// Bumps `EditorModel.form_reseed_pulse` with `form_reseed_tier_count_changed`
/// written first, so `EditorView`'s watcher reads the matching flag.
///
/// A plain increment rather than a flag: a same-value property write raises no
/// `changed` (see `EditorModel.form_reset_pulse`'s doc comment for the rule).
fn push_form_reseed_pulse(ui: &crate::MainWindow, tier_count_changed: bool) {
    let model = ui.global::<EditorModel>();
    model.set_form_reseed_tier_count_changed(tier_count_changed);
    let pulse = model.get_form_reseed_pulse();
    model.set_form_reseed_pulse(pulse.wrapping_add(1));
}

/// Re-seeds the Tier inspector form when an edit landed since the previous refresh and the
/// selected tier is still there.
///
/// The form is a snapshot that only a selection change loads. Without this, an edit that keeps
/// the selection would leave pre-edit values in the form, and Save Tier would write them back.
/// `EditorInspector.reseed_after_external_change` decides what the pulse does per form: a clean
/// form follows the design, a dirty one keeps the draft.
pub(super) fn sync_tier_form_with_design(ui: &crate::MainWindow, state: &EditorState) {
    let now = FormSyncMark {
        generation: state.generation.load(AtomicOrdering::Relaxed),
        // Concave tiers count too: a concave row is a selectable table row, and adding or
        // removing one is as structural as it is for a flat tier.
        tier_count: state.design.tiers.len() + state.design.concave_tiers.len(),
    };
    let previous = LAST_FORM_SYNC.with(|cell| cell.replace(Some(now)));
    let selected = ui.global::<EditorModel>().get_selected_tier_index();
    let selection_survives = usize::try_from(selected).is_ok_and(|index| index < now.tier_count);
    if let Some(tier_count_changed) = form_reseed_request(previous, now, selection_survives) {
        push_form_reseed_pulse(ui, tier_count_changed);
    }
}

/// Pushes Deep Solve's `available` and `hint` properties (see [`deep_solve_hint`]). Both refresh
/// paths call it, since whether there is anything to repair depends on the tier list.
pub(super) fn refresh_deep_solve_availability(ui: &crate::MainWindow, state: &EditorState) {
    let (available, hint) = deep_solve_hint(state);
    ui.global::<EditorModel>()
        .set_deep_solve_available(available);
    ui.global::<EditorModel>()
        .set_deep_solve_tooltip(wrap_tooltip_text(&hint, TOOLTIP_LINE_CHARS).into());
    ui.global::<EditorModel>().set_deep_solve_hint(hint.into());
}

/// Pushes Optimize's `available` and `hint` properties (see [`optimize_hint`]). Both refresh
/// paths call it.
///
/// It also re-derives `optimize_can_apply` against the current generation, so Apply is greyed
/// out as soon as an edit makes the pending result stale. The Optimize run callbacks set the
/// property to false when a run starts, and when it ends without a result that can be applied.
pub(super) fn refresh_optimize_availability(ui: &crate::MainWindow, state: &EditorState) {
    let (available, hint) = optimize_hint(state, configured_optimize_max_evaluations(ui));
    ui.global::<EditorModel>().set_optimize_available(available);
    // Tooltip-only text: hand-wrapped (see `refresh_deep_solve_availability`).
    ui.global::<EditorModel>()
        .set_optimize_hint(wrap_tooltip_text(&hint, TOOLTIP_LINE_CHARS).into());

    let current_generation = state.generation.load(AtomicOrdering::Relaxed);
    let can_apply = state
        .pending_optimize
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|(_, started_generation)| *started_generation == current_generation);
    ui.global::<EditorModel>().set_optimize_can_apply(can_apply);
    // The Optimize tab's own state. It restates the availability above, with the anchored-tier
    // choice taken into account.
    crate::gui::editor::optimize_panel::refresh(ui, state);
}

/// The evaluation budget a fresh Optimize run would use, read from
/// `EditorModel.optimize_budget_text` with `pleochroma_editor::optimize_view::parse_budget`.
/// An empty, unreadable or zero value falls back to the default budget. A run reads the same
/// property through the same parser, so the quoted number matches the one a click uses.
fn configured_optimize_max_evaluations(ui: &crate::MainWindow) -> usize {
    pleochroma_editor::optimize_view::parse_budget(
        &ui.global::<EditorModel>().get_optimize_budget_text(),
    )
    .unwrap_or(pleochroma_editor::optimize_view::DEFAULT_BUDGET)
}

/// The no-solve counterpart of [`super::panel::refresh_editor_panel`], run after every edit
/// except the explicit "Solve". The tier rows come from the last solve where `dirty` does not
/// name them. The validation banner shows a fixed "not solved" message. This function never
/// calls `Design::solve`, not even indirectly.
///
/// It re-seeds the Tier form (see [`sync_tier_form_with_design`]), then calls
/// [`auto_solve::on_edit`], which schedules the debounced background solve.
/// [`push_stale_content`] is the part without that call, for `refresh_all`'s large-design
/// path, which dispatches its own background solve.
///
/// `dirty` holds the tier indices the edit touched, as `tier_items_stale_with_last_solved`
/// reads them. Pass every index when the edit's reach is not tracked, as Undo, Redo, a gear
/// remap and a symmetry change do.
pub(in crate::gui::editor) fn refresh_editor_panel_stale(
    ui: &crate::MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    dirty: &BTreeSet<usize>,
) {
    push_stale_content(ui, render_ctx, state, dirty);
    auto_solve::on_edit(ui, render_ctx, state);
    // Every ordinary edit ends here (tier form, quick add, inline edits,
    // undo/redo, material/yield applies) -- the worked-example guide's main
    // automatic-advance hook.
    crate::gui::editor::guide::check_progress(ui, state);
}

/// The stale flags of the analysis results, part of [`push_stale_content`]. They are recomputed
/// on every edit, so the "Stale" badge appears as soon as a result is outdated.
fn push_stale_generation_badges(
    ui: &crate::MainWindow,
    state: &EditorState,
    current_generation: u64,
) {
    ui.global::<EditorModel>()
        .set_deep_solve_stale(result_is_stale(
            state.deep_solve_result_generation,
            current_generation,
        ));
    let optimize_result_generation = state
        .pending_optimize
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|(_, generation)| *generation);
    ui.global::<EditorModel>()
        .set_optimize_stale(result_is_stale(
            optimize_result_generation,
            current_generation,
        ));
    // The Retarget proposal has no generation field in `EditorState`, so `stale` tracks it.
    stale::refresh_badges(ui, current_generation);
}

/// The manufacturability warnings part of [`push_stale_content`]. The mesh-based checks need a
/// solve, so only the mast-free checks (gear quantization, cut order) are shown here, tagged
/// "(pre-solve)".
fn push_stale_warnings(ui: &crate::MainWindow, state: &EditorState) {
    let tagged_warnings = manufacturability_warnings_tagged(&state.design, None);
    let warning_tiers: Vec<i32> = tagged_warnings
        .iter()
        .map(|(index, _)| i32::try_from(*index).unwrap_or(i32::MAX))
        .collect();
    let warnings: Vec<SharedString> = tagged_warnings
        .into_iter()
        .map(|(_, text)| SharedString::from(text))
        .collect();
    super::state::push_rows(
        &ui.global::<EditorModel>().get_manufacturability_warnings(),
        warnings,
        |model| {
            ui.global::<EditorModel>()
                .set_manufacturability_warnings(model);
        },
    );
    super::state::push_rows(
        &ui.global::<EditorModel>()
            .get_manufacturability_warning_tiers(),
        warning_tiers,
        |model| {
            ui.global::<EditorModel>()
                .set_manufacturability_warning_tiers(model);
        },
    );
}

/// The preform and yield part of [`push_stale_content`]. The preform scratch fields reseed only
/// when the design's preform changed (`delta.preform`). The mm readouts and the yield figures
/// need a solve, so they are cleared on every stale push.
fn push_stale_preform_and_yield(ui: &crate::MainWindow, state: &EditorState, delta: &ScratchDelta) {
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
    }
    ui.global::<EditorModel>()
        .set_preform_half_width_mm_text("".into());
    ui.global::<EditorModel>()
        .set_preform_depth_mm_text("".into());
    // `mm_per_unit: None` always resolves to `""`.
    ui.global::<EditorModel>().set_preform_y_offset_mm(
        preform_y_offset_mm_text(state.design.preform_y_offset, None).into(),
    );

    // The form fields are seeded when their value changed (`delta`). The read-only figures
    // are cleared, since they need a solve.
    push_yield_material_scratch(ui, state, delta);
    ui.global::<EditorModel>()
        .set_volumetric_yield_text("".into());
    ui.global::<EditorModel>().set_carat_weight_text("".into());
    ui.global::<EditorModel>()
        .set_specific_gravity_used_text("".into());
    ui.global::<EditorModel>()
        .set_preform_fit_warning("".into());
}

/// Clears the proportions, cutting instructions and facet count, which need a solve. They come
/// back with the next solve.
fn push_stale_proportions_reset(ui: &crate::MainWindow) {
    ui.global::<EditorModel>()
        .set_proportions_table_pct("-".into());
    ui.global::<EditorModel>()
        .set_proportions_crown_height("-".into());
    ui.global::<EditorModel>()
        .set_proportions_pavilion_depth("-".into());
    ui.global::<EditorModel>()
        .set_proportions_total_depth("-".into());
    ui.global::<EditorModel>()
        .set_proportions_length_to_width("-".into());
    ui.global::<EditorModel>()
        .set_girdle_thickness_text("-".into());
    ui.global::<EditorModel>()
        .set_crown_to_width_text("-".into());
    ui.global::<EditorModel>()
        .set_pavilion_to_width_text("-".into());
    ui.global::<EditorModel>()
        .set_girdle_to_width_text("-".into());
    ui.global::<EditorModel>()
        .set_cutting_rows(ModelRc::new(VecModel::from(Vec::<AngleItem>::new())));
    // Facet count is 0 until a solve reports it.
    ui.global::<EditorModel>().set_facet_count(0);
    // The proportion verdict chips show "nothing to judge yet" until the next solve. This is a
    // direct push, since no design or effective refractive index is at hand here.
    ui.global::<EditorModel>()
        .set_proportion_verdict_table_level(-1);
    ui.global::<EditorModel>()
        .set_proportion_verdict_table_reason("".into());
    ui.global::<EditorModel>()
        .set_proportion_verdict_crown_angle_level(-1);
    ui.global::<EditorModel>()
        .set_proportion_verdict_crown_angle_reason("".into());
    ui.global::<EditorModel>()
        .set_proportion_verdict_pavilion_angle_level(-1);
    ui.global::<EditorModel>()
        .set_proportion_verdict_pavilion_angle_reason("".into());
    ui.global::<EditorModel>()
        .set_proportion_verdict_total_depth_level(-1);
    ui.global::<EditorModel>()
        .set_proportion_verdict_total_depth_reason("".into());
    ui.global::<EditorModel>()
        .set_proportion_verdict_girdle_level(-1);
    ui.global::<EditorModel>()
        .set_proportion_verdict_girdle_reason("".into());
}

/// The no-solve content push, without the auto-solve hook. See [`refresh_editor_panel_stale`].
/// Each part pushes its own group of `EditorModel` properties.
pub(super) fn push_stale_content(
    ui: &crate::MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    dirty: &BTreeSet<usize>,
) {
    super::viewport::push_trace_staleness(ui, render_ctx, state);
    let current_generation = state.generation.load(AtomicOrdering::Relaxed);
    push_stale_generation_badges(ui, state, current_generation);

    let delta = state.record_scratch_push();
    let n_d = super::inspector::refresh_design_settings(ui, render_ctx, state, &delta);
    // The last solve from before this edit still describes the rows `dirty` does not name. It
    // is read from the shared handle, and its generation is not checked: `dirty` already says
    // which rows changed.
    let last_solved = auto_solve::solid_last_solved().and_then(|cache| {
        cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .map(|(_, solved)| solved)
    });
    let tiers =
        tier_items_stale_with_last_solved(&state.design, n_d, last_solved.as_deref(), dirty);
    push_tier_list_and_undo_redo(ui, state, tiers);
    // After the tier rows: the form re-seeds from `EditorModel.tiers`.
    sync_tier_form_with_design(ui, state);

    ui.global::<EditorModel>().set_status_text(
        "Not solved -- click Solve to compute masts and validate this design.".into(),
    );
    ui.global::<EditorModel>().set_status_is_problem(true);
    ui.global::<EditorModel>().set_solve_state("stale".into());

    push_stale_warnings(ui, state);
    push_stale_preform_and_yield(ui, state, &delta);
    push_stale_proportions_reset(ui);
    // The overall verdict on screen no longer describes this design: dim it and hold its
    // Fix buttons until the next solve brings a new one (`gui::editor::verdict`).
    crate::gui::editor::verdict::mark_stale(ui);

    // The design label is cheap and never solve-dependent -- always kept current.
    ui.global::<EditorModel>()
        .set_design_label(design_label(state).into());

    refresh_deep_solve_availability(ui, state);
    refresh_optimize_availability(ui, state);
}

#[cfg(test)]
mod tests {
    use super::{FormSyncMark, form_reseed_request};

    const fn mark(generation: u64, tier_count: usize) -> FormSyncMark {
        FormSyncMark {
            generation,
            tier_count,
        }
    }

    #[test]
    fn a_new_generation_with_a_surviving_selection_requests_a_reseed() {
        assert_eq!(
            form_reseed_request(Some(mark(4, 6)), mark(5, 6), true),
            Some(false),
            "an in-place edit re-seeds without claiming the tier count moved"
        );
    }

    #[test]
    fn a_tier_count_change_is_reported_in_the_request() {
        assert_eq!(
            form_reseed_request(Some(mark(4, 6)), mark(5, 7), true),
            Some(true)
        );
        assert_eq!(
            form_reseed_request(Some(mark(4, 6)), mark(5, 5), true),
            Some(true)
        );
    }

    #[test]
    fn an_unchanged_design_never_requests_a_reseed() {
        assert_eq!(
            form_reseed_request(Some(mark(4, 6)), mark(4, 6), true),
            None,
            "a refresh with no edit must not disturb a form being typed into"
        );
    }

    #[test]
    fn a_vanished_selection_leaves_the_form_to_the_reset_pulse() {
        assert_eq!(
            form_reseed_request(Some(mark(4, 6)), mark(5, 5), false),
            None
        );
        assert_eq!(form_reseed_request(None, mark(1, 0), false), None);
    }

    #[test]
    fn the_first_refresh_reseeds_once() {
        assert_eq!(form_reseed_request(None, mark(1, 3), true), Some(false));
    }
}
