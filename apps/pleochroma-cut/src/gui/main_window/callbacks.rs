//! The window's callback wiring: layout persistence, render, optics, tilt, library, editor,
//! view-mode, remote-rendering and window-close callbacks.

use super::state::SolidView;
use crate::{
    EditorModel, LibraryModel, MainWindow, SolidPreviewModel, ViewportModel,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::{
        batch, commands, editor, editor_layout,
        external_links::{setup_reveal_last_saved_callback, setup_user_manual_callback},
        library, optics,
        remote::{setup_remote_rendering, setup_worker_callbacks},
        render, rough_plan,
        solid_preview::preview_state::{SolidPickState, SolidPreviewState},
        tilt,
        view_modes::{EditViewMode, LiveViewMode},
        window_close::setup_close_confirm_callbacks,
    },
    settings::SettingsPersister,
};
use pleochroma_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// Wires the persisted-layout, render, optics and tilt callbacks.
pub(super) fn setup_render_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    solid: &SolidView,
) {
    // Persists the library panel's collapsed state through the debounced settings writer.
    let settings_store_panel = settings_store.clone();
    ui.global::<LibraryModel>()
        .on_panel_collapsed_changed(move |collapsed: bool| {
            settings_store_panel.update(|s| s.settings.library_panel_collapsed = collapsed);
        });
    // Persists the two viewport flags: the gestures card shown once, and the folded figures
    // card. No tab reset touches them.
    // Each handler also writes the value back into its `in` property.
    let settings_store_gestures = settings_store.clone();
    let ui_weak_gestures = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_gestures_seen_set(move |seen: bool| {
            settings_store_gestures.update(|s| s.settings.gestures_seen = seen);
            if let Some(ui) = ui_weak_gestures.upgrade() {
                ui.global::<ViewportModel>().set_gestures_seen(seen);
            }
        });
    let settings_store_fold = settings_store.clone();
    let ui_weak_fold = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_figures_folded_set(move |folded: bool| {
            settings_store_fold.update(|s| s.settings.figures_folded = folded);
            if let Some(ui) = ui_weak_fold.upgrade() {
                ui.global::<ViewportModel>().set_figures_folded(folded);
            }
        });
    // Persists the Edit sub-tab's dock width, inspector height and collapsed sections.
    editor_layout::setup_editor_layout_callbacks(ui, settings_store);
    // The Parameters dock's open/tab state: restored now, saved on every change.
    crate::gui::parameters_dock::setup_parameters_dock(ui, settings_store);
    // The Preferences dialog and the header's Simple | Advanced pill: save each choice
    // and apply it -- see `gui::preferences`.
    crate::gui::preferences::setup_preferences_callbacks(ui, settings_store);
    // The same dialog's "Adaptive Sampling in the Viewport" switch, which also drives the render.
    crate::gui::preferences::setup_adaptive_viewport_callback(ui, render_ctx, settings_store);
    // And the path cache entries (viewport switch, disk limit, Open Folder, Clear Path Cache).
    crate::gui::preferences::setup_path_cache_callbacks(ui, render_ctx, settings_store);
    // The Faceting Machine section of the Preferences dialog: seeded from the saved preference.
    render::faceting_machine_ui::setup_faceting_machine_callbacks(ui, render_ctx, settings_store);

    setup_user_manual_callback(ui);
    // Context help ("?" buttons, the planner's Help button) and the glossary dialog.
    crate::gui::help::setup_help_callbacks(ui);
    setup_reveal_last_saved_callback(ui);
    render::camera_lighting::setup_camera_and_lighting_callbacks(
        ui,
        render_ctx,
        settings_store,
        &solid.preview_state,
        &solid.mesh_bounding_radius,
    );
    render::camera_lighting::setup_camera_drag_callbacks(ui, render_ctx);
    render::material::setup_material_changed_callback(ui, render_ctx);
    render::quality::setup_stone_shadow_callback(ui, render_ctx, settings_store);
    render::quality::setup_tray_reflection_callback(ui, render_ctx, settings_store);
    render::quality::setup_backdrop_callback(ui, render_ctx, settings_store);
    render::quality::setup_surface_glare_callback(ui, render_ctx, settings_store);
    render::quality::setup_head_shadow_callback(ui, render_ctx, settings_store);
    render::quality::setup_material_and_quality_callbacks(ui, render_ctx, settings_store);
    render::material::setup_material_effect_override_callbacks(ui, render_ctx);
    render::parameter_constants::publish_parameter_constants(ui);
    // "Reset this tab" for the Stone and Render & Output tabs (Lighting has its own handler).
    render::reset_tab::setup_reset_tab_callback(ui, render_ctx, settings_store);
    render::camera_lighting::setup_environment_map_callbacks(ui, render_ctx, settings_store);
    render::lighting_presets::setup_lighting_preset_callbacks(
        ui,
        render_ctx,
        settings_store,
        &solid.mesh_bounding_radius,
        &solid.preview_state,
    );
    render::render_export::setup_render_export_callbacks(
        ui,
        render_ctx,
        settings_store,
        &solid.mesh_bounding_radius,
    );
    // The render queue: "Add to Queue" in the export and tilt video dialogs, the Jobs
    // window and the queue controller. Needs the export's mesh radius for presets.
    crate::gui::render_jobs::setup_render_jobs(
        ui,
        db,
        render_ctx,
        settings_store,
        &solid.mesh_bounding_radius,
    );
    optics::custom_materials::setup_custom_material_callbacks(ui, render_ctx, db);
    library::clipboard::setup_copy_callbacks(ui);
    crate::gui::export_run::setup_export_run_callbacks(ui);
    tilt::tilt_profile::setup_tilt_profile_callback(ui, render_ctx, settings_store);
    tilt::tilt_hover_preview::setup_tilt_hover_preview_callback(ui, render_ctx);
}

