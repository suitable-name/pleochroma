//! Native CUDA port of the analytical training objective, sharing Burn's CUDA device.
//!
//! Physics comes from `pleochroma/src/color/metrics` and `optics/birefringence`, not the
//! beauty renderer. Native kernels batch d-line rays over poses/cells/apertures/modes.
//! Lighting classification, counters, ordered tone compaction, band evaluation and
//! spectral sums stay on CUDA. Compact summaries/spectra return for the CIE transform
//! and ordered objective mean. CUDA floating
//! arithmetic is not yet certified bit-identical to the CPU parity oracle: callers must
//! retain this provenance when persisting scores and certify discrete decisions on CPU.
use pleochroma::{
    color::metrics::{FaceUpTone, FinishedStone},
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use pleochroma_cut_core::{ObjectiveComponents, optimize::ObjectiveFidelity};
use std::{fmt, sync::atomic::AtomicBool};

#[cfg(feature = "cuda")]
mod buffers;
mod capacity;
mod diagnostics;
mod submission;
pub use submission::OpticalWork;
#[cfg(feature = "cuda")]
mod fan;
#[cfg(feature = "cuda")]
mod lighting;
#[cfg(feature = "cuda")]
mod postprocess;
#[cfg(all(test, feature = "cuda"))]
mod postprocess_tests;
#[cfg(feature = "cuda")]
mod prepare;
#[cfg(all(test, feature = "cuda"))]
mod reduce;
#[cfg(feature = "cuda")]
mod runtime;
#[cfg(all(test, feature = "cuda"))]
mod tests;
#[cfg(feature = "cuda")]
mod tone;
#[cfg(feature = "cuda")]
mod tone_cuda;
#[cfg(feature = "cuda")]
mod tools;
#[cfg(feature = "cuda")]
mod worker;
pub use capacity::OpticalCapacity;
pub use diagnostics::OpticalDiagnostics;

/// Scoring arithmetic identity, kept separate from the exact CPU reference.
pub const CUDA_OPTICAL_PROVENANCE: &str = "native-cuda-analytical-v2-unverified";

/// An optical service failure. Only `Unsupported` permits explicit CPU fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpticalError {
    /// The compiled feature, CUDA device or initialization resources are unavailable.
    Unavailable(String),
    /// A valid request exceeds native capabilities and may use an explicit CPU fallback.
    Unsupported(String),
    /// Geometry or optical parameters violate the scoring input contract.
    InvalidInput(String),
    /// The caller requested cancellation before the score was returned.
    Cancelled,
    /// Worker, kernel, transfer or numerical execution failed; CPU retry is not permitted.
    Runtime(String),
}
impl fmt::Display for OpticalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(s) => write!(f, "native CUDA optics unavailable: {s}"),
            Self::Unsupported(s) => write!(f, "native CUDA optics unsupported: {s}"),
            Self::InvalidInput(s) => write!(f, "invalid native CUDA optics input: {s}"),
            Self::Cancelled => f.write_str("native CUDA optics cancelled"),
            Self::Runtime(s) => write!(f, "native CUDA optics failed: {s}"),
        }
    }
}
impl std::error::Error for OpticalError {}

/// One completed candidate's optical objective and face-up tone.
pub struct ScoredOptics {
    /// Optical percentages at face-up for Fast fidelity, or averaged over the Full tilt sweep.
    pub components: ObjectiveComponents,
    /// Face-up spectral tone for either fidelity; absent when no tone can be measured.
    pub tone: FaceUpTone,
    /// Evaluated poses, including face-up tone: zero for empty geometry, one for Fast.
    pub poses: usize,
}

/// Per-run service. A bounded worker queue batches concurrent Fast candidates; Full pose
/// tiles bound device/host memory. Geometry and material upload once per candidate batch.
pub struct CudaOpticalScorer {
    #[cfg(feature = "cuda")]
    inner: worker::Worker,
}

