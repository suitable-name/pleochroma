//! Built-in material data: Pyrope, Almandine, Spessartine garnets.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

/// Spessartine's absorption (Mn2+ lines and bands plus a UV charge-transfer edge), fitted to a
/// measured spectrum; see the comment at the entry below. A separate function so the entry stays
/// within clippy's function-length limit.
fn spessartine_absorption() -> AbsorptionTensor {
    AbsorptionTensor::isotropic(vec![
        // UV / O->Mn charge-transfer edge (stand-in, centre on the bound)
        AbsorptionBand::energy(320.0, 7_023.0, 27.4),
        // Mn2+ 6A1 -> 4E,4A1 (4G) group
        AbsorptionBand::energy(411.0, 556.0, 14.2),
        AbsorptionBand::energy(421.0, 419.0, 6.62),
        AbsorptionBand::energy(429.0, 659.0, 8.24),
        // Mn2+ 4T2 (G)
        AbsorptionBand::energy(462.0, 1_551.0, 2.23),
        AbsorptionBand::energy(485.0, 811.0, 1.59),
        // Mn2+ 4T1 (G) + Fe2+ 505/520
        AbsorptionBand::energy(523.0, 1_971.0, 1.57),
        // weak 550-620 nm tail (sets T(600) = 0.86; without it L*80 C*78)
        AbsorptionBand::energy(590.0, 3_000.0, 0.132),
    ])
}

/// Pyrope's absorption: the measured Caltech GRR 1055 pyrope (Moses Rock, Utah; Ca0.33 Mn0.02
/// Mg2.13 Fe0.54 Al1.95 Cr0.07, 0.840 mm, CSV `variant_pyrope_chrome_iso.csv`): Cr3+ near 410
/// and 570 nm, Fe2+ at 504 / 530 nm and the ultraviolet edge. The bands are the Chrome Pyrope
/// variant's fit (fit record F3-02, per mm at k = 0.17 of the sample's chromophores) at
/// k = 0.22, converted to model units by the 9.905 mm face-up path of a 7 mm stone; the widths
/// are the fit's sigma x 2.35482, since `AbsorptionBand::energy` takes the FWHM. See the
/// entry's comment for why.
///
/// 2026-11-03 (owner: "match the pictures of actual gemstones"): GRR 1055 at k 0.22 PLUS the Mn2+
/// of the Spessartine built-in at 0.6, refitted to eight energy Gaussians (dE2000 2.5 against the
/// sum). Nineteen dealer photos (`GemSelect`) have a median hue of 26 (colored pixels, quartiles
/// -6 / +8); GRR 1055 alone rendered h 13, this h 19 (Light Tent; more Mn or added almandine iron
/// did not go further). Swatch L*25 C*55 h 25.
fn pyrope_absorption() -> AbsorptionTensor {
    AbsorptionTensor::isotropic(vec![
        // measured spectra, see the note above
        AbsorptionBand::energy(368.8, 4_517.3, 8.688),
        AbsorptionBand::energy(563.8, 1_813.5, 8.655),
        AbsorptionBand::energy(410.8, 579.9, 7.911),
        AbsorptionBand::energy(422.5, 2_767.5, 7.37),
        AbsorptionBand::energy(422.8, 527.9, 4.79),
        AbsorptionBand::energy(488.7, 3_139.5, 3.225),
        AbsorptionBand::energy(430.9, 379.5, 3.043),
        AbsorptionBand::energy(628.9, 2_939.5, 0.594_7),
    ])
}

