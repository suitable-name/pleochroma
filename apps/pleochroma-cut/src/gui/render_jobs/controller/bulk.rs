//! The Jobs window's Compute menu: sets where the waiting, paused and failed jobs compute.
//!
//! It stores the choice only and never changes a job's state: a failed job stays failed until
//! the person resumes it. Each row is written through the vault's guarded update, which
//! writes only while the row still has the state and `frames_done` it was read with. A
//! Ray-Traced job stays on this computer, and the toast counts what was skipped.

use super::super::rows::compute_words;
use pleochroma_render_jobs::{ComputeChoice, codec, edit::JobEdit, state::JobState};
use pleochroma_vault::{
    db::sqlite::{Database, ExpectedRow, UpdatedJob},
    model::render_job::RenderJobMeta,
};

/// What a bulk change did, job by job.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BulkOutcome {
    /// Jobs that were written with the new choice.
    pub written: Vec<i64>,
    /// Jobs that already had the choice (nothing to write).
    pub already: usize,
    /// Ray-Traced jobs left on this computer.
    pub ray_local: usize,
    /// Jobs skipped because they started, or changed since they were read.
    pub had_started: usize,
    /// Jobs the write failed for.
    pub refused: usize,
}

impl BulkOutcome {
    /// The number of jobs that now have the choice.
    #[must_use]
    pub const fn set(&self) -> usize {
        self.written.len() + self.already
    }
}

/// How one job went.
enum Stored {
    Written,
    Already,
    RayLocal,
    Raced,
}

/// The compute choice of the Slint menu: 0 Local Only, 1 Remote Only, 2 Local + Remote.
#[must_use]
pub const fn choice_of_index(index: i32) -> Option<ComputeChoice> {
    match index {
        0 => Some(ComputeChoice::Local),
        1 => Some(ComputeChoice::Remote),
        2 => Some(ComputeChoice::Both),
        _ => None,
    }
}

/// Whether a job in `state` is covered by the bulk change: waiting, paused or failed.
const fn covered(state: JobState) -> bool {
    matches!(
        state,
        JobState::Queued | JobState::Paused | JobState::Failed
    )
}

/// Stores `choice` in job `meta.job_id`. `Err` carries a sentence for the log.
fn store_compute(
    db: &Database,
    meta: &RenderJobMeta,
    choice: ComputeChoice,
) -> Result<Stored, String> {
    let row = db
        .get_render_job(meta.job_id)
        .map_err(|error| error.to_string())?;
    let Some(row) = row else {
        // Deleted since the list was read.
        return Ok(Stored::Raced);
    };
    let job = codec::from_text(&row.snapshot).map_err(|error| error.to_string())?;
    if job.engine.is_ray() && choice != ComputeChoice::Local {
        return Ok(Stored::RayLocal);
    }
    if job.compute.target == choice {
        return Ok(Stored::Already);
    }
    let edit = JobEdit {
        target: Some(choice),
        ..JobEdit::default()
    };
    let edited = edit.apply(&job).map_err(|refusal| format!("{refusal:?}"))?;
    let snapshot = codec::to_text(&edited).map_err(|error| error.to_string())?;
    // The row read above carries the state and frames the guard compares.
    let expected = ExpectedRow {
        state: row.meta.state.clone(),
        frames_done: i64::from(row.meta.frames_done),
    };
    let updated = UpdatedJob {
        label: row.meta.label.clone(),
        summary: row.meta.summary.clone(),
        output_path: row.meta.output_path.clone(),
        frames_total: i64::from(row.meta.frames_total),
        snapshot_version: i64::from(edited.format),
        snapshot,
        new_state: None,
        reset_progress: false,
    };
    let wrote = db
        .update_render_job(meta.job_id, &expected, &updated)
        .map_err(|error| error.to_string())?;
    Ok(if wrote {
        Stored::Written
    } else {
        Stored::Raced
    })
}

/// Sets `choice` on every waiting, paused and failed job of `metas`. `running` is the job that
/// renders now, if any; it is never touched.
pub fn apply_bulk_compute(
    db: &Database,
    metas: &[RenderJobMeta],
    choice: ComputeChoice,
    running: Option<i64>,
) -> BulkOutcome {
    let mut outcome = BulkOutcome::default();
    for meta in metas {
        if !JobState::parse(&meta.state).is_some_and(covered) {
            continue;
        }
        if running == Some(meta.job_id) {
            outcome.had_started += 1;
            continue;
        }
        match store_compute(db, meta, choice) {
            Ok(Stored::Written) => outcome.written.push(meta.job_id),
            Ok(Stored::Already) => outcome.already += 1,
            Ok(Stored::RayLocal) => outcome.ray_local += 1,
            Ok(Stored::Raced) => outcome.had_started += 1,
            Err(error) => {
                tracing::warn!(
                    "The compute choice of render job {} was not stored: {error}",
                    meta.job_id
                );
                outcome.refused += 1;
            }
        }
    }
    outcome
}

/// "1 job" or "5 jobs".
fn jobs(count: usize) -> String {
    if count == 1 {
        "1 job".to_string()
    } else {
        format!("{count} jobs")
    }
}

/// The toast of a bulk change: what was set, and every count that was skipped.
#[must_use]
pub fn bulk_message(choice: ComputeChoice, outcome: &BulkOutcome) -> String {
    let mut parts = Vec::new();
    if outcome.set() > 0 {
        parts.push(format!(
            "{} set to {}",
            jobs(outcome.set()),
            compute_words(choice)
        ));
    }
    match outcome.ray_local {
        0 => {}
        1 => parts.push("1 Ray-Traced job stays local".to_string()),
        n => parts.push(format!("{n} Ray-Traced jobs stay local")),
    }
    match outcome.had_started {
        0 => {}
        1 => parts.push("1 job had started".to_string()),
        n => parts.push(format!("{n} jobs had started")),
    }
    match outcome.refused {
        0 => {}
        1 => parts.push("1 job could not be changed".to_string()),
        n => parts.push(format!("{n} jobs could not be changed")),
    }
    if parts.is_empty() {
        return "There are no waiting, paused or failed jobs.".to_string();
    }
    format!("{}.", parts.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_index_names_a_choice() {
        assert_eq!(choice_of_index(0), Some(ComputeChoice::Local));
        assert_eq!(choice_of_index(1), Some(ComputeChoice::Remote));
        assert_eq!(choice_of_index(2), Some(ComputeChoice::Both));
        assert_eq!(choice_of_index(3), None);
        assert_eq!(choice_of_index(-1), None);
    }

    #[test]
    fn the_toast_counts_what_was_set_and_what_was_skipped() {
        let outcome = BulkOutcome {
            written: vec![1, 2, 3, 4],
            already: 1,
            ray_local: 1,
            had_started: 1,
            refused: 0,
        };
        assert_eq!(
            bulk_message(ComputeChoice::Both, &outcome),
            "5 jobs set to Local + Remote; 1 Ray-Traced job stays local; 1 job had started."
        );
        let plural = BulkOutcome {
            written: vec![1],
            already: 0,
            ray_local: 2,
            had_started: 3,
            refused: 2,
        };
        assert_eq!(
            bulk_message(ComputeChoice::Remote, &plural),
            "1 job set to Remote Only; 2 Ray-Traced jobs stay local; 3 jobs had started; \
             2 jobs could not be changed."
        );
        assert_eq!(
            bulk_message(ComputeChoice::Local, &BulkOutcome::default()),
            "There are no waiting, paused or failed jobs."
        );
    }
}
