//! The named-lighting and view-preset callbacks: save (lighting only), save (full view with
//! the camera pose), apply, rename, delete, and toggle export-usable.

use crate::{
    MainWindow, SettingsModel, ViewportModel,
    bridge::{
        export_thread::filename_template::civil_from_unix_seconds,
        render_thread::{RenderContext, load_env_map},
    },
    gui::{
        env_map_status_text, refresh_lighting_preset_options, show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
    settings::{
        LightingPreset as SavedLightingPreset, SettingsFile, SettingsPersister,
        model::{LightingValues, PITCH_RANGE_RAD, percent_from_surface_glare},
    },
};
use pleochroma::{
    optics::LightingPreset, render_setup::StonePose, renderer::env_map::EnvironmentMap,
};
use slint::{ComponentHandle, SharedString};
use std::{
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

/// Wires every named-lighting and view-preset callback. A thin orchestrator over one helper per
/// callback, kept apart to stay under clippy's function-length lint.
pub(in crate::gui) fn setup_lighting_preset_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    mesh_bounding_radius: &Arc<Mutex<f64>>,
    preview_state: &Arc<SolidPreviewState>,
) {
    setup_suggest_lighting_name_callback(ui, render_ctx, settings_store);
    setup_save_lighting_preset_callback(ui, render_ctx, settings_store);
    setup_save_view_preset_callback(ui, render_ctx, settings_store);
    setup_apply_lighting_preset_callback(
        ui,
        render_ctx,
        settings_store,
        mesh_bounding_radius,
        preview_state,
    );
    setup_rename_lighting_preset_callback(ui, settings_store);
    setup_delete_lighting_preset_callback(ui, settings_store);
    setup_toggle_preset_export_usable_callback(ui, settings_store);
}

/// Reads the fields both save paths capture: the lighting rig and the HDR environment map path
/// (`None` when no map is loaded). The map is part of the look, so both saves keep it; only the
/// camera pose tells them apart (see `LightingPreset::env_map_path`).
struct CommonViewFields {
    light_yaw_deg: f32,
    light_pitch_deg: f32,
    exposure: f32,
    lighting_rig: String,
    camera_distance: f32,
    env_map_path: Option<String>,
    surface_glare: f32,
    backdrop: crate::settings::model::Backdrop,
    head_shadow_deg: f32,
    stone_shadow: bool,
    tray_reflection: f32,
    /// The HDR map's yaw in degrees, `0..360`.
    env_map_yaw_deg: f32,
    env_map_follows_camera: bool,
}

fn read_common_view_fields(
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) -> CommonViewFields {
    let (
        light_yaw_deg,
        light_pitch_deg,
        exposure,
        lighting_rig,
        camera_distance,
        surface_glare,
        backdrop,
        head_shadow_deg,
        stone_shadow,
        tray_reflection,
        env_map_yaw_deg,
        env_map_follows_camera,
    ) = {
        let ctx = RenderContext::lock(render_ctx);
        (
            ctx.light_yaw.to_degrees(),
            ctx.light_pitch.to_degrees(),
            ctx.exposure,
            ctx.lighting_preset.label().to_string(),
            ctx.distance,
            ctx.surface_glare,
            ctx.backdrop,
            ctx.head_shadow_deg,
            ctx.stone_shadow,
            ctx.tray_reflection,
            ctx.hdr_yaw.to_degrees().rem_euclid(360.0),
            ctx.hdr_follows_camera,
        )
    };
    // The path comes from the settings store: the render context holds only the decoded map,
    // with no path. An empty `AppSettings::env_map_path` means no map and becomes `None`.
    let env_map_path = {
        let path = settings_store.snapshot().settings.env_map_path;
        (!path.is_empty()).then_some(path)
    };
    CommonViewFields {
        light_yaw_deg,
        light_pitch_deg,
        exposure,
        lighting_rig,
        camera_distance,
        env_map_path,
        surface_glare,
        backdrop,
        head_shadow_deg,
        stone_shadow,
        tray_reflection,
        env_map_yaw_deg,
        env_map_follows_camera,
    }
}

/// `base`, or `base (2)`, `base (3)`, ... when a saved lighting of that name exists already,
/// so a name offered for a new save never silently overwrites one.
fn unique_name(base: String, taken: &[String]) -> String {
    let is_taken = |name: &str| taken.iter().any(|existing| existing == name);
    if !is_taken(&base) {
        return base;
    }
    let mut n = 2_u32;
    loop {
        let candidate = format!("{base} ({n})");
        if !is_taken(&candidate) || n == u32::MAX {
            return candidate;
        }
        n += 1;
    }
}

/// Wires `ViewportModel.suggest_lighting_name`, the default name of a new saved lighting:
/// "<rig> - <date>" ([`SavedLightingPreset::default_save_name`]), made unique among the saved
/// lightings. The date is today's UTC date as `YYYY-MM-DD` (the app has no time-zone database).
fn setup_suggest_lighting_name_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx = render_ctx.clone();
    let settings_store = settings_store.clone();
    ui.global::<ViewportModel>()
        .on_suggest_lighting_name(move || {
            let rig = RenderContext::lock(&render_ctx)
                .lighting_preset
                .label()
                .to_string();
            let seconds = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0));
            let (year, month, day, ..) = civil_from_unix_seconds(seconds);
            let date = format!("{year:04}-{month:02}-{day:02}");
            let taken: Vec<String> = settings_store
                .snapshot()
                .presets
                .iter()
                .map(|preset| preset.name.clone())
                .collect();
            unique_name(SavedLightingPreset::default_save_name(&rig, &date), &taken).into()
        });
}

