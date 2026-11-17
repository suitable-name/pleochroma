//! The Settings page: shows the live settings ([`crate::settings::current`]) and changes them.
//!
//! The Slint side (`ui/pages/settings*.slint`) calls `set-bool`, `set-int` or `set-text` with
//! a key; this file validates and saves through the backend's `apply_settings` (the safe fields,
//! which another computer may also change; the whole-image knobs and the jobs per device then
//! apply to a running coordinator at once) or [`settings::update`] (network, ports, certificate
//! names, "Accept any client certificate", the data folder and the per-user choices), then pushes
//! the new values back. A refused change leaves the settings alone and shows its reason.
//!
//! Keys:
//!
//! - bool: `start_at_login`, `close_to_tray`, `keep_awake`, `update_check`, `high_contrast`,
//!   `discovery`, `allow_pairing`, `accept_workers`, `accept_any_cert` (off only; on goes
//!   through the confirm dialog);
//! - int: `listen`, `max_connections`, `max_per_address`, `job_memory`, `whole_seconds`,
//!   `whole_megapixels`, `jobs_per_device`, `interactive`, `compression`, `cpu_threads`,
//!   `retention_days`, `cache_mib`;
//! - text: `name`, `bind_address`, `port`, `cert_names`, `pinned_worker`, `ov_viewer`,
//!   `ov_viewer_enroll`, `ov_worker`, `ov_worker_enroll`.
//!
//! A change that only a restart applies (ports, the network, the certificate names, storage,
//! the safe fields that `serve` reads at start) raises the page's "Restart now" strip.
//!
//! The "Can phones reach this computer?" card asks the firewall and the network profile on a
//! thread of its own (each can take up to three seconds).

use crate::{
    control::{
        backend::{CoordinatorState, CoordinatorStatus, PeerRole, SettingsPatch},
        status::unix_now,
    },
    gui::{
        app::{ActiveBackend, AppWindow, SettingsState, ShellState, Theme},
        local,
        pages::picker::{self, Kind, Request},
    },
    platform::{self, FirewallState, NetworkProfile},
    settings::{
        self,
        backup::{self, MIN_PASSWORD_CHARS},
        model::{Listen, parse_cert_names},
    },
};
use pleochroma_net::admin::status::NetworkCompression;
use slint::{ComponentHandle, SharedString};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// The stepper value that means "all render computers" for live previews.
const ALL_WORKERS_STEP: i32 = 17;

/// Pixel samples in one million (a binary million, as the setting's default is 64 of them).
const MEGA: u64 = 1_048_576;

/// What a change needs to take effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    /// It applies at once.
    Live,
    /// The coordinator must restart.
    Restart,
}

/// Connects the Settings page of `window` to the settings and the coordinator.
pub fn wire(window: &AppWindow, backend: &ActiveBackend) {
    sync_data_dir();
    refresh(window, backend);
    super::settings_remote_admin::wire(window, backend);
    wire_page(window, backend);
    wire_setters(window, backend);
    wire_folders(window, backend);
}

/// Wires what the page does when it opens, the network re-check and "Restart now".
fn wire_page(window: &AppWindow, backend: &ActiveBackend) {
    let state = window.global::<SettingsState>();
    state.on_page_opened({
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                refresh(&window, &backend);
                // The network check is about this computer; a remote coordinator has its own.
                if !window.global::<ShellState>().get_remote() {
                    start_network_check(&window, &backend);
                }
            }
        }
    });
    state.on_recheck_network({
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                start_network_check(&window, &backend);
            }
        }
    });
    state.on_restart_now({
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                restart_shown(&window, &backend);
            }
        }
    });
}

