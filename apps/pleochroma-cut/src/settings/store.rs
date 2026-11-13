//! Load/save the settings TOML file.
//!
//! A settings file must never prevent the app from starting: every failure mode here
//! (missing file, unreadable file, corrupt/unparseable TOML) is caught and logged,
//! falling back to defaults rather than propagating an error.

use super::model::{DEFAULT_PATH_CACHE_DISK_LIMIT_GB, SettingsFile, normalize_ui_scale_percent};
use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};
use tracing::{info, warn};

// This crate's own app folder. Kept apart from the older app's folder so the two binaries
// can be installed side by side without one silently overwriting the other's settings.
const APP_DIR_NAME: &str = "pleochroma-cut";
/// The app-dir name of the legacy `diagram-gui` viewer, read only by
/// [`migrate_legacy_settings_if_needed`]. Must differ from [`APP_DIR_NAME`]: equal names would
/// copy the file onto itself.
const LEGACY_APP_DIR_NAME: &str = "diagram-gui";
const SETTINGS_FILE_NAME: &str = "settings.toml";
/// The folder of persisted path caches, beside the settings file.
const RENDER_CACHE_DIR_NAME: &str = "render_cache";
/// The design library's file name.
const LIBRARY_FILE_NAME: &str = "facet_diagrams.sqlite";
/// Names a design library file to open instead of the default one.
pub const LIBRARY_PATH_ENV: &str = "PLEOCHROMA_LIBRARY";

/// Resolves `app_dir_name/settings.toml` inside the platform config directory (not
/// next to the executable, to avoid requiring write access to the install directory).
/// Shared by [`default_settings_path`] and [`legacy_settings_path`] so the two paths
/// can never disagree about where the platform config directory itself lives.
///
/// - Windows: `%APPDATA%\<app_dir_name>\settings.toml`
/// - macOS: `~/Library/Application Support/<app_dir_name>/settings.toml`
/// - Linux/other Unix: `$XDG_CONFIG_HOME/<app_dir_name>/settings.toml`, falling back to
///   `~/.config/<app_dir_name>/settings.toml`
///
/// If none of the expected environment variables are set (unusual, but not
/// impossible), falls back to a `<app_dir_name>/settings.toml` path relative to the
/// current working directory rather than failing outright.
fn settings_path_under(app_dir_name: &str) -> PathBuf {
    platform_config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(app_dir_name)
        .join(SETTINGS_FILE_NAME)
}

/// The design library the app opens, first match wins:
///
/// 1. the file named by [`LIBRARY_PATH_ENV`];
/// 2. `facet_diagrams.sqlite` in the working directory, when that file exists (a library
///    kept beside a checkout or a portable install keeps working);
/// 3. `facet_diagrams.sqlite` beside `settings.toml` in the per-user app folder.
///
/// The working directory alone is never a place to create the library: an app started
/// from the Start menu or the Microsoft Store runs in a folder the user cannot write to
/// (the install folder or `System32`), and the library could not be created there.
#[must_use]
pub fn default_library_path() -> PathBuf {
    // With the UI-harness seams active, the working directory's library is used and the
    // environment is ignored, so a harness run never reaches a real library.
    if crate::harness_seams_active() {
        return std::env::current_dir().map_or_else(
            |_| PathBuf::from(LIBRARY_FILE_NAME),
            |dir| dir.join(LIBRARY_FILE_NAME),
        );
    }
    library_path_from(
        std::env::var_os(LIBRARY_PATH_ENV),
        std::env::current_dir().ok(),
        platform_config_dir(),
    )
}

/// [`default_library_path`] with its inputs passed in, for the tests.
fn library_path_from(
    override_path: Option<OsString>,
    working_dir: Option<PathBuf>,
    config_dir: Option<PathBuf>,
) -> PathBuf {
    if let Some(path) = override_path.filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    if let Some(local) = working_dir
        .map(|dir| dir.join(LIBRARY_FILE_NAME))
        .filter(|path| path.is_file())
    {
        return local;
    }
    config_dir.map_or_else(
        || PathBuf::from(LIBRARY_FILE_NAME),
        |dir| dir.join(APP_DIR_NAME).join(LIBRARY_FILE_NAME),
    )
}

