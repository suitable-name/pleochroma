//! The render-physics data of a custom material as the JSON text its library row stores: the
//! per-axis dispersion curves, the oriented inclusion families and the play-of-color structure.
//!
//! `pleochroma-vault` keeps all three as opaque text columns (`per_axis_dispersion_json`,
//! `inclusion_families_json`, `structure_json`); this module is the one place their spelling is
//! written down, shared by the desktop editor, the command line and the design file's material
//! snapshot. Every reader is lenient in the way [`super::absorption_bands_from_json`] is: text
//! that is blank, malformed or describes something the renderer cannot use reads as `None`, and
//! the caller then builds the material exactly as it did before the column existed. One biaxial
//! curve without the other is ignored, and a pair that crosses the base (beta) curve inside
//! 380-780 nm is ignored too.

use crate::{
    crystal_axis::CrystalOrientation,
    native::{CustomMaterialSnapshot, dispersion_model_dto, dispersion_model_from_dto},
};
use pleochroma::optics::{
    dispersion::DispersionModel,
    inclusions::{
        CrystalFrame, Family, MAX_FAMILIES,
        bragg::{StructureKind, StructureSpec},
    },
    materials::{GemMaterial, OpticalCharacter},
};
use pleochroma_formats::native::DispersionModelDto;
use serde::{Deserialize, Serialize};

/// The per-axis dispersion curves of a custom material.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PerAxisDispersion {
    /// The extraordinary-ray curve of a uniaxial material
    /// (`GemMaterial::uniaxial_extraordinary_dispersion`).
    Uniaxial(DispersionModel),
    /// The `n_alpha` and `n_gamma` curves of a biaxial material
    /// (`GemMaterial::biaxial_principal_dispersion`); `n_beta` stays the main curve.
    Biaxial([DispersionModel; 2]),
}

/// The text form: `{"e": <model>}` or `{"alpha": <model>, "gamma": <model>}`, each model the
/// table a `dispersion_model_json` holds.
#[derive(Debug, Default, Serialize, Deserialize)]
struct PerAxisText {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    e: Option<DispersionModelDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    alpha: Option<DispersionModelDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gamma: Option<DispersionModelDto>,
}

/// The JSON text a library row stores for `curves`.
#[must_use]
pub fn per_axis_dispersion_to_json(curves: &PerAxisDispersion) -> Option<String> {
    let text = match curves {
        PerAxisDispersion::Uniaxial(e) => PerAxisText {
            e: Some(dispersion_model_dto(e)),
            ..PerAxisText::default()
        },
        PerAxisDispersion::Biaxial([alpha, gamma]) => PerAxisText {
            alpha: Some(dispersion_model_dto(alpha)),
            gamma: Some(dispersion_model_dto(gamma)),
            ..PerAxisText::default()
        },
    };
    serde_json::to_string(&text).ok()
}

/// Inverse of [`per_axis_dispersion_to_json`].
///
/// `None` for blank or malformed text, for a table that names neither a lone `e` curve nor both
/// `alpha` and `gamma` (a half-set biaxial pair is not a material), and for a curve
/// [`dispersion_model_from_dto`] refuses.
#[must_use]
pub fn per_axis_dispersion_from_json(text: &str) -> Option<PerAxisDispersion> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed: PerAxisText = serde_json::from_str(trimmed).ok()?;
    let model = |dto: &Option<DispersionModelDto>| dto.as_ref().and_then(dispersion_model_from_dto);
    match (&parsed.e, &parsed.alpha, &parsed.gamma) {
        (Some(_), None, None) => model(&parsed.e).map(PerAxisDispersion::Uniaxial),
        (None, Some(_), Some(_)) => Some(PerAxisDispersion::Biaxial([
            model(&parsed.alpha)?,
            model(&parsed.gamma)?,
        ])),
        _ => None,
    }
}

/// The per-axis curves `material` carries, for a library row made from it.
///
/// The extraordinary curve of a uniaxial material or the `n_alpha`/`n_gamma` pair of a biaxial
/// one, `None` for a material that keeps the constant birefringence offset. The inverse of the
/// part of [`with_custom_physics`] that sets them.
#[must_use]
pub fn per_axis_dispersion_of_material(material: &GemMaterial) -> Option<PerAxisDispersion> {
    if material.biaxial_delta_beta_alpha.is_some() {
        return material
            .biaxial_principal_dispersion
            .map(PerAxisDispersion::Biaxial);
    }
    matches!(
        material.optical_character,
        OpticalCharacter::UniaxialPositive | OpticalCharacter::UniaxialNegative
    )
    .then_some(material.uniaxial_extraordinary_dispersion)
    .flatten()
    .map(PerAxisDispersion::Uniaxial)
}