/// Wires the switches, numbers and texts of the page (`set-bool`, `set-int`, `set-text`) and
/// the two buttons that change a setting without a key.
fn wire_setters(window: &AppWindow, backend: &ActiveBackend) {
    let state = window.global::<SettingsState>();
    state.on_set_bool({
        let weak = window.as_weak();
        let backend = backend.clone();
        move |key, value| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if key.as_str() == "start_at_login" {
                // The login entry is written on a helper thread (`reg.exe` can take seconds).
                set_autostart(&window, &backend, value);
                return;
            }
            let result = changed(|| apply_bool(&window, &backend, key.as_str(), value));
            finish(&window, &backend, result);
        }
    });
    state.on_set_int({
        let weak = window.as_weak();
        let backend = backend.clone();
        move |key, value| {
            if let Some(window) = weak.upgrade() {
                let result = changed(|| apply_int(&backend, key.as_str(), value));
                finish(&window, &backend, result);
            }
        }
    });
    state.on_set_text({
        let weak = window.as_weak();
        let backend = backend.clone();
        move |key, text| {
            if let Some(window) = weak.upgrade() {
                let result = changed(|| apply_text(&backend, key.as_str(), text.as_str()));
                finish(&window, &backend, result);
            }
        }
    });
    state.on_confirm_accept_any({
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                window.global::<SettingsState>().set_dialog(0);
                let result = changed(|| apply_bool(&window, &backend, "accept_any_cert", true));
                finish(&window, &backend, result);
            }
        }
    });
    state.on_reset_data_folder({
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                let result = changed(|| {
                    settings::update(|s| s.data_folder = None)?;
                    Ok(Effect::Restart)
                });
                finish(&window, &backend, result);
            }
        }
    });
}

/// Wires the buttons that use the file system: the folder picker, "Open folder" and the backup.
fn wire_folders(window: &AppWindow, backend: &ActiveBackend) {
    let state = window.global::<SettingsState>();
    state.on_choose_existing_folder({
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            let request = Request::new(Kind::Folder, "Choose the coordinator folder");
            let active = backend.clone();
            let started = picker::pick(&weak, &request, move |window, path| {
                finish(window, &active, use_existing_folder(&path));
            });
            if let (Err(reason), Some(window)) = (started, weak.upgrade()) {
                window.global::<SettingsState>().set_error(reason.into());
            }
        }
    });
    state.on_open_data_folder(|| {
        if let Some(dir) = settings::data_dir() {
            crate::gui::util::open_folder(&dir);
        }
    });
    state.on_run_backup({
        let weak = window.as_weak();
        let backend = backend.clone();
        move |mode, password, repeat| {
            if let Some(window) = weak.upgrade() {
                run_backup(
                    &window,
                    &backend,
                    mode,
                    Zeroizing::new(password.to_string()),
                    &Zeroizing::new(repeat.to_string()),
                );
            }
        }
    });
}

/// Restarts the coordinator that is shown, on a helper thread (a remote one can take seconds to
/// answer), and shows the outcome on the Settings page: the "restart needed" strip goes away
/// when the coordinator took the restart, and the reason shows when it did not.
///
/// This page changes the settings of this computer's own coordinator, so `backend.get()` is
/// the local backend whenever its switches are shown; its `restart()` stops and starts
/// `serve` with the settings as they are now.
pub(super) fn restart_shown(window: &AppWindow, backend: &ActiveBackend) {
    let shown = backend.get();
    let weak = window.as_weak();
    let spawned = std::thread::Builder::new()
        .name("settings-restart".to_owned())
        .spawn(move || {
            let result = shown.restart();
            let _ = weak.upgrade_in_event_loop(move |window| {
                let state = window.global::<SettingsState>();
                match result {
                    Ok(()) => {
                        state.set_restart_needed(false);
                        state.set_error(SharedString::new());
                    }
                    Err(reason) => state.set_error(reason.into()),
                }
            });
        });
    if let Err(e) = spawned {
        window
            .global::<SettingsState>()
            .set_error(format!("Could not start the work: {e}").into());
    }
}

