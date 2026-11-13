//! `pleochroma-cut`: browse, search, and render your own faceting-design library --
//! import/export `.asc`, organize by rename/delete, and the full spectral renderer.
//!
//! A library (this file) plus a thin `main.rs` binary, so that integration tests can drive
//! this crate's Slint window ([`MainWindow`]) and Rust wiring ([`gui::build_main_window`])
//! without a copy of either; see [`gui::MainWindowHandle`]'s doc comment for the public seam.

// The trait solver needs more depth to prove `Send` for the GPU export-thread closure in
// `bridge::export_thread::batch`, which goes through `wgpu`'s nested handle types. Not a
// lint: the default limit (128) is too small, 256 is enough.
#![recursion_limit = "256"]

slint::include_modules!();

// `bridge` stays crate-private: `gui::build_main_window` wires it up and hands back a
// `MainWindowHandle`, the one public seam (see its doc comment). Keeping it private also
// keeps its many `pub fn`s out of the public API, which would otherwise need `# Errors`
// and `# Panics` docs.
mod bridge;
#[cfg(feature = "render-bench")]
pub use bridge::render_thread::benchmark;
/// Slint user interface layer.
pub mod gui;
#[cfg(feature = "ui-harness")]
pub mod harness_support;

/// Whether the UI harness's seams answer in place of the real thing: file dialogs, the
/// clipboard, the OS opener, the library in the working directory and the RADV restart. Only in
/// a `ui-harness` build, and only while the harness drives it: the binary's `main` switches them
/// off (`harness_support::disarm_seams`), so such a build started as the app behaves like
/// any other.
#[must_use]
#[cfg(feature = "ui-harness")]
pub(crate) fn harness_seams_active() -> bool {
    harness_support::seams_armed()
}

/// Whether the UI harness's seams answer in place of the real thing; never, without the
/// `ui-harness` feature.
#[must_use]
#[cfg(not(feature = "ui-harness"))]
pub(crate) const fn harness_seams_active() -> bool {
    false
}
mod locate_io;
mod mesh_io;
mod plan_limit;
mod settings;
