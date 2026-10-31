//! Filmed routes of the five Crystal Axis lessons (`materials-orient-sapphire`,
//! `materials-tourmaline-axis-in-table`, `materials-best-blue-tanzanite`,
//! `materials-set-crystal-axis` and `planning-override-orientation`).
//!
//! Every step of these lessons is routed here (`pure_tables::MODULE_SCRIPTED`), reading steps
//! included, so the match in `scripts.rs` stays a list of arms. Each route works the control the
//! step names with the real pointer (the Stone tab's Crystal Axis section, the toolbar's Crystal
//! Axis pill, the handle over the stone, the Rough Planner's axis panel and stone rows) and then
//! reads one model value back, so a renamed button or a click that did nothing stops the video
//! with a sentence instead of filming something else.
//!
//! What the routes need at run time: the Rich Teaching Design on the bench (the lessons start from
//! it), a solved stone (the Along the Width preset measures it), the fixture library (the planner
//! plans a block rough against its designs) and an anisotropic material in each lesson (they pick
//! Sapphire, Tourmaline or Tanzanite in their first action step).

use super::planner_routes::{self, FOOTER_BAND};
use crate::{
    director::Film,
    pacing,
    support::WindowView,
    targets::{self, Query, Resolved},
};
use pleochroma_cut::{
    CrystalAxisModel, EditorModel, GuideModel, RoughPlanModel, SettingsModel, harness_support,
};
use pleochroma_editor::guide::CMD_UNDO;
use slint::{ComponentHandle, Model};
use std::time::Duration;
use uiprobe::AccessibleRole;

/// The lessons this file routes.
const LESSONS: [&str; 5] = [
    "materials-orient-sapphire",
    "materials-tourmaline-axis-in-table",
    "materials-best-blue-tanzanite",
    "materials-set-crystal-axis",
    "planning-override-orientation",
];

/// How long a plan of the filmed rough may take.
const PLAN_TIMEOUT: Duration = Duration::from_secs(180);

/// How far, in logical pixels, the handle is dragged to the right and down.
const DRAG: (f32, f32) = (70.0, 50.0);

/// The Reason typed for the stone of the planning lesson.
const REASON: &str = "avoids the crack";

/// Whether this module routes the lesson `id`.
pub(super) fn handles(id: &str) -> bool {
    LESSONS.contains(&id)
}

/// Films step `step` (from 0) of the Crystal Axis lesson the film is of.
pub(super) fn drive(film: &mut Film<'_>, step: usize) -> Result<(), String> {
    let id = film.id.clone();
    match (id.as_str(), step) {
        ("materials-orient-sapphire" | "materials-best-blue-tanzanite", 0 | 6)
        | ("materials-tourmaline-axis-in-table", 0 | 5)
        | ("materials-set-crystal-axis", 0)
        | ("planning-override-orientation", 0 | 3) => film.hold(pacing::RESULT_HOLD),
        ("materials-orient-sapphire" | "materials-set-crystal-axis", 1) => {
            film.film_material("Sapphire")
        }
        ("materials-tourmaline-axis-in-table", 1) => film.film_material("Tourmaline"),
        ("materials-best-blue-tanzanite", 1) => film.film_material("Tanzanite"),
        (
            "materials-orient-sapphire"
            | "materials-tourmaline-axis-in-table"
            | "materials-best-blue-tanzanite",
            2,
        ) => film.film_view_mode(1),
        ("materials-orient-sapphire", 3) | ("materials-set-crystal-axis", 2) => toggle_axis(film),
        ("materials-orient-sapphire", 4) => set_for_stone(film),
        ("materials-orient-sapphire", 5) => press_preset(film, "Through the Table"),
        ("materials-tourmaline-axis-in-table", 3) => press_preset(film, "In the Table Plane"),
        ("materials-tourmaline-axis-in-table", 4) => press_preset(film, "Along the Width"),
        ("materials-best-blue-tanzanite", 3) => second_axis(film),
        ("materials-best-blue-tanzanite", 4) => press_preset(film, "Best Blue Face-Up"),
        ("materials-best-blue-tanzanite", 5) => press_preset(film, "Violet Face-Up"),
        ("materials-set-crystal-axis", 3) => drag_handle(film, false),
        ("materials-set-crystal-axis", 4) => drag_handle(film, true),
        ("materials-set-crystal-axis", 5) => undo_turn(film),
        ("materials-set-crystal-axis", 6) => as_material(film),
        ("planning-override-orientation", 1) => rough_set_axis(film),
        ("planning-override-orientation", 2) => rough_fit_axis(film),
        ("planning-override-orientation", 4) => planner_routes::plan_current_rough(film),
        ("planning-override-orientation", 5) => override_stone(film),
        ("planning-override-orientation", 6) => lock_and_replan(film),
        ("planning-override-orientation", 7) => open_stone(film),
        _ => Err(format!(
            "no Crystal Axis route for step {} of {id}",
            step + 1
        )),
    }
}

