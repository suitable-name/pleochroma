//! One persistent CUDA owner with a bounded cross-candidate microbatch queue.
use super::{
    OpticalDiagnostics, OpticalError, ScoredOptics, diagnostics::Counters, fan::FanGeometry,
    runtime::CudaState, submission::OpticalWork,
};
use pleochroma::{
    color::metrics::FinishedStone,
    geometry::{
        GpuFacetPlane,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::{
        materials::GemMaterial,
        raytracer::{FacetFinish, LightingPreset},
    },
};
use pleochroma_cut_core::optimize::ObjectiveFidelity;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

pub(super) struct Candidate {
    pub planes: Vec<GpuFacetPlane>,
    pub tools: Vec<ToolPrimitive>,
    pub finishes: Vec<FacetFinish>,
    pub material: GemMaterial,
    pub fidelity: ObjectiveFidelity,
    pub preset: LightingPreset,
    pub cancel: Arc<AtomicBool>,
    pub fan: FanGeometry,
    admitted: Instant,
    reply: SyncSender<Result<ScoredOptics, OpticalError>>,
}
impl Candidate {
    pub fn stone(&self) -> FinishedStone<'_> {
        FinishedStone::new(
            StoneGeometry {
                planes: &self.planes,
                tools: &self.tools,
            },
            &self.finishes,
        )
    }
}

pub(super) struct Worker {
    sender: Option<SyncSender<Candidate>>,
    counters: Arc<Counters>,
    handle: Option<std::thread::JoinHandle<()>>,
}
impl Worker {
    pub fn new(device_ordinal: usize, max_poses: usize) -> Result<Self, OpticalError> {
        let counters = Arc::new(Counters::default());
        let state = CudaState::new(device_ordinal, max_poses, Arc::clone(&counters))?;
        let queue_capacity = counters.capacity.get().map_or(16, |c| c.queue_capacity);
        let (sender, receiver) = mpsc::sync_channel(queue_capacity);
        let owner_counters = Arc::clone(&counters);
        let handle = std::thread::Builder::new()
            .name(format!("ai-optics-cuda-{device_ordinal}"))
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(state, &receiver, &owner_counters)
                }));
                if let Err(payload) = outcome {
                    let reason = payload
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                        .unwrap_or_else(|| "unknown panic".into());
                    owner_counters.fail(format!("native CUDA optical worker panicked: {reason}"));
                }
                if let Some(reason) = owner_counters.error() {
                    for request in receiver.try_iter() {
                        let _ = request
                            .reply
                            .send(Err(OpticalError::Runtime(reason.clone())));
                    }
                }
                owner_counters.active.store(0, Ordering::Relaxed);
                owner_counters.queued.store(0, Ordering::Relaxed);
            })
            .map_err(|e| OpticalError::Unavailable(format!("CUDA optical worker: {e}")))?;
        Ok(Self {
            sender: Some(sender),
            counters,
            handle: Some(handle),
        })
    }

    pub fn diagnostics(&self) -> OpticalDiagnostics {
        self.counters.snapshot()
    }

    fn disconnected(&self) -> OpticalError {
        OpticalError::Runtime(
            self.counters
                .error()
                .unwrap_or_else(|| "CUDA optical worker disconnected".into()),
        )
    }

    pub fn prepare(
        &self,
        stone: FinishedStone<'_>,
        material: &GemMaterial,
        fidelity: ObjectiveFidelity,
        preset: LightingPreset,
        half_width: Option<f32>,
    ) -> Result<OpticalWork, OpticalError> {
        if let Some(reason) = self.counters.error() {
            return Err(OpticalError::Runtime(reason));
        }
        let fan = half_width.map_or_else(
            || FanGeometry::for_planes(stone.geom.planes),
            FanGeometry::from_half_width,
        );
        let (reply, receive) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut work = OpticalWork::new(
            receive,
            Arc::clone(&cancel),
            Arc::clone(&self.counters.error),
        );
        work.request = Some(Candidate {
            planes: stone.geom.planes.to_vec(),
            tools: stone.geom.tools.to_vec(),
            finishes: stone
                .finishes
                .iter()
                .take(stone.geom.planes.len() + stone.geom.tools.len())
                .copied()
                .collect(),
            material: material.clone(),
            fidelity,
            preset,
            cancel,
            reply,
            fan,
            admitted: Instant::now(),
        });
        Ok(work)
    }

    pub fn try_submit(&self, work: &mut OpticalWork) -> Result<bool, OpticalError> {
        if let Some(reason) = self.counters.error() {
            return Err(OpticalError::Runtime(reason));
        }
        let Some(candidate) = work.request.as_ref() else {
            return Ok(true);
        };
        if candidate.cancel.load(Ordering::Relaxed) {
            return Err(OpticalError::Cancelled);
        }
        let sender = self.sender.as_ref().ok_or_else(|| self.disconnected())?;
        if let Some(candidate) = work.request.as_mut() {
            candidate.admitted = Instant::now();
        }
        self.counters.queued.fetch_add(1, Ordering::Relaxed);
        match super::submission::try_admit(sender, &mut work.request) {
            Ok(true) => Ok(true),
            Ok(false) => {
                self.counters.dequeue(1);
                Ok(false)
            }
            Err(_) => {
                self.counters.dequeue(1);
                Err(self.disconnected())
            }
        }
    }

    pub fn evaluate(
        &self,
        stone: FinishedStone<'_>,
        material: &GemMaterial,
        fidelity: ObjectiveFidelity,
        preset: LightingPreset,
        cancel: &AtomicBool,
        half_width: Option<f32>,
    ) -> Result<ScoredOptics, OpticalError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(OpticalError::Cancelled);
        }
        let mut work = self.prepare(stone, material, fidelity, preset, half_width)?;
        loop {
            if cancel.load(Ordering::Relaxed) {
                work.cancel();
                return Err(OpticalError::Cancelled);
            }
            if self.try_submit(&mut work)? && work.poll() {
                return work.complete();
            }
            // Only the explicitly synchronous compatibility API waits. Training keeps
            // the owned ticket in its coordinator and releases the CPU worker instead.
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Closing the sole sender makes the owner stop after in-flight requests. Join
        // before releasing the run so no CUDA allocations or worker survive shutdown.
        drop(self.sender.take());
        if let Some(handle) = self.handle.take() {
            if handle.join().is_err() {
                self.counters
                    .fail("native CUDA optical owner failed during shutdown".into());
            }
        }
    }
}

