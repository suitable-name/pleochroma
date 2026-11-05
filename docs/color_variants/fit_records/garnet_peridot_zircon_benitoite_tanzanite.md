# Fit records: garnets, peridot, zircon, benitoite, tanzanite (lane F3, 2026-10-27)

Entries of `crates/pleochroma/data/color_variants/garnet_peridot_zircon_benitoite_tanzanite.toml`; tests in
`crates/pleochroma/src/optics/materials/variants/tests_garnet_peridot_zircon_benitoite_tanzanite.rs`.
Research: `docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md`.

## Update 2026-10-27 (fix round X6)

Applies to every entry below; the per-entry sections were corrected where the data changed.

* **Estimated flag (owner decision O3, 2026-10-27).** `estimated = true` when the spectral shape is not a measured spectrum of that variety, OR the strength is not anchored to a measurement (a chosen strength counts: the Cornflower / Deep Blue / Pigeon's Blood convention of lane F1 now holds in every group); `estimated = false` only when the shape is a measured spectrum of that very variety at a strength that is itself measured or published. The first `sources` line of every entry is ONE short plain sentence that names the estimated part; the UI shows it as the tooltip "Estimated: <first line>" (no tooltip for a measured entry). Citations, URLs, DOIs and repository paths start on the second line.
* **Face-up color (decision D-G).** The swatches and pins of the Variants list show the ORIENTED face-up color, the color the renderer shows for the stone: with the c axis along the view the o-ray alone (uniaxial hosts with `c_axis` Y: Sapphire, Ruby, Zircon, Benitoite, Emerald, Morganite, Amethyst, Quartz, Rutile, Moissanite), (o + e) / 2 for the hosts whose c axis lies in the table plane (Aquamarine and Tourmaline, `c_axis` X), (alpha + beta) / 2 for biaxial hosts (gamma along `c_axis` Y: Topaz, Chrysoberyl, Alexandrite, Tanzanite, Peridot, Andalusite); isotropic hosts show the one color. The entry sections below keep the unpolarised mean (the fit target, what `body_colors` returns) in their "reached" lines and list the oriented color as "reached face-up (oriented ...)"; the tests pin both. The face-up path is 9.905 mm (`reference_path_mm` = the 7 mm stone width, decision D-A).
* **Colorless entries (decision D-C).** An entry with no bands is valid on any host; the `pleochroic = true` trick was removed from the colorless entries.
* **Measured entries in this file** (`estimated = false`): none. Every other one of the 21 entries is estimated.
* **F-17.** The DOI doi:10.2138/am.2007.2163 cited for `pyrope_garnet.rhodolite` could not be verified and is dropped; the Fe2+-Fe3+ IVCT band near 21 700 cm^-1 is attributed to Taran, Dyar and Matsyuk 2007, Am. Mineral. 92, 753-760. The measured data are the Caltech GRR 131 file; the literature only explains the assignment.
* **F-18 Zircon default.** The B1 default is measured U4+ lines with the measured radiation-damage continuum removed line by line: a constructed colorless-with-lines stone, not a measured one. The comment in `zircon_through_topaz.rs` states it.
* **`zircon.blue` (k 3.5)** and every chosen strength are now estimated (O3): the slab itself is 96.6 / 6.4 at 2.65 mm; the strength is a choice.

**Tooltip sentences and oriented face-up colors** (D65, 9.905 mm):

| key | host | estimated | tooltip sentence | face-up (oriented) L* / C* / h | unpolarised mean L* / C* / h |
|---|---|---|---|---|---|
| `pyrope_garnet.rhodolite` | Pyrope Garnet | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 131 rhodolite). | 47.7 / 39.7 / 355 | 47.7 / 39.7 / 355 |
| `pyrope_garnet.chrome_pyrope` | Pyrope Garnet | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 1055 pyrope). | 39.8 / 29.7 / 13 | 39.8 / 29.7 / 13 |
| `almandine_garnet.purplish` | Almandine Garnet | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 1776 almandine). | 35.3 / 46.8 / 12 | 35.3 / 46.8 / 12 |
| `spessartine_garnet.reddish_orange` | Spessartine Garnet | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 72 spessartine). | 61.4 / 73.5 / 66 | 61.4 / 73.5 / 66 |
| `spessartine_garnet.pale_golden_orange` | Spessartine Garnet | yes | Scaled copy of the Spessartine Material Default (GRR 43) at a chosen strength; no pale stone measured. | 88.5 / 39.8 / 76 | 88.5 / 39.8 / 76 |
| `grossular_garnet__tsavorite_.mint` | Grossular Garnet (Tsavorite) | yes | Scaled copy of the Tsavorite Material Default (GRR 229) at a chosen strength; no pale stone measured. | 89.0 / 41.3 / 133 | 89.0 / 41.3 / 133 |
| `grossular_garnet__tsavorite_.hessonite` | Grossular Garnet (Tsavorite) | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 1386 brown grossular). | 68.5 / 74.5 / 71 | 68.5 / 74.5 / 71 |
| `andradite_garnet__demantoid_.topazolite` | Andradite Garnet (Demantoid) | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 48 andradite). | 74.3 / 53.5 / 98 | 74.3 / 53.5 / 98 |
| `color_change_garnet__pyrope_spessartine_.tunduru` | Color-Change Garnet (Pyrope-Spessartine) | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 2225). | 57.1 / 13.4 / 186 | 57.1 / 13.4 / 186 |
| `color_change_garnet__pyrope_spessartine_.malaya` | Color-Change Garnet (Pyrope-Spessartine) | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 1267). | 58.8 / 64.3 / 61 | 58.8 / 64.3 / 61 |
| `color_change_garnet__pyrope_spessartine_.weaker_change` | Color-Change Garnet (Pyrope-Spessartine) | yes | Tunduru spectrum (GRR 2225) with only the 581 nm band scaled; no such stone measured. | 68.1 / 19.4 / 120 | 68.1 / 19.4 / 120 |
| `color_change_garnet__pyrope_spessartine_.stronger_change` | Color-Change Garnet (Pyrope-Spessartine) | yes | Tunduru spectrum (GRR 2225) with only the 581 nm band scaled; no such stone measured. | 43.5 / 28.6 / 236 | 43.5 / 28.6 / 236 |
| `peridot.deep_green` | Peridot | yes | Scaled copy of the Peridot Material Default (GRR 418) at a chosen strength; no deeper stone measured. | 67.2 / 73.3 / 109 | 69.0 / 72.0 / 110 |
| `zircon.blue` | Zircon | yes | Strength chosen for a typical face-up color (3.5 times the slab); spectrum measured (GRR 1898). | 60.1 / 56.3 / 250 | 74.4 / 32.1 / 246 |
| `zircon.golden_brown` | Zircon | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 3106 brown region). | 62.2 / 31.1 / 71 | 61.6 / 33.7 / 77 |
| `zircon.red_brown` | Zircon | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 1844). | 50.5 / 71.8 / 52 | 50.5 / 71.3 / 52 |
| `zircon.green` | Zircon | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 3106 green region). | 58.2 / 27.8 / 80 | 56.2 / 27.8 / 90 |
| `zircon.colorless` | Zircon | yes | Colorless: no damage centres assumed; low-uranium zircon. | 100.0 / 0.0 / 0 | 100.0 / 0.0 / 0 |
| `benitoite.light_blue` | Benitoite | yes | Strength estimated (2 percent of the dark zone); spectrum shape measured (GRR 561-D4). | 90.0 / 14.3 / 262 (face-up with c in the table since 2026-11-03; 99.6 / 0.4 / 279 with c through it) | 93.4 / 9.4 / 263 |
| `tanzanite.unheated` | Tanzanite | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 1265). | 38.6 / 22.5 / 314 | 46.3 / 4.4 / 162 |
| `tanzanite.green_zoisite` | Tanzanite | yes | Strength chosen for a typical face-up color; spectrum measured (CIT-16940 zoisite). | 48.5 / 34.3 / 94 | 55.9 / 24.9 / 117 |


## Conventions (all entries)

* **Path.** Every entry is `per_mm`, calibrated for a 7 mm stone. A per-mm tensor face-up in a 7 mm stone covers
  `MODEL_UNIT_FACE_UP_PATH x width / REFERENCE_MODEL_WIDTH = 2.83 x 7 / 2.0 = 9.905 mm` (`absorption_path_scale_for`
  gives 3.5 mm per model unit at 7 mm; the tracer path is 2.83 units; the shared helper
  `variants::face_up_swatch_path(PerMm, 7.0)` returns it). Fits, targets and the tests all use 9.905 mm. R3's tables
  used 8.8 mm (the pre-2026-10-27 constant 2.52) and a model-unit = 8.8 x per-mm rule; this lane refits from the CSVs
  at 9.905 mm, and converts the three scaled-copy entries from model units by dividing by 9.905 (exact at every stone size).
