//! A snapshot of the running coordinator, as the shared [`CoordinatorStatus`] type.
//!
//! [`status`] reads everything a [`ServeCore`] (what a `ServeHandle` derefs to) knows and puts
//! it into one snapshot:
//!
//! - **devices**: every allowlist entry (viewers, render computers, monitors, administrators)
//!   merged with the live [`ConnectionTable`](super::table::ConnectionTable): online or
//!   offline, address, connected since. A connection whose device is on no list (the
//!   "any client certificate" mode) is listed too. The role comes from the Common Name;
//! - **render computers**: this computer's own lane when it renders, then the joined
//!   computers (one row per computer: its `--slots` connections are grouped by certificate
//!   label);
//! - **jobs**: the job store across all owners (see [`super::jobs`]);
//! - **renders**: what viewers have asked for and is traced now, live views, final pictures and
//!   the rest (see [`super::renders`]); a device with one shows "Rendering", and the own lane
//!   counts as busy while a render's route includes this computer;
//! - **pending pairing codes**, without their secrets;
//! - **library**, **addresses** (the listening ports and this computer's LAN addresses),
//!   the **flags** and the **warnings** the Overview shows.
//!
//! The call is cheap enough for a one-second refresh: the operating-system probes (firewall
//! and network profile, which can take up to three seconds) run on a thread of their own and
//! the snapshot uses their last answer.
//!
//! What it cannot say yet: how many designs the library holds (`designs` stays `None`), which
//! design a job shows (`design_name` stays empty). A joined computer that is working shows
//! "Rendering"; a viewer shows "Rendering" while its job or one of its renders runs.

use super::{api::ListedDevice, table::ConnectionInfo};
use crate::{
    cli::{ComputeMode, ServeArgs},
    serve::ServeCore,
};
use pleochroma_net::{
    admin::status::{
        CapacitySummary, CoordinatorState, CoordinatorStatus, DeviceStatus, LibraryStatus,
        ListenAddresses, OwnRenderStatus, RenderComputerStatus, RenderHardware, RenderMode,
        UnixSeconds, WorkerId,
    },
    messages::PROTOCOL_VERSION,
    netinfo::{LanAddress, lan_ipv4},
    tls::{Fingerprint, fingerprint_to_hex},
};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Mutex, PoisonError},
    thread,
    time::{Duration, Instant, SystemTime},
};

/// The warning the Devices page's red banner reads while any client certificate is accepted.
pub const ANY_CERTIFICATE_WARNING: &str = "Any client certificate is accepted";

/// The warning while connections are not encrypted (`--insecure-no-tls`, headless only).
pub const NO_ENCRYPTION_WARNING: &str = "Encryption is off: connections are plain text";

/// What a running job's route says (jobs render on this computer's own lane).
#[cfg(feature = "worker")]
const THIS_COMPUTER: &str = "this computer";

/// The picture size, in pixels, a measured rate is quoted for (samples per second at one
/// megapixel).
#[cfg(feature = "worker")]
const RATE_PIXELS: u32 = 1_000_000;

/// How long the operating-system probe answer is reused.
const PROBE_TTL: Duration = Duration::from_secs(30);

/// How long the LAN address list is reused.
const LAN_TTL: Duration = Duration::from_secs(5);

/// Now, in unix seconds (0 if the clock is before the epoch).
#[must_use]
pub fn unix_now() -> UnixSeconds {
    unix_seconds(SystemTime::now())
}

/// A [`SystemTime`] as unix seconds (0 for a time before the epoch).
#[must_use]
pub fn unix_seconds(time: SystemTime) -> UnixSeconds {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The name the coordinator shows for this computer: its real host name
/// ([`crate::settings::computer_name`]), or "This computer".
#[must_use]
pub fn default_display_name() -> String {
    crate::settings::computer_name().unwrap_or_else(|| "This computer".to_string())
}

/// The build identity both sides must share (the `pleochroma` build hash, 16 hex
/// characters); empty on a build without render support, which has no physics build.
#[cfg(feature = "worker")]
#[must_use]
pub fn build_identity() -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(16);
    for b in pleochroma_net::handshake::local_build_hash() {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The build identity both sides must share; empty on this build, which has no render support
/// and so no physics build.
#[cfg(not(feature = "worker"))]
#[must_use]
pub const fn build_identity() -> String {
    String::new()
}

/// The name this coordinator goes by: the display name of the window's settings (or this
/// computer's name when it is empty), and this computer's name for a headless `serve`.
#[must_use]
pub fn configured_display_name() -> String {
    #[cfg(feature = "gui")]
    {
        crate::settings::current().display_name()
    }
    #[cfg(not(feature = "gui"))]
    {
        default_display_name()
    }
}

/// The extra names (VPN or DNS) the server certificate lists besides the LAN addresses: the
/// window's `extra_cert_names` setting; none for a headless `serve`.
#[cfg(feature = "gui")]
#[must_use]
pub fn extra_certificate_names() -> Vec<String> {
    crate::settings::current().extra_cert_names
}

/// The extra names the server certificate lists: none on a build without the window.
#[cfg(not(feature = "gui"))]
#[must_use]
pub const fn extra_certificate_names() -> Vec<String> {
    Vec::new()
}

/// Whether the discovery setting is on: the coordinator then announces itself on the network.
///
/// This is the one place that reads the setting. It comes from the coordinator window's
/// settings (`crate::settings::current()`), so a `serve` that never loaded a settings file
/// (headless, or a test) never announces: before the file is loaded the settings are only the
/// defaults, and those must not switch the announcement on.
#[cfg(feature = "gui")]
#[must_use]
pub fn discovery_setting() -> bool {
    crate::settings::settings_file().is_some() && crate::settings::current().discovery
}

/// The discovery setting: always off on a build without the window.
#[cfg(not(feature = "gui"))]
#[must_use]
pub const fn discovery_setting() -> bool {
    false
}

/// What `serve` was started with that the status reports, kept in the [`ServeCore`].
#[derive(Debug, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a snapshot of independent start-up switches"
)]
pub struct ServeInfo {
    /// When the coordinator started running.
    pub started: SystemTime,
    /// The name shown for this coordinator.
    pub display_name: String,
    /// Whether new devices may pair (the enrollment listeners run).
    pub allow_pairing: bool,
    /// `--trust-any-client-cert`: any certificate of the right role is accepted.
    pub trust_any_certificate: bool,
    /// False with `--insecure-no-tls`.
    pub encrypted: bool,
    /// Whether the coordinator announces itself on the network (the announcement started).
    pub discovery: bool,
    /// `--allow-remote-admin` with encryption on: monitors and administrators on other
    /// computers may connect to the viewer port.
    pub remote_admin: bool,
    /// `--render`: this computer renders with its own lane.
    pub render: bool,
    /// Which engines the own lane uses.
    pub compute_mode: ComputeMode,
    /// `--threads` (0 means automatic).
    pub threads: usize,
    /// The library file, when it could be opened; `None` in rendering-only mode.
    pub library: Option<PathBuf>,
    /// The path the jobs and the asset cache sit next to (the configured library path).
    pub library_anchor: PathBuf,
}

