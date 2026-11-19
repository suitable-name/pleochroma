//! Seeding the edit dialog from a job, and reading its controls back into a `JobEdit`.
//!
//! The dialog's values are plain data ([`Controls`]) so the mapping is tested without a window.
//! A value that is not on the dialog's ladder (samples or bounces from the command line) shows
//! as "from job" and is written back only when the person moves the control.

use crate::{
    JobEditModel, MainWindow,
    gui::tilt::video_export::params::resolve_fps,
    settings::model::{
        BOUNCE_LADDER, EXPORT_SAMPLES_EXPONENT_RANGE, RemoteAvailability, bounce_index,
        clamp_exponent, nearest_bounce_cap,
    },
};
use pleochroma::renderer::engine::AA_TAP_CHOICES;
use pleochroma_render_jobs::{
    ComputeChoice, JobColorSpace, JobKind, RenderJobFile, TransferChoice,
    edit::JobEdit,
    job::{MetricCurvesData, OverlaySelection},
    state::JobState,
};
use pleochroma_vault::db::sqlite::ExpectedRow;
use slint::{ComponentHandle as _, ModelRc, SharedString, VecModel};
use std::path::{Path, PathBuf};

/// The sentence under the size when a product-shot job changes its aspect.
const ASPECT_NOTE: &str =
    "The stone was framed for the old aspect. Load Job View to frame it again.";

/// The fixed hint under the renderer line.
const RENDERER_HINT: &str =
    "To change it: Load Job View, switch the renderer, then Update Job from This View.";

/// The dialog's editable values.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Controls {
    pub(super) compute_index: i32,
    pub(super) transfer_index: i32,
    pub(super) size_preset: i32,
    pub(super) custom_width: i32,
    pub(super) custom_height: i32,
    pub(super) sample_exponent: i32,
    pub(super) bounce_index: i32,
    pub(super) color_space_index: i32,
    pub(super) ray_taps_index: i32,
    pub(super) ray_fire: bool,
    pub(super) output_folder: String,
    pub(super) file_name: String,
    pub(super) label: String,
    pub(super) material_index: i32,
    pub(super) fps_index: i32,
    pub(super) keep_frames: bool,
    pub(super) video_name: String,
    pub(super) start_deg: i32,
    pub(super) end_deg: i32,
    pub(super) step_deg: f32,
    pub(super) axes: [bool; 4],
    pub(super) overlay: [bool; 5],
}

/// The tilt axes a curve measurement was made for, and the material it used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CurveKey {
    pub(super) axes: Vec<u32>,
    pub(super) material: Option<String>,
}

/// Measured curves with what they were measured for.
#[derive(Debug, Clone)]
pub(super) struct MeasuredCurves {
    pub(super) key: CurveKey,
    pub(super) sets: Vec<MetricCurvesData>,
}

/// Everything the open dialog remembers about the job it edits.
pub(super) struct Seed {
    pub(super) id: i64,
    /// The state and finished frames the dialog was seeded with.
    pub(super) expected: ExpectedRow,
    pub(super) state: JobState,
    /// The job as it was loaded.
    pub(super) job: RenderJobFile,
    /// The dialog's values at seeding.
    pub(super) controls: Controls,
    /// The material list the combo shows.
    pub(super) materials: Vec<String>,
    /// The material index the pending change was resolved for.
    pub(super) material_index: i32,
    /// The material fields of a chosen material, as the resolver made them.
    pub(super) material_edit: Option<JobEdit>,
    /// The line under the material row.
    pub(super) material_note: String,
    /// The place of a located HDR map.
    pub(super) hdr_path: Option<PathBuf>,
    /// The curves measured for the current edit.
    pub(super) curves: Option<MeasuredCurves>,
    /// The measurement in progress.
    pub(super) measuring: Option<CurveKey>,
    /// The measurement that failed, so it is not started again at once.
    pub(super) measure_failed: Option<CurveKey>,
    /// The chosen material could not be resolved: Save stays off.
    pub(super) material_blocked: bool,
    /// A sentence about the last action (a refused HDR file, a failed material).
    pub(super) message: String,
    /// Set when the row changed under the dialog: the sentence shown in place of saving.
    pub(super) refused: Option<String>,
}

