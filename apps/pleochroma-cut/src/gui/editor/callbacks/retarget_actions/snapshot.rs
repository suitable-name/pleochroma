//! "Set Baseline"/"Compare to Baseline" (the ids, callbacks and event keep their "snapshot"
//! names, which tutorial routes use): the Baseline every later change is read against, and a
//! separate design-comparison feature that also lives behind this dialog's own shell
//! (`RetargetModel.compare_open`).
//!
//! Two holders live here: [`DESIGN_SNAPSHOT`], the Baseline (`gui::editor::baseline` shows its
//! figures and decides when it is replaced), and [`RETARGET_ORIGINAL`], the design as it was
//! before the last Retarget Apply, which the Optimize compare picker offers as "Original".

use crate::{
    EditorModel, MainWindow, RetargetModel, RetargetRowItem, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            baseline,
            compare::snapshot_summary,
            native_io,
            state::EditorState,
            view::{self, design_label},
        },
        show_toast,
        tutorial_events::raise,
    },
};
use pleochroma::{geometry::meet_solver::SolvedTier, optics::LightingPreset};
use pleochroma_cut_core::{Design, TierDelta, diff_tiers};
use pleochroma_editor::{guide::solving_events::COMPARE_OPENED, snapshot::DesignSnapshot};
use slint::{Color, ComponentHandle, ModelRc, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
};

thread_local! {
    /// The Baseline: a design plus its solved masts, set by Set Baseline
    /// ([`setup_snapshot_callbacks`]'s `snapshot_design` handler), automatically when a design
    /// opens and after an Optimize or Retarget apply (`gui::editor::baseline`), and read back
    /// by its `compare_to_snapshot` handler. A module-local
    /// `thread_local!` for the same reason [`super::RETARGET_ASYNC`] is one --
    /// rather than a new `EditorState` field, and Slint's single-threaded event
    /// loop makes this sound.
    static DESIGN_SNAPSHOT: RefCell<Option<DesignSnapshot>> = const { RefCell::new(None) };

    /// The design as it was before the last Retarget Apply, with its masts when known, which the
    /// Optimize compare picker offers as "Original (Before retarget to ...)". Held in memory
    /// only, never written to the library, and apart from [`DESIGN_SNAPSHOT`]: a Retarget Apply
    /// replaces the Baseline with the applied design but must not lose this one.
    static RETARGET_ORIGINAL: RefCell<Option<DesignSnapshot>> = const { RefCell::new(None) };
}

/// Bumped every time [`DESIGN_SNAPSHOT`] is replaced, so a figure or a background solve that
/// finishes after a newer Baseline was set can tell its result no longer belongs to the held one.
static SNAPSHOT_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Bumped every time [`RETARGET_ORIGINAL`] is replaced: the same guard for the background solve
/// of the original's masts.
static ORIGINAL_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Drops the held Baseline and the held Retarget original when their main window goes, so a
/// later window never compares against a design it did not hold. The epochs are bumped too: a
/// background solve still in flight then finds its result stale. See `gui::window_release`.
pub(in crate::gui::editor) fn release_window_state() {
    SNAPSHOT_EPOCH.fetch_add(1, AtomicOrdering::Relaxed);
    ORIGINAL_EPOCH.fetch_add(1, AtomicOrdering::Relaxed);
    DESIGN_SNAPSHOT.with(|cell| *cell.borrow_mut() = None);
    RETARGET_ORIGINAL.with(|cell| *cell.borrow_mut() = None);
}

/// Replaces the Baseline and returns its epoch (see [`SNAPSHOT_EPOCH`]).
pub(in crate::gui::editor) fn install_baseline(snapshot: DesignSnapshot) -> u64 {
    let epoch = SNAPSHOT_EPOCH.fetch_add(1, AtomicOrdering::Relaxed) + 1;
    DESIGN_SNAPSHOT.with(|cell| *cell.borrow_mut() = Some(snapshot));
    epoch
}

/// The Baseline's epoch and whether one is held. The epoch changes whenever the Baseline is
/// replaced or released, by any path, so a reader can tell its figures belong to another one.
#[must_use]
pub(in crate::gui::editor) fn baseline_marker() -> (u64, bool) {
    (
        SNAPSHOT_EPOCH.load(AtomicOrdering::Relaxed),
        DESIGN_SNAPSHOT.with(|cell| cell.borrow().is_some()),
    )
}

