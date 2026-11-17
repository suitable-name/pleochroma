//! The worker-thread wrapper ([`spawn_mirror_sync`]) the UI actually calls. The algorithm
//! itself is `pleochroma_library_mirror::run_mirror_sync`; see that crate's doc comment for the
//! full design.

use super::options::{
    MirrorHandle, MirrorOptions, MirrorOutcome, MirrorProgress, mirror_source_id,
};
use crate::{bridge::library::client::LibrarySession, settings::WorkerSettings};
use pleochroma_library_mirror::run_mirror_sync;
use pleochroma_vault::db::sqlite::Database;
use slint::{ComponentHandle, Weak};
use std::{
    sync::{Arc, Mutex},
    thread,
};

/// Spawns the mirror-sync worker thread against `worker`'s design library, writing into
/// `db`. `on_progress` is invoked on the UI event loop after each design examined;
/// `on_done` is invoked once, exactly once, when the sync completes, is cancelled, or
/// fails outright. Follows `bridge::export_thread::spawn_export`'s exact pattern (a
/// `thread::spawn` worker, a shared cancel flag, results marshalled back via
/// `Weak::upgrade_in_event_loop`).
///
/// `db` is locked only for each individual database call inside the sync loop (see
/// `pleochroma_library_mirror::run_mirror_sync`), never for the whole sync, so the local library
/// UI stays responsive on the same database during a multi-minute sync.
///
/// The sync runs over one [`LibrarySession`], built here from `worker` and held for the whole
/// sync. A real catalogue thus costs one handshake instead of thousands. See
/// [`LibrarySession`] for what happens when that connection drops mid-sync.
pub fn spawn_mirror_sync<T, P, D>(
    ui_weak: Weak<T>,
    db: Arc<Mutex<Database>>,
    worker: WorkerSettings,
    options: MirrorOptions,
    on_progress: P,
    on_done: D,
) -> MirrorHandle
where
    T: ComponentHandle + 'static,
    P: Fn(&T, MirrorProgress) + Send + 'static + Clone,
    D: Fn(&T, MirrorOutcome) + Send + 'static,
{
    let handle = MirrorHandle::new();
    let job = handle.clone();
    let ui_weak_done = ui_weak.clone();

    thread::spawn(move || {
        let source_id = mirror_source_id(&worker);
        let session = LibrarySession::new(worker);
        let progress_ui_weak = ui_weak;
        let outcome = run_mirror_sync(
            &db,
            &session,
            &source_id,
            options,
            job.flag(),
            move |progress| {
                let on_progress = on_progress.clone();
                let _ =
                    progress_ui_weak.upgrade_in_event_loop(move |ui| on_progress(&ui, progress));
            },
        );
        let _ = ui_weak_done.upgrade_in_event_loop(move |ui| on_done(&ui, outcome));
    });

    handle
}
