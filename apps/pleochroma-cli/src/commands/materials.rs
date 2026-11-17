//! `materials`: the optics and the gem data of the built-in materials (and of the custom
//! materials of a `--db` library).
//!
//! Without a name the command prints the Material Reference table, in text, JSON or CSV, or
//! one of the manual's tables as Markdown. With a name it describes that material: its optics,
//! colors as hex, color variants, effects, durability, weight and sources. The rows and the
//! gem facts come from the same view model the Material Reference window shows
//! ([`pleochroma_editor::material_reference`]), so the three front ends print the same words.
//!
//! This module also builds the gem data block of `info` ([`gem_block`], [`gem_data_json`]).

use super::deliver;
use crate::{
    args::{
        MarkdownTable, MaterialColumn, MaterialFilter, MaterialScale, MaterialsArgs,
        MaterialsFormat,
    },
    format::{Align, csv_line, json_number, json_number32, json_text, table},
    materials::{Catalogue, resolve_scoring},
    outcome::{CliError, CommandResult},
};
use pleochroma::{
    color::{
        body_color::{BodyColor, Illuminant, body_colors},
        principal_axes::{PrincipalFrame, face_up_body_color},
    },
    optics::{
        absorption::{AbsorptionBand, BandShape},
        inclusions::catalogue::presets_for,
        materials::{
            GemMaterial, MaterialLook, OpticalCharacter, optics_provenance,
            variants::{VariantProvenance, built_in_variants, face_up_swatch, face_up_swatch_path},
        },
    },
};
use pleochroma_cut_core::{
    Design, MaterialCatalogue, SgBasis, SpecificGravity, built_in_gem_data,
    built_in_specific_gravity,
    material::{CareTopic, GemBasis, GemData, GemField, GemSource, pattern::typical_patterns},
};
use pleochroma_editor::{
    material_lookup::EditorMaterialLookup,
    material_reference::{
        Column, DispersionScale, Filter, GemFacts, Origin, ReferenceRow, family_line, filter_rows,
        reference_rows, sort_rows,
        text::{DASH, column_label, sg_basis_note},
    },
};
use serde_json::{Value, json};
use std::fmt::Write as _;

/// The sodium D line, nanometers: where principal indices are quoted.
const LINE_D_NM: f32 = 589.3;

/// The reference stone width of the colors and the spectra, millimeters.
const REFERENCE_WIDTH_MM: f32 = 7.0;

/// The first and last wavelength of `--curves`, nanometers, and the step between rows.
const CURVE_RANGE_NM: (u16, u16, usize) = (380, 750, 5);

/// `2.355`: the FWHM of a Gaussian in units of its standard deviation.
const FWHM_PER_SIGMA: f32 = 2.354_82;

// --- Shared with `info` -------------------------------------------------------------------

/// The rows of the Material Reference table for `catalogue`, in catalogue order: the built-in
/// materials and the custom ones.
fn all_rows(catalogue: &Catalogue) -> Vec<ReferenceRow> {
    let material_catalogue = MaterialCatalogue::build(catalogue.custom());
    let lookup = EditorMaterialLookup::new(catalogue.custom());
    reference_rows(&material_catalogue, &lookup, &[])
}

/// The gem data of one material, as `info` and `materials NAME` print it.
pub(super) struct GemBlock {
    /// The material's name.
    name: String,
    /// Natural, synthetic, glass or the user's own.
    origin: Origin,
    /// The mineral species; empty for a custom material.
    species: String,
    /// The gem facts of the Material Reference.
    facts: GemFacts,
    /// Whether the material is one of the built-in ones.
    built_in: bool,
    /// The specific gravity, as text: "3.53 (3.49-3.57)".
    sg: Option<String>,
    /// The built-in specific gravity row.
    sg_row: Option<SpecificGravity>,
}

/// The gem data block of the material `design` names, or `None` when the design names no
/// material this build can resolve.
pub(super) fn gem_block(design: &Design, catalogue: &Catalogue) -> Option<GemBlock> {
    design.material.name.as_ref()?;
    let scoring = resolve_scoring(design, catalogue, None).ok()?;
    let name = scoring.resolved.gem.name;
    let row = all_rows(catalogue)
        .into_iter()
        .find(|row| row.name.eq_ignore_ascii_case(&name))?;
    Some(block_of(row, design.material.specific_gravity_override))
}

/// The block of one table row. A specific gravity entered in the design replaces the table's.
fn block_of(row: ReferenceRow, sg_override: Option<f64>) -> GemBlock {
    let built_in = row.origin != Origin::Custom;
    let sg_row = built_in_specific_gravity(&row.name);
    let sg = match (sg_override, &sg_row) {
        (Some(value), _) => Some(format!("{value:.2} (set in the design)")),
        (None, Some(found)) => Some(sg_text(found)),
        (None, None) => Some(row.sg_text.clone()).filter(|text| text != DASH),
    };
    GemBlock {
        name: row.name,
        origin: row.origin,
        species: row.species,
        facts: row.gem,
        built_in,
        sg,
        sg_row,
    }
}

/// A specific gravity as text: "3.53 (3.49-3.57)", or the single figure when the range has no
/// width.
fn sg_text(sg: &SpecificGravity) -> String {
    let (low, high) = sg.range;
    if (high - low).abs() < 1e-9 {
        format!("{:.2}", sg.representative)
    } else {
        format!("{:.2} ({low:.2}\u{2013}{high:.2})", sg.representative)
    }
}

/// `text` with its first letter lowercased.
fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(chars).collect()
    })
}

