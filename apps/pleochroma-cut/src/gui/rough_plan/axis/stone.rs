//! The stone menu: what the cutter can do with one stone of the selected result.
//!
//! - **Box poses**: turn the stone to another of its four box poses (a half turn about one of its
//!   own axes). The piece and the weight stay; only the crystal axis angle changes.
//! - **Re-Fit This Piece**: fit the best stone of the design into the same piece again, under the
//!   rule (`refit_*`), without moving the saw cuts.
//! - **Free Orientation** (a single-stone exact fit only): turn the stone by dragging it in the 3D
//!   view; the planner sizes the largest stone that fits the new position at once (`rescale_pose`).
//! - **Lock / Unlock**: a locked stone is kept, and the next plan cuts the rest of the rough around
//!   it (`rough_with_locked_pieces`, listed first with `LOCKED_FIRST_NOTE`).
//! - **Reason**: why the stone is set up differently, saved with the plan and printed on the
//!   cutting sheet.
//!
//! Every action is one undo step of the planner's history (`Session::edit_stones`). A pose, a lock
//! and a Reason are choices for the stone (`StoneChoice`); a re-fit and a turn by hand change the
//! layout itself, kept as a changed layout beside the planned ones (`LayoutEdits`), so undo brings
//! the planned stone back.

use super::{
    layouts,
    state::{FreeEdit, SavedSelection},
};
use crate::{
    RoughPlanModel,
    gui::{
        rough_plan::{
            editing,
            format::effective_id,
            host::{Host, on_host, on_idle_host},
            run::{
                ShownLayouts, fit_in_original_frame, free_kit, load_design_kit, refit, show_layouts,
            },
            view,
        },
        tutorial_events::raise_weak,
    },
};
use glam::{DMat3, DVec3};
use pleochroma_cut_core::rough_plan::{
    PlacedStone, RoughLayout, orientation::StoneChoice, rescale_pose,
};
use pleochroma_editor::guide::viewing_events::{ROUGH_STONE_LOCKED, ROUGH_STONE_OVERRIDDEN};
use pleochroma_vault::db::sqlite::Database;
use slint::{ComponentHandle, Model};
use std::{
    rc::Rc,
    sync::{Arc, Mutex},
};
use tracing::warn;

/// The actions of the stone menu, as `RoughPlanModel.stone_action` numbers them.
const ACTION_POSE: i32 = 0;
/// Free Orientation on or off.
const ACTION_FREE: i32 = 1;
/// Re-Fit This Piece.
const ACTION_REFIT: i32 = 2;
/// Lock or Unlock.
const ACTION_LOCK: i32 = 3;
/// The Reason.
const ACTION_REASON: i32 = 4;

/// How many box poses a stone has.
const POSE_COUNT: u8 = 4;

/// The longest Reason a plan stores, in characters.
const MAX_REASON_CHARS: usize = 500;

/// How far a stone turns for one pixel of a drag, in degrees.
const DEGREES_PER_PIXEL: f64 = 0.5;

/// The choice for a stone after the box pose `text` was picked (`None` for text that is not one of
/// the poses). The pose replaces a pose placed by hand.
#[must_use]
pub(super) fn with_pose(choice: &StoneChoice, text: &str) -> Option<StoneChoice> {
    let pose: u8 = text.trim().parse().ok()?;
    (pose < POSE_COUNT).then(|| StoneChoice {
        pose,
        free_pose: None,
        ..choice.clone()
    })
}

/// The choice for a stone after Lock or Unlock.
#[must_use]
pub(super) fn with_lock_flipped(choice: &StoneChoice) -> StoneChoice {
    StoneChoice {
        locked: !choice.locked,
        ..choice.clone()
    }
}

/// The choice for a stone after the Reason `text` was typed.
#[must_use]
pub(super) fn with_reason(choice: &StoneChoice, text: &str) -> StoneChoice {
    StoneChoice {
        reason: text.trim().chars().take(MAX_REASON_CHARS).collect(),
        ..choice.clone()
    }
}

