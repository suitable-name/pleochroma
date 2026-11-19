//! State to Slint: the overlay's properties and the frame image on `RenderModel`.

use std::cell::RefCell;

use pleochroma::diag::{self, Engine, PrepareStep};
use pleochroma_mobile_core::{
    app_state::AppState,
    render::{PauseReason, RenderFrame, RenderStatus},
    viewport::{
        light_dial::light_dial,
        render_view::{
            details_lines, light_picture_stale, long_step_offer, pill_line, probe_wait_line,
            progress_fraction, quality_index, status_line,
        },
    },
};
use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};

use super::prep_note::PreparationNote;
use crate::{AppWindow, RenderModel};

thread_local! {
    /// The session's preparation time for Render Details.
    static NOTE: RefCell<PreparationNote> = RefCell::new(PreparationNote::default());
}

/// The engine the Render view draws with now.
pub(super) fn current_engine(state: &AppState) -> Engine {
    if state.settings.render.engine_spec().is_ray() {
        Engine::RayTraced
    } else {
        Engine::PathTraced
    }
}

/// The engine's name as a person reads it.
const fn engine_name(engine: Engine) -> &'static str {
    match engine {
        Engine::PathTraced => "Path-Traced",
        Engine::RayTraced => "Ray-Traced",
    }
}

/// Marks the preparation as abandoned: "Use the CPU" was asked for.
pub(super) fn note_abandoned() {
    NOTE.with(|note| note.borrow_mut().abandon(diag::process_ms()));
}

/// The compile's progress while a Pause holds the render and the shaders still build:
/// `(done, total, seconds in this step)`.
fn compile_in_flight(engine: Engine) -> Option<(u32, u32, u32)> {
    let progress = diag::prepare_progress();
    if progress.step != PrepareStep::BuildingShaders || progress.engine != engine {
        return None;
    }
    let seconds = diag::process_ms().saturating_sub(progress.start_ms) / 1000;
    Some((
        progress.done,
        progress.total,
        u32::try_from(seconds).unwrap_or(u32::MAX),
    ))
}

/// The status line, the pill's two lines and the offer card, for the status and the gate.
fn push_status(model: &RenderModel<'_>, state: &AppState, status: &RenderStatus) {
    let engine = current_engine(state);
    let held = state.render.held_by_probe();
    let mut status_text = status_line(state);
    let (mut title, mut detail) = pill_line(state);
    if held {
        // The probe holds renders back: no picture yet says it in the status line, an earlier
        // picture says it in the pill.
        let (wait_title, wait_detail) = probe_wait_line(model.get_has_frame());
        if wait_title.is_empty() {
            status_text = wait_detail;
        } else {
            title = wait_title;
            detail = wait_detail;
        }
    }
    let offer = if held {
        None
    } else {
        long_step_offer(status, engine)
    };
    if offer.is_some()
        && let RenderStatus::Preparing { elapsed_s, .. } = status
    {
        // The pill and the status line keep their progress and add the time in the step.
        for text in [&mut detail, &mut status_text] {
            if !text.ends_with(" s") {
                *text = format!("{text} · {elapsed_s} s");
            }
        }
    }
    if let (RenderStatus::Paused(_), Some((done, total, seconds))) =
        (status, compile_in_flight(engine))
    {
        // A compile cannot be stopped: it runs on while the render is paused.
        title = "Paused".to_owned();
        detail = format!("Shaders still building · {done} of {total}");
        status_text = format!("Building shaders · {done} of {total} · {seconds} s");
    }
    model.set_status_text(status_text.into());
    model.set_pill_title(title.into());
    model.set_pill_detail(detail.into());
    model.set_probe_held(held);
    model.set_offer_visible(offer.is_some());
    let (sentence, button) =
        offer.map_or_else(Default::default, |offer| (offer.sentence, offer.button));
    model.set_offer_sentence(sentence.into());
    model.set_offer_button(button.into());
}

