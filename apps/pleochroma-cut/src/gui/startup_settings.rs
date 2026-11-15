//! Applies settings loaded from disk at startup ([`apply_loaded_settings`]).
//!
//! They go into the render context and the UI's own mirrored properties, with the
//! settings-value <-> UI-pill-index conversions that go with it (`bounces_index`,
//! `resolution_index`, `local_preview_scale_index`/`from_index`,
//! `local_compute_target_index`/`from_index`, `color_space_from_index`, `find_option_index`). It
//! also holds the option-list refreshers ([`refresh_lighting_preset_options`],
//! [`refresh_material_options`]) and [`env_map_status_text`]/[`is_c_axis_override_available`],
//! which a user-initiated callback shares. This lives apart from `gui::mod` to keep that file
//! small.

use crate::{
    EditorModel, LibraryModel, LightingPresetItem, MainWindow, RemoteWorkerModel, SettingsModel,
    SolidPreviewModel, ViewportModel,
    bridge::render_thread::{RenderContext, load_env_map, resolve_material},
    gui::{
        optics::offered_lighting::{
            offered_from_label, rig_blurb, rig_group, rig_kind, rig_short_name,
        },
        remote::live_compute_target_index,
        render::sample_scale::count_to_exponent,
        show_toast,
    },
    settings::{
        LightingPreset as SavedLightingPreset, LocalComputeTarget, LocalPreviewScale, SettingsFile,
        model::{
            EXPOSURE_RANGE, LightingValues, PITCH_RANGE_RAD, clamp_head_shadow_deg,
            clamp_surface_glare, percent_from_surface_glare,
        },
    },
};
use pleochroma::{
    color::ColorSpace,
    optics::{
        LightingPreset,
        materials::{GemMaterial, OpticalCharacter},
        raytracer::LightingModel,
    },
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::sync::{Arc, Mutex};

#[path = "startup_settings/material_options.rs"]
mod material_options;
pub(in crate::gui) use material_options::refresh_material_options;

/// The material every session falls back to when a persisted `selected_material`
/// is not found. Matches `RenderContext::default()`'s starting material.
const DEFAULT_MATERIAL_NAME: &str = "Diamond";

/// Applies settings loaded from disk into the render context and into the UI's own
/// mirrored properties (`target_samples_exponent`, `resolution_index`, `bounce_index`,
/// `exposure_val`, `light_yaw_deg`, `light_pitch_deg` -- hoisted onto `MainWindow` for exactly
/// this reason, see the comment beside them in `app.slint`). The stone's size and render effects
/// are the open design's, not the file's. Called once at startup, before the render
/// thread or any callback is wired up, so there is no risk of a callback firing
/// mid-application and racing this.
pub(super) fn apply_loaded_settings(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    loaded: &SettingsFile,
) {
    let s = &loaded.settings;
    super::startup_ray::apply(ui, render_ctx, s);
    // The upload compression setting has no UI mirror: the connection layer reads it.
    crate::bridge::remote::remote_render::set_payload_choice(s.payload_encoding);
    // Parse the persisted lighting-rig label back into its enum here. `from_label` maps legacy
    // and unrecognized labels itself instead of resetting the user's choice (see
    // `pleochroma::optics::LightingPreset::from_label`).
    // A stored UV lamp falls back to the default rig in a build without `physical-color`.
    let lighting_preset = crate::gui::optics::offered_lighting::offered_from_label(&s.lighting_rig);

    // Resolve the persisted material once upfront so the render context, dropdown,
    // and UI defaults all agree on the effective name. A deleted custom material
    // must not leave mismatched state between context and UI.
    let material_options = ui.global::<ViewportModel>().get_material_options();
    let material_found = find_option_index(&material_options, &s.selected_material).is_some();
    let effective_material_name: &str = if material_found {
        &s.selected_material
    } else {
        DEFAULT_MATERIAL_NAME
    };

    apply_loaded_render_context(render_ctx, s, lighting_preset, effective_material_name);
    apply_loaded_ui_mirrors(ui, s, lighting_preset);

    if let Some(idx) = find_option_index(&material_options, effective_material_name) {
        ui.global::<ViewportModel>()
            .set_selected_material_index(idx);
    }
    if !material_found {
        show_toast(
            ui,
            &format!(
                "Material '{}' no longer exists; using {DEFAULT_MATERIAL_NAME}.",
                s.selected_material
            ),
            "info",
        );
    }
    // The crystal-axis control's enabled state depends on the STARTING material, so it is set
    // here too: `on_material_changed` only fires on a later change. `resolve_material` returns
    // `Option`, so a `None` falls back to Diamond for these two UI defaults only, the same way
    // `render::material::setup_material_changed_callback` does. Nothing that reaches the
    // tracer uses that fallback.
    let starting_material = resolve_material(
        &GemMaterial::all_materials(),
        &RenderContext::lock(render_ctx).custom_materials,
        effective_material_name,
    );
    ui.global::<SettingsModel>().set_c_axis_override_available(
        starting_material
            .as_ref()
            .is_some_and(is_c_axis_override_available),
    );
    // The RI-tolerance filter default is pushed here for the same reason: without it, a
    // session restored on any starting material would show the `.slint` placeholder RI
    // until the material changes. `ri_tolerance_center` is seeded to the same value, so the
    // tolerance band starts centred on the loaded material.
    let ri_default = starting_material.as_ref().map_or_else(
        || crate::bridge::preview_render::material_ri_at_sodium_d(&GemMaterial::diamond()),
        crate::bridge::preview_render::material_ri_at_sodium_d,
    );
    // `ri_default` is an f64; Slint's `float` property type is f32 -- a lossy but
    // harmless narrowing for a display/filter-default value in the 1.0-3.0 RI range.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Slint float properties are f32; the refractive index is in the 1.0-3.0 range"
    )]
    let ri_default_f32 = ri_default as f32;
    ui.global::<LibraryModel>()
        .set_ri_material_default(ri_default_f32);
    ui.global::<LibraryModel>()
        .set_ri_tolerance_center(ri_default_f32);

    // Reload the last-loaded HDR environment map, if any. This runs before any callback is
    // wired up, so a failure here only toasts. `s.env_map_path` is kept either way, so a
    // transient failure (an unmounted drive, say) does not forget the path.
    ui.global::<SettingsModel>()
        .set_env_map_path(s.env_map_path.clone().into());
    if s.env_map_path.is_empty() {
        ui.global::<SettingsModel>().set_env_map_loaded(false);
        ui.global::<SettingsModel>()
            .set_env_map_status(String::new().into());
    } else {
        match load_env_map(&s.env_map_path) {
            Ok(map) => {
                let status = env_map_status_text(&map, &s.env_map_path);
                ui.global::<ViewportModel>()
                    .set_env_map_name(env_map_name_from_status(&status).into());
                ui.global::<SettingsModel>()
                    .set_env_map_status(status.into());
                ui.global::<SettingsModel>().set_env_map_loaded(true);
                RenderContext::lock(render_ctx).env_map = Some(map);
            }
            Err(err) => {
                ui.global::<SettingsModel>().set_env_map_loaded(false);
                ui.global::<SettingsModel>()
                    .set_env_map_status(String::new().into());
                show_toast(
                    ui,
                    &format!("Could not reload saved HDR environment: {err}"),
                    "error",
                );
            }
        }
    }
}

