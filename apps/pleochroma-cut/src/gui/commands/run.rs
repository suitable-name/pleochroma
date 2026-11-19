//! What the palette commands do beyond calling one Slint callback.
//!
//! Each function here mirrors what a menu item, a button or a key arm in the `.slint` files
//! does, so a command and its button can never drift apart. Plain "invoke this callback"
//! commands are written inline in `table.rs`.
//!
//! Every function runs on the UI thread, and none of them holds a borrow of the editor state.
//! The palette calls them from its deferred run. The navigation helpers (`show_tab`,
//! `show_edit_view`, `show_tier`, `request_pane`, ...) are also called from the commit points
//! of other actions, so that one rule decides where an action takes the user: it shows the
//! area where its result or its own controls appear, and nothing moves when that is on screen
//! already.

use crate::{
    EditorModel, ExportModel, GuideModel, LibraryModel, MainWindow, ManipulateModel,
    ParametersModel, PreferencesModel, SolidPreviewModel, TuneModel, VariantsModel, ViewportModel,
};
use slint::ComponentHandle;

/// The 3D Spectral Preview tab.
pub const TAB_3D: i32 = 0;
/// The Cutting Instructions tab.
pub const TAB_CUTTING: i32 = 1;
/// The Files and Downloads tab.
pub const TAB_FILES: i32 = 2;

/// The 3D tab's Live Render view.
const VIEW_LIVE: i32 = 0;
/// The 3D tab's Edit view.
const VIEW_EDIT: i32 = 1;

/// Switches the window to `tab`, firing the same callback the tab pill does.
pub fn show_tab(ui: &MainWindow, tab: i32) {
    if ui.get_active_tab() != tab {
        ui.set_active_tab(tab);
        ui.invoke_active_tab_changed(tab);
    }
}

/// Switches the 3D tab to `view`, firing the same callback the view pill does.
///
/// Showing the Edit view also gives the viewport back to the editor's design when a library
/// stone or a job view holds it. A change of view does that through `MainWindow`'s own
/// `changed render_view_tab` handler; when the view is Edit already (coming back from the
/// Cutting Instructions or Files tab, or after a library pick made in Edit) nothing changes,
/// so the hand-back is asked for here ([`reclaim_viewport_soon`]).
fn show_view(ui: &MainWindow, view: i32) {
    show_tab(ui, TAB_3D);
    if ui.get_render_view_tab() != view {
        ui.set_render_view_tab(view);
        ui.invoke_render_view_tab_changed(view);
    } else if view == VIEW_EDIT {
        reclaim_viewport_soon(ui);
    }
}

/// Fires `MainWindow.edit_view_entered` on the next turn of the event loop, if the Edit view
/// is still the one on screen then. Its handler does nothing when the editor's design already
/// owns the viewport or no design is open (`reclaim_viewport_for_editor`).
///
/// Not fired at once: the handler borrows the editor state, and the commit points that show
/// the Edit view (a design landing, an applied text) may still hold it. One turn later is
/// also when the `changed render_view_tab` handler runs for a real change of view.
fn reclaim_viewport_soon(ui: &MainWindow) {
    let weak = ui.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        if let Some(ui) = weak.upgrade()
            && edit_view_showing(&ui)
        {
            ui.invoke_edit_view_entered();
        }
    });
}

/// Whether the 3D tab's Edit view is the page on screen.
#[must_use]
pub fn edit_view_showing(ui: &MainWindow) -> bool {
    ui.get_active_tab() == TAB_3D && ui.get_render_view_tab() == VIEW_EDIT
}

/// Shows the Edit view of the 3D tab, where the tier table, inspector and viewport live.
pub fn show_edit_view(ui: &MainWindow) {
    show_view(ui, VIEW_EDIT);
}

/// The 3D view of the Edit view's short one-pane layout (`EditorModel.request_pane`).
pub const PANE_3D: i32 = 0;
/// The tier editor (design settings, tier table, inspector) of that layout.
pub const PANE_TIER_EDITOR: i32 = 1;

