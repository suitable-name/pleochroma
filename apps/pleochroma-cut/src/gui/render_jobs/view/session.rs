//! The view of a render job in the live view: what is showing, and the ways it ends.
//!
//! Load Job View ([`super::load`]) puts a job's stone, material, camera and lighting on the live
//! view. This module remembers what the cutter had before the first Load (the render context and
//! the controls; the lighting and the settings are the design-lighting session's,
//! `gui::render::design_lighting`), keeps the banner's texts, and puts everything back when the
//! view ends. Nothing is persisted: the settings file is pinned for as long as the view shows
//! (`design_lighting::job_view_pin`), so closing or killing the app leaves the cutter's own look
//! in it.
//!
//! [`end`] does nothing when no job view shows, so the hooks of the other modules call it
//! unconditionally. The view ends when a design opens, a library row is selected, the editor
//! changes the design, a saved lighting is applied, the job is deleted, or the cutter goes back.

use crate::{
    MainWindow, RenderJobsModel, SettingsModel, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::push_viewport_owner,
        render::{design_lighting, stone_pose::push_stone_pose_to_controls},
    },
};
use pleochroma::render_setup::StonePose;
use slint::{ComponentHandle as _, ModelRc, SharedString, VecModel};
use std::{
    cell::{Cell, RefCell},
    sync::{Arc, Mutex, Weak},
};

/// Why a job view ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::gui) enum EndReason {
    /// The cutter pressed Back to my view.
    BackToMyView,
    /// A design is opening (the caller shows its saved lighting next).
    DesignOpened,
    /// A library row was selected (the caller claims the viewport next).
    LibrarySelected,
    /// The editor changed the design, or the Edit view took the viewport back.
    EditorEdit,
    /// A saved lighting was applied or the lighting tab was reset: the cutter chose a new look.
    LightingChosen,
    /// The job whose view shows is gone.
    JobDeleted,
}

impl EndReason {
    /// Whether the open design's saved lighting shows again after this ending.
    ///
    /// A design opening shows the lighting of the design it opens itself, and a chosen lighting is
    /// the new normal look; every other ending leaves the design as it was.
    #[must_use]
    pub(in crate::gui) const fn shows_design_lighting_again(self) -> bool {
        !matches!(self, Self::DesignOpened | Self::LightingChosen)
    }
}

/// A closure that writes one remembered part of the render context back.
type Restore = Box<dyn FnOnce(&mut RenderContext)>;

/// The render context as the cutter had it before the first Load.
///
/// The stone (planes, tools, frosting, effects, owner, the editor's stashed material), the
/// material and its overrides, the camera, the Stone Pose, the bounce cap and the engine. The
/// lighting is not in it: the design-lighting session holds that.
pub(super) struct CtxLook {
    restores: Vec<Restore>,
}

impl CtxLook {
    /// Remembers `ctx`. The view color pattern is taken out of it (it cannot be copied); the
    /// restore puts it back.
    pub(super) fn capture(ctx: &mut RenderContext) -> Self {
        Self {
            restores: vec![
                capture_stone(ctx),
                capture_material(ctx),
                capture_camera(ctx),
            ],
        }
    }

    /// Writes everything back and asks for a new frame.
    pub(super) fn restore(self, ctx: &mut RenderContext) {
        for restore in self.restores {
            restore(ctx);
        }
        ctx.dirty = true;
    }
}

/// The stone, as one restore.
fn capture_stone(ctx: &mut RenderContext) -> Restore {
    let planes = Arc::clone(&ctx.active_planes);
    let frosted = Arc::clone(&ctx.active_frosted);
    let tools = Arc::clone(&ctx.active_tools);
    let placements = ctx.active_placements.clone();
    let effects = ctx.scene_effects.clone();
    let surfaces = ctx.surface_table.clone();
    let gear = ctx.design_gear;
    let owner = ctx.planes_owner;
    let over_generation = ctx.catalogue_over_generation;
    let stash = ctx.editor_material_stash.clone();
    let pattern = ctx.view_color_pattern.take();
    Box::new(move |ctx| {
        ctx.active_planes = planes;
        ctx.active_frosted = frosted;
        ctx.active_tools = tools;
        ctx.active_placements = placements;
        ctx.scene_effects = effects;
        ctx.surface_table = surfaces;
        ctx.design_gear = gear;
        ctx.planes_owner = owner;
        ctx.catalogue_over_generation = over_generation;
        ctx.editor_material_stash = stash;
        ctx.view_color_pattern = pattern;
        ctx.job_design = None;
    })
}

/// The material and the overrides the job view sets, as one restore.
fn capture_material(ctx: &RenderContext) -> Restore {
    let name = ctx.material_name.clone();
    let material = ctx.material_override.clone();
    let unresolved = ctx.material_unresolved.clone();
    let body_color = ctx.view_body_color.clone();
    let sigma = ctx.inclusion_sigma_s;
    let c_axis = ctx.c_axis_override;
    let alpha_axis = ctx.alpha_axis_override;
    let frosted = ctx.girdle_frosted;
    let rounding = ctx.edge_rounding_radius;
    let width = ctx.stone_width_mm;
    Box::new(move |ctx| {
        ctx.material_name = name;
        ctx.material_override = material;
        ctx.material_unresolved = unresolved;
        ctx.view_body_color = body_color;
        ctx.inclusion_sigma_s = sigma;
        ctx.c_axis_override = c_axis;
        ctx.alpha_axis_override = alpha_axis;
        ctx.girdle_frosted = frosted;
        ctx.edge_rounding_radius = rounding;
        ctx.stone_width_mm = width;
    })
}

