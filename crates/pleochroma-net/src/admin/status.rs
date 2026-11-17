//! The coordinator's observable state, as plain serde types.
//!
//! Everything a coordinator window (or a remote administrator) shows or does is expressed
//! here, so one set of types serves the in-process GUI and the remote administration
//! protocol (`admin`, protocol 45). This module has no GUI, worker or socket dependency.
//!
//! # Conventions
//!
//! - **Time** is unix seconds as `u64` ([`UnixSeconds`]), UTC. A value of `0` is never
//!   used as "unknown"; unknown is `None`.
//! - **Fingerprints** are carried as 64-character lowercase hex strings (the SHA-256 of the
//!   certificate, the same text as [`crate::tls::fingerprint_to_hex`]); the short form is
//!   the first [`FINGERPRINT_SHORT_LEN`] characters ([`fingerprint_short`]). Convert to the
//!   raw [`crate::tls::Fingerprint`] with [`crate::tls::fingerprint_from_hex`].
//! - **Secrets** never appear here. A pending pairing code is listed by id, role, name and
//!   expiry only; [`IssuedToken`] is the one type that holds a token, and its `Debug`
//!   redacts it.
//! - Events carry no time stamp; the receiver stamps them on arrival.

use crate::messages::PeerRole;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, net::SocketAddr, time::SystemTime};

/// Seconds since the unix epoch, UTC. The one time representation of this module.
pub type UnixSeconds = u64;

/// Characters of a hex fingerprint in its short form.
pub const FINGERPRINT_SHORT_LEN: usize = 8;

/// The short form of a hex fingerprint: its first [`FINGERPRINT_SHORT_LEN`] characters
/// (the whole string if it is shorter).
#[must_use]
pub fn fingerprint_short(hex: &str) -> String {
    hex.chars().take(FINGERPRINT_SHORT_LEN).collect()
}

/// Identifies one pending pairing code. Not secret: it names the code, it is not the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TokenId(pub u64);

/// Identifies one joined render computer (the coordinator registry's id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkerId(pub u32);

/// A job's durable id: 16 bytes, shown and serialized as 32 lowercase hex characters.
///
/// Converts from and to the wire job id (`[u8; 16]`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(pub [u8; 16]);

impl JobId {
    /// The 32-character lowercase hex form.
    #[must_use]
    pub fn to_hex(&self) -> String {
        use fmt::Write as _;
        let mut out = String::with_capacity(32);
        for b in self.0 {
            let _ = write!(out, "{b:02x}");
        }
        out
    }

    /// Parses the 32-character hex form (surrounding whitespace is ignored).
    #[must_use]
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.len() != 32 || !s.is_ascii() {
            return None;
        }
        let mut out = [0u8; 16];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(Self(out))
    }
}

impl fmt::Debug for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JobId({})", self.to_hex())
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl From<[u8; 16]> for JobId {
    fn from(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

impl From<JobId> for [u8; 16] {
    fn from(id: JobId) -> Self {
        id.0
    }
}

impl Serialize for JobId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for JobId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::from_hex(&text)
            .ok_or_else(|| serde::de::Error::custom("a job id is 32 hex characters"))
    }
}

/// A freshly made pairing code, as the Connect dialog needs it.
///
/// `token` and `link` are secrets: the manual [`fmt::Debug`] prints neither.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuedToken {
    /// Names this code for [`CoordinatorEvent::TokenClaimed`] and cancelling.
    pub id: TokenId,
    /// The one-time code. Secret.
    pub token: String,
    /// The pairing link that carries the code and the coordinator's addresses. Secret.
    pub link: String,
    /// When the code stops working.
    pub expires_at: SystemTime,
}

impl fmt::Debug for IssuedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IssuedToken")
            .field("id", &self.id)
            .field("token", &"<redacted>")
            .field("link", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Where the coordinator is in its life.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoordinatorState {
    /// Binding ports and loading the library.
    Starting,
    /// Accepting connections.
    Running,
    /// Shutting down (draining connections).
    Stopping,
    /// Not running.
    Stopped,
    /// It failed to start or died; the text is plain words for the user.
    Error(String),
}

impl CoordinatorState {
    /// True while the coordinator accepts connections.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }
}

/// The ports and addresses the coordinator listens on. A port is `None` when its
/// listener is not open (for example no worker port in a build without render support).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenAddresses {
    /// The viewer port (also serves remote administration).
    pub viewer: Option<SocketAddr>,
    /// The viewer enrollment port.
    pub viewer_enroll: Option<SocketAddr>,
    /// The render computer port.
    pub worker: Option<SocketAddr>,
    /// The render computer enrollment port.
    pub worker_enroll: Option<SocketAddr>,
    /// The LAN addresses other computers can reach this one on, display text such as
    /// `192.168.178.20`, default route first.
    pub lan: Vec<String>,
}

/// A device allowed to connect (an entry of an allowlist), with its live state.
///
/// The list in [`CoordinatorStatus::devices`] holds every role; the Devices page shows
/// the viewer, monitor and admin entries, the Render computers page the worker entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceStatus {
    /// The name the owner sees (renamable).
    pub name: String,
    /// The certificate fingerprint, 64 lowercase hex characters.
    pub fingerprint: String,
    /// The first [`FINGERPRINT_SHORT_LEN`] characters of `fingerprint`.
    pub fingerprint_short: String,
    /// Which allowlist it is on: Viewer, Worker, Monitor or Admin.
    pub role: PeerRole,
    /// A short kind line such as "Phone · Android"; empty when unknown.
    pub platform: String,
    /// When it was paired, if recorded.
    pub added: Option<UnixSeconds>,
    /// When it was last connected (now, if online).
    pub last_seen: Option<UnixSeconds>,
    /// True while a connection is open.
    pub online: bool,
    /// When the open connection started; `None` offline.
    pub connected_since: Option<UnixSeconds>,
    /// What it is doing now, as text ("Rendering 1024 × 1024 · Path-Traced"); empty when idle.
    pub activity: String,
    /// The peer address of the open connection, or the last known one.
    pub address: Option<String>,
}

impl DeviceStatus {
    /// Builds a device from a full hex fingerprint, filling `fingerprint_short`.
    /// Everything else starts offline and empty.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        fingerprint_hex: impl Into<String>,
        role: PeerRole,
    ) -> Self {
        let fingerprint = fingerprint_hex.into();
        Self {
            name: name.into(),
            fingerprint_short: fingerprint_short(&fingerprint),
            fingerprint,
            role,
            platform: String::new(),
            added: None,
            last_seen: None,
            online: false,
            connected_since: None,
            activity: String::new(),
            address: None,
        }
    }

    /// The raw fingerprint, if `fingerprint` is valid hex.
    #[must_use]
    pub fn raw_fingerprint(&self) -> Option<crate::tls::Fingerprint> {
        crate::tls::fingerprint_from_hex(&self.fingerprint)
    }
}

