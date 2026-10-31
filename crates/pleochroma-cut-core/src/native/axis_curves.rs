//! A custom material's render-physics data (per-axis dispersion curves, inclusion families and
//! play-of-color structure), between a [`GemMaterial`] and a [`CustomMaterialSnapshot`].
//!
//! The reading and gating live in [`crate::material`] (`with_custom_physics`,
//! `per_axis_dispersion_of_snapshot`): this module only feeds them the snapshot's tables, so a
//! library row and a design file's snapshot build the same stone. Without the tables the material
//! is bit-identical to one that never had them. A table that fails
//! [`pleochroma::optics::dispersion::DispersionModel::validate`] reads as absent, like the main
//! one, and one biaxial curve without the other is ignored.

use super::dispersion_dto::dispersion_model_dto;
use crate::{
    crystal_axis::CrystalOrientation,
    material::{
        PerAxisDispersion, custom_structure_from_json, inclusion_families_from_json,
        per_axis_dispersion_of_material, per_axis_dispersion_of_snapshot, with_custom_physics,
        with_custom_structure,
    },
};
use pleochroma::optics::materials::GemMaterial;
use pleochroma_formats::native::{CustomMaterialSnapshot, DispersionModelDto};

/// `mat` with the render-physics data `snapshot` carries applied.
///
/// That is the e-ray curve for a uniaxial material or the `[alpha, gamma]` pair for a biaxial
/// one, the inclusion families in the material's crystal frame, and a play-of-color structure
/// that names its kind.
///
/// The material's character and its default orientation must already be set (see
/// `gem_material_from_custom_snapshot`); `orientation` is that default, which carries the Second
/// Axis the families of a uniaxial stone are turned by.
#[must_use]
pub fn with_snapshot_physics(
    mat: GemMaterial,
    snapshot: &CustomMaterialSnapshot,
    orientation: Option<CrystalOrientation>,
) -> GemMaterial {
    let families = snapshot
        .inclusions
        .as_deref()
        .and_then(inclusion_families_from_json);
    let mat = with_custom_physics(
        mat,
        per_axis_dispersion_of_snapshot(snapshot),
        families.as_deref(),
        orientation,
    );
    let structure = snapshot
        .structure
        .as_deref()
        .and_then(custom_structure_from_json);
    with_custom_structure(mat, structure.as_ref())
}

