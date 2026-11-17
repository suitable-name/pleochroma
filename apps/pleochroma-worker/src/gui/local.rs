//! The coordinator that runs inside this process, as a [`CoordinatorBackend`], and the
//! folder it keeps its files in.
//!
//! [`LocalBackend`] owns the `serve` start-up, run and shutdown. The window creates it before
//! the coordinator exists (the first-run wizard runs first), starts it with
//! [`LocalBackend::start`], and polls it: while `serve` is still starting, [`status`] reports
//! `Starting` and [`StartingServe::poll`] is checked on every call. Pages see one
//! [`CoordinatorBackend`] and never care which phase it is in.
//!
//! Events: the backend keeps its own subscriber list, so [`events`] works from the first
//! moment and survives a restart. While a `serve` runs, a thread forwards its events here.
//!
//! [`status`]: CoordinatorBackend::status
//! [`events`]: CoordinatorBackend::events

use crate::{
    cli::ServeArgs,
    control::{
        backend::{
            Capabilities, CoordinatorBackend, CoordinatorEvent, CoordinatorState,
            CoordinatorStatus, Fingerprint, IssuedToken, JobId, LogLine, PeerRole, SettingsPatch,
            TokenId, WorkerId,
        },
        status as control_status,
    },
    pki,
    serve::{DEFAULT_SHUTDOWN_DEADLINE, ServeHandle, StartingServe, start_background},
};
use pleochroma_net::messages::PROTOCOL_VERSION;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    },
    time::{Duration, Instant},
};

/// What a method answers while the coordinator is not running.
const NOT_RUNNING: &str = "The coordinator is not running. Start it first.";

/// What a restart answers while the coordinator folder is not open (the first-run wizard).
const NO_FOLDER: &str = "The coordinator has no folder yet. Finish the setup first.";

/// How long a restart waits for a shutdown that is already going before it starts again.
const STOP_WAIT: Duration = Duration::from_secs(30);

/// How many events one subscriber may have waiting before newer ones are dropped for it.
const EVENT_QUEUE: usize = 1024;

/// The name of the settings file inside the per-user configuration folder.
pub const SETTINGS_FILE: &str = "coordinator.toml";

/// The coordinator folder the window opened, set once by [`set_folder`].
static FOLDER: OnceLock<CoordinatorFolder> = OnceLock::new();

/// Records the coordinator folder for the whole window (called once at start-up).
pub fn set_folder(folder: CoordinatorFolder) {
    let _ = FOLDER.set(folder);
}

/// The coordinator folder of this run: where the settings file and the certificate folder
/// are. `None` before the window has opened it. The wizard writes into it.
#[must_use]
pub fn folder() -> Option<&'static CoordinatorFolder> {
    FOLDER.get()
}

/// Where a coordinator keeps its settings file and its data.
///
/// # First run
///
/// A run is the **first run** when the settings file does not exist **and** the data folder
/// has no certificate authority (`pki/ca.pem`). A headless setup that was moved here (the
/// wizard's "Use an existing coordinator folder") has a CA, so it is not a first run. The
/// wizard ends by writing the settings file, which ends the first run.
///
/// # Layout
///
/// - Settings file: `%APPDATA%\Pleochroma\coordinator.toml`,
///   `~/Library/Application Support/Pleochroma/coordinator.toml` or
///   `$XDG_CONFIG_HOME/pleochroma/coordinator.toml` (`~/.config/pleochroma`).
/// - Data folder (`pki/`, `jobs/`, `asset-cache/`, `logs/`): `%LOCALAPPDATA%\Pleochroma`,
///   `~/Library/Application Support/Pleochroma` or `$XDG_DATA_HOME/pleochroma`
///   (`~/.local/share/pleochroma`).
/// - With `--config <file>` the file is the settings file and its folder is the data folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorFolder {
    settings_file: PathBuf,
    data_dir: PathBuf,
}

impl CoordinatorFolder {
    /// The folder for this user, or the one `--config` names. `None` when the platform
    /// gives no home or application-data folder.
    #[must_use]
    pub fn locate(config_override: Option<&Path>) -> Option<Self> {
        if let Some(file) = config_override {
            let dir = file
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
            return Some(Self::new(file.to_path_buf(), dir));
        }
        let config = user_config_dir()?;
        let data = user_data_dir()?;
        Some(Self::new(config.join(SETTINGS_FILE), data))
    }

