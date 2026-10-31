//! The Cutting Mode screen: wires `CuttingModeModel` to the model in the parent module.
//!
//! Slint owns whether the screen is open and what the keys do (`ui/models/cutting_mode_model.slint`);
//! this module answers its callbacks. It opens the model for the design, pushes each page
//! ([`super::push`]), shows the stone after the step through the Solid view
//! (`crate::views::show_cut_step`), stores the marks (through the model) and rebuilds the steps
//! when the design changes under the open screen.
//!
//! # The poll
//!
//! One timer runs for the page's lifetime. Every tick it refreshes the entry points' disabled
//! reason (`CuttingModeModel.unavailable-reason`); while the screen is open it also compares
//! the model with the design ([`super::check`]) and rebuilds the steps when the design changed
//! (an undo or a dropped file under the screen), saying so in the notice line.
//!
//! # The views
//!
//! Opening takes the Solid view ([`crate::views::hold_for_cutting`]): the Solid tab, the camera
//! fitted to the rough, nothing selected. Closing, by any route, gives it back.

use super::{
    CuttingMode, DESIGN_CHANGED_MESSAGE, Freshness, MARKS_LIVE_HERE, MarkOutcome, Unavailable,
    availability, check, close, open, push, restart, with_mode,
};
use crate::{AppModel, AppWindow, CuttingModeModel, app::Ctx, editor::guide, views};
use pleochroma_editor::guide::viewing_events::CUTTING_STEP_MARKED;
use pleochroma_solid::cut_position::CutPosition;
use slint::{ComponentHandle, Timer, TimerMode};
use std::{
    cell::{Cell, RefCell},
    time::Duration,
};

/// How often the poll runs.
const WATCH_INTERVAL: Duration = Duration::from_millis(400);

/// The notice when the browser refuses to store the marks.
const MARKS_NOT_STORED: &str =
    "This browser would not store your marks, so they last only until this page closes.";

/// The notice when the last step was just marked done.
const ALL_DONE: &str = "Every step is marked done.";

thread_local! {
    /// The views as they were before the screen took them; `Some` while it holds them.
    static HELD: RefCell<Option<views::HeldView>> = const { RefCell::new(None) };
    /// Counts the opens and closes, so a late answer for an older one is dropped.
    static TICKET: Cell<u64> = const { Cell::new(0) };
    /// The poll (kept alive for the page's lifetime).
    static WATCH: Timer = Timer::default();
}

/// Starts a new generation of the screen's work and returns its ticket.
fn next_ticket() -> u64 {
    TICKET.with(|cell| {
        let ticket = cell.get().wrapping_add(1);
        cell.set(ticket);
        ticket
    })
}

/// Whether `ticket` is still the current one.
fn is_current(ticket: u64) -> bool {
    TICKET.with(Cell::get) == ticket
}

/// Wires every `CuttingModeModel` callback and starts the poll. Call once, from
/// `app::callbacks::wire`.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<CuttingModeModel>();
    model.set_marks_note(MARKS_LIVE_HERE.into());
    let c = ctx.clone();
    model.on_open_requested(move || open_screen(&c));
    let c = ctx.clone();
    model.on_closed(move || closed(&c));
    let c = ctx.clone();
    model.on_previous(move || step(&c, CuttingMode::previous));
    let c = ctx.clone();
    model.on_next(move || step(&c, CuttingMode::next));
    let c = ctx.clone();
    model.on_jump_to(move |target| {
        if let Ok(target) = usize::try_from(target) {
            step(&c, |mode| mode.jump(target));
        }
    });
    let c = ctx.clone();
    model.on_toggle_done(move || mark(&c, CuttingMode::toggle_done, false));
    let c = ctx.clone();
    model.on_mark_done_and_next(move || mark(&c, CuttingMode::mark_done, true));
    let c = ctx.clone();
    model.on_chip_clicked(move |chip| {
        if let Ok(chip) = usize::try_from(chip) {
            mark(&c, |mode| mode.toggle_chip(chip), false);
        }
    });
    let c = ctx.clone();
    model.on_reset_confirmed(move || reset(&c));

    let poll = ctx.clone();
    WATCH.with(|timer| {
        timer.start(TimerMode::Repeated, WATCH_INTERVAL, move || tick(&poll));
    });
}

/// Fired once the screen has opened (busy): checks the design, takes the views, builds the
/// steps and shows the first page, or says why there is none.
fn open_screen(ctx: &Ctx) {
    let ticket = next_ticket();
    let available = availability(&ctx.state.borrow());
    if let Err(unavailable) = available {
        show_error(ctx, &unavailable);
        return;
    }
    hold_views(ctx);
    open(ctx, None, move |ctx, result| {
        if !is_current(ticket) {
            drop_if_closed(ctx);
            return;
        }
        match result {
            Ok(()) => show_first_page(ctx, ""),
            Err(unavailable) => show_error(ctx, &unavailable),
        }
    });
}

/// The design changed under the open screen: builds the steps again and says so.
fn restart_screen(ctx: &Ctx) {
    let ticket = next_ticket();
    if let Some(ui) = ctx.ui.upgrade() {
        // Busy: the keys do nothing until the new steps are in (`handle-key`).
        ui.global::<CuttingModeModel>().set_busy(true);
    }
    restart(ctx, move |ctx, result| {
        if !is_current(ticket) {
            drop_if_closed(ctx);
            return;
        }
        match result {
            Ok(()) => show_first_page(ctx, DESIGN_CHANGED_MESSAGE),
            Err(unavailable) => show_error(ctx, &unavailable),
        }
    });
}

