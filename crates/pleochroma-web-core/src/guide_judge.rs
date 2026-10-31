//! Judging a lesson step from state: the web's [`GoalContext`], and the web's reading of a
//! step's stage ("Show Me Where").
//!
//! Pure functions over plain values, so they are tested natively; `editor::guide` reads the
//! window and the app state and passes the numbers in.
//!
//! # The web's tabs, in the lessons' numbers
//!
//! The lessons count views the desktop's way (`Goal::ViewMode`: 0 Solid, 1 Path-Traced, 2 Solid
//! with Path-Traced, 3 Diagram, 4 Ray-Traced, 5 Solid with Ray-Traced) and inspector tabs by
//! number (0 Tier, 1 Preform, 2 Optimize, 3 Schedule, 4 History). The browser has three view
//! tabs (0 Render, 1 Solid, 2 Diagram) and the same five inspector tabs. The Render tab shows
//! either renderer, so it stands for both view modes 1 and 4.

use pleochroma_cut_core::{Design, PreformSpec, ScheduleMeta};
use pleochroma_editor::guide::{
    Goal, GoalContext, Guide, GuideStep, Stage, StageView, goal_met, stage_for,
};

/// The web's Render, Solid and Diagram view tabs (`AppModel.view-tab`).
pub const RENDER_TAB: i32 = 0;
/// See [`RENDER_TAB`].
pub const SOLID_TAB: i32 = 1;
/// See [`RENDER_TAB`].
pub const DIAGRAM_TAB: i32 = 2;

/// What the window shows besides the design, as numbers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiFacts {
    /// The editor's own solve verdict is "solved" and not a problem.
    pub solved_closed: bool,
    /// `AppModel.view-tab`.
    pub view_tab: i32,
    /// `InspectorModel.tab`.
    pub inspector_tab: i32,
    /// The solved depth of every tier of the design, parallel to its tiers. Only supplied when
    /// the step's goal compares depths (`Goal::wants_solved_masts`) and the solve is closed.
    pub masts: Option<Vec<f64>>,
}

/// The lessons' view modes the web's view tab `view_tab` stands for.
#[must_use]
pub const fn view_modes(view_tab: i32) -> &'static [i32] {
    match view_tab {
        SOLID_TAB => &[0],
        DIAGRAM_TAB => &[3],
        _ => &[1, 4],
    }
}

/// The browser's tab (Render, Solid, Diagram) that shows the shared view mode `mode`.
#[must_use]
pub fn tab_for_view_mode(mode: i32) -> Option<i32> {
    [RENDER_TAB, SOLID_TAB, DIAGRAM_TAB]
        .into_iter()
        .find(|&tab| view_modes(tab).contains(&mode))
}

/// Whether a goal can be judged with no design open.
///
/// Such a goal reads only events and the tabs. Any other goal is not met without a design (a placeholder design would match a goal such as
/// `Goal::FreshDesign` by accident).
#[must_use]
pub fn is_design_free(goal: &Goal) -> bool {
    match goal {
        Goal::Event(_) | Goal::ViewMode(_) | Goal::InspectorTab(_) => true,
        Goal::All(goals) | Goal::Any(goals) => goals.iter().all(is_design_free),
        _ => false,
    }
}

/// A design to hand [`GoalContext`] when none is open and the goal is [`is_design_free`].
fn placeholder_design() -> Design {
    Design::new(
        PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        ScheduleMeta::standard_round_brilliant(),
        Vec::new(),
    )
}

