//! The transport abstraction the mirror sync and the browse helpers run over.

use pleochroma_net::library::{LibraryRequest, LibraryResponse};

/// A source of [`LibraryResponse`]s for exactly one [`LibraryRequest`] at a time.
///
/// Lets the sync run against a scripted in-memory fake (`testing::FakeTransport`) with no socket
/// or TLS handshake. The caller supplies the real one: a held connection for a whole sync that
/// reconnects once per request if it drops, or a one-shot connection per call for interactive
/// browsing. This crate opens no sockets.
pub trait LibraryTransport {
    /// What a failed request carries. Shown to the user in the sync's failure text, so it reads
    /// as a sentence fragment.
    type Error: std::fmt::Display;

    /// Sends `req` and returns the worker's reply.
    ///
    /// # Errors
    ///
    /// Whatever the underlying transport failed with: a connection, handshake or framing
    /// error, or the worker's own refusal.
    fn request(&self, req: &LibraryRequest) -> Result<LibraryResponse, Self::Error>;
}
