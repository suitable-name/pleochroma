//! The built-in specific-gravity table, keyed by `GemMaterial::name`.
//!
//! Provenance and the cited ranges of Zircon and Tourmaline are described in
//! [`crate::material`]'s module doc.

/// How a specific-gravity figure was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SgBasis {
    /// A source prints the representative figure.
    Cited,
    /// Computed from cited figures. The text is the user-facing basis note, which names the
    /// computation and the figures it rests on. A derived figure is never shown as a source's own
    /// number.
    Derived(&'static str),
}

/// One material's specific gravity.
///
/// A representative point figure for [`crate::yield_metrics::carat_weight`], plus the
/// real low/high bounds it simplifies away for a species with a genuine compositional
/// range (see [`crate::material`]'s module doc). `range` equals `(representative,
/// representative)` when published sources agree to within rounding, not because no
/// natural variation exists at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpecificGravity {
    /// The single number [`crate::yield_metrics::carat_weight`] multiplies by,
    /// absent a per-design override.
    pub representative: f64,
    /// `(low, high)` -- the cited real-world spread this representative figure
    /// simplifies. Always `low <= representative <= high`.
    pub range: (f64, f64),
    /// Whether a source prints the representative figure or it is computed from cited figures.
    pub basis: SgBasis,
}

/// Looks up a built-in specific gravity by the name
/// `pleochroma::optics::materials::GemMaterial::name` uses for that preset.
///
/// Matched EXACTLY (case-insensitively only -- e.g. `"diamond"` still resolves,
/// but "My Blue Sapphire" does not, unlike
/// [`pleochroma::optics::materials::GemMaterial::by_name`]'s own substring
/// fallback; see [`super::built_in_material_by_exact_name`]'s doc comment for the
/// same distinction on the refractive-index side).
///
/// The covered names are the table inside this function; provenance is in
/// [`crate::material`]'s module doc. `None` for any other name, notably "Garnet", which
/// is a group name and not a built-in preset.
#[must_use]
pub fn built_in_specific_gravity(material_name: &str) -> Option<SpecificGravity> {
    // Sources: Handbook of Mineralogy / IGS (see module doc comment), except where
    // noted. Zircon and Tourmaline carry a real compositional range (see the Ranges section
    // of [`crate::material`]'s module doc); their representative is a deliberate choice
    // within that range, not an average.
    //
    // Source keys in the comments below: HoM = Handbook of Mineralogy, measured density unless
    // noted; WM = webmineral.com; GIA-E = GIA gem encyclopedia facts box; IGS-T = the International
    // Gem Society table "Specific Gravity Values of Selected Gems"; GG97 = Gems & Gemology, Winter
    // 1997, Table 1 (Synthetic Moissanite article); GG26 = Gems & Gemology 62(2), Summer 2026
    // (glass-ceramics, hydrostatic SG of 60 samples).
    const TABLE: &[(&str, f64, (f64, f64))] = &[
        ("Diamond", 3.52, (3.50, 3.53)),
        // Sapphire & Ruby: same corundum host lattice, same SG.
        ("Sapphire", 4.00, (3.99, 4.10)),
        ("Ruby", 4.00, (3.99, 4.10)),
        // Emerald-specific beryl figure (beryl overall runs wider, 2.63-2.92).
        ("Emerald", 2.76, (2.67, 2.78)),
        // Representative leans toward high (crystalline) zircon (4.65) to match
        // this crate's Zircon optical entry (gem-trade "starlite" pairing).
        ("Zircon", 4.65, (3.90, 4.73)),
        ("Alexandrite", 3.73, (3.68, 3.78)),
        // OH-rich/F-rich ends of the solid solution shift SG slightly.
        ("Topaz", 3.53, (3.49, 3.57)),
        ("Spinel", 3.60, (3.58, 3.61)),
        ("Quartz", 2.65, (2.65, 2.66)),
        // Representative (3.06) is elbaite's own midpoint (the species modeled
        // optically); the cited range spans the whole dravite/schorl family since a
        // cutter may not know which tourmaline species their rough is.
        ("Tourmaline", 3.06, (2.82, 3.32)),
        ("Tanzanite", 3.35, (3.15, 3.36)),
        ("Synthetic Moissanite", 3.22, (3.21, 3.22)),
        // Synthetic; SG varies with stabilizer content.
        ("Cubic Zirconia", 5.80, (5.60, 6.00)),
        // Synthetic rutile: the standard reference value.
        ("Rutile", 4.25, (4.25, 4.25)),
        // Yellow chrysoberyl: representative from HoM chrysoberyl (3.75) and the Alexandrite
        // row's own figure (3.73); range IGS-T "Chrysoberyl (Cat's eye, Alexandrite)" 3.70-3.78.
        // The Alexandrite row's low end 3.68 is in neither HoM nor IGS-T, so this row uses 3.70.
        ("Chrysoberyl (Yellow)", 3.73, (3.70, 3.78)),
        // Quartz varieties: HoM quartz D(meas.) 2.65 (2.66 is D(calc.)); IGS-T quartz 2.65.
        ("Amethyst", 2.65, (2.65, 2.66)),
        ("Citrine", 2.65, (2.65, 2.66)),
        ("Ametrine", 2.65, (2.65, 2.66)),
        // HoM andradite D(meas.) 3.8-3.9, D(calc.) 3.859.
        ("Andradite Garnet (Demantoid)", 3.86, (3.80, 3.90)),
        // Synthetic. GG97 Table 1 prints 4.55; range from IGS-T 4.55-4.65.
        ("YAG", 4.55, (4.55, 4.65)),
        // Synthetic. GG97 Table 1 prints 7.05; range from IGS-T 7.00-7.09.
        ("GGG", 7.05, (7.00, 7.09)),
        // HoM benitoite D(meas.) 3.65, D(calc.) 3.68; IGS-T 3.64-3.68.
        ("Benitoite", 3.65, (3.65, 3.68)),
        // HoM andalusite D(meas.) 3.13-3.16.
        ("Andalusite", 3.15, (3.13, 3.16)),
        // HoM titanite D(meas.) 3.48-3.60; 3.53 is HoM's bracketed calculated value, IGS-T 3.52-3.54.
        ("Titanite (Sphene)", 3.53, (3.48, 3.60)),
        // Opal, all three entries: WM opal "1.9 - 2.3, Average = 2.09". IGS-T prints a different
        // range for opal (1.25-2.23, footnoted, includes porous material); WM is kept.
        ("Opal", 2.09, (1.90, 2.30)),
        ("Precious Opal (White)", 2.09, (1.90, 2.30)),
        ("Precious Opal (Black)", 2.09, (1.90, 2.30)),
        // WM labradorite "2.68 - 2.71, Average = 2.69"; IGS-T 2.65-2.75 (wider, consistent).
        ("Labradorite", 2.69, (2.68, 2.71)),
        // Schott datasheet N-BK7, density 2.51 g/cm3.
        ("Glass (N-BK7)", 2.51, (2.51, 2.51)),
        // Schott datasheet F2HT (nd 1.62004, vd 36.37), density 3.60 g/cm3. The catalog has no
        // plain F2 sheet and its N-F2 page is a different, lead-free glass, so F2HT is the sheet used.
        ("Glass (F2)", 3.60, (3.60, 3.60)),
        // GIA-E moonstone 2.58; IGS moonstone page and IGS-T 2.56-2.59. HoM orthoclase (the
        // whole species) 2.55-2.63 is wider.
        ("Moonstone", 2.58, (2.56, 2.59)),
        // Gem values, not the pure end-member (HoM pyrope 3.582): WM pyrope average 3.74 (WM range
        // 3.65-3.84); IGS pyrope page 3.65-3.87.
        ("Pyrope Garnet", 3.74, (3.65, 3.87)),
        // Gem values, not the pure end-member (HoM almandine 4.318, above every gem range found):
        // WM almandine average 4.19 (WM range 4.09-4.31); IGS almandine page 3.95-4.30.
        ("Almandine Garnet", 4.19, (3.95, 4.30)),
        // WM spessartine 4.18; IGS spessartite "gems usually 4.12-4.20" (whole species 3.80-4.25);
        // HoM spessartine 4.190.
        ("Spessartine Garnet", 4.18, (4.12, 4.20)),
        // IGS tsavorite page 3.55-3.73 with mean 3.61; HoM grossular 3.594; IGS-T 3.56-3.73.
        ("Grossular Garnet (Tsavorite)", 3.61, (3.55, 3.73)),
        // GIA-E aquamarine 2.72; range IGS-T aquamarine 2.68-2.74 (the IGS aquamarine page prints
        // the wider 2.66-2.80). HoM beryl, the whole species, 2.63-2.97 with 2.7 average.
        ("Aquamarine", 2.72, (2.68, 2.74)),
        // Derived (see DERIVED): GIA-E morganite prints the range 2.80-2.91 only, and no source
        // prints a single figure. The IGS morganite page prints 2.71-2.90.
        ("Morganite", 2.86, (2.80, 2.91)),
        // GIA-E peridot 3.34, a single value, stored as both ends of the range. The IGS ranges
        // span the whole olivine series up to fayalite and are not a gem-peridot range; HoM
        // forsterite 3.275 is the pure magnesium end-member.
        ("Peridot", 3.34, (3.34, 3.34)),
        // Derived (see DERIVED): two Gems & Gemology studies of color-change pyrope-spessartine,
        // Fall 2017 GemNews (SG 3.83-3.88) and Spring 2017 GemNews (SG 3.93-4.00). No source
        // gives a species range.
        (
            "Color-Change Garnet (Pyrope-Spessartine)",
            3.92,
            (3.83, 4.00),
        ),
        // Derived (see DERIVED): GG26 measured 2.85-3.42 over all 60 samples (Table 1) and prints
        // no single figure; 3.0 is the mean of its three faceted glass-ceramics (Table 2: 3.06,
        // 2.93, 2.97). The trade figure 3.5-4.0 (the manufacturer) lies above every measurement.
        ("Nanosital", 3.0, (2.85, 3.42)),
    ];
    // Rows whose representative is computed from cited figures, with the note shown beside it.
    const DERIVED: &[(&str, &str)] = &[
        (
            "Morganite",
            "Derived: midpoint of the cited range 2.80-2.91.",
        ),
        (
            "Color-Change Garnet (Pyrope-Spessartine)",
            "Derived: range merged from two Gems & Gemology studies (3.83-3.88 and 3.93-4.00); \
             representative is its midpoint.",
        ),
        (
            "Nanosital",
            "Derived: mean of the three faceted glass-ceramics in Gems & Gemology 62(2), 2026 \
             (3.06, 2.93, 2.97), 2.99, rounded to 3.0; the study measured 2.85-3.42.",
        ),
    ];
    TABLE
        .iter()
        .find(|(name, ..)| name.eq_ignore_ascii_case(material_name))
        .map(|&(name, representative, range)| SpecificGravity {
            representative,
            range,
            basis: DERIVED
                .iter()
                .find(|(derived, _)| *derived == name)
                .map_or(SgBasis::Cited, |&(_, note)| SgBasis::Derived(note)),
        })
}
