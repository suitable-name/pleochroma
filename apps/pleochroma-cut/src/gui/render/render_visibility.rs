//! Decides `RenderContext::tab_visible`: whether the live-rendered image is on screen
//! right now, and therefore whether the tracer (local or remote) should be running.
//!
//! The answer combines several inputs: the outer tab, the Live Render/Edit sub-tab,
//! each sub-tab's own view-mode toggle, and comparison visibility. While Retarget or
//! the inline Optimize comparison is open, the main viewport is hidden and paused.
//! [`render_is_visible`] is the pure function encoding it. Every writer (the two Slint
//! signals wired in [`setup_live_render_visibility_callbacks`], the two view-mode toggles'
//! callbacks in `gui::main_window`, and the Retarget open/close handlers) recomputes it
//! through [`recompute_tab_visible`], so no call site's formula can drift from another's.

use crate::{
    CompareModel, MainWindow, RetargetModel, SolidPreviewModel, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{
        solid_preview::preview_state::SolidPreviewState,
        tutorial_events::raise,
        view_modes::{EditViewMode, LiveViewMode},
    },
};
use pleochroma_editor::guide::viewing_events as events;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// Whether the live-rendered image is visible anywhere right now. The render thread's
/// `RenderContext::tab_visible` should be `true` exactly when this is. Pure, so it is
/// unit-tested directly.
///
/// The tracer runs only where it can be seen, which depends on the sub-tab's own view-mode
/// toggle, not only on which sub-tab is selected:
/// - Live Render tab (`render_view_tab == 0`): visible in Path-traced (`1`) or Ray-Traced (`2`)
///   mode (`ViewportModel.live_view_mode`, [`LiveViewMode`]). Solid mode (`0`) shows the
///   flat-shaded CPU rasterizer, and the tracers must not spend GPU or remote time on it.
/// - Edit tab (`render_view_tab == 1`): visible in Path-traced (`1`), Solid + Path-Traced
///   (`2`), Ray-Traced (`4`) or Solid + Ray-Traced (`5`) (`SolidPreviewModel.view_mode`,
///   [`EditViewMode`]), not in Solid (`0`) or Diagram (`3`). The tab alone must not suspend
///   tracing, or the Edit tab's traced pills would show a stale frame instead of a live one.
///
/// `modal_compare_open` covers Retarget and the inline Optimize comparison: their own
/// images replace the main viewport, so local and remote tracing pause there.
#[must_use]
const fn render_is_visible(
    active_tab: i32,
    render_view_tab: i32,
    live_view_mode: i32,
    solid_view_mode: i32,
    modal_compare_open: bool,
) -> bool {
    !modal_compare_open
        && active_tab == 0
        && ((render_view_tab == 0 && LiveViewMode::from_raw(live_view_mode).is_traced())
            || (render_view_tab == 1 && EditViewMode::from_raw(solid_view_mode).is_traced()))
}

/// Reads the current signals off `ui` and writes the result into `render_ctx.tab_visible`, and
/// the shown sub-tab into `render_ctx.live_tab_shown`. Every writer calls this, so the formula
/// lives in one place.
///
/// `pub(in crate::gui)`: `gui::main_window`'s `on_live_view_mode_changed` and
/// `SolidPreviewModel.on_view_mode_changed` handlers call it too.
///
/// Also clears `RenderContext::camera_drag_held` and `RenderContext::pose_drag_held`: a tab,
/// sub-tab or view-mode switch mid-drag must never leave the preview gate closed or the stone
/// shadow paused.
pub(in crate::gui) fn recompute_tab_visible(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let visible = render_is_visible(
        ui.get_active_tab(),
        ui.get_render_view_tab(),
        ui.global::<ViewportModel>().get_live_view_mode(),
        ui.global::<SolidPreviewModel>().get_view_mode(),
        ui.global::<RetargetModel>().get_is_open()
            || ui.global::<CompareModel>().get_optimizer_inline_open(),
    );
    let mut ctx = RenderContext::lock(render_ctx);
    // The render loop shows the stone posed only on the Live Render tab; the Edit tab's traced
    // modes share the loop and learn the tab here (`RenderContext::effective_stone_pose`).
    ctx.live_tab_shown = ui.get_render_view_tab() == 0;
    let engine = if ui.get_render_view_tab() == 1 {
        EditViewMode::from_raw(ui.global::<SolidPreviewModel>().get_view_mode()).engine()
    } else {
        LiveViewMode::from_raw(ui.global::<ViewportModel>().get_live_view_mode()).engine()
    };
    if ctx.engine.engine != engine {
        ctx.engine.engine = engine;
        ctx.dirty = true;
    }
    ctx.tab_visible = visible;
    // A view switch mid-drag must never leave the preview gate closed (the release
    // may go to a different, now-hidden viewport); the next press re-arms it.
    ctx.camera_drag_held = false;
    ctx.pose_drag_held = false;
    drop(ctx);
    // The same for the progress bar, which stays hidden while a drag is held.
    crate::bridge::render_thread::progress::set_camera_drag(ui, false);
}

