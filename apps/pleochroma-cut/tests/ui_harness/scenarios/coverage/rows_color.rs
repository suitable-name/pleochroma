//! Hand-written rows of the direct-invoke table for the color variants: the callbacks of the
//! Stone tab's color combo, the per-direction Custom color editor, "Save as Variant...", the Color
//! Variants section of the material editor and the Live Render color popup, called from a state
//! where they apply and with an asserted effect.
//!
//! Like `rows_checked.rs`, a row here replaces the generated row with the same key (`all_rows` in
//! `coverage.rs`); the callbacks with no generated row (the new ones) have only this one. Every
//! row works on the first fixture design in the editor (the Editor group) and changes what it
//! needs to: the material of the design (a sapphire for the pleochroic rows, a diamond for a color
//! it keeps from before), then the callback itself. The throw-away profile's vault holds the
//! saved variants and is wiped with every relaunch of the group.
//!
//! The Color Pattern window's callbacks (`ColorPatternModel.color_form_changed`,
//! `color_source_changed`, `variant_picked`, `pick_color`) are on the allow list and are driven by
//! the `color_pattern_window` scenario instead (`scenarios/secondary_windows.rs`): that window has
//! a global instance of its own, which no row of this table reaches.

use super::{
    rows_checked::{clear_toast, toast_error},
    table::{Invoke, Row, Setup},
};
use crate::support::app::{pump, wait_until};
use pleochroma_cut::*;
use slint::{ComponentHandle, Model};
use std::time::Duration;

// ---------------------------------------------------------------------------------------------
// Helpers (the states of `states_color.rs` use them too)
// ---------------------------------------------------------------------------------------------

/// A row position as the `int` of a callback.
fn at(index: usize) -> i32 {
    i32::try_from(index).unwrap_or(i32::MAX)
}

/// The Edit view over the open design, so the Stone tab's controls apply to it.
pub(super) fn edit_view(ui: &MainWindow) {
    ui.set_active_tab(0);
    ui.invoke_active_tab_changed(0);
    ui.set_render_view_tab(1);
    ui.invoke_render_view_tab_changed(1);
    pump(150);
}

/// The place and name of the option called `wanted` (letter case ignored), else the first option
/// that contains it.
fn option_named(options: impl Iterator<Item = String>, wanted: &str) -> Option<(usize, String)> {
    let options: Vec<String> = options.collect();
    let low = wanted.to_lowercase();
    let place = options
        .iter()
        .position(|option| option.to_lowercase() == low)
        .or_else(|| {
            options
                .iter()
                .position(|option| option.to_lowercase().contains(&low))
        })?;
    Some((place, options.get(place)?.clone()))
}

/// Gives the open design the built-in material called `wanted` (the Stone tab's Material pick) and
/// waits for the solve it starts.
///
/// # Errors
///
/// The app has no such material, or the design did not take it.
pub(super) fn design_on(ui: &MainWindow, wanted: &str) -> Result<(), String> {
    edit_view(ui);
    let viewport = ui.global::<ViewportModel>();
    let _ = wait_until(Duration::from_secs(5), || {
        viewport.get_shows_editor_design()
    });
    let (_, name) = option_named(
        viewport.get_material_options().iter().map(String::from),
        wanted,
    )
    .ok_or_else(|| format!("the app offers no material called {wanted:?}"))?;
    let editor = ui.global::<EditorModel>();
    let code = editor.invoke_apply_named_material(name.as_str().into(), 0.0);
    if code != 0 {
        return Err(format!("the design did not take {name} (answer {code})"));
    }
    let _ = wait_until(Duration::from_secs(8), || !editor.get_solve_running());
    pump(300);
    Ok(())
}

/// The design on `material` with the color editor open on it (the Stone tab's Custom row opens it
/// the same way: target 0, then `open_requested`), once every direction's first solve landed.
///
/// # Errors
///
/// As [`design_on`], or the editor never finished its first solve.
pub(super) fn open_editor(ui: &MainWindow, material: &str) -> Result<(), String> {
    design_on(ui, material)?;
    let colour = ui.global::<BodyColorEditorModel>();
    colour.set_target(0);
    colour.invoke_open_requested();
    if wait_until(Duration::from_secs(30), || {
        colour.get_has_solution() && !colour.get_busy()
    }) {
        Ok(())
    } else {
        Err("the color editor never finished its first solve".to_owned())
    }
}

