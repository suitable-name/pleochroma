//! One persistent, bounded Rayon pool for CPU-bound environment work.
//!
//! Calls keep an indexed result vector even though Rayon steals uneven-cost tasks dynamically;
//! deterministic sampling and reductions therefore remain in the caller's index order. Inner
//! line searches may share spare capacity in this same pool once fewer slots remain than
//! workers. They enqueue Rayon tasks, so nested work cannot multiply the native thread count.

use std::{
    cmp::Reverse,
    collections::VecDeque,
    sync::{
        Mutex, OnceLock, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

use rayon::prelude::*;

#[path = "runner_pool_staged.rs"]
mod staged;

pub use staged::{ItemStages, ProbePlan, StagedLimits, run_staged_items_ranked};

static WORK_POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();

fn work_pool() -> Option<&'static rayon::ThreadPool> {
    WORK_POOL
        .get_or_init(|| {
            let threads = thread::available_parallelism()
                .map_or(1, usize::from)
                .max(1);
            match rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|index| format!("pleochroma-trainer-cpu-{index}"))
                .build()
            {
                Ok(pool) => Some(pool),
                Err(error) => {
                    eprintln!(
                        "warning: could not create trainer CPU pool ({error}); using serial work"
                    );
                    None
                }
            }
        })
        .as_ref()
}

fn ordered<R>(mut pairs: Vec<(usize, R)>) -> Vec<R> {
    pairs.sort_by_key(|(index, _)| *index);
    pairs.into_iter().map(|(_, result)| result).collect()
}

/// Useful pool tasks for `tasks` units of work: never more tasks than units.
#[must_use]
pub fn pass_threads(tasks: usize) -> usize {
    pool_threads().min(tasks)
}

/// Width of the persistent CPU pool (or the serial fallback width).
#[must_use]
pub fn pool_threads() -> usize {
    work_pool().map_or(1, rayon::ThreadPool::current_num_threads)
}

fn ranked(costs: &[u64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..costs.len()).collect();
    order.sort_by_key(|&index| Reverse(costs[index]));
    order
}

