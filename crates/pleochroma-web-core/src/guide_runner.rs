//! The lesson runner's pure half, without the window.
//!
//! It decides which lessons the browser offers, what a step hands the panel, which controls a
//! step leaves open, and the launch of a lesson that waits for its starting design.
//!
//! `editor::guide` (the runner in the page) holds an [`Offers`], builds the panel's rows from
//! [`step_view`] and keeps a [`Launch`]; everything it decides here is tested natively.
//!
//! - [`Offers`]: the shared catalogue and the browser's [`WebFeatures`] (every shared feature
//!   that exists in the browser, and the library once its lane has landed), so the page asks
//!   one object which lessons it offers and how it shows each.
//! - [`StepView`] / [`AllowFlags`]: one step as the panel renders it, and the lock groups a step
//!   leaves open, as plain values the page copies into Slint's `GuideStepData` / `GuideAllow`.
//! - [`Launch`] / [`observe`]: a lesson that has asked for its starting design and what the
//!   launch sees of the window while it waits.

use pleochroma_editor::guide::{
    Group, Guide, GuideCatalog, GuideStep, LIBRARY_GRACE_TICKS, LaunchObservation, WebFeatures,
};

/// How many 250 ms ticks a launch keeps waiting with nothing visibly pending: the unsaved
/// changes question was answered Save, and the download and the continuation follow it.
pub const LAUNCH_GRACE_TICKS: u32 = LIBRARY_GRACE_TICKS;

/// The lessons the browser offers: the catalogue (every built-in lesson, plus any generated
/// while the page runs) and whether the web library exists.
pub struct Offers {
    /// Every built-in lesson, plus any generated while the page runs.
    catalog: GuideCatalog,
    /// The web library exists (see [`Self::set_library`]).
    library: bool,
}

impl Default for Offers {
    fn default() -> Self {
        Self::new()
    }
}

impl Offers {
    /// The built-in lessons, with no library yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            catalog: GuideCatalog::new(),
            library: false,
        }
    }

    /// Tells the offers that the web library exists (or no longer does). The library lessons and
    /// the lessons built from a library design are offered from then on.
    pub const fn set_library(&mut self, available: bool) {
        self.library = available;
    }

    /// The features of this build: every shared one that exists in the browser (Deep Solve,
    /// concave editing and cutting mode), and the library once its lane has landed.
    #[must_use]
    pub const fn features(&self) -> WebFeatures {
        WebFeatures {
            library: self.library,
            ..WebFeatures::ALL
        }
    }

    /// Lesson `id` as the web shows it, if the browser offers it.
    #[must_use]
    pub fn web_guide(&self, id: &str) -> Option<Guide> {
        self.catalog.web_guide(id, self.features())
    }

    /// The lessons the browser offers, as it shows them, in the learning path's order.
    #[must_use]
    pub fn offered(&self) -> Vec<Guide> {
        self.catalog.web_guides(self.features())
    }

    /// Adds a lesson generated at run time (a lesson that rebuilds a library design, say) to the
    /// catalogue, so the browser lists it once the library exists.
    ///
    /// # Errors
    ///
    /// A lesson that is not fit to run, or whose id belongs to a built-in lesson, is refused with
    /// the reason (`GuideCatalog::add_generated`).
    pub fn register_generated(&mut self, guide: Guide) -> Result<(), String> {
        self.catalog.add_generated(guide)
    }
}

/// The lock groups a step leaves open (`GuideAllow` in Slint): a group not listed is locked.
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per lock group, mirroring the Slint `GuideAllow` field for field"
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AllowFlags {
    /// The New Design control.
    pub new_design: bool,
    /// The Tier form.
    pub tier_form: bool,
    /// The tier table.
    pub tier_table: bool,
    /// The Design settings dialog.
    pub design_settings: bool,
    /// Solve.
    pub solve: bool,
    /// The Preform tab.
    pub preform_tab: bool,
    /// The advanced tabs.
    pub advanced: bool,
    /// The view tabs.
    pub view_tabs: bool,
    /// Open / save.
    pub file_ops: bool,
    /// Undo / redo.
    pub history: bool,
    /// Authoring a concave tier ([`concave_tier_allowed`]).
    pub concave_tier: bool,
}

/// `allow`'s groups as [`AllowFlags`]: a group not listed is locked.
#[must_use]
pub fn allow_flags(allow: &[Group]) -> AllowFlags {
    AllowFlags {
        new_design: allow.contains(&Group::NewDesign),
        tier_form: allow.contains(&Group::TierForm),
        tier_table: allow.contains(&Group::TierTable),
        design_settings: allow.contains(&Group::DesignSettings),
        solve: allow.contains(&Group::Solve),
        preform_tab: allow.contains(&Group::PreformTab),
        advanced: allow.contains(&Group::Advanced),
        view_tabs: allow.contains(&Group::ViewTabs),
        file_ops: allow.contains(&Group::FileOps),
        history: allow.contains(&Group::History),
        // Concave tiers are not part of the guided lessons: authoring one edits both the
        // tier form and the tier table, so a step leaves it open only when it leaves both of
        // those open (the desktop's rule, `gui/editor/guide/launch.rs`).
        concave_tier: concave_tier_allowed(allow),
    }
}