/// Wires the library list, search, import, rename, delete, export and detail callbacks.
pub(super) fn setup_library_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    library_source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
) {
    library::diagram_list::load_filter_options_and_initial_list(ui, db);
    library::diagram_list::setup_search_and_filter_callbacks(ui, db, library_source);
    // Regenerates previews and tilt curves for the filtered set.
    library::diagram_list::setup_regenerate_filtered_set_callbacks(ui, db, library_source);
    // The Library menu's whole-catalogue counterparts.
    batch::regenerate_all::setup_regenerate_all_callbacks(ui, db, library_source);
    // The Library menu's "Plan Rough..." dialog.
    rough_plan::setup_rough_plan_callbacks(ui, db, library_source);
    library::diagram_list::setup_diagram_selection_and_export_callbacks(
        ui,
        db,
        library_source,
        render_ctx,
        preview_state,
    );
    // Tilt-performance filter rows (add/remove/clear-all) -- Rust-mediated but
    // UI-owned state.
    library::diagram_list::setup_performance_filter_callbacks(ui);
    library::local::setup_import_callback(ui, db, library_source, settings_store);
    library::local::setup_rename_callback(ui, db, library_source);
    library::local::setup_set_shape_callback(ui, db, library_source);
    library::local::setup_delete_callback(ui, db, library_source, render_ctx);
    library::local::setup_export_asc_callback(ui, db, library_source);
    library::local::setup_ignore_toggle_callback(ui, db, library_source);
    // Keeps a design out of the Rough Planner's candidates, without hiding it from the library.
    library::local::setup_planner_exclusion_toggle_callback(ui, db, library_source);
    library::local::setup_add_tag_callback(ui, db, library_source);
    library::local::setup_remove_tag_callback(ui, db, library_source);
    library::detail::setup_save_metadata_callback(ui, db, library_source, render_ctx);
    // The cutting-table's per-mode row-count pills.
    library::detail::setup_filtered_row_count_callback(ui);
}

/// Wires the Edit sub-tab's callbacks against the Solid viewport's shared handles.
pub(super) fn setup_editor(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    library_source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    solid: &SolidView,
) {
    // Bundles the Solid-viewport handles into the value `editor::setup_editor_callbacks`
    // takes (see [`SolidPickState`]).
    let solid_pick_state = SolidPickState {
        pick: Arc::clone(&solid.pick),
        hover_text: Arc::clone(&solid.hover_text),
        facet_owners: Arc::clone(&solid.facet_owners),
        geometry: Arc::clone(&solid.geometry),
    };
    editor::setup_editor_callbacks(
        ui,
        db,
        library_source,
        render_ctx,
        &solid.preview_state,
        &solid.last_solved,
        &solid_pick_state,
    );
    // The command palette (Ctrl+K) reads the same Slint state the menus and buttons do.
    commands::setup_command_palette(ui);
}

