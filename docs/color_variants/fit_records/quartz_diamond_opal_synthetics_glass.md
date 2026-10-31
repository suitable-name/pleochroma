# Fit records: quartz, opal, synthetics, glass, rutile (lane F4, 2026-10-27)

Entries of `crates/pleochroma/data/color_variants/quartz_diamond_opal_synthetics_glass.toml` (31 entries; Diamond is the lead's). Research: `docs/color_variants/R4_quartz_diamond_opal_synthetics_glass.md` and its lane report. Tests: `crates/pleochroma/src/optics/materials/variants/tests_quartz_diamond_opal_synthetics_glass.rs`, which pins every Lab value below to dE2000 1.0.

## Update 2026-10-27 (fix round X6)

Applies to every entry below; the per-entry sections were corrected where the data changed.

* **Estimated flag (owner decision O3, 2026-10-27).** `estimated = true` when the spectral shape is not a measured spectrum of that variety, OR the strength is not anchored to a measurement (a chosen strength counts: the Cornflower / Deep Blue / Pigeon's Blood convention of lane F1 now holds in every group); `estimated = false` only when the shape is a measured spectrum of that very variety at a strength that is itself measured or published. The first `sources` line of every entry is ONE short plain sentence that names the estimated part; the UI shows it as the tooltip "Estimated: <first line>" (no tooltip for a measured entry). Citations, URLs, DOIs and repository paths start on the second line.
* **Face-up color (decision D-G).** The swatches and pins of the Variants list show the ORIENTED face-up color, the color the renderer shows for the stone: with the c axis along the view the o-ray alone (uniaxial hosts with `c_axis` Y: Sapphire, Ruby, Zircon, Benitoite, Emerald, Morganite, Amethyst, Quartz, Rutile, Moissanite), (o + e) / 2 for the hosts whose c axis lies in the table plane (Aquamarine and Tourmaline, `c_axis` X), (alpha + beta) / 2 for biaxial hosts (gamma along `c_axis` Y: Topaz, Chrysoberyl, Alexandrite, Tanzanite, Peridot, Andalusite); isotropic hosts show the one color. The entry sections below keep the unpolarised mean (the fit target, what `body_colors` returns) in their "reached" lines and list the oriented color as "reached face-up (oriented ...)"; the tests pin both. The face-up path is 9.905 mm (`reference_path_mm` = the 7 mm stone width, decision D-A).
* **Colorless entries (decision D-C).** An entry with no bands is valid on any host; the `pleochroic = true` trick was removed from the colorless entries.
* **Measured entries in this file** (`estimated = false`): none. Every other one of the 31 entries is estimated.
* **F-13 target-fitted edits** (`green_tint` edge x 4, `ce_orange` 500 nm band cut to 0.07 wt%, `mn_purple` centre 500 -> 510 nm): all from a secondary source and all `estimated = true`; kept and flagged as the review accepts.
* **F-19 Amethyst e-ray.** The e-ray of `amethyst.deep_siberian` and `rose_de_france` (and of the Amethyst Material Default) is an unmeasured ray built with the Report's per-band weights 1.0 / 1.0 / 0.7 / 0.5; the data say so in the first source line.

**Tooltip sentences and oriented face-up colors** (D65, 9.905 mm):

| key | host | estimated | tooltip sentence | face-up (oriented) L* / C* / h | unpolarised mean L* / C* / h |
|---|---|---|---|---|---|
| `amethyst.deep_siberian` | Amethyst | yes | o-ray measured (Anahi, Bolivia); e-ray and strength estimated. | 31.0 / 47.8 / 330 | 36.2 / 42.2 / 333 |
| `amethyst.rose_de_france` | Amethyst | yes | o-ray measured (Anahi, Bolivia); e-ray and strength estimated. | 87.0 / 12.3 / 327 | 88.2 / 10.5 / 329 |
| `citrine.madeira` | Citrine | yes | Shape refitted from a secondary table; strength estimated. | 49.3 / 95.0 / 62 | 49.3 / 95.0 / 62 |
| `quartz.smoky` | Quartz | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 2918). | 58.1 / 13.8 / 60 | 54.9 / 15.4 / 65 |
| `quartz.pink_crystalline` | Quartz | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 2352). | 80.3 / 13.7 / 2 | 80.8 / 11.4 / 2 |
| `quartz.prasiolite` | Quartz | yes | Built from published band positions; strength estimated. | 93.8 / 36.0 / 133 | 93.8 / 36.0 / 133 |
| `opal.fire_orange` | Opal | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 1824 cherry opal). | 67.9 / 104.2 / 66 | 67.9 / 104.2 / 66 |
| `opal.fire_red` | Opal | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 1824 cherry opal). | 38.4 / 89.0 / 48 | 38.4 / 89.0 / 48 |
| `rutile.brownish_red_orange` | Rutile | yes | Strength chosen for a typical face-up color; spectrum measured (rutile 250-2h d). | 42.2 / 76.2 / 68 | 39.7 / 72.5 / 67 |
| `rutile.orange` | Rutile | yes | Strength chosen for a typical face-up color; spectrum measured (rutile DV-1). | 55.3 / 93.4 / 71 | 54.5 / 93.5 / 70 |
| `rutile.blue_reduced` | Rutile | yes | Built from published band positions; strength estimated. | 37.9 / 64.2 / 280 | 42.1 / 58.0 / 274 |
| `cubic_zirconia.ce_yellow` | Cubic Zirconia | yes | Built from published band positions; strength estimated. | 85.0 / 45.2 / 85 | 85.0 / 45.2 / 85 |
| `cubic_zirconia.ce_orange` | Cubic Zirconia | yes | Built from published band positions; strength estimated. | 68.4 / 65.3 / 73 | 68.4 / 65.3 / 73 |
| `cubic_zirconia.er_pink` | Cubic Zirconia | yes | Built from published band positions; strength estimated. | 93.7 / 20.8 / 337 | 93.7 / 20.8 / 337 |
| `cubic_zirconia.nd_lilac` | Cubic Zirconia | yes | Built from published band positions; strength estimated. | 92.5 / 13.7 / 294 | 92.5 / 13.7 / 294 |
| `cubic_zirconia.cr_green` | Cubic Zirconia | yes | Built from published band positions; strength estimated. | 81.1 / 28.4 / 174 | 81.1 / 28.4 / 174 |
| `yag.nd_lilac` | YAG | yes | Built from published band positions; strength estimated. | 92.7 / 12.6 / 263 | 92.7 / 12.6 / 263 |
| `yag.cr_green` | YAG | yes | Built from published band positions; strength estimated. | 69.2 / 74.5 / 169 | 69.2 / 74.5 / 169 |
| `yag.ce_yellow` | YAG | yes | Built from published band positions; strength estimated. | 91.7 / 103.8 / 90 | 91.7 / 103.8 / 90 |
| `synthetic_moissanite.near_colorless` | Synthetic Moissanite | yes | Built from published band positions; strength estimated. | 98.9 / 9.2 / 103 | 98.9 / 9.2 / 103 |
| `synthetic_moissanite.green_tint` | Synthetic Moissanite | yes | Built from published band positions; strength estimated. | 85.2 / 38.5 / 159 | 87.2 / 34.2 / 150 |
| `glass__n_bk7_.co_blue` | Glass (N-BK7) | yes | Built from published band positions; strength estimated. | 47.9 / 92.1 / 286 | 47.9 / 92.1 / 286 |
| `glass__n_bk7_.cr_green` | Glass (N-BK7) | yes | Built from published band positions; strength estimated. | 77.3 / 111.7 / 138 | 77.3 / 111.7 / 138 |
| `glass__n_bk7_.mn_purple` | Glass (N-BK7) | yes | Built from published band positions; strength estimated. | 50.4 / 89.4 / 344 | 50.4 / 89.4 / 344 |
| `glass__n_bk7_.se_ruby` | Glass (N-BK7) | yes | Built from published band positions; strength estimated. | 40.4 / 102.1 / 43 | 40.4 / 102.1 / 43 |
| `glass__n_bk7_.se_pink` | Glass (N-BK7) | yes | Built from published band positions; strength estimated. | 94.1 / 15.3 / 26 | 94.1 / 15.3 / 26 |
| `glass__f2_.co_blue` | Glass (F2) | yes | Built from published band positions; strength estimated. | 47.9 / 92.1 / 286 | 47.9 / 92.1 / 286 |
| `glass__f2_.cr_green` | Glass (F2) | yes | Built from published band positions; strength estimated. | 77.3 / 111.7 / 138 | 77.3 / 111.7 / 138 |
| `glass__f2_.mn_purple` | Glass (F2) | yes | Built from published band positions; strength estimated. | 50.4 / 89.4 / 344 | 50.4 / 89.4 / 344 |
| `glass__f2_.se_ruby` | Glass (F2) | yes | Built from published band positions; strength estimated. | 40.4 / 102.1 / 43 | 40.4 / 102.1 / 43 |
| `glass__f2_.se_pink` | Glass (F2) | yes | Built from published band positions; strength estimated. | 94.1 / 15.3 / 26 | 94.1 / 15.3 / 26 |


