//! Per-run accelerator ownership and explicit fallback diagnostics.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use pleochroma::{
    color::metrics::FinishedStone,
    geometry::ToolPrimitive,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use pleochroma_cut_core::optimize::ObjectiveFidelity;

use super::{
    OptimizerEnv, RetargetEnv,
    common::{EnvCore, EvalFailure},
};
use crate::{
    gpu_geometry::{GeometryDiagnostics, GpuGeometry},
    gpu_optics::{CudaOpticalScorer, OpticalDiagnostics, OpticalError, OpticalWork, ScoredOptics},
    gpu_solid::{GpuSolid, SolidDiagnostics, SolidResult},
};

/// Arithmetic identity for geometry, projection and optical CUDA execution.
///
/// Persisted validation and oracle caches must not mix this route with older math.
pub const CUDA_ENVIRONMENT_MATH: &str = "cuda-env-v3";

/// The physical execution route and cumulative work of one training run.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ExecutionSnapshot {
    /// Changes whenever the native environment math changes.
    pub revision: &'static str,
    /// Successfully evaluated candidate optical requests.
    pub cuda_optical_requests: u64,
    /// Unsupported optical requests explicitly routed to the CPU reference.
    pub cpu_optical_fallbacks: u64,
    /// Most recent unsupported optical input.
    pub last_fallback: Option<String>,
    /// First fatal accelerator error; these never become ordinary rejected moves.
    pub failure: Option<String>,
    /// CUDA optical batching, ray counts and host service timings.
    pub optics: OpticalDiagnostics,
    /// Projection support, launch and preservation-gate counters.
    pub geometry: GeometryDiagnostics,
    /// Solid enumeration, compact readbacks and explicit concave host work.
    pub solid: SolidDiagnostics,
}

/// Shared native CUDA services belonging to one run, never global process state.
pub struct EnvExecution {
    optics: CudaOpticalScorer,
    geometry: Arc<GpuGeometry>,
    solid: Arc<GpuSolid>,
    optical_requests: AtomicU64,
    optical_fallbacks: AtomicU64,
    last_fallback: Mutex<Option<String>>,
    failure: Mutex<Option<String>>,
    cancelled: AtomicBool,
}

impl std::fmt::Debug for EnvExecution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvExecution")
            .field("snapshot", &self.snapshot())
            .finish_non_exhaustive()
    }
}

impl EnvExecution {
    /// Compile native physics on the CUDA device used by the trainer.
    ///
    /// # Errors
    /// Returns missing CUDA/NVRTC support or kernel initialization errors.
    pub fn new_cuda(ordinal: usize) -> Result<Arc<Self>, String> {
        let solid = GpuSolid::new_cuda(ordinal)?;
        Ok(Arc::new(Self {
            optics: CudaOpticalScorer::new(ordinal, 128).map_err(|error| error.to_string())?,
            geometry: GpuGeometry::new_cuda_with_solid(ordinal, Arc::clone(&solid))?,
            solid,
            optical_requests: AtomicU64::new(0),
            optical_fallbacks: AtomicU64::new(0),
            last_fallback: Mutex::new(None),
            failure: Mutex::new(None),
            cancelled: AtomicBool::new(false),
        }))
    }

    /// Return the first infrastructure failure, separate from physical invalidity.
    ///
    /// # Errors
    /// A failed CUDA launch, lost device or violated numerical contract stops the run.
    pub fn check(&self) -> Result<(), String> {
        self.geometry.check()?;
        self.solid.check()?;
        let failure = self.failure.lock().map_err(|error| error.to_string())?;
        if let Some(error) = failure.as_ref() {
            return Err(error.clone());
        }
        self.optics.diagnostics().first_error.map_or(Ok(()), Err)
    }

    /// Read work counters without waiting for the optical kernel's execution lock.
    #[must_use]
    pub fn snapshot(&self) -> ExecutionSnapshot {
        let geometry = self.geometry.diagnostics();
        let optics = self.optics.diagnostics();
        let solid = self.solid.diagnostics();
        let failure = self
            .failure
            .lock()
            .ok()
            .and_then(|reason| reason.clone())
            .or_else(|| geometry.last_failure.clone())
            .or_else(|| solid.last_failure.clone())
            .or_else(|| optics.first_error.clone());
        ExecutionSnapshot {
            revision: CUDA_ENVIRONMENT_MATH,
            cuda_optical_requests: self.optical_requests.load(Ordering::Relaxed),
            cpu_optical_fallbacks: self.optical_fallbacks.load(Ordering::Relaxed),
            last_fallback: self
                .last_fallback
                .lock()
                .ok()
                .and_then(|reason| reason.clone()),
            failure,
            optics,
            geometry,
            solid,
        }
    }

