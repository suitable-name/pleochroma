//! Injectable engines for the stress and failure tests: decline, lose the device or cancel at a
//! chosen batch, pass or dispatch.
//!
//! [`Injected`] wraps any [`BeamEngine`] and fires one [`Fault`] at a [`Trigger`]: a decline (a
//! gone remote worker, a refused cap), a lost device (the engine declines this and every later
//! request) or a cancel (the hook sets the render's token and answers `Cancelled`). An
//! [`InjectionLog`] outlives the engine and tells the test what was asked and what fired. The rest
//! of the file is the shared harness of `tests_d7_stress` and `tests_d7_failures`: a [`Bench`] that
//! renders through the hybrid entry with recording hooks, and the checks every row makes (every
//! pixel written exactly once, the image bits).

use std::{
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use glam::Vec3;

use super::{
    BeamOff, BeamProgress, BeamRun, BeamSettings,
    engines::{
        Batch, BatchFragments, BatchResidual, BeamEngine, CpuBeamEngine, EngineOutcome, EngineSet,
    },
    leaf::node_count,
    pipeline::tile_list,
    primary::TileRect,
    stats::BeamStats,
    tests_support::BeamRig,
    tile_residual::ResidualScene,
};
use crate::{
    optics::{
        fluorescence::Fluorescence,
        raytracer::{surface::SurfaceScene, transport::PreparedMaterial},
    },
    renderer::{
        adaptive::{AdaptiveImage, AdaptiveSettings, LiveTiles, NoiseMetric},
        cpu_frame::render_beam_hybrid_surface,
        frame_scene::FrameScene,
    },
};

/// The decline reason of an engine whose device is lost.
pub const REASON_DEVICE_LOST: &str = "test: device lost";

/// The decline reason of an engine asked for more than its cap.
pub const REASON_OVER_CAP: &str = "test: the batch exceeds the engine's cap";

/// Which stage of a batch a trigger watches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// The fragment stage (`BeamEngine::fragments`).
    Fragments,
    /// The residual stage (`BeamEngine::residual`).
    Residual,
}

impl Stage {
    /// The index of the stage's call counter.
    const fn slot(self) -> usize {
        match self {
            Self::Fragments => 0,
            Self::Residual => 1,
        }
    }
}

/// When a fault fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// The first request of `stage` for a batch whose ordinal is at least `batch`.
    Batch {
        /// The smallest batch ordinal that fires.
        batch: u32,
        /// The stage that fires.
        stage: Stage,
    },
    /// The `nth` request of `stage` this engine receives (0-based): the dispatch count.
    Call {
        /// The request number that fires.
        nth: u32,
        /// The stage that fires.
        stage: Stage,
    },
}

/// What an injected fault does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The engine declines the request with this reason (a gone worker, a refused cap).
    Decline(&'static str),
    /// The device is lost: this request and every later one is declined.
    LoseDevice,
    /// The engine sets the render's cancel token and answers `Cancelled`.
    Cancel,
}

/// What an [`Injected`] engine saw and did; it outlives the engine so a test can read it.
#[derive(Debug, Default)]
pub struct InjectionLog {
    /// Requests per stage, in `Stage::slot` order.
    calls: [AtomicU32; 2],
    /// Requests the engine declined (an injected fault, a lost device, a cap).
    declines: AtomicU32,
    /// Whether the fault fired.
    fired: AtomicBool,
    /// Whether the device was lost.
    lost: AtomicBool,
    /// When a `Cancel` fault set the token.
    fired_at: Mutex<Option<Instant>>,
}

impl InjectionLog {
    /// A log with nothing recorded.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The fragment requests the engine received.
    #[must_use]
    pub fn fragment_calls(&self) -> u32 {
        self.calls[Stage::Fragments.slot()].load(Ordering::Relaxed)
    }

    /// The residual requests the engine received.
    #[must_use]
    pub fn residual_calls(&self) -> u32 {
        self.calls[Stage::Residual.slot()].load(Ordering::Relaxed)
    }

    /// The requests the engine declined.
    #[must_use]
    pub fn declines(&self) -> u32 {
        self.declines.load(Ordering::Relaxed)
    }

    /// `true` once the fault fired.
    #[must_use]
    pub fn fired(&self) -> bool {
        self.fired.load(Ordering::Relaxed)
    }

