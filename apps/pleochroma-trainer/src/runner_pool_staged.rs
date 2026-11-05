//! Dependency scheduling with CPU workers released while optical tickets are pending.

use std::{
    any::Any,
    cmp::Ordering as CmpOrdering,
    collections::BinaryHeap,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{Arc, Mutex, PoisonError, RwLock, atomic::Ordering, mpsc},
    time::Duration,
};

use super::{Tail, work_pool};

/// Stable probe identities and cost estimates for one mutable item.
pub struct ProbePlan {
    pub probes: Vec<usize>,
    pub probe_cost: u64,
    pub finish_cost: u64,
}

/// Pending preparation is bounded independently of the number of CPU workers.
pub struct StagedLimits {
    pub threads: usize,
    pub pending: usize,
    pub shared: Option<Arc<PendingBudget>>,
}

/// Shared admission across rollout streams for started probes and finish work.
///
/// A reservation spans preparation, waiting, and completion, including parked
/// continuations. Completed probe values are released from this budget: they must
/// remain available until their item's probes join, without blocking later probes.
/// Rollout probe values are small `TierProbe` records, bounded by concurrent slots
/// and the number of free tiers, and contain no outstanding optical request.
pub struct PendingBudget {
    capacity: usize,
    state: Mutex<BudgetState>,
}

#[derive(Default)]
struct BudgetState {
    used: usize,
    idle: usize,
}

impl PendingBudget {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            state: Mutex::new(BudgetState::default()),
        }
    }

    fn register(&self) -> BudgetLease<'_> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.idle += 1;
        BudgetLease {
            budget: self,
            held: 0,
        }
    }
}

struct BudgetLease<'a> {
    budget: &'a PendingBudget,
    held: usize,
}

impl BudgetLease<'_> {
    fn acquire(&mut self) -> bool {
        let mut state = self
            .budget
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Borrow unused capacity, leaving a place for each other participant
        // without work in flight. An idle participant may still enter when
        // there are more participants than places, ensuring forward progress.
        let idle_others = state.idle - usize::from(self.held == 0);
        let spare = self.budget.capacity.saturating_sub(state.used);
        if spare == 0 || (self.held > 0 && spare <= idle_others) {
            return false;
        }
        if self.held == 0 {
            state.idle -= 1;
        }
        state.used += 1;
        self.held += 1;
        true
    }

    fn release(&mut self) {
        let mut state = self
            .budget
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.used -= 1;
        self.held -= 1;
        if self.held == 0 {
            state.idle += 1;
        }
    }
}

impl Drop for BudgetLease<'_> {
    fn drop(&mut self) {
        let mut state = self
            .budget
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.used -= self.held;
        if self.held == 0 {
            state.idle -= 1;
        }
    }
}

/// Readiness checks run on the caller and must not block or perform CPU preparation.
///
/// Every other method runs in the bounded CPU pool. Probes share the unchanged item;
/// its finalizer starts only when all its own probes have completed.
pub trait ItemStages<T>: Sync {
    type ProbeWork: Send;
    type Probe: Send;
    type FinishWork: Send;
    type Output: Send;

    fn prepare_probe(&self, item: usize, value: &T, probe: usize, tail: &Tail) -> Self::ProbeWork;
    fn poll_probe(&self, work: &mut Self::ProbeWork) -> bool;
    fn complete_probe(
        &self,
        item: usize,
        value: &T,
        probe: usize,
        work: Self::ProbeWork,
    ) -> Self::Probe;
    fn begin_finish(
        &self,
        item: usize,
        value: &mut T,
        probes: Vec<(usize, Self::Probe)>,
        tail: &Tail,
    ) -> Self::FinishWork;
    fn poll_finish(&self, work: &mut Self::FinishWork) -> bool;
    /// None parks the continuation until `poll_finish` makes it runnable again.
    fn advance_finish(
        &self,
        item: usize,
        value: &mut T,
        work: &mut Self::FinishWork,
        tail: &Tail,
    ) -> Option<Self::Output>;
}

enum Job<W, F> {
    Prepare { item: usize, probe: usize },
    Complete { item: usize, probe: usize, work: W },
    Begin { item: usize },
    Advance { item: usize, work: F },
}

struct Ready<W, F> {
    cost: u64,
    identity: (usize, usize, usize),
    job: Job<W, F>,
}

