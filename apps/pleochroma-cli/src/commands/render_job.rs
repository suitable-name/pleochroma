//! `render` and `tilt-video`: render a job file written by the desktop app's render queue.
//!
//! The job is rendered by the very same executor the app runs
//! (`pleochroma_cut::gui::render_jobs::headless::run_job`), so a picture made from a script
//! is the one the app would have made, and a remote worker is used through the app's own
//! client. These two commands are the exception to "a command never prints or writes": they
//! write the picture or the frames themselves, and stream their progress to the writer they
//! are given (standard error for [`crate::run`], a buffer for [`crate::run_command_line`]).
//!
//! # Output
//!
//! Standard output is one line: the path written (the PNG; the MP4 or GIF; or the frame
//! folder when no video could be made). The rest goes through the progress writer:
//!
//! | Where | What |
//! |---|---|
//! | terminal | one line rewritten in place, and a line feed at the end |
//! | not a terminal | progress lines, as described below |
//! | always | `note: ...` lines, each distinct note once |
//!
//! Outside a terminal, a still prints a line each time the percentage crosses a multiple of 10,
//! and at least one a minute while an estimate exists; a video prints a line per finished frame.
//!
//! `--quiet` turns the progress lines off, never the notes.
//!
//! # Benchmark outputs
//!
//! `--raw-out` writes the finished linear image (the mean CIE XYZ of every pixel before the
//! tone map, and its sample count) as a `PLRAW1` file; `--progress-raw` also writes snapshots
//! at every power of two from 16 samples per pixel on; `--sample-offset` starts the render at
//! another absolute sample index, so a second render is independent of the first. None of them
//! changes the picture: without the flags a render is what it always was.
//!
//! # Adaptive sampling (a build with the `adaptive-sampling` feature)
//!
//! In a build with the optional `adaptive-sampling` feature, `--adaptive` (and `--target-noise`,
//! `--min-spp` and `--noise-metric`, each of which implies it) turns the job's samples per pixel
//! into a cap: a tile of 16 x 16 pixels stops once it is clean enough, so the picture needs fewer
//! samples. The settings are `AdaptiveSettings::provisional` for the job's sample count with the
//! flags laid over them. The render stays on this computer and on the path-traced engine, and
//! says how many samples it saved in a note. It makes another picture, so a tilt video's frames
//! get a token of their own, like the flags below. Without the feature the flags do not exist, and
//! a job's own `accel.adaptive` renders at the job's samples per pixel, without adaptive
//! sampling, with the note "Adaptive sampling off: this build has no adaptive sampling.".
//!
//! # Path cache
//!
//! `--path-cache` shades the stone from a cache of its light paths (guide section 8): the paths
//! are worked out once per pixel and every sample only looks the light up. The cache is used
//! only when it is kept (`placement::ONE_SHOT_NOTE` says so otherwise). `--persist-cache`
//! (which implies it) keeps the cache under `--cache-dir` (default: the app's `render_cache`
//! folder) so that a later render of the same view loads it. The flags win over the job's own
//! `accel.path_cache`; without them the job's setting applies (a tilt video persists only when
//! the job says `keep_path_cache`). A stone the cache cannot serve renders as without it and a
//! note says why. The picture is statistically the same, so the token and the frame folder do not
//! change.
//!
//! # Beam tracing (a build with the `beam` feature)
//!
//! In a build with the `beam` feature (off by default), `--beam` traces the stone's facets as
//! beams and samples only the residual. Its overrides (`--beam-residual-min-spp`,
//! `--beam-batch-tiles`, `--beam-region-mode`, `--beam-nodes`) imply it. The flags replace the
//! job's own `accel.beam`; without them the job's setting applies. A stone the beams cannot serve
//! renders as without them and a note says why. The beam picture is another picture, so the
//! flags change the job token and the frame folder of a tilt video. Without the feature the
//! flags do not exist, and a job's own `accel.beam` renders without beams with the note "Beam
//! tracing off: this build has no beam tracer.".
//!
//! # Optics tables
//!
//! `--optics-tables` sets the process default of the optics lookup tables
//! (`pleochroma::optics::tables::set_default_optics_tables`) before the first frame is prepared,
//! and puts the previous default back afterwards. The processor reads the Fresnel amplitudes and
//! the absorption exponential from the tables, and the graphics card compiles the table variant
//! of its kernels, so a hybrid render uses them on both engines. A remote worker renders without
//! the tables (the job format carries no switch for it), and a note says so. The tables are
//! accurate to about 2e-5, not bit for bit, so the flag gives a tilt video a token of its own.
//!
//! # Estimator
//!
//! A run renders with exit collection, the estimator of the apps (`Estimator::APP_DEFAULT`),
//! which follows every light path to all its exits. The estimator is the process default
//! (`pleochroma::optics::raytracer::set_default_estimator`) from before the first frame is
//! prepared until the run ends. `--path-cache`, `--persist-cache` and any `--beam*` flag (in a
//! `beam` build) need the branching estimator, the coin-flip one: they make that run branching,
//! and one note says so. The processor tracer and this computer's GPU both implement exit
//! collection, and a remote worker renders with the estimator the scene carries, so the run
//! works with every `--local` and `--compute` choice. A stone with concave cuts or a ground or
//! rough surface renders with branching on every engine, so a frame shared between the
//! processor and the GPU never mixes the two estimators. Under exit collection the executor
//! turns off a job's own path cache and beams, each with a note. In a build with the
//! `adaptive-sampling` feature, `--adaptive` with no `--min-spp` starts from a minimum of 64
//! samples per pixel instead of 512 whenever the run renders with exit collection (rounded down
//! to a multiple of the pass size, and at most the samples per pixel), because the error of exit
//! collection is concentrated in few pixels. When the run's estimator differs from the one the
//! job file recorded, the executor gives the job a token of its own
//! (`changed_token(token, "estimator=<word>")`), so a tilt video never mixes frames made with
//! the two: the folder of the other estimator is refused, and `--restart` renders them again.
//! The process-wide settings (the estimator, the highlight rule and the optics tables) are set
//! while one lock is held, so two renders in one process never see each other's.
//!
//! # Changing the picture
//!
//! `--material NAME` renders the job's stone in another built-in material, at the job's stone
//! width; `--color-variant KEY` renders the job's material (after `--material`, the new one) in
//! one of its built-in color variants, at the job's stone width; `--color-pattern NAME` renders
//! the stone in one of its material's typical color patterns. `--highlights whiten` shows
//! over-bright colors with `pleochroma::color::HighlightRule::Whiten` instead of the default
//! rule (process-wide while the job runs); `--stone-shadow on|off` sets the job's Stone Shadow
//! (format 3 with it); `--tray-reflection PERCENT` sets the job's Tray Reflection (format 4 with
//! it, 5 with color zones, 6 with a turned HDR map); `--meter` reads the job's exposure as the
//! compensation on top of the rig's light meter, as the app meters its live view. Each one gives
//! the job a token of its own, so a tilt video never mixes frames made with and without it.

mod raw_sink;

use crate::{
    args::{Exposure, JobCommandKind, RenderJobArgs},
    materials::Catalogue,
    outcome::{CliError, CommandResult, EXIT_DESIGN, EXIT_IO, EXIT_RENDER, Outcome},
};
#[cfg(feature = "beam")]
use pleochroma::renderer::accel_beam::{BeamOff, BeamSettings, RegionMode};
#[cfg(feature = "adaptive-sampling")]
use pleochroma::renderer::adaptive::{AdaptiveSettings, NoiseMetric};
use pleochroma::{
    color::{HighlightRule, highlight_rule, set_highlight_rule},
    optics::{
        inclusions::spec::{DEFAULT_DENSITY, InclusionSpec},
        materials::{
            MaterialLook,
            variants::{built_in_variants, material_slug},
        },
        raytracer::{Estimator, default_estimator, set_default_estimator},
        tables::{OpticsTables, default_optics_tables, set_default_optics_tables},
    },
    render_setup::{
        MaterialOverrides, apply_material_overrides, measure_model_width, stone_width_for_scale,
    },
};
use pleochroma_cut::gui::render_jobs::headless::{
    HeadlessOptions, RawTapSink, RemoteEndpoint, RenderTaps, changed_token, gpu_compiled_in,
    material_with_typical_pattern, run_job,
};
use pleochroma_render_jobs::{
    ComputeChoice, FailureKind, JobKind, JobOutcome, JobProgress, JobSink, LocalEngines,
    RenderJobFile, TransferChoice, codec, run::format_progress_line,
};
use raw_sink::{RawFileSink, RawTarget};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError, atomic::AtomicBool},
    time::{Duration, Instant},
};

/// Held for a whole render: the process-wide settings a render reads (the stone estimator, the
/// highlight rule, the optics tables) are set and put back under it, so two renders in one
/// process (the tests run in parallel) never see each other's.
static PROCESS_SETTINGS: Mutex<()> = Mutex::new(());

/// The longest a plain still stays silent while an estimate exists.
const PLAIN_LINE_INTERVAL: Duration = Duration::from_secs(60);

/// Streams progress lines and notes to a writer, in the layout `render` and `tilt-video` use.
struct StreamSink<'a> {
    out: &'a mut dyn Write,
    word: &'static str,
    terminal: bool,
    quiet: bool,
    /// The width of the line a terminal is showing now, 0 when no line is open.
    open_len: usize,
    /// The tens of percent a plain still has already reported.
    last_decile: u32,
    /// When a plain still printed its latest line.
    last_line: Option<Instant>,
    /// The latest estimate, which a plain video line repeats.
    last_eta: Option<f64>,
    notes: Vec<String>,
}

impl<'a> StreamSink<'a> {
    fn new(out: &'a mut dyn Write, word: &'static str, terminal: bool, quiet: bool) -> Self {
        Self {
            out,
            word,
            terminal,
            quiet,
            open_len: 0,
            last_decile: 0,
            last_line: None,
            last_eta: None,
            notes: Vec::new(),
        }
    }

    /// Ends the line a terminal is rewriting, so the next text starts on a fresh one.
    fn end_line(&mut self) {
        if self.open_len > 0 {
            let _ = writeln!(self.out);
            self.open_len = 0;
        }
    }

    /// Prints `note: {note}` on its own line, unless this note was printed already.
    fn note(&mut self, note: &str) {
        if self.notes.iter().any(|seen| seen == note) {
            return;
        }
        self.end_line();
        let _ = writeln!(self.out, "note: {note}");
        self.notes.push(note.to_string());
    }

    /// Ends the open line and flushes.
    fn finish(&mut self) {
        self.end_line();
        let _ = self.out.flush();
    }
}

impl JobSink for StreamSink<'_> {
    fn progress(&mut self, progress: &JobProgress) {
        if let Some(note) = &progress.note {
            self.note(note);
        }
        self.last_eta = progress.eta_secs;
        if self.quiet {
            return;
        }
        if self.terminal {
            let line = format_progress_line(self.word, progress, true);
            let width = self.open_len;
            let _ = write!(self.out, "\r{line:<width$}");
            let _ = self.out.flush();
            self.open_len = line.chars().count();
        } else if progress.frames_total <= 1 {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "fraction is clamped to 0..=1, so the rounded percent is in 0..=100"
            )]
            let percent = (progress.fraction.clamp(0.0, 1.0) * 100.0).round() as u32;
            let decile = percent / 10;
            let due = self
                .last_line
                .is_none_or(|at| at.elapsed() >= PLAIN_LINE_INTERVAL);
            if decile > self.last_decile || (due && progress.eta_secs.is_some() && percent > 0) {
                self.last_decile = decile.max(self.last_decile);
                self.last_line = Some(Instant::now());
                let _ = writeln!(
                    self.out,
                    "{}",
                    format_progress_line(self.word, progress, false)
                );
            }
        }
    }

    fn frame_finished(&mut self, frames_done: u32, frames_total: u32) {
        if self.quiet || self.terminal || frames_total <= 1 {
            return;
        }
        let progress = JobProgress {
            frames_done,
            frames_total,
            fraction: frames_done as f32 / frames_total as f32,
            eta_secs: self.last_eta,
            note: None,
            beam_phase: None,
        };
        let _ = writeln!(
            self.out,
            "{}",
            format_progress_line(self.word, &progress, false)
        );
    }
}

/// Refuses a job file of the other kind, naming the command that runs it.
fn check_kind(kind: JobCommandKind, job: &RenderJobFile) -> Result<(), CliError> {
    match (kind, &job.kind) {
        (JobCommandKind::Render, JobKind::TiltVideo(_)) => Err(CliError::usage(
            "This job file holds a tilt video; run it with: pleochroma-cli tilt-video FILE",
        )),
        (JobCommandKind::TiltVideo, JobKind::Still(_)) => Err(CliError::usage(
            "This job file holds a still picture; run it with: pleochroma-cli render FILE",
        )),
        _ => Ok(()),
    }
}