/// The `RenderContext` half of [`apply_loaded_settings`]: writes every
/// render-context field the settings load restores. Split out to keep
/// [`apply_loaded_settings`] under clippy's length lint.
///
/// `material_name` is the caller's already-resolved effective name, never the
/// raw persisted value, so this cannot reintroduce stale-name mismatches.
fn apply_loaded_render_context(
    render_ctx: &Arc<Mutex<RenderContext>>,
    s: &crate::settings::model::AppSettings,
    lighting_preset: LightingPreset,
    material_name: &str,
) {
    let mut ctx = RenderContext::lock(render_ctx);
    ctx.target_samples = s.target_samples;
    ctx.width = s.render_width;
    ctx.height = s.render_height;
    ctx.max_bounces = s.max_bounces;
    // Clamped the same way the live exposure handler does -- a hand-edited or stale settings
    // file could otherwise carry an out-of-range value straight into the render loop,
    // which every live-drag path already refuses to do.
    ctx.exposure = s.exposure.clamp(EXPOSURE_RANGE.0, EXPOSURE_RANGE.1);
    // The stone's size and render effects (inclusion haze, crystal axis, frosted girdle, edge
    // rounding) belong to the open design, so the render context keeps its built-in defaults
    // here. The settings file's app-level values are read only by
    // `AppSettings::legacy_stone_defaults`, the one-shot migration onto the first design opened.
    ctx.light_yaw = s.light_yaw_deg.to_radians();
    ctx.light_pitch = s
        .light_pitch_deg
        .to_radians()
        .clamp(PITCH_RANGE_RAD.0, PITCH_RANGE_RAD.1);
    ctx.lighting_preset = lighting_preset;
    ctx.yaw = s.camera_yaw;
    ctx.pitch = crate::gui::render::camera_lighting::wrap_pitch(s.camera_pitch);
    ctx.camera_roll = crate::gui::render::camera_lighting::wrap_pitch(s.camera_roll);
    // Limited to what a renderer accepts; a damaged component centres the stone on that axis.
    ctx.camera_pan = crate::settings::model::clamp_camera_pan(s.camera_pan);
    // The Stone Pose stored with the camera pose, limited to its ranges.
    ctx.stone_pose = pleochroma::render_setup::StonePose::from_array(s.stone_pose);
    // Clamped with the live orbit camera's bounds. This runs once at startup, before any
    // design has changed the live bounding radius, so the default radius is the live value.
    let (min_distance, max_distance) = crate::gui::render::camera_lighting::orbit_distance_bounds(
        crate::gui::solid_preview::preview_state::DEFAULT_MESH_BOUNDING_RADIUS,
    );
    ctx.distance = s.camera_distance.clamp(min_distance, max_distance);
    ctx.material_name = material_name.to_string();
    ctx.denoise_enabled = s.denoise_enabled;
    ctx.backdrop = s.backdrop;
    ctx.surface_glare = clamp_surface_glare(s.surface_glare);
    ctx.stone_shadow = s.stone_shadow;
    ctx.tray_reflection = crate::settings::model::clamp_tray_reflection(s.tray_reflection);
    ctx.head_shadow_deg = clamp_head_shadow_deg(s.head_shadow_deg);
    // The HDR map's turn: wrapped, and a damaged value reads as the map as loaded.
    let hdr = pleochroma::renderer::env_map::HdrOrientation {
        yaw: s.env_map_yaw_deg.to_radians(),
        follows_camera: s.env_map_follows_camera,
    }
    .sanitized();
    ctx.hdr_yaw = hdr.yaw;
    ctx.hdr_follows_camera = hdr.follows_camera;
    // The cutter's faceting machine (a per-user preference, never in a design).
    ctx.set_faceting_machine(crate::settings::model::set_current_faceting_machine(
        &s.faceting_machine,
    ));
    // Local preview and the live compute targets. `camera_moving` keeps its default
    // (`false`): `gui::remote::poll_tick` derives it from camera polling within the first
    // tick after the window opens, so a settings file has no say in it.
    ctx.local_preview_scale = s.local_preview_scale;
    ctx.live_compute_target = s.live_compute_target;
    ctx.local_compute_target = s.local_compute_target;
    // The Preferences switch for adaptive sampling of a still view (off by default).
    ctx.adaptive_viewport = s.adaptive_viewport;
    ctx.path_cache_viewport = s.path_cache_viewport;
    // The Preferences switch for instant lighting changes in the Ray-Traced view (off by default).
    ctx.instant_lighting = s.instant_lighting;
    // The beam switches (both off by default); the second counts only with the first. Without
    // the `beam` feature the keys stay in the settings file but never reach the render context.
    // A block, because stable Rust takes no `cfg` on an assignment statement.
    #[cfg(feature = "beam")]
    {
        ctx.beam_viewport = s.beam_viewport;
        ctx.beam_instant_lighting = super::preferences::effective_beam_instant_lighting(
            s.beam_viewport,
            s.beam_instant_lighting,
        );
    }
    ctx.dirty = true;
}

