//! Bounded imitation rollout streams. Policy inference stays on the caller; each slot has one
//! sequential stream, while its staged optical work shares the trainer's bounded CPU pool.

use std::{
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Instant,
};

use pleochroma_ai::{
    env::EnvExecution,
    model::{GaussianPolicy, Prng},
};

use super::{
    DatasetSplit, EpisodeSummary, RolloutStats, RunConfig, RunnerError, Slot, TrainingRunner,
    build_slot,
    imitation::LabelledSample,
    pool,
    rollout::{MAX_PENDING_WORK, PendingStep, SlotOutcome, step_slot},
    teacher::labels_pass,
};

const ACTION_STREAM: u64 = 0x4C55_4E41_5F41_4354;
const RESPAWN_STREAM: u64 = 0x4C55_4E41_5F52_5350;

struct Work {
    slot_index: usize,
    local_step: usize,
    slot: Slot,
    pending: PendingStep,
}

struct BuildWork {
    slot_index: usize,
    local_step: usize,
    start: usize,
    seed: u64,
    fallback: Slot,
}

enum WorkItem {
    Step(Work),
    Build(BuildWork),
}

impl WorkItem {
    fn slot_index(&self) -> usize {
        match self {
            Self::Step(work) => work.slot_index,
            Self::Build(work) => work.slot_index,
        }
    }
}

struct Done {
    slot_index: usize,
    local_step: usize,
    slot: Slot,
    pending: PendingStep,
    outcome: SlotOutcome,
    probe_interval: Option<(f32, f32)>,
}

enum DoneItem {
    LostSlot {
        slot_index: usize,
        message: String,
    },
    Step(Result<Done, (usize, Slot, String)>),
    Build {
        slot_index: usize,
        local_step: usize,
        fallback: Slot,
        result: Result<(Slot, Vec<String>), String>,
    },
}

fn catch_job(slot_index: usize, job: impl FnOnce() -> DoneItem) -> DoneItem {
    catch_unwind(AssertUnwindSafe(job)).unwrap_or_else(|payload| {
        let detail = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                payload
                    .downcast_ref::<&'static str>()
                    .map(|s| (*s).to_owned())
            })
            .unwrap_or_else(|| "unknown panic".into());
        DoneItem::LostSlot {
            slot_index,
            message: format!("rollout worker lost slot {slot_index} after panic: {detail}"),
        }
    })
}

fn restore_owned_slots(
    slots: Vec<Option<Slot>>,
    fatal: Option<&str>,
) -> Result<Vec<Slot>, RunnerError> {
    slots
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            let cause = fatal.unwrap_or("rollout workers did not return every slot");
            RunnerError::Data(format!(
                "{cause}; slot ownership was lost; restore the last committed recovery"
            ))
        })
}

