//! Editing a queued render job: one [`JobEdit`] says what changes, and the rules say what the change does to the job's progress.
//!
//! Pure functions over [`RenderJobFile`], so the rules are tested without a window or a
//! database. The app builds the edit from the dialog, asks [`JobEdit::apply`] for the new job
//! and [`edit_rule`] for what saving does to the row.

use crate::{
    ComputeChoice, TransferChoice, format_for,
    job::{
        JobColorSpace, JobKind, MetricCurvesData, OverlaySelection, RenderJobFile, TiltVideoJob,
    },
    state::JobState,
};
use pleochroma::optics::{
    fluorescence::Fluorescence, materials::GemMaterial, zoning::ZonedAbsorption,
};
use pleochroma_net::scene::material_inclusions::MaterialInclusions;
use std::{fmt, path::PathBuf};

/// The inclusion record a job keeps for `record`.
///
/// An empty record (no families, no structure) means "none".
fn inclusions_or_none(record: &MaterialInclusions) -> Option<MaterialInclusions> {
    (!record.families.is_empty() || record.structure.is_some()).then(|| record.clone())
}

/// One edit of a job. Every field `None` keeps the job as it is.
#[derive(Debug, Clone, Default)]
pub struct JobEdit {
    /// Which computers render.
    pub target: Option<ComputeChoice>,
    /// What a remote worker sends back.
    pub transfer: Option<TransferChoice>,
    /// The picture size in pixels, width and height.
    pub size: Option<(u32, u32)>,
    /// Samples per pixel. Refused for a Ray-Traced job, whose samples follow the taps.
    pub samples_per_pixel: Option<u32>,
    /// The bounce limit.
    pub max_bounces: Option<u32>,
    /// The color space of the picture or the frames.
    pub color_space: Option<JobColorSpace>,
    /// The anti-aliasing tap count of a Ray-Traced job (1, 4, 8 or 16).
    pub ray_taps: Option<u32>,
    /// The fire switch of a Ray-Traced job.
    pub ray_fire: Option<bool>,
    /// Video: frames per second.
    pub fps: Option<u32>,
    /// Video: whether the frames stay on disk.
    pub keep_frames: Option<bool>,
    /// Video: the video name, which is part of the frame folder.
    pub video_name: Option<String>,
    /// Video: the sweep as start, end and step, in degrees.
    pub sweep: Option<(f64, f64, f64)>,
    /// Video: the tilt axes in order, at least one.
    pub axes: Option<Vec<u32>>,
    /// Video: which overlays are drawn.
    pub overlay: Option<OverlaySelection>,
    /// Video: one measured curve set per axis, in the order of the axes.
    pub curves: Option<Vec<MetricCurvesData>>,
    /// The output: the picture file, or the frame folder of a video.
    pub output: Option<PathBuf>,
    /// The one-line name in the job list.
    pub label: Option<String>,
    /// The material, already re-resolved for the job.
    pub material: Option<GemMaterial>,
    /// The color zones: `Some(None)` removes them.
    pub zoning: Option<Option<ZonedAbsorption>>,
    /// The inclusions the material carries, which the scene's effects record.
    pub effects_material_inclusions: Option<MaterialInclusions>,
    /// The fluorescence of the material.
    pub fluorescence: Option<Fluorescence>,
    /// The new place of the job's HDR map (Locate HDR Map).
    pub hdr_path: Option<PathBuf>,
}

/// Why an edit cannot be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditRefusal {
    /// The job is running.
    Running,
    /// A Ray-Traced job cannot use the remote worker.
    RayRemote,
    /// The edit needs measured tilt curves and carries none.
    CurvesMissing,
    /// The edited job breaks a rule; the sentence says which.
    Invalid(String),
}

impl fmt::Display for EditRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => write!(f, "This job is running. Pause it before editing."),
            Self::RayRemote => write!(f, "Ray-Traced jobs render on this computer only."),
            Self::CurvesMissing => write!(
                f,
                "The tilt curves of this stone have not been measured yet."
            ),
            Self::Invalid(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for EditRefusal {}

/// What saving an edit does to a job's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditMode {
    /// The job is saved where it stands.
    InPlace,
    /// A failed job: a change that is more than a label or name puts it back in the queue and
    /// keeps its finished frames (like Resume). Applies only when
    /// [`JobEdit::only_label_or_name`] is false.
    KeepFramesRequeue,
    /// A done or cancelled job: a change that is more than a label or name asks "Render this job
    /// again?" first.
    RequeueWithConfirm,
}