// --- The editor ---------------------------------------------------------------------------

/// Clicks the Stone tab control called `label` with the role `role`.
fn press_stone_choice(
    film: &mut Film<'_>,
    label: &str,
    role: AccessibleRole,
) -> Result<(), String> {
    let spot = film.stone_control(label, |view| {
        targets::role_label(view, label, role, Some("ParametersPanel"))
    })?;
    film.click_and_invoke(&spot)?;
    film.hold(pacing::RESULT_HOLD)
}

/// The toolbar's Crystal Axis pill.
fn toggle_axis(film: &mut Film<'_>) -> Result<(), String> {
    let model = film.ui.global::<CrystalAxisModel>();
    let before = model.get_show_axis();
    let Resolved::Found(spot) = targets::resolve(&film.view(), "crystal_axis_toggle") else {
        return Err(
            "the Crystal Axis pill is not on the toolbar (the material needs a crystal axis)"
                .into(),
        );
    };
    film.click_and_invoke(&spot)?;
    if model.get_show_axis() == before {
        return Err("clicking the Crystal Axis pill did not show or hide the line".into());
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Set for This Stone, unless the stone has an axis of its own already.
pub(super) fn set_for_stone(film: &mut Film<'_>) -> Result<(), String> {
    let settings = film.ui.global::<SettingsModel>();
    if settings.get_c_axis_override_enabled() {
        return Err("the stone has an axis of its own before the lesson sets one".into());
    }
    press_stone_choice(film, "Set for This Stone", AccessibleRole::RadioButton)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        settings.get_c_axis_override_enabled()
    }) {
        return Err("Set for This Stone did not give the stone an axis of its own".into());
    }
    Ok(())
}

/// A preset button of the Crystal Axis section.
pub(super) fn press_preset(film: &mut Film<'_>, label: &str) -> Result<(), String> {
    let model = film.ui.global::<CrystalAxisModel>();
    press_stone_choice(film, label, AccessibleRole::Button)
        .map_err(|why| format!("the preset {label:?}: {why}"))?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_active_preset() >= 0) {
        return Err(format!("the preset {label:?} did not light up"));
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Set for This Stone, the Second Axis switch and a nudge of its Turn slider.
pub(super) fn second_axis(film: &mut Film<'_>) -> Result<(), String> {
    let settings = film.ui.global::<SettingsModel>();
    let model = film.ui.global::<CrystalAxisModel>();
    if !settings.get_c_axis_override_enabled() {
        set_for_stone(film)?;
    }
    if !model.get_biaxial() {
        return Err("the material is not biaxial, so there is no Second Axis".into());
    }
    press_stone_choice(film, "Set", AccessibleRole::RadioButton)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_second_set()) {
        return Err("the Second Axis switch did not turn on".into());
    }
    film.drag_stone_slider("Second Axis Turn in Degrees")?;
    if model.get_second_deg() <= 0.0 {
        return Err("dragging the Turn slider did not change the Second Axis".into());
    }
    Ok(())
}

