//! The coordinator switcher: the list under the coordinator's name at the top of the sidebar.
//!
//! It lists "This computer" and every coordinator saved by [`crate::settings::remotes`], each
//! with its access level (Full administration or Monitor only) and whether it answers, and
//! offers "Add a coordinator…" (see [`crate::gui::add_coordinator`]) and a way to forget a saved
//! one. Picking a row selects the backend every page talks to ([`ActiveBackend::set`]): the
//! coordinator in this process, or the one [`crate::gui::remote::open`] returns. The pages call
//! [`ActiveBackend::get`] on every use, so one switch reaches all of them.
//!
//! The switcher also tells the shell which kind of coordinator is shown: `ShellState.remote`
//! hides what only the local coordinator can do (Stop, Open data folder, Tools, Help another
//! coordinator, the network and backup settings), and the top bar's Disconnect comes back to
//! this computer without forgetting the pairing.
//!
//! Whether a saved remote answers is found with a short TCP connect to its addresses on a thread
//! of its own each time the list opens; the remote that is shown reports its own state.

use crate::{
    control::backend::{AccessRole, CoordinatorBackend, CoordinatorState},
    gui::{
        add_coordinator,
        app::{
            ActiveBackend, AppWindow, PairState, SettingsState, ShellState, SwitcherRow,
            SwitcherState,
        },
        local::LocalBackend,
        remote,
    },
    settings::remotes::{self, RemoteId},
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    collections::HashMap,
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

/// How long the probe waits for one address of a saved remote.
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// The pages only the local coordinator has (`ShellState.page`): Help another coordinator and
/// Tools.
const LOCAL_ONLY_PAGES: [i32; 2] = [7, 8];

/// What the switcher and its probe threads share.
struct Shared {
    backend: ActiveBackend,
    local: Arc<LocalBackend>,
    /// The id of the remote that is shown, `None` for this computer.
    active: Mutex<Option<String>>,
    /// Which saved remotes answered the last probe, by id.
    online: Mutex<HashMap<String, bool>>,
}

/// Locks a mutex and takes the data even when a thread panicked while holding it.
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Selects the coordinator the pages show. Cheap to clone; the clones share their state.
#[derive(Clone)]
pub struct Switcher {
    window: slint::Weak<AppWindow>,
    shared: Arc<Shared>,
}

/// Connects the sidebar's switcher and the top bar's Disconnect. `local` is the coordinator in
/// this process; it is what "This computer" selects.
#[must_use]
pub fn wire(window: &AppWindow, backend: &ActiveBackend, local: &Arc<LocalBackend>) -> Switcher {
    let switcher = Switcher {
        window: window.as_weak(),
        shared: Arc::new(Shared {
            backend: backend.clone(),
            local: Arc::clone(local),
            active: Mutex::new(None),
            online: Mutex::new(HashMap::new()),
        }),
    };
    set_rows(window, &switcher.shared);

    let state = window.global::<SwitcherState>();
    state.on_opened({
        let switcher = switcher.clone();
        move || {
            switcher.refresh_rows();
            switcher.probe();
        }
    });
    state.on_picked({
        let switcher = switcher.clone();
        move |id| switcher.pick(id.as_str())
    });
    state.on_add_requested({
        let switcher = switcher.clone();
        move || {
            if let Some(window) = switcher.window.upgrade() {
                add_coordinator::open(&window);
            }
        }
    });
    state.on_forget_confirmed({
        let switcher = switcher.clone();
        move || {
            if let Some(window) = switcher.window.upgrade() {
                let id = window.global::<SwitcherState>().get_forget_id();
                switcher.forget(id.as_str());
            }
        }
    });
    window.global::<ShellState>().on_disconnect({
        let switcher = switcher.clone();
        move || switcher.select_local()
    });
    switcher
}

impl Switcher {
    /// A row was picked: the id of a saved remote, empty for this computer.
    fn pick(&self, id: &str) {
        if id.is_empty() {
            self.select_local();
        } else {
            self.select_remote(id);
        }
    }

    /// Shows the coordinator in this process again.
    pub fn select_local(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        if locked(&self.shared.active).take().is_none() {
            return;
        }
        close_dialogs(&window);
        let local: Arc<dyn CoordinatorBackend> = self.shared.local.clone();
        self.shared.backend.set(local);
        let shell = window.global::<ShellState>();
        shell.set_remote(false);
        shell.set_coordinator_place("This computer".into());
        shell.set_denied_reason(SharedString::new());
        set_rows(&window, &self.shared);
    }

    /// Shows the saved coordinator with `id` (a no-op for an id that is not saved or is shown).
    pub fn select_remote(&self, id: &str) {
        if locked(&self.shared.active).as_deref() == Some(id) {
            return;
        }
        let Some(entry) = remotes::list().into_iter().find(|e| e.id.0 == id) else {
            return;
        };
        let Some(window) = self.window.upgrade() else {
            return;
        };
        close_dialogs(&window);
        let backend = remote::open(&entry.id);
        let reason = backend.capabilities().denied_reason();
        self.shared.backend.set(backend);
        *locked(&self.shared.active) = Some(entry.id.0.clone());
        let shell = window.global::<ShellState>();
        shell.set_remote(true);
        shell.set_coordinator_name(entry.name.as_str().into());
        shell.set_coordinator_place(format!("Remote · {}", first_host(&entry.addresses)).into());
        shell.set_denied_reason(reason.into());
        if LOCAL_ONLY_PAGES.contains(&shell.get_page()) {
            shell.set_page(0);
        }
        set_rows(&window, &self.shared);
    }

    /// Forgets the saved coordinator with `id`: back to this computer first when it is shown,
    /// then its credentials are deleted on a helper thread.
    fn forget(&self, id: &str) {
        if locked(&self.shared.active).as_deref() == Some(id) {
            self.select_local();
        }
        let id = RemoteId(id.to_owned());
        let weak = self.window.clone();
        let shared = Arc::clone(&self.shared);
        let spawned = std::thread::Builder::new()
            .name("remote-forget".to_owned())
            .spawn(move || {
                remotes::remove(&id);
                locked(&shared.online).remove(&id.0);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(window) = weak.upgrade() {
                        set_rows(&window, &shared);
                    }
                });
            });
        if let Err(e) = spawned {
            tracing::warn!("could not start forgetting a coordinator: {e}");
        }
    }

    /// Rebuilds the list from the saved coordinators and the last probe.
    fn refresh_rows(&self) {
        if let Some(window) = self.window.upgrade() {
            set_rows(&window, &self.shared);
        }
    }

    /// Asks every saved remote that is not shown whether it answers, on a helper thread, and
    /// rebuilds the list when the answers are in.
    fn probe(&self) {
        let active = locked(&self.shared.active).clone();
        let targets: Vec<(String, Vec<String>)> = remotes::list()
            .into_iter()
            .filter(|e| active.as_deref() != Some(e.id.0.as_str()))
            .map(|e| (e.id.0, e.addresses))
            .collect();
        if targets.is_empty() {
            return;
        }
        let shared = Arc::clone(&self.shared);
        let weak = self.window.clone();
        let spawned = std::thread::Builder::new()
            .name("remote-probe".to_owned())
            .spawn(move || {
                let handles: Vec<_> = targets
                    .into_iter()
                    .map(|(id, addresses)| std::thread::spawn(move || (id, answers(&addresses))))
                    .collect();
                for handle in handles {
                    if let Ok((id, up)) = handle.join() {
                        locked(&shared.online).insert(id, up);
                    }
                }
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(window) = weak.upgrade() {
                        set_rows(&window, &shared);
                    }
                });
            });
        if let Err(e) = spawned {
            tracing::warn!("could not start the check for saved coordinators: {e}");
        }
    }
}

