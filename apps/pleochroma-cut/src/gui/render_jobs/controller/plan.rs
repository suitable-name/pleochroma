//! The queue's pure decisions: what a finished run leaves behind, and which job runs next.
//! Free functions, so they are tested without a window.

use pleochroma_render_jobs::{
    JobOutcome,
    order::{QueueEntry, next_to_run},
    state::{APP_CLOSING_NOTE, JobState, StopReason, queue_continues_after, state_after_run},
};
use pleochroma_vault::model::render_job::RenderJobMeta;

/// What a finished run leaves behind, and whether the queue goes on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishPlan {
    /// The state to write.
    pub new_state: JobState,
    /// Frames finished (all of them after a finished run).
    pub frames_done: u32,
    /// What was written, for a finished run.
    pub result_path: Option<String>,
    /// The failure sentence, or the note a closing app leaves.
    pub error_text: Option<String>,
    /// Whether the queue tries to start the next job.
    pub start_next: bool,
    /// Whether the queue is still running afterwards (before the next job is looked for: a
    /// queue with nothing left stops itself).
    pub queue_running: bool,
}

/// What to write and do after a run ended with `outcome`, stopped for `stop` (the reason
/// the person asked for it, if any), on a queue that was `queue_running`.
pub fn after_finish(
    outcome: &JobOutcome,
    stop: Option<StopReason>,
    queue_running: bool,
    frames_total: u32,
) -> FinishPlan {
    let new_state = state_after_run(outcome.end(), stop);
    let error_text = match outcome {
        JobOutcome::Failed { message, .. } => Some(message.clone()),
        _ if new_state == JobState::Paused && stop == Some(StopReason::AppClosing) => {
            Some(APP_CLOSING_NOTE.to_string())
        }
        _ => None,
    };
    let result_path = match outcome {
        JobOutcome::Done { result, .. } => Some(result.to_string_lossy().into_owned()),
        _ => None,
    };
    let keeps_going = queue_running && queue_continues_after(stop);
    FinishPlan {
        new_state,
        frames_done: outcome.frames_done(frames_total),
        result_path,
        error_text,
        start_next: keeps_going,
        queue_running: keeps_going,
    }
}

/// The job a queue run takes next: the first waiting one in list order. Rows with a state
/// word this version does not know are skipped.
pub fn next_job(metas: &[RenderJobMeta]) -> Option<i64> {
    let entries: Vec<QueueEntry> = metas
        .iter()
        .filter_map(|meta| {
            JobState::parse(&meta.state).map(|state| QueueEntry {
                id: meta.job_id,
                state,
            })
        })
        .collect();
    next_to_run(&entries)
}
