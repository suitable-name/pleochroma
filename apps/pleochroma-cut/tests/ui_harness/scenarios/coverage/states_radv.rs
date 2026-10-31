//! The RADV shader workaround's UI (lane RW of the 2026-10-28 campaign): the one-time question and
//! the Preferences "Graphics Driver" section.
//!
//! Neither is reachable in a harness run by its normal route. The question appears when the GPU
//! adapter turns out to be Mesa's RADV driver and the user was never asked (the adapter watch of
//! `gui::radv_workaround` is off under the `ui-harness` feature: a harness run must never depend
//! on the GPU of the machine it runs on), and the Preferences section exists on Linux only (a
//! Windows run shows no section). The states below therefore raise both the way Rust would:
//! `RadvWorkaroundModel.prompt_pending` with a driver text, and `available`.
//!
//! The restart is faked: in the harness build `request_restart` only records the request
//! (`harness_support::take_radv_restart_request`), it never closes the window and never starts a
//! process.
//!
//! Proves:
//! * click-through states (every control pressed once): the question's four ways out and the
//!   Preferences switch and "Restart Now";
//! * the `radv_workaround_question` scenario: "Not Now" stores nothing, "Don't Use It" stores
//!   `off`, "Restart With Workaround" stores `on` and asks for the restart, Escape is "Not Now".

use super::{
    super::{Piece, dialogs::click_button, existing_install},
    overlay_states::{Base, NONE, State},
    state_pieces,
    states_dialogs::scroll_until_visible,
};
use crate::support::{
    Ctx, HarnessApp,
    app::{pump, wait_until},
    coverage,
    smoke::{DenyMatch, DenyRule},
};
use pleochroma_cut::{PreferencesModel, RadvWorkaroundModel, harness_support};
use slint::ComponentHandle;
use std::time::Duration;

/// The driver text Rust would hand the question for the owner's server.
const DRIVER_TEXT: &str = "Mesa 26.2.5 (RADV)";

/// The settings key the answer is stored under.
const KEY: &str = "radv_scheduler_workaround";

/// The pieces of this file: the two click-through states and the question scenario.
pub fn pieces() -> Vec<Piece> {
    let mut all = state_pieces("radv-workaround", existing_install, radv_states);
    all.push(Piece::new(
        "radv_workaround_question",
        existing_install,
        question,
    ));
    all
}

/// Preferences: switching the update check on is a GitHub request.
const DENY_PREFERENCES: &[DenyRule] = &[DenyRule::label(
    DenyMatch::Contains,
    "check for new versions",
    "network: switching it on starts an update check (UpdateModel.enabled_changed)",
)];

fn radv_states() -> Vec<State> {
    vec![
        State {
            name: "RADV Shader Workaround question",
            base: Base::Startup,
            scope: Some("ConfirmActionDialog"),
            deny: NONE,
            open: raise_question,
        },
        State {
            name: "Preferences, graphics driver section",
            base: Base::Startup,
            scope: Some("PreferencesDialog"),
            deny: DENY_PREFERENCES,
            open: |a| {
                // The section sits between "Updates" and "Tutorials", below the card's fold.
                a.ui().global::<RadvWorkaroundModel>().set_available(true);
                a.ui().global::<PreferencesModel>().set_dialog_open(true);
                pump(250);
                scroll_until_visible(a, "PreferencesDialog", "Restart Now");
            },
        },
    ]
}

/// Raises the question the way the adapter watch does.
fn raise_question(a: &HarnessApp) {
    let model = a.ui().global::<RadvWorkaroundModel>();
    model.set_driver_text(DRIVER_TEXT.into());
    model.set_prompt_pending(true);
    pump(250);
}

/// The settings file as the app last wrote it ("" when there is none).
fn settings_text() -> String {
    std::fs::read_to_string(harness_support::settings_path()).unwrap_or_default()
}

/// Waits (real time, the persister's debounce is 600 ms) until the settings file holds `needle`.
fn file_holds(needle: &str) -> bool {
    wait_until(Duration::from_secs(8), || settings_text().contains(needle))
}

/// The question's four ways out, each checked for what it stores and what it asks for.
fn question(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    let model = ui.global::<RadvWorkaroundModel>();

    // Not Now: stores nothing, asks for nothing.
    raise_question(ctx.app);
    let shown = ctx.app.main_view().has("Restart With Workaround");
    ctx.expect(
        shown,
        "radv-shown",
        "the question is not on screen after prompt_pending was raised",
    );
    if !shown {
        return;
    }
    ctx.expect(
        ctx.app.main_view().has("Not Now") && ctx.app.main_view().has("Don't Use It"),
        "radv-buttons",
        "the question lacks its Not Now or Don't Use It button",
    );
    ctx.sweep_main_at_sizes("RADV Shader Workaround question");
    let _ = click_button(&ctx.app.main_view(), &["Not Now"]);
    pump(300);
    ctx.expect(
        !model.get_prompt_pending() && !ctx.app.main_view().has("Restart With Workaround"),
        "radv-not-now-closes",
        "the question is still on screen after Not Now",
    );
    ctx.expect(
        !settings_text().contains(KEY) && !model.get_enabled(),
        "radv-not-now-stores-nothing",
        "Not Now stored an answer (it must be asked again at the next start)",
    );
    ctx.expect(
        !harness_support::take_radv_restart_request(),
        "radv-not-now-no-restart",
        "Not Now asked for a restart",
    );
    coverage::record_callback("RadvWorkaroundModel.not_now");

    // Escape is Not Now too.
    raise_question(ctx.app);
    ctx.press("Escape");
    pump(300);
    ctx.expect(
        !model.get_prompt_pending() && !settings_text().contains(KEY),
        "radv-escape",
        "Escape did not close the question without storing an answer",
    );

    // Don't Use It: stores off, never asks again, no restart.
    raise_question(ctx.app);
    let _ = click_button(&ctx.app.main_view(), &["Don't Use It"]);
    pump(300);
    ctx.expect(
        !model.get_prompt_pending() && !model.get_enabled(),
        "radv-dont-use-closes",
        "the question is still on screen, or the switch is on, after Don't Use It",
    );
    ctx.expect(
        file_holds(&format!("{KEY} = \"off\"")),
        "radv-dont-use-stores-off",
        format!("Don't Use It did not store `{KEY} = \"off\"` in the settings file"),
    );
    ctx.expect(
        !harness_support::take_radv_restart_request(),
        "radv-dont-use-no-restart",
        "Don't Use It asked for a restart",
    );
    coverage::record_callback("RadvWorkaroundModel.dont_use");

    // Restart With Workaround: stores on, asks for the restart (faked: only recorded).
    raise_question(ctx.app);
    let _ = click_button(&ctx.app.main_view(), &["Restart With Workaround"]);
    pump(300);
    ctx.expect(
        !model.get_prompt_pending() && model.get_enabled(),
        "radv-restart-closes",
        "the question is still on screen, or the switch is off, after Restart With Workaround",
    );
    ctx.expect(
        file_holds(&format!("{KEY} = \"on\"")),
        "radv-restart-stores-on",
        format!("Restart With Workaround did not store `{KEY} = \"on\"` in the settings file"),
    );
    ctx.expect(
        harness_support::take_radv_restart_request(),
        "radv-restart-requested",
        "Restart With Workaround did not ask for the restart",
    );
    ctx.expect(
        ctx.app.is_open(),
        "radv-window-stays",
        "the harness build closed the main window for the faked restart",
    );
    coverage::record_callback("RadvWorkaroundModel.restart_with_workaround");
    ctx.check_health("RADV Shader Workaround question");
}
