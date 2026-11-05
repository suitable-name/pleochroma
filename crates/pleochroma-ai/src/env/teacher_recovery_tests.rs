//! Durable teacher boundaries preserve the ordinary ordered search, including refusals.

use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use super::*;
use crate::env::{DEFAULT_LINE_SEARCH, test_fixtures::fixture_env};

fn assert_same_label(actual: &TeacherLabel, expected: &TeacherLabel) {
    assert_eq!(actual.evaluations, expected.evaluations);
    assert_eq!(actual.action_target, expected.action_target);
    assert_eq!(
        actual.oracle_gain.map(f32::to_bits),
        expected.oracle_gain.map(f32::to_bits)
    );
    assert_eq!(
        actual.oracle_scale.map(f32::to_bits),
        expected.oracle_scale.map(f32::to_bits)
    );
    assert_eq!(actual.tiers.len(), expected.tiers.len());
    for (actual, expected) in actual.tiers.iter().zip(&expected.tiers) {
        match (actual, expected) {
            (Some(actual), Some(expected)) => {
                assert_eq!(actual.forward_probe, expected.forward_probe);
                assert_eq!(actual.score.to_bits(), expected.score.to_bits());
                assert_eq!(
                    actual.components.map(f32::to_bits),
                    expected.components.map(f32::to_bits)
                );
            }
            (None, None) => {}
            _ => panic!("resumed sensitivity presence differs"),
        }
    }
}

fn interrupt_every_boundary(
    env: &RetargetEnv,
    h_deg: f64,
    initial: Option<TeacherSearchSnapshot>,
) -> (TeacherLabel, TeacherSearchSnapshot) {
    let saved = Mutex::new(initial);
    let mut completed = saved
        .lock()
        .unwrap()
        .as_ref()
        .map_or(0, TeacherSearchSnapshot::completed_units);
    for _ in 0..64 {
        let input = saved.lock().unwrap().clone();
        let result = env.teacher_label_with_recovery_adaptive(
            h_deg,
            &DEFAULT_LINE_SEARCH,
            input,
            &|| false,
            &|snapshot| {
                *saved.lock().unwrap() = Some(snapshot.clone());
                Err("simulated exit after durable write".to_string())
            },
        );
        let snapshot = saved
            .lock()
            .unwrap()
            .clone()
            .expect("a checkpoint was published");
        let next = snapshot.completed_units();
        assert!(
            next >= completed && next <= completed + 1,
            "completed work was repeated or lost"
        );
        completed = next;
        match result {
            Ok(label) => {
                assert_eq!(snapshot.phase, Phase::Complete);
                return (label, snapshot);
            }
            Err(error) => assert_eq!(error, "simulated exit after durable write"),
        }
    }
    panic!("search did not finish despite preserving every completed boundary");
}

#[test]
fn interruption_at_every_completed_unit_preserves_the_reference_label() {
    let env = fixture_env();
    let h = env.config().probe_scale_deg;
    let reference = env.teacher_label(h, &DEFAULT_LINE_SEARCH);
    let (resumed, saved) = interrupt_every_boundary(&env, h, None);
    assert_same_label(&resumed, &reference);
    assert!(
        saved
            .free_tiers
            .iter()
            .all(|&tier| saved.probes[tier].is_some())
    );
    let restored = RetargetEnv::from_episode(
        env.base_design().clone(),
        env.config().clone(),
        &env.snapshot_episode().unwrap(),
    )
    .unwrap();
    let completed = restored
        .teacher_label_with_recovery_adaptive(
            h,
            &DEFAULT_LINE_SEARCH,
            Some(saved),
            &|| panic!("complete recovery must not schedule expensive work"),
            &|_| panic!("complete recovery must not rewrite the checkpoint"),
        )
        .unwrap();
    assert_same_label(&completed, &reference);
}

#[test]
fn a_completed_empty_probe_is_not_mistaken_for_unstarted_work() {
    let env = fixture_env();
    // Invalid h is a supported empty-label request and makes every free probe return default.
    let (label, saved) = interrupt_every_boundary(&env, 0.0, None);
    assert_same_label(&label, &env.teacher_label(0.0, &DEFAULT_LINE_SEARCH));
    assert_eq!(saved.completed_units(), env.movable_tier_indices().len());
    for &tier in env.movable_tier_indices() {
        assert_eq!(saved.probes[tier], Some(TierProbe::default()));
    }
    // This empty snapshot contains no NaNs, so JSON can also exercise its serde schema.
    // Real optical labels use the trainer's MessagePack journal to preserve NaNs.
    let encoded = serde_json::to_vec(&saved).unwrap();
    let decoded: TeacherSearchSnapshot = serde_json::from_slice(&encoded).unwrap();
    env.core
        .validate_teacher_snapshot(&decoded, 0.0, &DEFAULT_LINE_SEARCH)
        .unwrap();
    assert_eq!(decoded.completed_units(), saved.completed_units());
}

#[test]
fn rescue_continuation_preserves_refused_candidates_and_evaluation_counts() {
    let env = fixture_env();
    let h = env.config().probe_scale_deg;
    // This is the supported all-refused-probes boundary. It must still try the ordinary
    // bounded rescue, persist refusals as completed candidates, and preserve tie order.
    let mut seed = TeacherSearchSnapshot::new(&env.core, h, &DEFAULT_LINE_SEARCH);
    for &tier in &seed.free_tiers {
        seed.probes[tier] = Some(TierProbe::default());
    }
    let reference = env.teacher_label_from_probes(h, &seed.probes(), &DEFAULT_LINE_SEARCH);
    let (resumed, saved) = interrupt_every_boundary(&env, h, Some(seed));
    assert!(!saved.rescue.is_empty());
    assert!(saved.rescue.iter().all(Option::is_some));
    assert_same_label(&resumed, &reference);
}

