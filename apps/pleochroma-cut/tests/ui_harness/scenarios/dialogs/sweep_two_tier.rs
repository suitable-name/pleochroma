//! Scenario: the Angle Sweep dialog's Two Tiers mode. The dialog opens by its normal route,
//! the mode pill switches it to Two Tiers, the mode sweeps clean at both sizes, and Esc closes
//! the dialog, handing the keyboard back to the main window. No sweep is run (it solves every
//! angle pair); the heat map's own checks are in the unit tests of `gui::editor::sweep`.

use super::{click_button, create_template_design, labels_like};
use crate::support::{Ctx, app::pump};
use pleochroma_cut::SweepModel;
use slint::{ComponentHandle, Model};

/// With a template design in the editor: opens the Angle Sweep dialog, switches it to Two
/// Tiers with the pill, sweeps, and closes it with Esc.
pub(super) fn dialogs_sweep_two_tiers(ctx: &mut Ctx<'_>) {
    if !create_template_design(ctx) {
        return;
    }
    ctx.check_health("template design for the two-tier sweep");
    let ui = ctx.app.ui().clone_strong();
    let sweep = ui.global::<SweepModel>();
    sweep.invoke_open_dialog();
    pump(250);
    let open = sweep.get_dialog_open();
    if !ctx.expect(
        open,
        "overlay-opens",
        "Angle Sweep: its open flag is false after the normal open route",
    ) {
        return;
    }

    let view = ctx.app.main_view();
    let clicked = click_button(&view, &["Two Tiers"]);
    pump(250);
    if !ctx.expect(
        clicked.is_some() && sweep.get_mode() == 1,
        "sweep-two-tiers",
        format!(
            "the Two Tiers pill did not switch the dialog to Two Tiers; similar labels: {:?}",
            labels_like(&ctx.app.main_view(), &["tier", "sweep"])
        ),
    ) {
        sweep.set_dialog_open(false);
        return;
    }
    // A design with two tiers to sweep gives a form that can run.
    if sweep.get_tier_names().row_count() >= 2 {
        ctx.expect(
            sweep.get_form_ok_2d(),
            "sweep-two-tiers-form",
            format!(
                "the Two Tiers form cannot run on the starting ranges: {}",
                sweep.get_cells_summary()
            ),
        );
    }
    ctx.sweep_main_at_sizes("Angle Sweep, Two Tiers");

    // Back to One Tier and to Two Tiers again with the pills: the dialog stays sound.
    click_button(&ctx.app.main_view(), &["One Tier"]);
    pump(250);
    ctx.expect(
        sweep.get_mode() == 0,
        "sweep-one-tier",
        "the One Tier pill did not switch the dialog back to One Tier",
    );
    click_button(&ctx.app.main_view(), &["Two Tiers"]);
    pump(250);
    ctx.check_health("Angle Sweep switched back and forth");

    ctx.press("Escape");
    pump(200);
    let closed = !sweep.get_dialog_open();
    ctx.expect(
        closed,
        "overlay-closes",
        "Angle Sweep (Two Tiers): still open after closing it with Esc",
    );
    if !closed {
        sweep.set_dialog_open(false);
        pump(150);
    }
    ctx.check_health("Angle Sweep (Two Tiers) closed with Esc");
}