/// The UI-mirrored-properties half of [`apply_loaded_settings`]: pushes every
/// `SettingsModel`/`LibraryModel`/`SolidPreviewModel`/
/// `EditorModel`/`ViewportModel` property this settings load restores. Split out
/// purely to keep that function under clippy's function-length lint.
/// The HDR map's turn (sanitized as the render loop reads it) and its follow-the-camera switch.
fn apply_loaded_env_map_mirrors(ui: &MainWindow, s: &crate::settings::model::AppSettings) {
    ui.global::<SettingsModel>().set_env_map_yaw_deg(
        pleochroma::renderer::env_map::HdrOrientation {
            yaw: s.env_map_yaw_deg.to_radians(),
            follows_camera: false,
        }
        .sanitized()
        .yaw
        .to_degrees(),
    );
    ui.global::<SettingsModel>()
        .set_env_map_follows_camera(s.env_map_follows_camera);
}

fn apply_loaded_ui_mirrors(
    ui: &MainWindow,
    s: &crate::settings::model::AppSettings,
    lighting_preset: LightingPreset,
) {
    // The slider stores an exponent and the settings file a sample count; see
    // `gui::render::sample_scale` for the conversion and its inverse.
    ui.global::<SettingsModel>()
        .set_target_samples_exponent(count_to_exponent(s.target_samples) as f32);
    ui.global::<SettingsModel>()
        .set_resolution_index(resolution_index(s.render_width, s.render_height));
    ui.global::<SettingsModel>()
        .set_preview_size_val(s.preview_size as f32);
    ui.global::<SettingsModel>()
        .set_thumbnail_renderer_index(i32::from(s.thumbnail_renderer.index()));
    ui.global::<SettingsModel>()
        .set_thumbnail_backdrop_index(s.thumbnail_backdrop.index());
    crate::bridge::preview_render::set_thumbnail_backdrop_index(s.thumbnail_backdrop.index());
    ui.global::<SettingsModel>()
        .set_preview_spp_val(s.preview_spp as f32);
    ui.global::<SettingsModel>()
        .set_bounce_index(bounces_index(s.max_bounces));
    ui.global::<SettingsModel>()
        .set_quality_preset_index(s.quality_preset_shown().index());
    // Clamped identically to `apply_loaded_render_context`'s own write into
    // `RenderContext` (same bounds the live handlers use) -- so a hand-edited or
    // stale out-of-range value shows the slider at the value ACTUALLY being traced,
    // not the raw unclamped figure the file happened to carry.
    ui.global::<SettingsModel>()
        .set_exposure_val(s.exposure.clamp(EXPOSURE_RANGE.0, EXPOSURE_RANGE.1));
    ui.global::<SettingsModel>()
        .set_backdrop_index(s.backdrop.index());
    ui.global::<SettingsModel>()
        .set_surface_glare_pct(percent_from_surface_glare(s.surface_glare));
    ui.global::<SettingsModel>()
        .set_head_shadow_deg(clamp_head_shadow_deg(s.head_shadow_deg));
    ui.global::<SettingsModel>()
        .set_stone_shadow(s.stone_shadow);
    ui.global::<SettingsModel>().set_tray_reflection_pct(
        crate::settings::model::percent_from_tray_reflection(s.tray_reflection),
    );
    crate::gui::render::stone_pose::push_stone_pose_to_controls(
        ui,
        pleochroma::render_setup::StonePose::from_array(s.stone_pose),
        false,
    );
    apply_loaded_env_map_mirrors(ui, s);
    // (The stone effects and width controls show the open design's values, not the file's.)
    ui.global::<SettingsModel>()
        .set_local_preview_scale_index(local_preview_scale_index(s.local_preview_scale));
    ui.global::<SettingsModel>()
        .set_live_compute_target_index(live_compute_target_index(s.live_compute_target));
    ui.global::<SettingsModel>()
        .set_local_compute_target_index(local_compute_target_index(s.local_compute_target));
    ui.global::<SettingsModel>()
        .set_contribute_to_final_picture(s.contribute_to_final_picture);
    // The tilt-video section opens with the settings of its last run, and the Render & Output
    // tab shows the remembered output defaults (the still export dialog seeds itself each time
    // it opens). This also sets the section's compute pill from the persisted choice.
    crate::gui::render::output_defaults::seed_tilt_dialog(ui, s);
    crate::gui::render::output_defaults::publish_summaries(
        ui,
        &s.output.clone().sanitised(),
        s.tilt_video_compute_target,
    );
    ui.global::<SettingsModel>()
        .set_light_yaw_deg(s.light_yaw_deg);
    // Clamped like `ctx.light_pitch` in `apply_loaded_render_context`, so the slider shows
    // the pitch the render loop uses (`PITCH_RANGE_RAD`, the live drag's bound).
    ui.global::<SettingsModel>().set_light_pitch_deg(
        s.light_pitch_deg
            .to_radians()
            .clamp(PITCH_RANGE_RAD.0, PITCH_RANGE_RAD.1)
            .to_degrees(),
    );
    // Restore the library panel's collapsed/expanded state.
    ui.global::<LibraryModel>()
        .set_panel_collapsed(s.library_panel_collapsed);
    // The Solid viewport always starts in Solid mode (0), whatever
    // `AppSettings::solid_view_mode` remembers. The stored value is kept and still follows
    // the user's in-session choice; it is only not applied at startup. 0 is also the
    // property's default, so this normally fires no `changed` handler.
    ui.global::<SolidPreviewModel>().set_view_mode(0);
    // The Live Render tab, where the app opens, also starts in Solid mode (0), whatever
    // `AppSettings::live_view_mode` remembers. The stored value is kept, as above, and no
    // `live_view_mode_changed` handler is connected yet at this point in startup.
    ui.global::<ViewportModel>().set_live_view_mode(0);
    // The "Edit" sub-tab's auto-solve budget (`gui::editor::auto_solve`).
    ui.global::<EditorModel>()
        .set_auto_solve_budget_ms(i32::try_from(s.editor_auto_solve_budget_ms).unwrap_or(i32::MAX));
    ui.global::<ViewportModel>()
        .set_selected_lighting_index(lighting_preset.index());
    ui.global::<ViewportModel>()
        .set_lighting_supports_stone_shadow(ModelRc::new(VecModel::from(
            LightingPreset::ALL
                .iter()
                .map(|preset| *preset != LightingPreset::Aset && !preset.is_uv_lamp())
                .collect::<Vec<bool>>(),
        )));
    // Which combo entries are Studio rigs (parallel to `LightingPreset::ALL`): the Optimize
    // tab's "measured against this rig's light sources" note keys on it.
    ui.global::<ViewportModel>()
        .set_lighting_is_studio(ModelRc::new(VecModel::from(
            LightingPreset::ALL
                .iter()
                .map(|preset| preset.model() == LightingModel::Studio)
                .collect::<Vec<bool>>(),
        )));
    // The rig picture kind, group, short name and blurb of every combo entry (parallel to
    // `LightingPreset::ALL`): the viewport's rig menu and the Lighting tab's rig card and gallery.
    push_lighting_rig_tables(ui);
    // The two viewport hints the cutter has dealt with (shown once; never reset by a tab reset).
    ui.global::<ViewportModel>()
        .set_gestures_seen(s.gestures_seen);
    ui.global::<ViewportModel>()
        .set_figures_folded(s.figures_folded);
    // File > Open Recent (`MainWindow.recent_native_files`, a root property of `ui/app.slint`).
    // `gui::editor::native_io` keeps it current on every Save and Open through the same
    // `SettingsPersister` snapshot seeded from `s`, so the list on screen matches the list
    // written on exit. This seeds it once, before those callbacks are wired.
    ui.set_recent_native_files(ModelRc::new(VecModel::from(
        s.recent_native_files
            .iter()
            .cloned()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
    // The Simple/Advanced switch, UI scale, high contrast, larger handles and the
    // Solid viewport's remembered Snap/Slice pills -- see `gui::preferences`.
    crate::gui::preferences::apply_loaded(ui, s);
}

/// The Parameters panel's "Loaded: <file> (`WxH`)" status line for a decoded
/// [`pleochroma::renderer::env_map::EnvironmentMap`] -- shared by the startup
/// reload in `apply_loaded_settings` and the user-initiated load in
/// `setup_environment_map_callbacks` so both report the map identically. Shows the
/// file name alone (not the full path, which can be long and is already visible in the
/// path field above it).
pub(in crate::gui) fn env_map_status_text(
    map: &pleochroma::renderer::env_map::EnvironmentMap,
    path: &str,
) -> String {
    let name = std::path::Path::new(path)
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned());
    format!("Loaded: {name} ({}\u{d7}{})", map.width(), map.height())
}

/// Whether the crystal-axis orientation override has any effect on
/// `material` -- `false` for an isotropic material (Diamond, Spinel, Cubic Zirconia,
/// and any custom material with `birefringence_delta == 0.0`), whose optic axis is
/// physically meaningless: there is no birefringence to orient. Drives
/// `MainWindow.c_axis_override_available`, which the Parameters panel uses to grey
/// out the control and explain why, rather than silently letting the user drag a
/// slider that does nothing.
pub(in crate::gui) fn is_c_axis_override_available(material: &GemMaterial) -> bool {
    material.optical_character != OpticalCharacter::Isotropic
}

/// Index of the bounce-count pill (4/8/12/24/64/128 at indices 0-5) that `bounces`
/// highlights. An exact match maps directly. Any other value, such as a hand-edited one,
/// picks the nearest rung of `settings::model::BOUNCE_LADDER` by absolute distance. The
/// ladder is uneven (24 to 64 is a 40-bounce gap), so a fixed fallback would misplace
/// values such as 40 or 96. Only the highlight uses this: the stored `max_bounces` is never
/// clamped (see `on_bounces_changed` in `render::quality`).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "the result is an index into the bounce ladder, a handful of rungs"
)]
const fn bounces_index(bounces: u32) -> i32 {
    // The ladder and the nearest-rung rule are Rust's one copy
    // (`settings::model::BOUNCE_LADDER`); Slint reads the same ladder.
    crate::settings::model::bounce_index(bounces) as i32
}

