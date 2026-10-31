//! The material editor's render-physics sections beyond the curves: the inclusion families and
//! the play-of-color structure of a custom material, and the [`PhysicsForm`] that gathers all
//! three sections for Save.
//!
//! The state lives in `InclusionFamiliesModel` and `StructureParamsModel`; the family list is
//! edited only through this module's callbacks (so the rules apply to the screen and to the
//! tests alike), the structure fields are bound straight to their sliders. [`push_row`] fills
//! the sections when the dialog's pre-fill is pushed and [`read_form`] reads them when Save runs.
//! No database here.

use super::curves::{self, CurvesForm};
use crate::{FamilyRow, InclusionFamiliesModel, MainWindow, StructureParamsModel};
use pleochroma::optics::{
    inclusions::{
        CrystalDir, Family, FamilyKind, MAX_FAMILIES,
        bragg::{StructureKind, StructurePattern, StructureSpec},
    },
    materials::{GemMaterial, OpticalCharacter},
};
use pleochroma_cut_core::{
    CrystalOrientation,
    material::{
        CustomStructure, PerAxisDispersion, custom_structure_from_json, custom_structure_to_json,
        inclusion_families_from_json, inclusion_families_to_json, per_axis_dispersion_from_json,
        per_axis_dispersion_to_json, with_custom_physics, with_custom_structure,
    },
};
use pleochroma_vault::model::material::CustomMaterialRow;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

/// The `edit_family` field ids (the same numbers `inclusion_families.slint` passes).
const FIELD_KIND: i32 = 0;
const FIELD_DIR: i32 = 1;
const FIELD_COLOR: i32 = 2;
const FIELD_TURN: i32 = 3;
const FIELD_TILT: i32 = 4;
const FIELD_STRENGTH: i32 = 5;
const FIELD_CONE: i32 = 6;
const FIELD_FORWARD: i32 = 7;

/// The spectral exponent of each color-combo entry: Grey, Slightly Blue, Blue, Strong Blue.
const COLOR_EXPONENTS: [u8; 4] = [0, 1, 2, 4];

/// The `kind` index of a stored choice that names no kind (kept, not rendered).
const KIND_UNAPPLIED: i32 = 2;

/// The structure of Precious Opal (White), the starting point of an Opal choice (used when the
/// built-in defaults cannot be read).
const OPAL_FALLBACK: StructureSpec = StructureSpec {
    pattern: StructurePattern::Harlequin,
    seed: 12345,
    size: None,
    color_low_nm: 440.0,
    color_high_nm: 680.0,
    purity: 0.05,
    coverage: 0.8,
};

/// A whole number of the `f32` a Slint control hands over, limited to `0..=max`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the value is limited to a few hundred before the cast, so it fits an i32 exactly"
)]
fn choice(value: f32, max: i32) -> i32 {
    (value.clamp(-1000.0, 1000.0).round() as i32).clamp(0, max)
}

/// A family a new "Add Family" starts with: growth tubes along the crystal axis.
const fn new_family_row() -> FamilyRow {
    family_to_row(&Family {
        kind: FamilyKind::Needle,
        dir: CrystalDir::AlongC,
        sigma: 0.15,
        p: 0,
        lobe: 0.15,
        rho: 0.3,
    })
}

/// The editor row of a stored family.
pub(super) const fn family_to_row(family: &Family) -> FamilyRow {
    let (dir, turn, tilt) = match family.dir {
        CrystalDir::AlongC => (0, 0.0, 0.0),
        CrystalDir::Basal { turn_deg } => (1, turn_deg, 0.0),
        CrystalDir::AlphaAxis => (2, 0.0, 0.0),
        CrystalDir::Tilted { tilt_deg, turn_deg } => (3, turn_deg, tilt_deg),
    };
    FamilyRow {
        kind: match family.kind {
            FamilyKind::Needle => 0,
            FamilyKind::Platelet => 1,
            FamilyKind::RandomNeedle => 2,
        },
        dir,
        turn,
        tilt,
        strength: family.sigma,
        color: match family.p {
            0 => 0,
            1 => 1,
            2 => 2,
            _ => 3,
        },
        cone: family.lobe,
        forward: family.rho,
    }
}

