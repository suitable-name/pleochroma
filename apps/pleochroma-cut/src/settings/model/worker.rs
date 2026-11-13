//! Remote-worker settings: [`PreviewScale`] (a worker's live preview-stream
//! resolution), [`LocalPreviewScale`] (the local preview-then-settle resolution
//! reduction), and [`WorkerSettings`] (one configured remote render worker).

use pleochroma_net::messages::{PreviewConfig, StreamConfig, TransferMode};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The resolution a remote worker's `PREVIEW` stream is rendered at, relative to the session's full
/// render resolution.
///
/// Unlike [`WorkerSettings::transfer_mode`], which governs the full-resolution `FRAME` payload,
/// this only sizes the preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PreviewScale {
    Full,
    Half,
    /// The default. A quarter-resolution preview has 1/16th of the pixels, which keeps
    /// the stream cheap and is still enough detail for a camera-drag preview. A fast
    /// link can pick `Full`.
    #[default]
    Quarter,
    /// A custom percentage of the session's full resolution, `1..=100`. Values outside
    /// that range are clamped by [`PreviewScale::percent`] rather than rejected, so a
    /// hand-edited or migrated settings file stays loadable.
    Custom(u32),
}

impl PreviewScale {
    /// This scale as an integer percentage of the full render resolution, clamped to
    /// `1..=100` (a `0%` preview would be a zero-area request the worker would reject).
    #[must_use]
    pub const fn percent(self) -> u32 {
        match self {
            Self::Full => 100,
            Self::Half => 50,
            Self::Quarter => 25,
            Self::Custom(p) => {
                if p == 0 {
                    1
                } else if p > 100 {
                    100
                } else {
                    p
                }
            }
        }
    }

    /// Resolves this scale against a session-wide `width x height` render resolution,
    /// floored at `1x1` so a worker is never asked for a zero-area preview regardless
    /// of how small `width`/`height` are.
    #[must_use]
    pub fn resolve(self, width: u32, height: u32) -> (u32, u32) {
        let pct = self.percent();
        let w = (width * pct / 100).max(1);
        let h = (height * pct / 100).max(1);
        (w, h)
    }
}

/// The resolution reduction the local render loop applies while the camera moves (see
/// `bridge::render_thread::local_preview::effective_dimensions`).
///
/// Unlike [`PreviewScale`], there is no custom percentage: the settings dialog offers a
/// fixed pill choice, not a slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LocalPreviewScale {
    /// No reduction: traces at the full configured resolution whatever the camera does.
    /// The default.
    #[default]
    Off,
    Half,
    Quarter,
}

impl LocalPreviewScale {
    /// The integer divisor applied to each configured dimension while the camera is
    /// moving. `Off` divides by `1` -- a caller that forgets to special-case `Off`
    /// still gets the correct answer.
    #[must_use]
    pub const fn divisor(self) -> u32 {
        match self {
            Self::Off => 1,
            Self::Half => 2,
            Self::Quarter => 4,
        }
    }
}

/// Which engine(s) contribute to the live viewport once the camera settles.
///
/// Mirrors `bridge::export_thread::ComputeTarget`, but is a separate type because the
/// live and export callers live in disjoint module trees. The export dialog's choice is
/// not an `AppSettings` field; this one is a persisted `AppSettings` field. `Both` is the default;
/// see [`super::app_settings::DEFAULT_LIVE_COMPUTE_TARGET`] for why that is safe with no worker
/// configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LiveComputeTarget {
    LocalOnly,
    RemoteOnly,
    #[default]
    Both,
}

/// The default per-request cadence a newly added [`WorkerSettings`] starts with,
/// before clamping to a specific worker's advertised `Welcome::min_cadence_ms` floor
/// -- see [`WorkerSettings::effective_cadence_ms`].
pub const DEFAULT_WORKER_CADENCE_MS: u32 = 500;

/// One configured remote render worker: the connection details and per-request
/// preferences for a specific machine and link.
///
/// Deliberately does NOT include render resolution -- that stays session-wide (set
/// once, shared by every worker and the local CPU path), since every worker must
/// trace identical dimensions or the summed samples don't compose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkerSettings {
    /// User-facing label for this remote in the "Remote coordinator" form and status
    /// messages; may be empty (the address is shown instead).
    pub name: String,
    /// `host:port` this worker listens on (`pleochroma-worker serve --bind`/`--allow-remote`).
    pub address: String,
    /// Directory holding this worker's mutual-TLS bundle (`ca.pem`, `client.pem` and
    /// `client.key`, as `pleochroma-worker cert issue-client` writes them).
    ///
    /// A plain `String`, not `PathBuf`, to match the other text fields here and keep the
    /// struct trivially TOML-serializable. See [`WorkerSettings::ca_path`] and the
    /// sibling path methods.
    pub cert_dir: String,
    /// How images are transferred from the worker.
    pub transfer_mode: TransferMode,
    /// This worker's requested cadence, in milliseconds, before clamping to its own
    /// advertised floor -- see [`WorkerSettings::effective_cadence_ms`].
    pub cadence_ms: u32,
    /// Resolution of the `PREVIEW` stream, see [`PreviewScale`].
    pub preview_scale: PreviewScale,
}

