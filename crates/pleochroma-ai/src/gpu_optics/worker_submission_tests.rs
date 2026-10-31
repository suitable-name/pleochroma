//! A fake owner exercises real admission without initializing CUDA or spawning threads.
use super::*;

fn prepare(worker: &Worker) -> OpticalWork {
    worker
        .prepare(
            FinishedStone::new(
                StoneGeometry {
                    planes: &[],
                    tools: &[],
                },
                &[],
            ),
            &GemMaterial::diamond(),
            ObjectiveFidelity::Fast,
            LightingPreset::IsoHemisphere,
            Some(1.0),
        )
        .expect("owned preparation")
}

#[test]
fn full_queue_retains_request_until_coordinator_retries() {
    let (sender, receive) = mpsc::sync_channel(1);
    let counters = Arc::new(Counters::default());
    let worker = Worker {
        sender: Some(sender),
        counters: Arc::clone(&counters),
        handle: None,
    };
    let mut first = prepare(&worker);
    let mut second = prepare(&worker);
    assert!(worker.try_submit(&mut first).expect("first admission"));
    assert!(
        !worker
            .try_submit(&mut second)
            .expect("bounded backpressure")
    );
    assert!(second.request.is_some());
    assert_eq!(counters.queued.load(Ordering::Relaxed), 1);
    let first_request = receive.try_recv().expect("first request");
    counters.dequeue(1);
    assert!(
        worker
            .try_submit(&mut second)
            .expect("retry after capacity")
    );
    let second_request = receive.try_recv().expect("second request");
    counters.dequeue(1);
    // Reverse service order is intentionally different from request identity.
    second_request
        .reply
        .send(Err(OpticalError::InvalidInput("second".into())))
        .unwrap_or_else(|_| panic!("second reply"));
    assert!(second.poll());
    assert!(!first.poll());
    drop(first);
    assert!(first_request.cancel.load(Ordering::Relaxed));
    assert!(
        matches!(second.complete(), Err(OpticalError::InvalidInput(reason)) if reason == "second")
    );
}

#[test]
fn disconnected_queue_and_cancelled_preparation_are_explicit_errors() {
    let (sender, receive) = mpsc::sync_channel(1);
    let worker = Worker {
        sender: Some(sender),
        counters: Arc::default(),
        handle: None,
    };
    let mut cancelled = prepare(&worker);
    cancelled.cancel();
    assert!(matches!(
        worker.try_submit(&mut cancelled),
        Err(OpticalError::Cancelled)
    ));
    let mut disconnected = prepare(&worker);
    drop(receive);
    assert!(matches!(
        worker.try_submit(&mut disconnected),
        Err(OpticalError::Runtime(_))
    ));
    assert_eq!(worker.counters.queued.load(Ordering::Relaxed), 0);
}
