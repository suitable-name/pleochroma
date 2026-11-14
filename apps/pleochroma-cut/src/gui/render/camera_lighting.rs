//! Camera orbit/pan/zoom, light move/position/reset, and HDR environment-map load/clear
//! callback wiring.

use crate::{
    MainWindow, SettingsModel, SolidPreviewModel, ViewportModel,
    bridge::render_thread::{RenderContext, load_env_map},
    gui::{
        env_map_status_text, show_toast,
        solid_preview::preview_state::{CameraPose, SolidPreviewState},
        view_modes::{EditViewMode, LiveViewMode},
    },
    settings::{SettingsPersister, model::LightingValues},
};
use pleochroma_solid::preview::{
    StoneGeometryBuf,
    camera::{orbit_step, pan_pose, unroll_drag_delta},
};
use slint::{ComponentHandle, SharedString};
use std::{
    f32::consts::{FRAC_PI_2, TAU},
    sync::{Arc, Mutex},
};

/// Converts a Slint `view-mode` model value (a small enum index stored as `i32`) to the
/// `u8` the preview state takes; an out-of-range value falls back to 0 (the live view).
pub fn view_mode_index(raw: i32) -> u8 {
    u8::try_from(EditViewMode::from_raw(raw).raw()).unwrap_or_default()
}

/// Converts a logical-times-scale viewport extent to whole device pixels. `as` saturates
/// (negative and NaN become 0), which is the wanted behaviour for a not-yet-laid-out
/// viewport that reports a zero or negative size.
pub const fn device_pixels(extent: f32) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "saturating float to pixel count: negative or NaN extents map to 0 by design"
    )]
    let pixels = extent as u32;
    pixels
}

/// Re-issues the planes of `ctx` at its just-updated camera pose to `preview_state`.
///
/// Sized to the Solid viewport's logical size. Camera-follow only: same planes, new pose,
/// never a re-solve. Does nothing while no planes are solved yet (`active_planes` empty), as
/// `gui::editor::view::viewport::refresh_viewport` does.
///
/// Picks the viewport and its view mode:
/// - Live Render tab (`ui.get_render_view_tab() == 0`): always Solid (`view_mode` 0). See
///   [`resubmit_live_solid`] for why the Edit tab's mode must not leak in. Only when the
///   tab's `ViewportModel.live_view_mode` is Solid (`0`): a traced image has no solid on
///   screen to redraw.
/// - Edit tab (`render_view_tab != 0`): the Edit tab's `SolidPreviewModel.view_mode`,
///   unconditionally.
///
/// Also forwards `ctx.design_gear` (the loaded design's gear tooth count and reference
/// angle), so a Diagram-mode (`view_mode` 3) drag shows this design's gear wheel, via
/// [`SolidPreviewState::request_redraw_with_gear`]. See `RenderContext::design_gear` for
/// the writers that keep it in sync with `active_planes`.
///
/// Sizes to the Live tab's `SolidPreviewModel.live_viewport_width`/`live_viewport_height`
/// while on that tab, never the Edit tab's size: `gem_viewport.slint` and
/// `solid_viewport.slint` each drive their own pair of properties, so one viewport's resize
/// must not overwrite the other's pending redraw size.
///
/// Both dimensions are scaled by `ui.window().scale_factor()`, so the rasterizer produces a
/// physical-pixel image instead of one that Slint would upscale blurrily. Pointer picking
/// applies the same factor to the logical position (see `solid_preview::diagram_wiring` and
/// `gui::editor::callbacks::tier_actions`).
pub fn resubmit_at_current_pose(
    ui: &MainWindow,
    ctx: &RenderContext,
    preview_state: &SolidPreviewState,
) {
    // The marker follows the pose immediately, including traced views whose next
    // beauty frame is still rendering. Defer past the caller's context lock.
    let _ = ui.as_weak().upgrade_in_event_loop(|ui| {
        ui.global::<crate::CrystalAxisModel>()
            .invoke_live_overlay_changed();
    });
    if ctx.active_planes.is_empty() {
        return;
    }
    let on_live_tab = ui.get_render_view_tab() == 0;
    if on_live_tab
        && LiveViewMode::from_raw(ui.global::<ViewportModel>().get_live_view_mode()).is_traced()
    {
        return;
    }
    let view_mode = if on_live_tab {
        0
    } else {
        view_mode_index(ui.global::<SolidPreviewModel>().get_view_mode())
    };
    let stone = active_stone(ctx);
    let scale = ui.window().scale_factor();
    let size = if on_live_tab {
        (
            device_pixels(ui.global::<SolidPreviewModel>().get_live_viewport_width() * scale),
            device_pixels(ui.global::<SolidPreviewModel>().get_live_viewport_height() * scale),
        )
    } else {
        let viewport_size = (
            device_pixels(ui.global::<SolidPreviewModel>().get_viewport_width() * scale),
            device_pixels(ui.global::<SolidPreviewModel>().get_viewport_height() * scale),
        );
        // Traced modes letterbox the image to the render size's aspect ratio; see
        // `contained_request_size`.
        contained_request_size(view_mode, viewport_size, (ctx.width, ctx.height))
    };
    let pose = CameraPose {
        roll: ctx.camera_roll,
        yaw: ctx.yaw,
        pitch: ctx.pitch,
        distance: ctx.distance,
        pan: ctx.camera_pan,
    };
    if on_live_tab {
        // The Live Render tab shows the committed design only: a Slice tier that is not
        // kept yet is an Edit-tab preview (see `request_redraw_committed_geometry`).
        preview_state.request_redraw_committed_geometry(
            stone,
            pose,
            size,
            view_mode,
            ctx.design_gear,
        );
    } else {
        preview_state.request_redraw_geometry(stone, pose, size, view_mode, ctx.design_gear);
    }
}

