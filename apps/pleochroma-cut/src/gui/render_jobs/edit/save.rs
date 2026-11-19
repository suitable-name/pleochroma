//! Saving an edited render job: the Save table, the optimistic guard, the label rule, and the one path that inserts a new job.
//!
//! Every function here runs on the UI thread. The token of a job never changes on a save; a new
//! job (Duplicate, Save as New Job, a copy) always gets a fresh one.

use super::{
    load::load_job_for_edit,
    naming::{JobKindTag, automatic_label, reserve_in},
};
use crate::{
    MainWindow,
    bridge::render_thread::RenderContext,
    gui::render_jobs::{
        capture_video::{VideoFacts, video_summary},
        controller::{is_running, job_changed, unix_now},
        convert,
    },
    settings::SettingsPersister,
};
use pleochroma::render_setup::StonePose;
use pleochroma_render_jobs::{
    JobKind, RenderJobFile, codec,
    edit::{EditMode, JobEdit, edit_rule, label_after_edit},
    job::{format_for, new_job_token},
    state::JobState,
};
use pleochroma_vault::{
    db::sqlite::{Database, ExpectedRow, UpdatedJob},
    model::render_job::{NewRenderJob, RenderJobMeta},
};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

/// The shared handles the edit code needs: the job list, the render context and the settings.
#[derive(Clone)]
pub(in crate::gui) struct EditDeps {
    /// The design library, which holds the job list.
    pub(in crate::gui) db: Arc<Mutex<Database>>,
    /// The live render context (materials, lighting).
    pub(in crate::gui) render_ctx: Arc<Mutex<RenderContext>>,
    /// The settings.
    pub(in crate::gui) settings_store: Arc<SettingsPersister>,
}

impl EditDeps {
    /// The locked database; a poisoned lock is used as it is.
    pub(in crate::gui) fn db(&self) -> MutexGuard<'_, Database> {
        self.db.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

thread_local! {
    /// The handles `setup` stored (UI thread only).
    static DEPS: RefCell<Option<EditDeps>> = const { RefCell::new(None) };
}

/// Stores the handles of the main window; called by `edit::setup`.
pub(super) fn install_deps(deps: EditDeps) {
    DEPS.with(|cell| *cell.borrow_mut() = Some(deps));
}

/// The handles `edit::setup` stored, or `None` before it ran.
#[must_use]
pub(in crate::gui) fn deps() -> Option<EditDeps> {
    DEPS.with(|cell| cell.borrow().clone())
}

/// Forgets the handles a previous main window stored. See `gui::window_release`.
pub fn release_window_state() {
    let old = DEPS.with(|cell| cell.borrow_mut().take());
    drop(old);
}

/// Names this module's holder in `out` while it is still set. See
/// `gui::window_release::live_holders`.
#[cfg(feature = "ui-harness")]
pub fn note_holders(out: &mut Vec<&'static str>) {
    if DEPS.with(|cell| cell.borrow().is_some()) {
        out.push("render_jobs::edit::save DEPS (vault, render context)");
    }
}

/// How a save ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::gui) enum SaveOutcome {
    /// The row was rewritten in place.
    Saved,
    /// A new job was inserted with this id.
    Created(i64),
    /// The row no longer has the seeded state or `frames_done`.
    RowChanged,
    /// A done or cancelled job whose compute or transfer changed: ask "Render this job again?".
    AskRequeue,
    /// The save was refused; the sentence says why.
    Refused(String),
}

/// The answer to the three-button question of a done or cancelled job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::gui) enum Requeue {
    /// The question was not asked yet.
    NotAsked,
    /// Store the choice and keep the state.
    SaveOnly,
    /// Put the job back in the queue and render it from the start.
    RenderAgain,
}

/// What a save writes besides the job itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SavePlan {
    /// The state the row gets; `None` keeps it.
    pub(super) new_state: Option<JobState>,
    /// Whether the finished frames and the result are cleared.
    pub(super) reset_progress: bool,
}

/// What the Save table says for one edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Decision {
    /// Write the row like this.
    Write(SavePlan),
    /// Ask "Render this job again?" first.
    Ask,
}

/// The Save table of the edit dialog.
///
/// `only_label` is `JobEdit::only_label_or_name`, `resets` is `JobEdit::resets_progress`.
#[must_use]
pub(super) const fn decide(
    mode: EditMode,
    only_label: bool,
    resets: bool,
    requeue: Requeue,
) -> Decision {
    let queued = Some(JobState::Queued);
    if resets {
        return Decision::Write(SavePlan {
            new_state: match mode {
                EditMode::InPlace => None,
                EditMode::KeepFramesRequeue | EditMode::RequeueWithConfirm => queued,
            },
            reset_progress: true,
        });
    }
    if only_label {
        return Decision::Write(SavePlan {
            new_state: None,
            reset_progress: false,
        });
    }
    match mode {
        EditMode::InPlace => Decision::Write(SavePlan {
            new_state: None,
            reset_progress: false,
        }),
        EditMode::KeepFramesRequeue => Decision::Write(SavePlan {
            new_state: queued,
            reset_progress: false,
        }),
        EditMode::RequeueWithConfirm => match requeue {
            Requeue::NotAsked => Decision::Ask,
            Requeue::SaveOnly => Decision::Write(SavePlan {
                new_state: None,
                reset_progress: false,
            }),
            Requeue::RenderAgain => Decision::Write(SavePlan {
                new_state: queued,
                reset_progress: true,
            }),
        },
    }
}

