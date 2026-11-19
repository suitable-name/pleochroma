//! The parsed command line: one typed argument struct per command, the [`Command`] that holds
//! them and the words the flags take.

#[cfg(feature = "beam")]
use pleochroma::renderer::accel_beam::{BeamSettings, RegionMode};
#[cfg(feature = "adaptive-sampling")]
use pleochroma::renderer::adaptive::{AdaptiveSettings, AdaptiveSettingsError, NoiseMetric};
use pleochroma::{
    color::HighlightRule,
    optics::{LightingPreset, tables::OpticsTables},
    renderer::accel::{CacheCompression, PathCacheSettings},
};
use pleochroma_cut_core::ObjectivePreset;
use pleochroma_editor::retarget::CrownShift;
use pleochroma_formats::native::design::DESIGN_EXTENSION;
use pleochroma_render_jobs::{ComputeChoice, LocalEngines, TransferChoice};
use std::path::{Path, PathBuf};

/// The names `--lighting` takes, and the preset each stands for.
#[cfg(not(feature = "physical-color"))]
pub const LIGHTING_NAMES: [(&str, LightingPreset); 14] = [
    ("daylight", LightingPreset::Daylight),
    ("incandescent", LightingPreset::Incandescent),
    ("ring", LightingPreset::RingLights),
    ("spotlight", LightingPreset::DarkSpotlight),
    ("iso", LightingPreset::IsoHemisphere),
    // An alias of "iso"; `lighting_name` answers "iso" for it.
    ("grading", LightingPreset::IsoHemisphere),
    ("tent", LightingPreset::LightTent),
    ("dome", LightingPreset::DaylightDome),
    ("sun", LightingPreset::DaylightSun),
    ("tray", LightingPreset::WhiteTray),
    ("shop", LightingPreset::ShopLights),
    ("window", LightingPreset::WindowDaylight),
    ("illuminant-a", LightingPreset::IlluminantA),
    ("aset", LightingPreset::Aset),
];

/// The names `--lighting` takes, and the preset each stands for; the `physical-color` build adds
/// the two UV lamps.
#[cfg(feature = "physical-color")]
pub const LIGHTING_NAMES: [(&str, LightingPreset); 16] = [
    ("daylight", LightingPreset::Daylight),
    ("incandescent", LightingPreset::Incandescent),
    ("ring", LightingPreset::RingLights),
    ("spotlight", LightingPreset::DarkSpotlight),
    ("iso", LightingPreset::IsoHemisphere),
    // An alias of "iso"; `lighting_name` answers "iso" for it.
    ("grading", LightingPreset::IsoHemisphere),
    ("tent", LightingPreset::LightTent),
    ("dome", LightingPreset::DaylightDome),
    ("sun", LightingPreset::DaylightSun),
    ("tray", LightingPreset::WhiteTray),
    ("shop", LightingPreset::ShopLights),
    ("window", LightingPreset::WindowDaylight),
    ("illuminant-a", LightingPreset::IlluminantA),
    ("aset", LightingPreset::Aset),
    ("uv365", LightingPreset::UvLamp365),
    ("uv395", LightingPreset::UvLamp395),
];

/// The names `--preset` takes, and the objective each stands for.
pub const PRESET_NAMES: [(&str, ObjectivePreset); 7] = [
    ("balanced", ObjectivePreset::Balanced),
    ("brilliance", ObjectivePreset::Brilliance),
    ("low-windowing", ObjectivePreset::LowWindowing),
    ("low-extinction", ObjectivePreset::LowExtinction),
    ("keep-weight", ObjectivePreset::KeepWeight),
    ("lighten-dark", ObjectivePreset::LightenDark),
    ("intensify-pale", ObjectivePreset::IntensifyPale),
];

/// The word `--lighting` takes for `preset`.
#[must_use]
pub fn lighting_name(preset: LightingPreset) -> &'static str {
    LIGHTING_NAMES
        .iter()
        .find(|(_, known)| *known == preset)
        .map_or("custom", |(name, _)| name)
}

/// The word `--preset` takes for `preset`.
#[must_use]
pub fn preset_name(preset: ObjectivePreset) -> &'static str {
    PRESET_NAMES
        .iter()
        .find(|(_, known)| *known == preset)
        .map_or("custom", |(name, _)| name)
}

