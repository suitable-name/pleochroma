//! The paired computer's design library on the Library page: the source switch, live
//! search on the computer, opening one of its designs, and finding out whether the
//! computer shares a library at all.
//!
//! Everything that talks to the computer runs on the runtime's `library-remote` queue and
//! lands through the event loop, checked against the search generation (searches and
//! the switch) or the open generation (opening one design), so an older answer never
//! replaces a newer one. Nothing here blocks the UI thread. Every action the user starts
//! asks for the local network first ([`crate::remote::with_local_network`]); the
//! background probe never does: without the access it does not run.
//!
//! While the computer is shown, `Runtime::cards` stays empty (those are the phone's
//! cards), the rows hold the computer's designs and `RemoteBrowse::cards` says what they
//! are. A card on the computer reads "On This Phone" when a sync already copied it.

use std::{collections::HashSet, rc::Rc, sync::Arc};

use pleochroma_mobile_core::{
    library::{FilterOptions, LibraryCard, LibraryDesign, LibraryHandle},
    platform::PickedFile,
    remote::library::{RemoteCard, RemoteLibrary, RemoteLibraryError, RemoteSearch},
    settings::RemoteSettings,
    tasks::{BackgroundQueue, Generation},
};
use slint::{ComponentHandle, Model};

use super::{
    format,
    list::{self, Reload},
    runtime::{self, Runtime},
};
use crate::{AppWindow, LibraryCardData, LibraryModel, design::Shared, push::toast};

/// Which library the page shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Source {
    /// The phone's own library.
    #[default]
    Phone,
    /// The paired computer's library.
    Computer,
}

impl Source {
    /// The source for the page's `source-index` (0 phone, 1 computer).
    pub(super) const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Phone),
            1 => Some(Self::Computer),
            _ => None,
        }
    }

    /// The page's `source-index`.
    pub(super) const fn index(self) -> i32 {
        match self {
            Self::Phone => 0,
            Self::Computer => 1,
        }
    }
}

/// What the page knows about the computer's library.
#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent UI state flags (unreachable, switching, available, opening, ...), not a state machine"
)]
pub(super) struct RemoteBrowse {
    /// The designs shown on the computer view, in the shown order.
    pub cards: Vec<RemoteCard>,
    /// Why the computer's library cannot be shown; empty when it can.
    pub error: String,
    /// The error says that no saved address answered.
    pub unreachable: bool,
    /// The switch to the computer is being made.
    pub switching: bool,
    /// The phone's filter options, kept while the computer's are shown.
    pub local_options: Option<FilterOptions>,
    /// The paired computer shares a design library (the source bar and Sync show).
    pub available: bool,
    /// The computer's name for the switch and the texts.
    pub name: String,
    /// Names the newest request to open one design.
    pub opens: Generation,
    /// A design of the computer is being fetched to open it.
    pub opening: bool,
    /// Names the newest probe.
    pub probes: Generation,
    /// A probe is running.
    pub probing: bool,
}

/// What a switch to the computer brings back: its filter options, the first search and
/// the urls of that search a sync already copied.
type Opened = Result<(FilterOptions, RemoteSearch, HashSet<String>), RemoteLibraryError>;

/// What a search of the computer brings back: the designs and the urls already copied.
type Found = Result<(RemoteSearch, HashSet<String>), RemoteLibraryError>;

/// The beginning of the busy line while a design of the computer is fetched.
const OPENING_PREFIX: &str = "Opening ";

// ---------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------

/// The card a row of the computer view is drawn from: the computer's card with "synced"
/// meaning "also on this phone" (`copied` holds the urls a sync copied). A design on the
/// computer is never "mine": that word is about this phone's own designs.
fn shown_card(card: &RemoteCard, copied: &HashSet<String>) -> LibraryCard {
    let mut shown = card.card.clone();
    shown.mine = false;
    shown.synced = copied.contains(&card.url);
    shown
}

/// The urls among `cards` that `handle` holds as live copies (none while the library is
/// not open).
fn mirrored(handle: Option<&LibraryHandle>, cards: &[RemoteCard]) -> HashSet<String> {
    handle.map_or_else(HashSet::new, |handle| {
        let urls: Vec<&str> = cards.iter().map(|card| card.url.as_str()).collect();
        handle.mirrored_urls(&urls)
    })
}

