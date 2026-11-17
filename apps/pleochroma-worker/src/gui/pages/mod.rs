//! The coordinator window's pages and dialogs. Each module exposes
//! `wire(window: &AppWindow, backend: &ActiveBackend)`, called once by `gui::app`.

pub mod devices;
pub mod jobs;
pub mod library;
pub mod log;
pub mod overview;
pub mod pair;
pub mod picker;
pub mod render;
pub mod settings;
pub mod settings_remote_admin;
pub mod tools;
pub mod wizard;
pub mod worker_mode;
