//! `diagnostics.toml`: the exit timestamp, the engine marks and the shader cache counts.
//!
//! Marks and counts belong to one pair of app version and `Build.FINGERPRINT`. A new version or
//! a system update makes the file forget them, so a GPU that was bad once gets another try.

use std::{
    io,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

use pleochroma::diag::Engine;
use serde::{Deserialize, Serialize};

use super::exit::CacheDecision;

/// The state file's name inside the app's private files folder.
pub const DIAG_FILE: &str = "diagnostics.toml";

/// What `diagnostics.toml` holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiagFile {
    /// The timestamp of the newest process exit already reported, so none is reported twice.
    pub last_exit_ms: i64,
    /// The app version the marks and counts below belong to.
    pub version: String,
    /// The `Build.FINGERPRINT` they belong to.
    pub fingerprint: String,
    /// Path-Traced uses the CPU in Automatic (a confirmed crash in one of its GPU steps).
    pub path_traced_marked: bool,
    /// Ray-Traced uses the CPU in Automatic.
    pub ray_traced_marked: bool,
    /// Confirmed crashes in a shader cache step under this pair.
    pub cache_crashes: u32,
    /// The next start runs without the cache (one cache crash happened, not yet acted on).
    pub cache_skip_next: bool,
    /// The step that was open at the last start without exit information (API 28 and 29), as a
    /// key; the same key twice in a row counts as a crash.
    pub previous_step: String,
}

impl DiagFile {
    /// Reads the file; a missing or unreadable file is the empty state.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes the file whole (a temporary file renamed over it).
    ///
    /// # Errors
    ///
    /// The file system's error, or a message when the state cannot be written as TOML.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string(self).map_err(io::Error::other)?;
        write_atomic(path, text.as_bytes())
    }

    /// Whether the marks and counts belong to this version and fingerprint.
    #[must_use]
    pub fn matches(&self, version: &str, fingerprint: &str) -> bool {
        self.version == version && self.fingerprint == fingerprint
    }

    /// Makes the file belong to this pair: another pair's marks and counts are dropped.
    pub fn for_pair(&mut self, version: &str, fingerprint: &str) {
        if self.matches(version, fingerprint) {
            return;
        }
        *self = Self {
            last_exit_ms: self.last_exit_ms,
            version: version.to_owned(),
            fingerprint: fingerprint.to_owned(),
            ..Self::default()
        };
    }

    /// Whether `engine` is marked.
    #[must_use]
    pub const fn marked(&self, engine: Engine) -> bool {
        match engine {
            Engine::PathTraced => self.path_traced_marked,
            Engine::RayTraced => self.ray_traced_marked,
        }
    }

    /// Sets or clears the mark of `engine`.
    pub const fn set_marked(&mut self, engine: Engine, marked: bool) {
        match engine {
            Engine::PathTraced => self.path_traced_marked = marked,
            Engine::RayTraced => self.ray_traced_marked = marked,
        }
    }
}

/// Writes `bytes` to a temporary sibling and renames it over `path`.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)
}

/// The session's view of `diagnostics.toml`.
pub struct DiagState {
    path: PathBuf,
    version: String,
    fingerprint: String,
    file: Mutex<DiagFile>,
    cache: CacheDecision,
}

impl DiagState {
    /// Reads `<files_dir>/diagnostics.toml`. `cache` is what this process decided at its start.
    #[must_use]
    pub fn open(files_dir: &Path, version: &str, fingerprint: &str, cache: CacheDecision) -> Self {
        let path = files_dir.join(DIAG_FILE);
        let file = DiagFile::load(&path);
        Self {
            path,
            version: version.to_owned(),
            fingerprint: fingerprint.to_owned(),
            file: Mutex::new(file),
            cache,
        }
    }

    /// False while the engine mark stands for this version and fingerprint.
    #[must_use]
    pub fn gpu_allowed(&self, engine: Engine) -> bool {
        let file = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        !(file.matches(&self.version, &self.fingerprint) && file.marked(engine))
    }

