//! Web overlays for the lessons that wait for a web feature (the `B` class of the lesson audit):
//! Deep Solve, the library, cutting mode and the Cut slider.
//!
//! Deep Solve, the library and cutting mode now exist in the browser, so these lines describe their
//! real screens: the Deep Solve section of the Optimize tab, the Design Library screen (File >
//! Design Library..., Alt+L) with its search box, Shape, Gear and Tag lists, Concave Only chip and
//! Open Design button, and the Cut Mode button; the Cut slider reads Cut: Rough, Cut: n/N and
//! Cut: Finished. The variants view and a design's own lighting have no browser screen, so those two
//! lessons are class C (`web_offer.rs`) and have no overlay here. Every `from` text is a line of the catalogue's lesson, word for
//! word; a test applies each overlay to the catalogue and fails when one no longer matches.

use super::overlay::{GuideOverlay, StepOverlay};
use crate::guide::{Goal, Group, viewing_events::LIBRARY_FILTERED};

/// The groups a library step leaves usable besides the catalogue's: the library is a screen of its
/// own that File > Design Library... (a file operation) opens, so a step must not lock it.
const OPEN_LIBRARY: &[Group] = &[Group::FileOps];

/// The overlays of this file.
pub(super) fn overlays() -> Vec<GuideOverlay> {
    vec![
        deep_solve(),
        cut_slider(),
        cutting_mode(),
        library_import(),
        library_search(),
        library_filters(),
        library_load_design(),
    ]
}

fn deep_solve() -> GuideOverlay {
    GuideOverlay::new("optimizing-deep-solve")
        .summary("Check a design's solved stone against the proportions printed for it, and read the verdict.")
        .step(
            StepOverlay::of("What Deep Solve checks")
                .line(
                    "Deep Solve is a slow, separate check: does this design's geometry reproduce the proportions printed in the catalogue?",
                    "Deep Solve is a slow, separate check: does this design's geometry reproduce the proportions printed for it? Its button is in the Optimize tab, below the tier table.",
                )
                .line(
                    "It needs a design with printed proportions: one loaded from the library. A new design or a template has none, and the button is dim. Hover the button to see why.",
                    "It needs a design with printed proportions: type them in Design Settings under Printed Proportions, or open a file that carries them. A new design or a template has none, and the button is dim. The line under the Deep Solve heading says why.",
                )
                .sub(
                    "the Adopt button in the tier table's ACTIONS group",
                    "the Adopt button in the tier table",
                )
                .sub(
                    "(the status strip says so)",
                    "(the status box under the tier table says so)",
                )
                .line(
                    "If the button is dim, use Skip Step: this lesson only reads the result of a run. Load a catalogue design from the Library, adopt a tier, and start the lesson again to run it.",
                    "If the button is dim, use Skip Step: this lesson only reads the result of a run. Open a design with printed proportions, adopt a tier, and start the lesson again to run it.",
                ),
        )
        .step(
            StepOverlay::of("Run Deep Solve")
                .line(
                    "Click Deep Solve on the command bar, next to Solve.",
                    "Open the Optimize tab below the tier table, then click Deep Solve in its Deep Solve section.",
                )
                .line(
                    "The button turns into Cancel Deep Solve, and the status strip shows that Deep Solve is running.",
                    "A Cancel button appears beside Deep Solve, and the box under them shows that Deep Solve is running.",
                )
                .line(
                    "the button reads Cancel Deep Solve.",
                    "a Cancel button appears beside Deep Solve.",
                )
                .sub(
                    "It runs off the main thread, so you can keep working.",
                    "It runs in a background worker, so the page stays responsive. An edit, Optimize or Retarget while it runs ends it.",
                )
                .sub(
                    "Cancel stops the search at its next step and discards its result.",
                    "Cancel stops the search at its next step and discards its result; pressing it again (it then reads Stop now) ends the search at once.",
                ),
        )
        .step(
            StepOverlay::of("Wait for the verdict")
                .line(
                    "Wait until the button reads Deep Solve again.",
                    "Wait until the Cancel button is gone.",
                )
                .line(
                    "Keep working elsewhere meanwhile if you like, but do not edit the design: a result for an edited design is out of date.",
                    "Look around meanwhile if you like, but do not edit the design or start Optimize or Retarget: any of them ends the run.",
                )
                .line(
                    "the full list of messages shows Deep Solve's verdict.",
                    "the box under the Deep Solve button shows Deep Solve's verdict.",
                ),
        )
        .step(
            StepOverlay::of("Read the verdict")
                .sub(
                    "Click the link at the right end of the status strip to open the full list of messages.",
                    "Read the box under the Deep Solve button: it holds the verdict, and a table lists the tiers whose mast moved, with BEFORE, AFTER and DELTA.",
                )
                .sub(
                    "If the design was edited since it was loaded, the log adds that the printed figures may no longer describe the design you are holding.",
                    "The printed figures describe the design as it was published, so after your own edits a deviation may be your change.",
                )
                .sub(
                    "A Pin on a run that is out of date does nothing but tell you to run Deep Solve again.",
                    "After an edit the result is badged Stale: Design Changed, its Pin buttons are dim, and Recompute runs Deep Solve again.",
                ),
        )
}