/// The one-line summary of the facts: "Mohs 8 · toughness poor · cleavage perfect basal {001}
/// · fracture subconchoidal to uneven". A built-in material says "not graded" or "not
/// recorded" for a field it has no source for; a custom material leaves such a field out.
fn summary_line(facts: &GemFacts, built_in: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(hardness) = &facts.hardness {
        parts.push(format!("Mohs {hardness}"));
    }
    if let Some(knoop) = &facts.knoop {
        parts.push(knoop.clone());
    }
    match (&facts.toughness, built_in) {
        (Some(toughness), _) => parts.push(format!("toughness {}", lower_first(toughness))),
        (None, true) => parts.push("toughness not graded".to_string()),
        (None, false) => {}
    }
    match (&facts.cleavage, built_in) {
        (Some(cleavage), _) => parts.push(format!("cleavage {}", lower_first(cleavage))),
        (None, true) => parts.push("cleavage not recorded".to_string()),
        (None, false) => {}
    }
    if let Some(parting) = facts
        .parting
        .as_ref()
        .filter(|text| text.as_str() != "None")
    {
        parts.push(format!("parting {}", lower_first(parting)));
    }
    match (&facts.fracture, built_in) {
        (Some(fracture), _) => parts.push(format!("fracture {}", lower_first(fracture))),
        (None, true) => parts.push("fracture not recorded".to_string()),
        (None, false) => {}
    }
    if parts.is_empty() {
        "Not recorded".to_string()
    } else {
        parts.join(" \u{b7} ")
    }
}

/// The labeled lines of the gem data block, as `info` prints them under its Material line:
/// `Gem data`, `SG`, `Care`, `Sources` and one `Note` per note.
pub(super) fn gem_lines(block: &GemBlock) -> Vec<(&'static str, String)> {
    let facts = &block.facts;
    let mut lines = vec![("Gem data", summary_line(facts, block.built_in))];
    if let Some(sg) = &block.sg {
        lines.push(("SG", sg.clone()));
    }
    if !facts.care.is_empty() {
        lines.push(("Care", facts.care.join(" ")));
    }
    if !facts.sources.is_empty() {
        lines.push(("Sources", facts.sources.join("; ")));
    }
    for note in &facts.notes {
        lines.push(("Note", note.clone()));
    }
    if block.origin != Origin::Custom
        && let Some(note) = sg_basis_note(&block.name)
        && !facts.notes.contains(&note)
    {
        lines.push(("Note", note));
    }
    lines
}

/// Every source the gem data of `name` cites, in field order, each once.
fn gem_sources(data: &GemData) -> Vec<GemSource> {
    let mut found: Vec<GemSource> = Vec::new();
    let mut add = |sources: &[GemSource]| {
        for source in sources {
            if !found.contains(source) {
                found.push(*source);
            }
        }
    };
    add(data.mohs.sources);
    if let Some(knoop) = &data.knoop {
        add(knoop.sources);
    }
    if let Some(toughness) = &data.toughness {
        add(toughness.sources);
    }
    if let Some(cleavage) = &data.cleavage {
        add(cleavage.sources);
    }
    if let Some(parting) = &data.parting {
        add(parting.sources);
    }
    if let Some(fracture) = &data.fracture {
        add(fracture.sources);
    }
    for (_, care) in data.care {
        add(care.sources);
    }
    for note in data.notes {
        add(note.sources);
    }
    found
}

/// A source as JSON: its full citation and its address, `null` for a source cited by title.
fn source_json(source: &GemSource) -> Value {
    json!({ "citation": source.citation(), "url": source.url() })
}

/// The name of a gem data field, for notes.
const fn field_name(field: GemField) -> &'static str {
    match field {
        GemField::Hardness => "hardness",
        GemField::Toughness => "toughness",
        GemField::Cleavage => "cleavage",
        GemField::Parting => "parting",
        GemField::Fracture => "fracture",
        GemField::Care => "care",
    }
}

/// The name of a care topic, for JSON.
const fn care_topic_name(topic: CareTopic) -> &'static str {
    match topic {
        CareTopic::Heat => "heat",
        CareTopic::ThermalShock => "thermal_shock",
        CareTopic::Chemicals => "chemicals",
        CareTopic::Light => "light",
        CareTopic::Cleaning => "cleaning",
        CareTopic::Cutting => "cutting",
    }
}

/// The word for a material's origin.
const fn origin_word(origin: Origin) -> &'static str {
    match origin {
        Origin::Natural => "natural",
        Origin::Synthetic => "synthetic",
        Origin::Glass => "glass",
        Origin::Custom => "custom",
    }
}

/// The specific gravity as JSON: the figure, its range and whether a source prints it.
fn sg_json(block: &GemBlock) -> Value {
    block.sg_row.as_ref().map_or_else(
        || json!({ "text": block.sg }),
        |sg| {
            let (basis, note) = match sg.basis {
                SgBasis::Cited => ("cited", None),
                SgBasis::Derived(note) => ("derived", Some(note)),
            };
            json!({
                "representative": json_number(sg.representative, 4),
                "low": json_number(sg.range.0, 4),
                "high": json_number(sg.range.1, 4),
                "basis": basis,
                "note": note,
                "text": block.sg,
            })
        },
    )
}

