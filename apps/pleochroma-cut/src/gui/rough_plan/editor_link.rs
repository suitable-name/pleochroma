//! "Open in Editor" on a design row: the first stone of that design opens in the main window's
//! editor with the piece of rough it comes from as the design's preform, at the stone's real
//! size (and with the rough's colour when the plan has one).
//!
//! The row's stone is picked ([`pick_stone`]) and placed ([`resolve_target`]: the design's
//! placement and widths from the library, the pose the cutter chose), the sawn piece of rough it
//! comes from is moved into the design's frame (`stone_preform_planes`), and the lot is handed to
//! `gui::editor::planned_stone`, which loads the design through the normal Load Selected path
//! (unsaved-changes guard included) and applies the preform, the real width and the colour to it as
//! one undoable edit. "Use Color" (`zoning_hooks`) goes through the same functions without the
//! preform.
//!
//! Neither action works while a remote library is active: the designs of a plan are local
//! library entries.

use super::{
    axis::material::{known_material, resolve},
    counts::is_remote,
    format::{design_name, effective_id, group_order},
    host::{Host, on_host},
    library_link::label_prefix,
    run::{DesignStatus, ResultsSource},
    session::Session,
    view::{DesignInfo, rescaled_pose},
};
use crate::{
    MainWindow, RoughPlanModel,
    gui::{
        editor::planned_stone::{self, PlannedStoneRequest},
        is_c_axis_override_available, show_toast,
    },
};
use glam::DVec3;
use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::{
    CrystalAxisPlan, CrystalOrientation, PlanRuleOrigin, PreformSpec,
    rough_plan::{
        PlacedStone, RoughLayout,
        fit::StonePose,
        orientation::{
            PlanSettingsOverrides, ResolveContext, RuleChoice, StoneChoice,
            stone_orientation_in_stone_frame,
        },
        stone_preform_planes,
    },
};
use pleochroma_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::{
    collections::BTreeMap,
    rc::Rc,
    sync::{Arc, Mutex},
};
use tracing::debug;

/// What the toast says when the active library is a remote one.
pub(super) const LOCAL_LIBRARY_FIRST: &str =
    "Switch to the local library first: this needs the design in the library.";

/// Registers the planner's "Open in Editor" callback.
pub(super) fn setup_editor_link_callbacks(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.on_open_in_editor(|result, group| on_host(|host| open_in_editor(host, result, group)));
}

/// The stone a design row names: the first stone of that design in the result.
pub(super) struct PickedStone {
    /// The result's place in the shown results, from 0.
    pub(super) layout_index: u32,
    /// The stone's place in the result, from 0.
    pub(super) stone_index: usize,
    /// The library entry to open: the design's own, or the one a design matched by title
    /// resolved to.
    pub(super) entry_id: i64,
    /// The design's title as the result card shows it.
    pub(super) title: String,
    /// The name of the saved plan the results were loaded from; `None` for results that were
    /// just planned.
    pub(super) plan_name: Option<String>,
    /// Whether the design is not the one that was planned (changed since the plan was saved, or
    /// found again by its title), so the result view scales it to the width the plan recorded
    /// and the stone is placed the same way.
    pub(super) rescale: bool,
    /// The result's layout (a copy; [`resolve_target`] may rescale its stone). When the cutter
    /// set the stone up in the planner (`session_posed`), the stone's pose in it is that one.
    pub(super) layout: RoughLayout,
    /// Whether the cutter gave the stone a pose of their own in the planner (a box pose, Free
    /// Orientation or Re-fit, `session.overrides`), already placed in `layout`. That pose is the
    /// stone's real one and wins over the rough color wizard's stored pose choice.
    pub(super) session_posed: bool,
    /// The Reason the cutter gave for the stone; empty when none.
    pub(super) stone_reason: String,
    /// The rules the plan was made with (the plan's, and those of its designs), for the rule
    /// record the hand-off stores on the design.
    pub(super) rules: PlanSettingsOverrides,
}

