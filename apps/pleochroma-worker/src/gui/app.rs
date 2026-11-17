//! The coordinator window: start-up, the shell (status pill, banner, sidebar counters,
//! Start/Stop), the tray and the one place that knows which backend the pages talk to.
//!
//! `slint::include_modules!()` below generates `AppWindow` and every exported global
//! (`ShellState` and each page's `<Page>State`); the page files import them from here
//! (`use crate::gui::app::{ActiveBackend, AppWindow, DevicesState};`).
//!
//! # Page contract
//!
//! Each page file has `pub fn wire(window: &AppWindow, backend: &ActiveBackend)`, called once
//! from [`run`]. A page that needs the backend in a callback or a timer clones the
//! [`ActiveBackend`] (cheap) and calls [`ActiveBackend::get`] on every use, never keeping the
//! `Arc` it returned.
//!
//! # Shell
//!
//! A timer asks the local coordinator for its state four times a second and shows it: the
//! status pill (Starting…, Running, Stopping…, Stopped, Problem), Start/Stop and
//! Connect-a-phone enablement, the sidebar counters, the tray's status line, and a banner for
//! "starting" and for a failed start. The shell sets the banner only when its own text is what
//! is showing, so a page's banner is never wiped.
//!
//! # Coordinator switcher
//!
//! The sidebar's switcher ([`switcher::wire`]) selects the backend the pages talk to: this
//! computer's coordinator, or a saved remote one ([`crate::gui::remote::open`]). The shell then
//! shows the selected coordinator: its state in the status pill ("Running · remote"), its
//! counters, and a banner "Managing <name> remotely · Admin" (or "Monitor"). Start/Stop,
//! "Open data folder", Tools and "Help another coordinator" belong to this computer and give way
//! to a Disconnect button on a remote one. The tray always speaks for this computer.
//!
//! # Settings
//!
//! [`settings::init`] reads the settings file at start. A broken file starts the window with
//! defaults and its notice shows in the banner. The coordinator starts only when this computer
//! runs one (`runs_here`); the arguments come from [`settings::CoordinatorSettings::serve_args`].
//! High contrast follows the setting (checked once a second), and the login entry follows
//! "start at login" at start.
//!
//! # First run
//!
//! When [`CoordinatorFolder::is_first_run`] is true the shell opens the wizard
//! (`WizardState.open = true`; the wizard page covers the whole window) and starts no
//! coordinator. The wizard's Rust side writes the folder (see [`local::folder`]) and closes the
//! page. The shell notices that the wizard has closed and the folder is set up, reads the
//! settings again, and starts the coordinator; `ShellState.wizard-finished()` does the same at
//! once when the wizard calls it.
//!
//! # Tray and closing
//!
//! With a tray (see [`tray::host_available`]) closing the window hides it and the coordinator
//! keeps running; the first time, a note says so for a few seconds before the window hides.
//! Without a tray host closing minimizes the window, never hides it. With "close to tray" off,
//! closing quits. "Quit" in the tray menu always quits. Quitting ends the event loop, then
//! stops the coordinator (up to the shutdown deadline) and ends the process.
//!
//! `--minimized` (what the login entry passes) starts with no window when there is a tray, and
//! with a minimized window when there is none. A second start of the program calls
//! [`InstanceGuard::listen`]'s callback, which brings this window forward.

use crate::{
    cli::GlobalFlags,
    control::backend::{AccessRole, CoordinatorBackend, CoordinatorState},
    gui::{
        add_coordinator,
        local::{self, CoordinatorFolder, LocalBackend},
        pages, shader_cache, switcher,
        util::open_folder,
    },
    platform::{
        autostart,
        instance::InstanceGuard,
        tray::{self, Tray},
    },
    settings,
};
use slint::{CloseRequestResponse, ComponentHandle, SharedString, Timer, TimerMode};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError, RwLock},
    time::{Duration, Instant},
};

