//! Versioned, field-level recovery identities of the run configuration and the corpus.
//!
//! The legacy identities are `Debug` strings of whole structs, so any code change (a new field,
//! a renamed field, a changed default, a reworded skip reason) refuses every saved run with a
//! misleading "changed" error. These identities name their fields explicitly with stable
//! spellings: a field enters only by being listed here, a `revision` bump marks a deliberate
//! change of meaning, and a refused resume names the fields that differ. Recovery bundles store
//! both forms and a resume is accepted when either one matches.
//!
//! Adding a field to a list: give it the value that reproduces the behaviour before the field
//! existed in the list's `ADDED_*` table, so bundles saved without it still match while that
//! value is in effect. Changing what a listed field means: bump the list's revision.

use std::{collections::BTreeMap, fmt};

use pleochroma_ai::{
    model::TrainingMode,
    training::{ImitationConfig, LrSchedule, PpoConfig},
};
use pleochroma_cut_core::optimize::ObjectiveFidelity;
use serde::{Deserialize, Serialize};

use super::{EvalFidelity, RunConfig};
use crate::dataset::{CORPUS_IDENTITY_REVISION, DatasetSplit};

/// Revision of the configuration field list ([`config_identity`]).
const CONFIG_IDENTITY_REVISION: u32 = 1;

/// Configuration fields listed after revision 1, each with the value that reproduces the
/// behaviour of runs saved before it was listed.
pub(super) const ADDED_CONFIG_FIELDS: &[(&str, &str)] = &[];

/// Corpus fields listed after revision 1 (see [`ADDED_CONFIG_FIELDS`]).
pub(super) const ADDED_CORPUS_FIELDS: &[(&str, &str)] = &[];

/// Changes listed in an error message before the rest are only counted.
const SHOWN_CHANGES: usize = 8;

/// A field-level identity: the revision of its field list and every field's canonical value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SemanticIdentity {
    /// Revision of the field list and of the meaning of its values.
    pub(super) revision: u32,
    /// Field path to canonical value.
    pub(super) fields: BTreeMap<String, String>,
}

impl SemanticIdentity {
    /// What differs between this saved identity and the `current` one, one line per field
    /// (`field: saved -> current`); empty when they match. A field the saved identity lacks is
    /// accepted while `current` holds the value `added` records for it.
    pub(super) fn changes(&self, current: &Self, added: &[(&str, &str)]) -> Vec<String> {
        if self.revision != current.revision {
            return vec![format!(
                "identity revision: {} -> {} (the meaning of the recorded fields changed)",
                self.revision, current.revision
            )];
        }
        let mut changes = Vec::new();
        for (field, value) in &current.fields {
            match self.fields.get(field) {
                Some(saved) if saved == value => {}
                Some(saved) => changes.push(format!("{field}: {saved} -> {value}")),
                None if added
                    .iter()
                    .any(|&(name, default)| name == field && default == value) => {}
                None => changes.push(format!("{field}: (not recorded) -> {value}")),
            }
        }
        for (field, saved) in &self.fields {
            if !current.fields.contains_key(field) {
                changes.push(format!("{field}: {saved} -> (no longer recorded)"));
            }
        }
        changes
    }
}

/// `changes` for an error message: the first few, then how many more.
pub(super) fn describe_changes(changes: &[String]) -> String {
    let shown = changes
        .iter()
        .take(SHOWN_CHANGES)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("; ");
    match changes.len().checked_sub(SHOWN_CHANGES) {
        Some(more) if more > 0 => format!("{shown}; and {more} more"),
        _ => shown,
    }
}

/// A float in its shortest round-trip spelling (exact, and stable across platforms).
fn float<T: fmt::Debug + Copy>(value: T) -> String {
    format!("{value:?}")
}

fn optional(value: Option<String>) -> String {
    value.unwrap_or_else(|| "none".to_owned())
}

fn schedule(value: LrSchedule) -> String {
    match value {
        LrSchedule::Constant => "constant".to_owned(),
        LrSchedule::Linear { total_updates } => format!("linear:{total_updates}"),
    }
}

