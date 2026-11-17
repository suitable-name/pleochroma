//! The Material Reference dialog's data without the window: a thin adapter over the editor's
//! shared view model (`pleochroma_editor::material_reference`).
//!
//! The browser shows a compact table (Material, RI, Biref., Disp. as B-G, SG, Hardness,
//! Toughness, Cleavage) and a one-column details page. This module turns the shared rows into the plain
//! cells the table draws, keeps the sort and filter state's vocabulary (column and chip
//! indices), and builds the details page of one material for the stone width of the open
//! design. A custom material has no gem data in the browser, so its hardness, toughness and
//! cleavage read as not recorded, and the Design settings' facts line reads "No gem data yet".

use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::material::{MaterialCatalogue, built_in_gem_data};
use pleochroma_editor::{
    material_lookup::EditorMaterialLookup,
    material_reference::{
        Column, DetailContext, Filter, GemFacts, MaterialDetail, ReferenceRow,
        facts::{cleavage_warns, toughness_chip_text},
        filter_counts, filter_rows, material_detail_for_variant, reference_rows, sort_rows,
        text::{self, DASH},
    },
};

/// The table's columns in the order they are drawn; a header click names one by its index.
pub const COLUMNS: [Column; 8] = [
    Column::Name,
    Column::Ri,
    Column::Birefringence,
    Column::Dispersion,
    Column::SpecificGravity,
    Column::Hardness,
    Column::Toughness,
    Column::Cleavage,
];

/// The column at drawn position `index`; an out-of-range index reads as the Material column.
#[must_use]
pub fn column_for_index(index: i32) -> Column {
    usize::try_from(index)
        .ok()
        .and_then(|i| COLUMNS.get(i))
        .copied()
        .unwrap_or(Column::Name)
}

/// The drawn position of `column`, or -1 for a column the browser's table does not show.
#[must_use]
pub fn index_for_column(column: Column) -> i32 {
    COLUMNS
        .iter()
        .position(|&c| c == column)
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(-1)
}

/// The filter a chip id names: 0 All, 1 Natural, 2 Synthetic, 3 Glass, 4 My Materials.
#[must_use]
pub const fn filter_for_id(id: i32) -> Filter {
    match id {
        1 => Filter::Natural,
        2 => Filter::Synthetic,
        3 => Filter::Glass,
        4 => Filter::Custom,
        _ => Filter::All,
    }
}

/// What the table shows: the chip, the search text and the sort.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableState {
    /// The selected filter chip.
    pub filter: Filter,
    /// The search text; blank admits every row.
    pub query: String,
    /// The sorted column.
    pub column: Column,
    /// Whether the sort runs from the largest value down.
    pub descending: bool,
}

impl Default for TableState {
    fn default() -> Self {
        Self {
            filter: Filter::All,
            query: String::new(),
            column: Column::Name,
            descending: false,
        }
    }
}

/// One drawn row of the table: every cell is final text.
#[derive(Debug, Clone, PartialEq)]
pub struct TableRowView {
    /// The material's name.
    pub name: String,
    /// The mini-chip beside the name ("Synthetic", "Glass", "Mine"); empty for a natural one.
    pub tag: String,
    /// The face-up color at 7 mm, sRGB.
    pub swatch: [u8; 3],
    /// The refractive index `n_D`.
    pub ri: String,
    /// The birefringence, or a dash.
    pub birefringence: String,
    /// The B-G dispersion.
    pub dispersion: String,
    /// The specific gravity, or a dash.
    pub sg: String,
    /// The Mohs hardness text, or a dash.
    pub hardness: String,
    /// The Mohs bar's fill, 0 to 1 (the top of the range over 10); 0 for no hardness.
    pub hardness_fill: f32,
    /// The toughness as a short word ("Poor", "Fair–good"), or a dash.
    pub toughness: String,
    /// The cleavage, or a dash.
    pub cleavage: String,
}

/// A filter chip: its id (see [`filter_for_id`]) and its label with the count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterChipView {
    /// The chip's id.
    pub id: i32,
    /// "All 39".
    pub label: String,
}

/// The table as the dialog draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct TableView {
    /// The visible rows, sorted and filtered.
    pub rows: Vec<TableRowView>,
    /// The chips; "My Materials" is present only when the page holds a material of the user's own.
    pub chips: Vec<FilterChipView>,
    /// The subtitle under the title.
    pub subtitle: String,
    /// The footer status line.
    pub footer: String,
}

/// The rows of every material the page knows: the built-ins, then `custom`.
#[must_use]
pub fn reference_table(custom: &[GemMaterial]) -> Vec<ReferenceRow> {
    let catalogue = MaterialCatalogue::build(custom);
    let lookup = EditorMaterialLookup::new(custom);
    reference_rows(&catalogue, &lookup, &[])
}

