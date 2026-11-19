//! Load Job View: a render job's stone, material, camera and lighting on the live view.
//!
//! The job's resolved scene carries everything a view needs. The stone is claimed as
//! `PlanesOwner::Job` (the editor document and the library selection are not touched); the material
//! goes in as the material override, with the stone size read back from its absorption scale; the
//! frozen exposure is divided by the meter's factor and the frozen tray radiance is matched to the
//! nearest tray; the lighting goes through the design-lighting session
//! (`gui::render::design_lighting`), which pins the cutter's normal look on disk while the view
//! shows. What the cutter had is remembered once, at the first Load ([`super::session`]).
//!
//! The decisions ([`look_from_job`], [`exposure_shown`], [`backdrop_shown`]) are pure functions,
//! tested without a window; the rest is thin glue over Slint and the render context.

use super::session::{self, EndReason};
use crate::{
    GuideModel, MainWindow, RenderJobsModel, SettingsModel, ViewportModel,
    bridge::render_thread::{JobDesignNames, JobStone, PlanesOwner, RenderContext},
    gui::{
        commands,
        editor::push_viewport_owner,
        optics::offered_lighting::offered,
        render::{design_lighting, stone_pose::push_stone_pose_to_controls},
        render_jobs::{
            convert,
            edit::load::{install_db, load_job_for_edit},
        },
        show_toast,
        startup_settings::find_option_index,
        view_modes::LiveViewMode,
    },
    settings::{
        SettingsPersister,
        model::{Backdrop, EXPOSURE_RANGE, LightingValues, bounce_index},
    },
};
use pleochroma::{
    optics::{materials::GemMaterial, raytracer::LightingPreset},
    render_setup::{StonePose, measure_model_width, stone_width_for_scale},
    renderer::{engine::EngineSpec, env_map::EnvironmentMap},
};
use pleochroma_render_jobs::{JobKind, RenderJobFile};
use pleochroma_vault::db::sqlite::Database;
use slint::ComponentHandle as _;
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

/// The exposure the job froze is outside the Exposure slider's range.
const EXPOSURE_NOTE: &str = "The exposure of this job is outside the range of the Exposure slider, so the view shows the nearest value.";
/// The tray the job froze is none of the three trays.
const BACKDROP_NOTE: &str =
    "The tray of this job is not one of the usual trays, so the view shows the nearest one.";
/// The HDR map of the job changed or cannot be read.
const HDR_UNUSABLE_NOTE: &str = "The HDR map of this job changed since the job was added or could not be read. The view uses your current map.";
/// The HDR map of the job cannot be decoded for lack of a thread.
const HDR_NO_THREAD_NOTE: &str =
    "The HDR map of this job could not be loaded. The view uses your current map.";
/// The guide's current step keeps the view tabs locked, so a job's view cannot be shown.
const VIEW_LOCKED_BY_GUIDE: &str = "Load Job View is not available right now. Locked by the guide.";
/// How far a frozen tray radiance may be from a tray's own, relative, to count as that tray.
const BACKDROP_TOLERANCE: f32 = 1e-4;
/// The exposure is shown to four decimals: the meter's division leaves only rounding noise.
const EXPOSURE_ROUNDING: f32 = 10_000.0;

/// What the Exposure control shows for a frozen exposure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ExposureShown {
    /// The control's value, inside the slider's range.
    pub value: f32,
    /// The job's value was outside the range and is shown clamped.
    pub clamped: bool,
}

/// The exact inverse of the meter: `frozen` divided by the rig's `factor` (1 for a UV lamp and for
/// an HDR map, which is unmetered), rounded to four decimals, inside the slider's range.
#[must_use]
pub(super) fn exposure_shown(frozen: f32, factor: f32) -> ExposureShown {
    let divided = if factor.is_finite() && factor > 0.0 {
        frozen / factor
    } else {
        frozen
    };
    let rounded = if divided.is_finite() {
        (divided * EXPOSURE_ROUNDING).round() / EXPOSURE_ROUNDING
    } else {
        1.0
    };
    let value = rounded.clamp(EXPOSURE_RANGE.0, EXPOSURE_RANGE.1);
    ExposureShown {
        value,
        clamped: rounded < EXPOSURE_RANGE.0 || rounded > EXPOSURE_RANGE.1,
    }
}