/// The choice for a stone after it was re-fitted: the pose chosen for the old stone means nothing
/// for the new one; a lock and a Reason stay.
#[must_use]
pub(super) fn after_refit(choice: &StoneChoice) -> StoneChoice {
    StoneChoice {
        pose: 0,
        free_pose: None,
        ..choice.clone()
    }
}

/// The stone of `layout` turned by a drag of `(dx, dy)` pixels seen from a camera whose right and
/// up directions are `right` and `up`: the new axes of its pose.
#[must_use]
pub(super) fn turned_axes(
    axes: [[f64; 3]; 3],
    right: DVec3,
    up: DVec3,
    dx: f32,
    dy: f32,
) -> [[f64; 3]; 3] {
    let about_up = DMat3::from_axis_angle(up, (f64::from(dx) * DEGREES_PER_PIXEL).to_radians());
    let about_right =
        DMat3::from_axis_angle(right, (f64::from(dy) * DEGREES_PER_PIXEL).to_radians());
    let turn = about_right * about_up;
    axes.map(|axis| (turn * DVec3::from_array(axis)).to_array())
}

/// The stones of the selected result that are locked, for the next plan to keep.
#[must_use]
pub(in crate::gui::rough_plan) fn locked_for_plan(host: &Host) -> Vec<PlacedStone> {
    let selected = host.window.global::<RoughPlanModel>().get_selected_result();
    let Ok(result) = usize::try_from(selected) else {
        return Vec::new();
    };
    let session = host.session.borrow();
    match (session.run.layouts.get(result), u32::try_from(result).ok()) {
        (Some(layout), Some(result)) => layouts::locked_stones(layout, result, &session.overrides),
        _ => Vec::new(),
    }
}

/// Registers the stone menu's callback.
pub(super) fn setup(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.on_stone_action(|stone, action, text| {
        on_idle_host(|host| stone_action(host, stone, action, &text));
    });
}

/// The selected result and the stone `stone` of it, as the indices the choices are kept by.
fn selected_stone(host: &Host, stone: i32) -> Option<(usize, usize)> {
    let result =
        usize::try_from(host.window.global::<RoughPlanModel>().get_selected_result()).ok()?;
    let stone = usize::try_from(stone).ok()?;
    let session = host.session.borrow();
    (session.run.layouts.get(result)?.stones.len() > stone).then_some((result, stone))
}

/// An action of the stone menu.
fn stone_action(host: &Rc<Host>, stone: i32, action: i32, text: &str) {
    let Some((result, stone)) = selected_stone(host, stone) else {
        return;
    };
    match action {
        ACTION_POSE => {
            if edit_choice(host, result, stone, |choice| with_pose(choice, text)) {
                raise_weak(&host.main, ROUGH_STONE_OVERRIDDEN);
            }
        }
        ACTION_LOCK => {
            let mut locked = false;
            let done = edit_choice(host, result, stone, |choice| {
                let next = with_lock_flipped(choice);
                locked = next.locked;
                Some(next)
            });
            if done && locked {
                raise_weak(&host.main, ROUGH_STONE_LOCKED);
            }
        }
        ACTION_REASON => {
            if edit_choice(host, result, stone, |choice| {
                Some(with_reason(choice, text))
            }) && !text.trim().is_empty()
            {
                raise_weak(&host.main, ROUGH_STONE_OVERRIDDEN);
            }
        }
        ACTION_REFIT => start_refit(host, result, stone),
        ACTION_FREE => toggle_free(host, result, stone),
        _ => {}
    }
}

/// Changes the choice for a stone through `change` (it gives `None` to refuse) as one undo step,
/// and mirrors it into the rows and the 3D view. `false` when nothing was changed.
fn edit_choice(
    host: &Rc<Host>,
    result: usize,
    stone: usize,
    change: impl FnOnce(&StoneChoice) -> Option<StoneChoice>,
) -> bool {
    let (Ok(result), Ok(stone)) = (u32::try_from(result), u32::try_from(stone)) else {
        return false;
    };
    {
        let mut session = host.session.borrow_mut();
        let current = session
            .overrides
            .stone_choice(result, stone)
            .cloned()
            .unwrap_or_default();
        let Some(next) = change(&current) else {
            return false;
        };
        session.edit_overrides(|overrides| overrides.set_stone_choice(result, stone, next));
    }
    editing::push_history_flags(host);
    super::sync(host);
    view::axis_view::stones_changed(host);
    true
}

