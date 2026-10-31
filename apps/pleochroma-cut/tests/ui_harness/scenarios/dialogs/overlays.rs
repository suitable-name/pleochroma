//! Scenarios: the overlay table and the main window's overlays (editor, library and data-free
//! ones), the Preferences Updates section, the update notice and the floating Parameters panel.

use super::{click_button, create_template_design, labels_like};
use crate::support::{
    Ctx, HarnessApp,
    app::{DEFAULT_SIZE, MINIMUM_SIZE, pump, wait_until},
    coverage, sample_designs,
};
use pleochroma_cut::{
    BatchModel, CommandPaletteModel, CuttingModeModel, EditorModel, HelpModel, LibraryModel,
    MainWindow, ParametersModel, PreferencesModel, RawTextModel, RenderJobsModel, RetargetModel,
    ShortcutsModel, SweepModel, TiltModel, TutorialsModel, UpdateModel, ViewportModel,
};
use slint::{
    ComponentHandle, LogicalPosition,
    platform::{PointerEventButton, WindowEvent},
};
use std::time::Duration;

// ---------------------------------------------------------------------------------------------
// The overlay table
// ---------------------------------------------------------------------------------------------

/// One overlay of the main window and how to drive it.
struct Overlay {
    /// Name used in findings.
    name: &'static str,
    /// Its normal open route (a global's `invoke_*` or, where the route runs through the library
    /// or a data-bound button, the global flag the route sets).
    open: fn(&MainWindow),
    /// Its open flag.
    is_open: fn(&MainWindow) -> bool,
    /// Closes it behind a failed test so later checks start clean.
    force_close: fn(&MainWindow),
    /// Close/cancel buttons, as label prefixes in order of preference; empty: none exists.
    close_buttons: &'static [&'static str],
}

/// An overlay that is a boolean property of a global: `get` reads it, `set(true)` opens it.
macro_rules! flag_overlay {
    ($name:expr, $model:ty, $get:ident, $set:ident, $buttons:expr) => {
        flag_overlay!($name, $model, $get, $set, $buttons, |ui: &MainWindow| ui
            .global::<$model>()
            .$set(true))
    };
    ($name:expr, $model:ty, $get:ident, $set:ident, $buttons:expr, $open:expr) => {
        Overlay {
            name: $name,
            open: $open,
            is_open: |ui: &MainWindow| ui.global::<$model>().$get(),
            force_close: |ui: &MainWindow| ui.global::<$model>().$set(false),
            close_buttons: $buttons,
        }
    };
}

/// The overlays that need a design in the editor.
fn editor_overlays() -> Vec<Overlay> {
    vec![
        flag_overlay!(
            "New Design",
            EditorModel,
            get_new_dialog_open,
            set_new_dialog_open,
            &["Cancel"]
        ),
        flag_overlay!(
            "Retarget",
            RetargetModel,
            get_is_open,
            set_is_open,
            &["Cancel", "Close"],
            |ui: &MainWindow| ui.global::<RetargetModel>().invoke_open()
        ),
        flag_overlay!(
            "Gear remap",
            EditorModel,
            get_gear_remap_open,
            set_gear_remap_open,
            &["Cancel"]
        ),
        flag_overlay!(
            "Material editor",
            ViewportModel,
            get_material_editor_open,
            set_material_editor_open,
            &["Cancel"]
        ),
        flag_overlay!(
            "Performance graph",
            TiltModel,
            get_dialog_open,
            set_dialog_open,
            &["Close"],
            |ui: &MainWindow| ui.global::<TiltModel>().invoke_toggle_dialog()
        ),
        flag_overlay!(
            "Anchor explainer",
            EditorModel,
            get_anchor_explainer_open,
            set_anchor_explainer_open,
            &["Got It"]
        ),
        flag_overlay!(
            "Edit as Text",
            RawTextModel,
            get_is_open,
            set_is_open,
            &["Close", "Cancel"],
            |ui: &MainWindow| ui.global::<RawTextModel>().invoke_open()
        ),
        flag_overlay!(
            "Angle Sweep",
            SweepModel,
            get_dialog_open,
            set_dialog_open,
            &["Close the angle sweep", "Close"],
            |ui: &MainWindow| ui.global::<SweepModel>().invoke_open_dialog()
        ),
        flag_overlay!(
            "Cutting mode",
            CuttingModeModel,
            get_open,
            set_open,
            &["Close"],
            |ui: &MainWindow| ui.global::<CuttingModeModel>().invoke_open_cutting_mode()
        ),
    ]
}

