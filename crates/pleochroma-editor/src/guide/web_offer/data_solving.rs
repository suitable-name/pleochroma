//! Web overlays for the solving, improving and history sections of the learning path.
//!
//! Every `from` text below is a line of the catalogue's lesson, word for word; a test applies each
//! overlay to the catalogue and fails when one no longer matches. The material lessons share
//! [`material_edits`]: the web sets a material in Design Settings and applies it with a button.

use super::overlay::{GuideOverlay, StepOverlay, TextEdit, material_edits};

/// The overlays of this file.
pub(super) fn overlays() -> Vec<GuideOverlay> {
    vec![
        solve(),
        auto_solve(),
        status(),
        verdict(),
        verdict_fixes(),
        preform(),
        yield_and_carat(),
        retarget_shift(),
        retarget_optimize(),
        history(),
        snapshot_compare(),
    ]
}

/// The web's name for the auto-solve list: a combo labelled `Auto:`.
fn auto_list() -> TextEdit {
    TextEdit::sub("Auto-Solve list", "Auto list")
}

/// The line that sends the learner to the strip's list of messages.
const STRIP_LINK: &str =
    "Click the link at the right end of the strip to see every current message in full.";

/// What the web has instead: the fold-out of manufacturability warnings in the status block.
const STRIP_LINK_WEB: &str =
    "Open Manufacturability in the status block to see every current warning in full.";

fn solve() -> GuideOverlay {
    GuideOverlay::new("solving-solve")
        .edit(auto_list())
        .step(StepOverlay::of("Press Solve").line(
            "Above 16 tiers Solve runs in the background: the button reads Abandon Solve, the strip's activity chip counts the seconds, and you can keep editing meanwhile. An edit that lands before it finishes discards its result.",
            "Above a few dozen planes Solve runs in the background: the button reads Solving... and you can keep editing meanwhile. An edit that lands before it finishes discards its result.",
        ))
        .step(
            StepOverlay::of("Read the result")
                .sub("Stale, Solving and Failed", "Stale, Solving and Cannot be solved")
                .line(STRIP_LINK, STRIP_LINK_WEB),
        )
}

fn auto_solve() -> GuideOverlay {
    GuideOverlay::new("solving-auto-solve").edit(auto_list())
}

fn status() -> GuideOverlay {
    GuideOverlay::new("solving-status")
        .summary("Read the status strip after a good solve and a failed one, and fix a missing anchor.")
        .step(
            StepOverlay::of("Read the status strip")
                .line(
                    "The strip is one dot, one line and a link to the full list of messages, and the solver's own state always leads.",
                    "The strip is one dot and one line, with the full list of warnings one click away, and the solver's own state always leads.",
                )
                .sub(
                    "Stale, Solving, Failed or Solved",
                    "Stale, Solving, Cannot be solved or Solved",
                )
                .line(STRIP_LINK, STRIP_LINK_WEB),
        )
        .step(
            StepOverlay::of("Solve the broken design")
                .sub("the state word says Failed", "the state word says Cannot be solved"),
        )
        .step(
            StepOverlay::of("Abandon, chips and warnings")
                .retitle("Warnings")
                .line(
                    "Three more things the strip tells you.",
                    "One more thing the strip tells you.",
                )
                .drop_line("Above 16 tiers Solve runs in the background. The Solve button turns into Abandon Solve, and the strip's activity chip counts the seconds. Abandon Solve stops waiting and ignores the result; the strip goes back to Stale.")
                .drop_line("Every task longer than about a fifth of a second (Deep Solve, Optimize, a sweep) shows as a small chip with its running time, and a cancel button where it can really be stopped."),
        )
}

fn verdict() -> GuideOverlay {
    GuideOverlay::new("solving-verdict")
        .edits(material_edits("Opal"))
        .step(StepOverlay::of("Find the verdict badge").line(
            "Look at the left end of the status strip, beside the solver state.",
            "Look at the top of the window, to the left of the Material list: the badge sits in the header.",
        ))
        .step(StepOverlay::of("Open the reasons").line(
            "Click the badge, or Tab to it and press Enter.",
            "Click the badge.",
        ))
}

fn verdict_fixes() -> GuideOverlay {
    GuideOverlay::new("solving-verdict-fixes").step(StepOverlay::of("Take the table away").line(
        "Click the trash button at the end of the Table row (or press Delete).",
        "Click the x button at the end of the Table row (or press Delete).",
    ))
}

