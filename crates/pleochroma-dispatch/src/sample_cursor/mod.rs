//! [`SampleCursor`]: the shared claim point from which every backend contributing to one
//! image draws disjoint absolute sample sub-ranges, including each lane of a
//! [`crate::LanePool`].
//!
//! # Why a shared claim point, not a one-shot split
//!
//! A fixed up-front split can leave a fast engine idle once its slice is finished while
//! the render keeps going. Every engine instead pulls fresh work from the shared point
//! whenever it is free, for as long as any remains.
//!
//! # Disjointness by construction
//!
//! [`claim`](SampleCursor::claim) and [`claim_local`](SampleCursor::claim_local) hand
//! out fresh ranges via an atomic compare-and-swap, so callers on any number of threads
//! never receive overlapping indices. Disjointness matters because the CPU tracer and
//! GPU megakernel derive each sample's jitter and RNG draws from its *absolute* index:
//! overlapping ranges would redraw identical samples and silently bias the average.
//!
//! # Two retry piles
//!
//! A range a lane failed to finish must be traced again, exactly once, by someone.
//!
//! - [`return_to_local`](SampleCursor::return_to_local) (the desktop's single-remote
//!   model) pushes it onto a pile only [`claim_local`](SampleCursor::claim_local) /
//!   [`claim_local_bounded`](SampleCursor::claim_local_bounded) drain, never back onto
//!   the shared pool [`claim`](SampleCursor::claim) draws from. Otherwise remote could
//!   re-claim its own just-failed range on its very next iteration -- an infinite loop
//!   for a persistent (not transient) failure. Routing it to a local-only pile
//!   guarantees it is retried at most once more, locally.
//! - [`requeue`](SampleCursor::requeue) (the [`crate::LanePool`] model, where every
//!   lane is equal) pushes it onto a pile [`claim_any`](SampleCursor::claim_any) drains
//!   before fresh work. The pool itself prevents the infinite loop: a failing lane
//!   backs off and is retired after a bounded number of consecutive failures.
//!
//! The two models are never mixed on one cursor in practice; [`claim`] alone never
//! sees either pile.
//!
//! [`claim`]: SampleCursor::claim

use std::{
    collections::VecDeque,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU32, Ordering},
    },
};

#[cfg(test)]
mod target_tests;
#[cfg(test)]
mod tests;

/// See this module's doc comment. Every method takes `&self`, so one instance can be
/// shared (by reference inside a `thread::scope`, or behind an `Arc`) across every
/// lane's own thread.
#[derive(Debug)]
pub struct SampleCursor {
    /// Absolute index of the next sample NO engine has claimed yet. Every claim is one
    /// atomic compare-and-swap against this -- see [`claim`](Self::claim).
    next: AtomicU32,
    /// One past the last sample in this budget (exclusive). A claim is never honoured
    /// past this, however large a range was requested.
    end: AtomicU32,
    /// Ranges a remote chunk failed to finish, reserved for the local lane alone -- see
    /// this module's doc comment.
    local_retry: Mutex<VecDeque<(u32, u32)>>,
    /// Ranges a pool lane failed to finish, open to every lane via
    /// [`claim_any`](Self::claim_any).
    requeued: Mutex<VecDeque<(u32, u32)>>,
}

