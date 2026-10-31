# Fit record: beryl (Emerald, Aquamarine, Morganite), Topaz, Tourmaline, Andalusite (lane F2, 2026-10-27)

Entries: `crates/pleochroma/data/color_variants/beryl_topaz_tourmaline_andalusite.toml` (32 entries). Research: `docs/color_variants/R2_beryl_topaz_tourmaline_andalusite.md`. Pins: `crates/pleochroma/src/optics/materials/variants/tests_beryl_topaz_tourmaline_andalusite.rs`.

## Method (valid for every entry)

* **Units.** Every entry is `per_mm`: `peak` is the Napierian absorption coefficient in mm^-1 at the band centre, `width_nm` is the sigma of an energy Gaussian in cm^-1 (FWHM / 2.35482). The reference spectra give the decadic absorbance `A` over a path `p` (header `path_cm`), so `alpha = 2.302585 A / p_mm`. The research document's peaks were hand reads; the bands here are refitted by bounded least squares (`scipy.optimize.least_squares`) on the CSVs.
* **Fit.** Per ray, a sum of at most 5 energy Gaussians (UV edge, crystal-field / charge-transfer bands, NIR bands whose tails reach 650-780 nm) plus ONE free constant per ray (the instrument pedestal and reflection loss, constrained to >= 0 unless the file is baseline corrected, where a small negative range is allowed). The constant is removed and never rendered. UV edge bands are energy Gaussians centered at 285-345 nm (centre bounded to >= 285 nm, so some sit on that bound; only their 380-450 nm tail matters). Fit windows start between 280 nm (the sparse GIA 3828 topaz traces) and 410 nm (the heliodor e-ray), mostly at 340-380 nm; a steep wall below the window (Miku emerald at 330-345 nm, andalusite below 375 nm) is a detector effect and is excluded. Sharp Cr3+ R lines (680 nm) are not rendered. Fit residuals (RMS in mm^-1) are in the "Fits" tables below.
* **Face-up path.** The color pipeline is `crate::color::body_color::body_colors` at D65 (reproduced in Python from the repo's CIE 1931 table and D65 table; checked against the built-in swatches of `calibration-records.md`: Aquamarine 85.2 / 16.0 / 247 (record 85 / 16 / 247; e-ray 44.2 / 88.4 / 287), Emerald 64.8 / 99.6 / 168 (record 65 / 101 / 168), Ruby 40.5 / 67.7 / 356 (record 41 / 67 / 356), all over 1.0 model unit as the ModelUnit rule says). A `per_mm` tensor in a 7 mm stone is seen over `MODEL_UNIT_FACE_UP_PATH x 7 mm / REFERENCE_MODEL_WIDTH` = 2.83 x 7 / 2.0 = **9.905 mm** (`render_setup::absorption_path_scale_for` gives 3.5 mm per model unit; the face-up path is 2.83 model units). The tests call `variants::face_up_swatch_path(PerMm, 7.0)`. `reference_path_mm = 7.0` is the stone width the numbers are calibrated for, not the optical path.
* **Face-up color.** The unpolarised mean of `body_colors`: `(2 To + Te) / 3` for uniaxial hosts, `(To + Tb + Te) / 3` for biaxial hosts (transmittances, then colorimetry). The renderer's oriented face-up color (`face_up_body_color`) mixes the rays by the stone's own c axis and differs from this; the per-direction colors are listed so the pleochroism is visible.
* **Targets and scaling.** There are no published CIELAB values for these varieties. Each target is a typical faceted-stone color chosen by the lane from the variety descriptions (GIA and the cited papers) and then moved to the nearest color the measured spectrum can reach: L* and hue come from the variety, the chroma is what the spectrum gives at that lightness (the spectral model is more saturated than a photograph; the built-in Emerald is C* 100). The strength factors are the only free parameters: one factor for all bands of a crystal, or two (chromophore bands against the UV edge) where the edge of the measured crystal would give the wrong hue (the edge strength differs between crystals of the same variety by a factor of 3 or more). The ratio between rays is never changed, so the measured polarization is kept. Where a factor is an estimate the entry is `estimated = true`. The recorded targets are therefore consistent with, not independent of, the fits; the acceptance dE2000 <= 3 is met by construction and the real checks are the plausibility of hue and the pleochroism order (last line of every entry) against the documented one.
* **Tests.** The face-up color and the o / e (/ beta) colors the app computes for every entry must be within dE2000 1.0 of the numbers in the "reached" lines below (the test pins the numbers computed from the shipped TOML values).
* **Not modelled.** Band shapes are Gaussian in energy; real spectra have asymmetric wings, so the 380-450 nm tails (which decide hue for the nearly colorless varieties) are the least certain part of every fit.
* **c axis.** The built-in orientation differs inside the beryl family (Aquamarine and Tourmaline `c_axis = X`, Emerald and Morganite `Y`). A variant is a tensor only, so the face-up look depends on the stone's own c axis; the beryl-family entries (Heliodor, Red Beryl, Goshenite) are listed on Emerald, Aquamarine and Morganite with distinct keys and the same tensor (decision D5).
* **Reference-spectrum CSVs** stay in `crates/pleochroma/data/reference_spectra/`; none is embedded in library code.

## Fits (measured CSVs)

The lines marked "Superseded" are the first-round fits kept for the record; the entry sections give the fits that ship.

Each fit: file, window, constant removed, RMS residual, bands before any scaling (center nm / sigma cm^-1 / peak mm^-1).

* `emerald_zambian_o`: `variant_emerald_zambian_miku_eperpc.csv`, window 350-1100 nm, constant 0.121 mm^-1, RMS 0.0117 mm^-1; bands: 334.3 / 3783 / 0.2741; 438.7 / 1549 / 0.2292; 609.5 / 1471 / 0.1991; 824.9 / 936 / 0.7678
* `emerald_zambian_e`: `variant_emerald_zambian_miku_eparc.csv`, window 350-1100 nm, constant 0.195 mm^-1, RMS 0.0193 mm^-1; bands: 293.9 / 3977 / 1.326; 422.3 / 1195 / 0.1655; 636.5 / 1275 / 0.3775; 878.8 / 1603 / 0.4448
* `emerald_light_o`: `variant_emerald_light_jos_nigeria_eperpc.csv`, window 350-1100 nm, constant 0.281 mm^-1, RMS 0.0088 mm^-1; bands: 340.0 / 4874 / 0.1308; 435.6 / 1366 / 0.1422; 604.1 / 1608 / 0.1556; 808.0 / 938 / 0.5721
* `emerald_light_e`: `variant_emerald_light_jos_nigeria_eparc.csv`, window 350-1100 nm, constant 0.309 mm^-1, RMS 0.0147 mm^-1; bands: 323.1 / 5800 / 0.1091; 420.1 / 1187 / 0.09558; 625.0 / 1065 / 0.1827
* `helio309_o`: `variant_heliodor_golden_grr309_eperpc.csv`, window 374-1100 nm, constant 0.028 mm^-1, RMS 0.0106 mm^-1; bands: 295.5 / 4534 / 2; 847.7 / 500 / 0.008155
* `helio309_e`: `variant_heliodor_golden_grr309_eparc.csv`, window 410-1100 nm, constant 0.005 mm^-1, RMS 0.0123 mm^-1; bands: 315.5 / 5800 / 0.8897; 403.9 / 1400 / 0.9391; 902.7 / 1500 / 0.1519
* `redberyl_o`: `variant_red_beryl_grr602_eperpc.csv`, window 350-1000 nm, constant 0.197 mm^-1, RMS 0.0285 mm^-1; bands: 312.7 / 3500 / 1.17; 440.0 / 778 / 0.05098; 495.9 / 1358 / 0.3302; 554.6 / 830 / 0.2964
* `redberyl_e`: `variant_red_beryl_grr602_eparc.csv`, window 350-1000 nm, constant 0.782 mm^-1, RMS 0.0277 mm^-1; bands: 319.9 / 3500 / 1.194; 440.0 / 1500 / 0.1652; 543.6 / 1481 / 0.5137
* `aqua1780_o`: `variant_aquamarine_santa_maria_grr1780_eperpc.csv`, window 380-1250 nm, constant 0.067 mm^-1, RMS 0.0086 mm^-1; bands: 294.9 / 5800 / 1.101; 428.4 / 300 / 0.01295; 652.8 / 2999 / 0.08244; 829.2 / 931 / 0.5275 **Superseded 2026-10-27: refitted with the visible pedestal removed (see Santa Maria).**
* `aqua1780_e`: `variant_aquamarine_santa_maria_grr1780_eparc.csv`, window 380-1250 nm, constant 0.091 mm^-1, RMS 0.0154 mm^-1; bands: 340.0 / 3632 / 2; 426.8 / 300 / 0.04888; 724.1 / 3348 / 1.809; 844.4 / 911 / 0.6706; 1009.7 / 853 / 0.5389 **Superseded 2026-10-27: refitted with the visible pedestal removed (see Santa Maria).**
* `aqua1488_o`: `caltech_aquamarine_grr1488_eperpc_5p531mm.csv`, window 360-1099 nm, constant 0.212 mm^-1, RMS 0.0140 mm^-1; bands: 316.2 / 5800 / 0.3215; 429.7 / 300 / 0.0105; 679.0 / 2736 / 0.1117; 825.1 / 926 / 0.7198
* `aqua1488_e`: `caltech_aquamarine_grr1488_eparc_5p531mm.csv`, window 360-1099 nm, constant 0.347 mm^-1, RMS 0.0187 mm^-1; bands: 309.3 / 3500 / 0.5811; 427.8 / 300 / 0.02733; 658.6 / 2450 / 0.6564; 877.2 / 1857 / 0.8922
* `pink_a`: `variant_topaz_pink_cr_ural_bobrovka_alpha.csv`, window 340-1100 nm, constant 0.267 mm^-1, RMS 0.1221 mm^-1; bands: 340.0 / 3500 / 1.335; 405.4 / 2195 / 3.789; 559.1 / 1394 / 1.927; 900.0 / 4000 / 0.2479
* `pink_b`: `variant_topaz_pink_cr_ural_bobrovka_beta.csv`, window 340-1100 nm, constant 0.554 mm^-1, RMS 0.1132 mm^-1; bands: 285.0 / 3500 / 2.02; 394.5 / 2242 / 2.841; 538.8 / 1348 / 3.176
* `pink_g`: `variant_topaz_pink_cr_ural_bobrovka_gamma.csv`, window 340-1100 nm, constant 0.493 mm^-1, RMS 0.0838 mm^-1; bands: 288.3 / 3500 / 10; 407.3 / 2258 / 2.069; 550.4 / 1428 / 2.719
* `brown_a`: `variant_topaz_brown_natural_grr515_alpha.csv`, window 330-1000 nm, constant 0.003 mm^-1, RMS 0.0023 mm^-1; bands: 312.8 / 5800 / 0.2601; 478.0 / 3069 / 0.1203
* `brown_b`: `variant_topaz_brown_natural_grr515_beta.csv`, window 330-1000 nm, constant 0.008 mm^-1, RMS 0.0024 mm^-1; bands: 340.0 / 5314 / 0.1147; 472.9 / 3005 / 0.1073
* `blue3828_a`: `variant_topaz_blue_natural_gia3828_alpha.csv`, window 290-1200 nm, constant -0.027 mm^-1, RMS 0.0037 mm^-1; bands: 298.7 / 3500 / 0.2549; 442.1 / 2971 / 0.06138; 660.6 / 2260 / 0.163
* `blue3828_b`: `variant_topaz_blue_natural_gia3828_beta.csv`, window 290-1200 nm, constant -0.010 mm^-1, RMS 0.0054 mm^-1; bands: 285.0 / 3500 / 0.2155
* `blue3828_g`: `variant_topaz_blue_natural_gia3828_gamma.csv`, window 280-1200 nm, constant -0.006 mm^-1, RMS 0.0093 mm^-1; bands: 285.0 / 4150 / 0.1843; 600.0 / 2770 / 0.01813 **Superseded 2026-10-27: refitted with three bands (see Topaz Sky).**
* `blue1632_a`: `variant_topaz_blue_irradiated_grr1632_alpha.csv`, window 330-1200 nm, constant -0.050 mm^-1, RMS 0.0345 mm^-1; bands: 292.3 / 3500 / 3; 437.0 / 2213 / 0.428; 654.9 / 2285 / 1.536 **Superseded 2026-10-27: replaced by the lane B1 three-ray fit (see Topaz Swiss).**
* `blue1632_b`: `variant_topaz_blue_irradiated_grr1632_beta.csv`, window 330-1200 nm, constant 0.001 mm^-1, RMS 0.0149 mm^-1; bands: 285.0 / 3500 / 2.125; 590.0 / 3500 / 0.05406 **Superseded 2026-10-27: replaced by the lane B1 three-ray fit (see Topaz Swiss).**
* `blue1632_g`: `variant_topaz_blue_irradiated_grr1632_gamma.csv`, window 330-1200 nm, constant 0.012 mm^-1, RMS 0.0072 mm^-1; bands: 286.1 / 3500 / 2.552; 429.4 / 2327 / 0.1425; 648.0 / 2418 / 0.2948 **Superseded 2026-10-27: replaced by the lane B1 three-ray fit (see Topaz Swiss).**
* `rubellite_o`: `variant_tourmaline_rubellite_grr1368_eperpc.csv`, window 350-1099 nm, constant 0.569 mm^-1, RMS 0.0051 mm^-1; bands: 285.0 / 4209 / 1.928; 453.6 / 2460 / 0.253; 542.2 / 1119 / 0.569; 1028.3 / 1426 / 0.1287
* `rubellite_e`: `variant_tourmaline_rubellite_grr1368_eparc.csv`, window 350-1099 nm, constant 0.388 mm^-1, RMS 0.0121 mm^-1; bands: 307.4 / 3500 / 0.4906; 479.6 / 2919 / 0.268; 539.3 / 883 / 0.193
* `indicolite_o`: `variant_tourmaline_indicolite_grr3770_eperpc.csv`, window 330-1250 nm, constant 0.346 mm^-1, RMS 0.0352 mm^-1; bands: 340.0 / 5800 / 0.3364; 712.3 / 1029 / 0.9474; 1158.9 / 911 / 0.3116 **Superseded 2026-10-27: refitted with the visible floor removed (see Indicolite).**
* `indicolite_e`: `variant_tourmaline_indicolite_grr3770_eparc.csv`, window 330-1250 nm, constant 0.338 mm^-1, RMS 0.0165 mm^-1; bands: 340.0 / 5800 / 0.226; 717.2 / 1062 / 0.9441; 1185.9 / 1004 / 0.368 **Superseded 2026-10-27: refitted with the visible floor removed (see Indicolite).**
* `verdelite_o`: `variant_tourmaline_verdelite_grr1935_green_eperpc.csv`, window 380-1099 nm, constant 0.263 mm^-1, RMS 0.0219 mm^-1; bands: 285.0 / 5684 / 2.272; 704.3 / 1017 / 0.5326; 1106.5 / 800 / 0.1726 **Superseded 2026-10-27: replaced by the lane B1 four-band fit (see Verdelite).**
* `verdelite_e`: `variant_tourmaline_verdelite_grr1935_green_eparc.csv`, window 380-1099 nm, constant 0.297 mm^-1, RMS 0.0139 mm^-1; bands: 285.0 / 4273 / 2.694; 707.4 / 1019 / 0.4634; 1152.1 / 800 / 0.1056 **Superseded 2026-10-27: replaced by the lane B1 four-band fit (see Verdelite).**
* `paraiba_o`: `variant_tourmaline_paraiba_r030_eperpc.csv`, window 340-1250 nm, constant 0.488 mm^-1, RMS 0.1447 mm^-1; bands: 293.9 / 3695 / 4.872; 718.7 / 1500 / 3.576; 907.0 / 786 / 6.26; 1100.0 / 1500 / 1.232
* `paraiba_e`: `variant_tourmaline_paraiba_r030_eparc.csv`, window 340-1250 nm, constant 2.875 mm^-1, RMS 0.4689 mm^-1; bands: 312.1 / 3500 / 12; 704.0 / 1377 / 11.08; 763.1 / 700 / 2.963; 955.8 / 1390 / 15.65
* `paraibaviolet_o`: `variant_tourmaline_paraiba_mn_cu_pink_eperpc.csv`, window 350-1250 nm, constant 0.527 mm^-1, RMS 0.0544 mm^-1; bands: 285.0 / 3500 / 1.258; 524.1 / 1069 / 0.8378; 715.9 / 905 / 0.2993; 905.6 / 912 / 0.9422; 1100.0 / 1500 / 0.004379
* `paraibaviolet_e`: `variant_tourmaline_paraiba_mn_cu_pink_eparc.csv`, window 350-1100 nm, constant 0.037 mm^-1, RMS 0.0167 mm^-1; bands: 340.0 / 5800 / 0.3817; 509.0 / 2600 / 0.6192; 717.5 / 1279 / 0.4352; 954.2 / 1800 / 0.5472
* `yellow_o`: `variant_tourmaline_yellow_grr1935_yellow_eperpc.csv`, window 300-900 nm, constant 0.100 mm^-1, RMS 0.0199 mm^-1; bands: 386.6 / 1745 / 0.4738
* `yellow_e`: `variant_tourmaline_yellow_grr1935_yellow_eparc.csv`, window 300-900 nm, constant -0.006 mm^-1, RMS 0.0163 mm^-1; bands: 367.9 / 1553 / 0.3371
* `andfeti_a`: `variant_andalusite_fe_ti_brazil_light_zone_alpha.csv`, window 378-1100 nm, constant 0.000 mm^-1, RMS 0.0107 mm^-1; bands: 285.0 / 5011 / 4.323; 464.6 / 2041 / 1.674; 510.0 / 1426 / 0.919; 600.0 / 1910 / 0.4425; 730.7 / 2818 / 0.1726
* `andfeti_b`: `variant_andalusite_fe_ti_brazil_light_zone_beta.csv`, window 380-1100 nm, constant 0.024 mm^-1, RMS 0.0110 mm^-1; bands: 297.8 / 4342 / 3; 480.0 / 4500 / 0.04364
* `andfeti_g`: `variant_andalusite_fe_ti_brazil_light_zone_gamma.csv`, window 380-1100 nm, constant 0.030 mm^-1, RMS 0.0079 mm^-1; bands: 340.0 / 3739 / 0.3416; 480.0 / 4500 / 0.02334
* `andv_a`: `variant_andalusite_v_cr_llano_alpha.csv`, window 290-880 nm, constant 0.434 mm^-1, RMS 0.2027 mm^-1; bands: 259.1 / 5800 / 21.49; 418.6 / 2131 / 16.35; 563.5 / 1344 / 3.958; 700.0 / 1731 / 1.646
* `andv_b`: `variant_andalusite_v_cr_llano_beta.csv`, window 320-860 nm, constant 0.846 mm^-1, RMS 0.2039 mm^-1; bands: 286.1 / 3500 / 30; 407.1 / 1717 / 1.694; 557.3 / 1800 / 0.6104
* `andv_g`: `variant_andalusite_v_cr_llano_gamma.csv`, window 290-860 nm, constant 0.769 mm^-1, RMS 0.0603 mm^-1; bands: 283.5 / 4848 / 5.061; 417.3 / 2203 / 0.6652; 578.7 / 1800 / 0.3085

Fit notes: `aqua1488_*` is the same crystal as the Aquamarine built-in, refitted per mm on its CSV (the built-in's ModelUnit numbers are alpha x 8.8 mm, a rule that predates the 2.83 face-up constant). The Zambian and Light emerald fits and the Sherry / Pink topaz fits have RMS far below the signal; the largest relative misfits are the UV edge of the e-ray of Miku emerald (the wall below 350 nm is excluded) and the Paraiba R030 e-ray plateau (RMS 0.47 of a 12 mm^-1 signal, the section is a thin 0.2 mm crystal with a pedestal of 2.9 mm^-1).

## Update 2026-10-27 (fix round X6)

Applies to every entry below; the per-entry sections were corrected where the data changed.

* **Estimated flag (owner decision O3, 2026-10-27).** `estimated = true` when the spectral shape is not a measured spectrum of that variety, OR the strength is not anchored to a measurement (a chosen strength counts: the Cornflower / Deep Blue / Pigeon's Blood convention of lane F1 now holds in every group); `estimated = false` only when the shape is a measured spectrum of that very variety at a strength that is itself measured or published. The first `sources` line of every entry is ONE short plain sentence that names the estimated part; the UI shows it as the tooltip "Estimated: <first line>" (no tooltip for a measured entry). Citations, URLs, DOIs and repository paths start on the second line.
* **Face-up color (decision D-G).** The swatches and pins of the Variants list show the ORIENTED face-up color, the color the renderer shows for the stone: with the c axis along the view the o-ray alone (uniaxial hosts with `c_axis` Y: Sapphire, Ruby, Zircon, Benitoite, Emerald, Morganite, Amethyst, Quartz, Rutile, Moissanite), (o + e) / 2 for the hosts whose c axis lies in the table plane (Aquamarine and Tourmaline, `c_axis` X), (alpha + beta) / 2 for biaxial hosts (gamma along `c_axis` Y: Topaz, Chrysoberyl, Alexandrite, Tanzanite, Peridot, Andalusite); isotropic hosts show the one color. The entry sections below keep the unpolarised mean (the fit target, what `body_colors` returns) in their "reached" lines and list the oriented color as "reached face-up (oriented ...)"; the tests pin both. The face-up path is 9.905 mm (`reference_path_mm` = the 7 mm stone width, decision D-A).
* **Colorless entries (decision D-C).** An entry with no bands is valid on any host; the `pleochroic = true` trick was removed from the colorless entries.
* **Measured entries in this file** (`estimated = false`): `topaz.pink`. Every other one of the 32 entries is estimated.
* **F-1 Indicolite, F-2 Santa Maria (owner decision O2).** Both ship the MEASURED crystal at ONE strength factor, the UV edge included, and are estimated (the strength is chosen). Indicolite: free three-band fit with the visible floor removed, k 3.0, face-up h 150-165 at every strength, so it is a teal green, greener than the trade name; Santa Maria: e-ray refitted after removing the header's visible pedestal (A 0.16 o / 0.40 e), k 0.5, o-ray yellow-green at every strength, so the face-up stone is greener than the trade name. See the entry sections.
* **F-3 Verdelite, F-4 Topaz Swiss / London, F-5 Topaz Sky.** Refitted as the entry sections say (four-band fit of the same crystal; the B1 three-ray fit of GRR 1632; gamma refitted with three bands). One factor per spectrum, the edge included. The beta ray of Swiss and London is the measured near-neutral teal, not violet.
* **Rubellite 0.8 and Paraiba 0.31** are one factor over every fitted band (the earlier 0.79 / 1.036 and 0.307 / 0.318 splits). F-11: the Paraiba e-ray plateau is only partly fitted; the e-ray is an estimate (see the entry).
* **F-16 Morganite Pink.** The 404 nm Fe3+ band of the heliodor part is removed from the e-ray (it turned the e-ray peach, h 10); the e-ray is now light violet (h 332), the documented morganite pleochroism.
* **F-15 first source lines.** Replaced by the short sentences below; the long citations moved to the second line.
* **F-17 citations.** Re-checked; the unverified DOI of `pyrope_garnet.rhodolite` is handled in the F3 record, and the vague citations (Springer PCM 2005, Optical Materials 2000, J. Gems & Gemmology 2024) were completed or marked in their entries.

**Tooltip sentences and oriented face-up colors** (D65, 9.905 mm):

| key | host | estimated | tooltip sentence | face-up (oriented) L* / C* / h | unpolarised mean L* / C* / h |
|---|---|---|---|---|---|
| `emerald.zambian` | Emerald | yes | Strength chosen for a typical face-up color; spectrum measured (Caltech Miku, Zambia). | 35.2 / 83.2 / 158 | 46.1 / 93.6 / 171 |
| `emerald.light` | Emerald | yes | Strength chosen for a typical face-up color; spectrum measured (Caltech Jos, Nigeria). | 83.0 / 21.9 / 160 | 85.4 / 24.3 / 167 |
| `emerald.heliodor` | Emerald | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 309, sparse digitised traces). | 94.2 / 64.0 / 99 | 85.6 / 63.7 / 92 |
| `emerald.red_beryl` | Emerald | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 602). | 44.6 / 81.3 / 3 | 39.3 / 74.7 / 1 |
| `emerald.goshenite` | Emerald | yes | Colorless: no visible absorption assumed; no goshenite spectrum exists. | 100.0 / 0.0 / 0 | 100.0 / 0.0 / 0 |
| `aquamarine.santa_maria` | Aquamarine | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 1780). | 80.0 / 12.8 / 231 | 87.1 / 7.9 / 175 |
| `aquamarine.greenish` | Aquamarine | yes | Blue part measured (GRR 1488) at a chosen strength; the Fe3+ amount is estimated. | 79.1 / 18.1 / 218 | 83.0 / 13.2 / 204 |
| `aquamarine.heliodor` | Aquamarine | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 309, sparse digitised traces). | 80.6 / 64.7 / 88 | 85.6 / 63.7 / 92 |
| `aquamarine.red_beryl` | Aquamarine | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 602). | 36.3 / 71.0 / 359 | 39.3 / 74.7 / 1 |
| `aquamarine.goshenite` | Aquamarine | yes | Colorless: no visible absorption assumed; no goshenite spectrum exists. | 100.0 / 0.0 / 0 | 100.0 / 0.0 / 0 |
| `morganite.pink` | Morganite | yes | Built from the bands of a different beryl (red beryl GRR 602) and published positions; strength estimated. | 90.6 / 16.5 / 349 | 89.0 / 16.7 / 343 |
| `morganite.peach` | Morganite | yes | Red beryl bands (GRR 602) plus an Fe3+ part (GRR 309), both at chosen strengths. | 88.4 / 16.6 / 17 | 85.5 / 17.6 / 40 |
| `morganite.heliodor` | Morganite | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 309, sparse digitised traces). | 94.2 / 64.0 / 99 | 85.6 / 63.7 / 92 |
| `morganite.red_beryl` | Morganite | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 602). | 44.6 / 81.3 / 3 | 39.3 / 74.7 / 1 |
| `morganite.goshenite` | Morganite | yes | Colorless: no visible absorption assumed; no goshenite spectrum exists. | 100.0 / 0.0 / 0 | 100.0 / 0.0 / 0 |
| `topaz.imperial` | Topaz | yes | Two measured topaz spectra (Cr3+ pink, brown GRR 515a) combined at chosen strengths; no imperial spectrum exists. | 56.4 / 49.2 / 52 | 57.0 / 47.9 / 51 |
| `topaz.sherry` | Topaz | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 515a, gamma taken equal to beta). | 70.3 / 41.9 / 63 | 71.1 / 41.3 / 63 |
| `topaz.pink` | Topaz | no | - | 74.0 / 20.1 / 352 | 73.2 / 22.3 / 343 |
| `topaz.sky` | Topaz | yes | Strength chosen for a typical face-up color; spectrum measured (GIA 3828). | 83.4 / 8.5 / 223 | 86.5 / 6.5 / 211 |
| `topaz.swiss` | Topaz | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 1632). | 72.9 / 10.7 / 239 | 73.2 / 14.0 / 227 |
| `topaz.london` | Topaz | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 1632). | 66.7 / 7.6 / 223 | 63.8 / 14.0 / 219 |
| `topaz.colorless` | Topaz | yes | Colorless: no visible absorption assumed; host edge near 220 nm. | 100.0 / 0.0 / 0 | 100.0 / 0.0 / 0 |
| `tourmaline.rubellite` | Tourmaline | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 1368). | 45.3 / 62.4 / 4 | 43.7 / 65.2 / 3 |
| `tourmaline.pink` | Tourmaline | yes | Measured rubellite spectrum (GRR 1368) scaled down to a light pink; no light pink spectrum exists. | 80.0 / 26.7 / 356 | 78.7 / 29.2 / 354 |
| `tourmaline.indicolite` | Tourmaline | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 3770). | 82.0 / 75.9 / 154 | 82.1 / 76.4 / 154 |
| `tourmaline.verdelite` | Tourmaline | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 1935 green). | 73.2 / 37.7 / 154 | 70.9 / 44.1 / 150 |
| `tourmaline.chrome` | Tourmaline | yes | Built from published band positions; strength estimated. | 63.0 / 83.1 / 170 | 60.0 / 89.5 / 169 |
| `tourmaline.paraiba` | Tourmaline | yes | Strength chosen for a typical face-up color; spectrum measured (GIA R030, a thin Cu-rich section scaled down). | 78.4 / 71.3 / 193 | 80.1 / 66.9 / 195 |
| `tourmaline.paraiba_violet` | Tourmaline | yes | Strength chosen for a typical face-up color; spectrum measured (Mn-Cu elbaite). | 51.2 / 74.5 / 332 | 54.9 / 84.5 / 331 |
| `tourmaline.yellow` | Tourmaline | yes | Strength chosen for a typical face-up color; spectrum measured (GRR 1935 yellow, a very pale crystal scaled up about tenfold). | 98.9 / 48.1 / 109 | 98.6 / 59.5 / 108 |
| `andalusite.greener` | Andalusite | yes | Measured Fe-Ti spectrum plus an estimated Mn3+ part, both at chosen strengths. | 73.1 / 46.2 / 106 | 81.5 / 45.8 / 113 |
| `andalusite.vanadian` | Andalusite | yes | Strength chosen for a typical face-up color; spectrum measured (Llano, Texas). | 80.1 / 15.6 / 96 | 86.2 / 11.5 / 100 |


