//! Built-in material data: Zircon, Alexandrite, Topaz.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Second quarter of the built-in material table (Zircon through Topaz). Split out
    /// of `built_in_materials_diamond_through_emerald` purely to keep each part under
    /// clippy's function-length lint -- this is plain data, not logic, so the split
    /// point carries no significance.
    #[expect(clippy::too_many_lines, reason = "a data table of built-in materials")]
    pub(super) fn built_in_materials_zircon_through_topaz() -> Vec<Self> {
        vec![
            // Zircon (High Zircon, ZrSiO4)
            //
            // n_o = 1.925, corroborated by three independent sources: (1) Mineral Data
            // Publishing's "Handbook of Mineralogy" (2001), zircon entry: omega =
            // 1.925-1.961; (2) International Gem Society: "High zircon: n_o =
            // 1.920-1.940 (often 1.925); n_e = 1.970-2.010 (often 1.984)"; (3) the
            // widely-reproduced gem-trade n_o=1.9250/n_e=1.9840 pairing, also the
            // standard GIA reference point for gem-quality (heat-treated) colorless
            // high zircon. birefringence_delta = n_e - n_o = 1.984 - 1.925 = +0.0590.
            //
            // No primary Sellmeier/Cauchy fit for zircon exists in the optics
            // literature, so this is a LOWER-CONFIDENCE 2-parameter Cauchy fallback.
            // Dispersion shape: zircon's "0.039" figure is near-universal across
            // gemological references (GIA, IGS) and is the Fraunhofer B-G interval,
            // not F-C. Converted via the same physically-derived B-G->F-C ratio as the
            // Emerald entry's comment (0.579, range 0.569-0.587): Delta n(F-C) =
            // 0.039*0.579 = 0.02258. Two data points (n_o=1.925, converted Delta n)
            // exactly determine a 2-parameter Cauchy fit:
            //   A = n_d - B/lambda_d^2,  B = (n_d-1)/(V_d * (1/lambda_F^2 - 1/lambda_C^2))
            // giving A=1.890963, B=0.011820, verified n_d=1.925, Delta n(F-C)=0.02258,
            // Abbe V_d=40.96. LOWER CONFIDENCE than a primary-literature entry --
            // flagged for human cross-check. n_d drives the critical angle
            // (brilliance/windowing/extinction), so a >0.002 change here is
            // significant: this value differs from an earlier, unsourced 1.956878 by
            // -0.0319, well above that threshold.
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
                // Chromophore: high (gem-quality) zircon's color and characteristic
                // sharp lines come from trace U4+ substituting for Zr4+, producing
                // narrow lines rather than a broad transition-metal band. Source: the
                // U4+ absorption spectrum of zircon, e.g. P.E. Fielding (1970) and
                // Nasdala et al., "Spectroscopic methods applied to zircon," Reviews
                // in Mineralogy & Geochemistry 53 (2003), reporting a dominant line
                // near 653nm plus a weaker set at 691/589/562/537nm.
                //
                // PER-AXIS since 2026-10-27 (owner decision: a built-in on an anisotropic
                // host never carries an isotropic tensor; the old five lines, sigma 6-8nm,
                // peaks 2.4 / 0.5, were a tuned, unpolarised stand-in). The lines are
                // measured, and they ARE polarised: the Caltech GRR 3106 spectra of a
                // Mogok zircon (green region, 2.592 mm, E perp c / E par c) show
                // different line sets and strengths in the two rays (653.4nm o 1.16 per
                // mm net against 652.8nm e 0.55; 690.9nm o 0.68 against none in e;
                // 589nm, 616nm and 511nm lines stronger or only in e), so the tensor
                // carries the measured split. Each line is an energy Gaussian at the
                // measured peak with the measured FWHM (65-170 cm^-1, i.e. 3-7nm), its
                // amplitude the peak above the LOCAL background (the broad radiation
                // damage continuum under the lines is NOT part of this default: it is
                // what makes zircon brown, red-brown or green and belongs to the colour
                // variants): o-ray 483.4 / 515.7 / 537.2 / 562.3 / 589.7 / 653.4 /
                // 659.5 / 690.9nm (the 8 bands the GPU holds), e-ray 511.5 / 542.0 /
                // 589.1 / 616.0 / 652.8 / 660.5nm; the weak 465, 484 and 538nm e-ray
                // features (below 0.08 per mm) are left out. Strength 1.0 x the
                // crystal's own per-mm absorption (per mm x 9.905 mm, the face-up path
                // of a 7 mm stone, no other scaling). Swatch (7 mm, D65, (2o + e)/3):
                // L*93 C*12 h 303 against the old L*93 C*13 h 279 (dE2000 6.3); o-ray
                // L*93 C*14 h 318 (pink-lilac), e-ray L*94 C*10 h 256 (blue-lilac); the
                // measured line set puts the unpolarised hue at lilac (303) where the
                // old red-line-dominated set gave blue-lilac (279).
                // CONSTRUCTED, NOT MEASURED AS A STONE (review F-18): the measured lines
                // sit on the measured radiation-damage continuum, and the continuum was
                // removed line by line, so this is a colorless zircon with the real
                // U4+ lines, a stone no sample of the server is. The measured continuum
                // lives in the `zircon.*` variants (brown, green, blue).
                // Calibration record: docs/chromophores/calibration-records.md#zircon
                // 2026-11-03 (owner: "match the pictures of actual gemstones"): every line x 0.05.
                // Eighteen dealer photos of white zircon (GemSelect, measured in the lead's private
                // comparison) are colorless (C* 0.7); the lines at the Mogok crystal's full
                // strength rendered a lilac stone (Light Tent C* 11 h 307). A colorless gem zircon
                // holds little uranium (Ratanakiri about 120 ppm, Zeug et al. 2018), far less than
                // the green, radiation-damaged Mogok zircon the lines were measured on, so the
                // lines stay, about a twentieth as strong: swatch L*99 C*1 (it was L*93 C*12).
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
            // Source: J. Walling et al., "Tunable alexandrite lasers," IEEE J. Quantum
            // Electron. 16, 1302 (1980), n(alpha)-direction Sellmeier
            // (refractiveindex.info "BeAl2O4: Walling-alpha"), valid 0.25-2.6 um:
            //   n^2 = 1.78522 + 1.21202*l^2/(l^2-0.01262) - 0.01681*l^2
            // Represented as Sellmeier3 with the "-0.01681*l^2" linear term encoded as
            // a distant pole (b/c = 0.01681, c=1000 um^2, outside the visible domain)
            // and the leading constant folded into a c=0 pole:
            //   pole0 (constant): b=0.78522, c=0
            //   pole1 (real UV pole, exact): b=1.21202, c=0.01262
            //   pole2 (linear-term approximation): b=16.81, c=1000
            // Verified: n_d = 1.742730, Delta n(F-C) = 0.010051, Abbe V_d = 73.90.
            //
            // birefringence_delta: Walling 1980 also tabulates beta and gamma
            // Sellmeier fits (Walling-beta.yml, Walling-gamma.yml, same repo). Note:
            // Walling's alpha/beta/gamma file labels are the paper's crystallographic
            // axis labels and do NOT sort by index magnitude the way the mineralogical
            // optical-indicatrix convention (n_alpha <= n_beta <= n_gamma) requires --
            // at the D line the three curves evaluate to alpha=1.742730,
            // beta=1.748360, gamma=1.740779, i.e. numerically gamma < alpha < beta.
            // Sorting by magnitude gives the true optical indicatrix: n_alpha(opt) =
            // 1.740779 (Walling's "gamma" file), n_beta(opt) = 1.742730 (Walling's
            // "alpha" file, used as n_d above), n_gamma(opt) = 1.748360 (Walling's
            // "beta" file). True total birefringence = n_gamma(opt) - n_alpha(opt) =
            // 0.007581.
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
                // TRICHROISM: the axes and colors after Farrell & Newnham (the band data
                // itself is measured, see 2026-11-03 below).
                //
                // Axes and colors: E.F. Farrell & R.E. Newnham, "Crystal-field spectra of
                // chrysoberyl, alexandrite, peridot, and sinhalite," American
                // Mineralogist 50, 1972 (1965). Polarized absorption at 77K, space
                // group Pnma (a=9.40, b=5.48, c=4.43 A -- same axis convention as this
                // entry's dispersion source, Walling 1980), pleochroic colors
                // yellow||a, green||b, red||c. Two Cr3+ band systems: 4A2->4T2 at
                // 560-595nm and 4A2->4T1 at 410-440nm, straddling Neuhaus's 580/415nm
                // red-green critical values -- that straddle is the illuminant-
                // dependent daylight-green/incandescent-red color change (pinned by
                // `raytracer_tests::alexandrite_shifts_redder_under_incandescent_than_d65`);
                // the per-axis split below adds direction-dependence on top.
                //
                // AXIS MAPPING: `AbsorptionTensor::biaxial(alpha, beta, gamma)` slots
                // are the n_alpha/n_beta/n_gamma principal directions. Per this
                // entry's Walling-derived indicatrix sort above (n_alpha(opt) is
                // Walling's E||c curve, n_beta(opt) is E||a, n_gamma(opt) is E||b),
                // the crystallographic axes land as: alpha = crystal c (RED), beta =
                // crystal a (YELLOW), gamma = crystal b (GREEN, along `c_axis` below).
                //
                // 2026-11-03 (photorealism round): REPLACED the tuned two-band sets
                // (F&N band positions, 40/32 nm widths, figure-read amplitudes x0.1/x0.14)
                // by the former `alexandrite.strong_change` color variant, which is now
                // this Material Default (the variant was removed). The tuned sets turned
                // ORANGE under a warm lamp (Pentalume Sapphire, Light Tent h 129 C* 10;
                // Incandescent h 45 C* 7): no ray changed by more than 20 deg, so the stone
                // went from a yellowish green to an orange, never to red. These sets turn
                // the stone from a bluish green to a purplish red (Light Tent h 164;
                // Incandescent h 333 C* 15), the classic alexandrite change.
                //
                // Source: beta (E||a) and gamma (E||b) are the measured rays of Caltech
                // GRR 874 (Lake Manyara, 0.717 mm; G. R. Rossman, CC BY-NC 4.0;
                // `data/reference_spectra/caltech_chrysoberyl_grr874_beta_ch874a_0p717mm.csv`
                // and `variant_alexandrite_grr874_gamma_0p717mm.csv`), constants removed,
                // the Cr3+ R line interpolated, at one Cr factor 0.15, plus the Fe3+ bands
                // of GRR 171 (the Chrysoberyl (Yellow) spectra) at 0.10. ESTIMATED: the
                // alpha (E||c) ray, which no spectrum measures: the gamma ray split at
                // 490 nm, its 4T2 half moved from 597 to 568 nm at 0.6 x and its 4T1 half
                // from 415 to 418 nm at 1.0 x (F&N Fig. 4 ratios, figure-read); and the two
                // factors, chosen for a green by daylight and a red to purplish red under
                // 3200 K. Six energy Gaussians per ray fitted to those spectra, per mm,
                // here x 9.905 mm (the face-up path of a 7 mm stone) in model units; the
                // widths are FWHM (`AbsorptionBand::energy` takes the FWHM).
                // Swatch (7 mm): alpha L*42 C*20 h 339 (D65) / C*41 h 344 (3200 K), beta
                // L*68 C*44 h 93, gamma L*46 C*75 h 187 / h 206; the three-ray mean
                // L*54 C*17 h 132 by daylight and C*12 h 20 under 3200 K. The 4T2 order
                // at 595 nm (gamma > alpha > beta) and the 4T1 maximum on beta at 422 nm
                // are F&N's.
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
                // n_beta - n_alpha at the D line, TIGHT confidence -- from the same
                // Walling 1980 alpha/beta/gamma Sellmeier trio used for
                // birefringence_delta above, sorted into the true optical indicatrix:
                // n_alpha(opt) = 1.740779, n_beta(opt) = 1.742730 (== n_d above),
                // n_gamma(opt) = 1.748360. delta_beta_alpha = 1.742730 - 1.740779 =
                // 0.001951 (cross-checks: n_gamma - n_alpha = 0.007581 is
                // birefringence_delta; both are the D-line values of the curves in
                // `biaxial_principal_dispersion`, which make the spreads vary with color).
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
                // F4: the other two principal indices, as their own Walling 1980 fits
                // (the distant-pole forms of the three files, re-evaluated and checked
                // against the files' values at 589.3 nm). `dispersion` above stays n_beta.
                //   n_alpha(opt) = Walling-gamma file: n^2 = 1.70690
                //     + 1.28477 l^2/(l^2-0.01169) - 0.01760 l^2
                //   n_gamma(opt) = Walling-beta file: n^2 = 1.58829
                //     + 1.42711 l^2/(l^2-0.01113) - 0.01696 l^2
                // The three curves never cross between 380 and 780 nm (a test pins it);
                // n_gamma - n_alpha runs from 0.00845 at 380 nm to 0.00735 at 780 nm.
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
            // No primary Sellmeier/Cauchy fit for topaz exists (absent from the
            // refractiveindex.info database, and no dedicated visible-range topaz
            // dispersion paper found). LOWER-CONFIDENCE 2-parameter Cauchy fallback.
            //
            // n_d = 1.627178, within International Gem Society's cited topaz range
            // "1.61-1.638". Dispersion shape: IGS's cited "dispersion .014" is the
            // Fraunhofer B-G interval, not F-C. Converted via the same
            // physically-derived B-G->F-C ratio as Emerald's comment above (0.579):
            // Delta n(F-C) = 0.0141*0.579 = 0.00816. Two data points (n_d, converted
            // Delta n) exactly determine a 2-parameter Cauchy fit (same method as
            // Zircon's comment): A=1.614872, B=0.004273, verified n_d=1.627176, Delta
            // n(F-C)=0.008163, Abbe V_d=76.83. LOWER CONFIDENCE than a
            // primary-literature entry -- flagged for human cross-check.
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
                // This entry models "London/Sky Blue" TOPAZ specifically
                // (the commercially dominant form: near-colorless natural topaz,
                // irradiated then heat-treated) -- its color comes from an
                // irradiation-induced color centre (a trapped-electron/hole defect),
                // not a transition-metal d-d transition, a broad band centred roughly
                // 610-620nm (red-orange), leaving blue transmitted. Source: K. Nassau,
                // "Gemstone Enhancement" (2nd ed., Butterworth-Heinemann, 1994), the
                // standard gemological reference for this mechanism. Width (70nm,
                // broader than the sharp transition-metal bands elsewhere in this
                // file -- color centres are typically broad) and peak (1.6) tuned;
                // band centre (620nm) cited. colorless natural (pre-treatment) topaz
                // is reachable with an empty band set; imperial topaz (orange-pink,
                // Cr3+ ~540nm) is a candidate for a follow-up built-in.
                //
                // PER-AXIS since 2026-10-27 (owner decision: a built-in on an anisotropic
                // host never carries an isotropic tensor; the old one-band 620nm
                // wavelength Gaussian, sigma 70nm, peak 1.6, was a tuned stand-in). The
                // measured O- hole-centre band is strongly polarised, and NOT along
                // beta: the Caltech spectra of the neutron-irradiated blue topaz GRR
                // 1632 (Nigeria, 3.462 mm, three rays, baselines A 0.007 / 0.004 /
                // 0.010 per mm removed; the same pattern in GIA 3828, GRR 825 and GRR
                // 10) give alpha : beta : gamma about 1 : 0.04 : 0.2 at the band
                // (alpha 1.45 per mm at 660nm), so alpha is the deep blue ray, gamma pale
                // blue and beta colorless (the old "maximum along beta" is not
                // supported). Per ray, energy Gaussians fitted by non-linear least
                // squares over 330-950nm (rms 0.010 / 0.001 / 0.002 per mm): alpha a UV
                // edge (323nm), a 425nm brown-centre plateau and the O- band as two
                // Gaussians (622nm and 719nm, FWHM 4930 / 3620 cm^-1: the band is
                // asymmetric, 5489 cm^-1 FWHM overall); beta only its UV edge (centre
                // on the 300nm bound) and two faint bands; gamma its own fit, about 0.2 x
                // alpha at the band. Axes: alpha / beta / gamma are the n_alpha / n_beta / n_gamma
                // directions (`o_ray` / `beta_ray` / `e_ray`), Caltech's alpha, beta
                // and gamma. Strength 0.45 x the crystal's per-mm absorption (the
                // measured crystal is "London"-dark; per mm x 0.45 x 9.905 mm, the
                // face-up path of a 7 mm stone): the swatch is the CLOSEST the measured
                // pleochroism gives to the old one, not the old one: (7 mm, D65, mean of
                // the three rays) L*74 C*14 h 227 against the old L*69 C*45 h 266
                // (dE2000 15.2; alpha L*33 C*43 h 241, beta L*93 C*4, gamma L*74 C*21 h
                // 215). The old swatch coloured all three directions alike; with one
                // deep ray, one colorless and one pale the mean is paler, less saturated
                // and greener (sky blue h 227, the measured band is 620-720nm, not the
                // old 620nm Gaussian of sigma 70nm). The (alpha + beta) / 2 view, what
                // the table sees with gamma along the view axis, is L*73 C*11 h 239.
                // Calibration record: docs/chromophores/calibration-records.md#topaz
                // 2026-11-03 (owner: "match the pictures of actual gemstones"): REPLACED the
                // neutron-irradiated GRR 1632 (the London-blue type: its band runs from 600 to past
                // 800 nm and turns the blue greenish, h 231) by the electron-irradiated Caltech GRR
                // 825c (Brazil, J. Borden, clean blue part, 5.63 mm; the Swiss and sky blue type,
                // its band peaks at 626 nm). Alpha (E par a) and beta (E par b) measured, constant
                // baselines (the 1200-1290 nm level) removed, four energy Gaussians a ray over
                // 330-950 nm (rms 0.004 / 0.003 per mm); gamma is not measured and is taken equal
                // to beta (ESTIMATED). Left out: the broad 400 nm band of both rays, the
                // irradiation brown centre that the heating of commercial Swiss and sky blue topaz
                // removes (with it the stone rendered h 228). Strength 2 x the crystal per mm, x
                // 9.905 to model units. Twenty-two dealer photos of blue topaz (GemSelect) have a
                // median hue of 247 (colored pixels); this renders h 245 (Light Tent; 3 x gave h
                // 251). Swatch: alpha L*78 C*48 h 227, beta and gamma L*82 C*33 h 248, unpolarised
                // L*80 C*37 h 240.
                // 2026-11-04 (owner: "The render seems to err toward saturated"): x 0.7 (1.4 x the
                // crystal per mm). Colored-pixel saturation s_uv: the faceted dealer photos 0.48 (a
                // sky blue), the Commons photos 0.73-0.95 (Swiss blue), the x 1 render 0.86; the
                // factor takes half the gap to the dealer photos in log terms. Swatch alpha L*83
                // C*39 h 225, beta and gamma L*86 C*24 h 246, unpolarised L*85 C*29 h 238.
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
                // n_beta - n_alpha at the D line, LOWER CONFIDENCE (derived, not
                // directly measured for a single specimen). No primary 3-index topaz
                // measurement exists. Source: The Gemology Project's topaz entry,
                // citing ranges n_alpha=1.606-1.634, n_beta=1.609-1.637,
                // n_gamma=1.616-1.644 across topaz's fluorine/hydroxyl compositional
                // range. Midpoints (1.620, 1.623, 1.630) give the fractional position
                // of beta between alpha and gamma: (1.623-1.620)/(1.630-1.620) = 0.30.
                // Applied to this entry's own birefringence_delta (0.0080):
                // delta_beta_alpha = 0.30 * 0.0080 = 0.0024. Implied n_alpha=1.624778,
                // n_gamma=1.632778, both inside the cited ranges.
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
