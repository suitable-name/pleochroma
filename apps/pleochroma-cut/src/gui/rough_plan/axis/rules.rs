//! The rule editor of the planner window: the plan's rule and the rule of one design.
//!
//! The rule is the way stones are oriented to the crystal axis ("Table Across", "In the Table
//! Plane", a custom angle, a color-named frame), how far a stone may be off (the tolerance) and how
//! hard the rule holds (Preferred, with a weight per degree, or Required). The plan's rule is the
//! material's default until the cutter changes it; a design may follow the plan, ignore the axis
//! or have a rule of its own. Each edit, with its optional Reason, is one undo step
//! (`Session::edit_overrides`).
//!
//! The pure half (the form and how a typed field changes a setting) is tested here; the window
//! half pushes the settings into the `RoughPlanModel` fields and reads the edits back.

use super::material;
use crate::{
    RoughPlanModel,
    gui::rough_plan::{
        editing,
        format::{design_name, group_order},
        host::{Host, on_idle_host},
        inputs::parse_with_unit,
    },
};
use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::{
    crystal_axis::{
        DEFAULT_TOLERANCE_DEG, DEFAULT_WEIGHT_PER_DEG, OrientationRule, OrientationRuleKind,
        RuleStrength, color_presets, default_rule,
    },
    rough_plan::orientation::{RuleChoice, RuleSetting},
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::rc::Rc;

/// The fields of the rule editor, as `RoughPlanModel.rule_edit` numbers them.
pub(super) const FIELD_CHOICE: i32 = 0;
/// The kind of rule (an index into the kind list).
pub(super) const FIELD_KIND: i32 = 1;
/// Preferred (`0`) or Required (`1`).
pub(super) const FIELD_HOLD: i32 = 2;
/// The tolerance, degrees.
pub(super) const FIELD_TOLERANCE: i32 = 3;
/// The weight of a Preferred rule, percent per degree.
pub(super) const FIELD_WEIGHT: i32 = 4;
/// The tilt of a custom angle, degrees.
pub(super) const FIELD_TILT: i32 = 5;
/// The azimuth of a custom angle, degrees (empty for none).
pub(super) const FIELD_AZIMUTH: i32 = 6;
/// The Reason.
pub(super) const FIELD_REASON: i32 = 7;

/// How many kinds of rule come before the color-named ones in the kind list.
const FIXED_KINDS: usize = 3;

/// The longest Reason a plan stores, in characters.
const MAX_REASON_CHARS: usize = 500;

/// The most percent per degree a Preferred rule charges.
const MAX_WEIGHT_PERCENT: f64 = 100.0;

/// What the rule editor holds: the choice (`0` follow the level above, `1` ignore the axis, `2` a
/// rule of its own), the kind, Preferred or Required, and the typed fields.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RuleForm {
    /// `0` follow, `1` ignore the axis, `2` own rule.
    pub(super) choice: i32,
    /// Index into the kind list.
    pub(super) kind: i32,
    /// `0` Preferred, `1` Required.
    pub(super) hold: i32,
    /// The tolerance text.
    pub(super) tolerance: String,
    /// The weight text (percent per degree).
    pub(super) weight: String,
    /// The custom angle's tilt text.
    pub(super) tilt: String,
    /// The custom angle's azimuth text.
    pub(super) azimuth: String,
    /// The Reason.
    pub(super) reason: String,
}

/// The rule an own setting starts from when nothing else gives one: the usual Table Across,
/// Preferred, 15 degrees.
const fn fallback_rule() -> OrientationRule {
    OrientationRule {
        kind: OrientationRuleKind::TableAcross,
        tolerance_deg: DEFAULT_TOLERANCE_DEG,
        strength: RuleStrength::Preferred {
            weight_per_deg: DEFAULT_WEIGHT_PER_DEG,
        },
    }
}

/// `value` as the text a field shows: at most two decimals, none for a whole number.
fn number(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    format!("{rounded}")
}

/// The names of the rule kinds the material offers, in the order of the kind list: the three plain
/// ones, then the material's color-named ones.
#[must_use]
pub(super) fn kind_names(material: &GemMaterial) -> Vec<String> {
    let mut names = vec![
        "Table Across".to_string(),
        "In the Table Plane".to_string(),
        "A Custom Angle".to_string(),
    ];
    names.extend(
        color_presets(material)
            .iter()
            .map(|preset| preset.id.label().to_string()),
    );
    names
}

