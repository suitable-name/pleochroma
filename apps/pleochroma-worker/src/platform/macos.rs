//! macOS: keep-awake, firewall state.
//!
//! No new dependency and no FFI. The sleep blocker is `caffeinate -i -w <pid>`, a child process
//! that holds an `IOPMAssertion` of type "prevent user-idle system sleep" for as long as this
//! process lives (`-w`), so even a crash of the worker cannot leave the Mac awake. The display
//! may still sleep. `caffeinate` takes no reason text, so the reason is not shown in
//! `pmset -g assertions`. The firewall state comes from `socketfilterfw`, read-only. macOS has
//! no public/private network profile, so that answer is always unknown.

use super::common::{
    COMMAND_TIMEOUT, ChildGuard, FirewallState, NetworkProfile, macos_block_all,
    parse_macos_app_blocked, parse_macos_global_state, run_bounded, spawn_resident,
};

const SOCKETFILTERFW: &str = "/usr/libexec/ApplicationFirewall/socketfilterfw";

/// Keeps the Mac from sleeping while it is held; the display may still turn off. Dropping it
/// ends the `caffeinate` child.
#[derive(Debug)]
pub struct KeepAwake {
    _child: ChildGuard,
}

impl KeepAwake {
    /// Starts `caffeinate -i -w <this pid>`. `None` if it cannot run.
    #[must_use]
    pub fn acquire(_reason: &str) -> Option<Self> {
        let pid = std::process::id().to_string();
        spawn_resident("/usr/bin/caffeinate", &["-i", "-w", &pid])
            .map(|child| Self { _child: child })
    }
}

/// The application firewall.
///
/// Off is open; "block all" is blocked; otherwise this program's own entry decides
/// (`--getappblocked`), and a program with no entry is unknown (a signed program is let in
/// automatically, an unsigned one asks the user on first use). Ports are not consulted: the macOS
/// firewall works per program.
#[must_use]
pub fn firewall_state(_ports: &[u16]) -> FirewallState {
    let Some(global) = run_bounded(SOCKETFILTERFW, &["--getglobalstate"], COMMAND_TIMEOUT) else {
        return FirewallState::Unknown;
    };
    // The text decides; a failing status is only noted, since it is not known to mean the text
    // is unusable.
    if !global.success {
        tracing::debug!("socketfilterfw --getglobalstate ended with a failing status");
    }
    match parse_macos_global_state(&global.stdout) {
        Some(false) => return FirewallState::Open,
        None => return FirewallState::Unknown,
        Some(true) => {}
    }
    if macos_block_all(&global.stdout) {
        return FirewallState::Blocked;
    }
    let Ok(exe) = std::env::current_exe() else {
        return FirewallState::Unknown;
    };
    let exe = exe.to_string_lossy().into_owned();
    match run_bounded(SOCKETFILTERFW, &["--getappblocked", &exe], COMMAND_TIMEOUT)
        .and_then(|out| parse_macos_app_blocked(&out.stdout))
    {
        Some(true) => FirewallState::Blocked,
        Some(false) => FirewallState::Open,
        None => FirewallState::Unknown,
    }
}

/// macOS has no network profile idea: always unknown.
#[must_use]
pub const fn network_profile() -> NetworkProfile {
    NetworkProfile::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_awake_acquire_and_drop() {
        let held = KeepAwake::acquire("test");
        drop(held);
    }

    #[test]
    fn network_profile_is_unknown() {
        assert_eq!(network_profile(), NetworkProfile::Unknown);
    }
}
