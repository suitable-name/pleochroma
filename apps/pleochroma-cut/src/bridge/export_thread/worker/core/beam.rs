//! The beam-tracing render of the export worker: [`render_beams_on_ctx`] renders one frame's
//! faceted stone from beams and its residual with the local engines instead of the sample-cursor
//! loop (beam guide sections 12.4, 14.6, 14.11).
//!
//! # What it does
//!
//! When `RenderTaps::beam` is set and the engine is Path-Traced, the frame goes to
//! `cpu_frame::render_beam_hybrid_surface`: the CPU builds the beam trees, the engines shade the
//! leaves and run the residual of each batch of tiles, and the finished frame (the white-balanced
//! beam part plus the residual mean, per pixel) goes through the export's usual tone map, PNG and
//! ICC path, like an adaptive frame: [`Accumulation::accum`] holds means with a count of 1.
//!
//! # Compute choices
//!
//! | Choice | Engines handed to the renderer |
//! |---|---|
//! | CPU | the CPU engine |
//! | GPU | the GPU engine |
//! | CPU + GPU | the GPU engine; the CPU runs the tree producer on all but one thread |
//!
//! The renderer's own CPU engine takes every batch a GPU engine declines. The GPU engine exists
//! only in a build with the `gpu` feature; without it every choice is the CPU engine.
//!
//! # Remote workers and kept beams
//!
//! With Remote Only or `Both` and a worker that can take part, the frame's tiles are split between
//! this computer and the worker, which renders whole tiles over protocol v38 ([`remote`]; a lost
//! worker's tiles fail over). Without a configured worker the render stays here and says so
//! ([`NOTE_BEAMS_LOCAL_ONLY`]). With `BeamSettings::keep_leaves` the render stores its leaves
//! after rendering and a re-render of the same view loads them instead of tracing the facets
//! ([`keep`]); that render stays on this computer.
//!
//! # The residual's sampling
//!
//! The residual is an adaptive loop over each batch's tiles ([`residual_settings`]). An adaptive
//! export (`RenderTaps::adaptive`) supplies its settings with the render's samples per pixel as
//! the cap. A fixed-sample export uses `AdaptiveSettings::provisional` for that cap, so its
//! samples per pixel bound the residual.
//!
//! # A stone beams cannot render
//!
//! [`eligible`] decides before anything is built. A stone that is not eligible renders exactly as
//! without beams (the branch returns [`BeamAttempt::Off`] and the ordinary render follows), and
//! the reason ([`BeamOff::message`], nothing for the silent reasons) is queued on the context as
//! the first progress note.
//!
//! # Path cache
//!
//! Beams win on an eligible stone: `with_export_ctx_taps` does not choose (or build) a path cache
//! for it ([`renders_stone`]) and the first note says so. The cache stays in use for an ineligible
//! stone.
//!
//! # Progress
//!
//! The renderer's two-phase progress becomes [`ExportProgress`] reports: the share, the phase and
//! the time left ([`BeamStatus`]), at most every [`PROGRESS_INTERVAL`] (a change of phase is
//! always reported). The thumbnail is built from the finished batches (`BeamRun::on_batch`). The
//! last report carries "Beams: 142318 leaves, 0.8 % residual" ([`beam_note`]): the residual share
//! is the residual samples traced over `width x height x samples per pixel`.
//!
//! # Cancel
//!
//! The export's cancel flag is the renderer's token. A cancelled render returns `Ok` with a
//! partial image, so the flag is checked after the call and the result is never used then.

mod keep;
mod remote;

