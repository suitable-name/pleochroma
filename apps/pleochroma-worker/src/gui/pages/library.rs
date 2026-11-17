//! The Library page.
//!
//! Shows the design library the coordinator serves (path, number of designs, read-only) from the
//! coordinator status once a second, or, in library-less mode, says in plain words that rendering
//! still works. "Choose a library…" and "Rendering only" write the `library` setting through
//! [`crate::settings::update`] and suggest a restart, because the coordinator opens its library at
//! start.

use crate::gui::{
    app::{ActiveBackend, AppWindow, LibraryState, ShellState},
    pages::picker::{self, Kind, Request},
};
use slint::{ComponentHandle, SharedString, Timer, TimerMode};
use std::{cell::RefCell, path::Path, time::Duration};

/// How often the page re-reads the status.
const REFRESH: Duration = Duration::from_secs(1);

/// The library setting saved but not yet in use.
#[derive(Clone, Debug)]
enum Pending {
    /// Nothing waits for a restart.
    Nothing,
    /// "No library": rendering only.
    NoLibrary,
    /// A chosen library file.
    Library(String),
}

thread_local! {
    /// The refresh timer; a `Timer` stops when dropped, so it lives for the UI thread.
    static REFRESH_TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
    /// The library setting saved but not yet in use.
    static PENDING: RefCell<Pending> = const { RefCell::new(Pending::Nothing) };
}

/// Connects the Library page of `window` to `backend`.
///
/// Shows the current library at once, refreshes it every second, and wires Choose a library,
/// Rendering only, Open folder and Restart now.
pub fn wire(window: &AppWindow, backend: &ActiveBackend) {
    let state = window.global::<LibraryState>();

    state.on_choose_library({
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            let request = Request::new(Kind::OpenFile, "Choose your design library")
                .filter("Design library", &["sqlite", "db"]);
            let active = backend.clone();
            let started = picker::pick(&weak, &request, move |window, path| {
                if path.is_file() {
                    save_library(window, &active, Some(path.as_path()));
                } else {
                    window
                        .global::<LibraryState>()
                        .set_error("That file cannot be opened. Pick another one.".into());
                }
            });
            if let (Err(reason), Some(window)) = (started, weak.upgrade()) {
                window.global::<LibraryState>().set_error(reason.into());
            }
        }
    });
    state.on_clear_library({
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                save_library(&window, &backend, None);
            }
        }
    });
    state.on_open_folder({
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            // The library file is on the coordinator's own computer: a remote one is not
            // opened here.
            let remote = weak
                .upgrade()
                .is_none_or(|window| window.global::<ShellState>().get_remote());
            if remote {
                return;
            }
            let status = backend.get().status();
            if let Some(folder) = status
                .library
                .path
                .as_deref()
                .and_then(|path| Path::new(path).parent())
            {
                crate::gui::util::open_folder(folder);
            }
        }
    });
    state.on_restart_now({
        let weak = window.as_weak();
        let backend = backend.clone();
        move || restart_now(&weak, &backend)
    });

    refresh(window, backend);
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, REFRESH, {
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                refresh(&window, &backend);
            }
        }
    });
    REFRESH_TIMER.with(|slot| *slot.borrow_mut() = Some(timer));
}

/// "Restart now": the shown coordinator restarts on a helper thread (a remote one can take
/// seconds to answer), and the notice goes away when it accepted.
fn restart_now(weak: &slint::Weak<AppWindow>, backend: &ActiveBackend) {
    let shown = backend.get();
    let for_thread = weak.clone();
    let spawned = std::thread::Builder::new()
        .name("library-restart".to_owned())
        .spawn(move || {
            let result = shown.restart();
            let _ = for_thread.upgrade_in_event_loop(move |window| {
                let state = window.global::<LibraryState>();
                match result {
                    Ok(()) => {
                        PENDING.with(|slot| *slot.borrow_mut() = Pending::Nothing);
                        state.set_restart_pending(false);
                        state.set_error(SharedString::new());
                    }
                    Err(reason) => state.set_error(reason.into()),
                }
            });
        });
    if let (Err(e), Some(window)) = (spawned, weak.upgrade()) {
        window
            .global::<LibraryState>()
            .set_error(format!("Could not start the work: {e}").into());
    }
}