/// This crate's own settings file path -- see [`settings_path_under`].
#[must_use]
pub fn default_settings_path() -> PathBuf {
    settings_path_under(APP_DIR_NAME)
}

/// The folder of the persisted path caches: `render_cache/` beside the settings file.
///
/// The folder is not created here; the path cache creates it when it writes its first cache.
#[must_use]
pub fn default_render_cache_dir() -> PathBuf {
    render_cache_dir_beside(&default_settings_path())
}

/// [`default_render_cache_dir`] for a given settings file (so tests can use a temp folder).
fn render_cache_dir_beside(settings_path: &Path) -> PathBuf {
    settings_path.parent().map_or_else(
        || PathBuf::from(RENDER_CACHE_DIR_NAME),
        |dir| dir.join(RENDER_CACHE_DIR_NAME),
    )
}

/// The older app's settings file, in the same platform config directory as this crate's.
/// It is the migration source for [`migrate_legacy_settings_if_needed`].
fn legacy_settings_path() -> PathBuf {
    settings_path_under(LEGACY_APP_DIR_NAME)
}

/// One-time carry-over of the older app's settings file into this crate's own file.
///
/// An existing install thus does not start blank (remote workers, cert directories, export
/// paths and presets all come across).
///
/// Call it before [`load_with_outcome`], with `new_path` this crate's settings path. It
/// is passed in so tests can use a temp directory.
///
/// - **Copies, never moves.** The older app may still be installed and read the same
///   file, so deleting or renaming it would break that app. `std::fs::copy` never
///   touches the source.
/// - **Never overwrites an existing settings file.** It only fills an absent
///   destination: a one-time bootstrap, not an ongoing sync.
/// - **A failed copy does not stop startup.** Failures are logged with `warn!`, and
///   [`load_with_outcome`] then falls back to defaults.
pub fn migrate_legacy_settings_if_needed(new_path: &Path) {
    copy_legacy_settings_if_absent(&legacy_settings_path(), new_path);
}

/// The actual copy logic behind [`migrate_legacy_settings_if_needed`], with `old_path`
/// passed in explicitly so this module's tests can exercise every branch with
/// temp-directory paths rather than touching a real `%APPDATA%`.
fn copy_legacy_settings_if_absent(old_path: &Path, new_path: &Path) {
    if new_path.exists() {
        // Already has its own settings -- never overwrite it.
        return;
    }
    if !old_path.exists() {
        // The common case: nothing to migrate. Not even worth a `warn!`.
        return;
    }
    if let Some(parent) = new_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        warn!(
            "Could not create {} to migrate legacy pleochroma-cut settings into; \
             starting with defaults instead: {e}",
            parent.display()
        );
        return;
    }
    match std::fs::copy(old_path, new_path) {
        Ok(_) => info!(
            "Migrated settings from the legacy config at {} to {}.",
            old_path.display(),
            new_path.display()
        ),
        Err(e) => warn!(
            "Could not copy legacy settings from {} to {}; starting with defaults \
             instead: {e}",
            old_path.display(),
            new_path.display()
        ),
    }
}

#[cfg(target_os = "windows")]
fn platform_config_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn platform_config_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_config_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
}

/// What happened while loading the settings file, returned by [`load_with_outcome`] so
/// `gui::main_window` can warn about a corrupt file. A log line alone goes unseen in a
/// release build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsLoadOutcome {
    /// Loaded normally, or the file is absent (first run).
    Ok,
    /// The file existed but did not parse as `SettingsFile` TOML. It was renamed to
    /// `renamed_to` (see [`rename_aside`]) before falling back to defaults, so the next
    /// save does not overwrite it.
    Corrupt {
        /// Where the corrupt file was moved, for the warning to name.
        renamed_to: PathBuf,
    },
}