/// The stone `ctx` currently holds -- planes, concave tools and each tool's
/// `(tier, placement)` -- as the owning geometry a camera-follow redraw carries.
///
/// The planes go through the same `(normal, -d)` flip the preview has always used, so a
/// planar stone is the request it always was; the tools ride along unchanged, which is
/// what stops a camera drag from redrawing a concave design as its flat hull.
pub(super) fn active_stone(ctx: &RenderContext) -> StoneGeometryBuf {
    StoneGeometryBuf {
        planes: ctx.active_planes.as_ref().clone(),
        tools: ctx.active_tools.as_ref().clone(),
        placements: ctx.active_placements.clone(),
        frosted: ctx.active_frosted.as_ref().clone(),
    }
}

/// Shrinks `viewport_size` to the rectangle that Slint's `image-fit: contain` draws
/// `render_size` into.
///
/// The traced `Image` elements of `solid_viewport.slint` fit the render resolution
/// (`RenderContext::width`/`height`) into the viewport with `contain`. When the aspect ratios
/// differ, the traced image is letterboxed, so a raster requested at the raw viewport size
/// would overhang it. The edges overlay and the pick buffer must match the traced image's
/// on-screen rectangle, so they are requested at this function's output.
///
/// Only the traced modes (Path-Traced `1`, Both `2`, Ray-Traced `4`) are letterboxed. Solid
/// `0` and Diagram `3` never show the traced image, so the raw size is returned unchanged,
/// as is any zero dimension.
///
/// Other callers that request a solid raster for a traced mode (`submit_preview_replan` and
/// `refresh_viewport` in `gui::editor::view::viewport`) must apply the same sizing, or the
/// overlay misaligns after an edit.
#[must_use]
pub(in crate::gui) fn contained_request_size(
    view_mode: u8,
    viewport_size: (u32, u32),
    render_size: (u32, u32),
) -> (u32, u32) {
    if !EditViewMode::from_raw(i32::from(view_mode)).is_traced() {
        return viewport_size;
    }
    let (viewport_width, viewport_height) = viewport_size;
    let (render_width, render_height) = render_size;
    if viewport_width == 0 || viewport_height == 0 || render_width == 0 || render_height == 0 {
        return viewport_size;
    }
    let viewport_aspect = viewport_width as f32 / viewport_height as f32;
    let render_aspect = render_width as f32 / render_height as f32;
    if render_aspect > viewport_aspect {
        // The traced image is wider than the viewport -- `contain` fits its
        // width and shrinks its height.
        let height = device_pixels((viewport_width as f32 / render_aspect).round().max(1.0));
        (viewport_width, height)
    } else {
        // The traced image is taller/narrower than the viewport -- `contain`
        // fits its height and shrinks its width. Already the on-screen size
        // whenever `render_aspect <= viewport_aspect`, so this branch is a near
        // no-op in that case beyond rounding.
        let width = device_pixels((viewport_height as f32 * render_aspect).round().max(1.0));
        (width, viewport_height)
    }
}

/// The logical size of the picture a pan drag moves the stone across, so the stone follows the
/// pointer and can reach the picture's edges (`pan_pose` measures in its half-heights).
///
/// A `traced` picture is the rectangle `image-fit: contain` draws the `render` resolution into
/// (see [`contained_request_size`]); the solid raster fills the whole `viewport`, as does a
/// traced picture before the render size is known.
#[must_use]
const fn pan_picture_size(traced: bool, viewport: (f32, f32), render: (u32, u32)) -> (f32, f32) {
    if !traced || render.0 == 0 || render.1 == 0 {
        return viewport;
    }
    let (render_width, render_height) = (render.0 as f32, render.1 as f32);
    let scale = (viewport.0 / render_width).min(viewport.1 / render_height);
    (render_width * scale, render_height * scale)
}

/// The logical size of the 3D viewport on screen and whether it shows a traced picture: the
/// Live Render viewport on the Live Render tab, the Edit tab's Solid viewport otherwise (the
/// two are never mounted together).
fn shown_viewport(ui: &MainWindow) -> ((f32, f32), bool) {
    let preview = ui.global::<SolidPreviewModel>();
    if ui.get_render_view_tab() == 0 {
        (
            (
                preview.get_live_viewport_width(),
                preview.get_live_viewport_height(),
            ),
            LiveViewMode::from_raw(ui.global::<ViewportModel>().get_live_view_mode()).is_traced(),
        )
    } else {
        (
            (preview.get_viewport_width(), preview.get_viewport_height()),
            EditViewMode::from_raw(preview.get_view_mode()).is_traced(),
        )
    }
}

