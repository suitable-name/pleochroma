//! The one driver workaround of the GPU renderer.
//!
//! Mesa's RADV (the open AMD Vulkan driver) mis-schedules the megakernel, so zoned stones with a domain structure (opal) or oriented
//! inclusions render wrong (GPU totals about 15 % above the CPU, NaN in the all-features
//! generic kernel). The fault is in the driver's shader compiler (ACO) instruction
//! scheduler: `ACO_DEBUG=nosched` makes the same shaders correct, the stock driver and
//! `ACO_DEBUG=noopt` fail. There is no Mesa fix at the time of writing.
//!
//! This module holds everything the desktop app and the headless worker share about it, and
//! none of it needs a GPU or the `gpu` feature (it compiles everywhere; only
//! [`apply`] does anything, and only on Linux):
//!
//! * [`RADV_ACO_DEBUG`], the ONE constant naming the compiler debug value that works around
//!   the fault. Change it here and everything else follows.
//! * [`GpuDriverIdentity`], what the wgpu adapter says about its driver, with the
//!   [`is_radv`](GpuDriverIdentity::is_radv) decision. `GpuContext::acquire_async` records it
//!   once per process ([`record_adapter`]) so the desktop app can learn the driver without
//!   touching wgpu itself ([`recorded_adapter`]).
//! * [`plan`] / [`apply`], the start-up step: set `ACO_DEBUG` (unless the user set it) and
//!   point Mesa's on-disk shader cache at a folder of its own, so a shader compiled WITHOUT
//!   the workaround is never served from the cache. Mesa reads both variables when the
//!   Vulkan device is created, so [`apply`] must run before the first wgpu call, from
//!   `main`, while the process is single-threaded.
//! * [`restart_env_edits`], what a restarted child must NOT inherit from this process.

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    sync::OnceLock,
};

/// The environment variable of Mesa's AMD shader compiler (ACO) that carries debug options.
pub const ACO_DEBUG_ENV: &str = "ACO_DEBUG";

/// The environment variable that moves Mesa's on-disk shader cache.
pub const SHADER_CACHE_DIR_ENV: &str = "MESA_SHADER_CACHE_DIR";

/// The environment variable that switches Mesa's on-disk shader cache off.
pub const SHADER_CACHE_DISABLE_ENV: &str = "MESA_SHADER_CACHE_DISABLE";

/// The `ACO_DEBUG` value that works around the RADV scheduler fault: it turns off ACO's
/// instruction scheduler. THE one place this value is written down.
pub const RADV_ACO_DEBUG: &str = "nosched";

/// The folder (inside an app's cache folder) holding the Mesa shader cache of a run with the
/// workaround, named after [`RADV_ACO_DEBUG`] so a different workaround value never shares a
/// cache with this one.
#[must_use]
pub fn shader_cache_folder() -> String {
    format!("mesa-shader-cache-{RADV_ACO_DEBUG}")
}

/// The name of the app's own GPU pipeline-cache folder.
///
/// `pipeline_cache`, or `pipeline_cache-<value>` while the workaround is in force (a pipeline
/// cached from a stock compile must not be served to a workaround run, and the other way round).
#[must_use]
pub fn pipeline_cache_folder(workaround_active: bool) -> String {
    if workaround_active {
        format!("pipeline_cache-{RADV_ACO_DEBUG}")
    } else {
        "pipeline_cache".to_owned()
    }
}

/// The per-user cache folder of the platform: `%LOCALAPPDATA%`, `~/Library/Caches`,
/// `$XDG_CACHE_HOME` or `~/.cache`. `None` when none of those variables is set.
#[must_use]
pub fn platform_cache_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library").join("Caches"))
    } else if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        Some(PathBuf::from(xdg))
    } else {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache"))
    }
}

/// AMD's PCI vendor id, as the kernel's DRM sysfs `vendor` files spell it.
const AMD_PCI_VENDOR: &str = "0x1002";

/// Whether the `(PCI vendor, kernel driver)` pairs of a machine's DRM devices include an AMD GPU
/// on the `amdgpu` kernel driver: the only GPUs Mesa's RADV runs on.
#[must_use]
pub fn amdgpu_among<'a>(devices: impl IntoIterator<Item = (&'a str, Option<&'a str>)>) -> bool {
    devices.into_iter().any(|(vendor, driver)| {
        vendor.trim().eq_ignore_ascii_case(AMD_PCI_VENDOR) && driver == Some("amdgpu")
    })
}

