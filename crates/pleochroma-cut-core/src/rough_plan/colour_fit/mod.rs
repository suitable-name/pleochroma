//! Rough colour and colour zoning from rig photos.
//!
//! - [`forward`]: the forward path tracer of the rig. It traces every working pixel's
//!   paths once, stores them as `(zone lengths, weight)` records, and evaluates any candidate
//!   absorption on them without tracing again.
//! - [`ColourRig`]: the rig profile together with its light model. It lives here and not inside
//!   [`RigProfile`](crate::rough_plan::locate::RigProfile), so the saved rig bytes stay unchanged.

pub mod forward;
mod rig;
pub mod solve;
pub mod zones;

pub use rig::ColourRig;
