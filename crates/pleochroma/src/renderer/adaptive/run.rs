//! A deterministic driver for adaptive sampling, free of threads and timing.
//!
//! The caller (the CPU frame loop, the export thread, the GPU or a remote coordinator) asks for
//! the next [`Pass`], traces it into the accumulators exactly as the pass contract in the parent
//! module says, and reports back with [`AdaptiveRun::finish_pass`]. The driver owns every
//! decision: pass sizes, when to check, which tiles stop. Decisions are made only between
//! passes and depend only on the accumulated sums, so the same inputs give the same image.

use glam::Vec3;

use super::{
    AdaptiveSettings, AdaptiveSettingsError, LiveTiles,
    map::ConvergenceMap,
    metric::{count_f32, live_tile_noises_with_offset},
};

/// One pass to trace: `spp` samples starting at the absolute index `first_sample`, for `live`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pass {
    /// Absolute index of the pass's first sample.
    pub first_sample: u32,
    /// Samples per pixel in this pass; the last pass of a run may be shorter.
    pub spp: u32,
    /// The tiles that receive the samples.
    pub live: LiveTiles,
}

/// The convergence check due after the pending pass: which tiles to measure and at what counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingCheck {
    /// The live tiles to measure (the ones the pass adds samples to), in ascending order.
    pub live: LiveTiles,
    /// Samples per pixel every live pixel holds once the pass is closed.
    pub samples: u32,
    /// How many of those samples had an odd absolute index (half-buffer B's count).
    pub odd_samples: u32,
}

/// The result of a finished adaptive render.
#[derive(Clone, Debug, PartialEq)]
pub struct AdaptiveImage {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Summed samples per pixel.
    pub sum: Vec<Vec3>,
    /// Summed odd-index samples per pixel.
    pub half_b: Vec<Vec3>,
    /// Samples each pixel received.
    pub counts: Vec<u32>,
    /// Per-pixel means: each pixel's sum divided by its own count.
    pub pixels: Vec<Vec3>,
    /// Samples traced over the whole frame (the sum of `counts`).
    pub samples_traced: u64,
}

/// Convergence checks per doubling of the sample count in a growing run past its minimum.
///
/// A tile then stops at most an eighth of its sample count later than with a check after every
/// pass.
const CHECKS_PER_OCTAVE: u32 = 8;

/// Number of odd absolute sample indices in `first..first + count`.
const fn odd_samples(first: u32, count: u32) -> u32 {
    if first.is_multiple_of(2) {
        count / 2
    } else {
        count.div_ceil(2)
    }
}

/// The pass loop of one adaptive render.
#[derive(Clone, Debug)]
pub struct AdaptiveRun {
    settings: AdaptiveSettings,
    width: u32,
    height: u32,
    map: ConvergenceMap,
    sample_offset: u32,
    done: u32,
    pending: Option<u32>,
    growing: bool,
}

impl AdaptiveRun {
    /// Starts a run over a `width` x `height` frame with every tile live.
    ///
    /// # Errors
    ///
    /// Returns the rule `settings` breaks, see [`AdaptiveSettings::validate`].
    pub fn new(
        settings: AdaptiveSettings,
        width: u32,
        height: u32,
    ) -> Result<Self, AdaptiveSettingsError> {
        settings.validate()?;
        Ok(Self {
            settings,
            width,
            height,
            map: ConvergenceMap::new(width, height, settings.tile),
            sample_offset: 0,
            done: 0,
            pending: None,
            growing: false,
        })
    }

    /// Makes the run start at the absolute sample index `offset` instead of 0.
    #[must_use]
    pub const fn with_sample_offset(mut self, offset: u32) -> Self {
        self.sample_offset = offset;
        self
    }

