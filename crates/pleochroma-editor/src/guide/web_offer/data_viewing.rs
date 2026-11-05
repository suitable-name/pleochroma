//! Web overlays for the materials, viewport, rendering, output and app sections of the learning
//! path.
//!
//! Every `from` text below is a line of the catalogue's lesson, word for word; a test applies each
//! overlay to the catalogue and fails when one no longer matches. On the web the stone effects are
//! Render Settings, not edits to the design, and a file is a browser download, not a Save dialog.

use super::overlay::{GuideOverlay, StepOverlay, material_edits};
use crate::guide::{Goal, Group};

/// The overlays of this file.
pub(super) fn overlays() -> Vec<GuideOverlay> {
    vec![
        custom_material(),
        stone_effects(),
        ametrine(),
        solid_picking(),
        drag_handles(),
        slice(),
        diagram(),
        live_render(),
        output_save(),
        output_open(),
        export_asc(),
        export_gcs(),
        cutting_sheet(),
        diagram_png(),
        keyboard(),
    ]
}

/// The "look for" line of a finished download: the web reports `Downloaded` and the file name.
const DOWNLOADED: &str = "A message says Downloaded, followed by the file name.";

/// The action line of a finished Save dialog on the desktop.
const SAVE_DIALOG: &str = "In the Save dialog choose a folder and a name, then click Save.";

fn custom_material() -> GuideOverlay {
    GuideOverlay::new("viewing-custom-material")
        .summary("Define a material of your own in Design Settings: refractive index, dispersion, double refraction and color.")
        .step(
            StepOverlay::of("What a custom material holds")
                .line(
                    "The built-in list has 33 materials. A custom material is one you describe yourself, for a stone the list does not have.",
                    "A custom material is one you describe yourself, for a stone the built-in list does not have.",
                )
                .line(
                    "Double refraction (birefringence), the crystal system and the optical character.",
                    "Double refraction (birefringence).",
                )
                .line(
                    "A body color, and the specific gravity if you know it, which the Rough Planner uses to turn volume into carats.",
                    "A body color, and the specific gravity if you know it, which turns volume into carats.",
                )
                .drop_line("Load Preset Template starts you from a built-in material to tweak.")
                .line(
                    "A custom material is saved in your library and appears in every material list, after the built-ins.",
                    "A custom material is saved with the design file and appears in every material list, after the built-ins.",
                ),
        )
        .step(
            StepOverlay::of("Make a material")
                .line(
                    "The Material Editor opens over the window, so this step carries everything you need.",
                    "The custom material form sits in Design Settings, so this step carries everything you need.",
                )
                .line(
                    "Open the Stone tab of the Parameters dock (View > Stone Properties, or Ctrl+Alt+1), then click New/Edit Material....",
                    "Open Design Settings, then click Custom material.",
                )
                .drop_line("Optionally pick a Load Preset Template, for example Quartz.")
                .line(
                    "Type a Material Name that no built-in material uses, for example Tutorial glass.",
                    "Type a Name that no built-in material uses, for example Tutorial glass.",
                )
                .line(
                    "Move the Refractive Index (nd) and Dispersion sliders to values you like.",
                    "Type a Refractive Index and a Dispersion you like, for example 1.62 and 0.03.",
                )
                .line(
                    "Click Save. Save & Apply would also make it the design's material.",
                    "Click Save and Use.",
                )
                .line(
                    "A toast says Saved custom material with the name you typed.",
                    "A message says the material was saved and set as this design's material.",
                ),
        )
        .step(
            StepOverlay::of("Use it on the design")
                .line(
                    "Saving only adds the material to the lists. The design keeps its own material until you change it.",
                    "Save and Use adds the material to the lists and makes it the design's material.",
                )
                .line(
                    "In the Stone tab, open the Material drop-down: your material is listed after the built-ins.",
                    "In Design Settings, open the Material list: your material is listed after the built-ins.",
                )
                .line(
                    "Pick it. It applies at once, and you can undo it.",
                    "The design already uses it, and Undo takes that back.",
                )
                .drop_line("To change or remove it later, open the Material Editor, pick it and use Save or Delete."),
        )
}

fn stone_effects() -> GuideOverlay {
    stone_effects_rest(stone_effects_first(GuideOverlay::new(
        "viewing-stone-effects",
    )))
}

