//! Bounded progressive passes and transactional accumulation for the live viewport.

use super::gpu_backend::FrameOutputs;
use crate::bridge::sample_cursor::LiveEpoch;
use glam::Vec3;
use std::time::Duration;

/// Return an unfinished local claim to its shared epoch, including early exits.
pub(super) struct PassClaim<'a> {
    epoch: Option<&'a LiveEpoch>,
    start: u32,
    count: u32,
    committed: bool,
}

impl<'a> PassClaim<'a> {
    pub(super) const fn new(epoch: Option<&'a LiveEpoch>, start: u32, count: u32) -> Self {
        Self {
            epoch,
            start,
            count,
            committed: false,
        }
    }

    pub(super) const fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for PassClaim<'_> {
    fn drop(&mut self) {
        if !self.committed
            && let Some(epoch) = self.epoch
        {
            epoch.return_to_local(self.start, self.count);
        }
    }
}

/// Measures completed passes, independently of the requested final sample count.
/// New views always start with one sample; stationary views grow conservatively.
#[derive(Default)]
pub(super) struct PassBudget {
    seconds_per_sample: Option<f64>,
    previous_samples: u32,
    radiance: Vec<Vec3>,
    depth: Vec<f32>,
    normal: Vec<Vec3>,
    facet: Vec<i32>,
}

impl PassBudget {
    pub(super) const fn reset(&mut self) {
        self.seconds_per_sample = None;
        self.previous_samples = 1;
    }

    pub(super) fn samples(&mut self, moving: bool, reset: bool, remaining: u32) -> u32 {
        if reset {
            self.reset();
        }
        if moving || self.seconds_per_sample.is_none() {
            return remaining.min(1);
        }
        let seconds = self.seconds_per_sample.unwrap_or(1.0);
        // Prefer a pass under 50 ms. If a single sample exceeds this, GPU chunks
        // and CPU cancellation checks still let a new camera interrupt the pass.
        let ceiling = self.previous_samples.max(1).saturating_mul(2).min(8);
        (1..=ceiling)
            .take_while(|&samples| seconds * f64::from(samples) <= 0.050)
            .last()
            .unwrap_or(1)
            .min(remaining)
    }

    pub(super) fn begin(&mut self, pixels: usize) -> FrameOutputs<'_> {
        self.radiance.resize(pixels, Vec3::ZERO);
        self.radiance.fill(Vec3::ZERO);
        self.depth.resize(pixels, 1.0e6);
        self.normal.resize(pixels, Vec3::ZERO);
        self.facet.resize(pixels, -1);
        FrameOutputs {
            accum: &mut self.radiance,
            depth: &mut self.depth,
            normal: &mut self.normal,
            facet_id: &mut self.facet,
        }
    }

    /// Commit only a whole pass. Cancelled passes never reach this method.
    pub(super) fn commit(
        &mut self,
        outputs: &mut FrameOutputs<'_>,
        samples: u32,
        elapsed: Duration,
    ) {
        assert_eq!(outputs.accum.len(), self.radiance.len());
        for (total, contribution) in outputs.accum.iter_mut().zip(&self.radiance) {
            *total += *contribution;
        }
        outputs.depth.copy_from_slice(&self.depth);
        outputs.normal.copy_from_slice(&self.normal);
        outputs.facet_id.copy_from_slice(&self.facet);
        let seconds = elapsed.as_secs_f64() / f64::from(samples.max(1));
        self.seconds_per_sample = Some(self.seconds_per_sample.map_or(seconds, |previous| {
            // React immediately to a slowdown; smooth improvements to avoid bursts.
            seconds.max(previous.mul_add(0.7, seconds * 0.3))
        }));
        self.previous_samples = samples;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_cost_does_not_grow_with_final_quality() {
        let mut budget = PassBudget::default();
        assert_eq!(budget.samples(true, true, 128), 1);
        assert_eq!(budget.samples(true, true, 1024), 1);
        assert_eq!(budget.samples(false, true, 1024), 1);
        assert_eq!(budget.samples(false, false, 0), 0);
    }

    #[test]
    fn incomplete_pass_is_discarded_without_losing_prior_samples() {
        let mut budget = PassBudget::default();
        let mut accum = [Vec3::ONE];
        budget.begin(1).accum[0] = Vec3::splat(100.0);
        // A new pass replaces the cancelled pass's scratch contribution.
        budget.begin(1).accum[0] = Vec3::splat(2.0);
        budget.commit(
            &mut FrameOutputs {
                accum: &mut accum,
                depth: &mut [0.0],
                normal: &mut [Vec3::ZERO],
                facet_id: &mut [-1],
            },
            1,
            Duration::from_millis(5),
        );
        assert_eq!(accum, [Vec3::splat(3.0)]);
        assert_eq!(budget.samples(false, false, 1024), 2);
        assert_eq!(budget.samples(true, false, 1024), 1);
        // A paused scene change resets pacing before another pass is requested.
        budget.reset();
        assert_eq!(budget.samples(false, false, 1024), 1);
    }

    #[test]
    fn slow_pass_reduces_the_next_batch() {
        let mut budget = PassBudget::default();
        budget.begin(0);
        budget.commit(
            &mut FrameOutputs {
                accum: &mut [],
                depth: &mut [],
                normal: &mut [],
                facet_id: &mut [],
            },
            8,
            Duration::from_secs(2),
        );
        assert_eq!(budget.samples(false, false, 1024), 1);
        assert_eq!(budget.samples(false, true, 1024), 1);
    }

    #[test]
    fn cancelled_local_work_returns_its_sample_range_to_the_shared_epoch() {
        let epoch = LiveEpoch::new(1, 1, 8);
        let (start, count) = epoch.claim_local(3).unwrap();
        drop(PassClaim::new(Some(&epoch), start, count));
        assert_eq!(epoch.claim_local(3), Some((start, count)));
        let (next, count) = epoch.claim_local(3).unwrap();
        let mut completed = PassClaim::new(Some(&epoch), next, count);
        completed.commit();
        drop(completed);
        assert_eq!(epoch.claim_local(8), Some((6, 2)));
    }
}
