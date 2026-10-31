//! Small row readback helper kept outside `perform_routes.rs` to keep that harness file compact.

use crate::{
    debug_log::debug_check,
    director::Film,
    pacing,
    perform_inputs::FIELD_PAUSE,
    support::{ElementHandle, app::pump},
    targets::{self, Spot},
};
use pleochroma_cut::{EditorModel, GuideModel, MainWindow};
use slint::{ComponentHandle, LogicalPosition, Model, platform::WindowEvent};
use std::time::Duration;

/// What a tier row shows: name, angle, indices, Meets kind and its text.
pub(super) type TierRowValues = (String, String, String, i32, String);

/// The values of tier row `at` of the tier table.
pub(super) fn tier_row_values(ui: &MainWindow, at: i32) -> Option<TierRowValues> {
    let at = usize::try_from(at).ok()?;
    ui.global::<EditorModel>()
        .get_tiers()
        .row_data(at)
        .map(|row| {
            (
                row.name.to_string(),
                row.angle_full.to_string(),
                row.indices_full.to_string(),
                row.constraint_kind,
                row.constraint_text.to_string(),
            )
        })
}

/// Finds a tier row's NAME cell by its displayed label: the first text with that label to the
/// right of the ANGLE cell (`after_x`, its right edge). The CODE cell left of ANGLE carries the
/// block as its label ("G1" reads "Girdle"), so the Girdle tier has two "Girdle" texts.
pub(super) fn name_cell(row: &ElementHandle, label: &str, after_x: f32) -> Option<Spot> {
    row.query_descendants()
        .match_accessible_role(uiprobe::AccessibleRole::Text)
        .find_all()
        .into_iter()
        .filter(|element| element.accessible_label().is_some_and(|text| text == label))
        .map(|element| Spot::of(&element))
        .filter(|spot| spot.x >= after_x - 1.0)
        .min_by(|left, right| left.x.total_cmp(&right.x))
}

/// Finds the tier row's ANGLE button (its accessible text starts with `Angle `).
pub(super) fn angle_cell(row: &ElementHandle) -> Option<Spot> {
    row.query_descendants()
        .match_accessible_role(uiprobe::AccessibleRole::Button)
        .find_all()
        .into_iter()
        .find(|element| {
            element
                .accessible_label()
                .is_some_and(|text| text.starts_with("Angle "))
        })
        .map(|element| Spot::of(&element))
}

/// Returns a center hit point only when enough of the NAME cell remains visibly clickable.
pub(super) fn visible_name_point(cell: &Spot, safe: &Spot) -> Option<(f32, f32)> {
    let left = cell.x.max(safe.x) + 2.0;
    let right = (cell.x + cell.w).min(safe.x + safe.w) - 2.0;
    let top = cell.y.max(safe.y) + 2.0;
    let bottom = (cell.y + cell.h).min(safe.y + safe.h) - 2.0;
    if right - left < 24.0 || bottom - top < 8.0 {
        return None;
    }
    // The centre of the visible band: a user clicks what can be seen.
    Some((f32::midpoint(left, right), f32::midpoint(top, bottom)))
}

/// A tier row's visible name and angle cell centers, clear of the table's horizontal bar.
pub(super) fn visible_row_centers(
    name: &Spot,
    angle: &Spot,
    safe: &Spot,
) -> Option<((f32, f32), (f32, f32))> {
    let name_point = visible_name_point(name, safe)?;
    let angle_point = visible_name_point(angle, safe)?;
    Some((name_point, angle_point))
}

/// Clicks the NAME cell of the tier row `name` and requires the table's selection to follow.
/// A click that did not select is repeated once (with the pointer placed first); the error of a
/// second failure names the point, the row, the safe viewport, the guide lock and what lies there.
pub(super) fn click_tier_row(film: &mut Film<'_>, name: &str) -> Result<i32, String> {
    let mut notes = Vec::new();
    for attempt in 0..2 {
        let (idx, point) = reveal_tier_row(film, name)?;
        let window = film.ui.window();
        if attempt > 0 {
            window.dispatch_event(WindowEvent::PointerMoved {
                position: LogicalPosition::new(point.0, point.1),
            });
            pump(40);
        }
        film.rec.click_at(window, point, (0.0, 0.0))?;
        film.cursor = point;
        let editor = film.ui.global::<EditorModel>();
        let followed = pacing::wait_real(Duration::from_millis(800), || {
            editor.get_selected_tier_index() == idx
        });
        debug_check(
            "selected tier row after the click",
            &idx,
            &editor.get_selected_tier_index(),
        );
        if followed {
            film.rec.hold(FIELD_PAUSE)?;
            return Ok(idx);
        }
        let guide = film.ui.global::<GuideModel>();
        notes.push(format!(
            "attempt {}: clicked ({:.0}, {:.0}), selected row stayed {}, row spot {:?}, safe \
             viewport {:?}, locked hint {:?}, tier table allowed {}, at the point: [{}]",
            attempt + 1,
            point.0,
            point.1,
            editor.get_selected_tier_index(),
            film.tier_row_spot(idx)
                .map(|row| (row.x, row.y, row.w, row.h)),
            film.safe_tier_table_viewport()
                .map(|safe| (safe.x, safe.y, safe.w, safe.h)),
            guide.get_locked_hint(),
            guide.invoke_allows_tier_table(),
            film.describe_at(point),
        ));
    }
    Err(format!(
        "tier {name:?}: selection did not change to its row: {}",
        notes.join(" // ")
    ))
}

