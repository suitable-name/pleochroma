//! [`RemoteEndpoint`]: the ONE remote this viewer talks to (a coordinator, rendering
//! itself when started with `--render`), plus the two "final picture only" transfer
//! preferences ([`ExportTransfer`], [`LiveTransfer`]) and the one-time migration from
//! the old multi-worker list ([`migrate_legacy_workers`]).

use super::worker::WorkerSettings;
use serde::{Deserialize, Serialize};

/// How a still export or a tilt video gets its result from the remote.
///
/// `FullData` (the default): the remote streams float radiance, which the viewer merges with
/// its local lanes and tone-maps. `FinalPicture` asks the remote for the whole job as one
/// finished 8-bit PNG (`FinalImageRequest`): about a tenth of the bytes over the link, but the
/// local CPU/GPU take no part. A server that doesn't support it refuses with
/// `UNSUPPORTED_REQUEST` and the viewer falls back to `FullData` with a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExportTransfer {
    /// Float radiance, merged with the local lanes (the default).
    #[default]
    FullData,
    /// One finished PNG per image (per frame for a video); local lanes stay idle.
    FinalPicture,
}

impl ExportTransfer {
    /// Index of the "Transfer" pill (0 = Full data, 1 = Final picture only).
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::FullData => 0,
            Self::FinalPicture => 1,
        }
    }

    /// Inverse of [`Self::index`]; anything but `1` is `FullData`.
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        if index == 1 {
            Self::FinalPicture
        } else {
            Self::FullData
        }
    }
}

/// Which computers render a tilt video: the "Compute" pill of the video section, remembered
/// across restarts. Mirrors `export_thread::ComputeTarget` without the settings layer
/// depending on the bridge; the pill order is the still-image export's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TiltVideoCompute {
    /// This computer only.
    Local,
    /// The remote only: nothing is traced on this computer.
    Remote,
    /// This computer and the remote together (the default).
    #[default]
    Both,
}

impl TiltVideoCompute {
    /// Index of the "Compute" pill (0 = Local only, 1 = Remote only, 2 = Local + Remote).
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::Local => 0,
            Self::Remote => 1,
            Self::Both => 2,
        }
    }

    /// Inverse of [`Self::index`]; anything unknown is `Both`.
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        match index {
            0 => Self::Local,
            1 => Self::Remote,
            _ => Self::Both,
        }
    }
}

/// How the live view gets remote contributions once the camera settles.
///
/// `FullData` (the default): float deltas, combined with local samples under
/// `LiveComputeTarget::Both`. `FinalPicture` asks for finished, denoised
/// 8-bit frames (`TransferMode::DisplayOnly`): less bandwidth, but 8-bit pictures cannot
/// be merged with local samples, so the settled image is remote-only and local tracing
/// pauses after the handoff exactly like `LiveComputeTarget::RemoteOnly`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LiveTransfer {
    /// Float deltas, combinable with local samples (the default).
    #[default]
    FullData,
    /// Finished, denoised display frames; the settled image is remote-only.
    FinalPicture,
}

impl LiveTransfer {
    /// Index of the Parameters panel's "Live transfer" pill (0 = Full data, 1 = Final
    /// picture).
    #[cfg(test)]
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::FullData => 0,
            Self::FinalPicture => 1,
        }
    }

    /// Inverse of [`Self::index`]; anything but `1` is `FullData`.
    #[cfg(test)]
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        if index == 1 {
            Self::FinalPicture
        } else {
            Self::FullData
        }
    }
}

/// The one configured remote: connection details (the same [`WorkerSettings`] shape the
/// connection code has always taken) plus the "final picture only" transfer preferences.
///
/// Stored as `[settings.remote]` with the connection in `[settings.remote.connection]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RemoteEndpoint {
    /// Name, address, certificate folder and live-stream preferences.
    pub connection: WorkerSettings,
    /// The default "Transfer" choice of the export dialog and the tilt-video section.
    pub export_transfer: ExportTransfer,
    /// The live view's transfer choice (Parameters panel, next to "Live Compute").
    pub live_transfer: LiveTransfer,
    /// Rendering addresses (`host:port`) tried in order, written by a pairing link and by
    /// rediscovery. Empty for an endpoint saved by 0.11.1 or entered by hand: read it through
    /// [`Self::endpoint_addresses`], which then falls back to `connection.address`. When it is
    /// not empty, `connection.address` repeats its first entry.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<String>,
    /// The coordinator's enrollment port from the pairing link (`0`: not paired through a
    /// link).
    #[serde(skip_serializing_if = "is_zero_port")]
    pub enroll_port: u16,
    /// The endpoint was paired through a link, so the coordinator is verified by its fixed
    /// certificate name whatever address answers. Off for an endpoint entered by hand, which
    /// keeps verifying the dialed name.
    #[serde(skip_serializing_if = "is_false")]
    pub fixed_name: bool,
}

