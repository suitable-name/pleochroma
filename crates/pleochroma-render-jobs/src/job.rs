//! The render job file: what a job freezes, and the rules a job must satisfy.
//!
//! A job is [`pleochroma_net::SceneState`] (the fully resolved scene, the same type a
//! remote worker receives) plus the export settings and the output location. The remote
//! worker address, its certificates and the local CPU/GPU choice are machine settings,
//! not render content, so they are deliberately not part of a job.

use crate::codec::JobFileError;
use pleochroma::{
    optics::{materials::AbsorptionUnit, zoning::ZonedAbsorption},
    renderer::{accel::RenderAccel, adaptive::AdaptiveSettings},
};
use pleochroma_net::SceneState;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The newest job file format this version reads.
pub const JOB_FORMAT_VERSION: u32 = 8;
/// The file format version a job is written in.
///
/// A job whose stone stands in a pose other than the design's own needs format 8 (an older
/// reader would render the stone unposed), a job whose camera pans the stone off-centre format 7
/// (an older reader would render it centred), a job lit by a turned HDR map format 6, a job with color zones format 5, a job
/// with a tray reflection format 4 and a shadowed job format 3. Other jobs take their engine's
/// format: 2 for Ray-Traced, 1 otherwise. The map's turn counts only in an HDR-lit scene.
///
/// A tilt video that draws the stone shadow or the tray reflection (the video's own switches)
/// holds them in its scene like a still does, so it takes the same format, 3 or 4: a reader older
/// than that refuses the file, which is right, because it would draw the video without them. The
/// video job's own fields did not change, so no format of its own is needed.
#[must_use]
pub const fn format_for(job: &RenderJobFile) -> u32 {
    let pose = &job.scene.stone_pose;
    if pose[0] != 0.0 || pose[1] != 0.0 || pose[2] != 0.0 {
        8
    } else if job.scene.camera_pan[0] != 0.0 || job.scene.camera_pan[1] != 0.0 {
        7
    } else if job.scene.hdr().is_some() && job.scene.hdr_orientation.is_turned() {
        6
    } else if job.zoning.is_some() {
        5
    } else if job.scene.tray_reflection > 0.0 {
        4
    } else if job.scene.stone_shadow {
        3
    } else if job.engine.is_ray() {
        2
    } else {
        1
    }
}
/// [`RenderJobFile::scene_backdrop_model`] of a legacy file: the backdrop is a card level that
/// only the camera ray saw.
pub const BACKDROP_MODEL_LEGACY_CARD: u32 = 0;
/// [`RenderJobFile::scene_backdrop_model`] of a file that carries the tray radiance at
/// exposure 1, as every writer writes it.
pub const BACKDROP_MODEL_TRAY: u32 = 1;
/// The Grey card level a legacy file holds. A literal, so a change to the renderer's
/// `BACKDROP_GREY` does not change how old files convert.
const LEGACY_CARD_GREY: f32 = 0.23;
/// The White card level a legacy file holds (a light box behind the stone).
const LEGACY_CARD_WHITE: f32 = 8.0;

/// Converts a legacy card level in `scene.backdrop` to the tray radiance, once.
///
/// The conversion runs under the scene's own lighting preset and light pose:
/// `0.0` (As Lit) stays `0.0`, `0.23` becomes the Grey tray, `8.0` the White tray, and any
/// other value is kept as a literal radiance.
///
/// Call it only on a scene read from a file whose
/// [`RenderJobFile::scene_backdrop_model`] is [`BACKDROP_MODEL_LEGACY_CARD`].
pub fn legacy_backdrop_to_tray(scene: &mut SceneState) {
    use pleochroma::render_setup::Backdrop;
    let bits = scene.backdrop.to_bits();
    let choice = if bits == LEGACY_CARD_GREY.to_bits() {
        Backdrop::Grey
    } else if bits == LEGACY_CARD_WHITE.to_bits() {
        Backdrop::White
    } else {
        return;
    };
    scene.backdrop = choice.resolve(scene.lighting_preset, scene.light_yaw, scene.light_pitch);
}

