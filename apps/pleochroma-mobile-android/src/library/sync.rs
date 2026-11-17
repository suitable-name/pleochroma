//! Copying the paired computer's designs into this phone's library: the Sync job of the
//! Library page.
//!
//! One sync runs at a time, on its own `library-sync` queue (it blocks for minutes). It
//! asks for the local network first, then `LibraryHandle::sync_from` does the work and
//! reports progress. Progress reaches the page at most ten times a second, and the phone's
//! list refreshes every two seconds while "This Phone" is shown, so a long sync fills the
//! list as it goes. The result stays on the page (a panel the user dismisses) and is also
//! said in a toast.
//!
//! A pairing change (forgetting the computer or pairing another one) stops the sync first
//! ([`stop_for_pairing_change`]): the designs already copied stay, and the result says why
//! the sync stopped.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    time::{Duration, Instant},
};

use pleochroma_mobile_core::{
    library::{LibraryHandle, MirrorHandle, SyncOutcome, progress_text},
    remote::library::{RemoteLibrary, RemoteLibraryError},
    tasks::BackgroundQueue,
};
use slint::{ComponentHandle, Timer, TimerMode, Weak};

use super::{
    format,
    list::{self, Reload},
    remote_source::{self, Source},
    runtime::{self, Runtime},
};
use crate::{AppWindow, LibraryModel, push::toast};

/// How often progress reaches the page at most.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// How often the phone's list is read again while a sync runs.
const LIST_REFRESH: Duration = Duration::from_secs(2);

/// Why a sync starts: the user tapped Sync on the Library page, or answered "Copy Designs"
/// after pairing a computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStart {
    /// Sync on the Library page.
    Library,
    /// The offer right after a new pairing; the user is on the Remote page, so the toast says
    /// where the progress is.
    AfterPairing,
}

/// The sync's state on the UI thread.
#[derive(Default)]
pub(super) struct SyncUi {
    /// The running sync's handle (cancels it); `None` when no sync runs.
    pub handle: Option<MirrorHandle>,
    /// The thread of the sync.
    pub queue: Option<BackgroundQueue>,
    /// What the running sync is doing.
    pub text: String,
    /// Progress from 0 to 1; below 0 while the designs are listed.
    pub fraction: f32,
    /// The outcome of the last sync; empty hides the panel's result.
    pub result: String,
    /// The result is an error.
    pub result_error: bool,
    /// A stop was asked for: the text says so and progress no longer changes it.
    pub stopping: bool,
    /// The stop was asked for because the pairing changed.
    pub pairing_changed: bool,
    /// Refreshes the phone's list while the sync runs.
    pub ticker: Timer,
}

impl SyncUi {
    /// Whether a sync runs.
    pub(super) const fn running(&self) -> bool {
        self.handle.is_some()
    }
}

/// Lets at most one update through per interval, except the last one of a run.
#[derive(Debug, Clone, Copy)]
struct Throttle {
    interval: Duration,
    last: Option<Instant>,
}

impl Throttle {
    /// A throttle that passes at most one update per `interval`.
    const fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    /// Whether the update at `now` goes through. The first one does, and so does the last one
    /// of a run (`last_update`), so the page never stops short of the end.
    fn admit(&mut self, now: Instant, last_update: bool) -> bool {
        let due = last_update
            || self
                .last
                .is_none_or(|previous| now.saturating_duration_since(previous) >= self.interval);
        if due {
            self.last = Some(now);
        }
        due
    }
}

/// How far a sync is: `processed` of `total` designs, below 0 while the total is not known.
fn fraction_of(processed: usize, total: usize) -> f32 {
    if total == 0 {
        return -1.0;
    }
    let processed = u64::try_from(processed).unwrap_or(u64::MAX);
    let total = u64::try_from(total).unwrap_or(u64::MAX);
    let permille = (processed.saturating_mul(1000) / total).min(1000);
    f32::from(u16::try_from(permille).unwrap_or(1000)) / 1000.0
}

// ---------------------------------------------------------------------------------
// Starting and stopping
// ---------------------------------------------------------------------------------

/// Starts a sync (the Sync button, or the offer after pairing). Asks for the local network
/// first; nothing happens when a sync already runs.
pub fn start(window: &AppWindow, start: SyncStart) {
    crate::remote::with_local_network(window, move |window| begin(window, start));
}

