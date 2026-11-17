//! Operating-system services for the coordinator: keeping the computer awake, and the
//! read-only firewall and network-profile state the Overview warns about.

pub mod autostart;
pub mod instance;
#[cfg(feature = "gui")]
pub mod tray;

pub mod common;
pub use common::{FirewallState, NetworkProfile, firewall_unknown_text, network_warnings};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{KeepAwake, firewall_state, network_profile};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{KeepAwake, firewall_state, network_profile};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{KeepAwake, firewall_state, network_profile};
