//! Clickable History and Variants routes, including controls in their nested scroll views.
use crate::{director::Film, pacing, targets::Spot};
use pleochroma_cut::{HistoryModel, VariantsModel};
use slint::{ComponentHandle, Model};
use std::{collections::HashSet, time::Duration};

/// Direction of the search for a control that is not in the accessibility tree yet: down until a
/// turn no longer moves the scope, then up for the remaining turns.
struct Search {
    down: bool,
    last_top: Option<f32>,
}

impl Search {
    const fn new() -> Self {
        Self {
            down: true,
            last_top: None,
        }
    }

    /// The wheel delta (negative scrolls down) for the next turn, given the scope's current top.
    fn next(&mut self, top: f32, step: f32) -> f32 {
        if self.down && self.last_top.is_some_and(|last| (last - top).abs() < 0.5) {
            self.down = false;
        }
        self.last_top = Some(top);
        if self.down { -step } else { step }
    }
}

/// How many turns `nested_control` scrolls a section's list down before it turns back up.
const MISSING_DOWN_TURNS: u32 = 16;

#[expect(clippy::too_many_lines, reason = "one filmed route step by step")]
fn nested_control(
    film: &mut Film<'_>,
    component: &str,
    label: &str,
    role: uiprobe::AccessibleRole,
) -> Result<Spot, String> {
    let mut missing_turns = 0_u32;
    for _ in 0..48 {
        let view = film.view();
        let scope = view
            .root
            .query_descendants()
            .match_inherits(component)
            .find_first();
        let Some(scope) = scope else {
            // The variant form sits above the list in the section's own scrolling area: with
            // the list turned down (a Delete at its end) the open form leaves the tree. Turn
            // that area up to bring it back.
            if component == "VariantForm" && film.ui.global::<VariantsModel>().get_form_mode() != 0
            {
                let section = view
                    .root
                    .query_descendants()
                    .match_inherits("VariantsSection")
                    .find_first()
                    .ok_or("VariantsSection is not visible")?;
                let (x, y) = Spot::of(&section).center();
                let wheel = Spot {
                    x,
                    y,
                    w: 3.0,
                    h: 3.0,
                    handle: None,
                };
                film.wheel_over(&wheel, 120.0)?;
                continue;
            }
            return Err(format!("{component} is not visible"));
        };
        let scroll_owner = if component == "VariantForm" {
            "VariantsSection"
        } else {
            component
        };
        let scroll_scope = view
            .root
            .query_descendants()
            .match_inherits(scroll_owner)
            .find_first()
            .ok_or_else(|| format!("{scroll_owner} is not visible"))?;
        let scroll = scroll_scope
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .ok_or_else(|| format!("{scroll_owner} scroll view is missing"))?;
        let dock =
            crate::targets::run(&view, &crate::targets::Query::Id("EditorView::dock_scroll"))
                .ok_or("editor dock is missing")?;
        let inspector = view
            .root
            .query_descendants()
            .match_inherits("EditorInspector")
            .find_first()
            .ok_or("Editor Inspector is missing")?;
        let inspector_scroll = inspector
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .ok_or("Inspector scroll view is missing")?;
        let inspector_clip = Spot::of(&inspector_scroll)
            .clipped_to(&dock, view.size)
            .ok_or("Inspector is clipped by the editor dock")?;
        let inner_viewport = Spot::of(&scroll);
        let Some(clip) = inner_viewport.clipped_to(&inspector_clip, view.size) else {
            let (_, viewport_y) = inner_viewport.center();
            let (_, outer_y) = inspector_clip.center();
            film.scroll_inspector(-(viewport_y - outer_y).clamp(-180.0, 180.0))?;
            continue;
        };
        if clip.h < inner_viewport.h * 0.65 || clip.w < inner_viewport.w * 0.65 {
            let (_, viewport_y) = inner_viewport.center();
            let (_, outer_y) = inspector_clip.center();
            film.scroll_inspector(-(viewport_y - outer_y).clamp(-180.0, 180.0))?;
            continue;
        }
        let found: Vec<_> = scope
            .query_descendants()
            .match_accessible_role(role)
            .find_all()
            .into_iter()
            .filter(|e| e.accessible_label().is_some_and(|s| s == label))
            .map(|e| Spot::of(&e))
            .collect();
        if found.len() > 1 {
            return Err(format!(
                "{label:?} in {component} is ambiguous ({} matches)",
                found.len()
            ));
        }
        if let Some(target) = found.first() {
            let visible = target.clipped_to(&clip, view.size);
            if let Some(visible) = visible {
                let (cx, cy) = target.center();
                if cx > clip.x
                    && cx < clip.x + clip.w
                    && cy > clip.y
                    && cy < clip.y + clip.h
                    // Half is enough once the centre is in view: the last row of a list that
                    // cannot scroll further can stay cut by the list's bottom edge.
                    && visible.h >= target.h * 0.5
                    && visible.w >= target.w * 0.7
                {
                    return Ok(target.clone());
                }
            }
            let delta = if target.y + target.h > clip.y + clip.h {
                -120.0
            } else {
                120.0
            };
            let wheel = Spot {
                x: clip.x + 4.0,
                y: clip.h.mul_add(0.5, clip.y),
                w: 3.0,
                h: 3.0,
                handle: None,
            };
            film.wheel_over(&wheel, delta)?;
        } else {
            // The control lies in the section's own scrolling list, out of its viewport (rows
            // out of view are not in the accessibility tree): turn that list, not the inspector,
            // down first and then back up.
            missing_turns += 1;
            let step = (clip.h * 0.65).clamp(24.0, 120.0);
            let wheel = Spot {
                x: clip.w.mul_add(0.5, clip.x),
                y: clip.h.mul_add(0.5, clip.y),
                w: 3.0,
                h: 3.0,
                handle: None,
            };
            // The variant form cut by the top of the list (it sits above a list turned down)
            // is revealed by turning up; otherwise down first, then back up.
            let form_above = component == "VariantForm" && Spot::of(&scope).y < clip.y - 1.0;
            let up = form_above || missing_turns > MISSING_DOWN_TURNS;
            film.wheel_over(&wheel, if up { step } else { -step })?;
        }
    }
    Err(format!(
        "{label:?} ({role:?}) is missing in {component}, or could not be revealed after 48 bounded scrolls of its list (down, then up)"
    ))
}