/// Pushes every Render & Output tab value of `s` into the render context and the controls
/// that mirror it: the live quality, the compute targets, the denoise switch, the output
/// defaults and the library thumbnail values. Used after "Reset this tab"; start-up has its
/// own two halves above. Like the live handlers, an actual live-compute change releases a
/// settled remote epoch.
pub(in crate::gui) fn apply_render_output_settings(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    s: &crate::settings::model::AppSettings,
) {
    super::startup_ray::apply(ui, render_ctx, s);
    {
        let mut ctx = RenderContext::lock(render_ctx);
        ctx.target_samples = s.target_samples;
        ctx.width = s.render_width;
        ctx.height = s.render_height;
        ctx.max_bounces = s.max_bounces;
        ctx.local_preview_scale = s.local_preview_scale;
        ctx.local_compute_target = s.local_compute_target;
        if ctx.live_compute_target != s.live_compute_target {
            if ctx.remote_active || ctx.live_epoch.is_some() {
                ctx.release_remote();
            }
            ctx.live_compute_target = s.live_compute_target;
        }
        ctx.denoise_enabled = s.denoise_enabled;
        ctx.redisplay_requested = true;
        ctx.dirty = true;
    }
    let settings = ui.global::<SettingsModel>();
    settings.set_target_samples_exponent(count_to_exponent(s.target_samples) as f32);
    settings.set_resolution_index(resolution_index(s.render_width, s.render_height));
    settings.set_preview_size_val(s.preview_size as f32);
    settings.set_thumbnail_renderer_index(i32::from(s.thumbnail_renderer.index()));
    settings.set_thumbnail_backdrop_index(s.thumbnail_backdrop.index());
    crate::bridge::preview_render::set_thumbnail_backdrop_index(s.thumbnail_backdrop.index());
    settings.set_preview_spp_val(s.preview_spp as f32);
    settings.set_bounce_index(bounces_index(s.max_bounces));
    settings.set_quality_preset_index(s.quality_preset_shown().index());
    settings.set_local_preview_scale_index(local_preview_scale_index(s.local_preview_scale));
    settings.set_live_compute_target_index(live_compute_target_index(s.live_compute_target));
    settings.set_local_compute_target_index(local_compute_target_index(s.local_compute_target));
    settings.set_contribute_to_final_picture(s.contribute_to_final_picture);
    ui.global::<RemoteWorkerModel>()
        .set_denoise_enabled(s.denoise_enabled);
    crate::gui::render::output_defaults::seed_tilt_dialog(ui, s);
    crate::gui::render::output_defaults::publish_summaries(
        ui,
        &s.output.clone().sanitised(),
        s.tilt_video_compute_target,
    );
}

