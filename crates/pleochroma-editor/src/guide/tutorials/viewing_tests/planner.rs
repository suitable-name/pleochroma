//! Walks of the lessons in `viewing/planner.rs`.

use super::{Sim, event, guide_named, read, viewing_events as events, walk};
use crate::guide::{GoalContext, StartingState, goal_met};

#[test]
fn model_the_rough_opens_the_planner_then_shapes_looks_cuts_and_undoes() {
    walk(
        "library-planner-model-rough",
        vec![
            event(events::ROUGH_PLANNER_OPENED),
            event(events::ROUGH_SHAPE_CHOSEN),
            event(events::ROUGH_VIEW_MOVED),
            event(events::ROUGH_CUT_FROM_VIEW),
            event(events::ROUGH_MODEL_UNDONE),
            read(),
        ],
    );
}

#[test]
fn the_results_are_planned_then_picked_opened_and_found_in_the_library() {
    walk(
        "library-planner-results",
        vec![
            event(events::ROUGH_PLAN_FINISHED),
            event(events::ROUGH_RESULT_SELECTED),
            event(events::ROUGH_DESIGN_SELECTED),
            event(events::ROUGH_CUT_PLAN_OPENED),
            read(),
            event(events::ROUGH_SHOWN_IN_LIBRARY),
        ],
    );
}

#[test]
fn a_plan_is_saved_opened_and_exported() {
    walk(
        "library-planner-saved-plans",
        vec![
            event(events::ROUGH_PLAN_FINISHED),
            event(events::ROUGH_PLAN_SAVED),
            event(events::ROUGH_SAVED_PLAN_OPENED),
            event(events::ROUGH_PLAN_EXPORTED),
            read(),
        ],
    );
}

/// The Preform tab step reads the inspector tab, which the walk helper cannot set, so this lesson
/// is played by hand.
#[test]
fn a_planned_stone_is_opened_and_its_piece_is_found_on_the_preform_tab() {
    let guide = guide_named("library-planner-open-in-editor");
    assert_eq!(guide.steps.len(), 4);
    let mut sim = Sim::start(&guide.starting_state);

    assert!(!sim.met(&guide.steps[0].goal));
    sim.fire(events::ROUGH_PLAN_FINISHED);
    assert!(sim.met(&guide.steps[0].goal));

    sim.events.clear();
    assert!(!sim.met(&guide.steps[1].goal));
    sim.fire(events::ROUGH_STONE_OPENED);
    assert!(sim.met(&guide.steps[1].goal));

    let preform = &guide.steps[2].goal;
    let design = &sim.session.design;
    assert!(
        !goal_met(preform, &GoalContext::new(design).inspector_tab(0)),
        "another tab is not the Preform tab"
    );
    assert!(goal_met(
        preform,
        &GoalContext::new(design).inspector_tab(1)
    ));

    assert!(
        guide.steps[3].is_manual(),
        "the last step is a reading step"
    );
}

#[test]
fn an_event_belongs_to_its_own_step() {
    let guide = guide_named("library-planner-results");
    let mut sim = Sim::start(&guide.starting_state);
    sim.fire(events::ROUGH_PLAN_FINISHED);
    for later in 1..=3 {
        assert!(
            !sim.met(&guide.steps[later].goal),
            "a finished plan is not a result picked, a design picked or a Cut Plan opened"
        );
    }
    let saved = guide_named("library-planner-saved-plans");
    let mut sim = Sim::start(&saved.starting_state);
    sim.fire(events::ROUGH_PLAN_SAVED);
    assert!(
        !sim.met(&saved.steps[2].goal),
        "saving a plan is not opening one"
    );
    assert!(
        !sim.met(&saved.steps[3].goal),
        "saving a plan is not exporting one"
    );
}

#[test]
fn the_planner_lessons_start_from_the_open_design_and_name_no_outline() {
    for id in [
        "library-planner-model-rough",
        "library-planner-results",
        "library-planner-saved-plans",
        "library-planner-open-in-editor",
    ] {
        let guide = guide_named(id);
        assert_eq!(guide.starting_state, StartingState::CurrentDesign, "{id}");
        for step in &guide.steps {
            // The planner is a window of its own: only the editor's Preform tab can be outlined.
            assert!(
                step.highlight_target.is_empty() || step.highlight_target == "preform_tab",
                "{id}, {:?}: {:?} cannot be drawn in the planner window",
                step.title,
                step.highlight_target
            );
        }
    }
}