/// Wires the outer tab change and the Live Render/Edit sub-tab change, two of the four signals
/// [`render_is_visible`] combines. The other two (`ViewportModel.live_view_mode` and
/// `SolidPreviewModel.view_mode`) are wired in `gui::main_window`, which already persists them.
///
/// `preview_state` is the solid-inspection preview controller. On a sub-tab change it
/// re-issues the last solved plane set for the new tab, via
/// `camera_lighting::resubmit_at_current_pose`.
pub(in crate::gui) fn setup_live_render_visibility_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
) {
    let render_ctx_at = render_ctx.clone();
    let ui_weak_at = ui.as_weak();
    ui.on_active_tab_changed(move |_tab| {
        if let Some(ui) = ui_weak_at.upgrade() {
            recompute_tab_visible(&ui, &render_ctx_at);
        }
    });

    let render_ctx_rv = render_ctx.clone();
    let preview_state_rv = Arc::clone(preview_state);
    let ui_weak_rv = ui.as_weak();
    ui.on_render_view_tab_changed(move |idx| {
        if let Some(ui) = ui_weak_rv.upgrade() {
            recompute_tab_visible(&ui, &render_ctx_rv);
            // A tutorial step may wait for the Live Render tab (0) to be shown.
            if idx == 0 {
                raise(&ui, events::LIVE_RENDER_OPENED);
            }
            // Re-request the image for the newly selected sub-tab, so Edit to Live Render
            // shows the Live tab's own solid raster. `resubmit_at_current_pose` decides the
            // variant and whether it is visible.
            //
            // Deferred by one event-loop turn (`slint::Timer::single_shot`): a synchronous
            // resubmit would run before the new viewport has pushed its `changed width` and
            // `changed height`, so the first frame would be sized from the old viewport.
            let render_ctx_deferred = render_ctx_rv.clone();
            let preview_state_deferred = Arc::clone(&preview_state_rv);
            let ui_weak_deferred = ui.as_weak();
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                let Some(ui) = ui_weak_deferred.upgrade() else {
                    return;
                };
                let ctx = render_ctx_deferred
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                crate::gui::render::camera_lighting::resubmit_at_current_pose(
                    &ui,
                    &ctx,
                    &preview_state_deferred,
                );
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_render_sub_tab_traces_only_in_path_or_ray_traced_mode() {
        // Solid mode (0): the flat-shaded CPU raster is shown instead -- the tracers
        // must not burn GPU/remote time on a frame nobody is looking at.
        assert!(!render_is_visible(0, 0, 0, 0, false));
        // Path-traced mode (1) and Ray-Traced mode (2): visible.
        assert!(render_is_visible(0, 0, 1, 0, false));
        assert!(render_is_visible(0, 0, 2, 0, false));
    }

    #[test]
    fn edit_sub_tab_traces_only_in_path_traced_both_or_ray_traced_mode() {
        // Solid (0) and Diagram (3): no traced image is shown there, so tracing
        // stays suspended.
        assert!(!render_is_visible(0, 1, 1, 0, false));
        assert!(!render_is_visible(0, 1, 1, 3, false));
        // Path-traced (1), Both (2) and Ray-Traced (4): these pills trace LIVE,
        // showing a fresh frame rather than a stale, frozen one.
        assert!(render_is_visible(0, 1, 1, 1, false));
        assert!(render_is_visible(0, 1, 1, 2, false));
        assert!(render_is_visible(0, 1, 1, 4, false));
        assert!(render_is_visible(0, 1, 1, 5, false));
    }

    #[test]
    fn never_visible_outside_the_3d_tab() {
        for render_view_tab in [0, 1] {
            for live_view_mode in 0..=2 {
                for solid_view_mode in 0..=5 {
                    assert!(!render_is_visible(
                        1,
                        render_view_tab,
                        live_view_mode,
                        solid_view_mode,
                        false
                    ));
                    assert!(!render_is_visible(
                        2,
                        render_view_tab,
                        live_view_mode,
                        solid_view_mode,
                        false
                    ));
                }
            }
        }
    }

    #[test]
    fn an_open_modal_comparison_pauses_the_viewport_in_every_configuration() {
        // The Retarget dialog's embedded pane is the only live view: the viewport
        // behind the modal is paused whichever tab, sub-tab or view mode would
        // otherwise have kept it tracing.
        for active_tab in [0, 1, 2] {
            for render_view_tab in [0, 1] {
                for live_view_mode in 0..=2 {
                    for solid_view_mode in 0..=5 {
                        assert!(!render_is_visible(
                            active_tab,
                            render_view_tab,
                            live_view_mode,
                            solid_view_mode,
                            true
                        ));
                    }
                }
            }
        }
    }

    #[test]
    fn closing_the_modal_comparison_restores_the_prior_visibility() {
        // Path-traced Live Render tab: visible, paused while the modal is open,
        // visible again on close.
        assert!(render_is_visible(0, 0, 1, 0, false));
        assert!(!render_is_visible(0, 0, 1, 0, true));
        assert!(render_is_visible(0, 0, 1, 0, false));
    }
}