/// The detail line of a job in the list, rebuilt after an edit.
#[must_use]
pub(super) fn summary_for(job: &RenderJobFile) -> String {
    let material = &job.scene.material.name;
    match &job.kind {
        JobKind::Still(still) => {
            format!(
                "{material} \u{b7} {} \u{d7} {} \u{b7} {} samples \u{b7} {} bounces \u{b7} {} \u{b7} {}",
                job.scene.width,
                job.scene.height,
                still.samples_per_pixel,
                job.scene.max_bounces,
                still.color_space.label(),
                job.scene.lighting_preset.label(),
            ) + &convert::adaptive_detail(still.accel.adaptive.as_ref())
                + &convert::path_cache_detail(still.accel.path_cache.as_ref())
                + &convert::beam_detail(still.accel.beam.as_ref())
                + &convert::stone_pose_detail(StonePose::from_array(job.scene.stone_pose))
        }
        JobKind::TiltVideo(video) => {
            video_summary(&VideoFacts {
                start_deg: video.start_deg,
                end_deg: video.end_deg,
                step_deg: video.step_deg,
                frames: video.total_frames as usize,
                fps: video.fps,
                width: job.scene.width,
                height: job.scene.height,
                samples: video.samples_per_pixel,
                material: material.clone(),
            }) + &convert::adaptive_detail(video.accel.adaptive.as_ref())
                + &convert::path_cache_detail(video.accel.path_cache.as_ref())
                + &convert::beam_detail(video.accel.beam.as_ref())
        }
    }
}

/// The job after `edit`, with the label rule applied: the label is rebuilt only while it still
/// equals the automatic label of the job before the edit.
///
/// # Errors
///
/// The sentence of the refusal of [`JobEdit::apply`].
pub(super) fn prepared_job(job: &RenderJobFile, edit: &JobEdit) -> Result<RenderJobFile, String> {
    let mut new = edit.apply(job).map_err(|refusal| refusal.to_string())?;
    new.label = label_after_edit(&new.label, &automatic_label(job), &automatic_label(&new));
    if new.label.trim().is_empty() {
        new.label = automatic_label(&new);
    }
    if new.scene.material.name != job.scene.material.name {
        new.design.material.clone_from(&new.scene.material.name);
    }
    Ok(new)
}

/// Rewrites job `meta.job_id` with `edit`, if the row still has the seeded state and frames.
///
/// The token never changes. A failed job with a label-only edit stays failed. No export
/// default and no lighting setting is written.
pub(super) fn write_edit(
    db: &Database,
    meta: &RenderJobMeta,
    job: &RenderJobFile,
    seeded: &ExpectedRow,
    edit: &JobEdit,
    requeue: Requeue,
    running: bool,
) -> SaveOutcome {
    if meta.state != seeded.state || i64::from(meta.frames_done) != seeded.frames_done {
        return SaveOutcome::RowChanged;
    }
    let Some(state) = JobState::parse(&meta.state) else {
        return SaveOutcome::Refused("This job has a state this version does not know.".into());
    };
    if running {
        return SaveOutcome::Refused("This job is rendering. Pause it first.".into());
    }
    let mode = match edit_rule(state) {
        Ok(mode) => mode,
        Err(sentence) => return SaveOutcome::Refused(sentence.to_string()),
    };
    let new = match prepared_job(job, edit) {
        Ok(new) => new,
        Err(sentence) => return SaveOutcome::Refused(sentence),
    };
    let plan = match decide(
        mode,
        edit.only_label_or_name(),
        edit.resets_progress(job),
        requeue,
    ) {
        Decision::Ask => return SaveOutcome::AskRequeue,
        Decision::Write(plan) => plan,
    };
    let text = match codec::to_text(&new) {
        Ok(text) => text,
        Err(error) => return SaveOutcome::Refused(error.to_string()),
    };
    let updated = UpdatedJob {
        label: new.label.clone(),
        summary: summary_for(&new),
        output_path: new.output.clone(),
        frames_total: i64::from(new.frames_total()),
        snapshot_version: i64::from(new.format),
        snapshot: text,
        new_state: plan.new_state.map(|state| state.as_str().to_string()),
        reset_progress: plan.reset_progress,
    };
    match db.update_render_job(meta.job_id, seeded, &updated) {
        Ok(true) => {
            // The zones now live in the job file itself.
            let dropped = crate::gui::rough_colour::store::delete_job_zoning(db, meta.job_id);
            if let Err(error) = dropped {
                tracing::warn!(
                    "The colour zones of render job {} were not removed: {error:#}",
                    meta.job_id
                );
            }
            SaveOutcome::Saved
        }
        Ok(false) => SaveOutcome::RowChanged,
        Err(error) => SaveOutcome::Refused(format!("The job could not be saved: {error}")),
    }
}