/// The Mohs bar's fill of `facts`.
fn hardness_fill(facts: &GemFacts) -> f32 {
    facts
        .hardness_range
        .map_or(0.0, |(_, high)| (high / 10.0).clamp(0.0, 1.0))
}

/// A text cell: `value`, or a dash when the field is empty.
fn cell(value: Option<&str>) -> String {
    value.unwrap_or(DASH).to_owned()
}

/// The table's toughness cell: the chip text without its "Toughness " lead ("Poor", "Fair–good"),
/// or a dash when none is recorded.
fn toughness_cell(facts: &GemFacts) -> String {
    toughness_chip_text(facts)
        .and_then(|chip| chip.strip_prefix("Toughness ").map(str::to_owned))
        .unwrap_or_else(|| DASH.to_owned())
}

/// The drawn cells of one shared row.
fn row_view(row: &ReferenceRow) -> TableRowView {
    TableRowView {
        name: row.name.clone(),
        tag: row.origin.tag().to_owned(),
        swatch: row.swatch,
        ri: row.ri_text.clone(),
        birefringence: row.birefringence_text.clone(),
        dispersion: row.dispersion_bg_text.clone(),
        sg: row.sg_text.clone(),
        hardness: cell(row.gem.hardness.as_deref()),
        hardness_fill: hardness_fill(&row.gem),
        toughness: toughness_cell(&row.gem),
        cleavage: cell(row.gem.cleavage.as_deref()),
    }
}

/// The table for `state`: `all` sorted by the state's column, filtered by its chip and search
/// text, with the chip counts, the subtitle and the footer.
#[must_use]
pub fn table_view(all: &[ReferenceRow], state: &TableState) -> TableView {
    let mut sorted = all.to_vec();
    sort_rows(&mut sorted, state.column, state.descending);
    let shown = filter_rows(&sorted, state.filter, &state.query);
    let counts = filter_counts(all);
    let filters = [
        Filter::All,
        Filter::Natural,
        Filter::Synthetic,
        Filter::Glass,
        Filter::Custom,
    ];
    let chips = filters
        .iter()
        .zip(counts)
        .enumerate()
        .filter(|(_, (filter, count))| **filter != Filter::Custom || *count > 0)
        .filter_map(|(id, (filter, count))| {
            Some(FilterChipView {
                id: i32::try_from(id).ok()?,
                label: format!("{} {count}", text::filter_label(*filter)),
            })
        })
        .collect();
    TableView {
        rows: shown.iter().map(|row| row_view(row)).collect(),
        chips,
        subtitle: text::subtitle(counts[0] - counts[4], counts[4]),
        footer: text::footer_text(shown.len(), state.column, state.descending),
    }
}

/// What the Design settings' facts line says for a material of the user's own.
pub const NO_GEM_DATA: &str = "No gem data yet";

/// The gem facts of the combo entry `name`; `None` for an entry that is no material.
fn facts_for_name(name: &str) -> Option<GemFacts> {
    if name.starts_with("Custom RI") || name == "(none)" {
        return None;
    }
    Some(built_in_gem_data(name).map_or_else(GemFacts::custom_empty, GemFacts::from_built_in))
}

/// The facts line under the Material combo of the Design settings for the material called `name`.
///
/// "Mohs 8 · Poor · perfect basal {001}" for a built-in material, "No gem data yet"
/// ([`NO_GEM_DATA`]) for a material of the user's own, and an empty string for a combo entry
/// that is no material (the "Custom RI..." and "(none)" rows), which hides the line.
#[must_use]
pub fn gem_summary_for_name(name: &str) -> String {
    facts_for_name(name).map_or_else(String::new, |facts| {
        if facts.hardness.is_some() {
            facts.summary()
        } else {
            NO_GEM_DATA.to_owned()
        }
    })
}

/// Whether the facts line of `name` carries the cleavage warning (the strongest cleavage is
/// perfect, good or distinct), so the Design settings tint it.
#[must_use]
pub fn gem_cleavage_warns_for_name(name: &str) -> bool {
    facts_for_name(name).is_some_and(|facts| cleavage_warns(&facts))
}