/// What a render computer renders with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderHardware {
    /// A graphics card.
    Gpu,
    /// The processor.
    Cpu,
    /// Both (a computer that offers GPU and CPU lanes).
    GpuAndCpu,
}

/// A render computer that is joined now (or this computer's own lane, `own_lane`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderComputerStatus {
    /// The registry id; the own lane uses `WorkerId(0)`.
    pub id: WorkerId,
    /// Display name ("garage-rig").
    pub name: String,
    /// The certificate fingerprint (hex) of the joined computer; empty for the own lane.
    pub fingerprint: String,
    /// True for this computer's own render lane.
    pub own_lane: bool,
    /// GPU, CPU or both.
    pub hardware: RenderHardware,
    /// The hardware line ("RTX 4070"); empty when unknown.
    pub hardware_name: String,
    /// CPU threads it offers.
    pub threads: u32,
    /// Render slots it offers.
    pub slots: u32,
    /// Slots working now (at most `slots`).
    pub busy: u32,
    /// Measured speed in samples per second, once it has rendered; `None` before.
    pub rate: Option<f32>,
    /// A platform line such as "Worker · Windows"; empty when unknown.
    pub platform: String,
    /// When it joined; `None` for the own lane.
    pub connected_since: Option<UnixSeconds>,
    /// What it is doing now; empty when idle.
    pub activity: String,
    /// The peer address; `None` for the own lane.
    pub address: Option<String>,
}

/// A job's phase (mirrors the wire `JobPhase`, without needing the render feature).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    /// Stored, waiting for a render lane.
    Queued,
    /// Rendering now.
    Running,
    /// Finished; the PNG can be fetched until it expires.
    Done,
    /// Stopped by an error; see `note`.
    Failed,
    /// Stopped by its owner or an administrator.
    Cancelled,
}

impl JobState {
    /// True for Done, Failed and Cancelled.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

/// One job, across all owners (the administrator's list).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobStatus {
    /// The durable id.
    pub id: JobId,
    /// The owner's display name (the device that submitted it).
    pub owner: String,
    /// The owner's fingerprint (hex); empty when unknown.
    pub owner_fingerprint: String,
    /// Queued, Running, Done, Failed or Cancelled.
    pub state: JobState,
    /// Progress from 0.0 to 1.0.
    pub progress: f32,
    /// Samples per pixel done so far.
    pub samples_done: u32,
    /// Samples per pixel asked for.
    pub samples_total: u32,
    /// Picture width in pixels.
    pub width: u32,
    /// Picture height in pixels.
    pub height: u32,
    /// The label the owner gave it ("Final render").
    pub label: String,
    /// The design's name, if the job carries one; empty otherwise.
    pub design_name: String,
    /// Which computers render it now ("this computer + garage-rig"); empty when not running.
    pub route: String,
    /// When it was stored.
    pub created: UnixSeconds,
    /// When it reached a terminal state.
    pub finished: Option<UnixSeconds>,
    /// The PNG's size in bytes, once Done.
    pub result_len: Option<u64>,
    /// Why it failed, or a short remark; empty when there is nothing to say.
    pub note: String,
}

/// The jobs footer: retention and usage.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobsSummary {
    /// Days a finished job's result is kept; `None` if kept until deleted.
    pub retention_days: Option<u32>,
    /// Jobs stored now.
    pub stored: u32,
    /// The most jobs the store keeps.
    pub max_jobs: u32,
    /// Bytes the stored jobs use on disk.
    pub used_bytes: u64,
    /// The disk budget in bytes, if one is set.
    pub limit_bytes: Option<u64>,
    /// The jobs folder (for "Open jobs folder"); empty if unknown.
    pub folder: String,
}

/// A pairing code that has not been claimed yet. **The secret is never included.**
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingToken {
    /// Names the code (use with `cancel_token`).
    pub id: TokenId,
    /// The role the code mints a certificate for.
    pub role: PeerRole,
    /// The default name the new device will get.
    pub name: String,
    /// When the code stops working.
    pub expires: UnixSeconds,
}

/// The design library the coordinator serves.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryStatus {
    /// The library file; `None` in library-less ("rendering only") mode.
    pub path: Option<String>,
    /// Designs in it; `None` when there is no library or the count is not known yet.
    pub designs: Option<u64>,
    /// True: the coordinator never writes to it (always true today).
    pub read_only: bool,
}

impl LibraryStatus {
    /// True in library-less mode.
    #[must_use]
    pub const fn is_none(&self) -> bool {
        self.path.is_none()
    }
}

/// How the own render lane is set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderMode {
    /// Graphics card and processor.
    GpuAndCpu,
    /// Graphics card only.
    GpuOnly,
    /// Processor only.
    CpuOnly,
}

/// "Render on this computer": the own lane's switch and mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnRenderStatus {
    /// Whether the own lane renders (`--render`).
    pub enabled: bool,
    /// GPU + CPU, GPU only or CPU only.
    pub mode: RenderMode,
    /// CPU threads in use; `None` means automatic.
    pub threads: Option<u32>,
    /// The hardware line ("Radeon 780M · 16 threads"); empty when unknown.
    pub hardware: String,
    /// True while it renders something.
    pub busy: bool,
}

/// The sum of what the coordinator can render with (the Render computers footer).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapacitySummary {
    /// GPUs across the own lane and joined computers.
    pub gpus: u32,
    /// CPU threads across all of them.
    pub threads: u32,
    /// The largest picture it accepts, width in pixels (0 when unknown).
    pub max_width: u32,
    /// The largest picture it accepts, height in pixels (0 when unknown).
    pub max_height: u32,
}

/// Where the graphics-card shader-cache warm-up stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShaderCacheState {
    /// No graphics-card lane, or the warm-up has not started.
    #[default]
    Idle,
    /// Building now.
    Running,
    /// Every step finished.
    Ready,
    /// Cancelled (the coordinator is stopping) before every step finished.
    Stopped,
    /// This computer has no usable graphics card for it.
    Unavailable,
}

/// The shader-cache warm-up of the own render lane, for the Overview card and the shell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShaderCacheStatus {
    /// Where the warm-up stands.
    pub state: ShaderCacheState,
    /// Build steps finished so far.
    pub done: u32,
    /// Build steps in total (0 when unknown).
    pub total: u32,
    /// True when some step compiled from source instead of loading the disk cache.
    pub fresh: bool,
}

