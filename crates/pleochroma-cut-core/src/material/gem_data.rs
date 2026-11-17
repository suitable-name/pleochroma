//! The built-in gem reference table: hardness, toughness, cleavage, parting, fracture and care
//! notes, keyed by `GemMaterial::name`. Provenance: see [`crate::material`]'s module doc,
//! "Gem data".
//!
//! The table itself is read-only `'static` data in [`built_in_gem_data`]. Every stored value
//! carries the sources it comes from ([`Cited`]); a value no gemological or mineralogical
//! source gives is not stored at all, so its field is empty. The text helpers in this file
//! ([`mohs_label`], [`cleavage_summary`] and the others) are shared by every front end, so a
//! value reads the same on every screen.

mod table;

#[cfg(test)]
mod tests;

use std::fmt::Write as _;

/// The GIA toughness words, weakest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Toughness {
    /// GIA "very poor".
    VeryPoor,
    /// GIA "poor".
    Poor,
    /// GIA "fair".
    Fair,
    /// GIA "good".
    Good,
    /// GIA "excellent".
    Excellent,
    /// GIA "exceptional".
    Exceptional,
}

impl Toughness {
    /// The word as printed: "Very poor", "Poor", "Fair", "Good", "Excellent", "Exceptional".
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::VeryPoor => "Very poor",
            Self::Poor => "Poor",
            Self::Fair => "Fair",
            Self::Good => "Good",
            Self::Excellent => "Excellent",
            Self::Exceptional => "Exceptional",
        }
    }

    /// Position on the scale for sorting: `VeryPoor` is 0 and `Exceptional` is 5.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::VeryPoor => 0,
            Self::Poor => 1,
            Self::Fair => 2,
            Self::Good => 3,
            Self::Excellent => 4,
            Self::Exceptional => 5,
        }
    }
}

/// A toughness grade or a range of grades, weakest word in `low`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToughnessRange {
    /// The weaker end of the range.
    pub low: Toughness,
    /// The stronger end of the range. Equal to `low` for a single grade.
    pub high: Toughness,
    /// Toughness depends on the direction (diamond: good along its cleavage, exceptional
    /// otherwise).
    pub directional: bool,
}

impl ToughnessRange {
    /// "Fair to good", "Excellent", or "Good to exceptional (by direction)". The weaker word
    /// always comes first, whatever order a source prints ("Good to excellent" for the GIA
    /// chart's "Excellent to Good").
    #[must_use]
    pub fn label(self) -> String {
        let mut text = if self.low == self.high {
            self.low.label().to_owned()
        } else {
            format!(
                "{} to {}",
                self.low.label(),
                self.high.label().to_ascii_lowercase()
            )
        };
        if self.directional {
            text.push_str(" (by direction)");
        }
        text
    }
}

/// The quality words mineralogical sources use, best first. `Moderate` is the Handbook of
/// Mineralogy's word for rutile {100}.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CleavageQuality {
    /// "Perfect".
    Perfect,
    /// "Good".
    Good,
    /// "Distinct".
    Distinct,
    /// "Moderate".
    Moderate,
    /// "Imperfect".
    Imperfect,
    /// "Indistinct".
    Indistinct,
    /// "Poor".
    Poor,
    /// "Very poor".
    VeryPoor,
}

impl CleavageQuality {
    /// The word as printed at the start of a sentence: "Perfect", ..., "Very poor".
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Perfect => "Perfect",
            Self::Good => "Good",
            Self::Distinct => "Distinct",
            Self::Moderate => "Moderate",
            Self::Imperfect => "Imperfect",
            Self::Indistinct => "Indistinct",
            Self::Poor => "Poor",
            Self::VeryPoor => "Very poor",
        }
    }
}

/// A crystal form: Miller(-Bravais) indices as printed ("{001}", "{10-11}"), and a common name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrystalForm {
    /// The indices with their braces, or an empty string where the source names the form
    /// without giving indices (quartz: "rhombohedral parting").
    pub indices: &'static str,
    /// A common name for the form: "basal", "octahedral", "rhombohedral".
    pub name: Option<&'static str>,
}

/// One cleavage direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cleavage {
    /// How well the stone cleaves along the form.
    pub quality: CleavageQuality,
    /// The crystal form of the cleavage plane.
    pub form: CrystalForm,
    /// The number of directions the form gives, where a source states it (diamond: 4).
    pub directions: Option<u8>,
}

