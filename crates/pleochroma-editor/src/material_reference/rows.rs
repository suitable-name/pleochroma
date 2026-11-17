//! The Material Reference table: one [`ReferenceRow`] per catalogue material.
//!
//! It adds sorting, filtering and the tab-separated copy text. Rows are plain owned data; each
//! front end maps them to its own row type.

use std::cmp::Ordering;

use pleochroma::optics::materials::{GemMaterial, OpticalCharacter, variants::face_up_swatch};
use pleochroma_cut_core::material::{
    GemData, MaterialCatalogue, MaterialEntry, MaterialKind, MaterialLookup, MaterialOrigin,
    built_in_gem_data, crystal_system_name,
};

use super::{
    facts::{CustomGemData, GemFacts},
    text::{self, DASH},
};

/// The stone width, in millimeters, a row's swatch is evaluated at.
const SWATCH_STONE_WIDTH_MM: f64 = 7.0;

/// What a material is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A natural gem material.
    Natural,
    /// A laboratory-grown crystal.
    Synthetic,
    /// A glass.
    Glass,
    /// A material of the user's own.
    Custom,
}

impl Origin {
    /// The mini-chip beside the name: "Synthetic", "Glass", "Mine", and an empty string for a
    /// natural material.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Natural => "",
            Self::Synthetic => "Synthetic",
            Self::Glass => "Glass",
            Self::Custom => "Mine",
        }
    }
}

impl From<MaterialOrigin> for Origin {
    fn from(origin: MaterialOrigin) -> Self {
        match origin {
            MaterialOrigin::Natural => Self::Natural,
            MaterialOrigin::Synthetic => Self::Synthetic,
            MaterialOrigin::Glass => Self::Glass,
        }
    }
}

/// A table column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// The material's name.
    Name,
    /// The crystal system.
    CrystalSystem,
    /// The optic character.
    OpticCharacter,
    /// The refractive index `n_D` of the base curve.
    Ri,
    /// The birefringence.
    Birefringence,
    /// The dispersion, in the row's chosen scale.
    Dispersion,
    /// The specific gravity.
    SpecificGravity,
    /// The Mohs hardness.
    Hardness,
    /// The toughness.
    Toughness,
    /// The cleavage.
    Cleavage,
}

/// A filter chip of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    /// Every material.
    All,
    /// Natural materials.
    Natural,
    /// Synthetic crystals.
    Synthetic,
    /// Glasses.
    Glass,
    /// The user's own materials.
    Custom,
}

impl Filter {
    /// Whether a row of `origin` passes this filter.
    #[must_use]
    pub fn admits(self, origin: Origin) -> bool {
        match self {
            Self::All => true,
            Self::Natural => origin == Origin::Natural,
            Self::Synthetic => origin == Origin::Synthetic,
            Self::Glass => origin == Origin::Glass,
            Self::Custom => origin == Origin::Custom,
        }
    }
}

/// Which dispersion convention the table's Dispersion column shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispersionScale {
    /// `n(430.8 nm) - n(686.7 nm)`, the figure gem references print.
    BG,
    /// `n(486.1 nm) - n(656.3 nm)`, the optical figure the Material Editor uses.
    FC,
}

impl DispersionScale {
    /// The heading of the Dispersion column for this scale: "Disp. (B–G)".
    #[must_use]
    pub const fn column_label(self) -> &'static str {
        match self {
            Self::BG => "Disp. (B\u{2013}G)",
            Self::FC => "Disp. (F\u{2013}C)",
        }
    }
}