// The camera math (orbit bounds, Fit distance, pitch wrap) is shared with the web app's
// Solid view and lives in `pleochroma_solid::preview::camera`.
pub use pleochroma_solid::preview::camera::{
    fit_distance_for_radius, orbit_distance_bounds, wrap_pitch,
};

/// The Live Render tab's own solid-raster redraw.
///
/// It is [`resubmit_at_current_pose`]'s Live-tab branch without the `ViewportModel.live_view_mode`
/// check, because every caller already knows the solid image is (or is about to be) visible.
/// Always forces `view_mode` `0`: the Live tab shows only the flat-shaded raster, never the Edit
/// tab's own mode (`SolidPreviewModel.view_mode`), since the two viewports each keep their own.
///
/// Called when a library selection lands while the Live tab is already in Solid mode
/// (`apply_reconstructed_planes` and `apply_design_record_to_ui` need an immediate redraw),
/// and when `on_live_view_mode_changed` in `gui::main_window::callbacks` switches into Solid.
///
/// Sizes to the Live tab's own `SolidPreviewModel.live_viewport_width`/
/// `live_viewport_height`, scaled by `ui.window().scale_factor()` as in
/// [`resubmit_at_current_pose`].
pub(in crate::gui) fn resubmit_live_solid(
    ui: &MainWindow,
    ctx: &RenderContext,
    preview_state: &SolidPreviewState,
) {
    if ctx.active_planes.is_empty() {
        return;
    }
    let stone = active_stone(ctx);
    let scale = ui.window().scale_factor();
    let size = (
        device_pixels(ui.global::<SolidPreviewModel>().get_live_viewport_width() * scale),
        device_pixels(ui.global::<SolidPreviewModel>().get_live_viewport_height() * scale),
    );
    // Committed planes only: the Slice tool's provisional tier is an Edit-tab preview.
    preview_state.request_redraw_committed_geometry(
        stone,
        CameraPose {
            roll: ctx.camera_roll,
            yaw: ctx.yaw,
            pitch: ctx.pitch,
            distance: ctx.distance,
            pan: ctx.camera_pan,
        },
        size,
        0,
        ctx.design_gear,
    );
}

/// What a right-drag in the viewport turned: the key light (studio lighting) or the HDR map.
enum LightMoved {
    /// The light's new yaw and pitch, in radians.
    Light(f32, f32),
    /// The map's new yaw, in radians.
    Map(f32),
}

/// Wires `ViewportModel.on_light_move` and `on_light_pos_changed`, persisting through the
/// debounced `settings_store` like the other pose callbacks. They share no state with the
/// camera callbacks in [`setup_camera_and_lighting_callbacks`].
fn setup_light_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx_light = render_ctx.clone();
    let settings_store_light = settings_store.clone();
    let ui_weak_light = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_light_move(move |dx: f32, dy: f32| {
            let moved = {
                let mut ctx = RenderContext::lock(&render_ctx_light);
                let (dx, dy) = unroll_drag_delta(ctx.camera_roll, dx, dy);
                ctx.dirty = true;
                if ctx.env_map.is_some() {
                    // With a map loaded the light controls turn the map; the light angles stay.
                    ctx.hdr_yaw = dx.mul_add(0.01, ctx.hdr_yaw).rem_euclid(TAU);
                    LightMoved::Map(ctx.hdr_yaw)
                } else {
                    ctx.light_yaw = dx.mul_add(0.01, ctx.light_yaw);
                    ctx.light_pitch = dy.mul_add(0.01, ctx.light_pitch).clamp(0.15, 1.55);
                    LightMoved::Light(ctx.light_yaw, ctx.light_pitch)
                }
            };
            let ui = ui_weak_light.upgrade();
            match moved {
                LightMoved::Map(hdr_yaw) => {
                    let degrees = hdr_yaw.to_degrees().rem_euclid(360.0);
                    settings_store_light.update(|s| s.settings.env_map_yaw_deg = degrees);
                    if let Some(ui) = &ui {
                        ui.global::<SettingsModel>().set_env_map_yaw_deg(degrees);
                    }
                }
                LightMoved::Light(light_yaw, light_pitch) => {
                    settings_store_light.update(|s| {
                        s.settings.light_yaw_deg = light_yaw.to_degrees();
                        s.settings.light_pitch_deg = light_pitch.to_degrees();
                    });
                    // The Lighting tab's direction sliders follow the drag.
                    if let Some(ui) = &ui {
                        let settings = ui.global::<SettingsModel>();
                        settings.set_light_yaw_deg(light_yaw.to_degrees().rem_euclid(360.0));
                        settings.set_light_pitch_deg(light_pitch.to_degrees());
                    }
                }
            }
        });

    let render_ctx_light_pos = render_ctx.clone();
    let settings_store_light_pos = settings_store.clone();
    ui.global::<ViewportModel>()
        .on_light_pos_changed(move |yaw_deg: f32, pitch_deg: f32| {
            {
                let mut ctx = RenderContext::lock(&render_ctx_light_pos);
                ctx.light_yaw = yaw_deg.to_radians();
                ctx.light_pitch = pitch_deg.to_radians().clamp(0.15, 1.55);
                ctx.dirty = true;
            }
            settings_store_light_pos.update(|s| {
                s.settings.light_yaw_deg = yaw_deg;
                s.settings.light_pitch_deg = pitch_deg;
            });
        });
}