/// Field name and canonical value.
type Field = (&'static str, String);

/// Every PPO setting (PPO runs are resumed with their own step size).
fn ppo_fields(ppo: &PpoConfig) -> [Field; 11] {
    let PpoConfig {
        learning_rate,
        lr_schedule,
        gamma,
        gae_lambda,
        clip_epsilon,
        value_loss_coeff,
        entropy_coeff,
        epochs_per_batch,
        mini_batch_size,
        max_grad_norm,
        target_kl,
    } = ppo;
    [
        ("ppo.learning_rate", float(learning_rate)),
        ("ppo.lr_schedule", schedule(*lr_schedule)),
        ("ppo.gamma", float(gamma)),
        ("ppo.gae_lambda", float(gae_lambda)),
        ("ppo.clip_epsilon", float(clip_epsilon)),
        ("ppo.value_loss_coeff", float(value_loss_coeff)),
        ("ppo.entropy_coeff", float(entropy_coeff)),
        ("ppo.epochs_per_batch", epochs_per_batch.to_string()),
        ("ppo.mini_batch_size", mini_batch_size.to_string()),
        ("ppo.max_grad_norm", float(max_grad_norm)),
        ("ppo.target_kl", float(target_kl)),
    ]
}

/// The imitation settings except the learning rate and the gradient-norm cap, which a resume
/// may override explicitly.
fn imitation_fields(imitation: &ImitationConfig) -> [Field; 7] {
    let ImitationConfig {
        learning_rate: _,
        lr_schedule,
        mini_batch_size,
        epochs_per_round,
        updates_per_round,
        max_grad_norm: _,
        cosine_coeff,
        action_coeff,
        gain_coeff,
    } = imitation;
    [
        ("imitation.lr_schedule", schedule(*lr_schedule)),
        ("imitation.mini_batch_size", mini_batch_size.to_string()),
        ("imitation.epochs_per_round", epochs_per_round.to_string()),
        ("imitation.updates_per_round", updates_per_round.to_string()),
        ("imitation.cosine_coeff", float(cosine_coeff)),
        ("imitation.action_coeff", float(action_coeff)),
        ("imitation.gain_coeff", float(gain_coeff)),
    ]
}

const fn objective_fidelity(fidelity: ObjectiveFidelity) -> &'static str {
    match fidelity {
        ObjectiveFidelity::Fast => "fast",
        ObjectiveFidelity::Full => "full",
    }
}

const fn eval_fidelity(fidelity: EvalFidelity) -> &'static str {
    match fidelity {
        EvalFidelity::Fast => "fast",
        EvalFidelity::Full => "full",
    }
}

const fn mode(task: TrainingMode) -> &'static str {
    match task {
        TrainingMode::Optimizer => "optimizer",
        TrainingMode::Retarget => "retarget",
    }
}

/// The field-level identity of `cfg`. Every field is destructured, so a new configuration field
/// does not compile until it is either listed here or explicitly ignored.
///
/// Ignored: the device (a resume may move to other hardware) and the imitation learning rate
/// and gradient-norm cap (explicit resume overrides of the step size).
pub(super) fn config_identity(cfg: &RunConfig) -> SemanticIdentity {
    let RunConfig {
        mode: task,
        tier,
        device: _,
        workers,
        rollout_size,
        ppo,
        corpus_mode,
        universal_spectrum,
        fixed_target,
        seed,
        validation_designs,
        validation_every,
        max_step_deg,
        train_fidelity,
        full_fidelity_frac,
        validation_fidelity,
        objective_sweep,
        classical_budget,
        algo,
        imitation,
        label_every,
        label_h_deg,
        label_capacity,
        label_memory_mib,
        explore_std,
        max_rejected_steps,
        line_search,
        objectives,
        same_material_frac,
    } = cfg;
    let fields = [
        ("mode", mode(*task).to_owned()),
        ("tier", tier.code_str().to_owned()),
        ("workers", workers.to_string()),
        ("rollout_size", rollout_size.to_string()),
        ("corpus_mode", corpus_mode.to_string()),
        ("universal_spectrum", universal_spectrum.to_string()),
        ("fixed_target", fixed_target.clone()),
        ("seed", seed.to_string()),
        ("validation_designs", validation_designs.to_string()),
        ("validation_every", validation_every.to_string()),
        ("max_step_deg", optional(max_step_deg.map(float))),
        (
            "train_fidelity",
            objective_fidelity(*train_fidelity).to_owned(),
        ),
        ("full_fidelity_frac", float(full_fidelity_frac)),
        (
            "validation_fidelity",
            eval_fidelity(*validation_fidelity).to_owned(),
        ),
        ("objective_sweep", objective_sweep.to_string()),
        (
            "classical_budget",
            optional(classical_budget.map(|b| b.to_string())),
        ),
        ("algo", algo.name().to_owned()),
        ("label_every", label_every.to_string()),
        ("label_h_deg", optional(label_h_deg.map(float))),
        ("label_capacity", label_capacity.to_string()),
        ("label_memory_mib", label_memory_mib.to_string()),
        ("explore_std", float(explore_std)),
        ("max_rejected_steps", max_rejected_steps.to_string()),
        (
            "line_search",
            line_search.iter().map(float).collect::<Vec<_>>().join(","),
        ),
        ("objectives", objectives.name().to_owned()),
        ("same_material_frac", float(same_material_frac)),
    ];
    SemanticIdentity {
        revision: CONFIG_IDENTITY_REVISION,
        fields: fields
            .into_iter()
            .chain(ppo_fields(ppo))
            .chain(imitation_fields(imitation))
            .map(|(field, value)| (field.to_owned(), value))
            .collect(),
    }
}

