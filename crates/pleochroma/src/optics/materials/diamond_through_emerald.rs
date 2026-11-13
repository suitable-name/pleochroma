//! Built-in material data: Diamond, Sapphire, Ruby, Emerald.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// The Diamond to Emerald entries of the built-in table, split out of `all_materials` to
    /// stay under clippy's function-length lint.
    pub(super) fn built_in_materials_diamond_through_emerald() -> Vec<Self> {
        let mut materials = Self::built_in_materials_diamond_and_sapphire();
        materials.extend(Self::built_in_materials_ruby_and_emerald());
        materials
    }

    /// First half of the Diamond-through-Emerald part (Diamond, Sapphire).
    fn built_in_materials_diamond_and_sapphire() -> Vec<Self> {
        vec![
            // Diamond (C)
            //
            // Source: R. Peter, Z. Phys. 15, 358 (1923), the standard 2-term Sellmeier
            // fit for diamond (refractiveindex.info "Diamond: n (Peter 1923)"), valid
            // 0.226-0.760 um:
            //   n^2 - 1 = 4.3356*lambda^2/(lambda^2-0.1060^2) + 0.3306*lambda^2/(lambda^2-0.1750^2)
            // Represented via Sellmeier3's 3-pole form with the 3rd pole zeroed (b=0,
            // c=1.0 um^2, outside the visible-range domain) since DispersionModel has
            // no native 2-term Sellmeier variant. Verified: n_d = 2.41726, Delta
            // n(F-C) = 0.02564, Abbe V_d = 55.27 -- matches literature (n_D ~ 2.417,
            // V_d ~ 55.3). See `tests::builtin_material_abbe_numbers_match_published_values`.
            Self {
                name: "Diamond".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [4.3356, 0.3306, 0.0],
                    c: [0.011_236, 0.030_625, 1.0],
                },
                birefringence_delta: 0.0,
                // colorless (no chromophore): empty band set, zero absorption at every
                // wavelength -- must render identically to before.
                absorption: AbsorptionTensor::isotropic(vec![]),
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
                uniaxial_extraordinary_dispersion: None,
                biaxial_principal_dispersion: None,
            },
            // Sapphire (Al2O3)
            //
            // Source: I.H. Malitson & M.J. Dodge, J. Opt. Soc. Am. 62, 1405A (1972),
            // ordinary-ray Sellmeier fit (refractiveindex.info "Al2O3: Malitson-o"),
            // valid 0.2-5.0 um:
            //   n^2-1 = 1.4313493 l^2/(l^2-0.0726631^2) + 0.65054713 l^2/(l^2-0.1193242^2)
            //           + 5.3414021 l^2/(l^2-18.028251^2)
            // Verified: n_d = 1.76808, Delta n(F-C) = 0.01063, Abbe V_d = 72.27,
            // matching the standard literature figure V_d(sapphire) ~ 72.
            //
            // birefringence_delta derived from Malitson & Dodge's own companion e-ray
            // Sellmeier fit (same 1972 paper, "Al2O3: Malitson-e"):
            //   n_e^2 = 1 + 1.5039759*l^2/(l^2-0.0740288^2) + 0.55069141*l^2/(l^2-0.1216529^2)
            //           + 6.5927379*l^2/(l^2-20.072248^2)
            // giving n_e(D) = 1.760002 against n_o(D) = 1.768076, i.e. n_e - n_o = -0.008074
            // at the sodium D line, both indices from the same primary paper. The stored
            // birefringence_delta -0.0081 is that value rounded; the e-ray Sellmeier
            // itself is the `uniaxial_extraordinary_dispersion` below, so n_e - n_o runs
            // from -0.00846 (400 nm) to -0.00798 (700 nm).
            // See `tests::builtin_material_abbe_numbers_match_published_values`.
            Self {
                name: "Sapphire".to_string(),
                crystal_system: CrystalSystem::Trigonal,
                optical_character: OpticalCharacter::UniaxialNegative,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [1.431_349, 0.650_547, 5.341_402],
                    c: [0.00528, 0.01424, 325.015],
                },
                birefringence_delta: -0.0081,
                // Chromophore: blue sapphire's color comes from the Fe2+-Ti4+ intervalence
                // charge-transfer (IVCT) band, which absorbs yellow to red and transmits blue.
                // Source: Dubinsky, Stone-Sundberg & Emmett, "A Quantitative Description of the
                // Causes of Color in Corundum," Gems & Gemology 56(1), 2020, pp. 2-28: the band
                // peaks at 580 nm for E perp c (o-ray) and at 700 nm for E par c (e-ray).
                //
                // Three energy Gaussians per ray, fitted to the GIA Fe2+-Ti4+ cross sections
                // (`data/reference_spectra/gia_v3plus_fe2plus_ti4plus_sigma_5nm.csv`) at
                // 12.5 ppma*cm: the medium-dark, vivid royal blue of a fine sapphire. The
                // 10 ppma*cm strength is the Medium Blue variant (`sapphire.medium_blue`).
                // Swatch (7 mm, D65): L*50 C*44 h 285; o-ray L*39 C*67 h 295.
                // Calibration record: docs/chromophores/calibration-records.md#sapphire
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        // o-ray (E-perp-c)
                        AbsorptionBand::energy(585.0, 4_878.0, 2.5125),
                        AbsorptionBand::energy(744.0, 3_142.0, 1.108_75),
                        AbsorptionBand::energy(457.0, 12_000.0, 0.541_25),
                    ],
                    vec![
                        // e-ray (E-parallel-c)
                        AbsorptionBand::energy(612.0, 7_466.0, 0.463_75),
                        AbsorptionBand::energy(729.0, 4_955.0, 1.2875),
                        AbsorptionBand::energy(467.0, 11_169.0, 0.655),
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
                // Malitson & Dodge (1972) e-ray Sellmeier (see the comment above):
                // the true n_e(lambda), so the birefringence and the walk-off disperse.
                uniaxial_extraordinary_dispersion: Some(DispersionModel::Sellmeier3 {
                    b: [1.503_975_9, 0.550_691_4, 6.592_737_9],
                    c: [0.005_480_263, 0.014_799_428, 402.895_14],
                }),
                biaxial_principal_dispersion: None,
            },
        ]
    }

    /// Second half of the Diamond-through-Emerald part (Ruby, Emerald), split off
    /// `built_in_materials_diamond_through_emerald` for the same function-length reason.
    fn built_in_materials_ruby_and_emerald() -> Vec<Self> {
        vec![
            // Ruby (Al2O3:Cr) -- same host lattice as Sapphire above (trace Cr3+ at
            // sub-1% does not measurably shift the host Al2O3 dispersion), so it
            // shares the identical Malitson & Dodge (1972) Sellmeier fit. See the
            // Sapphire entry's comment for the source, the verified n_d/Delta
            // n(F-C)/Abbe numbers, and the birefringence_delta derivation.
            Self {
                name: "Ruby".to_string(),
                crystal_system: CrystalSystem::Trigonal,
                optical_character: OpticalCharacter::UniaxialNegative,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [1.431_349, 0.650_547, 5.341_402],
                    c: [0.00528, 0.01424, 325.015],
                },
                birefringence_delta: -0.0081,
                // Chromophore: ruby's Cr3+ absorbs in two bands -- violet near 410nm
                // and yellow-green near 550nm (the two spin-allowed Cr3+ d-d
                // transitions, ~4A2->4T1 and ~4A2->4T2) -- leaving a wide red
                // transmission window (>~620nm) and a narrower blue window
                // (~470-490nm) between the peaks. Sources: GIA "Application of
                // UV-Vis-NIR Spectroscopy to Gemology" (Winter 2024 Gems & Gemology)
                // and Cr:Al2O3 spectroscopy papers citing peaks at ~410-413nm/~550nm.
                // This double-window structure is what makes ruby shift redder under
                // incandescent light than daylight (see
                // `illuminant_color_shift::ruby_shifts_redder_under_incandescent_than_d65`).
                //
                // Pleochroism: o-ray maxima near 560 nm and e-ray near 550 nm (Mandarino,
                // American Mineralogist 44, 961, 1959, Table 5).
                //
                // Seven energy Gaussians per ray, fitted to the GIA Cr3+ and Fe3+ cross sections
                // of the same paper (`data/reference_spectra/gia_dubinsky2020_corundum_six_
                // chromophores_sigma_1nm.csv`) at Cr 400 and Fe 1000 ppma*cm over the face-up
                // path. Fe3+ closes the blue window, which a pure Cr3+ set leaves open and
                // renders magenta. The 325 and 370 nm bands are ultraviolet edge stand-ins.
                // The two densities are chosen from renders against a clean ruby photo.
                // Swatch (7 mm, D65): unpolarised L*46 C*53 h 19.
                // Calibration record: docs/chromophores/calibration-records.md#ruby
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        // o-ray (E-perp-c)
                        AbsorptionBand::energy(324.9, 3_084.4, 61.59),
                        AbsorptionBand::energy(392.7, 4_765.1, 9.524),
                        AbsorptionBand::energy(565.2, 1_570.0, 6.776),
                        AbsorptionBand::energy(529.6, 2_121.2, 3.718),
                        AbsorptionBand::energy(450.0, 1_379.7, 1.472),
                        AbsorptionBand::energy(672.1, 3_994.0, 0.309),
                        AbsorptionBand::energy(472.0, 397.7, 0.220_4),
                    ],
                    vec![
                        // e-ray (E-parallel-c)
                        AbsorptionBand::energy(370.0, 3_607.5, 13.98),
                        AbsorptionBand::energy(408.0, 2_362.5, 9.937),
                        AbsorptionBand::energy(450.0, 1_861.3, 2.819),
                        AbsorptionBand::energy(553.2, 1_754.6, 2.18),
                        AbsorptionBand::energy(517.8, 1_971.2, 1.336),
                        AbsorptionBand::energy(452.4, 402.5, 0.313_2),
                        AbsorptionBand::energy(665.3, 5_273.2, 0.144_4),
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
                // Malitson & Dodge (1972) e-ray Sellmeier (see the comment above):
                // the true n_e(lambda), so the birefringence and the walk-off disperse.
                uniaxial_extraordinary_dispersion: Some(DispersionModel::Sellmeier3 {
                    b: [1.503_975_9, 0.550_691_4, 6.592_737_9],
                    c: [0.005_480_263, 0.014_799_428, 402.895_14],
                }),
                biaxial_principal_dispersion: None,
            },
            // Emerald (Beryl, Be3Al2(SiO3)6:Cr/V)
            //
            // No primary Sellmeier fit for beryl exists, so this is a visible-range Cauchy
            // fit with lower confidence than the Sellmeier entries, to be cross-checked.
            //
            // n_d = 1.5791, within the International Gem Society's emerald range 1.57-1.60.
            // The society's "dispersion .014" is B-G; Delta n(F-C) = 0.0141 * 0.579 = 0.00816
            // (the 0.579 B-G to F-C ratio of the gemological entries), giving Abbe V_d 70.93.
            Self {
                name: "Emerald".to_string(),
                crystal_system: CrystalSystem::Hexagonal,
                optical_character: OpticalCharacter::UniaxialNegative,
                dispersion: DispersionModel::Cauchy {
                    a: 1.566_794,
                    b: 0.004_273,
                    c: 0.0,
                },
                birefringence_delta: -0.0060,
                // Chromophore: emerald's Cr3+ (some localities: V3+ too) absorbs in a
                // blue-violet band near 430 nm and a red-orange band near 600 nm, leaving a
                // green transmission window near 510 nm. Sources: GIA gemological references
                // ("main absorption bands at approximately 620 and 430 nm").
                //
                // Uniaxial three-band energy-Gaussian fits to the Caltech GRR 3570 emerald
                // spectrum (`data/reference_spectra/caltech_emerald_grr3570_5nm.csv`) at 0.3 cm.
                // This gives the measured o/e dichroism: o-ray yellowish green, e-ray bluish
                // green (Wood & Nassau 1968). Swatch (7 mm, D65): L*65 C*101 h 168.
                // Calibration record: docs/chromophores/calibration-records.md#emerald
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        // o-ray (E-perp-c)
                        AbsorptionBand::energy(430.0, 3_579.0, 4.5),
                        AbsorptionBand::energy(591.0, 2_410.0, 2.7),
                        AbsorptionBand::energy(633.0, 1_860.0, 1.6),
                    ],
                    vec![
                        // e-ray (E-parallel-c)
                        AbsorptionBand::energy(422.0, 2_681.0, 3.7),
                        AbsorptionBand::energy(584.0, 1_656.0, 2.2),
                        AbsorptionBand::energy(645.0, 1_595.0, 5.4),
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
                uniaxial_extraordinary_dispersion: None,
                biaxial_principal_dispersion: None,
            },
        ]
    }
}
