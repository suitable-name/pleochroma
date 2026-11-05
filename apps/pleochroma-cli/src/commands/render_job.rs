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
//! | not a terminal | a still: one line each time the percentage crosses a multiple of 10; a video: one line per finished frame |
//! | always | `note: ...` lines, each distinct note once |
//!
//! `--quiet` turns the progress lines off, never the notes.
//!
//! # Changing the picture
//!
//! `--material NAME` renders the job's stone in another built-in material, at the job's stone
//! width; `--color-variant KEY` renders the job's material (after `--material`, the new one) in
//! one of its built-in color variants, at the job's stone width; `--color-pattern NAME` renders
//! the stone in one of its material's typical color patterns; `--highlights whiten` shows over-bright colors with
//! `pleochroma::color::HighlightRule::Whiten`, the rule of earlier versions (process-wide
//! while the job runs); `--meter`
//! reads the job's exposure as the compensation on top of the rig's light meter, as the app
//! meters its live view. Each one gives the job a token of its own, so a tilt video never mixes
//! frames made with and without it.

use crate::{
    args::{Exposure, JobCommandKind, RenderJobArgs},
    materials::Catalogue,
    outcome::{CliError, CommandResult, EXIT_DESIGN, EXIT_IO, EXIT_RENDER, Outcome},
};
use pleochroma::{
    color::{HighlightRule, highlight_rule, set_highlight_rule},
    optics::{
        inclusions::spec::{DEFAULT_DENSITY, InclusionSpec},
        materials::{
            MaterialLook,
            variants::{built_in_variants, material_slug},
        },
    },
    render_setup::{
        MaterialOverrides, apply_material_overrides, measure_model_width, stone_width_for_scale,
    },
};
use pleochroma_cut::gui::render_jobs::headless::{
    HeadlessOptions, RemoteEndpoint, gpu_compiled_in, material_with_typical_pattern, run_job,
};
use pleochroma_render_jobs::{
    ComputeChoice, FailureKind, JobKind, JobOutcome, JobProgress, JobSink, LocalEngines,
    RenderJobFile, TransferChoice, codec, run::format_progress_line,
};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

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
            if decile > self.last_decile {
                self.last_decile = decile;
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
/// ([`GemMaterial::with_color_variant_resized`](pleochroma::optics::materials::GemMaterial::with_color_variant_resized)).
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

/// What the command line changes in the picture, as text: empty when the job renders as it was
/// frozen.
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
    if args.exposure == Exposure::Metered {
        changes.push("meter".to_string());
    }
    changes.join(";")
}

/// The token of a job the command line changed: 32 lowercase hexadecimal characters derived
/// from the job's own token and `changes`.
///
/// A tilt video's frame folder is keyed by the token, so frames rendered without the change are
/// never reused for it (the folder of the plain run is refused as another job's), and the same
/// change resumes its own frames.
fn changed_token(token: &str, changes: &str) -> String {
    // Two FNV-1a passes with different offsets: 128 bits, the length of a job token.
    let fnv = |offset: u64| {
        token
            .bytes()
            .chain(std::iter::once(0))
            .chain(changes.bytes())
            .fold(offset, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
            })
    };
    format!(
        "{:016x}{:016x}",
        fnv(0xcbf2_9ce4_8422_2325),
        fnv(0x6c62_272e_07bb_0142)
    )
}

/// The exit code a failed run ends the process with.
const fn failure_code(kind: FailureKind) -> i32 {
    match kind {
        FailureKind::Input => EXIT_DESIGN,
        FailureKind::Output => EXIT_IO,
        FailureKind::Render => EXIT_RENDER,
    }
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
///   `--color-variant`, or `--highlights` would not reach a finished remote picture;
/// - a [`EXIT_RENDER`] error when the render was stopped, and the exit code of the failure
///   (`EXIT_DESIGN`, `EXIT_IO` or `EXIT_RENDER`) when it failed.
pub fn run(args: &RenderJobArgs, progress: &mut dyn Write, terminal: bool) -> CommandResult {
    let text = std::fs::read_to_string(&args.job)
        .map_err(|e| CliError::io(format!("cannot read {}: {e}", args.job.display())))?;
    let mut job = codec::from_text(&text).map_err(|e| CliError::design(e.to_string()))?;
    if let Some(engine) = args.engine {
        job.engine.engine = engine;
        job.format = pleochroma_render_jobs::job::format_for_engine(job.engine);
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
    check_kind(args.kind, &job)?;

    let remote = match (&args.remote, &args.cert_dir) {
        (Some(address), Some(cert_dir)) => Some(RemoteEndpoint {
            address: address.clone(),
            cert_dir: cert_dir.clone(),
        }),
        _ => None,
    };
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
    check_highlights_reach_the_picture(args, &job, remote.is_some())?;
    let changes = command_line_changes(args);
    if !changes.is_empty() {
        job.token = changed_token(&job.token, &changes);
    }
    let options = HeadlessOptions {
        local: args.local,
        remote,
        compute: args.compute,
        transfer: args.transfer,
        contribute_local: args.contribute_local.then_some(true),
        output: args.out.clone(),
        restart_frames: args.restart,
    };
    // Ctrl+C is not trapped: a video's frames are written atomically, so the next run
    // continues from them.
    let cancel = AtomicBool::new(false);
    let mut sink = StreamSink::new(progress, args.kind.word(), terminal, args.quiet);
    if args.local != LocalEngines::Cpu && !gpu_compiled_in() {
        sink.note("this build has no GPU support; rendering on the CPU.");
    }
    // The rule is process-wide (every tone mapping of this render reads it); the previous one
    // comes back afterwards, so a caller that runs several command lines in one process (the
    // tests) is not left with it.
    let previous_rule = highlight_rule();
    set_highlight_rule(args.highlights);
    let ended = run_job(&job, &job_folder(&args.job), &options, &cancel, &mut sink);
    set_highlight_rule(previous_rule);
    sink.finish();

    match ended {
        JobOutcome::Done { result, note } => {
            let mut outcome = Outcome::text(format!("{}\n", result.display()));
            if let Some(note) = note {
                outcome = outcome.with_note(&format!("note: {note}"));
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
        }
    }

    fn job_with(output: &Path, kind: JobKind) -> RenderJobFile {
        RenderJobFile {
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
            };
            sink.progress(&at(0.5));
            sink.progress(&at(1.0));
            sink.finish();
        }
        let text = String::from_utf8(buffer).expect("text");
        assert_eq!(text, "\rrender  50%\rrender  100%\n");
    }
}