/// Whether a step that leaves `allow` usable also leaves concave tiers open: only with both
/// the tier form and the tier table.
#[must_use]
pub fn concave_tier_allowed(allow: &[Group]) -> bool {
    allow.contains(&Group::TierForm) && allow.contains(&Group::TierTable)
}

/// One lesson step as the panel renders it (Slint's `GuideStepData`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepView {
    /// The step's title.
    pub title: String,
    /// The text under the title.
    pub intro: String,
    /// The things to do, one line each.
    pub actions: Vec<String>,
    /// What to look for when it worked.
    pub check: String,
    /// Why the step matters.
    pub why: String,
    /// What the step is waiting for.
    pub waiting: String,
    /// The control the step outlines.
    pub highlight_target: String,
    /// The completion key `GuideModel.notify` is given (`goal:<index>`, or `manual`).
    pub completion: String,
    /// The step's goal reads the view tab or the inspector tab.
    pub watches_ui: bool,
    /// The step has a recipe, so Next does it for the learner.
    pub performable: bool,
    /// The lock groups the step leaves open.
    pub allow: AllowFlags,
}

/// Step number `index` of its lesson as the panel renders it. `step` is from the web view of the
/// lesson, already in the browser's words.
#[must_use]
pub fn step_view(index: usize, step: &GuideStep) -> StepView {
    StepView {
        title: step.title.clone(),
        intro: step.intro.clone(),
        actions: step.actions.clone(),
        check: step.check.clone(),
        why: step.why.clone(),
        waiting: step.waiting.clone(),
        highlight_target: step.highlight_target.clone(),
        completion: step.completion_key(index),
        watches_ui: step.goal.watches_ui(),
        performable: step.can_perform(),
        allow: allow_flags(&step.allow),
    }
}

/// A lesson that has asked for its starting design and is waiting for it.
#[derive(Clone, Debug)]
pub struct Launch {
    /// The lesson to open once the design is there.
    pub guide_id: String,
    /// Checks made so far, one per timer tick, counted while no question is on screen.
    pub ticks: u32,
    /// The design has been installed.
    pub design_arrived: bool,
}

