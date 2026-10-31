//! What the Crystal Axis asks of the planner's 3D view besides drawing its own gizmo
//! (`axis_gizmo`): the layouts drawn with the cutter's box poses, the view switched to the rough
//! for a pick, and the scenes built again after a stone changed.
//!
//! A child of the view, so it reaches the view's own scene and selection functions.

use super::{
    request_render, result_index, select_result, show_fit, show_fit_with, showing_result,
    start_thumbnails, zoning_view::ColourJob,
};
use crate::{
    RoughPlanModel,
    gui::{
        rough_colour::store::PoseChoices,
        rough_plan::{axis, host::Host},
    },
};
use pleochroma_cut_core::rough_plan::{RoughLayout, orientation::PlanSettingsOverrides};
use slint::ComponentHandle;
use std::{rc::Rc, sync::Arc};

/// The pose choices of the colour wizard with the box poses the cutter chose in the planner over
/// them, for result `layout`: the planner's choice wins for a stone it has a pose for. `None` when
/// the planner chose no pose in this result (the wizard's choices stand as they are).
#[must_use]
fn wizard_poses_with_planner_poses(
    wizard: &PoseChoices,
    overrides: &PlanSettingsOverrides,
    layout: u32,
) -> Option<PoseChoices> {
    let own: Vec<(u32, u8)> = overrides
        .stones
        .range((layout, 0)..=(layout, u32::MAX))
        .filter(|(_, choice)| choice.pose != 0)
        .map(|(&(_, stone), choice)| (stone, choice.pose))
        .collect();
    if own.is_empty() {
        return None;
    }
    let mut merged = wizard.clone();
    for (stone, pose) in own {
        merged.insert((layout, stone), pose);
    }
    Some(merged)
}

/// The colour job of result `index` (see `zoning_hooks::colour_job`) with the cutter's box poses of
/// this session applied over the poses the wizard stored, so a plan with a rough color draws the
/// planner's poses too (the planner's choice wins on a stone both chose). `None` for a plan without
/// a rough color.
#[must_use]
pub(super) fn colour_job_with_poses(
    overrides: &PlanSettingsOverrides,
    index: usize,
) -> Option<ColourJob> {
    let mut job = crate::gui::rough_plan::zoning_hooks::colour_job(index)?;
    if let Some(choices) =
        wizard_poses_with_planner_poses(&job.inputs.choices, overrides, job.layout_index)
    {
        let mut inputs = (*job.inputs).clone();
        inputs.choices = choices;
        job.inputs = Arc::new(inputs);
    }
    Some(job)
}

/// The layout of result `index` as the view draws it: the planner's own with the box poses the
/// cutter chose applied. A plan with a rough color gets its poses through the color job
/// ([`colour_job_with_poses`], applied by `zoning_view::prepare`), so they are not applied twice.
#[must_use]
pub(super) fn drawn_layout(
    choices: &PlanSettingsOverrides,
    index: usize,
    layout: &RoughLayout,
    posed: bool,
) -> RoughLayout {
    if !posed
        || choices.stones.is_empty()
        || crate::gui::rough_plan::zoning_hooks::colour_job(index).is_some()
    {
        return layout.clone();
    }
    axis::layouts::posed_layout(layout, index, choices)
}

/// The rough is shown in place of a result (a pick on the rough needs it).
pub(in crate::gui::rough_plan) fn show_rough(host: &Rc<Host>) {
    if showing_result(host) {
        super::deselect_result(host);
    }
}

/// The stones of the shown result changed (a box pose, a lock): the scene and the thumbnails are
/// drawn again.
pub(in crate::gui::rough_plan) fn stones_changed(host: &Rc<Host>) {
    let count = host.session.borrow().run.layouts.len();
    let selected = result_index(
        host.window.global::<RoughPlanModel>().get_selected_result(),
        count,
    );
    if let Some(index) = selected {
        show_fit(host, index);
    }
    start_thumbnails(host);
}

/// The layout of result `index` changed while a stone is turned by hand: its scene is built again,
/// from the layout as it is.
pub(in crate::gui::rough_plan) fn redraw_result(host: &Rc<Host>, index: usize) {
    show_fit_with(host, index, false);
}

