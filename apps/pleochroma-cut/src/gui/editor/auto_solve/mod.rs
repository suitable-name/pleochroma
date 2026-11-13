//! Background solving for the "Edit" sub-tab.
//!
//! The explicit "Solve" action (button and F5, via [`super::view::refresh_all`]'s large-design
//! path) and the auto-solve after an edit both run `Design::solve` off the UI thread. See the
//! "Never block the UI thread with a solve" section of `gui/editor/mod.rs`.
//!
//! # Why a `thread_local!`, not a new `EditorState` field
//!
//! The state this module needs (debounce timer, request counter, last solve time,
//! shared preview handles) lives in [`runtime::Runtime`], a `thread_local!`
//! singleton. Only UI/event-loop code touches it, the same thread-confinement that
//! lets `EditorState` use `Rc<RefCell<..>>`. Worker threads compute pure `Design`
//! to data conversions and return the result across the event-loop boundary.
//!
//! # Two kinds of "stale" a completed background solve must detect
//!
//! A solve runs on a snapshot: a cloned [`Design`] plus the [`EditorState::generation`]
//! value at dispatch. On completion, [`apply::apply_background_solve_result`] checks
//! for two kinds of staleness:
//!
//! 1. **A newer dispatch exists.** [`runtime::Runtime::current_seq`] is bumped on every
//!    dispatch. A completion whose sequence differs touches nothing; the
//!    newer dispatch owns the "Solving..." banner.
//! 2. **The design changed without a new dispatch.** This happens when auto-solve is
//!    off or over budget and an edit lands during an explicit Solve. The sequence is
//!    still current, but `generation` has moved on. Only the running flag is cleared,
//!    since the edit already refreshed the banner.
//!
//! Both checks are `u64` comparisons. A stale result is never applied to a design it
//! does not describe.
//!
//! # Layout of this group
//!
//! [`runtime`] holds the thread-local state. [`scheduling`] decides when a solve runs
//! synchronously or is debounced into an auto-solve. [`dispatch`] spawns the solve and
//! ticks its banner. [`apply`] applies the result to the UI. [`replan`] is the
//! solid-preview replan handshake. [`tests`] covers all of these.

mod apply;
mod dispatch;
mod replan;
mod runtime;
mod scheduling;
#[cfg(test)]
mod tests;

pub(super) use dispatch::{cancel_in_flight_solve, dispatch_background_solve, solve_cancellably};
pub(super) use replan::{
    schedule_idle_replan_if_stale, stash_current_design, take_matching_design,
};
pub(super) use runtime::{
    activity, editor_state, init, preview_state, render_ctx, solid_last_solved, stash_editor_state,
    stash_render_ctx,
};
pub(super) use scheduling::{
    clear_frame_pending, last_solve, on_edit, preview_may_push_status, record_solve_duration,
    reset_for_new_design,
};
