//! The storage API of the color variants a cutter saved for a material ("My Variants").
//!
//! Every function takes the vault [`Database`] (callers hold the `Mutex` guard) and returns an
//! `anyhow::Result`; "nothing stored" is an empty collection or `false`, never an error, and a
//! stored row that cannot be used is an `Err` with a sentence the UI can show.
//!
//! # What is stored
//!
//! One `material_color_variants` row per variant: the key (`user.<uuid v4>`), the material name,
//! the label, the format version and `variant_json`, the JSON of the private `StoredVariant`:
//!
//! ```json
//! { "unit": "per_mm", "reference_path_mm": 7.0,
//!   "absorption": { "o_ray": [...], "e_ray": [...], "pleochroic": true } }
//! ```
//!
//! The tensor is [`VariantTensorRows`] (explicit band shape per row, exact round trip).
//! `reference_path_mm` is the STONE WIDTH in millimetres the numbers are calibrated for (never a
//! light path, like every `reference_path_mm` of the color variants): every swatch of a saved
//! variant is evaluated at `variants::face_up_swatch_path(unit, reference_path_mm)`. Built-in
//! variants live in the shipped data file and are never stored here; a variant read from the vault
//! always has [`VariantProvenance::User`].
//!
//! A label is unique per material (ASCII case ignored): saving or renaming to a label another
//! saved variant of the same material already has is refused with a sentence.
//!
//! # Function index
//!
//! | area | functions |
//! |---|---|
//! | read | [`user_color_variants`], [`load_user_color_variant`], [`decode_color_variant`] |
//! | write | [`save_user_color_variant`], [`rename_user_color_variant`], [`delete_user_color_variant`] |
//! | a material's variants | [`copy_color_variants`], [`remove_color_variants`] |

use anyhow::{Context, Result, anyhow, bail};
use pleochroma::optics::{
    absorption::AbsorptionTensor,
    materials::{
        AbsorptionUnit, GemMaterial,
        variants::{ColorVariant, MAX_VARIANT_BANDS, VariantProvenance, VariantTensorRows},
    },
};
use pleochroma_vault::{db::sqlite::Database, model::zoning::MaterialColorVariantRow};
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex, PoisonError, TryLockError},
};
use tracing::warn;

use super::crystal_optics::gem_material_from_row;
use crate::gui::{editor::fresh_design_uuid, rough_colour::material_zones::registered_vault};

/// The format version of `variant_json` this build writes and reads. A stored row with a newer
/// version is refused with a sentence ([`decode_color_variant`]).
pub const COLOR_VARIANT_FORMAT_VERSION: u32 = 1;

/// The key prefix of a saved variant (`user.<uuid v4>`).
pub const USER_KEY_PREFIX: &str = "user.";

/// The unit of a stored variant, as the JSON spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredUnit {
    /// [`AbsorptionUnit::PerMm`].
    PerMm,
    /// [`AbsorptionUnit::ModelUnit`].
    ModelUnit,
}

impl StoredUnit {
    /// The stored spelling of a unit.
    const fn of(unit: AbsorptionUnit) -> Self {
        match unit {
            AbsorptionUnit::PerMm => Self::PerMm,
            AbsorptionUnit::ModelUnit => Self::ModelUnit,
        }
    }

    /// The unit this spelling stands for.
    const fn unit(self) -> AbsorptionUnit {
        match self {
            Self::PerMm => AbsorptionUnit::PerMm,
            Self::ModelUnit => AbsorptionUnit::ModelUnit,
        }
    }
}

/// The JSON document of `variant_json` (format version 1).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredVariant {
    /// What the tensor's numbers are measured per.
    unit: StoredUnit,
    /// The stone width the tensor is calibrated for, millimetres (a width, never a light path).
    reference_path_mm: f32,
    /// The per-axis tensor as explicit band rows.
    absorption: VariantTensorRows,
}

/// A fresh key for a saved variant: `user.<uuid v4>` (the id is the design identity's
/// `fresh_design_uuid`: unique, not unguessable).
fn fresh_key() -> String {
    format!("{USER_KEY_PREFIX}{}", fresh_design_uuid())
}

/// The material named `name` (ASCII case ignored): a custom material of the vault first, else a
/// built-in. `None` when the name is unknown.
///
/// # Errors
///
/// The vault read fails.
fn host_material(db: &Database, name: &str) -> Result<Option<GemMaterial>> {
    let row = db
        .get_custom_materials()
        .context("The custom materials could not be read")?
        .into_iter()
        .find(|row| row.name.eq_ignore_ascii_case(name));
    if let Some(row) = row {
        return Ok(Some(gem_material_from_row(&row)));
    }
    Ok(GemMaterial::built_in()
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(name))
        .cloned())
}