/// The per-axis curves a design file's material snapshot carries.
///
/// `None` for no curve, a curve the renderer refuses, or a combination that is no material (an
/// `e` curve beside the biaxial pair, or only one of the pair).
#[must_use]
pub fn per_axis_dispersion_of_snapshot(
    snapshot: &CustomMaterialSnapshot,
) -> Option<PerAxisDispersion> {
    let model = |dto: &Option<DispersionModelDto>| dto.as_ref().and_then(dispersion_model_from_dto);
    match (
        &snapshot.dispersion_model_e,
        &snapshot.dispersion_model_alpha,
        &snapshot.dispersion_model_gamma,
    ) {
        (Some(_), None, None) => {
            model(&snapshot.dispersion_model_e).map(PerAxisDispersion::Uniaxial)
        }
        (None, Some(_), Some(_)) => Some(PerAxisDispersion::Biaxial([
            model(&snapshot.dispersion_model_alpha)?,
            model(&snapshot.dispersion_model_gamma)?,
        ])),
        _ => None,
    }
}

/// The JSON text a library row stores for the inclusion families `families` (at most
/// [`MAX_FAMILIES`] are kept), or `None` for no families.
#[must_use]
pub fn inclusion_families_to_json(families: &[Family]) -> Option<String> {
    let kept = &families[..families.len().min(MAX_FAMILIES)];
    (!kept.is_empty())
        .then(|| serde_json::to_string(kept).ok())
        .flatten()
}

/// Inverse of [`inclusion_families_to_json`].
///
/// `None` for blank, malformed or empty text and for a list in which no family can scatter
/// anything (a non-finite or non-positive strength). Families past [`MAX_FAMILIES`] are dropped;
/// the strengths and directions themselves are limited later, when the set is resolved.
#[must_use]
pub fn inclusion_families_from_json(text: &str) -> Option<Vec<Family>> {
    let mut families: Vec<Family> = serde_json::from_str(text.trim()).ok()?;
    families.truncate(MAX_FAMILIES);
    families
        .iter()
        .any(|f| f.sigma.is_finite() && f.sigma > 0.0)
        .then_some(families)
}

/// The JSON text a library row stores for a play-of-color choice.
#[must_use]
pub fn structure_spec_to_json(spec: &StructureSpec) -> Option<String> {
    serde_json::to_string(spec).ok()
}

/// Inverse of [`structure_spec_to_json`].
///
/// `None` for blank or malformed text and for a choice with a non-finite number or an empty
/// wavelength range, which no structure can be built from.
#[must_use]
pub fn structure_spec_from_json(text: &str) -> Option<StructureSpec> {
    let spec: StructureSpec = serde_json::from_str(text.trim()).ok()?;
    let finite = [
        spec.color_low_nm,
        spec.color_high_nm,
        spec.purity,
        spec.coverage,
    ]
    .iter()
    .all(|v| v.is_finite())
        && spec.size.is_none_or(f32::is_finite);
    (finite && spec.color_low_nm > 0.0 && spec.color_high_nm >= spec.color_low_nm).then_some(spec)
}

/// A custom material's stored play-of-color choice: the [`StructureSpec`] and, when the material
/// imitates a built-in structure, the [`StructureKind`] it is resolved as.
///
/// A choice without a kind is stored and carried but not applied: the structure's geometry (an
/// opal's random lattice, a labradorite's fixed lamella) is the kind's, so only a material that
/// names one can be built.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CustomStructure {
    /// The kind the material imitates, or `None` for a choice that stays unapplied.
    pub kind: Option<StructureKind>,
    /// The layout, colors and seed.
    pub spec: StructureSpec,
}

/// The JSON text a library row stores for `structure`.
///
/// It is the spec's own table plus a lowercase `kind` key when one is set (a choice without a
/// kind is spelled exactly as [`structure_spec_to_json`] spells it).
#[must_use]
pub fn custom_structure_to_json(structure: &CustomStructure) -> Option<String> {
    /// The spec's table with the kind in front.
    #[derive(Serialize)]
    struct Kinded<'a> {
        kind: &'static str,
        #[serde(flatten)]
        spec: &'a StructureSpec,
    }
    structure.kind.map_or_else(
        || structure_spec_to_json(&structure.spec),
        |kind| {
            serde_json::to_string(&Kinded {
                kind: kind.name(),
                spec: &structure.spec,
            })
            .ok()
        },
    )
}

/// Inverse of [`custom_structure_to_json`], with [`structure_spec_from_json`]'s leniency; an
/// unknown `kind` reads as no kind.
#[must_use]
pub fn custom_structure_from_json(text: &str) -> Option<CustomStructure> {
    let spec = structure_spec_from_json(text)?;
    let kind = serde_json::from_str::<serde_json::Value>(text.trim())
        .ok()
        .and_then(|value| {
            value
                .get("kind")?
                .as_str()
                .and_then(StructureKind::from_name)
        });
    Some(CustomStructure { kind, spec })
}

