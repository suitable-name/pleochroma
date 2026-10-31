//! Sweeping two tiers together, keeping trials, and tuning tier by tier: the hands-on ways to try
//! angles and keep what helps.

use super::{
    ANGLE_TOLERANCE, RICH, RICH_CROWN, RICH_PAVILION, TOOLS, TOOLS_EDIT, check, event, events,
    rich_at_start, tier_called,
};
use crate::guide::{Goal, GoalContext, Guide, GuideCategory, GuideStep, StartingState};

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![sweep_two_tiers(), keep_trials(), tune_by_hand()]
}

/// Whether the Rich Teaching Design's pavilion mains are away from where the template put them.
fn pavilion_nudged(ctx: &GoalContext<'_>) -> bool {
    tier_called(ctx.design, "Pavilion Main")
        .is_some_and(|tier| (tier.angle_deg - RICH_PAVILION).abs() > ANGLE_TOLERANCE)
}

/// Whether the Rich Teaching Design's crown mains are away from where the template put them.
fn crown_nudged(ctx: &GoalContext<'_>) -> bool {
    tier_called(ctx.design, "Crown Main")
        .is_some_and(|tier| (tier.angle_deg - RICH_CROWN).abs() > ANGLE_TOLERANCE)
}

fn sweep_two_tiers() -> Guide {
    Guide::new(
        "optimizing-sweep-two-tiers",
        "Sweep Two Tiers",
        "Try the angles of two tiers together over a grid, read the figures off a heat map and use the best pair.",
        GuideCategory::Optimizing,
    )
    .starting(StartingState::Template(RICH))
    .step(
        GuideStep::new(
            "Why two tiers",
            "The pavilion angle that is best for one crown angle is not best for another, so two tiers are worth trying together.",
        )
        .actions([
            "Two Tiers sweeps the angles of two tiers over a grid. Every angle pair is solved and scored on a private copy of the design, so your design does not change while it runs.",
            "The result is a heat map: one block for every pair, bright where the figure is good. A ring marks your design's own pair and a star marks the best pair of the figure.",
            "Only Use These Angles changes the design, as one step that Undo takes back.",
        ])
        .why("A grid takes longer than a sweep of one tier, because it solves one design for every pair. The form says how many pairs it tries and about how long, and refuses a grid of more than 900 pairs.")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Run a two-tier sweep and use a pair",
            "Sweep the crown and the pavilion mains together, then make a good pair the real one.",
        )
        .actions([
            "Choose Edit > Angle Sweep... (or press Ctrl+K and type sweep).",
            "Click Two Tiers at the top of the dialog. The form starts with Crown Main as the Across Tier and Pavilion Main as the Down Tier, each two degrees either side of its angle in half-degree steps. The line under the form says how many pairs it tries and about how long.",
            "Click Run. A bar shows how many pairs are done. Cancel keeps the pairs already finished.",
            "Read the heat map. Move the pointer over it to read off a pair: the readout under the map names both angles and gives the figures. The Figure list chooses what the map shows.",
            "Click a block that is not the ringed one, for example the one with the star, then click Use These Angles.",
            "This lesson goes on when the two tiers have their new angles. Then press Close (or Esc) to leave the dialog.",
        ])
        .check("a toast names both tiers and their new angles, and the tier table shows them.")
        .why("The Across Tier runs from left to right and the Down Tier from top to bottom. If you picked a tier in the tier table first, the Across Tier starts on it: choose Crown Main and Pavilion Main in the two lists if they are not there already. A pair that gives no stone, or the design's own pair, cannot be used.")
        .goal(
            Goal::All(vec![
                event(events::TWO_TIER_SWEEP_RUN),
                event(events::TWO_TIER_APPLIED),
            ]),
            "a two-tier sweep run and a pair used",
        )
        .highlight("sweep_two_tiers")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Undo it",
            "Using a pair is one undo step, however many tiers it moves.",
        )
        .actions(["Press Ctrl+Z, or click Undo on the command bar."])
        .check("Crown Main and Pavilion Main are back at their first angles.")
        .goal(
            check("the template's own crown and pavilion", rich_at_start),
            "both tiers back at their first angles",
        )
        .highlight("tier_table")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "The map, the plateau and the CSV",
            "The same run gives you more than the best block.",
        )
        .actions([
            "The Figure list chooses what the map shows: brilliance, windowing, extinction and the other figures. Bright blocks are the best values of the figure.",
            "Outline the Best Cells draws a line round every pair that is as good as the best, within the noise of the figure: the plateau you can choose from. A pair in the middle of the plateau is the safest to cut.",
            "Refine Around This sweeps a finer grid around the block you picked, with about the same number of pairs, and runs it at once.",
            "Also Average the Tilt Performance (Slower) adds the tilt figures to every pair. A long sweep with it asks before it runs. Copy CSV and Save CSV... take every pair and its figures to a spreadsheet.",
        ])
        .why("The map is not saved with the design and is gone when the dialog closes. With the keyboard, Tab to the map, then the arrow keys choose a pair and Enter uses it.")
        .allow(TOOLS),
    )
}