    /// A folder with explicit locations (the wizard's "existing folder" choice, tests).
    #[must_use]
    pub const fn new(settings_file: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            settings_file,
            data_dir,
        }
    }

    /// The settings file (it may not exist yet).
    #[must_use]
    pub fn settings_file(&self) -> &Path {
        &self.settings_file
    }

    /// The data folder.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The certificate folder: `<data>/pki`, where the data folder is the one the settings
    /// chose (`settings::data_dir()`), or this folder's own data folder before they are loaded.
    #[must_use]
    pub fn pki_dir(&self) -> PathBuf {
        self.pki_under(crate::settings::data_dir().as_deref())
    }

    /// `<data>/pki` for the data folder `chosen`, or this folder's own when there is none.
    fn pki_under(&self, chosen: Option<&Path>) -> PathBuf {
        chosen.unwrap_or(&self.data_dir).join("pki")
    }

    /// Whether nothing has been set up yet (see the type's first-run rule).
    #[must_use]
    pub fn is_first_run(&self) -> bool {
        self.first_run_under(crate::settings::data_dir().as_deref())
    }

    /// [`Self::is_first_run`] for the data folder `chosen` (or this folder's own).
    fn first_run_under(&self, chosen: Option<&Path>) -> bool {
        !self.settings_file.exists() && !self.pki_under(chosen).join(pki::CA_CERT_FILE).exists()
    }
}

/// The per-user configuration folder of the platform, with the product folder added.
fn user_config_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Pleochroma"))
    } else if cfg!(target_os = "macos") {
        home_dir().map(|h| h.join("Library/Application Support/Pleochroma"))
    } else if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(PathBuf::from(xdg).join("pleochroma"))
    } else {
        home_dir().map(|h| h.join(".config/pleochroma"))
    }
}

/// The per-user data folder of the platform, with the product folder added.
fn user_data_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Pleochroma"))
    } else if cfg!(target_os = "macos") {
        home_dir().map(|h| h.join("Library/Application Support/Pleochroma"))
    } else if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(PathBuf::from(xdg).join("pleochroma"))
    } else {
        home_dir().map(|h| h.join(".local/share/pleochroma"))
    }
}

/// The user's home folder.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(target_os = "windows") {
        "USERPROFILE"
    } else {
        "HOME"
    })
    .filter(|v| !v.is_empty())
    .map(PathBuf::from)
}

/// Where the coordinator is in its life.
enum Phase {
    /// Never started in this run (the first-run wizard is open).
    Idle,
    /// `serve` is starting on its own thread.
    Starting(StartingServe),
    /// `serve` is running.
    Running(Arc<ServeHandle>),
    /// A shutdown is in progress.
    Stopping,
    /// Shut down; it can be started again.
    Stopped,
    /// The last start failed, with the reason in plain words.
    Failed(String),
}

/// The operating system's sleep blocker (nothing on a system without one).
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
type KeepAwakeGuard = crate::platform::KeepAwake;
/// The operating system's sleep blocker (nothing on a system without one).
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
type KeepAwakeGuard = ();

/// How long a failed attempt to take the sleep blocker is remembered before the next one.
const KEEP_AWAKE_RETRY: Duration = Duration::from_secs(60);

/// The sleep blocker and what is known about taking it.
#[derive(Default)]
struct AwakeState {
    /// The blocker, held while the keep-awake setting is on and the coordinator is busy.
    guard: Option<KeepAwakeGuard>,
    /// True while a thread is taking the blocker (it can take a moment), so only one does.
    acquiring: bool,
    /// When the last attempt failed (no `systemd-inhibit`, logind refused): no new attempt for
    /// [`KEEP_AWAKE_RETRY`].
    failed_at: Option<Instant>,
}