/// Asks the Edit view for `pane` ([`PANE_3D`], [`PANE_TIER_EDITOR`]). Only the short one-pane
/// layout acts on it; a guide step that points into a pane wins (`Tab3dContent`).
pub fn request_pane(ui: &MainWindow, pane: i32) {
    ui.global::<EditorModel>().invoke_request_pane(pane);
}

/// Shows the Edit view with the tier at `index` selected and loaded in the Tier form: the
/// Tier tab, the inspector and the tier table unfolded, the tier editor pane. While a guide
/// step locks the Tier form the tier is only selected (`EditorModel.show_tier`).
pub fn show_tier(ui: &MainWindow, index: i32) {
    show_edit_view(ui);
    let guide = ui.global::<GuideModel>();
    ui.global::<EditorModel>().invoke_show_tier(
        index,
        guide.invoke_allows_tier_form(),
        guide.invoke_allows_tier_table(),
    );
}

/// Unfolds the tier table and asks for the tier editor pane: where a change to the tier list
/// shows.
fn show_tier_table(ui: &MainWindow) {
    ui.global::<EditorModel>().set_tier_table_collapsed(false);
    request_pane(ui, PANE_TIER_EDITOR);
}

/// Shows the Live Render view of the 3D tab.
pub fn show_live_render_view(ui: &MainWindow) {
    show_view(ui, VIEW_LIVE);
}

/// Shows the Live Render view of the 3D tab and fires the view pill's callback even when
/// the view is already the current one: the start-up view, where Live Render is the
/// property's default and the handlers must still hear it once.
pub fn enter_live_render_view(ui: &MainWindow) {
    show_tab(ui, TAB_3D);
    ui.set_render_view_tab(VIEW_LIVE);
    ui.invoke_render_view_tab_changed(VIEW_LIVE);
}

/// Opens the inspector's Tier tab ready to author a new tier.
pub fn add_tier(ui: &MainWindow) {
    show_edit_view(ui);
    let editor = ui.global::<EditorModel>();
    editor.set_selected_tier_index(-1);
    // The Tier tab with the inspector unfolded, the tier table (the new tier appears there)
    // and the tier editor pane: the same reveal as every "+ Add Tier" button.
    editor.invoke_show_tier_form();
}

/// Shows inspector tab `tab` (0 Tier, 1 Preform, 2 Optimize, 4 History; 3 was the Schedule tab, now the Cutting Instructions page), expanded, in the tier editor pane.
pub fn show_inspector_tab(ui: &MainWindow, tab: i32) {
    show_edit_view(ui);
    let editor = ui.global::<EditorModel>();
    editor.set_inspector_tab(tab);
    editor.set_inspector_collapsed(false);
    request_pane(ui, PANE_TIER_EDITOR);
}

/// `EditorModel.inspector_tab` of the Optimize tab.
const OPTIMIZE_TAB: i32 = 2;

/// Puts the Optimize tab on its Search view (weights, status, candidates), like its "Search"
/// segment. Does nothing when Search is showing already.
pub fn show_optimize_search_view(ui: &MainWindow) {
    let tune = ui.global::<TuneModel>();
    if tune.get_active() {
        tune.set_active(false);
        tune.invoke_mode_changed();
    }
}

/// `EditorModel.inspector_tab` of the History tab, which hosts the Variants view.
const HISTORY_TAB: i32 = 4;

/// Shows the History tab's Variants view (the list of saved variants), expanded.
pub fn show_variants(ui: &MainWindow) {
    show_inspector_tab(ui, HISTORY_TAB);
    ui.global::<VariantsModel>().set_view(1);
}

/// Opens the form that saves the open design as a variant, like the Variants view's
/// "Save as variant..." button.
pub fn save_variant(ui: &MainWindow) {
    show_variants(ui);
    ui.global::<VariantsModel>().invoke_begin_save(-1);
}

