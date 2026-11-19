//! The material check set: twelve queued stills of the open design that a cutter judges by eye.
//!
//! The design's own material is shot four times: table up and tilted 10 degrees (through the Stone
//! Pose), each in the Light Tent and in Daylight. Every one of those four stills is a normal queued
//! still, frozen by the same job builder as "Add to Queue" in the export dialog, with the size,
//! samples, bounces, color space and compute the dialog remembers. Quartz and Cubic Zirconia are
//! then made from each of the four by Copy to Materials, so the set is 12 jobs, or 8 when the
//! design's own material is one of the two. There is no second copy mechanism, and nothing is
//! written to the export defaults.

use super::{
    controller::unix_now,
    convert::{StillJobInputs, build_still_job},
    edit::{
        copy::copy_job_to_materials,
        naming::{JobKindTag, name_from_template, reserve_output},
        save::insert_job_after,
    },
};
use crate::{
    ExportModel, MainWindow, OpticalModel, RiChartModel,
    bridge::{
        export_thread::{self, ExportParams, RemoteSelection, SceneSnapshot},
        render_thread::RenderContext,
    },
    gui::{
        optics::offered_lighting::offered_from_label,
        render::{
            output_defaults::{
                seed_still_dialog, still_adaptive, still_beam, still_engine, still_path_cache,
            },
            render_export::{
                queue::{apply_export_bounce_cap, compute_target_from_index},
                wiring::{design_naming, resolve_export_dir_then},
            },
        },
        show_toast,
    },
    settings::{
        ExportTransfer, LightingPreset as SavedLightingPreset, SettingsPersister,
        model::{PITCH_RANGE_RAD, built_in_presets},
    },
};
use pleochroma::{
    color::{ColorSpace, metrics::PROFILE_AZIMUTHS_DEG},
    render_setup::StonePose,
    renderer::{
        accel::PathCacheSettings, accel_beam::BeamSettings, adaptive::AdaptiveSettings,
        env_map::HdrOrientation,
    },
};
use pleochroma_render_jobs::{RenderJobFile, job::DesignInfo};
use slint::ComponentHandle as _;
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// The materials the design is also shot in; its own is shot first.
pub(super) const EXTRA_MATERIALS: [&str; 2] = ["Quartz", "Cubic Zirconia"];

/// How far the tilted pose leans the stone, in degrees.
const TILT_DEG: f32 = 10.0;

/// The camera pitch that looks straight down at the table (the Grading tray's face-up view).
const FACE_UP_PITCH: f32 = 1.48;

/// What the toast says when the set cannot be queued for lack of a design.
const NO_DESIGN: &str = "Open a design to queue its material check set.";

/// One still of the design's own material: a pose and a lighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Shot {
    /// Whether the stone leans 10 degrees; otherwise the table is up.
    pub(super) tilted: bool,
    /// The built-in lighting preset that lights it.
    pub(super) preset: &'static str,
    /// The lighting as the label names it.
    pub(super) light: &'static str,
}

/// The four stills: table up then tilted, each in the Light Tent then in Daylight.
pub(super) const SHOTS: [Shot; 4] = [
    Shot {
        tilted: false,
        preset: "Light Tent",
        light: "Light Tent",
    },
    Shot {
        tilted: false,
        preset: "Daylight Bright",
        light: "Daylight",
    },
    Shot {
        tilted: true,
        preset: "Light Tent",
        light: "Light Tent",
    },
    Shot {
        tilted: true,
        preset: "Daylight Bright",
        light: "Daylight",
    },
];

impl Shot {
    /// The pose as the label names it.
    pub(super) const fn pose_word(self) -> &'static str {
        if self.tilted {
            "tilted 10\u{b0}"
        } else {
            "table up"
        }
    }

    /// The Stone Pose of the shot: the table up, or leaned 10 degrees on the first profile axis.
    pub(super) const fn pose(self) -> StonePose {
        if self.tilted {
            StonePose {
                tilt_deg: TILT_DEG,
                turn_deg: PROFILE_AZIMUTHS_DEG[0],
                spin_deg: 0.0,
            }
        } else {
            StonePose::IDENTITY
        }
    }
}

