//! The coordinator's control API: pairing codes, removing and renaming devices, and the event
//! stream, as methods on [`ServeCore`] (and so on a `ServeHandle`).
//!
//! [`ControlState`] holds everything those methods share: the two enrollment registries (one
//! per listener, so a code only claims where it was issued), the allowlist files, the
//! [`ConnectionTable`] of live connections and the [`EventHub`]. `serve` builds it with
//! [`ControlState::new`] from [`ControlParts`] and keeps it in `ServeCore::control`. The methods
//! of the API sit on [`ServeCore`], which a `ServeHandle` derefs to and the remote
//! administration sessions (`crate::admin`) hold directly.
//!
//! # Events
//!
//! A small background thread forwards the registries' [`ClaimEvent`]s as
//! [`CoordinatorEvent::TokenClaimed`] and reports pairing codes that ran out unused as
//! [`CoordinatorEvent::TokenExpired`]. It holds only a weak reference, so it ends by itself
//! after the last clone of the state is gone.
//!
//! # Secrets
//!
//! A code and a pairing link exist only in the returned [`IssuedToken`] (whose `Debug` hides
//! them). Nothing here logs or puts one into an error message or an event.

use super::{
    events::EventHub,
    status::{configured_display_name, unix_seconds},
    table::{ConnectionTable, device_name_of_common_name},
};
use crate::{
    cli::ServeArgs,
    enroll::{ClaimEvent, EnrollRegistry, TokenId as EnrollTokenId},
    pki::{
        self,
        role::{common_name_for, role_of_common_name},
    },
    serve::{Auth, ServeCore},
};
#[cfg(feature = "worker")]
use pleochroma_net::admin::status::JobState;
use pleochroma_net::{
    admin::status::{
        CoordinatorEvent, IssuedToken, JobId, PendingToken, TokenId, UnixSeconds, WorkerId,
        fingerprint_short,
    },
    messages::PeerRole,
    netinfo::{LanAddress, lan_ipv4},
    pairing::{MAX_ADDRS, MAX_NAME_BYTES, PairingLink},
    tls::{Allowlist, Fingerprint, fingerprint_to_hex, sanitize_allowlist_label},
};
use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc::Receiver,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

/// How often the event thread looks for claims and expired codes.
const PUMP_INTERVAL: Duration = Duration::from_millis(200);

/// Name used for a new device when the caller gives none.
const DEFAULT_DEVICE_NAME: &str = "New device";

/// What [`ControlState::new`] needs to know about the running `serve`.
pub struct ControlParts {
    /// The coordinator's display name, shown on the device that pairs. `None` follows the
    /// settings, read each time a code is made, so a rename shows without a restart.
    pub display_name: Option<String>,
    /// The folder holding the CA and the allowlists (`None` without TLS: nothing can be
    /// issued).
    pub pki_dir: Option<PathBuf>,
    /// The viewer enrollment listener's registry, if that listener runs.
    pub viewer_registry: Option<Arc<EnrollRegistry>>,
    /// The worker enrollment listener's registry, if that listener runs.
    pub worker_registry: Option<Arc<EnrollRegistry>>,
    /// The allowlist files in force (viewers and workers). Empty with
    /// `--trust-any-client-cert` or without TLS.
    pub allowlists: Vec<PathBuf>,
    /// Whether remote administration is on (`--allow-remote-admin`, with TLS): monitor and
    /// admin codes are then issued through the viewer registry.
    pub remote_admin: bool,
}

impl ControlParts {
    /// The parts for a `serve` started with `args`.
    ///
    /// `viewer_auth` is the viewer transport's [`Auth`] (`None` without TLS) and the registries are
    /// those of the two enrollment listeners.
    #[must_use]
    pub fn from_args(
        args: &ServeArgs,
        viewer_auth: Option<&Auth>,
        viewer_registry: Option<Arc<EnrollRegistry>>,
        worker_registry: Option<Arc<EnrollRegistry>>,
    ) -> Self {
        let mut allowlists: Vec<PathBuf> = Vec::new();
        if let Some(path) = viewer_auth.and_then(|auth| auth.allowlist.clone()) {
            allowlists.push(path);
        }
        if !args.trust_any_client_cert && !args.insecure_no_tls {
            let worker_list = args
                .worker_allowlist
                .clone()
                .or_else(|| args.ca.as_deref().map(pki::default_worker_allowlist_path));
            if let Some(path) = worker_list
                && !allowlists.contains(&path)
            {
                allowlists.push(path);
            }
        }
        Self {
            display_name: None,
            pki_dir: args.ca.as_deref().map(pki::role::pki_dir_of),
            viewer_registry,
            worker_registry,
            allowlists,
            remote_admin: args.allow_remote_admin && !args.insecure_no_tls,
        }
    }
}

/// Drops the saved allowlist read of `inner`.
fn forget_listed(inner: &Inner) {
    *inner.listed.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// The enrollment side (viewer or worker listener) whose registry a code for `role` is made
/// by: monitor and admin codes belong to the viewer registry.
const fn registry_side(role: PeerRole) -> PeerRole {
    match role {
        PeerRole::Monitor | PeerRole::Admin => PeerRole::Viewer,
        PeerRole::Viewer | PeerRole::Worker => role,
    }
}

/// One issued, not yet claimed pairing code, as the control layer tracks it.
struct TokenRecord {
    id: TokenId,
    side: PeerRole,
    enroll_id: EnrollTokenId,
    expires: Instant,
    /// The name the new device will get (shown in the pending list; never the secret).
    name: String,
    /// When the code stops working, in unix seconds.
    expires_unix: UnixSeconds,
}

/// One device on an allowlist, as the status lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedDevice {
    /// The certificate fingerprint.
    pub fingerprint: Fingerprint,
    /// The name without its role prefix (a placeholder with the short fingerprint when the
    /// line has no name).
    pub name: String,
    /// The role the certificate's Common Name gives it.
    pub role: PeerRole,
}

struct Inner {
    parts: ControlParts,
    hub: EventHub,
    table: ConnectionTable,
    tokens: Mutex<Vec<TokenRecord>>,
    next_token: AtomicU64,
    /// The last allowlist read for the status and when it was taken.
    listed: Mutex<Option<(Instant, Vec<ListedDevice>)>>,
}

/// How long the status reuses a read of the allowlist files.
const LISTED_TTL: Duration = Duration::from_secs(2);

/// The state behind the control API. Clones share it.
#[derive(Clone)]
pub struct ControlState {
    inner: Arc<Inner>,
}