    /// `true` once the device was lost.
    #[must_use]
    pub fn lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }

    /// When a `Cancel` fault set the token; `None` if it did not fire.
    #[must_use]
    pub fn fired_at(&self) -> Option<Instant> {
        *self.fired_at.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// An engine that wraps another and misbehaves once, at a chosen point.
pub struct Injected<'e> {
    /// The engine that does the work when the hook lets a request through.
    inner: Box<dyn BeamEngine + 'e>,
    /// The name for the log line.
    label: String,
    /// The fault and where it fires; `None` for a pass-through wrapper.
    plan: Option<(Trigger, Fault)>,
    /// Batches with more pixels than this are declined (a GPU storage cap); `None` for no cap.
    pixel_cap: Option<usize>,
    /// The render's token, set by a `Cancel` fault.
    token: Option<&'e AtomicBool>,
    /// Where the engine reports.
    log: &'e InjectionLog,
}

impl<'e> Injected<'e> {
    /// A pass-through wrapper of `inner` named `label` that reports to `log`.
    #[must_use]
    pub fn new(inner: impl BeamEngine + 'e, label: &str, log: &'e InjectionLog) -> Self {
        Self {
            inner: Box::new(inner),
            label: label.to_owned(),
            plan: None,
            pixel_cap: None,
            token: None,
            log,
        }
    }

    /// The wrapper with `fault` firing at `trigger`.
    #[must_use]
    pub const fn with_fault(mut self, trigger: Trigger, fault: Fault) -> Self {
        self.plan = Some((trigger, fault));
        self
    }

    /// The wrapper declining every batch of more than `pixels` pixels (the GPU storage-binding cap).
    #[must_use]
    pub const fn with_pixel_cap(mut self, pixels: usize) -> Self {
        self.pixel_cap = Some(pixels);
        self
    }

    /// The wrapper with the render's cancel token, which a `Cancel` fault sets.
    #[must_use]
    pub const fn with_cancel_token(mut self, token: &'e AtomicBool) -> Self {
        self.token = Some(token);
        self
    }

    /// `true` when the plan's trigger matches this request.
    fn matches(&self, stage: Stage, nth: u32, batch: &Batch<'_>) -> bool {
        match self.plan {
            Some((
                Trigger::Batch {
                    batch: from,
                    stage: s,
                },
                _,
            )) => s == stage && batch.index >= from,
            Some((
                Trigger::Call {
                    nth: wanted,
                    stage: s,
                },
                _,
            )) => s == stage && nth == wanted,
            None => false,
        }
    }

    /// Counts a decline and answers it.
    fn decline<T>(&self, reason: &'static str) -> EngineOutcome<T> {
        self.log.declines.fetch_add(1, Ordering::Relaxed);
        EngineOutcome::Declined(reason)
    }

    /// The answer the hook gives instead of the inner engine's, if any.
    fn intercept<T>(&self, stage: Stage, batch: &Batch<'_>) -> Option<EngineOutcome<T>> {
        let nth = self.log.calls[stage.slot()].fetch_add(1, Ordering::Relaxed);
        if self.log.lost() {
            return Some(self.decline(REASON_DEVICE_LOST));
        }
        if self.pixel_cap.is_some_and(|cap| batch.pixel_count() > cap) {
            return Some(self.decline(REASON_OVER_CAP));
        }
        let (_, fault) = self.plan?;
        if self.log.fired() || !self.matches(stage, nth, batch) {
            return None;
        }
        self.log.fired.store(true, Ordering::Relaxed);
        Some(match fault {
            Fault::Decline(reason) => self.decline(reason),
            Fault::LoseDevice => {
                self.log.lost.store(true, Ordering::Relaxed);
                self.decline(REASON_DEVICE_LOST)
            }
            Fault::Cancel => {
                *self
                    .log
                    .fired_at
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
                if let Some(token) = self.token {
                    token.store(true, Ordering::Relaxed);
                }
                EngineOutcome::Cancelled
            }
        })
    }
}