/// Which help page to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    /// The command list.
    Root,
    /// `info`.
    Info,
    /// `materials`.
    Materials,
    /// `solve`.
    Solve,
    /// `metrics`.
    Metrics,
    /// `validate`.
    Validate,
    /// `optimize`.
    Optimize,
    /// `retarget`.
    Retarget,
    /// `sweep`.
    Sweep,
    /// `snap-meets`.
    SnapMeets,
    /// `export`.
    Export,
    /// `census`.
    Census,
    /// `render`.
    Render,
    /// `tilt-video`.
    TiltVideo,
    /// `image-compare`.
    ImageCompare,
}

impl Topic {
    /// The topic a command word names; [`Topic::Root`] for anything else.
    #[must_use]
    pub fn of(word: &str) -> Self {
        match word {
            "info" => Self::Info,
            "materials" => Self::Materials,
            "solve" => Self::Solve,
            "metrics" => Self::Metrics,
            "validate" => Self::Validate,
            "optimize" => Self::Optimize,
            "retarget" => Self::Retarget,
            "sweep" => Self::Sweep,
            "snap-meets" => Self::SnapMeets,
            "export" => Self::Export,
            "census" => Self::Census,
            "render" => Self::Render,
            "tilt-video" => Self::TiltVideo,
            "image-compare" => Self::ImageCompare,
            _ => Self::Root,
        }
    }
}

/// A material named on the command line.
#[derive(Debug, Clone, PartialEq)]
pub enum MaterialArg {
    /// `--material NAME`: a built-in preset or a custom material of the `--db` library.
    Named(String),
    /// `--ri N`: a bare refractive index, a flat (non-dispersive) material.
    Ri(f64),
}

/// How a report is printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    /// Aligned text for a person.
    Text,
    /// JSON with sorted keys.
    Json,
    /// One header line and one row of values.
    Csv,
}

/// Which algorithm `retarget` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The critical-angle shift (the dialog's default).
    Shift,
    /// The shift, then a search around it.
    Optimize,
}

/// What `export` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// `GemCAD` cutting instructions, the file the editor's Export writes: CRLF line ends
    /// (the library-entry footnote the desktop adds is left out).
    Asc,
    /// A self-contained `.psdd` design file.
    Psdd,
    /// A Gem Cut Studio file (experimental).
    Gcs,
    /// The cutting sheet as a web page.
    Html,
    /// The cutting sheet as a PDF (the official template, US Letter).
    Pdf,
}

impl ExportFormat {
    /// The format a file name's extension names, if it names one.
    #[must_use]
    pub fn from_file_name(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        let extension = lower.rsplit_once('.').map(|(_, extension)| extension)?;
        match extension {
            "asc" => Some(Self::Asc),
            DESIGN_EXTENSION => Some(Self::Psdd),
            "gcs" => Some(Self::Gcs),
            "html" | "htm" => Some(Self::Html),
            "pdf" => Some(Self::Pdf),
            _ => None,
        }
    }

    /// The extension of the file this format is written as (without the dot).
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Psdd => DESIGN_EXTENSION,
            Self::Gcs => "gcs",
            Self::Html => "html",
            Self::Pdf => "pdf",
        }
    }
}

/// `info`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoArgs {
    /// The design file.
    pub design: PathBuf,
    /// Print JSON instead of text.
    pub json: bool,
    /// Write the report here instead of printing it.
    pub out: Option<PathBuf>,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
}

/// The column `materials --sort` orders the table by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialColumn {
    /// The material's name.
    Name,
    /// The crystal system.
    CrystalSystem,
    /// The optic character.
    OpticCharacter,
    /// The refractive index (`n_D`).
    Ri,
    /// The birefringence.
    Birefringence,
    /// The dispersion, in the scale `--dispersion` names.
    Dispersion,
    /// The specific gravity.
    SpecificGravity,
    /// The Mohs hardness (the low end first).
    Hardness,
    /// The toughness.
    Toughness,
    /// The cleavage, best quality first.
    Cleavage,
}

/// The materials `materials --filter` keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialFilter {
    /// Every material.
    All,
    /// Natural gem materials.
    Natural,
    /// Laboratory-grown materials.
    Synthetic,
    /// The glasses and the glass-ceramic.
    Glass,
    /// The custom materials of the `--db` library.
    Custom,
}

