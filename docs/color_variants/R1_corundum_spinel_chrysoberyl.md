# Color variants, research lane R1: corundum, spinel, chrysoberyl (2026-10-27)

Materials: Sapphire, Ruby, Spinel, Chrysoberyl (Yellow), Alexandrite. Plan: `color_variants_plan_2026-10-27/PLAN_color_variants_2026-10-27.md` sections 2, 6, 7.
No Rust was written or run. Every number below is either read from a numeric data file (listed), taken from a cited paper, or **derived by hand/script arithmetic from those files** (marked `derived`). Anything that is a judgement is marked `estimated` with what it rests on. **No number was digitised from a figure** in this lane; the only figure-read numbers are the ones already in the repo's Alexandrite comment (Farrell & Newnham Figs. 3-4), marked `repo figure-read`.

## 0. Conventions and method

### 0.1 Axes and rays

* Corundum (uniaxial negative): **o-ray = E perpendicular to c = "Along"** (the color seen looking down c; GIA "Ec"). **e-ray = E parallel to c = "Across"** (GIA "E||c").
* Spinel: cubic, one spectrum.
* Chrysoberyl / alexandrite (biaxial, Pnma, a 9.40, b 5.48, c 4.43 A). Caltech headers and Farrell & Newnham (1965, https://msaweb.org/AmMin/AM50/AM50_1972.pdf) agree with the repo's Alexandrite code: **E||a = beta (short direction, yellow), E||b = gamma (blue-green), E||c = alpha (red)**. The earlier research table in `docs/chromophores/research-2026-10-polarized-spectra.md` section 3 (alpha = E||c bluish green, beta = E||b orange-yellow, gamma = E||a purplish red) and the CSV built from it (`alexandrite_natural_russian.csv`, headed "beta_par_b / gamma_par_a") **contradict the Caltech files and F&N; do not use them** (see 0.5).

### 0.2 Strength convention used for recipes (inferred from the repo, checked numerically)

The repo's fitted corundum presets state their strength as an areal density in **ppma*cm over the 8.8 mm face-up path** (ppma = parts per million of all atoms, 1 ppma = 1.178e17 cm^-3 in corundum, GIA Box A). Ruby = 800 ppma*cm Cr (about 900 ppma over 0.88 cm), Sapphire = 10 ppma*cm Fe2+-Ti4+. Checking the stored band peaks: 800 x 1.178e17 x sigma(560 nm, E perp c) = 800 x 1.178e17 x 1.62e-19 = 15.3, and the o-ray bands of the Ruby entry sum to about 16 at 560 nm (my arithmetic from the stored centres, widths and peaks); 10 x 1.178e17 x 1.98e-18 = 2.33 against the Sapphire o-ray sum of about 2.3 at 585 nm. So **band peak in model units = sigma [cm^2] x 1.178e17 x D [ppma*cm]**, with D = bulk ppma x 0.88. Recipes below are given as D and as bulk ppma (= D / 0.88). For non-corundum hosts the equivalent statement is peak = alpha [cm^-1] x 0.88 cm (inferred, same reasoning; the existing Spinel peak 2.4 is alpha = 2.7 cm^-1 over 0.88 cm).

### 0.3 Colorimetry used for the targets

`color::body_color`'s method re-implemented outside the repo (T = exp(-alpha L), CIE 1931 2 deg 5 nm table and D65 10 nm table read from `cie1931.rs` / `spectral.rs` as data, 1 nm integration 380-780, Lab against the illuminant's own white, A = Planckian 2856 K). Cross-checks: GIA Appendix 2 Cr3+ 100 ppma*cm E perp c D65 is 70.46/37.74/-23.77, mine 69.0/38.8/-24.2; h.-Cr3+ 3 ppma*cm 69.58/43.49/102, mine 70.9/42.4/101.8; the repo's own Sapphire swatch (Fe-Ti 10, L*57 C*40 h283) is reproduced as 57/39/282. The two Appendix 2 checks in this paragraph are L*a*b*; **everywhere else a three-number color is L*/C*/h(deg)** (lightness / chroma / hue angle) and is `derived` unless a GIA Appendix 2 value is cited, and cited Appendix 2 values are always labelled L*a*b*. "mix" = (2 T_o + T_e)/3 (the repo's unpolarised rule; GIA's "both" column also fits this best). Caveat found: the Appendix 2 colors of the **Fe2+-Ti4+** chromophore are about 2.4x darker than the xlsx cross-section gives (L* 58.5 at "3 ppma*cm" needs ~2.4x); the paper itself says natural high-iron stones can show 2-3x larger Fe-Ti cross sections. So Fe-Ti targets here use the xlsx sigma, which is also what the repo's Sapphire uses.

### 0.4 Data produced (all in `crates/pleochroma/data/reference_spectra/`)

| File | What | Quality |
|---|---|---|
| `gia_dubinsky2020_corundum_six_chromophores_sigma_1nm.csv` | GIA cross sections sigma(lambda), E perp c and E par c, for Cr3+, V3+, Fe2+-Ti4+, h.-Fe3+, h.-Cr3+, Fe3+; 200-1100 nm, 1 nm (the full sheet; the older digest files held only 5 nm of some columns) | measured, reflection-corrected, SIMS-calibrated |
| `variant_sapphire_purple_grr3614_eperpc_6p26mm.csv`, `..._eparc_6p26mm.csv` | Caltech GRR 3614 violet sapphire, Colombia, both rays | measured, baseline-subtracted (slightly negative) |
| `variant_sapphire_blue_grr3615_eperpc_4p44mm.csv`, `variant_sapphire_blue_grr3615_eparc.csv` | Caltech GRR 3615 light blue sapphire, Colombia; E par c path not stated | measured, pedestal 0.3-0.6 A |
| `variant_ruby_thai_grr1843_eperpc_0p758mm_27c.csv` | Caltech GRR 1843 Thai ruby, the clean 27 C file, E perp c only | measured |
| `variant_spinel_red_jq13_1p52mm.csv` | Caltech JQ-13 red spinel (the one behind the existing Spinel preset) | measured, baseline-corrected |
| `variant_spinel_cobalt_blue_grr3017_1p435mm.csv`, `variant_spinel_cobalt_blue_grr2381_0p849mm.csv` | Caltech cobalt-blue spinels, Vietnam and Baffin Island | measured |
| `variant_spinel_lavender_jq7_0p711mm.csv` | Caltech JQ-7 pale lavender spinel, with its oxide analysis | measured, very weak |
| `variant_spinel_pale_blue_grr2224_2p986mm.csv` | Caltech GRR 2224 pale blue spinel | measured, saturates below 352 nm |
| `variant_chrysoberyl_yellow_grr171_beta_1p193mm.csv`, `..._gamma_1p193mm.csv` | Caltech GRR 171 greenish-yellow chrysoberyl (Fe3+), beta and gamma rays | measured, no alpha exists |
| `variant_alexandrite_grr874_gamma_0p717mm.csv` | Caltech GRR 874 alexandrite gamma ray at full resolution (beta is `caltech_chrysoberyl_grr874_beta_ch874a_0p717mm.csv`) | measured, composition unknown |