/// The overlays that are best met with a library behind them.
fn library_overlays() -> Vec<Overlay> {
    vec![
        flag_overlay!(
            "Metadata editor",
            LibraryModel,
            get_metadata_editor_open,
            set_metadata_editor_open,
            &["Cancel"]
        ),
        flag_overlay!(
            "Preview batch",
            BatchModel,
            get_preview_visible,
            set_preview_visible,
            // The confirm step (the state the opener below builds) closes with "Not Now".
            &["Not Now", "Skip", "Cancel", "Close"],
            |ui: &MainWindow| {
                let batch = ui.global::<BatchModel>();
                batch.set_preview_confirming(true);
                batch.set_preview_offer_count(2);
                batch.set_preview_visible(true);
            }
        ),
        flag_overlay!(
            "Tilt batch",
            BatchModel,
            get_tilt_visible,
            set_tilt_visible,
            &["Not Now", "Cancel", "Close"],
            |ui: &MainWindow| {
                let batch = ui.global::<BatchModel>();
                batch.set_tilt_confirming(true);
                batch.set_tilt_offer_count(2);
                batch.set_tilt_visible(true);
            }
        ),
        flag_overlay!(
            "Render Jobs",
            RenderJobsModel,
            get_panel_open,
            set_panel_open,
            &["Close the render jobs"]
        ),
    ]
}

/// The overlays that need no data at all.
fn app_overlays() -> Vec<Overlay> {
    vec![
        flag_overlay!(
            "Shortcuts",
            ShortcutsModel,
            get_open,
            set_open,
            &["Close the keyboard shortcuts"]
        ),
        flag_overlay!(
            "Preferences",
            PreferencesModel,
            get_dialog_open,
            set_dialog_open,
            &["Done"]
        ),
        // No close button by design (a search box over a list, like the editor's own pickers): it
        // closes with Esc, a click outside the card or by running a command; `run_overlay` drives
        // the first two.
        flag_overlay!(
            "Command palette",
            CommandPaletteModel,
            get_open,
            set_open,
            &[],
            |ui: &MainWindow| ui.global::<CommandPaletteModel>().invoke_open_palette()
        ),
        flag_overlay!(
            "Glossary",
            HelpModel,
            get_glossary_open,
            set_glossary_open,
            &["Close the glossary", "Close"],
            |ui: &MainWindow| ui.global::<HelpModel>().invoke_open_glossary("".into())
        ),
        flag_overlay!(
            "Tutorial browser",
            TutorialsModel,
            get_browser_open,
            set_browser_open,
            &["Close"],
            |ui: &MainWindow| ui.global::<TutorialsModel>().invoke_open_browser()
        ),
    ]
}

/// Opens `overlay` and expects its flag; a failure also closes whatever half-opened.
fn open_overlay(ctx: &mut Ctx<'_>, overlay: &Overlay) -> bool {
    let ui = ctx.app.ui().clone_strong();
    if (overlay.is_open)(&ui) {
        // The app opened it on its own (the start-up scans offer the preview and tilt batches
        // for a library without pictures). A dialog that is already up never takes the
        // keyboard again when the opener sets its flag once more, so start from closed.
        (overlay.force_close)(&ui);
        pump(200);
    }
    (overlay.open)(&ui);
    pump(250);
    let open = (overlay.is_open)(&ui);
    if !open {
        (overlay.force_close)(&ui);
    }
    ctx.expect(
        open,
        "overlay-opens",
        format!(
            "{}: its open flag is false after the normal open route",
            overlay.name
        ),
    )
}

/// After a close attempt: the flag is false, and the keyboard is back on the main scope (a
/// following Ctrl+K opens the command palette).
fn settle_closed(ctx: &mut Ctx<'_>, overlay: &Overlay, how: &str) {
    pump(200);
    let ui = ctx.app.ui().clone_strong();
    let closed = !(overlay.is_open)(&ui);
    ctx.expect(
        closed,
        "overlay-closes",
        format!("{}: still open after closing it with {how}", overlay.name),
    );
    if !closed {
        (overlay.force_close)(&ui);
        pump(200);
    }
    ctx.press("Ctrl+K");
    let palette = ui.global::<CommandPaletteModel>().get_open();
    ctx.expect(
        palette,
        "focus-returns",
        format!(
            "{}: after closing it with {how}, Ctrl+K did not open the command palette (the keyboard is not back on the main window)",
            overlay.name
        ),
    );
    ui.global::<CommandPaletteModel>().invoke_close_palette();
    pump(150);
    ctx.check_health(&format!("{} closed with {how}", overlay.name));
}