fn outer_control(
    film: &mut Film<'_>,
    component: &str,
    label: &str,
    role: uiprobe::AccessibleRole,
) -> Result<Spot, String> {
    let mut search = Search::new();
    for _ in 0..48 {
        let view = film.view();
        let scope = view
            .root
            .query_descendants()
            .match_inherits(component)
            .find_first()
            .ok_or_else(|| format!("{component} is not visible"))?;
        let dock =
            crate::targets::run(&view, &crate::targets::Query::Id("EditorView::dock_scroll"))
                .ok_or("editor dock is missing")?;
        let inspector = view
            .root
            .query_descendants()
            .match_inherits("EditorInspector")
            .find_first()
            .ok_or("Editor Inspector is missing")?;
        let outer = inspector
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .ok_or("Inspector scroll view is missing")?;
        let clip = Spot::of(&outer)
            .clipped_to(&dock, view.size)
            .ok_or("Inspector is clipped")?;
        let matches: Vec<_> = scope
            .query_descendants()
            .match_accessible_role(role)
            .find_all()
            .into_iter()
            .filter(|e| e.accessible_label().is_some_and(|s| s == label))
            .map(|e| Spot::of(&e))
            .collect();
        if matches.is_empty() {
            let step = (clip.h * 0.65).clamp(24.0, 120.0);
            film.scroll_inspector(search.next(Spot::of(&scope).y, step))?;
            continue;
        }
        if matches.len() != 1 {
            return Err(format!(
                "{label:?} in {component}: expected one visible control, found {}",
                matches.len()
            ));
        }
        let target = &matches[0];
        // The inspector's scrolling area, or the whole visible inspector: a page's own header
        // row (a diff page's "Back to Variants") sits above the scrolling area, always in view.
        let panel = Spot::of(&inspector).clipped_to(&dock, view.size);
        for area in std::iter::once(clip.clone()).chain(panel) {
            if target
                .clipped_to(&area, view.size)
                .is_some_and(|v| v.h >= target.h * 0.75 && v.w >= target.w * 0.75)
            {
                let (x, y) = target.center();
                if x > area.x && x < area.x + area.w && y > area.y && y < area.y + area.h {
                    return Ok(target.clone());
                }
            }
        }
        let (_, target_y) = target.center();
        let (_, outer_y) = clip.center();
        film.scroll_inspector(-(target_y - outer_y).clamp(-180.0, 180.0))?;
    }
    Err(format!(
        "{label:?} in {component}: expected one visible control after 48 bounded inspector scrolls (down, then up)"
    ))
}

fn click_control(
    film: &mut Film<'_>,
    component: &str,
    label: &str,
    role: uiprobe::AccessibleRole,
) -> Result<(), String> {
    let spot = nested_control(film, component, label, role)?;
    film.click_and_invoke(&spot)
}