/// Whether `step`'s goal is met by `design` (when one is open), the UI `facts` and the `events`
/// reported since the step began.
///
/// A reading step (`Goal::Manual`) is never met from state. A goal that compares the Render
/// views is met when the Render tab shows, whichever renderer it uses.
#[must_use]
pub fn goal_is_met(
    step: &GuideStep,
    design: Option<&Design>,
    facts: &UiFacts,
    events: &[String],
) -> bool {
    let placeholder;
    let design = if let Some(design) = design {
        design
    } else if is_design_free(&step.goal) {
        placeholder = placeholder_design();
        &placeholder
    } else {
        return false;
    };
    let mut ctx = GoalContext::new(design)
        .solved_closed(facts.solved_closed)
        .events(events)
        .inspector_tab(facts.inspector_tab);
    if let Some(masts) = facts.masts.as_deref() {
        ctx = ctx.solved_masts(masts);
    }
    view_modes(facts.view_tab)
        .iter()
        .any(|&mode| goal_met(&step.goal, &ctx.view_mode(mode)))
}

/// Which view tab(s) a step's stage asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewWish {
    /// No preference: the step works in any tab.
    Any,
    /// The Render tab.
    Render,
    /// The Solid tab or the Diagram tab (the desktop's "Edit" view).
    Edit,
    /// The Solid tab.
    Solid,
    /// The Diagram tab.
    Diagram,
}

impl ViewWish {
    /// Whether view tab `tab` is what the step asks for.
    #[must_use]
    pub const fn fits(self, tab: i32) -> bool {
        match self {
            Self::Any => true,
            Self::Render => tab == RENDER_TAB,
            Self::Edit => tab == SOLID_TAB || tab == DIAGRAM_TAB,
            Self::Solid => tab == SOLID_TAB,
            Self::Diagram => tab == DIAGRAM_TAB,
        }
    }

    /// The tab to switch to, or `None` when any tab will do.
    #[must_use]
    pub const fn target(self) -> Option<i32> {
        match self {
            Self::Any => None,
            Self::Render => Some(RENDER_TAB),
            Self::Edit | Self::Solid => Some(SOLID_TAB),
            Self::Diagram => Some(DIAGRAM_TAB),
        }
    }

    /// What the hint says to do.
    const fn action(self) -> &'static str {
        match self {
            Self::Any => "",
            Self::Render => "switch to the Render tab",
            Self::Edit | Self::Solid => "switch to the Solid tab",
            Self::Diagram => "switch to the Diagram tab",
        }
    }
}

/// What a step's stage asks of the window, in the web's terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wanted {
    /// The view tab.
    pub view: ViewWish,
    /// The inspector tab, if the step asks for one.
    pub inspector_tab: Option<i32>,
}

/// Whether `goal` reads the inspector tab. Such a step's stage names the tab before the one the
/// goal waits for, so the stage must not insist on it.
fn reads_inspector_tab(goal: &Goal) -> bool {
    match goal {
        Goal::InspectorTab(_) => true,
        Goal::All(goals) | Goal::Any(goals) => goals.iter().any(reads_inspector_tab),
        _ => false,
    }
}

/// The inspector tab a highlight target lives on in the browser when the shared stage names none:
/// the desktop's Deep Solve button sits in the command bar, the web's in the Optimize tab.
fn web_inspector_tab_of(target: &str) -> Option<i32> {
    match target {
        "deep_solve_button" => Some(2),
        _ => None,
    }
}

/// The view and inspector tab the stage `stage` of `step` asks for. The desktop's library panel,
/// accordion sections, Parameters dock and camera pose have no web counterpart and are ignored.
#[must_use]
pub fn wanted_of(stage: &Stage, step: &GuideStep) -> Wanted {
    let view = match (stage.view, stage.solid_view_mode) {
        (StageView::LiveRender, _) => ViewWish::Render,
        (StageView::Edit | StageView::Either, Some(3)) => ViewWish::Diagram,
        (StageView::Edit | StageView::Either, Some(0)) => ViewWish::Solid,
        (StageView::Edit, _) => ViewWish::Edit,
        (StageView::Either, _) => ViewWish::Any,
    };
    let inspector_tab = if reads_inspector_tab(&step.goal) {
        None
    } else {
        stage
            .inspector_tab
            .or_else(|| web_inspector_tab_of(&step.highlight_target))
    };
    Wanted {
        view,
        inspector_tab,
    }
}