/// The "Start with Windows" switch: writes or removes the login entry on a helper thread, then
/// saves the choice and shows the result (a failure puts the switch back).
fn set_autostart(window: &AppWindow, backend: &ActiveBackend, value: bool) {
    let weak = window.as_weak();
    let active = backend.clone();
    let spawned = std::thread::Builder::new()
        .name("settings-autostart".to_owned())
        .spawn(move || {
            let result = if value {
                platform::autostart::enable()
            } else {
                platform::autostart::disable()
            };
            let _ = weak.upgrade_in_event_loop(move |window| {
                let saved = result.and_then(|()| settings::update(|s| s.start_at_login = value));
                finish(&window, &active, saved.map(|()| false));
            });
        });
    if let Err(e) = spawned {
        finish(
            window,
            backend,
            Err(format!("Could not start the work: {e}")),
        );
    }
}

/// Tells the settings store which per-user folder holds `pki/` (the backup needs it).
fn sync_data_dir() {
    if let Some(folder) = local::folder() {
        settings::set_data_dir(folder.data_dir().to_path_buf());
    }
}

/// Runs `apply` and turns "the settings differ afterwards and it needs a restart" into the
/// restart flag: a change that left the settings as they were raises nothing.
fn changed(apply: impl FnOnce() -> Result<Effect, String>) -> Result<bool, String> {
    let before = settings::current();
    let effect = apply()?;
    Ok(effect == Effect::Restart && before != settings::current())
}

/// Shows the outcome of a change: the reason it was refused, or nothing, and the new values.
fn finish(window: &AppWindow, backend: &ActiveBackend, result: Result<bool, String>) {
    let state = window.global::<SettingsState>();
    match result {
        Ok(restart) => {
            state.set_error(SharedString::new());
            if restart {
                state.set_restart_needed(true);
            }
        }
        Err(reason) => state.set_error(reason.into()),
    }
    refresh(window, backend);
}

/// A patch with the fields `fill` sets, applied through the coordinator's `apply_settings`: it
/// saves the settings and applies at once what a running coordinator can take without a restart.
fn patch(backend: &ActiveBackend, fill: impl FnOnce(&mut SettingsPatch)) -> Result<(), String> {
    let mut patch = SettingsPatch::default();
    fill(&mut patch);
    backend.get().apply_settings(&patch)
}

/// A switch changed (`start_at_login` is handled apart, on a helper thread).
fn apply_bool(
    window: &AppWindow,
    backend: &ActiveBackend,
    key: &str,
    value: bool,
) -> Result<Effect, String> {
    match key {
        "close_to_tray" => live(|s| s.close_to_tray = value),
        "keep_awake" => live(|s| s.keep_awake = value),
        "update_check" => live(|s| s.update_check = value),
        "high_contrast" => {
            settings::update(|s| s.high_contrast = value)?;
            window.global::<Theme>().set_high_contrast(value);
            Ok(Effect::Live)
        }
        "discovery" => restart_patch(backend, |p| p.discovery = Some(value)),
        "allow_pairing" => restart_patch(backend, |p| p.allow_pairing = Some(value)),
        "accept_workers" => restart_patch(backend, |p| p.accept_render_computers = Some(value)),
        "accept_any_cert" => {
            settings::update(|s| s.accept_any_client_cert = value)?;
            Ok(Effect::Restart)
        }
        _ => Err("That setting is not known.".to_owned()),
    }
}

/// A local-only change that applies at once.
fn live(f: impl FnOnce(&mut settings::CoordinatorSettings)) -> Result<Effect, String> {
    settings::update(f)?;
    Ok(Effect::Live)
}

/// A patch of a field `serve` reads at start.
fn restart_patch(
    backend: &ActiveBackend,
    fill: impl FnOnce(&mut SettingsPatch),
) -> Result<Effect, String> {
    patch(backend, fill)?;
    Ok(Effect::Restart)
}

/// The numbers a running coordinator takes at once (`apply_settings` applies them live).
const LIVE_INT_KEYS: [&str; 3] = ["whole_seconds", "whole_megapixels", "jobs_per_device"];

