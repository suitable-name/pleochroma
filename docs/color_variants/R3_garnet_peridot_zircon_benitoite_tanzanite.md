# Color variants research R3: garnets, peridot, zircon, benitoite, tanzanite (2026-10-27)

Research lane R3 of the color variants campaign (plan `PLAN_color_variants_2026-10-27.md`, section 6). Scope: the variants listed for Pyrope, Almandine, Spessartine, Grossular (Tsavorite), Andradite (Demantoid), Color-Change Garnet, Peridot, Zircon, Benitoite and Tanzanite. No Rust was written or run. The only code run was small Python helpers in the session scratchpad (CSV conversion, evaluation of a hand-written band list against its spectrum, color arithmetic). Physics solving is out of scope; every band below was read from a published spectrum and written by hand.

## 0. Method, conventions and how far to trust the numbers

**Data.** Every numeric spectrum is a Caltech Mineral Spectroscopy Server (G. R. Rossman) file, downloaded 2026-10-27 over plain `http://` (the server refuses `https` with `ECONNREFUSED`; `curl` over `http` works). Each is stored unchanged except sorting, duplicate removal and, on two large files, a stated wavelength crop, as `crates/pleochroma/data/reference_spectra/variant_<material>_<variant>_<pol>.csv`, in the header style of `caltech_aquamarine_grr1488_eperpc_5p531mm.csv`. `<pol>` is `iso` (cubic), `eperpc` / `eparc` (E perpendicular / parallel to c = o-ray / e-ray) or `alpha` / `beta` / `gamma`. Spectra that already exist in the repo (Spessartine GRR 43, Tsavorite GRR 229, Demantoid GIA 2952 and GRR 2667, olivine GRR 418, zoisite GRR 2292 / 3045) were not duplicated.

**Band convention.** Every band is an energy Gaussian (`AbsorptionBand::energy(center_nm, fwhm_cm_inv, peak)`); the stored width is sigma = FWHM / 2.35482 (cm^-1). `peak per mm` is the Napierian absorption coefficient at band centre per mm of path, at the strength of the measured sample (baseline removed, stated per variant); multiply by the suggested strength factor `k` for a stone. `peak ModelUnit` = per mm x 8.8. This was checked by reproducing the repo's own calibration swatches: the face-up transmission is `exp(-sum of band peaks)` for the 8.8 mm face-up path. A variant stored in `PerMm` takes the per-mm column; one stored in `ModelUnit` takes the x8.8 column.

**Colors.** All colors are CIELAB (D65 unless stated, 8.8 mm path, 2 degree observer, no Fresnel loss, white = the illuminant), computed here from the CSV spectra ("data") and from the proposed bands ("model") with the Wyman et al. 2013 multi-lobe fit of the CIE 1931 functions and the CIE D65 table. Cross-checks of this pipeline against the repo's own recorded numbers:

- Spessartine GRR 43 raw spectrum: L* 77.0 C* 71.9 h 65.6 (calibration record: L* 77 C* 72 h 66).
- The repo's Spessartine bands as ModelUnit: L* 76.8 C* 71.9 h 66.
- Tsavorite GRR 229 at 0.5 / 0.35 / 0.18 x its vanadium: L* 55.4 / 65.2 / 79.4 (record: 56 / 65 / 78).
- The repo's Peridot bands, mean of alpha, beta, gamma: L* 75.8 C* 63.3 h 111 (record: 76 / 63 / 111).
- Spessartine GRR 72 model: L* 48 h 59 (record: "L* 51 h 62" for GRR 72).

Uniaxial face-up = (2 o + e) / 3 in transmission, biaxial = mean of the three rays, as in the existing entries. These are derived numbers, not measured stone colors; none of the variants has a measured CIELAB.

**Band models.** At most 8 bands per ray, read by hand: baseline subtracted, peak heights from `2.303 (A - baseline) / thickness_mm`, FWHM from half-maximum points, then adjusted by hand against a printed residual (three to six passes). No optimizer was used; the evaluation script only prints `data - model`. The residual (rms per mm, 380-780 nm at 10 nm) and the color difference dE76 (model vs data, D65, face-up) are given for each. The later calibration lane (plan section 7) should refine these, not trust them.

**Blocked or limited sources.** `minerals.gps.caltech.edu` over `https` refused. PubMed Central (PMC7465713, Pluthametwisute 2020) sits behind a captcha and MDPI returned 403, so that paper's numbers are taken from the repo's notes (`docs/chromophores/primary-2026-10/data/tanzanite/NOTES.md`). The Schwarzinger et al. 2025 PDF extracted as words without its digits. Taran et al. 2023 (Am. Min. 108) was not readable as text. No binary was saved in the repo.

**Findings that change the plan** (repeated in the report):

1. **Zircon's Material Default is an isotropic tensor on a uniaxial host** (`zircon_through_topaz.rs`: `AbsorptionTensor::isotropic` with U4+ lines). `host_rank(zircon)` is Uniaxial, so `validate_for` would flag the Material Default itself as `RankMismatch`. All zircon variants below are uniaxial (o and e sets).
2. **Spessartine's Material Default already is the "Mandarin"** (calibration record: GRR 43, near end-member, Fe-poor "Mandarin", L* 77 C* 72 h 66). A second "Mandarin" variant would duplicate it; Reddish Orange (GRR 72) and Pale Golden Orange (scaled) are proposed instead.
3. **A colorless variant cannot be an empty band list** (`VariantProblem::Empty`): Colorless zircon carries one estimated near-zero UV-edge band per ray.
4. **Pyrope's and Almandine's Material Defaults are tuned ModelUnit bands, not fits** (their comments: widths and peaks TUNED, centres cited). The pyrope default labels 505 nm as Cr3+; the 504 / 521 nm bands are Fe2+ (GIA Winter 2015; Caltech GRR 83 / 131), Cr3+ gives 410 and 570 nm.
5. **8 bands cannot reproduce a line spectrum.** Zircon (U4+ lines) keeps the five or six strongest lines plus two or three broad bands; the weaker lines are dropped (stated per ray).

## 1. Summary table

Band counts are per ray (o / e for uniaxial, alpha / beta / gamma for biaxial, one set for cubic). Data quality: **A** = measured polarized spectrum of a documented sample, bands read from it; **B** = measured spectrum but composition unpublished or with a gap (stated); **C** = scaled copy of an existing fitted entry (concentration scaling, estimated strength); **D** = estimated, no spectrum.

| Variant | Cause | Data quality | CSV files (`variant_*`) | bands |
|---|---|---|---|---|
| Pyrope / Rhodolite | Fe2+ (X site, about 33 mol% almandine) | A | `pyrope_rhodolite_iso`, `pyrope_rhodolite83_iso` (cross-check) | 7 |
| Pyrope / Chrome Pyrope (added) | Cr3+ (about 1.2 wt% Cr2O3 in the sample) + Fe2+ | A | `pyrope_chrome_iso` | 7 |
| Almandine / Purplish | Fe2+ (83 mol% almandine) + IVCT | A | `almandine_purplish_iso` | 7 |
| Spessartine / Reddish Orange (replaces Mandarin) | Mn2+ (93 mol% spessartine) | B (file starts at 430 nm) | `spessartine_reddishorange_iso` | 7 |
| Spessartine / Pale Golden Orange | Mn2+ diluted | C (default x 0.4) | none | 8 |
| Grossular (Tsavorite) / Mint | V3+ (about 0.33 wt% V2O3) | C (default x 0.36) | none (GRR 229 exists) | 5 |
| Grossular (Tsavorite) / Hessonite | Fe3+ edge + 440 nm band | B (no analysis) | `grossular_hessonite_iso`, `grossular_hessonite1113_iso` (cross-check) | 4 |
| Andradite (Demantoid) / Topazolite | Fe3+ (99 mol% andradite), no Cr | A | `andradite_topazolite_iso` | 6 |
| Color-Change Garnet / Tunduru | V3+ / Cr3+ band at 580 nm + Mn2+ | B (file starts at 412 nm) | `colorchange_tunduru_iso` | 7 |
| Color-Change Garnet / Weaker Change | as Tunduru, V band x 0.6 | D (estimated) | none | 7 |
| Color-Change Garnet / Stronger Change | as Tunduru, V band x 1.8 | D (estimated) | none | 7 |
| Color-Change Garnet / Malaya (added) | Mn2+ + Fe2+, no change | B | `colorchange_malaya_iso` | 7 |
| Peridot / Deep Green | Fe2+ (Fo about 86 instead of 90) | C (default x 1.4) | none (GRR 418 exists) | 6 / 6 / 6 |
| Zircon / Blue | heat-treated U4+ electron-trap centre | A | `zircon_blue_eperpc`, `zircon_blue_eparc` | 2 / 1 |
| Zircon / Golden Brown | damage continuum + U4+ lines | B (sample is "pale brown") | `zircon_golden_eperpc`, `zircon_golden_eparc` | 8 / 8 |
| Zircon / Red-Brown | alpha-recoil damage continuum | A | `zircon_redbrown_eperpc`, `zircon_redbrown_eparc` | 3 / 3 |
| Zircon / Green | U4+ lines + damage continuum | A | `zircon_green_eperpc`, `zircon_green_eparc` | 8 / 8 |
| Zircon / Colorless | none (UV edge only) | D | none | 1 / 1 |
| Benitoite / Light Blue | Fe-related charge transfer (cause unresolved) | C (default x 0.5; dark-zone spectrum gives the shape only) | `benitoite_lightblue_eperpc`, `benitoite_lightblue_eparc` | 1 / 1 |
| Tanzanite / Unheated | V3+ + Fe2+-Ti4+ IVCT (440 nm band) | A | `tanzanite_unheated_alpha`, `_beta`, `_gamma` | 4 / 5 / 5 |
| Tanzanite / Green Zoisite | V3+ / Cr3+ / Fe-Ti IVCT mix | B | `zoisite_green_alpha`, `_beta`, `_gamma` | 5 / 4 / 4 |