pub(in crate::scripts) fn compare_variants(film: &mut Film<'_>) -> Result<(), String> {
    let variants = film.ui.global::<VariantsModel>();
    let first = variants
        .get_choices()
        .iter()
        .position(|choice| choice.as_str() == "Variant 1")
        .ok_or("Variant 1 is missing from comparison choices")?;
    for (label, wanted, current) in [
        (
            "First Design to Compare",
            i32::try_from(first).map_err(|_| "choice overflow")?,
            variants.get_compare_first(),
        ),
        ("Second Design to Compare", 0, variants.get_compare_second()),
    ] {
        if current == wanted {
            continue;
        }
        click_control(
            film,
            "VariantsSection",
            label,
            uiprobe::AccessibleRole::Combobox,
        )?;
        film.hold(Duration::from_millis(40))?;
        let choice = variants
            .get_choices()
            .row_data(usize::try_from(wanted).map_err(|_| "negative comparison choice")?)
            .ok_or("comparison choice disappeared")?;
        film.select_open_combo(current, wanted)?;
        let committed = if label.starts_with("First") {
            variants.get_compare_first()
        } else {
            variants.get_compare_second()
        };
        if committed != wanted {
            return Err(format!(
                "{label:?} choice {choice:?} read back index {committed} after commit, expected index {wanted}"
            ));
        }
    }
    if variants.get_compare_first() != i32::try_from(first).unwrap_or(-1)
        || variants.get_compare_second() != 0
    {
        return Err("variant comparison choices did not match their readback".into());
    }
    click_control(
        film,
        "VariantsSection",
        "Compare Text",
        uiprobe::AccessibleRole::Button,
    )?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        variants.get_diff_open() && !variants.get_diff_busy()
    }) {
        return Err("variant text comparison did not open".into());
    }
    film.hold(pacing::RESULT_HOLD)?;
    let back = outer_control(
        film,
        "VariantDiffPage",
        "Back to Variants",
        uiprobe::AccessibleRole::Button,
    )?;
    film.click_and_invoke(&back)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || !variants.get_diff_open()) {
        return Err("Back to Variants did not close text comparison".into());
    }
    Ok(())
}

pub(in crate::scripts) fn save_variant(film: &mut Film<'_>, name: &str) -> Result<(), String> {
    let variants = film.ui.global::<VariantsModel>();
    let before: HashSet<_> = variants
        .get_rows()
        .iter()
        .map(|r| r.key.to_string())
        .collect();
    click_control(
        film,
        "VariantsSection",
        "Save as Variant...",
        uiprobe::AccessibleRole::Button,
    )?;
    let field = nested_control(
        film,
        "VariantForm",
        "Variant Name",
        uiprobe::AccessibleRole::TextInput,
    )?;
    film.click_and_type(&field, name)?;
    let save_label = variants.get_form_ok().to_string();
    let save = nested_control(
        film,
        "VariantForm",
        &save_label,
        uiprobe::AccessibleRole::Button,
    )?;
    film.click_and_invoke(&save)?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        !variants.get_busy()
            && variants
                .get_rows()
                .iter()
                .any(|row| row.name.as_str() == name && !before.contains(row.key.as_str()))
    }) {
        return Err(format!(
            "variant {name:?} was not persisted: {}",
            variants.get_form_error()
        ));
    }
    Ok(())
}

pub(in crate::scripts) fn open_variant(film: &mut Film<'_>, name: &str) -> Result<(), String> {
    let variants = film.ui.global::<VariantsModel>();
    let rows: Vec<_> = variants
        .get_rows()
        .iter()
        .filter(|r| r.name.as_str() == name)
        .collect();
    if rows.len() != 1 {
        return Err(format!(
            "variant {name:?}: expected unique persisted identity, found {}",
            rows.len()
        ));
    }
    let key = rows[0].key.clone();
    click_control(
        film,
        "VariantsSection",
        &format!("Open {name}"),
        uiprobe::AccessibleRole::Button,
    )?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        !variants.get_busy()
            && variants
                .get_rows()
                .iter()
                .any(|row| row.key == key && row.opened_from)
    }) {
        return Err(format!(
            "variant key {key:?}: Open did not set Working From"
        ));
    }
    Ok(())
}

