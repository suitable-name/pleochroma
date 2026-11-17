//! The Settings page's remote administration.
//!
//! This computer's own side (the switch, the codes and the list of computers with access) and the
//! settings of a remote coordinator while one is shown.
//!
//! **This computer (Security section).** "Allow other computers to monitor or manage this
//! coordinator" is local only and off by default; it changes the setting through
//! [`settings::update`] and raises the page's restart strip (the running coordinator reads it at
//! start). While the coordinator accepts remote administration, "Create code" makes a monitor
//! or administrator code with the local backend's `issue_token` and shows it in a dialog in the
//! style of the pairing dialog: the link with its code masked, a countdown, and a copy button
//! that keeps the link out of the clipboard history and clears it when the code runs out. The
//! list shows the Monitor and Admin entries of the status; "Remove access" is `revoke`.
//!
//! **A remote coordinator.** While `ShellState.remote` is on, the page shows what the remote's
//! status tells (name, pairing, discovery) and changes it through the backend's
//! `apply_settings`, greyed out by the signed-in access level. Network, ports and backups stay on
//! the remote computer and are not offered.
//!
//! A pairing link or code is never logged and never shown in full; the link is held in a wiping
//! string.

use crate::{
    control::{
        backend::{
            CoordinatorBackend, CoordinatorState, CoordinatorStatus, IssuedToken, PeerRole,
            SettingsPatch, TokenId,
        },
        status::unix_now,
    },
    gui::{
        app::{AccessRowData, ActiveBackend, AppWindow, SettingsState, ShellState},
        switcher::access_label,
        util::{clock_text, copy_secret, mask_link},
    },
    settings,
};
use pleochroma_net::tls::fingerprint_from_hex;
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use zeroize::Zeroizing;

/// How often the page refreshes its lists and the countdown while it is shown.
const TICK: Duration = Duration::from_secs(1);

/// The page number of Settings in `ShellState.page`.
const SETTINGS_PAGE: i32 = 5;

/// The code that is on screen in the code dialog.
struct LiveCode {
    /// Names the code for cancelling.
    id: TokenId,
    /// The coordinator that made it, so it is cancelled there even after a switch.
    backend: Arc<dyn CoordinatorBackend>,
    /// The pairing link, a secret; wiped when dropped.
    link: Zeroizing<String>,
    /// When it stops working.
    expires: Instant,
    /// True when this window put the link on the clipboard.
    copied: bool,
}

thread_local! {
    /// The code dialog's code (UI thread only).
    static LIVE: RefCell<Option<LiveCode>> = const { RefCell::new(None) };
    /// The clipboard, opened on first use.
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
    /// The rows of the list of computers with access.
    static ROWS: Rc<VecModel<AccessRowData>> = Rc::new(VecModel::default());
    /// The refresh timer; a `Timer` stops when dropped, so it lives for the UI thread.
    static TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
}

/// Connects the remote administration cards of the Settings page of `window`.
pub fn wire(window: &AppWindow, backend: &ActiveBackend) {
    let state = window.global::<SettingsState>();
    state.set_access_rows(ModelRc::from(ROWS.with(Rc::clone)));

    state.on_set_remote_admin({
        let weak = window.as_weak();
        let backend = backend.clone();
        move |value| {
            if let Some(window) = weak.upgrade() {
                set_remote_admin(&window, &backend, value);
            }
        }
    });
    state.on_create_code({
        let weak = window.as_weak();
        let backend = backend.clone();
        move |level, name| {
            if let Some(window) = weak.upgrade() {
                create_code(&window, &backend, level, name.as_str());
            }
        }
    });
    state.on_copy_code({
        let weak = window.as_weak();
        move || {
            if let Some(window) = weak.upgrade() {
                copy_code(&window);
            }
        }
    });
    state.on_close_code({
        let weak = window.as_weak();
        move || {
            if let Some(window) = weak.upgrade() {
                close_code(&window);
            }
        }
    });
    state.on_remove_access({
        let weak = window.as_weak();
        let backend = backend.clone();
        move |fingerprint| {
            if let Some(window) = weak.upgrade() {
                remove_access(&window, &backend, fingerprint.as_str());
            }
        }
    });
    state.on_remote_set_bool({
        let weak = window.as_weak();
        let backend = backend.clone();
        move |key, value| {
            if let Some(window) = weak.upgrade() {
                let mut patch = SettingsPatch::default();
                match key.as_str() {
                    "allow_pairing" => patch.allow_pairing = Some(value),
                    "discovery" => patch.discovery = Some(value),
                    _ => return,
                }
                apply_remote(&window, &backend, patch);
            }
        }
    });
    state.on_remote_set_name({
        let weak = window.as_weak();
        let backend = backend.clone();
        move |name| {
            if let Some(window) = weak.upgrade() {
                let name = name.trim();
                let state = window.global::<SettingsState>();
                if name.is_empty() || name == state.get_remote_display_name().as_str() {
                    return;
                }
                let patch = SettingsPatch {
                    display_name: Some(name.to_owned()),
                    ..SettingsPatch::default()
                };
                apply_remote(&window, &backend, patch);
            }
        }
    });

    let timer = Timer::default();
    timer.start(TimerMode::Repeated, TICK, {
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                tick(&window, &backend);
            }
        }
    });
    TIMER.with(|slot| *slot.borrow_mut() = Some(timer));
    refresh(window, backend);
}

