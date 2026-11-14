//! Callback wiring for the render-quality controls.
//!
//! Covers the lighting preset, target samples, resolution, pause, bounce count, exposure, local
//! render path, surface glare, head shadow, backdrop, stone shadow and tray reflection. Stone
//! material effects live in [`super::material`].

use crate::{
    MainWindow, SettingsModel, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{
        local_compute_target_from_index, local_preview_scale_from_index,
        render::sample_scale::exponent_to_count, show_toast,
        startup_settings::push_quality_controls, tutorial_events::raise,
    },
    settings::{
        SettingsPersister,
        model::{
            QualityPreset, head_shadow_deg_from_slider, surface_glare_from_percent,
            tray_reflection_from_percent,
        },
    },
};
use pleochroma_editor::guide::viewing_events as events;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// Wires the lighting-preset, target-samples, resolution, pause, bounce-count and exposure
/// callbacks. Each persisted setting also feeds the debounced `settings_store`.
pub(in crate::gui) fn setup_material_and_quality_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    super::ray_settings::setup(ui, render_ctx, settings_store);
    // The UI carries the lighting preset as a plain `int`; `offered_from_index` maps it back to
    // the enum, the one boundary crossing from UI index to physics enum. `.label()` is the text
    // that is persisted and shown.
    let render_ctx_lit = render_ctx.clone();
    let settings_store_lit = settings_store.clone();
    let ui_weak_lit = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_lighting_changed(move |idx: i32| {
            let preset = crate::gui::optics::offered_lighting::offered_from_index(idx);
            let mut ctx = RenderContext::lock(&render_ctx_lit);
            ctx.lighting_preset = preset;
            ctx.dirty = true;
            drop(ctx);
            settings_store_lit.update(|s| s.settings.lighting_rig = preset.label().to_string());
            if let Some(ui) = ui_weak_lit.upgrade() {
                show_toast(&ui, &format!("Lighting rig: {}", preset.label()), "info");
                // A tutorial step may wait for a lighting preset to be chosen.
                raise(&ui, events::LIGHTING_PRESET_CHOSEN);
            }
        });

    // Target Samples slider. Carries the exponent (3..=10); `exponent_to_count` converts it
    // to the sample count, which is what is stored, persisted and rendered.
    let render_ctx_samples = render_ctx.clone();
    let settings_store_samples = settings_store.clone();
    let ui_weak_samples = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_target_samples_changed(move |exponent: i32| {
            let target_samples = exponent_to_count(u32::try_from(exponent).unwrap_or(0));
            let mut ctx = RenderContext::lock(&render_ctx_samples);
            ctx.target_samples = target_samples;
            // A new target stops only the unfinished pass; the radiance already valid for
            // this scene is kept. The context guard handles the cancel and the redraw.
            drop(ctx);
            settings_store_samples.update(|s| {
                s.settings.target_samples = target_samples;
                // Touching a detail makes the quality preset Custom (and remembered).
                s.settings.note_quality_edit();
            });
            if let Some(ui) = ui_weak_samples.upgrade() {
                show_toast(&ui, &format!("Target samples: {target_samples}"), "info");
            }
        });

    // Render Resolution pill selector: carries the resolved (width, height) pair. Setting
    // `ctx.width`/`height` resets accumulation: the render loop reallocates its buffers when it
    // sees a size change on the next frame. `dirty` is set as for every other setting here.
    let render_ctx_res = render_ctx.clone();
    let settings_store_res = settings_store.clone();
    let ui_weak_res = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_resolution_changed(move |width: i32, height: i32| {
            let (width, height) = (width.cast_unsigned(), height.cast_unsigned());
            let mut ctx = RenderContext::lock(&render_ctx_res);
            ctx.width = width;
            ctx.height = height;
            ctx.dirty = true;
            drop(ctx);
            settings_store_res.update(|s| {
                s.settings.render_width = width;
                s.settings.render_height = height;
                s.settings.note_quality_edit();
            });
            if let Some(ui) = ui_weak_res.upgrade() {
                show_toast(&ui, &format!("Render resolution: {width}x{height}"), "info");
            }
        });

    setup_local_render_path_callbacks(ui, render_ctx, settings_store);
    setup_quality_preset_callback(ui, render_ctx, settings_store);

    // Explicit pause and resume, the state the button shows. Independent of the
    // tab-visibility suspend.
    let render_ctx_pause = render_ctx.clone();
    ui.global::<ViewportModel>()
        .on_pause_toggled(move |paused: bool| {
            let mut ctx = RenderContext::lock(&render_ctx_pause);
            ctx.paused = paused;
        });

    // The automatic suspend, used while the image is visible nowhere, is driven by
    // `RenderContext::tab_visible`, recomputed by `render_visibility::recompute_tab_visible`
    // from the signals `render_is_visible` combines. That path never touches `ctx.paused`, so
    // a manual pause survives switching tabs or view modes away and back.

    let render_ctx_bnc = render_ctx.clone();
    let settings_store_bnc = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_bounces_changed(move |bounces: i32| {
            let clamped = bounces.cast_unsigned().max(1);
            let mut ctx = RenderContext::lock(&render_ctx_bnc);
            ctx.max_bounces = clamped;
            ctx.dirty = true;
            drop(ctx);
            settings_store_bnc.update(|s| {
                s.settings.max_bounces = clamped;
                s.settings.note_quality_edit();
            });
        });

    let render_ctx_exp = render_ctx.clone();
    let settings_store_exp = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_exposure_changed(move |exposure: f32| {
            let clamped = exposure.clamp(0.2, 5.0);
            let mut ctx = RenderContext::lock(&render_ctx_exp);
            ctx.exposure = clamped;
            ctx.dirty = true;
            drop(ctx);
            settings_store_exp.update(|s| s.settings.exposure = clamped);
        });

    // The Parameters panel's "Some advanced settings are in use" note (Simple mode).
    super::advanced_in_use::setup_advanced_in_use_callback(ui);
}

