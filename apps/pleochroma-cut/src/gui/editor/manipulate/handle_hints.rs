//! Selection hint updates shared by frame refresh and hover exit.

use super::{SESSION, SelectionKey, Shared, selection_key};
use crate::{MainWindow, PreferencesModel};
use slint::ComponentHandle as _;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::gui::editor::manipulate) struct SelectionHintContext {
    key: SelectionKey,
    label: String,
    driven_by: Option<String>,
    large_handles: bool,
}

/// Updates a changed selection-hint context when hover, drag, and Slice leave the line free.
/// Called after handle placement and on hover exit, so a deferred context does not need a
/// later preview frame to become visible.
pub(super) fn refresh(ui: &MainWindow, ctx: &Shared) {
    if !super::hint_line_is_free(ui) {
        return;
    }
    let Some((key, label, driven_by)) = SESSION.with(|cell| {
        cell.borrow().target.as_ref().map(|target| {
            (
                selection_key(target),
                target.label.clone(),
                super::driven_relation(ctx, target.tier),
            )
        })
    }) else {
        return;
    };
    let context = SelectionHintContext {
        key,
        label,
        driven_by,
        large_handles: ui.global::<PreferencesModel>().get_large_handles(),
    };
    let changed =
        SESSION.with(|cell| cell.borrow().selection_hint_context.as_ref() != Some(&context));
    if !changed {
        return;
    }

    // Treat changed semantics as a fresh selection. A relation edit on the same tier
    // should explain the missing angle handle, and a now-free small-handle tier clears
    // the prior explanation.
    let hint = super::selection_hint_text(
        &context.label,
        context.driven_by.as_deref(),
        context.large_handles,
        None,
        context.key,
    );
    let had_hint = hint.map_or_else(
        || SESSION.with(|cell| std::mem::take(&mut cell.borrow_mut().hover_hint_active)),
        |hint| {
            super::show_selection_hint(ui, &hint);
            false
        },
    );
    SESSION.with(|cell| cell.borrow_mut().selection_hint_context = Some(context));
    if had_hint {
        super::slice::resting_hint(ui);
    }
}
