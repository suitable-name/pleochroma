//! Filmed routes of the six render-effects lessons (`materials-polish-quality`,
//! `materials-scratch-direction`, `materials-silk-sapphire`, `materials-opal-play-of-color`,
//! `materials-labradorite-schiller` and `materials-axis-curves`).
//!
//! Every step of these lessons is routed here (`pure_tables::MODULE_SCRIPTED`), reading steps
//! included, so the match in `scripts.rs` stays a list of arms. Each route works the control the
//! step names with the real pointer and keys (the tier table and the inspector's Surface Finish
//! list, the toolbar's Surface Finish pill, the Stone tab's Inclusions and Play-of-Color groups,
//! the Material Editor) and then reads one model value back, so a renamed button or a click that
//! did nothing stops the video with a sentence instead of filming something else. A step whose
//! goal reads the design also waits for the guide to report the step done, which happens once the
//! debounced commit has written the design.
//!
//! Not filmed: the Preferences dialog of the Scratch Direction lesson (step 5). It covers the
//! recording, so the route holds on the step's text; the dialog's own screen is covered by the
//! UI screens harness.
//!
//! What the routes need at run time: the Rich Teaching Design on the bench (the lessons start from
//! it) and, for the surface finish steps, the Advanced controls (the lessons unlock them).

use super::crystal_axis_routes::{press_preset, second_axis, set_for_stone};
use crate::{director::Film, pacing, targets};
use pleochroma_cut::{GuideModel, RenderEffectsModel, SurfaceModel, ViewportModel};
use slint::ComponentHandle;
use uiprobe::AccessibleRole;

/// The lessons this file routes.
const LESSONS: [&str; 6] = [
    "materials-polish-quality",
    "materials-scratch-direction",
    "materials-silk-sapphire",
    "materials-opal-play-of-color",
    "materials-labradorite-schiller",
    "materials-axis-curves",
];

/// The accessible label of the inspector's Surface Finish list.
const GRADE_LIST: &str = "Surface Finish Grade of This Tier";

/// The index of a grade in `SurfaceModel.grade_names` (the menu order of `SurfaceGrade::ALL`).
const COMMERCIAL: i32 = 3;
const PREPOLISH: i32 = 4;
const IDEAL: i32 = 0;

/// The index of Sapphire in the Material Editor's Load Preset Template list ("Custom", then
/// the gems alphabetically: Cubic Zirconia, Diamond, Emerald, Quartz, Ruby, Sapphire, ...).
const SAPPHIRE_TEMPLATE: usize = 6;

/// The Material Name typed for the curves lesson.
const CURVES_NAME: &str = "Tutorial curves";

/// Whether this module routes the lesson `id`.
pub(super) fn handles(id: &str) -> bool {
    LESSONS.contains(&id)
}