/// Saves the live lighting rig (light yaw and pitch, exposure, rig, camera distance, and the
/// loaded HDR map path if any) as a preset, overwriting a user preset of the same name. It
/// writes no camera pose (`camera_yaw`/`camera_pitch: None`); `setup_save_view_preset_callback`
/// is the save that captures one. See `settings::model::LightingPreset`.
fn setup_save_lighting_preset_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx_save = render_ctx.clone();
    let settings_store_save = settings_store.clone();
    let ui_weak_save = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_save_lighting_preset(move |name: SharedString| {
            let fields = read_common_view_fields(&render_ctx_save, &settings_store_save);
            let preset = SavedLightingPreset {
                name: name.to_string(),
                built_in: false,
                light_yaw_deg: fields.light_yaw_deg,
                light_pitch_deg: fields.light_pitch_deg,
                exposure: fields.exposure,
                lighting_rig: fields.lighting_rig,
                camera_distance: fields.camera_distance,
                camera_yaw: None,
                camera_pitch: None,
                camera_roll: None,
                camera_pan: None,
                stone_pose: None,
                env_map_path: fields.env_map_path,
                export_usable: false,
                surface_glare: fields.surface_glare,
                backdrop: fields.backdrop,
                head_shadow_deg: fields.head_shadow_deg,
                stone_shadow: fields.stone_shadow,
                tray_reflection: fields.tray_reflection,
                env_map_yaw_deg: fields.env_map_yaw_deg,
                env_map_follows_camera: fields.env_map_follows_camera,
                product_shot: false,
            };

            let mut result = Ok(());
            settings_store_save.update(|s| result = s.upsert_user_preset(preset.clone()));

            let Some(ui) = ui_weak_save.upgrade() else {
                return;
            };
            match result {
                Ok(()) => {
                    refresh_lighting_preset_options(&ui, &settings_store_save.snapshot().presets);
                    show_toast(&ui, &format!("Saved lighting '{name}'"), "success");
                }
                Err(err) => show_toast(&ui, &err, "error"),
            }
        });
}

/// The "Save as preset" viewport save: captures the full view, the lighting fields of
/// [`setup_save_lighting_preset_callback`] plus the camera yaw, pitch, roll and pan as
/// `Some(..)`, so applying the preset restores the exact shot, plus the Stone Pose when the
/// stone is posed. A camera pose is part of a preset only here; see
/// `settings::model::LightingPreset`.
fn setup_save_view_preset_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx_save = render_ctx.clone();
    let settings_store_save = settings_store.clone();
    let ui_weak_save = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_save_view_preset(move |name: SharedString| {
            let fields = read_common_view_fields(&render_ctx_save, &settings_store_save);
            let (camera_yaw, camera_pitch, camera_roll, camera_pan, stone_pose) = {
                let ctx = RenderContext::lock(&render_ctx_save);
                (
                    ctx.yaw,
                    ctx.pitch,
                    ctx.camera_roll,
                    ctx.camera_pan,
                    ctx.stone_pose.to_array(),
                )
            };
            let preset = SavedLightingPreset {
                name: name.to_string(),
                built_in: false,
                light_yaw_deg: fields.light_yaw_deg,
                light_pitch_deg: fields.light_pitch_deg,
                exposure: fields.exposure,
                lighting_rig: fields.lighting_rig,
                camera_distance: fields.camera_distance,
                camera_yaw: Some(camera_yaw),
                camera_pitch: Some(camera_pitch),
                camera_roll: Some(camera_roll),
                camera_pan: Some(camera_pan),
                stone_pose: LightingValues::recorded_stone_pose(stone_pose),
                env_map_path: fields.env_map_path,
                export_usable: false,
                surface_glare: fields.surface_glare,
                backdrop: fields.backdrop,
                head_shadow_deg: fields.head_shadow_deg,
                stone_shadow: fields.stone_shadow,
                tray_reflection: fields.tray_reflection,
                env_map_yaw_deg: fields.env_map_yaw_deg,
                env_map_follows_camera: fields.env_map_follows_camera,
                product_shot: false,
            };

            let mut result = Ok(());
            settings_store_save.update(|s| result = s.upsert_user_preset(preset.clone()));

            let Some(ui) = ui_weak_save.upgrade() else {
                return;
            };
            match result {
                Ok(()) => {
                    refresh_lighting_preset_options(&ui, &settings_store_save.snapshot().presets);
                    show_toast(
                        &ui,
                        &format!("Saved lighting '{name}' with the camera view"),
                        "success",
                    );
                }
                Err(err) => show_toast(&ui, &err, "error"),
            }
        });
}

