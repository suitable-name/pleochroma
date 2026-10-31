//! Films only real controls and pointer inputs; never injects tutorial completion events.
use super::pure_tables::output_path;
use crate::{
    debug_log::{debug_check, debug_check_with},
    director::Film,
    pacing,
    support::app::pump,
    targets::{self, Query, Spot},
};
use pleochroma_cut::{ExportModel, OpticalModel};
use slint::ComponentHandle;
use std::time::Duration;

fn panel_control(
    film: &mut Film<'_>,
    label: &str,
    role: uiprobe::AccessibleRole,
) -> Result<Spot, String> {
    for _ in 0..20 {
        let view = film.view();
        let panel = view
            .root
            .query_descendants()
            .match_inherits("OpticalPanel")
            .find_first()
            .ok_or("Optical Diagnostics panel is not open")?;
        let scroll = panel
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .ok_or("Optical Diagnostics scroll view is missing")?;
        let clip = Spot::of(&scroll)
            .clipped_to(
                &Spot {
                    x: 0.0,
                    y: 0.0,
                    w: view.size.0,
                    h: view.size.1,
                    handle: None,
                },
                view.size,
            )
            .ok_or("Optical Diagnostics scroll view is outside the window")?;
        let found: Vec<_> = panel
            .query_descendants()
            .match_accessible_role(role)
            .find_all()
            .into_iter()
            .filter(|e| e.accessible_label().is_some_and(|s| s == label))
            .collect();
        if found.len() != 1 {
            return Err(format!(
                "OpticalPanel control {label:?} ({role:?}): expected one, found {}",
                found.len()
            ));
        }
        let target = Spot::of(&found[0]);
        if target
            .clipped_to(&clip, view.size)
            .is_some_and(|v| v.h >= target.h * 0.75 && v.w >= target.w * 0.75)
        {
            return Ok(target);
        }
        let delta = if target.y + target.h > clip.y + clip.h {
            -100.0
        } else {
            100.0
        };
        let safe = Spot {
            x: clip.x + 3.0,
            y: clip.h.mul_add(0.5, clip.y),
            w: 3.0,
            h: 3.0,
            handle: None,
        };
        film.wheel_over(&safe, delta)?;
    }
    Err(format!(
        "OpticalPanel control {label:?} did not become visible after bounded scrolling"
    ))
}

fn ensure_layer(
    film: &mut Film<'_>,
    label: &str,
    wanted: bool,
    current: bool,
) -> Result<(), String> {
    if current == wanted {
        return Ok(());
    }
    let target = panel_control(film, label, uiprobe::AccessibleRole::Checkbox)?;
    film.click_and_invoke(&target)?;
    let optical = film.ui.global::<OpticalModel>();
    let actual = match label {
        "Enable Optical Diagnostics" => optical.get_enabled(),
        "Windowing (/)" => optical.get_windowing(),
        "Extinction (X)" => optical.get_extinction(),
        "Brilliance (Dots)" => optical.get_brilliance(),
        "Fire (Dispersion Potential)" => optical.get_fire(),
        "Diagnostic Map on Neutral Stone" => optical.get_map_mode(),
        _ => return Err(format!("no OpticalModel readback for {label:?}")),
    };
    if actual != wanted {
        return Err(format!(
            "{label:?} checkbox readback was {actual}, wanted {wanted}"
        ));
    }
    let checked = film
        .view()
        .root
        .query_descendants()
        .match_inherits("OpticalPanel")
        .find_first()
        .and_then(|panel| {
            panel
                .query_descendants()
                .match_accessible_role(uiprobe::AccessibleRole::Checkbox)
                .find_all()
                .into_iter()
                .find(|e| e.accessible_label().is_some_and(|s| s == label))
        })
        .and_then(|e| e.accessible_checked());
    if checked.is_some_and(|value| value != wanted) {
        return Err(format!(
            "{label:?} accessible checked state disagrees with model"
        ));
    }
    Ok(())
}

