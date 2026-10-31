//! The Orientation block of the cutting sheet: how the rough must sit for the stone's crystal
//! to lie as the design says.
//!
//! [`Design::sheet_orientation`] builds it, only for an anisotropic material whose Crystal Axis
//! is SET: the design's own setting (`[render_effects]`), else the stored default of a custom
//! material. An isotropic material, and a stone whose axis is not set, give `None`, and a sheet
//! without a block is byte for byte the sheet it always was.
//!
//! The block says where the axis runs (tilt from the table normal, azimuth, the azimuth also as
//! the index of the dop's index wheel: the facet index of the same direction,
//! `azimuth / 360 * teeth - reference`, see `StandardGemCuts::index_to_azimuth`), the turn of the
//! Second Axis of a biaxial stone, the sentence "Orient the rough so the crystal axis (c-axis)
//! runs ..." (shared with the phone's Cutting Mode, `orientation_sentence`), and whether the
//! stone follows the orientation rule of its material. A Reason for an override goes with it.

use std::fmt::Write as _;

use glam::{DVec3, Vec3};
use pleochroma::optics::{
    birefringence::alpha_axis_from_turn,
    materials::{GemMaterial, OpticalCharacter},
};

use super::sheet::format_sheet_index;
use crate::{
    crystal_axis::{
        CrystalAxis, CrystalOrientation, OrientationRule, default_rule,
        orientation_sentence_with_index, stone_orientation,
    },
    design::Design,
};

/// Tilts within this many degrees of 0 have no azimuth to speak of (the axis runs straight
/// through the table), as in `orientation_sentence`.
const NO_AZIMUTH_DEG: f64 = 0.5;

/// How the stone stands against the rule of its material.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetRuleCheck {
    /// The rule's name ("Table Across", "In the Table Plane", "Best Blue Face-Up").
    pub rule_name: String,
    /// Where the rule comes from: "Sapphire default" for the material's own rule, or the plan's
    /// own wording for a stone that came from a plan.
    pub source: String,
    /// The angle within which the stone counts as following the rule, degrees.
    pub tolerance_deg: f64,
    /// How far the stone is from the rule, degrees.
    pub deviation_deg: f64,
    /// Whether the stone follows the rule (`deviation_deg` within `tolerance_deg`).
    pub followed: bool,
}

impl SheetRuleCheck {
    /// The check of `orientation` against `rule`: `None` when the rule says nothing about the
    /// stone (no rule, or a color-named preset the material does not have).
    #[must_use]
    pub fn of(
        rule: &OrientationRule,
        source: &str,
        material: &GemMaterial,
        orientation: &CrystalOrientation,
    ) -> Option<Self> {
        let deviation_deg = rule.deviation_deg(material, orientation)?;
        Some(Self {
            rule_name: rule.name(),
            source: source.to_owned(),
            tolerance_deg: rule.tolerance_deg,
            deviation_deg,
            followed: deviation_deg <= rule.tolerance_deg + 1e-9,
        })
    }

    /// `Table Across (Sapphire default, within 15 deg): followed`, or `... : 20 deg off`.
    #[must_use]
    pub fn text(&self) -> String {
        let verdict = if self.followed {
            "followed".to_owned()
        } else {
            format!("{:.0}\u{b0} off", self.deviation_deg)
        };
        format!(
            "{} ({}, within {:.0}\u{b0}): {verdict}",
            self.rule_name, self.source, self.tolerance_deg
        )
    }
}

/// What the Orientation block of a cutting sheet says.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetOrientation {
    /// The stone's crystal orientation in its own (model) frame.
    pub orientation: CrystalOrientation,
    /// Whether the material is biaxial (then the Second Axis is part of the block).
    pub biaxial: bool,
    /// The azimuth of the axis as an index of the design's index gear, in `[0, teeth)`; `None`
    /// when the axis runs through the table (no azimuth).
    pub azimuth_index: Option<f64>,
    /// The design's index gear, teeth.
    pub gear_teeth: u32,
    /// The check against the orientation rule, when the material has one.
    pub rule: Option<SheetRuleCheck>,
    /// The Reason of an override, as the cutter wrote it; `None` for no override.
    pub reason: Option<String>,
}

