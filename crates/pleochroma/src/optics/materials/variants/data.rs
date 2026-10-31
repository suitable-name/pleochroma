//! The built-in color variant data file (`data/color_variants.toml`), embedded and parsed once.
//!
//! The schema is documented at the top of the data file. Parsing validates every entry against
//! the built-in material it names, so a data error is a build-time failure caught by the unit
//! tests, exactly like `data/chromophores.toml`.

use std::{collections::BTreeSet, sync::OnceLock};

use serde::Deserialize;

use super::{
    ColorVariant, VariantProvenance, VariantTensorRows, material_slug, validate::VariantProblem,
};
use crate::optics::materials::{AbsorptionUnit, GemMaterial};

/// The data schema version this code reads.
pub const COLOR_VARIANTS_DATA_VERSION: u32 = 1;

/// The key suffix reserved for a material's own variant (`<slug>.default`).
pub(super) const DEFAULT_KEY_SUFFIX: &str = "default";

/// Parsed entries of the embedded data file, in file order.
static ENTRIES: OnceLock<Vec<ColorVariant>> = OnceLock::new();

/// The unit of an entry as the data file spells it.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EntryUnit {
    PerMm,
    ModelUnit,
}

impl EntryUnit {
    const fn to_unit(self) -> AbsorptionUnit {
        match self {
            Self::PerMm => AbsorptionUnit::PerMm,
            Self::ModelUnit => AbsorptionUnit::ModelUnit,
        }
    }
}

/// One `[[variant]]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    material: String,
    key: String,
    label: String,
    unit: EntryUnit,
    reference_path_mm: f32,
    estimated: bool,
    sources: Vec<String>,
    #[serde(default)]
    fit_record: Option<String>,
    absorption: VariantTensorRows,
}

/// The whole file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    data_version: u32,
    #[serde(default, rename = "variant")]
    variants: Vec<Entry>,
}

/// The embedded data files, in listing order: the main file (schema notes) and one file per
/// material group, so each group's entries are written and reviewed on their own. Every file
/// has the same schema; keys are unique across all of them.
const DATA_FILES: [(&str, &str); 6] = [
    (
        "color_variants.toml",
        include_str!("../../../../data/color_variants.toml"),
    ),
    (
        "color_variants/corundum_spinel_chrysoberyl.toml",
        include_str!("../../../../data/color_variants/corundum_spinel_chrysoberyl.toml"),
    ),
    (
        "color_variants/beryl_topaz_tourmaline_andalusite.toml",
        include_str!("../../../../data/color_variants/beryl_topaz_tourmaline_andalusite.toml"),
    ),
    (
        "color_variants/garnet_peridot_zircon_benitoite_tanzanite.toml",
        include_str!(
            "../../../../data/color_variants/garnet_peridot_zircon_benitoite_tanzanite.toml"
        ),
    ),
    (
        "color_variants/quartz_diamond_opal_synthetics_glass.toml",
        include_str!("../../../../data/color_variants/quartz_diamond_opal_synthetics_glass.toml"),
    ),
    (
        "color_variants/diamond.toml",
        include_str!("../../../../data/color_variants/diamond.toml"),
    ),
];

/// The data files' entries in listing order (each file in file order), parsed once.
///
/// # Panics
///
/// Panics if an embedded data file fails to parse or validate, or two files share a key (a
/// build-time data error caught by the unit tests).
pub(super) fn file_entries() -> &'static [ColorVariant] {
    ENTRIES.get_or_init(|| {
        parse_all(&DATA_FILES).expect("canonical color variant data must parse and validate")
    })
}

/// [`parse`] over several files, in order, with keys unique across all of them.
///
/// # Errors
///
/// A message naming the file and the offending entry.
pub(super) fn parse_all(files: &[(&str, &str)]) -> Result<Vec<ColorVariant>, String> {
    let mut keys = BTreeSet::new();
    let mut out = Vec::new();
    for (name, src) in files {
        for variant in parse(src).map_err(|e| format!("{name}: {e}"))? {
            if !keys.insert(variant.key.clone()) {
                return Err(format!("{name}: {}: duplicate key", variant.key));
            }
            out.push(variant);
        }
    }
    Ok(out)
}

/// Parses and validates variant data from TOML source text.
///
/// Validation: the data version, a built-in material of exactly that name for every entry, the
/// key `<slug>.<name>` with `<slug>` the material's slug and `<name>` neither empty nor the
/// reserved `default`, unique keys, a nonempty label and at least one source, and
/// [`ColorVariant::validate_for`] against the material.
///
/// # Errors
///
/// A message naming the offending entry.
pub(super) fn parse(src: &str) -> Result<Vec<ColorVariant>, String> {
    let file: File = toml::from_str(src).map_err(|e| format!("TOML parse error: {e}"))?;
    if file.data_version != COLOR_VARIANTS_DATA_VERSION {
        return Err(format!(
            "color variant data_version {} is not the supported {COLOR_VARIANTS_DATA_VERSION}",
            file.data_version
        ));
    }
    let table = GemMaterial::built_in();
    let mut keys = BTreeSet::new();
    let mut out = Vec::with_capacity(file.variants.len());
    for entry in file.variants {
        let host = table
            .iter()
            .find(|m| m.name == entry.material)
            .ok_or_else(|| format!("{}: no built-in material {:?}", entry.key, entry.material))?;
        let prefix = format!("{}.", material_slug(&host.name));
        let name = entry.key.strip_prefix(&prefix).unwrap_or_default();
        if name.is_empty() || name == DEFAULT_KEY_SUFFIX {
            return Err(format!(
                "{}: the key must be {prefix}<name>, with a name other than {DEFAULT_KEY_SUFFIX:?}",
                entry.key
            ));
        }
        if !keys.insert(entry.key.clone()) {
            return Err(format!("{}: duplicate key", entry.key));
        }
        if entry.label.trim().is_empty() || entry.sources.is_empty() {
            return Err(format!(
                "{}: needs a label and at least one source",
                entry.key
            ));
        }
        let variant = ColorVariant {
            key: entry.key,
            label: entry.label,
            material: host.name.clone(),
            absorption: entry.absorption.to_tensor(),
            unit: entry.unit.to_unit(),
            reference_path_mm: entry.reference_path_mm,
            provenance: VariantProvenance::BuiltIn {
                sources: entry.sources,
                fit_record: entry.fit_record,
                estimated: entry.estimated,
            },
        };
        variant
            .validate_for(host)
            .map_err(|p: VariantProblem| format!("{}: {p}", variant.key))?;
        out.push(variant);
    }
    Ok(out)
}