/// The first three steps of the stone-effects overlay: what the effects are, the Path-Traced
/// view and the haze.
fn stone_effects_first(overlay: GuideOverlay) -> GuideOverlay {
    overlay
        .step(
            StepOverlay::of("What the Stone tab can change")
                .retitle("What Render Settings can change")
                .highlight("lighting_tab")
                .line(
                    "The Stone tab holds four effects that change how the stone looks in a render. They belong to the design, not to the program.",
                    "Render Settings holds four effects that change how the stone looks in a render.",
                )
                .line(
                    "Stone Width sets how wide the stone is across the girdle in millimeters. A wider stone looks deeper in color.",
                    "Stone size sets how wide the stone is across the girdle in millimeters. A wider stone looks deeper in color.",
                )
                .line(
                    "Open the Stone tab with the gem button above the picture (or Ctrl+Alt+1): the effects are under Material Effects, and Stone Width is under Size.",
                    "Open the Render tab and click Settings: the effects are under MATERIAL, and Stone size sits right below them.",
                )
                .line(
                    "They are saved with the design, so it opens the way you left it. Each change is one step in the history and Undo takes it back.",
                    "They change how the stone is drawn, not the cutting instructions.",
                ),
        )
        .step(
            StepOverlay::of("Look at the stone in Path-Traced")
                .line(
                    "Click Path-Traced in the row of view pills above the picture, and give the picture a moment to settle.",
                    "Click the Render tab above the picture, choose Path-Traced under QUALITY in Render Settings, and give the picture a moment to settle.",
                )
                .line(
                    "The picture shows the stone lit with traced light, and the pills show Path-Traced as chosen, with Solid chosen as well or on its own.",
                    "The picture shows the stone lit with traced light, and Path-Traced is chosen under QUALITY.",
                )
                .line(
                    "The Solid view is flat-shaded and does not show haze, a frosted girdle or rounded edges. From Solid, the click gives Solid combined with Path-Traced, which shows them; click Solid to turn the flat layer off.",
                    "The Solid tab is flat-shaded and does not show haze, a frosted girdle or rounded edges.",
                ),
        )
        .step(
            StepOverlay::of("Add inclusion haze")
                .highlight("lighting_tab")
                .line(
                    "Open the Stone tab if it is not open, and find Inclusion Haze under Material Effects.",
                    "Open Render Settings if it is not open, and find Inclusion Haze under MATERIAL.",
                )
                .sub("; the change is written to the design when you let go", ""),
        )
}

/// The remaining steps of the stone-effects overlay: the girdle, the edges, the width and the
/// history.
fn stone_effects_rest(overlay: GuideOverlay) -> GuideOverlay {
    overlay
        .step(
            StepOverlay::of("Frost the girdle")
                .highlight("lighting_tab")
                .line(
                    "In the Stone tab, switch Frosted Girdle on.",
                    "In Render Settings, switch Frosted Girdle on.",
                ),
        )
        .step(
            StepOverlay::of("Round the edges")
                .highlight("lighting_tab")
                .line(
                    "In the Stone tab, drag the Edge Rounding slider a little to the right and let go.",
                    "In Render Settings, drag the Edge Rounding slider a little to the right and let go.",
                ),
        )
        .step(
            StepOverlay::of("Set the stone width")
                .highlight("preform_tab")
                .allow_add(&[Group::PreformTab])
                .line("Stone Width set", "the Girdle Diameter applied")
                .line(
                    "In the Stone tab, scroll to Size and click the Stone Width (mm) field.",
                    "In the Preform tab, find Girdle Diameter (mm) in the Yield group.",
                )
                .line(
                    "Type a width in millimeters, for example 8, and press Enter. You can type arithmetic too, for example 6.5+0.25.",
                    "Type a width in millimeters, for example 8, and click Apply Yield Inputs. You can type arithmetic too, for example 6.5+0.25.",
                )
                .line(
                    "The line under the field says Saved in the design, with your width.",
                    "Stone size in Render Settings now reads your width in mm.",
                )
                .sub(
                    " This is the same value as the Girdle Diameter in the Preform tab.",
                    " Stone size in Render Settings follows the Girdle Diameter while Linked to Design is on.",
                ),
        )
        .step(
            StepOverlay::of("One step each, saved with the design")
                .retitle("Render settings, not design edits")
                .line(
                    "Every effect you set is its own step in the history.",
                    "Haze, the frosted girdle and edge rounding are render settings; the stone size follows the design.",
                )
                .line(
                    "Click Undo to take the stone width back, and Undo again for the edge rounding, the frosted girdle and the haze. The Stone tab follows each one.",
                    "Set Inclusion Haze and Edge Rounding back to Off and switch Frosted Girdle off to return to the clean picture.",
                )
                .drop_line("Save the design: the effects and the width are in the file, and they come back when you open it.")
                .drop_line("Reset This Tab at the foot of the Stone tab asks you to confirm, then puts every effect back to off as one step. It never changes the material or the shape.")
                .line(
                    "The same effects apply to the Render and to the exports of this design. A design from the library shows its effects for looking only: open it in the Edit tab to keep them.",
                    "The same effects apply to the Render tab and to the PNG export while they are set.",
                ),
        )
}