/// The fracture words mineralogical sources use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fracture {
    /// Curved, shell-like surfaces.
    Conchoidal,
    /// Shell-like surfaces that are less pronounced.
    Subconchoidal,
    /// Rough, irregular surfaces.
    Uneven,
    /// Flat surfaces.
    Even,
    /// Splinter-like surfaces.
    Splintery,
}

impl Fracture {
    /// The word as printed at the start of a sentence: "Conchoidal", "Uneven", ...
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Conchoidal => "Conchoidal",
            Self::Subconchoidal => "Subconchoidal",
            Self::Uneven => "Uneven",
            Self::Even => "Even",
            Self::Splintery => "Splintery",
        }
    }
}

/// What a care note is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CareTopic {
    /// Heat: dopping, treatment, jewelry repair.
    Heat,
    /// Sudden temperature change.
    ThermalShock,
    /// Acids, alkalis and other chemicals.
    Chemicals,
    /// Color change in strong light.
    Light,
    /// Steam, ultrasonic and washing.
    Cleaning,
    /// Cutting, polishing and handling at the wheel, including wear and breakage.
    Cutting,
}

impl CareTopic {
    /// A short heading for the note: "Heat", "Thermal shock", "Chemicals", "Light", "Cleaning",
    /// "Cutting".
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Heat => "Heat",
            Self::ThermalShock => "Thermal shock",
            Self::Chemicals => "Chemicals",
            Self::Light => "Light",
            Self::Cleaning => "Cleaning",
            Self::Cutting => "Cutting",
        }
    }
}

