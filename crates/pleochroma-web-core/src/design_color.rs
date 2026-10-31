//! The browser's design color: the keyed color combo, the per-axis Custom editor and the stored
//! user color variants, as plain logic the page only wires to Slint and `IndexedDB`.
//!
//! The desktop's rules live in `pleochroma_editor` (`material::color_entry` for the combo,
//! `lch_color` for the per-axis rows and the solve); this module adapts them to what the page
//! holds: a design, the session's custom materials and the cutter's saved variants (read from
//! `IndexedDB` once and kept in memory). Nothing here touches the DOM, so all of it is tested
//! natively.
//!
//! # Parts
//!
//! - [`DesignColors`]: the combo's rows for the open design, the row that shows its color, the
//!   "Legacy Color: No Pleochroism" flag, and the rule that turns a picked row into a
//!   [`MaterialSelection`] ([`DesignColors::material_for_apply`]).
//! - [`AxisEditor`]: the Custom editor's state. One L*C*h row per direction of the host (one,
//!   two or three), each solved by the shared 7-band solver; the page runs the solves (in the
//!   analysis Worker) and feeds the answers back; [`AxisEditor::applied_to`] stores all rows as one
//!   edit, [`AxisEditor::variant_to_save`] builds the variant "Save as Variant..." keeps and
//!   [`applied_variant`] puts that saved variant on the design as one edit.
//! - [`ColorVariantRecord`]: a saved variant as the browser stores it: the same JSON as the
//!   desktop vault's `variant_json` (`{"unit","reference_path_mm","absorption"}`) beside the
//!   material name, label and key.
//!
//! A variant's `reference_path_mm` is the STONE WIDTH (mm) its numbers are calibrated for
//! ([`stone_width_mm`] is the width of the open design): the editor stores it with every Custom
//! color and a saved variant keeps it, while the editor's own solves run at the solver path
//! (girdle times 1.5).

use pleochroma::{
    optics::materials::{
        AbsorptionUnit, GemMaterial,
        variants::{ColorVariant, TensorRank, VariantProvenance, VariantTensorRows, host_rank},
    },
    render_setup::PHYSICS_DEFAULT_STONE_WIDTH_MM,
};
use pleochroma_cut_core::{Design, MaterialLookup, MaterialSelection, material::VariantChoice};
use pleochroma_editor::{
    lch_color::{
        LchSolve, axis_rows, badge_text, color_family, custom_choice, direction_swatches,
        lch_from_material, recoloured_with_choice, recoloured_with_solve, seed_axis_lch,
        swatch_labels, tensor_from_solves,
    },
    material::{
        ColorEntry, ColorEntryItem, color_entries_for_design, entry_for_selection,
        legacy_color_on_pleochroic_host, listed_variants, selection_with_entry,
        with_body_color_choice,
    },
    material_lookup::{EditorMaterialLookup, traced_material_for},
};
use serde::{Deserialize, Serialize};

use crate::design_meta::uuid_v4_from_bytes;

// ---------------------------------------------------------------------------------------
// Saved variants as stored records
// ---------------------------------------------------------------------------------------

/// The format version of a stored variant record (the vault's `COLOR_VARIANT_FORMAT_VERSION`).
pub const RECORD_VERSION: u32 = 1;

/// The prefix of a saved variant's key (`"user.<uuid>"`); built-in keys never start with it.
pub const USER_KEY_PREFIX: &str = "user.";

/// The unit as the stored JSON spells it (the vault's `StoredUnit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredUnit {
    PerMm,
    ModelUnit,
}

impl StoredUnit {
    /// The stored spelling of `unit`.
    const fn of(unit: AbsorptionUnit) -> Self {
        match unit {
            AbsorptionUnit::PerMm => Self::PerMm,
            AbsorptionUnit::ModelUnit => Self::ModelUnit,
        }
    }