fn cut_slider() -> GuideOverlay {
    GuideOverlay::new("viewing-cut-slider")
        .step(
            StepOverlay::of("Find the Cut slider")
                .line(
                    "Click the Edit tab above the picture, then Solid, Path-Traced or Ray-Traced. Solid can be combined with either tracer.",
                    "Click the Solid tab above the picture.",
                )
                .sub("reads Finished", "reads Cut: Finished"),
        )
        .step(StepOverlay::of("Go back to the rough").sub("reads Rough", "reads Cut: Rough"))
        .step(StepOverlay::of("Step through the cuts").line(
            "The label names the tier you have just cut and counts the steps, for example After Crown Main (2 of 4).",
            "The label counts the steps, for example Cut: 2/4.",
        ))
        .step(StepOverlay::of("Back to the finished stone").sub("reads Finished", "reads Cut: Finished"))
        .step(StepOverlay::of("What the slider leaves alone").drop_line(
            "Live Render follows the cut, with a small amber badge and a Show Finished button.",
        ))
}

fn cutting_mode() -> GuideOverlay {
    GuideOverlay::new("output-cutting-mode").step(
        StepOverlay::of("Open it and mark a step")
            .line(
                "Click the Edit tab, then Cut Mode in the command bar (or choose Edit, Cutting Mode..., or press Ctrl+K and type cutting).",
                "Click Cut Mode in the header (or choose Design > Cutting Mode..., or press Alt+C).",
            )
            .line(
                "The progress line at the bottom of the page reads 1 of 4 steps done.",
                "The progress line under the title reads 1 of 4 steps done.",
            )
            .line(
                "Your marks are kept in your library on this computer, filed under the design. There is nothing to save, and closing the program loses nothing.",
                "Your marks are kept in this browser, filed under the design. There is nothing to save, and closing the page loses nothing. Another browser, or cleared site data, starts without them.",
            ),
    )
}

fn library_import() -> GuideOverlay {
    GuideOverlay::new("library-import")
        .step(
            StepOverlay::of("Make a file to import")
                .line(
                    "Choose File > Export .asc in the command bar.",
                    "Choose File > Save .asc.",
                )
                .line(
                    "In the Save dialog choose a folder you will find again, then click Save.",
                    "Your browser downloads the .asc file; remember where it goes.",
                )
                .line(
                    "A toast says Exported edited schedule to the path you chose.",
                    "A message says Downloaded, followed by the file name.",
                ),
        )
        .step(
            StepOverlay::of("Import it")
                .line(
                    "The Import panel opens over the window, so this step carries everything you need.",
                    "The design library opens over the window, so this step carries everything you need.",
                )
                .line(
                    "Click Import at the top right of the window.",
                    "Open the library with File > Design Library... (Alt+L), then click Import Designs... at the top right.",
                )
                .line(
                    "Click Choose File... and pick the .asc file you just wrote.",
                    "In the file picker, choose the .asc file you just wrote.",
                )
                .line(
                    "Wait for the import to finish. To import a whole folder instead, tick Include Subfolders first, then click Choose Folder...",
                    "Wait for the import to finish. To import several files at once, pick them all in the file picker.",
                )
                .line(
                    "A toast and the status line at the top report what was imported, and the new design is selected in the Catalog list.",
                    "A message reports what was imported, and the new design is in the list.",
                )
                .sub(
                    "A file with the same name as one already in the library asks before it replaces it.",
                    "Importing the same file again changes nothing; a changed file with the same name replaces the entry and keeps its tags.",
                )
                .allow_add(OPEN_LIBRARY),
        )
        .omit("Previews")
}