/// The name for the switch: the computer's, or "Computer" when it has none.
fn display_name(remote: &RemoteSettings) -> String {
    let name = remote.name.trim();
    if name.is_empty() {
        "Computer".to_owned()
    } else {
        name.to_owned()
    }
}

// ---------------------------------------------------------------------------------
// The runtime's view of the pairing
// ---------------------------------------------------------------------------------

/// The paired computer as a [`RemoteLibrary`] (a snapshot of the saved pairing), or `None`
/// when the phone is not paired or the app state is busy.
pub(super) fn remote_library(rt: &Runtime) -> Option<RemoteLibrary> {
    let app = rt.shared.try_borrow().ok()?;
    let remote = &app.settings.remote;
    remote
        .enrolled
        .then(|| RemoteLibrary::new(Arc::clone(app.platform()), remote.clone()))
}

/// The saved pairing changed (or may have): whether the computer's library is offered and
/// what its name is. Called from `push`. A computer that stopped sharing a library, or a
/// pairing that is gone, takes the page back to the phone.
pub(super) fn track_pairing(rt: &mut Runtime, window: &AppWindow, remote: &RemoteSettings) {
    rt.remote.available = remote.enrolled && remote.serves_library == Some(true);
    rt.remote.name = display_name(remote);
    if !rt.remote.available && (rt.source == Source::Computer || rt.remote.switching) {
        show_phone(rt, window);
    }
}

/// Stores what is now known about the computer's library (`None` forgets it) and lets every
/// surface see it. Must not be called while the runtime is borrowed.
pub(super) fn remember_serves(window: &AppWindow, shared: &Shared, serves: Option<bool>) {
    let changed = if let Ok(mut app) = shared.try_borrow_mut() {
        if app.settings.remote.serves_library == serves {
            false
        } else {
            app.update_settings(|settings| settings.remote.serves_library = serves);
            true
        }
    } else {
        tracing::warn!("the app state was busy; the computer's library answer was not saved");
        false
    };
    if changed {
        crate::design::refresh(window, shared);
    }
}

/// Whether the computer's library is the one on show.
pub(super) fn on_computer() -> bool {
    runtime::with(|rt| rt.source == Source::Computer).unwrap_or(false)
}

/// Writes the properties of the source bar and the computer view.
pub(super) fn sync_properties(rt: &Runtime, window: &AppWindow) {
    let model = window.global::<LibraryModel>();
    model.set_source_available(rt.remote.available);
    model.set_sync_available(rt.remote.available);
    model.set_computer_name(rt.remote.name.as_str().into());
    model.set_source_index(rt.source.index());
    model.set_source_switching(rt.remote.switching);
    model.set_remote_error(rt.remote.error.as_str().into());
    model.set_remote_unreachable(rt.remote.unreachable);
}

// ---------------------------------------------------------------------------------
// Switching
// ---------------------------------------------------------------------------------

/// The user picked a segment of the source switch.
pub(super) fn set_source(window: &AppWindow, index: i32) {
    match Source::from_index(index) {
        Some(Source::Phone) => {
            runtime::with(|rt| show_phone(rt, window));
        }
        Some(Source::Computer) => crate::remote::with_local_network(window, begin_switch),
        None => {}
    }
}

/// Shows the phone's library: the computer's answers still on their way are dropped and
/// the phone's list is read again.
pub(super) fn show_phone(rt: &mut Runtime, window: &AppWindow) {
    let was_switching = std::mem::take(&mut rt.remote.switching);
    let _superseded = rt.remote.opens.advance();
    finish_opening(rt);
    if rt.source == Source::Computer {
        rt.source = Source::Phone;
        // The computer's answers still on their way are dropped with the generation.
        let _superseded = rt.generation.advance();
        rt.remote.cards.clear();
        rt.remote.error.clear();
        rt.remote.unreachable = false;
        if let Some(options) = rt.remote.local_options.take() {
            rt.options = options;
            list::sync_options(rt, window);
        }
        rt.rows.set_vec(Vec::new());
        rt.loaded.clear();
        rt.total = 0;
        rt.loading = false;
        let revision = list::current_revision(rt);
        list::reload(rt, revision, Reload::Restart);
    } else if was_switching {
        let _superseded = rt.generation.advance();
        let revision = list::current_revision(rt);
        list::reload(rt, revision, Reload::Refresh);
    }
    list::sync(rt, window);
}