/// Starts the sync with the local network access in hand (on the UI thread).
fn begin(window: &AppWindow, start: SyncStart) {
    if runtime::with(|rt| rt.sync.running()).unwrap_or(true) {
        return;
    }
    // The library may not have been shown yet (a sync offered right after pairing).
    let Some(library) = list::ensure_open(window) else {
        toast::show(window, "The library could not be opened.");
        return;
    };
    let announce = runtime::with(|rt| launch(rt, window, library, start)).flatten();
    if let Some(computer) = announce {
        toast::show(window, &format::sync_started(&computer));
    }
}

/// Sets the sync up and hands it to its queue. Returns the computer's name when the sync
/// started and the user should hear about it (a sync started from the Remote page).
fn launch(
    rt: &mut Runtime,
    window: &AppWindow,
    library: LibraryHandle,
    start: SyncStart,
) -> Option<String> {
    let Some(remote) = remote_source::remote_library(rt) else {
        toast::show(window, "Pair this phone with a computer first.");
        return None;
    };
    let computer = remote.computer_name();
    let mirror = MirrorHandle::new();
    rt.sync.handle = Some(mirror.clone());
    rt.sync.stopping = false;
    rt.sync.pairing_changed = false;
    rt.sync.result.clear();
    rt.sync.result_error = false;
    rt.sync.text = format::sync_contacting(&computer);
    rt.sync.fraction = -1.0;

    let list_weak = rt.weak.clone();
    rt.sync
        .ticker
        .start(TimerMode::Repeated, LIST_REFRESH, move || {
            refresh_list(&list_weak);
        });

    let weak = rt.weak.clone();
    let name = computer.clone();
    rt.sync
        .queue
        .get_or_insert_with(|| BackgroundQueue::new("library-sync"))
        .submit(move || run(&library, &remote, &mirror, &weak, &name));
    list::sync(rt, window);
    (start == SyncStart::AfterPairing).then_some(computer)
}

/// The sync, on its queue's thread: copies the designs, reports progress, and delivers the
/// outcome.
fn run(
    library: &LibraryHandle,
    remote: &RemoteLibrary,
    mirror: &MirrorHandle,
    weak: &Weak<AppWindow>,
    computer: &str,
) {
    let mut throttle = Throttle::new(PROGRESS_INTERVAL);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        library.sync_from(remote, mirror, |progress| {
            let total = progress.counts.total_found;
            let last = total > 0 && progress.processed >= total;
            if !throttle.admit(Instant::now(), last) {
                return;
            }
            let text = progress_text(progress, computer);
            let fraction = fraction_of(progress.processed, total);
            let delivered = weak.upgrade_in_event_loop(move |window| {
                progress_shown(&window, text, fraction);
            });
            if delivered.is_err() {
                // The UI thread is gone: nobody is waiting for the rest.
                mirror.cancel();
            }
        })
    }))
    .unwrap_or_else(|_| {
        SyncOutcome::Failed(RemoteLibraryError::Failed(
            "The sync stopped unexpectedly.".to_owned(),
        ))
    });
    let name = computer.to_owned();
    let delivered = weak.upgrade_in_event_loop(move |window| {
        finished(&window, &outcome, &name);
    });
    if let Err(error) = delivered {
        tracing::debug!(%error, "a sync result was dropped: the UI thread is gone");
    }
}

/// The Cancel button (or Stop Sync): the running sync stops after the design it is on.
pub(super) fn cancel(window: &AppWindow) {
    runtime::with(|rt| {
        if let Some(handle) = &rt.sync.handle
            && !rt.sync.stopping
        {
            handle.cancel();
            rt.sync.stopping = true;
            format::SYNC_STOPPING.clone_into(&mut rt.sync.text);
        }
        list::sync(rt, window);
    });
}

/// The pairing is being forgotten or replaced: a running sync stops, and its result says why.
pub(super) fn stop_for_pairing_change(rt: &mut Runtime) {
    if let Some(handle) = &rt.sync.handle {
        handle.cancel();
        rt.sync.stopping = true;
        rt.sync.pairing_changed = true;
        format::SYNC_STOPPING.clone_into(&mut rt.sync.text);
    }
}

/// The result panel's close button.
pub(super) fn dismiss_result(window: &AppWindow) {
    runtime::with(|rt| {
        rt.sync.result.clear();
        rt.sync.result_error = false;
        list::sync(rt, window);
    });
}

/// Stops a running sync before the queues are dropped (the app is ending).
pub(super) fn stop_for_shutdown(rt: &Runtime) {
    if let Some(handle) = &rt.sync.handle {
        handle.cancel();
    }
}

