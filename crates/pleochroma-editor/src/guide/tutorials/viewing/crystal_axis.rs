//! The Crystal Axis in the editor: orienting a sapphire, a tourmaline and a tanzanite with the
//! Stone tab's presets and Second Axis, and turning the axis with the handle over the stone
//! (manual chapter "Crystal Axis").
//!
//! Every lesson starts from the Rich Teaching Design and picks its material in the first action
//! step, so the stone is the one the learner sees. What the steps wait for leaves no trace the
//! goals could read, except the last lesson's Undo: the Stone tab and the handles report it with
//! the events of [`crate::guide::tutorials::viewing_events`] (the Crystal Axis group). The lesson
//! about the rough's axis in the Rough Planner is in `crystal_axis_rough.rs`.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{VIEWPORT, check, event, path_traced_view};
use crate::guide::{
    Goal, Group, Guide, GuideCategory, GuideStep, Perform, StartingState,
    tutorials::viewing_events::{
        CRYSTAL_AXIS_AS_MATERIAL, CRYSTAL_AXIS_COLOR_PRESET_CHOSEN, CRYSTAL_AXIS_DRAG_CANCELLED,
        CRYSTAL_AXIS_DRAGGED, CRYSTAL_AXIS_PRESET_CHOSEN, CRYSTAL_AXIS_SET_FOR_STONE,
        CRYSTAL_AXIS_TOGGLED, CRYSTAL_SECOND_AXIS_CHANGED,
    },
};

/// The Stone tab's controls (the `DesignSettings` group gates them) and Undo.
const EFFECTS: &[Group] = &[Group::DesignSettings, Group::History];

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![sapphire(), tourmaline(), tanzanite(), set_axis()]
}

/// The step that picks `material` in the Stone tab (the same step every lesson of this file opens
/// its action steps with).
fn pick_material(material: &str, intro: &'static str, why: &'static str) -> GuideStep {
    GuideStep::new(format!("Choose {material}"), intro)
        .actions([
            "Open the Stone tab of the Parameters dock (View > Stone Properties, or Ctrl+Alt+1)."
                .to_owned(),
            format!("Set Material to {material}. It applies at once."),
        ])
        .check(format!("The Stone tab shows {material}."))
        .why(why)
        .goal(
            Goal::Material(material.to_owned()),
            format!("the material set to {material}"),
        )
        .highlight("stone_tab")
        .allow(EFFECTS)
}

/// The step that switches the Path-Traced view on, where the color of the stone can be judged.
fn path_traced(intro: &'static str) -> GuideStep {
    GuideStep::new("Look at it in Path-Traced", intro)
        .actions([
            "Click Path-Traced in the row of view pills above the picture, and give the picture a moment to settle.",
        ])
        .check("The picture shows the stone lit with traced light, and the pills show Path-Traced as chosen, with Solid chosen as well or on its own.")
        .why("The Solid view is flat-shaded and shows no color of the material. From Solid, the click gives Solid combined with Path-Traced, which shows it too; click Solid to turn the flat layer off.")
        .goal(path_traced_view(), "The Path-Traced view")
        .perform(Perform::ViewMode(1))
        .highlight("view_modes")
        .allow(VIEWPORT)
}