use super::{
    super::{
        taps::RenderTaps,
        types::{Accumulation, AdaptiveStats},
    },
    lanes::{BeamLaneCache, RenderJob},
};
use crate::{
    bridge::export_thread::{
        BeamStatus, ExportProgress, batch::ExportCtx, preview::PreviewThrottle,
    },
    settings::LocalComputeTarget,
};
use glam::Vec3;
use pleochroma::{
    geometry::tool::ToolPrimitive,
    optics::{
        fluorescence::Fluorescence,
        raytracer::{build_plane_soa, surface::SurfaceScene, transport::PreparedMaterial},
    },
    renderer::{
        accel_beam::{BeamOff, BeamSettings},
        adaptive::{AdaptiveImage, AdaptiveSettings, LiveTiles},
        beam::{
            BeamPhase, BeamProgress, BeamRun, NOTE_BEAMS_HOST_CONVERGENCE, NOTE_BEAMS_LOCAL_ONLY,
            NOTE_BEAMS_REPLACE_PATH_CACHE, beam_note,
            engines::{BeamEngine, CpuBeamEngine, EngineSet},
            frame::BeamFrame,
            leaf::node_count,
            leaf_cache::{LeafCache, RetainedBatch},
            pipeline::{consumer_threads, default_threads},
            primary::TileRect,
            stats::BeamStats,
            tile_residual::ResidualScene,
        },
        cpu_frame::render_beam_hybrid_surface,
        frame_scene::FrameScene,
        gpu_backend::GpuBackend,
        path_cache::eligibility::{StoneTraits, check_eligibility},
    },
    simd::PlanesSoA32,
};
use std::{
    cell::RefCell,
    sync::{
        PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// The display multiplier of the residual's noise check: 1, like the adaptive export (the
/// renderer's values already hold the job's exposure, and the export tone map applies none).
const CHECK_SCALE: f32 = 1.0;

/// What a render says when it re-lit kept beams instead of tracing the facets.
const NOTE_KEPT: &str = "Loaded from the kept beams.";

/// The most often a beam render forwards a progress report; a change of phase is always forwarded.
pub(super) const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// The tools, fluorescence and surface finish of a frame, as the renderer's entries take them.
type Extras<'x> = (&'x [ToolPrimitive], &'x Fluorescence, SurfaceScene<'x>);
/// The progress hook of one local run of the renderer's entry: its two-phase progress.
type ProgressHook<'h> = &'h mut dyn FnMut(BeamProgress);
/// The finished-batch hook of one local run: the batch's tiles and their final pixels.
type BatchHook<'h> = &'h mut dyn FnMut(&[TileRect], &[Vec3]);

/// What the beam branch made of a frame.
pub(super) enum BeamAttempt {
    /// The frame was rendered from beams.
    Done(Accumulation),
    /// The export's cancel flag was raised.
    Cancelled,
    /// A Remote Only render failed with no engine left to take over: the message fails the
    /// export.
    Failed(String),
    /// The frame is not rendered from beams (stone not eligible, settings the beam render does not
    /// take, the Ray Traced engine): the ordinary render follows, its first note already queued.
    Off,
}

/// The settings of the residual's adaptive loop for a render of `samples_per_pixel`: the request
/// of an adaptive export with that cap, or the provisional defaults of that cap.
#[must_use]
pub(super) fn residual_settings(
    samples_per_pixel: u32,
    requested: Option<AdaptiveSettings>,
) -> AdaptiveSettings {
    requested.map_or_else(
        || AdaptiveSettings::provisional(samples_per_pixel),
        |requested| AdaptiveSettings {
            max_spp: samples_per_pixel,
            ..requested
        },
    )
}

/// Whether `frame` with `extras` is beam traced under `settings`: the rules of the renderer's
/// entry (no tools, the path-cache eligibility rules with the real fluorescence and surface, and
/// what `BeamFrame::accepts` admits).
///
/// The frame is not built for the answer (`BeamFrame::accepts` runs its checks only), so no
/// material is prepared and no optics table is built here. The settings do not change what the
/// frame accepts; they stay in the signature of the callers.
///
/// # Errors
///
/// The [`BeamOff`] reason.
pub(super) fn eligible(
    frame: &FrameScene<'_>,
    extras: Extras<'_>,
    _settings: &BeamSettings,
) -> Result<(), BeamOff> {
    if frame.environment.tray_shadow().is_some() {
        return Err(BeamOff::StoneShadow);
    }
    if frame.environment.tray_mirror().is_some() {
        return Err(BeamOff::TrayReflection);
    }
    let (tools, fluorescence, surface) = extras;
    if !tools.is_empty() {
        return Err(BeamOff::Concave);
    }
    check_eligibility(&StoneTraits {
        material: frame.material,
        facet_finishes: frame.facet_finishes,
        tools,
        fluorescence,
        surface,
        depth_of_field: false,
    })
    .map_err(BeamOff::NotEligible)?;
    BeamFrame::accepts(frame)?;
    Ok(())
}

/// Whether beams render the stone of `frame` (they are asked for and the stone is eligible), so
/// that the path cache is not chosen for it.
#[must_use]
pub(super) fn renders_stone(
    settings: Option<&BeamSettings>,
    frame: &FrameScene<'_>,
    extras: Extras<'_>,
) -> bool {
    settings.is_some_and(|settings| eligible(frame, extras, settings).is_ok())
}

/// The note that opens a beam render: which of the other choices beams overrode or cannot join.
/// `None` when there is nothing to say.
#[must_use]
pub(super) fn start_note(
    path_cache_asked: bool,
    host_convergence_asked: bool,
    not_local: bool,
) -> Option<String> {
    let lines: Vec<&str> = [
        (path_cache_asked, NOTE_BEAMS_REPLACE_PATH_CACHE),
        (host_convergence_asked, NOTE_BEAMS_HOST_CONVERGENCE),
        (not_local, NOTE_BEAMS_LOCAL_ONLY),
    ]
    .into_iter()
    .filter_map(|(said, line)| said.then_some(line))
    .collect();
    (!lines.is_empty()).then(|| format!("{}.", lines.join(". ")))
}

/// The frame the context renders: its camera, size, planes and finishes with the traced material
/// and the environment the GPU requests carry.
fn frame_of<'c>(ctx: &'c ExportCtx<'_>) -> FrameScene<'c> {
    FrameScene {
        camera: ctx.camera,
        width: ctx.width,
        height: ctx.height,
        planes: &ctx.scene.active_planes,
        facet_finishes: &ctx.scene.facet_finishes,
        material: ctx.gpu_scene.material,
        max_bounces: ctx.scene.max_bounces,
        environment: ctx.gpu_scene.environment,
    }
}

/// Queues `text` as a sentence in front of the context's first progress note.
fn queue_note(ctx: &ExportCtx<'_>, text: &str) {
    let mut slot = ctx
        .cache_note
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let line = format!("{text}.");
    *slot = Some(match slot.take() {
        Some(existing) => format!("{line} {existing}"),
        None => line,
    });
}

/// The samples per pixel that a `share` (0 to 1) of a render of `total` stands for.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the share is clamped to 0..=1 and the product with a u32 lies in 0..=u32::MAX"
)]
fn samples_at(total: u32, share: f32) -> u32 {
    (f64::from(total) * f64::from(share.clamp(0.0, 1.0))).round() as u32
}