impl ServeInfo {
    /// The facts for a `serve` started with `args`. `library_present` is whether the library
    /// database opened; `discovery` whether the announcement started.
    #[must_use]
    pub fn new(
        args: &ServeArgs,
        library_anchor: PathBuf,
        library_present: bool,
        discovery: bool,
    ) -> Self {
        Self {
            started: SystemTime::now(),
            display_name: configured_display_name(),
            allow_pairing: !args.no_enroll && !args.insecure_no_tls,
            trust_any_certificate: args.trust_any_client_cert && !args.insecure_no_tls,
            encrypted: !args.insecure_no_tls,
            discovery,
            // A remote administrator needs a certificate, so never without encryption.
            remote_admin: args.allow_remote_admin && !args.insecure_no_tls,
            render: args.render,
            compute_mode: args.compute_mode,
            threads: args.threads,
            library: library_present.then(|| library_anchor.clone()),
            library_anchor,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Probes with a cache
// ---------------------------------------------------------------------------------------------

/// The last answer of a slow probe, refreshed on a thread of its own.
struct ProbeState {
    warnings: Vec<String>,
    taken: Option<Instant>,
    ports: Vec<u16>,
    running: bool,
}

/// A probe whose answer is cached and recomputed in the background.
struct ProbeCache {
    state: Mutex<ProbeState>,
}

impl ProbeCache {
    const fn new() -> Self {
        Self {
            state: Mutex::new(ProbeState {
                warnings: Vec::new(),
                taken: None,
                ports: Vec::new(),
                running: false,
            }),
        }
    }

    /// The warnings of the last finished probe (empty before the first one ends). When the
    /// answer is older than [`PROBE_TTL`], or was taken for other ports, a new probe starts on
    /// a background thread; this call never waits for it.
    fn get(&'static self, ports: &[u16], probe: fn(&[u16]) -> Vec<String>) -> Vec<String> {
        let (answer, start) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let outdated =
                state.taken.is_none_or(|taken| taken.elapsed() > PROBE_TTL) || state.ports != ports;
            let start = outdated && !state.running;
            if start {
                state.running = true;
            }
            (state.warnings.clone(), start)
        };
        if start {
            let ports = ports.to_vec();
            let spawned = thread::Builder::new()
                .name("network-probe".to_owned())
                .spawn(move || {
                    let warnings = probe(&ports);
                    let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                    state.warnings = warnings;
                    state.taken = Some(Instant::now());
                    state.ports = ports;
                    state.running = false;
                });
            if spawned.is_err() {
                self.state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .running = false;
            }
        }
        answer
    }
}

/// The firewall and network-profile warnings of the operating system for `ports`. Blocks for
/// up to three seconds: only [`ProbeCache`] calls it, on its own thread.
fn probe_network(ports: &[u16]) -> Vec<String> {
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    {
        crate::platform::network_warnings(
            crate::platform::firewall_state(ports),
            crate::platform::network_profile(),
            ports,
        )
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = ports;
        Vec::new()
    }
}

/// The cached network probe.
static NETWORK: ProbeCache = ProbeCache::new();

/// This computer's LAN addresses as display text, reused for [`LAN_TTL`].
static LAN: Mutex<Option<(Instant, Vec<String>)>> = Mutex::new(None);

/// The LAN addresses as display text, default route first: the physical adapters' addresses,
/// or every address when there is no physical one.
#[must_use]
pub fn lan_strings(lan: &[LanAddress]) -> Vec<String> {
    let physical: Vec<&LanAddress> = lan.iter().filter(|a| !a.virtual_adapter).collect();
    let chosen: Vec<&LanAddress> = if physical.is_empty() {
        lan.iter().collect()
    } else {
        physical
    };
    chosen.into_iter().map(|a| a.addr.to_string()).collect()
}

/// The LAN addresses of this computer, as [`lan_strings`] shows them; the interface scan is
/// repeated at most every few seconds.
fn lan_display() -> Vec<String> {
    {
        let cache = LAN.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((taken, list)) = cache.as_ref()
            && taken.elapsed() < LAN_TTL
        {
            return list.clone();
        }
    }
    // The interface scan runs without the lock, so another status never waits for it.
    let list = lan_strings(&lan_ipv4());
    *LAN.lock().unwrap_or_else(PoisonError::into_inner) = Some((Instant::now(), list.clone()));
    list
}

// ---------------------------------------------------------------------------------------------
// Pure builders (tested without a running coordinator)
// ---------------------------------------------------------------------------------------------

/// The peer's IP address as display text.
fn peer_text(peer: std::net::SocketAddr) -> String {
    peer.ip().to_canonical().to_string()
}

/// Marks `row` online from its live connections (the first is the oldest).
fn apply_live(row: &mut DeviceStatus, connections: &[&ConnectionInfo], now: UnixSeconds) {
    let Some(first) = connections.first() else {
        return;
    };
    row.online = true;
    row.last_seen = Some(now);
    row.connected_since = connections
        .iter()
        .map(|c| unix_seconds(c.connected_since))
        .min();
    row.address = connections.iter().find_map(|c| c.peer.map(peer_text));
    if row.name.is_empty() {
        row.name.clone_from(&first.name);
    }
}

/// Merges the allowlists with the live connections: one row per device.
///
/// A listed device with no connection is offline; a live connection of a device that is on no list
/// (the "any certificate" mode, or a device removed while connected) gets a row of its own, with
/// the role of its certificate.
#[must_use]
pub fn merge_devices(
    listed: &[ListedDevice],
    live: &[ConnectionInfo],
    now: UnixSeconds,
) -> Vec<DeviceStatus> {
    let mut rows = Vec::with_capacity(listed.len());
    let mut seen: HashSet<Fingerprint> = HashSet::new();
    for device in listed {
        if !seen.insert(device.fingerprint) {
            continue;
        }
        let mut row = DeviceStatus::new(
            device.name.clone(),
            fingerprint_to_hex(&device.fingerprint),
            device.role,
        );
        let connections: Vec<&ConnectionInfo> = live
            .iter()
            .filter(|c| c.fingerprint == device.fingerprint)
            .collect();
        apply_live(&mut row, &connections, now);
        rows.push(row);
    }
    for connection in live {
        if !seen.insert(connection.fingerprint) {
            continue;
        }
        let mut row = DeviceStatus::new(
            connection.name.clone(),
            fingerprint_to_hex(&connection.fingerprint),
            connection.role,
        );
        let connections: Vec<&ConnectionInfo> = live
            .iter()
            .filter(|c| c.fingerprint == connection.fingerprint)
            .collect();
        apply_live(&mut row, &connections, now);
        rows.push(row);
    }
    rows
}

/// Says "Rendering ..." on the device whose job runs now. `running` is the running job's owner
/// key (`certificate:<hex>`) and label.
pub fn mark_activity(devices: &mut [DeviceStatus], running: Option<(&str, &str)>) {
    let Some((owner, label)) = running else {
        return;
    };
    let Some(hex) = owner.strip_prefix("certificate:") else {
        return;
    };
    for device in devices.iter_mut().filter(|d| d.fingerprint == hex) {
        device.activity = format!("Rendering {label}");
    }
}

/// The thread count of the own lane: the setting, or the machine's when it is 0.
fn effective_threads(threads: usize) -> u32 {
    let count = if threads == 0 {
        thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
    } else {
        threads
    };
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// The own lane as a row of the render computers list and as the "Render on this computer"
/// block. `gpu` is the graphics card's name when one is in use.
#[must_use]
pub fn own_lane(
    info: &ServeInfo,
    gpu: Option<&str>,
    busy: bool,
    rate: Option<f32>,
) -> (RenderComputerStatus, OwnRenderStatus) {
    let threads = effective_threads(info.threads);
    let (hardware, mode) = match info.compute_mode {
        ComputeMode::OnlyCpu => (RenderHardware::Cpu, RenderMode::CpuOnly),
        ComputeMode::OnlyGpu => (RenderHardware::Gpu, RenderMode::GpuOnly),
        ComputeMode::Hybrid => (
            if gpu.is_some() {
                RenderHardware::GpuAndCpu
            } else {
                RenderHardware::Cpu
            },
            RenderMode::GpuAndCpu,
        ),
    };
    let uses_gpu = mode != RenderMode::CpuOnly;
    let uses_threads = mode != RenderMode::GpuOnly;
    let gpu_name = if uses_gpu {
        gpu.unwrap_or_default().to_owned()
    } else {
        String::new()
    };
    let row_threads = if uses_threads { threads } else { 0 };
    let mut parts: Vec<String> = Vec::new();
    if !gpu_name.is_empty() {
        parts.push(gpu_name.clone());
    }
    if uses_threads {
        parts.push(format!("{threads} threads"));
    }
    let row = RenderComputerStatus {
        id: WorkerId(0),
        name: info.display_name.clone(),
        fingerprint: String::new(),
        own_lane: true,
        hardware,
        hardware_name: gpu_name,
        threads: row_threads,
        slots: 1,
        busy: u32::from(busy),
        rate,
        platform: "This computer".to_owned(),
        connected_since: None,
        activity: if busy {
            "Rendering".to_owned()
        } else {
            String::new()
        },
        address: None,
    };
    let own = OwnRenderStatus {
        enabled: info.render,
        mode,
        threads: u32::try_from(info.threads).ok().filter(|t| *t != 0),
        hardware: parts.join(" \u{b7} "),
        busy,
    };
    (row, own)
}

/// The largest picture, as width and height, that holds at most `pixels` pixels in 16:9
/// (`(0, 0)` for none).
#[must_use]
pub fn picture_size(pixels: u32) -> (u32, u32) {
    let wide = u64::from(pixels) * 16 / 9;
    let width = u32::try_from(wide.isqrt()).unwrap_or(u32::MAX);
    (width, width / 16 * 9)
}

/// The Render computers footer: the own lane (if it renders) plus what joined.
#[must_use]
pub fn capacity(
    own: Option<&RenderComputerStatus>,
    joined_gpus: u32,
    joined_threads: u32,
    max_pixels: u32,
) -> CapacitySummary {
    let own_gpu = own.is_some_and(|row| !row.hardware_name.is_empty());
    let (max_width, max_height) = picture_size(max_pixels);
    CapacitySummary {
        gpus: joined_gpus.saturating_add(u32::from(own_gpu)),
        threads: joined_threads.saturating_add(own.map_or(0, |row| row.threads)),
        max_width,
        max_height,
    }
}

/// The library block: the path when the library opened, else "no library".
fn library_status(info: &ServeInfo) -> LibraryStatus {
    LibraryStatus {
        path: info.library.as_ref().map(|p| p.display().to_string()),
        designs: None,
        read_only: true,
    }
}

/// The warnings for the Overview: what is unsafe about the configuration, then the operating
/// system's `network` warnings.
#[must_use]
pub fn warnings(info: &ServeInfo, network: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    if info.trust_any_certificate {
        out.push(ANY_CERTIFICATE_WARNING.to_owned());
    }
    if !info.encrypted {
        out.push(NO_ENCRYPTION_WARNING.to_owned());
    }
    out.extend(network);
    out
}

/// The listening addresses of a started coordinator.
#[must_use]
pub fn listen_addresses(handle: &ServeCore) -> ListenAddresses {
    ListenAddresses {
        viewer: Some(handle.viewer_addr),
        viewer_enroll: handle.viewer_enroll_addr,
        worker: handle.worker_addr,
        worker_enroll: handle.worker_enroll_addr,
        lan: lan_display(),
    }
}

/// The TCP ports this coordinator listens on (for the firewall hint).
fn listen_ports(addresses: &ListenAddresses) -> Vec<u16> {
    [
        addresses.viewer,
        addresses.viewer_enroll,
        addresses.worker,
        addresses.worker_enroll,
    ]
    .into_iter()
    .flatten()
    .map(|addr| addr.port())
    .collect()
}

/// The network warnings, only for a coordinator that other computers can reach.
fn network_warnings_for(addresses: &ListenAddresses) -> Vec<String> {
    let reachable = addresses
        .viewer
        .is_some_and(|addr| !addr.ip().is_loopback());
    if reachable {
        NETWORK.get(&listen_ports(addresses), probe_network)
    } else {
        Vec::new()
    }
}

/// The coordinator's full status.
///
/// State `Running`, the version, protocol, build, the listening ports and LAN addresses, the
/// devices (allowlists merged with the live connections), render computers, jobs, pending
/// pairing codes, the library and the warnings; see the module doc comment. Never blocks on
/// the network.
#[must_use]
pub fn status(handle: &ServeCore) -> CoordinatorStatus {
    let info = &handle.info;
    let now = unix_now();
    // Read now, not at start-up, so a rename in the settings shows without a restart.
    let display_name = configured_display_name();
    let mut status = CoordinatorStatus::minimal(
        display_name.as_str(),
        env!("CARGO_PKG_VERSION"),
        PROTOCOL_VERSION,
        CoordinatorState::Running,
    );
    status.build = build_identity();
    status.started = Some(unix_seconds(info.started));
    status.addresses = listen_addresses(handle);

    let live = handle.control.table().list();
    status.devices = merge_devices(&handle.control.listed_devices_cached(), &live, now);
    status.pending_tokens = handle.control.pending_tokens();
    status.library = library_status(info);
    status.remote_admin_enabled = info.remote_admin;
    status.allow_pairing = info.allow_pairing;
    status.discovery = info.discovery;
    #[cfg(feature = "worker")]
    fill_render(&mut status, handle, &live, &display_name);
    status.warnings = warnings(info, network_warnings_for(&status.addresses));
    status
}

/// Fills the render computers, the own lane, the capacity and the jobs (`worker` builds).
#[cfg(feature = "worker")]
fn fill_render(
    status: &mut CoordinatorStatus,
    handle: &ServeCore,
    live: &[ConnectionInfo],
    display_name: &str,
) {
    use super::{
        jobs::{any_running, job_rows, running_job},
        renders,
    };
    use crate::coordinator::{LaneKey, WorkerInfo};

    let info = &handle.info;
    let jobs = handle
        .job_admin
        .as_ref()
        .map(super::jobs::JobAdmin::list_cached)
        .unwrap_or_default();
    let names = handle.control.device_names();
    status.jobs = job_rows(&jobs, &names, THIS_COMPUTER);
    if let Some(admin) = &handle.job_admin {
        status.jobs_summary = admin.summary(&jobs, &info.library_anchor);
    }
    mark_activity(&mut status.devices, running_job(&jobs));
    // The renders viewers asked for: a short copy behind a short lock, never a wait for a render.
    status.renders = renders::snapshot();
    renders::apply_device_names(&mut status.renders, &names);
    renders::mark_device_activity(&mut status.devices, &status.renders);

    let rates = handle
        .coordinator
        .as_ref()
        .map(|coordinator| std::sync::Arc::clone(coordinator.rates()));
    let rate_of = |worker: &WorkerInfo| {
        rates.as_ref().and_then(|book| {
            book.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .worker(worker, RATE_PIXELS)
                .map(single)
        })
    };
    let own_rate = rates.as_ref().and_then(|book| {
        book.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&LaneKey::Own, RATE_PIXELS)
            .map(single)
    });
    let own_gpu = handle
        .coordinator
        .as_ref()
        .and_then(|coordinator| coordinator.own())
        .and_then(|own| own.gpu.adapter_label());
    let own_busy = renders::own_lane_busy(any_running(&jobs), &status.renders);
    let (mut own_row, own_render) = own_lane(info, own_gpu.as_deref(), own_busy, own_rate);
    display_name.clone_into(&mut own_row.name);
    status.own_render = own_render;
    // Never waits for a render: a small copy behind a short lock.
    status.shader_cache = crate::serve::warmup::state();

    let workers = handle
        .registry
        .as_ref()
        .map(|registry| registry.workers())
        .unwrap_or_default();
    let mut computers = Vec::new();
    if info.render {
        computers.push(own_row);
    }
    computers.extend(group_render_computers(&workers, live, rate_of));
    let joined = handle
        .registry
        .as_ref()
        .map(|registry| registry.capacity())
        .unwrap_or_default();
    let own_pixels = if info.render {
        crate::validate::MAX_PIXELS
    } else {
        0
    };
    status.capacity = capacity(
        computers.first().filter(|row| row.own_lane),
        joined.gpus,
        joined.threads,
        own_pixels.max(joined.max_pixels),
    );
    status.render_computers = computers;
}

/// A rate as the status carries it.
#[cfg(feature = "worker")]
#[expect(
    clippy::cast_possible_truncation,
    reason = "a samples-per-second figure is shown rounded, an f32 is plenty"
)]
const fn single(value: f64) -> f32 {
    value as f32
}

