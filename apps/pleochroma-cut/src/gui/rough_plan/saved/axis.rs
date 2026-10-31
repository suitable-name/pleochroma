//! The Crystal Axis part of a plan file (schema version 5).
//!
//! A plan that never used the crystal axis has none of this and is written exactly as before
//! (every field below is skipped when it is empty, and `SavedPlanDto::content_version` keeps the
//! older version). What a version 5 plan adds:
//!
//! - `rough.orientation`: the rough's crystal orientation ([`OrientationDto`]), in the rough frame;
//! - `[axis.rule]`: the plan's rule setting, when the cutter changed it from the material's
//!   default (a Reason alone counts), and one `[[axis.designs]]` table per design with a rule of
//!   its own ([`RuleSettingDto`]; the same table with an `entry_id`);
//! - `[axis.planned]`: the rules the layouts were planned with (the same `rule` and `designs`),
//!   written only when they differ from the rules above, so reopening a plan whose rules were
//!   edited after planning still warns "Rules changed since planning" ([`PlannedRulesDto`]);
//! - `[[axis.stones]]`: the stones the cutter locked, gave a free pose or a Reason ([`StoneChoiceDto`]).
//!   A stone with only a box pose is the version 4 `[[pose_choices]]` table, as it always was.
//!   Like the colour and the pose choices, the stone choices live in the library's side table, not
//!   in the stored payload (see `sections`).
//!
//! This module is the file form and the checks; the core types are
//! `pleochroma_cut_core::rough_plan::orientation`.

use super::dto::{CRYSTAL_AXIS_SCHEMA_VERSION, PoseChoiceDto};
use pleochroma_cut_core::{
    crystal_axis::{
        ColorPresetId, CrystalOrientation, OrientationRule, OrientationRuleKind, RuleStrength,
    },
    rough_plan::{
        POSE_COUNT, RoughLayout,
        fit::StonePose,
        orientation::{PlanSettingsOverrides, RuleChoice, RuleSetting, StoneChoice},
    },
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Longest Reason a plan stores, in characters.
pub const MAX_REASON_CHARS: usize = 500;

/// Most design rules a plan lists: one per design a plan can use.
const MAX_DESIGN_RULES: usize = 990;

/// The rough's crystal orientation as the file holds it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::struct_field_names,
    reason = "a serde shape: the field names are the plan file's keys"
)]
pub struct OrientationDto {
    /// Tilt of the Crystal Axis from the rough's `+Y`, degrees `[0, 90]`.
    pub tilt_deg: f64,
    /// Azimuth of the Crystal Axis from `+X` toward `+Z`, degrees.
    pub azimuth_deg: f64,
    /// The Second Axis turn, degrees; absent when the material's own completion is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second_turn_deg: Option<f64>,
}

/// What a rule setting says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChoiceDto {
    /// The level above (the material's default for the plan, the plan's rule for a design).
    #[default]
    Default,
    /// No rule: the axis is ignored.
    Ignore,
    /// The rule in the other fields.
    Set,
}

/// What a rule asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KindDto {
    /// No rule.
    None,
    /// The Crystal Axis through the table.
    TableAcross,
    /// The Crystal Axis in the table plane.
    InTablePlane,
    /// A chosen tilt (and azimuth).
    CustomAngle,
    /// A color-named preset.
    FaceUp,
}

/// The color-named presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetDto {
    /// The bluest face-up color.
    BestBlueFaceUp,
    /// The most violet face-up color.
    VioletFaceUp,
    /// The face-up color that changes most between two lights.
    StrongestColorChange,
    /// The greenest face-up color.
    BestGreenFaceUp,
}

/// How hard a rule holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrengthDto {
    /// A penalty per degree beyond the tolerance.
    Preferred,
    /// Beyond the tolerance a stone is not used.
    Required,
}