fn preform() -> GuideOverlay {
    GuideOverlay::new("solving-preform")
        .step(
            StepOverlay::of("Read the proportions")
                .line(
                    "Table %, Crown Angle, Pavilion Angle, Total Depth % and Girdle % carry a chip: Within, Near or Outside a reference window.",
                    "Crown Angle and Pavilion Angle are estimated from the schedule (the main facet of each block).",
                )
                .line(
                    "The windows depend on the design's shape and material. They are guidance, not a grade: a chip reading Outside is a prompt to look closer, not a verdict on the design's worth.",
                    "These figures are guidance, not a grade: use them to look closer, not as a verdict on the design's worth.",
                ),
        )
        .step(
            StepOverlay::of("Make the rough wider")
                .line(
                    "In the Preform group, change Half-Width from 1.5 to 1.8. The slider under it works too.",
                    "In the Preform group, change Half-Width from 1.5 to 1.8.",
                )
                .sub(
                    "A slider only fills its field in: nothing changes in the design until you click Apply Preform.",
                    "Nothing changes in the design until you click Apply Preform.",
                ),
        )
        // The web Preform tab has no rough-shape control and no Simple interface, so nothing of
        // this step is true in the browser.
        .omit("Block, Cylinder and the Simple interface")
}

fn yield_and_carat() -> GuideOverlay {
    GuideOverlay::new("solving-yield-carat")
        .edits(material_edits("Quartz"))
        .step(
            StepOverlay::of("Where yield is used")
                .line(
                    "The Optimize tab's Keep Weight objective favours angles that waste less of the rough.",
                    "The Optimize tab's Yield Weight favours angles that waste less of the rough.",
                )
                .drop_line("The angle sweep table has a Yield % column.")
                .drop_line("The Rough Planner uses the carat estimate to fit stones into a rough."),
        )
}

fn retarget_shift() -> GuideOverlay {
    GuideOverlay::new("optimizing-retarget-shift")
        .edits(material_edits("Quartz"))
        .step(
            StepOverlay::of("How Shift works")
                .sub("Crown Handling (Advanced)", "Crown Handling"),
        )
        .step(
            StepOverlay::of("Retarget to topaz")
                .line(
                    "Click Retarget... on the command bar.",
                    "Choose Design > Retarget for Material... in the menu bar.",
                )
                .line(
                    "Set Target Material to Topaz. A card under it shows its n_D and critical angle. Stay on 1. Shift.",
                    "Set Target Material to Topaz. Keep Mode on Shift.",
                )
                .sub(
                    "Block, Tier, Old, New, Margin and Risk",
                    "Block, Name, Old, New, Margin and Risk",
                )
                .sub(
                    " Under it, Optical Comparison (Table Up) gives windowing, brilliance and extinction for the current stone, the current stone in Topaz, and the retargeted stone.",
                    "",
                ),
        )
}

fn retarget_optimize() -> GuideOverlay {
    GuideOverlay::new("optimizing-retarget-optimize")
        .edits(material_edits("Quartz"))
        .step(StepOverlay::of("Shift or Optimize").sub(
            " It runs only when you press Search, in the background.",
            " It runs in the background when you pick Optimize.",
        ))
        .step(
            StepOverlay::of("Search and apply")
                .line(
                    "Click Retarget... on the command bar.",
                    "Choose Design > Retarget for Material... in the menu bar.",
                )
                .line(
                    "Set Target Material to Sapphire, inspect 1. Shift, then select 2. Optimize. The shifted stone stays on the left; the right waits for an option.",
                    "Set Target Material to Sapphire, look at the Shift proposal, then set Mode to Optimize.",
                )
                .drop_line("Leave Objective on Balanced and Range on 6 degrees either side, and set Effort to Quick (100 steps). These three show in the Advanced interface; the lesson shows them for now.")
                .line(
                    "Click Search for Better Angles. A line names the stage and how far it has got; Cancel stops the search without closing the dialog.",
                    "Wait for the search to finish: a line names the stage and how far it has got, and Cancel Search stops it without closing the dialog.",
                )
                .line(
                    "Under Options (click one to see it) up to three options are listed, best score first. Click one: the table, the verdict and the comparison switch to it. Only valid options are listed.",
                    "Read the table: the search proposes one set of angles for the new material, with its verdict.",
                )
                .line(
                    "Click Apply. It stays dim until an option is picked. This lesson goes on when the design has changed, and the dialog closes itself.",
                    "Click Apply. This lesson goes on when the design has changed, and the dialog closes itself.",
                )
                .line(
                    "Searching and picking change nothing. Apply commits the option's angles, heights and the material as one undo step.",
                    "Searching changes nothing. Apply commits the proposal's angles, heights and the material as one undo step.",
                ),
        )
        .omit("Reading the options")
}

