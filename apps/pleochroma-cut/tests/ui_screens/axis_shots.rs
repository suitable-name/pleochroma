//! The shots of the Crystal Axis: the Stone tab's section (a sapphire and a tanzanite with its
//! Second Axis), the editor with Show Crystal Axis on, the Rough Planner's axis panel, rule line
//! and stone rows, the rough view with the stones' axes, and Cutting Mode's first step with the
//! picture of the axis. Each drives the real app through the model calls its controls make.
//!
//! Not here: the cutting sheet's Orientation block. The sheet is an exported HTML file
//! (`gui::editor::cut_sheet`), not a window of the app, so the harness has nothing to photograph.

use crate::{
    kit::{Rect, Sc, find_inherits, rect_of, size_window},
    nav::{FVS, STUDIO},
    support::{WindowView, app::pump},
    window_shots::planner_with_block,
};
use pleochroma_cut::{
    CrystalAxisModel, CuttingModeModel, EditorModel, MainWindow, ParametersModel, RoughPlanModel,
    RoughPlannerWindow, SettingsModel, ViewportModel,
};
use slint::{ComponentHandle, Model};
use uiprobe::screenshot::Shot;

/// The Rough Planner's size in these shots, logical pixels: tall enough for the axis panel and
/// the stone rows to show without scrolling the left column.
const AXIS_PLANNER_SIZE: (u32, u32) = (1280, 1280);
/// How long a plan may take, in seconds.
const PLAN_SECS: u64 = 900;
/// The tilt and azimuth the overlay and Cutting Mode shots set, degrees: an axis that leans
/// visibly across the stone.
const SHOWN_AXIS: (f32, f32) = (35.0, 40.0);

/// How a shot turns the Crystal Axis, through the controls the Stone tab has.
#[derive(Clone, Copy)]
enum Orient {
    /// "Set for This Stone" with this tilt and azimuth.
    Angles(f32, f32),
    /// A geometry preset (0 Through the Table, 1 In the Table Plane, 2 Along the Width).
    Geometry(i32),
    /// A color-named preset, by its place in the list.
    Color(i32),
}

/// The name of the material option that contains `wanted` (lower case).
fn material_name(ui: &MainWindow, wanted: &str) -> Result<String, String> {
    let options: Vec<String> = ui
        .global::<ViewportModel>()
        .get_material_options()
        .iter()
        .map(String::from)
        .collect();
    options
        .iter()
        .find(|name| name.to_lowercase().contains(wanted))
        .cloned()
        .ok_or_else(|| format!("no material called {wanted:?} among {options:?}"))
}

/// Gives the open design the material containing `wanted` (the Stone tab's Material pick) and
/// waits until the Crystal Axis section is offered for it.
fn apply_design_material(sc: &Sc<'_, '_>, wanted: &str) -> Result<(), String> {
    let ui = sc.ui();
    let name = material_name(&ui, wanted)?;
    let editor = ui.global::<EditorModel>();
    let code = editor.invoke_apply_named_material(name.as_str().into(), 0.0);
    if code != 0 {
        return Err(format!("the design did not take {name} (answer {code})"));
    }
    let _ = Sc::wait(30, || !editor.get_solve_running());
    pump(800);
    let settings = ui.global::<SettingsModel>();
    if !Sc::wait(15, || settings.get_c_axis_override_available()) {
        return Err(format!("{name} offers no crystal axis"));
    }
    sc.ctx.app.clear_notice();
    Ok(())
}

/// Turns the Crystal Axis of whatever is on the viewport the way `how` says, and lets the render
/// context and the section follow.
fn orient(sc: &Sc<'_, '_>, how: Orient) -> Result<(), String> {
    let ui = sc.ui();
    let axis = ui.global::<CrystalAxisModel>();
    match how {
        Orient::Angles(tilt, azimuth) => {
            let settings = ui.global::<SettingsModel>();
            settings.set_c_axis_override_enabled(true);
            settings.invoke_c_axis_override_changed(true);
            settings.set_c_axis_tilt_deg(tilt);
            settings.set_c_axis_azimuth_deg(azimuth);
            settings.invoke_c_axis_angles_changed(tilt, azimuth);
        }
        Orient::Geometry(index) => axis.invoke_geometry_preset_chosen(index),
        Orient::Color(index) => {
            let wanted = usize::try_from(index).unwrap_or(0);
            axis.invoke_refresh();
            if !Sc::wait(15, || axis.get_color_preset_names().row_count() > wanted) {
                return Err("the material offers no color-named presets".to_owned());
            }
            axis.invoke_color_preset_chosen(index);
        }
    }
    pump(800);
    Ok(())
}

