//! The narrow, read-only surface the visual before/after compare window
//! (`gui::editor::compare`) reaches this dialog's pending proposal through.
//!
//! Nothing here commits an edit. The compare window's "Keep after" invokes
//! `RetargetModel.apply()` -- i.e. [`super::apply::setup_retarget_apply_callback`],
//! the exact handler the dialog's own Apply button runs -- after checking with
//! [`current_retarget_guard`] that the proposal it compared is still the one that
//! handler would commit. "Discard" turns the dialog's viewport ghost off through
//! [`discard_retarget_preview`], sharing the close handler's own revert.

use super::{
    RETARGET_ASYNC,
    apply::revert_ghost_to_live_design,
    build_retarget_candidate,
    check_run::{candidate_anchors, is_invalid},
    material::{target_display_name, target_material_selection},
};
use crate::{
    MainWindow, RetargetModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            retarget::{AnchorChange, RetargetProposal},
            state::EditorState,
            view,
        },
        solid_preview::preview_state::SolidPreviewState,
    },
};
use pleochroma_cut_core::{Design, MaterialSelection};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex, PoisonError, atomic::Ordering as AtomicOrdering};

/// Everything that decides what `RetargetModel.apply()` would commit right now: the
/// held proposal, the design generation it was built against, the masts the validity
/// check re-anchored and the target material the dialog's picker names. The compare
/// window keeps the value it opened with and refuses "Keep after" unless
/// [`current_retarget_guard`] still returns an equal one -- so the cutter can never keep
/// something other than what they inspected (the dialog stays interactive while the
/// window is open).
#[derive(Debug, Clone, PartialEq)]
pub(in crate::gui::editor) struct RetargetKeepGuard {
    proposal: RetargetProposal,
    generation: u64,
    anchors: Vec<AnchorChange>,
    material: MaterialSelection,
    candidate: Option<Design>,
}

/// What the compare window needs to open on the dialog's pending proposal.
pub(in crate::gui::editor) struct RetargetCompareInputs {
    /// The live design, cloned -- the "before" side.
    pub(in crate::gui::editor) current: Design,
    /// [`build_retarget_candidate`]'s candidate with the dialog's TARGET material
    /// attached -- the "after" side, exactly what Apply would leave behind (Apply
    /// commits the angles and the material as one `Edit::Batch`).
    pub(in crate::gui::editor) candidate: Option<Design>,
    /// The validated Shift stage in Optimize mode; live source design in Shift mode.
    pub(in crate::gui::editor) before_label: String,
    pub(in crate::gui::editor) before_unavailable: bool,
    /// The after side's header label, e.g. "Retarget to Ruby (Shift)".
    pub(in crate::gui::editor) after_label: String,
    /// The proposal this comparison describes -- see [`RetargetKeepGuard`].
    pub(in crate::gui::editor) guard: Option<RetargetKeepGuard>,
}

/// The held proposal, looked up in the same order
/// [`super::apply::setup_retarget_apply_callback`] consumes it: the Optimize option the cutter
/// picked ([`RETARGET_ASYNC`]) first, else a Shift-mode proposal on `EditorState`. Cloned,
/// never taken. Optimize mode never holds a Shift proposal, so the order only matters for a
/// stale leftover, which must not win over what the cutter picked.
fn pending_proposal(st: &EditorState) -> Option<(RetargetProposal, u64)> {
    RETARGET_ASYNC
        .with(|cell| cell.borrow().pending.clone())
        .or_else(|| st.pending_retarget.clone())
}

/// The dialog's target material, resolved exactly as the Apply handler resolves it.
fn target_selection(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    design: &Design,
) -> MaterialSelection {
    let custom = render_ctx
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .custom_materials
        .as_ref()
        .clone();
    let model = ui.global::<RetargetModel>();
    target_material_selection(
        design,
        &custom,
        model.get_target_material_index(),
        &model.get_target_ri_override_text(),
    )
}

