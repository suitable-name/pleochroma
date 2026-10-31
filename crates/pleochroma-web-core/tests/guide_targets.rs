//! Guards the on-screen hooks of the lessons the browser offers (`GuideCatalog::web_guides`):
//!
//! - every UI event a lesson waits for is raised somewhere in `apps/pleochroma-web/src`: a Rust
//!   call (`guide::raise` or `guide::notify` with the constant, or the quoted name) or a Slint
//!   call of `GuideModel.event("name")`. An event nobody raises is a step that never completes;
//! - every highlight target a lesson names is drawn by some `.slint` file under
//!   `apps/pleochroma-web/ui` (a component that compares `GuideModel.highlight-target` with it),
//!   unless it is one of the native-menu targets the web shows as text only
//!   (`WEB_TEXT_ONLY_TARGETS`). A target nobody draws would only mean "no outline", silently.
//!
//! The runner's own files (`src/editor/guide.rs` and `src/editor/guide/`) are not raise sites:
//! they mention event names to judge steps, not because a user acted.
//!
//! It only reads the sources as text, like `apps/pleochroma-cut/tests/guide_highlights.rs`. The
//! two catalogue-wide tests (`every_event_of_an_offered_lesson_is_raised_by_the_browser`,
//! `every_target_of_an_offered_lesson_is_drawn_by_the_browser`) stay red until every lane of the
//! tutorial wiring has landed; each failure message lists everything still missing. The
//! `the_tier_selection_*` test is green as soon as the tier-editing lane has landed.
//!
//! The sources are found next to this crate (`../../apps/pleochroma-web`); a copy of the crate
//! without them (a packaged crate) skips the checks.

use pleochroma_editor::guide::{
    GuideCatalog, HIGHLIGHT_TARGETS, TIERS_MULTI_SELECTED, WEB_ONLY_TARGETS, WEB_TEXT_ONLY_TARGETS,
    WebFeatures, viewing_events::TIER_SELECTED,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Names (events or targets), each with the first offered lesson that needs it.
type Needs = Vec<(String, String)>;

/// Source files as `(path relative to the web app, text)`.
type Files = [(String, String)];

/// Calls that hand an event name to the lesson runner, as the Rust sources spell them.
const RAISE_CALLS: &[&str] = &["raise(", "guide::notify(", ".event("];

/// The web app's folder, or `None` when this crate was built without the workspace around it.
fn web_app() -> Option<PathBuf> {
    let app = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/pleochroma-web");
    app.join("src").is_dir().then_some(app)
}

/// Every file under `dir` with the extension `extension`, appended to `found`.
fn collect(dir: &Path, extension: &str, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, extension, found);
        } else if path.extension().is_some_and(|ext| ext == extension) {
            found.push(path);
        }
    }
}

/// The text of every file with `extension` under `folder` of the web app, with its path
/// relative to the app and `/` as the separator.
fn sources(app: &Path, folder: &str, extension: &str) -> Vec<(String, String)> {
    let mut paths = Vec::new();
    collect(&app.join(folder), extension, &mut paths);
    assert!(!paths.is_empty(), "no .{extension} files under {folder}/");
    paths
        .iter()
        .map(|path| {
            let text = fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
            let relative = path
                .strip_prefix(app)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            (relative, text)
        })
        .collect()
}

/// Whether `word` occurs in `text` as a whole identifier (no letter, digit or `_` next to it).
fn contains_word(text: &str, word: &str) -> bool {
    let is_part = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).any(|(start, found)| {
        let before = text[..start].chars().next_back();
        let after = text[start + found.len()..].chars().next();
        !before.is_some_and(is_part) && !after.is_some_and(is_part)
    })
}

/// Whether `relative` is a file of the lesson runner (`src/editor/guide.rs`, `src/editor/guide/`).
///
/// The runner judges events; it never raises one for a user's action.
fn is_runner_file(relative: &str) -> bool {
    relative == "src/editor/guide.rs" || relative.starts_with("src/editor/guide/")
}

/// The web-core modules that decide WHICH events a change raises (their pure half is tested
/// natively): the page asks them and raises the answers with a `raise(ctx, name)` loop that
/// names no event itself.
const CORE_EVENT_MODULES: &[&str] = &["guide_events"];

/// The code of the `CORE_EVENT_MODULES` files of this crate (their `#[cfg(test)]` part cut off),
/// as `(path relative to the crate, text)`.
fn core_event_modules() -> Vec<(String, String)> {
    sources(Path::new(env!("CARGO_MANIFEST_DIR")), "src", "rs")
        .into_iter()
        .filter(|(path, _)| {
            CORE_EVENT_MODULES
                .iter()
                .any(|module| *path == format!("src/{module}.rs"))
        })
        .map(|(path, text)| {
            let code = text.split("#[cfg(test)]").next().unwrap_or_default();
            (path, code.to_owned())
        })
        .collect()
}