/// The `material.gem_data` object of the JSON reports: every field of the gem facts, the sources
/// with their full citations and addresses, the specific gravity, and for a built-in material
/// the figure-by-figure care topics, the notes' sources and the basis of the values.
pub(super) fn gem_data_json(block: &GemBlock) -> Value {
    let facts = &block.facts;
    let data = built_in_gem_data(&block.name);
    let sources: Vec<Value> = data
        .map(gem_sources)
        .unwrap_or_default()
        .iter()
        .map(source_json)
        .collect();
    let notes: Vec<Value> = data.map_or_else(Vec::new, |data| {
        data.notes
            .iter()
            .map(|note| {
                json!({
                    "field": field_name(note.field),
                    "text": note.text,
                    "sources": note.sources.iter().map(source_json).collect::<Vec<_>>(),
                })
            })
            .collect()
    });
    let care: Vec<Value> = data.map_or_else(Vec::new, |data| {
        data.care
            .iter()
            .map(|(topic, cited)| {
                json!({
                    "topic": care_topic_name(*topic),
                    "text": cited.value,
                    "sources": cited.sources.iter().map(source_json).collect::<Vec<_>>(),
                })
            })
            .collect()
    });
    let basis = data.map(|data| match data.basis {
        GemBasis::Species => json!({ "kind": "species", "note": Value::Null }),
        GemBasis::Inferred(note) => json!({ "kind": "inferred", "note": note }),
        GemBasis::Derived(note) => json!({ "kind": "derived", "note": note }),
    });
    json!({
        "built_in": block.built_in,
        "origin": origin_word(block.origin),
        "species": Some(block.species.as_str()).filter(|species| !species.is_empty()),
        "summary": summary_line(facts, block.built_in),
        "hardness": facts.hardness,
        "hardness_range": facts
            .hardness_range
            .map(|(low, high)| [json_number32(low, 4), json_number32(high, 4)]),
        "knoop": facts.knoop,
        "toughness": facts.toughness,
        "toughness_rank": facts.toughness_rank,
        "cleavage": facts.cleavage,
        "cleavage_rank": facts.cleavage_rank,
        "parting": facts.parting,
        "fracture": facts.fracture,
        "care": facts.care,
        "care_topics": care,
        "notes": facts.notes,
        "source_notes": notes,
        "sources": facts.sources,
        "source_links": sources,
        "basis": basis,
        "specific_gravity": sg_json(block),
        "user_entered": facts.user_entered,
    })
}

// --- The command ---------------------------------------------------------------------------

/// The view model's column for a `--sort` word.
const fn column_of(column: MaterialColumn) -> Column {
    match column {
        MaterialColumn::Name => Column::Name,
        MaterialColumn::CrystalSystem => Column::CrystalSystem,
        MaterialColumn::OpticCharacter => Column::OpticCharacter,
        MaterialColumn::Ri => Column::Ri,
        MaterialColumn::Birefringence => Column::Birefringence,
        MaterialColumn::Dispersion => Column::Dispersion,
        MaterialColumn::SpecificGravity => Column::SpecificGravity,
        MaterialColumn::Hardness => Column::Hardness,
        MaterialColumn::Toughness => Column::Toughness,
        MaterialColumn::Cleavage => Column::Cleavage,
    }
}

/// The view model's filter for a `--filter` word.
const fn filter_of(filter: MaterialFilter) -> Filter {
    match filter {
        MaterialFilter::All => Filter::All,
        MaterialFilter::Natural => Filter::Natural,
        MaterialFilter::Synthetic => Filter::Synthetic,
        MaterialFilter::Glass => Filter::Glass,
        MaterialFilter::Custom => Filter::Custom,
    }
}

/// The view model's dispersion scale for a `--dispersion` word.
const fn scale_of(scale: MaterialScale) -> DispersionScale {
    match scale {
        MaterialScale::Bg => DispersionScale::BG,
        MaterialScale::Fc => DispersionScale::FC,
    }
}

/// The dispersion text of `row` in `scale`.
const fn dispersion_text(row: &ReferenceRow, scale: MaterialScale) -> &String {
    match scale {
        MaterialScale::Bg => &row.dispersion_bg_text,
        MaterialScale::Fc => &row.dispersion_fc_text,
    }
}

/// `text`, or the dash when it is absent.
fn or_dash(text: Option<&String>) -> String {
    text.cloned().unwrap_or_else(|| DASH.to_string())
}

/// The rows the options select, in the order they ask for.
fn selected_rows(catalogue: &Catalogue, args: &MaterialsArgs) -> Vec<ReferenceRow> {
    let mut rows = all_rows(catalogue);
    if let Some(column) = args.sort {
        sort_rows(&mut rows, column_of(column), args.descending);
    } else if args.descending {
        sort_rows(&mut rows, Column::Name, true);
    }
    let kept: Vec<String> = filter_rows(&rows, filter_of(args.filter), "")
        .into_iter()
        .map(|row| row.name.clone())
        .collect();
    rows.retain(|row| kept.contains(&row.name));
    rows
}

/// The table as aligned text.
fn text_table(rows: &[ReferenceRow], scale: MaterialScale) -> String {
    let headings = [
        column_label(Column::Name),
        column_label(Column::CrystalSystem),
        column_label(Column::OpticCharacter),
        column_label(Column::Ri),
        column_label(Column::Birefringence),
        scale_of(scale).column_label(),
        column_label(Column::SpecificGravity),
        column_label(Column::Hardness),
        column_label(Column::Toughness),
        column_label(Column::Cleavage),
    ];
    let aligns = [
        Align::Left,
        Align::Left,
        Align::Left,
        Align::Right,
        Align::Right,
        Align::Right,
        Align::Right,
        Align::Right,
        Align::Left,
        Align::Left,
    ];
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.name.clone(),
                row.crystal_system.clone(),
                row.optic_character.clone(),
                row.ri_text.clone(),
                row.birefringence_text.clone(),
                dispersion_text(row, scale).clone(),
                row.sg_text.clone(),
                or_dash(row.gem.hardness.as_ref()),
                or_dash(row.gem.toughness.as_ref()),
                or_dash(row.gem.cleavage.as_ref()),
            ]
        })
        .collect();
    let mut text = table(&headings, &aligns, &cells);
    let _ = writeln!(
        text,
        "\n{} materials. Values are typical for each species; a dash means not recorded.",
        rows.len()
    );
    text
}