/// The place of `kind` in the kind list of `material`.
fn kind_index(kind: OrientationRuleKind, material: &GemMaterial) -> usize {
    match kind {
        OrientationRuleKind::None | OrientationRuleKind::TableAcross => 0,
        OrientationRuleKind::InTablePlane => 1,
        OrientationRuleKind::CustomAngle { .. } => 2,
        OrientationRuleKind::FaceUp(id) => color_presets(material)
            .iter()
            .position(|preset| preset.id == id)
            .map_or(0, |position| position + FIXED_KINDS),
    }
}

/// The kind at place `index` of the kind list. A custom angle keeps the angle `previous` had when
/// it was one; `None` for a place the list does not have.
fn kind_at(
    index: usize,
    material: &GemMaterial,
    previous: OrientationRuleKind,
) -> Option<OrientationRuleKind> {
    match index {
        0 => Some(OrientationRuleKind::TableAcross),
        1 => Some(OrientationRuleKind::InTablePlane),
        2 => Some(match previous {
            OrientationRuleKind::CustomAngle { .. } => previous,
            _ => OrientationRuleKind::CustomAngle {
                tilt_deg: 45.0,
                azimuth_deg: None,
            },
        }),
        place => color_presets(material)
            .get(place - FIXED_KINDS)
            .map(|preset| OrientationRuleKind::FaceUp(preset.id)),
    }
}

/// The form of `setting`. `base` is the rule the level above gives (the material's default for
/// the plan, the plan's rule for a design); the form shows it while the setting follows it or
/// ignores the axis, so the fields always show a rule.
#[must_use]
pub(super) fn form_of(
    setting: &RuleSetting,
    material: &GemMaterial,
    base: Option<OrientationRule>,
) -> RuleForm {
    let (choice, shown) = match setting.choice {
        RuleChoice::Default => (0, base),
        RuleChoice::Ignore => (1, base),
        RuleChoice::Set(rule) => (2, Some(rule)),
    };
    let rule = shown.unwrap_or_else(fallback_rule);
    let (tilt, azimuth) = match rule.kind {
        OrientationRuleKind::CustomAngle {
            tilt_deg,
            azimuth_deg,
        } => (
            number(tilt_deg),
            azimuth_deg.map_or_else(String::new, number),
        ),
        _ => (String::new(), String::new()),
    };
    let (hold, weight) = match rule.strength {
        RuleStrength::Preferred { weight_per_deg } => (0, number(weight_per_deg)),
        RuleStrength::Required => (1, number(DEFAULT_WEIGHT_PER_DEG)),
    };
    RuleForm {
        choice,
        kind: i32::try_from(kind_index(rule.kind, material)).unwrap_or(0),
        hold,
        tolerance: number(rule.tolerance_deg),
        weight,
        tilt,
        azimuth,
        reason: setting.reason.clone(),
    }
}

/// Parses an angle field `text` that must lie in `low..=high` degrees.
fn angle(text: &str, label: &str, low: f64, high: f64) -> Result<f64, String> {
    let value = parse_with_unit(text, label, "degrees")?;
    if (low..=high).contains(&value) {
        Ok(value)
    } else {
        Err(format!("{label} must be between {low} and {high} degrees."))
    }
}