    /// Makes the passes grow with the samples done instead of staying `pass_spp` long.
    ///
    /// Below `min_spp`, where no check is due, a pass is as long as the samples already done (so
    /// the count doubles), cut at the minimum. From the minimum on a pass is an eighth of the
    /// largest power of two at or below the samples done, rounded down to a multiple of `pass_spp`
    /// and never below it. A render of 8192 samples at a minimum of 512 and a pass of 16 then makes
    /// 38 passes and 33 checks instead of 512 and 481, and every power of two is a pass end. Each
    /// pass costs one GPU round trip and one check of every live pixel, so short passes cost more
    /// than they save.
    #[must_use]
    pub const fn with_growing_passes(mut self) -> Self {
        self.growing = true;
        self
    }

    /// The length of the pass that starts now, before the cap cuts it.
    const fn pass_size(&self) -> u32 {
        let pass = self.settings.pass_spp;
        if !self.growing {
            return pass;
        }
        let step = if self.done < self.settings.min_spp {
            let doubled = if self.done > pass { self.done } else { pass };
            let to_minimum = self.settings.min_spp - self.done;
            if doubled < to_minimum {
                doubled
            } else {
                to_minimum
            }
        } else {
            // `done >= min_spp >= 1` here, so the logarithm exists.
            ((1_u32 << self.done.ilog2()) / CHECKS_PER_OCTAVE) / pass * pass
        };
        if step < pass { pass } else { step }
    }

    /// The longest pass the run can ask for: what a backend checks its per-call sample limit with.
    #[must_use]
    pub fn largest_pass(&self) -> u32 {
        if !self.growing {
            return self.settings.pass_spp;
        }
        let top = (1_u32 << self.settings.max_spp.max(1).ilog2()) / CHECKS_PER_OCTAVE;
        top.max(self.settings.min_spp.div_ceil(2))
            .max(self.settings.pass_spp)
    }

    /// The settings the run was started with.
    #[must_use]
    pub const fn settings(&self) -> &AdaptiveSettings {
        &self.settings
    }

    /// The per-tile convergence state.
    #[must_use]
    pub const fn map(&self) -> &ConvergenceMap {
        &self.map
    }

    /// Samples per pixel that every pass so far added (the count of the live tiles).
    #[must_use]
    pub const fn samples_done(&self) -> u32 {
        self.done
    }

    /// The next pass, or `None` when no tile is live or `max_spp` is reached.
    ///
    /// Asking again before [`Self::finish_pass`] returns the same pass.
    pub fn next_pass(&mut self) -> Option<Pass> {
        let left = self.settings.max_spp.saturating_sub(self.done);
        if left == 0 || self.map.is_done() {
            self.pending = None;
            return None;
        }
        let spp = self.pass_size().min(left);
        self.pending = Some(spp);
        Some(Pass {
            first_sample: self.sample_offset.saturating_add(self.done),
            spp,
            live: self.map.live_tiles(),
        })
    }

    /// Reports that the pass last returned by [`Self::next_pass`] was traced into the buffers.
    ///
    /// From `min_spp` on, every pass is followed by the convergence check on `sum` and `half_b`
    /// (full-frame buffers); `scale` multiplies the means before the display tone-map. Does
    /// nothing when no pass is pending.
    pub fn finish_pass(&mut self, sum: &[Vec3], half_b: &[Vec3], scale: f32) {
        self.finish_pass_with_offset(sum, half_b, scale, None);
    }

    /// [`Self::finish_pass`] with a beam part added to both half means (beam guide section 12.4).
    ///
    /// `offset`, when given, is laid out like `sum` and `half_b` (one entry per pixel of the
    /// full-frame buffers); the check compares `offset + mean_a` against `offset + mean_b`, so the
    /// noise it judges is the total's. With `None` this is [`Self::finish_pass`], bit for bit.
    pub fn finish_pass_with_offset(
        &mut self,
        sum: &[Vec3],
        half_b: &[Vec3],
        scale: f32,
        offset: Option<&[Vec3]>,
    ) {
        let Some(check) = self.close_pending_pass() else {
            return;
        };
        let noises = live_tile_noises_with_offset(
            sum,
            half_b,
            offset,
            &check.live,
            (check.samples, check.odd_samples),
            scale,
            self.settings.metric,
        );
        self.settle(&check.live, &noises);
    }

