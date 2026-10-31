//! What the results say about each stone's crystal orientation: the stone rows of the selected
//! result.
//!
//! A row shows the angle between the stone's Crystal Axis and its table normal, whether the stone
//! follows the plan's rule (a check mark), is off it ("15° off") or was overridden by the cutter
//! ("overridden (reason)"), and for a biaxial material the angle of the color direction to the
//! table. The orientation is the TRUE one, from the pose the stone really has (the cutter's choice
//! or the planner's), never the rule's target (`stone_orientation_row`).
//!
//! The pure half builds the rows from plain data and is tested here; the window half reads the
//! session and pushes them into `RoughPlanModel.stone_rows`.

use super::material;
use crate::{
    RoughPlanModel, RoughStoneRow,
    gui::rough_plan::{
        design_info_of,
        format::{design_name, effective_id, group_order, to_i32},
        host::Host,
    },
};
use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::{
    crystal_axis::CrystalOrientation,
    rough_plan::{
        DesignPlacement, RoughLayout,
        orientation::{
            LOCKED_FIRST_NOTE, OrientationRow, PlanSettingsOverrides, ResolveContext, RuleStatus,
            StoneChoice, stone_orientation_row,
        },
    },
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

/// One stone row as plain data.
#[derive(Debug, Clone, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a view model: one flag per control the row offers, as the window's row takes them"
)]
pub(super) struct StoneRowData {
    /// The stone's place in its result.
    pub(super) stone: usize,
    /// The stone's design row in the result card (the group order).
    pub(super) group: usize,
    /// "Stone 2: Round Brilliant".
    pub(super) title: String,
    /// "Axis 12° from the table normal".
    pub(super) angle_text: String,
    /// The rule column: a check mark, "15° off", "overridden (reason)" or empty.
    pub(super) status: String,
    /// `0` no rule, `1` followed, `2` off, `3` overridden.
    pub(super) status_level: i32,
    /// The color direction's angle, for a biaxial material; empty otherwise.
    pub(super) color_text: String,
    /// The stone is locked: a re-plan cuts the rest of the rough around it.
    pub(super) locked: bool,
    /// The Reason the cutter gave for the stone.
    pub(super) reason: String,
    /// The box pose chosen, `0` the planner's own.
    pub(super) pose: u8,
    /// The four box poses are offered.
    pub(super) can_pose: bool,
    /// Re-fit This Piece is offered.
    pub(super) can_refit: bool,
    /// Free Orientation is offered.
    pub(super) can_free: bool,
}

/// What the rows of one result are made from.
pub(super) struct RowInputs<'a> {
    /// The result's layout.
    pub(super) layout: &'a RoughLayout,
    /// The result's place in the shown results.
    pub(super) result: u32,
    /// The rough's orientation, rough frame.
    pub(super) rough: &'a CrystalOrientation,
    /// The plan's material.
    pub(super) material: &'a GemMaterial,
    /// The rules the plan was made with.
    pub(super) rules: &'a PlanSettingsOverrides,
    /// The cutter's choices for the stones, as they are now.
    pub(super) choices: &'a PlanSettingsOverrides,
    /// The designs' titles by entry id.
    pub(super) titles: &'a BTreeMap<i64, String>,
    /// The placement of a design (by the entry id the layout uses), `None` when it cannot be read.
    pub(super) placement_of: &'a dyn Fn(i64) -> Option<DesignPlacement>,
}

/// The level of a rule status: `0` no rule, `1` followed, `2` off, `3` overridden.
#[must_use]
const fn status_level(status: &RuleStatus) -> i32 {
    match status {
        RuleStatus::NoRule => 0,
        RuleStatus::Followed => 1,
        RuleStatus::Off { .. } => 2,
        RuleStatus::Overridden { .. } => 3,
    }
}

