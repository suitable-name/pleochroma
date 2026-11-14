// `search::refresh_diagram_list` is public for a downstream binary's sync-complete handler.
mod batch;
// The command palette (Ctrl+K): one table of every action with a fuzzy search and
// availability rules -- see the module's own doc comment.
mod commands;
// Converting a startup custom-materials row read into the render context's
// material/specific-gravity lists -- see the module's own doc comment.
mod custom_material_startup;
// The `pleochroma-cut-core`-backed Edit sub-tab.
mod editor;
#[cfg(feature = "ui-harness")]
pub(crate) use editor::handles_hidden_reason;
// Hand-made Color Patterns of custom materials: the vault storage API and the Color Pattern
// Editor window -- see the module's own doc comment.
mod color_pattern;
// The Edit sub-tab's resizable-dock and collapsible-section layout persistence -- see
// the module's own doc comment.
mod editor_layout;
// The Parameters dock's persisted open/tab state -- see the module's own doc comment.
mod parameters_dock;
// "Is an export running?" for the start guards, the header export indicator's wording and the
// close guard's sentence for a dialog export -- see the module's own doc comment.
mod export_run;
// Opening the bundled user manual and revealing the Edit tab's last-saved folder --
// see the module's own doc comment.
mod external_links;
// The in-app help: the manual compiled into the program and shown in its own window, the
// glossary dialog, and the topic ids a panel's "?" button opens -- see the module's own doc
// comment.
pub mod help;
pub(crate) mod latest_worker;
mod library;
// Building and wiring the `MainWindow` and its `MainWindowHandle` RAII guard -- see the
// module's own doc comment.
mod main_window;
mod optics;
// App-wide preferences (Simple/Advanced, UI scale, high contrast, larger handles, the
// remembered Snap/Slice pills) -- see the module's own doc comment. `pub` only for
// `saved_ui_scale_factor`, which `main` calls before the first window exists.
pub mod preferences;
// The RADV shader workaround: the start-up environment step (`main` calls it first), the
// one-time question, the Preferences switch and the restart -- see the module's own doc
// comment. `pub` for `main`, which runs the start-up step and the restart after the event loop.
pub mod radv_workaround;
// Resolving a native picker's starting directory from a text field's current value --
// see the module's own doc comment.
mod picker_field;
// The shared 5x7 bitmap-font glyph lookup (from `pleochroma-solid`): the panel labels of
// `solid_preview::diagram2d` and the readout text of `tilt::video_export::overlay` use it.
pub(in crate::gui) use pleochroma_solid::pixel_font;
// Every native file and folder picker -- see the module doc comment.
mod pickers;
mod remote;
pub(crate) mod render;
// Render jobs: the frozen job <-> scene conversion, the one executor that renders a job
// (the app's queue and `pleochroma-cli render`/`tilt-video` both call it) and the public
// headless entry point -- see the module's own doc comment.
pub mod render_jobs;
// The Library menu's "Plan Rough..." dialog: up to K stones of library designs out of
// one rough block -- see the module's own doc comment.
mod rough_plan;
// Rough colour and colour zoning -- see the module's own doc comment. Kept out of `rough_plan`
// so its tests do not depend on that module's test filter.
pub mod rough_colour;
pub mod solid_preview;
// The `PreviewSink` that hands a finished solid-preview frame to the UI thread -- see the
// module's own doc comment.
mod solid_sink;
// Applying the saved ray-trace options to the render context and settings controls at start-up.
mod startup_ray;
// Start-up settings application and the settings-value <-> UI-pill-index conversions. Several
// of its items are used beyond start-up, so they are re-exported below.
mod startup_settings;
pub(crate) mod view_modes;
// A design file named on the command line (an OS double-click association starts the
// program with it as the argument) -- see the module's own doc comment.
pub mod startup_file;
mod tilt;
// Reports what happens on screen (a facet picked, a file exported, ...) to the open tutorial --
// see the module's own doc comment.
pub(crate) mod tutorial_events;
// Fits the main window to its monitor on first show -- see the module doc for why the
// .slint preferred size alone is not enough on Full HD displays.
mod window_sizing;
// The new-version check (consent, Preferences switch, notice) -- see the module's own doc
// comment.
mod update_check;
// The window-close unsaved-changes guard's Save/Discard callbacks -- see the
// module's own doc comment.
mod window_close;
// Clears the handles a previous main window left in thread-locals (the UI harness builds one
// window per scenario) -- see the module's own doc comment.
mod window_release;
// The zoning UI's host-picker switch (follows the physical color feature) -- see the module's
// own doc comment.
pub(crate) mod zoning_ui;
// A rolling, regression-based time-remaining estimator shared by the still-image
// export queue (`render::render_export::queue`) and the tilt-video export
// (`tilt::video_export::run`) -- see the module's own doc comment for why a
// regression over a window rather than an instantaneous rate.
mod progress_eta;