/// The design on a sapphire with a one-color preset: the old single color an earlier version
/// saved, which the Stone tab reports as "Legacy Color: No Pleochroism". The preset is picked on a
/// diamond (a stone that looks the same in every direction lists the nine generic colors) and
/// carried to the sapphire by one Apply, as the Stone tab's combos do.
///
/// # Errors
///
/// A step did not work: the diamond's list has no preset, the app has no sapphire, or the note
/// did not show.
pub(super) fn legacy_on_sapphire(ui: &MainWindow) -> Result<(), String> {
    design_on(ui, "diamond")?;
    let editor = ui.global::<EditorModel>();
    // [Material Default, the nine generic colors, Custom]: the third color is the one to find.
    let presets = pleochroma_editor::material::body_color_options();
    let wanted = presets.get(3).ok_or("the editor knows no generic color")?;
    let row = editor
        .get_body_color_options()
        .iter()
        .position(|option| option.as_str() == wanted.as_str())
        .ok_or_else(|| format!("a diamond's Body Color list does not offer {wanted:?}"))?;
    let (sapphire, _) = option_named(
        editor.get_material_combo_options().iter().map(String::from),
        "sapphire",
    )
    .ok_or("the Material list has no sapphire")?;
    editor.invoke_apply_design_material(at(sapphire), "".into(), at(row));
    if wait_until(Duration::from_secs(5), || {
        editor.get_body_color_legacy_note()
    }) {
        Ok(())
    } else {
        Err(format!(
            "{wanted:?} carried to a sapphire did not show the legacy color note"
        ))
    }
}

// ---------------------------------------------------------------------------------------------
// BodyColorEditorModel and EditorModel
// ---------------------------------------------------------------------------------------------

/// The color of a sapphire has two rows ("Along the Axis", "Across the Axis"), a tanzanite three:
/// choosing a row makes it the one the sliders edit; a row the stone does not have changes nothing.
fn axis_selected(ui: &MainWindow) -> Result<(), String> {
    open_editor(ui, "sapphire")?;
    let colour = ui.global::<BodyColorEditorModel>();
    let rows = colour.get_axis_labels().row_count();
    if rows != 2 {
        return Err(format!(
            "the editor of a sapphire lists {rows} direction row(s), not the 2 of a uniaxial stone"
        ));
    }
    colour.invoke_axis_selected(1);
    pump(100);
    if colour.get_axis_active() != 1 {
        return Err(format!(
            "choosing row 1 of a sapphire left row {} active",
            colour.get_axis_active()
        ));
    }
    colour.invoke_axis_selected(7);
    pump(100);
    if colour.get_axis_active() != 1 {
        return Err("a row past the list moved the active row".to_owned());
    }
    open_editor(ui, "tanzanite")?;
    let rows = colour.get_axis_labels().row_count();
    if rows != 3 {
        return Err(format!(
            "the editor of a tanzanite lists {rows} direction row(s), not the 3 of a biaxial stone"
        ));
    }
    colour.invoke_axis_selected(2);
    pump(100);
    if colour.get_axis_active() == 2 {
        Ok(())
    } else {
        Err(format!(
            "choosing row 2 of a tanzanite left row {} active",
            colour.get_axis_active()
        ))
    }
}

/// "Save as Variant..." starts its name field on a suggestion: never empty.
fn suggest_variant_name(ui: &MainWindow) -> Result<(), String> {
    open_editor(ui, "sapphire")?;
    let name = ui
        .global::<BodyColorEditorModel>()
        .invoke_suggest_variant_name();
    if name.trim().is_empty() {
        Err("the suggested variant name is empty".to_owned())
    } else {
        Ok(())
    }
}

/// The name "Save as Variant..." saves under in these rows.
const SAVED_NAME: &str = "Harness Variant";

