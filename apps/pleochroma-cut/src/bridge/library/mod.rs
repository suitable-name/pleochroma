//! The design-library protocol's client side: a one-shot request plus a held session
//! ([`client`]), the local-vs-remote source abstraction the viewer's library UI reads
//! through ([`source`]), and the pull-mirror sync that copies a remote worker's
//! library into the local database ([`mirror`]). `mirror` and `source` both build on
//! `client`'s connection handling rather than duplicating it.

pub mod client;
pub mod mirror;
pub mod source;

/// The shared mirror crate's conversion of a design's wire frosting into the stored form, at the
/// path Cut's library UI has always used.
pub use pleochroma_library_mirror::frosting_from_wire;