/// Scrolls the Parameters panel until the Stone tab's section `component` (for example
/// `CrystalAxisSection`) is at the top of it, and returns the panel's rectangle.
pub fn section_into_view(sc: &Sc<'_, '_>, component: &str) -> Result<Rect, String> {
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
    let mut section = None;
    for _ in 0..40 {
        section = find_inherits(&sc.view(), component);
        if section.is_some() {
            break;
        }
        area.scroll(0.0, -300.0);
        pump(150);
    }
    let section = section.ok_or_else(|| format!("the section {component} never came into view"))?;
    let below = Rect::of(&section).y - Rect::of(&panel).y - 8.0;
    if below > 0.0 {
        area.scroll(0.0, -below);
        pump(300);
    }
    rect_of(&sc.view(), "ParametersDock").or_else(|_| rect_of(&sc.view(), "ParametersPanel"))
}

/// The Stone tab of a library design in `material`, scrolled to the Crystal Axis section, with
/// the axis turned as `how` says.
fn stone_tab(sc: &mut Sc<'_, '_>, material: &str, how: Orient) -> Result<Shot, String> {
    sc.select_design(STUDIO)?;
    sc.choose_material(material)?;
    sc.show_live_view();
    let ui = sc.ui();
    ui.global::<ParametersModel>().invoke_show_tab(0);
    pump(1000);
    let settings = ui.global::<SettingsModel>();
    if !Sc::wait(15, || settings.get_c_axis_override_available()) {
        return Err(format!("{material} offers no crystal axis"));
    }
    orient(sc, how)?;
    let axis = ui.global::<CrystalAxisModel>();
    if !Sc::wait(10, || axis.get_active_preset() >= 0) && !matches!(how, Orient::Angles(..)) {
        sc.note("the Stone tab lights no preset after the choice; photographed anyway");
    }
    pump(1500);
    let panel = section_into_view(sc, "CrystalAxisSection")?;
    let shot = sc.snap_main()?;
    Ok(panel.crop(&shot))
}

/// `crystal_axis_stone_tab.png`: the Stone tab of a sapphire with "Set for This Stone" on and In
/// the Table Plane chosen.
///
/// # Errors
///
/// The design or the material is missing, or the section is not on screen.
pub fn stone_tab_sapphire(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    stone_tab(sc, "sapphire", Orient::Geometry(1))
}

/// `crystal_axis_second_axis.png`: the Stone tab of a tanzanite with Best Blue Face-Up chosen,
/// which sets the Second Axis too.
///
/// # Errors
///
/// As [`stone_tab_sapphire`], or the material has no color-named presets.
pub fn stone_tab_tanzanite(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    stone_tab(sc, "tanzanite", Orient::Color(0))
}

/// The Edit view in Solid shading, the design in `material`, the axis turned as `how` says and
/// Show Crystal Axis on; cropped to the viewport.
fn overlay(sc: &Sc<'_, '_>, material: &str, how: Orient) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    apply_design_material(sc, material)?;
    sc.set_view_mode(0);
    orient(sc, how)?;
    let ui = sc.ui();
    let axis = ui.global::<CrystalAxisModel>();
    axis.set_show_axis(true);
    pump(500);
    if !Sc::wait(30, || axis.get_overlay_visible()) {
        return Err("the crystal axis was not drawn over the stone".to_owned());
    }
    pump(1500);
    let shot = sc.snap_main()?;
    let view = sc.view();
    let viewport =
        rect_of(&view, "SolidViewportView").or_else(|_| rect_of(&view, "GemViewportView"))?;
    Ok(viewport.crop(&shot))
}

/// `crystal_axis_overlay.png`: a sapphire in the Edit view with the crystal axis drawn over it and
/// its handle at the upper end.
///
/// # Errors
///
/// The design or the material is missing, or the axis is not drawn.
pub fn overlay_sapphire(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    overlay(sc, "sapphire", Orient::Angles(SHOWN_AXIS.0, SHOWN_AXIS.1))
}

/// `crystal_axis_overlay_second.png`: a tanzanite at Best Blue Face-Up with both handles, the
/// Crystal Axis and the Second Axis.
///
/// # Errors
///
/// As [`overlay_sapphire`], or the material has no color-named presets.
pub fn overlay_tanzanite(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    overlay(sc, "tanzanite", Orient::Color(0))
}