/// A blank name is refused (the call answers `false`); a real one saves the finished colors as a
/// variant of the design's material, uses it on the design and lists it in the Body Color combo.
fn save_variant_requested(ui: &MainWindow) -> Result<(), String> {
    open_editor(ui, "sapphire")?;
    let colour = ui.global::<BodyColorEditorModel>();
    clear_toast(ui);
    if colour.invoke_save_variant_requested("   ".into()) {
        return Err("a blank name was saved as a variant".to_owned());
    }
    clear_toast(ui);
    if !colour.invoke_save_variant_requested(SAVED_NAME.into()) {
        let toast = toast_error(ui).unwrap_or_else(|| "no toast".to_owned());
        return Err(format!(
            "the finished color was not saved as {SAVED_NAME:?} ({toast})"
        ));
    }
    let editor = ui.global::<EditorModel>();
    if wait_until(Duration::from_secs(5), || {
        editor
            .get_body_color_options()
            .iter()
            .any(|option| option.as_str() == SAVED_NAME)
    }) {
        Ok(())
    } else {
        Err(format!(
            "the Body Color list does not offer the saved variant {SAVED_NAME:?}"
        ))
    }
}

/// "Update" on a design with no saved color answers with a sentence; on a design that holds its
/// saved variant unchanged it copies the stored color (no change, no error).
fn update_body_color_variant(ui: &MainWindow) -> Result<(), String> {
    open_editor(ui, "sapphire")?;
    let editor = ui.global::<EditorModel>();
    clear_toast(ui);
    editor.invoke_update_body_color_variant();
    pump(300);
    if toast_error(ui).is_none() {
        return Err("Update on a design with no saved color raised no sentence".to_owned());
    }
    if !ui
        .global::<BodyColorEditorModel>()
        .invoke_save_variant_requested("Harness Stale".into())
    {
        return Err("the finished color was not saved".to_owned());
    }
    pump(300);
    clear_toast(ui);
    editor.invoke_update_body_color_variant();
    pump(300);
    toast_error(ui).map_or(Ok(()), |message| {
        Err(format!(
            "Update on a design that holds its saved variant raised: {message}"
        ))
    })
}

/// "Nearest Variant" on a design without an old single color answers with a sentence; on a
/// legacy color of a sapphire it replaces it by a variant of the material, and the note goes.
fn convert_to_nearest_variant(ui: &MainWindow) -> Result<(), String> {
    design_on(ui, "sapphire")?;
    let editor = ui.global::<EditorModel>();
    clear_toast(ui);
    editor.invoke_convert_to_nearest_variant();
    pump(300);
    if toast_error(ui).is_none() {
        return Err(
            "Nearest Variant on a design with no legacy color raised no sentence".to_owned(),
        );
    }
    legacy_on_sapphire(ui)?;
    clear_toast(ui);
    editor.invoke_convert_to_nearest_variant();
    if !wait_until(Duration::from_secs(5), || {
        !editor.get_body_color_legacy_note()
    }) {
        return Err("the legacy color note is still showing after Nearest Variant".to_owned());
    }
    toast_error(ui).map_or(Ok(()), |message| {
        Err(format!("Nearest Variant raised: {message}"))
    })
}

/// The color index an Apply hands back is a position in the list the Stone tab showed: on a
/// sapphire (Material Default, its variants, Custom) row 1 is a variant, and the design then shows
/// that row; row 0 gives the material's own color back. An Apply with the combo left on the
/// design's own variant row (typing a refractive index) keeps that row, and with a typical pattern
/// on the design it keeps the pattern too.
fn apply_design_material(ui: &MainWindow) -> Result<(), String> {
    design_on(ui, "sapphire")?;
    let editor = ui.global::<EditorModel>();
    let options: Vec<String> = editor
        .get_body_color_options()
        .iter()
        .map(String::from)
        .collect();
    if options.len() < 3 {
        return Err(format!(
            "a sapphire's Body Color list is {options:?}: expected Material Default, at least one variant and Custom"
        ));
    }
    clear_toast(ui);
    for wanted in [1, 0] {
        editor.invoke_apply_design_material(
            editor.get_material_combo_index(),
            editor.get_ri_override_text(),
            wanted,
        );
        if !wait_until(Duration::from_secs(5), || {
            editor.get_body_color_index() == wanted
        }) {
            return Err(format!(
                "applying row {wanted} of {options:?} left row {} chosen",
                editor.get_body_color_index()
            ));
        }
    }
    apply_with_the_combo_untouched(ui, &options)
}