/// The camera, the Stone Pose, the bounce cap and the engine, as one restore.
fn capture_camera(ctx: &RenderContext) -> Restore {
    let (yaw, pitch, roll) = (ctx.yaw, ctx.pitch, ctx.camera_roll);
    let pan = ctx.camera_pan;
    let distance = ctx.distance;
    let pose = ctx.stone_pose;
    let bounces = ctx.max_bounces;
    let engine = ctx.engine;
    Box::new(move |ctx| {
        ctx.yaw = yaw;
        ctx.pitch = pitch;
        ctx.camera_roll = roll;
        ctx.camera_pan = pan;
        ctx.distance = distance;
        ctx.stone_pose = pose;
        ctx.max_bounces = bounces;
        ctx.engine = engine;
    })
}

/// The controls the job view changes, as the cutter had them before the first Load.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ControlsLook {
    /// `ViewportModel.live_view_mode` (the engine pill of the live view).
    pub live_view_mode: i32,
    /// `ViewportModel.selected_material_index`.
    pub material_index: i32,
    /// `SettingsModel.bounce_index`.
    pub bounce_index: i32,
    /// `SettingsModel.quality_preset_index`.
    pub quality_preset_index: i32,
    /// `SettingsModel.edge_rounding_radius`.
    pub edge_rounding_radius: f32,
    /// `SettingsModel.girdle_frosted`.
    pub girdle_frosted: bool,
    /// `SettingsModel.stone_width_mm`.
    pub stone_width_mm: f32,
    /// `SettingsModel.ray_taps`.
    pub ray_taps: i32,
    /// `SettingsModel.ray_fire`.
    pub ray_fire: bool,
    /// The Stone Pose the pose controls showed.
    pub stone_pose: [f32; 3],
}

impl ControlsLook {
    /// Reads the controls; `pose` is the context's Stone Pose.
    pub(super) fn read(ui: &MainWindow, pose: StonePose) -> Self {
        let settings = ui.global::<SettingsModel>();
        let viewport = ui.global::<ViewportModel>();
        Self {
            live_view_mode: viewport.get_live_view_mode(),
            material_index: viewport.get_selected_material_index(),
            bounce_index: settings.get_bounce_index(),
            quality_preset_index: settings.get_quality_preset_index(),
            edge_rounding_radius: settings.get_edge_rounding_radius(),
            girdle_frosted: settings.get_girdle_frosted(),
            stone_width_mm: settings.get_stone_width_mm(),
            ray_taps: settings.get_ray_taps(),
            ray_fire: settings.get_ray_fire(),
            stone_pose: pose.to_array(),
        }
    }

    /// Shows these values. Setting a property from Rust fires no control callback, except the
    /// view mode's own `changed` handler, which is what switches the engine.
    pub(super) fn write(&self, ui: &MainWindow) {
        let settings = ui.global::<SettingsModel>();
        let viewport = ui.global::<ViewportModel>();
        settings.set_bounce_index(self.bounce_index);
        settings.set_quality_preset_index(self.quality_preset_index);
        settings.set_edge_rounding_radius(self.edge_rounding_radius);
        settings.set_girdle_frosted(self.girdle_frosted);
        settings.set_stone_width_mm(self.stone_width_mm);
        settings.set_ray_taps(self.ray_taps);
        settings.set_ray_fire(self.ray_fire);
        push_stone_pose_to_controls(ui, StonePose::from_array(self.stone_pose), false);
        viewport.set_selected_material_index(self.material_index);
        viewport.set_live_view_mode(self.live_view_mode);
    }
}

/// What the view remembers while a job's view shows.
struct ViewState {
    job_id: i64,
    ctx_look: CtxLook,
    controls: ControlsLook,
    /// The banner's notes, so one that lands later (a missing HDR map) joins the list.
    notes: Vec<String>,
}

thread_local! {
    /// The job view that shows, if any (UI thread only).
    static VIEW: RefCell<Option<ViewState>> = const { RefCell::new(None) };
    /// The render context (held weakly, so a released window state is not kept alive).
    static RENDER_CTX: RefCell<Weak<Mutex<RenderContext>>> = const { RefCell::new(Weak::new()) };
    /// Counts every Load and every end; an answer that was asked for an older number is stale.
    static GENERATION: Cell<u64> = const { Cell::new(0) };
}

/// The id of the job whose view shows, or `None`.
#[must_use]
pub(in crate::gui) fn active_job() -> Option<i64> {
    VIEW.with(|view| view.borrow().as_ref().map(|state| state.job_id))
}