impl<W, F> PartialEq for Ready<W, F> {
    fn eq(&self, other: &Self) -> bool {
        (self.cost, self.identity) == (other.cost, other.identity)
    }
}
impl<W, F> Eq for Ready<W, F> {}
impl<W, F> PartialOrd for Ready<W, F> {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}
impl<W, F> Ord for Ready<W, F> {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        self.cost
            .cmp(&other.cost)
            .then_with(|| other.identity.cmp(&self.identity))
    }
}

struct Waiting<W> {
    item: usize,
    probe: usize,
    work: W,
}

enum Event<W, P, F, R> {
    Prepared(Waiting<W>),
    Probed { item: usize, probe: usize, value: P },
    Parked { item: usize, work: F },
    Finished { item: usize, value: R },
    Panicked(Box<dyn Any + Send>),
}

/// Cost-ranked probes unlock their own finalizer, without a global probe barrier. The caller
/// polls tickets outside the pool. Results and probe inputs retain their original index order.
pub fn run_staged_items_ranked<T, S>(
    items: &mut [T],
    plans: &[ProbePlan],
    limits: StagedLimits,
    stages: S,
) -> Vec<S::Output>
where
    T: Send + Sync,
    S: ItemStages<T>,
{
    run_with_pool(items, plans, limits, stages, work_pool())
}

fn run_with_pool<T, S>(
    items: &mut [T],
    plans: &[ProbePlan],
    limits: StagedLimits,
    stages: S,
    pool: Option<&rayon::ThreadPool>,
) -> Vec<S::Output>
where
    T: Send + Sync,
    S: ItemStages<T>,
{
    assert_eq!(items.len(), plans.len(), "each item needs a probe plan");
    let cells: Vec<_> = items.iter_mut().map(RwLock::new).collect();
    let width = pool.map_or(1, rayon::ThreadPool::current_num_threads);
    let limits = StagedLimits {
        threads: limits.threads.max(1).min(width),
        pending: limits.pending.max(1),
        shared: limits.shared,
    };
    let tail = Tail {
        unfinished: std::sync::atomic::AtomicUsize::new(0),
        pool_width: width,
        shared_pool: super::is_shared_work_pool(pool),
    };
    // Coordination remains on the caller, so every pool thread can run CPU or nested work.
    let result = match pool {
        Some(pool) => {
            pool.in_place_scope(|scope| drive(Some(scope), &cells, plans, &limits, &stages, &tail))
        }
        None => drive(None, &cells, plans, &limits, &stages, &tail),
    };
    match result {
        Ok(results) => results,
        Err(panic) => resume_unwind(panic),
    }
}

type Outcome<R> = Result<Vec<R>, Box<dyn Any + Send>>;
type StageEvent<T, S> = Event<
    <S as ItemStages<T>>::ProbeWork,
    <S as ItemStages<T>>::Probe,
    <S as ItemStages<T>>::FinishWork,
    <S as ItemStages<T>>::Output,
>;

struct State<W, P, F, R> {
    starting: BinaryHeap<Ready<W, F>>,
    ready: BinaryHeap<Ready<W, F>>,
    waiting: Vec<Waiting<W>>,
    finishing: Vec<(usize, F)>,
    remaining: Vec<usize>,
    values: Vec<Vec<Option<P>>>,
    results: Vec<Option<R>>,
    active: usize,
    pending: usize,
    finished: usize,
}

impl<W, P, F, R> State<W, P, F, R> {
    fn new(plans: &[ProbePlan]) -> Self {
        let mut starting = BinaryHeap::new();
        for (item, plan) in plans.iter().enumerate() {
            if plan.probes.is_empty() {
                starting.push(begin_job(item, plan.finish_cost));
            }
            for probe in 0..plan.probes.len() {
                starting.push(Ready {
                    cost: plan.probe_cost,
                    identity: (item, probe, 0),
                    job: Job::Prepare { item, probe },
                });
            }
        }
        Self {
            starting,
            ready: BinaryHeap::new(),
            waiting: Vec::new(),
            finishing: Vec::new(),
            remaining: plans.iter().map(|plan| plan.probes.len()).collect(),
            values: plans
                .iter()
                .map(|plan| (0..plan.probes.len()).map(|_| None).collect())
                .collect(),
            results: (0..plans.len()).map(|_| None).collect(),
            active: 0,
            pending: 0,
            finished: 0,
        }
    }

