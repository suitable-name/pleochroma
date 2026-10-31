//! Tests of the Crystal Axis part of a plan file (schema version 5): that a plan without it is
//! written and stored exactly as it always was, that the rough orientation, the rules and the
//! stone choices round trip, what the loader refuses, and how the stone choices travel through the
//! library's side table, an export and an import.

use super::{
    axis::{
        AxisDto, ChoiceDto, FreePoseDto, StoneChoiceDto, axis_to_dto, orientation_to_dto,
        overrides_from_dtos, setting_from_dto, setting_to_dto,
    },
    dto::SavedPlanDto,
    fixtures::{plain_block, sample_layout, settings, write},
    format::{parse_and_validate_plan, payload_version_of},
    stone_choices::{StoneChoices, load_stone_choices, remap_to_saved, save_stone_choices},
    store,
    tests_db::library,
    transfer::{export_text, import_text},
};
use pleochroma_cut_core::{
    crystal_axis::{
        ColorPresetId, CrystalOrientation, OrientationRule, OrientationRuleKind, RuleStrength,
    },
    rough_plan::{
        fit::StonePose,
        orientation::{PlanSettingsOverrides, RuleChoice, RuleSetting, StoneChoice},
    },
};
use std::collections::BTreeMap;

fn document(text: &str) -> SavedPlanDto {
    toml::from_str(text).expect("the document parses")
}

fn rule(kind: OrientationRuleKind, tolerance_deg: f64, strength: RuleStrength) -> RuleChoice {
    RuleChoice::Set(OrientationRule {
        kind,
        tolerance_deg,
        strength,
    })
}

/// A set of settings that uses every field of the rule tables.
fn busy_overrides() -> PlanSettingsOverrides {
    let mut overrides = PlanSettingsOverrides::default();
    overrides.set_plan(RuleSetting {
        choice: rule(
            OrientationRuleKind::InTablePlane,
            10.0,
            RuleStrength::Required,
        ),
        reason: "cut for color".to_string(),
    });
    overrides.set_design(
        1,
        RuleSetting {
            choice: RuleChoice::Ignore,
            reason: "this one lies flat".to_string(),
        },
    );
    overrides.set_design(
        2,
        RuleSetting {
            choice: rule(
                OrientationRuleKind::CustomAngle {
                    tilt_deg: 30.0,
                    azimuth_deg: Some(45.0),
                },
                12.5,
                RuleStrength::Preferred {
                    weight_per_deg: 0.5,
                },
            ),
            reason: String::new(),
        },
    );
    overrides.set_design(
        3,
        RuleSetting {
            choice: rule(
                OrientationRuleKind::FaceUp(ColorPresetId::BestBlueFaceUp),
                15.0,
                RuleStrength::Preferred {
                    weight_per_deg: 0.25,
                },
            ),
            reason: String::new(),
        },
    );
    overrides
}

fn free_pose() -> StonePose {
    StonePose {
        center_mm: [1.0, 2.0, 3.0],
        axes: [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]],
        mm_per_unit: 2.5,
    }
}

/// The fixture plan as a document with `edit` applied, written with the version its content needs.
fn edited(edit: impl FnOnce(&mut SavedPlanDto)) -> String {
    let mut dto = document(&write(&plain_block(), &[sample_layout()]));
    edit(&mut dto);
    dto.version = dto.content_version();
    toml::to_string_pretty(&dto).expect("the document is written")
}

#[test]
fn a_plan_without_axis_data_is_written_and_stored_as_it_always_was() {
    let text = write(&plain_block(), &[sample_layout()]);
    assert_eq!(payload_version_of(&text), Some(1), "{text}");
    for word in ["orientation", "[axis", "axis.", "reason", "locked"] {
        assert!(!text.contains(word), "{word} in {text}");
    }
    // The default settings write the same bytes as the function without them.
    let dto = document(&text);
    assert_eq!(axis_to_dto(&PlanSettingsOverrides::default()), None);
    assert_eq!(dto.axis, None);
    assert_eq!(dto.rough.orientation, None);
    let loaded = parse_and_validate_plan(&text).expect("it loads");
    assert!(loaded.overrides.is_default());
    assert_eq!(loaded.model.orientation, None);
}

