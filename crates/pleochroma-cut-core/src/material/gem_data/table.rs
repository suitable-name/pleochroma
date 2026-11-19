//! The data of the built-in gem reference table: one entry per `GemMaterial::name`.
//!
//! Every value is typical for the species, carries the sources it comes from, and keeps the
//! source's own words where sources differ (a [`SourceNote`]). A field no gemological or
//! mineralogical source gives is empty (`None`), never filled in: the toughness of the glasses,
//! Benitoite, Titanite and Nanosital, the cleavage of Nanosital, and the cleavage and fracture of
//! GGG.
//!
//! Source keys: `gia_c("x")` is the GIA care page of gem x, `GIA_CH` the GIA Gem Cleaning and
//! Display Chart, `GG97` the Gems & Gemology 1997 table, `hom("x")` the Handbook of Mineralogy
//! page, `wm("X")` webmineral, `igs("x")` the International Gem Society article, `wp("X")`
//! Wikipedia, `GG26` the Gems & Gemology 2026 glass-ceramics study, `RJ16` the manufacturer's
//! text in Russian Jeweller 2016.

use super::{
    CareTopic, Cited, Cleavage, CleavageQuality, Confidence, CrystalForm, Fracture, GemBasis,
    GemData, GemField, GemSource, MaterialOrigin, SourceNote, Toughness, ToughnessRange,
};

/// A value with its sources and confidence: `cited!(High, value, [source, ...])`.
macro_rules! cited {
    ($confidence:ident, $value:expr, [$($source:expr),+ $(,)?]) => {
        Cited {
            value: $value,
            sources: &[$($source),+],
            confidence: Confidence::$confidence,
        }
    };
}

/// A source note: `note!(Field, "text", [source, ...])`.
macro_rules! note {
    ($field:ident, $text:expr, [$($source:expr),+ $(,)?]) => {
        SourceNote {
            field: GemField::$field,
            text: $text,
            sources: &[$($source),+],
        }
    };
}

/// One care note: `care!(Topic, High, "text", [source, ...])`.
macro_rules! care {
    ($topic:ident, $confidence:ident, $text:expr, [$($source:expr),+ $(,)?]) => {
        (CareTopic::$topic, cited!($confidence, $text, [$($source),+]))
    };
}

/// Care notes of the three corundum-like gems that share the GIA sapphire wording
/// (`corundum_care!("sapphire")`).
macro_rules! corundum_care {
    ($slug:literal) => {
        &[
            care!(Heat, High, "Stable to heat and light.", [gia_c($slug)]),
            care!(
                Chemicals,
                High,
                "Stable to common chemicals; boric acid etches the surface, and fracture-filled, cavity-filled and dyed stones are attacked even by mild acids.",
                [gia_c($slug)]
            ),
            care!(
                Cleaning,
                High,
                "Steam and ultrasonic are usually safe for untreated, heat-treated and lattice-diffusion stones; filled or dyed stones take a damp cloth only.",
                [gia_c($slug)]
            ),
        ]
    };
}

/// Care notes of beryl's aquamarine and morganite (`beryl_care!("aquamarine")`).
macro_rules! beryl_care {
    ($slug:literal) => {
        &[
            care!(
                Heat,
                High,
                "Avoid heat; the color is stable in light.",
                [gia_c($slug)]
            ),
            care!(
                Chemicals,
                High,
                "Attacked by hydrofluoric acid.",
                [gia_c($slug)]
            ),
        ]
    };
}

/// Care notes of amethyst, citrine and ametrine (`quartz_care!("amethyst")`).
macro_rules! quartz_care {
    ($slug:literal) => {
        &[
            care!(
                ThermalShock,
                High,
                "Abrupt temperature change can fracture it.",
                [gia_c($slug)]
            ),
            care!(
                Light,
                High,
                "Color can fade with prolonged exposure to intense light.",
                [gia_c($slug)]
            ),
            care!(Heat, High, "Do not expose it to heat.", [gia_c($slug)]),
            care!(
                Chemicals,
                High,
                "Attacked by hydrofluoric acid, ammonium fluoride and alkaline solutions.",
                [gia_c($slug)]
            ),
            care!(
                Cleaning,
                High,
                "Steam cleaning is not recommended.",
                [gia_c($slug)]
            ),
        ]
    };
}

/// Care notes of the garnets that share the GIA garnet wording (`garnet_care!(High)`).
macro_rules! garnet_care {
    ($confidence:ident) => {
        &[
            care!(
                Chemicals,
                $confidence,
                "Attacked by hydrofluoric acid.",
                [gia_c("garnet")]
            ),
            care!(
                Cleaning,
                $confidence,
                "Steam cleaning is not recommended.",
                [gia_c("garnet")]
            ),
            care!(ThermalShock, $confidence, "Avoid thermal shock.", [GIA_CH]),
        ]
    };
}

/// Care notes of opal and the precious opals (`opal_care!(High)`).
macro_rules! opal_care {
    ($confidence:ident) => {
        &[
            care!(
                Heat,
                $confidence,
                "Heat from intense light can cause crazing.",
                [gia_c("opal")]
            ),
            care!(
                ThermalShock,
                $confidence,
                "High heat or sudden temperature change can fracture it.",
                [gia_c("opal")]
            ),
            care!(
                Chemicals,
                $confidence,
                "Attacked by hydrofluoric acid and caustic alkaline solutions.",
                [gia_c("opal")]
            ),
            care!(
                Cleaning,
                $confidence,
                "Clean only with warm soapy water.",
                [gia_c("opal")]
            ),
            care!(
                Cleaning,
                $confidence,
                "Some opal is porous, so avoid liquids.",
                [GIA_CH]
            ),
        ]
    };
}

/// The GIA care page of a gem (`gia_c("topaz")` is `gia.edu/topaz-care-cleaning`).
const fn gia_c(slug: &'static str) -> GemSource {
    GemSource::GiaCare(slug)
}

/// A Handbook of Mineralogy page, by PDF file stem.
const fn hom(stem: &'static str) -> GemSource {
    GemSource::HandbookOfMineralogy(stem)
}

/// A webmineral page, by file stem.
const fn wm(stem: &'static str) -> GemSource {
    GemSource::Webmineral(stem)
}

/// An International Gem Society article, by the gem part of its slug.
const fn igs(gem: &'static str) -> GemSource {
    GemSource::Igs(gem)
}

/// A Wikipedia article, by title with underscores.
const fn wp(title: &'static str) -> GemSource {
    GemSource::Wikipedia(title)
}

/// A Schott optical glass datasheet.
const fn schott(glass: &'static str) -> GemSource {
    GemSource::Schott(glass)
}

