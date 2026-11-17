//! [`GemFacts`]: the gem data of one material as owned display text.
//!
//! A built-in material takes it from `pleochroma_cut_core::built_in_gem_data`, a custom one from
//! [`CustomGemData`]. Every front end renders this one type, so a value reads the same on the
//! Stone tab, in the Material Reference table, on the details page, on Android and in the CLI.

use pleochroma_cut_core::material::{
    CleavageQuality, GemBasis, GemData, GemField, GemSource, Toughness, ToughnessRange,
    built_in_gem_data, cleavage_summary, fracture_summary, mohs_label, parting_summary,
};

/// The text a field shows when a material's own facts leave it empty.
pub const NOT_RECORDED: &str = "Not recorded";
/// The text the toughness field shows for a built-in material no source grades.
pub const NOT_GRADED: &str = "Not graded";
/// The source line of a custom material's own facts.
pub const ENTERED_BY_YOU: &str = "Entered by you";

/// The gem data a user entered for a custom material.
///
/// Every field is optional; an empty one reads "Not recorded". Cleavage, fracture and care are free text: a cutter's own stone rarely has
/// Miller indices to hand.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CustomGemData {
    /// Mohs hardness as `(from, to)`.
    pub mohs: Option<(f32, f32)>,
    /// Toughness as `(from, to)`, in either order.
    pub toughness: Option<(Toughness, Toughness)>,
    /// Cleavage as the user wrote it ("Perfect basal {001}").
    pub cleavage: Option<String>,
    /// Fracture as the user wrote it.
    pub fracture: Option<String>,
    /// Care notes as the user wrote them.
    pub care: Option<String>,
    /// Where the user's figures come from.
    pub source: Option<String>,
}

/// One material's gem data as display text, the same shape for a built-in and a custom material.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GemFacts {
    /// The Mohs figure or range: "7.5–8".
    pub hardness: Option<String>,
    /// The Mohs range as numbers, for the hardness bar and for sorting.
    pub hardness_range: Option<(f32, f32)>,
    /// The Knoop figure where a source gives one: "HK 610 (Knoop)".
    pub knoop: Option<String>,
    /// The toughness grade or range: "Fair to good". `None`: no published grade.
    pub toughness: Option<String>,
    /// The weaker end of the toughness on the scale `VeryPoor` 0 to `Exceptional` 5, for sorting.
    pub toughness_rank: Option<u8>,
    /// The cleavage: "Perfect basal {001}", or "None" for a material without any.
    pub cleavage: Option<String>,
    /// The best cleavage quality present on the scale `VeryPoor` 0 to `Perfect` 7, for sorting.
    /// `None` for a material with no cleavage and for one whose cleavage is not recorded.
    pub cleavage_rank: Option<u8>,
    /// The parting forms as text.
    pub parting: Option<String>,
    /// The fracture as text.
    pub fracture: Option<String>,
    /// The care notes, one sentence each.
    pub care: Vec<String>,
    /// Remarks the data keeps: the basis of a value taken over from the species or computed from
    /// cited figures, and the places where sources differ.
    pub notes: Vec<String>,
    /// The citations, deduplicated, in field order.
    pub sources: Vec<String>,
    /// Whether these are a custom material's own facts.
    pub user_entered: bool,
}

/// The position of a cleavage quality on the scale `VeryPoor` 0 to `Perfect` 7: a higher rank is
/// a stronger cleavage.
#[must_use]
pub const fn cleavage_quality_rank(quality: CleavageQuality) -> u8 {
    match quality {
        CleavageQuality::Perfect => 7,
        CleavageQuality::Good => 6,
        CleavageQuality::Distinct => 5,
        CleavageQuality::Moderate => 4,
        CleavageQuality::Imperfect => 3,
        CleavageQuality::Indistinct => 2,
        CleavageQuality::Poor => 1,
        CleavageQuality::VeryPoor => 0,
    }
}