/// The label of a check job: `<design> · check · <pose> · <light> · <material>`.
///
/// The material comes last so that Copy to Materials replaces it, and does not append a second one.
pub(super) fn check_label(design: &str, shot: Shot, material: &str) -> String {
    format!(
        "{design} \u{b7} check \u{b7} {} \u{b7} {} \u{b7} {material}",
        shot.pose_word(),
        shot.light
    )
}

/// The materials the copies are made in: the extra materials, without the design's own.
pub(super) fn extra_materials(own: &str) -> Vec<String> {
    EXTRA_MATERIALS
        .iter()
        .filter(|name| !name.eq_ignore_ascii_case(own))
        .map(|name| (*name).to_string())
        .collect()
}

/// How many jobs the set queues for a design whose material is `own`: 12, or 8 when `own` is one
/// of the extra materials.
pub(super) fn job_count(own: &str) -> usize {
    SHOTS.len() * (1 + extra_materials(own).len())
}

/// What the set needs from the app, kept by [`setup`].
#[derive(Clone)]
struct Handles {
    render_ctx: Arc<Mutex<RenderContext>>,
    settings_store: Arc<SettingsPersister>,
}

thread_local! {
    /// The shared handles (set by [`setup`]; the UI thread only).
    static HANDLES: RefCell<Option<Handles>> = const { RefCell::new(None) };
    /// Whether a set is being queued: the folder picker or the finished stone is still pending.
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

/// The remembered export choices, as one still of the queue takes them.
struct Request {
    params: ExportParams,
    adaptive: Option<AdaptiveSettings>,
    path_cache: Option<PathCacheSettings>,
    beam: Option<BeamSettings>,
    color_space: ColorSpace,
    remote: RemoteSelection,
    export_dir: PathBuf,
}

/// The design, designer, shape and RI that every job of the set names.
struct Names {
    design: String,
    designer: String,
    shape: String,
    ri: String,
}

/// Registers `RiChartModel.queue_check_set`.
pub(in crate::gui) fn setup(
    ui: &MainWindow,
    _db: &Arc<Mutex<pleochroma_vault::db::sqlite::Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    HANDLES.with(|slot| {
        *slot.borrow_mut() = Some(Handles {
            render_ctx: Arc::clone(render_ctx),
            settings_store: Arc::clone(settings_store),
        });
    });
    let weak = ui.as_weak();
    ui.global::<RiChartModel>().on_queue_check_set(move || {
        if let Some(ui) = weak.upgrade()
            && let Err(message) = queue_check_set(&ui)
        {
            show_toast(&ui, &message, "info");
        }
    });
}

/// Queues the material check set of the open design and returns how many jobs it makes.
///
/// The set is built once the finished stone and the export folder are known, which can take a moment
/// (a solve, or the folder picker); the toast "12 check renders queued." says when it is done.
///
/// # Errors
///
/// A sentence when no design is open, a set is already being queued, or the remembered export choices
/// are not valid.
pub(in crate::gui) fn queue_check_set(ui: &MainWindow) -> Result<usize, String> {
    let handles = HANDLES
        .with(|slot| slot.borrow().clone())
        .ok_or_else(|| "The render queue is not available.".to_string())?;
    if !ui.global::<RiChartModel>().get_can_queue_check_set() {
        return Err(NO_DESIGN.to_string());
    }
    if BUSY.with(Cell::get) {
        return Err("The check set is already being queued.".to_string());
    }
    let own = RenderContext::lock(&handles.render_ctx)
        .material_name
        .clone();
    let expected = job_count(&own);
    let request = remembered(ui, &handles.settings_store)?;
    BUSY.with(|busy| busy.set(true));
    let store = Arc::clone(&handles.settings_store);
    resolve_export_dir_then(ui, &store, "Nothing was added.", move |ui, dir| {
        let Some(export_dir) = dir else {
            BUSY.with(|busy| busy.set(false));
            return;
        };
        let request = Request {
            export_dir,
            ..request
        };
        let render_ctx = Arc::clone(&handles.render_ctx);
        crate::gui::editor::finished_stone_then(
            ui,
            &render_ctx,
            |_| {},
            move |ui, finished| {
                let result = match finished {
                    Ok(finished) => queue_all(ui, &handles, &request, finished.as_ref()),
                    Err(withheld) => Err(withheld.export_message().replacen(
                        "Nothing was exported",
                        "Nothing was added",
                        1,
                    )),
                };
                BUSY.with(|busy| busy.set(false));
                match result {
                    Ok(count) => show_toast(ui, &format!("{count} check renders queued."), "info"),
                    Err(message) => show_toast(ui, &message, "info"),
                }
            },
        );
    });
    Ok(expected)
}

/// The export dialog's remembered choices, with the export folder still empty.
///
/// The dialog's controls take the remembered values, as they do each time the dialog opens, and the
/// settings themselves are not written.
fn remembered(ui: &MainWindow, store: &SettingsPersister) -> Result<Request, String> {
    let snapshot = store.snapshot();
    seed_still_dialog(ui, &snapshot.settings);
    let still = snapshot.settings.output.clone().sanitised().still;
    let width = match still.size_preset {
        0 => 1920,
        1 => 3840,
        _ => i32::try_from(still.custom_width).unwrap_or(i32::MAX),
    };
    let height = match still.size_preset {
        0 => 1080,
        1 => 2160,
        _ => i32::try_from(still.custom_height).unwrap_or(i32::MAX),
    };
    let engine = still_engine(ui);
    let samples = if engine.is_ray() {
        i32::from(engine.ray.sanitized().aa_taps)
    } else {
        1_i32 << still.sample_exponent.min(30)
    };
    let bounces = i32::try_from(still.bounces).unwrap_or(i32::MAX);
    let mut params = export_thread::validate_export_params(width, height, samples, bounces)?;
    params.engine = engine;
    params.samples_per_pixel = engine.target_samples(params.samples_per_pixel);
    let remote = RemoteSelection {
        compute_target: if crate::bridge::render_thread::remote_allowed(engine) {
            compute_target_from_index(still.compute.index())
        } else {
            export_thread::ComputeTarget::LocalOnly
        },
        worker: snapshot.settings.remote_worker(),
        transfer: ExportTransfer::from_index(ui.global::<ExportModel>().get_transfer_index()),
        contribute_local: snapshot.settings.contribute_to_final_picture,
    };
    Ok(Request {
        adaptive: still_adaptive(ui, engine, params.samples_per_pixel),
        path_cache: still_path_cache(ui, engine),
        beam: still_beam(ui, engine),
        color_space: crate::gui::color_space_from_index(still.color_space_index),
        params,
        remote,
        export_dir: PathBuf::new(),
    })
}

/// Queues the four stills and their copies, in this order: a still, then its copies, and so on.
/// Returns how many jobs were queued.
fn queue_all(
    ui: &MainWindow,
    handles: &Handles,
    request: &Request,
    finished: Option<&pleochroma_solid::preview::StoneGeometryBuf>,
) -> Result<usize, String> {
    let mut base = SceneSnapshot::capture_finished_for_engine(
        &handles.render_ctx,
        finished,
        request.params.engine,
    )
    .map(|scene| apply_export_bounce_cap(scene, request.params.max_bounces))?;
    base.engine = request.params.engine;
    if !base.engine.is_ray() && ui.global::<OpticalModel>().get_include_still() {
        base.optical_diagnostics = Some(crate::bridge::optical_overlay::presentation(ui));
    }
    let own = base.material.name.clone();
    let extras = extra_materials(&own);
    let total = SHOTS.len() * (1 + extras.len());
    let (design, designer, shape, ri) = design_naming(ui, &handles.render_ctx);
    let names = Names {
        design,
        designer,
        shape,
        ri,
    };
    let mut last = 0_i64;
    let mut queued = 0_usize;
    for shot in SHOTS {
        let scene = shot_scene(&base, shot)?;
        if let Some(reason) = crate::bridge::optical_overlay::export::refusal(&scene) {
            return Err(incomplete(reason, queued, total));
        }
        let job = base_job(ui, request, &names, shot, &scene)
            .map_err(|message| incomplete(&message, queued, total))?;
        let id = insert_job_after(ui, last, job)
            .map_err(|message| incomplete(&message, queued, total))?;
        queued += 1;
        last = id;
        if !extras.is_empty() {
            let made = copy_job_to_materials(ui, id, &extras)
                .map_err(|message| incomplete(&message, queued, total))?;
            queued += made.len();
            last = made.last().copied().unwrap_or(id);
        }
    }
    Ok(queued)
}

/// The sentence for a set that stopped part way: `message` and how many jobs are queued.
fn incomplete(message: &str, queued: usize, total: usize) -> String {
    format!("{message} ({queued} of {total} check renders were queued.)")
}

/// The scene of `shot`: `base` lit by the shot's preset, the stone posed, the camera looking down.
fn shot_scene(base: &SceneSnapshot, shot: Shot) -> Result<SceneSnapshot, String> {
    let preset = built_in_presets()
        .into_iter()
        .find(|preset| preset.name == shot.preset)
        .ok_or_else(|| format!("The lighting \"{}\" is not available.", shot.preset))?;
    let mut scene = lit(base.clone(), &preset);
    scene.stone_pose = shot.pose();
    scene.yaw = 0.0;
    scene.pitch = FACE_UP_PITCH;
    scene.camera_roll = 0.0;
    scene.camera_pan = [0.0, 0.0];
    Ok(scene)
}

/// `scene` with the lighting of `preset`: the rig, its light, exposure, backdrop, glare, head shadow,
/// Stone Shadow and Tray Reflection, and no HDR map.
///
/// The same overlay the export fan-out applies for a preset, without its camera, map and product
/// shot parts: the check set sets the camera and the pose itself.
fn lit(mut scene: SceneSnapshot, preset: &SavedLightingPreset) -> SceneSnapshot {
    let values = preset.values().sanitised();
    scene.light_yaw = values.light_yaw_deg.to_radians();
    scene.light_pitch = values
        .light_pitch_deg
        .to_radians()
        .clamp(PITCH_RANGE_RAD.0, PITCH_RANGE_RAD.1);
    scene.lighting_preset = offered_from_label(&values.lighting_rig);
    scene.surface_glare = values.surface_glare;
    scene.backdrop =
        values
            .backdrop
            .resolve(scene.lighting_preset, scene.light_yaw, scene.light_pitch);
    scene.head_shadow_deg = values.head_shadow_deg;
    scene.stone_shadow = values.stone_shadow;
    scene.tray_reflection = values.tray_reflection;
    if scene.stone_closed {
        scene.stone_closed = SceneSnapshot::stone_closed_for(
            scene.stone_shadow,
            scene.tray_reflection,
            &scene.active_planes,
            &scene.tools,
        );
    }
    scene.env_map = None;
    scene.hdr_orientation = HdrOrientation::default();
    scene.exposure = pleochroma::optics::raytracer::metered_exposure(
        values.exposure,
        scene.lighting_preset,
        scene.light_yaw,
        scene.light_pitch,
    );
    scene
}

/// The still job of `shot` in the design's own material, named from the export template.
fn base_job(
    ui: &MainWindow,
    request: &Request,
    names: &Names,
    shot: Shot,
    scene: &SceneSnapshot,
) -> Result<RenderJobFile, String> {
    let placeholder = request.export_dir.join("check.png");
    let mut job = build_still_job(
        &StillJobInputs {
            scene,
            params: request.params,
            color_space: request.color_space,
            remote: &request.remote,
            output: &placeholder,
            label: check_label(&names.design, shot, &scene.material.name),
            design: DesignInfo {
                title: names.design.clone(),
                designer: names.designer.clone(),
                shape: names.shape.clone(),
                ri: names.ri.clone(),
                material: scene.material.name.clone(),
            },
            preset_label: shot.preset.to_string(),
            adaptive: request.adaptive,
            path_cache: request.path_cache,
            beam: request.beam,
        },
        unix_now(),
    )?;
    let file_name = name_from_template(ui, &job)?;
    let wanted: &Path = &request.export_dir.join(file_name);
    job.output = reserve_output(ui, wanted, JobKindTag::Still)
        .to_string_lossy()
        .into_owned();
    Ok(job)
}