/// Saves the library choice (`None` for rendering only) and shows the restart notice.
fn save_library(window: &AppWindow, backend: &ActiveBackend, choice: Option<&Path>) {
    let state = window.global::<LibraryState>();
    let text = choice.map(|path| path.to_string_lossy().into_owned());
    // The setting holds an optional path; `into` lets it be a `String` or a `PathBuf`.
    let stored = text.clone();
    match crate::settings::update(move |settings| settings.library = stored.map(Into::into)) {
        Ok(()) => {
            PENDING.with(|slot| {
                *slot.borrow_mut() = text.map_or(Pending::NoLibrary, Pending::Library);
            });
            state.set_error(SharedString::new());
            refresh(window, backend);
        }
        Err(reason) => state.set_error(reason.into()),
    }
}

/// Reads the status and permissions and updates the page state.
fn refresh(window: &AppWindow, backend: &ActiveBackend) {
    let backend = backend.get();
    let status = backend.status();
    let caps = backend.capabilities();
    let library = &status.library;

    let state = window.global::<LibraryState>();
    state.set_has_library(!library.is_none());
    state.set_path(library.path.clone().unwrap_or_default().into());
    state.set_designs_text(match (library.path.is_some(), library.designs) {
        (false, _) => SharedString::new(),
        (true, Some(count)) => group_digits(count).into(),
        (true, None) => "Counting\u{2026}".into(),
    });
    let word = if library.designs == Some(1) {
        "design"
    } else {
        "designs"
    };
    state.set_designs_word(word.into());
    state.set_read_only(library.read_only);
    // The library setting is a local one: only the coordinator's own computer may change it.
    state.set_can_change(caps.change_network);
    state.set_can_restart(caps.restart);
    state.set_denied_reason(if caps.change_network {
        SharedString::new()
    } else {
        caps.denied_reason().into()
    });

    // The notice stays until the running library matches what was saved. A saved change belongs
    // to this computer's coordinator, so a remote one never shows it.
    let remote = window.global::<ShellState>().get_remote();
    let pending = PENDING.with(|slot| {
        if remote {
            return Pending::Nothing;
        }
        let mut slot = slot.borrow_mut();
        let running = library.path.as_deref();
        let settled = match &*slot {
            Pending::Nothing => false,
            Pending::NoLibrary => same_library(None, running),
            Pending::Library(target) => same_library(Some(target.as_str()), running),
        };
        if settled {
            *slot = Pending::Nothing;
        }
        slot.clone()
    });
    state.set_restart_pending(!matches!(pending, Pending::Nothing));
    match pending {
        Pending::Nothing => {}
        Pending::NoLibrary => state.set_pending_text("no library, rendering only".into()),
        Pending::Library(target) => state.set_pending_text(target.into()),
    }
}

/// Whether the saved choice and the running library are the same file (or both none).
fn same_library(saved: Option<&str>, running: Option<&str>) -> bool {
    match (saved, running) {
        (None, None) => true,
        (Some(saved), Some(running)) => {
            saved == running
                || matches!(
                    (std::fs::canonicalize(saved), std::fs::canonicalize(running)),
                    (Ok(a), Ok(b)) if a == b
                )
        }
        _ => false,
    }
}

/// A count with thousands separators: 4812 becomes "4,812".
fn group_digits(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_get_thousands_separators() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(812), "812");
        assert_eq!(group_digits(4812), "4,812");
        assert_eq!(group_digits(1_234_567), "1,234,567");
    }

    #[test]
    fn the_saved_choice_matches_the_running_library() {
        assert!(same_library(None, None));
        assert!(same_library(Some("a.sqlite"), Some("a.sqlite")));
        assert!(!same_library(Some("a.sqlite"), None));
        assert!(!same_library(None, Some("a.sqlite")));
        assert!(!same_library(
            Some("no-such-a.sqlite"),
            Some("no-such-b.sqlite")
        ));
    }
}
