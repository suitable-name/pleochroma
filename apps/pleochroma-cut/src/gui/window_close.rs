//! The window-close unsaved-changes guard's Save/Discard callbacks.

use crate::{
    EditorModel, MainWindow, bridge::render_thread::RenderContext, settings::SettingsPersister,
};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// The Save and Discard callbacks of the window-close unsaved-changes guard
/// (`MainWindow.close_confirm_save` and `close_confirm_discard`). Cancel needs no Rust
/// handler (see `close_confirm_open` in `app.slint`). Both finish with the sequence that
/// `on_close_requested` runs for a clean design: `running = false`, `settings_store.flush()`
/// and hide.
///
/// Save is asynchronous (the design is resolved and written on a spawned thread), so the
/// window closes only when the save lands: the
/// [`crate::gui::editor::native_io::AfterSave::CloseWindow`] listener below does it. A
/// cancelled or failed save clears `after_save` without firing the listener, and the
/// window stays open.
pub(super) fn setup_close_confirm_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    // Runs when the save this guard's "Save" started completes. A value that is not
    // `CloseWindow` belongs to another listener and is put back untouched (see
    // `crate::gui::editor::native_io::on_save_completed`).
    let render_ctx_listener = render_ctx.clone();
    let settings_store_listener = settings_store.clone();
    crate::gui::editor::native_io::on_save_completed(move |ui, state| {
        {
            let mut st = state.borrow_mut();
            match st.after_save.take() {
                Some(crate::gui::editor::native_io::AfterSave::CloseWindow) => {}
                // Not this guard's save (or none pending): put it back as found.
                other => {
                    st.after_save = other;
                    return;
                }
            }
        }
        RenderContext::lock(&render_ctx_listener).running = false;
        settings_store_listener.flush();
        // See `main_window`'s `on_close_requested`: the compare window must not
        // outlive the main one.
        crate::gui::editor::close_compare_window();
        crate::gui::rough_plan::close_planner_window();
        crate::gui::color_pattern::close_window();
        crate::gui::material_reference::close_window();
        crate::gui::render_jobs::stop_for_app_close();
        let _ = ui.hide();
    });

    let ui_weak_save = ui.as_weak();
    ui.on_close_confirm_save(move || {
        let Some(ui) = ui_weak_save.upgrade() else {
            return;
        };
        ui.set_close_confirm_open(false);
        // Marks `after_save` so the listener above closes the window once this save lands.
        crate::gui::editor::native_io::request_save_then_close();
        ui.global::<EditorModel>().invoke_save_native();
    });

    let render_ctx_discard = render_ctx.clone();
    let settings_store_discard = settings_store.clone();
    let ui_weak_discard = ui.as_weak();
    ui.on_close_confirm_discard(move || {
        let Some(ui) = ui_weak_discard.upgrade() else {
            return;
        };
        ui.set_close_confirm_open(false);
        RenderContext::lock(&render_ctx_discard).running = false;
        settings_store_discard.flush();
        crate::gui::editor::close_compare_window();
        crate::gui::rough_plan::close_planner_window();
        crate::gui::color_pattern::close_window();
        crate::gui::material_reference::close_window();
        crate::gui::render_jobs::stop_for_app_close();
        let _ = ui.hide();
    });
}