/// A number or a choice changed.
fn apply_int(backend: &ActiveBackend, key: &str, value: i32) -> Result<Effect, String> {
    let unsigned = u32::try_from(value).map_err(|_| "That number is not valid.".to_owned())?;
    match key {
        "listen" => {
            let suggested = first_lan_address();
            settings::update(move |s| {
                s.listen = match value {
                    1 => Listen::Network,
                    2 => Listen::Specific,
                    _ => Listen::ThisComputer,
                };
                if s.listen == Listen::Specific && s.bind_address.trim().is_empty() {
                    s.bind_address = suggested;
                }
            })?;
        }
        "max_connections" => settings::update(|s| s.max_connections = unsigned)?,
        "max_per_address" => settings::update(|s| s.max_per_address = unsigned)?,
        "job_memory" => patch(backend, |p| p.max_job_memory_mib = Some(unsigned))?,
        "whole_seconds" => patch(backend, |p| p.whole_image_seconds = Some(unsigned))?,
        "whole_megapixels" => patch(backend, |p| {
            p.whole_image_pixels = Some(u64::from(unsigned) * MEGA);
        })?,
        "jobs_per_device" => patch(backend, |p| p.jobs_per_device = Some(unsigned))?,
        "interactive" => patch(backend, |p| {
            p.interactive_workers = Some(workers_from_step(value));
        })?,
        "compression" => {
            let choice = match value {
                1 => NetworkCompression::Fast,
                2 => NetworkCompression::Off,
                _ => NetworkCompression::Default,
            };
            patch(backend, |p| p.network_compression = Some(choice))?;
        }
        "cpu_threads" => patch(backend, |p| p.cpu_threads = Some(unsigned))?,
        "retention_days" => patch(backend, |p| p.job_retention_days = Some(unsigned))?,
        "cache_mib" => patch(backend, |p| p.cache_size_mib = Some(unsigned))?,
        _ => return Err("That setting is not known.".to_owned()),
    }
    Ok(if LIVE_INT_KEYS.contains(&key) {
        Effect::Live
    } else {
        Effect::Restart
    })
}

/// A text was committed.
fn apply_text(backend: &ActiveBackend, key: &str, text: &str) -> Result<Effect, String> {
    let text = text.trim();
    match key {
        "name" => {
            let current = settings::current();
            // The field shows the computer's name while none is set; leaving it alone sets nothing.
            if current.name.is_empty() && text == current.display_name() {
                return Ok(Effect::Live);
            }
            patch(backend, |p| p.display_name = Some(text.to_owned()))?;
            Ok(Effect::Live)
        }
        "bind_address" => {
            let text = text.to_owned();
            settings::update(move |s| s.bind_address = text)?;
            Ok(Effect::Restart)
        }
        "port" => {
            let port = parse_port(text)?.ok_or("Enter a port number, such as 7878.")?;
            settings::update(|s| s.port = port)?;
            Ok(Effect::Restart)
        }
        "cert_names" => {
            let names = parse_cert_names(text)?;
            settings::update(move |s| s.extra_cert_names = names)?;
            Ok(Effect::Restart)
        }
        "pinned_worker" => restart_patch(backend, |p| p.pinned_worker = Some(text.to_owned())),
        "ov_viewer" => port_override(text, |s, p| s.viewer_port = p),
        "ov_viewer_enroll" => port_override(text, |s, p| s.viewer_enroll_port = p),
        "ov_worker" => port_override(text, |s, p| s.worker_port = p),
        "ov_worker_enroll" => port_override(text, |s, p| s.worker_enroll_port = p),
        _ => Err("That setting is not known.".to_owned()),
    }
}

/// Stores a port override (empty clears it).
fn port_override(
    text: &str,
    set: impl FnOnce(&mut settings::CoordinatorSettings, Option<u16>),
) -> Result<Effect, String> {
    let port = parse_port(text)?;
    settings::update(move |s| set(s, port))?;
    Ok(Effect::Restart)
}