/// The optical diagnostic model version this crate accepts in a job.
///
/// It must equal the renderer's `OPTICAL_MODEL_VERSION`. This crate cannot see the renderer,
/// so the two are kept equal by hand.
pub const SUPPORTED_OPTICAL_MODEL_VERSION: u32 = 1;
/// The suffix of a job file name.
pub const JOB_FILE_SUFFIX: &str = ".job.json";
/// The smallest picture side, in pixels.
pub const MIN_DIM: u32 = 16;
/// The largest picture side, in pixels.
pub const MAX_DIM: u32 = 8192;
/// The largest samples-per-pixel count.
pub const MAX_SPP: u32 = 32_768;
/// The largest bounce limit.
pub const MAX_BOUNCES: u32 = 128;
/// The largest number of frames in a tilt video.
pub const MAX_FRAMES: u32 = 18_001;
/// The largest video frame rate.
pub const MAX_FPS: u32 = 120;
/// The number of values in each metric curve (one per degree from -90 to 90).
pub const CURVE_POINTS: usize = 181;

/// A frozen render job: everything needed to render one picture or one video.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderJobFile {
    /// The image-forming engine. Absent when it is the default engine.
    #[serde(
        default,
        skip_serializing_if = "pleochroma::renderer::engine::EngineSpec::is_default"
    )]
    pub engine: pleochroma::renderer::engine::EngineSpec,
    /// The job file format, the one [`format_for`] gives this job.
    pub format: u32,
    /// A random token of 32 lowercase hexadecimal characters, made when the job is added.
    pub token: String,
    /// The one-line name shown in the job list.
    pub label: String,
    /// When the job was added, in unix seconds. Informational.
    pub created_at: i64,
    /// Where the design came from. Informational.
    #[serde(default)]
    pub design: DesignInfo,
    /// The fully resolved scene: material, planes, lighting and camera.
    pub scene: SceneState,
    /// What `scene.backdrop` means: [`BACKDROP_MODEL_LEGACY_CARD`] (0, the default when the
    /// key is missing: a card level of `0.0`, `0.23` or `8.0` that only the camera ray saw)
    /// or [`BACKDROP_MODEL_TRAY`] (1, the tray radiance at exposure 1 of the surface below the
    /// girdle plane). Writers write 1; [`crate::codec::from_text`] converts a 0 once through
    /// [`RenderJobFile::upgrade_backdrop_model`].
    #[serde(default)]
    pub scene_backdrop_model: u32,
    /// The HDR map file, exactly when the scene is lit by one.
    #[serde(default)]
    pub hdr: Option<HdrSource>,
    /// Which computers render, and how their results travel.
    pub compute: JobCompute,
    /// A still: the PNG path. A video: the frame folder. A relative path is taken
    /// against the folder of the job file.
    pub output: String,
    /// What the job renders.
    pub kind: JobKind,
    /// Optional analytical diagnostics, kept outside the remote transport scene.
    ///
    /// Missing means clean output. The job format does not change with it, and the struct
    /// accepts unknown fields, so a reader that predates the field renders the job without
    /// the diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optical_diagnostics: Option<OpticalDiagnosticOptions>,
    /// The stone's color zones. The scene's material cannot carry them (its serde leaves them
    /// out), so they travel beside it. Missing means one color throughout. Written only when
    /// the stone has zones, and then the job is format 5, so a format 4 reader refuses it
    /// instead of rendering the base color alone. A Ray-Traced job never carries them: that
    /// engine renders the base color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoning: Option<ZonedAbsorption>,
}

/// Frozen presentation for the versioned analytical model. Missing means clean output.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OpticalDiagnosticOptions {
    pub model_version: u32,
    pub layers: [bool; 4],
    pub opacity: f32,
    pub map_mode: bool,
    #[serde(default)]
    pub high_contrast: bool,
    /// The scintillation layer, a fifth channel beside `layers`. Missing reads as off, and it
    /// is written only when on. It does not change [`SUPPORTED_OPTICAL_MODEL_VERSION`]: the
    /// four existing layers keep their meaning.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub scintillation: bool,
}

