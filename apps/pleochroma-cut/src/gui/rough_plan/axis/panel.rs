//! The Crystal Axis panel of the rough: Unknown or Set, the angles, and the ways to find the
//! axis (Align to Face, Align to Edge, Fit to Crystal Length, From Color Zones).
//!
//! The axis is part of the rough model (`RoughModel::orientation`), so every change is one step of
//! the shape undo stack like any other edit of the rough. A drag on the axis in the 3D view changes
//! it live and counts as one step when the pointer goes up (`view::axis_gizmo`).
//!
//! The pure half (how a typed field changes the orientation, the notes, the warning that the axis
//! or the rules changed since planning) is tested here; the window half reads and writes the
//! session and the `RoughPlanModel` fields.

use super::{material, state::PickMode};
use crate::{
    RoughPlanModel,
    gui::{
        rough_plan::{
            editing,
            host::{Host, on_idle_host},
            inputs::parse_with_unit,
            view, zoning_hooks,
        },
        tutorial_events::raise_weak,
    },
};
use pleochroma::optics::materials::{GemMaterial, OpticalCharacter};
use pleochroma_cut_core::{
    crystal_axis::{CrystalOrientation, orientation_sentence},
    rough_plan::{
        RoughModel,
        orientation::{
            PlanSettingsOverrides, fit_to_crystal_length_of, orientation_from_color_zones,
        },
    },
};
use pleochroma_editor::guide::viewing_events::ROUGH_AXIS_SET;
use slint::ComponentHandle;
use std::rc::Rc;

/// The fields of the axis panel, as `RoughPlanModel.axis_edit` numbers them.
const FIELD_TILT: i32 = 0;
/// The azimuth field.
const FIELD_AZIMUTH: i32 = 1;
/// The Second Axis field.
const FIELD_SECOND: i32 = 2;

/// How clearly a longest direction must stand out for the fit to call it good, `0..=1`.
const GOOD_CONFIDENCE: f64 = 0.5;

/// Whether `material` has a crystal axis (every material but an isotropic one).
#[must_use]
pub(super) fn has_crystal_axis(material: &GemMaterial) -> bool {
    material.optical_character != OpticalCharacter::Isotropic
}

/// Whether `material` is biaxial, so the Second Axis means something.
#[must_use]
pub(super) const fn is_biaxial(material: &GemMaterial) -> bool {
    matches!(
        material.optical_character,
        OpticalCharacter::BiaxialPositive | OpticalCharacter::BiaxialNegative
    )
}