/// Puts the rows into the switcher's state.
fn set_rows(window: &AppWindow, shared: &Shared) {
    window
        .global::<SwitcherState>()
        .set_rows(ModelRc::new(VecModel::from(rows(shared))));
}

/// The rows: this computer, then every saved remote.
fn rows(shared: &Shared) -> Vec<SwitcherRow> {
    let active = locked(&shared.active).clone();
    let local_status = shared.local.status();
    let (tag, level) = match local_status.state {
        CoordinatorState::Running => ("Running", 0),
        CoordinatorState::Starting | CoordinatorState::Stopping => ("Starting…", 1),
        CoordinatorState::Stopped | CoordinatorState::Error(_) => ("Stopped", 3),
    };
    let mut rows = vec![SwitcherRow {
        id: SharedString::new(),
        name: local_status.name.as_str().into(),
        place: "This computer".into(),
        tag: tag.into(),
        level,
        selected: active.is_none(),
        remote: false,
    }];
    let shown_state = shared.backend.get().status().state;
    let online = locked(&shared.online).clone();
    for entry in remotes::list() {
        let selected = active.as_deref() == Some(entry.id.0.as_str());
        let (tag, level) = if selected {
            match shown_state {
                CoordinatorState::Running => ("Online", 0),
                CoordinatorState::Starting => ("Connecting…", 1),
                _ => ("Offline", 3),
            }
        } else {
            match online.get(&entry.id.0) {
                Some(true) => ("Online", 0),
                Some(false) => ("Offline", 3),
                None => ("Checking…", 1),
            }
        };
        rows.push(SwitcherRow {
            id: entry.id.0.as_str().into(),
            name: entry.name.as_str().into(),
            place: format!("Remote · {}", access_label(entry.role)).into(),
            tag: tag.into(),
            level,
            selected,
            remote: true,
        });
    }
    rows
}

