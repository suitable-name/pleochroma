//! What the desktop remembers between a guide's steps: the catalogue of guides (the
//! built-in ones plus any generated at run time), the UI events reported since the
//! current step began, the guide waiting for its starting design, and a handle on the
//! editor state.
//!
//! Slint callbacks all run on the UI thread, so this lives in a `thread_local`. Nothing
//! here calls into Slint, and nothing hands out the `RefCell` borrow: every function takes
//! it, does its work on plain data and lets go, so a handler can never re-enter it.

use crate::gui::editor::{auto_solve, state::EditorState};
use pleochroma::geometry::meet_solver::SolvedTier;
use pleochroma_cut_core::Design;
use pleochroma_editor::guide::{
    GoalContext, Guide, GuideCatalog, Perform, goal_met,
    solving_events::{DEEP_SOLVE_FINISHED, DEEP_SOLVE_STARTED},
};
use std::{cell::RefCell, rc::Rc};

/// What a guide waits for before it can open its first step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LaunchWant {
    /// A new design, made through the New Design callback.
    NewDesign,
    /// This library catalogue entry, opened through Load Selected.
    Library { entry_id: i64 },
}

/// A guide that has asked for its starting design and is waiting for it.
#[derive(Clone, Debug)]
pub(super) struct Launch {
    /// The guide to open once the design is there.
    pub(super) guide_id: String,
    /// The design it asked for.
    pub(super) want: LaunchWant,
    /// Checks made so far, one per timer tick.
    pub(super) ticks: u32,
    /// The New Design callback reported success since the launch began.
    pub(super) design_arrived: bool,
    /// The open design's id when the launch began, to tell a replacement from no change.
    pub(super) uuid_before: String,
}

/// The state of the UI a goal may read besides the design.
#[derive(Clone, Copy, Debug)]
pub(super) struct UiFacts {
    /// The editor's own verdict: solved, and not a problem.
    pub(super) solved_closed: bool,
    /// The Solid viewport's view mode.
    pub(super) view_mode: i32,
    /// The inspector's tab.
    pub(super) inspector_tab: i32,
}

#[derive(Default)]
struct Runtime {
    catalog: GuideCatalog,
    events: Vec<String>,
    entered_step: Option<(String, usize)>,
    launch: Option<Launch>,
    state: Option<Rc<RefCell<EditorState>>>,
}

thread_local! {
    static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::default());
}

fn with<R>(f: impl FnOnce(&mut Runtime) -> R) -> R {
    RUNTIME.with_borrow_mut(f)
}

/// Remembers the editor state, so a UI event can re-check the current step without a
/// handle being passed through every caller.
pub(super) fn install_state(state: &Rc<RefCell<EditorState>>) {
    with(|rt| rt.state = Some(Rc::clone(state)));
}

/// The editor state, once [`install_state`] has run.
pub(super) fn state() -> Option<Rc<RefCell<EditorState>>> {
    with(|rt| rt.state.clone())
}

/// A copy of the guide with this id.
pub(super) fn guide(id: &str) -> Option<Guide> {
    with(|rt| rt.catalog.get(id).cloned())
}

/// What Next does on step `index` of guide `guide_id` while it is unfinished, if anything.
pub(super) fn step_perform(guide_id: &str, index: usize) -> Option<Perform> {
    with(|rt| {
        rt.catalog
            .get(guide_id)
            .and_then(|guide| guide.steps.get(index))
            .and_then(|step| step.perform.clone())
    })
}

/// Reads the catalogue.
pub(super) fn with_catalog<R>(f: impl FnOnce(&GuideCatalog) -> R) -> R {
    with(|rt| f(&rt.catalog))
}

/// Adds a guide generated at run time (see `GuideCatalog::add_generated`).
pub(super) fn register_generated(guide: Guide) -> Result<(), String> {
    with(|rt| rt.catalog.add_generated(guide))
}

/// Notes that a UI event happened during the current step.
pub(super) fn record_event(name: &str) {
    with(|rt| {
        if name == DEEP_SOLVE_STARTED {
            rt.events.retain(|event| event != DEEP_SOLVE_FINISHED);
        }
        if !rt.events.iter().any(|event| event == name) {
            rt.events.push(name.to_owned());
        }
    });
}

