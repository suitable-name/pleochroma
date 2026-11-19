//! Shared device budget for the compute jobs (renders, exports, analysis and solves).
//! A job holds one permit while it runs; capacity 0 pauses all.

use std::{
    sync::{Condvar, Mutex, PoisonError},
    time::Duration,
};

/// One outstanding permit: its ticket and whether it was given up by [`ComputePolicy::forget_one`].
struct Holder {
    ticket: u64,
    forgotten: bool,
}

struct Budget {
    capacity: usize,
    closed: bool,
    next_ticket: u64,
    /// Every permit not dropped yet, oldest first.
    holders: Vec<Holder>,
}

impl Budget {
    /// The permits that count against the capacity: all but the forgotten ones.
    fn active(&self) -> usize {
        self.holders
            .iter()
            .filter(|holder| !holder.forgotten)
            .count()
    }
}

pub struct ComputePolicy {
    budget: Mutex<Budget>,
    wake: Condvar,
}

impl ComputePolicy {
    #[must_use]
    pub const fn new(capacity: usize) -> Self {
        Self {
            budget: Mutex::new(Budget {
                capacity,
                closed: false,
                next_ticket: 0,
                holders: Vec::new(),
            }),
            wake: Condvar::new(),
        }
    }

    pub fn set_capacity(&self, capacity: usize) {
        self.budget
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .capacity = capacity;
        self.wake.notify_all();
    }

    #[must_use]
    pub fn paused(&self) -> bool {
        let budget = self.budget.lock().unwrap_or_else(PoisonError::into_inner);
        budget.capacity == 0 || budget.closed
    }

    /// Waits off the UI thread, checking cancellation at least every 50 ms.
    pub fn acquire(&self, cancelled: &dyn Fn() -> bool) -> Option<ComputePermit<'_>> {
        loop {
            // Never call engine cancellation while holding the budget lock: engines
            // may hold their own control lock when updating this budget.
            if cancelled() {
                return None;
            }
            let mut budget = self.budget.lock().unwrap_or_else(PoisonError::into_inner);
            if budget.closed {
                return None;
            }
            if budget.active() < budget.capacity {
                let ticket = budget.next_ticket;
                budget.next_ticket += 1;
                budget.holders.push(Holder {
                    ticket,
                    forgotten: false,
                });
                return Some(ComputePermit {
                    policy: self,
                    ticket,
                });
            }
            drop(
                self.wake
                    .wait_timeout(budget, Duration::from_millis(50))
                    .unwrap_or_else(PoisonError::into_inner),
            );
        }
    }

    /// Gives back the permit of a thread that was abandoned while it held one.
    ///
    /// The oldest permit still counted is the one a hung GPU step holds, so that one stops
    /// counting against the capacity at once. The abandoned thread's own drop of it, if it ever
    /// comes, changes nothing, and a permit taken later is never mistaken for it.
    pub fn forget_one(&self) {
        {
            let mut budget = self.budget.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(holder) = budget.holders.iter_mut().find(|holder| !holder.forgotten) {
                holder.forgotten = true;
            }
        }
        self.wake.notify_all();
    }

    /// How many permits count against the capacity now.
    #[must_use]
    pub fn active(&self) -> usize {
        self.budget
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active()
    }

    pub fn close(&self) {
        self.budget
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed = true;
        self.wake.notify_all();
    }
}

pub struct ComputePermit<'a> {
    policy: &'a ComputePolicy,
    ticket: u64,
}

impl Drop for ComputePermit<'_> {
    fn drop(&mut self) {
        let mut budget = self
            .policy
            .budget
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // A forgotten permit no longer counted; dropping it late changes nothing else.
        budget.holders.retain(|holder| holder.ticket != self.ticket);
        drop(budget);
        self.policy.wake.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_budget_obeys_cancellation_and_releases_capacity() {
        let policy = ComputePolicy::new(1);
        let permit = policy.acquire(&|| false).expect("capacity");
        assert!(policy.acquire(&|| true).is_none());
        drop(permit);
        policy.set_capacity(0);
        assert!(policy.paused());
        assert!(policy.acquire(&|| true).is_none());
        policy.set_capacity(1);
        assert!(policy.acquire(&|| false).is_some());
        policy.close();
        assert!(policy.acquire(&|| false).is_none());
    }

    #[test]
    fn forget_one_returns_the_oldest_permit_and_its_late_drop_changes_nothing() {
        let policy = ComputePolicy::new(2);
        // The "hung" holder took its permit first and never lets go in this test's time.
        let hung = policy.acquire(&|| false).expect("first permit");
        let live = policy.acquire(&|| false).expect("second permit");
        assert_eq!(policy.active(), 2);
        assert!(policy.acquire(&|| true).is_none(), "the budget is full");

        policy.forget_one();
        assert_eq!(policy.active(), 1, "the hung permit stopped counting");
        let third = policy.acquire(&|| false).expect("the returned capacity");
        assert_eq!(policy.active(), 2);

        // A permit taken after the abandon is never taken for the forgotten one: dropping it
        // frees its own slot.
        drop(live);
        assert_eq!(policy.active(), 1);
        // The abandoned holder finally ends: nothing underflows, nothing is freed twice.
        drop(hung);
        assert_eq!(policy.active(), 1);
        drop(third);
        assert_eq!(policy.active(), 0);
        // Forgetting with nothing outstanding is a no-op.
        policy.forget_one();
        assert_eq!(policy.active(), 0);
    }
}
