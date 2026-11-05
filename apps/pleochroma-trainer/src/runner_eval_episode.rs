//! Durable validation episodes at completed steps, including independent teacher progress.
//!
//! Every journal write is atomic and fsync'd, so commits are batched where a crash only costs
//! recomputation. A model, sampled or noise pass replays its uncommitted steps from the last
//! committed episode, pilot RNG and accounting, so it commits at most every
//! [`COMMIT_INTERVAL`] and always at its terminal step (the completed-pass record). An oracle
//! pass commits every step: its teacher journal is keyed by step and identifies the exact
//! state, so a replay must not cross a step. Teacher snapshots are cumulative, so skipping one
//! only loses work; they are written at most every [`COMMIT_INTERVAL`] per step. The journal
//! therefore never claims a step or probe that was not completed.

use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use pleochroma_ai::env::recovery::EpisodeSnapshot;
use serde::{Deserialize, Serialize};

use super::{
    CaseProgress, ContinuousTierAction, EnvKind, EvalJob, GemstoneSetAttentionNetwork,
    InferenceScratch, Optics, Pilot, Played, RunnerError, action_from_direction,
    recovery::CaseJournal, work,
};

const VERSION: u32 = 1;

/// Longest stretch of batched journal work a crash may cost.
const COMMIT_INTERVAL: Duration = Duration::from_secs(2);

/// Whether the step just played must be committed now (see the module docs).
fn commit_due(state: &PassSnapshot, since_commit: Duration) -> bool {
    state.done || matches!(state.pilot, Pilot::Oracle) || since_commit >= COMMIT_INTERVAL
}

/// Reference-only continuation must not initialize a cold policy backend unnecessarily.
pub(super) fn needs_policy(journal: &CaseJournal, sampled: bool) -> Result<bool, RunnerError> {
    for part in [Some("pass-model"), sampled.then_some("pass-sampled")]
        .into_iter()
        .flatten()
    {
        let saved: Option<PassSnapshot> = journal.load(part)?;
        if saved.is_none_or(|saved| saved.version != VERSION || !saved.done) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// One atomic payload commits the accepted geometry, RNG and accounting together. A terminal
/// payload is also the completed-pass record, so no gap exists between the last step and its result.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PassSnapshot {
    version: u32,
    episode: EpisodeSnapshot,
    pilot: Pilot,
    steps: usize,
    evaluations: usize,
    accepted: usize,
    rejections: BTreeMap<String, usize>,
    action_rms_sum: f64,
    base: Optics,
    elapsed_seconds: f64,
    done: bool,
}

impl Pilot {
    fn key(&self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Sampled { .. } => "sampled",
            Self::Noise { .. } => "noise",
            Self::Oracle => "oracle",
        }
    }

    /// RNG state advances, but the pass kind and distribution cannot change during recovery.
    fn same_distribution(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Model, Self::Model) | (Self::Oracle, Self::Oracle) => true,
            (Self::Sampled { explore_std: a, .. }, Self::Sampled { explore_std: b, .. }) => {
                a.map(f32::to_bits) == b.map(f32::to_bits)
            }
            (Self::Noise { std: a, .. }, Self::Noise { std: b, .. }) => a.to_bits() == b.to_bits(),
            _ => false,
        }
    }
}

impl PassSnapshot {
    fn start(env: &EnvKind, pilot: Pilot, base: Optics) -> Result<Self, RunnerError> {
        Ok(Self {
            version: VERSION,
            episode: env.snapshot_episode()?,
            pilot,
            steps: 0,
            evaluations: 0,
            accepted: 0,
            rejections: BTreeMap::new(),
            action_rms_sum: 0.0,
            base,
            elapsed_seconds: 0.0,
            done: false,
        })
    }