slint::include_modules!();

/// How often the shell reads the coordinator's state.
const SHELL_REFRESH: Duration = Duration::from_millis(250);

/// How many shell refreshes lie between two reads of the settings the window follows.
const SETTINGS_POLL_EVERY: u32 = 4;

/// The banner the shell shows while the coordinator starts.
const STARTING_BANNER: &str = "Starting the coordinator. This can take a few seconds while the library opens and the graphics card is prepared.";

/// What the first close says before the window goes to the tray.
const TRAY_NOTE: &str =
    "Pleochroma Coordinator is still running in the tray. Choose Quit in the tray menu to stop it.";

/// How long the first close keeps the window up so the note can be read.
const TRAY_NOTE_TIME: Duration = Duration::from_secs(4);

/// The file in the data folder that remembers the first-close note was shown.
const TRAY_NOTE_MARKER: &str = "tray-note-shown";

/// What a failed start adds when the reason is a port that is already in use.
const PORT_IN_USE_HINT: &str =
    " Another user on this computer may be running a coordinator on the same port.";

/// The coordinator the pages talk to: the one in this process now, a remote one later.
///
/// Cheap to clone; every clone sees [`Self::set`]. Pages call [`Self::get`] on every use, so a
/// switch of coordinator reaches all of them at once.
#[derive(Clone)]
pub struct ActiveBackend {
    current: Arc<RwLock<Arc<dyn CoordinatorBackend>>>,
}

impl ActiveBackend {
    /// An active backend that starts with `backend`.
    #[must_use]
    pub fn new(backend: Arc<dyn CoordinatorBackend>) -> Self {
        Self {
            current: Arc::new(RwLock::new(backend)),
        }
    }

    /// The selected backend. Call it on every use; do not keep the result.
    #[must_use]
    pub fn get(&self) -> Arc<dyn CoordinatorBackend> {
        Arc::clone(&self.current.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Selects another backend for every page.
    pub fn set(&self, backend: Arc<dyn CoordinatorBackend>) {
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = backend;
    }
}

/// What the command line asked of the window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuiOptions {
    /// `--minimized`: start in the tray (or with the window minimized when there is no tray).
    pub minimized: bool,
    /// `--config <file>`: the settings file (its folder is then the data folder).
    pub config: Option<PathBuf>,
}

impl From<&GlobalFlags> for GuiOptions {
    fn from(flags: &GlobalFlags) -> Self {
        Self {
            minimized: flags.minimized,
            config: flags.config.clone(),
        }
    }
}

/// Why the window did not open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuiError {
    /// No display, or the windowing backend could not start (the detail names which).
    /// The program prints the "No display found" hint and exits with status 2.
    NoDisplay(String),
    /// Anything else, in plain words. The program exits with status 1.
    Failed(String),
}

/// Whether a display is there to open a window on. Only Linux can lack one before the
/// windowing backend starts: it needs `DISPLAY` or `WAYLAND_DISPLAY`.
fn display_available() -> bool {
    if cfg!(target_os = "linux") {
        ["DISPLAY", "WAYLAND_DISPLAY"]
            .iter()
            .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()))
    } else {
        true
    }
}

/// Where the single-instance listener finds the window: empty while the window opens.
#[derive(Default)]
struct ActivateSlot {
    /// The window, once it exists.
    window: Option<slint::Weak<AppWindow>>,
    /// A second start asked before the window existed.
    asked: bool,
}

/// The settings the window itself follows.
struct WindowSettings {
    /// Whether the program starts at login.
    start_at_login: bool,
    /// Whether the window uses the high-contrast colors.
    high_contrast: bool,
    /// Whether closing the window leaves the coordinator running in the tray.
    close_to_tray: bool,
}

/// The live settings, reduced to what the window follows.
fn window_settings() -> WindowSettings {
    let current = settings::current();
    WindowSettings {
        start_at_login: current.start_at_login,
        high_contrast: current.high_contrast,
        close_to_tray: current.close_to_tray,
    }
}