/// The GIA Gem Cleaning and Display Chart, rev. Oct 2018.
const GIA_CH: GemSource = GemSource::GiaChart2018;

/// Gems & Gemology, Winter 1997, Table 1.
const GG97: GemSource = GemSource::GemsGemology1997;

/// Gems & Gemology 62(2), Summer 2026, the glass-ceramics (Nanosital) study.
const GG26: GemSource = GemSource::GemsGemology2026;

/// The manufacturer's Nanosital text in Russian Jeweller, 13 July 2016.
const RJ16: GemSource = GemSource::RussianJeweller2016;

/// A single toughness grade.
const fn grade(toughness: Toughness) -> ToughnessRange {
    ToughnessRange {
        low: toughness,
        high: toughness,
        directional: false,
    }
}

/// A range of toughness grades, weaker grade first.
const fn grades(low: Toughness, high: Toughness) -> ToughnessRange {
    ToughnessRange {
        low,
        high,
        directional: false,
    }
}

/// A cleavage direction.
const fn cleave(
    quality: CleavageQuality,
    indices: &'static str,
    name: Option<&'static str>,
) -> Cleavage {
    Cleavage {
        quality,
        form: CrystalForm { indices, name },
        directions: None,
    }
}

/// A crystal form for a parting.
const fn form(indices: &'static str, name: Option<&'static str>) -> CrystalForm {
    CrystalForm { indices, name }
}

/// No cleavage.
const NO_CLEAVAGE: &[Cleavage] = &[];

/// Conchoidal fracture.
const CONCHOIDAL: &[Fracture] = &[Fracture::Conchoidal];

/// "Uneven to conchoidal".
const UNEVEN_CONCHOIDAL: &[Fracture] = &[Fracture::Uneven, Fracture::Conchoidal];

/// "Conchoidal to uneven".
const CONCHOIDAL_UNEVEN: &[Fracture] = &[Fracture::Conchoidal, Fracture::Uneven];

/// Imperfect basal cleavage of beryl.
const BERYL_CLEAVAGE: &[Cleavage] = &[cleave(CleavageQuality::Imperfect, "{0001}", Some("basal"))];

/// The chrysoberyl cleavage of Alexandrite and Chrysoberyl (Yellow).
const CHRYSOBERYL_CLEAVAGE: &[Cleavage] = &[
    cleave(CleavageQuality::Distinct, "{110}", None),
    cleave(CleavageQuality::Imperfect, "{010}", None),
    cleave(CleavageQuality::Poor, "{001}", None),
];

/// The quartz cleavage of Quartz, Amethyst, Citrine and Ametrine.
const QUARTZ_CLEAVAGE: &[Cleavage] = &[cleave(
    CleavageQuality::Poor,
    "{10-11}",
    Some("rhombohedral"),
)];

/// The quartz parting: a rhombohedral parting without indices in the source.
const QUARTZ_PARTING: &[CrystalForm] = &[form("", Some("rhombohedral"))];

/// The corundum partings of Sapphire and Ruby.
const CORUNDUM_PARTING: &[CrystalForm] = &[
    form("{0001}", Some("basal")),
    form("{10-11}", Some("rhombohedral")),
];

