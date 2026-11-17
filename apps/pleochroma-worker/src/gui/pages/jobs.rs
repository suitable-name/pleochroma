//! The Jobs page.
//!
//! Fills `JobsState` from the coordinator status once a second and answers its callbacks (filter,
//! Cancel, Delete, Open jobs folder) through the
//! [`CoordinatorBackend`](crate::control::backend::CoordinatorBackend).
//!
//! The status carries every job with its live state and progress, so the one-second refresh
//! is all the "live" the page needs; rows are updated in place so an open confirmation
//! survives. Times are relative ("12 min ago"), so no time zone is needed.

use crate::{
    control::{
        backend::{CoordinatorBackend, JobId},
        status::unix_now,
    },
    gui::app::{ActiveBackend, AppWindow, JobRowData, JobsState, ShellState},
};
use pleochroma_net::admin::status::{JobState, JobStatus, JobsSummary};
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::{cell::RefCell, rc::Rc, time::Duration};

/// How often the page re-reads the status.
const REFRESH: Duration = Duration::from_secs(1);

/// `JobsState.filter` value: running jobs only (0 is All).
const FILTER_RUNNING: i32 = 1;
/// Queued jobs only.
const FILTER_QUEUED: i32 = 2;
/// Finished jobs only.
const FILTER_DONE: i32 = 3;
/// Failed jobs only.
const FILTER_FAILED: i32 = 4;

thread_local! {
    /// The refresh timer; a `Timer` stops when dropped, so it lives for the UI thread.
    static REFRESH_TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
}

/// Connects the Jobs page of `window` to `backend`: shows the current jobs at once, refreshes
/// them every second, and wires the filter, Cancel, Delete and Open jobs folder.
pub fn wire(window: &AppWindow, backend: &ActiveBackend) {
    let state = window.global::<JobsState>();
    // The jobs matching the filter, and the running jobs.
    let jobs = Rc::new(VecModel::<JobRowData>::default());
    let running = Rc::new(VecModel::<JobRowData>::default());
    state.set_jobs(ModelRc::from(Rc::clone(&jobs)));
    state.set_running(ModelRc::from(Rc::clone(&running)));

    state.on_filter_changed({
        let weak = window.as_weak();
        let backend = backend.clone();
        let (jobs, running) = (Rc::clone(&jobs), Rc::clone(&running));
        move |filter| {
            if let Some(window) = weak.upgrade() {
                window.global::<JobsState>().set_filter(filter);
                refresh(&window, &backend, &jobs, &running);
            }
        }
    });
    state.on_cancel({
        let weak = window.as_weak();
        let backend = backend.clone();
        move |id| match parse_id(&id) {
            Ok(id) => run_action(&weak, &backend, move |b| b.cancel_job(id)),
            Err(reason) => show_result(&weak, &backend, Err(reason)),
        }
    });
    state.on_delete({
        let weak = window.as_weak();
        let backend = backend.clone();
        move |id| match parse_id(&id) {
            Ok(id) => run_action(&weak, &backend, move |b| b.delete_job(id)),
            Err(reason) => show_result(&weak, &backend, Err(reason)),
        }
    });
    state.on_open_folder({
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            // The folder is on the coordinator's own computer: a remote one is not opened here.
            let remote = weak
                .upgrade()
                .is_none_or(|window| window.global::<ShellState>().get_remote());
            if remote {
                return;
            }
            let folder = backend.get().status().jobs_summary.folder;
            if !folder.is_empty() {
                let dir = std::path::Path::new(&folder);
                crate::gui::util::open_folder(dir);
            }
        }
    });

    refresh(window, backend, &jobs, &running);
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, REFRESH, {
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                refresh(&window, &backend, &jobs, &running);
            }
        }
    });
    REFRESH_TIMER.with(|slot| *slot.borrow_mut() = Some(timer));
}

/// Runs `action` on the coordinator that is shown, on a helper thread (a remote coordinator
/// can take seconds to answer), and shows the outcome on the UI thread.
fn run_action<F>(weak: &slint::Weak<AppWindow>, backend: &ActiveBackend, action: F)
where
    F: FnOnce(&dyn CoordinatorBackend) -> Result<(), String> + Send + 'static,
{
    let shown = backend.get();
    let active = backend.clone();
    let for_thread = weak.clone();
    let spawned = std::thread::Builder::new()
        .name("jobs-command".to_owned())
        .spawn(move || {
            let result = action(shown.as_ref());
            let _ = for_thread.upgrade_in_event_loop(move |window| {
                finish_action(&window, &active, result);
            });
        });
    if let Err(e) = spawned {
        let reason = format!("Could not start the work: {e}");
        show_result(weak, backend, Err(reason));
    }
}