fn panel_button(film: &mut Film<'_>, label: &str) -> Result<(), String> {
    let target = panel_control(film, label, uiprobe::AccessibleRole::Button)?;
    film.click_and_invoke(&target)
}

const OPTICAL_BUTTON: &str = "Optical Diagnostics";

/// Why the toolbar button cannot be clicked: where every element carrying its label sits against
/// the toolbar row and the window (a button pushed past the row end or the window edge is
/// clipped, the button only exists in Path-Traced mode), so the next run names the cause.
fn optical_button_report(film: &Film<'_>) -> String {
    let view = film.view();
    let viewport = film.ui.global::<pleochroma_cut::ViewportModel>();
    let row = view
        .root
        .query_descendants()
        .match_inherits("GemViewportToolbarActions")
        .find_first();
    let labelled: Vec<String> = row
        .iter()
        .flat_map(|row| row.query_descendants().find_all())
        .filter(|e| {
            e.accessible_label()
                .is_some_and(|text| text.starts_with("Optical"))
        })
        .map(|e| {
            let spot = Spot::of(&e);
            format!(
                "{:?} at ({:.0}, {:.0}) size {:.0} x {:.0}",
                e.accessible_label(),
                spot.x,
                spot.y,
                spot.w,
                spot.h
            )
        })
        .collect();
    let row_bounds = row.as_ref().map(|row| {
        let spot = Spot::of(row);
        format!(
            "({:.0}, {:.0}) size {:.0} x {:.0}",
            spot.x, spot.y, spot.w, spot.h
        )
    });
    format!(
        "control {OPTICAL_BUTTON:?} is not visible: live_view_mode={} (the button exists only in 1, Path-Traced), previewing={}, window={:.0} x {:.0}, toolbar actions row={row_bounds:?}, elements labelled Optical*={labelled:?}",
        viewport.get_live_view_mode(),
        viewport.get_previewing(),
        view.size.0,
        view.size.1
    )
}

fn ensure_panel(film: &mut Film<'_>, wanted: bool) -> Result<(), String> {
    let current = film.ui.global::<OpticalModel>().get_panel_open();
    if current != wanted {
        // The toolbar re-lays out after a mode switch, a resize of the dock or the Previewing
        // badge appearing; give it a moment before calling the button missing.
        let found = pacing::wait_real(Duration::from_secs(3), || {
            !film
                .scoped_spots("GemViewportToolbarActions", OPTICAL_BUTTON)
                .is_empty()
        });
        if !found {
            return Err(optical_button_report(film));
        }
        let button = film.scoped_button("GemViewportToolbarActions", OPTICAL_BUTTON)?;
        film.click_and_invoke(&button)?;
    }
    let actual = film.ui.global::<OpticalModel>().get_panel_open();
    if actual != wanted {
        return Err(format!(
            "Optical Diagnostics toolbar readback was {actual}, wanted {wanted}"
        ));
    }
    Ok(())
}
fn wait_map(film: &Film<'_>) -> Result<(), String> {
    // Generous: the first map of a process waits for the first beauty frame, which on a cold
    // machine includes compiling the GPU tracer.
    if pacing::wait_real(Duration::from_secs(120), || {
        let optical = film.ui.global::<OpticalModel>();
        optical.get_enabled() && optical.get_ready()
    }) {
        Ok(())
    } else {
        Err("Matching optical map did not arrive".into())
    }
}

/// Waits for the layer picture to differ from `before` once a ready map is showing. A layer
/// toggle no longer blanks the overlay (the old map stays ready and the controller republishes
/// within one tick), so readiness alone would return at once with the old picture.
fn wait_new_layer(film: &Film<'_>, before: &slint::Image) -> Result<(), String> {
    if pacing::wait_real(Duration::from_secs(30), || {
        let optical = film.ui.global::<OpticalModel>();
        optical.get_ready() && optical.get_image() != *before
    }) {
        Ok(())
    } else {
        Err("The optical layer did not change after the layer toggle".into())
    }
}