/// Runs `work` on a thread of its own (it may wait for the library) and `done` with its answer on
/// the UI thread.
fn spawn_job<T, W, D>(host: &Rc<Host>, name: &str, work: W, done: D)
where
    T: Send + 'static,
    W: FnOnce(&Mutex<Database>) -> T + Send + 'static,
    D: FnOnce(&Rc<Host>, T) + Send + 'static,
{
    let db = Arc::clone(&host.db);
    let window = host.window.as_weak();
    let spawned = std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let answer = work(db.as_ref());
            let _ = window.upgrade_in_event_loop(move |_window| {
                on_host(|host| done(host, answer));
            });
        });
    if let Err(error) = spawned {
        warn!("Rough planner: could not start the stone job: {error}");
        host.session.borrow_mut().axis.note = Some(("Could not start the job.".to_string(), 3));
        super::sync(host);
    }
}

/// Everything a re-fit or a turn by hand needs about stone `stone` of result `result`, copied off
/// the session so a thread can work on it.
struct StoneJob {
    result: usize,
    stone: usize,
    before: PlacedStone,
    entry_id: i64,
    model: pleochroma_cut_core::rough_plan::RoughModel,
    settings: pleochroma_cut_core::rough_plan::PlanSettings,
    rough: Option<pleochroma_cut_core::crystal_axis::CrystalOrientation>,
    rules: pleochroma_cut_core::rough_plan::orientation::PlanSettingsOverrides,
    material: String,
    tail: Vec<PlacedStone>,
}

/// The job for stone `stone` of result `result`, or `None` while the results carry no model.
fn stone_job(host: &Host, result: usize, stone: usize) -> Option<StoneJob> {
    let session = host.session.borrow();
    let layout = session.run.layouts.get(result)?;
    let before = *layout.stones.get(stone)?;
    Some(StoneJob {
        result,
        stone,
        entry_id: effective_id(&session.run.statuses, before.entry_id),
        before,
        model: session.run.plan_model.clone()?,
        settings: session.run.plan_settings?,
        rough: session.run.plan_model.as_ref()?.orientation,
        rules: session.run.plan_overrides.clone(),
        material: session.run.material_name.clone(),
        tail: layout.stones.iter().skip(1).copied().collect(),
    })
}

/// Re-Fit This Piece: a thread reads the design and fits it again; the answer replaces the stone.
fn start_refit(host: &Rc<Host>, result: usize, stone: usize) {
    let Some(job) = stone_job(host, result, stone) else {
        return;
    };
    host.session.borrow_mut().axis.note = Some(("Fitting the piece again...".to_string(), 0));
    super::sync(host);
    spawn_job(
        host,
        "rough-axis-refit",
        move |db| {
            let kit = load_design_kit(db, job.entry_id, job.rough, &job.rules, &job.material)?;
            let fitted =
                refit(&kit, &job.model, &job.settings, &job.before, None).ok_or_else(|| {
                    "No stone of this design fits the piece under the rule.".to_string()
                })?;
            Ok::<_, String>((job.result, job.stone, job.before, fitted))
        },
        finish_refit,
    );
}