impl Seed {
    /// The seed of job `id` as read from the list.
    pub(super) fn new(
        id: i64,
        expected: ExpectedRow,
        state: JobState,
        job: RenderJobFile,
        materials: Vec<String>,
    ) -> Self {
        let controls = controls_of(&job, &materials);
        Self {
            id,
            expected,
            state,
            job,
            material_index: controls.material_index,
            controls,
            materials,
            material_edit: None,
            material_note: String::new(),
            hdr_path: None,
            curves: None,
            measuring: None,
            measure_failed: None,
            material_blocked: false,
            message: String::new(),
            refused: None,
        }
    }

    /// Whether the job is a tilt video.
    pub(super) const fn is_video(&self) -> bool {
        matches!(self.job.kind, JobKind::TiltVideo(_))
    }
}

fn to_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// The slider exponent nearest to `samples`, inside the export range.
fn exponent_for(samples: u32) -> u32 {
    let samples = samples.max(1);
    let low = 31 - samples.leading_zeros();
    let low_value = 1_u64 << low;
    let nearest = if u64::from(samples) - low_value <= low_value * 2 - u64::from(samples) {
        low
    } else {
        low + 1
    };
    clamp_exponent(nearest, EXPORT_SAMPLES_EXPONENT_RANGE)
}

fn samples_of(job: &RenderJobFile) -> u32 {
    match &job.kind {
        JobKind::Still(still) => still.samples_per_pixel,
        JobKind::TiltVideo(video) => video.samples_per_pixel,
    }
}

fn space_of(job: &RenderJobFile) -> JobColorSpace {
    match &job.kind {
        JobKind::Still(still) => still.color_space,
        JobKind::TiltVideo(video) => video.color_space,
    }
}

/// Whether the job's samples sit on the slider (a power of two inside the export range).
fn samples_on_ladder(samples: u32) -> bool {
    samples.is_power_of_two()
        && (1_u32 << EXPORT_SAMPLES_EXPONENT_RANGE.0..=1_u32 << EXPORT_SAMPLES_EXPONENT_RANGE.1)
            .contains(&samples)
}

/// The dialog's values for `job`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "angles are within +-90 degrees and steps are small; the dialog holds whole degrees"
)]
pub(super) fn controls_of(job: &RenderJobFile, materials: &[String]) -> Controls {
    let (width, height) = (job.scene.width, job.scene.height);
    let size_preset = match (width, height) {
        (1920, 1080) => 0,
        (3840, 2160) => 1,
        _ => 2,
    };
    let output = Path::new(&job.output);
    let folder = output
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default();
    let own_name = output
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut controls = Controls {
        compute_index: match job.compute.target {
            ComputeChoice::Local => 0,
            ComputeChoice::Remote => 1,
            ComputeChoice::Both => 2,
        },
        transfer_index: i32::from(job.compute.transfer == TransferChoice::FinalPicture),
        size_preset,
        custom_width: to_i32(width),
        custom_height: to_i32(height),
        sample_exponent: to_i32(exponent_for(samples_of(job))),
        bounce_index: i32::try_from(bounce_index(job.scene.max_bounces)).unwrap_or(0),
        color_space_index: match space_of(job) {
            JobColorSpace::Srgb => 0,
            JobColorSpace::DisplayP3 => 1,
            JobColorSpace::Rec2020 => 2,
        },
        ray_taps_index: AA_TAP_CHOICES
            .iter()
            .position(|taps| *taps == job.engine.ray.aa_taps)
            .and_then(|at| i32::try_from(at).ok())
            .unwrap_or(0),
        ray_fire: job.engine.ray.fire,
        output_folder: folder,
        file_name: own_name.clone(),
        label: job.label.clone(),
        material_index: materials
            .iter()
            .position(|name| *name == job.scene.material.name)
            .and_then(|at| i32::try_from(at).ok())
            .unwrap_or(0),
        fps_index: 1,
        keep_frames: false,
        video_name: String::new(),
        start_deg: 0,
        end_deg: 0,
        step_deg: 1.0,
        axes: [false; 4],
        overlay: [false; 5],
    };
    if let JobKind::TiltVideo(video) = &job.kind {
        controls.fps_index = match video.fps {
            24 => 0,
            60 => 2,
            _ => 1,
        };
        controls.keep_frames = video.keep_frames;
        controls.video_name.clone_from(&video.video_name);
        controls.start_deg = video.start_deg.round() as i32;
        controls.end_deg = video.end_deg.round() as i32;
        controls.step_deg = video.step_deg as f32;
        for axis in video.axes() {
            if let Some(slot) = controls.axes.get_mut(axis as usize) {
                *slot = true;
            }
        }
        controls.overlay = [
            video.overlay.brilliance,
            video.overlay.windowing,
            video.overlay.extinction,
            video.overlay.tilt_brilliance,
            video.overlay.angle,
        ];
    }
    controls
}

