//! Silk, play-of-color and per-ray curves in the editor: the Stone tab's Inclusions and
//! Play-of-Color (Schiller) groups, and the Material Editor's Per-Axis Curves (manual chapters
//! "Inclusions and Silk", "Play-of-Color and Schiller" and "Materials").
//!
//! Every lesson starts from the Rich Teaching Design and picks its material in an action step, so
//! the stone is the one the learner sees. What the Stone tab changes is in the design, so the
//! steps wait for it with a [`check`] on the design (see `effects_common.rs`); the Crystal Axis
//! and the saved material report their events as in the other lessons. The Material Editor covers
//! the guide panel while it is open, so its step carries every instruction it needs.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{
    VIEWPORT, check,
    effects_common::{
        EFFECTS, cats_eye_chosen, coverage_moved, inclusions_chosen, path_traced, pattern_pinfire,
        pick_material, range_moved, reseeded, silk_turned, structure_on,
    },
    event,
};
use crate::guide::{
    Goal, Group, Guide, GuideCategory, GuideStep, StartingState,
    tutorials::viewing_events::{CRYSTAL_AXIS_SET_FOR_STONE, CUSTOM_MATERIAL_SAVED},
};

/// The Stone tab, the Advanced controls and Undo: the Material Editor's Per-Axis Curves switch is
/// an Advanced control, which the Simple interface hides unless a step unlocks the group.
const CURVES: &[Group] = &[Group::DesignSettings, Group::Advanced, Group::History];

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![silk_sapphire(), opal(), labradorite(), axis_curves()]
}

fn silk_sapphire() -> Guide {
    Guide::new(
        "materials-silk-sapphire",
        "Silk in a Sapphire",
        "Give a sapphire its silk with the Stone tab's Inclusions, turn it about the Crystal Axis and see why a Cat's-Eye needs a Second Axis.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Silk is a set of needles",
            "The fine needles a sapphire grows are called silk. They lie in a fixed direction in the crystal, and that direction decides how the stone glints.",
        )
        .actions([
            "In corundum (sapphire and ruby) the needles lie flat in the plane across the Crystal Axis, in three directions 60 degrees apart. That is why a star stone shows six rays.",
            "The Inclusions group of the Stone tab draws a preset of such needles inside the stone. Density says how many, Sheen how widely they spread the light, and Silk Color what color the light has.",
            "Silk Turn turns the needles about the Crystal Axis, so the glints land where you want them.",
        ])
        .why("Nothing changes in the design until you choose a preset. None keeps the plain stone.")
        .allow(VIEWPORT),
    )
    .step(pick_material(
        "Sapphire",
        "The Inclusions group offers the presets of the material, so start with the Sapphire.",
        "A material with no preset says so in the Inclusions group, and offers nothing to choose.",
    ))
    .step(path_traced(
        "Silk glints where light travels through the stone, so the Path-Traced view is where to look.",
    ))
    .step(
        GuideStep::new(
            "Set the axis for this stone",
            "The needles are placed with respect to the Crystal Axis, so this stone needs one of its own.",
        )
        .actions([
            "In the Stone tab, find Crystal Axis and click Set for This Stone.",
            "Click Through the Table under the presets, so the axis stands straight up and the needles lie flat in the plane of the table.",
        ])
        .check("Tilt and Azimuth sliders appear under the presets, and the axis reads 0 degrees from the table normal.")
        .why("Without an axis of its own, the stone takes the axis the material has. Both ways show the silk.")
        .goal(event(CRYSTAL_AXIS_SET_FOR_STONE), "Set for This Stone chosen")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Choose a silk preset",
            "A preset is one click.",
        )
        .actions([
            "In the Stone tab, find Inclusions.",
            "Click Star Silk. Fine Silk and Rutile Silk are the other two presets of a sapphire.",
        ])
        .check("The preset is lit, Density, Silk Turn, Sheen and Silk Color appear under it, and the picture shows glints inside the stone.")
        .why("Star Silk is a thin set of needles that makes a sharp star on a cabochon. The choice is saved with the design and is one step in the history.")
        .goal(
            check("an inclusion preset chosen", inclusions_chosen),
            "A silk preset chosen",
        )
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Turn the silk",
            "Silk Turn turns the needles about the Crystal Axis.",
        )
        .actions([
            "Click Set Silk Turn, then drag the Silk Turn slider a little. It runs from 0 to 180 degrees.",
            "Switch Show Silk Directions on to draw the directions of the needles over the stone as lines. It is a view aid and is not saved.",
        ])
        .check("The glints move as you drag, and with Show Silk Directions on, amber lines turn with them.")
        .why("While Silk Turn is not set, the needles follow the stone's Second Axis, if it has one.")
        .goal(
            check("a Silk Turn set", silk_turned),
            "Silk Turn set",
        )
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "A Cat's-Eye needs a Second Axis",
            "Some silk lies along one direction in the crystal that is not the Crystal Axis. A chrysoberyl cat's-eye is the example.",
        )
        .actions([
            "In the Stone tab, set Material to Chrysoberyl (Yellow).",
            "Under Inclusions, Cat's-Eye Silk is greyed out, with a note that it needs a Second Axis.",
            "Under Crystal Axis, click Set for This Stone. Under Second Axis, click Set.",
            "Now click Cat's-Eye Silk.",
        ])
        .check("Cat's-Eye Silk is lit and the picture shows a bright band across the stone, at right angles to the needles.")
        .why("A chrysoberyl is a stone with three different directions. The needles lie along one of them, so the stone has to say which one that is.")
        .goal(
            check("the Cat's-Eye Silk preset chosen", cats_eye_chosen),
            "Cat's-Eye Silk chosen",
        )
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Facets and cabochons",
            "The same needles look different in a faceted stone and in a cabochon.",
        )
        .actions([
            "A cabochon has one smooth curved top, so the needles throw one star or one band across it.",
            "A faceted stone breaks the picture into flat facets. Each facet shows a bright sheen only when its angle lines up with the needles, so the sheen moves from facet to facet as you turn the stone.",
            "Click Undo to take back a choice, or None under Inclusions to go back to the plain stone.",
        ])
        .why("Figures ignore Inclusions, Surface Finish and Play-of-Color. The pictures show silk, the numbers do not. Every control is unlocked again."),
    )
}

