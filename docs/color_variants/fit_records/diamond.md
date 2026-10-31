# Fit records: diamond color variants (lane F5, 2026-10-27)

Data: `crates/pleochroma/data/color_variants/diamond.toml`. Tests: `crates/pleochroma/src/optics/materials/variants/tests_diamond.rs`. Research: `docs/color_variants/R5_diamond.md` (measured parameters) and `R4_quartz_diamond_opal_synthetics_glass.md` section 4 (band models).

## Common to all seven

* **No fit to a measured spectrum.** No public numeric visible spectrum of a colored natural diamond exists (R5 section 1). Every entry is built from measured band positions, widths and cross-section ratios, with the strength set by the target face-up color, and is `estimated = true`. The reported residual is therefore the CIEDE2000 distance of the reached face-up color to the target color of the grade name, not a spectral residual.
* **Targets.** Diamond color grades have no published CIELAB values. Each target L*C*h is an estimate for the named grade (R4 / R5 target names), limited to what a light-transmission model can show: a weakly absorbing stone stays near L* 99, and the hue of each variant is largely fixed by the measured band centres, not tuned.
* **Path.** Cubic, isotropic: `pleochroic = false`, o_ray = e_ray, so the face-up color, the o-ray, the e-ray and the unpolarised color are the same. Unit `per_mm`. The colors are computed exactly as the app does, `crate::color::body_color::body_colors(tensor, path, Illuminant::D65)`, over the face-up path of a 7 mm stone for a per-mm tensor: `variants::face_up_swatch_path(PerMm, 7.0)` = `MODEL_UNIT_FACE_UP_PATH` x 7 / `REFERENCE_MODEL_WIDTH` = 2.83 x 7 / 2.0 = **9.905 mm** (not the 8.8 mm of R4's checks, which used the old 2.52 constant). The Python reproduction of the pipeline (CIE 1931 table and D65 read from the repo source, 1 nm, 380-780 nm) matches the Sapphire, Ruby and Emerald swatches recorded in `docs/chromophores/calibration-records.md` (L* 56.7 / C* 39.4 / h 282.5 against 57 / 40 / 283; 40.5 / 67.7 / 355.6 against 41 / 67 / 356; 64.8 / 99.6 / 167.8 against 65 / 101 / 168) before it was used.
* **Band budget.** Cape Yellow 5 bands, Canary 5, Brown 5, Pink 7, Purple-Pink 2, Blue 1, Green 7 (at most 8 per ray). Peaks are mm^-1 = cm^-1 / 10; widths are sigma in cm^-1 (energy Gaussians); every entry stores the same bands in o_ray and e_ray.
* **`reference_path_mm = 7.0`** is the stone width the numbers are calibrated for (the shared brief), not the light path: the variants code reads the face-up swatch over `variants::face_up_swatch_path(PerMm, 7.0)`, the same 9.905 mm, so the swatch and the render agree.
* **Narrow lines.** The N3 ZPL (sigma 120 cm^-1, 2 nm), N2 line (150 cm^-1) and GR1 ZPL (60 cm^-1) are narrow; the CPU colors here sample at 1 nm and resolve them. A render that samples wavelengths coarsely could alias them; the lines do not carry the hue (the 741 nm line is outside the visible weighting).

## Update 2026-10-27 (fix round X6)

Applies to every entry below; the per-entry sections were corrected where the data changed.

* **Estimated flag (owner decision O3, 2026-10-27).** `estimated = true` when the spectral shape is not a measured spectrum of that variety, OR the strength is not anchored to a measurement (a chosen strength counts: the Cornflower / Deep Blue / Pigeon's Blood convention of lane F1 now holds in every group); `estimated = false` only when the shape is a measured spectrum of that very variety at a strength that is itself measured or published. The first `sources` line of every entry is ONE short plain sentence that names the estimated part; the UI shows it as the tooltip "Estimated: <first line>" (no tooltip for a measured entry). Citations, URLs, DOIs and repository paths start on the second line.
* **Face-up color (decision D-G).** The swatches and pins of the Variants list show the ORIENTED face-up color, the color the renderer shows for the stone: with the c axis along the view the o-ray alone (uniaxial hosts with `c_axis` Y: Sapphire, Ruby, Zircon, Benitoite, Emerald, Morganite, Amethyst, Quartz, Rutile, Moissanite), (o + e) / 2 for the hosts whose c axis lies in the table plane (Aquamarine and Tourmaline, `c_axis` X), (alpha + beta) / 2 for biaxial hosts (gamma along `c_axis` Y: Topaz, Chrysoberyl, Alexandrite, Tanzanite, Peridot, Andalusite); isotropic hosts show the one color. The entry sections below keep the unpolarised mean (the fit target, what `body_colors` returns) in their "reached" lines and list the oriented color as "reached face-up (oriented ...)"; the tests pin both. The face-up path is 9.905 mm (`reference_path_mm` = the 7 mm stone width, decision D-A).
* **Colorless entries (decision D-C).** An entry with no bands is valid on any host; the `pleochroic = true` trick was removed from the colorless entries.
* **Measured entries in this file** (`estimated = false`): none. Every other one of the 7 entries is estimated.
* **F-12 Canary.** The visible continuum uses an exponential with a 45 nm e-fold (R4 gave 60 nm; neither is measured, the Report value is secondary). alpha(425 nm) = 0.44 per mm was verified; the same strength with 60 nm gives 82 / 76 / 85 (orange-yellow). Shipped as estimated, documented here and in the entry's sources.
* **F-17.** Citations re-checked: Optics Letters 49(1) 137 (2024), APL 121 064002 (2022) / arXiv:2207.14278 (4.51 cm^-1 per ppm confirmed), US7910083B2, Molecules 25, 3743 (2020) are real; the 3.54 / 2.43 eV centres and the alpha(425) / alpha(270) = 0.1 derivation from the patent are unverified and marked so.

**Tooltip sentences and oriented face-up colors** (D65, 9.905 mm):

| key | host | estimated | tooltip sentence | face-up (oriented) L* / C* / h | unpolarised mean L* / C* / h |
|---|---|---|---|---|---|
| `diamond.cape_yellow` | Diamond | yes | Built from published band positions; strength estimated. | 99.2 / 21.9 / 108 | 99.2 / 21.9 / 108 |
| `diamond.canary` | Diamond | yes | Built from published band positions; the visible continuum and the strength are estimated. | 88.6 / 71.7 / 89 | 88.6 / 71.7 / 89 |
| `diamond.brown` | Diamond | yes | Built from published band positions; strength estimated. | 49.7 / 40.8 / 68 | 49.7 / 40.8 / 68 |
| `diamond.pink` | Diamond | yes | Built from published band positions; strength estimated. | 79.0 / 39.3 / 329 | 79.0 / 39.3 / 329 |
| `diamond.purple_pink` | Diamond | yes | Built from published band positions; strength estimated. | 76.0 / 45.8 / 307 | 76.0 / 45.8 / 307 |
| `diamond.blue` | Diamond | yes | Built from published band positions; strength estimated. | 76.5 / 39.4 / 245 | 76.5 / 39.4 / 245 |
| `diamond.green` | Diamond | yes | Built from published band positions; strength estimated. | 82.0 / 61.8 / 171 | 82.0 / 61.8 / 171 |


## Cape Yellow

Key `diamond.cape_yellow`. Cause: N3 (three nitrogen around a vacancy) and N2 of type Ia(AB) diamond.

**Sources (measured parameters).**

* N3 zero-phonon line (ZPL) 415.2 nm; peak cross section of the ZPL about 2.5e-15 cm^2, of the 370-390 nm vibronic band about 1e-15 cm^2 (Optics Letters 49(1), 137, 2024). The ZPL : sideband peak ratio 2.5 : 1 fixes the 415.2 nm line (peak 1.00) against the 385 nm sideband (peak 0.40).
* N2 line 478 nm with associated absorptions at 451 and 463 nm (Breeding et al., G&G Summer 2020; MSA review 2022).

**Estimated.**

* The N2 heights relative to N3 (478 nm line 0.45, 451-463 nm lump 0.375 of the N3 ZPL peak; R4 section 4.2 had 0.50 / 0.375) and the 330 nm band (0.60) for the blue-violet side of the Ia edge.
* The line widths: the ZPL sigma 120 cm^-1 (FWHM about 5 nm, a room-temperature width; the line is narrower than the 637 cm^-1 of R4's Report value) and the N2 line sigma 150 cm^-1.
* The absolute strength (below).

**Bands** (o_ray = e_ray; sigma in cm^-1, peak in cm^-1 = 10 x the stored mm^-1):

| center_nm | sigma cm^-1 | peak cm^-1 | peak mm^-1 (stored) |
|---|---|---|---|
| 415.2 | 120 | 1 | 0.1 |
| 385 | 1000 | 0.4 | 0.04 |
| 478 | 150 | 0.45 | 0.045 |
| 450 | 1100 | 0.375 | 0.0375 |
| 330 | 3500 | 0.6 | 0.06 |

**Scaling.** One scale factor on the whole set: N3 ZPL peak 1.0 cm^-1 (0.10 mm^-1), i.e. T(415 nm) = 0.28 over the 9.905 mm path (with the overlapping sidebands), T(478 nm) = 0.52. Chosen as the strength at which the face-up chroma is that of a light yellow; the dE2000 to the target is 1.97, all of it hue (the N3 / N2 line set gives a lemon-leaning 108 degrees, the target is 103 degrees).

**Target.** light yellow, L* 98, C* 22, h 103 (stone-color grade name, no published CIELAB: the lightness of a weak absorber stays near 99).

**Reached (D65, 9.905 mm, all directions).** L* 99.18, C* 21.94, h 108.15; Lab (99.18, -6.84, 20.85); dE2000 to the target L* 98 / C* 22 / h 103: 1.97.

**Per direction.** Cubic: o-ray, e-ray and unpolarised are the same color; no pleochroism is documented for diamond.

## Canary

Key `diamond.canary`. Cause: Isolated substitutional nitrogen (Ns0, C centre) of type Ib diamond.

**Sources (measured parameters).**

* 270 nm band: Gaussian, centre 268-272 nm, RMS width 13-27 nm, cross section 4.51 +/- 0.35 cm^-1 per ppm (natural log), calibrated against EPR (Luo et al., APL 121, 064002, 2022). Used as centre 270 nm, sigma 20 nm = 2743 cm^-1, with alpha(270) = 10 x alpha(425).
* The visible continuum reaches about 510 nm for yellow, about 600 nm for orange (Breeding et al. 2020); alpha(425) / alpha(270) = 0.1 from the ratio of the ranges in the Element Six patent US7910083B2.

**Estimated.**

* The shape of the visible continuum: alpha(425) exp(-(lambda - 425)/tau). R4 used tau = 60 nm (the Report's default); this fit takes tau = 45 nm so that a vivid canary stays yellow (hue 89 degrees) instead of going orange (the best fit with tau 60 reaches only hue 83 degrees at C* 55, dE2000 6.7 to the target). The exponential is fitted by four energy Gaussians (centres 340 / 400 / 460 / 530 nm, sigmas 2500 / 1800 / 1500 / 1200 cm^-1, amplitudes 4.16 / 0.94 / 0.21 / 0.08 times alpha(425)) by relative-error NNLS over 370-640 nm: the relative error is -6 % at 380 nm, 0 at 425 nm, -7 % at 450 nm, -9 % at 500 nm and +22 % at 550 nm (maximum 26 % within 370-640 nm).
* The absolute strength (below).

**Bands** (o_ray = e_ray; sigma in cm^-1, peak in cm^-1 = 10 x the stored mm^-1):

| center_nm | sigma cm^-1 | peak cm^-1 | peak mm^-1 (stored) |
|---|---|---|---|
| 270 | 2743 | 42.76 | 4.276 |
| 340 | 2500 | 17.7796 | 1.77796 |
| 400 | 1800 | 4.00617 | 0.400617 |
| 460 | 1500 | 0.882254 | 0.0882254 |
| 530 | 1200 | 0.328678 | 0.0328678 |

**Scaling.** One scale factor: alpha(425 nm) = 4.28 cm^-1, alpha(270 nm) = 42.8 cm^-1 (about 9.5 ppm Ns0 at 4.51 cm^-1 per ppm; the GIA statement is under 100 ppma for type Ib and a few ppm for strong color). R4's 1.1 cm^-1 (2.4 ppm) gives only C* 27 in the app's pipeline over the 9.905 mm path, a pale yellow, so the strength is set by the target. The fitted set itself gives alpha(425) = 4.4 cm^-1 (3 % above the nominal), T(425 nm) = 0.013 over the path: the stone is nearly opaque below about 420 nm, as a vivid canary is.

**Target.** Fancy Intense to Vivid yellow, L* 86, C* 72, h 90 (grade name, no published CIELAB).

**Reached (D65, 9.905 mm, all directions).** L* 88.55, C* 71.73, h 88.55; Lab (88.55, 1.82, 71.71); dE2000 to the target L* 86 / C* 72 / h 90: 1.97.

**Per direction.** Cubic: o-ray, e-ray and unpolarised are the same color; no pleochroism is documented for diamond.

## Brown

Key `diamond.brown`. Cause: Vacancy clusters (40-60 vacancies) from plastic deformation: a brown absorption continuum that rises toward short wavelengths.

**Sources (measured parameters).**

* The decomposition of measured brown spectra into a 360 nm Gaussian (vacancy clusters), a 520 nm Gaussian (NVH0) and a ramp R x lambda^-3 (Luo et al. 2022, section 2): used as a 360 nm band, a 510 nm band (patent value 2.43 eV, the NVH0 band is at 520 nm) of the same sigma 3425 cm^-1 (FWHM 1 eV) and a lambda^-3 ramp with lambda in micrometres.
* Element Six patent US7910083B2: peaks 1.5-6 cm^-1 (360 nm band) and 0.4-2 cm^-1 (510 nm band) preferred, ramp coefficient c below 0.2: the fitted 4.23 / 1.41 cm^-1 and c = 0.08 lie inside all three ranges. The patent describes CVD material, and GIA gives no functional form for natural stones.

**Estimated.**

* That a natural stone has the same decomposition; the band heights and the ramp coefficient (c = 0.08, set small so the bands set the brown hue).
* The absolute strength (below).

**Bands** (o_ray = e_ray; sigma in cm^-1, peak in cm^-1 = 10 x the stored mm^-1):

| center_nm | sigma cm^-1 | peak cm^-1 | peak mm^-1 (stored) |
|---|---|---|---|
| 284.706 | 4886.15 | 3.17547 | 0.317547 |
| 385.821 | 4546.68 | 0.645945 | 0.0645945 |
| 496.754 | 5967.27 | 0.33799 | 0.033799 |
| 360 | 3425 | 4.239 | 0.4239 |
| 510 | 3425 | 1.413 | 0.1413 |

**Scaling.** The ramp c x lambda^-3 is approximated by three energy Gaussians fitted to c = 1 over 380-700 nm by relative-error NNLS (centres 284.7 / 385.8 / 496.8 nm, sigmas 4886 / 4547 / 5967 cm^-1, peaks 39.69 / 8.07 / 4.22 cm^-1 times c): maximum relative error 0.6 %. With the ramp fixed at c = 0.08, one factor k scales the two bands (360 nm peak 3 k, 510 nm peak k): k = 1.41. A pure lambda^-3 ramp cannot reach brown hue 65 degrees (it gives 75 at L* 50); the 510 nm band moves it to 68. Total absorption 4.7 / 2.4 / 1.4 cm^-1 at 400 / 500 / 600 nm.

**Target.** medium brown, L* 50, C* 40, h 65 (grade name, no published CIELAB).

**Reached (D65, 9.905 mm, all directions).** L* 49.70, C* 40.85, h 68.06; Lab (49.70, 15.26, 37.89); dE2000 to the target L* 50 / C* 40 / h 65: 1.65.

**Per direction.** Cubic: o-ray, e-ray and unpolarised are the same color; no pleochroism is documented for diamond.

## Pink

Key `diamond.pink`. Cause: The 550 nm band of plastically deformed type Ia diamond, with the weak N3 that fancy and deeper tones carry.

**Sources (measured parameters).**

* Band 548 nm, FWHM 62 nm (Fancy Deep pinkish orange example, with H3 and N3) = 2065 cm^-1, sigma 877 cm^-1 (Eaton-Magana et al., G&G Winter 2018; fitted on 50 diamonds, centres 545-565 nm, FWHM 60-100 nm).
* The smaller, narrower UV companion at about 390 nm; faint colors show only the 550 nm band, fancy and deeper tones add N3 and/or H3 (same paper).

**Estimated.**

* The 390 nm companion height 0.15 of the 550 nm peak (R4 4.5, from the Report's 400 nm edge ratio) and its sigma 1200 cm^-1.
* The N3 / N2 content: the Cape Yellow band set at 0.6 times (N3 ZPL 0.6 cm^-1). Without it the single 548 nm band gives hue 320 degrees (magenta-pink); the N3 yellow absorption moves the hue to 329 degrees, a pink of the Fancy Intense class. H3 (503 nm) is not modelled.
* The absolute strength (below).

**Bands** (o_ray = e_ray; sigma in cm^-1, peak in cm^-1 = 10 x the stored mm^-1):

| center_nm | sigma cm^-1 | peak cm^-1 | peak mm^-1 (stored) |
|---|---|---|---|
| 548 | 876.74 | 1.246 | 0.1246 |
| 390 | 1200 | 0.1869 | 0.01869 |
| 415.2 | 120 | 0.6 | 0.06 |
| 385 | 1000 | 0.24 | 0.024 |
| 478 | 150 | 0.27 | 0.027 |
| 450 | 1100 | 0.225 | 0.0225 |
| 330 | 3500 | 0.36 | 0.036 |

**Scaling.** N3 / N2 set fixed at 0.6 times Cape Yellow; one factor on the 548 nm band: peak 1.25 cm^-1, T(548 nm) = 0.29 over the path. The strength is chosen by the target (the Report's 1.0 cm^-1 per unit, typical 0.1-3, is the order of magnitude).

**Target.** Fancy Intense pink, L* 78, C* 38, h 330 (grade name, no published CIELAB; the band positions limit the hue to the 320-335 degree range).

**Reached (D65, 9.905 mm, all directions).** L* 79.04, C* 39.33, h 329.27; Lab (79.04, 33.81, -20.10); dE2000 to the target L* 78 / C* 38 / h 330: 0.92.

**Per direction.** Cubic: o-ray, e-ray and unpolarised are the same color; no pleochroism is documented for diamond.

## Purple-Pink

Key `diamond.purple_pink`. Cause: The 550 nm band of plastically deformed type Ia diamond at the long-wavelength, broad end, without significant H3 or N3 (about 98 % of unmodified purples show none).

**Sources (measured parameters).**

* Band 556 nm, FWHM 105 nm (Fancy Vivid purple example) = 3396 cm^-1, sigma 1443 cm^-1; purple-pink 555-560 nm / 70-100 nm (Eaton-Magana et al. 2018). No N3 or H3.
* The 390 nm companion band (same paper).

**Estimated.**

* The companion height 0.15 of the 556 nm peak and its sigma 1200 cm^-1.
* The absolute strength (below).

**Bands** (o_ray = e_ray; sigma in cm^-1, peak in cm^-1 = 10 x the stored mm^-1):

| center_nm | sigma cm^-1 | peak cm^-1 | peak mm^-1 (stored) |
|---|---|---|---|
| 556 | 1442.39 | 1.006 | 0.1006 |
| 390 | 1200 | 0.1509 | 0.01509 |

**Scaling.** One factor on the whole set: 556 nm peak 1.01 cm^-1, T(556 nm) = 0.37 over the path. The hue is fixed by the band centre and nearly the same at every strength (305-312 degrees for peaks 0.1-4 cm^-1); a centre scan 545-565 nm and a width scan 62-105 nm changes it by at most 25 degrees, so the 556 nm / 105 nm of the lead's brief gives a purple-pink at the purple end (306.9 degrees). The Pink variant (320-330 degrees) is the pink end.

**Target.** Fancy purple-pink, L* 76, C* 46, h 308 (grade name; the hue is the band's, not a free target).

**Reached (D65, 9.905 mm, all directions).** L* 76.01, C* 45.78, h 306.85; Lab (76.01, 27.46, -36.63); dE2000 to the target L* 76 / C* 46 / h 308: 0.63.

**Per direction.** Cubic: o-ray, e-ray and unpolarised are the same color; no pleochroism is documented for diamond.

## Blue

Key `diamond.blue`. Cause: Uncompensated boron acceptor of type IIb diamond; the visible color is the long-wavelength tail of the photoionisation continuum, rising from the blue window into the red.

**Sources (measured parameters).**

* Natural type IIb boron 25-1500 ppb (Faint to Fancy Vivid blue); higher boron means a higher absorption gradient and a stronger blue (Eaton-Magana et al., G&G Summer 2018). Only infrared calibrations exist ([B0] = 0.0350 x H(2800 cm^-1) and similar); the visible continuum is published only qualitatively.

**Estimated.**

* The shape: one Gaussian centred at 1000 nm, sigma 4000 cm^-1 (R4 4.6), standing for the tail. A centre scan 760-1100 nm (sigma scaled with it) moves the hue from 212 to 256 degrees at a peak of 4 cm^-1; 1000 nm lands on the blue side (245 degrees) at the strength below.
* The absolute strength (below). The gray component below 500 nm that plastic deformation adds is not modelled.

**Bands** (o_ray = e_ray; sigma in cm^-1, peak in cm^-1 = 10 x the stored mm^-1):

| center_nm | sigma cm^-1 | peak cm^-1 | peak mm^-1 (stored) |
|---|---|---|---|
| 1000 | 4000 | 5.096 | 0.5096 |

**Scaling.** One factor: peak 5.1 cm^-1 at 1000 nm (alpha(700 nm) = 2.9 cm^-1, alpha(600 nm) = 1.3 cm^-1), T(700 nm) = 0.06 over the path. Against R4's boron anchor (0.5 ppm gives peak 1.56 cm^-1) this is about 1.6 ppm, just above the 25-1500 ppb range of natural stones: R4's calibration is `secondary` with an unresolved factor of 10, so the strength is set by the target, not by a ppm figure.

**Target.** Fancy blue, L* 76, C* 38, h 244 (grade name, no published CIELAB).

**Reached (D65, 9.905 mm, all directions).** L* 76.53, C* 39.44, h 244.78; Lab (76.53, -16.80, -35.68); dE2000 to the target L* 76 / C* 38 / h 244: 0.66.

**Per direction.** Cubic: o-ray, e-ray and unpolarised are the same color; no pleochroism is documented for diamond.

## Green

Key `diamond.green`. Cause: Neutral vacancy V0 (GR1) from irradiation, with the N3 / N2 absorption that green-blue stones carry.

**Sources (measured parameters).**

* GR1 zero-phonon line 741 nm, vibronic band reaching about 575 nm, band height referenced at 2.0 eV = 620 nm (MSA review 2022; the 2003 annealing study): the band maximum is put at 640 nm (R5 section 3: 620-650 nm, R4's 680 nm moved), sigma 1250 cm^-1 (FWHM 120 nm), and a 741 nm line.
* N3 415.2 nm and N2 478 nm in green-blue stones (G&G Summer 2018).

**Estimated.**

* The ZPL height (0.8 of the band peak) and width (sigma 60 cm^-1, FWHM 7.7 nm, a room-temperature width; the line lies beyond the eye's sensitivity, so it does not change the color).
* The blue-side absorption: the Cape Yellow set at 2.0 times (N3 ZPL peak 2.0 cm^-1). R4 4.7 had 0.3 times; with that much the stone is cyan (hue 205-215 degrees) because GR1 alone absorbs only red. A Fancy green needs the blue side absorbed too; the carrier here is the N3 / N2 set (H3 503 nm, common in green diamonds, is not modelled).
* The absolute strength (below).

**Bands** (o_ray = e_ray; sigma in cm^-1, peak in cm^-1 = 10 x the stored mm^-1):

| center_nm | sigma cm^-1 | peak cm^-1 | peak mm^-1 (stored) |
|---|---|---|---|
| 640 | 1250 | 2.136 | 0.2136 |
| 741 | 60 | 1.7088 | 0.17088 |
| 415.2 | 120 | 2 | 0.2 |
| 385 | 1000 | 0.8 | 0.08 |
| 478 | 150 | 0.9 | 0.09 |
| 450 | 1100 | 0.75 | 0.075 |
| 330 | 3500 | 1.2 | 0.12 |

**Scaling.** N3 / N2 set fixed at 2.0 times Cape Yellow; GR1 band peak 2.14 cm^-1 (about 3.6 ppm V0 at R4's 0.6 cm^-1 per ppm; R4's 2.0 cm^-1 stands). T(640 nm) = 0.12 over the path.

**Target.** Fancy green, L* 80, C* 58, h 170 (grade name, no published CIELAB: bluish green to green; Fancy green stones span hues of roughly 135-175).

**Reached (D65, 9.905 mm, all directions).** L* 82.00, C* 61.85, h 170.54; Lab (82.00, -61.01, 10.16); dE2000 to the target L* 80 / C* 58 / h 170: 1.75.

**Per direction.** Cubic: o-ray, e-ray and unpolarised are the same color; no pleochroism is documented for diamond.
