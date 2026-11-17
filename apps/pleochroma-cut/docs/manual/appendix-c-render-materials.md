# Appendix C: Built-in Render Materials

This table lists every one of the app's 39 built-in render materials (Ametrine is the zoned one), as
used by the spectral renderer (Chapter 2) and the Material Editor's
templates. n_D is the refractive index at the sodium D line (589.3nm).
Birefringence (Δn) is the difference between a material's fast and slow
rays at that same wavelength; isotropic materials have none. For a two-axis
material n_D is the middle index (nβ), which is the base curve.

**Dispersion is given as B–G**, the difference between the index at 430.8 nm
(the Fraunhofer G line) and at 686.7 nm (the B line), to four places. That is the figure
gemological references print, and the one the app shows wherever a dispersion is
read-only (the Stone tab, the Material Reference, the command line). The **Abbe
number** V_d beside it is built on the F–C interval (486.1 to 656.3 nm), which the
Material Editor takes as its input: V_d = (n_D − 1) / (n_F − n_C). A lower V_d means
more dispersive, i.e. more "fire". Every figure comes from the material's own index
curve, so it is the program's model value, not a literature value; the published
figures are listed under "Model Values and Published Figures" below. A dash in the
Abbe column means the figure is not printed here; `pleochroma-cli materials --markdown
optics` prints this table with every Abbe number.

All 39 are reachable from the Stone tab's **Material** and **Preview As...** combos
(Chapter 2), listed alphabetically, and in the **Material Reference** window (Chapter 6),
which also shows each one's hardness, toughness, cleavage, specific gravity and curves.
The entries marked with `*` are also the Material Editor's base templates.

## Optics and Color