fn sapphire() -> Guide {
    Guide::new(
        "materials-orient-sapphire",
        "Orient a Sapphire",
        "Choose the direction of a sapphire's crystal axis for one stone, see the axis over the picture and read how much the stone doubles.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "One special direction",
            "A sapphire has one special direction, its crystal axis, and the stone looks different depending on how that axis lies in the cut stone.",
        )
        .actions([
            "The axis is a line through the stone, not an arrow: it has no front and no back.",
            "Tilt is how far the axis leans away from the straight-up direction through the table, from 0 to 90 degrees. Azimuth is the direction it leans toward, around the table, from 0 to 360 degrees.",
            "Light is split in two inside the stone, so the color you see and the doubling of the facets seen through the table depend on the axis. The Stone tab lets you choose its direction for one stone.",
        ])
        .why("Nothing changes in the design until you choose a direction. As Material keeps the axis the material has.")
        .allow(VIEWPORT),
    )
    .step(pick_material(
        "Sapphire",
        "The Crystal Axis section of the Stone tab belongs to a material that has an axis, so start with the Sapphire.",
        "A material without a crystal axis, such as Diamond, says so in the Crystal Axis section and offers nothing to turn.",
    ))
    .step(path_traced(
        "The color and the doubling show where light travels through the stone, so the Path-Traced view is where to look.",
    ))
    .step(
        GuideStep::new(
            "Show the Crystal Axis",
            "A line over the stone shows where its axis lies.",
        )
        .actions([
            "Click Crystal Axis in the row of pills above the picture. If the line is already showing, click it twice.",
        ])
        .check("A purple line crosses the stone with a round handle marked Axis at its upper end, and the Crystal Axis pill is lit.")
        .why("The choice is remembered. The line also switches itself on, for the session, the first time you set an axis.")
        .goal(event(CRYSTAL_AXIS_TOGGLED), "The Crystal Axis button pressed")
        .highlight("crystal_axis_toggle")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Set the axis for this stone",
            "Set for This Stone saves a direction of your own in the design.",
        )
        .actions([
            "In the Stone tab, find Crystal Axis.",
            "Click Set for This Stone.",
        ])
        .check("Tilt and Azimuth sliders appear under the presets, set to the material's own axis, so the picture does not jump.")
        .why("It is one step in the history and Undo takes it back. As Material, next to it, goes back to the axis the material has.")
        .goal(event(CRYSTAL_AXIS_SET_FOR_STONE), "Set for This Stone chosen")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Look through the table",
            "The presets turn the axis to a direction that is useful to cut for.",
        )
        .actions([
            "Under the presets, click Through the Table.",
        ])
        .check("The preset is lit, Tilt reads 0 degrees and the axis line stands straight up through the table.")
        .why("Looking along the axis, a stone shows the color of that direction only. The other two presets are In the Table Plane and Along the Width.")
        .goal(event(CRYSTAL_AXIS_PRESET_CHOSEN), "A preset chosen")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Read the Doubling",
            "Two more numbers join the row over the picture once a stone has an axis.",
        )
        .actions([
            "In the row of numbers over the picture, after Extinction, Axis is the angle between the crystal axis and the table normal. It reads 0 degrees now.",
            "Doubling is how far apart, in arc-minutes, the two images of the stone fall. Looking straight along the axis it is close to zero. Choose In the Table Plane in the Stone tab and watch it grow.",
        ])
        .why("The Doubling figure is measured on the picture, so it appears a moment after the picture settles. An isotropic material has no Axis or Doubling to show.")
        .allow(EFFECTS),
    )
}

fn tourmaline() -> Guide {
    Guide::new(
        "materials-tourmaline-axis-in-table",
        "Tourmaline With the Axis in the Table",
        "Lay a tourmaline's crystal axis flat in the table plane, then along the stone's width, and watch the Path-Traced color change.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Where the axis lies decides the color",
            "A tourmaline takes in one of its two light directions far more than the other, so the color you see through the table depends on how its axis lies.",
        )
        .actions([
            "Cutters therefore choose the direction of the axis on purpose: standing straight through the table, or lying in the plane of the table.",
            "With the axis in the table plane you look across it. The Crystal Axis section of the Stone tab has two presets for that.",
            "In the Table Plane turns the axis flat, and Along the Width turns it flat along the stone's width, its narrowest direction across the girdle.",
        ])
        .why("This lesson tries both flat presets and looks at the picture each time.")
        .allow(VIEWPORT),
    )
    .step(pick_material(
        "Tourmaline",
        "The Crystal Axis section belongs to a material that has an axis, so start with the Tourmaline.",
        "Every choice in the Crystal Axis section is saved with the design and is one step in the history.",
    ))
    .step(path_traced(
        "The color shows where light travels through the stone, so the Path-Traced view is where to look.",
    ))
    .step(
        GuideStep::new(
            "Lay the axis in the table plane",
            "A preset turns the axis in one click.",
        )
        .actions([
            "In the Stone tab, find Crystal Axis.",
            "Click In the Table Plane.",
        ])
        .check("The preset is lit, Tilt reads 90 degrees and the Path-Traced picture settles on a new color.")
        .why("Choosing a preset turns on Set for This Stone for you. As Material goes back to the axis the material has.")
        .goal(event(CRYSTAL_AXIS_PRESET_CHOSEN), "A preset chosen")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Turn it along the width",
            "In the Table Plane points the flat axis in one fixed direction. Along the Width points it along the stone.",
        )
        .actions([
            "Click Along the Width, the next preset under In the Table Plane.",
        ])
        .check("Along the Width is lit and the axis line lies along the narrowest direction across the girdle.")
        .why("The button appears once the design has a solved stone, because the width is measured on it. A stone that is not round looks different with the axis along its width than across it.")
        .goal(event(CRYSTAL_AXIS_PRESET_CHOSEN), "A preset chosen")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Compare the two",
            "The point of the lesson is the change of color between the choices.",
        )
        .actions([
            "Click In the Table Plane and Along the Width one after the other, and give the picture a moment to settle each time.",
            "Click As Material to put the material's own axis back, and Undo to take a choice back.",
        ])
        .why("The tilt of the axis stays 90 degrees in both presets: only the azimuth, the direction it lies in, changes.")
        .allow(EFFECTS),
    )
}