/// `skip_serializing_if` helper: a zero port is not written.
#[allow(clippy::trivially_copy_pass_by_ref)] // serde passes a reference
const fn is_zero_port(port: &u16) -> bool {
    *port == 0
}

/// `skip_serializing_if` helper: a false flag is not written.
#[allow(clippy::trivially_copy_pass_by_ref)] // serde passes a reference
const fn is_false(flag: &bool) -> bool {
    !*flag
}

impl RemoteEndpoint {
    /// An endpoint for `connection` with both transfer preferences at their defaults.
    #[must_use]
    pub fn new(connection: WorkerSettings) -> Self {
        Self {
            connection,
            ..Self::default()
        }
    }

    /// The addresses to try, in order.
    ///
    /// A 0.11.1 endpoint stored one `host:port`; it loads as a one-element list here, without
    /// the file being rewritten. Blank entries are skipped. Empty when no address is saved.
    #[must_use]
    pub fn endpoint_addresses(&self) -> Vec<String> {
        let listed: Vec<String> = self
            .addresses
            .iter()
            .map(String::as_str)
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_owned)
            .collect();
        if !listed.is_empty() {
            return listed;
        }
        let single = self.connection.address.trim();
        if single.is_empty() {
            Vec::new()
        } else {
            vec![single.to_owned()]
        }
    }

    /// The endpoint after a pairing link was redeemed: `name`, `addresses` (the answering one
    /// first), the enrollment port and the certificate folder replace the old connection, the
    /// endpoint is marked fixed-name, and the stream and transfer preferences stay.
    #[must_use]
    pub fn paired(
        &self,
        name: &str,
        addresses: Vec<String>,
        enroll_port: u16,
        cert_dir: &str,
    ) -> Self {
        let mut next = self.clone();
        name.clone_into(&mut next.connection.name);
        next.connection.address = addresses.first().cloned().unwrap_or_default();
        cert_dir.clone_into(&mut next.connection.cert_dir);
        next.addresses = addresses;
        next.enroll_port = enroll_port;
        next.fixed_name = true;
        next
    }

    /// The endpoint after rediscovery found the coordinator at `addresses`: the list and the
    /// primary address are replaced, everything else (certificates, name, preferences) stays.
    #[must_use]
    pub fn moved_to(&self, addresses: Vec<String>) -> Self {
        let mut next = self.clone();
        next.connection.address = addresses.first().cloned().unwrap_or_default();
        next.addresses = addresses;
        next
    }

    /// The name shown for this endpoint: its name, else its first address, else "".
    #[must_use]
    pub fn display_name(&self) -> String {
        let name = self.connection.name.trim();
        if name.is_empty() {
            self.endpoint_addresses()
                .into_iter()
                .next()
                .unwrap_or_default()
        } else {
            name.to_owned()
        }
    }
}

/// What [`migrate_legacy_workers`] did with an old `remote_workers` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyWorkerMigration {
    /// The address of the entry that became the remote endpoint (`None` when an endpoint
    /// was already configured and every legacy entry was dropped).
    pub kept: Option<String>,
    /// Addresses of the entries that were dropped (every entry after the first).
    pub dropped: Vec<String>,
}

/// Folds an old multi-worker list into the single-endpoint model.
///
/// The FIRST entry becomes
/// `remote` (unless one is already configured) and every other entry is dropped, logged by
/// address. `None` when there was nothing to migrate. Only the first entry was ever used for
/// rendering, so it is the one kept.
pub fn migrate_legacy_workers(
    legacy: &mut Vec<WorkerSettings>,
    remote: &mut Option<RemoteEndpoint>,
) -> Option<LegacyWorkerMigration> {
    if legacy.is_empty() {
        return None;
    }
    let mut entries = std::mem::take(legacy).into_iter();
    let first = entries.next()?;
    let mut dropped: Vec<String> = entries.map(|w| w.address).collect();
    let kept = if remote.is_some() {
        dropped.insert(0, first.address);
        None
    } else {
        let address = first.address.clone();
        *remote = Some(RemoteEndpoint::new(first));
        Some(address)
    };
    if let Some(address) = &kept {
        tracing::info!(
            "settings: migrated the remote-worker list to a single remote endpoint ({address})"
        );
    }
    if !dropped.is_empty() {
        tracing::warn!(
            "settings: only one remote endpoint is supported now; dropped {} older \
             worker entr{}: {}",
            dropped.len(),
            if dropped.len() == 1 { "y" } else { "ies" },
            dropped.join(", ")
        );
    }
    Some(LegacyWorkerMigration { kept, dropped })
}
