//! Direct-invoke rows of the startup state: no design, an empty library.
//!
//! Callbacks on the allow list (`coverage_allow.rs`) have no row. Default arguments are the empty
//! value of each parameter type, except where a row gives its own value.

use super::table::{Invoke, Row, Setup};
use crate::support::app::{pump, wait_until};
use pleochroma_cut::*;
use slint::{ComponentHandle, Model};
use std::time::Duration;

/// Startup setup, Main window, shared state.
const STARTUP_MAIN_SHARED: &[Row] = rows!(MainWindow, Main, Startup, false;
    ActivityModel.invoke_progress_external(Default::default(), Default::default());
    CommandPaletteModel.invoke_activate(Default::default());
    CommandPaletteModel.invoke_query_changed(slint::SharedString::default());
    ExportRunModel.invoke_still_text(Default::default(), Default::default(), Default::default(), slint::SharedString::default());
    ExportRunModel.invoke_video_text(Default::default(), Default::default(), slint::SharedString::default());
    GuideModel.invoke_event(slint::SharedString::default());
    GuideModel.invoke_launch_tick();
    GuideModel.invoke_perform(Default::default());
    GuideModel.invoke_perform_tick();
    GuideModel.invoke_poll();
    GuideModel.invoke_step_entered();
    HelpModel.invoke_glossary_search(slint::SharedString::default());
    // The job edit and copy dialogs have no session in this state: each callback finds nothing
    // to act on and answers with nothing (a job id of 0 names no job).
    JobCopyModel.invoke_cancel();
    JobCopyModel.invoke_copy(Default::default());
    JobCopyModel.invoke_query_changed(slint::SharedString::default());
    JobCopyModel.invoke_selection_changed(Default::default());
    JobCopyModel.invoke_toggle(slint::SharedString::default());
    JobEditModel.invoke_browse_folder();
    JobEditModel.invoke_cancel();
    JobEditModel.invoke_edited();
    JobEditModel.invoke_load_again();
    JobEditModel.invoke_locate_hdr();
    JobEditModel.invoke_material_chosen(slint::SharedString::default());
    JobEditModel.invoke_rename_from_template();
    JobEditModel.invoke_requeue_answer(Default::default());
    JobEditModel.invoke_save(Default::default());
    PreferencesModel.invoke_adaptive_viewport_changed(Default::default());
    PreferencesModel.invoke_beam_instant_lighting_changed(Default::default());
    PreferencesModel.invoke_beam_viewport_changed(Default::default());
    PreferencesModel.invoke_high_contrast_changed(Default::default());
    PreferencesModel.invoke_instant_lighting_changed(Default::default());
    PreferencesModel.invoke_large_handles_changed(Default::default());
    PreferencesModel.invoke_path_cache_limit_changed(Default::default());
    PreferencesModel.invoke_path_cache_viewport_changed(Default::default());
    PreferencesModel.invoke_show_tour_again();
    PreferencesModel.invoke_simple_mode_changed(Default::default());
    PreferencesModel.invoke_ui_scale_changed(Default::default());
    // No pairing link was read: Review of an empty text offers nothing, Cancel forgets nothing,
    // and Pair finds no pending offer and returns before it connects. The "Render on <name>?"
    // question answered with No only closes.
    RemoteWorkerModel.invoke_read_pairing_link(slint::SharedString::default());
    RemoteWorkerModel.invoke_confirm_pairing();
    RemoteWorkerModel.invoke_cancel_pairing();
    RemoteWorkerModel.invoke_use_remote_answered(Default::default());
    RemoteWorkerModel.invoke_advanced_in_use(Default::default(), Default::default(), Default::default(), Default::default(), Default::default(), Default::default());
    RemoteWorkerModel.invoke_denoise_toggled(Default::default());
    RemoteWorkerModel.invoke_pick_cert_dir(slint::SharedString::default());
    RemoteWorkerModel.invoke_remote_batch_lanes_changed(Default::default());
    RemoteWorkerModel.invoke_remote_preview_batch_size_changed(Default::default());
    RemoteWorkerModel.invoke_switch_library_source(-1);
    RenderJobsModel.invoke_add_still_jobs(Default::default(), Default::default(), Default::default(), Default::default(), Default::default(), Default::default());
    RenderJobsModel.invoke_pause_job(Default::default());
    RenderJobsModel.invoke_pause_queue();
    RenderJobsModel.invoke_refresh();
    // Job id 0 names no job: edit, duplicate, copy and the job view do nothing.
    RenderJobsModel.invoke_back_to_my_view();
    RenderJobsModel.invoke_cancel_pick_view();
    RenderJobsModel.invoke_copy_to_materials(Default::default(), Default::default());
    RenderJobsModel.invoke_duplicate_job(Default::default());
    RenderJobsModel.invoke_edit_job(Default::default());
    RenderJobsModel.invoke_load_job_view(Default::default());
    RenderJobsModel.invoke_open_copy_dialog(Default::default());
    RenderJobsModel.invoke_set_waiting_compute(Default::default());
    RenderJobsModel.invoke_update_job_from_view();
    RenderJobsModel.invoke_resume_job(Default::default());
    RenderJobsModel.invoke_show_result(Default::default());
    // No design is open: the chip, the chart and the RI field have nothing to show or change.
    // `RiBatchModel.start` is on the allow list (it computes ranges for the whole library).
    RiBatchModel.invoke_cancel();
    RiBatchModel.invoke_close();
    RiChartModel.invoke_open_chart();
    RiChartModel.invoke_plot_resized();
    RiChartModel.invoke_queue_check_set();
    RiFieldModel.invoke_cancel_export_wait();
    RiFieldModel.invoke_refresh_requested();
    RiFieldModel.invoke_source_chosen(Default::default());
    RiFieldModel.invoke_typed_edited();
    RiRangeModel.invoke_chip_clicked();
    SettingsModel.invoke_advanced_in_use(Default::default(), Default::default(), Default::default(), Default::default(), Default::default(), Default::default(), Default::default());
    SettingsModel.invoke_backdrop_changed(Default::default());
    SettingsModel.invoke_backdrop_changed(1);
    SettingsModel.invoke_backdrop_changed(2);
    SettingsModel.invoke_backdrop_changed(3);
    SettingsModel.invoke_bounces_changed(Default::default());
    SettingsModel.invoke_ray_taps_selected(8);
    SettingsModel.invoke_ray_fire_selected(true);
    SettingsModel.invoke_thumbnail_renderer_selected(0);
    SettingsModel.invoke_thumbnail_renderer_selected(1);
    SettingsModel.invoke_c_axis_angles_changed(Default::default(), Default::default());
    SettingsModel.invoke_c_axis_override_changed(Default::default());
    SettingsModel.invoke_contribute_to_final_picture_changed(Default::default());
    SettingsModel.invoke_edge_rounding_changed(Default::default());
    SettingsModel.invoke_exposure_changed(Default::default());
    SettingsModel.invoke_girdle_frosted_changed(Default::default());
    SettingsModel.invoke_head_shadow_changed(Default::default());
    SettingsModel.invoke_live_compute_target_changed(Default::default());
    SettingsModel.invoke_local_compute_target_changed(Default::default());
    SettingsModel.invoke_local_preview_scale_changed(Default::default());
    SettingsModel.invoke_pick_hdr_file(slint::SharedString::default());
    SettingsModel.invoke_preview_size_changed(Default::default());
    SettingsModel.invoke_preview_spp_changed(Default::default());
    SettingsModel.invoke_resolution_changed(Default::default(), Default::default());
    SettingsModel.invoke_stone_effects_edited();
    SettingsModel.invoke_stone_face_camera();
    SettingsModel.invoke_stone_inclusion_edited(Default::default());
    SettingsModel.invoke_stone_pose_changed(Default::default(), Default::default(), Default::default(), Default::default());
    SettingsModel.invoke_stone_pose_reset();
    SettingsModel.invoke_stone_product_shot(Default::default());
    SettingsModel.invoke_stone_shadow_changed(Default::default());
    SettingsModel.invoke_stone_width_changed(Default::default());
    SettingsModel.invoke_surface_glare_changed(Default::default());
    SettingsModel.invoke_target_samples_changed(Default::default());
    SettingsModel.invoke_tray_reflection_changed(Default::default());
    ShortcutsModel.invoke_copy_markdown();
    TemplateGalleryModel.invoke_dialog_shown();
    TemplateGalleryModel.invoke_material_chosen(Default::default());
    TemplateGalleryModel.invoke_template_chosen(Default::default());
    TutorialsModel.invoke_query_changed(slint::SharedString::default());
    TutorialsModel.invoke_refresh();
    TutorialsModel.invoke_tour_templates();
    UpdateModel.invoke_consent_answered(Default::default());
    UpdateModel.invoke_enabled_changed(Default::default());
    UpdateModel.invoke_later();
    UpdateModel.invoke_skip_release();
    UpdateModel.invoke_view_release();
    Zoning.invoke_pose_option_changed(Default::default());
);