/// What a UI sees while a lesson waits for its starting design.
///
/// The grace counts from the moment no question is on screen, so an answer of Save (whose download and continuation take
/// a moment) is waited for, and a Cancel gives up soon after.
#[must_use]
pub const fn observe(launch: &Launch, dialog_open: bool) -> LaunchObservation {
    LaunchObservation {
        design_arrived: launch.design_arrived,
        dialog_open,
        decision_pending: false,
        ticks: if dialog_open { 0 } else { launch.ticks },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_editor::guide::{
        ALL_GROUPS, Goal, GuideCategory, HIGHLIGHT_TARGETS, LaunchStatus, MANUAL, NEEDS_A_DESIGN,
        Perform, Platform, StartPlan, WEB_ONLY_TARGETS, WORKED_EXAMPLE_ID, launch_status,
        plan_start,
    };

    #[test]
    fn concave_tiers_are_open_only_when_both_the_form_and_the_table_are() {
        assert!(allow_flags(ALL_GROUPS).concave_tier);
        assert!(!allow_flags(&[Group::TierForm]).concave_tier);
        assert!(!allow_flags(&[Group::TierTable]).concave_tier);
        assert!(allow_flags(&[Group::TierForm, Group::TierTable]).concave_tier);
        assert!(!allow_flags(&[]).concave_tier);
    }

    #[test]
    fn a_step_leaves_exactly_the_groups_it_allows_open() {
        let allow = allow_flags(&[Group::Solve, Group::History]);
        assert!(allow.solve && allow.history);
        assert!(!allow.new_design && !allow.tier_form && !allow.tier_table);
        assert!(!allow.design_settings && !allow.preform_tab && !allow.advanced);
        assert!(!allow.view_tabs && !allow.file_ops);
    }

    #[test]
    fn the_browser_has_deep_solve_concave_editing_and_cutting_mode_but_no_library_yet() {
        let features = Offers::new().features();
        assert!(features.concave_edit && features.deep_solve && features.cutting_mode);
        assert!(!features.library, "the library lane flips this");
    }

    #[test]
    fn the_library_lessons_are_offered_once_the_library_exists() {
        let mut offers = Offers::new();
        let without = offers.offered().len();
        offers.set_library(true);
        let with_library = offers.offered().len();
        assert!(with_library > without);
        assert!(offers.web_guide("library-search").is_some());
        offers.set_library(false);
        assert_eq!(offers.offered().len(), without);
        assert!(offers.web_guide("library-search").is_none());
    }

    #[test]
    fn the_walkthrough_is_offered_and_a_blocked_lesson_is_not() {
        let offers = Offers::new();
        assert!(offers.web_guide(WORKED_EXAMPLE_ID).is_some());
        assert!(offers.web_guide("tiers-relations").is_none());
        assert!(offers.web_guide("no-such-lesson").is_none());
    }

    #[test]
    fn every_step_of_every_offered_lesson_reaches_the_panel_whole() {
        let guides = Offers::new().offered();
        assert!(!guides.is_empty());
        for guide in &guides {
            assert_eq!(
                guide.problem_on(Platform::Web),
                None,
                "{} is fit to run on the web",
                guide.id
            );
            for (index, step) in guide.steps.iter().enumerate() {
                let data = step_view(index, step);
                let key = data.completion.as_str();
                if step.is_manual() {
                    assert_eq!(key, MANUAL, "{} step {index}", guide.id);
                } else {
                    assert_eq!(key, format!("goal:{index}"), "{} step {index}", guide.id);
                }
                assert_eq!(data.title.as_str(), step.title);
                assert_eq!(data.actions.len(), step.actions.len());
                assert_eq!(data.watches_ui, step.goal.watches_ui());
                let target = data.highlight_target.as_str();
                assert!(
                    HIGHLIGHT_TARGETS.contains(&target) || WEB_ONLY_TARGETS.contains(&target),
                    "{} step {index} outlines {target:?}",
                    guide.id
                );
            }
        }
    }

    #[test]
    fn only_a_step_with_a_recipe_reaches_the_panel_as_performable() {
        let reading = GuideStep::new("Read", "Read this.");
        assert!(!step_view(0, &reading).performable);
        let skippable = GuideStep::new("Show", "Open a tab.")
            .actions(["Click the Preform tab."])
            .goal(Goal::InspectorTab(1), "the Preform tab");
        assert!(
            !step_view(1, &skippable).performable,
            "no recipe, only Skip"
        );
        let with_recipe = skippable.perform(Perform::InspectorTab(1));
        let data = step_view(1, &with_recipe);
        assert!(data.performable);
        assert!(
            data.watches_ui,
            "the inspector tab changes without the design"
        );
        assert_eq!(data.completion.as_str(), "goal:1");
    }

    #[test]
    fn no_offered_lesson_starts_from_a_library_design_before_the_library() {
        for guide in Offers::new().offered() {
            for has_design in [false, true] {
                let plan = plan_start(&guide.starting_state, has_design);
                assert!(
                    !matches!(plan, StartPlan::OpenLibrary { .. }),
                    "{} opens a library design",
                    guide.id
                );
                if let StartPlan::Blocked(reason) = plan {
                    assert_eq!(reason, NEEDS_A_DESIGN, "{}", guide.id);
                    assert!(!has_design, "{} blocked with a design open", guide.id);
                }
            }
        }
    }

    #[test]
    fn a_launch_counts_its_grace_only_while_no_question_is_on_screen() {
        let mut launch = Launch {
            guide_id: "tiers-move".to_owned(),
            ticks: 7,
            design_arrived: false,
        };
        let waiting = observe(&launch, false);
        assert_eq!((waiting.dialog_open, waiting.ticks), (false, 7));
        assert_eq!(
            launch_status(&waiting, LAUNCH_GRACE_TICKS),
            LaunchStatus::Waiting
        );
        // The question is on screen: waiting, and the grace starts over.
        let asking = observe(&launch, true);
        assert_eq!(asking.ticks, 0);
        assert_eq!(launch_status(&asking, 0), LaunchStatus::Waiting);
        // Cancelled: nothing arrives, and once the grace is used up the launch is dropped.
        launch.ticks = LAUNCH_GRACE_TICKS;
        assert_eq!(
            launch_status(&observe(&launch, false), LAUNCH_GRACE_TICKS),
            LaunchStatus::Abandoned
        );
        launch.design_arrived = true;
        assert_eq!(
            launch_status(&observe(&launch, false), LAUNCH_GRACE_TICKS),
            LaunchStatus::Ready
        );
    }

    #[test]
    fn a_generated_lesson_is_refused_when_it_is_not_fit_to_run() {
        let unfit = Guide::new(
            "build:unfit",
            "Unfit",
            "Has no steps.",
            GuideCategory::BuildThisDesign,
        );
        assert!(Offers::new().register_generated(unfit).is_err());
    }
}
