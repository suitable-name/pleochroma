//! Everything specific to dispatching an export's remote lane against an
//! `pleochroma-worker`: whether remote is even available and within its advertised
//! limits ([`capability`]), how big a chunk to claim next given the rate observed so
//! far ([`rate`]), and running the dispatch loop for the whole concurrent phase
//! ([`dispatch`]).
//!
//! # Where disjointness comes from
//!
//! Nothing here hands out sample ranges itself: every chunk comes from
//! `SampleCursor::claim`, the single shared atomic claim point local and remote both
//! draw from. This module only decides how much to claim next
//! ([`rate::remote_request_samples`]: a target duration for the peer's kind, held to
//! what one request may carry) and runs each claimed chunk to completion or failure
//! ([`dispatch::run_remote_batch`]).
//!
//! # Partial completion, not failure
//!
//! A chunk that comes back short (dropped connection, worker-side error, early
//! `DONE`) is not automatically an export failure: [`dispatch::run_remote_lane`]
//! merges whatever prefix completed and hands the remainder back to
//! `SampleCursor::return_to_local` for the local lane, as long as one exists
//! (`ComputeTarget::Both`). Only `ComputeTarget::RemoteOnly`, with no local lane to
//! fall back to, turns a short chunk into a fatal [`dispatch::RemoteLaneOutcome`].
//!
//! # Beam tiles
//!
//! `beam_tiles` (the `beam` feature) is the client of the whole-tile requests: a beam-traced
//! render gives a joined worker a share of its tiles and gets their finished pixels back.
//!
//! # Live passes
//!
//! `live_pass` (the `adaptive-sampling` feature) is the client of the live passes: an adaptive
//! render gives a joined worker a share of one pass's live tiles. Without the feature there is
//! no such client, and no live pass is sent.
//!
//! # Final picture only
//!
//! [`final_image`] is the other transfer: one `FinalImageRequest` per image, the remote
//! renders AND tone-maps, the viewer receives one PNG. No sample cursor, no local lane.
#[cfg(feature = "beam")]
mod beam_tiles;
mod capability;
mod dispatch;
mod final_image;
#[cfg(feature = "adaptive-sampling")]
mod live_pass;
mod rate;

#[cfg(feature = "beam")]
pub(in crate::bridge::export_thread) use beam_tiles::{
    BeamClientPool, BeamTileClient, BeamTileFailure, BeamTileOrder, BeamTiles,
};
pub(in crate::bridge::export_thread) use capability::{REMOTE_MIN_SPP, exceeds_pixel_cap};
pub use capability::{RemoteCapability, probe_remote};
pub(in crate::bridge::export_thread) use dispatch::{
    RemoteLaneOutcome, RemoteProgress, run_remote_lane, scene_state_from_snapshot,
    scene_state_from_snapshot_with,
};
pub use final_image::forget_final_picture_refusals;
pub(in crate::bridge::export_thread) use final_image::{
    FinalPictureFollowUp, FinalPictureOutcome, LocalShare, TransferPlan, final_picture_follow_up,
    final_picture_refused, plan_export_transfer, record_split_rates,
    remember_final_picture_refused, run_final_image_request, split_rates, viewer_share,
};
#[cfg(feature = "adaptive-sampling")]
pub(in crate::bridge::export_thread) use live_pass::{LivePassClient, LivePassFailure, LiveStream};
pub(in crate::bridge::export_thread) use rate::{
    REMOTE_CALIBRATION_SAMPLES, RemoteCalibration, calibrate_remote_rate,
};