fn ametrine() -> GuideOverlay {
    GuideOverlay::new("viewing-ametrine")
        .edits(material_edits("Ametrine"))
        .step(StepOverlay::of("Pick Ametrine").line(
            "The Stone tab's Material list sets the material of the open design at once.",
            "The Material list in Design Settings sets the material of the open design when you click Apply Material.",
        ))
        .step(
            StepOverlay::of("Look at both halves in Path-Traced")
                .line(
                    "Click Path-Traced in the row of view pills above the picture, and give the picture a moment to settle.",
                    "Click the Render tab above the picture, choose Path-Traced under QUALITY in Render Settings, and give the picture a moment to settle.",
                )
                .line(
                    "From Solid, the click gives Solid combined with Path-Traced, which shows the zones too; click Solid to turn the flat layer off. The Solid view and Ray-Traced show a base color only.",
                    "The Solid tab and Ray-Traced show a base color only.",
                ),
        )
        .step(
            StepOverlay::of("Why the color controls are grey")
                .line(
                    "A design's Body Color in the Stone tab and the Live Render Color control are greyed out with a note. The refractive-index override still applies.",
                    "A design's Body Color in Design Settings and in Render Settings does not apply to Ametrine, because its color is its two zones. The refractive-index override still applies.",
                )
                .drop_line("To change a zone, open New/Edit Material... in the Stone tab. The Color Pattern section shows both halves as swatches with the note Built-in pattern. Save the material under a new name to change it.")
                .drop_line("Saving Ametrine under a new name makes a custom material that keeps both zones, and its pattern can then be edited in the Color Pattern window.")
                .line(
                    "The GPU renders Ametrine as long as the inclusions amount is zero. With inclusions the stone renders on the processor, like every zoned material. Every control is unlocked again.",
                    "Every control is unlocked again.",
                ),
        )
}

fn solid_picking() -> GuideOverlay {
    GuideOverlay::new("viewing-solid-picking")
        .step(
            StepOverlay::of("The Solid view")
                .line(
                    "The Edit tab shows your design as a flat-shaded stone you can point at.",
                    "The Solid tab shows your design as a flat-shaded stone you can point at.",
                )
                .line(
                    "Click the Edit tab above the picture, if it is not open.",
                    "Click the Solid tab above the picture, if it is not open.",
                )
                .line(
                    "Choose Solid, Path-Traced, Ray-Traced or Diagram. Solid can stay selected together with either tracer.",
                    "The Render, Solid and Diagram tabs switch the view; Render Settings chooses Path-Traced or Ray-Traced.",
                ),
        )
        // The web has no Solid-with-tracer overlay (view mode 2), so this step picks in the
        // Diagram, the other view that shows the stone.
        .step(
            StepOverlay::of("Pick in another view")
                .goal(Goal::ViewMode(3), "The Diagram view")
                .line(
                    "Keep Solid selected and click Path-Traced above the stone.",
                    "Click the Diagram tab above the stone.",
                )
                .line(
                    "Solid and Path-Traced are both lit, and facet edges are drawn over the traced picture.",
                    "Three flat panels appear: Crown, Pavilion and Profile.",
                )
                .line(
                    "Selecting Solid with a tracer shows which facet boundary belongs to each highlight. Ray-Traced can be combined with Solid too.",
                    "Picking works in the Diagram too: a click on a facet selects its whole tier there as well.",
                ),
        )
        .step(
            StepOverlay::of("A Ray-Traced picture")
                .line(
                    "Click Ray-Traced above the stone.",
                    "Click the Render tab, then choose Ray-Traced under QUALITY in Render Settings.",
                )
                .sub(", alone or with Solid still chosen,", ","),
        )
}

