//! A rolling, regression-based time-remaining estimator for a long-running progress
//! bar (a still-image export job or a tilt-video frame sweep), plus
//! [`format_eta`] to turn its result into the short string those bars show next to
//! their percentage.
//!
//! # Why regression over a window, not the rate between two observations
//!
//! A job's reported progress is step-like: a fan-out export advances in
//! coordinator-sized chunks, and a tilt video reports once per frame. The rate between
//! the last two observations would swing wildly (near zero just after a chunk lands,
//! high just before the next one). A least-squares line over a rolling window of
//! `(time, fraction_done)` observations averages the steps into a steady rate, which is
//! what a "how much longer" readout needs.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// Observations older than this are dropped once the window also has more than
/// [`MAX_OBSERVATIONS`] entries -- see [`EtaEstimator::observe`]'s own doc comment.
const MIN_WINDOW: Duration = Duration::from_secs(30);

/// Cap on the observations kept once [`MIN_WINDOW`] is covered; bounds the cost of the fit.
const MAX_OBSERVATIONS: usize = 64;

/// The shortest observed span [`EtaEstimator::eta`] estimates from. Shorter spans
/// extrapolate noise (two ticks of the same batch) into a wildly wrong slope.
const MIN_SPAN_FOR_ESTIMATE: Duration = Duration::from_secs(5);

/// A rolling window of `(time, fraction_done)` observations for one running job,
/// used to fit a least-squares line and read off "how much longer until
/// `fraction_done` reaches 1.0". See this module's own doc comment for why a
/// regression over a window, rather than a single instantaneous rate.
///
/// Feed it with [`observe`](Self::observe) on every progress tick, read
/// [`eta`](Self::eta) whenever the UI needs a fresh estimate, and
/// [`reset`](Self::reset) it when a new, unrelated run starts (a fresh export job or
/// video sweep) -- carrying a previous run's observations into a new one would fit a
/// line across two unrelated rates.
#[derive(Debug, Default, Clone)]
pub struct EtaEstimator {
    observations: VecDeque<(Instant, f64)>,
    /// Whether the latest [`Self::observe_stretch`] report belonged to the opening stretch.
    #[cfg(feature = "beam")]
    opening: bool,
}

impl EtaEstimator {
    /// Records one `(now, fraction_done)` observation, trimming the window afterward.
    ///
    /// Keeps at least the last [`MIN_WINDOW`] of history, and at most
    /// [`MAX_OBSERVATIONS`] once that span is covered: a chatty job stays bounded, and
    /// a sparse one keeps enough points to fit a line.
    pub fn observe(&mut self, now: Instant, fraction_done: f64) {
        self.observations
            .push_back((now, fraction_done.clamp(0.0, 1.0)));
        while self.observations.len() > MAX_OBSERVATIONS {
            let Some(&(oldest, _)) = self.observations.front() else {
                break;
            };
            if now.saturating_duration_since(oldest) <= MIN_WINDOW {
                // Still inside the required 30-second window -- keep it even
                // though the count is already over the cap.
                break;
            }
            self.observations.pop_front();
        }
    }

    /// [`Self::observe`] for a job whose reports come in two unrelated stretches: an opening one
    /// (`opening` true: a beam render, whose probe may give the render up) and the render proper.
    ///
    /// The first report after the opening stretch clears the history, so one line never fits
    /// both: a silent probe followed by the render would predict a far too long remaining time.
    /// A job without an opening stretch uses [`Self::observe`].
    #[cfg(feature = "beam")]
    pub fn observe_stretch(&mut self, now: Instant, fraction_done: f64, opening: bool) {
        if self.opening && !opening {
            self.observations.clear();
        }
        self.opening = opening;
        self.observe(now, fraction_done);
    }

