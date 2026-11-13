//! Coordinator tests.
//!
//! [`registry`] covers check-out, notifications and the `WELCOME` advertisement on loopback socket
//! pairs; [`e2e`] covers a real coordinator on ephemeral loopback ports with TLS, `join`,
//! enrollment tokens and liveness. [`jobs`], [`pictures`], [`small_pictures`], [`hdr`] and [`late`]
//! run requests through it; `live_pass` (`adaptive-sampling` builds) covers adaptive live passes.

#[cfg(all(feature = "beam", beam_tests))]
mod beam_tiles;
mod e2e;
mod fixtures;
mod hdr;
mod jobs;
mod late;
#[cfg(feature = "adaptive-sampling")]
mod live_pass;
mod pictures;
mod registry;
mod small_pictures;
mod support;
