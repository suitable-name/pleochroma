//! The web overlay: the browser build's wording of a lesson, as data on top of the catalogue.
//!
//! A [`GuideOverlay`] belongs to one lesson id. It can reword text (a whole line, or a fragment of
//! any line), drop a line, omit a step, give a step another title, replace a step's goal, point a
//! step at another highlight target and fix which control groups a step leaves usable. Nothing in
//! it touches the catalogue: [`web_view`] copies the desktop guide and applies the overlay to the
//! copy, so the desktop sees its lessons exactly as written.
//!
//! Two rules apply to every web guide, overlay or not: the highlight aliases of
//! [`web_target`], and unlocking the Render tab (group `ViewTabs`) on a step that is about it.

use crate::guide::{Goal, Group, Guide, GuideStep, Perform, default_perform};
use std::sync::OnceLock;

/// Highlight targets only the browser build has. A web guide may name them; a desktop guide may not.
///
/// `schedule_cutting_sheet` and `schedule_diagram` are the Cutting Sheet (HTML) and Diagram (PNG)
/// buttons of the Schedule tab: the nearest controls that can be outlined, since the File menu
/// items they stand in for are native menu entries that cannot be.
pub const WEB_ONLY_TARGETS: &[&str] = &["schedule_cutting_sheet", "schedule_diagram"];

/// Highlight targets of offered lessons that name a native menu item of the web build.
///
/// A menu item cannot be outlined, so these steps show their outline target only as text until the
/// web build chooses a proxy for each. A guard test of the web build may use this list to allow a
/// target that draws nothing.
pub const WEB_TEXT_ONLY_TARGETS: &[&str] = &[
    "retarget_button",
    "snapshot_button",
    "compare_button",
    "save_button",
    "open_button",
    "export_asc_button",
    "export_menu",
];

/// Highlight targets the desktop names that the web draws on a control of another name.
const TARGET_ALIASES: &[(&str, &str)] = &[
    // The material lives in the Design Settings panel on the web, not in a Stone tab.
    ("stone_tab", "design_settings"),
];

/// The highlight target the web build draws for the catalogue's `target`: the same name, unless
/// the web has the control under another one.
#[must_use]
pub fn web_target(target: &str) -> &str {
    TARGET_ALIASES
        .iter()
        .find(|(from, _)| *from == target)
        .map_or(target, |(_, to)| *to)
}

/// One change to the text of a step.
///
/// A text is the intro, the "look for" line, the explanation, the "waiting for" text or one
/// action line. A step's title is changed with [`StepOverlay::retitle`] only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextEdit {
    /// Replaces every text that is exactly `from` with `to`.
    Line {
        /// The desktop line, word for word.
        from: String,
        /// The web line.
        to: String,
    },
    /// Replaces every occurrence of the fragment `find` in every text with `with`.
    Sub {
        /// The fragment to find.
        find: String,
        /// What replaces it.
        with: String,
    },
    /// Removes the action line that is exactly `line`, or empties the "look for" or explanation
    /// text that is exactly `line`.
    Drop {
        /// The desktop line, word for word.
        line: String,
    },
}

impl TextEdit {
    /// Replaces the whole line `from` with `to`.
    #[must_use]
    pub fn line(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self::Line {
            from: from.into(),
            to: to.into(),
        }
    }

    /// Replaces the fragment `find` with `with` wherever it occurs.
    #[must_use]
    pub fn sub(find: impl Into<String>, with: impl Into<String>) -> Self {
        Self::Sub {
            find: find.into(),
            with: with.into(),
        }
    }

    /// Drops the line `line`.
    #[must_use]
    pub fn drop_line(line: impl Into<String>) -> Self {
        Self::Drop { line: line.into() }
    }

    /// A short description for a message about an edit that matched nothing.
    fn describe(&self) -> String {
        match self {
            Self::Line { from, .. } => format!("line {from:?}"),
            Self::Sub { find, .. } => format!("fragment {find:?}"),
            Self::Drop { line } => format!("dropped line {line:?}"),
        }
    }
}

