# Color variants research R4: quartz family, diamond, opal, synthetics, glass, rutile

Lane R4 of the color variants campaign (2026-10-27). Research only: no Rust code was written. Scope: Amethyst, Citrine, Quartz, Diamond, Opal (body color), Cubic Zirconia, YAG, GGG, Synthetic Moissanite, Glass (N-BK7), Glass (F2), Rutile.

## 0. How to read this document

**Band convention.** Every band is `{center_nm, width, peak, shape}`.

* `shape = E` is an energy (wavenumber) Gaussian and `width` is its standard deviation sigma in cm^-1 (`AbsorptionBand::energy` stores FWHM / 2.3548 as sigma; every FWHM quoted below is converted with the same factor). `shape = W` would be a wavelength Gaussian with sigma in nm; no band below needs it.
* `peak` is the Napierian absorption coefficient alpha in **cm^-1 at the stated reference density** (divide by 10 for the per-millimetre `PerMm` unit of the plan). Face-up path used for the arithmetic: 8.8 mm (7 mm stone, plan section 7).
* Spectra from Caltech are decadic absorbance A. alpha = 2.303 A / path_cm. "Baseline" is the constant that was subtracted (reflection loss plus plateau) before the bands were fitted; it is stated per fit.
* Fits quoted as "fixed centres, NNLS amplitudes": centres and widths were fixed from literature (stated), the non-negative amplitudes were solved by linear least squares on the tabulated Caltech points. The residual is reported as rms and maximum error in decadic A over the stated range. These are a starting point for the 2026-10-19-style bounded refit of the fitting lane, not a final fit. No chromophore-amount solving was done (owner decision of 2026-10-27).
* Confidence labels: `measured` (digital spectrum from the originating lab used, density scaled only), `secondary` (number taken from the repo's `docs/chromophores/` tables, which come from a user-supplied report with no per-number reference and are marked unverified in that file), `literature` (a band position or range quoted in a source that was read), `estimated` (my own estimate, reasoning given). A number labelled `estimated` is never to be presented as measured in the UI.
* Every `estimated` density is a TARGET to be tuned by the fitting lane against the stated face-up color, not a measurement.

**Rank rule (plan 2.1).** Quartz and moissanite and rutile are uniaxial (o-ray = E perpendicular to c = "Along", e-ray = E parallel to c = "Across"). Diamond, opal, CZ, YAG, GGG and both glasses are isotropic: the variant stores equal sets in every slot (per the plan, never `isotropic`-built for a pleochroic host; for an isotropic host a single set is the correct rank).

## 1. Summary table

CSV directory: `crates/pleochroma/data/reference_spectra/`. "Bands o/e" is the proposed band count per ray; isotropic hosts show the single set count.

| Material | Variant | Cause | Data quality | CSV files | Bands o/e |
|---|---|---|---|---|---|
| Amethyst | Deep Siberian | Fe4+ ([FeO4]0 hole) centre from irradiated Fe3+ | o-ray shape measured (Caltech Anahi, full resolution); e-ray and density estimated | `variant_amethyst_deep_siberian_eperpc.csv` | 3/3 |
| Amethyst | Rose de France | same centre, low density | same file as Deep Siberian, density x0.15-0.25 estimated | (same file) | 3/3 |
| Citrine | Golden (Material Default) | Fe3+ oxide-hydroxide nano-clusters, UV-blue edge | secondary (sparse unverified table); pleochroism negligible, estimated | none (existing `citrine_annealed_amethyst.csv`) | 3/3 |
| Citrine | Madeira | same, about 4x the density | estimated | none | 3/3 |
| Quartz | Rock Crystal (Material Default) | none | n/a | none | 0/0 |
| Quartz | Smoky | Al hole centre [AlO4/h+]0 (irradiation) | measured, both rays (Caltech GRR 2918) | `variant_quartz_smoky_eperpc.csv`, `variant_quartz_smoky_eparc.csv` | 4/4 |
| Quartz | Prasiolite | Fe, band near 720 nm (Fe2+-Fe3+ IVCT or Fe2+; contested) | estimated amplitudes, literature positions | none | 5/5 |
| Quartz | Rose (massive) | Fe2+-Ti4+ in dumortierite-type fibres plus scattering | DROPPED (section 3.6) | (`rose.mt` is diffuse transmission, not converted) | - |
| Quartz | Pink (Crystalline), optional replacement | P substitution plus O- hole centre (Caltech statement) | measured, both rays (Caltech GRR 2352) | `variant_quartz_pink_crystalline_eperpc.csv`, `variant_quartz_pink_crystalline_eparc.csv` | 3/3 |
| Diamond | Colorless (Material Default) | none (type IIa) | n/a | none | 0 |
| Diamond | Cape Yellow N3+N2 | N3 (415.2) and N2 (478) aggregates | literature positions, secondary strengths, estimated | none | 4 |
| Diamond | Canary (C centre) | isolated Ns0 | 270 nm cross-section primary; visible continuum estimated | none | 4 |
| Diamond | Brown | vacancy clusters, plastic deformation | band shape from a CVD patent, estimated | none | 3 |
| Diamond | Pink (550 nm) | 550 nm band, plastic deformation, centre unidentified | band position and width primary (GIA); strength estimated | none | 2 |
| Diamond | Blue (boron) | uncompensated B acceptor, photoionisation tail | boron range primary; tail shape and strength estimated | none | 1 |
| Diamond | Green (GR1) | neutral vacancy V0 (irradiation) plus cape lines | GR1 positions primary; strength estimated | none | 3 |
| Opal | Water (Material Default) | none | n/a | none | 0 |
| Opal | Fire Orange | Fe3+ (hematite-like sub-microscopic domains) | measured edge (Caltech GRR 1824), density scaled | `variant_opal_fire_red_0p537mm_iso.csv`, `variant_opal_fire_red_1p282mm_iso.csv` | 3 |
| Opal | Fire Red (cherry) | same, denser | same files | (same files) | 3 |
| Opal | Peruvian Blue-Green | Cu2+ in chrysocolla (or shattuckite) inclusions plus scattering | no spectrum; DROP recommended (section 4.8.3) | none | - |
| Cubic Zirconia | Colorless (MD) | none | n/a | none | 0 |
| Cubic Zirconia | Ce Yellow / Orange | Ce4+/Ce3+ charge-transfer edge | secondary | none | 2 |
| Cubic Zirconia | Er Pink | Er3+ 4f lines | secondary | none | 3 |
| Cubic Zirconia | Nd Lilac | Nd3+ 4f lines | secondary | none | 3 |
| Cubic Zirconia | Cr Green | Cr3+ bands | secondary, low | none | 2 |
| Cubic Zirconia | V Green | - | DROPPED (no data) | none | - |
| YAG | Colorless (MD) | none | n/a | none | 0 |
| YAG | Nd Lilac | Nd3+ lines | secondary (line manifold concern) | none | 4 |
| YAG | Cr Green | Cr3+ 4A2-4T1 and 4A2-4T2 | literature positions, secondary strengths | none | 2 |
| YAG | Ce Yellow | Ce3+ 4f-5d | literature position, secondary strengths | none | 2 |
| GGG | Colorless (MD) | none | n/a | none | 0 |
| GGG | doped variants | - | DROPPED (section 5.5) | none | - |
| Synthetic Moissanite | Colorless (MD) | none | n/a | none | 0/0 |
| Synthetic Moissanite | Near-Colorless (K-L edge) | 425 nm absorption edge of 6H SiC | literature, amplitude estimated | none | 1/1 |
| Synthetic Moissanite | Green Tint | N donor, 625 nm free-carrier band | secondary | none | 2/2 |
| Synthetic Moissanite | Gray | cause not documented | DROPPED | none | - |
| Glass (N-BK7), Glass (F2) | Colorless (MD) | none | n/a | none | 0 |
| Glass | Co Blue | Co2+ tetrahedral triplet | secondary, positions literature | none | 3 |
| Glass | Cr Green | Cr3+ octahedral (650 nm) | secondary, position literature | none | 2 |
| Glass | Mn Purple | Mn3+ (5Eg-5T2g, 500 nm) | secondary, position literature | none | 1 |
| Glass | Se Pink | elemental Se, 485-500 nm | extinction literature, rest estimated | none | 1 |
| Glass | Se Ruby (CdSe-CdS) | semiconductor-dot absorption edge | estimated (calibrate against Schott OG/RG curves) | none | 3 |
| Rutile | Material Default | intrinsic UV edge (350 nm band) | existing tensor | none | 1/1 |
| Rutile | Brownish Red-Orange | Fe/Cr (and V, Nb) charge-transfer edge | measured, both rays (Caltech), thickness ambiguity | `variant_rutile_brownish_red_orange_eperpc.csv`, `variant_rutile_brownish_red_orange_eparc.csv` | 1/1 |
| Rutile | Orange (Graves Mountain, optional) | same, 1.3 % other ions | measured, both rays | `variant_rutile_orange_dv1_eperpc.csv`, `variant_rutile_orange_dv1_eparc.csv` | 1/1 |
| Rutile | Blue (Reduced) | Ti3+ d-d and Ti3+-Ti4+ polaron | no numeric spectrum; estimated | none | 2/2 |

CSV files delivered (11): smoky x2, pink crystalline x2, amethyst x1, rutile brownish x2, rutile orange x2, opal x2. All carry the license line `CC BY-NC 4.0` copied from the existing Caltech files (see risk R-license in the report).

## 2. Cross-cutting findings (read these first)

1. **Diamond 270 nm cross-section is off by about 25x in the repo's data.** `docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md` stores alpha(270 nm) = 0.18 cm^-1/ppm for the C centre (= 5.5 ppm per cm^-1). A primary calibration (Rapid determination of Ns0 from UV-Vis, arXiv:2207.14278, Appl. Phys. Lett. 2022, https://arxiv.org/html/2207.14278v2) gives mu270 = sigma [Ns0] with sigma = 1.96 cm^-1/ppm (decadic) = 4.51 cm^-1/ppm (natural log) = 1.11e-17 cm^2, checked against EPR on six CVD diamonds (3.2-19.3 ppm Ns0, mu270 5.9-37.2 cm^-1). The paper cites an earlier literature value 1/sigma = 0.6 +/- 0.1 ppm per cm^-1, consistent. The repo's visible Ns0 continuum (0.0015 cm^-1/ppm at 560 nm onset, "estimated") is therefore also far too weak: with the patent's ratio alpha(425)/alpha(270) of about 0.1 (section 3.2) the continuum is about 0.45 cm^-1/ppm at 425 nm. Do not use the repo's Ns0 coefficients for a variant.
2. **Rutile (Caltech) has a thickness conflict.** The data headers of the "250-2h d" North Carolina sample say 0.269 mm; the Caltech Rutile index page says 2.69 mm. Unresolved. The Brownish Red-Orange variant is therefore defined by an absorbance TARGET at 500 nm (alpha about 2.5 cm^-1, section 7.2), which does not depend on the answer.
3. **Existing Amethyst, Citrine and Quartz tensors are not rank-matched.** `amethyst_through_citrine.rs` builds Amethyst and Citrine with `AbsorptionTensor::isotropic` on a uniaxial host (and `Quartz`, `Synthetic Moissanite` with an empty isotropic set). The plan's `host_rank` will report `Uniaxial` for these hosts, so the Material Default entry would fail `validate_for` with `RankMismatch`. See NEEDS in the report.
4. **Caltech spectra are the only downloadable numeric polarized spectra found for this lane** (quartz, rutile, opal). Diamond (GIA spectra on the Caltech diamond page) is images only; glass, CZ, YAG, GGG and moissanite have no digital spectra on the Caltech pages that I could reach as text (Pearson's CZ/YAG/GGG compendium is distributed as `.xls`, a binary format excluded by the lane rules).
5. **Blocked or unusable sources.** `minerals.gps.caltech.edu` over HTTPS timed out and the WebFetch tool got connection refused; plain HTTP via curl worked and all Caltech text files below were downloaded that way. The Crystals 12(3):322 fire-opal paper (https://www2.mdpi.com/2073-4352/12/3/322) failed on an expired certificate; its content is known only through a search summary. Patent PDFs and arXiv PDFs returned compressed streams; the arXiv HTML page and Google Patents page were readable.

## 3. Quartz family (host alpha-quartz, uniaxial positive)

Quartz facts used throughout: o-ray = E perpendicular c. All fits below use the o/e split; where no e-ray data exist the e-ray is labelled estimated.

### 3.1 Amethyst

#### 3.1.1 Deep Siberian and Rose de France

1. **Cause.** Fe3+ on a Si or interstitial site is oxidised by ionising radiation to Fe4+ ([FeO4]0 hole centre). Caltech: amethyst and citrine are both colored by about 40 ppm iron; "radiation oxidizes Fe3+ to Fe4+", the exact site is uncertain (https://minerals.gps.caltech.edu/Ametrine/Index.html, read 2026-10-27). Bands 545 nm, 357 nm and about 950 nm: Cox, J. Phys. C 10, 4631 (1977), as quoted in https://www.ncbi.nlm.nih.gov/pmc/articles/PMC7483767/ and https://link.springer.com/article/10.1007/s00269-016-0864-z. Heating above about 400-500 degrees C bleaches the centre (repo doc, Report p.26-27). "Deep Siberian" is a grade name (deep purple with red and blue flashes) and "Rose de France" a pale lilac grade: both are the same centre at different densities; no separate spectra exist.
2. **Spectra.**
   * Measured o-ray: Caltech amethyst, Anahi Mine, Bolivia, 6.17 mm slab cut (0001), beam along c so E is perpendicular to c. Source http://minerals.gps.caltech.edu/FILES/Visible/quartz/ameth.bo. Full-resolution CSV: `variant_amethyst_deep_siberian_eperpc.csv` (the repo already holds a 5 nm digest `caltech_quartz_amethyst_ameth_bo_5nm.csv`). Peak (at 361.7 nm) A = 0.638; the 545 nm band peaks at A = 0.451 over a baseline of about 0.23.
   * e-ray: no digital spectrum. The repo's polarized text states E perpendicular c bluish violet, E parallel c reddish violet (`research-2026-10-polarized-spectra.md`), and the Report weights are w_o : w_e = 1.0 : 0.7 for 545 nm, 1.0 : 0.5 for 950 nm, 1.0 : 1.0 for 357 nm (`secondary`). No primary polarized amethyst spectrum was found in this lane.
3. **Band model (density of the Anahi sample).** Fixed centres 357 / 545 / 950 nm, sigma 2800 / 1478 / 1910 cm^-1 (545 nm: FWHM 3480 cm^-1, the value of the repo's validation fit; 950: FWHM 4498 cm^-1 from the Report), baseline 0.23, range 330-1050 nm, NNLS: rms 0.030 A, maximum error 0.136 A (largest at 420-480 nm, where the model is 0.05-0.07 A low; a four-band alternative 357 / 520 / 570 / 950 with sigma 2800 / 1700 / 1300 / 1910 reaches rms 0.024 A).

   | ray | center_nm | width sigma cm^-1 | peak cm^-1 | shape |
   |---|---|---|---|---|
   | o | 357 | 2800 | 1.39 | E |
   | o | 545 | 1478 | 0.94 | E |
   | o | 950 | 1910 | 0.125 | E |
   | e (estimated) | 357 | 2800 | 1.39 | E |
   | e (estimated) | 545 | 1478 | 0.66 | E |
   | e (estimated) | 950 | 1910 | 0.06 | E |

   Density: Deep Siberian = 2.5-3.5 x these peaks; Rose de France = 0.15-0.25 x (`estimated`; set by the face-up target). Check of the anchor: the Anahi o-ray gives T(545 nm, 8.8 mm) = exp(-0.94 x 0.88) = 0.44 (pale to medium violet). Deep Siberian at x3 gives T = 0.08 (deep).
4. **Target face-up color.** Deep Siberian: deep purple with a reddish flash along the e-ray. Rose de France: very pale pinkish lilac. The repo computed the Anahi o-ray (6 mm, D65) as L* 62.4, a* 22.8, b* -18.5 (`research-2026-10-polarized-spectra.md` colorimetry table, `secondary`); use it as the o-ray anchor of the unit-density case, not as a published color.
5. **Concentration anchor.** About 40 ppm Fe in the Caltech statement (not Fe4+ only). The repo's calibration `amethyst_80ppmw_fe.csv` carries a per-ppma coefficient for the 545 band (0.879 per ppma in `research-2026-10-validation.md`; `secondary`).
6. **Confidence and gaps.** Shape: measured. Pleochroism of the e-ray: `estimated` from the Report weights. The UV band is only 269-360 nm in the data, so the 357 nm sigma depends on a one-sided slope. No Siberian-locality spectrum was found; the density multipliers are guesses.

### 3.2 Citrine

#### 3.2.1 Golden (Material Default family) and Madeira

1. **Cause.** Fe3+ in small hydrated aggregates (iron oxide-hydroxide clusters), formed when amethyst is heated or in some natural citrine. Caltech: citrine = heated amethyst, "due to small aggregates of iron oxide" (https://minerals.gps.caltech.edu/FILES/Visible/quartz/Index.html) and "Fe3+ ions are aggregated and hydrated in clusters of unknown size" (https://minerals.gps.caltech.edu/Ametrine/Index.html). The spectrum is a charge-transfer edge, not a discrete band.
2. **Spectra.** Caltech holds a JPG only (GRR 3910, Rio Grande do Sul, 2.29 mm, `quartz/Citrine_3910_spectrum.jpg`): no numeric file, so nothing was downloaded. The repo's `citrine_annealed_amethyst.csv` is a 16-point alpha(lambda) table (350 nm: 14.5 cm^-1; 400: 7.4; 450: 2.8; 500: 0.95; 545: 0.35; 605: 0.12; 800: 0.02) from `research-2026-10-polarized-spectra.md` (user-delivered, unverified). Over 350-605 nm it is a pure exponential edge with an e-fold of about 52-55 nm. Polarization: none found; gemological pleochroism of citrine is very weak, so equal o/e sets are proposed (`estimated`).
3. **Band model.** Three energy Gaussians, relative-error weighted NNLS on the 16 table points (error +5 % at 350 nm, +17 to +20 % at 520-545 nm, within 10 % at 370-500 nm; the faint tail beyond 635 nm, alpha under 0.1 cm^-1, is 30-50 % low):

   | ray | center_nm | width sigma cm^-1 | peak cm^-1 | shape |
   |---|---|---|---|---|
   | o = e | 330 | 3600 | 16.56 | E |
   | o = e | 410 | 3000 | 1.36 | E |
   | o = e | 500 | 2200 | 0.23 | E |

   Madeira: the same three peaks x 4 (`estimated`; the edge then reads about 1.4 cm^-1 at 545 nm and 0.5 cm^-1 at 605 nm, which moves the transmission edge into the orange). A two-band fit (345 / 450 nm, sigma 4000 / 2800, peaks 11.85 / 0.13 cm^-1) is within 20 % at 350-545 nm if the fitting lane prefers 2 bands.
4. **Target face-up color.** Golden: medium yellow to golden. The repo's own record: unpolarised, 6 mm, D65, L* 76.8, C* 58.4, h 83.4 for the table density, but the repo's validation notes that this CIELAB disagrees with the table spectrum by dE00 12.7 (`research-2026-10-validation.md`), so treat the table as the shape and choose the density from the visual target. Madeira: orange to reddish brown.
5. **Concentration anchor.** About 40 ppm total Fe (Caltech Ametrine page); the repo's calibration assumes 10 ppma for the edge in `research-2026-10-validation.md` (`secondary`).
6. **Confidence and gaps.** Shape: `secondary` (sparse, unverified table; no digital natural citrine spectrum exists in the data sets found). Pleochroism: `estimated`.

### 3.3 Quartz: Rock Crystal (Material Default)

Colorless. Empty band sets in both slots. No data needed. (The UV edge near 150 nm is irrelevant to the visible.)

### 3.4 Smoky

1. **Cause.** Radiation-induced Al hole centre [AlO4/h+]0 (hole on oxygen next to substitutional Al3+). The classic assignment places the smoky color in a broad band at 2.90 eV (428 nm) with half-width 1.5 eV; two further bands at 1.85 eV and 2.55 eV are the "blue" bands of the same paper, not correlated with the Al centre (RRUFF copy of "Smoky, Blue, Greenish Yellow, and Other Irradiation-Related Colors in Quartz", Mineralogical Magazine 41, 319, https://rruff.info/doclib/MinMag/Volume_41/41-319-301.htm, read through a search summary). A Brazilian gamma-irradiation study assigns absorption at 3.4 eV (365 nm), 2.6 eV (475 nm) and 1.9 eV (650 nm) to [AlSiO4/h+]0 centres following Itoh (https://www.scielo.br/j/mr/a/NPJGPDBLyFb5Dxy9TttWVpv/, search summary). Bleaches at about 300-400 degrees C.
2. **Spectra (measured, both rays).** Caltech GRR 2918, quartz var. smoky, 5.788 mm, header locality Jack Rabbit Claim, Teller County, Colorado (the index page lists "Crystal Peak, Teller County" for the same slice). Sources: http://minerals.gps.caltech.edu/FILES/Visible/quartz/Quartz2918%20perp%20c.asc and `.../Quartz2918%20parallel%20c.asc`. CSVs: `variant_quartz_smoky_eperpc.csv` (381-1101 nm) and `variant_quartz_smoky_eparc.csv` (380-1101 nm). The crystal is very dark ("dark black crystal"): A = 1.02 (o) and 1.46 (e) at 420 nm, falling to 0.09 / 0.15 at 1000 nm. The e-ray absorbs 1.5-1.75 times more than the o-ray across 400-900 nm after baseline subtraction (0.06 o, 0.10 e).
3. **Band model.** Fixed centres 365 / 428 / 486 / 652 nm. Sigma 5140 cm^-1 for 428 nm is the literature FWHM 1.5 eV = 12100 cm^-1 / 2.3548; 365 (3500), 486 (2500) and 652 (2300) are my width choices (`estimated`; the data have no resolved peaks). Amplitudes by NNLS on 380-1050 nm. Residual: o rms 0.017 A (max 0.039 A), e rms 0.041 A (max 0.072 A). The UV band at 365 nm lies at the data edge (380 nm), so its peak is poorly constrained.

   | ray | center_nm | width sigma cm^-1 | peak cm^-1 at the Caltech density | shape |
   |---|---|---|---|---|
   | o | 365 | 3500 | 2.99 | E |
   | o | 428 | 5140 | 1.05 | E |
   | o | 486 | 2500 | 2.31 | E |
   | o | 652 | 2300 | 1.77 | E |
   | e | 365 | 3500 | 3.55 | E |
   | e | 428 | 5140 | 2.32 | E |
   | e | 486 | 2500 | 2.45 | E |
   | e | 652 | 2300 | 2.44 | E |

   Density: the Caltech crystal is "morion-dark" (T about 4 % at 500 nm over 8.8 mm in the o-ray). Smoky gem density about 0.3 x these peaks (about 25-45 % transmission at 500 nm, `estimated`); Dark Smoky = 1.0 x.
4. **Target face-up color.** Grey-brown, slightly warmer in the e-ray (the e-ray is the stronger absorber across the visible, so looking along c is lighter). Documented dichroic ratio for the 2.9 eV band in synthetic quartz: 1.15-1.22 (American Mineralogist note, quoted in the same search summary) is far smaller than the 1.5-1.75 measured here; the Caltech spectra are the better basis for the pleochroism.
5. **Concentration anchor.** None published for this sample (no composition in the header). The repo's Report value (0.05 cm^-1/ppm Al, `secondary`) was not used.
6. **Confidence and gaps.** o and e shape: measured. Band widths and the 486 nm centre are my choices; the UV amplitude is weakly constrained.

### 3.5 Prasiolite

1. **Cause (contested).** Iron is the established chromophore; the mechanism is not. Platonov (1992) linked the green to an Fe2+-Fe3+ intervalence transfer near 725 nm; another paper places the dominant absorption at 13,660 cm^-1 (about 730 nm); heating amethyst at about 400-500 degrees C converts interstitial Fe3+ to Fe2+ with a broad band near 720 nm (Czaja, https://rebus.us.edu.pl/bitstream/20.500.12128/3822/1/Czaja_The_Mossbauer_spectra_of_prasiolite_and_amethyst_crystals.pdf; summaries of https://link.springer.com/article/10.1007/s00269-016-0864-z and https://reference-global.com/article/10.1515/mipo-2016-0004). A Mossbauer study found no Fe2+ or Fe4+ and attributed both violet and green to Fe3+ (Springer paper); Herbert and Rossman (2008) found iron plus irradiation is needed and that heating turns the Thunder Bay material yellow, brown or colorless above about 300 degrees C. Paradise (1982, Gems & Gemology) saw a weak iron band near 442 nm in a green variety (https://cms.gia.edu/dam/jcr:98bcbe8f-2256-4710-9f43-cf009433fd81/Satellite-1495880557373.pdf).
2. **Spectra.** No digital polarized spectrum. Not on the Caltech quartz page.
3. **Band model (all `estimated` densities).** The Report gives 720 nm with FWHM 3500 cm^-1 and 0.03 cm^-1/ppm Fe2+ (`secondary`); at 30 ppm that is 0.9 cm^-1. Add a weak 442 nm band (Paradise) and a thin Fe3+ edge (the Golden Citrine shape x 0.1) so the stone is green-yellow rather than cyan:

   | ray | center_nm | width sigma cm^-1 | peak cm^-1 | shape |
   |---|---|---|---|---|
   | o = e | 720 | 1486 | 0.90 | E |
   | o = e | 442 | 1500 | 0.20 | E |
   | o = e | 330 | 3600 | 1.66 | E |
   | o = e | 410 | 3000 | 0.14 | E |
   | o = e | 500 | 2200 | 0.02 | E |

   Check: T(720 nm, 8.8 mm) = exp(-0.9 x 0.88) = 0.45.
4. **Target face-up color.** Pale leek green to yellowish green (the repo Report: "pale leek green").
5. **Anchor.** The Report's 5-60 ppm Fe2+ is `estimated`.
6. **Confidence.** Low. The cause is disputed in the literature, no spectrum was read and polarization is unknown (equal sets, `estimated`). Ship as "estimated" with a tooltip.

### 3.6 Rose quartz: DROPPED, with the honest reasoning

* **Cause.** Massive rose quartz is colored by micro-fibres of a Ti and Fe bearing aluminium-borosilicate (dumortierite-type) phase inside the quartz, with an Fe2+-Ti4+ charge transfer in the fibres (Goreva, Ma and Rossman, Am. Mineral. 86, 270, 2001; Caltech: "minute fibers of an aluminum borosilicate phase", https://minerals.gps.caltech.edu/FILES/Visible/quartz/Index.html). The milky look and asterism are scattering by the fibres.
* **Data.** The only digital Caltech rose spectrum (GRR 1787, Ruby Range, Montana, `quartz/rose.mt`, 269-1099 nm) is a DIFFUSE TRANSMISSION measurement of a lump (A about 0.51-0.62 from 380 to 500 nm, peak 0.62 at 500 nm). It contains scattering, so it is not a Beer-Lambert absorption spectrum, and the path is not defined. It was not converted into a CSV.
* **Decision.** A band model would paint a scattering and inclusion color as bulk absorption and would also be used in a transparent faceted stone, where massive rose quartz does not exist. Drop "Rose" (massive).
* **Honest replacement (optional).** Crystalline transparent pink quartz (Mt Mica, Maine) has a different cause and measured polarized spectra: section 3.7.

### 3.7 Pink (Crystalline), optional variant to replace Rose

1. **Cause.** Caltech: "color of the pink quartz variety comes from P substitution in the silica"; natural gamma radiation strips an electron from the oxygen next to phosphorus to form an O- group; synthetic Russian pink quartz has essentially the same spectrum (https://minerals.gps.caltech.edu/FILES/Visible/quartz/Index.html). This is a color-centre absorption in a transparent crystal and is representable by bands.
2. **Spectra (measured, both rays).** Caltech GRR 2352, Mount Mica, Maine, 3.681 mm: http://minerals.gps.caltech.edu/FILES/Visible/quartz/Pink_quartz_2352_a.txt (E perpendicular c, 381-1129 nm) and `Pink_quartz_2352_c.txt` (E parallel c, 390-1051 nm). CSVs `variant_quartz_pink_crystalline_eperpc.csv` and `variant_quartz_pink_crystalline_eparc.csv`. Absorbance is small: peak A about 0.20 (o) and 0.18 (e) at 520-525 nm.
3. **Band model.** Fixed centres 390 / 525 / 700 nm with sigma 3000 / 2800 / 2500 cm^-1 (my widths), baselines 0.07 (o) and 0.10 (e), NNLS on 390-1000 nm. Residual: o rms 0.011 A (max 0.023), e rms 0.006 A (max 0.013).

   | ray | center_nm | width sigma cm^-1 | peak cm^-1 | shape |
   |---|---|---|---|---|
   | o | 390 | 3000 | 0.56 | E |
   | o | 525 | 2800 | 0.66 | E |
   | o | 700 | 2500 | 0.06 | E |
   | e | 390 | 3000 | 0.36 | E |
   | e | 525 | 2800 | 0.37 | E |
   | e | 700 | 2500 | 0.056 | E |

   The o-ray absorbs 1.75 x more at 525 nm than the e-ray (measured).
4. **Target face-up color.** Pale cool pink (T at 525 nm over 8.8 mm = exp(-0.66 x 0.88) = 0.56 for the o-ray at this density; real crystals are often paler).
5. **Anchor.** None published.
6. **Confidence.** Measured, but a single stone and small A (baseline sensitive). The variant is a replacement suggestion, not in the plan's candidate list.

## 4. Diamond (isotropic) and opal (isotropic)

Isotropic: o = e. All diamond variants are `Colorless Material Default` plus the variants below.

### 4.1 Diamond: shared background

* Spectra: the Caltech diamond page holds GIA spectra only as images (fancy intense yellow GIA 4890, fancy intense blue GIA 1119, pink GIA 7861, blue-green GIA 4684, brown-yellow GIA 6848, brown-orange GIA 6154, gray-violet GIA 6764; https://minerals.gps.caltech.edu/FILES/Visible/diamond/Index.html). No numeric files exist, and image downloads are excluded by the lane rules, so no CSV was produced for diamond. Every diamond band below is a band-list entry with its source.
* Type IIa diamond edge near 225 nm; type Ia near 330 nm ("increasingly transparent down to about 330 nm", Nassau et al. 1997 comparing diamond with moissanite, Gems & Gemology 33(4), https://www.moissaniteco.com/media/view/2018/09/synthetic-moissanite-a-new-diamond-substitute_67.pdf). None of these edges matters for the visible.

### 4.2 Cape Yellow (N3 + N2)

1. **Cause.** Nitrogen aggregates in type Ia(AB) diamond: N3 (three N around a vacancy), zero-phonon line 415.2 nm, with associated absorptions at 451, 463 and 478 nm (N2); minor 435 and 426 nm lines at higher saturation. Cape diamonds often exceed 1000 ppma total N; the defects are weak absorbers, so a saturated yellow needs high concentration or long path (GIA, Breeding et al., "Naturally Colored Yellow and Orange Gem Diamonds: The Nitrogen Factor", Gems & Gemology Summer 2020, https://www.gia.edu/gems-gemology/summer-2020-naturally-colored-yellow-orange-diamonds). GIA lab notes confirm 415 (N3) and 478 (N2) in yellow diamonds (https://www.gia.edu/gems-gemology/spring-2019-labnotes-largest-diamond-discovered-in-north-america).
2. **Spectra.** None numeric. Band widths and strengths: Report p.3-4 (`secondary`): N3 FWHM 1500 cm^-1, 8.0e-4 cm^-1/ppm N; N2 FWHM 2500 cm^-1, 4.0e-4 cm^-1/ppm.
3. **Band model at 1000 ppm total N (the GIA ">1000 ppma" anchor).**

   | center_nm | width sigma cm^-1 | peak cm^-1 | shape | basis |
   |---|---|---|---|---|
   | 415.2 | 637 | 0.80 | E | Report widths and 8.0e-4 x 1000 ppm (`secondary`) |
   | 478 | 1062 | 0.40 | E | Report 4.0e-4 x 1000 ppm (`secondary`) |
   | 445 | 1100 | 0.30 | E | lump of the 435/451/463 lines (`estimated`) |
   | 350 | 3500 | 0.50 | E | blue-violet side of the Ia edge (`estimated`) |

   Check: T(415 nm, 8.8 mm) = exp(-0.8 x 0.88) = 0.49 from N3 alone; with the other bands (total alpha about 1.1 cm^-1 at 415 nm) about 0.37, pale to light yellow ("Fancy Light", saturation limited as GIA states).
4. **Target face-up color.** Light yellow to Fancy Light yellow, cooler and weaker than a canary.
5. **Anchor.** Over 1000 ppma N (GIA 2020). 
6. **Confidence.** Positions: literature. Strengths: `secondary`. Note the repo's flagged concern (D21) that a Gaussian centred on a zero-phonon line is physically doubtful; the lump band mitigates it.

### 4.3 Canary (C centre, type Ib)

1. **Cause.** Isolated substitutional nitrogen Ns0. GIA: absorption continuum to about 510 nm gives yellow, to about 600 nm orange; "only a few ppm" suffices; C centres are very strong absorbers; total isolated N usually under 100 ppma; melee with Fancy yellow or stronger color are nearly always C-centre colored (Breeding 2020, same URL as 4.2).
2. **Spectra.** None numeric for gem stones. Primary calibration of the 270 nm band: sigma = 4.51 cm^-1/ppm (natural), band centre 268-272 nm, RMS width 13-27 nm (arXiv:2207.14278 html, section 2 item 1). The Element Six "Coloured diamond" patent states a single-substitutional-nitrogen component of 0.4-10 cm^-1 at 270 nm (full range 0.1-30) and an absorption coefficient at 425 nm of 0.04-1 cm^-1 (https://patents.google.com/patent/US7910083B2/en, read 2026-10-27): both ranges span the same factor, so alpha(425)/alpha(270) = 0.1 (`estimated` from the ratio of range ends; not a per-sample measurement).
3. **Band model.** Continuum approximation: target alpha(lambda) = alpha(425) exp(-(lambda - 425)/60 nm) between 380 and 560 nm (the 60 nm e-fold is the Report's own default, `estimated`) approximated by three energy Gaussians. Fixed centres 330 / 410 / 480 nm, sigma 3500 / 2200 / 1500 cm^-1, relative-error weighted NNLS: amplitudes 2.80 / 0.636 / 0.142 times alpha(425). Error against the exponential: -10 % at 380 nm, +5 % at 425 nm, +3 % at 450 nm, -14 % at 500 nm, and about -60 % at 550 nm (the red tail beyond 520 nm is under-represented; add a fourth band 540 nm, sigma 1000, 0.05 x alpha(425) if the orange extent matters). Canary density used: 2.4 ppm Ns0, alpha(270) = 10.8 cm^-1, alpha(425) = 1.1 cm^-1:

   | center_nm | width sigma cm^-1 | peak cm^-1 | shape |
   |---|---|---|---|
   | 270 | 2743 | 10.8 | E (UV band, kept for completeness; sigma = 20 nm x 1e7/270^2) |
   | 330 | 3500 | 3.08 | E |
   | 410 | 2200 | 0.70 | E |
   | 480 | 1500 | 0.16 | E |

   Check: T(425 nm, 8.8 mm) = exp(-1.1 x 0.88) = 0.38.
4. **Target face-up color.** Fancy Intense to Vivid yellow (canary); more Ns0 pushes the edge red (orange at 10 ppm: alpha(425) = 4.5 cm^-1).
5. **Anchor.** Ns0 = 25 ppm per cm^-1 of the 1130 cm^-1 line (verified in the repo, Woods, Collins and van Wyk, as quoted in arXiv:2112.07260); the 270 nm cross-section above; GIA "a few ppm". Fancy vivid canary about 2-3 ppm (`estimated` from the ratio argument).
6. **Confidence.** 270 nm band: primary. Visible continuum: `estimated`. The patent's own text on the ramp is inconsistent (lambda^-3 versus lambda^-1), another reason to keep the continuum labelled estimated.

### 4.4 Brown

1. **Cause.** Vacancy clusters (about 40-60 vacancies) created by plastic deformation produce a "brown absorption continuum" rising toward short wavelengths; HPHT annealing dissociates them (Eaton-Magana et al., "Natural-Color Pink, Purple, Red, and Brown Diamonds: Band of Many Colors", Gems & Gemology Winter 2018, https://www.gia.edu/gems-gemology/winter-2018-natural-color-pink-purple-red-brown-diamonds; Fisher, Sibley and Kelly, J. Phys.: Condens. Matter 21, 364213, 2009, not read in full, functional form not confirmed). GIA gives no functional form.
2. **Spectra.** None numeric for natural stones. Shape borrowed from the CVD brown diamond of the Element Six patent (https://patents.google.com/patent/US7910083B2/en): a band centred 3.54 eV (350 nm) +/- 0.2 eV, FWHM about 1 eV, peak 1-8 cm^-1 (preferred 1.5-6); a band centred 2.43 eV (510 nm) +/- 0.4 eV, FWHM about 1 eV, peak 0.2-4 cm^-1 (preferred 0.4-2); and a ramp c x lambda^-3 (lambda in microns, c < 0.2). The patent's brown diamonds are CVD material; natural brown is described only qualitatively the same, so this is `estimated` for a natural stone.
3. **Band model.** FWHM 1 eV = 8066 cm^-1, sigma 3425 cm^-1. The ramp c = 0.1 (about 1.5 cm^-1 at 510 nm, under the patent's 3 cm^-1 limit) is approximated by one Gaussian: centre 400 nm, sigma 3500 cm^-1, peak 1.94 cm^-1 (against the exact 0.1 lambda^-3: 1.82 / 1.56 / 1.30 / 1.10 / 0.80 / 0.60 cm^-1 at 380 / 400 / 425 / 450 / 500 / 550 nm; the fit gives 1.80 / 1.94 / 1.77 / 1.41 / 0.70 / 0.29, so it overshoots at 400-450 nm by up to 36 % and undershoots beyond 500 nm).

   | center_nm | width sigma cm^-1 | peak cm^-1 | shape |
   |---|---|---|---|
   | 350 | 3425 | 3.0 | E |
   | 510 | 3425 | 1.0 | E |
   | 400 | 3500 | 1.94 | E (ramp stand-in) |

   Sum at 400 / 500 / 600 nm = 3.9 / 1.8 / 0.8 cm^-1.
4. **Target face-up color.** Fancy Light to Fancy brown: yellowish to orangey brown. T(500 nm, 8.8 mm) = 0.20 at this density (medium brown).
5. **Anchor.** None for natural stones.
6. **Confidence.** `estimated` (shape borrowed, amplitudes chosen mid-range of the patent).

### 4.5 Pink (550 nm band)

1. **Cause.** An unidentified defect associated with plastic-deformation lamellae in type Ia diamond (Gaillou et al., Diamond Relat. Mater. 19, 1207, 2010: not read; the abstract states the centre was not identified). Band centre "nominally about 550 nm", fitted 545-565 nm, FWHM 60-100 nm; orangy pink to pink: centre 545-550 nm, width 60-80 nm; purple-pink: centre 555-560 nm, width 70-100 nm; example pinkish orange 548 nm / 62 nm; example purple 556 nm / 105 nm; a smaller narrower UV band at about 390 nm (Eaton-Magana et al. 2018, https://www.gia.edu/gems-gemology/winter-2018-natural-color-pink-purple-red-brown-diamonds).
2. **Spectra.** None numeric.
3. **Band model.** FWHM in energy: 1e7 x 62 / 548^2 = 2065 cm^-1, sigma 877 cm^-1. Amplitude: Report alpha_550 = 1.0 cm^-1 per unit (typical 0.1-3, `secondary`), T(550 nm, 8.8 mm) = exp(-0.88) = 0.41 (Fancy Intense class, `estimated` target). UV band at 390 nm: 0.15 of the 550 peak (Report's 400 nm edge ratio, `secondary`) with sigma 1200 cm^-1 (`estimated`).

   | center_nm | width sigma cm^-1 | peak cm^-1 | shape |
   |---|---|---|---|
   | 548 | 877 | 1.0 | E |
   | 390 | 1200 | 0.15 | E |

   Purple-pink alternative: 556 nm, FWHM 105 nm = 3397 cm^-1, sigma 1443 cm^-1.
4. **Target face-up color.** Pink (Fancy Intense pink); pinkish brown if combined with the brown continuum of 4.4 (not offered as a separate variant).
5. **Anchor.** None published for alpha.
6. **Confidence.** Band position and width: primary (GIA). Strength: `estimated`.

### 4.6 Blue (boron)

1. **Cause.** Uncompensated boron acceptor (0.37 eV above the valence band). The visible color is the long-wavelength photoionisation tail rising from the blue window into the red (GIA: "higher boron concentration is generally associated with a higher absorption gradient and a stronger blue color"; natural type IIb 25 ppb (Faint blue) to 1500 ppb (Fancy Vivid blue), median 300 ppb; Hope diamond 360 ppb; gray type IIb up to 400 ppb; plastic-deformation defects absorbing below 500 nm add a gray component: Eaton-Magana et al., Gems & Gemology Summer 2018, https://www.gia.edu/gems-gemology/summer-2018-natural-color-blue-gray-violet-diamonds). Boron calibrations: NA - ND = 1.00 +/- 0.15 x H1290 (ppm cm^-3, GIA HPHT lab note, https://my.gia.edu/gems-gemology/spring-2016-labnotes-largest-blue-hpht-synthetic-diamond); the repo's Report quotes alpha(3571 nm) = 2.7 cm^-1/ppm (a 10x conflict with another snippet remains unresolved, `secondary`).
2. **Spectra.** None numeric in the visible. The Report's sigma = 1.0e-17 cm^2 at 700 nm gives 1.76 cm^-1 per ppm B (`secondary`, interpretation of a table cell).
3. **Band model.** One Gaussian representing the tail, centre 1000 nm, sigma 4000 cm^-1 (`estimated` shape): relative to alpha(700 nm) it reads 0.08 / 0.22 / 0.44 / 0.72 / 1.00 / 1.46 at 500 / 550 / 600 / 650 / 700 / 800 nm. Density 0.5 ppm B (inside the 25-1500 ppb range): alpha(700) = 0.88 cm^-1, so the peak at 1000 nm = 1.775 x 0.88 = 1.56 cm^-1.

   | center_nm | width sigma cm^-1 | peak cm^-1 | shape |
   |---|---|---|---|
   | 1000 | 4000 | 1.56 | E |

   Check: T(700 nm, 8.8 mm) = exp(-0.88 x 0.88) = 0.46; T(600 nm) = exp(-0.39 x 0.88) = 0.71.
4. **Target face-up color.** Fancy Light to Fancy blue (steel to cornflower), grayish when the 500 nm gray component is added (not modelled).
5. **Anchor.** 25-1500 ppb natural IIb (GIA 2018).
6. **Confidence.** `estimated` shape and amplitude; the boron range is primary.

### 4.7 Green (GR1)

1. **Cause.** Neutral vacancy V0 from irradiation: zero-phonon line 741 nm with a broad absorption about 550-750 nm (GIA 2018, same URL as 4.6, stated as the GR1 feature). Green diamonds also absorb more below 500 nm through the cape lines (N2 477.6 nm, N3 415.2 nm) in green-blue stones (same article). GR1 anneals out at 600-700 degrees C (Report p.3).
2. **Spectra.** None numeric.
3. **Band model.** Report: GR1 alpha = 0.60 cm^-1/ppm vacancies, centre 700, FWHM 1600 cm^-1 (`secondary`, with its own sigma 3.4e-17 cm^2 inconsistent by 10x, D6). The Report width (78 nm) is narrower than the GIA statement of a 550-750 nm envelope, so a wider band is proposed (`estimated`): centre 680 nm, FWHM 2600 cm^-1, sigma 1100 cm^-1, peak 2.0 cm^-1 (about 3 ppm V0 at the Report coefficient). Add the cape lines at 30 % of section 4.2:

   | center_nm | width sigma cm^-1 | peak cm^-1 | shape |
   |---|---|---|---|
   | 680 | 1100 | 2.0 | E |
   | 415.2 | 637 | 0.24 | E |
   | 478 | 1062 | 0.12 | E |

   Check: T(680 nm, 8.8 mm) = exp(-1.76) = 0.17.
4. **Target face-up color.** Bluish green to green (Fancy green).
5. **Anchor.** 0.05-5 ppm V0 (`estimated`).
6. **Confidence.** `estimated`.

### 4.8 Opal (body color)

#### 4.8.1 Water (Material Default)

Colorless (current tensor is empty). No action.

#### 4.8.2 Fire Orange and Fire Red (cherry)

1. **Cause.** Fe3+ in sub-microscopic hematite-like domains. Rossman on the Caltech opal page: "The orangish-red color of this material comes from Fe3+. The absorption bands are similar in position to those of hematite suggesting that sub-microscopic domains of hematite are the cause" (https://minerals.gps.caltech.edu/FILES/Visible/opal/Index.html). A study of natural Mexican and Ethiopian fire opals attributes color to hematite and goethite (Crystals 12(3), 322, https://www2.mdpi.com/2073-4352/12/3/322, via a search summary; the page failed on an expired certificate); a Madagascar study ties fire-opal hues to Fe-rich oxide and hydroxide inclusions (https://air.unimi.it/handle/2434/150286).
2. **Spectra (measured).** Caltech GRR 1824, cherry opal (red variety), Opal Mountain, San Bernardino County, California. `variant_opal_fire_red_0p537mm_iso.csv` (440-1104 nm) from http://minerals.gps.caltech.edu/FILES/Visible/opal/opal1824_537um.txt, and `variant_opal_fire_red_1p282mm_iso.csv` (525-1104 nm) from `opal1824_1282um.txt`. The two plates are consistent per millimetre: A' (baseline-subtracted) per mm at 560 nm = 0.78 (0.537 mm) and 0.73 (1.282 mm); at 580 nm 0.35 and 0.36. The edge is steep: A' = 1.35 at 500 nm, 0.81 at 540 nm, 0.43 at 560 nm, 0.19 at 580 nm, 0.10 at 600 nm (0.537 mm plate, baseline 0.085).
3. **Band model.** A single or a two-Gaussian description fails (steeper than a Gaussian tail; rms 0.045 A for a 3-band set with centres 400/470/535, with the 580 nm point out by 63 %). A set that follows the edge: fixed centres 430 / 490 / 535 nm with sigma 2200 / 1200 / 800 cm^-1, NNLS on 440-1000 nm of the 0.537 mm plate: rms 0.028 A, maximum error 0.059 A (largest at the 600-650 nm foot, where the model is 0.05 A low). Peaks at the cherry density:

   | center_nm | width sigma cm^-1 | peak cm^-1 at cherry density | shape |
   |---|---|---|---|
   | 430 | 2200 | 90.4 | E |
   | 490 | 1200 | 22.3 | E |
   | 535 | 800 | 20.5 | E |

   The cherry plate is extremely dense (A about 2.5 decadic per mm at 500 nm). A face-up stone at the same density would be black-red. Targets (`estimated`): Fire Orange = 0.04 x these peaks (A at 500 nm over 8.8 mm = 0.88, at 560 nm 0.28, at 600 nm 0.07: transmits yellow-orange and red); Fire Red = 0.12 x (A at 500 / 560 / 600 nm = 2.6 / 0.84 / 0.20: transmits only the red).
4. **Target face-up color.** Fire Orange: saturated orange. Fire Red: deep orange-red.
5. **Anchor.** The Caltech plate itself defines the unit density; Fe content not stated in the files.
6. **Confidence.** Shape: measured (one stone). Density multipliers: `estimated`. Play-of-color is diffraction and is not modelled.

#### 4.8.3 Peruvian Blue-Green: DROP recommended

* **Cause (verified to the extent possible).** Cu2+ in chrysocolla inclusions is the commonly cited colorant (Rapaport; gemdat.org summarising Fritsch et al. 1999: up to about 1 % Cu); Gaillou et al. 2008 suggested Mg-rich shattuckite could be the inclusion; a Gems & Gemology 2024 news item on Chilean blue opal offers "internal light scattering and dispersed chrysocolla" and states that no detailed mineralogical analyses were reported (https://www.gia.edu/gems-gemology/spring-2024-gemnews-blue-opal-from-chile, https://www.gemdat.org/gem-43544.html, https://rapaport.com/?p=44613; all via search summaries).
* **Why drop.** (1) No digital absorption spectrum exists in any source I could read. (2) The color is part inclusion scattering (a Rayleigh/Tyndall contribution the absorption model cannot represent). (3) The Report's coefficient for the Cu2+ band (700 nm, FWHM 4000 cm^-1, 0.008 cm^-1/ppm) is 5-20x above typical Cu2+ d-d (Report discrepancy D26). (4) The stones are translucent to opaque and are cut as cabochons, not as faceted face-up stones.
* **If the owner still wants it,** the only available band is the Report's: centre 700 nm, sigma 1699 cm^-1, `secondary`, equal sets; target density to be chosen by color. Mark it `estimated` and low confidence.

## 5. Synthetics (cubic zirconia, YAG, GGG, synthetic moissanite)

All data below are `secondary` (from `docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md` and `corundum_chrysoberyl_spinel_garnet-synthetics.md`, which carry no per-number references) unless a literature position is cited. No digital spectrum for these hosts was available as text (Pearson's Caltech compendium is `.xls`). Sharp 4f lines need fine sampling in the renderer; sigma = FWHM / 2.3548 (250 cm^-1 -> 106; 200 -> 85; 300 -> 127; 40 -> 17; 60 -> 25.5).

### 5.1 Cubic zirconia (isotropic)

Colorless Material Default (empty set).

**Ce Yellow / Orange.**
1. Cause: Ce4+ charge-transfer edge with Ce3+; yellow at 0.1-0.5 wt% CeO2, deep red at 1-3 wt% (Report p.11). The Chinese study of colored CZ (https://jogg.cug.edu.cn/article/id/95388769-5a3e-4668-b0f0-1a0ec24e773f, search summary) lists "Ce red".
2. Spectra: none digital.
3. Band model: the Report gives an edge, "400 (onset 540)", 8.0 cm^-1 per wt% CeO2 at the onset, with no slope. Interpreting it as a plateau below the onset and an exponential decay of 60 nm (the Report's own default, `estimated`) above it, two Gaussians approximate it (fixed centres 380 / 500 nm, sigma 4500 / 2500, relative-error weighted NNLS; the model sits 22 % low at the 540 nm onset and within 10 % elsewhere): per wt% CeO2, peaks 7.25 cm^-1 (380 nm) and 5.49 cm^-1 (500 nm). Yellow at 0.2 wt%: 1.45 / 1.10 cm^-1. Orange at 0.6 wt%: 4.35 / 3.29 cm^-1.
4. Target: yellow to orange (0.2 / 0.6 wt%, `estimated`).
5. Confidence: `secondary`, low-medium. The edge model is mine.

**Er Pink.** Cause: Er3+ 4f lines. Bands (Report p.11): 520 nm (FWHM 250 cm^-1, 2.0 cm^-1/wt% Er2O3), 380 nm (FWHM 300, 2.0), 653 nm (FWHM 200, 0.5). At 1 wt% Er2O3 (`estimated`, Report range 0.2-3): peaks 2.0 / 2.0 / 0.5 cm^-1, sigma 106 / 127 / 85. Gap: Er3+ also absorbs near 487 and 543 nm (standard Er3+ term scheme, recalled; not in the Report). Target: pure pink. Confidence `secondary`. Bands 3.

**Nd Lilac.** Cause: Nd3+ (the CZ study confirms "Nd light purple"). Bands (Report p.11): 582 nm (FWHM 250, 4.0 cm^-1/wt% Nd2O3, strongest hypersensitive 4G5/2 + 2G7/2), 745 nm (FWHM 200, 1.5), 520 nm (FWHM 250, 0.8). At 1 wt% Nd2O3: 4.0 / 1.5 / 0.8 cm^-1, sigma 106 / 85 / 106. Color-change character (lilac in daylight, pinker under incandescent light) is documented (Report p.11). Confidence `secondary`. Bands 3.

**Cr Green.** Cause: Cr3+ (Report p.11-12). Bands: 600 nm (FWHM 3000 cm^-1, 10 cm^-1/wt% Cr2O3) and 440 nm (FWHM 3500, 8.0). At 0.1 wt% (`estimated`): peaks 1.0 and 0.8 cm^-1, sigma 1274 and 1486. T(600 nm, 8.8 mm) = 0.41. Gap: Cr3+ in the 8-fold Zr site is unusual (the Report gives no check) and the Cr color is documented only qualitatively; confidence `secondary`, low. Bands 2.

**V Green: dropped.** The Report lists V only as "Cu, V, Ti, Mn, Ni dopants (commercial, unverified)" with no band data. No source read in this lane gives vanadium bands in CZ.

### 5.2 YAG (isotropic)

Colorless Material Default.

**Nd Lilac.** Report (p.12, `secondary`): 586 nm (FWHM 40 cm^-1, 6.0e-4 cm^-1/ppm), 746 nm (FWHM 40, 3.0e-4), 530 nm (FWHM 60, 2.5e-4), 808 nm (9.0e-4, no width). At 10,000 ppm Nd (1 at. %, basis undefined in the Report): 6.0 / 3.0 / 2.5 / 9.0 cm^-1, sigma 17 / 17 / 25.5 / (width unknown). The 9 cm^-1 at 808 nm for 1 at. % is of the order commonly quoted for Nd:YAG (recalled, not re-sourced here). Concern: the 565-600 nm manifold has several lines, so a single 40 cm^-1 line understates the visible lilac; the fitting lane should widen or add lines (not tuned here). Bands 4.

**Cr Green.** Literature: Cr3+ 4A2-4T1 at about 430-450 nm and 4A2-4T2 at about 600-610 nm (a Cr:YAG nanopowder study at 450 / 600 nm, a Cr,Yb:YAG study at 430 / 600 nm, Optical Materials 2000 at 440 / 605 nm; the position moved to 470 / 610 nm after annealing and co-doping with Yb; Cr6+ charge-transfer bands at 270 and 370 nm and Cr4+ after air anneal; arXiv:2603.18664, arXiv:2603.18673, https://apps.dtic.mil/sti/pdfs/ADP011875.pdf, search summaries). The Report has 590 nm and 430 nm (FWHM 2800 and 3300 cm^-1, 8.0e-4 cm^-1/ppm each): the 590 nm centre is 10-20 nm short of the three literature values; use 605 nm (`literature`) with the Report widths and strength. At 3000 ppm Cr (`estimated`): 2.4 cm^-1 each, sigma 1189 (605 nm) and 1401 (430 nm). Target: emerald-like green, yellowish at low Cr. Bands 2.

**Ce Yellow.** Literature: Ce3+ absorption peaks near 460 nm (a patent measuring 1.0 at. % Ce on 1 mm samples fits a Gaussian to the 460 nm peak, https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/9651483; a McMaster thesis on 0.18 mol % Ce:YAG notes strong absorption near 476 nm) and a UV band near 340 nm (Report). Report strengths (`secondary`): 460 nm FWHM 2800 cm^-1, 0.028 cm^-1/ppm; 340 nm FWHM 2500, 0.015. At 1000 ppm (0.1 at. %, `estimated`): 28 cm^-1 and 15 cm^-1 (sigma 1189 and 1062). That implies a cross-section of about 2e-18 cm^2 if ppm is per Y site; I could not verify an absolute 460 nm coefficient (searches found only relative areas). Use 100-300 ppm for a gem tint (T(460, 8.8 mm) = exp(-2.8 x 0.88) = 0.085 at 100 ppm). Bands 2. Confidence `secondary`, low.

### 5.3 GGG (isotropic): colorless only, dropped variants

GGG is a gem simulant sold essentially colorless. The Report offers Cr3+ (620 and 450 nm, 6.0e-4 cm^-1/ppm each, "yellowish green, in weaker field than YAG") and Nd3+ (586 nm, 5.5e-4), both marked as estimates with undefined ppm basis (`corundum_chrysoberyl_spinel_garnet-synthetics.md`, section 5). No gem-trade color of doped GGG is documented in the sources read, and no spectrum exists. Drop the doped GGG variants; keep only Colorless (Material Default).

### 5.4 Synthetic Moissanite (6H SiC, uniaxial positive)

Colorless Material Default (empty o and e sets).

**Facts that apply to every variant.** Nassau, McClure, Elen and Shigley (Gems & Gemology 33(4), 260-275, 1997, https://www.moissaniteco.com/media/view/2018/09/synthetic-moissanite-a-new-diamond-substitute_67.pdf, text read): 23 GIA samples range from near colorless to light yellow, green and gray; "all showed a distinct increase in absorption at about 425 nm" with no sharp bands (eight UV-visible spectra identical in shape); stones in the I-K range look colorless face-up and show face-up color from about L-N; one U-V stone was obviously gray; refractive indices 2.648 (o) and 2.691 (e); samples were oriented with the c-axis perpendicular to the table. Color is controlled by very small amounts of nitrogen and boron.

**Near-Colorless (K-L edge).**
1. Cause: the 425 nm edge of 6H SiC (band gap 3.02 eV, Report). The 1997 paper gives the edge position but no absorption coefficient.
2. Spectra: none digital.
3. Band model: one UV-edge band per ray, centre 380 nm, sigma 3500 cm^-1; peak chosen so the 425 nm absorbance reaches about 0.1 over 8.8 mm (`estimated`): relative to the peak, the Gaussian at 425 nm (23529 cm^-1 against 26316 cm^-1) is exp(-0.5 x (2787/3500)^2) = 0.73, so peak = 0.1 x 2.303 / (0.88 x 0.73) = 0.36 cm^-1. Both rays 0.36 cm^-1 (equal, `estimated`; no polarized edge data).
4. Target face-up color: K-L, very slight yellowish.
5. Confidence: edge position literature; amplitude estimated. Bands 1/1.

**Green Tint.**
1. Cause: nitrogen donor, free-carrier band at 625 nm in 6H (about 465 nm in 4H), leaving a window near 500 nm (Weingartner et al. 2004; https://cris.fau.de/publications/122709224, as summarised in the repo; `literature` position, Report p.5-6). A 2024 Chinese study attributes green synthetic moissanite mainly to the N shallow donor and finds 6H mostly with minor 15R (https://jogg.cug.edu.cn/article/id/c32df1d1-7de2-4718-9ac5-94df088aeca9, search summary).
2. Spectra: none digital. Report (`secondary`): 625 nm, FWHM 2500 cm^-1, 0.50 cm^-1/ppma N, w_o = 1.0, w_e = 0.4; Al acceptor 415 nm, FWHM 3000, 0.30 cm^-1/ppma (the Report's compensation note: net donor must stay > 0).
3. Band model at 3 ppma N (`estimated`, Report range 1-50): o 625 nm, sigma 1062, peak 1.5 cm^-1; e 625 nm, sigma 1062, peak 0.6 cm^-1 (w_e = 0.4); both rays also carry the edge band of the Near-Colorless variant (380 nm, sigma 3500, 0.36 cm^-1). T(625 nm, 8.8 mm) = exp(-1.5 x 0.88) = 0.27 (o) and 0.59 (e). Bands 2/2.
4. Target: light green to green with a gray or olive undertone (GIA: "grayish green" O-P, "green" L-M entries).
5. Anchor: Report, 1e18 cm^-3 N = 10.4 ppma = 5 cm^-1.
6. Confidence: `secondary`; the 0.4 e/o weight is the Report's and I found no independent polarized spectrum.

**Gray: dropped.** The 1997 samples include several gray stones (0.74 ct U-V gray, 0.57 ct M-N gray, others), but the paper gives no cause or spectrum. A gray absorption is a neutral continuum that no source in this lane specifies.

## 6. Glass (N-BK7 and F2): isotropic

Colorless Material Default for both (empty set). The variants apply to either glass: only the colorant changes, the host (Schott N-BK7: transmission to about 320 nm; F2: lead flint, edge about 350 nm; Report p.28) does not change the visible bands. Coefficients below are cm^-1 per wt% of the oxide, from the Report (`secondary`), cross-checked as the Report does: alpha = 2.303 x eps x c, c = 10 x wt% x rho / M, rho = 2.5 g/cm^3; for CoO c = 0.334 M per wt%.

**Co Blue.**
1. Cause: tetrahedral Co2+ in the silicate network; three bands at 537, 595 and 640 nm in alkali silicate glasses (537-526 nm shifting soda to potash; British Glass digital library abstract, https://www.britglass.org.uk/knowledge-base/digital-library-and-information-services/effect-alkali-ions-optical-absorption, search summary; `literature` positions).
2. Spectra: none digital (Schott filter-glass transmission curves exist but were not obtained as text).
3. Band model: Report p.29: 537 nm (FWHM 1100 cm^-1, 80 cm^-1/wt% CoO), 595 nm (1100, 115), 640 nm (1100, 100). sigma = 467 cm^-1. At 0.01 wt% CoO (`estimated`; Report: 0.05 wt% gives saturated royal blue, alpha(595) = 5.8 cm^-1): peaks 0.8 / 1.15 / 1.0 cm^-1; T(595 nm, 8.8 mm) = 0.36 (cobalt blue, medium). A second option for the fitting lane: merge into one Pekar-like band at 590 nm, FWHM 4000 cm^-1 (`research-2026-10-polarized-spectra.md`, 0.02-0.15 wt% range).
4. Target: royal / cobalt blue.
5. Confidence: `secondary` (positions `literature`). Bands 3.

**Cr Green.**
1. Cause: octahedral Cr3+ gives a band at 650 nm and a light green; Cr6+ (CrO4) gives a very intense band near 365 nm and yellow (glass-chemistry source via search summary, https://www.thaiscience.info/journals/Article/TKJN/10974416.pdf). Redox set by the melt.
2. Spectra: none digital.
3. Band model (Cr3+, reducing melt; Report p.29): 650 nm (FWHM 2500 cm^-1, 11.0 cm^-1/wt% Cr2O3, sigma 1062) and 450 nm (FWHM 3000, 11.0, sigma 1274). At 0.1 wt% (`estimated`): 1.1 / 1.1 cm^-1; T(450 and 650 nm, 8.8 mm) = 0.38 each: a green transmission window near 520-550 nm.
4. Target: emerald-like green.
5. Confidence: `secondary`, 650 nm `literature`. Bands 2.

**Mn Purple.**
1. Cause: octahedral Mn3+, single broad band near 500 nm (5Eg-5T2g) in soda-lime-silicate glass; colorless to dark purple with increasing MnO2 (Coloration in soda-lime-silicate glass system containing manganese, https://kirim.kmutt.ac.th/converis/portal/detail/Publication/600362377?lang=en_GB, search summary; `literature` position, no extinction value found).
2. Spectra: none digital.
3. Band model: one band, Report p.29: 500 nm, FWHM 3500 cm^-1, 70 cm^-1/wt% (eps about 100). The Mn3+ fraction is below 1 (melt redox), so use an effective alpha(500) = 2.0 cm^-1 (T(500 nm, 8.8 mm) = 0.17; `estimated`): sigma 1486 cm^-1.
4. Target: amethyst-purple.
5. Confidence: `secondary`. Bands 1.

**Se Pink (elemental selenium).**
1. Cause: elemental Se in silicate glass; molar extinction coefficient about 20 L mol^-1 cm^-1 at about 485 nm (soda-lime-silica) and about 80 at about 500 nm (soda-strontia-silica); selenite and selenate give no color (Optical absorption and fluorescent behaviour of selenium in ternary silicate glasses, https://oa.tib.eu/renate/items/4645b348-ddcf-4719-9a64-920775412cfc; `literature`, via search summary). The Report's own entry is an `estimate`: 500 nm, FWHM 3000 cm^-1, 0.02 cm^-1/ppm Se0.
2. Spectra: none digital.
3. Band model: one band, centre 490 nm, sigma 1274 cm^-1 (FWHM 3000 cm^-1, Report estimate), peak 0.6 cm^-1 (about 30 ppm effective Se0, `estimated`; T(490 nm, 8.8 mm) = 0.59: pale pink). Bands 1.
4. Target: pale pink (decolorising Se leaves a pink cast).
5. Confidence: peak position and extinction: `literature` for soda-lime; amplitude `estimated`.

**Se Ruby (CdSe-CdS): optional, estimated.**
1. Cause: CdS(1-x)Se(x) semiconductor nanocrystals in glass: an absorption edge whose position (yellow, orange, red) depends on the Se fraction and the strike heat treatment, 0.1-1 wt% Se with Cd (Report p.30). This is absorption (not scattering) and is bandable as an edge.
2. Spectra: none read. The calibration source should be the Schott OG / RG sharp-cut filter glass transmission curves (published cut-on wavelengths), not obtained here.
3. Band model: the Report gives "550 (onset 580)" with 0.02 cm^-1 per ppm Se (no unit printed in the cell, `secondary`). Rather than reuse that I propose three Gaussians approximating an edge with a plateau of 10 cm^-1 below 575 nm and an e-fold of 12 nm above it (both `estimated`; the weighted NNLS fit to that target is 25 % low at 420 nm and 40 % low at 580 nm, so the edge is softer than the target): 430 nm, sigma 2800 cm^-1, 7.5 cm^-1; 500 nm, sigma 1300, 5.1 cm^-1; 545 nm, sigma 700, 8.2 cm^-1. Resulting T over 8.8 mm = below 0.001 at 500-550 nm, 0.04 at 580, 0.17 at 590, 0.37 at 600, 0.70 at 620 and 0.87 at 650 nm (50 % edge near 605 nm): deep orange-red ruby. Bands 3.
4. Target: ruby red (Se ruby / "selenium red").
5. Confidence: `estimated`, low. Recommend dropping if the fitting lane cannot calibrate it against a Schott curve.

**Dropped glass colors.** None of the requested ones is dropped; Au/Ag/Cu colloids and uranyl (Report entries) are not in the candidate list.

## 7. Rutile (uniaxial positive)

Existing Material Default: one band per ray, 350 nm (wavelength sigma 40 nm), peak 8.0, o = e (UV tail only). Rutile n(o) 2.61, n(e) 2.90; edge about 413 nm (Report p.13).

### 7.1 Facts from the Caltech rutile page

"Rutile, TiO2, would be nearly colorless if it were purely titanium dioxide. It commonly has minor amounts of iron, chromium, vanadium, niobium, and hydrogen. A small amount of reduction produces Ti3+ ions that enter into electron exchange with the Ti4+ ions and cause blue color." (https://minerals.gps.caltech.edu/FILES/Visible/Rutile/Index.html). Two samples have numeric polarized files.

### 7.2 Brownish Red-Orange (measured)

1. **Cause.** Fe3+ and Cr3+ (also V, Nb) on Ti sites shift the intrinsic edge to the red: an Urbach-like charge-transfer edge, not discrete bands (Report p.13-14; Caltech DV-1 composition "Cr~Fe>V>Nb", about 1.3 % other ions).
2. **Spectra (measured, both rays).** Caltech rutile "250-2h d", North Carolina, brownish red-orange: http://minerals.gps.caltech.edu/FILES/Visible/Rutile/Rutile%20250-2h%20d.a.txt (E perpendicular c) and `.../Rutile%20250-2h%20d.c.txt` (E parallel c). CSVs `variant_rutile_brownish_red_orange_eperpc.csv` (450-1101 nm) and `variant_rutile_brownish_red_orange_eparc.csv` (460-1101 nm), path 0.0269 cm as in the file header; the index page says 2.69 mm (UNRESOLVED, a factor of 10 on every alpha derived from these files). Also Graves Mountain DV-1 (orange, 0.040 mm, opaque thicker than 0.5 mm): `Rutile%20DV1%20G%20raves%20Mtn%20perp%20c.asc` and `Rutile%20DV1%20Graves%20Mtn%20parallel%20c.asc` -> `variant_rutile_orange_dv1_eperpc.csv`, `variant_rutile_orange_dv1_eparc.csv`.
3. **Band model.** One energy Gaussian per ray describes the edge: fixed centre 382 nm, sigma 3800 cm^-1 (from the slopes of ln A' between 460 and 600 nm: the local slope of ln A' falls from 6.0e-4 to 3.4e-4 per cm^-1 over 3750 cm^-1 of range, giving sigma = 3810 cm^-1 and a centre of about 26,200 cm^-1). Baselines 0.35 (o) and 0.49 (e): these include the reflection loss (n = 2.61 and 2.90: about 0.17 and 0.19 A for two surfaces) plus a NIR plateau of about 0.18 (o) and 0.30 (e) that I did not model (a free-carrier or scattering contribution of unknown origin). NNLS on 460-900 nm: o rms 0.023 A (max 0.046 A; the model is 0.03 A low beyond 700 nm), e rms 0.040 A (max 0.096 A). Peaks for the 0.269 mm path (divide by 10 if the path is really 2.69 mm):

   | ray | center_nm | width sigma cm^-1 | peak cm^-1 | shape |
   |---|---|---|---|---|
   | o | 382 | 3800 | 319 | E |
   | e | 382 | 3800 | 378 | E |

   The e-ray absorbs 1.18 x more than the o-ray (peak ratio; the baseline-subtracted A ratio at 500 nm is 1.21). Using a ratio instead of the absolute numbers makes the variant independent of the thickness conflict; the density is then fixed by the face-up target: alpha(500 nm, o) about 2.5 cm^-1 (T = 0.11 over 8.8 mm) corresponds to scaling the peaks above by 0.03 (assuming 0.269 mm) or 0.3 (assuming 2.69 mm). The 2.5 cm^-1 is `estimated` from "brownish red-orange" at gem size.
4. **Target face-up color.** Brownish red-orange; the e-ray is darker and redder.
5. **Anchor.** None (no composition for the NC sample).
6. **Confidence.** Shape and dichroism: measured. Absolute strength: unresolved thickness. The existing Material Default band (350 nm, sigma 40 nm, peak 8.0) is not related to this edge and would be replaced for this variant.

### 7.3 Orange (Graves Mountain), optional

Same model type. DV-1 is 0.040 mm thick with A = 2.5 at 475-490 nm, so it is much denser per mm than the NC sample (alpha(500 nm) = 2.303 x (1.854-0.33) / 0.004 = 877 cm^-1, o-ray). The edge is slightly less steep. Fit not repeated: one Gaussian of the same centre and sigma is expected to work; the fitting lane should refit it. Target: orange.

### 7.4 Blue (Reduced), optional, estimated

1. **Cause.** Reduction creates Ti3+ (interstitial or near an oxygen vacancy): d-d transitions of octahedral Ti3+ in the visible, a Ti3+-Ti4+ small-polaron system in the near-IR; blue because of an absorption minimum near 21,000 cm^-1 (476 nm) between them (Khomenko et al. 1998 polarized spectra of deep-blue synthetic rutile, via https://collaborate.princeton.edu/en/publications/localized-excitation-of-tisup3sup-ions-in-the-photoabsorption-and/, search summary). Cronemeyer (Phys. Rev. 113, 1222, 1958): hydrogen-reduced crystals show an IR peak at 0.75 eV (1653 nm) for high resistivity and 1.18 eV (1051 nm) for lower resistivity (https://link.aps.org/doi/10.1103/PhysRev.113.1222, abstract via search).
2. **Spectra.** None numeric (the Caltech page shows a synthetic rutile JPG only).
3. **Band model (`estimated`).** The Report gives an edge from 700 nm (0.002 cm^-1/ppm Ti3+, w_o : w_e = 1.0 : 0.7, onset 700, no slope; the repo itself flags the coefficient as low, D31). Proposed: a polaron/NIR Gaussian, centre 1050 nm (the 1.18 eV peak), sigma 2200 cm^-1, peaks 6 cm^-1 (o) and 4.2 cm^-1 (e) (0.7 ratio, Report), plus the intrinsic edge of the Material Default (350 nm, wavelength sigma 40 nm, peak 8.0 per model unit). Rutile blue transmits near 476 nm only if the visible Ti3+ d-d bands are small, which this model assumes. Check: at 700 nm (14286 cm^-1) the Gaussian is exp(-0.5 x (4762/2200)^2) = 0.096 of its peak, so alpha(700 nm, o) = 0.58 cm^-1 and T(700 nm, 8.8 mm) = exp(-0.58 x 0.88) = 0.60.
4. **Target face-up color.** Dark blue, nearly black in the e-ray at high density.
5. **Confidence.** `estimated`, low. Recommend shipping only with the "estimated" tooltip, or dropping it.

## 8. Dropped or conditionally dropped (summary of reasons)

| Item | Decision | Reason |
|---|---|---|
| Quartz Rose (massive) | DROP | Scattering plus dumortierite-type fibres; only diffuse-transmission data (section 3.6) |
| Opal Peruvian Blue-Green | DROP (recommended) | No spectrum; partly scattering; Cu2+ coefficient unphysical (section 4.8.3) |
| CZ V Green | DROP | No band data for V in CZ |
| GGG Cr and Nd variants | DROP | Not gem-trade colors, undefined concentration basis, no spectra (section 5.3) |
| Moissanite Gray | DROP | No documented cause or spectrum |
| Rutile Blue (Reduced) | keep only as estimated, or drop | No numeric spectrum |
| Glass Se Ruby | keep only if calibrated against a Schott curve | Estimated edge |

## 9. Sources (all read in this lane unless marked)

* Caltech Mineral Spectroscopy Server, G. R. Rossman: index http://minerals.gps.caltech.edu/FILES/Visible/Index.html; quartz page https://minerals.gps.caltech.edu/FILES/Visible/quartz/Index.html; ametrine page https://minerals.gps.caltech.edu/Ametrine/Index.html; rutile page https://minerals.gps.caltech.edu/FILES/Visible/Rutile/Index.html; opal page https://minerals.gps.caltech.edu/FILES/Visible/opal/Index.html; diamond page https://minerals.gps.caltech.edu/FILES/Visible/diamond/Index.html; Pearson compendium index (xls only) http://minerals.gps.caltech.edu/FILES/Visible/Pearson_Gems/Index.html. Data files listed per variant above. License: CC BY-NC 4.0, DOI 10.7907/jywr-qq57.
* Breeding, Eaton-Magana et al., Naturally Colored Yellow and Orange Gem Diamonds: The Nitrogen Factor, Gems & Gemology Summer 2020: https://www.gia.edu/gems-gemology/summer-2020-naturally-colored-yellow-orange-diamonds
* Eaton-Magana et al., Natural-Color Pink, Purple, Red, and Brown Diamonds, Gems & Gemology Winter 2018: https://www.gia.edu/gems-gemology/winter-2018-natural-color-pink-purple-red-brown-diamonds
* Eaton-Magana et al., Natural-Color Blue, Gray, and Violet Diamonds, Gems & Gemology Summer 2018: https://www.gia.edu/gems-gemology/summer-2018-natural-color-blue-gray-violet-diamonds
* Rapid determination of single substitutional nitrogen Ns0 concentration in diamond from UV-Vis spectroscopy, arXiv:2207.14278 (Appl. Phys. Lett. 2022): https://arxiv.org/html/2207.14278v2
* Element Six, Coloured diamond, US 7,910,083: https://patents.google.com/patent/US7910083B2/en
* GIA lab notes: https://www.gia.edu/gems-gemology/spring-2019-labnotes-largest-diamond-discovered-in-north-america and https://my.gia.edu/gems-gemology/spring-2016-labnotes-largest-blue-hpht-synthetic-diamond (search summaries)
* Nassau, McClure, Elen, Shigley, Synthetic Moissanite: A New Diamond Substitute, Gems & Gemology 33(4), 1997: https://www.moissaniteco.com/media/view/2018/09/synthetic-moissanite-a-new-diamond-substitute_67.pdf
* Smoky quartz: https://rruff.info/doclib/MinMag/Volume_41/41-319-301.htm and https://www.scielo.br/j/mr/a/NPJGPDBLyFb5Dxy9TttWVpv/ (search summaries)
* Prasiolite: https://link.springer.com/article/10.1007/s00269-016-0864-z, https://rebus.us.edu.pl/bitstream/20.500.12128/3822/1/Czaja_The_Mossbauer_spectra_of_prasiolite_and_amethyst_crystals.pdf, https://reference-global.com/article/10.1515/mipo-2016-0004, https://cms.gia.edu/dam/jcr:98bcbe8f-2256-4710-9f43-cf009433fd81/Satellite-1495880557373.pdf (search summaries)
* Amethyst bands (Cox 1977): https://www.ncbi.nlm.nih.gov/pmc/articles/PMC7483767/
* Opal: https://www2.mdpi.com/2073-4352/12/3/322 (search summary; fetch failed), https://air.unimi.it/handle/2434/150286, https://www.gia.edu/gems-gemology/spring-2024-gemnews-blue-opal-from-chile, https://www.gemdat.org/gem-43544.html, https://rapaport.com/?p=44613
* CZ: https://jogg.cug.edu.cn/article/id/95388769-5a3e-4668-b0f0-1a0ec24e773f, https://gia.edu/gems-gemology/summer-2017-gemnews-holmium-synthetic-cz (search summaries)
* YAG: arXiv:2603.18664, arXiv:2603.18673, https://apps.dtic.mil/sti/pdfs/ADP011875.pdf, https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/9651483 (search summaries)
* Glass: https://www.britglass.org.uk/knowledge-base/digital-library-and-information-services/effect-alkali-ions-optical-absorption, https://kirim.kmutt.ac.th/converis/portal/detail/Publication/600362377?lang=en_GB, https://oa.tib.eu/renate/items/4645b348-ddcf-4719-9a64-920775412cfc, https://www.thaiscience.info/journals/Article/TKJN/10974416.pdf (search summaries)
* Rutile: https://link.aps.org/doi/10.1103/PhysRev.113.1222 and https://collaborate.princeton.edu/en/publications/localized-excitation-of-tisup3sup-ions-in-the-photoabsorption-and/ (search summaries)
* Repo data (secondary): `docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md`, `docs/chromophores/corundum_chrysoberyl_spinel_garnet-synthetics.md`, `docs/chromophores/research-2026-10-polarized-spectra.md`, `research-2026-10-polarized-spectra-2.md`, `research-2026-10-primary-data.md`, `research-2026-10-validation.md`, `calibration-records.md`, `crates/pleochroma/data/reference_spectra/`.