/// Informational facts about the design a job was made from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesignInfo {
    /// The design title.
    pub title: String,
    /// The designer.
    pub designer: String,
    /// The shape name.
    pub shape: String,
    /// The refractive index, as text.
    pub ri: String,
    /// The material name.
    pub material: String,
}

/// An HDR map file and the hash its bytes must have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HdrSource {
    /// The file path. A relative path is taken against the folder of the job file.
    pub path: String,
    /// The SHA-256 of the file bytes, as 64 lowercase hexadecimal characters.
    pub sha256_hex: String,
}

/// The frozen compute choices of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobCompute {
    /// Which computers render.
    pub target: ComputeChoice,
    /// What a remote worker sends back.
    pub transfer: TransferChoice,
    /// With a final-picture transfer: whether this computer renders a share too.
    #[serde(default)]
    pub contribute_local: bool,
}

/// Which computers render a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputeChoice {
    /// This computer only.
    Local,
    /// The remote worker only.
    Remote,
    /// This computer and the remote worker.
    Both,
}

impl ComputeChoice {
    /// The command-line word: `local`, `remote` or `both`.
    #[must_use]
    pub const fn cli_word(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::Both => "both",
        }
    }

    /// Reads a command-line word, ignoring ASCII case.
    #[must_use]
    pub fn from_cli_word(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "local" => Some(Self::Local),
            "remote" => Some(Self::Remote),
            "both" => Some(Self::Both),
            _ => None,
        }
    }
}

/// What a remote worker sends back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferChoice {
    /// The raw sample data, merged on this computer.
    FullData,
    /// The finished picture only.
    FinalPicture,
}

impl TransferChoice {
    /// The command-line word: `full` or `final`.
    #[must_use]
    pub const fn cli_word(self) -> &'static str {
        match self {
            Self::FullData => "full",
            Self::FinalPicture => "final",
        }
    }

    /// Reads a command-line word, ignoring ASCII case.
    #[must_use]
    pub fn from_cli_word(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "full" => Some(Self::FullData),
            "final" => Some(Self::FinalPicture),
            _ => None,
        }
    }
}

/// Which engines of this computer render. A machine setting, never stored in a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LocalEngines {
    /// The processor only.
    Cpu,
    /// The processor and the graphics card.
    #[default]
    CpuGpu,
    /// The graphics card only.
    Gpu,
}

impl LocalEngines {
    /// The command-line word: `cpu`, `cpu+gpu` or `gpu`.
    #[must_use]
    pub const fn cli_word(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::CpuGpu => "cpu+gpu",
            Self::Gpu => "gpu",
        }
    }

    /// Reads a command-line word, ignoring ASCII case. `cpugpu` and `cpu-gpu` are
    /// accepted as aliases of `cpu+gpu`.
    #[must_use]
    pub fn from_cli_word(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "cpu" => Some(Self::Cpu),
            "cpu+gpu" | "cpugpu" | "cpu-gpu" => Some(Self::CpuGpu),
            "gpu" => Some(Self::Gpu),
            _ => None,
        }
    }
}

/// The colour space of the written picture or video frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobColorSpace {
    /// Standard sRGB.
    Srgb,
    /// Display P3.
    DisplayP3,
    /// Rec. 2020.
    Rec2020,
}

impl JobColorSpace {
    /// The name shown to people.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Srgb => "sRGB",
            Self::DisplayP3 => "Display P3",
            Self::Rec2020 => "Rec.2020",
        }
    }
}

/// What a job renders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobKind {
    /// One still picture.
    Still(StillJob),
    /// A tilt performance video.
    TiltVideo(TiltVideoJob),
}

/// The settings of a still picture job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StillJob {
    /// Samples per pixel.
    pub samples_per_pixel: u32,
    /// The colour space of the PNG.
    pub color_space: JobColorSpace,
    /// The lighting preset this picture was added with, if any. Informational.
    #[serde(default)]
    pub preset_label: String,
    /// Optional render accelerations. Missing reads as off; written only when something is on.
    #[serde(default, skip_serializing_if = "RenderAccel::is_off")]
    pub accel: RenderAccel,
}

