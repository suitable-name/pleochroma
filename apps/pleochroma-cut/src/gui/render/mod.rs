//! The live-viewport rendering/lighting/material stack: camera and lighting callback
//! wiring ([`camera_lighting`]) and its pose pills' Fit distance (`fit_view`), saved lighting presets ([`lighting_presets`]), the
//! lighting a design remembers for itself ([`design_lighting`]),
//! material selection ([`material`]) and render-quality controls ([`quality`]), the toolbar's
//! quick colour change ([`render_body_color`]), the
//! tab/view-mode logic deciding whether the tracer runs ([`render_visibility`]), the
//! target-samples slider's exponent<->count mapping ([`sample_scale`]), and the
//! high-resolution export flow ([`render_export`]).
//!
//! Groups the render/lighting/material domain into one module tree, with
//! `render_export.rs` (the largest, well over this codebase's ~700-line split
//! threshold) further split into its own [`render_export`] submodule folder.

pub mod advanced_in_use;
pub mod camera_handles;
pub mod camera_lighting;
pub mod crystal_axis_ui;
pub mod design_lighting;
pub mod edge_effects;
pub mod faceting_machine_ui;
pub mod fit_view;
pub mod inclusion_effects;
pub mod lighting_presets;
pub mod material;
pub mod output_defaults;
pub mod parameter_constants;
pub mod physics_effects_ui;
pub mod quality;
pub mod ray_settings;
pub mod render_body_color;
pub mod render_export;
pub mod render_visibility;
pub mod reset_tab;
pub mod sample_scale;
pub mod stone_effects;
pub mod structure_effects;
