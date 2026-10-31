//! The Crystal Axis in the Rough Planner: setting the rough's axis, reading the orientation rule,
//! overriding and locking one stone, planning again and opening the stone in the editor (manual
//! chapters "Crystal Axis" and "Planning from the Rough").
//!
//! The planner is a window of its own, so none of its controls can be outlined from the guide
//! panel; every step names the control in words. What happens in the planner leaves no trace in
//! the design, so the steps wait for the events the desktop raises from `gui/rough_plan` (see
//! [`super::super::viewing_events`]). The lesson makes its own plan, like the planner lessons that
//! are about results, because a stone row is what it is about.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{FILES, VIEWPORT, event};
use crate::guide::{
    Goal, Guide, GuideCategory, GuideStep, StartingState,
    tutorials::viewing_events::{
        ROUGH_AXIS_SET, ROUGH_PLAN_FINISHED, ROUGH_STONE_LOCKED, ROUGH_STONE_OPENED,
        ROUGH_STONE_OVERRIDDEN,
    },
};

/// The lessons of this file.
pub fn guides() -> Vec<Guide> {
    vec![override_orientation()]
}

fn override_orientation() -> Guide {
    Guide::new(
        "planning-override-orientation",
        "Override a Stone's Orientation",
        "Set the crystal axis of a rough, read the orientation rule, give one stone its own pose and a Reason, lock it, plan again and open it in the editor with its axis.",
        GuideCategory::RoughPlanning,
    )
    .starting(StartingState::CurrentDesign)
    .step(
        GuideStep::new(
            "Orient stones to the crystal axis",
            "The Rough Planner can place each stone so that the crystal axis of the rough lies the way you want it in the cut stone.",
        )
        .actions([
            "Until you set it, the direction of the axis in the rough is Unknown, and the plan is made as it always was.",
            "Once it is set, an orientation rule says how each stone sits to the axis: Table Across, In the Table Plane, A Custom Angle, or a rule named for a color the material shows face-up.",
            "A rule is Preferred, when a stone off the rule still counts but loses value for every degree beyond the tolerance, or Required, when no stone may be off by more than the tolerance.",
        ])
        .why("Every material starts with a default rule that you can change. This lesson uses a sapphire.")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Give the rough an axis",
            "The axis is part of the rough, so you set it in the ROUGH column of the planner.",
        )
        .actions([
            "Choose Library, then Plan Rough... in the menu bar.",
            "Choose Block and type 18.4, 11 and 9.6 in Rough X (mm), Rough Y (mm) and Rough Z (mm). Set Material to Sapphire.",
            "Scroll to CRYSTAL AXIS and click Set.",
        ])
        .check("Tilt (deg) and Azimuth (deg) fields appear, and a sentence under them says how the axis lies in the rough.")
        .why("A material without a crystal axis, such as Diamond, has no CRYSTAL AXIS section. Set starts with the axis along the rough's Y axis, and Undo in the planner takes it back like any edit of the rough.")
        .goal(event(ROUGH_AXIS_SET), "The rough's Crystal Axis set")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Find the direction",
            "Move the axis to where the crystal really points in your rough.",
        )
        .actions([
            "Click Fit to Crystal Length to set the axis along the longest direction of the rough. A crystal is usually longest along its axis, and a note says how clear that direction is.",
            "Or click Align to Face or Align to Edge and click the rough in the 3D view, or type Tilt (deg) and Azimuth (deg). Tilt is measured from the rough's Y axis.",
        ])
        .check("The Tilt (deg) and Azimuth (deg) fields show the new angles, and the axis line is drawn in the 3D view.")
        .why("Each way is one undo step. From Color Zones follows the color zone you accepted in Rough Color, and a drag on the axis line in the 3D view turns the axis too.")
        .goal(event(ROUGH_AXIS_SET), "The rough's Crystal Axis set")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Read the rule line",
            "With the axis set, an ORIENTATION RULE section appears under it.",
        )
        .actions([
            "The line at the top says which rule the plan uses and where it comes from, in the form Stones are oriented to the crystal axis: the rule, then the material's default in brackets.",
            "Material Default uses the rule the material starts with, Ignore the Axis plans for the most weight, and Set lets you choose the rule yourself: its kind, Preferred or Required, the Tolerance (deg) and a Reason.",
            "A design can have a rule of its own: select its row in the results, and the rule of that design shows under the stones.",
        ])
        .why("The Reason is saved with the plan and printed on the cutting sheet. A rule you change applies the next time you plan.")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Plan the rough",
            "Plan cuts stones from the rough under the rule.",
        )
        .actions([
            "Set Designs (Up To) to 1, choose Current Filter, and click Plan, or press Ctrl+Enter.",
        ])
        .check("Result cards appear in the column on the right, and under them the STONES AND THE CRYSTAL AXIS section lists the stones of the selected result.")
        .why("A narrow search in the library first keeps the first plan quick. A Required rule never places a stone off the rule by more than the tolerance.")
        .goal(event(ROUGH_PLAN_FINISHED), "A plan finished")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Override one stone",
            "A stone can sit differently from the rule when you have a reason.",
        )
        .actions([
            "Click the card of a result, then a design row, to show its stones.",
            "In a stone's row, click another Pose number if the row offers them. Each pose turns the stone a half turn about one of its own axes: the piece and the weight stay, only the angle of the crystal axis changes.",
            "Type a Reason in the stone's Reason field, for example avoids the crack, and press Enter.",
        ])
        .check("The row shows the new angle of the axis to the table, and the Reason stays in its field.")
        .why("A pose and a Reason are choices for the stone: they are saved with the plan, the Reason is printed on the cutting sheet, and each is one undo step.")
        .goal(event(ROUGH_STONE_OVERRIDDEN), "A stone overridden")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Lock the stone and plan again",
            "A locked stone is kept while the rest of the rough is planned again.",
        )
        .actions([
            "In the same stone row, click Lock. The row then says Locked.",
            "Click Plan again. When the planner asks whether to replace the unsaved layouts, click Replace.",
        ])
        .check("The new layouts keep the locked stone, and a note says it is cut first.")
        .why("The locked stone is cut first; the slabs below are for the remaining rough. Unlock lets the next plan use its piece again.")
        .goal(
            Goal::All(vec![event(ROUGH_STONE_LOCKED), event(ROUGH_PLAN_FINISHED)]),
            "A stone locked and a new plan finished",
        )
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Open it in the editor",
            "The stone goes to the editor with the axis it was planned with.",
        )
        .actions([
            "Click Open in Editor on the design row of the stone.",
            "If the design in the editor has unsaved changes you are asked to save, discard or cancel first.",
        ])
        .check("The main window shows the design. In the Stone tab, Crystal Axis shows Set for This Stone with the angles of the plan, and the axis shows over the stone.")
        .why("The picture is the stone as it will lie in the rough. As Material in the Stone tab goes back to the axis the material has.")
        .goal(event(ROUGH_STONE_OPENED), "A planned stone opened in the editor")
        .allow(FILES),
    )
}
