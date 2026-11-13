//! Built-in material data: Amethyst, Citrine.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Continuation of [`Self::built_in_materials_aquamarine_through_citrine`] (split
    /// purely to keep each part under clippy's function-length lint): Amethyst and
    /// Citrine, both quartz.
    pub(super) fn built_in_materials_amethyst_through_citrine() -> Vec<Self> {
        vec![
            // Amethyst (alpha-Quartz, SiO2, color centre). Same SiO2 host as the colorless
            // Quartz entry: it reuses that entry's dispersion and birefringence bit for bit
            // and differs only in absorption. n_d = 1.54421, Delta n(F-C) = 0.00781 (quartz's
            // 0.013 figure is B-G, not F-C).
            Self {
                name: "Amethyst".to_string(),
                crystal_system: CrystalSystem::Trigonal,
                optical_character: OpticalCharacter::UniaxialPositive,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [0.286_041_4, 1.070_440_8, 1.102_022_4],
                    c: [0.0, 0.010_058_6, 100.0],
                },
                birefringence_delta: 0.0091,
                // Chromophore: the irradiation-induced Fe4+ ([FeO4]0 hole) centre of
                // Fe3+-bearing quartz, a broad band centred ~545nm (green-yellow) plus
                // a 357nm and a weak ~950nm band (Cox, J. Phys. C 10, 4631 (1977); the
                // Caltech Ametrine page: amethyst and citrine both hold ~40 ppm iron and
                // radiation oxidises Fe3+ to Fe4+), leaving violet/purple transmitted.
                //
                // Per-axis tensor: an anisotropic host never carries an isotropic one.
                // o-ray (E perp c): a four-band energy-Gaussian fit of the Caltech Anahi
                // (Bolivia) amethyst spectrum (`ameth.bo`, 6.17 mm slab cut (0001)), with
                // centres 364.3 / 416.1 / 537.1 / 953.8 nm and FWHM 4509 / 2911 / 5175 /
                // 3073 cm^-1. The main band is moved to 549 nm, inside the 540-550 nm
                // literature range for amethyst: that centre reproduces the dealer photos'
                // median colored hue (316), where the 537 nm centre rendered h 333.
                // e-ray (E par c): estimated from the o-ray bands at e/o weights 1.0 / 1.0 /
                // 0.7 / 0.5, since no polarized amethyst spectrum exists. It is invisible
                // while the stone is viewed along c. Strength: one factor 2.47 x the Anahi
                // crystal's per-mm absorption over every band, stored as per mm x 2.47 x
                // 9.905 mm, the face-up path of a 7 mm stone (3.5 mm per model unit x
                // `MODEL_UNIT_FACE_UP_PATH` 2.83), because the unit stays `ModelUnit`.
                // Swatch (7 mm, D65): o-ray L*49 C*43 h 314; unpolarised mean L*53 C*36 h 315.
                // Calibration record: docs/chromophores/calibration-records.md#amethyst
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        AbsorptionBand::energy(364.3, 4_508.9, 3.65),
                        AbsorptionBand::energy(416.1, 2_911.3, 0.971),
                        AbsorptionBand::energy(549.0, 5_174.7, 2.14),
                        AbsorptionBand::energy(953.8, 3_072.8, 0.474),
                    ],
                    vec![
                        AbsorptionBand::energy(364.3, 4_508.9, 3.65),
                        AbsorptionBand::energy(416.1, 2_911.3, 0.971),
                        AbsorptionBand::energy(549.0, 5_174.7, 1.5),
                        AbsorptionBand::energy(953.8, 3_072.8, 0.237),
                    ],
                ),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                alpha_axis: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                zoning: None,
                inclusions: None,
                uniaxial_extraordinary_dispersion: Some(DispersionModel::Sellmeier3 {
                    b: [0.288_518_04, 1.095_099_2, 1.156_624_8],
                    c: [0.0, 0.010_210_186, 100.0],
                }),
                biaxial_principal_dispersion: None,
            },
            // Citrine (alpha-Quartz, SiO2, Fe3+) -- same host crystal/dispersion
            // reuse as Amethyst immediately above; see that entry's comment.
            Self {
                name: "Citrine".to_string(),
                crystal_system: CrystalSystem::Trigonal,
                optical_character: OpticalCharacter::UniaxialPositive,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [0.286_041_4, 1.070_440_8, 1.102_022_4],
                    c: [0.0, 0.010_058_6, 100.0],
                },
                birefringence_delta: 0.0091,
                // Chromophore: Fe3+ in small hydrated oxide-hydroxide aggregates (the
                // Caltech quartz page: citrine is heated amethyst, "small aggregates of
                // iron oxide"), a charge-transfer absorption edge rising from the
                // near-UV into blue (a tail rather than a discrete band), leaving
                // yellow-orange transmitted, citrine's characteristic color.
                //
                // Per-axis tensor, as for Amethyst. Shape: three energy Gaussians (centres
                // 330 / 410 / 500 nm, FWHM 8477 / 7064 / 5181 cm^-1) fitted to the 16-point
                // citrine alpha(lambda) table of `data/reference_spectra/
                // citrine_annealed_amethyst.csv`. That table is secondary and unverified, so
                // only its shape is claimed. No polarized citrine spectrum exists and citrine's
                // pleochroism is very weak, so the o- and e-ray sets are equal.
                // Strength: the fit at 0.53 of the table, then `.scaled(2.0)`, i.e. 1.06 x the
                // table per mm over the 9.905 mm face-up path. The x 2 strength matches the
                // colored-pixel saturation of the dealer photos (s_uv 0.79; 0.57 at x 1).
                // Swatch L*85 C*77 h 87.
                // Calibration record: docs/chromophores/calibration-records.md#citrine
                // Calibration record: docs/chromophores/calibration-records.md#saturation-round-2026-11-04
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        AbsorptionBand::energy(330.0, 8_477.0, 8.69),
                        AbsorptionBand::energy(410.0, 7_064.0, 0.714),
                        AbsorptionBand::energy(500.0, 5_181.0, 0.121),
                    ],
                    vec![
                        AbsorptionBand::energy(330.0, 8_477.0, 8.69),
                        AbsorptionBand::energy(410.0, 7_064.0, 0.714),
                        AbsorptionBand::energy(500.0, 5_181.0, 0.121),
                    ],
                )
                .scaled(2.0),
                c_axis: Vec3::Y,
                biaxial_delta_beta_alpha: None,
                alpha_axis: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                zoning: None,
                inclusions: None,
                uniaxial_extraordinary_dispersion: Some(DispersionModel::Sellmeier3 {
                    b: [0.288_518_04, 1.095_099_2, 1.156_624_8],
                    c: [0.0, 0.010_210_186, 100.0],
                }),
                biaxial_principal_dispersion: None,
            },
        ]
    }
}