/// Wires the Render tab's quality presets (`SettingsModel.quality_preset_selected`). Draft,
/// Balanced and High set the samples, the resolution and the bounces together; Custom brings back
/// the mix it remembered (`settings::model::QualityPreset`). The render context gets the three
/// values like the single controls give them, the settings file keeps the choice, and the
/// controls are pushed back so the slider and the lists show the preset's numbers.
fn setup_quality_preset_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx = render_ctx.clone();
    let settings_store = settings_store.clone();
    let ui_weak = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_quality_preset_selected(move |index: i32| {
            let preset = QualityPreset::from_index(index);
            // The same function on a copy to push and on the persisted copy: the settings
            // writer is debounced, and the push must not wait for it.
            let mut next = settings_store.snapshot().settings;
            next.select_quality_preset(preset);
            settings_store.update(|s| s.settings.select_quality_preset(preset));
            {
                let mut ctx = RenderContext::lock(&render_ctx);
                ctx.target_samples = next.target_samples;
                ctx.width = next.render_width;
                ctx.height = next.render_height;
                ctx.max_bounces = next.max_bounces;
                ctx.dirty = true;
            }
            if let Some(ui) = ui_weak.upgrade() {
                push_quality_controls(&ui, &next);
                show_toast(
                    &ui,
                    &format!("Live view quality: {}", preset.label()),
                    "info",
                );
            }
        });
}

/// Wires the two local render-path controls: the preview-then-settle resolution reduction and
/// the CPU/GPU compute target. Kept apart to stay under clippy's function-length lint.
///
/// Neither sets `ctx.dirty` nor raises a toast: neither changes what is on screen, only how the
/// next frame or drag is traced.
fn setup_local_render_path_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    // Local preview-then-settle: resolution is reduced while the camera moves (see
    // `RenderContext::local_preview_scale`). `Off` (index 0, the default) renders at full
    // resolution. No `ctx.dirty` or toast: only the next drag is affected.
    let render_ctx_preview = render_ctx.clone();
    let settings_store_preview = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_local_preview_scale_changed(move |index: i32| {
            let scale = local_preview_scale_from_index(index);
            RenderContext::lock(&render_ctx_preview).local_preview_scale = scale;
            settings_store_preview.update(|s| s.settings.local_preview_scale = scale);
        });

    // Local Compute: which engine(s) the local render loop uses (see
    // `RenderContext::local_compute_target`). The context is updated live, as the render loop
    // reads it every frame. No `ctx.dirty` or toast: switching engines mid-render continues the
    // running average rather than restarting it.
    let render_ctx_local_compute = render_ctx.clone();
    let settings_store_local_compute = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_local_compute_target_changed(move |index: i32| {
            let target = local_compute_target_from_index(index);
            RenderContext::lock(&render_ctx_local_compute).local_compute_target = target;
            settings_store_local_compute.update(|s| s.settings.local_compute_target = target);
        });
}

