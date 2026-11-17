//! The first-run wizard's driver.
//!
//! The Slint side (`ui/wizard.slint`, `WizardPage` and the `WizardState` global) draws the six
//! steps and keeps the owner's choices in `WizardState`. This file fills the defaults, runs
//! the two pickers and, when the last step's button is pressed, reads the choices back as a
//! [`WizardChoices`] and closes the page.
//!
//! On finish it writes `coordinator.toml` and the certificates (`pki/`: CA, server
//! certificate and key) into [`local::folder`], then closes the page; the shell starts the
//! coordinator and applies the "start at login" choice, which is stored in the settings.
//! A hook registered with [`set_finish_hook`] also receives the choices. When the choice was
//! "run a coordinator on this computer" the wizard then opens the pairing dialog on the phone
//! tab.

use crate::{
    control::status::default_display_name,
    gui::{
        app::{ActiveBackend, AppWindow, PairState, WizardState},
        local,
        pages::picker::{self, Kind, Request},
    },
    pki, settings,
    settings::model::{
        CoordinatorSettings, Listen, MAX_NAME_CHARS, RenderChoice as ModelRenderChoice, RunMode,
    },
};
use slint::{ComponentHandle, SharedString};
use std::{cell::RefCell, net::IpAddr, path::PathBuf, time::Duration};

/// Where the coordinator's library comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryChoice {
    /// A library file picked by the owner.
    File(PathBuf),
    /// "Rendering only (no library)".
    NoLibrary,
    /// An existing coordinator folder (a headless setup moved over, keeping every paired
    /// device and the keys in its `pki/` folder).
    ExistingFolder(PathBuf),
}

/// What this computer renders with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderChoice {
    /// The graphics card first, the processor for the rest.
    GpuAndCpu,
    /// The graphics card only.
    GpuOnly,
    /// The processor only.
    CpuOnly,
}

/// Everything the wizard asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WizardChoices {
    /// [`RunMode::Here`]: run a coordinator on this computer. [`RunMode::Monitor`]: only
    /// monitor or manage others (every field below is then unset: defaults).
    pub run: RunMode,
    /// The coordinator's name, at most 64 characters.
    pub name: String,
    /// The library.
    pub library: LibraryChoice,
    /// [`Listen::Network`]: devices on the network may connect ("Devices on my network").
    /// [`Listen::ThisComputer`]: only this computer.
    pub listen: Listen,
    /// How this computer renders.
    pub render: RenderChoice,
    /// "Start when I log in", off by default. Only a choice here: the settings page switches
    /// the autostart on (decision D10: the coordinator then runs only while this user is
    /// logged in).
    pub start_at_login: bool,
    /// "Keep this computer awake while devices are connected or jobs run", on by default.
    pub keep_awake: bool,
}

type FinishHook = Box<dyn Fn(&AppWindow, &WizardChoices)>;

thread_local! {
    /// The function that acts on the finished wizard (the user-interface thread only).
    static FINISH_HOOK: RefCell<Option<FinishHook>> = const { RefCell::new(None) };
}