/// Startup setup, Main window, fresh state per row.
const STARTUP_MAIN_FRESH: &[Row] = rows!(MainWindow, Main, Startup, true;
    ActivityModel.invoke_cancel(Default::default());
    ActivityModel.invoke_external_cancel(Default::default());
    ActivityModel.invoke_finish_external(Default::default());
    ActivityModel.invoke_start_external(slint::SharedString::default(), slint::SharedString::default(), Default::default());
    BatchModel.invoke_preview_cancel();
    BatchModel.invoke_preview_close();
    BatchModel.invoke_preview_dismiss_offer();
    BatchModel.invoke_tilt_cancel();
    BatchModel.invoke_tilt_close();
    BatchModel.invoke_tilt_dismiss_offer();
    BatchModel.invoke_tilt_single_cancel();
    CommandPaletteModel.invoke_opened();
    GuideModel.invoke_finished(slint::SharedString::default());
    GuideModel.invoke_start_guide("new-design-walkthrough".into());
    HelpModel.invoke_open_context_help();
    HelpModel.invoke_open_glossary(slint::SharedString::default());
    HelpModel.invoke_open_topic(slint::SharedString::default());
    PreferencesModel.invoke_clear_path_cache();
    PreferencesModel.invoke_open_path_cache_folder();
    PreferencesModel.invoke_reset_tutorials();
    RemoteWorkerModel.invoke_cancel_mirror_sync();
    RemoteWorkerModel.invoke_remove_remote();
    // Opens the Compute RI Ranges dialog (the confirm step); a fresh state closes it again.
    RiBatchModel.invoke_open_batch();
    RenderJobsModel.invoke_add_tilt_video_job(Default::default());
    RenderJobsModel.invoke_cancel_job(Default::default());
    RenderJobsModel.invoke_clear_finished();
    RenderJobsModel.invoke_delete_job(Default::default());
    RenderJobsModel.invoke_export_script();
    RenderJobsModel.invoke_move_job(Default::default(), Default::default());
    RenderJobsModel.invoke_restart_job(Default::default());
    SettingsModel.invoke_clear_env_map();
    SettingsModel.invoke_load_env_map(slint::SharedString::default());
    TemplateGalleryModel.invoke_create(Default::default(), Default::default(), Default::default(), slint::SharedString::default());
    TutorialsModel.invoke_opened();
    TutorialsModel.invoke_start(slint::SharedString::default());
    TutorialsModel.invoke_tour_dismiss();
    TutorialsModel.invoke_tour_start();
    UpdateModel.invoke_consent_dismissed();
    Zoning.invoke_adopt_colour(Default::default(), Default::default());
    Zoning.invoke_open_rough_colour();
);

