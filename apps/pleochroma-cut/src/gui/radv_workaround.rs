//! The RADV shader workaround in the desktop app: the start-up step, the one-time question,
//! the Preferences switch and the restart both of them end in.
//!
//! Background: Mesa's RADV (the open AMD Vulkan driver) schedules the megakernel wrongly, so
//! some stones (color zones with a domain structure, oriented inclusions) render too bright
//! or with errors on the GPU. Setting `ACO_DEBUG=nosched` before the Vulkan device exists
//! makes the driver correct; see `pleochroma::renderer::gpu_driver`, which owns the value and
//! the environment step. Mesa reads the variable only when the device is created, so a
//! change of the answer needs a restart.
//!
//! The Slint side is `ui/models/radv_workaround.slint` (`RadvWorkaroundModel`), the question
//! in `components/app/window_dialogs.slint` and the Preferences "Graphics Driver" section. This
//! module is the Rust half:
//!
//! - [`apply_at_startup`] is the first thing `main` does (before any thread or GPU call): it
//!   waits for the instance we were restarted from, reads the stored answer straight from
//!   the settings file and sets the environment;
//! - [`setup_radv_workaround`] seeds the model, wires its callbacks and watches (a timer, no
//!   thread) for the GPU adapter the render thread acquires, to ask the question once;
//! - a restart is a request ([`request_restart`]) that goes through the window-close guard
//!   (unsaved changes, running plans and exports) and is carried out when the event loop has
//!   ended ([`take_pending_restart`] then [`restart_process`]): the child is started with the
//!   same arguments plus `--restarted-from=<pid>`, waits at its own start until that process
//!   is gone (so the two never hold the library database or the settings file together) and
//!   applies the stored answer through the same start-up step.