/// Cutting Mode on its first step for a sapphire whose axis is set for the stone: the page shows
/// the Orientation note and the picture of the axis.
///
/// # Errors
///
/// The design or the material is missing, or Cutting Mode shows no picture of the axis.
pub fn cutting_mode_axis(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    apply_design_material(sc, "sapphire")?;
    orient(sc, Orient::Angles(SHOWN_AXIS.0, SHOWN_AXIS.1))?;
    // The axis is committed to the design after the debounce; Cutting Mode reads the design.
    pump(2000);
    let ui = sc.ui();
    let cutting = ui.global::<CuttingModeModel>();
    cutting.invoke_open_cutting_mode();
    let ready = Sc::wait(30, || {
        !cutting.get_busy() && (cutting.get_step_count() > 0 || !cutting.get_error().is_empty())
    });
    if !ready || !cutting.get_error().is_empty() {
        return Err(format!(
            "cutting mode did not open: {}",
            cutting.get_error()
        ));
    }
    if !Sc::wait(10, || cutting.get_axis_shown()) {
        return Err("the first step shows no picture of the crystal axis".to_owned());
    }
    pump(800);
    sc.snap_main()
}

/// The Rough Planner on the sapphire block of `rough_planner.png` with the rough's crystal axis
/// set by Fit to Crystal Length, in a window tall enough for the left column.
fn planner_with_axis(sc: &mut Sc<'_, '_>) -> Result<RoughPlannerWindow, String> {
    let window = planner_with_block(sc)?;
    let model = window.global::<RoughPlanModel>();
    if !Sc::wait(15, || model.get_axis_available()) {
        return Err("the Rough Planner offers no crystal axis for the sapphire rough".to_owned());
    }
    model.invoke_axis_fit_length();
    if !Sc::wait(15, || model.get_axis_set()) {
        return Err(format!(
            "the crystal axis of the rough was not set: {}",
            model.get_axis_note()
        ));
    }
    pump(800);
    if model.get_rule_line().is_empty() {
        sc.note("the planner shows no rule line for the sapphire; photographed anyway");
    }
    size_window(&window, AXIS_PLANNER_SIZE);
    Ok(window)
}

/// `crystal_axis_planner_axis.png`: the Rough Planner with the Crystal Axis panel (the axis set,
/// the ways to find it) and the Orientation Rule with its sentence.
///
/// # Errors
///
/// The planner does not open, or the axis cannot be set.
pub fn planner_axis(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    let window = planner_with_axis(sc)?;
    pump(1000);
    sc.snap_window("RoughPlannerWindow", &window)
}

/// The sapphire block planned with the axis set (at most two designs per layout) and the best
/// layout selected, once its stone rows are listed.
fn planned_with_axis(sc: &mut Sc<'_, '_>) -> Result<RoughPlannerWindow, String> {
    let window = planner_with_axis(sc)?;
    let model = window.global::<RoughPlanModel>();
    model.set_max_designs(2);
    pump(300);
    model.invoke_plan();
    pump(500);
    let done = Sc::wait(PLAN_SECS, || {
        !model.get_running()
            && (model.get_results().row_count() > 0 || !model.get_error_text().is_empty())
    });
    if !done {
        return Err(format!(
            "the plan did not finish within {PLAN_SECS} s: {}",
            model.get_stage()
        ));
    }
    if !model.get_error_text().is_empty() {
        return Err(format!("the plan failed: {}", model.get_error_text()));
    }
    model.invoke_select_result(0);
    pump(500);
    let _ = Sc::wait(120, || {
        !model.get_thumbnails_pending() && !model.get_view_busy()
    });
    if !Sc::wait(30, || model.get_stone_rows().row_count() > 0) {
        return Err("the selected layout lists no stone rows".to_owned());
    }
    pump(1500);
    Ok(window)
}

/// `crystal_axis_planner_stones.png`: the planner after Plan with the best layout selected: each
/// stone's axis line, its rule mark and its menu.
///
/// # Errors
///
/// As [`planner_axis`], or the plan fails or lists no stone rows.
pub fn planner_stones(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    let window = planned_with_axis(sc)?;
    sc.snap_window("RoughPlannerWindow", &window)
}

/// `crystal_axis_planner_view.png`: the 3D view of the planned layout with the crystal axis of
/// the rough and of each stone drawn in it.
///
/// # Errors
///
/// As [`planner_stones`], or the view is not on screen.
pub fn planner_view(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    let window = planned_with_axis(sc)?;
    let shot = sc.snap_window("RoughPlannerWindow", &window)?;
    let view = WindowView::of("RoughPlannerWindow", &window);
    Ok(rect_of(&view, "FitView")?.crop(&shot))
}
