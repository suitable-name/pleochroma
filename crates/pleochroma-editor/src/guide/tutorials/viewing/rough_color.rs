//! Rough color and color zoning: the Rough Color wizard, the rough's color in the Rough Planner,
//! and the built-in zoned Ametrine (manual chapter 24).
//!
//! The wizard needs photographs of a rough on a camera rig, which no lesson can ask the learner to
//! take on the spot, so its lesson walks through the steps and reads the result of a plan that
//! already has one. The only action it waits for is the window opening. The planner lesson waits
//! for the two controls that need a saved plan with a color (Best Color and Use Color).

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{FILES, VIEWPORT, event, path_traced_view};
use crate::guide::{
    Goal, Group, Guide, GuideCategory, GuideStep, Perform, StartingState,
    tutorials::viewing_events::{
        ROUGH_COLOR_BEST_CHOSEN, ROUGH_COLOR_USED, ROUGH_COLOR_WIZARD_OPENED,
    },
};

/// The Stone tab's material controls and Undo, for the Ametrine lesson.
const MATERIALS: &[Group] = &[Group::DesignSettings, Group::History];

/// The lessons of this file: two in the library section, one in the viewing section.
pub fn guides() -> Vec<Guide> {
    vec![wizard(), in_planner(), ametrine()]
}

fn wizard() -> Guide {
    Guide::new(
        "library-rough-color-wizard",
        "The Rough Color Wizard",
        "Work out the color of a piece of rough from photos on a backlit rig, and read how sure the result is.",
        GuideCategory::RoughPlanning,
    )
    .starting(StartingState::CurrentDesign)
    .step(
        GuideStep::new(
            "What the wizard does",
            "The Rough Color wizard turns photos of a rough stone on a backlit camera rig into the color of the stone.",
        )
        .actions([
            "The result is an absorption spectrum per millimetre of path, not a texture. A stone cut from the rough then shows that color at its real size.",
            "You need a mesh rough in the Rough Planner, a camera rig with eight fixed cameras and a backlight, and the mesh aligned to the rig in Locate Inclusion from Photos.",
            "For each view you need a photo of the stone and a photo of the empty rig with the backlight on (the white frame). A photo of the empty rig with the backlight off (the dark frame) is strongly recommended.",
            "One backlight gives one color. The wizard does not recover a color change, a color that depends on the direction of the light, or scattering.",
        ])
        .why("Chapter 24 of the manual has the capture protocol: fixed exposure, fixed ISO, fixed white balance and focus, and RAW if the camera can shoot it.")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Open the wizard",
            "The wizard is a window of its own, opened from the Rough Planner, so this step carries everything you need.",
        )
        .actions([
            "Choose Library, then Plan Rough... in the menu bar.",
            "Under ROUGH choose Mesh (OBJ/STL/PLY) and pick the file of your scan, or open a saved plan whose rough is a mesh (Saved Plans in the title bar). The Rough Color... button stays dimmed until a mesh is loaded and no plan is running.",
            "Click Rough Color..., next to Locate Inclusion from Photos....",
        ])
        .check("A window opens with a strip of eight steps along the top, and Back and Next at the bottom.")
        .why("The steps are Checklist, Photos, Calibration, Masks, Surfaces, Fit, Compare and Accept. A step opens only when the earlier ones are done, and a step you cannot open yet says why in its hover note. Closing the window only hides it: the photos and results stay until you close the Rough Planner.")
        .goal(event(ROUGH_COLOR_WIZARD_OPENED), "The Rough Color wizard opened")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Photos and calibration",
            "Steps 1 to 3 say which rig was used, load the photos and tell the wizard how the camera and the backlight are known.",
        )
        .actions([
            "Checklist: pick the rig. The line under it says whether the mesh is aligned to this rig; Locate... opens the Locate window where you align it.",
            "Photos: every view has a row with Stone, White, Dark, 2nd Exposure and Clear. Press a button and choose a file. Pressing White or Dark again adds another frame, which is averaged with the first. Use the Photos of the Locate Window takes over stone photos that are already loaded there.",
            "Press Prepare Photos. The wizard divides the stone photo by the white frame, so the lamp, the vignetting and the camera gain cancel. It needs at least two views that have a stone photo and a white frame.",
            "Calibration: choose how the camera is known (Measured Curves, a Reference Filter Set, or sRGB Fallback, the least certain) and how the backlight is known (Find from the White Frame, a CIE LED kind, or a measured spectrum). Then press Apply Calibration.",
        ])
        .why("A camera calibration does not depend on the stone, so calibrate once per camera and lens and keep the files.")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Masks and surfaces",
            "Pixels that do not show the stone's color are left out of the fit, and the skin of the rough decides how light meets it.",
        )
        .actions([
            "Masks: the picture shows the excluded pixels in color with a legend (your brush, an inclusion, saturated, below the noise, the edge band, outside the stone). Edge Band (px) widens the band along the outline, which is safer when the alignment is poor.",
            "The brush has three modes: Brush Off, Exclude and Restore. Use Exclude on reflections, stains, dust or the holder, and Clear Brush to start over.",
            "Surfaces: choose Polished, Frosted, Fixed (type a roughness) or Frosted, Find It, which tries about five roughness values and takes about five times as long. Paint Window marks faces a lapidary ground to look in.",
        ])
        .why("A frosted or uneven skin is the biggest enemy of this method, because it scatters the light and hides the body color. Immersion in a liquid whose index is close to the stone's removes most of that.")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Zones and the fit",
            "Step 6 fits one color for the whole stone, or one color for each zone you describe.",
        )
        .actions([
            "A stone without zones has just the Base Zone. For a watermelon, a bicolor or an ametrine, add zones with the five shape buttons: Half Space, Slab, Cylinder, Prism and Sector, up to four besides the base. Select a zone to type its numbers, or drag its handles in the Rough Planner's 3D view.",
            "Zones from Marks fits a zone to boundary points you click in at least two photos, and Suggest from the Photos lists up to six candidate zones that you can Accept one by one.",
            "Press Start Fit. This is the long step, minutes for a frosted rough with all eight views. The progress line shows the stage and the time left, and Cancel stops it. The tracing is stored, so fitting again after a color-only change is fast.",
        ])
        .why("Refine Geometry moves the unlocked zone boundaries to where they fit the photos best; run Start Fit again afterwards.")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Read the result",
            "Step 7 shows, for each view, the photo, the render the fitted color predicts and the difference between them.",
        )
        .actions([
            "If you have no photos yet, open a saved plan that already has a color (Saved Plans in the title bar of the Rough Planner) and press Rough Color...: the wizard restores the stored working-resolution photos and the fit, so Compare works without the original files.",
            "Under the pictures, Color difference (CIEDE2000) gives the median, the 95 % level and the worst value. A value of 1 is barely visible, about 2 is visible side by side, and 5 or more is a clear difference.",
            "How well it fits holds the leave-one-view-out table: the wizard fits again without one view, predicts that view and compares it with the photo. A row more than 3 dE off is amber and often means a wrong mask, a badly aligned mesh or a zone nobody described.",
            "Predicted Colors shows a swatch for the base zone and each zone, at 7 mm and at your planned width, under D65 and under lamp A, as Lab values with a plus-or-minus number and a metamer spread.",
        ])
        .why("The metamer spread is larger when different spectra look the same to the camera and the backlight but different in other light. If it is much bigger than the plus-or-minus, the camera data cannot fix the color better than that.")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Keep the result",
            "Accept stores the result with the saved plan and closes the wizard.",
        )
        .actions([
            "Accept needs a saved plan, because the color is stored against the plan. If it is dimmed, save the plan in the Rough Planner first.",
            "Export Report... writes a Markdown report and the pictures into a folder you choose.",
            "The color travels with the plan: Export in the Saved Plans list writes it into the plan file, and Import... stores it with the new plan. The next lesson uses the color in the planner.",
        ])
        .why("Every control is unlocked again.")
        .allow(VIEWPORT),
    )
}

