//! The coordinator's settings (`coordinator.toml`): the model, the process-wide store, the
//! encrypted backup, and the log (the ring the Log page shows and the daily files).

/// Encrypted backup and restore of the certificates and settings.
pub mod backup;
/// The log ring, the daily log files and the secret scrubber.
pub mod log;
/// `CoordinatorSettings` and its file format.
pub mod model;
/// The remote-administration switch and its helpers.
pub mod remote_admin;
/// The coordinators this computer manages remotely (`remotes.toml` and their credentials).
#[cfg(feature = "gui")]
pub mod remotes;
mod store;

pub use model::{CoordinatorSettings, Listen, LoadOutcome, RenderChoice, RunMode, computer_name};
pub use store::{
    StorageOverrides, apply_patch, current, data_dir, default_data_dir, init, init_read_only,
    last_change_summary, reload, set_change_hook, set_data_dir, settings_file, storage_overrides,
    update,
};
