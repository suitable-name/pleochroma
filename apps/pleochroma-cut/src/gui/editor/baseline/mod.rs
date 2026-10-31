//! The Baseline: the copy of the design that every later change is read against, and the strip
//! under the Inspector's tabs that shows the design now next to it
//! (`ui/components/editor_inspector/baseline_strip.slint`, state in `ui/models/baseline.slint`).
//!
//! The Baseline is the holder `retarget_actions::snapshot` keeps (what "Snapshot Design" took
//! before it was renamed Set Baseline): a design and its solved masts. It is replaced by
//!
//! - Set Baseline ([`set_baseline_to`]; the strip's button and the command bar's),
//! - a design opening (`EditorState::replace_wholesale`, which every New, Open and Load path
//!   goes through; this also clears the Baseline the design before it left behind),
//! - an Optimize apply and a Retarget apply: the Baseline becomes the applied design, so
//!   hand-tuning starts from the result. (The design before a Retarget Apply stays available as
//!   the compare picker's "Original".)
//!
//! # The figures
//!
//! They are the sweep's score (`pleochroma_editor::sweep::score_design`), under the Grading tray
//! at the canonical light pose, never the viewport's lighting, so the strip, the trials and the
//! Angle Sweep show the same numbers for the same design and material. The scene key is therefore
//! the material selection only: the Baseline stores the one it was scored under, and when the
//! design's material changes the Baseline design is re-scored under the new one (with its own
//! masts) before the next delta is shown.
//!
//! The strip never solves. [`on_solved`] is called wherever the editor publishes the masts of a
//! solve (the background solve's result, and the solid-preview frame that carries a synchronous
//! solve and every replan); [`state`]'s timer scores them 300 ms later on [`worker`]'s thread, so
//! a drag scores at its end, and not while an Optimize run or an Angle Sweep holds the cores.
//! A design that does not get solved (auto-solve off or over its budget) leaves the last figures
//! on the strip, dimmed, with "Out of Date: Press Solve". "Measure Tilt" is one run with the tilt
//! averages for both designs, on demand and cancellable.

mod present;
mod state;
mod worker;

#[cfg(test)]
mod tests;

pub(in crate::gui::editor) use state::{
    Origin, baseline_figures, design_figures, install, install_parts, solved_masts,
};

use crate::{
    BaselineModel, EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{editor::state::EditorState, show_toast, tutorial_events::raise},
};
use pleochroma::geometry::meet_solver::SolvedTier;
use pleochroma_cut_core::Design;
use pleochroma_editor::guide::solving_events::SNAPSHOT_TAKEN;
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// Wires the strip's callbacks and starts the timer that scores the design against the Baseline.
///
/// Set Baseline and Compare reuse the command bar's own handlers
/// (`EditorModel.snapshot_design()` and `compare_to_snapshot()`, registered by
/// `callbacks::setup_snapshot_callbacks`), so the strip and the bar can never act differently.
pub(super) fn setup_baseline(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    state::attach(ui, state, render_ctx);
    let model = ui.global::<BaselineModel>();
    let ui_weak = ui.as_weak();
    model.on_set_baseline(move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<EditorModel>().invoke_snapshot_design();
        }
    });
    let ui_weak = ui.as_weak();
    model.on_compare(move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<EditorModel>().invoke_compare_to_snapshot();
        }
    });
    model.on_measure_tilt(state::toggle_tilt);
}

/// Lets go of the window's strip state (timer, figures, the running job). See
/// `gui::window_release`.
pub(super) fn release_window_state() {
    state::release();
}

/// Feeds the masts of a solve of `design` at `generation` (the design generation the solve ran
/// against): the strip scores them. Never solves; a no-op until the window is wired.
pub(super) fn on_solved(design: &Design, solved: &[SolvedTier], generation: u64) {
    state::feed_solved(design, solved, generation);
}

/// Holds `design`, solved to `solved` when known, as the Baseline, and reports it: what Set
/// Baseline does. Shared by the command bar's and the strip's button (through
/// `EditorModel.snapshot_design`, on a cache hit and after a background solve alike) and by
/// anything that sets a given design as the Baseline, so every path reports the same way.
pub(in crate::gui::editor) fn set_baseline_to(
    ui: &MainWindow,
    design: &Design,
    solved: Option<Vec<SolvedTier>>,
    label: &str,
) {
    install_parts(Origin::Set, design.clone(), solved, None, label);
    // "Compare to Baseline" in the command bar is gated on this.
    ui.global::<EditorModel>().set_has_snapshot(true);
    show_toast(ui, &format!("Baseline set: \"{label}\"."), "success");
    raise(ui, SNAPSHOT_TAKEN);
}