/// Once a second: the code's countdown, and the lists while the page is shown.
fn tick(window: &AppWindow, backend: &ActiveBackend) {
    countdown(window);
    if window.global::<ShellState>().get_page() == SETTINGS_PAGE {
        refresh(window, backend);
    }
}

/// Fills the remote administration properties from the settings and the active coordinator.
pub fn refresh(window: &AppWindow, backend: &ActiveBackend) {
    let state = window.global::<SettingsState>();
    let shown = backend.get();
    let status = shown.status();
    if window.global::<ShellState>().get_remote() {
        let caps = shown.capabilities();
        state.set_remote_name(status.name.as_str().into());
        state.set_remote_display_name(status.name.as_str().into());
        state.set_remote_allow_pairing(status.allow_pairing);
        state.set_remote_discovery(status.discovery);
        state.set_remote_can_change(caps.apply_settings);
        state.set_remote_can_restart(caps.restart);
        state.set_remote_note(
            if caps.apply_settings {
                ""
            } else {
                caps.denied_reason()
            }
            .into(),
        );
        return;
    }
    state.set_remote_admin(settings::current().allow_remote_admin);
    state.set_remote_admin_active(status.remote_admin_enabled);
    ROWS.with(|model| sync_rows(model, access_rows(&status, unix_now())));
}

/// The Monitor and Admin entries of the status as rows.
fn access_rows(status: &CoordinatorStatus, now: u64) -> Vec<AccessRowData> {
    status
        .devices
        .iter()
        .filter(|d| matches!(d.role, PeerRole::Monitor | PeerRole::Admin))
        .map(|d| AccessRowData {
            name: d.name.as_str().into(),
            access: access_text(d.role).into(),
            seen: if d.online {
                "Now".to_owned()
            } else {
                d.last_seen.map_or_else(
                    || "Never".to_owned(),
                    |t| super::settings::ago(now.saturating_sub(t)),
                )
            }
            .into(),
            online: d.online,
            fingerprint: d.fingerprint.as_str().into(),
        })
        .collect()
}

/// "Full administration" for an Admin entry, "Monitor only" for a Monitor.
fn access_text(role: PeerRole) -> &'static str {
    if role == PeerRole::Admin {
        access_label(crate::control::backend::AccessRole::Admin)
    } else {
        access_label(crate::control::backend::AccessRole::Monitor)
    }
}

/// Brings `model` to `rows` without resetting it when only row contents changed.
fn sync_rows(model: &VecModel<AccessRowData>, rows: Vec<AccessRowData>) {
    if model.row_count() == rows.len() {
        for (i, row) in rows.into_iter().enumerate() {
            if model.row_data(i).as_ref() != Some(&row) {
                model.set_row_data(i, row);
            }
        }
    } else {
        model.set_vec(rows);
    }
}

// ---------------------------------------------------------------------------------------------
// The switch and the list
// ---------------------------------------------------------------------------------------------

/// The switch: the setting changes at once, the coordinator reads it after a restart.
fn set_remote_admin(window: &AppWindow, backend: &ActiveBackend, value: bool) {
    let state = window.global::<SettingsState>();
    if window.global::<ShellState>().get_remote() {
        // Never remote: only this computer's own coordinator can switch it.
        return;
    }
    let before = settings::remote_admin::enabled();
    match settings::remote_admin::set_enabled(value) {
        Ok(()) => {
            state.set_error(SharedString::new());
            if before != value {
                state.set_restart_needed(true);
                // Switching it off must not leave remote administrators connected until the
                // owner restarts by hand: the coordinator restarts at once. (Switching it on
                // waits for the owner's "Restart now".)
                if !value
                    && matches!(
                        backend.get().status().state,
                        CoordinatorState::Starting | CoordinatorState::Running
                    )
                {
                    super::settings::restart_shown(window, backend);
                }
            }
        }
        Err(reason) => state.set_error(reason.into()),
    }
    refresh(window, backend);
}

