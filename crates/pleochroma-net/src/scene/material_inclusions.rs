//! [`MaterialInclusions`]: the oriented inclusions and play-of-color a MATERIAL carries, as plain
//! wire data.
//!
//! A custom material stores its inclusion families and its play-of-color structure with the
//! material; the library row resolves them into `GemMaterial::inclusions`, which is
//! `#[serde(skip)]` (like the zones). The design's own choice (`SceneEffects::inclusions`,
//! `SceneEffects::structure`) is looked up by the material NAME, so it resolves nothing for a
//! custom name. This record carries the resolved set beside the material instead, so a worker
//! traces the silk and schiller the sender traces.
//!
//! `InclusionSet` has private fields, among them a length that indexes a fixed array, so it is
//! not read straight off the wire: the families and the structure travel as plain values, are
//! checked by [`MaterialInclusions::validate`], and the set is rebuilt with
//! [`MaterialInclusions::to_set`] (which sanitises every family again).

use pleochroma::optics::{
    inclusions::{InclusionSet, MAX_FAMILIES, ResolvedFamily, bragg::BraggStructure},
    materials::GemMaterial,
};

/// The families (in the stone frame) and the Bragg domain structure of one material.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MaterialInclusions {
    /// The families, at most [`MAX_FAMILIES`], already turned into the stone frame (the Crystal
    /// Axis of the design included).
    pub families: Vec<ResolvedFamily>,
    /// The play-of-color structure (opal, labradorite), when the material has one.
    pub structure: Option<BraggStructure>,
}

impl MaterialInclusions {
    /// The record of `set`; `None` for a set that scatters nothing.
    #[must_use]
    pub fn of_set(set: &InclusionSet) -> Option<Self> {
        set.is_active().then(|| Self {
            families: set.families().to_vec(),
            structure: set.structure().copied(),
        })
    }

    /// The record of the inclusions `material` traces with (`GemMaterial::inclusions`), or
    /// `None` when it has none or they are not valid on the wire ([`Self::validate`]). The
    /// second case is the sender's cue to keep the render local.
    #[must_use]
    pub fn of_material(material: &GemMaterial) -> Option<Self> {
        Self::of_set(material.oriented_inclusions()?).filter(|record| record.validate().is_ok())
    }

    /// The set this record describes. The families are sanitised again, so a record that passed
    /// [`Self::validate`] cannot put an unusable family into the tracer.
    #[must_use]
    pub fn to_set(&self) -> InclusionSet {
        let set = InclusionSet::new(&self.families);
        self.structure
            .map_or(set, |structure| set.with_structure(structure))
    }

    /// Refuses a record no honest sender produces.
    ///
    /// # Errors
    ///
    /// The first problem, as a sentence.
    pub fn validate(&self) -> Result<(), String> {
        if self.families.len() > MAX_FAMILIES {
            return Err(format!(
                "effects.material_inclusions has {} families (at most {MAX_FAMILIES})",
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
                    "effects.material_inclusions has an implausible family: {family:?}"
                ));
            }
        }
        self.structure.as_ref().map_or(Ok(()), validate_structure)
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
            "effects.material_inclusions.structure is implausible: {structure:?}"
        ))
    }
}