#[test]
fn parallel_completion_order_does_not_change_the_winner_or_counts() {
    let env = fixture_env();
    let h = env.config().probe_scale_deg;
    let reference = env.teacher_label(h, &DEFAULT_LINE_SEARCH);
    let progress = Mutex::new(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(3)
        .build()
        .unwrap();
    let actual = pool
        .install(|| {
            env.teacher_label_with_recovery_adaptive(
                h,
                &DEFAULT_LINE_SEARCH,
                None,
                &|| true,
                &|saved| {
                    let mut prior = progress.lock().unwrap();
                    assert!(
                        saved.completed_units() >= *prior,
                        "older checkpoint overwrote newer work"
                    );
                    *prior = saved.completed_units();
                    Ok(())
                },
            )
        })
        .unwrap();
    assert_same_label(&actual, &reference);
}

#[test]
fn slow_parallel_checkpoints_never_overlap_and_end_with_all_completed_work() {
    let env = fixture_env();
    let h = env.config().probe_scale_deg;
    let reference = env.teacher_label(h, &DEFAULT_LINE_SEARCH);
    let writing = AtomicBool::new(false);
    let last = Mutex::new(None::<TeacherSearchSnapshot>);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(3)
        .build()
        .unwrap();
    let actual = pool
        .install(|| {
            env.teacher_label_with_recovery_adaptive(
                h,
                &DEFAULT_LINE_SEARCH,
                None,
                &|| true,
                &|saved| {
                    assert!(
                        !writing.swap(true, Ordering::SeqCst),
                        "checkpoints overlapped"
                    );
                    // A slow durable write: units keep completing outside the search lock meanwhile.
                    thread::sleep(Duration::from_millis(5));
                    let mut prior = last.lock().unwrap();
                    let done = prior
                        .as_ref()
                        .map_or(0, TeacherSearchSnapshot::completed_units);
                    assert!(
                        saved.completed_units() >= done,
                        "older checkpoint overwrote newer work"
                    );
                    *prior = Some(saved.clone());
                    drop(prior);
                    writing.store(false, Ordering::SeqCst);
                    Ok(())
                },
            )
        })
        .unwrap();
    assert_same_label(&actual, &reference);
    let last = last
        .into_inner()
        .unwrap()
        .expect("a checkpoint was published");
    assert_eq!(last.phase, Phase::Complete);
    let resumed = env
        .teacher_label_with_recovery_adaptive(
            h,
            &DEFAULT_LINE_SEARCH,
            Some(last),
            &|| panic!("the last checkpoint holds every completed unit"),
            &|_| panic!("a complete search must not rewrite the checkpoint"),
        )
        .unwrap();
    assert_same_label(&resumed, &reference);
}

#[test]
fn changed_context_settings_and_malformed_progress_are_refused_before_work() {
    let mut env = fixture_env();
    let h = env.config().probe_scale_deg;
    let seed = TeacherSearchSnapshot::new(&env.core, h, &DEFAULT_LINE_SEARCH);
    let mut invalid_states = Vec::new();
    let mut changed = seed.clone();
    changed.version += 1;
    invalid_states.push(changed);
    let mut changed = seed.clone();
    changed.identity.push_str("changed backend or context");
    invalid_states.push(changed);
    let mut changed = seed.clone();
    changed.h_bits = (h * 0.5).to_bits();
    invalid_states.push(changed);
    let mut changed = seed.clone();
    changed.scale_bits.reverse();
    invalid_states.push(changed);
    let mut changed = seed.clone();
    changed.probes.pop();
    invalid_states.push(changed);
    let mut changed = seed.clone();
    changed.phase = Phase::Complete;
    invalid_states.push(changed);
    let mut changed = seed.clone();
    changed.probes[seed.free_tiers[0]] = Some(TierProbe {
        evaluations: 3,
        ..TierProbe::default()
    });
    invalid_states.push(changed);
    for saved in invalid_states {
        assert!(
            env.teacher_label_with_recovery_adaptive(
                h,
                &DEFAULT_LINE_SEARCH,
                Some(saved),
                &|| panic!("invalid state must not schedule work"),
                &|_| panic!("invalid state must not be persisted"),
            )
            .is_err()
        );
    }
    env.core.step_count += 1;
    assert!(
        env.teacher_label_with_recovery_adaptive(
            h,
            &DEFAULT_LINE_SEARCH,
            Some(seed),
            &|| panic!("changed episode must not schedule work"),
            &|_| panic!("changed episode must not be persisted"),
        )
        .is_err()
    );
}

#[test]
fn candidate_reduction_keeps_canonical_first_winner_for_equal_gains() {
    let env = fixture_env();
    let mut label = env.teacher_label(0.0, &[]);
    let first = Winner {
        raw: vec![0.5, 0.0],
        gain: 1.0,
        scale: 1.0,
    };
    let second = Winner {
        raw: vec![0.0, 0.5],
        gain: 1.0,
        scale: 0.5,
    };
    let mut best = None;
    reduce_candidates(
        &[
            Some(CandidateResult {
                winner: Some(first.clone()),
                evaluations: 1,
            }),
            Some(CandidateResult {
                winner: None,
                evaluations: 0,
            }),
            Some(CandidateResult {
                winner: Some(second),
                evaluations: 1,
            }),
        ],
        &mut label,
        &mut best,
    );
    assert_eq!(best, Some(first));
    assert_eq!(label.evaluations, 2);
}
