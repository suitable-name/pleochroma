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
const MAX_PENDING_WORK: usize = 128;

pub(super) struct PassContext<'a> {
    pub pending: &'a [PendingStep],
    pub fresh_labels: &'a [bool],
    pub probe_sizes: &'a [f64],
    pub cached_labels: &'a [Option<TeacherLabel>],
    pub scales: &'a [f64],
    pub progress: &'a TrainingProgress,
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

struct SlotWork {
    teacher: Option<TeacherLabelWork>,
    step: Option<StepWork>,
    label: Option<TeacherLabel>,
    fresh_label: bool,
    task: TaskGuard,
}

/// The longest-first estimate remains free tiers × (1 + labelled) × design tiers.
fn step_cost(slot: &Slot, labelled: bool) -> u64 {
    let free = slot.env.free_tiers().len() as u64;
    free.max(1) * (1 + u64::from(labelled)) * slot.obs.tiers.len() as u64
}

pub(super) fn step_pass(slots: &mut [Slot], context: PassContext<'_>) -> (Vec<SlotOutcome>, f32) {
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
    context
        .progress
        .phase("probes, oracle-and-steps", probes + slots.len());
    let stages = PassStages {
        context,
        started: Instant::now(),
        remaining_probes: AtomicUsize::new(probes),
        probe_micros: AtomicU64::new(0),
    };
    let outcomes = pool::run_staged_items_ranked(
        slots,
        &plans,
        pool::StagedLimits {
            threads: pool::pool_threads(),
            pending: MAX_PENDING_WORK,
        },
        &stages,
    );
    (
        outcomes,
        stages.probe_micros.load(Ordering::Relaxed) as f32 / 1_000_000.0,
    )
}

impl pool::ItemStages<Slot> for &PassStages<'_> {
    type ProbeWork = ProbeWork;
    type Probe = TierProbe;
    type FinishWork = SlotWork;
    type Output = SlotOutcome;

    fn prepare_probe(&self, w: usize, slot: &Slot, tier: usize, tail: &pool::Tail) -> ProbeWork {
        let task = self.context.progress.task(format!(
            "probe preparation slot {w} entry {} tier {tier}",
            slot.entry
        ));
        let inner =
            slot.env
                .prepare_tier_probe_with(tier, self.context.probe_sizes[w], tail.is_active());
        task.update_detail(format!(
            "probe optics wait slot {w} entry {} tier {tier}",
            slot.entry
        ));
        ProbeWork { inner, task }
    }

    fn poll_probe(&self, work: &mut ProbeWork) -> bool {
        work.inner.poll()
    }

    fn complete_probe(&self, w: usize, slot: &Slot, tier: usize, work: ProbeWork) -> TierProbe {
        work.task.update_detail(format!(
            "probe completion slot {w} entry {} tier {tier}",
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
        let task = self
            .context
            .progress
            .task(format!("oracle/step slot {w} entry {}", slot.entry));
        let fresh = self.context.fresh_labels[w];
        let label = self.context.cached_labels[w].clone();
        let mut work = SlotWork {
            teacher: None,
            step: None,
            label,
            fresh_label: fresh,
            task,
        };
        if fresh {
            work.task
                .update_detail(format!("teacher preparation slot {w} entry {}", slot.entry));
            let mut set = vec![TierProbe::default(); slot.obs.tiers.len()];
            for (tier, probe) in probes {
                set[tier] = probe;
            }
            work.teacher = Some(slot.env.prepare_teacher_label_adaptive(
                self.context.probe_sizes[w],
                &set,
                self.context.scales,
                &|| tail.is_active(),
            ));
            work.task
                .update_detail(format!("teacher optics wait slot {w} entry {}", slot.entry));
        } else {
            self.context
                .progress
                .labels(0, usize::from(work.label.is_some()));
            work.step = Some(slot.env.prepare_step_scaled_adaptive(
                &self.context.pending[w].action,
                self.context.scales,
                &|| tail.is_active(),
            ));
            work.task
                .update_detail(format!("policy optics wait slot {w} entry {}", slot.entry));
        }
        work
    }

    fn poll_finish(&self, work: &mut SlotWork) -> bool {
        if let Some(teacher) = &mut work.teacher {
            teacher.poll()
        } else {
            work.step
                .as_mut()
                .expect("slot has a search continuation")
                .poll()
        }
    }

    fn advance_finish(
        &self,
        w: usize,
        slot: &mut Slot,
        work: &mut SlotWork,
        tail: &pool::Tail,
    ) -> Option<SlotOutcome> {
        if let Some(teacher) = &mut work.teacher {
            work.task
                .update_detail(format!("teacher advance slot {w} entry {}", slot.entry));
            if !slot
                .env
                .advance_teacher_label_adaptive(teacher, &|| tail.is_active())
            {
                work.task
                    .update_detail(format!("teacher optics wait slot {w} entry {}", slot.entry));
                return None;
            }
            let label = slot
                .env
                .complete_teacher_label(work.teacher.take().expect("teacher complete"));
            slot.label_cache.put(
                slot.env.current_design(),
                self.context.probe_sizes[w],
                self.context.scales,
                label.clone(),
            );
            work.label = Some(label);
            self.context.progress.labels(1, 0);
            work.task
                .update_detail(format!("policy preparation slot {w} entry {}", slot.entry));
            work.step = Some(slot.env.prepare_step_scaled_adaptive(
                &self.context.pending[w].action,
                self.context.scales,
                &|| tail.is_active(),
            ));
            work.task
                .update_detail(format!("policy optics wait slot {w} entry {}", slot.entry));
            return None;
        }
        let step = work.step.as_mut().expect("policy search prepared");
        work.task
            .update_detail(format!("policy advance slot {w} entry {}", slot.entry));
        if !slot.env.advance_step_adaptive(step, &|| tail.is_active()) {
            work.task
                .update_detail(format!("policy optics wait slot {w} entry {}", slot.entry));
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