/// The dialog's current values.
pub(super) fn read_controls(ui: &MainWindow) -> Controls {
    let m = ui.global::<JobEditModel>();
    Controls {
        compute_index: m.get_compute_index(),
        transfer_index: m.get_transfer_index(),
        size_preset: m.get_size_preset(),
        custom_width: m.get_custom_width(),
        custom_height: m.get_custom_height(),
        sample_exponent: m.get_sample_exponent(),
        bounce_index: m.get_bounce_index(),
        color_space_index: m.get_color_space_index(),
        ray_taps_index: m.get_ray_taps_index(),
        ray_fire: m.get_ray_fire(),
        output_folder: m.get_output_folder().to_string(),
        file_name: m.get_file_name().to_string(),
        label: m.get_label().to_string(),
        material_index: m.get_material_index(),
        fps_index: m.get_fps_index(),
        keep_frames: m.get_keep_frames(),
        video_name: m.get_video_name().to_string(),
        start_deg: m.get_start_deg(),
        end_deg: m.get_end_deg(),
        step_deg: m.get_step_deg(),
        axes: [
            m.get_axis_0(),
            m.get_axis_1(),
            m.get_axis_2(),
            m.get_axis_3(),
        ],
        overlay: [
            m.get_overlay_brilliance(),
            m.get_overlay_windowing(),
            m.get_overlay_extinction(),
            m.get_overlay_tilt_brilliance(),
            m.get_overlay_angle(),
        ],
    }
}

/// Writes `c` into the dialog's controls.
pub(super) fn write_controls(ui: &MainWindow, c: &Controls) {
    let m = ui.global::<JobEditModel>();
    m.set_compute_index(c.compute_index);
    m.set_transfer_index(c.transfer_index);
    m.set_size_preset(c.size_preset);
    m.set_custom_width(c.custom_width);
    m.set_custom_height(c.custom_height);
    m.set_sample_exponent(c.sample_exponent);
    m.set_bounce_index(c.bounce_index);
    m.set_color_space_index(c.color_space_index);
    m.set_ray_taps_index(c.ray_taps_index);
    m.set_ray_fire(c.ray_fire);
    m.set_output_folder(c.output_folder.as_str().into());
    m.set_file_name(c.file_name.as_str().into());
    m.set_label(c.label.as_str().into());
    m.set_material_index(c.material_index);
    m.set_fps_index(c.fps_index);
    m.set_keep_frames(c.keep_frames);
    m.set_video_name(c.video_name.as_str().into());
    m.set_start_deg(c.start_deg);
    m.set_end_deg(c.end_deg);
    // The step field may keep stale text after a re-seed: write a different value first.
    m.set_step_deg(c.step_deg + 1.0);
    m.set_step_deg(c.step_deg);
    m.set_axis_0(c.axes[0]);
    m.set_axis_1(c.axes[1]);
    m.set_axis_2(c.axes[2]);
    m.set_axis_3(c.axes[3]);
    m.set_overlay_brilliance(c.overlay[0]);
    m.set_overlay_windowing(c.overlay[1]);
    m.set_overlay_extinction(c.overlay[2]);
    m.set_overlay_tilt_brilliance(c.overlay[3]);
    m.set_overlay_angle(c.overlay[4]);
}

