//! Export request validation and the export's parameter types.
//!
//! The default output path is resolved by `filename_template`, not here.

use crate::settings::{ExportTransfer, WorkerSettings};

/// Smallest allowed export width or height in pixels. The maxima below stop an accidental
/// huge value from locking up the machine.
pub const MIN_EXPORT_DIM: u32 = 16;
/// Largest allowed export width or height in pixels.
pub const MAX_EXPORT_DIM: u32 = 8192;
/// Smallest allowed export samples per pixel.
pub const MIN_EXPORT_SPP: u32 = 1;
/// Generous on purpose: the cap only stops a typo such as an extra zero.
pub const MAX_EXPORT_SPP: u32 = 32768;

/// Bounds for the export's max-bounce cap.
///
/// The ladder in `export_dialog.slint` is 4/8/12/24/64/128. 128 is the ceiling because
/// `bounce_cost.rs` found no further image change beyond it. The values are checked even though the
/// dialog sends only ladder rungs.
pub const MIN_EXPORT_BOUNCES: u32 = 1;
/// Largest allowed export bounce count.
pub const MAX_EXPORT_BOUNCES: u32 = 128;

/// User-chosen parameters of one export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportParams {
    /// The image-forming tracer, independent of compute target.
    pub engine: pleochroma::renderer::engine::EngineSpec,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Samples per pixel.
    pub samples_per_pixel: u32,
    /// The export's own max-bounce cap. It is not read from the live viewport.
    pub max_bounces: u32,
}

/// Which engines an export uses: the "Compute" pill of the export dialog, matching
/// `export_dialog.slint`'s `compute_target` (0/1/2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeTarget {
    LocalOnly,
    RemoteOnly,
    /// Local and remote trace disjoint sample ranges concurrently (see `export_thread::remote`).
    Both,
}

impl ComputeTarget {
    /// Whether the export suspends the live viewport. `RemoteOnly` uses no local CPU, GPU or
    /// probe, so the viewport keeps tracing; `LocalOnly` and `Both` share the hardware and
    /// pause it.
    #[must_use]
    pub const fn pauses_live_viewport(self) -> bool {
        !matches!(self, Self::RemoteOnly)
    }

    /// Whether a remote lane could take part: `RemoteOnly` always (it traces nothing here),
    /// `Both` only when a worker is configured (without one it renders locally), `LocalOnly`
    /// never.
    #[cfg(feature = "adaptive-sampling")]
    #[must_use]
    pub const fn uses_remote(self, worker_configured: bool) -> bool {
        match self {
            Self::LocalOnly => false,
            Self::RemoteOnly => true,
            Self::Both => worker_configured,
        }
    }
}

/// The remote side of one export or tilt video: the engines ([`ComputeTarget`]), the configured
/// remote's connection (`None` if none is configured) and how its result travels
/// ([`ExportTransfer`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSelection {
    /// The export dialog's "Compute" pill (the tilt video section has its own).
    pub compute_target: ComputeTarget,
    /// The remote endpoint's connection settings, if one is configured.
    pub worker: Option<WorkerSettings>,
    /// "Transfer: Full data / Final picture only".
    pub transfer: ExportTransfer,
    /// Whether this machine's idle CPU and GPU trace a share of a final-picture export
    /// alongside the remote (`AppSettings::contribute_to_final_picture`). Matters only for
    /// [`ExportTransfer::FinalPicture`] with [`ComputeTarget::Both`]; see
    /// `worker::final_picture::contribution_allowed`.
    pub contribute_local: bool,
}

#[cfg(test)]
impl RemoteSelection {
    /// A local-only selection (no remote involved at all) -- test fixtures.
    #[must_use]
    pub const fn local_only() -> Self {
        Self {
            compute_target: ComputeTarget::LocalOnly,
            worker: None,
            transfer: ExportTransfer::FullData,
            contribute_local: false,
        }
    }
}