/// What an interactive render of a viewer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RenderKind {
    /// The live view: samples keep arriving as the picture refines.
    #[default]
    Live,
    /// One finished picture (an export or a final render).
    FinalImage,
    /// The tilt curves of a design.
    TiltCurves,
    /// One chunk of an adaptive live pass.
    LivePass,
    /// Whole beam tiles.
    BeamTiles,
    /// A batch of preview pictures.
    Batch,
}

/// A render a viewer asked for and the coordinator is serving now (not a stored job).
///
/// It exists exactly as long as the request: it is gone when the picture is done, cancelled or
/// the viewer disconnected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveRender {
    /// Unique for this coordinator run; stable while the render lives.
    pub id: u64,
    /// The asking device's name as the Devices page shows it ("Phone 1"), else its short
    /// fingerprint.
    pub device: String,
    /// The asking device's fingerprint (hex), to tie the row to the Devices page; empty for an
    /// unauthenticated loopback viewer.
    pub fingerprint: String,
    /// What kind of request it is.
    pub kind: RenderKind,
    /// "Path-Traced" or "Ray-Traced" (empty when unknown).
    pub engine: String,
    /// Picture width in pixels.
    pub width: u32,
    /// Picture height in pixels.
    pub height: u32,
    /// Samples per pixel traced so far (the latest progress the viewer was sent).
    pub samples_done: u32,
    /// The target samples per pixel, or `None` for an open-ended live view.
    pub samples_target: Option<u32>,
    /// Unix seconds when the request started.
    pub started_unix: UnixSeconds,
    /// Who traces it: "This computer", "2 render computers", "This computer + 1 render
    /// computer".
    pub route: String,
    /// Millions of samples times pixels per second over the last few seconds, when known.
    pub rate_mps: Option<f64>,
    /// While the graphics card builds its shaders: (done, total) of the warm-up.
    pub preparing: Option<(u32, u32)>,
}

/// Everything the coordinator pages show, in one snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoordinatorStatus {
    /// The display name ("Studio PC").
    pub name: String,
    /// The product version ("0.12.0").
    pub version: String,
    /// The wire protocol version ([`crate::messages::PROTOCOL_VERSION`]).
    pub protocol: u16,
    /// The build identity (the hash both sides must share); empty when not known.
    pub build: String,
    /// Starting, Running, Stopping, Stopped or Error.
    pub state: CoordinatorState,
    /// When the coordinator started running; `None` before.
    pub started: Option<UnixSeconds>,
    /// Ports and LAN addresses.
    pub addresses: ListenAddresses,
    /// Every allowlisted device of every role, with live state.
    pub devices: Vec<DeviceStatus>,
    /// Joined render computers, the own lane first when it is on.
    pub render_computers: Vec<RenderComputerStatus>,
    /// The own render lane's switch and mode.
    pub own_render: OwnRenderStatus,
    /// Totals for the Render computers footer.
    pub capacity: CapacitySummary,
    /// All jobs across owners, newest first.
    pub jobs: Vec<JobStatus>,
    /// Retention and usage of the job store.
    pub jobs_summary: JobsSummary,
    /// Unclaimed pairing codes (never their secrets).
    pub pending_tokens: Vec<PendingToken>,
    /// The design library, or "no library".
    pub library: LibraryStatus,
    /// Whether remote administration is switched on.
    pub remote_admin_enabled: bool,
    /// Whether new devices may pair (`--no-enroll` off).
    pub allow_pairing: bool,
    /// Whether the coordinator announces itself on the network.
    pub discovery: bool,
    /// Plain-words warnings for the Overview (for example "any certificate is accepted").
    pub warnings: Vec<String>,
    /// The own lane's shader-cache warm-up; `Idle` when there is no graphics-card lane.
    #[serde(default)]
    pub shader_cache: ShaderCacheStatus,
    /// The renders viewers asked for that run now, newest first (not the stored jobs).
    #[serde(default)]
    pub renders: Vec<ActiveRender>,
}

impl CoordinatorStatus {
    /// A status with the given identity and state and nothing else: no devices, jobs or
    /// library. Used by backends that know little (and as the base of `Connecting…`).
    #[must_use]
    pub fn minimal(
        name: impl Into<String>,
        version: impl Into<String>,
        protocol: u16,
        state: CoordinatorState,
    ) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            protocol,
            build: String::new(),
            state,
            started: None,
            addresses: ListenAddresses::default(),
            devices: Vec::new(),
            render_computers: Vec::new(),
            own_render: OwnRenderStatus {
                enabled: false,
                mode: RenderMode::GpuAndCpu,
                threads: None,
                hardware: String::new(),
                busy: false,
            },
            capacity: CapacitySummary::default(),
            jobs: Vec::new(),
            jobs_summary: JobsSummary::default(),
            pending_tokens: Vec::new(),
            library: LibraryStatus {
                path: None,
                designs: None,
                read_only: true,
            },
            remote_admin_enabled: false,
            allow_pairing: true,
            discovery: false,
            warnings: Vec::new(),
            shader_cache: ShaderCacheStatus::default(),
            renders: Vec::new(),
        }
    }

    /// This status as a Monitor may see it: the local paths are blanked.
    ///
    /// The jobs folder becomes empty and the library path shrinks to its file name (so "no
    /// library" stays distinguishable from "a library"). Call it before a status leaves the
    /// coordinator for a Monitor.
    #[must_use]
    pub fn for_monitor(mut self) -> Self {
        self.jobs_summary.folder = String::new();
        if let Some(path) = self.library.path.take() {
            let name = path.rsplit(['/', '\\']).next().unwrap_or_default();
            self.library.path = Some(name.to_string());
        }
        self
    }

    /// Devices that are online now and are not render computers.
    #[must_use]
    pub fn devices_online(&self) -> usize {
        self.devices
            .iter()
            .filter(|d| d.online && d.role != PeerRole::Worker)
            .count()
    }

    /// Jobs in the Running state.
    #[must_use]
    pub fn jobs_running(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| j.state == JobState::Running)
            .count()
    }

    /// Jobs in the Queued state.
    #[must_use]
    pub fn jobs_queued(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| j.state == JobState::Queued)
            .count()
    }
}

/// A log severity, most severe first. Mirrors `tracing::Level` and is serde-friendly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum LogLevel {
    /// Something failed.
    Error,
    /// Something is wrong but work continues.
    Warn,
    /// Normal operation worth recording.
    Info,
    /// Detail for diagnosing.
    Debug,
    /// Finest detail.
    Trace,
}