/// Shows an outcome that is already known, on the UI thread.
fn show_result(weak: &slint::Weak<AppWindow>, backend: &ActiveBackend, result: Result<(), String>) {
    if let Some(window) = weak.upgrade() {
        finish_action(&window, backend, result);
    }
}

/// Shows the outcome of Cancel or Delete: an error in plain words, or none, and the new list
/// at once.
fn finish_action(window: &AppWindow, backend: &ActiveBackend, result: Result<(), String>) {
    let state = window.global::<JobsState>();
    state.set_error(SharedString::from(result.err().unwrap_or_default()));
    // The row models are the ones `wire` put into the page.
    let (jobs, running) = (state.get_jobs(), state.get_running());
    if let (Some(jobs), Some(running)) = (
        jobs.as_any().downcast_ref::<VecModel<JobRowData>>(),
        running.as_any().downcast_ref::<VecModel<JobRowData>>(),
    ) {
        refresh(window, backend, jobs, running);
    }
}

/// Parses the 32-character hex job id a row carries.
fn parse_id(hex: &str) -> Result<JobId, String> {
    JobId::from_hex(hex).ok_or_else(|| "That job's id is not valid.".to_string())
}

/// Reads the status and permissions and updates the page state and rows.
fn refresh(
    window: &AppWindow,
    backend: &ActiveBackend,
    jobs: &VecModel<JobRowData>,
    running_jobs: &VecModel<JobRowData>,
) {
    let backend = backend.get();
    let status = backend.status();
    let caps = backend.capabilities();
    let now = unix_now();

    let state = window.global::<JobsState>();
    let filter = state.get_filter();
    let ordered = ordered_jobs(&status.jobs);
    let count = |keep: fn(JobState) -> bool| {
        i32::try_from(ordered.iter().filter(|job| keep(job.state)).count()).unwrap_or(i32::MAX)
    };
    state.set_count_all(i32::try_from(ordered.len()).unwrap_or(i32::MAX));
    state.set_count_running(count(|s| s == JobState::Running));
    state.set_count_queued(count(|s| s == JobState::Queued));
    state.set_count_done(count(|s| s == JobState::Done));
    state.set_count_failed(count(|s| s == JobState::Failed));

    let rows: Vec<JobRowData> = ordered
        .iter()
        .filter(|job| matches_filter(filter, job.state))
        .map(|job| row_for(job, now))
        .collect();
    let running: Vec<JobRowData> = ordered
        .iter()
        .filter(|job| job.state == JobState::Running)
        .map(|job| row_for(job, now))
        .collect();

    state.set_storage_text(storage_text(&status.jobs_summary).into());
    state.set_has_folder(!status.jobs_summary.folder.is_empty());
    state.set_can_cancel(caps.cancel_jobs);
    state.set_can_delete(caps.delete_jobs);
    state.set_denied_reason(if caps.cancel_jobs && caps.delete_jobs {
        SharedString::new()
    } else {
        caps.denied_reason().into()
    });
    sync_rows(jobs, rows);
    sync_rows(running_jobs, running);
}

/// Brings `model` to `rows` without resetting it when only row contents changed, so an open
/// confirmation survives the refresh.
fn sync_rows(model: &VecModel<JobRowData>, rows: Vec<JobRowData>) {
    let same_jobs = model.row_count() == rows.len()
        && rows
            .iter()
            .enumerate()
            .all(|(i, row)| model.row_data(i).is_some_and(|old| old.id == row.id));
    if same_jobs {
        for (i, row) in rows.into_iter().enumerate() {
            if model.row_data(i).as_ref() != Some(&row) {
                model.set_row_data(i, row);
            }
        }
    } else {
        model.set_vec(rows);
    }
}

/// The jobs in display order: running, then queued (oldest first, the queue order), then the
/// finished ones, newest first.
fn ordered_jobs(jobs: &[JobStatus]) -> Vec<&JobStatus> {
    let rank = |state: JobState| match state {
        JobState::Running => 0,
        JobState::Queued => 1,
        JobState::Done | JobState::Failed | JobState::Cancelled => 2,
    };
    let mut ordered: Vec<&JobStatus> = jobs.iter().collect();
    ordered.sort_by(|a, b| {
        rank(a.state).cmp(&rank(b.state)).then_with(|| {
            if a.state.is_terminal() {
                b.created.cmp(&a.created)
            } else {
                a.created.cmp(&b.created)
            }
        })
    });
    ordered
}