| Material | Optical character | n_D | Birefringence (Δn) | Dispersion (B–G) | Abbe V_d | Color |
|---|---|---|---|---|---|---|
| Diamond* | Isotropic | 2.417 | — | 0.0444 | 55.3 | colorless |
| Sapphire* | Uniaxial (−) | 1.768 | −0.0081 | 0.0181 | 72.3 | Royal blue (Fe²⁺–Ti⁴⁺ charge transfer, GIA cross sections, 12.5 ppma·cm); a 7 mm stone face-up is a vivid, slightly violetish medium-dark blue, the o-ray darker than the greenish e-ray. The lighter Medium Blue is a Color Variant |
| Ruby* | Uniaxial (−) | 1.768 | −0.0081 | 0.0181 | 72.3 | Red (Cr³⁺ and Fe³⁺, GIA cross sections at about 400 and 1000 ppma·cm, a typical Mozambique-type ruby); a 7 mm stone is a dark red, the o-ray absorbs more than the e-ray |
| Emerald* | Uniaxial (−) | 1.579 | −0.0060 | 0.0140 | 70.9 | Green (Cr³⁺/V³⁺), narrow green transmission window |
| Zircon* | Uniaxial (+) | 1.925 | +0.0590 | 0.0386 | 41.0 | Colorless (trace U⁴⁺ lines, as in a low-uranium gem zircon) |
| Alexandrite | Biaxial (+) | 1.743 | +0.0076 | 0.0171 | 73.9 | color-change: bluish green in daylight, purplish red in incandescent light (Cr³⁺; measured rays of a Tanzanian alexandrite, the red ray estimated) |
| Topaz* | Biaxial (+) | 1.627 | +0.0080 | 0.0140 | 76.8 | Blue (the Swiss and sky blue of electron-irradiated, heated topaz; measured spectrum, one ray estimated) |
| Spinel* | Isotropic | 1.716 | — | 0.0202 | 60.6 | Red (Cr³⁺) |
| Quartz* | Uniaxial (+) | 1.544 | +0.0091 | 0.0133 | 69.7 | colorless (rock crystal) |
| Tourmaline | Uniaxial (−) | 1.639 | −0.0210 | 0.0169 | 64.6 | Light bluish green (the measured green elbaite crystal of Caltech GRR 1935 at about a third of its strength), strongly dichroic in the ultraviolet and blue edge — the "dark ray" runs along the c-axis |
| Tanzanite* | Biaxial (+) | 1.694 (nβ, model value) | +0.0090 (model value) | 0.0298 | 39.9 | Models the heat-treated commercial stone: violet-blue, trichroic: deepest and most purplish along the alpha axis, blue along beta, lighter blue along gamma (fitted to the Caltech spectra of zoisite GRR 2292, a naturally blue crystal without the band that heating removes) |
| Synthetic Moissanite* | Uniaxial (+) | 2.647 | +0.0415 | 0.1117 | 25.9 | colorless |
| Cubic Zirconia* | Isotropic | 2.158 | — | 0.0604 | 33.5 | colorless |
| Aquamarine | Uniaxial (−) | 1.577 | −0.0060 | 0.0140 | 70.7 | Pale blue, as most cut aquamarines are (Fe²⁺–Fe³⁺ intervalence band at about 600–680 nm along the c axis, Fe²⁺ at about 830 nm): the e-ray (along c) is a deep violet-blue, the o-ray nearly colorless; fitted to the Caltech spectra of aquamarine GRR 1488, with some Fe³⁺ for the slightly greenish blue of most stones. The stone is oriented with the c axis in the table plane (as for Tourmaline), so the face-up view shows the blue ray |
| Morganite | Uniaxial (−) | 1.577 | −0.0060 | 0.0140 | 70.7 | Pale salmon pink (Mn³⁺ with some Fe³⁺) |
| Chrysoberyl (Yellow) | Biaxial (+) | 1.746 | +0.0076 | 0.0171 | 74.2 | Greenish yellow (Fe³⁺ with a trace of Cr³⁺) |
| Amethyst | Uniaxial (+) | 1.544 | +0.0091 | 0.0133 | 69.7 | Purple/violet (irradiation-induced iron color centre) |
| Citrine | Uniaxial (+) | 1.544 | +0.0091 | 0.0133 | 69.7 | Pale yellow (Fe³⁺ absorption edge rising through the blue) |
| Ametrine | Uniaxial (+) | 1.544 | +0.0091 | 0.0133 | 69.7 | Bicolor quartz: amethyst purple on one side of a plane through the stone's centre, citrine yellow-orange on the other (Chapter 24) |
| Pyrope Garnet | Isotropic | 1.714 | — | 0.0218 | 56.1 | Medium red (Cr³⁺/Fe²⁺ with some Mn²⁺, measured spectra) |
| Almandine Garnet | Isotropic | 1.790 | — | 0.0238 | 56.9 | Deep red, slightly orange (Fe²⁺ triplet over a blue edge with some Mn²⁺, measured spectra) |
| Spessartine Garnet | Isotropic | 1.800 | — | 0.0267 | 51.2 | Light vivid orange (Mn²⁺ lines at 411, 421 and 429 nm plus a charge-transfer edge; fitted to the Caltech spectrum of spessartine GRR 43) |
| Color-Change Garnet (Pyrope-Spessartine) | Isotropic | 1.760 | — | 0.0248 | 52.5 | Tanzanian color-change garnet, the common type: khaki yellow in daylight, turning orange to salmon pink under incandescent light (the measured Tunduru spectrum with a weaker V³⁺/Cr³⁺ band at 581 nm: the Mn²⁺ edge closes the violet and blue). The rare blue-to-purple stones are the Tunduru and Stronger Change variants |
| Grossular Garnet (Tsavorite) | Isotropic | 1.734 | — | 0.0277 | 45.7 | Vivid green (V³⁺ at 436 and 606 nm), transmission windows at 520 nm and beyond 700 nm; fitted to the Caltech spectrum of tsavorite GRR 229 |
| Andradite Garnet (Demantoid) | Isotropic | 1.887 | — | 0.0565 | 26.9 | Yellow-green, midway between the deep green of the Russian slab and the light green of today's dealer stones (Fe³⁺ edge and Cr³⁺ at 615–650 nm, with one tuned band that sets the saturation); built from the Caltech spectra of demantoid GIA 2952 and andradite GRR 48 — the most dispersive of the garnets here |
| Peridot | Biaxial (+) | 1.669 (nβ, model value) | +0.0360 (model value) | 0.0198 | 56.5 | Yellow-green, trichroic (Fe²⁺, fitted to the Caltech olivine spectra) |
| YAG | Isotropic | 1.833 | — | 0.0272 | 52.0 | colorless synthetic |
| GGG | Isotropic | 1.970 | — | 0.0445 | — | colorless synthetic (historic diamond simulant) |
| Benitoite | Uniaxial (+) | 1.757 | +0.0470 | 0.0390 | 29.1 | Strongly dichroic: sapphire-blue one way, near-colorless the other (Ti/Fe) |
| Andalusite | Biaxial (−) | 1.634 | −0.0100 | 0.0158 | 68.5 | Trichroic: red-brown along one direction, pale yellow-green along the other two; face-up a brownish olive |
| Opal | Isotropic | 1.450 | — | 0.0017 | — | colorless body opal with no play-of-color; for the flashes of a precious opal use Precious Opal (White) or (Black) below |
| Glass (N-BK7) | Isotropic | 1.517 | — | 0.0138 | 64.0 | colorless reference crown glass |
| Glass (F2) | Isotropic | 1.620 | — | 0.0300 | 36.4 | colorless reference dense flint glass |
| Rutile | Uniaxial (+) | 2.613 | +0.2957 | 0.3123 | 8.5 | Pale straw yellow, as lab-grown rutile is (absorption edge at the violet end of the spectrum); this renderer's most extreme birefringence built-in |
| Precious Opal (White) | Isotropic | 1.450 | — | 0.0017 | — | milky, translucent cream-white body (its haze is switched on with the material); the play-of-color of its patches is a choice of the design (Chapter 28) |
| Precious Opal (Black) | Isotropic | 1.450 | — | 0.0017 | — | dark neutral body, which makes the play-of-color stand out (Chapter 28) |
| Labradorite | Biaxial (+) | 1.5625 (indices 1.559, 1.5625, 1.567) | +0.0080 | 0.012 (by construction) | — | translucent olive-grey body (its light haze is switched on with the material), lamellae parallel to the table by default; the schiller, blue with some green, is a choice of the design, fixed in the crystal (Chapter 28) |
| Moonstone | Biaxial (−) | 1.523 (indices 1.518, 1.523, 1.526) | −0.0080 | 0.012 (by construction) | — | colorless body; the adularescence is the Adularescence inclusion preset (Chapter 27) |
| Titanite (Sphene) | Biaxial (+), 2V 27° | 1.894 (indices 1.888, 1.894, 2.011) | +0.1226 | 0.0577 (nβ) | — | yellow-green; trichroic: X nearly colorless, Y yellow-green, Z golden; Z lies in the table |

