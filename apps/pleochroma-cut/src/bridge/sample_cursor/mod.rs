//! [`SampleCursor`]: the shared claim point every backend contributing to one image
//! draws disjoint absolute sample sub-ranges from -- the export's local and remote
//! lanes, and (through [`live::LiveEpoch`]) the live viewport's local tracer and its
//! remote chunk lane.
//!
//! `SampleCursor` itself now lives in the GUI-free `pleochroma-dispatch` crate (so the
//! render coordinator shares the exact same claim semantics) and is re-exported here
//! unchanged; see its own docs there for disjointness and the retry piles. Every
//! existing `bridge::sample_cursor::SampleCursor` path keeps compiling.
//!
//! [`LiveEpoch`] stays here: it is the live viewport's single-remote-lane bookkeeping
//! (one in-flight `Accumulator`, the app's scene-generation stamp), which the
//! coordinator replaces with `pleochroma_dispatch::LanePool` rather than reuses.

pub mod live;
#[cfg(test)]
pub(in crate::bridge) mod tests;

pub use live::{ChunkEnd, LiveEpoch};
pub use pleochroma_dispatch::SampleCursor;