/// The setting after `text` was typed into `field`.
///
/// Editing any field but the choice and the Reason makes the setting a rule of its own, starting
/// from the rule it showed (`base`, or the plain default). Tilt and azimuth mean something only to
/// a custom angle, and the weight only to a Preferred rule: elsewhere they change nothing.
///
/// # Errors
///
/// The message for `text` that is not a valid value of the field.
pub(super) fn apply_field(
    setting: &RuleSetting,
    field: i32,
    text: &str,
    material: &GemMaterial,
    base: Option<OrientationRule>,
) -> Result<RuleSetting, String> {
    let mut next = setting.clone();
    if field == FIELD_REASON {
        next.reason = text.trim().chars().take(MAX_REASON_CHARS).collect();
        return Ok(next);
    }
    let mut rule = match setting.choice {
        RuleChoice::Set(rule) => rule,
        _ => base.unwrap_or_else(fallback_rule),
    };
    if field == FIELD_CHOICE {
        next.choice = match text.trim() {
            "0" => RuleChoice::Default,
            "1" => RuleChoice::Ignore,
            "2" => RuleChoice::Set(rule),
            _ => return Err("Pick one of the three rule choices.".to_string()),
        };
        return Ok(next);
    }
    match field {
        FIELD_KIND => {
            let index: usize = text
                .trim()
                .parse()
                .map_err(|_| "Pick a kind of rule from the list.".to_string())?;
            rule.kind = kind_at(index, material, rule.kind)
                .ok_or_else(|| "The material has no such rule.".to_string())?;
        }
        FIELD_HOLD => {
            rule.strength = match text.trim() {
                "0" => RuleStrength::Preferred {
                    weight_per_deg: match rule.strength {
                        RuleStrength::Preferred { weight_per_deg } => weight_per_deg,
                        RuleStrength::Required => DEFAULT_WEIGHT_PER_DEG,
                    },
                },
                "1" => RuleStrength::Required,
                _ => return Err("Pick Preferred or Required.".to_string()),
            };
        }
        FIELD_TOLERANCE => rule.tolerance_deg = angle(text, "Tolerance", 0.0, 90.0)?,
        FIELD_WEIGHT => {
            let RuleStrength::Preferred { .. } = rule.strength else {
                return Ok(next);
            };
            let weight = parse_with_unit(text, "Weight", "percent per degree")?;
            if !(0.0..=MAX_WEIGHT_PERCENT).contains(&weight) {
                return Err(format!(
                    "Weight must be between 0 and {MAX_WEIGHT_PERCENT} percent per degree."
                ));
            }
            rule.strength = RuleStrength::Preferred {
                weight_per_deg: weight,
            };
        }
        FIELD_TILT | FIELD_AZIMUTH => {
            let OrientationRuleKind::CustomAngle {
                tilt_deg,
                azimuth_deg,
            } = rule.kind
            else {
                return Ok(next);
            };
            rule.kind = if field == FIELD_TILT {
                OrientationRuleKind::CustomAngle {
                    tilt_deg: angle(text, "Tilt", 0.0, 90.0)?,
                    azimuth_deg,
                }
            } else {
                OrientationRuleKind::CustomAngle {
                    tilt_deg,
                    azimuth_deg: if text.trim().is_empty() {
                        None
                    } else {
                        Some(angle(text, "Azimuth", 0.0, 360.0)?)
                    },
                }
            };
        }
        _ => return Err("That field does not exist.".to_string()),
    }
    next.choice = RuleChoice::Set(rule);
    Ok(next)
}

/// The design the selected design row names: its entry id as planned and its title.
fn selected_design(host: &Host) -> Option<(i64, String)> {
    let model = host.window.global::<RoughPlanModel>();
    let result = usize::try_from(model.get_selected_result()).ok()?;
    let group = usize::try_from(model.get_selected_group()).ok()?;
    let session = host.session.borrow();
    let layout = session.run.layouts.get(result)?;
    let (entry_id, _) = *group_order(layout).get(group)?;
    Some((entry_id, design_name(&session.run.titles, entry_id)))
}

/// Registers the rule editor's callback.
pub(super) fn setup(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.on_rule_edit(|entry, field, text| {
        on_idle_host(|host| rule_edit(host, entry, field, &text));
    });
}

/// An edit of the rule editor of the plan (`entry < 0`) or of design `entry`.
fn rule_edit(host: &Rc<Host>, entry: i32, field: i32, text: &str) {
    let Some(name) = material::picked_name(host) else {
        return;
    };
    let Some(material) = material::known_material(host, &name) else {
        return;
    };
    let (setting, base) = {
        let session = host.session.borrow();
        if entry < 0 {
            (session.overrides.plan.clone(), default_rule(&material))
        } else {
            (
                session
                    .overrides
                    .designs
                    .get(&i64::from(entry))
                    .cloned()
                    .unwrap_or_default(),
                session
                    .overrides
                    .plan_rule(session.model.orientation.as_ref(), &material),
            )
        }
    };
    match apply_field(&setting, field, text, &material, base) {
        Ok(next) => {
            host.session.borrow_mut().edit_overrides(|overrides| {
                if entry < 0 {
                    overrides.set_plan(next);
                } else {
                    overrides.set_design(i64::from(entry), next);
                }
            });
            host.session.borrow_mut().axis.note = None;
        }
        Err(message) => {
            host.session.borrow_mut().axis.note = Some((message, 3));
        }
    }
    editing::push_history_flags(host);
    super::sync(host);
}