    /// The renderer's unit.
    const fn unit(self) -> AbsorptionUnit {
        match self {
            Self::PerMm => AbsorptionUnit::PerMm,
            Self::ModelUnit => AbsorptionUnit::ModelUnit,
        }
    }
}

/// The JSON a record keeps in `variant_json`: exactly the desktop vault's shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredVariant {
    unit: StoredUnit,
    reference_path_mm: f32,
    absorption: VariantTensorRows,
}

/// One saved variant as the browser's `color_variants` store keeps it (the vault's
/// `material_color_variants` row without its timestamps, which the page adds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorVariantRecord {
    /// The variant's key (`"user.<uuid>"`), also the store key.
    pub key: String,
    /// The material the variant belongs to (exact built-in name, or the custom material's).
    pub material_name: String,
    /// The display label.
    pub label: String,
    /// [`RECORD_VERSION`] at write time.
    pub version: u32,
    /// The `{"unit","reference_path_mm","absorption"}` JSON.
    pub variant_json: String,
}

/// A fresh saved-variant key from 16 random bytes: `user.` and a version-4 UUID.
#[must_use]
pub fn user_key(bytes: [u8; 16]) -> String {
    format!("{USER_KEY_PREFIX}{}", uuid_v4_from_bytes(bytes))
}

/// Whether every number of `rows` is finite (JSON would write a NaN as `null`).
fn rows_are_finite(rows: &VariantTensorRows) -> bool {
    [Some(&rows.o_ray), Some(&rows.e_ray), rows.beta_ray.as_ref()]
        .into_iter()
        .flatten()
        .flatten()
        .all(|row| row.center_nm.is_finite() && row.width_nm.is_finite() && row.peak.is_finite())
}

/// The record that stores `variant`.
///
/// # Errors
///
/// A blank label or material, a key that is not a saved variant's (`user.` prefix), or a number
/// that is not finite -- as the sentence to show.
pub fn record_for(variant: &ColorVariant) -> Result<ColorVariantRecord, String> {
    let label = variant.label.trim();
    if label.is_empty() {
        return Err("A saved color needs a name.".to_owned());
    }
    let material = variant.material.trim();
    if material.is_empty() {
        return Err("A saved color belongs to one material: pick a material first.".to_owned());
    }
    if !variant.key.starts_with(USER_KEY_PREFIX) {
        return Err(format!(
            "The key '{}' is not a saved color's (it must start with '{USER_KEY_PREFIX}').",
            variant.key
        ));
    }
    let rows = VariantTensorRows::from_tensor(&variant.absorption);
    if !variant.reference_path_mm.is_finite() || !rows_are_finite(&rows) {
        return Err(format!(
            "The color '{label}' holds a value that is not a finite number."
        ));
    }
    let stored = StoredVariant {
        unit: StoredUnit::of(variant.unit),
        reference_path_mm: variant.reference_path_mm,
        absorption: rows,
    };
    let variant_json = serde_json::to_string(&stored).map_err(|error| error.to_string())?;
    Ok(ColorVariantRecord {
        key: variant.key.clone(),
        material_name: material.to_owned(),
        label: label.to_owned(),
        version: RECORD_VERSION,
        variant_json,
    })
}

/// The variant a stored `record` holds, always with `VariantProvenance::User`.
///
/// # Errors
///
/// A record written by a newer build, or JSON that is not a variant's -- as the sentence to show
/// (the page skips such a record and keeps it in the store).
pub fn variant_from_record(record: &ColorVariantRecord) -> Result<ColorVariant, String> {
    if record.version > RECORD_VERSION {
        return Err(format!(
            "The color '{}' was saved by a newer version of the program and cannot be used here.",
            record.label
        ));
    }
    let stored: StoredVariant = serde_json::from_str(&record.variant_json)
        .map_err(|error| format!("The color '{}' is damaged: {error}", record.label))?;
    Ok(ColorVariant {
        key: record.key.clone(),
        label: record.label.clone(),
        material: record.material_name.clone(),
        absorption: stored.absorption.to_tensor(),
        unit: stored.unit.unit(),
        reference_path_mm: stored.reference_path_mm,
        provenance: VariantProvenance::User,
    })
}