/// `material` with a custom material's stored play-of-color applied, when it names its kind.
///
/// Call it after the default orientation is baked in (a labradorite-like lamella follows the
/// material's axes). Without a structure or a kind the material is returned as it was, so a row
/// or snapshot that never had one is unchanged.
#[must_use]
pub fn with_custom_structure(
    material: GemMaterial,
    structure: Option<&CustomStructure>,
) -> GemMaterial {
    match structure {
        Some(CustomStructure {
            kind: Some(kind),
            spec,
        }) => material.with_structure_spec_as(*kind, spec),
        _ => material,
    }
}

/// Whether a custom biaxial pair keeps `n_alpha < n_beta < n_gamma` strictly at every 5 nm from
/// 380 to 780 nm; `base` is the material's own curve, which is `n_beta`.
///
/// The renderer sorts the three evaluated indices and attaches them to the unchanged principal
/// frame, so a curve that crosses the beta curve inside the band would move an index from one
/// principal direction to another at that wavelength while the absorption slots stay put. Only a
/// custom pair can cross (no built-in does), and a pair that touches the beta curve is no
/// biaxial material either, so the comparison is strict.
#[must_use]
pub fn biaxial_curves_are_ordered(
    base: &DispersionModel,
    alpha: &DispersionModel,
    gamma: &DispersionModel,
) -> bool {
    const STEP_NM: f32 = 5.0;
    // 380, 385, ... 780: the edges are whole multiples of the step, so both are sampled exactly.
    const STEP_COUNT: u16 = 80;
    const LAST_NM: f32 = STEP_COUNT as f32 * STEP_NM + DispersionModel::BAND_MIN_NM;
    const _: () = assert!(
        LAST_NM == DispersionModel::BAND_MAX_NM,
        "the sampled 5 nm grid must end exactly at the band's upper edge"
    );
    (0..=STEP_COUNT)
        .map(|step| f32::from(step).mul_add(STEP_NM, DispersionModel::BAND_MIN_NM))
        .all(|lambda| {
            let beta = base.evaluate(lambda);
            alpha.evaluate(lambda) < beta && beta < gamma.evaluate(lambda)
        })
}

/// `material` with the render-physics data of a custom material applied: the per-axis curves,
/// then the inclusion families resolved into the material's own crystal frame.
///
/// This is the one place a stored custom material's curves and families reach the stone, shared
/// by the library row ([`super::LibraryMaterial::gem_material`]) and a design file's snapshot,
/// so both build the same stone. Call it AFTER the default orientation has been baked in
/// ([`super::with_default_orientation`]): the families live in that frame, whose second
/// direction is the biaxial `alpha_axis` or, for a uniaxial stone, the stored Second Axis of
/// `orientation` (the Silk Turn).
///
/// Each curve is applied only to the optical character that reads it (the extraordinary curve to
/// a uniaxial stone, the `n_alpha`/`n_gamma` pair to a biaxial one), so a leftover curve of a
/// material switched to another character never reaches the optics. Nothing to apply returns the
/// material untouched.
///
/// One biaxial curve without the other is ignored (the readers already refuse it); a pair that
/// crosses the base curve inside the visible band, or touches it, is ignored too (see
/// [`biaxial_curves_are_ordered`]). The achromatic `n_alpha`/`n_gamma` spreads then apply.
///
/// The families are resolved in the frame the material has HERE. A design's own Crystal Axis does
/// not change them in this function, but the render setup turns them afterwards: when the design's
/// axis moves the material's, `render_setup::apply_material_overrides_with_inclusions` rebases the
/// resolved families into the new frame, so they follow the stone's axes.
#[must_use]
pub fn with_custom_physics(
    mut material: GemMaterial,
    per_axis: Option<PerAxisDispersion>,
    families: Option<&[Family]>,
    orientation: Option<CrystalOrientation>,
) -> GemMaterial {
    match per_axis {
        Some(PerAxisDispersion::Uniaxial(e))
            if matches!(
                material.optical_character,
                OpticalCharacter::UniaxialPositive | OpticalCharacter::UniaxialNegative
            ) =>
        {
            material.uniaxial_extraordinary_dispersion = Some(e);
        }
        Some(PerAxisDispersion::Biaxial(curves)) if material.biaxial_delta_beta_alpha.is_some() => {
            if biaxial_curves_are_ordered(&material.dispersion, &curves[0], &curves[1]) {
                material.biaxial_principal_dispersion = Some(curves);
            } else {
                tracing::warn!(
                    name = %material.name,
                    "custom biaxial curves cross the beta curve inside 380-780 nm; ignored"
                );
            }
        }
        _ => {}
    }
    let Some(families) = families.filter(|f| !f.is_empty()) else {
        return material;
    };
    let second = if material.biaxial_delta_beta_alpha.is_some() {
        material.alpha_axis
    } else {
        orientation.and_then(|o| o.to_override().1)
    };
    let set = CrystalFrame::new(material.c_axis, second).resolve(families);
    material.with_inclusions(set)
}

#[cfg(test)]
mod tests;
