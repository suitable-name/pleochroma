//! Material and crystal-optics domain logic, underneath the render, tilt and library UI.
//!
//! Modules: crystal-axis tilt/azimuth <-> `Vec3` conversion ([`c_axis`]), crystal-system and
//! optical-character conversions plus custom-`GemMaterial` construction ([`crystal_optics`]),
//! tilt-curve SVG paths for the performance graph ([`curve_path`]), the custom material
//! library's create/edit/delete callbacks ([`custom_materials`]), the material editor's
//! physics color mode (state: [`physics_state`], solver worker: [`physics_solver`], Slint
//! glue: [`physics_ui`]) and the Dispersion section ([`dispersion_editor`]).

pub mod c_axis;
pub mod color_variant_store;
pub mod color_variants_section;
pub mod crystal_optics;
pub mod curve_path;
pub mod custom_materials;
pub mod dialog_color;
pub mod dispersion_editor;
pub mod offered_lighting;
pub mod physics_solver;
pub mod physics_state;
pub mod physics_ui;