/// The folder of the job file: relative paths in the job resolve against it.
fn job_folder(job: &Path) -> PathBuf {
    job.parent()
        .filter(|folder| !folder.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Replaces the job's material with the built-in material `name` (any letter case), at the same
/// stone width.
///
/// The width comes back from the job material's own scale and the design's measured width
/// ([`stone_width_for_scale`]); the new material is then sized as the app sizes a design's
/// material ([`apply_material_overrides`]), with its own crystal axis and no inclusions or edge
/// rounding. The job's design label names the new material too.
///
/// A material whose look is a design effect ([`MaterialLook`]: precious opal's play-of-color,
/// labradorite's schiller, moonstone's adularescence, white opal's milky body, labradorite's
/// translucent one) gets that effect in the job's scene when the job has none of it, as choosing
/// it in the Stone tab does.
///
/// # Errors
///
/// [`CliError::usage`] when `name` is not a built-in material (the message lists them),
/// [`CliError::design`] when the stone's width cannot be measured.
fn apply_material(job: &mut RenderJobFile, name: &str) -> Result<(), CliError> {
    let catalogue = Catalogue::open(None, None)?;
    let Some(material) = catalogue.lookup_exact(name.trim()) else {
        return Err(CliError::usage(format!(
            "--material: {name:?} is not a built-in material. Available: {}",
            catalogue.names().join(", ")
        )));
    };
    let model_width = measure_model_width(&job.scene.planes);
    let old = &job.scene.material;
    let stone_width_mm =
        stone_width_for_scale(old.absorption_unit, old.absorption_path_scale, model_width)
            .ok_or_else(|| {
                CliError::design(format!(
                    "cannot size {} for the job's stone: its width cannot be measured",
                    material.name
                ))
            })?;
    let look = MaterialLook::of(&material.name);
    let overrides = MaterialOverrides {
        inclusion_sigma_s: look.haze_sigma_s.unwrap_or(0.0),
        c_axis_override: None,
        alpha_axis_override: None,
        edge_rounding_radius: 0.0,
        stone_width_mm,
    };
    job.design.material.clone_from(&material.name);
    job.scene.material = apply_material_overrides(material, &overrides, model_width);
    // The file's color zones belong to the old stone.
    job.zoning = None;
    job.format = pleochroma_render_jobs::job::format_for(job);
    let effects = &mut job.scene.effects;
    if effects.structure.is_none() {
        effects.structure = look.structure;
    }
    if effects.inclusions.is_none()
        && let Some(preset) = look.inclusion_preset
    {
        effects.inclusions = Some(InclusionSpec {
            preset: preset.to_owned(),
            density: DEFAULT_DENSITY,
            turn_deg: None,
            sheen: None,
            color: None,
        });
    }
    Ok(())
}

/// Replaces the color of the job's material with its built-in color variant `key` (a full key
/// such as `ruby.thai`, or the part after the dot), at the same stone width. After
/// `--material` the job's material is the new one, so its variants are the ones offered.
///
/// The job's material is already sized for its stone, so the variant is resized from the
/// material's own scale and the design's measured width
/// ([`pleochroma::optics::materials::GemMaterial::with_color_variant_resized`]).
///
/// # Errors
///
/// [`CliError::usage`] when the job's material is not a built-in one or has no variant `key`
/// (the message lists its keys), [`CliError::design`] when the stone's width cannot be measured.
fn apply_color_variant(job: &mut RenderJobFile, key: &str) -> Result<(), CliError> {
    let name = job.scene.material.name.clone();
    let variants = built_in_variants(&name);
    if variants.is_empty() {
        return Err(CliError::usage(format!(
            "--color-variant: the job's material {name:?} is not a built-in material, so it \
             has no built-in color variants"
        )));
    }
    let wanted = key.trim().to_ascii_lowercase();
    let qualified = format!("{}.{wanted}", material_slug(&name));
    let Some(variant) = variants
        .iter()
        .find(|variant| variant.key == wanted || variant.key == qualified)
    else {
        let keys: Vec<&str> = variants
            .iter()
            .map(|variant| variant.key.as_str())
            .collect();
        return Err(CliError::usage(format!(
            "{name} has no color variant {key:?}; its variants are {}",
            keys.join(", ")
        )));
    };
    let model_width = measure_model_width(&job.scene.planes);
    let material = job
        .scene
        .material
        .clone()
        .with_color_variant_resized(variant, model_width)
        .ok_or_else(|| {
            CliError::design(format!(
                "cannot size color variant {} for the job's stone: its width cannot be measured",
                variant.key
            ))
        })?;
    job.scene.material = material;
    // The file's color zones belong to the old color.
    job.zoning = None;
    job.format = pleochroma_render_jobs::job::format_for(job);
    Ok(())
}

/// `--color-pattern`: the job's stone in the typical color pattern `label` of its material (after
/// `--material`, the new one), built and solved as the Color Pattern window does it
/// ([`material_with_typical_pattern`]).
///
/// # Errors
///
/// [`CliError::usage`] when a color variant was asked for too (the pattern is built on the
/// material's own color), the material has no such pattern (the message lists its patterns) or a
/// color cannot be solved.
fn apply_color_pattern(
    job: &mut RenderJobFile,
    label: &str,
    with_variant: bool,
) -> Result<(), CliError> {
    if with_variant {
        return Err(CliError::usage(
            "--color-pattern builds on the material's own color: give it without --color-variant",
        ));
    }
    job.scene.material =
        material_with_typical_pattern(&job.scene.material, &job.scene.planes, label)
            .map_err(CliError::usage)?;
    // The file's color zones belong to the old color.
    job.zoning = None;
    job.format = pleochroma_render_jobs::job::format_for(job);
    Ok(())
}

/// `--meter`: the job's exposure becomes the metered one, read as the compensation on top of
/// the lighting rig's light meter at the job's light pose, the exposure the app gives a live
/// view of the same lighting. A job lit by an HDR map keeps its exposure.
fn apply_meter(job: &mut RenderJobFile) {
    if job.hdr.is_some() {
        return;
    }
    let scene = &mut job.scene;
    scene.exposure = pleochroma::optics::raytracer::metered_exposure(
        scene.exposure,
        scene.lighting_preset,
        scene.light_yaw,
        scene.light_pitch,
    );
}

/// Refuses a `--highlights` other than the default when the picture would come back finished
/// from the remote worker, which tone-maps it under its own (the default) rule.
fn check_highlights_reach_the_picture(
    args: &RenderJobArgs,
    job: &RenderJobFile,
    has_remote: bool,
) -> Result<(), CliError> {
    let compute = args.compute.unwrap_or(job.compute.target);
    let transfer = args.transfer.unwrap_or(job.compute.transfer);
    if args.highlights != HighlightRule::default()
        && has_remote
        && compute != ComputeChoice::Local
        && transfer == TransferChoice::FinalPicture
    {
        return Err(CliError::usage(format!(
            "--highlights {} needs the raw sample data here: a finished picture from the remote \
             worker is tone-mapped there under its own rule; add --transfer full",
            args.highlights.cli_word()
        )));
    }
    Ok(())
}

/// The samples per pixel the job asks for.
const fn job_samples_per_pixel(job: &RenderJobFile) -> u32 {
    match &job.kind {
        JobKind::Still(still) => still.samples_per_pixel,
        JobKind::TiltVideo(video) => video.samples_per_pixel,
    }
}

/// Refuses `--raw-out`, `--progress-raw` and `--sample-offset` where the render cannot deliver
/// them, rather than ignoring them:
///
/// - a finished picture from a remote worker (`--transfer final`) is tone-mapped there and
///   rendered from sample 0, so none of the three can apply to it;
/// - `--progress-raw` snapshots this computer's own accumulation while it builds up, which a
///   remote worker's share does not pass through;
/// - the Ray Traced engine has no random samples to start later;
/// - the offset plus the samples per pixel must fit the sample index.
fn check_benchmark_outputs(
    args: &RenderJobArgs,
    job: &RenderJobFile,
    has_remote: bool,
) -> Result<(), CliError> {
    let mut flags = Vec::new();
    if args.raw_out.is_some() {
        flags.push("--raw-out");
    }
    if args.progress_raw.is_some() {
        flags.push("--progress-raw");
    }
    if args.sample_offset != 0 {
        flags.push("--sample-offset");
    }
    if flags.is_empty() {
        return Ok(());
    }
    let compute = args.compute.unwrap_or(job.compute.target);
    let transfer = args.transfer.unwrap_or(job.compute.transfer);
    let remote_lane = has_remote && compute != ComputeChoice::Local;
    if remote_lane && transfer == TransferChoice::FinalPicture {
        return Err(CliError::usage(format!(
            "{} need the raw sample data here: a finished picture from the remote worker is \
             tone-mapped there and starts at sample 0; add --transfer full",
            flags.join(", ")
        )));
    }
    if args.progress_raw.is_some() && remote_lane {
        return Err(CliError::usage(
            "--progress-raw snapshots the picture while this computer builds it up and cannot \
             follow a remote worker's share: use --compute local",
        ));
    }
    if args.sample_offset != 0 && job.engine.is_ray() {
        return Err(CliError::usage(
            "--sample-offset is for the path-traced renderer: the Ray Traced engine's taps are \
             a fixed set, not a random sequence to start later",
        ));
    }
    if args
        .sample_offset
        .checked_add(job_samples_per_pixel(job))
        .is_none()
    {
        return Err(CliError::usage(format!(
            "--sample-offset {} plus the job's {} samples per pixel passes the largest sample \
             index ({})",
            args.sample_offset,
            job_samples_per_pixel(job),
            u32::MAX
        )));
    }
    if let Some(file) = &args.raw_out
        && args.kind == JobCommandKind::Render
        && file.is_dir()
    {
        return Err(CliError::usage(format!(
            "--raw-out needs a file name for \"render\", but {} is a folder",
            file.display()
        )));
    }
    Ok(())
}

/// The adaptive-sampling settings of this run.
///
/// The flags win: with `--adaptive` (or a flag that implies it) the settings are the defaults
/// for the job's samples per pixel with the flags laid over them, whatever the job holds. Without
/// them the job's own `accel.adaptive` applies, capped at the job's samples per pixel; a job
/// without it renders without adaptive sampling (`None`).
///
/// Refused rather than ignored where the render cannot honor it: the Ray Traced engine has no
/// noise to converge, and a remote worker's share is not adaptive, so a render with adaptive
/// sampling stays on this computer.
#[cfg(feature = "adaptive-sampling")]
fn adaptive_settings(
    args: &RenderJobArgs,
    job: &RenderJobFile,
    has_remote: bool,
    estimator: Estimator,
) -> Result<Option<AdaptiveSettings>, CliError> {
    let from_job = job_adaptive(job);
    if !args.adaptive.enabled && from_job.is_none() {
        return Ok(None);
    }
    if job.engine.is_ray() {
        return Err(CliError::usage(
            "--adaptive is for the path-traced renderer: the Ray Traced engine's taps are a \
             fixed set, with no noise to converge",
        ));
    }
    let compute = args.compute.unwrap_or(job.compute.target);
    if has_remote && compute != ComputeChoice::Local {
        return Err(CliError::usage(
            "--adaptive renders on this computer only: a remote worker's share is not adaptive \
             yet; use --compute local",
        ));
    }
    if args.adaptive.enabled {
        let max_spp = job_samples_per_pixel(job);
        let mut wanted = args.adaptive;
        if estimator == Estimator::ExitCollection && wanted.min_spp.is_none() {
            wanted.min_spp = Some(collect_min_spp(max_spp));
        }
        return wanted.settings(max_spp).map_err(CliError::usage);
    }
    let Some(stored) = from_job else {
        return Ok(None);
    };
    let settings = AdaptiveSettings {
        max_spp: job_samples_per_pixel(job),
        ..stored
    };
    settings
        .validate()
        .map_err(|e| CliError::design(format!("adaptive sampling in the job: {e}")))?;
    Ok(Some(settings))
}

/// The minimum samples per pixel adaptive sampling starts from when the estimator is exit
/// collection.
#[cfg(feature = "adaptive-sampling")]
const COLLECT_MIN_SPP: u32 = 64;

/// The minimum samples per pixel for an adaptive render of `max_spp` samples with exit
/// collection: 64, rounded down to a multiple of the pass size and clamped as
/// `AdaptiveSettings::validate` requires (at least one pass, at most `max_spp`).
#[cfg(feature = "adaptive-sampling")]
fn collect_min_spp(max_spp: u32) -> u32 {
    let pass = AdaptiveSettings::provisional(max_spp).pass_spp;
    ((COLLECT_MIN_SPP.min(max_spp) / pass) * pass).max(pass)
}

/// Refuses `--path-cache` where it cannot be honored: a Ray Traced job (the flag may also have
/// switched the job to it with `--engine ray`, which the parser already refuses) has no light
/// paths to cache. A job file's own `accel.path_cache` on a Ray Traced job is not an error: the
/// renderer ignores it.
fn check_path_cache(args: &RenderJobArgs, job: &RenderJobFile) -> Result<(), CliError> {
    if args.path_cache.enabled && job.engine.is_ray() {
        return Err(CliError::usage(
            "--path-cache is for the path-traced renderer: the Ray Traced engine has no light \
             paths to cache",
        ));
    }
    Ok(())
}

/// Refuses `--beam` (and its overrides) on a Ray Traced job. The parser refuses the same for
/// `--engine ray`; this covers a job file that is Ray Traced itself. A job file's own
/// `accel.beam` on a Ray Traced job is not an error: the renderer ignores it.
#[cfg(feature = "beam")]
fn check_beam(args: &RenderJobArgs, job: &RenderJobFile) -> Result<(), CliError> {
    if args.beam.enabled && job.engine.is_ray() {
        return Err(CliError::usage(format!(
            "{} (--beam): the Ray Traced engine has no residual to sample",
            BeamOff::RayTracedResidual.message()
        )));
    }
    Ok(())
}

/// What `--beam` changes in the picture, as text for the job's token: the settings it renders
/// with.
#[cfg(feature = "beam")]
fn beam_change(settings: &BeamSettings) -> String {
    let mode = match settings.region_mode {
        RegionMode::EndIntersection => "end-intersection",
        RegionMode::PerNode => "per-node",
    };
    format!(
        "beam={}:{}:{}:{mode}",
        settings.spectral_nodes, settings.residual_min_spp, settings.batch_tiles
    )
}

/// Refuses `--optics-tables` on a Ray Traced job, which does not use the tables. The parser
/// refuses the same for `--engine ray`; this covers a job file that is Ray Traced itself.
fn check_optics_tables(args: &RenderJobArgs, job: &RenderJobFile) -> Result<(), CliError> {
    if args.optics_tables == OpticsTables::On && job.engine.is_ray() {
        return Err(CliError::usage(
            "--optics-tables is for the path-traced renderer: the Ray Traced engine does not \
             use the tables",
        ));
    }
    Ok(())
}

/// Whether a `--beam` flag is given.
#[cfg(feature = "beam")]
const fn beam_flags_given(args: &RenderJobArgs) -> bool {
    args.beam.enabled
}

/// Without the `beam` feature the command line has no `--beam` flags.
#[cfg(not(feature = "beam"))]
const fn beam_flags_given(_args: &RenderJobArgs) -> bool {
    false
}

/// The note of a run whose cache or beam flags made it branching.
const IMPLIED_BRANCHING_NOTE: &str =
    "Estimator: branching for this run (the path cache and beam tracing need it).";

/// Whether the cache or beam flags make the run branching.
const fn flags_imply_branching(args: &RenderJobArgs) -> bool {
    args.path_cache.enabled || beam_flags_given(args)
}

/// The estimator of this run: branching when `--path-cache`, `--persist-cache` or a `--beam*`
/// flag is given (see [`flags_imply_branching`]), otherwise the apps' estimator, exit collection.
const fn effective_estimator(args: &RenderJobArgs) -> Estimator {
    if flags_imply_branching(args) {
        Estimator::Branching
    } else {
        Estimator::APP_DEFAULT
    }
}

/// The notes the estimator of this run owes.
///
/// That is the note that the cache or beam flags made the run branching. A job's own path cache
/// and beams are the executor's business: it drops them under exit collection with a note.
fn estimator_notes(args: &RenderJobArgs) -> Vec<&'static str> {
    let mut notes = Vec::new();
    if flags_imply_branching(args) {
        notes.push(IMPLIED_BRANCHING_NOTE);
    }
    notes
}

/// The adaptive settings a job file asks for, if any.
#[cfg(feature = "adaptive-sampling")]
const fn job_adaptive(job: &RenderJobFile) -> Option<AdaptiveSettings> {
    match &job.kind {
        JobKind::Still(still) => still.accel.adaptive,
        JobKind::TiltVideo(video) => video.accel.adaptive,
    }
}

/// What adaptive sampling changes in the picture, as text for the job's token: the metric, the
/// target and the minimum.
#[cfg(feature = "adaptive-sampling")]
fn adaptive_change(settings: &AdaptiveSettings) -> String {
    let metric = match settings.metric {
        NoiseMetric::RelativeLstar => "relative",
        NoiseMetric::AbsoluteDeltaE => "absolute",
    };
    format!(
        "adaptive={metric}:{}:{}:grow8",
        settings.target_noise, settings.min_spp
    )
}

/// The sink for `--raw-out` and `--progress-raw`, with its folders made now so that a bad path
/// fails before the render and not after it; `None` when neither flag was given.
fn make_raw_sink(args: &RenderJobArgs) -> Result<Option<Arc<RawFileSink>>, CliError> {
    if args.raw_out.is_none() && args.progress_raw.is_none() {
        return Ok(None);
    }
    let target = args.raw_out.clone().map(|path| match args.kind {
        JobCommandKind::Render => RawTarget::File(path),
        JobCommandKind::TiltVideo => RawTarget::Folder(path),
    });
    let own_folder = match &target {
        Some(RawTarget::File(file)) => file.parent().filter(|p| !p.as_os_str().is_empty()),
        Some(RawTarget::Folder(folder)) => Some(folder.as_path()),
        None => None,
    };
    for folder in own_folder.into_iter().chain(args.progress_raw.as_deref()) {
        std::fs::create_dir_all(folder)
            .map_err(|e| CliError::io(format!("cannot create {}: {e}", folder.display())))?;
    }
    Ok(Some(Arc::new(RawFileSink::new(
        target,
        args.progress_raw.clone(),
    ))))
}

/// What the command line changes in the picture, as text: empty when the job renders as it was
/// frozen. The estimator is not part of it: the executor gives the job its estimator's token.
fn command_line_changes(args: &RenderJobArgs) -> String {
    let mut changes = Vec::new();
    if let Some(name) = &args.material {
        changes.push(format!("material={}", name.trim().to_ascii_lowercase()));
    }
    if let Some(key) = &args.color_variant {
        changes.push(format!("color-variant={}", key.trim().to_ascii_lowercase()));
    }
    if let Some(label) = &args.color_pattern {
        changes.push(format!(
            "color-pattern={}",
            label.trim().to_ascii_lowercase()
        ));
    }
    if args.highlights != HighlightRule::default() {
        changes.push(format!("highlights={}", args.highlights.cli_word()));
    }
    if let Some(enabled) = args.stone_shadow {
        changes.push(format!("stone-shadow={enabled}"));
    }
    if let Some(reflection) = args.tray_reflection {
        changes.push(format!("tray-reflection={reflection}"));
    }
    if args.exposure == Exposure::Metered {
        changes.push("meter".to_string());
    }
    if args.optics_tables == OpticsTables::On {
        changes.push("optics-tables".to_string());
    }
    changes.join(";")
}

/// The exit code a failed run ends the process with.
const fn failure_code(kind: FailureKind) -> i32 {
    match kind {
        FailureKind::Input => EXIT_DESIGN,
        FailureKind::Output => EXIT_IO,
        FailureKind::Render => EXIT_RENDER,
    }
}

/// The log filter `--verbose` uses when `RUST_LOG` is not set: warnings, and the beam tracer's
/// per-batch debug lines, the GPU backend's and GPU beam engine's decline reasons, the adaptive
/// passes and the CPU+GPU split of every batch.
#[cfg(feature = "beam")]
const VERBOSE_FILTER: &str = "warn,pleochroma::beam=debug,pleochroma::renderer::gpu_backend=debug,pleochroma::renderer::gpu::beam=debug,pleochroma::adaptive=debug,pleochroma::hybrid=debug";

/// The log filter `--verbose` uses when `RUST_LOG` is not set: warnings, the GPU backend's decline
/// reasons, the adaptive passes and the CPU+GPU split of every batch.
#[cfg(all(not(feature = "beam"), feature = "adaptive-sampling"))]
const VERBOSE_FILTER: &str = "warn,pleochroma::renderer::gpu_backend=debug,pleochroma::adaptive=debug,pleochroma::hybrid=debug";

/// The log filter `--verbose` uses when `RUST_LOG` is not set: warnings, the GPU backend's decline
/// reasons and the CPU+GPU split of every batch.
#[cfg(not(feature = "adaptive-sampling"))]
const VERBOSE_FILTER: &str =
    "warn,pleochroma::renderer::gpu_backend=debug,pleochroma::hybrid=debug";

/// Installs the `--verbose` subscriber: log lines to standard error, filtered by `RUST_LOG` or
/// [`VERBOSE_FILTER`]. A subscriber that is already installed stays (the call is then a no-op).
fn install_verbose_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(VERBOSE_FILTER));
    // An error only means another subscriber is already installed.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Renders the job file `args.job`, streaming progress to `progress`.