/// Opens the coordinator window and runs until it quits, then stops the coordinator.
///
/// # Errors
///
/// [`GuiError::NoDisplay`] without a display or when the windowing backend cannot start;
/// [`GuiError::Failed`] when no folder for the settings can be found.
pub fn run(options: &GuiOptions) -> Result<(), GuiError> {
    run_with_instance(options, None)
}

/// Like [`run`], with the single-instance lock this process holds: a later start of the
/// program brings this window forward.
///
/// # Errors
///
/// As [`run`].
pub fn run_with_instance(
    options: &GuiOptions,
    instance: Option<InstanceGuard>,
) -> Result<(), GuiError> {
    // Answer a second start at once, so two starts that began together never both open a
    // window: the window itself is filled into the slot once it exists.
    let activate_slot = Arc::new(Mutex::new(ActivateSlot::default()));
    if let Some(guard) = instance {
        let slot = Arc::clone(&activate_slot);
        guard.listen(move || {
            let mut waiting = slot.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(weak) = waiting.window.clone() {
                let _ = weak.upgrade_in_event_loop(|window| restore_window(&window));
            } else {
                waiting.asked = true;
            }
        });
    }
    let folder = CoordinatorFolder::locate(options.config.as_deref()).ok_or_else(|| {
        GuiError::Failed("could not find a folder for the coordinator's settings".to_owned())
    })?;
    local::set_folder(folder.clone());
    if !display_available() {
        return Err(GuiError::NoDisplay(
            "neither DISPLAY nor WAYLAND_DISPLAY is set".to_owned(),
        ));
    }
    let loaded = settings::init(folder.settings_file().to_path_buf());
    let window = AppWindow::new().map_err(|e| GuiError::NoDisplay(e.to_string()))?;
    let followed = window_settings();
    apply_high_contrast(&window, followed.high_contrast);
    // The login entry starts the program with the same `--config` file as this run.
    autostart::set_config(options.config.as_deref());
    sync_autostart(followed.start_at_login);

    let local = LocalBackend::new();
    let backend = ActiveBackend::new(local.clone());
    wire_pages(&window, &backend);
    wire_shell(&window, &local, &folder);
    let switcher = switcher::wire(&window, &backend, &local);
    add_coordinator::wire(&window, &switcher);
    // A computer last set to help another coordinator rejoins it, also on a tray-only start.
    crate::gui::pages::worker_mode::resume_if_saved(&window);

    let mut tracker = ShellTracker::default();
    if let Some(notice) = &loaded.notice {
        let shell = window.global::<ShellState>();
        shell.set_banner_text(notice.as_str().into());
        shell.set_banner_tone(1);
        tracker.notice = Some(SharedString::from(notice.as_str()));
    }
    let first_run = folder.is_first_run();
    if first_run {
        crate::gui::pages::wizard::open(&window);
        tracker.wizard_pending = true;
    } else {
        start_local(&local, &folder);
    }

    let tray_up = install_tray(&window, &mut tracker);
    install_close_handler(&window, tray_up, &folder);
    // The window exists now: a request that came while it was opening is answered late.
    {
        let mut waiting = activate_slot.lock().unwrap_or_else(PoisonError::into_inner);
        let weak = window.as_weak();
        if waiting.asked {
            let _ = weak.upgrade_in_event_loop(|window| restore_window(&window));
        }
        waiting.window = Some(weak);
    }

    let weak = window.as_weak();
    let refresh_local = Arc::clone(&local);
    let refresh_backend = backend;
    let refresh_folder = folder;
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, SHELL_REFRESH, move || {
        if let Some(window) = weak.upgrade() {
            refresh_shell(
                &window,
                &refresh_local,
                &refresh_backend,
                &refresh_folder,
                &mut tracker,
            );
        }
    });

    // The first run needs the wizard, so it never starts hidden.
    let start_hidden = options.minimized && tray_up && !first_run;
    if !start_hidden {
        window
            .show()
            .map_err(|e| GuiError::NoDisplay(e.to_string()))?;
        if options.minimized {
            window.window().set_minimized(true);
        }
    }
    let outcome = slint::run_event_loop_until_quit().map_err(|e| GuiError::Failed(e.to_string()));
    drop(timer);
    crate::gui::pages::worker_mode::shutdown();
    local.shutdown_blocking();
    outcome
}