/// Picks the first variant of the sapphire, then applies the material row again with only a
/// refractive index typed: the combo stays on the variant (never "Material Default"), and after a
/// typical pattern the stone still has its pattern.
fn apply_with_the_combo_untouched(ui: &MainWindow, options: &[String]) -> Result<(), String> {
    let editor = ui.global::<EditorModel>();
    editor.invoke_apply_design_material(
        editor.get_material_combo_index(),
        editor.get_ri_override_text(),
        1,
    );
    if !wait_until(Duration::from_secs(5), || {
        editor.get_body_color_index() == 1
    }) {
        return Err("the first variant of the sapphire was not picked".to_owned());
    }
    editor.invoke_apply_design_material(
        editor.get_material_combo_index(),
        "1.77".into(),
        editor.get_body_color_index(),
    );
    pump(400);
    let shown = usize::try_from(editor.get_body_color_index())
        .ok()
        .and_then(|row| options.get(row));
    if editor.get_body_color_index() != 1 || shown.is_some_and(|name| name == "Material Default") {
        return Err(format!(
            "an Apply with the combo untouched left row {} chosen, not the variant row 1",
            editor.get_body_color_index()
        ));
    }
    ui.global::<Zoning>().invoke_apply_typical_pattern(0);
    if !wait_until(Duration::from_secs(30), || {
        editor.get_design_material_is_zoned()
    }) {
        return Err("a typical pattern did not reach the design".to_owned());
    }
    editor.invoke_apply_design_material(
        editor.get_material_combo_index(),
        "1.78".into(),
        editor.get_body_color_index(),
    );
    pump(400);
    if editor.get_design_material_is_zoned() {
        Ok(())
    } else {
        Err("an Apply of the material row dropped the stone's color pattern".to_owned())
    }
}

/// Whether a Slint color is not black (some channel above 30 of 255): a color editor that opens on
/// the stone's own color shows its rays, never black.
fn is_lit(color: slint::Color) -> bool {
    color.red().max(color.green()).max(color.blue()) > 30
}

/// Opening the editor on a plain sapphire seeds the sliders and the rows from the stone's own
/// colors (its Material Default read at the face-up path): the first row is not darker than L* 20
/// and its swatch is not black.
fn open_requested(ui: &MainWindow) -> Result<(), String> {
    open_editor(ui, "sapphire")?;
    let colour = ui.global::<BodyColorEditorModel>();
    if colour.get_tone() <= 20.0 {
        return Err(format!(
            "the editor of a plain sapphire opens on L* {}: black, not the stone's own color",
            colour.get_tone()
        ));
    }
    match colour.get_swatches().row_data(0) {
        Some(first) if is_lit(first) => Ok(()),
        other => Err(format!(
            "the first swatch of a plain sapphire's color editor is {other:?}"
        )),
    }
}

/// "Apply Color" on the untouched editor of a plain sapphire stores a Custom Color: the Body Color
/// combo then sits on "Custom...", the readout names it, and the Live Render dot is not black.
fn apply_requested(ui: &MainWindow) -> Result<(), String> {
    open_editor(ui, "sapphire")?;
    ui.global::<BodyColorEditorModel>().invoke_apply_requested();
    let editor = ui.global::<EditorModel>();
    let custom = pleochroma_editor::material::BODY_COLOR_CUSTOM_LABEL;
    let on_custom = || {
        usize::try_from(editor.get_body_color_index())
            .ok()
            .and_then(|row| editor.get_body_color_options().row_data(row))
            .is_some_and(|name| name.as_str() == custom)
    };
    if !wait_until(Duration::from_secs(5), on_custom) {
        return Err(format!(
            "after Apply Color the Body Color combo shows row {}, not {custom:?}",
            editor.get_body_color_index()
        ));
    }
    let readout = editor.get_body_color_readout();
    if !readout.as_str().contains("Custom") {
        return Err(format!(
            "the readout of an applied Custom Color is {readout:?}"
        ));
    }
    let viewport = ui.global::<ViewportModel>();
    viewport.invoke_render_color_refresh();
    pump(200);
    if viewport.get_render_color_has_swatch() && is_lit(viewport.get_render_color_current()) {
        Ok(())
    } else {
        Err("the Live Render dot of the applied Custom Color is black".to_owned())
    }
}