**Notes**

- Sapphire and Ruby share the same host-mineral optics (dispersion and
  birefringence), differing only in color. The same is true for
  Aquamarine and Morganite (both beryl; Aquamarine additionally carries the
  c-axis-in-the-table-plane orientation its row describes, Morganite the
  default orientation), and for Quartz, Amethyst, Citrine, and Ametrine (all
  the same silica host).
- Quartz (and, sharing its optics, Amethyst, Citrine and
  Ametrine) and Rutile have a wavelength-dependent curve for the
  extraordinary ray, and so do Sapphire, Ruby (Malitson and Dodge), Synthetic
  Moissanite (Wang) and Benitoite (a Cauchy curve for each ray). Alexandrite and
  Chrysoberyl (Yellow) have curves for the alpha and gamma rays (Walling).
  Every other birefringent built-in uses a constant offset. Both the CPU tracer
  and the GPU kernels evaluate the curves (Chapter 29, "Curves for the
  Extraordinary Ray").
- Chrysoberyl (Yellow) takes the published curves of Alexandrite, which gives
  it a total birefringence of 0.0076.
- Labradorite's B–G of 0.012 is a placeholder borrowed from moonstone; no measured value was found.
  Both entries are built from that figure. Titanite's
  0.0577 is its nβ curve (0.064 for nγ). The three opals' F–C figure is 0.0010, deliberately
  minimal.
- Titanite (Sphene) and Rutile carry birefringence and dispersion far beyond
  the other built-ins: Titanite uses measured indices and Rutile a Fresnel
  solve (see the `built_in_material_rutile` doc comment in
  `crates/pleochroma/src/optics/materials/rutile.rs`).
- The body-color strengths follow photographs of cut stones of the kind sold
  today. Aquamarine, Morganite, Topaz, Tanzanite, Peridot, Tsavorite, Demantoid,
  Pyrope and Spinel are lighter and less saturated than the measured crystals
  they were fitted to; Tourmaline and Citrine keep the measured crystal's color
  at the strength of the 2026-10-27 defaults. Several richer colors remain
  available as Color Variants, for example Aquamarine's Santa Maria, Topaz's
  Swiss and London and Peridot's Deep Green.

## Gem Data

Typical values for each species, as the Stone tab's identity card (SG, Mohs and Toughness chips and
the cleavage line) and the **Material Reference** window (Chapter 6) show them. Mohs is the hardness range; Toughness is GIA's word for resistance to
breaking; Cleavage, Parting and Fracture are given with Miller indices where the Handbook of
Mineralogy gives them; SG is the representative specific gravity with the range in brackets (a single
published figure has no range). "not recorded" means no gemological source gives the value, and
"not graded" means no published toughness grade exists for the material; the program leaves both
empty rather than guess. The two SG figures marked "derived" are computed from cited figures; see
"The Derived Basis" below. `pleochroma-cli materials --markdown gem-data` prints this table
from the program's own data.

