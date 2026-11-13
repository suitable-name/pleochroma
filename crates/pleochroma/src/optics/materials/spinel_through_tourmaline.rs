//! Built-in material data: Spinel, Quartz, Tourmaline.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Spinel, Quartz and Tourmaline. The table is split across builder functions (see
    /// `built_in_materials_diamond_through_emerald`).
    pub(super) fn built_in_materials_spinel_through_tourmaline() -> Vec<Self> {
        vec![
            // Spinel (MgAl2O4)
            //
            // Source: W.J. Tropf & M.E. Thomas, "Magnesium Aluminum Oxide (Spinel),"
            // Handbook of Optical Constants of Solids III (1991), as tabulated by
            // refractiveindex.info ("MgAl2O4: Tropf"), valid 0.35-5.5 um:
            //   n^2-1 = 1.8938*l^2/(l^2-0.09942^2) + 3.0755*l^2/(l^2-15.826^2)
            // Encoded via Sellmeier3 with the unused 3rd pole zeroed (b=0, c=1.0 um^2,
            // outside the visible-range l^2 domain). Verified: n_d = 1.71610, Delta
            // n(F-C) = 0.01181, Abbe V_d = 60.63. Cross-check: gemological (B-G)
            // dispersion table lists Spinel = 0.020; 0.020*0.579 (the B-G->F-C ratio
            // used throughout, see Emerald entry) = 0.01158, within 2% of this
            // Sellmeier-derived value. The stored convention is F-C (486.1-656.3 nm).
            Self {
                name: "Spinel".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [1.8938, 3.0755, 0.0],
                    c: [0.009_884, 250.462, 1.0],
                },
                birefringence_delta: 0.0,
                // Models RED spinel: Cr3+ on the Al3+ site (K. Nassau, "The Physics and
                // Chemistry of Color", 2nd ed., Ch. 7), the Cr3+ d-d family of Ruby and
                // Alexandrite. Four energy Gaussians of the measured Caltech JQ-13 spectrum
                // (G. R. Rossman; natural red spinel, 1.52 mm, unpolarised): the
                // 4A2 -> 4T1(F) band near 400 nm, the 4A2 -> 4T2 band near 540 nm, and two
                // weaker bands. Vasiliev's synthetic Cr spinel gives the same peak ratio.
                // The ultraviolet edge below 380 nm and blue spinel (Co2+) are not modelled.
                // Calibration record: docs/chromophores/calibration-records.md#spinel-red
                //
                // Strength 0.45 of the measured spectrum: the photos of red spinel (median
                // hue 27) are pinker at full strength, and the photos' brown is not in the
                // Cr3+ spectrum.
                // Calibration record: docs/chromophores/calibration-records.md#saturation-round-2026-11-04
                absorption: AbsorptionTensor::isotropic(vec![
                    AbsorptionBand::energy(401.8, 3_993.5, 8.296),
                    AbsorptionBand::energy(542.0, 2_324.9, 4.691),
                    AbsorptionBand::energy(492.3, 2_997.7, 1.581),
                    AbsorptionBand::energy(628.0, 2_527.2, 0.332),
                ])
                .scaled(0.45),
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
            // Quartz (Rock Crystal / Amethyst / Citrine, alpha-SiO2)
            //
            // Source: G. Ghosh, "Dispersion-equation coefficients for the refractive
            // index and birefringence of calcite and quartz crystals," Opt. Commun.
            // 163, 95-102 (1999), ordinary-ray fit (refractiveindex.info "SiO2:
            // Ghosh-o"), valid 0.198-2.0531 um:
            //   n^2-1 = 0.28604141 + 1.07044083*l^2/(l^2-0.0100585997) + 1.10202242*l^2/(l^2-100)
            // Independently verified against a manufacturer (pmoptics.com) quartz
            // spec sheet: n_o(630nm)=1.54270 (spec: 1.542737), n_o(1550nm)=1.52770
            // (spec: 1.527606), both matching to <1e-4. Encoded via Sellmeier3: the
            // leading 0.28604141 constant uses pole 1 with c=0.0 (reduces to the bare
            // constant); the other two are the genuine poles. Verified: n_d = 1.54421,
            // Delta n(F-C) = 0.00781, Abbe V_d = 69.65. Note: this is lower than the
            // ~0.013 gemological "dispersion" figure often quoted for quartz -- that
            // figure is the standard B-G (not F-C) interval (0.013*0.579 = 0.00753,
            // within 4% of this Ghosh-derived value; the stored curve is the Ghosh
            // fit itself, in the F-C convention, not the converted figure).
            Self {
                name: "Quartz".to_string(),
                crystal_system: CrystalSystem::Trigonal,
                optical_character: OpticalCharacter::UniaxialPositive,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [0.286_041_4, 1.070_440_8, 1.102_022_4],
                    c: [0.0, 0.010_058_6, 100.0],
                },
                birefringence_delta: 0.0091,
                // This entry is the colorless rock-crystal reference: empty band sets
                // (zero absorption at every wavelength). Rock crystal has no visible-range
                // chromophore (its UV edge sits near 150nm). Its tinted varieties (Amethyst:
                // a hole-color-centre defect; Citrine: Fe3+) are separate built-ins (see
                // [`Self::built_in_materials_amethyst_through_citrine`]) that share this
                // dispersion/birefringence data (same SiO2 host) with their own bands.
                // Being colorless, there is no pleochroism to keep, so the empty isotropic
                // tensor applies. Colored stones on anisotropic hosts carry a pleochroic tensor.
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
                // Quartz (with Amethyst and Citrine, which reuse this exact curve) and
                // Rutile are the built-ins with a genuine primary e-ray Sellmeier
                // fit alongside the o-ray one -- for Quartz, G. Ghosh, Opt. Commun. 163, 95-102
                // (1999), extraordinary-ray fit (refractiveindex.info "SiO2:
                // Ghosh-e"), valid 0.198-2.0531 um:
                //   n^2-1 = 0.28851804 + 1.09509924*l^2/(l^2-0.0102101864) + 1.15662475*l^2/(l^2-100)
                // Same encoding technique as the o-ray fit above. Verified: n_e(D) =
                // 1.55338 against n_o(D) = 1.54421, i.e. n_e - n_o = +0.00917 at the
                // sodium D line, matching `birefringence_delta` (0.0091) within
                // rounding, from the same primary paper as the o-ray fit. Wired
                // through [`GemMaterial::extraordinary_index_at`] so quartz's
                // extraordinary ray disperses with its own genuine curve rather than
                // the constant-offset approximation every other uniaxial built-in
                // uses.
                uniaxial_extraordinary_dispersion: Some(DispersionModel::Sellmeier3 {
                    b: [0.288_518_04, 1.095_099_2, 1.156_624_8],
                    c: [0.0, 0.010_210_186, 100.0],
                }),
                biaxial_principal_dispersion: None,
            },
            Self::built_in_material_tourmaline(),
        ]
    }

    /// Tourmaline (Elbaite, complex borosilicate), the last entry of
    /// `built_in_materials_spinel_through_tourmaline`, built here to keep that table short.
    fn built_in_material_tourmaline() -> Self {
        // No primary Sellmeier/Cauchy fit for tourmaline exists (absent from the
        // refractiveindex.info database; tourmaline's compositional variability,
        // a large solid-solution family rather than a fixed stoichiometric
        // crystal, makes a single universal fit unlikely to exist at all).
        // LOWER-CONFIDENCE 2-parameter Cauchy fallback.
        //
        // n_d = 1.639405, within International Gem Society's cited elbaite range
        // (n_o = 1.619-1.655, n_e = 1.603-1.634). Dispersion shape: IGS's cited
        // "dispersion .017" is the Fraunhofer B-G interval, not F-C. Converted via
        // the same B-G->F-C ratio (0.579) as the Emerald entry:
        // Delta n(F-C) = 0.0171*0.579 = 0.00990. Two data points exactly
        // determine a 2-parameter Cauchy fit (same method as Zircon's comment):
        // A=1.624481, B=0.005183, verified n_d=1.639406, Delta n(F-C)=0.009902,
        // Abbe V_d=64.58. Lower confidence than a primary-literature entry.
        //
        // birefringence_delta = -0.0210, matching the IGS representative pairing
        // (n_o~1.644, n_e~1.623) widely reproduced for "typical" (green) elbaite
        // -- IGS's own range only bounds it (n_e-n_o spans roughly -0.016 to
        // -0.021 across the elbaite range), so this is an estimate, not a
        // single-crystal measurement.
        Self {
            name: "Tourmaline".to_string(),
            crystal_system: CrystalSystem::Trigonal,
            optical_character: OpticalCharacter::UniaxialNegative,
            dispersion: DispersionModel::Cauchy {
                a: 1.624_481,
                b: 0.005_183,
                c: 0.0,
            },
            birefringence_delta: -0.0210,
            // PLEOCHROISM: uniaxial `o_ray`/`e_ray` split from a measured elbaite. The Fe2+
            // band near 720nm is NOT strongly dichroic in elbaite: the Caltech spectra of the
            // blue GRR 1935 (o 0.905, e 0.994 per mm), the pure-blue GRR 3770 (1.00 / 0.95)
            // and the green GRR 1935 (0.54 / 0.475) have o:e about 1:1. Strong Fe2+ dichroism
            // belongs to the Fe2+-Fe3+ paired bands of dravite and schorl (Mattson & Rossman,
            // Phys. Chem. Minerals 14, 163 (1987); Caltech GRR 787). What is polarised in
            // elbaite is the ultraviolet / blue edge (Fe2+-Ti4+ and Mn2+-Ti4+ charge transfer;
            // the Mn2+-Ti4+ band at 325nm, Rossman & Mattson, Am. Mineral. 71, 599 (1986), with
            // a tail into the visible): o about 2 x e.
            //
            // Source: Caltech GRR 1935, the small green crystal, Usakos, Namibia (3.645 mm),
            // `data/reference_spectra/variant_tourmaline_verdelite_grr1935_green_
            // {eperpc,eparc}.csv`, fitted per ray with energy Gaussians. The UV edge is a
            // stand-in whose visible tail is the measurement. The bands are the mix that
            // matches the dealer photos of green tourmaline (median hue 121, L* 52): the
            // elbaite spectrum at 3.5 x and the measured Yellow variant at 5.5 x, the Pink
            // variant's Mn3+ at 0.7 x (it greys the green window to olive), and the elbaite's
            // Fe2+ at 1.45 x with the yellow part at 0.9 x to hold the hue. Eight energy
            // Gaussians a ray (dE2000 0.01 against the fit). Stored per model unit (per mm x
            // 9.905, the face-up path of a 7 mm stone).
            // Calibration record: docs/chromophores/calibration-records.md#tourmaline
            // Swatch L*52 C*46 h 124 (o-ray L*44 C*68 h 114, e-ray L*66 C*38 h 131); edge
            // o : e at 400 nm 3.3, Fe2+ o : e at 711 nm 1.15.
            absorption: AbsorptionTensor::uniaxial(
                vec![
                    // o-ray (E perp c)
                    AbsorptionBand::energy(386.6, 4_111.5, 234.0),
                    AbsorptionBand::energy(301.0, 9_911.9, 28.94),
                    AbsorptionBand::energy(711.4, 2_185.4, 8.817),
                    AbsorptionBand::energy(315.6, 13_521.2, 7.735),
                    AbsorptionBand::energy(620.3, 3_473.7, 1.501),
                    AbsorptionBand::energy(1_066.1, 2_706.1, 0.761_9),
                    AbsorptionBand::energy(542.2, 2_626.8, 0.673_9),
                    AbsorptionBand::energy(444.4, 4_554.1, 0.358_4),
                ],
                vec![
                    // e-ray (E par c)
                    AbsorptionBand::energy(367.9, 3_657.5, 166.0),
                    AbsorptionBand::energy(300.0, 8_738.0, 36.38),
                    AbsorptionBand::energy(708.9, 2_419.0, 8.018),
                    AbsorptionBand::energy(307.4, 8_241.9, 2.336),
                    AbsorptionBand::energy(1_180.0, 2_500.0, 2.126),
                    AbsorptionBand::energy(524.8, 7_326.0, 0.603_9),
                    AbsorptionBand::energy(479.6, 6_873.2, 0.328_3),
                    AbsorptionBand::energy(539.3, 2_079.8, 0.236_4),
                ],
            ),
            // c lies in the table plane (`Vec3::X`, not the Vec3::Y default), since tourmaline
            // is cut with the table parallel to c. Looking along c, the o-ray (E perpendicular
            // to c) is the dark ray in this Fe-bearing preset, so a table normal along c would
            // be the dark view. Three things are kept apart: the table plane's orientation,
            // the propagation direction (the face-up view, perpendicular to c), and the
            // polarisation (E perpendicular or parallel to c, per eigenmode).
            c_axis: Vec3::X,
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
        }
    }
}