// ---------------------------------------------------------------------------------------------
// Zoning: the stone's own pattern (Stone tab, Live Render color popup)
// ---------------------------------------------------------------------------------------------

/// The design on a sapphire with the first typical pattern of its species applied, so the stone has
/// a pattern of its own: the Stone tab offers "Reset to Material Pattern" and the Live Render color
/// popup "Remove Zoning" (`Zoning.stone_pattern_overridden`). The states of `states_color.rs` use
/// it too.
///
/// # Errors
///
/// As [`design_on`], or the pattern never reached the design, or the stone is not listed as
/// overridden afterwards.
pub(super) fn zoned_sapphire(ui: &MainWindow) -> Result<(), String> {
    design_on(ui, "sapphire")?;
    let zoning = ui.global::<Zoning>();
    let editor = ui.global::<EditorModel>();
    // The match runs on a worker for a few seconds and is applied only if the stone is still the
    // one it was started for (`stone_pattern.rs`, `same_target`): a solve or a verdict that lands
    // meanwhile changes the design's generation and the app rejects the result with a toast. A
    // user waits for the design to settle, so the press waits too, and tries again once if a late
    // solve still got in the way.
    let mut zoned = false;
    for _ in 0..2 {
        settle_design(ui);
        zoning.invoke_apply_typical_pattern(0);
        // `busy` is raised by the press itself and drops when the match was applied or rejected.
        zoned = wait_until(Duration::from_secs(30), || {
            editor.get_design_material_is_zoned() || !zoning.get_typical_pattern_busy()
        }) && editor.get_design_material_is_zoned();
        if zoned {
            break;
        }
    }
    if !zoned {
        return Err("a typical pattern did not reach the design".to_owned());
    }
    zoning.invoke_refresh_stone_pattern();
    pump(200);
    if zoning.get_stone_pattern_overridden() {
        Ok(())
    } else {
        Err("the stone with a typical pattern is not listed as having its own".to_owned())
    }
}

/// Waits until no solve runs and the verdict that every solve asks for has landed (it marks itself
/// stale first, then lands after a 150 ms debounce and a worker), so nothing changes the design's
/// generation behind the caller.
pub(super) fn settle_design(ui: &MainWindow) {
    let editor = ui.global::<EditorModel>();
    let verdict = ui.global::<VerdictModel>();
    let _ = wait_until(Duration::from_secs(8), || !editor.get_solve_running());
    pump(400);
    let _ = wait_until(Duration::from_secs(8), || {
        !editor.get_solve_running() && !verdict.get_busy() && !verdict.get_stale()
    });
    pump(200);
}

/// The Stone tab's library branch: no design in the editor, the viewport showing a library design
/// as the built-in sapphire (the "Show the Library Design As" pick). The Color pill of that tab
/// is the one whose popup offers the zoning rows. Rust clears `shows_editor_design` only when the
/// library preview is drawn, which the passes never wait for, so it is cleared here (the pick
/// does nothing while it is set).
///
/// # Errors
///
/// The app offers no sapphire, or the stone shown is not zonable as one.
pub(super) fn library_sapphire(ui: &MainWindow) -> Result<(), String> {
    let viewport = ui.global::<ViewportModel>();
    viewport.set_shows_editor_design(false);
    let (place, name) = option_named(
        viewport.get_material_options().iter().map(String::from),
        "sapphire",
    )
    .ok_or("the app offers no sapphire to show a library design as")?;
    // The combo writes its index itself before it calls the handler.
    viewport.set_selected_material_index(at(place));
    viewport.invoke_library_material_chosen(name.as_str().into());
    pump(300);
    viewport.invoke_render_color_refresh();
    let zoning = ui.global::<Zoning>();
    zoning.invoke_refresh_stone_pattern();
    pump(200);
    // The zoning state is read from the stone itself: a sapphire has typical patterns, a stone
    // that is not shown (or not resolved) is not enabled.
    if zoning.get_stone_pattern_enabled() && zoning.get_typical_pattern_labels().row_count() > 0 {
        Ok(())
    } else {
        Err(format!(
            "the library design shown as {name} offers no typical zoning ({})",
            zoning.get_stone_pattern_hint()
        ))
    }
}

