use super::*;
use crate::env::{DEFAULT_LINE_SEARCH, test_fixtures::fixture_env};

#[test]
fn staged_steps_match_ordered_search_and_commit_once() {
    for parallel in [false, true] {
        let mut expected = fixture_env();
        let mut actual = fixture_env();
        let action = ContinuousTierAction::new(vec![-0.4; actual.current_design().tiers.len()]);
        for _ in 0..3 {
            let before = actual.current_design().clone();
            let mut work =
                actual.prepare_step_scaled_adaptive(&action, &DEFAULT_LINE_SEARCH, &|| parallel);
            while !actual.advance_step_adaptive(&mut work, &|| parallel) {
                assert!(
                    work.poll(),
                    "CPU fixture has no asynchronous optical request"
                );
                assert_eq!(actual.current_design(), &before);
            }
            assert_eq!(actual.current_design(), &before);
            let got = actual.complete_step(work);
            let want = expected.step_scaled_adaptive(&action, &DEFAULT_LINE_SEARCH, &|| false);
            assert_eq!(got, want);
            assert_eq!(actual.current_design(), expected.current_design());
        }
    }
}

#[test]
fn staged_teacher_retains_coordinates_rescue_and_evaluation_counts() {
    let env = fixture_env();
    let count = env.current_design().tiers.len();
    let probes: Vec<_> = (0..count).map(|tier| env.probe_tier(tier, 0.25)).collect();
    for scales in [&DEFAULT_LINE_SEARCH[..], &[][..]] {
        let expected = env.teacher_label_from_probes(0.25, &probes, scales);
        for parallel in [false, true] {
            let mut work = env.prepare_teacher_label_adaptive(0.25, &probes, scales, &|| parallel);
            while !env.advance_teacher_label_adaptive(&mut work, &|| parallel) {
                assert!(
                    work.poll(),
                    "CPU fixture has no asynchronous optical request"
                );
            }
            let actual = env.complete_teacher_label(work);
            // NaN denotes unavailable components in both paths; JSON preserves the mask as null.
            assert_eq!(
                serde_json::to_value(actual).unwrap(),
                serde_json::to_value(&expected).unwrap()
            );
        }
    }
}

#[test]
fn abandoning_prepared_work_does_not_mutate_an_episode() {
    let env = fixture_env();
    let before = env.current_design().clone();
    let action = ContinuousTierAction::new(vec![0.2; before.tiers.len()]);
    drop(env.prepare_step_scaled(&action, &DEFAULT_LINE_SEARCH));
    assert_eq!(env.current_design(), &before);
}