/// Commits the Tier form with its pinned `label` button ("Add Tier" or "Save Tier"). An added
/// tier must show up in the table: when the click does not add it, the click is repeated once and
/// then Return is pressed in a form field (which saves too), each only while the table still has
/// `tiers_before` rows, so a tier is never added twice.
pub(super) fn commit_tier_form(
    film: &mut Film<'_>,
    label: &str,
    adding: bool,
    tiers_before: usize,
) -> Result<(), String> {
    let editor = film.ui.global::<EditorModel>();
    let find = |film: &Film<'_>| {
        targets::role_label(
            &film.view(),
            label,
            uiprobe::AccessibleRole::Button,
            Some("EditorInspector"),
        )
    };
    let Some(commit) = find(film) else {
        let seen: Vec<String> = film
            .view()
            .by_label(label)
            .iter()
            .map(|element| {
                format!(
                    "{:?} enabled={:?}",
                    element.accessible_role(),
                    element.accessible_enabled()
                )
            })
            .collect();
        return Err(format!(
            "tier form commit {label:?} is not visible and enabled; elements with that label: \
             {seen:?}; form error {:?}; locked hint {:?}; inspector: [{}]",
            editor.get_tier_form_error(),
            film.ui.global::<GuideModel>().get_locked_hint(),
            film.inspector_dump()
        ));
    };
    film.click_pinned_commit(&commit)?;
    pump(80);
    if !adding {
        return Ok(());
    }
    let added = || editor.get_tiers().row_count() == tiers_before + 1;
    if pacing::wait_real(Duration::from_millis(1200), added) {
        return Ok(());
    }
    if editor.get_tiers().row_count() == tiers_before
        && editor.get_tier_form_error().is_empty()
        && let Some(again) = find(film)
    {
        film.click_pinned_commit(&again)?;
        if pacing::wait_real(Duration::from_millis(1200), added) {
            return Ok(());
        }
    }
    if editor.get_tiers().row_count() == tiers_before && editor.get_tier_form_error().is_empty() {
        for field in ["Index Positions", "Facet Name", "Angle in Degrees"] {
            if let Some(spot) = film.input_spot(field)? {
                film.click_and_invoke(&spot)?;
                film.rec
                    .press_keys(film.ui.window(), "Return")
                    .map_err(|e| format!("{label}: Return in {field}: {e:?}"))?;
                if pacing::wait_real(Duration::from_millis(1200), added) {
                    return Ok(());
                }
                break;
            }
        }
    }
    Err(format!(
        "{label}: unexpected tier count after the real form commit: before={tiers_before}, \
         after={}, selected row {}, form_error={:?}, commit button {:?}, guide_waiting={:?}",
        editor.get_tiers().row_count(),
        editor.get_selected_tier_index(),
        editor.get_tier_form_error(),
        find(film).map(|button| (button.x, button.y, button.w, button.h)),
        film.ui.global::<GuideModel>().get_current().waiting
    ))
}

/// Types `scale` into the Exact Scale Value field of the Tier form that the tier table's Add
/// Anchor opened (which already reads Meets: Exact Scale Value) and saves the tier with the
/// form's pinned Save Tier button.
pub(super) fn type_scale_and_save(film: &mut Film<'_>, scale: &str) -> Result<(), String> {
    let editor = film.ui.global::<EditorModel>();
    let meets = film
        .inspector_form_control("Meets", uiprobe::AccessibleRole::Combobox)?
        .ok_or("the Meets selector of the Tier form is not visible")?;
    let shown = meets
        .handle
        .as_ref()
        .and_then(ElementHandle::accessible_value)
        .unwrap_or_default();
    if !shown.contains("Exact Scale Value") {
        return Err(format!(
            "Add Anchor did not open the Tier form on Exact Scale Value: Meets reads {shown:?}"
        ));
    }
    let field = film
        .input_spot("Exact Scale Value")?
        .ok_or("the Exact Scale Value field of the Tier form is not visible")?;
    film.click_and_type(&field, scale)?;
    let tiers_before = editor.get_tiers().row_count();
    commit_tier_form(film, "Save Tier", false, tiers_before)?;
    if editor.get_tiers().row_count() != tiers_before || !editor.get_tier_form_error().is_empty() {
        return Err(format!(
            "Save Tier on the anchor: {} rows before, {} after, form error {:?}",
            tiers_before,
            editor.get_tiers().row_count(),
            editor.get_tier_form_error()
        ));
    }
    Ok(())
}

