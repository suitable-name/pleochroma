# Color variants research R2: beryl, topaz, tourmaline, andalusite (2026-10-27)

Lane R2 of the color variants campaign (plan: `C:\temp\mcp2\color_variants_plan_2026-10-27\PLAN_color_variants_2026-10-27.md`,
sections 2, 6, 7). Scope: candidate variants of Emerald, Aquamarine, Morganite, the beryl family
(Heliodor, Red Beryl, Goshenite), Topaz, Tourmaline and Andalusite. No Rust, no physics solving.
Everything below is data and arithmetic for the fit lane F2.

## 0. Conventions, method and what the lane found

### 0.1 Conventions used in every band table

* **Rays.** Uniaxial hosts (beryl, tourmaline): `o` = E perpendicular to c = o-ray = the "Along" color seen
  looking along c; `e` = E parallel to c = e-ray = "Across". Biaxial hosts: alpha / beta / gamma are the three
  principal vibration directions, tied to crystal axes in each section (Caltech's convention, see 3.1 and 5.1).
* **Peak** = Napierian absorption coefficient alpha in mm^-1 *above the stated visible baseline*:
  `alpha = 2.3026 * (A - A_base) / path_mm` with A the decadic absorbance of the CSV. This is the number for
  decision D4 (unit PerMm). The baseline is the smoothed visible minimum of that ray (listed per variant); it is
  the instrument pedestal and reflection loss, not color, and must not be rendered.
* **rel** = peak divided by the largest peak of that variant over all its rays.
* **sigma** = FWHM / 2.3548, in cm^-1 for energy Gaussians (all rows are `energy` shape). FWHM was read at half
  height from the CSV (`read`) or taken from a published width (`lit`); overlapped bands carry `est`.
* **Edge** rows are the UV charge-transfer tail (O to metal) modelled as one energy Gaussian centred in the UV
  (centre below the data window, so centre and sigma are `est`; the peak is hand-solved so that alpha at 380 nm
  matches the CSV after the neighbouring bands are subtracted). The plan's catalogue uses a combined edge band
  (`EDGE_DEFAULT_FWHM_CM1`) for the same purpose; the fit lane may substitute it.
* **Basis codes:** `read` = value read from a saved CSV, `lit` = published number, `est` = my estimate, the
  reasoning is given next to it, `scale` = existing fitted band times a factor read from a CSV.
* No variant uses more than 8 bands per ray (largest here: 4).
* Every CSV is `crates/pleochroma/data/reference_spectra/variant_<material>_<variant>_<pol>.csv`, header style of
  `caltech_aquamarine_grr1488_eperpc_5p531mm.csv`, downloaded 2026-10-27 from the Caltech Mineral Spectroscopy
  Server (G. R. Rossman, CC BY-NC 4.0, `http://minerals.gps.caltech.edu/FILES/Visible/...`). Rows: sorted
  ascending, duplicate wavelengths dropped, **rows above 1300 nm dropped** (NIR water lines, irrelevant to color;
  the original files reach 2500 nm), values unchanged. 55 files.

### 0.2 Findings that contradict the repo's current assumptions

These come from the saved spectra and should be read before the fit lane starts.

1. **Blue elbaite (indicolite, verdelite) is not dichroic at 720 nm.** Two independent samples (GRR 1935 blue and
   GRR 3770, plus the green GRR 1935) have the same Fe2+ band in both rays (GRR 1935 blue: o 0.905 vs e 0.994
   mm^-1; GRR 3770: 1.00 vs 0.95; GRR 1935 green: 0.540 vs 0.475). The built-in Tourmaline uses o:e = 3:1 for that band and `beryl_topaz_tourmaline_andalusite.md`
   uses w_e = 0.1; those weights belong to Fe2+-Fe3+ paired bands in dravite and schorl (Caltech page text for
   GRR 787), not to low-Fe elbaite. Pleochroism of elbaite comes from the UV/blue edge (o about twice e) and from Mn3+
   (rubellite) and Cu2+ (Paraiba).
2. **Blue topaz absorbs most along alpha (E parallel a), not beta.** Measured on three irradiated and one natural
   blue crystal: alpha about 5 times gamma, beta nearly zero (1632, A/mm above baseline at the peak: alpha 0.631, beta about 0.026, gamma 0.121). The
   repo's Report says "maximum along beta"; that is not supported.
3. **Andalusite axes:** Caltech labels alpha = E parallel c, beta = E parallel b, gamma = E parallel a. The repo
   doc says beta parallel a and gamma parallel b. The optic plane of andalusite is (010) so Y = b; Caltech is
   right. **All the Fe-Ti color is in alpha (E parallel c)**; beta and gamma are almost colorless. The built-in
   Andalusite (beta strongest) is qualitative and does not match.
4. **Mn2+-Ti4+ charge transfer in tourmaline sits at 325 nm**, with a tail into the visible (Rossman & Mattson 1986,
   Am. Mineral. 71:599, abstract). The repo's "canary 450 nm" band is not the IVCT band itself. The measured yellow
   elbaite has a polarized band at 397 nm (o) and 375 nm (e); its assignment is not given by Caltech.
5. **The Morganite and Topaz built-ins have isotropic tensors on anisotropic hosts** (Morganite: one isotropic
   540 nm band on a uniaxial host; Topaz: one isotropic 620 nm band on a biaxial host). `validate_for` will report
   `RankMismatch` for their Material Default unless C1 treats the default as exempt. The variants below are all
   full-rank.
6. **Host c_axis differs inside the beryl family:** Aquamarine has `c_axis = Vec3::X` (c in the table plane), Emerald
   and Morganite `Vec3::Y`. A variant is a tensor only (decision D5), so an Aquamarine variant on an Emerald host
   would look along c and show the near-colorless o-ray. The UI that lists "Also From Beryl" variants should either
   warn or the host's c_axis should be part of the choice. Same issue for the tourmaline built-in (`X`).
7. **`emerald_colombian_muzo.csv` is not measured data** (its own header: "user-delivered, secondary, unverified;
   sparse 19-point grid"). Nothing here depends on it.

### 0.3 Sources and blocked sources

Used and readable: Caltech index pages (beryl, tourmaline, topaz, Andalusite) and their ASCII files; Carlson &
Rossman 1988 abstract; Rossman & Mattson 1986 abstract; GIA description pages; the repo documents. Search-engine
summaries are marked "(search summary)" where the page itself could not be read.

Blocked or unusable (moved on): MDPI (Crystals 2023 Kamakanga emerald, Crystals 2025 Brazilian aquamarine,
HTTP 403, also with a browser user agent); RSC Advances 2025 imperial topaz (403); GIA
`Spectral-Differentiation-Between-Copper-and-Iron-Colorants.pdf` (404); old scanned Am. Mineral. PDFs
(Wood & Nassau 1968, Rossman et al. 1991: image-only, no text layer); Caltech over https (connection refused,
http works); Caltech files that are listed but return 404: `Andalusite305.alpha/beta/gamma` (GRR 305 viridine),
`Tr596g4.a/.c` (GRR 596 elbaite), `dravite12863.b`; several tourmaline entries are placeholders
("not yet", 0 bytes). Figure-only entries (GRR 508 morganite, 2536b pink beryl, 1393 pink topaz, 757 yellow-green
tourmaline, 565, 876, 888, the uvite/dravite chrome set 2128, 2467, 1719, 768) could not be read: figures are binary
images and the lane rules forbid binary downloads, so **no band was digitised from any figure**. Where the text of a
page describes a figure (for example "a manganese feature near 560 nm"), the text is quoted as the source.

## 1. Summary table

CSV column: files are `variant_...` in `crates/pleochroma/data/reference_spectra/`; `_eperpc` / `_eparc` or
`_alpha` / `_beta` / `_gamma` suffixes. Counts are bands in o / e (/ beta... as alpha / beta / gamma).

| Variant | Cause | Data quality | CSV files (stem) | Bands |
|---|---|---|---|---|
| Emerald, Colombian | Cr3+ (V3+), low Fe | partly measured (Afghan Cr emerald stand-in) | `emerald_colombian_proxy_grr3570` | o 3 / e 3 |
| Emerald, Zambian | Cr3+ plus Fe2+ | measured shape, edge estimated | `emerald_zambian_miku` | o 4 / e 3 |
| Emerald, Light | Cr3+ plus Fe2+, low content | measured | `emerald_light_jos_nigeria` | o 3 / e 2 |
| Aquamarine, Sky Blue (MD) | Fe2+ / Fe2+-Fe3+ IVCT | measured (existing fit of GRR 1488) | existing `caltech_aquamarine_grr1488_*` | o 4 / e 4 |
| Aquamarine, Santa Maria | same, strong | measured scale on the MD shape | `aquamarine_santa_maria_grr1780` | o 4 / e 4 |
| Aquamarine, Greenish | MD plus Fe3+ | estimated (heliodor edge added) | `heliodor_grr226`, existing 1488 | o 5 / e 5 |
| Morganite, Pink (MD) | Mn3+ | estimated shape from red beryl; no morganite file | `red_beryl_grr602` (shape donor) | o 1 / e 1 |
| Morganite, Peach | Mn3+ plus Fe3+ edge | estimated | `red_beryl_grr602`, `heliodor_grr226` | o 2 / e 2 |
| Heliodor (Golden) | Fe3+ | partly measured | `heliodor_golden_grr309`, `heliodor_grr226` | o 1 / e 2 |
| Red Beryl | Mn3+ | measured (one crystal) | `red_beryl_grr602` | o 2 / e 2 |
| Goshenite | none (host edge) | estimated | none | o 1 / e 1 |
| Topaz, Imperial | Cr3+ plus brown centre | estimated combination of measured parts | `topaz_pink_cr_ural_bobrovka`, `topaz_brown_natural_grr515` | a 3 / b 3 / g 3 |
| Topaz, Sherry | brown centre | measured shape, strength estimated | `topaz_brown_natural_grr515` | a 1 / b 1 / g 1 |
| Topaz, Pink | Cr3+ | measured shape, strength from literature | `topaz_pink_cr_ural_bobrovka` | a 2 / b 2 / g 2 |
| Topaz, Sky / Swiss / London Blue | O- centre | measured; Sky measured, Swiss estimated | `topaz_blue_*` (4 sets) | a 2 / b 2 / g 2 |
| Topaz, Colorless | none | estimated | none | a 1 / b 1 / g 1 |
| Tourmaline, Rubellite | Mn3+ | measured | `tourmaline_rubellite_grr1368` | o 3 / e 3 |
| Tourmaline, Pink | Mn3+, light | estimated scale of Rubellite | same | o 3 / e 3 |
| Tourmaline, Indicolite | Fe2+ | measured | `tourmaline_indicolite_grr1935_blue`, `_grr3770` | o 3 / e 3 |
| Tourmaline, Verdelite | Fe2+ plus Fe2+-Ti4+ | measured, edge estimated | `tourmaline_verdelite_grr1935_green` | o 3 / e 3 |
| Tourmaline, Chrome | Cr3+ / V3+ | estimated (no spectrum saved) | none | o 2 / e 2 |
| Tourmaline, Paraiba | Cu2+ | measured (one very Cu-rich section) | `tourmaline_paraiba_r030` | o 2 / e 2 |
| Tourmaline, Paraiba Violet (added) | Cu2+ plus Mn3+ | measured | `tourmaline_paraiba_mn_cu_pink` | o 2 / e 2 |
| Tourmaline, Yellow | Mn2+-Ti4+ (tail) | measured (very pale crystal) | `tourmaline_yellow_grr1935_yellow` | o 1 / e 1 |
| Andalusite, MD (Red-Brown / Green) | Fe2+-Ti4+, Fe3+ | measured shape, edge estimated | `andalusite_fe_ti_brazil_light_zone`, `_dark_zone` | a 4 / b 1 / g 1 |
| Andalusite, Greener | Mn3+ (viridine) | partly (band centres published, amplitudes secondary) | none (GRR 305 files 404) | a 3 / b 3 / g 3 plus Fe-Ti |
| Andalusite, Vanadian (added, optional) | V3+ / Cr3+ | measured | `andalusite_v_cr_llano` | a 2 / b 2 / g 2 |

Dropped or merged: none dropped. Two additions (Paraiba Violet, Andalusite Vanadian, the latter optional).
"Imperial" topaz is kept but flagged estimated because the cause is disputed (Cr3+ plus color centres versus
Cr4+, see 3.2).

---

## 2. Beryl family (Be3Al2Si6O18, hexagonal, uniaxial negative)

### 2.1 Axis convention and shared facts

* o-ray = E perpendicular to c ("a" files on the Caltech page: "c = polarized parallel to c; a = polarized
  perpendicular to c"), e-ray = E parallel to c ("c" files). Source:
  <http://minerals.gps.caltech.edu/FILES/Visible/Beryl/index.html>.
* Caltech page text (same URL): Fe2+ gives pale blue, Fe3+ golden yellow, both together deeper blue; green from a
  blue plus yellow mix, from Cr3+ (emerald) or V3+; Mn-bearing beryls are pink (Mn2+, pale) to red (Mn3+).
* Pleochroism (beryl reference, weak): emerald "green to blue green", aquamarine "blue to darker blue", morganite
  "light red to light violet" (<https://www.geo.utexas.edu/courses/347k/redesign/Gem_Notes/Beryl/beryl_main.htm>,
  search summary).
* Cr3+ in emerald: 4T1 near 425-437 nm, 4T2 near 605 nm (o) and 647 nm (e), spin-forbidden R lines 684 / 680 nm
  (sharp, 7 nm FWHM). The R lines are in the data but are not color; leave them out of the band model.
* Heliodor, Red Beryl and Goshenite are variants listed on the beryl built-ins (decisions D5, D6).

### 2.2 Emerald

Built-in Emerald is Caltech GRR 3570 (Afghanistan, Cr3+-dominated) refit 2026-10-19 (`calibration-records.md`).

#### 2.2.1 Emerald, Colombian

1. **Cause / axes:** Cr3+ on the Al site (V3+ often present, 427 / 610 nm, repo Report), very little Fe: Colombian
   stones have low Fe, moderate-to-high Cr and V ("Emerald myth busters", J. Bergman, search summary of
   <https://www.gemsociety.org/article/emerald-myth-busters-identifying-colombian-stones>). o = yellowish green,
   e = bluish green (Wood & Nassau 1968 via `calibration-records.md`; the PDF itself is an image scan).
2. **Spectra:** no Colombian spectrum exists on the Caltech server. Nearest measured stand-in: GRR 3570
   (Afghanistan, 0.859 mm), whose Fe2+ band is weak (A at 840 nm 0.15 vs 0.60 for the Cr band). Saved raw (the
   older repo file is a 5 nm resampling): `variant_emerald_colombian_proxy_grr3570_{eperpc,eparc}.csv`.
   The second Cr-dominated reference (Ural, 10 mm normalized, with an Fe2+ band at 800 nm):
   `variant_emerald_deep_ural_emer3_{eperpc,eparc}.csv` (377-800 nm).
3. **Band proposal** (baseline o 0.123, e 0.185 A/mm; path 0.859 mm):

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| o | 1 | 320 (edge) | 4500 | 0.50 | 0.26 | est (solved at 380 nm after the 4T1 tail) |
| o | 2 | 434 (4T1) | 1750 | 1.55 | 0.82 | read (FWHM 4123 cm^-1) |
| o | 3 | 605 (4T2) | 1110 | 1.38 | 0.73 | read (FWHM 2610 cm^-1) |
| e | 1 | 320 (edge) | 4500 | 0.20 | 0.11 | est |
| e | 2 | 422 (4T1) | 1550 | 1.19 | 0.63 | read (FWHM 3640 cm^-1) |
| e | 3 | 646 (4T2) | 1000 | 1.90 | 1.00 | read (FWHM 2342 cm^-1) |

   These are the same bands the Emerald built-in already carries. **A distinct Colombian variant therefore only
   makes sense as a different strength / Cr+V ratio on the same shape.** Hand arithmetic: the built-in's o 4T2 band
   (2.7 at 591 nm plus 1.6 at 633 nm, "0.3 cm" scale) is 0.9 + 0.53 mm^-1 against 1.38 here, so the built-in is
   already about the strength of the measured crystal.
4. **Target and pleochroism:** GIA: "the green to bluish green variety of beryl" (<https://www.gia.edu/emerald-description>);
   trade: Colombian greener and warmer than Zambian (sellers, qualitative). No published CIELAB found. A secondary,
   unverified digest (`research-2026-10-polarized-spectra.md` section 4) gives Muzo D65 L*a*b* 58.2 / -54.6 / 28.4
   (o, h 152.5) and 61.4 / -44.2 / 14.8 (e, h 161.5) at 3 mm; **do not use it as a measured target**.
5. **Strength anchor:** none published for GRR 3570. Colombian Cr/V: Cr and V "moderate to high" (Bergman); the
   Muzo digest says 0.55 wt% Cr2O3 + 0.10 wt% V2O3 (unverified). Practical anchor: scale the shape so a 7 mm stone
   reaches the target; the o 4T2 peak is 1.38 mm^-1 at the Afghan crystal's own Cr.
6. **Confidence:** partly measured. Missing: any Colombian spectrum with Cr/V analysis; V3+ separate bands.

#### 2.2.2 Emerald, Zambian

1. **Cause / axes:** Cr3+ plus a strong Fe2+ band; Zambian emeralds have higher Cr and Fe and lower V than
   Colombian (Bergman, search summary); Kamakanga: Cr 76-2934 ppmw (avg 1435), V 78-183 (avg 128), Fe 5136-11320
   (avg 8556) (<https://www.mindat.org/loc-258650.html>, summary of Crystals 13:1605, 2023); the Fe feature is much
   stronger than the Cr feature in those spectra, matching the data below. o = yellowish green, e = bluish green.
2. **Spectra:** Caltech, contributed by M. N. Taran, Miku, Zambia, 2.148 mm, light green:
   `variant_emerald_zambian_miku_{eperpc,eparc}.csv`. Source
   <http://minerals.gps.caltech.edu/FILES/Visible/Beryl/taran/Emerald_Zambia.a> and `.c`.
3. **Band proposal** (baseline o 0.057, e 0.080 A/mm):

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| o | 1 | 320 (edge) | 4500 | 0.25 | 0.32 | est |
| o | 2 | 430 (4T1) | 1980 | 0.27 | 0.35 | read |
| o | 3 | 605 (4T2) | 1320 | 0.21 | 0.27 | read |
| o | 4 | 833 (Fe2+) | 930 | 0.77 | 1.00 | read (FWHM 2183 cm^-1) |
| e | 1 | 420 (4T1) | 1550 | 0.22 | 0.29 | est (width from GRR 3570 e-ray; the finder returned a 4 nm spike) |
| e | 2 | 648 (4T2) | 820 | 0.44 | 0.57 | read |
| e | 3 | 850 (Fe2+, broad) | 1590 | 0.46 | 0.60 | read (FWHM 272 nm, overlapped with the NIR) |

4. **Target and pleochroism:** bluish green, darker than light Colombian (qualitative, trade). The Fe2+ band at
   833 nm puts a red-NIR edge on the e-ray and shifts the transmitted window bluer; o-ray stays yellower. No
   CIELAB found.
5. **Anchor:** Fe 5136-11320 ppmw (Kamakanga mean 8556) and Cr mean 1435 ppmw; this crystal has no analysis.
6. **Confidence:** measured shape (one crystal, baseline not removed). Missing: composition, edge.

#### 2.2.3 Emerald, Light

1. **Cause / axes:** same as Zambian at low content ("lighter green and bluish-green stones" have Cr/V below 10 in
   Kamakanga, Mindat summary above). Caltech: "light green, showing both chromium and iron features".
2. **Spectra:** Jos, Nigeria, 0.95 mm (Taran): `variant_emerald_light_jos_nigeria_{eperpc,eparc}.csv`.
3. **Bands** (baseline o 0.124, e 0.114 A/mm):

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| o | 1 | 434 | 1810 | 0.19 | 0.33 | read |
| o | 2 | 597 | 1120 | 0.17 | 0.30 | read |
| o | 3 | 812 (Fe2+) | 960 | 0.57 | 1.00 | read |
| e | 1 | 411 | 1340 | 0.19 | 0.33 | read |
| e | 2 | 644 | 850 | 0.25 | 0.44 | read (FWHM about 86 nm) |

   UV edge: add the Colombian edge times 0.2 (est). Fe2+ is present only in the o-ray here (e-ray 831 nm feature 0.067).
4. **Target:** light green (very pale: peak A above baseline below 0.1 per mm). GIA: green beryl is "too light"
   to be called emerald below a lab-set line; this is that boundary class.
5. **Anchor:** none. 6. **Confidence:** measured.

### 2.3 Aquamarine

Built-in Aquamarine = GRR 1488 (Lajes Pintada, 5.531 mm), bands in `aquamarine_through_citrine.rs`; its
ModelUnit peaks equal Napierian alpha at the 8.8 mm face-up path (checked: e 810 nm 8.82 / 8.8 = 1.00 mm^-1 plus
the 600 nm tail 0.23 gives 1.23 against 1.13 read from the CSV).

Common cause: Fe2+ (isolated, o-ray 830 nm) and the Fe2+-Fe3+ IVCT broad envelope 600-1000 nm (e-ray, the blue);
Fe3+ weak at 428 nm. Fe2+-Fe3+ at about 620 nm is "the main cause of blue coloration" (Crystals 2025, search
summary of <https://www.mdpi.com/2073-4352/15/9/775>).

#### 2.3.1 Aquamarine, Sky Blue (Material Default)

1. **Cause / axes:** as above; the e-ray carries the color, so the stone must be cut with c in the table plane
   (as the built-in does).
2. **Spectra:** existing `caltech_aquamarine_grr1488_{eperpc,eparc}_5p531mm.csv` (no new file needed).
3. **Bands:** the existing built-in (peaks in alpha mm^-1 = ModelUnit / 8.8):

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| o | 1 | 363 | 5531 | 0.135 | 0.13 | existing fit |
| o | 2 | 429 | 1500 | 0.024 | 0.02 | existing fit |
| o | 3 | 645 | 3657 | 0.0225 | 0.02 | existing fit |
| o | 4 | 845 | 2797 | 0.743 | 0.74 | existing fit |
| e | 1 | 378 | 3102 | 0.101 | 0.10 | existing fit |
| e | 2 | 428 | 565 | 0.036 | 0.04 | existing fit |
| e | 3 | 600 | 4405 | 0.371 | 0.37 | existing fit |
| e | 4 | 810 | 5471 | 1.002 | 1.00 | existing fit |

4. **Target:** GIA: "a light pastel greenish blue" for typical aquamarine, heat treatment "usually gives it a more
   bluish appearance" (<https://www.gia.edu/aquamarine-description>). Classification (search summary of Crystals
   2025, color space not confirmed): Sky Blue hue H 170-200, saturation S 15-25; Ocean Blue H 200-230, S 25-35;
   Deep Blue H > 230, S > 35; Light Blue H 140-170, S <= 15. Repo swatch: L* 85, C* 16, h 247 (7 mm, D65).
5. **Anchor:** Fe analysis not published for GRR 1488.
6. **Confidence:** measured.

#### 2.3.2 Aquamarine, Santa Maria (deep)

1. **Cause:** same chromophores at higher content, e-ray IVCT much stronger relative to the o-ray Fe2+ band.
2. **Spectra:** GRR 1780 "deep blue", Tatu Mine, Minas Gerais, 2.449 mm (stand-in; no Santa Maria sample on the
   server): `variant_aquamarine_santa_maria_grr1780_{eperpc,eparc}.csv`.
3. **Bands** (hand arithmetic: ratio of baseline-subtracted per-mm A, GRR 1780 over GRR 1488, at 600 / 650 / 800 nm:
   e 1.85 / 1.80 / 1.72; o at 750 / 800 nm 0.79 / 0.75; so e x 1.75 and o x 0.77 on every Sky Blue band; baselines
   1780: e 0.16, o 0.034 A/mm; 1488: e 0.157, o 0.123):

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| o | 1 | 363 | 5531 | 0.104 | 0.06 | scale x0.77 |
| o | 2 | 429 | 1500 | 0.019 | 0.01 | scale |
| o | 3 | 645 | 3657 | 0.017 | 0.01 | scale |
| o | 4 | 845 | 2797 | 0.572 | 0.33 | scale (read at 833 nm: 0.565) |
| e | 1 | 378 | 3102 | 0.177 | 0.10 | scale x1.75 |
| e | 2 | 428 | 565 | 0.063 | 0.04 | scale |
| e | 3 | 600 | 4405 | 0.648 | 0.37 | scale |
| e | 4 | 810 | 5471 | 1.754 | 1.00 | scale (read at 838 nm: 1.98 including the 600 nm tail, sum 2.16) |

4. **Target:** "Deep Blue" class H > 230, S > 35 (Crystals 2025 summary); deep saturated blue. No instrumental value.
5. **Anchor:** deep samples show a weaker 956 nm band (Crystals 2025 summary); no Fe ppm for GRR 1780.
6. **Confidence:** measured scale on the fitted shape.

#### 2.3.3 Aquamarine, Greenish

1. **Cause:** Fe2+ / Fe2+-Fe3+ blue plus Fe3+ UV-blue absorption (yellow); unheated aquamarine "leans greenish"
   (GIA). Heating converts Fe3+ to Fe2+ and removes the yellow (Caltech: GRR 1754 "heat treated to turn it from
   greenish-yellow to blue").
2. **Spectra:** the pre-heating spectrum of GRR 1754 is not on the server as data (only the heated one; figure
   pairs for the Taran Sneznoye crystals are images). Saved the Fe3+ donor: `variant_heliodor_grr226_*`,
   `variant_heliodor_golden_grr309_*` (2.6). I did not save GRR 1754.
3. **Bands:** the Sky Blue bands (2.3.1) plus one Fe3+ edge band in both rays (shape from the heliodor o-ray, hand
   solved, see 2.5): `edge 322 nm, sigma 4320 cm^-1, peak 0.45 mm^-1` (that is 0.5 of the heliodor edge,
   `est`), rel 0.45. Tuning range: peak 0.2-0.9. This gives alpha 0.25 mm^-1 at 380 nm and 0.06 mm^-1 at 450 nm.
4. **Target:** greenish blue ("light pastel greenish blue", GIA). Hue about 190-210 is expected between Sky Blue
   (170-200) and the Ocean class; no numeric target is published for greenish.
5. **Anchor:** none. 6. **Confidence:** estimated (the Fe3+ amount is a free parameter).

### 2.4 Morganite

Built-in Morganite is one isotropic band (540 nm, sigma 45 nm, peak 1.0) on a uniaxial host (see 0.2 item 5).

#### 2.4.1 Morganite, Pink (proposed replacement for the Material Default)

1. **Cause / axes:** Mn3+ on the Al site (pink); Mn2+ gives pale pink; GIA GNI Fall 2024 places the broad Mn3+
   band at about 567 nm (<https://www.gia.edu/gems-gemology/fall-2024-gemnews-helvine-inclusions-in-morganite>,
   search summary); Caltech GRR 2536b (pale pink, Mt. Antero): "a manganese feature near 560 nm" (index page, text).
   Pleochroism weak: light red to light violet.
2. **Spectra:** no digital morganite spectrum on the server (GRR 508 and 2536b are figures). **No CSV saved.** The
   shape donor is the Mn3+ red beryl (2.7): `variant_red_beryl_grr602_*`, same site, same host.
3. **Bands:** Mn3+ band at 560 nm in both rays, widths from red beryl (o 2190, e 1510), strength x0.2 of red beryl
   (target pastel; `est`):

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| o | 1 | 560 | 2190 | 0.089 | 0.86 | est (red beryl o peak 0.444 x 0.2) |
| e | 1 | 560 | 1510 | 0.103 | 1.00 | est (red beryl e peak 0.514 x 0.2) |

   The repo Report's o:e = 1.0:0.35 for morganite is `secondary`; the measured red beryl ratio is about 0.86:1
   and the beryl reference calls pleochroism weak, so the measured ratio is used.
4. **Target:** pastel pink; GIA: "morganite's pink", Madagascar rough "magenta-colored"
   (<https://www.gia.edu/morganite-description>).
5. **Anchor:** none (Mn content not found). 6. **Confidence:** estimated; band position from literature text,
   polarization and widths from a different beryl.

#### 2.4.2 Morganite, Peach

1. **Cause:** Mn3+ pink plus Fe3+-related orange: iron-bearing Mn beryl turns "orangey-pink to brown from an added
   charge-transfer band"; heating after irradiation removes it (GIA GNI summary above).
2. **Spectra:** none digital. 3. **Bands:** Pink bands (2.4.1) plus the heliodor Fe3+ edge band at 322 nm, sigma
   4320 cm^-1, peak 0.45 mm^-1 in both rays (`est`; Fe3+ edge from 2.5). 4. **Target:** orangy pink / peach, no
   numbers. 5. **Anchor:** none. 6. **Confidence:** estimated.

### 2.5 Heliodor (Golden)

1. **Cause / axes:** Fe3+ (UV edge plus 370 / 427 nm lines), minor Fe2+ near 800 nm (Caltech page).
2. **Spectra:** GRR 309 (golden, Goias, 3.306 mm, baseline corrected) and GRR 226 (yellow, 3.948 mm):
   `variant_heliodor_golden_grr309_*`, `variant_heliodor_grr226_*` (sparse digitised traces, 87-133 rows).
3. **Bands:** o-ray of both crystals agree (A/mm 380: 0.23 / 0.22; 400: 0.14 / 0.15; 428: 0.07 / 0.08; 450: 0.05).
   An energy Gaussian reproduces the tail; hand solution through 360 / 400 / 450 nm of GRR 226: centre 31 000 cm^-1
   (322 nm), sigma 4320 cm^-1, amplitude 0.39 A/mm; check at 428 nm: predicted 0.082 A/mm, measured 0.08.

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| o | 1 | 322 (edge) | 4320 | 0.90 | 0.68 | read/est (shape read, centre est) |
| e | 1 | 322 (edge) | 4320 | 0.90 | 0.68 | est (GRR 226 e-ray equals the o-ray within about 15 %) |
| e | 2 | 405 (Fe3+ 427 band side) | 1500 | 1.33 | 1.00 | est: GRR 309 e-ray A/mm is 0.40 at 428 nm and 0.19 at 450 nm (data start at 410 nm), solved for alpha 0.9 at 428 nm |

   Variant "Greenish Yellow" is obtained by adding the GRR 226 Fe2+ band: o 818 nm, sigma 955 cm^-1, 0.565 mm^-1
   (read) and e 955 nm, sigma 170 cm^-1 (narrow, ignore). GRR 226 and 309 disagree on the e-ray (0.10 vs 0.40 A/mm
   at 428 nm), so the e-ray Fe3+ band is the least certain part.
4. **Target:** golden yellow (Caltech page: "golden-yellow color"); no CIELAB found. 5. **Anchor:** none.
6. **Confidence:** partly measured (sparse traces).

### 2.6 Red Beryl

1. **Cause / axes:** Mn3+ on the Al site, Jahn-Teller elongated (NIR band 7190 cm^-1, outside the renderer's
   range; Czaja 2018 / Fridrichova 2017, search summary of
   <https://fgga.univie.ac.at/en/research/publications/publications-detail-view/pure/d94289e7-bbfd-46f6-8f24-57dbc95acc84/show/publ/Pure>).
   Nassau & Wood 1968 (Am. Mineral. 53:801): o-ray peaks near 0.48, 0.51 um, e-ray near 0.425 and 0.545 um
   (<https://msaweb.org/AmMin/AM53/AM53_801.pdf>, search summary).
2. **Spectra:** GRR 602, Wah Wah Mts., Utah, 1.559 mm: `variant_red_beryl_grr602_{eperpc,eparc}.csv`. The e-ray has
   an unusually high pedestal (0.338 A/mm) against 0.067 for the o-ray; treat the e-ray baseline with care.
3. **Bands** (baseline o 0.067, e 0.338 A/mm):

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| o | 1 | 330 (edge) | 4500 | 0.60 | 0.71 | est (rise to 360 nm, 0.32 A/mm raw) |
| o | 2 | 538 (Mn3+) | 2190 | 0.444 | 0.52 | read (FWHM 5166 cm^-1) |
| e | 1 | 330 (edge) | 4500 | 0.85 | 1.00 | est |
| e | 2 | 561 (Mn3+) | 1510 | 0.514 | 0.60 | read (FWHM 3560 cm^-1) |

   Weak extras in the data: o 880 nm 0.086 mm^-1 (narrow, water/NIR, ignore).
4. **Target:** red / raspberry (repo md, Caltech page "colored by Mn3+"); no CIELAB found. 5. **Anchor:** none
   (Mn concentration "high", no number found). 6. **Confidence:** measured (one crystal).

### 2.7 Goshenite

1. **Cause:** none; colorless beryl, host edge near 300 nm (repo Report). 2. **Spectra:** none (no goshenite file;
   GRR 3557's o-ray "IR colorless perp c" is an aquamarine). 3. **Bands:** a validation-safe pad, `est`:
   `edge 270 nm, sigma 4500 cm^-1, peak 0.40 mm^-1` in both rays; at 380 nm that is 0.023 mm^-1 (A about 0.01 per mm),
   invisible at 7 mm. If C1's `validate_for` forbids an empty band set, this is the minimal non-empty set.
   4. **Target:** colorless. 5. **Anchor:** none. 6. **Confidence:** estimated.

---

## 3. Topaz (Al2SiO4(F,OH)2, orthorhombic, biaxial positive)

### 3.1 Axes

Caltech labels (GRR 825 headers: "alpha=a, beta=b"; Taran Ural files "E || alpha / beta / gamma"): **alpha = E
parallel a, beta = E parallel b, gamma = E parallel c.** This agrees with the topaz optic orientation
"alpha = a, beta = b, gamma = c, optic plane (010)" (Mineral. Mag. 43:237,
<https://rruff.info/doclib/MinMag/Volume_43/43-326-237.pdf>, search summary). The Caltech `alpha_axis` /
`c_axis` mapping of the built-in must follow this: gamma along c.

Thickness caveat for the older topaz files (GRR 10, 515, 825, 1632, GIA 3828): the file header gives one thickness,
the index page says "plotted as N mm" with a different N (3828: 13.31 mm vs "10 mm"; 1632: 3.462 vs "3.0"; 10: 6.57
vs "10"; 825: 5.63 vs "5.0"). I used the **header** thickness; the absolute scale is uncertain by that ratio
(0.75-1.5). Shapes and polarization ratios are unaffected.

### 3.2 Topaz, Imperial

1. **Cause:** Cr3+ (pink to red) plus radiation-type brown centres (orange); heating imperial above about 500 C
   bleaches the centres and leaves pink (Cr 530-532 nm); the 420 / 530 nm absorption has also been assigned to Cr4+
   (J. Gems & Gemmology 2024, <https://jogg.cug.edu.cn/article/doi/10.15964/j.cnki.027jgg.2024.06.003>; RSC Adv.
   2025, page blocked; Cr up to 204 ppm, Ouro Preto summary). The cause is **disputed**; I model it as Cr3+ plus
   brown centre.
2. **Spectra:** no imperial spectrum on the server. Parts: Cr3+ from the Ural pink topaz (3.4) and the brown
   centre from GRR 515 (3.3).
3. **Bands** (alpha / beta / gamma; Cr part = Pink bands at x 0.055, brown = Sherry shape x 2, both `est`):

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| a | 1 | 562 (Cr 4T2) | 1640 | 0.099 | 0.25 | est (Ural x 0.055) |
| a | 2 | 390 (Cr 4T1 / edge) | 2570 | 0.224 | 0.56 | est |
| a | 3 | 400 (brown) | 4300 | 0.40 | 1.00 | est (515 alpha 0.20 x 2) |
| b | 1 | 533 | 1350 | 0.191 | 0.48 | est |
| b | 2 | 387 | 2580 | 0.163 | 0.41 | est |
| b | 3 | 452 (brown) | 3780 | 0.306 | 0.77 | est (515 beta 0.153 x 2) |
| g | 1 | 544 | 1610 | 0.160 | 0.40 | est |
| g | 2 | 400 | 2500 | 0.127 | 0.32 | est |
| g | 3 | 452 (brown) | 3780 | 0.306 | 0.77 | est (no gamma file; beta shape reused) |

4. **Target:** GIA "medium reddish orange to orange-red" (<https://www.gia.edu/topaz-description>); pleochroism
   "medium orange to light orange" (J. Gems & Gemmology 2024 above); reddish pleochroic color "often at the ends of
   pears and ovals" (GIA topaz quality factor, search summary).
5. **Anchor:** Cr up to 204 ppm (Ouro Preto, RSC 2025 summary); Cr 10-40 ppm gives pinkish red emission
   (Schneckenstein topaz, G&G 58(1):2, DOI 10.5741/GEMS.58.1.2, search summary).
6. **Confidence:** estimated combination (parts measured, ratio and gamma brown assumed).

### 3.3 Topaz, Sherry (brown)

1. **Cause:** unstable hole-type colour centres, brown (bleaches below 250 C or in sunlight; repo Report p.23).
2. **Spectra:** GRR 515a, natural brown, Thomas Mountains, Utah, 7.00 mm, alpha and beta only:
   `variant_topaz_brown_natural_grr515_{alpha,beta}.csv`. GRR 10 (electron irradiated before fading, brown plus
   blue, all three rays): `variant_topaz_blue_brown_electron_irradiated_grr10_*`.
3. **Bands** (baseline 0): 515 alpha is a broad plateau 340-480 nm (0.10 to 0.07 A/mm) falling to 0.01 at 650 nm;
   beta peaks at 452 nm. Sherry = measured x 1.5 (`est`, to reach "medium"):

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| a | 1 | 400 | 4300 | 0.30 | 1.00 | est: plateau 0.08 A/mm = 0.184 mm^-1 at 400 nm, peak 0.20 with the read FWHM 10 135 cm^-1, x 1.5 |
| b | 1 | 452 | 3780 | 0.23 | 0.77 | read (0.153 mm^-1, FWHM 8897 cm^-1) x 1.5 |
| g | 1 | 452 | 3780 | 0.23 | 0.77 | est (no gamma file) |

4. **Target:** GIA: "yellowish brown or brownish yellow to orange" (topaz description). 5. **Anchor:** none.
6. **Confidence:** measured shape, strength and gamma estimated.

### 3.4 Topaz, Pink

1. **Cause:** Cr3+ (4T2 about 530-560 nm, 4T1 about 390-420 nm). Katlang (Pakistan) pink topaz has Cr2O3 of
   about 0.01-0.03 wt% (Gubelin et al. 1986, G&G 22(3), <https://gia.edu/gems-gemology/fall-1986-pink-topaz-gubelin>,
   search summary).
2. **Spectra:** Taran, Bobrovka, Ural, 3.0 mm, Cr2O3 0.36, FeO 0.03, MnO 0.02 wt%:
   `variant_topaz_pink_cr_ural_bobrovka_{alpha,beta,gamma}.csv` (file page: "violet topaz"; the 0.36 wt% is 12-36
   times the Katlang value, so this crystal is much stronger than a pink gem).
3. **Bands** at x 0.055 (Katlang 0.02 wt% / 0.36 wt%; the factor range is 0.028-0.083). Native values (0.36 wt%,
   baseline alpha 0.186, beta 0.206, gamma 0.152 A/mm): alpha 562 nm 1.81 mm^-1, 390 nm 4.07; beta 533 nm 3.47,
   387 nm 2.96; gamma 544 nm 2.90, plateau 400 nm 2.30 (sigma read from FWHM: 1640 / 2570; 1350 / 2580; 1610 / 2500).

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 (x 0.055) | rel | basis |
|---|---|---|---|---|---|---|
| a | 1 | 562 | 1640 | 0.099 | 0.44 | read x scale |
| a | 2 | 390 | 2570 | 0.224 | 1.00 | read x scale |
| b | 1 | 533 | 1350 | 0.191 | 0.85 | read x scale |
| b | 2 | 387 | 2580 | 0.163 | 0.73 | read x scale |
| g | 1 | 544 | 1610 | 0.160 | 0.71 | read x scale |
| g | 2 | 400 | 2500 | 0.127 | 0.57 | est |

4. **Target / pleochroism:** GIA: chromium "pink, red, and violet-to-purple"; pleochroism: beta (533 nm) is the
   strongest pink ray, gamma next, alpha (strong 390 nm band, weak 562 nm) the yellowish ray. Gem-reference
   pleochroism "colorless, pale pink, pink" (<https://www.gemdat.org/gem-3996.html>).
5. **Anchor:** Cr2O3 0.01-0.03 wt% (Katlang); native 4T2 beta peak 3.47 mm^-1 at 0.36 wt% = 9.6 mm^-1 per wt%.
6. **Confidence:** measured shape, strength from literature (Cr scale only).

### 3.5 Topaz, Sky / Swiss / London Blue

1. **Cause:** irradiation O- hole centre (about 620 nm band; its concentration is linear in the 620 nm
   absorbance, Springer PCM 2005 summary <https://link.springer.com/doi/10.1007/s00269-005-0042-1>); stable to about
   200-300 C. Sky / Swiss / London are trade shade names tied to dose, not spectral classes ("encyclopedia colour
   centre", skyjems.ca, commercial). **Irradiation color centres are representable with a band model here because
   polarized spectra of three crystals are available.**
2. **Spectra:** (a) GRR 1632, neutron-irradiated, Nigeria, 3.462 mm, 3 rays:
   `variant_topaz_blue_irradiated_grr1632_*` (London-type strength); (b) GIA 3828, naturally blue, Zimbabwe,
   `variant_topaz_blue_natural_gia3828_*` (Sky-type strength); (c) GRR 825c clean blue region (alpha, beta):
   `variant_topaz_blue_clean_electron_irradiated_grr825c2_*`; (d) GRR 10 before fading (brown plus blue):
   `variant_topaz_blue_brown_electron_irradiated_grr10_*`.
3. **Bands.** Shape and polarization from GRR 1632 (baselines 0.007 / 0.004 / 0.010 A/mm); London = x 1;
   Sky = x 0.1 (3828 alpha peak 0.136 mm^-1 / 1632 1.45 = 0.094; 825c alpha 0.112 / 1.45 = 0.077); Swiss =
   x 0.3 (`est`, between the two measured strengths). Values for London:

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| a | 1 | 300 (edge) | 5000 | 1.5 | 1.00 | est (alpha 1.14 mm^-1 at 340 nm) |
| a | 2 | 660 (O-) | 2330 | 1.45 | 0.97 | read (FWHM 5489 cm^-1) |
| b | 1 | 300 (edge) | 5000 | 0.75 | 0.50 | est |
| b | 2 | 640 | 2300 | 0.07 | 0.05 | est (beta/alpha 0.04 at the peak, A/mm 0.026 vs 0.631) |
| g | 1 | 300 (edge) | 5000 | 1.0 | 0.67 | est |
| g | 2 | 645 | 2580 | 0.28 | 0.19 | read (FWHM 6075 cm^-1) |

   Sky (x 0.1): alpha 0.145, beta 0.007, gamma 0.028 at the band; Swiss (x 0.3): 0.435 / 0.021 / 0.084.
   The 3828 and 825c crystals show narrower or similar bands (alpha 661 nm FWHM 5312; 825c alpha 629 nm, FWHM
   3207 cm^-1), so the band centre is 630-660 nm and the width 3200-5500 cm^-1 depending on the crystal. Using the
   1632 values for all three strengths keeps one shape.
4. **Target / pleochroism:** blue is almost always treatment (GIA); London "a deep, slightly greenish or steely blue
   at higher doses" (skyjems.ca). From the data: alpha is the deep blue ray (absorbs 560-800 nm), gamma pale blue,
   beta colorless. The gem table says only "weak, colorless-pinkish-bluish" (gemdat).
5. **Anchor:** none in ppm; strengths above are measured A per mm.
6. **Confidence:** shape and ratios measured; Sky measured, Swiss and the three-name mapping estimated. Thickness
   caveat 3.1.

### 3.6 Topaz, Colorless

1. **Cause:** none, edge about 220 nm (repo Report). 2. **Bands (`est`):** `edge 250 nm, sigma 5000 cm^-1, peak
   0.30 mm^-1` in alpha, beta, gamma (0.007 mm^-1 at 380 nm). 3. Target colorless; anchor none; confidence estimated.

---

## 4. Tourmaline (elbaite, trigonal, uniaxial negative)

o-ray = E perpendicular to c (Caltech "a" files), e-ray = E parallel to c ("c" files)
(<http://minerals.gps.caltech.edu/FILES/Visible/tourmaline/Index.html>). The built-in has `c_axis = X`.
Caltech page statements: blue from Fe2+ (and rare Cu2+); green from Fe2+-Ti4+ IVCT with Fe2+, or Cr3+ / V3+;
amber-brown from Fe2+-Ti4+; yellow to yellow-brown from Mn2+-Ti4+ IVCT; pink and red from Mn3+, often radiation
induced.

### 4.1 Tourmaline, Rubellite

1. **Cause:** Mn3+ (5E to 5T2, about 535 nm). 2. **Spectra:** GRR 1368, dark pink elbaite, Otjimbinque, Namibia,
   2.188 mm ("one of the deeper colored natural pinks"): `variant_tourmaline_rubellite_grr1368_{eperpc,eparc}.csv`.
   Also on the page as figures: GRR 565 (natural and irradiated), 876, 888.
3. **Bands** (baseline o 0.250, e 0.160 A/mm). The e-ray is a broad envelope 450-560 nm (A above baseline 0.12,
   0.13, 0.17, 0.14, 0.05 at 450, 480, 520, 560, 600 nm); decomposed by hand into two bands and checked at the five
   wavelengths (model 0.24, 0.34, 0.40, 0.29, 0.13 vs read 0.28, 0.30, 0.39, 0.32, 0.12 mm^-1):

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| o | 1 | 320 (edge) | 4500 | 0.59 | 0.91 | est |
| o | 2 | 455 (plateau) | 1510 | 0.27 | 0.42 | read |
| o | 3 | 537 (Mn3+) | 1430 | 0.646 | 1.00 | read (FWHM 3364 cm^-1) |
| e | 1 | 320 (edge) | 4500 | 0.31 | 0.48 | est |
| e | 2 | 470 | 2200 | 0.25 | 0.39 | est (hand decomposition) |
| e | 3 | 535 (Mn3+) | 1500 | 0.25 | 0.39 | est (width taken from the o-ray) |

4. **Target / pleochroism:** GIA: "pink, red, purplish red, orangy red, or brownish red tourmaline"
   (<https://www.gia.edu/tourmaline-description>); from the data the o-ray (Along) is the deeper pink (peak 0.65
   vs 0.25 mm^-1), e-ray paler and broader. 5. **Anchor:** no composition published. 6. **Confidence:** measured.

### 4.2 Tourmaline, Pink

Light Mn3+ (often radiation induced; Caltech page). No light-pink spectrum as data (GRR 565 natural is a figure).
Bands: Rubellite x 0.35 (`est`, light pink): o 537 nm 0.226, 455 nm 0.095, edge 0.21; e 535 nm 0.088, 470 nm 0.088,
edge 0.11, widths as 4.1. Confidence: estimated from a measured shape.

### 4.3 Tourmaline, Indicolite

1. **Cause:** Fe2+ (720 nm and 1100-1120 nm) in low-Fe elbaite; no strong polarization (0.2 item 1).
2. **Spectra:** GRR 1935 blue, Usakos, Namibia, 3.641 mm; GRR 3770 pure blue, 2.00 mm:
   `variant_tourmaline_indicolite_grr1935_blue_*`, `variant_tourmaline_indicolite_grr3770_*`.
3. **Bands** (GRR 1935; baselines o 0.094, e 0.050 A/mm; the 3770 values are o 1.00 / e 0.95 mm^-1 at 720 nm,
   sigma 1154 / 1120):

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| o | 1 | 320 (edge) | 4500 | 0.65 | 0.65 | est (alpha 0.36 at 380 nm) |
| o | 2 | 719 (Fe2+) | 1080 | 0.905 | 0.91 | read (FWHM 2541 cm^-1) |
| o | 3 | 1120 (Fe2+ NIR) | 1610 | 0.40 | 0.40 | est (data end at 1104 nm, alpha 0.31 at 1100 nm; centre from the Caltech GRR 787 text; width from the repo Report 3800 cm^-1 FWHM) |
| e | 1 | 320 (edge) | 4500 | 0.33 | 0.33 | est |
| e | 2 | 723 (Fe2+) | 1120 | 0.994 | 1.00 | read (FWHM 2642 cm^-1) |
| e | 3 | 1120 | 1610 | 0.40 | 0.40 | est |

4. **Target:** GIA: "dark violetish blue, blue, or greenish blue". No CIELAB found. The data give a blue-green
   window (A flat 0.1-0.2 per mm at 450-560 nm, band at 720 nm); the violetish tone needs Mn3+ or Fe3+ not
   present in these crystals. 5. **Anchor:** no composition for 1935 / 3770 (page: Fe). 6. **Confidence:** measured.

### 4.4 Tourmaline, Verdelite

1. **Cause:** Fe2+ 712 nm plus a stronger blue-UV absorption (Fe2+-Ti4+ IVCT and edge) that removes blue.
2. **Spectra:** GRR 1935 small green crystal, Usakos, 3.645 mm (baseline corrected):
   `variant_tourmaline_verdelite_grr1935_green_{eperpc,eparc}.csv`.
3. **Bands** (baselines o 0.109, e 0.122 A/mm):

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| o | 1 | 330 (edge / Fe2+-Ti4+) | 4800 | 0.90 | 1.00 | est: checked at 400 / 450 / 480 nm: model 0.49 / 0.22 / 0.13 vs read 0.49 / 0.19 / 0.12 |
| o | 2 | 712 (Fe2+) | 1080 | 0.540 | 0.60 | read |
| o | 3 | 1120 (NIR) | 1610 | 0.19 | 0.21 | est |
| e | 1 | 330 (edge) | 4800 | 0.45 | 0.50 | est |
| e | 2 | 714 (Fe2+) | 1090 | 0.475 | 0.53 | read |
| e | 3 | 1120 (NIR) | 1610 | 0.11 | 0.12 | est |

4. **Target:** GIA lists no verdelite entry; green elbaite. Qualitative: green (blue-green) from the 712 nm band plus
   absorbed blue. 5. **Anchor:** none. 6. **Confidence:** measured, edge estimated.

### 4.5 Tourmaline, Chrome

1. **Cause:** Cr3+ and V3+ on Y/Z sites: bands about 432 and 607 nm (green Brumado uvite, 16 400 and 23 140 cm^-1, search
   summary); GIA: "intense green ... colored mostly by vanadium". Polarization: for the green Amstall tourmaline
   epsilon (E perp c / E par c): V 12.3 / 11.9, Cr 39.7 / 15.9 (search summary; paper not identified), so Cr is
   about 0.40 in the e-ray and V about equal.
2. **Spectra:** Caltech GRR 2128, 2467, 1719, 768 and the Cr olenite are figures only. **No CSV; no band
   digitised.** 3. **Bands** from the repo Report (`secondary`: Cr2O3 coefficients 20 / 18 cm^-1 per wt%, FWHM 2800 /
   3200 cm^-1), Cr2O3 0.2 wt% (GRR 1719 has 0.199 Cr2O3 + 0.452 V2O3, figure only) times 0.1 for mm: all `est`:

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| o | 1 | 432 | 1360 | 0.36 | 0.90 | est (Report 18 cm^-1 per wt% x 0.2 / 10) |
| o | 2 | 607 | 1190 | 0.40 | 1.00 | est (Report 20 cm^-1 per wt% x 0.2 / 10) |
| e | 1 | 432 | 1360 | 0.14 | 0.36 | est (o x 0.40) |
| e | 2 | 607 | 1190 | 0.16 | 0.40 | est |

4. **Target:** "intense green" (GIA). 5. **Anchor:** Cr2O3 0.2 wt% (GRR 1719, composition only).
6. **Confidence:** estimated; needs the figure data.

### 4.6 Tourmaline, Paraiba (Cu2+)

1. **Cause:** Cu2+ (Jahn-Teller d9): about 700 nm (shared with Fe2+) and a strong band 900-925 nm
   (Merkel & Breeding 2009, G&G 45(2), <https://www.gia.edu/gems-gemology/summer-2009-tourmalines-copper-merkel>,
   abstract); strongest E parallel c. Mn3+ (520 nm) gives violet; heating removes it.
2. **Spectra:** GIA R030, Sao Jose da Batalha, 0.200 mm (Rossman et al. 1991, Am. Mineral. 76:1479):
   `variant_tourmaline_paraiba_r030_{eperpc,eparc}.csv`. Caveats: o-ray "corrected for baseline", e-ray
   not (e-ray pedestal 1.38 A/mm = 0.276 A at 0.2 mm, against 0.224 for the o-ray).
3. **Bands** (baselines o 0.224, e 1.380 A/mm; the e-ray bands overlap into a 700-1000 nm plateau of
   alpha 12-16 mm^-1, the fit lane must deconvolve it; I give the two finder peaks):

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| o | 1 | 709 | 940 | 3.34 | 0.21 | read (FWHM 2211 cm^-1) |
| o | 2 | 908 | 1050 | 7.48 | 0.47 | read (FWHM 2464 cm^-1) |
| e | 1 | 756 | 1460 | 13.4 | 0.84 | read (overlapped) |
| e | 2 | 940 | 1380 | 15.9 | 1.00 | read (overlapped) |

   The ratio e:o at 720 nm is about 4:1 (5.5 vs 1.4 A/mm above baseline), e:o at 920 nm about 2:1.
4. **Target:** GIA: "intense violetish blue, greenish blue, or blue" (tourmaline description); neon blue.
5. **Anchor:** R030 is a thin section (0.2 mm) with Cu2+ A(940 nm, e) = 1.66 above 0.28 baseline. A 7 mm stone at the
   same Cu would have A about 70; typical stones need roughly 1 / 50 to 1 / 200 of these peaks (`est`,
   from absorbances typical for neon-blue stones; Merkel & Breeding relate CuO to A at 900 nm but the calibration
   page was not readable). **The fit lane must scale to a target, not use these peaks as published.**
6. **Confidence:** measured shape, unreadable calibration. Missing: CuO wt% for R030.

### 4.7 Tourmaline, Paraiba Violet (added)

Cu2+ plus Mn3+: the pink Taran crystal, 1.84 mm (`variant_tourmaline_paraiba_mn_cu_pink_*`), baselines o 0.206,
e 0.165 A/mm: o 526 nm sigma 1060 peak 0.923 (read) and 906 nm sigma 870 peak 1.063 (read); e 520 nm sigma 2070
peak 0.372 (read) and 738 nm sigma 1310 peak 0.248 (read). Target violet to violetish blue (GIA). Confidence measured.
Reason for the addition: the plan's "Paraiba (Cu2+, with Mn3+)" is exactly this and R030 has no Mn.

### 4.8 Tourmaline, Yellow

1. **Cause:** Mn2+-Ti4+ IVCT (band at 325 nm) and Mn2+; yellow-green Zambian elbaite: Rossman & Mattson 1986
   (Am. Mineral. 71:599, abstract, <https://authors.library.caltech.edu/38499/>); the Caltech page: "greenish yellow
   to yellow ... Mn2+ plus Mn2+-Ti4+ IVCT". The assignment of the 397 nm band of the saved crystal is not stated.
2. **Spectra:** GRR 1935 very pale yellow, Usakos, 5.353 mm: `variant_tourmaline_yellow_grr1935_yellow_*`.
   GRR 757 (yellow-green Zambia) is a figure only.
3. **Bands** (baselines o 0.040, e about 0; o A/mm 0.25 at 400 nm, e 0.13 at 380 nm):

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| o | 1 | 397 | 1820 | 0.49 | 1.00 | read (FWHM 4280 cm^-1) |
| e | 1 | 375 | 1800 | 0.32 | 0.65 | read (peak from the table, width est) |

4. **Target:** yellow (very pale in this crystal, stone scale x 3-5 `est`), canary-greenish yellow (Caltech page).
5. **Anchor:** none. 6. **Confidence:** measured but one very pale crystal; the o-ray band is strongly polarized (0.25
   vs 0.05 A/mm at 400 nm).

---

## 5. Andalusite (Al2SiO5, orthorhombic Pnnm, biaxial negative)

### 5.1 Axes

Caltech (Llano GRR 3467 and GRR 305 file labels): **alpha = E parallel c, beta = E parallel b, gamma = E parallel
a**. Taran files are labelled E || a / b / c and were mapped to gamma / beta / alpha. Source:
<http://minerals.gps.caltech.edu/FILES/Visible/Andalusite/Index.html>. Carlson & Rossman 1988 use alpha, beta,
gamma without a mapping. (Repo doc says beta || a, gamma || b: not supported, 0.2 item 3.)
Caltech page text: greenish yellow from Fe3+; dark green with Mn3+; "intense brownish-red color ... for light
polarized in the c direction ... strong absorption from Fe2+-Ti4+ intervalence charge transfer centered near 480 nm";
V3+ and Cr give reddish-brown colors.

### 5.2 Andalusite, Material Default proposal ("Red-Brown / Green")

1. **Cause:** Fe2+-Ti4+ IVCT at 486 nm in alpha, Fe3+ UV edge in beta and gamma (yellow-green).
2. **Spectra:** Taran, color-zoned crystal, Brazil, FeO 0.35 wt% and traces of Ti, normalized to 1.0 cm from 0.79
   mm, three rays: light zone `variant_andalusite_fe_ti_brazil_light_zone_{alpha,beta,gamma}.csv`, dark zone
   `variant_andalusite_fe_ti_brazil_dark_zone_*` (the dark zone alpha is saturated at 460-540 nm at A = 39 per cm and
   its band top is not measured; its 433 and 520 nm values are therefore lower bounds). Light zone is the
   reference.
3. **Bands** (light zone, baselines alpha 0.019, beta 0.008, gamma 0.013 A/mm):

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| a | 1 | 320 (edge) | 4200 | 2.0 | 0.87 | est |
| a | 2 | 400 (shoulder) | 2300 | 1.0 | 0.43 | est (plateau 0.43 A/mm at 380-410 nm) |
| a | 3 | 486 (Fe2+-Ti4+) | 2600 | 2.31 | 1.00 | read (FWHM 6124 cm^-1) |
| a | 4 | 760 (tail) | 2700 | 0.30 | 0.13 | est (tail 0.16 A/mm at 700 nm exceeds one Gaussian by 0.07) |
| b | 1 | 330 (edge) | 5200 | 1.34 | 0.58 | est (Gaussian through 390 / 420 / 460 nm, solved) |
| g | 1 | 330 (edge) | 5200 | 0.60 | 0.26 | est (0.45 x beta) |

   Dark zone: alpha 3.9 A/mm+ saturated (about 4x the light zone); the ratio is a lower bound.
4. **Target / pleochroism:** alpha brownish red (absorbs 380-560 nm, passes red), beta and gamma pale yellow to
   greenish yellow. Gem reference: "dark red / brown red" strong (Berkeley table, search summary); gem guides: "green,
   red or brown depending on viewing angle". No CIELAB found. 5. **Anchor:** FeO 0.35 wt% (light zone). The light
   zone alpha has A = 1.0 per mm at 486 nm, so an 8.8 mm face-up path along alpha is opaque (A about 9): the fit
   lane should scale to a stone (about 0.2-0.3 of this).
6. **Confidence:** measured shape (one crystal), edge estimated. The built-in's qualitative bands do not
   resemble it (0.2 item 3).

### 5.3 Andalusite, Greener (viridine, Mn3+)

1. **Cause:** Mn3+ on the Al1 site, three intense bands at 14 250, 22 000 and 23 500 cm^-1 (702, 455, 426 nm)
   (Halenius 1978, Can. Mineral. 16:567, <https://rruff.info/uploads/CM16_567.pdf>, search summary); Caltech GRR 305
   (Minas Gerais, MnO 1.06-1.72 wt%, avg 1.41; FeO 0.59-0.76 wt%, apple green, alpha / beta / gamma files). The three
   files are **404 on the server**, so no CSV.
2. **Bands:** centres from literature, widths and relative intensities from the repo Report (`secondary`, Report p.25:
   FWHM 2500 / 2000 / 1800 cm^-1; 18 / 15 / 12 cm^-1 per wt% Mn2O3; polarization weights alpha 0.3 / 0.4 / 0.4,
   beta 0.8 / 1.0 / 1.0, gamma 1.0 / 0.8 / 0.8). All values `est`, at 0.1 wt% Mn2O3 (viridine at 1.4 wt% is
   far too dark for a gem; the sample was 0.258 mm thick):

| ray | band nm | sigma cm^-1 | peak mm^-1 | rel |
|---|---|---|---|---|
| a | 702 | 1060 | 0.054 | 0.30 |
| a | 455 | 850 | 0.060 | 0.33 |
| a | 426 | 760 | 0.048 | 0.27 |
| b | 702 | 1060 | 0.144 | 0.80 |
| b | 455 | 850 | 0.150 | 0.83 |
| b | 426 | 760 | 0.120 | 0.67 |
| g | 702 | 1060 | 0.180 | 1.00 |
| g | 455 | 850 | 0.120 | 0.67 |
| g | 426 | 760 | 0.096 | 0.53 |

   Combine with the Fe-Ti variant of 5.2 scaled x 0.3 in alpha (`est`) and the same edges; the band count stays
   4-5 per ray.
3. **Target:** green along beta and gamma, yellow-red along alpha (Report); apple green viridine (Caltech).
4. **Anchor:** MnO 1.06-1.72 wt% (viridine); gem andalusite carries much less. 5. **Confidence:** partly published
   (centres), rest secondary or estimated; the polarization of the viridine bands was not verified against GRR 305.

### 5.4 Andalusite, Vanadian (added, optional)

V3+ / Cr3+: Llano uplift, GRR 3467, 0.110 mm, V2O3 about 0.81 wt% and Cr2O3 about 0.09 wt% (Caltech page; Carlson &
Rossman 1988: up to 0.81 and 0.12). Bands 320 (weak, gamma), 370 (unpolarized), 420 (strongest in alpha) and 565 nm
(strongest in alpha), 745 (V4+). Saved: `variant_andalusite_v_cr_llano_{alpha,beta,gamma}.csv` (UV-visible only,
275-880 nm). Reading (baselines alpha 0.365, beta 0.236, gamma 0.230 A/mm):

| ray | # | centre nm | sigma cm^-1 | peak mm^-1 | rel | basis |
|---|---|---|---|---|---|---|
| a | 1 | 418 | 2360 | 16.8 | 1.00 | read (FWHM 5568 cm^-1) |
| a | 2 | 557 | 1720 | 3.96 | 0.24 | read |
| b | 1 | 420 | 2400 | 1.9 | 0.11 | est |
| b | 2 | 573 | 1000 | 0.93 | 0.06 | read |
| g | 1 | 420 | 2400 | 1.2 | 0.07 | est |
| g | 2 | 574 | 1000 | 0.58 | 0.03 | est |

Per wt% V2O3 (alpha 418 nm) = 20.8 mm^-1. Pleochroism "yellow to colorless" from the polarized 420 nm band
(Carlson & Rossman 1988, <https://authors.library.caltech.edu/38513>). The plan asks only for MD and Greener; this
is the only measured V-andalusite and is offered as optional (not a common gem; the crystal is "reddish brown").

---

## 6. Open items and handoff notes

1. **Fit lane F2** should refit rows marked `read` with bounded energy Gaussians on the CSVs (baseline per variant
   above), keep `est` rows only as starting values, and not render R lines.
2. **ModelUnit meaning in the built-ins is not uniform.** Aquamarine's peaks equal alpha times the 8.8 mm face-up
   path (checked in 2.3), Emerald's were "scaled to 0.3 cm" (`calibration-records.md`). I converted the Aquamarine
   MD with /8.8; F2 should confirm both before mixing existing bands with new ones.
   **Strength scaling:** all peaks are per mm of the sample crystal; typical-stone scales are in each anchor
   paragraph (Pink topaz x 0.055, Santa Maria o x 0.77 / e x 1.75, Sky / Swiss / London x 0.1 / 0.3 / 1, Paraiba
   about 1/50-1/200, Andalusite about 0.2-0.3 of the light zone).
3. **Needs a figure reader** (a lane that may download images): GRR 508 / 2536b morganite, 1393 pink topaz,
   757 yellow-green tourmaline, 2128 / 2467 / 1719 / 768 chrome tourmaline, 565 / 876 / 888 pink tourmaline. Until
   then Morganite, Chrome Tourmaline, Imperial Topaz, Greenish Aquamarine, Pink Tourmaline and Goshenite are
   `estimated`.
4. **Decision for the owner:** keep a distinct Emerald Colombian variant only if a Cr/V-richer or V-bearing
   spectrum is found; with GRR 3570 it duplicates the Material Default (2.2.1).
5. **Fixes in `docs/chromophores/beryl_topaz_tourmaline_andalusite.md`** (not made here, file is not mine): topaz
   maximum along alpha not beta; andalusite beta parallel b, gamma parallel a; elbaite Fe2+ 720 nm not polarized;
   Mn2+-Ti4+ band at 325 nm.

## 7. Source list

* Caltech Mineral Spectroscopy Server, G. R. Rossman: beryl
  <http://minerals.gps.caltech.edu/FILES/Visible/Beryl/index.html>; tourmaline
  <http://minerals.gps.caltech.edu/FILES/Visible/tourmaline/Index.html>; topaz
  <http://minerals.gps.caltech.edu/FILES/Visible/topaz/Index.html>; andalusite
  <http://minerals.gps.caltech.edu/FILES/Visible/Andalusite/Index.html>; license CC BY-NC 4.0, DOI 10.7907/jywr-qq57.
* Carlson & Rossman 1988, Am. Mineral. 73:1366, <https://authors.library.caltech.edu/38513>,
  <https://msaweb.org/AmMin/AM73/AM73_1366.pdf>.
* Rossman & Mattson 1986, Am. Mineral. 71:599, <https://authors.library.caltech.edu/38499/>.
* Merkel & Breeding 2009, G&G 45(2):112, <https://www.gia.edu/gems-gemology/summer-2009-tourmalines-copper-merkel>.
* Rossman, Fritsch, Shigley 1991, Am. Mineral. 76:1479, <https://msaweb.org/AmMin/AM76/AM76_1479.pdf> (scan, unreadable).
* Halenius 1978, Can. Mineral. 16:567, <https://rruff.info/uploads/CM16_567.pdf> (search summary).
* Nassau & Wood 1968, Am. Mineral. 53:801, <https://msaweb.org/AmMin/AM53/AM53_801.pdf> (search summary);
  Wood & Nassau 1968, 53:777, <https://msaweb.org/AmMin/AM53/AM53_777.pdf> (scan, unreadable).
* Gubelin, Graziani, Kazmi 1986, G&G, <https://gia.edu/gems-gemology/fall-1986-pink-topaz-gubelin> (search summary).
* GIA descriptions: <https://www.gia.edu/aquamarine-description>, <https://www.gia.edu/emerald-description>,
  <https://www.gia.edu/morganite-description>, <https://www.gia.edu/topaz-description>,
  <https://www.gia.edu/tourmaline-description>; GIA GNI Fall 2024 morganite,
  <https://www.gia.edu/gems-gemology/fall-2024-gemnews-helvine-inclusions-in-morganite> (search summary).
* Crystals 2025 Brazilian aquamarine <https://www.mdpi.com/2073-4352/15/9/775> and Crystals 2023 Kamakanga emerald
  <https://www.mdpi.com/2073-4352/13/11/1605> (both 403; search summaries); Mindat Kamakanga
  <https://www.mindat.org/loc-258650.html>.
* Imperial topaz: <https://jogg.cug.edu.cn/article/doi/10.15964/j.cnki.027jgg.2024.06.003>; RSC Adv. 2025
  <https://pubs.rsc.org/en/content/articlehtml/2025/ra/d5ra02029h> (403); O- centre
  <https://link.springer.com/doi/10.1007/s00269-005-0042-1>; topaz optics
  <https://rruff.info/doclib/MinMag/Volume_43/43-326-237.pdf>.
* Gem pleochroism references: <https://www.gemdat.org/gem-3996.html>,
  <https://www.geo.utexas.edu/courses/347k/redesign/Gem_Notes/Beryl/beryl_main.htm>,
  <https://nature.berkeley.edu/classes/eps2/wisc/pleo.html> (search summaries).
* Repo: `docs/chromophores/beryl_topaz_tourmaline_andalusite.md` (Report p.21-25, secondary),
  `research-2026-10-polarized-spectra*.md`, `research-2026-10-primary-data.md`, `calibration-records.md`,
  `aquamarine_through_citrine.rs`, `diamond_through_emerald.rs`, `zircon_through_topaz.rs`,
  `spinel_through_tourmaline.rs`, `andalusite_through_glass.rs`.