fn drag_handles() -> GuideOverlay {
    GuideOverlay::new("viewing-drag-handles").step(StepOverlay::of("Select a tier").line(
        "Click the Edit tab above the picture, if it is not open.",
        "Click the Solid tab above the picture, if it is not open.",
    ))
}

fn slice() -> GuideOverlay {
    GuideOverlay::new("viewing-slice").step(StepOverlay::of("Switch Slice on").line(
        "Click the Edit tab above the picture, then Solid, if they are not already chosen.",
        "Click the Solid tab above the picture, if it is not already chosen.",
    ))
}

fn diagram() -> GuideOverlay {
    GuideOverlay::new("viewing-diagram")
        .summary(
            "Read the crown, pavilion and profile panels and pick a facet on the flat diagram.",
        )
        .step(
            StepOverlay::of("Switch to the Diagram")
                .drop_line("Click the Edit tab above the picture, if it is not open.")
                .line(
                    "Click Diagram above the stone.",
                    "Click the Diagram tab above the stone.",
                )
                .sub(": switching to it puts the Cut slider back to Finished", ""),
        )
        .step(
            StepOverlay::of("Pick a facet")
                .sub(", and its handles A, D and I appear on a panel", ""),
        )
        // The web hides the handles in the Diagram, so there is nothing to drag there.
        .omit("Drag a handle on the diagram")
        .step(
            StepOverlay::of("Back to all three")
                .drop_line("Press Ctrl+Z to put back the angle you changed."),
        )
}

fn live_render() -> GuideOverlay {
    GuideOverlay::new("viewing-live-render")
        .title("Render and Lighting")
        .step(open_render_step())
        .step(
            StepOverlay::of("Turn the stone and move the light")
                .line(
                    "Right-drag, or Shift and left-drag, moves the light.",
                    "Move the light with Light Azimuth and Light Elevation under Settings.",
                )
                .drop_line("Front and Top snap the camera to the standard poses; the reset button next to them puts the camera and the light back."),
        )
        .step(StepOverlay::of("Choose a lighting rig").line(
            "Open the Lighting drop-down in the toolbar.",
            "Open the Lighting list at the top of the window.",
        ))
        .step(
            StepOverlay::of("The Parameters dock")
                .retitle("Render Settings")
                .line(
                    "Everything else about the light is in the Lighting & Environment tab, beside the picture.",
                    "Everything else about the light is in Render Settings, opened by the Settings button.",
                )
                .line(
                    "The bulb button in the toolbar (or Ctrl+Alt+2) opens the tab; the step has opened it for you.",
                    "Click Settings above the picture to open the panel.",
                )
                .line(
                    "It holds the lighting rig, the light direction, the exposure, the HDR map and backdrop, the observer's surface glare and head shadow, your saved lighting and the camera.",
                    "It holds the lighting rig, the light direction, the exposure, the HDR map and backdrop, the surface glare and the head shadow, and the render quality.",
                )
                .drop_line("The gem button (Ctrl+Alt+1) opens the Stone tab, and the gear button (Ctrl+Alt+3) the Render & Output tab. The same three buttons are in the Edit view.")
                .drop_line("A saved lighting stores the whole look at once: the rig, the light direction, the exposure, the glare, the backdrop, the head shadow and the HDR map."),
        )
        .step(
            StepOverlay::of("The stone's material")
                .line(
                    "The toolbar shows the material of the picture as a small chip. The material itself belongs to the design and is changed in the Stone tab.",
                    "The material belongs to the design and is changed in Design Settings.",
                )
                .line(
                    "Open the Stone tab (the gem button, or Ctrl+Alt+1). Its Material drop-down changes the design's own material, and you can undo it.",
                    "Open Design Settings. Its Material list changes the design's own material, and you can undo it.",
                )
                .drop_line("Preview As... shows the same stone in another material without changing the design. While it is on, an amber Previewing badge says so in the toolbar and in the export dialogs, and Back to Design ends it.")
                .line(
                    "New/Edit Material... opens the Material Editor (see the custom materials tutorial).",
                    "Custom material, in the same panel, defines a material of your own (see the custom materials tutorial).",
                )
                .drop_line("A preview never starts by itself, so the picture, an export and the tilt video always use the design's material unless you can see the Previewing badge."),
        )
        .step(
            StepOverlay::of("Back to the Edit tab")
                .retitle("Back to the Solid tab")
                .line(
                    "The Edit tab is where you change the design.",
                    "The Solid tab is where you change the design with the handles.",
                )
                .line(
                    "Click Edit, the pill to the right of Live Render.",
                    "Click Solid, the tab next to Render.",
                ),
        )
}

