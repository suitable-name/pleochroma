//! Unit tests for the spectral transport loop, grouped by topic in the submodules below.

/// O<->e mode re-coupling at internal reflections.
mod mode_coupling_tests;

mod exit_splitting_tests;

mod exit_collection_tests;

mod nee_tests;

/// The scene exposure scales an HDR-lit trace linearly.
mod hdr_exposure_tests;

/// The yaw of an HDR map: column shifts, a turned stone and camera, the unturned source.
mod hdr_rotation_tests;

/// The inside/outside state of the bounce loop on a concave stone.
mod concave_state_tests;

mod surface_glare_tests;

mod attribution_tests;

mod surface_plumbing_tests;

mod wave_normal_tests;

/// The Tray Reflection: the floor bounce, the estimator rule for exit splitting over the tray.
mod tray_mirror_tests;