| Material | Mohs | Toughness | Cleavage | Parting | Fracture | SG (range) |
|---|---|---|---|---|---|---|
| Diamond | 10 | Good to exceptional (by direction) | Perfect octahedral {111}, 4 dir. | — | Conchoidal | 3.52 (3.50–3.53) |
| Sapphire | 9 | Excellent | None | Basal {0001}, rhombohedral {10-11} | Uneven to conchoidal | 4.00 (3.99–4.10) |
| Ruby | 9 | Excellent | None | Basal {0001}, rhombohedral {10-11} | Uneven to conchoidal | 4.00 (3.99–4.10) |
| Emerald | 7.5–8 | Fair to good | Imperfect basal {0001} | — | Conchoidal to uneven | 2.76 (2.67–2.78) |
| Zircon | 6–7.5 | Fair to good | Indistinct {110}, {111} | — | Conchoidal | 4.65 (3.90–4.73) |
| Alexandrite | 8.5 | Excellent | Distinct {110}, imperfect {010}, poor {001} | — | Uneven to conchoidal | 3.73 (3.68–3.78) |
| Topaz | 8 | Poor | Perfect basal {001} | — | Subconchoidal to uneven | 3.53 (3.49–3.57) |
| Spinel | 7.5–8 | Good | None | Octahedral {111} | Conchoidal, uneven to splintery | 3.60 (3.58–3.61) |
| Quartz | 7 | Good | Poor rhombohedral {10-11}, rarely seen | Rhombohedral | Conchoidal | 2.65 (2.65–2.66) |
| Tourmaline | 7–7.5 | Fair | Very poor {11-20}, {10-11} | — | Uneven to conchoidal | 3.06 (2.82–3.32) |
| Tanzanite | 6–7 | Poor to fair | Perfect {010}, imperfect {100} | — | Uneven to conchoidal | 3.35 (3.15–3.36) |
| Synthetic Moissanite | 9.25 | Excellent | Poor {0001} | — | Conchoidal | 3.22 (3.21–3.22) |
| Cubic Zirconia | 8–8.5 | Good | None | — | Conchoidal | 5.80 (5.60–6.00) |
| Aquamarine | 7.5–8 | Good | Imperfect basal {0001} | — | Conchoidal to uneven | 2.72 (2.68–2.74) |
| Morganite | 7.5–8 | Good | Imperfect basal {0001} | — | Conchoidal to uneven | 2.86 (2.80–2.91), derived |
| Chrysoberyl (Yellow) | 8.5 | Good to excellent | Distinct {110}, imperfect {010}, poor {001} | — | Uneven to conchoidal | 3.73 (3.70–3.78) |
| Amethyst | 7 | Good | Poor rhombohedral {10-11} | Rhombohedral | Conchoidal | 2.65 (2.65–2.66) |
| Citrine | 7 | Good | Poor rhombohedral {10-11} | Rhombohedral | Conchoidal | 2.65 (2.65–2.66) |
| Ametrine | 7 | Good | Poor rhombohedral {10-11} | Rhombohedral | Conchoidal | 2.65 (2.65–2.66) |
| Pyrope Garnet | 7–7.5 | Fair to good | None | — | Conchoidal | 3.74 (3.65–3.87) |
| Almandine Garnet | 7–7.5 | Fair to good | None | {110} | Subconchoidal | 4.19 (3.95–4.30) |
| Spessartine Garnet | 7–7.5 | Fair to good | None | — | Uneven to conchoidal | 4.18 (4.12–4.20) |
| Color-Change Garnet (Pyrope-Spessartine) | 7–7.5 | Fair to good | None | — | Conchoidal to uneven | 3.92 (3.83–4.00), derived |
| Grossular Garnet (Tsavorite) | 6.5–7.5 | Fair to good | None | {110}, rarely seen | Uneven to conchoidal | 3.61 (3.55–3.73) |
| Andradite Garnet (Demantoid) | 6.5–7 | Fair to good | None | — | Uneven to conchoidal | 3.86 (3.80–3.90) |
| Peridot | 6.5–7 | Fair to good | Imperfect {010}, {100} | — | Conchoidal | 3.34 |
| YAG | 8.25 | Good | None | — | Conchoidal to uneven | 4.55 (4.55–4.65) |
| GGG | 6.5 | Fair to good | not recorded | — | not recorded | 7.05 (7.00–7.09) |
| Benitoite | 6–6.5 | not graded | Poor {10-11} | — | Conchoidal | 3.65 (3.65–3.68) |
| Andalusite | 6.5–7.5 | Fair to good | Good {110}, poor {100} | — | Uneven to subconchoidal | 3.15 (3.13–3.16) |
| Opal | 5–6.5 | Very poor to fair | None (amorphous) | — | Conchoidal | 2.09 (1.90–2.30) |
| Glass (N-BK7) | about 5.5; HK 610 (Knoop) | not graded | None (amorphous) | — | Conchoidal | 2.51 |
| Glass (F2) | about 5.5; HK 420 (Knoop) | not graded | None (amorphous) | — | Conchoidal | 3.60 |
| Rutile | 6–6.5 | Poor to fair | Good {110}, moderate {100} | — | Conchoidal to uneven | 4.25 |
| Precious Opal (White) | 5–6.5 | Very poor to fair | None | — | Conchoidal | 2.09 (1.90–2.30) |
| Precious Opal (Black) | 5–6.5 | Very poor to fair | None | — | Conchoidal | 2.09 (1.90–2.30) |
| Labradorite | 6–6.5 | Poor | Perfect {001}, good {010} (about 90°), distinct {110} | — | Uneven to conchoidal | 2.69 (2.68–2.71) |
| Moonstone | 6–6.5 | Poor | Perfect {001} and {010} | {100}, {110}, {1-10}, {-201} | Conchoidal to uneven | 2.58 (2.56–2.59) |
| Titanite (Sphene) | 5–5.5 | not graded | Good {110} | {221} (twinning) | Subconchoidal | 3.53 (3.48–3.60) |