/// Wires `ViewportModel.on_env_map_turn_changed` (the Map Rotation slider) and
/// `on_env_map_follow_changed` (the Turns with the Camera switch). The slider turns the map's own
/// yaw. The switch keeps the picture: turning it on takes the camera's yaw out of the map's yaw
/// (the map then adds it back), turning it off puts it in, so the map does not jump either way.
/// Both write the render context, the settings file and (for the switch) the controls from here;
/// Slint has no `changed` handler in this path.
fn setup_env_map_turn_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx_turn = render_ctx.clone();
    let settings_store_turn = settings_store.clone();
    ui.global::<ViewportModel>()
        .on_env_map_turn_changed(move |degrees: f32| {
            if !degrees.is_finite() {
                return;
            }
            let degrees = degrees.rem_euclid(360.0);
            {
                let mut ctx = RenderContext::lock(&render_ctx_turn);
                ctx.hdr_yaw = degrees.to_radians();
                ctx.dirty = true;
            }
            settings_store_turn.update(|s| s.settings.env_map_yaw_deg = degrees);
        });

    let render_ctx_follow = render_ctx.clone();
    let settings_store_follow = settings_store.clone();
    let ui_weak_follow = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_env_map_follow_changed(move |on: bool| {
            let turned = {
                let mut ctx = RenderContext::lock(&render_ctx_follow);
                if ctx.hdr_follows_camera == on {
                    None
                } else {
                    let camera_yaw = ctx.yaw;
                    let own = if on {
                        ctx.hdr_yaw - camera_yaw
                    } else {
                        ctx.hdr_yaw + camera_yaw
                    };
                    ctx.hdr_yaw = own.rem_euclid(TAU);
                    ctx.hdr_follows_camera = on;
                    ctx.dirty = true;
                    Some(ctx.hdr_yaw.to_degrees().rem_euclid(360.0))
                }
            };
            let Some(degrees) = turned else {
                return;
            };
            settings_store_follow.update(|s| {
                s.settings.env_map_yaw_deg = degrees;
                s.settings.env_map_follows_camera = on;
            });
            if let Some(ui) = ui_weak_follow.upgrade() {
                let settings = ui.global::<SettingsModel>();
                settings.set_env_map_yaw_deg(degrees);
                settings.set_env_map_follows_camera(on);
            }
        });
}

/// Stores the path of a newly loaded HDR map in the settings. A different map starts unturned:
/// its saved yaw goes back to `0`. Returns whether it was a different map, so the caller resets
/// the live yaw and the slider the same way (the camera switch stays as the cutter set it).
fn remember_loaded_map(settings_store: &SettingsPersister, path: &str) -> bool {
    let mut different = false;
    settings_store.update(|s| {
        different = s.settings.env_map_path != path;
        path.clone_into(&mut s.settings.env_map_path);
        if different {
            s.settings.env_map_yaw_deg = 0.0;
        }
    });
    different
}

/// `ViewportModel.set_view`, selecting the pose by `kind`.
///
/// `0` = Front (girdle edge-on, index 0 towards the viewer), `1` = Top (straight down onto the
/// table), `2` = Bottom, `3` = Left, `4` = Right (the commonest inspection for windowing),
/// `5` = Fit, `6` = Turn 90°.
///
/// Fit keeps the current orbit angle and only recomputes `distance` (`fit_view::fit_distance`),
/// so a cutter who has orbited to an awkward angle keeps that angle. Turn 90° adds a quarter
/// turn to the yaw about the vertical axis (a face-up oval turns across the picture, Top
/// included), keeping tilt, zoom and the pan. Every kind but Fit also clears the image-plane
/// roll, so the drag directions are the usual ones. The others reset yaw to a canonical angle
/// and centre the stone in the picture again (the pan); distance and lighting stay untouched.
/// Both viewports' pose pills call this same callback.
fn setup_set_view_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
    mesh_bounding_radius: &Arc<Mutex<f64>>,
) {
    let render_ctx = render_ctx.clone();
    let settings_store = settings_store.clone();
    let preview_state = Arc::clone(preview_state);
    let mesh_bounding_radius = Arc::clone(mesh_bounding_radius);
    let ui_weak = ui.as_weak();
    ui.global::<ViewportModel>().on_set_view(move |kind: i32| {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let fitted = (kind == 5)
            .then(|| super::fit_view::fit_distance(&ui, &render_ctx, &mesh_bounding_radius));
        let mut ctx = RenderContext::lock(&render_ctx);
        if let Some(distance) = fitted {
            ctx.distance = distance;
        } else if kind == 6 {
            ctx.yaw = pleochroma_solid::preview::camera::turn_quarter_yaw(ctx.yaw);
            ctx.camera_roll = 0.0;
        } else {
            let (yaw, pitch) = match kind {
                1 => (0.0, FRAC_PI_2),  // Top
                2 => (0.0, -FRAC_PI_2), // Bottom
                3 => (-FRAC_PI_2, 0.0), // Left
                4 => (FRAC_PI_2, 0.0),  // Right
                _ => (0.0, 0.0),        // Front, and any unrecognized kind
            };
            ctx.yaw = yaw;
            ctx.pitch = pitch;
            ctx.camera_roll = 0.0;
            ctx.camera_pan = [0.0, 0.0];
        }
        ctx.dirty = true;
        resubmit_at_current_pose(&ui, &ctx, &preview_state);
        let (yaw, pitch, distance, roll, pan) = (
            ctx.yaw,
            ctx.pitch,
            ctx.distance,
            ctx.camera_roll,
            ctx.camera_pan,
        );
        drop(ctx);
        settings_store.update(|s| {
            s.settings.camera_yaw = yaw;
            s.settings.camera_pitch = pitch;
            s.settings.camera_distance = distance;
            s.settings.camera_roll = roll;
            s.settings.camera_pan = pan;
        });
    });
}

