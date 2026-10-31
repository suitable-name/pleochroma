//! Web overlays for the first four sections of the learning path: getting started, adding and
//! editing tiers, arranging tiers, and meets and concave tiers.
//!
//! Lessons that run on the web as written (the `A` class of the lesson audit) need no entry. Every
//! `from` text below is a line of the catalogue's lesson, word for word; a test applies each overlay
//! to the catalogue and fails when one no longer matches.

use super::overlay::{GuideOverlay, StepOverlay, TextEdit};

/// The overlays of this file.
pub(super) fn overlays() -> Vec<GuideOverlay> {
    vec![
        welcome_tour(),
        walkthrough(),
        quick_add(),
        facet_chips(),
        tier_notes(),
        duplicate(),
        delete(),
        multi_select(),
        step_series(),
        adopt_imported_meets(),
        arithmetic(),
        cheater_offset(),
        concave_cylinder_cone(),
        concave_sphere_disc(),
    ]
}

fn welcome_tour() -> GuideOverlay {
    GuideOverlay::new("welcome-tour")
        .summary("A two-minute look at the editor, the viewport and the help.")
        .step(StepOverlay::of("Welcome to Pleochroma Cut").line(
            "You can run this tour again from Edit > Preferences..., under Tutorials.",
            "You can run this tour again from the tutorial list.",
        ))
        .omit("The library")
        .step(StepOverlay::of("The command bar").line(
            "The File menu at the right holds New Design..., Open, Load Selected, Save and the exports.",
            "The File menu at the top of the window holds New Design..., Open, Save and the exports.",
        ))
        .step(StepOverlay::of("The inspector").line(
            "The Advanced interface adds a Schedule tab with the cutting sheet.",
            "The Schedule tab holds the cutting sheet.",
        ))
        .step(
            StepOverlay::of("The viewport")
                .line(
                    "The buttons choose Solid, Path-Traced, Ray-Traced or Diagram. Keep Solid selected with either tracer to see facet edges over the picture.",
                    "The Render, Solid and Diagram tabs above the picture switch the view. In the Render tab, Render Settings chooses Path-Traced or Ray-Traced.",
                )
                .line(
                    "Press 1 to 5 to switch the view from the keyboard: Solid, Path-Traced, Solid with Path-Traced, Diagram and Ray-Traced.",
                    "Press Alt+1, Alt+2 or Alt+3 to switch to the Render, Solid or Diagram tab.",
                ),
        )
        .omit("Simple and Advanced")
        .step(
            StepOverlay::of("Help and the command palette")
                .retitle("Help and shortcuts")
                .line(
                    "Help > Tutorials... lists short guided lessons, one for each tool.",
                    "Help > Guided Walkthrough builds a first stone step by step.",
                )
                .line(
                    "Help > User Manual opens the full manual inside the app.",
                    "Help > User Guide opens a short guide inside the app.",
                )
                .line(
                    "Press Ctrl+K to search every command by name and run it.",
                    "Press ? to list every keyboard shortcut.",
                ),
        )
}

/// The walkthrough the web already runs: the nine lines of the old `WEB_WORDING` table, plus the
/// sentences about the Simple interface, which the web does not have.
fn walkthrough() -> GuideOverlay {
    GuideOverlay::new("new-design-walkthrough")
        .step(StepOverlay::of("Start a new design").line(
            "Choose File > New Design... in the command bar (or in the File menu at the top of the window).",
            "Click New Design... on the start page (or File > New Design..., or Alt+N).",
        ))
        .step(StepOverlay::of("Solve and check").line(
            "Read the status strip at the bottom of the Edit tab.",
            "Read the status strip at the bottom of the window.",
        ))
        .step(
            StepOverlay::of("Pick a real material")
                .line(
                    "Open the Stone tab of the Parameters dock (View > Stone Properties, or Ctrl+Alt+1).",
                    "In Design Settings, set Material: Diamond.",
                )
                .line(
                    "Set Material: Diamond. It applies at once.",
                    "Click Apply Material.",
                ),
        )
        .step(StepOverlay::of("Yield and rendering (optional)").line(
            "The carat estimate uses the specific gravity of the material set in the Stone tab unless you type an override. The Girdle Diameter is the same value as the Stone tab's Stone Width; it only changes how deep a colored stone's color looks, so a colorless Diamond looks the same. Click Skip Step if you do not need real units.",
            "The carat estimate uses the specific gravity of the material set in Design Settings unless you type an override; neither changes the render. Click Skip Step if you do not need real units.",
        ))
        .step(
            StepOverlay::of("You have a working design")
                .line(
                    "Switch to Live Render to see the stone rendered in Diamond.",
                    "Switch to the Render tab to see the stone rendered in Diamond. Render Settings offers Path-Traced or the faster, deterministic Ray-Traced view.",
                )
                .line(
                    "Save it with Save (Ctrl+S), or keep editing.",
                    "Save it with File > Save design (Ctrl+S), or keep editing.",
                )
                .line(
                    "Continue with Chapter 8 of the manual: Deep Solve, Optimize, Adopt and Apply.",
                    "Try Design > Optimize angles and Retarget for material, or press ? for the keyboard shortcuts.",
                )
                .line(
                    "Every control is unlocked again. Reopen this guide any time from Help > Guide: New Design Walkthrough.",
                    "Every control is unlocked again. Reopen this guide any time from Help > Guided walkthrough.",
                ),
        )
        // The longer sentence ends step 2's explanation, the shorter one ends steps 3 to 5.
        .edit(TextEdit::sub(
            " This step shows the Advanced controls because the Simple interface leaves Exact Scale Value out of the Meets list, so you can pick it there even if you type the angle by hand.",
            "",
        ))
        .edit(TextEdit::sub(
            " This step shows the Advanced controls because the Simple interface leaves Exact Scale Value out of the Meets list.",
            "",
        ))
}

