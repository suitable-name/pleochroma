//! Writes one page of Cutting Mode into `CuttingModeModel`.
//!
//! All the wording comes from `pleochroma_editor::cutting_mode::display` (through the model's
//! reads); this only maps it onto the Slint model's properties and rows. Setting a property to
//! the value it holds changes nothing, so pushing a page again after a tick redraws only what
//! moved.

use super::CuttingMode;
use crate::{AppWindow, CmChip, CmScheduleRow, CmWheelMark, CmWheelScale, CuttingModeModel};
use pleochroma_editor::cutting_mode::{display::StepPage, progress::StepState};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

/// A count as the model's `int`.
fn int(count: usize) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// Pushes the page of `mode`'s current step, the progress line under it, the schedule, the
/// index wheel and (on the first step) the crystal axis picture. `design_name` is the name the
/// header shows.
///
/// A model without a current step clears the page.
pub fn push_page(ui: &AppWindow, mode: &CuttingMode, design_name: &str) {
    let Some(page) = mode.page() else {
        clear_page(ui);
        return;
    };
    let model = ui.global::<CuttingModeModel>();
    model.set_design_name(design_name.into());
    model.set_step_count(int(mode.step_count()));
    model.set_step_index(int(mode.position()));
    push_step(&model, &page, mode);
    model.set_has_marks(mode.has_marks());
    model.set_done_count(int(mode.done_count()));
    model.set_progress_text(mode.progress_line().into());
    model.set_progress_fraction(mode.progress_fraction());
    model.set_stone_caption(mode.stone_caption().unwrap_or_default().into());
    model.set_wheel_caption(mode.wheel_caption().unwrap_or_default().into());
    push_schedule(&model, mode);
    push_wheel(&model, mode);
    push_axis_picture(&model, mode);
}

/// The step's own texts and chips.
fn push_step(model: &CuttingModeModel<'_>, page: &StepPage, mode: &CuttingMode) {
    model.set_heading(page.heading.as_str().into());
    model.set_tier_name(page.tier_name.as_str().into());
    model.set_side_label(page.side.as_str().into());
    model.set_angle_text(page.angle.as_str().into());
    model.set_chips(ModelRc::new(VecModel::from(
        page.chips
            .iter()
            .map(|chip| CmChip {
                text: chip.text.as_str().into(),
                ticked: chip.ticked,
            })
            .collect::<Vec<_>>(),
    )));
    model.set_meet_text(page.meet.as_str().into());
    model.set_depth_text(page.depth.as_str().into());
    model.set_cheater_text(page.cheater.as_str().into());
    // The first step starts with how the rough must sit for the crystal axis (the cutting
    // sheet's Orientation block), ahead of the tier's own note.
    let notes = match mode.orientation_note() {
        Some(orientation) if page.notes.is_empty() => orientation,
        Some(orientation) => format!("{orientation}\n{}", page.notes),
        None => page.notes.clone(),
    };
    model.set_notes_text(notes.into());
    model.set_tool_text(page.tool.as_str().into());
    model.set_step_state(page.state.code());
    model.set_state_text(page.state_text.as_str().into());
    model.set_has_previous(page.has_previous);
    model.set_has_next(page.has_next);
}

/// Every step in cutting order, for the schedule list and the step strip.
fn push_schedule(model: &CuttingModeModel<'_>, mode: &CuttingMode) {
    let steps = &mode.plan().steps;
    model.set_schedule(ModelRc::new(VecModel::from(
        (0..steps.len())
            .filter_map(|position| StepPage::new(steps, position, mode.progress()))
            .map(|row| CmScheduleRow {
                code: row.tier_name.into(),
                angle: row.angle.into(),
                indices: row
                    .chips
                    .iter()
                    .map(|chip| chip.text.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
                    .into(),
                state: row.state.code(),
            })
            .collect::<Vec<_>>(),
    )));
}

/// The index wheel of the current step.
fn push_wheel(model: &CuttingModeModel<'_>, mode: &CuttingMode) {
    let Some(wheel) = mode.wheel() else {
        return;
    };
    model.set_wheel_minor(wheel.minor_ticks.into());
    model.set_wheel_major(wheel.major_ticks.into());
    model.set_wheel_spokes(wheel.spokes.into());
    model.set_wheel_marks(ModelRc::new(VecModel::from(
        wheel
            .marks
            .into_iter()
            .map(|mark| CmWheelMark {
                x: mark.x,
                y: mark.y,
                label_x: mark.label_x,
                label_y: mark.label_y,
                label: SharedString::from(mark.label),
                ticked: mark.ticked,
            })
            .collect::<Vec<_>>(),
    )));
    model.set_wheel_scale(ModelRc::new(VecModel::from(
        wheel
            .scale
            .into_iter()
            .map(|number| CmWheelScale {
                x: number.x,
                y: number.y,
                label: SharedString::from(number.label),
            })
            .collect::<Vec<_>>(),
    )));
}

/// The crystal axis picture of the first step: drawn for a stone whose axis is set, cleared on
/// every other page.
fn push_axis_picture(model: &CuttingModeModel<'_>, mode: &CuttingMode) {
    let picture = mode.axis_picture();
    model.set_axis_shown(picture.is_some());
    model.set_axis_frame(
        picture
            .as_ref()
            .map_or_else(SharedString::new, |p| p.frame.as_str().into()),
    );
    model.set_axis_path(
        picture
            .as_ref()
            .map_or_else(SharedString::new, |p| p.axis.as_str().into()),
    );
    model.set_axis_description(
        picture
            .as_ref()
            .map_or_else(SharedString::new, |p| p.description.as_str().into()),
    );
}

/// Empties the page: nothing to show (the screen is closed, or no step could be built).
pub fn clear_page(ui: &AppWindow) {
    let model = ui.global::<CuttingModeModel>();
    model.set_step_count(0);
    model.set_step_index(0);
    model.set_heading(SharedString::new());
    model.set_tier_name(SharedString::new());
    model.set_side_label(SharedString::new());
    model.set_angle_text(SharedString::new());
    model.set_chips(ModelRc::default());
    model.set_meet_text(SharedString::new());
    model.set_depth_text(SharedString::new());
    model.set_cheater_text(SharedString::new());
    model.set_notes_text(SharedString::new());
    model.set_tool_text(SharedString::new());
    model.set_step_state(StepState::NotDone.code());
    model.set_state_text(SharedString::new());
    model.set_has_previous(false);
    model.set_has_next(false);
    model.set_has_marks(false);
    model.set_done_count(0);
    model.set_progress_text(SharedString::new());
    model.set_progress_fraction(0.0);
    model.set_stone_caption(SharedString::new());
    model.set_wheel_caption(SharedString::new());
    model.set_schedule(ModelRc::default());
    model.set_wheel_minor(SharedString::new());
    model.set_wheel_major(SharedString::new());
    model.set_wheel_spokes(SharedString::new());
    model.set_wheel_marks(ModelRc::default());
    model.set_wheel_scale(ModelRc::default());
    model.set_axis_shown(false);
    model.set_axis_frame(SharedString::new());
    model.set_axis_path(SharedString::new());
    model.set_axis_description(SharedString::new());
}
