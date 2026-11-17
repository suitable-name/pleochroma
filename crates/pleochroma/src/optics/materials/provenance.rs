//! Where the optical data of each built-in material comes from.
//!
//! Each record names the source of the material's dispersion curve and of its absorption
//! spectrum, in the words of the comments that sit on the material's builder. Nothing here
//! feeds the optics: the records exist so a details page can show the citations.

/// The sources of one built-in material's optical data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpticsProvenance {
    /// The source of the refractive-index (dispersion) curve, with the fit it is.
    pub dispersion: &'static str,
    /// The source of the absorption spectrum, or the statement that the material is
    /// colorless.
    pub absorption: &'static str,
    /// Whether the absorption spectrum, or a ray of it, is an estimate or a tuned stand-in
    /// instead of a measurement.
    pub absorption_estimated: bool,
}

/// The shared dispersion source of the quartz host (Quartz, Amethyst, Citrine, Ametrine).
const QUARTZ_DISPERSION: &str = "G. Ghosh, Opt. Commun. 163, 95-102 (1999), ordinary-ray \
    Sellmeier fit (refractiveindex.info, SiO2: Ghosh-o), valid 0.198-2.0531 um; extraordinary \
    ray from the same paper (SiO2: Ghosh-e). Checked against a manufacturer (pmoptics.com) \
    quartz spec sheet at 630 nm and 1550 nm";

/// The shared dispersion source of the corundum host (Sapphire, Ruby).
const CORUNDUM_DISPERSION: &str = "I.H. Malitson & M.J. Dodge, J. Opt. Soc. Am. 62, 1405A \
    (1972), ordinary-ray Sellmeier fit (refractiveindex.info, Al2O3: Malitson-o), valid \
    0.2-5.0 um; extraordinary ray from the same paper (Al2O3: Malitson-e)";

/// The shared dispersion source of the beryl host (Aquamarine, Morganite): Emerald's curve with
/// the constant retargeted.
const BERYL_DISPERSION: &str = "No primary Sellmeier fit for beryl exists: a visible-range \
    Cauchy fit of lower confidence, set by the host lattice. It keeps Emerald's Cauchy b and \
    retargets only the constant a, so that n_d is 1.577 (see Emerald)";

/// The shared dispersion statement of the plain Opal entry.
const OPAL_DISPERSION: &str = "n_d = 1.45 is the standard reference value for common opal's \
    glass-like silica; the dispersion (Delta n(F-C) = 0.001) is a small hand-picked value, not \
    a cited figure";

/// The shared absorption statement of a colorless built-in.
const COLORLESS: &str = "Colorless: no chromophore, empty band set, zero absorption at every \
    wavelength";