/// Finds deterministic points on the displayed map, beyond the diagnostics panel and toolbar.
fn optical_hover_points(film: &Film<'_>) -> Result<Vec<(f32, f32)>, String> {
    let view = film.view();
    let viewport = view
        .root
        .query_descendants()
        .match_inherits("GemViewportView")
        .find_first()
        .ok_or("Live viewport is missing for optical hover readback")?;
    let viewport = Spot::of(&viewport);
    let aspect = film.ui.global::<OpticalModel>().get_aspect();
    if !aspect.is_finite() || aspect <= 0.0 {
        return Err(format!("optical map has invalid display aspect {aspect}"));
    }
    let image_w = viewport.w.min(viewport.h * aspect);
    let image_h = image_w / aspect;
    let image_x = viewport.x + (viewport.w - image_w) / 2.0;
    let image_y = viewport.y + (viewport.h - image_h) / 2.0;
    let optical_panels = view
        .root
        .query_descendants()
        .match_inherits("OpticalPanel")
        .find_all();
    if optical_panels.is_empty() {
        return Err("Optical Diagnostics panel is open in the model but not visible".into());
    }
    let panel_right = optical_panels
        .iter()
        .map(Spot::of)
        .map(|panel| panel.x + panel.w + 8.0)
        .fold(image_x, f32::max);
    let mut overlays: Vec<_> = optical_panels.iter().map(Spot::of).collect();
    let guides: Vec<_> = view
        .root
        .query_descendants()
        .match_inherits("GuidePanel")
        .find_all()
        .iter()
        .map(Spot::of)
        .collect();
    overlays.extend(guides);
    let toolbar_actions: Vec<_> = view
        .root
        .query_descendants()
        .match_inherits("GemViewportToolbarActions")
        .find_all()
        .iter()
        .map(Spot::of)
        .collect();
    overlays.extend(toolbar_actions.iter().cloned());
    let toolbar = targets::run(&view, &Query::Id("GemViewportView::toolbar"))
        .ok_or("Live viewport toolbar bounds are missing")?;
    let toolbar_bottom = toolbar_actions
        .iter()
        .map(|spot| spot.y + spot.h)
        .chain(std::iter::once(toolbar.y + toolbar.h))
        .fold(image_y, f32::max);
    overlays.push(toolbar);
    let left = image_w.mul_add(0.48, image_x).max(panel_right);
    let right = image_x + image_w - 8.0;
    let top = image_h.mul_add(0.20, image_y).max(toolbar_bottom + 8.0);
    let bottom = image_h.mul_add(0.76, image_y);
    if right <= left || bottom <= top {
        return Err("Optical image has no visible center/right area clear of its overlays".into());
    }
    let mut points = Vec::new();
    for y_fraction in [0.20_f32, 0.40, 0.60, 0.80] {
        for x_fraction in [0.12_f32, 0.38, 0.64, 0.88] {
            let point = (
                (right - left).mul_add(x_fraction, left),
                (bottom - top).mul_add(y_fraction, top),
            );
            if !overlays.iter().any(|spot| {
                point.0 >= spot.x - 4.0
                    && point.0 <= spot.x + spot.w + 4.0
                    && point.1 >= spot.y - 4.0
                    && point.1 <= spot.y + spot.h + 4.0
            }) {
                points.push(point);
            }
        }
    }
    Ok(points)
}