/// Starts the switch to the computer (with the local network access in hand): its filter
/// options and a first search arrive together, and only then does the page switch.
fn begin_switch(window: &AppWindow) {
    runtime::with(|rt| {
        if rt.source == Source::Computer || rt.remote.switching {
            return;
        }
        let Some(remote) = remote_library(rt) else {
            toast::show(window, "Pair this phone with a computer first.");
            return;
        };
        let generation = rt.generation.advance();
        rt.remote.switching = true;
        let query = rt.filters.to_remote_query(rt.settings.sort);
        let (counter, weak, handle) = (rt.generation.clone(), rt.weak.clone(), rt.handle.clone());
        rt.remote_queue
            .get_or_insert_with(|| BackgroundQueue::new("library-remote"))
            .submit(move || {
                let cancelled = || !counter.is_current(generation);
                let result: Opened = remote.open(&query, &cancelled).map(|(options, found)| {
                    let copied = mirrored(handle.as_ref(), &found.cards);
                    (options, found, copied)
                });
                let delivered = weak.upgrade_in_event_loop(move |window| {
                    switched(&window, generation, result);
                });
                if let Err(error) = delivered {
                    tracing::debug!(%error, "a library switch was dropped: the UI thread is gone");
                }
            });
        list::sync(rt, window);
    });
}

/// The switch to the computer finished on the queue.
fn switched(window: &AppWindow, generation: u64, result: Opened) {
    let failure = runtime::with(|rt| {
        if !rt.generation.is_current(generation) || !rt.remote.switching {
            return None;
        }
        rt.remote.switching = false;
        let failure = match result {
            Ok((options, found, copied)) => {
                rt.remote.local_options = Some(std::mem::replace(&mut rt.options, options));
                rt.source = Source::Computer;
                rt.selected.clear();
                rt.cards.clear();
                rt.loaded.clear();
                rt.filters.drop_phone_only_choices();
                rt.remote.error.clear();
                rt.remote.unreachable = false;
                let restart = rt.filters.prune(&rt.options);
                list::sync_options(rt, window);
                install(rt, found, &copied);
                rt.scroll_token = rt.scroll_token.wrapping_add(1);
                if restart {
                    let revision = list::current_revision(rt);
                    list::reload(rt, revision, Reload::Restart);
                }
                None
            }
            Err(error) => Some((error, rt.remote.name.clone())),
        };
        list::sync(rt, window);
        failure
    })
    .flatten();
    if let Some((error, name)) = failure {
        report_failure(window, &error, &name);
    }
}

// ---------------------------------------------------------------------------------
// Searching the computer
// ---------------------------------------------------------------------------------

/// Puts a search result of the computer on the page.
fn install(rt: &mut Runtime, found: RemoteSearch, copied: &HashSet<String>) {
    let rows: Vec<LibraryCardData> = found
        .cards
        .iter()
        .map(|card| list::card_row(&shown_card(card, copied), false, None))
        .collect();
    rt.rows.set_vec(rows);
    rt.total = found.cards.len();
    rt.remote.cards = found.cards;
    rt.loading = false;
    rt.loading_more = false;
}

/// A reload while the computer is shown: a new search or filter asks the computer again;
/// a change of this phone's library only re-reads which designs are already copied.
pub(super) fn reload(rt: &mut Runtime, revision: u64, mode: Reload) {
    rt.shown_revision = revision;
    match mode {
        Reload::Restart => {
            rt.scroll_token = rt.scroll_token.wrapping_add(1);
            restart(rt);
        }
        Reload::Refresh => refresh_flags(rt),
    }
}