The care notes of each material are on the Material Reference's details page and in `pleochroma-cli materials NAME`;
they are not repeated here.

## Model Values and Published Figures

The optics table above prints the program's model values: n_D and the dispersion come from each
material's own index curve. Three entries are marked so, because the figure shown is the model's and a
published range or figure differs from it.

- **Tanzanite.** The model's middle index is **nβ 1.694** (model value). A value of 1.701
  would lie above GIA's measured range and is not used. GIA gives a refractive index of
  **1.691–1.700** and a birefringence of **0.008–0.013** for gem tanzanite. One measured stone, in
  GIA's Gübelin collection (no. 34129), reads **1.690 / 1.702**, biaxial positive, birefringence
  **0.012**. The three principal indices of gem tanzanite are not given by any source this appendix can
  cite, so none is printed as a literature value. The model's birefringence (γ − α) is 0.0090, inside
  GIA's range. The model's dispersion is B–G 0.0298 and F–C 0.0174; no F–C figure for tanzanite is citable.
- **Peridot.** The model's middle index is **nβ 1.669** (model value). The model's α index is
  1.654; no source gives the three indices of gem peridot, so it is not used as n_D. GIA gives a
  refractive index of **1.65–1.69** and a birefringence of **0.035–0.038** for gem peridot; the model's
  0.0360 lies inside it. The Handbook of Mineralogy's indices for forsterite (1.635 / 1.651 / 1.670) are
  those of the magnesium end-member, and are not cited for gem peridot. The model's dispersion is B–G
  0.0198 and F–C 0.0116; no F–C figure for peridot is citable.
