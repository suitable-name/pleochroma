use super::*;

#[test]
fn selected_gpu_backend_is_explicit_and_never_auto_or_cpu() {
    assert!(matches!(
        resolved_device(CUDA_LABEL).unwrap(),
        DeviceChoice::Cuda
    ));
    assert!(matches!(
        resolved_device(GPU_LABEL).unwrap(),
        DeviceChoice::Gpu
    ));
    assert!(resolved_device(super::super::super::CPU_LABEL).is_err());
    assert!(resolved_device("unknown GPU").is_err());
}

fn request(tiers: usize, marker: f32) -> (Request, Receiver<PolicyResult>) {
    let (reply, result) = mpsc::sync_channel(1);
    let mut state = vec![marker; GLOBAL_DIM + tiers * TIER_DIM];
    state[0] = marker;
    (
        Request {
            state,
            tiers,
            reply,
        },
        result,
    )
}

fn output(states: &[Vec<f32>], counts: &[usize]) -> HostPolicyBatch {
    HostPolicyBatch {
        means: states
            .iter()
            .zip(counts)
            .map(|(row, &n)| vec![row[0]; n])
            .collect(),
        log_stds: counts.iter().map(|&n| vec![-0.5; n]).collect(),
        values: vec![0.0; counts.len()],
    }
}

#[test]
fn ready_cases_are_batched_bounded_padded_and_routed_to_their_original_callers() {
    let (send, receive) = mpsc::sync_channel(MAX_QUEUE);
    let mut replies = Vec::new();
    for i in 0..MAX_BATCH + 1 {
        let tiers = if i % 2 == 0 { 1 } else { 3 };
        let (request, reply) = request(tiers, i as f32);
        send.send(request).unwrap();
        replies.push((tiers, reply));
    }
    drop(send);
    let mut batch_sizes = Vec::new();
    serve(&receive, |states, counts| {
        batch_sizes.push(states.len());
        let padded = counts.iter().copied().max().unwrap();
        for (row, &n) in states.iter().zip(counts) {
            assert_eq!(row.len(), GLOBAL_DIM + padded * TIER_DIM);
            assert!(row[GLOBAL_DIM + n * TIER_DIM..].iter().all(|&x| x == 0.0));
        }
        Ok(output(states, counts))
    })
    .unwrap();
    assert_eq!(batch_sizes, [MAX_BATCH, 1]);
    for (i, (tiers, reply)) in replies.into_iter().enumerate() {
        let policy = reply.recv().unwrap().unwrap();
        assert_eq!(policy.mean, vec![i as f32; tiers]);
        assert_eq!(policy.log_std, vec![-0.5; tiers]);
    }
}

#[test]
fn failed_backend_notifies_the_batch_and_disconnects_queued_cases_without_fallback() {
    let (send, receive) = mpsc::sync_channel(MAX_QUEUE);
    let mut replies = Vec::new();
    for _ in 0..MAX_BATCH + 1 {
        let (request, reply) = request(1, 0.0);
        send.send(request).unwrap();
        replies.push(reply);
    }
    drop(send);
    let worker = thread::spawn(move || {
        let mut calls = 0;
        let error = serve(&receive, |_, _| {
            calls += 1;
            Err("CUDA failed".into())
        })
        .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(error, "CUDA failed");
    });
    for reply in replies.iter().take(MAX_BATCH) {
        assert_eq!(
            reply
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap_err(),
            "CUDA failed"
        );
    }
    assert!(matches!(
        replies[MAX_BATCH].recv_timeout(Duration::from_secs(2)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
    worker.join().unwrap();
}

#[test]
fn malformed_and_nonfinite_device_results_are_fatal() {
    let counts = [1, 3];
    let states = vec![vec![0.0], vec![1.0]];
    let valid = output(&states, &counts);
    assert!(check_output(&valid, &counts).is_ok());
    let mut malformed = valid.clone();
    malformed.means[0].clear();
    assert!(check_output(&malformed, &counts).is_err());
    let mut nonfinite = valid;
    nonfinite.log_stds[1][2] = f32::NAN;
    assert!(check_output(&nonfinite, &counts).is_err());
}