/// [`library_sapphire`] with the sapphire's first typical zoning applied to the library view (a
/// view-only pattern: the Stone tab's pill lists it as the stone's own, so the popup offers "Edit
/// Zones..." and "Remove Zoning").
///
/// # Errors
///
/// As [`library_sapphire`], or the pattern never reached the view, or the stone is not listed as
/// having a pattern of its own afterwards.
pub(super) fn zoned_library_sapphire(ui: &MainWindow) -> Result<(), String> {
    let zoning = ui.global::<Zoning>();
    // As in `zoned_sapphire`: the app drops a match whose stone changed while it ran (here: the
    // library design's planes arriving after the pick), so each try starts from a quiet app.
    for _ in 0..3 {
        library_sapphire(ui)?;
        pump(1500);
        zoning.invoke_apply_typical_pattern(0);
        let _ = wait_until(Duration::from_secs(30), || {
            zoning.get_stone_pattern_overridden() || !zoning.get_typical_pattern_busy()
        });
        zoning.invoke_refresh_stone_pattern();
        pump(200);
        if zoning.get_stone_pattern_overridden() {
            return Ok(());
        }
    }
    Err(format!(
        "the library view took no typical pattern (toast {:?})",
        ui.get_toast_message()
    ))
}

/// A typical pattern zones the stone: the design takes the zones and the stone is listed as
/// having a pattern of its own.
fn apply_typical_pattern(ui: &MainWindow) -> Result<(), String> {
    design_on(ui, "sapphire")?;
    if ui.global::<EditorModel>().get_design_material_is_zoned() {
        return Err("the plain sapphire is zoned before any pattern is applied".to_owned());
    }
    zoned_sapphire(ui)
}