/// The residual samples as a share of the `width x height x samples_per_pixel` of a full render.
fn residual_share(samples_traced: u64, (width, height): (u32, u32), samples_per_pixel: u32) -> f64 {
    let full = u64::from(width) * u64::from(height) * u64::from(samples_per_pixel);
    if full == 0 {
        0.0
    } else {
        samples_traced as f64 / full as f64
    }
}

/// Forwards at most one progress report per [`PROGRESS_INTERVAL`], and every change of phase.
#[derive(Debug, Default)]
struct Throttle {
    /// When and in which phase the last forwarded report was made.
    last: Option<(Instant, BeamPhase)>,
}

impl Throttle {
    /// Whether the report of `phase` at `now` is forwarded; the first one always is.
    fn admits(&mut self, phase: BeamPhase, now: Instant) -> bool {
        let forward = self.last.is_none_or(|(at, last_phase)| {
            last_phase != phase || now.saturating_duration_since(at) >= PROGRESS_INTERVAL
        });
        if forward {
            self.last = Some((now, phase));
        }
        forward
    }
}

/// What a [`Reporter`] changes as the reports go by.
struct ReportState {
    /// Admits at most one report per [`PROGRESS_INTERVAL`].
    throttle: Throttle,
    /// Builds the thumbnail at its own pace.
    previews: PreviewThrottle,
    /// The largest share reported so far: the share never goes back.
    done_so_far: f32,
    /// Whether a part of the frame has reached the finishing phase.
    finishing: bool,
}

/// The reporting of one beam render, fresh or from kept beams: the throttle, the share that never
/// goes back, the phase and the thumbnail of the finished batches.
struct Reporter {
    /// The samples per pixel of the render.
    total: u32,
    /// The frame size.
    size: (u32, u32),
    /// The finished pieces' pixels so far (the thumbnail's source).
    partial: RefCell<Vec<Vec3>>,
    /// The all-zero second buffer the thumbnail builder asks for.
    zeros: Vec<Vec3>,
    /// The throttle, the thumbnail pace and the monotone share and phase.
    state: RefCell<ReportState>,
}

impl Reporter {
    /// The reporter of a render of `size` pixels at `total` samples per pixel.
    fn new(size: (u32, u32), total: u32) -> Self {
        let pixel_count = size.0 as usize * size.1 as usize;
        Self {
            total,
            size,
            partial: RefCell::new(vec![Vec3::ZERO; pixel_count]),
            zeros: vec![Vec3::ZERO; pixel_count],
            state: RefCell::new(ReportState {
                throttle: Throttle::default(),
                previews: PreviewThrottle::new(),
                done_so_far: 0.0,
                finishing: false,
            }),
        }
    }

