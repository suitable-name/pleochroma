//! The one loader of every edit path: a queued job's row, its file and its legacy color zones.
//!
//! Edit, Duplicate, Save as New Job and Copy to Materials all read a job through
//! [`load_job_for_edit`], so a row written before format 5 keeps the zones its side table holds.
//! A successful save of the row forgets that side table (`edit::save`, `view::update`).

use crate::{MainWindow, gui::rough_colour::store};
use pleochroma_render_jobs::{RenderJobFile, codec};
use pleochroma_vault::{db::sqlite::Database, model::render_job::RenderJobMeta};
use std::{
    cell::RefCell,
    sync::{Arc, Mutex, PoisonError, Weak},
};

thread_local! {
    /// The app's vault, held weakly so a released window state is not kept alive (UI thread only).
    static DATABASE: RefCell<Weak<Mutex<Database>>> = const { RefCell::new(Weak::new()) };
}

/// Remembers the vault the edit paths read and write.
///
/// Called once from `edit::setup`, which is handed the vault; the handle is weak, so it keeps
/// nothing alive after the window state is released.
pub(in crate::gui) fn install_db(db: &Arc<Mutex<Database>>) {
    DATABASE.with(|cell| *cell.borrow_mut() = Arc::downgrade(db));
}

/// Runs `f` with the vault locked, or returns `None` when no vault was installed (or it is gone).
///
/// The lock is held for the call, so `f` must not call back into anything that locks the vault.
pub(in crate::gui) fn with_db<R>(f: impl FnOnce(&Database) -> R) -> Option<R> {
    let db = DATABASE.with(|cell| cell.borrow().upgrade())?;
    let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
    Some(f(&guard))
}

/// Reads job `id` and completes it with the zones its legacy side-table row holds.
///
/// # Errors
///
/// One plain sentence: the vault is not ready, the job is gone, its row or file cannot be read,
/// or its stored zones are unusable.
pub(in crate::gui) fn load_job_for_edit(
    _ui: &MainWindow,
    id: i64,
) -> Result<(RenderJobMeta, RenderJobFile), String> {
    with_db(|db| load_from(db, id)).unwrap_or_else(|| Err("The job list is not ready.".to_string()))
}

/// [`load_job_for_edit`] on a given vault: the row, the decoded file and `complete_job_zoning`.
///
/// A job that already holds its zones is left as it is; a Ray-Traced job keeps none.
///
/// # Errors
///
/// The same sentences as [`load_job_for_edit`].
pub(in crate::gui) fn load_from(
    db: &Database,
    id: i64,
) -> Result<(RenderJobMeta, RenderJobFile), String> {
    let row = db
        .get_render_job(id)
        .map_err(|error| format!("The job could not be read: {error}"))?
        .ok_or_else(|| "This job no longer exists.".to_string())?;
    let mut job = codec::from_text(&row.snapshot).map_err(|error| error.to_string())?;
    store::complete_job_zoning(db, id, &mut job).map_err(|error| format!("{error:#}"))?;
    Ok((row.meta, job))
}

/// Forgets the side-table zones of row `id`, as a save does once the job file holds them.
#[cfg(test)]
fn delete_zoning_in(db: &Database, id: i64) {
    if let Err(error) = store::delete_job_zoning(db, id) {
        tracing::warn!("The color zones of render job {id} were not removed: {error:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::optics::materials::GemMaterial;
    use pleochroma_vault::model::render_job::NewRenderJob;

    /// A queued row for `job`, with the job's file as its snapshot.
    fn add_row(db: &Database, job: &RenderJobFile) -> i64 {
        let text = codec::to_text(job).expect("the job encodes");
        db.add_render_job(
            &NewRenderJob {
                kind: "still",
                label: &job.label,
                summary: "test",
                frames_total: 1,
                output_path: &job.output,
                snapshot_version: job.format,
                snapshot: &text,
            },
            0,
        )
        .expect("the row is added")
    }

    /// A missing job is a sentence, not a panic, and a stored job loads whole.
    #[test]
    fn a_stored_job_loads_and_a_missing_one_is_a_sentence() {
        let db = Database::new(Some(":memory:")).expect("vault");
        let job = super::super::tests_material::still_job("Sapphire", false);
        let id = add_row(&db, &job);
        let (meta, loaded) = load_from(&db, id).expect("loads");
        assert_eq!(meta.job_id, id);
        assert_eq!(loaded, job);
        assert_eq!(
            load_from(&db, id + 99),
            Err("This job no longer exists.".to_string())
        );
    }

    /// A row from before format 5 gets its zones from the side table, and the zones are
    /// forgotten after a save; a Ray-Traced row gets none.
    #[test]
    fn legacy_side_table_zones_are_attached_and_then_forgotten() {
        let db = Database::new(Some(":memory:")).expect("vault");
        let zoned = GemMaterial::by_name("Ametrine").expect("the zoned built-in");
        let mut legacy = super::super::tests_material::still_job("Ametrine", false);
        legacy.scene.material = zoned.clone();
        legacy.move_scene_zoning_to_file();
        legacy.zoning = None;
        legacy.format = pleochroma_render_jobs::format_for(&legacy);
        let id = add_row(&db, &legacy);
        assert!(store::save_job_zoning(&db, id, &zoned).expect("saves"));

        let (_, loaded) = load_from(&db, id).expect("loads");
        assert_eq!(loaded.zoning, zoned.zoning);
        assert_eq!(loaded.format, 5);
        assert_eq!(loaded.validate(), Ok(()));

        delete_zoning_in(&db, id);
        let (_, after) = load_from(&db, id).expect("loads");
        assert!(after.zoning.is_none(), "the side table is gone");
    }

    /// A Ray-Traced row keeps no zones even when its side table holds some.
    #[test]
    fn a_ray_traced_row_gets_no_zones() {
        let db = Database::new(Some(":memory:")).expect("vault");
        let zoned = GemMaterial::by_name("Ametrine").expect("the zoned built-in");
        let mut ray = super::super::tests_material::still_job("Ametrine", true);
        ray.scene.material = zoned.clone();
        ray.zoning = None;
        ray.format = pleochroma_render_jobs::format_for(&ray);
        let id = add_row(&db, &ray);
        assert!(store::save_job_zoning(&db, id, &zoned).expect("saves"));
        let (_, loaded) = load_from(&db, id).expect("loads");
        assert!(loaded.zoning.is_none());
        assert!(loaded.scene.material.zoning.is_none());
    }
}
