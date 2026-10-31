//! Sample progress and a conservative time estimate for the live image.

use crate::{MainWindow, ViewportModel};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

thread_local! {
    static PROGRESS: RefCell<ProgressRate> = RefCell::new(ProgressRate::default());
}

#[derive(Default)]
struct ProgressRate {
    generation: Option<u64>,
    last: Option<(Instant, u32)>,
    seconds_per_sample: Option<f64>,
    observed: Duration,
    observations: u32,
    suspended: bool,
}

impl ProgressRate {
    const fn suspend(&mut self, suspended: bool) {
        if self.suspended != suspended {
            self.last = None;
        }
        self.suspended = suspended;
    }

    fn observe(&mut self, generation: u64, completed: u32, now: Instant) {
        if self.suspended {
            return;
        }
        if self.generation != Some(generation)
            || self.last.is_some_and(|(_, count)| completed < count)
        {
            *self = Self {
                generation: Some(generation),
                ..Self::default()
            };
        }
        // Preparation and the first completed sample do not predict steady rendering.
        if completed == 0 {
            self.last = None;
            self.seconds_per_sample = None;
            self.observed = Duration::ZERO;
            self.observations = 0;
            return;
        }
        if let Some((previous, count)) = self.last {
            if completed == count {
                return;
            }
            let elapsed = now.saturating_duration_since(previous);
            if !elapsed.is_zero() {
                let rate = elapsed.as_secs_f64() / f64::from(completed - count);
                self.seconds_per_sample = Some(
                    self.seconds_per_sample
                        .map_or(rate, |old| old.mul_add(0.7, rate * 0.3)),
                );
                self.observed += elapsed;
                self.observations += 1;
            }
        }
        self.last = Some((now, completed));
    }

    fn remaining(&self, completed: u32, target: u32) -> Option<Duration> {
        if completed >= target || self.observations < 2 || self.observed < Duration::from_secs(1) {
            return None;
        }
        Duration::try_from_secs_f64(self.seconds_per_sample? * f64::from(target - completed)).ok()
    }
}

fn eta_text(remaining: Option<Duration>) -> String {
    let Some(remaining) = remaining else {
        return String::new();
    };
    let seconds = remaining
        .as_secs()
        .saturating_add(u64::from(remaining.subsec_nanos() > 0));
    if seconds < 60 {
        format!("About {} s left", seconds.max(1))
    } else if seconds < 3600 {
        format!("About {} min left", seconds.div_ceil(60))
    } else {
        let minutes = seconds.div_ceil(60);
        format!("About {} h {} min left", minutes / 60, minutes % 60)
    }
}

/// Called only on the UI thread, independently of texture publication.
pub fn update_progress(
    ui: &MainWindow,
    stage: &str,
    generation: u64,
    completed: u32,
    target: u32,
    _pass_elapsed: Duration,
) {
    if PROGRESS.with_borrow(|rate| rate.suspended) {
        return;
    }
    let preview = stage.contains("preview");
    let completed = if preview { 0 } else { completed };
    let eta = PROGRESS.with_borrow_mut(|rate| {
        rate.observe(generation, completed, Instant::now());
        eta_text(rate.remaining(completed, target))
    });
    let model = ui.global::<ViewportModel>();
    let finished = target > 0 && completed >= target;
    model.set_render_progress_visible(target > 0);
    model.set_render_progress_indeterminate(completed == 0 && !finished);
    model.set_render_samples_done(i32::try_from(completed).unwrap_or(i32::MAX));
    model.set_render_samples_target(i32::try_from(target).unwrap_or(i32::MAX));
    model.set_render_progress_stage(if finished { "Complete" } else { stage }.into());
    model.set_render_eta_text(eta.into());
}

/// Keep queued callbacks and estimates consistent with pause, visibility and scene
/// changes. Called on the UI thread even when no tracing/display callback arrives.
pub fn sync_availability(
    ui: &MainWindow,
    generation: u64,
    suspension: Option<&str>,
    visible: bool,
    has_geometry: bool,
) {
    let (obsolete, resumed) = PROGRESS.with_borrow_mut(|rate| {
        let obsolete = !has_geometry || rate.generation.is_some_and(|old| old != generation);
        if obsolete {
            *rate = ProgressRate {
                generation: Some(generation),
                ..ProgressRate::default()
            };
        }
        let resumed = rate.suspended && suspension.is_none();
        rate.suspend(suspension.is_some() || !has_geometry);
        (obsolete, resumed)
    });
    let model = ui.global::<ViewportModel>();
    if obsolete {
        model.set_render_samples_done(0);
        model.set_render_samples_target(0);
        model.set_render_progress_stage("".into());
        model.set_render_eta_text("".into());
    }
    model.set_render_progress_visible(
        visible && has_geometry && model.get_render_samples_target() > 0,
    );
    if let Some(stage) = suspension {
        model.set_render_progress_stage(stage.into());
        model.set_render_eta_text("".into());
    } else if resumed {
        let stage = if model.get_render_samples_target() > 0
            && model.get_render_samples_done() >= model.get_render_samples_target()
        {
            "Complete"
        } else if model.get_render_samples_done() == 0 {
            "Preparing trace"
        } else {
            "Rendering"
        };
        model.set_render_progress_stage(stage.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_and_resume_exclude_the_suspended_interval() {
        let start = Instant::now();
        let mut rate = ProgressRate::default();
        rate.observe(1, 1, start);
        rate.observe(1, 2, start + Duration::from_secs(1));
        rate.suspend(true);
        rate.observe(1, 2, start + Duration::from_secs(600));
        rate.suspend(false);
        rate.observe(1, 3, start + Duration::from_secs(601));
        rate.observe(1, 4, start + Duration::from_secs(602));
        assert_eq!(rate.remaining(4, 10), Some(Duration::from_secs(6)));
    }

    #[test]
    fn hours_round_without_sixty_minute_remainders() {
        assert_eq!(
            eta_text(Some(Duration::from_secs(7199))),
            "About 2 h 0 min left"
        );
    }

    #[test]
    fn compilation_and_preview_time_do_not_enter_the_estimate() {
        let start = Instant::now();
        let mut rate = ProgressRate::default();
        rate.observe(1, 0, start);
        rate.observe(1, 1, start + Duration::from_secs(30));
        assert!(rate.remaining(1, 128).is_none());
        rate.observe(1, 2, start + Duration::from_secs(31));
        rate.observe(1, 3, start + Duration::from_secs(32));
        assert_eq!(rate.remaining(3, 128), Some(Duration::from_secs(125)));
        assert_eq!(rate.remaining(3, 256), Some(Duration::from_secs(253)));
    }

    #[test]
    fn a_new_image_or_preview_clears_the_old_estimate() {
        let start = Instant::now();
        let mut rate = ProgressRate::default();
        for count in 1..=3 {
            rate.observe(1, count, start + Duration::from_secs(u64::from(count)));
        }
        rate.observe(2, 1, start + Duration::from_secs(4));
        assert!(rate.remaining(1, 128).is_none());
        rate.observe(2, 0, start + Duration::from_secs(5));
        assert!(rate.remaining(0, 128).is_none());
    }
}
