//! Built-in material data: Aquamarine, Morganite, Chrysoberyl (Yellow).

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Aquamarine, Morganite and Chrysoberyl (Yellow): beryl and chrysoberyl entries, sharing
    /// the host-mineral optics of Emerald and Alexandrite.
    ///
    /// # Dispersion convention
    ///
    /// The stored convention is the Fraunhofer F-C interval (486.1-656.3 nm). Gemological
    /// "dispersion" figures are B-G and are converted to F-C with the 0.579 ratio (range
    /// 0.569-0.587). Entries with a named primary dispersion source use that curve directly.
    ///
    /// Sphene and rutile are not in this function: they have their own entries
    /// ([`Self::built_in_material_titanite`], [`Self::built_in_material_rutile`]), because their
    /// birefringence is beyond what the biaxial and uniaxial machinery was checked against.
    #[expect(clippy::too_many_lines, reason = "a data table of built-in materials")]
    pub(super) fn built_in_materials_aquamarine_through_citrine() -> Vec<Self> {
        vec![
            // Aquamarine (Beryl, Be3Al2(SiO3)6:Fe2+). Beryl's dispersion is set by the host
            // lattice, so it keeps Emerald's Cauchy `b` and retargets only the constant `a`:
            // a = 1.577 - 0.004273/0.5893^2 = 1.564698 gives n_d 1.577.
            Self {
                name: "Aquamarine".to_string(),
                crystal_system: CrystalSystem::Hexagonal,
                optical_character: OpticalCharacter::UniaxialNegative,
                dispersion: DispersionModel::Cauchy {
                    a: 1.564_698,
                    b: 0.004_273,
                    c: 0.0,
                },
                birefringence_delta: -0.0060,
                // Chromophore: Fe2+-Fe3+ intervalence band at ~600-680 nm polarised
                // along c (e-ray, the blue), Fe2+ at ~830 nm (strong in the o-ray,
                // where only its 650-780 nm tail is visible), weak Fe3+ at 428 nm and
                // a near-UV edge in both rays. Energy-Gaussian fits (the NIR band centres sit
                // on their bounds, only their tails matter) to the two polarised Caltech
                // spectra of aquamarine GRR 1488 (5.531 mm), `data/reference_spectra/
                // caltech_aquamarine_grr1488_{eperpc,eparc}_5p531mm.csv`, at the crystal's
                // own iron content over the 8.8 mm face-up path of a 7 mm
                // stone. The face-up view mixes the o- and e-ray and reads blue: `c_axis` is
                // in the table plane, so the blue e-ray shows face up.
                // Mix: 0.6 x GRR 1488 plus 0.4 x the Greenish variant (estimated Fe3+), which
                // matches the dealer photos' median hue (241). Strength: `.scaled(0.3)` for the
                // colored-pixel saturation of the dealer photos (s_uv 0.30).
                // Swatch L*92 C*10 h 233.
                // Calibration record: docs/chromophores/calibration-records.md#aquamarine
                // Calibration record: docs/chromophores/calibration-records.md#saturation-round-2026-11-04
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        // o-ray (E perp c)
                        AbsorptionBand::energy(847.1, 2_828.2, 3.894),
                        AbsorptionBand::energy(822.7, 2_158.1, 1.01),
                        AbsorptionBand::energy(356.9, 6_124.2, 0.947_4),
                        AbsorptionBand::energy(315.9, 13_576.9, 0.449_5),
                        AbsorptionBand::energy(680.0, 6_336.0, 0.178_3),
                        AbsorptionBand::energy(429.2, 1_392.4, 0.134_5),
                        AbsorptionBand::energy(640.3, 3_372.7, 0.092_57),
                    ],
                    vec![
                        // e-ray (E par c)
                        AbsorptionBand::energy(823.8, 5_651.0, 6.622),
                        AbsorptionBand::energy(604.1, 4_566.9, 2.527),
                        AbsorptionBand::energy(381.5, 4_125.2, 0.736_1),
                        AbsorptionBand::energy(314.9, 11_859.4, 0.522),
                        AbsorptionBand::energy(428.0, 595.8, 0.229_3),
                        AbsorptionBand::energy(842.9, 5_321.4, 0.149_3),
                        AbsorptionBand::energy(295.5, 7_226.8, 0.119_7),
                    ],
                )
                .scaled(0.3),
                // c in the table plane (Vec3::X), not the Vec3::Y default, as for Tourmaline.
                // With c along the view axis both rays are the near-colorless o-ray, so the
                // blue e-ray would never show. Test:
                // `aquamarine_face_up_sees_the_blue_ray_with_c_axis_in_table_plane`
                // in `tests/raytracer_tests/pleochroic_orientation.rs`.
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
            },
            // Morganite (Beryl, Be3Al2(SiO3)6:Mn3+) -- same beryl host as Aquamarine
            // immediately above (both varieties share n_d=1.577), so this entry's
            // dispersion is bit-identical to Aquamarine's.
            Self {
                name: "Morganite".to_string(),
                crystal_system: CrystalSystem::Hexagonal,
                optical_character: OpticalCharacter::UniaxialNegative,
                dispersion: DispersionModel::Cauchy {
                    a: 1.564_698,
                    b: 0.004_273,
                    c: 0.0,
                },
                birefringence_delta: -0.0060,
                // Chromophore: Mn3+ on the Al site of beryl, a single broad band in the
                // green (GIA G&G Fall 2024 places it near 567 nm), leaving red and violet
                // transmitted. Pleochroism is weak: light red to light violet.
                //
                // Per-axis tensor. No digitised morganite spectrum exists, so the shape is
                // borrowed from the same ion on the same site: the Caltech red beryl GRR 602
                // (Utah) fit that the `emerald.red_beryl` variant carries, scaled by 0.202
                // over the 9.905 mm face-up path. It is folded with the Peach variant's Fe3+
                // (GRR 309) at 0.35 / 0.65: the pink set alone rendered h 348, where the dealer
                // photos' salmon pink has hue 24. Strength: `.scaled(0.6)` for the
                // colored-pixel saturation of the dealer photos (s_uv 0.28).
                // Swatch L*89 C*12 h 18.
                // Calibration record: docs/chromophores/calibration-records.md#morganite
                // Calibration record: docs/chromophores/calibration-records.md#saturation-round-2026-11-04
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        // o-ray (E perp c)
                        AbsorptionBand::energy(312.7, 8_241.9, 0.819),
                        AbsorptionBand::energy(440.0, 1_832.0, 0.035_7),
                        AbsorptionBand::energy(495.9, 3_198.0, 0.465_9),
                        AbsorptionBand::energy(554.6, 1_954.9, 0.417_9),
                        AbsorptionBand::energy(295.5, 10_676.5, 3.991),
                        AbsorptionBand::energy(847.7, 1_177.4, 0.016_28),
                    ],
                    vec![
                        // e-ray (E par c)
                        AbsorptionBand::energy(319.9, 8_241.9, 0.836_5),
                        AbsorptionBand::energy(440.0, 3_532.2, 0.115_8),
                        AbsorptionBand::energy(543.6, 3_487.2, 0.725_2),
                        AbsorptionBand::energy(315.5, 13_658.0, 1.776),
                        AbsorptionBand::energy(403.9, 3_297.7, 1.874),
                        AbsorptionBand::energy(902.7, 3_532.2, 0.303),
                    ],
                )
                .scaled(0.6),
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
            // Chrysoberyl (Yellow, BeAl2O4:Fe3+), sharing Alexandrite's indicatrix.
            // Reuses Alexandrite's Sellmeier3 shape (both non-constant poles unchanged) with
            // only the leading constant pole
            // (`b[0]`, a c=0 pole contributing a wavelength-independent additive
            // constant to n^2-1) retargeted from Alexandrite's 0.78522 to 0.796473 so
            // n_d hits this variety's own 1.746 target. A constant added to n^2 scales
            // every index difference by n_old/n_new (0.9997 here), so the spreads are
            // preserved to about 1e-5, not exactly: 0.007581 (Alexandrite) becomes
            // 0.007567. Delta n(F-C) therefore stays within that margin of
            // Alexandrite's own primary-sourced 0.010051 (Abbe V_d = 74.22).
            Self {
                name: "Chrysoberyl (Yellow)".to_string(),
                crystal_system: CrystalSystem::Orthorhombic,
                optical_character: OpticalCharacter::BiaxialPositive,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [0.796_473, 1.212_02, 16.81],
                    c: [0.0, 0.012_62, 1000.0],
                },
                // n_gamma - n_alpha at the D line of the principal curves below (the published
                // Walling spreads, not the gem figure 0.0090).
                birefringence_delta: 0.007_567,
                // Chromophore: Fe3+ on the Al mirror site: sharp spin-forbidden bands at 440,
                // 385, 378, 368 and 355 nm plus very broad weak ones near 500 and 615 nm
                // (Farrell & Newnham, Am. Mineral. 50, 1972), leaving yellow-red transmitted.
                //
                // Per-axis tensor. Axis mapping as for Alexandrite: alpha = crystal c
                // (`o_ray`), beta = crystal a (`beta_ray`), gamma = crystal b (`e_ray`, along
                // `c_axis`). Beta and gamma come from the Caltech spectra of GRR 171 (Minas
                // Gerais, 1.193 mm). The gamma 441 nm band is
                // about 3.5 x the beta one, which contradicts the literature's "little or no
                // pleochroism", so the pleochroism is kept but unconfirmed. Alpha was never
                // measured and is estimated as the mean of the beta and gamma spectra.
                // The tensor is 0.5 x the Greenish Yellow variant (GRR 171 beta and gamma,
                // estimated alpha) plus 0.15 x the Alexandrite Cr3+ (GRR 874; a greenish-yellow
                // chrysoberyl carries traces of Cr and V), refitted to energy Gaussians (eight
                // for alpha and beta, seven for gamma). The mix matches the dealer photos' median
                // hue (109) at L* 64; more Cr greens it further but greys it, since Cr3+ absorbs at
                // 580 nm, the brightest part of the yellow. The gamma ray turns green (swatch h
                // 149), a trace of the alexandrite change. Swatch L*82 C*26 h 118. A golden yellow
                // is the Golden variant.
                // Calibration record: docs/chromophores/calibration-records.md#chrysoberyl-yellow
                absorption: AbsorptionTensor::biaxial(
                    vec![
                        // alpha
                        AbsorptionBand::energy(321.8, 5_258.4, 5.978),
                        AbsorptionBand::energy(437.5, 2_136.1, 0.885_7),
                        AbsorptionBand::energy(385.6, 3_140.5, 0.629_7),
                        AbsorptionBand::energy(442.5, 865.4, 0.601_3),
                        AbsorptionBand::energy(411.4, 2_575.1, 0.536),
                        AbsorptionBand::energy(576.3, 1_887.6, 0.515_6),
                        AbsorptionBand::energy(509.3, 6_019.1, 0.335_4),
                        AbsorptionBand::energy(536.3, 1_764.2, 0.169_6),
                    ],
                    vec![
                        // beta
                        AbsorptionBand::energy(353.0, 1_946.7, 16.11),
                        AbsorptionBand::energy(403.0, 3_021.6, 1.029),
                        AbsorptionBand::energy(435.8, 2_230.5, 0.912),
                        AbsorptionBand::energy(439.0, 1_000.0, 0.319_4),
                        AbsorptionBand::energy(558.0, 4_990.5, 0.180_5),
                        AbsorptionBand::energy(444.1, 6_698.3, 0.169_1),
                        AbsorptionBand::energy(565.6, 1_699.2, 0.105_9),
                        AbsorptionBand::energy(467.9, 476.2, 0.054_4),
                    ],
                    vec![
                        // gamma
                        AbsorptionBand::energy(234.5, 17_746.5, 16.68),
                        AbsorptionBand::energy(442.3, 941.0, 1.063),
                        AbsorptionBand::energy(589.0, 2_364.9, 0.985_3),
                        AbsorptionBand::energy(433.0, 2_429.7, 0.906_5),
                        AbsorptionBand::energy(614.0, 1_366.6, 0.283_1),
                        AbsorptionBand::energy(397.6, 1_659.7, 0.214_8),
                        AbsorptionBand::energy(510.2, 3_068.9, 0.170_6),
                    ],
                ),
                c_axis: Vec3::Y,
                // n_beta - n_alpha at the D line of the same curves (the Walling alpha
                // and gamma spreads carried onto this entry's retargeted n_beta):
                // 1.745955 - 1.744008 = 0.001947.
                biaxial_delta_beta_alpha: Some(0.001_947),
                alpha_axis: None,
                scattering_sigma_s: 0.0,
                scattering_g: 0.0,
                edge_rounding_radius: 0.0,
                absorption_path_scale: 1.0,
                absorption_unit: super::AbsorptionUnit::ModelUnit,
                zoning: None,
                inclusions: None,
                uniaxial_extraordinary_dispersion: None,
                // Alexandrite's n_alpha and n_gamma Walling 1980 curves, each with the same
                // +0.011253 shift of n^2-1 that retargets `dispersion` above. The shift scales
                // every index difference by about 0.9997, so the spreads stay within 1e-5 of
                // Alexandrite's. The three curves never cross between 380 and 780 nm.
                biaxial_principal_dispersion: Some([
                    DispersionModel::Sellmeier3 {
                        b: [0.718_153, 1.28477, 17.6],
                        c: [0.0, 0.01169, 1000.0],
                    },
                    DispersionModel::Sellmeier3 {
                        b: [0.599_543, 1.42711, 16.96],
                        c: [0.0, 0.01113, 1000.0],
                    },
                ]),
            },
        ]
    }
}
