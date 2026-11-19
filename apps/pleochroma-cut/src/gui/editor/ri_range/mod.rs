//! The traced RI range of the open design: the worker, the Stone tab chip, the Design Settings
//! field and the cutting sheet's wait for the estimate.

mod export_wait;
pub(in crate::gui::editor) mod field;
mod present;
mod worker;

pub(in crate::gui::editor) use export_wait::details_then;
#[cfg(feature = "ui-harness")]
pub(in crate::gui) use worker::holds_database;
pub(in crate::gui) use worker::{
    design_changed, design_cleared, latest, note_solved, release_window_state, setup, when_ready,
};