impl LogLevel {
    /// True if a line of this level passes a filter that shows lines up to `min` in detail
    /// (`Info` shows Error, Warn and Info).
    #[must_use]
    pub fn passes(self, min: Self) -> bool {
        self <= min
    }

    /// The upper-case name ("WARN").
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        }
    }
}

impl From<tracing::Level> for LogLevel {
    fn from(level: tracing::Level) -> Self {
        if level == tracing::Level::ERROR {
            Self::Error
        } else if level == tracing::Level::WARN {
            Self::Warn
        } else if level == tracing::Level::INFO {
            Self::Info
        } else if level == tracing::Level::DEBUG {
            Self::Debug
        } else {
            Self::Trace
        }
    }
}

impl From<LogLevel> for tracing::Level {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Error => Self::ERROR,
            LogLevel::Warn => Self::WARN,
            LogLevel::Info => Self::INFO,
            LogLevel::Debug => Self::DEBUG,
            LogLevel::Trace => Self::TRACE,
        }
    }
}

/// One line of the coordinator's log. It never contains a secret (the log layer guarantees
/// that; see the worker's `settings::log`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    /// When it was logged.
    pub time: UnixSeconds,
    /// Its severity.
    pub level: LogLevel,
    /// The module path that logged it.
    pub target: String,
    /// The message text.
    pub message: String,
}

/// How an event reads in the activity feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    /// Routine progress (a device connected).
    Info,
    /// Something finished well (a job is done).
    Good,
    /// Something to notice (the address changed, a code expired).
    Notice,
    /// Something failed.
    Problem,
}

/// Something that happened in the coordinator. Sent as it happens; carries no time stamp
/// and never a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoordinatorEvent {
    /// The coordinator's state changed.
    StateChanged(CoordinatorState),
    /// A device opened a connection.
    DeviceConnected {
        /// Device name.
        name: String,
        /// Fingerprint, hex.
        fingerprint: String,
        /// Its role.
        role: PeerRole,
        /// Peer address.
        peer: Option<SocketAddr>,
    },
    /// A device's connection closed.
    DeviceDisconnected {
        /// Device name.
        name: String,
        /// Fingerprint, hex.
        fingerprint: String,
        /// Its role.
        role: PeerRole,
    },
    /// A pairing code was made.
    TokenIssued {
        /// The code's id.
        id: TokenId,
        /// The role it is for.
        role: PeerRole,
        /// The default name for the new device.
        name: String,
        /// When it stops working.
        expires: UnixSeconds,
    },
    /// A pairing code was used: a new device is now on the allowlist.
    TokenClaimed {
        /// The role the new device got.
        role: PeerRole,
        /// Its fingerprint, hex.
        fingerprint: String,
        /// Where the claim came from.
        peer: SocketAddr,
    },
    /// A pairing code ran out unused.
    TokenExpired {
        /// The code's id.
        id: TokenId,
    },
    /// A pairing code was cancelled before use.
    TokenCancelled {
        /// The code's id.
        id: TokenId,
    },
    /// A device's access was removed.
    DeviceRevoked {
        /// Fingerprint, hex.
        fingerprint: String,
        /// Its name at the time.
        name: String,
    },
    /// A device was renamed.
    DeviceRenamed {
        /// Fingerprint, hex.
        fingerprint: String,
        /// The new name.
        name: String,
    },
    /// A render computer joined.
    WorkerJoined {
        /// Its registry id.
        id: WorkerId,
        /// Its name.
        name: String,
    },
    /// A render computer left (or was disconnected).
    WorkerLeft {
        /// Its registry id.
        id: WorkerId,
        /// Its name.
        name: String,
    },
    /// A job was stored.
    JobSubmitted {
        /// The job.
        id: JobId,
        /// The owner's name.
        owner: String,
        /// The job's label.
        label: String,
    },
    /// A job changed state (started, finished, failed, cancelled).
    JobStateChanged {
        /// The job.
        id: JobId,
        /// The new state.
        state: JobState,
        /// The owner's name.
        owner: String,
        /// The job's label.
        label: String,
    },
    /// A job was deleted.
    JobDeleted {
        /// The job.
        id: JobId,
    },
    /// Audit: an administrator did something. `who` is the administrator's name (and short
    /// fingerprint for a remote one, or "this computer" for the local GUI).
    AdminAction {
        /// Who did it.
        who: String,
        /// What they did ("revoke", "cancel job", "apply settings", ...).
        action: String,
        /// What it was done to (a device or job name); empty when it has no target.
        target: String,
    },
    /// The coordinator's server certificate was replaced (the network address changed).
    CertificateSwapped {
        /// The addresses the new certificate names, display text.
        addresses: Vec<String>,
    },
    /// The LAN addresses changed.
    AddressesChanged {
        /// The new LAN addresses, display text.
        addresses: Vec<String>,
    },
    /// Settings were applied.
    SettingsChanged {
        /// A short description ("performance, storage").
        summary: String,
    },
    /// A warning for the Overview.
    Warning {
        /// Plain words.
        text: String,
    },
    /// A log line (only sent to subscribers of the log).
    Log(LogLine),
}

impl CoordinatorEvent {
    /// How the activity feed should color this event.
    #[must_use]
    pub const fn kind(&self) -> EventKind {
        match self {
            Self::StateChanged(CoordinatorState::Error(_)) => EventKind::Problem,
            Self::StateChanged(_)
            | Self::DeviceDisconnected { .. }
            | Self::TokenIssued { .. }
            | Self::TokenCancelled { .. }
            | Self::JobSubmitted { .. }
            | Self::JobDeleted { .. }
            | Self::AdminAction { .. }
            | Self::SettingsChanged { .. } => EventKind::Info,
            Self::DeviceConnected { .. }
            | Self::TokenClaimed { .. }
            | Self::WorkerJoined { .. }
            | Self::DeviceRenamed { .. } => EventKind::Good,
            Self::JobStateChanged { state, .. } => match state {
                JobState::Done => EventKind::Good,
                JobState::Failed => EventKind::Problem,
                JobState::Queued | JobState::Running | JobState::Cancelled => EventKind::Info,
            },
            Self::TokenExpired { .. }
            | Self::DeviceRevoked { .. }
            | Self::WorkerLeft { .. }
            | Self::CertificateSwapped { .. }
            | Self::AddressesChanged { .. }
            | Self::Warning { .. } => EventKind::Notice,
            Self::Log(line) => match line.level {
                LogLevel::Error => EventKind::Problem,
                LogLevel::Warn => EventKind::Notice,
                LogLevel::Info | LogLevel::Debug | LogLevel::Trace => EventKind::Info,
            },
        }
    }

