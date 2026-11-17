//! The shader-cache card: what the window says while the coordinator's graphics card builds its
//! shader cache.
//!
//! The coordinator computes on the graphics card, whose kernels must compile before the first
//! render. It builds them in the background at start and reports that in
//! `CoordinatorStatus::shader_cache`, for a local and a remote coordinator alike. The Overview
//! page shows a card (`ShaderCardState`) only while a build compiles shaders fresh: a start that
//! only loads the finished cache shows nothing. The card stays [`LINGER`] after the build ends,
//! saying how it ended, and can be dismissed. While the build runs, the shell's status pill also
//! reads "Building shader cache…" ([`building`]), so the build is visible from every page.

use crate::{
    control::backend::CoordinatorStatus,
    gui::app::{AppWindow, ShaderCardState},
};
use pleochroma_net::admin::status::{ShaderCacheState, ShaderCacheStatus};
use slint::ComponentHandle;
use std::time::{Duration, Instant};

/// How long the card stays up, saying how the build ended.
pub const LINGER: Duration = Duration::from_secs(4);

/// The status pill's text while the build runs.
pub const PILL_TEXT: &str = "Building shader cache\u{2026}";

/// The card's title while the build runs.
const TITLE_RUNNING: &str = "Building shader cache";

/// What the card's texts say, ready to put into `ShaderCardState`.
#[derive(Clone, Debug, PartialEq)]
pub struct CardView {
    /// "Building shader cache", "Shader cache ready", and so on.
    pub title: String,
    /// The one-time note under the title.
    pub note: String,
    /// "12 of 40", "Starting", "Ready", "Stopped at 12 of 40" or "Not available".
    pub progress: String,
    /// The filled share of the bar, 0.0 to 1.0.
    pub fraction: f32,
    /// The build is still running.
    pub running: bool,
    /// The build did not finish: the card uses the notice tone.
    pub problem: bool,
}

/// What the card remembers between two looks at the status.
#[derive(Debug, Default)]
pub struct CardTracker {
    /// The state seen at the last look.
    last: ShaderCacheState,
    /// A fresh build put the card up, and it has not gone down again.
    shown: bool,
    /// The user dismissed the card for this build.
    dismissed: bool,
    /// When the build ended while the card was up.
    ended: Option<Instant>,
}

/// Whether the status pill says "Building shader cache…": the coordinator runs and a fresh build
/// is under way.
#[must_use]
pub fn building(status: &CoordinatorStatus) -> bool {
    status.state.is_running()
        && status.shader_cache.state == ShaderCacheState::Running
        && status.shader_cache.fresh
}

impl CardTracker {
    /// Whether the user dismissed the card for the current build.
    #[must_use]
    pub const fn dismissed(&self) -> bool {
        self.dismissed
    }

    /// Takes one look at `status`. `pressed` is the window's dismiss flag; `subject` names the
    /// computer that renders ("This computer", or the remote coordinator's name). Returns the
    /// card to show, or `None` while it is down.
    pub fn update(
        &mut self,
        status: ShaderCacheStatus,
        pressed: bool,
        subject: &str,
        now: Instant,
    ) -> Option<CardView> {
        let was_running = self.last == ShaderCacheState::Running;
        let is_running = status.state == ShaderCacheState::Running;
        if is_running && !was_running {
            // A new build: the card and its dismissal start over.
            self.shown = false;
            self.dismissed = false;
            self.ended = None;
        }
        if pressed {
            self.dismissed = true;
        }
        self.last = status.state;
        if is_running {
            // A start that only loads the finished cache never gets a card; one that turns
            // fresh partway puts it up from that step on.
            self.shown |= status.fresh;
        } else if was_running {
            // The build ended: the card lingers if it was up, or if a fresh build stopped early,
            // which the user should hear about.
            let stopped_fresh = status.state == ShaderCacheState::Stopped && status.fresh;
            if status.state != ShaderCacheState::Idle && (self.shown || stopped_fresh) {
                self.shown = true;
                self.ended = Some(now);
            } else {
                self.shown = false;
            }
        }
        if let Some(ended) = self.ended
            && now.saturating_duration_since(ended) >= LINGER
        {
            self.ended = None;
            self.shown = false;
        }
        if !self.shown || self.dismissed {
            return None;
        }
        if !is_running && self.ended.is_none() {
            return None;
        }
        Some(view(status, subject))
    }
}