    /// Forwards the renderer's progress (`phase`, `done`, `eta`) to `report` when the throttle
    /// admits it.
    ///
    /// Like the share, the phase never goes back: once a part of the frame is finishing, every
    /// later report says finishing, even when a later remote round restarts its local tracing.
    fn forward(
        &self,
        phase: BeamPhase,
        done: f32,
        eta: Option<Duration>,
        report: &mut dyn FnMut(ExportProgress),
    ) {
        let mut state = self.state.borrow_mut();
        state.finishing |= phase == BeamPhase::Finishing;
        let phase = if state.finishing {
            BeamPhase::Finishing
        } else {
            phase
        };
        if !state.throttle.admits(phase, Instant::now()) {
            return;
        }
        state.done_so_far = state.done_so_far.max(done);
        let fraction = state.done_so_far;
        let preview = state.previews.maybe_generate(
            self.size.0,
            self.size.1,
            &self.partial.borrow(),
            &self.zeros,
            None,
            1,
        );
        report(ExportProgress {
            fraction,
            samples_done: samples_at(self.total, fraction),
            samples_total: self.total,
            preview,
            note: None,
            beam: Some(BeamStatus { phase, eta }),
        });
    }

    /// Takes a finished batch's final pixels into the thumbnail's source.
    fn on_batch(&self, tiles: &[TileRect], finished: &[Vec3]) {
        paste_batch(&mut self.partial.borrow_mut(), self.size.0, tiles, finished);
    }
}

/// Copies the final pixels of a finished batch (`pixels`, the tiles in order and each tile
/// row-major) into the frame-sized `frame`; tiles or pixels outside it are skipped.
fn paste_batch(frame: &mut [Vec3], width: u32, tiles: &[TileRect], pixels: &[Vec3]) {
    let mut next = pixels.iter();
    for tile in tiles {
        for y in tile.y0..tile.y1 {
            for x in tile.x0..tile.x1 {
                let Some(&pixel) = next.next() else {
                    return;
                };
                let at = y as usize * width as usize + x as usize;
                if let Some(slot) = frame.get_mut(at) {
                    *slot = pixel;
                }
            }
        }
    }
}

/// `(tiles, tiles that stopped before the cap)` of a `width x height` frame cut in `tile`-pixel
/// tiles, from the per-pixel `counts`: a tile counts as converged when its first pixel has fewer
/// than `cap` samples.
fn tile_figures(
    (width, height): (u32, u32),
    tile: u32,
    counts: &[u32],
    cap: u32,
) -> (usize, usize) {
    let tile = tile.max(1);
    let (tiles_x, tiles_y) = (width.div_ceil(tile), height.div_ceil(tile));
    let converged = (0..tiles_y)
        .flat_map(|ty| (0..tiles_x).map(move |tx| (tx, ty)))
        .filter(|&(tx, ty)| {
            let at = (ty * tile) as usize * width as usize + (tx * tile) as usize;
            counts.get(at).is_some_and(|&count| count < cap)
        })
        .count();
    (tiles_x as usize * tiles_y as usize, converged)
}

/// The accumulation of a finished beam render: the final pixels as means with a count of 1, and
/// the residual's per-pixel counts as the statistics.
fn accumulation(image: AdaptiveImage, max_spp: u32, tile: u32) -> Accumulation {
    let (tiles_total, tiles_converged) =
        tile_figures((image.width, image.height), tile, &image.counts, max_spp);
    let samples_done = image.counts.iter().copied().max().unwrap_or(0);
    Accumulation {
        accum: image.pixels,
        samples_per_pixel: 1,
        adaptive: Some(AdaptiveStats {
            counts: image.counts,
            samples_done,
            samples_traced: image.samples_traced,
            max_spp,
            tiles_total,
            tiles_converged,
        }),
    }
}

/// The engines of the compute choice, in the order the renderer deals batches to them.
///
/// `backend` is the export's GPU backend; the GPU engine is built only in a build with the `gpu`
/// feature. An empty list would mean "the renderer's own CPU engine", but every choice names the
/// CPU engine explicitly so the display scale is the export's.
///
/// The second value is whether a GPU engine of the set runs the convergence check on the GPU
/// (`gpu_convergence` asked for and taken), so the host-convergence note is queued only when it
/// was asked for and nothing took it.
fn local_engines<'e>(
    choice: LocalComputeTarget,
    backend: &'e GpuBackend,
    residual: &ResidualScene<'e>,
    (settings, adaptive): (&BeamSettings, &AdaptiveSettings),
    gpu_convergence: bool,
) -> (Vec<Box<dyn BeamEngine + 'e>>, bool) {
    // The CPU engine takes the consumer share of the thread quota; the tree producer takes the
    // rest, so the two stay within the cores.
    let cpu: Box<dyn BeamEngine + 'e> = Box::new(
        CpuBeamEngine::new(residual, settings, adaptive)
            .with_threads(consumer_threads(default_threads()))
            .with_display_scale(CHECK_SCALE),
    );
    // With the `gpu` feature the GPU engine always exists; without it the stub returns `None`.
    #[cfg(feature = "gpu")]
    let gpu = || {
        Some(gpu_engine(
            backend,
            residual,
            (settings, adaptive),
            gpu_convergence,
        ))
    };
    #[cfg(not(feature = "gpu"))]
    let gpu = || gpu_engine(backend, residual, (settings, adaptive), gpu_convergence);
    match choice {
        LocalComputeTarget::Cpu => (vec![cpu], false),
        // The GPU engine alone for both GPU choices: the pipeline deals batches to the engines in
        // turn, and a CPU batch costs 10 to 100 times a GPU batch. So the CPU's part of "CPU +
        // GPU" is the tree producer, which takes the CPU engine's threads
        // (`pipeline::gpu_producer_threads`). The renderer's own CPU engine still takes every
        // batch the GPU declines.
        LocalComputeTarget::CpuGpu | LocalComputeTarget::Gpu => gpu().map_or_else(
            || (vec![cpu], false),
            |(gpu, resident)| (vec![gpu], resident),
        ),
    }
}

