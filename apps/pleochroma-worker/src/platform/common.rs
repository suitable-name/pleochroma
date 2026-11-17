//! Shared types and helpers of the platform layer.
//!
//! The firewall and network-profile answers, the bounded command runner, the plain-words warning
//! texts, and the pure parsers for the output of the commands the three operating systems are
//! asked.
//!
//! Everything here is read-only. The parsers are compiled on every platform so their tests run
//! everywhere; the operating-system files call only the ones they need.

use std::{
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// How long any helper program may run before it is killed and its answer becomes "unknown".
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

/// Whether the operating system's firewall lets other devices reach this program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirewallState {
    /// An allow rule applies, or the firewall is off.
    Open,
    /// A block rule applies, or there is no allow rule.
    Blocked,
    /// It could not be read (no rights, timeout, tool missing, unsupported system).
    Unknown,
}

/// How the operating system classes the network this computer is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkProfile {
    /// A private, home, work or domain network: other devices may connect.
    Private,
    /// A public network: the system hides this computer from other devices.
    Public,
    /// It could not be read, or the system has no such idea.
    Unknown,
}

/// The finished output of a helper program.
#[derive(Debug)]
pub(crate) struct CmdOutput {
    /// The exit status was success. A caller trusts `stdout` only when this is set.
    pub success: bool,
    /// Standard output, lossily decoded.
    pub stdout: String,
}

/// Runs a program with no window and no input, and waits at most `timeout`. A program that
/// cannot start or runs too long yields `None` (a timed-out program is killed).
pub(crate) fn run_bounded(program: &str, args: &[&str], timeout: Duration) -> Option<CmdOutput> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashes up from the windowed coordinator.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().ok()?;
    let mut pipe = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                // The reader thread is left to finish on its own: a grandchild may still hold
                // the pipe open.
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let bytes = reader.join().ok()?;
    Some(CmdOutput {
        success: status.success(),
        stdout: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

/// A helper process that is killed and reaped when this value is dropped. The macOS and Linux
/// sleep blockers are such processes.
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug)]
pub(crate) struct ChildGuard(pub std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts a helper that must keep running, with no window and no input or output. `None` if it
/// cannot start or has already exited after a short grace period (the tool refused to work).
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn spawn_resident(program: &str, args: &[&str]) -> Option<ChildGuard> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut guard = ChildGuard(child);
    std::thread::sleep(Duration::from_millis(150));
    match guard.0.try_wait() {
        Ok(None) => Some(guard),
        _ => None,
    }
}

/// The warnings the Overview shows for a firewall and profile reading, in plain words and in the
/// wording of this operating system.
///
/// Empty when nothing is wrong or nothing is known; use [`firewall_unknown_text`] for the unknown
/// case. `ports` are the TCP ports to name where the fix is a port rule (Linux).
#[must_use]
pub fn network_warnings(
    firewall: FirewallState,
    profile: NetworkProfile,
    ports: &[u16],
) -> Vec<String> {
    let mut out = Vec::new();
    if profile == NetworkProfile::Public {
        out.push(if cfg!(windows) {
            "This network is set to Public, so Windows hides this computer from other devices. \
             Set it to Private in Settings, Network & internet."
                .to_owned()
        } else {
            "This network is set to Public, so other devices may not reach this computer. \
             Switch the connection to a private or home network."
                .to_owned()
        });
    }
    if firewall == FirewallState::Blocked {
        out.push(if cfg!(windows) {
            "Windows Firewall may block other devices. Allow pleochroma-worker on private \
             networks in Windows Security, Firewall & network protection, Allow an app through \
             firewall."
                .to_owned()
        } else if cfg!(target_os = "macos") {
            "The macOS firewall may block other devices. Allow pleochroma-worker in System \
             Settings, Network, Firewall, Options."
                .to_owned()
        } else {
            let list = ports
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            if list.is_empty() {
                "The firewall may block other devices. Allow pleochroma-worker through it."
                    .to_owned()
            } else {
                format!("The firewall may block other devices. Allow TCP ports {list} through it.")
            }
        });
    }
    out
}

/// The note for a firewall that could not be read, so the screen says so instead of staying
/// silent.
#[must_use]
pub const fn firewall_unknown_text() -> &'static str {
    "The firewall could not be read. If a phone cannot connect, check that pleochroma-worker is \
     allowed on private networks."
}

// ---------------------------------------------------------------------------------------------
// Windows probe: the output of one PowerShell script (see `windows.rs`).
// ---------------------------------------------------------------------------------------------

/// One inbound firewall rule that names this program.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WinRule {
    /// The rule allows (otherwise it blocks).
    pub allow: bool,
    /// The rule is switched on.
    pub enabled: bool,
    /// The rule is inbound.
    pub inbound: bool,
    /// Lowercase profile names: `domain`, `private`, `public`, or `any`.
    pub profiles: Vec<String>,
}