/// Whether this machine has a GPU Mesa's RADV can drive.
///
/// Some `/sys/class/drm/card<N>` device has AMD's PCI vendor id and the `amdgpu` kernel driver.
/// Reads sysfs only (no Vulkan, no
/// thread), so it can decide the headless worker's default before the GPU is touched. Always
/// `false` off Linux.
///
/// A machine with an AMD GPU next to another one (an AMD iGPU beside an NVIDIA card) counts:
/// which adapter wgpu picks is not known before Vulkan starts, and the variables
/// [`apply`] sets are read by Mesa alone, so another vendor's driver ignores them.
#[must_use]
pub fn amd_gpu_present() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return false;
    };
    let devices: Vec<(String, Option<String>)> = entries
        .flatten()
        // `card0` is a GPU; `card0-DP-1` is one of its connectors.
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("card") && !name.contains('-'))
        })
        .map(|entry| {
            let device = entry.path().join("device");
            let vendor = std::fs::read_to_string(device.join("vendor")).unwrap_or_default();
            let driver = std::fs::read_link(device.join("driver"))
                .ok()
                .and_then(|link| link.file_name()?.to_str().map(str::to_owned));
            (vendor, driver)
        })
        .collect();
    amdgpu_among(
        devices
            .iter()
            .map(|(vendor, driver)| (vendor.as_str(), driver.as_deref())),
    )
}

/// Whether the debug-option string `value` of `ACO_DEBUG` (options separated by commas,
/// spaces, semicolons or colons) contains the option `flag`.
#[must_use]
pub fn aco_debug_has(value: &str, flag: &str) -> bool {
    value
        .split([',', ' ', ';', ':', '\t'])
        .any(|option| option == flag)
}

/// Whether the workaround is in force in THIS process right now: `ACO_DEBUG` (set by
/// [`apply`] or by the user) names [`RADV_ACO_DEBUG`]. Only meaningful on Linux.
#[must_use]
pub fn workaround_active() -> bool {
    std::env::var(ACO_DEBUG_ENV).is_ok_and(|value| aco_debug_has(&value, RADV_ACO_DEBUG))
}

// --- The adapter ---------------------------------------------------------------------------

/// What the wgpu adapter reports about its driver: the three strings of `wgpu::AdapterInfo`
/// that identify it, copied so no wgpu type leaks into the callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuDriverIdentity {
    /// The adapter name (`AMD Radeon 780M Graphics (RADV PHOENIX)`).
    pub name: String,
    /// The driver name (`radv` for Mesa's RADV).
    pub driver: String,
    /// The driver's version text (`Mesa 26.2.5`, optionally followed by ` (git-...)`).
    pub driver_info: String,
}

impl GpuDriverIdentity {
    /// Whether this is Mesa's RADV driver: the driver name is `radv`, or the adapter name
    /// carries `RADV` (RADV puts it in the device name).
    #[must_use]
    pub fn is_radv(&self) -> bool {
        self.driver.trim().eq_ignore_ascii_case("radv")
            || self.name.to_ascii_uppercase().contains("RADV")
    }

    /// The driver version as the driver spells it, without a trailing ` (git-...)` build
    /// note: `Mesa 26.2.5`. Empty when the driver gave no version text.
    #[must_use]
    pub fn version_label(&self) -> String {
        let info = self.driver_info.trim();
        info.split_once(" (")
            .map_or(info, |(version, _)| version)
            .trim()
            .to_owned()
    }

    /// The driver in one phrase for a message: `Mesa 26.2.5 (RADV)`, or `Mesa RADV (version
    /// unknown)` when the driver gave no version text.
    #[must_use]
    pub fn describe(&self) -> String {
        let version = self.version_label();
        if version.is_empty() {
            "Mesa RADV (version unknown)".to_owned()
        } else {
            format!("{version} (RADV)")
        }
    }
}

static ADAPTER: OnceLock<GpuDriverIdentity> = OnceLock::new();

/// Records the adapter this process acquired (the first one wins; later calls are ignored).
/// Called by `GpuContext::acquire_async` right after `request_adapter`.
pub fn record_adapter(name: &str, driver: &str, driver_info: &str) {
    let _ = ADAPTER.set(GpuDriverIdentity {
        name: name.to_owned(),
        driver: driver.to_owned(),
        driver_info: driver_info.to_owned(),
    });
}