    fn restore(&self, env: &mut EnvKind, expected: &Pilot) -> Result<(), RunnerError> {
        let rejected = self
            .rejections
            .values()
            .try_fold(0usize, |sum, value| sum.checked_add(*value));
        let base_is_finite = [
            self.base.brilliance,
            self.base.windowing,
            self.base.extinction,
            self.base.yield_pct,
        ]
        .into_iter()
        .all(f32::is_finite);
        if self.version != VERSION
            || !self.pilot.same_distribution(expected)
            || self.accepted > self.steps
            || rejected.is_none_or(|count| count > self.steps - self.accepted)
            || !self.action_rms_sum.is_finite()
            || self.action_rms_sum < 0.0
            || !self.elapsed_seconds.is_finite()
            || self.elapsed_seconds < 0.0
            || !base_is_finite
            || (self.done && self.steps == 0)
        {
            return Err(RunnerError::Data(
                "invalid validation pass recovery payload".into(),
            ));
        }
        env.restore_episode(&self.episode)?;
        if env.step_count() != self.steps {
            return Err(RunnerError::Data(
                "validation episode and pass step counts differ".into(),
            ));
        }
        Ok(())
    }

    fn played(self, env: &EnvKind, restored_complete: bool) -> Played {
        Played {
            obs: env.build_observation(),
            steps: self.steps,
            evaluations: self.evaluations,
            accepted: self.accepted,
            rejections: self.rejections,
            action_rms_sum: self.action_rms_sum,
            base: self.base,
            seconds: self.elapsed_seconds,
            restored_complete,
        }
    }
}