/// The settings of a tilt video job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TiltVideoJob {
    /// The tilt axis, as the index the tilt section uses. With [`Self::more_axes`], the
    /// first of the video's axes.
    pub axis_index: u32,
    /// Further axes after `axis_index`, in the order the video shows them. Empty for a
    /// one-axis video, a single `start -> end` pass. With any, the video rocks each axis
    /// through face-up in turn, `0 -> end -> start -> 0`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub more_axes: Vec<u32>,
    /// The first tilt angle, in degrees.
    pub start_deg: f64,
    /// The last tilt angle, in degrees.
    pub end_deg: f64,
    /// The angle between frames, in degrees.
    pub step_deg: f64,
    /// The number of frames.
    pub total_frames: u32,
    /// Video frames per second.
    pub fps: u32,
    /// Samples per pixel for each frame.
    pub samples_per_pixel: u32,
    /// The colour space of the frames.
    pub color_space: JobColorSpace,
    /// Which metric overlays are drawn.
    #[serde(default)]
    pub overlay: OverlaySelection,
    /// The frozen metric curves, present when an overlay is selected. With
    /// [`Self::more_axes`], those of the first axis.
    #[serde(default)]
    pub curves: Option<MetricCurvesData>,
    /// The frozen metric curves of [`Self::more_axes`], one per axis in the same order;
    /// present when an overlay is selected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub more_curves: Vec<MetricCurvesData>,
    /// Whether the frames stay on disk after the video is made.
    #[serde(default)]
    pub keep_frames: bool,
    /// The name of the video file, without folder or extension.
    pub video_name: String,
    /// Optional render accelerations for every frame. Missing reads as off; written only when
    /// something is on.
    #[serde(default, skip_serializing_if = "RenderAccel::is_off")]
    pub accel: RenderAccel,
    /// Whether each frame's path cache is written to disk and kept after the video is made.
    /// Written only when on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep_path_cache: bool,
}

/// Which metric overlays a tilt video draws.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the saved job file and the overlay check boxes; the fields are independent switches"
)]
pub struct OverlaySelection {
    /// The brilliance curve.
    pub brilliance: bool,
    /// The windowing curve.
    pub windowing: bool,
    /// The extinction curve.
    pub extinction: bool,
    /// The tilt brilliance read-out.
    pub tilt_brilliance: bool,
    /// The angle read-out.
    pub angle: bool,
}

impl OverlaySelection {
    /// How many overlays are selected.
    #[must_use]
    pub fn count(&self) -> usize {
        [
            self.brilliance,
            self.windowing,
            self.extinction,
            self.tilt_brilliance,
            self.angle,
        ]
        .into_iter()
        .filter(|on| *on)
        .count()
    }
}

impl TiltVideoJob {
    /// Every axis of the video in order: `axis_index`, then `more_axes`.
    #[must_use]
    pub fn axes(&self) -> Vec<u32> {
        std::iter::once(self.axis_index)
            .chain(self.more_axes.iter().copied())
            .collect()
    }
}

/// The three metric curves a tilt video overlay reads, one value per degree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricCurvesData {
    /// The brilliance curve.
    pub brilliance: Vec<f32>,
    /// The windowing curve.
    pub windowing: Vec<f32>,
    /// The extinction curve.
    pub extinction: Vec<f32>,
}