/// The dispersion figure `materials` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialScale {
    /// n(430.8 nm) - n(686.7 nm), the figure gem references print.
    Bg,
    /// n(486.1 nm) - n(656.3 nm), the optical figure the Material Editor takes.
    Fc,
}

/// A table of the manual that `materials --markdown` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkdownTable {
    /// The render-materials table of Appendix C: optical character, `n_D`, birefringence,
    /// dispersion (B-G) and the Abbe number.
    Optics,
    /// The gem data table of Appendix C: Mohs, toughness, cleavage, parting, fracture, SG.
    GemData,
    /// The built-in material list of the materials chapter: `n_D`, birefringence, optical
    /// character, SG and hardness.
    BuiltIn,
    /// The source list of Appendix C: every source the gem data cites, with its address, and
    /// the notes where sources differ.
    Sources,
}

/// How `materials` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialsFormat {
    /// Aligned text for a person.
    Text,
    /// JSON with sorted keys, full citations and addresses.
    Json,
    /// One header line and one row per material.
    Csv,
    /// A table of the manual as Markdown.
    Markdown(MarkdownTable),
}

/// `materials`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialsArgs {
    /// The one material to describe; the table of every material when absent.
    pub name: Option<String>,
    /// How to print.
    pub format: MaterialsFormat,
    /// The column to order the table by; the catalogue order when absent.
    pub sort: Option<MaterialColumn>,
    /// Order the table from the largest to the smallest value.
    pub descending: bool,
    /// Which materials the table lists.
    pub filter: MaterialFilter,
    /// Which dispersion figure the table and the CSV show.
    pub scale: MaterialScale,
    /// With a name: list the absorption bands of every ray too.
    pub bands: bool,
    /// With a name: write the refractive index of every direction against wavelength here, as CSV.
    pub curves: Option<PathBuf>,
    /// Write the report here instead of printing it.
    pub out: Option<PathBuf>,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
}

/// `solve`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolveArgs {
    /// The design file.
    pub design: PathBuf,
    /// Print JSON instead of text.
    pub json: bool,
    /// Save the design here as a `.psdd` file.
    pub out: Option<PathBuf>,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
}

/// `metrics`.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricsArgs {
    /// The design file.
    pub design: PathBuf,
    /// Score in this material instead of the design's own.
    pub material: Option<MaterialArg>,
    /// Also average the tilt performance (about a second and a half).
    pub tilt: bool,
    /// The range of the tilt averages in degrees each way from table up (`--tilt-range`): 15,
    /// 20, 30, 45, 60 or 90. `None` without the flag, which is every tilt to 90 degrees and
    /// leaves the range unnamed in the JSON and the CSV. Only with `tilt`.
    pub tilt_range_deg: Option<u8>,
    /// Also print the Blue share of every spot of the face-up grid that Contrast is computed
    /// from (the text report only).
    pub aset_grid: bool,
    /// Also trace the range of refractive indices the stone works in (`--ri-range`): the grid
    /// table and both bounds. With `--csv` the report is that table instead of the figures row.
    pub ri_range: bool,
    /// How to print.
    pub format: ReportFormat,
    /// The light the score is taken under.
    pub lighting: LightingPreset,
    /// Write the report here instead of printing it.
    pub out: Option<PathBuf>,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
}

/// `validate`.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidateArgs {
    /// The design file.
    pub design: PathBuf,
    /// Print JSON instead of text.
    pub json: bool,
    /// Judge the optics in this material instead of the design's own.
    pub material: Option<MaterialArg>,
    /// The light the optics are measured under.
    pub lighting: LightingPreset,
    /// Write the report here instead of printing it.
    pub out: Option<PathBuf>,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
}

/// `optimize`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimizeArgs {
    /// The design file.
    pub design: PathBuf,
    /// What the search favours.
    pub preset: ObjectivePreset,
    /// Evaluations of the coordinate stage.
    pub budget: usize,
    /// How many starting arrangements the search tries, 1 to 32.
    pub starts: usize,
    /// The search seed.
    pub seed: u64,
    /// Turn pinned (scale-reference) tiers about their girdle edges too.
    pub vary_anchored: bool,
    /// How many ranked candidates to keep.
    pub candidates: usize,
    /// The light the search scores under.
    pub lighting: LightingPreset,
    /// Print JSON instead of text.
    pub json: bool,
    /// Save the best candidate here as a `.psdd` file.
    pub out: Option<PathBuf>,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
}