## Method (all entries)

* **Units.** Every entry is `per_mm`, `reference_path_mm = 7.0` (the stone width the numbers are for). Peaks are the Napierian absorption coefficient per millimetre; energy rows carry sigma in cm^-1 (FWHM / 2.35482), the single wavelength row (rutile edge) sigma in nm.
* **Path used for every color below.** The renderer's face-up path of a 7 mm stone for a `per_mm` tensor: `MODEL_UNIT_FACE_UP_PATH` (2.83) x 7 mm / `REFERENCE_MODEL_WIDTH` (2.0) = **9.905 mm** (`render_setup::absorption_path_scale_for` gives 3.5 mm per model unit; the tracer's face-up path is that times 2.83; the tests use `variants::face_up_swatch_path(PerMm, 7.0)`). R4's arithmetic used 8.8 mm (the old constant 2.52); the strengths below are set at 9.905 mm.
* **Color pipeline.** `color::body_color::body_colors` reproduced in Python (the 5 nm CIE 1931 table and the 10 nm D65 table read from the repo source, 1 nm grid 380-780 nm, Lab relative to the illuminant's own white, mixture (2 T_o + T_e) / 3 for the unpolarised color, CIEDE2000 as in `delta_e_2000`). Check against the built-in swatches of `docs/chromophores/calibration-records.md` (ModelUnit tensors over one unit, unpolarised): Sapphire L*/C*/h 56.7 / 39.4 / 282.5 (record 57 / 40 / 283), Ruby 40.5 / 67.7 / 355.6 (record 41 / 67 / 356), Emerald 64.8 / 99.6 / 167.8 (record 65 / 101 / 168).
* **Baselines.** Caltech spectra are decadic absorbance as recorded; reflection loss and baseline are removed with a constant A offset before the bands are fitted: the Fresnel loss of two surfaces with multiple reflections, A = -log10((1-R)/(1+R)) = 0.040 (quartz o), 0.041 (quartz e), 0.175 (rutile o, n 2.61), 0.210 (rutile e, n 2.90); the amethyst slab shows a flat 0.217 A plateau and the opal plates 0.085 / 0.10 A (reflection plus scatter), which are removed as fitted / read from the plateau. Napierian alpha = 2.303 A / path.
* **Fits.** Bounded nonnegative least squares of energy Gaussians (scipy `least_squares`), at most 8 bands per ray (here at most 5). Scripts and outputs are in the lane's scratch folder, not in the repo.
* **Fit quality in color.** The color of each fit against the color of the baseline-removed measured spectrum at the final scale (the missing 380-450 nm of a spectrum filled by the fit): amethyst o-ray 0.36 (x 4.1) and 0.11 (x 0.5), smoky o 0.01 and e 0.02, pink o 0.21 and e 0.12, opal 0.41 (0.537 mm plate, Fire Orange), 1.22 (Fire Red) and 0.07 (1.282 mm plate), rutile brownish o 0.45 and e 0.38, orange o 0.20 and e 0.02 (dE2000).
* **Scaling and targets.** Each variant is scaled to a typical faceted stone of its variety by its face-up lightness (and, for line absorbers, by a plausible dopant amount). The nominal L*C*h targets are my estimates of trade colors from the descriptions in R4 and the Report: no published face-up L*C*h exists for these varieties, so the dE2000 to the target checks the consistency of the scaling, it is not a validation. The pinned values are the reached ones.
* **Estimated flag.** Superseded 2026-10-27 by owner decision O3 (see the Update section above): `estimated = false` only where the shape is a measured spectrum of that variety AND the strength is measured or published; the first-round rule below it (every fitted-from-a-measured-spectrum entry false) is withdrawn, the chosen strengths of Smoky, Pink (Crystalline), Fire Orange, Fire Red and Rutile now make those entries estimated as well, and the synthetics, glass and moissanite bands, which come from the unverified Report, stay estimates.
* **Isotropic hosts** (Opal, Cubic Zirconia, YAG, glass): equal `o_ray` / `e_ray` sets and `pleochroic = false` (the loader rejects any other rank). **Uniaxial hosts**: `o_ray` is E perpendicular to c, `e_ray` E parallel to c; Citrine and Prasiolite carry equal sets stored pleochroic (no visible pleochroism, no polarized spectrum).
* **Fitted from data, not from the built-in tensors.** The Amethyst, Citrine, Quartz and Synthetic Moissanite defaults are being changed by lane B1; no variant here is derived from them.

## Fits

* **Amethyst o-ray** (Anahi, 6.17 mm, 330-1050 nm, offset 0.217 A): four bands at 364.3, 416.1, 537.1 and 953.8 nm, rms 0.0041 A, max 0.035 A (R4's fixed-centre three-band set had rms 0.030 A). The 537 nm band is broader than the Report's (sigma 2197 vs 1478 cm^-1).
* **Citrine table** (the 19-point alpha table, 350-800 nm): four bands at 334.1, 421.3, 568.2 and 700 nm, log rms 0.06; model / table within 0.92-1.10 from 350 to 635 nm. A two-band fit reaches log rms 0.09.
* **Smoky** (GRR 2918, 381-1050 nm, Fresnel offsets): o-ray four bands (358.6, 500, 676.5, 846.3 nm), rms 0.0016 A, max 0.019 A; e-ray five bands (345, 410.7, 493.9, 659.2, 856.6 nm), rms 0.0046 A, max 0.058 A. The UV bands lie at the data edge (381 nm) and are poorly constrained, as R4 noted; the bands are smooth continuum describers.
* **Pink (Crystalline)** (GRR 2352, 390-1050 nm, Fresnel offsets): three bands per ray, rms 0.0017 A (o), 0.0012 A (e), max 0.0047 A; the UV band sigma sits at its 6000 cm^-1 bound.
* **Opal** (GRR 1824, both plates jointly, 440-1000 nm, A below 2.4): five bands, rms 0.0067 A, max 0.016 A; the two plates agree per millimetre to 10 % (R4).
* **Rutile** (450-1100 nm brownish, 480-1100 nm orange, Fresnel offsets): three bands per ray (edge, broad visible shoulder, NIR plateau), brownish rms 0.0054 / 0.0042 A, orange 0.0026 / 0.0023 A; the plateau and shoulder bands are what make the sample "brownish" instead of a clean orange edge, and they are fitted from the measured spectrum, not added.

## Entries


### Amethyst: Deep Siberian

Entry key `amethyst.deep_siberian`; `estimated = true`. Fe4+ ([FeO4]0) color center; Anahi o-ray fitted, e-ray from the Report weights, 4.1 x the Anahi density.

**Sources.**

* o-ray: G. R. Rossman, Caltech Mineral Spectroscopy Server (CC BY-NC 4.0, DOI 10.7907/jywr-qq57), Anahi Mine, Bolivia, 6.17 mm slab, E perpendicular c, http://minerals.gps.caltech.edu/FILES/Visible/quartz/ameth.bo (reference_spectra/variant_amethyst_deep_siberian_eperpc.csv); 4 energy Gaussians fitted, constant baseline 0.217 A removed, rms 0.0041 A
* e-ray: no measured spectrum; the o-ray bands times the Report weights 1.0 / 1.0 / 0.7 / 0.5 (UV, violet, 545 nm, 950 nm bands) from docs/chromophores/research-2026-10-polarized-spectra.md (secondary); estimated
* Band positions (357, 545, 950 nm): Cox, J. Phys. C 10, 4631 (1977), as quoted in https://www.ncbi.nlm.nih.gov/pmc/articles/PMC7483767/; strength scaled to the face-up target (4.1 x the Anahi density); estimated

**Scaling.** k = 4.1 x the Anahi peaks (R4 range 2.5-3.5 x was set against a baseline-included spectrum; the baseline-free fit is lighter, so the face-up L* 36 target needs 4.1 x).

**Bands** (per millimetre, Napierian; the `o_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 364.3 | 1914.75 cm^-1 | 0.61217 | energy |
| 416.12 | 1236.3 cm^-1 | 0.16268 | energy |
| 537.14 | 2197.48 cm^-1 | 0.35866 | energy |
| 953.79 | 1304.91 cm^-1 | 0.079429 | energy |

`e_ray`:

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 364.3 | 1914.75 cm^-1 | 0.61217 | energy |
| 416.12 | 1236.3 cm^-1 | 0.16268 | energy |
| 537.14 | 2197.48 cm^-1 | 0.25106 | energy |
| 953.79 | 1304.91 cm^-1 | 0.039715 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: deep purple with red and blue flashes (trade description 'Deep Siberian'); a typical dark grade; nominal L* 36, C* 43, h 332 (a trade-color estimate, not a published measurement). Reached: L* 36.2, C* 42.2, h 332.6 (Lab 36.16, 37.45, -19.44); dE2000 to the nominal target 0.4.

**Per direction.** o-ray L* 31, C* 48, h 330.5 (bluish violet); e-ray L* 44, C* 36, h 336.9 (lighter, reddish violet). The documented order (E perpendicular c bluish violet, E parallel c reddish violet) is reproduced, the hue gap (6 degrees) and lightness gap rest on the Report weights (estimated). Pinned Lab: o-ray [30.96, 41.62, -23.56], e-ray [44.25, 32.98, -14.09].

### Amethyst: Rose de France

Entry key `amethyst.rose_de_france`; `estimated = true`. Same Fe4+ center as Deep Siberian at 0.5 x the Anahi density (very pale lilac).

**Sources.**

* o-ray: G. R. Rossman, Caltech Mineral Spectroscopy Server (CC BY-NC 4.0, DOI 10.7907/jywr-qq57), Anahi Mine, Bolivia, 6.17 mm slab, E perpendicular c, http://minerals.gps.caltech.edu/FILES/Visible/quartz/ameth.bo (reference_spectra/variant_amethyst_deep_siberian_eperpc.csv); 4 energy Gaussians fitted, constant baseline 0.217 A removed, rms 0.0041 A
* e-ray: no measured spectrum; the o-ray bands times the Report weights 1.0 / 1.0 / 0.7 / 0.5 from docs/chromophores/research-2026-10-polarized-spectra.md (secondary); estimated
* Same color center as Deep Siberian at 0.5 x the Anahi density (a pale grade has no spectrum of its own); estimated

**Scaling.** k = 0.5 x the Anahi peaks (R4 0.15-0.25 x would be nearly colorless with the baseline removed).

**Bands** (per millimetre, Napierian; the `o_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 364.3 | 1914.75 cm^-1 | 0.074655 | energy |
| 416.12 | 1236.3 cm^-1 | 0.019839 | energy |
| 537.14 | 2197.48 cm^-1 | 0.04374 | energy |
| 953.79 | 1304.91 cm^-1 | 0.0096865 | energy |

`e_ray`:

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 364.3 | 1914.75 cm^-1 | 0.074655 | energy |
| 416.12 | 1236.3 cm^-1 | 0.019839 | energy |
| 537.14 | 2197.48 cm^-1 | 0.030618 | energy |
| 953.79 | 1304.91 cm^-1 | 0.0048433 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: very pale pinkish lilac ('Rose de France'); nominal L* 88, C* 10, h 328 (a trade-color estimate, not a published measurement). Reached: L* 88.2, C* 10.5, h 329.0 (Lab 88.22, 9.01, -5.42); dE2000 to the nominal target 0.5.

**Per direction.** o-ray L* 87.0, C* 12.3, h 327.2; e-ray L* 90.7, C* 7.2, h 334.9: same order as Deep Siberian. Pinned Lab: o-ray [86.95, 10.34, -6.67], e-ray [90.66, 6.5, -3.04].

### Citrine: Madeira

Entry key `citrine.madeira`; `estimated = true`. Fe3+ aggregate edge (4 bands refitted from the table), equal o and e sets, 7 x the golden table density.

**Sources.**

* Shape: the 19-point alpha(lambda) table of reference_spectra/citrine_annealed_amethyst.csv (docs/chromophores/research-2026-10-polarized-spectra.md section 4; user-delivered, secondary, unverified), refitted here with 4 energy Gaussians (log rms 0.06, within 10 % from 350 to 635 nm)
* Cause: Fe3+ in iron oxide-hydroxide aggregates, Caltech https://minerals.gps.caltech.edu/Ametrine/Index.html; no polarized spectrum exists and the pleochroism of citrine is negligible: o and e sets are equal (stored pleochroic because the host is uniaxial)
* Strength: 7 x the table density, set by the face-up target (Madeira = deep orange-brown); estimated

**Scaling.** k = 7.0 x the table (R4 proposed 4 x; 7 x reaches L* 49 over the 9.905 mm path).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 334.06 | 3489.62 cm^-1 | 10.603 | energy |
| 421.3 | 2580.16 cm^-1 | 1.2709 | energy |
| 568.22 | 1438.45 cm^-1 | 0.064723 | energy |
| 700 | 3000 cm^-1 | 0.014899 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: deep orange to reddish brown ('Madeira'); R4 gives only the description, so the lightness is a choice; nominal L* 48, C* 90, h 58 (a trade-color estimate, not a published measurement). Reached: L* 49.2, C* 95.0, h 61.7 (Lab 49.25, 45.03, 83.64); dE2000 to the nominal target 3.6.

**Per direction.** None: equal sets (negligible pleochroism of citrine; no polarized spectrum). Pinned Lab: o-ray [49.25, 45.03, 83.64], e-ray [49.25, 45.03, 83.64].

### Quartz: Smoky

Entry key `quartz.smoky`; `estimated = true`. Al hole center; both rays fitted from Caltech GRR 2918, 0.43 x the Caltech density.

**Sources.**

* G. R. Rossman, Caltech Mineral Spectroscopy Server (CC BY-NC 4.0, DOI 10.7907/jywr-qq57), GRR 2918, smoky quartz, Jack Rabbit Claim, Teller County, Colorado, 5.788 mm, E perpendicular c and E parallel c: http://minerals.gps.caltech.edu/FILES/Visible/quartz/Quartz2918%20perp%20c.asc and .../Quartz2918%20parallel%20c.asc (reference_spectra/variant_quartz_smoky_eperpc.csv, ..._eparc.csv)
* Fit: 4 (o) and 5 (e) energy Gaussians on 381-1050 nm with the Fresnel reflection loss (0.040 / 0.041 A) removed, rms 0.0016 / 0.0046 A; the e-ray absorbs 1.5-1.75 x more than the o-ray (measured)
* Cause: Al hole center [AlO4/h+]0, https://rruff.info/doclib/MinMag/Volume_41/41-319-301.htm (bands near 365, 428, 475, 650 nm); strength scaled to 0.43 x the very dark Caltech crystal for a typical smoky gem

**Scaling.** k = 0.43 x the Caltech crystal (a 'morion-dark' sample), face-up L* 55.

**Bands** (per millimetre, Napierian; the `o_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 358.551 | 3831.1 cm^-1 | 0.1598 | energy |
| 500 | 3544.98 cm^-1 | 0.13781 | energy |
| 676.49 | 2107.47 cm^-1 | 0.044846 | energy |
| 846.26 | 1274.22 cm^-1 | 0.0060312 | energy |

`e_ray`:

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 345 | 2000 cm^-1 | 0.24821 | energy |
| 410.683 | 2500 cm^-1 | 0.19641 | energy |
| 493.897 | 2252.2 cm^-1 | 0.16255 | energy |
| 659.15 | 2524.26 cm^-1 | 0.1362 | energy |
| 856.595 | 1261.01 cm^-1 | 0.0039705 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: medium grey-brown smoky gem (T about 25-45 % at 500 nm, R4); nominal L* 55, C* 15, h 65 (a trade-color estimate, not a published measurement). Reached: L* 54.9, C* 15.4, h 65.3 (Lab 54.89, 6.43, 14.01); dE2000 to the nominal target 0.3.

**Per direction.** o-ray L* 58.1, C* 13.8, h 60.1; e-ray L* 47.5, C* 21.3, h 75.6: the e-ray is darker and browner, the measured ratio 1.5-1.75 (the literature dichroic ratio of the 2.9 eV band in synthetic quartz, 1.15-1.22, is smaller; the Caltech spectra are the basis). Pinned Lab: o-ray [58.08, 6.89, 11.97], e-ray [47.48, 5.28, 20.62].

### Quartz: Pink (Crystalline)

Entry key `quartz.pink_crystalline`; `estimated = true`. P / O- hole center in a transparent crystal; both rays fitted from Caltech GRR 2352, 0.7 x the measured density.

**Sources.**

* G. R. Rossman, Caltech Mineral Spectroscopy Server (CC BY-NC 4.0, DOI 10.7907/jywr-qq57), GRR 2352, crystalline pink quartz, Mount Mica, Maine, 3.681 mm, E perpendicular c and E parallel c: http://minerals.gps.caltech.edu/FILES/Visible/quartz/Pink_quartz_2352_a.txt and .../Pink_quartz_2352_c.txt (reference_spectra/variant_quartz_pink_crystalline_eperpc.csv, ..._eparc.csv)
* Fit: 3 energy Gaussians per ray on 390-1050 nm with the Fresnel reflection loss (0.040 / 0.041 A) removed, rms 0.0017 / 0.0012 A; the o-ray absorbs 2 x more at 525 nm than the e-ray (measured)
* Cause: P substitution plus an O- hole center (Caltech https://minerals.gps.caltech.edu/FILES/Visible/quartz/Index.html); replaces massive Rose Quartz (dropped); 0.7 x the measured density for a light pink stone

**Scaling.** k = 0.7 x the measured density (the Caltech plate is 3.7 mm; scaling to a lighter pink than the raw sample).

**Bands** (per millimetre, Napierian; the `o_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 372.037 | 6000 cm^-1 | 0.061495 | energy |
| 523.897 | 1722.84 cm^-1 | 0.041663 | energy |
| 776.726 | 3375.87 cm^-1 | 0.020567 | energy |

`e_ray`:

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 388.161 | 6000 cm^-1 | 0.053497 | energy |
| 528.875 | 1721.7 cm^-1 | 0.020331 | energy |
| 846.164 | 5000 cm^-1 | 0.028463 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: light cool pink; nominal L* 80, C* 14, h 2 (a trade-color estimate, not a published measurement). Reached: L* 80.8, C* 11.4, h 2.4 (Lab 80.8, 11.37, 0.47); dE2000 to the nominal target 2.2.

**Per direction.** o-ray L* 80.3, C* 13.7, h 1.6; e-ray L* 81.8, C* 6.9, h 5.5: the o-ray is about twice as saturated (measured). Pinned Lab: o-ray [80.28, 13.65, 0.37], e-ray [81.84, 6.89, 0.66].

### Quartz: Prasiolite

Entry key `quartz.prasiolite`; `estimated = true`. Estimated: Fe2+ 720 nm band plus a weak Fe3+ edge, equal o and e sets (cause disputed).

**Sources.**

* Bands: 720 nm Fe2+ (Fe2+-Fe3+ charge transfer; Platonov 1992 and Czaja, https://rebus.us.edu.pl/bitstream/20.500.12128/3822/1/Czaja_The_Mossbauer_spectra_of_prasiolite_and_amethyst_crystals.pdf), weak 442 nm Fe band (Paradise 1982, Gems & Gemology), plus the Fe3+ edge of the Citrine fit at 0.1 x (docs/color_variants/R4_quartz_diamond_opal_synthetics_glass.md section 3.5)
* No polarized spectrum and the cause is disputed in the literature: equal o and e sets; the 0.90 cm^-1 peak is the Report value 0.03 cm^-1/ppm Fe2+ at 30 ppm (secondary); estimated
* Strength: 1.5 x R4's base amounts, set by the face-up target (pale leek green)

**Scaling.** k = 1.5 x R4's base amounts.

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 720 | 1486 cm^-1 | 0.135 | energy |
| 442 | 1500 cm^-1 | 0.03 | energy |
| 334.06 | 3489.62 cm^-1 | 0.22721 | energy |
| 421.3 | 2580.16 cm^-1 | 0.027232 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: pale leek green (Report description); nominal L* 93, C* 35, h 130 (a trade-color estimate, not a published measurement). Reached: L* 93.8, C* 36.0, h 133.2 (Lab 93.82, -24.62, 26.26); dE2000 to the nominal target 1.3.

**Per direction.** None: equal sets (no polarized spectrum). Pinned Lab: o-ray [93.82, -24.62, 26.26], e-ray [93.82, -24.62, 26.26].

### Opal: Fire Orange

Entry key `opal.fire_orange`; `estimated = true`. Fe3+ (hematite-like domains); one stone, both plates fitted jointly; 0.055 x the cherry density.

**Sources.**

* G. R. Rossman, Caltech Mineral Spectroscopy Server (CC BY-NC 4.0, DOI 10.7907/jywr-qq57), GRR 1824, cherry opal, Opal Mountain, San Bernardino County, California, 0.537 mm and 1.282 mm plates: http://minerals.gps.caltech.edu/FILES/Visible/opal/opal1824_537um.txt and .../opal1824_1282um.txt (reference_spectra/variant_opal_fire_red_0p537mm_iso.csv, ..._1p282mm_iso.csv)
* Fit: 5 energy Gaussians fitted jointly to both plates (per-millimetre consistent) on 440-1000 nm, plate baselines 0.085 / 0.10 A (reflection plus scatter) removed, rms 0.0067 A
* Cause: Fe3+ in sub-microscopic hematite-like domains (Rossman, https://minerals.gps.caltech.edu/FILES/Visible/opal/Index.html); isotropic; strength 0.055 x the (very dense) cherry plate, set by the face-up target

**Scaling.** k = 0.055 x the cherry plate (R4: 0.04 x).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 435.96 | 2202.46 cm^-1 | 0.51285 | energy |
| 496.21 | 809.62 cm^-1 | 0.078001 | energy |
| 533.28 | 772.72 cm^-1 | 0.11508 | energy |
| 609.24 | 977.09 cm^-1 | 0.0077677 | energy |
| 680 | 2500 cm^-1 | 0.008233 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: saturated orange fire opal; nominal L* 68, C* 90, h 64 (a trade-color estimate, not a published measurement). Reached: L* 67.9, C* 104.2, h 66.0 (Lab 67.94, 42.39, 95.22); dE2000 to the nominal target 3.2.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [67.94, 42.39, 95.22], e-ray [67.94, 42.39, 95.22].

### Opal: Fire Red

Entry key `opal.fire_red`; `estimated = true`. Same fit as Fire Orange at 0.25 x the cherry density (deep orange-red).

**Sources.**

* G. R. Rossman, Caltech Mineral Spectroscopy Server (CC BY-NC 4.0, DOI 10.7907/jywr-qq57), GRR 1824, cherry opal, Opal Mountain, San Bernardino County, California, 0.537 mm and 1.282 mm plates: http://minerals.gps.caltech.edu/FILES/Visible/opal/opal1824_537um.txt and .../opal1824_1282um.txt (reference_spectra/variant_opal_fire_red_0p537mm_iso.csv, ..._1p282mm_iso.csv)
* Fit: 5 energy Gaussians fitted jointly to both plates on 440-1000 nm, plate baselines 0.085 / 0.10 A removed, rms 0.0067 A
* Cause: Fe3+ in sub-microscopic hematite-like domains; isotropic; strength 0.25 x the cherry plate, set by the face-up target

**Scaling.** k = 0.25 x the cherry plate (R4: 0.12 x; the lighter R4 value gives orange, not red-orange, with this fit).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 435.96 | 2202.46 cm^-1 | 2.3312 | energy |
| 496.21 | 809.62 cm^-1 | 0.35455 | energy |
| 533.28 | 772.72 cm^-1 | 0.52307 | energy |
| 609.24 | 977.09 cm^-1 | 0.035308 | energy |
| 680 | 2500 cm^-1 | 0.037422 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: deep orange-red (cherry-leaning) fire opal; nominal L* 38, C* 85, h 45 (a trade-color estimate, not a published measurement). Reached: L* 38.4, C* 89.0, h 48.0 (Lab 38.4, 59.56, 66.08); dE2000 to the nominal target 2.6.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [38.4, 59.56, 66.08], e-ray [38.4, 59.56, 66.08].

### Rutile: Brownish Red-Orange

Entry key `rutile.brownish_red_orange`; `estimated = true`. Fe / Cr (V, Nb) charge-transfer edge; both rays fitted; strength by the face-up target, not the disputed thickness.

**Sources.**

* G. R. Rossman, Caltech Mineral Spectroscopy Server (CC BY-NC 4.0, DOI 10.7907/jywr-qq57), rutile 250-2h d, North Carolina, brownish red-orange, E perpendicular c and E parallel c: http://minerals.gps.caltech.edu/FILES/Visible/Rutile/Rutile%20250-2h%20d.a.txt and .../Rutile%20250-2h%20d.c.txt (reference_spectra/variant_rutile_brownish_red_orange_eperpc.csv, ..._eparc.csv)
* Fit: 3 energy Gaussians per ray on 450-1100 nm (edge, broad visible shoulder, NIR plateau), Fresnel loss (0.175 / 0.210 A) removed, rms 0.0054 / 0.0042 A; the e-ray edge sits redward of the o-ray edge (measured)
* The sample thickness is unresolved (0.269 mm in the file header, 2.69 mm on the Caltech index page): the strength is NOT taken from the thickness; it is set by the face-up target (0.05 x the header-thickness density); Cr / Fe / V / Nb on Ti sites

**Scaling.** k = 0.05 x the density of the 0.269 mm plate as listed in the file header. The header says 0.269 mm, the Caltech index page 2.69 mm; the factor 10 is unresolved, so the strength is defined by the face-up target (L* 40), as R4 did, and is independent of the thickness. For comparison R4's anchor alpha(500 nm, o) = 2.5 cm^-1 gives k = 0.03; this fit's k = 0.05 gives alpha(500, o) about 5 cm^-1 because the fit includes the visible shoulder and the NIR plateau.

**Bands** (per millimetre, Napierian; the `o_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 403.343 | 3196.5 cm^-1 | 1.3026 | energy |
| 900 | 5146.39 cm^-1 | 0.043583 | energy |
| 560 | 6000 cm^-1 | 0.068723 | energy |

`e_ray`:

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 440 | 2146.57 cm^-1 | 0.93044 | energy |
| 900 | 4810.91 cm^-1 | 0.11453 | energy |
| 456.908 | 4461.67 cm^-1 | 0.21457 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: brownish red-orange (the Caltech sample description), dark; nominal L* 40, C* 70, h 65 (a trade-color estimate, not a published measurement). Reached: L* 39.7, C* 72.5, h 66.9 (Lab 39.71, 28.48, 66.66); dE2000 to the nominal target 1.6.

**Per direction.** o-ray L* 42.2, C* 76.2, h 67.6; e-ray L* 34.1, C* 63.9, h 64.7: the e-ray is darker and redder (measured; R4: e-ray 1.2 x the o-ray strength). Pinned Lab: o-ray [42.16, 29.08, 70.41], e-ray [34.06, 27.3, 57.81].

### Rutile: Orange

Entry key `rutile.orange`; `estimated = true`. Same ions at a lower density; both rays fitted from the Graves Mountain plate; 0.004 x its density.

**Sources.**

* G. R. Rossman, Caltech Mineral Spectroscopy Server (CC BY-NC 4.0, DOI 10.7907/jywr-qq57), rutile DV-1, Graves Mountain, Georgia, orange, 0.040 mm, E perpendicular c and E parallel c: http://minerals.gps.caltech.edu/FILES/Visible/Rutile/Rutile%20DV1%20G%20raves%20Mtn%20perp%20c.asc and .../Rutile%20DV1%20Graves%20Mtn%20parallel%20c.asc (reference_spectra/variant_rutile_orange_dv1_eperpc.csv, ..._eparc.csv)
* Fit: 3 energy Gaussians per ray on 480-1100 nm, Fresnel loss (0.175 / 0.210 A) removed, rms 0.0026 / 0.0023 A; about 1.3 % other ions (Cr~Fe>V>Nb), opaque when thicker than 0.5 mm
* Strength: 0.004 x the 0.040 mm plate density (a pale, gem-transparent crystal of the same chemistry), set by the face-up target

**Scaling.** k = 0.004 x the 0.040 mm DV-1 plate (which is opaque thicker than 0.5 mm); face-up target L* 55.

**Bands** (per millimetre, Napierian; the `o_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 421.951 | 2862.36 cm^-1 | 0.80468 | energy |
| 900 | 5988.49 cm^-1 | 0.025472 | energy |
| 560 | 5320.26 cm^-1 | 0.031465 | energy |

`e_ray`:

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 432.708 | 2512.96 cm^-1 | 0.87811 | energy |
| 1500 | 12000 cm^-1 | 0.03192 | energy |
| 552.377 | 4930.94 cm^-1 | 0.037999 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: orange (the Caltech sample description); nominal L* 55, C* 90, h 68 (a trade-color estimate, not a published measurement). Reached: L* 54.5, C* 93.5, h 70.3 (Lab 54.47, 31.52, 88.04); dE2000 to the nominal target 2.2.

**Per direction.** o-ray L* 55.3, e-ray L* 52.8, hues 70.9 / 69.0: nearly equal (measured). Pinned Lab: o-ray [55.26, 30.53, 88.31], e-ray [52.83, 33.64, 87.41].

### Rutile: Blue (Reduced)

Entry key `rutile.blue_reduced`; `estimated = true`. Estimated: Ti3+ 625 nm band plus NIR polaron band on the built-in rutile edge; e-ray 0.7 x the o-ray.

**Sources.**

* Cause: Ti3+ (reduction) d-d and Ti3+-Ti4+ polaron absorption, Caltech https://minerals.gps.caltech.edu/FILES/Visible/Rutile/Index.html; Khomenko et al. 1998 and Cronemeyer, Phys. Rev. 113, 1222 (1958): polaron / NIR band near 1.18 eV (1050 nm) (via search summaries, docs/color_variants/R4_quartz_diamond_opal_synthetics_glass.md section 7.4)
* No numeric spectrum exists: the 1050 nm band (sigma 2200 cm^-1), the visible 625 nm Ti3+ band (the part that makes the stone blue) and the e/o ratio 0.7 (Report) are estimates; the UV edge is the built-in Rutile edge (350 nm, 8.0 per model unit = 0.808 per mm at the 9.905 mm face-up path); estimated

**Scaling.** Visible 625 nm band and polaron band at k = 1.4 (the UV edge group stays at 1.0); the 625 nm peak 4.2 cm^-1 (o) is the knob that sets the blue, chosen for the L* 40 target.

**Bands** (per millimetre, Napierian; the `o_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 350 | 40 nm | 0.8077 | wavelength |
| 1050 | 2200 cm^-1 | 0.84 | energy |
| 625 | 2500 cm^-1 | 0.42 | energy |

`e_ray`:

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 350 | 40 nm | 0.8077 | wavelength |
| 1050 | 2200 cm^-1 | 0.588 | energy |
| 625 | 2500 cm^-1 | 0.294 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: dark blue (R4); nominal L* 40, C* 55, h 270 (a trade-color estimate, not a published measurement). Reached: L* 42.1, C* 58.0, h 273.5 (Lab 42.09, 3.56, -57.93); dE2000 to the nominal target 2.4.

**Per direction.** o-ray L* 37.9, C* 64.2, h 279.5; e-ray L* 49.0, C* 50.1, h 262.2: the e-ray is lighter and greener because the Ti3+ bands are 0.7 x in the e-ray (Report weight, estimated). Pinned Lab: o-ray [37.92, 10.61, -63.28], e-ray [48.99, -6.79, -49.59].

### Cubic Zirconia: Ce Yellow

Entry key `cubic_zirconia.ce_yellow`; `estimated = true`. Ce edge, 380 nm band only, 0.3 wt% CeO2 (estimated).

**Sources.**

* Report p.11 (Ce4+/Ce3+ charge-transfer edge, 8.0 cm^-1 per wt% CeO2; secondary): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified); edge as the 380 nm Gaussian (sigma 4500 cm^-1) of R4 section 5.1
* Yellow = 0.3 wt% CeO2 of the 380 nm edge band alone (the 500 nm shoulder of R4 appears only at the higher Ce level); estimated

**Scaling.** 0.3 wt% CeO2 x the Report coefficient (7.25 cm^-1 per wt% for the 380 nm band); the 500 nm band is absent (weight 0).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 380 | 4500 cm^-1 | 0.2175 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: yellow CZ; nominal L* 86, C* 45, h 86 (a trade-color estimate, not a published measurement). Reached: L* 85.0, C* 45.2, h 85.4 (Lab 85.04, 3.59, 45.04); dE2000 to the nominal target 0.7.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [85.04, 3.59, 45.04], e-ray [85.04, 3.59, 45.04].

### Cubic Zirconia: Ce Orange

Entry key `cubic_zirconia.ce_orange`; `estimated = true`. Ce edge, 380 nm band at 0.55 wt% plus a weak 500 nm shoulder (estimated).

**Sources.**

* Report p.11 (Ce4+/Ce3+ charge-transfer edge; secondary): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified); R4 section 5.1 two-Gaussian edge (380 and 500 nm)
* Orange = 0.55 wt% CeO2 of the 380 nm band plus 0.07 wt% of the 500 nm band (the 500 nm weight is fitted to the orange target; with the full Report weights it would be red); estimated

**Scaling.** 0.55 wt% x the 380 nm band and 0.07 wt% x the 500 nm band (R4's 0.6 wt% with both bands at full weight gives about L* 36, dark red-orange: the 500 nm shoulder is fitted down to reach orange).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 380 | 4500 cm^-1 | 0.39875 | energy |
| 500 | 2500 cm^-1 | 0.03843 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: orange CZ; nominal L* 70, C* 65, h 73 (a trade-color estimate, not a published measurement). Reached: L* 68.4, C* 65.3, h 72.6 (Lab 68.38, 19.54, 62.34); dE2000 to the nominal target 1.3.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [68.38, 19.54, 62.34], e-ray [68.38, 19.54, 62.34].

### Cubic Zirconia: Er Pink

Entry key `cubic_zirconia.er_pink`; `estimated = true`. Er3+ lines (Report) plus the 488 and 545 nm lines, 2.0 wt% Er2O3 (estimated).

**Sources.**

* Report p.11: Er3+ 520 nm (FWHM 250 cm^-1, 2.0 cm^-1 per wt%), 380 nm (FWHM 300, 2.0), 653 nm (FWHM 200, 0.5) (secondary): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)
* Added from the standard Er3+ term scheme (R4 section 5.1 gap; not in the Report): 488 nm (0.8) and 545 nm (0.6) lines, estimated relative strengths; 2.0 wt% Er2O3 (Report range 0.2-3); estimated

**Scaling.** 2.0 wt% Er2O3 x the Report coefficients. The lines are narrow, so the lightness hardly moves with the amount (L* 93.7 at 2 wt%, 90.6 at 5 wt%); 2 wt% is inside the Report range 0.2-3.

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 380 | 127.398 cm^-1 | 0.4 | energy |
| 488 | 106.165 cm^-1 | 0.16 | energy |
| 520 | 106.165 cm^-1 | 0.4 | energy |
| 545 | 106.165 cm^-1 | 0.12 | energy |
| 653 | 84.9322 cm^-1 | 0.1 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: pure pink (R4); nominal L* 92, C* 25, h 340 (a trade-color estimate, not a published measurement). Reached: L* 93.7, C* 20.8, h 336.9 (Lab 93.68, 19.15, -8.16); dE2000 to the nominal target 2.6.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [93.68, 19.15, -8.16], e-ray [93.68, 19.15, -8.16].

### Cubic Zirconia: Nd Lilac

Entry key `cubic_zirconia.nd_lilac`; `estimated = true`. Nd3+ lines (Report), 2.5 wt% Nd2O3 (estimated).

**Sources.**

* Report p.11: Nd3+ 582 nm (FWHM 250 cm^-1, 4.0 cm^-1 per wt% Nd2O3), 745 nm (FWHM 200, 1.5), 520 nm (FWHM 250, 0.8) (secondary): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)
* 2.5 wt% Nd2O3 (estimated; the Report gives no range); lilac in daylight, pinker under incandescent light (Report); estimated

**Scaling.** 2.5 wt% Nd2O3 x the Report coefficients (narrow lines, lightness hardly moves with the amount).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 582 | 106.165 cm^-1 | 1 | energy |
| 745 | 84.9322 cm^-1 | 0.375 | energy |
| 520 | 106.165 cm^-1 | 0.2 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: lilac / light purple (Report, CZ study); nominal L* 92, C* 14, h 295 (a trade-color estimate, not a published measurement). Reached: L* 92.5, C* 13.7, h 294.1 (Lab 92.48, 5.62, -12.54); dE2000 to the nominal target 0.4.

**Per direction.** Isotropic: one color. (The Report notes a color change to pinker under incandescent light; not pinned here.) Pinned Lab: o-ray [92.48, 5.62, -12.54], e-ray [92.48, 5.62, -12.54].

### Cubic Zirconia: Cr Green

Entry key `cubic_zirconia.cr_green`; `estimated = true`. Cr3+ bands at 600 and 440 nm, 0.1 wt% Cr2O3 (estimated, low confidence).

**Sources.**

* Report p.11-12: Cr3+ 600 nm (FWHM 3000 cm^-1, 10 cm^-1 per wt% Cr2O3) and 440 nm (FWHM 3500, 8.0) (secondary, low confidence; Cr3+ in the 8-fold Zr site is unusual): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)
* 0.1 wt% Cr2O3 (R4 estimate); estimated

**Scaling.** 0.1 wt% Cr2O3 x the Report coefficients.

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 600 | 1273.98 cm^-1 | 0.1 | energy |
| 440 | 1486.31 cm^-1 | 0.08 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: emerald-like pale green; nominal L* 80, C* 35, h 165 (a trade-color estimate, not a published measurement). Reached: L* 81.1, C* 28.4, h 174.4 (Lab 81.1, -28.27, 2.76); dE2000 to the nominal target 4.4.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [81.1, -28.27, 2.76], e-ray [81.1, -28.27, 2.76].

### YAG: Nd Lilac

Entry key `yag.nd_lilac`; `estimated = true`. Nd3+ multiplet bands (estimated widths and strengths), 1.5 at. % Nd.

**Sources.**

* Report p.12: Nd3+ 586 nm (6.0e-4 cm^-1 per ppm), 746 nm (3.0e-4), 530 nm (2.5e-4), 808 nm (9.0e-4) (secondary): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)
* The Report's single 40 cm^-1 lines are replaced by effective multiplet bands (sigma 150 / 80 / 60 / 40 cm^-1, peaks 3.0 / 1.5 / 1.25 / 4.5 cm^-1 per 1 at. %), because the 565-600 nm manifold has several lines (R4 section 5.2); x 1.5 at. %; estimated

**Scaling.** 1.5 at. % Nd (10,000 ppm x 1.5) on the multiplet bands.

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 586 | 150 cm^-1 | 0.45 | energy |
| 746 | 80 cm^-1 | 0.225 | energy |
| 530 | 60 cm^-1 | 0.1875 | energy |
| 808 | 40 cm^-1 | 0.675 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: pale lilac; nominal L* 92, C* 12, h 275 (a trade-color estimate, not a published measurement). Reached: L* 92.7, C* 12.6, h 263.2 (Lab 92.66, -1.5, -12.52); dE2000 to the nominal target 3.3.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [92.66, -1.5, -12.52], e-ray [92.66, -1.5, -12.52].

### YAG: Cr Green

Entry key `yag.cr_green`; `estimated = true`. Cr3+ bands at 605 and 430 nm, 3000 ppm (estimated).

**Sources.**

* Cr3+ 4A2-4T2 near 605 nm and 4A2-4T1 near 430 nm: Optical Materials 2000 (440 / 605 nm) and https://apps.dtic.mil/sti/pdfs/ADP011875.pdf (R4 section 5.2); widths 2800 / 3300 cm^-1 FWHM and 8.0e-4 cm^-1 per ppm from the Report (secondary): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)
* 3000 ppm Cr (R4 estimate); estimated

**Scaling.** k = 1.0 (3000 ppm Cr, R4).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 605 | 1189.05 cm^-1 | 0.24 | energy |
| 430 | 1401.38 cm^-1 | 0.24 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: emerald-like green (R4); the host hue sits at the emerald hue 168 of the built-in Emerald; nominal L* 70, C* 70, h 165 (a trade-color estimate, not a published measurement). Reached: L* 69.2, C* 74.5, h 169.0 (Lab 69.15, -73.15, 14.27); dE2000 to the nominal target 2.5.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [69.15, -73.15, 14.27], e-ray [69.15, -73.15, 14.27].

### YAG: Ce Yellow

Entry key `yag.ce_yellow`; `estimated = true`. Ce3+ 460 nm band plus the 340 nm band, 200 ppm (estimated).

**Sources.**

* Ce3+ 4f-5d near 460 nm (US 9,651,483, https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/9651483) and a UV band near 340 nm; strengths 0.028 and 0.015 cm^-1 per ppm from the Report (secondary): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)
* 200 ppm Ce (R4: 100-300 ppm for a gem tint); estimated

**Scaling.** k = 0.2 (200 ppm Ce).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 460 | 1189.05 cm^-1 | 0.56 | energy |
| 340 | 1061.65 cm^-1 | 0.3 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: vivid canary yellow (R4); nominal L* 92, C* 100, h 90 (a trade-color estimate, not a published measurement). Reached: L* 91.7, C* 103.8, h 89.7 (Lab 91.71, 0.62, 103.8); dE2000 to the nominal target 0.8.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [91.71, 0.62, 103.8], e-ray [91.71, 0.62, 103.8].

### Synthetic Moissanite: Near Colorless

Entry key `synthetic_moissanite.near_colorless`; `estimated = true`. The 425 nm edge of 6H SiC, one band, equal o and e sets (estimated amplitude): K-L, very slightly yellowish.

**Sources.**

* 425 nm absorption edge of 6H SiC (Nassau, McClure, Elen, Shigley, Gems & Gemology 33(4), 1997, https://www.moissaniteco.com/media/view/2018/09/synthetic-moissanite-a-new-diamond-substitute_67.pdf): all samples show a distinct increase near 425 nm; no absorption coefficient is published
* One band per ray at 380 nm (sigma 3500 cm^-1), peak 0.36 cm^-1 so that A(425 nm) = 0.1 over 8.8 mm (R4 section 5.4); no polarized edge data, so equal o and e sets (stored pleochroic: uniaxial host); estimated

**Scaling.** k = 1 (R4 amplitude 0.36 cm^-1 sets A(425 nm) = 0.1 over 8.8 mm).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 380 | 3500 cm^-1 | 0.036 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: K-L face-up: looks colorless to very slightly yellowish (Nassau 1997); nominal L* 97, C* 6, h 100 (a trade-color estimate, not a published measurement). Reached: L* 98.9, C* 9.2, h 103.0 (Lab 98.86, -2.08, 9.0); dE2000 to the nominal target 2.8.

**Per direction.** None: equal o and e sets (no polarized edge data); the stone is nearly colorless in every direction. Pinned Lab: o-ray [98.86, -2.08, 9.0], e-ray [98.86, -2.08, 9.0].

### Synthetic Moissanite: Green Tint

Entry key `synthetic_moissanite.green_tint`; `estimated = true`. N donor 625 nm band (o stronger than e) plus the 425 nm edge, estimated.

**Sources.**

* N donor free-carrier band at 625 nm (6H SiC, Weingartner et al. 2004, https://cris.fau.de/publications/122709224), o:e weights 1.0 : 0.4 and 0.5 cm^-1 per ppma N from the Report (secondary); 425 nm edge (Nassau et al., Gems & Gemology 33(4), 1997, https://www.moissaniteco.com/media/view/2018/09/synthetic-moissanite-a-new-diamond-substitute_67.pdf): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)
* 0.7 x R4's 3 ppma N band, and the 380 nm edge band at 4 x the near-colorless amplitude (the edge brings the yellow-green hue; fitted to the target); estimated

**Scaling.** N band 0.7 x R4's 3 ppma value (2.1 ppma equivalent), edge band 4 x the near-colorless amplitude 0.36 cm^-1.

**Bands** (per millimetre, Napierian; the `o_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 625 | 1061.65 cm^-1 | 0.105 | energy |
| 380 | 3500 cm^-1 | 0.144 | energy |

`e_ray`:

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 625 | 1061.65 cm^-1 | 0.042 | energy |
| 380 | 3500 cm^-1 | 0.144 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: light green with a yellowish-olive undertone (Nassau 1997 'green' stones); nominal L* 86, C* 38, h 142 (a trade-color estimate, not a published measurement). Reached: L* 87.2, C* 34.2, h 149.9 (Lab 87.17, -29.59, 17.17); dE2000 to the nominal target 3.3.

**Per direction.** o-ray L* 85.2, C* 38.5, h 159.0; e-ray L* 90.9, C* 30.2, h 128.6: the o-ray carries the free-carrier band (Report e : o = 0.4), so it is the darker, bluer green and the e-ray the lighter, yellower one. Pinned Lab: o-ray [85.2, -35.92, 13.79], e-ray [90.9, -18.83, 23.57].

### Glass (N-BK7): Co Blue

Entry key `glass__n_bk7_.co_blue`; `estimated = true`. Co2+ triplet 537 / 595 / 640 nm, 0.06 wt% CoO (estimated).

**Sources.**

* Tetrahedral Co2+ triplet 537 / 595 / 640 nm (British Glass digital library, https://www.britglass.org.uk/knowledge-base/digital-library-and-information-services/effect-alkali-ions-optical-absorption); FWHM 1100 cm^-1 and 80 / 115 / 100 cm^-1 per wt% CoO from the Report p.29 (secondary): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)
* 0.06 wt% CoO (R4: 0.05 wt% is a saturated royal blue); the colorant bands do not depend on the host glass, so N-BK7 and F2 carry the same entry; estimated

**Scaling.** k = 6 (0.06 wt% CoO).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 537 | 467.127 cm^-1 | 0.48 | energy |
| 595 | 467.127 cm^-1 | 0.69 | energy |
| 640 | 467.127 cm^-1 | 0.6 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: royal / cobalt blue; nominal L* 45, C* 80, h 280 (a trade-color estimate, not a published measurement). Reached: L* 47.9, C* 92.1, h 286.2 (Lab 47.9, 25.67, -88.41); dE2000 to the nominal target 5.1.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [47.9, 25.67, -88.41], e-ray [47.9, 25.67, -88.41].

### Glass (N-BK7): Cr Green

Entry key `glass__n_bk7_.cr_green`; `estimated = true`. Cr3+ bands at 650 and 450 nm, 0.4 wt% Cr2O3 (estimated).

**Sources.**

* Octahedral Cr3+ at 650 and 450 nm (glass chemistry, https://www.thaiscience.info/journals/Article/TKJN/10974416.pdf); FWHM 2500 / 3000 cm^-1 and 11.0 cm^-1 per wt% Cr2O3 from the Report p.29 (secondary): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)
* 0.4 wt% Cr2O3 (reducing melt, Cr3+ only); same entry on both glasses; estimated

**Scaling.** k = 4 (0.4 wt% Cr2O3).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 650 | 1061.65 cm^-1 | 0.44 | energy |
| 450 | 1273.98 cm^-1 | 0.44 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: emerald-like green; nominal L* 75, C* 95, h 140 (a trade-color estimate, not a published measurement). Reached: L* 77.3, C* 111.7, h 138.5 (Lab 77.27, -83.66, 74.04); dE2000 to the nominal target 3.5.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [77.27, -83.66, 74.04], e-ray [77.27, -83.66, 74.04].

### Glass (N-BK7): Mn Purple

Entry key `glass__n_bk7_.mn_purple`; `estimated = true`. Mn3+ band at 510 nm (estimated strength), amethyst purple.

**Sources.**

* Octahedral Mn3+ 5Eg-5T2g near 500 nm (https://kirim.kmutt.ac.th/converis/portal/detail/Publication/600362377?lang=en_GB); FWHM 3500 cm^-1 from the Report p.29 (secondary): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)
* Centre set to 510 nm (Report 500 nm; literature spread 490-520 nm) so the transmitted color is purple, not pink-red; peak 2.7 x R4's effective 2.0 cm^-1 (the Mn3+ fraction of the melt), set by the face-up target; same entry on both glasses; estimated

**Scaling.** k = 2.7 on R4's effective 2.0 cm^-1 peak.

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 510 | 1486.31 cm^-1 | 0.54 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: amethyst purple; nominal L* 50, C* 90, h 340 (a trade-color estimate, not a published measurement). Reached: L* 50.4, C* 89.4, h 344.4 (Lab 50.41, 86.15, -24.0); dE2000 to the nominal target 2.4.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [50.41, 86.15, -24.0], e-ray [50.41, 86.15, -24.0].

### Glass (N-BK7): Se Ruby

Entry key `glass__n_bk7_.se_ruby`; `estimated = true`. CdSe-CdS edge as 3 Gaussians (estimated; no Schott curve), 50 % edge near 605 nm.

**Sources.**

* CdS(1-x)Se(x) nanocrystal absorption edge (Report p.30, secondary: docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)); three Gaussians approximating a 10 cm^-1 plateau below 575 nm with a 12 nm e-fold edge (R4 section 6; 50 % transmission edge near 605 nm over 8.8 mm)
* Not calibrated against a Schott OG / RG filter curve (none obtained); same entry on both glasses; estimated

**Scaling.** k = 1 (R4's three Gaussians as listed).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 430 | 2800 cm^-1 | 0.75 | energy |
| 500 | 1300 cm^-1 | 0.51 | energy |
| 545 | 700 cm^-1 | 0.82 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: deep ruby red (no scaling: the edge position defines it); nominal L* 40, C* 102, h 43 (a trade-color estimate, not a published measurement). Reached: L* 40.4, C* 102.1, h 42.6 (Lab 40.43, 75.13, 69.14); dE2000 to the nominal target 0.5.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [40.43, 75.13, 69.14], e-ray [40.43, 75.13, 69.14].

### Glass (N-BK7): Se Pink

Entry key `glass__n_bk7_.se_pink`; `estimated = true`. Se0 band at 490 nm (estimated amplitude), pale pink-salmon.

**Sources.**

* Elemental selenium in silicate glass: extinction about 20 L/mol/cm near 485 nm (soda-lime-silica) and about 80 near 500 nm (soda-strontia-silica), https://oa.tib.eu/renate/items/4645b348-ddcf-4719-9a64-920775412cfc (via search summary); Report p.29-30 entry 500 nm, FWHM 3000 cm^-1 (estimate): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)
* One band at 490 nm (sigma 1274 cm^-1, FWHM 3000), peak 0.6 cm^-1 (about 30 ppm effective Se0, R4 section 6); same entry on both glasses; estimated

**Scaling.** k = 1 (R4 amplitude 0.6 cm^-1).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 490 | 1273.98 cm^-1 | 0.06 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: pale pink (decolorising selenium leaves a pink cast); nominal L* 92, C* 16, h 15 (a trade-color estimate, not a published measurement). Reached: L* 94.1, C* 15.3, h 25.5 (Lab 94.08, 13.83, 6.6); dE2000 to the nominal target 2.7.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [94.08, 13.83, 6.6], e-ray [94.08, 13.83, 6.6].

### Glass (F2): Co Blue

Entry key `glass__f2_.co_blue`; `estimated = true`. Co2+ triplet 537 / 595 / 640 nm, 0.06 wt% CoO (estimated).

**Sources.**

* Tetrahedral Co2+ triplet 537 / 595 / 640 nm (British Glass digital library, https://www.britglass.org.uk/knowledge-base/digital-library-and-information-services/effect-alkali-ions-optical-absorption); FWHM 1100 cm^-1 and 80 / 115 / 100 cm^-1 per wt% CoO from the Report p.29 (secondary): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)
* 0.06 wt% CoO (R4: 0.05 wt% is a saturated royal blue); the colorant bands do not depend on the host glass, so N-BK7 and F2 carry the same entry; estimated

**Scaling.** k = 6 (0.06 wt% CoO).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 537 | 467.127 cm^-1 | 0.48 | energy |
| 595 | 467.127 cm^-1 | 0.69 | energy |
| 640 | 467.127 cm^-1 | 0.6 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: royal / cobalt blue; nominal L* 45, C* 80, h 280 (a trade-color estimate, not a published measurement). Reached: L* 47.9, C* 92.1, h 286.2 (Lab 47.9, 25.67, -88.41); dE2000 to the nominal target 5.1.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [47.9, 25.67, -88.41], e-ray [47.9, 25.67, -88.41].

### Glass (F2): Cr Green

Entry key `glass__f2_.cr_green`; `estimated = true`. Cr3+ bands at 650 and 450 nm, 0.4 wt% Cr2O3 (estimated).

**Sources.**

* Octahedral Cr3+ at 650 and 450 nm (glass chemistry, https://www.thaiscience.info/journals/Article/TKJN/10974416.pdf); FWHM 2500 / 3000 cm^-1 and 11.0 cm^-1 per wt% Cr2O3 from the Report p.29 (secondary): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)
* 0.4 wt% Cr2O3 (reducing melt, Cr3+ only); same entry on both glasses; estimated

**Scaling.** k = 4 (0.4 wt% Cr2O3).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 650 | 1061.65 cm^-1 | 0.44 | energy |
| 450 | 1273.98 cm^-1 | 0.44 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: emerald-like green; nominal L* 75, C* 95, h 140 (a trade-color estimate, not a published measurement). Reached: L* 77.3, C* 111.7, h 138.5 (Lab 77.27, -83.66, 74.04); dE2000 to the nominal target 3.5.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [77.27, -83.66, 74.04], e-ray [77.27, -83.66, 74.04].

### Glass (F2): Mn Purple

Entry key `glass__f2_.mn_purple`; `estimated = true`. Mn3+ band at 510 nm (estimated strength), amethyst purple.

**Sources.**

* Octahedral Mn3+ 5Eg-5T2g near 500 nm (https://kirim.kmutt.ac.th/converis/portal/detail/Publication/600362377?lang=en_GB); FWHM 3500 cm^-1 from the Report p.29 (secondary): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)
* Centre set to 510 nm (Report 500 nm; literature spread 490-520 nm) so the transmitted color is purple, not pink-red; peak 2.7 x R4's effective 2.0 cm^-1 (the Mn3+ fraction of the melt), set by the face-up target; same entry on both glasses; estimated

**Scaling.** k = 2.7 on R4's effective 2.0 cm^-1 peak.

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 510 | 1486.31 cm^-1 | 0.54 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: amethyst purple; nominal L* 50, C* 90, h 340 (a trade-color estimate, not a published measurement). Reached: L* 50.4, C* 89.4, h 344.4 (Lab 50.41, 86.15, -24.0); dE2000 to the nominal target 2.4.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [50.41, 86.15, -24.0], e-ray [50.41, 86.15, -24.0].

### Glass (F2): Se Ruby

Entry key `glass__f2_.se_ruby`; `estimated = true`. CdSe-CdS edge as 3 Gaussians (estimated; no Schott curve), 50 % edge near 605 nm.

**Sources.**

* CdS(1-x)Se(x) nanocrystal absorption edge (Report p.30, secondary: docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)); three Gaussians approximating a 10 cm^-1 plateau below 575 nm with a 12 nm e-fold edge (R4 section 6; 50 % transmission edge near 605 nm over 8.8 mm)
* Not calibrated against a Schott OG / RG filter curve (none obtained); same entry on both glasses; estimated

**Scaling.** k = 1 (R4's three Gaussians as listed).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 430 | 2800 cm^-1 | 0.75 | energy |
| 500 | 1300 cm^-1 | 0.51 | energy |
| 545 | 700 cm^-1 | 0.82 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: deep ruby red (no scaling: the edge position defines it); nominal L* 40, C* 102, h 43 (a trade-color estimate, not a published measurement). Reached: L* 40.4, C* 102.1, h 42.6 (Lab 40.43, 75.13, 69.14); dE2000 to the nominal target 0.5.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [40.43, 75.13, 69.14], e-ray [40.43, 75.13, 69.14].

### Glass (F2): Se Pink

Entry key `glass__f2_.se_pink`; `estimated = true`. Se0 band at 490 nm (estimated amplitude), pale pink-salmon.

**Sources.**

* Elemental selenium in silicate glass: extinction about 20 L/mol/cm near 485 nm (soda-lime-silica) and about 80 near 500 nm (soda-strontia-silica), https://oa.tib.eu/renate/items/4645b348-ddcf-4719-9a64-920775412cfc (via search summary); Report p.29-30 entry 500 nm, FWHM 3000 cm^-1 (estimate): docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md (Report; secondary, unverified)
* One band at 490 nm (sigma 1274 cm^-1, FWHM 3000), peak 0.6 cm^-1 (about 30 ppm effective Se0, R4 section 6); same entry on both glasses; estimated

**Scaling.** k = 1 (R4 amplitude 0.6 cm^-1).

**Bands** (per millimetre, Napierian; the `o_ray` set is also the `e_ray` set).

| center_nm | sigma | peak (mm^-1) | shape |
|---|---|---|---|
| 490 | 1273.98 cm^-1 | 0.06 | energy |

**Target and reached** (D65, 7 mm stone, path 9.905 mm, unpolarised). Target: pale pink (decolorising selenium leaves a pink cast); nominal L* 92, C* 16, h 15 (a trade-color estimate, not a published measurement). Reached: L* 94.1, C* 15.3, h 25.5 (Lab 94.08, 13.83, 6.6); dE2000 to the nominal target 2.7.

**Per direction.** Isotropic: one color. Pinned Lab: o-ray [94.08, 13.83, 6.6], e-ray [94.08, 13.83, 6.6].
