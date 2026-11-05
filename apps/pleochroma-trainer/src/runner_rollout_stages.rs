//! Per-slot teacher and policy continuations; no sampling or commit-order decisions live here.

use std::{
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    time::Instant,
};

use pleochroma_ai::env::{StepWork, TeacherLabel, TeacherLabelWork, TierProbe, TierProbeWork};

use super::{PendingStep, Slot, SlotOutcome, pool};
use crate::runner::progress::{TaskGuard, TrainingProgress};

// A fixed payload bound independent of episode count and CPU width. A probe owns at most two
// optical requests; the native optical channel has its own capacity and admission control.
pub(in super::super) const MAX_PENDING_WORK: usize = 128;

pub(super) struct PassContext<'a> {
    pub pending: &'a [PendingStep],
    pub fresh_labels: &'a [bool],
    pub probe_sizes: &'a [f64],
    pub cached_labels: &'a [Option<TeacherLabel>],
    pub scales: &'a [f64],
    pub progress: &'a TrainingProgress,
    pub slot_offset: usize,
    pub announce_phase: bool,
}

struct PassStages<'a> {
    context: PassContext<'a>,
    started: Instant,
    remaining_probes: AtomicUsize,
    probe_micros: AtomicU64,
}

struct ProbeWork {
    inner: TierProbeWork,
    task: TaskGuard,
}

/// The finish of one slot: the teacher search of a fresh label and the policy search run side
/// by side. Neither reads the other: both only read the unchanged pre-step state (the policy
/// search moves by the sampled action, never by the label), and the environment changes only
/// when the step commits, after the label is complete and cached for that state.
struct SlotWork {
    /// The teacher search, until its label is complete.
    teacher: Option<TeacherLabelWork>,
    /// The policy search; taken when the step commits.
    step: Option<StepWork>,
    /// Whether the policy search has its result and only waits for the teacher.
    step_done: bool,
    label: Option<TeacherLabel>,
    fresh_label: bool,
    task: TaskGuard,
}

impl SlotWork {
    const fn wait_detail(&self) -> &'static str {
        match (self.teacher.is_some(), self.step_done) {
            (true, false) => "teacher and policy optics wait",
            (true, true) => "teacher optics wait",
            (false, _) => "policy optics wait",
        }
    }
}

/// Runs `a` then `b`, or both at once on the pool when it has spare capacity. Each closure
/// returns the same value either way, so only the timing depends on the choice.
fn both<A: Send, B: Send>(
    parallel: bool,
    a: impl FnOnce() -> A + Send,
    b: impl FnOnce() -> B + Send,
) -> (A, B) {
    if parallel {
        rayon::join(a, b)
    } else {
        (a(), b())
    }
}

/// The longest-first estimate remains free tiers × (1 + labelled) × design tiers.
fn step_cost(slot: &Slot, labelled: bool) -> u64 {
    let free = slot.env.free_tiers().len() as u64;
    free.max(1) * (1 + u64::from(labelled)) * slot.obs.tiers.len() as u64
}

pub(super) fn step_pass(slots: &mut [Slot], context: PassContext<'_>) -> (Vec<SlotOutcome>, f32) {
    let threads = pool::pool_threads();
    step_pass_with_limits(
        slots,
        context,
        pool::StagedLimits {
            threads,
            pending: MAX_PENDING_WORK,
            shared: None,
        },
    )
}

