//! The live-viewport render, lighting and material modules.
//!
//! Camera and lighting wiring ([`camera_lighting`]), the Fit distance of the pose pills
//! ([`fit_view`]), saved lighting presets ([`lighting_presets`]), the lighting a design
//! remembers ([`design_lighting`]), material selection ([`material`]) and render-quality
//! controls ([`quality`]), the toolbar's quick color change ([`render_body_color`]), the tab and
//! view-mode logic deciding whether the tracer runs ([`render_visibility`]), the target-samples
//! slider's exponent-to-count mapping ([`sample_scale`]), and the high-resolution export flow
//! ([`render_export`], split into the `queue` and `wiring` submodules).

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
