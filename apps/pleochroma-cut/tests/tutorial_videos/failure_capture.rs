//! Privacy-checked screenshots of a failed tutorial's visible state.

use crate::{
    config::{Config, first_private},
    routes,
    support::WindowView,
};
use pleochroma_cut::{MainWindow, harness_support};
use slint::ComponentHandle;
use std::fs;
use uiprobe::screenshot::{self, Shot};

/// One open window: its name in the file names and the log, what the errors call it, the view the
/// privacy scan reads and the call that captures it.
struct Open {
    name: &'static str,
    label: &'static str,
    view: WindowView,
    capture: Box<dyn Fn() -> Result<Shot, String>>,
}

/// `window` as an [`Open`] when there is one (the harness only hands out shown windows).
fn open<C: ComponentHandle + 'static>(
    name: &'static str,
    label: &'static str,
    type_name: &str,
    window: Option<C>,
) -> Option<Open> {
    let window = window?;
    Some(Open {
        name,
        label,
        view: WindowView::of(type_name, &window),
        capture: Box::new(move || screenshot::capture(window.window())),
    })
}

/// The main window and every secondary window of the app that is shown right now.
fn open_windows(ui: &MainWindow) -> Vec<Open> {
    [
        open("main", "main", "MainWindow", Some(ui.clone_strong())),
        open("help", "Help", "HelpWindow", harness_support::help_window()),
        open(
            "rough-planner",
            "Rough Planner",
            "RoughPlannerWindow",
            harness_support::rough_planner_window(),
        ),
        open(
            "rough-color",
            "Rough Color wizard",
            "RoughColourWindow",
            harness_support::rough_color_window(),
        ),
        open(
            "compare",
            "Compare",
            "CompareWindow",
            harness_support::compare_window(),
        ),
        open(
            "locate",
            "Locate Inclusion",
            "LocateWindow",
            harness_support::locate_window(),
        ),
        open(
            "rig",
            "Camera Rig",
            "RigWindow",
            harness_support::rig_window(),
        ),
        open(
            "color-pattern",
            "Color Pattern",
            "ColorPatternWindow",
            harness_support::color_pattern_window(),
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn check_window(words: &[String], name: &str, view: &WindowView) -> Result<(), String> {
    for text in routes::visible_texts(view) {
        if first_private(words, &text).is_some() {
            return Err(format!(
                "private text is visible in the {name} window; failure screenshots skipped"
            ));
        }
    }
    Ok(())
}

/// Captures the main window and any open tutorial secondary windows, named `main`, `help`,
/// `rough-planner`, `rough-color`, `compare`, `locate`, `rig` and `color-pattern`.
///
/// The privacy scan runs over every open window and the visible toast before any image is
/// captured. Errors deliberately omit paths and UI text; a private-text error says "private text".
pub fn capture_windows(ui: &MainWindow, cfg: &Config) -> Result<Vec<(String, Shot)>, String> {
    capture_windows_for(ui, &cfg.private_words)
}

/// [`capture_windows`] with the private words themselves (debug mode keeps those, not the
/// `Config`, so it can capture from inside an input call).
pub fn capture_windows_for(
    ui: &MainWindow,
    private_words: &[String],
) -> Result<Vec<(String, Shot)>, String> {
    let windows = open_windows(ui);
    for window in &windows {
        check_window(private_words, window.label, &window.view)?;
    }
    let toast = ui.get_toast_message();
    if ui.get_toast_visible() && first_private(private_words, toast.as_str()).is_some() {
        return Err(
            "private text is visible in the main-window toast; failure screenshots skipped".into(),
        );
    }

    // Capture all images in memory first, so a later privacy or capture failure cannot leave a
    // misleading partial diagnostic set on disk.
    let mut images = Vec::with_capacity(windows.len());
    for window in &windows {
        let shot = (window.capture)()
            .map_err(|_| format!("could not capture the {} window", window.label))?;
        images.push((window.name.to_owned(), shot));
    }
    Ok(images)
}

/// Captures the main window and any open tutorial secondary windows for a failed run.
///
/// The privacy scan runs over every open window and the visible toast before any image is
/// captured or any output directory is created. Errors deliberately omit paths and UI text.
pub fn save_failure(ui: &MainWindow, cfg: &Config, id: &str) -> Result<(), String> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(
            "failure screenshot id must contain only ASCII letters, digits, and hyphens".into(),
        );
    }

    let images = capture_windows(ui, cfg)?;
    let directory = cfg.out_dir.join("failures");
    fs::create_dir_all(&directory)
        .map_err(|_| "could not create the failure screenshot folder".to_owned())?;
    for (window, image) in images {
        let path = directory.join(format!("{id}-{window}.png"));
        image
            .save_png(&path)
            .map_err(|_| format!("could not write the {window} failure screenshot"))?;
    }
    Ok(())
}
