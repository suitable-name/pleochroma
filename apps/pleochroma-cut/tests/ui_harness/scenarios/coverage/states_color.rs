//! States of the click-through passes for the color variants: the Stone tab's Body Color group on a
//! sapphire (variants, no generic colors), with the per-direction Custom editor open (two rows on a
//! sapphire, three on a tanzanite, with the "Save as Variant..." name row), with an old single
//! color (the legacy note, "Convert..." and "Nearest Variant") and with a saved variant that
//! changed (the "Update" line); the material editor's Color Variants section with a saved copy
//! (plain, renaming, asking before a delete); and the Color pill's popup on a library design shown as
//! a sapphire, plain and with a typical zoning of its own ("Edit Zones..." and "Remove Zoning" are
//! offered); the popup with an old single color has no pill to press (see `legacy_popup`). The
//! Stone tab is also shown on a sapphire design with a typical zoning, where "Reset to Material
//! Pattern" is offered.
//!
//! Proves: in each state every enabled button, chip and field of the scope is pressed once and
//! every field touched (the engine of `overlays.rs`), after the state's `open` function reached the
//! page the way a user does: the Stone tab's material pick, then the Custom row, a name or a click
//! on the control that reveals the next row. A control that only exists after another press (the
//! name row's Save and Cancel, the rename row, the delete question) is shown by the state's own
//! press, because the engine presses each control once per restore.

use super::{
    super::{Piece, existing_install},
    overlay_states::{Base, NONE, State, edit_tab, live_tab, parameters, press},
    rows_color::{
        design_on, legacy_on_sapphire, library_sapphire, open_editor, zoned_library_sapphire,
        zoned_sapphire,
    },
    state_pieces,
    states_dialogs::material_open,
    states_reach::scroll_scope,
};
use crate::support::{
    HarnessApp,
    app::{pump, wait_until},
};
use pleochroma_cut::{BodyColorEditorModel, ColorVariantsModel, EditorModel, ViewportModel};
use slint::{ComponentHandle, Model};
use std::time::Duration;

/// The pieces of this file: one `overlay:reach-color:<state>` piece per state.
pub fn pieces() -> Vec<Piece> {
    state_pieces("reach-color", existing_install, color_states)
}

/// Waits (real time, up to 30 s) until the color editor finished its solve.
fn editor_solved(a: &HarnessApp) -> bool {
    let colour = a.ui().global::<BodyColorEditorModel>();
    wait_until(Duration::from_secs(30), || {
        colour.get_has_solution() && !colour.get_busy()
    })
}

/// The design on `material`, shown in the Live Render view with the Parameters dock on its Stone
/// tab. A step that fails leaves the state as far as it got: the pass then reports what is missing.
fn stone_tab(a: &HarnessApp, material: &str) {
    if let Err(message) = design_on(a.ui(), material) {
        println!("states_color: {message}");
    }
    parameters(a, 0);
}

/// The Stone tab with the combo on its Custom row (the last entry) and the editor open, the way a
/// user opens it: the button under the combo ("Matching..." while a solve runs).
fn custom_editor(a: &HarnessApp, material: &str) {
    stone_tab(a, material);
    let editor = a.ui().global::<EditorModel>();
    let last = i32::try_from(editor.get_body_color_options().row_count()).unwrap_or(1) - 1;
    editor.set_body_color_index(last.max(0));
    pump(250);
    if !press(a, "Choose Color...", None) {
        let _ = press(a, "Matching...", None);
    }
    let _ = editor_solved(a);
    pump(250);
}

/// The design on a sapphire whose stone has a typical zoning of its own (the Stone tab's "Reset to
/// Material Pattern" and the color popup's "Remove Zoning" are offered). A step that fails leaves
/// the state as far as it got: the pass then reports what is missing.
fn zoned_stone(a: &HarnessApp) {
    if let Err(message) = zoned_sapphire(a.ui()) {
        println!("states_color: {message}");
    }
}

/// The design on a sapphire that holds a saved variant of its own (made in the editor without the
/// Stone tab, so the tab's editor is closed afterwards), shown on the Stone tab.
fn saved_variant(a: &HarnessApp, name: &str) -> bool {
    if open_editor(a.ui(), "sapphire").is_err() {
        return false;
    }
    let saved = a
        .ui()
        .global::<BodyColorEditorModel>()
        .invoke_save_variant_requested(name.into());
    pump(400);
    parameters(a, 0);
    saved
}

/// The material editor, scrolled to its end where the Color Variants section is, with a saved copy
/// of the first variant of Sapphire in it (so Rename and Delete are offered).
fn variants_section(a: &HarnessApp) {
    edit_tab(a);
    let _ = design_on(a.ui(), "sapphire");
    material_open(a);
    let colors = a.ui().global::<ColorVariantsModel>();
    // The section opens on the material of the live render; make sure it is the sapphire.
    colors.invoke_opened("Sapphire".into());
    pump(200);
    // Row 0 is the material's own color, row 1 the first variant of the data file.
    let first_variant = i32::from(colors.get_rows().row_count() > 1);
    colors.invoke_duplicate_requested(first_variant);
    pump(300);
    scroll_scope(a, "MaterialEditorDialog", 0.0, -4000.0);
}

/// The Live Render view of the design on a sapphire with an old single color, with the Color pill's
/// popup open. A design in the editor has the Body Color group on its Stone tab, not the pill: the
/// press finds nothing and the state reports it (an old single color exists only on a design).
fn legacy_popup(a: &HarnessApp) {
    edit_tab(a);
    if let Err(message) = legacy_on_sapphire(a.ui()) {
        println!("states_color: {message}");
    }
    open_color_pill(a);
}