/// Whether the Rust source `text` makes a raising call.
fn makes_raising_call(text: &str) -> bool {
    RAISE_CALLS.iter().any(|call| text.contains(call))
}

/// Whether `name` is raised: some Rust file outside the runner names it (its constant in
/// `UPPER_SNAKE_CASE` or the quoted name) and makes a raising call, or a Slint file calls
/// `.event("name")`, or a web-core event module (`core`) names it and a page file outside the
/// runner both uses a function of that module whose body names it and makes a raising call.
fn is_raised(name: &str, rust: &Files, slint: &Files, core: &Files) -> bool {
    let constant = name.to_ascii_uppercase();
    let quoted = format!("\"{name}\"");
    let slint_call = format!("event(\"{name}\")");
    let names = |text: &str| contains_word(text, &constant) || text.contains(&quoted);
    let in_rust = rust
        .iter()
        .any(|(path, text)| !is_runner_file(path) && names(text) && makes_raising_call(text));
    let through_core = core.iter().any(|(_, text)| {
        // The functions of the module whose body names the event, used by a raising page file.
        text.replace("pub const fn ", "pub fn ")
            .split("pub fn ")
            .skip(1)
            .filter(|chunk| names(chunk))
            .filter_map(|chunk| chunk.split('(').next())
            .any(|function| {
                rust.iter().any(|(page, source)| {
                    !is_runner_file(page)
                        && contains_word(source, function)
                        && makes_raising_call(source)
                })
            })
    });
    in_rust || through_core || slint.iter().any(|(_, text)| text.contains(&slint_call))
}

/// Whether some component compares the open step's highlight target with `target`.
fn is_drawn(target: &str, slint: &Files) -> bool {
    let kebab = format!("highlight-target == \"{target}\"");
    let snake = format!("highlight_target == \"{target}\"");
    slint
        .iter()
        .any(|(_, text)| text.contains(&kebab) || text.contains(&snake))
}

/// Every event the goals of the offered lessons wait for, and every target they outline, each
/// with the first lesson that needs it.
fn offered_hooks() -> (Needs, Needs) {
    let mut events = Needs::new();
    let mut targets = Needs::new();
    for guide in GuideCatalog::new().web_guides(WebFeatures::ALL) {
        for step in &guide.steps {
            for name in step.goal.events() {
                if !events.iter().any(|(seen, _)| seen == name) {
                    events.push((name.to_owned(), guide.id.clone()));
                }
            }
            let target = step.highlight_target.as_str();
            if !target.is_empty() && !targets.iter().any(|(seen, _)| seen == target) {
                targets.push((target.to_owned(), guide.id.clone()));
            }
        }
    }
    (events, targets)
}

