//! Core state -> Slint properties.
//!
//! One module per surface (shell, preferences, about, developer, toast). Each `push` function
//! only writes properties; none of them read the UI back.

use pleochroma_mobile_core::app_state::AppState;

use crate::AppWindow;

pub mod about;
pub mod developer;
pub mod prefs;
pub mod shell;
pub mod toast;

/// Pushes every surface from `state`. Called once after the callbacks are wired.
pub fn all(window: &AppWindow, state: &AppState) {
    shell::push(window, state);
    prefs::push(window, &state.settings);
    about::push(window, state);
    developer::push(window, state);
    crate::design::push_all(window, state);
}
