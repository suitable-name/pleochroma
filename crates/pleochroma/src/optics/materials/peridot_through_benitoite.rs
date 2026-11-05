//! Built-in material data: Peridot, YAG, GGG, Benitoite.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

/// Peridot's trichroic absorption (alpha/beta/gamma -> `o_ray`/`beta_ray`/`e_ray`): the
/// energy-Gaussian fits to Caltech GRR 418 described at the entry below. A separate function so
/// the entry stays within clippy's function-length limit.
fn peridot_absorption() -> AbsorptionTensor {
    AbsorptionTensor::biaxial(
        // alpha (o_ray)
        vec![
            AbsorptionBand::energy(345.0, 10_096.0, 5.38),
            AbsorptionBand::energy(455.0, 1_410.0, 0.53),
            AbsorptionBand::energy(490.0, 2_160.0, 0.44),
            AbsorptionBand::energy(637.0, 1_724.0, 0.80),
            AbsorptionBand::energy(840.0, 3_276.0, 4.76),
            AbsorptionBand::energy(1000.0, 2_534.0, 35.2),
        ],
        // beta (beta_ray)
        vec![
            AbsorptionBand::energy(341.0, 12_000.0, 6.78),
            AbsorptionBand::energy(465.0, 2_526.0, 1.07),
            AbsorptionBand::energy(500.0, 3_000.0, 0.47),
            AbsorptionBand::energy(635.0, 2_500.0, 1.33),
            AbsorptionBand::energy(840.0, 4_000.0, 4.25),
            AbsorptionBand::energy(1000.0, 2_401.0, 35.2),
        ],
        // gamma (e_ray)
        vec![
            AbsorptionBand::energy(376.0, 3_317.0, 4.77),
            AbsorptionBand::energy(440.0, 2_933.0, 2.24),
            AbsorptionBand::energy(500.0, 3_000.0, 0.79),
            AbsorptionBand::energy(627.0, 2_500.0, 0.77),
            AbsorptionBand::energy(840.0, 4_000.0, 2.34),
            AbsorptionBand::energy(1000.0, 3_663.0, 12.7),
        ],
    )
}