/// Searches the computer again from the top with the current text and filters. The shown
/// designs (or the error card) stay until the answer arrives.
fn restart(rt: &mut Runtime) {
    let Some(remote) = remote_library(rt) else {
        return;
    };
    let generation = rt.generation.advance();
    let query = rt.filters.to_remote_query(rt.settings.sort);
    let (counter, weak, handle) = (rt.generation.clone(), rt.weak.clone(), rt.handle.clone());
    rt.loading = true;
    rt.remote_queue
        .get_or_insert_with(|| BackgroundQueue::new("library-remote"))
        .submit(move || {
            let cancelled = || !counter.is_current(generation);
            let result: Found = remote.search(&query, &cancelled).map(|found| {
                let copied = mirrored(handle.as_ref(), &found.cards);
                (found, copied)
            });
            let delivered = weak.upgrade_in_event_loop(move |window| {
                searched(&window, generation, result);
            });
            if let Err(error) = delivered {
                tracing::debug!(%error, "a remote search was dropped: the UI thread is gone");
            }
        });
}

/// A search of the computer finished on the queue.
fn searched(window: &AppWindow, generation: u64, result: Found) {
    let failure = runtime::with(|rt| {
        if !rt.generation.is_current(generation) || rt.source != Source::Computer {
            return None;
        }
        rt.loading = false;
        let failure = match result {
            Ok((found, copied)) => {
                rt.remote.error.clear();
                rt.remote.unreachable = false;
                if found.capped {
                    toast::show(window, format::CAPPED_NOTICE);
                }
                install(rt, found, &copied);
                None
            }
            Err(RemoteLibraryError::Cancelled) => None,
            Err(error) => {
                rt.remote.error = error.message();
                rt.remote.unreachable = error.is_unreachable();
                Some(error)
            }
        };
        list::sync(rt, window);
        failure
    })
    .flatten();
    if let Some(error) = failure {
        follow_up(window, &error);
    }
}

/// Re-reads which of the shown designs a sync has copied (the phone's library changed).
fn refresh_flags(rt: &Runtime) {
    let (Some(handle), Some(queue)) = (rt.handle.clone(), rt.queue.as_ref()) else {
        return;
    };
    let urls: Vec<String> = rt
        .remote
        .cards
        .iter()
        .map(|card| card.url.clone())
        .collect();
    let generation = rt.generation.current();
    let weak = rt.weak.clone();
    queue.submit(move || {
        let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
        let copied = handle.mirrored_urls(&refs);
        let delivered = weak.upgrade_in_event_loop(move |window| {
            flags_ready(&window, generation, &copied);
        });
        if let Err(error) = delivered {
            tracing::debug!(%error, "copied flags were dropped: the UI thread is gone");
        }
    });
}

/// The copied flags of the shown designs arrived.
fn flags_ready(window: &AppWindow, generation: u64, copied: &HashSet<String>) {
    runtime::with(|rt| {
        if !rt.generation.is_current(generation) || rt.source != Source::Computer {
            return;
        }
        for (index, card) in rt.remote.cards.iter().enumerate() {
            let on = copied.contains(&card.url);
            if let Some(mut row) = rt.rows.row_data(index)
                && row.synced != on
            {
                row.synced = on;
                rt.rows.set_row_data(index, row);
            }
        }
        list::sync(rt, window);
    });
}

/// "Try Again" on the error card: asks the computer again.
pub(super) fn retry(window: &AppWindow) {
    crate::remote::with_local_network(window, |window| {
        runtime::with(|rt| {
            if rt.source == Source::Computer {
                let revision = list::current_revision(rt);
                reload(rt, revision, Reload::Restart);
                list::sync(rt, window);
            }
        });
    });
}

/// The computer was found again at new addresses: a computer view that showed an error
/// asks again.
pub(super) fn found_again(window: &AppWindow) {
    runtime::with(|rt| {
        if rt.source == Source::Computer && !rt.remote.error.is_empty() {
            let revision = list::current_revision(rt);
            reload(rt, revision, Reload::Restart);
            list::sync(rt, window);
        }
    });
}

// ---------------------------------------------------------------------------------
// Opening a design of the computer
// ---------------------------------------------------------------------------------

