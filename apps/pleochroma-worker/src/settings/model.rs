//! The coordinator's settings file (`coordinator.toml`).
//!
//! Every key the first-run wizard writes and every field the Settings page edits, plus loading,
//! atomic saving, validation and the mapping to the real `serve` arguments.
//!
//! A settings file is plain TOML. Every field has a default, so a file written by the wizard
//! (eight keys) loads, and so does a file from an older version. A file that is not valid
//! (not TOML, a wrong type, a value out of range) is never edited: it is renamed to
//! `coordinator.toml.broken-<YYYYMMDD-HHMMSS>` and the coordinator starts with the defaults; the
//! outcome carries the sentence the window shows in its banner.

use crate::{
    cli::{self, ComputeMode, DEFAULT_MAX_CONNECTIONS, ServeArgs},
    pki,
};
use pleochroma_net::{
    admin::status::{NetworkCompression, RenderMode, SettingsPatch},
    messages::adaptive::PayloadChoice,
};
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
};

/// The first line of every file this module writes.
const FILE_HEADER: &str = "# Pleochroma coordinator settings. The window edits this file; a hand edit is kept\n# as long as it stays valid. A file that is not valid is kept under the name\n# coordinator.toml.broken-<date> and replaced.\n";

/// The lowest port number of a listener the owner may choose.
const MIN_PORT: u16 = 1024;

/// The highest base port: three more ports follow it.
const MAX_BASE_PORT: u16 = 65_532;

/// The most extra names a certificate may carry.
const MAX_CERT_NAMES: usize = 16;

/// The most addresses a saved worker connection may hold (a pairing link carries three).
const MAX_WORKER_ADDRESSES: usize = 3;

/// The longest display name or worker pin, in characters.
pub const MAX_NAME_CHARS: usize = 64;

/// The largest pixel-sample count a setting may hold (the settings file stores integers up to
/// 63 bits).
const MAX_WHOLE_IMAGE_PIXELS: u64 = 1 << 40;

/// What this computer does: run a coordinator, manage other ones, or lend its render power.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunMode {
    /// Run a coordinator here (`"here"`).
    #[default]
    Here,
    /// Only monitor or manage other coordinators (`"monitor"`).
    Monitor,
    /// Help another coordinator: no coordinator runs here, and the saved worker connection is
    /// rejoined when the program starts (`"worker"`).
    Worker,
}

/// Who may connect: the wizard's `"this-computer"` and `"network"`, and a chosen address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Listen {
    /// Only this computer (`127.0.0.1`).
    #[default]
    ThisComputer,
    /// Devices on the network (`0.0.0.0`).
    Network,
    /// One address, kept in `bind_address`.
    Specific,
}

/// What the own render lane renders with: the wizard's `"gpu-and-cpu"`, `"gpu"` and `"cpu"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RenderChoice {
    /// The graphics card and the processor.
    #[default]
    GpuAndCpu,
    /// The graphics card only.
    Gpu,
    /// The processor only.
    Cpu,
}

impl From<RenderMode> for RenderChoice {
    fn from(mode: RenderMode) -> Self {
        match mode {
            RenderMode::GpuAndCpu => Self::GpuAndCpu,
            RenderMode::GpuOnly => Self::Gpu,
            RenderMode::CpuOnly => Self::Cpu,
        }
    }
}

impl From<RenderChoice> for RenderMode {
    fn from(choice: RenderChoice) -> Self {
        match choice {
            RenderChoice::GpuAndCpu => Self::GpuAndCpu,
            RenderChoice::Gpu => Self::GpuOnly,
            RenderChoice::Cpu => Self::CpuOnly,
        }
    }
}

/// Everything the coordinator remembers between runs.
///
/// Every field has a default (the container is `#[serde(default)]`), so any subset of keys
/// loads. The fields of [`SettingsPatch`] can also change from another computer; the rest
/// (network, ports, certificate names, "Accept any client certificate", the data folder and the
/// per-user choices) are never remote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a settings file: the switches are independent"
)]
pub struct CoordinatorSettings {
    // ---- The wizard's keys.
    /// The coordinator's name; empty means "the computer's name".
    pub name: String,
    /// Whether a coordinator runs on this computer.
    pub run: RunMode,
    /// Who may connect.
    pub listen: Listen,
    /// The library database; `None` is "rendering only".
    pub library: Option<PathBuf>,
    /// "Render on this computer" (`--render`).
    pub render: bool,
    /// What the own lane renders with.
    pub render_mode: RenderChoice,
    /// Keep the computer awake while devices are connected or jobs run.
    pub keep_awake: bool,
    /// Start in the tray when this user logs in.
    pub start_at_login: bool,

    // ---- General (per user, never remote).
    /// Closing the window keeps the coordinator running in the tray.
    pub close_to_tray: bool,
    /// Check for a newer version in the background.
    pub update_check: bool,
    /// The high-contrast palette.
    pub high_contrast: bool,

    // ---- Network (never remote).
    /// The address to listen on when `listen` is [`Listen::Specific`] (no port).
    pub bind_address: String,
    /// The base port of the viewer listener; three more ports follow it.
    pub port: u16,
    /// The viewer port, when it is not `port`.
    pub viewer_port: Option<u16>,
    /// The viewer enrollment port, when it is not the viewer port plus one.
    pub viewer_enroll_port: Option<u16>,
    /// The render-computer port, when it is not the viewer port plus two.
    pub worker_port: Option<u16>,
    /// The render-computer enrollment port, when it is not the worker port plus one.
    pub worker_enroll_port: Option<u16>,
    /// The most connections open at once (per listener).
    pub max_connections: u32,
    /// The most unfinished connections from one address.
    pub max_per_address: u32,
    /// Extra DNS names and addresses the server certificate must be valid for.
    pub extra_cert_names: Vec<String>,

    // ---- Security.
    /// New devices may pair (off is `--no-enroll`).
    pub allow_pairing: bool,
    /// Skip the allowlist: any certificate signed by this coordinator's authority connects.
    /// Never remote; shown red under Advanced.
    pub accept_any_client_cert: bool,
    /// Announce the coordinator on the network.
    pub discovery: bool,
    /// Monitors and administrators on other computers may look after this coordinator
    /// (`serve --allow-remote-admin`). Off by default. Never remote: only this computer's
    /// window changes it.
    pub allow_remote_admin: bool,

