//! Rough colour and colour zoning, application side.
//!
//! * [`store`]: THE storage API. Every function the Rough colour UI calls to save, load and
//!   delete a plan's rough colour, its cached photos, its pose choices, a material's zones and a
//!   render job's zones lives there, with typed arguments and one `anyhow::Result` each. The
//!   vault tables underneath are opaque text and bytes
//!   (`pleochroma_vault::db::sqlite::Database::save_rough_colour` and friends); the formats are
//!   `pleochroma_cut_core::rough_plan::zoned_plan`'s.
//!
//! * [`preview`], [`adopt_link`]: what the Rough Planner shows and does with a plan's colour (the
//!   coloured previews, the "Stone Orientation" option, "Use Color");
//! * [`handles`], [`mesh_paint`], [`zone_link`]: the 3D zone handles and the polished-window brush
//!   in the planner's rough view, and the wizard's side of them;
//! * [`material_zones`]: the "Zones..." list and the base-colour write-back of the material
//!   editor.
//!
//! # Flows
//!
//! **Planner.** A plan with a rough colour ([`store::load_rough_colour`]) previews each stone
//! with [`preview::stone_colours`]: the host material with the rough's zones moved into the
//! stone's frame, read for its face-up colour. The pose of a stone is the planner's own unless
//! the cutter chose "Best Color":[`store::choose_and_save_poses`] scores the box-symmetric
//! poses and stores the non-canonical choices; [`store::posed_stone_pose`] reads one back. The
//! saved plan's `PlacedStone` is not changed.
//!
//! **Adopt.** [`store::adopt_stone`] creates the custom material "<plan> - <design> color".
//! Its row holds the BASE zone, so a build that ignores zones still renders it. Its
//! `material_zoning` row holds the zones in the stone frame, in mm. The result is the
//! render-ready material plus the real stone width the editor should be set to; the stone is
//! rendered at that size, and no slider is involved. The planner's "Use Color" and "Open in
//! Editor" both hand the result to the design it was computed for
//! (`gui::editor::callbacks::tier_actions::planned_stone`), loading it first.
//!
//! **Material resolution.** [`store::attach_zoning`] / [`store::with_stored_zoning`] put the
//! stored zones back on a material resolved from a custom row. They are called wherever the
//! application builds its list of custom materials (startup and the material editor's save), so
//! every `resolve_material` afterwards returns a zoned material.
//!
//! **Render jobs.** A job freezes its scene as the wire's `SceneState`, which carries no zones.
//! The zones therefore travel in the job file itself (`RenderJobFile::zoning`). Older queued
//! rows keep theirs in a side table; [`store::complete_job_zoning`] moves them into the job when
//! it runs or is exported. Tilt-curve requests carry no zones either: a zoned material's curves
//! are always computed locally (`gui::batch::tilt::engine`).

pub mod adopt_link;
pub mod handles;
pub mod material_zones;
pub mod mesh_paint;
pub mod preview;
pub mod store;
#[cfg(test)]
mod store_tests;
pub mod wizard;
pub mod zone_link;
