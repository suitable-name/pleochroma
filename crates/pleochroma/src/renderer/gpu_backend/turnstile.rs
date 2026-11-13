//! The FIFO ticket lock [`backend::GpuBackend`](super::backend::GpuBackend) drives for
//! chunk-level fairness among concurrent GPU requests -- see the parent module's doc
//! comment's "Concurrency: chunk-level fairness" section for the correctness argument
//! this exists to support.

/// Chunks dispatched per turn before the renderer is released and the ticket rejoins the
/// back of the [`Turnstile`] queue.
///
/// Two is the minimum that keeps the chunk pipeline's overlap: chunk i is dispatched while
/// chunk i-1 is read back. A turn drains before it returns, so a one-chunk turn would block
/// on its own dispatch. Larger values mean fewer scene re-uploads but coarser fairness. A
/// turn lasts about two chunk durations; each chunk is sized toward a target time once the
/// chunk timing has calibrated.
pub(super) const CHUNKS_PER_TURN: usize = 2;

/// A FIFO ticket lock.
///
/// [`Self::take_ticket`] hands out tickets in call order. [`Self::wait_for_turn`] blocks
/// until its ticket is served and returns an RAII [`TurnstileTurn`] that advances to the
/// next ticket on drop. Admission follows call order, which a plain `Mutex` does not
/// guarantee. Concurrent requests therefore alternate turns instead of one starving the
/// rest. Built on std only: an `AtomicU64` counter plus a `Mutex`/`Condvar` "now serving"
/// pair.
pub(super) struct Turnstile {
    next_ticket: std::sync::atomic::AtomicU64,
    now_serving: std::sync::Mutex<u64>,
    turn_taken: std::sync::Condvar,
}

impl Turnstile {
    pub(super) const fn new() -> Self {
        Self {
            next_ticket: std::sync::atomic::AtomicU64::new(0),
            now_serving: std::sync::Mutex::new(0),
            turn_taken: std::sync::Condvar::new(),
        }
    }

    /// Claims the next ticket, in call order. Cheap and non-blocking: ordering among
    /// callers is decided HERE, at call time, not later when [`Self::wait_for_turn`]
    /// actually blocks -- two threads calling this back-to-back are served in that same
    /// order regardless of how long either later waits.
    pub(super) fn take_ticket(&self) -> u64 {
        self.next_ticket
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Whether any ticket was handed out AFTER `ticket` -- i.e. some other caller is
    /// queued (or about to queue) behind the holder of `ticket`.
    ///
    /// Lets a batch keep its turn (and its GPU queue full) while nobody else wants the
    /// renderer, and yield the moment somebody does. A caller that takes a ticket right
    /// after this returns `false` simply waits for the holder's next yield point, at most
    /// [`CHUNKS_PER_TURN`] chunks later.
    pub(super) fn has_waiters_behind(&self, ticket: u64) -> bool {
        self.next_ticket.load(std::sync::atomic::Ordering::Acquire) > ticket + 1
    }

    /// Blocks until `ticket` is being served, then returns a guard whose `Drop` advances
    /// `now_serving` and wakes every waiter -- so a turn is released exactly once, even
    /// if the caller returns early (a `?` on a GPU error, a cancellation), never leaked
    /// and never released twice.
    pub(super) fn wait_for_turn(&self, ticket: u64) -> TurnstileTurn<'_> {
        let mut serving = self
            .now_serving
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *serving != ticket {
            serving = self
                .turn_taken
                .wait(serving)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        drop(serving);
        TurnstileTurn { turnstile: self }
    }
}

/// RAII guard: holding it means holding the [`Turnstile`]'s current turn. Drop uses
/// `notify_all`, because only the next ticket can proceed. Turns are bounded by
/// [`CHUNKS_PER_TURN`], so the extra wakeups are cheap and a per-ticket condvar is not needed.
pub(super) struct TurnstileTurn<'a> {
    turnstile: &'a Turnstile,
}

impl Drop for TurnstileTurn<'_> {
    fn drop(&mut self) {
        let mut serving = self
            .turnstile
            .now_serving
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *serving += 1;
        drop(serving);
        self.turnstile.turn_taken.notify_all();
    }
}