/// Renames `path` aside to `<path>.corrupt-<unix-seconds>` so the next save never
/// silently overwrites the one piece of evidence explaining what went wrong. Returns
/// the new path on success; logs and returns `None` on failure (e.g. no write
/// permission on the containing directory) rather than blocking startup on a rename
/// that isn't the load itself.
fn rename_aside(path: &Path) -> Option<PathBuf> {
    let unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut renamed = path.as_os_str().to_owned();
    renamed.push(format!(".corrupt-{unix_secs}"));
    let renamed = PathBuf::from(renamed);
    match std::fs::rename(path, &renamed) {
        Ok(()) => Some(renamed),
        Err(e) => {
            warn!(
                "Could not rename the corrupt settings file at {} aside to {}: {e}. \
                 Continuing with defaults; the corrupt file is still at its original path.",
                path.display(),
                renamed.display()
            );
            None
        }
    }
}

/// The defaults for a settings file that EXISTED but could not be used (corrupt, or
/// unreadable): [`SettingsFile::default`], except that its owner is no newcomer, so they
/// keep the full interface and get no welcome tour. Only an absent file is a new install.
fn defaults_for_existing_install() -> SettingsFile {
    let mut file = SettingsFile::default();
    file.settings.treat_as_existing_install();
    file
}

/// The saved interface scale in percent (`0` = Automatic), read straight from the settings
/// file before anything else starts.
///
/// The scale must reach `SLINT_SCALE_FACTOR` before the first window is created, earlier
/// than [`load_with_outcome`]. This reads only that field, so it needs no model and never
/// fails: anything missing or unusable gives `0`.
#[must_use]
pub fn peek_ui_scale_percent() -> u16 {
    peek_ui_scale_percent_at(&default_settings_path())
}

/// [`peek_ui_scale_percent`] for an explicit `path`, so the tests can point it anywhere.
fn peek_ui_scale_percent_at(path: &Path) -> u16 {
    std::fs::read_to_string(path).map_or(0, |text| ui_scale_percent_in(&text))
}

/// The `[settings] ui_scale_percent` value in the settings document `text`, normalised
/// (`0` when absent or not an integer).
fn ui_scale_percent_in(text: &str) -> u16 {
    toml::from_str::<toml::Table>(text)
        .ok()
        .and_then(|document| {
            document
                .get("settings")
                .and_then(toml::Value::as_table)
                .and_then(|settings| settings.get("ui_scale_percent"))
                .and_then(toml::Value::as_integer)
        })
        .map_or(0, normalize_ui_scale_percent)
}

/// The stored RADV shader-workaround answer (`[settings] radv_scheduler_workaround`), read
/// before anything else starts.
///
/// Mesa reads `ACO_DEBUG` when the Vulkan device is created, so `main` must know the
/// answer before any thread or GPU call. Like [`peek_ui_scale_percent`] this reads one
/// field and never fails: anything missing or unfamiliar gives `None` ("not asked yet").
#[must_use]
pub fn peek_radv_choice() -> Option<super::model::RadvWorkaroundChoice> {
    peek_radv_choice_at(&default_settings_path())
}

/// [`peek_radv_choice`] for an explicit `path`, so the tests can point it anywhere.
fn peek_radv_choice_at(path: &Path) -> Option<super::model::RadvWorkaroundChoice> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| super::model::RadvWorkaroundChoice::in_document(&text))
}

/// The saved disk limit of the path caches in gigabytes, read straight from the file
/// (`[settings] path_cache_disk_limit_gb`).
///
/// For renders on the export thread, which has no handle on the `SettingsPersister`. Like
/// [`peek_ui_scale_percent`] it reads one field and never fails: anything missing or not a
/// whole number of gigabytes gives [`DEFAULT_PATH_CACHE_DISK_LIMIT_GB`]. A change reaches
/// the file only after the persister's debounce, so a render started just after it still
/// reads the old limit.
#[must_use]
pub fn peek_path_cache_disk_limit_gb() -> u32 {
    peek_path_cache_disk_limit_gb_at(&default_settings_path())
}

