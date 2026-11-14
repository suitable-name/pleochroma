//! The direct-invoke row of the startup shader cache card's callback (`ShaderCacheModel`).
//!
//! The row is skipped when the "Shader cache card" state (`overlay_states.rs`) already pressed
//! Hide. Under the harness seams the build never starts, so no Rust handler is installed and the
//! call only proves that invoking it is safe.

use super::table::{Invoke, Row, Setup};
use pleochroma_cut::*;
use slint::ComponentHandle;

/// Startup setup, Main window, shared state.
const SHADER_CACHE_MAIN_SHARED: &[Row] = rows!(MainWindow, Main, Startup, false;
    ShaderCacheModel.invoke_hide();
);

/// Every row of this file, in run order.
pub fn rows() -> Vec<&'static Row> {
    SHADER_CACHE_MAIN_SHARED.iter().collect()
}