/// Scrolls the tier table until the row's NAME and ANGLE text centers are fully visible.
/// Returns the source model index and a safe name-cell click point without selecting it.
#[expect(
    clippy::too_many_lines,
    reason = "one scroll-and-reveal loop step by step"
)]
pub(super) fn reveal_tier_row(
    film: &mut Film<'_>,
    name: &str,
) -> Result<(i32, (f32, f32)), String> {
    let editor = film.ui.global::<EditorModel>();
    if editor.get_tier_table_collapsed() {
        let header = targets::run_label_start(&film.view(), "TIERS (")
            .ok_or("collapsed tier-table header is not visible")?;
        film.click_and_invoke(&header)?;
        pump(50);
    }
    let wanted = name.trim();
    let tiers = editor.get_tiers();
    let mut found_idx = None;
    for i in 0..tiers.row_count() {
        if let Some(row) = tiers.row_data(i) {
            let matches = row.name.trim().eq_ignore_ascii_case(wanted)
                || row
                    .name
                    .split('/')
                    .any(|part| part.trim().eq_ignore_ascii_case(wanted))
                || row.code.trim().eq_ignore_ascii_case(wanted);
            if matches {
                found_idx = i32::try_from(i).ok();
                break;
            }
        }
    }
    let idx = found_idx.ok_or_else(|| format!("tier {wanted:?} not found in tier table"))?;
    let row_data = tiers
        .row_data(usize::try_from(idx).map_err(|_| "invalid row index")?)
        .ok_or_else(|| format!("tier {wanted:?}: row data disappeared"))?;
    let display_name = if row_data.name.trim().is_empty() {
        "(unnamed)".to_owned()
    } else {
        row_data.name.to_string()
    };
    let prefix = format!("Tier {},", idx + 1);
    let viewport = film.safe_tier_table_viewport()?;
    film.wheel_across(&viewport, 3000.0)?;
    film.wheel_over(&viewport, 3000.0)?;
    film.rec.hold(Duration::from_millis(200))?;
    pump(60);

    let mut last_row_y: Option<f32> = None;
    let mut last_bounds = None;
    for _ in 0..24 {
        let view = film.view();
        let viewport = film.safe_tier_table_viewport()?;
        let Some(row) = targets::run_label_start(&view, &prefix) else {
            // Rows out of view leave the tree: turn towards where the row was seen last (up
            // when it went out above the table), else down.
            let above = last_bounds.is_some_and(|(_, y, _, _)| y < viewport.y);
            film.wheel_over(&viewport, if above { 100.0 } else { -100.0 })?;
            film.rec.hold(Duration::from_millis(200))?;
            pump(60);
            continue;
        };
        last_bounds = Some((row.x, row.y, row.w, row.h));
        let row_element = row.handle.as_ref().ok_or("tier row handle is missing")?;
        let angle_cell = angle_cell(row_element)
            .ok_or_else(|| format!("tier {wanted:?}: ANGLE cell is missing"))?;
        let name_cell = name_cell(row_element, &display_name, angle_cell.x + angle_cell.w)
            .ok_or_else(|| format!("tier {wanted:?}: NAME cell is missing"))?;
        crate::debug_log::debug_check_with(
            "tier row NAME / ANGLE cells (safe viewport vs cells)",
            &(viewport.x, viewport.y, viewport.w, viewport.h),
            &(
                (name_cell.x, name_cell.y, name_cell.w, name_cell.h),
                (angle_cell.x, angle_cell.y, angle_cell.w, angle_cell.h),
            ),
            true,
        );
        if let Some((name_point, _angle_point)) =
            visible_row_centers(&name_cell, &angle_cell, &viewport)
        {
            debug_check("tier row visible band", &true, &true);
            return Ok((idx, name_point));
        }
        // The wheel did not move the row: the table cannot scroll further, so take the visible
        // band of the NAME cell when it has one.
        if last_row_y.is_some_and(|y| (y - row.y).abs() <= 0.5) {
            let band = visible_name_point(&name_cell, &viewport);
            debug_check("tier row visible band", &true, &band.is_some());
            if let Some(point) = band {
                return Ok((idx, point));
            }
            return Err(format!(
                "tier {wanted:?}, row {idx}: table cannot scroll further; row bounds {:?}, name \
                 cell {:?}, safe viewport {:?}",
                (row.x, row.y, row.w, row.h),
                (name_cell.x, name_cell.y, name_cell.w, name_cell.h),
                (viewport.x, viewport.y, viewport.w, viewport.h)
            ));
        }
        last_row_y = Some(row.y);
        let target_y = name_cell.center().1;
        let center_y = viewport.y + viewport.h / 2.0;
        let delta = (target_y - center_y).clamp(-120.0, 120.0);
        let wheel = if delta.abs() >= 1.0 {
            -delta
        } else if name_cell.y < viewport.y {
            80.0
        } else {
            -80.0
        };
        film.wheel_over(&viewport, wheel)?;
        film.rec.hold(Duration::from_millis(200))?;
        pump(60);
    }
    Err(format!(
        "tier {wanted:?}, row {idx}: NAME and ANGLE text centers never had a safe visible area \
         (last row bounds {last_bounds:?})"
    ))
}