/// The numeric sort keys of a row. A key a material has no value for is `None`, and such a row
/// sorts after every row that has one, in either direction. Keys are never NaN.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SortKeys {
    /// `n_D` of the base curve.
    pub ri: Option<f64>,
    /// The birefringence magnitude; `None` for an isotropic material.
    pub birefringence: Option<f64>,
    /// The B-G dispersion.
    pub dispersion_bg: Option<f64>,
    /// The F-C dispersion.
    pub dispersion_fc: Option<f64>,
    /// The specific gravity.
    pub sg: Option<f64>,
    /// The upper end of the Mohs range, the primary hardness key.
    pub hardness_high: Option<f64>,
    /// The lower end of the Mohs range, the hardness tie-break.
    pub hardness_low: Option<f64>,
    /// The weaker end of the toughness, the primary toughness key.
    pub toughness_low: Option<f64>,
    /// The stronger end of the toughness, the toughness tie-break.
    pub toughness_high: Option<f64>,
    /// The best cleavage quality present (a higher rank is a stronger cleavage).
    pub cleavage: Option<f64>,
}

/// One row of the reference table.
#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceRow {
    /// The material's name.
    pub name: String,
    /// Natural, synthetic, glass or the user's own.
    pub origin: Origin,
    /// The mineral species ("Corundum"); empty for a custom material.
    pub species: String,
    /// The face-up D65 color at 7 mm, sRGB.
    pub swatch: [u8; 3],
    /// The crystal system: "Trigonal".
    pub crystal_system: String,
    /// The optic character: "Uniaxial (−)", "Biaxial (+)", "Isotropic".
    pub optic_character: String,
    /// `n_D` of the base curve (`n_o`, or `n_beta` for a biaxial material), three decimals.
    pub ri_text: String,
    /// The birefringence to three decimals, or a dash for an isotropic material.
    pub birefringence_text: String,
    /// The B-G dispersion to three decimals.
    pub dispersion_bg_text: String,
    /// The F-C dispersion to three decimals.
    pub dispersion_fc_text: String,
    /// The specific gravity to two decimals, or a dash.
    pub sg_text: String,
    /// Hardness, toughness, cleavage and the rest of the gem data.
    pub gem: GemFacts,
    /// The numeric sort keys.
    pub sort: SortKeys,
}

/// `value`, or a dash when the field is empty.
fn or_dash(value: Option<&str>) -> &str {
    value.unwrap_or(DASH)
}

/// The sort keys of one material.
fn sort_keys(
    entry: &MaterialEntry,
    gem: &GemMaterial,
    facts: &GemFacts,
    toughness_high: Option<u8>,
) -> SortKeys {
    SortKeys {
        ri: Some(entry.ri_d),
        birefringence: (gem.optical_character != OpticalCharacter::Isotropic)
            .then(|| f64::from(gem.birefringence_delta.abs())),
        dispersion_bg: Some(f64::from(gem.dispersion.delta_b_g())),
        dispersion_fc: Some(f64::from(gem.dispersion.delta_f_c())),
        sg: entry.sg,
        hardness_high: facts.hardness_range.map(|(_, high)| f64::from(high)),
        hardness_low: facts.hardness_range.map(|(low, _)| f64::from(low)),
        toughness_low: facts.toughness_rank.map(f64::from),
        toughness_high: toughness_high.map(f64::from),
        cleavage: facts.cleavage_rank.map(f64::from),
    }
}

/// The stronger end of a built-in material's toughness on the scale 0 to 5.
fn built_in_toughness_high(data: Option<&GemData>) -> Option<u8> {
    data?.toughness.as_ref().map(|t| t.value.high.rank())
}

/// The stronger end of a custom material's toughness on the scale 0 to 5.
fn custom_toughness_high(data: Option<&CustomGemData>) -> Option<u8> {
    data?
        .toughness
        .map(|(first, second)| first.max(second).rank())
}