/// Whether the strongest cleavage direction is Perfect, Good or Distinct, so a front end shows
/// the cleavage warning.
///
/// A built-in material is judged on [`GemFacts::cleavage_rank`] (the best direction). A custom
/// material has only free text, so its leading word is matched, ignoring case: "perfect",
/// "good" or "distinct". Any other text, or none, gives `false`.
#[must_use]
pub fn cleavage_warns(facts: &GemFacts) -> bool {
    const DISTINCT_RANK: u8 = cleavage_quality_rank(CleavageQuality::Distinct);
    if let Some(rank) = facts.cleavage_rank {
        return rank >= DISTINCT_RANK;
    }
    if !facts.user_entered {
        return false;
    }
    let Some(text) = facts.cleavage.as_deref() else {
        return false;
    };
    let word: String = text
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphabetic())
        .collect::<String>()
        .to_lowercase();
    matches!(word.as_str(), "perfect" | "good" | "distinct")
}

/// The Stone tab chip's toughness text, `None` when no toughness is recorded.
///
/// "Toughness Poor" for one grade, "Toughness Fair–good" for a range (en dash, the second word
/// in lower case). The "(by direction)" note of a directional range stays on the details page.
#[must_use]
pub fn toughness_chip_text(facts: &GemFacts) -> Option<String> {
    let label = facts.toughness.as_deref()?;
    let label = label
        .strip_suffix(" (by direction)")
        .unwrap_or(label)
        .trim();
    if label.is_empty() {
        return None;
    }
    Some(match label.split_once(" to ") {
        Some((low, high)) => format!("Toughness {low}\u{2013}{}", high.to_lowercase()),
        None => format!("Toughness {label}"),
    })
}

/// The text of a derived value's basis note, which always starts with the word "Derived".
#[must_use]
pub fn derived_note(note: &str) -> String {
    if note.starts_with("Derived") {
        note.to_owned()
    } else {
        format!("Derived: {note}")
    }
}

/// `value` without surrounding blanks, `None` when nothing is left.
fn trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Adds the citations of `sources` to `list`, skipping any already there.
fn add_sources(list: &mut Vec<String>, sources: &[GemSource]) {
    for source in sources {
        let citation = source.citation();
        if !list.contains(&citation) {
            list.push(citation);
        }
    }
}

/// Adds the citations of the notes about `field` to `list`.
fn add_note_sources(list: &mut Vec<String>, data: &GemData, field: GemField) {
    for note in data.notes.iter().filter(|note| note.field == field) {
        add_sources(list, note.sources);
    }
}

impl GemFacts {
    /// The facts of a built-in material. A value taken over from the species or computed from
    /// cited figures carries its basis note first in [`Self::notes`].
    #[must_use]
    pub fn from_built_in(data: &GemData) -> Self {
        let mut sources = Vec::new();
        add_sources(&mut sources, data.mohs.sources);
        if let Some(knoop) = &data.knoop {
            add_sources(&mut sources, knoop.sources);
        }
        add_note_sources(&mut sources, data, GemField::Hardness);
        if let Some(toughness) = &data.toughness {
            add_sources(&mut sources, toughness.sources);
        }
        add_note_sources(&mut sources, data, GemField::Toughness);
        if let Some(cleavage) = &data.cleavage {
            add_sources(&mut sources, cleavage.sources);
        }
        add_note_sources(&mut sources, data, GemField::Cleavage);
        if let Some(parting) = &data.parting {
            add_sources(&mut sources, parting.sources);
        }
        add_note_sources(&mut sources, data, GemField::Parting);
        if let Some(fracture) = &data.fracture {
            add_sources(&mut sources, fracture.sources);
        }
        add_note_sources(&mut sources, data, GemField::Fracture);
        for (_, care) in data.care {
            add_sources(&mut sources, care.sources);
        }
        add_note_sources(&mut sources, data, GemField::Care);

        let mut notes = Vec::new();
        match data.basis {
            GemBasis::Species => {}
            GemBasis::Inferred(note) => notes.push(note.to_owned()),
            GemBasis::Derived(note) => notes.push(derived_note(note)),
        }
        notes.extend(data.notes.iter().map(|note| note.text.to_owned()));

        Self {
            hardness: Some(mohs_label(data.mohs.value)),
            hardness_range: Some(data.mohs.value),
            knoop: data
                .knoop
                .as_ref()
                .map(|knoop| format!("HK {} (Knoop)", knoop.value)),
            toughness: data.toughness.as_ref().map(|t| t.value.label()),
            toughness_rank: data.toughness.as_ref().map(|t| t.value.low.rank()),
            cleavage: data.cleavage.as_ref().map(|c| cleavage_summary(c.value)),
            cleavage_rank: data.cleavage.as_ref().and_then(|c| {
                c.value
                    .iter()
                    .map(|direction| cleavage_quality_rank(direction.quality))
                    .max()
            }),
            parting: data.parting.as_ref().map(|p| parting_summary(p.value)),
            fracture: data.fracture.as_ref().map(|f| fracture_summary(f.value)),
            care: data
                .care
                .iter()
                .map(|(_, care)| care.value.to_owned())
                .collect(),
            notes,
            sources,
            user_entered: false,
        }
    }

