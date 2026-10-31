//! Saves a pattern to its original material or guarded stone target.

use super::{
    actions::{close, notify, refresh_all},
    context::refresh_material,
    session::{Host, with_session},
    store,
};
use crate::{
    ViewportModel,
    gui::{show_toast, tutorial_events::raise},
};
use pleochroma_editor::guide::viewing_events as events;
use slint::ComponentHandle;
use std::sync::PoisonError;

/// Stores the pattern; `close_after` closes the window after a successful save.
pub(super) fn store_pattern(host: &Host, close_after: bool) {
    let job = with_session(host, |session| {
        session.can_store().then(|| {
            (
                session.material_name.clone(),
                session.pattern.clone(),
                session.stone_width_mm,
                session.stone_apply.clone(),
            )
        })
    })
    .flatten();
    let Some((name, pattern, width, stone_apply)) = job else {
        return;
    };
    // The port owns the identity check and undoable edit. Call it with no session/DB borrow.
    let result = stone_apply.as_ref().map_or_else(
        || {
            let guard = host.deps.db.lock().unwrap_or_else(PoisonError::into_inner);
            store::save_pattern(&guard, &name, &pattern, width)
                .map_err(|error| format!("{error:#}"))
        },
        |apply| apply(&pattern),
    );
    if let Err(error) = result {
        notify(host, &error, "error");
        return;
    }
    with_session(host, |session| {
        session.baseline = pattern;
        session.confirm_discard = false;
        if stone_apply.is_none() {
            session.import_note = None;
        }
    });
    if stone_apply.is_none() {
        refresh_material(&host.deps.render_ctx, &host.deps.db, &name, width);
    }
    if let Some(ui) = host.main.upgrade() {
        if stone_apply.is_none() {
            ui.global::<ViewportModel>()
                .invoke_selected_material_changed_for_editor_prefill();
        }
        let message = if stone_apply.is_some() {
            "Applied the color pattern to this stone.".to_owned()
        } else {
            format!("Saved the color pattern of '{name}'.")
        };
        show_toast(&ui, &message, "success");
        raise(&ui, events::PATTERN_SAVED);
    }
    if close_after {
        close(host);
    } else {
        refresh_all(host);
    }
}