/// The table as CSV: one line per material with the full gem facts.
fn csv_table(rows: &[ReferenceRow], scale: MaterialScale) -> String {
    let dispersion = match scale {
        MaterialScale::Bg => "dispersion_bg",
        MaterialScale::Fc => "dispersion_fc",
    };
    let mut text = csv_line(&[
        "name",
        "origin",
        "species",
        "crystal_system",
        "optic_character",
        "refractive_index",
        "birefringence",
        dispersion,
        "specific_gravity",
        "mohs",
        "toughness",
        "cleavage",
        "parting",
        "fracture",
    ]);
    for row in rows {
        text.push_str(&csv_line(&[
            row.name.as_str(),
            origin_word(row.origin),
            row.species.as_str(),
            row.crystal_system.as_str(),
            row.optic_character.as_str(),
            row.ri_text.as_str(),
            row.birefringence_text.as_str(),
            dispersion_text(row, scale).as_str(),
            row.sg_text.as_str(),
            row.gem.hardness.as_deref().unwrap_or(""),
            row.gem.toughness.as_deref().unwrap_or(""),
            row.gem.cleavage.as_deref().unwrap_or(""),
            row.gem.parting.as_deref().unwrap_or(""),
            row.gem.fracture.as_deref().unwrap_or(""),
        ]));
    }
    text
}

/// The optics figures of `gem` as JSON numbers.
fn optics_json(gem: &GemMaterial) -> Value {
    json!({
        "refractive_index": json_number32(gem.dispersion.n_d(), 6),
        "birefringence": json_number32(gem.birefringence_delta, 6),
        "dispersion_bg": json_number32(gem.dispersion.delta_b_g(), 6),
        "dispersion_fc": json_number32(gem.dispersion.delta_f_c(), 6),
        "abbe_number": gem.dispersion.abbe_number().map(|abbe| json_number32(abbe, 3)),
    })
}

/// A body color as `#rrggbb`.
fn hex(color: &BodyColor) -> String {
    hex_of_rgb(color.srgb)
}

/// The table as JSON: every row with its numbers, gem data, sources and addresses.
fn json_table(catalogue: &Catalogue, rows: Vec<ReferenceRow>, args: &MaterialsArgs) -> String {
    let count = rows.len();
    let materials: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            let optics = catalogue
                .lookup_exact(&row.name)
                .as_ref()
                .map_or(Value::Null, optics_json);
            let (name, swatch) = (row.name.clone(), hex_of_rgb(row.swatch));
            let origin = origin_word(row.origin);
            let (system, character) = (row.crystal_system.clone(), row.optic_character.clone());
            let block = block_of(row, None);
            json!({
                "name": name,
                "origin": origin,
                "swatch": swatch,
                "crystal_system": system,
                "optic_character": character,
                "optics": optics,
                "gem_data": gem_data_json(&block),
            })
        })
        .collect();
    json_text(&json!({
        "count": count,
        "dispersion_scale": match args.scale {
            MaterialScale::Bg => "bg",
            MaterialScale::Fc => "fc",
        },
        "materials": materials,
    }))
}

/// `#rrggbb` of an 8-bit color.
fn hex_of_rgb(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

// --- Markdown tables of the manual ---------------------------------------------------------

/// A Markdown table cell: pipes escaped, line breaks folded.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// A Markdown table with a header row and rows of cells.
fn markdown_rows(headings: &[&str], rows: &[Vec<String>]) -> String {
    let mut text = format!("| {} |\n", headings.join(" | "));
    let _ = writeln!(text, "|{}", "---|".repeat(headings.len()));
    for row in rows {
        let cells: Vec<String> = row.iter().map(|text| cell(text)).collect();
        let _ = writeln!(text, "| {} |", cells.join(" | "));
    }
    text
}

/// The birefringence with its sign, as the manual prints it ("+0.0076", with a minus sign for a
/// negative material), and a dash for an isotropic material.
fn signed_birefringence(gem: &GemMaterial) -> String {
    let magnitude = f64::from(gem.birefringence_delta).abs();
    match gem.optical_character {
        OpticalCharacter::Isotropic => DASH.to_string(),
        OpticalCharacter::UniaxialPositive | OpticalCharacter::BiaxialPositive => {
            format!("+{magnitude:.4}")
        }
        OpticalCharacter::UniaxialNegative | OpticalCharacter::BiaxialNegative => {
            format!("\u{2212}{magnitude:.4}")
        }
    }
}

/// The Abbe number as the manual prints it: "55.3", or a dash for a material with none.
fn abbe_text(gem: &GemMaterial) -> String {
    gem.dispersion
        .abbe_number()
        .map_or_else(|| DASH.to_string(), |abbe| format!("{abbe:.1}"))
}

/// The optics table of Appendix C.
fn markdown_optics(catalogue: &Catalogue, rows: &[ReferenceRow], scale: MaterialScale) -> String {
    let heading = match scale {
        MaterialScale::Bg => "Dispersion (B\u{2013}G)",
        MaterialScale::Fc => "Dispersion (F\u{2013}C)",
    };
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let gem = catalogue.lookup_exact(&row.name);
            vec![
                row.name.clone(),
                row.optic_character.clone(),
                row.ri_text.clone(),
                gem.as_ref()
                    .map_or_else(|| row.birefringence_text.clone(), signed_birefringence),
                dispersion_text(row, scale).clone(),
                gem.as_ref().map_or_else(|| DASH.to_string(), abbe_text),
            ]
        })
        .collect();
    markdown_rows(
        &[
            "Material",
            "Optical character",
            "n_D",
            "Birefringence (\u{394}n)",
            heading,
            "Abbe V_d",
        ],
        &cells,
    )
}