/// A port as typed: empty is `None`, otherwise a number from 1 to 65535.
fn parse_port(text: &str) -> Result<Option<u16>, String> {
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<u16>()
        .ok()
        .filter(|p| *p != 0)
        .map(Some)
        .ok_or_else(|| "A port is a number from 1024 to 65535, such as 7878.".to_owned())
}

/// The stepper's value as a number of render computers (the top step is all of them).
const fn workers_from_step(step: i32) -> u32 {
    if step >= ALL_WORKERS_STEP {
        u32::MAX
    } else {
        step.unsigned_abs()
    }
}

/// The number of render computers as a stepper value.
fn step_from_workers(workers: u32) -> i32 {
    if workers >= u32::try_from(ALL_WORKERS_STEP).unwrap_or(u32::MAX) {
        ALL_WORKERS_STEP
    } else {
        i32::try_from(workers).unwrap_or(0)
    }
}

/// The address of the computer's first network adapter, or `0.0.0.0` when it has none.
fn first_lan_address() -> String {
    pleochroma_net::netinfo::lan_ipv4()
        .first()
        .map_or_else(|| "0.0.0.0".to_owned(), |a| a.addr.to_string())
}

/// "Use an existing coordinator folder": the folder must hold the certificates.
fn use_existing_folder(path: &Path) -> Result<bool, String> {
    if !path.join("pki").join(crate::pki::CA_CERT_FILE).is_file() {
        return Err(
            "This folder has no certificates (pki/ca.pem), so it is not a coordinator folder. Pick the folder that contains the pki folder."
                .to_owned(),
        );
    }
    let path = path.to_path_buf();
    changed(move || {
        settings::update(move |s| s.data_folder = Some(path))?;
        Ok(Effect::Restart)
    })
}

/// Fills every property of the page from the live settings.
fn refresh(window: &AppWindow, backend: &ActiveBackend) {
    sync_data_dir();
    let s = settings::current();
    let state = window.global::<SettingsState>();
    state.set_loaded(true);
    state.set_can_restart(matches!(
        backend.get().status().state,
        CoordinatorState::Running
    ));

    state.set_name(s.display_name().into());
    state.set_autostart_label(
        if cfg!(windows) {
            "Start with Windows (in the tray)"
        } else {
            "Start at login (in the tray)"
        }
        .into(),
    );
    // The saved choice, not the registry: reading the login entry can take seconds on Windows,
    // and the program keeps the entry in line with this choice at start.
    state.set_start_at_login(s.start_at_login);
    state.set_keep_awake(s.keep_awake);
    state.set_close_to_tray(s.close_to_tray);
    state.set_update_check(s.update_check);
    state.set_high_contrast(s.high_contrast);

    state.set_listen(match s.listen {
        Listen::ThisComputer => 0,
        Listen::Network => 1,
        Listen::Specific => 2,
    });
    state.set_bind_address(s.bind_address.as_str().into());
    state.set_port(s.port.to_string().into());
    let ports = s.effective_ports().ok();
    let text = |port: Option<u16>| port.map_or_else(|| "-".to_owned(), |p| ToString::to_string(&p));
    state.set_port_viewer_enroll(text(ports.map(|p| p.viewer_enroll)).into());
    state.set_port_worker(text(ports.map(|p| p.worker)).into());
    state.set_port_worker_enroll(text(ports.map(|p| p.worker_enroll)).into());
    state.set_max_connections(int(s.max_connections));
    state.set_max_per_address(int(s.max_per_address));
    state.set_discovery(s.discovery);

    state.set_allow_pairing(s.allow_pairing);
    state.set_cert_names(s.cert_names_text().into());

    state.set_job_memory(int(s.max_job_memory_mib));
    state.set_whole_seconds(int(s.whole_image_seconds));
    state.set_whole_megapixels(
        i32::try_from((s.whole_image_pixels / MEGA).clamp(1, 4096)).unwrap_or(64),
    );
    state.set_jobs_per_device(int(s.jobs_per_device));
    state.set_interactive_workers(step_from_workers(s.interactive_workers));
    state.set_pinned_worker(s.pinned_worker.as_str().into());
    state.set_compression(match s.network_compression {
        NetworkCompression::Default => 0,
        NetworkCompression::Fast => 1,
        NetworkCompression::Off => 2,
    });
    state.set_accept_workers(s.accept_render_computers);
    state.set_cpu_threads(int(s.cpu_threads));

    state.set_retention_days(int(s.job_retention_days));
    state.set_cache_mib(int(s.cache_size_mib));
    let default_dir = settings::default_data_dir().unwrap_or_default();
    let data = s.data_dir(&default_dir);
    state.set_data_folder(data.display().to_string().into());
    state.set_data_folder_custom(s.data_folder.is_some());
    state.set_jobs_folder(data.join("jobs").display().to_string().into());
    state.set_cache_folder(data.join("asset-cache").display().to_string().into());
    state.set_log_folder(data.join("logs").display().to_string().into());

    state.set_accept_any_cert(s.accept_any_client_cert);
    let shown = |port: Option<u16>| port.map(|p| ToString::to_string(&p)).unwrap_or_default();
    state.set_override_viewer(shown(s.viewer_port).into());
    state.set_override_viewer_enroll(shown(s.viewer_enroll_port).into());
    state.set_override_worker(shown(s.worker_port).into());
    state.set_override_worker_enroll(shown(s.worker_enroll_port).into());
    super::settings_remote_admin::refresh(window, backend);
}