/// Builds the row of one catalogue entry.
fn build_row(
    entry: &MaterialEntry,
    gem: &GemMaterial,
    custom_gem: &[(String, CustomGemData)],
) -> ReferenceRow {
    let is_custom = entry.kind == MaterialKind::Custom;
    let custom_data = custom_gem
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(&entry.name))
        .map(|(_, data)| data);
    let built_in = if is_custom {
        None
    } else {
        built_in_gem_data(&entry.name)
    };
    let (facts, toughness_high) = if is_custom {
        (
            GemFacts::for_material(&entry.name, true, custom_data),
            custom_toughness_high(custom_data),
        )
    } else {
        (
            built_in.map_or_else(GemFacts::default, GemFacts::from_built_in),
            built_in_toughness_high(built_in),
        )
    };
    let origin = if is_custom {
        Origin::Custom
    } else {
        built_in.map_or(Origin::Natural, |data| Origin::from(data.origin))
    };
    let swatch = face_up_swatch(
        &gem.absorption,
        gem.absorption_unit,
        SWATCH_STONE_WIDTH_MM,
        gem,
    )
    .srgb;
    let birefringence_text = if gem.optical_character == OpticalCharacter::Isotropic {
        DASH.to_owned()
    } else {
        format!("{:.3}", gem.birefringence_delta.abs())
    };
    let sort = sort_keys(entry, gem, &facts, toughness_high);
    ReferenceRow {
        name: entry.name.clone(),
        origin,
        species: built_in.map_or_else(String::new, |data| data.species.to_owned()),
        swatch,
        crystal_system: crystal_system_name(gem.crystal_system).to_owned(),
        optic_character: text::optic_character_text(gem.optical_character).to_owned(),
        ri_text: format!("{:.3}", entry.ri_d),
        birefringence_text,
        dispersion_bg_text: text::dispersion_chip_text(gem),
        dispersion_fc_text: text::dispersion_fc_text(gem),
        sg_text: entry
            .sg
            .map_or_else(|| DASH.to_owned(), |sg| format!("{sg:.2}")),
        gem: facts,
        sort,
    }
}

/// The rows of every material in `catalogue`, in the catalogue's order.
///
/// Built-ins come first, then the user's own materials by name. A material `lookup` cannot
/// resolve has no row. `custom_gem` holds the gem data of custom materials by name; a custom material without an entry reads
/// "Not recorded".
#[must_use]
pub fn reference_rows(
    catalogue: &MaterialCatalogue,
    lookup: &dyn MaterialLookup,
    custom_gem: &[(String, CustomGemData)],
) -> Vec<ReferenceRow> {
    catalogue
        .entries()
        .iter()
        .filter_map(|entry| {
            let gem = lookup.lookup(&entry.name)?;
            Some(build_row(entry, &gem, custom_gem))
        })
        .collect()
}

/// `order` reversed when `descending`.
const fn directed(order: Ordering, descending: bool) -> Ordering {
    if descending { order.reverse() } else { order }
}

