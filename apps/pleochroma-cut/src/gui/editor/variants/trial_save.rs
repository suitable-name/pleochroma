//! Saving a trial's design as a variant: the Trials view's "Save as Variant...". The design saved
//! is the trial's, not the open design's (the Variants view's own save form always keeps the
//! design as it is now, or as it was at a history step).
//!
//! The name is "Trial N" (a numbered "Variant N" when that name is taken) and the trial's note
//! becomes the variant's note, so the button saves in one click and the Variants view's Rename
//! and Note buttons change them afterwards. Like every other save here, the library is written on
//! a worker thread and the list is read again when it is done.

use super::{
    Deps,
    actions::{EDITOR_BUSY_TEXT, NO_DESIGN_TEXT, is_busy, set_busy},
    capture, identity_of, live_deps, refresh, rows, spawn_job, store, unix_now, with_db,
};
use crate::{
    MainWindow,
    gui::{show_toast, tutorial_events::raise},
};
use pleochroma_cut_core::Design;
use pleochroma_editor::guide::solving_events::VARIANT_SAVED;
use std::{rc::Rc, sync::Arc};

/// Saves `design`, a trial of the design opened at `design_epoch`, as a variant of the open
/// design. Refuses a trial of a design that is no longer open.
pub(in crate::gui::editor) fn save_design(
    ui: &MainWindow,
    design: Design,
    preferred_name: &str,
    note: &str,
    design_epoch: u64,
) {
    let Some(deps) = live_deps() else {
        return;
    };
    if is_busy(ui, &deps) {
        return;
    }
    let Some(identity) = deps.with_state(identity_of) else {
        show_toast(ui, EDITOR_BUSY_TEXT, "info");
        return;
    };
    let Some(uuid) = identity.uuid else {
        show_toast(ui, NO_DESIGN_TEXT, "info");
        return;
    };
    if identity.epoch != design_epoch {
        show_toast(
            ui,
            "This trial belongs to a design that is no longer open.",
            "warning",
        );
        return;
    }
    let known = deps
        .db
        .try_lock()
        .ok()
        .and_then(|db| store::list(&db, &uuid).ok())
        .unwrap_or_else(|| deps.page.borrow().list.clone());
    let name = if known.iter().any(|variant| variant.name == preferred_name) {
        rows::default_name(&known)
    } else {
        preferred_name.to_owned()
    };
    let note = rows::clean_note(note).ok().flatten();
    let parent = deps.page.borrow().branch.get(&uuid, identity.epoch);
    set_busy(ui, &deps, true);
    let db = Arc::clone(&deps.db);
    let (job_uuid, job_name, now) = (uuid, name.clone(), unix_now());
    let started = spawn_job(
        ui,
        "variant-save-trial",
        move || {
            let text = capture::design_text(&design)?;
            let picture = capture::picture_png(&design);
            with_db(&db, |db| {
                store::save(
                    db,
                    &store::SaveRequest {
                        uuid: &job_uuid,
                        name: &job_name,
                        note: note.as_deref(),
                        design_text: &text,
                        picture_png: picture.as_deref(),
                        parent,
                        now,
                    },
                )
            })
        },
        move |ui, answer| {
            if let Some(deps) = live_deps() {
                finish(ui, &deps, &name, answer);
            }
        },
    );
    if let Err(reason) = started {
        set_busy(ui, &deps, false);
        show_toast(ui, &reason, "error");
    }
}

/// [`save_design`]'s continuation.
fn finish(ui: &MainWindow, deps: &Rc<Deps>, name: &str, answer: Result<i64, String>) {
    set_busy(ui, deps, false);
    match answer {
        Ok(_) => {
            show_toast(ui, &format!("Saved \"{name}\" as a variant."), "success");
            refresh(ui, deps);
            raise(ui, VARIANT_SAVED);
        }
        Err(reason) => show_toast(ui, &reason, "error"),
    }
}