/// A rule setting with its Reason: the plan's (no `entry_id`) or one design's (`entry_id`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RuleSettingDto {
    /// The design the setting belongs to; absent for the plan's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<i64>,
    /// What the setting says.
    #[serde(default)]
    pub choice: ChoiceDto,
    /// The kind of rule (with `choice = "set"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<KindDto>,
    /// A custom angle's tilt, degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tilt_deg: Option<f64>,
    /// A custom angle's azimuth, degrees; absent when it is free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azimuth_deg: Option<f64>,
    /// A color-named preset's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<PresetDto>,
    /// The tolerance, degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance_deg: Option<f64>,
    /// How hard the rule holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<StrengthDto>,
    /// A Preferred rule's weight, percent of the score per degree beyond the tolerance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight_per_deg: Option<f64>,
    /// Why the cutter changed it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

/// A free pose of a stone.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FreePoseDto {
    /// Caliper-frame origin in the rough frame, mm.
    pub center_mm: [f64; 3],
    /// Caliper-frame basis unit vectors in the rough frame.
    pub axes: [[f64; 3]; 3],
    /// Millimetres per model unit.
    pub mm_per_unit: f64,
}

/// What the cutter chose for one stone (the library's `stone_pose_choice` row, without the plan).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoneChoiceDto {
    /// The result's index in the plan's layouts. 0-based.
    pub layout: u32,
    /// The stone's index in that layout. 0-based.
    pub stone: u32,
    /// The box pose, `0..4`; absent is the planner's own (`0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pose: Option<u8>,
    /// Whether the stone is locked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// Why the cutter chose it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// A free pose, when the stone has one (it replaces `pose`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_pose: Option<FreePoseDto>,
}

/// The rules the layouts of a plan were planned with, when they are not the rules the plan holds
/// now (`[axis.planned]`): the plan's rule and the rules of single designs, as in the `[axis]`
/// table. An empty table is a plan that was planned with the material's defaults.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PlannedRulesDto {
    /// The plan's rule setting at planning time, when it was not the untouched default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<RuleSettingDto>,
    /// The rule settings of single designs at planning time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub designs: Vec<RuleSettingDto>,
}

/// The `[axis]` table of a version 5 plan.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AxisDto {
    /// The plan's rule setting, when it is not the untouched default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<RuleSettingDto>,
    /// The rule settings of single designs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub designs: Vec<RuleSettingDto>,
    /// The stones the cutter locked or gave a free pose or a Reason (side table, see the module).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stones: Vec<StoneChoiceDto>,
    /// The rules the layouts were planned with, written only when they differ from the rules
    /// above, so the window can warn "Rules changed since planning" after the plan is reopened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planned: Option<PlannedRulesDto>,
}

impl AxisDto {
    /// Whether the table holds nothing (it is then not written).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.rule.is_none()
            && self.designs.is_empty()
            && self.stones.is_empty()
            && self.planned.is_none()
    }
}

// ---- the rough's orientation ------------------------------------------------------------------

/// The file form of a rough's orientation.
#[must_use]
pub const fn orientation_to_dto(orientation: &CrystalOrientation) -> OrientationDto {
    OrientationDto {
        tilt_deg: orientation.axis.tilt_deg(),
        azimuth_deg: orientation.axis.azimuth_deg(),
        second_turn_deg: orientation.second_turn_deg,
    }
}

/// The orientation of `dto`, checked: finite angles, a tilt in `[0, 90]`.
///
/// # Errors
///
/// A message naming the field at `path`.
pub fn orientation_from_dto(
    dto: &OrientationDto,
    path: &str,
) -> Result<CrystalOrientation, String> {
    if !dto.tilt_deg.is_finite() || !(0.0..=90.0).contains(&dto.tilt_deg) {
        return Err(format!(
            "{path}.tilt_deg {} must be between 0 and 90 degrees",
            dto.tilt_deg
        ));
    }
    if !dto.azimuth_deg.is_finite() {
        return Err(format!(
            "{path}.azimuth_deg {} is not a number",
            dto.azimuth_deg
        ));
    }
    if let Some(turn) = dto.second_turn_deg
        && !turn.is_finite()
    {
        return Err(format!("{path}.second_turn_deg {turn} is not a number"));
    }
    Ok(CrystalOrientation::from_angles(
        dto.tilt_deg,
        dto.azimuth_deg,
        dto.second_turn_deg,
    ))
}