/// One record per built-in material, keyed by the material's name.
static RECORDS: [(&str, OpticsProvenance); 39] = [
    (
        "Diamond",
        OpticsProvenance {
            dispersion: "R. Peter, Z. Phys. 15, 358 (1923), two-term Sellmeier fit \
                (refractiveindex.info, Diamond: n (Peter 1923)), valid 0.226-0.760 um",
            absorption: COLORLESS,
            absorption_estimated: false,
        },
    ),
    (
        "Sapphire",
        OpticsProvenance {
            dispersion: CORUNDUM_DISPERSION,
            absorption: "Dubinsky, Stone-Sundberg & Emmett, A Quantitative Description of the \
                Causes of Color in Corundum, Gems & Gemology 56(1), 2020, pp. 2-28: the Fe2+-Ti4+ \
                charge-transfer band (580 nm for E perp c, 700 nm for E par c), fitted to the GIA \
                Fe2+-Ti4+ cross sections",
            absorption_estimated: false,
        },
    ),
    (
        "Ruby",
        OpticsProvenance {
            dispersion: "Same host lattice as Sapphire: Malitson & Dodge (1972) Sellmeier fit, \
                ordinary and extraordinary ray (see Sapphire)",
            absorption: "GIA, \"Application of UV-Vis-NIR Spectroscopy to Gemology\" (Gems & Gemology, \
Winter 2024) and Cr:Al2O3 \
                spectroscopy papers for the Cr3+ bands near 410 nm and 550 nm; pleochroism from \
                Mandarino, American Mineralogist 44, 961 (1959), Table 5; fitted to the GIA Cr3+ \
                and Fe3+ cross sections of the same paper",
            absorption_estimated: false,
        },
    ),
    (
        "Emerald",
        OpticsProvenance {
            dispersion: "No primary Sellmeier fit for beryl exists: a visible-range Cauchy fit \
                of lower confidence. n_d = 1.5791, within the International Gem Society's emerald \
                range 1.57-1.60; the society's dispersion 0.014 is B-G, converted to F-C with the \
                ratio 0.579",
            absorption: "Caltech GRR 3570 emerald spectrum (three-band energy-Gaussian fits per \
                ray); measured o/e dichroism from Wood & Nassau 1968; GIA gemological references \
                for the main bands near 620 nm and 430 nm",
            absorption_estimated: false,
        },
    ),
    (
        "Zircon",
        OpticsProvenance {
            dispersion: "No primary Sellmeier or Cauchy fit exists for zircon: a 2-parameter \
                Cauchy fit of lower confidence. n_o = 1.925 and n_e = 1.984 from the Handbook of \
                Mineralogy (2001), the International Gem Society and the GIA gem-quality \
                reference pair; the B-G interval 0.039 is converted to F-C with the ratio 0.579",
            absorption: "P.E. Fielding (1970) and Nasdala et al., Spectroscopic methods applied \
                to zircon, Reviews in Mineralogy & Geochemistry 53 (2003): dominant line at \
                653 nm; per-axis line positions and widths from Caltech GRR 3106 (Mogok zircon, \
                E perp c and E par c)",
            absorption_estimated: false,
        },
    ),
    (
        "Alexandrite",
        OpticsProvenance {
            dispersion: "J. Walling et al., Tunable alexandrite lasers, IEEE J. Quantum \
                Electron. 16, 1302 (1980), n(alpha) Sellmeier fit (refractiveindex.info, \
                BeAl2O4: Walling-alpha), valid 0.25-2.6 um; the other two principal indices \
                from the same paper's fits",
            absorption: "E.F. Farrell & R.E. Newnham, American Mineralogist 50 (1965), for the \
                trichroism and the Cr3+ band systems; beta and gamma measured on Caltech GRR 874 \
                (Lake Manyara; G. R. Rossman) plus the Fe3+ bands of GRR 171; alpha is not \
                measured and is estimated from gamma",
            absorption_estimated: true,
        },
    ),
    (
        "Topaz",
        OpticsProvenance {
            dispersion: "No primary Sellmeier or Cauchy fit exists for topaz: a 2-parameter \
                Cauchy fit of lower confidence. n_d = 1.627178 lies within the International Gem \
                Society's cited range 1.61-1.638; its B-G interval 0.0141 is converted to F-C \
                with the ratio 0.579",
            absorption: "K. Nassau, Gemstone Enhancement (2nd ed., Butterworth-Heinemann, 1994), \
                for the color center of London and Sky Blue topaz; alpha and beta measured on the \
                electron-irradiated Caltech GRR 825c; gamma is not measured and is taken equal to \
                beta",
            absorption_estimated: true,
        },
    ),
    (
        "Spinel",
        OpticsProvenance {
            dispersion: "W.J. Tropf & M.E. Thomas, Magnesium Aluminum Oxide (Spinel), Handbook \
                of Optical Constants of Solids III (1991), as tabulated by refractiveindex.info \
                (MgAl2O4: Tropf), valid 0.35-5.5 um",
            absorption: "K. Nassau, The Physics and Chemistry of Color, 2nd ed., Ch. 7 (Cr3+ on \
                the Al3+ site, red spinel); Caltech JQ-13 spectrum (G. R. Rossman; natural red \
                spinel, 1.52 mm, unpolarised); Vasiliev's synthetic Cr spinel gives the same \
                peak ratio",
            absorption_estimated: false,
        },
    ),
    (
        "Quartz",
        OpticsProvenance {
            dispersion: QUARTZ_DISPERSION,
            absorption: "Colorless rock crystal: no visible-range chromophore, empty band sets, \
                zero absorption at every wavelength",
            absorption_estimated: false,
        },
    ),
    (
        "Tourmaline",
        OpticsProvenance {
            dispersion: "No primary Sellmeier/Cauchy fit for tourmaline exists: a lower-confidence \
                2-parameter Cauchy fit. n_d = 1.639405, within the International Gem Society's \
                cited elbaite range; its dispersion 0.017 is B-G, converted to F-C with the \
                ratio 0.579",
            absorption: "Caltech GRR 1935, the small green crystal from Usakos, Namibia \
                (3.645 mm), fitted per ray at 0.34 times the crystal; \
                dichroism after Mattson & Rossman, Phys. Chem. Minerals 14, 163 (1987) and \
                Rossman & Mattson, Am. Mineral. 71, 599 (1986)",
            absorption_estimated: false,
        },
    ),
    (
        "Tanzanite",
        OpticsProvenance {
            dispersion: "No primary Sellmeier/Cauchy fit for zoisite exists: a lower-confidence \
                2-parameter Cauchy fit for the dispersion shape. n_beta(D) = 1.6943 from \
                Hurlbut 1969 (minimum deviation, Na light); the International Gem Society's \
                dispersion 0.030 is B-G, converted to F-C with the ratio 0.579",
            absorption: "C.S. Hurlbut, American Mineralogist 54, 702 (1969), citing B.W. \
                Anderson (1968), for the trichroism and the 595, 528 and 455 nm bands; \
                Schwarzinger, Ulatowski & Rossman (V3+ in zoisite); fitted to the three \
                polarised Caltech spectra of zoisite GRR 2292 (blue zone, Arusha, Tanzania), \
                with the alpha ray scaled to the single-crystal ratio of GRR 1265 and GRR 3045",
            absorption_estimated: false,
        },
    ),
    (
        "Synthetic Moissanite",
        OpticsProvenance {
            dispersion: "S. Wang et al., Laser Photonics Rev. 7, 831 (2013), 6H-SiC \
                ordinary-ray fit (refractiveindex.info, SiC: Wang-6H-o), valid 0.4358-2.325 um; \
                extraordinary ray from the companion fit (SiC: Wang-6H-e); corroborated by \
                Singh, Potopowicz, Van Uitert & Wemple, Appl. Phys. Lett. 19, 53 (1971)",
            absorption: COLORLESS,
            absorption_estimated: false,
        },
    ),
    (
        "Cubic Zirconia",
        OpticsProvenance {
            dispersion: "D.L. Wood & K. Nassau, Refractive index of cubic zirconia stabilized \
                with yttria, Appl. Opt. 21, 2978-2981 (1982), as tabulated by \
                refractiveindex.info (ZrO2: Wood), valid 0.361-5.135 um",
            absorption: COLORLESS,
            absorption_estimated: false,
        },
    ),
    (
        "Aquamarine",
        OpticsProvenance {
            dispersion: BERYL_DISPERSION,
            absorption: "Caltech GRR 1488 aquamarine (5.531 mm, polarised), mixed 0.6 with 0.4 \
                of the Greenish variant, whose Fe3+ part is estimated; mix and strength chosen \
                against dealer photos",
            absorption_estimated: true,
        },
    ),
    (
        "Morganite",
        OpticsProvenance {
            dispersion: "Same beryl host as Aquamarine (both n_d = 1.577): bit-identical to \
                Aquamarine's curve (see Aquamarine)",
            absorption: "Mn3+ band near 567 nm (GIA Gems & Gemology, Fall 2024); no digitised \
                morganite spectrum exists, so the shape is borrowed from the same ion on the same \
                site (Caltech red beryl GRR 602), folded with the Peach variant's Fe3+ (GRR 309)",
            absorption_estimated: true,
        },
    ),
    (
        "Chrysoberyl (Yellow)",
        OpticsProvenance {
            dispersion: "Alexandrite's Walling et al. (1980) Sellmeier shape with only the \
                leading constant retargeted, so that n_d is this variety's 1.746 (see \
                Alexandrite)",
            absorption: "Farrell & Newnham, Am. Mineral. 50 (1972), for the Fe3+ bands; beta and \
                gamma from the Caltech spectra of GRR 171 (Minas Gerais, 1.193 mm); alpha was \
                never measured and is estimated as the mean of beta and gamma; mixed with a \
                part of the Alexandrite Cr3+ spectrum (GRR 874)",
            absorption_estimated: true,
        },
    ),
    (
        "Amethyst",
        OpticsProvenance {
            dispersion: QUARTZ_DISPERSION,
            absorption: "Cox, J. Phys. C 10, 4631 (1977), and the Caltech Ametrine page, for the \
                Fe4+ hole center; o-ray fitted to the Caltech Anahi (Bolivia) amethyst spectrum \
                (6.17 mm slab); no polarised amethyst spectrum exists, so the e-ray is estimated \
                from the o-ray bands",
            absorption_estimated: true,
        },
    ),
    (
        "Citrine",
        OpticsProvenance {
            dispersion: QUARTZ_DISPERSION,
            absorption: "Fe3+ in small iron-oxide aggregates (Caltech quartz page); shape from a \
                16-point citrine alpha(lambda) table (citrine_annealed_amethyst.csv) that is \
                secondary and unverified, so only its shape is claimed; no polarised citrine \
                spectrum exists, so the o- and e-ray sets are equal",
            absorption_estimated: true,
        },
    ),
    (
        "Ametrine",
        OpticsProvenance {
            dispersion: "Citrine's optics, bit for bit: the quartz curve of Ghosh (1999) (see \
                Quartz)",
            absorption: "The chromophore catalogue's quartz iron recipes, per millimetre: the \
                amethyst half and the citrine half (at about 0.4 times the amethyst half's \
                iron)",
            absorption_estimated: false,
        },
    ),
    (
        "Pyrope Garnet",
        OpticsProvenance {
            dispersion: "No primary Sellmeier/Cauchy fit exists for garnet-group gemstones: a \
                lower-confidence 2-parameter Cauchy fit solved from n_d = 1.714 and the \
                dispersion figure 0.022, treated as B-G and converted to F-C with the ratio \
                0.579. The source of the two figures is not named in the material definition",
            absorption: "Caltech GRR 1055 pyrope spectrum (Moses Rock, Utah, 0.840 mm) with the \
                Mn2+ of the Spessartine entry at 0.6, refitted; the mix matches the median hue \
                of dealer photos",
            absorption_estimated: false,
        },
    ),
    (
        "Almandine Garnet",
        OpticsProvenance {
            dispersion: "No primary Sellmeier/Cauchy fit exists for garnet-group gemstones: a \
                lower-confidence 2-parameter Cauchy fit solved from n_d = 1.790 and the \
                dispersion figure 0.024, treated as B-G and converted to F-C with the ratio \
                0.579. The source of the two figures is not named in the material definition",
            absorption: "Caltech GRR 1776 almandine spectrum (0.213 mm) with the Mn2+ of the \
                Spessartine entry at 0.3, refitted; the mix matches the median hue of dealer \
                photos",
            absorption_estimated: false,
        },
    ),
    (
        "Spessartine Garnet",
        OpticsProvenance {
            dispersion: "No primary Sellmeier/Cauchy fit exists for garnet-group gemstones: a \
                lower-confidence 2-parameter Cauchy fit solved from n_d = 1.800 and the \
                dispersion figure 0.027, treated as B-G and converted to F-C with the ratio \
                0.579. The source of the two figures is not named in the material definition",
            absorption: "Caltech GRR 43 spessartine spectrum (Rutherford Mine, Virginia; \
                2.165 mm), fitted with energy Gaussians; the sample is a near end-member, so it \
                is Fe-poor",
            absorption_estimated: false,
        },
    ),
    (
        "Color-Change Garnet (Pyrope-Spessartine)",
        OpticsProvenance {
            dispersion: "No primary fit: a lower-confidence 2-parameter Cauchy fit. n_d = 1.760 \
                (typical range 1.755-1.765; the linear pyrope-spessartine mix gives 1.757); the \
                dispersion 0.025 B-G lies between pyrope's 0.022 and spessartine's 0.027 and is \
                converted to F-C with the ratio 0.579",
            absorption: "Caltech GRR 2225 Tunduru spectrum, with its 581 nm band at 0.25 and the \
                whole at 1.2",
            absorption_estimated: false,
        },
    ),
    (
        "Grossular Garnet (Tsavorite)",
        OpticsProvenance {
            dispersion: "No primary Sellmeier/Cauchy fit exists for garnet-group gemstones: a \
                lower-confidence 2-parameter Cauchy fit solved from n_d = 1.734 and the \
                dispersion figure 0.028, treated as B-G and converted to F-C with the ratio \
                0.579. The source of the two figures is not named in the material definition",
            absorption: "Caltech tsavorite GRR 229 spectrum (Tanzania, 0.758 mm), energy-Gaussian \
                fit; peaks scaled to a lower vanadium content to match the median hue of dealer \
                photos",
            absorption_estimated: false,
        },
    ),
    (
        "Andradite Garnet (Demantoid)",
        OpticsProvenance {
            dispersion: "No primary Sellmeier/Cauchy fit exists for garnet-group gemstones: a \
                lower-confidence 2-parameter Cauchy fit solved from n_d = 1.887 and the \
                dispersion figure 0.057, treated as B-G and converted to F-C with the ratio \
                0.579. The source of the two figures is not named in the material definition",
            absorption: "A mix of the Russian demantoid slab (Caltech GIA 2952, 2.5 mm, for its \
                Cr3+) and the Topazolite variant (measured Fe3+ andradite, Caltech GRR 48), \
                refitted",
            absorption_estimated: false,
        },
    ),
    (
        "Peridot",
        OpticsProvenance {
            dispersion: "No primary Sellmeier fit available: a lower-confidence Cauchy fit for \
                n_beta(D) = 1.669 (alpha 1.654 and gamma 1.690 after Deer, Howie & Zussman, An \
                Introduction to the Rock-Forming Minerals) and the dispersion figure 0.020, \
                treated as B-G and converted to F-C with the ratio 0.579",
            absorption: "Caltech Mineral Spectroscopy Server San Carlos olivine spectra GRR 418 \
                alpha, beta and gamma (G. R. Rossman); GIA's peridot characterisation for the \
                473 and 490 nm Fe2+ bands",
            absorption_estimated: false,
        },
    ),
    (
        "YAG",
        OpticsProvenance {
            dispersion: "D.E. Zelmon, D.L. Small & R. Page, Refractive-index measurements of \
                undoped yttrium aluminum garnet from 0.4 to 5.0 um, Appl. Opt. 37, 4933-4935 \
                (1998), as tabulated by refractiveindex.info (Y3Al5O12: Zelmon)",
            absorption: "Colorless: undoped YAG, the laser-host reference composition; empty \
                band set, zero absorption at every wavelength",
            absorption_estimated: false,
        },
    ),
    (
        "GGG",
        OpticsProvenance {
            dispersion: "D.L. Wood & K. Nassau, Optical properties of gadolinium gallium garnet, \
                Appl. Opt. 29, 3704-3707 (1990), Table I, the average of four crystals \
                (transparent 0.36-6.0 um)",
            absorption: "Colorless: undoped GGG; empty band set, zero absorption at every \
                wavelength",
            absorption_estimated: false,
        },
    ),
    (
        "Benitoite",
        OpticsProvenance {
            dispersion: "Per-ray two-term Cauchy fits from the gemological table of Laurs, \
                Rohtert & Gray, Gems & Gemology 33(3), 166-187 (1997), Table 1: n_omega 1.757, \
                n_epsilon 1.804, B-G dispersion 0.039 (o-ray) and 0.046 (e-ray) (the latter two \
                are Payne's 1939 figures). Secondary confidence: a gemological table, no primary \
                Sellmeier fit",
            absorption: "Caltech Mineral Spectroscopy Server (G. R. Rossman), sample GRR 561-D4, \
                dark-blue zone, The Gem Mine, San Benito County, 0.997 mm, polarised; one \
                Gaussian in energy per ray fitted to the measured band; the cause of the color \
                is not established, and the strength is tuned by eye",
            absorption_estimated: false,
        },
    ),
    (
        "Andalusite",
        OpticsProvenance {
            dispersion: "No primary Sellmeier fit is available: a lower-confidence 2-parameter \
                Cauchy fit from n_d = 1.634 and the dispersion figure 0.016, treated as B-G and \
                converted to F-C with the ratio 0.579. The source of the two figures is not \
                named in the material definition",
            absorption: "Caltech spectra of M. N. Taran (colour-zoned Fe-Ti andalusite, Brazil, \
                light zone, FeO 0.35 wt%), mixed 0.65 with 0.35 of the Greener variant (an \
                estimated Mn3+ part), refitted; the mix matches the median hue of dealer photos",
            absorption_estimated: false,
        },
    ),
    (
        "Opal",
        OpticsProvenance {
            dispersion: OPAL_DISPERSION,
            absorption: "Colorless neutral reference: body color varies too widely for one \
                representative band set; play-of-color is not modelled",
            absorption_estimated: false,
        },
    ),
    (
        "Glass (N-BK7)",
        OpticsProvenance {
            dispersion: "SCHOTT optical glass data sheet (N-BK7), Sellmeier coefficients as \
                tabulated by refractiveindex.info (SCHOTT-optical: N-BK7)",
            absorption: "Colorless glass: empty band set, zero absorption at every wavelength",
            absorption_estimated: false,
        },
    ),
    (
        "Glass (F2)",
        OpticsProvenance {
            dispersion: "SCHOTT optical glass data sheet (F2), Sellmeier coefficients as \
                tabulated by refractiveindex.info (SCHOTT-optical: F2)",
            absorption: "Colorless glass: empty band set, zero absorption at every wavelength",
            absorption_estimated: false,
        },
    ),
    (
        "Rutile",
        OpticsProvenance {
            dispersion: "DeVore, J. Opt. Soc. Am. 41, 416 (1951), as tabulated on \
                refractiveindex.info (TiO2 (Titanium dioxide): Rutile phase), ordinary and \
                extraordinary ray",
            absorption: "One band at 350 nm, wide enough to tail into the violet; tuned, not a \
                cited coefficient; its peak keeps cut rutile near-colorless to a pale straw \
                yellow, as dealer and Commons photos of cut rutile",
            absorption_estimated: true,
        },
    ),
    (
        "Precious Opal (White)",
        OpticsProvenance {
            dispersion: "The silica of precious opal has the optics of the plain Opal entry: \
                n_d = 1.45 and a small hand-picked Delta n(F-C) of 0.001, not a cited figure",
            absorption: "A faint cream from one weak band at the blue end: an aesthetic choice, \
                matched to the cream to white body of dealer photos of white opals",
            absorption_estimated: true,
        },
    ),
    (
        "Precious Opal (Black)",
        OpticsProvenance {
            dispersion: "The silica of precious opal has the optics of the plain Opal entry: \
                n_d = 1.45 and a small hand-picked Delta n(F-C) of 0.001, not a cited figure",
            absorption: "A uniform grey absorption of three broad bands over the visible range: \
                an aesthetic choice",
            absorption_estimated: true,
        },
    ),
    (
        "Labradorite",
        OpticsProvenance {
            dispersion: "The dispersion is not verified for labradorite: it borrows moonstone's \
                B-G of 0.012 as a placeholder of the right size for a 1.56 feldspar. Indices \
                from the ranges of Wikipedia, Labradorite (n_alpha 1.554-1.563, n_beta \
                1.559-1.568, n_gamma 1.562-1.573); the entry's 1.559 / 1.5625 / 1.567 is an \
                estimate inside them",
            absorption: "An olive-grey body: a neutral grey of three broad bands, a yellow edge \
                and a weak red tail, matched to dealer photos (median L* 52, a* -1.9, b* +7.4); \
                no pleochroism of the body was found",
            absorption_estimated: true,
        },
    ),
    (
        "Moonstone",
        OpticsProvenance {
            dispersion: "Wikipedia, Moonstone (gemstone): n 1.518-1.526, birefringence \
                0.005-0.010, dispersion 0.012 (taken as B-G); the entry's 1.518 / 1.523 / 1.526 \
                is an estimate inside the range",
            absorption: "Colorless: the empty isotropic tensor",
            absorption_estimated: false,
        },
    ),
    (
        "Titanite (Sphene)",
        OpticsProvenance {
            dispersion: "Busz, Neues Jahrbuch fur Mineralogie, Beilage-Band 5, 330 (1887), as \
                tabulated in Dana, System of Mineralogy, 6th ed. (1892), p. 714: St. Gotthard \
                titanite, nine measured indices at 670.8, 589.3 and 535.0 nm, fitted with \
                one-pole Sellmeier curves that share one pole; extrapolated below 535 nm",
            absorption: "Rossman, Caltech Mineral Spectroscopy Server, Titanite1478 \
                (yellow-green, Madagascar, 3.91 mm, E par b and E par c), a digitised figure; \
                beta is E par b as measured; gamma and alpha are estimated from E par c",
            absorption_estimated: true,
        },
    ),
];

/// The sources of the optical data of the built-in material called `name` (matched without
/// regard to ASCII letter case), or `None` when no built-in has that name.
#[must_use]
pub fn optics_provenance(name: &str) -> Option<OpticsProvenance> {
    RECORDS
        .iter()
        .find(|(record_name, _)| record_name.eq_ignore_ascii_case(name))
        .map(|&(_, record)| record)
}