fn run(mut state: CudaState, receiver: &Receiver<Candidate>, counters: &Counters) {
    let fast_candidates = counters
        .capacity
        .get()
        .map_or(16, |c| c.max_fast_candidates);
    let mut pending = VecDeque::new();
    loop {
        let first = match pending.pop_front().or_else(|| receiver.recv().ok()) {
            Some(first) => first,
            None => return,
        };
        if first.cancel.load(Ordering::Relaxed) {
            counters.dequeue(1);
            counters.completed.fetch_add(1, Ordering::Relaxed);
            let _ = first.reply.send(Err(OpticalError::Cancelled));
            continue;
        }
        if matches!(first.fidelity, ObjectiveFidelity::Full) {
            if evaluate_full(&mut state, first, counters) {
                return;
            }
            continue;
        }
        let batch = collect_fast_batch(first, receiver, &mut pending, fast_candidates, counters);
        counters.dequeue(batch.len());
        counters.active.store(batch.len(), Ordering::Relaxed);
        counters.batches.fetch_add(1, Ordering::Relaxed);
        counters
            .batch_candidates
            .fetch_add(batch.len() as u64, Ordering::Relaxed);
        counters.max_batch.fetch_max(batch.len(), Ordering::Relaxed);
        let service_start = Instant::now();
        let result = state.evaluate_fast_batch(&batch);
        counters.service_micros.fetch_add(
            u64::try_from(service_start.elapsed().as_micros()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        counters
            .completed
            .fetch_add(batch.len() as u64, Ordering::Relaxed);
        counters.active.store(0, Ordering::Relaxed);
        match result {
            Ok(results) => {
                if results.len() != batch.len() {
                    let error = OpticalError::Runtime(
                        "CUDA optical batch returned a mismatched request count".into(),
                    );
                    counters.fail(error.to_string());
                    for candidate in batch {
                        let _ = candidate.reply.send(Err(error.clone()));
                    }
                    return;
                }
                for (candidate, score) in batch.into_iter().zip(results) {
                    let result = if candidate.cancel.load(Ordering::Relaxed) {
                        Err(OpticalError::Cancelled)
                    } else {
                        Ok(score)
                    };
                    let _ = candidate.reply.send(result);
                }
            }
            Err(error) => {
                counters.fail(error.to_string());
                for candidate in batch {
                    let _ = candidate.reply.send(Err(error.clone()));
                }
                return;
            }
        }
    }
}

/// Collect already queued Fast requests and account for collection and queue time.
fn collect_fast_batch(
    first: Candidate,
    receiver: &Receiver<Candidate>,
    pending: &mut VecDeque<Candidate>,
    fast_candidates: usize,
    counters: &Counters,
) -> Vec<Candidate> {
    let mut batch = vec![first];
    let collection_start = Instant::now();
    // Bound inspected requests too: a stream of cancelled work must not delay
    // the first live candidate indefinitely. Never wait to fill a sparse batch.
    for _ in 1..fast_candidates {
        let next = match receiver.try_recv() {
            Ok(next) => next,
            Err(_) => break,
        };
        if next.cancel.load(Ordering::Relaxed) {
            counters.dequeue(1);
            counters.completed.fetch_add(1, Ordering::Relaxed);
            let _ = next.reply.send(Err(OpticalError::Cancelled));
        } else if matches!(next.fidelity, ObjectiveFidelity::Fast) {
            batch.push(next);
        } else {
            pending.push_back(next);
            break;
        }
    }
    counters.collection_micros.fetch_add(
        u64::try_from(collection_start.elapsed().as_micros()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    let service_ready = Instant::now();
    for candidate in &batch {
        counters.queue_micros.fetch_add(
            u64::try_from(service_ready.duration_since(candidate.admitted).as_micros())
                .unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }
    batch
}

fn evaluate_full(state: &mut CudaState, request: Candidate, counters: &Counters) -> bool {
    counters.queue_micros.fetch_add(
        u64::try_from(request.admitted.elapsed().as_micros()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    counters.dequeue(1);
    counters.active.store(1, Ordering::Relaxed);
    let start = Instant::now();
    let result = state.evaluate(
        request.stone(),
        &request.material,
        request.fidelity,
        request.preset,
        &request.cancel,
        request.fan,
    );
    counters.service_micros.fetch_add(
        u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    counters.completed.fetch_add(1, Ordering::Relaxed);
    counters.active.store(0, Ordering::Relaxed);
    let fatal = result
        .as_ref()
        .err()
        .filter(|e| !matches!(e, OpticalError::Cancelled));
    if let Some(error) = fatal {
        counters.fail(error.to_string());
    }
    let fatal = fatal.is_some();
    let _ = request.reply.send(result);
    fatal
}

#[cfg(test)]
#[path = "worker_submission_tests.rs"]
mod submission_tests;