// ---- rules ------------------------------------------------------------------------------------

const fn preset_to_dto(id: ColorPresetId) -> PresetDto {
    match id {
        ColorPresetId::BestBlueFaceUp => PresetDto::BestBlueFaceUp,
        ColorPresetId::VioletFaceUp => PresetDto::VioletFaceUp,
        ColorPresetId::StrongestColorChange => PresetDto::StrongestColorChange,
        ColorPresetId::BestGreenFaceUp => PresetDto::BestGreenFaceUp,
    }
}

const fn preset_from_dto(preset: PresetDto) -> ColorPresetId {
    match preset {
        PresetDto::BestBlueFaceUp => ColorPresetId::BestBlueFaceUp,
        PresetDto::VioletFaceUp => ColorPresetId::VioletFaceUp,
        PresetDto::StrongestColorChange => ColorPresetId::StrongestColorChange,
        PresetDto::BestGreenFaceUp => ColorPresetId::BestGreenFaceUp,
    }
}

/// The file form of a rule setting (`entry_id` for a design's).
#[must_use]
pub fn setting_to_dto(setting: &RuleSetting, entry_id: Option<i64>) -> RuleSettingDto {
    let mut dto = RuleSettingDto {
        entry_id,
        reason: setting.reason.clone(),
        ..RuleSettingDto::default()
    };
    match setting.choice {
        RuleChoice::Default => {}
        RuleChoice::Ignore => dto.choice = ChoiceDto::Ignore,
        RuleChoice::Set(rule) => {
            dto.choice = ChoiceDto::Set;
            dto.kind = Some(match rule.kind {
                OrientationRuleKind::None => KindDto::None,
                OrientationRuleKind::TableAcross => KindDto::TableAcross,
                OrientationRuleKind::InTablePlane => KindDto::InTablePlane,
                OrientationRuleKind::CustomAngle {
                    tilt_deg,
                    azimuth_deg,
                } => {
                    dto.tilt_deg = Some(tilt_deg);
                    dto.azimuth_deg = azimuth_deg;
                    KindDto::CustomAngle
                }
                OrientationRuleKind::FaceUp(id) => {
                    dto.preset = Some(preset_to_dto(id));
                    KindDto::FaceUp
                }
            });
            dto.tolerance_deg = Some(rule.tolerance_deg);
            match rule.strength {
                RuleStrength::Required => dto.strength = Some(StrengthDto::Required),
                RuleStrength::Preferred { weight_per_deg } => {
                    dto.strength = Some(StrengthDto::Preferred);
                    dto.weight_per_deg = Some(weight_per_deg);
                }
            }
        }
    }
    dto
}

fn finite_in(
    value: Option<f64>,
    low: f64,
    high: f64,
    name: &str,
    path: &str,
) -> Result<f64, String> {
    let Some(value) = value else {
        return Err(format!("{path}.{name} is missing"));
    };
    if value.is_finite() && (low..=high).contains(&value) {
        Ok(value)
    } else {
        Err(format!(
            "{path}.{name} {value} must be between {low} and {high}"
        ))
    }
}

fn check_reason(reason: &str, path: &str) -> Result<(), String> {
    if reason.chars().count() > MAX_REASON_CHARS {
        return Err(format!(
            "{path}.reason is longer than {MAX_REASON_CHARS} characters"
        ));
    }
    Ok(())
}