    /// A manual Device GPU choice: removes the mark and saves.
    pub fn clear_engine_mark(&self, engine: Engine) {
        let mut file = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        if !file.marked(engine) {
            return;
        }
        file.set_marked(engine, false);
        if let Err(error) = file.save(&self.path) {
            tracing::warn!(%error, "the diagnostics state could not be saved");
        }
    }

    /// What the process decided about the shader cache at its start.
    #[must_use]
    pub const fn cache(&self) -> CacheDecision {
        self.cache
    }

    /// A copy of the file's current content.
    #[must_use]
    pub fn snapshot(&self) -> DiagFile {
        self.file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_mock::unique_temp_dir;

    fn marked_file(version: &str, fingerprint: &str) -> DiagFile {
        let mut file = DiagFile::default();
        file.for_pair(version, fingerprint);
        file.set_marked(Engine::RayTraced, true);
        file
    }

    #[test]
    fn the_file_round_trips_through_toml() {
        let dir = unique_temp_dir("diag-state-roundtrip");
        let path = dir.join(DIAG_FILE);
        let mut file = marked_file("0.12.0", "fp-a");
        file.last_exit_ms = 1_700_000_000_123;
        file.cache_crashes = 1;
        file.cache_skip_next = true;
        file.previous_step = "Compile|RayTraced|whitted_main|".to_owned();
        file.save(&path).expect("saved");
        assert_eq!(DiagFile::load(&path), file);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_or_broken_file_is_the_empty_state() {
        let dir = unique_temp_dir("diag-state-broken");
        let path = dir.join(DIAG_FILE);
        assert_eq!(DiagFile::load(&path), DiagFile::default());
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(&path, "this is not = = toml [").expect("written");
        assert_eq!(DiagFile::load(&path), DiagFile::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_mark_is_keyed_by_version_and_fingerprint() {
        let dir = unique_temp_dir("diag-state-keyed");
        marked_file("0.12.0", "fp-a")
            .save(&dir.join(DIAG_FILE))
            .expect("saved");
        let same = DiagState::open(&dir, "0.12.0", "fp-a", CacheDecision::On);
        assert!(!same.gpu_allowed(Engine::RayTraced));
        assert!(same.gpu_allowed(Engine::PathTraced));
        let new_version = DiagState::open(&dir, "0.12.1", "fp-a", CacheDecision::On);
        assert!(new_version.gpu_allowed(Engine::RayTraced));
        let new_fingerprint = DiagState::open(&dir, "0.12.0", "fp-b", CacheDecision::On);
        assert!(new_fingerprint.gpu_allowed(Engine::RayTraced));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_manual_choice_clears_the_mark_and_keeps_it_cleared() {
        let dir = unique_temp_dir("diag-state-clear");
        marked_file("0.12.0", "fp-a")
            .save(&dir.join(DIAG_FILE))
            .expect("saved");
        let state = DiagState::open(&dir, "0.12.0", "fp-a", CacheDecision::On);
        assert!(!state.gpu_allowed(Engine::RayTraced));
        state.clear_engine_mark(Engine::RayTraced);
        assert!(state.gpu_allowed(Engine::RayTraced));
        let reopened = DiagState::open(&dir, "0.12.0", "fp-a", CacheDecision::On);
        assert!(reopened.gpu_allowed(Engine::RayTraced));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn another_pair_drops_marks_and_counts_but_keeps_the_exit_time() {
        let mut file = marked_file("0.12.0", "fp-a");
        file.last_exit_ms = 77;
        file.cache_crashes = 2;
        file.for_pair("0.12.0", "fp-b");
        assert_eq!(file.last_exit_ms, 77);
        assert_eq!(file.cache_crashes, 0);
        assert!(!file.marked(Engine::RayTraced));
        assert_eq!(file.fingerprint, "fp-b");
    }
}
