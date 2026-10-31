//! Corpus provenance and deterministic held-out cohort coverage.

use pleochroma::optics::materials::GemMaterial;
use pleochroma_ai::env::material_features::material_scope_notes;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::{DatasetSplit, connected_design_groups, normalized_geometry_key};

/// Counts for retained files, not assertions of model competence or measured accuracy.
#[derive(Debug, Clone, Serialize)]
pub struct DatasetCoverage {
    pub asc: usize,
    pub native: usize,
    pub custom_native_materials: usize,
    pub concave: usize,
    pub relations: usize,
    pub dimensional_targets: usize,
    pub physical_size: usize,
    pub authored_axis: usize,
    pub geometric_families: usize,
    pub tier_bands: BTreeMap<String, usize>,
    pub native_materials: BTreeMap<String, usize>,
    pub analytical_scope_notes: Vec<String>,
    /// Counts per split reveal whether a supported capability actually appears in training.
    pub split_capabilities: BTreeMap<String, BTreeMap<String, usize>>,
}

impl DatasetSplit {
    /// Describe actual retained training data and limitations of its analytical labels.
    #[must_use]
    pub fn coverage(&self) -> super::DatasetCoverage {
        let mut result = DatasetCoverage {
            asc: 0,
            native: 0,
            custom_native_materials: 0,
            concave: 0,
            relations: 0,
            dimensional_targets: 0,
            physical_size: 0,
            authored_axis: 0,
            geometric_families: 0,
            tier_bands: BTreeMap::new(),
            native_materials: BTreeMap::new(),
            analytical_scope_notes: Vec::new(),
            split_capabilities: self.split_capabilities(),
        };
        let mut notes = BTreeSet::new();
        for entry in &self.entries {
            let d = &entry.design;
            result.concave += usize::from(!d.concave_tiers.is_empty());
            result.relations += usize::from(!d.tier_relations.is_empty());
            result.dimensional_targets += usize::from(!d.tier_targets.is_empty());
            result.physical_size += usize::from(d.girdle_diameter_mm.is_some());
            result.authored_axis += usize::from(
                d.render_effects.c_axis_override_enabled || d.crystal_axis_plan.is_some(),
            );
            *result
                .tier_bands
                .entry(tier_band(d.tiers.len()).to_owned())
                .or_default() += 1;
            if let Some(material) = &entry.authored_material {
                result.native += 1;
                let name = d.material.name.as_deref().unwrap_or("Authored index");
                *result.native_materials.entry(name.to_owned()).or_default() += 1;
                result.custom_native_materials += usize::from(
                    d.material.name.is_some()
                        && !GemMaterial::built_in()
                            .iter()
                            .any(|g| g.name.eq_ignore_ascii_case(name)),
                );
                notes.extend(material_scope_notes(d, &material.gem));
            } else {
                result.asc += 1;
            }
        }
        // Catalog augmentation also needs honest scope disclosure even when a native corpus
        // happens not to contain these effects itself.
        if let Some(entry) = self.entries.first() {
            for gem in GemMaterial::built_in() {
                notes.extend(material_scope_notes(&entry.design, gem));
            }
        }
        result.analytical_scope_notes = notes.into_iter().map(str::to_owned).collect();
        let keys: Vec<_> = self
            .entries
            .iter()
            .map(|e| normalized_geometry_key(&e.design))
            .collect();
        result.geometric_families = connected_design_groups(&self.entries, &keys).len();
        result
    }

    fn split_capabilities(&self) -> BTreeMap<String, BTreeMap<String, usize>> {
        [
            ("train", &self.train),
            ("validation", &self.val),
            ("test", &self.test),
        ]
        .into_iter()
        .map(|(name, indices)| {
            let mut counts = BTreeMap::from([
                ("designs".to_owned(), indices.len()),
                ("native".to_owned(), 0),
                ("relations".to_owned(), 0),
                ("dimensional_targets".to_owned(), 0),
                ("concave".to_owned(), 0),
                ("authored_axis".to_owned(), 0),
            ]);
            for &index in indices {
                let entry = &self.entries[index];
                let design = &entry.design;
                for (key, present) in [
                    ("native", entry.authored_material.is_some()),
                    ("relations", !design.tier_relations.is_empty()),
                    ("dimensional_targets", !design.tier_targets.is_empty()),
                    ("concave", !design.concave_tiers.is_empty()),
                    (
                        "authored_axis",
                        design.render_effects.c_axis_override_enabled
                            || design.crystal_axis_plan.is_some(),
                    ),
                ] {
                    *counts.entry(key.to_owned()).or_default() += usize::from(present);
                }
            }
            (name.to_owned(), counts)
        })
        .collect()
    }

    /// A reproducible coverage-oriented cohort drawn only from the existing validation split.
    ///
    /// Round-robin strata cover format, tier band, concavity and authored relation programs;
    /// this intentionally is not a population-unbiased random estimate.
    #[must_use]
    pub fn validation_cohort(&self, limit: usize) -> Vec<usize> {
        let mut strata = BTreeMap::<_, VecDeque<usize>>::new();
        for &index in &self.val {
            let entry = &self.entries[index];
            let d = &entry.design;
            let key = (
                entry.authored_material.is_some(),
                tier_band(d.tiers.len()),
                !d.concave_tiers.is_empty(),
                !d.tier_relations.is_empty(),
            );
            strata.entry(key).or_default().push_back(index);
        }
        let mut result = Vec::with_capacity(limit.min(self.val.len()));
        while result.len() < limit && strata.values().any(|v| !v.is_empty()) {
            for queue in strata.values_mut() {
                if let Some(index) = queue.pop_front() {
                    result.push(index);
                }
                if result.len() == limit {
                    break;
                }
            }
        }
        result
    }
}

fn tier_band(tiers: usize) -> &'static str {
    match tiers {
        0..=8 => "1-8",
        9..=16 => "9-16",
        17..=32 => "17-32",
        33..=64 => "33-64",
        _ => "65-128",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validation_cohort_never_borrows_from_training_and_is_repeatable() {
        let split = DatasetSplit::fixture();
        assert!(split.validation_cohort(0).is_empty());
        assert_eq!(split.validation_cohort(10), split.val);
        assert_eq!(split.validation_cohort(1), split.validation_cohort(1));
        let mut empty = split;
        empty.val.clear();
        assert!(empty.validation_cohort(8).is_empty());
    }

    #[test]
    fn capability_report_distinguishes_training_from_held_out_support() {
        let mut split = DatasetSplit::fixture();
        split.train.clear();
        let coverage = split.coverage();
        assert_eq!(coverage.split_capabilities["train"]["designs"], 0);
        assert_eq!(coverage.split_capabilities["train"]["concave"], 0);
        assert_eq!(
            coverage.split_capabilities["validation"]["designs"],
            split.val.len()
        );
        assert_eq!(
            coverage.split_capabilities["test"]["designs"],
            split.test.len()
        );
    }
}