use crate::{
    MainWindow, RadvWorkaroundModel,
    settings::{
        SettingsPersister,
        model::{RadvWorkaroundChoice, should_ask_radv_workaround},
        store::peek_radv_choice,
    },
};
use pleochroma::renderer::gpu_driver::{self, WorkaroundStatus};
use slint::{ComponentHandle, Timer, TimerMode, platform::WindowEvent};
use std::{
    cell::RefCell,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tracing::{info, warn};

/// This app's folder inside the platform cache folder (`<cache>/pleochroma-cut`); Mesa's
/// shader cache of a run with the workaround lives under it.
const CACHE_APP_DIR: &str = "pleochroma-cut";

/// The argument a restarted instance is given: `--restarted-from=<pid of the old instance>`.
/// One token that starts with `-`, so `startup_file::path_from_args` skips it like any flag.
pub const RESTARTED_FROM_FLAG: &str = "--restarted-from";

/// How long a restarted instance waits for the old one to be gone before it starts anyway.
const RESTART_WAIT: Duration = Duration::from_secs(15);

/// How often the restarted instance looks whether the old one is gone.
const RESTART_POLL: Duration = Duration::from_millis(50);

/// How long (in one-second ticks) the question waits for the render thread to acquire its GPU
/// adapter before it gives up for this run.
const ADAPTER_WATCH_TICKS: u32 = 600;

// --- Start-up ------------------------------------------------------------------------------

/// The start-up step.
///
/// Call it FIRST in `main`, before the tracing subscriber, the panic hook, any thread or any GPU
/// call: it may wait for the instance this one replaces, and it changes the process environment,
/// which is only sound while the process is single-threaded.
///
/// Reads the stored answer from the settings file (`store::peek_radv_choice`: no model, never
/// fails) and, when it is `on` and the user did not set `ACO_DEBUG` themselves, sets
/// `ACO_DEBUG` and the Mesa shader-cache folder. Returns what happened, for
/// [`log_startup_status`] once a subscriber exists.
#[must_use]
pub fn apply_at_startup() -> WorkaroundStatus {
    wait_for_replaced_instance(std::env::args_os().skip(1));
    // A build without the `gpu` feature never creates a Vulkan device: nothing to work around.
    let requested = cfg!(feature = "gpu") && peek_radv_choice() == Some(RadvWorkaroundChoice::On);
    let cache_root = gpu_driver::platform_cache_dir().map(|dir| dir.join(CACHE_APP_DIR));
    gpu_driver::apply(requested, cache_root.as_deref())
}

/// Logs the outcome of [`apply_at_startup`] (info when something was set or left alone).
pub fn log_startup_status(status: &WorkaroundStatus) {
    match status {
        WorkaroundStatus::NotRequested | WorkaroundStatus::UnsupportedPlatform => {
            tracing::debug!("{}", status.log_line());
        }
        WorkaroundStatus::UserAcoDebug(_) | WorkaroundStatus::Applied(_) => {
            info!("{}", status.log_line());
        }
    }
}

// --- The instance we replace ---------------------------------------------------------------

/// The pid in a `--restarted-from=<pid>` argument among `args` (the last one wins).
fn restarted_from_pid(args: &[OsString]) -> Option<u32> {
    args.iter().rev().find_map(|arg| {
        arg.to_str()?
            .strip_prefix(RESTARTED_FROM_FLAG)?
            .strip_prefix('=')?
            .parse()
            .ok()
    })
}

/// The arguments the restarted instance gets: `args` (without the program name) with any
/// earlier `--restarted-from=...` removed, and `--restarted-from=<own_pid>` added.
fn restart_arguments(args: impl IntoIterator<Item = OsString>, own_pid: u32) -> Vec<OsString> {
    let prefix = format!("{RESTARTED_FROM_FLAG}=");
    let mut kept: Vec<OsString> = args
        .into_iter()
        .filter(|arg| !arg.to_string_lossy().starts_with(&prefix))
        .collect();
    kept.push(OsString::from(format!("{prefix}{own_pid}")));
    kept
}

/// The program to start again: the `AppImage` file when `$APPIMAGE` names one (the executable
/// inside a mounted `AppImage` disappears when it exits), else the running executable.
fn restart_program(appimage: Option<&OsStr>, current_exe: Option<PathBuf>) -> Option<PathBuf> {
    appimage
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or(current_exe)
}

/// Whether a `/proc/<pid>/stat` text describes a process that is still running: its state
/// (the letter after the last `)`, the command name may hold parentheses) is neither `Z`
/// (zombie, exited but not yet collected) nor `X` (dead). Unreadable text counts as gone.
fn stat_means_running(stat: &str) -> bool {
    stat.rfind(')')
        .and_then(|close| stat[close + 1..].trim_start().chars().next())
        .is_some_and(|state| state != 'Z' && state != 'X')
}

/// Whether process `pid` is still running. Linux only (`/proc`); elsewhere nothing is waited
/// for (the restart is offered on Linux only).
fn process_is_running(pid: u32) -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| stat_means_running(&stat))
}

/// Polls `is_running(pid)` up to `max_polls` times, calling `pause` between polls; `true`
/// when the process was gone before the polls ran out.
fn wait_for_exit(
    pid: u32,
    max_polls: u32,
    mut is_running: impl FnMut(u32) -> bool,
    mut pause: impl FnMut(),
) -> bool {
    for _ in 0..max_polls {
        if !is_running(pid) {
            return true;
        }
        pause();
    }
    !is_running(pid)
}

/// When this instance was started by a restart (`--restarted-from=<pid>` in `args`), waits
/// (at most [`RESTART_WAIT`]) until that instance is gone, so the two never hold the design
/// library database or the settings file at once. Runs before any thread starts.
fn wait_for_replaced_instance(args: impl IntoIterator<Item = OsString>) {
    let args: Vec<OsString> = args.into_iter().collect();
    let Some(pid) = restarted_from_pid(&args).filter(|pid| *pid != std::process::id()) else {
        return;
    };
    let polls = u32::try_from(RESTART_WAIT.as_millis() / RESTART_POLL.as_millis()).unwrap_or(300);
    // `eprintln!`, not `tracing`: no subscriber exists yet at this point of `main`.
    if !wait_for_exit(pid, polls, process_is_running, || {
        std::thread::sleep(RESTART_POLL);
    }) {
        eprintln!(
            "pleochroma-cut: the previous instance (pid {pid}) is still running after {} s; \
             starting anyway",
            RESTART_WAIT.as_secs()
        );
    }
}

