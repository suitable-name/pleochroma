//! Job file text: reading and writing `*.job.json`.
//!
//! JSON has no way to hold NaN or infinity, and `serde_json` writes them as `null`
//! without complaint. [`to_text`] therefore reads its own output back before returning
//! it, so a job that could not be read again is refused when it is saved, not when it is
//! rendered.

use crate::job::{JOB_FORMAT_VERSION, RenderJobFile};
use serde::Deserialize;
use std::fmt;

/// Why a job file could not be written or read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobFileError {
    /// The text is not a Pleochroma render job (not a JSON object with a `format`).
    NotAJobFile,
    /// The file was made by a newer version of Pleochroma.
    NewerFormat {
        /// The format in the file.
        found: u32,
        /// The newest format this version reads.
        supported: u32,
    },
    /// The text is a job file, but its content could not be read.
    Malformed(String),
    /// The job breaks a rule (the sentence names it).
    Invalid(String),
    /// The job cannot be written as text.
    Unencodable(String),
}

impl fmt::Display for JobFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAJobFile => write!(f, "This is not a Pleochroma render job file."),
            Self::NewerFormat { found, supported } => write!(
                f,
                "This job file was made by a newer version of Pleochroma (format {found}); this version reads format {supported}."
            ),
            Self::Malformed(message) => write!(f, "The job file could not be read: {message}"),
            Self::Invalid(message) => write!(f, "{message}"),
            Self::Unencodable(message) => write!(f, "The job could not be saved: {message}"),
        }
    }
}

impl std::error::Error for JobFileError {}

/// Writes a job as pretty JSON text ending in a newline.
///
/// # Errors
///
/// [`JobFileError::Invalid`] when the job breaks a rule, and
/// [`JobFileError::Unencodable`] when a number in it cannot be written (NaN, infinity).
pub fn to_text(job: &RenderJobFile) -> Result<String, JobFileError> {
    job.validate()?;
    let mut text = serde_json::to_string_pretty(job)
        .map_err(|error| JobFileError::Unencodable(error.to_string()))?;
    text.push('\n');
    if let Err(error) = serde_json::from_str::<RenderJobFile>(&text) {
        return Err(JobFileError::Unencodable(format!(
            "a number in the job is not a finite number ({error})."
        )));
    }
    Ok(text)
}

/// Reads a job from JSON text and validates it.
///
/// # Errors
///
/// [`JobFileError::NotAJobFile`], [`JobFileError::NewerFormat`],
/// [`JobFileError::Malformed`] or [`JobFileError::Invalid`], in that order of checking.
pub fn from_text(text: &str) -> Result<RenderJobFile, JobFileError> {
    read_supported_format(text, JOB_FORMAT_VERSION)
}

