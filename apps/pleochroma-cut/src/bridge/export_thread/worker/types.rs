//! The finished-accumulation/outcome/calibration-carry types [`super::core::render_accumulation`]
//! and its callers share.

use super::taps::RenderTaps;
use crate::bridge::export_thread::remote::RemoteCapability;
use glam::Vec3;

/// One frame/still image's finished linear accumulation buffer, before tone-mapping --
/// [`super::core::render_accumulation`]'s successful output. `accum` holds exactly
/// `width * height` summed XYZ radiance samples; `samples_per_pixel` always equals the
/// [`super::super::params::ExportParams::samples_per_pixel`] that call was rendered
/// with.
pub struct Accumulation {
    /// Summed XYZ radiance, one entry per pixel, `width * height` long. An adaptive render
    /// (the `adaptive` field, `adaptive-sampling` builds) holds each pixel's MEAN here, already
    /// divided by its own count.
    pub accum: Vec<Vec3>,
    /// How many samples deep each pixel in [`Self::accum`] is. An adaptive render has no
    /// frame-wide count: its buffer holds means, so this is `1` and the tone-map divides by one.
    pub samples_per_pixel: u32,
    /// Per-pixel counts and figures of an adaptive render; `None` for every other render.
    /// Present only with the `adaptive-sampling` feature.
    #[cfg(feature = "adaptive-sampling")]
    pub adaptive: Option<AdaptiveStats>,
}

/// What an adaptive render traced: the per-pixel counts and the figures of its log line.
///
/// Only an adaptive render builds one, so it exists only in a build with the
/// `adaptive-sampling` feature, like [`Accumulation::adaptive`].
#[cfg(feature = "adaptive-sampling")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveStats {
    /// The samples each pixel received, row-major.
    pub counts: Vec<u32>,
    /// Samples per pixel the live tiles received (the most any pixel has).
    pub samples_done: u32,
    /// Samples traced over the whole frame (the sum of [`Self::counts`]).
    pub samples_traced: u64,
    /// The cap: the render's samples per pixel.
    pub max_spp: u32,
    /// Tiles in the frame.
    pub tiles_total: usize,
    /// Tiles that met the target and stopped before the cap.
    pub tiles_converged: usize,
}

#[cfg(feature = "adaptive-sampling")]
impl AdaptiveStats {
    /// The one-line note an adaptive render ends with: samples traced, the saving against the
    /// fixed `width x height x max_spp` render, and the tiles that converged.
    #[must_use]
    pub fn note(&self, width: u32, height: u32) -> String {
        let full = u64::from(width) * u64::from(height) * u64::from(self.max_spp);
        let saving = if full == 0 {
            0.0
        } else {
            100.0 * (1.0 - self.samples_traced as f64 / full as f64)
        };
        format!(
            "adaptive sampling: {} samples traced, {saving:.1} % fewer than {width} x {height} x {} \
             ({} of {} tiles converged)",
            self.samples_traced, self.max_spp, self.tiles_converged, self.tiles_total
        )
    }
}

/// Result of [`super::core::render_accumulation`].
///
/// Like [`super::super::ExportOutcome`], but it stops before tone-mapping and writing to disk:
/// [`super::export::run_export_tapped`] (the still export) and the tilt video's per-frame PNG write
/// do those.
pub enum AccumulationOutcome {
    /// The render finished; `run_export_tapped`/the tilt video tone-map and save this.
    Completed(Accumulation),
    /// `cancel` was observed before this render finished.
    Cancelled,
    /// The render failed outright (only reachable via `ComputeTarget::RemoteOnly` with
    /// no local fallback to pick up a lost remote chunk).
    Failed(String),
}

/// What one sweep remembers about the remote's final-picture transfer
/// ([`AccumulationCarry::final_picture_transfer`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::bridge::export_thread::worker) enum FinalPictureTransferMemory {
    /// Nothing yet: a final picture may still be asked for and no note was shown.
    #[default]
    Untried,
    /// The note "the remote does not support final-picture transfer" was shown: the remote
    /// refused before this sweep, so nothing was asked in it.
    RefusalNoted,
    /// The remote declined or failed a `FinalImageRequest` during this sweep (its note was
    /// shown then): every later frame goes straight to full data instead of asking again.
    Declined,
}

impl FinalPictureTransferMemory {
    /// Whether a final-picture request was declined or failed during this sweep.
    #[must_use]
    pub(in crate::bridge::export_thread::worker) const fn declined(self) -> bool {
        matches!(self, Self::Declined)
    }