/// Whether a job in `state` belongs under `filter`. Cancelled jobs show only under All.
fn matches_filter(filter: i32, state: JobState) -> bool {
    match filter {
        FILTER_RUNNING => state == JobState::Running,
        FILTER_QUEUED => state == JobState::Queued,
        FILTER_DONE => state == JobState::Done,
        FILTER_FAILED => state == JobState::Failed,
        _ => true,
    }
}

/// One table row for `job`.
fn row_for(job: &JobStatus, now: u64) -> JobRowData {
    let label = if job.label.is_empty() {
        "Untitled render".to_string()
    } else {
        job.label.clone()
    };
    let note_shown =
        matches!(job.state, JobState::Failed | JobState::Cancelled) && !job.note.is_empty();
    let detail = if note_shown {
        job.note.clone()
    } else {
        job.design_name.clone()
    };
    let (state, state_text) = match job.state {
        JobState::Queued => (0, "Queued"),
        JobState::Running => (1, "Running"),
        JobState::Done => (2, "Done"),
        JobState::Failed => (3, "Failed"),
        JobState::Cancelled => (4, "Cancelled"),
    };
    let progress = if job.state == JobState::Done {
        1.0
    } else {
        job.progress.clamp(0.0, 1.0)
    };
    let progress_text = if job.state == JobState::Queued {
        "waiting".to_string()
    } else {
        format!("{}%", percent(progress))
    };
    JobRowData {
        id: job.id.to_hex().into(),
        owner: job.owner.clone().into(),
        label: label.into(),
        detail: detail.into(),
        detail_bad: note_shown && job.state == JobState::Failed,
        size: size_text(job.width, job.height).into(),
        samples: samples_text(job.samples_done, job.samples_total).into(),
        state,
        state_text: state_text.into(),
        progress,
        progress_text: progress_text.into(),
        created: ago_text(now, job.created).into(),
        finished: match job.finished {
            Some(at) if job.state.is_terminal() => {
                format!("Finished {}", ago_text(now, at).to_lowercase()).into()
            }
            _ => SharedString::new(),
        },
        route: job.route.clone().into(),
        can_cancel: matches!(job.state, JobState::Queued | JobState::Running),
        can_delete: job.state.is_terminal(),
    }
}

/// A fraction as a whole percent, 0 to 100.
fn percent(fraction: f32) -> u32 {
    // Truncation is the intent: 99.9 % must not show as a finished 100 %.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let whole = (fraction.clamp(0.0, 1.0) * 100.0) as u32;
    whole
}

/// "4096 × 2304", or empty when the size is not known.
fn size_text(width: u32, height: u32) -> String {
    if width == 0 || height == 0 {
        String::new()
    } else {
        format!("{width} \u{d7} {height}")
    }
}

/// "645 / 1024" samples per pixel.
fn samples_text(done: u32, total: u32) -> String {
    format!("{done} / {total}")
}