/// Persists the Solid viewport's view mode, and redraws on a mode change or a viewport
/// resize.
pub(super) fn setup_solid_view_mode_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
) {
    // Persists the mode (see `AppSettings::solid_view_mode`). It also recomputes
    // `render_ctx.tab_visible`, since the Edit tab traces live only in Path-traced and Both.
    // It re-requests a redraw at the new mode too. Otherwise switching to Diagram or Both
    // would keep whatever the previous mode left, usually the "Solve to preview" placeholder.
    let settings_store_solid_view_mode = settings_store.clone();
    let render_ctx_solid_view_mode = render_ctx.clone();
    let preview_state_solid_view_mode = Arc::clone(preview_state);
    let ui_weak_solid_view_mode = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_view_mode_changed(move |mode: i32| {
            // Before any redraw: a plan still being solved was made at the old mode, and must
            // be drawn at this one when it lands.
            preview_state_solid_view_mode
                .note_view_mode(render::camera_lighting::view_mode_index(mode));
            settings_store_solid_view_mode.update(|s| {
                s.settings.solid_view_mode =
                    u8::try_from(EditViewMode::from_raw(mode).raw()).unwrap_or_default();
            });
            if let Some(ui) = ui_weak_solid_view_mode.upgrade() {
                render::render_visibility::recompute_tab_visible(&ui, &render_ctx_solid_view_mode);
                let mut ctx = RenderContext::lock(&render_ctx_solid_view_mode);
                // Path-traced and Both restart accumulation on entry, as the Live tab does
                // below. Otherwise a finished accumulator leaves the render worker idle
                // (`bridge/render_thread/mod.rs`), and the switch shows stale pixels.
                if EditViewMode::from_raw(mode).is_traced() {
                    ctx.dirty = true;
                }
                render::camera_lighting::resubmit_at_current_pose(
                    &ui,
                    &ctx,
                    &preview_state_solid_view_mode,
                );
                drop(ctx);
            }
        });
    // Handles the debounced `viewport_resized()` signal from `solid_viewport.slint`. Without it,
    // a resize would leave the image scaled and the pick buffer misaligned until the next
    // redraw. The resubmit is deferred one event-loop turn, so it reads the size after Slint's
    // layout pass.
    let render_ctx_viewport_resized = render_ctx.clone();
    let preview_state_viewport_resized = Arc::clone(preview_state);
    let ui_weak_viewport_resized = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_viewport_resized(move || {
            let ui_weak = ui_weak_viewport_resized.clone();
            let render_ctx = render_ctx_viewport_resized.clone();
            let preview_state = Arc::clone(&preview_state_viewport_resized);
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                // A stone framed at load is framed again for the new size, before the redraw
                // reads the camera.
                render::fit_view::reframe_for_viewport(&ui, &render_ctx);
                let ctx = render_ctx
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                render::camera_lighting::resubmit_at_current_pose(&ui, &ctx, &preview_state);
            });
        });
}