/// The first stone of design group `group` of result `result` of the shown results, or `None`
/// when the result or the group does not exist.
pub(super) fn pick_stone(session: &Session, result: usize, group: usize) -> Option<PickedStone> {
    let run = &session.run;
    let mut layout = run.layouts.get(result)?.clone();
    let (planned_id, _) = *group_order(&layout).get(group)?;
    let stone_index = layout
        .stones
        .iter()
        .position(|stone| stone.entry_id == planned_id)?;
    let layout_index = u32::try_from(result).ok()?;
    // The cutter's own setup of the stone in the planner is its real pose: a box pose, a free
    // pose or a re-fit stone (`session.overrides`), as the rows and the 3D view read it.
    let choice = u32::try_from(stone_index)
        .ok()
        .and_then(|index| session.overrides.stone_choice(layout_index, index))
        .cloned();
    let session_posed = choice.as_ref().is_some_and(StoneChoice::is_overridden);
    if let Some(choice) = choice.as_ref().filter(|choice| choice.is_overridden()) {
        let stone = &mut layout.stones[stone_index];
        stone.pose = choice.pose_for(&stone.pose);
    }
    let plan_name = match &run.source {
        ResultsSource::Loaded { name, .. } => Some(name.clone()),
        ResultsSource::Planned => None,
    };
    Some(PickedStone {
        layout_index,
        stone_index,
        entry_id: effective_id(&run.statuses, planned_id),
        title: design_name(&run.titles, planned_id),
        plan_name,
        rescale: matches!(
            run.statuses.get(&planned_id),
            Some(DesignStatus::Changed | DesignStatus::MatchedByTitle { .. })
        ),
        layout,
        session_posed,
        stone_reason: choice.map_or_else(String::new, |choice| choice.reason),
        rules: run.plan_overrides.clone(),
    })
}

/// A picked stone with what both planner actions need of it: its design's placement and widths
/// from the library, and its pose.
pub(super) struct StoneTarget {
    /// The stone.
    pub(super) picked: PickedStone,
    /// The design's placement and widths.
    pub(super) info: DesignInfo,
    /// The stone's pose: the one the cutter chose, at the scale the stone is drawn with.
    pub(super) pose: StonePose,
}

impl StoneTarget {
    /// The stone's record in the layout.
    fn stone(&self) -> &PlacedStone {
        &self.picked.layout.stones[self.picked.stone_index]
    }

    /// The stone's real width in millimetres, the editor's girdle diameter: the design's
    /// axis-aligned width (what the girdle diameter is a measurement of) at the pose's scale.
    pub(super) const fn girdle_mm(&self) -> f64 {
        self.info.width_axis_units * self.pose.mm_per_unit
    }
}

/// The pose the cutter chose for stone `stone_index` of result `layout_index`: the pose they set
/// up in the planner (a box pose, Free Orientation or Re-fit, already in the picked layout), else
/// the stored "Stone orientation" choice when the results carry a colour, else the pose the
/// planner gave the stone.
fn stone_pose_for(
    layout_index: u32,
    stone_index: u32,
    stone: &PlacedStone,
    session_posed: bool,
) -> StonePose {
    if session_posed {
        stone.pose
    } else {
        super::zoning_hooks::chosen_pose(layout_index, stone_index, stone)
    }
}

/// Reads the stone's design from the library (its placement and widths) and gives the stone its
/// pose; `None`, with a toast, when the design cannot be read or has no solid.
///
/// A design the plan was not made with (see [`PickedStone::rescale`]) is scaled the way the result
/// view draws it: so it is as wide as the width the plan recorded for the stone. The library is
/// read here, so the vault must not be locked by the caller.
pub(super) fn resolve_target(main: &MainWindow, mut picked: PickedStone) -> Option<StoneTarget> {
    let Some(info) = super::design_info_of(picked.entry_id) else {
        show_toast(
            main,
            "This design could not be read from the library, so its stone cannot be opened.",
            "error",
        );
        return None;
    };
    if picked.rescale {
        let stone = &mut picked.layout.stones[picked.stone_index];
        stone.pose = rescaled_pose(stone, info.width_units).0;
    }
    let stone_index = u32::try_from(picked.stone_index).unwrap_or(u32::MAX);
    let pose = stone_pose_for(
        picked.layout_index,
        stone_index,
        &picked.layout.stones[picked.stone_index],
        picked.session_posed,
    );
    Some(StoneTarget { picked, info, pose })
}