fn library_search() -> GuideOverlay {
    GuideOverlay::new("library-search")
        .step(
            StepOverlay::of("The catalogue")
                .line(
                    "The Catalog panel on the left lists every design that matches your search and filters.",
                    "The design library lists every design that matches your search and filters.",
                )
                .line(
                    "Its header reads Catalog (N of M): how many designs match, out of how many there are.",
                    "Open it with File > Design Library... (Alt+L). The line above the list says how many designs match, for example 12 matching designs.",
                )
                .allow_add(OPEN_LIBRARY),
        )
        .step(
            StepOverlay::of("Type in the search box")
                .line(
                    "Click the search box at the top (or press Ctrl+F) and type a few letters of a title or a designer.",
                    "Click the search box at the top and type a few letters of a title or a designer.",
                )
                .line(
                    "The list shortens and the header's N changes.",
                    "The list shortens and the number above it changes.",
                )
                .line(
                    "The Shape and Gear drop-downs next to the box narrow the list further.",
                    "The Shape, Gear and Tag drop-downs under the box narrow the list further.",
                )
                .allow_add(OPEN_LIBRARY),
        )
        .step(
            StepOverlay::of("Clear it")
                .line(
                    "A small cross appears in the box once there is text in it.",
                    "The box shows its hint text again once it is empty.",
                )
                .line(
                    "Click the cross at the right end of the search box.",
                    "Select the text in the search box and delete it.",
                )
                .allow_add(OPEN_LIBRARY),
        )
        .step(
            StepOverlay::of("Select a design")
                .line(
                    "Click a card in the Catalog list.",
                    "Click a design in the list.",
                )
                .line(
                    "The detail header shows the title, the designer and a row of chips for shape, gear, facets, proportions and refractive index.",
                    "The detail pane shows the design's picture, its title and designer, and its shape, refractive index and proportions.",
                )
                .drop_line(
                    "Right-click a card for more: Generate Previews, Compute Tilt Curves, Ignore, Exclude from Planner and Build This Design.",
                )
                .allow_add(OPEN_LIBRARY),
        )
}

fn library_filters() -> GuideOverlay {
    GuideOverlay::new("library-filters")
        .step(
            StepOverlay::of("Pick a shape")
                .line(
                    "Open the Shape drop-down next to the search box and choose a shape.",
                    "Open the library with File > Design Library... (Alt+L), then open the Shape drop-down under the search box and choose a shape.",
                )
                .line(
                    "The list shortens. Choosing All Shapes puts it back.",
                    "The list shortens. Choosing All puts it back.",
                )
                .allow_add(OPEN_LIBRARY),
        )
        .step(
            StepOverlay::of("Range filters")
                .retitle("Tag and chips")
                .line(
                    "The Advanced Filters panel opens over the window, so this step carries everything you need.",
                    "Two more filters sit under the Shape and Gear drop-downs: the Tag drop-down and the Concave Only chip.",
                )
                .line(
                    "Click Filters, to the right of the Sort drop-down.",
                    "Open the Tag drop-down and choose a tag, or click the Concave Only chip.",
                )
                .line(
                    "Drag a handle of the Refractive Index slider. The panel's first line counts how many designs match.",
                    "Watch the line above the list: it counts how many designs match. Leave the filter on for the last step.",
                )
                .drop_line("Click outside the panel to close it.")
                .line(
                    "The Filters button turns amber with a dot while a filter is active.",
                    "The chip is lit while it is active, and the list shortens.",
                )
                .line(
                    "Each slider spans the range your catalogue really has. RI Match centres a tolerance band on a refractive index, and Use Current Material takes the one in the viewport. Tilt Performance filters narrow by windowing, extinction or brilliance.",
                    "Show Ignored, the other chip, does the opposite: it adds the designs you marked Ignored in the detail pane back to the list.",
                )
                .goal(
                    Goal::Event(LIBRARY_FILTERED.to_owned()),
                    "A tag or chip changed",
                )
                .allow_add(OPEN_LIBRARY),
        )
        .step(
            StepOverlay::of("Sort the list")
                .line(
                    "Open the Sort drop-down and choose Newest.",
                    "Open the Sort drop-down, at the right of the search box, and choose Oldest First.",
                )
                .line(
                    "The designs added last are at the top.",
                    "The designs added first are at the top.",
                )
                .line(
                    "The other orders are Catalogue Order, Title and Recently Edited.",
                    "The other orders are Recently Added (the default), Recently Edited and Title A-Z.",
                )
                .allow_add(OPEN_LIBRARY),
        )
        .step(
            StepOverlay::of("Put everything back")
                .line(
                    "An active filter is easy to forget, so the Filters button stays amber until you clear it.",
                    "An active filter is easy to forget, so look at the drop-downs and chips before you leave the library.",
                )
                .line(
                    "Click Filters, then Reset Filters at the bottom of the panel, then click outside the panel.",
                    "Click the Concave Only chip off, and set the Tag drop-down back to All.",
                )
                .line(
                    "Choose All Shapes and All Gears in the two drop-downs if you set them.",
                    "Choose All in the Shape and Gear drop-downs if you set them.",
                )
                .line(
                    "The Filters button is no longer amber and the header reads Catalog (M of M).",
                    "No chip is lit, every drop-down reads All and the line above the list counts every design.",
                )
                .allow_add(OPEN_LIBRARY),
        )
}