/// Pushes the live-view quality of `s` into the Render tab's controls: the Target Samples slider,
/// the resolution and bounce lists, and the preset tile. Used after a preset is picked; the render
/// context is the caller's.
pub(in crate::gui) fn push_quality_controls(
    ui: &MainWindow,
    s: &crate::settings::model::AppSettings,
) {
    let settings = ui.global::<SettingsModel>();
    settings.set_target_samples_exponent(count_to_exponent(s.target_samples) as f32);
    settings.set_resolution_index(resolution_index(s.render_width, s.render_height));
    settings.set_bounce_index(bounces_index(s.max_bounces));
    settings.set_quality_preset_index(s.quality_preset_shown().index());
}

/// Index of the "Render Resolution" pill (640x480, 800x600, 1280x720, 1920x1080 at indices
/// 0-3), used only to highlight the closest pill at startup. Any other pair, such as a
/// hand-edited value, highlights 800x600 (index 1), the default. The stored size is never
/// clamped by this (see `AppSettings::render_width`).
const fn resolution_index(width: u32, height: u32) -> i32 {
    match (width, height) {
        (640, 480) => 0,
        (1280, 720) => 2,
        (1920, 1080) => 3,
        _ => 1, // 800x600 and anything unrecognized
    }
}

/// Reverse of the mapping baked into the Parameters panel's "Motion Preview
/// Resolution" pill selector (Off/Half/Quarter at indices 0-2) -- same fixed-list
/// treatment as `resolution_index` above, used to seed the pill at startup from a
/// persisted `LocalPreviewScale`.
const fn local_preview_scale_index(scale: LocalPreviewScale) -> i32 {
    match scale {
        LocalPreviewScale::Off => 0,
        LocalPreviewScale::Half => 1,
        LocalPreviewScale::Quarter => 2,
    }
}