/// Registers what happens when the wizard is finished: it receives the window and the owner's
/// choices.
///
/// Settings persistence, certificate creation, starting the coordinator and autostart go here. Call
/// it from the user-interface thread; a second call replaces the first.
pub fn set_finish_hook(hook: impl Fn(&AppWindow, &WizardChoices) + 'static) {
    FINISH_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

impl WizardChoices {
    /// Whether a coordinator runs on this computer.
    #[must_use]
    pub fn runs_here(&self) -> bool {
        self.run == RunMode::Here
    }
}

/// `base` with the wizard's choices set: the eight wizard keys (`name`, `run`, `listen`,
/// `library`, `render`, `render_mode`, `keep_awake`, `start_at_login`). Every other setting
/// stays as it was, so running the setup again does not reset the rest of the file.
fn apply_choices(mut base: CoordinatorSettings, c: &WizardChoices) -> CoordinatorSettings {
    base.name.clone_from(&c.name);
    base.run = c.run;
    base.listen = c.listen;
    base.library = match &c.library {
        LibraryChoice::File(path) if !path.as_os_str().is_empty() => Some(path.clone()),
        LibraryChoice::File(_) | LibraryChoice::NoLibrary | LibraryChoice::ExistingFolder(_) => {
            None
        }
    };
    base.render = c.runs_here();
    base.render_mode = match c.render {
        RenderChoice::GpuAndCpu => ModelRenderChoice::GpuAndCpu,
        RenderChoice::GpuOnly => ModelRenderChoice::Gpu,
        RenderChoice::CpuOnly => ModelRenderChoice::Cpu,
    };
    base.keep_awake = c.keep_awake;
    base.start_at_login = c.start_at_login;
    base
}

/// Writes the settings file (through [`CoordinatorSettings::save`], keeping every setting the
/// wizard does not ask) and, for a coordinator run here, the certificates.
fn write_setup(c: &WizardChoices) -> Result<(), String> {
    let folder = local::folder().ok_or("The coordinator folder is not known.")?;
    if c.runs_here() {
        let dir = folder.pki_dir();
        if let LibraryChoice::ExistingFolder(from) = &c.library {
            take_over_pki(&from.join("pki"), &dir)?;
        }
        if !dir.join(pki::CA_CERT_FILE).exists() {
            pki::init(&dir).map_err(|e| format!("Could not create the certificates: {e}"))?;
        }
        if !dir.join(pki::SERVER_CERT_FILE).exists() {
            let hosts = vec![
                "localhost".to_string(),
                crate::control::status::default_display_name(),
            ];
            let mut ips: Vec<IpAddr> = vec![IpAddr::from([127, 0, 0, 1])];
            ips.extend(
                pleochroma_net::netinfo::lan_ipv4()
                    .iter()
                    .map(|a| IpAddr::V4(a.addr)),
            );
            pki::issue_server(&dir, &hosts, &ips)
                .map_err(|e| format!("Could not create the server certificate: {e}"))?;
        }
    }
    apply_choices(settings::current(), c).save(folder.settings_file())
}

/// Copies the files of an existing coordinator's `pki/` folder (keys, certificates,
/// allowlists) that this computer does not have yet, so every paired device stays paired.
fn take_over_pki(from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(to)
        .map_err(|e| format!("Could not create the certificate folder: {e}"))?;
    let entries =
        std::fs::read_dir(from).map_err(|e| format!("Could not read that folder: {e}"))?;
    for entry in entries.flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_file() && !target.exists() {
            std::fs::copy(entry.path(), &target).map_err(|e| {
                format!(
                    "Could not copy {}: {e}",
                    entry.file_name().to_string_lossy()
                )
            })?;
        }
    }
    Ok(())
}

/// Connects the wizard's pickers and its finish button. The page stays closed until [`open`].
pub fn wire(window: &AppWindow, _backend: &ActiveBackend) {
    let state = window.global::<WizardState>();
    state.set_open(false);

    // Both dialogs run on a helper thread; the choice comes back to the UI thread.
    state.on_choose_library({
        let weak = window.as_weak();
        move || {
            let request = Request::new(Kind::OpenFile, "Choose your design library")
                .filter("Design library", &["sqlite", "db"]);
            let started = picker::pick(&weak, &request, |win, path| {
                let state = win.global::<WizardState>();
                if path.is_file() {
                    state.set_library_path(SharedString::from(&*path.to_string_lossy()));
                    state.set_error(SharedString::new());
                } else {
                    state.set_error(SharedString::from(
                        "That file cannot be opened. Pick another one.",
                    ));
                }
            });
            show_picker_problem(&weak, started);
        }
    });

    state.on_choose_folder({
        let weak = window.as_weak();
        move || {
            let request = Request::new(Kind::Folder, "Choose the coordinator folder");
            let started = picker::pick(&weak, &request, |win, path| {
                let state = win.global::<WizardState>();
                match check_coordinator_folder(&path) {
                    Ok(()) => {
                        state.set_folder_path(SharedString::from(&*path.to_string_lossy()));
                        state.set_error(SharedString::new());
                    }
                    Err(reason) => state.set_error(SharedString::from(reason)),
                }
            });
            show_picker_problem(&weak, started);
        }
    });

    state.on_finished({
        let weak = window.as_weak();
        move || {
            let Some(win) = weak.upgrade() else {
                return;
            };
            let finished = choices(&win);
            if let Err(reason) = write_setup(&finished) {
                // The page stays open with the reason; nothing was half-applied that matters.
                win.global::<WizardState>()
                    .set_error(SharedString::from(reason));
                return;
            }
            win.global::<WizardState>().set_error(SharedString::new());
            win.global::<WizardState>().set_open(false);
            FINISH_HOOK.with(|slot| {
                if let Some(hook) = slot.borrow().as_ref() {
                    hook(&win, &finished);
                }
            });
            if finished.runs_here() {
                open_pairing_soon(&win);
            }
        }
    });
}

/// Shows on the wizard page why a file dialog could not open.
fn show_picker_problem(weak: &slint::Weak<AppWindow>, started: Result<(), String>) {
    if let (Err(reason), Some(win)) = (started, weak.upgrade()) {
        win.global::<WizardState>()
            .set_error(SharedString::from(reason));
    }
}