#[test]
fn the_rough_orientation_makes_a_version_5_plan_and_round_trips() {
    let orientation = CrystalOrientation::from_angles(40.0, 25.0, Some(10.0));
    let model = plain_block().with_orientation(Some(orientation));
    let text = write(&model, &[sample_layout()]);
    assert_eq!(payload_version_of(&text), Some(5), "{text}");
    assert!(text.contains("[rough.orientation]"), "{text}");
    let loaded = parse_and_validate_plan(&text).expect("it loads");
    assert_eq!(loaded.version, 5);
    let back = loaded.model.orientation.expect("the orientation is kept");
    let (a, b) = (orientation_to_dto(&orientation), orientation_to_dto(&back));
    assert!((a.tilt_deg - b.tilt_deg).abs() < 1e-9);
    assert!((a.azimuth_deg - b.azimuth_deg).abs() < 1e-9);
    assert_eq!(a.second_turn_deg, b.second_turn_deg);
}

#[test]
fn the_rules_and_reasons_round_trip_through_the_file() {
    let overrides = busy_overrides();
    let text = edited(|dto| dto.axis = axis_to_dto(&overrides));
    assert_eq!(payload_version_of(&text), Some(5), "{text}");
    assert!(text.contains("[axis.rule]"), "{text}");
    assert!(text.contains("[[axis.designs]]"), "{text}");
    let loaded = parse_and_validate_plan(&text).expect("it loads");
    assert_eq!(loaded.overrides, overrides);

    // A Reason alone is a setting too; the untouched default writes nothing.
    let mut reason_only = PlanSettingsOverrides::default();
    reason_only.set_plan(RuleSetting {
        choice: RuleChoice::Default,
        reason: "because".to_string(),
    });
    assert!(axis_to_dto(&reason_only).is_some());
    for setting in [
        RuleSetting::default(),
        RuleSetting {
            choice: RuleChoice::Ignore,
            reason: String::new(),
        },
        busy_overrides().plan,
    ] {
        let dto = setting_to_dto(&setting, None);
        assert_eq!(setting_from_dto(&dto, "rule").expect("valid"), setting);
    }
}

#[test]
fn a_rule_the_loader_cannot_use_is_refused_with_its_path() {
    let bad = |edit: &dyn Fn(&mut AxisDto)| {
        let text = edited(|dto| {
            let mut axis = axis_to_dto(&busy_overrides()).expect("busy settings");
            edit(&mut axis);
            dto.axis = Some(axis);
        });
        parse_and_validate_plan(&text).expect_err("the file is refused")
    };
    let error = bad(&|axis| {
        if let Some(rule) = &mut axis.rule {
            rule.tolerance_deg = Some(120.0);
        }
    });
    assert!(error.contains("axis.rule.tolerance_deg"), "{error}");
    let error = bad(&|axis| axis.designs[1].tilt_deg = Some(f64::NAN));
    assert!(error.contains("axis.designs[1].tilt_deg"), "{error}");
    let error = bad(&|axis| axis.designs[2].preset = None);
    assert!(error.contains("axis.designs[2].preset"), "{error}");
    let error = bad(&|axis| axis.designs[0].entry_id = None);
    assert!(error.contains("axis.designs[0].entry_id"), "{error}");
    let error = bad(&|axis| axis.designs[1].entry_id = Some(1));
    assert!(error.contains("a second time"), "{error}");
    let error = bad(&|axis| {
        if let Some(rule) = &mut axis.rule {
            rule.reason = "x".repeat(501);
        }
    });
    assert!(error.contains("axis.rule.reason"), "{error}");
    let error = bad(&|axis| {
        if let Some(rule) = &mut axis.rule {
            rule.choice = ChoiceDto::Set;
            rule.kind = None;
        }
    });
    assert!(error.contains("axis.rule.kind"), "{error}");

    // An older version cannot hold the tables.
    let text = edited(|dto| dto.axis = axis_to_dto(&busy_overrides())).replacen(
        "version = 5",
        "version = 4",
        1,
    );
    let error = parse_and_validate_plan(&text).expect_err("refused");
    assert!(error.contains("axis needs plan version 5"), "{error}");
}

fn stone_dto(layout: u32, stone: u32, locked: bool) -> StoneChoiceDto {
    StoneChoiceDto {
        layout,
        stone,
        pose: Some(2),
        locked,
        reason: "keep".to_string(),
        free_pose: Some(FreePoseDto {
            center_mm: [1.0, 2.0, 3.0],
            axes: [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]],
            mm_per_unit: 2.5,
        }),
    }
}