/// The request that opens the stone `target` in the editor: its design, its real width and
/// `preform`, without a colour.
/// It carries the stone's TRUE crystal orientation ([`stone_axis`]) when the rough has one, so
/// the editor writes it to the design in the same undo step.
pub(super) fn request_for(
    host: &Rc<Host>,
    target: &StoneTarget,
    preform: Option<PreformSpec>,
) -> PlannedStoneRequest {
    let name = host.session.borrow().run.material_name.clone();
    // A catalogue material the axis panel has not looked up yet is read now: the hand-off must
    // not lose the rule record to a lookup that is still running.
    let material = known_material(host, &name).or_else(|| resolve(&host.db, &name));
    let rough = rough_orientation(host, material.as_ref());
    request_from(
        Arc::clone(&host.db),
        rough.as_ref(),
        material.as_ref(),
        target,
        preform,
    )
}

/// [`request_for`] from its parts: the catalogue database, the rough's orientation and the plan's
/// material (`None` when it is not known: the design then gets no rule record).
fn request_from(
    db: Arc<Mutex<Database>>,
    rough: Option<&CrystalOrientation>,
    material: Option<&GemMaterial>,
    target: &StoneTarget,
    preform: Option<PreformSpec>,
) -> PlannedStoneRequest {
    let orientation = stone_axis(rough, target);
    // The rule record goes with the axis: a stone that arrives without one has no rule to keep.
    let axis_plan = orientation
        .as_ref()
        .zip(material)
        .and_then(|(_, material)| plan_record(rough, material, target));
    PlannedStoneRequest {
        entry_id: target.picked.entry_id,
        title: target.picked.title.clone(),
        preform,
        girdle_mm: target.girdle_mm(),
        material: None,
        orientation,
        axis_plan,
        db,
        result_number: target.picked.layout_index as usize + 1,
        stone_number: target.picked.stone_index + 1,
    }
}

/// The crystal orientation of the rough the shown results were planned for: `None` while it is
/// Unknown, for an isotropic material, built-in or custom (nothing to orient: the design must not
/// gain a `[render_effects]` table), and for a material that cannot be resolved (`None`).
fn rough_orientation(host: &Host, material: Option<&GemMaterial>) -> Option<CrystalOrientation> {
    if !is_c_axis_override_available(material?) {
        return None;
    }
    host.session.borrow().run.plan_model.as_ref()?.orientation
}

/// The stone's TRUE crystal orientation in its design's own frame: the rough's orientation
/// carried through the pose the cutter chose, at the scale and placement the stone really has
/// (`stone_orientation_in_stone_frame`), never the rule's target. `None` for an Unknown rough
/// orientation, so such a hand-off writes nothing.
fn stone_axis(
    rough: Option<&CrystalOrientation>,
    target: &StoneTarget,
) -> Option<CrystalOrientation> {
    stone_orientation_in_stone_frame(rough?, &target.pose, &target.info.placement)
}

/// The rule the stone was planned under, for its design file: the rule it was cut under (its
/// design's own, else the plan's, else the material's default; none when the cutter ignored the
/// axis), where that rule came from, and the cutter's Reason (the stone's, else the design's, else
/// the plan's). `None` for an Unknown rough orientation and when there is nothing to record.
fn plan_record(
    rough: Option<&CrystalOrientation>,
    material: &GemMaterial,
    target: &StoneTarget,
) -> Option<CrystalAxisPlan> {
    let rough = rough?;
    let rules = &target.picked.rules;
    // Rules are keyed by the design the plan was made with, not by the entry it resolved to.
    let planned_id = target.stone().entry_id;
    let placements = BTreeMap::new();
    let ctx = ResolveContext {
        rough: Some(*rough),
        material,
        placements: &placements,
    };
    let origin = match rules.designs.get(&planned_id).map(|setting| setting.choice) {
        Some(RuleChoice::Set(_) | RuleChoice::Ignore) => PlanRuleOrigin::Design,
        _ if rules.plan.choice == RuleChoice::Default => PlanRuleOrigin::MaterialDefault,
        _ => PlanRuleOrigin::Plan,
    };
    let stone_reason = Some(target.picked.stone_reason.trim()).filter(|text| !text.is_empty());
    let reason = stone_reason
        .or_else(|| rules.design_reason(planned_id))
        .or_else(|| rules.plan_reason())
        .unwrap_or_default();
    let record = CrystalAxisPlan {
        rule: rules.effective_rule(planned_id, &ctx),
        origin,
        reason: reason.to_owned(),
    };
    (!record.is_blank()).then_some(record)
}