/// `retarget`.
#[derive(Debug, Clone, PartialEq)]
pub struct RetargetArgs {
    /// The design file.
    pub design: PathBuf,
    /// The material to retarget for.
    pub target: MaterialArg,
    /// Shift alone, or shift and search.
    pub mode: Mode,
    /// How far the crown follows the pavilion's shift.
    pub crown: CrownShift,
    /// What the search favours (`--mode optimize`).
    pub preset: ObjectivePreset,
    /// Degrees either side of each angle the search may move; the dialog's default when absent.
    pub range_deg: Option<f64>,
    /// Evaluations of the search; the dialog's default when absent.
    pub budget: Option<usize>,
    /// The search seed.
    pub seed: u64,
    /// Penalise options that drift from the design's table size and crown-to-pavilion ratio
    /// (`--mode optimize`; `--no-keep-look` turns it off).
    pub keep_look: bool,
    /// Let the girdle band grow up to 5 % and then 10 % thicker, never thinner, when the new
    /// angles would pinch it at its corners (`--girdle-allowance`). Off unless asked for, unlike
    /// the dialog's "Allow a Thicker Girdle (up to +10 %)", which is on by default.
    pub girdle_allowance: bool,
    /// The light everything is scored under.
    pub lighting: LightingPreset,
    /// Print JSON instead of text.
    pub json: bool,
    /// Save the retargeted design here as a `.psdd` file.
    pub out: Option<PathBuf>,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
}

/// `sweep`.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepArgs {
    /// The design file.
    pub design: PathBuf,
    /// The tier to sweep: its name, or `#N` for the Nth row.
    pub tier: String,
    /// One end of the range, in degrees from flat (a sign in front is ignored: the side of
    /// the girdle comes from the tier).
    pub from_deg: f64,
    /// The other end.
    pub to_deg: f64,
    /// The distance between angles.
    pub step_deg: f64,
    /// Also average the tilt performance of every row.
    pub tilt: bool,
    /// Write the rows here as CSV.
    pub csv: Option<PathBuf>,
    /// Print JSON instead of text.
    pub json: bool,
    /// Score in this material instead of the design's own.
    pub material: Option<MaterialArg>,
    /// The light the score is taken under.
    pub lighting: LightingPreset,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
}

/// How far `snap-meets` may move the depth of one tier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SnapLimit {
    /// `--max-move-fraction F`: a share of the finished stone's size (its largest extent).
    /// Without either flag it is `pleochroma_cut_core::DEFAULT_SNAP_MAX_MOVE_FRACTION`.
    Fraction(f64),
    /// `--max-move MM`: millimeters, which only a design with a girdle diameter can turn into
    /// its own units (the command refuses it otherwise).
    Millimeters(f64),
}

/// `snap-meets`.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapMeetsArgs {
    /// The design file.
    pub design: PathBuf,
    /// How far one tier may move.
    pub limit: SnapLimit,
    /// Report what would move and write nothing, even with `out`.
    pub dry_run: bool,
    /// Save the snapped design here as a `.psdd` file. Always given unless `dry_run`.
    pub out: Option<PathBuf>,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
}

/// `export`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportArgs {
    /// The design file.
    pub design: PathBuf,
    /// What to write.
    pub format: ExportFormat,
    /// Where to write it.
    pub out: PathBuf,
    /// A design library, for its custom materials.
    pub db: Option<PathBuf>,
    /// The date the html or pdf cutting sheet prints beside the author (`October 2026`); `None`
    /// prints no date, which keeps the output the same every time.
    pub date: Option<String>,
}

/// The built-in designs a `census` measures.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CensusBuiltins {
    /// The nine built-in templates.
    pub templates: bool,
    /// The built-in fixtures (the concave fixture and the 1.72 brilliant).
    pub fixtures: bool,
}

/// How a `census` writes its report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CensusOutput {
    /// Add the timing columns (they differ from run to run).
    pub timings: bool,
    /// JSON instead of CSV.
    pub json: bool,
}