///
/// `terminal` says `progress` is a terminal, which rewrites one line instead of printing a
/// line per step.
///
/// # Errors
///
/// - [`CliError::io`] when the job file cannot be read;
/// - [`CliError::design`] when it is not a usable job file, or a `--color-variant` cannot be
///   sized for its stone;
/// - [`CliError::usage`] when it holds the other kind of job, its material has no such
///   `--color-variant`, `--highlights` would not reach a finished remote picture, or
///   `--raw-out`, `--progress-raw` or `--sample-offset` cannot be honored (a finished remote
///   picture, a remote share with `--progress-raw`, the Ray Traced engine with an offset), or
///   the adaptive-sampling flags (a build with that feature) cannot (the Ray Traced engine, a
///   remote worker, a value that breaks a rule of the settings);
/// - [`CliError::io`] when a raw folder cannot be made before the render, or a raw file cannot
///   be written during it (reported once the picture is done);
/// - a [`EXIT_RENDER`] error when the render was stopped, and the exit code of the failure
///   (`EXIT_DESIGN`, `EXIT_IO` or `EXIT_RENDER`) when it failed.
pub fn run(args: &RenderJobArgs, progress: &mut dyn Write, terminal: bool) -> CommandResult {
    if args.verbose {
        install_verbose_logging();
    }
    let mut job = load_job(args)?;
    let estimator = effective_estimator(args);
    let remote = match (&args.remote, &args.cert_dir) {
        (Some(address), Some(cert_dir)) => Some(RemoteEndpoint {
            address: address.clone(),
            cert_dir: cert_dir.clone(),
        }),
        _ => None,
    };
    check_highlights_reach_the_picture(args, &job, remote.is_some())?;
    check_benchmark_outputs(args, &job, remote.is_some())?;
    #[cfg(feature = "adaptive-sampling")]
    let adaptive = adaptive_settings(args, &job, remote.is_some(), estimator)?;
    check_path_cache(args, &job)?;
    #[cfg(feature = "beam")]
    check_beam(args, &job)?;
    check_optics_tables(args, &job)?;
    let notes = estimator_notes(args);
    let raw_sink = make_raw_sink(args)?;
    let changes = command_line_changes(args);
    #[cfg(any(feature = "beam", feature = "adaptive-sampling"))]
    let mut changes = changes;
    #[cfg(feature = "beam")]
    if let Some(settings) = args.beam.settings() {
        // Another picture (the beams shade the stone exactly, the residual is sampled at its own
        // minimum): a tilt video never mixes frames made with and without it.
        if !changes.is_empty() {
            changes.push(';');
        }
        changes.push_str(&beam_change(&settings));
    }
    #[cfg(feature = "adaptive-sampling")]
    if let (true, Some(settings)) = (args.adaptive.enabled, &adaptive) {
        // Another picture: a tilt video never mixes frames made with and without it. Settings
        // that come from the job file itself are part of the job's own token already.
        if !changes.is_empty() {
            changes.push(';');
        }
        changes.push_str(&adaptive_change(settings));
    }
    if !changes.is_empty() {
        job.token = changed_token(&job.token, &changes);
    }
    // Opt in; meaningful only for an adaptive render. It does not change the job token: it is
    // the same picture, measured on the GPU, and a render without it is byte-identical.
    #[cfg(feature = "adaptive-sampling")]
    let gpu_convergence = args.adaptive.gpu_convergence && adaptive.is_some();
    let options = HeadlessOptions {
        local: args.local,
        remote,
        compute: args.compute,
        transfer: args.transfer,
        contribute_local: args.contribute_local.then_some(true),
        output: args.out.clone(),
        restart_frames: args.restart,
        taps: RenderTaps {
            sample_offset: args.sample_offset,
            sink: raw_sink
                .as_ref()
                .map(|sink| -> Arc<dyn RawTapSink> { Arc::<RawFileSink>::clone(sink) }),
            snapshots: args.progress_raw.is_some(),
            frame_name: None,
            #[cfg(feature = "adaptive-sampling")]
            adaptive,
            // Without the `adaptive-sampling` feature there are no flags; a job's own
            // `accel.adaptive` renders without adaptive sampling, with a note (the app's
            // executor).
            #[cfg(not(feature = "adaptive-sampling"))]
            adaptive: None,
            #[cfg(feature = "adaptive-sampling")]
            gpu_convergence,
            #[cfg(not(feature = "adaptive-sampling"))]
            gpu_convergence: false,
            // The flags win; without them the job's own `accel.path_cache` applies (the render
            // queue's executor fills it in). A kept cache does not change the picture's token:
            // it is the same picture, shaded from cached paths.
            path_cache: args.path_cache.settings(),
            cache_dir: args.path_cache.cache_dir.clone(),
            cache_disk_limit_bytes: None,
            // Likewise `accel.beam`: the flags win, otherwise the job's own setting applies.
            #[cfg(feature = "beam")]
            beam: args.beam.settings(),
            // Without the `beam` feature there are no flags; a job's own `accel.beam` renders
            // without beams, with a note (the app's executor).
            #[cfg(not(feature = "beam"))]
            beam: None,
        },
    };
    let ended =
        run_with_process_settings(args, estimator, &job, &options, &notes, progress, terminal);
    report_outcome(ended, raw_sink.as_deref())
}

