//! Filmed routes of the Rough Planner in-depth lessons (`library-planner-model-rough`,
//! `library-planner-results`, `library-planner-saved-plans` and `library-planner-open-in-editor`).
//!
//! Every step of these four lessons is routed here (`pure_tables::MODULE_SCRIPTED`), reading steps
//! included, so the match in `scripts.rs` stays a list of arms. Each route clicks the controls the
//! step names in the planner window, with the real pointer, and then reads one model value back, so
//! a renamed button or a click that did nothing stops the video with a sentence instead of filming
//! something else. The planner window is attached to the video by the first step's
//! `film_rough_open` (the lessons that start with a plan do that inside [`plan_a_layout`]).

use super::pure_tables::output_path;
use crate::{
    debug_log::debug_check,
    director::Film,
    pacing, routes,
    support::{WindowView, app::pump},
    targets::{self, Query, Spot},
};
use pleochroma_cut::{
    EditorModel, GuideModel, LibraryModel, RoughPlanModel, RoughPlannerWindow, harness_support,
};
use slint::{ComponentHandle, Model};
use std::time::Duration;
use uiprobe::AccessibleRole;

/// A short beat between two inputs.
const BEAT: Duration = Duration::from_millis(180);

/// How long a cut made by clicking the picture may take: the planner waits out its 500 ms
/// double-click interval before it adds the cut, and the video clock has to run past it.
const CUT_SETTLE: Duration = Duration::from_millis(700);

/// How long a plan of the filmed rough may take.
const PLAN_TIMEOUT: Duration = Duration::from_secs(180);

/// The height under the results column that its pinned footer covers, in logical pixels: a card
/// whose clickable part is lower than this is behind the footer. At the filmed 780 px window the
/// list's viewport ends at about y = 636 (the footer's first button row starts near 652), so the
/// band is 145: the design row's pills, on their own line under the title, stay usable.
pub(super) const FOOTER_BAND: f32 = 145.0;

/// Films step `step` (from 0) of the planner lesson the film is of.
pub(super) fn drive(film: &mut Film<'_>, step: usize) -> Result<(), String> {
    let id = film.id.clone();
    match (id.as_str(), step) {
        ("library-planner-model-rough", 0) => film.film_rough_open(),
        ("library-planner-model-rough", 1) => choose_cylinder(film),
        ("library-planner-model-rough", 2) => look_around(film),
        ("library-planner-model-rough", 3) => cut_in_the_picture(film),
        ("library-planner-model-rough", 4) => undo_and_redo(film),
        ("library-planner-model-rough", 5)
        | ("library-planner-results" | "library-planner-saved-plans", 4)
        | ("library-planner-open-in-editor", 3) => film.hold(pacing::RESULT_HOLD),
        (
            "library-planner-results"
            | "library-planner-saved-plans"
            | "library-planner-open-in-editor",
            0,
        ) => plan_a_layout(film),
        ("library-planner-results", 1) => pick_layout(film),
        ("library-planner-results", 2) => pick_design(film),
        ("library-planner-results", 3) => open_cut_plan(film),
        ("library-planner-results", 5) => show_in_library(film),
        ("library-planner-saved-plans", 1) => tick_and_save(film),
        ("library-planner-saved-plans", 2) => open_saved_plan(film),
        ("library-planner-saved-plans", 3) => export_saved_plan(film),
        ("library-planner-open-in-editor", 1) => open_stone_in_editor(film),
        ("library-planner-open-in-editor", 2) => film.film_inspector_tab(1),
        _ => Err(format!("no planner route for step {} of {id}", step + 1)),
    }
}

/// The planner window and where it is drawn in the video.
pub(super) fn planner(film: &Film<'_>) -> Result<(RoughPlannerWindow, (f32, f32)), String> {
    let window =
        harness_support::rough_planner_window().ok_or("Rough Planner window is missing")?;
    let origin = film
        .rec
        .attached_origin()
        .ok_or("Rough Planner is not attached to the video")?;
    Ok((window, origin))
}

/// The visible elements of the planner window that `keep` accepts, in tree order.
pub(super) fn visible(
    window: &RoughPlannerWindow,
    keep: impl Fn(&uiprobe::ElementHandle) -> bool,
) -> Vec<Spot> {
    let view = WindowView::of("RoughPlannerWindow", window);
    view.elements()
        .into_iter()
        .filter(|element| keep(element))
        .map(|element| Spot::of(&element))
        .filter(|spot| spot.usable(view.size))
        .collect()
}

