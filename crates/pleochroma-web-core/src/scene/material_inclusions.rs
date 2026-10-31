//! [`MaterialInclusionsSpec`]: the oriented inclusions and play-of-color a custom MATERIAL
//! carries, as plain wire data.
//!
//! A custom material stores its inclusion families and its play-of-color structure with the
//! material; opening a design resolves them into `GemMaterial::inclusions`, which is
//! `#[serde(skip)]` (like the zones), so the built materials of
//! [`MaterialSpec::custom_materials`](super::MaterialSpec::custom_materials) reach a Worker
//! without them. The design's own choice ([`EffectsSpec`](super::EffectsSpec)) is looked up by the
//! material NAME, which a custom name does not match. This record carries the resolved set beside
//! the material instead (`MaterialSpec::custom_material_inclusions`, parallel to
//! `custom_materials`), so a Worker traces the silk and schiller the page traces.
//!
//! `InclusionSet` has private fields, among them a length that indexes a fixed array, so it is
//! not read straight off the wire: the families and the structure travel as plain values, are
//! checked by [`MaterialInclusionsSpec::validate`], and the set is rebuilt with
//! [`MaterialInclusionsSpec::to_set`] (which sanitises every family again).
//!
//! # Twin of the net record
//!
//! This is the web twin of `pleochroma_net::scene::material_inclusions::MaterialInclusions`
//! (`crates/pleochroma-net/src/scene/material_inclusions.rs`), which web-core cannot depend on:
//! same shape, same validation rules, and that file is the source of truth -- keep the two in
//! lock-step. `pleochroma-net` is not a dev-dependency of this crate, so the tests in
//! `scene/tests.rs` pin the rules by example (the accepted and refused values the net crate's own
//! tests use) rather than by calling the net type.
//!
//! One difference in what is recorded: the net record is taken from the RESOLVED material (after
//! the design's Crystal Axis turned the families), the web record from the page's custom material
//! as stored. An `InclusionSet`'s crystal frame is not part of the wire form, so a Worker cannot
//! turn these families with a design's own Crystal Axis override; they render in the frame the
//! material stored them in.

use pleochroma::optics::{
    inclusions::{InclusionSet, MAX_FAMILIES, ResolvedFamily, bragg::BraggStructure},
    materials::GemMaterial,
};
use serde::{Deserialize, Serialize};

use super::MaterialSpec;

/// The families (in the stone frame) and the Bragg domain structure of one material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialInclusionsSpec {
    /// The families, at most [`MAX_FAMILIES`], in the frame the material stored them in.
    pub families: Vec<ResolvedFamily>,
    /// The play-of-color structure (opal, labradorite), when the material has one.
    pub structure: Option<BraggStructure>,
}

impl MaterialInclusionsSpec {
    /// The record of `set`; `None` for a set that scatters nothing.
    #[must_use]
    pub fn of_set(set: &InclusionSet) -> Option<Self> {
        set.is_active().then(|| Self {
            families: set.families().to_vec(),
            structure: set.structure().copied(),
        })
    }

    /// The record of the inclusions `material` traces with (`GemMaterial::inclusions`), or
    /// `None` when it has none or they are not valid on the wire ([`Self::validate`]).
    #[must_use]
    pub fn of_material(material: &GemMaterial) -> Option<Self> {
        Self::of_set(material.oriented_inclusions()?).filter(|record| record.validate().is_ok())
    }

    /// The list for `MaterialSpec::custom_material_inclusions`: one entry per material of
    /// `materials`, or an empty list when none of them carries inclusions (the scene is then
    /// byte-for-byte the scene without the field's content).
    #[must_use]
    pub fn list_of(materials: &[GemMaterial]) -> Vec<Option<Self>> {
        let list: Vec<Option<Self>> = materials.iter().map(Self::of_material).collect();
        if list.iter().all(Option::is_none) {
            Vec::new()
        } else {
            list
        }
    }

    /// The set this record describes. The families are sanitised again, so a record that passed
    /// [`Self::validate`] cannot put an unusable family into the tracer.
    #[must_use]
    pub fn to_set(&self) -> InclusionSet {
        let set = InclusionSet::new(&self.families);
        // At most one structure: the fold applies it when present and leaves the set otherwise.
        self.structure
            .into_iter()
            .fold(set, InclusionSet::with_structure)
    }

    /// Refuses a record no honest sender produces.
    ///
    /// # Errors
    ///
    /// The first problem, as a sentence.
    pub fn validate(&self) -> Result<(), String> {
        if self.families.len() > MAX_FAMILIES {
            return Err(format!(
                "custom_material_inclusions has {} families (at most {MAX_FAMILIES})",
                self.families.len()
            ));
        }
        for family in &self.families {
            let finite = family.dir.is_finite()
                && family.sigma.is_finite()
                && family.lobe.is_finite()
                && family.rho.is_finite();
            if !finite || family.sigma < 0.0 {
                return Err(format!(
                    "custom_material_inclusions has an implausible family: {family:?}"
                ));
            }
        }
        self.structure.as_ref().map_or(Ok(()), validate_structure)
    }
}

impl MaterialSpec {
    /// Refuses a record no honest sender produces: an inclusion list whose length is neither
    /// zero nor the number of custom materials, or an entry that fails
    /// [`MaterialInclusionsSpec::validate`].
    ///
    /// # Errors
    ///
    /// The first problem, as a sentence.
    pub fn validate(&self) -> Result<(), String> {
        let inclusions = &self.custom_material_inclusions;
        if !inclusions.is_empty() && inclusions.len() != self.custom_materials.len() {
            return Err(format!(
                "custom_material_inclusions has {} entries for {} custom materials",
                inclusions.len(),
                self.custom_materials.len()
            ));
        }
        inclusions
            .iter()
            .flatten()
            .try_for_each(MaterialInclusionsSpec::validate)
    }
}

/// The ranges `BraggStructure::new` clamps to; a value outside them was not made by it.
fn validate_structure(structure: &BraggStructure) -> Result<(), String> {
    let in_range = |value: f32, lo: f32, hi: f32| value.is_finite() && (lo..=hi).contains(&value);
    let ok = in_range(structure.cell_size, 0.01, 5.0)
        && in_range(structure.lambda_low_nm, 350.0, 900.0)
        && in_range(structure.lambda_high_nm, 350.0, 900.0)
        && structure.lambda_low_nm <= structure.lambda_high_nm
        && in_range(structure.purity, 0.01, 0.3)
        && in_range(structure.coverage, 0.0, 1.0)
        && in_range(structure.peak_reflectance, 0.01, 0.95)
        && in_range(structure.mosaic, 0.0, 0.5)
        && structure.normal.is_finite()
        && structure.normal.length() <= 1.001;
    if ok {
        Ok(())
    } else {
        Err(format!(
            "custom_material_inclusions.structure is implausible: {structure:?}"
        ))
    }
}