/// Shows `form` in the plan's rule editor.
fn push_plan_form(model: &RoughPlanModel<'_>, form: &RuleForm) {
    model.set_plan_rule_choice(form.choice);
    model.set_plan_rule_kind(form.kind);
    model.set_plan_rule_hold(form.hold);
    model.set_plan_rule_tolerance(form.tolerance.as_str().into());
    model.set_plan_rule_weight(form.weight.as_str().into());
    model.set_plan_rule_tilt(form.tilt.as_str().into());
    model.set_plan_rule_azimuth(form.azimuth.as_str().into());
    model.set_plan_rule_reason(form.reason.as_str().into());
}

/// Shows `form` in the design's rule editor.
fn push_design_form(model: &RoughPlanModel<'_>, form: &RuleForm) {
    model.set_design_rule_choice(form.choice);
    model.set_design_rule_kind(form.kind);
    model.set_design_rule_hold(form.hold);
    model.set_design_rule_tolerance(form.tolerance.as_str().into());
    model.set_design_rule_weight(form.weight.as_str().into());
    model.set_design_rule_tilt(form.tilt.as_str().into());
    model.set_design_rule_azimuth(form.azimuth.as_str().into());
    model.set_design_rule_reason(form.reason.as_str().into());
}

/// The visible line of the rules: what applies to the stones now, or why nothing does.
#[must_use]
pub(super) fn rule_line_text(line: Option<String>, axis_set: bool) -> String {
    if !axis_set {
        return String::new();
    }
    line.unwrap_or_else(|| "Stones are not oriented to the crystal axis.".to_string())
}