/// What `RetargetModel.apply()` would commit if pressed right now, or `None` when
/// no proposal is held at all.
#[must_use]
pub(in crate::gui::editor) fn current_retarget_guard(
    ui: &MainWindow,
    st: &EditorState,
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> Option<RetargetKeepGuard> {
    let (proposal, generation) = pending_proposal(st)?;
    if generation != st.generation.load(AtomicOrdering::Relaxed) {
        return None;
    }
    let material = target_selection(ui, render_ctx, &st.design);
    // Still checking: no masts yet, which also differs from any finished guard, so a
    // window opened before the check finished can never be kept after it changed.
    let anchors = candidate_anchors(generation).unwrap_or_default();
    Some(RetargetKeepGuard {
        proposal,
        generation,
        anchors,
        material,
        candidate: super::check_run::checked_candidate(generation),
    })
}

/// The compare window's inputs for the held proposal.
///
/// # Errors
///
/// A cutter-facing sentence when there is nothing honest to compare: no proposal is
/// held, the design moved on since it was built (the same staleness Apply refuses), the
/// validity check is still running (the candidate needs its re-anchored masts), or its
/// retarget edit no longer applies to the live design. An INVALID change is compared:
/// that is how the cutter sees what is wrong with it.
pub(in crate::gui::editor) fn retarget_compare_inputs(
    ui: &MainWindow,
    st: &EditorState,
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> Result<RetargetCompareInputs, String> {
    if ui.global::<RetargetModel>().get_mode_index() == 1 {
        return optimized_compare_inputs(ui, st, render_ctx);
    }
    let guard = current_retarget_guard(ui, st, render_ctx)
        .ok_or_else(|| "No retarget proposal to compare yet.".to_string())?;
    if guard.generation != st.generation.load(AtomicOrdering::Relaxed) {
        return Err(
            "The design changed since this proposal was built -- click Recompute first."
                .to_string(),
        );
    }
    // Only the "still checking" refusal is wanted here: the candidate reads its masts from
    // the finished check itself.
    candidate_anchors(guard.generation)?;
    let mut candidate = build_retarget_candidate(&st.design)
        .or_else(|| super::shift_seed::preview(&st.design, guard.generation)?.candidate)
        .ok_or_else(|| {
            "This proposal no longer applies to the current design -- click Recompute first."
                .to_string()
        })?;
    candidate.material = guard.material.clone();
    let mode = if ui.global::<RetargetModel>().get_mode_index() == 1 {
        "Optimize"
    } else {
        "Shift"
    };
    let verdict = if is_invalid(guard.generation) {
        ", not valid"
    } else {
        ""
    };
    let after_label = format!(
        "Retarget to {} ({mode}{verdict})",
        target_display_name(&guard.material)
    );
    Ok(RetargetCompareInputs {
        current: st.design.clone(),
        candidate: Some(candidate),
        before_label: "Current Design".to_string(),
        before_unavailable: false,
        after_label,
        guard: Some(guard),
    })
}

/// Stage two is always measured against the certified Shift result. No original or
/// current-design fallback is meaningful here, even before a search has found an option.
fn optimized_compare_inputs(
    ui: &MainWindow,
    st: &EditorState,
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> Result<RetargetCompareInputs, String> {
    let generation = st.generation.load(AtomicOrdering::Relaxed);
    let material = target_selection(ui, render_ctx, &st.design);
    let shift = super::shift_seed::preview(&st.design, generation);
    let before_unavailable = shift.as_ref().is_none_or(|shift| shift.candidate.is_none());
    let before_label = match shift.as_ref() {
        Some(shift) if shift.validated => {
            format!("Validated Shift · {}", target_display_name(&material))
        }
        Some(shift) if shift.candidate.is_some() => format!("Shift · {}", shift.headline),
        Some(shift) => format!("Shift unavailable · {}", shift.headline),
        None => "Shift unavailable · waiting for its geometry check".to_string(),
    };
    // A missing Shift side uses a hidden render placeholder only. It is never displayed
    // or measured as a Shift design; the pane names the missing geometry explicitly.
    let mut shifted = shift
        .and_then(|shift| shift.candidate)
        .unwrap_or_else(|| st.design.clone());
    shifted.material = material.clone();
    let selected = ui.global::<RetargetModel>().get_selected_candidate() >= 0;
    let guard = selected
        .then(|| current_retarget_guard(ui, st, render_ctx))
        .flatten()
        .filter(|guard| guard.generation == generation);
    let candidate = guard.as_ref().and_then(|_| {
        let mut candidate = build_retarget_candidate(&st.design)?;
        candidate.material = material.clone();
        Some(candidate)
    });
    if selected && candidate.is_none() {
        return Err("The selected candidate is no longer current. Recompute first.".to_string());
    }
    Ok(RetargetCompareInputs {
        current: shifted,
        candidate,
        before_label,
        before_unavailable,
        after_label: if selected {
            format!("Optimized · {}", target_display_name(&material))
        } else {
            "Waiting for an optimized candidate".to_string()
        },
        guard,
    })
}

/// The compare window's "Discard" for a Retarget comparison: switches the legacy viewport
/// ghost preview (`RetargetModel.preview_enabled`, which the dialog no longer offers a
/// toggle for) off and, when it was on, puts the live design back in the
/// shared viewport through the close handler's own revert
/// ([`revert_ghost_to_live_design`]). The proposal itself stays pending and the
/// dialog stays open, so the cutter can adjust it and compare again.
pub(in crate::gui::editor) fn discard_retarget_preview(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
    st: &EditorState,
) {
    let model = ui.global::<RetargetModel>();
    if !model.get_preview_enabled() {
        return;
    }
    model.set_preview_enabled(false);
    revert_ghost_to_live_design(ui, render_ctx, preview_state, solid_last_solved, st);
}