/// What the Windows script printed, split into its three kinds of lines.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct WinProbe {
    /// Lowercase names of the active network categories: `domain`, `private`, `public`.
    pub categories: Vec<String>,
    /// Firewall profiles: lowercase name and whether the profile's firewall is on.
    pub profiles: Vec<(String, bool)>,
    /// Rules naming this program.
    pub rules: Vec<WinRule>,
}

fn is_true(text: &str) -> bool {
    matches!(text.trim().to_ascii_lowercase().as_str(), "true" | "1")
}

/// A profile or category word (or its numeric flag) as a lowercase name list.
fn profile_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in text.split(',') {
        let word = part.trim().to_ascii_lowercase();
        if word.is_empty() {
            continue;
        }
        if let Ok(flags) = word.parse::<u32>() {
            // `NetSecurity.Profile` flags: Domain 1, Private 2, Public 4; 0 and "all bits" mean any.
            if flags == 0 || flags >= 7 {
                out.push("any".to_owned());
            } else {
                for (bit, name) in [(1, "domain"), (2, "private"), (4, "public")] {
                    if flags & bit != 0 {
                        out.push(name.to_owned());
                    }
                }
            }
            continue;
        }
        out.push(match word.as_str() {
            "domainauthenticated" => "domain".to_owned(),
            "all" => "any".to_owned(),
            _ => word,
        });
    }
    out
}

/// A `NetworkCategory` value: the enum name, or its number (Public 0, Private 1, Domain 2).
fn category_name(text: &str) -> Option<String> {
    let word = text.trim().to_ascii_lowercase();
    match word.as_str() {
        "public" | "0" => Some("public".to_owned()),
        "private" | "1" => Some("private".to_owned()),
        "domainauthenticated" | "domain" | "2" => Some("domain".to_owned()),
        _ => None,
    }
}

/// Parses the lines `NET|category`, `FWP|profile|enabled` and `RULE|direction|action|enabled|profile`.
/// Unknown lines (errors, banners) are skipped.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn parse_windows_probe(text: &str) -> WinProbe {
    let mut probe = WinProbe::default();
    for line in text.lines() {
        let parts: Vec<&str> = line.trim().split('|').collect();
        match parts.as_slice() {
            ["NET", category] => {
                if let Some(name) = category_name(category) {
                    probe.categories.push(name);
                }
            }
            ["FWP", name, enabled] => {
                probe
                    .profiles
                    .push((name.trim().to_ascii_lowercase(), is_true(enabled)));
            }
            ["RULE", direction, action, enabled, profile] => {
                let direction = direction.trim().to_ascii_lowercase();
                let action = action.trim().to_ascii_lowercase();
                probe.rules.push(WinRule {
                    allow: action == "allow" || action == "2",
                    enabled: is_true(enabled),
                    inbound: direction == "inbound" || direction == "1",
                    profiles: profile_names(profile),
                });
            }
            _ => {}
        }
    }
    probe
}

/// Turns a Windows probe into the two answers.
///
/// Network profile: Private if any active connection is Private or on a domain (a VPN adapter
/// left on Public does not raise a warning), Public if only Public ones are active, otherwise
/// unknown. Firewall: off for every active profile means open; an enabled inbound block rule
/// for an active profile means blocked (Windows lets a block win); an enabled inbound allow
/// rule means open; with a network but no rule at all, blocked (Windows' inbound default);
/// without any reading, unknown.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn windows_decide(probe: &WinProbe) -> (FirewallState, NetworkProfile) {
    let has = |name: &str| probe.categories.iter().any(|c| c == name);
    let profile = if has("private") || has("domain") {
        NetworkProfile::Private
    } else if has("public") {
        NetworkProfile::Public
    } else {
        NetworkProfile::Unknown
    };
    if probe.categories.is_empty() && probe.profiles.is_empty() && probe.rules.is_empty() {
        return (FirewallState::Unknown, profile);
    }
    let active = |names: &[String]| {
        probe.categories.is_empty()
            || names
                .iter()
                .any(|n| n == "any" || probe.categories.contains(n))
    };
    let active_profiles: Vec<&(String, bool)> = probe
        .profiles
        .iter()
        .filter(|(name, _)| active(std::slice::from_ref(name)))
        .collect();
    if !active_profiles.is_empty() && !active_profiles.iter().any(|(_, on)| *on) {
        return (FirewallState::Open, profile);
    }
    let applying: Vec<&WinRule> = probe
        .rules
        .iter()
        .filter(|r| r.enabled && r.inbound && active(&r.profiles))
        .collect();
    let state = if applying.iter().any(|r| !r.allow) {
        FirewallState::Blocked
    } else if applying.iter().any(|r| r.allow) {
        FirewallState::Open
    } else if probe.categories.is_empty() {
        FirewallState::Unknown
    } else {
        FirewallState::Blocked
    };
    (state, profile)
}