/// The texts of a row: the angle, the status, its level and the color direction. A stone whose
/// design cannot be placed has no orientation to show.
#[must_use]
pub(super) fn describe(row: Option<&OrientationRow>) -> (String, String, i32, String) {
    let Some(row) = row else {
        return (
            "The axis angle is not available.".to_string(),
            String::new(),
            0,
            String::new(),
        );
    };
    (
        format!(
            "Axis {:.0}\u{b0} from the table normal",
            row.axis_to_table_deg
        ),
        row.status.text(),
        status_level(&row.status),
        row.color_direction_deg.map_or_else(String::new, |degrees| {
            format!("Color direction {degrees:.0}\u{b0} from the table normal")
        }),
    )
}

/// The rows of one result, locked stones first, the others in the layout's order.
#[must_use]
pub(super) fn stone_rows(inputs: &RowInputs<'_>) -> Vec<StoneRowData> {
    let order = group_order(inputs.layout);
    let placements = BTreeMap::new();
    let ctx = ResolveContext {
        rough: Some(*inputs.rough),
        material: inputs.material,
        placements: &placements,
    };
    let exact = inputs.layout.exact_fit;
    let mut rows: Vec<StoneRowData> = inputs
        .layout
        .stones
        .iter()
        .enumerate()
        .map(|(index, stone)| {
            let choice = u32::try_from(index)
                .ok()
                .and_then(|stone_index| inputs.choices.stone_choice(inputs.result, stone_index));
            let pose = choice.map_or(stone.pose, |choice| choice.pose_for(&stone.pose));
            let rule = inputs.rules.effective_rule(stone.entry_id, &ctx);
            let orientation = (inputs.placement_of)(stone.entry_id).and_then(|placement| {
                stone_orientation_row(
                    inputs.rough,
                    &pose,
                    &placement,
                    inputs.material,
                    rule.as_ref(),
                    choice,
                )
            });
            let (angle_text, status, level, color_text) = describe(orientation.as_ref());
            let locked = choice.is_some_and(|choice| choice.locked);
            StoneRowData {
                stone: index,
                group: order
                    .iter()
                    .position(|&(entry_id, _)| entry_id == stone.entry_id)
                    .unwrap_or(0),
                title: format!(
                    "Stone {}: {}",
                    index + 1,
                    design_name(inputs.titles, stone.entry_id)
                ),
                angle_text,
                status,
                status_level: level,
                color_text,
                locked,
                reason: choice
                    .map_or_else(String::new, |choice: &StoneChoice| choice.reason.clone()),
                pose: choice.map_or(0, |choice| choice.pose),
                can_pose: !exact && !locked,
                can_refit: !exact && !locked,
                can_free: exact && index == 0 && !locked,
            }
        })
        .collect();
    // Locked stones are cut first: they are listed first (a stable sort keeps the rest in order).
    rows.sort_by_key(|row| !row.locked);
    rows
}

/// The note under the rows: the locked stones are cut first, the slabs are for the rest.
#[must_use]
pub(super) fn rows_note(rows: &[StoneRowData]) -> &'static str {
    if rows.iter().any(|row| row.locked) {
        LOCKED_FIRST_NOTE
    } else {
        ""
    }
}

/// The line of one stone in the cut plan: its title, the angle and the rule column.
fn stone_line(row: &StoneRowData) -> String {
    let mut line = format!("  {}: {}", row.title, row.angle_text);
    if !row.status.is_empty() {
        line.push_str(", ");
        line.push_str(&row.status);
    }
    if !row.color_text.is_empty() {
        line.push_str(", ");
        line.push_str(&row.color_text);
    }
    if !row.reason.is_empty() {
        line.push_str(" (");
        line.push_str(&row.reason);
        line.push(')');
    }
    line
}

