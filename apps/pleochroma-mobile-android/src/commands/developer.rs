//! The Developer page: the five-tap unlock, the tracer switch, the GPU probe, the log list, the
//! viewer, e-mail and the delete confirmations.

use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};

use pleochroma_mobile_core::{
    app_state::AppState,
    diagnostics::{
        Session,
        bundle::{self, BundleError},
        probe,
        session::HeaderFacts,
        unlock::TapOutcome,
    },
    platform::OutgoingFile,
};
use slint::{ComponentHandle, Timer, TimerMode, Weak};

use crate::{AppWindow, Developer, Shell, push};

/// The delete confirmation for the ticked files.
pub const DIALOG_DELETE_SELECTED: i32 = 6001;
/// The delete confirmation for every log.
pub const DIALOG_DELETE_ALL: i32 = 6002;

/// How often the page re-reads the tracer file size and the probe while it is watched.
const WATCH_INTERVAL: Duration = Duration::from_millis(500);

/// The e-mail attachment's name.
const BUNDLE_NAME: &str = "pleochroma-logs.zip";

type Shared = Rc<RefCell<AppState>>;

thread_local! {
    /// Re-reads the live parts of the page while the page is open or the probe runs.
    static WATCH: Timer = Timer::default();
    /// The one-shot that runs after start-up (the preview flags are applied after `push::all`).
    static AFTER_START: Timer = Timer::default();
}

/// Connects the `Developer` callbacks and the info sheet's second action.
pub(super) fn wire(window: &AppWindow, state: &Shared) {
    let developer = window.global::<Developer>();

    developer.on_about_tapped(with(window, state, about_tapped));
    developer.on_page_opened(with(window, state, |window, state| {
        push::developer::push(window, &state.borrow());
        start_watch(window, state);
    }));
    developer.on_tracer_toggled({
        let (weak, state) = (window.as_weak(), Rc::clone(state));
        move |on| {
            if let Some(window) = weak.upgrade() {
                set_tracer(&window, &state, on);
            }
        }
    });
    developer.on_last_start_view(with(window, state, open_last_start));
    developer.on_probe_run(with(window, state, run_probe));
    developer.on_probe_cancel(probe::cancel);
    developer.on_probe_view_log(with(window, state, view_probe_log));
    developer.on_select_all_clicked(with(window, state, select_all));
    developer.on_log_toggled({
        let (weak, state) = (window.as_weak(), Rc::clone(state));
        move |index| {
            if let Some(window) = weak.upgrade() {
                toggle_log(&window, &state, index);
            }
        }
    });
    developer.on_log_opened({
        let (weak, state) = (window.as_weak(), Rc::clone(state));
        move |index| {
            if let Some(window) = weak.upgrade() {
                open_log_at(&window, &state, index);
            }
        }
    });
    developer.on_email_selected(with(window, state, |window, state| {
        let names = push::developer::with_state(|dev| {
            dev.order
                .iter()
                .filter(|name| dev.selected.contains(*name))
                .cloned()
                .collect::<Vec<_>>()
        });
        email(window, state, &names);
    }));
    developer.on_delete_selected(with(window, state, ask_delete_selected));
    developer.on_delete_all(with(window, state, ask_delete_all));
    developer.on_crash_in_gpu_step(with(window, state, crash_in_gpu_step));
    developer.on_viewer_copy(with(window, state, copy_viewer));
    developer.on_viewer_email(with(window, state, |window, state| {
        let name = push::developer::with_state(|dev| dev.viewer_name.clone());
        email(window, state, &[name]);
    }));

    window
        .global::<Shell>()
        .on_info_action_clicked(with(window, state, open_last_start));

    // The preview applies `--developer`, the sample logs and `--more-page` after `push::all`,
    // so the page is read again once the event loop runs, and the Last Start sheet is shown.
    let (weak, state) = (window.as_weak(), Rc::clone(state));
    AFTER_START.with(|timer| {
        timer.start(TimerMode::SingleShot, Duration::ZERO, move || {
            if let Some(window) = weak.upgrade() {
                push::developer::push(&window, &state.borrow());
                // A start straight on the log viewer (the preview's `--more-page 7`) shows the
                // newest text log, so the viewer is never empty there.
                if window.global::<Shell>().get_more_page() == 7
                    && window.global::<Developer>().get_viewer_name().is_empty()
                {
                    open_log_at(&window, &state, 0);
                }
                show_start_sheet(&window, &state);
                if probe::is_running() {
                    start_watch(&window, &state);
                }
            }
        });
    });
}