    /// Restricts the run to `tiles`: every other tile is stopped from the start, with no samples.
    ///
    /// This is the batch run of the beam residual (beam guide section 12.4): the run only ever
    /// sees the batch's tiles live, so [`Self::next_pass`] hands out those tiles only and the run
    /// is done when they have converged. The dilation rule reads the batch's own errors alone: a
    /// tile outside `tiles` counts as calm, so a batch border never holds a tile back (the
    /// documented beam-mode difference, deterministic because batch membership is fixed). With
    /// every tile of the frame in `tiles` the run is the unrestricted run. `tiles` of another
    /// frame size or tile edge leaves no tile live. Call it before the first pass.
    ///
    /// The state reached has the members live and unmeasured, and every other tile stopped with
    /// no samples and error 0. It is built directly as a sparse map ([`ConvergenceMap::sparse`]),
    /// so the cost follows the batch, not the frame; [`Self::new_restricted`] skips the full map
    /// that `new` would build first.
    #[must_use]
    pub fn restricted_to(mut self, tiles: &LiveTiles) -> Self {
        self.map = self.restricted_map(tiles);
        self
    }

    /// Starts a run over a `width` x `height` frame restricted to `tiles` from the start.
    ///
    /// The same run as `new(..)?.restricted_to(tiles)`, bit for bit, without the full-frame tile
    /// map `new` allocates: the work and memory follow `tiles`, not the frame. See
    /// [`Self::restricted_to`] for the restriction rules.
    ///
    /// # Errors
    ///
    /// Returns the rule `settings` breaks, see [`AdaptiveSettings::validate`].
    pub fn new_restricted(
        settings: AdaptiveSettings,
        width: u32,
        height: u32,
        tiles: &LiveTiles,
    ) -> Result<Self, AdaptiveSettingsError> {
        settings.validate()?;
        let mut run = Self {
            settings,
            width,
            height,
            map: ConvergenceMap::sparse(width, height, settings.tile, &[]),
            sample_offset: 0,
            done: 0,
            pending: None,
            growing: false,
        };
        run.map = run.restricted_map(tiles);
        Ok(run)
    }

    /// The sparse map of the restriction to `tiles`; a different frame size or tile edge gives no
    /// live tile.
    fn restricted_map(&self, tiles: &LiveTiles) -> ConvergenceMap {
        let fits = tiles.width() == self.width
            && tiles.height() == self.height
            && tiles.tile() == self.settings.tile;
        let members: &[u32] = if fits { tiles.indices() } else { &[] };
        ConvergenceMap::sparse(self.width, self.height, self.settings.tile, members)
    }

    /// What the check after the pending pass compares, or `None` when no pass is pending or
    /// that pass ends below `min_spp` (no check is due).
    ///
    /// A caller that measures the tiles itself (the GPU-resident run) asks this before the pass
    /// is closed, measures `check.live` at `check.samples` / `check.odd_samples`, and reports
    /// the noises with [`Self::finish_pass_with_tile_errors`].
    #[must_use]
    pub fn pending_check(&self) -> Option<PendingCheck> {
        let spp = self.pending?;
        let samples = self.done + spp;
        if samples < self.settings.min_spp {
            return None;
        }
        Some(PendingCheck {
            live: self.map.live_tiles(),
            samples,
            odd_samples: odd_samples(self.sample_offset, samples),
        })
    }

    /// [`Self::finish_pass`] for a caller that already measured the live tiles.
    ///
    /// `noises` holds the noise of every tile of [`PendingCheck::live`], in its order, in the
    /// unit of the run's metric (see [`metric::tile_noise`](super::metric::tile_noise)); when no
    /// check is due the slice is ignored and may be empty. A missing entry counts as unmeasured
    /// (infinite), so it never converges. The decision code is the one [`Self::finish_pass`]
    /// runs, so the same noises give the same stopped tiles.
    pub fn finish_pass_with_tile_errors(&mut self, noises: &[f32]) {
        let Some(check) = self.close_pending_pass() else {
            return;
        };
        self.settle(&check.live, noises);
    }