impl EvalJob<'_> {
    /// Continues a pass from its last committed step. A stored terminal step returns without a
    /// policy forward, RNG draw, optical evaluation, or repeated environment step.
    pub(super) fn play(
        &self,
        env: &mut EnvKind,
        pilot: Pilot,
        scratch: &mut InferenceScratch,
        progress: &CaseProgress<'_>,
        journal: Option<&CaseJournal>,
    ) -> Result<Played, RunnerError> {
        let part = format!("pass-{}", pilot.key());
        let saved: Option<PassSnapshot> = journal
            .map(|journal| journal.load(&part))
            .transpose()?
            .flatten();
        let mut state = if let Some(saved) = saved {
            saved.restore(env, &pilot)?;
            progress.stage(&format!(
                "{}: restore committed step {}",
                pilot.name(),
                saved.steps
            ));
            if saved.done {
                self.check_execution()?;
                return Ok(saved.played(env, true));
            }
            saved
        } else {
            progress.stage(&format!("{}: reset episode", pilot.name()));
            let obs = env.reset(self.seed);
            let state = PassSnapshot::start(env, pilot, Optics::from_global(&obs.global))?;
            if let Some(journal) = journal {
                journal.save(&part, &state)?;
            }
            state
        };
        let elapsed_before = state.elapsed_seconds;
        let started = Instant::now();
        let mut committed = Instant::now();
        loop {
            self.check_execution()?;
            self.play_step(env, &mut state, scratch, progress, journal)?;
            self.check_execution()?;
            state.elapsed_seconds = elapsed_before + started.elapsed().as_secs_f64();
            if let Some(journal) = journal
                && commit_due(&state, committed.elapsed())
            {
                state.episode = env.snapshot_episode()?;
                progress.stage(&format!(
                    "{} step {}: commit recovery",
                    state.pilot.name(),
                    state.steps
                ));
                journal.save(&part, &state)?;
                committed = Instant::now();
            }
            if state.done {
                return Ok(state.played(env, false));
            }
        }
    }

    fn play_step(
        &self,
        env: &mut EnvKind,
        state: &mut PassSnapshot,
        scratch: &mut InferenceScratch,
        progress: &CaseProgress<'_>,
        journal: Option<&CaseJournal>,
    ) -> Result<(), RunnerError> {
        let obs = env.build_observation();
        let max_step = env.max_step_deg();
        let oracle = matches!(state.pilot, Pilot::Oracle);
        let proposal = if oracle {
            if self.execution.is_some() {
                "host search control; CUDA geometry/optics"
            } else {
                "CPU search/geometry/optics"
            }
        } else {
            self.backend
        };
        progress.stage(&format!(
            "{} step {}: {proposal}",
            state.pilot.name(),
            state.steps + 1
        ));
        let (mut action, teacher_evaluations) = if oracle {
            self.recovered_oracle_action(env, max_step, state.steps, progress, journal)?
        } else {
            self.propose(env, &obs, &mut state.pilot, scratch, progress)?
        };
        self.check_execution()?;
        if !oracle {
            GemstoneSetAttentionNetwork::project_actions(
                &mut action,
                &obs.tiers,
                obs.global.target_critical_angle_deg,
                max_step,
            );
        }
        let free = obs.free_tier_mask();
        let count = free.iter().filter(|&&movable| movable).count();
        if count > 0 {
            let squared: f64 = action
                .deltas_deg
                .iter()
                .zip(&free)
                .filter(|(_, movable)| **movable)
                .map(|(delta, _)| delta * delta)
                .sum();
            state.action_rms_sum += (squared / count as f64).sqrt();
        }
        // Preserve the teacher's measured action at unit scale. The ordinary model/noise
        // passes retain the original first-accepted line-search and authoritative meet gate.
        let scales: &[f64] = if oracle {
            &[1.0]
        } else {
            &self.cfg.line_search
        };
        let backend = if self.execution.is_some() {
            "CUDA"
        } else {
            "CPU"
        };
        progress.stage(&format!(
            "{} step {}: geometry and optical scoring on {backend}",
            state.pilot.name(),
            state.steps + 1,
        ));
        let step = work::step(env, &action, scales, progress.tail);
        state.steps += 1;
        state.evaluations += teacher_evaluations + usize::from(step.info.evaluations);
        state.accepted += usize::from(step.info.scale.is_some());
        if let Some(reason) = &step.info.rejection {
            *state.rejections.entry(format!("{reason:?}")).or_insert(0) += 1;
        }
        state.done = step.terminated || step.truncated || (oracle && step.info.scale.is_none());
        Ok(())
    }

    fn recovered_oracle_action(
        &self,
        env: &EnvKind,
        max_step: f64,
        steps: usize,
        progress: &CaseProgress<'_>,
        journal: Option<&CaseJournal>,
    ) -> Result<(ContinuousTierAction, usize), RunnerError> {
        let Some(journal) = journal else {
            return Ok(self.oracle_action(env, max_step, progress));
        };
        // A new accepted state must never consume an earlier state's teacher continuation.
        let part = format!("oracle-teacher-{steps:06}");
        let saved = journal.load(&part)?;
        let h = self.cfg.label_h_deg.unwrap_or(max_step);
        // The search hands every snapshot over one at a time, newest last; each holds all
        // completed units, so a skipped one is contained in the next written one.
        let last_commit = Mutex::new(None::<Instant>);
        let label = work::oracle_with_recovery(
            env,
            h,
            &self.cfg.line_search,
            progress.tail,
            saved,
            &|snapshot| {
                let mut last = last_commit
                    .lock()
                    .map_err(|_| "teacher commit clock poisoned".to_owned())?;
                let units = snapshot.completed_units();
                if last.is_some_and(|at| at.elapsed() < COMMIT_INTERVAL) {
                    progress.stage(&format!(
                        "oracle step {}: {units} teacher units done",
                        steps + 1
                    ));
                    return Ok(());
                }
                journal
                    .save(&part, snapshot)
                    .map_err(|error| error.to_string())?;
                *last = Some(Instant::now());
                progress.stage(&format!(
                    "oracle step {}: {units} teacher units saved",
                    steps + 1
                ));
                Ok(())
            },
        )
        .map_err(RunnerError::Data)?;
        let raw = label
            .action_target
            .unwrap_or_else(|| vec![0.0; label.tiers.len()]);
        Ok((action_from_direction(&raw, max_step), label.evaluations))
    }
}

#[cfg(test)]
#[path = "runner_eval_episode_tests.rs"]
mod tests;