/// The rule setting of `dto`, checked field by field.
///
/// # Errors
///
/// A message naming the offending field at `path`.
pub fn setting_from_dto(dto: &RuleSettingDto, path: &str) -> Result<RuleSetting, String> {
    check_reason(&dto.reason, path)?;
    let choice = match dto.choice {
        ChoiceDto::Default => RuleChoice::Default,
        ChoiceDto::Ignore => RuleChoice::Ignore,
        ChoiceDto::Set => {
            let kind = match dto.kind {
                None => return Err(format!("{path}.kind is missing")),
                Some(KindDto::None) => OrientationRuleKind::None,
                Some(KindDto::TableAcross) => OrientationRuleKind::TableAcross,
                Some(KindDto::InTablePlane) => OrientationRuleKind::InTablePlane,
                Some(KindDto::CustomAngle) => OrientationRuleKind::CustomAngle {
                    tilt_deg: finite_in(dto.tilt_deg, 0.0, 90.0, "tilt_deg", path)?,
                    azimuth_deg: match dto.azimuth_deg {
                        None => None,
                        Some(azimuth) if azimuth.is_finite() => Some(azimuth),
                        Some(azimuth) => {
                            return Err(format!("{path}.azimuth_deg {azimuth} is not a number"));
                        }
                    },
                },
                Some(KindDto::FaceUp) => match dto.preset {
                    Some(preset) => OrientationRuleKind::FaceUp(preset_from_dto(preset)),
                    None => return Err(format!("{path}.preset is missing")),
                },
            };
            let tolerance_deg = finite_in(dto.tolerance_deg, 0.0, 90.0, "tolerance_deg", path)?;
            let strength = match dto.strength {
                None => return Err(format!("{path}.strength is missing")),
                Some(StrengthDto::Required) => RuleStrength::Required,
                Some(StrengthDto::Preferred) => RuleStrength::Preferred {
                    weight_per_deg: finite_in(
                        dto.weight_per_deg,
                        0.0,
                        100.0,
                        "weight_per_deg",
                        path,
                    )?,
                },
            };
            RuleChoice::Set(OrientationRule {
                kind,
                tolerance_deg,
                strength,
            })
        }
    };
    Ok(RuleSetting {
        choice,
        reason: dto.reason.clone(),
    })
}

// ---- stone choices ----------------------------------------------------------------------------

/// The file form of a free pose.
#[must_use]
pub const fn free_pose_to_dto(pose: &StonePose) -> FreePoseDto {
    FreePoseDto {
        center_mm: pose.center_mm,
        axes: pose.axes,
        mm_per_unit: pose.mm_per_unit,
    }
}

/// The free pose of `dto`: finite numbers and a positive scale.
///
/// # Errors
///
/// A message naming the problem at `path`.
pub fn free_pose_from_dto(dto: &FreePoseDto, path: &str) -> Result<StonePose, String> {
    let finite = dto.center_mm.iter().all(|v| v.is_finite())
        && dto.axes.iter().flatten().all(|v| v.is_finite());
    if !finite {
        return Err(format!("{path} holds a number that is not finite"));
    }
    if !dto.mm_per_unit.is_finite() || dto.mm_per_unit <= 0.0 {
        return Err(format!(
            "{path}.mm_per_unit {} must be positive",
            dto.mm_per_unit
        ));
    }
    Ok(StonePose {
        center_mm: dto.center_mm,
        axes: dto.axes,
        mm_per_unit: dto.mm_per_unit,
    })
}

/// Whether a choice needs the version 5 table: it has a free pose, a lock or a Reason (a choice of
/// a box pose alone is a version 4 `pose_choices` entry).
#[must_use]
pub const fn is_rich(choice: &StoneChoice) -> bool {
    choice.free_pose.is_some() || choice.locked || !choice.reason.is_empty()
}

/// Splits `stones` into the version 4 pose tables and the version 5 stone tables.
#[must_use]
pub fn split_stone_choices(
    stones: &BTreeMap<(u32, u32), StoneChoice>,
) -> (Vec<PoseChoiceDto>, Vec<StoneChoiceDto>) {
    let mut poses = Vec::new();
    let mut rich = Vec::new();
    for (&(layout, stone), choice) in stones {
        if is_rich(choice) {
            rich.push(StoneChoiceDto {
                layout,
                stone,
                pose: (choice.pose != 0).then_some(choice.pose),
                locked: choice.locked,
                reason: choice.reason.clone(),
                free_pose: choice.free_pose.as_ref().map(free_pose_to_dto),
            });
        } else if choice.pose != 0 {
            poses.push(PoseChoiceDto {
                layout,
                stone,
                pose: choice.pose,
            });
        }
    }
    (poses, rich)
}