/// Chooses a Tilt Average Range of ±30°, checks the tilt words, then chooses ±90° again.
///
/// The words read "to ±30°" (and "to 30°" inside a sentence); option 5 puts the default back, so
/// no later row or shot sees the changed range, even when a check failed. The options, the index
/// and the two words are the covered properties.
fn tilt_range_changed(ui: &MainWindow) -> Result<(), String> {
    const NARROW: i32 = 2;
    const DEFAULT: i32 = 5;
    let prefs = ui.global::<PreferencesModel>();
    let words = ui.global::<TiltWindows>();
    let count = prefs.get_tilt_range_options().row_count();
    if count != 6 {
        return Err(format!(
            "the Tilt Average Range lists {count} options, not 6"
        ));
    }
    if prefs.get_tilt_range_index() != DEFAULT {
        return Err(format!(
            "the Tilt Average Range starts at option {}, not the default 5",
            prefs.get_tilt_range_index()
        ));
    }
    // The combo sets its own index before it reports the choice.
    prefs.set_tilt_range_index(NARROW);
    prefs.invoke_tilt_range_changed(NARROW);
    let narrowed = wait_until(Duration::from_secs(3), || {
        words.get_displayed().as_str() == "to \u{b1}30\u{b0}"
    });
    let caption = words.get_displayed().to_string();
    let plain = words.get_displayed_plain().to_string();
    prefs.set_tilt_range_index(DEFAULT);
    prefs.invoke_tilt_range_changed(DEFAULT);
    let restored = wait_until(Duration::from_secs(3), || {
        words.get_displayed().as_str() == "to \u{b1}90\u{b0}"
    });
    pump(40);
    if !narrowed {
        return Err(format!(
            "choosing +-30 left TiltWindows.displayed at {caption:?}, not \"to \u{b1}30\u{b0}\""
        ));
    }
    if plain != "to 30\u{b0}" {
        return Err(format!(
            "choosing +-30 left TiltWindows.displayed_plain at {plain:?}, not \"to 30\u{b0}\""
        ));
    }
    if !restored {
        return Err(format!(
            "choosing +-90 again left TiltWindows.displayed at {:?}",
            words.get_displayed()
        ));
    }
    Ok(())
}