// Defined in `library::diagram_list`; re-exported as `gui::sync_range_bounds_to_ui`, the path
// the downstream binary's sync-complete handler and `gui::library` call.
pub use library::diagram_list::sync_range_bounds_to_ui;
// `search` lives in `gui::library`; re-exported as `gui::search`, the path that
// `search::refresh_diagram_list` documents for a downstream binary's sync-complete handler.
pub use library::search;
// Re-exported for the downstream binary that `lib.rs` describes.
pub use main_window::{MainWindowHandle, build_main_window};
// For the UI harness (`harness_support::release_window_state`); not part of the app.
#[cfg(feature = "ui-harness")]
pub(crate) use window_release::{live_holders, release_window_state};
// The secondary windows' accessors for the UI harness (`harness_support`); not part of the app.
#[cfg(feature = "ui-harness")]
pub(crate) use {
    color_pattern::{
        harness_open as harness_open_color_pattern, harness_window as harness_color_pattern_window,
    },
    editor::harness_compare_window,
    library::local::apply_measured_metadata as harness_apply_measured_metadata,
    rough_plan::{
        EXPORT_SUFFIX as ROUGH_PLAN_EXPORT_SUFFIX, HARNESS_SEED_PLAN_NAME, harness_locate_window,
        harness_planner_window, harness_rig_window, harness_seed_rough_color_plan,
        harness_view_report as harness_planner_view_report,
        harness_view_unsettled as harness_planner_view_unsettled,
    },
};
// `gui::library::search::fetch` opens a read-only connection to the same on-disk
// design library for background search -- see `db_path`'s own doc comment.
pub(in crate::gui) use main_window::db_path;
// Flat re-exports: the callers in `gui` use these as `gui::X`.
pub(in crate::gui) use startup_settings::{
    color_space_from_index, env_map_name_from_status, env_map_status_text,
    is_c_axis_override_available, local_compute_target_from_index, local_preview_scale_from_index,
    refresh_lighting_preset_options, refresh_material_options,
};
// Used by child modules as `gui::starting_dir_from_picker_field`.
use picker_field::starting_dir_from_picker_field;

use crate::MainWindow;
use slint::ComponentHandle;

/// This crate's binary entry point (`apps/pleochroma-cut/src/main.rs` calls
/// straight through to this).
///
/// # Errors
///
/// See [`run_gui`].
pub fn main() -> anyhow::Result<()> {
    run_gui()
}

/// Runs this crate's window standalone: builds it (see [`build_main_window`])
/// and runs it to completion.
///
/// # Errors
///
/// Returns an error if [`build_main_window`] or the window's own event loop
/// (`MainWindow::run`) fails -- see [`build_main_window`]'s doc comment for what that
/// covers.
pub fn run_gui() -> anyhow::Result<()> {
    let handle = build_main_window()?;
    // Whether the user has ever touched the Edit sub-tab's layout, from the settings store's
    // snapshot. `fit_initial_window_size` needs only this flag (see `window_sizing`).
    //
    // Read BEFORE `show()`: the queued layout writes of `apply_editor_layout_from_settings`
    // are flushed by `show()`, which raises `layout_changed` (and so marks the layout touched)
    // even on a fresh settings file. Read here, the flag holds the true on-disk value.
    let editor_layout_touched = handle
        .settings_store
        .snapshot()
        .settings
        .editor_layout_touched;
    // Spelled out instead of `MainWindow::run` so the monitor fit can be queued
    // between `show` and the event loop (the winit window only exists once the loop
    // turns -- see `window_sizing`).
    handle.ui.show()?;
    window_sizing::fit_initial_window_size(&handle.ui, editor_layout_touched);
    slint::run_event_loop()?;
    crate::bridge::shader_cache::cancel();
    handle.ui.hide()?;
    // A restart the user asked for (the RADV shader workaround question or the Preferences
    // switch): the window-close guard already ran and the settings are saved, so the program
    // starts again now. The new instance waits for this process to be gone, so it is started
    // after the window state is released and as late as possible.
    drop(handle);
    // Remote connections parked for reuse live in a static, which is never dropped: closed
    // here, they end cleanly instead of being cut off when the process exits.
    crate::bridge::remote::remote_render::close_idle_connections();
    if radv_workaround::take_pending_restart()
        && let Err(message) = radv_workaround::restart_process()
    {
        radv_workaround::report_restart_failure(&message);
    }
    Ok(())
}

/// Shows a toast message.
///
/// Informational/success toasts auto-dismiss after 3.5s; `"error"` and `"warning"`
/// stay on screen until the user dismisses them (via
/// `Toast.dismiss` -> `root.toast_visible = false` in `app.slint`), since a
/// critical outcome the user needed to act on could otherwise disappear before
/// they read it. `"warning"` is for a critical-but-not-failed outcome
/// (e.g. "your masts are placeholders", "the saved meet constraints were not
/// restored") that deserves the same persistence as an error without implying the
/// action itself failed; `ui/components/toast.slint`'s `Toast` component has
/// its own amber `"warning"` branch on background/border/icon color, so a
/// `"warning"` toast renders distinctly from both `"info"` and `"error"` rather
/// than falling back to the info-style default.
///
/// A plain function (not a closure) since it captures nothing from `run_gui` --
/// every `ui.on_X` callback across this module's submodules that needs it just calls
/// it directly on its own `ui: &MainWindow`. `pub` because a downstream binary
/// reusing this window needs to surface its own results through the same toast.
pub fn show_toast(ui: &MainWindow, msg: &str, toast_type: &str) {
    ui.set_toast_message(msg.into());
    ui.set_toast_type(toast_type.into());
    ui.set_toast_visible(true);

    // Every toast bumps this counter; a scheduled dismiss below only acts if it's
    // still THIS call's generation by the time it fires -- otherwise a second toast
    // shown while the first one's timer is still running would get hidden early by
    // that stale timer (see `MainWindow.toast_generation`'s own doc comment).
    let generation = ui.get_toast_generation() + 1;
    ui.set_toast_generation(generation);

    // Errors and warnings stay until the user dismisses them -- see this
    // function's own doc comment.
    if toast_type == "error" || toast_type == "warning" {
        return;
    }

    let ui_weak = ui.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(3500), move || {
        if let Some(ui) = ui_weak.upgrade()
            && ui.get_toast_generation() == generation
        {
            ui.set_toast_visible(false);
        }
    });
}
