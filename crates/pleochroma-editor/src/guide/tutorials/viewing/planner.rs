//! The Rough Planner in depth (manual chapter 15): four lessons that follow the first one,
//! `library-rough-planner`, which opens the planner and runs a first plan.
//!
//! - `library-planner-model-rough`: the shapes, the 3D view, cutting in the picture, Undo and
//!   the mesh roughs.
//! - `library-planner-results`: result cards, design rows, the Cut Plan and the library links.
//! - `library-planner-saved-plans`: saving results, opening a saved plan, exporting it.
//! - `library-planner-open-in-editor`: Open in Editor and what arrives on the bench.
//!
//! The planner is a window of its own, so none of its controls can be outlined from the guide
//! panel; every step names the control in words. What happens in the planner leaves no trace in
//! the design, so the steps wait for the events the desktop raises from `gui/rough_plan` (see
//! [`super::super::viewing_events`]). The lessons after the first one start by making a plan,
//! because a result is what they are about.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{FILES, VIEWPORT, event};
use crate::guide::{
    Goal, Group, Guide, GuideCategory, GuideStep, StartingState,
    tutorials::viewing_events::{
        ROUGH_CUT_FROM_VIEW, ROUGH_CUT_PLAN_OPENED, ROUGH_DESIGN_SELECTED, ROUGH_MODEL_UNDONE,
        ROUGH_PLAN_EXPORTED, ROUGH_PLAN_FINISHED, ROUGH_PLAN_SAVED, ROUGH_PLANNER_OPENED,
        ROUGH_RESULT_SELECTED, ROUGH_SAVED_PLAN_OPENED, ROUGH_SHAPE_CHOSEN, ROUGH_SHOWN_IN_LIBRARY,
        ROUGH_STONE_OPENED, ROUGH_VIEW_MOVED,
    },
};

/// The Preform tab and Undo: the step that looks at the planned piece in the editor.
const PREFORM: &[Group] = &[Group::PreformTab, Group::History];

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![model_rough(), results(), saved_plans(), open_in_editor()]
}

/// The first step of the lessons that are about a result: make a plan to read, save or open.
fn plan_a_layout() -> GuideStep {
    GuideStep::new(
        "Plan a Layout",
        "Results need a plan, so this lesson starts by making one.",
    )
    .actions([
        "Choose Library, then Plan Rough... in the menu bar.",
        "Type a size in Rough X (mm), Rough Y (mm) and Rough Z (mm), for example 18.4, 11 and 9.6.",
        "Set Designs (Up To) to 1, then click Plan or press Ctrl+Enter.",
    ])
    .check("Up to ten result cards appear in the column on the right.")
    .why("The Rough Planner lesson explains every field of this form. A short search in the library first keeps the first plan quick, because the first plan measures every design it may use.")
    .goal(event(ROUGH_PLAN_FINISHED), "A plan finished")
    .allow(VIEWPORT)
}

