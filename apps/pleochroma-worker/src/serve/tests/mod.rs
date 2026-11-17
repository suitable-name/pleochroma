//! Tests for `serve`, split by topic.
//!
//! The topics are [`handshake`] (`HELLO`/`WELCOME` and validation),
//! [`render_roundtrip`] (real-socket `RenderRequest`/CPU-GPU parity),
//! [`streaming`] (delta tiling, `FinalOnly` progress, `PREVIEW` isolation),
//! [`cancellation`] (`CANCEL`, the write-timeout bound, pipelining), [`tilt_curves`]
//! (the connection stays usable afterward), [`v14`] (payload-encoding negotiation and
//! compressed streams, `PING`/`PONG`, the v14 refusals), [`hdr_assets`] (an HDR
//! scene's map fetched once by `NEED_ASSET`/`ASSET`), [`lazy_library`] (a connection
//! opens the library database on its first library request, never for anything else),
//! [`batch`] (protocol v24 batched preview requests: shared PNG bytes, per-item failure,
//! `CANCEL`, two batches on one connection), `live_pass` (adaptive live passes;
//! `adaptive-sampling` builds), `live_pass_off` (the refusal of a live pass in a build without
//! that feature), [`estimator`] (a collect scene served with exit collection, protocol 42).
//! [`fixtures`] holds the scene/database builders and `Read + Write` test doubles shared
//! across them.
//!
//! [`mtls`] (real mutual-TLS handshakes over real loopback sockets) is a larger suite in
//! its own file.

mod batch;
#[cfg(all(feature = "beam", beam_tests))]
mod beam_tiles;
mod cancellation;
mod estimator;
pub mod fixtures;
mod handshake;
mod hdr_assets;
mod lazy_library;
mod lifecycle;
#[cfg(feature = "adaptive-sampling")]
mod live_pass;
#[cfg(all(feature = "worker", not(feature = "adaptive-sampling")))]
mod live_pass_off;
mod render_roundtrip;
mod streaming;
mod tilt_curves;
mod v14;
#[cfg(feature = "worker")]
mod zoning;

// Imported here for `mtls`, which reaches them through `super::{..}`.
use fixtures::{final_only, read_stream_until_done, test_db, tiny_scene};

// Each test builds a real throwaway private CA and issues real certificates via
// `crate::pki`, then drives a real TLS handshake over a real loopback `TcpStream` --
// nothing is mocked at the `rustls` layer, to catch what a mock would paper over (a
// missing SAN, a wrong trust anchor, an allowlist that isn't actually consulted).
mod mtls;