/// Duplicates the selected tier, like the row menu's Duplicate. The tier table is shown, since
/// the copy appears there.
pub fn duplicate_selected_tier(ui: &MainWindow) {
    let editor = ui.global::<EditorModel>();
    let index = editor.get_selected_tier_index();
    if index >= 0 {
        editor.invoke_duplicate_tier(index);
        show_tier_table(ui);
    }
}

/// Removes the selected tier, like the row menu's Delete. The tier table is shown.
pub fn delete_selected_tier(ui: &MainWindow) {
    let editor = ui.global::<EditorModel>();
    let index = editor.get_selected_tier_index();
    if index >= 0 {
        editor.invoke_remove_tier(index);
        show_tier_table(ui);
    }
}

/// Moves the selected tier by `step` places in cutting order (-1 earlier, 1 later). The tier
/// table is shown: the row's new place is the only sign of the move.
pub fn move_selected_tier(ui: &MainWindow, step: i32) {
    let editor = ui.global::<EditorModel>();
    let index = editor.get_selected_tier_index();
    if index >= 0 {
        editor.invoke_move_tier(index, step);
        show_tier_table(ui);
    }
}

/// Clears the tier selection, like Escape does.
pub fn clear_tier_selection(ui: &MainWindow) {
    ui.global::<EditorModel>().set_selected_tier_index(-1);
}

/// Moves the keyboard focus to the tier filter box, like Ctrl+F on the Edit view: the tier
/// table unfolded, the tier editor pane, then the focus pulse (`EditorModel.focus_tier_filter`,
/// the function the key calls).
pub fn focus_tier_filter(ui: &MainWindow) {
    ui.global::<EditorModel>().invoke_focus_tier_filter();
}

/// Starts Optimize with the weights currently typed into the Optimize tab, like its button.
/// The tab's Search view is shown first, so the weights the run uses and its progress are on
/// screen.
pub fn optimize(ui: &MainWindow) {
    show_inspector_tab(ui, OPTIMIZE_TAB);
    show_optimize_search_view(ui);
    let editor = ui.global::<EditorModel>();
    editor.invoke_optimize(
        editor.get_optimize_weight_windowing(),
        editor.get_optimize_weight_extinction(),
        editor.get_optimize_weight_tilt_brilliance(),
        editor.get_optimize_weight_yield(),
        editor.get_optimize_weight_tone(),
    );
}

/// Sets the Edit view's viewport mode (0 Solid, 1 Path-Traced, 2 Both, 3 Diagram, 4 Ray-Traced).
pub fn set_solid_view_mode(ui: &MainWindow, mode: i32) {
    show_edit_view(ui);
    request_pane(ui, PANE_3D);
    ui.global::<SolidPreviewModel>()
        .set_view_mode(crate::gui::view_modes::EditViewMode::from_raw(mode).raw());
}

/// Sets the Live Render view's mode (0 Solid, 1 Path-Traced, 2 Ray-Traced).
pub fn set_live_view_mode(ui: &MainWindow, mode: i32) {
    show_live_render_view(ui);
    ui.global::<ViewportModel>()
        .set_live_view_mode(crate::gui::view_modes::LiveViewMode::from_raw(mode).raw());
}

/// Turns the Solid viewport's Slice tool on or off, like its pill and the S key.
pub fn toggle_slice_mode(ui: &MainWindow) {
    show_edit_view(ui);
    request_pane(ui, PANE_3D);
    let manipulate = ui.global::<ManipulateModel>();
    manipulate.set_slice_mode(!manipulate.get_slice_mode());
    manipulate.invoke_slice_toggled();
}

/// Turns the Solid viewport's pick mode for the selected tier's meet corner on or off, like its
/// pill and the M key. Arming brings the 3D view up itself, for every entry point
/// (`gui::editor::manipulate::meet_pick`).
pub fn toggle_pick_meet(ui: &MainWindow) {
    show_edit_view(ui);
    let manipulate = ui.global::<ManipulateModel>();
    manipulate.set_pick_mode(!manipulate.get_pick_mode());
    manipulate.invoke_pick_toggled();
}

