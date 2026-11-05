//! Empty evaluation stays side-effect free.

use super::*;

#[test]
fn evaluating_no_designs_builds_nothing_and_reports_nothing() {
    let runner = fixture_runner_with(|_| {});
    let all = EvalPasses {
        sampled: true,
        oracle: true,
        noise: false,
    };
    let report = runner.evaluate(&[], EvalFidelity::Fast, 7, all).unwrap();
    assert_eq!((report.designs, report.failed), (0, 0));
    assert!(report.mean_improvement_oracle.is_none());
    assert_eq!(runner.validation_entries(0).len(), 0);
}
