//! The Render & Output tab's persisted and shared values: the output defaults of the still
//! export and the tilt-video export ([`OutputSettings`], `AppSettings::output`), the one
//! bounce ladder, the named sample ranges, and [`RemoteAvailability`].
//!
//! Pure data and logic (no Slint). The UI reads these constants through a model property,
//! so no Slint file keeps its own copy.

use super::remote_endpoint::{ExportTransfer, TiltVideoCompute};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------------
// The one bounce ladder.
// ---------------------------------------------------------------------------------------

/// The bounce-count rungs of every bounce picker (live view, still export, tilt video), in
/// pill order.
pub const BOUNCE_LADDER: [u32; 6] = [4, 8, 12, 24, 64, 128];

/// The index of the rung nearest to `bounces`.
///
/// Exact matches map directly; anything else (such as 16, or a hand-edited value) picks the nearest
/// rung by absolute distance, the lower rung on a tie. It never clamps the stored value itself.
#[must_use]
pub const fn bounce_index(bounces: u32) -> usize {
    let mut best_idx = 0usize;
    let mut best_dist = u32::MAX;
    let mut i = 0usize;
    while i < BOUNCE_LADDER.len() {
        let dist = BOUNCE_LADDER[i].abs_diff(bounces);
        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }
        i += 1;
    }
    best_idx
}

/// The rung value at pill `index`, an out-of-range index limited to the ends (a negative
/// Slint index selects the first rung).
#[must_use]
pub const fn nearest_bounce_cap(index: i32) -> u32 {
    if index <= 0 {
        BOUNCE_LADDER[0]
    } else if index.cast_unsigned() as usize >= BOUNCE_LADDER.len() {
        BOUNCE_LADDER[BOUNCE_LADDER.len() - 1]
    } else {
        BOUNCE_LADDER[index.cast_unsigned() as usize]
    }
}

// ---------------------------------------------------------------------------------------
// Sample ranges, one per purpose.
// ---------------------------------------------------------------------------------------

/// The live view's target samples, as the power-of-two exponent range of the slider:
/// 2^3 = 8 up to 2^10 = 1024.
pub const LIVE_SAMPLES_EXPONENT_RANGE: (u32, u32) = (3, 10);

/// The still export's and tilt video's samples, as the exponent range of their sliders:
/// 2^3 = 8 up to 2^15 = 32768.
pub const EXPORT_SAMPLES_EXPONENT_RANGE: (u32, u32) = (3, 15);

/// The library thumbnails' samples per pixel (`AppSettings::preview_spp`), slider step 32.
pub const PREVIEW_SAMPLES_RANGE: (u32, u32) = (32, 512);

/// The library thumbnails' square size in pixels (`AppSettings::preview_size`), slider step 16.
pub const PREVIEW_SIZE_RANGE: (u32, u32) = (96, 256);

/// `exponent` limited to `range` (an inclusive exponent range from above).
#[must_use]
pub const fn clamp_exponent(exponent: u32, range: (u32, u32)) -> u32 {
    if exponent < range.0 {
        range.0
    } else if exponent > range.1 {
        range.1
    } else {
        exponent
    }
}

// ---------------------------------------------------------------------------------------
// Remote availability.
// ---------------------------------------------------------------------------------------

/// What the app knows about the remote.
///
/// Pickers that only need a target to exist (live compute, the tilt-video pill, job scripts)
/// read [`Self::configured`]. The still export probes before it starts, so it also requires
/// [`Self::reachable_render_capable`]; see [`Self::disabled_reason`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RemoteAvailability {
    /// A remote endpoint is set up in the settings.
    pub configured: bool,
    /// The last probe found the remote reachable AND render-capable (not library-only).
    /// Meaningless (and `false`) while nothing is configured.
    pub reachable_render_capable: bool,
}

impl RemoteAvailability {
    /// No remote at all.
    pub const NONE: Self = Self {
        configured: false,
        reachable_render_capable: false,
    };

    /// A configured remote whose probe has not found it usable (yet).
    #[must_use]
    pub const fn configured_only() -> Self {
        Self {
            configured: true,
            reachable_render_capable: false,
        }
    }