impl Default for WorkerSettings {
    fn default() -> Self {
        Self {
            name: String::new(),
            address: String::new(),
            cert_dir: String::new(),
            transfer_mode: TransferMode::LiveProgressive,
            cadence_ms: DEFAULT_WORKER_CADENCE_MS,
            // See `PreviewScale::Quarter` for why a new worker starts at quarter res.
            preview_scale: PreviewScale::Quarter,
        }
    }
}

impl WorkerSettings {
    /// Path of the CA certificate file.
    #[must_use]
    pub fn ca_path(&self) -> PathBuf {
        Path::new(&self.cert_dir).join("ca.pem")
    }

    /// Path of the client certificate file.
    #[must_use]
    pub fn client_cert_path(&self) -> PathBuf {
        Path::new(&self.cert_dir).join("client.pem")
    }

    /// Path of the client private key file.
    #[must_use]
    pub fn client_key_path(&self) -> PathBuf {
        Path::new(&self.cert_dir).join("client.key")
    }

    /// Clamps [`WorkerSettings::cadence_ms`] to `min_cadence_ms` -- a worker's own
    /// advertised floor from its `Welcome::min_cadence_ms`. Requesting faster than a
    /// worker can deliver just means it coalesces anyway, but clamping up front avoids
    /// the false impression of a faster cadence than will ever be observed.
    #[must_use]
    pub const fn effective_cadence_ms(&self, min_cadence_ms: u32) -> u32 {
        if self.cadence_ms < min_cadence_ms {
            min_cadence_ms
        } else {
            self.cadence_ms
        }
    }

    /// Builds the [`StreamConfig`] this worker's settings imply for a render at the
    /// session's `width x height` resolution, against a specific worker's advertised
    /// `min_cadence_ms` (from its `Welcome`).
    #[must_use]
    pub fn stream_config(&self, min_cadence_ms: u32, width: u32, height: u32) -> StreamConfig {
        let (preview_width, preview_height) = self.preview_scale.resolve(width, height);
        StreamConfig {
            transfer_mode: self.transfer_mode,
            cadence_ms: self.effective_cadence_ms(min_cadence_ms),
            preview: Some(PreviewConfig {
                width: preview_width,
                height: preview_height,
            }),
        }
    }

    /// The export's [`StreamConfig`]: no preview, the export cadence floor, and always
    /// [`TransferMode::FinalOnly`] whatever this worker's `transfer_mode` says.
    ///
    /// Kept separate from [`Self::stream_config`], which the live viewport uses with a
    /// preview stream and progressive frames.
    ///
    /// # Why the transfer mode is forced
    ///
    /// A progressive `FRAME` is always the full `width * height * 12` bytes, however few
    /// samples it carries (~25 MB at 1080p, ~100 MB at 4K). Under `LiveProgressive` a
    /// worker can neither trace nor send heartbeats while one drains over a slow link,
    /// which can trip the export's chunk watchdog (`export_thread::remote::dispatch`).
    /// The export only needs one `FRAME` per chunk, so `FinalOnly` lets the worker trace
    /// continuously, send a bare `PROGRESS` heartbeat each cadence tick, and transfer
    /// each chunk's radiance once.
    ///
    /// The export never reads a preview stream: `run_remote_batch` ignores every
    /// `RemoteUpdate::Preview` and builds its thumbnail from the full-resolution `FRAME`
    /// buffer, so a worker-rendered preview would only cost bandwidth.
    #[must_use]
    pub fn export_stream_config(&self, min_cadence_ms: u32) -> StreamConfig {
        StreamConfig {
            transfer_mode: TransferMode::FinalOnly,
            cadence_ms: self
                .effective_cadence_ms(min_cadence_ms)
                .max(EXPORT_CADENCE_MS),
            preview: None,
        }
    }
}

/// Minimum cadence for [`WorkerSettings::export_stream_config`]: it raises whatever
/// [`WorkerSettings::effective_cadence_ms`] gives to at least this value.
///
/// The live viewport's cadence is tuned for small interactive payloads.
///
/// A `FRAME` is always `width * height * 12` bytes, so cadence sets the number of
/// full-resolution transfers, not their size. At 1920x1080 one `FRAME` is ~24.9 MB and
/// takes ~1 s to drain over a 200 Mbit/s link, so a 100 ms cadence cannot be reached
/// anyway. At 2000 ms there are 20x fewer transfers for the same samples.
pub const EXPORT_CADENCE_MS: u32 = 2000;
