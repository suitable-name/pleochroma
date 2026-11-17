//! Windows: keep-awake, firewall and network-profile state.
//!
//! No new dependency: the sleep request is one `kernel32` call (the C runtime already links it)
//! and the read-only firewall and profile state comes from one bounded PowerShell run
//! (`Get-NetConnectionProfile`, `Get-NetFirewallProfile`, `Get-NetFirewallApplicationFilter`).
//! Nothing here changes a setting; PowerShell and its `NetSecurity` module need no administrator
//! for these reads, and any failure or timeout reads as "unknown".

use std::{
    sync::{Mutex, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use super::common::{
    COMMAND_TIMEOUT, FirewallState, NetworkProfile, parse_windows_probe, run_bounded,
    windows_decide,
};

const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
const ES_CONTINUOUS: u32 = 0x8000_0000;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetThreadExecutionState(flags: u32) -> u32;
}

/// Keeps Windows from sleeping while it is held; the display may still turn off. Dropping it
/// lets the system sleep again.
///
/// `SetThreadExecutionState` is per thread and `ES_CONTINUOUS` lasts until that thread clears it
/// or ends, so a dedicated thread holds the request and ends on drop.
#[derive(Debug)]
pub struct KeepAwake {
    release: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl KeepAwake {
    /// Asks Windows not to sleep. `None` when the request is refused or the thread cannot start.
    /// The reason is not shown anywhere on Windows; it is kept for the other systems' sake.
    #[must_use]
    pub fn acquire(_reason: &str) -> Option<Self> {
        let (release, wait) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<bool>();
        let thread = std::thread::Builder::new()
            .name("keep-awake".to_owned())
            .spawn(move || {
                // SAFETY: a plain flags-in, flags-out call with no pointers.
                let previous =
                    unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
                let _ = ready_tx.send(previous != 0);
                if previous != 0 {
                    // Blocks until the sender is dropped or used.
                    let _ = wait.recv();
                    // SAFETY: as above; clears the request.
                    unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
                }
            })
            .ok()?;
        if ready_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or(false)
        {
            Some(Self {
                release: Some(release),
                thread: Some(thread),
            })
        } else {
            drop(release);
            let _ = thread.join();
            None
        }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        drop(self.release.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// How long one reading stays valid, so asking for the firewall and the profile back to back
/// runs PowerShell once.
const CACHE_LIFETIME: Duration = Duration::from_secs(10);

static CACHE: Mutex<Option<(Instant, FirewallState, NetworkProfile)>> = Mutex::new(None);

/// The PowerShell script: three kinds of output lines, parsed by
/// [`parse_windows_probe`]. `exe` is this program's path.
fn script(exe: &str) -> String {
    let quoted = exe.replace('\'', "''");
    format!(
        "$ErrorActionPreference='SilentlyContinue'; $exe='{quoted}'; \
         Get-NetConnectionProfile | ForEach-Object {{ 'NET|' + $_.NetworkCategory }}; \
         Get-NetFirewallProfile | ForEach-Object {{ 'FWP|' + $_.Name + '|' + $_.Enabled }}; \
         Get-NetFirewallApplicationFilter -Program $exe | Get-NetFirewallRule | \
         ForEach-Object {{ 'RULE|' + $_.Direction + '|' + $_.Action + '|' + $_.Enabled + '|' + $_.Profile }}"
    )
}

fn probe() -> (FirewallState, NetworkProfile) {
    if let Ok(cache) = CACHE.lock()
        && let Some((at, firewall, profile)) = *cache
        && at.elapsed() < CACHE_LIFETIME
    {
        return (firewall, profile);
    }
    let answer = std::env::current_exe()
        .ok()
        .and_then(|path| {
            let text = path.to_string_lossy().into_owned();
            let text = text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned();
            run_bounded(
                "powershell.exe",
                &[
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    &script(&text),
                ],
                COMMAND_TIMEOUT,
            )
        })
        .map_or((FirewallState::Unknown, NetworkProfile::Unknown), |out| {
            windows_decide(&parse_windows_probe(&out.stdout))
        });
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((Instant::now(), answer.0, answer.1));
    }
    answer
}

/// Whether the Windows firewall lets other devices reach this program: an enabled inbound allow
/// rule for it on the active network profile means open.
///
/// The ports are not consulted (Windows rules here are per program). May take up to 3 s; call it
/// off the interface thread.
#[must_use]
pub fn firewall_state(_ports: &[u16]) -> FirewallState {
    probe().0
}

/// The active network's profile; a Public network hides this computer from other devices. May
/// take up to 3 s; call it off the interface thread.
#[must_use]
pub fn network_profile() -> NetworkProfile {
    probe().1
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
    fn script_escapes_single_quotes() {
        assert!(script("C:\\a'b\\x.exe").contains("C:\\a''b\\x.exe"));
    }
}