fn tanzanite() -> Guide {
    Guide::new(
        "materials-best-blue-tanzanite",
        "Best Blue Tanzanite",
        "Turn the Second Axis of a tanzanite and use the presets named by the colors they give, Best Blue Face-Up and Violet Face-Up.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Three directions, three colors",
            "Tanzanite has three principal directions that stand at right angles to each other, and it shows a different color along each of them, mostly a deep blue and a violet.",
        )
        .actions([
            "Orienting it takes two things: the Crystal Axis, and the Second Axis, which turns the other two directions around the Crystal Axis from 0 to 180 degrees.",
            "The presets named by the color they give, Best Blue Face-Up and Violet Face-Up, set both at once so that the color shown beside the name is the one you see through the table.",
        ])
        .why("Only a material with three different directions has a Second Axis. A uniaxial stone such as a sapphire needs the Crystal Axis only.")
        .allow(VIEWPORT),
    )
    .step(pick_material(
        "Tanzanite",
        "The color-named presets belong to the material, so start with the Tanzanite.",
        "A material without color-named presets shows none in the Crystal Axis section.",
    ))
    .step(path_traced(
        "The colors show where light travels through the stone, so the Path-Traced view is where to look.",
    ))
    .step(
        GuideStep::new(
            "Set the Second Axis",
            "The Second Axis belongs to the axis you set for this stone.",
        )
        .actions([
            "In the Stone tab, find Crystal Axis and click Set for This Stone.",
            "Under Second Axis, click Set, and drag the Turn slider a little to the right.",
        ])
        .check("Turn appears under Second Axis, a second line marked 2nd joins the axis over the stone, and the picture's color shifts as you drag.")
        .why("While the Second Axis is Not Set, a note under the controls says the color across the first axis is approximate, because the material's own second direction is used.")
        .goal(event(CRYSTAL_SECOND_AXIS_CHANGED), "The Second Axis changed")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Choose Best Blue Face-Up",
            "A color preset sets the Crystal Axis and the Second Axis in one click.",
        )
        .actions([
            "Under the presets, click Best Blue Face-Up.",
        ])
        .check("The preset is lit, the swatch beside its name shows the color you now see through the table, and the picture turns to the stone's best blue.")
        .why("It is one step in the history, however many numbers it changes.")
        .goal(event(CRYSTAL_AXIS_COLOR_PRESET_CHOSEN), "A color preset chosen")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Choose Violet Face-Up",
            "The other preset shows the other color.",
        )
        .actions([
            "Click Violet Face-Up.",
        ])
        .check("Violet Face-Up is lit, its swatch is violet and the picture turns violet.")
        .why("Both are the same stone. Only the direction of the rough it is cut from changes, and that is the choice the cutter makes.")
        .goal(event(CRYSTAL_AXIS_COLOR_PRESET_CHOSEN), "A color preset chosen")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Why the Second Axis matters",
            "The note under the controls tells you when the color is only approximate.",
        )
        .actions([
            "Click Not Set under Second Axis and read the note: Second axis not set; the color across the first axis is approximate.",
            "Click Violet Face-Up again to get the exact color back, and Undo to take a choice back.",
        ])
        .why("The Crystal Axis and the Second Axis are saved with the design, so it opens the way you left it.")
        .allow(EFFECTS),
    )
}

