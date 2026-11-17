//! The shared wording of the Material Reference.
//!
//! It holds the family line, the Dispersion chip text, the optic-character and number texts of
//! the table, and the window's own phrases. The desktop, web and mobile front ends and the CLI
//! call these, so one material reads the same everywhere.

use pleochroma::optics::materials::{GemMaterial, OpticalCharacter};
use pleochroma_cut_core::material::{SgBasis, built_in_specific_gravity, crystal_system_name};

use super::{
    facts::derived_note,
    rows::{Column, Filter, Origin},
};

/// What a table cell shows for a value that is not recorded.
pub const DASH: &str = "\u{2014}";

/// The placeholder of the table's search field.
pub const SEARCH_PLACEHOLDER: &str = "Search by name, system or cleavage";

/// The tooltip of the Dispersion column toggle.
pub const DISPERSION_TOGGLE_TOOLTIP: &str = "B\u{2013}G: the figure gem references print, n at 430.8 nm minus n at 686.7 nm. F\u{2013}C: the optical figure, 486.1 to 656.3 nm, which the Material Editor uses. Both come from this material's index curve.";

/// The crystal family of a material: "Cubic · isotropic · built-in" (system, optical character,
/// and "custom" for a material of the user's own, "built-in" for every other origin).
#[must_use]
pub fn family_line(material: &GemMaterial, origin: Origin) -> String {
    let character = match material.optical_character {
        OpticalCharacter::Isotropic => "isotropic",
        OpticalCharacter::UniaxialPositive => "uniaxial (+)",
        OpticalCharacter::UniaxialNegative => "uniaxial (-)",
        OpticalCharacter::BiaxialPositive => "biaxial (+)",
        OpticalCharacter::BiaxialNegative => "biaxial (-)",
    };
    let source = if origin == Origin::Custom {
        "custom"
    } else {
        "built-in"
    };
    format!(
        "{} \u{b7} {character} \u{b7} {source}",
        crystal_system_name(material.crystal_system)
    )
}

/// The Dispersion chip's figure: the B-G dispersion of the material's base curve to three
/// decimals ("0.044" for Diamond). The chip's own label and tooltip are the front end's.
#[must_use]
pub fn dispersion_chip_text(material: &GemMaterial) -> String {
    format!("{:.3}", material.dispersion.delta_b_g())
}

/// The F-C dispersion of the material's base curve to three decimals, the figure the Material
/// Editor works in.
#[must_use]
pub fn dispersion_fc_text(material: &GemMaterial) -> String {
    format!("{:.3}", material.dispersion.delta_f_c())
}

/// The optic character as the table prints it: "Isotropic", "Uniaxial (+)", "Biaxial (−)".
#[must_use]
pub const fn optic_character_text(character: OpticalCharacter) -> &'static str {
    match character {
        OpticalCharacter::Isotropic => "Isotropic",
        OpticalCharacter::UniaxialPositive => "Uniaxial (+)",
        OpticalCharacter::UniaxialNegative => "Uniaxial (\u{2212})",
        OpticalCharacter::BiaxialPositive => "Biaxial (+)",
        OpticalCharacter::BiaxialNegative => "Biaxial (\u{2212})",
    }
}

/// The tooltip of the Dispersion chip: the B-G convention, and the F-C figure beside it.
#[must_use]
pub fn dispersion_chip_tooltip(material: &GemMaterial) -> String {
    format!(
        "Dispersion (B\u{2013}G): how much the refractive index differs between violet (G line, 430.8 nm) and red (B line, 686.7 nm) light. It makes the colored flashes (fire). The Material Editor's figure, F\u{2013}C, is {}. See the glossary.",
        dispersion_fc_text(material)
    )
}

/// The basis note of a specific gravity computed from cited figures.
///
/// For example "Derived: midpoint of the cited range 2.80-2.91." `None` when the built-in's
/// figure is cited as printed, and for a name with no built-in specific gravity.
#[must_use]
pub fn sg_basis_note(material_name: &str) -> Option<String> {
    match built_in_specific_gravity(material_name)?.basis {
        SgBasis::Cited => None,
        SgBasis::Derived(note) => Some(derived_note(note)),
    }
}

/// The table's column heading.
#[must_use]
pub const fn column_label(column: Column) -> &'static str {
    match column {
        Column::Name => "Material",
        Column::CrystalSystem => "Crystal System",
        Column::OpticCharacter => "Optic Character",
        Column::Ri => "RI",
        Column::Birefringence => "Biref.",
        Column::Dispersion => "Disp.",
        Column::SpecificGravity => "SG",
        Column::Hardness => "Hardness",
        Column::Toughness => "Toughness",
        Column::Cleavage => "Cleavage",
    }
}

/// The filter chip's label.
#[must_use]
pub const fn filter_label(filter: Filter) -> &'static str {
    match filter {
        Filter::All => "All",
        Filter::Natural => "Natural",
        Filter::Synthetic => "Synthetic",
        Filter::Glass => "Glass",
        Filter::Custom => "My Materials",
    }
}

/// The window subtitle.
///
/// "{n} built-in materials and {m} of your own. Click a column heading to sort; open a row for
/// its curves, spectra and colors." With no custom material the middle part is left out.
#[must_use]
pub fn subtitle(built_in: usize, custom: usize) -> String {
    if custom == 0 {
        format!(
            "{built_in} built-in materials. Click a column heading to sort; open a row for its curves, spectra and colors."
        )
    } else {
        format!(
            "{built_in} built-in materials and {custom} of your own. Click a column heading to sort; open a row for its curves, spectra and colors."
        )
    }
}

/// The phrase after the count in the footer: "hardest first", "by name", "highest RI first".
#[must_use]
pub const fn sort_phrase(column: Column, descending: bool) -> &'static str {
    match (column, descending) {
        (Column::Name, false) => "by name",
        (Column::Name, true) => "by name, reversed",
        (Column::CrystalSystem, false) => "by crystal system",
        (Column::CrystalSystem, true) => "by crystal system, reversed",
        (Column::OpticCharacter, false) => "by optic character",
        (Column::OpticCharacter, true) => "by optic character, reversed",
        (Column::Ri, false) => "lowest RI first",
        (Column::Ri, true) => "highest RI first",
        (Column::Birefringence, false) => "lowest birefringence first",
        (Column::Birefringence, true) => "highest birefringence first",
        (Column::Dispersion, false) => "lowest dispersion first",
        (Column::Dispersion, true) => "highest dispersion first",
        (Column::SpecificGravity, false) => "lightest first",
        (Column::SpecificGravity, true) => "heaviest first",
        (Column::Hardness, false) => "softest first",
        (Column::Hardness, true) => "hardest first",
        (Column::Toughness, false) => "least tough first",
        (Column::Toughness, true) => "toughest first",
        (Column::Cleavage, false) => "weakest cleavage first",
        (Column::Cleavage, true) => "strongest cleavage first",
    }
}

/// The footer status line of the table.
#[must_use]
pub fn footer_text(shown: usize, column: Column, descending: bool) -> String {
    format!(
        "{shown} materials, {}. Values are typical for each species; a dash means not recorded.",
        sort_phrase(column, descending)
    )
}

/// The toast after Copy Table.
#[must_use]
pub fn copied_text(count: usize) -> String {
    format!("Copied {count} materials as a table.")
}