/// Inverse of [`local_preview_scale_index`]: what `on_local_preview_scale_changed`
/// (in `render::quality`) converts the pill's clicked index back into. Falls back to `Off`
/// for anything outside `0..=2`, which the pill can never send.
pub(in crate::gui) const fn local_preview_scale_from_index(index: i32) -> LocalPreviewScale {
    match index {
        1 => LocalPreviewScale::Half,
        2 => LocalPreviewScale::Quarter,
        _ => LocalPreviewScale::Off,
    }
}

/// Reverse of the mapping baked into the Parameters panel's "Local Compute" pill
/// selector (CPU/CPU+GPU/GPU only at indices 0-2) -- same fixed-list treatment as
/// `local_preview_scale_index` above, used to seed the pill at startup from a
/// persisted `LocalComputeTarget`.
const fn local_compute_target_index(target: LocalComputeTarget) -> i32 {
    match target {
        LocalComputeTarget::Cpu => 0,
        LocalComputeTarget::CpuGpu => 1,
        LocalComputeTarget::Gpu => 2,
    }
}

/// Inverse of [`local_compute_target_index`]: what `on_local_compute_target_changed` (in
/// `render::quality`) converts the pill's clicked index back into. Falls back to `CpuGpu`,
/// matching `LocalComputeTarget::default()`, for anything outside `0..=2`.
pub(in crate::gui) const fn local_compute_target_from_index(index: i32) -> LocalComputeTarget {
    match index {
        0 => LocalComputeTarget::Cpu,
        2 => LocalComputeTarget::Gpu,
        _ => LocalComputeTarget::CpuGpu,
    }
}

/// Finds `needle`'s index in a Slint `[string]` model, to restore `ComboBox` selections
/// (material, lighting rig) from persisted names. Returns `None` when the name is absent,
/// for example a deleted custom material.
///
/// Case-insensitive, like the other material and lighting name lookups (`resolve_material`).
#[must_use]
pub(in crate::gui) fn find_option_index(
    options: &ModelRc<SharedString>,
    needle: &str,
) -> Option<i32> {
    (0..options.row_count()).find_map(|i| {
        let matches = options
            .row_data(i)
            .is_some_and(|s| s.as_str().eq_ignore_ascii_case(needle));
        matches.then_some(i).and_then(|row| i32::try_from(row).ok())
    })
}

/// Rebuilds the `MainWindow.lighting_presets` model from the settings store's current
/// preset list. Called after startup load and after every create/rename/delete so the
/// Parameters panel's preset rows stay in sync with what's actually persisted.
pub(in crate::gui) fn refresh_lighting_preset_options(
    ui: &MainWindow,
    presets: &[SavedLightingPreset],
) {
    let items: Vec<LightingPresetItem> = presets
        .iter()
        .map(|p| LightingPresetItem {
            name: p.name.clone().into(),
            built_in: p.built_in,
            export_usable: p.export_usable,
            has_env_map: p.env_map_path.is_some(),
            // Not export-dialog state -- see `LightingPresetItem.selected`'s own doc
            // comment; this list is the Parameters panel's, which never reads it.
            selected: false,
        })
        .collect();
    let kinds: Vec<i32> = presets
        .iter()
        .map(|p| rig_kind(offered_from_label(&p.lighting_rig)))
        .collect();
    let design_index = crate::gui::render::design_lighting::design_values()
        .map_or(-1, |values| design_preset_index(presets, &values));
    let viewport = ui.global::<ViewportModel>();
    viewport.set_lighting_presets(ModelRc::new(VecModel::from(items)));
    viewport.set_lighting_preset_kinds(ModelRc::new(VecModel::from(kinds)));
    viewport.set_design_lighting_preset(design_index);
}

/// Index into `presets` of the first saved lighting whose lighting fields equal `design`'s (the
/// camera pose and the export flag are not compared), or -1 when none does or no design lighting
/// is remembered. Feeds `ViewportModel.design_lighting_preset`.
#[must_use]
pub(in crate::gui) fn design_preset_index(
    presets: &[SavedLightingPreset],
    design: &LightingValues,
) -> i32 {
    presets
        .iter()
        .position(|p| same_lighting(design, &p.values()))
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(-1)
}