/// The tray choice a frozen tray radiance stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BackdropShown {
    /// The tray the view shows.
    pub choice: Backdrop,
    /// The radiance is that tray's own (not a literal value that only came near it).
    pub exact: bool,
}

/// The tray whose radiance, under `preset` at the light pose, is nearest to the frozen `radiance`.
///
/// `0.0` is As Lit, which is a real choice; any other value picks the nearest of Grey, White and
/// Black, never As Lit. A value that is not a number shows White, flagged as not exact.
#[must_use]
pub(super) fn backdrop_shown(
    radiance: f32,
    preset: LightingPreset,
    light_yaw: f32,
    light_pitch: f32,
) -> BackdropShown {
    if radiance.is_finite() && radiance <= 0.0 {
        return BackdropShown {
            choice: Backdrop::AsLit,
            exact: true,
        };
    }
    if !radiance.is_finite() {
        return BackdropShown {
            choice: Backdrop::White,
            exact: false,
        };
    }
    let mut best = (Backdrop::White, f32::INFINITY);
    for choice in [Backdrop::Grey, Backdrop::White, Backdrop::Black] {
        let level = choice.resolve(preset, light_yaw, light_pitch);
        let distance = (level - radiance).abs();
        if distance < best.1 {
            best = (choice, distance);
        }
    }
    BackdropShown {
        choice: best.0,
        exact: best.1 <= radiance * BACKDROP_TOLERANCE,
    }
}

/// Everything a job view puts on the live view, worked out from the job file alone.
pub(super) struct JobLook {
    /// The lighting the controls show.
    pub lighting: LightingValues,
    /// The light pose as the scene froze it (radians), so the render context holds it exactly.
    pub light_pose: (f32, f32),
    /// The exposure the control shows, exactly as it goes into the context.
    pub exposure: f32,
    /// The HDR map's turn as the scene froze it (radians).
    pub hdr_yaw: f32,
    /// The job is lit by the studio rig (no HDR map).
    pub studio: bool,
    /// Camera yaw, pitch and roll (radians).
    pub camera: (f32, f32, f32),
    /// The stone's place in the picture.
    pub camera_pan: [f32; 2],
    /// The camera distance.
    pub distance: f32,
    /// The Stone Pose.
    pub stone_pose: StonePose,
    /// The resolved material, with the job's color zones.
    pub material: GemMaterial,
    /// The stone width that reproduces the material's absorption scale (`0.0` when it cannot be
    /// read back).
    pub stone_width_mm: f32,
    /// The facet edge rounding the material carries.
    pub edge_rounding: f32,
    /// Whether the girdle is frosted.
    pub girdle_frosted: bool,
    /// The bounce cap.
    pub max_bounces: u32,
    /// The engine and its options.
    pub engine: EngineSpec,
    /// What the engine pill of the live view shows.
    pub view_mode: i32,
    /// The banner's title.
    pub title: String,
    /// The banner's subtitle.
    pub subtitle: String,
    /// The banner's notes.
    pub notes: Vec<String>,
}