/// Wires orbit dragging through the shared camera math, including image-plane roll.
fn setup_orbit_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
) {
    let render_ctx = render_ctx.clone();
    let settings_store = settings_store.clone();
    let preview_state = Arc::clone(preview_state);
    let ui_weak = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_camera_orbit(move |dx: f32, dy: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let (yaw, pitch) = {
                let mut ctx = RenderContext::lock(&render_ctx);
                let pose = orbit_step(
                    CameraPose {
                        yaw: ctx.yaw,
                        pitch: ctx.pitch,
                        roll: ctx.camera_roll,
                        distance: ctx.distance,
                        pan: ctx.camera_pan,
                    },
                    dx,
                    dy,
                );
                ctx.yaw = pose.yaw;
                ctx.pitch = pose.pitch;
                ctx.dirty = true;
                resubmit_at_current_pose(&ui, &ctx, &preview_state);
                (ctx.yaw, ctx.pitch)
            };
            settings_store.update(|s| {
                s.settings.camera_yaw = yaw;
                s.settings.camera_pitch = pitch;
            });
        });
}

/// Wires pan dragging (`ViewportModel.camera_pan`: the middle button, or Ctrl or Cmd with the
/// left button) through [`pan_pose`]: the stone follows the pointer across the picture on
/// screen ([`pan_picture_size`]) and stays in it.
///
/// Camera motion like an orbit step: the context is marked dirty, the solid raster is redrawn
/// at the new pose and the pan is persisted. A step the clamp swallows (the stone already at
/// the picture's edge) changes nothing, so the converged image stays.
fn setup_pan_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
) {
    let render_ctx = render_ctx.clone();
    let settings_store = settings_store.clone();
    let preview_state = Arc::clone(preview_state);
    let ui_weak = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_camera_pan(move |dx: f32, dy: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let (viewport, traced) = shown_viewport(&ui);
            let pan = {
                let mut ctx = RenderContext::lock(&render_ctx);
                let (width, height) = pan_picture_size(traced, viewport, (ctx.width, ctx.height));
                let pose = pan_pose(
                    CameraPose {
                        yaw: ctx.yaw,
                        pitch: ctx.pitch,
                        roll: ctx.camera_roll,
                        distance: ctx.distance,
                        pan: ctx.camera_pan,
                    },
                    dx,
                    dy,
                    width,
                    height,
                );
                if pose.pan == ctx.camera_pan {
                    return;
                }
                ctx.camera_pan = pose.pan;
                ctx.dirty = true;
                resubmit_at_current_pose(&ui, &ctx, &preview_state);
                ctx.camera_pan
            };
            settings_store.update(|s| s.settings.camera_pan = pan);
        });
}