/// Enters a step, retaining a completed Deep Solve only across its matching next step.
pub(super) fn enter_step(guide_id: &str, index: i32, revisiting: bool) {
    with(|rt| {
        let Some(index) = usize::try_from(index).ok() else {
            rt.events.clear();
            rt.entered_step = None;
            return;
        };
        let Some(guide) = rt.catalog.get(guide_id) else {
            rt.events.clear();
            rt.entered_step = None;
            return;
        };
        let current_goal = guide.steps.get(index).map(|step| &step.goal);
        let previous = rt.entered_step.as_ref();
        let preserve_deep_solve = !revisiting
            && previous.is_some_and(|(previous_id, previous_index)| {
                previous_id == guide_id && previous_index.checked_add(1) == Some(index)
            })
            && previous
                .and_then(|(_, previous_index)| guide.steps.get(*previous_index))
                .is_some_and(|step| step.goal.events().contains(&DEEP_SOLVE_STARTED))
            && current_goal.is_some_and(|goal| goal.events().contains(&DEEP_SOLVE_FINISHED))
            && rt.events.iter().any(|event| event == DEEP_SOLVE_STARTED)
            && rt.events.iter().any(|event| event == DEEP_SOLVE_FINISHED);

        if preserve_deep_solve {
            rt.events.retain(|event| event == DEEP_SOLVE_FINISHED);
        } else {
            rt.events.clear();
        }
        rt.entered_step = Some((guide_id.to_owned(), index));
    });
}

/// Forgets the events and step tracking when a guide starts or closes.
pub(super) fn clear_events() {
    with(|rt| {
        rt.events.clear();
        rt.entered_step = None;
    });
}

/// The solved depth of every tier, from the shared cache of the last solve that completed,
/// when that solve covers `tiers` tiers.
///
/// The cache is tagged with the editor generation it was solved for, but a guide check has
/// no generation of its own (see `native_io::solve::cached_solve_matching`, which settles
/// for the same tier-count test), and a check that runs a moment before the cache is written
/// is repeated by the guide's timer, so a depth list one solve behind is corrected at once.
fn masts_if_aligned(cache: Option<&(u64, Vec<SolvedTier>)>, tiers: usize) -> Option<Vec<f64>> {
    let (_, solved) = cache?;
    (solved.len() == tiers).then(|| solved.iter().map(|tier| tier.mast).collect())
}

/// [`masts_if_aligned`] over the live cache.
fn cached_masts(design: &Design) -> Option<Vec<f64>> {
    let cache = auto_solve::solid_last_solved()?;
    let guard = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    masts_if_aligned(guard.as_ref(), design.tiers.len())
}

/// Whether step `step` of guide `guide_id` has reached its goal.
///
/// A goal that compares solved depths (a "Build this design" lesson's last step) is given
/// the cached depths of the last solve, and only once the editor reports a closed solid.
pub(super) fn goal_is_met(guide_id: &str, step: usize, design: &Design, facts: UiFacts) -> bool {
    with(|rt| {
        let Some(step) = rt
            .catalog
            .get(guide_id)
            .and_then(|guide| guide.steps.get(step))
        else {
            return false;
        };
        let masts = if facts.solved_closed && step.goal.wants_solved_masts() {
            cached_masts(design)
        } else {
            None
        };
        let mut ctx = GoalContext::new(design)
            .solved_closed(facts.solved_closed)
            .events(&rt.events)
            .view_mode(facts.view_mode)
            .inspector_tab(facts.inspector_tab);
        if let Some(masts) = masts.as_deref() {
            ctx = ctx.solved_masts(masts);
        }
        goal_met(&step.goal, &ctx)
    })
}

/// Starts waiting for a guide's starting design (replacing any earlier wait).
pub(super) fn set_launch(launch: Option<Launch>) {
    with(|rt| rt.launch = launch);
}

/// A copy of the launch in progress, if any.
pub(super) fn launch() -> Option<Launch> {
    with(|rt| rt.launch.clone())
}

/// Counts one more check against the launch in progress.
pub(super) fn count_launch_tick() {
    with(|rt| {
        if let Some(launch) = rt.launch.as_mut() {
            launch.ticks = launch.ticks.saturating_add(1);
        }
    });
}

