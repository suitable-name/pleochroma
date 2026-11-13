//! Tilt-curve UI wiring: the performance-graph dialog's material/geometry-change callback
//! ([`tilt_profile`]) and the tilt-curve hover preview ([`tilt_hover_preview`]).
//!
//! Distinct from `gui::batch::tilt`, the batch tilt-curve *computation* job: these modules are
//! interactive, single-design UI wiring, not the background batch.

pub mod tilt_hover_preview;
pub mod tilt_profile;
pub mod video_export;