/// The "open the Render tab" step that Live Render and the lighting lesson share.
pub(super) fn open_render_step() -> StepOverlay {
    StepOverlay::of("Open Live Render")
        .retitle("Open the Render tab")
        .line("The Live Render view", "The Render view")
        .line(
            "Live Render is the physically based picture of the stone: the light, the material and the dispersion as they really behave.",
            "The Render tab is the physically based picture of the stone: the light, the material and the dispersion as they really behave.",
        )
        .line(
            "Click Live Render, the pill to the left of Edit, above the picture.",
            "Click Render, the first tab above the picture (or press Alt+1).",
        )
        .line(
            "The Live Render view opens with its toolbar of Lighting, the three Parameters buttons and the metrics. It starts on the flat Solid picture; click Path-Traced above the picture for the traced one.",
            "The Render view opens and the stone is traced with light. The Settings button holds the lighting and quality controls.",
        )
        .line(
            "The Edit tab's Solid view is quick and flat-shaded, and Live Render starts the same way. Path-Traced is slower and shows color play, so it settles after a moment.",
            "The Solid tab is quick and flat-shaded. The Render tab is traced, which is slower and shows color play, so it settles after a moment.",
        )
}

fn output_save() -> GuideOverlay {
    GuideOverlay::new("output-save")
        .step(StepOverlay::of("Make a change").line(
            "The row reads 35.0, the window title starts with an asterisk and the status strip shows an amber UNSAVED badge.",
            "The row reads 35.0 and an amber UNSAVED chip shows in the dock.",
        ))
        .step(
            StepOverlay::of("Save the design")
                .line(
                    "Choose File > Save in the command bar (or press Ctrl+S).",
                    "Choose File > Save Design (.psdd) in the menu bar (or press Ctrl+S).",
                )
                .line(
                    "The first time, a Save As dialog offers a name; choose a folder and click Save.",
                    "Your browser downloads the file, usually into your Downloads folder.",
                )
                .line(
                    "A toast names the file, and the UNSAVED badge and the asterisk go away.",
                    "A message says Downloaded, followed by the file name, and the UNSAVED chip goes away.",
                )
                .sub(
                    " Save also adds or updates the design's entry in your library.",
                    "",
                ),
        )
        .step(
            StepOverlay::of("Save again, and Save As")
                .retitle("Save again")
                .line(
                    "After the first Save, the file belongs to the design.",
                    "Every Save writes the whole design again.",
                )
                .line(
                    "Save now writes straight to the same file, and keeps the previous version beside it as a .bak, one generation back.",
                    "Each Save downloads a new .psdd file; your browser may number it, for example design (1).psdd.",
                )
                .line(
                    "Save As (File menu, or Ctrl+Shift+S) always asks for a file, and the design belongs to the new file from then on.",
                    "Save .asc (File menu, or Ctrl+Shift+S) writes only the cutting instructions, as an .asc file.",
                ),
        )
        .step(
            StepOverlay::of("If the program closes first")
                .retitle("If the page closes first")
                .line(
                    "While there are unsaved changes the program writes an autosave every two minutes, to its own folder, never over your design.",
                    "While there are unsaved changes the page keeps an autosave in this browser, never over a file of yours.",
                )
                .drop_line("After an unclean exit, the next start asks Recover Unsaved Work? with Recover, Not Now and Delete.")
                .drop_line("A recovered design is never written back by Save: the first Save asks for a file name."),
        )
}