    /// One line of plain words for the Recent activity feed ("garage-rig joined").
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::StateChanged(state) => match state {
                CoordinatorState::Starting => "Coordinator is starting".to_string(),
                CoordinatorState::Running => "Coordinator is running".to_string(),
                CoordinatorState::Stopping => "Coordinator is stopping".to_string(),
                CoordinatorState::Stopped => "Coordinator stopped".to_string(),
                CoordinatorState::Error(text) => format!("Coordinator problem: {text}"),
            },
            Self::DeviceConnected { name, .. } => format!("{name} connected"),
            Self::DeviceDisconnected { name, .. } => format!("{name} disconnected"),
            Self::TokenIssued { role, .. } => {
                format!("A pairing code for a {} was made", role_word(*role))
            }
            Self::TokenClaimed {
                role, fingerprint, ..
            } => format!(
                "A new {} paired ({})",
                role_word(*role),
                fingerprint_short(fingerprint)
            ),
            Self::TokenExpired { .. } => "A pairing code expired unused".to_string(),
            Self::TokenCancelled { .. } => "A pairing code was cancelled".to_string(),
            Self::DeviceRevoked { name, .. } => format!("Access removed for {name}"),
            Self::DeviceRenamed { name, .. } => format!("A device was renamed to {name}"),
            Self::WorkerJoined { name, .. } => format!("{name} joined"),
            Self::WorkerLeft { name, .. } => format!("{name} left"),
            Self::JobSubmitted { owner, label, .. } => format!("{owner} started {label}"),
            Self::JobStateChanged {
                state,
                owner,
                label,
                ..
            } => match state {
                JobState::Queued => format!("{label} from {owner} is waiting"),
                JobState::Running => format!("{label} from {owner} is rendering"),
                JobState::Done => format!("{label} from {owner} is done"),
                JobState::Failed => format!("{label} from {owner} failed"),
                JobState::Cancelled => format!("{label} from {owner} was cancelled"),
            },
            Self::JobDeleted { .. } => "A job was deleted".to_string(),
            Self::AdminAction {
                who,
                action,
                target,
            } => {
                if target.is_empty() {
                    format!("{who}: {action}")
                } else {
                    format!("{who}: {action} {target}")
                }
            }
            Self::CertificateSwapped { .. } => {
                "Network address changed, certificate updated".to_string()
            }
            Self::AddressesChanged { .. } => "Network address changed".to_string(),
            Self::SettingsChanged { summary } => format!("Settings changed: {summary}"),
            Self::Warning { text } => text.clone(),
            Self::Log(line) => line.message.clone(),
        }
    }
}

/// The plain word for a role in activity text.
const fn role_word(role: PeerRole) -> &'static str {
    match role {
        PeerRole::Viewer => "device",
        PeerRole::Worker => "render computer",
        PeerRole::Monitor => "monitor",
        PeerRole::Admin => "administrator",
    }
}

/// How a render computer is told to compress network traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkCompression {
    /// The coordinator's default (zstd).
    Default,
    /// Faster, larger (lz4), for gigabit networks.
    Fast,
    /// None.
    Off,
}

/// A change to the coordinator's safe settings.
///
/// Every field is optional: `None` leaves the setting as it is. Validation happens on the
/// coordinator (the worker's `settings::validate`), never in this type.
///
/// Never remote: network, port and bind changes, the remote-administration switch, CA
/// reset, rotating the issuing secret, backup export. They are deliberately not fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsPatch {
    // Performance
    /// Most memory one job may use, in MiB.
    pub max_job_memory_mib: Option<u32>,
    /// Longest a whole-image render may take, in seconds, before it is split.
    pub whole_image_seconds: Option<u32>,
    /// Most pixels of a whole-image render before it is split.
    pub whole_image_pixels: Option<u64>,
    /// Jobs one device may have at once.
    pub jobs_per_device: Option<u32>,
    /// Render computers used for live previews.
    pub interactive_workers: Option<u32>,
    /// A render computer name pinned for live previews; an empty string clears the pin.
    pub pinned_worker: Option<String>,
    /// Network compression.
    pub network_compression: Option<NetworkCompression>,
    /// Whether render computers may join.
    pub accept_render_computers: Option<bool>,
    /// CPU threads for rendering (0 = automatic).
    pub cpu_threads: Option<u32>,
    // Storage
    /// Days a finished job's result is kept.
    pub job_retention_days: Option<u32>,
    /// Size of the asset cache, in MiB.
    pub cache_size_mib: Option<u32>,
    // Security and discovery
    /// Whether new devices may pair.
    pub allow_pairing: Option<bool>,
    /// Whether the coordinator announces itself on the network.
    pub discovery: Option<bool>,
    // Render on this computer
    /// Whether the own render lane is on.
    pub render_enabled: Option<bool>,
    /// The own lane's mode.
    pub render_mode: Option<RenderMode>,
    /// CPU threads of the own lane (0 = automatic).
    pub render_threads: Option<u32>,
    // General
    /// The display name.
    pub display_name: Option<String>,
}

impl SettingsPatch {
    /// True if no field is set (applying it changes nothing).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// True if any field is set besides the three a running coordinator takes at once.
    ///
    /// Those are `whole_image_seconds`, `whole_image_pixels` and `jobs_per_device`. A
    /// coordinator with no settings file can apply a patch only when this is false.
    #[must_use]
    pub fn sets_restart_fields(&self) -> bool {
        let mut rest = self.clone();
        rest.whole_image_seconds = None;
        rest.whole_image_pixels = None;
        rest.jobs_per_device = None;
        !rest.is_empty()
    }
}

/// How the caller reaches the coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessRole {
    /// The GUI in the coordinator's own process: everything is allowed.
    Local,
    /// A remote full administrator.
    Admin,
    /// A remote read-only monitor.
    Monitor,
}

