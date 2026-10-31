//! Built-in material data: Precious Opal (White), Precious Opal (Black), Labradorite and
//! Moonstone.
//!
//! These are the host optics of the stones whose color comes from internal structure. The
//! structure itself is not part of the material: a design carries the play-of-color choice
//! (`inclusions::bragg::StructureSpec`, with its seed) and Moonstone's adularescence is the
//! `Adularescence` preset of `inclusions::catalogue`. The existing `Opal` entry stays as it is,
//! the plain body-color opal.

use super::{AbsorptionUnit, CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

/// The Fraunhofer F-C interval to Cauchy `B` factor: `1 / (1/0.4861^2 - 1/0.6563^2)` in
/// micrometres squared, the same conversion the `Opal` entry's comment uses.
const F_MINUS_C_PER_CAUCHY_B: f32 = 1.909_7;
/// The ratio of the F-C interval to the gemological B-G interval used file-wide.
const F_MINUS_C_PER_B_MINUS_G: f32 = 0.579;
/// `0.5893^2`, micrometres squared (the sodium D line).
const D_LINE_SQUARED: f32 = 0.347_271;

/// The Cauchy curve with index `n_d` at the sodium D line and the gemological dispersion
/// `b_minus_g` (the conversion of the other Cauchy entries).
fn cauchy_from_b_minus_g(n_d: f32, b_minus_g: f32) -> DispersionModel {
    let b = b_minus_g * F_MINUS_C_PER_B_MINUS_G / F_MINUS_C_PER_CAUCHY_B;
    DispersionModel::Cauchy {
        a: n_d - b / D_LINE_SQUARED,
        b,
        c: 0.0,
    }
}

impl GemMaterial {
    /// The shared fields of these entries: no scattering, no edge rounding, the model-unit
    /// absorption calibration, no zoning, no inclusions and no extraordinary curves.
    fn structural_color_host(
        name: &str,
        crystal_system: CrystalSystem,
        optical_character: OpticalCharacter,
        dispersion: DispersionModel,
        birefringence_delta: f32,
        biaxial_delta_beta_alpha: Option<f32>,
        absorption: AbsorptionTensor,
    ) -> Self {
        Self {
            name: name.to_string(),
            crystal_system,
            optical_character,
            dispersion,
            birefringence_delta,
            absorption,
            c_axis: Vec3::Y,
            biaxial_delta_beta_alpha,
            alpha_axis: None,
            scattering_sigma_s: 0.0,
            scattering_g: 0.0,
            edge_rounding_radius: 0.0,
            absorption_path_scale: 1.0,
            absorption_unit: AbsorptionUnit::ModelUnit,
            zoning: None,
            inclusions: None,
            uniaxial_extraordinary_dispersion: None,
            biaxial_principal_dispersion: None,
        }
    }

    /// Precious Opal (White), Precious Opal (Black), Labradorite and Moonstone.
    pub(super) fn built_in_materials_feldspars_and_precious_opal() -> Vec<Self> {
        // The silica of precious opal has the optics of the plain Opal entry (n_d 1.45, a small
        // Delta n(F-C) of 0.001, `A = 1.45 - 0.000523 / 0.5893^2`). Cubic is again a placeholder
        // for an amorphous solid. The play-of-color is not in the material: see this module's
        // comment.
        let opal_dispersion = DispersionModel::Cauchy {
            a: 1.448_493,
            b: 0.000_523,
            c: 0.0,
        };
        vec![
            // Precious Opal (White): a pale body (the colorless reference) that adds a milky
            // haze (`recommended_scattering`) beside the play-of-color of its domains.
            Self::structural_color_host(
                "Precious Opal (White)",
                CrystalSystem::Cubic,
                OpticalCharacter::Isotropic,
                opal_dispersion,
                0.0,
                None,
                AbsorptionTensor::isotropic(vec![]),
            ),
            // Precious Opal (Black): the same silica with a dark neutral body, which is what
            // makes the play-of-color stand out. The body is a uniform grey absorption (three
            // broad bands over the visible range, per model unit, an aesthetic choice): the plan
            // asked for a dark base zone toward the pavilion, but a zoned material cannot show
            // oriented inclusions or domains in the zoned estimator, and the domains sit where
            // the light enters anyway.
            Self::structural_color_host(
                "Precious Opal (Black)",
                CrystalSystem::Cubic,
                OpticalCharacter::Isotropic,
                opal_dispersion,
                0.0,
                None,
                AbsorptionTensor::isotropic(vec![
                    AbsorptionBand::new(450.0, 110.0, 1.4),
                    AbsorptionBand::new(550.0, 120.0, 1.4),
                    AbsorptionBand::new(650.0, 120.0, 1.4),
                ]),
            ),
            // Labradorite ((Ca,Na)(Al,Si)4O8, An 50-70 plagioclase), triclinic, biaxial positive,
            // 2V about 85 degrees. Ranges (Wikipedia, "Labradorite"): n_alpha 1.554-1.563,
            // n_beta 1.559-1.568, n_gamma 1.562-1.573, birefringence 0.008-0.010. The entry takes
            // 1.559 / 1.5625 / 1.567 (ESTIMATE, in the ranges, beta a little nearer alpha for the
            // positive sign): Delta n 0.008, beta - alpha 0.0035. The dispersion is NOT verified
            // for labradorite (0.012 is the figure of moonstone): it borrows that B-G of 0.012 as
            // a placeholder of the right size for a 1.56 feldspar. The lamellae that cause the
            // schiller are in `inclusions::bragg` (pair period 150-250 nm, Eur. J. Mineral. 34,
            // 393-410, 2022).
            Self::structural_color_host(
                "Labradorite",
                CrystalSystem::Triclinic,
                OpticalCharacter::BiaxialPositive,
                cauchy_from_b_minus_g(1.5625, 0.012),
                0.008,
                Some(0.0035),
                // Colorless: no absorption in any direction, so there is no pleochroism to keep
                // and the empty isotropic tensor stays (lead decision 2026-10-27: the
                // "anisotropic host, per-axis tensor" rule applies to colored stones).
                AbsorptionTensor::isotropic(vec![]),
            ),
            // Moonstone (orthoclase / adularia, KAlSi3O8), monoclinic, biaxial negative. Wikipedia,
            // "Moonstone (gemstone)": n 1.518-1.526, birefringence 0.005-0.010, dispersion 0.012.
            // The entry takes 1.518 / 1.523 / 1.526 (ESTIMATE, beta nearer gamma for the negative
            // sign): Delta n 0.008, beta - alpha 0.005. The adularescence is the `Adularescence`
            // preset of the inclusion catalogue (a broad platelet family): the lamella thickness
            // behind it is not established, so no lamella number is quoted anywhere.
            Self::structural_color_host(
                "Moonstone",
                CrystalSystem::Monoclinic,
                OpticalCharacter::BiaxialNegative,
                cauchy_from_b_minus_g(1.523, 0.012),
                0.008,
                Some(0.005),
                // Colorless: the empty isotropic tensor, as for Labradorite above.
                AbsorptionTensor::isotropic(vec![]),
            ),
        ]
    }
}