/// What applying a saved lighting does to the render context's HDR environment map.
///
/// A saved lighting is a complete look, so one WITHOUT a map clears any loaded map (the lighting
/// rig it names comes back), the same as choosing a rig; old presets without a map path meant
/// "no map" too. One WITH a map loads it. A map that could not be reloaded (`Err`: the file
/// moved or was deleted) leaves the current environment untouched.
fn apply_environment(
    ctx: &mut RenderContext,
    loaded: Option<&Result<Arc<EnvironmentMap>, String>>,
) {
    match loaded {
        Some(Ok(map)) => ctx.env_map = Some(Arc::clone(map)),
        None => ctx.env_map = None,
        Some(Err(_)) => {}
    }
}

/// The turn belongs to the map. A saved lighting whose map could not be reloaded keeps the
/// current map, so it also keeps that map's current turn and camera switch: they replace the
/// ones in `values`, which are then applied, persisted and shown as usual. The queued export
/// follows the same rule (`render_export::queue`).
fn keep_current_turn(values: &mut LightingValues, ctx: &RenderContext) {
    values.env_map_yaw_deg = ctx.hdr_yaw.to_degrees().rem_euclid(360.0);
    values.env_map_follows_camera = ctx.hdr_follows_camera;
}

/// Applies preset `idx` from the settings store's current preset list (the list the UI's
/// `lighting_presets` model is built from): pushes its fields into `RenderContext`, setting
/// `dirty` so the render restarts, mirrors them into the UI controls, and persists them as the
/// current settings so the look survives a restart.
///
/// The camera pose is restored only when the preset carries one (`camera_yaw`/`camera_pitch`
/// both `Some`). The HDR environment map is part of the look: a preset without one clears any
/// loaded map ([`apply_environment`]). A preset with `product_shot` (the built-in "Presentation")
/// then lays the stone out like the Product Shot button, for the design on screen: the stone
/// faces the camera on the tray, the camera drops to the product-shot elevation and the pan
/// places the stone ([`super::stone_pose::product_shot`]).
fn setup_apply_lighting_preset_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    mesh_bounding_radius: &Arc<Mutex<f64>>,
    preview_state: &Arc<SolidPreviewState>,
) {
    let render_ctx_apply = render_ctx.clone();
    let settings_store_apply = settings_store.clone();
    let mesh_bounding_radius_apply = mesh_bounding_radius.clone();
    let preview_state_apply = Arc::clone(preview_state);
    let ui_weak_apply = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_apply_lighting_preset(move |idx: i32| {
            let Some(ui) = ui_weak_apply.upgrade() else {
                return;
            };
            let Some(preset) = usize::try_from(idx)
                .ok()
                .and_then(|i| settings_store_apply.snapshot().presets.get(i).cloned())
            else {
                return;
            };
            // The cutter chose a lighting: a job's view ends first and puts the cutter's normal
            // look back, so the preset is written over that and not over the job's.
            crate::gui::render_jobs::view::session::end(
                &ui,
                crate::gui::render_jobs::view::session::EndReason::LightingChosen,
            );

            let lighting_preset =
                crate::gui::optics::offered_lighting::offered_from_label(&preset.lighting_rig);
            // The scene fields, each limited the way the live controls limit it, so a
            // hand-edited preset cannot feed the tracer nonsense.
            let mut values = preset.values().sanitised();
            // A built-in view (the Grading tray) is framed to the stone like Fit: its stored
            // distance suits one stone only. A view the cutter saved keeps its own distance.
            // Computed before the lock below, which the render thread shares.
            let fitted = (preset.built_in && values.camera_distance.is_some()).then(|| {
                super::fit_view::fit_distance(&ui, &render_ctx_apply, &mesh_bounding_radius_apply)
            });
            if fitted.is_some() {
                values.camera_distance = fitted;
            }

            // A product-shot lighting lays the stone out like the Product Shot button: the pose
            // and the camera come from this design. Measured before the lock below, which the
            // render thread shares; `None` for a stone that closes no solid, which then keeps
            // its pose and camera.
            let shot = if preset.product_shot {
                super::stone_pose::product_shot_for_view(&ui, &render_ctx_apply)
            } else {
                None
            };

            // Loading the HDR file is I/O, so it runs BEFORE the lock below. A map that cannot
            // be loaded (moved or deleted) leaves the current environment untouched, as in
            // `gui::camera_lighting::setup_environment_map_callbacks`.
            let env_map_result = preset.env_map_path.as_deref().map(load_env_map);

            let applied_pose = {
                let mut ctx = RenderContext::lock(&render_ctx_apply);
                if matches!(env_map_result, Some(Err(_))) {
                    keep_current_turn(&mut values, &ctx);
                }
                apply_values_to_context(&mut ctx, &values, lighting_preset);
                // The zoom clamp of the live orbit camera, sized to the current mesh's
                // bounding radius (the live `Arc<Mutex<f64>>` shared with
                // `gui::camera_lighting`), so the saved distance frames the loaded design.
                let radius = *mesh_bounding_radius_apply
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let (min_distance, max_distance) =
                    super::camera_lighting::orbit_distance_bounds(radius);
                if let Some(distance) = fitted {
                    // Fit's own distance is not clamped to the far end (see
                    // `fit_view::fit_distance`).
                    ctx.distance = distance;
                } else if let Some(distance) = values.camera_distance {
                    ctx.distance = distance.clamp(min_distance, max_distance);
                }
                // Camera pose, only when the preset carries one; the pitch is clamped so a
                // stored pose stays in range.
                apply_camera_pose(&mut ctx, &values);
                if let Some(shot) = &shot {
                    super::stone_pose::write_product_shot(&mut ctx, shot);
                }
                apply_environment(&mut ctx, env_map_result.as_ref());
                ctx.dirty = true;
                // A preset that moved the camera or the stone (a view, the Presentation shot,
                // the Grading tray) leaves the Solid raster at the old pose until it is redrawn.
                if fitted.is_some()
                    || shot.is_some()
                    || values.camera_distance.is_some()
                    || values.camera_pose().is_some()
                    || values.pose_pan().is_some()
                    || values.pose_stone_pose().is_some()
                {
                    super::camera_lighting::resubmit_at_current_pose(
                        &ui,
                        &ctx,
                        &preview_state_apply,
                    );
                }
                ctx.stone_pose
            };

            settings_store_apply.update(|s| {
                persist_preset_values(s, &values, &preset, env_map_result.as_ref());
                if let Some(shot) = &shot {
                    super::stone_pose::store_product_shot(&mut s.settings, shot);
                }
            });
            // The settings now hold this preset's look, so it is the cutter's normal
            // lighting: a design's own saved lighting that was showing is let go (it
            // stays saved) instead of being restored over this choice later.
            super::design_lighting::normal_lighting_chosen();
            // The app placed this camera, so the next design load frames its stone again.
            if let Some(distance) = fitted {
                super::fit_view::note_auto_framed(distance);
            }

            push_lighting_values_to_controls(&ui, &values, lighting_preset);
            // A view with a camera pose can change the Stone Pose; the STONE POSE group and the
            // Live view chip show the pose now in effect.
            super::stone_pose::push_stone_pose_to_controls(&ui, applied_pose, false);
            show_environment_result(&ui, &preset, env_map_result.as_ref());
            show_toast(&ui, &format!("Applied lighting '{}'", preset.name), "info");
        });
}