- **GGG.** n_D **1.970** (Gems & Gemology 1997, Table 1; the model gives 1.9701). That table's dispersion
  for GGG, **0.045**, is not an F–C figure: the model's curve gives **B–G 0.0445** and F–C 0.0255.

**Published dispersion values.** Gems & Gemology 1997, Table 1 (see Sources) prints a dispersion for
three of the synthetics, beside the model's B–G value:

| Material | Table 1 | Model B–G |
|---|---|---|
| Diamond | 0.044 | 0.0444 |
| Synthetic Moissanite | 0.104 | 0.1117 |
| Synthetic Rutile | 0.330 | 0.3123 |

Table 1 does not name the line pair for its "Dispersion" column. This appendix **assumes it is B–G**,
which is how every other gemological reference prints 0.044 for diamond; it is an assumption, not a
statement of the source. The same table's figures for synthetic corundum (0.018, the model's Sapphire
0.0181), synthetic spinel (0.020, the model's Spinel 0.0202), YAG (0.028, 0.0272) and GGG (0.045, 0.0445)
agree with the model within 0.001, and cubic zirconia's 0.058–0.066 brackets the model's 0.0604.
Synthetic moissanite and rutile are the two entries whose curve is farther from the printed figure.

## The Derived Basis

A figure is **cited** when a source prints it. It is **derived** when the program computed it from
cited figures, and then it is shown with the word "Derived" and its working, never as a source's own
number. Two specific gravities are derived, and every input of each is cited:

- **Morganite 2.86.** GIA prints only a range, 2.80 to 2.91. The representative is the midpoint of
  that range: "Derived: midpoint of the cited range 2.80-2.91."
- **Color-Change Garnet 3.92 (3.83–4.00).** Two Gems & Gemology studies of color-change
  pyrope-spessartine give 3.83–3.88 and 3.93–4.00. The range is the two merged and the representative
  is its midpoint: "Derived: range merged from two Gems & Gemology studies (3.83-3.88 and 3.93-4.00);
  representative is its midpoint."

A value taken over from the species or from the end-members of a mixed stone carries a basis note
instead: "Taken from the end-members pyrope and spessartine." (Color-Change Garnet), "Values of the
opal species." (the two Precious Opals) and "Values of the chrysoberyl species (from alexandrite)."
(Chrysoberyl, Yellow).

## Sources

Every Gem Data value names its source. A value that only a retail page, a hobbyist table or a search
snippet gives is not stored. The sources are:

| Source | Address or location |
|---|---|
| GIA gem encyclopedia care page of each gem (sapphire, ruby, emerald, zircon, alexandrite, topaz, spinel, tourmaline, tanzanite, aquamarine, morganite, amethyst, citrine, ametrine, garnet, peridot, opal, moonstone) | `https://www.gia.edu/<gem>-care-cleaning` |
| GIA gem encyclopedia page of a gem (specific gravity, refractive index and Mohs of aquamarine, morganite, peridot, moonstone, tanzanite, garnet) | `https://www.gia.edu/<gem>` |
| GIA, Gem Cleaning and Display Chart, revised October 2018 (GCC1018) | `https://store.gia.edu/products/gem-cleaning-and-display-chart` |
| Nassau, McClure, Elen and Shigley, "Synthetic Moissanite: A New Diamond Substitute", Gems & Gemology, Winter 1997, pp. 260–275, Table 1 (GG97). The table cites the GIA Gem Reference Guide (1988), which was not available and is cited only through this paper. | Gems & Gemology, Winter 1997 |
| Anthony, Bideaux, Bladh and Nichols, Handbook of Mineralogy, one page for each mineral (HoM) | `https://www.handbookofmineralogy.org/pdfs/<mineral>.pdf` |
| webmineral.com, one page for each mineral (WM) | `http://webmineral.com/data/<Mineral>.shtml` |
| International Gem Society, gem articles (IGS), for care where GIA has no page and for specific gravity | `https://www.gemsociety.org/article/<gem>-jewelry-and-gemstone-information/` and the pages for moonstone, pyrope, almandine, spessartine, tsavorite, grossular and color-change garnet |
| International Gem Society, "Specific Gravity Values of Selected Gems" (IGS-T) | `https://www.gemsociety.org/article/select-gems-ordered-density/` |
| Schott, optical glass datasheets (2023); the F2 values come from the F2HT sheet, which has the same n_d and V_d | `https://media.schott.com/api/public/content/38cbbe876d324e03b1881d33f3d26635` |
| Wikipedia, as a last resort and for synthetics only | `https://en.wikipedia.org/wiki/<page>` |
| GIA, gem encyclopedia pages "Tanzanite" and "Peridot", and the GIA Gübelin collection record for the tanzanite no. 34129 (refractive index and birefringence ranges above) | `https://www.gia.edu/tanzanite`, `https://www.gia.edu/peridot`, `https://gia.edu/doc/zoisite-tanzanite-34129.pdf` |

`pleochroma-cli materials --markdown sources` prints the list of the sources each of the 39 entries cites,
with its address, and the notes below.

**Which source wins when they differ.** Mohs: the GIA care page, widened by the Handbook of Mineralogy
where it differs (spinel: GIA 8, Handbook 7.5–8, shown 7.5–8); for a synthetic, GG97; a lone outlier
is not used. Toughness: the GIA care page, otherwise the GIA chart, otherwise GG97 for a synthetic,
otherwise "not graded". Cleavage, parting and fracture: the Handbook of Mineralogy, with webmineral or
Wikipedia only where the Handbook has no page (opal, labradorite, the synthetics). Care: GIA.

**Notes where sources differ.**

- **Emerald, Zircon:** the GIA chart's toughness range is wider (emerald poor to good; heat-treated
  zircon poor to fair); the care page's grade is shown.