fn model_rough() -> Guide {
    Guide::new(
        "library-planner-model-rough",
        "Model the Rough Shape",
        "Choose a rough shape, look at it from every side, cut it in the 3D view and take a cut back.",
        GuideCategory::RoughPlanning,
    )
    .starting(StartingState::CurrentDesign)
    .step(
        GuideStep::new(
            "Open the Planner",
            "The Rough Planner is a window of its own, and this lesson works in it.",
        )
        .actions(["Choose Library, then Plan Rough... in the menu bar."])
        .check("A window opens with the rough on the left, a 3D view in the middle and the results on the right.")
        .why("The Rough Planner lesson covers the first plan. This one is about the model: its shape and the way you cut it.")
        .goal(event(ROUGH_PLANNER_OPENED), "The Rough Planner opened")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Choose a Shape",
            "The ROUGH section starts with four choices of shape.",
        )
        .actions([
            "Click Cylinder. Diameter (mm) and Length (mm) take the place of the three box sizes, and an Axis choice appears.",
            "Type 12 in Diameter (mm) and 20 in Length (mm), pressing Enter after each. Axis Y, the default, is the direction the rod runs along.",
            "Pebble is the ellipsoid inscribed in the Rough X, Y and Z box, so it holds about half the volume of a Block of the same size.",
        ])
        .check("The readout under the material shows the model's volume and its weight in carats.")
        .why("Switching shape keeps the size as far as the shapes allow, and each switch is one undo step. Mesh (OBJ/STL/PLY) is the fourth choice; the last step of this lesson is about it.")
        .goal(event(ROUGH_SHAPE_CHOSEN), "A shape chosen")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Look from Every Side",
            "The 3D view shows the rough you are modelling, and it is yours to turn.",
        )
        .actions([
            "Drag the picture with the left mouse button to orbit the rough, and roll the mouse wheel to zoom.",
            "Click Front, Top and Side above the picture to snap the camera to those views.",
            "Click Reset, or double-click empty space in the picture, to go back to the starting view.",
        ])
        .check("The picture turns and zooms, and each view button snaps the camera at once.")
        .why("While you drag or roll the wheel the picture is drawn at half resolution and sharpens when you stop. Once you have clicked the picture, the arrow keys turn it, Plus and Minus zoom it and Home resets it.")
        .goal(event(ROUGH_VIEW_MOVED), "The view moved")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Cut in the Picture",
            "Flat cuts can be made in the 3D view itself, without the buttons.",
        )
        .actions([
            "Click Face from View. The hint line under the picture reads Face from view: click rough face (Esc to cancel).",
            "Move the pointer over the rough. When the hint reads Click to cut this face flat, click.",
            "On a block, an edge or a corner within a few pixels of the pointer can be clicked without Face from View.",
        ])
        .check("A numbered cut row appears under SHAPE, with how much it removes, and the cut face is tinted in the picture.")
        .why("A single click adds its cut after a moment, so that a double-click can add it at once. A cylinder and a pebble have no edges or corners, so +Edge and +Corner are dimmed for them, but a face cut works on every shape. Cuts are only made while the picture shows the Rough, and not while a plan runs.")
        .goal(event(ROUGH_CUT_FROM_VIEW), "A cut made in the picture")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Take the Cut Back",
            "Every finished edit of the model is one undo step.",
        )
        .actions([
            "Click Undo above the cut list, or press Ctrl+Z while no text box has the keyboard.",
            "Click Redo, or press Ctrl+Y, to bring the cut back.",
        ])
        .check("The cut row disappears and the picture shows the rough without that cut.")
        .why("A step is adding or deleting a cut, switching the shape, finishing a size field with Enter, opening a saved plan or New. The planner keeps 100 steps each way. New in the title bar starts again with an empty block.")
        .goal(event(ROUGH_MODEL_UNDONE), "A model edit undone")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Roughs from a Scan",
            "A rough with a notch or a hollow can be imported as a mesh.",
        )
        .actions([
            "Choose the unit of the file's numbers in Mesh File Units first, then click Mesh (OBJ/STL/PLY) and pick a Wavefront .obj, an .stl or a .ply file.",
            "A closed mesh is used as it is: stones keep out of the notches, and the weight and the yield use the mesh's own volume.",
            "A mesh that is open, crosses itself or has too many triangles falls back to its convex outline, and the status line says why.",
            "Add Inclusion... keeps stones clear of a flaw you know inside a mesh rough, and Fit to Weight scales the whole mesh to a weighed carat.",
        ])
        .why("A file that is read in the wrong unit is refused when it is smaller than 1 mm or larger than 2000 mm, and the message names the unit to try. Every control is unlocked again."),
    )
}