/// The height of the planner window.
pub(super) fn window_height(window: &RoughPlannerWindow) -> f32 {
    WindowView::of("RoughPlannerWindow", window).size.1
}

/// The first visible, enabled control of the planner window with this role and exact label (a
/// title-bar button, a view pill or a footer button).
fn control(window: &RoughPlannerWindow, role: AccessibleRole, label: &str) -> Result<Spot, String> {
    visible(window, |element| {
        element.accessible_role() == Some(role)
            && element.accessible_label().is_some_and(|name| name == label)
            && element.accessible_enabled() != Some(false)
    })
    .into_iter()
    .next()
    .ok_or_else(|| format!("Rough Planner control {label:?} is not visible"))
}

/// Clicks the planner button `label`.
pub(super) fn press_button(film: &mut Film<'_>, label: &str) -> Result<(), String> {
    let (window, origin) = planner(film)?;
    let spot = control(&window, AccessibleRole::Button, label)?;
    film.click_secondary(window.window(), origin, &spot)?;
    film.hold(BEAT)
}

/// Clicks a control of the scrolling left column (the rough form and the cut list), scrolling it
/// into view first.
fn press_in_form(film: &mut Film<'_>, label: &str, role: AccessibleRole) -> Result<(), String> {
    let (window, origin) = planner(film)?;
    let spot = film.rough_control(window.window(), origin, label, role)?;
    film.click_secondary(window.window(), origin, &spot)?;
    film.hold(BEAT)
}

/// Types `value` into the left column's field `label` and requires `read` to show it. The first
/// click into a freshly attached window can be spent on focus alone, so a field that did not take
/// its text is typed into once more before failing.
pub(super) fn fill(
    film: &mut Film<'_>,
    label: &str,
    value: &str,
    read: &dyn Fn() -> String,
) -> Result<(), String> {
    let (window, origin) = planner(film)?;
    for _ in 0..2 {
        let field =
            film.rough_control(window.window(), origin, label, AccessibleRole::TextInput)?;
        film.type_in_secondary(window.window(), origin, &field, value)?;
        if pacing::wait_real(Duration::from_secs(2), || read() == value) {
            debug_check(label, &value, &read());
            return Ok(());
        }
    }
    Err(format!(
        "{label} did not read back as {value:?}: it shows {:?}",
        read()
    ))
}

/// The 3D view (toolbar, picture and hint line); its centre lies in the picture.
fn picture(window: &RoughPlannerWindow) -> Result<Spot, String> {
    targets::run(
        &WindowView::of("RoughPlannerWindow", window),
        &Query::Id("RoughPlannerWindow::fit_view"),
    )
    .ok_or_else(|| "the Rough Planner's 3D view is missing".to_owned())
}

