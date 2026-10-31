//! Orbit tools: Complete orbit and Mirror indices, on the selected tier (the command bar)
//! or on a row (the amber orbit badge).
//!
//! - **Complete orbit** (`EditorSession::complete_orbit`) expands every incomplete orbit
//!   unit the tier decomposes into to its full symmetric membership, one undoable edit per
//!   unit.
//! - **Mirror** (`EditorSession::mirror_indices`) mirrors the tier's index-wheel positions
//!   to the other side of the symmetry axis as one undoable edit.

use super::{
    edit::{Dirty, finish_edit, with_app},
    table::{self, row_id},
};
use crate::{
    TierTableModel,
    app::{
        Ctx,
        push::{MessageKind, show_message},
    },
};
use pleochroma_cut_core::design::TierRef;

/// The selected row's id -- a flat tier's index, or a selected concave tier's row id (so the
/// tools refuse it with their message instead of doing nothing) -- -1 for none.
fn selected_row(ctx: &Ctx) -> i32 {
    let app = ctx.state.borrow();
    if let Some(tier) = app.selected_tier {
        return i32::try_from(tier).unwrap_or(-1);
    }
    app.design
        .as_ref()
        .and_then(|d| {
            let concave = d.session.selected_concave?;
            row_id(TierRef::Concave(concave), d.session.design.tiers.len())
        })
        .unwrap_or(-1)
}

/// Completes the orbit of tier `index`.
///
/// A concave row is refused (see [`table::editable_tier`]).
pub fn complete_orbit(ctx: &Ctx, index: i32) {
    let Some(index) = table::editable_tier(ctx, index, "Complete Orbit") else {
        return;
    };
    let result = with_app(ctx, |app| {
        Some(app.design.as_mut()?.session.complete_orbit(index))
    });
    match result {
        None => {}
        Some(Ok(false)) => show_message(
            ctx,
            MessageKind::Info,
            "Nothing to complete: this tier's orbit is already whole.",
        ),
        Some(Ok(true)) => finish_edit(ctx, Dirty::one(index)),
        Some(Err(e)) => {
            // Some units may have been completed before the failing one.
            finish_edit(ctx, Dirty::one(index));
            show_message(ctx, MessageKind::Error, &e.to_string());
        }
    }
}

/// Mirrors the index positions of tier `index` to the other side of the symmetry axis.
///
/// A concave row is refused (see [`table::editable_tier`]).
pub fn mirror_indices(ctx: &Ctx, index: i32) {
    let Some(index) = table::editable_tier(ctx, index, "Mirror") else {
        return;
    };
    let result = with_app(ctx, |app| {
        Some(app.design.as_mut()?.session.mirror_indices(index))
    });
    match result {
        None => {}
        Some(Ok(_)) => finish_edit(ctx, Dirty::one(index)),
        Some(Err(e)) => show_message(ctx, MessageKind::Error, &e.to_string()),
    }
}

/// Wires the orbit tools' callbacks.
pub fn wire(model: &TierTableModel<'_>, ctx: &Ctx) {
    let c = ctx.clone();
    model.on_complete_orbit(move |index| complete_orbit(&c, index));
    let c = ctx.clone();
    model.on_mirror_indices(move |index| mirror_indices(&c, index));
    let c = ctx.clone();
    model.on_complete_orbit_selected(move || complete_orbit(&c, selected_row(&c)));
    let c = ctx.clone();
    model.on_mirror_indices_selected(move || mirror_indices(&c, selected_row(&c)));
}