/// Wires the surface-glare slider: the scale of the white mirror image of the light in
/// the live view and every export (`RenderContext::surface_glare`, persisted as
/// `AppSettings::surface_glare`). The slider reports a percent; a drag that stays on the
/// same 5 % step changes nothing and so never restarts accumulation.
pub(in crate::gui) fn setup_surface_glare_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx = render_ctx.clone();
    let settings_store = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_surface_glare_changed(move |percent: f32| {
            let glare = surface_glare_from_percent(percent);
            let mut ctx = RenderContext::lock(&render_ctx);
            if ctx.surface_glare.to_bits() == glare.to_bits() {
                return;
            }
            ctx.surface_glare = glare;
            ctx.dirty = true;
            drop(ctx);
            settings_store.update(|s| s.settings.surface_glare = glare);
        });
}

/// Wires the head-shadow slider: the angular radius of the viewer's head shadow on the
/// lit lighting presets, in the live view and every export
/// (`RenderContext::head_shadow_deg`, persisted as `AppSettings::head_shadow_deg`). The
/// slider reports whole degrees; a drag that stays on the same degree changes nothing and
/// so never restarts accumulation.
pub(in crate::gui) fn setup_head_shadow_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx = render_ctx.clone();
    let settings_store = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_head_shadow_changed(move |degrees: f32| {
            let deg = head_shadow_deg_from_slider(degrees);
            let mut ctx = RenderContext::lock(&render_ctx);
            if ctx.head_shadow_deg.to_bits() == deg.to_bits() {
                return;
            }
            ctx.head_shadow_deg = deg;
            ctx.dirty = true;
            drop(ctx);
            settings_store.update(|s| s.settings.head_shadow_deg = deg);
        });
}

/// Applies the Stone Shadow toggle to the current frame and the stored lighting.
pub(in crate::gui) fn setup_stone_shadow_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx = Arc::clone(render_ctx);
    let settings_store = Arc::clone(settings_store);
    ui.global::<SettingsModel>()
        .on_stone_shadow_changed(move |enabled| {
            let mut ctx = RenderContext::lock(&render_ctx);
            if ctx.stone_shadow != enabled {
                ctx.stone_shadow = enabled;
                ctx.dirty = true;
                drop(ctx);
                settings_store.update(|s| s.settings.stone_shadow = enabled);
            }
        });
}

/// Wires the Tray Reflection slider: the head-on reflectance of the glossy tray under the stone
/// in the live view and every export (`RenderContext::tray_reflection`, persisted as
/// `AppSettings::tray_reflection`). The slider reports a whole percent; a drag that stays on the
/// same percent changes nothing and so never restarts accumulation. The render thread resolves
/// the plane per scene generation, and the scene identity includes the value.
pub(in crate::gui) fn setup_tray_reflection_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx = Arc::clone(render_ctx);
    let settings_store = Arc::clone(settings_store);
    ui.global::<SettingsModel>()
        .on_tray_reflection_changed(move |percent: f32| {
            let reflection = tray_reflection_from_percent(percent);
            let mut ctx = RenderContext::lock(&render_ctx);
            if ctx.tray_reflection.to_bits() == reflection.to_bits() {
                return;
            }
            ctx.tray_reflection = reflection;
            ctx.dirty = true;
            drop(ctx);
            settings_store.update(|s| s.settings.tray_reflection = reflection);
        });
}

/// Wires the backdrop pill row: the lit tray the stone lies on, seen around the stone and
/// through its windows, in the live view and every export (`RenderContext::backdrop`,
/// persisted as `AppSettings::backdrop`). The render thread resolves the choice to a tray
/// radiance at the scene's own lighting preset and light pose, so changing the choice, the
/// rig or the light pose all restart the accumulation through the scene identity.
pub(in crate::gui) fn setup_backdrop_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx = render_ctx.clone();
    let settings_store = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_backdrop_changed(move |index: i32| {
            let backdrop = crate::settings::model::Backdrop::from_index(index);
            let mut ctx = RenderContext::lock(&render_ctx);
            ctx.backdrop = backdrop;
            ctx.dirty = true;
            drop(ctx);
            settings_store.update(|s| s.settings.backdrop = backdrop);
        });
}