/// The re-fitted stone arrives: it replaces the old one as one undo step, unless the results
/// changed meanwhile.
fn finish_refit(host: &Rc<Host>, answer: Result<(usize, usize, PlacedStone, PlacedStone), String>) {
    let (result, stone, before, mut fitted) = match answer {
        Ok(found) => found,
        Err(message) => {
            host.session.borrow_mut().axis.note = Some((message, 3));
            super::sync(host);
            return;
        }
    };
    {
        let mut guard = host.session.borrow_mut();
        let session = &mut *guard;
        let Some(layout) = session.run.layouts.get(result) else {
            return;
        };
        // The stone the answer is for must still be the one on screen.
        if layout.stones.get(stone) != Some(&before) {
            return;
        }
        // The new stone keeps the id the layout uses for the design.
        fitted.entry_id = before.entry_id;
        let rough_volume = session
            .run
            .plan_model
            .as_ref()
            .and_then(|model| model.measure().ok())
            .map(|measure| measure.volume_mm3);
        let mut changed: RoughLayout = layout.clone();
        if !layouts::replace_stone(&mut changed, stone, fitted, rough_volume) {
            return;
        }
        let (Ok(result_key), Ok(stone_key)) = (u32::try_from(result), u32::try_from(stone)) else {
            return;
        };
        session.edit_stones(|overrides, edits| {
            edits.insert(result_key, changed);
            if let Some(choice) = overrides.stone_choice(result_key, stone_key).cloned() {
                overrides.set_stone_choice(result_key, stone_key, after_refit(&choice));
            }
        });
        session.axis.note = Some(("The piece was fitted again.".to_string(), 1));
    }
    editing::push_history_flags(host);
    super::sync(host);
}

/// Free Orientation on or off for stone `stone` of result `result` (a single-stone exact fit).
fn toggle_free(host: &Rc<Host>, result: usize, stone: usize) {
    let active = host
        .session
        .borrow()
        .axis
        .free
        .as_ref()
        .is_some_and(|free| free.result == result && free.stone == stone);
    if active {
        host.session.borrow_mut().axis.free = None;
        host.session.borrow_mut().axis.note = None;
        super::sync(host);
        return;
    }
    let exact = host
        .session
        .borrow()
        .run
        .layouts
        .get(result)
        .is_some_and(|layout| layout.exact_fit && stone == 0);
    let Some(job) = stone_job(host, result, stone).filter(|_| exact) else {
        return;
    };
    host.session.borrow_mut().axis.note = Some(("Getting the stone ready...".to_string(), 0));
    super::sync(host);
    spawn_job(
        host,
        "rough-axis-free",
        move |db| {
            let kit = load_design_kit(db, job.entry_id, job.rough, &job.rules, &job.material)?;
            let free = free_kit(&kit, &job.model, &job.settings, job.tail)?;
            Ok::<_, String>((job.result, job.stone, free))
        },
        |host, answer| match answer {
            Ok((result, stone, kit)) => {
                {
                    let mut session = host.session.borrow_mut();
                    session.axis.free = Some(FreeEdit { result, stone, kit });
                    session.axis.note = Some((
                        "Drag the stone in the 3D view to turn it. Press Done Turning when it sits right."
                            .to_string(),
                        0,
                    ));
                }
                super::sync(host);
            }
            Err(message) => {
                host.session.borrow_mut().axis.note = Some((message, 3));
                super::sync(host);
            }
        },
    );
}

/// Whether a stone is being turned by hand in the shown result.
#[must_use]
pub(in crate::gui::rough_plan) fn is_turning(host: &Host) -> bool {
    host.session.borrow().axis.free.is_some()
}

/// A drag of `(dx, dy)` pixels turns the stone being turned by hand, seen from a camera with the
/// directions `right` and `up`. Returns the result whose stone changed, for the view to draw
/// again; `None` when nothing turned (no stone is being turned, or nothing fits the new position).
pub(in crate::gui::rough_plan) fn turn(
    host: &Host,
    right: DVec3,
    up: DVec3,
    dx: f32,
    dy: f32,
) -> Option<usize> {
    let mut guard = host.session.borrow_mut();
    let session = &mut *guard;
    let free = session.axis.free.as_ref()?;
    let layout = session.run.layouts.get_mut(free.result)?;
    let stone = layout.stones.get(free.stone)?;
    let axes = turned_axes(stone.pose.axes, right, up, dx, dy);
    let kit = &free.kit;
    let fit = fit_in_original_frame(
        rescale_pose(
            &kit.region,
            &kit.hull,
            axes,
            &kit.settings,
            kit.ctx
                .as_ref()
                .and_then(pleochroma_cut_core::rough_plan::shaped::ShapedCtx::fit_mesh),
        )?,
        kit.offset_mm,
    );
    *layout = layouts::rebuilt_exact_fit(&fit, &kit.hull, kit.model_volume_mm3, &kit.tail);
    Some(free.result)
}