struct Record {
    slot_index: usize,
    local_step: usize,
    pending: PendingStep,
    outcome: SlotOutcome,
    ended: bool,
    stalled: bool,
    probe_interval: Option<(f32, f32)>,
    summary: Option<EpisodeSummary>,
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn stream_rng(seed: u64, domain: u64, iteration: usize, slot: usize, local_step: usize) -> Prng {
    let stream = splitmix64(
        domain
            ^ splitmix64(iteration as u64)
            ^ splitmix64((slot as u64).rotate_left(21))
            ^ splitmix64((local_step as u64).rotate_left(42)),
    );
    Prng::from_stream(seed, stream)
}

fn fixed_quotas(rollout_size: usize, slot_count: usize) -> Vec<usize> {
    (0..slot_count)
        .map(|w| rollout_size / slot_count + usize::from(w < rollout_size % slot_count))
        .collect()
}

fn remaining_quota(quotas: &[usize], completed: &[usize]) -> (usize, usize) {
    quotas
        .iter()
        .zip(completed)
        .fold((0, 0), |(slots, steps), (&quota, &done)| {
            let remaining = quota.saturating_sub(done);
            (slots + usize::from(remaining > 0), steps + remaining)
        })
}

fn coordinator_count(slot_count: usize, rollout_size: usize, cpu_width: usize) -> usize {
    if cpu_width <= 1 {
        1
    } else {
        slot_count
            .min(rollout_size)
            .min(cpu_width.saturating_mul(3))
            .min(MAX_PENDING_WORK)
            .max(1)
    }
}

/// Whether step `local_step` of `slot` is labelled. `label_phase` counts the passes of earlier
/// rollouts (it advances by the longest quota), so `label_phase + local_step` is the global pass
/// of the step and [`labels_pass`] spreads the labels evenly over the slots and over each slot's
/// steps whatever the slot count; the slot count must not enter, or with 27 slots and period 2
/// the same slots would label every step of every rollout.
const fn stream_labelled(every: usize, label_phase: usize, slot: usize, local_step: usize) -> bool {
    labels_pass(every, label_phase.wrapping_add(local_step), slot)
}

fn canonical_slot_order<T>(items: &mut [T], key: impl FnMut(&T) -> (usize, usize)) {
    items.sort_by_key(key);
}

fn take_ready_batch(ready: &mut VecDeque<usize>, capacity: usize) -> Vec<usize> {
    let mut batch: Vec<_> = (0..capacity).filter_map(|_| ready.pop_front()).collect();
    batch.sort_unstable();
    batch
}

fn respawn_seed(rollout_seed: u64, iteration: usize, w: usize, local_step: usize) -> u64 {
    stream_rng(rollout_seed, RESPAWN_STREAM, iteration, w, local_step).next_u64()
}

fn summary(slot: &Slot, optics: super::Optics) -> EpisodeSummary {
    EpisodeSummary {
        entry: slot.entry,
        base_score: slot.env.base_score(),
        final_score: slot.env.current_score(),
        optics,
    }
}

fn build_stream_slot(
    cfg: &RunConfig,
    split: &DatasetSplit,
    execution: Option<&Arc<EnvExecution>>,
    focus_entry: usize,
    mut start: usize,
    seed: u64,
) -> Result<(Slot, Vec<String>), String> {
    let mut warnings = Vec::new();
    let mut last_error = None;
    for _ in 0..split.train.len() {
        let (pos, entry) = if cfg.corpus_mode {
            let pos = start % split.train.len();
            (pos, split.train[pos])
        } else {
            (0, focus_entry)
        };
        let cost = split.entries[entry].design.tiers.len() as u64;
        let built = pool::run_indexed_ranked(&[cost], 1, |_| {
            let _active = pool::SharedActiveGuard::enter(true);
            build_slot(cfg, split, pos, entry, seed, execution)
        })
        .pop()
        .expect("one build result");
        match built.map_err(|error| error.to_string())? {
            Ok(slot) => return Ok((slot, warnings)),
            Err(error) if cfg.corpus_mode => {
                let title = &split.entries[entry].title;
                last_error = Some(error.to_string());
                warnings.push(format!("{}'{title}': {error}", super::SKIPPED_PREFIX));
                start = pos + 1;
            }
            Err(error) => {
                return Err(format!(
                    "the selected design '{}' cannot be trained: {error}",
                    split.entries[entry].title
                ));
            }
        }
    }
    Err(format!(
        "no sampled training design builds an environment for this target/objective: {}",
        last_error.unwrap_or_else(|| "empty train split".into())
    ))
}

fn merge_probe_intervals(
    raw_intervals: impl IntoIterator<Item = (f32, f32)>,
    max_elapsed: f32,
) -> f32 {
    let mut intervals: Vec<(f32, f32)> = raw_intervals
        .into_iter()
        .filter_map(|(start, end)| {
            let lo = start.clamp(0.0, max_elapsed);
            let hi = end.clamp(lo, max_elapsed);
            (hi > lo).then_some((lo, hi))
        })
        .collect();
    if intervals.is_empty() {
        return 0.0;
    }
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
    let mut total = 0.0f32;
    let (mut cur_lo, mut cur_hi) = intervals[0];
    for &(lo, hi) in &intervals[1..] {
        if lo <= cur_hi {
            cur_hi = cur_hi.max(hi);
        } else {
            total += cur_hi - cur_lo;
            cur_lo = lo;
            cur_hi = hi;
        }
    }
    (total + (cur_hi - cur_lo)).min(max_elapsed)
}

fn merged_probe_seconds(records: &[Record], max_elapsed: f32) -> f32 {
    merge_probe_intervals(
        records.iter().filter_map(|record| record.probe_interval),
        max_elapsed,
    )
}

fn commit_records(
    runner: &mut TrainingRunner,
    records: &mut [Record],
    rollout: &mut RolloutStats,
    rollout_elapsed: f32,
    cancelled: bool,
) {
    canonical_slot_order(records, |record| (record.slot_index, record.local_step));
    rollout.label_secs = merged_probe_seconds(records, rollout_elapsed);
    for record in records {
        let SlotOutcome {
            step,
            label,
            fresh_label,
        } = &record.outcome;
        rollout.evaluations += usize::from(step.info.evaluations);
        rollout.diagnostics.unchanged_proposals += usize::from(step.info.unchanged);
        rollout.diagnostics.labels_reused += usize::from(label.is_some() && !fresh_label);
        let label = label.clone().filter(|label| rollout.record_label(label));
        if let Some(label) = label
            && *fresh_label
        {
            runner.aggregate.push(LabelledSample::new(
                record.pending.state.clone(),
                record.pending.free.clone(),
                &label,
            ));
            rollout.diagnostics.labels_added += 1;
        }
        rollout.steps += 1;
        rollout.reward_sum += step.reward;
        rollout.count_rejection(step.info.rejection);
        if record.slot_index == 0 {
            if !record.ended {
                let optics = super::Optics::from_global(&step.observation.global);
                runner.stats.current = Some(optics);
            }
            runner.stats.solid_closed = step.info.is_closed;
        }
        if record.ended {
            rollout.diagnostics.stalled_episodes += usize::from(record.stalled);
            if let Some(summary) = record.summary.take() {
                rollout.episodes_improved += usize::from(summary.improvement() > 0.0);
                rollout.finished.push(summary);
            }
        }
    }
    if cancelled {
        // Cancellation keeps exact labels for states already evaluated, matching the legacy
        // partial-rollout behavior, but leaves report counters and PPO state untouched.
        runner.stats.labelled_samples = runner.aggregate.len();
    }
}

struct WorkerContext<'a> {
    cfg: RunConfig,
    split: &'a DatasetSplit,
    execution: Option<Arc<EnvExecution>>,
    progress: super::progress::TrainingProgress,
    budget: Arc<pool::PendingBudget>,
    focus_entry: usize,
    cpu_width: usize,
    started: Instant,
}

