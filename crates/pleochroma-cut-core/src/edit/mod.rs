//! Edit operations on a [`crate::design::Design`], with undo/redo.
//!
//! Undo is business logic, not UI plumbing, so it must be correct and testable without a
//! window. The model is a command/inverse pair: applying an [`Edit`] to a
//! [`crate::design::Design`] returns the exact [`Edit`] that undoes it. [`History`] thus
//! needs no separate reverse logic, and it stores only edits, never the design.
//!
//! [`Edit::SetConstraint`] exists alongside `ModifyTier` (which already covers
//! "replace the whole tier, constraint included") purely as the ergonomic
//! single-field version an editor UI wants for "change what this facet meets"
//! without re-typing angle/name/indices.
//!
//! # Module layout
//!
//! [`edit_type`] is [`Edit`]/[`EditError`] themselves; [`apply`] is
//! [`crate::design::Design::apply_edit`], the one command/inverse primitive;
//! [`history`] is [`History`], the undo/redo stack built on top of it.

mod apply;
mod apply_concave;
mod apply_frosting;
mod apply_shift;
mod apply_surface;
mod edit_type;
mod history;
mod schedule_state;
#[cfg(test)]
mod tests;

pub use apply::remap_ratio;
pub use edit_type::{Edit, EditError, RemapRounding};
pub use history::{History, HistoryEntry, JumpError};
pub use schedule_state::ScheduleState;