/// What saving an edit does to a job in `state`, or why it cannot be edited.
///
/// # Errors
///
/// A sentence for a running job.
pub const fn edit_rule(state: JobState) -> Result<EditMode, &'static str> {
    match state {
        JobState::Queued | JobState::Paused => Ok(EditMode::InPlace),
        JobState::Failed => Ok(EditMode::KeepFramesRequeue),
        JobState::Done | JobState::Cancelled => Ok(EditMode::RequeueWithConfirm),
        JobState::Running => Err("A running job cannot be edited. Pause it first."),
    }
}

/// The label of a job after an edit.
///
/// The label is rebuilt only while it still equals the automatic label of the job before the
/// edit; a typed label stays.
#[must_use]
pub fn label_after_edit(current: &str, automatic_before: &str, automatic_after: &str) -> String {
    if current == automatic_before {
        automatic_after.to_string()
    } else {
        current.to_string()
    }
}

fn invalid(message: &str) -> EditRefusal {
    EditRefusal::Invalid(message.to_string())
}

pub(crate) const fn video_of(job: &RenderJobFile) -> Option<&TiltVideoJob> {
    match &job.kind {
        JobKind::TiltVideo(video) => Some(video),
        JobKind::Still(_) => None,
    }
}

pub(crate) const fn samples_of(job: &RenderJobFile) -> u32 {
    match &job.kind {
        JobKind::Still(still) => still.samples_per_pixel,
        JobKind::TiltVideo(video) => video.samples_per_pixel,
    }
}

pub(crate) const fn color_space_of(job: &RenderJobFile) -> JobColorSpace {
    match &job.kind {
        JobKind::Still(still) => still.color_space,
        JobKind::TiltVideo(video) => video.color_space,
    }
}

pub(crate) fn set_samples(job: &mut RenderJobFile, samples: u32) {
    match &mut job.kind {
        JobKind::Still(still) => still.samples_per_pixel = samples,
        JobKind::TiltVideo(video) => video.samples_per_pixel = samples,
    }
}

fn set_color_space(job: &mut RenderJobFile, space: JobColorSpace) {
    match &mut job.kind {
        JobKind::Still(still) => still.color_space = space,
        JobKind::TiltVideo(video) => video.color_space = space,
    }
}

/// The curve sets of a video in axis order: the first axis, then the further ones.
fn curve_sets(video: &TiltVideoJob) -> Vec<&MetricCurvesData> {
    video.curves.iter().chain(&video.more_curves).collect()
}

impl JobEdit {
    /// The edited job, validated. The token and the format version never change by hand: the
    /// format is recomputed from the edited job.
    ///
    /// # Errors
    ///
    /// [`EditRefusal::RayRemote`] for a remote choice on a Ray-Traced job,
    /// [`EditRefusal::CurvesMissing`] when [`Self::needs_curves`] holds and `curves` is `None`,
    /// and [`EditRefusal::Invalid`] with one sentence when a field does not fit the job or the
    /// edited job breaks a rule of `validate()`.
    pub fn apply(&self, job: &RenderJobFile) -> Result<RenderJobFile, EditRefusal> {
        let mut out = job.clone();
        self.apply_compute_and_picture(&mut out)?;
        self.apply_ray(&mut out)?;
        self.apply_video(job, &mut out)?;
        self.apply_material(&mut out);
        out.format = format_for(&out);
        out.validate()
            .map_err(|error| EditRefusal::Invalid(error.to_string()))?;
        Ok(out)
    }

    /// Whether the edit changes the picture or the frames, against the job as it was.
    ///
    /// A field set to the value the job already has changes nothing. Compute, transfer, label,
    /// output, frame rate, keep frames and the HDR map's place are not pixels.
    #[must_use]
    pub fn changes_pixels(&self, before: &RenderJobFile) -> bool {
        self.changes_picture_fields(before)
            || self.changes_video_fields(before)
            || self.changes_material_fields(before)
    }