impl SampleCursor {
    /// Builds a cursor over `[start, end)`. `start` is the first sample still to trace,
    /// which is not 0 when earlier samples were already traced.
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self {
            next: AtomicU32::new(start),
            end: AtomicU32::new(end),
            local_retry: Mutex::new(VecDeque::new()),
            requeued: Mutex::new(VecDeque::new()),
        }
    }

    /// One past the last sample of this budget.
    #[must_use]
    pub fn end(&self) -> u32 {
        self.end.load(Ordering::Acquire)
    }

    /// Changes the stopping target without reissuing any previously claimed samples.
    ///
    /// Claims already in flight remain valid after a reduction. Retry ranges above
    /// the new target are retained for a later increase, but are not handed out now.
    pub fn set_end(&self, end: u32) {
        self.end.store(end, Ordering::Release);
    }

    /// Claims up to `want` fresh samples for ANY engine, returning `Some((start,
    /// count))` with `1 <= count <= want`, or `None` if the budget is already
    /// exhausted. Never returns a range past `end`, however large `want` is.
    ///
    /// `want == 0` always returns `None` rather than a zero-length range.
    ///
    /// # A CAS that never advances past `end`
    ///
    /// `next` is updated with [`AtomicU32::try_update`], whose closure refuses (`None`)
    /// once `next >= end`, leaving `next` untouched. A plain `fetch_add` would advance
    /// `next` on every call, so an idle lane polling [`claim_any`](Self::claim_any) while
    /// another lane's chunk is in flight could wrap `next` and hand out a range already
    /// traced. With the CAS, a call that finds nothing left costs nothing.
    ///
    /// # A concurrent [`set_end`](Self::set_end)
    ///
    /// `end` is read once, before the CAS, so a claim racing a reduction may still hand
    /// out a range up to the old end. This follows the in-flight rule of
    /// [`set_end`](Self::set_end): a claim counts as assigned from the moment it reads
    /// `end`, and assigned work stays valid and is merged. A hard stop would need `next`
    /// and `end` packed in one atomic.
    pub fn claim(&self, want: u32) -> Option<(u32, u32)> {
        if want == 0 {
            return None;
        }
        let end = self.end();
        let start = self
            .next
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                (current < end).then(|| current.saturating_add(want).min(end))
            })
            .ok()?;
        Some((start, want.min(end - start)))
    }

    /// Claims for the LOCAL lane only: a previously remote-failed range first (the
    /// whole portion below the current target, regardless of `want`), falling back
    /// to [`claim`](Self::claim) once no retry is eligible. See this module's doc
    /// comment for why a retried range never goes back to remote. Callers that trace one
    /// short frame per claim use [`claim_local_bounded`](Self::claim_local_bounded).
    pub fn claim_local(&self, want: u32) -> Option<(u32, u32)> {
        let retried = pop_bounded(&self.local_retry, u32::MAX, self.end());
        if retried.is_some() {
            return retried;
        }
        self.claim(want)
    }

    /// Like [`claim_local`](Self::claim_local), but never hands out more than `want`
    /// samples even from the retry pile: a longer retried range is split, its head
    /// returned now and its tail pushed back to the FRONT of the pile for the next
    /// call. The live viewport traces one short frame per claim, so a whole failed
    /// remote chunk (possibly hundreds of samples) must not land in a single frame.
    /// `want == 0` returns `None` without touching anything.
    pub fn claim_local_bounded(&self, want: u32) -> Option<(u32, u32)> {
        if want == 0 {
            return None;
        }
        pop_bounded(&self.local_retry, want, self.end()).or_else(|| self.claim(want))
    }

    /// Claims for a [`crate::LanePool`] lane: up to `want` samples of a requeued range
    /// first (split like [`claim_local_bounded`](Self::claim_local_bounded)), falling
    /// back to fresh work from [`claim`](Self::claim). Never touches the local-only
    /// retry pile. `want == 0` returns `None` without touching anything.
    pub fn claim_any(&self, want: u32) -> Option<(u32, u32)> {
        if want == 0 {
            return None;
        }
        pop_bounded(&self.requeued, want, self.end()).or_else(|| self.claim(want))
    }

    /// Whether the SHARED pool has nothing left to claim -- `true` once every sample up
    /// to `end` has been handed out to some engine. Says nothing about the retry
    /// piles. A cheap, lock-free read for callers deciding whether waiting around could
    /// still yield work.
    #[must_use]
    pub fn shared_pool_exhausted(&self) -> bool {
        self.next.load(Ordering::Relaxed) >= self.end()
    }

    /// Whether nothing at all is claimable right now: the shared pool is exhausted and
    /// neither retry pile holds work below the current target. A range being traced may
    /// still come back through [`requeue`](Self::requeue) or
    /// [`return_to_local`](Self::return_to_local) if its lane fails.
    #[must_use]
    pub fn fully_claimed(&self) -> bool {
        self.shared_pool_exhausted()
            && self
                .requeued
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .all(|&(start, count)| count == 0 || start >= self.end())
            && self
                .local_retry
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .all(|&(start, count)| count == 0 || start >= self.end())
    }

    /// Returns the unfinished part of an earlier claim to the local-only retry queue.
    ///
    /// `[start, start + count)` is never re-offered to remote. Samples above a reduced
    /// target wait for a later target increase. A `count` of `0` is a no-op (nothing
    /// to retry) rather than an empty queue entry.
    pub fn return_to_local(&self, start: u32, count: u32) {
        if count == 0 {
            return;
        }
        self.local_retry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back((start, count));
    }

    /// Returns `[start, start + count)` to the pile every [`claim_any`](Self::claim_any)
    /// caller drains first, after a pool lane failed to finish it. A `count` of `0` is
    /// a no-op.
    pub fn requeue(&self, start: u32, count: u32) {
        if count == 0 {
            return;
        }
        self.requeued
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back((start, count));
    }
}

/// Pops at most `want` samples off the front of `pile`, pushing a longer entry's tail
/// back to the front for the next caller. `want` must be non-zero.
fn pop_bounded(pile: &Mutex<VecDeque<(u32, u32)>>, want: u32, end: u32) -> Option<(u32, u32)> {
    let mut pile = pile.lock().unwrap_or_else(PoisonError::into_inner);
    let position = pile
        .iter()
        .position(|&(start, count)| start < end && count > 0)?;
    let (start, count) = pile.remove(position)?;
    let taken = count.min(want).min(end - start);
    if taken < count {
        pile.insert(position, (start + taken, count - taken));
    }
    drop(pile);
    Some((start, taken))
}
