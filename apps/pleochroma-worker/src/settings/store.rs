//! The process-wide current settings.
//!
//! Loaded once by [`init`], read with [`current`], changed with [`apply_patch`] (the safe fields,
//! also reachable from another computer) or [`update`] (everything, local only). Every change is
//! validated, written to disk atomically, and then announced through the hook set with
//! [`set_change_hook`].
//!
//! The storage choices (job folder, retention, cache) have no `serve` flag. The code that
//! opens the job folder and the asset cache asks [`storage_overrides`] after it has looked at
//! its environment variable, so an environment variable still wins.

use super::model::{CoordinatorSettings, LoadOutcome};
use pleochroma_net::admin::status::SettingsPatch;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock},
};

/// The loaded settings and where they live.
struct Store {
    /// The settings file.
    path: PathBuf,
    /// The folder `pki/` is in when the settings choose no data folder.
    data_dir: PathBuf,
    /// The live settings.
    settings: CoordinatorSettings,
}

/// A function told about every successful change, with the new settings.
type ChangeHook = Arc<dyn Fn(&CoordinatorSettings) + Send + Sync>;

/// The live store; `None` until [`init`].
static STORE: Mutex<Option<Store>> = Mutex::new(None);

/// The function told about changes.
static HOOK: RwLock<Option<ChangeHook>> = RwLock::new(None);

/// The groups the last change touched ("performance, storage"), for the activity line.
static LAST_SUMMARY: Mutex<String> = Mutex::new(String::new());

/// Where the job folder and the asset cache are, from the settings. Used when the matching
/// environment variable is not set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageOverrides {
    /// The folder durable jobs are kept in (`<data>/jobs`).
    pub job_dir: PathBuf,
    /// Hours a finished job is kept.
    pub job_retention_hours: u64,
    /// The asset cache folder (`<data>/asset-cache`).
    pub cache_dir: PathBuf,
    /// The asset cache size cap, in MiB.
    pub cache_mib: u64,
}

impl Store {
    /// Writes `next` to the settings file and, only if that worked, makes it the live
    /// settings. Returns the new settings and the summary of what changed.
    fn commit(
        &mut self,
        next: CoordinatorSettings,
    ) -> Result<(CoordinatorSettings, String), String> {
        next.save(&self.path)?;
        let summary = self.settings.summary_of_change(&next);
        self.settings = next.clone();
        Ok((next, summary))
    }

    /// The overrides these settings give the job folder and cache.
    fn storage_overrides(&self) -> StorageOverrides {
        let data = self.settings.data_dir(&self.data_dir);
        StorageOverrides {
            job_dir: data.join("jobs"),
            job_retention_hours: u64::from(self.settings.job_retention_days) * 24,
            cache_dir: data.join("asset-cache"),
            cache_mib: u64::from(self.settings.cache_size_mib),
        }
    }
}

/// The writer lock: one change at a time, from reading the live settings to saving the new
/// ones. It is always taken before the store's own lock and released before the hook runs.
static WRITER: Mutex<()> = Mutex::new(());

/// Locks the store cell `cell`, ignoring a lock poisoned by a panic elsewhere.
fn lock_cell(cell: &Mutex<Option<Store>>) -> MutexGuard<'_, Option<Store>> {
    cell.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Locks the store, ignoring a lock poisoned by a panic elsewhere.
fn lock() -> MutexGuard<'static, Option<Store>> {
    lock_cell(&STORE)
}

/// Takes the writer lock `writer`, ignoring a lock poisoned by a panic elsewhere.
fn lock_writer(writer: &Mutex<()>) -> MutexGuard<'_, ()> {
    writer.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Makes one change in the store cell `cell` while holding `writer`: `make` builds the new
/// settings from the live ones, and the result is saved and made live. `Ok(None)` when nothing
/// changed. Two changes made at the same time run one after the other, and each one sees the
/// other's result, so neither is lost and neither fails.
fn change_in(
    cell: &Mutex<Option<Store>>,
    writer: &Mutex<()>,
    make: impl FnOnce(&CoordinatorSettings) -> Result<CoordinatorSettings, String>,
) -> Result<Option<(CoordinatorSettings, String)>, String> {
    let _writer = lock_writer(writer);
    let base = {
        let guard = lock_cell(cell);
        guard.as_ref().ok_or_else(not_loaded)?.settings.clone()
    };
    // Outside the store's lock, so `make` may read the live settings itself.
    let next = make(&base)?;
    if next == base {
        return Ok(None);
    }
    let mut guard = lock_cell(cell);
    let store = guard.as_mut().ok_or_else(not_loaded)?;
    let committed = store.commit(next).map(Some);
    drop(guard);
    committed
}

