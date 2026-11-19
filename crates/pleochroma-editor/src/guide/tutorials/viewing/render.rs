//! Live Render and its lighting rigs, the Parameters dock's tabs, a design's saved lighting, and
//! custom materials: the two sliders, typed coefficients and a Color Pattern.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{VIEWPORT, VIEWPORT_TABS, event, path_traced_view};
use crate::guide::{
    Goal, Group, Guide, GuideCategory, GuideStep, Perform, StartingState,
    tutorials::viewing_events::{
        COEFFICIENT_MATERIAL_SAVED, CUSTOM_MATERIAL_SAVED, DESIGN_LIGHTING_FORGOTTEN,
        DESIGN_LIGHTING_SAVED, LIGHTING_PRESET_CHOSEN, LIVE_RENDER_OPENED, PATTERN_EDITOR_OPENED,
        PATTERN_SAVED, PATTERN_TEMPLATE_PICKED,
    },
};

/// The Stone tab's material controls (its "New/Edit material..." button; the `DesignSettings`
/// group gates them) and Undo.
const MATERIALS: &[Group] = &[Group::DesignSettings, Group::History];

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![
        live_render(),
        design_lighting(),
        custom_material(),
        material_coefficients(),
        color_pattern(),
    ]
}

/// The "open Live Render" step the first two lessons share.
fn open_live_render() -> GuideStep {
    GuideStep::new(
        "Open Live Render",
        "Live Render is the physically based picture of the stone: the light, the material and the dispersion as they really behave.",
    )
    .actions([
        "Click the Live Render tab in the design header (Live Render, Edit, Cutting Instructions, Files & Downloads).",
    ])
    .check("The Live Render view opens with the mode segment (Solid, Path, Ray) at the top left, the Lighting chip and the view tools at the top right, the FACE-UP figures card on the left and the status pill at the bottom right. It starts on the flat Solid picture; click Path (Path-Traced) for the traced one.")
    .why("The Edit tab's Solid view is quick and flat-shaded, and Live Render starts the same way. Path-Traced is slower and shows color play, so it settles after a moment.")
    .goal(event(LIVE_RENDER_OPENED), "The Live Render view")
    .highlight("live_render_tab")
    .allow(VIEWPORT_TABS)
}

fn live_render() -> Guide {
    Guide::new(
        "viewing-live-render",
        "Live Render and Lighting",
        "Look at the stone as light really treats it, and change the lighting rig.",
        GuideCategory::Rendering,
    )
    .starting(StartingState::Template(5))
    .step(open_live_render())
    .step(
        GuideStep::new(
            "Turn the stone and move the light",
            "The picture answers to the mouse as the Solid view does.",
        )
        .actions([
            "Drag to turn the stone, and scroll to zoom.",
            "Right-drag, or Shift and left-drag, moves the light; a middle-drag pans.",
            "Click the mouse button at the bottom left to see all the gestures (Gestures); it opens by itself the first time and closes when you first drag in the view.",
            "Front, Top and Turn 90\u{b0} snap the camera to the standard poses; Reset Camera next to them puts the camera and the light back. Below about 1000 px these tools fold into a View menu.",
        ])
        .why("Turning the stone here turns it in the Solid view too, so the two always show the same pose.")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Choose a lighting rig",
            "The Lighting chip changes the light the stone sits in.",
        )
        .actions([
            "Click the Lighting chip in the toolbar (a rig picture and the short rig name). The rig menu lists SAVED lightings, then LIT MODELS, STUDIO and ANALYSIS rigs.",
            "Choose a different rig, for example Gem Studio Ring Lights or Daylight sky + direct sun.",
        ])
        .check("A toast names the rig, and the picture starts again under the new light.")
        .why("The studio rigs give hard sparkle on a dark backdrop. The lit models (Grading tray, Light tent + black cards, Daylight sky + direct sun) show what the stone looks like in a real scene.")
        .goal(event(LIGHTING_PRESET_CHOSEN), "A lighting rig chosen")
        .highlight("lighting_combo")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "The Parameters dock",
            "Everything else about the light is in the Lighting tab, beside the picture.",
        )
        .actions([
            "The bulb button in the toolbar (or Ctrl+Alt+2) opens the tab, and All Rigs and Settings... at the foot of the rig menu does too; the step has opened it for you.",
            "From the top it holds the rig card (with Change, which opens the gallery of rigs), MAIN LIGHT (the dome, the azimuth and elevation fields, Exposure), SURROUNDINGS (the backdrop As Lit, Black, Grey, White, Stone Shadow, Tray Reflection and the HDR map), the Viewer card (Surface Glare and Head Shadow) and SAVED LIGHTINGS.",
            "The gem button (Ctrl+Alt+1) opens the Stone tab, and the gear button (Ctrl+Alt+3) the Render tab. The same three buttons are in the Edit view.",
        ])
        .why("A saved lighting stores the whole look at once: the rig, the main light direction, the exposure, the glare, the backdrop, Stone Shadow, the head shadow and the HDR map. The camera pose is stored only when you switch on Include the camera pose.")
        .highlight("lighting_tab")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "The stone's material",
            "The viewport shows the material of the picture as a small chip. The material itself belongs to the design and is changed in the Stone tab.",
        )
        .actions([
            "Open the Stone tab (the gem button, or Ctrl+Alt+1). Its Material drop-down changes the design's own material, and you can undo it.",
            "Preview As... shows the same stone in another material without changing the design. While it is on, an amber Previewing badge says so under the mode segment and in the export dialogs, and Back to Design ends it.",
            "The Material Menu (the \u{2026} button on the material card) holds RI Override and New/Edit Material..., which opens the Material Editor (see the custom materials tutorial).",
        ])
        .why("A preview never starts by itself, so the picture, an export and the tilt video always use the design's material unless you can see the Previewing badge.")
        .highlight("stone_tab")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Back to the Edit tab",
            "The Edit tab is where you change the design.",
        )
        .actions(["Click the Edit tab in the design header."])
        .why("Every control is unlocked again."),
    )
}