pub(in crate::scripts) fn delete_variant(film: &mut Film<'_>, name: &str) -> Result<(), String> {
    let variants = film.ui.global::<VariantsModel>();
    let matches: Vec<_> = variants
        .get_rows()
        .iter()
        .filter(|row| row.name.as_str() == name)
        .collect();
    if matches.len() != 1 {
        return Err(format!("delete {name:?}: expected one persisted row"));
    }
    let key = matches[0].key.clone();
    click_control(
        film,
        "VariantsSection",
        &format!("Delete {name}"),
        uiprobe::AccessibleRole::Button,
    )?;
    if variants.get_form_mode() != 4 {
        return Err("variant Delete did not open confirmation".into());
    }
    let confirm = variants.get_form_ok().to_string();
    click_control(
        film,
        "VariantForm",
        &confirm,
        uiprobe::AccessibleRole::Button,
    )?;
    if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
        !variants.get_busy() && !variants.get_rows().iter().any(|row| row.key == key)
    }) {
        return Err(format!("delete {name:?}: persisted key remains"));
    }
    Ok(())
}

#[expect(clippy::too_many_lines, reason = "one filmed route step by step")]
pub(in crate::scripts) fn history_jump(
    film: &mut Film<'_>,
    target_position: i32,
) -> Result<(), String> {
    let history = film.ui.global::<HistoryModel>();
    let rows = history.get_rows();
    let idx = rows
        .iter()
        .position(|r| r.position == target_position)
        .ok_or_else(|| format!("history row for position {target_position} not found"))?;
    let row = rows.row_data(idx).ok_or("history row disappeared")?;
    let expected_label = format!(
        "{}, {}{}",
        row.title,
        row.detail,
        if history.get_current_position() == target_position {
            ", now"
        } else {
            ""
        }
    );
    for _ in 0..48 {
        let view = film.view();
        let tab = view
            .root
            .query_descendants()
            .match_inherits("HistoryTab")
            .find_first()
            .ok_or("History tab is missing")?;
        let list = tab
            .query_descendants()
            .match_inherits("ListView")
            .find_first()
            .ok_or("History list is missing")?;
        let dock =
            crate::targets::run(&view, &crate::targets::Query::Id("EditorView::dock_scroll"))
                .ok_or("editor dock is missing")?;
        let inspector = view
            .root
            .query_descendants()
            .match_inherits("EditorInspector")
            .find_first()
            .ok_or("Editor Inspector is missing")?;
        let inspector_scroll = inspector
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .ok_or("Inspector scroll view is missing")?;
        let inspector_clip = Spot::of(&inspector_scroll)
            .clipped_to(&dock, view.size)
            .ok_or("Inspector is clipped")?;
        let tab_clip = Spot::of(&tab)
            .clipped_to(&inspector_clip, view.size)
            .ok_or("History tab is clipped")?;
        let clip = Spot::of(&list)
            .clipped_to(&tab_clip, view.size)
            .ok_or("history list is clipped")?;
        let candidates: Vec<_> = tab
            .query_descendants()
            .match_accessible_role(uiprobe::AccessibleRole::ListItem)
            .find_all()
            .into_iter()
            .filter(|e| e.accessible_label().is_some_and(|l| l == expected_label))
            .map(|e| Spot::of(&e))
            .collect();
        if candidates.len() > 1 {
            return Err(format!("history row {expected_label:?} is ambiguous"));
        }
        let list_spot = Spot::of(&list);
        if list_spot
            .clipped_to(&inspector_clip, view.size)
            .is_none_or(|v| v.h < list_spot.h * 0.65)
        {
            let (_, list_y) = list_spot.center();
            let (_, outer_y) = inspector_clip.center();
            film.scroll_inspector(-(list_y - outer_y).clamp(-180.0, 180.0))?;
            continue;
        }
        if let Some(spot) = candidates.first()
            && spot
                .clipped_to(&clip, view.size)
                .is_some_and(|v| v.h >= spot.h * 0.7)
        {
            film.click_and_invoke(spot)?;
            if history.get_current_position() != target_position {
                return Err(format!(
                    "history jump to {target_position} failed; current {}",
                    history.get_current_position()
                ));
            }
            return Ok(());
        }
        // Newer positions are at the top. Wheel over the list's safe left gutter, away from its bar.
        let delta = if target_position > history.get_current_position() {
            100.0
        } else {
            -100.0
        };
        let wheel = Spot {
            x: clip.x + 4.0,
            y: clip.h.mul_add(0.5, clip.y),
            w: 3.0,
            h: 3.0,
            handle: None,
        };
        film.wheel_over(&wheel, delta)?;
    }
    Err(format!(
        "history row {expected_label:?} did not become visible after bounded list scrolling"
    ))
}