/// What step `index` of `guide` asks of the window.
#[must_use]
pub fn wanted(guide: &Guide, index: usize) -> Wanted {
    guide.steps.get(index).map_or(
        Wanted {
            view: ViewWish::Any,
            inspector_tab: None,
        },
        |step| wanted_of(&stage_for(guide, index), step),
    )
}

/// The name of inspector tab `tab`, as the web labels it.
const fn inspector_tab_name(tab: i32) -> &'static str {
    match tab {
        0 => "Tier",
        1 => "Preform",
        2 => "Optimize",
        3 => "Schedule",
        4 => "History",
        _ => "inspector",
    }
}

/// What the window shows of the stage: the view tab, the inspector tab, whether the dock's
/// inspector is folded away and whether the Render tab may be used (the step's lock groups).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    /// `AppModel.view-tab`.
    pub view_tab: i32,
    /// `InspectorModel.tab`.
    pub inspector_tab: i32,
    /// `InspectorModel.collapsed`.
    pub inspector_collapsed: bool,
    /// `GuideModel.allows-view-tabs()`.
    pub render_allowed: bool,
}

/// Joins the things the learner must do, e.g. "Switch to the Solid tab and open the Preform tab".
fn hint(actions: &[String]) -> String {
    let mut text = String::new();
    for (i, action) in actions.iter().enumerate() {
        if i == 0 {
            let mut chars = action.chars();
            if let Some(first) = chars.next() {
                text.extend(first.to_uppercase());
                text.push_str(chars.as_str());
            }
        } else if i + 1 == actions.len() {
            text.push_str(" and ");
            text.push_str(action);
        } else {
            text.push_str(", ");
            text.push_str(action);
        }
    }
    text
}

/// What stands between the window and `wanted`: the hint for "Show Me Where", or `None` when
/// the window already shows what the step asks for.
///
/// A step that asks for the Render tab while the step locks that tab asks for nothing the
/// learner could do, so it is not reported.
#[must_use]
pub fn mismatch(wanted: &Wanted, shown: &Shown) -> Option<String> {
    let mut actions: Vec<String> = Vec::new();
    let view_blocked = wanted.view == ViewWish::Render && !shown.render_allowed;
    if !wanted.view.fits(shown.view_tab) && !view_blocked {
        actions.push(wanted.view.action().to_owned());
    }
    if let Some(tab) = wanted.inspector_tab
        && (shown.inspector_tab != tab || shown.inspector_collapsed)
    {
        actions.push(format!("open the {} tab", inspector_tab_name(tab)));
    }
    (!actions.is_empty()).then(|| hint(&actions))
}