    /// The tooltip for a remote-only choice that is greyed out, `None` when it is offered.
    /// `probed` says whether this surface probes at all (the still export does, the live
    /// picker does not and therefore never blames reachability).
    #[must_use]
    pub const fn disabled_reason(self, probed: bool) -> Option<&'static str> {
        if !self.configured {
            Some("No remote is configured. Add one in the Remote dialog.")
        } else if probed && !self.reachable_render_capable {
            Some(
                "The remote is configured but was not reachable and render-capable when last checked.",
            )
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------------------
// Output defaults.
// ---------------------------------------------------------------------------------------

/// The still export dialog's persisted values: it seeds its controls from them when it
/// opens and writes the values back when an export starts and when the dialog closes ("last
/// used").
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent switches of the dialog, not a state machine"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StillOutput {
    /// Image-forming renderer; missing settings keep the spectral path tracer.
    pub engine: pleochroma::renderer::engine::EngineSpec,
    /// `ExportModel.selected_size_preset`: 0 = 1080p, 1 = 4K, 2 = custom.
    pub size_preset: i32,
    /// Custom width in pixels (used when `size_preset == 2`).
    pub custom_width: u32,
    /// Custom height in pixels.
    pub custom_height: u32,
    /// Samples as a power-of-two exponent, within [`EXPORT_SAMPLES_EXPONENT_RANGE`].
    pub sample_exponent: u32,
    /// Maximum ray bounces (a [`BOUNCE_LADDER`] rung value).
    pub bounces: u32,
    /// `ExportModel.selected_color_space_index`: 0 = sRGB, 1 = Display P3, 2 = Rec.2020.
    pub color_space_index: i32,
    /// Compute pill (local / remote / local + remote). Without a usable remote the export
    /// renders locally whatever this holds.
    pub compute: TiltVideoCompute,
    /// Transfer choice; `None` follows the remote endpoint's own `export_transfer`.
    pub transfer: Option<ExportTransfer>,
    /// "Adaptive Sampling" check box (`ExportModel.adaptive_sampling`).
    pub adaptive_sampling: bool,
    /// "Target Noise" combo (`ExportModel.target_noise_index`): an index into
    /// `pleochroma::renderer::adaptive::TARGET_NOISE_PRESETS` (Fine, Standard, Draft).
    pub target_noise_index: i32,
    /// "Path Cache" check box (`ExportModel.path_cache`).
    pub path_cache: bool,
    /// "Keep for Re-Renders" check box (`ExportModel.keep_path_cache`): write the cache to disk.
    pub keep_path_cache: bool,
    /// "Beam Tracing (Faceted Stones)" check box (`ExportModel.beam`). Read and written in every
    /// build; only a build with the `beam` feature offers the box and acts on it.
    pub beam: bool,
    /// "Keep Beams for Re-Renders" check box (`ExportModel.keep_beams`): store the beams on disk.
    /// Read and written in every build; only a build with the `beam` feature acts on it.
    pub keep_beams: bool,
}

impl Default for StillOutput {
    fn default() -> Self {
        Self {
            engine: pleochroma::renderer::engine::EngineSpec {
                ray: pleochroma::renderer::engine::RayOptions {
                    aa_taps: 16,
                    fire: true,
                },
                ..Default::default()
            },
            size_preset: 0,
            custom_width: 1920,
            custom_height: 1080,
            sample_exponent: 10,
            bounces: BOUNCE_LADDER[2],
            color_space_index: 0,
            compute: TiltVideoCompute::Both,
            transfer: None,
            adaptive_sampling: false,
            target_noise_index: default_target_noise_index(),
            path_cache: false,
            keep_path_cache: false,
            beam: false,
            keep_beams: false,
        }
    }
}

/// The "Target Noise" index a fresh install starts with: Standard.
#[must_use]
pub fn default_target_noise_index() -> i32 {
    i32::try_from(pleochroma::renderer::adaptive::DEFAULT_TARGET_NOISE_PRESET).unwrap_or(0)
}

/// `index` limited to the "Target Noise" choices (a hand-edited value cannot select a fourth).
#[must_use]
pub fn clamp_target_noise_index(index: i32) -> i32 {
    let last =
        i32::try_from(pleochroma::renderer::adaptive::TARGET_NOISE_PRESETS.len() - 1).unwrap_or(0);
    index.clamp(0, last)
}

/// The tilt-video export section's persisted values (same seed-and-write-back rule: a video
/// that starts or is added to the queue, and closing the Tilt Performance dialog, write them).
/// The compute pill is NOT repeated here: it already persists as
/// `AppSettings::tilt_video_compute_target`.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent switches of the section, not a state machine"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TiltVideoOutput {
    /// Image-forming renderer; missing settings keep the spectral path tracer.
    pub engine: pleochroma::renderer::engine::EngineSpec,
    /// `TiltVideoExportModel.resolution_preset` (an index into
    /// `gui::tilt::video_export::params::RESOLUTION_PRESETS`, or the custom index).
    pub resolution_preset: i32,
    /// Custom width in pixels.
    pub custom_width: u32,
    /// Custom height in pixels.
    pub custom_height: u32,
    /// Samples as a power-of-two exponent, within [`EXPORT_SAMPLES_EXPONENT_RANGE`]; lower
    /// than the still default because it is multiplied by the frame count.
    pub sample_exponent: u32,
    /// Maximum ray bounces (a [`BOUNCE_LADDER`] rung value).
    pub bounces: u32,
    /// Colour space index, as for [`StillOutput::color_space_index`].
    pub color_space_index: i32,
    /// Transfer choice; `None` follows the remote endpoint's own `export_transfer`.
    pub transfer: Option<ExportTransfer>,
    /// The output folder-name template (`TiltVideoExportModel.filename_template`).
    pub filename_template: String,
    /// The sweep's Start angle in degrees (`TiltVideoExportModel.start_angle_deg`), -90..=90.
    pub start_angle_deg: i32,
    /// The sweep's End angle in degrees, -90..=90.
    pub end_angle_deg: i32,
    /// The step between frames in hundredths of a degree, within
    /// [`TILT_STEP_HUNDREDTHS_RANGE`] (0.01..=5.0 degrees). Whole numbers keep these settings
    /// `Eq` and read a typed step back exactly.
    pub step_hundredths_deg: u32,
    /// `TiltVideoExportModel.fps_index`: 0 = 24, 1 = 30, 2 = 60 frames per second.
    pub fps_index: i32,
    /// The chosen axis chips, as indices into `PROFILE_AZIMUTHS_DEG`, in the order the video
    /// plays them. Empty means the axis selected in the chart, alone.
    pub axes: Vec<u8>,
    /// "Turn Stone 90°": every frame turned a quarter turn, so a long stone lies the other way
    /// in the picture.
    pub quarter_turn: bool,
    /// "Show Performance Values" on every frame.
    pub show_values: bool,
    /// The brilliance reading among the shown values.
    pub metric_brilliance: bool,
    /// The windowing reading among the shown values.
    pub metric_windowing: bool,
    /// The extinction reading among the shown values.
    pub metric_extinction: bool,
    /// The tilt brilliance score among the shown values.
    pub metric_tilt_brilliance: bool,
    /// The tilt angle among the shown values.
    pub metric_angle: bool,
    /// "Keep PNG Frames After Muxing".
    pub keep_frames: bool,
    /// "Adaptive Sampling" check box (`TiltVideoExportModel.adaptive_sampling`).
    pub adaptive_sampling: bool,
    /// "Target Noise" combo, as [`StillOutput::target_noise_index`].
    pub target_noise_index: i32,
    /// "Path Cache" check box (`TiltVideoExportModel.path_cache`).
    pub path_cache: bool,
    /// "Keep Path Cache for This Video" check box (`TiltVideoExportModel.keep_video_cache`).
    pub keep_video_cache: bool,
    /// "Beam Tracing (Faceted Stones)" check box (`TiltVideoExportModel.beam`). Read and
    /// written in every build; only a build with the `beam` feature acts on it.
    pub beam: bool,
}