    /// [`Self::changes_pixels`], or a tilt video whose frame folder changed (a new output or a
    /// new video name).
    #[must_use]
    pub fn resets_progress(&self, before: &RenderJobFile) -> bool {
        if self.changes_pixels(before) {
            return true;
        }
        video_of(before).is_some_and(|video| {
            self.video_name
                .as_ref()
                .is_some_and(|name| *name != video.video_name)
                || self
                    .output
                    .as_ref()
                    .is_some_and(|path| path.to_string_lossy() != before.output)
        })
    }

    /// Whether the edit holds nothing but a label and an output name or place.
    ///
    /// A video name is not a name in this sense: it is part of the frame folder.
    #[must_use]
    pub fn only_label_or_name(&self) -> bool {
        self.target.is_none()
            && self.transfer.is_none()
            && self.size.is_none()
            && self.samples_per_pixel.is_none()
            && self.max_bounces.is_none()
            && self.color_space.is_none()
            && self.ray_taps.is_none()
            && self.ray_fire.is_none()
            && !self.has_video_fields()
            && self.material.is_none()
            && self.zoning.is_none()
            && self.effects_material_inclusions.is_none()
            && self.fluorescence.is_none()
            && self.hdr_path.is_none()
    }

    /// A video edit that needs new curves: an overlay is on afterwards, and the material or the
    /// axes changed, or the job holds no curves yet.
    #[must_use]
    pub fn needs_curves(&self, before: &RenderJobFile) -> bool {
        let Some(video) = video_of(before) else {
            return false;
        };
        if self.overlay.unwrap_or(video.overlay).count() == 0 {
            return false;
        }
        let material_changed = self
            .material
            .as_ref()
            .is_some_and(|material| *material != before.scene.material);
        let axes_changed = self.axes.as_ref().is_some_and(|axes| *axes != video.axes());
        material_changed || axes_changed || video.curves.is_none()
    }

    fn has_video_fields(&self) -> bool {
        self.fps.is_some()
            || self.keep_frames.is_some()
            || self.video_name.is_some()
            || self.sweep.is_some()
            || self.axes.is_some()
            || self.overlay.is_some()
            || self.curves.is_some()
    }

    fn changes_picture_fields(&self, before: &RenderJobFile) -> bool {
        self.size
            .is_some_and(|size| size != (before.scene.width, before.scene.height))
            || self
                .samples_per_pixel
                .is_some_and(|samples| samples != samples_of(before))
            || self
                .max_bounces
                .is_some_and(|bounces| bounces != before.scene.max_bounces)
            || self
                .color_space
                .is_some_and(|space| space != color_space_of(before))
            || self
                .ray_taps
                .is_some_and(|taps| taps != u32::from(before.engine.ray.aa_taps))
            || self
                .ray_fire
                .is_some_and(|fire| fire != before.engine.ray.fire)
    }

    fn changes_video_fields(&self, before: &RenderJobFile) -> bool {
        let Some(video) = video_of(before) else {
            return false;
        };
        self.sweep
            .is_some_and(|sweep| sweep != (video.start_deg, video.end_deg, video.step_deg))
            || self.axes.as_ref().is_some_and(|axes| *axes != video.axes())
            || self.overlay.is_some_and(|overlay| overlay != video.overlay)
            || self.curves.as_ref().is_some_and(|curves| {
                let held = curve_sets(video);
                curves.len() != held.len() || curves.iter().zip(held).any(|(new, old)| new != old)
            })
    }

    fn changes_material_fields(&self, before: &RenderJobFile) -> bool {
        self.material
            .as_ref()
            .is_some_and(|material| *material != before.scene.material)
            || self
                .zoning
                .as_ref()
                .is_some_and(|zoning| *zoning != before.zoning)
            || self
                .effects_material_inclusions
                .as_ref()
                .is_some_and(|record| {
                    before.scene.effects.material_inclusions != inclusions_or_none(record)
                })
            || self
                .fluorescence
                .as_ref()
                .is_some_and(|fluorescence| *fluorescence != before.scene.fluorescence)
    }