/// The Mohs figure as the gem data table prints it, with the Knoop figure of a glass.
fn mohs_cell(facts: &GemFacts) -> String {
    match (&facts.hardness, &facts.knoop) {
        (Some(mohs), Some(knoop)) => format!("{mohs}; {knoop}"),
        (Some(mohs), None) => mohs.clone(),
        (None, Some(knoop)) => knoop.clone(),
        (None, None) => "not recorded".to_string(),
    }
}

/// The gem data table of Appendix C.
fn markdown_gem_data(rows: &[ReferenceRow]) -> String {
    let cells: Vec<Vec<String>> =
        rows.iter()
            .map(|row| {
                let built_in = built_in_gem_data(&row.name).is_some();
                let facts = &row.gem;
                let missing = if built_in { "not recorded" } else { DASH };
                let sg = built_in_specific_gravity(&row.name)
                    .map_or_else(|| row.sg_text.clone(), |sg| sg_text(&sg));
                vec![
                    row.name.clone(),
                    mohs_cell(facts),
                    facts.toughness.clone().unwrap_or_else(|| {
                        (if built_in { "not graded" } else { DASH }).to_string()
                    }),
                    facts
                        .cleavage
                        .clone()
                        .unwrap_or_else(|| missing.to_string()),
                    facts.parting.clone().unwrap_or_else(|| DASH.to_string()),
                    facts
                        .fracture
                        .clone()
                        .unwrap_or_else(|| missing.to_string()),
                    sg,
                ]
            })
            .collect();
    markdown_rows(
        &[
            "Material",
            "Mohs",
            "Toughness",
            "Cleavage",
            "Parting",
            "Fracture",
            "SG (range)",
        ],
        &cells,
    )
}

/// The built-in material list of the materials chapter.
fn markdown_built_in(catalogue: &Catalogue, rows: &[ReferenceRow]) -> String {
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let gem = catalogue.lookup_exact(&row.name);
            vec![
                row.name.clone(),
                row.ri_text.clone(),
                gem.as_ref()
                    .map_or_else(|| row.birefringence_text.clone(), signed_birefringence),
                row.optic_character.clone(),
                row.sg_text.clone(),
                or_dash(row.gem.hardness.as_ref()),
            ]
        })
        .collect();
    markdown_rows(
        &[
            "Material",
            "n_D",
            "Birefringence (\u{394}n)",
            "Optical character",
            "SG",
            "Hardness (Mohs)",
        ],
        &cells,
    )
}

/// The source list of Appendix C: each source the gem data cites with its address, then the
/// notes where sources differ and the basis notes of values taken from a species or derived.
fn markdown_sources(rows: &[ReferenceRow]) -> String {
    let mut sources: Vec<GemSource> = Vec::new();
    for row in rows {
        if let Some(data) = built_in_gem_data(&row.name) {
            for source in gem_sources(data) {
                if !sources.contains(&source) {
                    sources.push(source);
                }
            }
        }
    }
    let cells: Vec<Vec<String>> = sources
        .iter()
        .map(|source| {
            vec![
                source.citation(),
                source.url().unwrap_or_else(|| DASH.to_string()),
            ]
        })
        .collect();
    let mut text = markdown_rows(&["Source", "Address"], &cells);
    let mut notes = String::new();
    for row in rows {
        let Some(data) = built_in_gem_data(&row.name) else {
            continue;
        };
        for note in data.notes {
            let _ = writeln!(
                notes,
                "- **{}**, {}: {}",
                row.name,
                field_name(note.field),
                note.text
            );
        }
        match data.basis {
            GemBasis::Species => {}
            GemBasis::Inferred(note) | GemBasis::Derived(note) => {
                let _ = writeln!(notes, "- **{}**: {note}", row.name);
            }
        }
        if let Some(note) = sg_basis_note(&row.name) {
            let _ = writeln!(notes, "- **{}**, specific gravity: {note}", row.name);
        }
    }
    if !notes.is_empty() {
        text.push_str("\nNotes where sources differ or a value is derived:\n\n");
        text.push_str(&notes);
    }
    text
}

// --- One material --------------------------------------------------------------------------

/// The principal indices of `gem` at `lambda_nm`: one for an isotropic material, two for a
/// uniaxial one (omega, epsilon), three for a biaxial one (alpha, beta, gamma).
fn principal_indices(gem: &GemMaterial, lambda_nm: f32) -> Vec<(&'static str, f32)> {
    let n_base = gem.dispersion.evaluate(lambda_nm);
    match gem.optical_character {
        OpticalCharacter::Isotropic => vec![("n", n_base)],
        OpticalCharacter::UniaxialPositive | OpticalCharacter::UniaxialNegative => vec![
            ("n\u{3c9}", n_base),
            ("n\u{3b5}", gem.extraordinary_index_at(lambda_nm, n_base)),
        ],
        OpticalCharacter::BiaxialPositive | OpticalCharacter::BiaxialNegative => gem
            .biaxial_indicatrix(lambda_nm)
            .map_or_else(Vec::new, |indicatrix| {
                vec![
                    ("n\u{3b1}", indicatrix.n_alpha),
                    ("n\u{3b2}", indicatrix.n_beta),
                    ("n\u{3b3}", indicatrix.n_gamma),
                ]
            }),
    }
}

/// The labels of the rays of `gem`'s absorption tensor, in the order o, (beta), e. A material
/// whose absorption does not depend on direction has one ray.
fn ray_labels(gem: &GemMaterial) -> Vec<&'static str> {
    if gem.absorption.beta_ray.is_some() {
        vec!["\u{3b1}", "\u{3b2}", "\u{3b3}"]
    } else if gem.absorption.is_pleochroic {
        vec!["\u{3c9} (ordinary)", "\u{3b5} (along c)"]
    } else {
        vec!["all directions"]
    }
}

