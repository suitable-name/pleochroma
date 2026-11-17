//! Pull-mirror sync: copies a remote `pleochroma-worker`'s design library into the
//! local `pleochroma_vault` database, so it is available offline.
//!
//! The algorithm and its design notes (the additive/update-only safety rule, "local delete
//! wins", identity and staleness, pagination, attachments, cancellation) live in the shared
//! `pleochroma_library_mirror` crate, which the Android app uses too. This module is Cut's
//! adapter: [`options`] re-exports the shared types and implements the transport trait for
//! Cut's connection types, and [`sync`] holds [`spawn_mirror_sync`], the worker-thread wrapper
//! the UI calls.

pub mod options;
pub mod sync;

pub use options::{MirrorHandle, MirrorOptions, MirrorOutcome, MirrorProgress};
pub use sync::spawn_mirror_sync;