/// `census`: the read-only measurement of the meet-preservation gate over many designs.
#[derive(Debug, Clone, PartialEq)]
pub struct CensusArgs {
    /// Design files to measure, in the order given.
    pub files: Vec<PathBuf>,
    /// The built-in templates and fixtures to include.
    pub builtins: CensusBuiltins,
    /// The design library to read (opened read-only).
    pub db: Option<PathBuf>,
    /// Every design of the library.
    pub all: bool,
    /// These library entry ids only.
    pub ids: Vec<i64>,
    /// At most this many library designs (after `offset`).
    pub limit: Option<usize>,
    /// Library designs skipped first.
    pub offset: usize,
    /// The materials every design is retargeted to.
    pub targets: Vec<MaterialArg>,
    /// The algorithms run for each design and target, shift before optimize.
    pub modes: Vec<Mode>,
    /// How far the crown follows the pavilion's shift.
    pub crown: CrownShift,
    /// Evaluations of the search of the optimize mode.
    pub budget: usize,
    /// The report format and its optional timing columns.
    pub output: CensusOutput,
    /// Write the report here instead of printing it.
    pub out: Option<PathBuf>,
    /// Write the retargeted `.psdd` of every Valid row into this folder.
    pub out_dir: Option<PathBuf>,
}

/// Which of the two render commands a [`RenderJobArgs`] belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobCommandKind {
    /// `render`: a still picture.
    Render,
    /// `tilt-video`: a tilt performance video.
    TiltVideo,
}

impl JobCommandKind {
    /// The command word, which is also the word progress lines start with.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Render => "render",
            Self::TiltVideo => "tilt-video",
        }
    }
}

/// `render` and `tilt-video`.
#[derive(Debug, Clone, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent command-line switch (--contribute-local, --restart, --quiet, --verbose) mirrored one to one; two-variant enums would only rename them and ripple through every parser and runner"
)]
pub struct RenderJobArgs {
    /// Override the frozen Stone Shadow option (`--stone-shadow on|off`).
    pub stone_shadow: Option<bool>,
    /// Override the frozen Tray Reflection (`--tray-reflection PERCENT`): the tray's head-on
    /// reflectance as a fraction in `0.0..=1.0` (the flag takes percent, `0` switches it off).
    pub tray_reflection: Option<f32>,
    /// Override the frozen Stone Pose (`--stone-pose TILT,TURN[,SPIN]`): tilt, turn and spin in
    /// degrees, as given (the job clamps them into the pose's ranges).
    pub stone_pose: Option<[f32; 3]>,
    /// Override the frozen renderer (`--engine path|ray`).
    pub engine: Option<pleochroma::renderer::engine::RenderEngine>,
    /// Which command this is.
    pub kind: JobCommandKind,
    /// The job file.
    pub job: PathBuf,
    /// `--out` (`render`: the PNG to write) or `--out-dir` (`tilt-video`: the frame folder),
    /// instead of the job's own.
    pub out: Option<PathBuf>,
    /// The engines of this computer that render.
    pub local: LocalEngines,
    /// Replaces the job's compute choice.
    pub compute: Option<ComputeChoice>,
    /// Replaces the job's transfer choice.
    pub transfer: Option<TransferChoice>,
    /// The remote worker's `HOST:PORT`.
    pub remote: Option<String>,
    /// The folder with the remote worker's certificates.
    pub cert_dir: Option<PathBuf>,
    /// With a final-picture transfer: this computer renders a share too.
    pub contribute_local: bool,
    /// `tilt-video` only: start again from the first frame.
    pub restart: bool,
    /// No progress lines.
    pub quiet: bool,
    /// How over-bright colors are shown (`--highlights`); [`HighlightRule::KeepColor`], the rule
    /// every render uses, unless asked.
    pub highlights: HighlightRule,
    /// A built-in material to render the job's stone in instead of its own (`--material`, any
    /// letter case), sized for the job's stone width as the app sizes a design's material.
    pub material: Option<String>,
    /// A built-in color variant of the job's material to render instead of its own color
    /// (`--color-variant`): a full key (`ruby.thai`) or the part after the dot (`thai`).
    pub color_variant: Option<String>,
    /// One of the typical color patterns of the job's material to render the stone in
    /// (`--color-pattern`), built on the material's own color as the Color Pattern window
    /// builds it: its label in any letter case, with or without spaces (`hexagonal-banding`).
    pub color_pattern: Option<String>,
    /// How the job's exposure is read: as written, or metered with `--meter`
    /// ([`Exposure::Metered`]).
    pub exposure: Exposure,
    /// `--raw-out`: write the finished linear image as `PLRAW1` here (`render`: a file;
    /// `tilt-video`: a folder that gets one `frame_NNNN.plraw` per rendered frame).
    pub raw_out: Option<PathBuf>,
    /// `--progress-raw` (`render` only): a folder that gets `spp_NNNNNN.plraw` snapshots of the
    /// linear image at every power of two from 16 samples per pixel on, and the final image.
    pub progress_raw: Option<PathBuf>,
    /// `--sample-offset`: the absolute index of the render's first sample; 0 is an ordinary
    /// render.
    pub sample_offset: u32,
    /// `--adaptive` and its companions: adaptive sampling as asked for on the command line (a
    /// build with the `adaptive-sampling` feature).
    #[cfg(feature = "adaptive-sampling")]
    pub adaptive: AdaptiveArgs,
    /// `--path-cache`, `--persist-cache` and `--cache-dir`: the path cache as asked for on the
    /// command line.
    pub path_cache: PathCacheArgs,
    /// `--optics-tables`: the optics lookup tables (the Fresnel amplitudes and the absorption
    /// exponential read from tables built per material) on every engine of this computer.
    pub optics_tables: OpticsTables,
    /// `--beam` and its overrides: beam tracing as asked for on the command line.
    #[cfg(feature = "beam")]
    pub beam: BeamArgs,
    /// `--verbose`: print the renderer's debug log lines to standard error (in a `beam` build the
    /// beam tracer's per-batch lines too). Off prints nothing extra.
    pub verbose: bool,
}