/// "Reset to Material Pattern" / "Remove Zoning" takes the stone's own pattern away: the design is
/// not zoned any more, the stone is not listed as having a pattern of its own, and nothing warned.
fn reset_stone_pattern(ui: &MainWindow) -> Result<(), String> {
    zoned_sapphire(ui)?;
    let zoning = ui.global::<Zoning>();
    let editor = ui.global::<EditorModel>();
    clear_toast(ui);
    zoning.invoke_reset_stone_pattern();
    if !wait_until(Duration::from_secs(5), || {
        !editor.get_design_material_is_zoned()
    }) {
        let toast = toast_error(ui).unwrap_or_else(|| "no toast".to_owned());
        return Err(format!(
            "the design is still zoned after the reset ({toast})"
        ));
    }
    zoning.invoke_refresh_stone_pattern();
    pump(200);
    if zoning.get_stone_pattern_overridden() {
        Err("the stone is still listed as having its own pattern after the reset".to_owned())
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// ViewportModel: the Live Render color popup
// ---------------------------------------------------------------------------------------------

/// A row of the popup is a position in the list the popup showed: the first variant of a sapphire
/// is shown afterwards (never "Material Default"), and position 0 gives the material's own color
/// back.
fn render_color_chosen(ui: &MainWindow) -> Result<(), String> {
    design_on(ui, "sapphire")?;
    let viewport = ui.global::<ViewportModel>();
    viewport.invoke_render_color_refresh();
    pump(200);
    let Some(position) = viewport.get_render_color_row_indexes().row_data(1) else {
        return Err(
            "the popup lists no color variant of a sapphire after Material Default".to_owned(),
        );
    };
    clear_toast(ui);
    viewport.invoke_render_color_chosen(position);
    if !wait_until(Duration::from_secs(5), || {
        viewport.get_render_color_index() == position
    }) {
        return Err(format!(
            "choosing position {position} left position {} shown",
            viewport.get_render_color_index()
        ));
    }
    if viewport.get_render_color_summary().as_str() == "Material Default" {
        return Err("a variant is shown as \"Material Default\"".to_owned());
    }
    viewport.invoke_render_color_chosen(0);
    if wait_until(Duration::from_secs(5), || {
        viewport.get_render_color_index() == 0
    }) {
        Ok(())
    } else {
        Err("position 0 did not give the material's own color back".to_owned())
    }
}

// ---------------------------------------------------------------------------------------------
// ColorVariantsModel: the material editor's Color Variants section
// ---------------------------------------------------------------------------------------------

/// The section opened on `Sapphire`.
fn open_section(ui: &MainWindow) -> Result<ColorVariantsModel<'_>, String> {
    let colors = ui.global::<ColorVariantsModel>();
    colors.invoke_opened("Sapphire".into());
    pump(200);
    let place = usize::try_from(colors.get_material_index()).unwrap_or(0);
    if colors
        .get_materials()
        .row_data(place)
        .is_none_or(|name| name.as_str() != "Sapphire")
    {
        return Err("the section did not open on Sapphire".to_owned());
    }
    Ok(colors)
}

/// The section lists the material combo, and Sapphire's rows: its own color first (not a saved
/// row), then its variants.
fn variants_opened(ui: &MainWindow) -> Result<(), String> {
    let colors = open_section(ui)?;
    let rows = colors.get_rows();
    if rows.row_count() < 2 {
        return Err(format!(
            "Sapphire lists {} row(s): expected its own color and at least one variant",
            rows.row_count()
        ));
    }
    if rows.row_data(0).is_some_and(|row| row.saved) {
        return Err("the Material Default row is marked as saved".to_owned());
    }
    Ok(())
}

/// Another material in the combo lists its rows; a place past the combo changes nothing.
fn variants_material_selected(ui: &MainWindow) -> Result<(), String> {
    let colors = open_section(ui)?;
    let diamond = colors
        .get_materials()
        .iter()
        .position(|name| name.as_str().eq_ignore_ascii_case("Diamond"))
        .ok_or("the section's material combo has no diamond")?;
    colors.set_material_index(at(diamond));
    colors.invoke_material_selected(at(diamond));
    pump(200);
    let rows = colors.get_rows();
    if rows.row_count() == 0 || rows.row_data(0).is_some_and(|row| row.saved) {
        return Err("Diamond does not list its own color first".to_owned());
    }
    let count = rows.row_count();
    colors.invoke_material_selected(9999);
    pump(100);
    if colors.get_rows().row_count() == count {
        Ok(())
    } else {
        Err("a material place past the combo changed the rows".to_owned())
    }
}

/// Duplicating the first row saves an editable copy: one more row, marked as saved. A row past the
/// list changes nothing.
fn variants_duplicate(ui: &MainWindow) -> Result<(), String> {
    let colors = open_section(ui)?;
    let before = colors.get_rows().row_count();
    clear_toast(ui);
    colors.invoke_duplicate_requested(0);
    pump(300);
    let rows = colors.get_rows();
    if rows.row_count() != before + 1 {
        let toast = toast_error(ui).unwrap_or_else(|| "no toast".to_owned());
        return Err(format!(
            "Duplicate left {} row(s) after {before} ({toast})",
            rows.row_count()
        ));
    }
    if !rows.row_data(before).is_some_and(|row| row.saved) {
        return Err("the copy is not listed as a saved variant".to_owned());
    }
    colors.invoke_duplicate_requested(99);
    pump(100);
    if colors.get_rows().row_count() == before + 1 {
        Ok(())
    } else {
        Err("a row past the list was duplicated".to_owned())
    }
}

/// A saved copy takes a new name; a built-in row is refused (the call answers `false`).
///
/// The saved rows are listed by label (the vault sorts them), so the renamed copy moves to where its
/// new name sorts, and the vault may hold saved copies of earlier rows: the check counts the rows
/// that carry the new name instead of reading one place.
fn variants_rename(ui: &MainWindow) -> Result<(), String> {
    const NEW_NAME: &str = "Harness Renamed";
    let colors = open_section(ui)?;
    let named = |colors: &ColorVariantsModel<'_>| {
        colors
            .get_rows()
            .iter()
            .filter(|row| row.saved && row.label.as_str() == NEW_NAME)
            .count()
    };
    let before = colors.get_rows().row_count();
    colors.invoke_duplicate_requested(0);
    pump(300);
    clear_toast(ui);
    if colors.invoke_rename_requested(0, "Not Allowed".into()) {
        return Err("a built-in row was renamed".to_owned());
    }
    let rows = colors.get_rows().row_count();
    if rows != before + 1 {
        return Err(format!(
            "Duplicate left {rows} row(s) after {before}, so there is no saved copy to rename"
        ));
    }
    let carrying = named(&colors);
    // The last row is a saved one (the built-in rows come first); which copy it is does not matter.
    if !colors.invoke_rename_requested(at(before), NEW_NAME.into()) {
        let toast = toast_error(ui).unwrap_or_else(|| "no toast".to_owned());
        return Err(format!("the saved copy was not renamed ({toast})"));
    }
    pump(200);
    let now = named(&colors);
    if now == carrying + 1 && colors.get_rows().row_count() == rows {
        Ok(())
    } else {
        Err(format!(
            "after the rename {now} saved row(s) read '{NEW_NAME}' (were {carrying}) of {} row(s)",
            colors.get_rows().row_count()
        ))
    }
}