/// Publishes what does not change while the dialog is open: the frame, the material list and
/// the renderer lines.
pub(super) fn publish_static(ui: &MainWindow, seed: &Seed) {
    let m = ui.global::<JobEditModel>();
    m.set_job_id(i32::try_from(seed.id).unwrap_or(i32::MAX));
    m.set_title(format!("Job {}", seed.id).into());
    m.set_is_video(seed.is_video());
    m.set_is_ray(seed.job.engine.is_ray());
    m.set_state_word(seed.state.label().into());
    let names: Vec<SharedString> = seed.materials.iter().map(|n| n.as_str().into()).collect();
    m.set_materials(ModelRc::new(VecModel::from(names)));
    let family = if seed.job.engine.is_ray() {
        "Ray-Traced"
    } else {
        "Path-Traced"
    };
    m.set_renderer_text(format!("Renderer {family} \u{b7} fixed for this job").into());
    m.set_renderer_hint(RENDERER_HINT.into());
    m.set_message("".into());
    m.set_refused(false);
    m.set_measuring_curves(false);
    m.set_requeue_question_open(false);
}

/// The place Save writes the job to, as the dialog shows it.
pub(super) fn destination(seed: &Seed, now: &Controls) -> String {
    if seed.is_video() {
        Path::new(&now.output_folder)
            .join(now.video_name.trim())
            .to_string_lossy()
            .into_owned()
    } else {
        Path::new(&now.output_folder)
            .join(now.file_name.trim())
            .to_string_lossy()
            .into_owned()
    }
}

/// Publishes the notes that follow the controls: the remote note, the aspect note, the "from
/// job" values, the HDR line, the material note and the frames line.
pub(super) fn publish_notes(
    ui: &MainWindow,
    seed: &Seed,
    now: &Controls,
    availability: RemoteAvailability,
    total_frames: u32,
) {
    let m = ui.global::<JobEditModel>();
    m.set_remote_configured(availability.configured);
    let note = if seed.job.engine.is_ray() {
        "Ray-Traced jobs render on this computer only."
    } else if now.compute_index != 0 && !availability.configured {
        "No remote is set up in Settings. The job uses it once one is."
    } else if now.compute_index != 0 && !availability.reachable_render_capable {
        "The remote is not reachable now; the job uses it when it runs"
    } else {
        ""
    };
    m.set_remote_note(note.into());

    let (w, h) = (
        i64::from(seed.job.scene.width),
        i64::from(seed.job.scene.height),
    );
    let sized = i64::from(now.custom_width.max(1));
    let sized_h = i64::from(now.custom_height.max(1));
    let (nw, nh) = match now.size_preset {
        0 => (1920, 1080),
        1 => (3840, 2160),
        _ => (sized, sized_h),
    };
    let framed = matches!(&seed.job.kind, JobKind::Still(still) if !still.preset_label.is_empty());
    m.set_aspect_note(if framed && w * nh != h * nw {
        ASPECT_NOTE.into()
    } else {
        "".into()
    });

    let samples = samples_of(&seed.job);
    let samples_from_job =
        !samples_on_ladder(samples) && now.sample_exponent == seed.controls.sample_exponent;
    m.set_samples_from_job(samples_from_job);
    m.set_samples_from_job_text(format!("{samples} samples (from job)").into());
    let bounces = seed.job.scene.max_bounces;
    let bounces_from_job =
        !BOUNCE_LADDER.contains(&bounces) && now.bounce_index == seed.controls.bounce_index;
    m.set_bounces_from_job(bounces_from_job);
    m.set_bounces_from_job_text(format!("{bounces} bounces (from job)").into());

    let hdr_name = seed.job.hdr.as_ref().and_then(|hdr| {
        Path::new(&hdr.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    });
    let base = Path::new(&seed.job.output)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let missing =
        seed.hdr_path.is_none() && seed.job.hdr_path(&base).is_some_and(|path| !path.exists());
    m.set_hdr_missing(missing);
    m.set_hdr_text(match (&hdr_name, missing, &seed.hdr_path) {
        (Some(name), true, _) => format!("The HDR map {name} is missing.").into(),
        (Some(_), false, Some(path)) => format!(
            "HDR map: {}",
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        )
        .into(),
        (Some(name), false, None) => format!("HDR map: {name}").into(),
        (None, _, _) => "".into(),
    });
    m.set_material_note(seed.material_note.as_str().into());
    m.set_frames_text(if seed.is_video() {
        format!("{total_frames} frames in all").into()
    } else {
        "".into()
    });
    m.set_saves_to(destination(seed, now).into());
}

fn plain_name(name: &str, what: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        Err(format!("The {what} is empty."))
    } else if name.contains(['/', '\\']) {
        Err(format!("The {what} cannot contain a folder."))
    } else {
        Ok(name.to_string())
    }
}