// ---------------------------------------------------------------------------------------
// The design color combo
// ---------------------------------------------------------------------------------------

/// The design's material as the host of its color, without any color override.
///
/// It is the material the design traces (`traced_material_for`), so its rank and its own tensor
/// are the host's. `None` when the design names no resolvable material, and for a design with no
/// material name at all: a refractive-index guess is not a host (a later index change could guess
/// a material of another rank), so a nameless design gets the plain list and no variant.
#[must_use]
pub fn host_material(design: &Design, custom: &[GemMaterial]) -> Option<GemMaterial> {
    if design
        .material
        .name
        .as_deref()
        .is_none_or(|name| name.trim().is_empty())
    {
        return None;
    }
    let (name, unresolved) = traced_material_for(design, custom);
    if unresolved.is_some() {
        return None;
    }
    EditorMaterialLookup::new(custom).lookup(&name)
}

/// The isotropic stand-in host of a design whose material cannot be resolved: the plain list of
/// the material's own color, the presets and Custom.
fn unresolved_host() -> GemMaterial {
    GemMaterial::new_custom("", 1.5, 0.0, 0.0, [0.0; 3])
}

/// What a combo list was built from: it is rebuilt only when one of these changes, not on every
/// refresh (a refresh runs after every edit).
#[derive(Debug, Clone, PartialEq)]
struct ListKey {
    host_name: String,
    host_rank: TensorRank,
    host_absorption: pleochroma::optics::absorption::AbsorptionTensor,
    /// The legacy preset triple of the design (the "Legacy" row exists only for it).
    legacy_triple: Option<[f32; 3]>,
    /// Whether the design stores the old L*C*h bands (the "Legacy" row exists only without them).
    legacy_bands: bool,
    /// The keys of the saved variants of the host's material.
    saved_keys: Vec<String>,
}

/// The design color combo for the design on the bench.
#[derive(Debug, Clone)]
pub struct DesignColors {
    /// The rows in combo order: Material Default, the material's variants (built in, then
    /// saved), the generic presets (isotropic hosts only), Custom last.
    pub items: Vec<ColorEntryItem>,
    /// The row that shows the design's own color.
    pub selected: usize,
    /// The design carries a legacy single color on a pleochroic material: the dialog shows the
    /// "Legacy Color: No Pleochroism" note with "Convert".
    pub legacy_note: bool,
    /// The resolved host, `None` when the design's material does not resolve.
    pub host: Option<GemMaterial>,
    variants: Vec<ColorVariant>,
    key: ListKey,
}

impl DesignColors {
    /// The combo for `design`. `previous` is the list shown last time: when nothing the rows
    /// depend on changed (host, legacy color, saved variants of the material) its rows are kept
    /// instead of recomputing every swatch. `saved` is every saved variant of the browser; the
    /// ones of another material or that the host does not accept are skipped by the list.
    #[must_use]
    pub fn for_design(
        previous: Option<&Self>,
        design: &Design,
        custom: &[GemMaterial],
        saved: &[ColorVariant],
    ) -> Self {
        let host = host_material(design, custom);
        let listed = host.clone().unwrap_or_else(unresolved_host);
        let key = ListKey {
            host_name: listed.name.clone(),
            host_rank: host_rank(&listed),
            host_absorption: listed.absorption.clone(),
            legacy_triple: design.material.body_color_override,
            legacy_bands: design
                .material
                .body_color_bands_override
                .as_ref()
                .is_some_and(|bands| !bands.is_empty()),
            saved_keys: saved
                .iter()
                .filter(|variant| variant.material.eq_ignore_ascii_case(&listed.name))
                .map(|variant| variant.key.clone())
                .collect(),
        };
        let (items, variants) = match previous {
            Some(previous) if previous.key == key => {
                (previous.items.clone(), previous.variants.clone())
            }
            _ => (
                color_entries_for_design(&listed, saved, &design.material),
                listed_variants(&listed, saved),
            ),
        };
        let selected = entry_for_selection(&design.material, &items);
        let legacy_note = host
            .as_ref()
            .is_some_and(|host| legacy_color_on_pleochroic_host(&design.material, host));
        Self {
            items,
            selected,
            legacy_note,
            host,
            variants,
            key,
        }
    }