/// Films step `step` (from 0) of the render-effects lesson the film is of.
pub(super) fn drive(film: &mut Film<'_>, step: usize) -> Result<(), String> {
    let id = film.id.clone();
    match (id.as_str(), step) {
        // The reading steps.
        ("materials-polish-quality" | "materials-opal-play-of-color", 0)
        | ("materials-silk-sapphire", 0 | 7)
        | ("materials-scratch-direction", 0 | 4 | 6)
        | ("materials-labradorite-schiller", 0 | 5)
        | ("materials-axis-curves", 0 | 3) => film.hold(pacing::RESULT_HOLD),
        // Use it on the design: the saved material is picked in the Stone tab's Material list,
        // where it is listed after the built-ins.
        ("materials-axis-curves", 2) => {
            film.film_material(CURVES_NAME)?;
            film.hold(pacing::RESULT_HOLD)
        }
        // The Path-Traced step every lesson but the last has.
        ("materials-polish-quality" | "materials-scratch-direction", 1)
        | (
            "materials-silk-sapphire"
            | "materials-opal-play-of-color"
            | "materials-labradorite-schiller",
            2,
        ) => film.film_view_mode(1),
        ("materials-silk-sapphire", 1) => film.film_material("Sapphire"),
        ("materials-opal-play-of-color", 1) => film.film_material("Precious Opal (White)"),
        ("materials-labradorite-schiller", 1) => film.film_material("Labradorite"),
        // Surface finish.
        ("materials-polish-quality", 2) => choose_grade(film, "Crown Main", COMMERCIAL),
        ("materials-polish-quality", 3) => choose_grade(film, "Crown Main", PREPOLISH),
        ("materials-polish-quality", 4) => film.film_stone_effect("stone_haze_changed"),
        ("materials-polish-quality", 5) => choose_grade(film, "Crown Main", IDEAL),
        ("materials-scratch-direction", 2) => choose_grade(film, "Pavilion Main", PREPOLISH),
        ("materials-scratch-direction", 3) => show_surface_finish(film),
        ("materials-scratch-direction", 5) => custom_angle(film),
        // Silk.
        ("materials-silk-sapphire", 3) => {
            set_for_stone(film)?;
            press_preset(film, "Through the Table")
        }
        ("materials-silk-sapphire", 4) => choose_silk_preset(film, "Star Silk"),
        ("materials-silk-sapphire", 5) => turn_silk(film),
        ("materials-silk-sapphire", 6) => cats_eye(film),
        // Play-of-color.
        ("materials-opal-play-of-color", 3) => show_structure_switch(film, "Show Play-of-Color"),
        ("materials-labradorite-schiller", 3) => show_structure_switch(film, "Show Schiller"),
        ("materials-opal-play-of-color", 4) => {
            open_structure_card(film)?;
            click_stone_label(film, "Pinfire")
        }
        ("materials-opal-play-of-color", 5) => drag_and_finish(film, "Color Range, Low End"),
        ("materials-opal-play-of-color", 6) => reseed(film),
        ("materials-opal-play-of-color", 7) => choose_material_again(film, "Precious Opal (Black)"),
        ("materials-labradorite-schiller", 4) => drag_and_finish(film, "Coverage"),
        // The Material Editor.
        ("materials-axis-curves", 1) => curve_material(film),
        _ => Err(format!(
            "no render-effects route for step {} of {id}",
            step + 1
        )),
    }
}

/// Waits for the guide to report the current step done: the debounced commit has written the
/// design and its goal holds.
fn wait_step_done(film: &Film<'_>, what: &str) -> Result<(), String> {
    let guide = film.ui.global::<GuideModel>();
    if pacing::wait_real(pacing::STEP_TIMEOUT, || guide.get_step_done()) {
        Ok(())
    } else {
        Err(format!(
            "{what} did not reach the design before the timeout"
        ))
    }
}

// --- Surface finish -------------------------------------------------------------------------

/// Selects the tier `tier` and picks grade number `grade` in the inspector's Surface Finish list
/// with the keys, like a learner with the list open.
fn choose_grade(film: &mut Film<'_>, tier: &str, grade: i32) -> Result<(), String> {
    let model = film.ui.global::<SurfaceModel>();
    film.film_select_tier(tier)?;
    // The list sits low in the inspector's scrolling Tier form: scrolled into view like the
    // other tier-form controls.
    let combo = film
        .inspector_form_control(GRADE_LIST, AccessibleRole::Combobox)?
        .ok_or_else(|| {
            format!(
                "the Surface Finish list of {tier} is not on screen (the inspector's Tier tab, in Advanced)"
            )
        })?;
    let current = model.get_tier_grade();
    if current == grade {
        return Err(format!(
            "{tier} already has grade {grade} before the lesson chooses it"
        ));
    }
    film.click_and_invoke(&combo)?;
    let key = if grade > current { "Down" } else { "Up" };
    for _ in 0..(grade - current).abs() {
        film.rec
            .press_keys(film.ui.window(), key)
            .map_err(|e| format!("Surface Finish {key}: {e:?}"))?;
    }
    film.rec
        .press_keys(film.ui.window(), "Return")
        .map_err(|e| format!("Surface Finish Return: {e:?}"))?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_tier_grade() == grade) {
        return Err(format!(
            "the Surface Finish list did not select grade {grade} for {tier}"
        ));
    }
    film.hold(pacing::RESULT_HOLD)?;
    wait_step_done(film, "the Surface Finish grade")
}