/// Turns snapping of the angle and depth handles on or off, like the Snap pill. The pill is in
/// the 3D view, so that pane is shown.
pub fn toggle_snap(ui: &MainWindow) {
    if edit_view_showing(ui) {
        request_pane(ui, PANE_3D);
    }
    let manipulate = ui.global::<ManipulateModel>();
    manipulate.set_snap_off(!manipulate.get_snap_off());
    manipulate.invoke_snap_toggled();
}

/// Collapses or expands the library panel, like its rail toggle.
pub fn toggle_library_panel(ui: &MainWindow) {
    let library = ui.global::<LibraryModel>();
    let collapsed = !library.get_panel_collapsed();
    library.set_panel_collapsed(collapsed);
    library.invoke_panel_collapsed_changed(collapsed);
}

/// Switches between the Simple and Advanced interface, like the header pill.
pub fn set_simple_interface(ui: &MainWindow, simple: bool) {
    let preferences = ui.global::<PreferencesModel>();
    preferences.set_simple_mode(simple);
    preferences.invoke_simple_mode_changed(simple);
}

/// `ParametersModel.active_tab` of the Stone tab.
const PARAMETERS_STONE: i32 = 0;
/// `ParametersModel.active_tab` of the Lighting & Environment tab.
const PARAMETERS_LIGHTING: i32 = 1;
/// `ParametersModel.active_tab` of the Render & Output tab.
const PARAMETERS_RENDER: i32 = 2;

/// Brings the 3D tab forward and shows tab `tab` of the Parameters dock, expanded.
///
/// Like the View menu items and the dock's toolbar buttons. The Live Render / Edit choice is
/// left as it is, since the dock is in both views.
fn show_parameters_tab(ui: &MainWindow, tab: i32) {
    show_tab(ui, TAB_3D);
    ui.global::<ParametersModel>().invoke_show_tab(tab);
}

/// Opens the Parameters dock on its Stone tab.
pub fn open_stone_properties(ui: &MainWindow) {
    show_parameters_tab(ui, PARAMETERS_STONE);
}

/// Opens the Parameters dock on its Lighting & Environment tab.
pub fn open_lighting_and_environment(ui: &MainWindow) {
    show_parameters_tab(ui, PARAMETERS_LIGHTING);
}

/// Opens the Parameters dock on its Render & Output tab. The "Render Settings..." command
/// runs the same action.
pub fn open_render_and_output(ui: &MainWindow) {
    show_parameters_tab(ui, PARAMETERS_RENDER);
}

/// Opens the high-resolution export dialog. It floats over every page, has its own preview
/// and its result is a file, so opening it does not move the user: the same as its buttons
/// (Live Render toolbar, Render & Output tab, Files tab) and the header's export chip.
pub fn open_render_export(ui: &MainWindow) {
    ui.global::<ExportModel>().set_is_open(true);
}

/// Unfolds the library panel, where designs that were just imported are listed. Only for now:
/// the saved choice is left alone (no `panel_collapsed_changed`), like the empty state's
/// "Start from a Library Design" card (`TutorialsModel.show_library`).
pub fn show_library_panel(ui: &MainWindow) {
    ui.global::<LibraryModel>().set_panel_collapsed(false);
}

/// "Back to My View" from the palette. The command is offered on every page while a job's view
/// shows, and what it changes is the Live Render picture, so that view is shown first.
pub fn back_to_my_view(ui: &MainWindow) {
    show_live_render_view(ui);
    ui.global::<crate::RenderJobsModel>()
        .invoke_back_to_my_view();
}

/// "Update Job from This View" from the palette. "This view" is the Live Render picture, so it
/// is shown first: the cutter sees what the job takes over.
pub fn update_job_from_view(ui: &MainWindow) {
    show_live_render_view(ui);
    ui.global::<crate::RenderJobsModel>()
        .invoke_update_job_from_view();
}