impl RenderJobFile {
    /// The word the database stores as the job kind: `still` or `tilt_video`.
    #[must_use]
    pub const fn kind_word(&self) -> &'static str {
        match self.kind {
            JobKind::Still(_) => "still",
            JobKind::TiltVideo(_) => "tilt_video",
        }
    }

    /// The number of frames the job renders: 1 for a still.
    #[must_use]
    pub const fn frames_total(&self) -> u32 {
        match &self.kind {
            JobKind::Still(_) => 1,
            JobKind::TiltVideo(video) => video.total_frames,
        }
    }

    /// Moves the color zones the scene's material carries into [`Self::zoning`], where they
    /// survive the file. Zones already in [`Self::zoning`] win, and a Ray-Traced job keeps none
    /// (that engine renders the base color), so either way the scene's material ends without
    /// them. Recompute [`Self::format`] with [`format_for`] afterwards.
    pub fn move_scene_zoning_to_file(&mut self) {
        let zoning = self.scene.material.zoning.take();
        if self.zoning.is_none() && !self.engine.is_ray() {
            self.zoning = zoning;
        }
    }

    /// Converts a legacy backdrop card level to the tray radiance (see
    /// [`legacy_backdrop_to_tray`]) and marks the job as [`BACKDROP_MODEL_TRAY`]. Does nothing
    /// to a job that already carries the tray, so it is safe to call on any job.
    pub fn upgrade_backdrop_model(&mut self) {
        if self.scene_backdrop_model == BACKDROP_MODEL_LEGACY_CARD {
            legacy_backdrop_to_tray(&mut self.scene);
            self.scene_backdrop_model = BACKDROP_MODEL_TRAY;
        }
    }

    /// Resolves a path of the job: an absolute path stays, a relative one is joined to
    /// `base_dir`.
    #[must_use]
    pub fn resolve(&self, path: &str, base_dir: &Path) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base_dir.join(path)
        }
    }

    /// The resolved output path (a PNG, or a frame folder).
    #[must_use]
    pub fn output_path(&self, base_dir: &Path) -> PathBuf {
        self.resolve(&self.output, base_dir)
    }

    /// The resolved HDR map path, when the job has one.
    #[must_use]
    pub fn hdr_path(&self, base_dir: &Path) -> Option<PathBuf> {
        self.hdr
            .as_ref()
            .map(|hdr| self.resolve(&hdr.path, base_dir))
    }

    /// Checks every rule a job must satisfy before it can be saved or rendered.
    ///
    /// # Errors
    ///
    /// [`JobFileError::Invalid`] with one plain sentence naming the first broken rule.
    pub fn validate(&self) -> Result<(), JobFileError> {
        // First, so that a hand-edited file with an impossible reflectance is told so rather than
        // that its format does not match (`format_for` reads the reflectance).
        if !(0.0..=1.0).contains(&self.scene.tray_reflection) {
            return Err(invalid("Tray reflection must be between 0 and 100 %."));
        }
        self.validate_common()?;
        if self.scene.stone_shadow && !self.scene.lit_by_studio_backdrop() {
            return Err(invalid(
                "Stone shadow needs a studio light with a backdrop.",
            ));
        }
        if self.scene.tray_reflection > 0.0 && !self.scene.lit_by_studio_backdrop() {
            return Err(invalid(
                "Tray reflection needs a studio light with a backdrop.",
            ));
        }
        match &self.kind {
            JobKind::Still(still) => {
                check_spp(still.samples_per_pixel)?;
                check_accel(&still.accel, still.samples_per_pixel)?;
            }
            JobKind::TiltVideo(video) => validate_video(video)?,
        }
        if self.engine.is_ray() {
            let samples = match &self.kind {
                JobKind::Still(still) => still.samples_per_pixel,
                JobKind::TiltVideo(video) => video.samples_per_pixel,
            };
            if samples != u32::from(self.engine.ray.aa_taps) {
                return Err(invalid(
                    "Ray-Traced samples must equal the anti-aliasing tap count.",
                ));
            }
            if self.optical_diagnostics.is_some() {
                return Err(invalid(
                    "Optical diagnostics require Path-Traced rendering.",
                ));
            }
            let adaptive = match &self.kind {
                JobKind::Still(still) => still.accel.adaptive,
                JobKind::TiltVideo(video) => video.accel.adaptive,
            };
            if adaptive.is_some() {
                return Err(invalid("Adaptive sampling requires Path-Traced rendering."));
            }
        }
        self.validate_hdr()
    }

    fn validate_common(&self) -> Result<(), JobFileError> {
        if let Some(d) = self.optical_diagnostics
            && (d.model_version != SUPPORTED_OPTICAL_MODEL_VERSION
                || !d.opacity.is_finite()
                || !(0.0..=1.0).contains(&d.opacity))
        {
            return Err(invalid(
                "Unsupported optical diagnostic model or invalid opacity",
            ));
        }
        if !(1..=JOB_FORMAT_VERSION).contains(&self.format) || self.format != format_for(self) {
            return Err(invalid(format!(
                "This job file has format {}, but this version reads format {JOB_FORMAT_VERSION}.",
                self.format
            )));
        }
        if self.scene_backdrop_model > BACKDROP_MODEL_TRAY {
            return Err(invalid(format!(
                "This job file has backdrop model {}, but this version reads models 0 and {BACKDROP_MODEL_TRAY}.",
                self.scene_backdrop_model
            )));
        }
        if self.engine.is_ray() && self.engine.ray != self.engine.ray.sanitized() {
            return Err(invalid(
                "Ray-Traced anti-aliasing must be 1, 4, 8 or 16 taps.",
            ));
        }
        if !is_token(&self.token) {
            return Err(invalid(
                "The job token must be 32 lowercase hexadecimal characters.",
            ));
        }
        if self.output.trim().is_empty() {
            return Err(invalid("The job has no output location."));
        }
        let side = MIN_DIM..=MAX_DIM;
        if !side.contains(&self.scene.width) || !side.contains(&self.scene.height) {
            return Err(invalid(format!(
                "The picture size must be between {MIN_DIM} and {MAX_DIM} pixels."
            )));
        }
        if !(1..=MAX_BOUNCES).contains(&self.scene.max_bounces) {
            return Err(invalid(format!(
                "The bounce limit must be between 1 and {MAX_BOUNCES}."
            )));
        }
        self.scene
            .effects
            .validate(self.scene.planes.len())
            .map_err(|message| invalid(format!("The render effects are not usable: {message}")))?;
        if let Some(zoning) = &self.zoning {
            if self.engine.is_ray() {
                return Err(invalid("Color zones require Path-Traced rendering."));
            }
            zoning
                .validate()
                .map_err(|error| invalid(format!("The color zones are not usable: {error}")))?;
            if self.scene.material.absorption_unit != AbsorptionUnit::PerMm {
                return Err(invalid(
                    "The color zones need a material with per-millimeter absorption.",
                ));
            }
        }
        Ok(())
    }

    fn validate_hdr(&self) -> Result<(), JobFileError> {
        match (&self.hdr, self.scene.hdr()) {
            (None, None) => Ok(()),
            (Some(_), None) => Err(invalid(
                "The job names an HDR map file, but its scene is not lit by an HDR map.",
            )),
            (None, Some(_)) => Err(invalid(
                "The scene is lit by an HDR map, but the job does not name the map file.",
            )),
            (Some(source), Some(env)) => {
                if source.sha256_hex == pleochroma_net::messages::hash_hex(&env.content_hash) {
                    Ok(())
                } else {
                    Err(invalid(
                        "The HDR map hash in the job does not match the hash in its scene.",
                    ))
                }
            }
        }
    }
}