// --- Restarting ----------------------------------------------------------------------------

/// A restart was asked for and has not been carried out or withdrawn.
static RESTART_PENDING: AtomicBool = AtomicBool::new(false);

/// True while [`request_restart`] is itself delivering the close request to the window, so
/// the close handler can tell it from the user's own click on the window's close button.
static DISPATCHING: AtomicBool = AtomicBool::new(false);

/// Asks for a restart: marks it pending and sends the main window a close request, which
/// runs the same guard as the window's close button (unsaved changes, a running rough plan
/// or export). When the guard lets the window close, the event loop ends and
/// [`take_pending_restart`] says the program must start again. A guard that is cancelled, or a
/// later close the user does themselves, withdraws the request ([`note_close_request`]).
///
/// The UI harness build only records the request: it never closes the window under test and
/// never starts a process (`harness_support::take_radv_restart_request` reads it).
pub(super) fn request_restart(ui: &MainWindow) {
    RESTART_PENDING.store(true, Ordering::SeqCst);
    if cfg!(feature = "ui-harness") {
        return;
    }
    DISPATCHING.store(true, Ordering::SeqCst);
    ui.window().dispatch_event(WindowEvent::CloseRequested);
    DISPATCHING.store(false, Ordering::SeqCst);
}

/// Called by the main window's close handler for every close request. One that did not come
/// from [`request_restart`] is the user quitting (or cancelling and quitting later): any
/// pending restart is withdrawn.
pub(super) fn note_close_request() {
    if !DISPATCHING.load(Ordering::SeqCst) {
        RESTART_PENDING.store(false, Ordering::SeqCst);
    }
}

/// Takes the pending restart request, if any. `run_gui` asks after the event loop ended.
pub fn take_pending_restart() -> bool {
    RESTART_PENDING.swap(false, Ordering::SeqCst)
}

/// Starts this program again, detached, with the same arguments.
///
/// It adds `--restarted-from=<this pid>` (the new instance waits for this one to be gone). Call
/// it after the event loop ended, as the last thing before the process exits.
///
/// The child gets this process's environment, except that what the start-up step added
/// (`ACO_DEBUG`, the Mesa shader-cache folder) is removed or restored
/// ([`gpu_driver::restart_env_edits`]): the child decides again from the stored answer.
///
/// # Errors
///
/// A message when the program path is unknown or the process could not be started.
pub fn restart_process() -> Result<(), String> {
    let appimage = std::env::var_os("APPIMAGE").filter(|path| Path::new(path).is_file());
    let program = restart_program(appimage.as_deref(), std::env::current_exe().ok())
        .ok_or_else(|| "the path of the running program is unknown".to_owned())?;
    let args = restart_arguments(std::env::args_os().skip(1), std::process::id());
    let mut command = std::process::Command::new(&program);
    command.args(&args).stdin(std::process::Stdio::null());
    for (key, value) in gpu_driver::restart_env_edits() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // Its own process group: a Ctrl+C in the terminal this was started from, or the end of
        // this process's group, must not take the new instance with it.
        command.process_group(0);
    }
    command
        .spawn()
        .map(drop)
        .map_err(|error| format!("could not start {}: {error}", program.display()))
}

// --- The question and the Preferences switch -----------------------------------------------

thread_local! {
    /// The timer that waits for the GPU adapter (UI thread only, like every Slint handle).
    static ADAPTER_WATCH: RefCell<Option<Timer>> = const { RefCell::new(None) };
}

/// Stores `choice` through the persister and brings the model in step with it.
fn answer(ui: &MainWindow, store: &SettingsPersister, choice: RadvWorkaroundChoice) {
    store.update(|s| s.settings.radv_scheduler_workaround = Some(choice));
    let model = ui.global::<RadvWorkaroundModel>();
    model.set_enabled(choice.is_on());
    model.set_prompt_pending(false);
}

/// Restarts the program for the stored answer: saves the settings now (the new instance reads
/// the file at its start), then asks for the restart.
fn restart_for_answer(ui: &MainWindow, store: &SettingsPersister) {
    store.flush();
    request_restart(ui);
}