fn results() -> Guide {
    Guide::new(
        "library-planner-results",
        "Read the Planner Results",
        "Pick a layout, a design and a Cut Plan, read the numbers on a card and find its designs in the library.",
        GuideCategory::RoughPlanning,
    )
    .starting(StartingState::CurrentDesign)
    .step(plan_a_layout())
    .step(
        GuideStep::new(
            "Pick a Layout",
            "Each card is one layout of stones in your rough, ranked by the weight of finished stone.",
        )
        .actions([
            "Click a card that is not outlined. The card in view is outlined, and result 1 is shown when a plan finishes.",
            "Read its header: the rank, the total weight in carats, then the yield (the stones' volume as a share of the rough's volume) and the stone count.",
        ])
        .check("The 3D view shows that layout: the stones inside a faint glass volume of the rough.")
        .why("The Rough, Saw and Stones switches above the picture show or hide the rough, the amber saw pieces and the stones. Rough | Result switches back to the model you are editing.")
        .goal(event(ROUGH_RESULT_SELECTED), "A result picked")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Pick a Design",
            "A card has one row for each design it uses, most stones first.",
        )
        .actions([
            "Click a design row inside a card. Its text starts with the stone count, for example 2 \u{d7} Round Brilliant.",
            "Look at the picture: the stones of that design are outlined, and the color of each design matches the swatch on its row.",
        ])
        .check("The row is marked and its stones are outlined in the 3D view.")
        .why("Clicking a stone in the picture does the same, and clicking empty space clears the outline. Hover a stone to read its design, its weight and its slab, bar and piece number.")
        .goal(event(ROUGH_DESIGN_SELECTED), "A design picked")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Open the Cut Plan",
            "The Cut Plan is the written sawing order of a layout.",
        )
        .actions([
            "Click Cut Plan on a card.",
            "Read it from the top: the order of the cuts and what each one loses, then each slab, the bars cut from it and the pieces cut from each bar, with the design, size and weight of the stone in every piece.",
        ])
        .check("The cutting steps unfold under the card as text.")
        .why("The numbers restart at every level, and the hover text in the 3D view uses the same slab, bar and piece numbers. An exact single stone is not sawn, so its plan has no slabs: it names the design and the box around the stone.")
        .goal(event(ROUGH_CUT_PLAN_OPENED), "A Cut Plan opened")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Read the Numbers",
            "A card and its design rows carry more than the weight.",
        )
        .actions([
            "Under the stone count: the number of saw cuts and an upper estimate of the weight the saw turns to dust.",
            "Each design row lists Weight, Size, Ratios, Volume, Orientation, Fill and Optics.",
            "The summary line above the cards says how many layouts came from how many designs, and how long the plan took.",
            "The Exclude pill on a design row keeps that design out of every later plan.",
        ])
        .why("The list is ranked by weight only, ties going to fewer stones, and at most three layouts share one set of designs. Beside the best mix of designs the planner also lists next-best mixes, so there are alternatives to compare. Weights and yields are an estimate for a rough that is clean, after your kerf and allowance.")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Find the Designs in the Library",
            "A result names designs from your library, and the library can show just those.",
        )
        .actions([
            "Click Show All in Library, under the cards, to list every design of the results in the library.",
            "Result in Library on one card does the same for that layout alone.",
        ])
        .check("The library list narrows under a banner that names the plan and how many designs it holds.")
        .why("A Clear link on the banner brings the whole list back. While the banner is on, Current Filter in the planner means exactly those designs.")
        .goal(event(ROUGH_SHOWN_IN_LIBRARY), "The designs shown in the library")
        .allow(VIEWPORT),
    )
}

