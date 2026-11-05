use std::sync::{Mutex, atomic::Ordering};

use super::super::EvalCase;
use super::*;

fn case(index: usize, entry: usize, restored: bool) -> DesignResult {
    DesignResult::Failed(EvalCase {
        design_id: i64::try_from(entry).unwrap(),
        failure: Some(format!(
            "{} {index}",
            if restored { "restored" } else { "pending" },
        )),
        ..EvalCase::default()
    })
}

fn weights(index: usize) -> Option<ObjectiveWeights> {
    (!index.is_multiple_of(2)).then(|| pleochroma_cut_core::ObjectivePreset::ALL[index].weights())
}

#[test]
fn partial_recovery_preserves_case_indices_and_counts_restores_before_pending_work() {
    // Repeated entries model an objective sweep; journal indices identify individual cases.
    let entries = [31, 11, 31, 7, 11, 41];
    let restored = [true, false, true, false, false, true];
    let tasks = entries
        .iter()
        .enumerate()
        .map(|(index, &entry)| {
            (
                entry,
                weights(index),
                restored[index].then(|| case(index, entry, true)),
            )
        })
        .collect();
    let progress = TrainingProgress::default();
    progress.phase("CPU evaluation", entries.len());
    let called = Mutex::new(Vec::new());
    let results = run_pending_cases(
        tasks,
        &progress,
        |entry| entry as u64,
        |index, entry, objective, current| {
            // Serialize these quick assertions so the first callback observes the pool before
            // any other pending callback can complete, independently of its CPU worker count.
            let mut called = called.lock().unwrap();
            if called.is_empty() {
                assert_eq!(progress.snapshot().completed_tasks, 3);
                assert_eq!(current.tail.unfinished.load(Ordering::Relaxed), 3);
            }
            assert!(
                !restored[index],
                "restored cases must not be evaluated again"
            );
            assert_eq!(entry, entries[index]);
            assert_eq!(current.case, index + 1);
            assert_eq!(current.entry, entry);
            assert_eq!(format!("{objective:?}"), format!("{:?}", weights(index)));
            called.push(index);
            Ok(case(index, entry, false))
        },
    );
    let mut called = called.into_inner().unwrap();
    called.sort_unstable();
    assert_eq!(called, [1, 3, 4]);
    assert_eq!(progress.snapshot().completed_tasks, entries.len());
    let report = reduce(EvalFidelity::Fast, EvalPasses::default(), results).unwrap();
    assert_eq!(report.failed, entries.len());
    assert_eq!(report.first_failure.as_deref(), Some("restored 0"));
    for (index, actual) in report.cases.iter().enumerate() {
        let DesignResult::Failed(expected) = case(index, entries[index], restored[index]) else {
            unreachable!();
        };
        assert_eq!(actual.design_id, expected.design_id);
        assert_eq!(actual.failure, expected.failure);
    }
}

#[test]
fn one_pending_case_can_share_spare_workers_despite_many_restored_cases() {
    let total = pool::pool_threads() + 3;
    let pending_index = total / 2;
    let tasks = (0..total)
        .map(|index| {
            (
                index,
                None,
                (index != pending_index).then(|| case(index, index, true)),
            )
        })
        .collect();
    let progress = TrainingProgress::default();
    progress.phase("CPU evaluation", total);
    let results = run_pending_cases(
        tasks,
        &progress,
        |entry| {
            assert_eq!(entry, pending_index);
            1
        },
        |index, entry, _, current| {
            assert_eq!(index, pending_index);
            assert_eq!(progress.snapshot().completed_tasks, total - 1);
            assert_eq!(current.tail.unfinished.load(Ordering::Relaxed), 1);
            assert_eq!(current.tail.is_active(), pool::pool_threads() > 1);
            Ok(case(index, entry, false))
        },
    );
    assert_eq!(results.len(), total);
    assert_eq!(progress.snapshot().completed_tasks, total);
}

#[test]
fn fully_restored_cases_need_no_cost_estimates_or_evaluation() {
    let tasks = (0..3)
        .map(|index| (index, None, Some(case(index, index, true))))
        .collect();
    let progress = TrainingProgress::default();
    progress.phase("CPU evaluation", 3);
    let results = run_pending_cases(
        tasks,
        &progress,
        |_| panic!("completed cases do not need scheduling"),
        |_, _, _, _| panic!("completed cases do not need evaluation"),
    );
    assert_eq!(results.len(), 3);
    assert_eq!(progress.snapshot().completed_tasks, 3);
}