/// The Stone tab of a library design shown as a sapphire (zoned with its first typical pattern when
/// `zoned`) with the Color pill's popup open. The pill is the Stone tab's color control of a
/// library design (and of a material preview): a design in the editor has the Body Color group
/// instead, so its Live Render view has no pill to press.
fn library_popup(a: &HarnessApp, zoned: bool) {
    parameters(a, 0);
    let made = if zoned {
        zoned_library_sapphire(a.ui())
    } else {
        library_sapphire(a.ui())
    };
    if let Err(message) = made {
        println!("states_color: {message}");
    }
    open_color_pill(a);
}

/// The Live Render view of the design with the Color pill's popup open.
fn open_color_pill(a: &HarnessApp) {
    live_tab(a);
    let viewport = a.ui().global::<ViewportModel>();
    viewport.invoke_render_color_refresh();
    pump(300);
    let label = format!("Color: {}", viewport.get_render_color_summary());
    if !press(a, &label, None) {
        println!("states_color: the Color pill {label:?} was not pressed");
    }
    pump(300);
}

fn color_states() -> Vec<State> {
    let mut states = stone_tab_states();
    states.extend(editor_and_popup_states());
    states
}

/// The Stone tab of a design in the editor: its Body Color group in every shape.
fn stone_tab_states() -> Vec<State> {
    vec![
        State {
            name: "Parameters: Stone, color variants of a sapphire",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| stone_tab(a, "sapphire"),
        },
        State {
            name: "Parameters: Stone, generic colors of a diamond",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| stone_tab(a, "diamond"),
        },
        State {
            name: "Parameters: Stone, Custom color of a sapphire, name row",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| {
                custom_editor(a, "sapphire");
                // The name row (its field, Save and Cancel) opens on the suggested name.
                press(a, "Save as Variant...", None);
                pump(250);
            },
        },
        State {
            name: "Parameters: Stone, Custom color of a tanzanite",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| custom_editor(a, "tanzanite"),
        },
        State {
            name: "Parameters: Stone, legacy color on a sapphire",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| {
                edit_tab(a);
                if let Err(message) = legacy_on_sapphire(a.ui()) {
                    println!("states_color: {message}");
                }
                parameters(a, 0);
            },
        },
        State {
            // A typical zoning on the stone: "Reset to Material Pattern" is offered (each control is
            // pressed in a fresh restore, so the Color Pattern window the neighbouring button
            // opens does not disable it).
            name: "Parameters: Stone, sapphire with its own zoning",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| {
                zoned_stone(a);
                parameters(a, 0);
            },
        },
        State {
            name: "Parameters: Stone, saved variant changed",
            base: Base::Design,
            scope: Some("ParametersPanel"),
            deny: NONE,
            open: |a| {
                if !saved_variant(a, "Harness Variant") {
                    println!("states_color: the saved variant was not made");
                }
                // A saved variant edited elsewhere makes the design's copy stale; Rust raises the
                // flag when the stored numbers (or the name) differ, and no control of the window
                // can change a stored variant, so the flag is raised the way Rust does. This
                // state therefore only shows and presses the "Variant Changed: Update" line; the
                // Update path itself (the copy, the label refresh) is unit-tested in
                // `render_color/color_variants.rs` and `body_color_combo.rs`.
                a.ui()
                    .global::<EditorModel>()
                    .set_body_color_variant_stale(true);
                pump(250);
            },
        },
    ]
}

/// The Gem Material Editor's Color Variants section and the Live Render color popup.
fn editor_and_popup_states() -> Vec<State> {
    vec![
        State {
            name: "Material editor: Color Variants of a sapphire, end",
            base: Base::Design,
            scope: Some("MaterialEditorDialog"),
            deny: NONE,
            open: variants_section,
        },
        State {
            name: "Material editor: Color Variants, renaming a saved copy",
            base: Base::Design,
            scope: Some("MaterialEditorDialog"),
            deny: NONE,
            open: |a| {
                variants_section(a);
                press(a, "Rename", None);
                pump(250);
            },
        },
        State {
            name: "Material editor: Color Variants, delete question",
            base: Base::Design,
            scope: Some("MaterialEditorDialog"),
            deny: NONE,
            open: |a| {
                variants_section(a);
                // The section comes before the dialog's own Delete button in the tree, and the
                // dialog's is disabled for a built-in material.
                press(a, "Delete", None);
                pump(250);
            },
        },
        State {
            // The popup's rows are not under a component the query can root at: the pass takes the
            // controls the open function added to the window.
            name: "Live Render color popup of a sapphire",
            base: Base::Library,
            scope: Some("RenderColorControl"),
            deny: NONE,
            open: |a| library_popup(a, false),
        },
        State {
            name: "Live Render color popup, legacy color on a sapphire",
            base: Base::Design,
            scope: Some("RenderColorPopup"),
            deny: NONE,
            open: legacy_popup,
        },
        State {
            // Color Zoning of a zoned stone: the typical patterns, "Edit Zones..." and "Remove
            // Zoning" (offered only while the stone has a pattern of its own).
            name: "Live Render color popup, sapphire with its own zoning",
            base: Base::Library,
            scope: Some("RenderColorControl"),
            deny: NONE,
            open: |a| library_popup(a, true),
        },
    ]
}