/// Remembers the render context the view works on. Called once from `view::setup`.
pub(super) fn install(render_ctx: &Arc<Mutex<RenderContext>>) {
    RENDER_CTX.with(|cell| *cell.borrow_mut() = Arc::downgrade(render_ctx));
}

/// The render context, or `None` before `view::setup` or after the window state was released.
pub(super) fn render_ctx() -> Option<Arc<Mutex<RenderContext>>> {
    RENDER_CTX.with(|cell| cell.borrow().upgrade())
}

/// Forgets what a previous main window left behind. See `gui::window_release`.
pub(in crate::gui) fn release_window_state() {
    let old = VIEW.with(|view| view.borrow_mut().take());
    drop(old);
    RENDER_CTX.with(|cell| *cell.borrow_mut() = Weak::new());
}

/// The number of the latest Load or end.
pub(super) fn generation() -> u64 {
    GENERATION.with(Cell::get)
}

fn bump_generation() -> u64 {
    GENERATION.with(|cell| {
        let next = cell.get().wrapping_add(1);
        cell.set(next);
        next
    })
}

/// Whether an answer asked for under `asked` still belongs to the view of job `id`.
pub(super) fn is_current(asked: u64, id: i64) -> bool {
    asked == generation() && active_job() == Some(id)
}

/// Marks job `id` as the one that shows, and returns the number its answers must carry.
///
/// Only the first Load (no job view showing) remembers the cutter's look, from `ctx` and `ui`; a
/// second Load replaces the first without taking a new snapshot.
pub(super) fn begin(ui: &MainWindow, ctx: &mut RenderContext, id: i64) -> u64 {
    let number = bump_generation();
    VIEW.with(|view| {
        let mut view = view.borrow_mut();
        if let Some(state) = view.as_mut() {
            state.job_id = id;
            state.notes.clear();
        } else {
            *view = Some(ViewState {
                job_id: id,
                ctx_look: CtxLook::capture(ctx),
                controls: ControlsLook::read(ui, ctx.stone_pose),
                notes: Vec::new(),
            });
        }
    });
    number
}

/// Shows the banner: the title and subtitle, and the notes of the view.
pub(super) fn publish_banner(
    ui: &MainWindow,
    id: i64,
    title: &str,
    subtitle: &str,
    notes: &[String],
) {
    VIEW.with(|view| {
        if let Some(state) = view.borrow_mut().as_mut() {
            state.notes = notes.to_vec();
        }
    });
    let model = ui.global::<RenderJobsModel>();
    model.set_job_view_id(i32::try_from(id).unwrap_or(i32::MAX));
    model.set_job_view_title(title.into());
    model.set_job_view_subtitle(subtitle.into());
    model.set_job_view_notes(notes_model(notes));
}

/// Adds `note` to the banner when the answer it belongs to is still current.
pub(super) fn add_note(ui: &MainWindow, asked: u64, id: i64, note: &str) {
    if !is_current(asked, id) {
        return;
    }
    let notes = VIEW.with(|view| {
        let mut view = view.borrow_mut();
        let state = view.as_mut()?;
        state.notes.push(note.to_owned());
        Some(state.notes.clone())
    });
    if let Some(notes) = notes {
        ui.global::<RenderJobsModel>()
            .set_job_view_notes(notes_model(&notes));
    }
}

fn notes_model(notes: &[String]) -> ModelRc<SharedString> {
    let rows: Vec<SharedString> = notes
        .iter()
        .map(|note| SharedString::from(note.as_str()))
        .collect();
    ModelRc::new(VecModel::from(rows))
}

/// Hides the banner and forgets the Update state the banner showed.
fn clear_banner(ui: &MainWindow) {
    let model = ui.global::<RenderJobsModel>();
    model.set_job_view_id(-1);
    model.set_job_view_title(SharedString::new());
    model.set_job_view_subtitle(SharedString::new());
    model.set_job_view_notes(ModelRc::default());
    model.set_job_view_can_update(false);
    model.set_job_view_update_reason(SharedString::new());
}

/// Ends the view of a render job: restores the normal look and hides the banner.
///
/// Does nothing when no job view shows. The render context, the controls, the lighting and the
/// settings return to what the first Load found; the settings file was never changed. Back to my
/// view, a deleted job, an editor edit and a library selection then show the open design's saved
/// lighting again ([`EndReason::shows_design_lighting_again`]).
pub(in crate::gui) fn end(ui: &MainWindow, reason: EndReason) {
    let Some(state) = VIEW.with(|view| view.borrow_mut().take()) else {
        return;
    };
    bump_generation();
    if let Some(render_ctx) = render_ctx() {
        let owner = {
            let mut ctx = RenderContext::lock(&render_ctx);
            state.ctx_look.restore(&mut ctx);
            ctx.planes_owner
        };
        push_viewport_owner(ui, owner);
    }
    state.controls.write(ui);
    design_lighting::release_job_view(ui);
    clear_banner(ui);
    // The color controls read the material and the owner that are back now.
    ui.global::<ViewportModel>().invoke_render_color_refresh();
    if reason.shows_design_lighting_again() {
        design_lighting::reapply_design_lighting(ui);
    }
}