* **Color pipeline.** The app's: `color::body_color::body_colors` at 1 nm from 380 to 780 nm, D65 table (10 nm,
  interpolated), the CIE 1931 5 nm table interpolated, Lab relative to the illuminant's own white. Face-up is the
  unpolarised mix: isotropic o; uniaxial (2 T_o + T_e) / 3; biaxial (T_alpha + T_beta + T_gamma) / 3. A Python replica reading
  the CMF and D65 tables from the repo source reproduces the recorded swatches of `docs/chromophores/calibration-records.md`:
  Spessartine L* 76.7 C* 71.9 h 65.7 (record 77 / 72 / 66), Tsavorite 55.5 / 86.3 / 147 (56 / 86 / 147), Peridot mean
  75.8 / 63.3 / 111 (76 / 63 / 111), Demantoid 41.1 / 92.1 / 140 (41 / 92 / 140), each with the model-unit path 1.0.
* **Data to alpha.** `alpha per mm = ln(10) (A - baseline) / thickness_mm` (Napierian, baseline = the flat NIR or
  minimum level, which is the reflection loss and scatter; stated per entry). Below the first data point the spectrum is
  continued with the slope of its first 15 nm (absorbance never falls toward the UV), so an opaque edge stays opaque.
* **k.** The per-mm strength factor relative to the Caltech sample: k = 0.6 means 0.6 times the sample's chromophore content per mm
  of path. It is chosen per entry for a typical faceted stone of the variety (reasons given); nothing but the color target fixes it.
* **Fit.** Eight energy Gaussians per ray at most (`width_nm` = sigma in cm^-1). Stage 1: bounded least squares on the cube root of
  the transmittance (a lightness-like scale, weighted by the illuminant times the CMF) at 0.5, 1 and 2 times the face-up path against
  the 1 nm bin average of the 0.1 nm-grid data, started from R3's hand bands, then adding one band at a time where the residual is
  largest while that lowers the cost by more than 6 %. Stage 2: a joint polish of all rays that minimises the dE2000 vector of
  every direction and the face-up color (and, for the color-change garnets, the 3200 K face-up color) at 0.75, 1 and 1.4 times the
  path against the physical data colors, with the stage-1 residual as a regulariser. Bands with a visible effect below
  0.1 optical density at the face-up path are dropped. The bands are empirical: those below about 440 nm describe the ultraviolet
  edge; centres are not assigned to a chromophore unless stated. Scripts: the lane scratch folder, not the repo.
* **Rounding.** Centres 0.01 nm, sigma 0.1 cm^-1, peaks 4 significant digits; the colors below and the tests use the stored numbers.
* **Test.** The app's face-up and per-direction Lab of each entry (D65, 7 mm stone, 9.905 mm) must be within dE2000 1.0 of the
  `reached` numbers below; the incandescent colors of the four color-change garnets (Planckian 3200 K) too.
* **Estimated** (owner decision O3, 2026-10-27; see the Update section above) = the shape is not a measured spectrum of that variety, or the strength is not anchored to a measurement. Every entry here ships a chosen strength, so every entry is estimated.
* Colors are L* / C* / h(degrees). `dE vs data` compares the model with the physical (0.1 nm-grid) data color at k, at 0.5 x / 1 x / 2 x path.
* **B1 and the zircon default.** Zircon's Material Default is changed by lane B1; the zircon entries here are fitted from their own
  spectra, never derived from the default.

## Summary