## Entries

### emerald-zambian

Keys: `emerald.zambian`. Label: Zambian. Estimated (`estimated = true`).

**Sources.**

* Strength chosen for a typical face-up color; spectrum measured (Caltech Miku, Zambia).
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: beryl, Miku, Zambia, light green emerald (M. N. Taran), 2.148 mm, E perpendicular / parallel to c, http://minerals.gps.caltech.edu/FILES/Visible/Beryl/taran/Emerald_Zambia.a and .c (reference_spectra/variant_emerald_zambian_miku_{eperpc,eparc}.csv)
* Bands refitted per ray with bounded energy Gaussians (Cr3+ 4T1 and 4T2, Fe2+ near 825-880 nm, UV edge), constant baseline removed; scaled to a medium-dark bluish green stone (fit record, emerald.zambian)

**Fit.** `emerald_zambian_o`, `emerald_zambian_e` (see Fits above).

**Scaling.** All bands of the measured crystal scaled together: the crystal's own Cr:Fe ratio is kept, so the strong Fe2+ band pushes the hue slightly bluer than the Material Default (h 171 against 168) and the stone is much darker (L* 46 against 65), as the trade describes Zambian emerald. The Cr R lines at 680 nm are not rendered.
Strength factors applied to the fitted bands (in the order of the parts of the fit script): 2.614.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 334.3 | 3783.2 | 8909 | 0.7166 |
  | 438.7 | 1549.0 | 3648 | 0.5991 |
  | 609.5 | 1470.6 | 3463 | 0.5206 |
  | 824.9 | 936.2 | 2205 | 2.007 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 293.9 | 3977.1 | 9365 | 3.466 |
  | 422.3 | 1194.8 | 2814 | 0.4327 |
  | 636.5 | 1275.0 | 3002 | 0.987 |
  | 878.8 | 1602.9 | 3775 | 1.163 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 46, C* 90, h 171