#[test]
fn every_event_of_an_offered_lesson_is_raised_by_the_browser() {
    let Some(app) = web_app() else {
        return;
    };
    let rust = sources(&app, "src", "rs");
    let slint = sources(&app, "ui", "slint");
    let core = core_event_modules();
    assert_eq!(
        core.len(),
        CORE_EVENT_MODULES.len(),
        "a web-core event module is missing from src/"
    );
    let (events, _) = offered_hooks();
    assert!(
        events.len() > 1,
        "the offered lessons wait for no events at all: the catalogue read went wrong"
    );
    let missing: Vec<String> = events
        .iter()
        .filter(|(name, _)| !is_raised(name, &rust, &slint, &core))
        .map(|(name, lesson)| format!("{name} (first needed by {lesson})"))
        .collect();
    assert!(
        missing.is_empty(),
        "nothing in apps/pleochroma-web raises these events, so their steps never complete: call \
         `guide::raise(ctx, EVENT)` where the action succeeds, or `GuideModel.event(\"name\")` \
         from Slint:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn every_target_of_an_offered_lesson_is_drawn_by_the_browser() {
    let Some(app) = web_app() else {
        return;
    };
    let slint = sources(&app, "ui", "slint");
    let (_, targets) = offered_hooks();
    assert!(
        targets.len() > 1,
        "the offered lessons outline no targets at all: the catalogue read went wrong"
    );
    let missing: Vec<String> = targets
        .iter()
        .filter(|(target, _)| {
            !WEB_TEXT_ONLY_TARGETS.contains(&target.as_str()) && !is_drawn(target, &slint)
        })
        .map(|(target, lesson)| format!("{target} (first named by {lesson})"))
        .collect();
    assert!(
        missing.is_empty(),
        "no component of apps/pleochroma-web/ui draws an outline for these targets: compare \
         `GuideModel.highlight-target == \"name\"` on the control that does the job:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn every_target_of_an_offered_lesson_is_one_the_catalogue_knows() {
    let (_, targets) = offered_hooks();
    for (target, lesson) in &targets {
        assert!(
            HIGHLIGHT_TARGETS.contains(&target.as_str())
                || WEB_ONLY_TARGETS.contains(&target.as_str()),
            "{lesson} outlines {target}, which is neither a catalogue target nor a web-only one"
        );
        assert_ne!(
            target, "stone_tab",
            "{lesson} still outlines the desktop's Stone tab: the web view aliases it to design_settings"
        );
    }
}

#[test]
fn a_text_only_target_is_one_the_catalogue_knows() {
    for target in WEB_TEXT_ONLY_TARGETS {
        assert!(
            HIGHLIGHT_TARGETS.contains(target),
            "{target} is a text-only web target but not a catalogue target"
        );
    }
}

#[test]
fn the_tier_selection_raises_the_events_of_the_tier_lessons() {
    let Some(app) = web_app() else {
        return;
    };
    let rust = sources(&app, "src", "rs");
    let (_, selection) = rust
        .iter()
        .find(|(path, _)| path == "src/editor/selection.rs")
        .expect("src/editor/selection.rs");
    assert!(
        contains_word(selection, "TIERS_MULTI_SELECTED")
            && contains_word(selection, "TIER_SELECTED"),
        "selection.rs must raise TIERS_MULTI_SELECTED (a group of two or more) and TIER_SELECTED \
         (a row picked)"
    );
    assert!(
        selection.contains("report_group(ctx, count)"),
        "both the Ctrl+click toggle and the Shift+click range must report their group size"
    );
    let (events, _) = offered_hooks();
    assert!(
        events.iter().any(|(name, _)| name == TIERS_MULTI_SELECTED),
        "a lesson of the browser waits for the multi-select event"
    );
    assert_eq!(TIER_SELECTED, "tier_selected");
}

#[test]
fn the_helpers_find_what_they_look_for() {
    let slint = vec![
        (
            "ui/a.slint".to_owned(),
            "highlighted: GuideModel.highlight-target == \"solve_button\";".to_owned(),
        ),
        (
            "ui/b.slint".to_owned(),
            "GuideModel.event(\"shortcuts_opened\");".to_owned(),
        ),
    ];
    assert!(is_drawn("solve_button", &slint));
    assert!(!is_drawn("open_button", &slint));

    let rust = vec![
        (
            "src/app/a.rs".to_owned(),
            "guide::raise(ctx, TIER_SELECTED);".to_owned(),
        ),
        (
            "src/editor/guide/events.rs".to_owned(),
            "raise(ctx, SLICE_STARTED); // the runner".to_owned(),
        ),
        (
            "src/library/b.rs".to_owned(),
            "const NAME: &str = \"library_searched\"; notify(ctx, kind, text);".to_owned(),
        ),
    ];
    let core = vec![(
        "src/guide_events.rs".to_owned(),
        "pub fn library_events(a: &Q) { raised.push(viewing_events::LIBRARY_SEARCHED); }\n\
         pub const fn other(a: u32) { LIBRARY_FILTERED }"
            .to_owned(),
    )];
    assert!(is_raised("tier_selected", &rust, &slint, &[]));
    assert!(is_raised("shortcuts_opened", &rust, &slint, &[]));
    assert!(!is_raised("slice_started", &rust, &slint, &[]));
    assert!(!is_raised("library_searched", &rust, &slint, &[]));
    // Named by a web-core event module: raised only when a page file uses the function that
    // names it (another function of the module, or a different name, does not count).
    assert!(!is_raised("library_searched", &rust, &slint, &core));
    let mut with_caller = rust;
    with_caller.push((
        "src/library/mod.rs".to_owned(),
        "for name in guide_events::library_events(&a, &b) { raise(ctx, name); }".to_owned(),
    ));
    assert!(is_raised("library_searched", &with_caller, &slint, &core));
    // `library_filtered` is named only by `other`, which no page file uses.
    assert!(!is_raised("library_filtered", &with_caller, &slint, &core));

    assert!(contains_word("raise(ctx, TIER_SELECTED);", "TIER_SELECTED"));
    assert!(!contains_word(
        "raise(ctx, TIER_SELECTED_ALL);",
        "TIER_SELECTED"
    ));
    assert!(!contains_word(
        "raise(ctx, MY_TIER_SELECTED);",
        "TIER_SELECTED"
    ));
    assert!(is_runner_file("src/editor/guide.rs"));
    assert!(is_runner_file("src/editor/guide/judge.rs"));
    assert!(!is_runner_file("src/editor/guided.rs"));
}
