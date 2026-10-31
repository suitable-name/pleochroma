//! Camera orbit/zoom, light move/position/reset, and HDR environment-map load/clear
//! callback wiring.
//!
//! Split out of `gui::mod` purely to keep that module (already sizeable) from growing
//! further -- same reasoning as `gui::detail`/`gui::search`/`gui::remote`.

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
    camera::{orbit_step, unroll_drag_delta},
};
use slint::{ComponentHandle, SharedString};
use std::{
    f32::consts::FRAC_PI_2,
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

/// Re-issues `ctx`'s current `active_planes` at its (just-updated) camera pose to
/// `preview_state`, sized to the Solid viewport's own logical size --
/// camera-follow only (same
/// planes, new pose), never a re-solve. A no-op while nothing has been solved
/// into the shared viewport yet (`active_planes` empty), matching
/// `gui::editor::view::refresh_viewport`'s own "no design, no planes" contract.
///
/// Picks which viewport's own view-mode to redraw at, and whether to bother at all:
/// - Live Render tab (`ui.get_render_view_tab() == 0`): always Solid (`view_mode` 0,
///   forced -- see [`resubmit_live_solid`]'s own doc comment for why this must never
///   leak the Edit tab's `SolidPreviewModel.view_mode`), and ONLY when that tab's own
///   `ViewportModel.live_view_mode` is actually `0` (Solid) -- a camera drag while the
///   Live tab is showing the path-traced image has no solid on screen to redraw, so
///   this is a no-op rather than wasted background work.
/// - Edit tab (`render_view_tab != 0`): the Edit tab's own `SolidPreviewModel.
///   view_mode`, unconditionally -- unchanged from before `live_view_mode` existed
///   (the Solid viewport's camera-follow always ran regardless of which of its four
///   modes was showing, so it was already up to date the moment the user switched to
///   it).
///
/// Also forwards `ctx.design_gear` (the currently loaded design's gear tooth
/// count/reference angle) so a Diagram-mode (`view_mode` 3) camera drag shows THIS
/// design's gear wheel -- via [`SolidPreviewState::request_redraw_with_gear`],
/// not the plain `request_redraw` -- rather than whatever gear info a previous
/// editor replan happened to leave in the worker's own `DiagramMemory`, which is
/// unrelated once the currently viewed design was loaded some other way (e.g. from
/// the library, never replanned through the editor). See
/// `bridge::render_thread::RenderContext::design_gear`'s doc comment for the full
/// list of writers that keep it in sync with `active_planes`.
///
/// Sizes to the Live Render tab's OWN `SolidPreviewModel.
/// live_viewport_width`/`live_viewport_height` while `on_live_tab`, never the Edit
/// tab's `viewport_width`/`viewport_height`: `gem_viewport.slint` and
/// `solid_viewport.slint` each drive their own pair of properties, keeping one
/// viewport's resize from silently overwriting the other's pending redraw size.
///
/// Both dimensions are scaled by `ui.window().scale_factor()` so the solid
/// rasterizer/diagram renderer produce a HiDPI-correct (physical-pixel) raster
/// instead of a logical-pixel one Slint then has to upscale blurrily. The matching
/// conversion on the input side multiplies the pointer's LOGICAL hover/click
/// position by the same factor to reach the physical pixel it names in the pick
/// buffer; it lives in `solid_preview::diagram_wiring` for the diagram and in
/// `gui::editor::callbacks::tier_actions` for the solid view.
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
        // Path-traced/Both letterbox the traced image to `ctx.width`/
        // `ctx.height`'s own aspect ratio (`solid_viewport.slint`'s
        // `image-fit: contain`) whenever it differs from the viewport's --
        // `contained_request_size`'s own doc comment.
        contained_request_size(view_mode, viewport_size, (ctx.width, ctx.height))
    };
    let pose = CameraPose {
        roll: ctx.camera_roll,
        yaw: ctx.yaw,
        pitch: ctx.pitch,
        distance: ctx.distance,
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

/// Shrinks `viewport_size` to the rectangle Slint's `image-fit: contain` draws
/// `render_size` into.
///
/// `solid_viewport.slint`'s Path-traced/Both `Image` elements (`ViewportModel.
/// render_image`, `SolidPreviewModel.edges_image`) are both `image-fit: contain`
/// against a `render_size` (the configured render resolution, `RenderContext::
/// width`/`height`) that is independent of the viewport's own logical size --
/// with a render resolution wider than the viewport (a common pairing: 1920x1080
/// against a docked ~1.2-1.5 aspect Edit viewport), `contain` fits the image's
/// WIDTH and letterboxes it top/bottom, drawn SMALLER than a solid/edges raster
/// still requested at the viewport's raw, un-letterboxed size. The edges overlay
/// then reads as larger than the render it is supposed to trace, and a click
/// lands on the visible (letterboxed) gem while indexing a pick buffer sized to
/// the viewport's own full rectangle. Requesting the solid/edges raster at THIS
/// function's output size instead keeps every pixel of the request inside the
/// same rectangle the traced image actually occupies on screen.
///
/// Used for traced modes: Path-Traced (`1`), Both (`2`) and Ray-Traced (`4`) -- Solid (`0`) and
/// Diagram (`3`) never show `render_image` at all, so the viewport's own raw
/// size is already correct for them, returned unchanged (also returned unchanged
/// if either dimension is `0`, i.e. nothing to divide by, or nothing requested
/// yet).
///
/// `gui::editor::view::planned_frame_size`'s own callers
/// (`refresh_viewport`, `submit_preview_replan`) compute
/// the SAME viewport-sized request independently, for the post-edit/post-solve
/// path this function's own caller (`resubmit_at_current_pose`) does not cover.
/// They need the identical treatment: read `SolidPreviewModel.view_mode` and
/// `render_ctx.width`/`height` at the call site and pass the result through this
/// function (or an equivalent copy) before calling `request_replan`/
/// `request_redraw_with_gear`, or the misregistration this fixes for a camera
/// drag/zoom/view-mode switch will still reappear immediately after the next
/// edit or Solve.
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

// The orbit clamp (`orbit_distance_bounds`), the "Fit" distance
// (`fit_distance_for_radius`) and the pitch wrap (`wrap_pitch`) moved to
// `pleochroma_solid::preview::camera` (shared with the web app's Solid view);
// re-exported here at their old paths. This module's tests below stay as their
// pins.
pub use pleochroma_solid::preview::camera::{
    fit_distance_for_radius, orbit_distance_bounds, wrap_pitch,
};

/// The Live Render tab's own solid-raster redraw: identical to
/// [`resubmit_at_current_pose`]'s Live-tab branch, except it never checks
/// `ViewportModel.live_view_mode` first -- every call site here already knows the
/// solid image is (about to become) visible before calling this (a library
/// selection while already in Solid mode, or `live_view_mode` itself just flipping to
/// `0`), so re-checking would be redundant. ALWAYS forces `view_mode` `0` (Solid
/// raster): the Live Render tab shows only the flat-shaded raster, never the Edit
/// tab's own Path-traced/Both/Diagram choice (`SolidPreviewModel.view_mode`), which
/// must not leak across tabs -- two independent viewports sharing one worker/one
/// `SolidPreviewState`, each entitled to its own mode.
///
/// Called from: `gui::library::diagram_list::setup_diagram_selection_and_export_callbacks`/
/// `gui::library::detail::apply_design_record_to_ui` (a library selection lands while
/// the Live tab is already in Solid mode -- the planes `apply_reconstructed_planes`
/// just wrote need an immediate redraw, not a wait for the next camera drag), and
/// `gui::mod`'s `on_live_view_mode_changed` handler (switching INTO Solid mode itself
/// must show something immediately, not wait for a drag).
///
/// Always sizes to the Live Render tab's OWN `SolidPreviewModel.
/// live_viewport_width`/`live_viewport_height` (never the Edit tab's
/// `viewport_width`/`viewport_height` -- see [`resubmit_at_current_pose`]'s doc
/// comment), scaled by `ui.window().scale_factor()` for a HiDPI-correct raster.
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
        },
        size,
        0,
        ctx.design_gear,
    );
}

