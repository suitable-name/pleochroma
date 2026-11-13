//! Coalescing UI edit queue: applies UI edits once per frame instead of once per raw event.
//!
//! Angle nudges, Cut-slider drags and Retarget crown-slider drags each post an [`EditIntent`].
//! Undo steps already merge in `History::apply_coalescing`. This queue also batches the
//! UI-thread work (cloning `Design`, rebuilding models, replanning the preview), which would
//! otherwise run at the widget's `changed` rate.
//!
//! [`EditIntentQueue::post`] merges each [`EditIntent`] into the pending one when
//! [`EditIntent::merge`] allows (two nudges on the same tier sum their deltas), and replaces it
//! otherwise (a slider keeps only its newest position). The first post starts a 16 ms
//! `slint::Timer`, one frame at 60 Hz. Each tick drains the pending intent to `on_drain` once,
//! and the timer stops when nothing is pending.
//!
//! Three call sites use it: `callbacks::tier_actions::setup_nudge_angle_callback`,
//! `gui::editor::setup::setup_tier_cutoff_callback` and
//! `callbacks::retarget_actions::setup_retarget_proposal_changed_callback`. Each owns its own
//! queue and posts only one kind of [`EditIntent`]. The shared enum and queue keep the
//! coalescing identical across the three.

use slint::{Timer, TimerMode};
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

// The intent enum and its merge rule live in `pleochroma_editor::edit_intent`, shared with the
// web app. `DRAIN_INTERVAL` is one frame at 60 Hz, the budget `stall_guard::STALL_THRESHOLD` uses.
use pleochroma_editor::edit_intent::DRAIN_INTERVAL;
pub(super) use pleochroma_editor::edit_intent::EditIntent;

/// A single-slot queue: posts coalesce into one pending intent, drained by a timer tick.
/// `on_drain` runs at most once per [`DRAIN_INTERVAL`], and only when something is pending.
pub(super) struct EditIntentQueue {
    pending: RefCell<Option<EditIntent>>,
    on_drain: Box<dyn Fn(EditIntent)>,
    timer: Timer,
    /// A `Weak` handle to this queue, set by [`Rc::new_cyclic`]. The timer's tick closure
    /// needs it; see [`Self::post`].
    self_weak: Weak<Self>,
}

impl EditIntentQueue {
    /// Builds an empty queue. The drain timer starts on the first [`Self::post`].
    pub(super) fn new(on_drain: impl Fn(EditIntent) + 'static) -> Rc<Self> {
        Rc::new_cyclic(|weak_self| Self {
            pending: RefCell::new(None),
            on_drain: Box::new(on_drain),
            timer: Timer::default(),
            self_weak: weak_self.clone(),
        })
    }

    /// Coalesces `intent` into the pending one (see [`EditIntent::merge`]) and starts the
    /// drain timer if the queue was empty. Posts while the timer runs only update `pending`.
    ///
    /// # Why the tick closure holds a `Weak`
    ///
    /// The timer is a field of this queue, so a strong `Rc` back to the queue in the closure
    /// would be a reference cycle that keeps the queue, timer included, alive forever. With a
    /// `Weak`, the upgrade fails once the owning `Rc` (held by the `on_*` callback) is gone.
    pub(super) fn post(&self, intent: EditIntent) {
        let was_empty = {
            let mut pending = self.pending.borrow_mut();
            let was_empty = pending.is_none();
            let merged = pending
                .as_mut()
                .is_some_and(|existing| existing.merge(&intent));
            if !merged {
                *pending = Some(intent);
            }
            was_empty
        };
        if !was_empty {
            // The timer is already running (or about to tick) and will pick up
            // the just-merged value -- nothing else to do.
            return;
        }
        let queue = self.self_weak.clone();
        self.timer
            .start(TimerMode::Repeated, DRAIN_INTERVAL, move || {
                let Some(queue) = queue.upgrade() else {
                    return;
                };
                let Some(intent) = queue.pending.borrow_mut().take() else {
                    queue.timer.stop();
                    return;
                };
                (queue.on_drain)(intent);
            });
    }
}