/// The GPU beam engine of the export's backend, and whether it takes the GPU-resident convergence.
#[cfg(feature = "gpu")]
fn gpu_engine<'e>(
    backend: &'e GpuBackend,
    residual: &ResidualScene<'e>,
    (settings, adaptive): (&BeamSettings, &AdaptiveSettings),
    gpu_convergence: bool,
) -> (Box<dyn BeamEngine + 'e>, bool) {
    use pleochroma::renderer::gpu::beam::engine::GpuBeamEngine;

    let engine = GpuBeamEngine::new(backend, residual, settings, adaptive)
        .with_display_scale(CHECK_SCALE)
        .with_gpu_convergence(gpu_convergence);
    let resident = engine.uses_gpu_convergence();
    (Box::new(engine), resident)
}

/// A build without the `gpu` feature has no GPU beam engine.
#[cfg(not(feature = "gpu"))]
fn gpu_engine<'e>(
    _backend: &'e GpuBackend,
    _residual: &ResidualScene<'e>,
    _tuning: (&BeamSettings, &AdaptiveSettings),
    _gpu_convergence: bool,
) -> Option<(Box<dyn BeamEngine + 'e>, bool)> {
    None
}

/// Renders `job`'s frame from beams, with `taps.beam` as the settings.
///
/// `local_compute` picks this computer's engines (see the module doc). With remote compute chosen
/// and a worker that can take part, the frame's tiles are split between this computer and the
/// worker ([`remote`]); with `keep_leaves` the beams are kept and loaded ([`keep`]) and the render
/// stays on this computer. Returns [`BeamAttempt::Off`] without rendering when `taps.beam` is
/// `None`, the engine is Ray Traced, the residual settings break a rule (the ordinary render
/// reports that itself) or the stone is not eligible; the reason is queued on `ctx` as the first
/// note of the render that follows.
pub(super) fn render_beams_on_ctx(
    ctx: &ExportCtx<'_>,
    job: &RenderJob<'_>,
    local_compute: LocalComputeTarget,
    taps: &RenderTaps,
    cache: BeamLaneCache<'_>,
    report_progress: impl FnMut(ExportProgress),
) -> BeamAttempt {
    render_beams_inner(
        ctx,
        job,
        local_compute,
        taps,
        (None, Some(cache)),
        report_progress,
    )
}

/// [`render_beams_on_ctx`] with its remote lane already opened (the seam the tests use to put a
/// scripted worker in place of a socket); `None` opens the lane the job asks for, without the
/// carry's cached probe and connections.
#[cfg_attr(
    not(all(test, beam_tests)),
    expect(dead_code, reason = "the remote-lane tests inject their lane here")
)]
fn render_beams_with(
    ctx: &ExportCtx<'_>,
    job: &RenderJob<'_>,
    local_compute: LocalComputeTarget,
    taps: &RenderTaps,
    lane: Option<remote::BeamLane<'_>>,
    report_progress: impl FnMut(ExportProgress),
) -> BeamAttempt {
    render_beams_inner(ctx, job, local_compute, taps, (lane, None), report_progress)
}