impl SheetOrientation {
    /// The index of the azimuth `azimuth_deg` on a gear of `teeth` with `reference` (teeth), in
    /// `[0, teeth)`. `None` for a gear without teeth.
    #[must_use]
    pub fn index_of_azimuth(azimuth_deg: f64, teeth: u32, reference: f64) -> Option<f64> {
        if teeth == 0 {
            return None;
        }
        let teeth = f64::from(teeth);
        let index = (azimuth_deg / 360.0)
            .mul_add(teeth, -reference)
            .rem_euclid(teeth);
        // A rounding that lands on a full turn is index 0.
        Some(if index >= teeth - 1e-9 { 0.0 } else { index })
    }

    /// The stone's axis seen by a cutter with the table up, built for `orientation` on a gear of
    /// `teeth` with `reference`. The rule check and the Reason are added by the caller.
    #[must_use]
    pub fn new(orientation: CrystalOrientation, biaxial: bool, teeth: u32, reference: f64) -> Self {
        let azimuth_index = (orientation.axis.tilt_deg() > NO_AZIMUTH_DEG)
            .then(|| Self::index_of_azimuth(orientation.axis.azimuth_deg(), teeth, reference))
            .flatten();
        Self {
            orientation,
            biaxial,
            azimuth_index,
            gear_teeth: teeth,
            rule: None,
            reason: None,
        }
    }

    /// This block with `rule` as its rule check.
    #[must_use]
    pub fn with_rule(mut self, rule: Option<SheetRuleCheck>) -> Self {
        self.rule = rule;
        self
    }

    /// This block with the Reason of an override (blank text is no Reason).
    #[must_use]
    pub fn with_reason(mut self, reason: Option<&str>) -> Self {
        self.reason = reason
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned);
        self
    }

    /// The Second Axis turn that is printed: only a biaxial stone with one set.
    #[must_use]
    pub fn second_turn_deg(&self) -> Option<f64> {
        self.orientation.second_turn_deg.filter(|_| self.biaxial)
    }

    /// The axis as a sentence: `Orient the rough so the crystal axis (c-axis) leans 40 deg from
    /// the table normal, toward azimuth 120 deg (index 32).` The azimuth of a leaning or flat axis
    /// carries its index; a biaxial stone with a Second Axis adds its turn.
    #[must_use]
    pub fn sentence(&self) -> String {
        // The very words of the phone's Cutting Mode, plus the index of the azimuth.
        let shown = CrystalOrientation {
            second_turn_deg: self.second_turn_deg(),
            ..self.orientation
        };
        let index = self.azimuth_index.map(format_sheet_index);
        orientation_sentence_with_index(&shown, index.as_deref())
    }

    /// The lines the sheet prints, in order: the axis figures, the Second Axis, the sentence, the
    /// rule check and the Reason (the last three only when there is something to say).
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        let axis = &self.orientation.axis;
        let mut figures = format!(
            "Crystal axis: {:.1}\u{b0} from the table normal",
            axis.tilt_deg()
        );
        if let Some(index) = self.azimuth_index {
            let _ = write!(
                figures,
                ", azimuth {:.1}\u{b0} (index {} of {})",
                axis.azimuth_deg(),
                format_sheet_index(index),
                self.gear_teeth
            );
        }
        let mut lines = vec!["Orientation".to_owned(), figures];
        if let Some(turn) = self.second_turn_deg() {
            lines.push(format!(
                "Second axis: turned {turn:.1}\u{b0} about the crystal axis"
            ));
        }
        lines.push(self.sentence());
        if let Some(rule) = &self.rule {
            lines.push(format!("Orientation rule: {}", rule.text()));
        }
        if let Some(reason) = &self.reason {
            lines.push(format!("Reason: {reason}"));
        }
        lines
    }
}

/// The turn, degrees in `[0, 180)`, that puts alpha at `alpha` for a stone whose Crystal Axis
/// is `gamma`, measured from the material's own completion (`alpha_axis_from_turn(gamma, 0)`).
fn turn_of_alpha(gamma: Vec3, alpha: Vec3) -> f64 {
    let axis = CrystalAxis::from_direction(gamma.as_dvec3()).direction();
    let reference: DVec3 = alpha_axis_from_turn(axis.as_vec3(), 0.0).as_dvec3();
    let alpha = alpha.as_dvec3();
    let turn = axis
        .dot(reference.cross(alpha))
        .atan2(reference.dot(alpha))
        .to_degrees()
        .rem_euclid(180.0);
    if turn >= 180.0 - 1e-9 { 0.0 } else { turn }
}