/// The name a joined render computer is shown under: the certificate's label, else a
/// placeholder with its registry id.
#[cfg(feature = "worker")]
#[must_use]
pub fn worker_display_name(info: &crate::coordinator::WorkerInfo) -> String {
    info.label
        .clone()
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| format!("Render computer {}", info.worker_id))
}

/// One row per joined computer.
///
/// The connections a `join --slots K` computer opens share its certificate label and become one
/// row: `slots` counts them, `busy` the ones a job holds, and the id is the lowest of them. `live`
/// supplies the certificate fingerprint and the join time (matched by peer address); `rate_of` the
/// measured speed.
#[cfg(feature = "worker")]
#[must_use]
pub fn group_render_computers(
    workers: &[(crate::coordinator::WorkerInfo, bool)],
    live: &[ConnectionInfo],
    rate_of: impl Fn(&crate::coordinator::WorkerInfo) -> Option<f32>,
) -> Vec<RenderComputerStatus> {
    use pleochroma_net::messages::Backend;

    let mut groups: Vec<Vec<&(crate::coordinator::WorkerInfo, bool)>> = Vec::new();
    for entry in workers {
        let existing = groups.iter_mut().find(|group| {
            entry.0.label.is_some() && group.first().is_some_and(|f| f.0.label == entry.0.label)
        });
        match existing {
            Some(group) => group.push(entry),
            None => groups.push(vec![entry]),
        }
    }
    groups
        .into_iter()
        .filter_map(|group| {
            let first = &group.first()?.0;
            let connections: Vec<&ConnectionInfo> = group
                .iter()
                .filter_map(|(info, _)| {
                    live.iter().find(|c| {
                        c.role == pleochroma_net::messages::PeerRole::Worker
                            && c.peer.is_some()
                            && c.peer == info.peer
                    })
                })
                .collect();
            let busy = group.iter().filter(|(_, idle)| !*idle).count();
            let (hardware, hardware_name, threads) = match &first.capability.backend {
                Backend::Cpu { threads } => (RenderHardware::Cpu, String::new(), *threads),
                Backend::Gpu { adapter } => (RenderHardware::Gpu, adapter.clone(), 0),
                Backend::Coordinator { threads, .. } => (
                    RenderHardware::GpuAndCpu,
                    "Another coordinator".to_owned(),
                    *threads,
                ),
            };
            Some(RenderComputerStatus {
                id: WorkerId(first.worker_id),
                name: worker_display_name(first),
                fingerprint: connections
                    .first()
                    .map(|c| fingerprint_to_hex(&c.fingerprint))
                    .unwrap_or_default(),
                own_lane: false,
                hardware,
                hardware_name,
                threads,
                slots: u32::try_from(group.len()).unwrap_or(u32::MAX),
                busy: u32::try_from(busy).unwrap_or(u32::MAX),
                rate: rate_of(first),
                platform: String::new(),
                connected_since: connections
                    .iter()
                    .map(|c| unix_seconds(c.connected_since))
                    .min(),
                activity: if busy > 0 {
                    "Rendering".to_owned()
                } else {
                    String::new()
                },
                address: first.peer.map(peer_text),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_net::messages::PeerRole;
    use std::net::SocketAddr;

    fn connection(
        id: u64,
        name: &str,
        fp: u8,
        role: PeerRole,
        peer: &str,
        since: u64,
    ) -> ConnectionInfo {
        ConnectionInfo {
            id,
            name: name.to_owned(),
            fingerprint: [fp; 32],
            role,
            peer: Some(peer.parse::<SocketAddr>().unwrap()),
            connected_since: SystemTime::UNIX_EPOCH + Duration::from_secs(since),
        }
    }

    fn listed(fp: u8, name: &str, role: PeerRole) -> ListedDevice {
        ListedDevice {
            fingerprint: [fp; 32],
            name: name.to_owned(),
            role,
        }
    }

    fn info_for(extra: &[&str]) -> ServeInfo {
        let mut words: Vec<String> = vec!["serve".to_owned()];
        words.extend(extra.iter().map(|word| (*word).to_owned()));
        let Ok(crate::cli::Command::Serve(args)) = crate::cli::parse(&words) else {
            panic!("the serve arguments parse");
        };
        ServeInfo::new(&args, PathBuf::from("library.sqlite"), true, false)
    }

    #[test]
    fn unix_seconds_of_epoch_is_zero() {
        assert_eq!(unix_seconds(SystemTime::UNIX_EPOCH), 0);
        assert!(unix_now() > 1_600_000_000);
    }

    #[test]
    fn display_name_is_never_empty() {
        assert!(!default_display_name().is_empty());
    }

    #[test]
    fn the_allowlist_and_the_table_give_online_and_offline_rows() {
        let list = [
            listed(1, "Niklas's S23", PeerRole::Viewer),
            listed(2, "Tablet", PeerRole::Viewer),
            listed(3, "garage-rig", PeerRole::Worker),
        ];
        let live = [
            connection(
                0,
                "Niklas's S23",
                1,
                PeerRole::Viewer,
                "192.168.178.42:5000",
                1_000,
            ),
            connection(
                1,
                "Niklas's S23",
                1,
                PeerRole::Viewer,
                "192.168.178.42:5001",
                900,
            ),
            connection(
                2,
                "garage-rig",
                3,
                PeerRole::Worker,
                "192.168.178.40:6000",
                800,
            ),
        ];
        let rows = merge_devices(&list, &live, 2_000);
        assert_eq!(rows.len(), 3);

        let phone = &rows[0];
        assert!(phone.online);
        assert_eq!(phone.role, PeerRole::Viewer);
        assert_eq!(
            phone.connected_since,
            Some(900),
            "the oldest connection counts"
        );
        assert_eq!(phone.address.as_deref(), Some("192.168.178.42"));
        assert_eq!(phone.last_seen, Some(2_000));
        assert_eq!(phone.fingerprint, fingerprint_to_hex(&[1; 32]));

        let tablet = &rows[1];
        assert!(!tablet.online);
        assert_eq!(tablet.connected_since, None);
        assert_eq!(tablet.address, None);
        assert_eq!(tablet.last_seen, None);

        let rig = &rows[2];
        assert_eq!(
            rig.role,
            PeerRole::Worker,
            "a worker row for the Render computers page"
        );
        assert!(rig.online);
        assert_eq!(rig.name, "garage-rig");
    }

    #[test]
    fn a_connection_that_is_on_no_list_still_gets_a_row() {
        let live = [connection(
            0,
            "guest",
            9,
            PeerRole::Viewer,
            "10.0.0.8:1234",
            5,
        )];
        let rows = merge_devices(&[], &live, 10);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].online);
        assert_eq!(rows[0].name, "guest");
        assert_eq!(rows[0].fingerprint_short, fingerprint_to_hex(&[9; 32])[..8]);
        assert!(merge_devices(&[], &[], 10).is_empty());
    }

    #[test]
    fn a_device_on_two_lists_is_one_row() {
        let list = [
            listed(1, "laptop", PeerRole::Viewer),
            listed(1, "laptop", PeerRole::Viewer),
        ];
        assert_eq!(merge_devices(&list, &[], 0).len(), 1);
    }

    #[test]
    fn a_running_job_marks_its_owner_device() {
        let mut rows = vec![
            DeviceStatus::new("A", fingerprint_to_hex(&[1; 32]), PeerRole::Viewer),
            DeviceStatus::new("B", fingerprint_to_hex(&[2; 32]), PeerRole::Viewer),
        ];
        let owner = format!("certificate:{}", fingerprint_to_hex(&[2; 32]));
        mark_activity(&mut rows, Some((&owner, "Final render")));
        assert!(rows[0].activity.is_empty());
        assert_eq!(rows[1].activity, "Rendering Final render");
        mark_activity(&mut rows, None);
        mark_activity(&mut rows, Some(("address:10.0.0.1", "x")));
    }

    #[test]
    fn the_any_certificate_mode_warns_and_the_default_does_not() {
        let any = info_for(&["--trust-any-client-cert", "--ca", "ca.pem"]);
        assert!(any.trust_any_certificate);
        let warned = warnings(&any, Vec::new());
        assert_eq!(warned, vec![ANY_CERTIFICATE_WARNING.to_owned()]);

        let normal = info_for(&["--ca", "ca.pem"]);
        assert!(!normal.trust_any_certificate);
        assert!(warnings(&normal, Vec::new()).is_empty());

        let plain = info_for(&["--insecure-no-tls"]);
        assert!(!plain.encrypted);
        assert!(!plain.allow_pairing);
        assert_eq!(
            warnings(&plain, Vec::new()),
            vec![NO_ENCRYPTION_WARNING.to_owned()]
        );

        let both = warnings(
            &any,
            vec!["The firewall may block other devices.".to_owned()],
        );
        assert_eq!(both.len(), 2);
        assert_eq!(
            both[0], ANY_CERTIFICATE_WARNING,
            "the red banner text comes first"
        );
    }

    // `--render` is only parsed on `worker` builds.
    #[cfg(feature = "worker")]
    #[test]
    fn the_flags_follow_the_arguments() {
        let info = info_for(&["--no-enroll", "--render", "--only-cpu", "--threads", "6"]);
        assert!(!info.allow_pairing);
        assert!(info.render);
        assert_eq!(info.compute_mode, ComputeMode::OnlyCpu);
        assert_eq!(info.threads, 6);
        assert!(info_for(&[]).allow_pairing);
        assert!(
            !info_for(&[]).remote_admin,
            "remote administration is off by default"
        );
        assert!(info_for(&["--allow-remote-admin", "--ca", "ca.pem"]).remote_admin);
        let library = library_status(&info);
        assert_eq!(library.path.as_deref(), Some("library.sqlite"));
        assert!(library.read_only);
        let mut missing = info;
        missing.library = None;
        assert!(library_status(&missing).is_none());
    }

    #[cfg(feature = "worker")]
    #[test]
    fn the_own_lane_follows_the_compute_mode() {
        let mut info = info_for(&["--render", "--threads", "8"]);
        let (row, own) = own_lane(&info, Some("Radeon 780M"), true, Some(12.5));
        assert!(row.own_lane);
        assert_eq!(row.id, WorkerId(0));
        assert_eq!(row.hardware, RenderHardware::GpuAndCpu);
        assert_eq!(row.hardware_name, "Radeon 780M");
        assert_eq!((row.threads, row.slots, row.busy), (8, 1, 1));
        assert_eq!(row.rate, Some(12.5));
        assert_eq!(row.activity, "Rendering");
        assert_eq!(own.mode, RenderMode::GpuAndCpu);
        assert_eq!(own.hardware, "Radeon 780M \u{b7} 8 threads");
        assert_eq!(own.threads, Some(8));
        assert!(own.enabled && own.busy);

        let (without_card, _) = own_lane(&info, None, false, None);
        assert_eq!(without_card.hardware, RenderHardware::Cpu);
        assert!(without_card.activity.is_empty());

        info.compute_mode = ComputeMode::OnlyGpu;
        let (gpu_row, gpu_own) = own_lane(&info, Some("RTX"), false, None);
        assert_eq!(gpu_row.hardware, RenderHardware::Gpu);
        assert_eq!(gpu_row.threads, 0);
        assert_eq!(gpu_own.mode, RenderMode::GpuOnly);
        assert_eq!(gpu_own.hardware, "RTX");

        info.compute_mode = ComputeMode::OnlyCpu;
        let (cpu_row, cpu_own) = own_lane(&info, Some("RTX"), false, None);
        assert_eq!(cpu_row.hardware, RenderHardware::Cpu);
        assert!(
            cpu_row.hardware_name.is_empty(),
            "a card the lane does not use is not named"
        );
        assert_eq!(cpu_own.mode, RenderMode::CpuOnly);
        assert_eq!(cpu_own.hardware, "8 threads");
    }

    #[cfg(feature = "worker")]
    #[test]
    fn capacity_adds_the_own_lane_to_what_joined() {
        let info = info_for(&["--render", "--threads", "4"]);
        let (row, _) = own_lane(&info, Some("Radeon 780M"), false, None);
        let total = capacity(Some(&row), 2, 16, 7680 * 4320);
        assert_eq!((total.gpus, total.threads), (3, 20));
        assert_eq!((total.max_width, total.max_height), (7680, 4320));
        let none = capacity(None, 0, 0, 0);
        assert_eq!(none, CapacitySummary::default());
        assert_eq!(picture_size(0), (0, 0));
    }

    #[test]
    fn lan_addresses_list_physical_adapters_first_and_virtual_only_as_a_fallback() {
        let lan = |a: [u8; 4], virtual_adapter: bool| LanAddress {
            addr: std::net::Ipv4Addr::from(a),
            interface: "x".to_owned(),
            default_route: false,
            virtual_adapter,
        };
        let mixed = [lan([192, 168, 178, 20], false), lan([172, 17, 0, 1], true)];
        assert_eq!(lan_strings(&mixed), vec!["192.168.178.20".to_owned()]);
        let only_virtual = [lan([172, 17, 0, 1], true)];
        assert_eq!(lan_strings(&only_virtual), vec!["172.17.0.1".to_owned()]);
        assert!(lan_strings(&[]).is_empty());
    }

    #[test]
    fn the_probe_runs_in_the_background_and_its_answer_is_cached() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        static CACHE: ProbeCache = ProbeCache::new();
        fn slow_probe(ports: &[u16]) -> Vec<String> {
            CALLS.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(200));
            vec![format!("ports {ports:?}")]
        }

        let begun = Instant::now();
        let first = CACHE.get(&[7878, 7880], slow_probe);
        assert!(
            first.is_empty(),
            "the first call does not wait for the probe"
        );
        assert!(
            begun.elapsed() < Duration::from_millis(150),
            "the caller's thread never blocks"
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut answer = Vec::new();
        while answer.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            answer = CACHE.get(&[7878, 7880], slow_probe);
        }
        assert_eq!(answer, vec!["ports [7878, 7880]".to_owned()]);
        assert_eq!(CACHE.get(&[7878, 7880], slow_probe), answer);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1, "a fresh answer is reused");
    }

    #[test]
    fn listen_ports_skip_closed_listeners() {
        let addresses = ListenAddresses {
            viewer: Some("0.0.0.0:7878".parse().unwrap()),
            viewer_enroll: Some("0.0.0.0:7879".parse().unwrap()),
            worker: None,
            worker_enroll: None,
            lan: Vec::new(),
        };
        assert_eq!(listen_ports(&addresses), vec![7878, 7879]);
        let loopback = ListenAddresses {
            viewer: Some("127.0.0.1:7878".parse().unwrap()),
            ..ListenAddresses::default()
        };
        assert!(
            network_warnings_for(&loopback).is_empty(),
            "loopback needs no firewall hint"
        );
    }

    #[test]
    fn a_running_coordinator_reports_itself() {
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-status-test-{}-{}",
            std::process::id(),
            unix_now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("absent.sqlite");
        let words: Vec<String> = [
            "serve",
            "--insecure-no-tls",
            "--bind",
            "127.0.0.1:0",
            "--no-enroll",
            "--no-workers",
            "--db",
        ]
        .iter()
        .map(|word| (*word).to_owned())
        .chain([db.display().to_string()])
        .collect();
        let Ok(crate::cli::Command::Serve(args)) = crate::cli::parse(&words) else {
            panic!("the serve arguments parse");
        };
        let handle = crate::serve::start(&args).expect("the coordinator starts");

        let status = status(&handle);
        assert_eq!(status.state, CoordinatorState::Running);
        assert_eq!(status.addresses.viewer, Some(handle.viewer_addr));
        assert!(status.started.is_some());
        assert!(status.library.is_none(), "no library file was found");
        assert!(!status.allow_pairing);
        assert!(!status.remote_admin_enabled);
        assert!(status.devices.is_empty());
        assert!(status.pending_tokens.is_empty());
        assert!(status.warnings.iter().any(|w| w == NO_ENCRYPTION_WARNING));

        assert!(handle.shutdown(Duration::from_secs(10)).is_clean());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "worker")]
    mod render {
        use super::*;
        use crate::coordinator::WorkerInfo;
        use pleochroma_net::messages::{Backend, PayloadEncoding, RenderCapability};

        fn worker(id: u32, label: Option<&str>, peer: &str, backend: Backend) -> WorkerInfo {
            WorkerInfo {
                worker_id: id,
                capability: RenderCapability {
                    backend,
                    max_pixels: 1_000_000,
                    min_cadence_ms: 50,
                    hdr: false,
                },
                peer: Some(peer.parse().unwrap()),
                label: label.map(str::to_owned),
                payload_encoding: PayloadEncoding::Raw,
            }
        }

        #[test]
        fn slots_of_one_computer_become_one_row() {
            let gpu = Backend::Gpu {
                adapter: "RTX 4070".to_owned(),
            };
            let workers = vec![
                (
                    worker(3, Some("garage-rig"), "192.168.178.40:6000", gpu.clone()),
                    true,
                ),
                (
                    worker(4, Some("garage-rig"), "192.168.178.40:6001", gpu),
                    false,
                ),
                (
                    worker(5, None, "192.168.178.41:6000", Backend::Cpu { threads: 12 }),
                    true,
                ),
            ];
            let live = [
                connection(
                    0,
                    "garage-rig",
                    7,
                    PeerRole::Worker,
                    "192.168.178.40:6000",
                    500,
                ),
                connection(
                    1,
                    "garage-rig",
                    7,
                    PeerRole::Worker,
                    "192.168.178.40:6001",
                    400,
                ),
            ];
            let rows = group_render_computers(&workers, &live, |_| Some(2.5));
            assert_eq!(rows.len(), 2);

            let rig = &rows[0];
            assert_eq!(rig.id, WorkerId(3));
            assert_eq!(rig.name, "garage-rig");
            assert_eq!((rig.slots, rig.busy), (2, 1));
            assert_eq!(rig.hardware, RenderHardware::Gpu);
            assert_eq!(rig.hardware_name, "RTX 4070");
            assert_eq!(rig.fingerprint, fingerprint_to_hex(&[7; 32]));
            assert_eq!(rig.connected_since, Some(400));
            assert_eq!(rig.address.as_deref(), Some("192.168.178.40"));
            assert_eq!(rig.rate, Some(2.5));
            assert_eq!(rig.activity, "Rendering");
            assert!(!rig.own_lane);

            let anonymous = &rows[1];
            assert_eq!(anonymous.name, "Render computer 5");
            assert_eq!(
                (anonymous.slots, anonymous.busy, anonymous.threads),
                (1, 0, 12)
            );
            assert_eq!(anonymous.hardware, RenderHardware::Cpu);
            assert!(
                anonymous.fingerprint.is_empty(),
                "no live connection matched"
            );
            assert!(anonymous.activity.is_empty());
        }

        #[test]
        fn a_running_render_shows_in_the_status_and_busies_the_own_lane() {
            use crate::control::renders::{self, ActiveRenderStart};
            use pleochroma_net::admin::status::RenderKind;

            let dir = std::env::temp_dir().join(format!(
                "pleochroma-status-renders-{}-{}",
                std::process::id(),
                unix_now()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let db = dir.join("absent.sqlite");
            let words: Vec<String> = [
                "serve",
                "--insecure-no-tls",
                "--bind",
                "127.0.0.1:0",
                "--no-enroll",
                "--no-workers",
                "--db",
            ]
            .iter()
            .map(|word| (*word).to_owned())
            .chain([db.display().to_string()])
            .collect();
            let Ok(crate::cli::Command::Serve(args)) = crate::cli::parse(&words) else {
                panic!("the serve arguments parse");
            };
            let handle = crate::serve::start(&args).expect("the coordinator starts");

            let before = status(&handle);
            assert!(before.renders.iter().all(|r| r.device != "t-status-phone"));

            let guard = renders::begin(ActiveRenderStart {
                device: "t-status-phone".to_owned(),
                fingerprint: String::new(),
                kind: RenderKind::Live,
                engine: renders::PATH_TRACED.to_owned(),
                width: 640,
                height: 360,
                target: None,
                route: renders::THIS_COMPUTER.to_owned(),
            });
            guard.progress(64);
            let during = status(&handle);
            let mine = during
                .renders
                .iter()
                .find(|r| r.id == guard.id())
                .expect("the running render is in the status");
            assert_eq!((mine.samples_done, mine.samples_target), (64, None));
            assert!(during.own_render.busy, "its route includes this computer");

            drop(guard);
            assert!(
                status(&handle)
                    .renders
                    .iter()
                    .all(|r| r.device != "t-status-phone")
            );
            assert!(handle.shutdown(Duration::from_secs(10)).is_clean());
            std::fs::remove_dir_all(&dir).ok();
        }

        #[test]
        fn connections_without_a_label_never_merge() {
            let cpu = Backend::Cpu { threads: 4 };
            let workers = vec![
                (worker(1, None, "10.0.0.2:1", cpu.clone()), true),
                (worker(2, None, "10.0.0.2:2", cpu), true),
            ];
            assert_eq!(group_render_computers(&workers, &[], |_| None).len(), 2);
            assert!(group_render_computers(&[], &[], |_| None).is_empty());
        }
    }
}