/// "Remove access" on a row of the list.
fn remove_access(window: &AppWindow, backend: &ActiveBackend, hex: &str) {
    let state = window.global::<SettingsState>();
    let Some(fp) = fingerprint_from_hex(hex) else {
        state.set_error("This computer's certificate could not be read.".into());
        return;
    };
    let backend = backend.get();
    let weak = window.as_weak();
    let spawned = std::thread::Builder::new()
        .name("settings-remove-access".to_owned())
        .spawn(move || {
            let result = backend.revoke(&fp);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak.upgrade() {
                    let state = window.global::<SettingsState>();
                    match result {
                        Ok(()) => state.set_error(SharedString::new()),
                        Err(reason) => state.set_error(reason.into()),
                    }
                }
            });
        });
    if let Err(e) = spawned {
        state.set_error(format!("Could not start the work: {e}").into());
    }
}

// ---------------------------------------------------------------------------------------------
// Codes
// ---------------------------------------------------------------------------------------------

/// "Create code": asks the local coordinator for a monitor or administrator code.
fn create_code(window: &AppWindow, backend: &ActiveBackend, level: i32, typed: &str) {
    let state = window.global::<SettingsState>();
    if window.global::<ShellState>().get_remote() || state.get_code_busy() {
        return;
    }
    let role = if level == 1 {
        PeerRole::Admin
    } else {
        PeerRole::Monitor
    };
    let shown = backend.get();
    let name = if typed.trim().is_empty() {
        default_name(&shown.status())
    } else {
        typed.trim().to_owned()
    };
    state.set_code_busy(true);
    state.set_code_error(SharedString::new());
    let weak = window.as_weak();
    let spawned = std::thread::Builder::new()
        .name("settings-create-code".to_owned())
        .spawn(move || {
            let result = shown.issue_token(role, &name);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak.upgrade() {
                    show_code(&window, shown, role, &name, result);
                }
            });
        });
    if let Err(e) = spawned {
        state.set_code_busy(false);
        state.set_code_error(format!("Could not start the work: {e}").into());
    }
}

/// "Computer 3": the name a code gets when none is typed.
fn default_name(status: &CoordinatorStatus) -> String {
    let count = status
        .devices
        .iter()
        .filter(|d| matches!(d.role, PeerRole::Monitor | PeerRole::Admin))
        .count();
    format!("Computer {}", count + 1)
}

/// Shows the code that was made, or why none was.
fn show_code(
    window: &AppWindow,
    backend: Arc<dyn CoordinatorBackend>,
    role: PeerRole,
    name: &str,
    result: Result<IssuedToken, String>,
) {
    let state = window.global::<SettingsState>();
    state.set_code_busy(false);
    let issued = match result {
        Ok(issued) => issued,
        Err(reason) => {
            state.set_code_error(reason.into());
            return;
        }
    };
    let IssuedToken {
        id,
        link,
        expires_at,
        ..
    } = issued;
    let remaining = expires_at
        .duration_since(SystemTime::now())
        .unwrap_or(Duration::ZERO);
    release_live();
    state.set_code_for(name.into());
    state.set_code_access(access_text(role).into());
    state.set_code_link_shown(mask_link(&link).into());
    state.set_code_remaining(clock_text(remaining).into());
    state.set_code_copied(false);
    state.set_code_expired(false);
    LIVE.with(|slot| {
        *slot.borrow_mut() = Some(LiveCode {
            id,
            backend,
            link: Zeroizing::new(link),
            expires: Instant::now() + remaining,
            copied: false,
        });
    });
    state.set_dialog(4);
}

/// Updates the countdown of the code dialog and ends the code when it runs out.
fn countdown(window: &AppWindow) {
    let left = LIVE.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|live| live.expires.saturating_duration_since(Instant::now()))
    });
    let Some(left) = left else {
        return;
    };
    let state = window.global::<SettingsState>();
    if state.get_code_expired() {
        return;
    }
    if left.is_zero() {
        clear_clipboard();
        LIVE.with(|slot| {
            if let Some(live) = slot.borrow_mut().as_mut() {
                live.link = Zeroizing::new(String::new());
                live.copied = false;
            }
        });
        state.set_code_expired(true);
        state.set_code_copied(false);
        state.set_code_remaining("0:00".into());
        state.set_code_link_shown(SharedString::new());
    } else {
        state.set_code_remaining(clock_text(left).into());
    }
}

