//! Background batch computation of the traced RI range of every design in "Mine".
//!
//! The range is `pleochroma_editor::ri_range::compute` over the design's resolved stone, stored
//! in the vault's `ri_ranges` table (`pleochroma_vault::model::ri_range::StoredRiRange`). It fills
//! the Material Reference window's list "Your designs that work here"
//! (`gui::material_reference`). The editor's worker stores the range of the open design the same
//! way, so the batch is only for designs that were never opened since.
//!
//! The batch follows `gui::batch::tilt`: a confirm step, progress with Cancel, a summary, a
//! `RenderContext::export_active_count` pause of the live viewport, and one `catch_unwind` per
//! design so a bad design counts as failed. It differs in three ways:
//!
//! - It computes locally only. A range is about 350 poses of one design, so there is no remote
//!   request for it.
//! - Resume and staleness are one rule. Every claimed design is resolved and its geometry hashed;
//!   a stored row of the current `THRESHOLD_VERSION` and the same hash is skipped, any other row
//!   (an older version, an edited design) is traced again. A cancelled run therefore continues
//!   where it stopped, with no list kept between runs.
//! - A design cancelled mid-trace stores nothing and counts as neither done nor failed.
//!
//! Split into [`engine`] (per-design resolve, trace and store, and the lanes) and [`wiring`] (the
//! `RiBatchModel` callbacks).

mod engine;
mod wiring;

pub(in crate::gui) use wiring::setup;