/// Closes the dialogs that talk to the backend that is about to change, so a code is cancelled
/// on the coordinator that made it.
fn close_dialogs(window: &AppWindow) {
    let pair = window.global::<PairState>();
    if pair.get_open() {
        pair.invoke_close_requested();
    }
    let settings = window.global::<SettingsState>();
    if settings.get_dialog() == 4 {
        settings.invoke_close_code();
    }
    window.global::<SwitcherState>().set_forget_open(false);
}

/// "Full administration" or "Monitor only".
#[must_use]
pub const fn access_label(role: AccessRole) -> &'static str {
    match role {
        AccessRole::Admin => "Full administration",
        AccessRole::Monitor => "Monitor only",
        AccessRole::Local => "This computer",
    }
}

/// The host of the first address of a saved remote (no port), or "unknown address".
fn first_host(addresses: &[String]) -> String {
    addresses.first().map_or_else(
        || "unknown address".to_owned(),
        |a| {
            a.rsplit_once(':')
                .map_or(a.as_str(), |(host, _)| host)
                .to_owned()
        },
    )
}

/// True when one of `addresses` (`ip:port`) accepts a connection within the probe timeout.
fn answers(addresses: &[String]) -> bool {
    addresses.iter().any(|text| {
        text.to_socket_addrs()
            .ok()
            .and_then(|mut found| found.next())
            .is_some_and(|addr: SocketAddr| {
                TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok()
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_access_levels_have_plain_names() {
        assert_eq!(access_label(AccessRole::Admin), "Full administration");
        assert_eq!(access_label(AccessRole::Monitor), "Monitor only");
    }

    #[test]
    fn the_first_host_loses_its_port() {
        assert_eq!(
            first_host(&["192.168.178.40:7878".to_owned()]),
            "192.168.178.40"
        );
        assert_eq!(first_host(&[]), "unknown address");
    }

    #[test]
    fn an_address_that_nobody_listens_on_does_not_answer() {
        // Port 1 on the loopback address refuses at once.
        assert!(!answers(&["127.0.0.1:1".to_owned()]));
        assert!(!answers(&["not an address".to_owned()]));
    }
}
