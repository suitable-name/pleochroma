//! Material information consumed by the analytical teacher.
//!
//! Absorption is sampled at every integer wavelength from 380 through 780 nm, exactly
//! the grid used by `color::body_color::body_color_from_spectra` for the teacher's tone.
//! Refraction uses the teacher's D/F/C lines. The three absorption slots are never
//! averaged: they are ordinary/alpha, ordinary/beta and extraordinary/gamma, in the
//! column order of `PrincipalFrame`. This represents the analytical metric material,
//! not the spatial or stochastic physics of the path tracer.

use pleochroma::{
    color::principal_axes::PrincipalFrame,
    optics::{
        absorption::AbsorptionBand,
        materials::{AbsorptionUnit, CrystalSystem, GemMaterial, OpticalCharacter},
    },
    render_setup::{MaterialOverrides, apply_material_overrides},
};
use pleochroma_cut_core::{Design, ResolvedMaterial};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Number of wavelengths used by the teacher's tone integral, including both endpoints.
pub const MATERIAL_SPECTRUM_SAMPLES: usize = 401;
/// Five character flags, three routing flags, nine frame entries, nine indices, scale and unit.
pub const MATERIAL_HEADER_DIM: usize = 28;
/// Width of one source or target material's appended observation features.
pub const MATERIAL_FEATURE_DIM: usize = MATERIAL_HEADER_DIM + 3 * MATERIAL_SPECTRUM_SAMPLES;

const FRAME_START: usize = 8;
const INDICES_START: usize = 17;
const SCALE_INDEX: usize = 26;
const UNIT_INDEX: usize = 27;
const METRIC_LINES_NM: [f32; 3] = [589.3, 486.1, 656.3];

/// Immutable raw material samples, captured once for a resolved episode material.
///
/// The internal `Arc` lets retained rollout states share these episode spectra rather
/// than copying thousands of floats into every structured observation. Dense replay packing
/// still owns its flat row; the trainer enforces a separate replay-memory budget.
/// Raw values remain inspectable in serialized observations; only `to_feature_vector`
/// performs normalization. Defaults support manually constructed synthetic observations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "MaterialFeaturesWire")]
pub struct MaterialFeatures {
    values: Arc<[f32]>,
}

#[derive(Deserialize)]
struct MaterialFeaturesWire {
    values: Arc<[f32]>,
}

impl TryFrom<MaterialFeaturesWire> for MaterialFeatures {
    type Error = &'static str;

    fn try_from(wire: MaterialFeaturesWire) -> Result<Self, Self::Error> {
        let features = Self {
            values: wire.values,
        };
        features.validate()?;
        Ok(features)
    }
}

impl Default for MaterialFeatures {
    fn default() -> Self {
        Self {
            values: vec![0.0; MATERIAL_FEATURE_DIM].into(),
        }
    }
}

impl MaterialFeatures {
    /// Width of a source or target material token suffix.
    pub const FEATURE_DIM: usize = MATERIAL_FEATURE_DIM;

    /// Captures the principal frame, optical modes and full teacher absorption grid.
    ///
    /// # Errors
    /// Returns a reason if material values are non-finite or physically invalid. Invalid
    /// input is rejected before the public optics helpers' defensive clamps can hide it.
    pub fn capture(material: &GemMaterial) -> Result<Self, &'static str> {
        validate_material(material)?;
        let mut values = vec![0.0; MATERIAL_FEATURE_DIM];
        let character = match material.optical_character {
            OpticalCharacter::Isotropic => 0,
            OpticalCharacter::UniaxialPositive => 1,
            OpticalCharacter::UniaxialNegative => 2,
            OpticalCharacter::BiaxialPositive => 3,
            OpticalCharacter::BiaxialNegative => 4,
        };
        values[character] = 1.0;
        // Match color::metrics::birefringent::is_anisotropic, which routes on crystal
        // symmetry and delta, not OpticalCharacter alone.
        values[5] = flag(
            material.crystal_system != CrystalSystem::Cubic
                && material.birefringence_delta.abs() > 1e-4,
        );
        values[6] = flag(material.absorption.is_pleochroic);
        // The histogram tone path explicitly distinguishes Some(beta) from None.
        values[7] = flag(material.absorption.beta_ray.is_some());
        values[FRAME_START..INDICES_START]
            .copy_from_slice(&PrincipalFrame::of_material(material).axes().to_cols_array());
        for (slot, lambda) in METRIC_LINES_NM.into_iter().enumerate() {
            let principal = principal_indices(material, lambda)?;
            let start = INDICES_START + 3 * slot;
            values[start..start + 3].copy_from_slice(&principal);
        }
        values[SCALE_INDEX] = material.absorption_path_scale;
        values[UNIT_INDEX] = flag(material.absorption_unit == AbsorptionUnit::PerMm);
        for (axis, bands) in absorption_slots(material).into_iter().enumerate() {
            for (offset, wavelength) in (380_u16..=780).enumerate() {
                let absorption = bands
                    .iter()
                    .map(|band| band.evaluate(f32::from(wavelength)))
                    .sum::<f32>();
                values[MATERIAL_HEADER_DIM + axis * MATERIAL_SPECTRUM_SAMPLES + offset] =
                    absorption;
            }
        }
        let features = Self {
            values: values.into(),
        };
        features.validate()?;
        Ok(features)
    }

    /// Validates the fixed width and values, including after deserialization.
    ///
    /// # Errors
    /// Rejects wrong dimensions, non-finite values and negative coefficients or scale.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.values.len() != MATERIAL_FEATURE_DIM {
            return Err("material feature width does not match the observation layout");
        }
        if self.values.iter().any(|value| !value.is_finite()) {
            return Err("material features contain a non-finite value");
        }
        if self.values[SCALE_INDEX] < 0.0
            || self.values[MATERIAL_HEADER_DIM..]
                .iter()
                .any(|&value| value < 0.0)
        {
            return Err("material absorption or path scale is negative");
        }
        Ok(())
    }

    /// Returns normalized features without clipping away spectral distinctions.
    ///
    /// Indices are divided by three; nonnegative absorption and path scale use `ln(1+x)`.
    /// This keeps large optical densities finite without collapsing every opaque value
    /// to one. Frame components and flags already have a natural unit scale.
    #[must_use]
    pub fn to_feature_vector(&self) -> Vec<f32> {
        let mut features = self.values.to_vec();
        for index in &mut features[INDICES_START..SCALE_INDEX] {
            *index /= 3.0;
        }
        features[SCALE_INDEX] = features[SCALE_INDEX].ln_1p();
        for absorption in &mut features[MATERIAL_HEADER_DIM..] {
            *absorption = absorption.ln_1p();
        }
        features
    }
}