/// Wires up camera orbit/pan/zoom, light move/position, and reset-camera callbacks. Each also
/// feeds the debounced `settings_store` so camera pose and light position survive a restart.
///
/// `preview_state` is the solid-inspection preview controller -- dragging or
/// zooming the Live Render viewport also moves the Solid viewport's shared
/// camera (yaw/pitch/distance/pan), so `on_camera_orbit`/`on_camera_pan`/`on_camera_zoom`
/// re-issue the last solved plane set at the new pose (see
/// `resubmit_at_current_pose`) while still holding `render_ctx`'s lock.
pub(in crate::gui) fn setup_camera_and_lighting_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
    mesh_bounding_radius: &Arc<Mutex<f64>>,
) {
    setup_orbit_callback(ui, render_ctx, settings_store, preview_state);
    setup_pan_callback(ui, render_ctx, settings_store, preview_state);

    let render_ctx_zoom = render_ctx.clone();
    let settings_store_zoom = settings_store.clone();
    let preview_state_zoom = Arc::clone(preview_state);
    let mesh_bounding_radius_zoom = Arc::clone(mesh_bounding_radius);
    let ui_weak_zoom = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_camera_zoom(move |delta: f32| {
            let Some(ui) = ui_weak_zoom.upgrade() else {
                return;
            };
            let distance = {
                let mut ctx = RenderContext::lock(&render_ctx_zoom);
                // Clamp by the current mesh's bounding radius (see `orbit_distance_bounds`).
                let radius = *mesh_bounding_radius_zoom.lock().unwrap();
                let (min, max) = orbit_distance_bounds(radius);
                ctx.distance = delta.mul_add(-0.002, ctx.distance).clamp(min, max);
                ctx.dirty = true;
                resubmit_at_current_pose(&ui, &ctx, &preview_state_zoom);
                ctx.distance
            };
            settings_store_zoom.update(|s| s.settings.camera_distance = distance);
        });

    setup_light_callbacks(ui, render_ctx, settings_store);
    setup_env_map_turn_callbacks(ui, render_ctx, settings_store);

    setup_set_view_callback(
        ui,
        render_ctx,
        settings_store,
        preview_state,
        mesh_bounding_radius,
    );

    let render_ctx_reset = render_ctx.clone();
    let settings_store_reset = settings_store.clone();
    let preview_state_reset = Arc::clone(preview_state);
    let ui_weak_reset = ui.as_weak();
    ui.global::<ViewportModel>().on_reset_camera(move || {
        let Some(ui) = ui_weak_reset.upgrade() else {
            return;
        };
        {
            let mut ctx = RenderContext::lock(&render_ctx_reset);
            ctx.yaw = 0.60;
            ctx.pitch = 0.45;
            ctx.camera_roll = 0.0;
            ctx.camera_pan = [0.0, 0.0];
            ctx.distance = 2.4;
            ctx.light_yaw = crate::settings::model::DEFAULT_LIGHT_YAW_DEG.to_radians();
            ctx.light_pitch = crate::settings::model::DEFAULT_LIGHT_PITCH_DEG.to_radians();
            // The light controls turn a loaded map, so the reset puts the map back too.
            ctx.hdr_yaw = 0.0;
            ctx.hdr_follows_camera = false;
            ctx.dirty = true;
            // Resubmit now, so both viewports show the reset pose at once.
            resubmit_at_current_pose(&ui, &ctx, &preview_state_reset);
            drop(ctx);
        }
        settings_store_reset.update(|s| {
            s.settings.camera_yaw = 0.60;
            s.settings.camera_pitch = 0.45;
            s.settings.camera_roll = 0.0;
            s.settings.camera_pan = [0.0, 0.0];
            s.settings.camera_distance = 2.4;
            s.settings.light_yaw_deg = crate::settings::model::DEFAULT_LIGHT_YAW_DEG;
            s.settings.light_pitch_deg = crate::settings::model::DEFAULT_LIGHT_PITCH_DEG;
            s.settings.env_map_yaw_deg = 0.0;
            s.settings.env_map_follows_camera = false;
        });
        // The Lighting tab's direction sliders show the light the reset just moved.
        let settings = ui.global::<SettingsModel>();
        settings.set_light_yaw_deg(crate::settings::model::DEFAULT_LIGHT_YAW_DEG);
        settings.set_light_pitch_deg(crate::settings::model::DEFAULT_LIGHT_PITCH_DEG);
        settings.set_env_map_yaw_deg(0.0);
        settings.set_env_map_follows_camera(false);
    });

    setup_reset_lighting_tab_callback(ui, render_ctx, settings_store);

    // Settings are loaded by now. The built-in stand-in is framed like Fit: at the default pose
    // distance the frame would cut it.
    super::fit_view::frame_stand_in_stone(ui, render_ctx);

    super::camera_handles::register_camera(
        ui,
        render_ctx,
        settings_store,
        preview_state,
        mesh_bounding_radius,
    );
}

/// `ViewportModel.reset_lighting_tab`: the Lighting & Environment tab's "Reset this tab". Puts
/// every lighting field back to [`LightingValues::default`] -- the rig, light direction,
/// exposure, surface glare, backdrop, head shadow, Stone Shadow, Tray Reflection and the HDR
/// map (cleared: the lighting rig comes back) -- in the render context, the settings file and
/// the controls. The camera pose is not lighting and stays. Like applying a saved lighting, it is
/// a choice of the cutter's normal lighting, so a design's own remembered lighting that was
/// showing is let go (it stays saved).
fn setup_reset_lighting_tab_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx = render_ctx.clone();
    let settings_store = settings_store.clone();
    let ui_weak = ui.as_weak();
    ui.global::<ViewportModel>().on_reset_lighting_tab(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let values = LightingValues::default();
        let rig = crate::gui::optics::offered_lighting::offered_from_label(&values.lighting_rig);
        {
            let mut ctx = RenderContext::lock(&render_ctx);
            super::lighting_presets::apply_values_to_context(&mut ctx, &values, rig);
            ctx.env_map = None;
            ctx.dirty = true;
        }
        settings_store.update(|s| values.write_into(&mut s.settings));
        // The newest word on the lighting is the cutter's: nothing still on its way for the
        // open design (a lookup, an HDR decode) may land over the reset.
        super::design_lighting::normal_lighting_chosen();
        super::design_lighting::env_map_chosen();

        super::lighting_presets::push_lighting_values_to_controls(&ui, &values, rig);
        let settings = ui.global::<SettingsModel>();
        settings.set_env_map_status(String::new().into());
        settings.set_env_map_loaded(false);
        show_toast(&ui, "Lighting reset to the defaults.", "info");
    });
}