/// The Solid view's Surface Finish toggle: on the toolbar, or in the Show menu of a narrow view.
fn show_surface_finish(film: &mut Film<'_>) -> Result<(), String> {
    let model = film.ui.global::<SurfaceModel>();
    if model.get_show_overlay() {
        return Err("Show Surface Finish is already on before the lesson clicks it".into());
    }
    film.click_solid_toggle("Surface Finish")
        .map_err(|why| format!("{why} (it is hidden in the Diagram view)"))?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_show_overlay()) {
        return Err("clicking the Surface Finish pill did not show the overlay".into());
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Ticks Custom Angle for the Pavilion Main tier in the inspector.
fn custom_angle(film: &mut Film<'_>) -> Result<(), String> {
    let model = film.ui.global::<SurfaceModel>();
    film.film_select_tier("Pavilion Main")?;
    if model.get_use_angle() {
        return Err("Custom Angle is already ticked before the lesson ticks it".into());
    }
    let tick = film
        .inspector_form_control("Custom Scratch Angle", AccessibleRole::Checkbox)?
        .ok_or("the Custom Angle box is not on screen (a polished tier, in Advanced)")?;
    film.click_and_invoke(&tick)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_use_angle()) {
        return Err("clicking Custom Angle did not tick it".into());
    }
    film.hold(pacing::RESULT_HOLD)?;
    drag_scratch_angle(film)?;
    wait_step_done(film, "Custom Angle")
}

/// The step's second action: drags the Angle slider that the ticked Custom Angle box shows half
/// way along its track, so that the tier's scratch ticks turn when the thumb is let go.
fn drag_scratch_angle(film: &mut Film<'_>) -> Result<(), String> {
    const LABEL: &str = "Scratch Angle in Degrees";
    let model = film.ui.global::<SurfaceModel>();
    let before = model.get_angle_deg();
    let slider = film
        .inspector_form_control(LABEL, AccessibleRole::Slider)?
        .ok_or("the Scratch Angle slider is not on screen after Custom Angle was ticked")?;
    let view = film.view();
    let handle = slider
        .handle
        .as_ref()
        .ok_or("the Scratch Angle slider has no accessible element handle")?;
    let thumbs: Vec<targets::Spot> = handle
        .query_descendants()
        .match_id("Slider::thumb")
        .find_all()
        .iter()
        .map(targets::Spot::of)
        .filter(|spot| spot.usable(view.size))
        .collect();
    let [thumb] = thumbs.as_slice() else {
        return Err(format!(
            "the Scratch Angle slider has {} usable rendered thumbs, expected exactly one",
            thumbs.len()
        ));
    };
    if slider.w <= thumb.w {
        return Err("the Scratch Angle slider track is unusable".into());
    }
    let start = thumb.center();
    let end = (
        (slider.w - thumb.w).mul_add(0.5, slider.x + thumb.w / 2.0),
        start.1,
    );
    film.gesture(start, end, None, false, false)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        (model.get_angle_deg() - before).abs() > 1.0
    }) {
        return Err("dragging the Scratch Angle slider did not change the angle".into());
    }
    film.hold(pacing::RESULT_HOLD)
}

/// Chooses `name` in the Stone tab's Material list after earlier steps scrolled the tab down
/// to its effect groups: the tab is scrolled back up to the list first.
fn choose_material_again(film: &mut Film<'_>, name: &str) -> Result<(), String> {
    film.stone_control("the Material list", |view| {
        targets::role_label(view, "Material", AccessibleRole::Combobox, None)
    })?;
    film.film_material(name)
}

