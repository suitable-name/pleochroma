//! What Render Details says about this session's preparation time.
//!
//! The poll feeds every status in; the note times the stretch in which the status was
//! `Preparing` and remembers the step it was in last. "Use the CPU" marks the preparation as
//! stopped, so Render Details can say where it stopped.

use pleochroma::diag::PrepareStep;
use pleochroma_mobile_core::{render::RenderStatus, viewport::render_view::PreparationTime};

/// The preparation of the session so far.
#[derive(Debug, Default)]
pub(super) struct PreparationNote {
    /// `process_ms()` when the preparing status was first seen.
    began_ms: Option<u64>,
    /// The step, `done` and `total` seen last.
    last: Option<(PrepareStep, u32, u32)>,
    /// Whole seconds from the first sight to the last.
    seconds: Option<u32>,
    /// A preparing status is on show now.
    live: bool,
    /// "Use the CPU" abandoned the preparation; the note stays as it was.
    stopped: bool,
}

impl PreparationNote {
    /// Takes the status seen at `now_ms` (milliseconds since the process started).
    pub(super) fn observe(&mut self, status: &RenderStatus, now_ms: u64) {
        if self.stopped {
            return;
        }
        if let RenderStatus::Preparing {
            step, done, total, ..
        } = status
        {
            if !self.live {
                *self = Self {
                    began_ms: Some(now_ms),
                    live: true,
                    ..Self::default()
                };
            }
            self.last = Some((*step, *done, *total));
            self.touch(now_ms);
        } else if self.live {
            self.live = false;
            self.touch(now_ms);
        }
    }

    /// The preparation was abandoned at `now_ms`; it stays stopped for the session.
    pub(super) fn abandon(&mut self, now_ms: u64) {
        if self.live {
            self.touch(now_ms);
            self.live = false;
            self.stopped = true;
        }
    }

    /// The time for Render Details, or `None` before any preparation was seen.
    pub(super) fn time(&self) -> Option<PreparationTime> {
        Some(PreparationTime {
            seconds: self.seconds?,
            stopped_in: if self.stopped { self.last } else { None },
        })
    }

    /// Sets the seconds from the first sight to `now_ms`.
    fn touch(&mut self, now_ms: u64) {
        if let Some(began) = self.began_ms {
            let seconds = now_ms.saturating_sub(began) / 1000;
            self.seconds = Some(u32::try_from(seconds).unwrap_or(u32::MAX));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preparing(done: u32) -> RenderStatus {
        RenderStatus::Preparing {
            step: PrepareStep::BuildingShaders,
            done,
            total: 18,
            elapsed_s: 0,
        }
    }

    #[test]
    fn nothing_seen_gives_no_time() {
        let mut note = PreparationNote::default();
        note.observe(&RenderStatus::Idle, 5_000);
        assert_eq!(note.time(), None);
    }

    #[test]
    fn a_finished_preparation_reports_its_length() {
        let mut note = PreparationNote::default();
        note.observe(&preparing(1), 1_000);
        note.observe(&preparing(9), 40_000);
        note.observe(
            &RenderStatus::Tracing {
                samples: 0,
                target: 32,
            },
            73_000,
        );
        assert_eq!(
            note.time(),
            Some(PreparationTime {
                seconds: 72,
                stopped_in: None
            })
        );
    }

    #[test]
    fn an_abandoned_preparation_names_the_step_it_stopped_in() {
        let mut note = PreparationNote::default();
        note.observe(&preparing(3), 0);
        note.abandon(72_500);
        note.observe(&preparing(4), 90_000);
        assert_eq!(
            note.time(),
            Some(PreparationTime {
                seconds: 72,
                stopped_in: Some((PrepareStep::BuildingShaders, 3, 18))
            })
        );
    }

    #[test]
    fn a_later_preparation_replaces_a_finished_one() {
        let mut note = PreparationNote::default();
        note.observe(&preparing(1), 0);
        note.observe(&RenderStatus::Idle, 10_000);
        note.observe(&preparing(2), 20_000);
        note.observe(&RenderStatus::Idle, 25_000);
        assert_eq!(note.time().map(|time| time.seconds), Some(5));
    }
}
