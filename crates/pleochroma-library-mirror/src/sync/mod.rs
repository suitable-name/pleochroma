//! The mirror algorithm itself: enumerating the remote catalogue (`catalogue`) and the
//! additive/update-only per-design sync loop (`pass`/`design`). See the crate's own doc comment
//! for the full design.

mod catalogue;
mod design;
mod pass;
#[cfg(test)]
mod tests;

pub use pass::run_mirror_sync;