impl Inner {
    fn lock_tokens(&self) -> MutexGuard<'_, Vec<TokenRecord>> {
        self.tokens.lock().unwrap_or_else(PoisonError::into_inner)
    }

    const fn registry_for(&self, side: PeerRole) -> Option<&Arc<EnrollRegistry>> {
        match side {
            PeerRole::Viewer => self.parts.viewer_registry.as_ref(),
            PeerRole::Worker => self.parts.worker_registry.as_ref(),
            // Remote administration roles enroll on the viewer listener, only while on.
            PeerRole::Monitor | PeerRole::Admin => {
                if self.parts.remote_admin {
                    self.parts.viewer_registry.as_ref()
                } else {
                    None
                }
            }
        }
    }

    /// The allowlist files to search: the configured ones plus the monitor and admin lists
    /// that exist next to the CA.
    fn allowlist_paths(&self) -> Vec<PathBuf> {
        let mut paths = self.parts.allowlists.clone();
        if let Some(dir) = &self.parts.pki_dir {
            for role in [PeerRole::Monitor, PeerRole::Admin] {
                let path = pki::role::allowlist_in(dir, role);
                if path.exists() && !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
        paths
    }

    /// Forwards the claims waiting in `receivers` and reports codes that ran out before
    /// `now`. Run by the event thread; separate so tests can drive it.
    fn pump_once(&self, receivers: &[(PeerRole, Receiver<ClaimEvent>)], now: Instant) {
        for (side, receiver) in receivers {
            while let Ok(claim) = receiver.try_recv() {
                forget_listed(self);
                self.lock_tokens().retain(|record| {
                    !(registry_side(record.side) == *side && record.enroll_id == claim.id)
                });
                self.hub.publish(&CoordinatorEvent::TokenClaimed {
                    role: claim.role,
                    fingerprint: fingerprint_to_hex(&claim.fingerprint),
                    peer: claim.peer,
                });
            }
        }
        let expired: Vec<TokenId> = {
            let mut tokens = self.lock_tokens();
            let mut gone = Vec::new();
            tokens.retain(|record| {
                if record.expires <= now {
                    gone.push(record.id);
                    false
                } else {
                    true
                }
            });
            gone
        };
        for id in expired {
            self.hub.publish(&CoordinatorEvent::TokenExpired { id });
        }
    }
}

impl ControlState {
    /// Builds the state and starts the event thread (subscribed to both registries).
    #[must_use]
    pub fn new(parts: ControlParts) -> Self {
        let hub = EventHub::new();
        let table = ConnectionTable::new(hub.clone());
        Self::with_table(parts, hub, table)
    }

    /// [`Self::new`] over a hub and a connection table that already exist.
    ///
    /// The listeners started before the state (the worker port) can register their connections in the
    /// same table and publish into the same stream. `table` must publish to `hub`.
    #[must_use]
    pub fn with_table(parts: ControlParts, hub: EventHub, table: ConnectionTable) -> Self {
        let receivers: Vec<(PeerRole, Receiver<ClaimEvent>)> = [
            (PeerRole::Viewer, parts.viewer_registry.as_ref()),
            (PeerRole::Worker, parts.worker_registry.as_ref()),
        ]
        .into_iter()
        .filter_map(|(side, registry)| registry.map(|r| (side, r.subscribe())))
        .collect();
        let inner = Arc::new(Inner {
            parts,
            table,
            hub,
            tokens: Mutex::new(Vec::new()),
            next_token: AtomicU64::new(1),
            listed: Mutex::new(None),
        });
        let weak = Arc::downgrade(&inner);
        let spawned = thread::Builder::new()
            .name("control-events".to_owned())
            .spawn(move || {
                loop {
                    thread::sleep(PUMP_INTERVAL);
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    inner.pump_once(&receivers, Instant::now());
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(
                "pleochroma-worker serve: the control event thread did not start ({error})"
            );
        }
        Self { inner }
    }

    /// The table of live viewer connections (for the accept loop and the status).
    #[must_use]
    pub fn table(&self) -> ConnectionTable {
        self.inner.table.clone()
    }

    /// The folder holding the CA and the allowlists, when there is one (TLS).
    #[must_use]
    pub fn pki_dir(&self) -> Option<PathBuf> {
        self.inner.parts.pki_dir.clone()
    }

    /// Whether remote administration is on for this coordinator.
    #[must_use]
    pub fn remote_admin(&self) -> bool {
        self.inner.parts.remote_admin
    }

    /// The event hub, so other parts of `serve` can publish into the same stream.
    #[must_use]
    pub fn hub(&self) -> EventHub {
        self.inner.hub.clone()
    }

    /// A new receiver of every event from now on.
    #[must_use]
    pub fn events(&self) -> Receiver<CoordinatorEvent> {
        self.inner.hub.subscribe()
    }

    /// The pairing codes that were issued and are neither claimed, cancelled nor expired,
    /// newest last. Never carries a code.
    #[must_use]
    pub fn pending_tokens(&self) -> Vec<PendingToken> {
        let now = Instant::now();
        self.inner
            .lock_tokens()
            .iter()
            .filter(|record| record.expires > now)
            .map(|record| PendingToken {
                id: record.id,
                role: record.side,
                name: record.name.clone(),
                expires: record.expires_unix,
            })
            .collect()
    }

    /// Every device on an allowlist in force (viewers, render computers, monitors and
    /// administrators), each once.
    ///
    /// Reads the files, so a device paired a moment ago shows. Empty without allowlists
    /// (`--trust-any-client-cert`, no TLS).
    #[must_use]
    pub fn listed_devices(&self) -> Vec<ListedDevice> {
        let mut seen: HashSet<Fingerprint> = HashSet::new();
        let mut devices = Vec::new();
        for path in self.inner.allowlist_paths() {
            let Ok(list) = Allowlist::load(&path) else {
                continue;
            };
            for entry in list.entries() {
                if !seen.insert(entry.fingerprint) {
                    continue;
                }
                let display = entry.display_name();
                devices.push(ListedDevice {
                    fingerprint: entry.fingerprint,
                    name: device_name_of_common_name(&display).to_string(),
                    role: role_of_common_name(entry.name.as_deref()),
                });
            }
        }
        devices
    }

    /// [`Self::listed_devices`] for the once-a-second status: the files are read at most every
    /// two seconds, and a claim, a removal or a rename reads them again at once.
    #[must_use]
    pub fn listed_devices_cached(&self) -> Vec<ListedDevice> {
        {
            let saved = self
                .inner
                .listed
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some((taken, devices)) = saved.as_ref()
                && taken.elapsed() < LISTED_TTL
            {
                return devices.clone();
            }
        }
        // The files are read without the lock, so a slow disk never blocks another reader.
        let devices = self.listed_devices();
        *self
            .inner
            .listed
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some((Instant::now(), devices.clone()));
        devices
    }

    /// Drops the saved allowlist read, so the next status reads the files again.
    fn forget_listed(&self) {
        forget_listed(&self.inner);
    }

    /// Device names by fingerprint (64 hex characters): the allowlist names, then the live
    /// connections' names for devices that are on no list.
    #[must_use]
    pub fn device_names(&self) -> HashMap<String, String> {
        let mut names: HashMap<String, String> = self
            .listed_devices_cached()
            .into_iter()
            .map(|device| (fingerprint_to_hex(&device.fingerprint), device.name))
            .collect();
        for live in self.inner.table.list() {
            names
                .entry(fingerprint_to_hex(&live.fingerprint))
                .or_insert(live.name);
        }
        names
    }

    /// Makes a pairing code for a `role` device and the link that carries it.
    ///
    /// `serve` and `enroll` are the bound addresses of the role's connection port and
    /// enrollment port.
    ///
    /// # Errors
    ///
    /// A plain sentence when the role cannot pair here (no TLS, pairing turned off, a monitor
    /// or admin code while remote administration is off), when there is no address to put in
    /// the link, or when the certificate could not be made.
    ///
    /// Monitor and admin codes are made here only for the coordinator's own computer: the
    /// remote session's permission table refuses them (`crate::admin`).
    pub fn issue(
        &self,
        role: PeerRole,
        default_name: &str,
        serve: Option<SocketAddr>,
        enroll: Option<SocketAddr>,
    ) -> Result<IssuedToken, String> {
        let inner = &self.inner;
        if matches!(role, PeerRole::Monitor | PeerRole::Admin) && !inner.parts.remote_admin {
            return Err(
                "Remote administration is switched off. Switch it on in Settings first, then make the code."
                    .to_string(),
            );
        }
        let Some(registry) = inner.registry_for(role) else {
            return Err(
                "Pairing is not available: this coordinator runs without encryption, or with pairing turned off."
                    .to_string(),
            );
        };
        let (Some(serve), Some(enroll)) = (serve, enroll) else {
            return Err(format!(
                "This coordinator has no pairing port for a {}.",
                pki::role::role_name(role)
            ));
        };
        let Some(pki_dir) = inner.parts.pki_dir.as_deref() else {
            return Err("This coordinator has no certificate folder to issue from.".to_string());
        };
        let addrs = link_addresses(serve, &lan_ipv4());
        if addrs.is_empty() {
            return Err(
                "This computer has no network address to put in a pairing link.".to_string(),
            );
        }
        let name =
            clean_device_name(default_name).unwrap_or_else(|| DEFAULT_DEVICE_NAME.to_string());
        let issued = registry
            .issue_for_role(pki_dir, &name, role)
            .map_err(|error| error.to_string())?;
        let coordinator_name = inner
            .parts
            .display_name
            .clone()
            .unwrap_or_else(configured_display_name);
        let built = PairingLink::new(
            role,
            fit_link_name(&coordinator_name),
            addrs,
            enroll.port(),
            issued.token.clone(),
        )
        .build();
        let link = match built {
            Ok(link) => link,
            Err(error) => {
                registry.remove(issued.id);
                return Err(error.to_string());
            }
        };

        let expires_at = SystemTime::now() + Duration::from_secs(issued.expires_in_secs);
        let expires_unix = unix_seconds(expires_at);
        let id = TokenId(inner.next_token.fetch_add(1, Ordering::Relaxed));
        inner.lock_tokens().push(TokenRecord {
            id,
            side: role,
            enroll_id: issued.id,
            expires: Instant::now() + Duration::from_secs(issued.expires_in_secs),
            name: name.clone(),
            expires_unix,
        });
        inner.hub.publish(&CoordinatorEvent::TokenIssued {
            id,
            role,
            name,
            expires: expires_unix,
        });
        Ok(IssuedToken {
            id,
            token: issued.token,
            link,
            expires_at,
        })
    }

    /// Cancels a pairing code that has not been used. Does nothing for a code that was
    /// claimed, expired or already cancelled.
    pub fn cancel(&self, id: TokenId) {
        let inner = &self.inner;
        let record = {
            let mut tokens = inner.lock_tokens();
            tokens
                .iter()
                .position(|record| record.id == id)
                .map(|index| tokens.remove(index))
        };
        let Some(record) = record else {
            return;
        };
        let removed = inner
            .registry_for(record.side)
            .is_some_and(|registry| registry.remove(record.enroll_id));
        if removed {
            inner.hub.publish(&CoordinatorEvent::TokenCancelled { id });
        }
    }

    /// Removes a device's access: deletes its allowlist line (atomically, through
    /// [`Allowlist::remove`]) and shuts down its live connections.
    ///
    /// # Errors
    ///
    /// A plain sentence when there is no allowlist (`--trust-any-client-cert`), when the
    /// device is on none, or when the file could not be written.
    pub fn revoke(&self, fp: &Fingerprint) -> Result<(), String> {
        let inner = &self.inner;
        let paths = inner.allowlist_paths();
        if paths.is_empty() {
            return Err(
                "This coordinator trusts every certificate it signed (no list of trusted devices), so one device cannot be removed."
                    .to_string(),
            );
        }
        let hex = fingerprint_to_hex(fp);
        let mut label: Option<String> = None;
        let mut removed = false;
        for path in &paths {
            let Some(found) = find_entry(path, &hex) else {
                continue;
            };
            match Allowlist::remove(path, fp) {
                Ok(true) => {
                    removed = true;
                    label.get_or_insert(found);
                }
                Ok(false) => {}
                Err(error) => {
                    return Err(format!(
                        "Could not update the list of trusted devices: {error}"
                    ));
                }
            }
        }
        if !removed {
            return Err("That device is not on the list of trusted devices (it may already have been removed).".to_string());
        }
        let live = inner.table.for_fingerprint(fp);
        let name = label
            .as_deref()
            .map(device_name_of_common_name)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| live.first().map(|info| info.name.clone()))
            .unwrap_or_else(|| fingerprint_short(&hex));
        self.forget_listed();
        let closed = inner.table.kill(fp);
        tracing::info!(
            "pleochroma-worker serve: removed access for device {} ({closed} connection(s) closed)",
            fingerprint_short(&hex)
        );
        inner.hub.publish(&CoordinatorEvent::DeviceRevoked {
            fingerprint: hex,
            name,
        });
        Ok(())
    }

    /// Renames a device in its allowlist (through [`Allowlist::rename`]) and in the live
    /// table. The certificate itself keeps its old Common Name.
    ///
    /// # Errors
    ///
    /// A plain sentence when the name is empty or unusable, when the device is on no list,
    /// or when the file could not be written.
    pub fn rename(&self, fp: &Fingerprint, name: &str) -> Result<(), String> {
        let inner = &self.inner;
        let bare = clean_device_name(name).ok_or_else(|| "A device needs a name.".to_string())?;
        let hex = fingerprint_to_hex(fp);
        let mut renamed = false;
        for path in &inner.allowlist_paths() {
            let Some(old) = find_entry(path, &hex) else {
                continue;
            };
            // Workers, monitors and admins keep the prefix their role gives the label.
            let role = role_of_common_name(Some(&old));
            let label = common_name_for(role, &bare).ok_or_else(|| {
                "A device name cannot start with \"worker:\", \"monitor:\" or \"admin:\"."
                    .to_string()
            })?;
            match Allowlist::rename(path, fp, &label) {
                Ok(true) => renamed = true,
                Ok(false) => {}
                Err(error) => {
                    return Err(format!(
                        "Could not update the list of trusted devices: {error}"
                    ));
                }
            }
        }
        if !renamed {
            return Err("That device is not on the list of trusted devices.".to_string());
        }
        self.forget_listed();
        inner.table.rename(fp, &bare);
        inner.hub.publish(&CoordinatorEvent::DeviceRenamed {
            fingerprint: hex,
            name: bare,
        });
        Ok(())
    }
}

impl ServeCore {
    /// Makes a one-time pairing code for a new `role` device.
    ///
    /// The code comes with the `pleochroma://pair` link that carries it (this computer's physical LAN addresses first, then the role's
    /// port and its enrollment port). `default_name` is the name the new device gets.
    ///
    /// A monitor or admin code uses the viewer port and the viewer enrollment port, and works
    /// only while remote administration is on.
    ///
    /// # Errors
    ///
    /// A plain sentence; see [`ControlState::issue`].
    pub fn issue_token(&self, role: PeerRole, default_name: &str) -> Result<IssuedToken, String> {
        let (serve, enroll) = match role {
            PeerRole::Viewer | PeerRole::Monitor | PeerRole::Admin => {
                (Some(self.viewer_addr), self.viewer_enroll_addr)
            }
            PeerRole::Worker => (self.worker_addr, self.worker_enroll_addr),
        };
        self.control.issue(role, default_name, serve, enroll)
    }

    /// Cancels an unused pairing code (no effect on one that was claimed or expired).
    pub fn cancel_token(&self, id: TokenId) {
        self.control.cancel(id);
    }

    /// Removes a device's access and closes its live connections.
    ///
    /// # Errors
    ///
    /// A plain sentence; see [`ControlState::revoke`].
    pub fn revoke(&self, fp: &Fingerprint) -> Result<(), String> {
        self.control.revoke(fp)
    }

    /// Renames a device.
    ///
    /// # Errors
    ///
    /// A plain sentence; see [`ControlState::rename`].
    pub fn rename(&self, fp: &Fingerprint, name: &str) -> Result<(), String> {
        self.control.rename(fp, name)
    }

    /// A new receiver of every [`CoordinatorEvent`] from now on. Each call gets its own;
    /// dropping the receiver unsubscribes it.
    #[must_use]
    pub fn events(&self) -> Receiver<CoordinatorEvent> {
        self.control.events()
    }

    /// The table of live connections (viewers and joined render computers).
    #[must_use]
    pub fn connections(&self) -> ConnectionTable {
        self.control.table()
    }

    /// Applies the settings of `patch` that a running coordinator can take without a restart: the
    /// whole-image knobs and the jobs per device of the coordinator's scheduling.
    ///
    /// Returns whether anything was applied live; the rest (job retention, pairing, the own render
    /// lane, ports) takes effect at the next restart.
    #[must_use]
    pub fn apply_live(&self, patch: &pleochroma_net::admin::status::SettingsPatch) -> bool {
        self.apply_live_checked(patch).unwrap_or(false)
    }

    /// [`Self::apply_live`] that tells a patch it could not apply from one that changed nothing.
    ///
    /// `Ok(true)` when a live knob changed, `Ok(false)` when every live field in `patch` already
    /// had that value (or none was set).
    ///
    /// # Errors
    ///
    /// A plain sentence when `patch` sets a live knob and this coordinator has no job
    /// scheduler to take it (a build without render support).
    pub fn apply_live_checked(
        &self,
        patch: &pleochroma_net::admin::status::SettingsPatch,
    ) -> Result<bool, String> {
        let sets_live = patch.whole_image_seconds.is_some()
            || patch.whole_image_pixels.is_some()
            || patch.jobs_per_device.is_some();
        if !sets_live {
            return Ok(false);
        }
        #[cfg(feature = "worker")]
        {
            let coordinator = self.coordinator.as_ref().ok_or_else(|| {
                "This coordinator is not scheduling render jobs, so there is nothing to change."
                    .to_string()
            })?;
            let mut config = coordinator.job_config();
            let before = config;
            if let Some(seconds) = patch.whole_image_seconds {
                config.whole_image_secs = f64::from(seconds);
            }
            if let Some(pixels) = patch.whole_image_pixels {
                config.whole_image_pixel_samples = pixels;
            }
            if let Some(jobs) = patch.jobs_per_device {
                config.jobs_per_viewer = jobs.max(1);
            }
            if config == before {
                return Ok(false);
            }
            coordinator.set_job_config(config);
            Ok(true)
        }
        #[cfg(not(feature = "worker"))]
        {
            let _ = self;
            Err(
                "This coordinator was built without render support, so there is nothing to change."
                    .to_string(),
            )
        }
    }

    /// Cancels a queued or running job, whoever owns it.
    ///
    /// # Errors
    ///
    /// A plain sentence when this coordinator keeps no jobs, the job is unknown or it has
    /// already ended.
    pub fn cancel_job(&self, id: JobId) -> Result<(), String> {
        self.cancel_job_as(id, LOCAL_ADMIN)
    }

    /// [`Self::cancel_job`] for an action taken by `who` (a remote administrator): the audit
    /// event names `who` instead of this computer.
    ///
    /// # Errors
    ///
    /// As [`Self::cancel_job`].
    pub fn cancel_job_as(&self, id: JobId, who: &str) -> Result<(), String> {
        #[cfg(feature = "worker")]
        {
            let admin = self.job_admin.as_ref().ok_or_else(|| NO_JOBS.to_string())?;
            let (owner, info) = admin.cancel(&id.0)?;
            let names = self.control.device_names();
            let (owner_name, _) = super::jobs::owner_display(&owner, &names);
            let hub = self.control.hub();
            if info.phase.is_terminal() {
                hub.publish(&CoordinatorEvent::JobStateChanged {
                    id,
                    state: JobState::Cancelled,
                    owner: owner_name,
                    label: info.label.clone(),
                });
            }
            hub.publish(&CoordinatorEvent::AdminAction {
                who: who.to_string(),
                action: "cancelled job".to_string(),
                target: info.label,
            });
            Ok(())
        }
        #[cfg(not(feature = "worker"))]
        {
            let _ = (id, who);
            Err(NO_JOBS.to_string())
        }
    }

    /// Deletes a job and its picture, whoever owns it.
    ///
    /// # Errors
    ///
    /// A plain sentence when this coordinator keeps no jobs, the job is unknown or it is
    /// still rendering.
    pub fn delete_job(&self, id: JobId) -> Result<(), String> {
        self.delete_job_as(id, LOCAL_ADMIN)
    }

    /// [`Self::delete_job`] for an action taken by `who` (a remote administrator).
    ///
    /// # Errors
    ///
    /// As [`Self::delete_job`].
    pub fn delete_job_as(&self, id: JobId, who: &str) -> Result<(), String> {
        #[cfg(feature = "worker")]
        {
            let admin = self.job_admin.as_ref().ok_or_else(|| NO_JOBS.to_string())?;
            let label = admin.delete(&id.0)?;
            let hub = self.control.hub();
            hub.publish(&CoordinatorEvent::JobDeleted { id });
            hub.publish(&CoordinatorEvent::AdminAction {
                who: who.to_string(),
                action: "deleted job".to_string(),
                target: label,
            });
            Ok(())
        }
        #[cfg(not(feature = "worker"))]
        {
            let _ = (id, who);
            Err(NO_JOBS.to_string())
        }
    }

    /// Disconnects a joined render computer (every connection it has open). It may rejoin.
    ///
    /// # Errors
    ///
    /// A plain sentence when there is no worker port or the computer is not joined.
    pub fn disconnect_worker(&self, id: WorkerId) -> Result<(), String> {
        self.disconnect_worker_as(id, LOCAL_ADMIN)
    }

    /// [`Self::disconnect_worker`] for an action taken by `who` (a remote administrator).
    ///
    /// # Errors
    ///
    /// As [`Self::disconnect_worker`].
    pub fn disconnect_worker_as(&self, id: WorkerId, who: &str) -> Result<(), String> {
        #[cfg(feature = "worker")]
        {
            let registry = self
                .registry
                .as_ref()
                .ok_or_else(|| "This coordinator has no port for render computers.".to_string())?;
            let joined: Vec<_> = registry
                .workers()
                .into_iter()
                .map(|(info, _idle)| info)
                .collect();
            let target = joined
                .iter()
                .find(|info| info.worker_id == id.0)
                .cloned()
                .ok_or_else(|| "That render computer is no longer connected.".to_string())?;
            let table = self.control.table();
            for member in joined.iter().filter(|info| {
                info.worker_id == target.worker_id
                    || (target.label.is_some() && info.label == target.label)
            }) {
                registry.remove(member.worker_id, "disconnected from the coordinator window");
                // A connection that a job holds right now is not in the registry: close its
                // socket through the table so the job notices at once.
                if let Some(peer) = member.peer {
                    for live in table.list() {
                        if live.role == PeerRole::Worker && live.peer == Some(peer) {
                            let _ = table.kill_connection(live.id);
                        }
                    }
                }
            }
            // The registry reports the computer as left once its last connection is removed.
            let name = super::status::worker_display_name(&target);
            let hub = self.control.hub();
            hub.publish(&CoordinatorEvent::AdminAction {
                who: who.to_string(),
                action: "disconnected render computer".to_string(),
                target: name,
            });
            Ok(())
        }
        #[cfg(not(feature = "worker"))]
        {
            let _ = (id, who);
            Err("This build has no port for render computers.".to_string())
        }
    }
}

/// Who the audit events name for an action taken in the coordinator's own window.
const LOCAL_ADMIN: &str = "This computer";

/// What the job calls answer when this coordinator keeps no jobs.
const NO_JOBS: &str =
    "This coordinator keeps no jobs. Jobs need \"Render on this computer\" to be on.";

/// The addresses a pairing link offers for the connection port `serve`.
///
/// A port bound to one IPv4 address offers just that address. A port bound to every address
/// offers this computer's physical LAN addresses, default route first (at most
/// [`MAX_ADDRS`]), or the virtual ones when there is no physical one. A specific IPv6 bind
/// offers nothing (links are IPv4 only).
#[must_use]
pub fn link_addresses(serve: SocketAddr, lan: &[LanAddress]) -> Vec<SocketAddr> {
    match serve.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() => {
            vec![SocketAddr::new(IpAddr::V4(ip), serve.port())]
        }
        IpAddr::V6(ip) if !ip.is_unspecified() => Vec::new(),
        _ => {
            let physical: Vec<&LanAddress> = lan.iter().filter(|a| !a.virtual_adapter).collect();
            let chosen: Vec<&LanAddress> = if physical.is_empty() {
                lan.iter().collect()
            } else {
                physical
            };
            chosen
                .into_iter()
                .take(MAX_ADDRS)
                .map(|a| SocketAddr::new(IpAddr::V4(a.addr), serve.port()))
                .collect()
        }
    }
}

/// A device name made safe for a certificate and an allowlist line: no control characters
/// or `#`, trimmed, at most [`MAX_NAME_BYTES`] bytes. `None` when nothing is left.
fn clean_device_name(name: &str) -> Option<String> {
    let scrubbed: String = sanitize_allowlist_label(name)
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let cut = truncate_bytes(scrubbed.trim(), MAX_NAME_BYTES);
    (!cut.is_empty()).then(|| cut.to_string())
}

/// The coordinator name as a link accepts it: no control characters, at most
/// [`MAX_NAME_BYTES`] bytes, never empty.
fn fit_link_name(name: &str) -> String {
    let scrubbed: String = name.chars().filter(|c| !c.is_control()).collect();
    let cut = truncate_bytes(scrubbed.trim(), MAX_NAME_BYTES);
    if cut.is_empty() {
        "Pleochroma".to_string()
    } else {
        cut.to_string()
    }
}

/// `text` cut to at most `max` bytes on a character boundary.
fn truncate_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].trim_end()
}