/// Brings the window back from the tray or from minimized.
fn restore_window(window: &AppWindow) {
    if let Err(e) = window.show() {
        tracing::warn!("could not show the window: {e}");
    }
    window.window().set_minimized(false);
}

/// Sets the window's high-contrast colors when they differ.
fn apply_high_contrast(window: &AppWindow, on: bool) {
    let theme = window.global::<Theme>();
    if theme.get_high_contrast() != on {
        theme.set_high_contrast(on);
    }
}

/// Makes the login entry match the "start at login" setting, off the UI thread. An entry that
/// points at another program path or settings file (the program was moved or updated) is
/// written again.
fn sync_autostart(wanted: bool) {
    let spawned = std::thread::Builder::new()
        .name("autostart-sync".to_owned())
        .spawn(move || {
            let result = if wanted {
                if autostart::is_current() {
                    Ok(())
                } else {
                    autostart::enable()
                }
            } else if autostart::is_enabled() {
                autostart::disable()
            } else {
                Ok(())
            };
            if let Err(e) = result {
                tracing::warn!("could not update the start-at-login entry: {e}");
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("could not start the start-at-login check: {e}");
    }
}

/// Puts the tray icon up when something will show it. True when it is up.
fn install_tray(window: &AppWindow, tracker: &mut ShellTracker) -> bool {
    if !tray::host_available() {
        tracing::info!("no tray host on this desktop: closing the window minimizes it");
        return false;
    }
    let open_weak = window.as_weak();
    let pair_weak = window.as_weak();
    let quit_weak = window.as_weak();
    let made = Tray::new(
        move || {
            if let Some(window) = open_weak.upgrade() {
                restore_window(&window);
            }
        },
        move || {
            if let Some(window) = pair_weak.upgrade() {
                restore_window(&window);
                if window.global::<ShellState>().get_can_pair() {
                    window.global::<PairState>().invoke_open_requested(0);
                }
            }
        },
        move || quit(&quit_weak),
    );
    match made {
        Ok(tray) => {
            tracker.tray = Some(tray);
            true
        }
        Err(e) => {
            tracing::warn!("could not show the tray icon: {e}");
            false
        }
    }
}

/// What closing the window does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseAction {
    /// Quit the program.
    Quit,
    /// Hide the window; the tray keeps the coordinator reachable.
    Hide,
    /// Minimize the window (there is no tray to come back from).
    Minimize,
}

/// Chooses what closing does from the "close to tray" setting and whether a tray is up.
const fn close_action(close_to_tray: bool, tray_up: bool) -> CloseAction {
    if !close_to_tray {
        CloseAction::Quit
    } else if tray_up {
        CloseAction::Hide
    } else {
        CloseAction::Minimize
    }
}

/// Replaces "closing stops everything" with the tray behavior.
fn install_close_handler(window: &AppWindow, tray_up: bool, folder: &CoordinatorFolder) {
    let weak = window.as_weak();
    let marker = folder.data_dir().join(TRAY_NOTE_MARKER);
    window.window().on_close_requested(move || {
        let Some(window) = weak.upgrade() else {
            return CloseRequestResponse::HideWindow;
        };
        match close_action(window_settings().close_to_tray, tray_up) {
            CloseAction::Quit => {
                let _ = slint::quit_event_loop();
                CloseRequestResponse::HideWindow
            }
            CloseAction::Minimize => {
                window.window().set_minimized(true);
                CloseRequestResponse::KeepWindowShown
            }
            CloseAction::Hide if marker.exists() => CloseRequestResponse::HideWindow,
            CloseAction::Hide => {
                if let Some(dir) = marker.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&marker, "shown\n");
                let shell = window.global::<ShellState>();
                shell.set_banner_text(TRAY_NOTE.into());
                shell.set_banner_tone(0);
                let later = weak.clone();
                Timer::single_shot(TRAY_NOTE_TIME, move || {
                    if let Some(window) = later.upgrade() {
                        let shell = window.global::<ShellState>();
                        if shell.get_banner_text() == TRAY_NOTE {
                            shell.set_banner_text(SharedString::new());
                        }
                        let _ = window.hide();
                    }
                });
                CloseRequestResponse::KeepWindowShown
            }
        }
    });
}