// --- The Stone tab --------------------------------------------------------------------------

/// Clicks the Stone tab control called `label`, whatever its role.
///
/// A switch row writes its name twice: as the row's text and as the name of the switch at its
/// right end. A click on the text does nothing, so a control that is not a text is taken first.
fn click_stone_label(film: &mut Film<'_>, label: &str) -> Result<(), String> {
    let spot = film.stone_control(label, |view| {
        let wanted = label.to_owned();
        view.root
            .query_descendants()
            .match_predicate(move |element| {
                element
                    .accessible_label()
                    .is_some_and(|name| name == wanted)
                    && element.accessible_role() != Some(AccessibleRole::Text)
                    && element.accessible_enabled() != Some(false)
            })
            .find_all()
            .iter()
            .map(targets::Spot::of)
            .find(|spot| spot.usable(view.size))
            .or_else(|| targets::run_label(view, label))
    })?;
    film.click_and_invoke(&spot)?;
    film.hold(pacing::RESULT_HOLD)
}

/// Opens the Stone tab's Play-of-Color (Schiller) fold card, which starts folded. Its title is the
/// material's own ("Play-of-Color" or "Schiller") while one is available.
fn open_structure_card(film: &mut Film<'_>) -> Result<(), String> {
    let model = film.ui.global::<RenderEffectsModel>();
    let title = if model.get_structure_available() {
        model.get_structure_title().to_string()
    } else {
        "Play-of-Color".to_owned()
    };
    film.open_stone_fold(&title, 8)
}

/// Clicks the silk preset `name` and waits for the design to hold it.
fn choose_silk_preset(film: &mut Film<'_>, name: &str) -> Result<(), String> {
    let model = film.ui.global::<RenderEffectsModel>();
    // The silk presets are in the Clarity card, which starts folded.
    film.open_stone_fold("Clarity", 4)?;
    if !model.get_inclusion_preset().is_empty() {
        return Err("a silk preset is already chosen before the lesson chooses one".into());
    }
    click_stone_label(film, name)?;
    if model.get_inclusion_preset().as_str() != name {
        return Err(format!("clicking {name} did not choose that preset"));
    }
    wait_step_done(film, name)
}

/// Set Silk Turn, a nudge of the Silk Turn slider and Show Silk Directions.
fn turn_silk(film: &mut Film<'_>) -> Result<(), String> {
    let model = film.ui.global::<RenderEffectsModel>();
    film.open_stone_fold("Clarity", 4)?;
    click_stone_label(film, "Set Silk Turn")?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_inclusion_turn_set()) {
        return Err("Set Silk Turn did not switch the turn on".into());
    }
    film.drag_stone_slider("Silk Turn")?;
    click_stone_label(film, "Show Silk Directions")?;
    if !model.get_show_silk_directions() {
        return Err("Show Silk Directions did not switch on".into());
    }
    wait_step_done(film, "Silk Turn")
}

/// Chrysoberyl, its Crystal Axis and Second Axis, then Cat's-Eye Silk.
fn cats_eye(film: &mut Film<'_>) -> Result<(), String> {
    let model = film.ui.global::<RenderEffectsModel>();
    choose_material_again(film, "Chrysoberyl (Yellow)")?;
    // Sets the axis for this stone first when the stone has none, then the Second Axis.
    second_axis(film)?;
    film.open_stone_fold("Clarity", 4)?;
    click_stone_label(film, "Cat's-Eye Silk")?;
    if model.get_inclusion_preset().as_str() != "Cat's-Eye Silk" {
        return Err("clicking Cat's-Eye Silk did not choose that preset".into());
    }
    wait_step_done(film, "Cat's-Eye Silk")
}