fn opal() -> Guide {
    Guide::new(
        "materials-opal-play-of-color",
        "Opal Play-of-Color",
        "Switch the flashes of color of a precious opal on, change their pattern and range, draw them again with Reseed and compare a black opal.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Patches of flashing color",
            "A precious opal is made of tiny spheres stacked in orderly patches. Each patch reflects one color of light, and the color changes as the stone or the light moves.",
        )
        .actions([
            "The Play-of-Color group of the Stone tab draws such patches. Pattern sets how large they are: Pinfire is many tiny points, Harlequin medium patches and Broad Flash a few large ones.",
            "Color Range is the span of colors the patches reflect, Purity how pure each color is, and Coverage how much of the stone flashes.",
            "The Seed fixes which patches flash. Reseed draws a new arrangement with the same settings.",
        ])
        .why("The group is there for a material that has this structure: the two precious opals and labradorite. Nothing changes in the design until you switch it on.")
        .allow(VIEWPORT),
    )
    .step(pick_material(
        "Precious Opal (White)",
        "The Play-of-Color group belongs to a material that has the structure, so start with the white opal.",
        "A material without it, such as Diamond, shows no Play-of-Color group.",
    ))
    .step(path_traced(
        "The flashes show where light travels through the stone, so the Path-Traced view is where to look.",
    ))
    .step(
        GuideStep::new(
            "Switch Play-of-Color on",
            "The flashes start from the pattern the material usually shows.",
        )
        .actions([
            "In the Stone tab, find Play-of-Color and switch Show Play-of-Color on.",
        ])
        .check("Patches of color flash across the stone, and Pattern, Color Range, Purity, Coverage and a Seed number appear under the switch.")
        .why("It is one step in the history, and the choice is saved with the design. Ray-Traced ignores Play-of-Color.")
        .goal(
            check("Play-of-Color switched on", structure_on),
            "Play-of-Color switched on",
        )
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Choose Pinfire",
            "The pattern decides how big the patches are.",
        )
        .actions([
            "Under Pattern, click Pinfire.",
        ])
        .check("Pinfire is lit and the picture breaks into many tiny points of color.")
        .why("Harlequin, the usual pattern of an opal, is made of larger patches. Broad Flash is a few large ones.")
        .goal(
            check("the Pinfire pattern chosen", pattern_pinfire),
            "Pinfire chosen",
        )
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Move the color range",
            "Color Range names the colors the patches can show, as wavelengths in nanometres.",
        )
        .actions([
            "Drag Color Range, Low End to the right.",
        ])
        .check("The patches lose their bluest colors and lean toward green and red as you drag.")
        .why("Around 450 is blue, 550 green and 650 red. The low end never goes above the high end.")
        .goal(
            check("the Color Range changed", range_moved),
            "Color Range changed",
        )
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Draw the patches again",
            "Reseed keeps every setting and changes the places.",
        )
        .actions([
            "Click Reseed, beside the Seed number.",
        ])
        .check("The Seed number changes and the patches flash in other places.")
        .why("A Reseed is one step in the history, so Undo brings the old arrangement back. The seed is saved with the design, so it opens the way you left it.")
        .goal(
            check("a new seed drawn", reseeded),
            "Reseed pressed",
        )
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Try a black opal",
            "A black opal has a dark body, so its flashes stand out more.",
        )
        .actions([
            "In the Stone tab, set Material to Precious Opal (Black).",
            "If the flashes are off, switch Show Play-of-Color on again.",
        ])
        .check("The stone is dark and the patches of color shine against it.")
        .why("Figures ignore Inclusions, Surface Finish and Play-of-Color, so the numbers do not change with the flashes.")
        .goal(
            Goal::Material("Precious Opal (Black)".to_owned()),
            "the material set to Precious Opal (Black)",
        )
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
}