/// Compares two optional keys: a present key before an absent one in either direction, present
/// keys by value (reversed when `descending`).
fn missing_last(first: Option<f64>, second: Option<f64>, descending: bool) -> Ordering {
    match (first, second) {
        (Some(a), Some(b)) => directed(a.total_cmp(&b), descending),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Names, case-insensitively, then exactly.
fn name_order(first: &ReferenceRow, second: &ReferenceRow) -> Ordering {
    first
        .name
        .to_lowercase()
        .cmp(&second.name.to_lowercase())
        .then_with(|| first.name.cmp(&second.name))
}

/// The primary and the secondary numeric keys of `row` for `column`.
const fn numeric_keys(row: &ReferenceRow, column: Column) -> (Option<f64>, Option<f64>) {
    let sort = &row.sort;
    match column {
        Column::Ri => (sort.ri, None),
        Column::Birefringence => (sort.birefringence, None),
        Column::Dispersion => (sort.dispersion_bg, None),
        Column::SpecificGravity => (sort.sg, None),
        Column::Hardness => (sort.hardness_high, sort.hardness_low),
        Column::Toughness => (sort.toughness_low, sort.toughness_high),
        Column::Cleavage => (sort.cleavage, None),
        Column::Name | Column::CrystalSystem | Column::OpticCharacter => (None, None),
    }
}

/// How `first` and `second` compare in `column`, ending in the name order.
fn compare(
    first: &ReferenceRow,
    second: &ReferenceRow,
    column: Column,
    descending: bool,
) -> Ordering {
    let by_column = match column {
        Column::Name => return directed(name_order(first, second), descending),
        Column::CrystalSystem => directed(
            first
                .crystal_system
                .to_lowercase()
                .cmp(&second.crystal_system.to_lowercase()),
            descending,
        ),
        Column::OpticCharacter => directed(
            first
                .optic_character
                .to_lowercase()
                .cmp(&second.optic_character.to_lowercase()),
            descending,
        ),
        _ => {
            let (a_primary, a_secondary) = numeric_keys(first, column);
            let (b_primary, b_secondary) = numeric_keys(second, column);
            missing_last(a_primary, b_primary, descending)
                .then_with(|| missing_last(a_secondary, b_secondary, descending))
        }
    };
    by_column.then_with(|| name_order(first, second))
}

/// Sorts `rows` by `column`.
///
/// The sort is stable; ties fall to the column's secondary key
/// (hardness: the low end of the range; toughness: the stronger end), then to the name. A row
/// with no value in the column sorts last, ascending or descending.
pub fn sort_rows(rows: &mut [ReferenceRow], column: Column, descending: bool) {
    rows.sort_by(|first, second| compare(first, second, column, descending));
}

/// Whether `row` matches the lowercase `needle` in its name, species, crystal system, optic
/// character or cleavage text.
fn matches_query(row: &ReferenceRow, needle: &str) -> bool {
    [
        row.name.as_str(),
        row.species.as_str(),
        row.crystal_system.as_str(),
        row.optic_character.as_str(),
        row.gem.cleavage.as_deref().unwrap_or(""),
    ]
    .iter()
    .any(|field| field.to_lowercase().contains(needle))
}

/// The rows that pass `filter` and match `query`, in the order of `rows`.
///
/// A row matches when its name, species, crystal system, optic character or cleavage text
/// contains `query` (case-insensitive substring; a blank query admits every row).
#[must_use]
pub fn filter_rows<'a>(
    rows: &'a [ReferenceRow],
    filter: Filter,
    query: &str,
) -> Vec<&'a ReferenceRow> {
    let needle = query.trim().to_lowercase();
    rows.iter()
        .filter(|row| filter.admits(row.origin))
        .filter(|row| needle.is_empty() || matches_query(row, &needle))
        .collect()
}

/// How many rows each filter chip admits, in the order All, Natural, Synthetic, Glass, Custom.
#[must_use]
pub fn filter_counts(rows: &[ReferenceRow]) -> [usize; 5] {
    let mut counts = [rows.len(), 0, 0, 0, 0];
    for row in rows {
        let slot = match row.origin {
            Origin::Natural => 1,
            Origin::Synthetic => 2,
            Origin::Glass => 3,
            Origin::Custom => 4,
        };
        counts[slot] += 1;
    }
    counts
}

/// The tab-separated text of `rows`, header first, one line per row (Copy Table, the CLI).
///
/// The Dispersion column holds the figure of `scale` and says so in its heading; a value that is not
/// recorded is a dash.
#[must_use]
pub fn table_text(rows: &[&ReferenceRow], scale: DispersionScale) -> String {
    let header = [
        text::column_label(Column::Name),
        text::column_label(Column::CrystalSystem),
        text::column_label(Column::OpticCharacter),
        text::column_label(Column::Ri),
        text::column_label(Column::Birefringence),
        scale.column_label(),
        text::column_label(Column::SpecificGravity),
        text::column_label(Column::Hardness),
        text::column_label(Column::Toughness),
        text::column_label(Column::Cleavage),
    ];
    let mut out = header.join("\t");
    out.push('\n');
    for row in rows {
        let dispersion = match scale {
            DispersionScale::BG => &row.dispersion_bg_text,
            DispersionScale::FC => &row.dispersion_fc_text,
        };
        let fields = [
            row.name.as_str(),
            row.crystal_system.as_str(),
            row.optic_character.as_str(),
            row.ri_text.as_str(),
            row.birefringence_text.as_str(),
            dispersion.as_str(),
            row.sg_text.as_str(),
            or_dash(row.gem.hardness.as_deref()),
            or_dash(row.gem.toughness.as_deref()),
            or_dash(row.gem.cleavage.as_deref()),
        ];
        out.push_str(&fields.join("\t"));
        out.push('\n');
    }
    out
}
