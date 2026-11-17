//! The Devices page.
//!
//! Fills `DevicesState` from the coordinator status once a second and answers its callbacks
//! (Rename, Remove access, Copy fingerprint) through the
//! [`CoordinatorBackend`](crate::control::backend::CoordinatorBackend).
//!
//! The page lists the viewer, monitor and administration entries of the status; render
//! computers (the worker role) belong to the Render computers page. Times are relative
//! ("Connected for 12 min", "Last seen 3 h ago"), so no time zone is needed.

use crate::{
    control::{
        backend::{CoordinatorBackend, CoordinatorStatus, DeviceStatus, PeerRole},
        status::unix_now,
    },
    gui::app::{ActiveBackend, AppWindow, DeviceRowData, DevicesState},
};
use pleochroma_net::tls::fingerprint_from_hex;
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::{cell::RefCell, rc::Rc, time::Duration};

/// How often the page re-reads the status.
const REFRESH: Duration = Duration::from_secs(1);

thread_local! {
    /// The refresh timer; a `Timer` stops when dropped, so it lives for the UI thread.
    static REFRESH_TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
}

/// Connects the Devices page of `window` to `backend`: shows the current devices at once,
/// refreshes them every second, and wires Rename, Remove access and Copy fingerprint.
pub fn wire(window: &AppWindow, backend: &ActiveBackend) {
    let state = window.global::<DevicesState>();
    let model = Rc::new(VecModel::<DeviceRowData>::default());
    state.set_devices(ModelRc::from(Rc::clone(&model)));

    state.on_rename({
        let weak = window.as_weak();
        let backend = backend.clone();
        move |fingerprint, name| {
            let name = name.trim().to_owned();
            match parse_fingerprint(&fingerprint) {
                Ok(fp) => run_action(&weak, &backend, move |b| b.rename(&fp, &name)),
                Err(reason) => show_result(&weak, &backend, Err(reason)),
            }
        }
    });
    state.on_remove({
        let weak = window.as_weak();
        let backend = backend.clone();
        move |fingerprint| match parse_fingerprint(&fingerprint) {
            Ok(fp) => run_action(&weak, &backend, move |b| b.revoke(&fp)),
            Err(reason) => show_result(&weak, &backend, Err(reason)),
        }
    });
    state.on_copy_fingerprint({
        let weak = window.as_weak();
        move |fingerprint| {
            if let Some(window) = weak.upgrade() {
                let error = match copy_to_clipboard(&fingerprint) {
                    Ok(()) => "",
                    Err(_) => "The fingerprint could not be copied.",
                };
                window.global::<DevicesState>().set_error(error.into());
            }
        }
    });

    refresh(window, backend, &model);
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, REFRESH, {
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                refresh(&window, &backend, &model);
            }
        }
    });
    REFRESH_TIMER.with(|slot| *slot.borrow_mut() = Some(timer));
}

/// Runs `action` on the coordinator that is shown, on a helper thread (a remote coordinator
/// can take seconds to answer), and shows the outcome on the UI thread.
fn run_action<F>(weak: &slint::Weak<AppWindow>, backend: &ActiveBackend, action: F)
where
    F: FnOnce(&dyn CoordinatorBackend) -> Result<(), String> + Send + 'static,
{
    let shown = backend.get();
    let active = backend.clone();
    let for_thread = weak.clone();
    let spawned = std::thread::Builder::new()
        .name("devices-command".to_owned())
        .spawn(move || {
            let result = action(shown.as_ref());
            let _ = for_thread.upgrade_in_event_loop(move |window| {
                finish_action(&window, &active, result);
            });
        });
    if let Err(e) = spawned {
        let reason = format!("Could not start the work: {e}");
        show_result(weak, backend, Err(reason));
    }
}

/// Shows an outcome that is already known, on the UI thread.
fn show_result(weak: &slint::Weak<AppWindow>, backend: &ActiveBackend, result: Result<(), String>) {
    if let Some(window) = weak.upgrade() {
        finish_action(&window, backend, result);
    }
}