/// Where a value comes from. Each variant renders a short citation ([`GemSource::citation`]),
/// and a URL where the source has a stable one of its own ([`GemSource::url`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GemSource {
    /// GIA gem encyclopedia care page; the payload is the page slug ("topaz" for
    /// `gia.edu/topaz-care-cleaning`).
    GiaCare(&'static str),
    /// GIA "Gem Cleaning and Display Chart", rev. Oct 2018 (GCC1018), cited by title and
    /// revision with GIA's own store page as its location.
    GiaChart2018,
    /// GIA gem encyclopedia page (the FACTS box gives SG, RI and Mohs); the payload is the
    /// page slug.
    GiaEncyclopedia(&'static str),
    /// Nassau, `McClure`, Elen and Shigley, "Synthetic Moissanite: A New Diamond Substitute",
    /// Gems & Gemology, Winter 1997, Table 1. Its table cites the GIA Gem Reference Guide
    /// (1988).
    GemsGemology1997,
    /// Handbook of Mineralogy; the payload is the PDF file stem ("corundum", "Spinel").
    HandbookOfMineralogy(&'static str),
    /// webmineral.com; the payload is the page file stem ("Opal").
    Webmineral(&'static str),
    /// International Gem Society gem article; the payload is the gem part of the slug
    /// ("benitoite" for `benitoite-jewelry-and-gemstone-information`).
    Igs(&'static str),
    /// Schott optical glass datasheet; the payload is the glass type ("N-BK7", "F2HT").
    Schott(&'static str),
    /// Wikipedia article; the payload is the article title with underscores
    /// (`Cubic_zirconia`).
    Wikipedia(&'static str),
    /// The GIA Gem Reference Guide, cited by title only, for values checked against the book.
    GiaGemReferenceGuide,
}

/// Capitalizes the first letter of `text` and turns separators into spaces.
fn title_case(text: &str) -> String {
    let spaced = text.replace(['_', '-'], " ");
    let mut chars = spaced.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

impl GemSource {
    /// A short citation: "GIA, Topaz care guide", "Handbook of Mineralogy, Chrysoberyl", ...
    #[must_use]
    pub fn citation(&self) -> String {
        match self {
            Self::GiaCare(slug) => format!("GIA, {} care guide", title_case(slug)),
            Self::GiaChart2018 => "GIA, Gem Cleaning and Display Chart (rev. Oct 2018)".to_owned(),
            Self::GiaEncyclopedia(slug) => format!("GIA, {} gem encyclopedia", title_case(slug)),
            Self::GemsGemology1997 => {
                "Nassau et al., Gems & Gemology, Winter 1997 (Table 1)".to_owned()
            }
            Self::HandbookOfMineralogy(stem) => {
                format!("Handbook of Mineralogy, {}", title_case(stem))
            }
            Self::Webmineral(stem) => format!("webmineral.com, {}", title_case(stem)),
            Self::Igs(slug) => format!("International Gem Society, {}", title_case(slug)),
            Self::Schott(glass) => format!("Schott optical glass datasheet, {glass}"),
            Self::Wikipedia(title) => format!("Wikipedia, {}", title.replace('_', " ")),
            Self::GiaGemReferenceGuide => "GIA Gem Reference Guide (1988)".to_owned(),
        }
    }

    /// The source's stable address. `None` for a source cited by title only (the GIA Gem
    /// Reference Guide) and for the Gems & Gemology article, which has no address of its own.
    #[must_use]
    pub fn url(&self) -> Option<String> {
        match self {
            Self::GiaCare(slug) => Some(format!("https://www.gia.edu/{slug}-care-cleaning")),
            Self::GiaChart2018 => {
                Some("https://store.gia.edu/products/gem-cleaning-and-display-chart".to_owned())
            }
            Self::GiaEncyclopedia(slug) => Some(format!("https://www.gia.edu/{slug}")),
            Self::HandbookOfMineralogy(stem) => Some(format!(
                "https://www.handbookofmineralogy.org/pdfs/{stem}.pdf"
            )),
            Self::Webmineral(stem) => Some(format!("http://webmineral.com/data/{stem}.shtml")),
            Self::Igs(slug) => Some(format!(
                "https://www.gemsociety.org/article/{slug}-jewelry-and-gemstone-information/"
            )),
            Self::Schott(_) => Some(
                "https://media.schott.com/api/public/content/38cbbe876d324e03b1881d33f3d26635"
                    .to_owned(),
            ),
            Self::Wikipedia(title) => Some(format!("https://en.wikipedia.org/wiki/{title}")),
            Self::GemsGemology1997 | Self::GiaGemReferenceGuide => None,
        }
    }
}

/// How firmly a value stands.
///
/// `Medium`: one source only, a source that describes the natural
/// mineral for a synthetic, or a value taken over from the species (see [`GemBasis`]). There
/// is no "unconfirmed" level: a value without a gemological or mineralogical source is not
/// stored at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// Two or more agreeing sources, or one authoritative source for this material.
    High,
    /// One source only, a source for the natural mineral, or a value taken over from the
    /// species.
    Medium,
}

/// A value with its sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cited<T: 'static> {
    /// The value.
    pub value: T,
    /// The sources the value comes from; never empty.
    pub sources: &'static [GemSource],
    /// How firmly the value stands.
    pub confidence: Confidence,
}

/// The field of a [`GemData`] a note or a source belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GemField {
    /// Mohs hardness (and Knoop).
    Hardness,
    /// Toughness.
    Toughness,
    /// Cleavage.
    Cleavage,
    /// Parting.
    Parting,
    /// Fracture.
    Fracture,
    /// Care notes.
    Care,
}

/// A disagreement between sources, or a remark on one value, that the data keeps and the UI
/// shows as a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceNote {
    /// The field the note is about.
    pub field: GemField,
    /// The note text, a complete sentence.
    pub text: &'static str,
    /// The sources the note rests on; never empty.
    pub sources: &'static [GemSource],
}

/// What the values of an entry describe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GemBasis {
    /// The species of this material.
    Species,
    /// Taken from the end-members (color-change garnet) or the species of a variety (precious
    /// opal). The payload is the basis note shown beside the values.
    Inferred(&'static str),
    /// Computed from cited figures (a midpoint of a cited range, a range merged from two cited
    /// studies). It is shown with its basis note, and it is distinct from an unsourced value,
    /// which stays absent.
    Derived(&'static str),
}

/// Whether a material is a mineral, a laboratory-grown material or a glass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialOrigin {
    /// A natural gem material.
    Natural,
    /// A laboratory-grown crystal.
    Synthetic,
    /// A glass.
    Glass,
}

/// The gem reference data of one built-in material.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GemData {
    /// Natural, synthetic or glass.
    pub origin: MaterialOrigin,
    /// The mineral species, for the family line ("Chrysoberyl", "Corundum", "Beryl").
    pub species: &'static str,
    /// Mohs hardness as `(low, high)`; equal ends for a single figure.
    pub mohs: Cited<(f32, f32)>,
    /// Knoop hardness, where a source gives it (the glasses).
    pub knoop: Option<Cited<u16>>,
    /// `None`: no published grade (the glasses, Benitoite, Titanite). Shown as "Not graded".
    pub toughness: Option<Cited<ToughnessRange>>,
    /// `Some` with an empty slice: no cleavage. `None`: not recorded (GGG).
    pub cleavage: Option<Cited<&'static [Cleavage]>>,
    /// The parting forms. `None`: none recorded.
    pub parting: Option<Cited<&'static [CrystalForm]>>,
    /// `None`: not recorded (GGG).
    pub fracture: Option<Cited<&'static [Fracture]>>,
    /// Short care notes, one sentence per topic.
    pub care: &'static [(CareTopic, Cited<&'static str>)],
    /// Disagreements between sources and remarks on single values.
    pub notes: &'static [SourceNote],
    /// What the values describe.
    pub basis: GemBasis,
}