/// Presses on the axis handle, drags it and either lets go or presses Escape first.
fn drag_handle(film: &mut Film<'_>, cancel: bool) -> Result<(), String> {
    let model = film.ui.global::<CrystalAxisModel>();
    let settings = film.ui.global::<SettingsModel>();
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_overlay_visible()) {
        return Err("the Crystal Axis is not drawn over the stone".into());
    }
    let before = (
        settings.get_c_axis_override_enabled(),
        settings.get_c_axis_tilt_deg(),
        settings.get_c_axis_azimuth_deg(),
    );
    let area = targets::run(&film.view(), &Query::Id("SolidViewportView::drag_area"))
        .ok_or("the Solid viewport's pointer surface is missing")?;
    let start = (
        area.x + model.get_axis_tip_x(),
        area.y + model.get_axis_tip_y(),
    );
    let end = (start.0 + DRAG.0, start.1 + DRAG.1);
    film.gesture(start, end, None, cancel, false)?;
    film.hold(pacing::RESULT_HOLD)?;
    let moved = |tilt: f32, azimuth: f32| {
        (tilt - before.1).abs() > 0.01 || (azimuth - before.2).abs() > 0.01
    };
    if cancel {
        let (tilt, azimuth) = (
            settings.get_c_axis_tilt_deg(),
            settings.get_c_axis_azimuth_deg(),
        );
        if moved(tilt, azimuth) || settings.get_c_axis_override_enabled() != before.0 {
            return Err("Escape did not put the axis back".into());
        }
    } else if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        settings.get_c_axis_override_enabled()
            && moved(
                settings.get_c_axis_tilt_deg(),
                settings.get_c_axis_azimuth_deg(),
            )
    }) {
        return Err("dragging the handle did not turn the axis".into());
    }
    Ok(())
}

/// Undo takes the whole turn back.
fn undo_turn(film: &mut Film<'_>) -> Result<(), String> {
    let settings = film.ui.global::<SettingsModel>();
    film.film_command(CMD_UNDO)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        !settings.get_c_axis_override_enabled()
    }) {
        return Err("Undo did not take the turn of the axis back".into());
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Set for This Stone, then As Material.
fn as_material(film: &mut Film<'_>) -> Result<(), String> {
    let settings = film.ui.global::<SettingsModel>();
    if !settings.get_c_axis_override_enabled() {
        set_for_stone(film)?;
    }
    press_stone_choice(film, "As Material", AccessibleRole::RadioButton)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        !settings.get_c_axis_override_enabled()
    }) {
        return Err("As Material did not hand the axis back to the material".into());
    }
    Ok(())
}

// --- The Rough Planner --------------------------------------------------------------------

