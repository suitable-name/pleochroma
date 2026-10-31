//! Guards where the browser raises the tutorial events of the output lessons (section 11:
//! `output-save`, `output-open`, `output-export-asc`, `output-export-gcs`, `output-cutting-sheet`,
//! `output-diagram-png`, `output-cutting-mode`).
//!
//! `guide_targets.rs` proves that every event of an offered lesson is raised somewhere. This file
//! pins WHERE, for the events of the files and the cutting steps: each is raised by the function
//! in which the action has succeeded (after the download was handed to the browser, after the
//! design was installed, after the mark was stored), never on a click and never when the action
//! was refused. The Schedule tab's two buttons are the outline stand-ins for the menu items that
//! cannot be outlined, so they must draw the two web-only targets.
//!
//! It only reads the sources as text, like `guide_targets.rs`; a copy of the crate without the web
//! app next to it skips the checks.

use pleochroma_editor::guide::viewing_events::{
    ASC_EXPORTED, CUTTING_STEP_MARKED, DESIGN_REPLACED, DESIGN_SAVED, DIAGRAM_EXPORTED,
    GCS_EXPORTED, SHEET_EXPORTED,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The web app's folder, or `None` when this crate was built without the workspace around it.
fn web_app() -> Option<PathBuf> {
    let app = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/pleochroma-web");
    app.join("src").is_dir().then_some(app)
}

/// The text of `relative` (below the web app), with `\n` line ends.
fn source(app: &Path, relative: &str) -> String {
    let path = app.join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
        .replace("\r\n", "\n")
}

/// The text of the top-level function that starts with `signature`, up to its closing brace in
/// column 0.
fn function<'a>(text: &'a str, signature: &str) -> &'a str {
    let start = text
        .find(signature)
        .unwrap_or_else(|| panic!("no function starts with `{signature}`"));
    let rest = &text[start..];
    let end = rest.find("\n}\n").map_or(rest.len(), |at| at + 3);
    &rest[..end]
}

/// Where `needle` first occurs in `text`, failing with `what` when it does not.
fn position(text: &str, needle: &str, what: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("{what}: `{needle}` is not there"))
}

#[test]
fn the_event_names_are_the_ones_the_lessons_wait_for() {
    assert_eq!(DESIGN_SAVED, "design_saved");
    assert_eq!(DESIGN_REPLACED, "design_replaced");
    assert_eq!(ASC_EXPORTED, "asc_exported");
    assert_eq!(GCS_EXPORTED, "gcs_exported");
    assert_eq!(SHEET_EXPORTED, "sheet_exported");
    assert_eq!(DIAGRAM_EXPORTED, "diagram_exported");
    assert_eq!(CUTTING_STEP_MARKED, "cutting_step_marked");
}

#[test]
fn each_download_raises_its_event_after_the_browser_took_the_file() {
    let Some(app) = web_app() else {
        return;
    };
    let save = source(&app, "src/io/save.rs");
    for (signature, event) in [
        ("pub fn save_asc(", "ASC_EXPORTED"),
        ("pub fn save_gcs(", "GCS_EXPORTED"),
        ("pub fn download_cutting_sheet(", "SHEET_EXPORTED"),
        ("pub fn download_diagram(", "DIAGRAM_EXPORTED"),
    ] {
        let body = function(&save, signature);
        let delivered = position(body, "deliver(ctx", signature);
        let raised = position(body, &format!("raise(ctx, {event})"), signature);
        assert!(
            delivered < raised,
            "{signature} must raise {event} after `deliver` handed the file to the browser"
        );
        assert_eq!(
            body.matches("raise(ctx").count(),
            1,
            "{signature} raises exactly one event"
        );
    }
}

#[test]
fn a_save_raises_design_saved_once_the_design_is_marked_saved() {
    let Some(app) = web_app() else {
        return;
    };
    let save = source(&app, "src/io/save.rs");
    let marked = function(&save, "fn mark_saved(");
    let marked_at = position(marked, "mark_saved();", "mark_saved");
    let raised_at = position(marked, "raise(ctx, DESIGN_SAVED)", "mark_saved");
    let continued_at = position(marked, "unsaved::design_saved(ctx)", "mark_saved");
    assert!(
        marked_at < raised_at && raised_at < continued_at,
        "DESIGN_SAVED goes out after the design is marked saved and before the action that waited \
         behind the unsaved-changes dialog goes on"
    );
    let design = function(&save, "pub fn save_design(");
    assert!(
        position(design, "deliver(ctx", "save_design")
            < position(design, "mark_saved(ctx", "save_design"),
        "save_design marks the design saved only after the download was handed over"
    );
    assert!(
        !function(&save, "pub fn save_to_library(").contains("raise("),
        "Save to Library downloads no file and is not the save the lessons ask for"
    );
}

#[test]
fn an_installed_design_raises_design_replaced() {
    let Some(app) = web_app() else {
        return;
    };
    let opened = source(&app, "src/io/mod.rs");
    let commit = function(&opened, "fn commit_opened(");
    assert!(
        position(commit, "replace_design(", "commit_opened")
            < position(commit, "raise(ctx, DESIGN_REPLACED)", "commit_opened"),
        "an opened file raises DESIGN_REPLACED after it replaced the design"
    );
    let created = source(&app, "src/editor/new_design/mod.rs");
    let install = function(&created, "fn install_session(");
    assert!(
        position(install, "replace_design(", "install_session")
            < position(install, "raise(ctx, DESIGN_REPLACED)", "install_session"),
        "a New Design raises DESIGN_REPLACED after it replaced the design"
    );
}

#[test]
fn a_mark_that_raised_the_done_count_raises_the_cutting_event() {
    let Some(app) = web_app() else {
        return;
    };
    let screen = source(&app, "src/editor/cutting_mode/screen.rs");
    let mark = function(&screen, "fn mark(");
    let guarded = position(mark, "if outcome.newly_done {", "mark");
    let raised = position(mark, "guide::raise(ctx, CUTTING_STEP_MARKED)", "mark");
    assert!(
        guarded < raised && raised > position(mark, "report(ctx, outcome", "mark"),
        "CUTTING_STEP_MARKED is raised only for a mark that raised the number of done steps, after \
         the page shows it"
    );
    for other in ["fn reset(", "fn step(", "fn present("] {
        assert!(
            !function(&screen, other).contains("CUTTING_STEP_MARKED"),
            "{other} must not raise CUTTING_STEP_MARKED: taking marks back or looking at a step \
             marks nothing"
        );
    }
}

#[test]
fn the_schedule_tab_draws_the_stand_ins_for_the_menu_downloads() {
    let Some(app) = web_app() else {
        return;
    };
    let schedule = source(&app, "ui/editor/inspector_schedule.slint");
    for target in ["schedule_cutting_sheet", "schedule_diagram"] {
        assert!(
            schedule.contains(&format!("GuideModel.highlight-target == \"{target}\"")),
            "the Schedule tab must outline {target}"
        );
    }
    assert!(
        schedule.matches("GuideModel.allows-file-ops()").count() >= 2,
        "the two downloads follow the file-action lock of the File menu entries they stand beside"
    );
}

#[test]
fn the_helpers_find_a_function_and_its_end() {
    let text = "fn a() {\n    one();\n}\n\nfn b() {\n    two();\n}\n";
    assert_eq!(function(text, "fn a("), "fn a() {\n    one();\n}\n");
    assert!(function(text, "fn b(").contains("two()"));
    assert!(!function(text, "fn b(").contains("one()"));
}