/// Wires `ViewportModel.on_light_move`/`on_light_pos_changed`, persisting the
/// result through the debounced `settings_store` exactly like every other pose
/// callback in this module. Split out of [`setup_camera_and_lighting_callbacks`]
/// purely to keep that function under clippy's function-length lint (the
/// `mesh_bounding_radius` clamp added there pushed it over the limit) -- these two
/// callbacks are otherwise unrelated to the camera pose/distance ones that stayed
/// behind.
fn setup_light_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx_light = render_ctx.clone();
    let settings_store_light = settings_store.clone();
    ui.global::<ViewportModel>()
        .on_light_move(move |dx: f32, dy: f32| {
            let (light_yaw, light_pitch) = {
                let mut ctx = RenderContext::lock(&render_ctx_light);
                let (dx, dy) = unroll_drag_delta(ctx.camera_roll, dx, dy);
                ctx.light_yaw = dx.mul_add(0.01, ctx.light_yaw);
                ctx.light_pitch = dy.mul_add(0.01, ctx.light_pitch).clamp(0.15, 1.55);
                ctx.dirty = true;
                (ctx.light_yaw, ctx.light_pitch)
            };
            settings_store_light.update(|s| {
                s.settings.light_yaw_deg = light_yaw.to_degrees();
                s.settings.light_pitch_deg = light_pitch.to_degrees();
            });
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

/// `ViewportModel.set_view`: `0` = Front (girdle edge-on, index 0 towards the
/// viewer), `1` = Top (straight down onto the table, the batch previews' own top
/// pose), `2` = Bottom, `3` = Left, `4` = Right (the commonest inspection for
/// windowing), and `5` = Fit, which uniquely leaves the CURRENT
/// orbit angle alone and only recomputes `distance` so the whole stone is in the
/// picture (`fit_view::fit_distance`), so a
/// cutter who has orbited to an awkward angle can recover a usable framing
/// without also losing the angle they were inspecting. `6` = Turn 90° adds a quarter
/// turn to the yaw (the stone turns about its vertical axis, so a face-up oval turns across
/// the picture, the Top view included), keeping the tilt and zoom. Every kind but Fit also
/// clears an image-plane roll left by an older version, so the drag directions are the
/// usual ones again. The others reset yaw to a canonical angle; distance and lighting stay
/// untouched for those.
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
        }
        ctx.dirty = true;
        resubmit_at_current_pose(&ui, &ctx, &preview_state);
        let (yaw, pitch, distance, roll) = (ctx.yaw, ctx.pitch, ctx.distance, ctx.camera_roll);
        drop(ctx);
        settings_store.update(|s| {
            s.settings.camera_yaw = yaw;
            s.settings.camera_pitch = pitch;
            s.settings.camera_distance = distance;
            s.settings.camera_roll = roll;
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

/// Wires up camera orbit/zoom, light move/position, and reset-camera callbacks. Each
/// also feeds the debounced `settings_store` so camera pose and light position survive
/// a restart. Split out of `run_gui` purely to keep that function under
/// clippy's function-length lint.
///
/// `preview_state` is the solid-inspection preview controller -- dragging or
/// zooming the Live Render viewport also moves the Solid viewport's shared
/// camera (yaw/pitch/distance), so both `on_camera_orbit`/`on_camera_zoom` below
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
                // The clamp is sized to the CURRENT solid's own bounding radius
                // rather than a fixed `[1.2, 8.0]` range --
                // `orbit_distance_bounds`'s own doc comment.
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
            ctx.distance = 2.4;
            ctx.light_yaw = crate::settings::model::DEFAULT_LIGHT_YAW_DEG.to_radians();
            ctx.light_pitch = crate::settings::model::DEFAULT_LIGHT_PITCH_DEG.to_radians();
            ctx.dirty = true;
            // Immediately resubmits at the new pose so both viewports (the Edit
            // tab's Solid preview and, when it is showing the solid raster, the
            // Live Render tab) show the reset pose right away, rather than
            // continuing to display the old pose until some unrelated trigger (a
            // camera drag, a view-mode switch) happens to redraw next.
            resubmit_at_current_pose(&ui, &ctx, &preview_state_reset);
            // Explicit: releases the `RenderContext` mutex before
            // `settings_store_reset.update` below, rather than leaving it held
            // until this block's closing brace.
            drop(ctx);
        }
        settings_store_reset.update(|s| {
            s.settings.camera_yaw = 0.60;
            s.settings.camera_pitch = 0.45;
            s.settings.camera_roll = 0.0;
            s.settings.camera_distance = 2.4;
            s.settings.light_yaw_deg = crate::settings::model::DEFAULT_LIGHT_YAW_DEG;
            s.settings.light_pitch_deg = crate::settings::model::DEFAULT_LIGHT_PITCH_DEG;
        });
        // The Lighting tab's direction sliders show the light the reset just moved.
        let settings = ui.global::<SettingsModel>();
        settings.set_light_yaw_deg(crate::settings::model::DEFAULT_LIGHT_YAW_DEG);
        settings.set_light_pitch_deg(crate::settings::model::DEFAULT_LIGHT_PITCH_DEG);
    });

    setup_reset_lighting_tab_callback(ui, render_ctx, settings_store);

    // The settings are loaded by now: before any design is opened the built-in stand-in
    // stone is framed like Fit, not at the default pose distance (which cut it on all sides).
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
/// exposure, surface glare, backdrop, head shadow and the HDR map (cleared: the lighting rig
/// comes back) -- in the render context, the settings file and the controls. The camera pose is
/// not lighting and stays. Like applying a saved lighting, it is a choice of the cutter's normal
/// lighting, so a design's own remembered lighting that was showing is let go (it stays saved).
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