/// A replacement goal for a step whose desktop goal cannot happen on the web.
#[derive(Clone, Debug)]
pub struct GoalFix {
    /// What completes the step on the web.
    pub goal: Goal,
    /// The "Waiting for: ..." text for it.
    pub waiting: String,
    /// What Next does for it, or `None` for the recipe the goal implies ([`default_perform`]).
    pub perform: Option<Perform>,
}

/// How one step of a lesson differs on the web.
#[derive(Clone, Debug, Default)]
pub struct StepOverlay {
    /// The step's title in the catalogue; it names the step.
    pub step: String,
    /// The step does not exist on the web.
    pub omit: bool,
    /// The step's title on the web.
    pub retitle: Option<String>,
    /// Text changes inside this step.
    pub edits: Vec<TextEdit>,
    /// A goal to replace the step's own.
    pub goal: Option<GoalFix>,
    /// The highlight target on the web, instead of the catalogue's (or its alias).
    pub highlight: Option<String>,
    /// Control groups the web step leaves usable in addition to the catalogue's.
    pub allow_add: Vec<Group>,
    /// Control groups the web step locks although the catalogue leaves them usable.
    pub allow_remove: Vec<Group>,
}

impl StepOverlay {
    /// An overlay for the step called `step` in the catalogue; add changes with the builder calls.
    #[must_use]
    pub fn of(step: impl Into<String>) -> Self {
        Self {
            step: step.into(),
            ..Self::default()
        }
    }

    /// An overlay that removes the step called `step` from the web lesson.
    #[must_use]
    pub fn omitted(step: impl Into<String>) -> Self {
        Self {
            omit: true,
            ..Self::of(step)
        }
    }

    /// Gives the step another title on the web.
    #[must_use]
    pub fn retitle(mut self, title: impl Into<String>) -> Self {
        self.retitle = Some(title.into());
        self
    }

    /// Adds [`TextEdit::line`].
    #[must_use]
    pub fn line(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.edits.push(TextEdit::line(from, to));
        self
    }

    /// Adds [`TextEdit::sub`].
    #[must_use]
    pub fn sub(mut self, find: impl Into<String>, with: impl Into<String>) -> Self {
        self.edits.push(TextEdit::sub(find, with));
        self
    }

    /// Adds [`TextEdit::drop_line`].
    #[must_use]
    pub fn drop_line(mut self, line: impl Into<String>) -> Self {
        self.edits.push(TextEdit::drop_line(line));
        self
    }

    /// Replaces the step's goal and its "waiting for" text; Next then does what the new goal implies.
    #[must_use]
    pub fn goal(mut self, goal: Goal, waiting: impl Into<String>) -> Self {
        self.goal = Some(GoalFix {
            goal,
            waiting: waiting.into(),
            perform: None,
        });
        self
    }

    /// Outlines `target` on the web instead of the catalogue's target.
    #[must_use]
    pub fn highlight(mut self, target: impl Into<String>) -> Self {
        self.highlight = Some(target.into());
        self
    }

    /// Leaves `groups` usable on the web in addition to the catalogue's.
    #[must_use]
    pub fn allow_add(mut self, groups: &[Group]) -> Self {
        self.allow_add.extend_from_slice(groups);
        self
    }

    /// Locks `groups` on the web although the catalogue leaves them usable.
    #[must_use]
    pub fn allow_remove(mut self, groups: &[Group]) -> Self {
        self.allow_remove.extend_from_slice(groups);
        self
    }
}

/// How one lesson differs on the web.
#[derive(Clone, Debug, Default)]
pub struct GuideOverlay {
    /// The lesson id.
    pub guide: String,
    /// The lesson's title on the web.
    pub title: Option<String>,
    /// The lesson's one-line summary on the web.
    pub summary: Option<String>,
    /// Text changes applied to every step that is kept.
    pub edits: Vec<TextEdit>,
    /// Per-step changes.
    pub steps: Vec<StepOverlay>,
}