    fn apply_compute_and_picture(&self, job: &mut RenderJobFile) -> Result<(), EditRefusal> {
        if let Some(target) = self.target {
            if job.engine.is_ray() && target != ComputeChoice::Local {
                return Err(EditRefusal::RayRemote);
            }
            job.compute.target = target;
        }
        if let Some(transfer) = self.transfer {
            job.compute.transfer = transfer;
        }
        if let Some((width, height)) = self.size {
            job.scene.width = width;
            job.scene.height = height;
        }
        if let Some(bounces) = self.max_bounces {
            job.scene.max_bounces = bounces;
        }
        if let Some(space) = self.color_space {
            set_color_space(job, space);
        }
        if let Some(label) = &self.label {
            job.label.clone_from(label);
        }
        if let Some(path) = &self.output {
            job.output = path.to_string_lossy().into_owned();
        }
        if let Some(path) = &self.hdr_path {
            let source = job
                .hdr
                .as_mut()
                .ok_or_else(|| invalid("This job is not lit by an HDR map."))?;
            source.path = path.to_string_lossy().into_owned();
        }
        Ok(())
    }

    /// Samples for a Path-Traced job; taps and fire for a Ray-Traced one. The samples of a
    /// Ray-Traced job follow its taps.
    fn apply_ray(&self, job: &mut RenderJobFile) -> Result<(), EditRefusal> {
        if job.engine.is_ray() {
            if self.samples_per_pixel.is_some() {
                return Err(invalid(
                    "A Ray-Traced job takes its samples from the anti-aliasing taps.",
                ));
            }
            if let Some(taps) = self.ray_taps {
                let taps = u8::try_from(taps)
                    .ok()
                    .filter(|taps| pleochroma::renderer::engine::AA_TAP_CHOICES.contains(taps))
                    .ok_or_else(|| {
                        invalid("Ray-Traced anti-aliasing must be 1, 4, 8 or 16 taps.")
                    })?;
                job.engine.ray.aa_taps = taps;
                job.scene.engine.ray.aa_taps = taps;
                set_samples(job, u32::from(taps));
            }
            if let Some(fire) = self.ray_fire {
                job.engine.ray.fire = fire;
                job.scene.engine.ray.fire = fire;
            }
            return Ok(());
        }
        if self.ray_taps.is_some() || self.ray_fire.is_some() {
            return Err(invalid(
                "Taps and fire belong to Ray-Traced jobs; this job is Path-Traced.",
            ));
        }
        if let Some(samples) = self.samples_per_pixel {
            set_samples(job, samples);
        }
        Ok(())
    }

    fn apply_video(
        &self,
        before: &RenderJobFile,
        job: &mut RenderJobFile,
    ) -> Result<(), EditRefusal> {
        let JobKind::TiltVideo(video) = &mut job.kind else {
            return if self.has_video_fields() {
                Err(invalid("Tilt video settings do not apply to a still."))
            } else {
                Ok(())
            };
        };
        if let Some(fps) = self.fps {
            video.fps = fps;
        }
        if let Some(keep) = self.keep_frames {
            video.keep_frames = keep;
        }
        if let Some(name) = &self.video_name {
            video.video_name.clone_from(name);
        }
        if let Some((start, end, step)) = self.sweep {
            video.start_deg = start;
            video.end_deg = end;
            video.step_deg = step;
        }
        if let Some(axes) = &self.axes {
            let (first, rest) = axes
                .split_first()
                .ok_or_else(|| invalid("A tilt video needs at least one axis."))?;
            video.axis_index = *first;
            video.more_axes = rest.to_vec();
        }
        if self.sweep.is_some() || self.axes.is_some() {
            let axes: Vec<u32> = video.axes();
            let count = pose_count(&axes, video.start_deg, video.end_deg, video.step_deg);
            if count == 0 {
                return Err(invalid("The tilt sweep has no frames."));
            }
            video.total_frames = u32::try_from(count).unwrap_or(u32::MAX);
        }
        if let Some(overlay) = self.overlay {
            video.overlay = overlay;
        }
        self.apply_curves(before, video)
    }

