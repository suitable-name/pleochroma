//! The export side of [`crate::design::Design`]: the [`pleochroma_formats::asc::AscSchedule`], the
//! full plane arrangement, and the meshed solid with its measurements.
//!
//! Each has an "already solved" variant next to the wrapper that solves first; see
//! [`crate::design::Design::to_asc_schedule_from_solved`] for why.

mod concave;
pub mod concave_frame;
mod planes;
mod refractive;
mod schedule;

#[cfg(test)]
mod concave_tests;
#[cfg(test)]
mod tests;

pub use concave::{ConcaveResolveError, FlatAndTools, ToolPlacements, resolve_tiers_on_planes};
pub use schedule::{
    CONCAVE_FOOTNOTE_MARKER, concave_facet_footnote, concave_tool_footnote, meet_name_is_asc_safe,
};
