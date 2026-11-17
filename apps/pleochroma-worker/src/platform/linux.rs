//! Linux: keep-awake, firewall state.
//!
//! The workspace has no D-Bus crate, so the logind inhibitor is taken the way `systemd-inhibit`
//! takes it: a child process holding a `sleep` lock (`org.freedesktop.login1.Manager.Inhibit`,
//! mode "block"). The child runs a short shell loop that ends when this process is gone
//! (`kill -0 <pid>`) or when `systemd-inhibit` is, so a crash of the worker cannot leave the
//! lock behind for more than five seconds. Without `systemd-inhibit` or logind,
//! [`KeepAwake::acquire`] returns `None`. The firewall state is read from `ufw` or `firewalld`
//! where present, otherwise unknown. There is no network profile on Linux: that answer is
//! always unknown.

use super::common::{
    COMMAND_TIMEOUT, ChildGuard, FirewallState, NetworkProfile, parse_firewalld, parse_ufw_status,
    run_bounded, spawn_resident,
};

/// Keeps the system from sleeping while it is held (the screen may still turn off). Dropping it
/// ends the inhibitor.
#[derive(Debug)]
pub struct KeepAwake {
    _child: ChildGuard,
}

impl KeepAwake {
    /// Takes a logind `sleep` inhibitor with the reason shown by `systemd-inhibit --list`.
    /// `None` when `systemd-inhibit` is missing or logind refuses.
    #[must_use]
    pub fn acquire(reason: &str) -> Option<Self> {
        let why = format!("--why={reason}");
        let pid = std::process::id().to_string();
        spawn_resident(
            "systemd-inhibit",
            &[
                "--what=sleep",
                "--who=Pleochroma",
                &why,
                "--mode=block",
                "sh",
                "-c",
                // Ends when the worker is gone or when the process that started this shell
                // (`systemd-inhibit`, killed by the guard's drop) is, so no loop is left behind.
                "p=$PPID; while kill -0 \"$1\" 2>/dev/null && kill -0 \"$p\" 2>/dev/null; do sleep 5; done",
                "sh",
                &pid,
            ],
        )
        .map(|child| Self { _child: child })
    }
}

/// `ufw` (needs root to list its rules, so usually unknown for a normal user), then `firewalld`
/// (`firewall-cmd --state` and `--list-ports` work without root); neither means unknown.
///
/// Only TCP port rules are recognised; other tools (nftables, iptables) are not read.
#[must_use]
pub fn firewall_state(ports: &[u16]) -> FirewallState {
    // A command that failed (`ufw` without root, `firewall-cmd` with firewalld stopped) is not
    // read: both print what the parsers use only with a success status.
    if let Some(out) = run_bounded("ufw", &["status"], COMMAND_TIMEOUT).filter(|out| out.success) {
        let state = parse_ufw_status(&out.stdout, ports);
        if state != FirewallState::Unknown {
            return state;
        }
    }
    let state =
        run_bounded("firewall-cmd", &["--state"], COMMAND_TIMEOUT).filter(|out| out.success);
    let listed =
        run_bounded("firewall-cmd", &["--list-ports"], COMMAND_TIMEOUT).filter(|out| out.success);
    if let (Some(state), Some(listed)) = (state, listed) {
        parse_firewalld(&state.stdout, &listed.stdout, ports)
    } else {
        FirewallState::Unknown
    }
}

/// Linux has no network profile idea: always unknown.
#[must_use]
pub const fn network_profile() -> NetworkProfile {
    NetworkProfile::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_awake_acquire_and_drop() {
        // `None` is fine on a machine without logind; it must not panic either way.
        let held = KeepAwake::acquire("test");
        drop(held);
    }

    #[test]
    fn network_profile_is_unknown() {
        assert_eq!(network_profile(), NetworkProfile::Unknown);
    }
}
