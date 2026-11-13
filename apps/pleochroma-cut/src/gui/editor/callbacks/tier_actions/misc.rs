//! Small standalone callbacks (tier filter, angle live-preview, anchor-explainer
//! dismiss) and the selection-index bookkeeping helpers shared across this module
//! group.

use std::{cell::RefCell, rc::Rc};

use pleochroma_editor::view_model::live_margin::angle_live_margin;
use slint::{ComponentHandle, Model, ModelRc, SharedString};

use crate::{
    EditorModel, EditorTierItem, MainWindow,
    gui::editor::state::{EditorState, tier_matches_filter},
};

/// The tier list's filter box asks Rust whether one row matches, because Slint's
/// `string` type has no substring test. A `pure` callback, so the table can call it
/// straight from a per-row binding.
pub(in crate::gui::editor) fn setup_tier_filter_callback(ui: &MainWindow) {
    ui.global::<EditorModel>().on_tier_matches_filter(
        |haystack: SharedString, filter: SharedString| tier_matches_filter(&haystack, &filter),
    );
    ui.global::<EditorModel>().on_tier_name_taken(
        |tiers: ModelRc<EditorTierItem>, name: SharedString| tier_name_taken(&tiers, &name),
    );
}

/// Whether one of `tiers` is already named `name` (any ASCII case; a `/`-joined tier name holds
/// several names, each one counts) -- the rule `parse_tier_form` refuses a save by.
fn tier_name_taken(tiers: &ModelRc<EditorTierItem>, name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && tiers.iter().any(|tier| {
            tier.name
                .split('/')
                .map(str::trim)
                .any(|existing| existing.eq_ignore_ascii_case(name))
        })
}

/// Live critical-angle guidance in the Tier form's Angle field, behind
/// `EditorModel.angle_live_preview` (`ui/models/editor.slint`).
///
/// The bar itself is [`angle_live_margin`], shared with the web inspector: the same
/// functions a saved row's MARGIN cell uses, so a value typed but not yet saved reads the
/// identical bar. `text` may be a number, arithmetic (`41.5+0.3`) or a relation
/// (`=P1-2`, read as the angle it would give). It uses the design's plain
/// [`pleochroma_cut_core::Design::effective_refractive_index`] (built-in materials only,
/// no custom-catalogue lookup) rather than threading `RenderContext` through for this
/// preview-only path -- the authoritative "Eff. RI"/critical-angle readouts elsewhere
/// already use the full custom-material-aware value; this is a live estimate while typing,
/// not the figure of record. Text that is not an angle, a zero angle, or a crown angle with
/// no pavilion tier in the design to estimate against, clears the bar (`level = -1`).
pub(in crate::gui::editor) fn setup_angle_live_preview_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_angle_live_preview(move |text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let model = ui.global::<EditorModel>();
            let margin = {
                let st = state.borrow();
                let n_d = st.design.effective_refractive_index();
                angle_live_margin(&st.design, n_d, &text)
            };
            if let Some(bar) = margin {
                model.set_angle_live_margin_text(bar.text.into());
                model.set_angle_live_margin_level(bar.level);
                model.set_angle_live_margin_is_estimate(bar.is_estimate);
            } else {
                model.set_angle_live_margin_text(SharedString::new());
                model.set_angle_live_margin_level(-1);
                model.set_angle_live_margin_is_estimate(false);
            }
        });
}

/// The anchor explainer card's "Got it" and "Don't show again" buttons. "Don't show again" also
/// persists the suppression (`state::anchor_explainer_suppress_permanently`, under the key
/// `"anchor_explainer"` in `AppSettings::suppressed_confirmations`), so
/// `state::should_open_anchor_explainer` never reopens the card.
pub(in crate::gui::editor) fn setup_anchor_explainer_dismiss_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_anchor_explainer_dismiss(move |dont_show_again: bool| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            ui.global::<EditorModel>().set_anchor_explainer_open(false);
            if dont_show_again {
                crate::gui::editor::state::anchor_explainer_suppress_permanently();
            }
        });
}

/// Bumps `EditorModel.form_reset_pulse`. A plain increment, not a flag, makes `EditorView`'s
/// watcher fire even when `selected_tier_index` is already `-1` and setting it again would raise
/// no `changed` (see `ui/models/editor.slint`).
pub(super) fn bump_form_reset_pulse(ui: &MainWindow) {
    let pulse = ui.global::<EditorModel>().get_form_reset_pulse();
    ui.global::<EditorModel>()
        .set_form_reset_pulse(pulse.wrapping_add(1));
}

/// Adjusts `EditorModel.selected_tier_index` after [`Edit::RemoveTier`] removed the tier at
/// `removed_index`, which shifts every later tier down by one. A selection on the removed tier is
/// cleared, and [`bump_form_reset_pulse`] blanks the form even when it was already `-1`. A
/// selection past the removed tier moves down by one to follow its tier. Anything below it is
/// left alone.
pub(super) fn adjust_selection_after_remove(ui: &MainWindow, removed_index: i32) {
    let selected = ui.global::<EditorModel>().get_selected_tier_index();
    if selected == removed_index {
        ui.global::<EditorModel>().set_selected_tier_index(-1);
        bump_form_reset_pulse(ui);
    } else if selected > removed_index {
        ui.global::<EditorModel>()
            .set_selected_tier_index(selected - 1);
    }
}

/// Clamps `EditorModel.selected_tier_index` after an undo or redo to `tier_count`: the table
/// rows after the replay, flat plus concave tiers. Unlike [`adjust_selection_after_remove`], an
/// undo or redo can change the count by any amount, so there is no shift to preserve. An index
/// that no longer names a tier is cleared, with the same [`bump_form_reset_pulse`] guarantee.
///
/// An in-range selection needs no pulse: the no-solve panel refresh just before
/// (`view::refresh_editor_panel_stale`) already re-seeded the form from the replayed design.
pub(super) fn clamp_selection_to_tier_count(ui: &MainWindow, tier_count: usize) {
    let selected = ui.global::<EditorModel>().get_selected_tier_index();
    let out_of_range = usize::try_from(selected).is_ok_and(|index| index >= tier_count);
    if out_of_range {
        ui.global::<EditorModel>().set_selected_tier_index(-1);
        bump_form_reset_pulse(ui);
    }
}
