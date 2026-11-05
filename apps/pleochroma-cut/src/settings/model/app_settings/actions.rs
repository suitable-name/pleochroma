//! Mutations of persisted settings, separate from the serialized fields.
use super::{
    AppSettings, LegacyWorkerMigration, MAX_RECENT_NATIVE_FILES, UiMode, WorkerSettings,
    clamp_remote_batch_lanes, clamp_remote_preview_batch_size, migrate_legacy_workers,
    normalize_ui_scale_percent,
};

impl AppSettings {
    /// The remote endpoint's connection settings, if one is configured -- what every
    /// render/library code path connects with.
    #[must_use]
    pub fn remote_worker(&self) -> Option<WorkerSettings> {
        self.remote.as_ref().map(|r| r.connection.clone())
    }

    /// Folds a legacy multi-worker `remote_workers` list into [`Self::remote`] -- see
    /// [`migrate_legacy_workers`]. Called by `settings::store::load_with_outcome` right
    /// after parsing; idempotent (a second call finds the legacy list empty).
    pub fn migrate_legacy_remote_workers(&mut self) -> Option<LegacyWorkerMigration> {
        migrate_legacy_workers(&mut self.legacy_remote_workers, &mut self.remote)
    }

    /// Sets [`Self::remote_batch_lanes`], limited to `1..=32` -- the only way the
    /// running app changes it.
    pub fn set_remote_batch_lanes(&mut self, lanes: u32) {
        self.remote_batch_lanes = clamp_remote_batch_lanes(lanes);
        self.remote_batch_lanes_user_set = true;
    }

    /// Sets [`Self::remote_preview_batch_size`], limited to `1..=32`.
    pub fn set_remote_preview_batch_size(&mut self, size: u32) {
        self.remote_preview_batch_size = clamp_remote_preview_batch_size(size);
    }

    /// Records `path` as the most-recently-used native design file: moves it to the
    /// front if already present (never duplicated), inserts it at the front
    /// otherwise, and truncates back to [`MAX_RECENT_NATIVE_FILES`] entries. Called on
    /// every successful Save/Open -- see this field's own doc comment.
    ///
    /// Only `.psdd` design files are recorded.
    pub fn record_recent_native_file(&mut self, path: String) {
        self.recent_native_files
            .retain(|existing| existing != &path);
        self.recent_native_files.insert(0, path);
        self.recent_native_files.truncate(MAX_RECENT_NATIVE_FILES);
    }

    /// Removes `path` from [`Self::recent_native_files`] (the startup offer's Discard), so it is
    /// neither offered at the next start nor listed under File > Open Recent. The file itself is
    /// not touched.
    pub fn forget_recent_native_file(&mut self, path: &str) {
        self.recent_native_files.retain(|existing| existing != path);
    }

    /// Whether the confirm prompt named `key` has been silenced -- see
    /// [`Self::suppressed_confirmations`]'s own doc comment.
    #[must_use]
    pub fn is_confirm_suppressed(&self, key: &str) -> bool {
        self.suppressed_confirmations.contains(key)
    }

    /// Silences the confirm prompt named `key` -- idempotent, like every other
    /// `BTreeSet::insert`.
    pub fn suppress_confirm(&mut self, key: impl Into<String>) {
        self.suppressed_confirmations.insert(key.into());
    }

    /// Turns a brand-new-install default into an existing install's: the full (Advanced)
    /// interface and no welcome tour. Used when a settings file existed but could not be
    /// read -- its owner is not a newcomer, and must not lose their controls to a reset.
    pub const fn treat_as_existing_install(&mut self) {
        self.ui_mode = UiMode::Advanced;
        self.first_run_tour_done = true;
    }

    /// Sets [`Self::ui_scale_percent`] through the one rule for it: `0` or an offered
    /// percentage is kept, anything else becomes `0` (Automatic).
    pub fn set_ui_scale_percent(&mut self, percent: i32) {
        self.ui_scale_percent = normalize_ui_scale_percent(i64::from(percent));
    }

    /// "Reset tutorial progress": forgets every finished tutorial. The welcome tour has
    /// its own switch ([`Self::show_tour_again`]) and is left alone.
    pub fn reset_tutorials(&mut self) {
        self.tutorials_completed.clear();
    }

    /// "Show the welcome tour again": the next start-up (or the preferences action that
    /// asked for it) runs the tour once more.
    pub const fn show_tour_again(&mut self) {
        self.first_run_tour_done = false;
    }
}