    // ---- Performance (the safe fields of the patch).
    /// Most memory one job may use, in MiB.
    pub max_job_memory_mib: u32,
    /// Seconds below which a whole-image render stays on one computer.
    pub whole_image_seconds: u32,
    /// The same decision by width x height x samples, before a render computer has a rate.
    pub whole_image_pixels: u64,
    /// Jobs one device may have at once.
    pub jobs_per_device: u32,
    /// Render computers a live preview takes besides this one (`u32::MAX` is all).
    pub interactive_workers: u32,
    /// A render computer's name pinned for live previews; empty is no pin.
    pub pinned_worker: String,
    /// How traffic to devices is compressed.
    pub network_compression: NetworkCompression,
    /// Render computers may join (off is `--no-workers`).
    pub accept_render_computers: bool,
    /// CPU threads for rendering; 0 is automatic.
    pub cpu_threads: u32,
    /// CPU threads of the own lane; 0 means `cpu_threads`.
    pub render_threads: u32,

    // ---- Storage.
    /// Days a finished job's result is kept.
    pub job_retention_days: u32,
    /// Size of the asset cache, in MiB.
    pub cache_size_mib: u32,
    /// The data folder (`pki/`, `jobs/`, `asset-cache/`, `logs/`) when it is not the default one.
    pub data_folder: Option<PathBuf>,

    // ---- Help another coordinator (worker mode; per user, never remote).
    /// The other coordinator's render-computer addresses (`host:port`, at most three); empty
    /// when no connection is saved.
    pub worker_coordinator: Vec<String>,
    /// The other coordinator's name, for the window.
    pub worker_coordinator_name: String,
    /// The folder of this computer's worker certificate when it is not the per-user
    /// `worker-cert` folder.
    pub worker_cert_dir: Option<PathBuf>,
    /// Parallel connections to the other coordinator (tiles rendered at the same time).
    pub worker_slots: u32,
    /// What this computer renders with while it helps.
    pub worker_mode: RenderChoice,
    /// CPU threads while it helps; 0 is automatic.
    pub worker_threads: u32,
}

impl Default for CoordinatorSettings {
    fn default() -> Self {
        Self {
            name: String::new(),
            run: RunMode::Here,
            listen: Listen::ThisComputer,
            library: None,
            render: false,
            render_mode: RenderChoice::GpuAndCpu,
            keep_awake: true,
            start_at_login: false,
            close_to_tray: true,
            update_check: true,
            high_contrast: false,
            bind_address: String::new(),
            port: 7878,
            viewer_port: None,
            viewer_enroll_port: None,
            worker_port: None,
            worker_enroll_port: None,
            max_connections: u32::try_from(DEFAULT_MAX_CONNECTIONS).unwrap_or(64),
            max_per_address: u32::try_from(cli::DEFAULT_MAX_PREAUTH_PER_IP).unwrap_or(8),
            extra_cert_names: Vec::new(),
            allow_pairing: true,
            accept_any_client_cert: false,
            discovery: true,
            allow_remote_admin: false,
            max_job_memory_mib: cli::DEFAULT_MAX_JOB_MEMORY_MIB,
            whole_image_seconds: 2,
            whole_image_pixels: cli::DEFAULT_WHOLE_IMAGE_PIXEL_SAMPLES,
            jobs_per_device: cli::DEFAULT_JOBS_PER_VIEWER,
            interactive_workers: cli::ALL_INTERACTIVE_WORKERS,
            pinned_worker: String::new(),
            network_compression: NetworkCompression::Default,
            accept_render_computers: true,
            cpu_threads: 0,
            render_threads: 0,
            job_retention_days: 7,
            cache_size_mib: 2048,
            data_folder: None,
            worker_coordinator: Vec::new(),
            worker_coordinator_name: String::new(),
            worker_cert_dir: None,
            worker_slots: 2,
            worker_mode: RenderChoice::GpuAndCpu,
            worker_threads: 0,
        }
    }
}

/// What [`CoordinatorSettings::load`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadOutcome {
    /// The settings to use: the file's, or the defaults.
    pub settings: CoordinatorSettings,
    /// Plain words for the window's banner when the file could not be read; `None` otherwise
    /// (a missing file is not a problem).
    pub notice: Option<String>,
}

/// The four listener ports a set of settings resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectivePorts {
    /// Viewers (devices).
    pub viewer: u16,
    /// Viewer enrollment.
    pub viewer_enroll: u16,
    /// Render computers.
    pub worker: u16,
    /// Render-computer enrollment.
    pub worker_enroll: u16,
}

impl EffectivePorts {
    /// The four ports as a list, for the firewall check.
    #[must_use]
    pub const fn all(self) -> [u16; 4] {
        [
            self.viewer,
            self.viewer_enroll,
            self.worker,
            self.worker_enroll,
        ]
    }
}