/// The gem data of a built-in material, matched on the exact `GemMaterial::name` (ASCII case
/// ignored, like [`crate::material::built_in_specific_gravity`]). `None` for any other name.
#[must_use]
pub fn built_in_gem_data(material_name: &str) -> Option<&'static GemData> {
    table::TABLE
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(material_name))
        .map(|(_, data)| data)
}

/// The Mohs figure or range as text: "8", "7.5–8", "9.25".
#[must_use]
pub fn mohs_label((low, high): (f32, f32)) -> String {
    if (high - low).abs() < f32::EPSILON {
        low.to_string()
    } else {
        format!("{low}\u{2013}{high}")
    }
}

/// Uppercases the first letter of `text`.
fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// A crystal form as text: "basal {001}", "{110}", "rhombohedral".
fn form_text(form: &CrystalForm) -> String {
    match (form.name, form.indices.is_empty()) {
        (Some(name), false) => format!("{name} {}", form.indices),
        (Some(name), true) => name.to_owned(),
        (None, _) => form.indices.to_owned(),
    }
}

/// The cleavage as text.
///
/// "None" for an empty slice, otherwise the directions strongest first
/// as the source lists them: "Perfect basal {001}", "Distinct {110}, imperfect {010}, poor
/// {001}". Neighboring directions of the same quality share one quality word ("Very poor
/// {11-20}, {10-11}").
#[must_use]
pub fn cleavage_summary(cleavage: &[Cleavage]) -> String {
    if cleavage.is_empty() {
        return "None".to_owned();
    }
    let mut groups: Vec<(CleavageQuality, Vec<String>)> = Vec::new();
    for direction in cleavage {
        let form = form_text(&direction.form);
        match groups.last_mut() {
            Some((quality, forms)) if *quality == direction.quality => forms.push(form),
            _ => groups.push((direction.quality, vec![form])),
        }
    }
    let text = groups
        .iter()
        .map(|(quality, forms)| {
            format!(
                "{} {}",
                quality.label().to_ascii_lowercase(),
                forms.join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    capitalize(&text)
}

/// The parting forms as text: "None" for an empty slice, otherwise the forms in the order the
/// source lists them: "Basal {0001}, rhombohedral {10-11}".
#[must_use]
pub fn parting_summary(parting: &[CrystalForm]) -> String {
    if parting.is_empty() {
        return "None".to_owned();
    }
    let text = parting.iter().map(form_text).collect::<Vec<_>>().join(", ");
    capitalize(&text)
}

/// The fracture as text.
///
/// One word stands alone ("Conchoidal"), two read "Uneven to
/// conchoidal", and three or more list all but the last two, then the pair:
/// "Conchoidal, uneven to splintery". An empty slice reads "Not recorded".
#[must_use]
pub fn fracture_summary(fracture: &[Fracture]) -> String {
    let words: Vec<String> = fracture
        .iter()
        .map(|word| word.label().to_ascii_lowercase())
        .collect();
    let mut text = String::new();
    match words.as_slice() {
        [] => return "Not recorded".to_owned(),
        [only] => text.push_str(only),
        [rest @ .., second_last, last] => {
            for word in rest {
                let _ = write!(text, "{word}, ");
            }
            let _ = write!(text, "{second_last} to {last}");
        }
    }
    capitalize(&text)
}