impl WorkerContext<'_> {
    fn step(&self, work: Work) -> DoneItem {
        let slot_index = work.slot_index;
        let local_step = work.local_step;
        let mut slot = work.slot;
        let step_started = self.started.elapsed().as_secs_f32();
        let result = catch_unwind(AssertUnwindSafe(|| {
            step_slot(
                &mut slot,
                &work.pending,
                &self.cfg,
                &self.progress,
                slot_index,
                pool::StagedLimits {
                    threads: self.cpu_width,
                    pending: MAX_PENDING_WORK,
                    shared: Some(Arc::clone(&self.budget)),
                },
            )
        }));
        match result {
            Ok((outcome, probe_secs)) => DoneItem::Step(Ok(Done {
                slot_index,
                local_step,
                slot,
                pending: work.pending,
                outcome,
                probe_interval: (probe_secs > 0.0)
                    .then_some((step_started, step_started + probe_secs)),
            })),
            Err(payload) => {
                let message = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| {
                        payload
                            .downcast_ref::<&'static str>()
                            .map(|s| (*s).to_owned())
                    })
                    .unwrap_or_else(|| "rollout worker panicked".into());
                DoneItem::Step(Err((slot_index, slot, message)))
            }
        }
    }

    fn build(&self, work: BuildWork) -> DoneItem {
        let _task = self.progress.task(format!(
            "respawn slot {} after step {}",
            work.slot_index, work.local_step
        ));
        let result = catch_unwind(AssertUnwindSafe(|| {
            build_stream_slot(
                &self.cfg,
                self.split,
                self.execution.as_ref(),
                self.focus_entry,
                work.start,
                work.seed,
            )
        }))
        .map_err(|_| "environment respawn worker panicked".to_owned())
        .and_then(|result| result);
        DoneItem::Build {
            slot_index: work.slot_index,
            local_step: work.local_step,
            fallback: work.fallback,
            result,
        }
    }

    fn run(&self, receiver: &Mutex<mpsc::Receiver<WorkItem>>, sender: &mpsc::Sender<DoneItem>) {
        loop {
            let next = receiver
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .recv();
            let Ok(work) = next else {
                break;
            };
            let slot_index = work.slot_index();
            let done = catch_job(slot_index, || match work {
                WorkItem::Step(work) => self.step(work),
                WorkItem::Build(work) => self.build(work),
            });
            let _ = sender.send(done);
        }
    }
}

type BuildWarnings = Vec<(usize, usize, Vec<String>)>;