/// The family of an editor row. Random needles have no direction.
pub(super) fn row_to_family(row: &FamilyRow) -> Family {
    let kind = match row.kind {
        1 => FamilyKind::Platelet,
        2 => FamilyKind::RandomNeedle,
        _ => FamilyKind::Needle,
    };
    let dir = match row.dir {
        _ if kind == FamilyKind::RandomNeedle => CrystalDir::AlongC,
        1 => CrystalDir::Basal { turn_deg: row.turn },
        2 => CrystalDir::AlphaAxis,
        3 => CrystalDir::Tilted {
            tilt_deg: row.tilt,
            turn_deg: row.turn,
        },
        _ => CrystalDir::AlongC,
    };
    let color = usize::try_from(row.color).unwrap_or(0).min(3);
    Family {
        kind,
        dir,
        sigma: row.strength,
        p: COLOR_EXPONENTS[color],
        lobe: row.cone,
        rho: row.forward,
    }
}

/// Applies one edit of the family editor to `row`. `false` when the field is unknown or the value
/// is not a finite number (the row is left alone).
pub(super) fn apply_edit(row: &mut FamilyRow, field: i32, value: f32) -> bool {
    if !value.is_finite() {
        return false;
    }
    match field {
        FIELD_KIND => row.kind = choice(value, 2),
        FIELD_DIR => row.dir = choice(value, 3),
        FIELD_COLOR => row.color = choice(value, 3),
        FIELD_TURN => row.turn = value.clamp(0.0, 360.0),
        FIELD_TILT => row.tilt = value.clamp(0.0, 90.0),
        FIELD_STRENGTH => row.strength = value.max(0.0),
        FIELD_CONE => row.cone = value.clamp(0.0, 1.0),
        FIELD_FORWARD => row.forward = value.clamp(0.0, 0.95),
        _ => return false,
    }
    true
}

/// The families the rows make, or `None` for no rows.
///
/// # Errors
///
/// The sentence for the toast when there are more rows than a stone holds or a family cannot
/// scatter anything.
pub(super) fn families_of(rows: &[FamilyRow]) -> Result<Option<Vec<Family>>, String> {
    if rows.is_empty() {
        return Ok(None);
    }
    if rows.len() > MAX_FAMILIES {
        return Err(format!(
            "A stone holds at most {MAX_FAMILIES} inclusion families."
        ));
    }
    let families: Vec<Family> = rows.iter().map(row_to_family).collect();
    let usable = |f: &Family| f.sigma.is_finite() && f.sigma > 0.0 && f.lobe.is_finite();
    if !families.iter().all(usable) {
        return Err("Every inclusion family needs a density above zero.".to_string());
    }
    Ok(Some(families))
}

/// The plain-sentence notes under the family list, one per line, for a material whose optical
/// character combo is at `character_idx` (0 = Isotropic, 1/2 = Uniaxial, 3/4 = Biaxial).
pub(super) fn family_notes(rows: &[FamilyRow], character_idx: i32) -> String {
    let mut notes: Vec<&str> = Vec::new();
    let directed = |f: &&FamilyRow| f.kind != 2;
    if rows.iter().filter(directed).any(|f| f.dir == 2) && character_idx < 3 {
        notes.push(
            "Along the Alpha Axis points along the reference direction, which only a biaxial \
             material fixes. Pick another direction for this material.",
        );
    }
    if character_idx == 0 && rows.iter().filter(directed).any(|f| f.dir != 0) {
        notes.push(
            "An isotropic material has no crystal axis. Set its optical character to uniaxial or \
             biaxial so the directions mean something.",
        );
    }
    if rows.len() >= MAX_FAMILIES {
        notes.push("A stone holds at most four families.");
    }
    notes.join("\n")
}