/// Persists the Live Render tab's view mode and the Edit sub-tab's auto-solve budget.
pub(super) fn setup_live_view_mode_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
) {
    // Persists the Live Render view mode (see `AppSettings::live_view_mode`) and recomputes
    // `tab_visible`, as for the Solid mode above. Switching to Path-traced (`1`) restarts
    // accumulation from the current planes. Switching to Solid (`0`) shows the solid raster at
    // once.
    let settings_store_live_view_mode = settings_store.clone();
    let render_ctx_live_view_mode = render_ctx.clone();
    let preview_state_live_view_mode = Arc::clone(preview_state);
    let ui_weak_live_view_mode = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_live_view_mode_changed(move |mode: i32| {
            settings_store_live_view_mode.update(|s| {
                s.settings.live_view_mode =
                    u8::try_from(LiveViewMode::from_raw(mode).raw()).unwrap_or_default();
            });
            let Some(ui) = ui_weak_live_view_mode.upgrade() else {
                return;
            };
            render::render_visibility::recompute_tab_visible(&ui, &render_ctx_live_view_mode);
            let mut ctx = RenderContext::lock(&render_ctx_live_view_mode);
            if LiveViewMode::from_raw(mode).is_traced() {
                ctx.dirty = true;
            } else if LiveViewMode::from_raw(mode) == LiveViewMode::Solid {
                render::camera_lighting::resubmit_live_solid(
                    &ui,
                    &ctx,
                    &preview_state_live_view_mode,
                );
            }
            drop(ctx);
        });
    // Re-plans the solid frame when the Live tab's size changes. Otherwise the shared image
    // stays at the old size and Slint scales it. Only in Solid mode, since traced modes show
    // no solid raster. Debounced, so a drag re-plans once rather than per layout pass.
    let resize_debounce = slint::Timer::default();
    let render_ctx_live_resized = render_ctx.clone();
    let preview_state_live_resized = Arc::clone(preview_state);
    let ui_weak_live_resized = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_live_viewport_resized(move || {
            let ui_weak = ui_weak_live_resized.clone();
            let render_ctx = render_ctx_live_resized.clone();
            let preview_state = Arc::clone(&preview_state_live_resized);
            resize_debounce.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(50),
                move || {
                    let Some(ui) = ui_weak.upgrade() else {
                        return;
                    };
                    // As for the Edit viewport above, for a stone framed at load.
                    if ui.get_render_view_tab() == 0 {
                        render::fit_view::reframe_for_viewport(&ui, &render_ctx);
                    }
                    if ui.get_render_view_tab() != 0
                        || LiveViewMode::from_raw(ui.global::<ViewportModel>().get_live_view_mode())
                            != LiveViewMode::Solid
                    {
                        return;
                    }
                    let ctx = RenderContext::lock(&render_ctx);
                    render::camera_lighting::resubmit_live_solid(&ui, &ctx, &preview_state);
                },
            );
        });
    // Persists the Edit sub-tab's auto-solve budget (`AppSettings::editor_auto_solve_budget_ms`).
    let settings_store_auto_solve_budget = settings_store.clone();
    ui.global::<EditorModel>()
        .on_auto_solve_budget_ms_changed(move |budget_ms: i32| {
            settings_store_auto_solve_budget.update(|s| {
                s.settings.editor_auto_solve_budget_ms =
                    u32::try_from(budget_ms).unwrap_or_default();
            });
        });
}

/// Wires the remote library, the remote-rendering handoff and the batch jobs.
///
/// Returns the handoff poll timer, which the caller must keep alive.
pub(super) fn setup_remote_and_batch(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    library_source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) -> slint::Timer {
    library::remote::setup_library_source_callbacks(ui, db, library_source, settings_store);
    library::remote::setup_mirror_sync_callbacks(ui, db, settings_store);
    // Remote rendering: a repeating `slint::Timer` polls the camera and light pose. The caller
    // must keep the timer alive, since a dropped `Timer` stops firing. It is set up before
    // `setup_worker_callbacks`, which needs the `RemoteOrchestratorHandle` returned here.
    let (remote_rendering_timer, remote_orchestrator) =
        setup_remote_rendering(ui, render_ctx, settings_store);
    setup_worker_callbacks(ui, render_ctx, settings_store, &remote_orchestrator);
    // Preview thumbnails, decoded and cached off the UI thread, and the batch job that
    // generates them. The batch setup also starts the once-per-session scan for designs
    // without previews. The batch invalidates the thumbnail cache per saved design, so the
    // cache handle is passed on.
    let thumbnail_cache = batch::preview_cache::setup_preview_thumbnail_callback(ui, db);
    batch::preview::setup_preview_batch_callbacks(
        ui,
        db,
        render_ctx,
        settings_store,
        &thumbnail_cache,
    );
    // Tilt-curve batch. Unlike the preview batch, it is never scanned at startup, because
    // its cost per catalogue is much higher.
    batch::tilt::setup_tilt_batch_callbacks(ui, db, render_ctx, settings_store);
    remote_rendering_timer
}

