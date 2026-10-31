use super::*;
use std::sync::mpsc;

fn ticket() -> (
    OpticalWork,
    mpsc::SyncSender<Result<ScoredOptics, OpticalError>>,
    Arc<AtomicBool>,
) {
    let (send, receive) = mpsc::sync_channel(1);
    let cancel = Arc::new(AtomicBool::new(false));
    (
        OpticalWork::new(receive, Arc::clone(&cancel), Arc::default()),
        send,
        cancel,
    )
}

#[test]
fn empty_ticket_is_pending_and_completion_is_retained() {
    let (mut work, send, _) = ticket();
    assert!(!work.poll());
    send.try_send(Err(OpticalError::InvalidInput("first".into())))
        .unwrap_or_else(|_| panic!("first reply fits"));
    assert!(work.poll());
    assert!(work.poll());
    assert!(
        matches!(work.complete(), Err(OpticalError::InvalidInput(reason)) if reason == "first")
    );
}

#[test]
fn reverse_completion_keeps_request_identity() {
    let (mut first, first_send, _) = ticket();
    let (mut second, second_send, _) = ticket();
    second_send
        .try_send(Err(OpticalError::InvalidInput("second".into())))
        .unwrap_or_else(|_| panic!("second reply fits"));
    assert!(second.poll());
    assert!(!first.poll());
    first_send
        .try_send(Err(OpticalError::InvalidInput("first".into())))
        .unwrap_or_else(|_| panic!("first reply fits"));
    for (work, expected) in [(first, "first"), (second, "second")] {
        assert!(
            matches!(work.complete(), Err(OpticalError::InvalidInput(reason)) if reason == expected)
        );
    }
}

#[test]
fn cancellation_and_drop_reach_the_owned_request() {
    let (mut work, _send, cancel) = ticket();
    work.cancel();
    assert!(cancel.load(Ordering::Relaxed));
    assert!(work.poll());
    assert!(matches!(work.complete(), Err(OpticalError::Cancelled)));
    let (work, _send, cancel) = ticket();
    drop(work);
    assert!(cancel.load(Ordering::Relaxed));
}

#[test]
fn disconnected_ticket_preserves_service_failure() {
    let (mut work, send, _) = ticket();
    *work.failure.lock().expect("failure lock") = Some("device lost".into());
    drop(send);
    assert!(work.poll());
    assert!(
        matches!(work.complete(), Err(OpticalError::Runtime(reason)) if reason == "device lost")
    );
}

#[test]
fn fake_service_admission_keeps_bounded_backpressure_and_request_ownership() {
    let (send, receive) = mpsc::sync_channel(1);
    let mut first = Some(1);
    let mut second = Some(2);
    assert!(try_admit(&send, &mut first).expect("first fits"));
    assert!(first.is_none());
    assert!(!try_admit(&send, &mut second).expect("full queue is not failure"));
    assert_eq!(second, Some(2));
    assert_eq!(receive.try_recv(), Ok(1));
    assert!(try_admit(&send, &mut second).expect("retry fits"));
    assert!(second.is_none());
    assert_eq!(receive.try_recv(), Ok(2));
    drop(receive);
    let mut disconnected = Some(3);
    assert_eq!(try_admit(&send, &mut disconnected), Err(mpsc::SendError(3)));
}