/// Drops a model a late answer built for a screen that has been closed since.
fn drop_if_closed(ctx: &Ctx) {
    let open = ctx
        .ui
        .upgrade()
        .is_some_and(|ui| ui.global::<CuttingModeModel>().get_open());
    if !open {
        close();
    }
}

/// Clears busy and the error, shows `notice`, and pushes the current page and its stone.
fn show_first_page(ctx: &Ctx, notice: &str) {
    if let Some(ui) = ctx.ui.upgrade() {
        let model = ui.global::<CuttingModeModel>();
        model.set_error("".into());
        model.set_notice(notice.into());
        model.set_confirm_reset(false);
        model.set_busy(false);
    }
    present(ctx);
}

/// Shows why there is nothing to step through, in place of a page.
fn show_error(ctx: &Ctx, unavailable: &Unavailable) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    push::clear_page(&ui);
    let model = ui.global::<CuttingModeModel>();
    model.set_confirm_reset(false);
    model.set_error(unavailable.reason().into());
    model.set_busy(false);
}

/// Takes the Solid view for the stone pictures, unless the screen already holds it.
fn hold_views(ctx: &Ctx) {
    if HELD.with(|held| held.borrow().is_some()) {
        return;
    }
    let held = views::hold_for_cutting(ctx);
    HELD.with(|cell| *cell.borrow_mut() = held);
}

/// Fired after the screen closed, however it was closed: drops the model, empties the page and
/// gives the views back.
fn closed(ctx: &Ctx) {
    next_ticket();
    close();
    let held = HELD.with(|cell| cell.borrow_mut().take());
    if let Some(held) = held {
        views::release_for_cutting(ctx, &held);
    }
    if let Some(ui) = ctx.ui.upgrade() {
        push::clear_page(&ui);
        let model = ui.global::<CuttingModeModel>();
        model.set_error("".into());
        model.set_notice("".into());
    }
}

/// Pushes the open model's current page and shows the stone after its step.
fn present(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let name = ui.global::<AppModel>().get_design_name();
    let shown = with_mode(|mode| {
        push::push_page(&ui, mode, name.as_str());
        (
            CutPosition::from_steps(mode.cut_steps(), mode.step_count()).steps(),
            mode.table_row(),
        )
    });
    if let Some((cut_steps, tier_row)) = shown {
        views::show_cut_step(ctx, cut_steps, tier_row);
    }
}

/// Runs a move on the open model and shows the page it lands on.
fn step(ctx: &Ctx, go: impl FnOnce(&mut CuttingMode) -> bool) {
    if with_mode(go).unwrap_or(false) {
        present(ctx);
    }
}

/// Runs a change of the marks (then, for Space or D, a move to the next step) and shows the
/// result.
fn mark(ctx: &Ctx, change: impl FnOnce(&mut CuttingMode) -> MarkOutcome, advance: bool) {
    let result = with_mode(|mode| {
        let outcome = change(mode);
        let moved = advance && mode.next();
        (outcome, moved, mode.done_count() == mode.step_count())
    });
    let Some((outcome, moved, all_done)) = result else {
        return;
    };
    if outcome.changed || moved {
        present(ctx);
    }
    report(ctx, outcome, all_done);
    // A tutorial step may wait for a step to be marked done. Raised once the page shows the
    // new state, and only for a mark that raised the number of done steps.
    if outcome.newly_done {
        guide::raise(ctx, CUTTING_STEP_MARKED);
    }
}

/// Tells the user what the storage did with a change of the marks.
///
/// A refused write is said once and stays until a write works again.
fn report(ctx: &Ctx, outcome: MarkOutcome, all_done: bool) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<CuttingModeModel>();
    if !outcome.saved {
        model.set_notice(MARKS_NOT_STORED.into());
    } else if outcome.newly_done && all_done {
        model.set_notice(ALL_DONE.into());
    } else if outcome.changed && model.get_notice().as_str() == MARKS_NOT_STORED {
        model.set_notice("".into());
    }
}

/// The answer yes to "Reset progress".
fn reset(ctx: &Ctx) {
    let outcome = with_mode(CuttingMode::reset);
    present(ctx);
    if let Some(outcome) = outcome {
        report(ctx, outcome, false);
    }
}

/// The poll: the entry points' reason, and the open screen's freshness.
fn tick(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<CuttingModeModel>();
    // A writer holds the state: the next tick looks again.
    if let Ok(app) = ctx.state.try_borrow() {
        let reason = availability(&app)
            .err()
            .map(|unavailable| unavailable.reason())
            .unwrap_or_default();
        drop(app);
        model.set_unavailable_reason(reason.into());
    }
    if !model.get_open() || model.get_busy() {
        return;
    }
    match check(ctx) {
        Freshness::Stale => restart_screen(ctx),
        // The marks were re-filed under the design's new id; show what is stored now.
        Freshness::Rebound => present(ctx),
        Freshness::Closed | Freshness::Fresh => {}
    }
}