/// Calls every page's `wire` once.
fn wire_pages(window: &AppWindow, backend: &ActiveBackend) {
    pages::overview::wire(window, backend);
    pages::devices::wire(window, backend);
    pages::render::wire(window, backend);
    pages::jobs::wire(window, backend);
    pages::library::wire(window, backend);
    pages::log::wire(window, backend);
    pages::settings::wire(window, backend);
    pages::worker_mode::wire(window, backend);
    pages::tools::wire(window, backend);
    pages::wizard::wire(window, backend);
    pages::pair::wire(window, backend);
}

/// Connects the top bar, the wizard hand-off and the "More" menu.
fn wire_shell(window: &AppWindow, local: &Arc<LocalBackend>, folder: &CoordinatorFolder) {
    let shell = window.global::<ShellState>();

    let toggle_local = Arc::clone(local);
    let toggle_folder = folder.clone();
    shell.on_stop_start(move || match toggle_local.state() {
        CoordinatorState::Running => toggle_local.stop(),
        CoordinatorState::Stopped | CoordinatorState::Error(_) => {
            start_local(&toggle_local, &toggle_folder);
        }
        CoordinatorState::Starting | CoordinatorState::Stopping => {}
    });

    let finish_local = Arc::clone(local);
    let finish_folder = folder.clone();
    shell.on_wizard_finished(move || start_after_wizard(&finish_local, &finish_folder));

    // The Devices page's "Connect a device" opens the pairing dialog on the phone tab.
    let weak = window.as_weak();
    window.global::<DevicesState>().on_connect_device(move || {
        if let Some(window) = weak.upgrade()
            && window.global::<ShellState>().get_can_pair()
        {
            window.global::<PairState>().invoke_open_requested(0);
        }
    });

    let data_dir = folder.data_dir().to_path_buf();
    shell.on_open_data_folder(move || open_folder(&data_dir));
    // The More menu's Quit ends the event loop like the tray's Quit; the coordinator is
    // stopped after the loop returns, with the window already gone.
    let quit_weak = window.as_weak();
    shell.on_quit(move || quit(&quit_weak));
}

/// Quits the program: the window goes first, so it does not sit frozen while the coordinator
/// shuts down (up to the shutdown deadline) after the event loop ends.
fn quit(window: &slint::Weak<AppWindow>) {
    if let Some(window) = window.upgrade()
        && let Err(e) = window.hide()
    {
        tracing::warn!("could not hide the window: {e}");
    }
    let _ = slint::quit_event_loop();
}

/// Starts the local coordinator with the current settings, unless this computer is set to help
/// another coordinator instead. A failure to build the arguments is logged; a failure to start
/// shows in the shell as a problem.
fn start_local(local: &LocalBackend, folder: &CoordinatorFolder) {
    let current = settings::current();
    if !current.runs_here() {
        tracing::info!("this computer is set to help another coordinator: not starting one");
        return;
    }
    match current.serve_args(folder.data_dir()) {
        Ok(args) => local.start(args),
        Err(e) => tracing::error!("could not prepare the coordinator's start: {e}"),
    }
}

