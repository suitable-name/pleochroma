//! Thread-safe progress snapshots for long training operations.
//!
//! This is diagnostic state only. It is deliberately separate from checkpoint and telemetry
//! state, and never participates in training decisions.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use serde::Serialize;

#[derive(Clone)]
pub struct TrainingProgress(Arc<Mutex<ProgressState>>);

struct ProgressState {
    iteration: usize,
    started: Instant,
    phase: &'static str,
    phase_started: Instant,
    total_steps: usize,
    completed_steps: usize,
    completed_tasks: usize,
    total_tasks: usize,
    fresh_labels: usize,
    reused_labels: usize,
    last_task_completion: Option<Instant>,
    last_task_detail: Option<String>,
    active: BTreeMap<u64, ActiveTask>,
    next_task_id: u64,
    status: &'static str,
    execution: Option<Arc<pleochroma_ai::env::EnvExecution>>,
}

struct ActiveTask {
    detail: String,
    started: Instant,
    detail_started: Instant,
}

/// A serializable point-in-time view of the runner's progress.
#[derive(Clone, Debug, Serialize)]
pub struct ProgressSnapshot {
    pub environment: Option<pleochroma_ai::env::ExecutionSnapshot>,
    pub iteration: usize,
    pub status: &'static str,
    pub phase: &'static str,
    pub elapsed_seconds: f64,
    pub phase_elapsed_seconds: f64,
    pub completed_steps: usize,
    pub total_steps: usize,
    pub completed_tasks: usize,
    /// Expected phase tasks; zero in JSON means the total is unknown.
    pub total_tasks: usize,
    pub fresh_labels: usize,
    pub reused_labels: usize,
    pub seconds_since_task_completion: Option<f64>,
    pub last_task_detail: Option<String>,
    pub oldest_active_task: Option<String>,
    pub oldest_active_task_seconds: Option<f64>,
    /// Age of the oldest task's currently reported detail, independent of task age.
    pub oldest_active_detail_seconds: Option<f64>,
}

impl Default for TrainingProgress {
    fn default() -> Self {
        let now = Instant::now();
        Self(Arc::new(Mutex::new(ProgressState {
            iteration: 0,
            started: now,
            phase: "idle",
            phase_started: now,
            total_steps: 0,
            completed_steps: 0,
            completed_tasks: 0,
            total_tasks: 0,
            fresh_labels: 0,
            reused_labels: 0,
            last_task_completion: None,
            last_task_detail: None,
            active: BTreeMap::new(),
            next_task_id: 0,
            status: "idle",
            execution: None,
        })))
    }
}

impl TrainingProgress {
    pub(super) fn set_execution(&self, execution: Option<Arc<pleochroma_ai::env::EnvExecution>>) {
        self.lock().execution = execution;
    }

    fn lock(&self) -> MutexGuard<'_, ProgressState> {
        // A poisoned diagnostic lock should not take down a training run. The lock is only
        // mutated by these small bookkeeping methods, so recover its state and continue.
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Starts a new iteration and resets that iteration's counters.
    pub fn begin(&self, iteration: usize, total_steps: usize) {
        let now = Instant::now();
        let mut state = self.lock();
        state.iteration = iteration;
        state.started = now;
        state.phase = "starting";
        state.phase_started = now;
        state.total_steps = total_steps;
        state.completed_steps = 0;
        state.completed_tasks = 0;
        state.total_tasks = 0;
        state.fresh_labels = 0;
        state.reused_labels = 0;
        state.last_task_completion = None;
        state.last_task_detail = None;
        state.active.clear();
        state.status = "running";
    }

    /// Sets the current main-thread phase and its expected task count, or zero if unknown.
    pub fn phase(&self, name: &'static str, total_tasks: usize) {
        let now = Instant::now();
        let mut state = self.lock();
        state.phase = name;
        state.status = "running";
        state.phase_started = now;
        state.total_tasks = total_tasks;
        state.completed_tasks = 0;
        state.active.clear();
    }

    /// Marks one unit of work active until its guard is dropped.
    pub fn task(&self, detail: String) -> TaskGuard {
        let mut state = self.lock();
        let id = state.next_task_id;
        state.next_task_id = state.next_task_id.wrapping_add(1);
        let now = Instant::now();
        state.active.insert(
            id,
            ActiveTask {
                detail,
                started: now,
                detail_started: now,
            },
        );
        TaskGuard {
            progress: self.clone(),
            id,
        }
    }

    /// Updates the completed rollout step count.
    pub fn steps(&self, completed: usize) {
        self.lock().completed_steps = completed;
    }

    /// Adds fresh and cache-reused labels to this iteration's totals.
    pub fn labels(&self, fresh: usize, reused: usize) {
        let mut state = self.lock();
        state.fresh_labels = state.fresh_labels.saturating_add(fresh);
        state.reused_labels = state.reused_labels.saturating_add(reused);
    }

