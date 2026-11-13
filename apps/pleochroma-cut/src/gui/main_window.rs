//! Building and fully wiring a `MainWindow`, and the RAII handle that keeps its
//! render thread/settings-autosave/remote-rendering polling alive.

mod callbacks;
mod state;

use crate::{
    EditorModel, MainWindow, SettingsModel,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::render,
    settings::SettingsPersister,
};
use callbacks::{
    setup_editor, setup_library_callbacks, setup_live_view_mode_callbacks, setup_remote_and_batch,
    setup_render_callbacks, setup_solid_view_mode_callbacks, setup_window_close,
};
use slint::ComponentHandle;
use state::{
    apply_custom_materials, create_solid_view, load_settings, open_design_library,
    spawn_viewport_render_thread,
};
use std::sync::{Arc, Mutex, PoisonError};

/// The library file `open_design_library` opened last, so every second (read-only)
/// connection opens the same file.
static LIBRARY_PATH: Mutex<Option<String>> = Mutex::new(None);

/// The on-disk design library's path, as opened at startup. Background searches and loads
/// open their own read-only connections to it, hence `pub(in crate::gui)`.
pub(in crate::gui) fn db_path() -> String {
    LIBRARY_PATH
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .unwrap_or_else(|| {
            crate::settings::store::default_library_path()
                .to_string_lossy()
                .into_owned()
        })
}

/// Resolves the library path afresh and remembers it for [`db_path`].
fn resolve_db_path() -> String {
    let path = crate::settings::store::default_library_path()
        .to_string_lossy()
        .into_owned();
    *LIBRARY_PATH.lock().unwrap_or_else(PoisonError::into_inner) = Some(path.clone());
    path
}

/// Rebuilds the custom-material list (render context and material picker) from the vault. The
/// planner's "Use Color" calls it after it wrote a new material.
pub(in crate::gui) fn reload_custom_materials(
    ui: &MainWindow,
    db: &Arc<Mutex<pleochroma_vault::db::sqlite::Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    apply_custom_materials(ui, db, render_ctx);
}

/// A fully constructed and wired [`MainWindow`], not yet shown or running.
///
/// Built by [`build_main_window`]. Keeping it separate from `run_gui`'s `ui.run()` lets another
/// binary show this wired window alongside its own, on one shared event loop.
///
/// `ui` is the only public field. The other fields are RAII guards: dropping one would stop
/// the render thread, settings autosave or remote-rendering polling, so the caller only keeps
/// the handle alive.
pub struct MainWindowHandle {
    pub ui: MainWindow,
    // Never read after construction. They are kept so that dropping the handle stops the
    // render thread and remote-rendering polling. Removing them would drop them early.
    #[expect(
        dead_code,
        reason = "RAII guard field -- kept alive, never read; see comment above"
    )]
    render_ctx: Arc<Mutex<RenderContext>>,
    // Read by `run_gui` (`handle.settings_store.snapshot()`) to check whether the Edit
    // sub-tab's layout was touched, for `window_sizing::fit_initial_window_size`. It is also
    // kept alive so the debounced settings writer runs for the life of the window.
    pub(super) settings_store: Arc<SettingsPersister>,
    #[expect(
        dead_code,
        reason = "RAII guard field -- kept alive, never read; see comment above"
    )]
    remote_rendering_timer: slint::Timer,
}

