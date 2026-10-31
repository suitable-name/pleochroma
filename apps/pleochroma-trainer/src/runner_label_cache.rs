//! One-entry exact cache for a slot's most recent deterministic teacher label.
//!
//! Environment material, objective, fidelity, and other scoring settings are fixed for the
//! lifetime of a slot. A new episode gets a new slot and therefore an empty cache.

use pleochroma_ai::env::{EnvError, TeacherLabel, recovery::DesignState};
use pleochroma_cut_core::Design;

#[derive(Default)]
pub(super) struct LabelCache {
    entry: Option<CacheEntry>,
}

struct CacheEntry {
    design: Design,
    probe_h_bits: u64,
    scale_bits: Vec<u64>,
    label: TeacherLabel,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct CacheSnapshot {
    design: DesignState,
    probe_h_bits: u64,
    scale_bits: Vec<u64>,
    label: TeacherLabel,
}

impl LabelCache {
    pub(super) fn snapshot(&self, base: &Design) -> Result<Option<CacheSnapshot>, EnvError> {
        self.entry
            .as_ref()
            .map(|entry| {
                Ok(CacheSnapshot {
                    design: DesignState::capture(base, &entry.design)?,
                    probe_h_bits: entry.probe_h_bits,
                    scale_bits: entry.scale_bits.clone(),
                    label: entry.label.clone(),
                })
            })
            .transpose()
    }

    pub(super) fn restore(
        snapshot: Option<CacheSnapshot>,
        base: &Design,
    ) -> Result<Self, EnvError> {
        let entry = snapshot
            .map(|snapshot| {
                if snapshot.label.tiers.len() != base.tiers.len() {
                    return Err(EnvError::InvalidConfig(
                        "saved label cache tier count differs",
                    ));
                }
                Ok(CacheEntry {
                    design: snapshot.design.apply(base)?,
                    probe_h_bits: snapshot.probe_h_bits,
                    scale_bits: snapshot.scale_bits,
                    label: snapshot.label,
                })
            })
            .transpose()?;
        Ok(Self { entry })
    }

    /// Returns an exact cached label. Evaluation cost is zero for a cache hit because the
    /// scorer was not called again; the stored label keeps its original cost for future hits.
    pub(super) fn get(
        &self,
        design: &Design,
        probe_h_deg: f64,
        scales: &[f64],
    ) -> Option<TeacherLabel> {
        let entry = self.entry.as_ref()?;
        if entry.design != *design
            || entry.probe_h_bits != probe_h_deg.to_bits()
            || entry.scale_bits.len() != scales.len()
            || !entry
                .scale_bits
                .iter()
                .zip(scales)
                .all(|(&bits, scale)| bits == scale.to_bits())
        {
            return None;
        }
        let mut label = entry.label.clone();
        label.evaluations = 0;
        Some(label)
    }

    /// Replaces the bounded slot-local entry with a label for the exact pre-step state.
    pub(super) fn put(
        &mut self,
        design: &Design,
        probe_h_deg: f64,
        scales: &[f64],
        label: TeacherLabel,
    ) {
        self.entry = Some(CacheEntry {
            design: design.clone(),
            probe_h_bits: probe_h_deg.to_bits(),
            scale_bits: scales.iter().map(|scale| scale.to_bits()).collect(),
            label,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::geometry::meet_solver::MeetConstraint;
    use pleochroma_ai::env::{OracleOutcome, TierSensitivity, assemble_label};
    use pleochroma_cut_core::{Design, PreformSpec, design::ConstraintTier};

    fn design(angle_deg: f64) -> Design {
        let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
        design.tiers.extend(ConstraintTier::step_series(
            "cache fixture",
            angle_deg,
            1.0,
            1,
            &[0.0],
            &MeetConstraint::ScaleReference(1.0),
        ));
        design
    }

    fn label() -> TeacherLabel {
        let mut measured = assemble_label(
            vec![Some(TierSensitivity {
                components: [1.0; 7],
                score: 2.0,
                forward_probe: true,
            })],
            3,
            OracleOutcome {
                gain: Some(0.5),
                scale: Some(0.25),
                evaluations: 2,
            },
        );
        measured.action_target = Some(vec![0.3]);
        measured
    }

    #[test]
    fn exact_state_and_probe_settings_hit_at_zero_repeat_cost() {
        let state = design(42.0);
        let scales = [1.0, 0.5, 0.25];
        let mut cache = LabelCache::default();
        assert!(cache.get(&state, 0.1, &scales).is_none());
        cache.put(&state, 0.1, &scales, label());
        let hit = cache.get(&state, 0.1, &scales).expect("exact cache hit");
        assert_eq!(hit.evaluations, 0);
        assert_eq!(hit.tiers, label().tiers);
        assert_eq!(hit.oracle_gain, label().oracle_gain);
        assert_eq!(hit.oracle_scale, label().oracle_scale);
        assert_eq!(hit.action_target, label().action_target);
    }

    #[test]
    fn design_probe_size_and_scale_bits_are_all_part_of_the_key() {
        let state = design(42.0);
        let changed = design(43.0);
        let scales = [1.0, 0.5];
        let mut cache = LabelCache::default();
        cache.put(&state, 0.1, &scales, label());
        assert!(cache.get(&changed, 0.1, &scales).is_none());
        assert!(cache.get(&state, 0.2, &scales).is_none());
        assert!(cache.get(&state, 0.1, &[1.0, 0.25]).is_none());
    }

    #[test]
    fn a_fresh_slot_starts_with_an_empty_cache() {
        let state = design(42.0);
        let scales = [1.0];
        let mut old_slot = LabelCache::default();
        old_slot.put(&state, 0.1, &scales, label());
        let new_slot = LabelCache::default();
        assert!(new_slot.get(&state, 0.1, &scales).is_none());
    }

    #[test]
    fn unchanged_angles_do_not_hide_a_changed_depth() {
        let state = design(42.0);
        let mut cache = LabelCache::default();
        cache.put(&state, 0.1, &[1.0], label());
        let mut changed = state.clone();
        changed.tiers[0].constraint = MeetConstraint::ScaleReference(0.9);
        assert!(cache.get(&changed, 0.1, &[1.0]).is_none());
    }

    #[test]
    fn unusable_labels_are_cached_without_inventing_targets() {
        let state = design(42.0);
        let missing = assemble_label(vec![None], 0, OracleOutcome::default());
        let mut cache = LabelCache::default();
        cache.put(&state, 0.1, &[1.0], missing.clone());
        assert_eq!(cache.get(&state, 0.1, &[1.0]), Some(missing));
    }
}