/// Shows the outcome of Rename or Remove access: an error in plain words, or none, and the
/// new list at once.
fn finish_action(window: &AppWindow, backend: &ActiveBackend, result: Result<(), String>) {
    let state = window.global::<DevicesState>();
    state.set_error(SharedString::from(result.err().unwrap_or_default()));
    // The rows model is the one `wire` put into the page.
    let rows = state.get_devices();
    if let Some(model) = rows.as_any().downcast_ref::<VecModel<DeviceRowData>>() {
        refresh(window, backend, model);
    }
}

/// Parses the 64-character hex fingerprint a row carries.
fn parse_fingerprint(hex: &str) -> Result<pleochroma_net::tls::Fingerprint, String> {
    fingerprint_from_hex(hex).ok_or_else(|| "That device's fingerprint is not valid.".to_string())
}

/// Reads the status and permissions and updates the page state and rows.
fn refresh(window: &AppWindow, backend: &ActiveBackend, model: &VecModel<DeviceRowData>) {
    let backend = backend.get();
    let status = backend.status();
    let caps = backend.capabilities();
    let now = unix_now();
    let rows = device_rows(&status, now);
    let online = rows.iter().filter(|row| row.online).count();

    let state = window.global::<DevicesState>();
    state.set_heading(heading(rows.len()).into());
    state.set_online_text(if rows.is_empty() {
        SharedString::new()
    } else {
        format!("{online} online now").into()
    });
    state.set_accept_any_certificate(accepts_any_certificate(&status));
    state.set_can_rename(caps.rename);
    state.set_can_remove(caps.revoke);
    state.set_can_pair(caps.issue_tokens);
    state.set_denied_reason(if caps.rename && caps.revoke {
        SharedString::new()
    } else {
        caps.denied_reason().into()
    });
    sync_rows(model, rows);
}

/// Brings `model` to `rows` without resetting it when only row contents changed, so an open
/// "..." menu or dialog survives the refresh.
fn sync_rows(model: &VecModel<DeviceRowData>, rows: Vec<DeviceRowData>) {
    let same_devices = model.row_count() == rows.len()
        && rows.iter().enumerate().all(|(i, row)| {
            model
                .row_data(i)
                .is_some_and(|old| old.fingerprint == row.fingerprint)
        });
    if same_devices {
        for (i, row) in rows.into_iter().enumerate() {
            if model.row_data(i).as_ref() != Some(&row) {
                model.set_row_data(i, row);
            }
        }
    } else {
        model.set_vec(rows);
    }
}

/// The rows of the Devices page: every non-worker entry of the status, in status order.
fn device_rows(status: &CoordinatorStatus, now: u64) -> Vec<DeviceRowData> {
    status
        .devices
        .iter()
        .filter(|device| device.role != PeerRole::Worker)
        .map(|device| row_for(device, now))
        .collect()
}

/// One table row for `device`.
fn row_for(device: &DeviceStatus, now: u64) -> DeviceRowData {
    let now_text = if !device.activity.is_empty() {
        device.activity.clone()
    } else if device.online {
        "Idle".to_string()
    } else {
        "\u{2014}".to_string()
    };
    DeviceRowData {
        name: device.name.clone().into(),
        kind: kind_line(device).into(),
        icon: icon_for(device),
        online: device.online,
        since: since_text(device, now).into(),
        now: now_text.into(),
        addr: device.address.clone().unwrap_or_default().into(),
        fingerprint: device.fingerprint.clone().into(),
        fingerprint_short: device.fingerprint_short.clone().into(),
    }
}

/// "3 devices can connect", "1 device can connect" or "No devices yet".
fn heading(count: usize) -> String {
    match count {
        0 => "No devices yet".to_string(),
        1 => "1 device can connect".to_string(),
        n => format!("{n} devices can connect"),
    }
}

/// Whether the status says every client certificate is accepted (the allowlist is skipped).
/// The status carries this as a warning text; the page matches on its words.
fn accepts_any_certificate(status: &CoordinatorStatus) -> bool {
    status.warnings.iter().any(|warning| {
        let lower = warning.to_lowercase();
        lower.contains("any client certificate") || lower.contains("any certificate")
    })
}