/// A past moment in plain words: "just now", "12 min ago", "3 h ago", "Yesterday" or
/// "4 days ago".
fn ago_text(now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..=59 => "Just now".to_string(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => format!("{} h ago", seconds / 3600),
        86_400..=172_799 => "Yesterday".to_string(),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

/// A byte count in plain units: "512 KB", "412 MB", "1.5 GB".
fn bytes_text(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    // Display only: the loss of precision above 2^53 bytes does not matter.
    #[allow(clippy::cast_precision_loss)]
    let gigabytes = bytes as f64 / GB as f64;
    if bytes < KB {
        format!("{bytes} B")
    } else if bytes < MB {
        format!("{} KB", bytes / KB)
    } else if bytes < GB {
        format!("{} MB", bytes / MB)
    } else if gigabytes < 10.0 {
        format!("{gigabytes:.1} GB")
    } else {
        format!("{gigabytes:.0} GB")
    }
}

/// The storage line: "Results are kept 7 days · 4 of 256 jobs · 412 MB of 8 GB".
fn storage_text(summary: &JobsSummary) -> String {
    let kept = match summary.retention_days {
        Some(1) => "Results are kept 1 day".to_string(),
        Some(days) => format!("Results are kept {days} days"),
        None => "Results are kept until you delete them".to_string(),
    };
    let jobs = if summary.max_jobs > 0 {
        format!("{} of {} jobs", summary.stored, summary.max_jobs)
    } else if summary.stored == 1 {
        "1 job".to_string()
    } else {
        format!("{} jobs", summary.stored)
    };
    let used = summary.limit_bytes.map_or_else(
        || format!("{} used", bytes_text(summary.used_bytes)),
        |limit| {
            format!(
                "{} of {}",
                bytes_text(summary.used_bytes),
                bytes_text(limit)
            )
        },
    );
    format!("{kept} \u{b7} {jobs} \u{b7} {used}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(state: JobState, created: u64) -> JobStatus {
        JobStatus {
            id: JobId([created.to_le_bytes()[0]; 16]),
            owner: "Niklas's S23".to_string(),
            owner_fingerprint: String::new(),
            state,
            progress: 0.41,
            samples_done: 840,
            samples_total: 2048,
            width: 4096,
            height: 2304,
            label: "Final render".to_string(),
            design_name: "Ring".to_string(),
            route: String::new(),
            created,
            finished: None,
            result_len: None,
            note: String::new(),
        }
    }

    #[test]
    fn jobs_are_ordered_running_queued_then_newest_finished() {
        let jobs = vec![
            job(JobState::Done, 10),
            job(JobState::Queued, 30),
            job(JobState::Running, 20),
            job(JobState::Failed, 40),
            job(JobState::Queued, 25),
        ];
        let order: Vec<u64> = ordered_jobs(&jobs).iter().map(|j| j.created).collect();
        assert_eq!(order, vec![20, 25, 30, 40, 10]);
    }

    #[test]
    fn filters_pick_their_states() {
        assert!(matches_filter(0, JobState::Cancelled));
        assert!(matches_filter(FILTER_RUNNING, JobState::Running));
        assert!(!matches_filter(FILTER_RUNNING, JobState::Queued));
        assert!(matches_filter(FILTER_QUEUED, JobState::Queued));
        assert!(matches_filter(FILTER_DONE, JobState::Done));
        assert!(matches_filter(FILTER_FAILED, JobState::Failed));
        assert!(!matches_filter(FILTER_FAILED, JobState::Cancelled));
    }

    #[test]
    fn rows_say_what_can_be_done() {
        let running = row_for(&job(JobState::Running, 5), 100);
        assert!(running.can_cancel && !running.can_delete);
        assert_eq!(running.progress_text.as_str(), "41%");
        assert_eq!(running.size.as_str(), "4096 \u{d7} 2304");
        assert_eq!(running.samples.as_str(), "840 / 2048");

        let queued = row_for(&job(JobState::Queued, 5), 100);
        assert!(queued.can_cancel);
        assert_eq!(queued.progress_text.as_str(), "waiting");

        let mut failed = job(JobState::Failed, 5);
        failed.note = "The graphics card ran out of memory".to_string();
        failed.finished = Some(90);
        let failed = row_for(&failed, 100);
        assert!(failed.can_delete && !failed.can_cancel && failed.detail_bad);
        assert_eq!(
            failed.detail.as_str(),
            "The graphics card ran out of memory"
        );
        assert_eq!(failed.finished.as_str(), "Finished just now");

        let done = row_for(&job(JobState::Done, 5), 100);
        assert_eq!(done.progress_text.as_str(), "100%");
        assert_eq!(done.detail.as_str(), "Ring");
    }

    #[test]
    fn times_are_relative() {
        assert_eq!(ago_text(1_000, 990), "Just now");
        assert_eq!(ago_text(1_000, 1_000 - 12 * 60), "12 min ago");
        assert_eq!(ago_text(100_000, 100_000 - 3 * 3600), "3 h ago");
        assert_eq!(ago_text(200_000, 200_000 - 30 * 3600), "Yesterday");
        assert_eq!(ago_text(1_000_000, 1_000_000 - 4 * 86_400), "4 days ago");
    }

    #[test]
    fn storage_line_reads_in_plain_words() {
        let summary = JobsSummary {
            retention_days: Some(7),
            stored: 4,
            max_jobs: 256,
            used_bytes: 412 * 1024 * 1024,
            limit_bytes: Some(8 * 1024 * 1024 * 1024),
            folder: String::new(),
        };
        assert_eq!(
            storage_text(&summary),
            "Results are kept 7 days \u{b7} 4 of 256 jobs \u{b7} 412 MB of 8.0 GB"
        );
        let open_ended = JobsSummary {
            retention_days: None,
            stored: 1,
            max_jobs: 0,
            used_bytes: 2048,
            limit_bytes: None,
            folder: String::new(),
        };
        assert_eq!(
            storage_text(&open_ended),
            "Results are kept until you delete them \u{b7} 1 job \u{b7} 2 KB used"
        );
    }

    #[test]
    fn a_bad_id_is_an_error_not_a_panic() {
        assert!(parse_id("abcd").is_err());
        assert!(parse_id(&"ab".repeat(16)).is_ok());
    }
}