/// Sets the camera pose when `values` carries one (a pose without a roll gets zero roll, one
/// without a pan centres the stone, one without a Stone Pose stands the stone as designed);
/// otherwise the current pose, pan and Stone Pose included, is kept.
fn apply_camera_pose(ctx: &mut RenderContext, values: &LightingValues) {
    if let Some((yaw, pitch, roll)) = values.camera_pose() {
        ctx.yaw = yaw;
        ctx.pitch = pitch.clamp(-1.48, 1.48);
        ctx.camera_roll = roll;
    }
    if let Some(pan) = values.pose_pan() {
        ctx.camera_pan = pan;
    }
    if let Some(pose) = values.pose_stone_pose() {
        ctx.stone_pose = StonePose::from_array(pose);
    }
}

/// Puts the scene fields of `values` into the render context: the light, exposure, rig, surface
/// glare, backdrop, head shadow, Stone Shadow, Tray Reflection and the HDR map's rotation and
/// camera switch. Fields an older preset lacks take their serde defaults, so applying any preset
/// sets the whole look. The caller sets the camera, the environment map and `dirty`.
pub(in crate::gui) const fn apply_values_to_context(
    ctx: &mut RenderContext,
    values: &LightingValues,
    rig: LightingPreset,
) {
    ctx.light_yaw = values.light_yaw_deg.to_radians();
    ctx.light_pitch = values
        .light_pitch_deg
        .to_radians()
        .clamp(PITCH_RANGE_RAD.0, PITCH_RANGE_RAD.1);
    ctx.exposure = values.exposure;
    ctx.lighting_preset = rig;
    ctx.surface_glare = values.surface_glare;
    ctx.backdrop = values.backdrop;
    ctx.head_shadow_deg = values.head_shadow_deg;
    ctx.stone_shadow = values.stone_shadow;
    ctx.tray_reflection = values.tray_reflection;
    ctx.hdr_yaw = values.env_map_yaw_deg.to_radians();
    ctx.hdr_follows_camera = values.env_map_follows_camera;
}