/// The banner's subtitle: the design and the material, and for a video what it shows.
#[must_use]
pub(super) fn subtitle_text(design: &str, material: &str, video: bool) -> String {
    let mut parts: Vec<&str> = [design.trim(), material.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    if video {
        parts.push("tilt video, shown face up");
    }
    parts.join(" \u{b7} ")
}

/// The lighting of a job's view.
struct LightingLook {
    values: LightingValues,
    exposure: f32,
    studio: bool,
    notes: Vec<String>,
}

/// The lighting job `job` froze, as the controls show it.
fn lighting_look(job: &RenderJobFile) -> LightingLook {
    let scene = &job.scene;
    let mut notes = Vec::new();
    let studio = scene.hdr().is_none();
    let preset = offered(scene.lighting_preset);
    // An HDR map is unmetered: its exposure is the control's value as it is.
    let factor = if studio {
        pleochroma::optics::raytracer::metered_exposure(
            1.0,
            scene.lighting_preset,
            scene.light_yaw,
            scene.light_pitch,
        )
    } else {
        1.0
    };
    let exposure = exposure_shown(scene.exposure, factor);
    if exposure.clamped {
        notes.push(EXPOSURE_NOTE.to_owned());
    }
    let tray = backdrop_shown(
        scene.backdrop,
        scene.lighting_preset,
        scene.light_yaw,
        scene.light_pitch,
    );
    if studio && !tray.exact {
        notes.push(BACKDROP_NOTE.to_owned());
    }
    let values = LightingValues {
        lighting_rig: preset.label().to_string(),
        light_yaw_deg: scene.light_yaw.to_degrees(),
        light_pitch_deg: scene.light_pitch.to_degrees(),
        exposure: exposure.value,
        surface_glare: scene.surface_glare,
        backdrop: tray.choice,
        head_shadow_deg: scene.head_shadow_deg,
        stone_shadow: scene.stone_shadow,
        tray_reflection: scene.tray_reflection,
        env_map_path: None,
        env_map_yaw_deg: scene.hdr_orientation.yaw.to_degrees(),
        env_map_follows_camera: scene.hdr_orientation.follows_camera,
        ..LightingValues::default()
    }
    .sanitised();
    LightingLook {
        values,
        exposure: exposure.value,
        studio,
        notes,
    }
}

/// Works out what job `id` shows on the live view.
#[must_use]
pub(super) fn look_from_job(job: &RenderJobFile, id: i64) -> JobLook {
    let scene = &job.scene;
    let LightingLook {
        values: lighting,
        exposure,
        studio,
        notes,
    } = lighting_look(job);
    let mut material = scene.material.clone();
    if material.zoning.is_none() {
        material.zoning.clone_from(&job.zoning);
    }
    let stone_width_mm = stone_width_for_scale(
        material.absorption_unit,
        material.absorption_path_scale,
        measure_model_width(&scene.planes),
    )
    .unwrap_or(0.0);
    let shown_material = if job.design.material.trim().is_empty() {
        material.name.as_str()
    } else {
        job.design.material.as_str()
    };
    let video = matches!(job.kind, JobKind::TiltVideo(_));
    let engine = EngineSpec {
        engine: scene.engine.engine,
        ray: scene.engine.ray.sanitized(),
    };
    JobLook {
        light_pose: (scene.light_yaw, scene.light_pitch),
        exposure,
        hdr_yaw: scene.hdr_orientation.yaw,
        studio,
        camera: (scene.yaw, scene.pitch, scene.camera_roll),
        camera_pan: scene.camera_pan,
        distance: scene.distance,
        stone_pose: StonePose::from_array(scene.stone_pose).clamped(),
        stone_width_mm,
        edge_rounding: material.edge_rounding_radius,
        girdle_frosted: scene.girdle_frosted,
        max_bounces: scene.max_bounces,
        view_mode: if engine.is_ray() {
            LiveViewMode::RayTraced.raw()
        } else {
            LiveViewMode::PathTraced.raw()
        },
        engine,
        title: format!("Showing the view of job {id}"),
        subtitle: subtitle_text(&job.design.title, shown_material, video),
        material,
        lighting,
        notes,
    }
}

/// Puts the job's stone, material, camera and render settings in the context. Returns whether the
/// claim of the stone was accepted (nothing is changed when it was not).
pub(super) fn apply_to_context(
    ctx: &mut RenderContext,
    id: i64,
    job: &RenderJobFile,
    look: &JobLook,
) -> bool {
    let scene = &job.scene;
    let stone = JobStone {
        planes: Arc::new(scene.planes.clone()),
        tools: Arc::new(scene.tools.clone()),
        placements: Vec::new(),
        frosted: scene.frosted_planes.clone(),
        effects: scene.effects.clone(),
        design: JobDesignNames {
            title: job.design.title.clone(),
            designer: job.design.designer.clone(),
            shape: job.design.shape.clone(),
            ri: job.design.ri.clone(),
        },
    };
    if !ctx.claim_job_geometry(id, stone) {
        return false;
    }
    // The material is the job's, whole: the overrides that were folded into it stay off, except the
    // ones whose application only sets a value (the stone size, which sets the absorption scale
    // from the width read back, and the edge rounding).
    ctx.material_name.clone_from(&look.material.name);
    ctx.material_override = Some(look.material.clone());
    ctx.material_unresolved = None;
    ctx.view_body_color = None;
    ctx.inclusion_sigma_s = 0.0;
    ctx.c_axis_override = None;
    ctx.alpha_axis_override = None;
    ctx.girdle_frosted = look.girdle_frosted;
    ctx.edge_rounding_radius = look.edge_rounding;
    ctx.stone_width_mm = look.stone_width_mm;
    (ctx.yaw, ctx.pitch, ctx.camera_roll) = look.camera;
    ctx.camera_pan = look.camera_pan;
    ctx.distance = look.distance;
    ctx.stone_pose = look.stone_pose;
    ctx.max_bounces = look.max_bounces;
    ctx.engine = look.engine;
    ctx.dirty = true;
    true
}

/// Puts the frozen light pose, exposure and HDR turn in the context exactly (the lighting
/// session's degrees would round them), and returns the owner for the viewport pill.
pub(super) fn apply_exact_light(ctx: &mut RenderContext, look: &JobLook) -> PlanesOwner {
    (ctx.light_yaw, ctx.light_pitch) = look.light_pose;
    ctx.exposure = look.exposure;
    ctx.hdr_yaw = look.hdr_yaw;
    ctx.dirty = true;
    ctx.planes_owner
}

/// Shows the controls of the job's view.
fn show_controls(ui: &MainWindow, look: &JobLook, material_name: &str) {
    let settings = ui.global::<SettingsModel>();
    settings.set_bounce_index(i32::try_from(bounce_index(look.max_bounces)).unwrap_or(2));
    settings.set_edge_rounding_radius(look.edge_rounding);
    settings.set_girdle_frosted(look.girdle_frosted);
    settings.set_stone_width_mm(look.stone_width_mm);
    if look.engine.is_ray() {
        settings.set_ray_taps(i32::from(look.engine.ray.aa_taps));
        settings.set_ray_fire(look.engine.ray.fire);
    }
    push_stone_pose_to_controls(ui, look.stone_pose, false);
    let viewport = ui.global::<ViewportModel>();
    let options = viewport.get_material_options();
    if let Some(index) = find_option_index(&options, material_name) {
        viewport.set_selected_material_index(index);
    }
    // Setting the mode runs the view mode's own handler, which switches the engine.
    viewport.set_live_view_mode(look.view_mode);
    viewport.invoke_render_color_refresh();
}

/// What reading the job's HDR map came to, worked out on a thread.
enum HdrAnswer {
    /// The map is the file the job was made with.
    Loaded {
        map: Arc<EnvironmentMap>,
        path: String,
    },
    /// The file is not there; `name` is its file name.
    Missing { name: String },
    /// The file changed or cannot be read or decoded.
    Unusable,
}

/// Checks and decodes the job's HDR map on a thread, and tells the view when it lands.
///
/// The map is shown only when the view of this job still shows; a missing or changed map becomes a
/// note in the banner, and the view keeps the map in use.
fn start_hdr(ui: &MainWindow, job: &RenderJobFile, id: i64, generation: u64) {
    let Some(source) = job.hdr.as_ref() else {
        return;
    };
    let base = Path::new(&job.output)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let path: Option<PathBuf> = job.hdr_path(&base);
    let name = Path::new(&source.path).file_name().map_or_else(
        || source.path.clone(),
        |name| name.to_string_lossy().into_owned(),
    );
    let job = job.clone();
    let weak = ui.as_weak();
    let spawned = thread::Builder::new()
        .name("job-view-hdr".to_owned())
        .spawn(move || {
            let missing = path.as_ref().is_some_and(|path| !path.exists());
            let loaded = catch_unwind(AssertUnwindSafe(|| convert::load_hdr(&job, &base)));
            let answer = match (loaded, path) {
                (Ok(Ok(Some(map))), Some(path)) => HdrAnswer::Loaded {
                    map,
                    path: path.to_string_lossy().into_owned(),
                },
                _ if missing => HdrAnswer::Missing { name },
                _ => HdrAnswer::Unusable,
            };
            let _ = weak.upgrade_in_event_loop(move |ui| hdr_landed(&ui, id, generation, answer));
        });
    if spawned.is_err() {
        session::add_note(ui, generation, id, HDR_NO_THREAD_NOTE);
    }
}

/// [`start_hdr`]'s answer, back on the UI thread. Dropped when the view it was asked for is over.
fn hdr_landed(ui: &MainWindow, id: i64, generation: u64, answer: HdrAnswer) {
    if !session::is_current(generation, id) {
        return;
    }
    match answer {
        HdrAnswer::Loaded { map, path } => {
            design_lighting::show_job_env_map(ui, map, &path);
        }
        HdrAnswer::Missing { name } => session::add_note(
            ui,
            generation,
            id,
            &format!(
                "The HDR map of this job is missing ({name}). The view uses your current map."
            ),
        ),
        HdrAnswer::Unusable => session::add_note(ui, generation, id, HDR_UNUSABLE_NOTE),
    }
}

/// Shows the view of job `id` on the live view; a second job's view replaces the first.
///
/// The editor document and the library selection are untouched, and nothing is persisted. A job
/// that cannot be read says so in a toast and changes nothing.
///
/// The Live Render view is shown once the job is read and the live view is ready, so a load
/// that fails leaves the cutter on the page they were on. While a guide step locks the view tabs the load
/// is refused, from the Jobs window's buttons as from the palette (`JOBS_LOAD_VIEW`).
pub(in crate::gui) fn load_job_view(ui: &MainWindow, id: i64) {
    if id <= 0 {
        return;
    }
    if !ui.global::<GuideModel>().invoke_allows_view_tabs() {
        show_toast(ui, VIEW_LOCKED_BY_GUIDE, "info");
        return;
    }
    let Some(render_ctx) = session::render_ctx() else {
        show_toast(ui, "The live view is not ready.", "error");
        return;
    };
    let job = match load_job_for_edit(ui, id) {
        Ok((_, job)) => job,
        Err(sentence) => {
            show_toast(ui, &sentence, "error");
            return;
        }
    };
    let look = look_from_job(&job, id);
    if !design_lighting::hold_for_job_view(ui) {
        show_toast(ui, "The live view is not ready.", "error");
        return;
    }
    // The job can be read and the live view is ready: show the Live Render, where the job's
    // view appears. Before `session::begin` below, which remembers the cutter's own look as the
    // Live Render shows it (the engine follows the view on screen), for Back to My View.
    commands::show_live_render_view(ui);
    let (generation, claimed) = {
        let mut ctx = RenderContext::lock(&render_ctx);
        let generation = session::begin(ui, &mut ctx, id);
        (generation, apply_to_context(&mut ctx, id, &job, &look))
    };
    if !claimed {
        session::end(ui, EndReason::BackToMyView);
        show_toast(ui, "The view of this job could not be shown.", "error");
        return;
    }
    design_lighting::show_job_lighting(ui, &look.lighting, look.studio);
    let owner = apply_exact_light(&mut RenderContext::lock(&render_ctx), &look);
    push_viewport_owner(ui, owner);
    show_controls(ui, &look, &look.material.name);
    session::publish_banner(ui, id, &look.title, &look.subtitle, &look.notes);
    let jobs = ui.global::<RenderJobsModel>();
    jobs.set_pick_view_mode(false);
    jobs.set_panel_open(false);
    start_hdr(ui, &job, id, generation);
}

/// Registers `RenderJobsModel.load_job_view` and `back_to_my_view`, and hands the view the render
/// context and the vault it reads jobs from.
pub(in crate::gui) fn setup(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    _settings_store: &Arc<SettingsPersister>,
) {
    install_db(db);
    session::install(render_ctx);
    let rows = ui.global::<RenderJobsModel>();
    let weak = ui.as_weak();
    rows.on_load_job_view(move |id| {
        if let Some(ui) = weak.upgrade() {
            load_job_view(&ui, i64::from(id));
        }
    });
    let weak = ui.as_weak();
    rows.on_back_to_my_view(move || {
        if let Some(ui) = weak.upgrade() {
            session::end(&ui, EndReason::BackToMyView);
        }
    });
}