/// Loads the settings file at `path` and makes it the live settings. Call once at start-up.
/// The data folder defaults to the folder the file is in until [`set_data_dir`] says otherwise.
///
/// An invalid file is renamed and the defaults are used; a file that cannot be read at all is
/// left as it is and the defaults are used for this run. The outcome's notice is the sentence
/// for the banner.
#[must_use]
pub fn init(path: PathBuf) -> LoadOutcome {
    let outcome = CoordinatorSettings::load(&path);
    install(path, outcome.settings.clone());
    outcome
}

/// Makes the settings in the file at `path` the live settings for a command-line tool that only
/// reads them (`cert issue-token --config`): nothing is renamed or logged.
///
/// Returns whether the file held valid settings; when it did not, the store stays as it was.
#[must_use]
pub fn init_read_only(path: PathBuf) -> bool {
    CoordinatorSettings::peek(&path).is_some_and(|settings| {
        install(path, settings);
        true
    })
}

/// Stores `settings` as the live settings read from `path`.
fn install(path: PathBuf, settings: CoordinatorSettings) {
    let _writer = lock_writer(&WRITER);
    let mut guard = lock();
    // A second call (after the wizard wrote the file) replaces the live settings and keeps the
    // data folder an earlier `set_data_dir` chose.
    let data_dir = guard.as_ref().map_or_else(
        || {
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf)
        },
        |s| s.data_dir.clone(),
    );
    *guard = Some(Store {
        path,
        data_dir,
        settings,
    });
}

/// A clone of the live settings; the defaults before [`init`].
#[must_use]
pub fn current() -> CoordinatorSettings {
    lock()
        .as_ref()
        .map_or_else(CoordinatorSettings::default, |s| s.settings.clone())
}

/// The settings file in use, or `None` before [`init`].
#[must_use]
pub fn settings_file() -> Option<PathBuf> {
    lock().as_ref().map(|s| s.path.clone())
}

/// Tells the store which folder holds `pki/` when the settings choose no data folder: the
/// per-user data folder in the window.
pub fn set_data_dir(dir: PathBuf) {
    if let Some(store) = lock().as_mut() {
        store.data_dir = dir;
    }
}

/// The data folder in use: the chosen one, or the one given to [`set_data_dir`] (the folder of
/// the settings file by default). `None` before [`init`].
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    lock().as_ref().map(|s| s.settings.data_dir(&s.data_dir))
}

/// The folder `pki/` is in when no data folder is chosen, or `None` before [`init`].
#[must_use]
pub fn default_data_dir() -> Option<PathBuf> {
    lock().as_ref().map(|s| s.data_dir.clone())
}

/// The job folder, retention and cache chosen by the settings; `None` before [`init`] (a
/// headless run that never loads the settings file).
#[must_use]
pub fn storage_overrides() -> Option<StorageOverrides> {
    lock().as_ref().map(Store::storage_overrides)
}

/// What the last change touched, for the activity line ("Settings changed: performance").
#[must_use]
pub fn last_change_summary() -> String {
    LAST_SUMMARY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Registers the function told about every successful change. A second call replaces the
/// first. The function runs on the thread that made the change, outside the settings lock.
pub fn set_change_hook(f: impl Fn(&CoordinatorSettings) + Send + Sync + 'static) {
    *HOOK.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(f));
}

/// Announces a change: records its summary and calls the hook.
fn announce(next: &CoordinatorSettings, summary: String) {
    *LAST_SUMMARY.lock().unwrap_or_else(PoisonError::into_inner) = summary;
    let hook = HOOK.read().unwrap_or_else(PoisonError::into_inner).clone();
    if let Some(hook) = hook {
        hook(next);
    }
}

/// Applies a change of the safe settings: validates it, saves it, and announces it.
///
/// # Errors
///
/// A message in plain words when a value is out of range, when the settings are not loaded,
/// or when the file cannot be written. Nothing changes on an error.
pub fn apply_patch(patch: &SettingsPatch) -> Result<(), String> {
    if patch.is_empty() {
        return Ok(());
    }
    if let Some((next, summary)) = change_in(&STORE, &WRITER, |live| live.with_patch(patch))? {
        announce(&next, summary);
    }
    Ok(())
}

/// Changes the settings with `f` (network, ports, certificate names and the other fields no
/// remote computer may change): the result is validated, saved and announced.
///
/// # Errors
///
/// A message in plain words when the result is not valid, when the settings are not loaded, or
/// when the file cannot be written. Nothing changes on an error.
///
/// Changes are made one at a time: a change made while another one is running waits for it and
/// then starts from its result, so two quick changes both land. `f` runs on a copy outside the
/// store's lock, so it may read the settings itself; it must not call [`update`],
/// [`apply_patch`] or [`reload`] (that would wait for itself).
pub fn update(f: impl FnOnce(&mut CoordinatorSettings)) -> Result<(), String> {
    let made = change_in(&STORE, &WRITER, |live| {
        let mut next = live.clone();
        f(&mut next);
        next.validate()?;
        Ok(next)
    })?;
    if let Some((next, summary)) = made {
        announce(&next, summary);
    }
    Ok(())
}