impl GemMaterial {
    /// Third quarter: Peridot (the one biaxial addition in this quarter), YAG, GGG
    /// and Benitoite.
    #[expect(clippy::too_many_lines, reason = "a data table of built-in materials")]
    pub(super) fn built_in_materials_peridot_through_benitoite() -> Vec<Self> {
        vec![
            // Peridot (Forsterite-rich olivine, (Mg,Fe)2SiO4): n_beta(D) = 1.669 (the
            // base curve IS n_beta by the biaxial convention; alpha 1.654 and gamma
            // 1.690 follow from the spreads below); B-G 0.020 -> Delta n(F-C) =
            // 0.01158, B = 0.006062, A = 1.669 - B/0.5893^2 = 1.651549.
            // Abbe V_d~56.49. No primary Sellmeier fit available; LOWER CONFIDENCE
            // Cauchy fallback, same convention as the garnets above.
            Self {
                name: "Peridot".to_string(),
                crystal_system: CrystalSystem::Orthorhombic,
                optical_character: OpticalCharacter::BiaxialPositive,
                dispersion: DispersionModel::Cauchy {
                    a: 1.651_549,
                    b: 0.006_062,
                    c: 0.0,
                },
                birefringence_delta: 0.036,
                // Chromophore: Fe2+ in olivine. Biaxial (trichroic) energy-Gaussian
                // sets, one per principal ray (alpha/beta/gamma -> o_ray/beta_ray/
                // e_ray, as Tanzanite). Each set is a bounded least-squares fit
                // (log residual, 380-780 nm) to the Caltech Mineral Spectroscopy
                // Server San Carlos olivine spectra GRR 418 alpha/beta/gamma (G. R.
                // Rossman; CSVs `crates/pleochroma/data/reference_spectra/
                // caltech_olivine_grr418_{alpha,beta,gamma}.csv`, URLs in their
                // headers), scaled to 0.88 cm = the 8.8 mm face-up path of a 7 mm
                // stone (fitted 2026-10-19; GIA Sardinia peridot
                // characterisation [S4]: 473/490 nm Fe2+ bands, total absorption
                // below ~440 nm, broad ~1050 nm band with an 830 nm shoulder). The
                // 345/341/376 nm bands stand in for the UV edge; the 840/1000 nm
                // bands for the Fe2+ NIR pair (fitted amplitudes sat on the fit
                // bound, only their 650-780 nm tails matter inside the 380-780 nm
                // sampling). Swatch (7 mm, D65): L*76 C*63 h=111 yellow-green; the
                // previous blue-only 450/475/495 nm triplet let red pass and
                // drifted to amber (h=81). Fitted, not hand-tuned.
                // Calibration record: docs/chromophores/calibration-records.md#peridot
                // 2026-11-04 (owner: "The render seems to err toward saturated"): x 0.6 of the San
                // Carlos crystal. Colored-pixel saturation s_uv: the faceted dealer photos 0.79, the
                // Commons photos 0.54-0.94, the x 1 render 0.96 (x 0.5: 0.72 at h 114, L* 71 against
                // the photos' 71). Swatch L*84 C*48 h 113 (at two units L*72 C*68 h 111).
                // Calibration record: docs/chromophores/calibration-records.md#saturation-round-2026-11-04
                absorption: peridot_absorption().scaled(0.6),
                c_axis: Vec3::Y,
                // Same fractional-position method as Alexandrite/Topaz/Tanzanite/
                // Chrysoberyl above: real olivine/peridot principal indices are
                // commonly tabulated (e.g. Deer, Howie & Zussman, *An Introduction to
                // the Rock-Forming Minerals*) around n_alpha~1.654, n_beta~1.669,
                // n_gamma~1.690 for gem-quality (Fo~90) peridot -- fraction
                // (1.669-1.654)/(1.690-1.654) = 0.417 -- applied to this entry's own
                // birefringence_delta: 0.417 * 0.036 = 0.01501, rounded to 0.0150.
                biaxial_delta_beta_alpha: Some(0.0150),
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
            // YAG (Yttrium Aluminium Garnet, Y3Al5O12, undoped/colorless laser host)
            //
            // Source: D.E. Zelmon, D.L. Small & R. Page, "Refractive-index
            // measurements of undoped yttrium aluminum garnet from 0.4 to 5.0 um,"
            // Appl. Opt. 37, 4933-4935 (1998), as tabulated by refractiveindex.info
            // ("Y3Al5O12: Zelmon"):
            //   n^2-1 = 2.28200*l^2/(l^2-0.01185) + 3.27644*l^2/(l^2-282.734)
            // Encoded via Sellmeier3 with the unused 3rd pole zeroed (b=0, c=1.0 um^2,
            // same technique as Diamond/Spinel above). Verified: n_d = 1.83263, Delta
            // n(F-C) = 0.01600, Abbe V_d = 52.04 -- matching YAG's widely-cited real
            // Abbe number (~52); a commonly quoted "0.028" dispersion figure for this
            // row is not reproduced by this primary Sellmeier fit and is not used.
            Self {
                name: "YAG".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [2.282_00, 3.276_44, 0.0],
                    c: [0.011_85, 282.734, 1.0],
                },
                birefringence_delta: 0.0,
                // colorless (undoped YAG, the laser-host reference composition):
                // empty band set, zero absorption at every wavelength.
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
            // GGG (Gadolinium Gallium Garnet, Gd3Ga5O12, colorless synthetic --
            // historically used as a diamond simulant before Cubic Zirconia).
            //
            // Source (primary): D.L. Wood & K. Nassau, "Optical properties of
            // gadolinium gallium garnet," Appl. Opt. 29, 3704-3707 (1990), Table I,
            // the average of four crystals; the journal's abstract page was read,
            // the full text was not (transparent 0.36-6.0 um, three-term Sellmeier
            // agreeing with experiment to better than 2 parts in the 4th decimal):
            //   n^2 = 1 + sum_i A_i*l^2/(l^2 - L_i^2),  l and L_i in um,
            //   A = (1.7727, 0.9767, 4.9668), L = (0.1567, 0.01375, 22.715).
            // The form is not printed on that page; it was verified numerically (the
            // L_i read as resonance wavelengths reproduce the paper's Table II He-Ne
            // value 1.96515 exactly; reading them as squared wavelengths gives
            // 2.199 and is wrong). Encoded via Sellmeier3 with c_i = L_i^2 in um^2,
            // the convention of the Spinel entry: 0.1567^2 = 0.024555, 0.01375^2 =
            // 0.000189, 22.715^2 = 515.971.
            // Verified: n_F(486.13) = 1.98833, n_d(587.56) = 1.97036, n_D(589.3) =
            // 1.97014, n(632.8) = 1.96515, n_C(656.27) = 1.96287, Delta n(F-C) =
            // 0.02546, Abbe V_d = 38.11.
            // The coefficients are primary: the paper's printed Table II n_F =
            // 1.98337 (and its n_F - n_C = 0.02046) disagree with the coefficients
            // and with the printed n_D, n_C and He-Ne values by exactly 0.005, a digit
            // slip in the table; the F-line value used here is the formula's.
            // Validity: the 156.7 nm and 13.75 nm poles sit below the sampled band
            // and the 22.7 um pole far above it, so `validate` finds no resonance in
            // 300-800 nm and n > 1 across 380-780 nm; `min_valid_nm` is the 300 nm
            // numerical floor (both UV poles are below it, 5 % above them is under
            // 300 nm). That floor is NOT the measured range: the data cover 0.36-6.0
            // um, so 300-360 nm is extrapolation.
            // History: the entry was a 2-parameter Cauchy (n_d 1.970, Delta n(F-C)
            // 0.026055 = the gemological B-G 0.045 x the file's 0.579 ratio, A =
            // 1.930726, B = 0.013639). That conversion held up: 0.026055 is 2.3 %
            // above the measured 0.02546.
            // Calibration record: docs/chromophores/calibration-records.md#ggg-dispersion
            Self {
                name: "GGG".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [1.7727, 0.9767, 4.9668],
                    c: [0.024_555, 0.000_189, 515.971],
                },
                birefringence_delta: 0.0,
                // colorless (undoped GGG): empty band set, zero absorption at
                // every wavelength.
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
            // Benitoite (BaTiSi3O9): per-ray two-term Cauchy fits, n = A + B/l^2 (l in
            // um), from the gemological table of Laurs, Rohtert & Gray, Gems &
            // Gemology 33(3), 166-187 (1997), Table 1: n_omega 1.757, n_epsilon 1.804,
            // Fraunhofer B-G dispersion 0.039 (o-ray) and 0.046 (e-ray) (the latter two
            // are Payne's 1939 figures). With the B-G factor 3.268 and the F-C factor
            // 1.9097 (both computed for this wavelength pair):
            //   o-ray: B = 0.039/3.268 = 0.011934, A = 1.757 - B/0.5893^2 = 1.722634
            //          (Delta n(F-C) = 0.02279, Abbe V_d = 33.2)
            //   e-ray: B = 0.046/3.268 = 0.014076, A = 1.804 - B/0.5893^2 = 1.763466
            //          (Delta n(F-C) = 0.02688, Abbe V_d = 29.9)
            // so n_e - n_o at D is 0.047, the stored birefringence_delta. SECONDARY
            // confidence (a gemological table, no primary Sellmeier fit). The earlier
            // single curve (A 1.717734, B 0.013639, B-G 0.045) served both rays.
            Self {
                name: "Benitoite".to_string(),
                crystal_system: CrystalSystem::Hexagonal,
                optical_character: OpticalCharacter::UniaxialPositive,
                dispersion: DispersionModel::Cauchy {
                    a: 1.722_634,
                    b: 0.011_934,
                    c: 0.0,
                },
                birefringence_delta: 0.047,
                // PLEOCHROISM: benitoite is famously STRONGLY dichroic -- an intensely
                // blue e-ray (E-parallel-c) against a near-colourless o-ray (E-perp-c).
                // SHAPE (fitted): Caltech Mineral Spectroscopy Server (G. R. Rossman),
                // sample GRR 561-D4, dark-blue zone, The Gem Mine, San Benito County,
                // 0.997 mm, polarised, room temperature not stated, decadic absorbance.
                // Linear baseline + one Gaussian in energy fitted over 420-1600 nm,
                // net amplitudes:
                //   e-ray (E parallel c): centre 698 nm (1.776 eV), FWHM 0.88 eV (7100
                //     cm^-1; `AbsorptionBand::energy` takes the FWHM in cm^-1:
                //     0.88 eV * 8065.5 cm^-1/eV = 7098, sigma 3015 cm^-1), net
                //     amplitude 2.73 absorbance;
                //   o-ray (E perp c): centre 603 nm (2.057 eV), FWHM 1.07 eV (8630
                //     cm^-1), net amplitude 0.146 -- poorly defined (comparable to the
                //     baseline drift), "about 5 %" of the e-ray's (0.146 / 2.73 =
                //     0.053), at most about 5 %.
                // The measured e-ray band is asymmetric (a tail into the near infrared:
                // half-maximum at 559 and 929 nm, 1.47 absorbance still at 1000 nm);
                // one Gaussian in energy is this model's simplification and reproduces
                // the half-maximum points only approximately. The rising ultraviolet
                // edge (0.62 absorbance at 350 nm, both rays) is not modelled.
                // CAUSE of color: Laurs, Rohtert & Gray (G&G 33(3), 1997, Fig. 28 shows
                // the same E-parallel-c band near 700 nm running into the near
                // infrared, and a weak E-perp-c curve) state that the cause is not
                // established; Fe2+-Ti4+ or Fe2+-Fe3+ charge transfer and Zr in the Ti
                // site have been proposed, none confirmed. No chromophore is asserted.
                // STRENGTH (tuned): the measured 2.73 net absorbance over 0.997 mm is
                // 6.3 /mm natural; over 7 mm that is A = 2.73 x 7 / 0.997 = 19
                // (decadic) at 700 nm along c -- opaque. GRR 561-D4 is a dark-blue zone of a crystal, gem benitoite is
                // far lighter, and no concentration is published. The e-ray peak was
                // the earlier 2.0 (model units) and the o-ray peak followed the measured
                // ratio, 2.0 x 0.053 = 0.106, written 0.11. 2026-11-03 (owner: "make the
                // benitoite a bit more saturated") both peaks x 2, 4.0 and 0.22 (the ratio
                // kept), chosen from renders of the Pentalume Sapphire design: Light Tent
                // L*69 C*17 h 264 at x 1, L*65 C*22 at x 1.5, L*61 C*27 h 274 at x 2, L*55
                // C*33 at x 3 (Studio Softbox C*13 -> C*22 at x 2). Swatch (7 mm, D65):
                // e-ray L*53 C*70 h 278, o-ray L*93 C*8, unpolarised L*83 C*22 h 277.
                // Calibration record: docs/chromophores/calibration-records.md#benitoite
                absorption: AbsorptionTensor::uniaxial(
                    vec![AbsorptionBand::energy(603.0, 8_630.0, 0.22)], // o-ray (E-perp-c)
                    vec![AbsorptionBand::energy(698.0, 7_100.0, 4.0)],  // e-ray (E-parallel-c)
                ),
                // The c axis lies in the table (2026-11-03), the way benitoite is cut: the blue
                // is the e-ray's (E parallel to c) and the o-ray is nearly colorless, so a stone
                // with c along the table normal shows face-up only the o-ray and looks almost
                // clear (Pentalume Sapphire, Light Tent: L* 69 C* 15). With c in the table,
                // face-up light vibrates both along and across c and the stone shows its blue.
                // The Crystal Axis of a design still turns it anywhere.
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
                // e-ray (n_epsilon): the Cauchy fit described in the comment above.
                uniaxial_extraordinary_dispersion: Some(DispersionModel::Cauchy {
                    a: 1.763_466,
                    b: 0.014_076,
                    c: 0.0,
                }),
                biaxial_principal_dispersion: None,
            },
        ]
    }
}