/// The drag ended: the stone as it sits now is one undo step (a changed layout, and the pose as the
/// choice for the stone so the row says "overridden").
pub(in crate::gui::rough_plan) fn finish_turn(host: &Rc<Host>) {
    {
        let mut guard = host.session.borrow_mut();
        let session = &mut *guard;
        let Some(free) = session.axis.free.as_ref() else {
            return;
        };
        let (result, stone) = (free.result, free.stone);
        let Some(layout) = session.run.layouts.get(result).cloned() else {
            return;
        };
        let (Ok(result_key), Ok(stone_key)) = (u32::try_from(result), u32::try_from(stone)) else {
            return;
        };
        let Some(pose) = layout.stones.get(stone).map(|placed| placed.pose) else {
            return;
        };
        session.edit_stones(|overrides, edits| {
            let current = overrides
                .stone_choice(result_key, stone_key)
                .cloned()
                .unwrap_or_default();
            edits.insert(result_key, layout);
            overrides.set_stone_choice(
                result_key,
                stone_key,
                StoneChoice {
                    pose: 0,
                    free_pose: Some(pose),
                    ..current
                },
            );
        });
    }
    editing::push_history_flags(host);
    super::sync(host);
}

/// Shows the layouts the cutter changed in place of the shown ones when they differ (after an
/// edit, an undo or a redo): the results are formatted again and the selection comes back.
pub(super) fn refresh_layouts(host: &Rc<Host>) {
    let needed = {
        let session = host.session.borrow();
        !session.axis.base.is_empty() && session.axis.shown_edits != session.edits
    };
    if !needed {
        return;
    }
    let (selected, group) = {
        let model = host.window.global::<RoughPlanModel>();
        (model.get_selected_result(), model.get_selected_group())
    };
    let state = {
        let mut guard = host.session.borrow_mut();
        let session = &mut *guard;
        session.axis.shown_edits.clone_from(&session.edits);
        session.axis.restore = Some(SavedSelection {
            result: selected,
            group,
            keep: session.run.keep.clone(),
        });
        let run = &session.run;
        ShownLayouts {
            layouts: layouts::with_edits(&session.axis.base, &session.edits),
            plan_model: run.plan_model.clone(),
            plan_settings: run.plan_settings,
            material_name: run.material_name.clone(),
            weighed_ct: run.weighed_ct,
            candidate_source: run.candidate_source,
            titles: run.titles.clone(),
            statuses: run.statuses.clone(),
            shapes: run.shapes.clone(),
            source: run.source.clone(),
            plan_overrides: run.plan_overrides.clone(),
            labels: run.labels.clone(),
            refreshed: true,
        }
    };
    show_layouts(host, state, Some(after_shown));
}

/// The refreshed results are on screen: the selection and the keep ticks the cutter had come back.
pub(in crate::gui::rough_plan) fn after_shown(host: &Rc<Host>) {
    let Some(saved) = host.session.borrow_mut().axis.restore.take() else {
        return;
    };
    let count = host.session.borrow().run.layouts.len();
    let mut keep = saved.keep;
    keep.resize(count, false);
    host.session.borrow_mut().run.keep.clone_from(&keep);
    let model = host.window.global::<RoughPlanModel>();
    let rows = model.get_results();
    for (index, &kept) in keep.iter().enumerate() {
        if let Some(mut row) = rows.row_data(index)
            && row.keep != kept
        {
            row.keep = kept;
            rows.set_row_data(index, row);
        }
    }
    model.set_keep_count(i32::try_from(keep.iter().filter(|&&kept| kept).count()).unwrap_or(0));
    if usize::try_from(saved.result).is_ok_and(|result| result < count) {
        view::axis_view::reselect(host, saved.result, saved.group);
    }
}