/// The most samples per pixel `--beam-residual-min-spp` takes.
#[cfg(feature = "beam")]
pub const MAX_BEAM_RESIDUAL_MIN_SPP: u32 = 65_536;

/// The most tiles per batch `--beam-batch-tiles` takes.
#[cfg(feature = "beam")]
pub const MAX_BEAM_BATCH_TILES: u32 = 4096;

/// The fewest and the most spectral nodes `--beam-nodes` takes (a multiple of 8 in between).
#[cfg(feature = "beam")]
pub const BEAM_NODES_RANGE: (u32, u32) = (8, 256);

/// Whether this build can keep beams on disk (`--keep-beams`): the renderer's `path-cache-disk`
/// feature, which the command line always builds with. When it is ever dropped, set this to
/// `false` and the flag is refused with [`keep_beams_refusal`]'s message.
#[cfg(feature = "beam")]
pub const BEAM_PERSISTENCE_AVAILABLE: bool = true;

/// Why `--keep-beams` cannot be honored, or `None` when it can.
///
/// `keep` is whether the flag was given and `persistence` whether the build can store beams
/// ([`BEAM_PERSISTENCE_AVAILABLE`]).
#[cfg(feature = "beam")]
#[must_use]
pub const fn keep_beams_refusal(keep: bool, persistence: bool) -> Option<&'static str> {
    if keep && !persistence {
        Some(
            "--keep-beams stores the beams on disk, and this build has no disk storage for them \
             (it was built without path-cache-disk)",
        )
    } else {
        None
    }
}

/// `--beam`, `--beam-residual-min-spp`, `--beam-batch-tiles`, `--beam-region-mode`,
/// `--beam-nodes` and `--keep-beams`: beam tracing as the command line asks for it.
///
/// An override flag implies `--beam`, as the adaptive companions imply `--adaptive`. The flags
/// replace the job's own `accel.beam` as a whole: values that are not given are the
/// [`BeamSettings`] defaults.
#[cfg(feature = "beam")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BeamArgs {
    /// Beam tracing is on: `--beam`, or any override flag, which implies it.
    pub enabled: bool,
    /// `--keep-beams`: store the beams on disk so a re-render of the same view skips the tree.
    pub keep: bool,
    /// `--beam-residual-min-spp`: the reference samples every stone pixel gets at least.
    pub residual_min_spp: Option<u32>,
    /// `--beam-batch-tiles`: the tiles in one batch.
    pub batch_tiles: Option<u32>,
    /// `--beam-region-mode`: how a kept region is formed across a spectral interval.
    pub region_mode: Option<RegionMode>,
    /// `--beam-nodes`: the spectral nodes, a multiple of 8.
    pub nodes: Option<u32>,
}

