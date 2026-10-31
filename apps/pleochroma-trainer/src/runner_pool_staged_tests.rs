//! Synthetic dependency tests: no simulator, CUDA device or training backend is needed.

use super::*;
use rayon::prelude::*;
use std::{
    sync::atomic::{AtomicBool, AtomicUsize},
    time::Instant,
};

#[derive(Debug, PartialEq, Eq)]
struct Output {
    item: usize,
    probes: Vec<(usize, Result<usize, ()>)>,
}

#[derive(Default)]
struct Counters {
    cached_finished: AtomicBool,
    cancelled: AtomicBool,
    live: AtomicUsize,
    peak: AtomicUsize,
    prepared: AtomicUsize,
    completed: AtomicUsize,
    dropped: AtomicUsize,
    nested: AtomicUsize,
}

struct Ticket<'a> {
    counters: &'a Counters,
    value: usize,
    polls: usize,
    gated: bool,
    deadline: Instant,
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        self.counters.live.fetch_sub(1, Ordering::Relaxed);
        self.counters.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

struct Finalizer {
    probes: Vec<(usize, Result<usize, ()>)>,
    advances: usize,
}

struct Synthetic<'a> {
    counters: &'a Counters,
    gated_item: Option<usize>,
    cancel_on_cached: bool,
    panic_probe: bool,
}

impl<'a> ItemStages<usize> for Synthetic<'a> {
    type ProbeWork = Ticket<'a>;
    type Probe = Result<usize, ()>;
    type FinishWork = Finalizer;
    type Output = Output;

    fn prepare_probe(&self, item: usize, value: &usize, probe: usize, _tail: &Tail) -> Ticket<'a> {
        assert_eq!(*value, item, "probes must see the unchanged item");
        assert!(!self.panic_probe, "synthetic preparation panic");
        let live = self.counters.live.fetch_add(1, Ordering::Relaxed) + 1;
        self.counters.peak.fetch_max(live, Ordering::Relaxed);
        self.counters.prepared.fetch_add(1, Ordering::Relaxed);
        Ticket {
            counters: self.counters,
            value: item * 100 + probe,
            polls: (item + probe) % 4,
            gated: self.gated_item == Some(item),
            deadline: Instant::now() + Duration::from_secs(5),
        }
    }

    fn poll_probe(&self, work: &mut Ticket<'a>) -> bool {
        if work.gated && !self.counters.cached_finished.load(Ordering::Relaxed) {
            // A broken global barrier fails after a bounded wait rather than hanging the suite.
            return Instant::now() >= work.deadline;
        }
        if work.polls > 0 {
            work.polls -= 1;
            return false;
        }
        true
    }

    fn complete_probe(
        &self,
        _item: usize,
        _value: &usize,
        _probe: usize,
        work: Ticket<'a>,
    ) -> Self::Probe {
        assert!(!work.gated || self.counters.cached_finished.load(Ordering::Relaxed));
        self.counters.completed.fetch_add(1, Ordering::Relaxed);
        if self.counters.cancelled.load(Ordering::Relaxed) {
            Err(())
        } else {
            Ok(work.value)
        }
    }

    fn begin_finish(
        &self,
        item: usize,
        value: &mut usize,
        probes: Vec<(usize, Self::Probe)>,
        _tail: &Tail,
    ) -> Finalizer {
        assert_eq!(*value, item);
        Finalizer {
            probes,
            advances: 2,
        }
    }

    fn poll_finish(&self, _work: &mut Finalizer) -> bool {
        true
    }

    fn advance_finish(
        &self,
        item: usize,
        value: &mut usize,
        work: &mut Finalizer,
        tail: &Tail,
    ) -> Option<Output> {
        if work.advances > 0 {
            work.advances -= 1;
            return None;
        }
        assert_eq!(*value, item, "finish must commit once");
        if tail.is_active() {
            let sum: usize = (0..16).into_par_iter().sum();
            assert_eq!(sum, 120);
            self.counters.nested.fetch_add(1, Ordering::Relaxed);
        }
        *value += 1000;
        if work.probes.is_empty() {
            self.counters
                .cancelled
                .store(self.cancel_on_cached, Ordering::Relaxed);
            self.counters.cached_finished.store(true, Ordering::Relaxed);
        }
        Some(Output {
            item,
            probes: std::mem::take(&mut work.probes),
        })
    }
}

fn pool(width: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(width)
        .build()
        .unwrap()
}