#[test]
fn stone_choices_in_the_file_are_checked_and_come_back_as_settings() {
    let text = edited(|dto| {
        dto.axis = Some(AxisDto {
            stones: vec![stone_dto(0, 1, true)],
            ..AxisDto::default()
        });
        dto.pose_choices = vec![super::dto::PoseChoiceDto {
            layout: 0,
            stone: 2,
            pose: 3,
        }];
    });
    assert!(text.contains("[[axis.stones]]"), "{text}");
    let loaded = parse_and_validate_plan(&text).expect("it loads");
    let kept = loaded
        .overrides
        .stone_choice(0, 1)
        .expect("the rich choice");
    assert!(kept.locked && kept.free_pose == Some(free_pose()) && kept.reason == "keep");
    assert_eq!(loaded.overrides.stone_choice(0, 2).map(|c| c.pose), Some(3));

    // A stone the plan does not have, and a stone chosen twice, are refused.
    for (stones, wanted) in [
        (vec![stone_dto(1, 0, false)], "axis.stones[0].layout"),
        (vec![stone_dto(0, 3, false)], "axis.stones[0].stone"),
        (
            vec![stone_dto(0, 1, false), stone_dto(0, 1, true)],
            "a second time",
        ),
    ] {
        let text = edited(|dto| {
            dto.axis = Some(AxisDto {
                stones,
                ..AxisDto::default()
            });
        });
        let error = parse_and_validate_plan(&text).expect_err("refused");
        assert!(error.contains(wanted), "{error}");
    }
    let text = edited(|dto| {
        let mut stone = stone_dto(0, 0, false);
        stone.pose = Some(4);
        dto.axis = Some(AxisDto {
            stones: vec![stone],
            ..AxisDto::default()
        });
    });
    let error = parse_and_validate_plan(&text).expect_err("refused");
    assert!(error.contains("axis.stones[0].pose"), "{error}");
}

#[test]
fn stone_choices_save_to_the_library_and_travel_through_an_export_and_an_import() {
    let first = library();
    let layouts = [sample_layout()];
    let mut overrides = busy_overrides();
    let rich = StoneChoice {
        pose: 2,
        free_pose: Some(free_pose()),
        locked: true,
        reason: "keep".to_string(),
    };
    overrides.set_stone_choice(
        0,
        1,
        StoneChoice {
            free_pose: None,
            ..rich.clone()
        },
    );
    overrides.set_stone_choice(
        0,
        2,
        StoneChoice {
            pose: 3,
            ..StoneChoice::default()
        },
    );
    overrides.set_stone_choice(0, 0, rich);
    // The payload carries the rules, not the stones.
    let payload = edited(|dto| dto.axis = axis_to_dto(&overrides));
    let id = store::save_plan(&first, "Axis plan", &payload).expect("stored");
    {
        let database = first.lock().expect("lock");
        save_stone_choices(&database, id, &overrides.stones).expect("stored");
        assert_eq!(
            load_stone_choices(&database, id).expect("read"),
            overrides.stones
        );
    }

    let text = export_text(&first, id).expect("exported");
    assert_eq!(payload_version_of(&text), Some(5), "{text}");
    assert!(text.contains("[[axis.stones]]"), "{text}");
    assert!(text.contains("[[pose_choices]]"), "{text}");
    let loaded = parse_and_validate_plan(&text).expect("the export is a plan");
    assert_eq!(loaded.layouts, layouts);
    assert_eq!(loaded.overrides, overrides);

    // An import stores the rules in the payload and the stones in the side table.
    let second = library();
    let opened = import_text(&second, "plan.toml", &text).expect("imported");
    let stored = store::load_saved(&second, opened.plan_id).expect("loaded");
    assert!(
        !stored.payload.contains("axis.stones"),
        "{}",
        stored.payload
    );
    assert!(stored.payload.contains("[axis.rule]"), "{}", stored.payload);
    assert_eq!(stored.payload_version, 5);
    {
        let database = second.lock().expect("lock");
        assert_eq!(
            load_stone_choices(&database, opened.plan_id).expect("read"),
            overrides.stones
        );
    }
    assert_eq!(
        export_text(&second, opened.plan_id).expect("exported"),
        text
    );
}