impl CoordinatorSettings {
    /// Reads the settings file at `path`.
    ///
    /// A missing file gives the defaults and no notice. A file that is not valid (not TOML, a
    /// wrong type, a value out of range) is renamed to `<name>.broken-<YYYYMMDD-HHMMSS>` (UTC),
    /// the defaults are used, and the outcome's notice says so. A file that cannot be read at
    /// all (a locked file, a failing disk) is not renamed: the defaults are used for this run
    /// and the notice says the file was left as it is. Nothing is written back: the defaults
    /// reach the disk with the next change.
    #[must_use]
    pub fn load(path: &Path) -> LoadOutcome {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return LoadOutcome {
                    settings: Self::default(),
                    notice: None,
                };
            }
            // A read error says nothing about the content: keep the file where it is.
            Err(e) if e.kind() != std::io::ErrorKind::InvalidData => {
                return Self::unreadable(path, &e);
            }
            // Not UTF-8: the content is what is wrong.
            Err(_) => return Self::broken(path),
        };
        match toml::from_str::<Self>(&text) {
            Ok(settings) if settings.validate().is_ok() => LoadOutcome {
                settings,
                notice: None,
            },
            _ => Self::broken(path),
        }
    }

    /// The settings in the file at `path`, read without any side effect: `None` when the file is
    /// missing, unreadable or invalid (nothing is renamed and nothing is logged).
    ///
    /// For the few things needed before the real [`load`](Self::load), such as where the log folder
    /// is.
    #[must_use]
    pub fn peek(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        toml::from_str::<Self>(&text)
            .ok()
            .filter(|settings| settings.validate().is_ok())
    }

    /// Answers with the defaults for this run and leaves a file that could not be read alone.
    fn unreadable(path: &Path, error: &std::io::Error) -> LoadOutcome {
        let notice = format!(
            "Settings could not be read ({error}) and the defaults are used for this run; the file {} was left as it is.",
            path.display()
        );
        tracing::warn!("{notice}");
        LoadOutcome {
            settings: Self::default(),
            notice: Some(notice),
        }
    }

    /// Keeps the invalid file under a new name and answers with the defaults.
    fn broken(path: &Path) -> LoadOutcome {
        let kept = broken_name(path);
        let notice = if std::fs::rename(path, &kept).is_ok() {
            format!(
                "Settings could not be read and were reset; the old file was kept as {}.",
                kept.display()
            )
        } else {
            format!(
                "Settings could not be read and were reset for this run; the file {} was left as it is.",
                path.display()
            )
        };
        tracing::warn!("{notice}");
        LoadOutcome {
            settings: Self::default(),
            notice: Some(notice),
        }
    }

    /// Writes the settings to `path` atomically: a temporary file next to it, then a rename.
    ///
    /// A file that is there but is not valid (one [`load`](Self::load) left in place because it
    /// could not be read, or one edited by hand since) is first copied to
    /// `<name>.broken-<YYYYMMDD-HHMMSS>`, so the defaults never replace content nobody kept.
    ///
    /// # Errors
    ///
    /// A message in plain words when the folder or the file cannot be written, or when the
    /// file that is there cannot be copied aside (it is then not overwritten).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let body =
            toml::to_string_pretty(self).map_err(|e| format!("Could not write settings: {e}"))?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Could not create the settings folder: {e}"))?;
        }
        Self::keep_unusable_file(path)?;
        let mut temp_name = path.file_name().unwrap_or_default().to_os_string();
        temp_name.push(".tmp");
        let temp = path.with_file_name(temp_name);
        std::fs::write(&temp, format!("{FILE_HEADER}{body}"))
            .map_err(|e| format!("Could not save settings: {e}"))?;
        std::fs::rename(&temp, path).map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            format!("Could not save settings: {e}")
        })
    }

    /// Copies a file at `path` that is not valid settings to its `.broken-` name before a save
    /// replaces it. Nothing to do when there is no file or it is valid.
    fn keep_unusable_file(path: &Path) -> Result<(), String> {
        if !path.is_file() || Self::peek(path).is_some() {
            return Ok(());
        }
        let kept = broken_name(path);
        std::fs::copy(path, &kept).map(|_| ()).map_err(|e| {
            format!(
                "The settings file could not be read, and a copy of it could not be made first ({e}), so it was not replaced."
            )
        })?;
        tracing::warn!(
            "The settings file is not valid; the old file was kept as {}.",
            kept.display()
        );
        Ok(())
    }

    /// Whether this computer runs a coordinator (`run` is `"here"`; `"monitor"` and `"worker"`
    /// start none).
    #[must_use]
    pub fn runs_here(&self) -> bool {
        self.run == RunMode::Here
    }

    /// Whether the program rejoins another coordinator by itself: `run` is `"worker"` and a
    /// connection is saved.
    #[must_use]
    pub fn resumes_worker(&self) -> bool {
        self.run == RunMode::Worker && !self.worker_coordinator.is_empty()
    }

    /// The name the window shows: the setting, or the computer's name when it is empty.
    #[must_use]
    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            computer_name().unwrap_or_else(crate::control::status::default_display_name)
        } else {
            self.name.trim().to_owned()
        }
    }

    /// The data folder in use: the chosen one, or `default_dir`.
    #[must_use]
    pub fn data_dir(&self, default_dir: &Path) -> PathBuf {
        self.data_folder
            .clone()
            .unwrap_or_else(|| default_dir.to_path_buf())
    }

    /// The address the listeners bind (without a port).
    ///
    /// # Errors
    ///
    /// A message when the chosen address is not an IP address.
    pub fn bind_ip(&self) -> Result<IpAddr, String> {
        match self.listen {
            Listen::ThisComputer => Ok(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            Listen::Network => Ok(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            Listen::Specific => self.bind_address.trim().parse::<IpAddr>().map_err(|_| {
                "The address to listen on is not a valid address, such as 192.168.1.20.".to_owned()
            }),
        }
    }

    /// The four listener ports, with the same defaults `serve` derives from the viewer port.
    ///
    /// # Errors
    ///
    /// A message when a derived port would pass 65535.
    pub fn effective_ports(&self) -> Result<EffectivePorts, String> {
        let viewer = self.viewer_port.unwrap_or(self.port);
        let next = |base: u16, by: u16| {
            base.checked_add(by)
                .ok_or_else(|| "The port is too high: three more ports follow it.".to_owned())
        };
        let viewer_enroll = self
            .viewer_enroll_port
            .map_or_else(|| next(viewer, 1), Ok)?;
        let worker = self
            .worker_port
            .map_or_else(|| next(viewer, cli::WORKER_PORT_OFFSET), Ok)?;
        let worker_enroll = self
            .worker_enroll_port
            .map_or_else(|| next(worker, 1), Ok)?;
        Ok(EffectivePorts {
            viewer,
            viewer_enroll,
            worker,
            worker_enroll,
        })
    }

    /// Checks every value; the first problem comes back in plain words.
    ///
    /// # Errors
    ///
    /// A message naming the setting and its allowed range.
    pub fn validate(&self) -> Result<(), String> {
        check_text("The name", &self.name, true)?;
        check_text("The pinned render computer", &self.pinned_worker, true)?;
        range("The port", self.port, MIN_PORT, MAX_BASE_PORT)?;
        for (what, port) in [
            ("The viewer port", self.viewer_port),
            ("The viewer enrollment port", self.viewer_enroll_port),
            ("The render computer port", self.worker_port),
            (
                "The render computer enrollment port",
                self.worker_enroll_port,
            ),
        ] {
            if let Some(port) = port {
                range(what, port, MIN_PORT, u16::MAX)?;
            }
        }
        let ports = self.effective_ports()?.all();
        for (i, a) in ports.iter().enumerate() {
            if ports[i + 1..].contains(a) {
                return Err(format!(
                    "Port {a} is used twice. Each listener needs its own."
                ));
            }
        }
        self.bind_ip()?;
        range("Connections", self.max_connections, 1, 100_000)?;
        range("Connections per address", self.max_per_address, 1, 10_000)?;
        if self.extra_cert_names.len() > MAX_CERT_NAMES {
            return Err(format!(
                "A certificate can carry at most {MAX_CERT_NAMES} extra names."
            ));
        }
        for name in &self.extra_cert_names {
            check_cert_name(name)?;
        }
        range("Job memory", self.max_job_memory_mib, 64, 1_048_576)?;
        range("The whole-image time", self.whole_image_seconds, 1, 3600)?;
        range(
            "The whole-image size",
            self.whole_image_pixels,
            1,
            MAX_WHOLE_IMAGE_PIXELS,
        )?;
        range("Jobs per device", self.jobs_per_device, 1, 256)?;
        range("CPU threads", self.cpu_threads, 0, 1024)?;
        range("CPU threads of this computer", self.render_threads, 0, 1024)?;
        range("Job retention (days)", self.job_retention_days, 1, 3650)?;
        range("The cache size (MiB)", self.cache_size_mib, 64, 4_194_304)?;
        self.validate_worker()
    }

    /// The worker-mode fields.
    fn validate_worker(&self) -> Result<(), String> {
        range("Slots", self.worker_slots, 1, 64)?;
        range("CPU threads while helping", self.worker_threads, 0, 1024)?;
        check_text(
            "The coordinator's name",
            &self.worker_coordinator_name,
            true,
        )?;
        if self.worker_coordinator.len() > MAX_WORKER_ADDRESSES {
            return Err(format!(
                "A saved connection can have at most {MAX_WORKER_ADDRESSES} addresses."
            ));
        }
        for address in &self.worker_coordinator {
            let ok = address.len() <= 260
                && address
                    .rsplit_once(':')
                    .is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok())
                && !address
                    .chars()
                    .any(|c| c.is_control() || c == ',' || c == ' ');
            if !ok {
                return Err(format!(
                    "\"{address}\" is not a coordinator address. Use host:port, such as 192.168.1.20:7880."
                ));
            }
        }
        Ok(())
    }

    /// The settings after `patch`: every set field is validated first, and nothing changes on an
    /// error. The coordinator applies this to a patch from the window or from another computer.
    ///
    /// # Errors
    ///
    /// A message in plain words naming the field and its range.
    pub fn with_patch(&self, patch: &SettingsPatch) -> Result<Self, String> {
        let mut next = self.clone();
        if let Some(v) = patch.max_job_memory_mib {
            next.max_job_memory_mib = v;
        }
        if let Some(v) = patch.whole_image_seconds {
            next.whole_image_seconds = v;
        }
        if let Some(v) = patch.whole_image_pixels {
            next.whole_image_pixels = v;
        }
        if let Some(v) = patch.jobs_per_device {
            next.jobs_per_device = v;
        }
        if let Some(v) = patch.interactive_workers {
            next.interactive_workers = v;
        }
        if let Some(v) = &patch.pinned_worker {
            v.trim().clone_into(&mut next.pinned_worker);
        }
        if let Some(v) = patch.network_compression {
            next.network_compression = v;
        }
        if let Some(v) = patch.accept_render_computers {
            next.accept_render_computers = v;
        }
        if let Some(v) = patch.cpu_threads {
            next.cpu_threads = v;
        }
        if let Some(v) = patch.job_retention_days {
            next.job_retention_days = v;
        }
        if let Some(v) = patch.cache_size_mib {
            next.cache_size_mib = v;
        }
        if let Some(v) = patch.allow_pairing {
            next.allow_pairing = v;
        }
        if let Some(v) = patch.discovery {
            next.discovery = v;
        }
        if let Some(v) = patch.render_enabled {
            next.render = v;
        }
        if let Some(v) = patch.render_mode {
            next.render_mode = v.into();
        }
        if let Some(v) = patch.render_threads {
            next.render_threads = v;
        }
        if let Some(v) = &patch.display_name {
            let name = v.trim();
            if name.is_empty() {
                return Err("The name cannot be empty.".to_owned());
            }
            name.clone_into(&mut next.name);
        }
        next.validate()?;
        Ok(next)
    }

    /// The groups of settings that differ between `self` and `other`, for the activity line
    /// ("performance, storage"). `"settings"` when none of the groups differ.
    #[must_use]
    pub fn summary_of_change(&self, other: &Self) -> String {
        let groups = [
            ("general", self.general_differs(other)),
            ("network", self.network_differs(other)),
            ("security", self.security_differs(other)),
            ("performance", self.performance_differs(other)),
            (
                "render on this computer",
                (self.render, self.render_mode) != (other.render, other.render_mode),
            ),
            ("storage", self.storage_differs(other)),
            ("help another coordinator", self.worker_differs(other)),
        ];
        let changed: Vec<&str> = groups
            .iter()
            .filter(|(_, differs)| *differs)
            .map(|(name, _)| *name)
            .collect();
        if changed.is_empty() {
            "settings".to_owned()
        } else {
            changed.join(", ")
        }
    }

    /// Whether the general group (name, mode, awake, login, contrast, tray, update check)
    /// differs.
    fn general_differs(&self, b: &Self) -> bool {
        (
            &self.name,
            self.run,
            self.keep_awake,
            self.start_at_login,
            self.high_contrast,
            self.close_to_tray,
            self.update_check,
        ) != (
            &b.name,
            b.run,
            b.keep_awake,
            b.start_at_login,
            b.high_contrast,
            b.close_to_tray,
            b.update_check,
        )
    }

    /// Whether the network group (address, ports, discovery, limits) differs.
    fn network_differs(&self, b: &Self) -> bool {
        (
            self.listen,
            &self.bind_address,
            self.port,
            self.discovery,
            self.max_connections,
            self.max_per_address,
        ) != (
            b.listen,
            &b.bind_address,
            b.port,
            b.discovery,
            b.max_connections,
            b.max_per_address,
        ) || (
            self.viewer_port,
            self.viewer_enroll_port,
            self.worker_port,
            self.worker_enroll_port,
        ) != (
            b.viewer_port,
            b.viewer_enroll_port,
            b.worker_port,
            b.worker_enroll_port,
        )
    }

    /// Whether the security group (pairing, trust, certificate names, remote administration)
    /// differs.
    fn security_differs(&self, b: &Self) -> bool {
        (
            self.allow_pairing,
            self.accept_any_client_cert,
            &self.extra_cert_names,
            self.allow_remote_admin,
        ) != (
            b.allow_pairing,
            b.accept_any_client_cert,
            &b.extra_cert_names,
            b.allow_remote_admin,
        )
    }

    /// Whether the performance group differs.
    fn performance_differs(&self, b: &Self) -> bool {
        (
            self.max_job_memory_mib,
            self.whole_image_seconds,
            self.whole_image_pixels,
            self.jobs_per_device,
            self.interactive_workers,
            &self.pinned_worker,
        ) != (
            b.max_job_memory_mib,
            b.whole_image_seconds,
            b.whole_image_pixels,
            b.jobs_per_device,
            b.interactive_workers,
            &b.pinned_worker,
        ) || (
            self.network_compression,
            self.accept_render_computers,
            self.cpu_threads,
            self.render_threads,
        ) != (
            b.network_compression,
            b.accept_render_computers,
            b.cpu_threads,
            b.render_threads,
        )
    }

    /// Whether the storage group (retention, cache, data folder, library) differs.
    fn storage_differs(&self, b: &Self) -> bool {
        (
            self.job_retention_days,
            self.cache_size_mib,
            &self.data_folder,
            &self.library,
        ) != (
            b.job_retention_days,
            b.cache_size_mib,
            &b.data_folder,
            &b.library,
        )
    }

    /// Whether the "help another coordinator" group differs.
    fn worker_differs(&self, b: &Self) -> bool {
        (
            &self.worker_coordinator,
            &self.worker_coordinator_name,
            &self.worker_cert_dir,
            self.worker_slots,
            self.worker_mode,
            self.worker_threads,
        ) != (
            &b.worker_coordinator,
            &b.worker_coordinator_name,
            &b.worker_cert_dir,
            b.worker_slots,
            b.worker_mode,
            b.worker_threads,
        )
    }

    /// The extra certificate names as the one text line the Settings page edits.
    #[must_use]
    pub fn cert_names_text(&self) -> String {
        self.extra_cert_names.join(", ")
    }

    /// The `serve` arguments these settings mean.
    ///
    /// `data_dir` is the folder `pki/` lives in when no data folder is chosen: the per-user folder
    /// in the window, the working directory of a headless run.
    ///
    /// Storage (job folder, retention, cache folder and size) has no `serve` flag: it is read
    /// through [`crate::settings::storage_overrides`], and an environment variable still wins.
    ///
    /// # Errors
    ///
    /// A message when a setting is invalid, or when the command-line defaults cannot be built
    /// (an internal fault).
    pub fn serve_args(&self, data_dir: &Path) -> Result<ServeArgs, String> {
        self.validate()?;
        let mut args = default_serve_args()?;
        let data = self.data_dir(data_dir);
        let pki_dir = data.join("pki");
        args.ca = Some(pki_dir.join(pki::CA_CERT_FILE));
        args.cert = Some(pki_dir.join(pki::SERVER_CERT_FILE));
        args.key = Some(pki_dir.join(pki::SERVER_KEY_FILE));

        let ip = self.bind_ip()?;
        let ports = self.effective_ports()?;
        args.bind = SocketAddr::new(ip, ports.viewer).to_string();
        args.allow_remote = !ip.is_loopback();
        args.enroll_bind = self
            .viewer_enroll_port
            .map(|p| SocketAddr::new(ip, p).to_string());
        args.worker_bind = self.worker_port.map(|p| SocketAddr::new(ip, p).to_string());
        args.worker_enroll_bind = self
            .worker_enroll_port
            .map(|p| SocketAddr::new(ip, p).to_string());

        args.no_enroll = !self.allow_pairing;
        args.no_workers = !self.accept_render_computers;
        args.trust_any_client_cert = self.accept_any_client_cert;
        args.allow_remote_admin = self.allow_remote_admin;
        // "Rendering only" is an explicit state: no library is served, and nothing falls back
        // to `facet_diagrams.sqlite` in the working directory.
        args.db.clone_from(&self.library);
        args.no_library = self.library.is_none();

        args.render = self.render;
        // "GPU only" needs a build with the GPU renderer; without it the lane stays hybrid.
        args.compute_mode = match self.render_mode {
            RenderChoice::Gpu if cfg!(feature = "gpu") => ComputeMode::OnlyGpu,
            RenderChoice::Cpu => ComputeMode::OnlyCpu,
            RenderChoice::GpuAndCpu | RenderChoice::Gpu => ComputeMode::Hybrid,
        };
        let threads = if self.render_threads == 0 {
            self.cpu_threads
        } else {
            self.render_threads
        };
        args.threads = usize::try_from(threads).unwrap_or(0);

        args.max_connections = to_usize(self.max_connections);
        args.max_preauth_per_ip = to_usize(self.max_per_address);
        args.interactive_workers = self.interactive_workers;
        args.pin_interactive_worker =
            Some(self.pinned_worker.trim().to_owned()).filter(|name| !name.is_empty());
        args.max_job_memory_mib = self.max_job_memory_mib;
        args.whole_image_secs = f64::from(self.whole_image_seconds);
        args.whole_image_pixel_samples = self.whole_image_pixels;
        args.jobs_per_viewer = self.jobs_per_device;
        args.payload_encoding = match self.network_compression {
            NetworkCompression::Default => PayloadChoice::Auto,
            NetworkCompression::Fast => "lz4".parse().unwrap_or_default(),
            NetworkCompression::Off => "raw".parse().unwrap_or_default(),
        };
        Ok(args)
    }
}