/// The beam render behind [`render_beams_on_ctx`] and [`render_beams_with`]: `lane` is an
/// injected remote lane, `cache` the carry's probe and connections the job's lane is opened from;
/// with neither, the lane is opened afresh.
fn render_beams_inner(
    ctx: &ExportCtx<'_>,
    job: &RenderJob<'_>,
    local_compute: LocalComputeTarget,
    taps: &RenderTaps,
    (lane, cache): (Option<remote::BeamLane<'_>>, Option<BeamLaneCache<'_>>),
    report_progress: impl FnMut(ExportProgress),
) -> BeamAttempt {
    let Some(settings) = taps.beam else {
        return BeamAttempt::Off;
    };
    // A worker refuses what `validate` refuses, so the local render does too, with a note, instead
    // of rendering with values that force endless subdivision.
    if let Err(error) = settings.validate() {
        queue_note(ctx, &format!("Beam tracing off: {error}"));
        return BeamAttempt::Off;
    }
    let scene = ctx.scene;
    if scene.engine.is_ray() {
        queue_note(ctx, BeamOff::RayTracedResidual.message());
        return BeamAttempt::Off;
    }
    let total = job.samples_per_pixel;
    let adaptive = residual_settings(total, taps.adaptive);
    if adaptive.validate().is_err() {
        return BeamAttempt::Off;
    }
    let frame = frame_of(ctx);
    let extras: Extras<'_> = (&scene.tools, &scene.fluorescence, ctx.surface);
    if let Err(off) = eligible(&frame, extras, &settings) {
        if !off.is_silent() {
            queue_note(ctx, off.message());
        }
        return BeamAttempt::Off;
    }

    let plane_soa = build_plane_soa(&scene.active_planes);
    let prepared = PreparedMaterial::new(frame.material);
    let residual = ResidualScene {
        frame: &frame,
        extras,
        plane_soa: &plane_soa,
        prepared: &prepared,
        n_lambda: node_count(&settings),
        sample_offset: job.sample_offset,
        residual_min_spp: settings.residual_min_spp,
    };
    let (list, resident) = local_engines(
        local_compute,
        ctx.gpu,
        &residual,
        (&settings, &adaptive),
        taps.gpu_convergence,
    );
    // The engines were built over `prepared`; the renderer reuses it for its frame and fallback
    // engine, so the optics tables are built once for the export.
    let engines = EngineSet::with_prepared(list, &prepared);
    tracing::debug!(
        engines = ?engines.engines.iter().map(|engine| engine.name()).collect::<Vec<_>>(),
        width = ctx.width,
        height = ctx.height,
        samples_per_pixel = total,
        "export beams: rendering"
    );
    let inputs = Inputs {
        frame: &frame,
        extras,
        plane_soa: &plane_soa,
        settings,
        adaptive,
        engines: &engines,
        // Asked for, and no GPU engine took it: the check runs on the host.
        host_convergence: taps.gpu_convergence && !resident,
    };
    let keeper = keep::Keeper::new(&frame, &settings, &adaptive, taps);
    run_beams(
        (ctx, job, taps),
        &inputs,
        (keeper.as_ref(), lane, cache),
        report_progress,
    )
}

/// Everything one beam render reads, besides the export's context and job.
struct Inputs<'a, 'f, 'e> {
    /// The frame.
    frame: &'a FrameScene<'f>,
    /// Tools, fluorescence and surface finish.
    extras: Extras<'a>,
    /// The intersection arena of the frame's planes.
    plane_soa: &'a PlanesSoA32,
    /// The beam settings.
    settings: BeamSettings,
    /// The residual's adaptive settings (their `max_spp` is the render's samples per pixel).
    adaptive: AdaptiveSettings,
    /// The engines of the compute choice.
    engines: &'a EngineSet<'e>,
    /// `--gpu-convergence` was asked for and no GPU engine runs the check on the GPU.
    host_convergence: bool,
}

/// The hooks of one local run of the renderer's entry.
struct LocalHooks<'h> {
    /// Two-phase progress of the run.
    progress: ProgressHook<'h>,
    /// Each finished batch's tiles and final pixels.
    on_batch: BatchHook<'h>,
    /// Each finished batch's leaves and kept index (the leaf cache).
    retain: Option<&'h mut dyn FnMut(RetainedBatch)>,
}

/// One local run of the renderer's entry over `tiles` (the whole frame for `None`).
fn render_local(
    inputs: &Inputs<'_, '_, '_>,
    (cancel, sample_offset): (&AtomicBool, u32),
    tiles: Option<&LiveTiles>,
    hooks: LocalHooks<'_>,
) -> Result<(AdaptiveImage, BeamStats), BeamOff> {
    render_beam_hybrid_surface(
        inputs.frame,
        inputs.extras,
        inputs.plane_soa,
        &inputs.settings,
        &inputs.adaptive,
        inputs.engines,
        (sample_offset, CHECK_SCALE),
        cancel,
        BeamRun {
            tiles,
            progress: hooks.progress,
            on_batch: Some(hooks.on_batch),
            // `&mut` is invariant, so an optional hook keeps its own lifetime unless it is
            // coerced to the run's one here (a direct field coerces by itself).
            retain: hooks
                .retain
                .map(|retain| retain as &mut dyn FnMut(RetainedBatch)),
            preloaded: None,
        },
    )
}