fn labradorite() -> Guide {
    Guide::new(
        "materials-labradorite-schiller",
        "Labradorite Flash",
        "Switch the Schiller of a labradorite on, change how much of the stone flashes and turn the stone to catch the flash.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "A flash from one direction",
            "A labradorite looks grey from most directions and flashes a color when it is turned to the right angle. This flash is called the Schiller.",
        )
        .actions([
            "The Stone tab calls the group Schiller for this material. It has the same controls as an opal's Play-of-Color: Pattern, Color Range, Purity, Coverage and a Seed.",
            "The pattern the material starts with is Broad Flash, a few large flashes.",
        ])
        .why("The Moonstone is another stone with a sheen from one direction. Its preset, Adularescence, is in the Inclusions group.")
        .allow(VIEWPORT),
    )
    .step(pick_material(
        "Labradorite",
        "The Schiller group belongs to a material that has the structure, so start with the Labradorite.",
        "A material without it shows no Schiller group.",
    ))
    .step(path_traced(
        "The flash shows where light travels through the stone, so the Path-Traced view is where to look.",
    ))
    .step(
        GuideStep::new(
            "Switch Schiller on",
            "The flash starts from the pattern the material usually shows.",
        )
        .actions([
            "In the Stone tab, find Schiller and switch Show Schiller on.",
        ])
        .check("A flash of color crosses the stone, and Pattern, Color Range, Purity, Coverage and a Seed number appear under the switch.")
        .why("It is one step in the history, and the choice is saved with the design. Ray-Traced ignores Schiller.")
        .goal(
            check("Schiller switched on", structure_on),
            "Schiller switched on",
        )
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Change the coverage",
            "Coverage is the share of the stone that flashes.",
        )
        .actions([
            "Drag Coverage to the left.",
        ])
        .check("Less of the stone flashes as you drag. At 0% none of it does.")
        .why("The flash of a real labradorite covers part of the stone, and the rest stays grey. Coverage is the setting that matches the stone you have in mind.")
        .goal(
            check("the Coverage changed", coverage_moved),
            "Coverage changed",
        )
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Turn the stone",
            "The flash is the point of the stone, so catch it.",
        )
        .actions([
            "Drag the picture to turn the stone. The flash comes and goes as the angle changes.",
            "Click Reseed to move the flashing patches, and Undo to bring the old arrangement back.",
        ])
        .why("A labradorite is cut so that its best flash faces up through the table. Figures ignore Play-of-Color, so Checks and Compare do not change with the flash. Every control is unlocked again."),
    )
}

