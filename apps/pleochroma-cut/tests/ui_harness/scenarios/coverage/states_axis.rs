//! States of the click-through passes for the Crystal Axis: the Stone tab's section with an axis
//! set for the stone (a sapphire, and a tanzanite with its Second Axis), the Edit view's "Crystal
//! Axis" toolbar toggle with the axis drawn, and Cutting Mode's first step with the picture of
//! the axis.
//!
//! Proves: in each state every enabled button, tab, pill and slider of the scope is pressed once
//! and every field touched (the engine of `overlays.rs`), after the state's `open` function
//! reached the page the way a user does: the Stone tab's material pick, then a preset.
//!
//! What kept the controls unreached: the section's Tilt, Azimuth and Second Axis controls exist
//! only while "Set for This Stone" is on, the color-named presets only for a material that has
//! them, the Second Axis only for a biaxial one, and the Stone tab is a `ScrollView` whose
//! scrolled-out controls are not enumerated ([`scroll_scope`]).

use super::{
    super::{Piece, existing_install},
    overlay_states::{Base, NONE, State, edit_tab, open_cutting_mode, parameters},
    state_pieces,
    states_reach::scroll_scope,
};
use crate::support::{
    HarnessApp,
    app::{pump, wait_until},
};
use pleochroma_cut::{CrystalAxisModel, EditorModel, ViewportModel};
use slint::{ComponentHandle, Model};
use std::time::Duration;

/// The pieces of this file: one `overlay:reach-axis:<state>` piece per state.
pub fn pieces() -> Vec<Piece> {
    state_pieces("reach-axis", existing_install, axis_states)
}

/// How a state turns the Crystal Axis.
#[derive(Clone, Copy)]
enum Pick {
    /// A geometry preset (0 Through the Table, 1 In the Table Plane, 2 Along the Width).
    Geometry(i32),
    /// A color-named preset, by its place in the list.
    Color(i32),
}

/// Gives the open design the material containing `wanted` (lower case), as the Stone tab's
/// Material pick does, and waits for the solve it starts. `false` when the app has no such
/// material or the design did not take it.
pub(super) fn design_material(a: &HarnessApp, wanted: &str) -> bool {
    let ui = a.ui();
    let name = ui
        .global::<ViewportModel>()
        .get_material_options()
        .iter()
        .find(|name| name.to_lowercase().contains(wanted));
    let Some(name) = name else {
        return false;
    };
    let editor = ui.global::<EditorModel>();
    if editor.invoke_apply_named_material(name, 0.0) != 0 {
        return false;
    }
    let _ = wait_until(Duration::from_secs(8), || !editor.get_solve_running());
    pump(300);
    true
}

/// Turns the axis of the open design as `pick` says (the render context and the section follow).
fn orient(a: &HarnessApp, pick: Pick) {
    let axis = a.ui().global::<CrystalAxisModel>();
    match pick {
        Pick::Geometry(index) => axis.invoke_geometry_preset_chosen(index),
        Pick::Color(index) => {
            axis.invoke_refresh();
            let wanted = usize::try_from(index).unwrap_or(0);
            let _ = wait_until(Duration::from_secs(8), || {
                axis.get_color_preset_names().row_count() > wanted
            });
            axis.invoke_color_preset_chosen(index);
        }
    }
    pump(500);
}

/// The Edit view's design in `material` with the axis turned as `pick` says.
fn oriented_design(a: &HarnessApp, material: &str, pick: Pick) {
    edit_tab(a);
    if design_material(a, material) {
        orient(a, pick);
    }
}

/// The Parameters panel on the Stone tab, scrolled by `dy` so the Crystal Axis section shows.
fn stone_tab(a: &HarnessApp, material: &str, pick: Pick, dy: f32) {
    oriented_design(a, material, pick);
    parameters(a, 0);
    scroll_scope(a, "ParametersPanel", 0.0, dy);
}

/// The Edit view with the axis drawn over a tanzanite (both handles) and the toolbar toggle on.
fn overlay_drawn(a: &HarnessApp) {
    oriented_design(a, "tanzanite", Pick::Color(0));
    a.ui().global::<CrystalAxisModel>().set_show_axis(true);
    pump(500);
}

/// Cutting Mode on its first step, for a sapphire whose axis lies in the table plane.
fn cutting_first_step(a: &HarnessApp) {
    oriented_design(a, "sapphire", Pick::Geometry(1));
    // The axis is committed to the design after the debounce; Cutting Mode reads the design.
    pump(2000);
    open_cutting_mode(a);
}

fn axis_states() -> Vec<State> {
    vec![
        State {
            name: "Parameters: Stone, crystal axis of a sapphire",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| stone_tab(a, "sapphire", Pick::Geometry(1), -900.0),
        },
        State {
            name: "Parameters: Stone, second axis of a tanzanite",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| stone_tab(a, "tanzanite", Pick::Color(0), -900.0),
        },
        State {
            name: "Parameters: Stone, second axis of a tanzanite, end",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| stone_tab(a, "tanzanite", Pick::Color(0), -4000.0),
        },
        State {
            name: "Edit view, crystal axis drawn",
            base: Base::Design,
            scope: Some("SolidViewportToolbar"),
            deny: NONE,
            open: overlay_drawn,
        },
        State {
            name: "Cutting mode, first step with the crystal axis",
            base: Base::Design,
            scope: Some("CuttingModeScreen"),
            deny: NONE,
            open: cutting_first_step,
        },
    ]
}
