//! States of the click-through passes for the popups, menus and folds of the UI redesign (round 1).
//!
//! The Live Render toolbar's Lighting menu (with a saved lighting and the save row) and its View
//! menu on a narrow view, the gestures card, the figures card folded, with Contrast and the Zones
//! (ASET-style) block, in the Simple interface and with its "How the Face-Up Figures Are Measured"
//! popover open, the problem banner; the Edit
//! viewport's Show Overlays menu below 900 px; the command bar's Solve chevron, Baseline menu and
//! the header's Design Actions Menu; the Design Checks chip; the Stone tab's Material Menu and RI
//! override; the Lighting tab's rig gallery and HDR map row; the Render tab's folding cards.
//!
//! Proves: in each state every enabled button, chip, segment and field of the scope is pressed
//! once and every field touched (the engine of `overlays.rs`), after the state's `open` function
//! reached the page the way a user does: the opener (a chip, a menu button, a fold header) is
//! pressed first, because the content of a popup or a closed fold exists only after that.
//!
//! The width states set the window narrower than the thresholds of the toolbars (the Edit toolbar
//! folds its overlay toggles into "Show Overlays" below 900 px, the Live Render toolbar folds its
//! camera buttons into "View" below 1040 px). Nothing to open for the Edit toolbar's wide layout:
//! `states_axis.rs` and `states_physics.rs` pass over its toggles.