/// The tilt video's step field limits in hundredths of a degree (0.01..=5.0 degrees, the
/// field's own `gui::tilt::video_export::params` range).
pub const TILT_STEP_HUNDREDTHS_RANGE: (u32, u32) = (1, 500);

/// The tilt-video folder-name template a fresh install starts with.
pub const DEFAULT_TILT_VIDEO_FILENAME_TEMPLATE: &str =
    "tilt_video_{design}_{material}_axis{axis}_{width}x{height}_{timestamp}";

impl Default for TiltVideoOutput {
    fn default() -> Self {
        Self {
            engine: pleochroma::renderer::engine::EngineSpec {
                ray: pleochroma::renderer::engine::RayOptions {
                    aa_taps: 16,
                    fire: true,
                },
                ..Default::default()
            },
            resolution_preset: 8,
            custom_width: 512,
            custom_height: 512,
            sample_exponent: 6,
            bounces: BOUNCE_LADDER[2],
            color_space_index: 0,
            transfer: None,
            filename_template: DEFAULT_TILT_VIDEO_FILENAME_TEMPLATE.to_string(),
            start_angle_deg: -90,
            end_angle_deg: 90,
            step_hundredths_deg: 100,
            fps_index: 1,
            axes: Vec::new(),
            quarter_turn: false,
            show_values: false,
            metric_brilliance: true,
            metric_windowing: true,
            metric_extinction: true,
            metric_tilt_brilliance: false,
            metric_angle: true,
            keep_frames: false,
            adaptive_sampling: false,
            target_noise_index: default_target_noise_index(),
            path_cache: false,
            keep_video_cache: false,
            beam: false,
        }
    }
}

