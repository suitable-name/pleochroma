//! Three small per-frame caches keyed on the same cheap plane-hash identity.
//!
//! All three key on the same `Pod`-bytes plane-hash identity (`render_thread::hash_planes`),
//! invalidating only when the active design's geometry actually changes: which planes are
//! the girdle band ([`girdle_finish`]), the local guide-buffer prepass for denoising a
//! remote-sourced image ([`guide_pass`]), and the design's real girdle width for the
//! "Stone size" absorption-scale control ([`stone_width`]). One pattern applied three
//! times, not three unrelated caches.

pub mod girdle_finish;
pub mod guide_pass;
pub mod stone_width;