fn invalid(message: impl Into<String>) -> JobFileError {
    JobFileError::Invalid(message.into())
}

fn is_token(token: &str) -> bool {
    token.len() == 32
        && token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn check_spp(spp: u32) -> Result<(), JobFileError> {
    if (1..=MAX_SPP).contains(&spp) {
        Ok(())
    } else {
        Err(invalid(format!(
            "The samples per pixel must be between 1 and {MAX_SPP}."
        )))
    }
}

/// Checks the adaptive settings of a job against the job's own sample count, which is their cap,
/// and its beam settings against the range a worker accepts.
fn check_accel(accel: &RenderAccel, spp: u32) -> Result<(), JobFileError> {
    if let Some(adaptive) = accel.adaptive {
        let capped = AdaptiveSettings {
            max_spp: spp,
            ..adaptive
        };
        capped
            .validate()
            .map_err(|error| invalid(format!("Adaptive sampling: {error}.")))?;
    }
    if let Some(beam) = &accel.beam {
        beam.validate()
            .map_err(|error| invalid(format!("Beam tracing: {error}.")))?;
    }
    Ok(())
}

fn validate_video(video: &TiltVideoJob) -> Result<(), JobFileError> {
    check_spp(video.samples_per_pixel)?;
    check_accel(&video.accel, video.samples_per_pixel)?;
    if !(1..=MAX_FPS).contains(&video.fps) {
        return Err(invalid(format!(
            "The frame rate must be between 1 and {MAX_FPS} frames per second."
        )));
    }
    if !(1..=MAX_FRAMES).contains(&video.total_frames) {
        return Err(invalid(format!(
            "A tilt video must have between 1 and {MAX_FRAMES} frames."
        )));
    }
    if !video.start_deg.is_finite() || !video.end_deg.is_finite() {
        return Err(invalid("The tilt start and end angles must be numbers."));
    }
    if !(video.step_deg.is_finite() && video.step_deg > 0.0) {
        return Err(invalid("The tilt step must be a number above zero."));
    }
    let axes = video.axes();
    if axes
        .iter()
        .enumerate()
        .any(|(i, axis)| axes[..i].contains(axis))
    {
        return Err(invalid("A tilt video names the same axis twice."));
    }
    for curves in video.curves.iter().chain(&video.more_curves) {
        let short = [&curves.brilliance, &curves.windowing, &curves.extinction]
            .into_iter()
            .any(|curve| curve.len() < CURVE_POINTS);
        if short {
            return Err(invalid(format!(
                "Each metric curve must hold at least {CURVE_POINTS} values."
            )));
        }
    }
    if video.overlay.count() > 0
        && (video.curves.is_none() || video.more_curves.len() != video.more_axes.len())
    {
        return Err(invalid(
            "The video overlay needs the metric curves of every axis, but the job holds fewer.",
        ));
    }
    Ok(())
}

/// A new random job token: 32 lowercase hexadecimal characters.
///
/// Built from two per-process random hasher keys mixed with the clock, so no random
/// number crate is needed.
#[must_use]
pub fn new_job_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut first = std::collections::hash_map::RandomState::new().build_hasher();
    first.write_u128(nanos);
    let mut second = std::collections::hash_map::RandomState::new().build_hasher();
    second.write_u128(nanos.rotate_left(17) ^ 0x9e37_79b9_7f4a_7c15);
    format!("{:016x}{:016x}", first.finish(), second.finish())
}