    /// The combo labels, in row order.
    #[must_use]
    pub fn labels(&self) -> Vec<String> {
        self.items.iter().map(|item| item.label.clone()).collect()
    }

    /// The row's swatch as sRGB bytes; black for Custom, which has none (the page hides it).
    #[must_use]
    pub fn swatch_bytes(&self) -> Vec<[u8; 3]> {
        self.items
            .iter()
            .map(|item| item.swatch_rgb().unwrap_or([0; 3]))
            .collect()
    }

    /// The tooltip sentences, in row order.
    #[must_use]
    pub fn tips(&self) -> Vec<String> {
        self.items.iter().map(|item| item.tooltip.clone()).collect()
    }

    /// The entry of combo row `index`; `None` outside the list.
    #[must_use]
    pub fn entry_at(&self, index: i32) -> Option<&ColorEntry> {
        let at = usize::try_from(index).ok()?;
        self.items.get(at).map(|item| &item.entry)
    }

    /// Whether row `index` is the Custom row (the last one).
    #[must_use]
    pub fn is_custom(&self, index: i32) -> bool {
        self.entry_at(index) == Some(&ColorEntry::Custom)
    }

    /// The material an Apply produces: `material` (the parsed material/RI row) with the color of
    /// combo row `color_index` applied on top, given the design's `current` selection. A Custom
    /// row keeps the design's own color, a variant or preset pick replaces it. An index outside
    /// the list falls back to the fixed-index contract of `with_body_color_choice`.
    #[must_use]
    pub fn material_for_apply(
        &self,
        material: MaterialSelection,
        color_index: i32,
        current: &MaterialSelection,
    ) -> MaterialSelection {
        match self.entry_at(color_index) {
            Some(entry) => selection_with_entry(material, entry, current, &self.variants),
            None => with_body_color_choice(material, color_index, current),
        }
    }
}

// ---------------------------------------------------------------------------------------
// The per-axis Custom editor
// ---------------------------------------------------------------------------------------

/// The sliders' default `[L*, C*, h]` for a design whose material cannot be resolved.
pub const DEFAULT_LCH: [f64; 3] = [55.0, 40.0, 260.0];

/// What the badge says while a row is still being solved.
pub const MATCHING_BADGE: &str = "Matching...";

/// The width (mm) of the stone of `design`: its girdle diameter, or the default stone width
/// (7 mm) for a design with none. This is the number a stored color's `reference_path_mm` holds.
#[must_use]
pub fn stone_width_mm(design: &Design) -> f64 {
    design
        .girdle_diameter_mm
        .filter(|mm| mm.is_finite() && *mm > 0.0)
        .unwrap_or_else(|| f64::from(PHYSICS_DEFAULT_STONE_WIDTH_MM))
}

/// What the Custom editor shows for its current rows.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisPreview {
    /// The swatches (sRGB 0..1): the face-up color, then one per direction for a two- or
    /// three-color host; else the solved color at 3 mm, 5 mm, 10 mm and the stone's path.
    pub swatches: Vec<[f32; 3]>,
    /// The swatch captions.
    pub labels: Vec<String>,
    /// The reachability sentence; [`MATCHING_BADGE`] while a row is still being solved.
    pub badge: String,
    /// Whether every row's color is reachable by the material model.
    pub reachable: bool,
    /// One caption per row: "Along the Axis: Blue".
    pub captions: Vec<String>,
    /// One chip color per row (sRGB 0..1), at the stone's path; black until the row is solved.
    pub row_colors: Vec<[f32; 3]>,
}