/// "Copy link": the link goes to the clipboard, outside its history.
fn copy_code(window: &AppWindow) {
    let state = window.global::<SettingsState>();
    let link = LIVE.with(|slot| slot.borrow().as_ref().map(|live| live.link.clone()));
    let Some(link) = link.filter(|l| !l.is_empty()) else {
        return;
    };
    let copied = CLIPBOARD.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = arboard::Clipboard::new().ok();
        }
        slot.as_mut()
            .is_some_and(|clipboard| copy_secret(clipboard, &link).is_ok())
    });
    if copied {
        LIVE.with(|slot| {
            if let Some(live) = slot.borrow_mut().as_mut() {
                live.copied = true;
            }
        });
        state.set_code_copied(true);
        state.set_code_error(SharedString::new());
    } else {
        state.set_code_error("The link could not be copied.".into());
    }
}

/// "Close" on the code dialog: the code, if unused, is cancelled and the clipboard cleared.
fn close_code(window: &AppWindow) {
    release_live();
    let state = window.global::<SettingsState>();
    state.set_dialog(0);
    state.set_code_link_shown(SharedString::new());
    state.set_code_copied(false);
    state.set_code_expired(false);
}

/// Cancels the code that is on screen, if any, and clears the clipboard if it holds its link.
fn release_live() {
    clear_clipboard();
    let live = LIVE.with(|slot| slot.borrow_mut().take());
    if let Some(live) = live {
        let LiveCode { id, backend, .. } = live;
        std::thread::spawn(move || backend.cancel_token(id));
    }
}

/// Empties the clipboard if it still holds the link this page copied.
fn clear_clipboard() {
    let link = LIVE.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|live| live.copied)
            .map(|live| live.link.clone())
    });
    let Some(link) = link else {
        return;
    };
    CLIPBOARD.with(|slot| {
        if let Some(clipboard) = slot.borrow_mut().as_mut() {
            let holds_link = clipboard
                .get_text()
                .is_ok_and(|text| text.as_str() == link.as_str());
            if holds_link {
                let _ = clipboard.clear();
            }
        }
    });
}

// ---------------------------------------------------------------------------------------------
// A remote coordinator's settings
// ---------------------------------------------------------------------------------------------

/// Applies `patch` on the coordinator that is shown, on a helper thread.
fn apply_remote(window: &AppWindow, backend: &ActiveBackend, patch: SettingsPatch) {
    let state = window.global::<SettingsState>();
    if !window.global::<ShellState>().get_remote() {
        return;
    }
    let shown = backend.get();
    if !shown.capabilities().apply_settings {
        state.set_error(shown.capabilities().denied_reason().into());
        return;
    }
    let weak = window.as_weak();
    let active = backend.clone();
    let spawned = std::thread::Builder::new()
        .name("settings-apply-remote".to_owned())
        .spawn(move || {
            let result = shown.apply_settings(&patch);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak.upgrade() {
                    let state = window.global::<SettingsState>();
                    match result {
                        Ok(()) => state.set_error(SharedString::new()),
                        Err(reason) => state.set_error(reason.into()),
                    }
                    refresh(&window, &active);
                }
            });
        });
    if let Err(e) = spawned {
        state.set_error(format!("Could not start the work: {e}").into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::backend::{CoordinatorState, DeviceStatus};

    #[test]
    fn the_masked_link_hides_the_code() {
        let link =
            "pleochroma://pair?v=1&r=admin&n=Garage&a=192.168.1.5:7878&e=7879&t=GW1-SECRETSECRET";
        let masked = mask_link(link);
        assert!(!masked.contains("SECRET"));
        assert!(masked.contains("a=192.168.1.5:7878"));
        assert_eq!(mask_link("nothing"), "pleochroma://pair?…");
    }

    #[test]
    fn the_clock_rounds_up() {
        assert_eq!(clock_text(Duration::from_secs(180)), "3:00");
        assert_eq!(clock_text(Duration::from_millis(500)), "0:01");
        assert_eq!(clock_text(Duration::ZERO), "0:00");
    }

    #[test]
    fn only_monitors_and_administrators_are_listed() {
        let mut status =
            CoordinatorStatus::minimal("Studio", "0.12.0", 45, CoordinatorState::Running);
        for (name, role) in [
            ("Phone", PeerRole::Viewer),
            ("Laptop", PeerRole::Admin),
            ("Office PC", PeerRole::Monitor),
            ("render-01", PeerRole::Worker),
        ] {
            status
                .devices
                .push(DeviceStatus::new(name, "ab".repeat(32), role));
        }
        let rows = access_rows(&status, 1000);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].access.as_str(), "Full administration");
        assert_eq!(rows[1].access.as_str(), "Monitor only");
        assert_eq!(rows[1].seen.as_str(), "Never");
        assert_eq!(default_name(&status), "Computer 3");
    }
}