use super::{
    super::{Piece, existing_install},
    overlay_states::{
        Base, NONE, State, deny, edit_tab, live_tab, parameters, parameters_stone_open, press,
    },
    state_pieces,
};
use crate::support::{
    HarnessApp,
    app::{pump, wait_until},
    smoke::{DenyMatch, DenyRule},
};
use pleochroma_cut::{
    DesignLightingModel, EditorModel, ParametersModel, PreferencesModel, SettingsModel,
    ViewportModel,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::time::Duration;

/// The pieces of this file: one `overlay:popups-*:<state>` piece per state.
pub fn pieces() -> Vec<Piece> {
    let groups: [super::StateGroup; 4] = [
        ("popups-live", live_states),
        ("popups-edit", edit_states),
        ("popups-stone", stone_states),
        ("popups-panels", panel_states),
    ];
    let mut all = Vec::new();
    for (group, states) in groups {
        all.extend(state_pieces(group, existing_install, states));
    }
    all
}

/// A window narrower than both toolbar thresholds (900 px for the Edit toolbar, 1040 px for the
/// Live Render toolbar), as tall as the passes' other states.
const NARROW: (u32, u32) = (860, 3200);

/// Recompute is a Solve or a Deep Solve: a long job the passes leave alone.
const DENY_RECOMPUTE: &[DenyRule] = &[deny(
    DenyMatch::Contains,
    "recompute",
    "long job: Recompute starts a solve or an Optimize search",
)];

/// Waits (real time, up to 8 s) until no solve runs, so a late refresh does not overwrite what a
/// state sets afterwards.
fn settle(a: &HarnessApp) {
    let editor = a.ui().global::<EditorModel>();
    let _ = wait_until(Duration::from_secs(8), || !editor.get_solve_running());
    pump(400);
}

/// Narrows the main window (the layout re-flows at once).
fn narrow(a: &HarnessApp) {
    a.set_size(NARROW.0, NARROW.1);
    pump(300);
}

/// Saves a lighting of the user's own and remembers it for the design, so the Lighting menu lists
/// it under SAVED with the design's star.
fn saved_lighting(a: &HarnessApp) {
    let viewport = a.ui().global::<ViewportModel>();
    viewport.invoke_save_lighting_preset("Harness lighting".into());
    let _ = wait_until(Duration::from_secs(5), || {
        viewport
            .get_lighting_presets()
            .iter()
            .any(|preset| !preset.built_in)
    });
    let design = a.ui().global::<DesignLightingModel>();
    design.invoke_use_for_design();
    let _ = wait_until(Duration::from_secs(5), || design.get_has_saved());
    pump(300);
}

/// The Live Render tab in Path-Traced view (the figures card exists only there) with the figures a
/// finished render pushes: Contrast, the four zone shares and the line that splits Dark.
fn measured_figures(a: &HarnessApp) {
    live_tab(a);
    settle(a);
    let viewport = a.ui().global::<ViewportModel>();
    viewport.set_live_view_mode(1);
    pump(400);
    viewport.set_brilliance_pct(62.0);
    viewport.set_fire_index(0.42);
    viewport.set_scintillation_pct(61.0);
    viewport.set_windowing_pct(9.0);
    viewport.set_extinction_pct(29.0);
    viewport.set_contrast(27.3);
    viewport.set_aset_shares(ModelRc::new(VecModel::from(vec![44.0, 19.0, 16.0, 21.0])));
    viewport.set_aset_split_text("Dark = Leak 9 % + Lost 12 %".into());
    pump(300);
}

// ---------------------------------------------------------------------------------------------
// Live Render
// ---------------------------------------------------------------------------------------------

fn live_states() -> Vec<State> {
    vec![
        State {
            // The chip opens the menu: SAVED (none yet), the rigs by group, Save Current Lighting,
            // Use This Lighting for This Design and All Rigs and Settings.
            name: "Live Render, Lighting menu",
            base: Base::Design,
            scope: Some("RigMenu"),
            deny: NONE,
            open: |a| {
                live_tab(a);
                settle(a);
                if !press(a, "Lighting", None) {
                    println!("states_popups: the Lighting chip was not pressed");
                }
                pump(250);
            },
        },
        State {
            // With a saved lighting (the design's own, starred) and the save-name row open.
            name: "Live Render, Lighting menu, saved lighting and save row",
            base: Base::Design,
            scope: Some("RigMenu"),
            deny: NONE,
            open: |a| {
                live_tab(a);
                settle(a);
                saved_lighting(a);
                let _ = press(a, "Lighting", None);
                pump(250);
                // The menu item keeps the name of the old toolbar button.
                let _ = press(a, "Save Lighting", None);
                pump(250);
            },
        },
        State {
            // Below 1040 px the camera buttons fold into the View menu (Reset Camera, Front, Top,
            // Turn 90 degrees); Tilt Curve and Diagnostics lose their texts.
            name: "Live Render, narrow, View menu",
            base: Base::Design,
            scope: Some("ViewMenu"),
            deny: NONE,
            open: |a| {
                live_tab(a);
                settle(a);
                narrow(a);
                if !press(a, "View", None) {
                    println!("states_popups: the View button was not pressed");
                }
                pump(250);
            },
        },
        State {
            name: "Live Render, narrow toolbar",
            base: Base::Design,
            scope: Some("GemViewportToolbarActions"),
            deny: NONE,
            open: |a| {
                live_tab(a);
                settle(a);
                a.ui().global::<ViewportModel>().set_live_view_mode(1);
                narrow(a);
            },
        },
        State {
            // The card opens by itself while the gestures were never seen; the state makes sure
            // it is open, whatever the saved flag says.
            name: "Live Render, gestures card",
            base: Base::Design,
            scope: Some("GesturesButton"),
            deny: NONE,
            open: |a| {
                live_tab(a);
                settle(a);
                if !a.main_view().has("Mouse gestures") && !press(a, "Gestures", None) {
                    println!("states_popups: the gestures button was not pressed");
                }
                pump(250);
            },
        },
        State {
            // "Fold Figures" folds the FACE-UP card to one line (ViewportModel.figures_folded_set);
            // the pass then presses its "Show Figures".
            name: "Live Render, figures card folded",
            base: Base::Design,
            scope: Some("LiveFigures"),
            deny: NONE,
            open: |a| {
                live_tab(a);
                settle(a);
                if !press(a, "Fold Figures", None) {
                    println!("states_popups: the Fold Figures button was not pressed");
                }
                pump(250);
            },
        },
        State {
            // The card with its Contrast row and Zones (ASET-style) block (Advanced interface): the
            // pass presses the card (Copy the Face-Up Figures), the ? button and Fold Figures.
            name: "Live Render, figures card with Contrast and zones",
            base: Base::Design,
            scope: Some("LiveFigures"),
            deny: NONE,
            open: measured_figures,
        },
        State {
            // The ? button opens the popover over the viewport; its link opens the manual.
            name: "Live Render, figures popover open",
            base: Base::Design,
            scope: None,
            deny: NONE,
            open: |a| {
                measured_figures(a);
                if !press(a, "How these figures are measured", None) {
                    println!("states_popups: the figures ? button was not pressed");
                    a.ui().global::<ViewportModel>().set_figures_help_open(true);
                }
                pump(300);
            },
        },
        State {
            // Contrast stays, the zones block goes.
            name: "Live Render, figures card, Simple interface",
            base: Base::Design,
            scope: Some("LiveFigures"),
            deny: NONE,
            open: |a| {
                measured_figures(a);
                let prefs = a.ui().global::<PreferencesModel>();
                prefs.set_simple_mode(true);
                prefs.invoke_simple_mode_changed(true);
                pump(300);
            },
        },
        State {
            // The picture is older than the edit: the banner says so and holds Recompute.
            name: "Live Render, problem banner, stale picture and graphics status",
            base: Base::Design,
            scope: Some("ProblemBanner"),
            deny: DENY_RECOMPUTE,
            open: |a| {
                live_tab(a);
                settle(a);
                let viewport = a.ui().global::<ViewportModel>();
                viewport.set_live_view_mode(1);
                viewport.set_trace_stale(true);
                viewport.set_gpu_status_text("Reacquiring the graphics card".into());
                pump(300);
            },
        },
    ]
}

// ---------------------------------------------------------------------------------------------
// Edit
// ---------------------------------------------------------------------------------------------

fn edit_states() -> Vec<State> {
    vec![
        State {
            // Below 900 px the overlay toggles fold into the "Show Overlays" menu; its items keep
            // the labels of the toggles.
            name: "Edit view, narrow, Show Overlays menu",
            base: Base::Design,
            scope: Some("SolidViewportToolbar"),
            deny: NONE,
            open: |a| {
                edit_tab(a);
                settle(a);
                narrow(a);
                if !press(a, "Show Overlays", None) {
                    println!("states_popups: the Show Overlays button was not pressed");
                }
                pump(250);
            },
        },
        State {
            name: "Edit view, narrow toolbar",
            base: Base::Design,
            scope: Some("SolidViewportToolbar"),
            deny: NONE,
            open: |a| {
                edit_tab(a);
                settle(a);
                narrow(a);
            },
        },
        State {
            // The chevron of the Solve split button: the Auto-Solve times and Abandon Solve.
            name: "Edit view, Solve chevron menu",
            base: Base::Design,
            scope: None,
            deny: NONE,
            open: |a| {
                edit_tab(a);
                settle(a);
                if !press(a, "Auto-Solve", None) {
                    println!("states_popups: the Auto-Solve chevron was not pressed");
                }
                pump(250);
            },
        },
        State {
            // Set Baseline and Compare are items of the Baseline menu.
            name: "Edit view, Baseline menu",
            base: Base::Design,
            scope: None,
            deny: NONE,
            open: |a| {
                edit_tab(a);
                settle(a);
                if !press(a, "Baseline", None) {
                    println!("states_popups: the Baseline button was not pressed");
                }
                pump(250);
            },
        },
        State {
            // The header's "..." menu: Export as Gem Cut Studio File, Delete This Design (with its
            // confirmation inside the menu).
            name: "Edit view, header Design Actions Menu",
            base: Base::Design,
            scope: None,
            deny: NONE,
            open: |a| {
                edit_tab(a);
                settle(a);
                if !press(a, "More Actions", None) {
                    println!("states_popups: the More Actions button was not pressed");
                }
                pump(250);
            },
        },
        State {
            // The "Design Checks" chip of the command bar opens the verdict popover.
            name: "Edit view, Design Checks chip",
            base: Base::Design,
            scope: Some("VerdictPopover"),
            deny: NONE,
            open: |a| {
                edit_tab(a);
                settle(a);
                if !press(a, "Design Checks", None) {
                    println!("states_popups: the Design Checks chip was not pressed");
                }
                pump(300);
            },
        },
    ]
}

// ---------------------------------------------------------------------------------------------
// Stone tab
// ---------------------------------------------------------------------------------------------

fn stone_states() -> Vec<State> {
    vec![
        State {
            // The identity card's "..." menu: RI Override (Advanced), New/Edit Material...
            name: "Parameters: Stone, Material Menu",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| {
                settle(a);
                parameters_stone_open(a);
                if !press(a, "Material Menu", None) {
                    println!("states_popups: the Material Menu button was not pressed");
                }
                pump(250);
            },
        },
        State {
            // The RI chip opens the override field (Advanced only).
            name: "Parameters: Stone, RI override",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| {
                settle(a);
                parameters_stone_open(a);
                let label = format!(
                    "Effective Refractive Index {}",
                    a.ui().global::<EditorModel>().get_effective_ri_text()
                );
                if !press(a, &label, None) {
                    println!("states_popups: the RI chip {label:?} was not pressed");
                }
                pump(250);
            },
        },
    ]
}

// ---------------------------------------------------------------------------------------------
// Lighting and Render tabs
// ---------------------------------------------------------------------------------------------

fn panel_states() -> Vec<State> {
    vec![
        State {
            // "Change" on the rig card opens the gallery over the tab (its accessible name is
            // "Lighting Rig"): the rigs by group, the picture of each.
            name: "Parameters: Lighting, rig gallery",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| {
                settle(a);
                parameters(a, 1);
                if !press(a, "Lighting Rig", None) {
                    println!("states_popups: the rig Change button was not pressed");
                }
                pump(250);
            },
        },
        State {
            // The Render tab's folding cards, opened: Compute, Ray-Traced View, Library Thumbnails
            // (Live View Details is open from the start).
            name: "Parameters: Render and Output, cards open",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| {
                settle(a);
                parameters(a, 2);
                for card in ["Compute", "Ray-Traced View", "Library Thumbnails"] {
                    let _ = press(a, card, None);
                }
                pump(250);
            },
        },
        State {
            // The quality presets are tiles; Custom is what an edit of a detail marks.
            name: "Parameters: Render and Output, Custom preset",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| {
                settle(a);
                parameters(a, 2);
                a.ui()
                    .global::<SettingsModel>()
                    .invoke_quality_preset_selected(3);
                pump(250);
            },
        },
        State {
            // The panel's folding cards are persisted; the state reads the bits the Stone tab
            // wrote last (a mounted tab shows its cards as saved).
            name: "Parameters: Stone, every card folded",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| {
                settle(a);
                a.ui().global::<ParametersModel>().set_stone_folds(0);
                parameters(a, 2);
                parameters(a, 0);
                pump(250);
            },
        },
    ]
}
