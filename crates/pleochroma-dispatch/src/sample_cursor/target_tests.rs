use super::SampleCursor;

#[test]
fn raising_a_finished_target_continues_at_the_next_unused_index() {
    let cursor = SampleCursor::new(0, 128);
    assert_eq!(cursor.claim(256), Some((0, 128)));
    assert_eq!(cursor.claim(1), None);
    cursor.set_end(1024);
    assert_eq!(cursor.claim(16), Some((128, 16)));
}

#[test]
fn lowering_and_raising_a_target_never_reissues_in_flight_samples() {
    let cursor = SampleCursor::new(0, 128);
    assert_eq!(cursor.claim(96), Some((0, 96)));
    cursor.set_end(64);
    assert_eq!(cursor.claim(1), None);
    cursor.set_end(128);
    assert_eq!(cursor.claim(64), Some((96, 32)));
}

#[test]
fn a_reduced_target_defers_retry_tails_without_losing_them() {
    let cursor = SampleCursor::new(0, 128);
    assert_eq!(cursor.claim(128), Some((0, 128)));
    cursor.return_to_local(32, 96);
    cursor.set_end(64);
    assert_eq!(cursor.claim_local_bounded(128), Some((32, 32)));
    assert_eq!(cursor.claim_local_bounded(1), None);
    assert!(cursor.fully_claimed());
    cursor.set_end(128);
    assert!(!cursor.fully_claimed());
    assert_eq!(cursor.claim_local_bounded(128), Some((64, 64)));
}

#[test]
fn whole_local_retries_respect_a_paused_target_and_resume_exactly_once() {
    let cursor = SampleCursor::new(0, 128);
    assert_eq!(cursor.claim(128), Some((0, 128)));
    cursor.return_to_local(32, 96);
    cursor.set_end(0);
    assert_eq!(cursor.claim_local(1), None);
    assert!(cursor.fully_claimed());

    cursor.set_end(64);
    // The unbounded API returns the whole eligible part, even when want is 1.
    assert_eq!(cursor.claim_local(1), Some((32, 32)));
    assert_eq!(cursor.claim_local(1), None);
    assert!(cursor.fully_claimed());

    cursor.set_end(128);
    assert!(!cursor.fully_claimed());
    assert_eq!(cursor.claim_local(1), Some((64, 64)));
    assert_eq!(cursor.claim_local(1), None);
    assert_eq!(cursor.claim(1), None);
    assert!(cursor.fully_claimed());
}

#[test]
fn out_of_order_retries_below_the_current_target_are_still_claimable() {
    let cursor = SampleCursor::new(0, 128);
    assert_eq!(cursor.claim(128), Some((0, 128)));
    cursor.requeue(96, 32);
    cursor.requeue(16, 16);
    cursor.set_end(64);
    assert_eq!(cursor.claim_any(64), Some((16, 16)));
    assert_eq!(cursor.claim_any(64), None);
    cursor.set_end(128);
    assert_eq!(cursor.claim_any(64), Some((96, 32)));
}
