//! The tutorial browser (Help > Tutorials...): the lessons the browser build offers, by section,
//! with a search box, a "Done" mark and Start / Restart, and the marks themselves.
//!
//! Which lessons exist, how they are grouped and searched, and the "Done" bookkeeping are pure
//! functions in `pleochroma_editor::guide` (`browser_rows`, `progress_text`, ...); the lessons
//! listed are the web views of `editor::guide::offered_guides`, so a learner sees only what the
//! browser can complete, in the browser's words. This module turns them into Slint rows
//! (`ui/components/tutorial_browser.slint`'s `TutorialsModel`), starts the chosen lesson through
//! `editor::guide::start_guide`, and keeps the finished lessons in `localStorage` under
//! `pleochroma.tutorials.v1` (`pleochroma_web_core::storage_keys::TutorialMarks`).
//!
//! The marks live in this browser only: nothing leaves it, and another browser or a cleared
//! site starts without them. A browser that blocks storage simply shows no marks.

use crate::{
    AppModel, AppWindow, TutorialRowData, TutorialsModel,
    app::{Ctx, persist},
    editor::guide,
};
use pleochroma_web_core::{
    storage_keys::{TutorialMarks, tutorials_key},
    tutorial_list::{TutorialRow, listing},
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, collections::BTreeSet};

/// The event the browser reports when it opens (a lesson may wait for it).
pub const TUTORIALS_OPENED: &str = "tutorials_opened";

thread_local! {
    /// The finished lessons, read from `localStorage` on first use.
    static MARKS: RefCell<Option<TutorialMarks>> = const { RefCell::new(None) };
}

/// The marks as stored, or none when nothing (usable) is stored.
fn load_marks() -> TutorialMarks {
    let key = tutorials_key();
    let Some(text) = persist::read(&key) else {
        return TutorialMarks::default();
    };
    match TutorialMarks::from_json(&text) {
        Ok(marks) => marks,
        Err(e) => {
            persist::discard(&key, &e);
            TutorialMarks::default()
        }
    }
}

/// Runs `f` on the marks, loading them first when this is the first use.
fn with_marks<R>(f: impl FnOnce(&mut TutorialMarks) -> R) -> R {
    MARKS.with_borrow_mut(|slot| f(slot.get_or_insert_with(load_marks)))
}

/// Writes the marks to `localStorage`.
fn save_marks(marks: &TutorialMarks) {
    match marks.to_json() {
        Ok(json) => persist::write(&tutorials_key(), &json),
        Err(e) => persist::discard(&tutorials_key(), &e),
    }
}

/// The ids of the lessons finished so far.
#[must_use]
pub fn finished_ids() -> BTreeSet<String> {
    with_marks(|marks| marks.done.clone())
}

/// One [`TutorialRow`] as the Slint row the browser draws.
fn row_data(row: &TutorialRow) -> TutorialRowData {
    TutorialRowData {
        id: row.id.as_str().into(),
        title: row.title.as_str().into(),
        summary: row.summary.as_str().into(),
        category: row.category.as_str().into(),
        first_in_category: row.first_in_category,
        done: row.done,
        start_label: row.start_label.as_str().into(),
        blocked_reason: row.blocked_reason.as_str().into(),
        step_text: row.step_text.as_str().into(),
    }
}

/// Fills the browser's list for `query` (the rows and the progress line are
/// `pleochroma_web_core::tutorial_list::listing`'s).
fn fill_browser(ui: &AppWindow, query: &str) {
    let guides = guide::offered_guides();
    let has_design = ui.global::<AppModel>().get_has_design();
    let (rows, line) = listing(&guides, &finished_ids(), has_design, query);
    let rows: Vec<TutorialRowData> = rows.iter().map(row_data).collect();
    let model = ui.global::<TutorialsModel>();
    model.set_rows(ModelRc::new(VecModel::from(rows)));
    model.set_progress_text(line.into());
}

/// Re-lists the browser when it is open (a lesson was finished, the progress was reset).
fn refill_if_open(ui: &AppWindow) {
    let tutorials = ui.global::<TutorialsModel>();
    if tutorials.get_browser_open() {
        fill_browser(ui, tutorials.get_query().as_str());
    }
}

/// Saves that lesson `id` was finished and refreshes the browser if it is open. Called by
/// `editor::guide` when the last step of a lesson is done.
pub fn record_finished(ctx: &Ctx, id: &str) {
    let changed = with_marks(|marks| {
        let changed = marks.record(id);
        if changed {
            save_marks(marks);
        }
        changed
    });
    if changed && let Some(ui) = ctx.ui.upgrade() {
        refill_if_open(&ui);
    }
}

/// Forgets every finished lesson and refreshes the browser.
fn reset_progress(ctx: &Ctx) {
    with_marks(|marks| {
        if marks.clear() {
            save_marks(marks);
        }
    });
    if let Some(ui) = ctx.ui.upgrade() {
        refill_if_open(&ui);
    }
}

/// Registers the browser's callbacks. Called once from `editor::guide::wire`.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let tutorials = ui.global::<TutorialsModel>();

    let c = ctx.clone();
    tutorials.on_opened(move || {
        if let Some(ui) = c.ui.upgrade() {
            fill_browser(&ui, ui.global::<TutorialsModel>().get_query().as_str());
            guide::raise(&c, TUTORIALS_OPENED);
        }
    });

    let c = ctx.clone();
    tutorials.on_query_changed(move |query| {
        if let Some(ui) = c.ui.upgrade() {
            fill_browser(&ui, query.as_str());
        }
    });

    let c = ctx.clone();
    tutorials.on_refresh(move || {
        if let Some(ui) = c.ui.upgrade() {
            refill_if_open(&ui);
        }
    });

    // The browser closes first, so a question the start asks (unsaved changes) is the only
    // thing on screen.
    let c = ctx.clone();
    tutorials.on_start(move |id| {
        if let Some(ui) = c.ui.upgrade() {
            ui.global::<TutorialsModel>().set_browser_open(false);
        }
        guide::start_guide(&c, id.as_str());
    });

    let c = ctx.clone();
    tutorials.on_reset_progress(move || reset_progress(&c));
}
