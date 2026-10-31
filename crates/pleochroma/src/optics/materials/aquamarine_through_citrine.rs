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
    /// # Deliberately left out of this list: Sphene
    ///
    /// Sphene (titanite) needs `birefringence_delta` up to ~0.135, well beyond every
    /// existing Delta-n-range assumption this file's biaxial/uniaxial machinery has
    /// been verified against (the largest here, Zircon's +0.059, is under half that),
    /// so adding it without dedicated verification at that magnitude would ship an
    /// unverified extrapolation, not a measurement. Left out.
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
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        // o-ray (E perp c): near-colorless; UV/Fe3+ edge, Fe3+ 429 nm,
                        // weak IVCT leak, Fe2+ ~830 nm tail
                        AbsorptionBand::energy(363.0, 5_531.0, 1.19),
                        AbsorptionBand::energy(429.0, 1_500.0, 0.214),
                        AbsorptionBand::energy(645.0, 3_657.0, 0.198),
                        AbsorptionBand::energy(845.0, 2_797.0, 6.54),
                    ],
                    vec![
                        // e-ray (E par c): the blue ray; Fe2+-Fe3+ IVCT ~600-680 nm +
                        // Fe2+ ~830 nm
                        AbsorptionBand::energy(378.0, 3_102.0, 0.892),
                        AbsorptionBand::energy(428.0, 565.0, 0.317),
                        AbsorptionBand::energy(600.0, 4_405.0, 3.26),
                        AbsorptionBand::energy(810.0, 5_471.0, 8.82),
                    ],
                ),
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
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        AbsorptionBand::energy(312.7, 8_241.9, 2.34),
                        AbsorptionBand::energy(440.0, 1_832.0, 0.102),
                        AbsorptionBand::energy(495.9, 3_197.8, 0.661),
                        AbsorptionBand::energy(554.6, 1_954.5, 0.593),
                    ],
                    vec![
                        AbsorptionBand::energy(319.9, 8_241.9, 2.39),
                        AbsorptionBand::energy(440.0, 3_532.2, 0.331),
                        AbsorptionBand::energy(543.6, 3_487.5, 1.03),
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
                absorption: AbsorptionTensor::biaxial(
                    vec![
                        // alpha (E par c): estimated mean of beta and gamma
                        AbsorptionBand::energy(342.5, 2_976.0, 5.5),
                        AbsorptionBand::energy(373.5, 1_194.0, 3.81),
                        AbsorptionBand::energy(378.0, 3_600.0, 4.09),
                        AbsorptionBand::energy(440.1, 1_530.0, 4.36),
                        AbsorptionBand::energy(490.0, 7_400.0, 1.19),
                    ],
                    vec![
                        // beta (E par a): measured, GRR 171
                        AbsorptionBand::energy(337.0, 1_816.0, 4.0),
                        AbsorptionBand::energy(369.3, 3_197.0, 3.86),
                        AbsorptionBand::energy(377.6, 484.0, 2.81),
                        AbsorptionBand::energy(436.0, 2_071.0, 1.97),
                        AbsorptionBand::energy(490.0, 9_296.0, 0.865),
                    ],
                    vec![
                        // gamma (E par b): measured, GRR 171
                        AbsorptionBand::energy(344.7, 2_813.0, 7.1),
                        AbsorptionBand::energy(372.2, 1_190.0, 5.63),
                        AbsorptionBand::energy(378.0, 3_600.0, 5.3),
                        AbsorptionBand::energy(440.6, 1_455.0, 6.85),
                        AbsorptionBand::energy(490.0, 6_304.0, 1.53),
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