/// A text that differs whenever the rows do, for the dialog's unsaved-changes check.
pub(super) fn rows_signature(rows: &[FamilyRow]) -> String {
    rows.iter()
        .map(|r| {
            format!(
                "{}|{}|{}|{}|{}|{}|{}|{}",
                r.kind, r.dir, r.turn, r.tilt, r.strength, r.color, r.cone, r.forward
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// The Play-of-Color section as the dialog holds it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct StructureForm {
    /// The material has a structure at all.
    pub(super) enabled: bool,
    /// 0 = Opal, 1 = Labradorite, 2 = a stored choice that names no kind.
    pub(super) kind: i32,
    /// 0 = Pinfire, 1 = Harlequin, 2 = Broad Flash.
    pub(super) pattern: i32,
    /// The cell size below replaces the pattern's own.
    pub(super) use_size: bool,
    /// The cell size, model units.
    pub(super) size: f32,
    /// The shortest color, nanometres.
    pub(super) color_low: f32,
    /// The longest color, nanometres.
    pub(super) color_high: f32,
    /// The relative bandwidth.
    pub(super) purity: f32,
    /// The fraction of domains that reflect.
    pub(super) coverage: f32,
    /// The seed, as the bits of an unsigned number (Slint holds whole numbers as `i32`).
    pub(super) seed: i32,
}

/// The pattern combo index of `pattern`.
const fn pattern_index(pattern: StructurePattern) -> i32 {
    match pattern {
        StructurePattern::Pinfire => 0,
        StructurePattern::Harlequin => 1,
        StructurePattern::BroadFlash => 2,
    }
}

/// The seed as the whole number Slint holds.
const fn seed_to_i32(seed: u32) -> i32 {
    i32::from_ne_bytes(seed.to_ne_bytes())
}

/// The seed from the whole number Slint holds.
const fn seed_from_i32(seed: i32) -> u32 {
    u32::from_ne_bytes(seed.to_ne_bytes())
}

/// The next seed after `seed`: a fixed step of a linear congruential generator, so "Reseed"
/// always moves on to another layout.
pub(super) const fn next_seed(seed: i32) -> i32 {
    seed_to_i32(
        seed_from_i32(seed)
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223),
    )
}

/// The default choice of `kind` (0 = Opal, 1 = Labradorite).
fn default_spec(kind: i32) -> StructureSpec {
    let name = if kind == 1 {
        "Labradorite"
    } else {
        "Precious Opal (White)"
    };
    StructureSpec::default_for(name).unwrap_or(OPAL_FALLBACK)
}

impl StructureForm {
    /// The section with the defaults of `kind` filled in, and `enabled` as given.
    pub(super) fn with_defaults(kind: i32, enabled: bool) -> Self {
        let spec = default_spec(kind);
        Self::of_spec(&spec, i32::from(kind == 1), enabled)
    }

    fn of_spec(spec: &StructureSpec, kind: i32, enabled: bool) -> Self {
        Self {
            enabled,
            kind,
            pattern: pattern_index(spec.pattern),
            use_size: spec.size.is_some(),
            size: spec.size.unwrap_or(0.2),
            color_low: spec.color_low_nm,
            color_high: spec.color_high_nm,
            purity: spec.purity,
            coverage: spec.coverage,
            seed: seed_to_i32(spec.seed),
        }
    }

    /// The section as a stored row opens it: off for a row with no (or unreadable) structure.
    pub(super) fn from_stored(text: Option<&str>) -> Self {
        let Some(stored) = text.and_then(custom_structure_from_json) else {
            return Self::with_defaults(0, false);
        };
        let kind = match stored.kind {
            Some(StructureKind::Opal) => 0,
            Some(StructureKind::Labradorite) => 1,
            None => KIND_UNAPPLIED,
        };
        Self::of_spec(&stored.spec, kind, true)
    }

    /// The structure to store, or `None` when the section is off.
    ///
    /// # Errors
    ///
    /// The sentence for the toast when the colors cannot make a structure.
    pub(super) fn resolve(&self) -> Result<Option<CustomStructure>, String> {
        if !self.enabled {
            return Ok(None);
        }
        let finite = [
            self.size,
            self.color_low,
            self.color_high,
            self.purity,
            self.coverage,
        ]
        .iter()
        .all(|v| v.is_finite());
        if !finite || self.color_low <= 0.0 || self.color_high < self.color_low {
            return Err(
                "The Play-of-Color section needs its longest color to be at least its shortest."
                    .to_string(),
            );
        }
        let pattern = match self.pattern {
            0 => StructurePattern::Pinfire,
            2 => StructurePattern::BroadFlash,
            _ => StructurePattern::Harlequin,
        };
        let kind = match self.kind {
            0 => Some(StructureKind::Opal),
            1 => Some(StructureKind::Labradorite),
            _ => None,
        };
        Ok(Some(CustomStructure {
            kind,
            spec: StructureSpec {
                pattern,
                seed: seed_from_i32(self.seed),
                size: (self.use_size && self.size > 0.0).then_some(self.size),
                color_low_nm: self.color_low,
                color_high_nm: self.color_high,
                purity: self.purity.clamp(0.001, 1.0),
                coverage: self.coverage.clamp(0.0, 1.0),
            },
        }))
    }
}

/// Everything the three render-physics sections hold, as Save receives it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct PhysicsForm {
    /// The per-axis curves.
    pub(super) curves: CurvesForm,
    /// The inclusion family rows.
    pub(super) families: Vec<FamilyRow>,
    /// The play-of-color structure.
    pub(super) structure: StructureForm,
}

/// What a [`PhysicsForm`] stores for a material of one optical character.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct ResolvedPhysics {
    /// The per-axis curves.
    pub(super) per_axis: Option<PerAxisDispersion>,
    /// The inclusion families.
    pub(super) families: Option<Vec<Family>>,
    /// The play-of-color structure.
    pub(super) structure: Option<CustomStructure>,
}

