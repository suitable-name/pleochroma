//! Color patterns for individual stones and custom materials.
//!
//! Stone sessions apply through a guarded editor callback: design overrides are undoable and
//! saved in native files, while library-preview overrides belong only to the current view.
//! Neither route changes a material preset or requires a custom material.
//!
//! * [`store`] is the vault storage API: save, load, remove and copy a material's pattern, the
//!   pattern presets, and the base-color write-back the material dialog calls. A pattern is stored
//!   beside the material's resolved zones (`material_zoning`), so the renderer keeps reading the
//!   zones it always read; the pattern is the editable form of the same colors.
//! * The window: `model.rs` (pure rules and numbers), `edit.rs` (pure edits), `undo.rs`,
//!   `presets.rs`, `colors.rs` (swatches and the solve worker), `form.rs` (the color forms by the
//!   material's rank), `variants.rs` (the color variants a zone can take), `own.rs` (the stone's
//!   own color per direction, which typical patterns are built on), `cross_section.rs`
//!   (rasters and handles), `session.rs` (the state and the thread-local host), `view.rs` (session to window),
//!   `preview.rs` (the preview area), `actions.rs` (what the callbacks do), `wiring.rs` (the window
//!   and its callbacks), `entry.rs` and `present.rs` (the material dialog's section), and
//!   `context.rs` (the render context after a store).
//! * [`follow`] is "Follows Crystal Axis": the zones of a pattern with that axis mode are turned
//!   to the stone's axis wherever the application resolves them.

mod actions;
mod colors;
mod context;
mod cross_section;
mod edit;
mod entry;
pub mod follow;
mod form;
mod model;
mod own;
mod picker;
mod present;
mod presets;
mod preview;
mod save;
mod session;
mod stone;
mod store;
#[cfg(test)]
mod store_tests;
mod typical;
mod undo;
mod variants;
mod view;
mod wiring;

pub use entry::setup_pattern_callbacks;
pub use own::own_color;
pub use present::present_section;
pub use stone::{StonePatternOpening, open_stone_pattern, seed_stone_pattern};
pub use store::*;
pub use typical::solve_pattern_colors;
pub use variants::zone_variants;
pub use wiring::{close_window, release_window_state};
#[cfg(feature = "ui-harness")]
pub use wiring::{harness_open, harness_window};

use pleochroma_editor::lch_color::LchSolve;
use session::on_host;

/// `BodyColorEditorModel.target` while the body-color editor picks a zone color for the Color
/// Pattern window (`render_color.rs` routes its open and apply to this module).
pub const TARGET_PATTERN_ZONE: i32 = 3;

/// The `[L*, C*, h]` the body-color editor opens on for the pending pick of the pattern window.
pub fn pick_seed() -> Option<[f64; 3]> {
    session::with_host(|host| actions::pick_seed(host)).flatten()
}

/// The body-color editor's "Apply Color" for target [`TARGET_PATTERN_ZONE`]: the solved color goes
/// to the zone and slot that asked for it.
pub fn picked(solve: &LchSolve) {
    on_host(|host| actions::picked_color(host, solve.lch, solve.bands.clone()));
}