/// Mirrors `ViewportModel.camera_drag_begin`/`camera_drag_end` (both viewports' orbit, pan and
/// light drags) into `RenderContext::camera_drag_held`. Only the flag changes here: each move in
/// `on_camera_orbit`/`on_camera_pan`/`on_light_move` still restarts the interactive preview.
pub(in crate::gui) fn setup_camera_drag_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let render_ctx_begin = render_ctx.clone();
    let ui_weak_begin = ui.as_weak();
    ui.global::<ViewportModel>().on_camera_drag_begin(move || {
        RenderContext::lock(&render_ctx_begin).camera_drag_held = true;
        if let Some(ui) = ui_weak_begin.upgrade() {
            crate::bridge::render_thread::progress::set_camera_drag(&ui, true);
        }
    });

    let render_ctx_end = render_ctx.clone();
    let ui_weak_end = ui.as_weak();
    ui.global::<ViewportModel>().on_camera_drag_end(move || {
        RenderContext::lock(&render_ctx_end).camera_drag_held = false;
        if let Some(ui) = ui_weak_end.upgrade() {
            crate::bridge::render_thread::progress::set_camera_drag(&ui, false);
        }
    });
}

/// Wires `SettingsModel.clear_env_map`: drops the loaded map and its saved path, and the studio
/// rig comes back.
fn wire_clear_env_map(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx_clear = render_ctx.clone();
    let settings_store_clear = settings_store.clone();
    let ui_weak_clear = ui.as_weak();
    ui.global::<SettingsModel>().on_clear_env_map(move || {
        let Some(ui) = ui_weak_clear.upgrade() else {
            return;
        };
        {
            let mut ctx = RenderContext::lock(&render_ctx_clear);
            ctx.env_map = None;
            ctx.dirty = true;
        }
        settings_store_clear.update(|s| s.settings.env_map_path.clear());
        ui.global::<SettingsModel>()
            .set_env_map_status(String::new().into());
        ui.global::<SettingsModel>().set_env_map_loaded(false);
        // Clearing is a choice too: a decode still on its way must not bring a map back.
        super::design_lighting::env_map_chosen();
        show_toast(
            &ui,
            "Cleared HDR environment; back to the studio rig.",
            "info",
        );
    });
}

/// Wires up the HDR environment-map load and clear callbacks. The path is a typed text field
/// in the Parameters panel's "Environment Map (HDR)" section; its Browse button only fills
/// that field (`on_pick_hdr_file`).
pub(in crate::gui) fn setup_environment_map_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx_load = render_ctx.clone();
    let settings_store_load = settings_store.clone();
    let ui_weak_load = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_load_env_map(move |raw_path: SharedString| {
            let Some(ui) = ui_weak_load.upgrade() else {
                return;
            };
            let path = raw_path.to_string();
            // `load_env_map` never panics -- a missing, unreadable, or malformed file
            // returns `Err` and `RenderContext::env_map`/the settings file are left exactly
            // as they were (see that function's doc comment).
            match load_env_map(&path) {
                Ok(map) => {
                    let status = env_map_status_text(&map, &path);
                    // A different map starts unturned (the same one again keeps its turn).
                    let different = remember_loaded_map(&settings_store_load, &path);
                    {
                        let mut ctx = RenderContext::lock(&render_ctx_load);
                        ctx.env_map = Some(map);
                        if different {
                            ctx.hdr_yaw = 0.0;
                        }
                        ctx.dirty = true;
                    }
                    ui.global::<SettingsModel>()
                        .set_env_map_status(status.clone().into());
                    ui.global::<SettingsModel>().set_env_map_loaded(true);
                    if different {
                        ui.global::<SettingsModel>().set_env_map_yaw_deg(0.0);
                    }
                    // The cutter's own choice is the newest word on the map: a decode of the
                    // design's saved map still on its way must not land over it, and "Use
                    // this lighting for this design" stores this map.
                    super::design_lighting::env_map_chosen();
                    // The GPU path renders an HDR map directly, so no CPU-only warning is needed.
                    show_toast(&ui, &status, "info");
                }
                Err(err) => {
                    show_toast(
                        &ui,
                        &format!("Could not load HDR environment: {err}"),
                        "error",
                    );
                }
            }
        });

    wire_clear_env_map(ui, render_ctx, settings_store);

    // Native file-open picker for `env_map_path` ("Browse..." in
    // the Parameters panel) -- fills the field, doesn't load anything itself;
    // `on_load_env_map` above still does that, unchanged, whichever way the path got
    // typed in. Filtered to exactly `.hdr`: the only format `EnvironmentMap::from_hdr_file`
    // (crates/pleochroma/src/renderer/env_map.rs) decodes, via `image::ImageFormat::Hdr`.
    // `env_map_path` is simply left unchanged when the user cancels.
    //
    // `pick_hdr_file` is void (`ui/models/settings.slint`): Slint has no way to
    // await a value-returning callback, so the Parameters panel's click handler
    // just fires it, the dialog itself runs off the UI thread through
    // `gui::pickers::pick`, and this closure pushes the chosen path into
    // `env_map_path` from the completion continuation once the picker returns.
    let ui_weak_pick_hdr = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_pick_hdr_file(move |current: SharedString| {
            use crate::gui::pickers::pick;

            let Some(ui) = ui_weak_pick_hdr.upgrade() else {
                return;
            };
            pick(&ui, hdr_picker_request(current.as_str()), |ui, picked| {
                if let Some(path) = picked {
                    ui.global::<SettingsModel>()
                        .set_env_map_path(path.display().to_string().into());
                }
            });
        });

    // The Lighting tab's "Choose...": the same picker, but the pick is loaded at once through
    // `on_load_env_map` above (what Browse followed by Load does). Nothing happens on cancel.
    let ui_weak_choose = ui.as_weak();
    ui.global::<ViewportModel>().on_env_map_choose(move || {
        use crate::gui::pickers::pick;

        let Some(ui) = ui_weak_choose.upgrade() else {
            return;
        };
        let current = ui.global::<SettingsModel>().get_env_map_path();
        pick(&ui, hdr_picker_request(current.as_str()), |ui, picked| {
            if let Some(path) = picked {
                let path = SharedString::from(path.display().to_string());
                let settings = ui.global::<SettingsModel>();
                settings.set_env_map_path(path.clone());
                settings.invoke_load_env_map(path);
            }
        });
    });

    // The Lighting tab's clear button: today's "Clear" (the studio rig comes back).
    let ui_weak_unload = ui.as_weak();
    ui.global::<ViewportModel>().on_env_map_clear(move || {
        if let Some(ui) = ui_weak_unload.upgrade() {
            ui.global::<SettingsModel>().invoke_clear_env_map();
        }
    });

    // `ViewportModel.env_map_name` follows the status line, which every load, clear, reset,
    // saved lighting and design lighting already writes.
    let ui_weak_name = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_env_map_status_changed(move |status: SharedString| {
            if let Some(ui) = ui_weak_name.upgrade() {
                ui.global::<ViewportModel>()
                    .set_env_map_name(crate::gui::env_map_name_from_status(&status).into());
            }
        });
}