    /// The facts a user entered for a custom material. Its sources are "Entered by you" and the
    /// user's own source text.
    #[must_use]
    pub fn from_custom(data: &CustomGemData) -> Self {
        let mohs = data
            .mohs
            .filter(|(low, high)| low.is_finite() && high.is_finite())
            .map(|(low, high)| (low.min(high), low.max(high)));
        let toughness = data.toughness.map(|(first, second)| ToughnessRange {
            low: first.min(second),
            high: first.max(second),
            directional: false,
        });
        let mut facts = Self::custom_empty();
        facts.hardness = mohs.map(mohs_label);
        facts.hardness_range = mohs;
        facts.toughness = toughness.map(ToughnessRange::label);
        facts.toughness_rank = toughness.map(|range| range.low.rank());
        facts.cleavage = trimmed(data.cleavage.as_deref());
        facts.fracture = trimmed(data.fracture.as_deref());
        facts.care = trimmed(data.care.as_deref()).into_iter().collect();
        if let Some(source) = trimmed(data.source.as_deref()) {
            facts.sources.push(source);
        }
        facts
    }

    /// The facts of a custom material with no gem data: every field empty, the source line
    /// "Entered by you".
    #[must_use]
    pub fn custom_empty() -> Self {
        Self {
            sources: vec![ENTERED_BY_YOU.to_owned()],
            user_entered: true,
            ..Self::default()
        }
    }

    /// The facts of the material called `name`: a custom material's own (`is_custom`, with its
    /// stored `custom` data, if any) or a built-in's. A name with no gem data gives empty facts.
    #[must_use]
    pub fn for_material(name: &str, is_custom: bool, custom: Option<&CustomGemData>) -> Self {
        if is_custom {
            return custom.map_or_else(Self::custom_empty, Self::from_custom);
        }
        built_in_gem_data(name).map_or_else(Self::default, Self::from_built_in)
    }

    /// Whether no field holds a value.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.hardness.is_none()
            && self.toughness.is_none()
            && self.cleavage.is_none()
            && self.parting.is_none()
            && self.fracture.is_none()
            && self.care.is_empty()
    }

    /// The hardness line: "{h} (Mohs)", and for a glass "about {h} (Mohs) · HK 610 (Knoop)".
    #[must_use]
    pub fn hardness_line(&self) -> Option<String> {
        let hardness = self.hardness.as_deref()?;
        Some(self.knoop.as_ref().map_or_else(
            || format!("{hardness} (Mohs)"),
            |knoop| format!("about {hardness} (Mohs) \u{b7} {knoop}"),
        ))
    }

    /// The Stone tab card's summary: "Mohs {h} · {toughness} · {cleavage}".
    ///
    /// "Not recorded" when there is no hardness. A missing toughness reads "Not graded" for a built-in material
    /// and "Not recorded" for a custom one; a missing cleavage reads "Not recorded".
    #[must_use]
    pub fn summary(&self) -> String {
        let Some(hardness) = self.hardness.as_deref() else {
            return NOT_RECORDED.to_owned();
        };
        let missing_toughness = if self.user_entered {
            NOT_RECORDED
        } else {
            NOT_GRADED
        };
        format!(
            "Mohs {hardness} \u{b7} {} \u{b7} {}",
            self.toughness.as_deref().unwrap_or(missing_toughness),
            self.cleavage.as_deref().unwrap_or(NOT_RECORDED),
        )
    }
}