/// The absorption bands of each ray of `gem`, labeled, in the order of [`ray_labels`].
fn ray_bands(gem: &GemMaterial) -> Vec<(&'static str, &[AbsorptionBand])> {
    let tensor = &gem.absorption;
    let labels = ray_labels(gem);
    match (&tensor.beta_ray, labels.as_slice()) {
        (Some(beta), [alpha, beta_label, gamma]) => vec![
            (*alpha, tensor.o_ray.as_slice()),
            (*beta_label, beta.as_slice()),
            (*gamma, tensor.e_ray.as_slice()),
        ],
        (None, [omega, epsilon]) => vec![
            (*omega, tensor.o_ray.as_slice()),
            (*epsilon, tensor.e_ray.as_slice()),
        ],
        _ => vec![("all directions", tensor.o_ray.as_slice())],
    }
}

/// How wide a band is: "FWHM 3461 cm\u{207b}\u{b9}" for a band that is Gaussian in wavenumber,
/// "\u{3c3} 20 nm" for one that is Gaussian in wavelength.
fn width_text(band: &AbsorptionBand) -> String {
    match band.shape {
        BandShape::GaussianEnergy => {
            format!(
                "FWHM {:.0} cm\u{207b}\u{b9}",
                band.width_nm * FWHM_PER_SIGMA
            )
        }
        BandShape::GaussianWavelength => format!("\u{3c3} {:.0} nm", band.width_nm),
    }
}

/// The face-up color of `gem` for `illuminant`, in the orientation its Crystal Axis default
/// gives the stone (the Material Reference's face-up swatch).
fn face_up(gem: &GemMaterial, illuminant: Illuminant) -> BodyColor {
    let path = face_up_swatch_path(gem.absorption_unit, REFERENCE_WIDTH_MM);
    let frame = gem
        .absorption
        .is_pleochroic
        .then(|| PrincipalFrame::of_material(gem));
    face_up_body_color(&gem.absorption, frame.as_ref(), path, illuminant)
}

/// The color of each ray in daylight (D65) and incandescent light (3200 K), as hex.
fn ray_colors(gem: &GemMaterial) -> Vec<(&'static str, String, String)> {
    let path = face_up_swatch_path(gem.absorption_unit, REFERENCE_WIDTH_MM);
    let labels = ray_labels(gem);
    let per_ray = |illuminant: Illuminant| -> Vec<BodyColor> {
        let colors = body_colors(&gem.absorption, path, illuminant);
        match (&colors.beta_ray, labels.len()) {
            (Some(beta), 3) => vec![colors.o_ray, *beta, colors.e_ray],
            (None, 2) => vec![colors.o_ray, colors.e_ray],
            _ => vec![colors.unpolarised],
        }
    };
    let daylight = per_ray(Illuminant::D65);
    let incandescent = per_ray(Illuminant::Planckian(3200.0));
    labels
        .iter()
        .zip(daylight.iter().zip(incandescent.iter()))
        .map(|(label, (day, lamp))| (*label, hex(day), hex(lamp)))
        .collect()
}

/// The effects a material offers: inclusion presets, structural color, a milky body, and the
/// typical color zoning of the species.
fn effects(name: &str) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    let presets: Vec<&str> = presets_for(name).iter().map(|preset| preset.name).collect();
    if !presets.is_empty() {
        lines.push(("Inclusions", presets.join(", ")));
    }
    let look = MaterialLook::of(name);
    if look.structure.is_some() {
        lines.push((
            "Structural color",
            "play of color or schiller, from the stone's structure".to_string(),
        ));
    }
    if look.haze_sigma_s.is_some() {
        lines.push(("Body", "milky or translucent".to_string()));
    }
    let patterns: Vec<&str> = typical_patterns(name)
        .iter()
        .map(|pattern| pattern.label)
        .collect();
    if !patterns.is_empty() {
        lines.push(("Color pattern", patterns.join(", ")));
    }
    lines
}

/// "N % heavier than diamond at the same size", or lighter, for a specific gravity.
fn weight_line(sg: &SpecificGravity) -> Option<String> {
    let diamond = built_in_specific_gravity("Diamond")?;
    let percent = (sg.representative / diamond.representative - 1.0) * 100.0;
    let word = if percent >= 0.0 { "heavier" } else { "lighter" };
    Some(format!(
        "{}: {:.0} % {word} than diamond at the same size",
        sg_text(sg),
        percent.abs()
    ))
}

/// One ray's absorption bands: the ray's label, then each band's center, width text and peak.
type RayBands = (String, Vec<(f32, String, f32)>);