/// Wires the window-close handling: the dirty-design guard and the render thread's
/// shutdown.
pub(super) fn setup_window_close(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    // Signals the render thread to exit on close. `running` is set to false only here, since
    // the thread cannot be restarted. Without this the render thread would outlive the window.
    //
    // Closing asks first while the Edit tab has unsaved changes, or while the rough planner
    // or a render job is at work (`close_confirm_open` in `app.slint`). The guard's save and
    // discard actions finish the shutdown.
    let render_ctx_close = render_ctx.clone();
    let settings_store_close = settings_store.clone();
    let ui_weak_close = ui.as_weak();
    ui.window().on_close_requested(move || {
        // A close the user does themselves withdraws a pending RADV-workaround restart; the
        // restart's own close request keeps it (see `radv_workaround::request_restart`).
        crate::gui::radv_workaround::note_close_request();
        let is_dirty = ui_weak_close
            .upgrade()
            .is_some_and(|ui| ui.global::<EditorModel>().get_is_dirty());
        let export_risk = ui_weak_close
            .upgrade()
            .and_then(|ui| crate::gui::export_run::work_at_risk(&ui));
        let planner_risk = joined_risk(
            joined_risk(
                rough_plan::planner_work_at_risk(),
                crate::gui::render_jobs::work_at_risk(),
            ),
            export_risk,
        );
        if !is_dirty && planner_risk.is_none() {
            RenderContext::lock(&render_ctx_close).running = false;
            // Writes now, skipping the debounce, so a change from just before quitting is kept.
            settings_store_close.flush();
            // Otherwise an open compare window would keep the process running.
            editor::close_compare_window();
            rough_plan::close_planner_window();
            crate::gui::color_pattern::close_window();
            crate::gui::render_jobs::stop_for_app_close();
            crate::gui::help::close_help_window();
            crate::gui::material_reference::close_window();
            return slint::CloseRequestResponse::HideWindow;
        }
        if let Some(ui) = ui_weak_close.upgrade() {
            ui.set_close_confirm_message(
                close_confirm_message(is_dirty, planner_risk.as_deref()).into(),
            );
            ui.set_close_confirm_can_save(is_dirty);
            ui.set_close_confirm_open(true);
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    setup_close_confirm_callbacks(ui, render_ctx, settings_store);
}

/// What the design-dirty message of the close guard says.
const DIRTY_DESIGN_MESSAGE: &str =
    "Closing the window will discard the current design's unsaved changes.";

/// The risk sentences of the work that closing the app would stop (the rough planner's and
/// the render queue's), joined with a space, or `None` when neither has any.
fn joined_risk(planner: Option<String>, render_jobs: Option<String>) -> Option<String> {
    match (planner, render_jobs) {
        (Some(planner), Some(jobs)) => Some(format!("{planner} {jobs}")),
        (one, other) => one.or(other),
    }
}

/// The message of the close guard.
///
/// It is the dirty design's sentence, the work-at-risk sentences (see
/// `rough_plan::planner_work_at_risk` and `render_jobs::work_at_risk`), or both, design first.
fn close_confirm_message(design_dirty: bool, planner_risk: Option<&str>) -> String {
    match (design_dirty, planner_risk) {
        (true, Some(risk)) => format!("{DIRTY_DESIGN_MESSAGE} {risk}"),
        (true, None) => DIRTY_DESIGN_MESSAGE.to_string(),
        (false, Some(risk)) => risk.to_string(),
        (false, None) => String::new(),
    }
}

#[cfg(test)]
mod close_message_tests {
    use super::*;

    #[test]
    fn the_guard_names_what_is_at_risk() {
        assert_eq!(
            close_confirm_message(true, None),
            "Closing the window will discard the current design's unsaved changes."
        );
        assert_eq!(
            close_confirm_message(false, Some("A rough plan is still running.")),
            "A rough plan is still running."
        );
        assert_eq!(
            close_confirm_message(true, Some("A rough plan is still running.")),
            "Closing the window will discard the current design's unsaved changes. A rough plan is still running."
        );
    }

    /// A rough plan and a render job both at risk give one message, planner first. A dirty
    /// design's sentence comes before both.
    #[test]
    fn the_guard_joins_the_planner_and_render_job_risks() {
        let joined = joined_risk(
            Some("A rough plan is still running.".to_string()),
            Some("A render job is running.".to_string()),
        );
        assert_eq!(
            joined.as_deref(),
            Some("A rough plan is still running. A render job is running.")
        );
        assert_eq!(
            joined_risk(None, Some("A render job is running.".to_string())).as_deref(),
            Some("A render job is running.")
        );
        assert_eq!(joined_risk(None, None), None);
        assert_eq!(
            close_confirm_message(true, joined.as_deref()),
            "Closing the window will discard the current design's unsaved changes. \
             A rough plan is still running. A render job is running."
        );
    }
}