/// Reads the job file `args.job` and lays the command line's changes over it: the renderer, the
/// material, the color variant and pattern, the metered exposure, the stone shadow and the tray
/// reflection (which recompute the job's format and refuse a light with no backdrop).
fn load_job(args: &RenderJobArgs) -> Result<RenderJobFile, CliError> {
    let text = std::fs::read_to_string(&args.job)
        .map_err(|e| CliError::io(format!("cannot read {}: {e}", args.job.display())))?;
    let mut job = codec::from_text(&text).map_err(|e| CliError::design(e.to_string()))?;
    if let Some(engine) = args.engine {
        job.engine.engine = engine;
        if job.engine.is_ray() {
            // A Ray-Traced job never carries color zones.
            job.zoning = None;
        }
        job.format = pleochroma_render_jobs::job::format_for(&job);
        job.scene.engine = job.engine;
        if job.engine.is_ray() {
            let taps = job.engine.target_samples(1);
            match &mut job.kind {
                JobKind::Still(still) => still.samples_per_pixel = taps,
                JobKind::TiltVideo(video) => video.samples_per_pixel = taps,
            }
            if job.optical_diagnostics.is_some() {
                return Err(CliError::design(
                    "Optical diagnostics require --engine path.",
                ));
            }
        }
    }
    if let Some(enabled) = args.stone_shadow {
        job.scene.stone_shadow = enabled;
        job.format = pleochroma_render_jobs::job::format_for(&job);
        job.validate()
            .map_err(|error| CliError::design(error.to_string()))?;
    }
    if let Some(reflection) = args.tray_reflection {
        job.scene.tray_reflection = reflection;
        job.format = pleochroma_render_jobs::job::format_for(&job);
        job.validate()
            .map_err(|error| CliError::design(error.to_string()))?;
    }
    check_kind(args.kind, &job)?;
    if let Some(name) = &args.material {
        apply_material(&mut job, name)?;
    }
    if let Some(key) = &args.color_variant {
        apply_color_variant(&mut job, key)?;
    }
    if let Some(label) = &args.color_pattern {
        apply_color_pattern(&mut job, label, args.color_variant.is_some())?;
    }
    if args.exposure == Exposure::Metered {
        apply_meter(&mut job);
    }
    Ok(job)
}

/// Runs `job` with the process-wide settings the command line asks for in force (the stone
/// estimator `estimator` among them), and puts the previous ones back afterwards. The whole run
/// holds [`PROCESS_SETTINGS`]; a lock poisoned by a panicking render is taken over, never a
/// panic of its own.
fn run_with_process_settings(
    args: &RenderJobArgs,
    estimator: Estimator,
    job: &RenderJobFile,
    options: &HeadlessOptions,
    notes: &[&str],
    progress: &mut dyn Write,
    terminal: bool,
) -> JobOutcome {
    // The rule, the optics tables and the estimator are process-wide: they are set before the
    // first frame is prepared and put back when `settings` drops, on every path.
    let settings = ProcessSettings::apply(estimator, args.highlights, args.optics_tables);
    // Ctrl+C is not trapped: a video's frames are written atomically, so the next run
    // continues from them.
    let cancel = AtomicBool::new(false);
    let mut sink = StreamSink::new(progress, args.kind.word(), terminal, args.quiet);
    if args.local != LocalEngines::Cpu && !gpu_compiled_in() {
        sink.note("this build has no GPU support; rendering on the CPU.");
    }
    // A remote worker is a process of its own and renders without the optics tables.
    if args.optics_tables == OpticsTables::On
        && options.remote.is_some()
        && !matches!(options.compute, Some(ComputeChoice::Local))
    {
        sink.note(
            "--optics-tables applies to this computer only; a remote worker renders without \
             the tables.",
        );
    }
    for note in notes {
        sink.note(note);
    }
    let ended = run_job(job, &job_folder(&args.job), options, &cancel, &mut sink);
    drop(settings);
    sink.finish();
    ended
}

/// The process-wide settings a render reads (the stone estimator, the highlight rule and the
/// optics tables), set for one render and put back when it drops, whether the render ended or
/// panicked. It holds [`PROCESS_SETTINGS`] for as long as it lives; a lock poisoned by a
/// panicking render is taken over, never a panic of its own.
struct ProcessSettings {
    previous_estimator: Estimator,
    previous_rule: HighlightRule,
    /// The tables' previous default, kept only when this render turned them on.
    previous_tables: Option<OpticsTables>,
    /// Released after the settings are put back, since fields drop after [`Drop::drop`] runs.
    _lock: MutexGuard<'static, ()>,
}

impl ProcessSettings {
    /// Takes [`PROCESS_SETTINGS`] and sets the estimator, the highlight rule and, when asked for,
    /// the optics tables, remembering the previous values.
    fn apply(estimator: Estimator, rule: HighlightRule, tables: OpticsTables) -> Self {
        let lock = PROCESS_SETTINGS
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let settings = Self {
            previous_estimator: default_estimator(),
            previous_rule: highlight_rule(),
            previous_tables: (tables == OpticsTables::On).then(default_optics_tables),
            _lock: lock,
        };
        set_default_estimator(estimator);
        set_highlight_rule(rule);
        if tables == OpticsTables::On {
            set_default_optics_tables(OpticsTables::On);
        }
        settings
    }
}

impl Drop for ProcessSettings {
    fn drop(&mut self) {
        set_default_estimator(self.previous_estimator);
        set_highlight_rule(self.previous_rule);
        if let Some(tables) = self.previous_tables {
            set_default_optics_tables(tables);
        }
    }
}