/// Whether every number of the tensor and the reference path is finite (JSON cannot hold a
/// NaN or an infinity, so a variant with one is never stored, whether or not its material is
/// known).
fn all_finite(variant: &ColorVariant) -> bool {
    let tensor: &AbsorptionTensor = &variant.absorption;
    let sets = [
        Some(&tensor.o_ray),
        Some(&tensor.e_ray),
        tensor.beta_ray.as_ref(),
    ];
    variant.reference_path_mm.is_finite()
        && sets.into_iter().flatten().flatten().all(|band| {
            band.center_nm.is_finite() && band.width_nm.is_finite() && band.peak.is_finite()
        })
}

/// The `variant_json` text of a variant (format version [`COLOR_VARIANT_FORMAT_VERSION`]).
fn encode_variant(variant: &ColorVariant) -> Result<String> {
    let stored = StoredVariant {
        unit: StoredUnit::of(variant.unit),
        reference_path_mm: variant.reference_path_mm,
        absorption: VariantTensorRows::from_tensor(&variant.absorption),
    };
    serde_json::to_string(&stored).context("The color could not be written as text")
}

/// The variant a vault row holds, with [`VariantProvenance::User`].
///
/// # Errors
///
/// The sentence to show the cutter: the row was written by a newer version of the program, or its
/// text does not parse.
pub fn decode_color_variant(row: &MaterialColorVariantRow) -> Result<ColorVariant> {
    if row.version > COLOR_VARIANT_FORMAT_VERSION {
        bail!(
            "The color '{}' was saved by a newer version of the program and cannot be used here",
            row.label
        );
    }
    let stored: StoredVariant = serde_json::from_str(&row.variant_json)
        .with_context(|| format!("The saved color '{}' cannot be read", row.label))?;
    // The renderer's material buffer holds `MAX_VARIANT_BANDS` per ray and drops the rest, so a
    // longer (hand-edited) row would render differently on the CPU and the GPU.
    let rows = &stored.absorption;
    let longest = [Some(&rows.o_ray), Some(&rows.e_ray), rows.beta_ray.as_ref()]
        .into_iter()
        .flatten()
        .map(Vec::len)
        .max()
        .unwrap_or(0);
    if longest > MAX_VARIANT_BANDS {
        bail!(
            "The saved color '{}' has {longest} bands in one direction, at most {MAX_VARIANT_BANDS} are allowed",
            row.label
        );
    }
    Ok(ColorVariant {
        key: row.key.clone(),
        label: row.label.clone(),
        material: row.material_name.clone(),
        absorption: stored.absorption.to_tensor(),
        unit: stored.unit.unit(),
        reference_path_mm: stored.reference_path_mm,
        provenance: VariantProvenance::User,
    })
}

/// The color variants saved for `material_name` (ASCII case ignored), ordered by label.
///
/// A row that cannot be used (written by a newer version, or damaged) is left out and logged, so
/// one bad row does not hide the others; [`load_user_color_variant`] reports it with its sentence.
///
/// # Errors
///
/// The vault read fails.
pub fn user_color_variants(db: &Database, material_name: &str) -> Result<Vec<ColorVariant>> {
    let rows = db.load_color_variants_of_material(material_name)?;
    Ok(rows
        .iter()
        .filter_map(|row| match decode_color_variant(row) {
            Ok(variant) => Some(variant),
            Err(error) => {
                warn!("Color variant '{}' is not listed: {error:#}", row.key);
                None
            }
        })
        .collect())
}

/// The saved variant stored under `key`, or `None` when there is none.
///
/// # Errors
///
/// The vault read fails, or the row cannot be used ([`decode_color_variant`]).
pub fn load_user_color_variant(db: &Database, key: &str) -> Result<Option<ColorVariant>> {
    db.load_color_variant(key)?
        .as_ref()
        .map(decode_color_variant)
        .transpose()
}

/// Saves `variant` for its material and returns the key it is stored under.
///
/// An empty `variant.key` gets a new key `user.<uuid v4>`; a key that is already stored is
/// replaced (an edit). Its `provenance` is ignored: what the vault holds is always a user
/// variant. The vault keeps the label as given after trimming.
///
/// When the material is known (a custom material of the vault, else a built-in) the variant is
/// checked with [`ColorVariant::validate_for`] first, against the saved form (rank rule of a
/// user variant), and the problem is returned as the error sentence. A label that another saved
/// variant of the same material already has (ASCII case ignored) is refused; saving a variant
/// again under its own key and label is an edit and passes.
///
/// # Errors
///
/// The sentence to show the cutter: a blank label or material, a key that does not start with
/// `user.`, a number that is not finite, a `VariantProblem` for the material, a label the
/// material already lists, or a vault failure.
pub fn save_user_color_variant(db: &Database, variant: &ColorVariant) -> Result<String> {
    let label = variant.label.trim();
    if label.is_empty() {
        bail!("Give the color a name before saving it");
    }
    let material = variant.material.trim();
    if material.is_empty() {
        bail!("A color needs a material to belong to");
    }
    if !variant.key.is_empty() && !variant.key.starts_with(USER_KEY_PREFIX) {
        bail!(
            "'{}' is not the key of a saved color (it must start with '{USER_KEY_PREFIX}')",
            variant.key
        );
    }
    if !all_finite(variant) {
        bail!("The color '{label}' has a value that is not a number");
    }
    let saved = ColorVariant {
        provenance: VariantProvenance::User,
        ..variant.clone()
    };
    if let Some(host) = host_material(db, material)? {
        saved
            .validate_for(&host)
            .map_err(|problem| anyhow!("The color '{label}' does not fit {material}: {problem}"))?;
    }
    let key = if variant.key.is_empty() {
        fresh_key()
    } else {
        variant.key.clone()
    };
    refuse_duplicate_label(db, material, label, &key)?;
    let written = db
        .save_color_variant(&MaterialColorVariantRow {
            key: key.clone(),
            material_name: material.to_string(),
            label: label.to_string(),
            version: COLOR_VARIANT_FORMAT_VERSION,
            variant_json: encode_variant(&saved)?,
            created_utc: String::new(),
            updated_utc: String::new(),
        })
        .with_context(|| format!("The color '{label}' could not be saved"));
    forget_last_read(material);
    written?;
    Ok(key)
}