fn flag(value: bool) -> f32 {
    if value { 1.0 } else { 0.0 }
}

fn absorption_slots(material: &GemMaterial) -> [&[AbsorptionBand]; 3] {
    let tensor = &material.absorption;
    [
        &tensor.o_ray,
        tensor.beta_ray.as_deref().unwrap_or(&tensor.o_ray),
        &tensor.e_ray,
    ]
}

fn validate_material(material: &GemMaterial) -> Result<(), &'static str> {
    if !material.c_axis.is_finite()
        || material.alpha_axis.is_some_and(|axis| !axis.is_finite())
        || !material.birefringence_delta.is_finite()
        || material
            .biaxial_delta_beta_alpha
            .is_some_and(|delta| !delta.is_finite())
        || !material.absorption_path_scale.is_finite()
        || material.absorption_path_scale < 0.0
    {
        return Err("material orientation, birefringence or absorption scale is invalid");
    }
    if absorption_slots(material)
        .into_iter()
        .flatten()
        .any(|band| {
            !band.center_nm.is_finite()
                || band.center_nm <= 0.0
                || !band.width_nm.is_finite()
                || band.width_nm <= 0.0
                || !band.peak.is_finite()
                || band.peak < 0.0
        })
    {
        return Err("material absorption band is invalid");
    }
    Ok(())
}

fn principal_indices(material: &GemMaterial, lambda: f32) -> Result<[f32; 3], &'static str> {
    let base = material.dispersion.evaluate(lambda);
    let values = material.biaxial_indicatrix(lambda).map_or_else(
        || [base, base, material.extraordinary_index_at(lambda, base)],
        |indicatrix| [indicatrix.n_alpha, indicatrix.n_beta, indicatrix.n_gamma],
    );
    if values
        .iter()
        .any(|value| !value.is_finite() || *value <= 1.0)
    {
        return Err("material principal refractive index is invalid");
    }
    Ok(values)
}

/// Applies the design's crystal orientation and physical size using production helpers.
///
/// `model_width` must be the solved stone's measured girdle width, not the preform size.
/// Call this for each scored candidate because that width may change with its angles.
/// The analytical objective does not model inclusion scattering or spatial zoning; this
/// does not pretend otherwise by adding them to the teacher's transport.
#[must_use]
pub fn material_for_design(
    design: &Design,
    material: &GemMaterial,
    model_width: Option<f64>,
) -> GemMaterial {
    let material = pleochroma_editor::stone_orientation::oriented_for_effects(
        material.clone(),
        &design.render_effects,
    );
    apply_material_overrides(
        material,
        &MaterialOverrides {
            inclusion_sigma_s: 0.0,
            c_axis_override: None,
            alpha_axis_override: None,
            edge_rounding_radius: 0.0,
            stone_width_mm: design.girdle_diameter_mm.unwrap_or(0.0) as f32,
        },
        model_width,
    )
}

/// The source/target material with the same orientation and size that scoring uses.
#[must_use]
pub fn resolved_material_for_design(
    design: &Design,
    material: &ResolvedMaterial,
    model_width: Option<f64>,
) -> ResolvedMaterial {
    ResolvedMaterial {
        gem: material_for_design(design, &material.gem, model_width),
        n_d: material.n_d,
        critical_angle_deg: material.critical_angle_deg,
    }
}

/// Effects present in a stone that the analytical training objective does not evaluate.
///
/// A material can still supply useful refraction and homogeneous tone supervision. These
/// notes prevent such a case from being reported as full render-physics coverage.
#[must_use]
pub fn material_scope_notes(design: &Design, material: &GemMaterial) -> Vec<&'static str> {
    let mut notes = Vec::new();
    if material.zoning.is_some() {
        notes.push("spatial color zoning is not evaluated by the analytical training objective");
    }
    if material.inclusions.is_some()
        || design.render_effects.inclusions.is_some()
        || design.render_effects.structure.is_some()
    {
        notes.push("oriented inclusions and structural color are not evaluated by the analytical training objective");
    }
    if material.scattering_sigma_s > 0.0 || design.render_effects.inclusion_sigma_s > 0.0 {
        notes.push("volume scattering is not evaluated by the analytical training objective");
    }
    if material.edge_rounding_radius > 0.0 || design.render_effects.edge_rounding_radius > 0.0 {
        notes.push("facet-edge rounding is not evaluated by the analytical training objective");
    }
    if design.render_effects.edge_abrasion.width > 0.0 {
        notes.push("edge abrasion is not evaluated by the analytical training objective");
    }
    if design.has_surface() {
        notes.push("surface polish grades, scratch directions and roughness are not evaluated by the analytical training objective; authored binary frosting is evaluated");
    }
    notes
}

#[cfg(test)]
#[path = "material_features_tests.rs"]
mod tests;