impl PhysicsForm {
    /// The sections as a stored row opens them (all empty for `None`, a built-in selection).
    pub(super) fn from_row(row: Option<&CustomMaterialRow>) -> Self {
        let families = row
            .and_then(|r| r.inclusion_families_json.as_deref())
            .and_then(inclusion_families_from_json)
            .unwrap_or_default();
        Self {
            curves: CurvesForm::from_stored(
                row.and_then(|r| r.per_axis_dispersion_json.as_deref()),
            ),
            families: families.iter().map(family_to_row).collect(),
            structure: StructureForm::from_stored(row.and_then(|r| r.structure_json.as_deref())),
        }
    }

    /// The data to store for a material of `character`.
    ///
    /// # Errors
    ///
    /// The sentence for the toast when a section holds something that cannot be stored.
    pub(super) fn resolve(&self, character: OpticalCharacter) -> Result<ResolvedPhysics, String> {
        Ok(ResolvedPhysics {
            per_axis: self.curves.resolve(character)?,
            families: families_of(&self.families)?,
            structure: self.structure.resolve()?,
        })
    }
}

impl ResolvedPhysics {
    /// What a stored row already holds (a save that authored nothing keeps it).
    pub(super) fn from_row(row: &CustomMaterialRow) -> Self {
        Self {
            per_axis: row
                .per_axis_dispersion_json
                .as_deref()
                .and_then(per_axis_dispersion_from_json),
            families: row
                .inclusion_families_json
                .as_deref()
                .and_then(inclusion_families_from_json),
            structure: row
                .structure_json
                .as_deref()
                .and_then(custom_structure_from_json),
        }
    }

    /// The text of the curves column; blank clears it.
    pub(super) fn per_axis_text(&self) -> String {
        self.per_axis
            .as_ref()
            .and_then(per_axis_dispersion_to_json)
            .unwrap_or_default()
    }

    /// The text of the families column; blank clears it.
    pub(super) fn families_text(&self) -> String {
        self.families
            .as_deref()
            .and_then(inclusion_families_to_json)
            .unwrap_or_default()
    }

    /// The text of the structure column, with the kind the material imitates; blank clears it.
    pub(super) fn structure_text(&self) -> String {
        self.structure
            .as_ref()
            .and_then(custom_structure_to_json)
            .unwrap_or_default()
    }

    /// `material` with the data applied, the way a library row applies it when it loads: call it
    /// after the default orientation is baked in.
    pub(super) fn apply(
        &self,
        material: GemMaterial,
        orientation: Option<CrystalOrientation>,
    ) -> GemMaterial {
        let material = with_custom_physics(
            material,
            self.per_axis,
            self.families.as_deref(),
            orientation,
        );
        with_custom_structure(material, self.structure.as_ref())
    }
}

/// The rows of the family editor as they stand.
fn rows_of(ui: &MainWindow) -> Vec<FamilyRow> {
    ui.global::<InclusionFamiliesModel>()
        .get_rows()
        .iter()
        .collect()
}