#[cfg(feature = "beam")]
impl BeamArgs {
    /// The settings the flags ask for; `None` when no flag turned beam tracing on (the job's own
    /// `accel.beam` then applies).
    #[must_use]
    pub fn settings(&self) -> Option<BeamSettings> {
        if !self.enabled {
            return None;
        }
        let defaults = BeamSettings::default();
        Some(BeamSettings {
            spectral_nodes: self.nodes.unwrap_or(defaults.spectral_nodes),
            residual_min_spp: self.residual_min_spp.unwrap_or(defaults.residual_min_spp),
            region_mode: self.region_mode.unwrap_or(defaults.region_mode),
            batch_tiles: self.batch_tiles.unwrap_or(defaults.batch_tiles),
            keep_leaves: self.keep,
            ..defaults
        })
    }
}

/// `--path-cache`, `--persist-cache` and `--cache-dir`: the path cache as the command line asks
/// for it. The flags override the job's own `accel.path_cache`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathCacheArgs {
    /// The path cache is on: `--path-cache`, or `--persist-cache`, which implies it.
    pub enabled: bool,
    /// `--persist-cache`: keep the cache on disk for re-renders.
    pub persist: bool,
    /// `--cache-dir`: the folder kept caches live in; `None` is the app's `render_cache` folder.
    pub cache_dir: Option<PathBuf>,
}

impl PathCacheArgs {
    /// The settings the flags ask for; `None` when no flag turned the cache on (the job's own
    /// setting then applies).
    #[must_use]
    pub const fn settings(&self) -> Option<PathCacheSettings> {
        if self.enabled {
            Some(PathCacheSettings {
                persist: self.persist,
                compression: CacheCompression::Zstd,
            })
        } else {
            None
        }
    }
}

/// `--adaptive`, `--target-noise`, `--min-spp`, `--noise-metric` and `--gpu-convergence`:
/// adaptive sampling as the command line asks for it (a build with the `adaptive-sampling`
/// feature).
///
/// The values that are not given are filled in from [`AdaptiveSettings::provisional`] once the
/// job's samples per pixel are known ([`Self::settings`]).
#[cfg(feature = "adaptive-sampling")]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AdaptiveArgs {
    /// Adaptive sampling is on: `--adaptive`, or any of the three flags that imply it.
    pub enabled: bool,
    /// `--target-noise`: the error at which a tile stops, in the unit of the metric.
    pub target_noise: Option<f32>,
    /// `--min-spp`: no tile stops before this many samples per pixel.
    pub min_spp: Option<u32>,
    /// `--noise-metric`: how the target is measured.
    pub metric: Option<NoiseMetric>,
    /// `--gpu-convergence`: keep the sums on the GPU and measure the tiles there. Needs
    /// `--adaptive` (the parser refuses it alone) and implies nothing else.
    pub gpu_convergence: bool,
}

#[cfg(feature = "adaptive-sampling")]
impl AdaptiveArgs {
    /// The settings for a job of `max_spp` samples per pixel; `None` when adaptive sampling is
    /// off.
    ///
    /// # Errors
    ///
    /// The message of the rule the combination breaks: a target out of range for the metric, a
    /// minimum that is not a multiple of the 16-sample pass or above `max_spp`. Without
    /// `--target-noise` the metric's own default applies (`NoiseMetric::default_target`: 1 %
    /// relative, Delta E 1 absolute).
    pub fn settings(&self, max_spp: u32) -> Result<Option<AdaptiveSettings>, String> {
        if !self.enabled {
            return Ok(None);
        }
        let metric = self.metric.unwrap_or(NoiseMetric::RelativeLstar);
        let base = AdaptiveSettings::provisional(max_spp);
        let settings = AdaptiveSettings {
            target_noise: self.target_noise.unwrap_or_else(|| metric.default_target()),
            metric,
            min_spp: self.min_spp.unwrap_or(base.min_spp),
            ..base
        };
        settings
            .validate()
            .map_err(|e: AdaptiveSettingsError| format!("adaptive sampling: {e}"))?;
        Ok(Some(settings))
    }
}