/// The second line under a name: the platform, with the remote-administration role added.
fn kind_line(device: &DeviceStatus) -> String {
    let role = match device.role {
        PeerRole::Viewer | PeerRole::Worker => "",
        PeerRole::Monitor => "Remote monitor",
        PeerRole::Admin => "Remote administrator",
    };
    match (device.platform.is_empty(), role.is_empty()) {
        (true, true) => "Viewer".to_string(),
        (true, false) => role.to_string(),
        (false, true) => device.platform.clone(),
        (false, false) => format!("{} \u{b7} {role}", device.platform),
    }
}

/// 0 phone or tablet, 1 computer, 2 remote administration.
fn icon_for(device: &DeviceStatus) -> i32 {
    if matches!(device.role, PeerRole::Monitor | PeerRole::Admin) {
        return 2;
    }
    let platform = device.platform.to_lowercase();
    let mobile = ["phone", "tablet", "android", "iphone", "ipad", "ios"];
    i32::from(!mobile.iter().any(|word| platform.contains(word)))
}

/// "Connected for 12 min" while online, otherwise when it was last seen.
fn since_text(device: &DeviceStatus, now: u64) -> String {
    if device.online {
        return device.connected_since.map_or_else(
            || "Connected".to_string(),
            |since| {
                let seconds = now.saturating_sub(since);
                if seconds < 60 {
                    "Connected just now".to_string()
                } else {
                    format!("Connected for {}", duration_text(seconds))
                }
            },
        );
    }
    let Some(seen) = device.last_seen else {
        return "Never connected".to_string();
    };
    let seconds = now.saturating_sub(seen);
    match seconds {
        0..=59 => "Last seen just now".to_string(),
        60..=3599 => format!("Last seen {} min ago", seconds / 60),
        3600..=86_399 => format!("Last seen {} h ago", seconds / 3600),
        86_400..=172_799 => "Last seen yesterday".to_string(),
        _ => format!("Last seen {} days ago", seconds / 86_400),
    }
}

/// A span in plain words: "12 min", "2 h 5 min" or "3 days".
fn duration_text(seconds: u64) -> String {
    let minutes = seconds / 60;
    if minutes < 60 {
        format!("{minutes} min")
    } else if minutes < 24 * 60 {
        let (hours, rest) = (minutes / 60, minutes % 60);
        if rest == 0 {
            format!("{hours} h")
        } else {
            format!("{hours} h {rest} min")
        }
    } else {
        let days = minutes / (24 * 60);
        format!("{days} {}", if days == 1 { "day" } else { "days" })
    }
}

/// The platform's clipboard tool (the same ones the desktop app uses): the text is piped
/// to its standard input. A fingerprint is public, so the clipboard is fine for it.
#[cfg(target_os = "windows")]
const CLIPBOARD_TOOLS: &[(&str, &[&str])] = &[("clip", &[])];
/// See the Windows definition above.
#[cfg(target_os = "linux")]
const CLIPBOARD_TOOLS: &[(&str, &[&str])] =
    &[("xclip", &["-selection", "clipboard"]), ("wl-copy", &[])];
/// See the Windows definition above.
#[cfg(target_os = "macos")]
const CLIPBOARD_TOOLS: &[(&str, &[&str])] = &[("pbcopy", &[])];
/// Platforms without a known clipboard tool: copying reports failure.
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
const CLIPBOARD_TOOLS: &[(&str, &[&str])] = &[];