fn design_lighting() -> Guide {
    Guide::new(
        "viewing-design-lighting",
        "Lighting for This Design",
        "Make a design remember its own lighting, and forget it again.",
        GuideCategory::Rendering,
    )
    .starting(StartingState::Template(5))
    .step(open_live_render())
    .step(
        GuideStep::new(
            "Choose the lighting",
            "A design can remember the lighting you see on it, so it always opens under the light you chose.",
        )
        .actions([
            "Click the Lighting chip in the toolbar.",
            "Choose the rig you want this design to keep.",
        ])
        .check("A toast names the rig.")
        .why("The main light, exposure, surface glare, head shadow, the backdrop, Stone Shadow and an environment map belong to the lighting too; the Lighting tab changes them.")
        .goal(event(LIGHTING_PRESET_CHOSEN), "A lighting rig chosen")
        .highlight("lighting_combo")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Save it for this design",
            "Saving is one button in the Lighting tab, beside the picture. The rig menu of the Lighting chip has the same button.",
        )
        .actions([
            "Open the Lighting tab (the bulb button in the toolbar, or Ctrl+Alt+2).",
            "Scroll to SAVED LIGHTINGS and look for the group called Lighting This Design Opens With under the chips.",
            "Click Use This Lighting for This Design.",
        ])
        .check("A toast says Saved. This design will open with this lighting.")
        .why("The lighting is kept in your library on this computer, never in the design file. Saving again replaces it.")
        .goal(event(DESIGN_LIGHTING_SAVED), "Lighting saved for this design")
        .highlight("lighting_tab")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Forget it again",
            "The same group holds the way back.",
        )
        .actions([
            "Keep the Lighting tab open.",
            "Under Lighting This Design Opens With, click Forget for This Design (it appears once lighting is saved). The design's own saved lighting wears a star in the rig menu and among the saved chips.",
        ])
        .check("A toast says This design now uses your normal lighting.")
        .goal(event(DESIGN_LIGHTING_FORGOTTEN), "Lighting forgotten for this design")
        .highlight("lighting_tab")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Where a design's lighting applies",
            "A remembered lighting comes back when you open the design in the Edit tab.",
        )
        .actions([
            "Opening a design that has no saved lighting brings your normal lighting back.",
            "A design you only browse in the catalog is previewed under your normal lighting, not its saved one.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn custom_material() -> Guide {
    Guide::new(
        "viewing-custom-material",
        "Custom Materials",
        "Define a material of your own with the Material Editor: refractive index, dispersion, double refraction and color.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "What a custom material holds",
            "The built-in list has 40 materials. A custom material is one you describe yourself, for a stone the list does not have.",
        )
        .actions([
            "Refractive index (nd) and dispersion (the fire, as the spread between the blue and red lines).",
            "Double refraction (birefringence), the crystal system and the optical character.",
            "A body color, and the specific gravity if you know it, which the Rough Planner uses to turn volume into carats.",
            "Load Preset Template starts you from a built-in material to tweak.",
        ])
        .why("A custom material is saved in your library and appears in every material list, after the built-ins.")
        .highlight("stone_tab")
        .allow(MATERIALS),
    )
    .step(
        GuideStep::new(
            "Make a material",
            "The Material Editor opens over the window, so this step carries everything you need.",
        )
        .actions([
            "Open the Stone tab of the Parameters dock (View > Stone Properties, or Ctrl+Alt+1), open the Material Menu (the \u{2026} button on the material card) and click New/Edit Material... to open the Material Editor.",
            "Optionally pick a Load Preset Template, for example Quartz.",
            "Type a Material Name that no built-in material uses, for example Tutorial glass.",
            "Move the Refractive Index (nd) and Dispersion sliders to values you like.",
            "Click Save. Save & Apply would also make it the design's material.",
        ])
        .check("A toast says Saved custom material with the name you typed.")
        .why("The name must not be one of the built-in names, because that would hide the built-in material everywhere. Save is refused with a message if it is.")
        .goal(event(CUSTOM_MATERIAL_SAVED), "A custom material saved")
        .highlight("stone_tab")
        .allow(MATERIALS),
    )
    .step(
        GuideStep::new(
            "Use it on the design",
            "Saving only adds the material to the lists. The design keeps its own material until you change it.",
        )
        .actions([
            "In the Stone tab, open the Material drop-down: your material is listed after the built-ins.",
            "Pick it. It applies at once, and you can undo it.",
            "To change or remove it later, open the Material Editor, pick it and use Save or Delete.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn material_coefficients() -> Guide {
    Guide::new(
        "viewing-material-coefficients",
        "Materials from Coefficients",
        "Type a published Sellmeier or Cauchy fit as a material's refractive index curve.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Sliders or coefficients",
            "Two sliders describe a curve with two numbers: the index at the yellow sodium line and the spread between blue and red. A published fit describes it exactly.",
        )
        .actions([
            "Sellmeier, 3 terms: n squared is 1 plus three terms; fields B1, B2, B3, C1, C2, C3. Glass catalogs print this form.",
            "Sellmeier, 1 term: the same with one term; fields B1 and C1.",
            "Cauchy: n = A + B/λ² + C/λ^4; fields A, B and C.",
            "The wavelength λ is always in micrometers, so 589 nm is 0.589.",
        ])
        .why("The stone is then traced with exactly your curve, on the graphics card and on the processor alike.")
        .highlight("stone_tab")
        .allow(MATERIALS),
    )
    .step(
        GuideStep::new(
            "Type a curve",
            "The Material Editor opens over the window, so this step carries everything you need.",
        )
        .actions([
            "Open the Stone tab of the Parameters dock (View > Stone Properties, or Ctrl+Alt+1), open the Material Menu (the \u{2026} button on the material card) and click New/Edit Material... to open the Material Editor.",
            "Switch Refractive Index Curve from Simple to Coefficients.",
            "Choose the Sellmeier, 3 terms model, then pick Glass (N-BK7) in Copy From to fill the six fields.",
            "Change one number a little and watch the readout under the fields: nd, nF, nC, the spread and the Abbe number.",
            "Type a Material Name that no built-in material uses, for example Tutorial coefficients, and click Save.",
        ])
        .check("A toast says Saved custom material with the name you typed.")
        .why("Save stays disabled while a field is not a number or the curve cannot be traced; the red line under the fields says why.")
        .goal(event(COEFFICIENT_MATERIAL_SAVED), "A coefficient material saved")
        .highlight("stone_tab")
        .allow(MATERIALS),
    )
    .step(
        GuideStep::new(
            "Errors and warnings",
            "The editor checks the whole curve, not only each field.",
        )
        .actions([
            "A red message means the curve cannot be traced: a resonance (a Sellmeier C) inside 300 to 800 nm, an index at or below 1, or a calculation that divides by zero.",
            "A C is the resonance wavelength squared in µm², so a resonance at 100 nm is C = 0.01. A resonance in the visible range is nearly always a unit slip.",
            "An amber message is a warning and Save still works: an index that rises with wavelength, an Abbe number outside 5 to 120, or no fire at all.",
        ])
        .why("Switching a material back to Simple and saving replaces the curve with a fit of nd and the spread. Every control is unlocked again."),
    )
}

fn color_pattern() -> Guide {
    Guide::new(
        "viewing-color-pattern",
        "Make a Watermelon Tourmaline",
        "Give a custom material a green rim and a pink core with a Color Pattern, and look at it in the Path-Traced view.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "What a Color Pattern is",
            "A Color Pattern gives one material zones of different colors, the way a watermelon tourmaline has a green rim around a pink core.",
        )
        .actions([
            "A pattern belongs to a saved custom material that has a body color. This lesson makes one, adds the Watermelon template to it and looks at the stone.",
            "Templates start you off: Uniform, Bicolor, Tricolor, Banded, Watermelon, Reverse Watermelon, Core and Rim, Split and Sectors. Every zone can be changed afterward.",
            "The zones show in the Path-Traced views, where light travels through the stone. The Solid view and Ray-Traced show the base color.",
        ])
        .why("A pattern is kept with its material in your library on this computer, so every design that uses the material shows it.")
        .highlight("stone_tab")
        .allow(MATERIALS),
    )
    .step(
        GuideStep::new(
            "Make a material with a color",
            "The Material Editor opens over the window, so this step carries everything you need.",
        )
        .actions([
            "Open the Stone tab of the Parameters dock (View > Stone Properties, or Ctrl+Alt+1), open the Material Menu (the \u{2026} button on the material card) and click New/Edit Material... to open the Material Editor.",
            "Pick a Load Preset Template, for example Quartz, and type a Material Name that no built-in material uses, for example Tutorial tourmaline.",
            "Under Color, click Pick..., set a color with Tone, Saturation and Hue (a Saturation of about 40 gives a visible color), wait for Matching... to end and click Apply Color.",
            "Click Save & Apply. The material becomes the design's material.",
        ])
        .check("A toast says Saved custom material with the name you typed.")
        .why("A pattern starts from the material's own color, and only a color made with Pick... gives it one. A swatch alone does not: the Color Pattern section then says Pick a color for this material first.")
        .goal(event(CUSTOM_MATERIAL_SAVED), "A custom material saved")
        .highlight("stone_tab")
        .allow(MATERIALS),
    )
    .step(
        GuideStep::new(
            "Add the Watermelon pattern",
            "The Color Pattern window opens over the editor too, so this step carries everything you need.",
        )
        .actions([
            "In the Stone tab open the Material Menu and click New/Edit Material... again. The editor shows your material and its Color Pattern section.",
            "Click Add Pattern... and the Color Pattern window opens.",
            "Click Start From... in the header. In the gallery under Templates click Watermelon, and wait for Solving colors... to end.",
            "Above the stage click Slice in the Stone | Slice | Crystal switch: a slice of the stone shows the green Rim around the pink Core.",
            "In the Layers panel click the Core card, then move its Size slider to make the core larger or smaller. In the Advanced interface the Shape and its numbers take the place of the Size slider.",
            "Click Save to Material. The window closes.",
        ])
        .check("A toast says Saved the color pattern of your material, and the Color Pattern section shows a strip of two colors.")
        .why("Save to Material stores the pattern with the material and closes the window, and Cancel closes it and keeps the pattern as it was. Picking a tile in Start From... replaces the layers, and Undo brings them back; Undo and Redo in the window also take back single edits.")
        .goal(
            Goal::All(vec![
                event(PATTERN_EDITOR_OPENED),
                event(PATTERN_TEMPLATE_PICKED),
                event(PATTERN_SAVED),
            ]),
            "The Watermelon pattern saved",
        )
        .highlight("pattern_add_button")
        .allow(MATERIALS),
    )
    .step(
        GuideStep::new(
            "Look at it in Path-Traced",
            "The zones show where light travels through the stone, so the Path-Traced view is where to look.",
        )
        .actions([
            "Close the Material Editor if it is still open.",
            "Click Path-Traced in the mode segment at the top left of the picture (it reads Path in a narrow window), and give the picture a moment to settle.",
        ])
        .check("The picture shows a green rim around a pink core.")
        .why("From Solid, the click gives Solid combined with Path-Traced, which shows the zones too; click Solid to turn the flat layer off. Ray-Traced shows only the base color.")
        .goal(path_traced_view(), "The Path-Traced view")
        .perform(Perform::ViewMode(1))
        .highlight("view_modes")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Change or remove the pattern",
            "A pattern can be edited again at any time.",
        )
        .actions([
            "Open New/Edit Material... from the Stone tab's Material Menu and click Edit Pattern... to change the zones. Remove Pattern takes the zones away and keeps the base color.",
            "Start From... in the Color Pattern window opens the gallery. Its Save Current... tile keeps a pattern under a name, and the tile then sits under My Presets and loads it again for another material. The x on a preset tile deletes it.",
            "Add in the Layers panel puts up to four layers on top of the base layer. Upper layers cover lower ones, and the top layer is listed first.",
            "A stone of a species known for its zoning, such as a sapphire or a tourmaline, also offers Typical Zoning in one click: open the Color pill above the picture (or look in the Stone tab, below the Color Pattern card) and pick a pattern. It uses the stone's own color, or the material's color variants where it has them.",
            "A zone can also take its color from a color variant of the material: pick one in the From variant list of the layer. The Zone Colors From Variants lesson shows it.",
        ])
        .why("Every control is unlocked again."),
    )
}