/// The adapter [`record_adapter`] recorded, or `None` while no GPU was acquired (or in a
/// build without the `gpu` feature, which never acquires one).
#[must_use]
pub fn recorded_adapter() -> Option<&'static GpuDriverIdentity> {
    ADAPTER.get()
}

// --- The start-up step ---------------------------------------------------------------------

/// Where Mesa's shader cache goes while the workaround is in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShaderCache {
    /// A folder of its own (`MESA_SHADER_CACHE_DIR`).
    Folder(PathBuf),
    /// No usable folder: the cache is switched off (`MESA_SHADER_CACHE_DISABLE=true`), which
    /// is slower but never serves a shader compiled without the workaround.
    Disabled,
}

/// What [`plan`] / [`apply`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkaroundStatus {
    /// Nobody asked for the workaround (the setting is off or not yet answered).
    NotRequested,
    /// Not a Linux build: RADV exists only there.
    UnsupportedPlatform,
    /// The user set `ACO_DEBUG` themselves; their value stays and nothing else is touched.
    UserAcoDebug(String),
    /// The workaround is in force: `ACO_DEBUG` is [`RADV_ACO_DEBUG`] and the cache moved.
    Applied(ShaderCache),
}

impl WorkaroundStatus {
    /// Whether this process runs with the workaround ([`Self::Applied`], or a user value
    /// that already contains it).
    #[must_use]
    pub fn is_active(&self) -> bool {
        match self {
            Self::Applied(_) => true,
            Self::UserAcoDebug(value) => aco_debug_has(value, RADV_ACO_DEBUG),
            Self::NotRequested | Self::UnsupportedPlatform => false,
        }
    }

    /// One log line saying what happened.
    #[must_use]
    pub fn log_line(&self) -> String {
        match self {
            Self::NotRequested => "RADV scheduler workaround: not requested".to_owned(),
            Self::UnsupportedPlatform => {
                "RADV scheduler workaround: not applicable on this platform".to_owned()
            }
            Self::UserAcoDebug(value) => format!(
                "RADV scheduler workaround: {ACO_DEBUG_ENV}={value} was set by the user and is left alone"
            ),
            Self::Applied(ShaderCache::Folder(dir)) => format!(
                "RADV scheduler workaround on: {ACO_DEBUG_ENV}={RADV_ACO_DEBUG}, {SHADER_CACHE_DIR_ENV}={}",
                dir.display()
            ),
            Self::Applied(ShaderCache::Disabled) => format!(
                "RADV scheduler workaround on: {ACO_DEBUG_ENV}={RADV_ACO_DEBUG}, Mesa shader cache disabled (no cache folder)"
            ),
        }
    }
}

/// Decides what the start-up step does, without touching anything.
///
/// * `requested`: the stored setting says `on` (or the worker was told so).
/// * `linux`: whether this is a Linux build.
/// * `user_aco_debug`: the current value of `ACO_DEBUG`; an empty value counts as unset.
/// * `cache_root`: the app's own cache folder (`<cache>/pleochroma-cut`), or `None`.
#[must_use]
pub fn plan(
    requested: bool,
    linux: bool,
    user_aco_debug: Option<&OsStr>,
    cache_root: Option<&Path>,
) -> WorkaroundStatus {
    if !requested {
        return WorkaroundStatus::NotRequested;
    }
    if !linux {
        return WorkaroundStatus::UnsupportedPlatform;
    }
    if let Some(value) = user_aco_debug.filter(|value| !value.is_empty()) {
        return WorkaroundStatus::UserAcoDebug(value.to_string_lossy().into_owned());
    }
    WorkaroundStatus::Applied(cache_root.map_or(ShaderCache::Disabled, |root| {
        ShaderCache::Folder(root.join(shader_cache_folder()))
    }))
}

/// What [`apply`] changed, so a restarted child can be started without it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AppliedRecord {
    /// `MESA_SHADER_CACHE_DIR` before [`apply`] replaced it.
    previous_cache_dir: Option<OsString>,
    /// `MESA_SHADER_CACHE_DISABLE` before [`apply`] set it.
    previous_cache_disable: Option<OsString>,
}

static APPLIED: OnceLock<AppliedRecord> = OnceLock::new();

