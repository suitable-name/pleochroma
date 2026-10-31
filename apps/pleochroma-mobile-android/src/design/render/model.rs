//! State to Slint: the overlay's properties and the frame image on `RenderModel`.

use pleochroma_mobile_core::{
    app_state::AppState,
    render::{PauseReason, RenderFrame, RenderStatus},
    viewport::{
        light_dial::light_dial,
        render_view::{light_picture_stale, progress_fraction, quality_index, status_line},
    },
};
use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};

use crate::{AppWindow, RenderModel};

/// Pushes the status line, the progress, the chips' state and whether there is something
/// to render. Cheap: a handful of property writes (Slint skips unchanged values).
pub(super) fn push(window: &AppWindow, state: &AppState) {
    let model = window.global::<RenderModel>();
    let status = state.render.status();
    let generation = state.session.current_generation();
    let solved = state.solve.current_tiers(generation).is_some();

    model.set_status_text(status_line(state).into());
    model.set_backend(state.settings.render.backend.index());
    model.set_engine(i32::from(
        state.settings.render.engine_spec().engine.index(),
    ));
    model.set_backend_report(state.render.backend_report().into());
    if let Some(fraction) = progress_fraction(&status) {
        model.set_progress(if solved { fraction } else { 0.0 });
    }
    model.set_paused(status == RenderStatus::Paused(PauseReason::User));
    model.set_can_pause(solved && !matches!(status, RenderStatus::Idle | RenderStatus::Failed(_)));
    model.set_can_render(solved);
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
                .unwrap_or(1),
        )
        .unwrap_or(1)
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
    model.set_frame(Image::from_rgba8(buffer));
    model.set_has_frame(true);
}