fn output_open() -> GuideOverlay {
    GuideOverlay::new("output-open")
        .step(
            StepOverlay::of("Save the design first")
                .line(
                    "Choose File > Save in the command bar.",
                    "Choose File > Save Design (.psdd) in the menu bar.",
                )
                .line(
                    "Choose a folder, keep the name offered and click Save.",
                    "Your browser downloads the file; note where it goes.",
                )
                .line(
                    "A toast names the file you saved.",
                    "A message says Downloaded, followed by the file name.",
                ),
        )
        .step(
            StepOverlay::of("Open it")
                .line(
                    "Choose File > Open in the command bar (or press Ctrl+O).",
                    "Choose File > Open... in the menu bar (or press Ctrl+O).",
                )
                .line(
                    "The window title names the file, and the design is as you saved it.",
                    "The design is as you saved it.",
                ),
        )
        .step(
            StepOverlay::of("More ways to open")
                .drop_line("File, Open Recent lists the designs you opened or saved lately.")
                .sub(
                    "and Save writes a new .psdd beside it, never over your file",
                    "and Save Design (.psdd) downloads a new .psdd file",
                ),
        )
}

fn export_asc() -> GuideOverlay {
    GuideOverlay::new("output-export-asc")
        .step(
            StepOverlay::of("The .asc file")
                .line(
                    "Export .asc in the command bar's File menu writes the design as you edited it in this tab, wherever you choose.",
                    "Save .asc in the File menu writes the design as you edited it, as an .asc download.",
                )
                .line(
                    "It does not add anything to your library. Save writes the whole .psdd design file instead.",
                    "Save Design (.psdd) writes the whole design file instead.",
                ),
        )
        .step(
            StepOverlay::of("Export it")
                .line(
                    "Choose File > Export .asc in the command bar (or File, Export Edited .asc in the menu at the top of the window).",
                    "Choose File > Save .asc (or press Ctrl+Shift+S).",
                )
                .line(SAVE_DIALOG, "Your browser downloads the .asc file.")
                .line(
                    "A toast says Exported edited schedule to the path you chose.",
                    DOWNLOADED,
                )
                .line(
                    "If the design does not solve to a closed solid you are asked first; Export Anyway stamps a warning into the file's header, and Cancel writes nothing.",
                    "If the design does not solve to a closed solid the file is still written, with a warning stamped into its header, and the message says so.",
                ),
        )
}

fn export_gcs() -> GuideOverlay {
    GuideOverlay::new("output-export-gcs")
        .step(
            StepOverlay::of("Gem Cut Studio files").sub("as Export .asc", "as Save .asc"),
        )
        .step(
            StepOverlay::of("Export it")
                .line(
                    "The export sits in the command bar's File menu and in the File menu at the top of the window.",
                    "The export sits in the File menu at the top of the window.",
                )
                .line(
                    "Click File in the command bar, then Export as Gem Cut Studio (.gcs)...",
                    "Choose File > Export .gcs (Experimental).",
                )
                .line(SAVE_DIALOG, "Your browser downloads the .gcs file.")
                .line(
                    "A toast names the file and reminds you to check it in Gem Cut Studio before cutting from it.",
                    "A message says Downloaded, followed by the file name. Check the file in Gem Cut Studio before cutting from it.",
                ),
        )
}

fn cutting_sheet() -> GuideOverlay {
    GuideOverlay::new("output-cutting-sheet")
        .step(StepOverlay::of("A sheet to print").highlight("schedule_cutting_sheet"))
        .step(
            StepOverlay::of("Export it")
                .highlight("schedule_cutting_sheet")
                .line(
                    "The sheet is in the File menu of the command bar.",
                    "The sheet is in the Schedule tab and in the File menu.",
                )
                .line(
                    "Click File in the command bar, then Export Cutting Sheet (HTML)...",
                    "Open the Schedule tab in the inspector, then click Cutting Sheet (HTML) (or choose File > Download Cutting Sheet (HTML)).",
                )
                .line(SAVE_DIALOG, "Your browser downloads the sheet.")
                .line(
                    "A toast says Wrote cutting sheet to the path you chose.",
                    DOWNLOADED,
                ),
        )
        .step(StepOverlay::of("Open and print").line(
            "For the standard template alone, as a PDF ready to print, choose Export Cutting Sheet (PDF)... in the same File menu.",
            "For the standard template alone, as a PDF ready to print, click Cutting Sheet (PDF) in the Schedule tab (or choose File > Download Cutting Sheet (PDF)).",
        ))
}

