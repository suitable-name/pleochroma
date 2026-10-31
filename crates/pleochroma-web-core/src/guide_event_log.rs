//! The UI events a lesson step may wait for, remembered for the rest of the step.
//!
//! The same rules as the desktop's `gui/editor/guide/runtime.rs`: an event counts from the moment
//! it is reported until the next step is entered, and a Deep Solve that finished during one step
//! is kept for the step that waits for its result. Plain data, no window, so the rules are tested
//! natively.

use pleochroma_editor::guide::{
    Guide,
    solving_events::{DEEP_SOLVE_FINISHED, DEEP_SOLVE_STARTED},
};

/// The events reported since the current step began, and which step that is.
#[derive(Debug, Default)]
pub struct EventLog {
    events: Vec<String>,
    entered: Option<(String, usize)>,
}

impl EventLog {
    /// Notes that event `name` happened during the current step. A new Deep Solve start
    /// invalidates an older finish.
    pub fn record(&mut self, name: &str) {
        if name == DEEP_SOLVE_STARTED {
            self.events.retain(|event| event != DEEP_SOLVE_FINISHED);
        }
        if !self.events.iter().any(|event| event == name) {
            self.events.push(name.to_owned());
        }
    }

    /// Enters step `index` of `guide`, forgetting the events of the step before, except a
    /// completed Deep Solve that crosses its matching next step (a run that finishes before the
    /// learner presses Next must still complete the step that waits for its result).
    /// `revisiting` is true after Back, which never carries anything forward.
    pub fn enter(&mut self, guide: &Guide, index: usize, revisiting: bool) {
        let current_goal = guide.steps.get(index).map(|step| &step.goal);
        let previous = self.entered.as_ref();
        let carry = !revisiting
            && previous.is_some_and(|(previous_id, previous_index)| {
                *previous_id == guide.id && previous_index.checked_add(1) == Some(index)
            })
            && previous
                .and_then(|(_, previous_index)| guide.steps.get(*previous_index))
                .is_some_and(|step| step.goal.events().contains(&DEEP_SOLVE_STARTED))
            && current_goal.is_some_and(|goal| goal.events().contains(&DEEP_SOLVE_FINISHED))
            && self.events.iter().any(|event| event == DEEP_SOLVE_STARTED)
            && self.events.iter().any(|event| event == DEEP_SOLVE_FINISHED);

        if carry {
            self.events.retain(|event| event == DEEP_SOLVE_FINISHED);
        } else {
            self.events.clear();
        }
        self.entered = Some((guide.id.clone(), index));
    }

    /// Forgets the events and the step tracking: a lesson starts or closes.
    pub fn clear(&mut self) {
        self.events.clear();
        self.entered = None;
    }

    /// The events reported since the current step began, oldest first.
    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        &self.events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_editor::guide::{Goal, GuideCategory, GuideStep};

    fn deep_solve_lesson() -> Guide {
        Guide::new(
            "events-deep-solve",
            "Deep Solve",
            "Exercise the event handoff.",
            GuideCategory::Solving,
        )
        .step(
            GuideStep::new("Run", "Start a Deep Solve.")
                .goal(Goal::Event(DEEP_SOLVE_STARTED.into()), "a run started"),
        )
        .step(
            GuideStep::new("Wait", "Wait for the result.")
                .goal(Goal::Event(DEEP_SOLVE_FINISHED.into()), "the run finished"),
        )
    }

    #[test]
    fn an_event_counts_once_until_the_next_step() {
        let guide = deep_solve_lesson();
        let mut log = EventLog::default();
        log.enter(&guide, 0, false);
        log.record("palette_opened");
        log.record("palette_opened");
        assert_eq!(log.as_slice(), ["palette_opened"]);
        log.enter(&guide, 1, false);
        assert!(log.as_slice().is_empty(), "the next step starts clean");
    }

    #[test]
    fn a_fast_deep_solve_completion_survives_its_matching_forward_step() {
        let guide = deep_solve_lesson();
        let mut log = EventLog::default();
        log.enter(&guide, 0, false);
        log.record(DEEP_SOLVE_STARTED);
        log.record(DEEP_SOLVE_FINISHED);
        log.enter(&guide, 1, false);
        assert_eq!(log.as_slice(), [DEEP_SOLVE_FINISHED]);
    }

    #[test]
    fn a_deep_solve_finishing_after_the_step_was_entered_is_recorded_normally() {
        let guide = deep_solve_lesson();
        let mut log = EventLog::default();
        log.enter(&guide, 0, false);
        log.record(DEEP_SOLVE_STARTED);
        log.enter(&guide, 1, false);
        log.record(DEEP_SOLVE_FINISHED);
        assert_eq!(log.as_slice(), [DEEP_SOLVE_FINISHED]);
    }

    #[test]
    fn going_back_clears_a_carried_completion() {
        let guide = deep_solve_lesson();
        let mut log = EventLog::default();
        log.enter(&guide, 0, false);
        log.record(DEEP_SOLVE_STARTED);
        log.record(DEEP_SOLVE_FINISHED);
        log.enter(&guide, 1, false);
        log.enter(&guide, 0, true);
        log.enter(&guide, 1, false);
        assert!(log.as_slice().is_empty());
    }

    #[test]
    fn a_finish_without_a_start_is_not_carried() {
        let guide = deep_solve_lesson();
        let mut log = EventLog::default();
        log.enter(&guide, 0, false);
        log.record(DEEP_SOLVE_FINISHED);
        log.enter(&guide, 1, false);
        assert!(log.as_slice().is_empty());
    }

    #[test]
    fn a_repeated_start_invalidates_an_older_finish() {
        let guide = deep_solve_lesson();
        let mut log = EventLog::default();
        log.enter(&guide, 0, false);
        log.record(DEEP_SOLVE_STARTED);
        log.record(DEEP_SOLVE_FINISHED);
        log.record(DEEP_SOLVE_STARTED);
        log.enter(&guide, 1, false);
        assert!(log.as_slice().is_empty());
    }

    #[test]
    fn another_lesson_carries_nothing_and_clear_forgets_everything() {
        let guide = deep_solve_lesson();
        let other = Guide::new(
            "events-other",
            "Other",
            "Another lesson.",
            GuideCategory::Solving,
        )
        .step(GuideStep::new("One", "Read."))
        .step(
            GuideStep::new("Two", "Wait.")
                .goal(Goal::Event(DEEP_SOLVE_FINISHED.into()), "the run finished"),
        );
        let mut log = EventLog::default();
        log.enter(&guide, 0, false);
        log.record(DEEP_SOLVE_STARTED);
        log.record(DEEP_SOLVE_FINISHED);
        log.enter(&other, 1, false);
        assert!(log.as_slice().is_empty());
        log.record("x");
        log.clear();
        assert!(log.as_slice().is_empty());
    }
}