    /// Whether the note about the missing final-picture transfer was already shown.
    #[must_use]
    pub(in crate::bridge::export_thread::worker) const fn noted(self) -> bool {
        !matches!(self, Self::Untried)
    }
}

/// Probing and calibration state that one [`super::core::render_accumulation`] call hands to the
/// next call of the same render session.
///
/// A multi-frame caller (the tilt performance video) pays remote probing and the local hybrid
/// CPU/GPU split calibration once per sweep, not once per frame: adapter acquisition and a
/// network handshake are too slow to repeat every frame.
/// [`super::export::run_export_tapped`] always starts from a fresh carry
/// ([`AccumulationCarry::with_taps`]).
#[derive(Default)]
pub struct AccumulationCarry {
    /// Small deterministic GPU backend, reused across tilt frames.
    pub(in crate::bridge::export_thread::worker) ray_gpu:
        Option<std::sync::Arc<pleochroma::renderer::gpu_backend::whitted::WhittedGpuBackend>>,
    /// Whether remote availability has been probed for this carry. Once `true`,
    /// [`super::core::render_accumulation`] reuses [`Self::remote_capability`] instead of
    /// probing again. A fresh [`AccumulationCarry`] starts `false`, so a still export probes
    /// once per export.
    pub(in crate::bridge::export_thread::worker) remote_probed: bool,
    /// The probed remote worker, if any. `None` covers both "not probed yet" (while
    /// [`Self::remote_probed`] is `false`) and "probed and unavailable" (once it's
    /// `true`).
    pub(in crate::bridge::export_thread::worker) remote_capability: Option<RemoteCapability>,
    /// Beam-tile connections kept across the frames of one sweep, so a tilt video does not
    /// reconnect for every frame.
    #[cfg(feature = "beam")]
    pub(in crate::bridge::export_thread::worker) beam_clients:
        crate::bridge::export_thread::remote::BeamClientPool,
    /// The local hybrid CPU/GPU split's current best estimate -- seeded from a prior
    /// frame's calibration/adaptation instead of re-measuring one from scratch. Reset
    /// to whatever the most recent frame ended on (including `None` if the GPU
    /// declined partway), so a persistently failing GPU is retried, not permanently
    /// written off, across frames. `Some(1.0)` is the GPU alone: the split settled there
    /// because the GPU does 70 % or more of the work (`batch::settled_gpu_share`), and every
    /// later frame of the sweep keeps it without measuring again.
    pub(in crate::bridge::export_thread::worker) hybrid_frac: Option<f64>,
    /// The remote lane's current best throughput estimate, samples/sec -- seeded from
    /// a prior frame's calibration/adaptation instead of re-measuring one from
    /// scratch.
    pub(in crate::bridge::export_thread::worker) remote_rate: Option<f64>,
    /// What this sweep knows about the remote's final-picture transfer: whether a
    /// `FinalImageRequest` was declined or failed (every later frame then goes straight to
    /// full data), and whether the note about it was shown (a video shows it once, not once
    /// per frame).
    pub(in crate::bridge::export_thread::worker) final_picture_transfer: FinalPictureTransferMemory,
    /// A frame of this sweep asked for a path cache it does not keep and was told that it is not
    /// used (`placement::ONE_SHOT_NOTE`). The later frames are the same request, so they skip the
    /// cache's decision and the note is said once.
    pub(in crate::bridge::export_thread::worker) one_shot_cache_refused: bool,
    /// The optional taps of this sweep (sample offset, linear-image sink); none by default.
    pub(in crate::bridge::export_thread::worker) taps: RenderTaps,
}

impl AccumulationCarry {
    /// A fresh carry whose renders use `taps`.
    #[must_use]
    pub fn with_taps(taps: RenderTaps) -> Self {
        Self {
            taps,
            ..Self::default()
        }
    }

    /// Names the frame the next render makes, for the sink (`frame_0007`); `None` for a still.
    pub fn set_frame_name(&mut self, name: Option<String>) {
        self.taps.frame_name = name;
    }
}

#[cfg(test)]
mod tests {
    use super::FinalPictureTransferMemory;

    #[test]
    fn the_transfer_memory_has_the_three_states_of_the_two_former_flags() {
        // (declined, noted) was (false, false), (false, true) or (true, true).
        let untried = FinalPictureTransferMemory::default();
        assert_eq!(untried, FinalPictureTransferMemory::Untried);
        assert!(!untried.declined() && !untried.noted());
        let noted = FinalPictureTransferMemory::RefusalNoted;
        assert!(!noted.declined() && noted.noted());
        let declined = FinalPictureTransferMemory::Declined;
        assert!(declined.declined() && declined.noted());
    }
}