/// Hands `request` to the main window's editor and brings that window forward: the planner may
/// be covering it (and the unsaved-changes guard, if it asks, is in that window).
pub(super) fn open_in_main(main: &MainWindow, request: PlannedStoneRequest) {
    planned_stone::open(main, request);
    main.window().set_minimized(false);
    let _ = main.window().show();
}

/// `Saved plan "Aqua pebble", result 3, stone 1` for a stone of a loaded plan, `Rough plan,
/// result 3, stone 1` for one just planned: where the preform of an opened stone came from, as
/// the Preform tab shows it.
fn planned_source_label(
    plan_name: Option<&str>,
    result_number: usize,
    stone_number: usize,
) -> String {
    format!(
        "{}, result {result_number}, stone {stone_number}",
        label_prefix(plan_name)
    )
}

/// The planes of the rough the results were planned for (rough frame, mm), or `None` with a
/// toast when there are none or they are not usable.
fn rough_halfspaces(host: &Host, main: &MainWindow) -> Option<Vec<(DVec3, f64)>> {
    let plan_model = host.session.borrow().run.plan_model.clone();
    let Some(plan_model) = plan_model else {
        show_toast(
            main,
            "These results have no rough model to take the piece from.",
            "error",
        );
        return None;
    };
    match plan_model.halfspaces() {
        Ok(halfspaces) => Some(halfspaces),
        Err(error) => {
            show_toast(
                main,
                &format!("The rough's shape is not usable: {error}"),
                "error",
            );
            None
        }
    }
}

/// The piece of rough the stone `target` comes from as a preform in the design's frame: the sawn
/// piece box clipped by the rough's planes (`stone_preform_planes`), labelled with where it came
/// from. `None`, with a toast, when the piece is not a closed solid; the design then opens
/// without it.
fn planned_preform(
    main: &MainWindow,
    halfspaces: &[(DVec3, f64)],
    target: &StoneTarget,
) -> Option<PreformSpec> {
    let picked = &target.picked;
    let label = planned_source_label(
        picked.plan_name.as_deref(),
        picked.layout_index as usize + 1,
        picked.stone_index + 1,
    );
    let preform = stone_preform_planes(
        halfspaces,
        target.stone(),
        &target.pose,
        &target.info.placement,
    )
    .and_then(|piece| PreformSpec::from_planes(piece.planes, label));
    if preform.is_none() {
        show_toast(
            main,
            "The planned piece of this stone is not a closed solid; the design opens without it.",
            "warning",
        );
    }
    preform
}

/// "Open in Editor" on design group `group` of result `result`: the first stone of that design
/// opens in the editor with the piece of rough it comes from as the design's preform, its real
/// width as the girdle diameter and, when the plan has a stored rough colour,
/// the adopted colour material. The design is loaded through the normal Load Selected path, so
/// the editor's unsaved-changes guard may ask first.
fn open_in_editor(host: &Rc<Host>, result: i32, group: i32) {
    let Some(main) = host.main.upgrade() else {
        return;
    };
    let (Ok(result), Ok(group)) = (usize::try_from(result), usize::try_from(group)) else {
        return;
    };
    if is_remote(&host.source) {
        show_toast(&main, LOCAL_LIBRARY_FIRST, "info");
        return;
    }
    let picked = pick_stone(&host.session.borrow(), result, group);
    let Some(picked) = picked else {
        show_toast(&main, "This design has no stone to open.", "info");
        return;
    };
    let Some(halfspaces) = rough_halfspaces(host, &main) else {
        return;
    };
    // The design is read from the library, so the vault is not locked here.
    let Some(target) = resolve_target(&main, picked) else {
        return;
    };
    let preform = planned_preform(&main, &halfspaces, &target);
    debug!(
        "Rough planner: opening stone {} of result {} (design #{}) in the editor{}",
        target.picked.stone_index + 1,
        result + 1,
        target.picked.entry_id,
        if preform.is_some() {
            " with its piece of rough"
        } else {
            ""
        }
    );
    let request = PlannedStoneRequest {
        material: super::zoning_hooks::adopt_for_open(host, &target),
        ..request_for(host, &target, preform)
    };
    open_in_main(&main, request);
}