/// Test scenes and jobs shared by the tests of this crate.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use pleochroma::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };
    use pleochroma_net::scene::{HdrEnvironment, SceneEnvironment};

    pub const TOKEN: &str = "9f2c4a7d1e0b48c3a65d2f71c8e4b903";

    pub fn scene() -> SceneState {
        SceneState {
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            width: 64,
            height: 48,
            yaw: 0.4,
            camera_roll: 0.0,
            stone_shadow: false,
            tray_reflection: 0.0,
            hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
            estimator: pleochroma::optics::raytracer::Estimator::Branching,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
            backdrop: 0.0,
            environment: SceneEnvironment::Studio,
            surface_glare: 1.0,
            tools: Vec::new(),
            fluorescence: pleochroma::optics::fluorescence::Fluorescence::default(),
            head_shadow_deg: 22.0,
            frosted_planes: Vec::new(),
            effects: pleochroma_net::scene::effects::SceneEffects::default(),
            camera_pan: [0.0, 0.0],
            stone_pose: [0.0, 0.0, 0.0],
        }
    }

    pub fn hdr_scene() -> (SceneState, HdrSource) {
        let mut scene = scene();
        let hash = [7_u8; 32];
        scene.environment = SceneEnvironment::Hdr(HdrEnvironment {
            content_hash: hash,
            width: 64,
            height: 32,
        });
        let source = HdrSource {
            path: "assets/studio.hdr".to_string(),
            sha256_hex: pleochroma_net::messages::hash_hex(&hash),
        };
        (scene, source)
    }

    pub fn still_job() -> RenderJobFile {
        RenderJobFile {
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            optical_diagnostics: None,
            zoning: None,
            format: 1,
            token: TOKEN.to_string(),
            label: "Round · Current view · 64×48".to_string(),
            created_at: 1_791_302_580,
            design: DesignInfo {
                title: "Round".to_string(),
                designer: "J. Doe".to_string(),
                shape: "Round".to_string(),
                ri: "2.42".to_string(),
                material: "Diamond".to_string(),
            },
            scene: scene(),
            scene_backdrop_model: BACKDROP_MODEL_TRAY,
            hdr: None,
            compute: JobCompute {
                target: ComputeChoice::Both,
                transfer: TransferChoice::FullData,
                contribute_local: false,
            },
            output: "C:\\renders\\gem.png".to_string(),
            kind: JobKind::Still(StillJob {
                samples_per_pixel: 64,
                color_space: JobColorSpace::Srgb,
                preset_label: String::new(),
                accel: RenderAccel::default(),
            }),
        }
    }

    pub fn curves() -> MetricCurvesData {
        let curve = |scale: f32| {
            (0..CURVE_POINTS)
                .map(|index| (index as f32).mul_add(scale, 0.1))
                .collect::<Vec<f32>>()
        };
        MetricCurvesData {
            brilliance: curve(0.01),
            windowing: curve(0.02),
            extinction: curve(0.003),
        }
    }

    /// A still lit by an HDR map the job turns: 0.7 radians, following the camera (format 6).
    pub fn turned_hdr_still_job() -> RenderJobFile {
        let (scene, source) = hdr_scene();
        let mut job = still_job();
        job.scene = scene;
        job.hdr = Some(source);
        job.scene.hdr_orientation = pleochroma::renderer::env_map::HdrOrientation {
            yaw: 0.7,
            follows_camera: true,
        };
        job.format = format_for(&job);
        job
    }

    /// A still whose camera puts the stone off-centre (format 7).
    pub fn panned_still_job() -> RenderJobFile {
        let mut job = still_job();
        job.scene.camera_pan = [0.25, -0.4];
        job.format = format_for(&job);
        job
    }

    /// A still whose stone stands in a pose on the tray (format 8).
    pub fn posed_still_job() -> RenderJobFile {
        let mut job = still_job();
        job.scene.stone_pose = [90.0, 30.0, 0.0];
        job.format = format_for(&job);
        job
    }

    /// A still whose stone carries the Ametrine color zones (format 5).
    pub fn zoned_still_job() -> RenderJobFile {
        let mut job = still_job();
        job.scene.material = GemMaterial::diamond();
        job.zoning = Some(pleochroma::optics::materials::ametrine_zoning());
        job.scene.material.absorption_unit = AbsorptionUnit::PerMm;
        job.format = format_for(&job);
        job
    }

    pub fn video_job() -> RenderJobFile {
        let (scene, source) = hdr_scene();
        RenderJobFile {
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            scene,
            hdr: Some(source),
            output: "frames".to_string(),
            kind: JobKind::TiltVideo(TiltVideoJob {
                axis_index: 2,
                more_axes: Vec::new(),
                start_deg: -90.0,
                end_deg: 90.0,
                step_deg: 1.0,
                total_frames: 181,
                fps: 30,
                samples_per_pixel: 8,
                color_space: JobColorSpace::DisplayP3,
                overlay: OverlaySelection {
                    brilliance: true,
                    angle: true,
                    ..OverlaySelection::default()
                },
                curves: Some(curves()),
                more_curves: Vec::new(),
                keep_frames: true,
                video_name: "Round tilt".to_string(),
                accel: RenderAccel::default(),
                keep_path_cache: false,
            }),
            ..still_job()
        }
    }
}

#[cfg(test)]
#[path = "job_tests.rs"]
mod tests;
