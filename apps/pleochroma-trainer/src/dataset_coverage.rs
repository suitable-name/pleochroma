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
    /// Designs with an effective physical size, including deterministic augmentation.
    pub physical_size: usize,
    /// Designs whose physical size was supplied by the source.
    pub authored_physical_size: usize,
    /// Designs assigned a physical size because their source did not specify one.
    pub augmented_physical_size: usize,
    /// Entries eligible for source-material sampling, not a count of sampled episodes.
    pub augmented_source_materials: usize,
    pub authored_axis: usize,
    pub geometric_families: usize,
    pub tier_bands: BTreeMap<String, usize>,
    pub native_materials: BTreeMap<String, usize>,
    pub analytical_scope_notes: Vec<String>,
    /// Counts per split reveal whether a supported capability actually appears in training.
    pub split_capabilities: BTreeMap<String, BTreeMap<String, usize>>,
}

impl DatasetSplit {
    /// Counts connected source/geometry families represented by these entry indices.
    #[must_use]
    pub fn distinct_families(&self, indices: &[usize]) -> usize {
        let selected: std::collections::BTreeSet<_> = indices.iter().copied().collect();
        let keys: Vec<_> = self
            .entries
            .iter()
            .map(|entry| normalized_geometry_key(&entry.design))
            .collect();
        connected_design_groups(&self.entries, &keys)
            .into_iter()
            .filter(|(_, members)| members.iter().any(|index| selected.contains(index)))
            .count()
    }

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
            authored_physical_size: 0,
            augmented_physical_size: 0,
            augmented_source_materials: 0,
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
            result.authored_physical_size +=
                usize::from(d.girdle_diameter_mm.is_some() && !entry.physical_size_augmented);
            result.augmented_physical_size +=
                usize::from(d.girdle_diameter_mm.is_some() && entry.physical_size_augmented);
            result.augmented_source_materials += usize::from(entry.authored_material.is_none());
            result.native += usize::from(entry.is_native());
            result.asc += usize::from(!entry.is_native());
            result.authored_axis += usize::from(
                d.render_effects.c_axis_override_enabled || d.crystal_axis_plan.is_some(),
            );
            *result
                .tier_bands
                .entry(tier_band(d.tiers.len()).to_owned())
                .or_default() += 1;
            if let Some(material) = &entry.authored_material {
                let name = d.material.name.as_deref().unwrap_or("Authored index");
                *result.native_materials.entry(name.to_owned()).or_default() += 1;
                result.custom_native_materials += usize::from(
                    d.material.name.is_some()
                        && !GemMaterial::built_in()
                            .iter()
                            .any(|g| g.name.eq_ignore_ascii_case(name)),
                );
                notes.extend(material_scope_notes(d, &material.gem));
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
                ("physical_size".to_owned(), 0),
                ("authored_physical_size".to_owned(), 0),
                ("augmented_physical_size".to_owned(), 0),
                ("authored_source_materials".to_owned(), 0),
                ("augmented_source_materials".to_owned(), 0),
            ]);
            for &index in indices {
                let entry = &self.entries[index];
                let design = &entry.design;
                for (key, present) in [
                    ("native", entry.is_native()),
                    ("relations", !design.tier_relations.is_empty()),
                    ("dimensional_targets", !design.tier_targets.is_empty()),
                    ("concave", !design.concave_tiers.is_empty()),
                    ("physical_size", design.girdle_diameter_mm.is_some()),
                    (
                        "authored_physical_size",
                        design.girdle_diameter_mm.is_some() && !entry.physical_size_augmented,
                    ),
                    (
                        "augmented_physical_size",
                        design.girdle_diameter_mm.is_some() && entry.physical_size_augmented,
                    ),
                    (
                        "authored_source_materials",
                        entry.authored_material.is_some(),
                    ),
                    (
                        "augmented_source_materials",
                        entry.authored_material.is_none(),
                    ),
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
                entry.is_native(),
                tier_band(d.tiers.len()),
                !d.concave_tiers.is_empty(),
                !d.tier_relations.is_empty(),
            );
            strata.entry(key).or_default().push_back(index);
        }
        let mut result = Vec::with_capacity(limit.min(self.val.len()));
        while strata.values().any(|v| !v.is_empty()) {
            for queue in strata.values_mut() {
                if let Some(index) = queue.pop_front() {
                    result.push(index);
                }
            }
        }
        // Pick one representative per leakage-resistant family before revisiting any family.
        // Preserve the stratum order within both passes so the cohort stays deterministic.
        let keys: Vec<_> = self
            .entries
            .iter()
            .map(|entry| normalized_geometry_key(&entry.design))
            .collect();
        let mut family_by_entry = vec![0; self.entries.len()];
        for (family, (_, members)) in connected_design_groups(&self.entries, &keys)
            .into_iter()
            .enumerate()
        {
            for index in members {
                family_by_entry[index] = family;
            }
        }
        let mut seen = BTreeSet::new();
        let mut unique = Vec::new();
        let mut repeated = Vec::new();
        for index in result {
            if seen.insert(family_by_entry[index]) {
                unique.push(index);
            } else {
                repeated.push(index);
            }
        }
        unique.extend(repeated);
        unique.truncate(limit);
        unique
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
    fn native_format_and_augmentation_coverage_are_independent_of_authored_material() {
        let mut split = DatasetSplit::fixture();
        let mut authored = split.entries[0].clone();
        authored.filename = "authored.PSDD".to_owned();
        authored.design.girdle_diameter_mm = Some(9.25);
        authored.physical_size_augmented = false;
        authored.authored_material =
            Some(pleochroma_ai::env::design_source_material(&authored.design));
        let mut unknown_native = authored.clone();
        unknown_native.filename = "unknown.psdd".to_owned();
        unknown_native.authored_material = None;
        unknown_native.design.girdle_diameter_mm = Some(7.0);
        unknown_native.physical_size_augmented = true;
        let mut asc = unknown_native.clone();
        asc.filename = "legacy.asc".to_owned();
        split.entries = vec![authored, unknown_native, asc];
        split.train = vec![0, 1];
        split.val = vec![2];
        split.test.clear();

        let coverage = split.coverage();
        assert_eq!((coverage.native, coverage.asc), (2, 1));
        assert_eq!(coverage.physical_size, 3);
        assert_eq!(coverage.authored_physical_size, 1);
        assert_eq!(coverage.augmented_physical_size, 2);
        assert_eq!(coverage.augmented_source_materials, 2);
        assert_eq!(coverage.native_materials.values().sum::<usize>(), 1);
        let training = &coverage.split_capabilities["train"];
        assert_eq!(training["native"], 2);
        assert_eq!(training["physical_size"], 2);
        assert_eq!(training["authored_physical_size"], 1);
        assert_eq!(training["augmented_physical_size"], 1);
        assert_eq!(training["authored_source_materials"], 1);
        assert_eq!(training["augmented_source_materials"], 1);
        assert_eq!(coverage.split_capabilities["validation"]["native"], 0);
        assert_eq!(
            coverage.split_capabilities["validation"]["augmented_physical_size"],
            1
        );
        assert_eq!(
            coverage.split_capabilities["test"]["augmented_source_materials"],
            0
        );
    }

    #[test]
    fn native_material_presence_does_not_create_another_format_stratum() {
        let mut split = DatasetSplit::fixture();
        let mut authored = split.entries[0].clone();
        authored.filename = "authored.psdd".to_owned();
        authored.authored_material =
            Some(pleochroma_ai::env::design_source_material(&authored.design));
        let mut unknown = authored.clone();
        unknown.filename = "unknown.psdd".to_owned();
        unknown.authored_material = None;
        split.entries = vec![authored.clone(), unknown, authored];
        split.train.clear();
        split.val = vec![0, 1, 2];
        split.test.clear();

        // All three share one geometry family and one native-format stratum, so source
        // material provenance must not reorder their existing validation order.
        assert_eq!(split.validation_cohort(3), vec![0, 1, 2]);
    }

    #[test]
    fn validation_cohort_never_borrows_from_training_and_is_repeatable() {
        let split = DatasetSplit::fixture();
        assert!(split.validation_cohort(0).is_empty());
        let mut all = split.validation_cohort(10);
        all.sort_unstable();
        let mut expected = split.val.clone();
        expected.sort_unstable();
        assert_eq!(all, expected);
        assert_eq!(split.validation_cohort(1), split.validation_cohort(1));
        let mut empty = split;
        empty.val.clear();
        assert!(empty.validation_cohort(8).is_empty());
    }

    #[test]
    fn cohort_prefers_distinct_connected_families_before_contexts() {
        let mut split = DatasetSplit::fixture();
        let first = split.val[0];
        let mut duplicate = split.entries[first].clone();
        duplicate.id = split.entries[first].id;
        split.entries.push(duplicate);
        split.val.insert(1, split.entries.len() - 1);
        let keys: Vec<_> = split
            .entries
            .iter()
            .map(|e| normalized_geometry_key(&e.design))
            .collect();
        let groups = connected_design_groups(&split.entries, &keys);
        let cohort = split.validation_cohort(split.val.len());
        let families: Vec<_> = cohort
            .iter()
            .map(|index| {
                groups
                    .iter()
                    .find(|(_, members)| members.contains(index))
                    .unwrap()
                    .0
            })
            .collect();
        let distinct = families.iter().copied().collect::<BTreeSet<_>>().len();
        assert_eq!(
            families[..distinct]
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .len(),
            distinct
        );
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