    /// Takes the pending pass into the books: its samples are added to the live tiles.
    ///
    /// Returns the check that is due after it, `None` when nothing was pending or the run is
    /// still below `min_spp`.
    fn close_pending_pass(&mut self) -> Option<PendingCheck> {
        let spp = self.pending.take()?;
        self.map.add_samples(spp);
        self.done += spp;
        if self.done < self.settings.min_spp {
            return None;
        }
        let samples = self.done;
        Some(PendingCheck {
            live: self.map.live_tiles(),
            samples,
            odd_samples: odd_samples(self.sample_offset, samples),
        })
    }

    /// The one decision: records the tile `noises` (in the order of `live`) and stops the tiles
    /// that converged. Shared by the host and the GPU-resident paths.
    fn settle(&mut self, live: &LiveTiles, noises: &[f32]) {
        let errors: Vec<(u32, f32)> = live
            .indices()
            .iter()
            .enumerate()
            .map(|(slot, &index)| (index, noises.get(slot).copied().unwrap_or(f32::INFINITY)))
            .collect();
        self.map.record_errors(&errors);
        self.map
            .converge(self.settings.min_spp, self.settings.target_noise);
    }

    /// Samples per pixel the pixel `(x, y)` received.
    #[must_use]
    pub fn pixel_spp(&self, x: u32, y: u32) -> u32 {
        self.map.pixel_spp(x, y)
    }

    /// Samples per pixel for every pixel, row-major.
    #[must_use]
    pub fn pixel_counts(&self) -> Vec<u32> {
        let mut counts = Vec::with_capacity(self.width as usize * self.height as usize);
        for y in 0..self.height {
            for x in 0..self.width {
                counts.push(self.map.pixel_spp(x, y));
            }
        }
        counts
    }

    /// Samples traced over the whole frame, for the "samples saved" figure.
    #[must_use]
    pub fn samples_traced(&self) -> u64 {
        self.map.samples_traced()
    }

    /// Divides each pixel of `sum` by its own sample count; pixels without samples stay zero.
    #[must_use]
    pub fn resolve(&self, sum: &[Vec3]) -> Vec<Vec3> {
        let mut out = Vec::with_capacity(sum.len());
        let width = self.width.max(1) as usize;
        for (index, &value) in sum.iter().enumerate() {
            let x = u32::try_from(index % width).unwrap_or(u32::MAX);
            let y = u32::try_from(index / width).unwrap_or(u32::MAX);
            let count = self.map.pixel_spp(x, y);
            out.push(if count == 0 {
                Vec3::ZERO
            } else {
                value / count_f32(count)
            });
        }
        out
    }

    /// Runs the whole render with `trace` and returns the image.
    ///
    /// `trace(pass, sum, half_b)` must add the pass to the full-frame buffers as the pass
    /// contract says. `scale` is as in [`Self::finish_pass`].
    #[must_use]
    pub fn run_with<F>(mut self, scale: f32, mut trace: F) -> AdaptiveImage
    where
        F: FnMut(&Pass, &mut [Vec3], &mut [Vec3]),
    {
        let pixels = self.width as usize * self.height as usize;
        let mut sum = vec![Vec3::ZERO; pixels];
        let mut half_b = vec![Vec3::ZERO; pixels];
        while let Some(pass) = self.next_pass() {
            trace(&pass, &mut sum, &mut half_b);
            self.finish_pass(&sum, &half_b, scale);
        }
        AdaptiveImage {
            width: self.width,
            height: self.height,
            counts: self.pixel_counts(),
            pixels: self.resolve(&sum),
            samples_traced: self.samples_traced(),
            sum,
            half_b,
        }
    }
}