/// The details page of the material `name`. `None` for a name the page does not know.
///
/// It shows the color variant `variant_key` (empty for the Material Default), for a stone
/// `stone_width_mm` wide (0 or less reads as the default 7 mm).
#[must_use]
pub fn detail(
    name: &str,
    custom: &[GemMaterial],
    stone_width_mm: f64,
    variant_key: &str,
) -> Option<MaterialDetail> {
    let lookup = EditorMaterialLookup::new(custom);
    let ctx = DetailContext {
        stone_width_mm,
        ..DetailContext::default()
    };
    material_detail_for_variant(name, &lookup, &ctx, variant_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom() -> [GemMaterial; 1] {
        [GemMaterial::new_custom(
            "Test Stone",
            1.62,
            0.02,
            0.0,
            [0.0; 3],
        )]
    }

    /// T-W1: the adapter maps every shared row to a drawn row, counts the chips, and shows a
    /// custom material as "Mine" with no gem data.
    #[test]
    fn t_w1_rows_map_the_view_model() {
        let custom = custom();
        let rows = reference_table(&custom);
        assert_eq!(rows.len(), GemMaterial::all_materials().len() + 1);
        let view = table_view(&rows, &TableState::default());
        assert_eq!(view.rows.len(), rows.len());
        assert_eq!(view.chips[0].label, format!("All {}", rows.len()));
        assert!(view.chips.iter().any(|chip| chip.id == 4));
        let mine = view
            .rows
            .iter()
            .find(|row| row.name == "Test Stone")
            .expect("the custom material has a row");
        assert_eq!(mine.tag, "Mine");
        assert_eq!(mine.hardness, DASH);
        assert_eq!(mine.cleavage, DASH);
        assert!(mine.hardness_fill.abs() < f32::EPSILON);
        let without = table_view(&reference_table(&[]), &TableState::default());
        assert!(without.chips.iter().all(|chip| chip.id != 4));
    }

    /// The table grades Topaz toughness Poor (GIA care page), and a custom material has none.
    #[test]
    fn t_w1_toughness_cell_follows_the_table() {
        let custom = custom();
        let view = table_view(&reference_table(&custom), &TableState::default());
        let cell_of = |name: &str| {
            view.rows
                .iter()
                .find(|row| row.name == name)
                .map(|row| row.toughness.clone())
        };
        assert_eq!(cell_of("Topaz").as_deref(), Some("Poor"));
        assert_eq!(cell_of("Test Stone").as_deref(), Some(DASH));
    }

    /// T-W1: sorting by hardness runs hardest first, the chip and the search narrow the rows.
    #[test]
    fn t_w1_sort_and_filter() {
        let custom = custom();
        let rows = reference_table(&custom);
        let hardest = table_view(
            &rows,
            &TableState {
                column: Column::Hardness,
                descending: true,
                ..TableState::default()
            },
        );
        let fills: Vec<f32> = hardest
            .rows
            .iter()
            .filter(|row| row.hardness != DASH)
            .map(|row| row.hardness_fill)
            .collect();
        assert!(!fills.is_empty());
        assert!(fills.windows(2).all(|pair| pair[0] >= pair[1]));
        assert_eq!(
            hardest.rows.last().map(|row| row.hardness.as_str()),
            Some(DASH),
            "a row with no hardness sorts last"
        );

        let mine = table_view(
            &rows,
            &TableState {
                filter: filter_for_id(4),
                ..TableState::default()
            },
        );
        assert_eq!(mine.rows.len(), 1);

        let found = table_view(
            &rows,
            &TableState {
                query: "diamond".to_owned(),
                ..TableState::default()
            },
        );
        assert!(found.rows.iter().any(|row| row.name == "Diamond"));
        assert!(found.rows.len() < rows.len());
    }

    /// T-W1: the details page builds for a built-in and a custom material, and not for an
    /// unknown name.
    #[test]
    fn t_w1_detail_maps_the_view_model() {
        let custom = custom();
        let diamond = detail("Diamond", &custom, 0.0, "").expect("Diamond has a page");
        assert_eq!(diamond.header.name, "Diamond");
        assert!(!diamond.ri.series.is_empty());
        assert!(!diamond.is_custom);
        let mine = detail("Test Stone", &custom, 6.0, "").expect("the custom material has a page");
        assert!(mine.is_custom);
        assert!(mine.durability.facts.is_empty());
        assert!(detail("No Such Material", &custom, 0.0, "").is_none());
    }

    /// T-W1: the Gem Data block's summary and the column and chip vocabularies.
    #[test]
    fn t_w1_summary_and_indices() {
        assert!(gem_summary_for_name("Diamond").starts_with("Mohs "));
        assert_eq!(gem_summary_for_name("Custom RI\u{2026}"), "");
        assert_eq!(gem_summary_for_name("(none)"), "");
        assert_eq!(gem_summary_for_name("Test Stone"), NO_GEM_DATA);
        assert!(gem_cleavage_warns_for_name("Topaz"));
        assert!(!gem_cleavage_warns_for_name("Test Stone"));
        assert!(!gem_cleavage_warns_for_name("(none)"));
        for (i, column) in COLUMNS.iter().enumerate() {
            let index = i32::try_from(i).expect("small index");
            assert_eq!(column_for_index(index), *column);
            assert_eq!(index_for_column(*column), index);
        }
        assert_eq!(column_for_index(99), Column::Name);
        assert_eq!(index_for_column(Column::Toughness), 6);
        assert_eq!(index_for_column(Column::Cleavage), 7);
        assert_eq!(index_for_column(Column::CrystalSystem), -1);
        assert_eq!(filter_for_id(0), Filter::All);
        assert_eq!(filter_for_id(3), Filter::Glass);
    }
}