/// Refuses `label` when another saved variant of `material` (any key but `own_key`) already has
/// it, ASCII case ignored: two rows of one list must not read the same.
fn refuse_duplicate_label(db: &Database, material: &str, label: &str, own_key: &str) -> Result<()> {
    let taken = db
        .load_color_variants_of_material(material)?
        .iter()
        .any(|row| row.key != own_key && row.label.trim().eq_ignore_ascii_case(label));
    if taken {
        bail!("A color named '{label}' already exists for {material}; choose another name");
    }
    Ok(())
}

/// Renames the saved variant `key` to `label` (trimmed).
///
/// # Errors
///
/// The sentence to show the cutter: a blank label, no saved variant of that key, a label another
/// saved variant of the same material already has, or a vault failure.
pub fn rename_user_color_variant(db: &Database, key: &str, label: &str) -> Result<()> {
    let label = label.trim();
    if label.is_empty() {
        bail!("Give the color a name");
    }
    let Some(row) = db.load_color_variant(key)? else {
        bail!("The saved color '{key}' no longer exists");
    };
    refuse_duplicate_label(db, &row.material_name, label, key)?;
    let renamed = db.rename_color_variant_label(key, label);
    forget_last_read(&row.material_name);
    if renamed? == 0 {
        bail!("The saved color '{key}' no longer exists");
    }
    Ok(())
}

/// Deletes the saved variant `key`. Returns whether a variant was removed (`false`: there was
/// none).
///
/// A design that picked the variant keeps its own snapshot of the tensor and is not affected.
///
/// # Errors
///
/// The vault write fails.
pub fn delete_user_color_variant(db: &Database, key: &str) -> Result<bool> {
    let material = db.load_color_variant(key)?.map(|row| row.material_name);
    let removed = db.delete_color_variant(key);
    if let Some(material) = material {
        forget_last_read(&material);
    }
    Ok(removed? > 0)
}

/// Copies the variants of `from` to `to` with new keys (a custom material saved under a new
/// name), unless `to` has variants of its own or `from` has none. Returns how many variants were
/// copied. The variants of `from` stay: the save does not delete the material it was copied from.
///
/// The copy is all or nothing: when a write fails, the copies made so far are deleted again (the
/// vault handle offers no transaction), so `to` never keeps a partial set and the next save of
/// the material can copy again.
///
/// # Errors
///
/// The vault read or write fails.
pub fn copy_color_variants(db: &Database, from: &str, to: &str) -> Result<usize> {
    if from.eq_ignore_ascii_case(to) || !db.load_color_variants_of_material(to)?.is_empty() {
        return Ok(0);
    }
    let rows = db.load_color_variants_of_material(from)?;
    let copied = copy_rows(db, &rows, to, |row| db.save_color_variant(row));
    forget_last_read(to);
    copied
}

/// Writes `rows` as variants of `to` with new keys through `save`; when one write fails, the keys
/// already written are deleted again and the failure is returned.
fn copy_rows(
    db: &Database,
    rows: &[MaterialColorVariantRow],
    to: &str,
    mut save: impl FnMut(&MaterialColorVariantRow) -> Result<()>,
) -> Result<usize> {
    let mut written: Vec<String> = Vec::with_capacity(rows.len());
    for row in rows {
        let copy = MaterialColorVariantRow {
            key: fresh_key(),
            material_name: to.to_string(),
            ..row.clone()
        };
        if let Err(error) = save(&copy) {
            for key in written.iter().chain(std::iter::once(&copy.key)) {
                if let Err(undo) = db.delete_color_variant(key) {
                    warn!("A partly copied color '{key}' could not be removed: {undo:#}");
                }
            }
            return Err(error.context(format!(
                "The saved colors could not be copied to {to}; nothing was copied"
            )));
        }
        written.push(copy.key);
    }
    Ok(rows.len())
}