impl BeamEngine for Injected<'_> {
    fn name(&self) -> &str {
        &self.label
    }

    fn fragments(&self, batch: &Batch<'_>, cancel: &AtomicBool) -> EngineOutcome<BatchFragments> {
        if let Some(outcome) = self.intercept(Stage::Fragments, batch) {
            return outcome;
        }
        self.inner.fragments(batch, cancel)
    }

    fn residual(
        &self,
        batch: &Batch<'_>,
        offset: &[Vec3],
        cancel: &AtomicBool,
    ) -> EngineOutcome<BatchResidual> {
        self.residual_with_progress(batch, offset, cancel, &mut |_| {})
    }

    fn residual_with_progress(
        &self,
        batch: &Batch<'_>,
        offset: &[Vec3],
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(f32),
    ) -> EngineOutcome<BatchResidual> {
        if let Some(outcome) = self.intercept(Stage::Residual, batch) {
            return outcome;
        }
        self.inner
            .residual_with_progress(batch, offset, cancel, progress)
    }

    fn fragment_stats(&self) -> BeamStats {
        self.inner.fragment_stats()
    }

    fn needs_packed(&self) -> bool {
        self.inner.needs_packed()
    }

    fn shares_cpu_threads(&self) -> bool {
        self.inner.shares_cpu_threads()
    }
}

// ---- the shared harness ----

/// The extras of a plain stone.
#[must_use]
pub fn plain_extras() -> (
    &'static [crate::geometry::tool::ToolPrimitive],
    &'static Fluorescence,
    SurfaceScene<'static>,
) {
    (&[], Fluorescence::none(), SurfaceScene::IDEAL)
}

/// Adaptive settings whose tiles never converge: every pixel gets exactly `spp` samples, in passes
/// of `pass_spp`, on tiles of `tile` pixels.
#[must_use]
pub const fn fixed_settings(spp: u32, pass_spp: u32, tile: u32) -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise: 1.0e-6,
        metric: NoiseMetric::AbsoluteDeltaE,
        min_spp: spp,
        max_spp: spp,
        pass_spp,
        tile,
    }
}

/// The beam settings of the rows: `batch_tiles` tiles per batch and `spp` residual samples.
#[must_use]
pub fn beam_settings(batch_tiles: u32, spp: u32) -> BeamSettings {
    BeamSettings {
        residual_min_spp: spp,
        batch_tiles,
        ..BeamSettings::default()
    }
}

/// What a recorded render returned.
pub struct Run {
    /// The image.
    pub image: AdaptiveImage,
    /// The counters.
    pub stats: BeamStats,
    /// Every progress report, in order.
    pub progress: Vec<BeamProgress>,
    /// Every `on_batch` call: the piece's tiles and their final pixels.
    pub batches: Vec<(Vec<TileRect>, Vec<Vec3>)>,
    /// How long the call took.
    pub elapsed: Duration,
    /// When the call returned.
    pub returned_at: Instant,
}

/// A frame with its settings and the prepared material the engines borrow.
///
/// Logs, flags and tokens the engines point at must be declared before the bench, so they outlive
/// the engines it builds.
pub struct Bench<'r> {
    /// The rig (the stone, its arena and camera).
    pub rig: &'r BeamRig,
    /// The frame.
    pub scene: &'r FrameScene<'r>,
    /// The prepared material of the residual.
    prepared: PreparedMaterial<'r>,
    /// The beam settings.
    pub beam: BeamSettings,
    /// The adaptive settings.
    pub adaptive: AdaptiveSettings,
}

impl<'r> Bench<'r> {
    /// A bench for `scene` with the given settings.
    #[must_use]
    pub fn new(
        rig: &'r BeamRig,
        scene: &'r FrameScene<'r>,
        beam: BeamSettings,
        adaptive: AdaptiveSettings,
    ) -> Self {
        Self {
            rig,
            scene,
            prepared: PreparedMaterial::new(&rig.material),
            beam,
            adaptive,
        }
    }