/// Seeds `RadvWorkaroundModel` from the settings, wires its callbacks and starts watching for
/// the GPU adapter. Called once from `build_main_window`, after the settings are loaded.
pub(super) fn setup_radv_workaround(ui: &MainWindow, settings_store: &Arc<SettingsPersister>) {
    let model = ui.global::<RadvWorkaroundModel>();
    model.set_available(cfg!(target_os = "linux"));
    let stored = settings_store.snapshot().settings.radv_scheduler_workaround;
    model.set_enabled(stored == Some(RadvWorkaroundChoice::On));

    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    model.on_restart_with_workaround(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        answer(&ui, &store, RadvWorkaroundChoice::On);
        restart_for_answer(&ui, &store);
    });

    // "Not Now" stores nothing: the question comes again at the next start.
    let ui_weak = ui.as_weak();
    model.on_not_now(move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<RadvWorkaroundModel>().set_prompt_pending(false);
        }
    });

    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    model.on_dont_use(move || {
        if let Some(ui) = ui_weak.upgrade() {
            answer(&ui, &store, RadvWorkaroundChoice::Off);
        }
    });

    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    model.on_enabled_changed(move |on: bool| {
        if let Some(ui) = ui_weak.upgrade() {
            answer(&ui, &store, RadvWorkaroundChoice::from_on(on));
        }
    });

    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    model.on_restart_now(move || {
        if let Some(ui) = ui_weak.upgrade() {
            restart_for_answer(&ui, &store);
        }
    });

    if stored.is_none() {
        start_adapter_watch(ui, settings_store);
    }
}

/// Watches (once a second, on the UI thread) for the GPU adapter the render thread acquires;
/// when it is RADV, the answer is still open and the workaround is not in force anyway, the
/// question is raised. Never in the UI harness (its profile and GPU are not the user's) and
/// never in a build without the `gpu` feature (no adapter is ever acquired).
fn start_adapter_watch(ui: &MainWindow, settings_store: &Arc<SettingsPersister>) {
    if !cfg!(feature = "gpu") || cfg!(feature = "ui-harness") || !cfg!(target_os = "linux") {
        return;
    }
    let ui_weak = ui.as_weak();
    let store = Arc::clone(settings_store);
    let mut ticks = 0_u32;
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
        ticks += 1;
        let finished = gpu_driver::recorded_adapter().map_or_else(
            || ticks >= ADAPTER_WATCH_TICKS || ui_weak.upgrade().is_none(),
            |identity| {
                let ask = should_ask_radv_workaround(
                    store.snapshot().settings.radv_scheduler_workaround,
                    identity.is_radv(),
                    gpu_driver::workaround_active(),
                    cfg!(target_os = "linux"),
                );
                if ask && let Some(ui) = ui_weak.upgrade() {
                    info!(driver = %identity.describe(), "asking about the RADV scheduler workaround");
                    let model = ui.global::<RadvWorkaroundModel>();
                    model.set_driver_text(identity.describe().into());
                    model.set_prompt_pending(true);
                }
                true
            },
        );
        if finished {
            ADAPTER_WATCH.with_borrow(|slot| {
                if let Some(timer) = slot {
                    timer.stop();
                }
            });
        }
    });
    ADAPTER_WATCH.with_borrow_mut(|slot| *slot = Some(timer));
}