/// The Custom editor's state: one `[L*, C*, h]` row per direction of the host, the solve of each
/// row and the row the sliders edit.
#[derive(Debug, Clone)]
pub struct AxisEditor {
    host: Option<GemMaterial>,
    rank: TensorRank,
    /// The stone width (mm) a stored color is calibrated for (its `reference_path_mm`).
    stone_width_mm: f64,
    /// The solver path (mm): the stone's girdle times 1.5.
    path_mm: f64,
    rows: Vec<[f64; 3]>,
    solves: Vec<Option<LchSolve>>,
    active: usize,
    /// The rows the editor opened with, while a stored color exists and nothing was touched: an
    /// Apply of the untouched seed would only re-solve the stored color, so it does nothing.
    /// A legacy single color on a pleochroic host is not such a color: "Convert" exists to turn
    /// it into a per-axis one, so an untouched Apply stores that conversion.
    opened: Option<Vec<[f64; 3]>>,
}

impl AxisEditor {
    /// The editor for the design selection `material`, whose resolved host is `host` (`None`:
    /// the material does not resolve, one legacy color), for a stone `stone_width_mm` wide
    /// ([`stone_width_mm`]) at the design's solver `path_mm`.
    /// The rows start on the stored color of each direction (see `seed_axis_lch`).
    #[must_use]
    pub fn open(
        material: &MaterialSelection,
        host: Option<GemMaterial>,
        stone_width_mm: f64,
        path_mm: f64,
    ) -> Self {
        let (rank, rows) = host.as_ref().map_or_else(
            || {
                (
                    TensorRank::Isotropic,
                    vec![lch_from_material(material, path_mm).unwrap_or(DEFAULT_LCH)],
                )
            },
            |host| {
                (
                    host_rank(host),
                    seed_axis_lch(material, host, stone_width_mm, path_mm),
                )
            },
        );
        let converts = host
            .as_ref()
            .is_some_and(|host| legacy_color_on_pleochroic_host(material, host));
        let opened = (material.has_body_color_override() && !converts).then(|| rows.clone());
        Self {
            host,
            rank,
            stone_width_mm,
            path_mm,
            solves: vec![None; rows.len()],
            rows,
            active: 0,
            opened,
        }
    }

    /// The host's rank: one, two or three rows.
    #[must_use]
    pub const fn rank(&self) -> TensorRank {
        self.rank
    }

    /// Whether the design's material resolved (a color can only be saved for a known material).
    #[must_use]
    pub const fn has_host(&self) -> bool {
        self.host.is_some()
    }

    /// The solver path the rows are solved at.
    #[must_use]
    pub const fn path_mm(&self) -> f64 {
        self.path_mm
    }

    /// The stone width a stored color of this editor is calibrated for.
    #[must_use]
    pub const fn stone_width_mm(&self) -> f64 {
        self.stone_width_mm
    }

    /// The `[L*, C*, h]` of every row.
    #[must_use]
    pub fn rows(&self) -> &[[f64; 3]] {
        &self.rows
    }

    /// The row the sliders edit.
    #[must_use]
    pub const fn active(&self) -> usize {
        self.active
    }