/// Removes every saved variant of `material_name` (a deleted custom material). Returns how many
/// were removed; nothing stored is a no-op that succeeds.
///
/// # Errors
///
/// The vault write fails.
pub fn remove_color_variants(db: &Database, material_name: &str) -> Result<usize> {
    let removed = db.delete_color_variants_of_material(material_name);
    forget_last_read(material_name);
    removed
}

/// The saved variants last read per material (lowercase name), so a refresh that finds the vault
/// busy keeps showing them. Process-wide, so a write on any thread invalidates it.
static LAST_READ: LazyLock<Mutex<HashMap<String, Vec<ColorVariant>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The `LAST_READ` key of a material name.
fn last_read_key(material_name: &str) -> String {
    material_name.trim().to_ascii_lowercase()
}

/// Remembers `variants` as the list last read for `material_name`.
fn remember_last_read(material_name: &str, variants: &[ColorVariant]) {
    LAST_READ
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(last_read_key(material_name), variants.to_vec());
}

/// The list last read for `material_name`, if it was not invalidated since.
fn recall_last_read(material_name: &str) -> Option<Vec<ColorVariant>> {
    LAST_READ
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&last_read_key(material_name))
        .cloned()
}

/// Drops the remembered list of `material_name`. Every write to the saved variants of a material
/// calls this, so a refresh that finds the vault busy afterwards shows an empty list rather than
/// a color that was just deleted or renamed.
fn forget_last_read(material_name: &str) {
    LAST_READ
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&last_read_key(material_name));
}

/// The saved variants of `material_name` as the UI thread lists them: read through the main
/// window's vault ([`registered_vault`]) without ever waiting for it. A vault that is busy (a
/// worker holds it) or unreadable answers with the list last read for that material, unless a
/// write to the material's saved variants happened since (then nothing is remembered and the list
/// is empty); before the main window registered a vault the list is empty.
#[must_use]
pub fn saved_variants(material_name: &str) -> Vec<ColorVariant> {
    let remembered = || recall_last_read(material_name).unwrap_or_default();
    let Some(vault) = registered_vault() else {
        return Vec::new();
    };
    let read = match vault.try_lock() {
        Ok(guard) => user_color_variants(&guard, material_name),
        Err(TryLockError::Poisoned(poisoned)) => {
            user_color_variants(&poisoned.into_inner(), material_name)
        }
        Err(TryLockError::WouldBlock) => return remembered(),
    };
    match read {
        Ok(variants) => {
            remember_last_read(material_name, &variants);
            variants
        }
        Err(error) => {
            warn!("Saved colors of '{material_name}' could not be read: {error:#}");
            remembered()
        }
    }
}

/// The note of an estimated built-in variant: `"Estimated: <first source line>"`, so a tooltip or
/// list names where the numbers come from and says they are not a fit to a measured spectrum.
/// `None` for a measured built-in, a material's own color and a saved variant. (The combo's
/// tooltip builds the same text in `pleochroma_editor::material::color_entry`.)
#[must_use]
pub fn estimated_note(provenance: &VariantProvenance) -> Option<String> {
    let VariantProvenance::BuiltIn {
        sources,
        estimated: true,
        ..
    } = provenance
    else {
        return None;
    };
    let first = sources
        .iter()
        .flat_map(|source| source.lines())
        .map(str::trim)
        .find(|line| !line.is_empty());
    Some(first.map_or_else(
        || "Estimated: no published spectrum.".to_owned(),
        |line| format!("Estimated: {line}"),
    ))
}

/// `base` made unique among `taken` labels (ASCII case ignored): `base`, then `base 2`, `base 3`.
#[must_use]
pub fn unique_label(base: &str, taken: &[String]) -> String {
    let base = base.trim();
    let is_taken = |label: &str| taken.iter().any(|t| t.trim().eq_ignore_ascii_case(label));
    if !is_taken(base) {
        return base.to_owned();
    }
    // `taken.len() + 1` numbered candidates cannot all be taken: the search is bounded.
    (2..=taken.len() + 2)
        .map(|n| format!("{base} {n}"))
        .find(|label| !is_taken(label))
        .unwrap_or_else(|| base.to_owned())
}

/// The name the "Save as Variant" field starts with: the coarse color family of the first
/// direction (`"Blue"`), made unique among the labels the material already lists.
#[must_use]
pub fn suggested_label(families: &[&str], taken: &[String]) -> String {
    let base = families.first().copied().unwrap_or("Custom");
    unique_label(base, taken)
}