/// After the wizard has written the settings file: reads it again, follows its login choice
/// and starts the coordinator.
fn start_after_wizard(local: &LocalBackend, folder: &CoordinatorFolder) {
    let _ = settings::init(folder.settings_file().to_path_buf());
    sync_autostart(window_settings().start_at_login);
    start_local(local, folder);
}

/// What the shell remembers between refreshes.
#[derive(Default)]
struct ShellTracker {
    /// The state shown last, so the banner changes only on a change of state.
    last: Option<CoordinatorState>,
    /// The banner text the shell put up itself, if it is still the one showing.
    own_banner: Option<SharedString>,
    /// The settings notice (a broken file was set aside), if it is still the banner. A start
    /// banner never replaces it; a problem does.
    notice: Option<SharedString>,
    /// True while the first-run wizard the shell opened has not closed yet.
    wizard_pending: bool,
    /// The tray icon, kept alive here for the whole run.
    tray: Option<Tray>,
    /// Refreshes left until the settings the window follows are read again.
    poll_in: u32,
    /// Which remote coordinator the banner was last made for (its name and access level), or
    /// `None` for this computer. A change starts the banner over.
    remote_key: Option<String>,
    /// The shader-cache card on the Overview page.
    shader_card: shader_cache::CardTracker,
}

/// The status pill's text and level, and whether the coordinator runs and is busy, for a state
/// of the coordinator that is shown (`remote`: another computer's).
const fn pill(state: &CoordinatorState, remote: bool) -> (&'static str, i32, bool, bool) {
    match state {
        CoordinatorState::Starting if remote => ("Connecting…", 1, false, true),
        CoordinatorState::Starting => ("Starting…", 1, false, true),
        CoordinatorState::Running if remote => ("Running · remote", 0, true, false),
        CoordinatorState::Running => ("Running", 0, true, false),
        CoordinatorState::Stopping => ("Stopping…", 1, false, true),
        CoordinatorState::Stopped => ("Stopped", 3, false, false),
        CoordinatorState::Error(_) => ("Problem", 2, false, false),
    }
}