impl GuideOverlay {
    /// An overlay for lesson `guide`; add changes with the builder calls.
    #[must_use]
    pub fn new(guide: impl Into<String>) -> Self {
        Self {
            guide: guide.into(),
            ..Self::default()
        }
    }

    /// Gives the lesson another title on the web.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Gives the lesson another summary on the web.
    #[must_use]
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Adds a text change that applies to every step.
    #[must_use]
    pub fn edit(mut self, edit: TextEdit) -> Self {
        self.edits.push(edit);
        self
    }

    /// Adds text changes that apply to every step.
    #[must_use]
    pub fn edits(mut self, edits: impl IntoIterator<Item = TextEdit>) -> Self {
        self.edits.extend(edits);
        self
    }

    /// Adds a step overlay.
    #[must_use]
    pub fn step(mut self, step: StepOverlay) -> Self {
        self.steps.push(step);
        self
    }

    /// Removes the step called `title` from the web lesson.
    #[must_use]
    pub fn omit(self, title: impl Into<String>) -> Self {
        self.step(StepOverlay::omitted(title))
    }
}

/// The text changes that turn the desktop's "pick a material" lines into the web's.
///
/// The desktop opens the Stone tab of the Parameters dock and sets the material at once; the web
/// sets it in Design Settings and applies it with a button. The three changes are guide-wide, so a
/// lesson adds them once for every step that names the material.
#[must_use]
pub fn material_edits(material: &str) -> Vec<TextEdit> {
    vec![
        TextEdit::line(
            "Open the Stone tab of the Parameters dock (View > Stone Properties, or Ctrl+Alt+1).",
            "Open Design Settings: click Design Settings in the dock, or choose Design > Design Settings...",
        ),
        TextEdit::sub(
            format!("Set Material to {material}. It applies at once."),
            format!("Set Material to {material}, then click Apply Material."),
        ),
        TextEdit::sub("the Stone tab shows", "Design Settings shows"),
    ]
}

/// Every overlay, built once.
fn overlays() -> &'static [GuideOverlay] {
    static OVERLAYS: OnceLock<Vec<GuideOverlay>> = OnceLock::new();
    OVERLAYS.get_or_init(|| {
        let mut all = Vec::new();
        all.extend(super::data_tiers::overlays());
        all.extend(super::data_solving::overlays());
        all.extend(super::data_viewing::overlays());
        all.extend(super::data_features::overlays());
        all
    })
}

/// The overlay of lesson `id`, if it has one.
pub(super) fn overlay_for(id: &str) -> Option<&'static GuideOverlay> {
    overlays().iter().find(|overlay| overlay.guide == id)
}

/// Every overlay, for the tests that check them against the catalogue.
#[cfg(test)]
pub(super) fn all_overlays() -> &'static [GuideOverlay] {
    overlays()
}

/// `guide` as the browser build shows it.
///
/// A lesson without an overlay is shown as written, apart from the two rules that apply to every
/// web guide (highlight aliases, the Render tab's lock group).
#[must_use]
pub fn web_view(guide: &Guide) -> Guide {
    web_adapt(guide).0
}

/// `guide` as the browser build shows it, and a sentence for every part of its overlay that
/// matched nothing in `guide` (the catalogue's wording moved on, so the overlay is stale).
///
/// The list is empty for a lesson without an overlay and for a healthy overlay.
#[must_use]
pub fn web_adapt(guide: &Guide) -> (Guide, Vec<String>) {
    let mut view = guide.clone();
    let mut unmatched = Vec::new();
    if let Some(overlay) = overlay_for(&guide.id) {
        apply_overlay(&mut view, overlay, &mut unmatched);
    }
    for step in &mut view.steps {
        apply_platform_rules(step);
    }
    (view, unmatched)
}