struct Coordinator<'a> {
    cfg: &'a RunConfig,
    model: &'a super::BurnSetAttention,
    stats: &'a mut super::RunnerStats,
    progress: &'a super::progress::TrainingProgress,
    execution: Option<&'a Arc<EnvExecution>>,
    max_tiers: usize,
    label_phase: usize,
    iteration: usize,
    seed: u64,
    worker_count: usize,
    cpu_width: usize,
    tail_started: Option<(Instant, usize)>,
    tail_interval: Option<(Instant, usize, usize)>,
    slots: Vec<Option<Slot>>,
    quotas: Vec<usize>,
    completed: Vec<usize>,
    next_steps: Vec<usize>,
    ready: VecDeque<usize>,
    active: usize,
    records: Vec<Record>,
    warnings: BuildWarnings,
    stopped: bool,
    fatal: Option<String>,
}

impl Coordinator<'_> {
    fn select(&mut self, keep_going: &mut dyn FnMut() -> bool) -> Vec<usize> {
        let count = self.worker_count - self.active;
        let queued = take_ready_batch(&mut self.ready, count);
        let mut selected = Vec::with_capacity(count);
        for slot in queued {
            if !keep_going() {
                self.stopped = true;
                break;
            }
            selected.push(slot);
        }
        selected
    }

    fn submit_ready(&mut self, selected: &[usize], send: &mpsc::SyncSender<WorkItem>) {
        if selected.is_empty() {
            return;
        }
        let started = Instant::now();
        let mut states = Vec::with_capacity(selected.len());
        let mut counts = Vec::with_capacity(selected.len());
        for &index in selected {
            let slot = self.slots[index]
                .as_ref()
                .expect("ready slot owns its environment");
            match slot.obs.try_flatten(self.max_tiers) {
                Ok(state) => states.push(state),
                Err(error) => {
                    self.fatal = Some(error.to_string());
                    return;
                }
            }
            counts.push(slot.obs.tiers.len());
        }
        let (batch, profile) = match self.model.forward_host_profiled(&states, &counts) {
            Ok(result) => result,
            Err(error) => {
                self.fatal = Some(error.to_string());
                return;
            }
        };
        let diagnostics = &mut self.stats.rollout_scheduling;
        diagnostics.forward_batches += 1;
        diagnostics.real_rows += profile.real_rows;
        diagnostics.padded_rows += profile.padded_rows;
        diagnostics.padded_tiers += profile.padded_tiers;
        diagnostics.validation_ms += profile.validation_ms;
        diagnostics.inference_view_ms += profile.inference_view_ms;
        diagnostics.packing_ms += profile.packing_ms;
        diagnostics.forward_submit_ms += profile.forward_submit_ms;
        diagnostics.readback_ms += profile.readback_ms;
        diagnostics.unpacking_ms += profile.unpacking_ms;
        if batch.means.len() != selected.len()
            || batch.log_stds.len() != selected.len()
            || batch.values.len() != selected.len()
        {
            self.fatal = Some("batched inference returned an unexpected row count".into());
            return;
        }
        self.stats.rollout_forward_ms += started.elapsed().as_secs_f32() * 1_000.0;
        for (row, (&index, state)) in selected.iter().zip(states).enumerate() {
            let slot = self.slots[index].take().expect("forwarded slot is ready");
            let free = slot.obs.free_tier_mask();
            let policy = GaussianPolicy {
                mean: batch.means[row].clone(),
                log_std: batch.log_stds[row].clone(),
            };
            let local_step = self.next_steps[index];
            let mut rng = stream_rng(self.seed, ACTION_STREAM, self.iteration, index, local_step);
            let raw =
                super::imitation::explore_raw(&policy.mean, &free, self.cfg.explore_std, &mut rng);
            let max_step = slot.env.max_step_deg();
            let mut action = GaussianPolicy::squash(&raw, max_step);
            super::GemstoneSetAttentionNetwork::project_actions(
                &mut action,
                &slot.obs.tiers,
                slot.obs.global.target_critical_angle_deg,
                max_step,
            );
            let pending = PendingStep {
                free,
                raw,
                log_prob: 0.0,
                value: 0.0,
                action,
                state,
                labelled: stream_labelled(
                    self.cfg.label_every,
                    self.label_phase,
                    index,
                    local_step,
                ),
            };
            match send.send(WorkItem::Step(Work {
                slot_index: index,
                local_step,
                slot,
                pending,
            })) {
                Ok(()) => {
                    self.active += 1;
                    self.next_steps[index] += 1;
                }
                Err(error) => {
                    if let WorkItem::Step(work) = error.0 {
                        self.slots[index] = Some(work.slot);
                    }
                    self.fatal
                        .get_or_insert_with(|| "rollout worker queue closed".into());
                    break;
                }
            }
        }
    }

    fn finish_step(&mut self, mut done: Done, send: &mpsc::SyncSender<WorkItem>) {
        done.slot.obs = done.outcome.step.observation.clone();
        done.slot.rejected_steps = if done.outcome.step.info.rejection.is_some() {
            done.slot.rejected_steps.saturating_add(1)
        } else {
            0
        };
        let stalled = !done.outcome.step.terminated
            && !done.outcome.step.truncated
            && self.cfg.max_rejected_steps > 0
            && done.slot.rejected_steps >= self.cfg.max_rejected_steps;
        let ended = done.outcome.step.terminated || done.outcome.step.truncated || stalled;
        let optics = super::Optics::from_global(&done.outcome.step.observation.global);
        let episode_summary = ended.then(|| summary(&done.slot, optics));
        if ended {
            let start = if self.cfg.corpus_mode {
                done.slot.pos + self.cfg.effective_workers()
            } else {
                0
            };
            let job = BuildWork {
                slot_index: done.slot_index,
                local_step: done.local_step,
                start,
                seed: respawn_seed(self.seed, self.iteration, done.slot_index, done.local_step),
                fallback: done.slot,
            };
            if let Err(error) = send.send(WorkItem::Build(job)) {
                if let WorkItem::Build(work) = error.0 {
                    self.slots[work.slot_index] = Some(work.fallback);
                }
                self.fatal
                    .get_or_insert_with(|| "respawn worker queue closed".into());
                return;
            }
            self.active += 1;
        } else {
            self.slots[done.slot_index] = Some(done.slot);
        }
        self.records.push(Record {
            slot_index: done.slot_index,
            local_step: done.local_step,
            pending: done.pending,
            outcome: done.outcome,
            ended,
            stalled,
            probe_interval: done.probe_interval,
            summary: episode_summary,
        });
        self.completed[done.slot_index] += 1;
        self.progress.steps(self.records.len());
        if !ended && self.completed[done.slot_index] < self.quotas[done.slot_index] {
            self.ready.push_back(done.slot_index);
        }
    }

    fn handle_event(&mut self, event: DoneItem, send: &mpsc::SyncSender<WorkItem>) {
        self.active -= 1;
        match event {
            DoneItem::LostSlot { message, .. } => {
                if let Some(previous) = &mut self.fatal {
                    previous.push_str("; ");
                    previous.push_str(&message);
                } else {
                    self.fatal = Some(message);
                }
            }
            DoneItem::Step(Err((index, slot, message))) => {
                self.slots[index] = Some(slot);
                self.fatal.get_or_insert(message);
            }
            DoneItem::Step(Ok(done)) => self.finish_step(done, send),
            DoneItem::Build {
                slot_index,
                local_step,
                fallback,
                result,
            } => match result {
                Ok((replacement, warnings)) => {
                    if slot_index == 0 {
                        let optics = super::Optics::from_global(&replacement.obs.global);
                        self.stats.baseline = Some(optics);
                        self.stats.current = Some(optics);
                    }
                    self.warnings.push((slot_index, local_step, warnings));
                    self.slots[slot_index] = Some(replacement);
                    if self.completed[slot_index] < self.quotas[slot_index] {
                        self.ready.push_back(slot_index);
                    }
                }
                Err(message) => {
                    self.slots[slot_index] = Some(fallback);
                    self.fatal.get_or_insert(message);
                }
            },
        }
    }

    fn receive_events(
        &mut self,
        receive: &mpsc::Receiver<DoneItem>,
        send: &mpsc::SyncSender<WorkItem>,
    ) -> bool {
        let first = match receive.recv() {
            Ok(event) => event,
            Err(_) => {
                self.fatal.get_or_insert_with(|| {
                    "rollout workers stopped before returning all in-flight slots".into()
                });
                return false;
            }
        };
        let mut events = vec![first];
        while let Ok(event) = receive.try_recv() {
            events.push(event);
        }
        events.sort_by_key(|event| match event {
            DoneItem::LostSlot { slot_index, .. } => (*slot_index, usize::MAX, 0usize),
            DoneItem::Step(Ok(done)) => (done.slot_index, done.local_step, 0usize),
            DoneItem::Step(Err((w, _, _))) => (*w, usize::MAX, 0),
            DoneItem::Build {
                slot_index,
                local_step,
                ..
            } => (*slot_index, *local_step, 1),
        });
        for event in events {
            self.handle_event(event, send);
            self.track_tail();
        }
        if self
            .execution
            .is_some_and(|execution| execution.check().is_err())
        {
            self.fatal
                .get_or_insert_with(|| "environment execution failed during rollout".into());
        }
        true
    }

    fn track_tail(&mut self) {
        let now = Instant::now();
        let (slots, steps) = remaining_quota(&self.quotas, &self.completed);
        if let Some((previous, previous_slots, previous_records)) = self.tail_interval {
            let buckets = &mut self.stats.rollout_scheduling.tail_by_remaining_slots;
            if let Some(bucket) = buckets
                .iter_mut()
                .find(|bucket| bucket.slots_with_steps_remaining == previous_slots)
            {
                bucket.wall_secs += now.duration_since(previous).as_secs_f64();
                bucket.completed_steps += self.records.len() - previous_records;
            } else {
                buckets.push(crate::stats::RolloutTailBucket {
                    slots_with_steps_remaining: previous_slots,
                    wall_secs: now.duration_since(previous).as_secs_f64(),
                    completed_steps: self.records.len() - previous_records,
                });
            }
            self.tail_interval = Some((now, slots, self.records.len()));
        } else if slots > 0 && slots <= self.cpu_width {
            let diagnostics = &mut self.stats.rollout_scheduling;
            diagnostics.tail_slot_threshold = self.cpu_width;
            diagnostics.tail_start_slots = slots;
            diagnostics.tail_start_steps_remaining = steps;
            self.tail_started = Some((now, self.records.len()));
            self.tail_interval = Some((now, slots, self.records.len()));
        }
    }

    fn drive(
        &mut self,
        receive: &mpsc::Receiver<DoneItem>,
        send: &mpsc::SyncSender<WorkItem>,
        keep_going: &mut dyn FnMut() -> bool,
    ) {
        self.track_tail();
        while self.active > 0 || (!self.ready.is_empty() && !self.stopped && self.fatal.is_none()) {
            if !self.stopped
                && self.fatal.is_none()
                && self.active < self.worker_count
                && !self.ready.is_empty()
            {
                let selected = self.select(keep_going);
                self.submit_ready(&selected, send);
            }
            if self.active == 0 {
                continue;
            }
            if !self.receive_events(receive, send) {
                break;
            }
        }
        self.track_tail();
        if let Some((started, records)) = self.tail_started {
            self.stats.rollout_scheduling.tail_secs = started.elapsed().as_secs_f64();
            self.stats.rollout_scheduling.tail_completed_steps = self.records.len() - records;
        }
    }
}

