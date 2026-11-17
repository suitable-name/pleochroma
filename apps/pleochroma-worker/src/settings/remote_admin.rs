//! The remote-administration switch and what goes with it.
//!
//! Whether monitors and administrators on other computers may look after this coordinator is
//! one setting, [`CoordinatorSettings::allow_remote_admin`](super::CoordinatorSettings) (off by
//! default; `serve --allow-remote-admin` is the same switch on the command line). It is a
//! **never-remote** setting: only this computer's window changes it, through [`set_enabled`].
//! The settings patch a remote administrator sends has no field for it
//! (`pleochroma_net::admin::status::SettingsPatch`), and the permission table refuses the
//! requests that could reach monitor and administrator codes
//! (`pleochroma_net::admin::permissions`).
//!
//! The switch is read when the coordinator starts: it takes effect at the next (re)start, and
//! the status reports what the running coordinator does
//! (`CoordinatorStatus::remote_admin_enabled`). [`needs_restart`] says when the two differ.
//! Switching it **off** must not wait for that: live sessions stay open and codes can still
//! be claimed until the coordinator restarts. The Settings page therefore restarts the
//! coordinator at once after `set_enabled(false)`, through its own restart path while the
//! coordinator is starting or running. The pages
//! use these helpers: [`enabled`], [`set_enabled`], [`needs_restart`], [`remote_devices`],
//! [`can_make_codes`], [`allowlists`] and the two notices.

use super::{current, update};
use crate::pki::role::{admin_allowlist_in, monitor_allowlist_in};
use pleochroma_net::{
    admin::status::{CoordinatorStatus, DeviceStatus},
    messages::PeerRole,
};
use std::path::{Path, PathBuf};

/// What the Settings card says while the switch is off.
pub const OFF_NOTICE: &str = "Remote administration is off. Monitors and administrators on \
     other computers cannot connect, and no codes for them can be made.";

/// What the Settings card says about the saved switch not being in force yet.
pub const RESTART_NOTICE: &str = "The change takes effect when the coordinator restarts.";

/// Whether the saved settings allow remote administration.
#[must_use]
pub fn enabled() -> bool {
    current().allow_remote_admin
}

/// Switches remote administration on or off in the saved settings. Local only: nothing a
/// remote administrator can send reaches this.
///
/// # Errors
///
/// A message in plain words when the settings cannot be saved.
pub fn set_enabled(on: bool) -> Result<(), String> {
    update(|settings| settings.allow_remote_admin = on)
}

/// Whether the saved switch differs from what the running coordinator does (`running` is
/// `CoordinatorStatus::remote_admin_enabled`).
///
/// The Settings card then offers "Restart now".
#[must_use]
pub const fn needs_restart(running: bool, saved: bool) -> bool {
    running != saved
}

/// The two allowlists remote administration uses in the certificate folder `pki_dir`:
/// monitors first, then administrators.
#[must_use]
pub fn allowlists(pki_dir: &Path) -> [(PeerRole, PathBuf); 2] {
    [
        (PeerRole::Monitor, monitor_allowlist_in(pki_dir)),
        (PeerRole::Admin, admin_allowlist_in(pki_dir)),
    ]
}

/// The monitor and administrator devices of `status`, for the Settings card's list (each has
/// "Remove"): administrators first, then monitors, each group in the status's order.
#[must_use]
pub fn remote_devices(status: &CoordinatorStatus) -> Vec<&DeviceStatus> {
    [PeerRole::Admin, PeerRole::Monitor]
        .into_iter()
        .flat_map(|role| {
            status
                .devices
                .iter()
                .filter(move |device| device.role == role)
        })
        .collect()
}

/// Whether the coordinator in `status` can make monitor and administrator codes now: remote
/// administration is on in the running coordinator.
#[must_use]
pub const fn can_make_codes(status: &CoordinatorStatus) -> bool {
    status.remote_admin_enabled
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_net::admin::status::CoordinatorState;

    fn status_with(devices: &[(&str, PeerRole)]) -> CoordinatorStatus {
        let mut status =
            CoordinatorStatus::minimal("Studio", "0.12.0", 45, CoordinatorState::Running);
        for (index, (name, role)) in devices.iter().enumerate() {
            status
                .devices
                .push(DeviceStatus::new(*name, format!("{index:064x}"), *role));
        }
        status
    }

    #[test]
    fn the_card_lists_administrators_then_monitors_and_nobody_else() {
        let status = status_with(&[
            ("Phone", PeerRole::Viewer),
            ("Wall", PeerRole::Monitor),
            ("Ops", PeerRole::Admin),
            ("Garage", PeerRole::Worker),
            ("Desk", PeerRole::Admin),
        ]);
        let names: Vec<&str> = remote_devices(&status)
            .iter()
            .map(|device| device.name.as_str())
            .collect();
        assert_eq!(names, ["Ops", "Desk", "Wall"]);
        assert!(remote_devices(&status_with(&[])).is_empty());
    }

    #[test]
    fn a_saved_switch_that_differs_from_the_running_one_needs_a_restart() {
        assert!(!needs_restart(false, false));
        assert!(!needs_restart(true, true));
        assert!(needs_restart(false, true));
        assert!(needs_restart(true, false));
    }

    #[test]
    fn codes_follow_the_running_coordinator_not_the_saved_switch() {
        let mut status = status_with(&[]);
        assert!(!can_make_codes(&status));
        status.remote_admin_enabled = true;
        assert!(can_make_codes(&status));
    }

    #[test]
    fn the_allowlists_sit_next_to_the_certificate_authority() {
        let dir = Path::new("/data/pki");
        let [(first_role, first), (second_role, second)] = allowlists(dir);
        assert_eq!(
            (first_role, second_role),
            (PeerRole::Monitor, PeerRole::Admin)
        );
        assert_eq!(first, dir.join(crate::pki::MONITOR_ALLOWLIST_FILE));
        assert_eq!(second, dir.join(crate::pki::ADMIN_ALLOWLIST_FILE));
    }

    #[test]
    fn the_switch_is_off_until_it_is_set() {
        // The settings store may hold anything another test saved; the default is off.
        assert!(!crate::settings::CoordinatorSettings::default().allow_remote_admin);
        assert!(!OFF_NOTICE.is_empty() && !RESTART_NOTICE.is_empty());
    }
}