/// Step 2: opens the planner, models a block of Sapphire and presses Set under CRYSTAL AXIS.
fn rough_set_axis(film: &mut Film<'_>) -> Result<(), String> {
    film.film_rough_open()?;
    let (window, origin) = planner_routes::planner(film)?;
    let model = window.global::<RoughPlanModel>();
    let block = film.rough_control(
        window.window(),
        origin,
        "Block",
        AccessibleRole::RadioButton,
    )?;
    film.click_secondary(window.window(), origin, &block)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_base_kind() == 0) {
        return Err("the Block choice did not select the block rough".into());
    }
    for (label, value) in [
        ("Rough X (mm)", "18.4"),
        ("Rough Y (mm)", "11.0"),
        ("Rough Z (mm)", "9.6"),
    ] {
        planner_routes::fill(film, label, value, &|| match label {
            "Rough X (mm)" => model.get_rough_x().to_string(),
            "Rough Y (mm)" => model.get_rough_y().to_string(),
            _ => model.get_rough_z().to_string(),
        })?;
    }
    choose_material(film, "Sapphire")?;
    if !model.get_axis_available() {
        return Err("the planner shows no CRYSTAL AXIS section for Sapphire".into());
    }
    let set = film.rough_control(window.window(), origin, "Set", AccessibleRole::Button)?;
    film.click_secondary(window.window(), origin, &set)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_axis_set()) {
        return Err("Set did not give the rough a crystal axis".into());
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Picks `name` in the planner's Material list with the keys, like a learner with the list open.
fn choose_material(film: &mut Film<'_>, name: &str) -> Result<(), String> {
    let (window, origin) = planner_routes::planner(film)?;
    let model = window.global::<RoughPlanModel>();
    let materials = model.get_materials();
    let wanted = (0..materials.row_count())
        .find(|&at| {
            materials
                .row_data(at)
                // The list shows "<name> (SG <gravity>)".
                .is_some_and(|candidate| {
                    candidate.as_str() == name
                        || candidate
                            .as_str()
                            .strip_prefix(name)
                            .is_some_and(|rest| rest.starts_with(" (SG "))
                })
        })
        .and_then(|at| i32::try_from(at).ok())
        .ok_or_else(|| format!("{name} is not in the planner's Material list"))?;
    if model.get_material_index() != wanted {
        let combo = film.rough_control(
            window.window(),
            origin,
            "Material of the Rough",
            AccessibleRole::Combobox,
        )?;
        film.click_secondary(window.window(), origin, &combo)?;
        for _ in 0..(3 * materials.row_count() + 4) {
            let current = model.get_material_index();
            if current == wanted {
                break;
            }
            let key = if wanted > current { "Down" } else { "Up" };
            film.rec
                .press_keys(window.window(), key)
                .map_err(|e| format!("Material {key}: {e:?}"))?;
            if !pacing::wait_real(Duration::from_millis(600), || {
                model.get_material_index() != current
            }) {
                break;
            }
        }
        film.rec
            .press_keys(window.window(), "Return")
            .map_err(|e| format!("Material Return: {e:?}"))?;
        film.hold(Duration::from_millis(300))?;
    }
    if model.get_material_index() != wanted {
        return Err(format!(
            "the planner's Material list did not select {name}: index {} of {wanted}",
            model.get_material_index()
        ));
    }
    Ok(())
}

/// Step 3: Fit to Crystal Length.
fn rough_fit_axis(film: &mut Film<'_>) -> Result<(), String> {
    let (window, origin) = planner_routes::planner(film)?;
    let model = window.global::<RoughPlanModel>();
    let fit = film.rough_control(
        window.window(),
        origin,
        "Fit to Crystal Length",
        AccessibleRole::Button,
    )?;
    film.click_secondary(window.window(), origin, &fit)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || !model.get_axis_note().is_empty()) {
        return Err("Fit to Crystal Length left no note under the axis".into());
    }
    film.hold(pacing::RESULT_HOLD)
}

/// The first stone row of the selected result, picking a result card and a design row when the
/// rows are not shown yet.
fn show_stone_rows(film: &mut Film<'_>) -> Result<(), String> {
    let (window, origin) = planner_routes::planner(film)?;
    let model = window.global::<RoughPlanModel>();
    let height = planner_routes::window_height(&window);
    let shown = || model.get_stone_rows().row_count() > 0;
    if shown() {
        return Ok(());
    }
    let card = planner_routes::visible(&window, |element| {
        element.accessible_role() == Some(AccessibleRole::ListItem)
            && element
                .accessible_label()
                .is_some_and(|label| label.starts_with("Result 1:"))
    })
    .into_iter()
    .next()
    .ok_or("the card of result 1 is not visible")?;
    film.rec.click_at(
        window.window(),
        (card.w.mul_add(0.6, card.x), card.y + 16.0),
        origin,
    )?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, shown) {
        let row = planner_routes::visible(&window, |element| {
            element.accessible_role() == Some(AccessibleRole::ListItem)
                && element
                    .accessible_label()
                    .is_some_and(|label| label.contains(" \u{d7} "))
        })
        .into_iter()
        .find(|row| row.h.mul_add(0.6, row.y) < height - FOOTER_BAND)
        .ok_or("no design row is visible above the results footer")?;
        film.rec.click_at(
            window.window(),
            (row.w.mul_add(0.5, row.x), row.h.mul_add(0.6, row.y)),
            origin,
        )?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, shown) {
            return Err(
                "the selected result lists no stone under STONES AND THE CRYSTAL AXIS".into(),
            );
        }
    }
    film.hold(pacing::RESULT_HOLD)
}

