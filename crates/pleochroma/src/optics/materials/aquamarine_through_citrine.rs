//! Built-in material data: Aquamarine, Morganite, Chrysoberyl (Yellow).

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// First of four new-species quarters: Aquamarine, Morganite (both beryl, sharing
    /// Emerald's host-mineral optics), Chrysoberyl (Yellow) (sharing Alexandrite's
    /// host-mineral indicatrix), Amethyst and Citrine (both quartz, sharing the
    /// colorless Quartz entry's exact Ghosh 1999 o/e Sellmeier pair above).
    ///
    /// # Dispersion-figure convention note (applies to every entry in this file)
    ///
    /// The stored convention is the Fraunhofer F-C interval (486.1-656.3 nm). Every
    /// gemological "dispersion" target below is the standard Fraunhofer B-G
    /// table value (e.g. Andradite/demantoid's famous "0.057," higher than diamond's
    /// own 0.044) and is converted B-G -> F-C by multiplying with the physically-derived
    /// 0.579 ratio (range 0.569-0.587) the Emerald/Zircon/Topaz/Tourmaline/Tanzanite
    /// entries above use, rather than taken at face value as F-C. The exception is entries with a genuine named
    /// primary dispersion source (Chrysoberyl/Aquamarine/Morganite, which reuse an
    /// already-F-C-fitted host-mineral curve directly; YAG, whose real Zelmon 1998
    /// Sellmeier is used as-is per this file's primary-source-wins rule; and the two
    /// optical glasses, whose Schott catalogue Abbe numbers are already true F-C).
    ///
    /// # Not in this list: Sphene
    ///
    /// Sphene (titanite) needs a `birefringence_delta` of about 0.12, beyond what this
    /// file's biaxial/uniaxial machinery had been verified against (Zircon's +0.059), so it
    /// was left out until it had measured indices and a check at that magnitude: it is
    /// [`Self::built_in_material_titanite`] since 2026-11-03.
    ///
    /// Rutile needs the full anisotropic (uniaxial, extremely high birefringence
    /// +0.2957) Fresnel treatment, compounded by rutile's very strong dispersion, so
    /// it gets its own dedicated entry -- see [`Self::built_in_material_rutile`].
    #[expect(clippy::too_many_lines, reason = "a data table of built-in materials")]
    pub(super) fn built_in_materials_aquamarine_through_citrine() -> Vec<Self> {
        vec![
            // Aquamarine (Beryl, Be3Al2(SiO3)6:Fe2+) -- same host mineral as Emerald
            // above, so it reuses Emerald's exact Cauchy dispersion shape (the `b`
            // coefficient, i.e. the same Delta n(F-C) curvature) with only the leading
            // constant `a` retargeted to this variety's own n_d -- beryl's dispersion
            // is a host-lattice property essentially independent of which trace ion
            // tints it. a = 1.577 - 0.004273/0.5893^2 = 1.564698. Verified: n_d =
            // 1.577000 (exact by construction), Delta n(F-C) = 0.00816 (identical to
            // Emerald's), Abbe V_d = 70.71.
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
                // a near-UV edge in both rays. FITTED 2026-10-19 by bounded
                // least-squares energy-Gaussian fits (log residual, 380-780 nm; the
                // NIR band centres sit on their bounds, only their tails matter) to
                // the two polarised Caltech Mineral Spectroscopy Server spectra of
                // aquamarine GRR 1488 (Lajes Pintada, Brazil; 5.531 mm; per-ray
                // baselines 0.683 / 0.866 removed), `data/reference_spectra/
                // caltech_aquamarine_grr1488_{eperpc,eparc}_5p531mm.csv`, at the
                // crystal's own iron content over the 8.8 mm face-up path of a 7 mm
                // stone. Swatch (7 mm, D65, (2o + e)/3): L*85 C*16 h=247 (o-ray
                // L*97 C*13: near-colorless; e-ray L*44 C*88 h 287: deep
                // violet-blue); p = 2: h 227. The face-up wave normal is the view
                // axis and `c_axis` is `Vec3::X` (c in the table plane, as for
                // Tourmaline, the way aquamarine is cut), so the face-up view mixes
                // the o- and e-ray and reads blue; with c along the view axis it
                // would see the near-colorless o-ray only.
                // Calibration record: docs/chromophores/calibration-records.md#aquamarine
                // 2026-11-03 (owner: "match the pictures of actual gemstones"): REPLACED GRR 1488
                // alone by 0.6 x it plus 0.4 x the Greenish variant (its estimated Fe3+), refitted.
                // Twenty dealer photos (GemSelect) have a median hue of 241 (colored pixels); GRR
                // 1488 alone rendered h 252, this mix h 240 (Light Tent). Swatch L*84 C*15 h 235
                // (it was L*85 C*16 h 247); o-ray L*94 C*12.
                // 2026-11-04 (owner: "The render seems to err toward saturated"): x 0.3 of that mix.
                // Colored-pixel saturation (CIELUV s_uv of the mean colored pixel, which exposure
                // and the photo set-up's dark reflections leave unchanged): the faceted dealer
                // photos 0.30, the Commons photos 0.15 and 0.31, the x 1 render 0.57; x 0.25 renders
                // 0.30 at h 233. Swatch L*92 C*10 h 233; o-ray L*98 C*4, e-ray L*78 C*35 h 245.
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
                // c_axis set INTO the table plane (Vec3::X) rather than the Vec3::Y
                // every other built-in defaults to, exactly as Tourmaline does -- a
                // deliberate override, not an oversight. Aquamarine's color sits in
                // the e-ray (E parallel to c): with c_axis=Y the face-up wave normal
                // is parallel to c, where both eigenmodes are the o-ray (L*97, almost
                // colorless), backwards from how aquamarine is cut and worn (c in the
                // table plane, so the blue ray shows face-up). See the test
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
                // green (the standard attribution for morganite's pink; GIA G&G Fall
                // 2024 puts it near 567nm, the Caltech GRR 2536b page text at ~560nm),
                // leaving red and violet transmitted. Pleochroism weak: "light red to
                // light violet" (the beryl reference).
                //
                // PER-AXIS since 2026-10-27 (owner decision: a built-in on an anisotropic
                // host never carries an isotropic tensor; the old one-band 540nm
                // wavelength Gaussian, sigma 45nm, peak 1.0, was a tuned stand-in).
                // No digital morganite spectrum exists (GRR 508 and 2536b are figures,
                // and none was digitised), so the SHAPE is borrowed from the same ion on
                // the same site in the same host: the Caltech GRR 602 red beryl (Wah Wah
                // Mts., Utah, 1.559 mm; per-ray constants removed), the free energy-Gaussian
                // fit of both rays that the `emerald.red_beryl` variant carries (o: UV edge
                // 313nm, 440, 496 and 555nm bands; e: UV edge 320nm, 440 and 544nm bands).
                // Strength: ONE factor 0.202 x the red beryl's per-mm absorption over every
                // band, the edge included (per mm x 0.202 x 9.905 mm, the face-up path of a
                // 7 mm stone). A first version kept one band per ray and could not hold the
                // 440 / 496 / 555nm structure of the donor: it came out violet (h 311)
                // where the donor at the same strength is pink. Swatch (7 mm, D65): with c
                // along the view the stone shows the o-ray, L*81 C*33 h 347 (a light
                // pink); unpolarised L*79 C*32 h 342; e-ray L*75 C*31 h 332, the documented
                // "light red to light violet" pleochroism (o pinker than e).
                // Calibration record: docs/chromophores/calibration-records.md#morganite
                // 2026-11-03 (owner: "match the pictures of actual gemstones"): REPLACED the pink
                // set alone by 0.35 x it plus 0.65 x the Peach variant (the same red beryl bands
                // plus measured Fe3+, GRR 309); bands that both carry are folded together. Sixteen
                // dealer photos of pink morganite (GemSelect) have a median hue of 24 (colored
                // pixels), a salmon pink; the pink set rendered h 348, this mix h 20 (Light Tent).
                // Swatch L*83 C*20 h 15 (it was L*79 C*32 h 342); o-ray h 2, e-ray h 54.
                // 2026-11-04 (owner: "The render seems to err toward saturated"): x 0.6 of that mix.
                // Colored-pixel saturation s_uv: the faceted dealer photos 0.28, the x 1 render 0.45
                // (x 0.25: 0.26 at h 31); the factor takes half the gap in log terms, since a dealer's
                // commercial morganite is paler than a typical one. Swatch L*89 C*12 h 18.
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
            // Chrysoberyl (Yellow, BeAl2O4:Fe3+) -- same host mineral as Alexandrite
            // above (a Cr3+-free, Fe3+-bearing chrysoberyl), sharing its indicatrix.
            // Reuses Alexandrite's exact Sellmeier3 dispersion shape (both
            // non-constant poles unchanged) with only the leading constant pole
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
                // n_gamma - n_alpha at the D line of the two principal curves below
                // (owner decision 2026-10-22: the published Walling spreads win over the
                // earlier gem figure 0.0090).
                birefringence_delta: 0.007_567,
                // Chromophore: Fe3+ substitution (rather than Alexandrite's Cr3+), on the
                // Al mirror site: sharp spin-forbidden bands at 440, 385, 378, 368 and
                // 355nm plus very broad weak ones near 500 and 615nm (Farrell & Newnham,
                // Am. Mineral. 50, 1972 (1965)), leaving yellow-red transmitted -- the
                // standard attribution for ordinary (non-color-change) yellow/green
                // chrysoberyl.
                //
                // PER-AXIS since 2026-10-27 (owner decision: a built-in on an anisotropic
                // host never carries an isotropic tensor; the old one-band 440nm
                // wavelength Gaussian, sigma 35nm, peak 1.4, was a tuned stand-in).
                // AXIS MAPPING as for Alexandrite above: alpha = crystal c (`o_ray`),
                // beta = crystal a (`beta_ray`), gamma = crystal b (`e_ray`, along
                // `c_axis`). Beta (E par a) and gamma (E par b) are the measured
                // room-temperature Caltech spectra of GRR 171 (greenish-yellow, Minas
                // Gerais, 1.193 mm; baselines A 0.255 / 0.16 removed): five energy
                // Gaussians each (a UV edge near 340nm, 369-378nm, the 436-441nm band,
                // a broad one at 490nm), non-linear least squares over 330-800nm, rms
                // 0.018 / 0.034 per mm; the gamma 441nm band is ~3.5 x the beta one,
                // which contradicts F&N's "little or no pleochroism" at 77 K, so the
                // pleochroism is kept but is unconfirmed. The slot assignment follows the
                // Pnma cell the CSV headers give (a 9.40, b 5.48, c 4.43 A; Caltech
                // "E||a = beta", "E||b = gamma", and the GRR 874 beta ray vibrates along
                // the 9.40 A axis): beta is the a axis and gamma the b axis, as in the
                // Walling sort of Alexandrite above. Alpha (E par c) was never measured
                // (no file on the server): ESTIMATED as the mean of the beta and gamma
                // spectra, refitted with the same five bands (rms 0.026).
                // Strength (owner decision 2026-10-27, a golden yellow): ONE factor 0.7 x
                // GRR 171's per-mm absorption over every band (per mm x 0.7 x 9.905 mm, the
                // face-up path of a 7 mm stone); the earlier 0.25 x was a pale greenish
                // yellow that duplicated the `greenish_yellow` variant. Swatch (7 mm, D65):
                // with gamma along the view the stone shows alpha and beta, L*74 C*48 h 88;
                // unpolarised mean L*72 C*53 h 83 (alpha L*72 C*56, beta L*76 C*42, gamma
                // L*69 C*67), against the old tuned L*97 C*56 h 103.
                // Calibration record: docs/chromophores/calibration-records.md#chrysoberyl-yellow
                // 2026-11-03 (owner: "match the pictures of actual gemstones"; this supersedes the
                // 2026-10-27 golden yellow): 0.5 x the former Greenish Yellow variant's tensor (GRR
                // 171 beta and gamma, estimated alpha) plus 0.15 x the Alexandrite built-in's Cr3+
                // (GRR 874; a greenish-yellow chrysoberyl carries traces of Cr and V), refitted to
                // eight energy Gaussians a ray (dE2000 below 0.1). Twenty-three dealer photos
                // (GemSelect) have a median hue of 109 (colored pixels) and L* 64; the golden 0.7 x
                // GRR 171 rendered h 76, the Greenish Yellow tensor alone h 93, a pale cream (a
                // lower strength only paled it), this mix h 111 at L*64 (Light Tent). More Cr
                // greens it further but greys it (0.6 x + 0.25 x: h 113 at L*57, a khaki): Cr3+
                // absorbs at 580 nm, the brightest part of the yellow. The gamma ray turns green
                // (swatch h 149), a trace of the alexandrite change. Swatch L*82 C*26 h 118. The
                // former golden default is the Golden variant.
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
                // F4: Alexandrite's n_alpha and n_gamma Walling 1980 curves (see that
                // entry), each with the same +0.011253 shift of n^2-1 that retargets
                // `dispersion` above (b[0] 0.78522 -> 0.796473). A constant added to n^2
                // scales every index difference by n_old/n_new (0.9997 here), so the
                // spreads are preserved to about 1e-5, not exactly: 0.007581
                // (Alexandrite) becomes 0.007567 (n_gamma - n_alpha at D). The three
                // curves never cross between 380 and 780 nm.
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
