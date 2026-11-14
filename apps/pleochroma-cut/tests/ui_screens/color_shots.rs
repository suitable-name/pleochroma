//! The shots of the color variants: the Stone tab's Body Color combo on a sapphire (its color
//! variants, no generic colors) and on a diamond (the nine generic colors), the per-direction
//! Custom color editor on a uniaxial (sapphire) and a biaxial (tanzanite) stone, a design that
//! keeps an old single color on a sapphire (the legacy note with "Convert..." and "Nearest
//! Variant"), the Color Variants section of the Gem Material Editor, a Color Pattern zone whose
//! Source is Variant, and the Live Render color popup on a sapphire. Each drives the real app
//! through the model calls its controls make.
//!
//! The Stone tab shots work on a design in the editor (a library design shows a view-only color
//! instead); the Live Render popup shot works on a library design.

use crate::{
    dialog_shots::{open_material_editor, popup_in, scroll_to_label},
    kit::{Rect, Sc, card_rect, click_first, find_inherits, rect_of},
    nav::{FVS, STUDIO},
    support::{SecondaryWindow, app::pump},
};
use pleochroma_cut::{
    BodyColorEditorModel, ColorPatternModel, ColorVariantsModel, EditorModel, ParametersModel,
    ViewportModel, Zoning, harness_support,
};
use slint::{ComponentHandle, Model};
use uiprobe::screenshot::Shot;

/// How far one wheel step scrolls a panel, in logical pixels.
const WHEEL_STEP: f32 = 300.0;
/// Room kept above the Body Color combo when it is scrolled to the top of the Stone tab: its
/// heading and the one-line hint, in logical pixels.
const HEADING_ROOM: f32 = 70.0;