/// A callback closure that upgrades the window and passes the shared state along.
fn with(
    window: &AppWindow,
    state: &Shared,
    run: impl Fn(&AppWindow, &Shared) + 'static,
) -> impl Fn() + 'static {
    let (weak, state) = (window.as_weak(), Rc::clone(state));
    move || {
        if let Some(window) = weak.upgrade() {
            run(&window, &state);
        }
    }
}

fn session_of(state: &Shared) -> Arc<Session> {
    Arc::clone(&state.borrow().diagnostics)
}

fn refresh(window: &AppWindow, state: &Shared) {
    push::developer::push(window, &state.borrow());
}

/// The About block was tapped: count towards the unlock.
fn about_tapped(window: &AppWindow, state: &Shared) {
    let session = session_of(state);
    let outcome = session.unlock().tap(pleochroma::diag::process_ms());
    match outcome {
        TapOutcome::Nothing => {}
        TapOutcome::Countdown(left) => {
            let text = if left == 1 {
                "1 tap to developer options".to_owned()
            } else {
                format!("{left} taps to developer options")
            };
            push::toast::show(window, &text);
        }
        TapOutcome::Unlocked => {
            refresh(window, state);
            push::toast::show(window, "Developer options are on for this session");
            let shell = window.global::<Shell>();
            shell.set_developer_return(2);
            shell.set_more_page(6);
        }
        TapOutcome::AlreadyUnlocked => {
            push::toast::show(window, "Developer options are already on.");
        }
    }
}

/// The tracer switch; the header facts come from the settings, never from the GPU.
fn set_tracer(window: &AppWindow, state: &Shared, on: bool) {
    let session = session_of(state);
    let header = header_facts(&state.borrow());
    session.set_tracer(on, &header);
    push::developer::push_tracer(window, &session);
    push::developer::push_logs(window, &session);
}

fn header_facts(state: &AppState) -> HeaderFacts {
    let render = &state.settings.render;
    HeaderFacts {
        engine: if render.engine == 1 {
            "Ray-Traced"
        } else {
            "Path-Traced"
        }
        .to_owned(),
        backend: format!("{:?}", render.backend),
        quality: format!("{:?}", render.quality),
        sample_targets: format!(
            "{} live, {} export",
            render.live_spp_compact, render.export_cap_spp
        ),
        bounces: render.bounces,
        denoise: render.denoise,
    }
}

/// Runs the GPU probe: the live render stops first, then the probe thread starts.
fn run_probe(window: &AppWindow, state: &Shared) {
    if window.global::<crate::RenderModel>().get_export_running() {
        push::toast::show(window, "Not while an export runs.");
        return;
    }
    let session = session_of(state);
    state.borrow_mut().render.stop();
    match probe::start(&session) {
        Ok(()) => {
            push::developer::with_state(|dev| {
                dev.probe_started_here = true;
                dev.probe_was_running = true;
            });
            refresh(window, state);
            start_watch(window, state);
        }
        Err(message) => {
            tracing::warn!("GPU probe did not start: {message}");
            push::toast::show(window, "The GPU probe is already running.");
            refresh(window, state);
        }
    }
}

/// Opens the newest probe log in the viewer.
fn view_probe_log(window: &AppWindow, state: &Shared) {
    let session = session_of(state);
    let name = probe::snapshot().file.or_else(|| {
        session
            .store()
            .list()
            .into_iter()
            .find(|entry| entry.kind == pleochroma_mobile_core::diagnostics::store::LogKind::Probe)
            .map(|entry| entry.name)
    });
    match name {
        Some(name) => open_log(window, state, &name),
        None => push::toast::show(window, "No probe log yet."),
    }
}

/// Opens the log the Last Start finding names (the sheet's Details and the page's View).
fn open_last_start(window: &AppWindow, state: &Shared) {
    match push::developer::with_state(|dev| dev.last_start_log.clone()) {
        Some(name) => open_log(window, state, &name),
        None => push::toast::show(window, "That log is gone."),
    }
}