impl CudaOpticalScorer {
    #[must_use]
    /// Snapshot of queue, native kernel and service-wall counters; no GPU synchronization.
    pub fn diagnostics(&self) -> OpticalDiagnostics {
        #[cfg(feature = "cuda")]
        {
            self.inner.diagnostics()
        }
        #[cfg(not(feature = "cuda"))]
        {
            OpticalDiagnostics::default()
        }
    }
    /// Native device ordinal; pass the same ordinal used by Burn's `Device::cuda`.
    ///
    /// # Errors
    /// Returns `Unavailable` when CUDA/NVRTC or the compiled feature is unavailable,
    /// and `Runtime` for device initialization/allocation failures.
    pub fn new(device_ordinal: usize, max_poses_per_batch: usize) -> Result<Self, OpticalError> {
        #[cfg(feature = "cuda")]
        {
            // Dynamic CUDA/NVRTC loaders may panic when their libraries are absent.
            // Keep initialization an explicit service error at this public boundary.
            let inner = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                worker::Worker::new(device_ordinal, max_poses_per_batch)
            }))
            .map_err(|payload| {
                let reason = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                    .unwrap_or_else(|| "CUDA/NVRTC initialization panic".into());
                OpticalError::Unavailable(reason)
            })??;
            Ok(Self { inner })
        }
        #[cfg(not(feature = "cuda"))]
        {
            let _ = (device_ordinal, max_poses_per_batch);
            Err(OpticalError::Unavailable(
                "pleochroma-ai was built without the cuda feature".into(),
            ))
        }
    }

    /// Capability check is independent of CUDA submission; no physics is silently dropped.
    ///
    /// # Errors
    /// Returns `Unsupported` for request limits, `InvalidInput` for invalid finite geometry
    /// or optics, and `Unavailable` when this binary has no CUDA feature.
    pub fn supports(
        &self,
        stone: FinishedStone<'_>,
        material: &GemMaterial,
    ) -> Result<(), OpticalError> {
        #[cfg(feature = "cuda")]
        {
            prepare::validate(stone, material)
        }
        #[cfg(not(feature = "cuda"))]
        {
            let _ = (stone, material);
            Err(OpticalError::Unavailable(
                "pleochroma-ai was built without the cuda feature".into(),
            ))
        }
    }

    /// Prepare an owned request with the exact quantized geometry fan half-width.
    /// No queue wait or optical execution occurs on the caller.
    ///
    /// # Errors
    /// Returns capability/input errors or an existing service failure.
    pub fn prepare_with_width(
        &self,
        stone: FinishedStone<'_>,
        material: &GemMaterial,
        fidelity: ObjectiveFidelity,
        preset: LightingPreset,
        half_width: f32,
    ) -> Result<OpticalWork, OpticalError> {
        self.supports(stone, material)?;
        if !half_width.is_finite() || half_width <= 0.0 {
            return Err(OpticalError::InvalidInput(
                "fan half-width must be finite and positive".into(),
            ));
        }
        #[cfg(feature = "cuda")]
        {
            self.inner
                .prepare(stone, material, fidelity, preset, Some(half_width))
        }
        #[cfg(not(feature = "cuda"))]
        {
            let _ = (fidelity, preset);
            Err(OpticalError::Unavailable("CUDA feature is disabled".into()))
        }
    }

    /// Try once to enqueue a prepared request. `false` retains ownership on backpressure.
    /// The caller must bound its retained requests and retry from its coordinator.
    ///
    /// # Errors
    /// Returns cancellation or a fatal service/disconnection error.
    pub fn try_submit(&self, work: &mut OpticalWork) -> Result<bool, OpticalError> {
        #[cfg(feature = "cuda")]
        {
            self.inner.try_submit(work)
        }
        #[cfg(not(feature = "cuda"))]
        {
            let _ = work;
            Err(OpticalError::Unavailable("CUDA feature is disabled".into()))
        }
    }

    /// Scores one candidate using the persistent native CUDA worker.
    ///
    /// # Errors
    /// Returns capability/input errors before queuing, `Cancelled` when requested, and
    /// `Runtime` for worker, kernel, transfer or numeric failures. It never runs a CPU fallback.
    pub fn evaluate(
        &self,
        stone: FinishedStone<'_>,
        material: &GemMaterial,
        fidelity: ObjectiveFidelity,
        preset: LightingPreset,
        cancel: &AtomicBool,
    ) -> Result<ScoredOptics, OpticalError> {
        self.supports(stone, material)?;
        #[cfg(feature = "cuda")]
        {
            self.inner
                .evaluate(stone, material, fidelity, preset, cancel, None)
        }
        #[cfg(not(feature = "cuda"))]
        {
            let _ = (fidelity, preset, cancel);
            Err(OpticalError::Unavailable(
                "pleochroma-ai was built without the cuda feature".into(),
            ))
        }
    }
    /// Scores with the exact fan half-width of the quantized `GpuFacetPlane` geometry.
    ///
    /// Supply `(measure_model_width(planes) * 0.5) as f32`, or the reference fallback
    /// `1.0` when that measurement is absent. A design's f64 geometry width is distinct.
    /// This CUDA route never measures the fan on the CPU.
    ///
    /// # Errors
    /// Returns [`Self::evaluate`]'s errors, plus `InvalidInput` for a width that is not
    /// finite and positive.
    pub fn evaluate_with_width(
        &self,
        stone: FinishedStone<'_>,
        material: &GemMaterial,
        fidelity: ObjectiveFidelity,
        preset: LightingPreset,
        cancel: &AtomicBool,
        half_width: f32,
    ) -> Result<ScoredOptics, OpticalError> {
        self.supports(stone, material)?;
        if !half_width.is_finite() || half_width <= 0.0 {
            return Err(OpticalError::InvalidInput(
                "fan half-width must be finite and positive".into(),
            ));
        }
        #[cfg(feature = "cuda")]
        {
            self.inner
                .evaluate(stone, material, fidelity, preset, cancel, Some(half_width))
        }
        #[cfg(not(feature = "cuda"))]
        {
            let _ = (fidelity, preset, cancel);
            Err(OpticalError::Unavailable(
                "pleochroma-ai was built without the cuda feature".into(),
            ))
        }
    }
}

#[cfg(feature = "cuda")]
fn head_shadow_cone_cos(studio_rig: bool, degrees: f32) -> f32 {
    use pleochroma::optics::raytracer::DEFAULT_HEAD_SHADOW_DEG;
    if studio_rig || degrees.is_nan() || degrees.to_bits() == DEFAULT_HEAD_SHADOW_DEG.to_bits() {
        0.96
    } else if degrees <= 0.0 {
        2.0
    } else {
        let offset = 0.96f32.acos() - 16.0f32.to_radians();
        (degrees.clamp(1.0, 89.0).to_radians() + offset).cos()
    }
}