    /// A CPU engine of the frame on `threads` workers.
    #[must_use]
    pub fn cpu(&self, threads: usize) -> CpuBeamEngine<'_> {
        let residual = ResidualScene {
            frame: self.scene,
            extras: plain_extras(),
            plane_soa: &self.rig.soa,
            prepared: &self.prepared,
            n_lambda: node_count(&self.beam),
            sample_offset: 0,
            residual_min_spp: self.beam.residual_min_spp,
        };
        CpuBeamEngine::new(&residual, &self.beam, &self.adaptive).with_threads(threads)
    }

    /// The hybrid entry with `engines`, recording the progress and `on_batch` hooks.
    ///
    /// # Errors
    ///
    /// The [`BeamOff`] reason the frame is not beam traced.
    pub fn render(
        &self,
        engines: &EngineSet<'_>,
        restrict: Option<&LiveTiles>,
        cancel: &AtomicBool,
    ) -> Result<Run, BeamOff> {
        let started = Instant::now();
        let mut progress_log = Vec::new();
        let mut batches = Vec::new();
        let result = {
            let mut progress = |report: BeamProgress| progress_log.push(report);
            let mut on_batch = |tiles: &[TileRect], means: &[Vec3]| {
                batches.push((tiles.to_vec(), means.to_vec()));
            };
            render_beam_hybrid_surface(
                self.scene,
                plain_extras(),
                &self.rig.soa,
                &self.beam,
                &self.adaptive,
                engines,
                (0, 1.0),
                cancel,
                BeamRun {
                    tiles: restrict,
                    progress: &mut progress,
                    on_batch: Some(&mut on_batch),
                    retain: None,
                    preloaded: None,
                },
            )
        };
        let returned_at = Instant::now();
        let (image, stats) = result?;
        Ok(Run {
            image,
            stats,
            progress: progress_log,
            batches,
            elapsed: returned_at.saturating_duration_since(started),
            returned_at,
        })
    }
}

/// An engine boxed for an `EngineSet`.
#[must_use]
pub fn boxed<'e>(engine: impl BeamEngine + 'e) -> Box<dyn BeamEngine + 'e> {
    Box::new(engine)
}

/// The bit patterns of a vector image.
#[must_use]
pub fn vec_bits(values: &[Vec3]) -> Vec<[u32; 3]> {
    values
        .iter()
        .map(|v| v.to_array().map(f32::to_bits))
        .collect()
}

/// An image as comparable bits: sums, odd sums, counts, pixels and the sample total.
pub type ImageBits = (Vec<[u32; 3]>, Vec<[u32; 3]>, Vec<u32>, Vec<[u32; 3]>, u64);

/// The bits of every field of `image`.
#[must_use]
pub fn image_bits(image: &AdaptiveImage) -> ImageBits {
    (
        vec_bits(&image.sum),
        vec_bits(&image.half_b),
        image.counts.clone(),
        vec_bits(&image.pixels),
        image.samples_traced,
    )
}

/// How many `on_batch` pieces wrote each pixel of the `size` frame, row-major (saturating at 255).
#[must_use]
pub fn written_counts(run: &Run, size: (u32, u32)) -> Vec<u8> {
    let mut counts = vec![0_u8; size.0 as usize * size.1 as usize];
    for (tiles, _) in &run.batches {
        for rect in tiles {
            for y in rect.y0..rect.y1.min(size.1) {
                for x in rect.x0..rect.x1.min(size.0) {
                    if let Some(count) = counts.get_mut(y as usize * size.0 as usize + x as usize) {
                        *count = count.saturating_add(1);
                    }
                }
            }
        }
    }
    counts
}

/// Asserts that no pixel was written twice and that every pixel nothing wrote is still zero in the
/// image (a cancelled render's partial image). Returns the number of pixels written.
///
/// # Panics
///
/// When a pixel was written more than once or an unwritten pixel is not zero.
#[must_use]
pub fn assert_written_at_most_once(run: &Run, size: (u32, u32)) -> usize {
    let counts = written_counts(run, size);
    let mut written = 0_usize;
    for (index, &count) in counts.iter().enumerate() {
        assert!(count <= 1, "pixel {index} was written {count} times");
        if count == 1 {
            written += 1;
        } else {
            assert_eq!(
                run.image.pixels[index],
                Vec3::ZERO,
                "unwritten pixel {index}"
            );
            assert_eq!(run.image.sum[index], Vec3::ZERO, "unwritten sum {index}");
            assert_eq!(run.image.counts[index], 0, "unwritten count {index}");
        }
    }
    written
}