fn apply_overlay(guide: &mut Guide, overlay: &GuideOverlay, unmatched: &mut Vec<String>) {
    if let Some(title) = &overlay.title {
        guide.title.clone_from(title);
    }
    if let Some(summary) = &overlay.summary {
        guide.summary.clone_from(summary);
    }
    let titles: Vec<String> = guide.steps.iter().map(|step| step.title.clone()).collect();
    let mut omitted = vec![false; titles.len()];
    for step_overlay in &overlay.steps {
        let Some(at) = titles.iter().position(|title| *title == step_overlay.step) else {
            unmatched.push(format!(
                "{}: no step is titled {:?}",
                overlay.guide, step_overlay.step
            ));
            continue;
        };
        if step_overlay.omit {
            omitted[at] = true;
        } else {
            apply_step(
                &mut guide.steps[at],
                step_overlay,
                &overlay.guide,
                unmatched,
            );
        }
    }
    let mut hits = vec![false; overlay.edits.len()];
    for (at, step) in guide.steps.iter_mut().enumerate() {
        if !omitted[at] {
            edit_step(step, &overlay.edits, &mut hits);
        }
    }
    for (edit, hit) in overlay.edits.iter().zip(hits) {
        if !hit {
            unmatched.push(format!(
                "{}: {} matched nothing",
                overlay.guide,
                edit.describe()
            ));
        }
    }
    let mut keep = omitted.into_iter().map(|gone| !gone);
    guide.steps.retain(|_| keep.next().unwrap_or(true));
}

fn apply_step(
    step: &mut GuideStep,
    overlay: &StepOverlay,
    guide_id: &str,
    unmatched: &mut Vec<String>,
) {
    let mut hits = vec![false; overlay.edits.len()];
    edit_step(step, &overlay.edits, &mut hits);
    for (edit, hit) in overlay.edits.iter().zip(hits) {
        if !hit {
            unmatched.push(format!(
                "{guide_id} / {:?}: {} matched nothing",
                overlay.step,
                edit.describe()
            ));
        }
    }
    if let Some(title) = &overlay.retitle {
        step.title.clone_from(title);
    }
    if let Some(fix) = &overlay.goal {
        step.goal = fix.goal.clone();
        step.waiting.clone_from(&fix.waiting);
        step.perform = fix.perform.clone().or_else(|| default_perform(&fix.goal));
    }
    if let Some(target) = &overlay.highlight {
        step.highlight_target.clone_from(target);
    }
    for group in &overlay.allow_add {
        if !step.allow.contains(group) {
            step.allow.push(*group);
        }
    }
    step.allow
        .retain(|group| !overlay.allow_remove.contains(group));
}

/// The texts of a step that a [`TextEdit`] reaches: the intro, the "look for" line, the
/// explanation, the "waiting for" text and every action line.
fn texts_mut(step: &mut GuideStep) -> impl Iterator<Item = &mut String> {
    [
        &mut step.intro,
        &mut step.check,
        &mut step.why,
        &mut step.waiting,
    ]
    .into_iter()
    .chain(step.actions.iter_mut())
}

/// Applies `edits` to `step` in order; `hits[i]` becomes `true` when edit `i` changed something.
fn edit_step(step: &mut GuideStep, edits: &[TextEdit], hits: &mut [bool]) {
    for (edit, hit) in edits.iter().zip(hits.iter_mut()) {
        match edit {
            TextEdit::Line { from, to } => {
                for text in texts_mut(step) {
                    if text == from {
                        text.clone_from(to);
                        *hit = true;
                    }
                }
            }
            TextEdit::Sub { find, with } => {
                if find.is_empty() {
                    continue;
                }
                for text in texts_mut(step) {
                    if text.contains(find.as_str()) {
                        *text = text.replace(find.as_str(), with);
                        *hit = true;
                    }
                }
            }
            TextEdit::Drop { line } => {
                let before = step.actions.len();
                step.actions.retain(|action| action != line);
                if step.actions.len() != before {
                    *hit = true;
                }
                for text in [&mut step.check, &mut step.why] {
                    if text == line {
                        text.clear();
                        *hit = true;
                    }
                }
            }
        }
    }
}