// No startup prune of saved variants (lead decision 2026-10-27): a variant whose material is
// unknown is kept, not deleted, so a built-in renamed or removed in a later version never takes
// a cutter's saved colors with it. A deleted custom material removes its variants explicitly
// (`remove_color_variants`, called by `apply_custom_material_delete`).

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::optics::absorption::AbsorptionBand;
    use pleochroma_vault::db::sqlite::CustomMaterialParams;

    fn db() -> Database {
        Database::new(Some(":memory:")).expect("in-memory vault")
    }

    #[test]
    fn a_label_is_made_unique_among_the_taken_ones() {
        let taken = vec!["Blue".to_owned(), "blue 2".to_owned()];
        assert_eq!(unique_label("Pink", &taken), "Pink");
        assert_eq!(unique_label("Blue", &taken), "Blue 3");
        assert_eq!(unique_label("  Blue ", &[]), "Blue");
    }

    #[test]
    fn the_suggested_name_is_the_first_direction_family_or_custom() {
        assert_eq!(suggested_label(&["Green", "Blue"], &[]), "Green");
        assert_eq!(suggested_label(&[], &[]), "Custom");
        assert_eq!(
            suggested_label(&["Green"], &["Green".to_owned()]),
            "Green 2"
        );
    }

    #[test]
    fn without_a_registered_vault_no_saved_variants_are_listed() {
        assert!(saved_variants("Sapphire").is_empty());
    }

    fn add_custom_material(db: &Database, name: &str) {
        db.save_custom_material(&CustomMaterialParams {
            name,
            refractive_index: 1.55,
            dispersion: 0.01,
            birefringence: 0.0,
            absorption_rgb: [0.0; 3],
            crystal_system: None,
            optical_character: None,
            biaxial_delta_beta_alpha: None,
            per_axis_dispersion_json: None,
            specific_gravity: None,
            color_recipe_json: None,
            dispersion_model_json: None,
            absorption_bands_json: None,
            c_axis_tilt_deg: None,
            c_axis_azimuth_deg: None,
            c_axis_second_azimuth_deg: None,
            inclusion_families_json: None,
            structure_json: None,
        })
        .expect("material saved");
    }

    /// A uniaxial tensor with awkward float values (a wavelength band, an energy band).
    fn uniaxial_tensor() -> AbsorptionTensor {
        AbsorptionTensor::uniaxial(
            vec![
                AbsorptionBand::new(570.123, 41.7, 0.034_567),
                AbsorptionBand::energy(451.1, 1500.5, 0.000_123),
            ],
            vec![AbsorptionBand::new(430.0, 38.3333, 0.12)],
        )
    }

    /// A biaxial tensor with three different band sets.
    fn biaxial_tensor() -> AbsorptionTensor {
        AbsorptionTensor::biaxial(
            vec![AbsorptionBand::new(600.7, 55.55, 0.31)],
            vec![
                AbsorptionBand::energy(520.3, 2100.1, 0.077),
                AbsorptionBand::new(380.0, 25.0, 1.0e-7),
            ],
            vec![AbsorptionBand::new(455.5, 30.3, 0.6)],
        )
    }

    fn variant_of(
        material: &str,
        label: &str,
        absorption: AbsorptionTensor,
        unit: AbsorptionUnit,
    ) -> ColorVariant {
        ColorVariant {
            key: String::new(),
            label: label.to_string(),
            material: material.to_string(),
            absorption,
            unit,
            reference_path_mm: 7.25,
            provenance: VariantProvenance::User,
        }
    }

    /// Every number of two tensors is the same bit pattern.
    fn assert_bit_exact(a: &AbsorptionTensor, b: &AbsorptionTensor) {
        assert_eq!(a.is_pleochroic, b.is_pleochroic);
        assert_eq!(a.beta_ray.is_some(), b.beta_ray.is_some());
        let sets = |t: &AbsorptionTensor| {
            let mut sets = vec![t.o_ray.clone(), t.e_ray.clone()];
            sets.extend(t.beta_ray.clone());
            sets
        };
        let (left, right) = (sets(a), sets(b));
        assert_eq!(left.len(), right.len());
        for (x, y) in left.iter().zip(&right) {
            assert_eq!(x.len(), y.len());
            for (p, q) in x.iter().zip(y) {
                assert_eq!(p.center_nm.to_bits(), q.center_nm.to_bits());
                assert_eq!(p.width_nm.to_bits(), q.width_nm.to_bits());
                assert_eq!(p.peak.to_bits(), q.peak.to_bits());
                assert_eq!(p.shape, q.shape);
            }
        }
    }

    #[test]
    fn a_uniaxial_and_a_biaxial_tensor_round_trip_through_the_json_bit_exact() {
        let db = db();
        for (material, tensor) in [
            ("Sapphire", uniaxial_tensor()),
            ("Tanzanite", biaxial_tensor()),
        ] {
            let variant = variant_of(material, "Round Trip", tensor, AbsorptionUnit::PerMm);
            let key = save_user_color_variant(&db, &variant).unwrap();
            let loaded = load_user_color_variant(&db, &key).unwrap().unwrap();
            assert_bit_exact(&variant.absorption, &loaded.absorption);
            assert_eq!(loaded.absorption, variant.absorption);
            assert_eq!(loaded.unit, AbsorptionUnit::PerMm);
            assert_eq!(
                loaded.reference_path_mm.to_bits(),
                variant.reference_path_mm.to_bits()
            );
            assert_eq!(loaded.provenance, VariantProvenance::User);
            assert_eq!(loaded.material, material);
        }
    }

    #[test]
    fn the_json_spells_the_unit_and_the_band_shape_explicitly() {
        let variant = variant_of(
            "Sapphire",
            "Spelled",
            uniaxial_tensor(),
            AbsorptionUnit::ModelUnit,
        );
        let text = encode_variant(&variant).unwrap();
        assert!(text.contains("\"unit\":\"model_unit\""), "{text}");
        assert!(text.contains("\"shape\":\"energy\""), "{text}");
        assert!(text.contains("\"shape\":\"wavelength\""), "{text}");
        assert!(text.contains("\"reference_path_mm\":7.25"), "{text}");
    }

    #[test]
    fn an_empty_key_gets_a_new_user_key_and_a_known_key_is_updated() {
        let db = db();
        let mut variant = variant_of(
            "Sapphire",
            "First",
            uniaxial_tensor(),
            AbsorptionUnit::PerMm,
        );
        let first = save_user_color_variant(&db, &variant).unwrap();
        variant.label = "Second".to_string();
        let second = save_user_color_variant(&db, &variant).unwrap();
        variant.label = "First".to_string();
        assert!(first.starts_with("user."));
        assert_eq!(first.len(), "user.".len() + 36);
        assert_ne!(first, second, "two saves of an empty key make two variants");
        assert_eq!(user_color_variants(&db, "sapphire").unwrap().len(), 2);

        variant.key = first.clone();
        variant.label = "  Edited  ".to_string();
        assert_eq!(save_user_color_variant(&db, &variant).unwrap(), first);
        let loaded = load_user_color_variant(&db, &first).unwrap().unwrap();
        assert_eq!(loaded.label, "Edited");
        assert_eq!(user_color_variants(&db, "Sapphire").unwrap().len(), 2);
    }

    #[test]
    fn a_label_another_saved_variant_of_the_material_has_is_refused() {
        let db = db();
        let blue = variant_of("Sapphire", "Blue", uniaxial_tensor(), AbsorptionUnit::PerMm);
        let key = save_user_color_variant(&db, &blue).unwrap();

        // The same label again, in any case and with stray spaces, is a second row: refused.
        for label in ["Blue", "blue", "  BLUE "] {
            let again = variant_of("Sapphire", label, uniaxial_tensor(), AbsorptionUnit::PerMm);
            let error = save_user_color_variant(&db, &again)
                .unwrap_err()
                .to_string();
            assert!(error.contains("already exists for Sapphire"), "{error}");
        }
        assert_eq!(user_color_variants(&db, "Sapphire").unwrap().len(), 1);

        // Saving the variant again under its own key and label is an edit, and passes.
        let edit = ColorVariant {
            key: key.clone(),
            ..blue
        };
        assert_eq!(save_user_color_variant(&db, &edit).unwrap(), key);

        // Another material may use the label, and a rename onto a taken label is refused.
        let ruby = variant_of("Ruby", "Blue", uniaxial_tensor(), AbsorptionUnit::PerMm);
        assert!(save_user_color_variant(&db, &ruby).is_ok());
        let pink = variant_of("Sapphire", "Pink", uniaxial_tensor(), AbsorptionUnit::PerMm);
        let pink_key = save_user_color_variant(&db, &pink).unwrap();
        let error = rename_user_color_variant(&db, &pink_key, "blue")
            .unwrap_err()
            .to_string();
        assert!(error.contains("already exists for Sapphire"), "{error}");
        assert!(rename_user_color_variant(&db, &pink_key, "Pink").is_ok());
        assert!(rename_user_color_variant(&db, &pink_key, "Rose").is_ok());
    }

    #[test]
    fn a_colorless_material_default_can_be_duplicated_as_a_saved_variant() {
        // Quartz's own tensor is isotropic and empty on a uniaxial host: it has no pleochroism to
        // lose, so the copy fits.
        let db = db();
        let colorless = AbsorptionTensor::isotropic(vec![]);
        let copy = variant_of(
            "Quartz",
            "Colorless Copy",
            colorless,
            AbsorptionUnit::ModelUnit,
        );
        let key = save_user_color_variant(&db, &copy).unwrap();
        let loaded = load_user_color_variant(&db, &key).unwrap().unwrap();
        assert_eq!(loaded.absorption, copy.absorption);
    }

    #[test]
    fn a_failed_copy_leaves_no_partial_set_behind() {
        let db = db();
        add_custom_material(&db, "Melon");
        let flat = AbsorptionTensor::isotropic(vec![AbsorptionBand::new(500.0, 40.0, 0.2)]);
        for label in ["Green", "Yellow", "Red"] {
            let variant = variant_of("Melon", label, flat.clone(), AbsorptionUnit::PerMm);
            save_user_color_variant(&db, &variant).unwrap();
        }
        let rows = db.load_color_variants_of_material("Melon").unwrap();
        assert_eq!(rows.len(), 3);

        // The third write fails: the two copies already made are removed again.
        let mut calls = 0;
        let error = copy_rows(&db, &rows, "Melon Copy", |row| {
            calls += 1;
            if calls == 3 {
                bail!("the vault is full");
            }
            db.save_color_variant(row)
        })
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("nothing was copied"),
            "{error:#}"
        );
        assert!(user_color_variants(&db, "Melon Copy").unwrap().is_empty());
        // A later copy is not skipped as "already has variants".
        assert_eq!(copy_color_variants(&db, "Melon", "Melon Copy").unwrap(), 3);
    }

    #[test]
    fn a_write_forgets_the_remembered_list_of_its_material() {
        let db = db();
        let material = "Invalidation Test Stone";
        let variant = variant_of(material, "Gone", uniaxial_tensor(), AbsorptionUnit::PerMm);
        remember_last_read(material, std::slice::from_ref(&variant));
        assert!(recall_last_read(&material.to_uppercase()).is_some());
        // A save, a rename and a delete each drop it (a refresh that finds the vault busy must
        // not bring a deleted color back).
        let key = save_user_color_variant(&db, &variant).unwrap();
        assert_eq!(recall_last_read(material), None);
        remember_last_read(material, std::slice::from_ref(&variant));
        rename_user_color_variant(&db, &key, "Renamed").unwrap();
        assert_eq!(recall_last_read(material), None);
        remember_last_read(material, std::slice::from_ref(&variant));
        assert!(delete_user_color_variant(&db, &key).unwrap());
        assert_eq!(recall_last_read(material), None);
        remember_last_read(material, std::slice::from_ref(&variant));
        remove_color_variants(&db, material).unwrap();
        assert_eq!(recall_last_read(material), None);
    }

    #[test]
    fn a_variant_of_another_material_is_not_listed() {
        let db = db();
        let variant = variant_of("Sapphire", "Mine", uniaxial_tensor(), AbsorptionUnit::PerMm);
        save_user_color_variant(&db, &variant).unwrap();
        assert!(user_color_variants(&db, "Ruby").unwrap().is_empty());
        assert_eq!(load_user_color_variant(&db, "user.missing").unwrap(), None);
    }

    #[test]
    fn rename_and_delete_work_by_key() {
        let db = db();
        let variant = variant_of("Sapphire", "Old", uniaxial_tensor(), AbsorptionUnit::PerMm);
        let key = save_user_color_variant(&db, &variant).unwrap();
        rename_user_color_variant(&db, &key, " New ").unwrap();
        assert_eq!(
            load_user_color_variant(&db, &key).unwrap().unwrap().label,
            "New"
        );
        assert!(rename_user_color_variant(&db, &key, "   ").is_err());
        assert!(rename_user_color_variant(&db, "user.missing", "X").is_err());

        assert!(delete_user_color_variant(&db, &key).unwrap());
        assert!(!delete_user_color_variant(&db, &key).unwrap());
        assert!(user_color_variants(&db, "Sapphire").unwrap().is_empty());
    }

    #[test]
    fn a_variant_that_does_not_fit_its_material_is_refused_with_the_sentence() {
        let db = db();
        // An isotropic color on a uniaxial built-in host.
        let isotropic = AbsorptionTensor::isotropic(vec![AbsorptionBand::new(500.0, 40.0, 0.2)]);
        let error = save_user_color_variant(
            &db,
            &variant_of("Sapphire", "Flat", isotropic.clone(), AbsorptionUnit::PerMm),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("does not fit Sapphire"), "{error}");
        assert!(error.contains("does not fit a"), "{error}");

        // An isotropic color on a biaxial built-in host.
        assert!(
            save_user_color_variant(
                &db,
                &variant_of(
                    "Tanzanite",
                    "Flat",
                    isotropic.clone(),
                    AbsorptionUnit::PerMm
                )
            )
            .is_err()
        );

        // A custom (isotropic) material takes an isotropic color only.
        add_custom_material(&db, "Melon");
        assert!(
            save_user_color_variant(
                &db,
                &variant_of("melon", "Flat", isotropic, AbsorptionUnit::PerMm)
            )
            .is_ok()
        );
        assert!(
            save_user_color_variant(
                &db,
                &variant_of("Melon", "Two", uniaxial_tensor(), AbsorptionUnit::PerMm)
            )
            .is_err()
        );
        assert_eq!(user_color_variants(&db, "Melon").unwrap().len(), 1);
    }

    #[test]
    fn a_blank_label_a_foreign_key_and_a_non_finite_number_are_refused() {
        let db = db();
        let ok = variant_of("Sapphire", "Fine", uniaxial_tensor(), AbsorptionUnit::PerMm);
        let mut blank = ok.clone();
        blank.label = "  ".to_string();
        assert!(save_user_color_variant(&db, &blank).is_err());

        let mut foreign = ok.clone();
        foreign.key = "sapphire.padparadscha".to_string();
        assert!(save_user_color_variant(&db, &foreign).is_err());

        let mut nan = ok;
        nan.absorption.o_ray[0].peak = f32::NAN;
        assert!(save_user_color_variant(&db, &nan).is_err());
        assert!(user_color_variants(&db, "Sapphire").unwrap().is_empty());
    }

    #[test]
    fn an_unknown_material_is_stored_without_a_rank_check() {
        let db = db();
        let variant = variant_of(
            "Not Yet Saved",
            "Early",
            uniaxial_tensor(),
            AbsorptionUnit::PerMm,
        );
        save_user_color_variant(&db, &variant).unwrap();
        assert_eq!(user_color_variants(&db, "not yet saved").unwrap().len(), 1);
    }

    #[test]
    fn a_newer_version_is_refused_with_a_sentence_and_left_out_of_the_list() {
        let db = db();
        let variant = variant_of(
            "Sapphire",
            "Future",
            uniaxial_tensor(),
            AbsorptionUnit::PerMm,
        );
        let key = save_user_color_variant(&db, &variant).unwrap();
        let mut row = db.load_color_variant(&key).unwrap().unwrap();
        row.version = COLOR_VARIANT_FORMAT_VERSION + 1;
        db.save_color_variant(&row).unwrap();

        let error = load_user_color_variant(&db, &key).unwrap_err().to_string();
        assert!(error.contains("newer version"), "{error}");
        assert!(error.contains("Future"), "{error}");
        assert!(user_color_variants(&db, "Sapphire").unwrap().is_empty());
        // The row is still in the vault: an older build never deletes what it cannot read.
        assert!(db.load_color_variant(&key).unwrap().is_some());
    }

    #[test]
    fn a_damaged_row_is_refused_with_a_sentence() {
        let db = db();
        db.save_color_variant(&MaterialColorVariantRow {
            key: "user.bad".to_string(),
            material_name: "Sapphire".to_string(),
            label: "Broken".to_string(),
            version: 1,
            variant_json: "{\"unit\":\"per_mm\"".to_string(),
            created_utc: String::new(),
            updated_utc: String::new(),
        })
        .unwrap();
        let error = load_user_color_variant(&db, "user.bad")
            .unwrap_err()
            .to_string();
        assert!(error.contains("Broken"), "{error}");
        assert!(user_color_variants(&db, "Sapphire").unwrap().is_empty());
    }

    #[test]
    fn saving_a_material_under_a_new_name_copies_its_variants_with_new_keys() {
        let db = db();
        add_custom_material(&db, "Melon");
        let flat = AbsorptionTensor::isotropic(vec![AbsorptionBand::new(500.0, 40.0, 0.2)]);
        let key = save_user_color_variant(
            &db,
            &variant_of("Melon", "Green", flat, AbsorptionUnit::PerMm),
        )
        .unwrap();

        assert_eq!(copy_color_variants(&db, "Melon", "Melon Copy").unwrap(), 1);
        let copied = user_color_variants(&db, "melon copy").unwrap();
        assert_eq!(copied.len(), 1);
        assert_eq!(copied[0].label, "Green");
        assert_ne!(copied[0].key, key);
        assert_eq!(copied[0].material, "Melon Copy");
        assert_eq!(
            copied[0].absorption,
            user_color_variants(&db, "Melon").unwrap()[0].absorption
        );
        assert_eq!(
            user_color_variants(&db, "Melon").unwrap().len(),
            1,
            "the source keeps its variants"
        );

        // Nothing is copied twice, onto itself, or from a material without variants.
        assert_eq!(copy_color_variants(&db, "Melon", "Melon Copy").unwrap(), 0);
        assert_eq!(copy_color_variants(&db, "MELON", "melon").unwrap(), 0);
        assert_eq!(copy_color_variants(&db, "Nothing", "Other").unwrap(), 0);
    }

    #[test]
    fn deleting_a_material_removes_its_variants() {
        let db = db();
        let variant = variant_of("Sapphire", "Mine", uniaxial_tensor(), AbsorptionUnit::PerMm);
        save_user_color_variant(&db, &variant).unwrap();
        save_user_color_variant(
            &db,
            &variant_of("Ruby", "Mine", uniaxial_tensor(), AbsorptionUnit::PerMm),
        )
        .unwrap();
        assert_eq!(remove_color_variants(&db, "SAPPHIRE").unwrap(), 1);
        assert_eq!(remove_color_variants(&db, "SAPPHIRE").unwrap(), 0);
        assert!(user_color_variants(&db, "Sapphire").unwrap().is_empty());
        assert_eq!(user_color_variants(&db, "Ruby").unwrap().len(), 1);
    }
}
