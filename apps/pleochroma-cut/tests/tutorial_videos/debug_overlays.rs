//! Debug mode's "during" screenshots: which dialogs, popups, overlays and secondary windows are up.
//!
//! A step often opens a dialog, an overlay or a window, uses it and closes it again before its
//! "after" screenshots are taken, so nothing in the folder would show it. `debug_log` therefore
//! looks at this table before every input and after every longer hold; while anything in the table
//! is up it keeps a screenshot of the latest state in memory, and it writes that screenshot out as
//! `NN-during-<window>.png` when the table's answer changes (the thing was closed) or the step
//! ends. The table only reads flags the app already keeps; it changes nothing.

use pleochroma_cut::{
    BatchModel, CommandPaletteModel, CuttingModeModel, EditorModel, HelpModel, LibraryModel,
    MainWindow, PreferencesModel, RawTextModel, RenderJobsModel, RetargetModel, ShortcutsModel,
    SweepModel, TiltModel, ViewportModel,
    harness_support::{self, SecondaryWindow},
};
use slint::ComponentHandle;
use uiprobe::screenshot::Shot;

/// One thing that can be up and how to tell.
struct Probe {
    /// What the log calls it.
    name: &'static str,
    /// Whether it is up.
    is_up: fn(&MainWindow) -> bool,
}

/// The overlays of the main window (the open flags the UI harness drives) and the secondary
/// windows. The tutorial browser is left out on purpose: every video opens and closes it in its
/// setup, and its steps are not what the debug folder is for.
const PROBES: &[Probe] = &[
    Probe {
        name: "Command Palette",
        is_up: |ui| ui.global::<CommandPaletteModel>().get_open(),
    },
    Probe {
        name: "Shortcuts",
        is_up: |ui| ui.global::<ShortcutsModel>().get_open(),
    },
    Probe {
        name: "Preferences",
        is_up: |ui| ui.global::<PreferencesModel>().get_dialog_open(),
    },
    Probe {
        name: "Cutting Mode",
        is_up: |ui| ui.global::<CuttingModeModel>().get_open(),
    },
    Probe {
        name: "Glossary",
        is_up: |ui| ui.global::<HelpModel>().get_glossary_open(),
    },
    Probe {
        name: "Render Jobs",
        is_up: |ui| ui.global::<RenderJobsModel>().get_panel_open(),
    },
    Probe {
        name: "Angle Sweep",
        is_up: |ui| ui.global::<SweepModel>().get_dialog_open(),
    },
    Probe {
        name: "Edit as Text",
        is_up: |ui| ui.global::<RawTextModel>().get_is_open(),
    },
    Probe {
        name: "New Design",
        is_up: |ui| ui.global::<EditorModel>().get_new_dialog_open(),
    },
    Probe {
        name: "Gear Remap",
        is_up: |ui| ui.global::<EditorModel>().get_gear_remap_open(),
    },
    Probe {
        name: "Anchor Explainer",
        is_up: |ui| ui.global::<EditorModel>().get_anchor_explainer_open(),
    },
    Probe {
        name: "Retarget",
        is_up: |ui| ui.global::<RetargetModel>().get_is_open(),
    },
    Probe {
        name: "Material Editor",
        is_up: |ui| ui.global::<ViewportModel>().get_material_editor_open(),
    },
    Probe {
        name: "Performance Graph",
        is_up: |ui| ui.global::<TiltModel>().get_dialog_open(),
    },
    Probe {
        name: "Metadata Editor",
        is_up: |ui| ui.global::<LibraryModel>().get_metadata_editor_open(),
    },
    Probe {
        name: "Preview Batch",
        is_up: |ui| ui.global::<BatchModel>().get_preview_visible(),
    },
    Probe {
        name: "Tilt Batch",
        is_up: |ui| ui.global::<BatchModel>().get_tilt_visible(),
    },
    Probe {
        name: "Help Window",
        is_up: |_| harness_support::is_window_open(SecondaryWindow::Help),
    },
    Probe {
        name: "Rough Planner Window",
        is_up: |_| harness_support::is_window_open(SecondaryWindow::RoughPlanner),
    },
    Probe {
        name: "Rough Color Wizard",
        is_up: |_| harness_support::is_window_open(SecondaryWindow::RoughColor),
    },
    Probe {
        name: "Compare Window",
        is_up: |_| harness_support::is_window_open(SecondaryWindow::Compare),
    },
    Probe {
        name: "Locate Window",
        is_up: |_| harness_support::is_window_open(SecondaryWindow::Locate),
    },
    Probe {
        name: "Camera Rig Window",
        is_up: |_| harness_support::is_window_open(SecondaryWindow::Rig),
    },
    Probe {
        name: "Color Pattern Window",
        is_up: |_| harness_support::is_window_open(SecondaryWindow::ColorPattern),
    },
];

/// The names of everything in the table that is up now, in table order.
#[must_use]
pub fn up_now(ui: &MainWindow) -> Vec<&'static str> {
    PROBES
        .iter()
        .filter(|probe| (probe.is_up)(ui))
        .map(|probe| probe.name)
        .collect()
}

/// The latest screenshots taken while `up` was up, waiting to be written when it goes away.
pub struct Pending {
    /// What was up when the pictures were taken.
    pub up: Vec<&'static str>,
    /// Every open window, named as in the file names.
    pub shots: Vec<(String, Shot)>,
}