    /// Fits a least-squares line across the current window and reads off the
    /// remaining time until it reaches `fraction_done == 1.0`, evaluated at `now`
    /// (not necessarily the time of the last observation -- the caller may ask for
    /// an estimate between ticks).
    ///
    /// Returns `None` when there isn't enough history to trust a fit yet (fewer
    /// than two observations, or a span under [`MIN_SPAN_FOR_ESTIMATE`]), when the
    /// window's own timestamps are all identical (a zero-variance fit has no
    /// slope to read), or when the fitted slope is zero or negative -- a stalled
    /// or regressing job has no forward rate to project from, so no number is
    /// better than a wrong one.
    #[must_use]
    pub fn eta(&self, now: Instant) -> Option<Duration> {
        let &(first_time, _) = self.observations.front()?;
        let &(last_time, _) = self.observations.back()?;
        let span = last_time.duration_since(first_time);
        if self.observations.len() < 2 || span < MIN_SPAN_FOR_ESTIMATE {
            return None;
        }

        let xs_ys: Vec<(f64, f64)> = self
            .observations
            .iter()
            .map(|&(t, f)| (t.duration_since(first_time).as_secs_f64(), f))
            .collect();
        let count = xs_ys.len() as f64;
        let mean_x = xs_ys.iter().map(|&(x, _)| x).sum::<f64>() / count;
        let mean_y = xs_ys.iter().map(|&(_, y)| y).sum::<f64>() / count;

        let mut covariance = 0.0;
        let mut variance = 0.0;
        for &(x, y) in &xs_ys {
            let dx = x - mean_x;
            covariance = dx.mul_add(y - mean_y, covariance);
            variance = dx.mul_add(dx, variance);
        }
        if variance <= f64::EPSILON {
            return None;
        }
        let slope = covariance / variance;
        if slope <= 0.0 {
            return None;
        }

        let intercept = slope.mul_add(-mean_x, mean_y);
        let now_x = now.duration_since(first_time).as_secs_f64();
        let fitted_now = slope.mul_add(now_x, intercept).max(0.0);
        let remaining_fraction = (1.0 - fitted_now).max(0.0);
        let remaining_secs = remaining_fraction / slope;
        if !remaining_secs.is_finite() {
            return None;
        }
        Some(Duration::from_secs_f64(remaining_secs))
    }

    /// Clears every recorded observation -- called when a new, unrelated run starts
    /// (a fresh export job or video sweep begins, or the previous one finished,
    /// failed, or was cancelled), so its history never leaks into the next run's own
    /// estimate. See this struct's own doc comment.
    pub fn reset(&mut self) {
        self.observations.clear();
        #[cfg(feature = "beam")]
        {
            self.opening = false;
        }
    }
}

/// Formats an [`EtaEstimator::eta`] result as the short string a progress bar shows.
///
/// It is shown next to the percentage: `""` for `None` (hides the estimate entirely), otherwise
/// `"about 5 s left"` / `"about 2 min left"` / `"about 1 h 20 min left"`, rounded to
/// the coarsest unit that still reads as informative -- never a sub-second reading,
/// and never a bare `"0 s"` for a duration that rounds down to nothing.
#[must_use]
pub fn format_eta(eta: Option<Duration>) -> String {
    let Some(remaining) = eta else {
        return String::new();
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is rounded and at least 1.0 above; an ETA never nears u64::MAX seconds"
    )]
    let total_secs = remaining.as_secs_f64().round().max(1.0) as u64;

    if total_secs < 60 {
        format!("about {total_secs} s left")
    } else if total_secs < 3600 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "total_secs is below 3600, so the rounded minute count is a small positive value"
        )]
        let minutes = (total_secs as f64 / 60.0).round() as u64;
        format!("about {minutes} min left")
    } else {
        let hours = total_secs / 3600;
        let minutes = (total_secs % 3600) / 60;
        if minutes == 0 {
            format!("about {hours} h left")
        } else {
            format!("about {hours} h {minutes} min left")
        }
    }
}

