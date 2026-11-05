//! CLI observer for the runner's diagnostic progress handle.

use std::{
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{
    cli_console::Console,
    runner::progress::{ProgressSnapshot, TrainingProgress},
};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const REPORT_INTERVAL: Duration = Duration::from_secs(30);
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(10);

/// Periodically prints real runner state and writes a small sidecar JSON snapshot.
pub(super) struct LiveProgressObserver {
    stop: mpsc::Sender<()>,
    handle: Option<JoinHandle<()>>,
}

impl LiveProgressObserver {
    pub(super) fn start(
        progress: TrainingProgress,
        path: PathBuf,
        console: Console,
        total_iterations: usize,
    ) -> Self {
        let (stop, stopped) = mpsc::channel();
        let observer_console = console.clone();
        let handle = thread::Builder::new()
            .name("trainer-progress-observer".into())
            .spawn(move || {
                observe(
                    &progress,
                    &path,
                    &observer_console,
                    total_iterations,
                    &stopped,
                )
            })
            .ok();
        if handle.is_none() {
            console.warning("warning: could not start the live progress observer");
        }
        Self { stop, handle }
    }
}

impl Drop for LiveProgressObserver {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Sets failed status on all early returns and unwinding; callers mark success explicitly.
pub(super) struct ProgressStatusGuard(TrainingProgress, bool);

impl ProgressStatusGuard {
    pub(super) fn new(progress: TrainingProgress) -> Self {
        Self(progress, false)
    }

    pub(super) fn complete(&mut self) {
        self.0.finish("completed");
        self.1 = true;
    }
}

impl Drop for ProgressStatusGuard {
    fn drop(&mut self) {
        if !self.1 {
            self.0.finish("failed");
        }
    }
}

fn observe(
    progress: &TrainingProgress,
    path: &Path,
    console: &Console,
    total_iterations: usize,
    stopped: &mpsc::Receiver<()>,
) {
    let mut last_phase = None;
    let mut last_status = None;
    let mut last_report = Instant::now();
    let mut last_snapshot = Instant::now();
    let mut write_warned = false;
    let mut stop_requested = false;
    loop {
        let snapshot = progress.snapshot();
        let phase_key = (snapshot.iteration, snapshot.status, snapshot.phase);
        let phase_changed = last_phase != Some(phase_key);
        let status_key = (snapshot.iteration, snapshot.status);
        if display_due(
            console.is_live(),
            last_status,
            status_key,
            last_report.elapsed(),
            stop_requested,
        ) {
            console.progress(&format_progress(&snapshot, total_iterations));
            last_report = Instant::now();
            last_status = Some(status_key);
        }
        if phase_changed || last_snapshot.elapsed() >= SNAPSHOT_INTERVAL || stop_requested {
            if let Err(error) = write_snapshot(path, &snapshot)
                && !write_warned
            {
                console.warning(&format!(
                    "warning: could not write live progress {}: {error}",
                    path.display()
                ));
                write_warned = true;
            }
            last_phase = Some(phase_key);
            last_snapshot = Instant::now();
        }
        if stop_requested {
            console.finish();
            break;
        }
        match stopped.recv_timeout(POLL_INTERVAL) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => stop_requested = true,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn display_due(
    live: bool,
    previous: Option<(usize, &str)>,
    current: (usize, &str),
    since_report: Duration,
    stopping: bool,
) -> bool {
    live || stopping || previous != Some(current) || since_report >= REPORT_INTERVAL
}

fn write_snapshot(path: &Path, snapshot: &ProgressSnapshot) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut writer = BufWriter::new(temp.as_file_mut());
        serde_json::to_writer(&mut writer, snapshot)?;
        writer.flush()?;
    }
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn format_progress(snapshot: &ProgressSnapshot, total_iterations: usize) -> Vec<String> {
    let task_total = if snapshot.total_tasks == 0 {
        "unknown".to_owned()
    } else {
        snapshot.total_tasks.to_string()
    };
    let rollout = if snapshot.total_steps == 0 {
        "rollout: none expected during this phase".to_owned()
    } else {
        let eta = rollout_eta(snapshot).map_or_else(String::new, |seconds| {
            format!(" | rollout ETA ~{} (estimate)", human_time(seconds))
        });
        format!(
            "rollout {}/{} ({:.1}%){eta}",
            snapshot.completed_steps,
            snapshot.total_steps,
            100.0 * snapshot.completed_steps as f64 / snapshot.total_steps as f64
        )
    };
    let task_kind = if snapshot.phase.starts_with("validation:") {
        "cases"
    } else {
        "tasks"
    };
    let mut rows = vec![
        format!(
            "iter {}/{} [{}] | iteration elapsed {}",
            snapshot.iteration,
            total_iterations,
            snapshot.status,
            human_time(snapshot.elapsed_seconds)
        ),
        rollout,
        format!(
            "{} | {task_kind} {}/{task_total} | phase {}",
            snapshot.phase,
            snapshot.completed_tasks,
            human_time(snapshot.phase_elapsed_seconds)
        ),
    ];
    match (
        snapshot.oldest_active_task.as_deref(),
        snapshot.oldest_active_task_seconds,
    ) {
        (Some(detail), Some(seconds)) => {
            let detail_age = snapshot
                .oldest_active_detail_seconds
                .map_or_else(|| "unknown".to_owned(), human_time);
            rows.push(format!(
                "active {detail} | detail age {detail_age} | task age {}",
                human_time(seconds)
            ));
        }
        _ => rows.push(snapshot.seconds_since_task_completion.map_or_else(
            || "active task: none reported".to_owned(),
            |seconds| format!("last task completed {} ago", human_time(seconds)),
        )),
    }
    if snapshot.total_steps > 0 {
        rows.push(format!(
            "label requests: {} fresh / {} cached",
            snapshot.fresh_labels, snapshot.reused_labels
        ));
    }
    if let Some(environment) = &snapshot.environment {
        rows.push(format!(
            "CUDA: {} optical scores | {} solid arrangements | {} projection probes",
            environment.cuda_optical_requests,
            environment.solid.cuda_arrangements,
            environment.geometry.cuda_probes,
        ));
        rows.push(format!(
            "CUDA meet proofs: {} | triples {} | capacity bypasses {}",
            environment.geometry.arrangement_probes,
            environment.geometry.arrangement_triples,
            environment.geometry.arrangement_capacity_bypasses,
        ));
        rows.push(format!(
            "Proof service: {:.1}s cumulative call wall; overlaps, not kernel time",
            environment.geometry.arrangement_service_ms / 1_000.0,
        ));
        rows.push(format!(
            "CPU preservation: {} checks, {:.1}s cumulative wall time | {} cached certificates",
            environment.geometry.cpu_gate_calls,
            environment.geometry.cpu_gate_ms / 1_000.0,
            environment.geometry.certificate_cache_hits,
        ));
    }
    rows
}

/// A coarse rollout-only estimate; early samples and non-rollout phases are not predictive.
fn rollout_eta(snapshot: &ProgressSnapshot) -> Option<f64> {
    if snapshot.status != "running"
        || !matches!(
            snapshot.phase,
            "teacher probes"
                | "oracle-and-steps"
                | "device forward"
                | "respawn"
                | "serial respawn retry"
        )
        || snapshot.completed_steps < 128
        || snapshot.completed_steps >= snapshot.total_steps
        || snapshot.completed_steps as f64 / (snapshot.total_steps as f64) < 0.05
        || !snapshot.elapsed_seconds.is_finite()
        || snapshot.elapsed_seconds < 30.0
    {
        return None;
    }
    Some(
        snapshot.elapsed_seconds * (snapshot.total_steps - snapshot.completed_steps) as f64
            / snapshot.completed_steps as f64,
    )
}

fn human_time(seconds: f64) -> String {
    let Ok(duration) = Duration::try_from_secs_f64(seconds) else {
        return "unknown".to_owned();
    };
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!(
            "{}h {:02}m {:02}s",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_output_waits_thirty_seconds_but_status_and_stop_are_immediate() {
        let status = (1, "running");
        assert!(!display_due(
            false,
            Some(status),
            status,
            Duration::from_secs(29),
            false
        ));
        assert!(display_due(
            false,
            Some(status),
            status,
            REPORT_INTERVAL,
            false
        ));
        assert!(display_due(
            false,
            Some(status),
            (2, "running"),
            Duration::ZERO,
            false
        ));
        assert!(display_due(
            false,
            Some(status),
            (1, "iteration complete"),
            Duration::ZERO,
            false
        ));
        assert!(display_due(
            false,
            Some(status),
            status,
            Duration::ZERO,
            true
        ));
    }

    #[test]
    fn eta_needs_measured_rollout_progress_and_never_means_iteration_completion() {
        let progress = TrainingProgress::default();
        progress.begin(1, 4096);
        progress.phase("teacher probes", 512);
        let mut snapshot = progress.snapshot();
        snapshot.elapsed_seconds = 120.0;
        assert_eq!(rollout_eta(&snapshot), None);
        snapshot.completed_steps = 256;
        assert_eq!(rollout_eta(&snapshot), Some(1800.0));
        snapshot.phase = "validation";
        assert_eq!(rollout_eta(&snapshot), None);
        snapshot.phase = "teacher probes";
        snapshot.completed_steps = 4096;
        assert_eq!(rollout_eta(&snapshot), None);
    }

    #[test]
    fn elapsed_times_are_readable_for_long_rollouts() {
        assert_eq!(human_time(0.0), "0s");
        assert_eq!(human_time(125.0), "2m 05s");
        assert_eq!(human_time(7165.0), "1h 59m 25s");
        assert_eq!(human_time(f64::NAN), "unknown");
    }

    #[test]
    fn resumed_runs_keep_absolute_iteration_and_show_substep_tasks() {
        let progress = TrainingProgress::default();
        progress.begin(101, 4096);
        progress.phase("teacher probes", 512);
        let task = progress.task("slot 7 entry 2 tier 4".into());
        let rows = format_progress(&progress.snapshot(), 300).join(" | ");
        assert!(rows.contains("iter 101/300"));
        assert!(rows.contains("rollout 0/4096 (0.0%)"));
        assert!(rows.contains("teacher probes | tasks 0/512"));
        assert!(rows.contains("active slot 7 entry 2 tier 4 | detail age"));
        assert!(!rows.contains("ETA"));
        drop(task);
        assert!(
            format_progress(&progress.snapshot(), 300)
                .join(" | ")
                .contains("tasks 1/512")
        );
    }

    #[test]
    fn resumed_validation_shows_cases_and_proof_work_without_an_expected_rollout() {
        let progress = TrainingProgress::default();
        progress.begin(5, 0);
        progress.phase("validation: CUDA policy/physics; host search control", 112);
        let mut snapshot = progress.snapshot();
        snapshot.completed_tasks = 2;
        snapshot.environment = Some(pleochroma_ai::env::ExecutionSnapshot {
            revision: pleochroma_ai::env::CUDA_ENVIRONMENT_MATH,
            cuda_optical_requests: 211,
            cpu_optical_fallbacks: 0,
            last_fallback: None,
            failure: None,
            optics: Default::default(),
            geometry: pleochroma_ai::gpu_geometry::GeometryDiagnostics {
                arrangement_probes: 321_030,
                arrangement_triples: 254_506_874_708,
                arrangement_capacity_bypasses: 7,
                arrangement_service_ms: 18_528_000.0,
                ..Default::default()
            },
            solid: Default::default(),
        });
        let rows = format_progress(&snapshot, 300).join(" | ");
        assert!(rows.contains("iter 5/300"));
        assert!(rows.contains("rollout: none expected during this phase"));
        assert!(
            rows.contains("validation: CUDA policy/physics; host search control | cases 2/112")
        );
        assert!(
            rows.contains("CUDA meet proofs: 321030 | triples 254506874708 | capacity bypasses 7")
        );
        assert!(rows.contains("18528.0s cumulative call wall; overlaps, not kernel time"));
        assert!(!rows.contains("rollout 0/unknown"));
        assert!(!rows.contains("ETA"));
    }
}