/// What the caller may do. The GUI greys out an action whose flag is false; the coordinator
/// enforces the same table (`PERMISSION_DENIED`), so a stale flag is harmless.
///
/// Never remote (false for `Admin` and `Monitor`, true only for `Local`): `change_network`,
/// `change_remote_admin`, `reset_pairings`, `rotate_secret`, `export_backup`, `stop`,
/// `issue_privileged_tokens`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct Capabilities {
    /// How the caller is connected.
    pub role: AccessRole,
    /// May read status, devices, jobs, the log.
    pub view: bool,
    /// May fetch a finished job's picture.
    pub fetch_results: bool,
    /// May make device and render computer pairing codes.
    pub issue_tokens: bool,
    /// May cancel a code.
    pub cancel_tokens: bool,
    /// May remove a device's access.
    pub revoke: bool,
    /// May rename devices.
    pub rename: bool,
    /// May cancel jobs.
    pub cancel_jobs: bool,
    /// May delete jobs.
    pub delete_jobs: bool,
    /// May disconnect a render computer.
    pub disconnect_workers: bool,
    /// May change the safe settings ([`SettingsPatch`]).
    pub apply_settings: bool,
    /// May restart the coordinator.
    pub restart: bool,
    /// May stop the coordinator (Local only).
    pub stop: bool,
    /// May change ports, bind address or who can connect (Local only).
    pub change_network: bool,
    /// May switch remote administration on or off (Local only).
    pub change_remote_admin: bool,
    /// May make monitor and administrator codes (Local only).
    pub issue_privileged_tokens: bool,
    /// May reset all pairings, rotate the issuing secret or export a backup (Local only).
    pub manage_secrets: bool,
}

impl Capabilities {
    /// The in-process GUI: everything.
    #[must_use]
    pub const fn local() -> Self {
        Self {
            role: AccessRole::Local,
            view: true,
            fetch_results: true,
            issue_tokens: true,
            cancel_tokens: true,
            revoke: true,
            rename: true,
            cancel_jobs: true,
            delete_jobs: true,
            disconnect_workers: true,
            apply_settings: true,
            restart: true,
            stop: true,
            change_network: true,
            change_remote_admin: true,
            issue_privileged_tokens: true,
            manage_secrets: true,
        }
    }

    /// A remote full administrator: everything except the never-remote actions.
    #[must_use]
    pub const fn admin() -> Self {
        Self {
            role: AccessRole::Admin,
            stop: false,
            change_network: false,
            change_remote_admin: false,
            issue_privileged_tokens: false,
            manage_secrets: false,
            ..Self::local()
        }
    }

    /// A remote monitor: it may look and fetch pictures, nothing else.
    #[must_use]
    pub const fn monitor() -> Self {
        Self {
            role: AccessRole::Monitor,
            view: true,
            fetch_results: true,
            issue_tokens: false,
            cancel_tokens: false,
            revoke: false,
            rename: false,
            cancel_jobs: false,
            delete_jobs: false,
            disconnect_workers: false,
            apply_settings: false,
            restart: false,
            stop: false,
            change_network: false,
            change_remote_admin: false,
            issue_privileged_tokens: false,
            manage_secrets: false,
        }
    }

    /// No permission at all (a backend that is not connected yet).
    #[must_use]
    pub const fn none(role: AccessRole) -> Self {
        Self {
            role,
            view: false,
            fetch_results: false,
            ..Self::monitor()
        }
    }

    /// True if the caller is the in-process GUI.
    #[must_use]
    pub const fn is_local(&self) -> bool {
        matches!(self.role, AccessRole::Local)
    }

    /// True if the caller is remote.
    #[must_use]
    pub const fn is_remote(&self) -> bool {
        !self.is_local()
    }

    /// True if the caller can change nothing (a monitor, or not connected).
    #[must_use]
    pub const fn is_read_only(&self) -> bool {
        !(self.issue_tokens
            || self.cancel_tokens
            || self.revoke
            || self.rename
            || self.cancel_jobs
            || self.delete_jobs
            || self.disconnect_workers
            || self.apply_settings
            || self.restart
            || self.stop
            || self.change_network
            || self.change_remote_admin
            || self.issue_privileged_tokens
            || self.manage_secrets)
    }

    /// May the caller make a pairing code for `role`? Viewer and Worker codes need
    /// `issue_tokens`; Monitor and Admin codes need `issue_privileged_tokens`.
    #[must_use]
    pub const fn can_issue(&self, role: PeerRole) -> bool {
        match role {
            PeerRole::Viewer | PeerRole::Worker => self.issue_tokens,
            PeerRole::Monitor | PeerRole::Admin => self.issue_privileged_tokens,
        }
    }