/// The card's texts for `status`.
fn view(status: ShaderCacheStatus, subject: &str) -> CardView {
    let counted = status.total > 0;
    let fraction = if counted {
        (status.done as f32 / status.total as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let once = format!(
        "{subject} renders for your devices on its graphics card, which needs its shaders \
         compiled first. This runs in the background after an install or an update; later \
         starts load the finished cache."
    );
    match status.state {
        ShaderCacheState::Running | ShaderCacheState::Idle => CardView {
            title: TITLE_RUNNING.to_owned(),
            note: once,
            progress: if counted {
                format!("{} of {}", status.done.min(status.total), status.total)
            } else {
                "Starting".to_owned()
            },
            fraction,
            running: status.state == ShaderCacheState::Running,
            problem: false,
        },
        ShaderCacheState::Ready => CardView {
            title: "Shader cache ready".to_owned(),
            note: once,
            progress: "Ready".to_owned(),
            fraction: 1.0,
            running: false,
            problem: false,
        },
        ShaderCacheState::Stopped => CardView {
            title: "Shader cache partly built".to_owned(),
            note: format!(
                "{subject} renders for your devices on its graphics card, which needs its \
                 shaders compiled first. The build stopped early; the remaining shaders compile \
                 when a render first needs them."
            ),
            progress: if counted {
                format!(
                    "Stopped at {} of {}",
                    status.done.min(status.total),
                    status.total
                )
            } else {
                "Stopped".to_owned()
            },
            fraction,
            running: false,
            problem: true,
        },
        ShaderCacheState::Unavailable => CardView {
            title: "Shader cache not built".to_owned(),
            note: format!(
                "{subject} has no usable graphics card renderer, so no shaders were built. \
                 Renders run on the processor instead."
            ),
            progress: "Not available".to_owned(),
            fraction: 0.0,
            running: false,
            problem: true,
        },
    }
}

/// Looks at `status` and puts the card into the window's `ShaderCardState`.
pub fn apply(
    window: &AppWindow,
    tracker: &mut CardTracker,
    status: ShaderCacheStatus,
    subject: &str,
    now: Instant,
) {
    let model = window.global::<ShaderCardState>();
    let card = tracker.update(status, model.get_dismissed(), subject, now);
    model.set_dismissed(tracker.dismissed());
    match card {
        Some(card) => {
            model.set_running(card.running);
            model.set_problem(card.problem);
            model.set_title(card.title.into());
            model.set_note(card.note.into());
            model.set_progress_text(card.progress.into());
            model.set_fraction(card.fraction);
            model.set_visible(true);
        }
        None => model.set_visible(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: ShaderCacheState, done: u32, total: u32, fresh: bool) -> ShaderCacheStatus {
        ShaderCacheStatus {
            state,
            done,
            total,
            fresh,
        }
    }

    fn look(
        tracker: &mut CardTracker,
        status: ShaderCacheStatus,
        now: Instant,
    ) -> Option<CardView> {
        tracker.update(status, false, "This computer", now)
    }

    #[test]
    fn a_start_that_only_loads_the_cache_shows_nothing() {
        let (mut tracker, now) = (CardTracker::default(), Instant::now());
        for done in 0..5 {
            let running = status(ShaderCacheState::Running, done, 5, false);
            assert!(look(&mut tracker, running, now).is_none());
        }
        let ready = status(ShaderCacheState::Ready, 5, 5, false);
        assert!(look(&mut tracker, ready, now).is_none());
    }

    #[test]
    fn a_fresh_build_counts_steps_and_lingers_after_it_ends() {
        let (mut tracker, now) = (CardTracker::default(), Instant::now());
        let card = look(
            &mut tracker,
            status(ShaderCacheState::Running, 12, 40, true),
            now,
        )
        .expect("the card is up");
        assert_eq!(card.title, "Building shader cache");
        assert_eq!(card.progress, "12 of 40");
        assert!(card.running && !card.problem);
        assert!((card.fraction - 0.3).abs() < 1e-6);

        let ready = status(ShaderCacheState::Ready, 40, 40, true);
        let card = look(&mut tracker, ready, now).expect("it lingers");
        assert_eq!(card.title, "Shader cache ready");
        assert!(!card.running);
        let later = now + LINGER / 2;
        assert!(look(&mut tracker, ready, later).is_some());
        assert!(look(&mut tracker, ready, now + LINGER).is_none());
        assert!(look(&mut tracker, ready, now + LINGER * 3).is_none());
    }

    #[test]
    fn a_cached_start_that_turns_fresh_shows_the_card_from_that_step_on() {
        let (mut tracker, now) = (CardTracker::default(), Instant::now());
        assert!(
            look(
                &mut tracker,
                status(ShaderCacheState::Running, 2, 9, false),
                now
            )
            .is_none()
        );
        let card = look(
            &mut tracker,
            status(ShaderCacheState::Running, 3, 9, true),
            now,
        );
        assert_eq!(card.expect("up").progress, "3 of 9");
    }

    #[test]
    fn an_unknown_total_says_starting() {
        let (mut tracker, now) = (CardTracker::default(), Instant::now());
        let card = look(
            &mut tracker,
            status(ShaderCacheState::Running, 0, 0, true),
            now,
        );
        let card = card.expect("up");
        assert_eq!(card.progress, "Starting");
        assert!(card.fraction.abs() < 1e-6);
    }

    #[test]
    fn dismissing_hides_the_card_for_that_build_only() {
        let (mut tracker, now) = (CardTracker::default(), Instant::now());
        let running = status(ShaderCacheState::Running, 1, 9, true);
        assert!(look(&mut tracker, running, now).is_some());
        assert!(
            tracker
                .update(running, true, "This computer", now)
                .is_none()
        );
        assert!(tracker.dismissed());
        assert!(look(&mut tracker, running, now).is_none(), "stays hidden");
        let ready = status(ShaderCacheState::Ready, 9, 9, true);
        assert!(
            look(&mut tracker, ready, now).is_none(),
            "also while it lingers"
        );
        // A later build starts over.
        assert!(
            look(
                &mut tracker,
                status(ShaderCacheState::Idle, 0, 0, false),
                now
            )
            .is_none()
        );
        assert!(look(&mut tracker, running, now).is_some());
        assert!(!tracker.dismissed());
    }

    #[test]
    fn a_fresh_build_that_stops_early_says_so() {
        let (mut tracker, now) = (CardTracker::default(), Instant::now());
        assert!(
            look(
                &mut tracker,
                status(ShaderCacheState::Running, 4, 9, true),
                now
            )
            .is_some()
        );
        let card = look(
            &mut tracker,
            status(ShaderCacheState::Stopped, 4, 9, true),
            now,
        )
        .expect("it lingers");
        assert_eq!(card.title, "Shader cache partly built");
        assert_eq!(card.progress, "Stopped at 4 of 9");
        assert!(card.problem);
        assert!(card.note.contains("stopped early"));
    }

    #[test]
    fn a_stop_before_any_fresh_step_is_silent_and_one_after_is_not() {
        let (mut tracker, now) = (CardTracker::default(), Instant::now());
        assert!(
            look(
                &mut tracker,
                status(ShaderCacheState::Running, 4, 9, false),
                now
            )
            .is_none()
        );
        assert!(
            look(
                &mut tracker,
                status(ShaderCacheState::Stopped, 4, 9, false),
                now
            )
            .is_none()
        );
        // A build that ends between two looks, having compiled fresh, still reports a stop.
        let mut tracker = CardTracker::default();
        assert!(
            look(
                &mut tracker,
                status(ShaderCacheState::Running, 0, 9, false),
                now
            )
            .is_none()
        );
        let card = look(
            &mut tracker,
            status(ShaderCacheState::Stopped, 4, 9, true),
            now,
        );
        assert_eq!(card.expect("shown").title, "Shader cache partly built");
    }

    #[test]
    fn no_graphics_card_renderer_after_a_shown_build_says_not_built() {
        let (mut tracker, now) = (CardTracker::default(), Instant::now());
        assert!(
            look(
                &mut tracker,
                status(ShaderCacheState::Running, 0, 0, true),
                now
            )
            .is_some()
        );
        let card = look(
            &mut tracker,
            status(ShaderCacheState::Unavailable, 0, 0, true),
            now,
        )
        .expect("it lingers");
        assert_eq!(card.title, "Shader cache not built");
        assert_eq!(card.progress, "Not available");
    }

    #[test]
    fn the_first_look_at_an_ended_build_shows_nothing() {
        let (mut tracker, now) = (CardTracker::default(), Instant::now());
        let stopped = status(ShaderCacheState::Stopped, 4, 9, true);
        assert!(look(&mut tracker, stopped, now).is_none());
    }

    #[test]
    fn the_note_names_the_computer_that_renders() {
        let card = view(status(ShaderCacheState::Running, 1, 2, true), "Studio PC");
        assert!(card.note.starts_with("Studio PC renders for your devices"));
    }

    #[test]
    fn the_pill_mirrors_only_a_fresh_running_build() {
        use crate::control::backend::CoordinatorState;
        let mut coordinator =
            CoordinatorStatus::minimal("Studio PC", "0.12.0", 45, CoordinatorState::Running);
        coordinator.shader_cache = status(ShaderCacheState::Running, 1, 9, false);
        assert!(!building(&coordinator));
        coordinator.shader_cache.fresh = true;
        assert!(building(&coordinator));
        coordinator.shader_cache.state = ShaderCacheState::Ready;
        assert!(!building(&coordinator));
        coordinator.shader_cache.state = ShaderCacheState::Running;
        coordinator.state = CoordinatorState::Stopped;
        assert!(!building(&coordinator));
    }
}