- **Alexandrite:** GIA reports no cleavage; the Handbook's cleavage of chrysoberyl is shown. The GIA chart
  grades chrysoberyl good to excellent.
- **Tanzanite, Peridot:** webmineral indexes the cleavage on a different axis setting ({001} for tanzanite;
  {001} and {010} for peridot); the Handbook's setting is shown.
- **Synthetic Moissanite:** GG97 found no cleavage in a hammer test; the Handbook's poor {0001} is shown.
  The Handbook gives a Mohs of 9.5 for the natural mineral; the table shows GG97's 9.25.
- **YAG:** Wikipedia gives a Mohs of 8.5; GG97's 8.25 is shown.
- **Opal:** GIA's chart grades opal poor to fair with a Mohs of 4.5–6; the care page's very poor to fair
  and 5–6.5 govern. The specific gravity of 2.09 (1.90–2.30) is webmineral's; IGS-T prints 1.25–2.23 for
  porous material and is not used.
- **Labradorite:** webmineral's Mohs of 7 is a lone outlier against 6–6.5 everywhere else and is not used.
- **Pyrope, Almandine:** the Handbook's densities (3.582 and 4.318) are those of the pure end-members and
  are not representative of a gem; the gem values 3.74 and 4.19 are shown.
- **Aquamarine, Morganite, Moonstone, Spessartine, Peridot:** wider or whole-species ranges
  (the beryl species 2.63–2.97; IGS's morganite 2.71–2.90 and aquamarine 2.66–2.80; orthoclase 2.55–2.63;
  the whole spessartine species 3.80–4.25; the olivine series up to fayalite for peridot) are notes, not
  the figures shown.
- **Chrysoberyl (Yellow):** the Alexandrite row keeps its own 3.68–3.78; 3.68 is in neither the Handbook nor
  IGS-T, so the Chrysoberyl row uses 3.70–3.78.
- **Glass (F2):** the Schott catalog has no plain F2 sheet, and its N-F2 is a different, lead-free glass,
  so the F2HT sheet (n_d 1.62004, V_d 36.37) is the one used.

## Next Steps

This is the final appendix. Return to the manual's [overview](README.md) to
find any other chapter.