/// Asserts the no-lost-tile property of a finished render: every pixel of the tiles to render
/// (`restrict`, or all) was written exactly once and every other pixel not at all.
///
/// # Panics
///
/// When a pixel of a rendered tile was not written exactly once, or another was written.
pub fn assert_written_exactly_once(
    run: &Run,
    size: (u32, u32),
    tile: u32,
    restrict: Option<&LiveTiles>,
) {
    let members = tile_list(size, tile, restrict);
    let columns = size.0.div_ceil(tile.max(1));
    let counts = written_counts(run, size);
    for y in 0..size.1 {
        for x in 0..size.0 {
            let index = (y / tile) * columns + x / tile;
            let expected = u8::from(members.binary_search(&index).is_ok());
            let seen = counts[y as usize * size.0 as usize + x as usize];
            assert_eq!(seen, expected, "pixel ({x}, {y}) of tile {index}");
        }
    }
}

/// The next value of a xorshift64* sequence (the random points of the cancel rows, reproducible).
#[must_use]
pub const fn next_random(state: u64) -> u64 {
    let mut x = if state == 0 {
        0x9E37_79B9_7F4A_7C15
    } else {
        state
    };
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::beam::beam::{PackedKept, PackedLeaves};

    /// An engine that answers every request with zeros of the right length.
    struct Null;

    impl BeamEngine for Null {
        fn name(&self) -> &'static str {
            "null"
        }

        fn fragments(&self, batch: &Batch<'_>, _: &AtomicBool) -> EngineOutcome<BatchFragments> {
            EngineOutcome::Done(BatchFragments {
                xyz: vec![Vec3::ZERO; batch.pixel_count()],
            })
        }

        fn residual(
            &self,
            batch: &Batch<'_>,
            _: &[Vec3],
            _: &AtomicBool,
        ) -> EngineOutcome<BatchResidual> {
            let n = batch.pixel_count();
            EngineOutcome::Done(BatchResidual {
                sum: vec![Vec3::ZERO; n],
                half_b: vec![Vec3::ZERO; n],
                counts: vec![0; n],
                samples_traced: 0,
            })
        }
    }

    /// The tiles every hook test uses: one 8 x 8 tile.
    const TILES: [TileRect; 1] = [TileRect {
        x0: 0,
        y0: 0,
        x1: 8,
        y1: 8,
    }];

    /// Runs `body` with the batch of ordinal `index`.
    fn with_batch<R>(index: u32, body: impl FnOnce(&Batch<'_>) -> R) -> R {
        let packed_leaves = PackedLeaves::default();
        let packed_kept = PackedKept::default();
        let batch = Batch {
            index,
            tiles: &TILES,
            leaves: &[],
            kept: &[],
            packed_leaves: &packed_leaves,
            packed_kept: &packed_kept,
        };
        body(&batch)
    }

    /// `true` for a `Done` answer.
    fn is_done<T>(outcome: &EngineOutcome<T>) -> bool {
        matches!(outcome, EngineOutcome::Done(_))
    }

    #[test]
    fn a_pass_through_wrapper_forwards_and_counts() {
        let log = InjectionLog::new();
        let engine = Injected::new(Null, "wrapped", &log);
        let cancel = AtomicBool::new(false);
        assert_eq!(engine.name(), "wrapped");
        assert!(engine.needs_packed());
        with_batch(0, |batch| {
            assert!(is_done(&engine.fragments(batch, &cancel)));
            assert!(is_done(&engine.residual(batch, &[], &cancel)));
        });
        assert_eq!(log.fragment_calls(), 1);
        assert_eq!(log.residual_calls(), 1);
        assert_eq!(log.declines(), 0);
        assert!(!log.fired());
        assert!(!log.lost());
        assert_eq!(log.fired_at(), None);
    }

    #[test]
    fn a_batch_trigger_declines_once_at_the_chosen_ordinal() {
        let log = InjectionLog::new();
        let fault = Fault::Decline("test: gone");
        let trigger = Trigger::Batch {
            batch: 2,
            stage: Stage::Residual,
        };
        let engine = Injected::new(Null, "flaky", &log).with_fault(trigger, fault);
        let cancel = AtomicBool::new(false);
        with_batch(1, |batch| {
            assert!(is_done(&engine.residual(batch, &[], &cancel)));
        });
        // The fragment stage of batch 2 is not watched.
        with_batch(2, |batch| {
            assert!(is_done(&engine.fragments(batch, &cancel)));
            assert_eq!(
                engine.residual(batch, &[], &cancel),
                EngineOutcome::Declined("test: gone")
            );
        });
        assert!(log.fired());
        assert_eq!(log.declines(), 1);
        assert!(!log.lost());
        // It fires once: a later request passes.
        with_batch(3, |batch| {
            assert!(is_done(&engine.residual(batch, &[], &cancel)));
        });
        assert_eq!(log.declines(), 1);
    }

    #[test]
    fn a_call_trigger_counts_dispatches_of_its_stage() {
        let log = InjectionLog::new();
        let trigger = Trigger::Call {
            nth: 2,
            stage: Stage::Fragments,
        };
        let engine =
            Injected::new(Null, "third", &log).with_fault(trigger, Fault::Decline("test: third"));
        let cancel = AtomicBool::new(false);
        with_batch(7, |batch| {
            assert!(is_done(&engine.fragments(batch, &cancel)));
            assert!(is_done(&engine.residual(batch, &[], &cancel)));
            assert!(is_done(&engine.fragments(batch, &cancel)));
            assert_eq!(
                engine.fragments(batch, &cancel),
                EngineOutcome::Declined("test: third")
            );
        });
        assert_eq!(log.fragment_calls(), 3);
        assert_eq!(log.residual_calls(), 1);
    }

    #[test]
    fn a_lost_device_declines_this_and_every_later_request() {
        let log = InjectionLog::new();
        let trigger = Trigger::Batch {
            batch: 0,
            stage: Stage::Residual,
        };
        let engine = Injected::new(Null, "gpu", &log).with_fault(trigger, Fault::LoseDevice);
        let cancel = AtomicBool::new(false);
        with_batch(0, |batch| {
            assert!(is_done(&engine.fragments(batch, &cancel)));
            assert_eq!(
                engine.residual(batch, &[], &cancel),
                EngineOutcome::Declined(REASON_DEVICE_LOST)
            );
            assert_eq!(
                engine.fragments(batch, &cancel),
                EngineOutcome::Declined(REASON_DEVICE_LOST)
            );
        });
        assert!(log.lost());
        assert_eq!(log.declines(), 2);
    }

    #[test]
    fn a_cancel_fault_sets_the_token_and_stamps_the_time() {
        let log = InjectionLog::new();
        let token = AtomicBool::new(false);
        let trigger = Trigger::Batch {
            batch: 1,
            stage: Stage::Fragments,
        };
        let engine = Injected::new(Null, "cancelling", &log)
            .with_fault(trigger, Fault::Cancel)
            .with_cancel_token(&token);
        with_batch(0, |batch| {
            assert!(is_done(&engine.fragments(batch, &token)));
        });
        assert!(!token.load(Ordering::Relaxed));
        with_batch(1, |batch| {
            assert_eq!(engine.fragments(batch, &token), EngineOutcome::Cancelled);
        });
        assert!(token.load(Ordering::Relaxed));
        assert!(log.fired());
        assert!(log.fired_at().is_some());
        assert_eq!(log.declines(), 0);
    }

    #[test]
    fn a_pixel_cap_declines_big_batches_only() {
        let log = InjectionLog::new();
        let cancel = AtomicBool::new(false);
        let roomy = Injected::new(Null, "roomy", &log).with_pixel_cap(64);
        let tight = Injected::new(Null, "tight", &log).with_pixel_cap(63);
        with_batch(0, |batch| {
            assert_eq!(batch.pixel_count(), 64);
            assert!(is_done(&roomy.fragments(batch, &cancel)));
            assert_eq!(
                tight.fragments(batch, &cancel),
                EngineOutcome::Declined(REASON_OVER_CAP)
            );
        });
        assert_eq!(log.declines(), 1);
        assert!(!log.fired(), "a cap is not the fault");
    }

    #[test]
    fn the_random_sequence_is_reproducible_and_never_stuck() {
        let mut state = 1_u64;
        let mut seen = Vec::new();
        for _ in 0..8 {
            state = next_random(state);
            seen.push(state);
        }
        let mut again = 1_u64;
        for &value in &seen {
            again = next_random(again);
            assert_eq!(again, value);
        }
        assert!(seen.windows(2).all(|pair| pair[0] != pair[1]));
        assert_ne!(next_random(0), 0);
    }
}