/// The command's result for how the job ended, with the notes about the raw files.
fn report_outcome(ended: JobOutcome, raw_sink: Option<&RawFileSink>) -> CommandResult {
    match ended {
        JobOutcome::Done { result, note } => {
            if let Some(message) = raw_sink.and_then(RawFileSink::take_error) {
                return Err(CliError::io(message));
            }
            let mut outcome = Outcome::text(format!("{}\n", result.display()));
            if let Some(note) = note {
                outcome = outcome.with_note(&format!("note: {note}"));
            }
            if raw_sink.is_some_and(|sink| sink.files_written() == 0) {
                outcome = outcome.with_note(
                    "note: no frame was rendered by this run (they were already on disk or \
                     copied from earlier poses), so no raw file was written",
                );
            }
            Ok(outcome.with_note(&format!("wrote {}", result.display())))
        }
        JobOutcome::Stopped { .. } => Err(CliError {
            code: EXIT_RENDER,
            message: "the render was stopped".to_string(),
        }),
        JobOutcome::Failed { kind, message, .. } => Err(CliError {
            code: failure_code(kind),
            message,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        outcome::{EXIT_OK, EXIT_USAGE},
        run_command_line, testing,
    };
    use pleochroma::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
        renderer::{accel::RenderAccel, adaptive::AdaptiveSettings},
    };
    use pleochroma_net::{SceneState, scene::SceneEnvironment};
    use pleochroma_render_jobs::{
        ComputeChoice, JobColorSpace, StillJob, TiltVideoJob, TransferChoice,
        job::{DesignInfo, JobCompute, OverlaySelection},
    };

    /// The smallest scene a job accepts: a round brilliant, 16 by 16, four bounces.
    fn tiny_scene() -> SceneState {
        SceneState {
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            width: 16,
            height: 16,
            yaw: 0.4,
            camera_roll: 0.0,
            stone_shadow: false,
            tray_reflection: 0.0,
            hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
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
            head_shadow_deg: 16.0,
            frosted_planes: Vec::new(),
            effects: pleochroma_net::scene::effects::SceneEffects::default(),
            // Branching, unlike the apps' exit collection: a plain run changes the estimator,
            // which the executor puts in the token.
            estimator: Estimator::Branching,
        }
    }

    fn job_with(output: &Path, kind: JobKind) -> RenderJobFile {
        RenderJobFile {
            zoning: None,
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            optical_diagnostics: None,
            format: 1,
            token: "9f2c4a7d1e0b48c3a65d2f71c8e4b903".to_string(),
            label: "Test job".to_string(),
            created_at: 1_791_302_580,
            design: DesignInfo::default(),
            scene: tiny_scene(),
            scene_backdrop_model: pleochroma_render_jobs::job::BACKDROP_MODEL_TRAY,
            hdr: None,
            compute: JobCompute {
                target: ComputeChoice::Local,
                transfer: TransferChoice::FullData,
                contribute_local: false,
            },
            output: output.to_string_lossy().into_owned(),
            kind,
        }
    }

    fn still_kind() -> JobKind {
        JobKind::Still(StillJob {
            samples_per_pixel: 2,
            color_space: JobColorSpace::Srgb,
            preset_label: String::new(),
            accel: RenderAccel::default(),
        })
    }

    fn video_kind() -> JobKind {
        JobKind::TiltVideo(TiltVideoJob {
            axis_index: 0,
            more_axes: Vec::new(),
            start_deg: 0.0,
            end_deg: 1.0,
            step_deg: 1.0,
            total_frames: 2,
            fps: 2,
            samples_per_pixel: 1,
            color_space: JobColorSpace::Srgb,
            overlay: OverlaySelection::default(),
            curves: None,
            more_curves: Vec::new(),
            keep_frames: true,
            video_name: "tiny tilt".to_string(),
            accel: RenderAccel::default(),
            keep_path_cache: false,
        })
    }

    /// A fresh, empty folder for one test.
    fn folder(name: &str) -> PathBuf {
        let folder = testing::temp_path(name);
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("the test folder can be made");
        folder
    }

    fn write_job(folder: &Path, job: &RenderJobFile) -> String {
        let path = folder.join("tiny.job.json");
        std::fs::write(&path, codec::to_text(job).expect("the job encodes"))
            .expect("the job file can be written");
        path.to_string_lossy().into_owned()
    }

    fn run_words(parts: &[&str]) -> crate::Outcome {
        let args: Vec<String> = parts.iter().map(|part| (*part).to_string()).collect();
        run_command_line(&args)
    }

    #[test]
    fn a_still_job_renders_a_png_and_prints_its_path() {
        let folder = folder("render-job-still");
        let png = folder.join("out.png");
        let job = write_job(&folder, &job_with(&png, still_kind()));
        let outcome = run_words(&["render", &job, "--local", "cpu"]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert!(png.is_file(), "the picture is on disk");
        assert_eq!(outcome.stdout, format!("{}\n", png.display()));
        assert!(
            outcome.stderr.contains(&format!("wrote {}", png.display())),
            "{}",
            outcome.stderr
        );
    }

    /// A zoned job file (format 5) keeps its zones through `load_job`, and the overrides that
    /// replace the stone's color drop them and recompute the format: `--engine ray` gives
    /// format 2, `--material Diamond` format 1.
    #[test]
    fn overrides_that_replace_the_stones_color_drop_the_file_zones() {
        let folder = folder("render-job-zoned-overrides");
        let mut job = job_with(&folder.join("out.png"), still_kind());
        job.zoning = Some(pleochroma::optics::materials::ametrine_zoning());
        job.scene.material.absorption_unit = pleochroma::optics::materials::AbsorptionUnit::PerMm;
        job.format = pleochroma_render_jobs::job::format_for(&job);
        assert_eq!(job.format, 5);
        let file = write_job(&folder, &job);

        let kept = load_job(&render_args_of(&["render", &file])).expect("loads");
        assert_eq!(kept.zoning, job.zoning, "the zones are used as written");
        assert_eq!(kept.format, 5);

        let ray = load_job(&render_args_of(&["render", &file, "--engine", "ray"])).expect("loads");
        assert!(
            ray.zoning.is_none(),
            "the Ray-Traced engine renders the base color"
        );
        assert_eq!(ray.format, 2);

        let diamond =
            load_job(&render_args_of(&["render", &file, "--material", "Diamond"])).expect("loads");
        assert!(diamond.zoning.is_none(), "a new material has its own color");
        assert_eq!(diamond.format, 1);
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// The design's render physics (inclusions, play-of-color, surface finish, the machine and
    /// the abraded edge) ride in the job's scene: a job that sets them is a usable job, and a
    /// record no sender could produce is refused as a design problem, not rendered.
    #[test]
    fn a_job_carrying_render_effects_renders_and_a_malformed_record_is_refused() {
        use pleochroma::optics::raytracer::surface::{FacetSurface, PolishPreset};

        let folder = folder("render-job-effects");
        let png = folder.join("out.png");
        let mut job = job_with(&png, still_kind());
        job.scene.effects.facet_surfaces =
            vec![FacetSurface::from_preset(PolishPreset::Commercial, 0.3).packed(); 4];
        job.scene.effects.edge.width = 0.02;
        job.scene.effects.edge.alpha = 0.3;
        let path = write_job(&folder, &job);
        let outcome = run_words(&["render", &path, "--local", "cpu"]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert!(png.is_file(), "the picture is on disk");

        job.scene.effects.edge.alpha = 7.0;
        let bad = folder.join("bad.job.json");
        std::fs::write(&bad, serde_json::to_string_pretty(&job).unwrap()).unwrap();
        let outcome = run_words(&["render", &bad.to_string_lossy(), "--local", "cpu"]);
        assert_eq!(
            outcome.exit,
            crate::outcome::EXIT_DESIGN,
            "{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains("render effects"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn quiet_leaves_out_the_progress_lines() {
        let folder = folder("render-job-quiet");
        let png = folder.join("out.png");
        let job = write_job(&folder, &job_with(&png, still_kind()));
        let outcome = run_words(&["render", &job, "--local", "cpu", "--quiet"]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert!(!outcome.stderr.contains("render:"), "{}", outcome.stderr);
        assert!(png.is_file());
    }

    #[test]
    fn a_still_file_under_tilt_video_is_a_command_line_mistake() {
        let folder = folder("render-job-wrong-kind");
        let job = write_job(&folder, &job_with(&folder.join("out.png"), still_kind()));
        let outcome = run_words(&["tilt-video", &job, "--local", "cpu"]);
        assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
        assert!(
            outcome.stderr.contains("pleochroma-cli render FILE"),
            "{}",
            outcome.stderr
        );

        let video = write_job(&folder, &job_with(&folder.join("frames"), video_kind()));
        let outcome = run_words(&["render", &video, "--local", "cpu"]);
        assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
        assert!(
            outcome.stderr.contains("pleochroma-cli tilt-video FILE"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_missing_job_file_is_exit_3() {
        let missing = testing::temp_path("render-job-no-such-folder").join("none.job.json");
        let outcome = run_words(&["render", &missing.to_string_lossy(), "--local", "cpu"]);
        assert_eq!(outcome.exit, EXIT_IO, "{}", outcome.stderr);
        assert!(
            outcome.stderr.starts_with("error: cannot read"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_job_file_of_a_newer_format_is_exit_2() {
        let folder = folder("render-job-newer");
        let job = job_with(&folder.join("out.png"), still_kind());
        let mut value: serde_json::Value =
            serde_json::from_str(&codec::to_text(&job).expect("the job encodes"))
                .expect("the text is JSON");
        value["format"] = serde_json::json!(99);
        let path = folder.join("newer.job.json");
        std::fs::write(&path, value.to_string()).expect("the job file can be written");
        let outcome = run_words(&["render", &path.to_string_lossy(), "--local", "cpu"]);
        assert_eq!(outcome.exit, EXIT_DESIGN, "{}", outcome.stderr);
        assert!(!folder.join("out.png").exists());
    }

    #[test]
    fn a_tilt_video_resumes_from_its_frames_and_restart_renders_them_again() {
        let folder = folder("render-job-video");
        let frames = folder.join("frames");
        let job = write_job(&folder, &job_with(&frames, video_kind()));

        let first = run_words(&["tilt-video", &job, "--local", "cpu"]);
        assert_eq!(first.exit, EXIT_OK, "{}", first.stderr);
        let video = first.stdout.trim().to_string();
        let extension = Path::new(&video)
            .extension()
            .map(std::ffi::OsStr::to_ascii_lowercase);
        assert!(
            extension.is_some_and(|extension| extension == "mp4" || extension == "gif"),
            "{}",
            first.stdout
        );
        assert!(
            first.stderr.contains("frame 1 of 2 done"),
            "{}",
            first.stderr
        );
        assert!(
            first.stderr.contains("frame 2 of 2 done"),
            "{}",
            first.stderr
        );

        std::fs::remove_file(&video).expect("the video was written");
        let second = run_words(&["tilt-video", &job, "--local", "cpu"]);
        assert_eq!(second.exit, EXIT_OK, "{}", second.stderr);
        assert!(
            !second.stderr.contains("frame 1 of 2 done"),
            "frame 1 is reused: {}",
            second.stderr
        );
        assert!(Path::new(&video).is_file(), "the video is made again");

        let third = run_words(&["tilt-video", &job, "--local", "cpu", "--restart"]);
        assert_eq!(third.exit, EXIT_OK, "{}", third.stderr);
        assert!(
            third.stderr.contains("frame 1 of 2 done"),
            "{}",
            third.stderr
        );
        assert!(
            third.stderr.contains("frame 2 of 2 done"),
            "{}",
            third.stderr
        );
    }

    #[test]
    fn a_color_variant_renders_and_an_unknown_one_is_refused_with_the_keys() {
        let folder = folder("render-job-variant");
        let png = folder.join("out.png");
        let job = write_job(&folder, &job_with(&png, still_kind()));
        let outcome = run_words(&[
            "render",
            &job,
            "--local",
            "cpu",
            "--color-variant",
            "canary",
        ]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert!(png.is_file(), "the picture is on disk");

        let outcome = run_words(&[
            "render",
            &job,
            "--local",
            "cpu",
            "--color-variant",
            "ruby.thai",
        ]);
        assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
        assert!(
            outcome
                .stderr
                .contains("Diamond has no color variant \"ruby.thai\""),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains("diamond.canary"),
            "{}",
            outcome.stderr
        );

        let mut custom = job_with(&png, still_kind());
        custom.scene.material.name = "Fantasy Glass".to_string();
        let custom = write_job(&folder, &custom);
        let outcome = run_words(&["render", &custom, "--local", "cpu", "--color-variant", "x"]);
        assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
        assert!(
            outcome.stderr.contains("not a built-in material"),
            "{}",
            outcome.stderr
        );
    }

    /// `--material` switches the look of an effect material on, as the Stone tab does: precious
    /// opal its play-of-color (white opal also its milky body), moonstone its adularescence; a
    /// plain material leaves the job's effects alone.
    #[test]
    fn an_effect_material_brings_its_look_into_the_job() {
        let mut job = job_with(Path::new("out.png"), still_kind());
        apply_material(&mut job, "Precious Opal (White)").expect("built in");
        assert!(job.scene.effects.structure.is_some(), "play-of-color on");
        assert!(job.scene.material.scattering_sigma_s > 0.0, "milky body");

        let mut job = job_with(Path::new("out.png"), still_kind());
        apply_material(&mut job, "moonstone").expect("built in");
        assert_eq!(
            job.scene
                .effects
                .inclusions
                .as_ref()
                .map(|i| i.preset.as_str()),
            Some(pleochroma::optics::materials::MOONSTONE_PRESET)
        );

        let mut job = job_with(Path::new("out.png"), still_kind());
        apply_material(&mut job, "Sapphire").expect("built in");
        assert!(
            job.scene.effects.is_none(),
            "a plain material sets no effect"
        );
    }

    #[test]
    fn another_material_keeps_the_stone_width_and_an_unknown_one_is_refused() {
        let mut job = job_with(Path::new("out.png"), still_kind());
        let before = job.scene.material.clone();
        apply_material(&mut job, " ruby ").expect("Ruby is built in");
        assert_eq!(job.scene.material.name, "Ruby");
        assert_eq!(job.design.material, "Ruby");
        let model_width = measure_model_width(&job.scene.planes);
        let width = stone_width_for_scale(
            before.absorption_unit,
            before.absorption_path_scale,
            model_width,
        )
        .expect("a width");
        let expected = pleochroma::render_setup::absorption_path_scale_for(
            job.scene.material.absorption_unit,
            width,
            model_width,
        )
        .expect("a scale");
        assert!(
            (job.scene.material.absorption_path_scale - expected).abs() < 1e-5 * expected,
            "{} against {expected}",
            job.scene.material.absorption_path_scale
        );

        let folder = folder("render-job-material");
        let job = write_job(&folder, &job_with(&folder.join("out.png"), still_kind()));
        let outcome = run_words(&[
            "render",
            &job,
            "--local",
            "cpu",
            "--material",
            "Unobtainium",
        ]);
        assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
        assert!(
            outcome.stderr.contains("is not a built-in material")
                && outcome.stderr.contains("Ruby"),
            "{}",
            outcome.stderr
        );
        assert_eq!(
            command_line_changes(&match crate::args::parse(
                &[
                    "render",
                    "a.job.json",
                    "--material",
                    "Ruby",
                    "--color-variant",
                    "thai"
                ]
                .map(str::to_string),
            ) {
                Ok(crate::args::Command::Render(args)) => args,
                other => panic!("{other:?}"),
            }),
            "material=ruby;color-variant=thai"
        );
    }

    #[test]
    fn a_color_pattern_is_refused_with_a_variant_an_unknown_name_or_a_material_without_any() {
        let folder = folder("render-job-pattern");
        let png = folder.join("out.png");
        let mut sapphire = job_with(&png, still_kind());
        apply_material(&mut sapphire, "Sapphire").expect("Sapphire is built in");
        let sapphire = write_job(&folder, &sapphire);
        let outcome = run_words(&[
            "render",
            &sapphire,
            "--local",
            "cpu",
            "--color-variant",
            "cornflower",
            "--color-pattern",
            "parti-color",
        ]);
        assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
        assert!(
            outcome.stderr.contains("without --color-variant"),
            "{}",
            outcome.stderr
        );

        let outcome = run_words(&[
            "render",
            &sapphire,
            "--local",
            "cpu",
            "--color-pattern",
            "zebra",
        ]);
        assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
        assert!(
            outcome.stderr.contains("Hexagonal Banding") && outcome.stderr.contains("Parti Color"),
            "{}",
            outcome.stderr
        );

        let diamond = write_job(&folder, &job_with(&png, still_kind()));
        let outcome = run_words(&[
            "render",
            &diamond,
            "--local",
            "cpu",
            "--color-pattern",
            "split",
        ]);
        assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
        assert!(
            outcome.stderr.contains("has no typical color patterns"),
            "{}",
            outcome.stderr
        );
        assert!(!png.exists());
    }

    #[test]
    fn a_variant_keeps_the_stone_width_of_the_job() {
        let mut job = job_with(Path::new("out.png"), still_kind());
        let before = job.scene.material.clone();
        apply_color_variant(&mut job, "diamond.canary").expect("Diamond has Canary");
        let model_width = measure_model_width(&job.scene.planes);
        let width = pleochroma::render_setup::stone_width_for_scale(
            before.absorption_unit,
            before.absorption_path_scale,
            model_width,
        )
        .expect("a width");
        let expected = pleochroma::render_setup::absorption_path_scale_for(
            job.scene.material.absorption_unit,
            width,
            model_width,
        )
        .expect("a scale");
        assert!(
            (job.scene.material.absorption_path_scale - expected).abs() < 1e-5 * expected,
            "{} against {expected}",
            job.scene.material.absorption_path_scale
        );
        assert_ne!(job.scene.material.absorption, before.absorption);
    }

    #[test]
    fn whiten_with_a_finished_remote_picture_is_refused_before_connecting() {
        let folder = folder("render-job-highlights-remote");
        let job = write_job(&folder, &job_with(&folder.join("out.png"), still_kind()));
        let outcome = run_words(&[
            "render",
            &job,
            "--remote",
            "box:7878",
            "--cert-dir",
            "no-certs",
            "--compute",
            "remote",
            "--transfer",
            "final",
            "--highlights",
            "whiten",
        ]);
        assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
        assert!(
            outcome.stderr.contains("--transfer full"),
            "{}",
            outcome.stderr
        );
        assert!(!folder.join("out.png").exists());
    }

    #[test]
    fn a_changed_job_gets_a_token_of_its_own_and_the_same_change_the_same_token() {
        let token = "9f2c4a7d1e0b48c3a65d2f71c8e4b903";
        let variant = changed_token(token, "color-variant=thai");
        assert_eq!(variant.len(), 32);
        assert!(
            variant
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{variant}"
        );
        assert_ne!(variant, token);
        assert_eq!(variant, changed_token(token, "color-variant=thai"));
        assert_ne!(variant, changed_token(token, "highlights=whiten"));
        assert_ne!(
            variant,
            changed_token("0f2c4a7d1e0b48c3a65d2f71c8e4b903", "color-variant=thai")
        );

        let parsed = |line: &str| match crate::args::parse(
            &line
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>(),
        ) {
            Ok(crate::args::Command::Render(args)) => args,
            other => panic!("{other:?}"),
        };
        assert_eq!(command_line_changes(&parsed("render a.job.json")), "");
        assert_eq!(
            command_line_changes(&parsed(
                "render a.job.json --color-variant Thai --highlights whiten"
            )),
            "color-variant=thai;highlights=whiten"
        );
        assert_eq!(
            command_line_changes(&parsed("render a.job.json --highlights keep-color")),
            "",
            "the default rule changes nothing"
        );
        assert_eq!(
            command_line_changes(&parsed("render a.job.json --meter")),
            "meter"
        );
        // The shadow changes the picture, so either word renews the token, and differently.
        let on = command_line_changes(&parsed("render a.job.json --stone-shadow on"));
        let off = command_line_changes(&parsed("render a.job.json --stone-shadow off"));
        assert_eq!(on, "stone-shadow=true");
        assert_eq!(off, "stone-shadow=false");
        assert_ne!(changed_token(token, &on), token);
        assert_ne!(changed_token(token, &on), changed_token(token, &off));
        // The reflection changes the picture too, so it renews the token, and differently.
        let mirror = command_line_changes(&parsed("render a.job.json --tray-reflection 4"));
        assert_eq!(mirror, "tray-reflection=0.04");
        assert_ne!(changed_token(token, &mirror), token);
        assert_ne!(changed_token(token, &mirror), changed_token(token, &on));
    }

    /// The `render` command line `words`, parsed.
    fn render_args_of(words: &[&str]) -> RenderJobArgs {
        let words: Vec<String> = words.iter().map(|part| (*part).to_string()).collect();
        match crate::args::parse(&words) {
            Ok(crate::args::Command::Render(args)) => args,
            other => panic!("{words:?} should parse as a render command, got {other:?}"),
        }
    }

    #[test]
    fn a_tilt_video_made_with_branching_is_refused_by_a_plain_run_and_restart_renders_it() {
        let folder = folder("render-job-video-estimator");
        let frames = folder.join("frames");
        let job = write_job(&folder, &job_with(&frames, video_kind()));

        // The job records branching, and `--path-cache` renders with it, so this run keeps the
        // job's token and its frame folder.
        let first = run_words(&["tilt-video", &job, "--local", "cpu", "--path-cache"]);
        assert_eq!(first.exit, EXIT_OK, "{}", first.stderr);
        let video = first.stdout.trim().to_string();
        std::fs::remove_file(&video).expect("the video was written");

        // A plain run renders with exit collection: another token, another picture, so the
        // folder of the branching run is refused, with the way out in the message.
        let refused = run_words(&["tilt-video", &job, "--local", "cpu"]);
        assert_eq!(refused.exit, EXIT_DESIGN, "{}", refused.stderr);
        assert!(
            refused.stderr.contains("belongs to another render job"),
            "{}",
            refused.stderr
        );
        assert!(refused.stderr.contains("--restart"), "{}", refused.stderr);
        assert!(
            !refused.stderr.contains("--estimator"),
            "{}",
            refused.stderr
        );

        let again = run_words(&["tilt-video", &job, "--local", "cpu", "--restart"]);
        assert_eq!(again.exit, EXIT_OK, "{}", again.stderr);
        assert!(
            again.stderr.contains("frame 2 of 2 done"),
            "{}",
            again.stderr
        );
    }

    #[test]
    fn the_cache_and_beam_flags_make_a_run_branching_with_a_note() {
        // No flags: the apps' estimator, silently.
        let plain = render_args_of(&["render", "a.job.json"]);
        assert_eq!(effective_estimator(&plain), Estimator::APP_DEFAULT);
        assert_eq!(estimator_notes(&plain), [] as [&str; 0]);
        for flags in [
            &["--path-cache"][..],
            &["--persist-cache"],
            #[cfg(all(feature = "beam", beam_tests))]
            &["--beam"],
            #[cfg(all(feature = "beam", beam_tests))]
            &["--keep-beams"],
            #[cfg(all(feature = "beam", beam_tests))]
            &["--beam-nodes", "16"],
        ] {
            let mut words = vec!["render", "a.job.json"];
            words.extend_from_slice(flags);
            let args = render_args_of(&words);
            assert_eq!(
                effective_estimator(&args),
                Estimator::Branching,
                "{flags:?}"
            );
            assert_eq!(
                estimator_notes(&args),
                [IMPLIED_BRANCHING_NOTE],
                "{flags:?}"
            );
        }
        assert_eq!(
            IMPLIED_BRANCHING_NOTE,
            "Estimator: branching for this run (the path cache and beam tracing need it)."
        );
    }

    #[test]
    fn a_run_with_the_path_cache_flag_prints_the_branching_note_once() {
        let folder = folder("render-job-cache-implies-branching");
        let png = folder.join("out.png");
        let job = write_job(&folder, &job_with(&png, still_kind()));
        let outcome = run_words(&["render", &job, "--local", "cpu", "--path-cache"]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert_eq!(
            outcome.stderr.matches(IMPLIED_BRANCHING_NOTE).count(),
            1,
            "{}",
            outcome.stderr
        );
        assert!(
            !outcome.stderr.contains("Path cache off: exit collection"),
            "branching keeps the cache: {}",
            outcome.stderr
        );
    }

    #[cfg(feature = "adaptive-sampling")]
    #[test]
    fn exit_collection_adaptive_sampling_starts_from_64() {
        let job = job_with(Path::new("out.png"), still_of(8192));
        let minimum = |line: &[&str]| settings_for(line, &job).map(|settings| settings.min_spp);
        let plain = ["render", "a.job.json", "--local", "cpu"];
        let with = |extra: &[&'static str]| {
            let mut words = plain.to_vec();
            words.extend_from_slice(extra);
            words
        };
        assert_eq!(minimum(&with(&["--adaptive"])), Some(64));
        assert_eq!(minimum(&with(&["--target-noise", "0.02"])), Some(64));
        // A minimum on the command line wins; a branching run (the path cache needs it) keeps
        // the default of 512.
        assert_eq!(minimum(&with(&["--min-spp", "256"])), Some(256));
        assert_eq!(minimum(&with(&["--adaptive", "--path-cache"])), Some(512));
        assert_eq!(minimum(&plain), None, "adaptive sampling stays off");
    }

    #[cfg(feature = "adaptive-sampling")]
    #[test]
    fn the_collect_minimum_is_a_whole_number_of_passes_inside_the_cap() {
        assert_eq!(collect_min_spp(8192), 64);
        assert_eq!(collect_min_spp(64), 64);
        assert_eq!(collect_min_spp(50), 48);
        assert_eq!(collect_min_spp(16), 16);
        assert_eq!(collect_min_spp(10), 10);
        assert_eq!(collect_min_spp(1), 1);
        for max_spp in [1, 2, 10, 15, 16, 17, 48, 63, 64, 65, 100, 512, 4096] {
            let settings = AdaptiveSettings {
                min_spp: collect_min_spp(max_spp),
                ..AdaptiveSettings::provisional(max_spp)
            };
            assert!(settings.validate().is_ok(), "{max_spp}");
        }
    }

    /// `--stone-shadow` is laid over the job's own setting and the format follows it (3 with the
    /// shadow, the engine's own format without); a light with no backdrop is refused.
    #[test]
    fn the_stone_shadow_override_recomputes_the_format_and_refuses_as_lit() {
        let folder = folder("render-job-stone-shadow");
        let load = |job: &RenderJobFile, word: &str| {
            let path = write_job(&folder, job);
            let args = match crate::args::parse(
                &["render", &path, "--stone-shadow", word].map(str::to_string),
            ) {
                Ok(crate::args::Command::Render(args)) => args,
                other => panic!("{other:?}"),
            };
            load_job(&args)
        };

        let mut plain = job_with(&folder.join("out.png"), still_kind());
        plain.scene.backdrop = 1.0;
        let on = load(&plain, "on").expect("a studio light with a backdrop takes the shadow");
        assert!(on.scene.stone_shadow);
        assert_eq!(on.format, 3);
        assert_eq!(on.validate(), Ok(()));

        let mut shadowed = plain.clone();
        shadowed.scene.stone_shadow = true;
        shadowed.format = pleochroma_render_jobs::job::format_for(&shadowed);
        assert_eq!(shadowed.format, 3);
        let off = load(&shadowed, "off").expect("off needs nothing");
        assert!(!off.scene.stone_shadow);
        assert_ne!(
            off.format, 3,
            "a shadow-free job drops back to its engine's format"
        );
        assert_eq!(off.format, pleochroma_render_jobs::job::format_for(&off));
        assert_eq!(off.validate(), Ok(()));

        let mut as_lit = plain;
        as_lit.scene.backdrop = 0.0;
        let refused = load(&as_lit, "on").unwrap_err();
        assert!(
            refused
                .message
                .contains("Stone shadow needs a studio light with a backdrop."),
            "{}",
            refused.message
        );
    }

    /// `--tray-reflection` is laid over the job's own setting and the format follows it (4 with a
    /// reflection, whatever the shadow, the engine's own format without); a light with no backdrop
    /// is refused.
    #[test]
    fn the_tray_reflection_override_recomputes_the_format_and_refuses_as_lit() {
        let folder = folder("render-job-tray-reflection");
        let load = |job: &RenderJobFile, percent: &str| {
            let path = write_job(&folder, job);
            let args = match crate::args::parse(
                &["render", &path, "--tray-reflection", percent].map(str::to_string),
            ) {
                Ok(crate::args::Command::Render(args)) => args,
                other => panic!("{other:?}"),
            };
            load_job(&args)
        };

        let mut plain = job_with(&folder.join("out.png"), still_kind());
        plain.scene.backdrop = 1.0;
        let on = load(&plain, "4").expect("a studio light with a backdrop takes the reflection");
        assert!((on.scene.tray_reflection - 0.04).abs() < f32::EPSILON);
        assert_eq!(on.format, 4);
        assert_eq!(on.validate(), Ok(()));

        let mut mirrored = plain.clone();
        mirrored.scene.tray_reflection = 0.04;
        mirrored.format = pleochroma_render_jobs::job::format_for(&mirrored);
        assert_eq!(mirrored.format, 4);
        let off = load(&mirrored, "0").expect("zero needs nothing");
        assert!(off.scene.tray_reflection.abs() < f32::EPSILON);
        assert_ne!(
            off.format, 4,
            "a reflection-free job drops back to its engine's format"
        );
        assert_eq!(off.format, pleochroma_render_jobs::job::format_for(&off));
        assert_eq!(off.validate(), Ok(()));

        let mut as_lit = plain;
        as_lit.scene.backdrop = 0.0;
        let refused = load(&as_lit, "4").unwrap_err();
        assert!(
            refused
                .message
                .contains("Tray reflection needs a studio light with a backdrop."),
            "{}",
            refused.message
        );
    }

    /// `--meter` gives the job the exposure the app's live view would use: the job's own
    /// exposure as the compensation on the rig's meter at the job's light pose.
    #[test]
    fn the_meter_reads_the_job_exposure_as_the_compensation() {
        let mut job = job_with(Path::new("out.png"), still_kind());
        job.scene.exposure = 1.5;
        let scene = job.scene.clone();
        apply_meter(&mut job);
        let expected = pleochroma::optics::raytracer::metered_exposure(
            1.5,
            scene.lighting_preset,
            scene.light_yaw,
            scene.light_pitch,
        );
        assert_eq!(job.scene.exposure.to_bits(), expected.to_bits());
        let paper = pleochroma::render_setup::Backdrop::White.resolve(
            scene.lighting_preset,
            scene.light_yaw,
            scene.light_pitch,
        ) * job.scene.exposure;
        assert!(
            1.5_f32
                .mul_add(
                    -pleochroma::optics::raytracer::METERED_PAPER_RADIANCE,
                    paper
                )
                .abs()
                < 1e-3,
            "white paper at {paper}"
        );
    }

    fn read_raw(path: &Path) -> crate::raw_image::RawImage {
        crate::raw_image::RawImage::from_bytes(
            &std::fs::read(path).expect("the raw file is on disk"),
        )
        .expect("a PLRAW1 file")
    }

    fn still_of(samples_per_pixel: u32) -> JobKind {
        JobKind::Still(StillJob {
            samples_per_pixel,
            color_space: JobColorSpace::Srgb,
            preset_label: String::new(),
            accel: RenderAccel::default(),
        })
    }

    /// Renders `job` on the CPU with `--raw-out` into `folder/name` and `extra` flags, and reads
    /// the file back.
    fn render_raw(
        folder: &Path,
        job: &str,
        name: &str,
        extra: &[&str],
    ) -> crate::raw_image::RawImage {
        let raw = folder.join(name);
        let raw_text = raw.to_string_lossy().into_owned();
        let mut words = vec!["render", job, "--local", "cpu", "--raw-out", &raw_text];
        words.extend_from_slice(extra);
        let outcome = run_words(&words);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        read_raw(&raw)
    }

    #[test]
    fn raw_out_writes_the_linear_image_and_leaves_the_png_as_it_was() {
        let folder = folder("render-job-raw-out");
        let job = write_job(&folder, &job_with(&folder.join("out.png"), still_kind()));
        let raw = folder.join("sub").join("still.plraw");
        let with = run_words(&[
            "render",
            &job,
            "--local",
            "cpu",
            "--raw-out",
            &raw.to_string_lossy(),
        ]);
        assert_eq!(with.exit, EXIT_OK, "{}", with.stderr);
        let image = read_raw(&raw);
        assert_eq!((image.width, image.height), (16, 16));
        assert_eq!(image.counts, vec![2; 16 * 16]);
        assert!(
            image
                .xyz
                .iter()
                .flatten()
                .all(|v| v.is_finite() && *v >= 0.0)
        );
        assert!(image.xyz.iter().flatten().any(|v| *v > 0.0), "a lit stone");

        // Off means identical: the picture is the one a render without the flag makes (the
        // name is taken, so the second one is "out (2).png").
        let without = run_words(&["render", &job, "--local", "cpu"]);
        assert_eq!(without.exit, EXIT_OK, "{}", without.stderr);
        assert_ne!(with.stdout, without.stdout);
        assert_eq!(
            std::fs::read(with.stdout.trim()).expect("the first picture"),
            std::fs::read(without.stdout.trim()).expect("the second picture")
        );
    }

    #[test]
    fn a_sample_offset_continues_the_sample_sequence_and_zero_is_the_plain_render() {
        let folder = folder("render-job-offset");
        let two = write_job(&folder, &job_with(&folder.join("two.png"), still_of(2)));
        let first = render_raw(&folder, &two, "first.plraw", &[]);
        let zero = render_raw(&folder, &two, "zero.plraw", &["--sample-offset", "0"]);
        let second = render_raw(&folder, &two, "second.plraw", &["--sample-offset", "2"]);
        assert_eq!(zero, first, "--sample-offset 0 is today's render");
        assert_eq!(
            second.counts,
            vec![2; 16 * 16],
            "the same samples per pixel"
        );
        assert_ne!(second.xyz, first.xyz, "other sample indices, other samples");

        // Samples 0..2 and 2..4 together are samples 0..4 of one render.
        let four = write_job(&folder, &job_with(&folder.join("four.png"), still_of(4)));
        let both = render_raw(&folder, &four, "four.plraw", &[]);
        assert_eq!(both.counts, vec![4; 16 * 16]);
        for ((a, b), whole) in first.xyz.iter().zip(&second.xyz).zip(&both.xyz) {
            for channel in 0..3 {
                let mean = f32::midpoint(a[channel], b[channel]);
                assert!(
                    (mean - whole[channel]).abs() <= 1e-5_f32.mul_add(whole[channel].abs(), 1e-6),
                    "{mean} against {}",
                    whole[channel]
                );
            }
        }
    }

    #[test]
    fn progress_raw_writes_a_snapshot_at_each_power_of_two_and_the_final_image() {
        let folder = folder("render-job-progress");
        let job = write_job(&folder, &job_with(&folder.join("out.png"), still_of(64)));
        let steps = folder.join("steps");
        let raw = folder.join("final.plraw");
        let outcome = run_words(&[
            "render",
            &job,
            "--local",
            "cpu",
            "--raw-out",
            &raw.to_string_lossy(),
            "--progress-raw",
            &steps.to_string_lossy(),
        ]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        for (name, count) in [
            ("spp_000016.plraw", 16),
            ("spp_000032.plraw", 32),
            ("spp_000064.plraw", 64),
        ] {
            let snapshot = read_raw(&steps.join(name));
            assert_eq!(snapshot.counts, vec![count; 16 * 16], "{name}");
            assert!(
                snapshot.xyz.iter().flatten().all(|v| v.is_finite()),
                "{name}"
            );
        }
        assert!(!steps.join("spp_000008.plraw").exists());
        assert_eq!(
            std::fs::read(steps.join("spp_000064.plraw")).expect("the last snapshot"),
            std::fs::read(&raw).expect("the final image"),
            "the last snapshot is the final image"
        );
    }

    /// A job of a 64 x 64 picture at `samples_per_pixel`, so that 16-pixel tiles can differ.
    #[cfg(feature = "adaptive-sampling")]
    fn larger_job(folder: &Path, samples_per_pixel: u32) -> String {
        let mut file = job_with(&folder.join("out.png"), still_of(samples_per_pixel));
        file.scene.width = 64;
        file.scene.height = 64;
        write_job(folder, &file)
    }

    #[cfg(feature = "adaptive-sampling")]
    #[test]
    fn an_adaptive_render_writes_each_pixels_own_count_below_the_cap() {
        let folder = folder("render-job-adaptive");
        let job = larger_job(&folder, 64);
        let raw = folder.join("adaptive.plraw");
        // A loose target: the background and the calm parts stop at the minimum. A tile stops
        // only when its eight neighbours are within twice the target, and in a 4 x 4 tile frame
        // every tile touches the stone, whose worst pixel reads near 1 on the relative scale at
        // 16 samples; 0.9 puts that limit at 1.8.
        let outcome = run_words(&[
            "render",
            &job,
            "--local",
            "cpu",
            "--raw-out",
            &raw.to_string_lossy(),
            "--min-spp",
            "16",
            "--target-noise",
            "0.9",
        ]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        let image = read_raw(&raw);
        assert_eq!((image.width, image.height), (64, 64));
        assert!(
            image.counts.iter().all(|count| (16..=64).contains(count)),
            "no pixel stops before the minimum or passes the cap"
        );
        assert!(
            image.counts.iter().any(|&count| count < 64),
            "some pixels stop below max_spp"
        );
        assert!(
            image
                .xyz
                .iter()
                .flatten()
                .all(|v| v.is_finite() && *v >= 0.0)
        );
        assert!(image.xyz.iter().flatten().any(|v| *v > 0.0), "a lit stone");
        assert!(
            outcome.stderr.contains("adaptive sampling:")
                && outcome.stderr.contains("tiles converged"),
            "the log line: {}",
            outcome.stderr
        );
        assert!(
            Path::new(outcome.stdout.trim()).is_file(),
            "the picture is written"
        );

        // Deterministic: the same flags give the same file.
        let again = folder.join("again.plraw");
        let second = run_words(&[
            "render",
            &job,
            "--local",
            "cpu",
            "--raw-out",
            &again.to_string_lossy(),
            "--min-spp",
            "16",
            "--target-noise",
            "0.9",
        ]);
        assert_eq!(second.exit, EXIT_OK, "{}", second.stderr);
        assert_eq!(
            std::fs::read(&raw).expect("the first file"),
            std::fs::read(&again).expect("the second file")
        );
    }

    #[cfg(feature = "adaptive-sampling")]
    #[test]
    fn an_adaptive_render_takes_progress_snapshots_named_by_the_samples_done() {
        let folder = folder("render-job-adaptive-progress");
        let job = larger_job(&folder, 64);
        let steps = folder.join("steps");
        let raw = folder.join("final.plraw");
        // A target nothing meets: the stone runs to the cap while an empty background may stop.
        let outcome = run_words(&[
            "render",
            &job,
            "--local",
            "cpu",
            "--raw-out",
            &raw.to_string_lossy(),
            "--progress-raw",
            &steps.to_string_lossy(),
            "--min-spp",
            "16",
            "--target-noise",
            "0.000001",
        ]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        for (name, samples) in [
            ("spp_000016.plraw", 16),
            ("spp_000032.plraw", 32),
            ("spp_000064.plraw", 64),
        ] {
            let snapshot = read_raw(&steps.join(name));
            assert!(
                snapshot
                    .counts
                    .iter()
                    .all(|&count| (16..=samples).contains(&count)),
                "{name}"
            );
            assert!(snapshot.counts.contains(&samples), "{name}: a live tile");
        }
        assert_eq!(
            std::fs::read(steps.join("spp_000064.plraw")).expect("the last snapshot"),
            std::fs::read(&raw).expect("the final image"),
            "the last snapshot is the final image"
        );
    }

    #[cfg(feature = "adaptive-sampling")]
    #[test]
    fn an_adaptive_tilt_video_renders_every_frame_adaptively() {
        let folder = folder("render-job-adaptive-video");
        let job = write_job(&folder, &job_with(&folder.join("frames"), video_kind()));
        let raw = folder.join("raw");
        let outcome = run_words(&[
            "tilt-video",
            &job,
            "--local",
            "cpu",
            "--adaptive",
            "--raw-out",
            &raw.to_string_lossy(),
        ]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        for name in ["frame_1.plraw", "frame_2.plraw"] {
            // One sample per pixel is the cap: nothing can stop earlier, and no pass is cut.
            assert_eq!(read_raw(&raw.join(name)).counts, vec![1; 16 * 16], "{name}");
        }
        assert!(
            outcome.stderr.contains("adaptive sampling:"),
            "{}",
            outcome.stderr
        );
    }

    #[cfg(feature = "adaptive-sampling")]
    #[test]
    fn adaptive_flags_that_cannot_be_honored_are_refused_before_anything_renders() {
        let folder = folder("render-job-adaptive-refused");
        let png = folder.join("out.png");
        let job = write_job(&folder, &job_with(&png, still_of(64)));
        let refused = |extra: &[&str]| {
            let mut words = vec!["render", job.as_str(), "--local", "cpu"];
            words.extend_from_slice(extra);
            let outcome = run_words(&words);
            assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
            outcome.stderr
        };
        let remote = ["--remote", "box:7878", "--cert-dir", "no-certs"];
        for compute in ["remote", "both"] {
            let mut words = remote.to_vec();
            words.extend_from_slice(&["--compute", compute, "--adaptive"]);
            let message = refused(&words);
            assert!(message.contains("this computer only"), "{message}");
        }
        let message = refused(&["--adaptive", "--engine", "ray"]);
        assert!(message.contains("path-traced"), "{message}");
        // 24 is under the job's cap of 64 but not a whole number of 16-sample passes (100 would
        // be above the cap, which the sample-count rule names first).
        let message = refused(&["--min-spp", "24"]);
        assert!(message.contains("multiple of the pass size"), "{message}");
        let message = refused(&["--min-spp", "128"]);
        assert!(message.contains("sample counts"), "{message}");
        let message = refused(&["--target-noise", "1.5"]);
        assert!(message.contains("out of range"), "{message}");
        assert!(!png.exists(), "nothing was rendered");
    }

    #[cfg(all(feature = "beam", beam_tests))]
    #[test]
    fn beam_tracing_with_the_ray_traced_engine_is_refused_before_anything_renders() {
        let folder = folder("render-job-beam-refused");
        let png = folder.join("out.png");
        let job = write_job(&folder, &job_with(&png, still_kind()));
        for extra in [
            ["--beam", "--engine", "ray"],
            ["--engine", "ray", "--beam-nodes"],
        ] {
            let mut words = vec!["render", job.as_str(), "--local", "cpu"];
            words.extend_from_slice(&extra);
            if extra[2] == "--beam-nodes" {
                words.push("16");
            }
            let outcome = run_words(&words);
            assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
            assert!(outcome.stderr.contains("Path-Traced"), "{}", outcome.stderr);
        }
        assert!(!png.exists(), "nothing was rendered");
    }

    #[cfg(all(feature = "beam", beam_tests))]
    #[test]
    fn a_job_file_with_accel_beam_round_trips_and_the_flags_replace_it() {
        let folder = folder("render-job-beam-round-trip");
        let beam = BeamSettings {
            spectral_nodes: 96,
            residual_min_spp: 32,
            region_mode: RegionMode::PerNode,
            batch_tiles: 16,
            ..BeamSettings::default()
        };
        let mut file = job_with(&folder.join("out.png"), still_kind());
        if let JobKind::Still(still) = &mut file.kind {
            still.accel.beam = Some(beam);
        }
        let text = codec::to_text(&file).expect("the job encodes");
        let back = codec::from_text(&text).expect("the job decodes");
        let JobKind::Still(still) = &back.kind else {
            panic!("a still job stays a still job");
        };
        assert_eq!(still.accel.beam, Some(beam));
        // A video job carries it too.
        let mut video = job_with(&folder.join("v"), video_kind());
        if let JobKind::TiltVideo(tilt) = &mut video.kind {
            tilt.accel.beam = Some(beam);
        }
        let back = codec::from_text(&codec::to_text(&video).expect("the job encodes"))
            .expect("the job decodes");
        let JobKind::TiltVideo(tilt) = &back.kind else {
            panic!("a video job stays a video job");
        };
        assert_eq!(tilt.accel.beam, Some(beam));
        // A job without it stays without it.
        let plain = codec::from_text(
            &codec::to_text(&job_with(&folder.join("p.png"), still_kind())).expect("encodes"),
        )
        .expect("decodes");
        let JobKind::Still(still) = &plain.kind else {
            panic!("a still job stays a still job");
        };
        assert_eq!(still.accel.beam, None);
        // The command line replaces the job's setting as a whole.
        let args = match crate::args::parse(&["render".to_string(), "a.job.json".to_string()]) {
            Ok(crate::args::Command::Render(args)) => args,
            other => panic!("should parse as render, got {other:?}"),
        };
        assert_eq!(
            args.beam.settings(),
            None,
            "no flag: the job's setting applies"
        );
    }

    /// A still job of `samples_per_pixel` whose file asks for adaptive sampling with `target`
    /// and `min_spp`.
    fn adaptive_job(folder: &Path, samples_per_pixel: u32, target: f32, min_spp: u32) -> String {
        let mut file = job_with(&folder.join("out.png"), still_of(samples_per_pixel));
        file.scene.width = 64;
        file.scene.height = 64;
        if let JobKind::Still(still) = &mut file.kind {
            still.accel.adaptive = Some(AdaptiveSettings {
                target_noise: target,
                min_spp,
                ..AdaptiveSettings::provisional(samples_per_pixel)
            });
        }
        write_job(folder, &file)
    }

    /// A job whose file asks for adaptive sampling renders at its own samples per pixel in a build
    /// without the feature, and the executor's note says that adaptive sampling is off.
    #[cfg(not(feature = "adaptive-sampling"))]
    #[test]
    fn a_job_that_asks_for_adaptive_sampling_renders_without_it_and_says_so() {
        let folder = folder("render-job-adaptive-off");
        let job_path = adaptive_job(&folder, 64, 0.5, 32);
        let raw = folder.join("job.plraw");
        let outcome = run_words(&[
            "render",
            &job_path,
            "--local",
            "cpu",
            "--raw-out",
            &raw.to_string_lossy(),
        ]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert_eq!(
            outcome
                .stderr
                .matches("Adaptive sampling off: this build has no adaptive sampling.")
                .count(),
            1,
            "{}",
            outcome.stderr
        );
        assert!(!outcome.stderr.contains("adaptive sampling:"));
        let image = read_raw(&raw);
        assert!(
            image.counts.iter().all(|count| *count == 64),
            "every pixel gets the job's samples per pixel"
        );
    }

    /// The settings `adaptive_settings` picks for `line` (a `render` command line) and `job`.
    #[cfg(feature = "adaptive-sampling")]
    fn settings_for(line: &[&str], job: &RenderJobFile) -> Option<AdaptiveSettings> {
        let words: Vec<String> = line.iter().map(|part| (*part).to_string()).collect();
        let Ok(crate::args::Command::Render(args)) = crate::args::parse(&words) else {
            panic!("{line:?} should parse as a render command");
        };
        adaptive_settings(&args, job, false, effective_estimator(&args))
            .expect("the settings are usable")
    }

    #[cfg(feature = "adaptive-sampling")]
    #[test]
    fn a_job_that_asks_for_adaptive_sampling_gets_it_without_flags() {
        let folder = folder("render-job-adaptive-from-job");
        let job_path = adaptive_job(&folder, 64, 0.5, 32);
        let job = codec::from_text(&std::fs::read_to_string(&job_path).unwrap()).unwrap();
        let settings = settings_for(&["render", &job_path], &job).expect("adaptive is on");
        assert_eq!(settings.target_noise.to_bits(), 0.5_f32.to_bits());
        assert_eq!((settings.min_spp, settings.max_spp), (32, 64));

        // A stale cap in the file gives way to the job's own samples.
        let mut stale = job;
        if let JobKind::Still(still) = &mut stale.kind
            && let Some(adaptive) = still.accel.adaptive.as_mut()
        {
            adaptive.max_spp = 9;
        }
        let settings = settings_for(&["render", &job_path], &stale).expect("adaptive is on");
        assert_eq!(settings.max_spp, 64);

        // The render uses it: no pixel stops before the file's minimum.
        let raw = folder.join("job.plraw");
        let outcome = run_words(&[
            "render",
            &job_path,
            "--local",
            "cpu",
            "--raw-out",
            &raw.to_string_lossy(),
        ]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        let image = read_raw(&raw);
        assert!(
            image.counts.iter().all(|count| (32..=64).contains(count)),
            "counts follow the job's minimum and cap"
        );
        assert!(
            outcome.stderr.contains("adaptive sampling:"),
            "{}",
            outcome.stderr
        );
    }

    #[cfg(feature = "adaptive-sampling")]
    #[test]
    fn a_job_without_adaptive_sampling_renders_as_before() {
        let folder = folder("render-job-no-adaptive");
        let job = job_with(&folder.join("out.png"), still_of(64));
        assert_eq!(settings_for(&["render", "a.job.json"], &job), None);
    }

    #[cfg(feature = "adaptive-sampling")]
    #[test]
    fn the_flags_override_the_adaptive_settings_of_the_job() {
        let folder = folder("render-job-adaptive-override");
        let job_path = adaptive_job(&folder, 64, 0.5, 32);
        let job = codec::from_text(&std::fs::read_to_string(&job_path).unwrap()).unwrap();
        let settings = settings_for(
            &[
                "render",
                &job_path,
                "--min-spp",
                "16",
                "--target-noise",
                "0.25",
            ],
            &job,
        )
        .expect("adaptive is on");
        assert_eq!(settings.target_noise.to_bits(), 0.25_f32.to_bits());
        assert_eq!((settings.min_spp, settings.max_spp), (16, 64));
        // The flags are the whole answer: the job's own target does not leak into them.
        let settings = settings_for(&["render", &job_path, "--adaptive"], &job).expect("on");
        assert_eq!(
            settings.target_noise.to_bits(),
            AdaptiveSettings::provisional(64).target_noise.to_bits()
        );
    }

    #[cfg(feature = "adaptive-sampling")]
    #[test]
    fn the_adaptive_settings_of_a_job_are_refused_where_they_cannot_run() {
        let folder = folder("render-job-adaptive-from-job-refused");
        let job_path = adaptive_job(&folder, 64, 0.5, 32);
        let png = folder.join("out.png");
        let refused = |extra: &[&str]| {
            let mut words = vec!["render", job_path.as_str()];
            words.extend_from_slice(extra);
            let outcome = run_words(&words);
            assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
            outcome.stderr
        };
        let message = refused(&[
            "--remote",
            "box:7878",
            "--cert-dir",
            "no-certs",
            "--compute",
            "remote",
        ]);
        assert!(message.contains("this computer only"), "{message}");
        let message = refused(&["--engine", "ray"]);
        assert!(message.contains("path-traced"), "{message}");
        assert!(!png.exists(), "nothing was rendered");
    }

    #[test]
    fn a_tilt_video_writes_a_raw_file_per_rendered_frame_named_like_its_png() {
        let folder = folder("render-job-video-raw");
        let job = write_job(&folder, &job_with(&folder.join("frames"), video_kind()));
        let raw = folder.join("raw");
        let first = run_words(&[
            "tilt-video",
            &job,
            "--local",
            "cpu",
            "--raw-out",
            &raw.to_string_lossy(),
        ]);
        assert_eq!(first.exit, EXIT_OK, "{}", first.stderr);
        for name in ["frame_1.plraw", "frame_2.plraw"] {
            let image = read_raw(&raw.join(name));
            assert_eq!((image.width, image.height), (16, 16), "{name}");
            assert_eq!(image.counts, vec![1; 16 * 16], "{name}");
        }

        // A resumed run renders no frame, so it writes no raw file, and says so.
        std::fs::remove_file(raw.join("frame_1.plraw")).expect("the file is there");
        // The first run's video is removed, so that the second run encodes it again.
        let _ = std::fs::remove_file(first.stdout.trim());
        let second = run_words(&[
            "tilt-video",
            &job,
            "--local",
            "cpu",
            "--raw-out",
            &raw.to_string_lossy(),
        ]);
        assert_eq!(second.exit, EXIT_OK, "{}", second.stderr);
        assert!(!raw.join("frame_1.plraw").exists());
        assert!(
            second.stderr.contains("no raw file was written"),
            "{}",
            second.stderr
        );
    }

    #[test]
    fn benchmark_flags_that_cannot_be_honored_are_refused_before_anything_renders() {
        let folder = folder("render-job-raw-refused");
        let png = folder.join("out.png");
        let job = write_job(&folder, &job_with(&png, still_kind()));
        let raw = folder.join("never.plraw");
        let raw_text = raw.to_string_lossy().into_owned();
        let steps = folder.join("never-steps");
        let steps_text = steps.to_string_lossy().into_owned();
        let remote = ["--remote", "box:7878", "--cert-dir", "no-certs"];
        let refused = |extra: &[&str]| {
            let mut words = vec!["render", job.as_str(), "--local", "cpu"];
            words.extend_from_slice(extra);
            let outcome = run_words(&words);
            assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
            outcome.stderr
        };

        let refused_remote = |extra: &[&str]| {
            let mut all = remote.to_vec();
            all.extend_from_slice(extra);
            refused(&all)
        };
        let message = refused_remote(&[
            "--compute",
            "remote",
            "--transfer",
            "final",
            "--raw-out",
            &raw_text,
        ]);
        assert!(message.contains("--transfer full"), "{message}");
        let message = refused_remote(&[
            "--compute",
            "both",
            "--transfer",
            "full",
            "--progress-raw",
            &steps_text,
        ]);
        assert!(
            message.contains("cannot follow a remote worker"),
            "{message}"
        );
        let message = refused(&["--engine", "ray", "--sample-offset", "5"]);
        assert!(message.contains("path-traced"), "{message}");
        let message = refused(&["--sample-offset", "4294967295"]);
        assert!(message.contains("largest sample index"), "{message}");
        let message = refused(&["--sample-offset", "-1"]);
        assert!(message.contains("--sample-offset"), "{message}");
        let message = refused(&["--raw-out", &folder.to_string_lossy()]);
        assert!(message.contains("is a folder"), "{message}");

        let video = write_job(&folder, &job_with(&folder.join("frames"), video_kind()));
        let outcome = run_words(&["tilt-video", &video, "--progress-raw", &steps_text]);
        assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
        assert!(
            outcome.stderr.contains("--progress-raw is for \"render\""),
            "{}",
            outcome.stderr
        );
        assert!(!raw.exists() && !steps.exists() && !png.exists());
    }

    #[test]
    fn the_sink_prints_plain_lines_per_ten_percent_and_each_note_once() {
        let mut buffer: Vec<u8> = Vec::new();
        {
            let mut sink = StreamSink::new(&mut buffer, "render", false, false);
            let at = |fraction: f32, note: Option<&str>| JobProgress {
                frames_done: 0,
                frames_total: 1,
                fraction,
                eta_secs: None,
                note: note.map(str::to_string),
                beam_phase: None,
            };
            sink.progress(&at(0.04, None));
            sink.progress(&at(0.12, Some("slow")));
            sink.progress(&at(0.15, Some("slow")));
            sink.progress(&at(0.4, None));
            sink.finish();
        }
        let text = String::from_utf8(buffer).expect("text");
        assert_eq!(text, "note: slow\nrender: 12%\nrender: 40%\n");
    }

    #[test]
    fn a_terminal_sink_rewrites_one_line_and_ends_it() {
        let mut buffer: Vec<u8> = Vec::new();
        {
            let mut sink = StreamSink::new(&mut buffer, "render", true, false);
            let at = |fraction: f32| JobProgress {
                frames_done: 0,
                frames_total: 1,
                fraction,
                eta_secs: None,
                note: None,
                beam_phase: None,
            };
            sink.progress(&at(0.5));
            sink.progress(&at(1.0));
            sink.finish();
        }
        let text = String::from_utf8(buffer).expect("text");
        assert_eq!(text, "\rrender  50%\rrender  100%\n");
    }

    /// A job file whose `accel` carries `"beam": {}` loads in every build through the CLI's own
    /// `load_job`, keeps `accel.beam` as `Some(default)` through a serde round trip and adds no
    /// estimator note. A build without `beam` renders it without beams, not refused, and prints
    /// the note that says why.
    #[test]
    fn a_job_file_whose_accel_carries_beam_loads_in_every_build() {
        // The full path: the module imports `BeamSettings` only in a build with `beam`.
        let asked = Some(pleochroma::renderer::accel_beam::BeamSettings::default());
        let folder = folder("render-job-accel-beam-loads");
        let png = folder.join("out.png");
        let mut value: serde_json::Value = serde_json::from_str(
            &codec::to_text(&job_with(&png, still_kind())).expect("the job encodes"),
        )
        .expect("the text is JSON");
        value["kind"]["accel"]["beam"] = serde_json::json!({});
        let path = folder.join("beam.job.json");
        std::fs::write(&path, value.to_string()).expect("the job file can be written");
        let path_text = path.to_string_lossy().into_owned();

        let args = render_args_of(&["render", &path_text, "--local", "cpu"]);
        let job = load_job(&args).expect("the CLI loads a job whose accel carries beam");
        let JobKind::Still(still) = &job.kind else {
            panic!("a still job stays a still job");
        };
        assert_eq!(still.accel.beam, asked);

        let back = codec::from_text(&codec::to_text(&job).expect("the job encodes"))
            .expect("the job decodes");
        let JobKind::Still(still) = &back.kind else {
            panic!("a still job stays a still job");
        };
        assert_eq!(still.accel.beam, asked);

        let notes = estimator_notes(&args);
        assert!(
            notes.is_empty(),
            "the default estimator says nothing: {notes:?}"
        );
        let JobKind::Still(still) = &job.kind else {
            panic!("a still job stays a still job");
        };
        assert_eq!(still.accel.beam, asked);

        #[cfg(not(feature = "beam"))]
        {
            let outcome = run_words(&["render", &path_text, "--local", "cpu"]);
            assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
            assert!(png.is_file(), "the picture is on disk");
            assert!(
                outcome
                    .stderr
                    .contains("note: Beam tracing off: this build has no beam tracer."),
                "{}",
                outcome.stderr
            );
        }
        let _ = std::fs::remove_dir_all(&folder);
    }
}
