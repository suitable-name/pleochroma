//! Cut's side of the mirror-sync types: the shared crate's options, progress, outcome, handle
//! and transport trait, re-exported at their original paths, plus the pieces that need
//! Cut's own settings and connection types.
//!
//! The types themselves live in `pleochroma_library_mirror::options`; see that crate's doc
//! comment for the design. What stays here is [`mirror_source_id`] (a worker's address as its
//! `source_id`) and the [`LibraryTransport`] impls for Cut's [`WorkerSettings`] (a new
//! connection per call, which the interactive remote browse uses) and [`LibrarySession`] (one
//! connection held for a whole sync, which reconnects once per request if it drops).

use crate::{
    bridge::library::client::{self as library_client, LibraryClientError, LibrarySession},
    settings::WorkerSettings,
};
pub use pleochroma_library_mirror::{
    LibraryTransport, MirrorHandle, MirrorOptions, MirrorOutcome, MirrorProgress,
};
use pleochroma_net::library::{LibraryRequest, LibraryResponse};

/// The `source_id` a mirror sync attributes every design it saves/updates to -- distinct
/// per configured worker (its address).
#[must_use]
pub fn mirror_source_id(worker: &WorkerSettings) -> String {
    pleochroma_library_mirror::source_id(&worker.address)
}

impl LibraryTransport for WorkerSettings {
    type Error = LibraryClientError;

    fn request(&self, req: &LibraryRequest) -> Result<LibraryResponse, LibraryClientError> {
        library_client::request(self, req)
    }
}

impl LibraryTransport for LibrarySession {
    type Error = LibraryClientError;

    fn request(&self, req: &LibraryRequest) -> Result<LibraryResponse, LibraryClientError> {
        // Resolves to `LibrarySession`'s own inherent `request` (inherent methods take
        // priority over a trait method of the same name) -- see that type's doc comment.
        self.request(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_source_id_is_distinct_per_worker_address() {
        let a = WorkerSettings {
            address: "10.0.0.5:9443".to_string(),
            ..WorkerSettings::default()
        };
        let b = WorkerSettings {
            address: "10.0.0.6:9443".to_string(),
            ..WorkerSettings::default()
        };
        assert_ne!(mirror_source_id(&a), mirror_source_id(&b));
    }
}