/// A tap on a card of the computer view: fetches the design file and opens it as the
/// current design, after the unsaved-changes question the document code asks.
pub(super) fn tap(window: &AppWindow, index: usize) {
    runtime::with(|rt| {
        if rt.remote.opening {
            return;
        }
        let Some(card) = rt.remote.cards.get(index) else {
            return;
        };
        let Some(remote) = remote_library(rt) else {
            return;
        };
        let (entry, title) = (card.card.id, format::title(&card.card));
        let generation = rt.remote.opens.advance();
        let counter = rt.remote.opens.clone();
        let weak = rt.weak.clone();
        let name = remote.computer_name();
        rt.remote.opening = true;
        if rt.busy.is_none() {
            rt.busy = Some(format!("{OPENING_PREFIX}{title}"));
        }
        rt.remote_queue
            .get_or_insert_with(|| BackgroundQueue::new("library-remote"))
            .submit(move || {
                let cancelled = || !counter.is_current(generation);
                let result = remote.fetch_design(entry, &title, &cancelled);
                let delivered = weak.upgrade_in_event_loop(move |window| {
                    opened(&window, generation, &name, result);
                });
                if let Err(error) = delivered {
                    tracing::debug!(%error, "an opened design was dropped: the UI thread is gone");
                }
            });
        list::sync(rt, window);
    });
}

/// Ends the "Opening ..." state and its busy line.
fn finish_opening(rt: &mut Runtime) {
    rt.remote.opening = false;
    if rt
        .busy
        .as_deref()
        .is_some_and(|text| text.starts_with(OPENING_PREFIX))
    {
        rt.busy = None;
    }
}

/// The design file of the computer arrived on the queue.
fn opened(
    window: &AppWindow,
    generation: u64,
    name: &str,
    result: Result<LibraryDesign, RemoteLibraryError>,
) {
    let shared = runtime::with(|rt| {
        if !rt.remote.opens.is_current(generation) {
            return None;
        }
        finish_opening(rt);
        list::sync(rt, window);
        Some(Rc::clone(&rt.shared))
    })
    .flatten();
    let Some(shared) = shared else {
        return;
    };
    match result {
        Ok(design) => {
            // An unsaved design makes the document code ask first: the toast would be early.
            let clean = shared.try_borrow().is_ok_and(|app| !app.session.is_dirty());
            crate::design::document::open_incoming(
                window,
                &shared,
                vec![PickedFile {
                    display_name: design.file_name,
                    bytes: design.bytes,
                }],
            );
            if clean {
                toast::show(window, &format::opened_from(name));
            }
        }
        Err(error) => report_failure(window, &error, name),
    }
}

// ---------------------------------------------------------------------------------
// Failures
// ---------------------------------------------------------------------------------

/// Tells the user why a request to the computer failed, then does what the failure needs.
fn report_failure(window: &AppWindow, error: &RemoteLibraryError, name: &str) {
    match error {
        RemoteLibraryError::Cancelled => {}
        RemoteLibraryError::NoLibrary => toast::show(window, &format::no_library(name)),
        other => toast::show(window, &other.message()),
    }
    follow_up(window, error);
}