/// Validates raw (Slint-supplied, hence signed) export request fields, rejecting zero,
/// negative, and absurdly large values before anything is allocated or a thread is
/// spawned. Pure and side-effect-free so it's directly unit-testable.
pub fn validate_export_params(
    width: i32,
    height: i32,
    samples_per_pixel: i32,
    max_bounces: i32,
) -> Result<ExportParams, String> {
    if width <= 0 || height <= 0 || samples_per_pixel <= 0 || max_bounces <= 0 {
        return Err(
            "Width, height, sample count, and max bounces must all be positive.".to_string(),
        );
    }
    let width = width.cast_unsigned();
    let height = height.cast_unsigned();
    let samples_per_pixel = samples_per_pixel.cast_unsigned();
    let max_bounces = max_bounces.cast_unsigned();

    if width < MIN_EXPORT_DIM || height < MIN_EXPORT_DIM {
        return Err(format!(
            "Minimum export size is {MIN_EXPORT_DIM}x{MIN_EXPORT_DIM} px."
        ));
    }
    if width > MAX_EXPORT_DIM || height > MAX_EXPORT_DIM {
        return Err(format!(
            "Maximum export size is {MAX_EXPORT_DIM}x{MAX_EXPORT_DIM} px (requested {width}x{height})."
        ));
    }
    if samples_per_pixel < MIN_EXPORT_SPP {
        return Err("Sample count must be at least 1.".to_string());
    }
    if samples_per_pixel > MAX_EXPORT_SPP {
        return Err(format!(
            "Maximum sample count is {MAX_EXPORT_SPP} samples per pixel."
        ));
    }
    if !(MIN_EXPORT_BOUNCES..=MAX_EXPORT_BOUNCES).contains(&max_bounces) {
        return Err(format!(
            "Max bounces must be between {MIN_EXPORT_BOUNCES} and {MAX_EXPORT_BOUNCES}."
        ));
    }

    Ok(ExportParams {
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        width,
        height,
        samples_per_pixel,
        max_bounces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_export_params_rejects_zero_and_negative() {
        assert!(validate_export_params(0, 1080, 64, 12).is_err());
        assert!(validate_export_params(1920, 0, 64, 12).is_err());
        assert!(validate_export_params(1920, 1080, 0, 12).is_err());
        assert!(validate_export_params(1920, 1080, 64, 0).is_err());
        assert!(validate_export_params(-1920, 1080, 64, 12).is_err());
        assert!(validate_export_params(1920, -1080, 64, 12).is_err());
        assert!(validate_export_params(1920, 1080, -1, 12).is_err());
        assert!(validate_export_params(1920, 1080, 64, -1).is_err());
    }

    #[test]
    fn validate_export_params_rejects_absurdly_large_requests() {
        let err = validate_export_params(100_000, 100_000, 64, 12).unwrap_err();
        assert!(err.contains("Maximum export size"));
    }

    #[test]
    fn validate_export_params_rejects_excessive_sample_counts() {
        let err = validate_export_params(1920, 1080, 1_000_000, 12).unwrap_err();
        assert!(err.contains("Maximum sample count"));
    }

    #[test]
    fn validate_export_params_rejects_tiny_dimensions_below_minimum() {
        assert!(validate_export_params(1, 1, 64, 12).is_err());
    }

    /// The ladder `export_dialog.slint` offers tops out at 128, so anything above that
    /// is rejected rather than silently clamped.
    #[test]
    fn validate_export_params_rejects_bounce_caps_outside_the_ladders_range() {
        let err = validate_export_params(1920, 1080, 64, 129).unwrap_err();
        assert!(err.contains("Max bounces"));
        assert!(validate_export_params(1920, 1080, 64, 0).is_err());
    }

    #[test]
    fn validate_export_params_accepts_sensible_presets() {
        assert_eq!(
            validate_export_params(1920, 1080, 256, 12).unwrap(),
            ExportParams {
                engine: pleochroma::renderer::engine::EngineSpec::default(),
                width: 1920,
                height: 1080,
                samples_per_pixel: 256,
                max_bounces: 12,
            }
        );
        assert_eq!(
            validate_export_params(3840, 2160, 64, 128).unwrap(),
            ExportParams {
                engine: pleochroma::renderer::engine::EngineSpec::default(),
                width: 3840,
                height: 2160,
                samples_per_pixel: 64,
                max_bounces: 128,
            }
        );
    }

    #[test]
    fn validate_export_params_accepts_boundary_values() {
        assert!(
            validate_export_params(
                MIN_EXPORT_DIM.cast_signed(),
                MIN_EXPORT_DIM.cast_signed(),
                MIN_EXPORT_SPP.cast_signed(),
                MIN_EXPORT_BOUNCES.cast_signed(),
            )
            .is_ok()
        );
        assert!(
            validate_export_params(
                MAX_EXPORT_DIM.cast_signed(),
                MAX_EXPORT_DIM.cast_signed(),
                MAX_EXPORT_SPP.cast_signed(),
                MAX_EXPORT_BOUNCES.cast_signed(),
            )
            .is_ok()
        );
    }

    // Default output path coverage lives in `filename_template::tests`, which
    // tests the configurable template resolver.
}