/// The tabs "Show Me Where" switches to: the view tab and the inspector tab, each `None` when it
/// is already right.
#[must_use]
pub fn moves(wanted: &Wanted, shown: &Shown) -> (Option<i32>, Option<i32>) {
    let view = wanted.view.target().filter(|&tab| {
        !wanted.view.fits(shown.view_tab) && (tab != RENDER_TAB || shown.render_allowed)
    });
    let inspector = wanted
        .inspector_tab
        .filter(|&tab| shown.inspector_tab != tab);
    (view, inspector)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::ConstraintTier;
    use pleochroma_editor::guide::{
        MANUAL, NEW_DESIGN_CREATED, WORKED_EXAMPLE_ID, WebFeatures, steps::STEPS,
        worked_example_guide,
    };

    #[test]
    fn a_view_mode_has_a_tab_exactly_when_the_plan_accepts_it() {
        use crate::guide_perform::plan;
        use pleochroma_editor::guide::Perform;
        for mode in 0..=5 {
            assert_eq!(
                tab_for_view_mode(mode).is_some(),
                plan(&Perform::ViewMode(mode)).is_ok(),
                "view mode {mode}"
            );
        }
        assert_eq!(tab_for_view_mode(0), Some(SOLID_TAB));
        assert_eq!(tab_for_view_mode(3), Some(DIAGRAM_TAB));
        assert_eq!(tab_for_view_mode(1), Some(RENDER_TAB));
        assert_eq!(tab_for_view_mode(4), Some(RENDER_TAB));
        assert_eq!(tab_for_view_mode(2), None);
    }

    fn facts(view_tab: i32, inspector_tab: i32) -> UiFacts {
        UiFacts {
            view_tab,
            inspector_tab,
            ..UiFacts::default()
        }
    }

    fn design_with_girdle() -> Design {
        let mut design = Design::new(
            PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        design.material.name = Some("Diamond".to_owned());
        design
    }

    fn step_with(goal: Goal) -> GuideStep {
        GuideStep::new("Step", "Do it.").goal(goal, "it")
    }

    #[test]
    fn the_render_tab_stands_for_both_renderers() {
        assert_eq!(view_modes(RENDER_TAB), [1, 4]);
        assert_eq!(view_modes(SOLID_TAB), [0]);
        assert_eq!(view_modes(DIAGRAM_TAB), [3]);
        let design = design_with_girdle();
        let path = step_with(Goal::Any(vec![Goal::ViewMode(1), Goal::ViewMode(2)]));
        let ray = step_with(Goal::Any(vec![Goal::ViewMode(4), Goal::ViewMode(5)]));
        let diagram = step_with(Goal::ViewMode(3));
        let solid = step_with(Goal::ViewMode(0));
        for (tab, expected) in [
            (RENDER_TAB, [true, true, false, false]),
            (SOLID_TAB, [false, false, false, true]),
            (DIAGRAM_TAB, [false, false, true, false]),
        ] {
            let got = [&path, &ray, &diagram, &solid]
                .map(|step| goal_is_met(step, Some(&design), &facts(tab, 0), &[]));
            assert_eq!(got, expected, "view tab {tab}");
        }
    }

    #[test]
    fn show_me_where_for_deep_solve_opens_the_optimize_tab() {
        let step = GuideStep::new("Run it", "Run Deep Solve.").highlight("deep_solve_button");
        let wanted = wanted_of(&Stage::default(), &step);
        assert_eq!(wanted.inspector_tab, Some(2));
        let plain = GuideStep::new("Read", "Read it.");
        assert_eq!(wanted_of(&Stage::default(), &plain).inspector_tab, None);
    }

    #[test]
    fn the_inspector_tab_is_compared_as_it_is() {
        let design = design_with_girdle();
        let preform = step_with(Goal::InspectorTab(1));
        assert!(goal_is_met(&preform, Some(&design), &facts(1, 1), &[]));
        assert!(!goal_is_met(&preform, Some(&design), &facts(1, 0), &[]));
    }

    #[test]
    fn a_reading_step_is_never_met_from_state() {
        let design = design_with_girdle();
        let reading = GuideStep::new("Read", "Read it.");
        assert!(!goal_is_met(&reading, Some(&design), &facts(1, 0), &[]));
        assert!(!goal_is_met(&reading, None, &facts(1, 0), &[]));
    }

    #[test]
    fn an_event_goal_needs_the_event_and_no_design() {
        let waits = step_with(Goal::Event(NEW_DESIGN_CREATED.to_owned()));
        let seen = [NEW_DESIGN_CREATED.to_owned()];
        assert!(!goal_is_met(&waits, None, &facts(1, 0), &[]));
        assert!(goal_is_met(&waits, None, &facts(1, 0), &seen));
        let design = design_with_girdle();
        assert!(goal_is_met(&waits, Some(&design), &facts(1, 0), &seen));
        assert!(!goal_is_met(&waits, Some(&design), &facts(1, 0), &[]));
    }

    #[test]
    fn a_goal_about_the_design_is_not_met_without_one() {
        let fresh = step_with(Goal::FreshDesign {
            gear_teeth: 96,
            symmetry_order: 8,
            mirror: true,
        });
        assert!(!goal_is_met(&fresh, None, &facts(1, 0), &[]));
        let tier = step_with(Goal::tier("G1", 90.0));
        assert!(!goal_is_met(&tier, None, &facts(1, 0), &[]));
        assert!(!is_design_free(&Goal::SolvedClosed));
        assert!(!is_design_free(&Goal::All(vec![
            Goal::ViewMode(1),
            Goal::Material("Diamond".to_owned())
        ])));
        assert!(is_design_free(&Goal::All(vec![
            Goal::Event(NEW_DESIGN_CREATED.to_owned()),
            Goal::ViewMode(3)
        ])));
    }

    #[test]
    fn the_solve_verdict_and_the_material_come_from_the_facts_and_the_design() {
        let design = design_with_girdle();
        let solved = step_with(Goal::SolvedClosed);
        let mut shown = facts(1, 0);
        assert!(!goal_is_met(&solved, Some(&design), &shown, &[]));
        shown.solved_closed = true;
        assert!(goal_is_met(&solved, Some(&design), &shown, &[]));
        let diamond = step_with(Goal::Material("diamond".to_owned()));
        assert!(goal_is_met(&diamond, Some(&design), &shown, &[]));
        let ruby = step_with(Goal::Material("Ruby".to_owned()));
        assert!(!goal_is_met(&ruby, Some(&design), &shown, &[]));
    }

    #[test]
    fn the_walkthrough_keeps_its_ten_steps_locks_and_goals_on_the_web() {
        let catalogue = pleochroma_editor::guide::GuideCatalog::new();
        let web = catalogue
            .web_guide(WORKED_EXAMPLE_ID, WebFeatures::ALL)
            .expect("the walkthrough is offered");
        assert_eq!(web.steps.len(), STEPS.len());
        for (index, (step, shared)) in web.steps.iter().zip(STEPS).enumerate() {
            assert_eq!(step.title, shared.title, "step {index}");
            // The web may only add a lock group's worth of freedom (the Render tab), never lock more.
            assert!(
                shared.allow.iter().all(|group| step.allow.contains(group)),
                "the locks of step {index}"
            );
            assert_eq!(
                step.is_manual(),
                shared.completion == MANUAL,
                "step {index} is manual exactly where the shared step is"
            );
        }
        let first = &web.steps[0];
        assert_eq!(first.goal.events(), [NEW_DESIGN_CREATED]);
        // It is the same guide the desktop shows, reworded: same number of steps.
        assert_eq!(worked_example_guide().steps.len(), web.steps.len());
    }

    #[test]
    fn a_wish_for_a_view_knows_which_tabs_fit() {
        assert!(ViewWish::Any.fits(RENDER_TAB) && ViewWish::Any.fits(DIAGRAM_TAB));
        assert!(ViewWish::Render.fits(RENDER_TAB) && !ViewWish::Render.fits(SOLID_TAB));
        assert!(ViewWish::Edit.fits(SOLID_TAB) && ViewWish::Edit.fits(DIAGRAM_TAB));
        assert!(!ViewWish::Edit.fits(RENDER_TAB));
        assert!(ViewWish::Solid.fits(SOLID_TAB) && !ViewWish::Solid.fits(DIAGRAM_TAB));
        assert!(ViewWish::Diagram.fits(DIAGRAM_TAB) && !ViewWish::Diagram.fits(SOLID_TAB));
        assert_eq!(ViewWish::Any.target(), None);
        assert_eq!(ViewWish::Edit.target(), Some(SOLID_TAB));
        assert_eq!(ViewWish::Render.target(), Some(RENDER_TAB));
    }

    fn shown(view_tab: i32, inspector_tab: i32) -> Shown {
        Shown {
            view_tab,
            inspector_tab,
            inspector_collapsed: false,
            render_allowed: true,
        }
    }

    #[test]
    fn a_step_on_the_wrong_tab_is_reported_with_what_to_do() {
        let wanted = Wanted {
            view: ViewWish::Edit,
            inspector_tab: Some(1),
        };
        assert_eq!(mismatch(&wanted, &shown(SOLID_TAB, 1)), None);
        assert_eq!(mismatch(&wanted, &shown(DIAGRAM_TAB, 1)), None);
        assert_eq!(
            mismatch(&wanted, &shown(RENDER_TAB, 1)).as_deref(),
            Some("Switch to the Solid tab")
        );
        assert_eq!(
            mismatch(&wanted, &shown(RENDER_TAB, 0)).as_deref(),
            Some("Switch to the Solid tab and open the Preform tab")
        );
        assert_eq!(
            mismatch(&wanted, &shown(SOLID_TAB, 3)).as_deref(),
            Some("Open the Preform tab")
        );
        let folded = Shown {
            inspector_collapsed: true,
            ..shown(SOLID_TAB, 1)
        };
        assert_eq!(
            mismatch(&wanted, &folded).as_deref(),
            Some("Open the Preform tab"),
            "a folded inspector shows no tab"
        );
    }

    #[test]
    fn the_render_tab_is_not_asked_for_while_the_step_locks_it() {
        let wanted = Wanted {
            view: ViewWish::Render,
            inspector_tab: None,
        };
        let locked = Shown {
            render_allowed: false,
            ..shown(SOLID_TAB, 0)
        };
        assert_eq!(mismatch(&wanted, &locked), None);
        assert_eq!(moves(&wanted, &locked), (None, None));
        assert_eq!(
            mismatch(&wanted, &shown(SOLID_TAB, 0)).as_deref(),
            Some("Switch to the Render tab")
        );
        assert_eq!(
            moves(&wanted, &shown(SOLID_TAB, 0)),
            (Some(RENDER_TAB), None)
        );
    }

    #[test]
    fn show_me_where_moves_only_what_is_wrong() {
        let wanted = Wanted {
            view: ViewWish::Edit,
            inspector_tab: Some(2),
        };
        assert_eq!(
            moves(&wanted, &shown(RENDER_TAB, 0)),
            (Some(SOLID_TAB), Some(2))
        );
        assert_eq!(moves(&wanted, &shown(DIAGRAM_TAB, 2)), (None, None));
        assert_eq!(moves(&wanted, &shown(SOLID_TAB, 0)), (None, Some(2)));
    }

    #[test]
    fn a_step_that_waits_for_an_inspector_tab_does_not_insist_on_one() {
        let reading = step_with(Goal::InspectorTab(1));
        let stage = Stage {
            view: StageView::Edit,
            inspector_tab: Some(0),
            ..Stage::default()
        };
        assert_eq!(wanted_of(&stage, &reading).inspector_tab, None);
        let tier = step_with(Goal::tier("G1", 90.0));
        assert_eq!(wanted_of(&stage, &tier).inspector_tab, Some(0));
        let any_of = step_with(Goal::Any(vec![Goal::InspectorTab(2), Goal::ViewMode(0)]));
        assert_eq!(wanted_of(&stage, &any_of).inspector_tab, None);
    }

    #[test]
    fn the_stage_of_a_step_maps_onto_the_web_tabs() {
        let step = step_with(Goal::SolvedClosed);
        let of = |view, solid_view_mode| {
            wanted_of(
                &Stage {
                    view,
                    solid_view_mode,
                    ..Stage::default()
                },
                &step,
            )
            .view
        };
        assert_eq!(of(StageView::LiveRender, None), ViewWish::Render);
        assert_eq!(of(StageView::Edit, None), ViewWish::Edit);
        assert_eq!(of(StageView::Edit, Some(3)), ViewWish::Diagram);
        assert_eq!(of(StageView::Edit, Some(0)), ViewWish::Solid);
        assert_eq!(of(StageView::Either, None), ViewWish::Any);
        assert_eq!(of(StageView::Either, Some(3)), ViewWish::Diagram);
    }

    #[test]
    fn a_step_past_the_end_of_a_lesson_asks_for_nothing() {
        let guide = worked_example_guide();
        let nothing = wanted(&guide, guide.steps.len() + 3);
        assert_eq!(nothing.view, ViewWish::Any);
        assert_eq!(nothing.inspector_tab, None);
    }
}
