//! The wire messages, and encode/decode for each over a length-prefixed
//! [`crate::framing`] stream.
//!
//! ```text
//! -> HELLO    { protocol_version, build_hash, source_hash, role, capability, accept_encodings }
//! <- WELCOME  { protocol_version, build_hash, source_hash, render: Option<RenderCapability>,
//!               library, tilt_curves, registration: Option<WorkerRegistration>,
//!               payload_encoding }
//! -> <ClientMessage>  Cancel | Library(LibraryRequest) | RenderRequest | TiltCurvesRequest
//!                     | Ping { nonce } | FinalImageRequest | Asset { content_hash, len }
//!                       + payload
//!                     | Contribution + payload                        (post-handshake, tagged)
//!                     | ZoningPayload (`render` feature only; no protocol bump, see `zoning`)
//!                     | LivePassRequest (`render` feature only, see `live_pass`)
//!                     | BeamTileRequest (`render` feature only, see `beam_tiles`)
//! <- <StreamEvent>    Frame | Preview | Progress | Done | Error
//!                     | Pong { nonce } | DisplayFrame | FinalImage | CapabilityChanged { render }
//!                     | NeedAsset { content_hash }                      (tagged)
//!                     | LivePass (the reply to a LivePassRequest)
//!                     | BeamTiles (the reply to a BeamTileRequest)
//! <- <TiltCurvesResponse>  Curves | Cancelled | Error
//!                          (TILT_CURVES's single reply, not a stream)
//! ```
//!
//! Split across submodules by topic:
//!
//! - [`hello`]: handshake messages, `HELLO`/`WELCOME`, peer roles.
//! - [`asset`]: content-addressed assets: `NEED_ASSET`/`ASSET`, the
//!   SHA-256 [`content_hash`] and the bounded payload read.
//! - [`encoding`]: payload-encoding negotiation.
//! - [`adaptive`]: per-frame encoding choice that follows the measured link bandwidth;
//!   needs no wire change (headers name each frame's encoding).
//! - [`render`]: `RENDER`'s request shape (`render` feature only).
//! - [`final_image`]: `FINAL_IMAGE_REQUEST`'s shape and reply semantics (`render` feature
//!   only).
//! - [`contribution`]: `-> CONTRIBUTION`, the viewer's own share of a final-picture
//!   export (`render` feature only).
//! - `batch`: `-> BATCH_RENDER_REQUEST`, several finished pictures per request on one
//!   persistent connection (`render` feature only).
//! - `live_pass`: `-> LIVE_PASS_REQUEST`, one chunk of an adaptive-sampling pass for a set of
//!   live tiles (`render` feature only). A server built without the apps' optional
//!   `adaptive-sampling` feature refuses it with `UNSUPPORTED_REQUEST`.
//! - `beam_tiles`: `-> BEAM_TILE_REQUEST`, whole beam-traced tiles (beams and residual) for a set
//!   of tiles (`render` feature only). `BEAM_TILE_REQUEST` and `BEAM_TILES` are part of every
//!   `render` build so that indices and decoding agree between builds; only builds with the apps'
//!   retired `beam` features send or serve them; a worker or coordinator built without the
//!   feature answers `UNSUPPORTED_REQUEST` (code 5) and keeps the connection.
//! - [`tilt`]: `TILT_CURVES`'s request/response shapes (`render` feature only).
//! - [`stream`]: everything else post-handshake -- [`stream::ClientMessage`],
//!   [`stream::StreamEvent`], and [`stream::ErrorMsg`].
//! - [`error_codes`]: the `ErrorMsg::code` vocabulary.
//! - [`codec`]: the generic framed codec ([`codec::NetError`],
//!   [`codec::write_message`]/[`codec::read_message`]) every one of the above builds on.
//!
//! Every item is re-exported here at its original flat `messages::` path.