/// Whether `a` and `b` set the same light: every lighting field equal (floats within a
/// thousandth), the camera pose ignored.
fn same_lighting(a: &LightingValues, b: &LightingValues) -> bool {
    let close = |x: f32, y: f32| (x - y).abs() <= 1e-3;
    a.lighting_rig == b.lighting_rig
        && close(a.light_yaw_deg, b.light_yaw_deg)
        && close(a.light_pitch_deg, b.light_pitch_deg)
        && close(a.exposure, b.exposure)
        && close(a.surface_glare, b.surface_glare)
        && a.backdrop == b.backdrop
        && close(a.head_shadow_deg, b.head_shadow_deg)
        && a.stone_shadow == b.stone_shadow
        && close(a.tray_reflection, b.tray_reflection)
        && a.env_map_path == b.env_map_path
        && close(a.env_map_yaw_deg, b.env_map_yaw_deg)
        && a.env_map_follows_camera == b.env_map_follows_camera
}

/// Fills the four rig tables of `ViewportModel` (kind, group, short name, blurb), one entry per
/// `LightingPreset::ALL`, in the same order as `lighting_options`.
fn push_lighting_rig_tables(ui: &MainWindow) {
    let viewport = ui.global::<ViewportModel>();
    viewport.set_lighting_kinds(ModelRc::new(VecModel::from(
        LightingPreset::ALL
            .iter()
            .map(|p| rig_kind(*p))
            .collect::<Vec<i32>>(),
    )));
    viewport.set_lighting_groups(ModelRc::new(VecModel::from(
        LightingPreset::ALL
            .iter()
            .map(|p| rig_group(*p))
            .collect::<Vec<i32>>(),
    )));
    viewport.set_lighting_short_names(ModelRc::new(VecModel::from(
        LightingPreset::ALL
            .iter()
            .map(|p| SharedString::from(rig_short_name(*p)))
            .collect::<Vec<_>>(),
    )));
    viewport.set_lighting_blurbs(ModelRc::new(VecModel::from(
        LightingPreset::ALL
            .iter()
            .map(|p| SharedString::from(rig_blurb(*p)))
            .collect::<Vec<_>>(),
    )));
}

/// The file name an HDR map's status line names ("Loaded: sky.hdr (4096x2048)" gives "sky.hdr"),
/// or "" when no map is loaded. [`env_map_status_text`] writes the line; this is its inverse, so
/// one `SettingsModel.env_map_status` handler covers every place that loads or clears a map.
#[must_use]
pub(in crate::gui) fn env_map_name_from_status(status: &str) -> String {
    let Some(rest) = status.strip_prefix("Loaded: ") else {
        return String::new();
    };
    // The size is the last parenthesis; a file name may hold parentheses of its own.
    rest.rfind(" (")
        .map_or(rest, |cut| &rest[..cut])
        .to_string()
}

