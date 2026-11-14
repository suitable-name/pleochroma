//! The Android Back commit (API 33+, from `BackNavigationController` through JNI).
//!
//! It calls `Navigation.commit` directly. It used to dispatch an Escape key and let
//! `AppWindow`'s root `FocusScope` commit, but a key only reaches the item that has keyboard
//! focus: once a text field, choice field or stepper had taken focus and its page was hidden,
//! Slint dropped the focus altogether (`WindowInner::process_key_input`), every later Escape
//! went nowhere, and Back never did anything again. The app has no `PopupWindow` that Escape
//! would have to close first. `commit` checks the target and the version and advances the
//! version itself, so a duplicate queued callback for the same gesture is still rejected.
use crate::{AppWindow, Navigation};
use slint::ComponentHandle;

pub fn back(window: &AppWindow, target: i32, version: i32) {
    let navigation = window.global::<Navigation>();
    if target == 0 || navigation.get_target() != target || navigation.get_version() != version {
        tracing::debug!(
            target,
            version,
            current_target = navigation.get_target(),
            current_version = navigation.get_version(),
            "stale Back commit ignored"
        );
        return;
    }
    if !navigation.invoke_commit(target, version) {
        tracing::warn!(target, version, "Back commit refused");
    }
}
