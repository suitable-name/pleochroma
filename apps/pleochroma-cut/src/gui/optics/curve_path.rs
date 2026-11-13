//! Converts a fixed-length tilt-angle curve into an SVG path-data string for `Path::commands`
//! in `performance_graph_dialog.slint`.
//!
//! The functions and their tests live in `pleochroma_editor::view_model::curve_path`, shared
//! with the browser app's tilt chart; this module re-exports them for the desktop.

pub use pleochroma_editor::view_model::curve_path::{full_axis_curve_path, tilt_curve_path};
