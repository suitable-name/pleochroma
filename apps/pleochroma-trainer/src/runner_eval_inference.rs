//! One device owner dynamically batches policy requests from independent evaluation episodes.
//!
//! Only host states and policies cross the channel. Burn's `Module` is `Send` (not a promise
//! of `Sync`); as with the updater, all tensors are created, used and dropped by their owner.
//! Queue length, batch size and tier count are bounded. Sampling remains in each case's RNG.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, PoisonError,
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use pleochroma_ai::{
    env::{GLOBAL_DIM, GemstoneObservation, TIER_DIM},
    model::{GaussianPolicy, GemstoneSetAttentionNetwork},
    training::HostPolicyBatch,
};

use super::super::{CUDA_LABEL, DeviceChoice, GPU_LABEL, RunConfig, RunnerError, build_trainer};

const MAX_BATCH: usize = 32;
const MAX_QUEUE: usize = 64;
const GATHER_WINDOW: Duration = Duration::from_millis(1);

type PolicyResult = Result<GaussianPolicy, String>;

struct Request {
    state: Vec<f32>,
    tiers: usize,
    reply: SyncSender<PolicyResult>,
}

pub(in super::super) struct Client {
    requests: SyncSender<Request>,
    failure: Arc<Mutex<Option<String>>>,
    max_tiers: usize,
    backend: &'static str,
}

impl Client {
    pub(super) fn policy(&self, obs: &GemstoneObservation) -> Result<GaussianPolicy, RunnerError> {
        let tiers = obs.tiers.len();
        if tiers == 0 || tiers > self.max_tiers {
            return Err(self.error("observation tier count exceeds the inference budget"));
        }
        let (reply, result) = mpsc::sync_channel(1);
        self.requests
            .send(Request {
                state: obs.flatten(tiers),
                tiers,
                reply,
            })
            .map_err(|_| self.disconnected())?;
        result
            .recv()
            .map_err(|_| self.disconnected())?
            .map_err(|error| self.error(&error))
    }

    fn disconnected(&self) -> RunnerError {
        let failure = self.failure.lock().unwrap_or_else(PoisonError::into_inner);
        self.error(
            failure
                .as_deref()
                .unwrap_or("inference worker disconnected"),
        )
    }

    fn error(&self, error: &str) -> RunnerError {
        RunnerError::Data(format!("validation policy on {}: {error}", self.backend))
    }
}

pub(super) struct Service {
    client: Option<Client>,
    owner: Option<JoinHandle<()>>,
}

impl Service {
    /// Uploads the last installed twin exactly once; never reads the lagged rollout copy.
    pub(super) fn start(
        network: GemstoneSetAttentionNetwork,
        cfg: RunConfig,
        backend: &'static str,
        max_tiers: usize,
        active_cases: usize,
    ) -> Result<Self, RunnerError> {
        let device = resolved_device(backend)?;
        let (requests, receive) = mpsc::sync_channel(active_cases.clamp(1, MAX_QUEUE));
        let (ready, initialized) = mpsc::sync_channel(1);
        let failure = Arc::new(Mutex::new(None));
        let worker_failure = Arc::clone(&failure);
        let owner = thread::Builder::new()
            .name("pleochroma-validation-policy".into())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
                    // Explicit resolved choice: native CUDA must never become Auto/WGPU/CPU.
                    let (mut trainer, _, _) =
                        build_trainer(&cfg.ppo, cfg.tier, max_tiers, device, cfg.seed)
                            .map_err(|error| error.to_string())?;
                    trainer
                        .load_weights(&network)
                        .map_err(|error| error.to_string())?;
                    let model = trainer.rollout_model(); // Module::valid(): no autodiff graph.
                    drop(trainer);
                    let _ = ready.send(Ok(()));
                    serve(&receive, |states, counts| {
                        model
                            .forward_host(states, counts)
                            .map_err(|error| error.to_string())
                    })
                }));
                let error = match result {
                    Ok(Ok(())) => return,
                    Ok(Err(error)) => error,
                    Err(payload) => payload
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| {
                            payload
                                .downcast_ref::<&str>()
                                .map(|message| (*message).to_owned())
                        })
                        .map_or_else(
                            || "backend worker panicked".into(),
                            |message| format!("backend worker panicked: {message}"),
                        ),
                };
                *worker_failure
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(error.clone());
                let _ = ready.send(Err(error));
                // Dropping receive releases queued replies and blocked request senders.
            })
            .map_err(|error| {
                RunnerError::Data(format!("start validation policy worker: {error}"))
            })?;
        let service = Self {
            client: Some(Client {
                requests,
                failure,
                max_tiers,
                backend,
            }),
            owner: Some(owner),
        };
        initialized
            .recv()
            .map_err(|_| service.client().disconnected())?
            .map_err(|error| service.client().error(&error))?;
        Ok(service)
    }

    pub(super) fn client(&self) -> &Client {
        self.client
            .as_ref()
            .expect("client exists until owner shutdown")
    }
}