    /// Returns a snapshot; elapsed durations are calculated here so no `Instant` is serialized.
    #[must_use]
    pub fn snapshot(&self) -> ProgressSnapshot {
        let now = Instant::now();
        let state = self.lock();
        let oldest = state.active.values().min_by_key(|task| task.started);
        ProgressSnapshot {
            environment: state
                .execution
                .as_ref()
                .map(|execution| execution.snapshot()),
            iteration: state.iteration,
            status: state.status,
            phase: state.phase,
            elapsed_seconds: seconds(now.saturating_duration_since(state.started)),
            phase_elapsed_seconds: seconds(now.saturating_duration_since(state.phase_started)),
            completed_steps: state.completed_steps,
            total_steps: state.total_steps,
            completed_tasks: state.completed_tasks,
            total_tasks: state.total_tasks,
            fresh_labels: state.fresh_labels,
            reused_labels: state.reused_labels,
            seconds_since_task_completion: state
                .last_task_completion
                .map(|instant| seconds(now.saturating_duration_since(instant))),
            last_task_detail: state.last_task_detail.clone(),
            oldest_active_task: oldest.map(|task| task.detail.clone()),
            oldest_active_task_seconds: oldest
                .map(|task| seconds(now.saturating_duration_since(task.started))),
            oldest_active_detail_seconds: oldest
                .map(|task| seconds(now.saturating_duration_since(task.detail_started))),
        }
    }

    /// Marks this run's current operation as complete, failed, or another caller-defined state.
    pub fn finish(&self, status: &'static str) {
        let mut state = self.lock();
        state.status = status;
        if status == "completed" {
            state.phase = "finished";
            state.phase_started = Instant::now();
            state.active.clear();
        }
    }
}

fn seconds(duration: Duration) -> f64 {
    duration.as_secs_f64()
}

pub struct TaskGuard {
    progress: TrainingProgress,
    id: u64,
}

impl TaskGuard {
    /// Updates the active substep without resetting the task's age or completion count.
    ///
    /// A guard from an earlier phase cannot alter the current phase's tasks.
    pub fn update_detail(&self, detail: String) {
        if let Some(task) = self.progress.lock().active.get_mut(&self.id)
            && task.detail != detail
        {
            task.detail = detail;
            task.detail_started = Instant::now();
        }
    }
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        let now = Instant::now();
        let mut state = self.progress.lock();
        if let Some(task) = state.active.remove(&self.id) {
            state.completed_tasks = state.completed_tasks.saturating_add(1);
            state.last_task_completion = Some(now);
            state.last_task_detail = Some(task.detail);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_task_completion_and_iteration_reset_are_reported() {
        let progress = TrainingProgress::default();
        progress.begin(4, 20);
        progress.phase("probe", 2);
        let task = progress.task("worker 3 tier 7".into());
        let active = progress.snapshot();
        assert_eq!(
            active.oldest_active_task.as_deref(),
            Some("worker 3 tier 7")
        );
        assert_eq!(active.completed_tasks, 0);
        drop(task);
        progress.steps(5);
        progress.labels(3, 2);
        let done = progress.snapshot();
        assert_eq!(done.completed_tasks, 1);
        assert_eq!(done.completed_steps, 5);
        assert_eq!((done.fresh_labels, done.reused_labels), (3, 2));
        assert!(done.seconds_since_task_completion.is_some());
        assert_eq!(done.last_task_detail.as_deref(), Some("worker 3 tier 7"));

        progress.begin(5, 8);
        let reset = progress.snapshot();
        assert_eq!(reset.iteration, 5);
        assert_eq!(reset.completed_steps, 0);
        assert_eq!(reset.completed_tasks, 0);
        assert_eq!((reset.fresh_labels, reset.reused_labels), (0, 0));
    }

    #[test]
    fn an_old_guard_cannot_count_work_in_a_new_phase() {
        let progress = TrainingProgress::default();
        progress.phase("old phase", 1);
        let old = progress.task("old task".into());
        progress.phase("new phase", 1);
        let new = progress.task("new task".into());
        old.update_detail("old task changed".into());
        drop(old);
        let pending = progress.snapshot();
        assert_eq!(pending.completed_tasks, 0);
        assert_eq!(pending.oldest_active_task.as_deref(), Some("new task"));
        drop(new);
        assert_eq!(progress.snapshot().completed_tasks, 1);
    }

    #[test]
    fn changing_a_substep_preserves_the_case_age_and_completes_only_once() {
        let progress = TrainingProgress::default();
        progress.phase("CPU evaluation", 1);
        let task = progress.task("case 1: initialization".into());
        let started = progress.lock().active[&task.id].started;
        {
            let mut state = progress.lock();
            state.active.get_mut(&task.id).unwrap().detail_started =
                started - Duration::from_secs(30);
        }
        task.update_detail("case 1: oracle step 2".into());
        let detail_started = progress.lock().active[&task.id].detail_started;
        assert_eq!(progress.lock().active[&task.id].started, started);
        assert!(detail_started > started - Duration::from_secs(30));
        task.update_detail("case 1: oracle step 2".into());
        assert_eq!(
            progress.lock().active[&task.id].detail_started,
            detail_started
        );
        let running = progress.snapshot();
        assert_eq!(running.completed_tasks, 0);
        assert!(running.oldest_active_detail_seconds.is_some());
        assert_eq!(
            running.oldest_active_task.as_deref(),
            Some("case 1: oracle step 2")
        );
        drop(task);
        let finished = progress.snapshot();
        assert_eq!(finished.completed_tasks, 1);
        assert_eq!(
            finished.last_task_detail.as_deref(),
            Some("case 1: oracle step 2")
        );
    }

    #[test]
    fn failure_keeps_the_operation_that_failed() {
        let progress = TrainingProgress::default();
        progress.phase("device forward", 1);
        let task = progress.task("forward for ten slots".into());
        drop(task);
        progress.finish("failed");
        let failed = progress.snapshot();
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.phase, "device forward");
        assert_eq!(
            failed.last_task_detail.as_deref(),
            Some("forward for ten slots")
        );
    }
}