/// The controls half of showing `values`: the lighting sliders, pills and toggles and the rig
/// drop-down. Setting a property from Rust does not fire the control's own "changed"
/// callback, so nothing is written to the settings file by this. Shared by applying a saved
/// lighting and by the Lighting tab's reset, so the two cannot drift apart.
pub(in crate::gui) fn push_lighting_values_to_controls(
    ui: &MainWindow,
    values: &LightingValues,
    rig: LightingPreset,
) {
    let settings = ui.global::<SettingsModel>();
    settings.set_light_yaw_deg(values.light_yaw_deg);
    settings.set_light_pitch_deg(values.light_pitch_deg);
    settings.set_exposure_val(values.exposure);
    settings.set_surface_glare_pct(percent_from_surface_glare(values.surface_glare));
    settings.set_backdrop_index(values.backdrop.index());
    settings.set_head_shadow_deg(values.head_shadow_deg);
    settings.set_stone_shadow(values.stone_shadow);
    settings.set_tray_reflection_pct(crate::settings::model::percent_from_tray_reflection(
        values.tray_reflection,
    ));
    settings.set_env_map_yaw_deg(values.env_map_yaw_deg);
    settings.set_env_map_follows_camera(values.env_map_follows_camera);
    ui.global::<ViewportModel>()
        .set_selected_lighting_index(rig.index());
}

/// Writes an applied saved lighting into the settings file, so the applied look survives a
/// restart. The camera pose, Stone Pose and distance only when the preset carries a camera
/// view (a view without a Stone Pose stores the stone as designed); the environment
/// path follows `env_map_result` exactly as the render context does ([`apply_environment`]).
fn persist_preset_values(
    s: &mut SettingsFile,
    values: &LightingValues,
    preset: &SavedLightingPreset,
    env_map_result: Option<&Result<Arc<EnvironmentMap>, String>>,
) {
    s.settings.light_yaw_deg = values.light_yaw_deg;
    s.settings.light_pitch_deg = values.light_pitch_deg;
    s.settings.exposure = values.exposure;
    s.settings.lighting_rig.clone_from(&values.lighting_rig);
    if let Some(distance) = values.camera_distance {
        s.settings.camera_distance = distance;
    }
    s.settings.surface_glare = values.surface_glare;
    s.settings.backdrop = values.backdrop;
    s.settings.head_shadow_deg = values.head_shadow_deg;
    s.settings.stone_shadow = values.stone_shadow;
    s.settings.tray_reflection = values.tray_reflection;
    s.settings.env_map_yaw_deg = values.env_map_yaw_deg;
    s.settings.env_map_follows_camera = values.env_map_follows_camera;
    if let Some((yaw, pitch, roll)) = values.camera_pose() {
        s.settings.camera_yaw = yaw;
        s.settings.camera_pitch = pitch.clamp(-1.48, 1.48);
        s.settings.camera_roll = roll;
    }
    if let Some(pan) = values.pose_pan() {
        s.settings.camera_pan = pan;
    }
    if let Some(pose) = values.pose_stone_pose() {
        s.settings.stone_pose = StonePose::from_array(pose).to_array();
    }
    match (env_map_result, preset.env_map_path.as_ref()) {
        (Some(Ok(_)), Some(path)) => s.settings.env_map_path.clone_from(path),
        // A saved lighting without a map means "no map": the rig comes back.
        (None, _) => s.settings.env_map_path.clear(),
        // A map that could not be reloaded leaves the current environment alone.
        (Some(_), _) => {}
    }
}