/// Checks the reader's format ceiling before deserializing the scene.
fn read_supported_format(text: &str, supported: u32) -> Result<RenderJobFile, JobFileError> {
    #[derive(Deserialize)]
    struct Probe {
        format: Option<u32>,
    }
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| JobFileError::NotAJobFile)?;
    if !value.is_object() {
        return Err(JobFileError::NotAJobFile);
    }
    // A `format` that is not a number is not one of ours either.
    let probe: Probe =
        serde_json::from_value(value.clone()).map_err(|_| JobFileError::NotAJobFile)?;
    let Some(found) = probe.format else {
        return Err(JobFileError::NotAJobFile);
    };
    if found > supported {
        return Err(JobFileError::NewerFormat { found, supported });
    }
    let mut job: RenderJobFile = serde_json::from_value(value)
        .map_err(|error| JobFileError::Malformed(error.to_string()))?;
    // A legacy file holds a card level, not the tray radiance: convert it once, under the
    // file's own preset and light pose.
    job.upgrade_backdrop_model();
    job.validate()?;
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{
        JobKind, TiltVideoJob,
        test_support::{still_job, video_job},
    };

    fn round_trip(job: &RenderJobFile) -> RenderJobFile {
        let text = to_text(job).expect("the job encodes");
        assert!(text.ends_with('\n'));
        from_text(&text).expect("the job decodes")
    }

    #[test]
    fn a_still_round_trips_bitwise() {
        let job = still_job();
        let back = round_trip(&job);
        assert_eq!(back, job);
        assert_eq!(back.scene.exposure.to_bits(), job.scene.exposure.to_bits());
        assert_eq!(back.scene.yaw.to_bits(), job.scene.yaw.to_bits());
        assert_eq!(
            back.scene.surface_glare.to_bits(),
            job.scene.surface_glare.to_bits()
        );
        assert_eq!(
            back.scene.head_shadow_deg.to_bits(),
            job.scene.head_shadow_deg.to_bits()
        );
        for (a, b) in back.scene.planes.iter().zip(&job.scene.planes) {
            assert_eq!(a, b);
        }
        assert_eq!(back.scene.material, job.scene.material);
    }

    #[test]
    fn a_video_with_hdr_and_curves_round_trips_bitwise() {
        let job = video_job();
        let back = round_trip(&job);
        assert_eq!(back, job);
        let (JobKind::TiltVideo(a), JobKind::TiltVideo(b)) = (&back.kind, &job.kind) else {
            panic!("a video stays a video");
        };
        let (Some(a), Some(b)) = (&a.curves, &b.curves) else {
            panic!("the curves survive");
        };
        for (x, y) in a.brilliance.iter().zip(&b.brilliance) {
            assert_eq!(x.to_bits(), y.to_bits());
        }
        for (x, y) in a.extinction.iter().zip(&b.extinction) {
            assert_eq!(x.to_bits(), y.to_bits());
        }
    }

    #[test]
    fn legacy_path_jobs_omit_the_engine_and_ray_jobs_use_format_two() {
        use pleochroma::renderer::engine::{EngineSpec, RayOptions, RenderEngine};
        let legacy = still_job();
        let text = to_text(&legacy).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(value.get("engine").is_none());
        assert_eq!(value["format"], 1);
        assert_eq!(from_text(&text).unwrap().engine, EngineSpec::default());
        for taps in [1, 4, 8, 16] {
            let mut ray = legacy.clone();
            ray.engine = EngineSpec {
                engine: RenderEngine::RayTraced,
                ray: RayOptions {
                    aa_taps: taps,
                    fire: false,
                },
            };
            ray.scene.engine = ray.engine;
            ray.format = 2;
            let JobKind::Still(still) = &mut ray.kind else {
                unreachable!()
            };
            still.samples_per_pixel = u32::from(taps);
            assert_eq!(round_trip(&ray), ray);
            ray.format = 1;
            assert!(matches!(to_text(&ray), Err(JobFileError::Invalid(_))));
            ray.format = 2;
            if let JobKind::Still(still) = &mut ray.kind {
                still.samples_per_pixel = 32;
            }
            assert!(matches!(to_text(&ray), Err(JobFileError::Invalid(_))));
        }
        let mut ray_video = video_job();
        ray_video.engine.engine = RenderEngine::RayTraced;
        ray_video.scene.engine = ray_video.engine;
        ray_video.format = 2;
        if let JobKind::TiltVideo(video) = &mut ray_video.kind {
            video.samples_per_pixel = 8;
        }
        assert_eq!(round_trip(&ray_video), ray_video);
    }

    #[test]
    fn a_format_two_reader_refuses_a_shadowed_job() {
        let mut job = still_job();
        job.scene.stone_shadow = true;
        job.scene.lighting_preset = pleochroma::optics::raytracer::LightingPreset::LightTent;
        job.scene.backdrop = 0.9;
        job.format = crate::job::format_for(&job);
        let text = to_text(&job).expect("encode shadowed job");
        assert_eq!(
            read_supported_format(&text, 2),
            Err(JobFileError::NewerFormat {
                found: 3,
                supported: 2
            })
        );
        assert_eq!(from_text(&text).expect("read format 3"), job);
    }

    #[test]
    fn a_newer_format_is_refused_with_the_exact_sentence() {
        let error = from_text(r#"{"format": 7}"#).unwrap_err();
        assert_eq!(
            error,
            JobFileError::NewerFormat {
                found: 7,
                supported: 6
            }
        );
        assert_eq!(
            error.to_string(),
            "This job file was made by a newer version of Pleochroma (format 7); \
             this version reads format 6."
        );
    }

    /// A zoned job survives the text: its zones come back equal, the scene's material stays
    /// unzoned (the material's serde skips them), and a format-4 reader refuses the file.
    #[test]
    fn a_zoned_job_round_trips_and_a_format_four_reader_refuses_it() {
        use crate::job::test_support::zoned_still_job;
        let job = zoned_still_job();
        assert_eq!(job.format, 5);
        let text = to_text(&job).expect("encode zoned job");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["format"], 5);
        assert!(value.get("zoning").is_some());
        assert!(value["scene"]["material"].get("zoning").is_none());
        assert_eq!(
            read_supported_format(&text, 4),
            Err(JobFileError::NewerFormat {
                found: 5,
                supported: 4
            })
        );
        let back = from_text(&text).expect("read format 5");
        assert_eq!(back, job);
        assert_eq!(back.zoning, job.zoning);
        assert!(back.scene.material.zoning.is_none());
    }

    /// A job lit by a turned HDR map survives the text bit for bit, and a format-5 reader refuses
    /// the file instead of lighting the map unturned.
    #[test]
    fn a_turned_hdr_job_round_trips_and_a_format_five_reader_refuses_it() {
        use crate::job::test_support::turned_hdr_still_job;
        let job = turned_hdr_still_job();
        assert_eq!(job.format, 6);
        let text = to_text(&job).expect("encode turned job");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["format"], 6);
        assert!(value["scene"].get("hdr_orientation").is_some());
        assert_eq!(
            read_supported_format(&text, 5),
            Err(JobFileError::NewerFormat {
                found: 6,
                supported: 5
            })
        );
        let back = from_text(&text).expect("read format 6");
        assert_eq!(back, job);
        assert_eq!(
            back.scene.hdr_orientation.yaw.to_bits(),
            job.scene.hdr_orientation.yaw.to_bits()
        );
        assert!(back.scene.hdr_orientation.follows_camera);
    }

    /// Zones with a turned frame, a soft boundary and no axis following come back bit for bit:
    /// the omitted `follows_axis` reads as false and the frame's quaternion is not disturbed.
    #[test]
    fn rotated_soft_zones_without_axis_following_round_trip() {
        use crate::job::test_support::zoned_still_job;
        use pleochroma::optics::zoning::ZoneFrame;
        let mut job = zoned_still_job();
        let zoning = job.zoning.as_mut().expect("the job is zoned");
        zoning.frame = ZoneFrame::from_rotation_vector([0.3, 0.2, 0.1], [0.1, 0.0, 0.2]);
        zoning.boundary_softness_mm = 0.25;
        zoning.follows_axis = false;
        let back = round_trip(&job);
        assert_eq!(back, job);
        assert_eq!(back.zoning, job.zoning);
    }

    /// A job without zones writes no `zoning` key, so unzoned files keep the bytes they had
    /// before format 5.
    #[test]
    fn an_unzoned_job_writes_no_zoning_key() {
        let text = to_text(&still_job()).unwrap();
        assert!(!text.contains("zoning"));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["format"], 1);
    }

    #[test]
    fn a_format_three_reader_refuses_a_tray_reflection_job() {
        let mut job = still_job();
        job.scene.tray_reflection = 0.04;
        job.scene.lighting_preset = pleochroma::optics::raytracer::LightingPreset::LightTent;
        job.scene.backdrop = 0.9;
        job.format = crate::job::format_for(&job);
        let text = to_text(&job).expect("encode reflective job");
        assert_eq!(
            read_supported_format(&text, 3),
            Err(JobFileError::NewerFormat {
                found: 4,
                supported: 3
            })
        );
        assert_eq!(from_text(&text).expect("read format 4"), job);
    }

    #[test]
    fn other_text_is_not_a_job_file() {
        for text in ["{}", "[1,2]", "not json", "", r#"{"format": "one"}"#] {
            assert_eq!(from_text(text), Err(JobFileError::NotAJobFile), "{text}");
        }
        assert_eq!(
            JobFileError::NotAJobFile.to_string(),
            "This is not a Pleochroma render job file."
        );
    }

    #[test]
    fn an_unknown_extra_field_is_accepted() {
        let text = to_text(&still_job()).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value["added_in_the_future"] = serde_json::json!({"anything": [1, 2, 3]});
        let back = from_text(&value.to_string()).expect("extra fields are ignored");
        assert_eq!(back, still_job());
    }

    #[test]
    fn a_missing_required_field_is_malformed() {
        let text = to_text(&still_job()).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value.as_object_mut().unwrap().remove("scene");
        let error = from_text(&value.to_string()).unwrap_err();
        assert!(matches!(error, JobFileError::Malformed(_)), "{error:?}");
        assert!(
            error
                .to_string()
                .starts_with("The job file could not be read: ")
        );
    }

    #[test]
    fn an_invalid_job_is_refused_on_both_sides() {
        let mut job = still_job();
        job.scene.width = 4;
        let error = to_text(&job).unwrap_err();
        assert_eq!(
            error.to_string(),
            "The picture size must be between 16 and 8192 pixels."
        );

        let text = to_text(&still_job()).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value["scene"]["width"] = serde_json::json!(4);
        let error = from_text(&value.to_string()).unwrap_err();
        assert!(matches!(error, JobFileError::Invalid(_)), "{error:?}");
    }

    #[test]
    fn a_nan_in_the_scene_is_unencodable() {
        let mut job = still_job();
        job.scene.exposure = f32::NAN;
        let error = to_text(&job).unwrap_err();
        assert!(matches!(error, JobFileError::Unencodable(_)), "{error:?}");
        assert!(
            error
                .to_string()
                .starts_with("The job could not be saved: ")
        );
    }

    #[test]
    fn a_nan_in_a_curve_is_unencodable() {
        let mut job = video_job();
        if let JobKind::TiltVideo(TiltVideoJob {
            curves: Some(curves),
            ..
        }) = &mut job.kind
        {
            curves.brilliance[10] = f32::INFINITY;
        }
        assert!(matches!(to_text(&job), Err(JobFileError::Unencodable(_))));
    }
}
