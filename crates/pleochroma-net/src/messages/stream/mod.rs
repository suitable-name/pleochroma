//! Every message a client may send after the handshake ([`ClientMessage`]), and every
//! reply a worker sends for one `RENDER` request ([`StreamEvent`]) -- plus the small
//! per-message structs both are built from, and [`ErrorMsg`], used well beyond just
//! this file (handshake refusal, library-request errors).
//!
//! ```text
//! -> CANCEL   { request_id }
//! -> PING     { nonce }
//! <- FRAME    { request_id, first_sample, samples, payload_len, encoding, raw_len }
//!              + payload -- DELTA, full-res
//! <- PREVIEW  { request_id, width, height, samples_done, payload_len, encoding, raw_len }
//!              + payload -- CUMULATIVE, reduced-res
//! <- PROGRESS { request_id, samples_done }
//! <- DONE     { request_id, cancelled, stats: Stats }
//! <- ERROR    { code, message }
//! <- PONG     { nonce }
//! <- DISPLAY_FRAME { request_id, samples_done, width, height, encoding, payload_len } + payload
//! <- FINAL_IMAGE   { request_id, width, height, samples_done, encoding, payload_len } + payload
//! <- CAPABILITY_CHANGED { render: Option<RenderCapability> }
//! <- NEED_ASSET { content_hash }
//! -> ASSET    { content_hash, len } + payload
//! -> CONTRIBUTION { request_id, first_sample, samples, width, height, encoding,
//!                   payload_len, raw_len } + payload    (see crate::messages::contribution)
//! -> BATCH_RENDER_REQUEST { request_id, reply, items }    (see crate::messages::batch)
//! <- BATCH_ITEM_PROGRESS  { request_id, item_id, samples_done }
//! <- BATCH_ITEM_DONE      { request_id, item_id, samples_done, payload_len } + PNG
//! <- BATCH_ITEM_FAILED    { request_id, item_id, reason }
//! <- BATCH_DONE           { request_id, cancelled }
//! -> LIVE_PASS_REQUEST    { request_id, scene, first_sample, samples, live }
//!                          (see crate::messages::live_pass)
//! <- LIVE_PASS            { request_id, first_sample, samples, live_pixels, payload_len }
//!              + payload
//! -> BEAM_TILE_REQUEST    { request_id, scene, tiles, beam, adaptive, sample_offset }
//!                          (see crate::messages::beam_tiles)
//! <- BEAM_TILES           { request_id, sample_offset, pixels, stats, payload_len } + payload
//! ```
//!
//! `FRAME`/`PREVIEW` payloads are encoded per their header's `encoding` (see
//! `crate::radiance::payload`); `DISPLAY_FRAME`/`FINAL_IMAGE` payloads are RGBA8 or PNG
//! (`crate::display`).
//!
//! `TILT_CURVES` (see [`super::tilt`]) shares this file's [`ClientMessage`]/[`Cancel`]
//! on the request side but NOT [`StreamEvent`] on the reply side -- it is a single
//! request/response family, not a stream.
//!
//! `FRAME` and `PREVIEW` carry their radiance payload as a raw frame, never through
//! `postcard` (see [`crate::radiance`]): each is sent as two consecutive frames, a small
//! `postcard`-encoded header then the payload bytes verbatim, via [`write_frame_message`]/
//! [`read_frame_message`] and [`write_preview_message`]/[`read_preview_message`], all of
//! which cross-check the header's declared `payload_len` against the actual byte count.
//!
//! # `FRAME` (delta, full resolution) vs `PREVIEW` (cumulative, reduced resolution)
//!
//! Not interchangeable -- the distinction is load-bearing for correctness:
//!
//! - `FRAME` carries a DELTA -- the summed contribution of exactly the sample sub-range
//!   named by [`FrameHeader::first_sample`]/[`FrameHeader::samples`], at full
//!   resolution. A viewer SUMS every `FRAME` it receives (whose `request_id` matches its
//!   current epoch) into its running total. This is what makes deltas coalesce
//!   losslessly under backpressure: two adjacent, not-yet-sent deltas sum to one delta
//!   over their union, identical whether sent as one `FRAME` or several.
//! - `PREVIEW` carries a CUMULATIVE, reduced-resolution snapshot -- the full running
//!   total so far, downsampled to [`PreviewHeader::width`] x [`PreviewHeader::height`].
//!   Each `PREVIEW` SUPERSEDES the previous one; a viewer never sums two `PREVIEW`s, and
//!   never sums one into the full-resolution accumulator (a reduced buffer isn't
//!   additive with a full-resolution one). This also makes `PREVIEW` freely droppable
//!   under backpressure: the next one already reflects everything the dropped one did.
//!
//! # `request_id` and cancellation epochs
//!
//! Every message from `RENDER` onward echoes the `request_id` the client chose for that
//! `RENDER`. This makes "never merge a stale partial into the next render" mechanical: a
//! `CANCEL` can be in flight past a worker already mid-batch, so `FRAME`/`PREVIEW`
//! payloads for the just-cancelled request may still arrive after the client moves on.
//! Rule: sum/display a payload iff its `request_id` matches the current epoch, drop
//! everything else -- no connection-drop-and-reconnect required.
//!
//! # [`ClientMessage`]: why every post-handshake client message is tagged
//!
//! A client may pipeline its next `RenderRequest` before `DONE` for the one still streaming,
//! so whatever arrives mid-stream could be a `CANCEL` or the next request. [`ClientMessage`]
//! resolves this the same way [`StreamEvent`] resolves the reply side: a tag says which, never
//! inferred from position.
//!
//! [`ClientMessage::Library`] extends the same tagged envelope to the read-only
//! design-library protocol ([`crate::library`]), and [`ClientMessage::TiltCurvesRequest`]
//! extends it again to the `TILT_CURVES` sweep family ([`super::tilt`]) -- one message
//! type for a worker's post-handshake read loop to decode regardless of family. Each
//! replies with its own single response, never a [`StreamEvent`].
//!
//! **Variant order is deliberately NOT source order.** [`ClientMessage::Cancel`] and
//! [`ClientMessage::Library`] come first and are ALWAYS compiled in, at the same
//! `postcard` discriminant regardless of this crate's `render` feature. Every later variant
//! is `render`-gated and appended in declaration order, so a library-only worker and a full
//! worker agree on tags 0 and 1, and exchange higher tags only with a peer whose `WELCOME`
//! proved it has the matching capability. A new variant goes after the last one.
//!
//! Split across submodules by topic:
//!
//! - [`types`]: the data types -- header/message structs, [`ClientMessage`],
//!   [`StreamEvent`].
//! - [`wire`]: read/write helpers for [`StreamEvent`] and its `FRAME`/`PREVIEW` variants.
//!
//! Every item is re-exported here at its original flat `stream::` path.

#[cfg(test)]
mod tests;
mod types;
mod wire;

pub use types::{
    BatchDone, BatchItemDoneHeader, BatchItemFailed, BatchItemProgress, BeamTileStats,
    BeamTilesHeader, Cancel, ClientMessage, DisplayFrameHeader, Done, ErrorMsg, FinalImageHeader,
    FrameHeader, LivePassHeader, PreviewHeader, Progress, Stats, StreamEvent,
};
pub use wire::{
    read_frame_message, read_preview_message, read_stream_event, write_frame_message,
    write_preview_message, write_stream_event,
};
