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
| Thickness as used | A density of 12.5 ppma*cm of the pair, not a stone thickness (10 ppma*cm until 2026-11-03, now the Medium Blue variant) |
| Ordinate convention | Not stated |
| Baseline / reflection treatment | Not stated |
| Fit domain | Not stated for this entry (bounded 3-band energy-Gaussian fits) |
| Deliberate strength adjustments | One factor, 1.25 on every peak (owner decision 2026-11-03: the royal blue a fine sapphire is traded as is the default blue). Before it the comment noted that a factor 1.2 would give L* 51 instead of L* 57 |
| Not known | The chemistry of any particular stone; the finished-stone color (the comment gives only the computed swatch, 7 mm, D65: L* 50, C* 44, h 285; face-up o-ray L* 39, C* 67, h 295; at 10 ppma*cm it was L* 57, C* 40, h 283) |
| Where the bands live | `diamond_through_emerald.rs`, the Sapphire entry's `AbsorptionTensor::uniaxial` (inline bands; no named constant) |

## Ruby

Since 2026-11-03 (owner: "make the ruby follow more the clean photo").

| Field | Record |
|---|---|
| Sample id | The GIA corundum chromophore cross sections, Cr3+ and Fe3+ (synthetic Czochralski crystals, SIMS / LA-ICP-MS analysed) |
| Source | Dubinsky, Stone-Sundberg & Emmett, Gems & Gemology 56(1) 2-28 (2020) ([S1]), supplementary data; CSV `gia_dubinsky2020_corundum_six_chromophores_sigma_1nm.csv` (Napierian sigma, 1 nm); 1 ppma = 1.178e17 cm^-3 [S1] |
| Orientation / rays | o-ray (E perpendicular to c, the `perp` columns) and e-ray (E parallel to c, `par`), seven energy Gaussians each |
| Temperature | Room |
| Thickness as used | Cr 400 and Fe 1000 ppma*cm over the face-up path (one model unit at 7 mm); about 0.10 wt% Cr and 0.28 wt% Fe, a Mozambique-type (amphibolite-hosted) ruby |
| Ordinate convention | Napierian absorption coefficient, alpha = sigma N; the band peaks are absorbance per model unit |
| Baseline / reflection treatment | As published (reflection-corrected cross sections); negative noise-floor values clipped to 0 |
| Fit domain | 380-780 nm, least squares on transmittance plus a log-absorbance term; rms of T 1.8 % (o) / 0.6 % (e); dE2000 of the fitted swatch against the cross-section spectrum below 0.1. The 325 nm (o) and 370 nm (e) bands are ultraviolet edge stand-ins |
| Deliberate strength adjustments | The two densities, chosen from renders of the Pentalume Sapphire design against a clean faceted-ruby photo on white (WEZ "Ruby cushion", L* 39 C* 52 h 14). Light Tent: Cr 800 alone (the former default) L* 30 C* 45 h 358; Cr 400 / Fe 750 L* 33 h 8; Cr 500 / Fe 1000 L* 30 h 11; chosen Cr 400 / Fe 1000 L* 32 C* 41 h 11 (Studio Softbox h 13). The measured Thai ruby GRR 1843 at its own strength renders h 9 but L* 27 |
| Swatch (7 mm, D65) | o L* 36 C* 58 h 5; e L* 60 C* 57 h 37; unpolarised L* 46 C* 53 h 19 (two paths: h 26) |
| Not known | The photo stone's origin and chemistry; the camera's rendering. The daylight R-line glow was tested (quantum yield 0.9): +3 L*, +2 deg of hue, so it does not explain the gap (and it is physics-color only) |
| Where the bands live | `diamond_through_emerald.rs`, the Ruby entry's `AbsorptionTensor::uniaxial` |