// ---------------------------------------------------------------------------------------------
// macOS: `socketfilterfw` output.
// ---------------------------------------------------------------------------------------------

/// The application firewall state from `socketfilterfw --getglobalstate`: `Some(false)` when it
/// is off, `Some(true)` when on, `None` when the text is not recognized. A "block all" state
/// (2) counts as on; see [`macos_block_all`].
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_macos_global_state(text: &str) -> Option<bool> {
    let lower = text.to_ascii_lowercase();
    if lower.contains("disabled") || lower.contains("state = 0") {
        Some(false)
    } else if lower.contains("enabled")
        || lower.contains("state = 1")
        || lower.contains("state = 2")
    {
        Some(true)
    } else {
        None
    }
}

/// The firewall is in "block all incoming connections" mode (`State = 2`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn macos_block_all(text: &str) -> bool {
    text.to_ascii_lowercase().contains("state = 2")
}

/// The answer of `socketfilterfw --getappblocked <path>`: `Some(true)` blocked, `Some(false)`
/// allowed, `None` when the program has no entry or the text is not recognized.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_macos_app_blocked(text: &str) -> Option<bool> {
    let lower = text.to_ascii_lowercase();
    if lower.contains("not part of the firewall") {
        None
    } else if lower.contains("is not blocked") || lower.contains("permitted") {
        Some(false)
    } else if lower.contains("blocked") {
        Some(true)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------------------------
// Linux: `ufw status` and `firewall-cmd`.
// ---------------------------------------------------------------------------------------------

/// `ufw status` text against the wanted TCP ports. Inactive is open; active is open when every
/// port has an ALLOW row, otherwise blocked; anything else (the "you need to be root" error) is
/// unknown.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_ufw_status(text: &str, ports: &[u16]) -> FirewallState {
    let lower = text.to_ascii_lowercase();
    if lower.contains("status: inactive") {
        return FirewallState::Open;
    }
    if !lower.contains("status: active") {
        return FirewallState::Unknown;
    }
    let allowed = |port: u16| {
        let plain = port.to_string();
        let tcp = format!("{port}/tcp");
        lower.lines().any(|line| {
            line.split_whitespace()
                .next()
                .is_some_and(|first| first == plain || first == tcp)
                && line.contains("allow")
        })
    };
    if ports.iter().all(|&p| allowed(p)) {
        FirewallState::Open
    } else {
        FirewallState::Blocked
    }
}