/// The default orientation a custom material carries (its axis, baked into the material by
/// `with_default_orientation`), or `None` while it has none: a material whose Crystal Axis is the
/// table normal and that holds no Second Axis is the plain material.
fn baked_default(material: &GemMaterial) -> Option<CrystalOrientation> {
    let through_table = material.c_axis.cross(Vec3::Y).length() < 1e-5;
    if through_table && material.alpha_axis.is_none() {
        return None;
    }
    let turn = material
        .alpha_axis
        .filter(|_| material.biaxial_delta_beta_alpha.is_some())
        .map(|alpha| turn_of_alpha(material.c_axis, alpha));
    let axis = CrystalAxis::from_direction(material.c_axis.as_dvec3());
    Some(CrystalOrientation::from_angles(
        axis.tilt_deg(),
        axis.azimuth_deg(),
        turn,
    ))
}

impl Design {
    /// The Orientation block of this design's cutting sheet, or `None` for an isotropic
    /// material, a material the sheet cannot resolve, and a stone whose Crystal Axis is not set
    /// (neither by the design's `[render_effects]` nor by a custom material's stored default).
    ///
    /// `custom` is the caller's resolved catalogue, as for [`Self::cutting_sheet_with`]. The rule
    /// check is the rule the Rough Planner's hand-off stored on the design
    /// ([`Design::crystal_axis_plan`], with its Reason), else the material's own default rule.
    #[must_use]
    pub fn sheet_orientation(&self, custom: &[GemMaterial]) -> Option<SheetOrientation> {
        self.sheet_orientation_with_default(custom, None)
    }