#[test]
fn saving_stone_choices_merges_with_those_the_wizard_stored() {
    let lib = library();
    let payload = edited(|dto| dto.axis = axis_to_dto(&busy_overrides()));
    let id = store::save_plan(&lib, "Merge plan", &payload).expect("stored");
    let database = lib.lock().expect("lock");
    let pose_of = |pose: u8| StoneChoice {
        pose,
        ..StoneChoice::default()
    };
    // The wizard stored stones (0, 0) and (0, 1).
    let mut wizard = StoneChoices::new();
    wizard.insert((0, 0), pose_of(1));
    wizard.insert((0, 1), pose_of(2));
    save_stone_choices(&database, id, &wizard).expect("stored");
    // The planner saves (0, 1) with another pose and a new stone (0, 2).
    let mut planner = StoneChoices::new();
    planner.insert((0, 1), pose_of(3));
    planner.insert((0, 2), pose_of(4));
    save_stone_choices(&database, id, &planner).expect("stored");
    let merged = load_stone_choices(&database, id).expect("read");
    drop(database);
    assert_eq!(merged.len(), 3);
    assert_eq!(merged[&(0, 0)].pose, 1, "the wizard's choice survives");
    assert_eq!(merged[&(0, 1)].pose, 3, "the planner wins a conflict");
    assert_eq!(merged[&(0, 2)].pose, 4);
}

#[test]
fn a_save_of_some_rows_moves_their_choices_to_the_new_positions() {
    let mut stones: BTreeMap<(u32, u32), StoneChoice> = BTreeMap::new();
    for (row, stone) in [(0, 0), (1, 4), (2, 1)] {
        stones.insert(
            (row, stone),
            StoneChoice {
                pose: u8::try_from(row + 1).expect("small"),
                ..StoneChoice::default()
            },
        );
    }
    // Rows 3 and 1 (1-based ranks) are saved, in that order.
    let moved = remap_to_saved(&stones, &[3, 1]);
    assert_eq!(moved.len(), 2);
    assert_eq!(moved[&(0, 1)].pose, 3);
    assert_eq!(moved[&(1, 0)].pose, 1);
    assert!(remap_to_saved(&stones, &[]).is_empty());
}

#[test]
fn overrides_from_nothing_are_the_default() {
    assert!(
        overrides_from_dtos(None, &[])
            .expect("nothing to read")
            .is_default()
    );
}

#[test]
fn a_reason_edited_after_planning_survives_a_save_and_an_open() {
    use super::{
        open::prepare_open,
        save::{snapshot, write_plan},
    };
    use crate::gui::rough_plan::session::Session;

    // The results were planned with no rules; the cutter then gives the plan a rule and a Reason
    // and a stone a Reason, and saves.
    let mut session = Session::default();
    session.run.layouts = vec![sample_layout()];
    session.run.plan_model = Some(plain_block());
    // The fixture layout's carats follow its own specific gravity (`fixtures::settings`).
    session.run.plan_settings = Some(settings());
    session.run.plan_overrides = PlanSettingsOverrides::default();
    session.run.material_name = "Sapphire".to_string();
    let mut edited_rules = busy_overrides();
    edited_rules.set_stone_choice(
        0,
        0,
        StoneChoice {
            reason: "kept the color".to_string(),
            ..StoneChoice::default()
        },
    );
    session.overrides = edited_rules.clone();

    let taken = snapshot(&session, false).expect("a snapshot");
    let lib = library();
    let id = write_plan(&lib, "Edited after planning", &taken).expect("saved");
    let opened = prepare_open(&lib, id).expect("opens");
    assert_eq!(opened.plan.overrides.plan, edited_rules.plan);
    assert_eq!(opened.plan.overrides.designs, edited_rules.designs);
    assert_eq!(
        opened.plan.overrides.stones[&(0, 0)].reason,
        "kept the color"
    );
}