fn keep_trials() -> Guide {
    Guide::new(
        "solving-trials",
        "Keep Trials",
        "Keep the design as a trial, try something else, and come back to the trial or make it the Baseline.",
        GuideCategory::Optimizing,
    )
    .starting(StartingState::Template(RICH))
    .step(
        GuideStep::new(
            "Keep a trial",
            "A trial is the design as it is now, kept with its figures, so you can try something else and come back to it.",
        )
        .actions([
            "Under the inspector's tabs the strip shows the figures of the design against the Baseline.",
            "Click Keep as Trial, or press Ctrl+Shift+K.",
        ])
        .check("a toast says Kept as Trial 1.")
        .why("Trials live in this session only: they are not saved with the design. A Baseline is set when a design opens, so the strip compares from the start.")
        .goal(event(events::TRIAL_KEPT), "a trial kept")
        .highlight("keep_trial_button")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "Change the crown",
            "Give the next trial something to differ in.",
        )
        .actions([
            "Double-click the ANGLE cell of the Crown Main row in the tier table, or pick the row and press F2.",
            "Type 36 and press Enter.",
        ])
        .check("the Crown Main row shows 36.00.")
        .goal(Goal::tier("Crown Main", 36.0), "Crown Main at 36.0")
        .highlight("tier_table")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "Keep a second trial",
            "Keep this version too.",
        )
        .actions(["Click Keep as Trial again, or press Ctrl+Shift+K."])
        .check("a toast says Kept as Trial 2.")
        .goal(event(events::TRIAL_KEPT), "a second trial kept")
        .highlight("keep_trial_button")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "Open the Trials view",
            "The trials are listed in the History tab, next to the steps and the variants.",
        )
        .actions([
            "Click History, the last tab at the top of the inspector below the tier table.",
            "At the top of the tab, click Trials. Steps and Variants are the other views.",
        ])
        .check("a table with a Baseline line and the two trials under it, each with five figures.")
        .why("Each figure shows its value and the signed change since the Baseline. Green is better than the figure's noise threshold, red is worse and grey is about the same, so a single small nudge often reads grey. The best value of each column has an outline.")
        .goal(
            Goal::All(vec![
                Goal::InspectorTab(4),
                event(events::TRIALS_VIEW_OPENED),
            ]),
            "the Trials view opened",
        )
        .highlight("history_tab")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "Restore the first trial",
            "Restore brings a trial's design back.",
        )
        .actions([
            "Click the row of Trial 1. The stone shows that trial while it is picked; a click on the picked row puts the design back.",
            "Click Restore under the table.",
        ])
        .check("Crown Main reads 34.50 again, and a toast says Restored Trial 1.")
        .why("Restore is one undo step, named Restore Trial 1 in the Steps view, and a restored trial is not kept as a new one. A trial of a design that is no longer open cannot be restored.")
        .goal(
            Goal::All(vec![
                event(events::TRIAL_RESTORED),
                check("the template's own crown and pavilion", rich_at_start),
            ]),
            "Trial 1 restored",
        )
        .highlight("history_tab")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "Make a trial the Baseline",
            "A trial can also become the Baseline every figure is read against.",
        )
        .actions([
            "Click the row of Trial 2, then click Set as Baseline.",
        ])
        .check("a toast says Baseline set: Trial 2, and the figures strip now reads against it.")
        .why("The Baseline lives in this session only. Compare to Current shows a trial and the design now side by side, Save as Variant... keeps a trial's design in the library, and Delete removes a trial.")
        .goal(event(events::SNAPSHOT_TAKEN), "a Baseline set from a trial")
        .highlight("history_tab")
        .allow(TOOLS_EDIT),
    )
    .step(
        GuideStep::new(
            "Where else trials come from",
            "Keeping by hand is not the only way to get one.",
        )
        .actions([
            "The tools keep trials for you: the design just before an Optimize or a Retarget is applied, a candidate applied from Optimize, an angle or a pair used from a sweep, and a tier accepted in Tune. The Origin column says where each came from.",
            "Record Every Change keeps one trial for every change you make, at most one a second. Undo and Redo never add one.",
            "Sort orders the table by a figure, best first. A star keeps a trial when the list is full: it holds 50, and the oldest unstarred trial drops first.",
        ])
        .why("A trial scored in another material shows its figures grey until you press Re-score Trials.")
        .allow(TOOLS_EDIT),
    )
}