/// Shows the selected coordinator's state in the shell, and starts the local coordinator once
/// the first-run wizard has closed with the folder set up.
fn refresh_shell(
    window: &AppWindow,
    local: &LocalBackend,
    backend: &ActiveBackend,
    folder: &CoordinatorFolder,
    tracker: &mut ShellTracker,
) {
    if tracker.wizard_pending && !window.global::<WizardState>().get_open() {
        tracker.wizard_pending = false;
        if !folder.is_first_run() {
            start_after_wizard(local, folder);
        }
    }
    if tracker.poll_in == 0 {
        tracker.poll_in = SETTINGS_POLL_EVERY - 1;
        apply_high_contrast(window, window_settings().high_contrast);
    } else {
        tracker.poll_in -= 1;
    }
    let shown = backend.get();
    let status = shown.status();
    let caps = shown.capabilities();
    let shell = window.global::<ShellState>();
    let remote = shell.get_remote();
    let (text, level, running, busy) = pill(&status.state, remote);
    // While the graphics card builds its shader cache the pill says so, whichever page is shown.
    if shader_cache::building(&status) {
        shell.set_status_text(shader_cache::PILL_TEXT.into());
        shell.set_status_level(1);
    } else {
        shell.set_status_text(text.into());
        shell.set_status_level(level);
    }
    shell.set_running(running);
    shell.set_busy(busy);
    shell.set_can_pair(running && caps.issue_tokens);
    if remote {
        // A remote coordinator keeps the name the switcher gave it until its status has one.
        if !status.name.trim().is_empty() {
            shell.set_coordinator_name(status.name.as_str().into());
        }
        shell.set_denied_reason(caps.denied_reason().into());
    } else {
        shell.set_coordinator_name(status.name.as_str().into());
    }
    shell.set_devices_badge(badge(status.devices_online()).into());
    shell.set_jobs_badge(badge(status.jobs_running() + status.jobs_queued()).into());
    if let Some(tray) = &tracker.tray {
        // The tray speaks for this computer, whichever coordinator the window shows.
        if remote {
            let own = local.status();
            let (own_text, ..) = pill(&own.state, false);
            tray.set_status(
                &tray_status_line(own_text, own.devices_online()),
                own.state.is_running() && local.capabilities().issue_tokens,
            );
        } else {
            tray.set_status(
                &tray_status_line(text, status.devices_online()),
                shell.get_can_pair(),
            );
        }
    }

    let key = remote.then(|| format!("{}|{:?}", shell.get_coordinator_name(), caps.role));
    if tracker.remote_key != key {
        tracker.remote_key = key;
        tracker.last = None;
        clear_own_banner(&shell, tracker);
        // Another coordinator has its own build: the card starts over.
        tracker.shader_card = shader_cache::CardTracker::default();
    }
    let subject = if remote {
        shell.get_coordinator_name().to_string()
    } else {
        "This computer".to_string()
    };
    shader_cache::apply(
        window,
        &mut tracker.shader_card,
        status.shader_cache,
        &subject,
        Instant::now(),
    );
    if tracker.last.as_ref() != Some(&status.state) {
        if remote {
            let name = shell.get_coordinator_name().to_string();
            remote_banner(&shell, tracker, &name, caps.role, &status.state);
        } else {
            match &status.state {
                CoordinatorState::Starting => set_own_banner(&shell, tracker, STARTING_BANNER, 0),
                CoordinatorState::Error(reason) => {
                    let mut text = format!("The coordinator could not start: {reason}");
                    if port_in_use(reason) {
                        text.push_str(PORT_IN_USE_HINT);
                    }
                    set_own_banner(&shell, tracker, &text, 2);
                }
                CoordinatorState::Running
                | CoordinatorState::Stopping
                | CoordinatorState::Stopped => {
                    clear_own_banner(&shell, tracker);
                }
            }
        }
        tracker.last = Some(status.state);
    }
}

/// The banner while a remote coordinator is shown: who is managed and with which access level,
/// or why it is not reachable.
fn remote_banner(
    shell: &ShellState<'_>,
    tracker: &mut ShellTracker,
    name: &str,
    role: AccessRole,
    state: &CoordinatorState,
) {
    match state {
        CoordinatorState::Starting => {
            set_own_banner(shell, tracker, &format!("Connecting to {name}…"), 0);
        }
        CoordinatorState::Running => set_own_banner(
            shell,
            tracker,
            &format!("Managing {name} remotely · {}", role_word(role)),
            0,
        ),
        CoordinatorState::Error(reason) => {
            set_own_banner(shell, tracker, &format!("Cannot reach {name}: {reason}"), 2);
        }
        CoordinatorState::Stopping | CoordinatorState::Stopped => {
            set_own_banner(shell, tracker, &format!("{name} is not running."), 1);
        }
    }
}

/// "Admin" or "Monitor", the word the remote banner ends with.
const fn role_word(role: AccessRole) -> &'static str {
    match role {
        AccessRole::Admin => "Admin",
        AccessRole::Monitor => "Monitor",
        AccessRole::Local => "This computer",
    }
}

/// A sidebar counter's text: empty (hidden) for zero.
fn badge(count: usize) -> String {
    if count == 0 {
        String::new()
    } else {
        count.to_string()
    }
}

/// The tray menu's status line: the state, and while running how many devices are online.
fn tray_status_line(state: &str, devices: usize) -> String {
    match (state, devices) {
        (_, 0) => state.to_owned(),
        (_, 1) => format!("{state} · 1 device online"),
        (_, n) => format!("{state} · {n} devices online"),
    }
}