/// Notes that the New Design callback succeeded, which is what a new-design launch waits
/// for. Does nothing when no such launch is in progress.
pub(super) fn mark_new_design_arrived() {
    with(|rt| {
        if let Some(Launch {
            want: LaunchWant::NewDesign,
            design_arrived,
            ..
        }) = rt.launch.as_mut()
        {
            *design_arrived = true;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::geometry::meet_solver::SolveStrategy;
    use pleochroma_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};
    use pleochroma_editor::guide::{
        COMPARE_STEP_TITLE, Goal, GuideCategory, GuideStep, build_this_design_guide,
    };

    fn deep_solve_transition_guide() -> String {
        let id = "runtime-deep-solve-transition";
        let guide = Guide::new(
            id,
            "Deep Solve transition",
            "Exercise the desktop event handoff.",
            GuideCategory::Solving,
        )
        .step(
            GuideStep::new("Run", "Start a Deep Solve.")
                .actions(["Click Deep Solve."])
                .check("The Deep Solve run starts.")
                .goal(Goal::Event(DEEP_SOLVE_STARTED.into()), "a run started"),
        )
        .step(
            GuideStep::new("Wait", "Wait for the result.")
                .actions(["Wait for the run to finish."])
                .check("The run's result is shown.")
                .goal(Goal::Event(DEEP_SOLVE_FINISHED.into()), "the run finished"),
        );
        register_generated(guide).expect("test guide registered");
        id.to_owned()
    }

    fn events() -> Vec<String> {
        with(|rt| rt.events.clone())
    }

    fn solved(masts: &[f64]) -> (u64, Vec<SolvedTier>) {
        let tiers = masts
            .iter()
            .map(|&mast| SolvedTier {
                mast,
                strategy: SolveStrategy::ScaleReference,
                detail: String::new(),
            })
            .collect();
        (3, tiers)
    }

    #[test]
    fn cached_depths_are_used_only_when_they_cover_the_design() {
        let cache = solved(&[1.0, 0.5]);
        assert_eq!(masts_if_aligned(Some(&cache), 2), Some(vec![1.0, 0.5]));
        assert_eq!(masts_if_aligned(Some(&cache), 3), None);
        assert_eq!(masts_if_aligned(Some(&cache), 0), None);
        assert_eq!(masts_if_aligned(None, 2), None);
    }

    #[test]
    fn a_depth_goal_does_not_complete_without_a_closed_solve_and_cached_depths() {
        let design = Design::new(
            PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        let guide = build_this_design_guide(&design, None, "Round brilliant", "test-key")
            .expect("a lesson for the round brilliant");
        let compare = guide
            .steps
            .iter()
            .position(|step| step.title == COMPARE_STEP_TITLE)
            .expect("a compare step");
        let id = guide.id.clone();
        register_generated(guide).expect("a fit lesson");
        let facts = |solved_closed| UiFacts {
            solved_closed,
            view_mode: 0,
            inspector_tab: 0,
        };
        // The design is the lesson's own original, so every cut matches; what is missing is
        // the solve. Not solved: no. Solved, but no depths cached (no auto-solve ran): no.
        assert!(!goal_is_met(&id, compare, &design, facts(false)));
        assert!(!goal_is_met(&id, compare, &design, facts(true)));
    }

    #[test]
    fn a_goal_of_an_unknown_guide_is_never_met() {
        let design = Design::new(
            PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
            ScheduleMeta::standard_round_brilliant(),
            Vec::new(),
        );
        let facts = UiFacts {
            solved_closed: true,
            view_mode: 0,
            inspector_tab: 0,
        };
        assert!(!goal_is_met("no-such-guide", 0, &design, facts));
    }

    #[test]
    fn a_fast_deep_solve_completion_survives_its_matching_forward_step() {
        let id = deep_solve_transition_guide();
        clear_events();
        enter_step(&id, 0, false);
        record_event(DEEP_SOLVE_STARTED);
        record_event(DEEP_SOLVE_FINISHED);
        enter_step(&id, 1, false);
        assert_eq!(events(), [DEEP_SOLVE_FINISHED]);
    }

    #[test]
    fn a_deep_solve_finishing_after_step_entry_is_recorded_normally() {
        let id = deep_solve_transition_guide();
        clear_events();
        enter_step(&id, 0, false);
        record_event(DEEP_SOLVE_STARTED);
        enter_step(&id, 1, false);
        record_event(DEEP_SOLVE_FINISHED);
        assert_eq!(events(), [DEEP_SOLVE_FINISHED]);
    }

    #[test]
    fn back_navigation_clears_a_carried_deep_solve_completion() {
        let id = deep_solve_transition_guide();
        clear_events();
        enter_step(&id, 0, false);
        record_event(DEEP_SOLVE_STARTED);
        record_event(DEEP_SOLVE_FINISHED);
        enter_step(&id, 1, false);
        enter_step(&id, 0, true);
        enter_step(&id, 1, false);
        assert_eq!(events().len(), 0);
    }

    #[test]
    fn skipping_without_a_start_does_not_carry_a_finish() {
        let id = deep_solve_transition_guide();
        clear_events();
        enter_step(&id, 0, false);
        record_event(DEEP_SOLVE_FINISHED);
        enter_step(&id, 1, false);
        assert_eq!(events().len(), 0);
    }

    #[test]
    fn a_repeated_start_invalidates_an_older_finish() {
        let id = deep_solve_transition_guide();
        clear_events();
        enter_step(&id, 0, false);
        record_event(DEEP_SOLVE_STARTED);
        record_event(DEEP_SOLVE_FINISHED);
        record_event(DEEP_SOLVE_STARTED);
        enter_step(&id, 1, false);
        assert_eq!(events().len(), 0);
    }
}