/// A pointer press and release on the backdrop in the window's bottom-left corner, which no dialog
/// card reaches (a card is centred and at least 16 px from every edge). Not the top-left corner:
/// the window's menu bar (drawn by Slint where there is no native one) sits above all content in
/// the top 40 px, so a click there lands on its first menu item.
fn click_backdrop(app: &HarnessApp) {
    let window = app.ui().window();
    let size = window.size().to_logical(window.scale_factor());
    let at = LogicalPosition::new(6.0, size.height - 6.0);
    window.dispatch_event(WindowEvent::PointerMoved { position: at });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Left,
    });
    pump(150);
}

/// Opens, sweeps, closes with Esc; opens again and closes with the close button (or, for an
/// overlay without one, with a click outside its card).
fn run_overlay(ctx: &mut Ctx<'_>, overlay: &Overlay) {
    if open_overlay(ctx, overlay) {
        ctx.sweep_main_at_sizes(overlay.name);
        ctx.press("Escape");
        settle_closed(ctx, overlay, "Esc");
    }
    if overlay.close_buttons.is_empty() {
        // No close button exists: the other close route is a click on the backdrop.
        if open_overlay(ctx, overlay) {
            click_backdrop(ctx.app);
            settle_closed(ctx, overlay, "a click outside its card");
        }
        return;
    }
    if open_overlay(ctx, overlay) {
        let view = ctx.app.main_view();
        if click_button(&view, overlay.close_buttons).is_some() {
            settle_closed(ctx, overlay, "its close button");
        } else {
            let near = labels_like(&view, &["close", "cancel", "done", "not now", "got it"]);
            ctx.expect(
                false,
                "overlay-close-button",
                format!(
                    "{}: no enabled Button labelled {:?}; similar labels: {near:?}",
                    overlay.name, overlay.close_buttons
                ),
            );
            (overlay.force_close)(ctx.app.ui());
            pump(150);
        }
    }
}

fn run_overlays(ctx: &mut Ctx<'_>, overlays: &[Overlay]) {
    for overlay in overlays {
        run_overlay(ctx, overlay);
    }
}

// ---------------------------------------------------------------------------------------------
// Overlay scenarios
// ---------------------------------------------------------------------------------------------

/// With a template design in the editor, every editor overlay (New Design, Retarget, Gear remap,
/// Material editor, Performance graph, Anchor explainer, Edit as Text, Angle Sweep, Cutting mode)
/// opens, sweeps, and closes with Esc and with its button; then the floating Parameters panel
/// (narrow window) opens, sweeps and folds away with Esc and with its button.
pub(super) fn dialogs_editor_overlays(ctx: &mut Ctx<'_>) {
    if !create_template_design(ctx) {
        return;
    }
    ctx.check_health("template design for the overlays");
    run_overlays(ctx, &editor_overlays());
    floating_parameters(ctx);
}

/// A library without pictures makes the start-up scans offer the preview and tilt batches
/// (full-window dialogs). While one is up, `MainWindow.other_overlay_open` stays true and no other
/// dialog's close hands the keyboard back (`ui/app.slint`), and the dialog sits over the whole
/// window. Waits briefly for the offers, then declines them as "Not Now" does.
fn dismiss_batch_offers(ctx: &Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    let batch = ui.global::<BatchModel>();
    let _ = wait_until(Duration::from_secs(3), || {
        batch.get_preview_visible() || batch.get_tilt_visible()
    });
    for _ in 0..3 {
        if batch.get_preview_visible() {
            batch.invoke_preview_dismiss_offer();
        }
        if batch.get_tilt_visible() {
            batch.invoke_tilt_dismiss_offer();
        }
        pump(200);
    }
}

/// With the sample library behind it, the library overlays (Metadata editor, Preview batch, Tilt
/// batch, Render Jobs) open, sweep and close with Esc and with their buttons.
pub(super) fn dialogs_library_overlays(ctx: &mut Ctx<'_>) {
    if !ctx.relaunch_with_library(&sample_designs()) {
        return;
    }
    dismiss_batch_offers(ctx);
    run_overlays(ctx, &library_overlays());
}

/// The data-free overlays (Shortcuts, Preferences, Command palette, Glossary, Tutorial browser)
/// open, sweep and close; the Preferences "Updates" section is present and shows the answered
/// "Don't Check" state; the update banner sweeps clean and hides on Later. Neither "Check Now" nor
/// "Check Daily" is ever pressed (no network request).
pub(super) fn dialogs_app_overlays(ctx: &mut Ctx<'_>) {
    run_overlays(ctx, &app_overlays());
    preferences_updates_section(ctx);
    update_banner(ctx);
}