/// The reading step on the switch of the Play-of-Color (Schiller) card: choosing the material
/// switched it on, so the card is opened, the cursor rests on the switch and nothing is clicked.
fn show_structure_switch(film: &mut Film<'_>, label: &str) -> Result<(), String> {
    let model = film.ui.global::<RenderEffectsModel>();
    if !model.get_structure_on() {
        return Err(format!(
            "{label} is off: choosing the material did not switch it on"
        ));
    }
    open_structure_card(film)?;
    let switch = film.stone_control(label, |view| targets::run_label(view, label))?;
    film.glide_to(switch.center())?;
    film.hold(pacing::RESULT_HOLD)
}

/// Drags the Stone tab slider `label` a little along its track and waits for the design.
fn drag_and_finish(film: &mut Film<'_>, label: &str) -> Result<(), String> {
    open_structure_card(film)?;
    film.drag_stone_slider(label)?;
    wait_step_done(film, label)
}

/// The Reseed button: the Seed number changes.
fn reseed(film: &mut Film<'_>) -> Result<(), String> {
    let model = film.ui.global::<RenderEffectsModel>();
    let before = model.get_structure_seed();
    open_structure_card(film)?;
    click_stone_label(film, "Reseed")?;
    if model.get_structure_seed() == before {
        return Err("Reseed did not draw a new seed".into());
    }
    wait_step_done(film, "Reseed")
}

// --- The Material Editor --------------------------------------------------------------------

/// Opens the Material Editor, loads the Sapphire template, names the material, switches to
/// Curves, nudges one coefficient and saves.
fn curve_material(film: &mut Film<'_>) -> Result<(), String> {
    let editor = film.ui.global::<ViewportModel>();
    // New/Edit Material... is an item of the identity card's "..." menu.
    let menu = film.stone_control("Material Menu", |view| {
        targets::role_label(
            view,
            "Material Menu",
            AccessibleRole::Button,
            Some("ParametersPanel"),
        )
    })?;
    film.click_and_invoke(&menu)?;
    let open = film.stone_control("New/Edit Material...", |view| {
        targets::role_label(
            view,
            "New/Edit Material...",
            AccessibleRole::Button,
            Some("ParametersPanel"),
        )
    })?;
    film.click_and_invoke(&open)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || editor.get_material_editor_open()) {
        return Err("Material Editor did not open".into());
    }
    let combo = targets::run_label(&film.view(), "Preset Template")
        .ok_or("the Preset Template list is missing")?;
    film.click_and_invoke(&combo)?;
    for _ in 0..SAPPHIRE_TEMPLATE {
        film.rec
            .press_keys(film.ui.window(), "Down")
            .map_err(|e| format!("Preset Template Down: {e:?}"))?;
    }
    film.rec
        .press_keys(film.ui.window(), "Return")
        .map_err(|e| format!("Preset Template Return: {e:?}"))?;
    let field = targets::text_input(&film.view(), "Material Name")
        .ok_or("the Material Name field is missing")?;
    film.click_and_type(&field, CURVES_NAME)?;
    film.extra_click("Curves").map_err(|why| {
        format!("the Curves switch is not offered (a material that is not isotropic, in Advanced): {why}")
    })?;
    film.hold(pacing::RESULT_HOLD)?;
    let coefficient = targets::text_input_start(&film.view(), "Axis Curve Coefficient")
        .ok_or("the first coefficient field of the curve is missing")?;
    let old = coefficient
        .handle
        .as_ref()
        .and_then(uiprobe::ElementHandle::accessible_value)
        .ok_or("the first coefficient has no readable value")?;
    let old_number: f64 = old
        .parse()
        .map_err(|_| format!("the seeded coefficient {old:?} is not a number"))?;
    film.click_and_type(&coefficient, &format!("{:.7}", old_number + 0.0005))?;
    film.hold(pacing::RESULT_HOLD)?;
    film.extra_click("Save")?;
    if editor.get_material_editor_open() {
        return Err("Material Save did not close the editor".into());
    }
    Ok(())
}