/// The rules the layouts were planned with travel in `[axis.planned]` only when they differ from
/// the rules the plan holds, and come back on open; a plan whose rules were not edited after
/// planning is written exactly as before.
#[test]
fn the_rules_a_plan_was_planned_with_survive_a_save_and_an_open() {
    use super::{
        axis::axis_to_dto_planned,
        open::prepare_open,
        save::{snapshot, write_plan},
    };
    use crate::gui::rough_plan::session::Session;

    // Not edited after planning: the planned rules equal the held ones and nothing is added.
    let held = busy_overrides();
    assert_eq!(
        axis_to_dto_planned(&held, Some(&held)),
        axis_to_dto(&held),
        "the same rules add no planned table"
    );
    assert_eq!(
        axis_to_dto_planned(&PlanSettingsOverrides::default(), None),
        None
    );

    // Edited after planning: the results were planned with no rules, the plan holds `held`.
    let mut session = Session::default();
    session.run.layouts = vec![sample_layout()];
    session.run.plan_model = Some(plain_block());
    // The fixture layout's carats follow its own specific gravity (`fixtures::settings`).
    session.run.plan_settings = Some(settings());
    session.run.plan_overrides = PlanSettingsOverrides::default();
    session.run.material_name = "Sapphire".to_string();
    session.overrides = held.clone();
    let taken = snapshot(&session, false).expect("a snapshot");
    let lib = library();
    let id = write_plan(&lib, "Edited rules", &taken).expect("saved");
    let opened = prepare_open(&lib, id).expect("opens");
    assert_eq!(opened.plan.overrides.plan, held.plan);
    let planned = opened
        .plan
        .planned_rules
        .expect("the planned rules are kept");
    assert!(planned.is_default(), "planned with no rules: {planned:?}");

    // Planned with other rules than the plan holds now: both come back, and the window's warning
    // compares them.
    let mut earlier = PlanSettingsOverrides::default();
    earlier.set_plan(RuleSetting {
        choice: RuleChoice::Ignore,
        reason: String::new(),
    });
    session.run.plan_overrides = earlier.clone();
    let taken = snapshot(&session, false).expect("a snapshot");
    let id = write_plan(&lib, "Edited rules again", &taken).expect("saved");
    let opened = prepare_open(&lib, id).expect("opens");
    let planned = opened
        .plan
        .planned_rules
        .expect("the planned rules are kept");
    assert_eq!(planned.plan, earlier.plan);
    assert_eq!(opened.plan.overrides.designs, held.designs);

    // Planned with the rules the plan holds: no planned table, nothing on open.
    session.run.plan_overrides = held;
    let taken = snapshot(&session, false).expect("a snapshot");
    let id = write_plan(&lib, "Unedited rules", &taken).expect("saved");
    let opened = prepare_open(&lib, id).expect("opens");
    assert_eq!(opened.plan.planned_rules, None);
}

/// A planned table in the file is checked like the rules around it.
#[test]
fn a_planned_table_with_a_bad_rule_is_refused_with_its_path() {
    use super::axis::PlannedRulesDto;

    let mut bad = setting_to_dto(&busy_overrides().plan, None);
    bad.tolerance_deg = Some(120.0);
    let text = edited(|dto| {
        dto.axis = Some(AxisDto {
            planned: Some(PlannedRulesDto {
                rule: Some(bad),
                designs: Vec::new(),
            }),
            ..AxisDto::default()
        });
    });
    let error = parse_and_validate_plan(&text).expect_err("refused");
    assert!(error.contains("axis.planned.rule.tolerance_deg"), "{error}");
}

#[test]
fn a_stone_set_back_to_the_default_loses_its_stored_row() {
    use super::stone_choices::clear_removed_stone_choices;

    let lib = library();
    let payload = edited(|dto| dto.axis = axis_to_dto(&busy_overrides()));
    let id = store::save_plan(&lib, "Reset plan", &payload).expect("stored");
    let database = lib.lock().expect("lock");
    let choice = |pose: u8| StoneChoice {
        pose,
        ..StoneChoice::default()
    };
    // Stones (0, 0) and (0, 1) of the saved layout and (4, 0) of a layout the save did not cover
    // (the color wizard's) have stored rows.
    let mut stored = StoneChoices::new();
    stored.insert((0, 0), choice(1));
    stored.insert((0, 1), choice(2));
    stored.insert((4, 0), choice(3));
    save_stone_choices(&database, id, &stored).expect("stored");

    // The cutter put (0, 1) back to the default: the session no longer holds it.
    let mut session = StoneChoices::new();
    session.insert((0, 0), choice(1));
    clear_removed_stone_choices(&database, id, &session, 1).expect("cleared");
    let left = load_stone_choices(&database, id).expect("read");
    drop(database);
    assert!(left.contains_key(&(0, 0)));
    assert!(!left.contains_key(&(0, 1)), "the reset stone keeps no row");
    assert!(left.contains_key(&(4, 0)), "an unsaved layout's row stays");
}