/// Inserts `job` as a new queued row right after `after_id`: fresh token, a free output, the
/// summary rebuilt. Returns the new id.
///
/// # Errors
///
/// A sentence when the job breaks a rule or the database refuses it.
pub(super) fn insert_after(
    db: &Database,
    after_id: i64,
    mut job: RenderJobFile,
) -> Result<i64, String> {
    let taken: Vec<PathBuf> = match db.render_job_output_paths() {
        Ok(paths) => paths.into_iter().map(PathBuf::from).collect(),
        Err(error) => return Err(format!("The render job list could not be read: {error}")),
    };
    let reserved = reserve_in(
        Path::new(&job.output),
        JobKindTag::of(&job),
        &taken,
        &|path| path.exists(),
    );
    job.output = reserved.to_string_lossy().into_owned();
    job.token = new_job_token();
    job.format = format_for(&job);
    if job.label.trim().is_empty() {
        job.label = automatic_label(&job);
    }
    job.validate().map_err(|error| error.to_string())?;
    let text = codec::to_text(&job).map_err(|error| error.to_string())?;
    let summary = summary_for(&job);
    let id = db
        .add_render_job(
            &NewRenderJob {
                kind: job.kind_word(),
                label: &job.label,
                summary: &summary,
                frames_total: job.frames_total(),
                output_path: &job.output,
                snapshot_version: job.format,
                snapshot: &text,
            },
            unix_now(),
        )
        .map_err(|error| format!("The job could not be saved: {error}"))?;
    place_after(db, after_id, id);
    Ok(id)
}

/// Moves job `id` to the place right after `after_id`. A failure leaves the job at the end.
fn place_after(db: &Database, after_id: i64, id: i64) {
    let listed = match db.list_render_jobs() {
        Ok(metas) => metas,
        Err(error) => {
            tracing::warn!("The render job list could not be read: {error}");
            return;
        }
    };
    let mut ids: Vec<i64> = listed
        .iter()
        .map(|meta| meta.job_id)
        .filter(|other| *other != id)
        .collect();
    match ids.iter().position(|other| *other == after_id) {
        Some(at) => ids.insert(at + 1, id),
        None => ids.push(id),
    }
    if let Err(error) = db.reorder_render_jobs(&ids) {
        tracing::warn!("The new render job could not be placed: {error}");
    }
}

/// Saves `edit` as a new job after `id`: Save as New Job.
fn create_from_edit(
    db: &Database,
    id: i64,
    job: &RenderJobFile,
    edit: &JobEdit,
) -> Result<i64, String> {
    let new = prepared_job(job, edit)?;
    insert_after(db, id, new)
}

/// Saves the edit of row `id`. UI thread only.
///
/// `seeded` is the state and `frames_done` the dialog was seeded with: the save writes only while
/// the row still has them. `as_new` saves a new job after `id` instead and always works.
/// `requeue` carries the answer of the three-button question.
pub(in crate::gui) fn save_edit(
    ui: &MainWindow,
    id: i64,
    seeded: &ExpectedRow,
    edit: &JobEdit,
    as_new: bool,
    requeue: Requeue,
) -> SaveOutcome {
    let Some(deps) = deps() else {
        return SaveOutcome::Refused("The job list is not available.".into());
    };
    let (meta, job) = match load_job_for_edit(ui, id) {
        Ok(loaded) => loaded,
        Err(sentence) => return SaveOutcome::Refused(sentence),
    };
    if as_new {
        let created = create_from_edit(&deps.db(), id, &job, edit);
        return match created {
            Ok(new_id) => {
                job_changed(ui, new_id);
                SaveOutcome::Created(new_id)
            }
            Err(sentence) => SaveOutcome::Refused(sentence),
        };
    }
    let running = is_running(id);
    let outcome = write_edit(&deps.db(), &meta, &job, seeded, edit, requeue, running);
    if outcome == SaveOutcome::Saved {
        job_changed(ui, id);
    }
    outcome
}

/// Inserts `job` as a new queued row right after `after_id`; reserves its output. The one path
/// for Duplicate, Save as New Job and every copy.
///
/// # Errors
///
/// A sentence when the job breaks a rule or the database refuses it.
pub(in crate::gui) fn insert_job_after(
    ui: &MainWindow,
    after_id: i64,
    job: RenderJobFile,
) -> Result<i64, String> {
    let deps = deps().ok_or_else(|| "The job list is not available.".to_string())?;
    let id = insert_after(&deps.db(), after_id, job)?;
    job_changed(ui, id);
    Ok(id)
}