* reached face-up (oriented, the renderer's color; pinned): L* 35.2, C* 83.2, h 158 (unpolarised mean, the fit target: L* 46.1, C* 93.6, h 171)
* o-ray / alpha: L* 35.2, C* 83.2, h 158; e-ray / gamma: L* 60.7, C* 113.5, h 177
* Lab (pinned by the test): face [35.15, -77.19, 31.01], o [35.15, -77.19, 31.01], e [60.70, -113.35, 5.22]

**Pleochroism check.** o-ray yellowish green, e-ray bluish green (hue o < e), matching Wood and Nassau 1968 as quoted in the Emerald calibration record.

### emerald-light

Keys: `emerald.light`. Label: Light. Estimated (`estimated = true`).

**Sources.**

* Strength chosen for a typical face-up color; spectrum measured (Caltech Jos, Nigeria).
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: beryl, Jos, Nigeria, light green emerald (M. N. Taran), 0.95 mm, both rays (reference_spectra/variant_emerald_light_jos_nigeria_{eperpc,eparc}.csv)
* Bands refitted per ray (Cr3+ 4T1 / 4T2, Fe2+ near 808 nm, UV edge), baseline removed; scaled to a pale green stone (fit record, emerald.light)

**Fit.** `emerald_light_o`, `emerald_light_e` (see Fits above).

**Scaling.** All bands of the measured crystal scaled together to a light green stone (L* 85), the class below the lab line that separates emerald from green beryl.
Strength factors applied to the fitted bands (in the order of the parts of the fit script): 0.4491.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 340.0 | 4874.3 | 11478 | 0.05876 |
  | 435.6 | 1366.0 | 3217 | 0.06389 |
  | 604.1 | 1608.4 | 3787 | 0.06987 |
  | 808.0 | 937.5 | 2208 | 0.2569 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 323.1 | 5800.0 | 13658 | 0.04899 |
  | 420.1 | 1187.5 | 2796 | 0.04293 |
  | 625.0 | 1064.6 | 2507 | 0.08205 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 85, C* 24, h 166
* reached face-up (oriented, the renderer's color; pinned): L* 83.0, C* 21.9, h 160 (unpolarised mean, the fit target: L* 85.4, C* 24.3, h 167)
* o-ray / alpha: L* 83.0, C* 21.9, h 160; e-ray / gamma: L* 89.8, C* 29.4, h 176
* Lab (pinned by the test): face [83.03, -20.61, 7.50], o [83.03, -20.61, 7.50], e [89.79, -29.30, 1.86]

**Pleochroism check.** o-ray yellowish green, e-ray bluish green (hue o < e); the Fe2+ band is mainly in the o-ray, as measured.

### heliodor

Keys: `emerald.heliodor`, `aquamarine.heliodor`, `morganite.heliodor`. Label: Heliodor. Estimated (`estimated = true`).

**Sources.**

* Strength chosen for a typical face-up color; spectrum measured (GRR 309, sparse digitised traces).
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: beryl GRR 309, golden heliodor, Goias, 3.306 mm, baseline corrected (reference_spectra/variant_heliodor_golden_grr309_{eperpc,eparc}.csv; sparse digitised traces, o-ray 374-1295 nm, e-ray from 410 nm)
* Fe3+ (UV edge, 427 nm line); minor Fe2+ near 800 nm (Caltech beryl index page). The color is defined relative to the c axis (o = E perpendicular to c, e = E parallel to c): the stone's own c axis decides the face-up look

**Fit.** `helio309_o`, `helio309_e` (see Fits above).

**Scaling.** Both rays of the crystal scaled together to a golden yellow stone (L* 86). The e-ray band is fitted to data that start at 410 nm, so its centre (404 nm) is the least certain number; GRR 226 gives a weaker e-ray (R2 section 2.5).
Strength factors applied to the fitted bands (in the order of the parts of the fit script): 2.296.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 295.5 | 4533.9 | 10677 | 4.593 |
  | 847.7 | 500.0 | 1177 | 0.01873 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 315.5 | 5800.0 | 13658 | 2.043 |
  | 403.9 | 1400.4 | 3298 | 2.157 |
  | 902.7 | 1500.0 | 3532 | 0.3487 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 85, C* 62, h 92
* reached face-up on Emerald (c along the view: the o-ray alone; pinned): L* 94.2, C* 64.0, h 99 (unpolarised mean: L* 85.6, C* 63.7, h 92)
* reached face-up on Aquamarine (c in the table plane: (o + e) / 2; pinned): L* 80.6, C* 64.7, h 88 (unpolarised mean: L* 85.6, C* 63.7, h 92)
* reached face-up on Morganite (c along the view: the o-ray alone; pinned): L* 94.2, C* 64.0, h 99 (unpolarised mean: L* 85.6, C* 63.7, h 92)
* o-ray / alpha: L* 94.2, C* 64.0, h 99; e-ray / gamma: L* 61.6, C* 93.4, h 75
* Lab (pinned by the test): Emerald face [94.22, -9.74, 63.25]; Aquamarine face [80.65, 1.99, 64.67]; Morganite face [94.22, -9.74, 63.25]; o [94.22, -9.74, 63.25], e [61.56, 23.76, 90.31]

**Pleochroism check.** Weak in the reference; the data give an o-ray light yellow and a deeper yellow-orange e-ray.

### red-beryl

Keys: `emerald.red_beryl`, `aquamarine.red_beryl`, `morganite.red_beryl`. Label: Red Beryl. Estimated (`estimated = true`).

**Sources.**

* Strength chosen for a typical face-up color; spectrum measured (GRR 602).
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: beryl GRR 602, red beryl, Wah Wah Mountains, Utah, 1.559 mm (reference_spectra/variant_red_beryl_grr602_{eperpc,eparc}.csv)
* Mn3+ on the Al site (Nassau and Wood 1968, Am. Mineral. 53:801). The color is defined relative to the c axis (o = E perpendicular to c, e = E parallel to c): the stone's own c axis decides the face-up look

**Fit.** `redberyl_o`, `redberyl_e` (see Fits above).

**Scaling.** Both rays of the crystal scaled together to a raspberry red stone (L* 39). The e-ray pedestal (0.78 mm^-1) is fitted as a free constant and removed.
Strength factors applied to the fitted bands (in the order of the parts of the fit script): 1.197.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 312.7 | 3500.0 | 8242 | 1.401 |
  | 440.0 | 778.0 | 1832 | 0.06105 |
  | 495.9 | 1358.1 | 3198 | 0.3955 |
  | 554.6 | 830.3 | 1955 | 0.3549 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 319.9 | 3500.0 | 8242 | 1.43 |
  | 440.0 | 1500.0 | 3532 | 0.1979 |
  | 543.6 | 1480.8 | 3487 | 0.6151 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 40, C* 74, h 2
* reached face-up on Emerald (c along the view: the o-ray alone; pinned): L* 44.6, C* 81.3, h 3 (unpolarised mean: L* 39.3, C* 74.7, h 1)
* reached face-up on Aquamarine (c in the table plane: (o + e) / 2; pinned): L* 36.3, C* 71.0, h 359 (unpolarised mean: L* 39.3, C* 74.7, h 1)
* reached face-up on Morganite (c along the view: the o-ray alone; pinned): L* 44.6, C* 81.3, h 3 (unpolarised mean: L* 39.3, C* 74.7, h 1)
* o-ray / alpha: L* 44.6, C* 81.3, h 3; e-ray / gamma: L* 23.8, C* 58.4, h 347
* Lab (pinned by the test): Emerald face [44.60, 81.14, 4.74]; Aquamarine face [36.26, 71.02, -1.60]; Morganite face [44.60, 81.14, 4.74]; o [44.60, 81.14, 4.74], e [23.77, 56.83, -13.38]

**Pleochroism check.** o-ray red, e-ray deeper purplish red.

### goshenite

Keys: `emerald.goshenite`, `aquamarine.goshenite`, `morganite.goshenite`. Label: Goshenite. Estimated (`estimated = true`).

**Sources.**

* Colorless: no visible absorption assumed; no goshenite spectrum exists.
* Colorless beryl: no absorption in 380-780 nm, the host edge lies near 300 nm (no goshenite spectrum on the Caltech server; GRR 3557 'colorless perp c' is an aquamarine)
* The color is defined relative to the c axis (o = E perpendicular to c, e = E parallel to c): the stone's own c axis decides the face-up look

**Fit.** None (no spectrum to fit); see sources.

**Scaling.** Empty o_ray and e_ray with pleochroic = true (a valid colorless tensor of a pleochroic host).

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  none (colorless)

* e_ray / gamma:

  none (colorless)

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 100, C* 0, h 0
* reached face-up on Emerald (c along the view: the o-ray alone; pinned): L* 100.0, C* 0.0, h 0 (unpolarised mean: L* 100.0, C* 0.0, h 0)
* reached face-up on Aquamarine (c in the table plane: (o + e) / 2; pinned): L* 100.0, C* 0.0, h 0 (unpolarised mean: L* 100.0, C* 0.0, h 0)
* reached face-up on Morganite (c along the view: the o-ray alone; pinned): L* 100.0, C* 0.0, h 0 (unpolarised mean: L* 100.0, C* 0.0, h 0)
* o-ray / alpha: L* 100.0, C* 0.0, h 0; e-ray / gamma: L* 100.0, C* 0.0, h 0
* Lab (pinned by the test): Emerald face [100.00, 0.00, 0.00]; Aquamarine face [100.00, 0.00, 0.00]; Morganite face [100.00, 0.00, 0.00]; o [100.00, 0.00, 0.00], e [100.00, 0.00, 0.00]

**Pleochroism check.** Colorless in both rays.

### aquamarine-santa-maria

Keys: `aquamarine.santa_maria`. Label: Santa Maria. Estimated (`estimated = true`).

**Sources.**

* Strength chosen for a typical face-up color; spectrum measured (GRR 1780).
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: beryl GRR 1780, deep blue aquamarine (Tatu mine, Minas Gerais; stand-in for Santa Maria, there is no Santa Maria sample on the server), 2.449 mm (reference_spectra/variant_aquamarine_santa_maria_grr1780_{eperpc,eparc}.csv)
* Refitted 2026-10-27 per ray (UV edge, Fe3+ 428 nm, IVCT 660 nm, Fe2+ 830-860 nm, near-infrared band) with the header's visible pedestal (A 0.16 o / 0.40 e) removed first; ONE factor 0.5 over every band, the edge included (owner rule). The crystal's o-ray is yellow-green and its face-up color a teal blue, greener than the trade name suggests

**Fit.** Per ray, energy Gaussians fitted by bounded least squares to the CSV with the header's visible pedestal removed first (A 0.16 o / 0.40 e, the visible minima the CSV header states; free constant fixed at 0), window 380-1250 nm, visible weight 3: o-ray RMS 0.0133 mm^-1 (UV edge 340 nm, Fe3+ 428 nm, 720 nm, Fe2+ 825 nm), e-ray RMS 0.0240 mm^-1 (edge 360 nm, 423 nm, IVCT 662 nm, Fe2+ 859 nm, 1047 nm). Model / data in the visible: o 420-750 nm within +-0.005 mm^-1, e-ray 450 nm 0.063 against 0.042, 600-750 nm within 2 percent. (The earlier fit put the free constant at 0.071 / 0.097 mm^-1, below the CSV's own visible minima 0.155 / 0.391 A, so the pedestal sat in the bands and the e-ray came out at L* 17.7.)

**Scaling.** ONE factor 0.5 over every band, the UV edge included (owner rule 2026-10-27: measured values win). The factor is chosen for a saleable face-up lightness: with the c axis in the table plane the face-up color is (o + e) / 2 and cannot get darker than half the o-ray's transmittance (L* about 76 however strong the e-ray), so the factor mainly sets the chroma: k 0.4 gives L* 81.4 C* 14.7 h 232, k 0.5 80.0 / 12.8 / 231, k 0.7 78.2 / 9.0 / 218, k 1.0 76.6 / 7.4 / 168. **The crystal is greener than the trade name suggests**: its o-ray stays yellow-green (h 113, C* 20) at every strength because the 428 nm Fe3+ band and the edge tail are part of the measured spectrum, and only the deep blue-violet e-ray (L* 47, C* 63, h 259 at k 0.5) makes the face-up teal blue.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 340.0 | 3370.4 | 7937 | 0.2966 |
  | 428.3 | 373.5 | 880 | 0.01271 |
  | 720.0 | 1500.0 | 3532 | 0.00543 |
  | 825.1 | 872.0 | 2053 | 0.2442 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 360.0 | 2121.7 | 4996 | 0.8119 |
  | 423.4 | 500.0 | 1177 | 0.05513 |
  | 662.4 | 2237.3 | 5268 | 0.6372 |
  | 859.2 | 1500.0 | 3532 | 0.7427 |
  | 1047.3 | 806.2 | 1898 | 0.21 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 74, C* 16, h 248
* reached face-up (oriented, the renderer's color; pinned): L* 80.0, C* 12.8, h 231 (unpolarised mean, the fit target: L* 87.1, C* 7.9, h 175)
* o-ray / alpha: L* 99.0, C* 19.5, h 113; e-ray / gamma: L* 46.9, C* 62.9, h 259
* Lab (pinned by the test): face [79.97, -8.07, -9.88], o [98.97, -7.63, 17.90], e [46.85, -12.35, -61.71]

**Pleochroism check.** Blue to darker blue: the o-ray is yellow-green and nearly colorless (L* 99, C* 19.5, h 113), the e-ray a deep blue-violet (L* 46.9, C* 62.9, h 259).

### aquamarine-greenish

Keys: `aquamarine.greenish`. Label: Greenish. Estimated (`estimated = true`).

**Sources.**

* Blue part measured (GRR 1488) at a chosen strength; the Fe3+ amount is estimated.
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: beryl GRR 1488 Lajes Pintada, 5.531 mm (reference_spectra/caltech_aquamarine_grr1488_{eperpc,eparc}_5p531mm.csv) refitted per mm for the Fe2+ / Fe2+-Fe3+ blue; GRR 309 golden heliodor (variant_heliodor_golden_grr309_*.csv) for the added Fe3+ absorption
* Estimated: the Fe3+ amount of a greenish aquamarine is a free parameter (GIA: unheated aquamarine leans greenish; Caltech GRR 1754: heating turns greenish-yellow to blue)

**Fit.** `aqua1488_o`, `aqua1488_e`, `helio309_o`, `helio309_e` (see Fits above).

**Scaling.** Blue part: GRR 1488 bands at the strength of the Material Default's face-up color (L* 85). Fe3+ part: the heliodor GRR 309 bands, scaled for a greenish-blue hue (h 204 against 236). Estimated.
Strength factors applied to the fitted bands (in the order of the parts of the fit script): 0.34, 0.0754.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 295.5 | 4533.9 | 10677 | 0.1508 |
  | 316.2 | 5800.0 | 13658 | 0.1093 |
  | 429.7 | 300.0 | 706 | 0.003569 |
  | 679.0 | 2735.8 | 6442 | 0.03799 |
  | 825.1 | 925.9 | 2180 | 0.2447 |
  | 847.7 | 500.0 | 1177 | 0.0006149 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 309.3 | 3500.0 | 8242 | 0.1976 |
  | 315.5 | 5800.0 | 13658 | 0.06708 |
  | 403.9 | 1400.4 | 3298 | 0.07081 |
  | 427.8 | 300.0 | 706 | 0.009292 |
  | 658.6 | 2450.1 | 5770 | 0.2232 |
  | 877.2 | 1857.2 | 4373 | 0.3033 |
  | 902.7 | 1500.0 | 3532 | 0.01145 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 83, C* 14, h 204
* reached face-up (oriented, the renderer's color; pinned): L* 79.1, C* 18.1, h 218 (unpolarised mean, the fit target: L* 83.0, C* 13.2, h 204)
* o-ray / alpha: L* 90.0, C* 10.1, h 150; e-ray / gamma: L* 65.0, C* 41.4, h 233
* Lab (pinned by the test): face [79.10, -14.30, -11.13], o [89.95, -8.76, 5.06], e [64.97, -25.16, -32.89]

**Pleochroism check.** Weak: o-ray near-colorless greenish, e-ray blue; the blue still sits in the e-ray.

### morganite-pink

Keys: `morganite.pink`. Label: Pink. Estimated (`estimated = true`).

**Sources.**

* Built from the bands of a different beryl (red beryl GRR 602) and published positions; strength estimated.
* no digital morganite spectrum exists (Caltech GRR 508 and 2536b are figures). Mn3+ band shapes (visible bands only, no UV edge) from the measured red beryl GRR 602 (variant_red_beryl_grr602_*.csv); Mn3+ band at 560-567 nm from the Caltech GRR 2536b page text and GIA Gems & Gemology Fall 2024 GNI
* A trace of the heliodor Fe3+ edge (GRR 309) is added to the o-ray so the hue is pink rather than purple; the e-ray keeps only the Mn3+ band and the UV edge (the heliodor 404 nm band turned it peach, the documented e-ray is light violet). The color is defined relative to the c axis (o = E perpendicular to c, e = E parallel to c): the stone's own c axis decides the face-up look

**Fit.** `redberyl_o`, `redberyl_e`, `helio309_o`, `helio309_e` (see Fits above).

**Scaling.** Mn3+ bands of red beryl scaled to a pastel, plus a trace of the Fe3+ part in the o-ray (UV edge and 848 nm band of the heliodor fit, factor 0.0876). The e-ray keeps only the Mn3+ 544 nm band, the heliodor UV edge and its 903 nm band: the heliodor 404 nm Fe3+ band that used to sit in the e-ray turned it peach (L* 85.7 C* 10.8 h 10) where the documented e-ray of morganite is light violet (now L* 85.9, C* 17.8, h 332). Estimated: band position from literature, polarization and widths from a different beryl.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 295.5 | 4533.9 | 10677 | 0.1752 |
  | 495.9 | 1358.1 | 3198 | 0.03065 |
  | 554.6 | 830.3 | 1955 | 0.02751 |
  | 847.7 | 500.0 | 1177 | 0.0007145 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 315.5 | 5800.0 | 13658 | 0.07796 |
  | 543.6 | 1480.8 | 3487 | 0.04768 |
  | 902.7 | 1500.0 | 3532 | 0.01331 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 88, C* 14, h 352
* reached face-up (oriented, the renderer's color; pinned): L* 90.6, C* 16.5, h 349 (unpolarised mean, the fit target: L* 89.0, C* 16.7, h 343)
* o-ray / alpha: L* 90.6, C* 16.5, h 349; e-ray / gamma: L* 85.9, C* 17.8, h 332
* Lab (pinned by the test): face [90.55, 16.15, -3.28], o [90.55, 16.15, -3.28], e [85.87, 15.77, -8.25]

**Pleochroism check.** Weak, as in the beryl reference (light red to light violet): o-ray pink, e-ray slightly more orange.

### morganite-peach

Keys: `morganite.peach`. Label: Peach. Estimated (`estimated = true`).

**Sources.**

* Red beryl bands (GRR 602) plus an Fe3+ part (GRR 309), both at chosen strengths.
* Mn3+ bands of red beryl GRR 602 (visible bands only) plus the Fe3+ part of heliodor GRR 309; GIA Gems & Gemology Fall 2024 GNI (iron-bearing Mn beryl turns orangey pink from an added charge-transfer band)
* The color is defined relative to the c axis (o = E perpendicular to c, e = E parallel to c): the stone's own c axis decides the face-up look

**Fit.** `redberyl_o`, `redberyl_e`, `helio309_o`, `helio309_e` (see Fits above).

**Scaling.** Mn3+ bands and a stronger Fe3+ part, chosen for an orangy pink (h 40). Estimated.
Strength factors applied to the fitted bands (in the order of the parts of the fit script): 0.1103, 0.31.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 295.5 | 4533.9 | 10677 | 0.6199 |
  | 495.9 | 1358.1 | 3198 | 0.03642 |
  | 554.6 | 830.3 | 1955 | 0.03268 |
  | 847.7 | 500.0 | 1177 | 0.002528 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 315.5 | 5800.0 | 13658 | 0.2758 |
  | 403.9 | 1400.4 | 3298 | 0.2911 |
  | 543.6 | 1480.8 | 3487 | 0.05665 |
  | 902.7 | 1500.0 | 3532 | 0.04707 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 86, C* 18, h 40
* reached face-up (oriented, the renderer's color; pinned): L* 88.4, C* 16.6, h 17 (unpolarised mean, the fit target: L* 85.5, C* 17.6, h 40)
* o-ray / alpha: L* 88.4, C* 16.6, h 17; e-ray / gamma: L* 79.2, C* 30.8, h 76
* Lab (pinned by the test): face [88.41, 15.87, 4.87], o [88.41, 15.87, 4.87], e [79.18, 7.70, 29.86]

**Pleochroism check.** Weak: o-ray pink-orange, e-ray yellower, from the polarization of the Fe3+ part.

### topaz-imperial

Keys: `topaz.imperial`. Label: Imperial. Estimated (`estimated = true`).

**Sources.**

* Two measured topaz spectra (Cr3+ pink, brown GRR 515a) combined at chosen strengths; no imperial spectrum exists.
* Estimated combination of two measured parts: Cr3+ from the Ural pink topaz (variant_topaz_pink_cr_ural_bobrovka_*.csv) and the brown color center of GRR 515 (variant_topaz_brown_natural_grr515_*.csv); no imperial topaz spectrum exists on the Caltech server
* The cause is disputed (Cr3+ plus color centers versus Cr4+, RSC Advances 2025, article d5ra02029h, https://pubs.rsc.org/en/content/articlehtml/2025/ra/d5ra02029h (Cr3+ gives pink to purple, Cr4+ red-orange)); modelled as Cr3+ plus a brown center. GIA: medium reddish orange to orange-red

**Fit.** `pink_a`, `pink_b`, `pink_g`, `brown_a`, `brown_b` (see Fits above).

**Scaling.** Cr part (about 0.0065 wt% Cr2O3 equivalent) and brown part (gamma = beta as for Sherry) scaled to a reddish orange (L* 57, h 51). Estimated.
Strength factors applied to the fitted bands (in the order of the parts of the fit script): 0.01801, 1.471.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 312.8 | 5800.0 | 13658 | 0.3827 |
  | 340.0 | 3500.0 | 8242 | 0.02404 |
  | 405.4 | 2194.5 | 5168 | 0.06823 |
  | 478.0 | 3069.1 | 7227 | 0.1771 |
  | 559.1 | 1393.8 | 3282 | 0.03469 |
  | 900.0 | 4000.0 | 9419 | 0.004463 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 288.3 | 3500.1 | 8242 | 0.1801 |
  | 340.0 | 5313.7 | 12513 | 0.1687 |
  | 407.3 | 2257.7 | 5316 | 0.03726 |
  | 472.9 | 3005.1 | 7076 | 0.1579 |
  | 550.4 | 1427.6 | 3362 | 0.04895 |

* beta_ray:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 285.0 | 3500.0 | 8242 | 0.03638 |
  | 340.0 | 5313.7 | 12513 | 0.1687 |
  | 394.5 | 2242.1 | 5280 | 0.05115 |
  | 472.9 | 3005.1 | 7076 | 0.1579 |
  | 538.8 | 1348.0 | 3174 | 0.05718 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 57, C* 48, h 51
* reached face-up (oriented, the renderer's color; pinned): L* 56.4, C* 49.2, h 52 (unpolarised mean, the fit target: L* 57.0, C* 47.9, h 51)
* o-ray / alpha: L* 54.5, C* 50.7, h 58; e-ray / gamma: L* 58.1, C* 45.6, h 50; beta: L* 58.2, C* 48.7, h 46
* Lab (pinned by the test): face [56.41, 30.42, 38.61], o [54.48, 26.88, 43.00], e [58.09, 29.50, 34.75], beta [58.25, 33.61, 35.24]

**Pleochroism check.** Medium orange to light orange pleochroism (published); the three rays differ by about 12 degrees of hue (alpha h 58, gamma h 50, beta h 46).

### topaz-sherry

Keys: `topaz.sherry`. Label: Sherry. Estimated (`estimated = true`).

**Sources.**

* Strength chosen for a typical face-up color; spectrum measured (GRR 515a, gamma taken equal to beta).
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: topaz GRR 515a, natural brown, Thomas Mountains, Utah, 7.00 mm, E parallel alpha and beta only (variant_topaz_brown_natural_grr515_{alpha,beta}.csv)
* Unstable hole-type color centers, brown (bleach below 250 C or in sunlight); gamma has no data and is set equal to beta (a documented estimate for that ray)

**Fit.** `brown_a`, `brown_b` (see Fits above).

**Scaling.** All bands of the 7 mm crystal scaled to a medium yellowish brown stone (L* 71). Gamma = beta (no gamma file).
Strength factors applied to the fitted bands (in the order of the parts of the fit script): 1.154.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 312.8 | 5800.0 | 13658 | 0.3001 |
  | 478.0 | 3069.1 | 7227 | 0.1388 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 340.0 | 5313.7 | 12513 | 0.1323 |
  | 472.9 | 3005.1 | 7076 | 0.1238 |

* beta_ray:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 340.0 | 5313.7 | 12513 | 0.1323 |
  | 472.9 | 3005.1 | 7076 | 0.1238 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 71, C* 41, h 63
* reached face-up (oriented, the renderer's color; pinned): L* 70.3, C* 41.9, h 63 (unpolarised mean, the fit target: L* 71.1, C* 41.3, h 63)
* o-ray / alpha: L* 67.7, C* 43.7, h 62; e-ray / gamma: L* 72.7, C* 40.4, h 63; beta: L* 72.7, C* 40.4, h 63
* Lab (pinned by the test): face [70.27, 19.18, 37.23], o [67.67, 20.28, 38.74], e [72.73, 18.22, 36.05], beta [72.73, 18.22, 36.05]

**Pleochroism check.** Almost no pleochroism in the data (alpha and beta both h 62-63).

### topaz-pink

Keys: `topaz.pink`. Label: Pink. Measured shape and anchored strength (`estimated = false`).

**Sources.**

* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: topaz, Bobrovka, Ural (M. N. Taran), 3.0 mm, Cr2O3 0.36, FeO 0.03, MnO 0.02 wt%, E parallel alpha / beta / gamma (variant_topaz_pink_cr_ural_bobrovka_{alpha,beta,gamma}.csv)
* Cr3+ 4T2 near 530-560 nm and 4T1 near 390-410 nm; Katlang pink topaz has Cr2O3 0.01-0.03 wt% (Gubelin et al. 1986, G&G 22(3)): the crystal is 12-36 times stronger than a pink gem

**Fit.** `pink_a`, `pink_b`, `pink_g` (see Fits above).

**Scaling.** All bands scaled by 0.043 (equivalent to about 0.015 wt% Cr2O3, inside the Katlang range 0.01-0.03) to a pink stone (L* 73).
Strength factors applied to the fitted bands (in the order of the parts of the fit script): 0.04306.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 340.0 | 3500.0 | 8242 | 0.05749 |
  | 405.4 | 2194.5 | 5168 | 0.1632 |
  | 559.1 | 1393.8 | 3282 | 0.08296 |
  | 900.0 | 4000.0 | 9419 | 0.01067 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 288.3 | 3500.1 | 8242 | 0.4306 |
  | 407.3 | 2257.7 | 5316 | 0.08911 |
  | 550.4 | 1427.6 | 3362 | 0.1171 |

* beta_ray:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 285.0 | 3500.0 | 8242 | 0.08701 |
  | 394.5 | 2242.1 | 5280 | 0.1223 |
  | 538.8 | 1348.0 | 3174 | 0.1368 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 74, C* 24, h 345
* reached face-up (oriented, the renderer's color; pinned): L* 74.0, C* 20.1, h 352 (unpolarised mean, the fit target: L* 73.2, C* 22.3, h 343)
* o-ray / alpha: L* 76.9, C* 10.3, h 75; e-ray / gamma: L* 71.6, C* 28.0, h 330; beta: L* 71.0, C* 40.8, h 338
* Lab (pinned by the test): face [74.02, 19.92, -2.74], o [76.87, 2.70, 9.96], e [71.64, 24.11, -14.14], beta [70.98, 37.87, -15.07]

**Pleochroism check.** Beta the strongest pink ray, gamma pink-violet, alpha the weak yellowish ray, as GIA describes.

### topaz-sky

Keys: `topaz.sky`. Label: Sky. Estimated (`estimated = true`).

**Sources.**

* Strength chosen for a typical face-up color; spectrum measured (GIA 3828).
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: GIA 3828, natural blue topaz, Zimbabwe, E parallel alpha 13.31 mm, beta 13.31 mm, gamma 9.28 mm (variant_topaz_blue_natural_gia3828_{alpha,beta,gamma}.csv)
* O- hole color center near 660 nm (RSC Advances 2025, article d5ra02029h, https://pubs.rsc.org/en/content/articlehtml/2025/ra/d5ra02029h: the O- concentration follows the 620 nm absorption); blue absorbs most along alpha (E parallel a), beta nearly zero (R2 section 0.2 item 2); the header thickness was used (the index page plots another)
* Gamma ray refitted 2026-10-27 with three bands (edge, 410 nm, 630 nm; the old two-band fit had the 400-450 nm absorption 4 times too weak); ONE factor 1.5 over every band

**Fit.** Alpha and beta as before (blue3828_a: edge 298.7 nm, 442 nm, 660.6 nm; blue3828_b: edge only). Gamma refitted with three bands (285 nm edge, 412 nm, 634 nm) to the GIA 3828 gamma trace, window 280-1200 nm (visible weight 3), constant limited to -0.006 .. 0.006 mm^-1 as for the other baseline-corrected traces (fitted -0.006): RMS 0.0081 mm^-1, model + constant within 0.001 mm^-1 of the data at 400-750 nm. The previous two-band gamma fit had the 400-450 nm absorption 4 times too weak (0.004 against 0.016 mm^-1), which moved the face-up hue from the measured 207-213 to 248.

**Scaling.** ONE factor 1.5 over every band, the edge included (owner rule; the earlier 1.78 / 1.5 split). Chosen for a light sky blue: k 1.0 gives face-up L* 87.0 C* 8.6 h 221, k 1.5 83.4 / 8.5 / 223, k 2.0 81.1 / 7.4 / 225 (alpha ray at k 1.5: L* 58, C* 30, h 220). Sky, Swiss and London are trade names of strength, not spectral classes.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 298.7 | 3500.0 | 8242 | 0.3824 |
  | 442.1 | 2971.0 | 6996 | 0.09207 |
  | 660.6 | 2260.0 | 5322 | 0.2445 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 285.0 | 3833.0 | 9026 | 0.2812 |
  | 412.0 | 3500.0 | 8242 | 0.02655 |
  | 633.7 | 2223.0 | 5235 | 0.0255 |

* beta_ray:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 285.0 | 3500.0 | 8242 | 0.3232 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 85, C* 8, h 248
* reached face-up (oriented, the renderer's color; pinned): L* 83.4, C* 8.5, h 223 (unpolarised mean, the fit target: L* 86.5, C* 6.5, h 211)
* o-ray / alpha: L* 58.0, C* 30.3, h 220; e-ray / gamma: L* 92.1, C* 4.7, h 167; beta: L* 100.0, C* 0.4, h 117
* Lab (pinned by the test): face [83.43, -6.18, -5.85], o [58.02, -23.21, -19.53], e [92.10, -4.58, 1.09], beta [99.99, -0.17, 0.34]

**Pleochroism check.** Alpha the blue ray (L* 58.0, C* 30.3, h 220), gamma very pale (L* 92.1, C* 4.7), beta colorless.

### topaz-swiss

Keys: `topaz.swiss`. Label: Swiss. Estimated (`estimated = true`).

**Sources.**

* Strength chosen for a typical face-up color; spectrum measured (GRR 1632).
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: topaz GRR 1632, neutron-irradiated blue, Nigeria, 3.462 mm, three rays (variant_topaz_blue_irradiated_grr1632_{alpha,beta,gamma}.csv)
* Refitted 2026-10-27 as London (the three-ray fit of the Topaz Material Default); ONE factor 0.46 over every band. Swiss is a trade shade between the natural Sky and the irradiated London strength

**Fit.** The three-ray fit of GRR 1632 that the Topaz Material Default carries (lane B1: alpha 4 bands, beta 3, gamma 4; model / data 0.97-1.02 on alpha and gamma over 400-750 nm, beta 0.96-1.16 over 400-700 nm), converted from model units to per mm (peak / (0.45 x 9.905)). The two-band beta fit of lane F2 had put beta at h 284 (violet) where the data are h 204 (near-neutral teal): beta 0.55-0.64 and 1.4-2.2 times the data at 450 and 700 nm.

**Scaling.** ONE factor 0.46 over every band (the earlier 0.46 / 0.50 split), chosen for a medium blue; Swiss is a trade shade between the natural Sky and the irradiated London strength, so the strength is estimated.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 322.6 | 1992.1 | 4691 | 0.7245 |
  | 424.6 | 2156.9 | 5079 | 0.1837 |
  | 622.1 | 2093.6 | 4930 | 0.5088 |
  | 719.2 | 1537.3 | 3620 | 0.2838 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 300.0 | 2945.9 | 6937 | 0.8081 |
  | 424.0 | 2210.8 | 5206 | 0.0645 |
  | 617.6 | 2272.8 | 5352 | 0.1042 |
  | 723.3 | 1596.3 | 3759 | 0.0483 |

* beta_ray:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 300.0 | 2700.0 | 6358 | 0.71 |
  | 400.0 | 3761.6 | 8858 | 0.02157 |
  | 603.9 | 1685.1 | 3968 | 0.02312 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 72, C* 13, h 236
* reached face-up (oriented, the renderer's color; pinned): L* 72.9, C* 10.7, h 239 (unpolarised mean, the fit target: L* 73.2, C* 14.0, h 227)
* o-ray / alpha: L* 32.4, C* 43.1, h 241; e-ray / gamma: L* 73.7, C* 21.7, h 215; beta: L* 93.0, C* 3.9, h 205
* Lab (pinned by the test): face [72.94, -5.47, -9.18], o [32.36, -20.62, -37.81], e [73.67, -17.86, -12.33], beta [92.97, -3.54, -1.63]

**Pleochroism check.** Alpha deep blue (L* 32.4, C* 43.1, h 241), gamma lighter teal (L* 73.7, C* 21.7, h 215), beta nearly colorless (L* 93.0, C* 3.9, h 205).

### topaz-london

Keys: `topaz.london`. Label: London. Estimated (`estimated = true`).

**Sources.**

* Strength chosen for a typical face-up color; spectrum measured (GRR 1632).
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: topaz GRR 1632, neutron-irradiated blue, Nigeria, 3.462 mm, three rays (variant_topaz_blue_irradiated_grr1632_{alpha,beta,gamma}.csv)
* Refitted 2026-10-27: the three-ray fit of the Topaz Material Default (4 / 3 / 4 bands, model / data 1.00 +- 3 percent on alpha and gamma, beta within 5 percent from 400 to 650 nm); ONE factor 0.85 over every band
* O- hole color center near 620-720 nm; London blue is a deep, steely blue at high dose

**Fit.** As Swiss: the B1 three-ray fit of GRR 1632, converted to per mm; the beta ray is the measured near-neutral teal (L* 87, C* 7, h 204), not the earlier fit artefact (pale violet, h 284), so the record's old "beta pale violet" pleochroism line is withdrawn.

**Scaling.** ONE factor 0.85 over every band (the earlier 0.85 / 0.89 split). Chosen for a deep steel blue; the alpha ray is nearly opaque over a 9.9 mm path (L* 13).

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 322.6 | 1992.1 | 4691 | 1.339 |
  | 424.6 | 2156.9 | 5079 | 0.3394 |
  | 622.1 | 2093.6 | 4930 | 0.9402 |
  | 719.2 | 1537.3 | 3620 | 0.5244 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 300.0 | 2945.9 | 6937 | 1.493 |
  | 424.0 | 2210.8 | 5206 | 0.1192 |
  | 617.6 | 2272.8 | 5352 | 0.1926 |
  | 723.3 | 1596.3 | 3759 | 0.08925 |

* beta_ray:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 300.0 | 2700.0 | 6358 | 1.312 |
  | 400.0 | 3761.6 | 8858 | 0.03986 |
  | 603.9 | 1685.1 | 3968 | 0.04272 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 62, C* 13, h 244
* reached face-up (oriented, the renderer's color; pinned): L* 66.7, C* 7.6, h 223 (unpolarised mean, the fit target: L* 63.8, C* 14.0, h 219)
* o-ray / alpha: L* 13.3, C* 31.5, h 251; e-ray / gamma: L* 57.2, C* 31.0, h 214; beta: L* 87.4, C* 6.9, h 204
* Lab (pinned by the test): face [66.69, -5.52, -5.23], o [13.30, -10.37, -29.75], e [57.21, -25.78, -17.26], beta [87.41, -6.27, -2.82]

**Pleochroism check.** Alpha very dark blue (L* 13.3, C* 31.5, h 251), gamma teal (L* 57.2, C* 31.0, h 214), beta pale teal (L* 87.4, C* 6.9, h 204); the measured beta ray is near-neutral, not violet.

### topaz-colorless

Keys: `topaz.colorless`. Label: Colorless. Estimated (`estimated = true`).

**Sources.**

* Colorless: no visible absorption assumed; host edge near 220 nm.
* Colorless topaz: host edge near 220 nm, no absorption in 380-780 nm (repo Report)

**Fit.** None (no spectrum to fit); see sources.

**Scaling.** Empty alpha, beta and gamma sets with pleochroic = true.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  none (colorless)

* e_ray / gamma:

  none (colorless)

* beta_ray:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 100, C* 0, h 0
* reached face-up (oriented, the renderer's color; pinned): L* 100.0, C* 0.0, h 0 (unpolarised mean, the fit target: L* 100.0, C* 0.0, h 0)
* o-ray / alpha: L* 100.0, C* 0.0, h 0; e-ray / gamma: L* 100.0, C* 0.0, h 0; beta: L* 100.0, C* 0.0, h 0
* Lab (pinned by the test): face [100.00, 0.00, 0.00], o [100.00, 0.00, 0.00], e [100.00, 0.00, 0.00], beta [100.00, 0.00, 0.00]

**Pleochroism check.** Colorless.

### tourmaline-rubellite

Keys: `tourmaline.rubellite`. Label: Rubellite. Estimated (`estimated = true`).

**Sources.**

* Strength chosen for a typical face-up color; spectrum measured (GRR 1368).
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: elbaite GRR 1368, dark pink, Otjimbinque, Namibia, 2.188 mm (reference_spectra/variant_tourmaline_rubellite_grr1368_{eperpc,eparc}.csv)
* Mn3+ (5E to 5T2) near 540 nm; GIA tourmaline description
* Single factor 0.8 over every fitted band, the edge included (2026-10-27, owner rule; the earlier 0.79 / 1.04 split differed by 1.3 times)

**Fit.** `rubellite_o`, `rubellite_e` (see Fits above).

**Scaling.** ONE factor 0.8 over every fitted band, the UV edge included (owner rule; the earlier 0.79 on the chromophore bands and 1.036 on the edge differed by 1.3 times and gave face-up L* 45.6, within dE 2 of this). Chosen for a red stone (L* 45). The NIR rise of the o-ray is fitted as one band at 1028 nm and a free baseline.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 285.0 | 4209.0 | 9911 | 1.542 |
  | 453.6 | 2460.0 | 5793 | 0.2024 |
  | 542.2 | 1119.0 | 2635 | 0.4552 |
  | 1028.3 | 1426.0 | 3358 | 0.103 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 307.4 | 3500.0 | 8242 | 0.3925 |
  | 479.6 | 2919.0 | 6874 | 0.2144 |
  | 539.3 | 883.0 | 2079 | 0.1544 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 44, C* 66, h 4
* reached face-up (oriented, the renderer's color; pinned): L* 45.3, C* 62.4, h 4 (unpolarised mean, the fit target: L* 43.7, C* 65.2, h 3)
* o-ray / alpha: L* 40.0, C* 71.9, h 0; e-ray / gamma: L* 49.9, C* 55.6, h 9
* Lab (pinned by the test): face [45.34, 62.24, 4.77], o [39.98, 71.88, 0.26], e [49.90, 55.03, 8.30]

**Pleochroism check.** o-ray the deeper pink-red, e-ray paler and slightly more orange.

### tourmaline-pink

Keys: `tourmaline.pink`. Label: Pink. Estimated (`estimated = true`).

**Sources.**

* Measured rubellite spectrum (GRR 1368) scaled down to a light pink; no light pink spectrum exists.
* the measured rubellite spectrum (GRR 1368, as in tourmaline.rubellite) scaled down to a light pink; no light pink spectrum is available as data (Caltech GRR 565 natural is a figure)
* Mn3+, often radiation induced (Caltech tourmaline index page)

**Fit.** `rubellite_o`, `rubellite_e` (see Fits above).

**Scaling.** Chromophore bands and edge of the rubellite crystal scaled down to a light pink (L* 79). Estimated strength.
Strength factors applied to the fitted bands (in the order of the parts of the fit script): 0.1767, 0.6866.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 285.0 | 4209.2 | 9912 | 1.324 |
  | 453.6 | 2459.7 | 5792 | 0.0447 |
  | 542.2 | 1119.5 | 2636 | 0.1005 |
  | 1028.3 | 1426.2 | 3358 | 0.02274 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 307.4 | 3500.0 | 8242 | 0.3369 |
  | 479.6 | 2918.8 | 6873 | 0.04735 |
  | 539.3 | 883.2 | 2080 | 0.0341 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 78, C* 28, h 353
* reached face-up (oriented, the renderer's color; pinned): L* 80.0, C* 26.7, h 356 (unpolarised mean, the fit target: L* 78.7, C* 29.2, h 354)
* o-ray / alpha: L* 75.8, C* 34.6, h 350; e-ray / gamma: L* 83.9, C* 20.2, h 4
* Lab (pinned by the test): face [80.04, 26.66, -1.96], o [75.79, 34.12, -5.80], e [83.95, 20.18, 1.43]

**Pleochroism check.** o-ray pink, e-ray paler.

### tourmaline-indicolite

Keys: `tourmaline.indicolite`. Label: Indicolite. Estimated (`estimated = true`).

**Sources.**

* Strength chosen for a typical face-up color; spectrum measured (GRR 3770).
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: elbaite GRR 3770, pure blue, 2.00 mm (variant_tourmaline_indicolite_grr3770_{eperpc,eparc}.csv), checked against GRR 1935 blue, Usakos, 3.641 mm (variant_tourmaline_indicolite_grr1935_blue_*.csv)
* Refitted 2026-10-27 (UV edge, a weak 640 nm shoulder, Fe2+ 720 nm, near-infrared Fe2+ band per ray) with the visible floor (A 0.394 o / 0.341 e) removed; ONE factor 3.0 over every band, the edge included (owner rule). The measured crystal is a teal green (face-up hue about 155), greener than the trade name suggests; the factor gives a saleable face-up lightness
* Fe2+ at 713-720 nm and 1120 nm; the Fe2+ band is almost unpolarized in both crystals (o:e 0.95:0.94 and 0.91:0.98), unlike the 3:1 of the Tourmaline built-in (R2 section 0.2). The o and e sets are stored separately (pleochroic) but nearly equal in the Fe2+ band

**Fit.** Per ray, three energy Gaussians (UV edge, Fe2+ 720 nm, near-infrared Fe2+ 1160 nm) plus a weak 640 nm shoulder, bounded least squares on 380-1250 nm (visible weight 3) with the visible FLOOR removed (A 0.394 o / 0.341 e, the minimum of the 7-point mean over 440-620 nm; the free constant is fixed at 0): o-ray RMS 0.019 mm^-1, e-ray 0.013. The file is a merge scaled by the contributor, so its absolute pedestal (A about 0.4 at 2 mm) is not a chromophore absorption; the earlier fit removed only 0.346 mm^-1 and left 0.12 mm^-1 of grey absorption across the visible. Model / data 0.5-2 at 400-550 nm (all below 0.15 mm^-1) and 0.95-1.03 at 650-750 nm.

**Scaling.** ONE factor 3.0 over every band, the UV edge included (owner rule 2026-10-27; the earlier 4.19 on the Fe2+ bands and 0.56 on the edge differed by 7.5 times and was not the measured crystal at any strength). The factor is chosen for a saleable face-up lightness: k 1 gives L* 91 C* 42 h 164, k 2 86 / 63 / 158, k 3 82 / 76 / 154, k 5 75 / 91 / 148. **The measured crystal is a teal green (face-up hue 150-165 at every strength), greener than the trade name suggests**; the visible window is wide (450-600 nm) and the Fe2+ band only closes the red.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 285.0 | 5121.5 | 12060 | 3.082 |
  | 640.0 | 800.0 | 1884 | 0.3442 |
  | 720.0 | 907.0 | 2136 | 2.503 |
  | 1161.9 | 700.7 | 1650 | 0.6144 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 353.7 | 3876.5 | 9128 | 0.4812 |
  | 640.0 | 800.0 | 1884 | 0.2272 |
  | 721.2 | 979.2 | 2306 | 2.674 |
  | 1185.6 | 891.1 | 2098 | 0.9477 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 80, C* 54, h 171
* reached face-up (oriented, the renderer's color; pinned): L* 82.0, C* 75.9, h 154 (unpolarised mean, the fit target: L* 82.1, C* 76.4, h 154)
* o-ray / alpha: L* 82.1, C* 77.6, h 156; e-ray / gamma: L* 81.9, C* 74.4, h 151
* Lab (pinned by the test): face [82.02, -68.05, 33.61], o [82.13, -70.83, 31.69], e [81.91, -65.29, 35.60]

**Pleochroism check.** Both rays teal green and nearly equal (o L* 82.1 C* 77.6 h 156, e L* 81.9 C* 74.4 h 151): the Fe2+ band is almost unpolarized in the measured crystals (R2 section 0.2).

### tourmaline-verdelite

Keys: `tourmaline.verdelite`. Label: Verdelite. Estimated (`estimated = true`).

**Sources.**

* Strength chosen for a typical face-up color; spectrum measured (GRR 1935 green).
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: elbaite GRR 1935, small green crystal, Usakos, Namibia, 3.645 mm, baseline corrected (variant_tourmaline_verdelite_grr1935_green_{eperpc,eparc}.csv)
* Refitted 2026-10-27 with four bands per ray (UV edge, 525-620 nm shoulder, Fe2+ 711 nm, near-infrared Fe2+), the fit of the Tourmaline Material Default (model / data 0.94-1.03 at 550-750 nm); ONE factor 1.4 over every band. The earlier yellow-green hue (h 136) is not reachable from this crystal: its face-up hue is about 154
* Fe2+ near 712 nm plus Fe2+-Ti4+ charge transfer and UV edge absorbing blue (Caltech tourmaline index page)

**Fit.** The four-band fit of the same crystal that the Tourmaline Material Default carries (UV edge 300 nm, a weak 525-620 nm shoulder, Fe2+ 711 / 709 nm, near-infrared Fe2+ 1156 / 1180 nm; Caltech GRR 1935 green, 3.645 mm, per-ray baselines 0.109 / 0.122 A per mm removed, rms 0.014 / 0.009 mm^-1), converted from model units to per mm (peak / (0.34 x 9.905)). Model / data over 400-750 nm: o 0.73-1.23 (1.02 at 400 nm, 0.99 at 450, 0.73 at 500, 0.97-1.23 at 550-600, 0.94-1.03 at 620-750), e 0.79-1.27. The three-band fit it replaces had no 560-620 nm absorption (0.05-0.10 against the measured 0.14-0.22 mm^-1 at 2.7 x).

**Scaling.** ONE factor 1.4 over every band, the UV edge included (owner rule; the earlier 2.70 / 2.30 split was nearly one factor but the 3-band shape was wrong). Chosen for a medium-light green: k 1.0 gives face-up L* 79.6 C* 33.5 h 153, k 1.4 73.2 / 37.7 / 154, k 2.0 64.9 / 40.3 / 155. **The earlier yellow-green hue (h 136) is not reachable from this crystal**: its face-up hue is 153-156 at every strength (the o-ray alone is h 142, the e-ray h 169).

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 300.0 | 5171.5 | 12178 | 2.494 |
  | 620.0 | 1482.1 | 3490 | 0.128 |
  | 711.4 | 928.3 | 2186 | 0.7233 |
  | 1156.1 | 1061.7 | 2500 | 0.2977 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 300.0 | 3710.7 | 8738 | 2.981 |
  | 524.8 | 3111.1 | 7326 | 0.04947 |
  | 708.9 | 1027.3 | 2419 | 0.6568 |
  | 1180.0 | 1061.7 | 2500 | 0.1742 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 78, C* 60, h 136
* reached face-up (oriented, the renderer's color; pinned): L* 73.2, C* 37.7, h 154 (unpolarised mean, the fit target: L* 70.9, C* 44.1, h 150)
* o-ray / alpha: L* 65.8, C* 62.8, h 142; e-ray / gamma: L* 79.5, C* 24.4, h 169
* Lab (pinned by the test): face [73.21, -33.90, 16.49], o [65.83, -49.49, 38.64], e [79.54, -23.98, 4.62]

**Pleochroism check.** The o-ray is the deeper, yellower green (L* 65.8, C* 62.8, h 142), the e-ray paler and bluer (L* 79.5, C* 24.4, h 169): the edge is polarised o about 2 x e (the Fe2+ band o : e 1.16).

### tourmaline-chrome

Keys: `tourmaline.chrome`. Label: Chrome. Estimated (`estimated = true`).

**Sources.**

* Built from published band positions; strength estimated.
* no Chrome tourmaline spectrum is available as data (Caltech GRR 2128, 2467, 1719, 768 are figures). Cr3+ / V3+ bands at 432 nm and 607 nm (green Brumado uvite, 16 400 and 23 140 cm^-1) with the polarization e = 0.4 o (Amstall tourmaline Cr epsilon 39.7 / 15.9)
* Band widths and strengths from docs/chromophores/beryl_topaz_tourmaline_andalusite.md (Report p.22, secondary): Cr2O3 0.2 wt%; GIA: intense green colored mostly by vanadium

**Fit.** None (no spectrum to fit); see sources.

**Scaling.** Two bands per ray from the research document (FWHM 3200 / 2800 cm^-1, sigma 1360 / 1190 cm^-1) scaled to an intense green (L* 60). Estimated.
Strength factors applied to the fitted bands (in the order of the parts of the fit script): 1.421.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 432.0 | 1360.0 | 3203 | 0.5115 |
  | 607.0 | 1190.0 | 2802 | 0.5683 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 432.0 | 1360.0 | 3203 | 0.2046 |
  | 607.0 | 1190.0 | 2802 | 0.2273 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 60, C* 90, h 169
* reached face-up (oriented, the renderer's color; pinned): L* 63.0, C* 83.1, h 170 (unpolarised mean, the fit target: L* 60.0, C* 89.5, h 169)
* o-ray / alpha: L* 52.9, C* 113.0, h 169; e-ray / gamma: L* 71.1, C* 71.7, h 170
* Lab (pinned by the test): face [63.05, -81.73, 15.02], o [52.87, -111.05, 21.19], e [71.12, -70.67, 12.03]

**Pleochroism check.** o-ray the deeper green, e-ray lighter, both about h 170.

### tourmaline-paraiba

Keys: `tourmaline.paraiba`. Label: Paraiba. Estimated (`estimated = true`).

**Sources.**

* Strength chosen for a typical face-up color; spectrum measured (GIA R030, a thin Cu-rich section scaled down).
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57 and Rossman, Fritsch and Shigley 1991, Am. Mineral. 76:1479: GIA R030, Sao Jose da Batalha, 0.200 mm thin section (variant_tourmaline_paraiba_r030_{eperpc,eparc}.csv)
* Cu2+ (about 700 nm and 900-950 nm, strongest E parallel c; Merkel and Breeding 2009, G&G 45(2)); the o-ray is baseline corrected, the e-ray is not (pedestal fitted and removed)
* Single factor 0.31 over every fitted band, the edge included (2026-10-27, owner rule; the earlier 0.31 / 0.32 split was the same within 4 percent). The e-ray plateau (0.4 mm^-1 raw, visible 700-1000 nm) is deconvolved into three bands and a 1100 nm plateau; the thin section's fit RMS (0.47 mm^-1) equals the visible signal there, so the e-ray is the least certain part

**Fit.** `paraiba_o`, `paraiba_e` (see Fits above).

**Scaling.** ONE factor 0.31 over every fitted band, the edge included (owner rule; the earlier 0.307 / 0.318 split was the same within 4 percent). The thin section is far too Cu-rich for a stone: the bands of the 0.2 mm section are scaled to about one third for a neon greenish blue (L* 78). **The e-ray plateau is an estimate**: its visible 700-1000 nm absorption (0.4 mm^-1 raw, 0.12 at the factor, optical density 1.2 over the 9.9 mm path) is only partly described by the fit (RMS 0.47 mm^-1 equals the visible signal; data e-ray at the factor L* 52 C* 63 against the model's 73 / 91), so the e-ray, not the o-ray, carries the uncertainty.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 293.9 | 3695.0 | 8701 | 1.51 |
  | 718.7 | 1500.0 | 3532 | 1.109 |
  | 907.0 | 786.0 | 1851 | 1.941 |
  | 1100.0 | 1500.0 | 3532 | 0.3819 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 312.1 | 3500.0 | 8242 | 3.72 |
  | 704.0 | 1377.0 | 3243 | 3.435 |
  | 763.1 | 700.0 | 1648 | 0.9185 |
  | 955.8 | 1390.0 | 3273 | 4.851 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 80, C* 66, h 195
* reached face-up (oriented, the renderer's color; pinned): L* 78.4, C* 71.3, h 193 (unpolarised mean, the fit target: L* 80.1, C* 66.9, h 195)
* o-ray / alpha: L* 83.5, C* 60.0, h 201; e-ray / gamma: L* 72.5, C* 90.7, h 185
* Lab (pinned by the test): face [78.35, -69.57, -15.73], o [83.52, -55.99, -21.46], e [72.55, -90.29, -8.21]

**Pleochroism check.** Cu2+ strongest in the e-ray against the o-ray, as published.

### tourmaline-paraiba-violet

Keys: `tourmaline.paraiba_violet`. Label: Paraiba Violet. Estimated (`estimated = true`).

**Sources.**

* Strength chosen for a typical face-up color; spectrum measured (Mn-Cu elbaite).
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: elbaite with Cu and Mn, pink, 1.84 mm (M. N. Taran; variant_tourmaline_paraiba_mn_cu_pink_{eperpc,eparc}.csv)
* Cu2+ plus Mn3+ (about 520 nm): violet Paraiba (GIA: intense violetish blue; Mn3+ gives violet)

**Fit.** `paraibaviolet_o`, `paraibaviolet_e` (see Fits above).

**Scaling.** All bands of the crystal scaled together to a saturated violet-pink face (L* 55). The Mn3+ band in the o-ray is the strongest feature; the e-ray is dark brownish.
Strength factors applied to the fitted bands (in the order of the parts of the fit script): 0.4517.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 285.0 | 3500.0 | 8242 | 0.5682 |
  | 524.1 | 1068.9 | 2517 | 0.3784 |
  | 715.9 | 905.4 | 2132 | 0.1352 |
  | 905.6 | 912.3 | 2148 | 0.4256 |
  | 1100.0 | 1500.0 | 3532 | 0.001978 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 340.0 | 5800.0 | 13658 | 0.1724 |
  | 509.0 | 2600.0 | 6123 | 0.2797 |
  | 717.5 | 1278.6 | 3011 | 0.1966 |
  | 954.2 | 1800.0 | 4239 | 0.2472 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 55, C* 85, h 330
* reached face-up (oriented, the renderer's color; pinned): L* 51.2, C* 74.5, h 332 (unpolarised mean, the fit target: L* 54.9, C* 84.5, h 331)
* o-ray / alpha: L* 61.2, C* 100.4, h 330; e-ray / gamma: L* 36.9, C* 24.9, h 8
* Lab (pinned by the test): face [51.25, 66.06, -34.51], o [61.23, 87.13, -49.92], e [36.86, 24.62, 3.36]

**Pleochroism check.** o-ray violet-pink, e-ray darker and brownish: strongly dichroic.

### tourmaline-yellow

Keys: `tourmaline.yellow`. Label: Yellow. Estimated (`estimated = true`).

**Sources.**

* Strength chosen for a typical face-up color; spectrum measured (GRR 1935 yellow, a very pale crystal scaled up about tenfold).
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: elbaite GRR 1935, very pale yellow, Usakos, 5.353 mm (variant_tourmaline_yellow_grr1935_yellow_{eperpc,eparc}.csv)
* Mn2+-Ti4+ charge transfer (band at 325 nm, Rossman and Mattson 1986, Am. Mineral. 71:599) with a tail into the visible; the polarized bands at 397 nm (o) and 375 nm (e) are not assigned by Caltech

**Fit.** `yellow_o`, `yellow_e` (see Fits above).

**Scaling.** The crystal is very pale: its bands are scaled up about tenfold (a stone richer in Mn and Ti) to a light canary yellow (L* 98.6).
Strength factors applied to the fitted bands (in the order of the parts of the fit script): 10.04.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 386.6 | 1745.1 | 4109 | 4.759 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 367.9 | 1553.2 | 3658 | 3.385 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 96, C* 58, h 107
* reached face-up (oriented, the renderer's color; pinned): L* 98.9, C* 48.1, h 109 (unpolarised mean, the fit target: L* 98.6, C* 59.5, h 108)
* o-ray / alpha: L* 98.0, C* 90.8, h 104; e-ray / gamma: L* 99.8, C* 21.9, h 117
* Lab (pinned by the test): face [98.95, -16.03, 45.40], o [98.03, -22.60, 87.94], e [99.85, -9.84, 19.55]

**Pleochroism check.** The o-ray carries the color, the e-ray is nearly colorless, as the data show (0.25 against 0.05 per mm at 400 nm).

### andalusite-greener

Keys: `andalusite.greener`. Label: Greener. Estimated (`estimated = true`).

**Sources.**

* Measured Fe-Ti spectrum plus an estimated Mn3+ part, both at chosen strengths.
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: andalusite, color-zoned crystal, Brazil, FeO 0.35 wt% (M. N. Taran), light zone, normalized 1.0 cm, E parallel c = alpha, b = beta, a = gamma (variant_andalusite_fe_ti_brazil_light_zone_{alpha,beta,gamma}.csv): Fe2+-Ti4+ in alpha
* Estimated: Mn3+ on the Al1 site, bands at 702, 455 and 426 nm (Halenius 1978, Can. Mineral. 16:567); widths and polarization from the repo Report (secondary); the viridine spectra GRR 305 are not available (404 on the server)

**Fit.** `andfeti_a`, `andfeti_b`, `andfeti_g` (see Fits above).

**Scaling.** Fe-Ti part scaled to a quarter (the 1.0 cm zone is far too dark for a stone) plus the Mn3+ part (about 0.07 wt% Mn2O3) to a yellowish green face (L* 82). Estimated.
Strength factors applied to the fitted bands (in the order of the parts of the fit script): 0.25, 0.7086.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 285.0 | 5011.2 | 11800 | 1.081 |
  | 426.0 | 760.0 | 1790 | 0.03401 |
  | 455.0 | 850.0 | 2002 | 0.04252 |
  | 464.6 | 2041.3 | 4807 | 0.4184 |
  | 510.0 | 1426.4 | 3359 | 0.2298 |
  | 600.0 | 1909.6 | 4497 | 0.1106 |
  | 702.0 | 1060.0 | 2496 | 0.03827 |
  | 730.7 | 2818.1 | 6636 | 0.04316 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 340.0 | 3739.5 | 8806 | 0.08539 |
  | 426.0 | 760.0 | 1790 | 0.06803 |
  | 455.0 | 850.0 | 2002 | 0.08504 |
  | 480.0 | 4500.0 | 10597 | 0.005835 |
  | 702.0 | 1060.0 | 2496 | 0.1276 |

* beta_ray:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 297.8 | 4342.1 | 10225 | 0.75 |
  | 426.0 | 760.0 | 1790 | 0.08504 |
  | 455.0 | 850.0 | 2002 | 0.1063 |
  | 480.0 | 4500.0 | 10597 | 0.01091 |
  | 702.0 | 1060.0 | 2496 | 0.102 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 80, C* 45, h 114
* reached face-up (oriented, the renderer's color; pinned): L* 73.1, C* 46.2, h 106 (unpolarised mean, the fit target: L* 81.5, C* 45.8, h 113)
* o-ray / alpha: L* 31.0, C* 52.2, h 49; e-ray / gamma: L* 95.0, C* 48.3, h 122; beta: L* 93.4, C* 58.0, h 115
* Lab (pinned by the test): face [73.07, -12.45, 44.48], o [31.04, 34.08, 39.47], e [94.95, -25.42, 41.06], beta [93.39, -24.67, 52.46]

**Pleochroism check.** Alpha (E parallel c) brownish orange, beta and gamma pale yellow-green: all the Fe-Ti color is in alpha.

### andalusite-vanadian

Keys: `andalusite.vanadian`. Label: Vanadian. Estimated (`estimated = true`).

**Sources.**

* Strength chosen for a typical face-up color; spectrum measured (Llano, Texas).
* Caltech Mineral Spectroscopy Server (G. R. Rossman), CC BY-NC 4.0, DOI 10.7907/jywr-qq57: andalusite, Llano uplift, Texas, GRR 3467, 0.110 mm, V2O3 about 0.81 and Cr2O3 about 0.09 wt% (Carlson and Rossman 1988, Am. Mineral. 73:1366), alpha / beta / gamma (variant_andalusite_v_cr_llano_{alpha,beta,gamma}.csv)
* V3+ bands at 418 nm (strongest in alpha) and 557-575 nm; the crystal is a reddish-brown thin section, so the scale is set to a trace of V2O3

**Fit.** `andv_a`, `andv_b`, `andv_g` (see Fits above).

**Scaling.** All bands scaled by 0.032 (about 0.026 wt% V2O3 at 20.8 mm^-1 per wt%) to a pale yellowish brown face (L* 86).
Strength factors applied to the fitted bands (in the order of the parts of the fit script): 0.03159.

**Stored bands** (rounded as in the TOML):

* o_ray / alpha:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 259.1 | 5800.0 | 13658 | 0.6789 |
  | 418.6 | 2131.1 | 5018 | 0.5166 |
  | 563.5 | 1343.9 | 3165 | 0.125 |
  | 700.0 | 1731.2 | 4077 | 0.05201 |

* e_ray / gamma:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 283.5 | 4847.6 | 11415 | 0.1599 |
  | 417.3 | 2203.4 | 5189 | 0.02102 |
  | 578.7 | 1800.0 | 4239 | 0.009746 |

* beta_ray:

  | center nm | sigma cm^-1 | FWHM cm^-1 | peak mm^-1 |
  |---|---|---|---|
  | 286.1 | 3500.0 | 8242 | 0.9478 |
  | 407.1 | 1716.7 | 4043 | 0.05351 |
  | 557.3 | 1800.0 | 4239 | 0.01929 |

**Target and reached** (D65, 9.905 mm path):

* target face-up: L* 86, C* 12, h 101
* reached face-up (oriented, the renderer's color; pinned): L* 80.1, C* 15.6, h 96 (unpolarised mean, the fit target: L* 86.2, C* 11.5, h 100)
* o-ray / alpha: L* 60.4, C* 53.5, h 93; e-ray / gamma: L* 96.5, C* 6.5, h 114; beta: L* 94.0, C* 5.0, h 104
* Lab (pinned by the test): face [80.12, -1.68, 15.46], o [60.41, -2.89, 53.46], e [96.50, -2.61, 5.91], beta [93.97, -1.18, 4.86]

**Pleochroism check.** Yellow to colorless pleochroism (published): alpha brown-yellow, beta and gamma nearly colorless.