/// The built-in gem reference table: the exact `GemMaterial::name` and its data.
pub(super) static TABLE: &[(&str, GemData)] = &[
    (
        "Diamond",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Diamond",
            mohs: cited!(High, (10.0, 10.0), [hom("diamond"), GG97]),
            knoop: None,
            toughness: Some(cited!(
                High,
                ToughnessRange {
                    low: Toughness::Good,
                    high: Toughness::Exceptional,
                    directional: true,
                },
                [GIA_CH]
            )),
            cleavage: Some(cited!(
                High,
                &[Cleavage {
                    quality: CleavageQuality::Perfect,
                    form: CrystalForm {
                        indices: "{111}",
                        name: Some("octahedral"),
                    },
                    directions: Some(4),
                }],
                [hom("diamond"), wm("Diamond")]
            )),
            parting: None,
            fracture: Some(cited!(High, CONCHOIDAL, [hom("diamond"), wm("Diamond")])),
            care: &[
                care!(
                    ThermalShock,
                    High,
                    "Sudden extreme temperature change can crack it.",
                    [GIA_CH]
                ),
                care!(Cutting, High, "Chips at thin girdles and points.", [GIA_CH]),
            ],
            notes: &[],
            basis: GemBasis::Species,
        },
    ),
    (
        "Sapphire",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Corundum",
            mohs: cited!(
                High,
                (9.0, 9.0),
                [gia_c("sapphire"), hom("corundum"), GIA_CH]
            ),
            knoop: None,
            toughness: Some(cited!(
                High,
                grade(Toughness::Excellent),
                [gia_c("sapphire")]
            )),
            cleavage: Some(cited!(
                High,
                NO_CLEAVAGE,
                [hom("corundum"), gia_c("sapphire")]
            )),
            parting: Some(cited!(High, CORUNDUM_PARTING, [hom("corundum")])),
            fracture: Some(cited!(High, UNEVEN_CONCHOIDAL, [hom("corundum")])),
            care: corundum_care!("sapphire"),
            notes: &[],
            basis: GemBasis::Species,
        },
    ),
    (
        "Ruby",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Corundum",
            mohs: cited!(High, (9.0, 9.0), [gia_c("ruby"), hom("corundum"), GIA_CH]),
            knoop: None,
            toughness: Some(cited!(High, grade(Toughness::Excellent), [gia_c("ruby")])),
            cleavage: Some(cited!(High, NO_CLEAVAGE, [hom("corundum"), gia_c("ruby")])),
            parting: Some(cited!(High, CORUNDUM_PARTING, [hom("corundum")])),
            fracture: Some(cited!(High, UNEVEN_CONCHOIDAL, [hom("corundum")])),
            care: corundum_care!("ruby"),
            notes: &[],
            basis: GemBasis::Species,
        },
    ),
    (
        "Emerald",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Beryl",
            mohs: cited!(High, (7.5, 8.0), [gia_c("emerald"), hom("beryl")]),
            knoop: None,
            toughness: Some(cited!(
                High,
                grades(Toughness::Fair, Toughness::Good),
                [gia_c("emerald")]
            )),
            cleavage: Some(cited!(High, BERYL_CLEAVAGE, [hom("beryl")])),
            parting: None,
            fracture: Some(cited!(High, CONCHOIDAL_UNEVEN, [hom("beryl")])),
            care: &[
                care!(
                    Heat,
                    High,
                    "Heat can extend existing fractures.",
                    [gia_c("emerald")]
                ),
                care!(
                    Chemicals,
                    High,
                    "Light and chemicals can alter the fillers used in surface-reaching fractures.",
                    [gia_c("emerald")]
                ),
                care!(
                    Cleaning,
                    High,
                    "Risky to clean with steam or ultrasonic; use warm soapy water.",
                    [gia_c("emerald")]
                ),
            ],
            notes: &[note!(
                Toughness,
                "Sources differ: the GIA chart grades emerald poor to good; the care guide's fair to good is shown.",
                [GIA_CH]
            )],
            basis: GemBasis::Species,
        },
    ),
    (
        "Zircon",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Zircon",
            mohs: cited!(High, (6.0, 7.5), [gia_c("zircon"), hom("zircon")]),
            knoop: None,
            toughness: Some(cited!(
                High,
                grades(Toughness::Fair, Toughness::Good),
                [gia_c("zircon")]
            )),
            cleavage: Some(cited!(
                High,
                &[
                    cleave(CleavageQuality::Indistinct, "{110}", None),
                    cleave(CleavageQuality::Indistinct, "{111}", None),
                ],
                [hom("zircon")]
            )),
            parting: None,
            fracture: Some(cited!(High, CONCHOIDAL, [hom("zircon")])),
            care: &[
                care!(
                    Cutting,
                    High,
                    "Brittle: avoid hard rubbing and blows; it can abrade.",
                    [gia_c("zircon")]
                ),
                care!(
                    Heat,
                    High,
                    "Heat can alter the color of some zircon.",
                    [gia_c("zircon")]
                ),
                care!(
                    Cleaning,
                    High,
                    "Steam and ultrasonic are not recommended.",
                    [gia_c("zircon")]
                ),
            ],
            notes: &[note!(
                Toughness,
                "Sources differ: the GIA chart grades heat-treated zircon poor to fair; the care guide's fair to good is shown.",
                [GIA_CH]
            )],
            basis: GemBasis::Species,
        },
    ),
    (
        "Alexandrite",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Chrysoberyl",
            mohs: cited!(High, (8.5, 8.5), [gia_c("alexandrite"), hom("chrysoberyl")]),
            knoop: None,
            toughness: Some(cited!(
                High,
                grade(Toughness::Excellent),
                [gia_c("alexandrite")]
            )),
            cleavage: Some(cited!(High, CHRYSOBERYL_CLEAVAGE, [hom("chrysoberyl")])),
            parting: None,
            fracture: Some(cited!(High, UNEVEN_CONCHOIDAL, [hom("chrysoberyl")])),
            care: &[
                care!(
                    Heat,
                    High,
                    "Resists heat, light and common chemicals.",
                    [gia_c("alexandrite")]
                ),
                care!(
                    Cleaning,
                    High,
                    "Steam and ultrasonic are usually safe; fracture-filled stones take warm soapy water only.",
                    [gia_c("alexandrite")]
                ),
            ],
            notes: &[
                note!(
                    Cleavage,
                    "Sources differ: GIA reports no cleavage for alexandrite; the Handbook of Mineralogy's cleavage for chrysoberyl is shown.",
                    [gia_c("alexandrite")]
                ),
                note!(
                    Toughness,
                    "Sources differ: the GIA chart grades chrysoberyl good to excellent; the care guide's excellent is shown.",
                    [GIA_CH]
                ),
            ],
            basis: GemBasis::Species,
        },
    ),
    (
        "Topaz",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Topaz",
            mohs: cited!(High, (8.0, 8.0), [gia_c("topaz"), GIA_CH, hom("topaz")]),
            knoop: None,
            toughness: Some(cited!(High, grade(Toughness::Poor), [gia_c("topaz")])),
            cleavage: Some(cited!(
                High,
                &[cleave(CleavageQuality::Perfect, "{001}", Some("basal"))],
                [hom("topaz")]
            )),
            parting: None,
            fracture: Some(cited!(
                High,
                &[Fracture::Subconchoidal, Fracture::Uneven],
                [hom("topaz")]
            )),
            care: &[
                care!(
                    Cutting,
                    High,
                    "The cleavage needs special care in cutting, polishing and mounting.",
                    [gia_c("topaz")]
                ),
                care!(
                    ThermalShock,
                    High,
                    "High heat or sudden temperature change can cause breaks; a hard blow might split it.",
                    [gia_c("topaz")]
                ),
                care!(
                    Light,
                    High,
                    "Prolonged heat or sunlight can fade some yellow-to-brown, reddish-brown and dark brown colors.",
                    [gia_c("topaz")]
                ),
                care!(
                    Cleaning,
                    High,
                    "Avoid steam and ultrasonic; warm soapy water works best.",
                    [gia_c("topaz")]
                ),
            ],
            notes: &[],
            basis: GemBasis::Species,
        },
    ),
    (
        "Spinel",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Spinel",
            mohs: cited!(High, (7.5, 8.0), [gia_c("spinel"), hom("Spinel")]),
            knoop: None,
            toughness: Some(cited!(High, grade(Toughness::Good), [gia_c("spinel")])),
            cleavage: Some(cited!(High, NO_CLEAVAGE, [hom("Spinel"), wm("Spinel")])),
            parting: Some(cited!(
                High,
                &[form("{111}", Some("octahedral"))],
                [hom("Spinel")]
            )),
            fracture: Some(cited!(
                High,
                &[Fracture::Conchoidal, Fracture::Uneven, Fracture::Splintery],
                [hom("Spinel")]
            )),
            care: &[
                care!(
                    Heat,
                    High,
                    "High heat can fade some colors.",
                    [gia_c("spinel")]
                ),
                care!(
                    Cleaning,
                    High,
                    "Steam and ultrasonic are usually safe, but fractures can be a problem; fracture-filled stones take warm soapy water or a damp cloth only.",
                    [gia_c("spinel")]
                ),
            ],
            notes: &[note!(
                Hardness,
                "Sources differ: the GIA care guide gives 8 and the Handbook of Mineralogy 7.5 to 8; the wider range is shown.",
                [gia_c("spinel"), hom("Spinel")]
            )],
            basis: GemBasis::Species,
        },
    ),
    (
        "Quartz",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Quartz",
            mohs: cited!(High, (7.0, 7.0), [GIA_CH, hom("quartz")]),
            knoop: None,
            toughness: Some(cited!(High, grade(Toughness::Good), [GIA_CH])),
            cleavage: Some(cited!(High, QUARTZ_CLEAVAGE, [hom("quartz")])),
            parting: Some(cited!(High, QUARTZ_PARTING, [hom("quartz")])),
            fracture: Some(cited!(High, CONCHOIDAL, [hom("quartz")])),
            care: &[
                care!(ThermalShock, High, "Avoid thermal shock.", [GIA_CH]),
                care!(
                    Chemicals,
                    High,
                    "Attacked by hydrofluoric acid and alkalis.",
                    [gia_c("amethyst")]
                ),
            ],
            notes: &[note!(
                Cleavage,
                "Rarely seen in practice. The Handbook of Mineralogy lists {10-11}, {01-11}, {10-10} and others; the first is shown.",
                [hom("quartz")]
            )],
            basis: GemBasis::Species,
        },
    ),
    (
        "Tourmaline",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Tourmaline",
            mohs: cited!(High, (7.0, 7.5), [gia_c("tourmaline"), hom("elbaite")]),
            knoop: None,
            toughness: Some(cited!(High, grade(Toughness::Fair), [gia_c("tourmaline")])),
            cleavage: Some(cited!(
                High,
                &[
                    cleave(CleavageQuality::VeryPoor, "{11-20}", None),
                    cleave(CleavageQuality::VeryPoor, "{10-11}", None),
                ],
                [hom("elbaite")]
            )),
            parting: None,
            fracture: Some(cited!(High, UNEVEN_CONCHOIDAL, [hom("elbaite")])),
            care: &[
                care!(
                    ThermalShock,
                    High,
                    "Sudden temperature change can fracture it.",
                    [gia_c("tourmaline")]
                ),
                care!(
                    Heat,
                    High,
                    "High heat can alter the color.",
                    [gia_c("tourmaline")]
                ),
                care!(
                    Cleaning,
                    High,
                    "Steam and ultrasonic are not recommended.",
                    [gia_c("tourmaline")]
                ),
            ],
            notes: &[],
            basis: GemBasis::Species,
        },
    ),
    (
        "Tanzanite",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Zoisite",
            mohs: cited!(High, (6.0, 7.0), [gia_c("tanzanite"), hom("zoisite")]),
            knoop: None,
            toughness: Some(cited!(
                High,
                grades(Toughness::Poor, Toughness::Fair),
                [gia_c("tanzanite")]
            )),
            cleavage: Some(cited!(
                High,
                &[
                    cleave(CleavageQuality::Perfect, "{010}", None),
                    cleave(CleavageQuality::Imperfect, "{100}", None),
                ],
                [hom("zoisite")]
            )),
            parting: None,
            fracture: Some(cited!(High, UNEVEN_CONCHOIDAL, [hom("zoisite")])),
            care: &[
                care!(
                    ThermalShock,
                    High,
                    "Avoid high heat and sudden temperature change.",
                    [gia_c("tanzanite")]
                ),
                care!(
                    Chemicals,
                    High,
                    "Attacked by hydrochloric and hydrofluoric acid.",
                    [gia_c("tanzanite")]
                ),
                care!(
                    Cleaning,
                    High,
                    "Steam and ultrasonic are never recommended.",
                    [gia_c("tanzanite")]
                ),
            ],
            notes: &[note!(
                Cleavage,
                "Sources differ: webmineral gives {001}, a different axis setting; the Handbook of Mineralogy's {010} is shown.",
                [wm("Zoisite")]
            )],
            basis: GemBasis::Species,
        },
    ),
    (
        "Synthetic Moissanite",
        GemData {
            origin: MaterialOrigin::Synthetic,
            species: "Moissanite",
            mohs: cited!(High, (9.25, 9.25), [GG97]),
            knoop: None,
            toughness: Some(cited!(High, grade(Toughness::Excellent), [GG97])),
            cleavage: Some(cited!(
                High,
                &[cleave(CleavageQuality::Poor, "{0001}", None)],
                [hom("Moissanite")]
            )),
            parting: None,
            fracture: Some(cited!(High, CONCHOIDAL, [hom("Moissanite")])),
            care: &[
                care!(
                    Heat,
                    High,
                    "Reportedly stable in air to about 1700 °C.",
                    [GG97]
                ),
                care!(
                    Chemicals,
                    High,
                    "Inert to most chemicals except fluorine, chlorine, molten alkalis and some molten metals.",
                    [GG97]
                ),
            ],
            notes: &[
                note!(
                    Cleavage,
                    "Sources differ: Gems & Gemology found no cleavage in a hammer test; the Handbook of Mineralogy's poor {0001} is shown.",
                    [GG97, hom("Moissanite")]
                ),
                note!(
                    Hardness,
                    "The Handbook of Mineralogy gives Mohs 9.5 for the mineral; the synthetic's 9.25 is shown.",
                    [hom("Moissanite")]
                ),
            ],
            basis: GemBasis::Species,
        },
    ),
    (
        "Cubic Zirconia",
        GemData {
            origin: MaterialOrigin::Synthetic,
            species: "Zirconia",
            mohs: cited!(Medium, (8.0, 8.5), [GG97, wp("Cubic_zirconia")]),
            knoop: None,
            toughness: Some(cited!(Medium, grade(Toughness::Good), [GG97])),
            cleavage: Some(cited!(Medium, NO_CLEAVAGE, [wp("Cubic_zirconia")])),
            parting: None,
            fracture: Some(cited!(Medium, CONCHOIDAL, [wp("Cubic_zirconia")])),
            care: &[],
            notes: &[],
            basis: GemBasis::Species,
        },
    ),
    (
        "Aquamarine",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Beryl",
            mohs: cited!(
                High,
                (7.5, 8.0),
                [gia_c("aquamarine"), GIA_CH, hom("beryl")]
            ),
            knoop: None,
            toughness: Some(cited!(High, grade(Toughness::Good), [GIA_CH])),
            cleavage: Some(cited!(High, BERYL_CLEAVAGE, [hom("beryl")])),
            parting: None,
            fracture: Some(cited!(High, CONCHOIDAL_UNEVEN, [hom("beryl")])),
            care: beryl_care!("aquamarine"),
            notes: &[],
            basis: GemBasis::Species,
        },
    ),
    (
        "Morganite",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Beryl",
            mohs: cited!(High, (7.5, 8.0), [gia_c("morganite"), GIA_CH, hom("beryl")]),
            knoop: None,
            toughness: Some(cited!(High, grade(Toughness::Good), [GIA_CH])),
            cleavage: Some(cited!(High, BERYL_CLEAVAGE, [hom("beryl")])),
            parting: None,
            fracture: Some(cited!(High, CONCHOIDAL_UNEVEN, [hom("beryl")])),
            care: beryl_care!("morganite"),
            notes: &[],
            basis: GemBasis::Species,
        },
    ),
    (
        "Chrysoberyl (Yellow)",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Chrysoberyl",
            mohs: cited!(High, (8.5, 8.5), [hom("chrysoberyl"), GIA_CH]),
            knoop: None,
            toughness: Some(cited!(
                High,
                grades(Toughness::Good, Toughness::Excellent),
                [GIA_CH]
            )),
            cleavage: Some(cited!(High, CHRYSOBERYL_CLEAVAGE, [hom("chrysoberyl")])),
            parting: None,
            fracture: Some(cited!(High, UNEVEN_CONCHOIDAL, [hom("chrysoberyl")])),
            care: &[care!(
                Heat,
                Medium,
                "Resists heat and chemicals.",
                [gia_c("alexandrite")]
            )],
            notes: &[],
            basis: GemBasis::Inferred("Values of the chrysoberyl species (from alexandrite)."),
        },
    ),
    (
        "Amethyst",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Quartz",
            mohs: cited!(High, (7.0, 7.0), [gia_c("amethyst"), hom("quartz")]),
            knoop: None,
            toughness: Some(cited!(High, grade(Toughness::Good), [gia_c("amethyst")])),
            cleavage: Some(cited!(High, QUARTZ_CLEAVAGE, [hom("quartz")])),
            parting: Some(cited!(High, QUARTZ_PARTING, [hom("quartz")])),
            fracture: Some(cited!(High, CONCHOIDAL, [hom("quartz")])),
            care: quartz_care!("amethyst"),
            notes: &[note!(
                Cleavage,
                "Rarely seen in practice. The Handbook of Mineralogy lists {10-11}, {01-11}, {10-10} and others; the first is shown.",
                [hom("quartz")]
            )],
            basis: GemBasis::Species,
        },
    ),
    (
        "Citrine",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Quartz",
            mohs: cited!(High, (7.0, 7.0), [gia_c("citrine"), hom("quartz")]),
            knoop: None,
            toughness: Some(cited!(High, grade(Toughness::Good), [gia_c("citrine")])),
            cleavage: Some(cited!(High, QUARTZ_CLEAVAGE, [hom("quartz")])),
            parting: Some(cited!(High, QUARTZ_PARTING, [hom("quartz")])),
            fracture: Some(cited!(High, CONCHOIDAL, [hom("quartz")])),
            care: quartz_care!("citrine"),
            notes: &[note!(
                Cleavage,
                "Rarely seen in practice. The Handbook of Mineralogy lists {10-11}, {01-11}, {10-10} and others; the first is shown.",
                [hom("quartz")]
            )],
            basis: GemBasis::Species,
        },
    ),
    (
        "Ametrine",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Quartz",
            mohs: cited!(High, (7.0, 7.0), [gia_c("ametrine"), hom("quartz")]),
            knoop: None,
            toughness: Some(cited!(High, grade(Toughness::Good), [gia_c("ametrine")])),
            cleavage: Some(cited!(High, QUARTZ_CLEAVAGE, [hom("quartz")])),
            parting: Some(cited!(High, QUARTZ_PARTING, [hom("quartz")])),
            fracture: Some(cited!(High, CONCHOIDAL, [hom("quartz")])),
            care: quartz_care!("ametrine"),
            notes: &[note!(
                Cleavage,
                "Rarely seen in practice. The Handbook of Mineralogy lists {10-11}, {01-11}, {10-10} and others; the first is shown.",
                [hom("quartz")]
            )],
            basis: GemBasis::Species,
        },
    ),
    (
        "Pyrope Garnet",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Pyrope",
            mohs: cited!(High, (7.0, 7.5), [hom("Pyrope"), GIA_CH]),
            knoop: None,
            toughness: Some(cited!(
                High,
                grades(Toughness::Fair, Toughness::Good),
                [gia_c("garnet")]
            )),
            cleavage: Some(cited!(High, NO_CLEAVAGE, [hom("Pyrope")])),
            parting: None,
            fracture: Some(cited!(High, CONCHOIDAL, [hom("Pyrope")])),
            care: garnet_care!(High),
            notes: &[note!(
                Hardness,
                "The GIA care guide gives only the garnet family range of 6.5 to 7.5; the species range is from the GIA chart and the Handbook of Mineralogy.",
                [gia_c("garnet")]
            )],
            basis: GemBasis::Species,
        },
    ),
    (
        "Almandine Garnet",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Almandine",
            mohs: cited!(High, (7.0, 7.5), [hom("almandine"), GIA_CH]),
            knoop: None,
            toughness: Some(cited!(
                High,
                grades(Toughness::Fair, Toughness::Good),
                [gia_c("garnet")]
            )),
            cleavage: Some(cited!(High, NO_CLEAVAGE, [hom("almandine")])),
            parting: Some(cited!(High, &[form("{110}", None)], [hom("almandine")])),
            fracture: Some(cited!(High, &[Fracture::Subconchoidal], [hom("almandine")])),
            care: garnet_care!(High),
            notes: &[note!(
                Hardness,
                "The GIA care guide gives only the garnet family range of 6.5 to 7.5; the species range is from the GIA chart and the Handbook of Mineralogy.",
                [gia_c("garnet")]
            )],
            basis: GemBasis::Species,
        },
    ),
    (
        "Spessartine Garnet",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Spessartine",
            mohs: cited!(High, (7.0, 7.5), [hom("spessartine"), GIA_CH]),
            knoop: None,
            toughness: Some(cited!(
                High,
                grades(Toughness::Fair, Toughness::Good),
                [gia_c("garnet")]
            )),
            cleavage: Some(cited!(High, NO_CLEAVAGE, [hom("spessartine")])),
            parting: None,
            fracture: Some(cited!(High, UNEVEN_CONCHOIDAL, [hom("spessartine")])),
            care: garnet_care!(High),
            notes: &[note!(
                Hardness,
                "The GIA care guide gives only the garnet family range of 6.5 to 7.5; the species range is from the GIA chart and the Handbook of Mineralogy.",
                [gia_c("garnet")]
            )],
            basis: GemBasis::Species,
        },
    ),
    (
        "Color-Change Garnet (Pyrope-Spessartine)",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Pyrope-spessartine",
            mohs: cited!(
                Medium,
                (7.0, 7.5),
                [hom("Pyrope"), hom("spessartine"), gia_c("garnet")]
            ),
            knoop: None,
            toughness: Some(cited!(
                Medium,
                grades(Toughness::Fair, Toughness::Good),
                [gia_c("garnet")]
            )),
            cleavage: Some(cited!(
                Medium,
                NO_CLEAVAGE,
                [hom("Pyrope"), hom("spessartine")]
            )),
            parting: None,
            fracture: Some(cited!(
                Medium,
                CONCHOIDAL_UNEVEN,
                [hom("Pyrope"), hom("spessartine")]
            )),
            care: garnet_care!(Medium),
            notes: &[],
            basis: GemBasis::Inferred("Taken from the end-members pyrope and spessartine."),
        },
    ),
    (
        "Grossular Garnet (Tsavorite)",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Grossular",
            mohs: cited!(
                High,
                (6.5, 7.5),
                [hom("grossular"), gia_c("garnet"), GIA_CH]
            ),
            knoop: None,
            toughness: Some(cited!(
                High,
                grades(Toughness::Fair, Toughness::Good),
                [gia_c("garnet")]
            )),
            cleavage: Some(cited!(High, NO_CLEAVAGE, [hom("grossular")])),
            parting: Some(cited!(High, &[form("{110}", None)], [hom("grossular")])),
            fracture: Some(cited!(High, UNEVEN_CONCHOIDAL, [hom("grossular")])),
            care: garnet_care!(High),
            notes: &[
                note!(
                    Hardness,
                    "The Handbook of Mineralogy gives 6.5 to 7 for grossular; the GIA range of 6.5 to 7.5, with tsavorite on the harder side, is shown.",
                    [hom("grossular"), gia_c("garnet")]
                ),
                note!(
                    Parting,
                    "The parting is rarely observed.",
                    [hom("grossular")]
                ),
            ],
            basis: GemBasis::Species,
        },
    ),
    (
        "Andradite Garnet (Demantoid)",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Andradite",
            mohs: cited!(High, (6.5, 7.0), [GIA_CH, hom("andradite")]),
            knoop: None,
            toughness: Some(cited!(
                High,
                grades(Toughness::Fair, Toughness::Good),
                [gia_c("garnet")]
            )),
            cleavage: Some(cited!(High, NO_CLEAVAGE, [hom("andradite")])),
            parting: None,
            fracture: Some(cited!(High, UNEVEN_CONCHOIDAL, [hom("andradite")])),
            care: &[
                care!(
                    Cutting,
                    High,
                    "Softer than the other garnets.",
                    [gia_c("garnet")]
                ),
                care!(
                    Chemicals,
                    High,
                    "Attacked by hydrofluoric acid.",
                    [gia_c("garnet")]
                ),
                care!(
                    Cleaning,
                    High,
                    "Steam cleaning is not recommended.",
                    [gia_c("garnet")]
                ),
                care!(
                    Cleaning,
                    High,
                    "Avoid ultrasonic cleaning when liquid inclusions are present.",
                    [GIA_CH]
                ),
                care!(ThermalShock, High, "Avoid thermal shock.", [GIA_CH]),
            ],
            notes: &[
                note!(
                    Hardness,
                    "The GIA care guide gives only the garnet family range of 6.5 to 7.5 and calls demantoid a bit softer; the species range is from the GIA chart and the Handbook of Mineralogy.",
                    [gia_c("garnet")]
                ),
                note!(
                    Care,
                    "The GIA care guide says ultrasonic cleaning is usually safe except for fractured stones; the chart's liquid-inclusion caution is shown.",
                    [gia_c("garnet"), GIA_CH]
                ),
            ],
            basis: GemBasis::Species,
        },
    ),
    (
        "Peridot",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Olivine",
            mohs: cited!(High, (6.5, 7.0), [gia_c("peridot"), hom("forsterite")]),
            knoop: None,
            toughness: Some(cited!(
                High,
                grades(Toughness::Fair, Toughness::Good),
                [gia_c("peridot")]
            )),
            cleavage: Some(cited!(
                High,
                &[
                    cleave(CleavageQuality::Imperfect, "{010}", None),
                    cleave(CleavageQuality::Imperfect, "{100}", None),
                ],
                [hom("forsterite")]
            )),
            parting: None,
            fracture: Some(cited!(High, CONCHOIDAL, [hom("forsterite")])),
            care: &[
                care!(
                    ThermalShock,
                    High,
                    "Rapid or uneven heat can fracture it.",
                    [gia_c("peridot")]
                ),
                care!(
                    Chemicals,
                    High,
                    "Easily attacked by sulfuric acid, less easily by hydrochloric acid.",
                    [gia_c("peridot")]
                ),
                care!(
                    Cleaning,
                    High,
                    "Steam and ultrasonic are not recommended.",
                    [gia_c("peridot")]
                ),
            ],
            notes: &[note!(
                Cleavage,
                "Sources differ: webmineral indexes the cleavage as {001} and {010}, a different axis setting; the Handbook of Mineralogy's {010} and {100} are shown.",
                [wm("Forsterite")]
            )],
            basis: GemBasis::Species,
        },
    ),
    (
        "YAG",
        GemData {
            origin: MaterialOrigin::Synthetic,
            species: "Yttrium aluminum garnet",
            mohs: cited!(Medium, (8.25, 8.25), [GG97]),
            knoop: None,
            toughness: Some(cited!(Medium, grade(Toughness::Good), [GG97])),
            cleavage: Some(cited!(
                Medium,
                NO_CLEAVAGE,
                [wp("Yttrium_aluminium_garnet")]
            )),
            parting: None,
            fracture: Some(cited!(
                Medium,
                CONCHOIDAL_UNEVEN,
                [wp("Yttrium_aluminium_garnet")]
            )),
            care: &[],
            notes: &[note!(
                Hardness,
                "Wikipedia gives Mohs 8.5; the Gems & Gemology figure is shown.",
                [wp("Yttrium_aluminium_garnet")]
            )],
            basis: GemBasis::Species,
        },
    ),
    (
        "GGG",
        GemData {
            origin: MaterialOrigin::Synthetic,
            species: "Gadolinium gallium garnet",
            mohs: cited!(Medium, (6.5, 6.5), [GG97]),
            knoop: None,
            toughness: Some(cited!(
                Medium,
                grades(Toughness::Fair, Toughness::Good),
                [GG97]
            )),
            cleavage: None,
            parting: None,
            fracture: None,
            care: &[],
            notes: &[],
            basis: GemBasis::Species,
        },
    ),
    (
        "Benitoite",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Benitoite",
            mohs: cited!(High, (6.0, 6.5), [hom("benitoite")]),
            knoop: None,
            toughness: None,
            cleavage: Some(cited!(
                High,
                &[cleave(CleavageQuality::Poor, "{10-11}", None)],
                [hom("benitoite")]
            )),
            parting: None,
            fracture: Some(cited!(High, CONCHOIDAL, [hom("benitoite")])),
            care: &[
                care!(
                    Heat,
                    High,
                    "Heat treatment can destroy stones whose inclusions explode.",
                    [igs("benitoite")]
                ),
                care!(
                    Cleaning,
                    High,
                    "Avoid steam and ultrasonic because of inclusions.",
                    [igs("benitoite")]
                ),
            ],
            notes: &[],
            basis: GemBasis::Species,
        },
    ),
    (
        "Andalusite",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Andalusite",
            mohs: cited!(High, (6.5, 7.5), [hom("andalusite"), GIA_CH]),
            knoop: None,
            toughness: Some(cited!(
                High,
                grades(Toughness::Fair, Toughness::Good),
                [GIA_CH]
            )),
            cleavage: Some(cited!(
                High,
                &[
                    cleave(CleavageQuality::Good, "{110}", None),
                    cleave(CleavageQuality::Poor, "{100}", None),
                ],
                [hom("andalusite")]
            )),
            parting: None,
            fracture: Some(cited!(
                High,
                &[Fracture::Uneven, Fracture::Subconchoidal],
                [hom("andalusite")]
            )),
            care: &[care!(
                Cleaning,
                High,
                "Take extra care with ultrasonic cleaning when liquid inclusions are present.",
                [GIA_CH]
            )],
            notes: &[],
            basis: GemBasis::Species,
        },
    ),
    (
        "Opal",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Opal",
            mohs: cited!(High, (5.0, 6.5), [gia_c("opal"), wm("Opal")]),
            knoop: None,
            toughness: Some(cited!(
                High,
                grades(Toughness::VeryPoor, Toughness::Fair),
                [gia_c("opal"), GIA_CH]
            )),
            cleavage: Some(cited!(High, NO_CLEAVAGE, [wm("Opal")])),
            parting: None,
            fracture: Some(cited!(High, CONCHOIDAL, [wm("Opal")])),
            care: opal_care!(High),
            notes: &[
                note!(
                    Hardness,
                    "Sources differ: the GIA chart gives Mohs 4.5 to 6; the care guide's 5 to 6.5 is shown.",
                    [GIA_CH]
                ),
                note!(
                    Toughness,
                    "Sources differ: the GIA chart grades opal poor to fair; the care guide's very poor to fair is shown.",
                    [GIA_CH]
                ),
            ],
            basis: GemBasis::Species,
        },
    ),
    (
        "Glass (N-BK7)",
        GemData {
            origin: MaterialOrigin::Glass,
            species: "Glass",
            mohs: cited!(Medium, (5.5, 5.5), [wp("Mohs_scale")]),
            knoop: Some(cited!(High, 610, [schott("N-BK7")])),
            toughness: None,
            cleavage: Some(cited!(Medium, NO_CLEAVAGE, [wp("Conchoidal_fracture")])),
            parting: None,
            fracture: Some(cited!(Medium, CONCHOIDAL, [wp("Conchoidal_fracture")])),
            care: &[
                care!(Heat, High, "Glass transition at 557 °C.", [schott("N-BK7")]),
                care!(
                    Chemicals,
                    High,
                    "Acid resistance class SR 1, alkali resistance class AR 2.3.",
                    [schott("N-BK7")]
                ),
            ],
            notes: &[note!(
                Hardness,
                "Mohs 5.5 is the generic figure for glass; Schott gives only the Knoop hardness.",
                [wp("Mohs_scale"), schott("N-BK7")]
            )],
            basis: GemBasis::Species,
        },
    ),
    (
        "Glass (F2)",
        GemData {
            origin: MaterialOrigin::Glass,
            species: "Glass",
            mohs: cited!(Medium, (5.5, 5.5), [wp("Mohs_scale")]),
            knoop: Some(cited!(Medium, 420, [schott("F2HT")])),
            toughness: None,
            cleavage: Some(cited!(Medium, NO_CLEAVAGE, [wp("Conchoidal_fracture")])),
            parting: None,
            fracture: Some(cited!(Medium, CONCHOIDAL, [wp("Conchoidal_fracture")])),
            care: &[
                care!(
                    Cutting,
                    Medium,
                    "Lead flint glass, softer than N-BK7 (Knoop 420 against 610).",
                    [schott("F2HT")]
                ),
                care!(
                    Heat,
                    Medium,
                    "Glass transition at 434 °C.",
                    [schott("F2HT")]
                ),
            ],
            notes: &[note!(
                Hardness,
                "Knoop hardness and glass transition are taken from the F2HT datasheet, which has the same refractive index and Abbe number as F2. Mohs 5.5 is the generic figure for glass.",
                [schott("F2HT"), wp("Mohs_scale")]
            )],
            basis: GemBasis::Species,
        },
    ),
    (
        "Rutile",
        GemData {
            origin: MaterialOrigin::Synthetic,
            species: "Rutile",
            mohs: cited!(Medium, (6.0, 6.5), [hom("rutile"), GG97]),
            knoop: None,
            toughness: Some(cited!(
                Medium,
                grades(Toughness::Poor, Toughness::Fair),
                [GG97]
            )),
            cleavage: Some(cited!(
                Medium,
                &[
                    cleave(CleavageQuality::Good, "{110}", None),
                    cleave(CleavageQuality::Moderate, "{100}", None),
                ],
                [hom("rutile")]
            )),
            parting: None,
            fracture: Some(cited!(Medium, CONCHOIDAL_UNEVEN, [hom("rutile")])),
            care: &[
                care!(
                    Cutting,
                    Medium,
                    "Somewhat vulnerable to scratches; use a protective setting.",
                    [igs("rutile")]
                ),
                care!(
                    Cleaning,
                    Medium,
                    "Clean with a soft brush, mild detergent and warm water.",
                    [igs("rutile")]
                ),
            ],
            notes: &[
                note!(
                    Cleavage,
                    "Cleavage and fracture are those of the natural mineral (Handbook of Mineralogy).",
                    [hom("rutile")]
                ),
                note!(
                    Fracture,
                    "The Handbook of Mineralogy lists conchoidal, subconchoidal and uneven.",
                    [hom("rutile")]
                ),
            ],
            basis: GemBasis::Species,
        },
    ),
    (
        "Precious Opal (White)",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Opal",
            mohs: cited!(Medium, (5.0, 6.5), [gia_c("opal"), wm("Opal")]),
            knoop: None,
            toughness: Some(cited!(
                Medium,
                grades(Toughness::VeryPoor, Toughness::Fair),
                [gia_c("opal"), GIA_CH]
            )),
            cleavage: Some(cited!(Medium, NO_CLEAVAGE, [wm("Opal")])),
            parting: None,
            fracture: Some(cited!(Medium, CONCHOIDAL, [wm("Opal")])),
            care: opal_care!(Medium),
            notes: &[
                note!(
                    Hardness,
                    "Sources differ: the GIA chart gives Mohs 4.5 to 6; the care guide's 5 to 6.5 is shown.",
                    [GIA_CH]
                ),
                note!(
                    Toughness,
                    "Sources differ: the GIA chart grades opal poor to fair; the care guide's very poor to fair is shown.",
                    [GIA_CH]
                ),
            ],
            basis: GemBasis::Inferred("Values of the opal species."),
        },
    ),
    (
        "Precious Opal (Black)",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Opal",
            mohs: cited!(Medium, (5.0, 6.5), [gia_c("opal"), wm("Opal")]),
            knoop: None,
            toughness: Some(cited!(
                Medium,
                grades(Toughness::VeryPoor, Toughness::Fair),
                [gia_c("opal"), GIA_CH]
            )),
            cleavage: Some(cited!(Medium, NO_CLEAVAGE, [wm("Opal")])),
            parting: None,
            fracture: Some(cited!(Medium, CONCHOIDAL, [wm("Opal")])),
            care: opal_care!(Medium),
            notes: &[
                note!(
                    Hardness,
                    "Sources differ: the GIA chart gives Mohs 4.5 to 6; the care guide's 5 to 6.5 is shown.",
                    [GIA_CH]
                ),
                note!(
                    Toughness,
                    "Sources differ: the GIA chart grades opal poor to fair; the care guide's very poor to fair is shown.",
                    [GIA_CH]
                ),
            ],
            basis: GemBasis::Inferred("Values of the opal species."),
        },
    ),
    (
        "Labradorite",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Plagioclase feldspar",
            mohs: cited!(High, (6.0, 6.5), [GIA_CH, hom("anorthite")]),
            knoop: None,
            toughness: Some(cited!(High, grade(Toughness::Poor), [GIA_CH])),
            cleavage: Some(cited!(
                High,
                &[
                    cleave(CleavageQuality::Perfect, "{001}", None),
                    cleave(CleavageQuality::Good, "{010}", None),
                    cleave(CleavageQuality::Distinct, "{110}", None),
                ],
                [wm("Labradorite"), wp("Labradorite"), hom("anorthite")]
            )),
            parting: None,
            fracture: Some(cited!(
                High,
                UNEVEN_CONCHOIDAL,
                [wm("Labradorite"), hom("anorthite")]
            )),
            care: &[
                care!(Heat, High, "Avoid heat.", [GIA_CH]),
                care!(Cutting, High, "Mind the cleavage.", [GIA_CH]),
            ],
            notes: &[
                note!(
                    Cleavage,
                    "Sources differ: the Handbook of Mineralogy (anorthite) calls {010} less perfect than {001} and {110} imperfect; webmineral's good {010} (about 90° to {001}) and distinct {110}, which Wikipedia shares, are shown.",
                    [wm("Labradorite"), wp("Labradorite"), hom("anorthite")]
                ),
                note!(
                    Hardness,
                    "Webmineral's Mohs 7 is a lone outlier against 6 to 6.5 elsewhere and is not used.",
                    [wm("Labradorite")]
                ),
            ],
            basis: GemBasis::Species,
        },
    ),
    (
        "Moonstone",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Alkali feldspar",
            mohs: cited!(High, (6.0, 6.5), [gia_c("moonstone"), hom("orthoclase")]),
            knoop: None,
            toughness: Some(cited!(High, grade(Toughness::Poor), [gia_c("moonstone")])),
            cleavage: Some(cited!(
                High,
                &[
                    cleave(CleavageQuality::Perfect, "{001}", None),
                    cleave(CleavageQuality::Perfect, "{010}", None),
                ],
                [hom("orthoclase")]
            )),
            parting: Some(cited!(
                High,
                &[
                    form("{100}", None),
                    form("{110}", None),
                    form("{1-10}", None),
                    form("{-201}", None),
                ],
                [hom("orthoclase")]
            )),
            fracture: Some(cited!(High, CONCHOIDAL_UNEVEN, [hom("orthoclase")])),
            care: &[
                care!(
                    ThermalShock,
                    High,
                    "High heat or sudden temperature change can cause breaks.",
                    [gia_c("moonstone")]
                ),
                care!(
                    Chemicals,
                    High,
                    "Harmed by hydrofluoric acid.",
                    [gia_c("moonstone")]
                ),
                care!(
                    Cleaning,
                    High,
                    "Steam and ultrasonic are never recommended.",
                    [gia_c("moonstone")]
                ),
            ],
            notes: &[],
            basis: GemBasis::Species,
        },
    ),
    (
        "Titanite (Sphene)",
        GemData {
            origin: MaterialOrigin::Natural,
            species: "Titanite",
            mohs: cited!(Medium, (5.0, 5.5), [hom("titanite")]),
            knoop: None,
            toughness: None,
            cleavage: Some(cited!(
                Medium,
                &[cleave(CleavageQuality::Good, "{110}", None)],
                [hom("titanite")]
            )),
            parting: Some(cited!(Medium, &[form("{221}", None)], [hom("titanite")])),
            fracture: Some(cited!(Medium, &[Fracture::Subconchoidal], [wm("Titanite")])),
            care: &[
                care!(Cutting, Medium, "Hard to polish well.", [igs("sphene")]),
                care!(Heat, Medium, "Heat changes its color.", [igs("sphene")]),
                care!(
                    Cleaning,
                    Medium,
                    "Never use steam or ultrasonic.",
                    [igs("sphene")]
                ),
                care!(Chemicals, Medium, "Protect it from acids.", [igs("sphene")]),
            ],
            notes: &[note!(
                Parting,
                "The parting is due to twinning on {221}.",
                [hom("titanite")]
            )],
            basis: GemBasis::Species,
        },
    ),
    (
        "Nanosital",
        GemData {
            origin: MaterialOrigin::Glass,
            species: "Glass-ceramic",
            mohs: cited!(Medium, (7.0, 7.0), [RJ16]),
            knoop: None,
            toughness: None,
            cleavage: None,
            parting: None,
            fracture: Some(cited!(Medium, CONCHOIDAL, [GG26])),
            care: &[care!(
                Heat,
                Medium,
                "Keeps its color through the heat of casting, so it can be set in the wax before the metal is cast.",
                [GG26, RJ16]
            )],
            notes: &[
                note!(
                    Hardness,
                    "Mohs 7 is the manufacturer's figure; no independent measurement is published.",
                    [RJ16]
                ),
                note!(
                    Fracture,
                    "Gems & Gemology lists conchoidal fractures among the inclusions of the transparent stones.",
                    [GG26]
                ),
            ],
            basis: GemBasis::Species,
        },
    ),
];