/// A saved copy is deleted (one row fewer); a built-in row is not.
fn variants_delete(ui: &MainWindow) -> Result<(), String> {
    let colors = open_section(ui)?;
    let before = colors.get_rows().row_count();
    colors.invoke_duplicate_requested(0);
    pump(300);
    colors.invoke_delete_requested(0);
    pump(200);
    if colors.get_rows().row_count() != before + 1 {
        return Err("Delete on a built-in row changed the rows".to_owned());
    }
    colors.invoke_delete_requested(at(before));
    pump(200);
    if colors.get_rows().row_count() == before {
        Ok(())
    } else {
        Err(format!(
            "Delete on the saved copy left {} row(s) of {before}",
            colors.get_rows().row_count()
        ))
    }
}

/// The rows that leave the design alone: the section's callbacks.
const COLOR_VARIANTS_SECTION: &[Row] = checked_rows!(Editor, false;
    ColorVariantsModel.opened => variants_opened;
    ColorVariantsModel.material_selected => variants_material_selected;
    ColorVariantsModel.duplicate_requested => variants_duplicate;
    ColorVariantsModel.rename_requested => variants_rename;
    ColorVariantsModel.delete_requested => variants_delete;
);

/// The rows that change the design: a fresh state for each.
const COLOR_EDITOR: &[Row] = checked_rows!(Editor, true;
    BodyColorEditorModel.axis_selected => axis_selected;
    BodyColorEditorModel.open_requested => open_requested;
    BodyColorEditorModel.apply_requested => apply_requested;
    BodyColorEditorModel.suggest_variant_name => suggest_variant_name;
    BodyColorEditorModel.save_variant_requested => save_variant_requested;
    EditorModel.update_body_color_variant => update_body_color_variant;
    EditorModel.convert_to_nearest_variant => convert_to_nearest_variant;
    EditorModel.apply_design_material => apply_design_material;
    ViewportModel.render_color_chosen => render_color_chosen;
    Zoning.apply_typical_pattern => apply_typical_pattern;
    Zoning.reset_stone_pattern => reset_stone_pattern;
);

/// Every row of this file, in run order.
pub fn rows() -> Vec<&'static Row> {
    [COLOR_VARIANTS_SECTION, COLOR_EDITOR]
        .into_iter()
        .flatten()
        .collect()
}
