//! The Render view's Rust side: the ray-traced picture of the Design viewport's Render
//! mode.
//!
//! The decisions (when a trace starts, restarts and stops, the status line, the light
//! drag, the export options) live in `pleochroma_mobile_core::viewport::render_view` and
//! the engine in `pleochroma_mobile_core::render`; this module ties them to the UI:
//!
//! | Submodule | Job |
//! |---|---|
//! | `runtime` | the poll and kick timers, the 10 s governor, the timers' state handle |
//! | `model` | `RenderModel`'s overlay properties from the state |
//! | `prep_note` | the session's preparation time for Render Details |
//! | `export` | the export sheet: options, the worker thread, share and save |
//!
//! Camera changes reach the trace through `viewport::push` (see [`on_viewport_changed`]);
//! settings and design changes through `design::refresh` (see [`push`]); the poll (see
//! `runtime`) catches the rest and stops the trace when the Design destination is left.

mod export;
mod model;
mod prep_note;
mod runtime;
mod samples;

use pleochroma_mobile_core::{
    app_state::AppState,
    render::{PauseReason, RenderStatus},
    settings::{RenderBackend, RenderQuality},
    viewport::render_view,
};
use slint::ComponentHandle;

use super::Shared;
use crate::{AppWindow, RenderModel, push::toast};

pub use runtime::{forget_failure, on_touch_released, on_viewport_changed};

