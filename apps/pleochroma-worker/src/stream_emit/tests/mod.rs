//! Tests for `stream_emit`, one topic per file.
//!
//! [`pending_delta`] covers delta coalescing, [`batch_sizing`] the adaptive sub-batch size,
//! [`downsample`] the reduced-resolution `PREVIEW`, [`cadence`] the effective cadence,
//! [`emit_tick`] one cadence-tick emission, [`liveness`] stream-timeout classification and
//! the heartbeat, [`stall`] the producer-stall watchdog end to end, [`ray_failures`] declined
//! ray ranges and [`adaptive_link`] adaptive compression on a throttled or fast wire.
//! [`fixtures`] holds the scene, state and request builders shared by them.

mod adaptive_link;
mod batch_sizing;
mod cadence;
mod downsample;
mod emit_tick;
mod fixtures;
mod liveness;
mod pending_delta;
mod ray_failures;
mod stall;