/// Whether a failed start's reason says a port is already in use.
fn port_in_use(reason: &str) -> bool {
    let reason = reason.to_lowercase();
    ["in use", "10048", "only one usage of each socket"]
        .iter()
        .any(|needle| reason.contains(needle))
}

/// Puts the shell's own banner up. A start banner leaves a settings notice standing; a
/// problem replaces it.
fn set_own_banner(shell: &ShellState<'_>, tracker: &mut ShellTracker, text: &str, tone: i32) {
    if tone < 2 && tracker.notice.as_ref() == Some(&shell.get_banner_text()) {
        return;
    }
    let text = SharedString::from(text);
    shell.set_banner_text(text.clone());
    shell.set_banner_tone(tone);
    tracker.own_banner = Some(text);
}

/// Takes the shell's own banner down, unless something else has replaced it.
fn clear_own_banner(shell: &ShellState<'_>, tracker: &mut ShellTracker) {
    if let Some(own) = tracker.own_banner.take()
        && shell.get_banner_text() == own
    {
        shell.set_banner_text(SharedString::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_hides_zero() {
        assert_eq!(badge(0), "");
        assert_eq!(badge(3), "3");
    }

    #[test]
    fn active_backend_switches_for_every_clone() {
        let first: Arc<dyn CoordinatorBackend> = LocalBackend::new();
        let second: Arc<dyn CoordinatorBackend> = LocalBackend::new();
        let active = ActiveBackend::new(Arc::clone(&first));
        let clone = active.clone();
        assert!(Arc::ptr_eq(&clone.get(), &first));
        active.set(Arc::clone(&second));
        assert!(Arc::ptr_eq(&clone.get(), &second));
    }

    #[test]
    fn options_come_from_the_global_flags() {
        let flags = GlobalFlags {
            headless: false,
            minimized: true,
            config: Some(PathBuf::from("c.toml")),
        };
        let options = GuiOptions::from(&flags);
        assert!(options.minimized);
        assert_eq!(options.config, Some(PathBuf::from("c.toml")));
    }

    #[test]
    fn closing_quits_hides_or_minimizes() {
        assert_eq!(close_action(false, true), CloseAction::Quit);
        assert_eq!(close_action(false, false), CloseAction::Quit);
        assert_eq!(close_action(true, true), CloseAction::Hide);
        assert_eq!(close_action(true, false), CloseAction::Minimize);
    }

    #[test]
    fn the_tray_status_line_counts_devices() {
        assert_eq!(tray_status_line("Stopped", 0), "Stopped");
        assert_eq!(tray_status_line("Running", 0), "Running");
        assert_eq!(tray_status_line("Running", 1), "Running · 1 device online");
        assert_eq!(tray_status_line("Running", 4), "Running · 4 devices online");
    }

    #[test]
    fn the_pill_says_remote_only_for_a_remote_coordinator() {
        assert_eq!(pill(&CoordinatorState::Running, false).0, "Running");
        assert_eq!(pill(&CoordinatorState::Running, true).0, "Running · remote");
        assert_eq!(pill(&CoordinatorState::Starting, true).0, "Connecting…");
        assert_eq!(pill(&CoordinatorState::Starting, false).0, "Starting…");
        assert!(pill(&CoordinatorState::Starting, true).3);
        assert!(!pill(&CoordinatorState::Stopped, true).2);
    }

    #[test]
    fn the_remote_banner_names_the_access_level() {
        assert_eq!(role_word(AccessRole::Admin), "Admin");
        assert_eq!(role_word(AccessRole::Monitor), "Monitor");
    }

    #[test]
    fn a_busy_port_is_recognized_in_every_wording() {
        assert!(port_in_use("Address already in use (os error 98)"));
        assert!(port_in_use(
            "Only one usage of each socket address (protocol/network address/port) is normally permitted. (os error 10048)"
        ));
        assert!(!port_in_use("the certificate folder is missing"));
    }
}