fn history() -> GuideOverlay {
    GuideOverlay::new("solving-history")
        .step(
            StepOverlay::of("Open the History tab")
                .sub(", with a small picture of the design at that step", "")
                .drop_line("At the top of the tab, check that Steps is picked. Variants is the other view.")
                .sub(" You can also press Ctrl+K and type history.", ""),
        )
        .step(StepOverlay::of("Change the crown").sub(
            " with a picture and a line saying what the step did",
            " with a line saying what the step did",
        ))
        .step(StepOverlay::of("Go back one step").sub(
            " The top row is now dimmed and reads undone.",
            " The top row is now dimmed.",
        ))
        .step(StepOverlay::of("Go back to Start").drop_line("The whole list is one Tab stop. Up and Down move an outline between rows, Enter or Space goes to the outlined row, Home moves to the newest step and End to Start. Moving the outline only looks; nothing changes until you press Enter."))
        .step(
            StepOverlay::of("A new change drops the undone steps")
                .sub(
                    "Creating a new design, opening a file or loading a library design starts a new history",
                    "Creating a new design or opening a file starts a new history",
                )
                .line(
                    "To keep a design for good before you try something risky, save it as a variant. The next lesson shows how.",
                    "To keep a design for good before you try something risky, save it with File > Save Design (.psdd).",
                ),
        )
}

fn snapshot_compare() -> GuideOverlay {
    GuideOverlay::new("solving-snapshot-compare")
        .title("Snapshot and Compare")
        .summary("Take a Snapshot, change the design, and see what changed in a table.")
        .step(
            StepOverlay::of("Set a Baseline")
                .retitle("Take a Snapshot")
                .line(
                    "The Baseline is a copy of the design and its solved depths, to compare against later. A Baseline is set when a design opens, so this step sets a fresh one on purpose.",
                    "The Snapshot is a copy of the design and its solved depths, to compare against later.",
                )
                .line(
                    "Click Set Baseline on the command bar, or Set as Baseline in the strip under the inspector's tabs.",
                    "Choose Design > Snapshot Design in the menu bar.",
                )
                .line(
                    "a message says Baseline set, and the strip reads against it.",
                    "a message says the Snapshot was taken.",
                )
                .line(
                    "The Baseline is not saved anywhere. It lives only in memory for this session: closing the design or the program, or setting a new Baseline, replaces or loses it. It is not written to the design file. A trial can also become the Baseline (see Keep Trials).",
                    "The Snapshot is not saved anywhere. It lives only in memory for this session: closing the design or the page, or taking a new Snapshot, replaces or loses it. It is not written to the design file.",
                )
                .line("a Baseline set", "a Snapshot taken"),
        )
        .step(
            StepOverlay::of("Compare to the Baseline")
                .retitle("Compare to the Snapshot")
                .line(
                    "Click Compare to Baseline, next to Set Baseline. The Compare to Baseline window opens over the editor, so everything for this step is here.",
                    "Choose Design > Compare to Snapshot... in the menu bar. The comparison opens over the editor, so everything for this step is here.",
                )
                .drop_line("Read the line under the title. Its second line says in words how the stone differs optically, or No clear optical difference.")
                .sub("its Before and After angles", "its Snapshot and Now angles")
                .line(
                    "a table of the tiers with Before and After columns.",
                    "a table of the tiers with Snapshot and Now columns.",
                )
                .line(
                    "Added is a tier that did not exist at the Baseline, Removed one the Baseline had that is gone.",
                    "Added is a tier that did not exist at the Snapshot, Removed one the Snapshot had that is gone.",
                ),
        )
        .omit("See both stones")
        .omit("Read the compare window")
}