/// The remaining-time text a design-count batch dialog (preview or tilt-curve batch)
/// shows after its "Completed N / M" caption: `""` before the first design finishes
/// and once all are done (the summary line takes over), `"estimating..."` while the
/// estimator has no trustworthy fit yet, otherwise [`format_eta`]'s own wording.
#[must_use]
pub fn batch_eta_label(completed: u32, total: u32, eta: Option<Duration>) -> String {
    if total == 0 || completed >= total {
        return String::new();
    }
    match eta {
        Some(_) => format_eta(eta),
        None => "estimating...".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{EtaEstimator, batch_eta_label, format_eta};
    use std::time::{Duration, Instant};

    /// Perfectly linear progress (a constant rate the whole way) must produce an
    /// exact ETA: a least-squares fit across points that already lie exactly on one
    /// line reproduces that line's own slope with no error.
    #[test]
    fn linear_progress_gives_exact_eta() {
        let start = Instant::now();
        let mut eta = EtaEstimator::default();
        // 0.05 fraction/second -- reaches 1.0 at t = 20s.
        for t in 0..=9u64 {
            eta.observe(start + Duration::from_secs(t), 0.05 * t as f64);
        }
        let now = start + Duration::from_secs(9);
        let remaining = eta.eta(now).expect("enough history for an estimate");
        // At t=9 the line is at fraction 0.45; 0.55 left at 0.05/s is 11s.
        assert!(
            (remaining.as_secs_f64() - 11.0).abs() < 1e-6,
            "expected ~11s, got {remaining:?}"
        );
    }

    /// A coordinator-style step cadence (a jump every 20 s) must still land within 25% of
    /// the true remaining time once a few steps are observed: the estimator fits a line
    /// across the window instead of reading the latest step's own rate.
    #[test]
    fn step_like_progress_is_within_a_quarter_of_the_truth_after_three_steps() {
        let start = Instant::now();
        let mut eta = EtaEstimator::default();
        // 5 steps of 20s each reach fraction 1.0 at t=100s; a true rate of 0.01/s.
        eta.observe(start, 0.0);
        eta.observe(start + Duration::from_secs(20), 0.2);
        eta.observe(start + Duration::from_secs(40), 0.4);
        eta.observe(start + Duration::from_secs(60), 0.6);
        let now = start + Duration::from_secs(60);
        let remaining = eta.eta(now).expect("enough history after three steps");
        let truth_secs = 40.0; // 100s total - 60s elapsed
        let error = (remaining.as_secs_f64() - truth_secs).abs() / truth_secs;
        assert!(
            error <= 0.25,
            "expected within 25% of {truth_secs}s, got {remaining:?}"
        );
    }

    /// Progress that never advances (the same fraction reported over and over) must
    /// never produce an estimate -- a zero slope has no forward rate to project a
    /// completion time from.
    #[test]
    fn stalled_progress_returns_none() {
        let start = Instant::now();
        let mut eta = EtaEstimator::default();
        for t in 0..=6u64 {
            eta.observe(start + Duration::from_secs(t), 0.3);
        }
        assert!(eta.eta(start + Duration::from_secs(6)).is_none());
    }

    /// [`EtaEstimator::reset`] must drop every prior observation -- confirmed by
    /// re-feeding only a single, too-recent observation afterward and checking the
    /// estimate is still refused for lack of history, exactly as a brand-new
    /// estimator would behave.
    #[test]
    fn reset_clears_prior_history() {
        let start = Instant::now();
        let mut eta = EtaEstimator::default();
        for t in 0..=9u64 {
            eta.observe(start + Duration::from_secs(t), 0.05 * t as f64);
        }
        assert!(eta.eta(start + Duration::from_secs(9)).is_some());

        eta.reset();
        eta.observe(start + Duration::from_secs(9), 0.45);
        assert!(
            eta.eta(start + Duration::from_secs(9)).is_none(),
            "a single observation right after reset must not yet produce an estimate"
        );
    }

    /// The render after an opening stretch (a beam probe that gave up) gets its own estimate;
    /// without the stretch flag the same reports fit one line through both and overestimate.
    #[cfg(all(feature = "beam", beam_tests))]
    #[test]
    fn the_stretch_after_an_opening_starts_its_own_estimate() {
        let start = Instant::now();
        let mut eta = EtaEstimator::default();
        // Six seconds of an opening stretch that does not advance.
        for t in 0..=6u64 {
            eta.observe_stretch(start + Duration::from_secs(t), 0.0, true);
        }
        // The render proper: 0.05 per second from t = 6 s.
        for t in 7..=16u64 {
            eta.observe_stretch(start + Duration::from_secs(t), 0.05 * (t - 6) as f64, false);
        }
        let now = start + Duration::from_secs(16);
        let remaining = eta.eta(now).expect("nine seconds of the render");
        // At t = 16 s the render is at 0.5: ten seconds left at 0.05 per second.
        assert!(
            (remaining.as_secs_f64() - 10.0).abs() < 1e-6,
            "expected 10 s, got {remaining:?}"
        );
        let mut mixed = EtaEstimator::default();
        for t in 0..=6u64 {
            mixed.observe(start + Duration::from_secs(t), 0.0);
        }
        for t in 7..=16u64 {
            mixed.observe(start + Duration::from_secs(t), 0.05 * (t - 6) as f64);
        }
        // One line through both stretches: slope 0.0337 per second, 16.9 s left.
        assert!(mixed.eta(now).expect("history").as_secs_f64() > 11.0);
    }

    #[test]
    fn batch_eta_label_covers_estimating_known_and_finished() {
        assert_eq!(batch_eta_label(3, 10, None), "estimating...");
        assert_eq!(
            batch_eta_label(3, 10, Some(Duration::from_secs(120))),
            "about 2 min left"
        );
        assert_eq!(batch_eta_label(10, 10, None), "");
        assert_eq!(batch_eta_label(0, 0, None), "");
    }

    #[test]
    fn format_eta_renders_each_bucket() {
        assert_eq!(format_eta(None), "");
        assert_eq!(format_eta(Some(Duration::from_secs(5))), "about 5 s left");
        assert_eq!(
            format_eta(Some(Duration::from_secs(120))),
            "about 2 min left"
        );
        assert_eq!(
            format_eta(Some(Duration::from_hours(1) + Duration::from_mins(20))),
            "about 1 h 20 min left"
        );
        assert_eq!(
            format_eta(Some(Duration::from_millis(400))),
            "about 1 s left"
        );
    }
}