/// The start-up step: carries out the decision of [`plan`].
///
/// Reads the environment, creates the cache folder and sets the variables. Returns what
/// happened; log it with [`WorkaroundStatus::log_line`] once a log subscriber exists.
///
/// `cache_root` is the app's own cache folder (`<platform cache>/pleochroma-cut`).
///
/// Call it exactly once, first thing in `main`, before any thread starts and before wgpu or
/// Vulkan is touched: Mesa reads the variables when the Vulkan device is created, and
/// changing the environment is only sound while the process is single-threaded.
#[must_use]
pub fn apply(requested: bool, cache_root: Option<&Path>) -> WorkaroundStatus {
    let user = std::env::var_os(ACO_DEBUG_ENV);
    let mut status = plan(
        requested,
        cfg!(target_os = "linux"),
        user.as_deref(),
        cache_root,
    );
    if let WorkaroundStatus::Applied(cache) = &mut status {
        // A folder that cannot be made falls back to no cache at all: correctness first.
        if let ShaderCache::Folder(dir) = cache
            && let Err(error) = std::fs::create_dir_all(&*dir)
        {
            eprintln!(
                "pleochroma: could not create the Mesa shader cache folder {}: {error}; \
                 the Mesa shader cache is switched off instead",
                dir.display()
            );
            *cache = ShaderCache::Disabled;
        }
        let record = AppliedRecord {
            previous_cache_dir: std::env::var_os(SHADER_CACHE_DIR_ENV),
            previous_cache_disable: std::env::var_os(SHADER_CACHE_DISABLE_ENV),
        };
        // SAFETY: the caller runs this first thing in `main`, before the tracing subscriber,
        // the panic hook, the window or any thread exists, so nothing else can be reading or
        // writing the process environment while it changes (the same argument as the saved
        // UI scale's `SLINT_SCALE_FACTOR` in the desktop app's `main`).
        unsafe {
            std::env::set_var(ACO_DEBUG_ENV, RADV_ACO_DEBUG);
            match cache {
                ShaderCache::Folder(dir) => std::env::set_var(SHADER_CACHE_DIR_ENV, &*dir),
                ShaderCache::Disabled => std::env::set_var(SHADER_CACHE_DISABLE_ENV, "true"),
            }
        }
        let _ = APPLIED.set(record);
    }
    status
}