    pub(super) fn prepare_optics(
        self: &Arc<Self>,
        stone: FinishedStone<'_>,
        material: &GemMaterial,
        fidelity: ObjectiveFidelity,
        lighting: LightingPreset,
    ) -> Result<Option<PendingOptics>, EvalFailure> {
        if self.check().is_err() {
            return Err(EvalFailure::NonFiniteScore);
        }
        // Match the reference's quantized f32 planes, not authored f64 geometry.
        let fan_planes: Vec<_> = stone
            .geom
            .planes
            .iter()
            .map(|plane| {
                (
                    glam::DVec3::new(
                        f64::from(plane.normal[0]),
                        f64::from(plane.normal[1]),
                        f64::from(plane.normal[2]),
                    ),
                    -f64::from(plane.d),
                )
            })
            .collect();
        let half_width = self
            .solid
            .measure_width(&fan_planes)
            .map_err(|_| EvalFailure::NonFiniteScore)?
            .filter(|width| width.is_finite() && *width > 0.0)
            .map_or(1.0, |width| (width * 0.5) as f32);
        match self
            .optics
            .prepare_with_width(stone, material, fidelity, lighting, half_width)
        {
            Ok(work) => {
                let mut pending = PendingOptics {
                    execution: Arc::clone(self),
                    work: Some(work),
                    result: None,
                };
                pending.poll();
                Ok(Some(pending))
            }
            Err(OpticalError::Unsupported(reason)) => {
                self.optical_fallbacks.fetch_add(1, Ordering::Relaxed);
                if let Ok(mut last) = self.last_fallback.lock() {
                    *last = Some(reason);
                }
                Ok(None)
            }
            Err(error) => {
                self.optical_failure(&error);
                Err(EvalFailure::NonFiniteScore)
            }
        }
    }

    fn optical_failure(&self, error: &OpticalError) {
        if let Ok(mut failure) = self.failure.lock()
            && failure.is_none()
        {
            *failure = Some(format!("native CUDA optical scoring failed: {error}"));
        }
    }

    pub(super) fn solid(
        &self,
        planes: &[(glam::DVec3, f64)],
        tools: &[ToolPrimitive],
    ) -> Result<SolidResult, EvalFailure> {
        self.check().map_err(|_| EvalFailure::NonFiniteScore)?;
        self.solid
            .build_and_measure(planes, tools)
            .map_err(|_| EvalFailure::NonFiniteScore)
    }
}

impl EnvCore {
    pub(super) fn set_execution(&mut self, execution: Arc<EnvExecution>) {
        self.space.set_gpu_geometry(Arc::clone(&execution.geometry));
        self.execution = Some(execution);
    }
}

impl OptimizerEnv {
    /// Attach the run's accelerator while retaining the CPU geometry acceptance gates.
    pub fn set_execution(&mut self, execution: Arc<EnvExecution>) {
        self.core.set_execution(execution);
    }
}

impl RetargetEnv {
    /// Attach the run's accelerator while retaining the CPU geometry acceptance gates.
    pub fn set_execution(&mut self, execution: Arc<EnvExecution>) {
        self.core.set_execution(execution);
    }
}

impl EnvCore {
    pub(super) fn with_execution(
        design: pleochroma_cut_core::Design,
        gem: GemMaterial,
        settings: super::common::CoreSettings,
        reward: super::RewardConfig,
        execution: Option<Arc<EnvExecution>>,
    ) -> Result<Self, super::EnvError> {
        if execution.is_none() {
            return Self::new(design, gem, settings, reward);
        }
        Self::build(design, gem, settings, reward, None, execution)
    }
}

impl OptimizerEnv {
    /// Construct an episode whose baseline and candidate optics share the accelerator.
    ///
    /// # Errors
    /// As [`Self::new`]; the owner must also check [`EnvExecution::check`] for device failures.
    pub fn new_with_execution(
        design: pleochroma_cut_core::Design,
        config: super::OptimizerEnvConfig,
        execution: Option<Arc<EnvExecution>>,
    ) -> Result<Self, super::EnvError> {
        Self::build(design, config, None, execution)
    }
}

impl RetargetEnv {
    /// Construct an episode whose baseline and candidate optics share the accelerator.
    ///
    /// # Errors
    /// As [`Self::new`]; the owner must also check [`EnvExecution::check`] for device failures.
    pub fn new_with_execution(
        design: pleochroma_cut_core::Design,
        config: super::RetargetEnvConfig,
        execution: Option<Arc<EnvExecution>>,
    ) -> Result<Self, super::EnvError> {
        Self::build(design, config, None, execution)
    }
}

/// The runtime remains alive until all owned submissions have been completed or dropped.
pub(super) struct PendingOptics {
    execution: Arc<EnvExecution>,
    work: Option<OpticalWork>,
    result: Option<Result<ScoredOptics, EvalFailure>>,
}

impl PendingOptics {
    pub(super) fn poll(&mut self) -> bool {
        if self.result.is_some() {
            return true;
        }
        let work = self.work.as_mut().expect("unfinished optical ticket");
        if self.execution.cancelled.load(Ordering::Relaxed) {
            work.cancel();
        }
        let result = match self.execution.optics.try_submit(work) {
            Ok(false) => return false,
            Ok(true) => {
                if !work.poll() {
                    return false;
                }
                self.work.take().expect("ready optical ticket").complete()
            }
            Err(error) => {
                drop(self.work.take());
                Err(error)
            }
        };
        // Account every native success once, including ready speculative candidates
        // later discarded by an earlier accepted line-search scale.
        self.result = Some(match result {
            Ok(result) => {
                self.execution
                    .optical_requests
                    .fetch_add(1, Ordering::Relaxed);
                Ok(result)
            }
            Err(error) => {
                self.execution.optical_failure(&error);
                Err(EvalFailure::NonFiniteScore)
            }
        });
        true
    }

    pub(super) fn complete(mut self) -> Result<ScoredOptics, EvalFailure> {
        if !self.poll() {
            self.execution.optical_failure(&OpticalError::Runtime(
                "optical request completed before readiness".into(),
            ));
            return Err(EvalFailure::NonFiniteScore);
        }
        self.result.take().expect("ready optical result")
    }
}