/// Searches a bounded image grid with real pointer moves and verifies the visible model readout.
fn read_optical_hover(film: &mut Film<'_>) -> Result<(), String> {
    let optical = film.ui.global::<OpticalModel>();
    if !optical.get_panel_open() || !optical.get_ready() {
        return Err("Optical hover readback requires the open panel and a ready map".into());
    }
    let points = optical_hover_points(film)?;
    let mut last = String::new();
    for point in points.iter().copied() {
        let previous = optical.get_hover_text().to_string();
        film.glide_to(point)?;
        pump(30);
        let _ = pacing::wait_real(Duration::from_millis(300), || {
            optical.get_hover_text().as_str() != previous
        });
        last = optical.get_hover_text().to_string();
        let readout = !last.is_empty()
            && !last.contains("No Center-Ray Hit")
            && (last.contains(" (Center Ray)") || last.contains(" (Mixed Subpixel Surfaces)"))
            && last.contains(" | Exit ")
            && last.contains(" Reflections ");
        // A point without a centre-ray hit is part of the search, not a mismatch: the route takes
        // the first point that gives a full readout.
        debug_check_with(
            if readout {
                "optical hover text: a full centre-ray readout"
            } else {
                "optical hover text: search point without a full readout, next point"
            },
            &"a centre-ray readout with its exit and reflections",
            &last,
            true,
        );
        if readout {
            film.hold(Duration::from_secs(3))?;
            return Ok(());
        }
    }
    Err(format!(
        "No bounded optical hover point produced an entry, exit, and reflections readout ({} points; final text={last:?})",
        points.len()
    ))
}