/// Shows the Last Start sheet once, when the start-up check made a crash entry.
fn show_start_sheet(window: &AppWindow, state: &Shared) {
    let session = session_of(state);
    if !session.take_start_notice() {
        return;
    }
    let last = &session.start_report().last_start;
    push::developer::with_state(|dev| dev.last_start_log = push::developer::last_start_log(last));
    if let Some(body) = push::developer::last_start_sheet_body(last) {
        window.global::<Shell>().invoke_show_info_with_action(
            "Last Start".into(),
            body.into(),
            "Details".into(),
        );
    }
}

/// Opens `name` in the viewer (more-page 7).
fn open_log(window: &AppWindow, state: &Shared, name: &str) {
    let session = session_of(state);
    let locked = push::developer::locked_names(&session)
        .iter()
        .any(|n| n == name);
    match session
        .store()
        .read_tail(name, push::developer::VIEWER_TAIL_BYTES)
    {
        Ok((text, whole)) => {
            push::developer::push_viewer(window, name, &text, whole, locked);
            let shell = window.global::<Shell>();
            if shell.get_destination() != 3 {
                // Switching destination also resets nothing else; More opens on its menu.
                shell.invoke_select_destination(3);
            }
            shell.set_more_page(7);
        }
        Err(error) => {
            tracing::warn!("a log could not be read: {error}");
            push::toast::show(window, "That log could not be opened.");
        }
    }
}

fn open_log_at(window: &AppWindow, state: &Shared, index: i32) {
    let name = name_at(index);
    if let Some(name) = name {
        if name.ends_with(".pb") {
            return;
        }
        open_log(window, state, &name);
    }
}

fn name_at(index: i32) -> Option<String> {
    let index = usize::try_from(index).ok()?;
    push::developer::with_state(|dev| dev.order.get(index).cloned())
}

fn toggle_log(window: &AppWindow, state: &Shared, index: i32) {
    if let Some(name) = name_at(index) {
        push::developer::with_state(|dev| {
            if !dev.selected.remove(&name) {
                dev.selected.insert(name);
            }
        });
        push::developer::push_logs(window, &session_of(state));
    }
}

/// Select all ticks the text logs; when they are all ticked it clears them. Never touches a
/// crash snapshot.
fn select_all(window: &AppWindow, state: &Shared) {
    let session = session_of(state);
    let names = bundle::select_all_names(&session.store().list());
    push::developer::with_state(|dev| {
        if !names.is_empty() && names.iter().all(|name| dev.selected.contains(name)) {
            for name in &names {
                dev.selected.remove(name);
            }
        } else {
            dev.selected.extend(names);
        }
    });
    push::developer::push_logs(window, &session);
}

/// Builds the zip and hands it to the mail app.
fn email(window: &AppWindow, state: &Shared, names: &[String]) {
    let session = session_of(state);
    let adapter = session.adapter();
    let device = bundle::device_txt(session.facts(), adapter.as_ref());
    match bundle::build(session.store(), names, &device) {
        Ok(bytes) => {
            let (to, subject, body) = bundle::mail_fields(session.facts(), adapter.as_ref());
            let file = OutgoingFile {
                suggested_name: BUNDLE_NAME.to_owned(),
                mime: "application/zip".to_owned(),
                bytes,
            };
            let platform = Arc::clone(state.borrow().platform());
            if let Err(error) = platform.compose_email(&to, &subject, &body, file) {
                tracing::warn!("the mail app could not be opened: {error:?}");
                push::toast::show(window, "No mail app could be opened.");
            }
        }
        Err(BundleError::TooLarge) => {
            push::toast::show(window, "Too large for e-mail; select fewer logs");
        }
        Err(BundleError::Empty) => push::toast::show(window, "Select at least one log."),
        Err(error) => {
            tracing::warn!("the log bundle could not be built: {error:?}");
            push::toast::show(window, "The logs could not be prepared.");
        }
    }
}

fn copy_viewer(window: &AppWindow, state: &Shared) {
    let text = push::developer::with_state(|dev| dev.viewer_raw.clone());
    let platform = Arc::clone(state.borrow().platform());
    match platform.set_clipboard_text(&text) {
        Ok(()) => push::toast::show(window, "Copied"),
        Err(error) => {
            tracing::warn!("the clipboard could not be written: {error:?}");
            push::toast::show(window, "Could not copy.");
        }
    }
}