/// The rules every web step follows.
fn apply_platform_rules(step: &mut GuideStep) {
    let target = web_target(&step.highlight_target).to_owned();
    step.highlight_target = target;
    if needs_render_tab(step) && !step.allow.contains(&Group::ViewTabs) {
        step.allow.push(Group::ViewTabs);
    }
}

/// Whether the step is about the Render tab. The desktop's view pills are never locked, but the
/// web's Render tab is locked by the `ViewTabs` group, so such a step must leave it usable.
fn needs_render_tab(step: &GuideStep) -> bool {
    matches!(
        step.highlight_target.as_str(),
        "live_render_tab" | "lighting_tab"
    ) || goal_needs_render_tab(&step.goal)
}

fn goal_needs_render_tab(goal: &Goal) -> bool {
    match goal {
        Goal::ViewMode(mode) => matches!(*mode, 1 | 2 | 4 | 5),
        Goal::Event(name) => name == crate::guide::viewing_events::LIVE_RENDER_OPENED,
        Goal::All(goals) | Goal::Any(goals) => goals.iter().any(goal_needs_render_tab),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guide::GuideCategory;

    fn sample() -> Guide {
        Guide::new("sample", "Sample", "A sample.", GuideCategory::Tiers)
            .step(
                GuideStep::new("First", "Click the Edit tab.")
                    .actions(["Click A.", "Click B.", "Click C."])
                    .check("A shows.")
                    .why("Because A."),
            )
            .step(GuideStep::new("Second", "Second intro.").actions(["Click D."]))
    }

    fn adapt_with(overlay: &GuideOverlay) -> (Guide, Vec<String>) {
        let mut guide = sample();
        let mut unmatched = Vec::new();
        apply_overlay(&mut guide, overlay, &mut unmatched);
        (guide, unmatched)
    }

    #[test]
    fn a_line_edit_replaces_whole_lines_only() {
        let overlay = GuideOverlay::new("sample")
            .step(StepOverlay::of("First").line("Click A.", "Press A."))
            .step(StepOverlay::of("Second").line("Click", "Press"));
        let (guide, unmatched) = adapt_with(&overlay);
        assert_eq!(guide.steps[0].actions, ["Press A.", "Click B.", "Click C."]);
        assert_eq!(guide.steps[1].actions, ["Click D."]);
        assert_eq!(unmatched.len(), 1, "{unmatched:?}");
    }

    #[test]
    fn a_fragment_edit_reaches_every_text_of_the_step() {
        let overlay = GuideOverlay::new("sample").step(StepOverlay::of("First").sub("A", "Z"));
        let (guide, unmatched) = adapt_with(&overlay);
        let step = &guide.steps[0];
        assert_eq!(step.actions, ["Click Z.", "Click B.", "Click C."]);
        assert_eq!(step.check, "Z shows.");
        assert_eq!(step.why, "Because Z.");
        assert_eq!(step.intro, "Click the Edit tab.");
        assert!(unmatched.is_empty());
        assert_eq!(guide.steps[1].actions, ["Click D."]);
    }

    #[test]
    fn a_guide_wide_edit_reaches_every_kept_step() {
        let overlay = GuideOverlay::new("sample")
            .edit(TextEdit::sub("Click", "Press"))
            .omit("Second");
        let (guide, unmatched) = adapt_with(&overlay);
        assert!(unmatched.is_empty());
        assert_eq!(guide.steps.len(), 1);
        assert_eq!(guide.steps[0].intro, "Press the Edit tab.");
        assert_eq!(guide.steps[0].actions, ["Press A.", "Press B.", "Press C."]);
    }

    #[test]
    fn a_drop_removes_an_action_and_empties_an_explanation() {
        let overlay = GuideOverlay::new("sample").step(
            StepOverlay::of("First")
                .drop_line("Click B.")
                .drop_line("Because A."),
        );
        let (guide, unmatched) = adapt_with(&overlay);
        assert!(unmatched.is_empty());
        assert_eq!(guide.steps[0].actions, ["Click A.", "Click C."]);
        assert_eq!(guide.steps[0].why, "");
    }

    #[test]
    fn a_step_can_be_retitled_and_a_missing_step_is_reported() {
        let overlay = GuideOverlay::new("sample")
            .title("Web sample")
            .summary("A web sample.")
            .step(StepOverlay::of("First").retitle("Opening"))
            .step(StepOverlay::of("Third").line("x", "y"));
        let (guide, unmatched) = adapt_with(&overlay);
        assert_eq!(guide.title, "Web sample");
        assert_eq!(guide.summary, "A web sample.");
        assert_eq!(guide.steps[0].title, "Opening");
        assert_eq!(unmatched.len(), 1, "{unmatched:?}");
    }

    #[test]
    fn a_replaced_goal_brings_its_waiting_text_and_its_recipe() {
        let overlay = GuideOverlay::new("sample")
            .step(StepOverlay::of("Second").goal(Goal::ViewMode(3), "The Diagram view"));
        let (guide, unmatched) = adapt_with(&overlay);
        assert!(unmatched.is_empty());
        let step = &guide.steps[1];
        assert!(matches!(step.goal, Goal::ViewMode(3)));
        assert_eq!(step.waiting, "The Diagram view");
        assert!(matches!(step.perform, Some(Perform::ViewMode(3))));
    }

    #[test]
    fn a_step_can_name_another_highlight_target_and_more_groups() {
        let overlay = GuideOverlay::new("sample").step(
            StepOverlay::of("First")
                .highlight("preform_tab")
                .allow_add(&[Group::PreformTab])
                .allow_remove(&[Group::History]),
        );
        let (guide, _) = adapt_with(&overlay);
        let step = &guide.steps[0];
        assert_eq!(step.highlight_target, "preform_tab");
        assert!(step.allow.contains(&Group::PreformTab));
        assert!(!step.allow.contains(&Group::History));
    }

    #[test]
    fn the_stone_tab_is_the_design_settings_panel_on_the_web() {
        assert_eq!(web_target("stone_tab"), "design_settings");
        assert_eq!(web_target("solve_button"), "solve_button");
        let mut step = GuideStep::new("Pick", "Pick a material.").highlight("stone_tab");
        apply_platform_rules(&mut step);
        assert_eq!(step.highlight_target, "design_settings");
    }

    #[test]
    fn a_step_about_a_render_view_unlocks_the_render_tab() {
        let locked = |step: GuideStep| {
            let mut step = step.allow(&[Group::History]);
            apply_platform_rules(&mut step);
            !step.allow.contains(&Group::ViewTabs)
        };
        assert!(!locked(
            GuideStep::new("Ray", "Ray.").goal(Goal::ViewMode(4), "Ray")
        ));
        assert!(!locked(GuideStep::new("Any", "Any.").goal(
            Goal::Any(vec![Goal::ViewMode(1), Goal::ViewMode(2)]),
            "Path"
        )));
        assert!(!locked(
            GuideStep::new("Live", "Live.").highlight("live_render_tab")
        ));
        // The Solid and Diagram tabs are never locked, and a reading step needs nothing.
        assert!(locked(
            GuideStep::new("Diagram", "D.").goal(Goal::ViewMode(3), "D")
        ));
        assert!(locked(GuideStep::new("Read", "Read.")));
    }

    #[test]
    fn the_material_edits_cover_the_three_lines_of_a_material_step() {
        let edits = material_edits("Opal");
        assert_eq!(edits.len(), 3);
        assert!(matches!(
            &edits[1],
            TextEdit::Sub { find, with }
                if find == "Set Material to Opal. It applies at once."
                    && with == "Set Material to Opal, then click Apply Material."
        ));
    }
}