    /// Moves completed tickets and runnable continuations to the ready queue; returns whether
    /// any moved.
    fn poll<T, S>(&mut self, plans: &[ProbePlan], stages: &S) -> bool
    where
        S: ItemStages<T, ProbeWork = W, Probe = P, FinishWork = F, Output = R>,
    {
        let mut moved = false;
        let mut index = 0;
        while index < self.waiting.len() {
            if !stages.poll_probe(&mut self.waiting[index].work) {
                index += 1;
                continue;
            }
            let Waiting { item, probe, work } = self.waiting.swap_remove(index);
            self.ready.push(Ready {
                cost: plans[item].probe_cost,
                identity: (item, probe, 1),
                job: Job::Complete { item, probe, work },
            });
            moved = true;
        }
        let mut index = 0;
        while index < self.finishing.len() {
            if !stages.poll_finish(&mut self.finishing[index].1) {
                index += 1;
                continue;
            }
            let (item, work) = self.finishing.swap_remove(index);
            self.ready.push(Ready {
                cost: plans[item].finish_cost,
                identity: (item, usize::MAX, 3),
                job: Job::Advance { item, work },
            });
            moved = true;
        }
        moved
    }

    fn next(
        &mut self,
        limits: &StagedLimits,
        lease: &mut Option<BudgetLease<'_>>,
    ) -> Option<Ready<W, F>> {
        let start_next = self.pending < limits.pending
            && self
                .starting
                .peek()
                .is_some_and(|next| self.ready.peek().is_none_or(|other| next > other));
        let start_next = start_next && lease.as_mut().is_none_or(BudgetLease::acquire);
        let next = if start_next {
            self.starting.pop()
        } else {
            self.ready.pop()
        }?;
        self.pending += usize::from(matches!(&next.job, Job::Prepare { .. } | Job::Begin { .. }));
        self.active += 1;
        Some(next)
    }

    fn workload(&self, limits: &StagedLimits) -> usize {
        self.active + self.ready.len() + self.starting.len().min(limits.pending - self.pending)
    }

    fn accept(
        &mut self,
        event: Event<W, P, F, R>,
        plans: &[ProbePlan],
    ) -> Result<(), Box<dyn Any + Send>> {
        self.active -= 1;
        match event {
            Event::Prepared(work) => self.waiting.push(work),
            Event::Probed { item, probe, value } => {
                self.pending -= 1;
                self.values[item][probe] = Some(value);
                self.remaining[item] -= 1;
                if self.remaining[item] == 0 {
                    self.starting.push(begin_job(item, plans[item].finish_cost));
                }
            }
            Event::Parked { item, work } => self.finishing.push((item, work)),
            Event::Finished { item, value } => {
                self.pending -= 1;
                self.results[item] = Some(value);
                self.finished += 1;
            }
            Event::Panicked(panic) => return Err(panic),
        }
        Ok(())
    }
}

/// How long the coordinator waits for an event while optical tickets are pending.
///
/// Tickets send no event when they complete, so the coordinator polls them. Each wait that ends
/// without an event doubles the next one from [`Self::FIRST`] up to [`Self::LONGEST`]; an event
/// or a completed ticket resets it. A burst of completions is still seen within tens of
/// microseconds, while a long optical wait costs each coordinator about a thousand wake-ups per
/// second instead of twenty thousand.
struct PollBackoff {
    wait: Duration,
}

impl PollBackoff {
    const FIRST: Duration = Duration::from_micros(50);
    const LONGEST: Duration = Duration::from_millis(1);

    const fn new() -> Self {
        Self { wait: Self::FIRST }
    }

    const fn reset(&mut self) {
        self.wait = Self::FIRST;
    }

    fn grow(&mut self) {
        self.wait = self.wait.saturating_mul(2).min(Self::LONGEST);
    }
}