/// `AppSettings::output`: the persisted output defaults. A settings file without the
/// table loads the defaults above (every struct and field is `serde(default)`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSettings {
    /// The still export's defaults.
    pub still: StillOutput,
    /// The tilt video's defaults.
    pub tilt_video: TiltVideoOutput,
}

impl OutputSettings {
    /// Limits every value to what its control allows, so a hand-edited file cannot seed a
    /// dialog with an impossible slider position.
    #[must_use]
    pub fn sanitised(mut self) -> Self {
        self.still.engine.ray = self.still.engine.ray.sanitized();
        self.tilt_video.engine.ray = self.tilt_video.engine.ray.sanitized();
        let image_dim = |v: u32| v.clamp(16, 32768);
        self.still.size_preset = self.still.size_preset.clamp(0, 2);
        self.still.custom_width = image_dim(self.still.custom_width);
        self.still.custom_height = image_dim(self.still.custom_height);
        self.still.sample_exponent =
            clamp_exponent(self.still.sample_exponent, EXPORT_SAMPLES_EXPONENT_RANGE);
        self.still.bounces = self.still.bounces.max(1);
        self.still.color_space_index = self.still.color_space_index.clamp(0, 2);
        self.still.target_noise_index = clamp_target_noise_index(self.still.target_noise_index);
        self.tilt_video.target_noise_index =
            clamp_target_noise_index(self.tilt_video.target_noise_index);

        self.tilt_video.resolution_preset = self.tilt_video.resolution_preset.max(0);
        self.tilt_video.custom_width = self.tilt_video.custom_width.clamp(128, 8192);
        self.tilt_video.custom_height = self.tilt_video.custom_height.clamp(128, 8192);
        self.tilt_video.sample_exponent = clamp_exponent(
            self.tilt_video.sample_exponent,
            EXPORT_SAMPLES_EXPONENT_RANGE,
        );
        self.tilt_video.bounces = self.tilt_video.bounces.max(1);
        self.tilt_video.color_space_index = self.tilt_video.color_space_index.clamp(0, 2);
        if self.tilt_video.filename_template.trim().is_empty() {
            self.tilt_video.filename_template = DEFAULT_TILT_VIDEO_FILENAME_TEMPLATE.to_string();
        }
        self.tilt_video.start_angle_deg = self.tilt_video.start_angle_deg.clamp(-90, 90);
        self.tilt_video.end_angle_deg = self.tilt_video.end_angle_deg.clamp(-90, 90);
        self.tilt_video.step_hundredths_deg = self
            .tilt_video
            .step_hundredths_deg
            .clamp(TILT_STEP_HUNDREDTHS_RANGE.0, TILT_STEP_HUNDREDTHS_RANGE.1);
        self.tilt_video.fps_index = self.tilt_video.fps_index.clamp(0, 2);
        let axis_count = pleochroma::color::metrics::PROFILE_AZIMUTHS_DEG.len();
        self.tilt_video
            .axes
            .retain(|&axis| usize::from(axis) < axis_count);
        self.tilt_video.axes.sort_unstable();
        self.tilt_video.axes.dedup();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_round_trips_and_unknown_values_pick_the_nearest_rung() {
        for (i, rung) in BOUNCE_LADDER.iter().enumerate() {
            assert_eq!(bounce_index(*rung), i);
            assert_eq!(
                nearest_bounce_cap(i32::try_from(i).expect("ladder index fits i32")),
                *rung
            );
        }
        assert_eq!(bounce_index(16), 2);
        assert_eq!(bounce_index(1), 0);
        assert_eq!(bounce_index(999), 5);
        assert_eq!(bounce_index(44), 3);
        assert_eq!(nearest_bounce_cap(-3), 4);
        assert_eq!(nearest_bounce_cap(40), 128);
    }

    #[test]
    fn remote_availability_disabled_reasons() {
        let up = RemoteAvailability {
            configured: true,
            reachable_render_capable: true,
        };
        assert_eq!(up.disabled_reason(true), None);
        assert!(RemoteAvailability::NONE.disabled_reason(false).is_some());
        // A surface that does not probe never blames reachability; one that probes does.
        assert_eq!(
            RemoteAvailability::configured_only().disabled_reason(false),
            None
        );
        assert!(
            RemoteAvailability::configured_only()
                .disabled_reason(true)
                .is_some()
        );
    }

    /// The sweep, the axes, the 90° turn and the frame toggles come back from the file, and a
    /// hand-edited file cannot seed an angle, step, frame rate or axis the section lacks.
    #[test]
    fn the_tilt_video_sweep_round_trips_and_is_limited_to_what_the_section_allows() {
        use crate::settings::model::SettingsFile;
        let mut file = SettingsFile::default();
        file.settings.output.tilt_video = TiltVideoOutput {
            start_angle_deg: -30,
            end_angle_deg: 45,
            step_hundredths_deg: 25,
            fps_index: 2,
            axes: vec![1, 3],
            quarter_turn: true,
            show_values: true,
            metric_angle: false,
            keep_frames: true,
            ..TiltVideoOutput::default()
        };
        let text = toml::to_string(&file).expect("serialise");
        let parsed: SettingsFile = toml::from_str(&text).expect("parse");
        assert_eq!(
            parsed.settings.output.tilt_video,
            file.settings.output.tilt_video
        );

        let odd = OutputSettings {
            tilt_video: TiltVideoOutput {
                start_angle_deg: -200,
                end_angle_deg: 400,
                step_hundredths_deg: 0,
                fps_index: 9,
                axes: vec![3, 7, 1, 3],
                ..TiltVideoOutput::default()
            },
            ..OutputSettings::default()
        }
        .sanitised()
        .tilt_video;
        assert_eq!((odd.start_angle_deg, odd.end_angle_deg), (-90, 90));
        assert_eq!(odd.step_hundredths_deg, TILT_STEP_HUNDREDTHS_RANGE.0);
        assert_eq!(odd.fps_index, 2);
        assert_eq!(odd.axes, vec![1, 3]);
    }

    #[test]
    fn a_settings_file_without_output_loads_the_defaults() {
        use crate::settings::model::SettingsFile;
        let parsed: SettingsFile = toml::from_str("[settings]\nexposure = 1.2\n").expect("parse");
        assert_eq!(parsed.settings.output, OutputSettings::default());
        let partial: SettingsFile =
            toml::from_str("[settings.output.still]\nsample_exponent = 12\n").expect("parse");
        assert_eq!(partial.settings.output.still.sample_exponent, 12);
        assert_eq!(
            partial.settings.output.tilt_video,
            TiltVideoOutput::default()
        );
    }

    #[test]
    fn output_values_are_limited_to_their_controls() {
        let mut wild = OutputSettings::default();
        wild.still.sample_exponent = 99;
        wild.tilt_video.custom_width = 1;
        wild.tilt_video.filename_template = "  ".to_string();
        let tame = wild.sanitised();
        assert_eq!(tame.still.sample_exponent, 15);
        assert_eq!(tame.tilt_video.custom_width, 128);
        assert_eq!(
            tame.tilt_video.filename_template,
            DEFAULT_TILT_VIDEO_FILENAME_TEMPLATE
        );
    }
}
