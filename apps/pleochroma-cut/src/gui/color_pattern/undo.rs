//! The Color Pattern editor's own Undo and Redo: a stack of pattern snapshots.
//!
//! The material dialog has no undo, so this stack and Cancel are the only way back before Save.
//! A slider drag changes its value many times a second; consecutive changes of the same control
//! within [`COALESCE`] of each other are one step. A button press, a template, a drag of a
//! handle (one step for the whole drag, see [`UndoStack::begin_group`]) is one step each.

use pleochroma_cut_core::material::pattern::ColorPattern;
use std::time::{Duration, Instant};

/// Changes of one control closer together than this are one undo step.
pub(super) const COALESCE: Duration = Duration::from_millis(700);

/// The most steps kept; the oldest is dropped beyond it.
const MAX_STEPS: usize = 100;

/// Which control made a change, so a run of changes of one control is one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditKey {
    /// Parameter row `row` of zone `zone` (Advanced).
    Param { zone: usize, row: usize },
    /// The Size slider of zone `zone`.
    Size { zone: usize },
    /// The boundary softness slider.
    Softness,
    /// The Tilt and Turn sliders of the Custom axis.
    TiltTurn,
}

/// Undo and Redo stacks of pattern snapshots.
#[derive(Debug, Default)]
pub(super) struct UndoStack {
    undo: Vec<ColorPattern>,
    redo: Vec<ColorPattern>,
    last: Option<(EditKey, Instant)>,
}

impl UndoStack {
    /// An empty history.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Records that `before` is about to be replaced. With a `key` equal to the previous change's
    /// and at most [`COALESCE`] after it, no step is added (the first change of the run already
    /// holds the state to return to). Always clears Redo.
    pub(super) fn record(&mut self, before: &ColorPattern, key: Option<EditKey>, now: Instant) {
        self.redo.clear();
        let joins_run = key.is_some_and(|key| {
            self.last
                .is_some_and(|(last, at)| last == key && now.duration_since(at) <= COALESCE)
        });
        self.last = key.map(|key| (key, now));
        if joins_run {
            return;
        }
        self.undo.push(before.clone());
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
    }

    /// Starts a step that several changes make (a handle drag): `before` is stored once and
    /// the changes that follow add nothing.
    pub(super) fn begin_group(&mut self, before: &ColorPattern) {
        self.record(before, None, Instant::now());
    }

    /// Ends a run of coalescing changes, so the next change starts a new step.
    pub(super) const fn break_run(&mut self) {
        self.last = None;
    }

    /// The state to go back to, with `current` moved to Redo; `None` when there is nothing to
    /// undo.
    pub(super) fn undo(&mut self, current: &ColorPattern) -> Option<ColorPattern> {
        let previous = self.undo.pop()?;
        self.redo.push(current.clone());
        self.last = None;
        Some(previous)
    }

    /// The state to bring back, with `current` moved to Undo; `None` when there is nothing to
    /// redo.
    pub(super) fn redo(&mut self, current: &ColorPattern) -> Option<ColorPattern> {
        let next = self.redo.pop()?;
        self.undo.push(current.clone());
        self.last = None;
        Some(next)
    }

    /// Whether there is a step to take back.
    pub(super) const fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is a step to bring back.
    pub(super) const fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::material::pattern::{PatternAxis, TemplateId, template};

    fn pattern(softness: f32) -> ColorPattern {
        ColorPattern {
            softness,
            ..template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL)
        }
    }

    #[test]
    fn a_run_of_one_slider_is_one_step() {
        let mut stack = UndoStack::new();
        let start = Instant::now();
        let key = Some(EditKey::Softness);
        stack.record(&pattern(0.0), key, start);
        stack.record(&pattern(0.1), key, start + Duration::from_millis(100));
        stack.record(&pattern(0.2), key, start + Duration::from_millis(200));
        let back = stack.undo(&pattern(0.3)).unwrap();
        assert!((back.softness - 0.0).abs() < f32::EPSILON);
        assert!(!stack.can_undo());
        assert!(stack.can_redo());
    }

    #[test]
    fn another_control_or_a_pause_starts_a_new_step() {
        let mut stack = UndoStack::new();
        let start = Instant::now();
        stack.record(&pattern(0.0), Some(EditKey::Softness), start);
        stack.record(
            &pattern(0.1),
            Some(EditKey::TiltTurn),
            start + Duration::from_millis(50),
        );
        stack.record(
            &pattern(0.2),
            Some(EditKey::TiltTurn),
            start + Duration::from_secs(5),
        );
        let mut steps = 0;
        let mut current = pattern(0.3);
        while let Some(previous) = stack.undo(&current) {
            current = previous;
            steps += 1;
        }
        assert_eq!(steps, 3);
    }

    #[test]
    fn a_change_without_a_key_is_its_own_step_and_clears_redo() {
        let mut stack = UndoStack::new();
        let now = Instant::now();
        stack.record(&pattern(0.0), None, now);
        let undone = stack.undo(&pattern(0.1)).unwrap();
        assert!(stack.can_redo());
        stack.record(&undone, None, now);
        assert!(!stack.can_redo());
        assert!(stack.can_undo());
    }

    #[test]
    fn redo_brings_back_what_undo_took() {
        let mut stack = UndoStack::new();
        stack.record(&pattern(0.0), None, Instant::now());
        let back = stack.undo(&pattern(0.5)).unwrap();
        let forward = stack.redo(&back).unwrap();
        assert!((forward.softness - 0.5).abs() < f32::EPSILON);
        assert!(stack.can_undo());
        assert!(!stack.can_redo());
    }

    #[test]
    fn the_history_is_bounded() {
        let mut stack = UndoStack::new();
        let now = Instant::now();
        for _ in 0..(MAX_STEPS + 20) {
            stack.record(&pattern(0.0), None, now);
        }
        assert_eq!(stack.undo.len(), MAX_STEPS);
    }
}
