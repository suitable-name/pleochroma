//! The Crystal Axis in the Rough Planner: the axis of the rough, the rule stones are oriented by,
//! what the results say about each stone, and what the cutter can do with one stone.
//!
//! The logic of the orientation (the rules, the fits, the lock, the alternatives) is in
//! `pleochroma_cut_core::rough_plan::orientation`; this module is its window:
//!
//! - [`panel`]: the axis of the rough (Unknown or Set, the angles, Align to Face, Align to Edge,
//!   Fit to Crystal Length, From Color Zones). The axis is part of the rough model, so every change
//!   is one step of the shape undo stack.
//! - [`rules`]: the plan's rule and the rule of one design (Material Default, Ignore the Axis, or a
//!   rule of your own with a tolerance and Preferred or Required), each edit with its Reason one
//!   undo step.
//! - [`rows`]: the stones of the selected result with the angle of their crystal axis to the table
//!   and whether they follow the rule, and the same lines in the cut plan text.
//! - [`stone`]: the stone menu (box poses, Re-Fit This Piece, Free Orientation, Lock, Reason).
//! - [`layouts`]: the plain-data side of changed layouts and locked stones.
//! - [`material`]: the plan's material, built in or from the library.
//! - [`state`]: what the window keeps that is not undoable.
//!
//! The 3D view's part (the axis drawn on the rough, picking a face or an edge, turning a stone by
//! hand) is `view::axis_gizmo`; the run's part (the rule each design is cut under, the locked
//! stones, the alternatives) is `run::axis_plan`.
//!
//! A plan that never sets the axis goes through none of it: nothing is drawn, no rule applies and
//! its layouts and saved plans are exactly what they always were.

pub(super) mod layouts;
pub(super) mod material;
mod panel;
mod rows;
mod rules;
pub(super) mod state;
mod stone;

pub(super) use self::state::{AxisState, PickMode};
pub(in crate::gui::rough_plan) use self::{
    panel::{apply_orientation, finish_live, set_live},
    stone::{finish_turn, is_turning, locked_for_plan, turn},
};
use super::host::Host;
use std::rc::Rc;

/// Registers the callbacks of the axis panel, the rule editor and the stone menu.
pub(super) fn setup(host: &Rc<Host>) {
    panel::setup(host);
    rules::setup(host);
    stone::setup(host);
}

/// Mirrors everything about the crystal axis into the window: the panel, the rules, the stone
/// being turned, the rows of the selected result; and shows the layouts the cutter changed when
/// they are not the ones on screen (after an edit, an undo or a redo).
pub(super) fn sync(host: &Rc<Host>) {
    panel::push(host);
    rules::push(host);
    stone::push(host);
    rows::push(host);
    stone::refresh_layouts(host);
}

/// The pick a click on the 3D view makes now.
#[must_use]
pub(in crate::gui::rough_plan) fn pick_mode(host: &Host) -> PickMode {
    host.session.borrow().axis.pick
}

/// Sets the pick the next click makes (the first click of an edge turns the pick into the second).
pub(in crate::gui::rough_plan) fn set_pick(host: &Rc<Host>, pick: PickMode) {
    host.session.borrow_mut().axis.pick = pick;
    panel::push(host);
}