fn axis_curves() -> Guide {
    Guide::new(
        "materials-axis-curves",
        "Ordinary and Extraordinary Curves",
        "Give a custom material a separate dispersion curve for the extraordinary ray with the Material Editor's Per-Axis Curves.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Two rays, two curves",
            "Light in a uniaxial stone splits into two rays. The ordinary ray sees the main refractive-index curve of the material. The extraordinary ray sees another index, and its own dispersion.",
        )
        .actions([
            "Per-Axis Curves: Simple adds the birefringence to the main curve as one constant, so both rays disperse alike.",
            "Curves lets you type the Sellmeier or Cauchy coefficients of the extraordinary ray, so each ray has its own fire. A biaxial stone gets an alpha and a gamma curve.",
            "Some built-in materials, such as Sapphire, carry curves of their own. This lesson makes one for a material of your own.",
            "The switch belongs to the Advanced interface. If the pill at the top left of the window reads Simple, click it to switch to Advanced before you open the Material Editor.",
        ])
        .why("A curve is traced exactly as typed, on the graphics card and on the processor alike. The switch appears only for a material that is not isotropic.")
        .highlight("stone_tab")
        .allow(CURVES),
    )
    .step(
        GuideStep::new(
            "Give the extraordinary ray a curve",
            "The Material Editor opens over the window, so this step carries everything you need.",
        )
        .actions([
            "Open the Stone tab of the Parameters dock (View > Stone Properties, or Ctrl+Alt+1), then click New/Edit Material....",
            "Pick Sapphire in Load Preset Template. It fills in a uniaxial material.",
            "Type a Material Name that no built-in material uses, for example Tutorial curves.",
            "Under Per-Axis Curves, click Curves. Curve For reads Extraordinary Ray, and its fields start from a Cauchy fit of the numbers above.",
            "Change one coefficient a little, and read the line under the fields: nd, the spread and the Abbe number of that ray.",
            "Click Save.",
        ])
        .check("A toast says Saved custom material with the name you typed.")
        .why("Save stays disabled while a curve cannot be traced; the red line under the fields says why. A biaxial material needs both of its curves.")
        .goal(event(CUSTOM_MATERIAL_SAVED), "A custom material saved")
        .highlight("stone_tab")
        .allow(CURVES),
    )
    .step(
        GuideStep::new(
            "Use it on the design",
            "Saving only adds the material to the lists. The design keeps its own material until you change it.",
        )
        .actions([
            "In the Stone tab, open the Material drop-down: your material is listed after the built-ins. Pick it.",
            "To change the curve later, open the Material Editor, pick the material and click Curves again.",
        ])
        .why("Switching a material back to Simple and saving replaces the extraordinary curve with the constant birefringence."),
    )
    .step(
        GuideStep::new(
            "More in the Material Editor",
            "The same dialog holds two more sections for a material of your own.",
        )
        .actions([
            "Inclusion Families: up to four families of needles or platelets, each with a direction, a density, a color and a width. The Stone tab then shows them as part of the material, and they are edited here, not there.",
            "Play-of-Color: switch it on, choose whether it imitates an opal or a labradorite, and set the pattern, size, color range, purity and coverage. Reseed draws a new arrangement and Reset To Defaults goes back to the imitated material's.",
        ])
        .why("Both are saved with the material in your library, so every design that uses it shows them. Every control is unlocked again."),
    )
}