/// A setting as a Slint number.
fn int(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

// ---------------------------------------------------------------------------------------------
// "Can phones reach this computer?"
// ---------------------------------------------------------------------------------------------

/// The texts of the network card.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NetworkText {
    /// The network profile line.
    profile: String,
    /// 0 fine, 1 attention, 2 not known.
    profile_level: i32,
    /// The firewall line.
    firewall: String,
    /// 0 fine, 1 attention, 2 not known.
    firewall_level: i32,
    /// What to do about a problem, in the wording of this system; empty when nothing is wrong.
    advice: String,
}

/// Words for a firewall and network-profile reading.
fn describe_network(
    firewall: FirewallState,
    profile: NetworkProfile,
    warnings: &[String],
) -> NetworkText {
    let (profile_text, profile_level) = match profile {
        NetworkProfile::Private => ("Private network", 0),
        NetworkProfile::Public => (
            "Public network: other devices may not reach this computer",
            1,
        ),
        NetworkProfile::Unknown => ("Not known on this system", 2),
    };
    let (firewall_text, firewall_level) = match firewall {
        FirewallState::Open => ("Pleochroma is allowed", 0),
        FirewallState::Blocked => ("The firewall may block other devices", 1),
        FirewallState::Unknown => (platform::firewall_unknown_text(), 2),
    };
    NetworkText {
        profile: profile_text.to_owned(),
        profile_level,
        firewall: firewall_text.to_owned(),
        firewall_level,
        advice: warnings.join(" "),
    }
}

/// The last device that connected, in words, and its level (0 found, 2 none yet).
fn last_contact(status: &CoordinatorStatus, now: u64) -> (String, i32) {
    let best = status
        .devices
        .iter()
        .filter(|d| d.role != PeerRole::Worker)
        .filter_map(|d| {
            let when = if d.online {
                Some(u64::MAX)
            } else {
                d.last_seen
            };
            when.map(|w| (w, d))
        })
        .max_by_key(|(when, _)| *when);
    match best {
        Some((u64::MAX, device)) => (format!("{}, connected now", device.name), 0),
        Some((when, device)) => (
            format!("{}, {}", device.name, ago(now.saturating_sub(when))),
            0,
        ),
        None => ("No device has connected yet".to_owned(), 2),
    }
}