/// The label of the line for `hex` in the allowlist at `path` (empty when the line has no
/// comment); `None` when the file is missing, unreadable or has no such line.
fn find_entry(path: &Path, hex: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().find_map(|line| {
        let (entry, comment) = line.split_once('#').unwrap_or((line, ""));
        entry
            .trim()
            .eq_ignore_ascii_case(hex)
            .then(|| comment.trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ClaimEvent, ControlParts, ControlState, EnrollRegistry, EnrollTokenId, PUMP_INTERVAL,
        TokenRecord, clean_device_name, find_entry, fit_link_name, link_addresses,
    };
    use crate::pki;
    use pleochroma_net::{
        admin::status::{CoordinatorEvent, TokenId},
        messages::PeerRole,
        netinfo::LanAddress,
        pairing::PairingLink,
        tls::{Allowlist, append_to_allowlist, fingerprint_to_hex},
    };
    use std::{
        io::Read,
        net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
        path::PathBuf,
        sync::{Arc, mpsc},
        thread,
        time::{Duration, Instant},
    };

    fn unique_temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-worker-control-test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn state(dir: Option<PathBuf>, allowlists: Vec<PathBuf>, with_workers: bool) -> ControlState {
        ControlState::new(ControlParts {
            display_name: Some("Studio".to_string()),
            pki_dir: dir,
            viewer_registry: Some(Arc::new(EnrollRegistry::new())),
            worker_registry: with_workers
                .then(|| Arc::new(EnrollRegistry::for_role(PeerRole::Worker))),
            allowlists,
            remote_admin: false,
        })
    }

    /// A state with remote administration on: the viewer registry has the monitor and admin
    /// allowlists of `dir` registered (what `serve` does under `--allow-remote-admin`).
    fn admin_state(dir: &std::path::Path) -> (ControlState, Arc<EnrollRegistry>) {
        let registry = Arc::new(EnrollRegistry::new());
        registry.register_allowlist(PeerRole::Monitor, pki::monitor_allowlist_in(dir));
        registry.register_allowlist(PeerRole::Admin, pki::admin_allowlist_in(dir));
        let control = ControlState::new(ControlParts {
            display_name: Some("Studio".to_string()),
            pki_dir: Some(dir.to_path_buf()),
            viewer_registry: Some(Arc::clone(&registry)),
            worker_registry: None,
            allowlists: Vec::new(),
            remote_admin: true,
        });
        (control, registry)
    }

    fn lan(addr: [u8; 4], default_route: bool, virtual_adapter: bool) -> LanAddress {
        LanAddress {
            addr: Ipv4Addr::from(addr),
            interface: "x".to_string(),
            default_route,
            virtual_adapter,
        }
    }

    #[test]
    fn issue_then_cancel_reports_both_and_a_second_cancel_is_silent() {
        let dir = unique_temp_dir("issue");
        pki::init(&dir).unwrap();
        let control = state(Some(dir.clone()), Vec::new(), false);
        let events = control.events();
        let serve: SocketAddr = "127.0.0.1:7878".parse().unwrap();
        let enroll: SocketAddr = "127.0.0.1:7879".parse().unwrap();

        let issued = control
            .issue(PeerRole::Viewer, "Attic", Some(serve), Some(enroll))
            .unwrap();
        assert!(issued.expires_at > std::time::SystemTime::now());
        let debug = format!("{issued:?}");
        assert!(!debug.contains(&issued.token), "{debug}");
        assert!(!debug.contains(&issued.link), "{debug}");

        let link = PairingLink::parse(&issued.link).unwrap();
        assert_eq!(link.role, PeerRole::Viewer);
        assert_eq!(link.addrs, vec![serve]);
        assert_eq!(link.enroll_port, 7879);
        assert_eq!(link.token(), issued.token);
        assert_eq!(link.name, "Studio");

        assert!(matches!(
            events.try_recv().unwrap(),
            CoordinatorEvent::TokenIssued { id, role: PeerRole::Viewer, name, .. }
                if id == issued.id && name == "Attic"
        ));
        control.cancel(issued.id);
        assert_eq!(
            events.try_recv().unwrap(),
            CoordinatorEvent::TokenCancelled { id: issued.id }
        );
        control.cancel(issued.id);
        assert!(events.try_recv().is_err(), "a second cancel does nothing");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn issue_refuses_what_cannot_pair() {
        let dir = unique_temp_dir("refuse");
        pki::init(&dir).unwrap();
        let control = state(Some(dir.clone()), Vec::new(), false);
        let serve: SocketAddr = "127.0.0.1:7880".parse().unwrap();
        let enroll: SocketAddr = "127.0.0.1:7881".parse().unwrap();
        // No worker registry, and the monitor and admin roles need remote administration.
        for role in [PeerRole::Worker, PeerRole::Monitor, PeerRole::Admin] {
            assert!(
                control.issue(role, "x", Some(serve), Some(enroll)).is_err(),
                "{role:?}"
            );
        }
        let off = control
            .issue(PeerRole::Admin, "x", Some(serve), Some(enroll))
            .unwrap_err();
        assert!(off.contains("switched off"), "{off}");
        // No enrollment port.
        assert!(
            control
                .issue(PeerRole::Viewer, "x", Some(serve), None)
                .is_err()
        );
        // No certificate folder.
        let bare = state(None, Vec::new(), false);
        assert!(
            bare.issue(PeerRole::Viewer, "x", Some(serve), Some(enroll))
                .is_err()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_worker_code_uses_the_worker_ports() {
        let dir = unique_temp_dir("worker");
        pki::init(&dir).unwrap();
        let control = state(Some(dir.clone()), Vec::new(), true);
        let serve: SocketAddr = "127.0.0.1:7880".parse().unwrap();
        let enroll: SocketAddr = "127.0.0.1:7881".parse().unwrap();
        let issued = control
            .issue(PeerRole::Worker, "Garage", Some(serve), Some(enroll))
            .unwrap();
        let link = PairingLink::parse(&issued.link).unwrap();
        assert_eq!(link.role, PeerRole::Worker);
        assert_eq!(link.addrs, vec![serve]);
        assert_eq!(link.enroll_port, 7881);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn monitor_and_admin_codes_use_the_viewer_ports_while_remote_administration_is_on() {
        let dir = unique_temp_dir("admin-codes");
        pki::init(&dir).unwrap();
        let (control, _registry) = admin_state(&dir);
        let events = control.events();
        let serve: SocketAddr = "127.0.0.1:7878".parse().unwrap();
        let enroll: SocketAddr = "127.0.0.1:7879".parse().unwrap();

        for role in [PeerRole::Monitor, PeerRole::Admin] {
            let issued = control
                .issue(role, "Wall", Some(serve), Some(enroll))
                .unwrap();
            let link = PairingLink::parse(&issued.link).unwrap();
            assert_eq!(link.role, role);
            assert_eq!(link.addrs, vec![serve]);
            assert_eq!(link.enroll_port, 7879);
            let debug = format!("{issued:?}");
            assert!(!debug.contains(&issued.token), "{debug}");

            let pending = control.pending_tokens();
            assert_eq!(pending.len(), 1);
            assert_eq!((pending[0].role, pending[0].name.as_str()), (role, "Wall"));
            assert!(matches!(
                events.try_recv().unwrap(),
                CoordinatorEvent::TokenIssued { id, role: issued_role, .. }
                    if id == issued.id && issued_role == role
            ));

            control.cancel(issued.id);
            assert_eq!(
                events.try_recv().unwrap(),
                CoordinatorEvent::TokenCancelled { id: issued.id }
            );
            assert!(control.pending_tokens().is_empty());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_claimed_monitor_code_leaves_the_book_and_is_reported() {
        let control = state(None, Vec::new(), false);
        let events = control.events();
        let (tx, rx) = mpsc::channel::<ClaimEvent>();
        let receivers = vec![(PeerRole::Viewer, rx)];
        let peer: SocketAddr = "192.168.178.42:5000".parse().unwrap();
        control.inner.lock_tokens().push(TokenRecord {
            id: TokenId(5),
            side: PeerRole::Monitor,
            enroll_id: EnrollTokenId(21),
            expires: Instant::now() + Duration::from_secs(60),
            name: "Wall".to_string(),
            expires_unix: 1_700_000_060,
        });
        tx.send(ClaimEvent {
            id: EnrollTokenId(21),
            role: PeerRole::Monitor,
            name: "monitor:Wall".to_string(),
            fingerprint: [8; 32],
            peer,
        })
        .unwrap();
        control
            .inner
            .pump_once(&receivers, Instant::now() + Duration::from_millis(1));
        assert_eq!(
            events.try_recv().unwrap(),
            CoordinatorEvent::TokenClaimed {
                role: PeerRole::Monitor,
                fingerprint: fingerprint_to_hex(&[8; 32]),
                peer
            }
        );
        assert!(control.inner.lock_tokens().is_empty());
    }

    #[test]
    fn link_addresses_follow_the_bind_address() {
        let list = [
            lan([192, 168, 178, 20], true, false),
            lan([172, 17, 0, 1], false, true),
            lan([10, 0, 0, 5], false, false),
        ];
        let any: SocketAddr = "0.0.0.0:7878".parse().unwrap();
        assert_eq!(
            link_addresses(any, &list),
            vec![
                "192.168.178.20:7878".parse::<SocketAddr>().unwrap(),
                "10.0.0.5:7878".parse().unwrap()
            ],
            "physical only, default route first"
        );
        let only_virtual = [lan([172, 17, 0, 1], false, true)];
        assert_eq!(link_addresses(any, &only_virtual).len(), 1);
        assert!(link_addresses(any, &[]).is_empty());

        let many: Vec<LanAddress> = (1..=5).map(|n| lan([10, 0, 0, n], false, false)).collect();
        assert_eq!(link_addresses(any, &many).len(), 3);

        let specific: SocketAddr = "192.168.1.9:7878".parse().unwrap();
        assert_eq!(link_addresses(specific, &list), vec![specific]);
        let v6: SocketAddr = "[::1]:7878".parse().unwrap();
        assert!(link_addresses(v6, &list).is_empty());
        let v6_any: SocketAddr = "[::]:7878".parse().unwrap();
        assert_eq!(link_addresses(v6_any, &list).len(), 2);
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(
            clean_device_name("  Attic\r\n# x  ").as_deref(),
            Some("Attic___ x"),
            "line breaks and # become underscores"
        );
        assert_eq!(clean_device_name("At\u{7}tic").as_deref(), Some("Attic"));
        assert_eq!(clean_device_name(" \t "), None);
        assert_eq!(clean_device_name(&"é".repeat(100)).unwrap().len(), 64);
        assert_eq!(fit_link_name("   "), "Pleochroma");
        assert_eq!(fit_link_name("Studio\n"), "Studio");
    }

    #[test]
    fn revoke_removes_the_line_closes_the_connection_and_reports_it() {
        let dir = unique_temp_dir("revoke");
        let list = dir.join("allowlist-viewers.txt");
        append_to_allowlist(&list, &[3; 32], "laptop").unwrap();
        append_to_allowlist(&list, &[4; 32], "phone").unwrap();
        let control = state(Some(dir.clone()), vec![list.clone()], false);
        let events = control.events();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        let tracked = control.table().register(
            "laptop".to_string(),
            [3; 32],
            PeerRole::Viewer,
            None,
            server.try_clone().ok(),
        );
        let reader = thread::spawn(move || {
            let mut byte = [0_u8; 1];
            server.read(&mut byte)
        });
        thread::sleep(Duration::from_millis(100));

        control.revoke(&[3; 32]).unwrap();
        let outcome = reader.join().unwrap();
        assert!(matches!(outcome, Ok(0) | Err(_)), "{outcome:?}");
        drop(tracked);

        let after = Allowlist::load(&list).unwrap();
        assert!(!after.contains(&[3; 32]));
        assert!(after.contains(&[4; 32]), "the other device keeps access");
        assert!(find_entry(&list, &fingerprint_to_hex(&[3; 32])).is_none());

        let mut revoked = None;
        while let Ok(event) = events.try_recv() {
            if let CoordinatorEvent::DeviceRevoked { fingerprint, name } = event {
                revoked = Some((fingerprint, name));
            }
        }
        assert_eq!(
            revoked,
            Some((fingerprint_to_hex(&[3; 32]), "laptop".to_string()))
        );
        assert!(control.revoke(&[3; 32]).is_err(), "already removed");
        assert!(state(None, Vec::new(), false).revoke(&[3; 32]).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rename_changes_the_label_keeps_the_role_prefix_and_reports_it() {
        let dir = unique_temp_dir("rename");
        let list = dir.join("allowlist-workers.txt");
        append_to_allowlist(&list, &[5; 32], "worker:attic").unwrap();
        append_to_allowlist(&list, &[6; 32], "laptop").unwrap();
        let control = state(Some(dir.clone()), vec![list.clone()], false);
        let events = control.events();

        control.rename(&[5; 32], "Garage").unwrap();
        assert_eq!(
            find_entry(&list, &fingerprint_to_hex(&[5; 32])).as_deref(),
            Some("worker:Garage")
        );
        assert_eq!(
            find_entry(&list, &fingerprint_to_hex(&[6; 32])).as_deref(),
            Some("laptop"),
            "other lines are untouched"
        );
        assert_eq!(
            events.try_recv().unwrap(),
            CoordinatorEvent::DeviceRenamed {
                fingerprint: fingerprint_to_hex(&[5; 32]),
                name: "Garage".to_string()
            }
        );
        assert!(control.rename(&[6; 32], "admin:sneaky").is_err());
        assert!(control.rename(&[6; 32], "  ").is_err());
        assert!(control.rename(&[9; 32], "Nobody").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn claims_are_forwarded_and_expired_codes_reported() {
        let control = state(None, Vec::new(), false);
        let events = control.events();
        let (tx, rx) = mpsc::channel::<ClaimEvent>();
        let receivers = vec![(PeerRole::Viewer, rx)];
        let peer: SocketAddr = "192.168.178.42:5000".parse().unwrap();
        {
            let mut tokens = control.inner.lock_tokens();
            tokens.push(TokenRecord {
                id: TokenId(1),
                side: PeerRole::Viewer,
                enroll_id: EnrollTokenId(11),
                expires: Instant::now() + Duration::from_secs(60),
                name: "S23".to_string(),
                expires_unix: 1_700_000_060,
            });
            tokens.push(TokenRecord {
                id: TokenId(2),
                side: PeerRole::Viewer,
                enroll_id: EnrollTokenId(12),
                expires: Instant::now(),
                name: "Tablet".to_string(),
                expires_unix: 1_700_000_000,
            });
        }
        tx.send(ClaimEvent {
            id: EnrollTokenId(11),
            role: PeerRole::Viewer,
            name: "S23".to_string(),
            fingerprint: [7; 32],
            peer,
        })
        .unwrap();

        control
            .inner
            .pump_once(&receivers, Instant::now() + Duration::from_millis(1));
        assert_eq!(
            events.try_recv().unwrap(),
            CoordinatorEvent::TokenClaimed {
                role: PeerRole::Viewer,
                fingerprint: fingerprint_to_hex(&[7; 32]),
                peer
            }
        );
        assert_eq!(
            events.try_recv().unwrap(),
            CoordinatorEvent::TokenExpired { id: TokenId(2) }
        );
        assert!(events.try_recv().is_err());
        assert!(
            control.inner.lock_tokens().is_empty(),
            "a claimed and an expired code both leave the book"
        );
        assert!(PUMP_INTERVAL > Duration::ZERO);
    }

    #[test]
    fn pending_tokens_list_unused_codes_without_their_secrets() {
        let dir = unique_temp_dir("pending");
        pki::init(&dir).unwrap();
        let control = state(Some(dir.clone()), Vec::new(), false);
        let serve: SocketAddr = "127.0.0.1:7890".parse().unwrap();
        let enroll: SocketAddr = "127.0.0.1:7891".parse().unwrap();
        assert!(control.pending_tokens().is_empty());

        let issued = control
            .issue(PeerRole::Viewer, "Attic", Some(serve), Some(enroll))
            .unwrap();
        let pending = control.pending_tokens();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, issued.id);
        assert_eq!(pending[0].role, PeerRole::Viewer);
        assert_eq!(pending[0].name, "Attic");
        assert!(pending[0].expires > crate::control::status::unix_now());
        let text = format!("{pending:?}");
        assert!(!text.contains(&issued.token), "{text}");

        control.cancel(issued.id);
        assert!(control.pending_tokens().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn listed_devices_read_every_list_once_with_the_role_of_their_name() {
        let dir = unique_temp_dir("listed");
        let viewers = dir.join("allowlist-viewers.txt");
        let workers = dir.join("allowlist-workers.txt");
        append_to_allowlist(&viewers, &[3; 32], "laptop").unwrap();
        append_to_allowlist(&workers, &[5; 32], "worker:attic").unwrap();
        append_to_allowlist(&workers, &[3; 32], "laptop").unwrap();
        let control = state(Some(dir.clone()), vec![viewers, workers], false);

        let listed = control.listed_devices();
        assert_eq!(listed.len(), 2, "a device on two lists appears once");
        let laptop = listed.iter().find(|d| d.fingerprint == [3; 32]).unwrap();
        assert_eq!(
            (laptop.name.as_str(), laptop.role),
            ("laptop", PeerRole::Viewer)
        );
        let attic = listed.iter().find(|d| d.fingerprint == [5; 32]).unwrap();
        assert_eq!(
            (attic.name.as_str(), attic.role),
            ("attic", PeerRole::Worker)
        );

        let _guest =
            control
                .table()
                .register("guest".to_string(), [7; 32], PeerRole::Viewer, None, None);
        let names = control.device_names();
        assert_eq!(names[&fingerprint_to_hex(&[5; 32])], "attic");
        assert_eq!(names[&fingerprint_to_hex(&[7; 32])], "guest");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_events_call_gets_its_own_receiver() {
        let control = state(None, Vec::new(), false);
        let first = control.events();
        let second = control.events();
        control
            .hub()
            .publish(&CoordinatorEvent::TokenExpired { id: TokenId(3) });
        assert!(first.try_recv().is_ok());
        assert!(second.try_recv().is_ok());
        drop(first);
        control
            .hub()
            .publish(&CoordinatorEvent::TokenExpired { id: TokenId(4) });
        assert!(second.try_recv().is_ok());
        assert_eq!(control.hub().subscriber_count(), 1);
    }
}