Headers follow `caltech_aquamarine_grr1488_eperpc_5p531mm.csv`; Caltech data: CC BY-NC 4.0, attribution G. R. Rossman, download date 2026-10-27, values unchanged, sorted, duplicate wavelengths dropped. **License caveat for the GIA file:** the GIA download page states no license (G&G supplementary data, (c) GIA); the owner must confirm redistribution before release (the repo already ships two GIA-derived digests without a license line).

### 0.5 Existing repo spectra that should not be used as measurements

`alexandrite_natural_russian.csv`, `sapphire_s_met_metamorphic.csv`, `sapphire_s_bas_basaltic.csv`, `sapphire2_moderate_4mm.csv`, `ruby_r1/r2/r3_*.csv`, `ruby2_synthesised_0p25wt.csv`, `corundum_v_0p26wt_v2o3.csv`, `tanzanite_*` and `amethyst_*` carry the header "user-delivered, secondary, unverified" or "synthesised by the report itself": they come from an unsourced research report, not from instruments. The alexandrite one also has its axes mislabelled (0.1). Also: the code comment of the Sapphire entry cites "A.J. Emmett, M. Dubinsky, R. Hughes & M. Scarratt, 'The Colors of Sapphires,' G&G 56(1), Spring 2020"; no such paper exists. The quoted 580 / 700 nm band positions are in the real source, **Dubinsky, Stone-Sundberg & Emmett 2020, "A Quantitative Description of the Causes of Color in Corundum", G&G 56(1) 2-28**, https://www.gia.edu/gems-gemology/spring-2020-corundum-chromophores (the xlsx gives E par c maximum at 704 nm). The citation in the comment should be corrected by the lead.

### 0.6 Sources read (full text unless stated)