/// "3 min ago", "2 h ago", "5 days ago".
pub(super) fn ago(seconds: u64) -> String {
    match seconds {
        0..=59 => "just now".to_owned(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => format!("{} h ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

/// Starts the firewall and profile check on a thread and shows "Checking…" meanwhile.
fn start_network_check(window: &AppWindow, backend: &ActiveBackend) {
    let state = window.global::<SettingsState>();
    state.set_net_profile("Checking…".into());
    state.set_net_profile_level(2);
    state.set_net_firewall("Checking…".into());
    state.set_net_firewall_level(2);
    state.set_net_advice(SharedString::new());
    let (contact, contact_level) = last_contact(&backend.get().status(), unix_now());
    state.set_net_contact(contact.into());
    state.set_net_contact_level(contact_level);

    let ports = settings::current()
        .effective_ports()
        .map(|p| p.all().to_vec())
        .unwrap_or_default();
    let weak = window.as_weak();
    let spawned = std::thread::Builder::new()
        .name("settings-network-check".to_owned())
        .spawn(move || {
            let firewall = platform::firewall_state(&ports);
            let profile = platform::network_profile();
            let warnings = platform::network_warnings(firewall, profile, &ports);
            let text = describe_network(firewall, profile, &warnings);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak.upgrade() {
                    let state = window.global::<SettingsState>();
                    state.set_net_profile(text.profile.into());
                    state.set_net_profile_level(text.profile_level);
                    state.set_net_firewall(text.firewall.into());
                    state.set_net_firewall_level(text.firewall_level);
                    state.set_net_advice(text.advice.into());
                }
            });
        });
    if let Err(e) = spawned {
        tracing::warn!("could not start the network check: {e}");
    }
}

// ---------------------------------------------------------------------------------------------
// Backup and restore
// ---------------------------------------------------------------------------------------------

/// The dialog's button: checks the passwords, asks for the file (on a helper thread), then seals
/// or opens it on a thread of its own. `mode` 1 is a backup, 2 a restore. A backup is never
/// offered to a remote administrator; this page only ever runs in the coordinator's own window.
fn run_backup(
    window: &AppWindow,
    backend: &ActiveBackend,
    mode: i32,
    password: Zeroizing<String>,
    repeat: &Zeroizing<String>,
) {
    let state = window.global::<SettingsState>();
    state.set_dialog_error(SharedString::new());
    state.set_dialog_done(SharedString::new());
    let export = mode == 1;
    if export {
        if password.chars().count() < MIN_PASSWORD_CHARS {
            state.set_dialog_error(
                format!("The password needs at least {MIN_PASSWORD_CHARS} characters.").into(),
            );
            return;
        }
        if *password != **repeat {
            state.set_dialog_error("The two passwords are not the same.".into());
            return;
        }
    }
    let request = if export {
        Request::new(Kind::SaveFile, "Save the backup")
            .file_name("pleochroma-coordinator.pleobackup")
    } else {
        Request::new(Kind::OpenFile, "Choose the backup")
    }
    .filter("Pleochroma backup", &["pleobackup"]);
    let active = backend.clone();
    // The password moves into the continuation; it is wiped there, or when the dialog is
    // cancelled and the continuation is dropped.
    // The dialog is busy while the file picker is open, so a second click opens no second one.
    state.set_dialog_busy(true);
    let started = picker::pick_or_else(
        &window.as_weak(),
        &request,
        move |window, path| start_backup(window, &active, export, path, password),
        |window| window.global::<SettingsState>().set_dialog_busy(false),
    );
    if let Err(reason) = started {
        state.set_dialog_busy(false);
        state.set_dialog_error(reason.into());
    }
}

/// Seals or opens the backup file `path` on a thread of its own and shows the outcome.
fn start_backup(
    window: &AppWindow,
    backend: &ActiveBackend,
    export: bool,
    path: PathBuf,
    password: Zeroizing<String>,
) {
    let state = window.global::<SettingsState>();
    state.set_dialog_busy(true);
    let weak = window.as_weak();
    let backend = backend.clone();
    let spawned = std::thread::Builder::new()
        .name("settings-backup".to_owned())
        .spawn(move || {
            let result = if export {
                backup::export_backup(&path, &password)
            } else {
                backup::import_backup(&path, &password)
            };
            drop(password);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak.upgrade() {
                    finish_backup(&window, &backend, export, &path, result);
                }
            });
        });
    if let Err(e) = spawned {
        state.set_dialog_busy(false);
        state.set_dialog_error(format!("Could not start the work: {e}").into());
    }
}

