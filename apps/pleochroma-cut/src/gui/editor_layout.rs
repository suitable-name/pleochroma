//! Persists the Edit sub-tab's dock width, inspector height and section collapse states.
//!
//! [`apply_editor_layout_from_settings`] restores them at startup and
//! [`setup_editor_layout_callbacks`] writes them back on every change.
//!
//! `EditorModel` itself is a plain Slint global, always compiled in.

use crate::{
    EditorModel, MainWindow,
    settings::{SettingsPersister, model::AppSettings},
};
use slint::ComponentHandle;
use std::sync::Arc;

/// Minimum inspector height while the Edit layout is untouched.
///
/// Taller than `AppSettings::DEFAULT_EDITOR_INSPECTOR_HEIGHT`, so the Tier tab's `ScrollView`
/// shows its title and the Angle, Meets, Name and Indices rows. The Save/Add Tier row sits
/// outside that scroll area and needs about 38 px more. Applied in
/// [`apply_editor_layout_from_settings`] only while `editor_layout_touched` is `false`; a saved
/// height is used as it is.
const FIRST_RUN_INSPECTOR_HEIGHT: f32 = 380.0;

/// Applies the persisted Edit layout to `EditorModel`. Called once at startup, after
/// `startup_settings::apply_loaded_settings` and before [`setup_editor_layout_callbacks`].
///
/// The setters queue `EditorModel`'s `changed` handlers, so `layout_changed` fires only at
/// the window's first flush, which looks the same as a user drag. A raised inspector height
/// would otherwise be saved as if the user had chosen it, so
/// [`setup_editor_layout_callbacks`] skips that first firing.
pub(super) fn apply_editor_layout_from_settings(ui: &MainWindow, s: &AppSettings) {
    let editor = ui.global::<EditorModel>();
    editor.set_dock_width(s.editor_dock_width);
    let inspector_height = if s.editor_layout_touched {
        s.editor_inspector_height
    } else {
        s.editor_inspector_height.max(FIRST_RUN_INSPECTOR_HEIGHT)
    };
    editor.set_inspector_height(inspector_height);
    editor.set_settings_collapsed(s.editor_settings_collapsed);
    editor.set_inspector_collapsed(s.editor_inspector_collapsed);
    editor.set_tier_table_collapsed(s.editor_tier_table_collapsed);
    editor.set_remap_collapsed(s.editor_remap_collapsed);
}

/// Persists the Edit layout through the same debounced writer as the other settings.
///
/// `EditorModel.layout_changed` fires on every change to the dock width, inspector height or a
/// collapsed flag. The handler is wired like `on_panel_collapsed_changed` in
/// `gui::main_window::callbacks`.
///
/// Also sets `AppSettings::editor_layout_touched`, which turns off the small-screen inspector
/// default of `window_sizing` for good: nothing resets it to `false`.
///
/// The first firing is skipped. It is the flush of [`apply_editor_layout_from_settings`]'s
/// push, not a user action. Without the skip, a fresh settings file would set
/// `editor_layout_touched` on the first launch, because [`FIRST_RUN_INSPECTOR_HEIGHT`] differs
/// from the stored height, and defeat the small-screen default from the second launch on.
pub(super) fn setup_editor_layout_callbacks(
    ui: &MainWindow,
    settings_store: &Arc<SettingsPersister>,
) {
    let settings_store = settings_store.clone();
    let ui_weak = ui.as_weak();
    let startup_flush_pending = std::cell::Cell::new(true);
    ui.global::<EditorModel>().on_layout_changed(move || {
        if startup_flush_pending.replace(false) {
            return;
        }
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let editor = ui.global::<EditorModel>();
        let dock_width = editor.get_dock_width();
        let inspector_height = editor.get_inspector_height();
        let settings_collapsed = editor.get_settings_collapsed();
        let inspector_collapsed = editor.get_inspector_collapsed();
        let tier_table_collapsed = editor.get_tier_table_collapsed();
        let remap_collapsed = editor.get_remap_collapsed();
        settings_store.update(|s| {
            s.settings.editor_dock_width = dock_width;
            s.settings.editor_inspector_height = inspector_height;
            s.settings.editor_settings_collapsed = settings_collapsed;
            s.settings.editor_inspector_collapsed = inspector_collapsed;
            s.settings.editor_tier_table_collapsed = tier_table_collapsed;
            s.settings.editor_remap_collapsed = remap_collapsed;
            s.settings.editor_layout_touched = true;
        });
    });
}