pub(super) fn step_pass_with_limits(
    slots: &mut [Slot],
    context: PassContext<'_>,
    limits: pool::StagedLimits,
) -> (Vec<SlotOutcome>, f32) {
    let plans: Vec<_> = slots
        .iter()
        .zip(context.fresh_labels)
        .map(|(slot, &fresh)| pool::ProbePlan {
            probes: if fresh {
                slot.env.free_tiers().to_vec()
            } else {
                Vec::new()
            },
            probe_cost: slot.obs.tiers.len() as u64,
            finish_cost: step_cost(slot, fresh),
        })
        .collect();
    let probes = plans.iter().map(|plan| plan.probes.len()).sum();
    if context.announce_phase {
        context
            .progress
            .phase("probes, oracle-and-steps", probes + slots.len());
    }
    let stages = PassStages {
        context,
        started: Instant::now(),
        remaining_probes: AtomicUsize::new(probes),
        probe_micros: AtomicU64::new(0),
    };
    let outcomes = pool::run_staged_items_ranked(slots, &plans, limits, &stages);
    (
        outcomes,
        stages.probe_micros.load(Ordering::Relaxed) as f32 / 1_000_000.0,
    )
}

pub(in super::super) fn step_slot(
    slot: &mut Slot,
    pending: &PendingStep,
    cfg: &super::RunConfig,
    progress: &TrainingProgress,
    slot_index: usize,
    limits: pool::StagedLimits,
) -> (SlotOutcome, f32) {
    let probe_size = cfg.label_h_deg.unwrap_or_else(|| slot.env.max_step_deg());
    let cached_label = pending
        .labelled
        .then(|| {
            slot.label_cache
                .get(slot.env.current_design(), probe_size, &cfg.line_search)
        })
        .flatten();
    let fresh_label = pending.labelled && cached_label.is_none();
    let pending = [PendingStep {
        free: pending.free.clone(),
        raw: pending.raw.clone(),
        log_prob: pending.log_prob,
        value: pending.value,
        action: pending.action.clone(),
        state: pending.state.clone(),
        labelled: pending.labelled,
    }];
    let fresh_labels = [fresh_label];
    let probe_sizes = [probe_size];
    let cached_labels = [cached_label];
    let (mut outcomes, seconds) = step_pass_with_limits(
        std::slice::from_mut(slot),
        PassContext {
            pending: &pending,
            fresh_labels: &fresh_labels,
            probe_sizes: &probe_sizes,
            cached_labels: &cached_labels,
            scales: &cfg.line_search,
            progress,
            slot_offset: slot_index,
            announce_phase: false,
        },
        limits,
    );
    (outcomes.remove(0), seconds)
}