pub mod adaptive;
pub mod asset;
/// Batched finished-picture requests -- see this module's own doc comment.
#[cfg(feature = "render")]
pub mod batch;
/// Beam-traced tiles -- see this module's own doc comment.
#[cfg(feature = "render")]
pub mod beam_tiles;
mod codec;
#[cfg(feature = "render")]
mod contribution;
mod encoding;
mod encoding_matrix;
pub mod envmap_asset;
/// The `ErrorMsg::code` values.
pub mod error_codes;
#[cfg(feature = "render")]
mod final_image;
mod hello;
/// Durable server jobs -- see this module's own doc comment.
#[cfg(feature = "render")]
pub mod job;
/// Adaptive-sampling live passes -- see this module's own doc comment.
///
/// The messages are part of every build; a server without the apps' `adaptive-sampling`
/// feature refuses them.
#[cfg(feature = "render")]
pub mod live_pass;
#[cfg(feature = "render")]
mod render;
mod stream;
#[cfg(feature = "render")]
mod tilt;
/// The zoning wire extension (`render` feature only; no protocol bump).
///
/// A capability marker after `HELLO`/`WELCOME` and the separate `ZoningPayload` message -- see
/// this module's own doc comment.
#[cfg(feature = "render")]
pub mod zoning;

#[cfg(feature = "render")]
pub use asset::write_asset_message;
pub use asset::{
    AssetError, AssetHeader, ContentHash, MAX_ASSET_LEN, content_hash, discard_asset_payload,
    hash_hex, read_asset_payload,
};
#[cfg(feature = "render")]
pub use batch::{
    BatchItem, BatchRenderRequest, BatchReply, MAX_BATCH_ITEMS, MAX_BATCHES_IN_FLIGHT,
};
#[cfg(feature = "render")]
pub use beam_tiles::{
    BEAM_TILE_PIXEL_BYTES, BeamTileError, BeamTilePixel, BeamTileRequest, MAX_BEAM_TILE_PIXELS,
    beam_tiles_reply, decode_beam_tile_pixels, decode_beam_tile_request, decode_beam_tiles_reply,
    encode_beam_tile_pixels, encode_beam_tile_request,
};
pub use codec::{
    NetError, decode_control_frame, read_control_message, read_message, read_message_bounded,
    write_message,
};
#[cfg(feature = "render")]
pub use contribution::{
    ContributionError, ContributionHeader, ExpectedContribution, discard_contribution_payload,
    read_contribution_payload, write_contribution_message, write_contribution_message_with_link,
};
pub use encoding::{
    DEFAULT_SERVER_PREFERENCE, DisplayEncoding, LOOPBACK_SERVER_PREFERENCE, PayloadEncoding,
    negotiate,
};
#[cfg(feature = "render")]
pub use final_image::{FinalImageRequest, FinalOutput, WireColorSpace};
pub use hello::{Backend, Hello, PeerRole, RenderCapability, Welcome, WorkerRegistration};
#[cfg(feature = "render")]
pub use job::{
    JobId, JobInfo, JobPhase, JobRequest, JobResponse, JobSubmit, MAX_JOB_LABEL_CHARS,
    MAX_JOB_LIST, MAX_JOB_RESULT_LEN, MAX_JOB_SAMPLES, derive_job_id, job_id_hex, parse_job_id_hex,
    read_job_response, read_job_result, write_job_asset, write_job_request, write_job_response,
    write_job_result,
};
#[cfg(feature = "render")]
pub use live_pass::{
    LIVE_PIXEL_BYTES, LivePassError, LivePassRequest, LiveTileSet, MAX_LIVE_PASS_PIXELS,
    PATH_CACHE_FLAG, decode_live_sums, encode_live_sums, gather_live, request_id_with_path_cache,
    scatter_live,
};
#[cfg(feature = "render")]
pub use render::{PreviewConfig, RenderRequest, RequestIntent, StreamConfig, TransferMode};
pub use stream::{
    BatchDone, BatchItemDoneHeader, BatchItemFailed, BatchItemProgress, BeamTileStats,
    BeamTilesHeader, Cancel, ClientMessage, DisplayFrameHeader, Done, ErrorMsg, FinalImageHeader,
    FrameHeader, LivePassHeader, PreviewHeader, Progress, Stats, StreamEvent, read_frame_message,
    read_preview_message, read_stream_event, write_frame_message, write_preview_message,
    write_stream_event,
};
#[cfg(feature = "render")]
pub use tilt::{
    AxisTiltCurves, TILT_CURVE_AXIS_COUNT, TILT_CURVE_POINTS_PER_AXIS, TiltCurvesRequest,
    TiltCurvesResponse, TiltCurvesResult,
};
#[cfg(feature = "render")]
pub use zoning::{
    MAX_ZONED_ENTRIES, ZONING_TAIL, ZonedMaterialEntry, ZoningPayload, ZoningPayloadError,
    hello_from_bytes, split_welcome_tail, write_hello_message, write_welcome_message,
    write_zoning_payload,
};