/// The Material Reference window, opened on a built-in material.
fn reference_window(ui: &MainWindow) -> Result<MaterialReferenceWindow, String> {
    ui.global::<EditorModel>()
        .invoke_open_material_reference("Diamond".into());
    if !wait_until(Duration::from_secs(10), || {
        harness_support::material_reference_window().is_some()
    }) {
        return Err("the Material Reference window did not open".to_owned());
    }
    harness_support::material_reference_window()
        .ok_or_else(|| "the Material Reference window closed again".to_owned())
}

/// "Your designs that work here": a design id of 0 names no design, so the window answers
/// without a panic and stays open. The handler lives on the window's own global, which the
/// generated main-window rows cannot reach.
fn design_clicked(ui: &MainWindow) -> Result<(), String> {
    let window = reference_window(ui)?;
    window
        .global::<MaterialReferenceModel>()
        .invoke_design_clicked(0);
    pump(100);
    Ok(())
}

/// "Compute RI Ranges": the Compute RI Ranges dialog opens in the main window.
fn compute_ri_ranges(ui: &MainWindow) -> Result<(), String> {
    let window = reference_window(ui)?;
    window
        .global::<MaterialReferenceModel>()
        .invoke_compute_ri_ranges();
    if wait_until(Duration::from_secs(3), || {
        ui.global::<RiBatchModel>().get_visible()
    }) {
        // Decline the confirm step, so no batch can start.
        ui.global::<RiBatchModel>().invoke_cancel();
        pump(40);
        Ok(())
    } else {
        Err("the Compute RI Ranges dialog did not open".to_owned())
    }
}

/// Checked rows of the startup state, each on a fresh state.
const STARTUP_MAIN_CHECKED: &[Row] = checked_rows!(Startup, true;
    PreferencesModel.tilt_range_changed => tilt_range_changed;
    MaterialReferenceModel.design_clicked => design_clicked;
    MaterialReferenceModel.compute_ri_ranges => compute_ri_ranges;
);

/// Every row of this file, in run order.
pub fn rows() -> Vec<&'static Row> {
    [
        STARTUP_MAIN_SHARED,
        STARTUP_MAIN_FRESH,
        STARTUP_MAIN_CHECKED,
    ]
    .into_iter()
    .flatten()
    .collect()
}