Replaced: the GIA sample 1110 Cr3+ cross sections alone at 800 ppma*cm (owner decision 2026-10-19), three bands per ray; swatch L* 41 C* 67 h 356, a magenta-pink face-up (Light Tent h 358). Lighter Cr alone opens the blue window and turns the stone pinker, so it cannot follow the photo; Fe3+ closes that window.

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
| Deliberate strength adjustments | Strength is tuned: the measured net 2.73 over 0.997 mm gives A = 19 (decadic) along c over 7 mm, opaque, and a dark-blue zone is not a gem; no concentration is published. The e-ray peak was 2.0 model units (the swatch depth of the earlier entry) and the o-ray peak followed the measured ratio, 2.0 x 0.053, written 0.11. 2026-11-03 (owner: "a bit more saturated") both x 2, 4.0 and 0.22, chosen from renders of the Pentalume Sapphire design (Light Tent C* 17 at x 1, 22 at x 1.5, 27 at x 2, 33 at x 3; chosen L* 61 C* 27 h 274); swatch e-ray L* 53 C* 70 h 278, o-ray L* 93 C* 8. The c axis lies in the table (2026-11-03). Only the SHAPE is fitted |
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
| Deliberate strength adjustments | SUPERSEDED 2026-11-03 (the dealer-photo round): the measured JQ-13 spectrum itself (the Pink variant's four energy Gaussians) at 4 x that variant, swatch L* 41 C* 67 h 18, Light Tent about h 18 against eleven red spinel photos at 27. Before: 540 nm band: centre cited (measured 538 nm), width (sigma 55 nm) and peak (2.4) stay tuned; the measured FWHM is 77 nm (sigma 33 nm), narrower, and narrowing it changes the look, a separate owner decision. 395 nm band added: centre 395 nm (fit 398 nm, raw maximum 391 nm, +-10 nm), sigma 27 nm (FWHM 0.48 eV, about 60-65 nm), peak 1.6 x 2.4 = 3.84. The peak ratio is 1.62 raw (natural) and 1.61 (synthetic); the baseline-subtracted ratio is 2.1 and the Gaussian-amplitude ratio 1.37, so the range is 1.4-2.1 |
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

Superseded 2026-11-03 (0.5 x the Greenish Yellow tensor plus 0.15 x the Alexandrite Cr3+; see the dealer-photo round below).

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

Superseded 2026-11-03: the default is now the electron-irradiated GRR 825c (see the dealer-photo round below); this record describes the GRR 1632 fit it replaced.

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

Strength superseded 2026-11-03 (3.5 x the crystal plus 5.5 x the Yellow variant; see the dealer-photo round below).

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

The entry's comment labelled the 505 nm band Cr3+; since 2026-10-27 it says what R3 found: the 504 / 521 nm bands are Fe2+ (GIA G&G Winter 2015; Caltech GRR 83 and GRR 131), Cr3+ gives 410 and 570 nm. Superseded by the fit of 2026-11-03 below.

## Pyrope (2026-11-03)

| Field | Record |
|---|---|
| Sample id | Caltech GRR 1055 pyrope, Moses Rock, Utah (Ca0.33 Mn0.02 Mg2.13 Fe0.54 Al1.95 Cr0.07) |
| Source | Caltech Mineral Spectroscopy Server; CSV `variant_pyrope_chrome_iso.csv` |
| Orientation / rays | Isotropic (cubic) |
| Temperature | Room |
| Thickness as used | Recorded at 0.840 mm; used at k = 0.22 of the sample's chromophores per mm over the 9.905 mm face-up path of a 7 mm stone |
| Ordinate convention | As fit record F3-02 (docs/color_variants/fit_records/garnet_peridot_zircon_benitoite_tanzanite.md) |
| Baseline / reflection treatment | As fit record F3-02 |
| Fit domain | The Chrome Pyrope variant's energy-Gaussian fit (F3-02, per mm at k = 0.17), every peak x 0.22 / 0.17 x 9.905 to model units |
| Deliberate strength adjustments | k = 0.22, chosen from renders of the Pentalume Sapphire design (Light Tent: k 0.17 L* 29 C* 30 h 12, k 0.3 L* 20 C* 22 h 12) against a faceted pyrope photo on white (L* 27, h 29): a deep Bohemian red, a little deeper than the Chrome Pyrope variant |
| Not known | A pyrope-rich stone without Cr is nearly colorless; the default stands for the usual Cr- and Fe-bearing red pyrope |
| Where the bands live | `garnets_pyrope_through_spessartine.rs`, function `pyrope_absorption` |

Replaced: two tuned wavelength bands (505 and 570 nm) with no blue absorption, which rendered every pyrope a hot magenta (Light Tent L* 42 C* 60 h 323).

## Almandine (2026-11-03)

| Field | Record |
|---|---|
| Sample id | Caltech GRR 1776 almandine ([Fe2.5 Mg0.3 Mn0.2 Ca0.1]Al2.0(SiO4)3) |
| Source | Caltech Mineral Spectroscopy Server; CSV `variant_almandine_purplish_iso.csv` |
| Orientation / rays | Isotropic (cubic) |
| Temperature | Room |
| Thickness as used | Recorded at 0.213 mm (merged file); used at k = 0.35 of the sample's Fe per mm over the 9.905 mm face-up path of a 7 mm stone |
| Ordinate convention | As fit record F3-03 |
| Baseline / reflection treatment | As fit record F3-03 |
| Fit domain | The Purplish variant's energy-Gaussian fit (F3-03, per mm at k = 0.3), every peak x 0.35 / 0.3 x 9.905 to model units |
| Deliberate strength adjustments | k = 0.35, chosen from renders of the Pentalume Sapphire design (Light Tent: k 0.3 L* 25 C* 30 h 13, k 0.5 L* 18 C* 14 h 13) against a faceted almandine photo on white (L* 24, C* 28, h 25): a deep red a touch deeper than the Purplish variant |
| Not known | The finished-stone color of the sample itself (k = 1 renders nearly black at 7 mm) |
| Where the bands live | `garnets_pyrope_through_spessartine.rs`, function `almandine_absorption` |

Replaced: three tuned wavelength bands (505, 520, 573 nm) with no blue absorption, which rendered every almandine a hot magenta (Light Tent L* 51 C* 68 h 329).

## Titanite (Sphene) (2026-11-03)

| Field | Record |
|---|---|
| Sample id | Caltech titanite Titanite1478, yellow-green, cleavage face of a 4 cm crystal, (Lac Alaotra?) Madagascar |
| Source | G. R. Rossman, Caltech Mineral Spectroscopy Server, `http://minerals.gps.caltech.edu/FILES/Visible/titanite/Titanite1478.gif` (a figure; no numeric file); digitised 2026-11-03 to CSV `caltech_titanite_1478_digitized_3p91mm.csv` (10 nm steps, about +-0.005 A) |
| Orientation / rays | E parallel b taken as Y (n_beta), used as measured. E parallel c mixes Z and X (Z ^ c = 51 deg in Dana's setting: about 0.4 Z + 0.6 X); gamma (Z) = 2 x E parallel c and alpha (X) = 0.3 x E parallel c are ESTIMATED (X nearly colorless, Z red-yellow to orange: Dana 1892 after Rosenbusch; Handbook of Mineralogy). 0.4 x 2 + 0.6 x 0.3 = 0.98 rebuilds E parallel c |
| Temperature | Room (not stated) |
| Thickness as used | Recorded at 3.91 mm; used at 0.28 times the sample's absorption per mm over the 9.905 mm face-up path (0.7 times until the dealer-photo round, 2026-11-03) |
| Ordinate convention | Decadic absorbance read off the figure; alpha per mm = ln 10 (A - 0.125) / 3.91 |
| Baseline / reflection treatment | The plot's flat level near 900 nm, 0.125 in both rays (reflection loss and scatter), removed |
| Fit domain | 430 (E parallel b) / 460 (E parallel c) to 780 nm; the values above A = 1 are off the plot. Three energy Gaussians per ray: an ultraviolet edge (323.6 nm / 400 nm, the latter on its bound), the 609 nm band of E parallel b, and weak bands at 484-567 nm. Relative rms 3.4 % / 4.1 % |
| Deliberate strength adjustments | 0.7 times the sample at first, chosen from renders of the Pentalume Sapphire design (Light Tent: 0.5 times L* 65 C* 37 h 96-99, 1.0 times L* 52 C* 33-36 h 91-95): a medium yellow-green. The same day 0.28 times (the dealer photos, median hue 108; render h 101, see the dealer-photo round) |
| Indices | Busz 1887 (St. Gotthard, no iron in the analysis), via Dana 1892 p. 714, n_alpha / n_beta / n_gamma at 670.8, 589.3 and 535.0 nm; one-pole Sellmeier curves with one shared pole (0.1524 um), largest residual 0.0013. D line: 1.88795 / 1.89415 / 2.01057, birefringence 0.1226, 2V_z 27 deg; B-G 0.057 (alpha, beta), 0.064 (gamma), extrapolated below 535 nm |
| Not known | Pure X and Z spectra; the cause of the bands (Fe3+, Ti3+, Fe-Ti charge transfer and REE are proposed); the iron and REE of the sample. The gem tables' B-G 0.051 names no ray |
| Where the bands live | `titanite.rs`, `built_in_material_titanite` |

## Alexandrite (2026-11-03)

| Field | Record |
|---|---|
| Sample id | Caltech GRR 874 alexandrite (Lake Manyara, Tanzania, 0.717 mm) for Cr3+; Caltech GRR 171 chrysoberyl (the Chrysoberyl (Yellow) spectra) for Fe3+ |
| Source | Caltech Mineral Spectroscopy Server, G. R. Rossman, CC BY-NC 4.0; CSV `caltech_chrysoberyl_grr874_beta_ch874a_0p717mm.csv`, `variant_alexandrite_grr874_gamma_0p717mm.csv`, `variant_chrysoberyl_yellow_grr171_*.csv` |
| Orientation / rays | beta = E parallel a (`beta_ray`), gamma = E parallel b (`e_ray`, along `c_axis`), alpha = E parallel c (`o_ray`). Beta and gamma measured; alpha ESTIMATED from the gamma ray (split at 490 nm, 4T2 half moved 597 to 568 nm at 0.6 x, 4T1 half 415 to 418 nm at 1.0 x, Farrell & Newnham Fig. 4 ratios) |
| Temperature | Room |
| Thickness as used | Per mm of the samples, Cr 0.15 x GRR 874 and Fe3+ 0.10 x GRR 171, over the 9.905 mm face-up path of a 7 mm stone (model units: per mm x 9.905) |
| Ordinate convention, baseline | As the fit record: constants 0.1108 / 0.1552 removed, the Cr3+ R line (672-690 nm) interpolated |
| Fit domain | Six energy Gaussians per ray, 380-780 nm (the fit record's Alexandrite Material Default section) |
| Deliberate strength adjustments | The two factors, chosen for a green by daylight and a red to purplish red under 3200 K (three-ray mean L* 54 C* 17 h 132 / L* 53 C* 12 h 20). Renders of the Pentalume Sapphire design: Light Tent h 164, Incandescent h 333 C* 15 |
| Not known | The alpha ray; GRR 874's own Cr content (a Cr-rich stone, about 1 wt% Cr2O3 by one unverified cross section) |
| Where the bands live | `zircon_through_topaz.rs`, the Alexandrite entry |

Replaced: the tuned two-band sets after Farrell & Newnham (band positions cited, widths and amplitudes set by hand), whose stone went from a yellowish green (Light Tent h 129) to an orange (Incandescent h 45) and never to red. The fit was the color variant Strong Change, removed when it became the default.

## Dealer-photo round (2026-11-03)

Owner: "I want them to match the pictures of actual gemstones." Reference: about 24 product photos per gem from one dealer (GemSelect; standardised light-grey studio photos), saved in the lead's private comparison folder and never shipped, measured with the render statistics (stone mask on a plane-fitted background, several stones per photo counted) plus the hue of the colored pixels only (C* > 10), which neutral shadows and white glints cannot pull. The source is not biased in hue for the stones checked before: Sapphire renders 2 deg and Ruby 5 deg from its medians. Renders: Pentalume Sapphire design, Light Tent, the colored-pixel hue. These rows supersede the strength rows of the records above for the same entries.

| Entry | Photos (n, median hue) | Before | Change | After |
|---|---|---|---|---|
| Zircon | 18, colorless (C* 0.7) | lilac, C* 11 | the U4+ lines x 0.05 (a low-uranium zircon) | swatch C* 1 |
| Grossular Garnet (Tsavorite) | 22, h 139 | h 152 | x 0.5 (0.25 x the GRR 229 sample's V) | h 143 |
| Andradite Garnet (Demantoid) | 19, h 117 | h 150 | 0.15 x GIA 2952 plus the Topazolite variant (Fe3+ andradite GRR 48), refitted | h 120 |
| Andalusite | 23, h 71 (and 6 Commons photos, h 64) | h 51 | 0.65 x the Fe-Ti crystal plus 0.35 x the Greener variant, refitted | h 75 |
| Aquamarine | 20, h 241 | h 252 | 0.6 x GRR 1488 plus 0.4 x the Greenish variant, refitted | h 240 |
| Morganite | 16, h 24 | h 348 | 0.35 x the pink set plus 0.65 x the Peach variant | h 20 |
| Almandine Garnet | 21, h 29 | h 16 | plus the Spessartine built-in's Mn2+ at 0.3, refitted (dE2000 4.9) | h 26 |
| Pyrope Garnet | 19, h 26 | h 13 | plus the Spessartine built-in's Mn2+ at 0.6, refitted (dE2000 2.5) | h 19 |
| Tourmaline | 14, h 121, L* 52 | h 146, L* 78 | 3.5 x the elbaite plus 5.5 x the Yellow variant (GRR 1935 yellow); the first step 2 x + 1 x gave h 127, L* 71 | h 125, L* 62 |
| Chrysoberyl (Yellow) | 23, h 109 | h 76 | 0.5 x the Greenish Yellow variant (0.3 x GRR 171) plus 0.15 x the Alexandrite Cr3+ (traces of Cr in a greenish-yellow chrysoberyl), refitted; the Greenish Yellow tensor alone gave h 93, a pale cream, and more Cr greys it (0.6 x + 0.25 x: h 113, L* 57, a khaki); the 0.7 x default is the Golden variant | h 111, L* 64 (photos L* 64) |
| Titanite (Sphene) | 24, h 108 | h 94 | x 0.4 (0.28 x the sample); no split of the estimated alpha / gamma rays and no strength goes past h 102 with the one measured crystal | h 101 |
| Topaz | 22, h 247 | h 231 | the electron-irradiated GRR 825c (Swiss / sky blue type) in place of the neutron-irradiated GRR 1632, without its brown centre (removed by the heating of commercial blue topaz), gamma = beta (estimated), 2 x the crystal | h 245 |
| Tourmaline (second step) | 14, h 121, L* 52, C* 28.5 | h 125, L* 62, C* 66 (a lime, 2.3 x the photos' chroma where the matched materials sit near 1.4 x) | the elbaite x 1.45 and the yellow part x 0.9 of the first step, plus 0.7 x the Pink variant's Mn3+, which greys the green window; more Pink or Rubellite alone pulls the hue to yellow (Rubellite 0.3 x: h 99), Indicolite pulls it back but adds chroma; refitted to eight bands a ray | h 122, L* 41, C* 50 (swatch L* 52 C* 46 h 124) |
| Color-Change Garnet (Pyrope-Spessartine) | 24 stones (the common Tanzanian type), day light h 86, L* 59 (4 plain-background day / lamp pairs; 40 day-light views: h 72); under a lamp 16 deg more orange (h 70) | h 299, a violet (hand-tuned bands: a strong 573 nm band between a blue-green and a red window) | the measured Tunduru spectrum (Caltech GRR 2225) with its 581 nm V3+ / Cr3+ band x 0.25, the whole x 1.2 of the variant's strength; at x 1.0 the band gives the rare blue-to-purple type (day h 202, lamp h 302) | day h 86, L* 60, C* 36; lamp h 70 (shift -16) |

Not changed: Peridot (photos 115, render 113), Emerald (159, 164), Spessartine (51, 57), Citrine (87, 91), Sapphire (288, 290), Ruby (7, 12), Tanzanite (289, 298). Amethyst renders h 333 against 316 at every strength and orientation and with both variants; violet-rich stones photograph bluer than the eye sees them (a camera's red channel lacks the eye's second red-cone peak near 445 nm; Tanzanite +9, Amethyst +17, Sapphire +2), so the gap was left (closed on 2026-11-04 by moving the main band: see Remaining dealer-photo gaps). Spinel: eleven red spinels (h 27, quartiles -4 / +11) against the tuned render's 350: the measured JQ-13 spectrum at 4 x the Pink variant (renders h 16 at 3 x, h 20 at 5 x) replaced the tuned bands.

Ametrine (zoning check, same day): the built-in's citrine half went from 14 to 11 ppma iron (`AMETRINE_CITRINE_FE_PPMA`); the amethyst half stays at the Anahi iron. Reference: sixteen dealer photos of ametrine, hue shares of the colored pixels (C* > 8) as purple (h 280-15) / peach blend (h 15-60) / yellow (h 60-120): median 30 / 28 / 34 %, purple quartiles 7 / 30 / 50 %. Renders (Pentalume Sapphire, Light Tent face-up): 14 ppma 21 / 56 / 23, 8 ppma 39 / 42 / 19, 11 ppma 25 / 54 / 21. The large blend share is the half-space geometry (most light crosses both halves), not the iron. The typical Split and Sectors patterns of Citrine and Ametrine, and the Color Pattern window's Ametrine preset, took the seeds amethyst L* 46 C* 45 h 305 and citrine L* 84 C* 58 h 85 (they were L* 48 C* 42 and L* 82 C* 72 h 88): the Split went from 20 / 14 / 65 to 31 / 34 / 33, the Sectors from 25 / 10 / 64 to 33 / 25 / 41.

## Effect-stone bodies (2026-11-04)

Owner: "Fix the white opal and labradorite bodies." Reference: the 23 usable GemSelect labradorite photos and four web photos of white opals (Australian and Ethiopian, private comparison folder), measured for the body (the stone's pixels with C* at most 18 and L* below 92) and the flashes (C* above 18, by hue class). Renders: Pentalume Sapphire design, softbox face-up, effects on, the app's haze asymmetry 0.4.

| Entry | Photos | Before | Change | After |
|---|---|---|---|---|
| Precious Opal (White) | a milky, translucent white body with a slight cream (b* +3 to +14); flashes green and yellow-orange | a clear body (haze 0.15, barely visible): glass with flashes | haze 1.2 (switched on with the material) and one weak band at 400 nm (0.15 per model unit) for the cream; 0.8 to 2.5 all read milky, the cream band 0.08 to 0.2 gave b* +2 to +3.4 | a milky cream-white body, b* +3, the play-of-color unchanged |
| Labradorite | an olive-grey translucent body, median L* 52, a* -1.9, b* +7.4 (body L* 10-90 % about 37-76); flashes mostly blue, some green and gold, almost no violet or red | a clear, colorless body (L* 10-90 % 9-89); a rainbow schiller (470-740 nm) face up | an olive-grey body of three neutral bands (0.4), a 400 nm yellow edge (0.8) and a 950 nm red tail (0.3), the same in all three directions; haze 0.25 (switched on with the material; 0.8 and more scattered the light before the lamellae and the schiller was gone); the lamellae parallel to the table by default (n_gamma = cos 79 Y + sin 79 Z, n_alpha = X: with n_gamma = Y they stood nearly upright, and face up the flash came only from light that had crossed the clear stone many times, which the body absorbs); the default flash range 400-560 nm (a domain's wavelength is Gaussian about the middle of the range, so 470-740 and 430-600 centred on green and flashed green, orange and red face up; larger domains did not help) | body L* 49 (32-75), olive grey; flashes blue 26 %, green 25 %, the rest the yellowish body edge and a little violet |

## Remaining dealer-photo gaps (2026-11-04)

Owner: "Fix the remaining gaps." Same photos and measure as the dealer-photo round (Light Tent face-up, the hue of the colored pixels).

| Entry | Photos | Before | Change | After |
|---|---|---|---|---|
| Amethyst | 16+, h 316 | h 333 at every strength (the round above left it as a camera bias) | the main Fe4+ band from the Anahi slab's 537.1 to 549 nm (the literature's 540-550 nm), widths and strengths unchanged; 545 nm gave h 322, 555 nm h 309, a red band at 680 nm h 315-324 at lower chroma. The Ametrine keeps the Anahi recipe: its stones come from that mine and its photos' purple reads h 337 (the half-match test allows 25 deg for the amethyst half) | h about 316 (545: 322, 555: 309); swatch L* 53 C* 36 h 315 |
| Titanite (Sphene) | 24, h 108 | h 101 | beta's 609 nm band (the yellow-green of E parallel b) x 3, an estimate (gem sphene is greener than the one measured crystal); x 2 gave h 104, gamma's ultraviolet edge x 0.6-0.7 changed nothing | h 108, chroma unchanged; swatch L* 88 C* 35 h 111 |
| Labradorite (check) | 17 photos with a blue flash: flash hue 252, chroma 23 | | (the body round above) | Light Tent flash hue 261, chroma 22; softbox 272 |
| Tourmaline Watermelon (check) | slices: pink L* 29-71, C* 16-30, h 348-21; green L* 13-60, C* 12-35, h 93-118. A search for faceted watermelons cut across c found none: faceted pink-and-green stones are cut along c (a pink stripe between green edges, or pink at one end) | the faceted comparison design blended pink and green to orange where light crosses both zones | none to the material: compared on an 18 mm, 4 mm thick slice model instead | pink h 349-350, green h 113-118, the geometry as the slices; lighter (pink L* 67-74, green L* 74-82), since the zone colors are solved for a faceted stone's face-up path and slices are cut from darker material |

## Saturation round (2026-11-04)

Owner, passing on a review: "The render seems to err toward saturated", and "I want photorealism". The earlier rounds matched hue; this one matches saturation.

**Why not the display:** the CLI's linear XYZ dump (`PLEOCHROMA_LINEAR_XYZ_OUT`) fed a Python twin of the encode, which reproduces the PNGs exactly. Whiten, per-channel ACES, a blend and Mantiuk's correction all lower the median chroma, but none removes the spread between materials: s 0.55 needs a 7 deg hue shift. A lens veiling glare of 1-8 % of the frame's mean lowers the median by 0.18 at most and leaves the spread (log sd 0.34) unchanged. So the excess is per material.

**Measure:** exposure-free, since the dealer photos are darker than the Light Tent renders (background L* 74 against 91; colorless diamonds on a light ground L* 28-43):
- the CIELUV saturation s_uv = 13 |u'v' - D65| of the mean colored pixel (C* > 10), which a scale by black leaves unchanged;
- only faceted, transparent stones count (90th-percentile Sobel gradient inside the stone at least 0.30; this drops cabochons and milky stones);
- only photos with a single stone count.

**Renders:** Pentalume Sapphire, Light Tent, each material at x 0.25 / 0.5 / 1 / 2 of its absorption.

**Result:** at x 1 the median render / photo ratio was 1.28 (quartiles 1.01-1.64). The Commons photos of the shareable deck read richer than the dealer's commercial stones for Topaz, Tanzanite and Ruby, and as pale for Aquamarine.

**Rule:**
- where both photo sources agree, the factor whose render matches the dealer photos;
- where the Commons photos are richer, half the gap in log terms;
- for small-stone species, rounded toward 1, since a 4-6 mm stone has a shorter path than the 7 mm reference.

| Entry | Photos s_uv (dealer; Commons) | Render s_uv at x 1 | Factor | After (rendered with the built CLI: s_uv, hue, render / dealer ratio; replica swatch) |
|---|---|---|---|---|
| Aquamarine | 0.30; 0.15, 0.31 | 0.57 (h 240) | x 0.3 | 0.32, h 234 (ratio 1.06); swatch L* 92 C* 10 h 233 |
| Morganite | 0.28; - | 0.45 | x 0.6 | 0.34, h 23 (1.23); swatch L* 89 C* 12 h 18 |
| Topaz | 0.48 (sky blue); 0.73-0.95 (Swiss blue) | 0.86 | x 0.7 | 0.64, h 242 (1.35); swatch L* 85 C* 29 h 238 |
| Tanzanite | 0.79; 0.84, 1.76 | 1.57 (L* 30) | x 0.7 | 1.15, h 296, L* 40 (1.45); swatch L* 56 C* 40 h 290 |
| Peridot | 0.79; 0.54-0.94 | 0.96 | x 0.6 | 0.79, h 114, L* 68 (1.00); swatch L* 84 C* 48 h 113 |
| Grossular Garnet (Tsavorite) | 0.88 (small stones); - | 1.11 | x 0.7 (x 0.63 matches) | 0.92, h 140 (1.05); swatch L* 80 C* 62 h 137 |
| Andradite Garnet (Demantoid) | 0.62 (small stones); 0.96 | 0.98 | x 0.35 | 0.79, h 116 (1.28); swatch L* 85 C* 59 h 115 |
| Pyrope Garnet | 1.12 (L* 41, h 29, brownish); 0.81, 1.76 | 1.94 (L* 22) | x 0.45 | 1.36, h 16, L* 34 (1.21); swatch L* 46 C* 36 h 16 |
| Spinel | 0.83 (L* 43, h 27, brownish); - | 2.13 (L* 32) | x 0.45 | 1.40, h 12, L* 48 (1.68); swatch L* 61 C* 50 h 7 |
| Rutile (owner: "way less yellow in the photos"; the measure is the whole stone, as lab rutile has few body-colored pixels between its fire) | lab-rutile dealer 0.08-0.21 (median 0.14, C* 3-10); Commons synthetic round 0.14 | 0.53 (C* 33, a lemon yellow) | x 0.08 (the tuned 350 nm band, peak 8.0 -> 0.64) | about 0.14 (x 0.1: 0.17, x 0.05: 0.10), L* 86 unchanged |
| Citrine | 0.79; 0.95-1.13 | 0.57 (a pale lemon) | x 2 | 0.80, h 83 (1.01); swatch L* 85 C* 77 h 87 |

**Overall** (all twenty dealer-photo materials, rendered again): the median render / photo saturation went from 1.28 to 1.13 and the spread between materials (log sd) from 0.33 to 0.20; the unchanged entries render identically.

**Left alone:**
- Amethyst (render / photo 0.99), Almandine (1.02), Spessartine (0.96), Andalusite (1.10) and Emerald (1.17, the Commons photos 0.69-1.10) are already close.
- Sapphire (1.29, x 0.71 would match) and Ruby (1.54 against two faceted dealer photos; the Commons rubies are as saturated as the render) keep the owner's strengths of 2026-11-03.
- Tourmaline (1.50) does not fall monotonically with strength, and its photo set mixes colors.
- Chrysoberyl (Yellow) (0.75) and Titanite (0.82) are under-saturated, but a stronger stone only darkens them: the gap is in the spectrum's shape.

**Remaining gap:** Pyrope and Spinel turn pinker as they get lighter (h 16 and 13 against the photos' 29 and 27). Their photos are brownish, a grey mask under the red that the measured Cr3+ spectra lack. A 330 nm edge band moved Spinel by 4 deg only, and a stronger blue-violet part of Pyrope restored the hue (h 29) but raised the saturation to 2.0. A broad band needs room that the 8-band budget does not have.

**Tests:**
- the swatch ranges in `built_in_hues.rs` follow the replica values above;
- the aquamarine orientation test's lightness bound is 5;
- criterion 6 re-pinned from a run: Tanzanite 2.78 (it was 7.27; the lighter stone now agrees with the V3+ recipe), Peridot 0.10, Aquamarine 0.67, Citrine 1.28, seven of eight agree;
- the Tsavorite Mint variant (0.18 x the GRR 229 vanadium) had come to sit at the default's strength (0.175 x); owner: "Make Mint paler so it differs from the default": Mint is now half the default (0.0875 x, L* 89 C* 41 h 133, dE2000 8.9 from the default), the scaled-copy test factor 0.5.

Scripts: `sat_analysis.py`, `sat_glare.py`, `sat_purity.py`, `sat_matched.py`, `sat_swatches.py` (private workspace).

## No record yet

None left: Alexandrite, the last preset on this list, has had its record since 2026-11-03.