fn quick_add() -> GuideOverlay {
    GuideOverlay::new("tiers-quick-add")
        .step(
            StepOverlay::of("Quick add a girdle")
                .line(
                    "The Quick Add buttons under the tier table's toolbar add a common first tier with one click.",
                    "The Quick Add panel, opened from the command bar, adds a common first tier with one click.",
                )
                .line(
                    "Click Girdle next to Quick Add.",
                    "Open Quick Add if it is not already open, then click Girdle.",
                ),
        )
        .step(StepOverlay::of("Quick add a table").line(
            "Click Table next to Quick Add.",
            "Open Quick Add if it is not already open, then click Table.",
        ))
        .step(StepOverlay::of("Quick add a culet").line(
            "Click Culet next to Quick Add.",
            "Open Quick Add if it is not already open, then click Culet.",
        ))
}

fn facet_chips() -> GuideOverlay {
    GuideOverlay::new("tiers-facet-chips")
        .step(StepOverlay::of("Detach one facet").line(
            "On the chip for position 84, click the detach icon.",
            "On the chip for position 84, click the \u{2192} arrow, which detaches the facet from its orbit.",
        ))
        .step(StepOverlay::of("Rotate the tier").line(
            "Click the first Rotate button, the one that moves facets up to higher index numbers.",
            "Click Rotate >, the button that moves facets up to higher index numbers.",
        ))
}

fn tier_notes() -> GuideOverlay {
    GuideOverlay::new("tiers-tier-notes")
        .step(StepOverlay::of("Add a note").line(
            "In the Tier tab, find Note under the facet chips.",
            "In the Tier tab, find Note for This Tier under the facet chips.",
        ))
        .step(StepOverlay::of("Clear the note").line(
            "Click Clear next to the field.",
            "Delete the text in the field, then click Set.",
        ))
}

fn duplicate() -> GuideOverlay {
    GuideOverlay::new("tiers-duplicate").step(StepOverlay::of("Duplicate a tier").line(
        "Click the copy icon at the end of the Crown Main row, or pick the row and press Ctrl+D.",
        "Click the duplicate (+) icon at the end of the Crown Main row, or pick the row and press Ctrl+D.",
    ))
}

fn delete() -> GuideOverlay {
    GuideOverlay::new("tiers-delete").step(
        StepOverlay::of("Remove a tier")
            .line(
                "Click the trash icon at the end of the Table row, or pick the row and press Delete.",
                "Click the x icon at the end of the Table row, or pick the row and press Delete.",
            )
            .line(
                "the Table row is gone, and a note says Removed Table, Undo.",
                "the Table row is gone, and a message offers Undo.",
            ),
    )
}

fn multi_select() -> GuideOverlay {
    GuideOverlay::new("tiers-multi-select")
        // The web tier table has no Space handler for the tick boxes.
        .step(
            StepOverlay::of("Pick two tiers")
                .sub(" Space on a row ticks it from the keyboard.", ""),
        )
        .step(StepOverlay::of("More on groups").sub("Adopt Sel. (Advanced)", "Adopt Sel."))
}

fn step_series() -> GuideOverlay {
    GuideOverlay::new("tiers-step-series")
        .step(StepOverlay::of("Generate a crown ladder").drop_line("Leave Keep Linked unticked."))
        .step(StepOverlay::of("Edit one rung").line(
            "A plain ladder is only a quick way to add tiers. The next lesson, Keep Linked, ties the rungs together.",
            "A plain ladder is only a quick way to add tiers: after Generate, each rung is an ordinary tier.",
        ))
}

fn adopt_imported_meets() -> GuideOverlay {
    GuideOverlay::new("tiers-adopt-imported-meets").step(
        StepOverlay::of("Open an imported design").line(
            "Open a .asc file with File > Open, or load an imported design from the library.",
            "Open a .asc file with File > Open (Ctrl+O).",
        ),
    )
}

fn arithmetic() -> GuideOverlay {
    GuideOverlay::new("tiers-arithmetic").step(
        StepOverlay::of("Where arithmetic works")
            .drop_line("The concave tier form's number fields."),
    )
}

fn cheater_offset() -> GuideOverlay {
    GuideOverlay::new("tiers-cheater-offset").step(StepOverlay::of("Clear it").line(
        "Click Clear next to the field.",
        "Delete the number in the field, then click Set.",
    ))
}

// The web's "+ Add Concave Tier" sits in the command bar above the tier table
// (`tier_table_commands.slint`, row 3), not in the table's own toolbar.
fn concave_cylinder_cone() -> GuideOverlay {
    GuideOverlay::new("tiers-concave-cylinder-cone")
        .edit(TextEdit::sub(
            "in the tier table's toolbar",
            "in the command bar above the tier table",
        ))
        .step(
            StepOverlay::of("A tier cut with a tool")
                .sub(" in the Simple and the Advanced interface alike", ""),
        )
}

fn concave_sphere_disc() -> GuideOverlay {
    GuideOverlay::new("tiers-concave-sphere-disc")
        .edit(TextEdit::sub(
            "in the tier table's toolbar",
            "in the command bar above the tier table",
        ))
        .step(StepOverlay::of("What the tool line says").line(
            "Open Cutting Mode to see the same two lines while you cut.",
            "Open Cutting Mode (the Cut Mode button in the header) to see the same two lines while you cut.",
        ))
}