/// A control inside the first stone row.
fn in_first_row(label: &str, role: AccessibleRole) -> Result<targets::Spot, String> {
    let window =
        harness_support::rough_planner_window().ok_or("Rough Planner window is missing")?;
    targets::role_label(
        &WindowView::of("RoughPlannerWindow", &window),
        label,
        role,
        Some("StoneRowItem"),
    )
    .ok_or_else(|| format!("the stone row has no visible {label:?}"))
}

/// Step 6: another pose, if the row offers one, and a Reason.
fn override_stone(film: &mut Film<'_>) -> Result<(), String> {
    show_stone_rows(film)?;
    let (window, origin) = planner_routes::planner(film)?;
    let model = window.global::<RoughPlanModel>();
    if let Ok(pose) = in_first_row("2", AccessibleRole::Button) {
        film.click_secondary(window.window(), origin, &pose)?;
        film.hold(pacing::RESULT_HOLD)?;
    }
    let field = in_first_row("Reason", AccessibleRole::TextInput)?;
    film.type_in_secondary(window.window(), origin, &field, REASON)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        model
            .get_stone_rows()
            .row_data(0)
            .is_some_and(|row| row.reason.as_str() == REASON)
    }) {
        return Err("the stone row did not keep its Reason".into());
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Step 7: Lock on the first stone row, then Plan again.
fn lock_and_replan(film: &mut Film<'_>) -> Result<(), String> {
    let (window, origin) = planner_routes::planner(film)?;
    let model = window.global::<RoughPlanModel>();
    let lock = in_first_row("Lock", AccessibleRole::Button)?;
    film.click_secondary(window.window(), origin, &lock)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        model
            .get_stone_rows()
            .row_data(0)
            .is_some_and(|row| row.locked)
    }) {
        return Err("Lock did not lock the stone".into());
    }
    film.hold(pacing::RESULT_HOLD)?;
    planner_routes::press_button(film, "Plan")?;
    // The first plan's layouts are not saved: the planner asks before replacing them. The lesson
    // replaces them (its text says so).
    if pacing::wait_real(Duration::from_secs(2), || model.get_confirm_replace_open()) {
        planner_routes::press_button(film, "Replace")?;
    }
    // The results of the first plan are on screen, so the wait is for the run to start and end.
    let _started = pacing::wait_real(Duration::from_secs(5), || model.get_running());
    if !pacing::wait_lapse(&mut film.rec, PLAN_TIMEOUT, || {
        !model.get_running() && model.get_results().row_count() > 0
    })? {
        // The stage line says where the run was when the wait ended (a stage that stops reporting
        // is a stuck lane, a line that moves is a plan that is only slow).
        return Err(format!(
            "the second plan did not finish within 180 seconds (running={}, stage {:?}, bar {:.0} %, error {:?})",
            model.get_running(),
            model.get_stage().as_str(),
            100.0 * model.get_progress_fraction(),
            model.get_error_text().as_str()
        ));
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Step 8: Open in Editor on a design row, then the planner leaves the video.
fn open_stone(film: &mut Film<'_>) -> Result<(), String> {
    let (window, origin) = planner_routes::planner(film)?;
    let height = planner_routes::window_height(&window);
    let pill = planner_routes::visible(&window, |element| {
        element.accessible_role() == Some(AccessibleRole::Button)
            && element.accessible_enabled() != Some(false)
            && element.accessible_label().is_some_and(|label| {
                label.starts_with("Open ") && label.ends_with(" in the editor")
            })
    })
    .into_iter()
    .find(|spot| spot.y + spot.h < height - FOOTER_BAND)
    .ok_or("no Open in Editor pill is visible above the results footer")?;
    film.click_secondary(window.window(), origin, &pill)?;
    film.hold(Duration::from_millis(500))?;
    if film.ui.global::<EditorModel>().get_unsaved_dialog_open() {
        return Err("Open in Editor raised an unsaved-changes question".into());
    }
    let guide = film.ui.global::<GuideModel>();
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || guide.get_step_done()) {
        return Err(format!(
            "the planned stone did not reach the editor: toast {:?}",
            film.ui.get_toast_message()
        ));
    }
    // The main window shows the design now; the planner is not part of the picture any more.
    film.rec.detach_window();
    film.attached_window = None;
    film.hold(pacing::RESULT_HOLD)
}
