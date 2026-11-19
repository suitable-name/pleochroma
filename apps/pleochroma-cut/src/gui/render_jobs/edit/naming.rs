//! Names for edited and copied jobs: reserving an output, renaming from the template, and the automatic label.

use super::save::deps;
use crate::{
    MainWindow,
    bridge::export_thread::filename_template::{TemplateContext, resolve_export_name},
    gui::render_jobs::{capture_still::still_label, capture_video::video_label},
};
use pleochroma::color::metrics::PROFILE_AZIMUTHS_DEG;
use pleochroma_render_jobs::{
    JobKind, RenderJobFile,
    paths::{reserve_unique_file, reserve_unique_folder},
};
use std::path::{Path, PathBuf};

/// Whether a job's output is a picture file or a frame folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::gui) enum JobKindTag {
    /// One still picture: the output is a PNG file.
    Still,
    /// A tilt video: the output is a frame folder.
    TiltVideo,
}

impl JobKindTag {
    /// The kind of `job`.
    #[must_use]
    pub(in crate::gui) const fn of(job: &RenderJobFile) -> Self {
        match job.kind {
            JobKind::Still(_) => Self::Still,
            JobKind::TiltVideo(_) => Self::TiltVideo,
        }
    }
}

/// `wanted`, or the first free variant of it, against `taken` and `exists`.
pub(super) fn reserve_in(
    wanted: &Path,
    kind: JobKindTag,
    taken: &[PathBuf],
    exists: &dyn Fn(&Path) -> bool,
) -> PathBuf {
    match kind {
        JobKindTag::Still => reserve_unique_file(wanted, taken, exists),
        JobKindTag::TiltVideo => reserve_unique_folder(wanted, taken, exists),
    }
}

/// The outputs of every job in the queue, except `own` when given.
pub(super) fn taken_outputs(own: Option<&Path>) -> Vec<PathBuf> {
    let Some(deps) = deps() else {
        return Vec::new();
    };
    let listed = deps.db().render_job_output_paths();
    match listed {
        Ok(paths) => paths
            .into_iter()
            .map(PathBuf::from)
            .filter(|path| own.is_none_or(|own| path.as_path() != own))
            .collect(),
        Err(error) => {
            tracing::warn!("The render job outputs could not be read: {error}");
            Vec::new()
        }
    }
}

/// A free file (still) or frame folder (video) against `render_job_output_paths()` and the disk.
pub(in crate::gui) fn reserve_output(_ui: &MainWindow, wanted: &Path, kind: JobKindTag) -> PathBuf {
    reserve_in(wanted, kind, &taken_outputs(None), &|path| path.exists())
}

/// [`reserve_output`] for a job's own new place: its current output does not count as taken.
pub(super) fn reserve_excluding(wanted: &Path, kind: JobKindTag, own: &Path) -> PathBuf {
    reserve_in(wanted, kind, &taken_outputs(Some(own)), &|path| {
        path.exists()
    })
}

/// "Rename from Template": the file name from the current template and the job's own context.
///
/// A still takes the export file name template and ends in `.png`. A video takes the tilt
/// video's template, without an extension.
///
/// # Errors
///
/// A sentence when the settings are not available.
pub(in crate::gui) fn name_from_template(
    _ui: &MainWindow,
    job: &RenderJobFile,
) -> Result<String, String> {
    let deps = deps().ok_or_else(|| "The settings are not available.".to_string())?;
    let settings = deps.settings_store.snapshot().settings;
    let (template, samples, color_space, is_video) = match &job.kind {
        JobKind::Still(still) => (
            settings.export_filename_template.clone(),
            still.samples_per_pixel,
            still.color_space,
            false,
        ),
        JobKind::TiltVideo(video) => (
            settings.output.tilt_video.filename_template.clone(),
            video.samples_per_pixel,
            video.color_space,
            true,
        ),
    };
    let context = TemplateContext {
        design: job.design.title.clone(),
        designer: job.design.designer.clone(),
        shape: job.design.shape.clone(),
        material: job.scene.material.name.clone(),
        ri: job.design.ri.clone(),
        width: job.scene.width,
        height: job.scene.height,
        spp: samples,
        bounces: job.scene.max_bounces,
        colorspace: color_space.label().to_string(),
        preset: match &job.kind {
            JobKind::Still(still) => still.preset_label.clone(),
            JobKind::TiltVideo(_) => String::new(),
        },
        lighting: job.scene.lighting_preset.label().to_string(),
        yaw_deg: job.scene.yaw.to_degrees(),
        pitch_deg: job.scene.pitch.to_degrees(),
        distance: job.scene.distance,
        exposure: job.scene.exposure,
    };
    let name = resolve_export_name(&template, &context);
    if is_video {
        Ok(name
            .strip_suffix(".png")
            .map_or_else(|| name.clone(), str::to_string))
    } else {
        Ok(name)
    }
}

/// The automatic label of a job (for the label rule of the edit dialog).
#[must_use]
pub(in crate::gui) fn automatic_label(job: &RenderJobFile) -> String {
    match &job.kind {
        JobKind::Still(still) => still_label(
            &job.design.title,
            &still.preset_label,
            job.scene.width,
            job.scene.height,
        ),
        JobKind::TiltVideo(video) => {
            let angles: Vec<f32> = video
                .axes()
                .iter()
                .map(|&axis| {
                    PROFILE_AZIMUTHS_DEG
                        .get(axis as usize)
                        .copied()
                        .unwrap_or(0.0)
                })
                .collect();
            video_label(&job.design.title, &angles)
        }
    }
}