    fn apply_curves(
        &self,
        before: &RenderJobFile,
        video: &mut TiltVideoJob,
    ) -> Result<(), EditRefusal> {
        if video.overlay.count() == 0 {
            if self.overlay.is_some() || self.curves.is_some() {
                video.curves = None;
                video.more_curves.clear();
            }
            return Ok(());
        }
        let curves = match &self.curves {
            Some(curves) => curves,
            None if self.needs_curves(before) => return Err(EditRefusal::CurvesMissing),
            None => return Ok(()),
        };
        if curves.len() != video.axes().len() {
            return Err(invalid("The tilt curves must be one set for each axis."));
        }
        let mut sets = curves.iter().cloned();
        video.curves = sets.next();
        video.more_curves = sets.collect();
        Ok(())
    }

    fn apply_material(&self, job: &mut RenderJobFile) {
        if let Some(material) = &self.material {
            job.scene.material = material.clone();
        }
        match &self.zoning {
            Some(zoning) => {
                job.zoning.clone_from(zoning);
                job.scene.material.zoning = None;
            }
            None if self.material.is_some() => job.move_scene_zoning_to_file(),
            None => {}
        }
        if let Some(record) = &self.effects_material_inclusions {
            // An empty record (no families, no structure) is how a material change says
            // "the new material has no inclusions".
            job.scene.effects.material_inclusions = inclusions_or_none(record);
        }
        if let Some(fluorescence) = &self.fluorescence {
            job.scene.fluorescence = fluorescence.clone();
        }
    }
}

/// The number of frames of a video over `axes`, as the tilt video export counts them: one pass
/// for one axis, and for several axes each rocks `0 -> end -> start -> 0`, a pose shared by two
/// legs drawn once.
fn pose_count(axes: &[u32], start_deg: f64, end_deg: f64, step_deg: f64) -> usize {
    if frame_count(start_deg, end_deg, step_deg) == 0 {
        return 0;
    }
    if axes.len() == 1 {
        return frame_count(start_deg, end_deg, step_deg);
    }
    let mut count = 0_usize;
    let mut last: Option<f64> = None;
    for _ in axes {
        for (from, to) in [(0.0, end_deg), (end_deg, start_deg), (start_deg, 0.0)] {
            for tilt in leg_angles(from, to, step_deg) {
                if !last.is_some_and(|last| (last - tilt).abs() < 1e-9) {
                    count += 1;
                    last = Some(tilt);
                }
            }
        }
    }
    count
}

/// `floor((end - start) / step) + 1` frames; 0 for a non-positive step or an end before the start.
fn frame_count(start_deg: f64, end_deg: f64, step_deg: f64) -> usize {
    if step_deg <= 0.0 || end_deg < start_deg {
        return 0;
    }
    // The epsilon keeps a division that lands a hair under an exact multiple from losing a frame.
    let steps = ((end_deg - start_deg) / step_deg + 1e-6).floor();
    if steps.is_finite() {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "steps is finite and not negative here; `as` saturates"
        )]
        let steps = steps as usize;
        steps.saturating_add(1)
    } else {
        0
    }
}

/// The angle of frame `index` of `total`; the last frame is pinned to `end_deg`.
fn frame_angle(start_deg: f64, end_deg: f64, step_deg: f64, index: usize, total: usize) -> f64 {
    if total == 0 {
        start_deg
    } else if index + 1 >= total {
        end_deg
    } else {
        step_deg.mul_add(index as f64, start_deg)
    }
}

/// The angles of one leg from `from_deg` to `to_deg`, both ends included, in either direction.
fn leg_angles(from_deg: f64, to_deg: f64, step_deg: f64) -> Vec<f64> {
    let span = (to_deg - from_deg).abs();
    let count = frame_count(0.0, span, step_deg);
    let sign: f64 = if to_deg < from_deg { -1.0 } else { 1.0 };
    (0..count)
        .map(|i| sign.mul_add(frame_angle(0.0, span, step_deg, i, count), from_deg))
        .collect()
}