/// The sentences of `parts` that exist, joined by a space.
fn join_notes<const N: usize>(parts: [Option<String>; N]) -> Option<String> {
    let text = parts.into_iter().flatten().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

/// The opening report of a beam render of `total` samples per pixel: nothing traced yet, the
/// tracing phase, and `note` as the one-off status line.
const fn tracing_report(total: u32, note: Option<String>) -> ExportProgress {
    ExportProgress {
        fraction: 0.0,
        samples_done: 0,
        samples_total: total,
        preview: None,
        note,
        beam: Some(BeamStatus {
            phase: BeamPhase::Tracing,
            eta: None,
        }),
    }
}

/// Renders the frame from the beams kept on disk: loads them and hands them to the renderer as
/// preloaded beams, so the trees of the kept batches are not traced. `None` when nothing usable is
/// kept (the full render follows). The fragments and the adaptive residual run as in a fresh
/// render, so the image equals the fresh one.
fn render_from_kept(
    (ctx, job, taps): (&ExportCtx<'_>, &RenderJob<'_>, &RenderTaps),
    inputs: &Inputs<'_, '_, '_>,
    keeper: &keep::Keeper,
    report_progress: &mut impl FnMut(ExportProgress),
) -> Option<BeamAttempt> {
    let mut cache = keeper.load()?;
    let total = job.samples_per_pixel;
    // The same opening notes as the fresh render that stored the beams: a kept render is always on
    // this computer (`not_local` false) and, under remote compute, says so.
    report_progress(tracing_report(
        total,
        join_notes([
            start_note(taps.path_cache.is_some(), inputs.host_convergence, false),
            remote::BeamLane::kept(job.compute_target).note,
        ]),
    ));
    let reporter = Reporter::new((ctx.width, ctx.height), total);
    let mut forward = |p: BeamProgress| {
        reporter.forward(p.phase, p.done, p.eta, report_progress);
    };
    let mut on_batch = |tiles: &[TileRect], finished: &[Vec3]| {
        reporter.on_batch(tiles, finished);
    };
    let rendered = keep::render_preloaded(
        inputs,
        &cache,
        (job.cancel, job.sample_offset),
        (&mut forward, &mut on_batch),
    );
    if job.cancel.load(Ordering::Acquire) {
        return Some(BeamAttempt::Cancelled);
    }
    let keep::Rerendered {
        image,
        stats,
        fresh,
    } = rendered.ok()?;
    if !fresh.is_empty() {
        // Tiles the kept beams did not cover were traced now: keep them too.
        for batch in fresh {
            cache.retain(batch);
        }
        keeper.store(&cache);
    }
    let (width, height) = (inputs.frame.width, inputs.frame.height);
    let share = residual_share(image.samples_traced, (width, height), total);
    let done = accumulation(image, total, inputs.adaptive.tile);
    let samples_done = done.adaptive.as_ref().map_or(total, |s| s.samples_done);
    report_progress(ExportProgress {
        fraction: 1.0,
        samples_done,
        samples_total: total,
        preview: None,
        note: Some(format!("{}. {NOTE_KEPT}", beam_note(stats.leaves, share))),
        beam: Some(BeamStatus {
            phase: BeamPhase::Finishing,
            eta: Some(Duration::ZERO),
        }),
    });
    Some(BeamAttempt::Done(done))
}

/// The lane of a render: the injected one, the local lane of a render that keeps its beams, or the
/// lane the job asks for (from the carry's cached probe and connections when there is a `cache`).
///
/// # Errors
///
/// The message that fails the export when the asked-for lane cannot be opened.
fn lane_for<'l>(
    job: &RenderJob<'_>,
    keeps_beams: bool,
    (injected, cache): (Option<remote::BeamLane<'l>>, Option<BeamLaneCache<'l>>),
) -> Result<remote::BeamLane<'l>, String> {
    match (injected, cache) {
        (Some(lane), _) => Ok(lane),
        (None, _) if keeps_beams => Ok(remote::BeamLane::kept(job.compute_target)),
        (None, Some(cache)) => remote::open_cached(job, cache),
        (None, None) => remote::open(job),
    }
}