fn set_axis() -> Guide {
    Guide::new(
        "materials-set-crystal-axis",
        "Set a Stone's Crystal Axis",
        "Turn a stone's crystal axis with the handle over the picture, cancel a turn with Escape, undo it, and go back to the material's own axis.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Turn the axis with your hand",
            "With the Crystal Axis showing, its handle can be dragged, as well as the sliders in the Stone tab.",
        )
        .actions([
            "The handle is the round marker at the upper end of the axis line, marked Axis. Dragging it turns the axis around the stone's center, from straight up to flat.",
            "The picture follows while you drag, and one drag is one step in the history.",
            "Escape during a drag puts the axis back as it was when you pressed.",
        ])
        .why("The first drag also turns on Set for This Stone, with the angles the stone already showed, so the stone does not jump.")
        .allow(VIEWPORT),
    )
    .step(pick_material(
        "Sapphire",
        "The handle belongs to a material that has an axis, so start with the Sapphire.",
        "A stone without a crystal axis shows no line and no handle.",
    ))
    .step(
        GuideStep::new(
            "Show the Crystal Axis",
            "The handle is part of the line over the stone.",
        )
        .actions([
            "Click Crystal Axis in the row of pills above the picture. If the line is already showing, click it twice.",
        ])
        .check("A purple line crosses the stone with a round handle marked Axis at its upper end.")
        .why("The choice is remembered. The line also switches itself on, for the session, the first time you set an axis.")
        .goal(event(CRYSTAL_AXIS_TOGGLED), "The Crystal Axis button pressed")
        .highlight("crystal_axis_toggle")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Drag the handle",
            "Press on the handle, move, and let go.",
        )
        .actions([
            "Press on the round Axis handle, drag it to a new place and let go.",
        ])
        .check("The line leans to where you let go, the Stone tab shows Set for This Stone with a new Tilt and Azimuth, and the Axis angle over the picture has changed.")
        .why("The handle is only grabbed when you press on it. A press anywhere else orbits the picture as before.")
        .goal(event(CRYSTAL_AXIS_DRAGGED), "The handle dragged")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Escape puts it back",
            "A turn you did not want can be dropped before you let go.",
        )
        .actions([
            "Press on the handle again and drag it somewhere else, but do not let go.",
            "Press Escape. Then let go.",
        ])
        .check("The line jumps back to where it was before this drag.")
        .why("Only this drag is dropped: the turn you made before stays.")
        .goal(event(CRYSTAL_AXIS_DRAG_CANCELLED), "The drag cancelled with Escape")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Undo the turn",
            "Every turn is one step in the history, however long the drag was.",
        )
        .actions([
            "Click Undo, or press Ctrl+Z.",
        ])
        .check("The stone looks as the material has it again, and the Stone tab shows As Material selected.")
        .why("The whole drag comes back in one step. Redo, Ctrl+Y, brings it again.")
        .goal(
            check("the stone has the material's own axis again", |ctx| {
                !ctx.design.render_effects.c_axis_override_enabled
            }),
            "Undo of the turn",
        )
        .allow(&[Group::History]),
    )
    .step(
        GuideStep::new(
            "Back to the material's axis",
            "Set for This Stone and As Material are the two ends of the choice.",
        )
        .actions([
            "In the Stone tab, find Crystal Axis.",
            "Click Set for This Stone to give the stone an axis of its own again, then click As Material to hand it back to the material.",
        ])
        .check("As Material is selected, the Tilt and Azimuth sliders are gone and the line lies on the material's own axis.")
        .why("As Material is how a design with an axis of its own goes back: the stone is again rendered as it was cut from the rough.")
        .goal(event(CRYSTAL_AXIS_AS_MATERIAL), "As Material chosen")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
}
