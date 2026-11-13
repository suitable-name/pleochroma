//! The render worker's request-resolution state, re-exported from `pleochroma_solid::preview` at
//! the paths this module's worker and tests use.
//!
//! `resolve_request_state` turns one `RedrawRequest` into the planes, camera and style that
//! `super::render` draws.

pub use pleochroma_solid::preview::WorkerMemory;
#[cfg(test)]
pub use pleochroma_solid::preview::{escaping_tier_label, resolve_request_state};
