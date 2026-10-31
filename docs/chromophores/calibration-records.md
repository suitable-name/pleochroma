# Calibration records of the fitted color presets

This page records how each fitted built-in preset was fitted; the numbers themselves live in the code (the Rust file and constant named in each section), never here.
Source studies: [primary measured spectra](research-2026-10-primary-data.md) and [validation against our colorimetry](research-2026-10-validation.md).

Every fact below is taken from the preset's own comment in `crates/pleochroma/src/optics/materials/`; "not stated" means the comment does not say. Reference-spectra CSV files are in `crates/pleochroma/data/reference_spectra/`. All paths and strengths are for the 8.8 mm face-up path of a 7 mm stone unless a row says otherwise. Fields in every section: sample id, source, orientation / rays, temperature, thickness as used, ordinate convention, baseline / reflection treatment, fit domain, deliberate strength adjustments, what is not known, and where the fitted bands live.

## Sapphire

| Field | Record |
|---|---|
| Sample id | The GIA Fe2+-Ti4+ pair cross section (no single stone id is stated) |
| Source | Dubinsky, Stone-Sundberg & Emmett, Gems & Gemology Spring 2020 ([S1] in the code comment); CSV `gia_v3plus_fe2plus_ti4plus_sigma_5nm.csv` (1.98e-18 cm^2 at 580 nm E perpendicular to c, 1.25e-18 at 700 nm E parallel to c) |
| Orientation / rays | o-ray (E perpendicular to c) and e-ray (E parallel to c), three energy-Gaussian bands each |
| Temperature | Room (not stated in the comment) |
| Thickness as used | A density of 10 ppma*cm of the pair, not a stone thickness |
| Ordinate convention | Not stated |
| Baseline / reflection treatment | Not stated |
| Fit domain | Not stated for this entry (bounded 3-band energy-Gaussian fits) |
| Deliberate strength adjustments | None applied. The comment notes that a factor 1.2 would give L* 51 instead of L* 57 |
| Not known | The chemistry of any particular stone; the finished-stone color (the comment gives only the computed swatch, 7 mm, D65: L* 57, C* 40, h 283) |
| Where the bands live | `diamond_through_emerald.rs`, the Sapphire entry's `AbsorptionTensor::uniaxial` (inline bands; no named constant) |

## Ruby