1. Dubinsky, Stone-Sundberg & Emmett 2020, G&G 56(1): PDF https://www.gia.edu/dam/migrated-assets/docs/doc1/sp20-corundum-chromophores.pdf; cross-section xlsx https://www.gia.edu/dam/migrated-assets/docs/doc1/sp20-corundum-chromophores-absorption-cross-section-data.xlsx; Appendix 1 (samples) https://www.gia.edu/dam/migrated-assets/docs/doc1/sp20-corundum-chromophores-appendix1.xlsx; Appendix 2 (L*a*b*) https://www.gia.edu/dam/migrated-assets/docs/doc1/sp20-corundum-chromophores-appendix2.xlsx.
2. Emmett, Atikarnsakul, Stone-Sundberg, Sangsawong et al., "Chromophores in yellow sapphire", G&G 59(3) Fall 2023: https://www.gia.edu/gems-gemology/fall-2023-yellow-sapphire-chromophores (PDF https://cms.gia.edu/dam/jcr:103a4067-aef2-4b20-97b7-33a440eaf6bf/fall-2023-yellow-sapphire-chromophores.pdf).
3. GIA Winter 2019 origin papers, Tables of trace elements in ppma / ppmw: ruby https://www.gia.edu/doc/WN19-ruby-geographic-origin-determination.pdf, blue sapphire https://www.gia.edu/doc/WN19-blue-sapphire-geographic-origin-determination.pdf, alexandrite https://www.gia.edu/doc/WN19-alexandrite-geographic-origin-determination.pdf.
4. Belley & Palke, "Comparison of two purple spinels from Vietnam and Afghanistan", G&G 57(3) Fall 2021: https://www.gia.edu/doc/fall-2021-purple-gem-spinel-vietnam-afghanistan.pdf.
5. Shigley & Stockton, "Cobalt-blue gem spinels", G&G 20(1) 1984: https://www.gia.edu/doc/Cobalt-Blue-Gem-Spinels.pdf.
6. Farrell & Newnham 1965, Am. Mineral. 50, 1972 (figures only, OCR text; bands and statements read): https://msaweb.org/AmMin/AM50/AM50_1972.pdf. Hassan & El-Rakhawy 1974, Am. Mineral. 59, 159 (c-axis and perpendicular only, no numbers used): https://msaweb.org/AmMin/AM59/AM59_159.pdf.
7. Caltech Mineral Spectroscopy Server index pages and ASCII files: https://minerals.gps.caltech.edu/FILES/Visible/corundum/Index.html, .../spinel/Index.html, .../CHRYSOBeryl/Index.html (http://minerals.gps.caltech.edu/FILES/Visible/... for the files).
8. **Search-snippet only, not opened (treat as unverified):** Sun et al. 2017, Am. Mineral. 102, "How to facet gem-quality chrysoberyl" (3-axis spectra and colorimetry of synthetic Cr chrysoberyl; paywalled, https://www.degruyterbrill.com/document/doi/10.2138/am-2017-6011/html: **blocked**, best lead for the alexandrite alpha ray); GIA Fall 2014 abstract, alexandrite Cr2O3 typically 0.4-0.7 wt% (https://my.gia.edu/gems-gemology/fall-2014-abstract-red-chrysoberyl-versus-alexandrite-elevated-chromium-levels-color-change-properties); Geoscience (geoscience.net.cn) 2005, vol. 19 no. 3 p. 355 (title not seen), 15 chrysoberyl/alexandrite samples, average Fe 17,800 ppm (https://www.geoscience.net.cn/EN/Y2005/V19/I3/355); Acta Photonica Sinica review table of alexandrite pump-band cross sections (https://www.researching.cn/articles/OJ6f5b29cd6ce18d9f/figureandtable); LMHC padparadscha definition via https://gem-a.com/wp-content/uploads/2024/01/8.-JoG2018_36_4-Vol-36-4.pdf; Thai ruby EPMA (Geoscience 2023, https://www.geoscience.net.cn/EN/Y2023/V37/I02/486: Cr2O3 0.404-0.484 wt%, FeO 0.382-0.571 wt%, two stones); Gubelin color terms https://www.gubelingemlab.com/en/gemstonecolourterms (pigeon's blood is defined by master stones, no numbers); Schmetzer et al. 1989, N. Jb. Miner. Abh. 160, 107 (spinel color-line study, abstract only).

## 1. Summary table

D = ppma*cm over the 8.8 mm face-up path. Band counts are per ray after merging (limit 8). "Data" is the quality of the **spectral shape**; "target" quality is in the sections.

| Material / variant | Cause | Data quality | CSV files | Bands o/e (/beta) |
|---|---|---|---|---|
| Sapphire / Blue (Material Default) | Fe2+-Ti4+ IVCT | measured (GIA sigma, both rays) | gia_..._sigma_1nm; existing caltech_sapphire_grr1020a_* | 3/3 (existing) |
| Sapphire / Cornflower | lighter Fe2+-Ti4+ (D 6); Kashmir "velvet" is scattering, not absorption | measured shape, strength estimated | same | 3/3 |
| Sapphire / Yellow | h.-Fe3+ (1.5 D), optionally + Fe3+ | measured (GIA) | gia_... | 1/1 (+3/2 optional Fe3+) |
| Sapphire / Pink | Cr3+ (D 100) | measured (GIA) | gia_... | 3/3 |
| Sapphire / Padparadscha | Cr3+ + h.-Cr3+ (D 50 + 0.6) | components measured, combination estimated | gia_... | 6/6 |
| Sapphire / Purple | Cr3+ + Fe2+-Ti4+ (D 80 + 3) | components measured + Caltech GRR 3614 | gia_...; variant_sapphire_purple_* | 6/6 |
| Sapphire / Green | Fe2+-Ti4+ + Fe3+ (D 7 + 1580) | components measured, combination estimated | gia_... | 6/5 |
| Sapphire / Orange | h.-Cr3+ + h.-Fe3+ (D 1.2 + 0.5) | components measured | gia_... | 4/4 |
| Sapphire / Colorless | trace Fe3+ edge only | estimated | gia_... | 3/2 |
| Ruby / Pigeon's Blood | Cr3+ only, Fe-poor (D 1000) | measured (GIA) | gia_... | 3/3 |
| Ruby / Thai | Cr3+ + Fe3+ + h.-Cr3+ (D 800 + 960 + 0.3) | partly measured (E perp c GRR 1843; E par c from GIA) | variant_ruby_thai_*; gia_... | 8/8 |
| Ruby / Pinkish Red | Cr3+ (D 300) | measured (GIA) | gia_... | 3/3 |
| Spinel / Red (Material Default) | Cr3+ | measured (JQ-13) | variant_spinel_red_jq13 | 2 |
| Spinel / Pink | Cr3+, 0.35x red density | measured shape, strength derived | variant_spinel_red_jq13 | 2 |
| Spinel / Cobalt Blue | tetrahedral Co2+ + weak Fe | measured (2 stones) | variant_spinel_cobalt_blue_* | 6 |
| Spinel / Lavender | Fe2+ (T) with minor Cr3+, Fe3+ | measured but only pale grey-violet | variant_spinel_lavender_jq7; variant_spinel_pale_blue_grr2224 | 4 |
| Spinel / Purple (added) | Cr3+ + Co2+ equal weight | derived from the two measured sets | red + cobalt files | 7 |
| Spinel / Grey | Fe2+, Fe3+ IVCT | **estimated; recommend drop** | none | 4 |
| Chrysoberyl (Yellow) / Yellow | Fe3+ | partly measured (beta, gamma; alpha estimated) | variant_chrysoberyl_yellow_grr171_* | 3/3/3 |
| Chrysoberyl (Yellow) / Greenish Yellow | Fe3+, lighter | partly measured | same | 3/3/3 |
| Chrysoberyl (Yellow) / Honey | Fe3+, deeper, brown shift | **estimated; recommend drop** | same | 3/3/3 |
| Alexandrite / Strong Change | Cr3+ (4T2, 4T1) | partly measured (beta, gamma; alpha estimated) | caltech_chrysoberyl_grr874_beta; variant_alexandrite_grr874_gamma | 3/3/2 (+Fe3+) |
| Alexandrite / Brazilian | Cr3+ + more Fe3+ | partly measured, strength from GIA ppmw | same + grr171 | 5/5/4 |
| Alexandrite / Sri Lankan | weak Cr3+ + Fe3+ | partly measured, strength from GIA ppmw | same + grr171 | 5/5/4 |

## 2. Sapphire (corundum) and Ruby: shared data

### 2.1 Chromophore cross sections (GIA 2020, `gia_dubinsky2020_..._sigma_1nm.csv`)

Natural corundum color is the sum of six chromophores (GIA): Cr3+, V3+, Fe3+, Fe2+-Ti4+ IVCT, h.-Fe3+, h.-Cr3+ (h. = trapped hole on O2-, needs Mg or Be acceptor excess over Ti/Si donors, pairs with Cr3+ in preference to Fe3+). Absorbance is additive: alpha = sum sigma_i N_i. Strength differs by more than 1000: GIA Figure 16 gives the ppma that produce "similar saturation" at 1 cm, E perp c, D65: Fe3+ 2960, V3+ 400, Cr3+ 100, Fe2+-Ti4+ 3, h.-Fe3+ 3, h.-Cr3+ 2.

Peak cross sections, 1e-20 cm^2 (xlsx; "perp" = E perp c, "par" = E par c; all read from the CSV, not fitted):

| Chromophore | Band (perp) | Band (par) |
|---|---|---|
| Cr3+ | U 559 nm 16.2; Y 410 nm 16.5; R-line 693 nm 0.6 | U 546 nm 5.5; Y 398 nm 29.8 |
| V3+ | 578 nm 3.4; 399 nm 9.8 | 577 nm 2.8; 403 nm 45.7 |
| Fe2+-Ti4+ | 580 nm 198 (FWHM ~1900 cm^-1 above a continuum); 700 nm tail 142 | 704 nm 125.5; shoulder 530 nm ~64 |
| h.-Fe3+ | monotone UV edge: 2736 at 330, 914 at 450, 175 at 550, 53 at 600, 19 at 650 | 2857 at 330, 659 at 450, 97 at 550, 30 at 600 |
| h.-Cr3+ | 464 nm 1338 on a 1090 plateau (900 at 330, 1085 at 400); NIR broad 17.7 at 700 nm; CT edge 1.1e-16 cm^2 (1.1e4 in these units) at 200 nm | 453 nm 820 on a 770 plateau; NIR 21.6 at 750 nm; 800-1000 nm ~14 |
| Fe3+ | 346 nm 11.1; 377 nm 4.9; 451 nm 2.2 (FWHM ~1170 cm^-1); edge 1.1e-19 at 330 | 376 nm 4.5; 452 nm 1.9 |

Polarisation (GIA text, read): Cr3+ and Fe2+-Ti4+ are strongly dichroic (Fe-Ti: o-ray more effective at blocking blue-green, "transmitting a purer blue"; e-ray greener); h.-Fe3+ "Ec and E||c spectra are quite similar", h.-Cr3+ "exhibits little dichroism", Fe3+ weak and nearly isotropic.

### 2.2 Starting band sets per chromophore (energy Gaussians; `estimated start values`, the fit lane refits)

Peak is given as **model-unit peak per 1 ppma*cm** (sigma x 1.178e17); FWHM in cm^-1 (sigma = FWHM / 2.3548).

* **Cr3+** (already fitted in the repo Ruby entry at D = 800; divide the stored peaks by 800): o: (562 nm, FWHM 1493, 0.01363), (410, 2767, 0.01975), (537, 2688, 0.008825); e: (553, 1721, 0.004225), (397, 3445, 0.033875), (525, 2303, 0.0032). Landmarks from the CSV for the check: U 559/546 nm FWHM ~2000 cm^-1 (half-height above the local floor), Y 410/398 nm FWHM ~3100/2900.
* **Fe2+-Ti4+** (already fitted in the repo Sapphire entry at D = 10; divide by 10): o: (585 nm, 4878, 0.201), (744, 3142, 0.0887), (457, 12000, 0.0433); e: (612, 7466, 0.0371), (729, 4955, 0.103), (467, 11169, 0.0524).
* **h.-Fe3+** (new; one band through the data points at 340/450/550 nm, hand arithmetic, not a fit): o: centre 345 nm (28,960 cm^-1), FWHM 10,900 cm^-1, sigma 2.64e-17 -> **3.11**; e: 335 nm, FWHM 10,600, 2.75e-17 -> **3.24**. Reproduces 450 and 550 nm exactly, 400 nm +30 %, 600-650 nm +50-150 % (the model tail is too fat; a second narrow band or an exponential tail would improve it).
* **h.-Cr3+** (new, landmarks): o: (465 nm, FWHM ~4900 cm^-1 [from the 525 nm half-height on the red side], 1.34e-17 -> **1.58**), (380 nm UV plateau band, FWHM ~9000, 1.0e-17 -> 1.18), (710 nm, FWHM ~6000, 1.8e-19 -> 0.021); e: (455 nm, FWHM ~5000, 8.2e-18 -> 0.97), (340 nm, FWHM ~8000, 4.5e-18 -> 0.53), (750 nm, FWHM ~5000, 2.2e-19 -> 0.026). Plateau and widths are the least certain numbers; fit against columns `hCr3_perp` / `hCr3_par`.
* **Fe3+** (net of local floor): o: (346 nm, FWHM 1813, 4.5e-20 -> 0.0053), (377, 1036, 1.9e-20 -> 0.0022), (451, 1170, 1.6e-20 -> 0.0019); e: (376, 635, 1.5e-20 -> 0.0018), (452, 1073, 1.4e-20 -> 0.0016); the broad UV edge (1e-19 at 330 nm) is the first band. Fall 2023 paper: the 450 nm cross section is not constant; it is ~constant below ~700 ppma Fe and rises to ~4.5x at 4730 ppma. The xlsx column is one specific stone.

### 2.3 Concentration anchors (GIA, ppma of all atoms)

* GIA 2020: red needs "several hundred to a few thousand ppma" Cr3+; Cr3+ ~20-40 ppma with h.-Cr3+ already shifts to orange; a few ppma h.-Fe3+ give strong yellow, ~2000-3000 ppma Fe3+ alone are needed for a similar yellow.
* Marble-hosted ruby, Mogok (GIA W2019 Table 1, ppma, range / mean): Mg 5-75 / 40, Ti 7-80 / 45, V 48-1089 / 212, **Fe bdl-301 / 35**, Ga 2-51 / 18. High-iron ruby, **Thailand** (Table 2): Mg 126-181 / 143, Ti 36-89 / 54, V 4-14 / 8, **Fe 756-1442 / 1089**; Cambodia Fe 1317 mean; Mozambique Fe 622; Madagascar Fe 933.
* Metamorphic blue sapphire (GIA W2019 Table 1): Myanmar Ti 53 / Fe 928 / Mg 41 ppma means; Sri Lanka Ti 133 (median 66) / Fe 330 / Mg 42; Madagascar Ti 128 / Fe 598 / Mg 30. A Kashmir reference stone (Table 3, example 3): Mg 25, Ti 104, Fe 305, V 3, Ga 18 ppma.
* Yellow sapphire (Fall 2023): Fe 4-4750 ppma in nature, only 2000-4750 contributes significantly; samples with 3.2 ppma h.-Fe3+ (Bang Kha Cha 940) are "well saturated" at 1.43 mm; 0.25 ppma already erases the 495 nm minimum.

### 2.4 Recipes and derived face-up colors (D65 unless noted; L*/C*/h; o = Along, e = Across, mix = (2o + e)/3)

Computed from the GIA sigma columns at the stated D. "A" = illuminant A. `derived` for every row.

| Variant | Recipe D (ppma*cm) = bulk ppma | o | e | mix D65 | mix A |
|---|---|---|---|---|---|
| Blue (MD) | Fe-Ti 10 = 11 | 47/59/292 | 70/16/246 | 57/39/282 | 54/41/261 |
| Cornflower | Fe-Ti 6 = 6.8 | 64/41/286 | 81/11/244 | 70/29/279 | 69/30/261 |
| (optional Deep Blue) | Fe-Ti 15 = 17 | 32/74/299 | 59/21/250 | 44/47/287 | 41/48/262 |
| Yellow | h.-Fe3+ 1.5 = 1.7 | 88/57/86 | 93/47/93 | 90/53/88 | 92/55/78 |
| Yellow alt (basalt) | h.-Fe3+ 1.0 + Fe3+ 880 = 1.1 + 1000 | 83/67/91 | 90/64/96 | 86/66/93 | 88/64/81 |
| Pink | Cr 100 = 114 | 69/46/328 | 87/15/31 | 76/31/337 | 78/36/341 |
| Padparadscha | Cr 50 + h.-Cr3+ 0.6 = 57 + 0.7 | 74/36/31 | 87/29/64 | 78/32/42 | 82/39/40 |
| Purple | Cr 80 + Fe-Ti 3 = 91 + 3.4 | 59/46/310 | 80/6/19 | 67/28/312 | 68/30/313 |
| Green | Fe-Ti 7 + Fe3+ 1580 = 8 + 1800 | 48/18/136 | 71/46/116 | 57/29/123 | 57/21/106 |
| Orange | h.-Cr3+ 1.2 + h.-Fe3+ 0.5 = 1.4 + 0.6 | 81/76/73 | 86/54/77 | 83/67/74 | 87/74/70 |
| Colorless | Fe3+ 26 = 30 | 100/2/111 | 100/2/111 | 100/2/111 | 100/2/92 |
| Ruby MD (existing) | Cr 800 = 909 | 31/68/346 | 49/66/10 | 38/65/356 | 45/68/4 |
| Pigeon's Blood | Cr 1000 = 1136 | 27/65/351 | 45/70/13 | 34/65/1 | 42/68/9 |
| Thai | Cr 800 + h.-Cr3+ 0.3 + Fe3+ 960 = 909 + 0.34 + 1089 | 26/64/14 | 46/74/28 | 34/66/22 | 42/73/27 |
| Pinkish Red | Cr 300 = 341 | 46/72/334 | 69/40/10 | 56/55/343 | 60/59/349 |

Published colorimetry (GIA Appendix 2, **calculated** not measured, all values L*a*b*, 'both' = unpolarised, D65; no measured color exists for any single-chromophore sample, confirmed in `research-2026-10-primary-data.md`; for comparison with the table convert a*b* to C*h, e.g. Cr 100 'both' = 78/28/339, our mix 76/31/337): Cr3+ 100 ppma*cm 78.05/25.96/-9.92; 200: 64.34/44.64/-16.54; 375: 50.00/62.74/-18.87; 750: 38.67/70.10/-9.70; 1500: 29.65/68.84/9.75. h.-Cr3+ 1.6: 82.06/20.66/61.42; 3: 71.60/38.06/92.02. h.-Fe3+ 1.7: 88.42/1.58/60.78; 3: 79.79/13.74/89.57. Fe3+ 2960 ppma at 1 cm (Ec): 87.54/-10.73/63.62. Fe-Ti: use with the 2.4x caveat of 0.3.

### 2.5 Sapphire variants

**Blue (Material Default).** Cause Fe2+-Ti4+ IVCT: edge-sharing basal pairs give the E perp c band (580 nm), face-sharing pairs along c the E par c band (704 nm). Pleochroism (GIA text + derived): o violetish blue (47/59/292 at D 10), e greenish blue/greyer (70/16/246). Spectra: GIA sigma (CSV) and the existing Caltech GRR 1020a (`caltech_sapphire_grr1020a_eperpc/eparc_4p289mm.csv`, medium blue, Fe2+-Fe3+ 880 nm maximum A 2.66). Band model: existing 3/3 (section 2.2). Target: the repo's own swatch, 57/40/283 at 7 mm; reproduced from the GIA sigma at D 10. Anchor: Fe-Ti pairs of only ~3 ppma already give GIA's "similar saturation" at 1 cm; real stones hold 5-100 ppma Ti but only a small fraction forms active Fe-Ti pairs (Ti_active = Ti - Mg). Confidence: **measured**. Missing: a measured finished-stone color.

**Cornflower / Kashmir.** Same chromophore as Blue at lower pair density: D 6 gives 70/29/279 mix (medium-light violetish blue, hue stays 279-286 with C* 29-41). Pleochroism as Blue. "Velvety/sleepy" Kashmir appearance comes from sub-micron rutile/TiO2 silk and scattering, **not** from a different absorber; Kashmir chemistry (GIA W2019 Table 3 example 3: Ti 104, Fe 305, Mg 25 ppma, V 3) is ordinary metamorphic blue. So a band model cannot make "Kashmir" different from "Cornflower"; keep one variant named **Cornflower** (optionally Deep Blue, D 15) and let a Scattering preset handle the silk. Target: estimated verbal ("medium-light, vivid violetish blue"; no published colorimetry found). Confidence: **measured shape, strength estimated**.

**Yellow.** Cause: h.-Fe3+ pair (Mg/Be acceptor excess, oxidising anneal), the main cause of Sri Lankan and many Montana yellows; pure Fe3+ yellow (450/388/377 nm) needs thousands of ppma and gives the greenish-yellow Montana/Subera look. Pleochroism: negligible (GIA); derived o 88/57/86 vs e 93/47/93. Band model: h.-Fe3+ one band per ray (3.11 / 3.24 per ppma*cm) + optionally Fe3+ (3/2 bands) for the basalt type. Target: GIA Appendix 2, calculated, L*a*b* (h.-Fe3+ 1.7 ppma*cm Ec D65 86.55/4.62/65.25, 'both' 88.42/1.58/60.78, i.e. L*/C*/h 88/61/88); recipe D 1.5 gives 90/53/88 (mix, D65, our CMFs). Confidence: **measured**.

**Pink.** Cause Cr3+ (low density); same bands as ruby. Pleochroism: o purplish pink (328 deg), e orange-pink (31 deg), derived. Recipe Cr 100 ppma*cm = 114 ppma; GIA 'both' at 100 (L*a*b*): 78.05/25.96/-9.92. Confidence: **measured**.

**Padparadscha.** Cause: h.-Cr3+ pair (orange) plus Cr3+ (pink) in low-iron acceptor-dominated corundum (GIA 2020; Fall 2023 note 3: trapped holes pair with Cr3+ in preference to Fe3+). The trade/LMHC definition (2007, via JoG 2018; secondary): low-to-medium saturation, pinkish orange to orangey pink, light-to-medium tone. Pleochroism: small (hue 31 deg vs 64 deg derived). Recipe Cr 50 + h.-Cr3+ 0.6 ppma*cm: mix 78/32/42, D65, C* 32 = low-medium saturation, light tone; the effect of ~0.7 ppma h.-Cr is as large as 57 ppma Cr, which is why the stones are rare. Raising h.-Cr3+ goes orange (Orange row), raising Cr goes pink. Band model: Cr (3) + h.-Cr3+ (3) = 6/6. No spectrum of a measured padparadscha with chemistry was found (GIA reports are origin/stability studies without cross sections); the combination is therefore `estimated`, the components `measured`. Target: verbal LMHC definition plus our derived 78/32/42.

**Purple.** Cause: Cr3+ plus Fe2+-Ti4+ (GIA). Recipe Cr 80 + Fe-Ti 3 ppma*cm: mix 67/28/312 (violet-purple), o 59/46/310, e nearly neutral (80/6/19): strong dichroism, purple one way and pale pinkish the other. Caltech GRR 3614 (violet, Colombia) confirms the pattern: o-ray bands 387 nm (Fe3+), 450 nm, 563 nm Cr3+ U band (alpha 1.7 cm^-1 above the local floor -> Cr ~ 90 ppma if sigma(560)=1.6e-19), e-ray only the 551 nm Cr band (alpha 0.54 cm^-1 -> ~85 ppma), consistent with Cr ~ 90 ppma. The GRR 3614 files have negative baselines (min -0.25 A), so they are shape only. Band model Cr (3) + Fe-Ti (3) = 6/6. Confidence: **measured components + a measured stone, combination derived**.

**Green.** Cause: Fe2+-Ti4+ (blue) plus Fe3+/h.-Fe3+ (yellow), basalt-hosted Australian, Thai, Montana stones (GIA 2020: gem corundum is usually colored by a combination of two or three chromophores; Caltech corundum page: "Green sapphires contain a mixture of the blue and green colors"). Recipe Fe-Ti 7 + Fe3+ 1800 ppma: mix 57/29/123, but o 48/18/136 vs e 71/46/116: bluish-green one way, yellow-green the other; Montana GRR 1930-2/-5 (pale greenish-yellow / light greenish-blue, distinct dichroism) exist only as images. Band model Fe-Ti (3) + Fe3+ (3/2) = 6/5. Confidence: **components measured, combination estimated** (concentrations are typical basalt values, not from one analysed stone).

**Orange.** h.-Cr3+ dominated (GIA: pure orange needs Cr3+ only 20-40 ppma to shift), Mekong-whisky variants add Fe3+. Recipe h.-Cr3+ 1.2 + h.-Fe3+ 0.5 ppma*cm: mix 83/67/74, C* 67 (strong). Pleochroism little. Band model h.-Cr (3) + h.-Fe (1) = 4/4. Confidence: **components measured**.

**Colorless.** No chromophore; keep a trace Fe3+ (30 ppma) so the tensor is non-empty (validate_for(Empty) would reject an empty set): C* 2. `estimated`.

### 2.6 Ruby variants

Cause for all: Cr3+ (U band 559 nm, Y band 410 nm, transmission window near 474 nm, sharp R lines 692.9/694.3 nm are below sampling). Pleochroism (derived from GIA sigma): o purplish red (hue 346-353), e orange-red (hue 10-14).

**Pigeon's Blood** (Mogok/marble-hosted: Fe mean 35 ppma, i.e. essentially Cr only; Gubelin defines it by master stones, no numbers; trade lore: vivid saturated red, medium to medium-dark, fluorescence adds to the glow). Recipe Cr 1000 ppma*cm (1136 ppma): mix 34/65/1 (hue 1 deg = pure red; the existing Ruby at D 800 sits at hue 356). Band model: existing Cr bands x 1000 (3/3). Target: the Cr-only GIA Appendix 2 colors (L*a*b*, 'both') bracket it (750: 38.7/70.1/-9.7 -> 1500: 29.7/68.8/9.8; b* crosses 0 at ~1100). Because the Material Default (D 800) is already Cr only and differs by 0.2 L* units, **Pigeon's Blood adds little; keep it only as the "pure red, slightly deeper" entry**. Confidence: **measured**.

**Thai** (high-iron: Fe mean 1089 ppma, Mg 143, Ti 54; Cr2O3 0.40-0.48 wt% in two EPMA stones). Caltech GRR 1843 (Chanthaburi, 0.758 mm, E perp c; `variant_ruby_thai_grr1843_eperpc_0p758mm_27c.csv`): Y 414 nm alpha 47 cm^-1, U 562 nm alpha 42 cm^-1 above the floor, alpha at the 474 nm window minimum is **0.114** of the U band (Cr-only GIA sigma gives 0.086): only +1.2 cm^-1 of excess window absorption, equal to ~0.75 ppma h.-Cr3+ or ~15 ppma Fe2+-Ti4+ pairs; at 450 nm the excess is 8 cm^-1 (Y-band flank mismatch, Fe3+ and hole centres; the stone has no analysis, all derived from the GIA sigma scaled to the 562 nm band). So this Thai ruby is Cr3+ plus a modest hole/iron contribution, not a different absorber. Implied Cr (alpha_U / sigma(562)) = 2240 ppma, 0.83 wt% Cr2O3-equivalent (derived; higher than the two EPMA stones because this is a thin, deep piece). Recipe for the face-up variant (h.-Cr3+ 0.3 ppma*cm is inside the 0-0.75 range the GRR 1843 window allows, Fe3+ = Thailand mean 1089 ppma): Cr 800 + h.-Cr3+ 0.3 + Fe3+ 960 ppma*cm: mix 34/66/22 (hue +21 deg vs pigeon's blood: slightly orangier and darker, as described for Thai stones). E par c: not measured on any Thai ruby; take the GIA e-ray columns. Band model 8/8: Cr 3 + h.-Cr 3 + Fe3+ 376/452 nm 2 (drop the 346 nm Fe3+ band). Confidence: **partly measured** (E perp c shape measured, strengths and e-ray derived).

**Pinkish Red.** Cr 300 ppma*cm (341 ppma): mix 56/55/343 (D65); GIA 'both' (L*a*b*) at 375: 50.0/62.7/-18.9 and at 200: 64.3/44.6/-16.5 bracket it. Confidence: **measured**.

## 3. Spinel (MgAl2O4, isotropic)

Convention: ppma of all atoms in spinel = ppmw x 142.27 / (7 x atomic weight) (Belley & Palke), 1 ppma = 1.06e17 cm^-3 (derived, rho 3.58). Measured alphas are decadic A x 2.303 / t, baseline subtracted as stated; all targets are `derived` from measured spectra at the face-up path 0.88 cm.

**Red (Material Default)** Cr3+ (octahedral). Spectrum `variant_spinel_red_jq13_1p52mm.csv`: 391.3 nm (alpha 19.0 cm^-1 over a 9.6 floor, FWHM ~3170 cm^-1), 537.4 nm (11.5 over 4.7, FWHM ~2020 cm^-1; the repo calibration record quotes 77 nm = 2650 cm^-1 on a different baseline), R complex 684 nm (A 0.07). Band model: 2 energy bands (537 nm and 391 nm, ratio 1.4-2.1) + optional UV edge. Anchor: GIA reference vivid pink spinel Cr 1423 ppma (Fe 46, V 70), Belley & Palke Table 2; sigma(537) ~7e-20 cm^2 (Report-1 secondary; JQ-13 is unanalysed). Face-up color: the existing MD (peak 2.4 -> alpha 2.7 cm^-1) = L* ~59; at alpha(537) = 3 cm^-1 -> 56/48/8, 4 -> 48/54/10, i.e. a medium red. Confidence: **measured**.

**Pink.** Same tensor at ~0.35x: alpha(537) 1.5 cm^-1 -> 73/31/4; 1.0 -> 81/22/3 (pale pink). Not a different chromophore: GIA's vivid pink and the red differ in Cr and Fe (red often has more Fe, darker). Confidence: **measured shape, strength derived**.

**Cobalt Blue.** Cause: Co2+ on tetrahedral Mg sites: 4A2 -> 4T1(P) triplet 551 / 594 / 623 nm, plus weak Fe-related 450 and 478 nm and a UV Fe/Co edge. Spectra: GRR 3017/3027 Vietnam (A 1.0 at 594 nm, 1.435 mm) and GRR 2381 Baffin (0.849 mm), triplet net alpha (3017): 9.1 / 14.7 / 13.2 cm^-1 (ratio 0.62 : 1 : 0.90), 450 nm 0.74, 478 nm 0.71, UV 336/357 nm 3.3/2.8 cm^-1; GRR 2381 has 594 nm 22 cm^-1 and a 386 nm band 4.8 cm^-1. Individual triplet components have FWHM ~700 cm^-1 (Report-1, secondary; the blended 565-633 nm envelope is 1900 cm^-1). Band model 6: 551, 594, 623, 450, 478, UV edge. Anchors: GIA vivid-blue reference Co 197 ppma, Fe 2570, Ni 388, Cr 77 (Table 2); Shigley & Stockton: natural grey-blue spinel with 5.3 ppm (weight) Co and 1.43 wt% FeO; Co-rich blue spinels typically have Cr < 60 ppma; Co2+ has a much higher molar absorptivity than Cr3+ and Fe2+ (Belley & Palke; Shigley & Stockton). Target: GRR 3017 at 0.88 cm gives 36/75/286 (too dark); alpha(594) = 2 cm^-1 -> 72/44/258, 3 -> 63/55/262, GRR 3017 at 4 mm equivalent 48/71/275 (vivid medium-dark blue). Suggest the face-up default at alpha(594) ~ 4.5 cm^-1 (55/65/268; about 0.3x GRR 3017; the Co ppma behind it is not derivable because the Caltech stones are unanalysed). Confidence: **measured**; chemistry of the Caltech stones unknown.

**Lavender.** Cause: tetrahedral Fe2+ with minor Fe3+ (UV O->Fe edge, 387 nm) and Cr3+ (556 nm). JQ-7 (0.711 mm, FeO 0.731 wt%, Fe3+ 0.132 wt%, Cr2O3 0.050 wt%): bands 387 nm (alpha 2.8 cm^-1, FWHM 2340), 458 nm (1.0), 556 nm (1.1, FWHM 1600), 337 nm (0.85); the stone is nearly colorless (C* 0.5 at 0.7 mm). Scaled to extreme thickness it stays desaturated: 8.8 mm L* 60 C* 4.6 h307; x1.7 -> 40/6/306. A real vivid lavender/violet spinel is Cr + Fe, or Cr + Co (Belley & Palke): purple VN: Cr 593, Co 17.9, Fe 2437, V 85 ppma; AF (grey-violet, Fe only): Fe 3089, Cr/Co < 10 ppma ("medium tone, moderate saturation"). So **keep Lavender as the Fe2+-only "grey-lavender" variant** (JQ-7 shape at 6 mm of path, i.e. ~8x its measured 0.711 mm: 71/4/307) and add **Purple (Cr + Co)**: equal Cr and Co peak heights (alpha 1.2 cm^-1 each) -> 52/33/302; Co/Cr 0.6 -> 58/29/320 (reddish purple). Band model 7 (Cr 2 + Co 3 + Fe 2). Confidence: Lavender **measured but pale**, Purple **derived**.

**Grey.** Cause stated in the literature: Fe2+/Fe3+ (tetrahedral Fe2+ bands plus a broad IVCT tail); no spectrum with a grey color found (GRR 2224 pale blue-grey is the nearest: bands 457 and 557 nm, 625 nm; neutral grey needs a flat visible absorption that a few Gaussians cannot honestly give). **Recommend dropping Grey** or shipping it flagged `estimated` using the GRR 2224 shape.

## 4. Chrysoberyl (Yellow)

Cause: Fe3+ on the Al mirror site, spin-forbidden 6A1 -> 4T1(G)/4T2(G) very broad weak bands at 615 and 500 nm and sharper bands at 440, 385, 378, 368 and 355 nm (Farrell & Newnham 1965). F&N (77 K, Brazilian yellow chrysoberyl Al1.96Fe0.04BeO4): spectra "nearly identical in the three polarization directions, showing little or no pleochroism". Room-temperature Caltech GRR 171 (greenish-yellow, Minas Gerais, 1.193 mm): beta (E||a): 340, 377, 439 nm (439 nm net alpha 4.3 cm^-1, FWHM ~900 cm^-1) on a 0.26 A pedestal; gamma (E||b): 352, 369, 441 nm (441 nm net alpha 12.7, FWHM ~950; broad 368 nm 17.5). **The gamma 441 nm band is ~3x the beta one (net of floor) here, contradicting F&N**: treat the dichroism as unconfirmed and keep the three rays within a factor 3 (estimated: alpha = mean of beta and gamma). Colors of GRR 171 (derived, D65, face-up 8.8 mm x s): s 0.25: beta 93/21/103, gamma 89/47/97, mix 91/32/99; s 0.5: 86/34/99, 80/64/89, mix 83/46/93; s 1.0: 74/50/93, 65/77/79, mix 70/59/85.

Strength anchors (chemistry, none measured on GRR 171): Fe2O3 in yellow chrysoberyl ~2.5 wt% average (Earth Science Frontiers 2005, search snippet, 15 stones, 17,800 ppm Fe), F&N 0.04 apfu Fe (~2.5 wt% Fe2O3 by arithmetic); Report-1 (secondary): Fe3+ 440 nm 8 cm^-1 per wt% Fe2O3 (b), 375 nm edge 60 cm^-1 per wt%. By that coefficient GRR 171 gamma (12.7 cm^-1) is ~1.6 wt% Fe2O3 (derived, uncertain).

* **Yellow (MD).** s ~0.7 of GRR 171 (mix 77/53/89): saturated golden yellow. Band model per ray 3: (440 nm, FWHM ~950 cm^-1), (368 nm broad, ~3300), (340-350 nm). Alpha ray `estimated` (mean of beta/gamma). The current built-in is **isotropic** (one 440 nm band); the variant must be a 3-ray tensor.
* **Greenish Yellow.** s ~0.25-0.4 (mix 91/32/99 -> 86/42/95): the Caltech sample itself is labelled greenish-yellow. Hue 96-99 is the green side of yellow.
* **Honey.** Brown shift = more Fe3+ plus a tail; no spectrum found with composition (Crystals 2023 canary-yellow chrysoberyl study reports a 650-660 nm band, V- and Cr-poor, via search snippet). Scaling GRR 171 to s 1.6 gives mix 57/63/79 (s 2.0: 50/63/75), a deep golden yellow rather than brown; the brown comes from a broader edge that this data set does not show. **Recommend drop, or ship as `estimated`.**
Confidence: **partly measured** (beta, gamma); alpha and strengths estimated; no polarized alpha file exists on the Caltech server (confirmed).

## 5. Alexandrite

Cause: Cr3+ on both Al sites (mirror ~78 %, inversion ~22 %), 4A2 -> 4T2 (560-595 nm) and 4T1 (410-430 nm) windows straddling Neuhaus's 580/415 nm critical values -> green by daylight, red by incandescent light; Fe3+ yellows. Axes: beta = E||a (yellow), gamma = E||b (blue-green), alpha = E||c (red) (F&N, Caltech). Measured: GRR 874 (Lake Manyara, 0.717 mm; baseline ~0.11 beta / 0.155 gamma): beta 4T1 at 428.7 nm (alpha 61 cm^-1, FWHM ~2770 cm^-1), 4T2 564.7 nm (11.1, FWHM ~1750), UV rise (8 at 350, 18 at 380, 41 at 400 nm); gamma 4T2 596.9 nm (56.2, FWHM ~1680), 4T1 414.9 nm (35.2, FWHM ~2880), UV band 339.6 nm (18.0, FWHM ~1400), R-line 679 nm (9.7, FWHM ~100). alpha: **no measured file**; F&N did not manage c (flux platelets), the natural-stone c spectrum is in their Fig. 4 only (77 K); the repo's `repo figure-read` values are 4T2 net peaks a 6.5 : b 31 : c 19 cm^-1 and 4T1 a:b 25:10.5, c ~1.24 x b, giving alpha 4T2 565 nm, 4T1 420 nm. Take alpha = 4T2 at 565-572 nm with 0.6 x the gamma 4T2 peak, 4T1 at 415-420 nm with ~1.0 x the gamma 4T1 peak: `estimated`.

Color of GRR 874 (derived, baseline subtracted, viewing along c = (beta + gamma)/2): D65 L*/C*/h at 3 mm: 43/16/141; illuminant A 42/18/352; Delta E76 32; at 1.5 mm D65 59/28/147 -> A 57/4/23. Single rays at 0.717 mm: beta 82/44/104 (yellow), gamma 60/66/193 (blue-green): consistent with F&N's yellow/green pleochroism. The earlier research tables list alpha as bluish-green and gamma purplish red (wrong).

Strength anchors, GIA W2019 Table 1 (ppmw, mean, range): Russia Cr 3090 (156-14200), Fe 2780 (1390-8610), V 102; **Brazil Cr 2960 (424-6050), Fe 6350 (4180-11500), V 136**; **Sri Lanka Cr 702 (24-3200), Fe 7680 (1720-12200), V 211 (30-1190)**; Tanzania Cr 1990, Fe 5620, V 76. In wt% oxide (x1.4615 for Cr, x1.4297 for Fe): Russia Cr2O3 0.45 / Fe2O3 0.40, Brazil 0.43 / 0.91, Sri Lanka 0.10 / 1.10, Tanzania 0.29 / 0.80; GIA Fall 2014 abstract (snippet): alexandrite Cr2O3 typically 0.4-0.7 wt%. A review table (Acta Photonica Sinica, snippet only, unverified) lists the Cr:BeAl2O4 peak cross sections 1.9e-19 cm^2 (E||b, 595 nm) and 3.9e-20 (E||a, 550 nm): with 1.9e-19, GRR 874 gamma alpha 56 cm^-1 means N = 3.0e20 cm^-3 = **~1.0 wt% Cr2O3**, the top of GIA's Tanzanian range (derived; sigma unverified). So GRR 874 is a Cr-rich stone and variants scale it down by the GIA Cr ratios: s_Cr = 0.45 (Russian-type Strong Change), 0.43 (Brazilian), 0.10 (Sri Lankan).

Recipes (GRR 874 beta/gamma x s_Cr + GRR 171 x s_Fe, s_Fe from Fe2O3 relative to the ~1.8 wt% assumed for GRR 171; face-up 8.8 mm; D65 / A, c-view, derived; Delta E76 between illuminants):

| Variant | s_Cr, s_Fe | beta (D65) | gamma (D65) | c-view D65 | c-view A | Delta E76 |
|---|---|---|---|---|---|---|
| Strong Change (MD, Russian-type) | 0.45, 0.2 | 39/37/51 | 25/65/188 | 33/11/86 | 33/27/5 | 27 |
| Brazilian | 0.43, 0.5 | 36/39/49 | 18/49/185 | 29/18/61 | 30/31/19 | 22 |
| Sri Lankan (weaker, yellowish) | 0.10, 0.6 | 65/49/89 | 37/49/166 | 54/37/107 | 54/28/71 | 21 |

**Path warning:** at the 8.8 mm face-up path the GIA-mean Cr is too dark and the D65 hue has left green (Strong Change 33/11/86). The colour-change look needs s_Cr x path ~ 0.22 cm: for the 8.8 mm face-up path use s_Cr / s_Fe = 0.25 / 0.11 (Strong: c-view D65 48/21/131, A 47/15/15, Delta E76 30), 0.25 / 0.28 (Brazilian: 44/21/112, A 44/20/28, Delta E76 27), 0.06 / 0.35 (Sri Lankan: 67/39/120, A 67/24/89, Delta E76 22), all derived, i.e. the fit lane must pick the strength by the D65 / A target colors, not by the ppmw table alone (real alexandrites cut for color are small or thin). At 5 mm path Strong Change reads 47/20/130 (daylight, green) -> 46/15/15 (incandescent, red): Delta E76 30. Brazilian: yellower-green by day (113 deg), more orange-red by incandescent; Sri Lankan: yellow-green (107) to orange-brown (71), the weak/yellow change GIA describes. The c-view only uses beta and gamma; the alpha ray (estimated) enters side views, so alpha errors do not affect the table. Band model per ray: beta 3 (428.7, 564.7, UV 340-380 rise) + Fe3+ 2 = 5; gamma 3 (596.9, 414.9, 339.6) + 2 = 5; alpha 2 (estimated) + 2 = 4; the 679 nm R-line is below sampling and may be dropped. Confidence: **partly measured** (beta, gamma); strengths **derived from GIA ppmw with one unverified sigma**; alpha **estimated**.

## 6. Dropped, changed, added

* Sapphire: **Cornflower/Kashmir -> "Cornflower"** (Kashmir cannot be separated by absorption). Optional addition: Deep Blue (Royal) at D 15. Colorless kept as `estimated`.
* Ruby: **Pigeon's Blood barely differs from the Material Default** (Cr only at D 1000 vs 800); keep as the "pure red, deeper" entry or drop.
* Spinel: **Grey -> recommend drop** (no honest band model); **Lavender stays pale grey-lavender**; **Purple (Cr + Co) added**, the way GIA describes the best purple spinels; Pink kept as a lower-density Red.
* Chrysoberyl (Yellow): **Honey -> recommend drop or `estimated`**; also the built-in material is isotropic while the host is biaxial (variants must be 3-ray).
* Alexandrite: three variants kept; all depend on an `estimated` alpha ray and on one unverified cross section.

## 7. Gaps

* No polarized spectrum with analysed chemistry for padparadscha, orange, green, purple sapphire, Cornflower or any ruby type except the GIA single-chromophore sheets; Sun et al. 2017 (3-axis chrysoberyl, paywalled) and Palke/Emmett papers on padparadscha cross sections not accessible.
* No E par c spectrum of any Thai ruby; no alpha ray of any chrysoberyl or alexandrite (confirmed absent on the Caltech server).
* No measured finished-stone colors for any of these (GIA colors are calculated; confirmed by `research-2026-10-primary-data.md`).
* Caltech sample chemistry unknown for GRR 874, 171, 1843, 3017, 2381, 3614, 3615, JQ-13.
* The GIA Fe2+-Ti4+ xlsx sigma vs Appendix 2 discrepancy (2.4x) is unresolved; sapphire strength anchors depend on which is right.