/// The field-level identity of the corpus (see `DatasetSplit::semantic_identity_fields`).
pub(super) fn corpus_identity(split: &DatasetSplit) -> SemanticIdentity {
    SemanticIdentity {
        revision: CORPUS_IDENTITY_REVISION,
        fields: split.semantic_identity_fields(),
    }
}

/// The imitation learning rate and gradient-norm cap a legacy `Debug` configuration identity
/// recorded, read from its `imitation: ImitationConfig { .. }` part (`None` when absent).
pub(super) fn legacy_imitation_step_size(saved: &str) -> (Option<f32>, Option<f32>) {
    let Some(start) = saved.find("imitation: ImitationConfig { ") else {
        return (None, None);
    };
    let imitation = &saved[start..];
    let value = |field: &str| -> Option<f32> {
        let key = format!("{field}: ");
        let from = imitation.find(&key)? + key.len();
        let text = &imitation[from..];
        let end = text.find([',', ' '])?;
        text[..end].parse().ok()
    };
    (value("learning_rate"), value("max_grad_norm"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(fields: &[(&str, &str)]) -> SemanticIdentity {
        SemanticIdentity {
            revision: 1,
            fields: fields
                .iter()
                .map(|&(field, value)| (field.to_owned(), value.to_owned()))
                .collect(),
        }
    }

    #[test]
    fn changes_name_every_differing_added_and_removed_field() {
        let saved = identity(&[("a", "1"), ("b", "2"), ("gone", "x")]);
        let current = identity(&[("a", "1"), ("b", "3"), ("new", "y")]);
        assert_eq!(
            saved.changes(&current, &[]),
            vec![
                "b: 2 -> 3".to_owned(),
                "new: (not recorded) -> y".to_owned(),
                "gone: x -> (no longer recorded)".to_owned(),
            ]
        );
        assert!(saved.changes(&saved, &[]).is_empty());
    }

    #[test]
    fn a_newly_listed_field_matches_only_at_its_recorded_default() {
        let saved = identity(&[("a", "1")]);
        let at_default = identity(&[("a", "1"), ("new", "off")]);
        let changed = identity(&[("a", "1"), ("new", "on")]);
        assert!(saved.changes(&at_default, &[("new", "off")]).is_empty());
        assert_eq!(saved.changes(&changed, &[("new", "off")]).len(), 1);
    }

    #[test]
    fn a_revision_bump_refuses_whatever_the_fields() {
        let saved = identity(&[("a", "1")]);
        let mut current = saved.clone();
        current.revision = 2;
        assert_eq!(saved.changes(&current, &[]).len(), 1);
    }

    #[test]
    fn long_change_lists_are_cut_in_messages() {
        let changes: Vec<String> = (0..10).map(|i| format!("f{i}: 0 -> 1")).collect();
        let text = describe_changes(&changes);
        assert!(text.starts_with("f0: 0 -> 1; "));
        assert!(text.ends_with("; and 2 more"));
        assert_eq!(describe_changes(&changes[..1]), "f0: 0 -> 1");
    }

    #[test]
    fn the_legacy_step_size_is_read_from_the_imitation_part_only() {
        let saved = "RunConfig { ppo: PpoConfig { learning_rate: 0.0001, max_grad_norm: 0.5 }, \
                     imitation: ImitationConfig { learning_rate: 0.0003, lr_schedule: Linear { \
                     total_updates: 7 }, max_grad_norm: 1e-7, cosine_coeff: 0.5 }, seed: 42 }";
        assert_eq!(legacy_imitation_step_size(saved), (Some(3e-4), Some(1e-7)));
        assert_eq!(
            legacy_imitation_step_size("RunConfig { seed: 42 }"),
            (None, None)
        );
    }
}
