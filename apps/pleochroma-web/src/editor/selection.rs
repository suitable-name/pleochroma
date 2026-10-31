//! Selecting tiers in the table, in step with the Solid and Diagram views.
//!
//! `WebApp::selected_tier` is the one selection all three share, and
//! `EditorSession::multi_selected` the Ctrl / Shift group (whose facets the views outline).
//! Like the desktop:
//!
//! - a plain click, the arrow keys, a click on a manufacturability warning or on a facet
//!   select ONE tier and empty the group ([`crate::views::select_tier`]);
//! - Ctrl+click toggles a row in the group and leaves the plain selection where it is;
//! - Shift+click replaces the group with every row between the selection (the anchor) and
//!   the clicked row -- with no selection yet, just the clicked row.

use super::{guide, table};
use crate::{TierTableModel, app::Ctx, views};
use pleochroma_cut_core::design::TierRef;
use pleochroma_editor::guide::{TIERS_MULTI_SELECTED, viewing_events::TIER_SELECTED};
use std::collections::BTreeSet;

/// Selects row `index` (the plain selection); a negative index clears it.
///
/// A concave row is selected in `EditorSession::selected_concave` (the flat selection and
/// the multi-selection address `design.tiers` only, so the Solid and Diagram views and the
/// handles see no selection) and opens the inspector's concave form on it.
///
/// A row that really became the selection tells the running lesson (`TIER_SELECTED`);
/// clearing the selection is not a selection. A click on a facet in the Solid or Diagram view
/// reports itself (`views`).
pub fn select(ctx: &Ctx, index: i32) {
    let picked = match table::row_target(ctx, index) {
        Some(TierRef::Flat(tier)) => {
            set_concave(ctx, None);
            views::select_tier(ctx, Some(tier));
            true
        }
        Some(TierRef::Concave(tier)) => {
            views::select_tier(ctx, None);
            set_concave(ctx, Some(tier));
            super::inspector::concave::open(ctx, Some(tier));
            true
        }
        None => {
            set_concave(ctx, None);
            views::select_tier(ctx, None);
            false
        }
    };
    table::sync(ctx);
    if picked {
        guide::raise(ctx, TIER_SELECTED);
    }
}

/// Makes concave tier `tier` (or none) the session's selected concave tier. The flat
/// selection is the caller's to change ([`views::select_tier`]).
fn set_concave(ctx: &Ctx, tier: Option<usize>) {
    if let Some(design) = ctx.state.borrow_mut().design.as_mut() {
        design.session.selected_concave = tier;
    }
}

/// Whether row `index` is a concave row (the multi-selection group is flat tiers only, so
/// such a click selects the row instead).
fn is_concave_row(ctx: &Ctx, index: i32) -> bool {
    matches!(table::row_target(ctx, index), Some(TierRef::Concave(_)))
}

/// Tells the running lesson that a group of `count` tiers is now ticked, once it holds two or
/// more (the multi-select lesson waits for that).
///
/// Called with the app state released and after the table was refreshed, so the lesson can
/// re-check its step at once. Ticking changes no design data, so a step cannot read it from
/// state; only an actual tick or range pick gets here.
fn report_group(ctx: &Ctx, count: usize) {
    if count >= 2 {
        guide::raise(ctx, TIERS_MULTI_SELECTED);
    }
}

/// Ctrl+click / Select-mode click: toggles row `index` in the group.
fn toggle_multi(ctx: &Ctx, index: i32) {
    if is_concave_row(ctx, index) {
        select(ctx, index);
        return;
    }
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    let count = {
        let mut app = ctx.state.borrow_mut();
        let Some(design) = app.design.as_mut() else {
            return;
        };
        if index >= design.session.design.tiers.len() {
            return;
        }
        if !design.session.multi_selected.remove(&index) {
            design.session.multi_selected.insert(index);
        }
        design.session.multi_selected.len()
    };
    views::request_refresh(ctx);
    table::sync(ctx);
    report_group(ctx, count);
}

/// Shift+click: the group becomes every row between the anchor and `index`.
fn select_range(ctx: &Ctx, index: i32) {
    if is_concave_row(ctx, index) {
        select(ctx, index);
        return;
    }
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    let count = {
        let mut app = ctx.state.borrow_mut();
        let anchor = app.selected_tier.unwrap_or(index);
        let Some(design) = app.design.as_mut() else {
            return;
        };
        let (low, high) = if anchor <= index {
            (anchor, index)
        } else {
            (index, anchor)
        };
        let tier_count = design.session.design.tiers.len();
        design.session.multi_selected = (low..=high).filter(|&i| i < tier_count).collect();
        design.session.multi_selected.len()
    };
    views::request_refresh(ctx);
    table::sync(ctx);
    report_group(ctx, count);
}

/// The batch bar's Clear: empties the group.
fn clear_multi(ctx: &Ctx) {
    {
        let mut app = ctx.state.borrow_mut();
        let Some(design) = app.design.as_mut() else {
            return;
        };
        if design.session.multi_selected.is_empty() {
            return;
        }
        design.session.multi_selected = BTreeSet::new();
    }
    views::request_refresh(ctx);
    table::sync(ctx);
}

/// Wires the selection callbacks of `TierTableModel`.
pub fn wire(model: &TierTableModel<'_>, ctx: &Ctx) {
    let c = ctx.clone();
    model.on_select_tier(move |index| select(&c, index));
    let c = ctx.clone();
    model.on_toggle_multi(move |index| toggle_multi(&c, index));
    let c = ctx.clone();
    model.on_select_range(move |index| select_range(&c, index));
    let c = ctx.clone();
    model.on_clear_multi(move || clear_multi(&c));
    let c = ctx.clone();
    model.on_warning_clicked(move |tier| select(&c, tier));
}