/// Opens the wizard on its first step with fresh defaults (the computer's name, the address
/// phones would use). The first start calls this; so can "Run the setup again".
pub fn open(window: &AppWindow) {
    let state = window.global::<WizardState>();
    state.set_step(0);
    state.set_mode(0);
    state.set_name(SharedString::from(default_display_name()));
    state.set_library_kind(0);
    state.set_library_path(SharedString::new());
    state.set_folder_path(SharedString::new());
    state.set_network(0);
    state.set_network_address(SharedString::from(network_text()));
    state.set_render_mode(0);
    state.set_start_at_login(false);
    state.set_keep_awake(true);
    state.set_error(SharedString::new());
    state.set_open(true);
}

/// The owner's choices as they stand now.
#[must_use]
pub fn choices(window: &AppWindow) -> WizardChoices {
    let state = window.global::<WizardState>();
    let run = if state.get_mode() == 0 {
        RunMode::Here
    } else {
        RunMode::Monitor
    };
    let library = match state.get_library_kind() {
        1 => LibraryChoice::NoLibrary,
        2 => LibraryChoice::ExistingFolder(PathBuf::from(state.get_folder_path().as_str())),
        _ => LibraryChoice::File(PathBuf::from(state.get_library_path().as_str())),
    };
    let render = match state.get_render_mode() {
        1 => RenderChoice::GpuOnly,
        2 => RenderChoice::CpuOnly,
        _ => RenderChoice::GpuAndCpu,
    };
    WizardChoices {
        run,
        name: state
            .get_name()
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_NAME_CHARS)
            .collect(),
        library,
        listen: if state.get_network() == 0 {
            Listen::Network
        } else {
            Listen::ThisComputer
        },
        render,
        start_at_login: state.get_start_at_login(),
        keep_awake: state.get_keep_awake(),
    }
}

/// "192.168.178.20 · Wi-Fi": the address phones would use, the default-route adapter first.
/// Empty when the computer has no network address.
fn network_text() -> String {
    pleochroma_net::netinfo::lan_ipv4()
        .first()
        .map(|a| format!("{} · {}", a.addr, a.interface))
        .unwrap_or_default()
}

/// A coordinator folder is one with a `pki` folder inside: the keys a headless setup made.
fn check_coordinator_folder(path: &std::path::Path) -> Result<(), &'static str> {
    if !path.is_dir() {
        Err("That folder cannot be opened. Pick another one.")
    } else if !path.join("pki").is_dir() {
        Err(
            "This folder has no pki folder, so it is not a coordinator folder. Pick the folder that contains it.",
        )
    } else {
        Ok(())
    }
}

/// Opens the pairing dialog on the phone tab a moment after the wizard closes, so the
/// coordinator has started. If it is not up yet the dialog says so and offers "Try again".
fn open_pairing_soon(window: &AppWindow) {
    let weak = window.as_weak();
    slint::Timer::single_shot(Duration::from_millis(1500), move || {
        if let Some(win) = weak.upgrade() {
            win.global::<PairState>().invoke_open_requested(0);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_without_pki_is_refused_with_a_reason() {
        let dir = std::env::temp_dir().join("pleochroma-wizard-test-no-pki");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let reason = check_coordinator_folder(&dir).expect_err("no pki");
        assert!(reason.contains("pki"));
        std::fs::create_dir_all(dir.join("pki")).expect("pki dir");
        assert!(check_coordinator_folder(&dir).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_choices_change_only_the_wizard_keys() {
        let base = CoordinatorSettings {
            max_connections: 12,
            ..CoordinatorSettings::default()
        };
        let choices = WizardChoices {
            run: RunMode::Here,
            name: "Studio".to_owned(),
            library: LibraryChoice::File(PathBuf::from("lib.sqlite")),
            listen: Listen::Network,
            render: RenderChoice::CpuOnly,
            start_at_login: true,
            keep_awake: false,
        };
        let done = apply_choices(base, &choices);
        assert_eq!(done.max_connections, 12, "other settings stay");
        assert_eq!(done.name, "Studio");
        assert_eq!(done.listen, Listen::Network);
        assert_eq!(done.library, Some(PathBuf::from("lib.sqlite")));
        assert!(done.render && done.start_at_login && !done.keep_awake);
        assert_eq!(done.render_mode, ModelRenderChoice::Cpu);
        assert!(done.validate().is_ok());
    }

    #[test]
    fn a_missing_folder_is_refused() {
        let missing = std::env::temp_dir().join("pleochroma-wizard-test-missing-folder");
        assert!(check_coordinator_folder(&missing).is_err());
    }
}
