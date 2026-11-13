//! A shared generation-stamp registry for analysis results that have no other
//! natural home to track their own "computed against generation N" bookkeeping in.
//!
//! # Why this sits beside `session::result_is_stale`
//!
//! Deep Solve and Optimize keep their result generation on
//! [`super::state::EditorState`] and are checked in [`super::view::push_stale_content`].
//! Their state already holds the stamp, so this registry does not duplicate it.
//!
//! This module covers the Retarget proposal. It has no `EditorState` field of its own,
//! and two call sites produce it (the Shift-mode intent handler and the async Optimize-mode
//! completion in `callbacks::retarget_actions`). A `thread_local!` map keyed by
//! [`ResultKind`] gives both one place to stamp and [`refresh_badges`] one place to read.
//!
//! Tilt curves are not stamped here. `gui::tilt` is outside `gui::editor`'s module tree and
//! cannot reach this registry. `gui::tilt::tilt_profile` compares its own `AxesCacheKey`
//! and `hash_planes` instead, which match the planes, material and light the curve was swept
//! against. Trace staleness keeps its own stamp on `RenderContext::planes_owner`, read by
//! `view::push_trace_staleness`.
//!
//! The registry itself (`ResultKind`, `StaleStamps`) lives in `pleochroma_editor::stale`.
//! This module keeps the UI-thread holder and the Slint badge push.

use crate::{MainWindow, RetargetModel};
pub(in crate::gui::editor) use pleochroma_editor::stale::ResultKind;
use pleochroma_editor::stale::StaleStamps;
use slint::ComponentHandle;
use std::cell::RefCell;

thread_local! {
    /// UI-thread only, since Slint's event loop is single-threaded. Holds the generation each
    /// [`ResultKind`] was last [`stamp`]ed at. Absent means there is no result to badge.
    static STAMPS: RefCell<StaleStamps> = const { RefCell::new(StaleStamps::new()) };
}

/// Records that `kind`'s current result was computed against `generation`. Called when a
/// fresh result lands.
pub(in crate::gui::editor) fn stamp(kind: ResultKind, generation: u64) {
    STAMPS.with(|cell| cell.borrow_mut().stamp(kind, generation));
}

/// Forgets `kind`'s stamp when its result is discarded, so a badge never outlives its result.
pub(in crate::gui::editor) fn clear(kind: ResultKind) {
    STAMPS.with(|cell| cell.borrow_mut().clear(kind));
}

/// Re-derives every `*_stale` Slint property this registry drives, against the design's
/// current `live_generation`. Called once per edit from [`super::view::push_stale_content`],
/// which pushes the Deep Solve and Optimize staleness itself.
pub(in crate::gui::editor) fn refresh_badges(ui: &MainWindow, live_generation: u64) {
    let stale = STAMPS.with(|cell| {
        cell.borrow()
            .is_stale(ResultKind::Retarget, live_generation)
    });
    ui.global::<RetargetModel>().set_stale(stale);
}