/// Replaces the family editor's rows.
fn set_rows(ui: &MainWindow, rows: Vec<FamilyRow>) {
    ui.global::<InclusionFamiliesModel>()
        .set_rows(ModelRc::new(VecModel::from(rows)));
}

/// Fills `StructureParamsModel` from `form`.
fn push_structure(ui: &MainWindow, form: &StructureForm) {
    let model = ui.global::<StructureParamsModel>();
    model.set_enabled(form.enabled);
    model.set_kind(form.kind);
    model.set_pattern(form.pattern);
    model.set_use_size(form.use_size);
    model.set_size(form.size);
    model.set_color_low(form.color_low);
    model.set_color_high(form.color_high);
    model.set_purity(form.purity);
    model.set_coverage(form.coverage);
    model.set_seed(form.seed);
}

/// The Play-of-Color section as it stands.
fn read_structure(ui: &MainWindow) -> StructureForm {
    let model = ui.global::<StructureParamsModel>();
    StructureForm {
        enabled: model.get_enabled(),
        kind: model.get_kind(),
        pattern: model.get_pattern(),
        use_size: model.get_use_size(),
        size: model.get_size(),
        color_low: model.get_color_low(),
        color_high: model.get_color_high(),
        purity: model.get_purity(),
        coverage: model.get_coverage(),
        seed: model.get_seed(),
    }
}

/// Fills the three render-physics sections for the material the dialog is about to edit: the
/// stored row's data, or the empty sections for a built-in selection.
pub(super) fn push_row(ui: &MainWindow, row: Option<&CustomMaterialRow>) {
    let form = PhysicsForm::from_row(row);
    curves::push(ui, &form.curves);
    set_rows(ui, form.families);
    push_structure(ui, &form.structure);
}

/// The three sections as they stand (what the dialog's Save stores).
pub(super) fn read_form(ui: &MainWindow) -> PhysicsForm {
    PhysicsForm {
        curves: curves::read_form(ui),
        families: rows_of(ui),
        structure: read_structure(ui),
    }
}

/// Registers the callbacks of the family editor and of the Play-of-Color section.
pub(super) fn setup_callbacks(ui: &MainWindow) {
    let families = ui.global::<InclusionFamiliesModel>();
    let weak = ui.as_weak();
    families.on_add_family(move || {
        let Some(ui) = weak.upgrade() else { return };
        let mut rows = rows_of(&ui);
        if rows.len() < MAX_FAMILIES {
            rows.push(new_family_row());
            set_rows(&ui, rows);
        }
    });
    let weak = ui.as_weak();
    families.on_remove_family(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        let mut rows = rows_of(&ui);
        if let Ok(i) = usize::try_from(index)
            && i < rows.len()
        {
            rows.remove(i);
            set_rows(&ui, rows);
        }
    });
    let weak = ui.as_weak();
    families.on_edit_family(move |index, field, value| {
        let Some(ui) = weak.upgrade() else { return };
        let model = ui.global::<InclusionFamiliesModel>().get_rows();
        let Some(i) = usize::try_from(index).ok() else {
            return;
        };
        let Some(mut row) = model.row_data(i) else {
            return;
        };
        if apply_edit(&mut row, field, value) {
            model.set_row_data(i, row);
        }
    });
    families.on_notes(|rows, character_idx| {
        let rows: Vec<FamilyRow> = rows.iter().collect();
        SharedString::from(family_notes(&rows, character_idx))
    });
    families.on_signature(|rows| {
        let rows: Vec<FamilyRow> = rows.iter().collect();
        SharedString::from(rows_signature(&rows))
    });

    let structure = ui.global::<StructureParamsModel>();
    let weak = ui.as_weak();
    structure.on_load_defaults(move |kind| {
        let Some(ui) = weak.upgrade() else { return };
        // The kind the user picked stays; everything else becomes that kind's default.
        let form = StructureForm::with_defaults(kind, true);
        push_structure(&ui, &form);
    });
    let weak = ui.as_weak();
    structure.on_reseed(move || {
        let Some(ui) = weak.upgrade() else { return };
        let model = ui.global::<StructureParamsModel>();
        model.set_seed(next_seed(model.get_seed()));
    });
}

#[cfg(test)]
mod tests;