/// `firewall-cmd --state` and `--list-ports` against the wanted TCP ports. Not running is
/// unknown (some other firewall may be in charge); running is open when every port is listed,
/// otherwise blocked. A service that opens the port without listing it reads as blocked.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_firewalld(state: &str, list_ports: &str, ports: &[u16]) -> FirewallState {
    if state.trim() != "running" {
        return FirewallState::Unknown;
    }
    let listed = |port: u16| {
        let tcp = format!("{port}/tcp");
        list_ports.split_whitespace().any(|t| t == tcp)
    };
    if ports.iter().all(|&p| listed(p)) {
        FirewallState::Open
    } else {
        FirewallState::Blocked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN_PRIVATE_ALLOWED: &str = "NET|Private\r\n\
        FWP|Domain|True\r\nFWP|Private|True\r\nFWP|Public|True\r\n\
        RULE|Inbound|Allow|True|Private, Public\r\n";

    #[test]
    fn windows_private_with_allow_rule_is_open() {
        let probe = parse_windows_probe(WIN_PRIVATE_ALLOWED);
        assert_eq!(probe.categories, ["private"]);
        assert_eq!(probe.rules.len(), 1);
        assert_eq!(
            windows_decide(&probe),
            (FirewallState::Open, NetworkProfile::Private)
        );
    }

    #[test]
    fn windows_public_with_rule_only_for_private_is_blocked() {
        let text = "NET|Public\nFWP|Private|True\nFWP|Public|True\n\
                    RULE|Inbound|Allow|True|Private\n";
        assert_eq!(
            windows_decide(&parse_windows_probe(text)),
            (FirewallState::Blocked, NetworkProfile::Public)
        );
    }

    #[test]
    fn windows_block_rule_beats_allow_rule() {
        let text = "NET|Private\nRULE|Inbound|Allow|True|Any\nRULE|Inbound|Block|True|Private\n";
        assert_eq!(
            windows_decide(&parse_windows_probe(text)).0,
            FirewallState::Blocked
        );
    }

    #[test]
    fn windows_disabled_rule_and_outbound_rule_do_not_count() {
        let text = "NET|Private\nRULE|Inbound|Allow|False|Any\nRULE|Outbound|Allow|True|Any\n";
        assert_eq!(
            windows_decide(&parse_windows_probe(text)).0,
            FirewallState::Blocked
        );
    }

    #[test]
    fn windows_firewall_off_is_open() {
        let text = "NET|Public\nFWP|Domain|False\nFWP|Private|False\nFWP|Public|False\n";
        assert_eq!(
            windows_decide(&parse_windows_probe(text)),
            (FirewallState::Open, NetworkProfile::Public)
        );
    }

    #[test]
    fn windows_domain_counts_as_private_and_vpn_public_does_not_warn() {
        let text = "NET|DomainAuthenticated\nNET|Public\nRULE|Inbound|Allow|True|Domain\n";
        assert_eq!(
            windows_decide(&parse_windows_probe(text)),
            (FirewallState::Open, NetworkProfile::Private)
        );
    }

    #[test]
    fn windows_numeric_values_and_noise_lines() {
        let text = "WARNING: something\nNET|1\nFWP|Private|True\nRULE|1|2|1|0\nERROR|x\n";
        let probe = parse_windows_probe(text);
        assert_eq!(probe.categories, ["private"]);
        assert_eq!(probe.rules[0].profiles, ["any"]);
        assert_eq!(
            windows_decide(&probe),
            (FirewallState::Open, NetworkProfile::Private)
        );
    }

    #[test]
    fn windows_empty_output_is_unknown() {
        assert_eq!(
            windows_decide(&parse_windows_probe("")),
            (FirewallState::Unknown, NetworkProfile::Unknown)
        );
    }

    #[test]
    fn windows_network_without_any_rule_is_blocked() {
        assert_eq!(
            windows_decide(&parse_windows_probe("NET|Private\nFWP|Private|True\n")),
            (FirewallState::Blocked, NetworkProfile::Private)
        );
    }

    #[test]
    fn macos_global_state() {
        assert_eq!(
            parse_macos_global_state("Firewall is disabled. (State = 0)"),
            Some(false)
        );
        assert_eq!(
            parse_macos_global_state("Firewall is enabled. (State = 1)"),
            Some(true)
        );
        assert!(macos_block_all("Firewall is enabled. (State = 2)"));
        assert_eq!(parse_macos_global_state("garbage"), None);
    }

    #[test]
    fn macos_app_blocked() {
        assert_eq!(
            parse_macos_app_blocked("The application is not part of the firewall"),
            None
        );
        assert_eq!(
            parse_macos_app_blocked(
                "Application at path ( /x ) is not blocked from accepting incoming connections"
            ),
            Some(false)
        );
        assert_eq!(
            parse_macos_app_blocked("Incoming connections to this application are blocked"),
            Some(true)
        );
    }

    #[test]
    fn ufw_rows() {
        let active = "Status: active\n\nTo                         Action      From\n\
                      --                         ------      ----\n\
                      7878/tcp                   ALLOW       Anywhere\n\
                      7879                       ALLOW       Anywhere\n\
                      7878/tcp (v6)              ALLOW       Anywhere (v6)\n";
        assert_eq!(parse_ufw_status(active, &[7878, 7879]), FirewallState::Open);
        assert_eq!(
            parse_ufw_status(active, &[7878, 7880]),
            FirewallState::Blocked
        );
        assert_eq!(
            parse_ufw_status("Status: inactive\n", &[7878]),
            FirewallState::Open
        );
        assert_eq!(
            parse_ufw_status("ERROR: You need to be root to run this script", &[7878]),
            FirewallState::Unknown
        );
    }

    #[test]
    fn firewalld_ports() {
        assert_eq!(
            parse_firewalld("running\n", "7878/tcp 7879/tcp\n", &[7878, 7879]),
            FirewallState::Open
        );
        assert_eq!(
            parse_firewalld("running\n", "22/tcp\n", &[7878]),
            FirewallState::Blocked
        );
        assert_eq!(
            parse_firewalld("not running\n", "", &[7878]),
            FirewallState::Unknown
        );
    }

    #[test]
    fn warnings_are_plain_and_quiet_when_fine() {
        assert!(network_warnings(FirewallState::Open, NetworkProfile::Private, &[7878]).is_empty());
        assert!(
            network_warnings(FirewallState::Unknown, NetworkProfile::Unknown, &[7878]).is_empty()
        );
        let both = network_warnings(
            FirewallState::Blocked,
            NetworkProfile::Public,
            &[7878, 7879],
        );
        assert_eq!(both.len(), 2);
        assert!(both.iter().all(|w| !w.is_empty()));
    }

    #[test]
    fn run_bounded_missing_program_is_none() {
        assert!(
            run_bounded(
                "pleochroma-no-such-program-xyz",
                &[],
                Duration::from_millis(200)
            )
            .is_none()
        );
    }
}