fn drive<'scope, T, S>(
    scope: Option<&rayon::Scope<'scope>>,
    cells: &'scope [RwLock<&mut T>],
    plans: &'scope [ProbePlan],
    limits: &StagedLimits,
    stages: &'scope S,
    tail: &'scope Tail,
) -> Outcome<S::Output>
where
    T: Send + Sync,
    S: ItemStages<T>,
    S::ProbeWork: 'scope,
    S::Probe: 'scope,
    S::FinishWork: 'scope,
    S::Output: 'scope,
{
    let (send, receive) = mpsc::channel();
    let mut state = State::new(plans);
    let mut lease = limits.shared.as_ref().map(|budget| budget.register());
    let mut backoff = PollBackoff::new();
    while state.finished < plans.len() {
        match catch_unwind(AssertUnwindSafe(|| state.poll::<T, S>(plans, stages))) {
            Ok(true) => backoff.reset(),
            Ok(false) => {}
            Err(panic) => {
                while state.active > 0 {
                    if receive.recv().is_err() {
                        break;
                    }
                    state.active -= 1;
                }
                drop(state);
                return Err(panic);
            }
        }
        while state.active < limits.threads {
            let Some(next) = state.next(limits, &mut lease) else {
                break;
            };
            let probes = if let Job::Begin { item } = &next.job {
                plans[*item]
                    .probes
                    .iter()
                    .copied()
                    .zip(
                        state.values[*item]
                            .iter_mut()
                            .map(|value| value.take().expect("all probes completed before finish")),
                    )
                    .collect()
            } else {
                Vec::new()
            };
            tail.unfinished
                .store(state.workload(limits), Ordering::Relaxed);
            let send = send.clone();
            // Counted against the shared pool's spare capacity until a worker starts it.
            let queued = super::SharedQueuedGuard::enter(tail.shared_pool);
            let execute = move || {
                let event = {
                    let _active = queued.start();
                    catch_unwind(AssertUnwindSafe(|| {
                        execute_job(cells, plans, stages, tail, next.job, probes)
                    }))
                    .unwrap_or_else(Event::Panicked)
                };
                let _ = send.send(event);
            };
            if let Some(scope) = scope {
                scope.spawn(move |_| execute());
            } else {
                execute();
            }
        }
        tail.unfinished
            .store(state.workload(limits), Ordering::Relaxed);
        // Only the coordinator waits; all pool threads remain available for nested work.
        // Pending tickets are polled with a growing wait, since their completion sends nothing.
        let tickets = !state.waiting.is_empty() || !state.finishing.is_empty();
        let poll_timeout = if tickets {
            backoff.wait
        } else {
            PollBackoff::LONGEST
        };
        let event = match receive.recv_timeout(poll_timeout) {
            Ok(event) => {
                backoff.reset();
                event
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if tickets {
                    backoff.grow();
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                unreachable!("the coordinator retains an event sender")
            }
        };
        let releases = matches!(&event, Event::Probed { .. } | Event::Finished { .. });
        if let Err(panic) = state.accept(event, plans) {
            // Keep the shared reservations until all spawned jobs have returned and their
            // payloads have been dropped, even when a stage panics.
            while state.active > 0 {
                if receive.recv().is_err() {
                    break;
                }
                state.active -= 1;
            }
            drop(state);
            return Err(panic);
        }
        if releases && let Some(lease) = &mut lease {
            lease.release();
        }
    }
    Ok(state
        .results
        .into_iter()
        .map(|value| value.expect("every item finishes exactly once"))
        .collect())
}

fn execute_job<T, S>(
    cells: &[RwLock<&mut T>],
    plans: &[ProbePlan],
    stages: &S,
    tail: &Tail,
    job: Job<S::ProbeWork, S::FinishWork>,
    probes: Vec<(usize, S::Probe)>,
) -> StageEvent<T, S>
where
    T: Send + Sync,
    S: ItemStages<T>,
{
    match job {
        Job::Prepare { item, probe } => {
            let guard = cells[item].read().unwrap_or_else(PoisonError::into_inner);
            Event::Prepared(Waiting {
                item,
                probe,
                work: stages.prepare_probe(item, &guard, plans[item].probes[probe], tail),
            })
        }
        Job::Complete { item, probe, work } => {
            let guard = cells[item].read().unwrap_or_else(PoisonError::into_inner);
            Event::Probed {
                item,
                probe,
                value: stages.complete_probe(item, &guard, plans[item].probes[probe], work),
            }
        }
        Job::Begin { item } => {
            let mut guard = cells[item].write().unwrap_or_else(PoisonError::into_inner);
            Event::Parked {
                item,
                work: stages.begin_finish(item, &mut guard, probes, tail),
            }
        }
        Job::Advance { item, mut work } => {
            let mut guard = cells[item].write().unwrap_or_else(PoisonError::into_inner);
            match stages.advance_finish(item, &mut guard, &mut work, tail) {
                Some(value) => Event::Finished { item, value },
                None => Event::Parked { item, work },
            }
        }
    }
}

fn begin_job<W, F>(item: usize, cost: u64) -> Ready<W, F> {
    Ready {
        cost,
        identity: (item, usize::MAX, 2),
        job: Job::Begin { item },
    }
}

#[cfg(test)]
#[path = "runner_pool_staged_tests.rs"]
mod tests;