fn library_load_design() -> GuideOverlay {
    GuideOverlay::new("library-load-design")
        .step(
            StepOverlay::of("Select a design")
                .line(
                    "Load Selected opens the design that is highlighted in the Catalog list.",
                    "Open Design opens the design that is highlighted in the library's list.",
                )
                .line(
                    "Click a card in the Catalog list. If the list is empty, run the Import tutorial first.",
                    "Open the library with File > Design Library... (Alt+L), then click a design in the list. If the list is empty, run the Import tutorial first.",
                )
                .line(
                    "The detail header shows the design's title and chips.",
                    "The detail pane shows the design's picture, title and fields.",
                ),
        )
        .step(
            StepOverlay::of("Load it")
                .drop_line("Click the Edit tab above the picture.")
                .line(
                    "Choose File > Load Selected in the command bar. If the design on screen has unsaved changes you are asked first.",
                    "Click Open Design in the detail pane (or double-click the design in the list). If the design on screen has unsaved changes you are asked first.",
                )
                .sub("its stone is in the Solid view", "its stone is in the picture")
                .line(
                    "The design is built from the entry's own file (its .asc, else its .gem, else its .gcs). An entry with only an angle table is rebuilt from the table with placeholder depths, so solve it before you trust it.",
                    "The design is built from the entry's own file: its design file (.psdd) if it has one, else its .gcs, .asc or .gem. The library closes and the design opens in the editor.",
                ),
        )
        .step(
            StepOverlay::of("What loading does not do")
                .line(
                    "Your edits belong to the design in the editor. Save writes them to a .psdd file and updates the library entry; Export .asc writes a file and leaves the library alone.",
                    "Your edits belong to the design in the editor. File > Save to Library writes them back into the library entry; Save Design (.psdd) and Save .asc download a file and leave the library alone.",
                )
                .drop_line(
                    "The status strip may offer to set the material when the design's refractive index matches a built-in one.",
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::{
        super::{WebClass, web_lesson},
        *,
    };
    use crate::guide::{Guide, GuideCatalog, Platform};

    /// Lesson `id` as the browser shows it.
    fn web(id: &str) -> Guide {
        GuideCatalog::new()
            .guide_for(id, Platform::Web)
            .unwrap_or_else(|| panic!("the catalogue has no lesson {id}"))
    }

    /// Every text a learner reads in `guide`, one per line.
    fn words(guide: &Guide) -> String {
        let mut lines = vec![guide.title.clone(), guide.summary.clone()];
        for step in &guide.steps {
            lines.push(step.title.clone());
            lines.push(step.intro.clone());
            lines.extend(step.actions.iter().cloned());
            lines.push(step.check.clone());
            lines.push(step.why.clone());
            lines.push(step.waiting.clone());
        }
        lines.join("\n")
    }

    const LIBRARY_LESSONS: [&str; 4] = [
        "library-import",
        "library-search",
        "library-filters",
        "library-load-design",
    ];

    #[test]
    fn every_overlay_of_this_file_belongs_to_a_lesson_that_waits_for_a_feature() {
        for overlay in overlays() {
            let lesson = web_lesson(&overlay.guide)
                .unwrap_or_else(|| panic!("{} is not a lesson", overlay.guide));
            assert_eq!(lesson.class, WebClass::B, "{}", overlay.guide);
        }
    }

    #[test]
    fn the_library_lessons_name_the_screens_real_labels_and_no_desktop_panel() {
        let search = words(&web("library-search"));
        for label in ["Design Library... (Alt+L)", "Shape, Gear and Tag"] {
            assert!(search.contains(label), "search lacks {label:?}");
        }
        let filters = words(&web("library-filters"));
        for label in ["Concave Only", "Oldest First", "Tag drop-down"] {
            assert!(filters.contains(label), "filters lack {label:?}");
        }
        assert!(words(&web("library-import")).contains("Import Designs..."));
        let load = words(&web("library-load-design"));
        assert!(load.contains("Open Design"));
        assert!(load.contains("Save to Library"));
        for id in LIBRARY_LESSONS {
            let text = words(&web(id));
            for gone in [
                "Catalog (",
                "Catalog list",
                "Catalog panel",
                "Catalogue Order",
                "Filters button",
                "Reset Filters",
                "slider",
                "Choose File...",
                "Include Subfolders",
                "the cross",
            ] {
                assert!(!text.contains(gone), "{id} still says {gone:?}");
            }
        }
    }

    #[test]
    fn a_library_step_that_waits_leaves_the_file_menu_usable() {
        // The library is a screen that File > Design Library... opens; a lock on file
        // operations would shut the learner out of the screen the step is about.
        for id in LIBRARY_LESSONS {
            for step in &web(id).steps {
                if !matches!(step.goal, Goal::Manual) {
                    assert!(
                        step.allow.contains(&Group::FileOps),
                        "{id} / {:?} locks file operations",
                        step.title
                    );
                }
            }
        }
    }

    #[test]
    fn the_library_steps_wait_only_for_events_the_library_raises() {
        let raised = [
            "asc_exported",
            "library_imported",
            "library_searched",
            "library_search_cleared",
            "library_filtered",
            "library_filters_reset",
            "library_design_selected",
            "design_replaced",
        ];
        for id in LIBRARY_LESSONS {
            for step in &web(id).steps {
                for name in step.goal.events() {
                    assert!(
                        raised.contains(&name),
                        "{id} / {:?} waits for {name}",
                        step.title
                    );
                }
            }
        }
    }

    #[test]
    fn the_range_filter_step_became_the_tag_and_chip_step() {
        let guide = web("library-filters");
        let step = guide
            .steps
            .iter()
            .find(|step| step.title == "Tag and chips")
            .expect("the step was retitled");
        assert_eq!(step.goal.events(), [LIBRARY_FILTERED]);
        assert_eq!(step.waiting, "A tag or chip changed");
        assert!(guide.steps.iter().all(|step| step.title != "Range filters"));
    }

    #[test]
    fn deep_solve_is_in_the_optimize_tab_and_has_a_cancel_button() {
        let text = words(&web("optimizing-deep-solve"));
        for label in [
            "Optimize tab",
            "Cancel",
            "Stop now",
            "Stale: Design Changed",
            "Pin",
        ] {
            assert!(text.contains(label), "Deep Solve lacks {label:?}");
        }
        for gone in [
            "status strip",
            "Cancel Deep Solve",
            "command bar",
            "next to Solve",
        ] {
            assert!(!text.contains(gone), "Deep Solve still says {gone:?}");
        }
    }

    #[test]
    fn cutting_mode_is_opened_from_the_header_or_alt_c() {
        let text = words(&web("output-cutting-mode"));
        assert!(text.contains("Cut Mode in the header"));
        assert!(text.contains("Alt+C"));
        assert!(!text.contains("Ctrl+K"));
        assert!(text.contains("kept in this browser"));
    }

    #[test]
    fn the_cut_slider_lesson_reads_the_browser_labels() {
        let text = words(&web("viewing-cut-slider"));
        for label in ["Cut: Finished", "Cut: Rough", "Cut: 2/4", "Solid tab"] {
            assert!(
                text.contains(label),
                "the Cut slider lesson lacks {label:?}"
            );
        }
        assert!(!text.contains("After Crown Main"));
        assert!(!text.contains("Show Finished"));
    }
}