/// Mirrors the environment-map outcome of applying `preset` into the HDR panel: the status text
/// and toggle (the same `env_map_status_text` helper `gui::camera_lighting::on_load_env_map`
/// uses, so a saved-lighting load reads exactly like a manual one), or an error toast when the
/// file could not be reloaded.
fn show_environment_result(
    ui: &MainWindow,
    preset: &SavedLightingPreset,
    env_map_result: Option<&Result<Arc<EnvironmentMap>, String>>,
) {
    let settings = ui.global::<SettingsModel>();
    match (env_map_result, preset.env_map_path.as_deref()) {
        (Some(Ok(map)), Some(path)) => {
            settings.set_env_map_status(env_map_status_text(map, path).into());
            settings.set_env_map_loaded(true);
        }
        (Some(Err(err)), _) => show_toast(
            ui,
            &format!("The lighting's HDR environment could not be reloaded: {err}"),
            "error",
        ),
        (None, _) => {
            // The preset carries no map, so a map loaded earlier is let go: a saved
            // lighting is a complete look, the same as choosing a rig.
            settings.set_env_map_status("".into());
            settings.set_env_map_loaded(false);
            // Nothing still on its way (a decode of a design's saved map) may land
            // over this choice.
            super::design_lighting::env_map_chosen();
        }
        // A loaded map always has the path that loaded it.
        (Some(Ok(_)), None) => {}
    }
}

fn setup_rename_lighting_preset_callback(ui: &MainWindow, settings_store: &Arc<SettingsPersister>) {
    let settings_store_rename = settings_store.clone();
    let ui_weak_rename = ui.as_weak();
    ui.global::<ViewportModel>().on_rename_lighting_preset(
        move |idx: i32, new_name: SharedString| {
            let Some(old_name) = usize::try_from(idx).ok().and_then(|i| {
                settings_store_rename
                    .snapshot()
                    .presets
                    .get(i)
                    .map(|p| p.name.clone())
            }) else {
                return;
            };

            let mut result = Ok(());
            settings_store_rename.update(|s| result = s.rename_preset(&old_name, &new_name));

            let Some(ui) = ui_weak_rename.upgrade() else {
                return;
            };
            match result {
                Ok(()) => {
                    refresh_lighting_preset_options(&ui, &settings_store_rename.snapshot().presets);
                    show_toast(&ui, "Lighting renamed.", "success");
                }
                Err(err) => show_toast(&ui, &err, "error"),
            }
        },
    );
}

fn setup_delete_lighting_preset_callback(ui: &MainWindow, settings_store: &Arc<SettingsPersister>) {
    let settings_store_delete = settings_store.clone();
    let ui_weak_delete = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_delete_lighting_preset(move |idx: i32| {
            let Some(name) = usize::try_from(idx).ok().and_then(|i| {
                settings_store_delete
                    .snapshot()
                    .presets
                    .get(i)
                    .map(|p| p.name.clone())
            }) else {
                return;
            };

            let mut result = Ok(());
            settings_store_delete.update(|s| result = s.delete_preset(&name));

            let Some(ui) = ui_weak_delete.upgrade() else {
                return;
            };
            match result {
                Ok(()) => {
                    refresh_lighting_preset_options(&ui, &settings_store_delete.snapshot().presets);
                    show_toast(&ui, &format!("Deleted lighting '{name}'"), "info");
                }
                Err(err) => show_toast(&ui, &err, "error"),
            }
        });
}