fn in_planner() -> Guide {
    Guide::new(
        "library-rough-color-in-planner",
        "Use the Rough Color in the Planner",
        "See stones in the color of the rough, choose the pose that shows the most of the inner zone, and take the color to the editor.",
        GuideCategory::RoughPlanning,
    )
    .starting(StartingState::CurrentDesign)
    .step(
        GuideStep::new(
            "A plan that has a color",
            "The rough's color is stored against a saved plan, so the planner shows it only for a plan you have saved and opened.",
        )
        .actions([
            "Choose Library, then Plan Rough..., and open Saved Plans in the title bar. Open a plan that has a rough color; the Rough Color wizard lesson shows how to make one.",
            "The stones in the result scenes and thumbnails are then drawn in their own color: the color of the rough where the stone's face-up path runs, weighted by how much of that path each zone takes.",
            "Results that were just planned and not saved are drawn with the palette colors.",
        ])
        .why("Saving a plan again keeps the color with it, and Export in the Saved Plans list writes it into the plan file.")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Choose Best Color",
            "With zones, the same layout can place a stone with different parts of the rough face up.",
        )
        .actions([
            "Above the results find Stone Orientation. It appears only for a saved plan that has a color.",
            "Click Best Color. Keep Planner Pose puts every stone back as the planner gave it.",
        ])
        .check("The Best Color pill is lit and the stone pictures are drawn again.")
        .why("Best Color turns each stone, among the few poses that are equivalent for the planner, so that as much as possible of the last zone in the list is under the table; for a watermelon that is the inner pink. The choice is saved with the plan. A rough with no zones, or an exact fit, keeps its planner pose.")
        .goal(event(ROUGH_COLOR_BEST_CHOSEN), "Best Color chosen")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Use the color of a design",
            "Use Color adopts the color for one design and opens that design in the editor.",
        )
        .actions([
            "On a design row of the results click Use Color. If the design that is open has unsaved changes you are asked to save, discard or cancel first, and Cancel opens nothing.",
        ])
        .check("The design opens in the editor with a material called after the plan and the design, ending in color, and its stone width set to the width of the stone in the plan.")
        .why("Undo takes back the material and the width as one step. Open in Editor on the same row does the same and also makes the planned piece of rough the design's preform.")
        .goal(event(ROUGH_COLOR_USED), "The color of a design adopted")
        .allow(FILES),
    )
    .step(
        GuideStep::new(
            "What the color is made of",
            "The material carries real millimetres, so the color scales with the stone and no slider is involved.",
        )
        .actions([
            "Open the Stone tab of the Parameters dock and click New/Edit Material.... Its Color Pattern section shows the zones as a strip of swatches, and Edit Pattern... opens them in the Color Pattern window.",
            "Zone Geometry Relative to Stone Size keeps the zones as fractions of the stone width, so a stone cut twice as wide gets zones twice as wide. Off, the zones keep their millimetre sizes.",
            "A design's Body Color and the Live Render Color control do not apply to a zoned material, because its color is its zones. Both are greyed out with a note.",
            "Zones show in the Path-Traced views. A zoned stone with scattering renders on the processor.",
        ])
        .why("Every control is unlocked again.")
        .allow(VIEWPORT),
    )
}