fn saved_plans() -> Guide {
    Guide::new(
        "library-planner-saved-plans",
        "Save and Share Plans",
        "Save results as a plan, open the plan again and export it as a file.",
        GuideCategory::RoughPlanning,
    )
    .starting(StartingState::CurrentDesign)
    .step(plan_a_layout())
    .step(
        GuideStep::new(
            "Tick and Save",
            "Nothing is saved unless you ask, and you choose which layouts to keep.",
        )
        .actions([
            "Tick the box at the top left of result 1 and of result 2 (Keep Result N for Saving).",
            "Click Save Selected (2) in the title bar. Save All in the footer under the cards saves every result instead.",
            "A name row opens above the results with a suggested name. Click Save.",
        ])
        .check("A line under RESULTS reads Saved 2 results as, followed by the name.")
        .why("Ctrl+S does the same as Save Selected, and saves every result when none is ticked. A saved plan holds the model and the settings the results were planned for, the material and the weighed carat, and the exact place of every stone.")
        .goal(event(ROUGH_PLAN_SAVED), "Results saved as a plan")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Open It Again",
            "Saved Plans in the title bar lists the plans in your library database.",
        )
        .actions([
            "Click Saved Plans in the title bar, or press Ctrl+O. The list takes the place of the results.",
            "Click Open on the plan you just saved.",
            "If the planner asks whether to replace unsaved layouts, click Replace. The layouts you ticked are safe in the plan.",
        ])
        .check("A banner above the results names the saved plan and its date, and the left column holds the plan's rough and settings.")
        .why("Open puts the rough, the settings and the material back as one undo step and shows the layouts as they were saved; nothing is calculated again. Re-Plan in the banner plans again with the model and settings now in the left column. Results that were never saved are never replaced without asking.")
        .goal(event(ROUGH_SAVED_PLAN_OPENED), "A saved plan opened")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Share a Plan as a File",
            "A saved plan can leave the program as a file.",
        )
        .actions([
            "Click Saved Plans to open the list, then click Export on the plan.",
            "Choose a folder in the Save dialog, keep the suggested name, which ends in .psrd, and click Save.",
        ])
        .check("A line under the Saved Plans heading says the plan was exported.")
        .why("A .psrd file is plain text. Import..., at the top of the list, reads one back as a new plan, in this program or in another copy of it. Rename changes a plan's name in place, and Delete asks first.")
        .goal(event(ROUGH_PLAN_EXPORTED), "A plan exported")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Read the Status Chips",
            "A plan is checked against your library each time it is opened.",
        )
        .actions([
            "A deleted chip means the design is gone from the library. Its name stops being a link and its stones are drawn as plain grey boxes.",
            "A changed chip means the design is still there but its shape no longer matches the one the plan was made with. It is drawn with its current shape.",
            "A by title chip means the design's number is gone but one design has the same title and shape, so the planner uses that one. This is how a file from another library finds its designs.",
        ])
        .why("The full wording of each chip is on the line under the design's name. The plan does not record which designs were excluded from planning: that belongs to the library. Every control is unlocked again."),
    )
}

fn open_in_editor() -> Guide {
    Guide::new(
        "library-planner-open-in-editor",
        "From Plan to Editor",
        "Open a planned stone in the editor with the piece of rough it comes from.",
        GuideCategory::RoughPlanning,
    )
    .starting(StartingState::CurrentDesign)
    .step(plan_a_layout())
    .step(
        GuideStep::new(
            "Open a Stone in the Editor",
            "A design row can hand its stone to the editor, together with the rough it is cut from.",
        )
        .actions([
            "Click Open in Editor on a design row of result 1.",
            "If the design in the editor has unsaved changes you are asked to save, discard or cancel first.",
        ])
        .check("The main window shows the design, and a message names the stone and the result it came from.")
        .why("The design opens as Load Selected opens it. The stone is the first one of that design in the result. Open in Editor is dimmed while a plan runs and while a remote library is selected.")
        .goal(event(ROUGH_STONE_OPENED), "A planned stone opened in the editor")
        .allow(FILES),
    )
    .step(
        GuideStep::new(
            "Find the Planned Piece",
            "The piece of rough comes with the design as its preform.",
        )
        .actions(["Click Preform in the inspector below the tier table."])
        .check("A note under the shape box starts with Planned piece from the Rough Planner and names the result and the stone.")
        .goal(Goal::InspectorTab(1), "the Preform tab open")
        .highlight("preform_tab")
        .allow(PREFORM),
    )
    .step(
        GuideStep::new(
            "What Arrived",
            "One edit put the piece, the width and the orientation into the design.",
        )
        .actions([
            "The preform is the sawn piece the stone comes from: the stone's box plus the kerf and the allowances, clipped by the rough's outline. It may have any number of flat sides, and the number fields show its bounding box.",
            "Girdle Diameter is set to the stone's real width, and the piece follows the orientation you chose in the plan.",
            "Press Undo once to take the piece and the width back. Apply Preform replaces the piece with a Block or a Cylinder, and Undo brings the piece back.",
            "A plan that carries a rough color sets the color material in the same edit; the Rough Color lessons cover that.",
        ])
        .why("The piece is saved in the design file. Inclusions you gave the planner do not reach the editor, and a mesh rough arrives as its convex outline. Every control is unlocked again."),
    )
}
