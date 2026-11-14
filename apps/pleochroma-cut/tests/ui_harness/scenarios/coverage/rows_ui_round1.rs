//! Direct-invoke rows for the callbacks the UI redesign (round 1) added.
//!
//! The viewport's gestures and figure folds, the HDR map's choose and clear, the Stone tab's fold
//! bits, the Render tab's quality presets, Cutting Instructions' row helpers and the Files page's
//! Open Exports Folder. Every row works on the first fixture design in the editor and is safe to
//! call with empty arguments: the settings callbacks write the throw-away profile, the pickers get
//! the cancels `run_row` queues, the openers are captured (`harness_support`).
//!
//! `VerdictModel.open_pulse` is a property, not a callback: the Design Checks chip bumps it, and
//! `states_popups.rs` presses that chip.

use super::table::{Invoke, Row, Setup};
use pleochroma_cut::*;
use slint::ComponentHandle;

/// Editor setup, Main window, shared state.
const ROUND1_EDITOR: &[Row] = rows!(MainWindow, Main, Editor, false;
    CuttingTableModel.invoke_copy_row_of(slint::ModelRc::default(), Default::default());
    CuttingTableModel.invoke_copy_rows(slint::ModelRc::default());
    CuttingTableModel.invoke_count_rows(slint::ModelRc::default(), Default::default());
    CuttingTableModel.invoke_open_tier(slint::SharedString::default());
    CuttingTableModel.invoke_split_indices(slint::SharedString::default());
    ExportModel.invoke_open_exports_folder(slint::SharedString::default());
    ParametersModel.invoke_stone_fold_set(Default::default(), Default::default());
    SettingsModel.invoke_env_map_status_changed(slint::SharedString::default());
    SettingsModel.invoke_quality_preset_selected(Default::default());
    ViewportModel.invoke_env_map_choose();
    ViewportModel.invoke_env_map_clear();
    ViewportModel.invoke_figures_folded_set(Default::default());
    ViewportModel.invoke_gestures_seen_set(Default::default());
);

/// Every row of this file, in run order.
pub fn rows() -> Vec<&'static Row> {
    ROUND1_EDITOR.iter().collect()
}
