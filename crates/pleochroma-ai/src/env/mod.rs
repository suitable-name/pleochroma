//! Reinforcement learning environments and Markov Decision Processes for gemstone design.
//!
//! Both environments are deterministic and greedy (see [`traits`]), take a per-tier action
//! ([`ContinuousTierAction`]) and describe the stone with the observation layout of
//! [`observation`], versioned by [`OBS_LAYOUT_VERSION`]. A step may try its action at several
//! scales and keep the first improving one ([`GemstoneRlEnv::step_scaled`]). [`teacher`]
//! holds measured per-tier component sensitivities and a separate verified behavior target.
//! Both environments expose `probe_tier` and `teacher_label_from_probes` so a trainer can
//! schedule probes independently, then select the best measured coordinate or gradient
//! candidate with bounded rescue. [`assemble_label`] and [`OracleOutcome`] remain legacy
//! slope-only helpers; they do not produce the behavior target.

pub mod action;
pub(crate) mod common;
mod continuations;
pub mod design_features;
pub mod error;
mod evaluation;
mod features;
mod runtime;
pub use runtime::{CUDA_ENVIRONMENT_MATH, EnvExecution, ExecutionSnapshot};
pub mod material_features;
pub(crate) mod moves;
pub mod observation;
mod observe;
pub mod optimizer_env;
mod projection_benchmark;
pub mod recovery;
pub mod retarget_env;
pub mod reward;
pub(crate) mod stepping;
pub mod teacher;
#[cfg(test)]
mod test_fixtures;
pub mod traits;

pub use action::ContinuousTierAction;
pub use common::design_source_material;
pub use error::EnvError;
pub use moves::{MAX_SAFE_CANDIDATE_ANGLE_DEG, MIN_SAFE_CANDIDATE_ANGLE_DEG};
pub use observation::{
    CROWN_MAX_ANGLE_DEG, CROWN_MIN_ANGLE_DEG, GLOBAL_DIM, GemstoneObservation, GlobalFeature,
    IMMOBILE_INDEX, MOBILITY_INDEX, OBS_LAYOUT_VERSION, ObservationError,
    PAVILION_CRITICAL_MARGIN_DEG, PAVILION_MAX_ANGLE_DEG, TASK_KIND_INDEX, TIER_DIM, TierFeature,
    WEIGHT_FRACTION_INDEX, angle_envelope, flat_len, objective_coefficients, split_flat,
    weight_fractions,
};
pub use optimizer_env::{OptimizerEnv, OptimizerEnvConfig};
pub use projection_benchmark::{
    ProjectionActionReport, ProjectionAttemptReport, ProjectionComparison, ProjectionProbeConfig,
    ProjectionProbeEvent, ProjectionProbeReport, projection_probe_actions, run_projection_probe,
    run_projection_probe_with_events,
};
pub use retarget_env::{RetargetEnv, RetargetEnvConfig};
pub use reward::{RewardConfig, RewardEvaluator, StepFlags};
pub use stepping::StepWork;
pub use teacher::TeacherLabelWork;
pub use teacher::{
    COMPONENT_BRILLIANCE_LOSS, COMPONENT_DEEPER_TONE_LOSS, COMPONENT_EXTINCTION,
    COMPONENT_LIGHTER_TONE_LOSS, COMPONENT_SHAPE_DISTANCE, COMPONENT_WINDOWING,
    COMPONENT_YIELD_LOSS, CoordinateProbe, DEFAULT_LINE_SEARCH, MEAN_BOUND, N_COMPONENTS,
    OracleOutcome, SLOPE_EPS, SLOPE_NORM_FLOOR, TEACHER_SEARCH_RECOVERY_REVISION, TeacherLabel,
    TeacherSearchSnapshot, TierProbe, TierProbeWork, TierSensitivity, action_from_direction,
    assemble_label, direction_from_slopes, score_slopes,
};
pub use traits::{GemstoneRlEnv, Step, StepInfo, StepRejection};