| Entry | key | estimated | k / scale | bands per ray | face-up reached L* / C* / h | dE vs data (1 x) |
|---|---|---|---|---|---|---|
| Rhodolite (Pyrope Garnet) | `pyrope_garnet.rhodolite` | yes | k 0.6 | 8 | 47.7 / 39.7 / 355 | 0.00 |
| Chrome Pyrope (Pyrope Garnet) | `pyrope_garnet.chrome_pyrope` | yes | k 0.17 | 7 | 39.8 / 29.7 / 13 | 0.00 |
| Purplish (Almandine Garnet) | `almandine_garnet.purplish` | yes | k 0.3 | 8 | 35.3 / 46.8 / 12 | 0.00 |
| Reddish Orange (Spessartine Garnet) | `spessartine_garnet.reddish_orange` | yes | k 0.6 | 7 | 61.4 / 73.5 / 66 | 0.00 |
| Pale Golden Orange (Spessartine Garnet) | `spessartine_garnet.pale_golden_orange` | yes | x 0.4 of default | 8 | 88.5 / 39.8 / 76 | 0.00 |
| Mint (Grossular Garnet (Tsavorite)) | `grossular_garnet__tsavorite_.mint` | yes | x 0.5 of the default since 2026-11-04 (0.0875 x the sample's vanadium; it was 0.18 x, which the default reached that day) | 5 | 89.0 / 41.3 / 133 | 0.00 |
| Hessonite (Grossular Garnet (Tsavorite)) | `grossular_garnet__tsavorite_.hessonite` | yes | k 0.7 | 4 | 68.5 / 74.5 / 71 | 0.01 |
| Topazolite (Andradite Garnet (Demantoid)) | `andradite_garnet__demantoid_.topazolite` | yes | k 0.35 | 8 | 74.3 / 53.5 / 98 | 0.00 |
| Tunduru (Color-Change Garnet (Pyrope-Spessartine)) | `color_change_garnet__pyrope_spessartine_.tunduru` | yes | k 0.35 | 7 | 57.1 / 13.4 / 186 | 0.00 |
| Malaya (Color-Change Garnet (Pyrope-Spessartine)) | `color_change_garnet__pyrope_spessartine_.malaya` | yes | k 0.35 | 8 | 58.8 / 64.3 / 61 | 0.01 |
| Weaker Change (Color-Change Garnet (Pyrope-Spessartine)) | `color_change_garnet__pyrope_spessartine_.weaker_change` | yes | V x 0.6 | 7 | 68.1 / 19.4 / 120 | - |
| Stronger Change (Color-Change Garnet (Pyrope-Spessartine)) | `color_change_garnet__pyrope_spessartine_.stronger_change` | yes | V x 1.8 | 7 | 43.5 / 28.6 / 236 | - |
| Deep Green (Peridot) | `peridot.deep_green` | yes | x 1.4 of default | 6/6/6 | 69.0 / 72.0 / 110 | 0.00 |
| Blue (Zircon) | `zircon.blue` | yes | k 3.5 | 7/6 | 74.4 / 32.1 / 246 | 0.00 |
| Golden Brown (Zircon) | `zircon.golden_brown` | yes | k 0.9 | 8/8 | 61.6 / 33.7 / 77 | 0.00 |
| Red-Brown (Zircon) | `zircon.red_brown` | yes | k 0.75 | 3/4 | 50.5 / 71.3 / 52 | 0.02 |
| Green (Zircon) | `zircon.green` | yes | k 0.8 | 8/8 | 56.2 / 27.8 / 90 | 0.01 |
| Colorless (Zircon) | `zircon.colorless` | yes | - | 0/0 | 100.0 / 0.0 / 0 | - |
| Light Blue (Benitoite) | `benitoite.light_blue` | yes | k 0.02 | 1/2 | 93.4 / 9.4 / 263 | 0.30 |
| Unheated (Tanzanite) | `tanzanite.unheated` | yes | k 1.3 | 6/6/7 | 46.3 / 4.4 / 162 | 0.00 |
| Green Zoisite (Tanzanite) | `tanzanite.green_zoisite` | yes | k 1.5 | 6/6/6 | 55.9 / 24.9 / 117 | 0.00 |

### F3-01 Pyrope Garnet: Rhodolite

Key `pyrope_garnet.rhodolite`, estimated: **yes**.

* **Sources.** Caltech Mineral Spectroscopy Server (G. R. Rossman, CC BY-NC 4.0, DOI 10.7907/jywr-qq57): GRR 131 rhodolite (Mg1.82 Fe0.99 Ca0.16 Mn0.04 Al2.00, 2.007 mm), http://minerals.gps.caltech.edu/FILES/Visible/garnet/grr131, crates/pleochroma/data/reference_spectra/variant_pyrope_rhodolite_iso.csv; GRR 83 (Umba) agrees per almandine fraction; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 2.1.1 (Fe2+ 504 / 527 / 576 nm over a UV edge, Fe2+-Fe3+ IVCT continuum; the band near 21 700 cm^-1 in Fe-rich garnets: Taran, Dyar and Matsyuk 2007, Am. Mineral. 92, 753-760; the DOI doi:10.2138/am.2007.2163 cited earlier could not be verified and is dropped)
* **Cause.** Fe2+ on the dodecahedral site (spin-forbidden 504, 527-530, 576 nm, weak 610 and 690 nm), a rising ultraviolet edge and a broad Fe2+-Fe3+ continuum. About 33 mol% almandine, inside the rhodolite range.
* **Spectrum and baseline.** GRR 131, 2.007 mm, 365-1108 nm, baseline A = 0.031 removed (flat 800-870 nm, the reflection loss).
* **Scaling.** k = 0.6: the stone holds 0.6 times the sample's Fe per mm (Fe about 0.6 apfu, 20 mol% almandine). Chosen for a medium raspberry stone (L* about 48); k = 1 is dark (L* 30), k = 0.3 is pink-purple (L* 69).
* **Target.** Raspberry / purplish red, L* 48 C* 40 h 355 (documented: pink to purplish red, no pleochroism, cubic).
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 47.7 / 39.7 / 355; target 47.7 / 39.7 / 355; dE00 0.00
* **dE00 vs data at 0.5 x / 1 x / 2 x path:** max 0.01 / max 0.00 / max 0.01. Transmittance rms at 1 x path: o 0.014.
* **Pleochroism.** Cubic: one band set, no pleochroism.

Bands (per mm):

all rays (o = e):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 390.22 | 2367.4 | 5575 | 0.3065 |
| 427.95 | 537.4 | 1265 | 0.04581 |
| 462.64 | 500.8 | 1179 | 0.05525 |
| 501.21 | 218.0 | 513 | 0.06347 |
| 517.64 | 1033.5 | 2434 | 0.2754 |
| 575.31 | 404.0 | 951 | 0.1913 |
| 614.19 | 475.7 | 1120 | 0.09736 |
| 695.49 | 794.3 | 1870 | 0.07463 |


### F3-02 Pyrope Garnet: Chrome Pyrope

Key `pyrope_garnet.chrome_pyrope`, estimated: **yes**.

* **Sources.** Caltech Mineral Spectroscopy Server (G. R. Rossman, CC BY-NC 4.0, DOI 10.7907/jywr-qq57): GRR 1055 pyrope, Moses Rock, Utah (Ca0.33 Mn0.02 Mg2.13 Fe0.54 Al1.95 Cr0.07, 0.840 mm), http://minerals.gps.caltech.edu/FILES/Visible/garnet/Pyrope_grr1055.txt, crates/pleochroma/data/reference_spectra/variant_pyrope_chrome_iso.csv; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 2.1.2 (Cr3+ 415 and 571 nm plus Fe2+ 504 / 527 nm; GIA Gems & Gemology Winter 2015 pink pyrope)
* **Cause.** Cr3+ on the Y site (4A2 -> 4T1 near 410 nm, 4A2 -> 4T2 at 569 nm) plus Fe2+ 504 / 530 nm.
* **Spectrum and baseline.** GRR 1055, 0.840 mm, 350-1108 nm, baseline A = 0.007 removed. 1.24 wt% Cr2O3 in the sample (Cr 0.07 apfu), Fe 0.54 apfu.
* **Scaling.** k = 0.17 (0.21 wt% Cr2O3 equivalent): the full-strength sample is black at 9.9 mm (L* 3); a typical gem pyrope holds 0.05-1 wt% Cr2O3.
* **Target.** Medium-dark red, L* 40 C* 30 h 13 (documented: red; Cr pyrope is not pleochroic).
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 39.8 / 29.7 / 13; target 39.8 / 29.7 / 13; dE00 0.00
* **dE00 vs data at 0.5 x / 1 x / 2 x path:** max 0.00 / max 0.00 / max 0.00. Transmittance rms at 1 x path: o 0.023.
* **Pleochroism.** Cubic: one band set, no pleochroism.

Bands (per mm):

all rays (o = e):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 406.06 | 1565.2 | 3686 | 0.5633 |
| 428.73 | 766.8 | 1806 | 0.1434 |
| 462.73 | 707.3 | 1666 | 0.09431 |
| 503.76 | 830.7 | 1956 | 0.145 |
| 529.93 | 341.3 | 804 | 0.02126 |
| 568.89 | 982.8 | 2314 | 0.3682 |
| 686.30 | 586.2 | 1380 | 0.02624 |


### F3-03 Almandine Garnet: Purplish

Key `almandine_garnet.purplish`, estimated: **yes**.

* **Sources.** Caltech Mineral Spectroscopy Server (G. R. Rossman, CC BY-NC 4.0, DOI 10.7907/jywr-qq57): GRR 1776 almandine ([Fe2.5 Mg0.3 Mn0.2 Ca0.1]Al2.0(SiO4)3, 0.213 mm, merged file), crates/pleochroma/data/reference_spectra/variant_almandine_purplish_iso.csv; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 2.2.1 (Fe2+ 504 / 527 / 576 nm over a strong blue-ultraviolet edge, Fe2+-Fe3+ IVCT)
* **Cause.** Fe2+ spin-forbidden bands at 504, 527 and 576 nm over a broad blue edge (83 mol% almandine).
* **Spectrum and baseline.** GRR 1776, 0.213 mm, 370-1110 nm, baseline A = 0.089 removed (840-860 nm).
* **Scaling.** k = 0.3 (25 mol% almandine equivalent per mm of path): gem almandine is cut shallow; k = 1 is black, k = 0.5 L* 19.
* **Target.** Deep red with a slight violet cast, L* 35 C* 47 h 12 (documented: dark reddish purple; the sample's hue stays redder than purple, an honest limit).
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 35.3 / 46.8 / 12; target 35.3 / 46.8 / 12; dE00 0.00
* **dE00 vs data at 0.5 x / 1 x / 2 x path:** max 0.00 / max 0.00 / max 0.00. Transmittance rms at 1 x path: o 0.009.
* **Pleochroism.** Cubic: one band set, no pleochroism.

Bands (per mm):

all rays (o = e):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 400.36 | 3224.9 | 7594 | 0.3082 |
| 429.73 | 464.0 | 1093 | 0.05266 |
| 462.51 | 421.7 | 993 | 0.05352 |
| 502.22 | 217.0 | 511 | 0.0798 |
| 517.74 | 1102.9 | 2597 | 0.3543 |
| 575.60 | 403.6 | 950 | 0.2272 |
| 613.34 | 438.8 | 1033 | 0.1289 |
| 692.92 | 792.2 | 1865 | 0.0919 |


### F3-04 Spessartine Garnet: Reddish Orange

Key `spessartine_garnet.reddish_orange`, estimated: **yes**.

* **Sources.** Caltech Mineral Spectroscopy Server (G. R. Rossman, CC BY-NC 4.0, DOI 10.7907/jywr-qq57): GRR 72 spessartine, Rutherford Mine, Virginia (Mn2.79 Fe0.07 Mg0.04 Ca0.03 Al2.00, 3.446 mm), crates/pleochroma/data/reference_spectra/variant_spessartine_reddishorange_iso.csv (the file starts at 430 nm); docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 2.3.1 (Mn2+ 429 / 462 / 486 / 523 nm; Taran et al. 2023, Am. Min. 108)
* **Cause.** Mn2+ spin-forbidden bands (429, 462, 482, 524 nm) with Fe2+; Fe- and Mn-rich, deeper than the Material Default (GRR 43, near end-member).
* **Spectrum and baseline.** GRR 72, 3.446 mm, 430-1108 nm, baseline A = 0.300 removed. Below 430 nm the file is empty: the spectrum is continued with the slope of its first 15 nm; the stone is opaque there (alpha above 1.4 per mm) so the gap does not move the color.
* **Scaling.** k = 0.6 of the sample's Mn per mm (the sample is 2-3 times stronger per mm than GRR 43). k = 1 is L* 47, k = 0.3 L* 77.
* **Target.** Reddish orange, L* 61 C* 74 h 66 (documented: Fe- and Mn-rich spessartine is a deeper reddish orange than the Mandarin default).
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 61.4 / 73.5 / 66; target 61.4 / 73.5 / 66; dE00 0.00
* **dE00 vs data at 0.5 x / 1 x / 2 x path:** max 0.00 / max 0.00 / max 0.00. Transmittance rms at 1 x path: o 0.027.
* **Pleochroism.** Cubic: one band set, no pleochroism.

Bands (per mm):

all rays (o = e):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 394.42 | 1544.2 | 3636 | 2.033 |
| 434.50 | 207.9 | 490 | 0.1112 |
| 461.35 | 328.4 | 773 | 0.169 |
| 482.16 | 484.6 | 1141 | 0.2502 |
| 523.68 | 932.3 | 2195 | 0.2147 |
| 596.82 | 911.1 | 2145 | 0.06521 |
| 682.96 | 601.0 | 1415 | 0.02229 |


### F3-05 Spessartine Garnet: Pale Golden Orange

Key `spessartine_garnet.pale_golden_orange`, estimated: **yes**.

* **Sources.** Material Default of Spessartine Garnet (Caltech GRR 43, fitted 2026-10-19; docs/chromophores/calibration-records.md#spessartine) with every peak x 0.4, converted from model units to per mm by the 9.905 mm face-up path; estimated; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 2.3.2 (Mn2+ absorption scales linearly with the Mn fraction)
* **Cause.** The same Mn2+ bands at lower Mn content (spessartine-pyrope or -grossular solid solution).
* **Spectrum and baseline.** None new: the default's 8 bands, peaks x 0.4 / 9.905 (per mm).
* **Scaling.** k = 0.4 of the default (a choice; range 0.3-0.6 gives L* 91-84). The ultraviolet edge scales too, which is not guaranteed in nature. Estimated.
* **Target.** Pale yellow-orange, L* 88.5 C* 39.8 h 76 (the default scaled; no measured pale spessartine).
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 88.5 / 39.8 / 76; target 88.5 / 39.8 / 76; dE00 0.00
* **Pleochroism.** Cubic: one band set, no pleochroism.

Bands (per mm):

all rays (o = e):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 320.00 | 2982.4 | 7023 | 1.107 |
| 411.00 | 236.1 | 556 | 0.5734 |
| 421.00 | 177.9 | 419 | 0.2673 |
| 429.00 | 279.9 | 659 | 0.3328 |
| 462.00 | 658.6 | 1551 | 0.09006 |
| 485.00 | 344.4 | 811 | 0.06421 |
| 523.00 | 837.0 | 1971 | 0.0634 |
| 590.00 | 1274.0 | 3000 | 0.005331 |


### F3-06 Grossular Garnet (Tsavorite): Mint

Key `grossular_garnet__tsavorite_.mint`, estimated: **yes**.

* **Sources.** Caltech GRR 229 as fitted 2026-10-19 (docs/chromophores/calibration-records.md#tsavorite) at 0.0875 x the sample's vanadium (about 0.16 wt% V2O3), half the Material Default since 2026-11-04, converted to per mm by the 9.905 mm face-up path; estimated; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 2.4.1 (V3+ 436 / 606 nm). Until 2026-11-04 Mint was 0.18 x the vanadium (about 0.33 wt% V2O3, L* 79); the saturation round moved the default to 0.175 x, so Mint was halved to stay a paler stone.
* **Cause.** V3+ (traces of Cr3+) on the Y site, light type.
* **Spectrum and baseline.** None new: the default's 5 bands at half its strength, / 9.905 (per mm).
* **Scaling.** x 0.5 of the default = 0.0875 x the sample's vanadium (about 0.16 wt% V2O3). Estimated (scaling of a fitted measured spectrum).
* **Target.** Light mint green, L* 89 C* 41 h 133 (the default: L* 80 C* 62 h 137, dE2000 8.9).
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 89.0 / 41.3 / 133; target 89.0 / 41.3 / 133; dE00 0.00
* **Pleochroism.** Cubic: one band set, no pleochroism.

Bands (per mm):

all rays (o = e):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 393.00 | 1583.6 | 3729 | 0.1263 |
| 436.00 | 994.1 | 2341 | 0.1217 |
| 490.00 | 1061.7 | 2500 | 0.01961 |
| 606.00 | 1158.1 | 2727 | 0.05283 |
| 666.00 | 495.2 | 1166 | 0.01171 |


### F3-07 Grossular Garnet (Tsavorite): Hessonite

Key `grossular_garnet__tsavorite_.hessonite`, estimated: **yes**.

* **Sources.** Caltech Mineral Spectroscopy Server (G. R. Rossman, CC BY-NC 4.0, DOI 10.7907/jywr-qq57): GRR 1386 brown grossular, Lalatema Hills, Kenya (4.126 mm, composition unpublished), crates/pleochroma/data/reference_spectra/variant_grossular_hessonite_iso.csv; GRR 1113 as cross-check; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 2.4.2 (Fe3+ edge; the Caltech index calls the orange-brown grossulars hessonite)
* **Cause.** Fe3+ charge-transfer edge and a weak 440 nm Fe3+ band plus a broad band near 490 nm (assignment estimated).
* **Spectrum and baseline.** GRR 1386, 4.126 mm, 373-1108 nm, baseline A = 0.052 removed. The species label 'hessonite' is the trade's and Caltech's; the sample is documented only as brown grossular, no analysis. Marked estimated for that reason.
* **Scaling.** k = 0.7 of the sample per mm (honey to cinnamon orange); k = 1 is L* 60, k = 0.35 L* 84.
* **Target.** Orange-brown to honey, L* 68.5 C* 74.5 h 70.5 (documented: orange-brown).
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 68.5 / 74.5 / 71; target 68.5 / 74.5 / 70; dE00 0.01
* **dE00 vs data at 0.5 x / 1 x / 2 x path:** max 0.00 / max 0.01 / max 0.01. Transmittance rms at 1 x path: o 0.008.
* **Pleochroism.** Cubic: one band set, no pleochroism.

Bands (per mm):

all rays (o = e):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 369.91 | 3869.5 | 9112 | 0.4289 |
| 432.81 | 352.2 | 829 | 0.04451 |
| 466.89 | 981.0 | 2310 | 0.02636 |
| 488.42 | 2474.7 | 5827 | 0.1217 |


### F3-08 Andradite Garnet (Demantoid): Topazolite

Key `andradite_garnet__demantoid_.topazolite`, estimated: **yes**.

* **Sources.** Caltech Mineral Spectroscopy Server (G. R. Rossman, CC BY-NC 4.0, DOI 10.7907/jywr-qq57): GRR 48 andradite, Val Malenco, Italy (Ca3.0 Mg0.01 Fe1.98, 99 mol% andradite, 1.026 mm), http://minerals.gps.caltech.edu/FILES/Visible/garnet/grr48, crates/pleochroma/data/reference_spectra/variant_andradite_topazolite_iso.csv; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 2.5.1 (Fe3+ only, no Cr3+: charge-transfer edge and the 441 nm 6A1 -> 4A1,4E(G) band)
* **Cause.** Fe3+ only (no Cr3+): the O -> Fe3+ edge and the narrow 441 nm band.
* **Spectrum and baseline.** GRR 48, 1.026 mm, 390-1108 nm, baseline A = 0.196 removed; the 390 nm start is continued with the slope of its first 15 nm (opaque below).
* **Scaling.** k = 0.35 of the sample's iron per mm (gem topazolite is pale to medium yellow); k = 1 is olive yellow L* 42.
* **Target.** Greenish yellow, L* 74 C* 54 h 98 (documented: yellow to greenish yellow).
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 74.3 / 53.5 / 98; target 74.3 / 53.5 / 98; dE00 0.00
* **dE00 vs data at 0.5 x / 1 x / 2 x path:** max 0.00 / max 0.00 / max 0.00. Transmittance rms at 1 x path: o 0.020.
* **Pleochroism.** Cubic: one band set, no pleochroism. The four bands near 440 nm only shape the 420-445 nm edge; none is assigned on its own.

Bands (per mm):

all rays (o = e):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 383.45 | 604.7 | 1424 | 1.326 |
| 406.15 | 265.8 | 626 | 0.07532 |
| 424.14 | 820.7 | 1933 | 0.3109 |
| 429.59 | 2873.7 | 6767 | 0.1754 |
| 436.74 | 277.0 | 652 | 0.558 |
| 441.18 | 65.7 | 155 | 0.539 |
| 582.95 | 1211.1 | 2852 | 0.05616 |
| 638.21 | 587.3 | 1383 | 0.01646 |


### F3-09 Color-Change Garnet: Tunduru

Key `color_change_garnet__pyrope_spessartine_.tunduru`, estimated: **yes**.

* **Sources.** Caltech Mineral Spectroscopy Server (G. R. Rossman, CC BY-NC 4.0, DOI 10.7907/jywr-qq57): GRR 2225 pyrope-spessartine, Tunduru, Tanzania (about 0.6 wt% V, 2.426 mm, 412-1101 nm), http://minerals.gps.caltech.edu/FILES/Visible/garnet/Pyrope2225.asc, crates/pleochroma/data/reference_spectra/variant_colorchange_tunduru_iso.csv; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 2.6.1 (V3+ / Cr3+ 4T2 band near 580 nm between a blue-green and a red window; GIA Gems & Gemology Winter 2015)
* **Cause.** A V3+ / Cr3+ band at 581 nm between a blue-green window near 480 nm and a red window beyond 650 nm, with the Mn2+ edge cutting the violet.
* **Spectrum and baseline.** GRR 2225, 2.426 mm, 412-1101 nm, baseline A = 0.655 removed (flat 800-880 nm; whether it is a real plateau or reflection / scatter is not stated). The 380-412 nm region is not measured: it is continued with the slope of the first 15 nm and the bands below 440 nm are an opaque edge, NOT individual Mn2+ lines. Fit objective includes the 3200 K (Incandescent) weights, so the D65 -> lamp shift is fitted, not hoped for.
* **Scaling.** k = 0.35 of the sample's vanadium per mm (the sample has about 0.6 wt% V, so about 0.2 wt% V). Chosen for a medium-tone stone: k = 0.3 L* 62, k = 0.5 L* 46.
* **Target.** D65 grey-blue-green L* 57 C* 13 h 186; Incandescent (3200 K) purple L* 56 C* 8 h 276; hue shift 90 degrees, dE00 15.8 (documented: pale purplish blue in daylight to light purple under a lamp).
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 57.1 / 13.4 / 186; target 57.1 / 13.4 / 186; dE00 0.00
* **dE00 vs data at 0.5 x / 1 x / 2 x path:** max 0.02 / max 0.00 / max 0.04. Transmittance rms at 1 x path: o 0.022.
* **Incandescent (Planckian 3200 K) face-up:** 55.8 / 7.9 / 276; D65 to lamp dE00 15.8, hue shift 90 degrees.
* **Pleochroism.** Cubic: one band set, no pleochroism.

Bands (per mm):

all rays (o = e):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 386.64 | 1737.7 | 4092 | 0.7517 |
| 425.55 | 599.9 | 1413 | 0.2095 |
| 432.28 | 108.7 | 256 | 0.05158 |
| 449.45 | 262.6 | 618 | 0.01824 |
| 460.80 | 277.5 | 653 | 0.02333 |
| 492.98 | 1297.1 | 3054 | 0.06316 |
| 580.85 | 1173.0 | 2762 | 0.214 |


### F3-10 Color-Change Garnet: Malaya

Key `color_change_garnet__pyrope_spessartine_.malaya`, estimated: **yes**.

* **Sources.** Caltech Mineral Spectroscopy Server (G. R. Rossman, CC BY-NC 4.0, DOI 10.7907/jywr-qq57): GRR 1267 pyrope-spessartine, Kenya (composition unpublished, 1.686 mm), http://minerals.gps.caltech.edu/FILES/Visible/garnet/GRR1267b.txt, crates/pleochroma/data/reference_spectra/variant_colorchange_malaya_iso.csv; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 2.6.3 (Mn2+ lines and Fe2+ over a plateau; enough Fe2+ to fill the 480-520 nm window, so no color change)
* **Cause.** Mn2+ lines at 411 / 427 nm and Fe2+ over a broad 450-520 nm plateau.
* **Spectrum and baseline.** GRR 1267, 1.686 mm, 378-1108 nm, baseline A = 0.152 removed. Composition unknown, so the Mn : Fe split is unknown.
* **Scaling.** k = 0.35 of the sample per mm (light orange stone); k = 0.5 L* 48, k = 1 L* 27.
* **Target.** Orange-pink, L* 59 C* 64 h 61; Incandescent L* 63 C* 72 h 59, dE00 4.5, so almost no color change (documented: Malaya does not change color noticeably).
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 58.8 / 64.3 / 61; target 58.8 / 64.3 / 61; dE00 0.01
* **dE00 vs data at 0.5 x / 1 x / 2 x path:** max 0.01 / max 0.01 / max 0.01. Transmittance rms at 1 x path: o 0.052.
* **Incandescent (Planckian 3200 K) face-up:** 63.1 / 72.0 / 59; D65 to lamp dE00 4.5, hue shift 358 degrees.
* **Pleochroism.** Cubic: one band set, no pleochroism.

Bands (per mm):

all rays (o = e):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 383.78 | 1112.7 | 2620 | 0.6396 |
| 410.67 | 258.7 | 609 | 0.3175 |
| 426.67 | 574.7 | 1353 | 0.3816 |
| 454.77 | 533.7 | 1257 | 0.08128 |
| 488.66 | 1831.3 | 4312 | 0.252 |
| 572.00 | 352.4 | 830 | 0.04042 |
| 607.75 | 417.6 | 983 | 0.0199 |
| 691.90 | 7082.6 | 16678 | 0.02893 |


### F3-11 Color-Change Garnet: Weaker Change

Key `color_change_garnet__pyrope_spessartine_.weaker_change`, estimated: **yes**.

* **Sources.** Derived from the fitted Tunduru entry (Caltech GRR 2225): the 581 nm V3+ / Cr3+ band x 0.6, every other band unchanged; estimated, no measured weaker stone; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 2.6.2
* **Cause.** Less V3+ / Cr3+ than Tunduru: the stone tilts toward the Mn orange-yellow (daylight yellow-green, lamp yellow).
* **Spectrum and baseline.** None: band x 0.6 of the Tunduru fit.
* **Scaling.** V band x 0.6 (an assumed 0.12 wt% V equivalent). Estimated.
* **Target.** D65 yellowish green, Incandescent yellow (model values below).
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 68.1 / 19.4 / 120
* **Incandescent (Planckian 3200 K) face-up:** 67.9 / 12.6 / 72; D65 to lamp dE00 15.6, hue shift 313 degrees.
* **Pleochroism.** Cubic: one band set, no pleochroism.

Bands (per mm):

all rays (o = e):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 386.64 | 1737.7 | 4092 | 0.7517 |
| 425.55 | 599.9 | 1413 | 0.2095 |
| 432.28 | 108.7 | 256 | 0.05158 |
| 449.45 | 262.6 | 618 | 0.01824 |
| 460.80 | 277.5 | 653 | 0.02333 |
| 492.98 | 1297.1 | 3054 | 0.06316 |
| 580.85 | 1173.0 | 2762 | 0.1284 |


### F3-12 Color-Change Garnet: Stronger Change

Key `color_change_garnet__pyrope_spessartine_.stronger_change`, estimated: **yes**.

* **Sources.** Derived from the fitted Tunduru entry (Caltech GRR 2225): the 581 nm V3+ / Cr3+ band x 1.8, every other band unchanged; estimated, no measured stronger stone; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 2.6.2
* **Cause.** More V3+ / Cr3+ than Tunduru: the stone tilts toward blue (daylight blue, lamp blue-violet).
* **Spectrum and baseline.** None: band x 1.8 of the Tunduru fit.
* **Scaling.** V band x 1.8 (an assumed 0.36 wt% V equivalent). Estimated.
* **Target.** D65 blue, Incandescent blue-violet (model values below).
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 43.5 / 28.6 / 236
* **Incandescent (Planckian 3200 K) face-up:** 40.4 / 33.1 / 263; D65 to lamp dE00 9.2, hue shift 27 degrees.
* **Pleochroism.** Cubic: one band set, no pleochroism.

Bands (per mm):

all rays (o = e):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 386.64 | 1737.7 | 4092 | 0.7517 |
| 425.55 | 599.9 | 1413 | 0.2095 |
| 432.28 | 108.7 | 256 | 0.05158 |
| 449.45 | 262.6 | 618 | 0.01824 |
| 460.80 | 277.5 | 653 | 0.02333 |
| 492.98 | 1297.1 | 3054 | 0.06316 |
| 580.85 | 1173.0 | 2762 | 0.3852 |


### F3-13 Peridot: Deep Green

Key `peridot.deep_green`, estimated: **yes**.

* **Sources.** Material Default of Peridot (Caltech GRR 418 San Carlos alpha / beta / gamma, fitted 2026-10-19; docs/chromophores/calibration-records.md#peridot) with every peak x 1.4, converted to per mm by the 9.905 mm face-up path; estimated; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 3.1 (Fe2+ scaling with the fayalite content, Report p. 46; Cr3+ and Ni2+ of the deepest Burmese and Pakistani stones are not modelled)
* **Cause.** Fe2+ on M1 / M2 (NIR bands whose flank blocks the red, 403-490 and 635 nm bands, UV edge) at a higher fayalite content.
* **Spectrum and baseline.** None new: the default's 6 bands per ray, peaks x 1.4 / 9.905 (per mm).
* **Scaling.** x 1.4 (Fo 86 against an assumed Fo 90 for San Carlos); estimated, no fayalite analysis is published for GRR 418.
* **Target.** Rich yellowish green, face-up L* 69 C* 72 h 110.
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 69.0 / 72.0 / 110; target 69.0 / 72.0 / 110; dE00 0.00
  * o / alpha: reached 79.0 / 78.9 / 109; target 79.0 / 78.9 / 109; dE00 0.00
  * beta: reached 50.6 / 70.8 / 109; target 50.6 / 70.8 / 109; dE00 0.00
  * e / gamma: reached 72.3 / 70.5 / 112; target 72.3 / 70.5 / 112; dE00 0.01
* **Pleochroism.** Alpha L* 79, beta L* 51 (darkest), gamma L* 72, hue 108-113 in every ray: the pleochroism is in strength, not hue, as documented (trichroic, weak).

Bands (per mm):

o / alpha:

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 345.00 | 4287.4 | 10096 | 0.7604 |
| 455.00 | 598.8 | 1410 | 0.07491 |
| 490.00 | 917.3 | 2160 | 0.06219 |
| 637.00 | 732.1 | 1724 | 0.1131 |
| 840.00 | 1391.2 | 3276 | 0.6728 |
| 1000.00 | 1076.1 | 2534 | 4.975 |

beta:

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 341.00 | 5095.9 | 12000 | 0.9583 |
| 465.00 | 1072.7 | 2526 | 0.1512 |
| 500.00 | 1274.0 | 3000 | 0.06643 |
| 635.00 | 1061.7 | 2500 | 0.188 |
| 840.00 | 1698.6 | 4000 | 0.6007 |
| 1000.00 | 1019.6 | 2401 | 4.975 |

gamma:

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 376.00 | 1408.6 | 3317 | 0.6742 |
| 440.00 | 1245.5 | 2933 | 0.3166 |
| 500.00 | 1274.0 | 3000 | 0.1117 |
| 627.00 | 1061.7 | 2500 | 0.1088 |
| 840.00 | 1698.6 | 4000 | 0.3307 |
| 1000.00 | 1555.5 | 3663 | 1.795 |


### F3-14 Zircon: Blue

Key `zircon.blue`, estimated: **yes**.

* **Sources.** Caltech Mineral Spectroscopy Server (G. R. Rossman, CC BY-NC 4.0, DOI 10.7907/jywr-qq57): GRR 1898 zircon, Burma near the Thai border, blue, probably heat treated (2.65 mm, E perp c and E parallel c), http://minerals.gps.caltech.edu/FILES/Visible/zircon/Zir1898.a and Zir1898.c, crates/pleochroma/data/reference_spectra/variant_zircon_blue_eperpc.csv and variant_zircon_blue_eparc.csv; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 4.1 (heat-induced electron-trap centre near U4+, 650 nm band along E perp c; GIA Lab Notes Summer 2015; Report p. 19)
* **Cause.** A heat-induced centre near U4+: a broad band near 640 nm (FWHM 3700 cm^-1) polarized along E perpendicular to c, e-ray absorption below 0.01 per mm in the visible.
* **Spectrum and baseline.** GRR 1898, 2.65 mm, 350-1104 nm, baselines A = 0.300 (o) and 0.200 (e) removed.
* **Scaling.** k = 3.5 on both rays: the Caltech slab is pale to medium blue; a saturated gem holds about 3.5 times more of the centre per mm (R3: k = 2-3, estimated from the sample's look). Stated, not measured.
* **Target.** Sky to medium blue, face-up L* 74 C* 32 h 246 (documented: blue, strongly dichroic, blue along E perp c and nearly colorless along c).
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 74.4 / 32.1 / 246; target 74.4 / 32.1 / 246; dE00 0.00
  * o / alpha: reached 60.1 / 56.3 / 250; target 60.1 / 56.3 / 250; dE00 0.01
  * e / gamma: reached 94.1 / 8.5 / 163; target 94.1 / 8.4 / 163; dE00 0.00
* **dE00 vs data at 0.5 x / 1 x / 2 x path:** max 0.00 / max 0.01 / max 0.01. Transmittance rms at 1 x path: o 0.031, e 0.034.
* **Pleochroism.** o-ray (E perp c) L* 60 C* 56 h 250 blue; e-ray (along c) L* 94 C* 8.5, almost colorless. Strong dichroism, as documented. The e-ray hue (163) is meaningless at C* 8.

Bands (per mm):

o (E perp c):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 352.00 | 1153.7 | 2717 | 0.4876 |
| 457.61 | 1306.3 | 3076 | 0.02373 |
| 483.34 | 172.3 | 406 | 0.01021 |
| 516.62 | 187.7 | 442 | 0.0122 |
| 521.51 | 1222.6 | 2879 | 0.03044 |
| 537.90 | 50.9 | 120 | 0.01126 |
| 641.63 | 1577.8 | 3715 | 0.4006 |

e / gamma (E parallel c):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 387.23 | 1840.3 | 4334 | 0.1205 |
| 518.57 | 420.9 | 991 | 0.008134 |
| 589.22 | 63.8 | 150 | 0.01498 |
| 598.23 | 1529.3 | 3601 | 0.02073 |
| 615.70 | 93.3 | 220 | 0.01189 |
| 654.37 | 160.6 | 378 | 0.02433 |


### F3-15 Zircon: Golden Brown

Key `zircon.golden_brown`, estimated: **yes**.

* **Sources.** Caltech Mineral Spectroscopy Server (G. R. Rossman, CC BY-NC 4.0, DOI 10.7907/jywr-qq57): GRR 3106 zircon, Mogok, Myanmar, light brown region (3.806 mm), http://minerals.gps.caltech.edu/FILES/Visible/zircon/Zircon3106-brown_a.asc and _c.asc, crates/pleochroma/data/reference_spectra/variant_zircon_golden_eperpc.csv and variant_zircon_golden_eparc.csv; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 4.2 (radiation-damage continuum over weak U4+ lines)
* **Cause.** A radiation-damage (alpha-recoil) continuum rising toward the UV over weak U4+ lines (515.5, 589.3, 654.8, 691 nm along o; 510, 588, 617, 652, 661 nm along e).
* **Spectrum and baseline.** GRR 3106 light brown region, 3.806 mm, baselines A = 0.110 (o) and 0.105 (e). The sample is 'pale brown'; the golden trade color is made by air heat treatment and has no spectrum anywhere: marked estimated.
* **Scaling.** k = 0.9 of the sample per mm. k = 0.5 L* 76 (pale yellow), k = 2 L* 35 (brown).
* **Target.** Golden brown, face-up L* 62 C* 34 h 77.
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 61.6 / 33.7 / 77; target 61.6 / 33.7 / 77; dE00 0.00
  * o / alpha: reached 62.2 / 31.1 / 71; target 62.2 / 31.1 / 71; dE00 0.01
  * e / gamma: reached 60.3 / 41.1 / 87; target 60.3 / 41.1 / 87; dE00 0.01
* **dE00 vs data at 0.5 x / 1 x / 2 x path:** max 0.04 / max 0.01 / max 0.09. Transmittance rms at 1 x path: o 0.050, e 0.061.
* **Pleochroism.** o-ray L* 62 C* 31 h 71, e-ray L* 60 C* 41 h 87: nearly equal, the e-ray slightly yellower. The sharp U4+ lines (0.1-0.4 nm wide in the spectrum) are fitted no narrower than about 0.4 nm FWHM: the app samples color at 1 nm, and the fit target is the 1 nm bin average of the 0.1 nm-grid data, so each bin keeps its integrated absorption.

Bands (per mm):

o (E perp c):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 401.39 | 2244.5 | 5285 | 0.2944 |
| 496.17 | 1214.1 | 2859 | 0.1352 |
| 515.53 | 149.9 | 353 | 0.1125 |
| 588.48 | 2266.6 | 5337 | 0.07763 |
| 589.29 | 107.6 | 253 | 0.09317 |
| 654.07 | 40.0 | 94 | 0.3361 |
| 654.82 | 186.6 | 439 | 0.1741 |
| 690.80 | 40.0 | 94 | 0.1724 |

e / gamma (E parallel c):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 393.09 | 2710.1 | 6382 | 0.3046 |
| 478.34 | 691.3 | 1628 | 0.09601 |
| 510.19 | 159.5 | 376 | 0.1045 |
| 527.61 | 3001.6 | 7068 | 0.1102 |
| 588.34 | 96.9 | 228 | 0.1336 |
| 617.15 | 110.7 | 261 | 0.1252 |
| 652.44 | 66.2 | 156 | 0.1872 |
| 660.95 | 41.5 | 98 | 0.1924 |


### F3-16 Zircon: Red-Brown

Key `zircon.red_brown`, estimated: **yes**.

* **Sources.** Caltech Mineral Spectroscopy Server (G. R. Rossman, CC BY-NC 4.0, DOI 10.7907/jywr-qq57): GRR 1844 zircon, Chanthaburi, Thailand, brownish red, radiation damage (1.519 mm), http://minerals.gps.caltech.edu/FILES/Visible/zircon/zi1844.a and zi1844.c, crates/pleochroma/data/reference_spectra/variant_zircon_redbrown_eperpc.csv and variant_zircon_redbrown_eparc.csv; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 4.3 (alpha-recoil damage band near 485 nm, Report p. 19)
* **Cause.** Alpha-recoil radiation damage: a continuum rising toward the UV with a broad band near 485-515 nm.
* **Spectrum and baseline.** GRR 1844, 1.519 mm, baseline A = 0.215 removed (780 nm; the curve is still falling). The two polarizations agree to within 0.05 absorbance above 440 nm: red-brown is NOT pleochroic in the visible. The o and e sets are fitted from their own spectra (nearly equal) and stored pleochroic only because the host is uniaxial. The e-ray file's first row (76 nm) is a typo in the source and is dropped.
* **Scaling.** k = 0.75 of the sample per mm; k = 0.5 L* 62, k = 1 L* 42.
* **Target.** Hyacinth red-brown, L* 50.5 C* 71 h 52.
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 50.5 / 71.3 / 52; target 50.5 / 71.4 / 52; dE00 0.02
  * o / alpha: reached 50.5 / 71.8 / 52; target 50.5 / 71.9 / 52; dE00 0.03
  * e / gamma: reached 50.5 / 70.4 / 51; target 50.5 / 70.4 / 51; dE00 0.01
* **dE00 vs data at 0.5 x / 1 x / 2 x path:** max 0.11 / max 0.03 / max 0.18. Transmittance rms at 1 x path: o 0.015, e 0.015.
* **Pleochroism.** o and e agree to 0.5 L* / 1.5 C* / 1 degree hue, as the spectra do: no visible dichroism.

Bands (per mm):

o (E perp c):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 382.84 | 4475.3 | 10539 | 0.4435 |
| 484.45 | 1559.8 | 3673 | 0.05548 |
| 517.92 | 1797.7 | 4233 | 0.1436 |

e / gamma (E parallel c):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 312.87 | 6402.2 | 15076 | 0.8749 |
| 452.55 | 202.3 | 476 | 0.02599 |
| 469.66 | 1302.3 | 3067 | 0.03904 |
| 513.89 | 1675.6 | 3946 | 0.1869 |


### F3-17 Zircon: Green

Key `zircon.green`, estimated: **yes**.

* **Sources.** Caltech Mineral Spectroscopy Server (G. R. Rossman, CC BY-NC 4.0, DOI 10.7907/jywr-qq57): GRR 3106 zircon, Mogok, Myanmar, green region (2.592 mm, E perp c and E parallel c), http://minerals.gps.caltech.edu/FILES/Visible/zircon/Zircon3106-GREEN_a.asc and _c.asc, crates/pleochroma/data/reference_spectra/variant_zircon_green_eperpc.csv and variant_zircon_green_eparc.csv; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 4.4 (U4+ lines 482.8, 515.7, 537.2, 562.3, 589.7, 653.4, 690.9 nm over a damage continuum)
* **Cause.** U4+ sharp lines over a broad radiation-damage continuum (a crystalline Mogok crystal, not stated metamict).
* **Spectrum and baseline.** GRR 3106 green, 2.592 mm, baselines A = 0.105 (o) and 0.060 (e), 720 nm minimum.
* **Scaling.** k = 0.8 of the sample per mm (k = 0.5 L* 68, k = 1.5 L* 36).
* **Target.** Olive to yellowish green, face-up L* 56 C* 28 h 90 (documented: green, the lines carry the color).
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 56.2 / 27.8 / 90; target 56.1 / 27.8 / 90; dE00 0.01
  * o / alpha: reached 58.2 / 27.8 / 80; target 58.2 / 27.8 / 80; dE00 0.01
  * e / gamma: reached 51.7 / 31.1 / 111; target 51.7 / 31.2 / 111; dE00 0.02
* **dE00 vs data at 0.5 x / 1 x / 2 x path:** max 0.06 / max 0.02 / max 0.15. Transmittance rms at 1 x path: o 0.118, e 0.068.
* **Pleochroism.** o-ray L* 58 C* 28 h 80 (yellow-olive), e-ray L* 52 C* 31 h 111 (greener), as the data. RESIDUAL: 8 bands per ray cannot hold 7 lines plus the continuum (the 483 and 537 nm o-lines and the 465, 485, 542 nm e-lines are merged into the broad bands), so the transmittance spectrum has an rms error of 0.12 (o) and 0.07 (e) at the face-up path; the COLORS are fitted directly and agree with the physical (0.1 nm grid) data colors to dE00 below 0.2 at 0.5 x, 1 x and 2 x path. Lines are fitted no narrower than about 0.4 nm FWHM against the 1 nm bin average of the 0.1 nm-grid data, because the app samples color at 1 nm.

Bands (per mm):

o (E perp c):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 415.16 | 1014.2 | 2388 | 0.4794 |
| 499.14 | 1451.3 | 3418 | 0.2501 |
| 515.14 | 152.3 | 359 | 0.3709 |
| 589.08 | 136.9 | 322 | 0.2477 |
| 643.89 | 125.1 | 295 | 0.1232 |
| 645.15 | 835.3 | 1967 | 0.1388 |
| 655.92 | 117.5 | 277 | 0.8311 |
| 690.43 | 52.0 | 122 | 0.3359 |

e / gamma (E parallel c):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 409.45 | 1189.3 | 2801 | 0.5785 |
| 495.46 | 1809.3 | 4261 | 0.2346 |
| 510.08 | 127.6 | 300 | 0.2555 |
| 587.70 | 130.5 | 307 | 0.3194 |
| 614.37 | 547.1 | 1288 | 0.1316 |
| 617.77 | 106.2 | 250 | 0.356 |
| 655.71 | 175.2 | 413 | 0.4776 |
| 683.81 | 122.9 | 289 | 0.09348 |


### F3-18 Zircon: Colorless

Key `zircon.colorless`, estimated: **yes**.

* **Sources.** No spectrum: low-uranium zircon without damage centres (Caltech zircon page: 'colorless zircons have little uranium'); estimated; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 4.5
* **Cause.** No visible chromophore.
* **Spectrum and baseline.** None: empty o and e band sets stored pleochroic (valid for a pleochroic host).
* **Scaling.** None.
* **Target.** Colorless, L* 100 C* 0.
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 100.0 / 0.0 / 0
  * o / alpha: reached 100.0 / 0.0 / 0
  * e / gamma: reached 100.0 / 0.0 / 0
* **Pleochroism.** None.

Bands (per mm):

o (E perp c):

(empty)

e / gamma (E parallel c):

(empty)


### F3-19 Benitoite: Light Blue

Key `benitoite.light_blue`, estimated: **yes**.

* **Sources.** Caltech Mineral Spectroscopy Server (G. R. Rossman, CC BY-NC 4.0, DOI 10.7907/jywr-qq57): GRR 561-D4 benitoite, dark blue zone, The Gem Mine, San Benito County (0.997 mm), http://minerals.gps.caltech.edu/FILES/Visible/benitoite/benitoite561-D4D.A.txt and .C.txt, crates/pleochroma/data/reference_spectra/variant_benitoite_lightblue_eperpc.csv and variant_benitoite_lightblue_eparc.csv (shape only); docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 5.1; Laurs, Rohtert and Gray, Gems & Gemology 33(3), 1997, 166-187 (the cause of color is unresolved; blue FeO 0.20 wt%, colorless 0.04 wt%)
* **Cause.** Unresolved (Fe2+-Ti4+ or Fe2+-Fe3+ charge transfer proposed): a broad band near 700 nm running into the NIR, polarized along c.
* **Spectrum and baseline.** GRR 561-D4 dark blue zone, 0.997 mm, baseline A = 0.46 removed. The e-ray shape is fitted (698 nm band plus a 529 nm shoulder); the o-ray is the measured 603 nm band (FWHM 8630 cm^-1, 0.06 per mm) times k.
* **Scaling.** k = 0.02 of the dark zone (e-ray 6.3 per mm -> 0.12 per mm), about half the Material Default's strength, which is itself 3.6 % of the dark zone; no concentration is published for the measured piece, so the strength is ESTIMATED. The ultraviolet edge is not modelled.
* **Target.** Very pale violetish blue, face-up L* 93 C* 9 h 262.
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 93.4 / 9.4 / 263; target 93.5 / 9.4 / 261; dE00 0.30
  * o / alpha: reached 99.6 / 0.4 / 279; target 99.7 / 0.5 / 243; dE00 0.47
  * e / gamma: reached 78.3 / 32.0 / 261; target 78.3 / 32.0 / 261; dE00 0.00
* **dE00 vs data at 0.5 x / 1 x / 2 x path:** max 0.23 / max 0.47 / max 0.92. Transmittance rms at 1 x path: o 0.007, e 0.009.
* **Pleochroism.** e-ray (along c) L* 78 C* 32 h 261 blue; o-ray colorless (L* 99.6): strongly dichroic, 'violetish blue and colorless' (GIA 1997), as documented.

Bands (per mm):

o (E perp c):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 603.00 | 3664.8 | 8630 | 0.0012 |

e / gamma (E parallel c):

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 528.87 | 1934.3 | 4555 | 0.01328 |
| 697.87 | 2848.6 | 6708 | 0.1237 |


### F3-20 Tanzanite: Unheated

Key `tanzanite.unheated`, estimated: **yes**.

* **Sources.** Caltech Mineral Spectroscopy Server (G. R. Rossman, CC BY-NC 4.0, DOI 10.7907/jywr-qq57): GRR 1265 zoisite, Tanzania (V2O3 0.35, FeO 0.027), one crystal, alpha / beta / gamma at 3.00 mm normalized, http://minerals.gps.caltech.edu/FILES/Visible/zoisite/zo1265.a, .b, .c, crates/pleochroma/data/reference_spectra/variant_tanzanite_unheated_alpha.csv, _beta.csv, _gamma.csv; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 6.1 (V3+ bands near 600 and 740 nm plus the Fe2+-Ti4+ IVCT band near 440 nm along gamma; Hurlbut, Am. Min. 54, 702, 1969; Schwarzinger, Ulatowski and Rossman, IGC 2025; Pluthametwisute et al., Molecules 25, 3743, 2020)
* **Cause.** V3+ (about 540 and 600 nm, NIR 760-780 nm) plus the Fe2+-Ti4+ IVCT near 440-450 nm along gamma, which gives the brown to yellow direction and is removed by heating.
* **Spectrum and baseline.** GRR 1265, 3.00 mm, 401-1104 nm, baselines A = 0.038 / 0.035 / 0.046 removed (1000 nm). The UV edge below 401 nm is continued with the slope of the first 15 nm (steep in gamma).
* **Scaling.** k = 1.3 (V2O3 about 0.46 wt% equivalent), chosen so the stone's face-up tone matches the Material Default tanzanite (L* 45); equal vanadium to the default (0.6 wt%) would be k = 1.7.
* **Target.** Trichroic: alpha red-violet, beta blue-grey, gamma yellow-green; face-up a dark neutral grey-olive (L* 46 C* 4), which is the orientation average, as R3 found (Caltech: greenish yellow to deep blue to red-violet).
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 46.3 / 4.4 / 162; target 46.3 / 4.4 / 162; dE00 0.00
  * o / alpha: reached 36.4 / 39.7 / 330; target 36.4 / 39.7 / 330; dE00 0.00
  * beta: reached 40.6 / 13.2 / 257; target 40.7 / 13.2 / 257; dE00 0.01
  * e / gamma: reached 57.6 / 42.9 / 134; target 57.6 / 42.9 / 134; dE00 0.01
* **dE00 vs data at 0.5 x / 1 x / 2 x path:** max 0.01 / max 0.01 / max 0.01. Transmittance rms at 1 x path: o 0.005, beta 0.002, e 0.020.
* **Pleochroism.** alpha L* 36 C* 40 h 330 (red-violet), beta L* 41 C* 13 h 257 (blue-grey), gamma L* 58 C* 43 h 134 (yellow-green): Hurlbut's X red / Y blue / Z yellow-green.

Bands (per mm):

o / alpha:

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 363.30 | 5960.4 | 14036 | 0.1768 |
| 380.38 | 1088.2 | 2563 | 0.1347 |
| 454.53 | 856.0 | 2016 | 0.04076 |
| 538.15 | 535.2 | 1260 | 0.01471 |
| 539.89 | 1715.1 | 4039 | 0.2107 |
| 597.56 | 771.9 | 1818 | 0.1002 |

beta:

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 379.09 | 259.4 | 611 | 0.143 |
| 385.76 | 905.3 | 2132 | 0.3826 |
| 421.44 | 3451.8 | 8128 | 0.1958 |
| 533.35 | 1090.0 | 2567 | 0.1333 |
| 607.31 | 963.1 | 2268 | 0.2143 |
| 762.39 | 1360.2 | 3203 | 0.2443 |

gamma:

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 365.88 | 963.6 | 2269 | 34.64 |
| 411.97 | 509.7 | 1200 | 0.222 |
| 428.57 | 729.3 | 1717 | 0.1313 |
| 452.21 | 925.7 | 2180 | 0.2272 |
| 539.23 | 1982.9 | 4669 | 0.116 |
| 610.10 | 838.0 | 1973 | 0.1093 |
| 783.04 | 1646.2 | 3877 | 0.1428 |


### F3-21 Tanzanite: Green Zoisite

Key `tanzanite.green_zoisite`, estimated: **yes**.

* **Sources.** Caltech Mineral Spectroscopy Server (G. R. Rossman, CC BY-NC 4.0, DOI 10.7907/jywr-qq57): CIT-16940 zoisite, Alchuri, Shigar Valley, Pakistan, alpha 6.75 mm / beta 5.585 mm / gamma 6.275 mm, http://minerals.gps.caltech.edu/FILES/Visible/zoisite/Zoisite16940_a.txt, _b.txt, _c.txt, crates/pleochroma/data/reference_spectra/variant_zoisite_green_alpha.csv, _beta.csv, _gamma.csv; docs/color_variants/R3_garnet_peridot_zircon_benitoite_tanzanite.md, section 6.2 (V3+ with Fe-Ti IVCT or Cr3+; Caltech zoisite page: green varieties contain Fe3+)
* **Cause.** A mix of V3+ and the Fe-Ti IVCT (yellow plus blue-green) or Cr3+; the Pakistani chemistry is unpublished.
* **Spectrum and baseline.** CIT-16940, three different faces and thicknesses, baselines A = 0.104 / 0.075 / 0.036 removed. The file the index lists as E||c = beta carries 'E\a' in its own header (unresolved, may be a typo). Marked estimated for the single sample, the mismatched faces and the ray-label conflict.
* **Scaling.** k = 1.5 of the sample per mm (k = 1 L* 67, k = 3 L* 33).
* **Target.** Yellowish-green grey to olive green, face-up L* 56 C* 25 h 117.
* **Colors (D65, 9.905 mm), reached by the stored bands versus the target (data at k, or the scaled default):**
  * face-up: reached 55.9 / 24.9 / 117; target 55.9 / 24.9 / 117; dE00 0.00
  * o / alpha: reached 60.8 / 46.2 / 91; target 60.8 / 46.2 / 91; dE00 0.00
  * beta: reached 27.6 / 14.5 / 133; target 27.6 / 14.5 / 133; dE00 0.01
  * e / gamma: reached 67.3 / 25.9 / 151; target 67.3 / 25.9 / 151; dE00 0.00
* **dE00 vs data at 0.5 x / 1 x / 2 x path:** max 0.01 / max 0.01 / max 0.01. Transmittance rms at 1 x path: o 0.016, beta 0.016, e 0.053.
* **Pleochroism.** alpha L* 61 C* 46 h 91 (yellow), beta L* 28 C* 15 h 133 (dark olive), gamma L* 67 C* 26 h 151 (pale green): pleochroic but 'not as spectacular as tanzanite' (Caltech). Approximate.

Bands (per mm):

o / alpha:

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 390.18 | 710.6 | 1673 | 0.7437 |
| 425.74 | 120.5 | 284 | 0.01793 |
| 451.56 | 224.1 | 528 | 0.1417 |
| 451.70 | 1656.4 | 3901 | 0.2479 |
| 569.05 | 1454.2 | 3424 | 0.05199 |
| 623.44 | 4811.9 | 11331 | 0.06723 |

beta:

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 362.81 | 1460.4 | 3439 | 1.347 |
| 426.73 | 142.3 | 335 | 0.1735 |
| 448.80 | 3329.1 | 7839 | 0.1578 |
| 452.72 | 168.0 | 396 | 0.1401 |
| 625.49 | 9000.0 | 21193 | 0.2037 |
| 647.26 | 1174.4 | 2766 | 0.1232 |

gamma:

| center nm | sigma cm^-1 | FWHM cm^-1 | peak per mm |
|---|---|---|---|
| 389.09 | 826.4 | 1946 | 0.5311 |
| 406.34 | 7514.2 | 17695 | 0.1119 |
| 426.06 | 269.3 | 634 | 0.01932 |
| 450.64 | 376.5 | 887 | 0.04508 |
| 452.87 | 118.7 | 280 | 0.05478 |
| 683.66 | 1560.1 | 3674 | 0.1344 |