/// Inverse of `export_dialog.slint`'s colour space pill selector (sRGB, Display P3,
/// Rec.2020 at indices 0-2), with the same fallback as [`local_preview_scale_from_index`].
/// Falls back to `ColorSpace::Srgb`, index 0, for any value the pill can never send.
///
/// `ColorSpace::AcesCg` has no index: the picker does not offer it, because a scene-linear
/// space does not belong in an 8-bit PNG export (see `bridge::export_thread`).
pub(in crate::gui) const fn color_space_from_index(index: i32) -> ColorSpace {
    match index {
        1 => ColorSpace::DisplayP3,
        2 => ColorSpace::Rec2020,
        _ => ColorSpace::Srgb,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_env_map_name_is_read_back_from_the_status_line() {
        assert_eq!(
            env_map_name_from_status("Loaded: sky.hdr (4096\u{d7}2048)"),
            "sky.hdr"
        );
        assert_eq!(
            env_map_name_from_status("Loaded: studio (v2).hdr (64\u{d7}32)"),
            "studio (v2).hdr"
        );
        assert_eq!(env_map_name_from_status(""), "");
        assert_eq!(env_map_name_from_status("something else"), "");
    }

    #[test]
    fn the_design_preset_is_the_saved_lighting_with_equal_lighting_fields() {
        let design = LightingValues::default();
        let mut moved = design.clone();
        moved.light_yaw_deg += 20.0;
        let mut with_camera = design.clone();
        with_camera.camera_yaw = Some(0.5);
        with_camera.camera_pitch = Some(0.2);
        assert!(same_lighting(&design, &with_camera));
        assert!(!same_lighting(&design, &moved));
        assert_eq!(design_preset_index(&[], &design), -1);
    }

    #[test]
    fn bounces_index_round_trips_all_six_pill_values() {
        assert_eq!(bounces_index(4), 0);
        assert_eq!(bounces_index(8), 1);
        assert_eq!(bounces_index(12), 2);
        assert_eq!(bounces_index(24), 3);
        assert_eq!(bounces_index(64), 4);
        assert_eq!(bounces_index(128), 5);
    }

    /// The start-up pill index and the `nearest_bounce_cap` lookup are both views of
    /// `BOUNCE_LADDER`: index -> rung -> index is the identity for every rung.
    #[test]
    fn bounces_index_and_the_cap_lookup_agree_with_the_one_ladder() {
        use crate::settings::model::{BOUNCE_LADDER, nearest_bounce_cap};
        for (i, rung) in BOUNCE_LADDER.iter().enumerate() {
            let index = i32::try_from(i).expect("a small index");
            assert_eq!(bounces_index(*rung), index);
            assert_eq!(nearest_bounce_cap(index), *rung);
        }
    }

    #[test]
    fn bounces_index_picks_the_nearest_rung_for_unknown_values() {
        // 16 is not a rung: 4 away from 12 and 8 away from 24, so the nearest rung is 12.
        assert_eq!(bounces_index(16), 2);
        // Below the lowest rung and above the highest rung both clamp to the nearest
        // end of the ladder rather than falling back to the default (12).
        assert_eq!(bounces_index(1), 0);
        assert_eq!(bounces_index(999), 5);
        // Equidistant between 24 and 64 (20 either way): the tie resolves to the lower rung.
        assert_eq!(bounces_index(44), 3);
    }

    #[test]
    fn resolution_index_round_trips_all_four_pill_values() {
        assert_eq!(resolution_index(640, 480), 0);
        assert_eq!(resolution_index(800, 600), 1);
        assert_eq!(resolution_index(1280, 720), 2);
        assert_eq!(resolution_index(1920, 1080), 3);
    }

    #[test]
    fn resolution_index_falls_back_to_800x600_for_unknown_values() {
        assert_eq!(resolution_index(1, 1), 1);
        assert_eq!(resolution_index(3840, 2160), 1);
        // A mismatched pair (e.g. a hand-edited file with one dimension changed but not
        // the other) must not accidentally match a pill via one coordinate alone.
        assert_eq!(resolution_index(640, 600), 1);
    }

    #[test]
    fn local_preview_scale_index_round_trips_all_three_pill_values() {
        for scale in [
            LocalPreviewScale::Off,
            LocalPreviewScale::Half,
            LocalPreviewScale::Quarter,
        ] {
            let idx = local_preview_scale_index(scale);
            assert_eq!(
                local_preview_scale_from_index(idx),
                scale,
                "scale={scale:?}"
            );
        }
        assert_eq!(local_preview_scale_index(LocalPreviewScale::Off), 0);
        assert_eq!(local_preview_scale_index(LocalPreviewScale::Half), 1);
        assert_eq!(local_preview_scale_index(LocalPreviewScale::Quarter), 2);
    }

    #[test]
    fn local_preview_scale_from_index_falls_back_to_off_for_unknown_values() {
        assert_eq!(local_preview_scale_from_index(-1), LocalPreviewScale::Off);
        assert_eq!(local_preview_scale_from_index(99), LocalPreviewScale::Off);
    }

    #[test]
    fn local_compute_target_index_round_trips_all_three_pill_values() {
        for target in [
            LocalComputeTarget::Cpu,
            LocalComputeTarget::CpuGpu,
            LocalComputeTarget::Gpu,
        ] {
            let idx = local_compute_target_index(target);
            assert_eq!(
                local_compute_target_from_index(idx),
                target,
                "target={target:?}"
            );
        }
        assert_eq!(local_compute_target_index(LocalComputeTarget::Cpu), 0);
        assert_eq!(local_compute_target_index(LocalComputeTarget::CpuGpu), 1);
        assert_eq!(local_compute_target_index(LocalComputeTarget::Gpu), 2);
    }

    #[test]
    fn local_compute_target_from_index_falls_back_to_cpu_gpu_for_unknown_values() {
        assert_eq!(
            local_compute_target_from_index(-1),
            LocalComputeTarget::CpuGpu
        );
        assert_eq!(
            local_compute_target_from_index(99),
            LocalComputeTarget::CpuGpu
        );
    }

    #[test]
    fn color_space_from_index_maps_all_three_pill_values() {
        assert_eq!(color_space_from_index(0), ColorSpace::Srgb);
        assert_eq!(color_space_from_index(1), ColorSpace::DisplayP3);
        assert_eq!(color_space_from_index(2), ColorSpace::Rec2020);
    }

    #[test]
    fn color_space_from_index_falls_back_to_srgb_for_unknown_values() {
        assert_eq!(color_space_from_index(-1), ColorSpace::Srgb);
        assert_eq!(color_space_from_index(99), ColorSpace::Srgb);
    }

    #[test]
    fn find_option_index_locates_an_exact_match() {
        let options: ModelRc<SharedString> = ModelRc::new(VecModel::from(vec![
            SharedString::from("Diamond"),
            SharedString::from("Sapphire"),
            SharedString::from("Ruby"),
        ]));
        assert_eq!(find_option_index(&options, "Sapphire"), Some(1));
        assert_eq!(find_option_index(&options, "Ruby"), Some(2));
    }

    #[test]
    fn find_option_index_matches_case_insensitively() {
        let options: ModelRc<SharedString> =
            ModelRc::new(VecModel::from(vec![SharedString::from("My Sapphire")]));
        assert_eq!(find_option_index(&options, "my sapphire"), Some(0));
        assert_eq!(find_option_index(&options, "MY SAPPHIRE"), Some(0));
    }

    #[test]
    fn find_option_index_returns_none_when_absent_rather_than_guessing() {
        let options: ModelRc<SharedString> =
            ModelRc::new(VecModel::from(vec![SharedString::from("Diamond")]));
        assert_eq!(find_option_index(&options, "Moissanite"), None);
    }

    #[test]
    fn find_option_index_on_an_empty_model_returns_none() {
        let options: ModelRc<SharedString> =
            ModelRc::new(VecModel::from(Vec::<SharedString>::new()));
        assert_eq!(find_option_index(&options, "anything"), None);
    }
}