fn size_of(c: &Controls, is_video: bool) -> Result<(u32, u32), String> {
    let (w, h) = match c.size_preset {
        0 => (1920, 1080),
        1 => (3840, 2160),
        _ => (
            u32::try_from(c.custom_width).map_err(|_| "The size must be a positive number.")?,
            u32::try_from(c.custom_height).map_err(|_| "The size must be a positive number.")?,
        ),
    };
    // A video needs an even size.
    Ok(if is_video { (w & !1, h & !1) } else { (w, h) })
}

/// The axes in play order after the checkboxes changed: the kept ones in their old order, then
/// the new ones in ascending order.
fn axes_of(now: &Controls, job: &RenderJobFile) -> Vec<u32> {
    let old: Vec<u32> = match &job.kind {
        JobKind::TiltVideo(video) => video.axes(),
        JobKind::Still(_) => Vec::new(),
    };
    let on = |axis: u32| now.axes.get(axis as usize).copied().unwrap_or(false);
    let mut axes: Vec<u32> = old.iter().copied().filter(|axis| on(*axis)).collect();
    for axis in 0..4_u32 {
        if on(axis) && !axes.contains(&axis) {
            axes.push(axis);
        }
    }
    axes
}

/// The axes the video has after the edit.
pub(super) fn effective_axes(seed: &Seed, now: &Controls) -> Vec<u32> {
    if now.axes == seed.controls.axes {
        match &seed.job.kind {
            JobKind::TiltVideo(video) => video.axes(),
            JobKind::Still(_) => Vec::new(),
        }
    } else {
        axes_of(now, &seed.job)
    }
}

/// The edit the dialog describes: only the fields the person moved. An output that changed is
/// returned as asked for, not yet reserved.
///
/// # Errors
///
/// A sentence for a value that cannot be a job setting.
pub(super) fn edit_from_controls(seed: &Seed, now: &Controls) -> Result<JobEdit, String> {
    let s = &seed.controls;
    let job = &seed.job;
    let is_video = seed.is_video();
    let is_ray = job.engine.is_ray();
    let mut edit = JobEdit::default();
    if now.compute_index != s.compute_index {
        edit.target = Some(match now.compute_index {
            0 => ComputeChoice::Local,
            1 => ComputeChoice::Remote,
            _ => ComputeChoice::Both,
        });
    }
    if now.transfer_index != s.transfer_index {
        edit.transfer = Some(if now.transfer_index == 1 {
            TransferChoice::FinalPicture
        } else {
            TransferChoice::FullData
        });
    }
    if (now.size_preset, now.custom_width, now.custom_height)
        != (s.size_preset, s.custom_width, s.custom_height)
    {
        edit.size = Some(size_of(now, is_video)?);
    }
    if !is_ray && now.sample_exponent != s.sample_exponent {
        let exponent = clamp_exponent(
            u32::try_from(now.sample_exponent).unwrap_or(0),
            EXPORT_SAMPLES_EXPONENT_RANGE,
        );
        edit.samples_per_pixel = Some(1_u32 << exponent);
    }
    if now.bounce_index != s.bounce_index {
        edit.max_bounces = Some(nearest_bounce_cap(now.bounce_index));
    }
    if now.color_space_index != s.color_space_index {
        edit.color_space = Some(match now.color_space_index {
            1 => JobColorSpace::DisplayP3,
            2 => JobColorSpace::Rec2020,
            _ => JobColorSpace::Srgb,
        });
    }
    if is_ray {
        if now.ray_taps_index != s.ray_taps_index {
            edit.ray_taps = usize::try_from(now.ray_taps_index)
                .ok()
                .and_then(|at| AA_TAP_CHOICES.get(at))
                .map(|taps| u32::from(*taps));
        }
        if now.ray_fire != s.ray_fire {
            edit.ray_fire = Some(now.ray_fire);
        }
    }
    if now.label != s.label {
        edit.label = Some(now.label.trim().to_string());
    }
    edit_output(seed, now, &mut edit)?;
    if is_video {
        edit_video(seed, now, &mut edit)?;
    }
    if let Some(material) = &seed.material_edit {
        edit.material.clone_from(&material.material);
        edit.zoning.clone_from(&material.zoning);
        edit.effects_material_inclusions
            .clone_from(&material.effects_material_inclusions);
        edit.fluorescence.clone_from(&material.fluorescence);
    }
    edit.hdr_path.clone_from(&seed.hdr_path);
    if let Some(measured) = &seed.curves
        && measured.key == curve_key(seed, now)
    {
        edit.curves = Some(measured.sets.clone());
    }
    Ok(edit)
}