/// Counts what a delete would remove and asks first.
fn ask_delete(window: &AppWindow, id: i32, names: Vec<String>, locked: &[String]) {
    let (keep, remove): (Vec<String>, Vec<String>) =
        names.into_iter().partition(|n| locked.contains(n));
    if remove.is_empty() {
        push::toast::show(window, "Nothing to delete.");
        return;
    }
    let snapshots = remove.iter().filter(|name| name.ends_with(".pb")).count();
    let logs = remove.len() - snapshots;
    let tracer_kept = keep.iter().any(|name| name.starts_with("tracer-"));
    let probe_kept = keep.iter().any(|name| name.starts_with("probe-"));
    let (title, body) = push::developer::delete_texts(logs, snapshots, (tracer_kept, probe_kept));
    push::developer::with_state(|dev| dev.pending_delete = remove);
    window.global::<Shell>().invoke_show_dialog(
        id,
        title.into(),
        body.into(),
        "Delete".into(),
        "Cancel".into(),
    );
}

fn ask_delete_selected(window: &AppWindow, _state: &Shared) {
    let (names, locked) = push::developer::with_state(|dev| {
        let names: Vec<String> = dev
            .order
            .iter()
            .filter(|name| dev.selected.contains(*name))
            .cloned()
            .collect();
        (names, dev.locked.clone())
    });
    ask_delete(window, DIALOG_DELETE_SELECTED, names, &locked);
}

fn ask_delete_all(window: &AppWindow, _state: &Shared) {
    let (names, locked) =
        push::developer::with_state(|dev| (dev.order.clone(), dev.locked.clone()));
    ask_delete(window, DIALOG_DELETE_ALL, names, &locked);
}

/// Answers the page's confirmations; `false` when the id is not one of them.
pub fn handle_dialog_result(window: &AppWindow, state: &Shared, id: i32, confirmed: bool) -> bool {
    if !matches!(id, DIALOG_DELETE_SELECTED | DIALOG_DELETE_ALL) {
        return false;
    }
    let pending = push::developer::with_state(|dev| std::mem::take(&mut dev.pending_delete));
    if confirmed {
        let session = session_of(state);
        let keep = push::developer::locked_names(&session);
        let removed = session.store().delete(&pending, &keep);
        tracing::info!(removed, "logs deleted");
        push::developer::with_state(|dev| {
            for name in &pending {
                dev.selected.remove(name);
            }
        });
    }
    refresh(window, state);
    true
}

/// The debug row: aborts the process in a GPU step of the selected engine.
fn crash_in_gpu_step(_window: &AppWindow, state: &Shared) {
    #[cfg(debug_assertions)]
    {
        let engine = if state.borrow().settings.render.engine == 1 {
            pleochroma::diag::Engine::RayTraced
        } else {
            pleochroma::diag::Engine::PathTraced
        };
        probe::crash_in_gpu_step(engine);
    }
    #[cfg(not(debug_assertions))]
    let _ = state;
}

/// Starts the watcher: while the page is open or the probe runs, the live parts are re-read
/// twice a second. It stops itself when neither holds.
fn start_watch(window: &AppWindow, state: &Shared) {
    let (weak, state): (Weak<AppWindow>, Shared) = (window.as_weak(), Rc::clone(state));
    WATCH.with(|timer| {
        timer.start(TimerMode::Repeated, WATCH_INTERVAL, move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if !watch_tick(&window, &state) {
                WATCH.with(Timer::stop);
            }
        });
    });
}

/// One tick of the watcher. Returns whether it should keep running.
fn watch_tick(window: &AppWindow, state: &Shared) -> bool {
    let session = session_of(state);
    let running = probe::is_running();
    let was_running =
        push::developer::with_state(|dev| std::mem::replace(&mut dev.probe_was_running, running));
    if session.take_storage_full_notice() {
        push::toast::show(window, "The tracer log stopped: the device storage is full");
    }
    if was_running && !running {
        let snapshot = probe::snapshot();
        push::toast::show(
            window,
            &format!(
                "GPU probe finished: {} of {} ok",
                snapshot.ok, snapshot.total
            ),
        );
        push::developer::with_state(|dev| dev.probe_started_here = false);
        // The renders that were held back run now.
        state.borrow_mut().render.resume_after_probe();
    }
    let page_open = window.global::<Shell>().get_more_page() == 6
        && window.global::<Shell>().get_destination() == 3;
    if page_open || running || was_running {
        refresh(window, state);
    }
    page_open || running
}