/// Connects `RenderModel`'s callbacks and gives the timers their handle on the state.
pub fn wire(window: &AppWindow, state: &Shared) {
    samples::apply(&mut state.borrow_mut());
    runtime::install(window, state);
    export::wire(window, state);

    let model = window.global::<RenderModel>();

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_engine_selected(move |index| {
        let Some(window) = weak.upgrade() else { return };
        let engine = pleochroma::renderer::engine::RenderEngine::from_index(
            u8::try_from(index).unwrap_or(0),
        );
        {
            let mut app = shared.borrow_mut();
            app.update_settings(|settings| settings.render.engine = engine.index());
            samples::apply(&mut app);
            app.render.stop();
        }
        crate::design::refresh(&window, &shared);
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_backend_selected(move |index| {
        let Some(window) = weak.upgrade() else { return };
        {
            let mut app = shared.borrow_mut();
            let Some(backend) = pleochroma_mobile_core::settings::RenderBackend::from_index(index)
            else {
                return;
            };
            if backend == pleochroma_mobile_core::settings::RenderBackend::DeviceGpu {
                // A manual Device GPU choice is the owner's answer to an engine mark.
                let engine = model::current_engine(&app);
                app.diagnostics.state().clear_engine_mark(engine);
            }
            if app.settings.render.backend == backend {
                return;
            }
            app.update_settings(|settings| settings.render.backend = backend);
            samples::apply(&mut app);
            app.render.set_user_paused(false);
            app.render.stop();
        }
        crate::design::refresh(&window, &shared);
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_restart_requested({
        let weak = window.as_weak();
        let shared = state.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                shared.borrow_mut().render.stop();
                crate::design::refresh(&window, &shared);
            }
        }
    });
    model.on_pause_toggled(move || {
        let Some(window) = weak.upgrade() else { return };
        {
            let mut app = shared.borrow_mut();
            if app.settings.render.backend
                == pleochroma_mobile_core::settings::RenderBackend::Remote
            {
                app.render.cancel_remote();
                drop(app);
                model::push(&window, &shared.borrow());
                toast::show(
                    &window,
                    "Remote session cancelled. Retry starts a new session at sample zero.",
                );
                return;
            }
            let paused = app.render.status() == RenderStatus::Paused(PauseReason::User);
            app.render.set_user_paused(!paused);
        }
        model::push(&window, &shared.borrow());
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_use_cpu_requested(move || {
        let Some(window) = weak.upgrade() else { return };
        {
            let mut app = shared.borrow_mut();
            let engine = model::current_engine(&app);
            model::note_abandoned();
            app.render.use_cpu_for_session(engine);
        }
        crate::design::refresh(&window, &shared);
    });

    wire_settings(window, state);
}

/// The quality dropdown, denoise and light toggles, and the Export button.
fn wire_settings(window: &AppWindow, state: &Shared) {
    let model = window.global::<RenderModel>();

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_quality_selected(move |index| {
        let Some(window) = weak.upgrade() else { return };
        if !shared.borrow().settings.render.engine_spec().is_ray() {
            if samples::select(&mut shared.borrow_mut(), index) {
                crate::design::refresh(&window, &shared);
            }
            return;
        }
        let next = match index {
            0 => RenderQuality::Draft,
            1 => RenderQuality::Normal,
            2 => RenderQuality::Fine,
            _ => return,
        };
        {
            let mut app = shared.borrow_mut();
            if app.settings.render.quality == next {
                return;
            }
            app.update_settings(|settings| settings.render.quality = next);
        }
        toast::show(
            &window,
            &format!("Render quality: {}", render_view::quality_label(next)),
        );
        crate::design::refresh(&window, &shared);
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_denoise_toggled(move || {
        let Some(window) = weak.upgrade() else { return };
        {
            let mut app = shared.borrow_mut();
            let on = !app.settings.render.denoise;
            app.update_settings(|settings| settings.render.denoise = on);
            if app.settings.render.backend
                == pleochroma_mobile_core::settings::RenderBackend::Remote
            {
                app.render.stop();
            }
            app.render.set_denoise(on);
        }
        crate::design::refresh(&window, &shared);
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_light_toggled(move || {
        let Some(window) = weak.upgrade() else { return };
        let available = render_view::light_drag_available(&shared.borrow());
        if available {
            let mut app = shared.borrow_mut();
            let on = !app.viewport.light_drag.on;
            app.viewport.light_drag.on = on;
            app.viewport.light_drag.end();
        } else {
            toast::show(
                &window,
                "This lighting rig has no movable light. Pick another on the Light tab.",
            );
        }
        crate::design::refresh(&window, &shared);
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_export_requested(move || {
        let Some(window) = weak.upgrade() else { return };
        if shared.borrow().render.held_by_probe() {
            // The export button is dimmed while the probe runs; a tap says why.
            toast::show(&window, &render_view::probe_wait_line(false).1);
            return;
        }
        export::open(&window, &shared);
    });
}

/// Pushes the render view's properties from the state and makes sure the timers that
/// follow the Render mode run (or stop) with it. Cheap.
pub fn push(window: &AppWindow, state: &AppState) {
    export::refresh_document(window, state);
    model::push(window, state);
    runtime::sync(window, state);
}

/// Lifecycle notifications arrive without a window handle, after releasing `AppState`.
pub fn refresh_editor() {
    if let Some((window, shared)) = runtime::context() {
        super::refresh(&window, &shared);
    }
}

/// Cancels a picture export or Send To Server upload that is talking to the remote
/// endpoint (Forget Server calls this, so no connection outlives the credentials it uses).
/// An export that renders on this phone keeps running.
pub fn cancel_remote_export() {
    export::cancel_remote();
}

/// A remote render that failed because its computer moved starts over, now that the saved
/// addresses are the new ones: the failed tries are renewed and the trace restarts from sample
/// zero. A render that runs, or does not use the remote computer, is left alone.
pub fn restart_after_move() {
    let Some((window, shared)) = runtime::context() else {
        return;
    };
    let restarted = shared.try_borrow_mut().is_ok_and(|mut app| {
        app.settings.render.backend == RenderBackend::Remote
            && render_view::restart_failed(&mut app)
    });
    if restarted {
        if let Ok(app) = shared.try_borrow() {
            on_viewport_changed(&window, &app);
        }
        super::refresh(&window, &shared);
    }
}

/// Stops the timers and the export thread and drops every thread-local handle (the state,
/// the window). Call once on the UI thread when the event loop has ended.
pub fn shutdown() {
    export::shutdown();
    runtime::shutdown();
}