// ---------------------------------------------------------------------------------
// While it runs and when it ends
// ---------------------------------------------------------------------------------

/// A progress update arrived (on the UI thread).
fn progress_shown(window: &AppWindow, text: String, fraction: f32) {
    runtime::with(|rt| {
        if !rt.sync.running() || rt.sync.stopping {
            return;
        }
        rt.sync.text = text;
        rt.sync.fraction = fraction;
        sync_properties(rt, window);
    });
}

/// The two-second tick: the phone's list catches up with what the sync has written.
fn refresh_list(weak: &Weak<AppWindow>) {
    let Some(window) = weak.upgrade() else {
        return;
    };
    runtime::with(|rt| {
        if rt.source != Source::Phone || rt.handle.is_none() {
            return;
        }
        let revision = list::current_revision(rt);
        if revision != rt.shown_revision {
            list::reload(rt, revision, Reload::Refresh);
            list::sync(rt, &window);
        }
    });
}

/// The sync ended (on the UI thread): its result goes on the page and into a toast, the
/// list is read again, and a computer that cannot be reached or no longer shares a library
/// is dealt with.
fn finished(window: &AppWindow, outcome: &SyncOutcome, computer: &str) {
    let done = runtime::with(|rt| {
        rt.sync.handle = None;
        rt.sync.ticker.stop();
        rt.sync.stopping = false;
        rt.sync.text.clear();
        let pairing_changed = std::mem::take(&mut rt.sync.pairing_changed);
        let (message, error) = if pairing_changed {
            (format::SYNC_PAIRING_CHANGED.to_owned(), false)
        } else {
            (
                outcome.summary(computer),
                matches!(
                    outcome,
                    SyncOutcome::Failed(_) | SyncOutcome::Interrupted { .. }
                ),
            )
        };
        rt.sync.result.clone_from(&message);
        rt.sync.result_error = error;
        if outcome.changed() {
            let revision = list::current_revision(rt);
            list::reload(rt, revision, Reload::Refresh);
        }
        list::sync(rt, window);
        (message, pairing_changed, Rc::clone(&rt.shared))
    });
    let Some((message, pairing_changed, shared)) = done else {
        return;
    };
    toast::show(window, &message);
    if pairing_changed {
        return;
    }
    let reason = match outcome {
        SyncOutcome::Failed(error) | SyncOutcome::Interrupted { error, .. } => Some(error),
        SyncOutcome::Completed(_) | SyncOutcome::Cancelled(_) => None,
    };
    match reason {
        Some(RemoteLibraryError::Unreachable(text)) => crate::remote::connect_failed(window, text),
        Some(RemoteLibraryError::NoLibrary) => {
            remote_source::remember_serves(window, &shared, Some(false));
        }
        _ => {}
    }
}

/// Writes the sync's properties of the page.
pub(super) fn sync_properties(rt: &Runtime, window: &AppWindow) {
    let model = window.global::<LibraryModel>();
    model.set_syncing(rt.sync.running());
    model.set_sync_progress(rt.sync.fraction);
    model.set_sync_text(rt.sync.text.as_str().into());
    model.set_sync_result(rt.sync.result.as_str().into());
    model.set_sync_result_error(rt.sync.result_error);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_throttled_but_the_last_update_lands() {
        let start = Instant::now();
        let step = |millis: u64| start + Duration::from_millis(millis);
        let mut throttle = Throttle::new(Duration::from_millis(100));
        assert!(
            throttle.admit(step(0), false),
            "the first update goes through"
        );
        assert!(!throttle.admit(step(10), false));
        assert!(!throttle.admit(step(99), false));
        assert!(throttle.admit(step(100), false));
        assert!(!throttle.admit(step(150), false));
        assert!(
            throttle.admit(step(151), true),
            "the last update lands inside the interval"
        );
        assert!(
            !throttle.admit(step(160), false),
            "and restarts the interval"
        );
    }

    #[test]
    fn the_fraction_reads_as_a_bar_or_as_unknown() {
        assert!(fraction_of(0, 0) < 0.0, "no total yet: indeterminate");
        assert!(fraction_of(5, 0) < 0.0);
        assert!((fraction_of(1, 4) - 0.25).abs() < 1e-6);
        assert!((fraction_of(4, 4) - 1.0).abs() < 1e-6);
        assert!((fraction_of(9, 4) - 1.0).abs() < 1e-6, "never past the end");
    }
}