/// Mirrors the plan's rule, the rule of the selected design, the kind list and the visible line
/// into the window.
pub(super) fn push(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    let known = material::picked_name(host).and_then(|name| material::known_material(host, &name));
    let Some(material) = known else {
        model.set_rule_line("".into());
        model.set_design_rule_entry(-1);
        return;
    };
    let design = selected_design(host);
    let (overrides, orientation) = {
        let session = host.session.borrow();
        (session.overrides.clone(), session.model.orientation)
    };
    let names: Vec<SharedString> = kind_names(&material)
        .into_iter()
        .map(SharedString::from)
        .collect();
    model.set_rule_kind_names(ModelRc::new(VecModel::from(names)));

    let plan_form = form_of(&overrides.plan, &material, default_rule(&material));
    push_plan_form(&model, &plan_form);
    let line = overrides
        .rule_line(orientation.as_ref(), &material)
        .map(|line| line.text);
    model.set_rule_line(rule_line_text(line, orientation.is_some()).into());

    match design {
        Some((entry_id, title)) if orientation.is_some() => {
            let setting = overrides
                .designs
                .get(&entry_id)
                .cloned()
                .unwrap_or_default();
            let base = overrides.plan_rule(orientation.as_ref(), &material);
            push_design_form(&model, &form_of(&setting, &material, base));
            model.set_design_rule_title(format!("Rule for {title}").into());
            model.set_design_rule_entry(i32::try_from(entry_id).unwrap_or(-1));
        }
        _ => model.set_design_rule_entry(-1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sapphire() -> GemMaterial {
        GemMaterial::by_name("Sapphire").expect("a built-in material")
    }

    fn tanzanite() -> GemMaterial {
        GemMaterial::by_name("Tanzanite").expect("a built-in material")
    }

    #[test]
    fn the_untouched_plan_setting_shows_the_materials_default_rule() {
        let material = sapphire();
        let form = form_of(&RuleSetting::default(), &material, default_rule(&material));
        assert_eq!(form.choice, 0);
        assert_eq!(form.kind, 0, "Table Across");
        assert_eq!(form.hold, 0, "Preferred");
        assert_eq!(form.tolerance, "15");
        assert_eq!(form.weight, "0.5");
        assert_eq!(form.reason.len(), 0);
    }

    #[test]
    fn the_kind_list_has_three_plain_kinds_and_the_materials_color_presets() {
        assert_eq!(
            kind_names(&sapphire()),
            ["Table Across", "In the Table Plane", "A Custom Angle"]
        );
        let names = kind_names(&tanzanite());
        assert!(
            names.len() > 3,
            "tanzanite has color-named rules: {names:?}"
        );
        assert_eq!(names[3], "Best Blue Face-Up");
    }

    #[test]
    fn typing_a_tolerance_makes_an_own_rule_that_keeps_the_rest() {
        let material = sapphire();
        let base = default_rule(&material);
        let next = apply_field(
            &RuleSetting::default(),
            FIELD_TOLERANCE,
            "20",
            &material,
            base,
        )
        .expect("a valid tolerance");
        let RuleChoice::Set(rule) = next.choice else {
            panic!("an own rule");
        };
        assert_eq!(rule.tolerance_deg, 20.0);
        assert_eq!(rule.kind, OrientationRuleKind::TableAcross);
        assert_eq!(
            rule.strength,
            RuleStrength::Preferred {
                weight_per_deg: DEFAULT_WEIGHT_PER_DEG
            }
        );
    }

    #[test]
    fn required_and_back_keeps_the_weight() {
        let material = sapphire();
        let required = apply_field(
            &RuleSetting::default(),
            FIELD_HOLD,
            "1",
            &material,
            default_rule(&material),
        )
        .expect("required");
        let RuleChoice::Set(rule) = required.choice else {
            panic!("an own rule");
        };
        assert_eq!(rule.strength, RuleStrength::Required);
        // The weight means nothing to a Required rule: the field changes nothing.
        let same = apply_field(&required, FIELD_WEIGHT, "2", &material, None).expect("ignored");
        assert_eq!(same, required);
    }

    #[test]
    fn the_custom_angle_fields_work_only_on_a_custom_angle() {
        let material = sapphire();
        let across = apply_field(&RuleSetting::default(), FIELD_TILT, "30", &material, None)
            .expect("ignored on Table Across");
        assert_eq!(across.choice, RuleChoice::Default, "nothing changed");

        let custom = apply_field(&RuleSetting::default(), FIELD_KIND, "2", &material, None)
            .expect("a custom angle");
        let tilted = apply_field(&custom, FIELD_TILT, "30", &material, None).expect("30 degrees");
        let turned = apply_field(&tilted, FIELD_AZIMUTH, "120", &material, None).expect("120");
        let RuleChoice::Set(rule) = turned.choice else {
            panic!("an own rule");
        };
        assert_eq!(
            rule.kind,
            OrientationRuleKind::CustomAngle {
                tilt_deg: 30.0,
                azimuth_deg: Some(120.0)
            }
        );
        let cleared = apply_field(&turned, FIELD_AZIMUTH, " ", &material, None).expect("empty");
        let RuleChoice::Set(rule) = cleared.choice else {
            panic!("an own rule");
        };
        assert_eq!(
            rule.kind,
            OrientationRuleKind::CustomAngle {
                tilt_deg: 30.0,
                azimuth_deg: None
            }
        );
    }

    #[test]
    fn the_choice_and_the_reason_edit_only_themselves() {
        let material = sapphire();
        let ignored = apply_field(&RuleSetting::default(), FIELD_CHOICE, "1", &material, None)
            .expect("ignore");
        assert_eq!(ignored.choice, RuleChoice::Ignore);
        let reasoned = apply_field(&ignored, FIELD_REASON, "  cut for color  ", &material, None)
            .expect("a reason");
        assert_eq!(reasoned.choice, RuleChoice::Ignore);
        assert_eq!(reasoned.reason, "cut for color");
        assert!(apply_field(&ignored, FIELD_CHOICE, "7", &material, None).is_err());
    }

    #[test]
    fn invalid_values_are_refused_with_a_message_naming_the_field() {
        let material = sapphire();
        let setting = RuleSetting::default();
        for (field, text, name) in [
            (FIELD_TOLERANCE, "ninety", "Tolerance"),
            (FIELD_TOLERANCE, "120", "Tolerance"),
            (FIELD_WEIGHT, "-1", "Weight"),
            (FIELD_KIND, "9", "material"),
        ] {
            let message =
                apply_field(&setting, field, text, &material, None).expect_err("not a valid value");
            assert!(message.contains(name), "{message}");
        }
    }

    #[test]
    fn a_design_that_follows_the_plan_shows_the_plans_rule() {
        let material = sapphire();
        let plan = OrientationRule {
            kind: OrientationRuleKind::InTablePlane,
            tolerance_deg: 10.0,
            strength: RuleStrength::Required,
        };
        let form = form_of(&RuleSetting::default(), &material, Some(plan));
        assert_eq!((form.choice, form.kind, form.hold), (0, 1, 1));
        assert_eq!(form.tolerance, "10");
    }

    #[test]
    fn the_visible_line_is_empty_for_an_unknown_axis_and_says_so_when_no_rule_applies() {
        assert_eq!(rule_line_text(Some("x".to_string()), false), "");
        assert_eq!(rule_line_text(Some("x".to_string()), true), "x");
        assert!(rule_line_text(None, true).contains("not oriented"));
    }
}