/// Shows the outcome of a backup or restore.
fn finish_backup(
    window: &AppWindow,
    backend: &ActiveBackend,
    export: bool,
    path: &Path,
    result: Result<(), String>,
) {
    let state = window.global::<SettingsState>();
    state.set_dialog_busy(false);
    match result {
        Ok(()) if export => {
            state.set_dialog_done(format!("Backup saved to {}.", path.display()).into());
        }
        Ok(()) => {
            state.set_dialog_done(
                "Restored. Restart the coordinator to use the restored certificates.".into(),
            );
            state.set_restart_needed(true);
        }
        Err(reason) => state.set_dialog_error(reason.into()),
    }
    refresh(window, backend);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::backend::DeviceStatus;

    #[test]
    fn ports_are_parsed_or_cleared() {
        assert_eq!(parse_port(""), Ok(None));
        assert_eq!(parse_port("7878"), Ok(Some(7878)));
        assert!(parse_port("0").is_err());
        assert!(parse_port("70000").is_err());
        assert!(parse_port("abc").is_err());
    }

    #[test]
    fn the_top_stepper_value_means_all_render_computers() {
        assert_eq!(workers_from_step(17), u32::MAX);
        assert_eq!(workers_from_step(3), 3);
        assert_eq!(step_from_workers(u32::MAX), 17);
        assert_eq!(step_from_workers(4), 4);
    }

    #[test]
    fn the_network_card_names_a_blocked_firewall() {
        let warnings =
            platform::network_warnings(FirewallState::Blocked, NetworkProfile::Public, &[7878]);
        let text = describe_network(FirewallState::Blocked, NetworkProfile::Public, &warnings);
        assert_eq!(text.profile_level, 1);
        assert_eq!(text.firewall_level, 1);
        assert!(!text.advice.is_empty());
        let fine = describe_network(FirewallState::Open, NetworkProfile::Private, &[]);
        assert_eq!((fine.profile_level, fine.firewall_level), (0, 0));
        assert!(fine.advice.is_empty());
    }

    #[test]
    fn the_last_contact_is_the_newest_device() {
        let mut status =
            CoordinatorStatus::minimal("Studio", "0.12.0", 45, CoordinatorState::Running);
        assert_eq!(last_contact(&status, 1000).1, 2);
        let mut old = DeviceStatus::new("Tablet", "aa".repeat(32), PeerRole::Viewer);
        old.last_seen = Some(100);
        let mut newer = DeviceStatus::new("Phone", "bb".repeat(32), PeerRole::Viewer);
        newer.last_seen = Some(700);
        status.devices.push(old);
        status.devices.push(newer);
        assert_eq!(
            last_contact(&status, 1000),
            ("Phone, 5 min ago".to_owned(), 0)
        );
        status.devices[0].online = true;
        assert_eq!(
            last_contact(&status, 1000),
            ("Tablet, connected now".to_owned(), 0)
        );
    }

    #[test]
    fn a_folder_without_certificates_is_refused() {
        let dir =
            std::env::temp_dir().join(format!("pleochroma-settings-page-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let reason = use_existing_folder(&dir).expect_err("no pki");
        assert!(reason.contains("pki/ca.pem"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