/// The file picker request for a Radiance `.hdr` map: the only format
/// `EnvironmentMap::from_hdr_file` decodes. `current` is the path field's text, used to pick the
/// folder the dialog opens in.
fn hdr_picker_request(current: &str) -> crate::gui::pickers::PickerRequest {
    use crate::gui::pickers::{PickerFilter, PickerKind, PickerRequest};

    PickerRequest {
        kind: PickerKind::OpenFile,
        title: None,
        filters: vec![PickerFilter {
            label: "Radiance HDR".to_string(),
            extensions: vec!["hdr".to_string()],
        }],
        default_file_name: None,
        starting_dir: crate::gui::starting_dir_from_picker_field(current),
    }
}

#[cfg(test)]
#[path = "camera_lighting_tests.rs"]
mod tests;

#[cfg(test)]
mod pan_tests {
    use super::pan_picture_size;
    use pleochroma_solid::preview::camera::{RESET_POSE, pan_pose};

    #[test]
    fn the_solid_raster_pans_across_the_whole_viewport() {
        assert_eq!(
            pan_picture_size(false, (900.0, 700.0), (1920, 1080)),
            (900.0, 700.0)
        );
    }

    #[test]
    fn a_traced_picture_pans_across_its_letterboxed_rectangle() {
        // A 16:9 render fills a 900x700 viewport's width; a 4:3 one fills a 900x500
        // viewport's height, as `image-fit: contain` draws them.
        assert_eq!(
            pan_picture_size(true, (900.0, 700.0), (1920, 1080)),
            (900.0, 506.25)
        );
        let (width, height) = pan_picture_size(true, (900.0, 500.0), (640, 480));
        assert!((width - 2000.0 / 3.0).abs() < 1e-3, "{width}");
        assert!((height - 500.0).abs() < 1e-3, "{height}");
    }

    #[test]
    fn an_unknown_render_size_pans_across_the_viewport() {
        assert_eq!(
            pan_picture_size(true, (900.0, 700.0), (0, 1080)),
            (900.0, 700.0)
        );
        assert_eq!(
            pan_picture_size(true, (900.0, 700.0), (1920, 0)),
            (900.0, 700.0)
        );
    }

    #[test]
    fn the_stone_reaches_the_edges_of_the_traced_picture_and_no_further() {
        let (width, height) = pan_picture_size(true, (900.0, 700.0), (1920, 1080));
        let far = pan_pose(RESET_POSE, 10_000.0, -10_000.0, width, height);
        assert!((far.pan[0] - 1920.0 / 1080.0).abs() < 1e-5, "{:?}", far.pan);
        assert_eq!(far.pan[1], 1.0);
    }
}