/// How `render` and `tilt-video` read the job's exposure.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Exposure {
    /// The job's exposure as it was written (no `--meter`).
    #[default]
    AsJob,
    /// `--meter`: read the job's exposure as compensation on top of the lighting rig's light
    /// meter, as the app meters its live view
    /// (`pleochroma::optics::raytracer::metered_exposure`). A job lit by an HDR map keeps its
    /// exposure.
    Metered,
}

/// `image-compare`: two `PLRAW1` renders of the same scene, compared as a viewer would see them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageCompareArgs {
    /// The reference image (`PLRAW1`).
    pub reference: PathBuf,
    /// The image under test (`PLRAW1`).
    pub run: PathBuf,
    /// A second, independent reference of the same scene; gives the noise floor.
    pub ref2: Option<PathBuf>,
    /// A render job whose color space the images are tone-mapped for.
    pub tonemap_from: Option<PathBuf>,
    /// Write a false-color ΔE map here, as a PNG.
    pub error_png: Option<PathBuf>,
    /// Print JSON instead of text.
    pub json: bool,
    /// Write the report here instead of printing it.
    pub out: Option<PathBuf>,
}

/// How the reports of several designs are put together on standard output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportStyle {
    /// Text: each report under a `==> FILE <==` line, a blank line between them.
    Text,
    /// JSON: the reports one after the other, each a complete document.
    Json,
    /// CSV: one header line, then the rows of every design.
    Csv,
}

impl ReportStyle {
    /// [`ReportStyle::Json`] when `json`, else [`ReportStyle::Text`].
    #[must_use]
    pub const fn json_or_text(json: bool) -> Self {
        if json { Self::Json } else { Self::Text }
    }
}

/// A command line that names several design files: the command for each one, in the order
/// given. A line with one design is the plain command, never a batch.
#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    /// The single-design command of every file. None of them is itself a batch.
    pub items: Vec<Command>,
    /// The folder `--out-dir` names (it must exist when the batch runs).
    pub out_dir: Option<PathBuf>,
    /// How the reports are put together.
    pub style: ReportStyle,
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Several design files, one command each.
    Batch(Batch),
    /// Print a help page.
    Help(Topic),
    /// Print the version.
    Version,
    /// `info`.
    Info(InfoArgs),
    /// `materials`.
    Materials(MaterialsArgs),
    /// `solve`.
    Solve(SolveArgs),
    /// `metrics`.
    Metrics(MetricsArgs),
    /// `validate`.
    Validate(ValidateArgs),
    /// `optimize`.
    Optimize(OptimizeArgs),
    /// `retarget`.
    Retarget(RetargetArgs),
    /// `sweep`.
    Sweep(SweepArgs),
    /// `snap-meets`.
    SnapMeets(SnapMeetsArgs),
    /// `export`.
    Export(ExportArgs),
    /// `census`.
    Census(CensusArgs),
    /// `render`.
    Render(RenderJobArgs),
    /// `tilt-video`.
    TiltVideo(RenderJobArgs),
    /// `image-compare`.
    ImageCompare(ImageCompareArgs),
}

impl Command {
    /// The design file a single-design command works on; `None` for help, the version, the
    /// render commands (they take a job file) and a batch.
    #[must_use]
    pub fn design(&self) -> Option<&Path> {
        match self {
            Self::Info(args) => Some(&args.design),
            Self::Solve(args) => Some(&args.design),
            Self::Metrics(args) => Some(&args.design),
            Self::Validate(args) => Some(&args.design),
            Self::Optimize(args) => Some(&args.design),
            Self::Retarget(args) => Some(&args.design),
            Self::Sweep(args) => Some(&args.design),
            Self::SnapMeets(args) => Some(&args.design),
            Self::Export(args) => Some(&args.design),
            Self::Batch(_)
            | Self::Census(_)
            | Self::Help(_)
            | Self::Materials(_)
            | Self::Version
            | Self::Render(_)
            | Self::TiltVideo(_)
            | Self::ImageCompare(_) => None,
        }
    }
}