/// `value` as the text a field shows: at most two decimals, none for a whole number.
fn number(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    format!("{rounded}")
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

/// The orientation after `text` was typed into `field` of the panel (`current` is what the panel
/// showed).
///
/// # Errors
///
/// The message for a value the field cannot take.
pub(super) fn edited_orientation(
    current: &CrystalOrientation,
    field: i32,
    text: &str,
) -> Result<CrystalOrientation, String> {
    let (mut tilt, mut azimuth, mut second) = (
        current.axis.tilt_deg(),
        current.axis.azimuth_deg(),
        current.second_turn_deg,
    );
    match field {
        FIELD_TILT => tilt = angle(text, "Tilt", 0.0, 90.0)?,
        FIELD_AZIMUTH => azimuth = angle(text, "Azimuth", 0.0, 360.0)?,
        FIELD_SECOND => {
            second = if text.trim().is_empty() {
                None
            } else {
                Some(angle(text, "Second Axis", 0.0, 180.0)?)
            };
        }
        _ => return Err("That field does not exist.".to_string()),
    }
    Ok(CrystalOrientation::from_angles(tilt, azimuth, second))
}

/// The note under the panel after a Fit to Crystal Length: how clearly the longest direction
/// stands out, and the level of the note (`1` good, `2` weak).
#[must_use]
pub(super) fn fit_note(confidence: f64, elongation: f64) -> (String, i32) {
    let percent = confidence * 100.0;
    if confidence >= GOOD_CONFIDENCE {
        (
            format!(
                "Fitted to the rough's length. Confidence {percent:.0} % (the rough is {elongation:.1} times longer than wide)."
            ),
            1,
        )
    } else {
        (
            format!(
                "Fitted to the rough's longest direction, but it hardly stands out. Confidence {percent:.0} %: check the axis against the crystal."
            ),
            2,
        )
    }
}

/// What changed since the shown results were planned, as the warning under the rule: `None` when
/// nothing did. `planned_axis` and `planned_rules` are what the results were planned with, `axis`
/// and `rules` what the window holds now.
#[must_use]
pub(super) fn changed_since_planning(
    planned_axis: Option<CrystalOrientation>,
    axis: Option<CrystalOrientation>,
    planned_rules: &PlanSettingsOverrides,
    rules: &PlanSettingsOverrides,
) -> Option<String> {
    let axis_changed = planned_axis != axis;
    let rules_changed = planned_rules.plan != rules.plan || planned_rules.designs != rules.designs;
    match (axis_changed, rules_changed) {
        (false, false) => None,
        (true, false) => Some(
            "The crystal axis changed since planning. The stones keep the orientation they were planned with; plan again to apply it."
                .to_string(),
        ),
        (false, true) => Some(
            "Rules changed since planning. The stones keep the orientation they were planned with; plan again to apply them."
                .to_string(),
        ),
        (true, true) => Some(
            "The crystal axis and the rules changed since planning. The stones keep the orientation they were planned with; plan again to apply them."
                .to_string(),
        ),
    }
}

/// Registers the axis panel's callbacks.
pub(super) fn setup(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.on_axis_make_set(|| on_idle_host(make_set));
    model.on_axis_unknown(|| on_idle_host(|host| apply_orientation(host, None, None)));
    model.on_axis_edit(|field, text| on_idle_host(|host| edit_field(host, field, &text)));
    model.on_axis_pick_start(|kind| on_idle_host(|host| pick_start(host, kind)));
    model.on_axis_fit_length(|| on_idle_host(fit_length));
    model.on_axis_from_zones(|| on_idle_host(from_zones));
}

/// Makes the axis a plain one along the rough's Y axis (the starting point to edit from).
fn make_set(host: &Rc<Host>) {
    if host.session.borrow().model.orientation.is_none() {
        apply_orientation(
            host,
            Some(CrystalOrientation::from_angles(0.0, 0.0, None)),
            None,
        );
    }
}

/// Makes `orientation` the rough's Crystal Axis as one undo step, shows `note` under the panel,
/// ends any pick, and mirrors it all into the window.
pub(in crate::gui::rough_plan) fn apply_orientation(
    host: &Rc<Host>,
    orientation: Option<CrystalOrientation>,
    note: Option<(String, i32)>,
) {
    {
        let mut session = host.session.borrow_mut();
        session.model.orientation = orientation;
        session.commit();
        session.axis.note = note;
        session.axis.pick = PickMode::Off;
    }
    editing::push_history_flags(host);
    super::sync(host);
    view::redraw(host);
    if orientation.is_some() {
        // A tutorial step may wait for the rough's axis to be set.
        raise_weak(&host.main, ROUGH_AXIS_SET);
    }
}

/// Sets the axis while a drag in the 3D view is under way: no undo step yet, the panel's fields
/// and the view follow.
pub(in crate::gui::rough_plan) fn set_live(host: &Rc<Host>, orientation: CrystalOrientation) {
    host.session.borrow_mut().model.orientation = Some(orientation);
    push(host);
    view::redraw(host);
}

/// The drag ended: the axis it reached is one undo step.
pub(in crate::gui::rough_plan) fn finish_live(host: &Rc<Host>) {
    host.session.borrow_mut().commit();
    editing::push_history_flags(host);
    super::sync(host);
}

/// A field of the panel was typed.
fn edit_field(host: &Rc<Host>, field: i32, text: &str) {
    let Some(current) = host.session.borrow().model.orientation else {
        return;
    };
    match edited_orientation(&current, field, text) {
        Ok(next) => apply_orientation(host, Some(next), None),
        Err(message) => {
            host.session.borrow_mut().axis.note = Some((message, 3));
            // The field shows the axis it had again.
            push(host);
        }
    }
}

/// An "Align to ..." button: starts the pick (`1` a face, `2` the ends of an edge) or cancels it
/// (`0`). The next clicks on the 3D view make it (`view::axis_gizmo`).
fn pick_start(host: &Rc<Host>, kind: i32) {
    let (pick, note) = match kind {
        1 => (
            PickMode::Face,
            Some(("Click a face of the rough in the 3D view.".to_string(), 0)),
        ),
        2 => (
            PickMode::EdgeFirst,
            Some((
                "Click the first end of the edge on the rough in the 3D view.".to_string(),
                0,
            )),
        ),
        _ => (PickMode::Off, None),
    };
    {
        let mut session = host.session.borrow_mut();
        session.axis.pick = pick;
        session.axis.note = note;
    }
    // A pick works on the rough, so the 3D view shows it.
    if pick.is_on() {
        view::axis_view::show_rough(host);
    }
    push(host);
}

/// Fit to Crystal Length: the axis along the rough's longest direction.
fn fit_length(host: &Rc<Host>) {
    let (model, second): (RoughModel, Option<f64>) = {
        let session = host.session.borrow();
        (
            session.model.clone(),
            session.model.orientation.and_then(|o| o.second_turn_deg),
        )
    };
    if let Some(fit) = fit_to_crystal_length_of(&model, second) {
        apply_orientation(
            host,
            Some(fit.orientation),
            Some(fit_note(fit.confidence, fit.elongation)),
        );
    } else {
        host.session.borrow_mut().axis.note = Some((
            "The rough has no length direction to fit. Model the rough first.".to_string(),
            3,
        ));
        push(host);
    }
}

/// From Color Zones: the axis along the accepted coaxial color zone of the rough color.
fn from_zones(host: &Rc<Host>) {
    let second = host
        .session
        .borrow()
        .model
        .orientation
        .and_then(|o| o.second_turn_deg);
    let found =
        zoning_hooks::colour_zones().and_then(|zoned| orientation_from_color_zones(&zoned, second));
    if let Some(orientation) = found {
        apply_orientation(
            host,
            Some(orientation),
            Some((
                "The crystal axis follows the axis of the color zone.".to_string(),
                1,
            )),
        );
    } else {
        host.session.borrow_mut().axis.note = Some((
            "There is no accepted color zone along the crystal.".to_string(),
            3,
        ));
        push(host);
    }
}

/// Mirrors the axis, its words, the pick, the note and the warning into the window.
pub(super) fn push(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    let known = material::picked_name(host).and_then(|name| material::known_material(host, &name));
    let (orientation, pick, note, stale) = {
        let session = host.session.borrow();
        let stale = if session.run.layouts.is_empty() {
            None
        } else {
            session.run.plan_model.as_ref().and_then(|planned| {
                changed_since_planning(
                    planned.orientation,
                    session.model.orientation,
                    &session.run.plan_overrides,
                    &session.overrides,
                )
            })
        };
        (
            session.model.orientation,
            session.axis.pick,
            session.axis.note.clone(),
            stale,
        )
    };
    model.set_axis_available(known.as_ref().is_some_and(has_crystal_axis));
    model.set_axis_biaxial(known.as_ref().is_some_and(is_biaxial));
    model.set_axis_set(orientation.is_some());
    if let Some(orientation) = orientation {
        model.set_axis_tilt(number(orientation.axis.tilt_deg()).into());
        model.set_axis_azimuth(number(orientation.axis.azimuth_deg()).into());
        model.set_axis_second(
            orientation
                .second_turn_deg
                .map_or_else(String::new, number)
                .into(),
        );
        model.set_axis_sentence(orientation_sentence(&orientation).into());
    } else {
        model.set_axis_sentence("".into());
    }
    model.set_axis_zones_possible(
        zoning_hooks::colour_zones()
            .and_then(|zoned| orientation_from_color_zones(&zoned, None))
            .is_some(),
    );
    model.set_axis_pick(pick.code());
    let (text, level) = note.unwrap_or_default();
    model.set_axis_note(text.into());
    model.set_axis_note_level(level);
    model.set_axis_stale_text(stale.unwrap_or_default().into());
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::rough_plan::orientation::{RuleChoice, RuleSetting};

    fn axis(tilt: f64, azimuth: f64) -> CrystalOrientation {
        CrystalOrientation::from_angles(tilt, azimuth, None)
    }

    #[test]
    fn a_typed_tilt_changes_only_the_tilt() {
        let current = CrystalOrientation::from_angles(10.0, 40.0, Some(30.0));
        let next = edited_orientation(&current, FIELD_TILT, "25").expect("a valid tilt");
        assert_eq!(next.axis.tilt_deg(), 25.0);
        assert_eq!(next.axis.azimuth_deg(), 40.0);
        assert_eq!(next.second_turn_deg, Some(30.0));
    }

    #[test]
    fn the_azimuth_and_the_second_axis_are_typed_the_same_way() {
        let current = axis(30.0, 0.0);
        let turned = edited_orientation(&current, FIELD_AZIMUTH, "120").expect("an azimuth");
        assert_eq!(turned.axis.azimuth_deg(), 120.0);
        let second = edited_orientation(&turned, FIELD_SECOND, "45").expect("a second axis");
        assert_eq!(second.second_turn_deg, Some(45.0));
        let cleared = edited_orientation(&second, FIELD_SECOND, "  ").expect("empty is none");
        assert_eq!(cleared.second_turn_deg, None);
    }

    #[test]
    fn a_value_outside_the_range_or_not_a_number_is_refused_with_its_name() {
        let current = axis(30.0, 0.0);
        for (field, text, name) in [
            (FIELD_TILT, "95", "Tilt"),
            (FIELD_AZIMUTH, "400", "Azimuth"),
            (FIELD_SECOND, "190", "Second Axis"),
            (FIELD_TILT, "steep", "Tilt"),
        ] {
            let message = edited_orientation(&current, field, text).expect_err("refused");
            assert!(message.contains(name), "{message}");
        }
        assert!(edited_orientation(&current, 9, "1").is_err());
    }

    #[test]
    fn the_fit_says_how_clearly_the_length_stands_out() {
        let (good, level) = fit_note(0.9, 3.2);
        assert_eq!(level, 1);
        assert!(good.contains("90 %") && good.contains("3.2"), "{good}");
        let (weak, level) = fit_note(0.2, 1.1);
        assert_eq!(level, 2);
        assert!(weak.contains("20 %") && weak.contains("hardly"), "{weak}");
    }

    #[test]
    fn nothing_changed_since_planning_means_no_warning() {
        let rules = PlanSettingsOverrides::default();
        assert_eq!(
            changed_since_planning(Some(axis(10.0, 0.0)), Some(axis(10.0, 0.0)), &rules, &rules),
            None
        );
        // A choice for a stone is not a rule.
        let mut with_stone = PlanSettingsOverrides::default();
        with_stone.stones.insert(
            (0, 0),
            pleochroma_cut_core::rough_plan::orientation::StoneChoice {
                locked: true,
                ..Default::default()
            },
        );
        assert_eq!(
            changed_since_planning(None, None, &rules, &with_stone),
            None
        );
    }

    #[test]
    fn a_changed_rule_or_axis_is_named_in_the_warning() {
        let planned = PlanSettingsOverrides::default();
        let mut edited = PlanSettingsOverrides::default();
        edited.set_plan(RuleSetting {
            choice: RuleChoice::Ignore,
            reason: String::new(),
        });
        let rules = changed_since_planning(None, None, &planned, &edited).expect("a warning");
        assert!(rules.starts_with("Rules changed since planning"), "{rules}");
        let moved = changed_since_planning(
            Some(axis(0.0, 0.0)),
            Some(axis(5.0, 0.0)),
            &planned,
            &planned,
        )
        .expect("a warning");
        assert!(moved.contains("axis changed"), "{moved}");
        let both = changed_since_planning(None, Some(axis(5.0, 0.0)), &planned, &edited)
            .expect("a warning");
        assert!(both.contains("axis and the rules"), "{both}");
    }

    #[test]
    fn only_a_material_with_a_crystal_axis_offers_the_panel() {
        let sapphire = GemMaterial::by_name("Sapphire").expect("a built-in material");
        assert!(has_crystal_axis(&sapphire));
        assert!(!is_biaxial(&sapphire), "sapphire is uniaxial");
        let tanzanite = GemMaterial::by_name("Tanzanite").expect("a built-in material");
        assert!(has_crystal_axis(&tanzanite));
        assert!(is_biaxial(&tanzanite), "tanzanite is biaxial");
    }
}