    /// The reason text the GUI shows beside a greyed-out action.
    #[must_use]
    pub const fn denied_reason(&self) -> &'static str {
        match self.role {
            AccessRole::Local => "",
            AccessRole::Admin => "This can only be done on the coordinator's own computer.",
            AccessRole::Monitor => {
                "You have monitor access. Ask for full administration to change this."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_status() -> CoordinatorStatus {
        let mut status =
            CoordinatorStatus::minimal("Studio PC", "0.12.0", 45, CoordinatorState::Running);
        status.addresses.viewer = Some("192.168.178.20:7878".parse().unwrap());
        status.addresses.lan = vec!["192.168.178.20".to_string()];
        let mut device = DeviceStatus::new("Niklas's S23", "ab".repeat(32), PeerRole::Viewer);
        device.online = true;
        device.last_seen = Some(1_700_000_000);
        device.address = Some("192.168.178.42".to_string());
        status.devices.push(device);
        status.render_computers.push(RenderComputerStatus {
            id: WorkerId(3),
            name: "garage-rig".to_string(),
            fingerprint: "cd".repeat(32),
            own_lane: false,
            hardware: RenderHardware::Gpu,
            hardware_name: "RTX 4070".to_string(),
            threads: 24,
            slots: 2,
            busy: 1,
            rate: Some(12.5),
            platform: "Worker · Windows".to_string(),
            connected_since: Some(1_700_000_100),
            activity: "Final render".to_string(),
            address: Some("192.168.178.40".to_string()),
        });
        status.jobs.push(JobStatus {
            id: JobId([7; 16]),
            owner: "Niklas's S23".to_string(),
            owner_fingerprint: String::new(),
            state: JobState::Running,
            progress: 0.41,
            samples_done: 840,
            samples_total: 2048,
            width: 4096,
            height: 2304,
            label: "Final render".to_string(),
            design_name: "Ring".to_string(),
            route: "garage-rig".to_string(),
            created: 1_700_000_200,
            finished: None,
            result_len: None,
            note: String::new(),
        });
        status.pending_tokens.push(PendingToken {
            id: TokenId(9),
            role: PeerRole::Worker,
            name: "Attic PC".to_string(),
            expires: 1_700_000_900,
        });
        status.library = LibraryStatus {
            path: Some("D:/Gems/facet_diagrams.sqlite".to_string()),
            designs: Some(4812),
            read_only: true,
        };
        status
            .warnings
            .push("Any client certificate is accepted".to_string());
        status.shader_cache = ShaderCacheStatus {
            state: ShaderCacheState::Running,
            done: 12,
            total: 40,
            fresh: true,
        };
        status.renders.push(ActiveRender {
            id: 7,
            device: "Phone 1".to_string(),
            fingerprint: "ab".repeat(32),
            kind: RenderKind::FinalImage,
            engine: "Path-Traced".to_string(),
            width: 1920,
            height: 1080,
            samples_done: 128,
            samples_target: Some(512),
            started_unix: 1_700_000_300,
            route: "This computer + 1 render computer".to_string(),
            rate_mps: Some(4.2),
            preparing: Some((12, 40)),
        });
        status
    }

    #[test]
    fn renders_default_to_empty_and_survive_json_without_the_field() {
        let minimal = CoordinatorStatus::minimal("A", "0.12.0", 45, CoordinatorState::Running);
        assert!(minimal.renders.is_empty());
        assert_eq!(RenderKind::default(), RenderKind::Live);
        let mut value = serde_json::to_value(sample_status()).unwrap();
        value.as_object_mut().unwrap().remove("renders");
        let back: CoordinatorStatus = serde_json::from_value(value).unwrap();
        assert!(back.renders.is_empty());
    }

    #[test]
    fn an_active_render_round_trips_through_json_and_postcard() {
        let render = sample_status().renders.remove(0);
        let text = serde_json::to_string(&render).unwrap();
        assert_eq!(serde_json::from_str::<ActiveRender>(&text).unwrap(), render);
        let bytes = postcard::to_allocvec(&render).unwrap();
        assert_eq!(
            postcard::from_bytes::<ActiveRender>(&bytes).unwrap(),
            render
        );
        for kind in [
            RenderKind::Live,
            RenderKind::FinalImage,
            RenderKind::TiltCurves,
            RenderKind::LivePass,
            RenderKind::BeamTiles,
            RenderKind::Batch,
        ] {
            let bytes = postcard::to_allocvec(&kind).unwrap();
            assert_eq!(postcard::from_bytes::<RenderKind>(&bytes).unwrap(), kind);
        }
    }

    #[test]
    fn shader_cache_defaults_to_idle_and_survives_json_without_the_field() {
        let minimal = CoordinatorStatus::minimal("A", "0.12.0", 45, CoordinatorState::Running);
        assert_eq!(minimal.shader_cache, ShaderCacheStatus::default());
        assert_eq!(minimal.shader_cache.state, ShaderCacheState::Idle);
        let mut value = serde_json::to_value(sample_status()).unwrap();
        value.as_object_mut().unwrap().remove("shader_cache");
        let back: CoordinatorStatus = serde_json::from_value(value).unwrap();
        assert_eq!(back.shader_cache, ShaderCacheStatus::default());
    }

    #[test]
    fn status_round_trips_through_json() {
        let status = sample_status();
        let text = serde_json::to_string(&status).unwrap();
        let back: CoordinatorStatus = serde_json::from_str(&text).unwrap();
        assert_eq!(back, status);
        assert_eq!(back.devices_online(), 1);
        assert_eq!(back.jobs_running(), 1);
        assert_eq!(back.jobs_queued(), 0);
    }

    #[test]
    fn for_monitor_blanks_local_paths() {
        let mut status = sample_status();
        status.jobs_summary.folder = "C:\\Users\\x\\jobs".to_string();
        status.library.path = Some("C:\\Users\\x\\lib\\designs.sqlite".to_string());
        let shown = status.for_monitor();
        assert!(shown.jobs_summary.folder.is_empty());
        assert_eq!(shown.library.path.as_deref(), Some("designs.sqlite"));
        assert!(!shown.library.is_none());
    }

    #[test]
    fn status_round_trips_through_postcard() {
        let status = sample_status();
        let bytes = postcard::to_allocvec(&status).unwrap();
        let back: CoordinatorStatus = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, status);
    }

    #[test]
    fn pending_token_has_no_secret_field() {
        let text = serde_json::to_string(&sample_status().pending_tokens[0]).unwrap();
        assert!(!text.contains("token\""));
        assert!(!text.contains("secret"));
    }

    #[test]
    fn state_error_round_trips() {
        let state = CoordinatorState::Error("port 7878 is taken".to_string());
        let text = serde_json::to_string(&state).unwrap();
        assert_eq!(
            serde_json::from_str::<CoordinatorState>(&text).unwrap(),
            state
        );
        assert!(!state.is_running());
        assert!(CoordinatorState::Running.is_running());
    }

    #[test]
    fn every_event_round_trips() {
        let events = vec![
            CoordinatorEvent::StateChanged(CoordinatorState::Stopped),
            CoordinatorEvent::DeviceConnected {
                name: "S23".to_string(),
                fingerprint: "ab".repeat(32),
                role: PeerRole::Viewer,
                peer: Some("192.168.178.42:5000".parse().unwrap()),
            },
            CoordinatorEvent::DeviceDisconnected {
                name: "S23".to_string(),
                fingerprint: "ab".repeat(32),
                role: PeerRole::Viewer,
            },
            CoordinatorEvent::TokenIssued {
                id: TokenId(1),
                role: PeerRole::Worker,
                name: "Attic".to_string(),
                expires: 10,
            },
            CoordinatorEvent::TokenClaimed {
                role: PeerRole::Monitor,
                fingerprint: "cd".repeat(32),
                peer: "10.0.0.2:4000".parse().unwrap(),
            },
            CoordinatorEvent::TokenExpired { id: TokenId(1) },
            CoordinatorEvent::TokenCancelled { id: TokenId(2) },
            CoordinatorEvent::DeviceRevoked {
                fingerprint: "ab".repeat(32),
                name: "S23".to_string(),
            },
            CoordinatorEvent::DeviceRenamed {
                fingerprint: "ab".repeat(32),
                name: "Phone".to_string(),
            },
            CoordinatorEvent::WorkerJoined {
                id: WorkerId(4),
                name: "garage-rig".to_string(),
            },
            CoordinatorEvent::WorkerLeft {
                id: WorkerId(4),
                name: "garage-rig".to_string(),
            },
            CoordinatorEvent::JobSubmitted {
                id: JobId([1; 16]),
                owner: "S23".to_string(),
                label: "Final".to_string(),
            },
            CoordinatorEvent::JobStateChanged {
                id: JobId([1; 16]),
                state: JobState::Done,
                owner: "S23".to_string(),
                label: "Final".to_string(),
            },
            CoordinatorEvent::JobDeleted { id: JobId([1; 16]) },
            CoordinatorEvent::AdminAction {
                who: "Laptop (ab12cd34)".to_string(),
                action: "revoke".to_string(),
                target: "S23".to_string(),
            },
            CoordinatorEvent::CertificateSwapped {
                addresses: vec!["192.168.178.20".to_string()],
            },
            CoordinatorEvent::AddressesChanged {
                addresses: vec!["192.168.178.21".to_string()],
            },
            CoordinatorEvent::SettingsChanged {
                summary: "storage".to_string(),
            },
            CoordinatorEvent::Warning {
                text: "careful".to_string(),
            },
            CoordinatorEvent::Log(LogLine {
                time: 5,
                level: LogLevel::Warn,
                target: "serve".to_string(),
                message: "hello".to_string(),
            }),
        ];
        for event in events {
            let text = serde_json::to_string(&event).unwrap();
            let back: CoordinatorEvent = serde_json::from_str(&text).unwrap();
            assert_eq!(back, event);
            assert!(!event.summary().is_empty());
            let bytes = postcard::to_allocvec(&event).unwrap();
            assert_eq!(
                postcard::from_bytes::<CoordinatorEvent>(&bytes).unwrap(),
                event
            );
        }
    }

    #[test]
    fn event_kinds_and_summaries() {
        let done = CoordinatorEvent::JobStateChanged {
            id: JobId([0; 16]),
            state: JobState::Failed,
            owner: "S23".to_string(),
            label: "Final".to_string(),
        };
        assert_eq!(done.kind(), EventKind::Problem);
        assert_eq!(done.summary(), "Final from S23 failed");
        let joined = CoordinatorEvent::WorkerJoined {
            id: WorkerId(1),
            name: "garage-rig".to_string(),
        };
        assert_eq!(joined.summary(), "garage-rig joined");
        assert_eq!(joined.kind(), EventKind::Good);
    }

    #[test]
    fn settings_patch_round_trips_and_default_is_empty() {
        assert!(SettingsPatch::default().is_empty());
        let patch = SettingsPatch {
            max_job_memory_mib: Some(4096),
            pinned_worker: Some(String::new()),
            network_compression: Some(NetworkCompression::Fast),
            job_retention_days: Some(7),
            allow_pairing: Some(false),
            render_mode: Some(RenderMode::CpuOnly),
            render_threads: Some(8),
            display_name: Some("Studio PC".to_string()),
            ..SettingsPatch::default()
        };
        assert!(!patch.is_empty());
        let text = serde_json::to_string(&patch).unwrap();
        assert_eq!(serde_json::from_str::<SettingsPatch>(&text).unwrap(), patch);
        let bytes = postcard::to_allocvec(&patch).unwrap();
        assert_eq!(
            postcard::from_bytes::<SettingsPatch>(&bytes).unwrap(),
            patch
        );
    }

    #[test]
    fn log_level_ordering_and_tracing_mapping() {
        assert!(LogLevel::Error.passes(LogLevel::Info));
        assert!(LogLevel::Info.passes(LogLevel::Info));
        assert!(!LogLevel::Debug.passes(LogLevel::Info));
        for level in [
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ] {
            let t: tracing::Level = level.into();
            assert_eq!(LogLevel::from(t), level);
        }
        assert_eq!(LogLevel::Warn.label(), "WARN");
    }

    #[test]
    fn issued_token_debug_is_redacted() {
        let issued = IssuedToken {
            id: TokenId(5),
            token: "SECRET-TOKEN-VALUE".to_string(),
            link: "pleochroma://pair?t=SECRET-TOKEN-VALUE".to_string(),
            expires_at: SystemTime::UNIX_EPOCH,
        };
        let text = format!("{issued:?}");
        assert!(!text.contains("SECRET"));
        assert!(text.contains("<redacted>"));
        assert!(text.contains("TokenId(5)"));
        // serde still carries the secret, because the dialog needs it.
        let json = serde_json::to_string(&issued).unwrap();
        let back: IssuedToken = serde_json::from_str(&json).unwrap();
        assert_eq!(back, issued);
    }

    #[test]
    fn job_id_hex_round_trip() {
        let id = JobId([0xab; 16]);
        assert_eq!(id.to_hex(), "ab".repeat(16));
        assert_eq!(JobId::from_hex(&id.to_hex()), Some(id));
        assert_eq!(JobId::from_hex("zz"), None);
        assert_eq!(JobId::from_hex(&"g".repeat(32)), None);
        let text = serde_json::to_string(&id).unwrap();
        assert_eq!(text, format!("\"{}\"", "ab".repeat(16)));
        assert_eq!(serde_json::from_str::<JobId>(&text).unwrap(), id);
        assert!(serde_json::from_str::<JobId>("\"abc\"").is_err());
        let raw: [u8; 16] = id.into();
        assert_eq!(JobId::from(raw), id);
    }

    #[test]
    fn fingerprint_short_form() {
        assert_eq!(fingerprint_short(&"1234abcd".repeat(8)), "1234abcd");
        assert_eq!(fingerprint_short("ab"), "ab");
        let device = DeviceStatus::new("x", "00ff".repeat(16), PeerRole::Admin);
        assert_eq!(device.fingerprint_short, "00ff00ff");
        assert!(device.raw_fingerprint().is_some());
    }

    #[test]
    fn capabilities_local_admin_monitor() {
        let local = Capabilities::local();
        assert!(local.is_local() && !local.is_remote() && !local.is_read_only());
        assert!(local.stop && local.change_network && local.can_issue(PeerRole::Admin));

        let admin = Capabilities::admin();
        assert_eq!(admin.role, AccessRole::Admin);
        assert!(admin.is_remote() && !admin.is_read_only());
        assert!(admin.revoke && admin.apply_settings && admin.restart);
        assert!(admin.can_issue(PeerRole::Viewer) && admin.can_issue(PeerRole::Worker));
        assert!(!admin.can_issue(PeerRole::Admin) && !admin.can_issue(PeerRole::Monitor));
        assert!(!admin.stop && !admin.change_network && !admin.change_remote_admin);
        assert!(!admin.manage_secrets);

        let monitor = Capabilities::monitor();
        assert!(monitor.view && monitor.fetch_results);
        assert!(monitor.is_read_only());
        assert!(!monitor.can_issue(PeerRole::Viewer));
        assert!(!monitor.revoke && !monitor.restart && !monitor.apply_settings);

        let none = Capabilities::none(AccessRole::Monitor);
        assert!(!none.view && none.is_read_only());
        assert_eq!(Capabilities::local().denied_reason(), "");
        assert!(!monitor.denied_reason().is_empty());
        assert!(!admin.denied_reason().is_empty());
    }

    #[test]
    fn capabilities_round_trip() {
        for caps in [
            Capabilities::local(),
            Capabilities::admin(),
            Capabilities::monitor(),
        ] {
            let text = serde_json::to_string(&caps).unwrap();
            assert_eq!(serde_json::from_str::<Capabilities>(&text).unwrap(), caps);
        }
    }
}