/// The computer's own name, or `None` when it cannot be found.
///
/// Windows has `COMPUTERNAME`. Elsewhere `HOSTNAME` is usually not exported to a program
/// (it is a shell variable), so the kernel's host name file, `/etc/hostname` and the
/// `hostname` command are tried in turn.
#[must_use]
pub fn computer_name() -> Option<String> {
    fn clean(text: &str) -> Option<String> {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_owned())
    }
    if cfg!(windows) {
        return std::env::var("COMPUTERNAME")
            .ok()
            .and_then(|text| clean(&text));
    }
    if let Some(name) = std::env::var("HOSTNAME").ok().and_then(|text| clean(&text)) {
        return Some(name);
    }
    for file in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Some(name) = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| clean(&text))
        {
            return Some(name);
        }
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|text| clean(&text))
}

/// `serve` with every option at its default, taken from the command-line parser so the two
/// never drift apart.
fn default_serve_args() -> Result<ServeArgs, String> {
    match cli::parse(&["serve".to_owned()]) {
        Ok(cli::Command::Serve(args)) => Ok(*args),
        Ok(_) => Err("internal error: `serve` did not parse to the serve command".to_owned()),
        Err(e) => Err(format!("internal error: {e}")),
    }
}

/// A `u32` as a `usize` (every supported platform has at least 32 bits).
fn to_usize(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// `Ok` when `lo <= value <= hi`, otherwise a sentence naming `what` and the range.
fn range<T: PartialOrd + Copy + std::fmt::Display>(
    what: &str,
    value: T,
    lo: T,
    hi: T,
) -> Result<(), String> {
    if value < lo || value > hi {
        Err(format!("{what} must be between {lo} and {hi}."))
    } else {
        Ok(())
    }
}

/// A short single-line text: no control characters, at most [`MAX_NAME_CHARS`] characters.
fn check_text(what: &str, text: &str, may_be_empty: bool) -> Result<(), String> {
    if !may_be_empty && text.trim().is_empty() {
        return Err(format!("{what} cannot be empty."));
    }
    if text.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "{what} can have at most {MAX_NAME_CHARS} characters."
        ));
    }
    if text.chars().any(char::is_control) {
        return Err(format!("{what} cannot contain control characters."));
    }
    Ok(())
}