/// Builds and fully wires a `MainWindow`, without running the event loop.
///
/// See [`MainWindowHandle`] for why this is split out from `super::run_gui`.
///
/// # Errors
///
/// Returns an error if constructing the `MainWindow` itself fails (a Slint platform
/// initialization failure), or if the on-disk design library fails to open AND the
/// in-memory fallback database also fails to open (SQLite itself unusable in this
/// process -- see the database-open block below). A plain on-disk open failure is
/// handled internally: it falls back to a throwaway in-memory database and reports
/// the failure via a persistent error toast rather than propagating it or panicking.
pub fn build_main_window() -> anyhow::Result<MainWindowHandle> {
    // Clears the state a previous window on this thread left behind (the UI harness builds
    // one per scenario). A no-op for the application's single window.
    super::window_release::release_window_state();
    let ui = MainWindow::new()?;

    // Build features, set once here because Slint has no `cfg!`. The "Local Compute" row
    // shows only in a `gpu` build, so a CPU-only build offers no choice it cannot act on.
    ui.global::<SettingsModel>()
        .set_gpu_build(cfg!(feature = "gpu"));
    // The "Beam Tracing" controls show only in a `beam` build.
    ui.global::<SettingsModel>()
        .set_beam_build(cfg!(feature = "beam"));
    // The "Adaptive Sampling" controls show only in an `adaptive-sampling` build.
    ui.global::<SettingsModel>()
        .set_adaptive_sampling_build(cfg!(feature = "adaptive-sampling"));

    // The editor is always built, so `EditorModel.enabled` is `true`. The Edit sub-tab and
    // other components read it as their guard.
    ui.global::<EditorModel>().set_enabled(true);

    let db = open_design_library(&ui)?;
    // The UI harness checks after each shutdown whether the window and this handle were freed.
    #[cfg(feature = "ui-harness")]
    crate::harness_support::register_window(&ui, &db);

    // Shared Render Context for 3D Viewport
    let render_ctx = Arc::new(Mutex::new(RenderContext::default()));

    let solid = create_solid_view(&ui, &render_ctx);

    // The library being browsed: the local database, or a remote worker. Starts local.
    let library_source: Arc<Mutex<LibrarySource>> = Arc::new(Mutex::new(LibrarySource::default()));

    apply_custom_materials(&ui, &db, &render_ctx);
    // The planner's "Use Color" needs the render context to select the material.
    crate::gui::rough_colour::adopt_link::register_render_context(&render_ctx);
    let settings_store = load_settings(&ui, &render_ctx);

    // Live Render visibility: outer tab / sub-tab change wiring -- see
    // `render_visibility`'s module doc comment.
    render::render_visibility::setup_live_render_visibility_callbacks(
        &ui,
        &render_ctx,
        &solid.preview_state,
    );
    // The settings load has set the restored view modes in the UI, but `render_ctx.tab_visible`
    // still holds its default `true`. Without this sync, a session restored into Solid mode
    // would keep tracing until a tab was touched. It runs once, after every signal that
    // `render_visibility::render_is_visible` reads is in place.
    render::render_visibility::recompute_tab_visible(&ui, &render_ctx);

    spawn_viewport_render_thread(&ui, &render_ctx, &solid);

    setup_render_callbacks(&ui, &db, &render_ctx, &settings_store, &solid);
    setup_library_callbacks(
        &ui,
        &db,
        &library_source,
        &render_ctx,
        &settings_store,
        &solid.preview_state,
    );
    setup_editor(&ui, &db, &library_source, &render_ctx, &solid);
    // The lighting a design remembers for itself (Parameters panel buttons + the hook the
    // editor calls when a design opens, see `render::design_lighting`).
    render::design_lighting::setup_design_lighting_callbacks(
        &ui,
        &db,
        &render_ctx,
        &settings_store,
    );
    setup_solid_view_mode_callbacks(&ui, &render_ctx, &settings_store, &solid.preview_state);
    setup_live_view_mode_callbacks(&ui, &render_ctx, &settings_store, &solid.preview_state);
    let remote_rendering_timer =
        setup_remote_and_batch(&ui, &db, &library_source, &render_ctx, &settings_store);
    setup_window_close(&ui, &render_ctx, &settings_store);
    // The new-version check: the one-time question, the Preferences switch and, when it is
    // due, the automatic check on a background thread (never before the user said yes).
    crate::gui::update_check::setup_update_check(&ui, &settings_store);
    // The RADV shader workaround (Linux, Mesa's AMD driver): the Preferences switch, the
    // one-time question once the GPU adapter is known, and the restart both end in.
    crate::gui::radv_workaround::setup_radv_workaround(&ui, &settings_store);

    // The app opens on the 3D tab's Live Render view in Solid mode, never on the editor.
    // Startup forces both view modes to Solid. It takes the same path as a click on the pill,
    // and runs after every handler is connected, so the visibility gate sees the real view.
    crate::gui::commands::enter_live_render_view(&ui);

    Ok(MainWindowHandle {
        ui,
        render_ctx,
        settings_store,
        remote_rendering_timer,
    })
}