/// `work(i)` for every index, on at most `threads` Rayon tasks. Results are index ordered.
pub fn run_indexed<R, F>(tasks: usize, threads: usize, work: F) -> Vec<R>
where
    R: Send,
    F: Fn(usize) -> R + Sync,
{
    if tasks == 0 {
        return Vec::new();
    }
    let workers = threads.min(tasks).max(1);
    if workers == 1 || work_pool().is_none() {
        return (0..tasks).map(work).collect();
    }
    let next = AtomicUsize::new(0);
    ordered(
        work_pool()
            .expect("checked above")
            .install(|| {
                (0..workers)
                    .into_par_iter()
                    .map(|_| {
                        let mut results = Vec::new();
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            if index >= tasks {
                                return results;
                            }
                            results.push((index, work(index)));
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .flatten()
            .collect(),
    )
}

/// Whether unfinished units leave spare capacity for nested tasks in the current pool.
pub struct Tail {
    unfinished: AtomicUsize,
    pool_width: usize,
}

impl Tail {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.unfinished.load(Ordering::Relaxed) < self.pool_width
    }

    fn complete(&self) {
        self.unfinished.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Cost-ranked indexed work; output remains in original index order.
pub fn run_indexed_ranked<R, F>(costs: &[u64], threads: usize, work: F) -> Vec<R>
where
    R: Send,
    F: Fn(usize) -> R + Sync,
{
    if costs.is_empty() {
        return Vec::new();
    }
    let order = ranked(costs);
    let workers = threads.min(order.len()).max(1);
    if workers == 1 || work_pool().is_none() {
        return ordered(
            order
                .into_iter()
                .map(|index| (index, work(index)))
                .collect(),
        );
    }
    let next = AtomicUsize::new(0);
    ordered(
        work_pool()
            .expect("checked above")
            .install(|| {
                (0..workers)
                    .into_par_iter()
                    .map(|_| {
                        let mut results = Vec::new();
                        loop {
                            let rank = next.fetch_add(1, Ordering::Relaxed);
                            let Some(&index) = order.get(rank) else {
                                return results;
                            };
                            results.push((index, work(index)));
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .flatten()
            .collect(),
    )
}

/// Work mutable items in cost order using the persistent pool; return results in item order.
/// `threads` caps the outer workers; nested work can share the bounded pool's spare capacity.
pub fn run_items_ranked<T, R, F>(items: &mut [T], costs: &[u64], threads: usize, work: F) -> Vec<R>
where
    T: Send,
    R: Send,
    F: Fn(usize, &mut T, &Tail) -> R + Sync,
{
    if items.is_empty() {
        return Vec::new();
    }
    let count = items.len();
    let workers = threads.min(count).max(1);
    let order = if costs.len() == count {
        ranked(costs)
    } else {
        (0..count).collect()
    };
    let mut by_index: Vec<Option<&mut T>> = items.iter_mut().map(Some).collect();
    let queue: VecDeque<_> = order
        .into_iter()
        .filter_map(|index| by_index.get_mut(index)?.take().map(|item| (index, item)))
        .collect();
    let queue = Mutex::new(queue.into_iter());
    let pool = work_pool();
    let tail = Tail {
        unfinished: AtomicUsize::new(count),
        pool_width: pool.map_or(1, rayon::ThreadPool::current_num_threads),
    };
    let run_worker = || {
        let mut result = Vec::new();
        loop {
            let next = queue.lock().unwrap_or_else(PoisonError::into_inner).next();
            let Some((index, item)) = next else {
                break;
            };
            result.push((index, work(index, item, &tail)));
            tail.complete();
        }
        result
    };
    let Some(pool) = pool else {
        return ordered(run_worker());
    };
    // Even a single outer item must enter this pool: its adaptive line search can then
    // borrow these workers instead of accidentally initializing Rayon's global pool.
    let parts = pool.install(|| {
        (0..workers)
            .into_par_iter()
            .map(|_| run_worker())
            .collect::<Vec<_>>()
    });
    let pairs: Vec<_> = parts.into_iter().flatten().collect();
    ordered(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn task_count_is_bounded_by_useful_work() {
        assert_eq!(pass_threads(0), 0);
        assert_eq!(pass_threads(1), 1);
        assert!(pass_threads(17) <= 17);
    }

    #[test]
    fn indexed_work_runs_once_and_returns_deterministically() {
        let count = AtomicUsize::new(0);
        for threads in [1, 2, 8, 100] {
            let got = run_indexed(257, threads, |index| {
                count.fetch_add(1, Ordering::Relaxed);
                index * 3 + 1
            });
            assert_eq!(got, (0..257).map(|i| i * 3 + 1).collect::<Vec<_>>());
        }
        assert_eq!(count.load(Ordering::Relaxed), 257 * 4);
    }

    #[test]
    fn ranked_work_returns_original_order_and_mutates_each_item_once() {
        let costs: Vec<u64> = (0..64_u64).map(|i| i * 37 % 19).collect();
        let expected: Vec<usize> = (0..64).map(|i| i * 2).collect();
        assert_eq!(run_indexed_ranked(&costs, 1, |i| i * 2), expected);
        assert_eq!(run_indexed_ranked(&costs, 6, |i| i * 2), expected);
        let mut items: Vec<_> = (0..64).collect();
        let results = run_items_ranked(&mut items, &costs, 6, |i, item, _tail| {
            *item += 100;
            i * 2
        });
        assert_eq!(results, expected);
        assert_eq!(items, (100..164).collect::<Vec<_>>());
    }

    #[test]
    fn nested_work_activates_only_after_outer_work_leaves_spare_capacity() {
        let tail = Tail {
            unfinished: AtomicUsize::new(4),
            pool_width: 3,
        };
        assert!(!tail.is_active());
        tail.complete();
        assert!(!tail.is_active());
        tail.complete();
        assert!(tail.is_active());
        let serial = Tail {
            unfinished: AtomicUsize::new(1),
            pool_width: 1,
        };
        assert!(!serial.is_active());
    }

    #[test]
    fn one_item_and_its_nested_work_use_the_persistent_pool() {
        let Some(pool) = work_pool() else {
            return;
        };
        let mut items = [0];
        let results = run_items_ranked(&mut items, &[1], 1, |_, item, tail| {
            assert!(pool.current_thread_index().is_some());
            assert_eq!(tail.is_active(), pool.current_num_threads() > 1);
            *item = (0..8)
                .into_par_iter()
                .map(|index| {
                    assert!(pool.current_thread_index().is_some());
                    index
                })
                .sum();
            *item
        });
        assert_eq!(results, [28]);
        assert_eq!(items, [28]);
    }
}
