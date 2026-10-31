# Color variants research R5: diamond, measured data (lead, 2026-10-27)

Owner asked the lead to research measured diamond spectra directly. This note adds to R4
section 4 (`R4_quartz_diamond_opal_synthetics_glass.md`), which holds the band models; read both.

## 1. Result in one paragraph

No public, numeric (downloadable) visible absorption spectrum of a colored natural diamond was
found: GIA, the Caltech server (GIA spectra as images only) and the MSA review give figures,
and no dataset turned up on figshare or Zenodo. What IS measured and published are band
positions and widths, a few absorption cross sections and calibrations, and decompositions of
measured spectra into fitted components. The diamond variants are therefore built from these
measured parameters; their strengths are set by the target face-up color and are `estimated`
(owner decision: estimated variants ship, marked).

## 2. Measured parameters, with sources

| Defect / color | Measured parameter | Source |
|---|---|---|
| N3 (Cape yellow) | ZPL 415.2 nm; absorption cross section of the ZPL about 2.5e-15 cm^2, of the 370-390 nm vibronic band about 1e-15 cm^2, measured on one N3-rich natural plate with [N3] about 6e14 cm^-3 | Optics Letters 49(1), 137 (2024), "Photo-physical characteristics of color N3-center in diamond studied via UV femtosecond-laser pumped luminescence" (abstract; https://opg.optica.org/ol/abstract.cfm?uri=ol-49-1-137, https://pubmed.ncbi.nlm.nih.gov/38134171/). The authors note no calibration of alpha(415 nm) against N3 concentration exists for natural diamonds. |
| N2, Cape lines | 478 nm (N2) with associated absorptions at 451, 463 nm; minor 435, 426 nm | Breeding et al., G&G Summer 2020 (R4 4.2); MSA review (below) |
| Ns0 (C centre, canary) | 270 nm band: Gaussian, centre 268-272 nm, RMS width 13-27 nm, cross section 4.51 +/- 0.35 cm^-1 ppm^-1 (natural log; 1.96 decadic; 1.11e-17 cm^2), calibrated against EPR on six CVD samples | Luo et al., Appl. Phys. Lett. 121, 064002 (2022), arXiv:2207.14278, eq. (1) and section 2 |
| CVD brown / vacancy clusters | measured spectra decompose into a 360 nm Gaussian (vacancy clusters), a 520 nm Gaussian (NVH0), and a "ramp" R x lambda^-3, over an electronic-grade baseline | Luo et al. 2022, section 2, Fig. 1 |
| Natural brown | "brown absorption continuum" from vacancy clusters (40-60 vacancies), absorption gradually increasing toward short wavelengths | Eaton-Magana et al., G&G Winter 2018, "Band of Many Colors", p. 360 (https://www.gia.edu/doc/WN18-natural-color-pink-purple-red-brown-diamonds.pdf) |
| Pink (550 nm band) | fitted on 50 diamonds (mixed Gaussian/Lorentzian): centre 545-565 nm, FWHM 60-100 nm; orangy pink to pink 545-550 nm / 60-80 nm; purple-pink 555-560 nm / 70-100 nm; examples: Fancy Deep pinkish orange 548 nm / 62 nm (with H3 and N3), Fancy Vivid purple 556 nm / 105 nm (no H3/N3); a smaller, narrower UV band at about 390 nm accompanies it | Eaton-Magana et al. 2018, p. 360-363 and Fig. 9C |
| Pink tone | faint colors show only the 550 nm band; fancy and deeper tones add N3 and/or H3 (ZPL 503.2 nm) | same, p. 361 |
| Purple | about 98 % of unmodified purples show no significant H3/N3; saturation follows the 550 nm band strength | same, Box A |
| GR1 (green) | ZPL 741 nm, vibronic band reaching about 575 nm; the band height is measured at 2.0 eV (620 nm) in the vacancy formula of the 2003 annealing study; ZPL calibration (1.2) x 1e-16 meV cm^2 per vacancy as used by Newsom et al. 2025 (original reference not confirmed here) | MSA review (below); Physica B 2003 annealing study (search result, abstract); arXiv:2509.06517 |
| Boron (blue) | only infrared calibrations: [B0] (ppm) = 0.0350 x H(2800 cm^-1) (Collins, as used in arXiv:2310.12587); [B0] = 5.53e-4 x integrated 2802 cm^-1 area (Fisher et al. 2009); natural IIb 25-1500 ppb (GIA 2018, R4 4.6). The visible continuum is published only qualitatively. | as named |

MSA review: "Diamond Spectroscopy, Defect Centers, Color, and Treatments", Reviews in Mineralogy
and Geochemistry 88 (2022) 637 ff., https://msaweb.org/wp-content/uploads/2022/07/REV088C12.pdf
(text read in full: Fig. 7 N3 absorption, Fig. 32 Cape vs canary, Fig. 34 550 nm band, Fig. 35
boron, Figs. 36-38 GR1; all figures, no tables of band parameters).

## 3. What changes against R4 section 4

* N3: the band strengths in R4 4.2 are `secondary` (the repo Report). The measured ZPL : vibronic
  cross-section ratio (2.5 : 1) fixes the relative height of the 415 nm line against the
  370-390 nm sideband; keep N2 relative to N3 as estimated. Overall strength by target color.
* GR1: R4 4.7 centres the band at 680 nm. The calibration's reference point at 2.0 eV
  (620 nm) and the vibronic envelope 575-741 nm put the band maximum nearer 620-650 nm; the fit
  should use that range and a ZPL line at 741 nm.
* Pink: R4 4.5 already uses the GIA measured centre and width (548 nm / 62 nm and
  556 nm / 105 nm). Offer two variants: Pink (548 / 62, with the 390 nm companion) and
  Purple-Pink (556 / 105, no N3/H3).
* Brown: use the measured decomposition form (360 nm band + lambda^-3 ramp) rather than the
  patent mid-range only; strength by target; `estimated` for natural stones.
* Canary: the measured 270 nm band does not reach the visible; the visible continuum stays
  `estimated` (R4 4.3).
* Blue: `estimated` shape (R4 4.6).

## 4. Recommended diamond variants

Cape Yellow, Canary, Brown, Pink, Purple-Pink, Blue, Green. All cubic (isotropic, o = e,
`pleochroic = false`). All `estimated = true` for the strengths; sources name the measured
parameters each rests on.