/// One extra certificate name: a DNS name or an IP address, letters, digits, `.`, `-` and `:`.
fn check_cert_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 253
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "\"{name}\" is not a valid name. Use a DNS name or an address, such as studio.example.com or 10.0.0.5."
        ))
    }
}

/// Splits the page's one-line list ("a.example.com, 10.0.0.5") into validated names, without
/// duplicates. Commas, semicolons and spaces separate.
///
/// # Errors
///
/// A message naming the first name that is not valid, or saying there are too many.
pub fn parse_cert_names(text: &str) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = Vec::new();
    for part in text.split([',', ';', ' ', '\n', '\t']) {
        let name = part.trim();
        if name.is_empty() {
            continue;
        }
        check_cert_name(name)?;
        if !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            names.push(name.to_owned());
        }
    }
    if names.len() > MAX_CERT_NAMES {
        return Err(format!(
            "A certificate can carry at most {MAX_CERT_NAMES} extra names."
        ));
    }
    Ok(names)
}

/// The new name of an unreadable settings file: `<name>.broken-<YYYYMMDD-HHMMSS>` (UTC), with
/// `-2`, `-3` ... added when that name is taken.
fn broken_name(path: &Path) -> PathBuf {
    let now = time::OffsetDateTime::now_utc();
    let stamp = format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let file = path.file_name().map_or_else(
        || "coordinator.toml".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let mut candidate = path.with_file_name(format!("{file}.broken-{stamp}"));
    let mut n = 2_u32;
    while candidate.exists() {
        candidate = path.with_file_name(format!("{file}.broken-{stamp}-{n}"));
        n += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pleochroma-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// A file that is there but not valid (as after an unreadable load) is copied aside before
    /// the first save replaces it; a valid file is not copied.
    #[test]
    fn an_unusable_file_is_copied_aside_before_the_first_save() {
        let dir = temp_dir("keep-unusable");
        let file = dir.join("coordinator.toml");
        let kept_files = || -> Vec<PathBuf> {
            std::fs::read_dir(&dir)
                .expect("read dir")
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().contains(".broken-"))
                })
                .collect()
        };
        let original = "this is [not valid toml";
        std::fs::write(&file, original).expect("write garbage");
        CoordinatorSettings::default().save(&file).expect("save");
        let kept = kept_files();
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(std::fs::read_to_string(&kept[0]).expect("copy"), original);
        assert_eq!(CoordinatorSettings::load(&file).notice, None);
        // The file is valid now: a second save keeps no further copy.
        CoordinatorSettings::default().save(&file).expect("save");
        assert_eq!(kept_files().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn serde_round_trips_every_field() {
        let mut settings = CoordinatorSettings {
            name: "Studio PC".to_owned(),
            listen: Listen::Specific,
            bind_address: "192.168.1.20".to_owned(),
            library: Some(PathBuf::from("D:/Gems/facet_diagrams.sqlite")),
            render: true,
            render_mode: RenderChoice::Cpu,
            viewer_port: Some(9000),
            worker_port: Some(9100),
            extra_cert_names: vec!["studio.example.com".to_owned()],
            allow_pairing: false,
            accept_any_client_cert: true,
            allow_remote_admin: true,
            network_compression: NetworkCompression::Fast,
            interactive_workers: 3,
            data_folder: Some(PathBuf::from("D:/Pleochroma")),
            ..CoordinatorSettings::default()
        };
        settings.cpu_threads = 6;
        let dir = temp_dir("round-trip");
        let file = dir.join("coordinator.toml");
        settings.save(&file).expect("save");
        let loaded = CoordinatorSettings::load(&file);
        assert_eq!(loaded.notice, None);
        assert_eq!(loaded.settings, settings);
        // No temporary file is left behind.
        assert!(!dir.join("coordinator.toml.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wizard_file_loads() {
        let text = "# Written by the first-run wizard.\nname = \"Studio PC\"\nrun = \"here\"\n\
                    listen = \"network\"\nlibrary = \"D:/Gems/facet_diagrams.sqlite\"\n\
                    render = true\nrender_mode = \"gpu\"\nkeep_awake = false\n\
                    start_at_login = true\n";
        let dir = temp_dir("wizard");
        let file = dir.join("coordinator.toml");
        std::fs::write(&file, text).expect("write");
        let outcome = CoordinatorSettings::load(&file);
        assert_eq!(outcome.notice, None);
        let s = outcome.settings;
        assert_eq!(s.name, "Studio PC");
        assert!(s.runs_here());
        assert_eq!(s.listen, Listen::Network);
        assert_eq!(s.render_mode, RenderChoice::Gpu);
        assert!(s.render && s.start_at_login && !s.keep_awake);
        assert_eq!(s.port, 7878);
        assert!(s.allow_pairing && s.discovery);
        let monitor = "run = \"monitor\"\n";
        std::fs::write(&file, monitor).expect("write");
        assert!(!CoordinatorSettings::load(&file).settings.runs_here());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_saved_worker_connection_round_trips_and_resumes() {
        let settings = CoordinatorSettings {
            run: RunMode::Worker,
            worker_coordinator: vec!["192.168.178.40:7880".to_owned()],
            worker_coordinator_name: "Garage coordinator".to_owned(),
            worker_cert_dir: Some(PathBuf::from("D:/Pleochroma/worker-cert")),
            worker_slots: 4,
            worker_mode: RenderChoice::Gpu,
            worker_threads: 8,
            ..CoordinatorSettings::default()
        };
        assert!(!settings.runs_here());
        assert!(settings.resumes_worker());
        let dir = temp_dir("worker-mode");
        let file = dir.join("coordinator.toml");
        settings.save(&file).expect("save");
        let loaded = CoordinatorSettings::load(&file);
        assert_eq!(loaded.notice, None);
        assert_eq!(loaded.settings, settings);
        let text = std::fs::read_to_string(&file).expect("read");
        assert!(text.contains("run = \"worker\""), "{text}");
        // Worker mode without a saved connection has nothing to rejoin.
        let empty = CoordinatorSettings {
            run: RunMode::Worker,
            ..CoordinatorSettings::default()
        };
        assert!(!empty.resumes_worker());
        // A file from before worker mode loads with the worker defaults.
        assert_eq!(CoordinatorSettings::default().worker_slots, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn worker_addresses_are_checked() {
        let mut bad = CoordinatorSettings::default();
        for address in ["nonsense", "host:99999", ":7880", "a b:7880", "a,b:7880"] {
            bad.worker_coordinator = vec![address.to_owned()];
            assert!(bad.validate().is_err(), "{address}");
        }
        bad.worker_coordinator = vec!["10.0.0.5:7880".to_owned(); 4];
        assert!(bad.validate().is_err());
        bad.worker_coordinator = vec!["10.0.0.5:7880".to_owned()];
        bad.worker_slots = 0;
        assert!(bad.validate().is_err());
        bad.worker_slots = 2;
        assert!(bad.validate().is_ok());
    }

    #[test]
    fn a_missing_file_is_the_defaults_without_a_notice() {
        let dir = temp_dir("missing");
        let outcome = CoordinatorSettings::load(&dir.join("coordinator.toml"));
        assert_eq!(outcome.settings, CoordinatorSettings::default());
        assert_eq!(outcome.notice, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_is_kept_and_replaced_by_defaults() {
        let dir = temp_dir("broken");
        let file = dir.join("coordinator.toml");
        std::fs::write(&file, "name = = nonsense [[[").expect("write");
        let outcome = CoordinatorSettings::load(&file);
        assert_eq!(outcome.settings, CoordinatorSettings::default());
        let notice = outcome.notice.expect("a notice");
        assert!(
            notice.starts_with(
                "Settings could not be read and were reset; the old file was kept as "
            )
        );
        assert!(notice.contains("coordinator.toml.broken-"));
        assert!(!file.exists());
        let kept: Vec<_> = std::fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(kept.len(), 1);
        assert!(kept[0].starts_with("coordinator.toml.broken-"));
        assert_eq!(
            std::fs::read_to_string(dir.join(&kept[0])).expect("kept"),
            "name = = nonsense [[["
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A read error (here: a folder where the file should be) is not a verdict on the content:
    /// the defaults are used for this run, and nothing is renamed.
    #[test]
    fn a_file_that_cannot_be_read_is_left_alone() {
        let dir = temp_dir("unreadable");
        let file = dir.join("coordinator.toml");
        std::fs::create_dir_all(&file).expect("blocker");
        let outcome = CoordinatorSettings::load(&file);
        assert_eq!(outcome.settings, CoordinatorSettings::default());
        let notice = outcome.notice.expect("a notice");
        assert!(notice.contains("left as it is"), "{notice}");
        assert!(file.is_dir(), "the file is still where it was");
        let names: Vec<_> = std::fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .collect();
        assert_eq!(names.len(), 1, "nothing was renamed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_value_out_of_range_counts_as_broken() {
        let dir = temp_dir("range");
        let file = dir.join("coordinator.toml");
        std::fs::write(&file, "port = 80\n").expect("write");
        let outcome = CoordinatorSettings::load(&file);
        assert!(outcome.notice.is_some());
        assert_eq!(outcome.settings.port, 7878);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn serve_args_default_is_this_computer_only() {
        let settings = CoordinatorSettings::default();
        let args = settings.serve_args(Path::new("/data")).expect("args");
        assert_eq!(args.bind, cli::DEFAULT_BIND);
        assert!(!args.allow_remote);
        assert!(!args.render && !args.no_enroll && !args.no_workers);
        // No library chosen: "rendering only", never the working directory's file.
        assert!(args.no_library && args.db.is_none());
        assert!(!args.trust_any_client_cert);
        assert!(
            !args.allow_remote_admin,
            "remote administration is off by default"
        );
        assert_eq!(args.max_connections, 64);
        assert_eq!(
            args.ca,
            Some(PathBuf::from("/data").join("pki").join(pki::CA_CERT_FILE))
        );
        assert_eq!(
            args.key,
            Some(
                PathBuf::from("/data")
                    .join("pki")
                    .join(pki::SERVER_KEY_FILE)
            )
        );
        assert_eq!(args.payload_encoding, PayloadChoice::Auto);
    }

    #[test]
    fn remote_administration_is_off_by_default_maps_to_serve_and_survives_a_file_without_it() {
        assert!(!CoordinatorSettings::default().allow_remote_admin);
        let on = CoordinatorSettings {
            allow_remote_admin: true,
            ..CoordinatorSettings::default()
        };
        assert!(
            on.serve_args(Path::new("/data"))
                .expect("args")
                .allow_remote_admin
        );
        // It is a security change, so the activity line says so.
        assert!(
            CoordinatorSettings::default()
                .summary_of_change(&on)
                .contains("security")
        );
        // A settings file from an older version has no such key and loads with it off.
        let older: CoordinatorSettings = toml::from_str("name = \"Studio\"\n").expect("loads");
        assert!(!older.allow_remote_admin);
        // The patch other computers send cannot carry it: applying one leaves it as it is.
        let patched = on
            .with_patch(&SettingsPatch {
                job_retention_days: Some(30),
                ..SettingsPatch::default()
            })
            .expect("valid");
        assert!(patched.allow_remote_admin);
    }

    #[test]
    fn serve_args_maps_every_setting() {
        let settings = CoordinatorSettings {
            listen: Listen::Network,
            port: 9000,
            worker_port: Some(9500),
            library: Some(PathBuf::from("lib.sqlite")),
            render: true,
            render_mode: RenderChoice::Cpu,
            cpu_threads: 4,
            allow_pairing: false,
            accept_render_computers: false,
            accept_any_client_cert: true,
            max_connections: 10,
            max_per_address: 2,
            interactive_workers: 1,
            pinned_worker: "garage".to_owned(),
            max_job_memory_mib: 512,
            whole_image_seconds: 5,
            jobs_per_device: 3,
            network_compression: NetworkCompression::Off,
            data_folder: Some(PathBuf::from("/other")),
            ..CoordinatorSettings::default()
        };
        let args = settings.serve_args(Path::new("/data")).expect("args");
        assert_eq!(args.bind, "0.0.0.0:9000");
        assert!(args.allow_remote);
        assert_eq!(args.worker_bind.as_deref(), Some("0.0.0.0:9500"));
        assert_eq!(args.enroll_bind, None);
        assert_eq!(args.db, Some(PathBuf::from("lib.sqlite")));
        assert!(!args.no_library);
        assert!(args.render);
        assert_eq!(args.compute_mode, ComputeMode::OnlyCpu);
        assert_eq!(args.threads, 4);
        assert!(args.no_enroll && args.no_workers && args.trust_any_client_cert);
        assert_eq!((args.max_connections, args.max_preauth_per_ip), (10, 2));
        assert_eq!(args.interactive_workers, 1);
        assert_eq!(args.pin_interactive_worker.as_deref(), Some("garage"));
        assert_eq!(args.max_job_memory_mib, 512);
        assert!((args.whole_image_secs - 5.0).abs() < f64::EPSILON);
        assert_eq!(args.jobs_per_viewer, 3);
        assert_eq!(
            args.payload_encoding,
            "raw".parse::<PayloadChoice>().expect("raw parses")
        );
        assert_eq!(
            args.ca,
            Some(PathBuf::from("/other").join("pki").join(pki::CA_CERT_FILE))
        );
    }

    #[test]
    fn a_specific_address_binds_there() {
        let settings = CoordinatorSettings {
            listen: Listen::Specific,
            bind_address: "192.168.1.20".to_owned(),
            ..CoordinatorSettings::default()
        };
        let args = settings.serve_args(Path::new("/d")).expect("args");
        assert_eq!(args.bind, "192.168.1.20:7878");
        assert!(args.allow_remote);
        let bad = CoordinatorSettings {
            listen: Listen::Specific,
            bind_address: "not an address".to_owned(),
            ..CoordinatorSettings::default()
        };
        assert!(bad.serve_args(Path::new("/d")).is_err());
        let loopback = CoordinatorSettings {
            listen: Listen::Specific,
            bind_address: "127.0.0.1".to_owned(),
            ..CoordinatorSettings::default()
        };
        assert!(
            !loopback
                .serve_args(Path::new("/d"))
                .expect("args")
                .allow_remote
        );
    }

    #[test]
    fn effective_ports_follow_the_viewer_port() {
        let settings = CoordinatorSettings {
            port: 9000,
            worker_enroll_port: Some(9990),
            ..CoordinatorSettings::default()
        };
        let ports = settings.effective_ports().expect("ports");
        assert_eq!(ports.all(), [9000, 9001, 9002, 9990]);
        let clash = CoordinatorSettings {
            viewer_enroll_port: Some(7880),
            ..CoordinatorSettings::default()
        };
        assert!(clash.validate().is_err());
    }

    #[test]
    fn a_patch_is_validated_and_changes_nothing_on_error() {
        let base = CoordinatorSettings::default();
        let good = SettingsPatch {
            max_job_memory_mib: Some(4096),
            display_name: Some("  Garage  ".to_owned()),
            render_mode: Some(RenderMode::GpuOnly),
            allow_pairing: Some(false),
            ..SettingsPatch::default()
        };
        let next = base.with_patch(&good).expect("valid");
        assert_eq!(next.max_job_memory_mib, 4096);
        assert_eq!(next.name, "Garage");
        assert_eq!(next.render_mode, RenderChoice::Gpu);
        assert!(!next.allow_pairing);

        for bad in [
            SettingsPatch {
                display_name: Some("   ".to_owned()),
                ..SettingsPatch::default()
            },
            SettingsPatch {
                max_job_memory_mib: Some(1),
                ..SettingsPatch::default()
            },
            SettingsPatch {
                jobs_per_device: Some(0),
                ..SettingsPatch::default()
            },
            SettingsPatch {
                job_retention_days: Some(100_000),
                ..SettingsPatch::default()
            },
            SettingsPatch {
                cpu_threads: Some(5000),
                ..SettingsPatch::default()
            },
            SettingsPatch {
                pinned_worker: Some("a\nb".to_owned()),
                ..SettingsPatch::default()
            },
            SettingsPatch {
                whole_image_pixels: Some(u64::MAX),
                ..SettingsPatch::default()
            },
        ] {
            assert!(base.with_patch(&bad).is_err(), "{bad:?}");
        }
        assert!(base.with_patch(&SettingsPatch::default()).is_ok());
    }

    #[test]
    fn cert_names_are_split_checked_and_deduplicated() {
        assert_eq!(
            parse_cert_names("studio.example.com, 10.0.0.5;studio.example.com").expect("names"),
            vec!["studio.example.com".to_owned(), "10.0.0.5".to_owned()]
        );
        assert!(parse_cert_names("bad name!").is_err());
        assert!(parse_cert_names("").expect("empty").is_empty());
    }

    #[test]
    fn the_change_summary_names_the_groups() {
        let a = CoordinatorSettings::default();
        let b = a
            .with_patch(&SettingsPatch {
                cache_size_mib: Some(4096),
                max_job_memory_mib: Some(4096),
                ..SettingsPatch::default()
            })
            .expect("valid");
        assert_eq!(a.summary_of_change(&b), "performance, storage");
        assert_eq!(a.summary_of_change(&a), "settings");
    }
}