/// The environment edits a restarted child process needs to start from the user's own
/// environment again.
///
/// `(variable, value)` pairs where `None` means "remove it". Empty when
/// [`apply`] changed nothing (the variables the user set themselves are inherited as they
/// are). Without this, a child of a run WITH the workaround would find `ACO_DEBUG` set,
/// take it for the user's own and could never switch the workaround off.
#[must_use]
pub fn restart_env_edits() -> Vec<(&'static str, Option<OsString>)> {
    APPLIED.get().map_or_else(Vec::new, |record| {
        vec![
            (ACO_DEBUG_ENV, None),
            (SHADER_CACHE_DIR_ENV, record.previous_cache_dir.clone()),
            (
                SHADER_CACHE_DISABLE_ENV,
                record.previous_cache_disable.clone(),
            ),
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(name: &str, driver: &str, info: &str) -> GpuDriverIdentity {
        GpuDriverIdentity {
            name: name.to_owned(),
            driver: driver.to_owned(),
            driver_info: info.to_owned(),
        }
    }

    #[test]
    fn radv_is_recognised_by_driver_name_or_device_name() {
        assert!(
            identity(
                "AMD Radeon 780M Graphics (RADV PHOENIX)",
                "radv",
                "Mesa 26.2.5"
            )
            .is_radv()
        );
        assert!(identity("anything", "RADV", "").is_radv(), "any case");
        assert!(identity("AMD Radeon (RADV GFX1103_R1)", "", "").is_radv());
        assert!(!identity("NVIDIA GeForce RTX 4090", "NVIDIA", "550.54").is_radv());
        assert!(!identity("AMD Radeon RX 7900", "AMD proprietary driver", "2.0.300").is_radv());
        assert!(
            !identity(
                "Intel(R) Graphics (ADL GT2)",
                "Intel open-source Mesa driver",
                "Mesa 26.2.5"
            )
            .is_radv()
        );
        assert!(!identity("", "", "").is_radv());
    }

    #[test]
    fn the_version_text_loses_a_trailing_git_note() {
        let radv = |info: &str| identity("x (RADV Y)", "radv", info);
        assert_eq!(radv("Mesa 26.2.5").version_label(), "Mesa 26.2.5");
        assert_eq!(
            radv("Mesa 26.2.5 (git-1a2b3c)").version_label(),
            "Mesa 26.2.5"
        );
        assert_eq!(radv("  Mesa 26.2.5  ").version_label(), "Mesa 26.2.5");
        assert_eq!(radv("").version_label(), "");
        assert_eq!(radv("Mesa 26.2.5").describe(), "Mesa 26.2.5 (RADV)");
        assert_eq!(radv("").describe(), "Mesa RADV (version unknown)");
    }

    #[test]
    fn the_workaround_value_is_one_constant_everything_follows() {
        assert!(shader_cache_folder().ends_with(RADV_ACO_DEBUG));
        assert!(pipeline_cache_folder(true).ends_with(RADV_ACO_DEBUG));
        assert_eq!(pipeline_cache_folder(false), "pipeline_cache");
        assert_ne!(pipeline_cache_folder(true), pipeline_cache_folder(false));
    }

    #[test]
    fn only_an_amd_gpu_on_the_amdgpu_kernel_driver_counts() {
        assert!(amdgpu_among([("0x1002\n", Some("amdgpu"))]));
        assert!(amdgpu_among([
            ("0x10de\n", Some("nvidia")),
            ("0x1002\n", Some("amdgpu"))
        ]));
        assert!(!amdgpu_among([("0x10de\n", Some("nvidia"))]));
        assert!(!amdgpu_among([("0x8086\n", Some("i915"))]));
        // The old `radeon` kernel driver, or no driver bound: RADV does not run there.
        assert!(!amdgpu_among([("0x1002\n", Some("radeon"))]));
        assert!(!amdgpu_among([("0x1002\n", None)]));
        assert!(!amdgpu_among(std::iter::empty()));
    }

    #[test]
    fn aco_debug_options_are_found_among_separators() {
        assert!(aco_debug_has("nosched", "nosched"));
        assert!(aco_debug_has("validateir,nosched", "nosched"));
        assert!(aco_debug_has("validateir nosched", "nosched"));
        assert!(aco_debug_has("a:nosched;b", "nosched"));
        assert!(!aco_debug_has("noschedx", "nosched"));
        assert!(!aco_debug_has("noopt", "nosched"));
        assert!(!aco_debug_has("", "nosched"));
    }

    #[test]
    fn nothing_is_done_unless_requested_on_linux() {
        let root = Path::new("/home/u/.cache/pleochroma-cut");
        assert_eq!(
            plan(false, true, None, Some(root)),
            WorkaroundStatus::NotRequested
        );
        assert_eq!(
            plan(true, false, None, Some(root)),
            WorkaroundStatus::UnsupportedPlatform
        );
    }

    #[test]
    fn a_requested_workaround_sets_the_constant_and_a_cache_folder_of_its_own() {
        let root = Path::new("/home/u/.cache/pleochroma-cut");
        let status = plan(true, true, None, Some(root));
        assert_eq!(
            status,
            WorkaroundStatus::Applied(ShaderCache::Folder(root.join(shader_cache_folder())))
        );
        assert!(status.is_active());
        // An empty ACO_DEBUG is not the user's own.
        assert_eq!(plan(true, true, Some(OsStr::new("")), Some(root)), status);
        assert!(status.log_line().contains("nosched"));
    }

    #[test]
    fn without_a_cache_root_the_mesa_cache_is_switched_off() {
        let status = plan(true, true, None, None);
        assert_eq!(status, WorkaroundStatus::Applied(ShaderCache::Disabled));
        assert!(status.is_active());
        assert!(status.log_line().contains("cache disabled"));
    }

    #[test]
    fn a_value_the_user_set_is_respected_and_nothing_else_is_touched() {
        let root = Path::new("/home/u/.cache/pleochroma-cut");
        let status = plan(true, true, Some(OsStr::new("noopt")), Some(root));
        assert_eq!(status, WorkaroundStatus::UserAcoDebug("noopt".to_owned()));
        assert!(!status.is_active(), "noopt is not the workaround");
        assert!(status.log_line().contains("left alone"));

        let own = plan(true, true, Some(OsStr::new("nosched")), Some(root));
        assert!(own.is_active(), "the user already runs the workaround");
    }

    #[test]
    fn the_recorded_adapter_is_the_first_one() {
        // In a `gpu` build another test of this process may already have acquired the real GPU
        // (and recorded it): whichever record came first must survive a later one.
        record_adapter("first (RADV X)", "radv", "Mesa 1.0");
        let first = recorded_adapter().expect("an adapter was recorded").clone();
        record_adapter("second", "other", "2.0");
        assert_eq!(recorded_adapter(), Some(&first));
        assert_ne!(first.name, "second");
    }
}