/// The per-axis curves of `mat` as the snapshot's tables: `(e, alpha, gamma)`, each `None` when
/// the material has no such curve. A biaxial material gives the pair, a uniaxial one the e-ray.
#[must_use]
pub fn axis_dispersion_dtos(
    mat: &GemMaterial,
) -> (
    Option<DispersionModelDto>,
    Option<DispersionModelDto>,
    Option<DispersionModelDto>,
) {
    match per_axis_dispersion_of_material(mat) {
        Some(PerAxisDispersion::Uniaxial(e)) => (Some(dispersion_model_dto(&e)), None, None),
        Some(PerAxisDispersion::Biaxial([alpha, gamma])) => (
            None,
            Some(dispersion_model_dto(&alpha)),
            Some(dispersion_model_dto(&gamma)),
        ),
        None => (None, None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::inclusion_families_to_json;
    use pleochroma::optics::{
        dispersion::DispersionModel,
        inclusions::{CrystalDir, Family, FamilyKind},
        materials::OpticalCharacter,
    };

    const CURVE: DispersionModel = DispersionModel::Cauchy {
        a: 1.75,
        b: 0.008,
        c: 0.0,
    };

    // `biaxial_material()` (nd 1.7, dispersion 0.02) has the base curve a = 1.669855,
    // b = 0.010468 (1.7424 at 380 nm, 1.6871 at 780 nm). These two run 0.0099 below and 0.0101
    // above it at every wavelength (b differs by only 3e-5), so alpha < beta < gamma holds.
    const ALPHA_CURVE: DispersionModel = DispersionModel::Cauchy {
        a: 1.66,
        b: 0.0105,
        c: 0.0,
    };

    const GAMMA_CURVE: DispersionModel = DispersionModel::Cauchy {
        a: 1.68,
        b: 0.0105,
        c: 0.0,
    };

    fn snapshot() -> CustomMaterialSnapshot {
        CustomMaterialSnapshot::new(1.7, 0.02, 0.008, None, "Trigonal", "UniaxialNegative")
    }

    fn biaxial_material() -> GemMaterial {
        let mut mat = GemMaterial::new_custom("Sample", 1.7, 0.02, -0.008, [0.0; 3]);
        mat.optical_character = OpticalCharacter::BiaxialNegative;
        mat.biaxial_delta_beta_alpha = Some(0.002);
        mat
    }

    #[test]
    fn a_uniaxial_snapshot_takes_the_e_curve_and_nothing_else() {
        let with_e =
            snapshot().with_axis_dispersion(Some(dispersion_model_dto(&CURVE)), None, None);
        let mat = with_snapshot_physics(
            GemMaterial::new_custom("Sample", 1.7, 0.02, -0.008, [0.0; 3]),
            &with_e,
            None,
        );
        assert_eq!(mat.uniaxial_extraordinary_dispersion, Some(CURVE));
        assert_eq!(mat.biaxial_principal_dispersion, None);
        let (e, alpha, gamma) = axis_dispersion_dtos(&mat);
        assert_eq!(e, Some(dispersion_model_dto(&CURVE)));
        assert!(alpha.is_none() && gamma.is_none());
    }

    #[test]
    fn an_isotropic_material_ignores_a_leftover_e_curve() {
        let with_e =
            snapshot().with_axis_dispersion(Some(dispersion_model_dto(&CURVE)), None, None);
        let mut iso = GemMaterial::new_custom("Sample", 1.7, 0.02, 0.0, [0.0; 3]);
        iso.optical_character = OpticalCharacter::Isotropic;
        let mat = with_snapshot_physics(iso, &with_e, None);
        assert_eq!(mat.uniaxial_extraordinary_dispersion, None);
    }

    #[test]
    fn a_snapshot_without_curves_leaves_the_material_untouched() {
        let plain = GemMaterial::new_custom("Sample", 1.7, 0.02, -0.008, [0.0; 3]);
        let same = with_snapshot_physics(plain.clone(), &snapshot(), None);
        assert_eq!(same, plain);
        assert_eq!(axis_dispersion_dtos(&same), (None, None, None));
    }

    #[test]
    fn one_biaxial_curve_alone_is_ignored() {
        let mat = biaxial_material();
        let only_alpha =
            snapshot().with_axis_dispersion(None, Some(dispersion_model_dto(&CURVE)), None);
        let result = with_snapshot_physics(mat.clone(), &only_alpha, None);
        assert_eq!(result.biaxial_principal_dispersion, None);
        let both = snapshot().with_axis_dispersion(
            None,
            Some(dispersion_model_dto(&ALPHA_CURVE)),
            Some(dispersion_model_dto(&GAMMA_CURVE)),
        );
        let result = with_snapshot_physics(mat, &both, None);
        assert_eq!(
            result.biaxial_principal_dispersion,
            Some([ALPHA_CURVE, GAMMA_CURVE])
        );
        let (e, alpha, gamma) = axis_dispersion_dtos(&result);
        assert!(e.is_none() && alpha.is_some() && gamma.is_some());
    }

    #[test]
    fn the_snapshot_families_reach_the_material() {
        let families = inclusion_families_to_json(&[Family {
            kind: FamilyKind::Needle,
            dir: CrystalDir::AlongC,
            sigma: 0.2,
            p: 2,
            lobe: 0.05,
            rho: 0.0,
        }]);
        let with_silk = snapshot().with_inclusion_families(families, None);
        let plain = GemMaterial::new_custom("Sample", 1.7, 0.02, -0.008, [0.0; 3]);
        let mat = with_snapshot_physics(plain.clone(), &with_silk, None);
        assert!(mat.scatters());
        assert!(mat.oriented_inclusions().is_some());
        assert!(!plain.scatters());
    }
}