fn run_coordinators(
    worker: &WorkerContext<'_>,
    coordinator: &mut Coordinator<'_>,
    keep_going: &mut dyn FnMut() -> bool,
) {
    let worker_count = coordinator.worker_count;
    thread::scope(|scope| {
        let (send, receive_work) = mpsc::sync_channel::<WorkItem>(worker_count);
        let receive_work = Arc::new(Mutex::new(receive_work));
        let (done, receive_done) = mpsc::channel::<DoneItem>();
        for _ in 0..worker_count {
            let receiver = Arc::clone(&receive_work);
            let sender = done.clone();
            scope.spawn(move || worker.run(&receiver, &sender));
        }
        drop(done);
        coordinator.drive(&receive_done, &send, keep_going);
    });
}

/// Collect imitation with fixed round-robin quotas and independent deterministic slot streams.
pub(super) fn collect(
    runner: &mut TrainingRunner,
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<Option<RolloutStats>, RunnerError> {
    let slot_count = runner.slots.len();
    if slot_count == 0 {
        return Err(RunnerError::Data("rollout has no environment slots".into()));
    }
    let cpu_width = pool::pool_threads();
    let worker_count = coordinator_count(slot_count, runner.cfg.rollout_size, cpu_width);
    let quotas = fixed_quotas(runner.cfg.rollout_size, slot_count);
    let started = Instant::now();
    let seed = runner.rng.next_u64();
    let iteration = runner.stats.iteration;
    runner.progress.phase("streaming imitation rollout", 0);
    let worker = WorkerContext {
        cfg: runner.cfg.clone(),
        split: &runner.split,
        execution: runner.execution.clone(),
        progress: runner.progress.clone(),
        budget: Arc::new(pool::PendingBudget::new(MAX_PENDING_WORK)),
        focus_entry: runner.focus_entry,
        cpu_width,
        started,
    };
    let mut coordinator = Coordinator {
        cfg: &worker.cfg,
        model: &runner.rollout_model,
        stats: &mut runner.stats,
        progress: &worker.progress,
        execution: worker.execution.as_ref(),
        max_tiers: worker.split.max_tiers,
        label_phase: runner.label_phase,
        iteration,
        seed,
        worker_count,
        cpu_width,
        tail_started: None,
        tail_interval: None,
        slots: std::mem::take(&mut runner.slots)
            .into_iter()
            .map(Some)
            .collect(),
        ready: (0..slot_count).filter(|&w| quotas[w] > 0).collect(),
        quotas,
        completed: vec![0; slot_count],
        next_steps: vec![0; slot_count],
        active: 0,
        records: Vec::with_capacity(worker.cfg.rollout_size),
        warnings: Vec::new(),
        stopped: false,
        fatal: None,
    };
    run_coordinators(&worker, &mut coordinator, keep_going);
    let Coordinator {
        slots,
        mut records,
        mut warnings,
        stopped,
        fatal,
        ..
    } = coordinator;
    runner.slots = restore_owned_slots(slots, fatal.as_deref())?;
    warnings.sort_by_key(|(slot, local_step, _)| (*slot, *local_step));
    runner
        .warnings
        .extend(warnings.into_iter().flat_map(|(_, _, warnings)| warnings));
    // The phase counts passes: a slot with the full quota continues its labelling stride in the
    // next rollout, and a resumed run labels exactly as an uninterrupted one.
    let phase_advances = records
        .iter()
        .map(|record| record.local_step + 1)
        .max()
        .unwrap_or(0);
    runner.label_phase = runner.label_phase.wrapping_add(phase_advances);
    let mut rollout = RolloutStats::default();
    commit_records(
        runner,
        &mut records,
        &mut rollout,
        started.elapsed().as_secs_f32(),
        stopped || fatal.is_some(),
    );
    if let Some(slot) = runner.slots.first() {
        runner.stats.current = Some(super::Optics::from_global(&slot.obs.global));
    }
    if let Some(error) = fatal {
        return Err(RunnerError::Data(error));
    }
    if stopped {
        return Ok(None);
    }
    Ok(Some(rollout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unexpected_job_panic_reports_lost_slot_and_prevents_partial_restore() {
        let event = catch_job(7, || panic!("injected outer worker panic"));
        let DoneItem::LostSlot {
            slot_index,
            message,
        } = event
        else {
            panic!("expected a lost-slot event");
        };
        assert_eq!(slot_index, 7);
        assert!(message.contains("injected outer worker panic"));
        let error = restore_owned_slots(vec![None], Some(&message))
            .err()
            .expect("missing slot must fail");
        let description = error.to_string();
        assert!(description.contains("injected outer worker panic"));
        assert!(description.contains("restore the last committed recovery"));
    }

    #[test]
    fn fixed_quotas_cover_the_rollout_without_fast_slot_stealing() {
        for (rollout, slots) in [(256, 27), (17, 6), (3, 12), (1024, 1024)] {
            let quotas = fixed_quotas(rollout, slots);
            assert_eq!(quotas.iter().sum::<usize>(), rollout);
            assert!(quotas.iter().max().unwrap() - quotas.iter().min().unwrap() <= 1);
            for local_step in 0..quotas.iter().copied().max().unwrap_or(0) {
                let active: Vec<_> = quotas
                    .iter()
                    .enumerate()
                    .filter_map(|(slot, &quota)| (local_step < quota).then_some(slot))
                    .collect();
                assert_eq!(
                    active.len(),
                    slots.min(rollout.saturating_sub(local_step * slots))
                );
            }
        }
    }

    #[test]
    fn remaining_quota_counts_only_unfinished_slots_and_steps() {
        assert_eq!(remaining_quota(&[3, 2, 1, 0], &[3, 1, 0, 0]), (2, 2));
        assert_eq!(remaining_quota(&[3, 2, 1], &[3, 2, 1]), (0, 0));
    }

    #[test]
    fn streamed_labels_spread_evenly_over_every_slot_and_each_slots_steps() {
        for slots in 1..=40 {
            for every in 1..=4 {
                for rollout in [1, slots, 3 * slots + 1, 64, 256] {
                    let quotas = fixed_quotas(rollout, slots);
                    for phase in [0, 1, 2, 3, 875, 880] {
                        let counts: Vec<usize> = quotas
                            .iter()
                            .enumerate()
                            .map(|(slot, &quota)| {
                                let labelled: Vec<bool> = (0..quota)
                                    .map(|step| stream_labelled(every, phase, slot, step))
                                    .collect();
                                if every >= 2 && quota >= 2 {
                                    assert!(
                                        labelled.windows(2).all(|pair| !(pair[0] && pair[1])),
                                        "slot {slot} of {slots} labels consecutive steps \
                                         (every {every}, phase {phase})"
                                    );
                                }
                                labelled.iter().filter(|&&flag| flag).count()
                            })
                            .collect();
                        let total: usize = counts.iter().sum();
                        for (slot, &count) in counts.iter().enumerate() {
                            // |count - total / slots| <= 1, in integers.
                            assert!(
                                (count * slots).abs_diff(total) <= slots,
                                "slot {slot} labels {count} of {total} \
                                 ({slots} slots, every {every}, rollout {rollout}, phase {phase})"
                            );
                        }
                        // |total - rollout / every| < every: the share stays one in `every`.
                        assert!(
                            (total * every).abs_diff(rollout) < every * every,
                            "{total} labels for {rollout} steps \
                             ({slots} slots, every {every}, phase {phase})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_label_phase_continues_each_full_slots_stride_across_rollouts() {
        // 27 slots and 256 steps: slots 0..13 take 10 steps, so the phase advances by 10 per
        // rollout. The old sample-index schedule labelled slots 0, 2, .., 26 on every step.
        let quotas = fixed_quotas(256, 27);
        let passes = quotas.iter().copied().max().unwrap_or(0);
        for every in 1..=4 {
            let mut phase = 7;
            let mut history = Vec::new();
            for _ in 0..3 {
                history.extend((0..quotas[0]).map(|step| stream_labelled(every, phase, 0, step)));
                phase += passes;
            }
            let expected: Vec<bool> = (0..history.len())
                .map(|step| (7 + step).is_multiple_of(every))
                .collect();
            assert_eq!(history, expected, "every {every}");
        }
        let labelled_slots: Vec<usize> = (0..27)
            .filter(|&slot| (0..quotas[slot]).all(|step| stream_labelled(2, 880, slot, step)))
            .collect();
        assert!(labelled_slots.is_empty(), "{labelled_slots:?}");
    }

    #[test]
    fn action_streams_do_not_depend_on_completion_order() {
        let expected: Vec<_> = (0..8)
            .map(|slot| {
                let mut rng = stream_rng(71, ACTION_STREAM, 12, slot, 5);
                (0..4).map(|_| rng.next_f32()).collect::<Vec<_>>()
            })
            .collect();
        let mut observed = vec![Vec::new(); 8];
        for slot in [7, 1, 6, 0, 5, 2, 4, 3] {
            let mut rng = stream_rng(71, ACTION_STREAM, 12, slot, 5);
            observed[slot] = (0..4).map(|_| rng.next_f32()).collect();
        }
        assert_eq!(observed, expected);
    }

    #[test]
    fn variable_completion_order_commits_records_canonically() {
        let mut completed = vec![(2, 0), (0, 1), (1, 1), (0, 0), (2, 1), (1, 0)];
        canonical_slot_order(&mut completed, |&(slot, step)| (slot, step));
        assert_eq!(completed, [(0, 0), (0, 1), (1, 0), (1, 1), (2, 0), (2, 1)]);
    }

    #[test]
    fn a_slow_slot_does_not_block_an_unrelated_ready_slot() {
        // Slots 0 and 27 previously shared a lane. While slot 0 is in flight,
        // any freed coordinator must be able to take slot 27.
        let mut ready: VecDeque<_> = (0..54).collect();
        assert_eq!(
            take_ready_batch(&mut ready, 27),
            (0..27).collect::<Vec<_>>()
        );
        assert_eq!(take_ready_batch(&mut ready, 1), [27]);
    }

    #[test]
    fn coordinator_count_is_bounded_with_many_slots() {
        for (slots, rollout, cpus) in [(27, 256, 9), (1024, 1024, 9), (256, 8, 4), (12, 256, 1)] {
            let coordinators = coordinator_count(slots, rollout, cpus);
            assert!(coordinators <= slots.min(rollout).max(1));
            assert!(coordinators <= MAX_PENDING_WORK);
        }
    }

    #[test]
    fn merged_probe_intervals_measure_wall_clock_union_within_rollout_elapsed() {
        let elapsed = 2.0f32;
        let intervals = [(0.1, 1.4), (0.3, 1.8), (1.9, 2.5)];
        let wall = merge_probe_intervals(intervals, elapsed);
        assert!((wall - 1.8).abs() < 1e-5, "wall={wall}");
        assert!(wall <= elapsed);
    }
}