impl pool::ItemStages<Slot> for &PassStages<'_> {
    type ProbeWork = ProbeWork;
    type Probe = TierProbe;
    type FinishWork = SlotWork;
    type Output = SlotOutcome;

    fn prepare_probe(&self, w: usize, slot: &Slot, tier: usize, tail: &pool::Tail) -> ProbeWork {
        let slot_index = w + self.context.slot_offset;
        let task = self.context.progress.task(format!(
            "probe preparation slot {slot_index} entry {} tier {tier}",
            slot.entry
        ));
        let inner =
            slot.env
                .prepare_tier_probe_with(tier, self.context.probe_sizes[w], tail.is_active());
        task.update_detail(format!(
            "probe optics wait slot {slot_index} entry {} tier {tier}",
            slot.entry
        ));
        ProbeWork { inner, task }
    }

    fn poll_probe(&self, work: &mut ProbeWork) -> bool {
        work.inner.poll()
    }

    fn complete_probe(&self, w: usize, slot: &Slot, tier: usize, work: ProbeWork) -> TierProbe {
        let slot_index = w + self.context.slot_offset;
        work.task.update_detail(format!(
            "probe completion slot {slot_index} entry {} tier {tier}",
            slot.entry
        ));
        let result = slot.env.complete_tier_probe(work.inner);
        if self.remaining_probes.fetch_sub(1, Ordering::Relaxed) == 1 {
            let micros = u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX);
            self.probe_micros.store(micros, Ordering::Relaxed);
        }
        result
    }

    fn begin_finish(
        &self,
        w: usize,
        slot: &mut Slot,
        probes: Vec<(usize, TierProbe)>,
        tail: &pool::Tail,
    ) -> SlotWork {
        let slot_index = w + self.context.slot_offset;
        let task = self.context.progress.task(format!(
            "oracle/step slot {slot_index} entry {}",
            slot.entry
        ));
        let fresh = self.context.fresh_labels[w];
        let label = self.context.cached_labels[w].clone();
        if !fresh {
            self.context
                .progress
                .labels(0, usize::from(label.is_some()));
        }
        let parallel = || tail.is_active();
        let env = &slot.env;
        let action = &self.context.pending[w].action;
        let scales = self.context.scales;
        let prepare_step = || env.prepare_step_scaled_adaptive(action, scales, &parallel);
        let (teacher, step) = if fresh {
            task.update_detail(format!(
                "teacher and policy preparation slot {slot_index} entry {}",
                slot.entry
            ));
            let mut set = vec![TierProbe::default(); slot.obs.tiers.len()];
            for (tier, probe) in probes {
                set[tier] = probe;
            }
            let h_deg = self.context.probe_sizes[w];
            let (teacher, step) = both(
                tail.is_active(),
                || env.prepare_teacher_label_adaptive(h_deg, &set, scales, &parallel),
                prepare_step,
            );
            (Some(teacher), step)
        } else {
            (None, prepare_step())
        };
        let work = SlotWork {
            teacher,
            step: Some(step),
            step_done: false,
            label,
            fresh_label: fresh,
            task,
        };
        work.task.update_detail(format!(
            "{} slot {slot_index} entry {}",
            work.wait_detail(),
            slot.entry
        ));
        work
    }

    fn poll_finish(&self, work: &mut SlotWork) -> bool {
        // Poll both, so each retries its own bounded submission.
        let teacher = work.teacher.as_mut().is_some_and(TeacherLabelWork::poll);
        let step = !work.step_done && work.step.as_mut().is_some_and(StepWork::poll);
        teacher || step
    }

    fn advance_finish(
        &self,
        w: usize,
        slot: &mut Slot,
        work: &mut SlotWork,
        tail: &pool::Tail,
    ) -> Option<SlotOutcome> {
        let slot_index = w + self.context.slot_offset;
        work.task.update_detail(format!(
            "oracle/step advance slot {slot_index} entry {}",
            slot.entry
        ));
        let teacher_ready = work.teacher.as_mut().is_some_and(TeacherLabelWork::poll);
        let step_ready = !work.step_done && work.step.as_mut().is_some_and(StepWork::poll);
        let step_was_done = work.step_done;
        let parallel = || tail.is_active();
        let env = &slot.env;
        let SlotWork { teacher, step, .. } = &mut *work;
        // A search that is not ready returns false at once, without changing its work.
        let (teacher_done, step_done) = both(
            teacher_ready && step_ready && tail.is_active(),
            || {
                teacher
                    .as_mut()
                    .is_none_or(|teacher| env.advance_teacher_label_adaptive(teacher, &parallel))
            },
            || {
                step_was_done
                    || env.advance_step_adaptive(
                        step.as_mut().expect("policy search prepared"),
                        &parallel,
                    )
            },
        );
        work.step_done = step_done;
        if teacher_done && let Some(teacher) = work.teacher.take() {
            // Cached for the pre-step state, before the step below changes the environment.
            let label = slot.env.complete_teacher_label(teacher);
            slot.label_cache.put(
                slot.env.current_design(),
                self.context.probe_sizes[w],
                self.context.scales,
                label.clone(),
            );
            work.label = Some(label);
            self.context.progress.labels(1, 0);
        }
        if work.teacher.is_some() || !work.step_done {
            work.task.update_detail(format!(
                "{} slot {slot_index} entry {}",
                work.wait_detail(),
                slot.entry
            ));
            return None;
        }
        Some(SlotOutcome {
            step: slot
                .env
                .complete_step(work.step.take().expect("policy search complete")),
            label: work.label.take(),
            fresh_label: work.fresh_label,
        })
    }
}
