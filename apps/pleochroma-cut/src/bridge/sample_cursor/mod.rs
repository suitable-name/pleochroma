//! [`SampleCursor`]: the shared claim point every backend contributing to one image
//! draws disjoint absolute sample sub-ranges from -- the export's local and remote
//! lanes, and (through [`live::LiveEpoch`]) the live viewport's local tracer and its
//! remote chunk lane.
//!
//! The type is defined in the GUI-free `pleochroma-dispatch` crate, which the render
//! coordinator shares, and re-exported here. Its docs there cover disjointness and the
//! retry piles.
//!
//! [`LiveEpoch`] stays here: it is the live viewport's single remote lane (one in-flight
//! `Accumulator`, the app's scene-generation stamp). Its multi-lane counterpart is
//! `pleochroma_dispatch::LanePool`.

pub mod live;
#[cfg(test)]
pub(in crate::bridge) mod tests;

pub use live::{ChunkEnd, LiveEpoch};
pub use pleochroma_dispatch::SampleCursor;