/// A copy of the Baseline with the epoch it was read at, for scoring it; `None` without one.
#[must_use]
pub(in crate::gui::editor) fn baseline_clone() -> Option<(u64, DesignSnapshot)> {
    DESIGN_SNAPSHOT.with(|cell| {
        cell.borrow().as_ref().map(|snapshot| {
            (
                SNAPSHOT_EPOCH.load(AtomicOrdering::Relaxed),
                snapshot.clone(),
            )
        })
    })
}

/// Stores `solved` into the Baseline when it is still Baseline `epoch`, still without masts and
/// the mast count fits its design: a Baseline set without a solve (a design just opened, or
/// just applied) gets its masts from the solve of that very design.
pub(in crate::gui::editor) fn fill_baseline_masts(epoch: u64, solved: Vec<SolvedTier>) {
    if SNAPSHOT_EPOCH.load(AtomicOrdering::Relaxed) != epoch {
        return;
    }
    DESIGN_SNAPSHOT.with(|cell| {
        if let Some(snapshot) = cell.borrow_mut().as_mut()
            && snapshot.solved.is_none()
            && solved.len() == snapshot.design.tiers.len()
        {
            snapshot.solved = Some(solved);
        }
    });
}

/// The Baseline's design and label, for the visual compare window's
/// "Compare visually…" entry (`gui::editor::compare`) -- `None` when none is held. A clone:
/// the compare window only ever reads it, and the Baseline itself stays exactly as set.
#[must_use]
pub(in crate::gui::editor) fn snapshot_for_compare() -> Option<(Design, String)> {
    DESIGN_SNAPSHOT.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|snapshot| (snapshot.design.clone(), snapshot.label.clone()))
    })
}

/// Holds `design`, solved to `solved`, as the Baseline "Compare to Baseline" and "Compare
/// visually..." measure against -- what Set Baseline does for the open design, without its
/// toast. A guide's "Build this design" lesson holds the library design it rebuilds this way
/// while its last step is open, so the learner's stone can be compared with the original.
pub(in crate::gui::editor) fn hold_reference_snapshot(
    ui: &MainWindow,
    design: Design,
    solved: Vec<SolvedTier>,
    label: &str,
) {
    install_baseline(DesignSnapshot {
        design,
        solved: Some(solved),
        label: label.to_string(),
        from_retarget_apply: false,
    });
    ui.global::<EditorModel>().set_has_snapshot(true);
}

/// The label of the original held on Retarget Apply, e.g. `Before retarget to Sapphire`.
#[must_use]
pub(in crate::gui::editor) fn original_snapshot_label(target: &str) -> String {
    format!("Before retarget to {target}")
}

/// The design as it was before the last Retarget Apply, with its label -- `None` when no
/// Retarget has been applied in this session.
#[must_use]
pub(in crate::gui::editor) fn original_for_compare() -> Option<(Design, String)> {
    RETARGET_ORIGINAL.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|snapshot| (snapshot.design.clone(), snapshot.label.clone()))
    })
}

/// Holds the design as it was before a Retarget Apply as the Retarget original, replacing any
/// earlier one (in memory only; no database write). `solved` is the cached solve of that design
/// when one is at hand; without it the original's mast diff simply has no old masts until
/// [`solve_original_masts_in_background`] fills them. Returns the original's epoch for that call.
pub(in crate::gui::editor) fn hold_original_before_retarget(
    design: Design,
    solved: Option<Vec<SolvedTier>>,
    label: &str,
) -> u64 {
    let epoch = ORIGINAL_EPOCH.fetch_add(1, AtomicOrdering::Relaxed) + 1;
    RETARGET_ORIGINAL.with(|cell| {
        *cell.borrow_mut() = Some(DesignSnapshot {
            design,
            solved,
            label: label.to_string(),
            from_retarget_apply: true,
        });
    });
    epoch
}

/// Solves `design` (the original held by [`hold_original_before_retarget`] as original
/// `epoch`) on a worker and stores the masts into that original on the UI thread. Used when
/// the check run's cache missed and the design is too costly for an inline solve (the
/// corpus' worst case is 5.9 s); the result is dropped when a newer original replaced the
/// held one meanwhile.
pub(in crate::gui::editor) fn solve_original_masts_in_background(
    ui: &MainWindow,
    design: Design,
    epoch: u64,
) {
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let solved = design.solve().ok();
        // reason: a closed window simply has no original left to fill.
        let _ = weak.upgrade_in_event_loop(move |_ui| fill_original_masts(epoch, solved));
    });
}