/// The wire protocol version this build of `pleochroma-net` speaks.
///
/// Bumped only for changes to the MESSAGE SHAPES in this module. A change to `pleochroma`'s
/// physics does not touch the wire format and is caught instead by the two-level identity
/// check in [`crate::handshake`] (`build_hash`, and -- when both sides can establish it --
/// `source_hash`).
///
/// This is pre-release software: every peer runs its own private build, so there is no
/// deployed fleet to stay wire-compatible with, and this crate carries no compatibility
/// shim or version-conditional decode path -- [`crate::handshake::verify_compatible`]
/// simply refuses to pair peers speaking different versions. The bump's job is making
/// that refusal a clear, diagnosable handshake failure instead of a peer silently
/// mis-decoding garbage.
///
/// Note: [`StreamEvent::Progress`] doubles as the stream's liveness heartbeat, not just
/// real sample progress -- a peer must not assume otherwise.
///
/// # When you MUST bump this
///
/// The wire codec is [`postcard`], which is **not self-describing**:
///
/// - **Enums are encoded by declaration-order index, not name.** An older peer sees an
///   unrecognized index on an appended variant and fails outright -- it cannot skip it.
/// - **Structs are encoded as fields in declaration order, no names, no length prefix.**
///   Appending a field is worse: an older peer silently MISALIGNS every field after it.
///   Applies transitively to anything a wire struct embeds (e.g.
///   [`crate::scene::SceneState`]'s `GemMaterial`, inside [`RenderRequest`]).
///
/// So: append a variant to any wire enum, or a field to any wire struct (or anything it
/// embeds), and bump this -- turning silent corruption into an up-front, diagnosable
/// version-mismatch log line.
///
/// **`#[serde(default)]` does not help here**: it matters only for self-describing
/// formats (e.g. `pleochroma-worker`'s local `scene.json`), not postcard's fixed-layout
/// encoding.
///
/// 9: three lighting models appended to `LightingPreset`.
/// 10: `LightingPreset` 4-6 redefined. Same encoding, but a peer on 9 shades those scenes
///     differently.
/// 11: `SceneState::backdrop` appended.
/// 12: `Hello`/`Welcome` gained `source_hash`, a content hash of the `pleochroma` source
///     tree. It is carried for diagnostics only and never compared; see [`crate::handshake`].
/// 13: `LibraryRequest::Search` gained `order` and `tag_filter`. Without them a remote
///     search ignores the sort and tag choices.
/// 14: coordinator mode and lossless payload compression. `Hello` gained `role`,
///     `capability` and `accept_encodings`; `Welcome` gained `registration` and
///     `payload_encoding`; `RenderRequest` gained `intent`; frame headers gained `encoding`
///     and `raw_len`; `ClientMessage::{Ping, FinalImageRequest}`,
///     `StreamEvent::{Pong, DisplayFrame, FinalImage, CapabilityChanged}` and the error codes
///     `UNSUPPORTED_REQUEST`, `ALL_WORKERS_LOST` and `ROLE_REFUSED` were added. The first three
///     `Hello`/`Welcome` fields are unchanged, so either side can still read the other's
///     version and refuse with a message naming both. HDR environments (`ClientMessage::Asset`,
///     `StreamEvent::NeedAsset`, `ASSET_FAILED`) are part of this version too; see [`asset`].
/// 15: `ErrorMsg` gained `request_id`, so a late error for an abandoned request is dropped
///     instead of failing the current one; see [`ErrorMsg`]. `library::DesignRecord` gained
///     fields under the same bump.
/// 16: `FinalImageRequest.viewer_samples`, `ClientMessage::Contribution` and
///     `Stats.reclaimed_samples`; see `messages::contribution`.
/// 17: `library::DesignSummary::design_version` appended, and `library::DesignRecord::version`
///     carries that revision token, so a mirror sees an edit that leaves the summary unchanged.
/// 18: `SceneState::surface_glare` appended.
/// 19: concave facets. `SceneState::tools`, `library::DesignSummary::concave_tiers`,
///     `library::AngleSettingWire::tool_line` and the native-design request and reply variants
///     were appended. `DesignSummary::version` also hashes `concave_tiers`.
/// 20: fluorescence and UV lamps. `LightingPreset::{UvLamp365, UvLamp395}` and
///     `SceneState::fluorescence` were appended. A scene with either is traced on the CPU only.
/// 21: `SceneState::head_shadow_deg` appended.
/// 22: `LightingPreset` variants 9 to 14 and `LightingModel::{Aset, DaylightSun}` added.
/// 23: `GemMaterial::absorption_unit` appended, and inclusion scattering is not scaled by
///     the path scale. A peer on 22 would misalign the material.
/// 24: batched previews: `ClientMessage::BatchRenderRequest` and the four `StreamEvent::Batch*`
///     events; see `messages::batch`.
/// 25: an HDR asset may be an `IXENVMAP` float asset (`messages::envmap_asset`) as well as a
///     Radiance file, and durable jobs add `ClientMessage::Job` and `JobResponse`.
/// 26: `SceneState::frosted_planes` appended, and the library carries frosting too
///     (`library::DesignRecord::frosting`, `LibraryResponse::DesignSource::frosting`).
/// 27: turn-aware Retarget bounds, bumped together with the browser worker protocol. No
///     message changed shape; the bump pairs peers built on the same side of that rule.
///
/// A `render` build appends `ClientMessage::ZoningPayload` (index 9) WITHOUT a bump: it is
/// sent only to a peer whose `WELCOME` carried the zoning capability marker, which a peer
/// sends only in reply to a `HELLO` that carried the marker -- so a peer without the
/// extension never sees the message or the marker. See the `zoning` module.
/// 28: deterministic Ray-Traced image engines; `SceneState::engine` appended.
/// 29: `SceneState::backdrop` means the radiance of a lit tray below the girdle plane, seen
///     by every escape, not a card only the camera ray sees. A peer on 28 would merge
///     samples from different trays.
/// 30: `GemMaterial::alpha_axis` appended, and the GPU material buffer grew by one `vec4`.
///     A worker on 29 would trace a biaxial stone with the wrong second crystal axis.
/// 31: `ZonedAbsorption::follows_axis` appended. Its postcard form is fixed width, so a peer
///     on 30 would misalign the zones after it.
/// 32: render physics: `GemMaterial::biaxial_principal_dispersion` and `SceneState::effects`
///     appended. A peer on 31 would misalign the material and ignore the effects.
/// 33: `SceneState::camera_roll` appended.
/// 34: `SceneEffects::material_inclusions` appended. A peer on 33 would misalign the effects.
/// 35: HDR header `EXPOSURE` and `COLORCORR` are applied. No message changed shape, but a
///     peer on 34 would light such a file differently.
/// 36: a coordinator forwards zones (see [`zoning`]). The bump also separates builds whose
///     Ray-Traced engine tints by the binned absorption spectrum (`WHITTED_VERSION` 3) and
///     whose final pictures use the keep-color highlight rule by default, from earlier builds.
/// 37: adaptive live passes: `ClientMessage::LivePassRequest` and `StreamEvent::LivePass`
///     appended. The messages stay in every build; see `messages::live_pass`.
/// 38: beam tiles: `ClientMessage::BeamTileRequest` and `StreamEvent::BeamTiles` appended.
///     The bump is global, so the worker fleet is updated together.
/// 39: an HDR-lit scene is lit at `SceneState::exposure`. A worker on 38 would ignore it.
/// 40: `SceneState::stone_shadow` appended after `camera_roll`.
/// 41: `SceneState::tray_reflection` appended after `stone_shadow`.
/// 42: `SceneState::hdr_orientation` appended after `tray_reflection`, then
///     `SceneState::estimator` (the stone estimator every engine of the frame runs). A viewer
///     and its workers must be replaced together.
pub const PROTOCOL_VERSION: u16 = 42;

#[cfg(test)]
mod tests {
    #[test]
    /// Pins the constant so a bump is always a deliberate, reviewed edit.
    fn protocol_version_matches_constant() {
        assert_eq!(super::PROTOCOL_VERSION, 42);
    }
}