/// Mirrors the stone being turned by hand into the window; a stone of a result that is no longer
/// selected is let go.
pub(super) fn push(host: &Rc<Host>) {
    let selected = host.window.global::<RoughPlanModel>().get_selected_result();
    let stone = {
        let mut session = host.session.borrow_mut();
        let still = session
            .axis
            .free
            .as_ref()
            .is_some_and(|free| i32::try_from(free.result).ok() == Some(selected));
        if !still {
            session.axis.free = None;
        }
        session
            .axis
            .free
            .as_ref()
            .and_then(|free| i32::try_from(free.stone).ok())
            .unwrap_or(-1)
    };
    host.window.global::<RoughPlanModel>().set_free_stone(stone);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pose_is_one_of_the_four_box_poses() {
        let choice = StoneChoice {
            locked: true,
            reason: "flaw".to_string(),
            ..StoneChoice::default()
        };
        let posed = with_pose(&choice, "2").expect("pose 2");
        assert_eq!(posed.pose, 2);
        assert!(posed.locked, "a lock stays");
        assert_eq!(posed.reason, "flaw", "a Reason stays");
        assert!(with_pose(&choice, "4").is_none());
        assert!(with_pose(&choice, "x").is_none());
        assert_eq!(with_pose(&choice, " 0 ").map(|c| c.pose), Some(0));
    }

    #[test]
    fn a_pose_replaces_a_pose_placed_by_hand() {
        let hand = StoneChoice {
            free_pose: Some(pleochroma_cut_core::rough_plan::fit::StonePose {
                center_mm: [0.0; 3],
                axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                mm_per_unit: 1.0,
            }),
            ..StoneChoice::default()
        };
        assert!(with_pose(&hand, "1").expect("pose 1").free_pose.is_none());
    }

    #[test]
    fn lock_flips_and_keeps_the_rest() {
        let choice = StoneChoice {
            pose: 3,
            reason: "keep".to_string(),
            ..StoneChoice::default()
        };
        let locked = with_lock_flipped(&choice);
        assert!(locked.locked);
        assert_eq!((locked.pose, locked.reason.as_str()), (3, "keep"));
        assert!(!with_lock_flipped(&locked).locked);
    }

    #[test]
    fn a_reason_is_trimmed_and_limited() {
        let choice = StoneChoice::default();
        assert_eq!(
            with_reason(&choice, "  avoids the crack  ").reason,
            "avoids the crack"
        );
        let long = "x".repeat(MAX_REASON_CHARS + 50);
        assert_eq!(
            with_reason(&choice, &long).reason.chars().count(),
            MAX_REASON_CHARS
        );
        assert!(with_reason(&choice, "  ").is_default());
    }

    #[test]
    fn a_refit_forgets_the_pose_of_the_old_stone_and_keeps_lock_and_reason() {
        let choice = StoneChoice {
            pose: 2,
            locked: true,
            reason: "r".to_string(),
            ..StoneChoice::default()
        };
        let refitted = after_refit(&choice);
        assert_eq!((refitted.pose, refitted.locked), (0, true));
        assert_eq!(refitted.reason, "r");
    }

    #[test]
    fn a_drag_turns_the_pose_about_the_cameras_up_and_right_directions() {
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        // No movement, no turn.
        assert_eq!(
            turned_axes(identity, DVec3::X, DVec3::Y, 0.0, 0.0),
            identity
        );
        // 180 pixels at half a degree each is a quarter turn about the up axis (+Y): +X goes to -Z.
        let turned = turned_axes(identity, DVec3::X, DVec3::Y, 180.0, 0.0);
        assert!(
            (DVec3::from_array(turned[0]) - DVec3::NEG_Z).length() < 1e-9,
            "{turned:?}"
        );
        assert!((DVec3::from_array(turned[1]) - DVec3::Y).length() < 1e-9);
        // The axes stay a proper frame.
        let x = DVec3::from_array(turned[0]);
        let y = DVec3::from_array(turned[1]);
        let z = DVec3::from_array(turned[2]);
        assert!((x.cross(y) - z).length() < 1e-9);
    }
}