/// Reports a failed relaunch (the window is gone by then, so the log is the only place).
pub fn report_restart_failure(message: &str) {
    warn!("the restart did not start a new instance: {message}");
    eprintln!("pleochroma-cut: the restart did not start a new instance: {message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_restart_flag_carries_the_old_pid_and_replaces_an_earlier_one() {
        let args = restart_arguments(os(&["--log", "design.psdd"]), 4242);
        assert_eq!(args, os(&["--log", "design.psdd", "--restarted-from=4242"]));
        // A restart of a restart: the old flag goes, the new pid stays.
        let again = restart_arguments(args, 77);
        assert_eq!(again, os(&["--log", "design.psdd", "--restarted-from=77"]));
        assert_eq!(restart_arguments(os(&[]), 5), os(&["--restarted-from=5"]));
    }

    #[test]
    fn the_flag_is_read_back_and_never_taken_for_a_file() {
        let args = restart_arguments(os(&["--log"]), 4242);
        assert_eq!(restarted_from_pid(&args), Some(4242));
        assert_eq!(restarted_from_pid(&os(&["--log"])), None);
        assert_eq!(restarted_from_pid(&os(&["--restarted-from=abc"])), None);
        assert_eq!(restarted_from_pid(&os(&["--restarted-from"])), None);
        assert_eq!(restarted_from_pid(&os(&["--restarted-from=-3"])), None);
        assert_eq!(
            restarted_from_pid(&os(&["--restarted-from=1", "--restarted-from=2"])),
            Some(2),
            "the last one wins"
        );
        // The start-up file argument skips it: it starts with a dash.
        assert_eq!(
            crate::gui::startup_file::path_from_args(args),
            None,
            "the flag is not a design file"
        );
    }

    #[test]
    fn an_appimage_is_started_by_its_own_file_else_the_running_executable() {
        let exe = Some(PathBuf::from("/tmp/.mount_x/usr/bin/pleochroma-cut"));
        assert_eq!(
            restart_program(Some(OsStr::new("/home/u/Pleochroma.AppImage")), exe.clone()),
            Some(PathBuf::from("/home/u/Pleochroma.AppImage"))
        );
        assert_eq!(restart_program(None, exe.clone()), exe);
        assert_eq!(
            restart_program(Some(OsStr::new("")), exe.clone()),
            exe,
            "an empty APPIMAGE is not one"
        );
        assert_eq!(restart_program(None, None), None);
    }

    #[test]
    fn a_zombie_or_dead_process_is_gone_for_the_waiting_child() {
        assert!(stat_means_running(
            "1234 (pleochroma-cut) S 1 1234 1234 0 -1 4194560"
        ));
        assert!(stat_means_running(
            "1234 (pleochroma-cut) R 1 1234 1234 0 -1 4194560"
        ));
        assert!(
            stat_means_running("1234 (D) isk) D 1 1234"),
            "parentheses in the name"
        );
        assert!(!stat_means_running(
            "1234 (pleochroma-cut) Z 1 1234 1234 0 -1 4194560"
        ));
        assert!(!stat_means_running("1234 (pleochroma-cut) X 1 1234"));
        assert!(!stat_means_running("garbage"));
        assert!(!stat_means_running(""));
    }

    #[test]
    fn the_wait_ends_when_the_process_is_gone_or_the_polls_run_out() {
        // Gone after three looks.
        let mut looks = 0;
        let mut pauses = 0;
        let gone = wait_for_exit(
            9,
            100,
            |_| {
                looks += 1;
                looks <= 3
            },
            || pauses += 1,
        );
        assert!(gone);
        assert_eq!(pauses, 3);
        // Never gone: gives up after the polls, having paused once per poll.
        let mut pauses = 0;
        assert!(!wait_for_exit(9, 5, |_| true, || pauses += 1));
        assert_eq!(pauses, 5);
        // Already gone: no pause at all.
        let mut pauses = 0;
        assert!(wait_for_exit(9, 5, |_| false, || pauses += 1));
        assert_eq!(pauses, 0);
    }

    #[test]
    fn a_restart_request_is_withdrawn_by_a_close_the_user_did_themselves() {
        // The only test touching the two flags.
        RESTART_PENDING.store(false, Ordering::SeqCst);
        assert!(!take_pending_restart());

        RESTART_PENDING.store(true, Ordering::SeqCst);
        DISPATCHING.store(true, Ordering::SeqCst);
        note_close_request();
        DISPATCHING.store(false, Ordering::SeqCst);
        assert!(
            RESTART_PENDING.load(Ordering::SeqCst),
            "the close request the restart itself sent keeps it pending"
        );

        note_close_request();
        assert!(
            !take_pending_restart(),
            "the user closing the window later is a plain quit"
        );

        RESTART_PENDING.store(true, Ordering::SeqCst);
        assert!(take_pending_restart());
        assert!(!take_pending_restart(), "taken exactly once");
    }
}