/// [`peek_path_cache_disk_limit_gb`] for an explicit `path`, so the tests can point it anywhere.
fn peek_path_cache_disk_limit_gb_at(path: &Path) -> u32 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| path_cache_disk_limit_gb_in(&text))
        .unwrap_or(DEFAULT_PATH_CACHE_DISK_LIMIT_GB)
}

/// The `[settings] path_cache_disk_limit_gb` value in the settings document `text`, when it is
/// there and fits a `u32`.
fn path_cache_disk_limit_gb_in(text: &str) -> Option<u32> {
    toml::from_str::<toml::Table>(text)
        .ok()?
        .get("settings")
        .and_then(toml::Value::as_table)
        .and_then(|settings| settings.get("path_cache_disk_limit_gb"))
        .and_then(toml::Value::as_integer)
        .and_then(|gb| u32::try_from(gb).ok())
}

/// Loads the settings file and reports what happened, see [`SettingsLoadOutcome`].
/// Never panics and never propagates an error: a broken settings file must never block
/// startup.
#[must_use]
pub fn load_with_outcome(path: &Path) -> (SettingsFile, SettingsLoadOutcome) {
    let (mut file, outcome) = match std::fs::read_to_string(path) {
        Ok(contents) => match toml::from_str::<SettingsFile>(&contents) {
            Ok(file) => (file, SettingsLoadOutcome::Ok),
            Err(e) => {
                warn!(
                    "Settings file at {} is corrupt ({e}); falling back to defaults.",
                    path.display()
                );
                // Renamed before falling back, so the next save (which happens on almost
                // any interaction) cannot overwrite the corrupt file.
                let outcome = rename_aside(path).map_or(SettingsLoadOutcome::Ok, |renamed_to| {
                    SettingsLoadOutcome::Corrupt { renamed_to }
                });
                (defaults_for_existing_install(), outcome)
            }
        },
        // The ONE place a brand-new install is recognised: `SettingsFile::default()` is
        // the newcomer's start (the Simple interface, the welcome tour still to come).
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            info!(
                "No settings file at {} yet; using defaults.",
                path.display()
            );
            (SettingsFile::default(), SettingsLoadOutcome::Ok)
        }
        Err(e) => {
            warn!(
                "Could not read settings file at {} ({e}); falling back to defaults.",
                path.display()
            );
            (defaults_for_existing_install(), SettingsLoadOutcome::Ok)
        }
    };
    file.ensure_built_in_presets();
    // Older files may hold a `remote_workers` list; fold it into the single `remote`
    // endpoint (logged inside). The next save writes only `remote`.
    file.settings.migrate_legacy_remote_workers();
    (file, outcome)
}

/// Loads settings from `path` like [`load_with_outcome`], for tests that only need the
/// settings. Startup uses [`load_with_outcome`] to report a corrupt file.
///
/// Reading the file belongs to startup only: once the `SettingsPersister` is seeded from
/// it, the persister's in-memory snapshot is the source of truth, and a re-read mid-session
/// would miss changes still inside the debounce window.
#[cfg(test)]
#[must_use]
pub fn load_or_default(path: &Path) -> SettingsFile {
    load_with_outcome(path).0
}

/// Writes `settings` to `path` as pretty-printed TOML, creating the parent directory if
/// needed.
///
/// Writes a temporary sibling file and renames it into place, so a crash or power loss
/// mid-write cannot leave a corrupt settings file. `std::fs::rename` replaces the
/// destination atomically on Windows and Unix.
pub fn save(path: &Path, settings: &SettingsFile) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let toml_str = toml::to_string_pretty(settings)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let tmp_path = path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, toml_str)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests;