/// The labeled lines of the details of one material, grouped under section headings.
struct Detail {
    /// The material's name.
    name: String,
    /// The family line: species, crystal system, optic character and where it comes from.
    family: String,
    /// The refractive-index and dispersion figures.
    optics: Vec<(String, String)>,
    /// The sources of the optical data.
    optics_sources: Vec<(String, String)>,
    /// Color per ray, then face up: label, daylight hex, incandescent hex.
    colors: Vec<(String, String, String)>,
    /// Color variants: label, face-up hex, first source, whether it is an estimate.
    variants: Vec<(String, String, String, bool)>,
    /// The effects.
    effects: Vec<(&'static str, String)>,
    /// The absorption bands of each ray: label, then band lines.
    bands: Vec<RayBands>,
    /// The gem data.
    block: GemBlock,
}

/// What the table and the gem data know about `gem`, for one material's report.
fn detail_of(gem: &GemMaterial, row: ReferenceRow) -> Detail {
    let name = gem.name.clone();
    let mut family = family_line(gem, row.origin);
    if !row.species.is_empty() && !row.species.eq_ignore_ascii_case(&name) {
        family = format!("{} \u{b7} {family}", row.species);
    }

    let mut optics = vec![
        ("Crystal system".to_string(), row.crystal_system.clone()),
        ("Optic character".to_string(), row.optic_character.clone()),
        ("Refractive index (n_D)".to_string(), row.ri_text.clone()),
        ("Birefringence".to_string(), row.birefringence_text.clone()),
        (
            "Dispersion (B\u{2013}G)".to_string(),
            row.dispersion_bg_text.clone(),
        ),
        (
            "Dispersion (F\u{2013}C)".to_string(),
            row.dispersion_fc_text.clone(),
        ),
        ("Abbe number".to_string(), abbe_text(gem)),
        (
            "Critical angle".to_string(),
            format!(
                "{:.1}\u{b0}",
                (1.0 / f64::from(gem.dispersion.n_d())).asin().to_degrees()
            ),
        ),
    ];
    let indices = principal_indices(gem, LINE_D_NM);
    if indices.len() > 1 {
        optics.push((
            "Principal indices (D line)".to_string(),
            indices
                .iter()
                .map(|(label, value)| format!("{label} {value:.4}"))
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    let mut optics_sources = Vec::new();
    if let Some(provenance) = optics_provenance(&name) {
        optics_sources.push((
            "Refractive index".to_string(),
            provenance.dispersion.to_string(),
        ));
        let estimate = if provenance.absorption_estimated {
            " (estimated)"
        } else {
            ""
        };
        optics_sources.push((
            "Absorption".to_string(),
            format!("{}{estimate}", provenance.absorption),
        ));
    }

    let mut colors: Vec<(String, String, String)> = ray_colors(gem)
        .into_iter()
        .map(|(label, day, lamp)| (label.to_string(), day, lamp))
        .collect();
    colors.push((
        "face up".to_string(),
        hex(&face_up(gem, Illuminant::D65)),
        hex(&face_up(gem, Illuminant::Planckian(3200.0))),
    ));

    let variants = variant_lines(gem, &name);
    let bands = band_lines(gem);

    Detail {
        effects: effects(&name),
        block: block_of(row, None),
        name,
        family,
        optics,
        optics_sources,
        colors,
        variants,
        bands,
    }
}

/// The color variants of material `name` for its report: label, face-up hex, first source and
/// whether it is an estimate.
fn variant_lines(gem: &GemMaterial, name: &str) -> Vec<(String, String, String, bool)> {
    built_in_variants(name)
        .into_iter()
        .map(|variant| {
            let swatch = face_up_swatch(
                &variant.absorption,
                variant.unit,
                f64::from(variant.reference_path_mm),
                gem,
            );
            let (source, estimated) = match &variant.provenance {
                VariantProvenance::BuiltIn {
                    sources, estimated, ..
                } => (sources.first().cloned().unwrap_or_default(), *estimated),
                VariantProvenance::MaterialDefault => ("the material's own spectrum".into(), false),
                VariantProvenance::User => (String::new(), false),
            };
            (variant.label, hex(&swatch), source, estimated)
        })
        .collect()
}

/// The absorption bands of each ray of `gem` for its report, strongest peak first.
fn band_lines(gem: &GemMaterial) -> Vec<RayBands> {
    ray_bands(gem)
        .into_iter()
        .map(|(label, list)| {
            let mut sorted: Vec<&AbsorptionBand> = list.iter().collect();
            sorted.sort_by(|a, b| b.peak.total_cmp(&a.peak));
            (
                label.to_string(),
                sorted
                    .into_iter()
                    .map(|band| (band.center_nm, width_text(band), band.peak))
                    .collect(),
            )
        })
        .collect()
}

/// The text of one material's report.
fn detail_text(detail: &Detail, with_bands: bool) -> String {
    let mut text = format!("{}\n{}\n", detail.name, detail.family);
    text.push_str("\nOptics\n");
    for (label, value) in &detail.optics {
        let _ = writeln!(text, "  {:<29}{value}", format!("{label}:"));
    }
    for (label, value) in &detail.optics_sources {
        let _ = writeln!(text, "  {:<29}{value}", format!("Source, {label}:"));
    }
    text.push_str("\nColors (7 mm stone, face up)\n");
    let width = detail
        .colors
        .iter()
        .map(|(label, _, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    let _ = writeln!(
        text,
        "  {:<width$}  {:<9}  Incandescent (3200 K)",
        "", "Daylight"
    );
    for (label, day, lamp) in &detail.colors {
        let _ = writeln!(text, "  {label:<width$}  {day:<9}  {lamp}");
    }
    if !detail.variants.is_empty() {
        text.push_str("\nColor variants\n");
        for (label, swatch, source, estimated) in &detail.variants {
            let estimate = if *estimated { " (estimated)" } else { "" };
            let source = if source.is_empty() {
                String::new()
            } else {
                format!("  {source}")
            };
            let _ = writeln!(text, "  {label}  {swatch}{estimate}{source}");
        }
    }
    if !detail.effects.is_empty() {
        text.push_str("\nEffects\n");
        for (label, value) in &detail.effects {
            let _ = writeln!(text, "  {:<18}{value}", format!("{label}:"));
        }
    }
    if with_bands {
        text.push_str("\nAbsorption bands\n");
        for (label, bands) in &detail.bands {
            let _ = writeln!(text, "  {label}");
            if bands.is_empty() {
                text.push_str("    none\n");
            }
            for (center, width, peak) in bands {
                let _ = writeln!(text, "    {center:.1} nm  {width}  peak {peak:.4}");
            }
        }
    }
    text.push_str("\nDurability and weight\n");
    for (label, value) in gem_lines(&detail.block) {
        let _ = writeln!(text, "  {:<10}{value}", format!("{label}:"));
    }
    if let Some(sg) = &detail.block.sg_row
        && let Some(line) = weight_line(sg)
    {
        let _ = writeln!(text, "  {:<10}{line}", "Weight:");
    }
    text
}

/// The JSON of one material's report.
fn detail_json(detail: &Detail, gem: &GemMaterial, with_bands: bool) -> Value {
    let pair = |entries: &[(String, String)]| -> Value {
        Value::Array(
            entries
                .iter()
                .map(|(label, value)| json!({ "label": label, "value": value }))
                .collect(),
        )
    };
    let bands: Value = if with_bands {
        Value::Array(
            detail
                .bands
                .iter()
                .map(|(ray, bands)| {
                    json!({
                        "ray": ray,
                        "bands": bands
                            .iter()
                            .map(|(center, width, peak)| json!({
                                "center_nm": json_number32(*center, 2),
                                "width": width,
                                "peak": json_number32(*peak, 5),
                            }))
                            .collect::<Vec<_>>(),
                    })
                })
                .collect(),
        )
    } else {
        Value::Null
    };
    json!({
        "name": detail.name,
        "family": detail.family,
        "optics": pair(&detail.optics),
        "optics_figures": optics_json(gem),
        "optics_sources": pair(&detail.optics_sources),
        "colors": detail
            .colors
            .iter()
            .map(|(ray, daylight, incandescent)| json!({
                "ray": ray,
                "daylight": daylight,
                "incandescent": incandescent,
            }))
            .collect::<Vec<_>>(),
        "variants": detail
            .variants
            .iter()
            .map(|(label, swatch, source, estimated)| json!({
                "label": label,
                "swatch": swatch,
                "source": source,
                "estimated": estimated,
            }))
            .collect::<Vec<_>>(),
        "effects": detail
            .effects
            .iter()
            .map(|(label, value)| json!({ "label": label, "value": value }))
            .collect::<Vec<_>>(),
        "bands": bands,
        "gem_data": gem_data_json(&detail.block),
    })
}

/// The refractive index of every direction against wavelength, as CSV.
fn curves_csv(gem: &GemMaterial) -> String {
    let labels: Vec<&str> = match gem.optical_character {
        OpticalCharacter::Isotropic => vec!["n"],
        OpticalCharacter::UniaxialPositive | OpticalCharacter::UniaxialNegative => {
            vec!["n_omega", "n_epsilon"]
        }
        OpticalCharacter::BiaxialPositive | OpticalCharacter::BiaxialNegative => {
            vec!["n_alpha", "n_beta", "n_gamma"]
        }
    };
    let mut header = vec!["wavelength_nm"];
    header.extend(labels);
    let mut text = csv_line(&header);
    let (first, last, step) = CURVE_RANGE_NM;
    for nm in (first..=last).step_by(step) {
        let lambda = f32::from(nm);
        let mut fields = vec![format!("{nm}")];
        fields.extend(
            principal_indices(gem, lambda)
                .iter()
                .map(|(_, value)| format!("{value:.5}")),
        );
        text.push_str(&csv_line(&fields));
    }
    text
}

/// The error for a name no material answers to.
fn unknown_material(name: &str, catalogue: &Catalogue) -> CliError {
    let hint = if catalogue.custom().is_empty() {
        " (custom materials need --db)"
    } else {
        ""
    };
    CliError::usage(format!(
        "unknown material {name:?}: it is not a built-in material{hint}. Available: {}",
        catalogue.names().join(", ")
    ))
}

#[cfg(test)]
mod tests;

/// Runs `materials`.
///
/// # Errors
///
/// [`CliError`] when the library cannot be opened, or the name matches no material.
pub fn run(args: &MaterialsArgs) -> CommandResult {
    let catalogue = Catalogue::open(args.db.as_deref(), None)?;
    let Some(name) = &args.name else {
        let rows = selected_rows(&catalogue, args);
        let report = match args.format {
            MaterialsFormat::Text => text_table(&rows, args.scale),
            MaterialsFormat::Csv => csv_table(&rows, args.scale),
            MaterialsFormat::Json => json_table(&catalogue, rows, args),
            MaterialsFormat::Markdown(MarkdownTable::Optics) => {
                markdown_optics(&catalogue, &rows, args.scale)
            }
            MaterialsFormat::Markdown(MarkdownTable::GemData) => markdown_gem_data(&rows),
            MaterialsFormat::Markdown(MarkdownTable::BuiltIn) => {
                markdown_built_in(&catalogue, &rows)
            }
            MaterialsFormat::Markdown(MarkdownTable::Sources) => markdown_sources(&rows),
        };
        return Ok(deliver(report, args.out.as_deref()));
    };
    let gem = catalogue
        .lookup_exact(name)
        .ok_or_else(|| unknown_material(name, &catalogue))?;
    let row = all_rows(&catalogue)
        .into_iter()
        .find(|row| row.name.eq_ignore_ascii_case(&gem.name))
        .ok_or_else(|| unknown_material(name, &catalogue))?;
    let detail = detail_of(&gem, row);
    let report = match args.format {
        MaterialsFormat::Json => json_text(&detail_json(&detail, &gem, args.bands)),
        _ => detail_text(&detail, args.bands),
    };
    let mut outcome = deliver(report, args.out.as_deref());
    if let Some(path) = &args.curves {
        outcome = outcome.with_file(path.clone(), curves_csv(&gem));
    }
    Ok(outcome)
}