/// Selects result `result` and design row `group` again after the results were shown anew.
pub(in crate::gui::rough_plan) fn reselect(host: &Rc<Host>, result: i32, group: i32) {
    select_result(host, result);
    host.window
        .global::<RoughPlanModel>()
        .set_selected_group(group);
    if !host.session.borrow().view.building {
        request_render(host);
    }
    axis::sync(host);
}

#[cfg(test)]
mod tests {
    use super::super::edited_since_plan;
    use pleochroma_cut_core::{
        crystal_axis::CrystalOrientation,
        rough_plan::{
            Axis, CutOrder, CutPlan, PlacedStone, RoughBase, RoughModel, StonePose,
            orientation::StoneChoice,
        },
    };

    use super::*;

    #[test]
    fn setting_the_crystal_axis_does_not_leave_the_planned_results() {
        let planned = RoughModel::new(
            RoughBase::Block {
                x_mm: 10.0,
                y_mm: 8.0,
                z_mm: 6.0,
            },
            Vec::new(),
        );
        let mut with_axis = planned.clone();
        with_axis.orientation = Some(CrystalOrientation::from_angles(20.0, 30.0, None));
        assert!(!edited_since_plan(Some(&planned), &with_axis));
    }

    fn layout() -> RoughLayout {
        let stone = PlacedStone {
            entry_id: 1,
            piece_origin_mm: [0.0; 3],
            piece_size_mm: [1.0; 3],
            stone_size_mm: [1.0; 3],
            table_axis: Axis::Y,
            carat: 0.1,
            volume_mm3: 1.0,
            pose: StonePose {
                center_mm: [0.5; 3],
                axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                mm_per_unit: 1.0,
            },
        };
        RoughLayout {
            cut_order: CutOrder::Xyz,
            stones: vec![stone],
            cut_plan: CutPlan { slabs: Vec::new() },
            total_carat: 0.1,
            total_volume_mm3: 1.0,
            yield_fraction: 0.1,
            exact_fit: false,
        }
    }

    #[test]
    fn the_box_poses_are_drawn_only_when_asked_for() {
        let mut choices = PlanSettingsOverrides::default();
        choices.set_stone_choice(
            0,
            0,
            StoneChoice {
                pose: 1,
                ..StoneChoice::default()
            },
        );
        let planned = layout();
        assert_eq!(drawn_layout(&choices, 0, &planned, false), planned);
        assert_ne!(drawn_layout(&choices, 0, &planned, true), planned);
        assert_eq!(
            drawn_layout(&PlanSettingsOverrides::default(), 0, &planned, true),
            planned
        );
    }

    #[test]
    fn the_planners_box_poses_win_over_the_wizards_for_a_plan_with_a_rough_color() {
        let mut overrides = PlanSettingsOverrides::default();
        let pose_of = |pose| StoneChoice {
            pose,
            ..StoneChoice::default()
        };
        overrides.set_stone_choice(0, 1, pose_of(3));
        overrides.set_stone_choice(0, 2, pose_of(2));
        overrides.set_stone_choice(1, 0, pose_of(1));
        // A lock alone chooses no pose.
        overrides.set_stone_choice(
            0,
            4,
            StoneChoice {
                locked: true,
                ..StoneChoice::default()
            },
        );
        let wizard = PoseChoices::from([((0, 0), 1), ((0, 1), 2), ((2, 0), 3)]);
        let merged =
            wizard_poses_with_planner_poses(&wizard, &overrides, 0).expect("a pose chosen");
        // The wizard's stone stays, the conflict goes to the planner, the new stone is added, and
        // the choices of other results are not touched.
        assert_eq!(
            merged,
            PoseChoices::from([((0, 0), 1), ((0, 1), 3), ((0, 2), 2), ((2, 0), 3)])
        );
        // A result the planner chose no pose in keeps the wizard's choices as they are.
        assert!(wizard_poses_with_planner_poses(&wizard, &overrides, 2).is_none());
    }
}