/// Lesson 1, step 2: Cylinder, its two sizes.
fn choose_cylinder(film: &mut Film<'_>) -> Result<(), String> {
    let (window, _) = planner(film)?;
    let model = window.global::<RoughPlanModel>();
    press_in_form(film, "Cylinder", AccessibleRole::RadioButton)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_base_kind() == 1) {
        return Err("the Cylinder choice did not select the cylinder rough".into());
    }
    fill(film, "Diameter (mm)", "12", &|| {
        model.get_cyl_diameter().to_string()
    })?;
    fill(film, "Length (mm)", "20", &|| {
        model.get_cyl_length().to_string()
    })?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        !model.get_model_text().is_empty() && model.get_model_error().is_empty()
    }) {
        return Err(format!(
            "the cylinder rough gave no valid readout: {:?}",
            model.get_model_error()
        ));
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Lesson 1, step 3: the three view buttons, the wheel and Reset.
fn look_around(film: &mut Film<'_>) -> Result<(), String> {
    for label in ["Front", "Top", "Side"] {
        press_button(film, label)?;
        film.hold(Duration::from_millis(350))?;
    }
    let (window, origin) = planner(film)?;
    let centre = picture(&window)?.center();
    for delta in [120.0, -120.0] {
        film.rec
            .scroll_at_in(window.window(), centre, origin, delta)?;
        pump(20);
        film.hold(Duration::from_millis(350))?;
    }
    press_button(film, "Reset")?;
    film.hold(pacing::RESULT_HOLD)
}

/// Lesson 1, step 4: Face from View, then one click on the rough in the picture.
fn cut_in_the_picture(film: &mut Film<'_>) -> Result<(), String> {
    let (window, origin) = planner(film)?;
    let model = window.global::<RoughPlanModel>();
    let before = model.get_cuts().row_count();
    press_in_form(film, "Face from View", AccessibleRole::Button)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_face_from_view_armed()) {
        return Err("Face from View did not arm the picture".into());
    }
    // The picking reads the frame on screen, so the picture must have been drawn after Reset.
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || !model.get_view_busy()) {
        return Err("the 3D view kept drawing".into());
    }
    film.hold(Duration::from_millis(500))?;
    let centre = picture(&window)?.center();
    film.rec.click_at(window.window(), centre, origin)?;
    pump(60);
    film.hold(CUT_SETTLE)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        model.get_cuts().row_count() == before + 1
    }) {
        return Err(format!(
            "the click on the rough added no cut: cuts={}, armed={}, hint={:?}, error={:?}",
            model.get_cuts().row_count(),
            model.get_face_from_view_armed(),
            model.get_view_hint(),
            model.get_model_error()
        ));
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Lesson 1, step 5: Undo takes the cut back, Redo brings it again.
fn undo_and_redo(film: &mut Film<'_>) -> Result<(), String> {
    let (window, _) = planner(film)?;
    let model = window.global::<RoughPlanModel>();
    let cuts = model.get_cuts().row_count();
    if cuts == 0 {
        return Err("the model has no cut to take back".into());
    }
    press_in_form(film, "Undo", AccessibleRole::Button)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        model.get_cuts().row_count() == cuts - 1
    }) {
        return Err("Undo did not take the cut back".into());
    }
    film.hold(pacing::RESULT_HOLD)?;
    press_in_form(film, "Redo", AccessibleRole::Button)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        model.get_cuts().row_count() == cuts
    }) {
        return Err("Redo did not bring the cut back".into());
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Step 1 of the lessons that need results: opens the planner, types the size of a block, sets
/// Designs (Up To) to 1 and plans.
fn plan_a_layout(film: &mut Film<'_>) -> Result<(), String> {
    film.film_rough_open()?;
    plan_current_rough(film)
}

/// The rest of [`plan_a_layout`], for a lesson that has the planner open already: types the size
/// of a block, sets Designs (Up To) to 1 and plans.
pub(super) fn plan_current_rough(film: &mut Film<'_>) -> Result<(), String> {
    let (window, origin) = planner(film)?;
    let model = window.global::<RoughPlanModel>();
    fill(film, "Rough X (mm)", "18.4", &|| {
        model.get_rough_x().to_string()
    })?;
    fill(film, "Rough Y (mm)", "11.0", &|| {
        model.get_rough_y().to_string()
    })?;
    fill(film, "Rough Z (mm)", "9.6", &|| {
        model.get_rough_z().to_string()
    })?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        !model.get_model_text().is_empty() && model.get_model_error().is_empty()
    }) {
        return Err(format!(
            "the rough model gave no valid readout: {:?}",
            model.get_model_error()
        ));
    }
    film.hold(pacing::RESULT_HOLD)?;

    // Designs (Up To) and the Current Filter choice are pinned below the scrolling column.
    let view = WindowView::of("RoughPlannerWindow", &window);
    let whole = Spot {
        x: 0.0,
        y: 0.0,
        w: view.size.0,
        h: view.size.1,
        handle: None,
    };
    let designs = targets::role_label(
        &view,
        "Most Designs per Layout",
        AccessibleRole::Spinbox,
        None,
    )
    .ok_or("Rough Planner Designs (Up To) spin box is missing")?;
    let point = designs
        .visible_point_in(&whole, view.size)
        .ok_or("Rough Planner Designs (Up To) spin box is outside the window")?;
    film.rec.click_at(window.window(), point, origin)?;
    for chord in ["Ctrl+A", "Backspace"] {
        film.rec
            .press_keys(window.window(), chord)
            .map_err(|e| format!("Designs (Up To) {chord}: {e:?}"))?;
    }
    film.rec
        .type_text(window.window(), "1", Duration::from_millis(55))?;
    film.rec
        .press_keys(window.window(), "Return")
        .map_err(|e| format!("Designs (Up To) Return: {e:?}"))?;
    film.hold(BEAT)?;
    if model.get_max_designs() != 1 {
        return Err(format!(
            "Designs (Up To) did not read back as 1: {}",
            model.get_max_designs()
        ));
    }
    if !model.get_use_filter() {
        let view = WindowView::of("RoughPlannerWindow", &window);
        let choice =
            targets::role_label_start(&view, "Current Filter", AccessibleRole::RadioButton, None)
                .ok_or("Rough Planner Current Filter choice is missing")?;
        let point = choice
            .visible_point_in(&whole, view.size)
            .ok_or("Rough Planner Current Filter choice is outside the window")?;
        film.rec.click_at(window.window(), point, origin)?;
        film.hold(BEAT)?;
    }
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        model.get_filter_count() > 0 && !model.get_running() && !model.get_mesh_busy()
    }) {
        return Err(format!(
            "Plan is not ready: filter_count={}, running={}, mesh_busy={}",
            model.get_filter_count(),
            model.get_running(),
            model.get_mesh_busy()
        ));
    }
    press_button(film, "Plan")?;
    pump(60);
    if !pacing::wait_lapse(&mut film.rec, PLAN_TIMEOUT, || {
        !model.get_running()
            && (model.get_results().row_count() > 0 || !model.get_error_text().is_empty())
    })? {
        return Err("Rough Planner did not finish within 180 seconds".into());
    }
    if model.get_results().row_count() < 2 {
        return Err(format!(
            "the plan gave {} layouts (the lessons need at least two): {}",
            model.get_results().row_count(),
            model.get_error_text()
        ));
    }
    for text in routes::visible_texts(&WindowView::of("RoughPlannerWindow", &window)) {
        if let Some(word) = film.cfg.private_hit(&text) {
            return Err(format!(
                "private text in Rough Planner results ({word:?}): {text:?}"
            ));
        }
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Lesson 2, step 2: a click on the header of result 2.
fn pick_layout(film: &mut Film<'_>) -> Result<(), String> {
    let (window, origin) = planner(film)?;
    let model = window.global::<RoughPlanModel>();
    let height = window_height(&window);
    let card = visible(&window, |element| {
        element.accessible_role() == Some(AccessibleRole::ListItem)
            && element
                .accessible_label()
                .is_some_and(|label| label.starts_with("Result 2:"))
    })
    .into_iter()
    .next()
    .ok_or("the card of result 2 is not visible")?;
    // The header strip of the card, past the tick box: the card's own click target.
    let point = (card.w.mul_add(0.6, card.x), card.y + 16.0);
    if point.1 > height - FOOTER_BAND {
        return Err("the card of result 2 is behind the results footer".into());
    }
    film.rec.click_at(window.window(), point, origin)?;
    pump(60);
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        model.get_selected_result() == 1 && model.get_view_mode() == 1
    }) {
        return Err(format!(
            "the click on result 2 did not show it: selected={}, view mode={}",
            model.get_selected_result(),
            model.get_view_mode()
        ));
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Lesson 2, step 3: a click on the body of a design row.
fn pick_design(film: &mut Film<'_>) -> Result<(), String> {
    let (window, origin) = planner(film)?;
    let model = window.global::<RoughPlanModel>();
    let height = window_height(&window);
    let row = visible(&window, |element| {
        element.accessible_role() == Some(AccessibleRole::ListItem)
            && element
                .accessible_label()
                .is_some_and(|label| label.contains(" \u{d7} "))
    })
    .into_iter()
    .find(|row| row.h.mul_add(0.6, row.y) < height - FOOTER_BAND)
    .ok_or("no design row is visible above the results footer")?;
    // The metric lines of the row carry no control of their own: a click there is the row's.
    let point = (row.w.mul_add(0.5, row.x), row.h.mul_add(0.6, row.y));
    film.rec.click_at(window.window(), point, origin)?;
    pump(60);
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_selected_group() >= 0) {
        return Err("the click on a design row did not select it".into());
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Lesson 2, step 4: the Cut Plan fold of a card.
fn open_cut_plan(film: &mut Film<'_>) -> Result<(), String> {
    let (window, origin) = planner(film)?;
    let model = window.global::<RoughPlanModel>();
    let height = window_height(&window);
    let button = visible(&window, |element| {
        element.accessible_role() == Some(AccessibleRole::Button)
            && element
                .accessible_label()
                .is_some_and(|label| label == "Cut Plan")
    })
    .into_iter()
    .find(|spot| spot.y + spot.h < height - FOOTER_BAND)
    .ok_or("no Cut Plan button is visible above the results footer")?;
    film.click_secondary(window.window(), origin, &button)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_expanded_index() >= 0) {
        return Err("Cut Plan did not fold open".into());
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Lesson 2, step 6: Show All in Library, and the banner it puts over the library list.
fn show_in_library(film: &mut Film<'_>) -> Result<(), String> {
    press_button(film, "Show All in Library")?;
    let library = film.ui.global::<LibraryModel>();
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        !library.get_id_filter_label().is_empty()
    }) {
        return Err("Show All in Library put no banner over the library list".into());
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Lesson 3, step 2: ticks results 1 and 2, Save Selected, and the name row's Save.
fn tick_and_save(film: &mut Film<'_>) -> Result<(), String> {
    let (window, origin) = planner(film)?;
    let model = window.global::<RoughPlanModel>();
    let height = window_height(&window);
    for rank in 1..=2 {
        let label = format!("Keep Result {rank} for saving");
        let tick = visible(&window, |element| {
            element.accessible_role() == Some(AccessibleRole::Checkbox)
                && element
                    .accessible_label()
                    .is_some_and(|name| name == label.as_str())
        })
        .into_iter()
        .next()
        .ok_or_else(|| format!("the tick box {label:?} is not visible"))?;
        if tick.y + tick.h > height - FOOTER_BAND {
            return Err(format!(
                "the tick box {label:?} is behind the results footer"
            ));
        }
        film.click_secondary(window.window(), origin, &tick)?;
        film.hold(BEAT)?;
    }
    if model.get_keep_count() != 2 {
        return Err(format!(
            "ticking two cards left {} ticked",
            model.get_keep_count()
        ));
    }
    press_button(film, "Save Selected (2)")?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_save_name_open()) {
        return Err("Save Selected did not open the name row".into());
    }
    film.hold(pacing::RESULT_HOLD)?;
    press_button(film, "Save")?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        !model.get_save_name_open() && model.get_saved_plans().row_count() >= 1
    }) {
        return Err(format!(
            "the plan was not saved: {:?}, {:?}",
            model.get_status_text(),
            model.get_error_text()
        ));
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Lesson 3, step 3: Saved Plans, then Open on the plan (and Replace when the planner asks).
fn open_saved_plan(film: &mut Film<'_>) -> Result<(), String> {
    let (window, _) = planner(film)?;
    let model = window.global::<RoughPlanModel>();
    press_button(film, "Saved Plans")?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_saved_open()) {
        return Err("Saved Plans did not open the list".into());
    }
    film.hold(pacing::RESULT_HOLD)?;
    press_button(film, "Open")?;
    // Most of the results were never saved, so the planner asks before it replaces them.
    let asked = pacing::wait_real(Duration::from_secs(2), || {
        model.get_confirm_replace_open() || !model.get_loaded_banner().is_empty()
    });
    if asked && model.get_confirm_replace_open() {
        film.hold(pacing::RESULT_HOLD)?;
        press_button(film, "Replace")?;
    }
    // The banner and the closing of the list wait for the rows of the opened plan.
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        !model.get_loaded_banner().is_empty() && !model.get_saved_open()
    }) {
        return Err(format!(
            "the saved plan did not open: {:?}",
            model.get_error_text()
        ));
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Lesson 3, step 4: Export on the saved plan, the Save dialog answered with a path of the
/// lesson's own output folder.
fn export_saved_plan(film: &mut Film<'_>) -> Result<(), String> {
    let (window, _) = planner(film)?;
    let model = window.global::<RoughPlanModel>();
    press_button(film, "Saved Plans")?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_saved_open()) {
        return Err("Saved Plans did not open the list".into());
    }
    film.hold(pacing::RESULT_HOLD)?;
    let path = output_path(&film.id, "plan.psrd");
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|e| format!("create tutorial output: {e}"))?;
    }
    if path.exists() {
        return Err(format!(
            "the exported plan already exists before the click: {}",
            path.display()
        ));
    }
    harness_support::queue_picker_path(path.clone());
    press_button(film, "Export")?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        path.is_file() && harness_support::pending_picker_answers() == 0
    }) {
        return Err(format!(
            "Export did not write the plan: queued answers={}, error={:?}",
            harness_support::pending_picker_answers(),
            model.get_error_text()
        ));
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Lesson 4, step 2: Open in Editor on the first design row, then the planner leaves the video
/// so the editor shows.
fn open_stone_in_editor(film: &mut Film<'_>) -> Result<(), String> {
    let (window, origin) = planner(film)?;
    let height = window_height(&window);
    let pill = visible(&window, |element| {
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