/// Stores `solved` into the held Retarget original when it is still original `epoch`, still
/// without masts, and the mast count fits its design.
fn fill_original_masts(epoch: u64, solved: Option<Vec<SolvedTier>>) {
    if ORIGINAL_EPOCH.load(AtomicOrdering::Relaxed) != epoch {
        return;
    }
    RETARGET_ORIGINAL.with(|cell| {
        if let Some(snapshot) = cell.borrow_mut().as_mut()
            && snapshot.solved.is_none()
            && solved
                .as_ref()
                .is_some_and(|s| s.len() == snapshot.design.tiers.len())
        {
            snapshot.solved = solved;
        }
    });
}

// The pure diff-row view (the status badge, the signed mast delta, the angle texts) moved
// to `pleochroma_editor::snapshot` (shared with the web snapshot dialog); re-exported at
// its old path, which `super::tests` exercises.
pub(super) use pleochroma_editor::snapshot::diff_row_view;

/// `deltas` (`pleochroma_cut_core::diff_tiers`'s own output)
/// rendered into [`RetargetModel::compare_rows`], reusing this dialog's existing
/// [`RetargetRowItem`] row shape rather than a second table type (this shape
/// already fits): `block` is
/// left blank (a design comparison has no crown/pavilion grouping of its own),
/// `old_angle`/`new_angle` are this tier's angle in the snapshot vs. now,
/// `margin` is repurposed to show the signed MAST delta (there is no target
/// material here to measure a critical-angle margin against), and
/// `risk_label`/`risk_color` become a plain change-status badge ("Same" /
/// "Changed" / "Added" / "Removed") instead of a windowing risk.
#[must_use]
pub(super) fn diff_rows_from_deltas(deltas: &[TierDelta]) -> Vec<RetargetRowItem> {
    deltas
        .iter()
        .map(diff_row_view)
        .map(|v| RetargetRowItem {
            tier_index: i32::try_from(v.tier_index).unwrap_or(i32::MAX),
            block: "".into(),
            name: v.name.into(),
            old_angle: v.old_angle.into(),
            new_angle: v.new_angle.into(),
            margin: v.mast_delta.into(),
            risk_label: v.status_label.into(),
            risk_color: Color::from_rgb_u8(v.status_rgb.0, v.status_rgb.1, v.status_rgb.2),
        })
        .collect()
}

/// [`setup_snapshot_callbacks`]'s `compare_to_snapshot` tail -- diffs `snapshot`
/// against `design`'s current state via [`pleochroma_cut_core::diff_tiers`] and
/// opens the Retarget dialog's own shell in its Compare mode
/// (`RetargetModel.compare_open`) -- see `retarget_dialog.slint`'s `if
/// compare_open` branch, which mounts `retarget_dialog/baseline_compare.slint`. Shared by that callback's own cache-hit/cache-miss
/// paths, so the two can never report a comparison differently.
///
/// The label carries a second line, in words, about how the stone differs optically
/// (`gui::editor::compare::snapshot_summary`): the table-up figures of both designs take a
/// few milliseconds, under the viewport's lighting.
fn show_compare_to_snapshot(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    snapshot: &DesignSnapshot,
    design: &Design,
    current_label: &str,
    current_solved: Option<&[SolvedTier]>,
) {
    let deltas = diff_tiers(
        &snapshot.design.tiers,
        snapshot.solved.as_deref(),
        &design.tiers,
        current_solved,
    );
    let rows = diff_rows_from_deltas(&deltas);
    let custom = render_ctx
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .custom_materials
        .as_ref()
        .clone();
    let lighting =
        LightingPreset::from_index(ui.global::<ViewportModel>().get_selected_lighting_index());
    let optical = snapshot_summary(
        &snapshot.design,
        snapshot.solved.as_deref(),
        design,
        current_solved,
        &custom,
        lighting,
    );
    ui.global::<RetargetModel>()
        .set_compare_rows(ModelRc::new(VecModel::from(rows)));
    ui.global::<RetargetModel>().set_compare_label(
        format!(
            "\"{}\" vs. current (\"{current_label}\")\n{optical}",
            snapshot.label
        )
        .into(),
    );
    ui.global::<RetargetModel>().set_compare_open(true);
    raise(ui, COMPARE_OPENED);
}