/// Fails when a stone choice names a layout or stone `layouts` do not have, a pose of
/// `POSE_COUNT` or more, a free pose that is not usable, a Reason that is too long, or a stone a
/// second time (also across `poses`).
///
/// # Errors
///
/// A message naming the offending entry.
pub fn check_stone_choices(
    poses: &[PoseChoiceDto],
    rich: &[StoneChoiceDto],
    layouts: &[RoughLayout],
) -> Result<(), String> {
    let mut seen: BTreeSet<(u32, u32)> = poses.iter().map(|c| (c.layout, c.stone)).collect();
    for (i, choice) in rich.iter().enumerate() {
        let path = format!("axis.stones[{i}]");
        let stones = usize::try_from(choice.layout)
            .ok()
            .and_then(|index| layouts.get(index))
            .map(|layout| layout.stones.len())
            .ok_or_else(|| {
                format!(
                    "{path}.layout {} names no layout; the plan has {}",
                    choice.layout,
                    layouts.len()
                )
            })?;
        if !usize::try_from(choice.stone).is_ok_and(|index| index < stones) {
            return Err(format!(
                "{path}.stone {} names no stone of layouts[{}]; it has {stones}",
                choice.stone, choice.layout
            ));
        }
        if let Some(pose) = choice.pose.filter(|&pose| pose >= POSE_COUNT) {
            return Err(format!(
                "{path}.pose {pose} does not exist; a stone has {POSE_COUNT} poses"
            ));
        }
        check_reason(&choice.reason, &path)?;
        if let Some(free) = &choice.free_pose {
            free_pose_from_dto(free, &format!("{path}.free_pose"))?;
        }
        if !seen.insert((choice.layout, choice.stone)) {
            return Err(format!(
                "{path} chooses layouts[{}].stones[{}] a second time",
                choice.layout, choice.stone
            ));
        }
    }
    Ok(())
}

// ---- the whole table ---------------------------------------------------------------------------

/// The `[axis]` table the payload of a plan with `overrides` carries (the rules and Reasons; the
/// stones are in the side table), `None` when there is nothing to write.
#[cfg(test)]
#[must_use]
pub fn axis_to_dto(overrides: &PlanSettingsOverrides) -> Option<AxisDto> {
    axis_to_dto_planned(overrides, None)
}

/// The `[axis]` table of `overrides`, with the rules the layouts were planned with (`planned`) written as
/// `[axis.planned]` only when they differ from `overrides` in the plan's rule or the rules of its
/// designs, so a plan whose rules were not edited after planning is written as it always was.
#[must_use]
pub fn axis_to_dto_planned(
    overrides: &PlanSettingsOverrides,
    planned: Option<&PlanSettingsOverrides>,
) -> Option<AxisDto> {
    let (rule, designs) = rule_dtos(overrides);
    let planned = planned
        .filter(|planned| planned.plan != overrides.plan || planned.designs != overrides.designs)
        .map(|planned| {
            let (rule, designs) = rule_dtos(planned);
            PlannedRulesDto { rule, designs }
        });
    let axis = AxisDto {
        rule,
        designs,
        stones: Vec::new(),
        planned,
    };
    (!axis.is_empty()).then_some(axis)
}

/// The plan's rule and the rules of its designs as tables.
fn rule_dtos(overrides: &PlanSettingsOverrides) -> (Option<RuleSettingDto>, Vec<RuleSettingDto>) {
    let rule = (!overrides.plan.is_default()).then(|| setting_to_dto(&overrides.plan, None));
    let designs = overrides
        .designs
        .iter()
        .map(|(&entry_id, setting)| setting_to_dto(setting, Some(entry_id)))
        .collect();
    (rule, designs)
}