/// The opening report of a beam render of `total` samples per pixel: the notes about the choices
/// beams overrode (`taps`, `inputs`) and about the lane, which are said once.
fn opening_report(
    total: u32,
    (taps, inputs): (&RenderTaps, &Inputs<'_, '_, '_>),
    lane: &mut remote::BeamLane<'_>,
) -> ExportProgress {
    tracing_report(
        total,
        join_notes([
            start_note(
                taps.path_cache.is_some(),
                inputs.host_convergence,
                lane.not_local,
            ),
            lane.note.take(),
        ]),
    )
}

/// Runs the beam render over `inputs` and turns its progress and its image into the export's: the
/// reports, the thumbnail and the [`Accumulation`]. Kept beams are tried first; a render that
/// keeps its beams stores them at the end.
fn run_beams(
    (ctx, job, taps): (&ExportCtx<'_>, &RenderJob<'_>, &RenderTaps),
    inputs: &Inputs<'_, '_, '_>,
    (keeper, injected, cache): (
        Option<&keep::Keeper>,
        Option<remote::BeamLane<'_>>,
        Option<BeamLaneCache<'_>>,
    ),
    mut report_progress: impl FnMut(ExportProgress),
) -> BeamAttempt {
    if let Some(keeper) = keeper
        && let Some(attempt) =
            render_from_kept((ctx, job, taps), inputs, keeper, &mut report_progress)
    {
        return attempt;
    }
    // The row leaves "Starting" before the probe and the lane's handshakes; the opening report
    // below carries the notes.
    report_progress(tracing_report(job.samples_per_pixel, None));
    let mut lane = match lane_for(job, keeper.is_some(), (injected, cache)) {
        Ok(lane) => lane,
        Err(message) => return BeamAttempt::Failed(message),
    };
    let total = job.samples_per_pixel;
    report_progress(opening_report(total, (taps, inputs), &mut lane));
    let reporter = Reporter::new((ctx.width, ctx.height), total);
    let mut forward = |phase: BeamPhase, done: f32, eta: Option<Duration>| {
        reporter.forward(phase, done, eta, &mut report_progress);
    };
    let mut on_batch = |tiles: &[TileRect], finished: &[Vec3]| {
        reporter.on_batch(tiles, finished);
    };
    let mut cache = keeper.map(|_| LeafCache::new(inputs.settings.leaf_cache_bytes));
    let mut keep_batch = |batch: RetainedBatch| {
        if let Some(cache) = cache.as_mut() {
            cache.retain(batch);
        }
    };
    let retain: Option<&mut dyn FnMut(RetainedBatch)> = keeper
        .is_some()
        .then_some(&mut keep_batch as &mut dyn FnMut(RetainedBatch));
    let rendered = remote::render_shares(
        inputs,
        (job.cancel, job.sample_offset),
        &mut lane,
        remote::ShareHooks {
            progress: &mut forward,
            on_batch: &mut on_batch,
            retain,
        },
    );
    // A cancelled render returns what it finished: never use it.
    if job.cancel.load(Ordering::Acquire) {
        return BeamAttempt::Cancelled;
    }
    let (image, stats, lost) = match rendered {
        remote::Shares::Done {
            image,
            stats,
            notes,
        } => (*image, *stats, notes),
        remote::Shares::Cancelled => return BeamAttempt::Cancelled,
        remote::Shares::Off(off) => {
            if !off.is_silent() {
                queue_note(ctx, off.message());
            }
            return BeamAttempt::Off;
        }
        remote::Shares::Failed(message) => return BeamAttempt::Failed(message),
    };
    if let (Some(keeper), Some(cache)) = (keeper, cache.as_ref()) {
        keeper.store(cache);
    }

    finish_render(
        (image, stats, lost),
        (ctx.width, ctx.height),
        (total, inputs.adaptive.tile),
        &mut report_progress,
    )
}

/// The closing report of a beam render and its [`Accumulation`]; `lost` are the sentences about
/// workers that stopped on the way.
fn finish_render(
    (image, stats, lost): (AdaptiveImage, BeamStats, Vec<String>),
    (width, height): (u32, u32),
    (total, tile): (u32, u32),
    report_progress: &mut impl FnMut(ExportProgress),
) -> BeamAttempt {
    let share = residual_share(image.samples_traced, (width, height), total);
    let done = accumulation(image, total, tile);
    let samples_done = done.adaptive.as_ref().map_or(total, |s| s.samples_done);
    let closing = beam_note(stats.leaves, share);
    report_progress(ExportProgress {
        fraction: 1.0,
        samples_done,
        samples_total: total,
        preview: None,
        note: Some(if lost.is_empty() {
            closing
        } else {
            format!("{closing}. {}", lost.join(" "))
        }),
        beam: Some(BeamStatus {
            phase: BeamPhase::Finishing,
            eta: Some(Duration::ZERO),
        }),
    });
    BeamAttempt::Done(done)
}

#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_keep;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_remote;