fn ametrine() -> Guide {
    Guide::new(
        "viewing-ametrine",
        "Look at Ametrine",
        "Pick the built-in Ametrine and see its amethyst and citrine halves in the Path-Traced view.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "What Ametrine is",
            "Ametrine is bicolor quartz: a purple amethyst half and a yellow-orange citrine half in one stone.",
        )
        .actions([
            "The built-in material has amethyst on one side of the plane through the stone's center that contains the table normal, and citrine on the other, with a sharp boundary.",
            "The boundary is fixed in the design's frame: it is the plane x = 0 of the model, with the citrine on the side of positive x. Seen face-up, the line runs along the design's Z axis through the center.",
            "Both halves are the same quartz with the same iron. The citrine half is the amethyst half after a heat treatment, so the stone darkens with size like every built-in material.",
        ])
        .why("The facets decide what the two halves do. To see the line elsewhere, choose or orient the design accordingly.")
        .highlight("stone_tab")
        .allow(MATERIALS),
    )
    .step(
        GuideStep::new(
            "Pick Ametrine",
            "The Stone tab's Material list sets the material of the open design at once.",
        )
        .actions([
            "Open the Stone tab of the Parameters dock (View > Stone Properties, or Ctrl+Alt+1).",
            "Set Material to Ametrine. It applies at once.",
        ])
        .check("the Stone tab shows Ametrine.")
        .why("The swatch in the material pickers shows the amethyst half only.")
        .goal(Goal::Material("Ametrine".to_owned()), "the material set to Ametrine")
        .highlight("stone_tab")
        .allow(MATERIALS),
    )
    .step(
        GuideStep::new(
            "Look at both halves in Path-Traced",
            "The zones show where light travels through the stone, so the Path-Traced view is where to look.",
        )
        .actions([
            "Click Path-Traced in the row of view pills above the picture, and give the picture a moment to settle.",
        ])
        .check("The picture shows a purple half and a yellow-orange half that meet along one straight line.")
        .why("From Solid, the click gives Solid combined with Path-Traced, which shows the zones too; click Solid to turn the flat layer off. The Solid view and Ray-Traced show a base color only.")
        .goal(path_traced_view(), "The Path-Traced view")
        .perform(Perform::ViewMode(1))
        .highlight("view_modes")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Why the color controls are grey",
            "The color of Ametrine is its two zones, so a single color has nothing to change.",
        )
        .actions([
            "A design's Body Color in the Stone tab and the Live Render Color control are greyed out with a note. The refractive-index override still applies.",
            "To change a zone, open New/Edit Material... in the Stone tab. The Color Pattern section shows both halves as swatches with the note Built-in pattern. Save the material under a new name to change it.",
            "Saving Ametrine under a new name makes a custom material that keeps both zones, and its pattern can then be edited in the Color Pattern window.",
        ])
        .why("The GPU renders Ametrine as long as the inclusions amount is zero. With inclusions the stone renders on the processor, like every zoned material. Every control is unlocked again.")
        .allow(VIEWPORT),
    )
}