/// What the measured curves would be keyed by for the dialog as it stands.
pub(super) fn curve_key(seed: &Seed, now: &Controls) -> CurveKey {
    CurveKey {
        axes: effective_axes(seed, now),
        material: (seed.material_edit.is_some())
            .then(|| {
                seed.materials
                    .get(usize::try_from(now.material_index).unwrap_or(0))
                    .cloned()
            })
            .flatten(),
    }
}

fn edit_output(seed: &Seed, now: &Controls, edit: &mut JobEdit) -> Result<(), String> {
    let s = &seed.controls;
    let folder_changed = now.output_folder != s.output_folder;
    if seed.is_video() {
        if now.video_name != s.video_name {
            edit.video_name = Some(plain_name(&now.video_name, "video name")?);
        }
        if folder_changed || now.video_name != s.video_name {
            let name = plain_name(&now.video_name, "video name")?;
            edit.output = Some(Path::new(now.output_folder.trim()).join(name));
        }
        return Ok(());
    }
    if folder_changed || now.file_name != s.file_name {
        let mut name = plain_name(&now.file_name, "file name")?;
        if !name.to_ascii_lowercase().ends_with(".png") {
            name.push_str(".png");
        }
        if now.output_folder.trim().is_empty() {
            return Err("The output folder is empty.".to_string());
        }
        edit.output = Some(Path::new(now.output_folder.trim()).join(name));
    }
    Ok(())
}

fn edit_video(seed: &Seed, now: &Controls, edit: &mut JobEdit) -> Result<(), String> {
    let s = &seed.controls;
    if now.fps_index != s.fps_index {
        edit.fps = Some(resolve_fps(now.fps_index));
    }
    if now.keep_frames != s.keep_frames {
        edit.keep_frames = Some(now.keep_frames);
    }
    if (now.start_deg, now.end_deg, now.step_deg.to_bits())
        != (s.start_deg, s.end_deg, s.step_deg.to_bits())
    {
        if !(now.step_deg.is_finite() && now.step_deg > 0.0) {
            return Err("The tilt step must be above zero.".to_string());
        }
        let step = (f64::from(now.step_deg) * 100.0).round() / 100.0;
        edit.sweep = Some((f64::from(now.start_deg), f64::from(now.end_deg), step));
    }
    if now.axes != s.axes {
        let axes = axes_of(now, &seed.job);
        if axes.is_empty() {
            return Err("Choose at least one tilt axis.".to_string());
        }
        edit.axes = Some(axes);
    }
    if now.overlay != s.overlay {
        edit.overlay = Some(OverlaySelection {
            brilliance: now.overlay[0],
            windowing: now.overlay[1],
            extinction: now.overlay[2],
            tilt_brilliance: now.overlay[3],
            angle: now.overlay[4],
        });
    }
    Ok(())
}