#[cfg(test)]
mod tests_axis;

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::{
        CrystalAxis,
        rough_plan::{
            Axis, CutOrder, CutPlan, DesignPlacement, orientation::stone_orientation_row,
            rough_to_stone_map,
        },
    };

    fn layout_of(ids: &[i64]) -> RoughLayout {
        let stone = |entry_id| PlacedStone {
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
        };
        RoughLayout {
            cut_order: CutOrder::Xyz,
            stones: ids.iter().map(|&id| stone(id)).collect(),
            cut_plan: CutPlan { slabs: Vec::new() },
            total_carat: 0.1,
            total_volume_mm3: 1.0,
            yield_fraction: 0.1,
            exact_fit: false,
        }
    }

    /// Two results: stones 9, 4, 9 (groups: 9 twice, then 4) and stones 4, 12 (4, then 12).
    pub(super) fn session_of_two_results() -> Session {
        let mut session = Session::default();
        session.run.layouts = vec![layout_of(&[9, 4, 9]), layout_of(&[4, 12])];
        session.run.titles = BTreeMap::from([(4, "Oval".to_string())]);
        session
    }

    pub(super) fn target_with(pose: StonePose, placement: DesignPlacement) -> StoneTarget {
        let mut layout = layout_of(&[7]);
        layout.stones[0].pose = pose;
        StoneTarget {
            picked: PickedStone {
                layout_index: 0,
                stone_index: 0,
                entry_id: 7,
                title: "Oval".to_owned(),
                plan_name: None,
                rescale: false,
                layout,
                session_posed: false,
                stone_reason: String::new(),
                rules: PlanSettingsOverrides::default(),
            },
            info: DesignInfo {
                placement,
                width_units: 1.0,
                width_axis_units: 1.0,
            },
            pose,
        }
    }

    pub(super) fn memory_db() -> Arc<Mutex<Database>> {
        Arc::new(Mutex::new(
            Database::new(Some(":memory:")).expect("an in-memory vault"),
        ))
    }

    /// A pose turned about the vertical axis (not the identity, so the hand-off must carry it).
    pub(super) fn turned_pose() -> StonePose {
        StonePose {
            center_mm: [10.0, 0.0, 0.0],
            axes: [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]],
            mm_per_unit: 2.0,
        }
    }

    #[test]
    fn the_request_carries_the_true_orientation_of_the_stone() {
        let placement = DesignPlacement::IDENTITY;
        let target = target_with(turned_pose(), placement);
        let rough = CrystalOrientation::from_angles(90.0, 0.0, None);
        let request = request_from(memory_db(), Some(&rough), None, &target, None);
        let carried = request.orientation.expect("the rough has an orientation");

        // The rough's axis seen through the stone's actual pose.
        let map = rough_to_stone_map(&target.pose, &placement).expect("a proper pose");
        let expected = CrystalAxis::from_direction(map.rotation * rough.axis.direction());
        assert!((carried.axis.tilt_deg() - expected.tilt_deg()).abs() < 1e-9);
        assert!((carried.axis.azimuth_deg() - expected.azimuth_deg()).abs() < 1e-9);
        assert!(
            (carried.axis.azimuth_deg() - rough.axis.azimuth_deg()).abs() > 1.0,
            "the pose turned the axis"
        );
    }

    #[test]
    fn the_hand_off_orientation_equals_the_planners_row() {
        let placement = DesignPlacement::IDENTITY;
        let target = target_with(turned_pose(), placement);
        let rough = CrystalOrientation::from_angles(35.0, 20.0, Some(40.0));
        let sapphire = GemMaterial::by_name("Sapphire").expect("a built-in");
        let request = request_from(memory_db(), Some(&rough), Some(&sapphire), &target, None);
        let row = stone_orientation_row(&rough, &target.pose, &placement, &sapphire, None, None)
            .expect("a usable pose");
        assert_eq!(request.orientation, Some(row.orientation));
    }

    #[test]
    fn an_unknown_rough_orientation_hands_off_nothing() {
        let target = target_with(turned_pose(), DesignPlacement::IDENTITY);
        let sapphire = GemMaterial::by_name("Sapphire").expect("a built-in");
        let request = request_from(memory_db(), None, Some(&sapphire), &target, None);
        assert_eq!(request.orientation, None);
        assert_eq!(request.axis_plan, None, "no axis, no rule record");
    }

    #[test]
    fn the_source_label_names_the_plan_the_result_and_the_stone() {
        assert_eq!(
            planned_source_label(Some("Aqua pebble"), 3, 1),
            "Saved plan \"Aqua pebble\", result 3, stone 1"
        );
        assert_eq!(
            planned_source_label(None, 3, 1),
            "Rough plan, result 3, stone 1"
        );
    }

    #[test]
    fn a_design_row_picks_the_first_stone_of_its_design_in_its_result() {
        let session = session_of_two_results();

        let first = pick_stone(&session, 0, 0).expect("group 0 of result 0");
        assert_eq!(first.layout_index, 0);
        assert_eq!(
            first.stone_index, 0,
            "the first of the two stones of design 9"
        );
        assert_eq!(first.entry_id, 9);
        assert_eq!(first.title, "Design #9", "a design without a title");
        assert_eq!(first.layout.stones.len(), 3);

        let oval = pick_stone(&session, 0, 1).expect("group 1 of result 0");
        assert_eq!((oval.stone_index, oval.entry_id), (1, 4));
        assert_eq!(oval.title, "Oval");

        // The second result has its own order: one stone each, lower id first.
        let second = pick_stone(&session, 1, 1).expect("group 1 of result 1");
        assert_eq!(second.layout_index, 1);
        assert_eq!((second.stone_index, second.entry_id), (1, 12));
        let fourth = pick_stone(&session, 1, 0).expect("group 0 of result 1");
        assert_eq!((fourth.stone_index, fourth.entry_id), (0, 4));
    }

    #[test]
    fn a_row_that_does_not_exist_picks_nothing() {
        let session = session_of_two_results();
        assert!(pick_stone(&session, 2, 0).is_none(), "no third result");
        assert!(pick_stone(&session, 0, 2).is_none(), "no third group");
        assert!(
            pick_stone(&Session::default(), 0, 0).is_none(),
            "no results"
        );
    }

    #[test]
    fn only_a_loaded_plan_has_a_name() {
        let mut session = session_of_two_results();
        assert_eq!(pick_stone(&session, 0, 0).expect("picked").plan_name, None);
        session.run.source = ResultsSource::Loaded {
            plan_id: 3,
            name: "Aqua pebble".to_string(),
            created_at: 0,
        };
        assert_eq!(
            pick_stone(&session, 0, 0)
                .expect("picked")
                .plan_name
                .as_deref(),
            Some("Aqua pebble")
        );
    }

    #[test]
    fn a_design_the_plan_was_not_made_with_is_found_and_rescaled() {
        let mut session = session_of_two_results();
        session.run.statuses = BTreeMap::from([
            (9, DesignStatus::Changed),
            (
                12,
                DesignStatus::MatchedByTitle {
                    resolved_entry_id: 40,
                },
            ),
            (4, DesignStatus::Unchanged),
        ]);
        let changed = pick_stone(&session, 0, 0).expect("picked");
        assert_eq!(changed.entry_id, 9, "the design itself, edited");
        assert!(changed.rescale);

        let matched = pick_stone(&session, 1, 1).expect("picked");
        assert_eq!(matched.entry_id, 40, "the entry its title resolved to");
        assert_eq!(matched.title, "Design #12", "the plan's own record of it");
        assert!(matched.rescale);

        let unchanged = pick_stone(&session, 0, 1).expect("picked");
        assert_eq!(unchanged.entry_id, 4);
        assert!(!unchanged.rescale);
    }
}