/// Scrolls the scroll area inside the dialog `scope` with the mouse wheel (to the top first, then
/// down in steps) until a control labelled `label` is enumerable.
fn scroll_dialog_until_visible(app: &HarnessApp, scope: &str, label: &str) {
    let mut delta = 3000.0_f32;
    for _ in 0..20 {
        if app.main_view().has(label) {
            return;
        }
        let Some(dialog) = app
            .main_view()
            .root
            .query_descendants()
            .match_inherits(scope)
            .find_first()
        else {
            return;
        };
        let area = dialog
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first();
        let element = area.as_ref().unwrap_or(&dialog);
        let origin = element.absolute_position();
        let size = element.size();
        let at = LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        let window = app.ui().window();
        window.dispatch_event(WindowEvent::PointerMoved { position: at });
        window.dispatch_event(WindowEvent::PointerScrolled {
            position: at,
            delta_x: 0.0,
            delta_y: delta,
        });
        pump(100);
        // The first pass went to the top; every later one moves a screenful down.
        delta = -240.0;
    }
}

/// Preferences > Updates: the section exists, shows the answered state and is not clicked.
fn preferences_updates_section(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    ui.global::<PreferencesModel>().set_dialog_open(true);
    pump(300);
    // The Updates section sits in the middle of the dialog's scroll area: a control scrolled out of
    // a `ScrollView` is not enumerated, so the area is scrolled until the button shows.
    scroll_dialog_until_visible(ctx.app, "PreferencesDialog", "Check Now");
    let view = ctx.app.main_view();
    ctx.expect(
        view.has("Check Now"),
        "preferences-updates",
        "the Preferences Updates section has no Check Now button (not pressed here)",
    );
    let update = ui.global::<UpdateModel>();
    ctx.expect(
        !update.get_check_enabled() && !update.get_consent_pending(),
        "preferences-updates-state",
        "an existing install is seeded as Don't Check, but the daily check is on or the question pending",
    );
    ctx.expect(
        update.get_status_text().is_empty() && !update.get_checking(),
        "preferences-updates-idle",
        "a check ran or is running although nothing asked for one",
    );
    ctx.sweep_main_at_sizes("Preferences with the Updates section");
    ui.global::<PreferencesModel>().set_dialog_open(false);
    pump(150);
    ctx.check_health("Preferences Updates section");
}

/// The update notice under the header: shown by setting its model, swept, hidden by Later.
fn update_banner(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    let update = ui.global::<UpdateModel>();
    update.set_notice_text("Version 99.0.0 is available.".into());
    update.set_notice_visible(true);
    pump(250);
    let view = ctx.app.main_view();
    let shown = view.has("Later") && view.has("View Release") && view.has("Skip This Version");
    ctx.expect(
        shown,
        "update-banner-shown",
        "the update notice lacks its three buttons",
    );
    ctx.sweep_main_at_sizes("update notice");
    let _ = click_button(&ctx.app.main_view(), &["Later"]);
    pump(250);
    let hidden = !update.get_notice_visible();
    ctx.expect(
        hidden,
        "update-banner-later",
        "the update notice is still visible after Later",
    );
    if hidden {
        coverage::record_callback("UpdateModel.later");
    }
    ctx.check_health("update notice");
}

/// The Parameters panel where it floats over a narrow window: opens, sweeps, folds with Esc and
/// with a close button.
fn floating_parameters(ctx: &mut Ctx<'_>) {
    ctx.app.set_size(MINIMUM_SIZE.0, MINIMUM_SIZE.1);
    let ui = ctx.app.ui().clone_strong();
    let params = ui.global::<ParametersModel>();
    for how in ["Esc", "its close button"] {
        params.invoke_show_tab(0);
        pump(300);
        let open = params.get_open();
        ctx.expect(
            open,
            "params-open",
            "Parameters panel: not open after show_tab",
        );
        let view = ctx.app.main_view();
        ctx.sweep_view(&view, "floating Parameters panel");
        if how == "Esc" {
            ctx.press("Escape");
        } else if click_button(&view, &["Close", "Collapse", "Hide", "Fold"]).is_none() {
            let near = labels_like(&view, &["close", "collapse", "hide", "fold"]);
            ctx.expect(
                false,
                "params-close-button",
                format!("floating Parameters panel: no close button; similar labels: {near:?}"),
            );
        }
        pump(250);
        ctx.expect(
            !params.get_open(),
            "params-closes",
            format!("floating Parameters panel: still open after {how}"),
        );
        if params.get_open() {
            // A failed close must not leak into the next round.
            params.invoke_collapse();
        }
        ctx.check_health(&format!("floating Parameters panel closed with {how}"));
    }
    ctx.app.set_size(DEFAULT_SIZE.0, DEFAULT_SIZE.1);
}
