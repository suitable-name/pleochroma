//! Tests of the native design file, split by topic.
//!
//! The topics are per-tier field round trips, refractive index, history and notes, imported
//! meet and drafts, `.asc` preserve and regenerate, untouched `.asc` notes, printed
//! proportions, custom materials, and the preform offset with the whole-design round trip.

mod fixtures;

mod axis_dispersion;
mod color_variant;
mod crystal_axis_plan;
mod custom_material;
mod design_file;
mod design_file_meta;
mod dispersion_model;
mod field_round_trips;
mod history_and_notes;
mod imported_meet_and_drafts;
mod pattern;
mod preform_offset_and_full_design;
mod printed_proportions_and_version;
mod refractive_index;
mod relations;
mod save_preserve_regenerate;
mod untouched_notes;
