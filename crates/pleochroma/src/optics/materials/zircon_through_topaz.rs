//! Built-in material data: Zircon, Alexandrite, Topaz.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// The Zircon, Alexandrite and Topaz entries of the built-in material table. Kept apart from
    /// `built_in_materials_diamond_through_emerald` to stay under clippy's function-length lint.
    #[expect(clippy::too_many_lines, reason = "a data table of built-in materials")]
    pub(super) fn built_in_materials_zircon_through_topaz() -> Vec<Self> {
        vec![
            // Zircon (High Zircon, ZrSiO4)
            //
            // n_o = 1.925 and n_e = 1.984 (birefringence_delta 0.0590): Handbook of Mineralogy
            // (2001), International Gem Society and the GIA gem-quality reference pair.
            //
            // No primary Sellmeier or Cauchy fit exists for zircon, so the dispersion is a
            // 2-parameter Cauchy fit with lower confidence (to cross-check by hand). It takes the
            // B-G interval 0.039 and converts it to F-C with the B-G to F-C ratio 0.579 of the
            // Emerald entry: Delta n(F-C) = 0.02258, Abbe V_d = 40.96. n_d sets the critical
            // angle, so a change of more than 0.002 in n_d is significant.
            Self {
                name: "Zircon".to_string(),
                crystal_system: CrystalSystem::Tetragonal,
                optical_character: OpticalCharacter::UniaxialPositive,
                dispersion: DispersionModel::Cauchy {
                    a: 1.890_963,
                    b: 0.011_820,
                    c: 0.0,
                },
                birefringence_delta: 0.0590,
                // Chromophore: trace U4+ substituting for Zr4+ gives narrow lines rather than a
                // broad transition-metal band. Sources: P.E. Fielding (1970) and Nasdala et al.,
                // "Spectroscopic methods applied to zircon," Reviews in Mineralogy & Geochemistry
                // 53 (2003): dominant line at 653 nm; weaker at 691, 589, 562 and 537 nm.
                //
                // Per axis, as the lines are polarised (Caltech GRR 3106, Mogok zircon,
                // E perp c and E par c): each line is an energy Gaussian at its measured peak with
                // the measured FWHM (3-7 nm), and its amplitude is the peak above the local
                // background. The broad radiation-damage continuum is not included; it makes the
                // brown, red-brown and green zircons and belongs to the `zircon.*` color variants.
                // Weak e-ray features below 0.08 per mm (465, 484, 538 nm) are left out.
                //
                // Peaks are the measured per-mm values x 9.905 mm (the 7 mm face-up path) x 0.05.
                // With the continuum removed the stone is colorless apart from the lines. Full
                // strength rendered lilac (C* about 11) where white zircon photos are colorless
                // (C* 0.7): a colorless gem zircon holds far less uranium (about 120 ppm at
                // Ratanakiri, Zeug et al. 2018) than the Mogok crystal.
                // Calibration record: docs/chromophores/calibration-records.md#zircon
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        // o-ray (E perp c)
                        AbsorptionBand::energy(483.4, 120.0, 0.157_5),
                        AbsorptionBand::energy(515.7, 170.0, 0.192),
                        AbsorptionBand::energy(537.2, 120.0, 0.145_5),
                        AbsorptionBand::energy(562.3, 100.0, 0.087_5),
                        AbsorptionBand::energy(589.7, 100.0, 0.168),
                        AbsorptionBand::energy(653.4, 105.0, 0.57),
                        AbsorptionBand::energy(659.5, 150.0, 0.153_5),
                        AbsorptionBand::energy(690.9, 65.0, 0.337),
                    ],
                    vec![
                        // e-ray (E par c)
                        AbsorptionBand::energy(511.5, 150.0, 0.164),
                        AbsorptionBand::energy(542.0, 120.0, 0.091_5),
                        AbsorptionBand::energy(589.1, 100.0, 0.239),
                        AbsorptionBand::energy(616.0, 150.0, 0.174_5),
                        AbsorptionBand::energy(652.8, 110.0, 0.271_5),
                        AbsorptionBand::energy(660.5, 80.0, 0.216),
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
            // Alexandrite (Chrysoberyl, BeAl2O4:Cr)
            //
            // Dispersion: J. Walling et al., "Tunable alexandrite lasers," IEEE J. Quantum
            // Electron. 16, 1302 (1980), n(alpha) Sellmeier (refractiveindex.info
            // "BeAl2O4: Walling-alpha"), valid 0.25-2.6 um:
            //   n^2 = 1.78522 + 1.21202*l^2/(l^2-0.01262) - 0.01681*l^2
            // The linear term is a pole at c = 1000 um^2 (b = 16.81), outside the visible range;
            // the constant is a c = 0 pole. Verified: n_d = 1.742730, Delta n(F-C) = 0.010051,
            // Abbe V_d = 73.90.
            //
            // Walling's alpha, beta and gamma files do not sort by index magnitude. Sorted into
            // the optical indicatrix (n_alpha <= n_beta <= n_gamma) at the D line: n_alpha =
            // 1.740779 (the "gamma" file), n_beta = 1.742730 (the "alpha" file, used above),
            // n_gamma = 1.748360 (the "beta" file). birefringence_delta = n_gamma - n_alpha.
            Self {
                name: "Alexandrite".to_string(),
                crystal_system: CrystalSystem::Orthorhombic,
                optical_character: OpticalCharacter::BiaxialPositive,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [0.785_22, 1.212_02, 16.81],
                    c: [0.0, 0.012_62, 1000.0],
                },
                // n_gamma - n_alpha at the D line of the two principal curves below.
                birefringence_delta: 0.007_581,
                // Trichroism (E.F. Farrell & R.E. Newnham, "Crystal-field spectra of chrysoberyl,
                // alexandrite, peridot, and sinhalite", American Mineralogist 50 (1965); 77 K,
                // space group Pnma): yellow along a, green along b, red along c. Two Cr3+ band
                // systems, 4A2->4T2 at 560-595 nm and 4A2->4T1 at 410-440 nm, straddle Neuhaus's
                // 580 and 415 nm red-green limits: that is the daylight-green, incandescent-red
                // change. The per-axis split below adds direction dependence on top.
                //
                // Axis mapping: the slots of `AbsorptionTensor::biaxial` are the n_alpha, n_beta
                // and n_gamma directions. With the indicatrix sort above, alpha is crystal c
                // (red), beta is crystal a (yellow) and gamma is crystal b (green, `c_axis`).
                //
                // Beta (E||a) and gamma (E||b) are measured: Caltech GRR 874 (Lake Manyara,
                // 0.717 mm; G. R. Rossman, CC BY-NC 4.0), constants removed, the Cr3+ R line
                // interpolated, at a Cr factor of 0.15, plus the Fe3+ bands of GRR 171 at 0.10.
                // Alpha (E||c) is not measured and is estimated from gamma: its 4T2 half moved
                // from 597 to 568 nm at 0.6 x and its 4T1 half from 415 to 418 nm at 1.0 x (ratios
                // from F&N Fig. 4). The two factors give a green by daylight and a purplish red
                // under 3200 K. Six energy Gaussians per ray, per mm, x 9.905 mm (the face-up
                // path of a 7 mm stone) in model units; the widths are FWHM
                // (`AbsorptionBand::energy`). The 4T2 order at 595 nm (gamma > alpha > beta) and
                // the 4T1 maximum on beta at 422 nm are F&N's.
                // Fit record: docs/color_variants/fit_records/corundum_spinel_chrysoberyl.md#alexandrite-material-default
                absorption: AbsorptionTensor::biaxial(
                    vec![
                        // alpha -- crystal c-axis, RED (estimated from gamma, see above)
                        AbsorptionBand::energy(401.1, 3_399.2, 3.532),
                        AbsorptionBand::energy(578.0, 1_958.7, 3.427),
                        AbsorptionBand::energy(435.0, 2_527.7, 3.254),
                        AbsorptionBand::energy(537.9, 2_416.8, 2.146),
                        AbsorptionBand::energy(444.2, 738.5, 0.824_3),
                        AbsorptionBand::energy(683.2, 988.1, 0.035_96),
                    ],
                    vec![
                        // beta -- crystal a-axis, YELLOW: measured (GRR 874 + GRR 171)
                        AbsorptionBand::energy(414.5, 3_461.3, 6.962),
                        AbsorptionBand::energy(437.7, 1_649.1, 3.006),
                        AbsorptionBand::energy(570.6, 1_969.3, 1.135),
                        AbsorptionBand::energy(534.9, 2_592.7, 0.626_8),
                        AbsorptionBand::energy(467.6, 528.4, 0.465_3),
                        AbsorptionBand::energy(641.5, 2_042.3, 0.120_5),
                    ],
                    vec![
                        // gamma -- crystal b-axis, GREEN (along `c_axis` below): measured
                        AbsorptionBand::energy(410.4, 3_665.7, 5.016),
                        AbsorptionBand::energy(585.0, 1_961.1, 4.21),
                        AbsorptionBand::energy(615.3, 1_539.3, 3.237),
                        AbsorptionBand::energy(441.0, 1_375.5, 2.118),
                        AbsorptionBand::energy(565.6, 3_029.9, 1.54),
                        AbsorptionBand::energy(469.7, 499.9, 0.213),
                    ],
                ),
                c_axis: Vec3::Y,
                // n_beta - n_alpha at the D line, from the Walling 1980 trio above:
                // 1.742730 - 1.740779 = 0.001951. The curves in `biaxial_principal_dispersion`
                // give the spreads at other wavelengths.
                biaxial_delta_beta_alpha: Some(0.001_951),
                alpha_axis: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                zoning: None,
                inclusions: None,
                uniaxial_extraordinary_dispersion: None,
                // The other two principal indices, from their own Walling 1980 fits (distant-pole
                // forms, checked at 589.3 nm). `dispersion` above stays n_beta.
                //   n_alpha: n^2 = 1.70690 + 1.28477 l^2/(l^2-0.01169) - 0.01760 l^2
                //   n_gamma: n^2 = 1.58829 + 1.42711 l^2/(l^2-0.01113) - 0.01696 l^2
                // The curves do not cross between 380 and 780 nm; n_gamma - n_alpha falls from
                // 0.00845 at 380 nm to 0.00735 at 780 nm.
                biaxial_principal_dispersion: Some([
                    DispersionModel::Sellmeier3 {
                        b: [0.7069, 1.28477, 17.6],
                        c: [0.0, 0.01169, 1000.0],
                    },
                    DispersionModel::Sellmeier3 {
                        b: [0.58829, 1.42711, 16.96],
                        c: [0.0, 0.01113, 1000.0],
                    },
                ]),
            },
            // Topaz (Al2SiO4(F,OH)2)
            //
            // No primary Sellmeier or Cauchy fit exists for topaz (none in refractiveindex.info,
            // no visible-range study found), so the dispersion is a 2-parameter Cauchy fit with
            // lower confidence (to cross-check by hand). n_d = 1.627178 lies within International
            // Gem Society's cited range 1.61-1.638. Its B-G interval 0.0141 is converted to F-C
            // with the B-G to F-C ratio 0.579 of the Emerald entry: Delta n(F-C) = 0.00816,
            // Abbe V_d = 76.83.
            Self {
                name: "Topaz".to_string(),
                crystal_system: CrystalSystem::Orthorhombic,
                optical_character: OpticalCharacter::BiaxialPositive,
                dispersion: DispersionModel::Cauchy {
                    a: 1.614_872,
                    b: 0.004_273,
                    c: 0.0,
                },
                birefringence_delta: 0.0080,
                // Models London and Sky Blue topaz (near-colorless natural topaz, irradiated then
                // heat-treated). The color is an irradiation-induced color center (a trapped
                // electron or hole defect), not a transition-metal transition: a broad band near
                // 610-620 nm (red-orange) leaves blue transmitted. Source: K. Nassau, "Gemstone
                // Enhancement" (2nd ed., Butterworth-Heinemann, 1994). Colorless natural topaz is
                // an empty band set; imperial topaz (Cr3+ near 540 nm) would need its own entry.
                //
                // Per axis: alpha (E par a) and beta (E par b) are measured on the electron-
                // irradiated Caltech GRR 825c (Brazil, 5.63 mm, clean blue part; the Swiss and sky
                // blue type, band peak at 626 nm). Constant baselines (the 1200-1290 nm level) are
                // removed and each ray is fitted with energy Gaussians over 330-950 nm. Gamma is
                // not measured and is taken equal to beta (estimated). The broad 400 nm band and
                // the irradiation brown center, which commercial heat treatment removes, are left
                // out. Strength: 2 x the crystal per mm, x 9.905 mm to model units, then
                // `.scaled(0.7)`: 1.4 x in total. The 0.7 was tuned against dealer photos of blue
                // topaz: their faceted colored-pixel saturation is 0.48, the render at 1.0 gives
                // 0.86, and 0.7 takes half the log gap between them.
                // Calibration record: docs/chromophores/calibration-records.md#topaz
                // Calibration record: docs/chromophores/calibration-records.md#saturation-round-2026-11-04
                absorption: AbsorptionTensor::biaxial(
                    vec![
                        // alpha
                        AbsorptionBand::energy(314.0, 5_337.0, 17.6),
                        AbsorptionBand::energy(626.2, 2_864.1, 1.883),
                        AbsorptionBand::energy(660.0, 8_180.3, 0.237_7),
                    ],
                    vec![
                        // beta
                        AbsorptionBand::energy(294.3, 6_720.0, 30.94),
                        AbsorptionBand::energy(604.7, 3_628.3, 0.921),
                        AbsorptionBand::energy(800.0, 5_581.6, 0.302_7),
                    ],
                    vec![
                        // gamma
                        AbsorptionBand::energy(294.3, 6_720.0, 30.94),
                        AbsorptionBand::energy(604.7, 3_628.3, 0.921),
                        AbsorptionBand::energy(800.0, 5_581.6, 0.302_7),
                    ],
                )
                .scaled(0.7),
                c_axis: Vec3::Y,
                // n_beta - n_alpha at the D line, lower confidence (no primary topaz data).
                // The Gemology Project's ranges (alpha 1.606-1.634, beta 1.609-1.637, gamma
                // 1.616-1.644, across the fluorine/hydroxyl composition range) have midpoints
                // 1.620, 1.623 and 1.630, which put beta 0.30 of the way from alpha to gamma:
                // delta_beta_alpha = 0.30 x birefringence_delta = 0.0024.
                biaxial_delta_beta_alpha: Some(0.0024),
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