    /// The row labels in Title Case ("Along the Axis"; "Alpha"; "Color" for one row).
    #[must_use]
    pub fn labels(&self) -> Vec<&'static str> {
        axis_rows(self.rank).iter().map(|row| row.label).collect()
    }

    /// One tooltip sentence per row: which direction it colors.
    #[must_use]
    pub fn hints(&self) -> Vec<&'static str> {
        axis_rows(self.rank).iter().map(|row| row.hint).collect()
    }

    /// Makes `row` the one the sliders edit; a row outside the list is ignored.
    pub const fn select(&mut self, row: usize) {
        if row < self.rows.len() {
            self.active = row;
        }
    }

    /// The `[L*, C*, h]` of the row the sliders edit.
    #[must_use]
    pub fn active_lch(&self) -> [f64; 3] {
        self.rows.get(self.active).copied().unwrap_or(DEFAULT_LCH)
    }

    /// The sliders (or the picker) set the active row; the editor no longer counts as untouched.
    pub fn set_active_lch(&mut self, lch: [f64; 3]) {
        self.opened = None;
        if let Some(row) = self.rows.get_mut(self.active) {
            *row = lch;
        }
    }

    /// The solve of `row` when it is for the row's current target and path.
    fn current(&self, row: usize) -> Option<&LchSolve> {
        let target = self.rows.get(row)?;
        self.solves.get(row)?.as_ref().filter(|solve| {
            solve.lch == *target && solve.path_mm.to_bits() == self.path_mm.to_bits()
        })
    }

    /// The rows that still need a solve (no answer, or an answer for an older target), with
    /// their targets, in row order.
    #[must_use]
    pub fn pending(&self) -> Vec<(usize, [f64; 3])> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(row, _)| self.current(*row).is_none())
            .map(|(row, target)| (row, *target))
            .collect()
    }

    /// Keeps the answer for `row`; `false` (and nothing kept) when it is for a target the row no
    /// longer has (a newer edit superseded it).
    pub fn store_solve(&mut self, row: usize, solve: LchSolve) -> bool {
        let fresh = self
            .rows
            .get(row)
            .is_some_and(|target| solve.lch == *target)
            && solve.path_mm.to_bits() == self.path_mm.to_bits();
        if !fresh {
            return false;
        }
        if let Some(slot) = self.solves.get_mut(row) {
            *slot = Some(solve);
        }
        true
    }

    /// Whether every row has a current solve.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        (0..self.rows.len()).all(|row| self.current(row).is_some())
    }

    /// Every row's current solve, `None` while one is missing.
    fn ready_solves(&self) -> Option<Vec<LchSolve>> {
        (0..self.rows.len())
            .map(|row| self.current(row).cloned())
            .collect()
    }

    /// The color the design stores for the rows: key `custom`, a tensor of the host's rank, per
    /// millimetre, calibrated for the stone's width (`custom_choice`). `None` while a row is
    /// still being solved.
    #[must_use]
    pub fn choice(&self) -> Option<VariantChoice> {
        custom_choice(self.rank, &self.ready_solves()?, self.stone_width_mm)
    }

    /// What the editor shows now.
    #[must_use]
    pub fn preview(&self) -> AxisPreview {
        let rows = axis_rows(self.rank);
        let captions = rows
            .iter()
            .zip(&self.rows)
            .map(|(row, lch)| format!("{}: {}", row.label, color_family(*lch)))
            .collect();
        let row_colors = (0..self.rows.len())
            .map(|row| {
                self.current(row)
                    .and_then(|solve| solve.swatches.last().copied())
                    .unwrap_or([0.0; 3])
            })
            .collect();
        let Some(solves) = self.ready_solves() else {
            return AxisPreview {
                swatches: Vec::new(),
                labels: Vec::new(),
                badge: MATCHING_BADGE.to_owned(),
                reachable: true,
                captions,
                row_colors,
            };
        };
        let reachable = solves.iter().all(|solve| solve.reachable);
        let badge = solves
            .iter()
            .zip(rows)
            .find(|(solve, _)| !solve.reachable)
            .or_else(|| solves.get(self.active).zip(rows.get(self.active)))
            .map(|(solve, row)| {
                if solves.len() > 1 {
                    format!(
                        "{}: {}",
                        row.label,
                        badge_text(solve.delta_e, solve.reachable)
                    )
                } else {
                    badge_text(solve.delta_e, solve.reachable)
                }
            })
            .unwrap_or_default();
        let (swatches, labels) = if self.rank == TensorRank::Isotropic {
            solves
                .first()
                .map(|solve| (solve.swatches.clone(), swatch_labels(self.path_mm)))
                .unwrap_or_default()
        } else {
            tensor_from_solves(self.rank, &solves).map_or_else(
                || (Vec::new(), Vec::new()),
                |tensor| {
                    let mut labels = vec!["Face-Up".to_owned()];
                    labels.extend(rows.iter().map(|row| row.short.to_owned()));
                    (direction_swatches(&tensor, self.path_mm), labels)
                },
            )
        };
        AxisPreview {
            swatches,
            labels,
            badge,
            reachable,
            captions,
            row_colors,
        }
    }

    /// The selection with the editor's rows stored as the design's color (one edit, whatever the
    /// number of rows), or `None` when there is nothing to change: the editor is still on the
    /// seed it opened with, a row is not solved yet, or the design already has exactly that.
    ///
    /// A host that did not resolve keeps the old single legacy color (bands plus triple). A legacy
    /// single color on a pleochroic host is converted even when no slider moved: the rows are its
    /// color in every direction, stored as a per-axis Custom Color.
    #[must_use]
    pub fn applied_to(&self, material: &MaterialSelection) -> Option<MaterialSelection> {
        if self.opened.is_some() {
            return None;
        }
        if self.host.is_none() {
            return recoloured_with_solve(material, self.current(0)?);
        }
        recoloured_with_choice(material, self.choice()?)
    }

    /// The sentence for an Apply that changed nothing ([`Self::applied_to`] was `None`), so the
    /// dialog never closes without a word.
    #[must_use]
    pub fn nothing_to_apply(&self) -> &'static str {
        if self.is_ready() {
            "The design already has this color. Move a slider or pick another color to change it."
        } else {
            "The color is still being matched. Wait for it to finish, then apply it."
        }
    }

    /// Marks the editor as showing exactly what the design stores now (after a saved variant was
    /// put on the design), so an Apply changes nothing until a row is touched.
    pub fn settle(&mut self) {
        self.opened = Some(self.rows.clone());
    }

    /// The variant "Save as Variant..." keeps: the editor's rows under the name `label` and
    /// `key`, for the host's material, per millimetre, checked against the host. Its
    /// `reference_path_mm` is the stone width the rows were made for.
    ///
    /// # Errors
    ///
    /// No resolvable material, a blank name, a row still being solved, or a color the host does
    /// not accept (its rank) -- as the sentence to show.
    pub fn variant_to_save(&self, label: &str, key: String) -> Result<ColorVariant, String> {
        let host = self.host.as_ref().ok_or_else(|| {
            "A saved color belongs to one material: pick a material for this design first."
                .to_owned()
        })?;
        let label = label.trim();
        if label.is_empty() {
            return Err("Name the color first.".to_owned());
        }
        let choice = self
            .choice()
            .ok_or_else(|| "Wait for the color to finish matching, then save it.".to_owned())?;
        let variant = ColorVariant {
            key,
            label: label.to_owned(),
            material: host.name.clone(),
            absorption: choice.absorption,
            unit: choice.unit,
            // `custom_choice` stored the stone width (the meaning of `reference_path_mm`).
            reference_path_mm: choice.reference_path_mm,
            provenance: VariantProvenance::User,
        };
        variant.validate_for(host).map_err(|problem| {
            format!("The color '{label}' does not fit {}: {problem}", host.name)
        })?;
        Ok(variant)
    }
}

/// The selection with the saved `variant` used as the design's color.
///
/// `None` when the design already uses exactly that. "Save as Variant..." applies the variant it
/// saved this way, as one edit, like the desktop does.
#[must_use]
pub fn applied_variant(
    material: &MaterialSelection,
    variant: &ColorVariant,
) -> Option<MaterialSelection> {
    recoloured_with_choice(material, VariantChoice::from_variant(variant))
}

#[cfg(test)]
mod tests;