/// Mirrors `ViewportModel.camera_drag_begin`/`camera_drag_end` (both viewports' orbit
/// and light drags) into `RenderContext::camera_drag_held` -- see that field's doc
/// comment. Nothing else changes here: the per-move
/// restart in `on_camera_orbit`/`on_light_move` IS the interactive preview and stays.
pub(in crate::gui) fn setup_camera_drag_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let render_ctx_begin = render_ctx.clone();
    ui.global::<ViewportModel>().on_camera_drag_begin(move || {
        RenderContext::lock(&render_ctx_begin).camera_drag_held = true;
    });

    let render_ctx_end = render_ctx.clone();
    ui.global::<ViewportModel>().on_camera_drag_end(move || {
        RenderContext::lock(&render_ctx_end).camera_drag_held = false;
    });
}

/// Wires up the HDR environment-map load/clear callbacks. Path is a plain typed text
/// field (the Parameters panel's "Environment Map (HDR)" section) with no native
/// picker of its own -- unlike `gui::library::setup_import_callback`'s `.asc`
/// pickers, which go through `gui::pickers`. Split out of
/// `run_gui` purely to keep that function under clippy's function-length lint.
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
                    {
                        let mut ctx = RenderContext::lock(&render_ctx_load);
                        ctx.env_map = Some(map);
                        ctx.dirty = true;
                    }
                    settings_store_load.update(|s| s.settings.env_map_path.clone_from(&path));
                    ui.global::<SettingsModel>()
                        .set_env_map_status(status.clone().into());
                    ui.global::<SettingsModel>().set_env_map_loaded(true);
                    // The cutter's own choice is the newest word on the map: a decode of the
                    // design's saved map still on its way must not land over it, and "Use
                    // this lighting for this design" stores this map.
                    super::design_lighting::env_map_chosen();
                    // The GPU megakernel has its own `env_mode` for `HdrMap` and
                    // renders it directly (see `render_thread::mod`'s module doc
                    // comment), so there is no CPU-only restriction to warn about
                    // here.
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
            use crate::gui::pickers::{PickerFilter, PickerKind, PickerRequest, pick};

            let Some(ui) = ui_weak_pick_hdr.upgrade() else {
                return;
            };
            let starting_dir = crate::gui::starting_dir_from_picker_field(current.as_str());
            let request = PickerRequest {
                kind: PickerKind::OpenFile,
                title: None,
                filters: vec![PickerFilter {
                    label: "Radiance HDR".to_string(),
                    extensions: vec!["hdr".to_string()],
                }],
                default_file_name: None,
                starting_dir,
            };
            pick(&ui, request, |ui, picked| {
                if let Some(path) = picked {
                    ui.global::<SettingsModel>()
                        .set_env_map_path(path.display().to_string().into());
                }
            });
        });
}

#[cfg(test)]
#[path = "camera_lighting_tests.rs"]
mod tests;