/// Almandine's absorption: the measured Caltech GRR 1776 almandine
/// ([Fe2.5 Mg0.3 Mn0.2 Ca0.1]Al2.0(SiO4)3, 0.213 mm, merged file, CSV
/// `variant_almandine_purplish_iso.csv`): the Fe2+ 504 / 527 / 576 nm triplet over a strong
/// blue-ultraviolet edge (Fe2+-Fe3+ charge transfer). The bands are the Purplish variant's fit
/// (fit record F3-03, per mm at k = 0.3 of the sample's Fe) at k = 0.35, converted to model
/// units by the 9.905 mm face-up path of a 7 mm stone; the widths are the fit's sigma x 2.35482,
/// since `AbsorptionBand::energy` takes the FWHM. See the entry's comment for why.
///
/// 2026-11-03 (owner: "match the pictures of actual gemstones"): GRR 1776 at k 0.35 PLUS the Mn2+
/// of the Spessartine built-in at 0.3 (an almandine carries a few percent of spessartine), refitted
/// to eight energy Gaussians (dE2000 4.9 against the sum; the refit is what rendered). Twenty-one
/// dealer photos (`GemSelect`) have a median hue of 29 (colored pixels), a clean Commons photo
/// 25; GRR 1776 alone rendered h 16, this h 26 (Light Tent). Swatch L*28 C*53 h 36.
fn almandine_absorption() -> AbsorptionTensor {
    AbsorptionTensor::isotropic(vec![
        // measured spectra, see the note above
        AbsorptionBand::energy(365.5, 3_243.6, 4.736),
        AbsorptionBand::energy(460.4, 9_131.5, 4.321),
        AbsorptionBand::energy(413.2, 916.3, 3.926),
        AbsorptionBand::energy(428.2, 482.5, 2.805),
        AbsorptionBand::energy(517.6, 1_453.4, 2.179),
        AbsorptionBand::energy(573.9, 847.4, 1.541),
        AbsorptionBand::energy(312.2, 655.3, 1.017),
        AbsorptionBand::energy(612.8, 354.8, 0.254_6),
    ])
}

