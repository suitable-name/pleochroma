# Fit records: corundum, spinel, chrysoberyl, alexandrite (lane F1, 2026-10-27)

Entries of `crates/pleochroma/data/color_variants/corundum_spinel_chrysoberyl.toml` (20 variants of Sapphire, Ruby, Spinel, Chrysoberyl (Yellow) and Alexandrite; the `Yellow` variant was dropped in the 2026-10-27 fix round), pinned by `crates/pleochroma/src/optics/materials/variants/tests_corundum_spinel_chrysoberyl.rs`. Research: `docs/color_variants/R1_corundum_spinel_chrysoberyl.md`. The two estimated seed entries `sapphire.pink` / `sapphire.yellow` of `data/color_variants.toml` are replaced by the measured Pink and Yellow here (same keys). Dropped by owner decision: Spinel Grey, Chrysoberyl Honey.

## Path, units and the color pipeline

* **Path used.** Every entry is `per_mm` at `reference_path_mm = 7.0` (the stone width). The colors are `color::body_color::body_colors` at the face-up path of a 7 mm stone, `variants::face_up_swatch_path(PerMm, 7.0)` = `MODEL_UNIT_FACE_UP_PATH x 7 / REFERENCE_MODEL_WIDTH` = 2.83 x 7 / 2.0 = **9.905 mm** (the rule of `render_setup::absorption_path_scale_for` and the zone swatches, valid since the 2026-10-27 re-pin from 2.52). A Material Default's model-unit peak sum is the face-up optical depth (swatch = 1.0 unit at 7 mm), so a stored per-mm peak is that optical depth divided by 9.905.
* **Convention of the strengths.** Face-up optical depth P(lambda). Corundum: P = sigma(lambda) x 1.178e17 x D, D in ppma*cm (areal density over the face-up path: D / 0.9905 cm is the bulk ppma; 1 ppma = 1.178e17 cm^-3, GIA Box A) summed over chromophores. Caltech spectra: P = 2.303 x (A - baseline) / path_cm x 0.88 cm x factor (the research doc's 0.88 cm convention, so its s factors and its target colors carry over; the physical coefficient behind a stored number is P / 0.9905 cm).
* **Pipeline check.** My color arithmetic (CIE 1931 5 nm table and the D65 10 nm table parsed from the repo source, 1 nm integration 380-780, Lab against the illuminant's own white, Planckian 3200 K from `blackbody_spectrum`) reproduces the two recorded built-in swatches at one model unit: Sapphire default 56.7 / 39.4 / 283 against the recorded L* 57, C* 40, h 283, and Ruby default 40.5 / 67.7 / 356 against L* 41, C* 67, h 356 (`docs/chromophores/calibration-records.md`). dE2000 of the CIEDE2000 implementation is checked against the app's by construction (same formula, Sharma 2005).
* **The Fe2+-Ti4+ factor 2.4.** The research doc found GIA's Appendix 2 Fe2+-Ti4+ color circles about 2.4 times darker than the supplementary cross-section sheet gives. GIA's text says natural non-Yogo high-iron stones show Fe2+-Ti4+ cross sections two to three times larger than the synthetic / Yogo value of the sheet (1.94e-18 cm^2 +/- 25 % at 580 nm), either enhanced pair interaction or surface-analysis bias (zoning). Decision: use the sheet's cross sections, the ones behind the built-in Sapphire (they reproduce its recorded swatch above), and set every Fe2+-Ti4+ strength relative to the Sapphire default's own density (10 ppma*cm = Material Default): Cornflower 6, Deep Blue 15, and the small Fe2+-Ti4+ parts of Purple (3) and Green (7). The factor therefore moves only the ppma label, not a color: a natural stone with the larger cross section carries 2.4 times fewer pairs for the same color.
* **One strength factor per spectrum (owner rule).** No band is scaled on its own to reach a color. Single-source variants scale one measured spectrum (all rays) by one factor: Spinel Pink, Cobalt Blue, Lavender (path), Chrysoberyl Yellow and Greenish Yellow, Ruby Thai (all measured o-ray). Corundum variants are sums of GIA chromophore cross sections at stated ppma*cm, which is how GIA defines the variety. The combinations of measured spectra, each with one factor, are Spinel Purple (JQ-13 Cr3+ + GRR 3017 Co2+) and Alexandrite (GRR 874 Cr3+ + GRR 171 Fe3+). The only per-band ratios are those that build the unmeasured alexandrite alpha ray (see Alexandrite Strong Change) and the Ruby Thai e-ray excess; both describe an unmeasured ray and are marked estimated.
* **Fitting.** For every entry the face-up optical depth curve P(lambda) of each ray is built from the sources above and fitted with at most 8 energy Gaussians per ray (`width_nm` = FWHM / 2.35482 in cm^-1), the smallest band count that gives dE2000 of at most 0.8 on every ray, face-up color and illuminant (D65 and 3200 K) and a transmittance residual of at most 2.5 % RMS, with the physical band count of the chromophores as the minimum. All rays use the stored (rounded) values for the reported colors. Scripts and intermediate files are outside the repo.
* **Baselines.** Caltech files carry reflection loss and an offset (not removed by the source); the constant mean absorbance at 740-780 nm (where the chromophores do not absorb) is removed for every Caltech spectrum and the result clipped at 0. GIA cross sections are already reflection corrected.
* **Reference spectra not shipped.** The CSVs stay in `crates/pleochroma/data/reference_spectra/` as sources; the test pins the numbers of this record and reads no CSV.

## Update 2026-10-27 (fix round X6)

Applies to every entry below; the per-entry sections were corrected where the data changed.

* **Estimated flag (owner decision O3, 2026-10-27).** `estimated = true` when the spectral shape is not a measured spectrum of that variety, OR the strength is not anchored to a measurement (a chosen strength counts: the Cornflower / Deep Blue / Pigeon's Blood convention of lane F1 now holds in every group); `estimated = false` only when the shape is a measured spectrum of that very variety at a strength that is itself measured or published. The first `sources` line of every entry is ONE short plain sentence that names the estimated part; the UI shows it as the tooltip "Estimated: <first line>" (no tooltip for a measured entry). Citations, URLs, DOIs and repository paths start on the second line.
* **Face-up color (decision D-G).** The swatches and pins of the Variants list show the ORIENTED face-up color, the color the renderer shows for the stone: with the c axis along the view the o-ray alone (uniaxial hosts with `c_axis` Y: Sapphire, Ruby, Zircon, Benitoite, Emerald, Morganite, Amethyst, Quartz, Rutile, Moissanite), (o + e) / 2 for the hosts whose c axis lies in the table plane (Aquamarine and Tourmaline, `c_axis` X), (alpha + beta) / 2 for biaxial hosts (gamma along `c_axis` Y: Topaz, Chrysoberyl, Alexandrite, Tanzanite, Peridot, Andalusite); isotropic hosts show the one color. The entry sections below keep the unpolarised mean (the fit target, what `body_colors` returns) in their "reached" lines and list the oriented color as "reached face-up (oriented ...)"; the tests pin both. The face-up path is 9.905 mm (`reference_path_mm` = the 7 mm stone width, decision D-A).
* **Colorless entries (decision D-C).** An entry with no bands is valid on any host; the `pleochroic = true` trick was removed from the colorless entries.
* **Measured entries in this file** (`estimated = false`): `sapphire.pink`, `sapphire.yellow`, `ruby.pinkish_red`, `spinel.lavender`. Every other one of the 21 entries is estimated.
* **Chrysoberyl (owner decision O1).** The Material Default of Chrysoberyl (Yellow) is now 0.7 x the B1 fit of GRR 171 (one factor, per mm; the F1 "0.7" is the 0.88 cm research convention, 0.622 per mm). The `Yellow` variant duplicated it and is DROPPED; `Greenish Yellow` (0.3 x) stays; `Honey` stays dropped (owner decision: no usable data).
* **F-14 Ruby Pigeon's Blood.** Documented, not changed: it reproduces GIA's Cr3+ cross sections at 1000 ppma*cm; the Ruby default is 800.
* **F-13 (core F13) Chrysoberyl axis setting.** The Caltech GRR 171 headers give the cell as Pnma (a 9.40, b 5.48, c 4.43 A) and the polarizations "E||a = beta", "E||b = gamma"; GRR 874 states the beta ray vibrates along the 9.40 A axis. Walling's sort of Alexandrite uses the same Pnma setting, so beta = a and gamma = b hold. REFUTED; the comment in `aquamarine_through_citrine.rs` says so. The 3.5 x gamma / beta split contradicts Farrell & Newnham at 77 K and is kept as measured.

**Tooltip sentences and oriented face-up colors** (D65, 9.905 mm):

| key | host | estimated | tooltip sentence | face-up (oriented) L* / C* / h | unpolarised mean L* / C* / h |
|---|---|---|---|---|---|
| `sapphire.pink` | Sapphire | no | - | 69.0 / 45.8 / 328 | 75.8 / 30.7 / 336 |
| `sapphire.yellow` | Sapphire | no | - | 86.8 / 62.0 / 85 | 88.5 / 57.7 / 87 |
| `sapphire.padparadscha` | Sapphire | yes | - | 73.5 / 35.6 / 31 | 78.4 / 31.6 / 42 |
| `sapphire.orange` | Sapphire | yes | - | 81.1 / 76.0 / 73 | 82.7 / 67.0 / 74 |
| `sapphire.cornflower` | Sapphire | yes | - | 63.7 / 41.0 / 286 | 70.3 / 28.6 / 279 |
| `sapphire.deep_blue` | Sapphire | yes | - | 32.3 / 74.3 / 299 | 43.9 / 47.3 / 287 |
| `sapphire.purple` | Sapphire | yes | - | 58.8 / 46.2 / 310 | 67.1 / 28.1 / 312 |
| `sapphire.green` | Sapphire | yes | - | 46.5 / 20.6 / 131 | 56.1 / 32.4 / 120 |
| `sapphire.colorless` | Sapphire | yes | - | 100.0 / 0.0 / 0 | 100.0 / 0.0 / 0 |
| `ruby.pinkish_red` | Ruby | no | - | 46.2 / 72.2 / 334 | 55.5 / 54.8 / 343 |
| `ruby.thai` | Ruby | yes | - | 29.7 / 59.8 / 4 | 34.6 / 59.8 / 9 |
| `ruby.pigeons_blood` | Ruby | yes | - | 27.3 / 65.2 / 351 | 34.5 / 64.7 / 1 |
| `spinel.pink` | Spinel | yes | Strength chosen for a typical face-up color; spectrum measured (Caltech JQ-13). | 74.3 / 33.2 / 4 | 74.3 / 33.2 / 4 |
| `spinel.cobalt_blue` | Spinel | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 3017). | 55.4 / 64.8 / 268 | 55.4 / 64.8 / 268 |
| `spinel.lavender` | Spinel | no | - | 71.7 / 6.2 / 307 | 71.7 / 6.2 / 307 |
| `spinel.purple` | Spinel | yes | - | 61.7 / 29.3 / 303 | 61.7 / 29.3 / 303 |
| `chrysoberyl__yellow_.greenish_yellow` | Chrysoberyl (Yellow) | yes | Strength chosen for a typical face-up color; beta and gamma rays measured (GRR 171), alpha ray estimated. | 90.6 / 31.6 / 100 | 89.5 / 37.3 / 98 |
| `alexandrite.strong_change` | Alexandrite | yes | Beta and gamma rays measured (GRR 874) with Fe3+ from GRR 171; alpha ray and strengths estimated. | 57.4 / 23.1 / 78 | 53.9 / 16.8 / 132 |
| `alexandrite.brazilian` | Alexandrite | yes | Beta and gamma rays measured (GRR 874) with Fe3+ from GRR 171; alpha ray and strengths estimated. | 54.5 / 27.0 / 74 | 50.5 / 18.7 / 111 |
| `alexandrite.sri_lankan` | Alexandrite | yes | Beta and gamma rays measured (GRR 874) with Fe3+ from GRR 171; alpha ray and strengths estimated. | 69.0 / 34.0 / 90 | 64.4 / 30.6 / 110 |


## Entries

### Sapphire Pink

Key `sapphire.pink`; `estimated = false`; 4 bands per ray (o_ray / e_ray).

**Measured, chosen, why.** Measured: the GIA Cr3+ cross sections (E perp c and E par c). Chosen: the areal density 100 ppma*cm (one factor for both rays). Why: it is the density of GIA's published Appendix 2 color circle (calculated 'both' color L*a*b* 78.05 / 25.96 / -9.92, i.e. L*C*h 78.1 / 27.8 / 339), a light-to-medium pink that leaves room between Pinkish Red (300) and the Ruby default (800).

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 75.8 / 30.7 / 337 | 75.8 / 30.7 / 336 | 0.04 | 77.3 / 35.2 / 341 | 77.3 / 35.2 / 341 | 0.04 |
| o ray / alpha | 69.0 / 45.7 / 328 | 69.0 / 45.8 / 328 | 0.06 | 70.7 / 48.7 / 332 | 70.7 / 48.8 / 332 | 0.06 |
| e ray / gamma | 86.9 / 15.2 / 31 | 86.9 / 15.1 / 31 | 0.04 | 88.3 / 20.5 / 21 | 88.3 / 20.5 / 21 | 0.03 |

**Fit.** Energy Gaussians, bounded, 4 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 1.6 / 9.6; e / gamma: 0.9 / 3.1. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 69.0 / 45.8 / 328; e / gamma: 86.9 / 15.1 / 31. Documented (GIA): Cr3+ strongly dichroic, E perp c the stronger absorber. Reached: o purplish pink, e lighter orange-pink; the o ray is darker.


### Sapphire Yellow

Key `sapphire.yellow`; `estimated = false`; 2 bands per ray (o_ray / e_ray).

**Measured, chosen, why.** Measured: the GIA h.-Fe3+ pair cross sections (both rays, quite similar). Chosen: 1.7 ppma*cm (one factor). Why: GIA's published Appendix 2 color at 1.7 ppma*cm (calculated 'both' L*a*b* 88.42 / 1.58 / 60.78, L*C*h 88.4 / 60.8 / 88.5) is a strong yellow at high lightness; the entry sits on that anchor.

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 88.6 / 58.1 / 87 | 88.5 / 57.7 / 87 | 0.44 | 90.9 / 60.0 / 78 | 90.9 / 59.7 / 78 | 0.31 |
| o ray / alpha | 86.9 / 62.4 / 85 | 86.8 / 62.0 / 85 | 0.45 | 89.6 / 65.1 / 77 | 89.5 / 64.7 / 77 | 0.28 |
| e ray / gamma | 91.8 / 51.4 / 92 | 91.7 / 51.0 / 91 | 0.41 | 93.6 / 51.8 / 81 | 93.5 / 51.5 / 81 | 0.36 |

**Fit.** Energy Gaussians, bounded, 2 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 1.6 / 4.6; e / gamma: 0.9 / 7.3. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 86.8 / 62.0 / 85; e / gamma: 91.7 / 51.0 / 91. Documented (GIA): h.-Fe3+ 'Ec and E||c quite similar'. Reached: nearly equal, o slightly darker.


### Sapphire Padparadscha

Key `sapphire.padparadscha`; `estimated = true`; 6 bands per ray (o_ray / e_ray).

**Measured, chosen, why.** Measured: GIA Cr3+ and h.-Cr3+ pair cross sections. Chosen: Cr3+ 50 and h.-Cr3+ 0.6 ppma*cm. Why: the trade definition (LMHC 2007, via JoG 2018, secondary) is a low-to-medium saturation, light-tone pinkish orange to orangey pink; the pink part is the Cr3+ and the orange part the hole pair, and 0.6 ppma*cm of h.-Cr3+ is as strong as 57 ppma of Cr3+. No spectrum of a measured padparadscha with chemistry exists, so the combination is an estimate.

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 78.4 / 31.6 / 42 | 78.4 / 31.6 / 42 | 0.01 | 81.2 / 38.6 / 40 | 81.2 / 38.6 / 40 | 0.01 |
| o ray / alpha | 73.5 / 35.6 / 31 | 73.5 / 35.6 / 31 | 0.01 | 76.8 / 43.2 / 32 | 76.8 / 43.2 / 32 | 0.01 |
| e ray / gamma | 86.9 / 28.7 / 64 | 86.9 / 28.7 / 64 | 0.01 | 89.0 / 33.2 / 56 | 89.0 / 33.2 / 56 | 0.01 |

**Fit.** Energy Gaussians, bounded, 6 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 0.5 / 3.5; e / gamma: 0.2 / 1.1. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 73.5 / 35.6 / 31; e / gamma: 86.9 / 28.7 / 64. Documented: little dichroism. Reached: o pinkish orange, e yellower and lighter.


### Sapphire Orange

Key `sapphire.orange`; `estimated = true`; 5 bands per ray (o_ray / e_ray).

**Measured, chosen, why.** Measured: GIA h.-Cr3+ and h.-Fe3+ pair cross sections. Chosen: h.-Cr3+ 1.2 and h.-Fe3+ 0.5 ppma*cm. Why: GIA says pure orange needs the h.-Cr3+ pair with some Fe; the amounts give a strong orange with the pair-spectrum shape and no stone is measured, so the amounts are an estimate.

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 82.7 / 67.0 / 74 | 82.7 / 67.0 / 74 | 0.01 | 86.5 / 73.4 / 70 | 86.5 / 73.4 / 70 | 0.01 |
| o ray / alpha | 81.1 / 76.0 / 73 | 81.1 / 76.0 / 73 | 0.01 | 85.3 / 82.9 / 70 | 85.3 / 82.9 / 70 | 0.01 |
| e ray / gamma | 85.8 / 54.0 / 77 | 85.8 / 54.0 / 77 | 0.01 | 88.8 / 59.2 / 71 | 88.8 / 59.1 / 71 | 0.01 |

**Fit.** Energy Gaussians, bounded, 5 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 0.3 / 1.0; e / gamma: 0.4 / 2.2. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 81.1 / 76.0 / 73; e / gamma: 85.8 / 54.0 / 77. Documented (GIA): h.-Cr3+ 'little dichroism'. Reached: o and e both orange, o stronger.


### Sapphire Cornflower

Key `sapphire.cornflower`; `estimated = true`; 4 bands per ray (o_ray / e_ray).

**Measured, chosen, why.** Measured: the GIA Fe2+-Ti4+ pair cross sections. Chosen: 6 ppma*cm, 0.6 of the Sapphire default density (one factor). Why: a medium-light violetish blue, lighter than the default. A Kashmir 'velvet' look is scattering by silk, not absorption (GIA W2019 Kashmir chemistry is ordinary metamorphic blue), so this is one Cornflower entry, not a Kashmir one. No published color target exists.

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 70.3 / 28.6 / 279 | 70.3 / 28.6 / 279 | 0.00 | 68.9 / 29.8 / 263 | 68.9 / 29.8 / 263 | 0.00 |
| o ray / alpha | 63.7 / 41.0 / 286 | 63.7 / 41.0 / 286 | 0.00 | 61.9 / 41.0 / 270 | 61.9 / 41.0 / 270 | 0.00 |
| e ray / gamma | 81.2 / 11.3 / 244 | 81.2 / 11.3 / 244 | 0.00 | 80.2 / 14.3 / 233 | 80.2 / 14.3 / 233 | 0.00 |

**Fit.** Energy Gaussians, bounded, 4 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 0.1 / 0.9; e / gamma: 0.5 / 2.2. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 63.7 / 41.0 / 286; e / gamma: 81.2 / 11.3 / 244. Documented (GIA): violetish blue along, greener and grayer across. Reached: o 286 deg hue (violetish), e 244 deg (greenish).


### Sapphire Deep Blue

Key `sapphire.deep_blue`; `estimated = true`; 3 bands per ray (o_ray / e_ray).

**Measured, chosen, why.** Measured: the GIA Fe2+-Ti4+ pair cross sections. Chosen: 15 ppma*cm, 1.5 of the Sapphire default density. Why: a royal-type deep blue darker than the default. No published color target exists.

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 43.9 / 47.3 / 287 | 43.9 / 47.3 / 287 | 0.01 | 41.5 / 47.5 / 265 | 41.5 / 47.5 / 265 | 0.01 |
| o ray / alpha | 32.3 / 74.2 / 299 | 32.3 / 74.3 / 299 | 0.03 | 28.8 / 69.0 / 277 | 28.8 / 69.0 / 277 | 0.03 |
| e ray / gamma | 58.9 / 21.0 / 250 | 58.9 / 21.0 / 250 | 0.00 | 57.1 / 26.7 / 235 | 57.1 / 26.7 / 235 | 0.01 |

**Fit.** Energy Gaussians, bounded, 3 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 0.8 / 4.0; e / gamma: 2.0 / 8.4. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 32.3 / 74.3 / 299; e / gamma: 58.9 / 21.0 / 250. As Cornflower: o 299 deg, e 250 deg.


### Sapphire Purple

Key `sapphire.purple`; `estimated = true`; 6 bands per ray (o_ray / e_ray).

**Measured, chosen, why.** Measured: GIA Cr3+ and Fe2+-Ti4+ cross sections. Chosen: Cr3+ 80 and Fe2+-Ti4+ 3 ppma*cm. Why: GIA names Cr3+ with Fe2+-Ti4+ as the cause. Caltech GRR 3614 (violet sapphire, both rays) is consistent in kind (o-ray: the Cr3+ 563 nm U band with the violet 450 nm band; e-ray only the weak Cr3+ 551 nm band, bulk Cr of 70 to 90 ppma from the U band against 81 ppma for the recipe) but its baseline is over-corrected (minimum A -0.245), so it is a qualitative check only and is not fitted.

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 67.1 / 28.1 / 312 | 67.1 / 28.1 / 312 | 0.01 | 67.4 / 29.6 / 313 | 67.4 / 29.6 / 313 | 0.01 |
| o ray / alpha | 58.8 / 46.2 / 310 | 58.8 / 46.2 / 310 | 0.01 | 58.8 / 46.3 / 309 | 58.8 / 46.3 / 309 | 0.01 |
| e ray / gamma | 80.1 / 5.7 / 19 | 80.1 / 5.7 / 19 | 0.04 | 80.6 / 9.3 / 3 | 80.6 / 9.3 / 3 | 0.02 |

**Fit.** Energy Gaussians, bounded, 6 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 0.7 / 5.9; e / gamma: 2.2 / 9.7. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 58.8 / 46.2 / 310; e / gamma: 80.1 / 5.7 / 19. Documented pattern (GRR 3614): purple along c-perpendicular, pale pinkish across. Reached: o purple (310 deg), e nearly neutral pink.


### Sapphire Green

Key `sapphire.green`; `estimated = true`; 6 bands per ray (o_ray / e_ray).

**Measured, chosen, why.** Measured: GIA Fe2+-Ti4+ and Fe3+ cross sections. Chosen: Fe2+-Ti4+ 7 and Fe3+ 1800 ppma*cm. Why: basalt-hosted green sapphire is blue plus yellow chromophores; the amounts are typical basalt-type values (the research doc's recipe), not one analysed stone. Fe3+'s 450 nm cross section is not constant at high iron (Emmett et al. 2023), a further uncertainty.

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 56.1 / 32.2 / 120 | 56.1 / 32.4 / 120 | 0.05 | 55.9 / 24.6 / 105 | 55.9 / 24.7 / 105 | 0.04 |
| o ray / alpha | 46.5 / 20.5 / 131 | 46.5 / 20.6 / 131 | 0.06 | 46.0 / 12.0 / 109 | 46.0 / 12.1 / 108 | 0.06 |
| e ray / gamma | 70.0 / 48.8 / 114 | 70.0 / 48.9 / 114 | 0.04 | 70.1 / 41.0 / 103 | 70.1 / 41.1 / 103 | 0.03 |

**Fit.** Energy Gaussians, bounded, 6 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 0.3 / 1.3; e / gamma: 0.3 / 1.6. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 46.5 / 20.6 / 131; e / gamma: 70.0 / 48.9 / 114. Documented (Caltech): bluish green one way, yellow-green the other. Reached: o bluish green (131 deg), e yellow-green (114 deg, stronger chroma).


### Sapphire Colorless

Key `sapphire.colorless`; `estimated = true`; 0 bands per ray (o_ray / e_ray).

**Measured, chosen, why.** Nothing measured or fitted: both band sets are empty (a colorless variant of a pleochroic host, stored pleochroic). Trace Fe3+ would give C* of about 2 at 7 mm and is not modelled.

**Target and reached.** White (L* 100, C* 0) in every direction under every illuminant.

**Per direction (reached, D65).** o / alpha: 100.0 / 0.0 / 0; e / gamma: 100.0 / 0.0 / 0. None: both rays white.


### Ruby Pinkish Red

Key `ruby.pinkish_red`; `estimated = false`; 8 bands per ray (o_ray / e_ray).

**Measured, chosen, why.** Measured: the GIA Cr3+ cross sections. Chosen: 300 ppma*cm (one factor). Why: between the Pink and the Ruby default; GIA's Cr3+ color circles at 200 and 375 ppma*cm (calculated 'both' L*a*b* 64.34 / 44.64 / -16.54 and 50.00 / 62.74 / -18.87) bracket the reached lightness and chroma.

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 55.5 / 54.7 / 343 | 55.5 / 54.8 / 343 | 0.01 | 59.2 / 59.2 / 348 | 59.2 / 59.2 / 348 | 0.01 |
| o ray / alpha | 46.2 / 72.2 / 334 | 46.2 / 72.2 / 334 | 0.01 | 50.1 / 73.7 / 338 | 50.1 / 73.7 / 338 | 0.01 |
| e ray / gamma | 69.0 / 39.9 / 10 | 69.0 / 39.9 / 10 | 0.06 | 72.6 / 47.4 / 11 | 72.6 / 47.4 / 11 | 0.06 |

**Fit.** Energy Gaussians, bounded, 8 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 1.8 / 19.0; e / gamma: 1.0 / 4.7. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 46.2 / 72.2 / 334; e / gamma: 69.0 / 39.9 / 10. Documented (GIA): o purplish red, e orange-red. Reached: o 334 deg, e 10 deg.


### Ruby Thai

Key `ruby.thai`; `estimated = true`; 8 bands per ray (o_ray / e_ray).

**Measured, chosen, why.** Measured: the E perp c spectrum of one Thai ruby (Caltech GRR 1843, Chanthaburi, 0.758 mm, 27 C), with ONE factor of 4.76 over the whole spectrum (face-up Cr3+ density 800 ppma*cm, the Ruby default's own density, over the sample's 168 ppma*cm). Baseline: the constant mean absorbance at 740-780 nm (0.069) removed; the Cr3+ R lines (684-702 nm) interpolated. Not measured: the E par c ray (no Thai ruby has one); it is the GIA Cr3+ E par c cross section at 800 ppma*cm plus the o-ray's excess over the GIA Cr3+ o-ray (the extra window absorption of this stone, from 400 nm on), assuming the excess centers (h.-Cr3+, Fe3+) are nearly non-dichroic as GIA states. This replaces an earlier recipe (Cr3+ 800 + h.-Cr3+ 0.3 + Fe3+ 960 ppma*cm, 34.4 / 66.2 / 22 face-up) whose window absorption was 3.7 times the measured excess (window ratio A(474)/A(562): measured 0.094, Cr3+ alone 0.075, recipe 0.146).

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 34.6 / 59.7 / 9 | 34.6 / 59.8 / 9 | 0.04 | 40.4 / 68.5 / 13 | 40.4 / 68.6 / 13 | 0.03 |
| o ray / alpha | 29.7 / 59.6 / 4 | 29.7 / 59.8 / 4 | 0.04 | 35.6 / 67.8 / 9 | 35.6 / 67.8 / 9 | 0.03 |
| e ray / gamma | 42.2 / 62.4 / 17 | 42.2 / 62.3 / 17 | 0.06 | 48.1 / 71.7 / 18 | 48.1 / 71.7 / 18 | 0.06 |

**Fit.** Energy Gaussians, bounded, 8 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 1.2 / 5.0; e / gamma: 1.9 / 8.2. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 29.7 / 59.8 / 4; e / gamma: 42.2 / 62.3 / 17. Documented (GIA): o purplish red, e orange-red. Reached: o 4 deg, e 17 deg (both red, the iron and hole centers orange the stone).


### Ruby Pigeon's Blood

Key `ruby.pigeons_blood`; `estimated = true`; 6 bands per ray (o_ray / e_ray).

**Measured, chosen, why.** Measured: the GIA Cr3+ cross sections. Chosen: 1000 ppma*cm, 1.25 of the Ruby default density (estimated, owner decision 2026-10-27). Why: Mogok marble-hosted ruby is Cr only (Fe mean 35 ppma, GIA W2019) and the trade look is a vivid, saturated pure red; GIA's Cr3+ colors at 750 and 1500 ppma*cm bracket it. It differs from the Ruby default only by the density, so it adds little.

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 34.5 / 64.8 / 1 | 34.5 / 64.7 / 1 | 0.03 | 40.6 / 68.4 / 7 | 40.6 / 68.4 / 7 | 0.03 |
| o ray / alpha | 27.3 / 65.4 / 351 | 27.3 / 65.2 / 351 | 0.06 | 33.2 / 67.2 / 358 | 33.2 / 67.1 / 358 | 0.07 |
| e ray / gamma | 44.7 / 70.0 / 13 | 44.7 / 69.9 / 13 | 0.05 | 51.4 / 75.3 / 18 | 51.4 / 75.3 / 18 | 0.04 |

**Fit.** Energy Gaussians, bounded, 6 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 3.9 / 41.6; e / gamma: 1.2 / 6.4. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 27.3 / 65.2 / 351; e / gamma: 44.7 / 69.9 / 13. Documented (GIA): o purplish red, e orange-red. Reached: o 351 deg, e 13 deg.


### Spinel Pink

Key `spinel.pink`; `estimated = true`; 4 bands per ray (one set, cubic).

**Measured, chosen, why.** Measured: the Caltech JQ-13 red spinel spectrum (unpolarised, cubic, 1.52 mm, 'corrected for baseline' by the contributor; constant 0.0499 = mean A at 740-780 nm removed). Chosen: ONE factor of 0.132 over the whole spectrum, i.e. a Napierian alpha of 1.5 cm^-1 at 537 nm in the research doc's 0.88 cm convention (the face-up optical depth is 1.32 at 537 nm). Why: a pale to light pink, a lower density of the Red stone, not a different chromophore (the research doc's pink band of 1.5 to 1.0 cm^-1).

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 74.3 / 33.2 / 4 | 74.3 / 33.2 / 4 | 0.01 | 77.3 / 36.5 / 10 | 77.2 / 36.5 / 10 | 0.01 |
| o ray / alpha | 74.3 / 33.2 / 4 | 74.3 / 33.2 / 4 | 0.01 | 77.3 / 36.5 / 10 | 77.2 / 36.5 / 10 | 0.01 |
| e ray / gamma | 74.3 / 33.2 / 4 | 74.3 / 33.2 / 4 | 0.01 | 77.3 / 36.5 / 10 | 77.2 / 36.5 / 10 | 0.01 |

**Fit.** Energy Gaussians, bounded, 4 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 1.2 / 8.7. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 74.3 / 33.2 / 4; e / gamma: 74.3 / 33.2 / 4. Cubic: one color for every direction.


### Spinel Cobalt Blue

Key `spinel.cobalt_blue`; `estimated = true`; 6 bands per ray (one set, cubic).

**Measured, chosen, why.** Measured: Caltech GRR 3017 cobalt-blue spinel (Vietnam, 1.435 mm; constant 0.0846 removed). ONE factor of 0.31 over the whole spectrum: alpha(594 nm) 4.5 cm^-1 in the 0.88 cm convention instead of the stone's own 14.7. Why: the stone itself at the face-up path would be near black (36 / 75 / 286); 0.31 gives a vivid medium-dark blue. The stone's chemistry is unknown, so no ppma is stated. GRR 2381 (Baffin Island) is a second stone, not fitted (594 nm alpha 22.1 cm^-1).

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 55.4 / 64.8 / 268 | 55.4 / 64.8 / 268 | 0.01 | 49.1 / 77.4 / 257 | 49.1 / 77.3 / 257 | 0.02 |
| o ray / alpha | 55.4 / 64.8 / 268 | 55.4 / 64.8 / 268 | 0.01 | 49.1 / 77.4 / 257 | 49.1 / 77.3 / 257 | 0.02 |
| e ray / gamma | 55.4 / 64.8 / 268 | 55.4 / 64.8 / 268 | 0.01 | 49.1 / 77.4 / 257 | 49.1 / 77.3 / 257 | 0.02 |

**Fit.** Energy Gaussians, bounded, 6 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 1.1 / 7.1. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 55.4 / 64.8 / 268; e / gamma: 55.4 / 64.8 / 268. Cubic: one color for every direction.


### Spinel Lavender

Key `spinel.lavender`; `estimated = false`; 4 bands per ray (one set, cubic).

**Measured, chosen, why.** Measured: Caltech JQ-7 (0.711 mm, FeO 0.731 wt%, Fe3+ 0.132 wt%, Cr2O3 0.050 wt%; constant 0.1306 removed), ONE factor over the whole spectrum: a path of 1.0 cm (14.1 times the measured thickness). The result stays a pale grey-lavender (C* about 6) however thick it is made, because Fe2+ in tetrahedral spinel is a weak chromophore: this is the honest 'Lavender' of an Fe-only spinel, not the saturated lilac of Cr or Co bearing stones (see Spinel Purple).

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 71.7 / 6.0 / 307 | 71.7 / 6.2 / 307 | 0.13 | 71.7 / 7.6 / 315 | 71.7 / 7.7 / 315 | 0.08 |
| o ray / alpha | 71.7 / 6.0 / 307 | 71.7 / 6.2 / 307 | 0.13 | 71.7 / 7.6 / 315 | 71.7 / 7.7 / 315 | 0.08 |
| e ray / gamma | 71.7 / 6.0 / 307 | 71.7 / 6.2 / 307 | 0.13 | 71.7 / 7.6 / 315 | 71.7 / 7.7 / 315 | 0.08 |

**Fit.** Energy Gaussians, bounded, 4 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 1.7 / 7.0. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 71.7 / 6.2 / 307; e / gamma: 71.7 / 6.2 / 307. Cubic: one color for every direction.


### Spinel Purple

Key `spinel.purple`; `estimated = true`; 6 bands per ray (one set, cubic).

**Measured, chosen, why.** Measured: the JQ-13 Cr3+ spectrum and the GRR 3017 Co2+ spectrum, each with ONE factor over its whole spectrum. Chosen: the two factors, equal peak heights (alpha 1.2 cm^-1 at 537 nm for the Cr3+ part and 1.2 cm^-1 at 594 nm for the Co2+ part, 0.88 cm convention), because Belley & Palke (2021) describe the best purple spinels as Cr + Co (Co molar absorptivity far above Cr). The research doc quoted 52 / 33 / 302 for this recipe with its own baselines; recomputed here with this record's baselines it is 61.7 / 29.3 / 303 (hue unchanged).

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 61.7 / 29.3 / 303 | 61.7 / 29.3 / 303 | 0.03 | 61.5 / 31.7 / 305 | 61.5 / 31.7 / 305 | 0.03 |
| o ray / alpha | 61.7 / 29.3 / 303 | 61.7 / 29.3 / 303 | 0.03 | 61.5 / 31.7 / 305 | 61.5 / 31.7 / 305 | 0.03 |
| e ray / gamma | 61.7 / 29.3 / 303 | 61.7 / 29.3 / 303 | 0.03 | 61.5 / 31.7 / 305 | 61.5 / 31.7 / 305 | 0.03 |

**Fit.** Energy Gaussians, bounded, 6 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 0.6 / 2.6. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 61.7 / 29.3 / 303; e / gamma: 61.7 / 29.3 / 303. Cubic: one color for every direction.


### Chrysoberyl Greenish Yellow

Key `chrysoberyl__yellow_.greenish_yellow`; `estimated = true`; 6 bands per ray (o_ray / e_ray / beta_ray).

**Measured, chosen, why.** Measured: Caltech GRR 171 greenish-yellow chrysoberyl (Minas Gerais, 1.193 mm), beta (E||a) and gamma (E||b) rays; constants 0.2618 and 0.1597 (mean A at 740-780 nm) removed. ONE factor 0.3 over all three rays in the 0.88 cm convention. Not measured: the alpha (E||c) ray, no such spectrum exists (confirmed absent on the Caltech server); estimated as the mean of the beta and gamma optical depths. Why 0.3: a light greenish yellow (the sample is itself labelled greenish-yellow).

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 89.5 / 37.3 / 98 | 89.5 / 37.3 / 98 | 0.00 | 90.6 / 35.5 / 84 | 90.6 / 35.5 / 84 | 0.00 |
| o ray / alpha | 89.4 / 40.5 / 99 | 89.4 / 40.5 / 99 | 0.01 | 90.6 / 38.2 / 85 | 90.6 / 38.2 / 85 | 0.01 |
| beta ray | 91.7 / 24.0 / 102 | 91.7 / 24.0 / 102 | 0.00 | 92.3 / 22.5 / 87 | 92.3 / 22.5 / 87 | 0.00 |
| e ray / gamma | 87.3 / 51.6 / 95 | 87.3 / 51.6 / 95 | 0.01 | 88.9 / 49.4 / 83 | 88.9 / 49.4 / 83 | 0.00 |

**Fit.** Energy Gaussians, bounded, 6 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 1.2 / 7.4; beta: 0.8 / 2.7; e / gamma: 0.8 / 4.8. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 89.4 / 40.5 / 99; beta: 91.7 / 24.0 / 102; e / gamma: 87.3 / 51.6 / 95. Measured beta pale yellow, gamma more saturated greenish yellow; alpha is the estimated mean.

**Oriented face-up views** (two eigenmodes, D65 / 3200 K): alpha + beta (looking along b: the app's default orientation, the gamma axis is the stone's `c_axis`): 90.6 / 31.6 / 100 / 91.5 / 29.8 / 86; beta + gamma (looking along c): 89.5 / 35.7 / 98 / 90.6 / 34.2 / 84; alpha + gamma (looking along a): 88.4 / 45.7 / 97 / 89.7 / 43.5 / 84. The pinned face-up value above is the app's `body_colors` unpolarised color, the equal mean of the three rays.


### Alexandrite Strong Change

Key `alexandrite.strong_change`; `estimated = true`; 6 bands per ray (o_ray / e_ray / beta_ray).

**Measured, chosen, why.** Measured: Caltech GRR 874 alexandrite (Lake Manyara, 0.717 mm), beta (E||a) and gamma (E||b) rays; constants 0.1108 and 0.1552 removed, the Cr3+ R line (672-690 nm) interpolated; one factor Cr 0.15 over both rays. Fe3+: the GRR 171 beta and gamma rays with one factor 0.10 (alpha: their mean). Estimated: the alpha (E||c) ray, no spectrum exists: built from the measured gamma ray by splitting it at 490 nm into its 4T2 and 4T1 halves, moving the 4T2 half from 597 to 568 nm at 0.6 times the strength and the 4T1 half from 415 to 418 nm at 1.0 times (Farrell & Newnham Fig. 4, figure-read: 4T2 c:b = 19:31 and 4T1 c about 1.24 b, the 1.0 adopted by the research doc to discount their Fe3+ contamination). These per-band ratios describe an unmeasured ray, not a fit to a target color. Chosen: the two factors, by the target colors: green by daylight and red to purplish red by incandescent light (3-ray mean).

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 53.9 / 16.8 / 132 | 53.9 / 16.8 / 132 | 0.05 | 53.3 / 11.6 / 20 | 53.3 / 11.6 / 20 | 0.05 |
| o ray / alpha | 41.7 / 20.0 / 339 | 41.6 / 20.0 / 339 | 0.03 | 42.6 / 40.6 / 344 | 42.6 / 40.6 / 344 | 0.03 |
| beta ray | 68.4 / 43.9 / 93 | 68.4 / 43.9 / 93 | 0.07 | 69.5 / 41.1 / 63 | 69.5 / 41.2 / 63 | 0.04 |
| e ray / gamma | 45.6 / 75.3 / 187 | 45.6 / 75.4 / 187 | 0.05 | 39.5 / 61.8 / 206 | 39.4 / 61.9 / 206 | 0.06 |

**Fit.** Energy Gaussians, bounded, 6 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 1.0 / 6.5; beta: 0.6 / 5.5; e / gamma: 1.0 / 7.0. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 41.6 / 20.0 / 339; beta: 68.4 / 43.9 / 93; e / gamma: 45.6 / 75.4 / 187. Documented (Farrell & Newnham, Caltech): yellow along a (beta), green along b (gamma), red along c (alpha). Reached: beta 93 deg, gamma 187 deg, alpha 339 deg (the red ray is the estimated one).

**Oriented face-up views** (two eigenmodes, D65 / 3200 K): alpha + beta (looking along b: the app's default orientation, the gamma axis is the stone's `c_axis`): 57.4 / 23.1 / 78 / 58.5 / 30.1 / 34; beta + gamma (looking along c): 58.7 / 29.5 / 137 / 57.5 / 9.9 / 86; alpha + gamma (looking along a): 43.7 / 23.6 / 200 / 41.1 / 19.6 / 279. The pinned face-up value above is the app's `body_colors` unpolarised color, the equal mean of the three rays.


### Alexandrite Brazilian

Key `alexandrite.brazilian`; `estimated = true`; 6 bands per ray (o_ray / e_ray / beta_ray).

**Measured, chosen, why.** As Strong Change (same Cr3+ factor 0.15) with more Fe3+: factor 0.25 (GIA W2019: Brazil Cr 2960, Fe 6350 ppmw, about 2.3 times Russia's Fe at the same Cr). Chosen by the target colors: a yellower green by daylight, an orange-red by incandescent light.

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 50.5 / 18.6 / 111 | 50.5 / 18.7 / 111 | 0.04 | 50.4 / 17.4 / 35 | 50.4 / 17.4 / 35 | 0.02 |
| o ray / alpha | 38.7 / 21.9 / 356 | 38.6 / 21.9 / 356 | 0.05 | 40.3 / 41.2 / 354 | 40.3 / 41.2 / 354 | 0.04 |
| beta ray | 65.3 / 44.7 / 89 | 65.3 / 44.7 / 89 | 0.05 | 66.7 / 43.2 / 62 | 66.7 / 43.2 / 63 | 0.04 |
| e ray / gamma | 40.5 / 68.5 / 182 | 40.5 / 68.7 / 182 | 0.04 | 35.3 / 50.3 / 202 | 35.3 / 50.5 / 202 | 0.05 |

**Fit.** Energy Gaussians, bounded, 6 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 0.8 / 4.5; beta: 0.7 / 6.3; e / gamma: 0.8 / 4.9. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 38.6 / 21.9 / 356; beta: 65.3 / 44.7 / 89; e / gamma: 40.5 / 68.7 / 182. As documented: beta 89 deg yellow, gamma 182 deg blue-green, alpha 356 deg red.

**Oriented face-up views** (two eigenmodes, D65 / 3200 K): alpha + beta (looking along b: the app's default orientation, the gamma axis is the stone's `c_axis`): 54.5 / 27.0 / 74 / 55.9 / 33.8 / 39; beta + gamma (looking along c): 55.1 / 29.7 / 124 / 54.5 / 16.2 / 74; alpha + gamma (looking along a): 39.6 / 16.0 / 188 / 37.9 / 14.8 / 307. The pinned face-up value above is the app's `body_colors` unpolarised color, the equal mean of the three rays.


### Alexandrite Sri Lankan

Key `alexandrite.sri_lankan`; `estimated = true`; 7 bands per ray (o_ray / e_ray / beta_ray).

**Measured, chosen, why.** As Strong Change with weak Cr3+, factor 0.06, and Fe3+ 0.35 (GIA W2019: Sri Lanka Cr 702, Fe 7680 ppmw, the weak, yellowish change). Chosen by the target colors: a yellow-green by daylight and a weak shift to orange-brown under the lamp.

**Target and reached** (L* / C* / h, dE2000 between the color of the measured / recipe spectrum, the target, and the stored rounded bands, the reached; the pinned values are the reached ones):

| Direction | Target D65 | Reached D65 | dE2000 | Target 3200 K | Reached 3200 K | dE2000 |
|---|---|---|---|---|---|---|
| face-up (unpolarised) | 64.4 / 30.5 / 110 | 64.4 / 30.6 / 110 | 0.02 | 64.5 / 23.3 / 74 | 64.5 / 23.3 / 74 | 0.02 |
| o ray / alpha | 57.8 / 19.6 / 61 | 57.8 / 19.6 / 61 | 0.06 | 59.1 / 29.6 / 28 | 59.1 / 29.6 / 28 | 0.02 |
| beta ray | 77.8 / 47.4 / 98 | 77.8 / 47.4 / 98 | 0.01 | 78.9 / 42.6 / 78 | 78.9 / 42.6 / 78 | 0.01 |
| e ray / gamma | 53.0 / 51.2 / 163 | 53.0 / 51.3 / 163 | 0.02 | 49.9 / 29.0 / 175 | 49.9 / 29.0 / 175 | 0.03 |

**Fit.** Energy Gaussians, bounded, 7 bands per ray (limit 8), least squares on the transmittance weighted by the observer and the D65 / 3200 K illuminants plus a log-absorbance term; residual of the transmittance over 380-780 nm (RMS / largest, in percent points) o / alpha: 0.7 / 3.4; beta: 0.5 / 1.6; e / gamma: 0.8 / 4.1. Band positions near 250 or 1000 nm and widths at the 16000 cm^-1 bound are UV or near-infrared edge stand-ins, as in the other fitted presets.

**Per direction (reached, D65).** o / alpha: 57.8 / 19.6 / 61; beta: 77.8 / 47.4 / 98; e / gamma: 53.0 / 51.3 / 163. As documented: beta 98 deg yellow, gamma 163 deg green, alpha 61 deg orange-red.

**Oriented face-up views** (two eigenmodes, D65 / 3200 K): alpha + beta (looking along b: the app's default orientation, the gamma axis is the stone's `c_axis`): 69.0 / 34.0 / 90 / 70.2 / 33.5 / 62; beta + gamma (looking along c): 67.3 / 38.9 / 120 / 67.0 / 26.3 / 95; alpha + gamma (looking along a): 55.5 / 21.8 / 133 / 54.8 / 9.8 / 62. The pinned face-up value above is the app's `body_colors` unpolarised color, the equal mean of the three rays.

