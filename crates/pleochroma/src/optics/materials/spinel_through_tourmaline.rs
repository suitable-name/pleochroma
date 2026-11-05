//! Built-in material data: Spinel, Quartz, Tourmaline.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Third quarter of the built-in material table (Spinel through Tourmaline). See
    /// `built_in_materials_diamond_through_emerald` for why the table is split into
    /// several functions.
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
                // Models RED spinel: Cr3+ substituting for Al3+, the standard
                // gemological attribution (e.g. K. Nassau, "The Physics and Chemistry
                // of Color" (2nd ed., Wiley, 2001), Ch. 7), with the same Cr3+ d-d
                // family as Ruby/Alexandrite/Emerald. Two spin-allowed bands, the
                // usual ligand-field pair in MgAl2O4:
                //   4A2 -> 4T2 near 540 nm (green absorption), and
                //   4A2 -> 4T1(F) near 395 nm (violet absorption).
                // Measured on the Caltech Mineral Spectroscopy Server (G. R. Rossman)
                // natural red spinel JQ-13, Sri Lanka, 1.52 mm, baseline-corrected,
                // unpolarised (cubic); Vasiliev's synthetic flux-grown Cr spinel (10 mm
                // normalized, 25 K) gives the same peak ratio.
                //   540 nm band: measured centre 538 nm, FWHM about 77 nm (sigma 33
                //     nm). Centre cited; width (sigma 55 nm) and peak (2.4) stay TUNED
                //     -- the measured width is narrower, and narrowing the band changes
                //     the look, so it is a separate owner decision.
                //   395 nm band: fitted centre 398 nm (raw maximum 391 nm), uncertain
                //     by about +-10 nm because its UV side runs into the rising edge;
                //     FWHM 0.48 eV, about 60-65 nm, so sigma 27 nm (the constructor's
                //     width is a wavelength-domain standard deviation in nm). Peak =
                //     1.6 x the 540 band's 2.4 = 3.84: the raw peak-absorbance ratio is
                //     1.62 (natural) and 1.61 (synthetic); the baseline-subtracted
                //     ratio is 2.1 and the Gaussian-amplitude ratio 1.37, so the
                //     defensible range is 1.4-2.1.
                // The ultraviolet edge below 380 nm is not modelled. BLUE spinel (Co2+,
                // an optically distinct chromophore) is not modelled here -- a
                // candidate follow-up built-in.
                // Calibration record: docs/chromophores/calibration-records.md#spinel-red
                //
                // 2026-11-03 (owner: "match the pictures of actual gemstones"): REPLACED the two
                // tuned wavelength bands (the 540 nm one wider than measured) by the measured JQ-13
                // spectrum itself, the Pink variant's four energy Gaussians at 4 x that variant's
                // strength. Eleven dealer photos of red spinel (GemSelect) have a median hue of 27
                // (colored pixels, quartiles -4 / +11); the tuned bands rendered a magenta-pink
                // (Light Tent h 350), the measured spectrum renders h 16 at 3 x and h 20 at 5 x
                // (L* 37 and 29). Swatch (7 mm, D65) L*41 C*67 h 18; T(650) 0.72.
                // 2026-11-04 (owner: "The render seems to err toward saturated"): x 0.45 (1.8 x the
                // Pink variant). Colored-pixel saturation s_uv: the faceted dealer photos 0.83 (L*
                // 43, h 27, brownish pinks and reds), the x 1 render 2.13 at L* 32 (x 0.5: 1.50 at
                // L* 46, h 13; x 0.25: 0.88, h 10); the factor takes half the gap in log terms. A
                // weaker spectrum is pinker, and a 330 nm edge band moved it by 4 deg only: the
                // photos' brown is not in the Cr3+ spectrum. Swatch L*61 C*50 h 7.
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
                // (zero absorption at every wavelength). Rock crystal quartz
                // genuinely has no visible-range chromophore (its UV edge sits near
                // 150nm); its tinted varieties (Amethyst: a hole-color-centre defect;
                // Citrine: Fe3+) are their own separate built-ins (see
                // [`Self::built_in_materials_amethyst_through_citrine`]), sharing
                // this exact dispersion/birefringence data (same SiO2 host crystal)
                // but with their own cited absorption bands. Colorless (rock crystal): no
                // absorption in any direction, so there is no pleochroism to keep and the
                // empty isotropic tensor stays (lead decision 2026-10-27: the
                // "anisotropic host, per-axis tensor" rule applies to colored stones).
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
        // the same physically-derived B-G->F-C ratio as Emerald's comment above
        // (0.579): Delta n(F-C) = 0.0171*0.579 = 0.00990. Two data points exactly
        // determine a 2-parameter Cauchy fit (same method as Zircon's comment):
        // A=1.624481, B=0.005183, verified n_d=1.639406, Delta n(F-C)=0.009902,
        // Abbe V_d=64.58. LOWER CONFIDENCE than a primary-literature entry --
        // flagged for human cross-check.
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
            // PLEOCHROISM: genuine uniaxial `o_ray`/`e_ray` split, REFITTED
            // 2026-10-27 to a measured elbaite. The Fe2+ band near 720nm is NOT
            // strongly dichroic in elbaite: the Caltech spectra of the blue GRR 1935
            // (o 0.905, e 0.994 per mm), the pure-blue GRR 3770 (1.00 / 0.95) and
            // the green GRR 1935 (0.54 / 0.475) have the same Fe2+ intensity in both
            // rays (o:e about 1:1), so the old 3:1 ratio (and the "the dark ray"
            // wording) belonged to the Fe2+-Fe3+ paired bands of dravite and schorl
            // (Mattson & Rossman, Phys. Chem. Minerals 14, 163 (1987); Caltech GRR
            // 787), not to low-iron elbaite. What IS polarised in elbaite is the
            // ultraviolet / blue edge (Fe2+-Ti4+ and Mn2+-Ti4+ charge transfer, the
            // Mn2+-Ti4+ band lies at 325nm, Rossman & Mattson, Am. Mineral. 71, 599
            // (1986), with a tail into the visible): o about 2 x e. The old 430nm
            // "Fe2+-Ti4+ IVCT" band is not a measured feature of any elbaite
            // spectrum and is gone.
            //
            // Source: Caltech GRR 1935, the small green crystal, Usakos, Namibia
            // (3.645 mm; per-ray baselines 0.109 / 0.122 A/mm removed),
            // `data/reference_spectra/variant_tourmaline_verdelite_grr1935_green_
            // {eperpc,eparc}.csv`; four energy Gaussians per ray, non-linear least
            // squares over 380-1100nm, rms 0.014 / 0.009 per mm: a UV edge (centre on
            // its 300nm bound, FWHM 12 178 / 8 738 cm^-1: a stand-in whose visible
            // tail is the measurement), a weak broad Fe2+-Fe3+ / Fe2+-Ti4+ band near
            // 525-620nm, the Fe2+ band (711 / 709nm, FWHM 2186 / 2419 cm^-1) and its
            // near-infrared partner (1156 / 1180nm, outside the 380-780nm range but
            // its tail closes the red). Strength: every band, the UV edge included, is
            // 0.34 x the crystal's per-mm absorption, i.e. the measured spectrum at one
            // strength (owner 2026-10-27: measured values win; a first draft scaled the
            // edge a further 3 x to keep the old yellow-green h 121), stored as per mm x
            // 9.905 mm, the face-up path of a 7 mm stone.
            // Swatch (7 mm, D65, (2o + e)/3): L*91 C*20 h 148, a light bluish green,
            // against the old L*93 C*49 h 121; o-ray L*90 C*27 h 142 (the deeper ray),
            // e-ray L*94 C*10 h 175 (paler, bluer). Edge o : e at 400 nm 2.75, Fe2+ o : e
            // at 711 nm 1.16.
            // Calibration record: docs/chromophores/calibration-records.md#tourmaline
            // 2026-11-03 (owner: "match the pictures of actual gemstones"; this supersedes the
            // 2026-10-27 rule that the measured strength wins): 3.5 x the measured elbaite plus
            // 5.5 x the Yellow variant (the measured GRR 1935 yellow, one band a ray). Fourteen
            // dealer photos of green tourmaline (GemSelect) have a median hue of 121 (colored
            // pixels), an olive to yellowish green, and are dark (L* 52); the 0.34 x crystal
            // alone rendered h 146 L*78. A first step (2 x + 1 x) gave h 127 L*71; a darker
            // stone turns bluer unless the yellow part grows faster, so 3.5 x + 5.5 x: h 125
            // L*62 (Light Tent; the hue does not go below about 125 with these two spectra).
            // That stone was still a vivid lime: C* 66 against the photos' 28.5, 2.3 x where
            // the matched materials render about 1.4 x their photos' chroma. So (same day,
            // owner: "Fix the ... tourmaline gaps") the Pink variant's Mn3+ at 0.7 x greys the
            // green window to olive, and the elbaite's Fe2+ grows to 1.45 x (the yellow part
            // to 0.9 x) to hold the hue against the Pink's pull to yellow: Light Tent h 122
            // L*41 C*50 (1.75 x the photos' chroma). The three parts refitted to eight energy
            // Gaussians a ray (dE2000 0.01; the near-infrared partner of the Fe2+ band moves to
            // 1066 nm, its tail into the red unchanged). Swatch L*52 C*46 h 124 (o-ray L*44
            // C*68 h 114, e-ray L*66 C*38 h 131); edge o : e at 400 nm 3.3, Fe2+ o : e at
            // 711 nm 1.15.
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
            // c_axis set INTO the table plane (Vec3::X) rather than the Vec3::Y
            // every other built-in defaults to -- a deliberate override, not an
            // oversight. c_axis lies IN the table plane (`Vec3::X`), i.e. the table
            // is PARALLEL to c, the way tourmaline is cut. Looking ALONG c every
            // polarisation has E perpendicular to c and sees the o-ray absorption,
            // which in this (Fe-bearing) preset is the dark ray; looking ACROSS c
            // one mode has E parallel to c (the lighter e-ray). So a table normal
            // along c would be the dark view; `Vec3::X` is correct and must not be
            // changed to match older wording. Three things are kept apart here: the
            // orientation of the table plane (c in it), the propagation direction
            // (the face-up view axis, perpendicular to c), and the polarisation (E
            // perpendicular to or parallel to c, per eigenmode). See
            // `raytracer_tests::tourmaline_face_up_is_brighter_with_c_axis_in_table_plane`.
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