impl GemMaterial {
    /// Three of the five garnet-group species (the other two, Grossular and
    /// Andradite, are [`Self::built_in_materials_garnets_grossular_and_andradite`] --
    /// split purely to keep each part under clippy's function-length lint). All five
    /// are cubic (isotropic, `birefringence_delta: 0.0`).
    ///
    /// No primary Sellmeier/Cauchy fit is available for any of the five
    /// (garnet-group gemstones are not present in the refractiveindex.info database);
    /// every dispersion fit below is a 2-parameter Cauchy solved from `n_d` and its
    /// "dispersion" figure treated as the standard gemological Fraunhofer B-G
    /// interval, converted to F-C via the same 0.579 ratio as every other gemological
    /// entry in this file -- see
    /// [`Self::built_in_materials_aquamarine_through_citrine`]'s doc comment for the
    /// full convention note. LOWER CONFIDENCE tier, same as Zircon/Topaz/
    /// Tourmaline/Tanzanite/Emerald above; flagged for human cross-check.
    pub(super) fn built_in_materials_garnets_pyrope_through_spessartine() -> Vec<Self> {
        vec![
            // Pyrope (Mg3Al2(SiO4)3): n_d=1.714, B-G 0.022 -> Delta n(F-C) = 0.01274,
            // A=1.714-0.006668/0.5893^2=1.694804, B=0.006668. Abbe V_d ~ 56.05.
            Self {
                name: "Pyrope Garnet".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Cauchy {
                    a: 1.694_804,
                    b: 0.006_668,
                    c: 0.0,
                },
                birefringence_delta: 0.0,
                // Chromophore: Fe2+ (the 504/521nm spin-forbidden bands, GIA G&G
                // Winter 2015; Caltech GRR 83 and GRR 131) and Cr3+ (410 and 570 nm),
                // over the ultraviolet edge that closes the blue. MEASURED since 2026-11-03:
                // the Caltech GRR 1055 pyrope at k = 0.22 (`pyrope_absorption`). The two
                // tuned green bands it replaces absorbed nothing in the blue, so red AND blue
                // light passed and every render was a hot magenta (Light Tent L* 42 C* 60
                // h 323) where real pyrope is a deep red. The measured spectrum renders a deep
                // red (Pentalume Sapphire, Light Tent: k 0.17 L* 29 C* 30 h 12, k 0.3 L* 20
                // C* 22 h 12; a faceted pyrope photo on white measures L* 27 h 29). k 0.22
                // sits between, a little deeper than the Chrome Pyrope variant (k 0.17).
                // 2026-11-04 (owner: "The render seems to err toward saturated"): x 0.45 of
                // `pyrope_absorption`. Colored-pixel saturation s_uv: the faceted dealer photos 1.12
                // (L* 41, h 29, a brownish red), the Commons photos 0.81 and 1.76, the x 1 render
                // 1.94 at L* 22 (x 0.5: 1.50 at L* 32, h 17). Lighter only turns it pinker (h 19 ->
                // 16); the photos' brown (a grey mask under the red) needs a broad band the 8-band
                // budget has no room for. Swatch L*46 C*36 h 16.
                // Calibration record: docs/chromophores/calibration-records.md#saturation-round-2026-11-04
                absorption: pyrope_absorption().scaled(0.45),
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
            // Almandine (Fe3Al2(SiO4)3): n_d=1.790, B-G 0.024 -> Delta n(F-C) =
            // 0.01390, A=1.790-0.007274/0.5893^2=1.769057, B=0.007274. Abbe V_d~56.86.
            Self {
                name: "Almandine Garnet".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Cauchy {
                    a: 1.769_057,
                    b: 0.007_274,
                    c: 0.0,
                },
                birefringence_delta: 0.0,
                // Chromophore: the diagnostic Fe2+ 504/527/576nm triplet (the standard
                // gemological identification feature for this species) over a strong
                // blue-ultraviolet edge. MEASURED since 2026-11-03: the Caltech GRR 1776
                // almandine at k = 0.35 (`almandine_absorption`). The three tuned green bands
                // it replaces left the blue open, so every render was a hot magenta (Light
                // Tent L* 51 C* 68 h 329) where real almandine is a deep, slightly violet red.
                // The measured spectrum renders that red (Pentalume Sapphire, Light Tent:
                // k 0.3 L* 25 C* 30 h 13, k 0.5 L* 18 C* 14 h 13; a faceted almandine photo on
                // white measures L* 24 C* 28 h 25). k 0.35 is a touch deeper than the Purplish
                // variant (k 0.3).
                absorption: almandine_absorption(),
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
            // Spessartine (Mn3Al2(SiO4)3): n_d=1.800, B-G 0.027 -> Delta n(F-C) =
            // 0.01563, A=1.800-0.008182/0.5893^2=1.776445, B=0.008182. Abbe V_d~51.19.
            Self {
                name: "Spessartine Garnet".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Cauchy {
                    a: 1.776_445,
                    b: 0.008_182,
                    c: 0.0,
                },
                birefringence_delta: 0.0,
                // Chromophore: Mn2+ spin-forbidden lines at 411/421/429 nm (the
                // near-UV cut-off), the 462 and 485 nm Mn2+ bands and the weak 523 nm
                // Mn2+/Fe2+ band, plus a UV charge-transfer edge (stand-in band at
                // 320 nm). FITTED 2026-10-19 by a bounded
                // least-squares energy-Gaussian fit (log residual, 380-780 nm) to the
                // Caltech Mineral Spectroscopy Server spectrum of spessartine GRR 43
                // (Rutherford Mine, Amelia, Virginia; 2.165 mm; baseline 0.030
                // removed),
                // `data/reference_spectra/caltech_spessartine_grr43_2p165mm.csv`,
                // scaled to the 8.8 mm face-up path of a 7 mm stone at the sample's
                // own (near end-member, Fe-poor "Mandarin") composition. Swatch
                // (7 mm, D65): L*77 C*72 h=66; p = 1.5: h 63, p = 2: h 61. The
                // Fe-richer GRR 72 from the same mine gives L*51 h 62.
                // Calibration record: docs/chromophores/calibration-records.md#spessartine
                absorption: spessartine_absorption(),
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

    /// The color-change garnet (Tanzanian pyrope-spessartine), split out of
    /// [`Self::built_in_materials_garnets_pyrope_through_spessartine`] purely to keep each
    /// part under clippy's function-length lint. Same 2-parameter Cauchy convention as the
    /// three species above.
    pub(super) fn built_in_materials_garnet_color_change() -> Vec<Self> {
        vec![
            // Color-Change Garnet: pyrope-spessartine solid solution (Mg,Mn)3Al2(SiO4)3 with
            // traces of Cr3+ and/or V3+, Tanzania (Umba valley, Tanga; also Lindi/Tunduru).
            // Cubic, isotropic. SG ~3.85 (comment only, not stored on the material).
            //
            // n_d=1.760 (typical range 1.755-1.765), roughly the midpoint of pyrope 1.714 and
            // spessartine 1.800 (cross-check: `chromophore::catalogue::garnet_optics(
            // &[("spessartine", 0.5)])` fills the remainder with pyrope and returns the
            // linear mix 0.5*1.714+0.5*1.800 = 1.757, SG 3.88; the stored 1.760 is the
            // typical gemmological value for the Tanzanian material and stays).
            // Dispersion B-G 0.025 (between pyrope 0.022 and spessartine 0.027) -> Delta n(F-C)
            // = 0.025*0.579 = 0.014475; B = 0.014475/(1/0.4861^2-1/0.6563^2) =
            // 0.014475/1.91053 = 0.007576; A = 1.760-0.007576/0.5893^2 = 1.760-0.021816 =
            // 1.738184. Abbe V_d ~ 52.5.
            Self {
                name: "Color-Change Garnet (Pyrope-Spessartine)".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Cauchy {
                    a: 1.738_184,
                    b: 0.007_576,
                    c: 0.0,
                },
                birefringence_delta: 0.0,
                // Chromophores: the Mn2+ edge and spin-forbidden group (386 / 426 / 432 nm and
                // the 449-461 nm pair) close the violet and blue; Fe2+ near 493 nm; and the
                // V3+/Cr3+ 4T2 band at 581 nm holds the yellow-green below the open red window.
                // Daylight (D65) leaves a khaki yellow; the 3200 K lamp, weak in the green, lets
                // the red window pull the stone to orange-pink.
                //
                // 2026-11-03 (owner: "match the pictures of actual gemstones"): the measured
                // Tunduru spectrum (Caltech GRR 2225, the Tunduru variant's fit) with its 581 nm
                // band x 0.25 and the whole x 1.2 of the variant's strength, in model units (x
                // 9.905). Twenty-four GemSelect stones (the common Tanzanian type) read khaki
                // yellow in day light (colored hue 86, L* 59) and 16 deg more orange under a
                // lamp; the renders give h 86, L* 60 and h 70 under the lamp. The hand-tuned
                // bands before (a strong 573 nm band between a blue-green and a red window)
                // rendered violet, h 299, the rare blue-to-purple type that the Tunduru and
                // Stronger Change variants still offer.
                absorption: AbsorptionTensor::isotropic(vec![
                    AbsorptionBand::energy(386.64, 4_092.0, 8.935),
                    AbsorptionBand::energy(425.55, 1_412.7, 2.49),
                    AbsorptionBand::energy(432.28, 256.0, 0.613_1),
                    AbsorptionBand::energy(449.45, 618.4, 0.216_8),
                    AbsorptionBand::energy(460.80, 653.5, 0.277_3),
                    AbsorptionBand::energy(492.98, 3_054.4, 0.750_7),
                    AbsorptionBand::energy(580.85, 2_762.2, 0.635_9),
                ]),
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