/// The cut plan text of a result with what the crystal axis says about its stones: the locked
/// stones first, with the note that they are cut before the slabs below (the slabs are for the
/// rest of the rough), then the saw plan as it was, then one line per stone. A result without rows
/// (no axis) keeps its text exactly.
#[must_use]
pub(super) fn decorate_cut_plan(base: &str, rows: &[StoneRowData], note: &str) -> String {
    if rows.is_empty() {
        return base.to_string();
    }
    let mut text = String::new();
    let locked: Vec<&StoneRowData> = rows.iter().filter(|row| row.locked).collect();
    if !locked.is_empty() {
        text.push_str("Locked stones, cut first\n");
        for row in &locked {
            text.push_str(&stone_line(row));
            text.push('\n');
        }
        text.push_str(note);
        text.push_str("\n\n");
    }
    text.push_str(base);
    text.push_str("\n\nCrystal Axis of the stones\n");
    let lines: Vec<String> = rows.iter().map(stone_line).collect();
    text.push_str(&lines.join("\n"));
    text
}

/// Shows the cut plan texts: the selected result's with the lines about its stones, every other
/// result's as the formatter made it.
fn push_cut_plans(host: &Rc<Host>, rows: &[StoneRowData], note: &str) {
    let bases = host.session.borrow().axis.cut_plans.clone();
    let model = host.window.global::<RoughPlanModel>();
    let selected = usize::try_from(model.get_selected_result()).ok();
    let results = model.get_results();
    for (index, base) in bases.iter().enumerate() {
        let text = if Some(index) == selected {
            decorate_cut_plan(base, rows, note)
        } else {
            base.clone()
        };
        if let Some(mut row) = results.row_data(index)
            && row.cut_plan.as_str() != text
        {
            row.cut_plan = text.into();
            results.set_row_data(index, row);
        }
    }
}

/// The rows of the selected result, or none while no result is selected, the plan has no rough
/// orientation, or the material is not known yet.
fn compute(host: &Rc<Host>) -> (Vec<StoneRowData>, &'static str) {
    let no_rows = (Vec::new(), "");
    let model = host.window.global::<RoughPlanModel>();
    let Ok(result) = usize::try_from(model.get_selected_result()) else {
        return no_rows;
    };
    let name = host.session.borrow().run.material_name.clone();
    let Some(material) = material::known_material(host, &name) else {
        return no_rows;
    };
    let session = host.session.borrow();
    let (Some(layout), Some(plan_model)) = (
        session.run.layouts.get(result),
        session.run.plan_model.as_ref(),
    ) else {
        return no_rows;
    };
    let Some(rough) = plan_model.orientation else {
        return no_rows;
    };
    let Ok(result_index) = u32::try_from(result) else {
        return no_rows;
    };
    let cache: RefCell<BTreeMap<i64, Option<DesignPlacement>>> = RefCell::default();
    let placement_of = |entry_id: i64| {
        *cache.borrow_mut().entry(entry_id).or_insert_with(|| {
            design_info_of(effective_id(&session.run.statuses, entry_id)).map(|info| info.placement)
        })
    };
    let rows = stone_rows(&RowInputs {
        layout,
        result: result_index,
        rough: &rough,
        material: &material,
        rules: &session.run.plan_overrides,
        choices: &session.overrides,
        titles: &session.run.titles,
        placement_of: &placement_of,
    });
    let note = rows_note(&rows);
    (rows, note)
}