fn tune_by_hand() -> Guide {
    Guide::new(
        "optimizing-tune-by-hand",
        "Tune by Hand",
        "Go through the tiers one at a time, nudge each angle and accept the ones you are happy with.",
        GuideCategory::Optimizing,
    )
    .starting(StartingState::Template(RICH))
    .step(
        GuideStep::new(
            "Open the Tune view",
            "Tune goes through the tiers one at a time, so you can nudge each angle and keep what helps.",
        )
        .actions([
            "Click Optimize, the third tab at the top of the inspector below the tier table.",
            "At the top of the tab, click Tune. Search is the other view.",
        ])
        .check("a list headed Tiers (Tuning Order) with the pavilion main first.")
        .why("The order is the pavilion main, the crown main, the other crown tiers and then the other pavilion tiers. A tier whose angle follows a relation is greyed and says what it follows, a girdle tier has no angle to tune, and the table and the culet are not listed.")
        .goal(
            Goal::All(vec![
                Goal::InspectorTab(2),
                event(events::TUNE_OPENED),
            ]),
            "the Tune view open",
        )
        .highlight("optimize_tab")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Nudge the pavilion",
            "One small step at a time shows what an angle does.",
        )
        .actions([
            "Click the Pavilion Main row of the list. A title with the tier's name and angle appears over a row of nudge buttons.",
            "Click +0.1 to make the angle a tenth of a degree steeper.",
        ])
        .check("the title reads 40.10 degrees, and the figures strip under the tabs shows what the nudge did.")
        .why("A nudge is one undo step, like an angle typed in the tier table. -0.5, -0.1, +0.1 and +0.5 are the nudge buttons, and the angle box takes a number you type.")
        .goal(
            Goal::All(vec![
                event(events::TUNE_NUDGED),
                check("the pavilion main moved", pavilion_nudged),
            ]),
            "the pavilion main nudged",
        )
        .highlight("optimize_tab")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Accept the pavilion",
            "Accept says you are happy with a tier.",
        )
        .actions(["Click Accept."])
        .check("a dot beside Pavilion Main turns green, and a toast says it was accepted and kept as a trial.")
        .why("Accept keeps the design as it is now as a trial (see the History tab's Trials view) and ticks the tier, so Next Tier skips it. Revert This Tier puts a tier back to the angle it has in the Baseline, as one undo step.")
        .goal(event(events::TUNE_ACCEPTED), "a tier accepted")
        .highlight("tune_accept_button")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Go on to the crown",
            "Next Tier picks the next tier that is not accepted yet.",
        )
        .actions([
            "Click Next Tier. The crown main is picked.",
            "Click +0.1 to nudge it.",
        ])
        .check("the title reads 34.60 degrees.")
        .goal(
            Goal::All(vec![
                event(events::TUNE_NUDGED),
                check("the crown main moved", crown_nudged),
            ]),
            "the crown main nudged",
        )
        .highlight("optimize_tab")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Accept the crown",
            "Every accepted tier is a trial you can come back to.",
        )
        .actions(["Click Accept."])
        .check("a dot beside Crown Main turns green.")
        .goal(event(events::TUNE_ACCEPTED), "the second tier accepted")
        .highlight("tune_accept_button")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "What else Tune shows",
            "Tune is the same tools in a smaller loop.",
        )
        .actions([
            "The strip under the inspector's tabs shows the figures against the Baseline after every nudge. Set as Baseline in this view makes the design as it is now the new Baseline.",
            "The History tab's Trials view lists the two accepted tiers as trials with the origin Tune.",
            "In the Advanced interface Tune also draws a small sweep of the picked tier's angle with a star on the best angle: a click on the chart or Jump to Best sets that angle. Pair With opens Two Tiers with the tier filled in, and the Up and Dn buttons reorder the list.",
        ])
        .why("The chart is drawn when the design settles. Out of Date says another tier changed since it was drawn; Recompute draws it again.")
        .allow(TOOLS),
    )
}