    /// [`Self::sheet_orientation`] with the stored default orientation of the design's material
    /// supplied by the caller (`LibraryMaterial::default_orientation` of its library row).
    ///
    /// A caller that holds the row passes it here, so a custom material whose cutter explicitly
    /// set "tilt 0" still prints its block; the baked axes of `custom` cannot tell that from a
    /// plain material. With `None` the default is inferred from the baked axes, as
    /// [`Self::sheet_orientation`] always did.
    #[must_use]
    pub fn sheet_orientation_with_default(
        &self,
        custom: &[GemMaterial],
        stored_default: Option<CrystalOrientation>,
    ) -> Option<SheetOrientation> {
        let name = self.material.name.as_deref()?;
        let from_catalogue = custom
            .iter()
            .find(|gem| gem.name.eq_ignore_ascii_case(name));
        let material = from_catalogue.cloned().or_else(|| {
            GemMaterial::by_name(name).filter(|gem| gem.name.eq_ignore_ascii_case(name))
        })?;
        if material.optical_character == OpticalCharacter::Isotropic {
            return None;
        }
        let default = stored_default.or_else(|| from_catalogue.and_then(baked_default));
        let orientation = stone_orientation(&self.render_effects, default)?;
        // A stone handed over by the Rough Planner carries the rule it was planned under (and the
        // Reason of an override); any other design is checked against the material's default.
        let (rule, reason) = self.crystal_axis_plan.as_ref().map_or_else(
            || {
                (
                    default_rule(&material).and_then(|rule| {
                        SheetRuleCheck::of(
                            &rule,
                            &format!("{} default", material.name),
                            &material,
                            &orientation,
                        )
                    }),
                    None,
                )
            },
            |plan| {
                (
                    plan.rule.and_then(|rule| {
                        SheetRuleCheck::of(
                            &rule,
                            &plan.source_text(&material.name),
                            &material,
                            &orientation,
                        )
                    }),
                    plan.reason_text(),
                )
            },
        );
        Some(
            SheetOrientation::new(
                orientation,
                material.biaxial_delta_beta_alpha.is_some(),
                self.meta.gear_teeth_abs(),
                self.meta.gear_reference_angle,
            )
            .with_rule(rule)
            .with_reason(reason),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        design::{ConstraintTier, ScheduleMeta, StoneRenderEffects},
        material::MaterialSelection,
        preform::PreformSpec,
    };

    fn plain_design() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    fn design_with(name: &str, effects: StoneRenderEffects) -> Design {
        let mut design = plain_design();
        design.material = MaterialSelection {
            name: Some(name.to_owned()),
            specific_gravity_override: None,
            refractive_index_override: None,
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
            color_pattern_override: None,
            color_variant_override: None,
        };
        design.render_effects = effects;
        design
    }

    fn axis_on(tilt: f64, azimuth: f64, second: Option<f64>) -> StoneRenderEffects {
        StoneRenderEffects {
            c_axis_override_enabled: true,
            c_axis_tilt_deg: tilt,
            c_axis_azimuth_deg: azimuth,
            c_axis_second_azimuth_deg: second,
            ..StoneRenderEffects::default()
        }
    }

    #[test]
    fn the_index_of_an_azimuth_follows_the_gear() {
        // 96 teeth: 90 degrees is index 24, a half turn index 48; a reference shifts it.
        let index = |azimuth, reference| SheetOrientation::index_of_azimuth(azimuth, 96, reference);
        assert_eq!(index(0.0, 0.0), Some(0.0));
        assert_eq!(index(90.0, 0.0), Some(24.0));
        assert_eq!(index(180.0, 0.0), Some(48.0));
        assert_eq!(index(90.0, 4.0), Some(20.0));
        // Before index 0 wraps around the wheel.
        assert_eq!(index(0.0, 4.0), Some(92.0));
        assert_eq!(SheetOrientation::index_of_azimuth(90.0, 0, 0.0), None);
    }

    #[test]
    fn sapphire_through_the_table_reads_straight_through() {
        let design = design_with("Sapphire", axis_on(0.0, 0.0, None));
        let block = design.sheet_orientation(&[]).expect("an anisotropic stone");
        assert_eq!(block.azimuth_index, None, "a vertical axis has no azimuth");
        assert_eq!(
            block.sentence(),
            "Orient the rough so the crystal axis (c-axis) runs straight through the table."
        );
        let lines = block.lines();
        assert_eq!(lines[0], "Orientation");
        assert_eq!(lines[1], "Crystal axis: 0.0\u{b0} from the table normal");
        assert_eq!(
            lines.last().map(String::as_str),
            Some("Orientation rule: Table Across (Sapphire default, within 15\u{b0}): followed")
        );
    }

    #[test]
    fn a_sapphire_cut_across_says_how_far_off_it_is_and_where_the_index_lies() {
        let design = design_with("Sapphire", axis_on(40.0, 90.0, None));
        let block = design.sheet_orientation(&[]).expect("an anisotropic stone");
        assert_eq!(block.azimuth_index, Some(24.0));
        assert_eq!(
            block.sentence(),
            "Orient the rough so the crystal axis (c-axis) leans 40\u{b0} from the table \
             normal, toward azimuth 90\u{b0} (index 24)."
        );
        assert_eq!(
            block.lines().last().map(String::as_str),
            Some(
                "Orientation rule: Table Across (Sapphire default, within 15\u{b0}): 40\u{b0} off"
            )
        );
    }

    #[test]
    fn a_turned_tanzanite_names_its_second_axis() {
        let design = design_with("Tanzanite", axis_on(90.0, 0.0, Some(37.0)));
        let block = design.sheet_orientation(&[]).expect("an anisotropic stone");
        assert!(block.biaxial);
        assert_eq!(block.second_turn_deg(), Some(37.0));
        let lines = block.lines();
        assert!(
            lines.contains(&"Second axis: turned 37.0\u{b0} about the crystal axis".to_owned()),
            "{lines:?}"
        );
        assert!(
            block
                .sentence()
                .ends_with("(index 00). The second axis is turned 37\u{b0} about it.")
        );
        // The rule is the color-named one, so a rule line is printed.
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("Orientation rule: "))
        );
    }

    #[test]
    fn a_stored_default_of_tilt_zero_prints_a_block_the_baked_axes_cannot_show() {
        let mut custom = GemMaterial::by_name("Sapphire").expect("a built-in material");
        custom.name = "My Blue Crystal".to_owned();
        let design = design_with("My Blue Crystal", StoneRenderEffects::default());
        let catalogue = [custom];
        // The baked axes of a plain material and of a "set, tilt 0" one are the same.
        assert!(design.sheet_orientation(&catalogue).is_none());
        let stored = CrystalOrientation::from_angles(0.0, 0.0, None);
        let block = design
            .sheet_orientation_with_default(&catalogue, Some(stored))
            .expect("a stored default is an axis that is set");
        assert_eq!(block.orientation, stored);
        // Nothing stored: the old inference.
        assert!(
            design
                .sheet_orientation_with_default(&catalogue, None)
                .is_none()
        );
    }

    #[test]
    fn a_uniaxial_stone_never_prints_a_second_axis() {
        let design = design_with("Sapphire", axis_on(90.0, 0.0, Some(37.0)));
        let block = design.sheet_orientation(&[]).expect("an anisotropic stone");
        assert_eq!(block.second_turn_deg(), None);
        assert!(!block.sentence().contains("second axis"));
    }

    #[test]
    fn an_isotropic_stone_or_an_unset_axis_has_no_block() {
        // Diamond is isotropic: the axis is hidden, nothing is printed.
        assert!(
            design_with("Diamond", axis_on(30.0, 10.0, None))
                .sheet_orientation(&[])
                .is_none()
        );
        // No axis set on the design: the built-in's own axis, untouched.
        assert!(
            design_with("Sapphire", StoneRenderEffects::default())
                .sheet_orientation(&[])
                .is_none()
        );
        // No material at all.
        assert!(
            plain_design().sheet_orientation(&[]).is_none(),
            "a design without a material"
        );
    }

    #[test]
    fn the_sheet_text_gains_the_block_only_when_the_axis_is_set() {
        let plain = design_with("Sapphire", StoneRenderEffects::default());
        let solved = plain.solve().expect("the standard round brilliant solves");
        let without = plain.cutting_sheet(&solved);
        assert!(without.orientation.is_none());
        assert!(!without.to_text().contains("Crystal axis"));

        let set = design_with("Sapphire", axis_on(0.0, 0.0, None));
        let with = set.cutting_sheet(&solved);
        let block = with.orientation.as_ref().expect("an axis is set");
        assert_eq!(
            with.header.len(),
            without.header.len() + block.lines().len()
        );
        assert!(
            with.to_text()
                .contains("Orient the rough so the crystal axis")
        );
        // The rows are the same either way.
        assert_eq!(with.rows, without.rows);
    }

    #[test]
    fn a_stored_plan_rule_and_reason_replace_the_material_default() {
        use crate::crystal_axis::{
            CrystalAxisPlan, OrientationRule, OrientationRuleKind, PlanRuleOrigin, RuleStrength,
        };
        let mut design = design_with("Sapphire", axis_on(40.0, 90.0, None));
        let plain = design.sheet_orientation(&[]).expect("a block");
        assert_eq!(plain.reason, None);
        assert!(
            plain
                .rule
                .as_ref()
                .is_some_and(|rule| rule.source == "Sapphire default")
        );

        design.crystal_axis_plan = Some(CrystalAxisPlan {
            rule: Some(OrientationRule {
                kind: OrientationRuleKind::InTablePlane,
                tolerance_deg: 20.0,
                strength: RuleStrength::Required,
            }),
            origin: PlanRuleOrigin::Design,
            reason: "keeps the best color".to_owned(),
        });
        let planned = design.sheet_orientation(&[]).expect("a block");
        let rule = planned.rule.expect("a rule check");
        assert_eq!(rule.source, "design");
        assert_eq!(rule.rule_name, "In the Table Plane");
        assert!((rule.tolerance_deg - 20.0).abs() < 1e-12);
        assert_eq!(planned.reason.as_deref(), Some("keeps the best color"));
        // The axis figures are the design's own either way.
        assert_eq!(planned.orientation, plain.orientation);
    }

    #[test]
    fn a_plan_that_ignores_the_axis_prints_no_rule_line() {
        use crate::crystal_axis::{CrystalAxisPlan, PlanRuleOrigin};
        let mut design = design_with("Sapphire", axis_on(40.0, 90.0, None));
        design.crystal_axis_plan = Some(CrystalAxisPlan {
            rule: None,
            origin: PlanRuleOrigin::Plan,
            reason: "cut for yield".to_owned(),
        });
        let block = design.sheet_orientation(&[]).expect("a block");
        assert!(block.rule.is_none());
        assert_eq!(
            block.lines().last().map(String::as_str),
            Some("Reason: cut for yield")
        );
    }

    #[test]
    fn a_reason_is_kept_and_a_blank_one_is_not() {
        let design = design_with("Sapphire", axis_on(40.0, 90.0, None));
        let block = design.sheet_orientation(&[]).expect("a block");
        let with = block.clone().with_reason(Some("  cut for the window  "));
        assert_eq!(with.reason.as_deref(), Some("cut for the window"));
        assert_eq!(
            with.lines().last().map(String::as_str),
            Some("Reason: cut for the window")
        );
        assert_eq!(block.with_reason(Some("   ")).reason, None);
    }
}