/// The rules a plan was planned with, when its `[axis.planned]` table says (the plan's rule and
/// the rules of its designs; no stone choices), checked like the rules above. `None` when the
/// file has no such table: the layouts were planned with the rules the plan holds.
///
/// # Errors
///
/// A message naming the offending field.
pub fn planned_from_dto(axis: Option<&AxisDto>) -> Result<Option<PlanSettingsOverrides>, String> {
    let Some(planned) = axis.and_then(|axis| axis.planned.as_ref()) else {
        return Ok(None);
    };
    let as_axis = AxisDto {
        rule: planned.rule.clone(),
        designs: planned.designs.clone(),
        stones: Vec::new(),
        planned: None,
    };
    overrides_from_dtos_at(Some(&as_axis), &[], "axis.planned").map(Some)
}

/// The settings a document holds: the rules and Reasons of `axis` and the stone choices of the
/// version 4 `pose_choices` and the version 5 `axis.stones`, converted and checked.
///
/// # Errors
///
/// A message naming the offending field.
pub fn overrides_from_dtos(
    axis: Option<&AxisDto>,
    pose_choices: &[PoseChoiceDto],
) -> Result<PlanSettingsOverrides, String> {
    overrides_from_dtos_at(axis, pose_choices, "axis")
}

/// [`overrides_from_dtos`] for a table that sits at `prefix` in the file (`axis`, or
/// `axis.planned`), so a message names the field where it is.
fn overrides_from_dtos_at(
    axis: Option<&AxisDto>,
    pose_choices: &[PoseChoiceDto],
    prefix: &str,
) -> Result<PlanSettingsOverrides, String> {
    let mut overrides = PlanSettingsOverrides::default();
    if let Some(axis) = axis {
        if let Some(rule) = &axis.rule {
            if rule.entry_id.is_some() {
                return Err(format!(
                    "{prefix}.rule names a design; the plan's rule has no entry_id"
                ));
            }
            overrides.set_plan(setting_from_dto(rule, &format!("{prefix}.rule"))?);
        }
        if axis.designs.len() > MAX_DESIGN_RULES {
            return Err(format!(
                "{prefix}.designs lists {} rules; a plan holds at most {MAX_DESIGN_RULES}",
                axis.designs.len()
            ));
        }
        for (i, dto) in axis.designs.iter().enumerate() {
            let path = format!("{prefix}.designs[{i}]");
            let Some(entry_id) = dto.entry_id else {
                return Err(format!("{path}.entry_id is missing"));
            };
            if overrides.designs.contains_key(&entry_id) {
                return Err(format!("{path} sets design {entry_id} a second time"));
            }
            overrides.set_design(entry_id, setting_from_dto(dto, &path)?);
        }
        for (i, dto) in axis.stones.iter().enumerate() {
            let path = format!("{prefix}.stones[{i}]");
            let free_pose = dto
                .free_pose
                .as_ref()
                .map(|free| free_pose_from_dto(free, &format!("{path}.free_pose")))
                .transpose()?;
            overrides.set_stone_choice(
                dto.layout,
                dto.stone,
                StoneChoice {
                    pose: dto.pose.unwrap_or(0),
                    free_pose,
                    locked: dto.locked,
                    reason: dto.reason.clone(),
                },
            );
        }
    }
    for choice in pose_choices {
        let kept = overrides
            .stone_choice(choice.layout, choice.stone)
            .cloned()
            .unwrap_or_default();
        overrides.set_stone_choice(
            choice.layout,
            choice.stone,
            StoneChoice {
                pose: choice.pose,
                ..kept
            },
        );
    }
    Ok(overrides)
}

/// Fails when a version 5 section sits in a document that declares an older version.
///
/// # Errors
///
/// A message naming the section.
pub fn check_axis_version(
    version: u32,
    has_orientation: bool,
    has_axis: bool,
) -> Result<(), String> {
    if version >= CRYSTAL_AXIS_SCHEMA_VERSION {
        return Ok(());
    }
    let section = if has_orientation {
        "rough.orientation"
    } else if has_axis {
        "axis"
    } else {
        return Ok(());
    };
    Err(format!(
        "{section} needs plan version {CRYSTAL_AXIS_SCHEMA_VERSION}; this plan declares version {version}."
    ))
}