/// Takes the operating system's sleep blocker; `None` when it cannot be had. It can take a
/// moment (Linux starts a helper process), so never call it on the UI thread.
fn acquire_awake() -> Option<KeepAwakeGuard> {
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    {
        crate::platform::KeepAwake::acquire("The coordinator is serving devices")
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// The pieces the stop thread and the event thread share with the backend.
struct Shared {
    phase: Mutex<Phase>,
    subscribers: Mutex<Vec<SyncSender<CoordinatorEvent>>>,
    /// Raised each time a `serve` stops being the current one, so its event thread ends.
    generation: AtomicU64,
    /// The sleep blocker, held while the keep-awake setting is on and the coordinator is busy.
    awake: Mutex<AwakeState>,
}

impl Shared {
    /// The running `serve`, if there is one.
    fn running_handle(&self) -> Option<Arc<ServeHandle>> {
        let phase = self.phase.lock().unwrap_or_else(PoisonError::into_inner);
        let handle = if let Phase::Running(handle) = &*phase {
            Some(Arc::clone(handle))
        } else {
            None
        };
        drop(phase);
        handle
    }

    /// Whether the coordinator was never started in this run.
    fn is_idle(&self) -> bool {
        let phase = self.phase.lock().unwrap_or_else(PoisonError::into_inner);
        let idle = matches!(*phase, Phase::Idle);
        drop(phase);
        idle
    }

    /// Whether a shutdown is going on.
    fn is_stopping(&self) -> bool {
        let phase = self.phase.lock().unwrap_or_else(PoisonError::into_inner);
        let stopping = matches!(*phase, Phase::Stopping);
        drop(phase);
        stopping
    }

    /// Whether `serve` is starting or running.
    fn is_active(&self) -> bool {
        let phase = self.phase.lock().unwrap_or_else(PoisonError::into_inner);
        let active = matches!(*phase, Phase::Starting(_) | Phase::Running(_));
        drop(phase);
        active
    }

    /// Sends `event` to every subscriber and forgets the ones that went away.
    fn broadcast(&self, event: &CoordinatorEvent) {
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|tx| {
                // A full queue (a reader that fell behind) drops this event; only a reader that
                // is gone is forgotten.
                !matches!(
                    tx.try_send(event.clone()),
                    Err(mpsc::TrySendError::Disconnected(_))
                )
            });
    }

    /// Records what taking the sleep blocker came to: the blocker, or the time of the failure.
    fn finish_acquire(&self, taken: Option<KeepAwakeGuard>) {
        let mut state = self.awake.lock().unwrap_or_else(PoisonError::into_inner);
        state.acquiring = false;
        if taken.is_some() {
            state.failed_at = None;
            state.guard = taken;
        } else {
            state.failed_at = Some(Instant::now());
        }
    }

    /// Sets the phase and announces the new state.
    fn set_phase(&self, phase: Phase) {
        let state = state_of(&phase);
        *self.phase.lock().unwrap_or_else(PoisonError::into_inner) = phase;
        self.broadcast(&CoordinatorEvent::StateChanged(state));
    }
}

/// The public state of a phase.
fn state_of(phase: &Phase) -> CoordinatorState {
    match phase {
        Phase::Idle | Phase::Stopped => CoordinatorState::Stopped,
        Phase::Starting(_) => CoordinatorState::Starting,
        Phase::Running(_) => CoordinatorState::Running,
        Phase::Stopping => CoordinatorState::Stopping,
        Phase::Failed(reason) => CoordinatorState::Error(reason.clone()),
    }
}

/// The coordinator of this process.
pub struct LocalBackend {
    shared: Arc<Shared>,
}

impl LocalBackend {
    /// A backend with nothing running: state `Stopped` until [`Self::start`].
    #[must_use]
    pub fn new() -> Arc<Self> {
        let backend = Arc::new(Self {
            shared: Arc::new(Shared {
                phase: Mutex::new(Phase::Idle),
                subscribers: Mutex::new(Vec::new()),
                generation: AtomicU64::new(0),
                awake: Mutex::new(AwakeState::default()),
            }),
        });
        // Every saved change of the settings (from this window or a remote administrator) is
        // announced as a `SettingsChanged` event. While `serve` runs it goes into its event
        // hub, so remote administrators see it and the forwarding thread hands it to the local
        // subscribers; otherwise the local subscribers get it directly. A weak handle, so the
        // hook never keeps the backend alive.
        let announcer = Arc::downgrade(&backend.shared);
        crate::settings::set_change_hook(move |_| {
            let Some(shared) = announcer.upgrade() else {
                return;
            };
            let event = CoordinatorEvent::SettingsChanged {
                summary: crate::settings::last_change_summary(),
            };
            shared.running_handle().map_or_else(
                || shared.broadcast(&event),
                |handle| handle.control.hub().publish(&event),
            );
        });
        // A remote administrator's Restart is carried out by this window's backend, which
        // owns stopping and starting `serve`. A weak handle, so the hook never keeps it alive.
        let restarter = Arc::downgrade(&backend);
        crate::admin::restart::set_hook(move || {
            restarter.upgrade().map_or_else(
                || Err("This computer's coordinator window is closed.".to_string()),
                |local| CoordinatorBackend::restart(&*local),
            )
        });
        backend
    }

    /// Starts `serve` with `args` on its own thread and returns at once: the state becomes
    /// `Starting`, then `Running` or `Error`. Does nothing while it is starting or running.
    pub fn start(&self, args: ServeArgs) {
        {
            let phase = self
                .shared
                .phase
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if matches!(
                *phase,
                Phase::Starting(_) | Phase::Running(_) | Phase::Stopping
            ) {
                return;
            }
        }
        self.shared
            .set_phase(Phase::Starting(start_background(args)));
    }

    /// Stops the coordinator on a thread of its own: the state becomes `Stopping`, then
    /// `Stopped`. A start that is still going is cancelled. Does nothing when stopped.
    pub fn stop(&self) {
        let Some(previous) = self.begin_stop() else {
            return;
        };
        let shared = Arc::clone(&self.shared);
        let spawned = std::thread::Builder::new()
            .name("coordinator-stop".to_owned())
            .spawn(move || finish_stop(&shared, previous));
        if let Err(e) = spawned {
            tracing::warn!("could not start the shutdown thread: {e}");
            self.shared.set_phase(Phase::Stopped);
        }
    }

    /// Stops the coordinator and waits (at most [`DEFAULT_SHUTDOWN_DEADLINE`]) for it: the
    /// last thing the window does before the process exits.
    pub fn shutdown_blocking(&self) {
        if let Some(previous) = self.begin_stop() {
            finish_stop(&self.shared, previous);
        }
    }

    /// Moves a starting or running coordinator to `Stopping` and returns what it was;
    /// `None` when there is nothing to stop.
    fn begin_stop(&self) -> Option<Phase> {
        let mut phase = self
            .shared
            .phase
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !matches!(*phase, Phase::Running(_) | Phase::Starting(_)) {
            return None;
        }
        let previous = std::mem::replace(&mut *phase, Phase::Stopping);
        drop(phase);
        self.shared
            .broadcast(&CoordinatorEvent::StateChanged(CoordinatorState::Stopping));
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        Some(previous)
    }

    /// The current state; also moves a finished start on to `Running` or `Error`.
    #[must_use]
    pub fn state(&self) -> CoordinatorState {
        let mut phase = self
            .shared
            .phase
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let changed = if let Phase::Starting(starting) = &mut *phase {
            match starting.poll() {
                None => return CoordinatorState::Starting,
                Some(Ok(handle)) => {
                    let handle = Arc::new(handle);
                    self.forward_events(&handle);
                    *phase = Phase::Running(handle);
                }
                Some(Err(reason)) => {
                    tracing::warn!("the coordinator could not start: {reason}");
                    *phase = Phase::Failed(reason);
                }
            }
            true
        } else {
            false
        };
        let state = state_of(&phase);
        drop(phase);
        if changed {
            self.shared
                .broadcast(&CoordinatorEvent::StateChanged(state.clone()));
        }
        state
    }

    /// Starts the thread that hands a running `serve`'s events to the subscribers.
    fn forward_events(&self, handle: &ServeHandle) {
        let events = handle.events();
        let shared = Arc::clone(&self.shared);
        let generation = shared.generation.load(Ordering::Acquire);
        let spawned = std::thread::Builder::new()
            .name("coordinator-events".to_owned())
            .spawn(move || {
                while shared.generation.load(Ordering::Acquire) == generation {
                    match events.recv_timeout(Duration::from_millis(500)) {
                        Ok(event) => shared.broadcast(&event),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("could not start the event thread: {e}");
        }
    }

    /// Holds the sleep blocker while the keep-awake setting is on and devices are connected
    /// or jobs run, and lets go of it otherwise. Called from every status poll, on the UI
    /// thread, so taking the blocker (which can start a helper process) runs on a thread of its
    /// own, and a failed attempt is remembered for [`KEEP_AWAKE_RETRY`].
    fn update_keep_awake(&self, status: &CoordinatorStatus) {
        let wanted = wants_awake(crate::settings::current().keep_awake, status);
        let mut state = self
            .shared
            .awake
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !wanted {
            let released = state.guard.take();
            drop(state);
            drop(released);
            return;
        }
        let waiting = state
            .failed_at
            .is_some_and(|failed| failed.elapsed() < KEEP_AWAKE_RETRY);
        if state.guard.is_some() || state.acquiring || waiting {
            return;
        }
        state.acquiring = true;
        drop(state);
        let shared = Arc::clone(&self.shared);
        let spawned = std::thread::Builder::new()
            .name("keep-awake".to_owned())
            .spawn(move || {
                let taken = acquire_awake();
                shared.finish_acquire(taken);
            });
        if let Err(e) = spawned {
            tracing::warn!("could not start the keep-awake thread: {e}");
            self.shared.finish_acquire(None);
        }
    }

    /// Whether the coordinator was never started in this run (the first-run wizard is open).
    fn never_started(&self) -> bool {
        self.shared.is_idle()
    }

    /// Waits (at most [`STOP_WAIT`]) until no shutdown is going on.
    fn wait_while_stopping(&self) {
        let give_up = Instant::now() + STOP_WAIT;
        while Instant::now() < give_up && self.shared.is_stopping() {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The restart itself, on the calling thread: waits out a shutdown that is going, stops
    /// the coordinator (waiting for it to be down, so its ports are free) and only then
    /// starts it with `args`. The state goes `Stopping`, `Stopped`, `Starting`, `Running`.
    fn restart_with(&self, args: ServeArgs) {
        self.wait_while_stopping();
        self.shutdown_blocking();
        self.start(args);
    }

    /// Builds the arguments from the settings as they are now and restarts on a thread of its
    /// own. The arguments are built first, so settings that cannot start a coordinator are
    /// refused here and the coordinator keeps running.
    fn spawn_restart(&self) -> Result<(), String> {
        let folder = folder().ok_or_else(|| NO_FOLDER.to_owned())?;
        let args = crate::settings::current().serve_args(folder.data_dir())?;
        let this = Self {
            shared: Arc::clone(&self.shared),
        };
        std::thread::Builder::new()
            .name("coordinator-restart".to_owned())
            .spawn(move || this.restart_with(args))
            .map(|_| ())
            .map_err(|error| format!("The coordinator could not restart: {error}"))
    }

    /// Applies a setting that only a restart can take (for example switching remote administration
    /// off, which must close the open sessions).
    ///
    /// Restarts a starting or running coordinator at once with the settings as they are now, and
    /// returns without waiting for it. A coordinator that is stopped, or never started, has nothing
    /// to restart: the settings are used when it starts.
    ///
    /// # Errors
    ///
    /// A message in plain words when the settings cannot start a coordinator (the running one
    /// is left alone) or the restart thread cannot start.
    pub fn restart_now(&self) -> Result<(), String> {
        if !self.shared.is_active() {
            return Ok(());
        }
        self.spawn_restart()
    }

    /// The running `serve`, or the plain-words refusal.
    fn running(&self) -> Result<Arc<ServeHandle>, String> {
        let _ = self.state();
        self.shared
            .running_handle()
            .ok_or_else(|| NOT_RUNNING.to_owned())
    }
}

/// Whether the computer should stay awake: the setting is on and the coordinator runs with a
/// device connected or a job waiting or rendering.
fn wants_awake(keep_awake: bool, status: &CoordinatorStatus) -> bool {
    keep_awake
        && status.state.is_running()
        && (status.devices.iter().any(|device| device.online)
            || status.jobs_running() + status.jobs_queued() > 0)
}

/// Ends what [`LocalBackend::begin_stop`] took out of the phase, then reports `Stopped`.
fn finish_stop(shared: &Shared, previous: Phase) {
    match previous {
        Phase::Running(handle) => {
            let _ = handle.shutdown(DEFAULT_SHUTDOWN_DEADLINE);
        }
        Phase::Starting(starting) => {
            starting.cancel();
            // A start that finished just before the cancel arrived still has to be stopped.
            if let Ok(handle) = starting.wait() {
                let _ = handle.shutdown(DEFAULT_SHUTDOWN_DEADLINE);
            }
        }
        Phase::Idle | Phase::Stopping | Phase::Stopped | Phase::Failed(_) => {}
    }
    shared.set_phase(Phase::Stopped);
}

impl CoordinatorBackend for LocalBackend {
    fn status(&self) -> CoordinatorStatus {
        let state = self.state();
        let handle = self.shared.running_handle();
        let status = handle.map_or_else(
            || {
                CoordinatorStatus::minimal(
                    control_status::configured_display_name(),
                    env!("CARGO_PKG_VERSION"),
                    PROTOCOL_VERSION,
                    state,
                )
            },
            |handle| control_status::status(&handle),
        );
        self.update_keep_awake(&status);
        status
    }

    fn events(&self) -> Receiver<CoordinatorEvent> {
        let (tx, rx) = mpsc::sync_channel(EVENT_QUEUE);
        self.shared
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tx);
        rx
    }

    fn logs(&self, min: tracing::Level, limit: usize) -> Vec<LogLine> {
        crate::settings::log::recent(min, limit)
    }

    fn issue_token(&self, role: PeerRole, default_name: &str) -> Result<IssuedToken, String> {
        self.running()?.issue_token(role, default_name)
    }

    fn cancel_token(&self, id: TokenId) {
        if let Ok(handle) = self.running() {
            handle.cancel_token(id);
        }
    }

    fn revoke(&self, fp: &Fingerprint) -> Result<(), String> {
        self.running()?.revoke(fp)
    }

    fn rename(&self, fp: &Fingerprint, name: &str) -> Result<(), String> {
        self.running()?.rename(fp, name)
    }

    fn cancel_job(&self, id: JobId) -> Result<(), String> {
        self.running()?.cancel_job(id)
    }

    fn delete_job(&self, id: JobId) -> Result<(), String> {
        self.running()?.delete_job(id)
    }

    fn disconnect_worker(&self, id: WorkerId) -> Result<(), String> {
        self.running()?.disconnect_worker(id)
    }

    /// Validates and saves through `crate::settings::apply_patch` (which announces the change
    /// as a `SettingsChanged` event; it works while the coordinator is stopped too), then
    /// applies live what is cheap: the whole-image knobs and the jobs per device. The rest
    /// waits for a restart.
    fn apply_settings(&self, patch: &SettingsPatch) -> Result<(), String> {
        crate::settings::apply_patch(patch)?;
        // What a running coordinator can take without a restart is applied now.
        if let Ok(handle) = self.running() {
            let _ = handle.apply_live(patch);
        }
        Ok(())
    }

    /// Stops the coordinator and starts it again with the settings as they are now
    /// (`crate::settings::current().serve_args(data_dir)`). Returns at once: the stop and the
    /// start run on a thread of their own and show as the state changes. The arguments are
    /// built first, so settings that cannot start a coordinator are refused here and the
    /// coordinator keeps running.
    fn restart(&self) -> Result<(), String> {
        if self.never_started() {
            return Err(NOT_RUNNING.to_owned());
        }
        self.spawn_restart()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::local()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{self, Command};

    #[test]
    fn config_override_makes_its_folder_the_data_folder() {
        let folder = CoordinatorFolder::locate(Some(Path::new("/srv/pw/coordinator.toml")))
            .expect("an override always locates");
        assert_eq!(
            folder.settings_file(),
            Path::new("/srv/pw/coordinator.toml")
        );
        assert_eq!(folder.data_dir(), Path::new("/srv/pw"));
        assert_eq!(folder.pki_under(None), Path::new("/srv/pw").join("pki"));
    }

    #[test]
    fn first_run_needs_neither_settings_nor_authority() {
        let dir = std::env::temp_dir().join(format!("pleochroma-first-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let folder = CoordinatorFolder::new(dir.join(SETTINGS_FILE), dir.clone());
        let pki = dir.join("pki");
        assert!(folder.first_run_under(None));
        std::fs::create_dir_all(&pki).unwrap();
        std::fs::write(pki.join(pki::CA_CERT_FILE), b"x").unwrap();
        assert!(!folder.first_run_under(None));
        std::fs::remove_file(pki.join(pki::CA_CERT_FILE)).unwrap();
        std::fs::write(
            folder.settings_file(),
            b"name = \"x\"
",
        )
        .unwrap();
        assert!(!folder.first_run_under(None));
        let chosen = dir.join("elsewhere");
        assert_eq!(folder.pki_under(Some(&chosen)), chosen.join("pki"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_idle_backend_is_stopped_and_refuses_pairing() {
        let backend = LocalBackend::new();
        assert_eq!(backend.state(), CoordinatorState::Stopped);
        assert_eq!(backend.status().state, CoordinatorState::Stopped);
        assert!(backend.issue_token(PeerRole::Viewer, "Phone").is_err());
        assert!(backend.restart().is_err(), "nothing was ever started");
        assert!(backend.capabilities().is_local());
        assert!(backend.events().try_recv().is_err());
    }

    #[test]
    fn the_computer_stays_awake_only_for_a_busy_running_coordinator() {
        let mut status = CoordinatorStatus::minimal("x", "0", 45, CoordinatorState::Running);
        assert!(!wants_awake(true, &status), "idle");
        status.devices.push({
            let mut device = pleochroma_net::admin::status::DeviceStatus::new(
                "d",
                "ab".repeat(32),
                PeerRole::Viewer,
            );
            device.online = true;
            device
        });
        assert!(wants_awake(true, &status));
        assert!(!wants_awake(false, &status), "the setting is off");
        status.state = CoordinatorState::Stopped;
        assert!(!wants_awake(true, &status));
    }

    #[test]
    fn job_and_worker_actions_say_the_coordinator_is_not_running() {
        let backend = LocalBackend::new();
        assert_eq!(
            backend.cancel_job(JobId([1; 16])),
            Err(NOT_RUNNING.to_owned())
        );
        assert_eq!(
            backend.delete_job(JobId([1; 16])),
            Err(NOT_RUNNING.to_owned())
        );
        assert_eq!(
            backend.disconnect_worker(WorkerId(1)),
            Err(NOT_RUNNING.to_owned())
        );
    }

    /// `serve` on loopback without encryption, workers, pairing or a library, on a free port.
    fn quiet_args(dir: &Path) -> ServeArgs {
        let mut words: Vec<String> = [
            "serve",
            "--insecure-no-tls",
            "--bind",
            "127.0.0.1:0",
            "--no-enroll",
            "--no-workers",
            "--db",
        ]
        .iter()
        .map(|word| (*word).to_owned())
        .collect();
        words.push(dir.join("absent.sqlite").display().to_string());
        match cli::parse(&words) {
            Ok(Command::Serve(args)) => *args,
            Ok(_) => panic!("the arguments did not parse to the serve command"),
            Err(error) => panic!("the serve arguments do not parse: {error}"),
        }
    }

    /// Polls the backend until it reports `wanted` (which also moves a finished start on).
    fn wait_for(backend: &LocalBackend, wanted: &CoordinatorState) {
        let give_up = Instant::now() + Duration::from_secs(20);
        while backend.state() != *wanted {
            assert!(Instant::now() < give_up, "never became {wanted:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-local-test-{label}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_restart_stops_before_it_starts_again() {
        let dir = temp_dir("restart");
        let backend = LocalBackend::new();
        let events = backend.events();
        backend.start(quiet_args(&dir));
        wait_for(&backend, &CoordinatorState::Running);
        let first = backend.status().addresses.viewer;
        assert!(first.is_some());

        backend.restart_with(quiet_args(&dir));
        wait_for(&backend, &CoordinatorState::Running);

        let states: Vec<CoordinatorState> = events
            .try_iter()
            .filter_map(|event| {
                if let CoordinatorEvent::StateChanged(state) = event {
                    Some(state)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            states,
            vec![
                CoordinatorState::Starting,
                CoordinatorState::Running,
                CoordinatorState::Stopping,
                CoordinatorState::Stopped,
                CoordinatorState::Starting,
                CoordinatorState::Running,
            ],
            "the old coordinator is down before the new one starts"
        );
        assert!(backend.status().addresses.viewer.is_some());

        backend.shutdown_blocking();
        assert_eq!(backend.state(), CoordinatorState::Stopped);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_running_coordinator_without_rendering_keeps_no_jobs() {
        let dir = temp_dir("nojobs");
        let backend = LocalBackend::new();
        backend.start(quiet_args(&dir));
        wait_for(&backend, &CoordinatorState::Running);

        let cancel = backend.cancel_job(JobId([2; 16])).unwrap_err();
        assert!(cancel.contains("keeps no jobs"), "{cancel}");
        let delete = backend.delete_job(JobId([2; 16])).unwrap_err();
        assert!(delete.contains("keeps no jobs"), "{delete}");
        let worker = backend.disconnect_worker(WorkerId(9)).unwrap_err();
        assert!(!worker.is_empty());

        let status = backend.status();
        assert_eq!(status.state, CoordinatorState::Running);
        assert!(status.jobs.is_empty());
        backend.shutdown_blocking();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