**Dropped or not offered, and why.**

- Pyrope "Red": it is the Material Default.
- Spessartine "Mandarin": it is the default (finding 2).
- Demantoid variants other than Topazolite: the repo already holds the green demantoid (GIA 2952) and the heated Ural stone (GRR 2667). The unheated brown-orange GRR 2666 was downloaded for evaluation only and is not proposed (its brown component is a different chromophore).
- Zircon "Green (metamict)": GRR 1486 is unpolarized, saturated above A = 3 below 440 nm, and its sharp lines are broadened. The crystalline Mogok green (GRR 3106) is offered instead. Heat-induced blue zircon is covered by GRR 1898, which has measured bands.
- Benitoite pink and orange (GRR 303, 456, 1846): figure-only on the server and rare.
- Tanzanite "Violet-Blue heated": it is the Material Default (GRR 2292 blue zone, already fitted).

## 2. Garnets (cubic, isotropic: one band set, o = e)

Sources common to all garnet variants: Caltech garnet index (http://minerals.gps.caltech.edu/FILES/Visible/garnet/Index.html; Cr3+, V3+, Fe3+, Fe2+, Mn2+ and Fe2+-Ti4+ IVCT listed as the color causes); Stockton and Manson, "A Proposed New Classification for Gem-Quality Garnets", Gems & Gemology 21(4), 1985, 205-218 (https://www.gia.edu/gems-gemology/winter-1985-garnet-classification-stockton; abstract only); the repo guide `docs/chromophores/garnet_peridot_zircon_benitoite.md` (Report pp. 14-17; band centres 504 / 527 / 576 nm Fe2+, 409-486 nm Mn2+, 571 / 574 nm Cr3+ / V3+, 440 nm Fe3+). Garnet is cubic, so every ray is the same set; no pleochroism.

### 2.1 Pyrope Garnet

The Material Default is the tuned two-band "deep red" (finding 4); it stays the first entry ("Red (Material Default)").

#### 2.1.1 Rhodolite (Pyrope-Almandine)

1. **Cause.** Fe2+ on the dodecahedral X site (spin-forbidden 504, 527, 576 nm bands, weaker 610 and 683 nm), a rising ultraviolet charge-transfer edge, and a broad Fe2+-Fe3+ IVCT continuum (centre 461 nm, FWHM about 7300 cm^-1 in S1 = Am. Min. 92, 2007, doi:10.2138/am.2007.2163). Caltech calls GRR 83 "rhodolite ... colored pink by Fe2+" and GRR 131 "colored pink by Fe2+". Report p. 15 puts rhodolite at x_Alm 0.30-0.45, x_Prp 0.55-0.70.
2. **Spectrum.** `variant_pyrope_rhodolite_iso.csv` = Caltech GRR 131 (Mg1.82 Fe0.99 Ca0.16 Mn0.04 Al2.00, 2.007 mm, 365-1108 nm), Fe 0.99 of 3 sites = 33 mol% almandine, inside the rhodolite range. Baseline A = 0.031 (flat 800-870 nm). Cross-check `variant_pyrope_rhodolite83_iso.csv` = GRR 83 (Umba, Tanzania, Fe0.78 = 26 mol%, 0.78 mm): alpha(500 nm) per mm per almandine fraction is 1.43 for GRR 83 and 1.48 for GRR 131, so the bands scale linearly with Fe (Report p. 46).
3. **Band model** (isotropic, 7 bands, baseline removed, native strength of GRR 131; rms 0.028 per mm, face-up dE76 4.7):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 330 | 10000 | 4247 | 0.8 | 7.04 | UV / charge-transfer edge (estimated shape: file starts at 365 nm) |
| 2 | energy | 450 | 7000 | 2973 | 0.17 | 1.496 | broad Fe2+-Fe3+ IVCT / continuum (461 nm in S1) |
| 3 | energy | 504 | 1000 | 425 | 0.3 | 2.64 | Fe2+ 504 nm spin-forbidden |
| 4 | energy | 530 | 1300 | 552 | 0.28 | 2.464 | Fe2+ 527-530 nm |
| 5 | energy | 576 | 1500 | 637 | 0.32 | 2.816 | Fe2+ 576 nm (strongest) |
| 6 | energy | 620 | 1400 | 595 | 0.09 | 0.792 | Fe2+ 610 nm (weak) |
| 7 | energy | 700 | 2200 | 934 | 0.1 | 0.88 | Fe2+ 683-700 nm hump |


4. **Target color.** Data, D65, 8.8 mm, k = 1: L* 34.2 C* 43.6 h 358 (dark purplish red, raspberry); k = 0.6: L* 51.7 C* 37.4 h 354; k = 0.3: L* 71 C* 25 h 351 (pink-purple). Illuminant A: h 10 at k = 1 (warmer red). Documented: "pink" to purplish red (Caltech), no pleochroism (cubic).
5. **Strength anchor.** Fe 0.99 apfu (33 mol% almandine) at k = 1 (native). Peak absorption scales with almandine fraction: 1.45 per mm per unit x_Alm at 500 nm. A pale rhodolite is k about 0.3.
6. **Confidence and gaps.** Confidence high for the Fe2+ bands (two samples agree), medium for the UV edge (file starts at 365 nm, edge shape estimated) and the 450 nm continuum. Mn 0.04, Ca 0.16 apfu are present and not separated. The Report's Cr / V contribution to pink pyrope is absent from these spectra.

#### 2.1.2 Chrome Pyrope (added; the classic ruby-red pyrope)

1. **Cause.** Cr3+ on the Y site: 4A2 -> 4T2 at 571 nm and 4A2 -> 4T1 at about 415 nm (Report p. 15: 571 and 410 nm; GIA Winter 2015, https://www.gia.edu/gems-gemology/winter-2015-vanadium-chromium-bearing-pink-pyrope-garnet-characterization-quantitative-colorimetric-analysis, assigns the 550-600 nm band to Cr3+ and V3+ and 504 / 521 nm to Fe2+), plus Fe2+ 504 / 527 nm. Caltech index: GRR 332 and GRR 1055 "colored by Cr3+".
2. **Spectrum.** `variant_pyrope_chrome_iso.csv` = GRR 1055, Moses Rock, Utah (Ca0.33 Mn0.02 Mg2.13 Fe0.54 Al1.95 Cr0.07, 0.840 mm, 350-1108 nm). Baseline A = 0.007 (870 nm). Cr 0.07 apfu = 1.24 wt% Cr2O3 (0.035 Cr2O3 x 151.99 = 5.32 g in a formula mass of 428.8 g; hand arithmetic).
3. **Band model** (7 bands; rms 0.104 per mm of a 3.6 per mm peak, dE76 4.9 at k = 0.3):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 340 | 8000 | 3397 | 1.9 | 16.72 | UV / charge-transfer edge (estimated shape) |
| 2 | energy | 415 | 3000 | 1274 | 3 | 26.4 | Cr3+ 4A2 -> 4T1 |
| 3 | energy | 475 | 5500 | 2336 | 0.7 | 6.16 | blend of Fe2+ / Cr3+ wings (estimated assignment) |
| 4 | energy | 504 | 1000 | 425 | 0.35 | 3.08 | Fe2+ 504 nm |
| 5 | energy | 530 | 1200 | 510 | 0.3 | 2.64 | Fe2+ 527-530 nm |
| 6 | energy | 571 | 2300 | 977 | 2.1 | 18.48 | Cr3+ 4A2 -> 4T2 |
| 7 | energy | 690 | 1800 | 764 | 0.12 | 1.056 | Cr3+ / Fe2+ red tail |


4. **Target color.** k = 1 is black (L* 3.7; Cr 1.24 wt% over 8.8 mm). k = 0.3 (0.37 wt% Cr2O3): data L* 25.9 C* 38.9 h 16 (deep red), A: L* 31 C* 50 h 18; k = 0.15 (0.19 wt%): L* 47.8 C* 22.7 h 16 (medium red). Documented: red, no pleochroism.
5. **Strength anchor.** Sample 1.24 wt% Cr2O3; Report p. 15 puts gem pyrope Cr2O3 at 0.0-1.5 wt%, pink / red gem pyrope at 0.05-1. Suggested k = 0.3 (0.37 wt%).
6. **Confidence and gaps.** Medium: the fit overshoots by 0.1-0.2 per mm over 400-630 nm; the 475 nm blend band is an estimated assignment (Fe2+ / Cr3+ wings). Fe 0.54 apfu is in the sample (18 mol% almandine), so this is chrome pyrope, not pure Cr pyrope. The UV edge below 350 nm is not measured.

### 2.2 Almandine Garnet

#### 2.2.1 Purplish (dark reddish-purple almandine)

1. **Cause.** Fe2+ (X site) spin-forbidden bands at 504, 527, 576 nm over a strong blue-ultraviolet edge, plus the Fe2+-Fe3+ IVCT continuum (S1). Fe-rich almandine has the widest blue plateau, hence the purple tilt. Caltech: GRR 1776 "dark reddish-purple gemstone", GRR 1056 "light reddish-purple, colored by Fe2+".
2. **Spectrum.** `variant_almandine_purplish_iso.csv` = GRR 1776 ([Fe2.5 Mg0.3 Mn0.2 Ca0.1]Al2.0(SiO4)3, probably India, 0.213 mm, merged UV-Vis-NIR file cropped to 340-1110 nm). Fe 2.5 of 3 sites = 83 mol% almandine. Baseline A = 0.089 (840-860 nm). GRR 1056 (Wrangell, Alaska, Fe2.03, 1.0 mm normalized) was not used: its visible data stop at 730 nm (detector gap to 1100 nm), so the red end and the baseline are missing.
3. **Band model** (7 bands, native strength of GRR 1776; rms 0.093 per mm of a 1.5 per mm peak, dE76 6.5 at k = 0.3):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 330 | 10000 | 4247 | 1.4 | 12.32 | UV / charge-transfer edge (estimated shape) |
| 2 | energy | 440 | 6500 | 2760 | 0.62 | 5.456 | broad IVCT / continuum |
| 3 | energy | 504 | 1000 | 425 | 0.85 | 7.48 | Fe2+ 504 nm |
| 4 | energy | 530 | 1300 | 552 | 0.78 | 6.864 | Fe2+ 527-530 nm |
| 5 | energy | 576 | 1500 | 637 | 0.8 | 7.04 | Fe2+ 576 nm |
| 6 | energy | 620 | 1400 | 595 | 0.22 | 1.936 | Fe2+ 610 nm |
| 7 | energy | 700 | 1900 | 807 | 0.28 | 2.464 | Fe2+ 683-700 nm |


4. **Target color.** k = 1 is black (L* 5.7). k = 0.5: data L* 22.7 C* 48 h 19; k = 0.3: L* 39.3 C* 44.9 h 10 (deep red with a slight violet cast; the hue is redder than "purple", an honest limit of this sample). Documented: dark reddish-purple (Caltech). No pleochroism.
5. **Strength anchor.** Fe 2.5 apfu (83 mol% almandine) at k = 1. Per-mm Fe2+ 504 nm peak per almandine fraction is 1.0 here against 0.9 for rhodolite (Section 2.1.1), consistent linear Fe scaling. Suggested k = 0.3-0.5 (a gem path rarely reaches 8.8 mm of solid-color material; gem almandine is cut shallow or hollowed).
6. **Confidence and gaps.** Medium-high. The 330 nm edge, the 440 nm continuum and the 700 nm hump are shape stand-ins. Single sample.

### 2.3 Spessartine Garnet

The Material Default is the fitted GRR 43 "Mandarin" (calibration record "Spessartine").

#### 2.3.1 Reddish Orange (Fe- and Mn-rich; replaces the plan's "Mandarin")

1. **Cause.** Mn2+ spin-forbidden bands: 6A1 -> 4A1,4E(G) near 430 nm, 462 and 486 nm, and a 523 nm Mn2+ / Fe2+ band (S2, Taran et al. 2023, Am. Min. 108, https://msaweb.org/MSA/AmMin/products/AmMin/AM108P1149.pdf; Report p. 15).
2. **Spectrum.** `variant_spessartine_reddishorange_iso.csv` = GRR 72, Rutherford Mine, Virginia (Mn2.79 Fe0.07 Mg0.04 Ca0.03 Al2.00 = 93 mol% spessartine, 3.446 mm). **Gap:** the file starts at 430 nm (A = 2.38 there); the 380-430 nm Mn2+ lines are not measured. Baseline A = 0.300 (900 nm).
3. **Band model** (7 bands; rms over 440-780 nm 0.03 per mm, dE76 5.5 at k = 1 against the data clamped at 431 nm):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 345 | 7000 | 2973 | 5 | 44 | UV edge and the unmeasured Mn2+ lines below 430 nm (estimated: only an opacity floor, the file starts at 430 nm) |
| 2 | energy | 429 | 800 | 340 | 1.7 | 14.96 | Mn2+ 6A1 -> 4A1,4E(G) |
| 3 | energy | 462 | 1500 | 637 | 0.32 | 2.816 | Mn2+ 462 nm |
| 4 | energy | 486 | 1100 | 467 | 0.3 | 2.64 | Mn2+ 486 nm |
| 5 | energy | 525 | 2000 | 849 | 0.3 | 2.64 | Mn2+ / Fe2+ 525 nm |
| 6 | energy | 565 | 2500 | 1062 | 0.1 | 0.88 | tail (565 nm) |
| 7 | energy | 620 | 3500 | 1486 | 0.05 | 0.44 | tail (620 nm) |


4. **Target color.** k = 1: model L* 48.4 C* 85 h 59 (deep reddish orange); the repo's calibration record gives "L* 51, h 62" for GRR 72. k = 0.5: L* 67 C* 68 h 68; k = 0.3: L* 78 C* 53 h 76. Below 430 nm the 8.8 mm path is opaque (alpha above 1.4 per mm), so the unmeasured region does not move the color.
5. **Strength anchor.** Mn 2.79 apfu. GRR 72 is 2-3 x stronger per mm than the Material Default's GRR 43 (alpha(460) 0.55 against 0.29 per mm).
6. **Confidence and gaps.** Medium. Band 1 is an opacity floor, estimated, not data. The red-orange hue depends on the Fe2+ 525 nm and tail bands.

#### 2.3.2 Pale Golden Orange (scaled)

1. **Cause.** Mn2+ at lower Mn content (spessartine-pyrope or -grossular solid solution), same bands as the default; absorption scales linearly with the Mn fraction (Report p. 46).
2. **Spectrum.** None new: the Material Default's fitted bands (GRR 43) times k = 0.4.
3. **Band model** (8 bands: the default's bands with peaks x 0.4; widths unchanged; peaks in ModelUnit as stored in the repo, per mm = ModelUnit / 8.8):

| # | center_nm | FWHM (cm^-1) | peak ModelUnit (default x 0.4) | peak per mm |
|---|---|---|---|---|
| 1 | 320 | 7023 | 10.96 | 1.245 |
| 2 | 411 | 556 | 5.68 | 0.645 |
| 3 | 421 | 419 | 2.648 | 0.301 |
| 4 | 429 | 659 | 3.296 | 0.375 |
| 5 | 462 | 1551 | 0.892 | 0.101 |
| 6 | 485 | 811 | 0.636 | 0.072 |
| 7 | 523 | 1971 | 0.628 | 0.071 |
| 8 | 590 | 3000 | 0.0528 | 0.006 |

4. **Target color.** D65 L* 88.6 C* 39.8 h 76 (pale yellow-orange); illuminant A L* 91.5 C* 44 h 69.
5. **Strength anchor.** None published; k = 0.4 is a choice (**estimated**), range 0.3-0.6 gives L* 91-84.
6. **Confidence and gaps.** Low (D / C). It is a concentration scaling, not a measured pale spessartine. The ultraviolet edge also scales here, which is not guaranteed.

### 2.4 Grossular Garnet (Tsavorite)

The Material Default is the fitted tsavorite (GRR 229 at 0.5 x vanadium; 5 energy bands).

#### 2.4.1 Mint (light tsavorite)

1. **Cause.** V3+ (and traces of Cr3+) on the Y site: 600 / 606 nm and 420-436 nm bands, a green window near 510-540 nm (Report p. 16; Caltech: "colored green by 0.11 wt% V3+").
2. **Spectrum.** None new: GRR 229 (existing `caltech_tsavorite_grr229_0p758mm.csv`) as already fitted. By the repo's own scaling rule, the Material Default is 0.5 x the sample's vanadium; Mint is 0.18 x (record: "0.18 times, about 0.32 wt% V2O3, gives L* 78"), i.e. 0.36 x the default.
3. **Band model** (5 bands; default peaks x 0.36 in ModelUnit, widths unchanged, hand arithmetic):

| # | center_nm | FWHM (cm^-1) | peak ModelUnit | peak per mm |
|---|---|---|---|---|
| 1 | 393 | 3729 | 2.574 | 0.2925 |
| 2 | 436 | 2341 | 2.480 | 0.2818 |
| 3 | 490 | 2500 | 0.400 | 0.0454 |
| 4 | 606 | 2727 | 1.076 | 0.1223 |
| 5 | 666 | 1166 | 0.239 | 0.0271 |

4. **Target color.** D65 L* 79.3 C* 63.3 h 137 (light yellowish green; from the raw GRR 229 at 0.18 x: L* 79.4 C* 64.5 h 137); illuminant A L* 77 C* 42 h 135.
5. **Strength anchor.** Sample V 0.11 apfu = 1.83 wt% V2O3 (0.055 mol x 149.88 g / formula mass 450.8; hand arithmetic; the Caltech page text "0.11 wt% V3+" is the apfu number). Mint = 0.33 wt% V2O3.
6. **Confidence and gaps.** Medium-high (scaling of a measured spectrum, rule already used by the repo). The pale end of tsavorite also carries a different V : Cr ratio in nature.

#### 2.4.2 Hessonite (Orange-Brown)

1. **Cause.** Fe3+ on the Y site: the Fe3+ charge-transfer edge and a 440 nm spin-forbidden band (Report p. 16-17: x_And 0.02-0.15, edge tail, 440 nm). The Caltech index says only "orange-brown" for hessonite GRR 53 / GRR 1113 and gives GRR 1386 as "brown"; for GRR 1113 "with both Fe2+ and Fe3+".
2. **Spectrum.** `variant_grossular_hessonite_iso.csv` = GRR 1386 brown, Lalatema Hills, Kenya (4.126 mm, 373-1108 nm, **composition unpublished**). Baseline A = 0.052 (780-800 nm). Cross-check `variant_grossular_hessonite1113_iso.csv` = GRR 1113 (Riverside, California, 0.336 mm; Y site Fe0.43 = 21.5 mol% andradite, above the hessonite range, about 10 x stronger per mm).
3. **Band model** (4 bands; rms 0.032 per mm, dE76 6.5):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 360 | 9000 | 3822 | 0.72 | 6.336 | Fe3+ / O->Fe charge-transfer edge |
| 2 | energy | 440 | 1300 | 552 | 0.05 | 0.44 | Fe3+ 440 nm spin-forbidden (small) |
| 3 | energy | 450 | 5500 | 2336 | 0.2 | 1.76 | broad Fe3+ band (assignment estimated) |
| 4 | energy | 530 | 5500 | 2336 | 0.07 | 0.616 | long tail (530 nm) |


4. **Target color.** k = 1: data L* 62.8 C* 82.8 h 68 (orange-brown); k = 0.6: L* 74.4 C* 64.1 h 74; k = 0.35: L* 83.6 C* 44.3 h 79 (honey yellow-orange). Documented: orange-brown (Caltech).
5. **Strength anchor.** None published for GRR 1386. For gem hessonite x_And 0.02-0.15 (Report p. 16-17).
6. **Confidence and gaps.** Medium-low: the species label "hessonite" is Caltech's / the trade's; the sample is only "brown grossular". The band assignments of bands 3 and 4 are estimated. The 440 nm band is small here (0.05 per mm, a shoulder).

### 2.5 Andradite Garnet (Demantoid)

The Material Default is the bright green demantoid fitted to GIA 2952 (Fe3+ and Cr3+).

#### 2.5.1 Topazolite (yellow, greenish yellow)

1. **Cause.** Fe3+ only (no Cr3+): the O -> Fe3+ charge-transfer edge, a sharp 6A1 -> 4A1,4E(G) band at 441 nm, 6A1 -> 4T1 near 861 nm (Report p. 16-17; S8). Caltech: GRR 48 "colored greenish-yellow by Fe3+, near end-member"; GRR 3137 "the spectrum with the least deviation from pure andradite".
2. **Spectrum.** `variant_andradite_topazolite_iso.csv` = GRR 48, Val Malenco, Italy (Ca3.0 Mg0.01 Fe1.98 = 99 mol% andradite, 1.026 mm, 390-1108 nm). Baseline A = 0.196 (720-740 nm minimum). Not downloaded as CSV: GRR 3137 (Madagascar, 0.805 mm, light green) and GRR 1765 (Ural, 0.737 mm, pale yellow-green), both of similar shape and 2-3 x weaker per mm in the visible.
3. **Band model** (6 bands; rms 0.124 per mm of a 3.3 per mm peak, dE76 5.7):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 355 | 5500 | 2336 | 4.8 | 42.24 | Fe3+ charge-transfer edge |
| 2 | energy | 431 | 800 | 340 | 1.4 | 12.32 | Fe3+ shoulder (431 nm) |
| 3 | energy | 441 | 450 | 191 | 2.6 | 22.88 | Fe3+ 6A1 -> 4A1,4E(G) at 441 nm |
| 4 | energy | 470 | 3000 | 1274 | 0.26 | 2.288 | Fe3+ continuum (470 nm) |
| 5 | energy | 570 | 4300 | 1826 | 0.2 | 1.76 | Fe3+ continuum (570 nm) |
| 6 | energy | 860 | 2000 | 849 | 0.1 | 0.88 | Fe3+ 6A1 -> 4T1 (861 nm, NIR tail) |


4. **Target color.** k = 1: data L* 46.5 C* 55 h 86 (olive yellow); k = 0.5: L* 68.6 C* 55.7 h 96 (yellow, slightly green); k = 0.3: L* 79.7 C* 50.6 h 102 (greenish yellow). Documented: yellow to greenish yellow. No pleochroism.
5. **Strength anchor.** Fe 1.98 apfu. The 441 nm band is about 3.5 per mm (35 cm^-1; Report p. 17 uses 60 cm^-1 per x_And, a circular number).
6. **Confidence and gaps.** Medium-high. The 441 nm band is narrower than the 10 nm sampling of my check (band FWHM 450 cm^-1 = 8.7 nm).

### 2.6 Color-Change Garnet (Pyrope-Spessartine)

The Material Default is a tuned five-band set (410 / 421 / 430 Mn2+, 505 Fe2+, one broad 573 nm Cr3+ / V3+ band).

#### 2.6.1 Tunduru (Blue-Green to Purple)

1. **Cause.** A broad V3+ / Cr3+ 4T2 band near 580 nm flanked by two transmission windows (blue-green near 480 nm and red beyond 650 nm); Mn2+ 6A1 -> 4A1,4E lines at 411-433 nm cut the violet; Fe2+ must stay low (Report p. 15: x_Alm <= 0.10). GIA Winter 2015 (pink pyrope) explains the same windows: a red window about 650-700 nm and a higher-energy window near 477 nm, with Mn2+ lines at 410 / 422 / 430 nm. Caltech GRR 2225: "goes from pale purplish-blue to light purple depending upon the illumination ... about 0.6 wt% vanadium".
2. **Spectrum.** `variant_colorchange_tunduru_iso.csv` = GRR 2225 (Tunduru, Tanzania, 2.426 mm, 412-1101 nm). **Gap:** the file starts at 412 nm; the Mn2+ lines below that are cut. Baseline A = 0.655 (flat 800-880 nm); the baseline is large and whether it is a real V3+ plateau or reflection / scatter is not stated.
3. **Band model** (7 bands; rms 0.054 per mm):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 340 | 9000 | 3822 | 1 | 8.8 | UV edge (estimated: file starts at 412 nm) |
| 2 | energy | 411 | 700 | 297 | 1 | 8.8 | Mn2+ 411 nm (shape estimated: cut by the file start) |
| 3 | energy | 423 | 600 | 255 | 0.7 | 6.16 | Mn2+ 423 nm |
| 4 | energy | 433 | 1100 | 467 | 0.75 | 6.6 | Mn2+ 430 nm group |
| 5 | energy | 462 | 1800 | 764 | 0.2 | 1.76 | Mn2+ 462 nm |
| 6 | energy | 500 | 1500 | 637 | 0.09 | 0.792 | Fe2+ / Mn2+ 500 nm (weak) |
| 7 | energy | 580 | 2900 | 1232 | 0.62 | 5.456 | V3+ / Cr3+ 4T2 band (the color-change band) |


4. **Target color (the color change).** Data (D65 / A), 8.8 mm: k = 0.5: D65 L* 49.6 C* 14.9 h 192 (grey-blue-green), A L* 48.1 C* 11.9 h 285 (purple); k = 0.3: D65 L* 65 C* 13.6 h 176, A h 277; k = 1: D65 L* 26 h 214, A L* 25 h 305. Hue shift D65 to A 90-100 degrees (data), a clear color change. **The model reproduces the direction but not the full size:** k = 0.5 model D65 L* 50 C* 23 h 171, A C* 6.5 h 240 (dE76 to the data 11 / 8.5). Documented: pale purplish blue to light purple (Caltech).
5. **Strength anchor.** About 0.6 wt% V (Caltech). Report p. 15: Cr2O3 + V2O3 of 0.5-1.5 wt% for a color change.
6. **Confidence and gaps.** Medium for the 580 nm band (shape measured), low for the four bands below 440 nm (extrapolated). Suggested k = 0.5 for a 7 mm stone.

#### 2.6.2 Weaker Change and Stronger Change (estimated)

1. **Cause.** The same chromophores with the V3+ / Cr3+ band scaled: less V shifts the stone toward Mn orange-yellow (daylight green-yellow, lamp orange-pink); more V toward blue (daylight blue, lamp violet).
2. **Spectrum.** None: **estimated** multipliers on band 7 of the Tunduru set only (Mn lines unchanged): Weaker x 0.6 (about 0.36 wt% V2O3-equivalent, below the Report's 0.5 floor, so a pale change), Stronger x 1.8 (about 1.1 wt%, inside the Report range).
3. **Band model:** the Tunduru table with band 7 peak per mm 0.372 (Weaker) or 1.116 (Stronger); ModelUnit 3.27 / 9.82. Other six bands unchanged.
4. **Target color (model, k = 0.5, 8.8 mm).** Weaker: D65 L* 63 C* 31 h 127 (yellowish green), A L* 62.5 C* 17 h 90 (yellow). Stronger: D65 L* 35.5 C* 31 h 219 (blue), A L* 32 C* 32 h 255 (blue-violet). Tunduru model: D65 h 171, A h 240.
5. **Strength anchor.** None measured.
6. **Confidence and gaps.** Low (D). No measured sample of a weaker or stronger stone is in the repo or on the Caltech page. Offer only with the "estimated" tooltip, or leave out.

#### 2.6.3 Malaya (Orange-Pink, no change; added)

1. **Cause.** Mn2+ lines and Fe2+ over a broad plateau in a pyrope-spessartine with enough Fe2+ to fill the 480-520 nm window, so no color change (Report p. 15, 46); the trade name "Malaya" (Kenya).
2. **Spectrum.** `variant_colorchange_malaya_iso.csv` = GRR 1267, pyrope-spessartine, Kenya (1.686 mm, 378-1108 nm, composition unpublished). Baseline A = 0.152 (850-900 nm).
3. **Band model** (7 bands; rms 0.044 per mm, dE76 5.0):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 345 | 8000 | 3397 | 2.6 | 22.88 | UV edge |
| 2 | energy | 411 | 600 | 255 | 0.7 | 6.16 | Mn2+ 411 nm |
| 3 | energy | 430 | 900 | 382 | 0.5 | 4.4 | Mn2+ 430 nm |
| 4 | energy | 480 | 6000 | 2548 | 0.58 | 5.104 | broad plateau 450-520 nm (Fe2+ / Mn2+, assignment estimated) |
| 5 | energy | 510 | 1500 | 637 | 0.06 | 0.528 | Fe2+ 510 nm |
| 6 | energy | 545 | 2500 | 1062 | 0.1 | 0.88 | broad 545 nm (V3+ / Cr3+ / Fe2+, estimated) |
| 7 | energy | 690 | 2500 | 1062 | 0.07 | 0.616 | red hump 690 nm |


4. **Target color.** k = 1: data L* 30.3 C* 68.6 h 48 (dark red-orange); k = 0.5: L* 51.8 C* 70.7 h 60 (orange); k = 0.3: L* 66 C* 55.7 h 65 (light orange). Illuminant A is close (h 59).
5. **Strength anchor.** None published.
6. **Confidence and gaps.** Medium. Composition unknown, so the Mn : Fe split is unknown.

## 3. Peridot

Host: forsterite-rich olivine (Mg,Fe)2SiO4, orthorhombic Pbnm, biaxial positive. Optical directions (Caltech olivine page, http://minerals.gps.caltech.edu/FILES/Visible/olivine/Index.html): 10 Angstrom axis (b) = alpha, 6 Angstrom axis (c) = beta, 4.7 Angstrom axis (a) = gamma; this agrees with the repo's research note (alpha || b, beta || c, gamma || a). The Material Default is the fitted GRR 418 San Carlos set (6 bands per ray, alpha / beta / gamma stored as o_ray / beta_ray / e_ray); GRR 418 CSVs already exist (`caltech_olivine_grr418_{alpha,beta,gamma}.csv`).

### 3.1 Deep Green (Fe-richer; scaled)

1. **Cause.** Fe2+ on the M1 and M2 sites: spin-allowed bands near 840-1050 nm whose short-wave flank blocks the red, spin-forbidden bands at 403-490 and 635 nm, and the UV charge-transfer edge (Caltech: "green with lower contents of Fe2+ but becomes brown at high iron contents"; GIA Sardinia peridot study S4; Gems & Gemology Winter 2021 on Pyaung-Gaung peridot, https://gia.edu/gems-gemology/winter-2021-gemological-characterization-peridot-from-pyaung-gaung-mogok-myanmar, says the green is caused by Fe2+). The trade view is that about 12-15 % fayalite gives the best green and more gives a muddier brown (International Gem Society, secondary). Cr3+ and Ni2+ occur in the deepest Burmese and Pakistani stones but no band data are available, so they are **not** modelled.
2. **Spectrum.** No new spectrum. GRR 418 (San Carlos; alpha 1.141 mm, beta 1.138 mm, gamma 0.66 mm) is the data behind the default. Deep Green is the default with all peaks x 1.4 (**estimated**): fayalite 14 mol% against an assumed 10 mol% for San Carlos (no Fa analysis is published for GRR 418; Fo 90 is the general value for San Carlos olivine, not a source here), by the linear x_Fa scaling of Report p. 46.
3. **Band model** (6 bands per ray, the default's centres and widths, peaks x 1.4; ModelUnit as stored in the repo, per mm = ModelUnit / 8.8):

| ray | center_nm | FWHM (cm^-1) | peak ModelUnit (default x 1.4) | peak per mm |
|---|---|---|---|---|
| alpha | 345 | 10096 | 7.532 | 0.856 |
| alpha | 455 | 1410 | 0.742 | 0.084 |
| alpha | 490 | 2160 | 0.616 | 0.070 |
| alpha | 637 | 1724 | 1.12 | 0.127 |
| alpha | 840 | 3276 | 6.664 | 0.757 |
| alpha | 1000 | 2534 | 49.28 | 5.600 |
| beta | 341 | 12000 | 9.492 | 1.079 |
| beta | 465 | 2526 | 1.498 | 0.170 |
| beta | 500 | 3000 | 0.658 | 0.075 |
| beta | 635 | 2500 | 1.862 | 0.212 |
| beta | 840 | 4000 | 5.95 | 0.676 |
| beta | 1000 | 2401 | 49.28 | 5.600 |
| gamma | 376 | 3317 | 6.678 | 0.759 |
| gamma | 440 | 2933 | 3.136 | 0.356 |
| gamma | 500 | 3000 | 1.106 | 0.126 |
| gamma | 627 | 2500 | 1.078 | 0.123 |
| gamma | 840 | 4000 | 3.276 | 0.372 |
| gamma | 1000 | 3663 | 17.78 | 2.020 |

4. **Target color and pleochroism.** Default (x 1): mean of the three rays L* 75.8 C* 63.3 h 111; alpha L* 84 C* 66 h 110, beta L* 61 C* 68 h 109, gamma L* 79 C* 61 h 113. Deep Green x 1.4 (model, computed): mean L* 69.0 C* 72.1 h 110, alpha L* 79.0 C* 79.0, beta L* 50.7 C* 70.8, gamma L* 72.4 C* 70.5 (hue stays 108-112 in all rays: pleochroism is in strength, beta darkest, not in hue). Illuminant A, x 1.4: L* 69.3 C* 64.7 h 107.
5. **Strength anchor.** Fo 90 to Fo 86 (x_Fa 0.10 to 0.14), **estimated**; Report p. 18: gem peridot Fo 88-93.
6. **Confidence and gaps.** Low-medium (C). Only a strength scaling; the real deepest stones owe part of their color to Cr / Ni. The raw GRR 418 spectra give a lighter default than the repo's bands (L* about 90 against 76), because the repo's fit also includes baseline-like terms in beta; use the relative factor only.

## 4. Zircon

Host: ZrSiO4, tetragonal, uniaxial positive. E perpendicular to c is the o-ray ("Along"), E parallel to c the e-ray ("Across"). The Material Default is an isotropic set of U4+ lines (finding 1). Caltech zircon page (http://minerals.gps.caltech.edu/FILES/Visible/zircon/Index.html): "The color in zircon comes from both uranium ions and radiation damage. U4+ ... causes the blue color of heat-treated zircon ... radiation damage centers cause a range of red-brown and amber colors. Heat treatment removes the radiation damage centers and restores the blue color ... Green colors are associated with a mixture of the blue color from uranium and red-brown color from radiation damage centers ... Colorless zircons have little uranium." GIA Gems & Gemology Summer 2015 Lab Notes (https://www.gia.edu/gems-gemology/summer-2015-labnotes-chemical-analysis-zircon): blue theorized to come from U4+; "electric" blue by heating near 1000 C in a reducing atmosphere, near-colorless in an oxidizing one; heavily metamict stones never reach electric blue. Other sources: Renfro et al., G&G 52(3), 2016 (S9) and Zeug et al., J. Gemmology 2018 (https://www.duo.uio.no/bitstream/10852/67410/1/Zeug_2018_JGe_Ratanakiri-zircon.pdf; U4+ lines at 654 and 690 nm), both via the repo guide.

### 4.1 Blue (heat-treated)

1. **Cause.** An electron-trap centre associated with U4+ made by heating brown zircon at 900-1000 C in a reducing atmosphere: a broad band centred near 650 nm, polarized strongly along E perpendicular to c (Report p. 19: 650 nm, FWHM about 3500 cm^-1, e-ray 0.1 x the o-ray). The measured sample below confirms every one of these numbers (650 nm, FWHM 3700 cm^-1, e / o below 0.1).
2. **Spectra.** `variant_zircon_blue_eperpc.csv` and `variant_zircon_blue_eparc.csv` = GRR 1898, Burma near the Thai border, "probably heat treated", 2.65 mm, 350-1104 nm. Baselines A = 0.300 (o, 420 nm minimum) and 0.200 (e, 500 nm).
3. **Band model.** o-ray (2 bands; rms 0.015 per mm):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 332 | 5000 | 2123 | 0.45 | 3.96 | UV edge (estimated shape; file starts at 350 nm) |
| 2 | energy | 650 | 3700 | 1571 | 0.115 | 1.012 | heat-treated blue centre, electron trap near U4+ (650 nm, FWHM 3700) |


   e-ray (1 band; rms 0.008 per mm). Its absorption in the visible is below 0.01 per mm, so only the edge is kept:


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 332 | 5000 | 2123 | 0.5 | 4.4 | UV edge only (estimated); e-ray absorption in the visible is below 0.01 per mm |


4. **Target color and pleochroism.** o-ray alone, D65, 8.8 mm, k = 1: L* 85.5 C* 27 h 230 (pale sky blue); e-ray L* 98.4 C* 2.4 (colorless). Face-up (2 o + e) / 3: L* 90.2 C* 17.1 h 229 (pale blue); k = 2: L* 83.2 C* 25.7 h 235; k = 3: L* 78 C* 30 h 241 (the model at k = 3 is L* 85, too light by 7). Documented: blue, strongly dichroic (Report p. 19; Caltech). Illuminant A: L* 88 C* 22.5 h 226.
5. **Strength anchor.** None published. The sample is a pale to medium blue at 2.65 mm; a saturated gem needs k = 2-3 (**estimated**, from how the sample looks at 8.8 mm).
6. **Confidence and gaps.** High for the shape and the polarization, medium for the strength. The UV edge is a stand-in (file starts at 350 nm).

### 4.2 Golden Brown

1. **Cause.** A broad radiation-damage (alpha-recoil) continuum rising toward the ultraviolet over weak U4+ lines (653.4, 690.9, 589.7, 515.7 nm). The Caltech sample is "pale / light brown", not named golden; golden stones are heat-treated in air in the trade (no spectrum in the repo or on Caltech). **Estimated** as the closest measured light brown.
2. **Spectra.** `variant_zircon_golden_eperpc.csv` and `variant_zircon_golden_eparc.csv` = GRR 3106 light brown region, Mogok, Myanmar, 3.806 mm, merged spectra with sharp lines. Baselines A = 0.110 (o) and 0.105 (e).
3. **Band model.** o-ray (8 bands; rms 0.036 per mm, mix dE76 5.6):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 350 | 7500 | 3185 | 0.45 | 3.96 | UV / damage edge |
| 2 | energy | 470 | 5500 | 2336 | 0.14 | 1.232 | broad brown band |
| 3 | energy | 590 | 3500 | 1486 | 0.07 | 0.616 | broad 600 nm shoulder |
| 4 | energy | 515.7 | 170 | 72 | 0.2 | 1.76 | U4+ line 515.7 nm |
| 5 | energy | 589.7 | 100 | 42 | 0.12 | 1.056 | U4+ line 589.7 nm |
| 6 | energy | 653.4 | 105 | 45 | 0.67 | 5.896 | U4+ line 653.4 nm (strongest) |
| 7 | energy | 659.4 | 150 | 64 | 0.23 | 2.024 | line 659.4 nm (shoulder) |
| 8 | energy | 690.9 | 65 | 28 | 0.28 | 2.464 | U4+ line 690.9 nm |


   e-ray (8 bands; rms 0.028 per mm):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 350 | 7500 | 3185 | 0.46 | 4.048 | UV / damage edge |
| 2 | energy | 470 | 5500 | 2336 | 0.19 | 1.672 | broad brown band |
| 3 | energy | 590 | 3500 | 1486 | 0.08 | 0.704 | broad 590 nm shoulder |
| 4 | energy | 511.5 | 150 | 64 | 0.12 | 1.056 | line 511.5 nm |
| 5 | energy | 589 | 100 | 42 | 0.16 | 1.408 | line 589 nm |
| 6 | energy | 616 | 150 | 64 | 0.13 | 1.144 | line 616 nm |
| 7 | energy | 652.5 | 110 | 47 | 0.27 | 2.376 | line 652.5 nm |
| 8 | energy | 660.8 | 80 | 34 | 0.24 | 2.112 | line 660.8 nm |


4. **Target color and pleochroism.** Face-up data, D65, 8.8 mm: k = 0.5 L* 78.3 C* 20.8 h 81 (pale yellow); k = 1 L* 61.9 C* 33.3 h 78 (golden brown); k = 2 L* 39 C* 43.2 h 73 (brown). Rays at k = 1: o L* 62.6 C* 30.7 h 71, e L* 60.7 C* 40.9 h 88: nearly equal, a slight yellower e-ray.
5. **Strength anchor.** None published.
6. **Confidence and gaps.** Medium (B). Line widths are the measured 65-170 cm^-1; dropped lines: 483, 537, 562 nm (o) and 465, 485, 542 nm (e), each below 0.15 per mm net.

### 4.3 Red-Brown

1. **Cause.** Alpha-recoil radiation damage from U / Th (Report p. 19: 485 nm band, FWHM about 4500 cm^-1, rising continuum to 380 nm); annealing above 900 C repairs it. Caltech: GRR 1844 "brownish-red ... colored by radiation damage".
2. **Spectra.** `variant_zircon_redbrown_eperpc.csv`, `variant_zircon_redbrown_eparc.csv` = GRR 1844, Chanthaburi, Thailand (ruby fields), 1.519 mm. Baseline A = 0.215 (780 nm; the curve is still falling). Note: the e-ray file's first row reads 76.0 nm (an evident typo in the source), kept unchanged; ignore rows below 300 nm. **The two polarizations agree to within 0.05 absorbance above 440 nm**, so red-brown is not pleochroic in the visible; both rays carry the same set and the provenance should say so (plan section 2.1).
3. **Band model** (3 bands, o = e; rms 0.035 per mm, dE76 2.8):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 360 | 7500 | 3185 | 0.95 | 8.36 | damage continuum / UV edge |
| 2 | energy | 480 | 4800 | 2038 | 0.42 | 3.696 | alpha-recoil brown band |
| 3 | energy | 570 | 3000 | 1274 | 0.12 | 1.056 | red tail of the damage band |


4. **Target color.** k = 1: data L* 45.6 C* 75.5 h 52 (red-brown); k = 0.5: L* 64.9 C* 53.3 h 54; k = 0.25: L* 79.7 C* 31.5 h 56 (light orange-brown). Illuminant A, k = 1: L* 52.7 C* 85.5 h 55.
5. **Strength anchor.** None published (sample is a 1.5 mm slab).
6. **Confidence and gaps.** High for the shape (two matching rays), the strength is a choice. The band at 360 nm is an edge stand-in.

### 4.4 Green

1. **Cause.** U4+ sharp lines (482.8, 515.7, 537.2, 562.3, 589.7, 653.4, 690.9 nm along E perpendicular to c) over a broad radiation-damage continuum; Caltech: GRR 3106 "most of the spectroscopic features are from U4+", green = blue U4+ centre + red-brown damage. GRR 3106 is a Mogok crystal, not stated metamict.
2. **Spectra.** `variant_zircon_green_eperpc.csv`, `variant_zircon_green_eparc.csv` = GRR 3106 green, 2.592 mm. Baselines A = 0.105 (o) and 0.060 (e), 720 nm minimum.
3. **Band model.** o-ray (8 bands; face-up dE76 5.1 D65, 11.4 illuminant A):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 350 | 7500 | 3185 | 0.68 | 5.984 | UV / damage edge |
| 2 | energy | 470 | 5500 | 2336 | 0.2 | 1.76 | broad brown band |
| 3 | energy | 600 | 3500 | 1486 | 0.11 | 0.968 | broad 600 nm shoulder |
| 4 | energy | 515.7 | 170 | 72 | 0.54 | 4.752 | U4+ line 515.7 nm |
| 5 | energy | 589.7 | 100 | 42 | 0.4 | 3.52 | U4+ line 589.7 nm |
| 6 | energy | 653.4 | 105 | 45 | 1.63 | 14.34 | U4+ line 653.4 nm (strongest) |
| 7 | energy | 659.5 | 150 | 64 | 0.5 | 4.4 | line 659.5 nm (shoulder) |
| 8 | energy | 690.9 | 65 | 28 | 0.72 | 6.336 | U4+ line 690.9 nm |


   e-ray (8 bands; dropped: lines at 465, 485, 542 nm; the e-ray color is only roughly reproduced, model h 69 against data h 111 at C* 30):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 350 | 7500 | 3185 | 0.68 | 5.984 | UV / damage edge |
| 2 | energy | 470 | 5500 | 2336 | 0.24 | 2.112 | broad brown band |
| 3 | energy | 580 | 3500 | 1486 | 0.13 | 1.144 | broad 580 nm shoulder |
| 4 | energy | 511.5 | 150 | 64 | 0.42 | 3.696 | line 511.5 nm |
| 5 | energy | 589 | 100 | 42 | 0.45 | 3.96 | line 589 nm |
| 6 | energy | 616 | 150 | 64 | 0.4 | 3.52 | line 616 nm |
| 7 | energy | 652.5 | 110 | 47 | 0.89 | 7.832 | line 652.5 nm |
| 8 | energy | 660.8 | 80 | 34 | 0.76 | 6.688 | line 660.8 nm |


4. **Target color and pleochroism.** Face-up data, D65: k = 1 L* 53.1 C* 29.2 h 90 (yellow-green, olive); k = 0.5 L* 70.9 C* 18.8 h 92; k = 2 L* 31 C* 36 h 89. Rays at k = 1: o L* 55 C* 29 h 81, e L* 48 C* 33 h 111 (greener e-ray). Documented: green (Caltech). The lines carry the color: at 8.8 mm the 653 nm line is opaque.
5. **Strength anchor.** None published.
6. **Confidence and gaps.** Medium. 8 bands per ray cannot hold the line spectrum (the o-ray lost 483 and 537 nm, which the bands now smear into the continuum). Sharp lines (FWHM 65-170 cm^-1) are narrower than a 5 nm color grid; a 1 nm grid was used here.

### 4.5 Colorless (estimated)

1. **Cause.** Low U, no damage centre (Caltech zircon page). No spectrum.
2. **Band model** (1 band per ray, o = e): one near-zero ultraviolet-edge band, **estimated** and present only because an empty set is invalid: energy, centre 330 nm, FWHM 5000 cm^-1, peak 0.01 per mm (0.088 ModelUnit). At 380 nm this contributes 0.98 x 0.01 per mm, face-up D65 L* 100.0 C* 0.0.
3. **Confidence.** D. Do not describe it as measured.

## 5. Benitoite

Host: BaTiSi3O9, hexagonal, uniaxial positive. The Material Default is one energy Gaussian per ray, shape fitted to GRR 561-D4 and strength tuned (calibration record "Benitoite").

### 5.1 Light Blue (scaled)

1. **Cause.** Unresolved. Proposed: Fe2+-Ti4+ or Fe2+-Fe3+ intervalence charge transfer (Burns 1970, Millage 1981), Zr in the Ti site, defects (Burns et al. 1967); Laurs, Rohtert and Gray, G&G 33(3), 1997, 166-187 (https://www.gia.edu/doc/benitoite-from-the-new-idria-district.pdf): "the cause of color in benitoite remains elusive"; blue gem-mine benitoite has FeO 0.20 wt% against 0.04 wt% in colorless (Table 2, emission spectroscopy). The spectrum shows "a broad peak at about 700 nm, most of which appears in the near-infrared" (their Fig. 28).
2. **Spectra.** `variant_benitoite_lightblue_eperpc.csv` and `variant_benitoite_lightblue_eparc.csv` = GRR 561-D4 **dark blue zone** (The Gem Mine, 0.997 mm, 300-2599 nm; the same data as the existing record). Baseline A = 0.46. Measured e-ray: peak about 6.3 per mm near 720 nm (A about 2.7 over 1 mm), blue window below 480 nm; o-ray: 0.06 per mm at 600 nm, essentially colorless.
3. **Band model.** Light Blue is the Material Default x 0.5 (**estimated**; hand arithmetic, ModelUnit as stored in the repo, per mm = ModelUnit / 8.8):

| ray | shape | center_nm | FWHM (cm^-1) | peak ModelUnit | peak per mm |
|---|---|---|---|---|---|
| o | energy | 603 | 8630 | 0.055 | 0.00625 |
| e | energy | 698 | 7100 | 1.000 | 0.1136 |

4. **Target color and pleochroism.** Default x 1: e-ray L* 70.6 C* 44.5 h 261, o-ray L* 96.3 C* 3.8, face-up L* 89.1 C* 14.9 h 266. Light Blue x 0.5: e-ray L* 83.4 C* 26.3 h 252, o-ray L* 98.2, face-up L* 93.7 C* 9.1 h 258 (very pale blue; the plain stone is mostly o-ray). An optional deeper stone x 2: e-ray L* 52.5 C* 70 h 279, face-up L* 82.8 C* 22.2 h 278. Documented (GIA 1997): strongly dichroic, "typically violetish blue and colorless"; colorless looking down c.
5. **Strength anchor.** The repo default's e-ray peak (0.23 per mm) is 3.6 % of the dark zone's 6.3 per mm; the Report's 45 cm^-1 per wt% FeO with FeO 0.20 wt% would give 0.9 per mm. These disagree by a factor of 4-30; the strength is unknown and k = 0.5 is **estimated**.
6. **Confidence and gaps.** Shape: high (measured, both rays). Strength: low. No concentration for the measured piece. The ultraviolet edge (A about 0.62 at 350 nm, both rays) is not modelled.

## 6. Tanzanite (zoisite)

Host: zoisite Ca2Al3(SiO4)3(OH), orthorhombic Pnma, biaxial positive. Directions (Caltech zoisite page, Hurlbut, American Mineralogist 54, 702, 1969 via the repo): alpha || b (red-violet), beta || c (blue), gamma || a (yellow-green); the stored order is alpha / beta / gamma = o_ray / beta_ray / e_ray as in the Material Default. The default is the fitted heated GRR 2292 blue zone. Sources: Caltech zoisite page (http://minerals.gps.caltech.edu/FILES/Visible/zoisite/Index.html: "The blue variety, tanzanite, is colored by vanadium. It is highly pleochroic before it is heat-treated ... Green varieties contain Fe3+"); Pluthametwisute et al., Molecules 25, 3743, 2020 (https://pmc.ncbi.nlm.nih.gov/articles/PMC7465713/; via the repo notes: 460 nm band reduced by heating, largest along c, 585-600 nm unchanged; V2O3 0.06-0.43 wt%); Schwarzinger, Ulatowski and Rossman, "Zoisite and its many colors", 38th IGC, Athens, 2025 (https://www.igc-gemmology.org/wp-content/uploads/2025/11/2025-Schwarzinger-et-al-Zoisite-and-its-many-colors.pdf; Fe2+-Ti4+ IVCT assigned to the yellow-brown unheated color, correlated with Ti and a band near 445 nm in the gamma ray; green zoisite = V plus Fe-Ti IVCT, or Cr dominated; the band positions and trace tables were not readable).

### 6.1 Unheated

1. **Cause.** Heated tanzanite: V3+ bands near 600 and 740 nm. Unheated adds the Fe2+-Ti4+ IVCT band near 440 nm (strongest along gamma), which gives the brown to yellow direction and is removed by heating near 500-550 C. The cause of the heating change (V3+ to V4+ or Ti3+ to Ti4+) is disputed (Pluthametwisute 2020).
2. **Spectra.** `variant_tanzanite_unheated_alpha.csv`, `_beta.csv`, `_gamma.csv` = Caltech GRR 1265, Tanzania (V2O3 = 0.35, FeO = 0.027 in the header), one single crystal, all three rays at 3.00 mm "normalized", 401-1104 nm. Baselines A = 0.038 (alpha), 0.035 (beta), 0.046 (gamma), the 1000 nm values (the repo's near-infrared baseline convention). Unlike the 2292 blue zone (alpha from a different 5.142 mm piece) all three rays share a crystal. The repo's existing `caltech_zoisite_grr1265_5nm.csv` holds the same sample on a 5 nm grid.
3. **Band model** (per mm at V2O3 0.35 wt%, baselines removed). alpha (4 bands; rms 0.013 per mm; edge bands estimated):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 365 | 3000 | 1274 | 0.5 | 4.4 | UV edge (estimated: file starts at 401 nm) |
| 2 | energy | 460 | 6500 | 2760 | 0.14 | 1.232 | broad violet-blue band |
| 3 | energy | 560 | 3000 | 1274 | 0.19 | 1.672 | V3+ band near 560 nm (the red-violet alpha color) |
| 4 | energy | 620 | 2500 | 1062 | 0.03 | 0.264 | red edge of the V3+ band |


   beta (5 bands; rms 0.033 per mm):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 365 | 3000 | 1274 | 0.9 | 7.92 | UV edge (estimated) |
| 2 | energy | 470 | 6000 | 2548 | 0.12 | 1.056 | broad blue band |
| 3 | energy | 600 | 2800 | 1189 | 0.12 | 1.056 | V3+ 600 nm band |
| 4 | energy | 760 | 4500 | 1911 | 0.17 | 1.496 | V3+ / Fe NIR band 760 nm |
| 5 | energy | 540 | 4000 | 1699 | 0.05 | 0.44 | broad 540 nm shoulder |


   gamma (5 bands; rms about 0.02 per mm over 420-780 nm):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 385 | 1700 | 722 | 8.3 | 73.04 | UV edge (steep, measured at 401-420 nm) |
| 2 | energy | 440 | 3200 | 1359 | 0.19 | 1.672 | Fe2+-Ti4+ IVCT band near 440 nm (the unheated yellow) |
| 3 | energy | 560 | 9000 | 3822 | 0.085 | 0.748 | broad continuum |
| 4 | energy | 602 | 2800 | 1189 | 0.07 | 0.616 | V3+ 600 nm band |
| 5 | energy | 745 | 3500 | 1486 | 0.03 | 0.264 | V3+ 745 nm band |


4. **Target color and pleochroism.** Rays, D65, 8.8 mm, k = 1 (data): alpha L* 50.5 C* 32.6 h 326 (red-violet), beta L* 54.9 C* 11.3 h 254 (blue-grey), gamma L* 68.8 C* 37.4 h 133 (yellow-green); the models give 50.3 / 36.5 / 329, 54.8 / 19.1 / 269 and 65.1 / 34.6 / 136. Face-up (mean of the three) is nearly neutral, data L* 58.9 C* 3.4, model L* 57.3 C* 4.6 (grey; k = 2: L* 35, k = 3: L* 20). This reproduces the Caltech description of GRR 1265, "greenish yellow to deep blue to red-violet". The plan's "brown" direction would come from a stronger 440 nm band (GRR 2292 yellow zone A = 0.75 at 446 nm, downloaded for the digest but not fitted).
5. **Strength anchor.** V2O3 0.35 wt% at k = 1. The Material Default is about 0.6 wt% (calibration record), so equal vanadium is k = 1.7.
6. **Confidence and gaps.** Medium-high for rays and colors. The file starts at 401 nm, so the UV edges (alpha, beta) are estimated; the steep gamma edge is measured at 401-420 nm only. The face-up mix of three different colors is nearly colorless, which is the physics, not a fault, but the cutter's view depends on orientation.

### 6.2 Green Zoisite (Alchuri, Pakistan)

1. **Cause.** A mix of V3+ and Fe-Ti IVCT (yellow plus blue) or Cr3+ dominance (Schwarzinger et al. 2025; Caltech: "Green varieties contain Fe3+" and "pleochroic colors, but not as spectacular as those of Tanzanite"). Pakistani CIT-16940 chemistry is not published.
2. **Spectra.** `variant_zoisite_green_alpha.csv` (6.75 mm), `_beta.csv` (5.585 mm), `_gamma.csv` (6.275 mm) = Caltech CIT-16940, Alchuri, Shigar Valley. Baselines A = 0.104 (alpha, 900 nm), 0.075 (beta, 1000 nm), 0.036 (gamma, 1000 nm). **Header conflict:** the file the index lists as E||c = beta reads "E\\a, k1e2" in its own header; unresolved, may be a typo. The three rays come from different faces and thicknesses (a 'short on nice face', b 'short on bad face').
3. **Band model** (per mm at the sample strength). alpha (5 bands; rms 0.031 per mm):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 390 | 2300 | 977 | 0.4 | 3.52 | 380-400 nm band / UV edge |
| 2 | energy | 455 | 3200 | 1359 | 0.13 | 1.144 | broad blue band |
| 3 | energy | 500 | 3500 | 1486 | 0.03 | 0.264 | weak 500 nm shoulder |
| 4 | energy | 570 | 9000 | 3822 | 0.075 | 0.66 | broad yellow-brown band |
| 5 | energy | 650 | 5000 | 2123 | 0.02 | 0.176 | red edge |


   beta (4 bands; rms 0.039 per mm):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 385 | 2800 | 1189 | 0.5 | 4.4 | 380 nm edge |
| 2 | energy | 580 | 10000 | 4247 | 0.19 | 1.672 | broad plateau (yellow-brown) |
| 3 | energy | 455 | 3500 | 1486 | 0.07 | 0.616 | blue shoulder |
| 4 | energy | 640 | 3500 | 1486 | 0.03 | 0.264 | red band |


   gamma (4 bands; rms 0.026 per mm):


| # | shape | center_nm | FWHM (cm^-1) | sigma (cm^-1) | peak per mm | peak ModelUnit (x8.8) | band |
|---|---|---|---|---|---|---|---|
| 1 | energy | 390 | 2800 | 1189 | 0.36 | 3.168 | 390 nm band / UV edge |
| 2 | energy | 455 | 3500 | 1486 | 0.06 | 0.528 | blue shoulder |
| 3 | energy | 500 | 5000 | 2123 | 0.03 | 0.264 | weak 500 nm shoulder |
| 4 | energy | 745 | 7000 | 2973 | 0.1 | 0.88 | broad NIR band (V3+ / Cr3+) |


4. **Target color and pleochroism.** Rays, D65, 8.8 mm, k = 1 (data): alpha L* 74.6 C* 37.1 h 96 (yellow), beta L* 48.9 C* 14.1 h 133 (olive-grey), gamma L* 79.2 C* 18.5 h 150 (pale green); the model rays are 69.6 / 23.4 / 90, 49.2 / 8.0 / 260 and 81.8 / 17.8 / 130 (the beta hue is wrong at C* 8; low-chroma colors are sensitive). Face-up data L* 69.6 C* 20.8 h 118 (yellowish-green grey), model dE76 8.3; k = 2: L* 50.8 C* 25.9 h 117; k = 3: L* 37.6 C* 25.5 h 117.
5. **Strength anchor.** None published.
6. **Confidence and gaps.** Medium-low (B). Single sample, mismatched ray faces, beta label conflict, low chroma. Offer with the pleochroism noted as approximate.

## 7. Sources (all opened or read in this lane unless marked)

Caltech Mineral Spectroscopy Server, G. R. Rossman (CC BY-NC 4.0, DOI 10.7907/jywr-qq57):

- Index pages: http://minerals.gps.caltech.edu/FILES/Visible/garnet/Index.html, .../zircon/Index.html, .../benitoite/Index.html, .../olivine/Index.html, .../zoisite/Index.html (read over `http`).
- Data files, base http://minerals.gps.caltech.edu/FILES/Visible/: `garnet/grr131`, `garnet/grr83`, `garnet/Pyrope_grr1055.txt`, `garnet/Almandine_1776_Merged.txt`, `garnet/grr72`, `garnet/Grossular_GRR1386_brown.txt`, `garnet/Grossular_GRR1113_Riverside.txt`, `garnet/grr48`, `garnet/Pyrope2225.asc`, `garnet/GRR1267b.txt`, `zircon/Zir1898.a`, `zircon/Zir1898.c`, `zircon/zi1844.a`, `zircon/zi1844.c`, `zircon/Zircon3106-brown_a.asc` and `_c.asc`, `zircon/Zircon3106-GREEN_a.asc` and `_c.asc`, `benitoite/benitoite561-D4D.A.txt` and `.C.txt`, `zoisite/zo1265.a`, `.b`, `.c`, `zoisite/Zoisite16940_a.txt`, `_b.txt`, `_c.txt`. Also downloaded for evaluation only (not stored): `garnet/Andradite_1765_Russia.txt`, `garnet/Andradite3137_%20Madagascar.txt`, `garnet/Andradite2666_unheated.asc`, `zircon/zi1486`, `zircon/Zircon3106-PaleRed_*.asc` and `-DarkRed_*.asc`, `zoisite/Zoisite2292_yellow_*.asc`, `zoisite/Zoisite2891*.asc`, `zoisite/Zoisite17283*.txt`, `olivine/ol418.*`.

Literature:

- GIA, Gems & Gemology Winter 2015, vanadium / chromium-bearing pink pyrope (https://www.gia.edu/gems-gemology/winter-2015-vanadium-chromium-bearing-pink-pyrope-garnet-characterization-quantitative-colorimetric-analysis; page read).
- Stockton and Manson 1985, G&G 21(4) (https://www.gia.edu/gems-gemology/winter-1985-garnet-classification-stockton; abstract only).
- Taran et al. 2023, Am. Min. 108, 1149 (https://msaweb.org/MSA/AmMin/products/AmMin/AM108P1149.pdf; not readable as text here; cited via the repo guide).
- Laurs, Rohtert and Gray 1997, G&G 33(3) (https://www.gia.edu/doc/benitoite-from-the-new-idria-district.pdf; text read, Table 2 and spectroscopy section).
- GIA Lab Notes Summer 2015, zircon (https://www.gia.edu/gems-gemology/summer-2015-labnotes-chemical-analysis-zircon; page read).
- Renfro et al., G&G 52(3), 2016 (https://gia.edu/gems-gemology/fall-2016-reversible-color-modification-blue-zircon-long-wave-ultraviolet-radiation; via the repo guide).
- Zeug et al. 2018, J. Gemmology (https://www.duo.uio.no/bitstream/10852/67410/1/Zeug_2018_JGe_Ratanakiri-zircon.pdf; via the repo guide).
- Schwarzinger, Ulatowski and Rossman 2025, IGC Athens (https://www.igc-gemmology.org/wp-content/uploads/2025/11/2025-Schwarzinger-et-al-Zoisite-and-its-many-colors.pdf; text without digits).
- Pluthametwisute et al. 2020, Molecules 25, 3743 (https://pmc.ncbi.nlm.nih.gov/articles/PMC7465713/; page blocked by a captcha, search-result summary and the repo notes used).
- Hurlbut 1969, Am. Min. 54, 702 (https://msaweb.org/AmMin/AM54/AM54_702.pdf; via the repo notes).
- GIA, Winter 2021, peridot from Pyaung-Gaung (https://gia.edu/gems-gemology/winter-2021-gemological-characterization-peridot-from-pyaung-gaung-mogok-myanmar; search summary only); International Gem Society peridot page (https://www.gemsociety.org/article/peridot-jewelry-and-gemstone-information; search summary only, secondary).
- Fe2+-Fe3+ IVCT in garnets, Am. Min. 92, 2007, doi:10.2138/am.2007.2163 (S1, via the repo guide).
- Repo: `docs/chromophores/garnet_peridot_zircon_benitoite.md` (Report pp. 14-21, 46), `calibration-records.md`, `research-2026-10-primary-data.md`, `research-2026-10-polarized-spectra*.md`, `primary-2026-10/data/tanzanite/NOTES.md`.

## 8. Open questions

1. Zircon Material Default is isotropic on a uniaxial host (finding 1).
2. Which strength the lead wants as `k` for each variant; the document gives native strength plus colors at two or three k values.
3. Whether to keep the estimated-only entries (Weaker Change, Stronger Change, Colorless zircon, Pale Golden Orange, Deep Green, Light Blue). They have no measured spectrum of their own.
4. Caltech GRR 2292 thickness normalization and CIT-16940 beta header conflict stay unresolved (only the server owner can close them).
5. No published concentration exists for the zircon, hessonite, Malaya and green zoisite samples.