/// Reads the settings file again and makes it the live settings (after a restore). A file that
/// cannot be used is handled exactly as in [`init`].
///
/// # Errors
///
/// A message when the settings are not loaded yet.
pub fn reload() -> Result<LoadOutcome, String> {
    let (outcome, summary) = {
        let _writer = lock_writer(&WRITER);
        let mut guard = lock();
        let store = guard.as_mut().ok_or_else(not_loaded)?;
        let outcome = CoordinatorSettings::load(&store.path);
        let summary = store.settings.summary_of_change(&outcome.settings);
        store.settings = outcome.settings.clone();
        drop(guard);
        (outcome, summary)
    };
    announce(&outcome.settings, summary);
    Ok(outcome)
}

/// The refusal before [`init`] has run.
fn not_loaded() -> String {
    "The settings are not loaded yet.".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn store_in(name: &str) -> (Store, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("pleochroma-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("coordinator.toml");
        (
            Store {
                path,
                data_dir: dir.clone(),
                settings: CoordinatorSettings::default(),
            },
            dir,
        )
    }

    #[test]
    fn a_commit_saves_first_and_reports_the_groups() {
        let (mut store, dir) = store_in("commit");
        let next = store
            .settings
            .with_patch(&SettingsPatch {
                job_retention_days: Some(30),
                ..SettingsPatch::default()
            })
            .expect("valid");
        let (live, summary) = store.commit(next).expect("saved");
        assert_eq!(summary, "storage");
        assert_eq!(store.settings.job_retention_days, 30);
        assert_eq!(live, store.settings);
        let reloaded = CoordinatorSettings::load(&store.path);
        assert_eq!(reloaded.settings.job_retention_days, 30);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_save_leaves_the_live_settings_alone() {
        let (mut store, dir) = store_in("failed");
        // A folder where the file should be: the rename cannot succeed.
        std::fs::create_dir_all(&store.path).expect("blocker");
        let mut next = store.settings.clone();
        next.discovery = false;
        assert!(store.commit(next).is_err());
        assert!(store.settings.discovery);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two threads change the settings at the same time, many times each: every change lands
    /// and none fails with a "settings changed" error.
    #[test]
    fn concurrent_updates_both_land() {
        const STEPS: u32 = 25;
        let (store, dir) = store_in("concurrent");
        let cell = Mutex::new(Some(store));
        let writer = Mutex::new(());
        let bump = |read: fn(&CoordinatorSettings) -> u32,
                    write: fn(&mut CoordinatorSettings, u32)| {
            for _ in 0..STEPS {
                let made = change_in(&cell, &writer, |live| {
                    let mut next = live.clone();
                    // Read the live value inside the change: a lost update shows as a
                    // smaller total at the end.
                    write(&mut next, read(live) + 1);
                    next.validate()?;
                    Ok(next)
                });
                assert!(made.expect("no change may fail").is_some());
            }
        };
        std::thread::scope(|scope| {
            scope.spawn(|| {
                bump(|s| s.job_retention_days, |s, v| s.job_retention_days = v);
            });
            scope.spawn(|| {
                bump(|s| s.max_per_address, |s, v| s.max_per_address = v);
            });
        });
        let live = lock_cell(&cell).as_ref().expect("store").settings.clone();
        let defaults = CoordinatorSettings::default();
        assert_eq!(live.job_retention_days, defaults.job_retention_days + STEPS);
        assert_eq!(live.max_per_address, defaults.max_per_address + STEPS);
        // The file holds the last change too.
        let saved = CoordinatorSettings::load(&dir.join("coordinator.toml")).settings;
        assert_eq!(saved, live);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn storage_overrides_sit_under_the_data_folder() {
        let (mut store, dir) = store_in("storage");
        store.data_dir = PathBuf::from("/data");
        let o = store.storage_overrides();
        assert_eq!(o.job_dir, Path::new("/data").join("jobs"));
        assert_eq!(o.cache_dir, Path::new("/data").join("asset-cache"));
        assert_eq!(o.job_retention_hours, 7 * 24);
        assert_eq!(o.cache_mib, 2048);
        store.settings.data_folder = Some(PathBuf::from("/chosen"));
        assert_eq!(
            store.storage_overrides().job_dir,
            Path::new("/chosen").join("jobs")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