/// The Parameters panel's "usable for export" checkbox next to each preset row -- see
/// `settings::model::LightingPreset::export_usable`'s own doc comment for what this
/// controls, and `SettingsFile::set_preset_export_usable` for why this is deliberately
/// NOT refused for a built-in preset the way rename/delete are.
fn setup_toggle_preset_export_usable_callback(
    ui: &MainWindow,
    settings_store: &Arc<SettingsPersister>,
) {
    let settings_store_toggle = settings_store.clone();
    let ui_weak_toggle = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_toggle_preset_export_usable(move |idx: i32, usable: bool| {
            let Some(name) = usize::try_from(idx).ok().and_then(|i| {
                settings_store_toggle
                    .snapshot()
                    .presets
                    .get(i)
                    .map(|p| p.name.clone())
            }) else {
                return;
            };

            let mut result = Ok(());
            settings_store_toggle.update(|s| result = s.set_preset_export_usable(&name, usable));

            let Some(ui) = ui_weak_toggle.upgrade() else {
                return;
            };
            match result {
                Ok(()) => {
                    refresh_lighting_preset_options(&ui, &settings_store_toggle.snapshot().presets);
                }
                Err(err) => show_toast(&ui, &err, "error"),
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{apply_camera_pose, apply_environment, unique_name};
    use crate::{bridge::render_thread::RenderContext, settings::model::LightingValues};
    use pleochroma::renderer::env_map::EnvironmentMap;
    use std::sync::Arc;

    fn loaded_map() -> Arc<EnvironmentMap> {
        Arc::new(EnvironmentMap::uniform(2, 2, [1.0, 1.0, 1.0]))
    }

    #[test]
    fn saved_camera_roll_applies_and_older_saved_views_reset_it() {
        let mut ctx = RenderContext {
            camera_roll: 0.75,
            ..Default::default()
        };
        let mut values = LightingValues {
            camera_yaw: Some(0.6),
            camera_pitch: Some(0.45),
            camera_roll: Some(std::f32::consts::FRAC_PI_2),
            ..Default::default()
        };
        apply_camera_pose(&mut ctx, &values);
        assert_eq!(ctx.camera_roll, std::f32::consts::FRAC_PI_2);
        values.camera_roll = None;
        apply_camera_pose(&mut ctx, &values);
        assert_eq!(ctx.camera_roll, 0.0);
        values.camera_yaw = None;
        ctx.camera_roll = 0.75;
        apply_camera_pose(&mut ctx, &values);
        assert_eq!(ctx.camera_roll, 0.75, "lighting-only preserves the camera");
    }

    #[test]
    fn saved_camera_pan_applies_and_older_saved_views_centre_the_stone() {
        let mut ctx = RenderContext {
            camera_pan: [0.5, 0.5],
            ..Default::default()
        };
        let mut values = LightingValues {
            camera_yaw: Some(0.6),
            camera_pitch: Some(0.45),
            camera_pan: Some([-0.3, 0.2]),
            ..Default::default()
        };
        apply_camera_pose(&mut ctx, &values);
        assert_eq!(ctx.camera_pan, [-0.3, 0.2]);
        values.camera_pan = None;
        apply_camera_pose(&mut ctx, &values);
        assert_eq!(ctx.camera_pan, [0.0, 0.0], "an older view is centred");
        values.camera_yaw = None;
        ctx.camera_pan = [0.5, 0.5];
        apply_camera_pose(&mut ctx, &values);
        assert_eq!(
            ctx.camera_pan,
            [0.5, 0.5],
            "lighting-only preserves the camera"
        );
    }

    /// The applied view's pan is what the settings file reopens with; a lighting-only apply
    /// leaves the file's pan alone.
    #[test]
    fn applying_a_saved_view_persists_its_pan() {
        let mut file = crate::settings::SettingsFile::default();
        file.settings.camera_pan = [0.5, 0.5];
        let preset = crate::settings::model::built_in_presets().remove(0);
        super::persist_preset_values(&mut file, &preset.values(), &preset, None);
        assert_eq!(file.settings.camera_pan, [0.5, 0.5]);
        let view = LightingValues {
            camera_yaw: Some(0.6),
            camera_pitch: Some(0.45),
            camera_pan: Some([-0.3, 0.2]),
            ..Default::default()
        };
        super::persist_preset_values(&mut file, &view, &preset, None);
        assert_eq!(file.settings.camera_pan, [-0.3, 0.2]);
    }

    /// A saved view brings its Stone Pose, a view recorded without one stands the stone as
    /// designed, and a lighting-only apply keeps the current pose.
    #[test]
    fn saved_stone_pose_applies_and_a_view_without_one_stands_the_stone_as_designed() {
        use pleochroma::render_setup::StonePose;
        let posed = StonePose::from_array([90.0, 30.0, 10.0]);
        let mut ctx = RenderContext {
            stone_pose: posed,
            ..Default::default()
        };
        let mut values = LightingValues {
            camera_yaw: Some(0.6),
            camera_pitch: Some(0.45),
            stone_pose: Some([45.0, 10.0, 0.0]),
            ..Default::default()
        };
        apply_camera_pose(&mut ctx, &values);
        assert_eq!(ctx.stone_pose, StonePose::from_array([45.0, 10.0, 0.0]));
        values.stone_pose = None;
        apply_camera_pose(&mut ctx, &values);
        assert_eq!(ctx.stone_pose, StonePose::IDENTITY, "an older view");
        values.camera_yaw = None;
        ctx.stone_pose = posed;
        apply_camera_pose(&mut ctx, &values);
        assert_eq!(ctx.stone_pose, posed, "lighting-only keeps the pose");
    }

    /// The Grading tray carries a camera pose and no Stone Pose, so applying it stands a posed
    /// stone as designed; a built-in mood leaves the pose alone.
    #[test]
    fn applying_the_grading_tray_clears_the_stone_pose_and_a_mood_keeps_it() {
        use pleochroma::render_setup::StonePose;
        let posed = StonePose::from_array([90.0, 30.0, 10.0]);
        for preset in crate::settings::model::built_in_presets() {
            let mut ctx = RenderContext {
                stone_pose: posed,
                ..Default::default()
            };
            apply_camera_pose(&mut ctx, &preset.values().sanitised());
            if preset.name == "Grading tray" {
                assert_eq!(ctx.stone_pose, StonePose::IDENTITY);
            } else {
                assert_eq!(ctx.stone_pose, posed, "{}", preset.name);
            }
        }
    }

    /// What the settings file reopens with: the applied view's Stone Pose, the stone as designed
    /// for a view recorded without one, and a lighting-only apply leaves the file's pose alone.
    #[test]
    fn applying_a_saved_view_persists_its_stone_pose() {
        let mut file = crate::settings::SettingsFile::default();
        file.settings.stone_pose = [90.0, 30.0, 10.0];
        let preset = crate::settings::model::built_in_presets().remove(0);
        super::persist_preset_values(&mut file, &preset.values(), &preset, None);
        assert_eq!(file.settings.stone_pose, [90.0, 30.0, 10.0]);
        let mut view = LightingValues {
            camera_yaw: Some(0.6),
            camera_pitch: Some(0.45),
            stone_pose: Some([45.0, 10.0, 0.0]),
            ..Default::default()
        };
        super::persist_preset_values(&mut file, &view, &preset, None);
        assert_eq!(file.settings.stone_pose, [45.0, 10.0, 0.0]);
        view.stone_pose = None;
        super::persist_preset_values(&mut file, &view, &preset, None);
        assert_eq!(file.settings.stone_pose, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_saved_lighting_without_a_map_clears_a_loaded_map() {
        let mut ctx = RenderContext {
            env_map: Some(loaded_map()),
            ..Default::default()
        };
        apply_environment(&mut ctx, None);
        assert!(ctx.env_map.is_none(), "the rig comes back");
    }

    #[test]
    fn a_saved_lighting_with_a_map_loads_it() {
        let mut ctx = RenderContext::default();
        let map = loaded_map();
        apply_environment(&mut ctx, Some(&Ok(Arc::clone(&map))));
        assert!(ctx.env_map.as_ref().is_some_and(|m| Arc::ptr_eq(m, &map)));
    }

    #[test]
    fn a_map_that_cannot_be_reloaded_leaves_the_environment_alone() {
        let current = loaded_map();
        let mut ctx = RenderContext {
            env_map: Some(Arc::clone(&current)),
            ..Default::default()
        };
        apply_environment(&mut ctx, Some(&Err("missing".to_string())));
        assert!(
            ctx.env_map
                .as_ref()
                .is_some_and(|m| Arc::ptr_eq(m, &current))
        );
        // And with no map loaded, nothing appears out of nowhere.
        let mut empty = RenderContext::default();
        apply_environment(&mut empty, Some(&Err("missing".to_string())));
        assert!(empty.env_map.is_none());
    }

    /// A saved lighting brings the map's rotation and its camera switch back, and a lighting
    /// without them (an older one) puts both at their defaults.
    #[test]
    fn a_saved_lighting_carries_the_map_rotation_into_the_context() {
        let rig = pleochroma::optics::raytracer::LightingPreset::RingLights;
        let mut ctx = RenderContext::default();
        let turned = LightingValues {
            env_map_yaw_deg: 90.0,
            env_map_follows_camera: true,
            ..Default::default()
        };
        super::apply_values_to_context(&mut ctx, &turned, rig);
        assert_eq!(ctx.hdr_yaw.to_bits(), 90.0_f32.to_radians().to_bits());
        assert!(ctx.hdr_follows_camera);
        super::apply_values_to_context(&mut ctx, &LightingValues::default(), rig);
        assert_eq!(ctx.hdr_yaw.to_bits(), 0.0_f32.to_bits());
        assert!(!ctx.hdr_follows_camera);
    }

    /// The turn belongs to the map: a lighting whose map cannot be reloaded keeps the current
    /// map's turn and camera switch instead of bringing its own.
    #[test]
    fn a_map_that_cannot_be_reloaded_keeps_the_current_turn() {
        let rig = pleochroma::optics::raytracer::LightingPreset::RingLights;
        let mut ctx = RenderContext {
            hdr_yaw: 30.0_f32.to_radians(),
            hdr_follows_camera: false,
            ..Default::default()
        };
        let mut values = LightingValues {
            env_map_yaw_deg: 90.0,
            env_map_follows_camera: true,
            ..Default::default()
        };
        super::keep_current_turn(&mut values, &ctx);
        super::apply_values_to_context(&mut ctx, &values, rig);
        assert!((ctx.hdr_yaw.to_degrees() - 30.0).abs() < 1e-4);
        assert!(!ctx.hdr_follows_camera);
        assert!((values.env_map_yaw_deg - 30.0).abs() < 1e-4);
        assert!(!values.env_map_follows_camera);
    }

    #[test]
    fn a_free_name_is_offered_unchanged() {
        assert_eq!(
            unique_name("Tent - 2026-10-12".into(), &[]),
            "Tent - 2026-10-12"
        );
    }

    #[test]
    fn a_taken_name_gets_the_next_free_number() {
        let taken = vec!["A".to_string(), "A (2)".to_string()];
        assert_eq!(unique_name("A".into(), &taken), "A (3)");
    }
}