/// What a failure needs besides a message: an unreachable computer is looked for on the
/// network, and one that no longer shares a library is remembered as such.
fn follow_up(window: &AppWindow, error: &RemoteLibraryError) {
    match error {
        RemoteLibraryError::Unreachable(message) => {
            crate::remote::connect_failed(window, message);
        }
        RemoteLibraryError::NoLibrary => {
            if let Some(shared) = runtime::with(|rt| Rc::clone(&rt.shared)) {
                remember_serves(window, &shared, Some(false));
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------------
// Probing
// ---------------------------------------------------------------------------------

/// Finds out in the background whether the paired computer shares a library, when the
/// phone does not know yet. It never opens the system dialog for the local network: without
/// the access it does nothing (a connection test or any action the user starts asks).
pub(super) fn probe_if_unknown() {
    if !crate::remote::local_network_granted() {
        return;
    }
    runtime::with(|rt| {
        if rt.remote.probing {
            return;
        }
        let unknown = rt.shared.try_borrow().is_ok_and(|app| {
            app.settings.remote.enrolled && app.settings.remote.serves_library.is_none()
        });
        if !unknown {
            return;
        }
        let Some(remote) = remote_library(rt) else {
            return;
        };
        let token = rt.remote.probes.advance();
        let (counter, weak) = (rt.remote.probes.clone(), rt.weak.clone());
        rt.remote.probing = true;
        rt.remote_queue
            .get_or_insert_with(|| BackgroundQueue::new("library-remote"))
            .submit(move || {
                let cancelled = || !counter.is_current(token);
                let result = remote.probe(&cancelled);
                let delivered = weak.upgrade_in_event_loop(move |window| {
                    probed(&window, token, result);
                });
                if let Err(error) = delivered {
                    tracing::debug!(%error, "a library probe was dropped: the UI thread is gone");
                }
            });
    });
}

/// The probe finished on the queue.
fn probed(window: &AppWindow, token: u64, result: Result<bool, RemoteLibraryError>) {
    let shared = runtime::with(|rt| {
        if !rt.remote.probes.is_current(token) {
            return None;
        }
        rt.remote.probing = false;
        Some(Rc::clone(&rt.shared))
    })
    .flatten();
    let Some(shared) = shared else {
        return;
    };
    match result {
        Ok(serves) => remember_serves(window, &shared, Some(serves)),
        // Unknown stays unknown: the next visit probes again. A computer that is away is
        // not looked for from here (the page did not ask for it).
        Err(error) => tracing::debug!(%error, "the computer's library could not be probed"),
    }
}

// ---------------------------------------------------------------------------------
// A changed pairing
// ---------------------------------------------------------------------------------

/// The pairing is being forgotten or replaced: everything of the old computer goes. The
/// page shows the phone again, answers on their way are dropped and what was known about
/// the library is forgotten.
pub(super) fn pairing_changed(rt: &mut Runtime, window: &AppWindow) {
    let _superseded = rt.remote.probes.advance();
    rt.remote.probing = false;
    rt.remote.available = false;
    show_phone(rt, window);
}

/// Stops every request to the computer before the runtime goes away.
pub(super) fn stop_all(rt: &Runtime) {
    let _superseded = rt.generation.advance();
    let _superseded = rt.remote.opens.advance();
    let _superseded = rt.remote.probes.advance();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote_card(url: &str, mine: bool) -> RemoteCard {
        RemoteCard {
            card: LibraryCard {
                id: 7,
                title: "Barion".to_owned(),
                designer: String::new(),
                shape: "round".to_owned(),
                gear: 96,
                facets: 57,
                ri: None,
                tags: Vec::new(),
                mine,
                synced: false,
                ignored: false,
                updated_unix: 0,
            },
            url: url.to_owned(),
            revision: [0; 32],
        }
    }

    #[test]
    fn mine_does_not_count_on_the_computer() {
        let copied: HashSet<String> = std::iter::once("local://a.asc".to_owned()).collect();
        // A design the computer imported itself is "mine" there, but not on this phone's list.
        let shown = shown_card(&remote_card("local://a.asc", true), &copied);
        assert!(!shown.mine);
        assert!(shown.synced, "a copied design reads as on this phone");
        let other = shown_card(&remote_card("local://b.asc", true), &copied);
        assert!(!other.mine);
        assert!(!other.synced);
    }

    #[test]
    fn the_source_index_round_trips() {
        assert_eq!(Source::from_index(0), Some(Source::Phone));
        assert_eq!(Source::from_index(1), Some(Source::Computer));
        assert_eq!(Source::from_index(2), None);
        assert_eq!(Source::from_index(-1), None);
        assert_eq!(Source::Phone.index(), 0);
        assert_eq!(Source::Computer.index(), 1);
    }

    #[test]
    fn a_nameless_computer_is_called_computer() {
        let mut remote = RemoteSettings::default();
        assert_eq!(display_name(&remote), "Computer");
        remote.name = "  Studio PC ".to_owned();
        assert_eq!(display_name(&remote), "Studio PC");
    }
}