| Field | Record |
|---|---|
| Sample id | GIA sample 1110 (Cr3+ cross sections) |
| Source | Dubinsky, Stone-Sundberg & Emmett, Gems & Gemology Spring 2020 ([S1]); CSV `gia_cr3plus_sample1110_sigma.csv`; 1 ppma = 1.178e17 cm^-3 [S1] |
| Orientation / rays | o-ray (E perpendicular to c) and e-ray (E parallel to c), three bands each (the o-ray is the stronger absorber; the e-ray's 397 nm band carries the measured E-parallel-c near-UV rise) |
| Temperature | Room (not stated in the comment) |
| Thickness as used | Scaled to 800 ppma*cm Cr (about 900 ppma over the 8.8 mm face-up path) |
| Ordinate convention | Not stated |
| Baseline / reflection treatment | Not stated |
| Fit domain | Not stated for this entry (bounded 3-band energy-Gaussian fits) |
| Deliberate strength adjustments | The density 800 ppma*cm is the owner's choice (decision 2026-10-19); 600 ppma*cm would be these peaks times 0.75 |
| Not known | The chemistry of sample 1110 beyond Cr3+ (not stated); the finished-stone color. For comparison the comment gives the swatch (7 mm, D65) L* 41, C* 67, h 356 and GIA Appendix 2 "both" 38.7 / 70.8 / 352 at 750 ppma*cm |
| Where the bands live | `diamond_through_emerald.rs`, the Ruby entry's `AbsorptionTensor::uniaxial` (inline bands; no named constant) |

## Emerald

| Field | Record |
|---|---|
| Sample id | Caltech GRR 3570 |
| Source | G. R. Rossman, Caltech Mineral Spectroscopy Server; CSV `caltech_emerald_grr3570_5nm.csv`; the o/e dichroism is checked against Wood & Nassau 1968 ([S3]: o-ray yellowish green, e-ray bluish green) |
| Orientation / rays | o-ray and e-ray, uniaxial bounded 3-band energy-Gaussian fits |
| Temperature | Room (not stated in the comment) |
| Thickness as used | Scaled to 0.3 cm |
| Ordinate convention | Not stated |
| Baseline / reflection treatment | Not stated |
| Fit domain | Not stated for this entry |
| Deliberate strength adjustments | None stated |
| Not known | The sample's chemistry (not stated in the comment); the finished-stone color. Computed swatch (7 mm, D65): L* 65, C* 101, h 168 |
| Where the bands live | `diamond_through_emerald.rs`, the Emerald entry's `AbsorptionTensor::uniaxial` (inline bands; no named constant) |

## Aquamarine

| Field | Record |
|---|---|
| Sample id | Caltech GRR 1488, Lajes Pintada, Brazil |
| Source | Caltech Mineral Spectroscopy Server; CSVs `caltech_aquamarine_grr1488_eperpc_5p531mm.csv` and `caltech_aquamarine_grr1488_eparc_5p531mm.csv` |
| Orientation / rays | Two polarised spectra, E perpendicular to c (o-ray) and E parallel to c (e-ray); the face-up view mixes both because `c_axis` lies in the table plane |
| Temperature | Room (not stated in the comment) |
| Thickness as used | Recorded at 5.531 mm; used at the crystal's own iron content over the 8.8 mm face-up path |
| Ordinate convention | Not stated |
| Baseline / reflection treatment | Per-ray baselines 0.683 / 0.866 removed (as listed in the comment) |
| Fit domain | 380-780 nm, bounded least-squares energy-Gaussian fits, log residual; the near-infrared band centres sit on their bounds and only their tails matter |
| Deliberate strength adjustments | None stated |
| Not known | The chemistry beyond the iron the fit encodes (not stated); the finished-stone color. Computed swatch (7 mm, D65, (2o + e)/3): L* 85, C* 16, h 247 (o-ray L* 97, C* 13; e-ray L* 44, C* 88, h 287) |
| Where the bands live | `aquamarine_through_citrine.rs`, the Aquamarine entry's `AbsorptionTensor::uniaxial` (inline bands; no named constant) |

## Spessartine

| Field | Record |
|---|---|
| Sample id | Caltech GRR 43, Rutherford Mine, Amelia, Virginia |
| Source | Caltech Mineral Spectroscopy Server; CSV `caltech_spessartine_grr43_2p165mm.csv` |
| Orientation / rays | Isotropic (cubic) |
| Temperature | Room (not stated in the comment) |
| Thickness as used | Recorded at 2.165 mm; scaled to the 8.8 mm face-up path at the sample's own composition (near end-member, Fe-poor "Mandarin") |
| Ordinate convention | Not stated |
| Baseline / reflection treatment | Baseline 0.030 removed |
| Fit domain | 380-780 nm, bounded least-squares energy-Gaussian fit, log residual |
| Deliberate strength adjustments | The 320 nm charge-transfer edge band is a stand-in with its centre on the bound; a weak 590 nm tail band is added to set T(600) = 0.86 (without it L* 80, C* 78) |
| Not known | The finished-stone color; the Fe-richer GRR 72 from the same mine would give L* 51, h 62 (not fitted). Computed swatch (7 mm, D65): L* 77, C* 72, h 66 |
| Where the bands live | `garnets_pyrope_through_spessartine.rs`, function `spessartine_absorption` |

## Tsavorite

| Field | Record |
|---|---|
| Sample id | Caltech GRR 229, Tanzania; Ca2.97 Al1.75 Cr0.01 V0.11 Ti0.02 |
| Source | Caltech Mineral Spectroscopy Server; CSV `caltech_tsavorite_grr229_0p758mm.csv` |
| Orientation / rays | Isotropic (cubic) |
| Temperature | Room (not stated in the comment) |
| Thickness as used | Recorded at 0.758 mm; used over the 8.8 mm face-up path at 0.5 times the sample's vanadium (about 0.9 wt% V2O3) |
| Ordinate convention | Not stated |
| Baseline / reflection treatment | Baseline 0.042 removed |
| Fit domain | 380-780 nm, bounded least-squares energy-Gaussian fit, log residual |
| Deliberate strength adjustments | 0.5 times the sample's vanadium; the comment gives the scaling rule (multiply every peak by s/0.5 for another V content; 0.35 times gives L* 65, 0.18 times, about 0.32 wt% V2O3, gives L* 78) |
| Not known | The finished-stone color. Computed swatch (7 mm, D65): L* 56, C* 86, h 147 |
| Where the bands live | `garnets_grossular_and_andradite.rs`, the Grossular Garnet (Tsavorite) entry's `AbsorptionTensor::isotropic` (inline bands; no named constant) |

## Demantoid

| Field | Record |
|---|---|
| Sample id | GIA 2952, a Russian demantoid slab |
| Source | Caltech Mineral Spectroscopy Server; CSV `caltech_demantoid_gia2952_2p5mm.csv`; the near-infrared band is compared with Stubna et al. 2019 (861 nm) |
| Orientation / rays | Isotropic (cubic) |
| Temperature | Room (not stated in the comment) |
| Thickness as used | Recorded at 2.5 mm; used at the slab's own composition over the 8.8 mm face-up path |
| Ordinate convention | Not stated |
| Baseline / reflection treatment | Scattering baseline 0.816 removed |
| Fit domain | 380-780 nm, bounded least-squares energy-Gaussian fit, log residual |
| Deliberate strength adjustments | None applied. The comment notes that 0.75 times all peaks would give L* 50, and that the heated Ural demantoid GRR 2667 is 2.6 times more absorbing per cm (near-black at 7 mm; not used) |
| Not known | The finished-stone color. Computed swatch (7 mm, D65): L* 41, C* 92, h 140 |
| Where the bands live | `garnets_grossular_and_andradite.rs`, the Andradite Garnet (Demantoid) entry's `AbsorptionTensor::isotropic` (inline bands; no named constant) |

## Tanzanite

| Field | Record |
|---|---|
| Sample id | Caltech GRR 2292, blue zone (Arusha, Tanzania); cross-checked against the digitised Caltech GRR 3045 after-heating curves |
| Source | Caltech Mineral Spectroscopy Server; CSVs `caltech_zoisite_grr2292_blue_a_gamma_3p150mm.csv`, `caltech_zoisite_grr2292_blue_b_alpha_5p142mm.csv`, `caltech_zoisite_grr2292_blue_c_beta_3p150mm.csv`; check file `caltech_zoisite_grr3045_after_heating_digitized_5p747mm.csv` |
| Orientation / rays | Three polarised rays: E parallel to a = gamma and E parallel to c = beta (3.150 mm piece), E parallel to b = alpha (5.142 mm piece); stored as `o_ray` (alpha), `beta_ray`, `e_ray` (gamma) |
| Temperature | Room (not stated in the comment) |
| Thickness as used | Recorded at 3.150 mm and 5.142 mm; used at the crystal's own concentration (about 0.6 wt% V2O3) over the 8.8 mm face-up path |
| Ordinate convention | Not stated |
| Baseline / reflection treatment | Near-infrared baselines removed (values not stated) |
| Fit domain | 380-780 nm, bounded least-squares energy-Gaussian fits, log residual |
| Deliberate strength adjustments | The alpha ray is scaled by 2.119 so that alpha(540 nm) / gamma(600 nm) = 1.48, the ratio measured on single crystals (GRR 1265: 1.49, GRR 3045: 1.47; the two-piece 2292 pair gives 0.70) |
| Not known | That the blue zone's spectrum is the heated stone's is an inference from the absent 445 nm band, agreed by the GRR 3045 after-heating curves; the finished-stone color is not known. Computed swatch (7 mm, D65): L* 45, C* 49, h 292 (alpha L* 31, beta L* 45, gamma L* 54) |
| Where the bands live | `tanzanite_through_cubic_zirconia.rs`, function `tanzanite_absorption` |

The principal indices and the open question of the GRR 2292 normalisation have their own sections below ("Tanzanite indices", "Tanzanite GRR 2292 normalisation").

## Peridot

The Rust file of this entry (`peridot_through_benitoite.rs`) links here with a `// Calibration record:` line at the Peridot fit comment.

| Field | Record |
|---|---|
| Sample id | Caltech GRR 418, San Carlos olivine |
| Source | G. R. Rossman, Caltech Mineral Spectroscopy Server; CSVs `caltech_olivine_grr418_{alpha,beta,gamma}.csv` (URLs in their headers); compared with the GIA Sardinia peridot characterisation ([S4]: 473/490 nm Fe2+ bands, total absorption below about 440 nm, broad band near 1050 nm with an 830 nm shoulder) |
| Orientation / rays | Three principal rays alpha / beta / gamma, stored as `o_ray`, `beta_ray`, `e_ray` |
| Temperature | Room (not stated in the comment) |
| Thickness as used | Scaled to 0.88 cm, the 8.8 mm face-up path |
| Ordinate convention | Not stated |
| Baseline / reflection treatment | Not stated |
| Fit domain | 380-780 nm, bounded least-squares fits, log residual (fitted 2026-10-19) |
| Deliberate strength adjustments | The 345 / 341 / 376 nm bands stand in for the ultraviolet edge; the 840 / 1000 nm bands stand in for the Fe2+ near-infrared pair (fitted amplitudes sat on the fit bound; only their 650-780 nm tails matter) |
| Not known | The finished-stone color. Computed swatch (7 mm, D65): L* 76, C* 63, h 111 |
| Where the bands live | `peridot_through_benitoite.rs`, function `peridot_absorption` |

## Tanzanite indices

Re-based on 2026-10-26 to a measured triplet. Not an absorption fit: the record covers the refractive-index data of the same entry.

| Field | Record |
|---|---|
| Sample id | Hurlbut's gem zoisite from Tanzania (no specimen number read) |
| Source | C. S. Hurlbut Jr., American Mineralogist 54, 702 (1969), abstract read on the scanned paper (the Table 2 index column is not legible on the scan): alpha 1.6925, beta 1.6943, gamma 1.7015, +-0.0001, minimum deviation, Na light; X = b, Y = c, Z = a; optically positive; 2V = 53 degrees; r > v |
| Orientation / rays | Three principal indices; prisms with prism axes a and c. For zoisite gamma lies along crystallographic a and beta along c, so the field `c_axis` in the code is the optical gamma direction, not the crystallographic c |
| Temperature | Room (not stated) |
| Thickness as used | Not applicable (index data) |
| Ordinate convention | Not applicable |
| Baseline / reflection treatment | Not applicable |
| Fit domain | The base curve is n_beta (the biaxial convention of `GemMaterial::biaxial_indicatrix`): n_beta(589.3 nm) = 1.6943 and the dispersion shape B = 0.009123 (the International Gem Society B-G figure 0.0301 converted by 0.579), unchanged; A = 1.6943 - 0.009123 / 0.5893^2 = 1.668029; Delta n(F-C) = 0.017422, V_d = 39.85 |
| Deliberate strength adjustments | None. `birefringence_delta` 0.0090 (gamma - alpha) and `biaxial_delta_beta_alpha` 0.0018 (beta - alpha), both from the triplet; the sign (0.0018 < 0.0072) is the declared positive |
| Not known | The dispersion shape is a lower-confidence conversion, not a measured fit; the other specimens' indices; the triplet's wavelength dependence. Consistency check from the triplet: tan^2 V_z = (1/alpha^2 - 1/beta^2) / (1/beta^2 - 1/gamma^2) = 0.00074 / 0.00294 gives 2V = 53 degrees, Hurlbut's own value |
| Where it lives | `tanzanite_through_cubic_zirconia.rs`, the Tanzanite entry's `dispersion`, `birefringence_delta` and `biaxial_delta_beta_alpha` |

The earlier values (n_beta 1.700858, a two-point construction slightly above the International Gem Society range; triplet 1.6915 / 1.6935 / 1.7020, which is not Hurlbut's) are superseded.

## Tanzanite GRR 2292 normalisation

An unresolved data question, recorded rather than settled; no factor is applied.

| Field | Record |
|---|---|
| The question | The Caltech zoisite page says every listed spectrum is "normalized to X mm" (GRR 2292: 3.0 mm). The GRR 1265 file header says "3.00 mm (normalized)"; the GRR 2292 file headers say "3.150 mm" (a, c) and "5.142 mm" (b) with no "normalized" |
| Call | Apply no factor. The effect is bounded at 5 percent on beta and gamma (3.150 / 3.0) and is none on alpha |
| Why alpha is untouched | The alpha ray's absolute strength is not taken from the 5.142 mm b-file: it is anchored to gamma through the single-crystal ratio alpha(540 nm) / gamma(600 nm) = 1.48 (GRR 1265, GRR 3045), so the b-file's thickness never reaches the rendered stone |
| Evidence | Under the page's reading the two-piece pair's raw ratio 0.70 becomes 0.70 x (5.142 / 3.0) / (3.150 / 3.0) = 1.14, most of the way to the single-crystal 1.48, and the ad-hoc factor would be 1.30 instead of 2.119; so 2.119 reads as 1.3 of real ratio correction plus the normalisation the files already carry. The 1265 header convention points the other way |
| Open | Only G. R. Rossman can close it. Notes: `docs/chromophores/primary-2026-10/data/tanzanite/NOTES.md`. Ten rows of the b-file were restored afterwards without re-running the fit |

## Benitoite

| Field | Record |
|---|---|
| Sample id | Caltech GRR 561-D4, dark-blue zone, The Gem Mine, San Benito County, California |
| Source | G. R. Rossman, Caltech Mineral Spectroscopy Server: `http://minerals.gps.caltech.edu/Files/Visible/benitoite/benitoite561-D4D.A.txt` (E perpendicular to c) and `.../benitoite561-D4D.C.txt` (E parallel to c); the figure of Laurs, Rohtert & Gray, Gems & Gemology 33(3), 166-187 (1997), Fig. 28, shows the same spectrum (E parallel to c band near 700 nm running into the near infrared, a weak E perpendicular curve) |
| Orientation / rays | o-ray (E perpendicular to c) and e-ray (E parallel to c), one energy Gaussian each |
| Temperature | Room (not stated) |
| Thickness as used | Recorded at 0.997 mm; the strength is not scaled from it, see the adjustments row |
| Ordinate convention | Decadic absorbance (net amplitudes) |
| Baseline / reflection treatment | Linear baseline plus one Gaussian, fitted together; net amplitudes used. Baseline offset about 0.46 absorbance |
| Fit domain | 420-1600 nm (the file runs 300-2599 nm); rms residual 0.042 absorbance |
| Deliberate strength adjustments | Strength is tuned: the measured net 2.73 over 0.997 mm gives A = 19 (decadic) along c over 7 mm, opaque, and a dark-blue zone is not a gem; no concentration is published. The e-ray peak stays at 2.0 model units (the swatch depth of the earlier entry); the o-ray peak follows the measured ratio, 2.0 x 0.053, written 0.11. Only the SHAPE is fitted |
| Fitted shape | e-ray centre 698 nm (1.776 eV), FWHM 0.88 eV (7100 cm^-1); o-ray centre 603 nm (2.057 eV), FWHM 1.07 eV (8630 cm^-1), net amplitude 0.146 against 2.73 for the e-ray (ratio 0.053, poorly defined). The measured e-ray band is asymmetric (half-maximum at 559 and 929 nm; 1.47 absorbance at 1000 nm); one Gaussian is the model's simplification |
| Not known | The chemistry of the sample and the gem-grade concentration; the cause of color (Fe2+-Ti4+ or Fe2+-Fe3+ charge transfer and Zr in the Ti site have been proposed, none confirmed); the ultraviolet edge (0.62 absorbance at 350 nm, both rays) is not modelled. The earlier 635 nm / sigma 60 nm band and its Fe2+-Ti4+ attribution were tuned, not measured |
| Where the bands live | `peridot_through_benitoite.rs`, the Benitoite entry's `AbsorptionTensor::uniaxial` (inline bands; no named constant) |

The color-chemistry guide `garnet_peridot_zircon_benitoite.md` (section 7) still describes the earlier band; it is kept unedited and this record supersedes it.

## Spinel (red)

| Field | Record |
|---|---|
| Sample id | Caltech JQ-13, natural red spinel, Sri Lanka (Ceylon) |
| Source | Caltech Mineral Spectroscopy Server, `http://minerals.gps.caltech.edu/Files/Visible/spinel/sp1-6` (the page lists it as 1.50 mm; the file header says 1.52 mm); cross-check Vasiliev's synthetic flux-grown red Cr spinel, Novosibirsk (10 mm normalized, 25 K), which gives the same peak ratio |
| Orientation / rays | Isotropic (cubic), unpolarised |
| Temperature | Room for JQ-13; 25 K for the synthetic cross-check |
| Thickness as used | Recorded at 1.52 mm; the strength is the tuned one of the 540 nm band, see the adjustments row |
| Ordinate convention | Absorbance |
| Baseline / reflection treatment | The file is "corrected for baseline"; no further treatment |
| Fit domain | Gaussians in energy with a quadratic baseline over 330-800 nm for the read of the measured widths and centres; the preset itself uses wavelength-domain Gaussians (`AbsorptionBand::new`) |
| Deliberate strength adjustments | 540 nm band: centre cited (measured 538 nm), width (sigma 55 nm) and peak (2.4) stay tuned; the measured FWHM is 77 nm (sigma 33 nm), narrower, and narrowing it changes the look, a separate owner decision. 395 nm band added: centre 395 nm (fit 398 nm, raw maximum 391 nm, +-10 nm), sigma 27 nm (FWHM 0.48 eV, about 60-65 nm), peak 1.6 x 2.4 = 3.84. The peak ratio is 1.62 raw (natural) and 1.61 (synthetic); the baseline-subtracted ratio is 2.1 and the Gaussian-amplitude ratio 1.37, so the range is 1.4-2.1 |
| Not known | The chemistry of JQ-13; the ultraviolet edge below 380 nm (0.65 absorbance at 350 nm) is not modelled; published peak tables (Taran, Andreozzi, Malsy and Klemm) were not compared; the width of the 395 nm band is uncertain because its ultraviolet side is entangled with the edge. Blue spinel (Co2+) is a different chromophore and is not modelled |
| Where the bands live | `spinel_through_tourmaline.rs`, the Spinel entry's `AbsorptionTensor::isotropic` (inline bands; no named constant) |

## GGG dispersion

A refractive-index record, not an absorption fit (the preset is colorless).

| Field | Record |
|---|---|
| Sample id | Average of four crystals (Wood & Nassau Table I) |
| Source | D. L. Wood and K. Nassau, "Optical properties of gadolinium gallium garnet," Applied Optics 29, 3704-3707 (1990), DOI 10.1364/AO.29.003704; the journal's abstract page (Tables I-III) was read, the full text was not |
| Orientation / rays | Isotropic (cubic) |
| Temperature | Room (not stated) |
| Thickness as used | Not applicable |
| Ordinate convention | n^2 = 1 + sum A_i l^2 / (l^2 - L_i^2), l and L_i in micrometers, A = (1.7727, 0.9767, 4.9668), L = (0.1567, 0.01375, 22.715). The equation is not printed on the page; it was verified numerically (the L_i read as resonance wavelengths reproduce the printed He-Ne value 1.96515 exactly; read as squared wavelengths they give 2.199, wrong). Stored as Sellmeier3 with c_i = L_i^2 = (0.024555, 0.000189, 515.971) |
| Baseline / reflection treatment | Not applicable |
| Fit domain | The paper's fit; its stated transparency range is 0.36-6.0 um (the fit window is not stated). The renderer's numerical floor is 300 nm (both ultraviolet poles, 156.7 and 13.75 nm, lie below it; the 22.7 um pole is far above the band), so 300-360 nm is extrapolation |
| Deliberate strength adjustments | None. Values from the coefficients: n_F(486.13) 1.98833, n_d(587.56) 1.97036, n_D(589.3) 1.97014, n_C(656.27) 1.96287, Delta n(F-C) 0.02546, V_d 38.11 |
| Not known | The paper's printed Table II n_F (1.98337) and n_F - n_C (0.02046) disagree with its coefficients, n_D and n_C by exactly 0.005, a digit slip; the coefficients are taken as primary. The earlier Cauchy (the gemological B-G 0.045 x 0.579 = 0.026055) was 2.3 percent above the measured Delta n(F-C) |
| Where it lives | `peridot_through_benitoite.rs`, the GGG entry's `dispersion` |

## Per-axis defaults of the anisotropic hosts (2026-10-27)

Owner decision of 2026-10-27 (color variants campaign, lane B1): a built-in material on an anisotropic host never carries an isotropic tensor, and a stone's default color is its own measured pleochroism. The eight sections below, and the colorless one after them, record the entries rebuilt under that rule. The research behind them is `docs/color_variants/R1_corundum_spinel_chrysoberyl.md`, `R2_beryl_topaz_tourmaline_andalusite.md`, `R3_garnet_peridot_zircon_benitoite_tanzanite.md` and `R4_quartz_diamond_opal_synthetics_glass.md`.

Conventions of these eight sections (they differ from the older ones above in the path):

- The unit stays `ModelUnit`. A stored peak is the measured per-millimetre coefficient times a strength factor times 9.905 mm, which is 3.5 mm per model unit (a 7 mm stone over the reference width 2.0) times `MODEL_UNIT_FACE_UP_PATH` 2.83 (re-pinned 2026-10-27); the older records use the former constant, 8.8 mm. Over one model unit the swatch is what `color::body_color::body_colors` returns, and that is the swatch quoted: 7 mm, D65, 1 nm integration, unpolarised (2 `T_o` + `T_e`) / 3 for a uniaxial and the mean of the three rays for a biaxial tensor.
- The color pipeline of the fits was reproduced outside the repo (CIE 1931 table and the D65 table parsed from `cie1931.rs` and `spectral.rs`, Lab against the illuminant's own white) and checked before use against three recorded swatches at one model unit: Sapphire L* 56.7 C* 39.4 h 282.5 (record L* 57, C* 40, h 283), Ruby 40.5 / 67.7 / 355.6 (41 / 67 / 356) and Aquamarine 85.2 / 16.0 / 246.6 (85 / 16 / 247). Every energy band is stored as `AbsorptionBand::energy(centre, FWHM in cm^-1, peak)`.
- "Strength" is the one scale factor of a section. dE2000 is against the swatch the entry had before 2026-10-27, computed from the old bands by the same pipeline. The values are a hand evaluation of the stored numbers, not a run of the app: the owner's run of `built_in_hues.rs` is the check.
- Fit scripts and outputs are not part of the repo; they were run in the session scratchpad. CSV file names refer to `crates/pleochroma/data/reference_spectra/`, which the library does not read.

### Amethyst

| Field | Record |
|---|---|
| Sample id | Caltech Anahi Mine, Bolivia, amethyst region, `ameth.bo`: a 6.17 mm slab cut perpendicular to c (0001), so the beam runs along c and E is perpendicular to c (the o-ray). No e-ray spectrum |
| Source | G. R. Rossman, Caltech Mineral Spectroscopy Server, `http://minerals.gps.caltech.edu/FILES/Visible/quartz/ameth.bo`; CSV `variant_amethyst_deep_siberian_eperpc.csv` (full resolution; the 5 nm digest is `caltech_quartz_amethyst_ameth_bo_5nm.csv`). Band assignment: Cox, J. Phys. C 10, 4631 (1977) (545, 357 and about 950 nm of the Fe4+ [FeO4]0 centre), as quoted in R4; Caltech Ametrine page (about 40 ppm iron, radiation oxidises Fe3+ to Fe4+) |
| Orientation / rays | o-ray measured; e-ray ESTIMATED (owner review 2026-10-27, F-19): an unmeasured ray built from the o-ray bands with the Report's per-band e/o weights 1.0 (357 nm), 1.0 (416 nm), 0.7 (545 nm), 0.5 (950 nm) (`research-2026-10-polarized-spectra.md`, secondary). The Material Default's e-ray is invisible while the stone is viewed along c, and the variants `amethyst.deep_siberian` / `rose_de_france` carry the same estimated e-ray (their first source line says so) |
| Temperature | Room (not stated) |
| Thickness as used | Recorded at 6.17 mm; used at 2.47 times the crystal's per-mm absorption over the 9.905 mm face-up path, ONE factor over every band |
| Ordinate convention | Decadic absorbance; alpha = 2.3026 (A - baseline) / path |
| Baseline / reflection treatment | Constant A 0.217 removed (the free constant of the fit) |
| Fit domain | 330-1050 nm, the free four-band energy-Gaussian fit that the `amethyst.deep_siberian` variant carries (lane F4): 364.3 / 416.1 / 537.1 / 953.8 nm, FWHM 4509 / 2911 / 5175 / 3073 cm^-1, per-mm peaks 0.612 / 0.163 / 0.359 / 0.0794 (x 4.1 in the variant's own convention), rms 0.0041 A, model / data 0.98-1.02 from 400 to 650 nm. The first per-axis version of this entry (fixed Report centres and widths, amplitudes only: 357 / 545 / 950 nm, FWHM 6593 / 3480 / 4498 cm^-1) did not reproduce the slab: o-ray L* 54 C* 66 h 322 against the data x 2.47 at L* 52 C* 42 h 329 (dE2000 about 10; model / data 0.47 at 450 nm, 2.3 at 550 nm, 0.32 at 650 nm). Replaced 2026-10-27 (review F-6) |
| Deliberate strength adjustments | 2.47 times the measured crystal over every band (the low end of the range R4 gives for "Deep Siberian", 2.5-3.5 times; Anahi itself is a pale to medium stone, T(545 nm) = 0.44 over 8.8 mm). Python replica of the app's colorimetry (not a run): with c along the view the stone shows the o-ray, L* 50 C* 41 h 329 (the measured slab at 2.47 times: 50 / 40 / 329); unpolarised mean L* 54 C* 35 h 331; e-ray L* 61 C* 27 h 336. The old tuned stand-in was L* 59 C* 66 h 321: the measured crystal is less saturated, and the measured value wins |
| Not known | The e-ray (no primary polarized amethyst spectrum was found; its weights are the Report's); the 357 nm part, which lies at the edge of the data; the absolute iron of the stone |
| Where the bands live | `amethyst_through_citrine.rs`, the Amethyst entry's `AbsorptionTensor::uniaxial` (inline bands; no named constant) |

The Ametrine built-in is not touched: its zones are the chromophore catalogue's quartz recipes and its plain absorption is the amethyst zone, not this entry.

### Citrine

| Field | Record |
|---|---|
| Sample id | None: the 16-point citrine alpha(lambda) table of `citrine_annealed_amethyst.csv` (350 nm 14.5 cm^-1, 400 nm 7.4, 450 nm 2.8, 500 nm 0.95, 545 nm 0.35, 605 nm 0.12, 800 nm 0.02). The only digital Caltech citrine spectrum is a JPG (GRR 3910) |
| Source | `research-2026-10-polarized-spectra.md` via the CSV, user-delivered, unverified (SECONDARY: the repo's validation disagrees with the table's own color by dE2000 12.7); cause: Caltech quartz page and Ametrine page (Fe3+ in small hydrated oxide aggregates, heated amethyst) |
| Orientation / rays | No polarized spectrum; the gemological pleochroism of citrine is very weak, so the o-ray and e-ray sets are EQUAL, stored pleochroic (`AbsorptionTensor::uniaxial`) |
| Temperature | Room (not stated) |
| Thickness as used | A coefficient table, not a stone; used at 0.53 times the table over the 9.905 mm face-up path |
| Ordinate convention | Napierian alpha in cm^-1 (the table), converted to per mm by 1/10 |
| Baseline / reflection treatment | None |
| Fit domain | Three energy Gaussians, centres 330 / 410 / 500 nm, FWHM 8477 / 7064 / 5181 cm^-1, relative-error weighted non-negative least squares on the 16 points: per-mm peaks 1.656 / 0.136 / 0.023; the table is followed within 10 % at 370-500 nm, +5 % at 350 nm, +17-20 % at 520-545 nm, and the faint tail beyond 635 nm (alpha under 0.1 cm^-1) is 30-50 % low |
| Deliberate strength adjustments | 0.53 times the table so the swatch stays the old one: L* 92 C* 51 h 94 against the old L* 93 C* 49 h 91 (dE2000 2.0). The table's own density would be L* 86 C* 75 |
| Not known | Any measured polarization of citrine; the chemistry of the table's stone (the repo's calibration assumes 10 ppma iron for the edge, secondary) |
| Where the bands live | `amethyst_through_citrine.rs`, the Citrine entry's `AbsorptionTensor::uniaxial` (inline bands; no named constant) |

The Ametrine's citrine zone is the catalogue recipe, not this entry (see Amethyst).

### Morganite

| Field | Record |
|---|---|
| Sample id | Caltech GRR 602, red beryl, Wah Wah Mountains, Utah, 1.559 mm, used as the SHAPE donor (same ion, same Al site, same host). No digital morganite spectrum exists (GRR 508 and 2536b are figures; none was digitised) |
| Source | G. R. Rossman, Caltech Mineral Spectroscopy Server; CSVs `variant_red_beryl_grr602_eperpc.csv` and `variant_red_beryl_grr602_eparc.csv`. Mn3+ in morganite: GIA G&G Fall 2024 (about 567 nm, search summary), Caltech GRR 2536b page text (a manganese feature near 560 nm); pleochroism "light red to light violet" (beryl reference, search summary) |
| Orientation / rays | o-ray (E perpendicular to c): the red beryl o bands (UV edge 313 nm, 440, 496 and 555 nm); e-ray (E parallel to c): UV edge 320 nm, 440 and 544 nm. The measured positions reproduce the documented pleochroism, o-ray light red / pink, e-ray light violet (the shipped e-ray is L* 75 C* 31 h 332) |
| Temperature | Room (not stated) |
| Thickness as used | Recorded at 1.559 mm; used at 0.20 times the red beryl's per-mm absorption over the 9.905 mm face-up path |
| Ordinate convention | Decadic absorbance; alpha = 2.3026 (A - baseline) / path |
| Baseline / reflection treatment | Per-ray baselines 0.067 (o) and 0.338 (e) A per mm removed (as R2 states them; the e-ray pedestal is unusually high) |
| Fit domain | The free energy-Gaussian fit of both rays of GRR 602 that the `emerald.red_beryl` variant carries (lane F2): o 312.7 / 440.0 / 495.9 / 554.6 nm, e 319.9 / 440.0 / 543.6 nm; per-mm peaks (o) 1.17 / 0.0510 / 0.330 / 0.296, (e) 1.194 / 0.165 / 0.514. The first per-axis version kept one band per ray (538 nm, FWHM 5166 cm^-1, 0.445 per mm; 561 nm, FWHM 3560 cm^-1, 0.514 per mm), dropped the 440 and 496 nm structure of the donor and came out violet (L* 75 C* 40 h 311) where red beryl at the same strength is pink (77 / 33 / 344, dE2000 about 10). Replaced 2026-10-27 (review F-7) |
| Deliberate strength adjustments | 0.202 times the red beryl over every band, the UV edge included (ONE factor). Python replica of the app's colorimetry (not a run): with c along the view the stone shows the o-ray, L* 81 C* 33 h 347 (light pink); unpolarised mean L* 79 C* 32 h 342; e-ray L* 75 C* 31 h 332 (light violet). The old tuned stand-in was L* 77 C* 47 h 325 |
| Not known | Any morganite spectrum (position, width and strength are from red beryl); the manganese content; the e/o ratio |
| Where the bands live | `aquamarine_through_citrine.rs`, the Morganite entry's `AbsorptionTensor::uniaxial` (inline bands; no named constant) |

### Chrysoberyl (Yellow)

| Field | Record |
|---|---|
| Sample id | Caltech GRR 171, greenish-yellow chrysoberyl, Minas Gerais, Brazil, 1.193 mm (beta and gamma rays) |
| Source | G. R. Rossman, Caltech Mineral Spectroscopy Server; CSVs `variant_chrysoberyl_yellow_grr171_beta_1p193mm.csv` and `variant_chrysoberyl_yellow_grr171_gamma_1p193mm.csv`; Fe3+ bands: Farrell & Newnham, Am. Mineral. 50, 1972 (1965) (little or no pleochroism at 77 K, 615 / 500 nm broad, 440 / 385 / 378 / 368 / 355 nm sharp); R1 section 4 |
| Orientation / rays | beta = E parallel a (`beta_ray`), gamma = E parallel b (`e_ray`), alpha = E parallel c (`o_ray`), the mapping of the Alexandrite entry. The GRR 171 headers give the cell as Pnma (a 9.40, b 5.48, c 4.43 A) and the polarizations "E||a = beta", "E||b = gamma", and GRR 874 states the beta ray vibrates along the 9.40 A axis, the setting Walling's sort of Alexandrite uses (review F13: checked, the slot assignment holds). Beta and gamma measured; alpha was never measured (no file exists) and is ESTIMATED as the mean of the beta and gamma spectra |
| Temperature | Room |
| Thickness as used | Recorded at 1.193 mm; used at 0.7 times GRR 171's per-mm absorption over the 9.905 mm face-up path, ONE factor over every band of every ray |
| Ordinate convention | Decadic absorbance; alpha = 2.3026 (A - baseline) / path |
| Baseline / reflection treatment | Constant A 0.255 (beta) and 0.16 (gamma) removed (the 800 nm minima; the pedestal is about 0.26 / 0.17) |
| Fit domain | 330-800 nm, five free-centre energy Gaussians per ray (centres within 10 nm of 340 / 368 / 378 / 440 / 500 nm, a UV edge, 369-378 nm, the 436-441 nm band and a broad one near 490 nm), non-linear least squares: rms 0.018 (beta), 0.034 (gamma), 0.026 (the mean spectrum, alpha) per mm. The gamma 441 nm band is about 3.5 times the beta one (R1: net 12.7 against 4.3 cm^-1), which contradicts F&N's "little or no pleochroism" at 77 K, so the pleochroism is kept but unconfirmed |
| Deliberate strength adjustments | 0.7 times GRR 171 (owner decision 2026-10-27: a golden yellow; the earlier 0.25 was a pale greenish yellow that duplicated the `Greenish Yellow` variant, 0.3 times the same spectrum, review F-9 / F14). Python replica of the app's colorimetry (not a run): with gamma along the view (`c_axis` Y) the stone shows alpha and beta, L* 74 C* 48 h 88; unpolarised mean L* 72 C* 53 h 83 (alpha L* 72 C* 56, beta L* 76 C* 42, gamma L* 69 C* 67). The old tuned stand-in was L* 97 C* 56 h 103, a pair no measured Fe3+ spectrum gives. The `Yellow` variant of the old list was dropped; `Greenish Yellow` (0.3 times) stays, and `Honey` stays dropped (owner decision: no usable data) |
| Not known | The alpha ray; the dichroism (conflicting sources); the iron content (about 2.5 wt% Fe2O3 on average for yellow chrysoberyl, GRR 171's own is not published); the stone's strength |
| Where the bands live | `aquamarine_through_citrine.rs`, the Chrysoberyl (Yellow) entry's `AbsorptionTensor::biaxial` (inline bands; no named constant) |

### Topaz

| Field | Record |
|---|---|
| Sample id | Caltech GRR 1632, neutron-irradiated blue topaz, Nigeria, 3.462 mm, three rays (the same polarisation pattern in GIA 3828 (natural blue, Zimbabwe), GRR 825 and GRR 10; R2 section 3.5) |
| Source | G. R. Rossman, Caltech Mineral Spectroscopy Server; CSVs `variant_topaz_blue_irradiated_grr1632_{alpha,beta,gamma}.csv`; mechanism: K. Nassau, "Gemstone Enhancement" (O- hole centre near 620-660 nm) |
| Orientation / rays | alpha / beta / gamma are the Caltech labels (E parallel to alpha, beta, gamma), stored as `o_ray`, `beta_ray`, `e_ray` (`biaxial(alpha, beta, gamma)`). The old entry's "maximum along beta" is not supported: the band is strongest along alpha (alpha : beta : gamma about 1 : 0.04 : 0.2 at 660 nm; the model gives 6.45 : 0.17 : 1.24) |
| Temperature | Room |
| Thickness as used | Recorded at 3.462 mm (the index page says "plotted as 3.0 mm"; the header thickness is used, so the absolute scale is uncertain by that 0.87-1.15); used at 0.45 times the crystal's per-mm absorption over the 9.905 mm face-up path |
| Ordinate convention | Decadic absorbance; alpha = 2.3026 (A - baseline) / path |
| Baseline / reflection treatment | Per-ray baselines 0.007 / 0.004 / 0.010 A per mm (A 0.024 / 0.014 / 0.035) removed |
| Fit domain | 330-950 nm, free-centre energy Gaussians, non-linear least squares, weights 1 over 380-780 nm and 0.4 outside: alpha four bands (a UV edge 323 nm, a 425 nm plateau, the O- band as two Gaussians at 622 and 719 nm, FWHM 4930 / 3620 cm^-1; rms 0.010 per mm); beta three (a UV edge on its 300 nm bound and two faint bands; rms 0.001); gamma four (rms 0.002) |
| Deliberate strength adjustments | 0.45 times GRR 1632 ("London"-strength). The swatch is the CLOSEST the measured pleochroism gives to the old one, not the old one: L* 74 C* 14 h 227 (alpha L* 33 C* 43 h 241, beta L* 93 C* 4, gamma L* 74 C* 21 h 215) against the old isotropic L* 69 C* 45 h 266 (dE2000 15.2, the minimum over the strength). The old swatch colored all three directions alike; with one deep, one colorless and one pale ray the mean is paler, less saturated and greener. The (alpha + beta) / 2 view is L* 73 C* 11 h 239. The old 620 nm band (sigma 70 nm) was a tuned stand-in for a measured band at 620-720 nm |
| Not known | Which crystallographic axis alpha, beta and gamma are (the Caltech labels are used, R2 section 3.1); the thickness question above; Sky / Swiss / London are trade names tied to dose, the entry is one stone |
| Where the bands live | `zircon_through_topaz.rs`, the Topaz entry's `AbsorptionTensor::biaxial` (inline bands; no named constant) |

### Zircon

| Field | Record |
|---|---|
| Sample id | Caltech GRR 3106, zircon, green region, Mogok, Myanmar, 2.592 mm (merged spectra, sharp U4+ lines at 482.8, 515.7, 537.2, 562.3, 589.7, 653.4, 690.9 nm along E perpendicular to c) |
| Source | G. R. Rossman, Caltech Mineral Spectroscopy Server; CSVs `variant_zircon_green_eperpc.csv` and `variant_zircon_green_eparc.csv`; line widths and the e-ray list: R3 section 4 (FWHM 65-170 cm^-1); U4+ lines: Fielding (1970), Nasdala et al., Rev. Mineral. Geochem. 53 (2003) |
| Orientation / rays | o-ray (E perpendicular to c) and e-ray (E parallel to c), measured; they differ (653 nm about twice as strong along o, 691 nm o only, 616 nm e only), so the tensor is the measured split, not equal sets |
| Temperature | Room |
| Thickness as used | Recorded at 2.592 mm; used at 1.0 times the crystal's per-mm line strength over the 9.905 mm face-up path, no other scaling |
| Ordinate convention | Decadic absorbance; net line height alpha = 2.3026 (A_peak - A_local background) / path |
| Baseline / reflection treatment | LOCAL background (the mean absorbance in 5-12 nm either side of each line) removed, so the broad radiation-damage continuum under the lines (which makes zircon brown, red-brown or green) is not in this default |
| Fit domain | Line by line: o-ray 483.4 / 515.7 / 537.2 / 562.3 / 589.7 / 653.4 / 659.5 / 690.9 nm (net 0.318 / 0.388 / 0.294 / 0.177 / 0.339 / 1.155 / 0.31 / 0.68 per mm; FWHM 120 / 170 / 120 / 100 / 100 / 105 / 150 / 65 cm^-1, the 483, 537 and 562 nm widths estimated); e-ray 511.5 / 542.0 / 589.1 / 616.0 / 652.8 / 660.5 nm (net 0.331 / 0.185 / 0.483 / 0.352 / 0.548 / 0.436 per mm; FWHM 150 / 120 / 100 / 150 / 110 / 80 cm^-1). The weak e-ray 465, 484 and 538 nm features (below 0.08 per mm) are dropped; the o-ray list is the 8 bands the GPU holds |
| Deliberate strength adjustments | None beyond the path conversion (strength 1.0). Swatch L* 93 C* 12 h 303 against the old L* 93 C* 13 h 279 (dE2000 6.3; o-ray L* 93 C* 14 h 318, e-ray L* 94 C* 10 h 256; the minimum over the strength is 5.6 at 0.5). The old five lines (sigma 6-8 nm, peaks 2.4 / 0.5) were an unpolarised, tuned stand-in; the measured lines are 3-7 nm FWHM |
| Not known | The line shapes below the instrument resolution (peak heights of lines this narrow depend on it); the broad-band damage colors (variants); the uranium content |
| Where the bands live | `zircon_through_topaz.rs`, the Zircon entry's `AbsorptionTensor::uniaxial` (inline bands; no named constant) |

### Tourmaline

| Field | Record |
|---|---|
| Sample id | Caltech GRR 1935, small green crystal, elbaite, Usakos Mine, Namibia, 3.645 mm (baseline corrected by the contributor). Cross-checks: GRR 1935 blue and GRR 3770 pure blue, both with the same Fe2+ intensity in both rays (o 0.905 / e 0.994 and 1.00 / 0.95 per mm; the green crystal 0.54 / 0.475) |
| Source | G. R. Rossman, Caltech Mineral Spectroscopy Server; CSVs `variant_tourmaline_verdelite_grr1935_green_eperpc.csv` and `..._eparc.csv` (also `variant_tourmaline_indicolite_grr1935_blue_*` and `variant_tourmaline_indicolite_grr3770_*`); Mattson & Rossman, Phys. Chem. Minerals 14, 163 (1987); Rossman & Mattson, Am. Mineral. 71, 599 (1986) (Mn2+-Ti4+ charge transfer at 325 nm with a tail into the visible); R2 section 4 |
| Orientation / rays | o-ray (E perpendicular to c, the "a" files) and e-ray (E parallel to c, the "c" files). Finding: the Fe2+ band near 720 nm is not dichroic in low-iron elbaite (o : e about 1.2), the 3 : 1 of the earlier entry belongs to the Fe2+-Fe3+ pairs of dravite and schorl; elbaite's polarization is the UV / blue edge (o about 2 times e) |
| Temperature | Room |
| Thickness as used | Recorded at 3.645 mm; used at 0.34 times the crystal's per-mm absorption over the 9.905 mm face-up path, every band including the UV edge |
| Ordinate convention | Decadic absorbance; alpha = 2.3026 (A - baseline) / path |
| Baseline / reflection treatment | Constant A 0.3966 (o) and 0.4457 (e) removed (the per-ray minima, 0.109 and 0.122 A per mm; R2 states them per mm) |
| Fit domain | 380-1100 nm, four free-centre energy Gaussians per ray, non-linear least squares weighted 1 over 380-780 nm: o-ray a UV edge (centre on its 300 nm bound, FWHM 12 178 cm^-1), a weak band at 620 nm, the Fe2+ band at 711.4 nm (FWHM 2186 cm^-1) and its near-infrared partner at 1156 nm (rms 0.014 per mm); e-ray the edge (FWHM 8738 cm^-1), a weak band at 525 nm, Fe2+ at 708.9 nm (FWHM 2419 cm^-1) and 1180 nm (rms 0.009). Per-mm peaks o 1.78 / 0.091 / 0.517 / 0.213, e 2.127 / 0.035 / 0.469 / 0.124 |
| Deliberate strength adjustments | One strength for the whole measured spectrum: every band, the UV edge included, at 0.34 times the crystal (owner 2026-10-27: measured values win; a first draft scaled the edge a further 3 times to keep the old yellow-green h 121, reverted the same day). Swatch L* 91.4 C* 20.4 h 148.1, a light bluish green, against the old L* 93 C* 49 h 121; o-ray L* 89.9 C* 27.2 h 142.3, e-ray L* 94.2 C* 9.8 h 174.9; edge o : e at 400 nm 2.75, Fe2+ o : e at 711 nm 1.16. The old 430 nm "Fe2+-Ti4+" band is not a measured feature of any elbaite spectrum and is gone |
| Not known | The titanium and manganese of the stone; the near-infrared bands' centres (at the data end, 1104 nm); the edge's centre (below the data window, 300 nm is a bound) |
| Where the bands live | `spinel_through_tourmaline.rs`, the Tourmaline entry's `AbsorptionTensor::uniaxial` (inline bands; no named constant) |

### Andalusite

| Field | Record |
|---|---|
| Sample id | Caltech (M. N. Taran) color-zoned Fe-Ti andalusite, Brazil, FeO 0.35 wt% and a trace of Ti, light zone, normalised by the contributor to 1.0 cm from 0.79 mm, three rays |
| Source | G. R. Rossman, Caltech Mineral Spectroscopy Server, Andalusite page; CSVs `variant_andalusite_fe_ti_brazil_light_zone_{alpha,beta,gamma}.csv` (the dark zone, about 4 times stronger and saturated at 460-540 nm, is not used); R2 section 5 |
| Orientation / rays | alpha = E parallel c (`o_ray`), beta = E parallel b (`beta_ray`), gamma = E parallel a (`e_ray`), Caltech's labelling, which is andalusite's optical orientation X = c, Y = b, Z = a (optic plane (010)); the repo's earlier note (beta parallel a, gamma parallel b) is not supported. `biaxial(alpha, beta, gamma)` slots are the n_alpha / n_beta / n_gamma directions and `c_axis` is the n_gamma direction, so the convention is unchanged and the DATA was moved: the old entry's strongest band sat on beta, the measurements put the Fe2+-Ti4+ color (486 nm, 2.3 per mm) on alpha, with beta and gamma nearly colorless (0.08 and 0.04 per mm at 486 nm) |
| Temperature | Room |
| Thickness as used | Normalised to 1 cm in the file; used at 0.10 times the light zone's per-mm absorption over the 9.905 mm face-up path |
| Ordinate convention | Decadic absorbance per cm; alpha = 2.3026 (A - baseline) / 10 per mm |
| Baseline / reflection treatment | Constant A 0.19 / 0.08 / 0.13 per cm removed (the 800-1100 nm minima: 0.19, 0.08, 0.13; R2's 0.019 / 0.008 / 0.013 A per mm) |
| Fit domain | 380-900 nm, free-centre energy Gaussians, weights 1 over 380-780 nm: alpha five bands (UV edge 308 nm, 430 nm, the 497 nm Fe2+-Ti4+ band FWHM 4718 cm^-1, 620 and 700 nm; rms 0.017 per mm); beta three (UV edge on its 300 nm bound, 380 nm, a faint broad one; rms 0.011); gamma three (rms 0.006) |
| Deliberate strength adjustments | 0.10 times the light zone (alpha 2.3 per mm at 486 nm would be opaque over 9.9 mm; R2's "0.2-0.3" is too dark for a swatch). The measured colors DISAGREE with the old swatch and the old one is the wrong one: alpha L* 63 C* 39 h 41 (brownish red), beta L* 98 C* 7 h 108, gamma L* 99 C* 3 h 111 (pale yellow), unpolarised L* 89 C* 9 h 60 against the old L* 82 C* 22 h 356 (a pink, the mean of a red-brown, a yellow-green and a violet ray; dE2000 17.3). The (alpha + beta) / 2 view is L* 84 C* 14 h 53. The three-ray mean has no green: the greenish, olive and brown stones owe their color to Fe3+, Mn3+ (viridine) and V3+ / Cr3+ |
| Not known | One crystal; the UV edge centres (below the data window); the dark zone's band top; other chromophores |
| Where the bands live | `andalusite_through_glass.rs`, the Andalusite entry's `AbsorptionTensor::biaxial` (inline bands; no named constant) |

### Colorless stones on anisotropic hosts

Quartz (rock crystal), Synthetic Moissanite, Labradorite and Moonstone have no visible-range chromophore. They keep the empty isotropic tensor (`AbsorptionTensor::isotropic(vec![])`): with no absorption in any direction there is no pleochroism to keep, so the per-axis rule of 2026-10-27 applies to colored stones only (lead decision 2026-10-27; a first draft stored them as empty pleochroic tensors, which changed their render path for nothing). Nothing is fitted. Synthetic Moissanite's 425 nm absorption edge (Nassau et al., G&G 33(4), 260 (1997)) has no measured coefficient and belongs to a color variant. Entries: `spinel_through_tourmaline.rs` (Quartz), `tanzanite_through_cubic_zirconia.rs` (Synthetic Moissanite), `feldspars_and_precious_opal.rs` (Labradorite, Moonstone).

### Pyrope (comment fix)

No fit. The entry's comment labelled the 505 nm band Cr3+; since 2026-10-27 it says what R3 found: the 504 / 521 nm bands are Fe2+ (GIA G&G Winter 2015; Caltech GRR 83 and GRR 131), Cr3+ gives 410 and 570 nm. The numbers are unchanged (`garnets_pyrope_through_spessartine.rs`).

## No record yet

Presets whose entry comment holds no fit to a recorded sample:

- **Alexandrite** (`zircon_through_topaz.rs`): band positions are cited (Farrell & Newnham 1965, polarised absorption at 77 K) and the amplitudes are figure-read and scaled by hand ("tuned"), not fitted to a sample spectrum.
- **Pyrope Garnet** (`garnets_pyrope_through_spessartine.rs`): widths and peaks are tuned for a deep red, band centres are cited (see the comment fix above).