/// Copies `text` to the clipboard with the first tool that works.
///
/// # Errors
///
/// The last tool's error, or `Unsupported` when the platform has none.
fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    use std::{
        io::Write as _,
        process::{Command, Stdio},
    };

    let mut last = std::io::Error::from(std::io::ErrorKind::Unsupported);
    for (program, args) in CLIPBOARD_TOOLS {
        let mut command = Command::new(program);
        command.args(*args);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt as _;
            // CREATE_NO_WINDOW: no console flash from the console program `clip`.
            command.creation_flags(0x0800_0000);
        }
        let spawned = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(e) => {
                last = e;
                continue;
            }
        };
        // Dropping stdin closes the pipe; the tool exits at end of input.
        if let Some(mut stdin) = child.stdin.take()
            && let Err(e) = stdin.write_all(text.as_bytes())
        {
            drop(stdin);
            last = e;
            let _ = child.wait();
            continue;
        }
        match child.wait() {
            Ok(status) if status.success() => return Ok(()),
            Ok(_) => last = std::io::Error::other("the clipboard tool failed"),
            Err(e) => last = e,
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(role: PeerRole, platform: &str) -> DeviceStatus {
        let mut device = DeviceStatus::new("Test", "ab".repeat(32), role);
        device.platform = platform.to_string();
        device
    }

    #[test]
    fn heading_counts_devices() {
        assert_eq!(heading(0), "No devices yet");
        assert_eq!(heading(1), "1 device can connect");
        assert_eq!(heading(3), "3 devices can connect");
    }

    #[test]
    fn workers_stay_off_the_devices_page() {
        let mut status = CoordinatorStatus::minimal(
            "Studio PC",
            "0.12.0",
            45,
            crate::control::backend::CoordinatorState::Running,
        );
        status
            .devices
            .push(device(PeerRole::Viewer, "Phone \u{b7} Android"));
        status
            .devices
            .push(device(PeerRole::Worker, "Worker \u{b7} Windows"));
        status.devices.push(device(PeerRole::Admin, ""));
        let rows = device_rows(&status, 1_000);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].kind.as_str(), "Remote administrator");
        assert_eq!(rows[1].icon, 2);
    }

    #[test]
    fn kind_and_icon_follow_platform_and_role() {
        assert_eq!(kind_line(&device(PeerRole::Viewer, "")), "Viewer");
        assert_eq!(
            icon_for(&device(PeerRole::Viewer, "Phone \u{b7} Android")),
            0
        );
        assert_eq!(
            icon_for(&device(PeerRole::Viewer, "Desktop app \u{b7} Windows")),
            1
        );
        assert_eq!(
            kind_line(&device(PeerRole::Monitor, "Desktop app")),
            "Desktop app \u{b7} Remote monitor"
        );
    }

    #[test]
    fn since_text_is_relative() {
        let mut online = device(PeerRole::Viewer, "");
        online.online = true;
        online.connected_since = Some(1_000);
        assert_eq!(since_text(&online, 1_030), "Connected just now");
        assert_eq!(since_text(&online, 1_000 + 12 * 60), "Connected for 12 min");
        assert_eq!(
            since_text(&online, 1_000 + 2 * 3600 + 5 * 60),
            "Connected for 2 h 5 min"
        );

        let mut offline = device(PeerRole::Viewer, "");
        assert_eq!(since_text(&offline, 5_000), "Never connected");
        offline.last_seen = Some(1_000);
        assert_eq!(since_text(&offline, 1_000 + 5 * 60), "Last seen 5 min ago");
        assert_eq!(since_text(&offline, 1_000 + 3 * 3600), "Last seen 3 h ago");
        assert_eq!(
            since_text(&offline, 1_000 + 30 * 3600),
            "Last seen yesterday"
        );
        assert_eq!(
            since_text(&offline, 1_000 + 3 * 86_400),
            "Last seen 3 days ago"
        );
    }

    #[test]
    fn the_accept_any_warning_is_recognized() {
        let mut status = CoordinatorStatus::minimal(
            "Studio PC",
            "0.12.0",
            45,
            crate::control::backend::CoordinatorState::Running,
        );
        assert!(!accepts_any_certificate(&status));
        status
            .warnings
            .push("Any client certificate is accepted".to_string());
        assert!(accepts_any_certificate(&status));
    }

    #[test]
    fn a_bad_fingerprint_is_an_error_not_a_panic() {
        assert!(parse_fingerprint("abcd").is_err());
        assert!(parse_fingerprint(&"ab".repeat(32)).is_ok());
    }
}