/// The place and name of the option called `wanted` (letter case ignored), else the first option
/// that contains it.
fn option_named(options: &[String], wanted: &str) -> Option<(usize, String)> {
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

/// Gives the open design the built-in material called `wanted` (the Stone tab's Material
/// pick) and waits for the solve it starts.
fn design_on(sc: &Sc<'_, '_>, wanted: &str) -> Result<(), String> {
    let ui = sc.ui();
    let names: Vec<String> = ui
        .global::<ViewportModel>()
        .get_material_options()
        .iter()
        .map(String::from)
        .collect();
    let (_, name) = option_named(&names, wanted)
        .ok_or_else(|| format!("no material called {wanted:?} among {names:?}"))?;
    let editor = ui.global::<EditorModel>();
    let code = editor.invoke_apply_named_material(name.as_str().into(), 0.0);
    if code != 0 {
        return Err(format!("the design did not take {name} (answer {code})"));
    }
    let _ = Sc::wait(30, || !editor.get_solve_running());
    pump(800);
    sc.ctx.app.clear_notice();
    Ok(())
}

/// The design of the editor shots in `material`, in the Live Render view with the Parameters dock
/// on its Stone tab.
fn stone_tab_of(sc: &Sc<'_, '_>, material: &str) -> Result<(), String> {
    sc.open_in_editor(FVS)?;
    design_on(sc, material)?;
    sc.show_live_view();
    let ui = sc.ui();
    let dock = ui.global::<ParametersModel>();
    dock.set_open(true);
    dock.invoke_show_tab(0);
    pump(1500);
    Ok(())
}

/// Scrolls the Stone tab until the Body Color combo is near the top of the Parameters panel and
/// returns the dock's rectangle.
fn body_color_into_view(sc: &Sc<'_, '_>) -> Result<Rect, String> {
    let view = sc.view();
    let panel = find_inherits(&view, "ParametersPanel")
        .ok_or_else(|| "the Parameters panel is not on screen".to_owned())?;
    let area = panel
        .query_descendants()
        .match_inherits("ScrollView")
        .find_all()
        .into_iter()
        .max_by(|a, b| {
            let (sa, sb) = (a.size(), b.size());
            (sa.width * sa.height).total_cmp(&(sb.width * sb.height))
        })
        .ok_or_else(|| "the Stone tab has no scroll area".to_owned())?;
    let mut combo = None;
    for _ in 0..40 {
        combo = sc.view().try_find("Body Color");
        if combo.is_some() {
            break;
        }
        area.scroll(0.0, -WHEEL_STEP);
        pump(150);
    }
    let combo = combo.ok_or_else(|| "the Body Color combo never came into view".to_owned())?;
    let above = Rect::of(&combo).y - Rect::of(&panel).y - HEADING_ROOM;
    if above > 0.0 {
        area.scroll(0.0, -above);
        pump(300);
    }
    rect_of(&sc.view(), "ParametersDock").or_else(|_| rect_of(&sc.view(), "ParametersPanel"))
}

/// The labels of the Body Color combo.
fn body_color_options(sc: &Sc<'_, '_>) -> Vec<String> {
    sc.ui()
        .global::<EditorModel>()
        .get_body_color_options()
        .iter()
        .map(String::from)
        .collect()
}

/// The Stone tab of a design in `material` with the Body Color tiles in view. The tiles are always
/// drawn (one per entry, five to a row), so there is nothing to open.
fn combo_open(sc: &mut Sc<'_, '_>, material: &str) -> Result<Shot, String> {
    stone_tab_of(sc, material)?;
    let dock = body_color_into_view(sc)?;
    let options = body_color_options(sc);
    // Each entry is a tile labelled with its option name.
    let listed = options
        .get(1)
        .is_some_and(|second| !sc.view().by_label(second).is_empty());
    if !listed {
        sc.note(format!(
            "the Body Color tiles ({options:?}) did not show their entries"
        ));
    }
    let shot = sc.snap_main()?;
    Ok(dock.crop(&shot))
}

/// `color_variants_stone_sapphire.png`: the Stone tab's Body Color list of a sapphire: its own
/// color, its color variants and Custom, with no generic colors.
///
/// # Errors
///
/// The design or the material is missing, or the combo is not on screen.
pub fn stone_sapphire(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    combo_open(sc, "sapphire")
}

/// `color_variants_stone_diamond.png`: the Stone tab's Body Color list of a diamond: its own
/// color, the nine generic colors and Custom.
///
/// # Errors
///
/// As [`stone_sapphire`].
pub fn stone_diamond(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    combo_open(sc, "diamond")
}

/// The Stone tab of a design in `material` with the Custom color editor open (the combo on its
/// last row and "Choose Color..." pressed), once it solved its `rows` direction rows.
fn custom_editor(sc: &Sc<'_, '_>, material: &str, rows: usize) -> Result<Shot, String> {
    stone_tab_of(sc, material)?;
    let ui = sc.ui();
    let editor = ui.global::<EditorModel>();
    let options = editor.get_body_color_options();
    let custom = options
        .row_data(options.row_count().saturating_sub(1))
        .map(|name| name.to_string())
        .unwrap_or_default();
    body_color_into_view(sc)?;
    // The last tile is Custom: pressing it opens the editor under the tiles.
    if !click_first(&sc.view(), &custom) {
        return Err(format!("the Stone tab shows no {custom:?} tile"));
    }
    let colour = ui.global::<BodyColorEditorModel>();
    if !Sc::wait(60, || colour.get_has_solution() && !colour.get_busy()) {
        return Err("the color editor never finished its first solve".to_owned());
    }
    pump(1000);
    let shown = colour.get_axis_labels().row_count();
    if shown != rows {
        return Err(format!(
            "the editor of {material} shows {shown} direction row(s), not {rows}"
        ));
    }
    let dock = body_color_into_view(sc)?;
    let shot = sc.snap_main()?;
    Ok(dock.crop(&shot))
}

/// `color_variants_custom_sapphire.png`: the Custom color editor of a sapphire (a uniaxial
/// crystal): a row for the color Along the Axis and one for Across the Axis.
///
/// # Errors
///
/// As [`stone_sapphire`], or the editor does not solve.
pub fn custom_sapphire(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    custom_editor(sc, "sapphire", 2)
}

/// `color_variants_custom_tanzanite.png`: the Custom color editor of a tanzanite (a biaxial
/// crystal): a row each for Alpha, Beta and Gamma.
///
/// # Errors
///
/// As [`custom_sapphire`].
pub fn custom_tanzanite(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    custom_editor(sc, "tanzanite", 3)
}

/// `color_variants_legacy.png`: a sapphire that keeps a single saved color: the Stone tab says so
/// and offers "Convert..." and "Nearest Variant".
///
/// The color is a generic preset picked on a diamond and carried to the sapphire by one Apply, as
/// the combos do.
///
/// # Errors
///
/// As [`stone_sapphire`], or the preset or the note does not show.
pub fn legacy_color(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    stone_tab_of(sc, "diamond")?;
    let ui = sc.ui();
    let editor = ui.global::<EditorModel>();
    // [Material Default, the nine generic colors, Custom]: the third color is the one to find.
    let presets = pleochroma_editor::material::body_color_options();
    let wanted = presets
        .get(3)
        .ok_or_else(|| "the editor knows no generic color".to_owned())?;
    let options = body_color_options(sc);
    let row = options
        .iter()
        .position(|option| option == wanted)
        .ok_or_else(|| format!("a diamond's Body Color list {options:?} has no {wanted:?}"))?;
    let combo: Vec<String> = editor
        .get_material_combo_options()
        .iter()
        .map(String::from)
        .collect();
    let (sapphire, _) = option_named(&combo, "sapphire")
        .ok_or_else(|| format!("the Material list {combo:?} has no sapphire"))?;
    editor.invoke_apply_design_material(
        i32::try_from(sapphire).unwrap_or(0),
        "".into(),
        i32::try_from(row).unwrap_or(0),
    );
    if !Sc::wait(15, || editor.get_body_color_legacy_note()) {
        return Err(format!(
            "{wanted:?} on a sapphire did not show the legacy color note"
        ));
    }
    let _ = Sc::wait(30, || !editor.get_solve_running());
    pump(1500);
    let dock = body_color_into_view(sc)?;
    let shot = sc.snap_main()?;
    Ok(dock.crop(&shot))
}

/// `color_variants_section.png`: the Color Variants section of the Gem Material Editor.
///
/// It is shot on a sapphire and shows its own color, its built-in variants (with their
/// Estimated mark and source line), and a saved copy that can be renamed and deleted.
///
/// # Errors
///
/// The design or the material is missing, or the dialog does not appear.
pub fn variants_section(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    open_material_editor(sc, "Sapphire")?;
    scroll_to_label(sc, "MaterialEditorDialog", "Color Variants")?;
    let ui = sc.ui();
    let colors = ui.global::<ColorVariantsModel>();
    let _ = Sc::wait(10, || colors.get_rows().row_count() > 1);
    // Row 0 is the material's own color, row 1 the first variant of the data file.
    colors.invoke_duplicate_requested(i32::from(colors.get_rows().row_count() > 1));
    pump(800);
    // The section is the last of the dialog: scroll to the end and photograph from its heading
    // (when it is still in view) to the bottom of the scrolled area.
    let view = sc.view();
    let root = find_inherits(&view, "MaterialEditorDialog")
        .ok_or_else(|| "the dialog MaterialEditorDialog is not on screen".to_owned())?;
    let area = root
        .query_descendants()
        .match_inherits("ScrollView")
        .find_first();
    area.as_ref().unwrap_or(&root).scroll(0.0, -4000.0);
    pump(500);
    let view = sc.view();
    let card = card_rect(&view, "MaterialEditorDialog")?;
    let scrolled = area.as_ref().map_or(card, Rect::of);
    let top = view
        .try_find("Color Variants")
        .map_or(scrolled.y, |heading| {
            (Rect::of(&heading).y - 12.0).max(scrolled.y)
        });
    let shot = sc.snap_main()?;
    Ok(Rect {
        x: card.x,
        y: top,
        w: card.w,
        h: scrolled.bottom() - top,
    }
    .crop(&shot))
}

/// `color_pattern_variant.png`: the Color Pattern window of a sapphire stone with the selected
/// layer's From variant dropdown set to a variant: its caption and the Along and Across colors of
/// the variant.
///
/// # Errors
///
/// The design or the material is missing, or the window does not open.
pub fn pattern_variant(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    design_on(sc, "sapphire")?;
    sc.show_live_view();
    let ui = sc.ui();
    ui.global::<Zoning>().invoke_edit_stone_pattern();
    if !Sc::wait(30, || {
        harness_support::is_window_open(SecondaryWindow::ColorPattern)
    }) {
        return Err("the Color Pattern window did not open".to_owned());
    }
    let window = harness_support::color_pattern_window()
        .ok_or_else(|| "the Color Pattern window is gone".to_owned())?;
    pump(1500);
    let model = window.global::<ColorPatternModel>();
    // The typical zoning of the species gives layers to look at (the gallery picks it and
    // closes); the top layer then takes the first variant of the material, when it has one.
    if model.get_typical_tiles().row_count() > 0 {
        model.invoke_typical_picked(0);
        pump(1000);
    }
    model.invoke_select_row(0);
    pump(300);
    if model.get_variant_labels().row_count() > 1 {
        model.invoke_variant_picked(1);
        pump(800);
    }
    if model.get_variant_index() != 1 {
        return Err("the From variant dropdown did not give the layer a color variant".to_owned());
    }
    pump(1000);
    sc.snap_window("ColorPatternWindow", &window)
}

/// `color_variants_live_render.png`: the color popup of a library design shown as a sapphire (the
/// Stone tab's Color pill, in the Live Render view): its own color and its color variants with
/// their dots, the Custom Color row and the zoning section, and no generic colors.
///
/// # Errors
///
/// The design or the material is missing, or the popup does not open.
pub fn live_render_popup(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.select_design(STUDIO)?;
    sc.choose_material("sapphire")?;
    sc.show_live_view();
    let ui = sc.ui();
    // The Color pill of a library design is the Stone tab's color control, under its Material.
    let dock = ui.global::<ParametersModel>();
    dock.set_open(true);
    dock.invoke_show_tab(0);
    pump(1000);
    let viewport = ui.global::<ViewportModel>();
    viewport.invoke_render_color_refresh();
    pump(600);
    let label = format!("Color: {}", viewport.get_render_color_summary());
    // The popup's rows report places inside the popup, not the window: it is found by the pixels
    // it changes (the Solid view behind it is a still picture).
    let before = sc.snap_main()?;
    if !click_first(&sc.view(), &label) {
        return Err(format!("the Stone tab shows no {label:?} button"));
    }
    pump(800);
    let after = sc.snap_main()?;
    popup_in(&before, &after)
}