#[expect(clippy::too_many_lines, reason = "one arm per filmed step")]
pub(super) fn drive(film: &mut Film<'_>, step: usize) -> Result<(), String> {
    match step {
        0 => {
            film.film_command("view.live_render")?;
            let path_traced = targets::role_label(
                &film.view(),
                "Path-Traced",
                uiprobe::AccessibleRole::RadioButton,
                Some("GemViewportView"),
            )
            .ok_or("Live Render Path-Traced mode pill is missing")?;
            film.click_and_invoke(&path_traced)?;
            if film
                .ui
                .global::<pleochroma_cut::ViewportModel>()
                .get_live_view_mode()
                != 1
            {
                return Err(format!(
                    "Path-Traced mode pill did not select mode 1; actual mode={}",
                    film.ui
                        .global::<pleochroma_cut::ViewportModel>()
                        .get_live_view_mode()
                ));
            }
            film.rec
                .caption("Path-Traced mode is active for optical diagnostics.");
            film.hold(pacing::RESULT_HOLD)?;
        }
        1 => {
            ensure_panel(film, true)?;
            let enabled = film.ui.global::<OpticalModel>().get_enabled();
            ensure_layer(film, "Enable Optical Diagnostics", true, enabled)?;
            if !film.ui.global::<OpticalModel>().get_enabled() {
                return Err("Enable Optical Diagnostics did not read back as enabled".into());
            }
            wait_map(film)?;
        }
        2 => {
            let before = film.ui.global::<OpticalModel>().get_image();
            let windowing = film.ui.global::<OpticalModel>().get_windowing();
            ensure_layer(film, "Windowing (/)", false, windowing)?;
            let extinction = film.ui.global::<OpticalModel>().get_extinction();
            ensure_layer(film, "Extinction (X)", true, extinction)?;
            wait_new_layer(film, &before)?;
            film.hold(pacing::RESULT_HOLD)?;
            let before = film.ui.global::<OpticalModel>().get_image();
            let extinction = film.ui.global::<OpticalModel>().get_extinction();
            ensure_layer(film, "Extinction (X)", false, extinction)?;
            let brilliance = film.ui.global::<OpticalModel>().get_brilliance();
            ensure_layer(film, "Brilliance (Dots)", true, brilliance)?;
            wait_new_layer(film, &before)?;
            film.hold(pacing::RESULT_HOLD)?;
            let fire = film.ui.global::<OpticalModel>().get_fire();
            ensure_layer(film, "Fire (Dispersion Potential)", true, fire)?;
            // The visible refresh clears readiness synchronously, so an older
            // base-only map cannot satisfy this Fire demonstration's wait.
            panel_button(film, "Refresh Diagnostic Map")?;
            wait_map(film)?;
            film.hold(pacing::RESULT_HOLD)?;
        }
        3 => {
            ensure_panel(film, false)?;
            if film.ui.global::<OpticalModel>().get_panel_open() {
                return Err(
                    "Optical Diagnostics panel did not close before the camera orbit".into(),
                );
            }
            let element = film
                .view()
                .root
                .query_descendants()
                .match_inherits("GemViewportView")
                .find_first()
                .ok_or("Live viewport is missing")?;
            let spot = targets::Spot::of(&element);
            let start = (spot.w.mul_add(0.55, spot.x), spot.h.mul_add(0.48, spot.y));
            film.gesture(start, (start.0 + 50.0, start.1 + 15.0), None, false, false)?;
            wait_map(film)?;
            ensure_panel(film, true)?;
            if !film.ui.global::<OpticalModel>().get_panel_open() {
                return Err(
                    "Optical Diagnostics panel did not reopen after the camera orbit".into(),
                );
            }
            // The orbit invalidates and replaces the map. Re-query the viewport geometry only
            // after that map is ready and the overlay panel has been mounted again.
            wait_map(film)?;
            read_optical_hover(film)?;
        }
        4 => {
            ensure_panel(film, false)?;
            film.film_command("tools.export_render")?;
            film.press_label("Include Optical Diagnostics (4 Samples/Pixel)")?;
            film.press_label("Custom")?;
            for label in ["Width in Pixels", "Height in Pixels"] {
                // These are SpinBoxes: one Spinbox node, whose editor takes focus on a click.
                let field = targets::role_label(
                    &film.view(),
                    label,
                    uiprobe::AccessibleRole::Spinbox,
                    None,
                )
                .ok_or_else(|| format!("{label} is missing"))?;
                film.click_and_type(&field, "128")?;
                film.rec
                    .press_keys(film.ui.window(), "Return")
                    .map_err(|e| format!("{label} Return: {e:?}"))?;
                film.hold(Duration::from_millis(80))?;
            }
            let export = film.ui.global::<ExportModel>();
            debug_check("export custom width", &128, &export.get_custom_width());
            debug_check("export custom height", &128, &export.get_custom_height());
            if export.get_custom_width() != 128 || export.get_custom_height() != 128 {
                return Err(format!(
                    "custom export size read back {}x{}, expected 128x128",
                    export.get_custom_width(),
                    export.get_custom_height()
                ));
            }
            let folder = output_path(&film.id, "optical");
            std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
            if std::fs::read_dir(&folder)
                .map_err(|e| e.to_string())?
                .next()
                .is_some()
            {
                return Err("Tutorial output folder must be empty".into());
            }
            pleochroma_cut::harness_support::queue_picker_path(folder.clone());
            film.press_label("Start Export")?;
            let artifacts = || {
                std::fs::read_dir(&folder).is_ok_and(|files| {
                    let files: Vec<_> = files.filter_map(Result::ok).map(|e| e.path()).collect();
                    files
                        .iter()
                        .any(|p| p.extension().is_some_and(|s| s == "png"))
                        && files
                            .iter()
                            .any(|p| p.to_string_lossy().ends_with(".optical.txt"))
                })
            };
            if !pacing::wait_real(Duration::from_secs(120), || {
                artifacts() && !film.ui.global::<ExportModel>().get_is_exporting()
            }) {
                return Err("Diagnostic PNG and companion legend did not complete".into());
            }
            if pleochroma_cut::harness_support::pending_picker_answers() != 0 {
                return Err("Diagnostic export did not consume its folder picker answer".into());
            }
            if film.ui.global::<ExportModel>().get_has_error() {
                return Err("Diagnostic export reported an error".into());
            }
        }
        _ => return Err("Unknown optical tutorial step".into()),
    }
    Ok(())
}