/// "Set Baseline"/"Compare to Baseline": registers both
/// halves of the design-comparison feature. The command bar
/// (`editor_command_bar.slint`) and the Baseline strip trigger
/// `EditorModel.snapshot_design()` and `compare_to_snapshot()`, and `gui::editor` registers
/// this function at startup.
///
/// `snapshot_design` captures `state.design` plus its current solved masts (from
/// `solid_last_solved`'s cache when it is aligned with the design, else a
/// background solve via `native_io::resolve_solved_then`, so a large,
/// not-yet-cached design is held without blocking the UI thread with a
/// synchronous `Design::solve()` right here) as the Baseline
/// ([`baseline::set_baseline_to`], which fills [`DESIGN_SNAPSHOT`]).
/// `compare_to_snapshot` diffs that Baseline against the design's CURRENT state
/// via [`pleochroma_cut_core::diff_tiers`] and opens the Retarget dialog's own
/// shell in its Compare mode (`RetargetModel.compare_open`) -- see
/// `retarget_dialog.slint`'s `if compare_open` branch (`retarget_dialog/baseline_compare.slint`).
pub(in crate::gui::editor) fn setup_snapshot_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    solid_last_solved: &view::SolidLastSolved,
) {
    {
        let state = Rc::clone(state);
        let solid_last_solved = Arc::clone(solid_last_solved);
        let ui_weak = ui.as_weak();
        ui.global::<EditorModel>().on_snapshot_design(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let (design, label, cached) = {
                let st = state.borrow();
                // the shared cache is now generation-tagged -- only the
                // masts themselves matter here.
                let cached = solid_last_solved
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                    .filter(|(_, s)| s.len() == st.design.tiers.len())
                    .map(|(_, s)| s);
                (Arc::new(st.design.clone()), design_label(&st), cached)
            };
            // A cache hit stays synchronous (no perceptible cost); a miss goes
            // through `native_io::resolve_solved_then`'s SAME cached-or-background
            // resolution the write/export paths already use, so a large,
            // not-yet-cached design snapshots without a multi-second freeze.
            if let Some(solved) = cached {
                baseline::set_baseline_to(&ui, &design, Some(solved), &label);
                return;
            }
            native_io::resolve_solved_then(&ui, design, move |ui, design, solved| {
                baseline::set_baseline_to(ui, &design, solved.ok(), &label);
            });
        });
    }

    {
        let state = Rc::clone(state);
        let render_ctx = Arc::clone(render_ctx);
        let solid_last_solved = Arc::clone(solid_last_solved);
        let ui_weak = ui.as_weak();
        ui.global::<EditorModel>().on_compare_to_snapshot(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let snapshot = DESIGN_SNAPSHOT.with(|cell| cell.borrow().clone());
            let Some(snapshot) = snapshot else {
                show_toast(
                    &ui,
                    "No Baseline set yet -- use Set Baseline first.",
                    "error",
                );
                return;
            };
            let (design, current_label, cached) = {
                let st = state.borrow();
                // the shared cache is now generation-tagged -- only the
                // masts themselves matter here.
                let cached = solid_last_solved
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                    .filter(|(_, s)| s.len() == st.design.tiers.len())
                    .map(|(_, s)| s);
                (Arc::new(st.design.clone()), design_label(&st), cached)
            };
            // Same cached-or-background resolution as `on_snapshot_design` just
            // above, for the same reason.
            if let Some(current_solved) = &cached {
                show_compare_to_snapshot(
                    &ui,
                    &render_ctx,
                    &snapshot,
                    &design,
                    &current_label,
                    Some(current_solved),
                );
                return;
            }
            let render_ctx = Arc::clone(&render_ctx);
            native_io::resolve_solved_then(&ui, design, move |ui, design, solved| {
                show_compare_to_snapshot(
                    ui,
                    &render_ctx,
                    &snapshot,
                    &design,
                    &current_label,
                    solved.ok().as_deref(),
                );
            });
        });
    }

    let ui_weak = ui.as_weak();
    ui.global::<RetargetModel>().on_compare_close(move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<RetargetModel>().set_compare_open(false);
        }
    });
}