fn diagram_png() -> GuideOverlay {
    GuideOverlay::new("output-diagram-png")
        .step(StepOverlay::of("The diagram as a picture").highlight("schedule_diagram"))
        .step(
            StepOverlay::of("Export it")
                .highlight("schedule_diagram")
                .line(
                    "The diagram is in the File menu of the command bar.",
                    "The diagram is in the Schedule tab and in the File menu.",
                )
                .line(
                    "Click File in the command bar, then Export Diagram (PNG)...",
                    "Open the Schedule tab in the inspector, then click Diagram (PNG) (or choose File > Download Diagram PNG).",
                )
                .line(SAVE_DIALOG, "Your browser downloads the picture.")
                .line(
                    "A toast says Wrote diagram to the path you chose.",
                    DOWNLOADED,
                ),
        )
}

fn keyboard() -> GuideOverlay {
    GuideOverlay::new("app-keyboard-shortcuts")
        .step(
            StepOverlay::of("Keys you will use")
                .line(
                    "You never need to learn these, because the command palette finds everything. These are the ones people use all day.",
                    "You never need to learn these, because every action is also in the menus. These are the ones people use all day.",
                )
                .line(
                    "File: Ctrl+N new, Ctrl+O open, Ctrl+S save, Ctrl+Shift+S save as.",
                    "File: Alt+N new, Ctrl+O open, Ctrl+S save the design, Ctrl+Shift+S save the .asc.",
                )
                .line(
                    "Tabs: Ctrl+1, Ctrl+2 and Ctrl+3 switch the three top tabs, and Ctrl+E toggles Live Render and Edit.",
                    "Tabs: Alt+1, Alt+2 and Alt+3 switch to the Render, Solid and Diagram tabs.",
                )
                .line(
                    "Solid viewport: 1, 2, 3, 4 and 5 switch the view mode, and S toggles Slice.",
                    "Solid tab: S toggles Slice, and Enter keeps and F flips a provisional slice.",
                )
                .line(
                    "Everywhere: Ctrl+K opens the palette, Ctrl+, Preferences, F1 help for the screen, Esc backs out, and ? shows every shortcut.",
                    "Everywhere: Esc backs out, and ? shows every shortcut.",
                ),
        )
        .step(
            StepOverlay::of("Switch the view with a key")
                .line(
                    "The number keys choose the Solid viewport's view mode.",
                    "Alt and a number key choose the view tab.",
                )
                .line(
                    "Click the Edit tab above the picture, then click an empty part of the window so that no text box has the keyboard.",
                    "Click an empty part of the window so that no text box has the keyboard.",
                )
                .line("Press 4 for the Diagram view.", "Press Alt+3 for the Diagram view."),
        )
        .step(
            StepOverlay::of("And back")
                .line(
                    "The keys 1 to 5 select Solid, Path-Traced, Solid with Path-Traced, Diagram and Ray-Traced.",
                    "Alt+1, Alt+2 and Alt+3 select Render, Solid and Diagram.",
                )
                .line("Press 1 for the Solid view.", "Press Alt+2 for the Solid view."),
        )
        .step(
            StepOverlay::of("Swap tabs")
                .retitle("Open the Render tab")
                .line("The Live Render view", "The Render view")
                .line(
                    "Ctrl+E flips between Live Render and Edit.",
                    "Alt+1 opens the Render tab and Alt+2 returns to the Solid tab.",
                )
                .line("Press Ctrl+E.", "Press Alt+1.")
                .line(
                    "Press Ctrl+E again to come back to the Edit tab.",
                    "Press Alt+2 to come back to the Solid tab.",
                )
                .line(
                    "The Live Render view opens, and the second press brings back the Edit tab.",
                    "The Render view opens, and the second press brings back the Solid tab.",
                ),
        )
        .step(StepOverlay::of("Done").line(
            "The palette's rows show the key of each command, so it teaches you the shortcuts as you use it.",
            "The menus show the key of each command, so they teach you the shortcuts as you use them.",
        ))
}