/// The lines Render Details adds under the report: the CPU sentence, the shader cache and
/// the preparation time.
fn push_details(model: &RenderModel<'_>, state: &AppState, status: &RenderStatus) {
    let engine = current_engine(state);
    NOTE.with(|note| note.borrow_mut().observe(status, diag::process_ms()));
    let diagnostics = &state.diagnostics;
    let lines = details_lines(
        diagnostics.state().cache(),
        diagnostics
            .adapter()
            .map(|adapter| adapter.pipeline_cache_offered),
        NOTE.with(|note| note.borrow().time()),
        state
            .render
            .session_cpu_only(engine)
            .then_some(engine_name(engine)),
    );
    model.set_details_lines(lines.join("\n").into());
}

/// Pushes the status line, the progress, the chips' state and whether there is something
/// to render. Cheap: a handful of property writes (Slint skips unchanged values).
pub(super) fn push(window: &AppWindow, state: &AppState) {
    let model = window.global::<RenderModel>();
    let status = state.render.status();
    let generation = state.session.current_generation();
    let solved = state.solve.current_tiers(generation).is_some();
    let held = state.render.held_by_probe();

    push_status(&model, state, &status);
    push_details(&model, state, &status);
    model.set_backend(state.settings.render.backend.index());
    model.set_engine(i32::from(
        state.settings.render.engine_spec().engine.index(),
    ));
    model.set_backend_report(state.render.backend_report().into());
    if let Some(fraction) = progress_fraction(&status) {
        model.set_progress(if solved { fraction } else { 0.0 });
    }
    model.set_paused(status == RenderStatus::Paused(PauseReason::User));
    model.set_can_pause(
        solved && !held && !matches!(status, RenderStatus::Idle | RenderStatus::Failed(_)),
    );
    // The probe's gate is the one switch that turns the pill's Retry chip and Export off.
    model.set_can_render(solved && !held);
    let ray = state.settings.render.engine_spec().is_ray();
    let choices = super::samples::choices(super::samples::remote_ready(state));
    let labels: Vec<SharedString> = if ray {
        ["Draft", "Normal", "Fine"]
            .into_iter()
            .map(SharedString::from)
            .collect()
    } else {
        choices
            .iter()
            .map(|spp| SharedString::from(format!("{spp} samples")))
            .collect()
    };
    let current = model.get_quality_choices();
    if current.row_count() != labels.len()
        || labels
            .iter()
            .enumerate()
            .any(|(i, label)| current.row_data(i).as_ref() != Some(label))
    {
        model.set_quality_choices(ModelRc::new(VecModel::from(labels)));
    }
    model.set_quality(if ray {
        quality_index(state.settings.render.quality)
    } else {
        i32::try_from(
            choices
                .iter()
                .position(|&spp| spp == state.settings.render.live_spp_compact)
                .or_else(|| choices.iter().position(|&spp| spp == 32))
                .unwrap_or(0),
        )
        .unwrap_or(0)
    });
    model.set_denoise(state.settings.render.denoise);
    model.set_light_drag(state.viewport.light_drag.on);
    let dial = light_dial(state);
    model.set_light_dial_x(dial.x);
    model.set_light_dial_y(dial.y);
    model.set_light_readout(dial.readout.into());
    model.set_light_moving(light_picture_stale(state));
}

/// Shows `frame` in the Render view. A frame whose pixel count does not match its size is
/// dropped (logged).
pub(super) fn show_frame(window: &AppWindow, frame: &RenderFrame) {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(frame.width, frame.height);
    let bytes = buffer.make_mut_bytes();
    if bytes.len() != frame.rgba.len() {
        tracing::warn!(
            width = frame.width,
            height = frame.height,
            len = frame.rgba.len(),
            "a render frame was dropped: its pixel count does not match its size"
        );
        return;
    }
    bytes.copy_from_slice(&frame.rgba);
    let model = window.global::<RenderModel>();
    // The corner pixel is the picture's backdrop; it fills the view around the fitted frame.
    if let [r, g, b, _, ..] = frame.rgba[..] {
        model.set_backdrop(slint::Color::from_rgb_u8(r, g, b));
    }
    model.set_frame(Image::from_rgba8(buffer));
    model.set_has_frame(true);
}