/// Mirrors the stone rows of the selected result into the window.
pub(super) fn push(host: &Rc<Host>) {
    let (rows, note) = compute(host);
    push_cut_plans(host, &rows, note);
    let slint_rows: Vec<RoughStoneRow> = rows
        .into_iter()
        .map(|row| RoughStoneRow {
            stone: to_i32(row.stone),
            group: to_i32(row.group),
            title: row.title.into(),
            angle_text: row.angle_text.into(),
            status: row.status.into(),
            status_level: row.status_level,
            color_text: row.color_text.into(),
            locked: row.locked,
            reason: row.reason.into(),
            pose: i32::from(row.pose),
            can_pose: row.can_pose,
            can_refit: row.can_refit,
            can_free: row.can_free,
        })
        .collect();
    let model = host.window.global::<RoughPlanModel>();
    model.set_stone_rows(ModelRc::new(VecModel::from(slint_rows)));
    model.set_stone_rows_note(note.into());
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::rough_plan::{
        Axis, CutOrder, CutPlan, PlacedStone,
        fit::StonePose,
        orientation::{RuleChoice, RuleSetting},
    };

    fn stone(entry_id: i64) -> PlacedStone {
        PlacedStone {
            entry_id,
            piece_origin_mm: [0.0; 3],
            piece_size_mm: [1.0; 3],
            stone_size_mm: [1.0; 3],
            table_axis: Axis::Y,
            carat: 0.1,
            volume_mm3: 1.0,
            pose: StonePose {
                center_mm: [0.5; 3],
                axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                mm_per_unit: 1.0,
            },
        }
    }

    fn layout(stones: Vec<PlacedStone>, exact_fit: bool) -> RoughLayout {
        RoughLayout {
            cut_order: CutOrder::Xyz,
            stones,
            cut_plan: CutPlan { slabs: Vec::new() },
            total_carat: 0.1,
            total_volume_mm3: 1.0,
            yield_fraction: 0.1,
            exact_fit,
        }
    }

    fn rows_for(
        rough_tilt_deg: f64,
        layout: &RoughLayout,
        choices: &PlanSettingsOverrides,
    ) -> Vec<StoneRowData> {
        let material = GemMaterial::by_name("Sapphire").expect("a built-in material");
        let rough = CrystalOrientation::from_angles(rough_tilt_deg, 0.0, None);
        let titles = BTreeMap::from([(4, "Oval".to_string())]);
        let placement_of = |_: i64| Some(DesignPlacement::IDENTITY);
        stone_rows(&RowInputs {
            layout,
            result: 0,
            rough: &rough,
            material: &material,
            rules: &PlanSettingsOverrides::default(),
            choices,
            titles: &titles,
            placement_of: &placement_of,
        })
    }

    #[test]
    fn a_stone_along_the_axis_follows_the_materials_rule() {
        let rows = rows_for(
            0.0,
            &layout(vec![stone(4)], false),
            &PlanSettingsOverrides::default(),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Stone 1: Oval");
        assert_eq!(rows[0].angle_text, "Axis 0\u{b0} from the table normal");
        assert_eq!(rows[0].status, "\u{2713}");
        assert_eq!(rows[0].status_level, 1);
        assert!(rows[0].color_text.is_empty(), "sapphire is uniaxial");
    }

    #[test]
    fn a_stone_across_the_axis_is_off_by_the_angle() {
        let rows = rows_for(
            90.0,
            &layout(vec![stone(4)], false),
            &PlanSettingsOverrides::default(),
        );
        assert_eq!(rows[0].status, "90\u{b0} off");
        assert_eq!(rows[0].status_level, 2);
    }

    #[test]
    fn a_turned_stone_says_overridden_with_its_reason() {
        let mut choices = PlanSettingsOverrides::default();
        choices.set_stone_choice(
            0,
            0,
            StoneChoice {
                pose: 1,
                reason: "flaw".to_string(),
                ..StoneChoice::default()
            },
        );
        let rows = rows_for(0.0, &layout(vec![stone(4)], false), &choices);
        assert_eq!(rows[0].status, "overridden (flaw)");
        assert_eq!(rows[0].status_level, 3);
        assert_eq!((rows[0].pose, rows[0].reason.as_str()), (1, "flaw"));
    }

    #[test]
    fn the_menu_offers_what_each_stone_can_do() {
        let planned = rows_for(
            0.0,
            &layout(vec![stone(4)], false),
            &PlanSettingsOverrides::default(),
        );
        assert!(planned[0].can_pose && planned[0].can_refit && !planned[0].can_free);

        let exact = rows_for(
            0.0,
            &layout(vec![stone(4)], true),
            &PlanSettingsOverrides::default(),
        );
        assert!(!exact[0].can_pose && !exact[0].can_refit && exact[0].can_free);

        let mut choices = PlanSettingsOverrides::default();
        choices.set_stone_choice(
            0,
            0,
            StoneChoice {
                locked: true,
                ..StoneChoice::default()
            },
        );
        let locked = rows_for(0.0, &layout(vec![stone(4)], false), &choices);
        assert!(locked[0].locked);
        assert!(!locked[0].can_pose && !locked[0].can_refit && !locked[0].can_free);
    }

    #[test]
    fn locked_stones_are_listed_first_and_the_note_says_why() {
        let mut choices = PlanSettingsOverrides::default();
        choices.set_stone_choice(
            0,
            1,
            StoneChoice {
                locked: true,
                ..StoneChoice::default()
            },
        );
        let rows = rows_for(0.0, &layout(vec![stone(4), stone(4)], false), &choices);
        assert_eq!(rows.iter().map(|row| row.stone).collect::<Vec<_>>(), [1, 0]);
        assert_eq!(rows_note(&rows), LOCKED_FIRST_NOTE);
        let plain = rows_for(
            0.0,
            &layout(vec![stone(4)], false),
            &PlanSettingsOverrides::default(),
        );
        assert_eq!(rows_note(&plain), "");
    }

    #[test]
    fn the_cut_plan_gets_one_line_per_stone_and_keeps_its_own_text() {
        let rows = rows_for(
            90.0,
            &layout(vec![stone(4)], false),
            &PlanSettingsOverrides::default(),
        );
        let text = decorate_cut_plan("Slab 1: 3.0 mm", &rows, "");
        assert!(
            text.starts_with("Slab 1: 3.0 mm\n\nCrystal Axis of the stones\n"),
            "{text}"
        );
        assert!(
            text.contains("Stone 1: Oval: Axis 90\u{b0} from the table normal, 90\u{b0} off"),
            "{text}"
        );
        assert_eq!(
            decorate_cut_plan("Slab 1: 3.0 mm", &[], ""),
            "Slab 1: 3.0 mm"
        );
    }

    #[test]
    fn a_locked_stone_is_listed_before_the_slabs_with_the_note() {
        let mut choices = PlanSettingsOverrides::default();
        choices.set_stone_choice(
            0,
            1,
            StoneChoice {
                locked: true,
                reason: "keep".to_string(),
                ..StoneChoice::default()
            },
        );
        let rows = rows_for(0.0, &layout(vec![stone(4), stone(4)], false), &choices);
        let text = decorate_cut_plan("Slab 1", &rows, LOCKED_FIRST_NOTE);
        assert!(
            text.starts_with("Locked stones, cut first\n  Stone 2: Oval"),
            "{text}"
        );
        assert!(text.contains("(keep)"), "{text}");
        let note_at = text.find(LOCKED_FIRST_NOTE).expect("the note");
        let slab_at = text.find("Slab 1").expect("the saw plan");
        assert!(note_at < slab_at, "the note comes before the slabs");
    }

    #[test]
    fn ignoring_the_axis_leaves_no_rule_to_follow() {
        let material = GemMaterial::by_name("Sapphire").expect("a built-in material");
        let rough = CrystalOrientation::from_angles(90.0, 0.0, None);
        let mut rules = PlanSettingsOverrides::default();
        rules.set_plan(RuleSetting {
            choice: RuleChoice::Ignore,
            reason: String::new(),
        });
        let placement_of = |_: i64| Some(DesignPlacement::IDENTITY);
        let rows = stone_rows(&RowInputs {
            layout: &layout(vec![stone(4)], false),
            result: 0,
            rough: &rough,
            material: &material,
            rules: &rules,
            choices: &PlanSettingsOverrides::default(),
            titles: &BTreeMap::new(),
            placement_of: &placement_of,
        });
        assert_eq!(rows[0].status, "");
        assert_eq!(rows[0].status_level, 0);
        assert_eq!(rows[0].title, "Stone 1: Design #4");
    }

    #[test]
    fn a_design_that_cannot_be_placed_has_no_angle() {
        let (angle, status, level, color) = describe(None);
        assert!(angle.contains("not available"));
        assert_eq!((status.as_str(), level, color.as_str()), ("", 0, ""));
    }
}