#[test]
fn cached_slot_finishes_while_an_unrelated_teacher_probe_is_pending() {
    for cancel_on_cached in [false, true] {
        let counters = Counters::default();
        let stages = Synthetic {
            counters: &counters,
            gated_item: Some(0),
            cancel_on_cached,
            panic_probe: false,
        };
        let plans = [
            ProbePlan {
                probes: vec![4, 1],
                probe_cost: 100,
                finish_cost: 100,
            },
            ProbePlan {
                probes: vec![],
                probe_cost: 1,
                finish_cost: 1,
            },
        ];
        let mut items = [0, 1];
        let got = run_with_pool(
            &mut items,
            &plans,
            StagedLimits {
                threads: 2,
                pending: 3,
            },
            stages,
            Some(&pool(2)),
        );
        assert_eq!(got.iter().map(|row| row.item).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(
            got[0].probes.iter().map(|row| row.0).collect::<Vec<_>>(),
            [4, 1]
        );
        assert!(
            got[0]
                .probes
                .iter()
                .all(|row| row.1.is_err() == cancel_on_cached)
        );
        assert_eq!(items, [1000, 1001]);
        assert_eq!(counters.completed.load(Ordering::Relaxed), 2);
        assert_eq!(counters.live.load(Ordering::Relaxed), 0);
        assert_eq!(counters.dropped.load(Ordering::Relaxed), 2);
    }
}

#[test]
fn bounded_staged_work_keeps_every_probe_and_canonical_order_across_pool_widths() {
    let plans: Vec<_> = (0..12)
        .map(|item| ProbePlan {
            probes: vec![7, 2, 5],
            probe_cost: (12 - item) as u64,
            finish_cost: (item + 1) as u64,
        })
        .collect();
    let expected: Vec<_> = (0..12)
        .map(|item| Output {
            item,
            probes: [7, 2, 5]
                .map(|probe| (probe, Ok(item * 100 + probe)))
                .to_vec(),
        })
        .collect();
    for (width, pending) in [(1, 1), (1, 4), (2, 4), (5, 4)] {
        let counters = Counters::default();
        let stages = Synthetic {
            counters: &counters,
            gated_item: None,
            cancel_on_cached: false,
            panic_probe: false,
        };
        let mut items: Vec<_> = (0..12).collect();
        let got = run_with_pool(
            &mut items,
            &plans,
            StagedLimits {
                threads: width,
                pending,
            },
            stages,
            Some(&pool(width)),
        );
        assert_eq!(got, expected);
        assert_eq!(items, (1000..1012).collect::<Vec<_>>());
        assert!(counters.peak.load(Ordering::Relaxed) <= pending);
        assert_eq!(counters.prepared.load(Ordering::Relaxed), 36);
        assert_eq!(counters.completed.load(Ordering::Relaxed), 36);
        assert_eq!(counters.dropped.load(Ordering::Relaxed), 36);
    }
}

#[test]
fn an_empty_probe_plan_preserves_nested_work_and_serial_fallback() {
    for width in [0, 4] {
        let counters = Counters::default();
        let stages = Synthetic {
            counters: &counters,
            gated_item: None,
            cancel_on_cached: false,
            panic_probe: false,
        };
        let plans = [ProbePlan {
            probes: vec![],
            probe_cost: 1,
            finish_cost: 1,
        }];
        let pool = (width > 0).then(|| pool(width));
        let got = run_with_pool(
            &mut [0],
            &plans,
            StagedLimits {
                threads: width,
                pending: 1,
            },
            stages,
            pool.as_ref(),
        );
        assert_eq!(
            got,
            [Output {
                item: 0,
                probes: Vec::new()
            }]
        );
        assert_eq!(
            counters.nested.load(Ordering::Relaxed),
            usize::from(width > 1)
        );
    }
}

#[test]
fn a_worker_panic_reaches_the_caller_instead_of_stranding_dependencies() {
    let counters = Counters::default();
    let stages = Synthetic {
        counters: &counters,
        gated_item: None,
        cancel_on_cached: false,
        panic_probe: true,
    };
    let plans = [ProbePlan {
        probes: vec![0],
        probe_cost: 1,
        finish_cost: 1,
    }];
    let result = catch_unwind(AssertUnwindSafe(|| {
        run_with_pool(
            &mut [0],
            &plans,
            StagedLimits {
                threads: 1,
                pending: 1,
            },
            stages,
            Some(&pool(1)),
        )
    }));
    assert!(result.is_err());
    assert_eq!(counters.live.load(Ordering::Relaxed), 0);
}