fn resolved_device(backend: &str) -> Result<DeviceChoice, RunnerError> {
    match backend {
        CUDA_LABEL => Ok(DeviceChoice::Cuda),
        GPU_LABEL => Ok(DeviceChoice::Gpu),
        _ => Err(RunnerError::Data(format!(
            "unsupported validation GPU backend: {backend}"
        ))),
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        // All case clients are borrowed, so no sender survives this service.
        self.client.take();
        if let Some(owner) = self.owner.take() {
            let _ = owner.join();
        }
    }
}

fn serve(
    requests: &Receiver<Request>,
    mut forward: impl FnMut(&[Vec<f32>], &[usize]) -> Result<HostPolicyBatch, String>,
) -> Result<(), String> {
    while let Ok(first) = requests.recv() {
        let mut batch = vec![first];
        let deadline = Instant::now() + GATHER_WINDOW;
        while batch.len() < MAX_BATCH {
            match requests.try_recv() {
                Ok(request) => {
                    batch.push(request);
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            match requests.recv_timeout(remaining) {
                Ok(request) => batch.push(request),
                Err(_) => break,
            }
        }
        let counts: Vec<_> = batch.iter().map(|request| request.tiers).collect();
        let padded = counts.iter().copied().max().expect("nonempty batch");
        let width = GLOBAL_DIM + padded * TIER_DIM;
        let states: Vec<_> = batch
            .iter_mut()
            .map(|request| {
                let mut state = std::mem::take(&mut request.state);
                state.resize(width, 0.0);
                state
            })
            .collect();
        let output = forward(&states, &counts).and_then(|out| {
            check_output(&out, &counts)?;
            Ok(out)
        });
        match output {
            Ok(out) => {
                for ((request, mean), log_std) in batch.into_iter().zip(out.means).zip(out.log_stds)
                {
                    let _ = request.reply.send(Ok(GaussianPolicy { mean, log_std }));
                }
            }
            Err(error) => {
                for request in batch {
                    let _ = request.reply.send(Err(error.clone()));
                }
                return Err(error);
            }
        }
    }
    Ok(())
}

fn check_output(out: &HostPolicyBatch, counts: &[usize]) -> Result<(), String> {
    if out.means.len() != counts.len()
        || out.log_stds.len() != counts.len()
        || out.values.len() != counts.len()
        || out
            .means
            .iter()
            .zip(&out.log_stds)
            .zip(counts)
            .any(|((mean, std), &n)| {
                mean.len() != n || std.len() != n || mean.iter().chain(std).any(|x| !x.is_finite())
            })
        || out.values.iter().any(|x| !x.is_finite())
    {
        return Err("backend returned malformed or non-finite policy outputs".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "runner_eval_inference_tests.rs"]
mod tests;
