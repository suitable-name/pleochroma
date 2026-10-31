//! Deep Solve (the desktop's `solve_actions::{deep_solve_run,deep_solve_pin}` and
//! `deep_solve.rs`): checks the stone against the printed proportions of its published
//! source, in the solve Worker, and shows the verdict with the tiers whose mast it moved.
//!
//! # Flow
//!
//! Run sends the design (native TOML) and the five printed figures (they are not part of
//! the TOML) to the solve Worker as `SolveRequest::DeepSolve`. The Worker answers with the
//! verified masts, the plain solve of the same design and the verdict, and streams a status
//! line with the phase and the elapsed time meanwhile (no fraction: the search has no honest
//! one). The shared text (`pleochroma_editor::deep_solve`) turns the answer into the
//! verdict line and the per-tier rows (`pleochroma_web_core::deep_summary`).
//!
//! A Deep Solve is a read-only check: it never edits the design. Each row has a Pin button,
//! the cutter's own opt-in, which pins that one tier to its verified mast through
//! `EditorSession::pin_tier_mast` (one undoable edit) while the result still describes the
//! design.
//!
//! # What differs from the desktop
//!
//! - The Worker serves solves and the other searches too, so the rules are Optimize's: an
//!   edit's auto-solve (or an Optimize or Retarget request) supersedes a running Deep Solve,
//!   which then reports that it stopped, and Run waits for a running solve, Optimize or
//!   Retarget. The Optimize button waits for a running Deep Solve in return.
//! - Cancel is two-stage, as Optimize's: the first press revokes the job's cancel URL and the
//!   Worker answers "cancelled" at its next checkpoint (a search has no partial result to
//!   keep); "Stop now" (a second press), or a Worker that does not answer in time, terminates
//!   the Worker.
//! - There is no activity list.
//! - The lessons' two events are raised as on the desktop: `deep_solve_started` once the search
//!   has been sent to the Worker, `deep_solve_finished` once a verdict is on screen (also one
//!   that is already stale, or not accepted); a cancelled, replaced or failed run raises
//!   neither.
//! - The verdict line does not name the file the printed figures came from, nor warn about
//!   edits since load (the desktop's `format_verification_targets` and
//!   `edited_since_load_caveat` are desktop-local text).

use crate::{
    AppWindow, DeepSolveModel, DeepSolveRow, InspectorModel, OptimizeModel, RetargetModel,
    app::{
        Ctx, coalesce_now,
        push::{MessageKind, show_message},
        solve::{auto_solve, flush_auto_solve, is_solving},
    },
    editor::{edit::Dirty, guide::raise, inspector::finish},
    workers,
};
use pleochroma_cut_core::Design;
use pleochroma_editor::{
    deep_solve::availability,
    guide::solving_events::{DEEP_SOLVE_FINISHED, DEEP_SOLVE_STARTED},
};
use pleochroma_web_core::{
    deep_summary::{PinCheck, Summary, failure_status, pin_check, summarize},
    solve::{
        DeepSolveParams, DeepSolveResultData, PrintedTargetsData, SolveRequest, SolveResponse,
        design_to_toml,
    },
    solve_error::SolveError,
};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::{cell::RefCell, time::Duration};

/// How often the availability and the staleness of a shown result are re-read.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The result on screen, for Pin and the stale badge.
struct Shown {
    /// The design generation the result describes: the one the run started at, moved
    /// forward by each of this result's own pins so the other rows stay pinnable.
    generation: u64,
    /// `(tier index, verified mast)` for every row.
    masts: Vec<(usize, f64)>,
}

#[derive(Default)]
struct Runtime {
    /// Bumped by every Run and by the hard cancel; an answer of an older run is ignored.
    epoch: u64,
    shown: Option<Shown>,
}

thread_local! {
    static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::default());
    static POLL: Timer = Timer::default();
}

/// What Run captured from the app.
struct Start {
    toml: String,
    design: Design,
    generation: u64,
    targets: PrintedTargetsData,
}

fn capture(ctx: &Ctx) -> Result<Start, String> {
    let app = ctx.state.borrow();
    let design_state = app.design.as_ref().ok_or("No design loaded.")?;
    let design = design_state.session.design.clone();
    let printed = design_state.printed_proportions.as_ref();
    let (available, hint) = availability(&design, printed);
    let Some(printed) = printed.filter(|_| available) else {
        return Err(hint);
    };
    Ok(Start {
        toml: design_to_toml(&design)?,
        targets: PrintedTargetsData::from(printed),
        design,
        generation: design_state.session.current_generation(),
    })
}

/// Run: sends the check to the Worker.
fn run(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DeepSolveModel>();
    if model.get_running() {
        return;
    }
    if ui.global::<OptimizeModel>().get_running() || ui.global::<RetargetModel>().get_is_busy() {
        show_message(
            ctx,
            MessageKind::Info,
            "Optimize or Retarget is still running -- start Deep Solve when it has finished.",
        );
        return;
    }
    // An edit's auto-solve may still be waiting out its debounce: start it now, so the
    // check below refuses instead of the solve replacing the Deep Solve once it fires.
    flush_auto_solve(ctx);
    if is_solving(ctx) {
        show_message(
            ctx,
            MessageKind::Info,
            "The design is still solving -- start Deep Solve when it has finished.",
        );
        return;
    }
    let start = match capture(ctx) {
        Ok(start) => start,
        Err(message) => {
            show_message(ctx, MessageKind::Error, &message);
            return;
        }
    };
    let pool = match workers::pool() {
        Ok(pool) => pool,
        Err(message) => {
            show_message(ctx, MessageKind::Error, &message);
            return;
        }
    };
    let epoch = RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        rt.epoch += 1;
        rt.shown = None;
        rt.epoch
    });
    model.set_result_stale(false);
    model.set_cancelling(false);
    model.set_running(true);
    model.set_status("Deep Solve... starting".into());
    model.set_status_is_problem(false);
    model.set_rows(ModelRc::new(VecModel::default()));

    let request = SolveRequest::DeepSolve {
        params: DeepSolveParams {
            targets: start.targets,
        },
    };
    let weak = ctx.ui.clone();
    let future = pool
        .solve()
        .solve_with_progress(start.toml, request, move |progress| {
            if RUNTIME.with(|rt| rt.borrow().epoch) != epoch {
                return;
            }
            if let Some(ui) = weak.upgrade() {
                ui.global::<DeepSolveModel>()
                    .set_status(progress.message.into());
            }
        });
    // The search is on its way: the lessons' "run Deep Solve" step.
    raise(ctx, DEEP_SOLVE_STARTED);
    let ctx = ctx.clone();
    let (design, generation) = (start.design, start.generation);
    wasm_bindgen_futures::spawn_local(async move {
        let result = future.await;
        finish_run(&ctx, epoch, &design, generation, result);
    });
}

/// Puts a finished run on screen.
fn finish_run(
    ctx: &Ctx,
    epoch: u64,
    design: &Design,
    generation: u64,
    result: Result<SolveResponse, SolveError>,
) {
    if RUNTIME.with(|rt| rt.borrow().epoch) != epoch {
        return;
    }
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DeepSolveModel>();
    model.set_running(false);
    model.set_cancelling(false);
    match result {
        Ok(SolveResponse::DeepSolved(data)) => {
            show_outcome(ctx, &model, design, generation, &data);
        }
        Ok(SolveResponse::AnalysisFailed {
            message,
            missing_anchor,
        }) => {
            let status = failure_status(&message, missing_anchor);
            model.set_status(status.as_str().into());
            model.set_status_is_problem(true);
            show_message(ctx, MessageKind::Error, &status);
        }
        Ok(SolveResponse::InvalidDesign { message }) => {
            let status = format!("The solve worker could not read the design: {message}");
            model.set_status(status.as_str().into());
            model.set_status_is_problem(true);
            show_message(ctx, MessageKind::Error, &status);
        }
        Ok(SolveResponse::Cancelled) => {
            model.set_status("Deep Solve cancelled.".into());
            model.set_status_is_problem(false);
        }
        Ok(_) => {
            model.set_status("Deep Solve was interrupted.".into());
            model.set_status_is_problem(false);
        }
        // The Worker did not stop within the grace period and was terminated.
        Err(SolveError::Cancelled) => {
            model.set_status(
                "Deep Solve cancelled -- the search did not stop in time and was ended.".into(),
            );
            model.set_status_is_problem(false);
            auto_solve(ctx);
        }
        Err(SolveError::Superseded) => {
            model.set_status(
                "Deep Solve stopped: an edit, Optimize or Retarget replaced it while it was \
                 running. Run it again."
                    .into(),
            );
            model.set_status_is_problem(false);
        }
        Err(error) => {
            let status = format!("Deep Solve failed: {error}");
            model.set_status(status.as_str().into());
            model.set_status_is_problem(true);
            show_message(ctx, MessageKind::Error, &status);
        }
    }
}

/// The verdict line and the tier rows for a finished run.
fn show_outcome(
    ctx: &Ctx,
    model: &DeepSolveModel<'_>,
    design: &Design,
    generation: u64,
    data: &DeepSolveResultData,
) {
    let stale = ctx.state.borrow().generation() != Some(generation);
    let Summary {
        status,
        problem,
        rows,
    } = summarize(data, design, stale);
    model.set_status(status.as_str().into());
    model.set_status_is_problem(problem);
    model.set_rows(ModelRc::new(VecModel::from(
        rows.iter()
            .map(|row| DeepSolveRow {
                tier_index: i32::try_from(row.tier_index).unwrap_or(i32::MAX),
                tier_number: row.text.tier_number.as_str().into(),
                name: row.text.name.as_str().into(),
                before: row.text.before_mast.as_str().into(),
                after: row.text.after_mast.as_str().into(),
                delta: row.text.delta.as_str().into(),
            })
            .collect::<Vec<_>>(),
    )));
    model.set_result_stale(stale);
    RUNTIME.with(|rt| {
        rt.borrow_mut().shown = Some(Shown {
            generation,
            masts: rows
                .iter()
                .map(|row| (row.tier_index, row.verified_mast))
                .collect(),
        });
    });
    // A verdict is on screen (stale or not accepted ones count, as on the desktop): the
    // lessons' "wait for the verdict" step.
    raise(ctx, DEEP_SOLVE_FINISHED);
    if stale {
        show_message(
            ctx,
            MessageKind::Info,
            "The design changed while Deep Solve was running -- its verdict is shown for \
             reference, but its tiers can no longer be pinned. Re-run Deep Solve on the \
             current design.",
        );
        return;
    }
    if problem {
        show_message(
            ctx,
            MessageKind::Info,
            "Deep Solve finished: not accepted -- the stone still deviates from the printed \
             figures.",
        );
    } else {
        show_message(
            ctx,
            MessageKind::Success,
            "Deep Solve finished: accepted (a strong external signal, not a proof).",
        );
    }
    if let Some(ui) = ctx.ui.upgrade() {
        // Bring the tab into view: completion is easy to miss on another tab.
        let inspector = ui.global::<InspectorModel>();
        inspector.set_tab(2);
        inspector.set_collapsed(false);
    }
}

/// Cancel: asks the Worker to stop (see the module doc comment). The answer arrives through
/// [`finish_run`]; only when the pool cannot ask, or on a second press, is the Worker ended.
fn cancel(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DeepSolveModel>();
    if !model.get_running() {
        return;
    }
    if model.get_cancelling() {
        // A second press while the Worker is still winding down ("Stop now").
        abandon(ctx, &model);
        return;
    }
    let asked = workers::pool().and_then(|pool| pool.solve().request_cancel());
    if asked.is_ok() {
        model.set_cancelling(true);
        model.set_status(
            "Cancelling -- the search stops at its next checkpoint (Stop now ends it at once)..."
                .into(),
        );
        model.set_status_is_problem(false);
        return;
    }
    abandon(ctx, &model);
}

/// The hard cancel: the Worker is terminated and the page is free again.
fn abandon(ctx: &Ctx, model: &DeepSolveModel<'_>) {
    RUNTIME.with(|rt| rt.borrow_mut().epoch += 1);
    if let Ok(pool) = workers::pool() {
        // Terminating the Worker discards the search; the pool starts a fresh one.
        let _ = pool.solve().cancel();
    }
    model.set_running(false);
    model.set_cancelling(false);
    model.set_status("Deep Solve cancelled -- the search was ended.".into());
    model.set_status_is_problem(false);
    // A solve that was queued behind the search has to run again.
    auto_solve(ctx);
}

/// Pin: sets tier `tier_index` to the mast Deep Solve verified for it, as one undoable edit,
/// if the result still describes the design.
fn pin(ctx: &Ctx, tier_index: i32) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    if ui.global::<DeepSolveModel>().get_running() {
        return;
    }
    let Ok(tier) = usize::try_from(tier_index) else {
        return;
    };
    let (shown, masts) = RUNTIME.with(|rt| {
        let rt = rt.borrow();
        (
            rt.shown.as_ref().map(|shown| shown.generation),
            rt.shown
                .as_ref()
                .map(|shown| shown.masts.clone())
                .unwrap_or_default(),
        )
    });
    let current = ctx.state.borrow().generation();
    let mast = match pin_check(shown, current, &masts, tier) {
        PinCheck::Ready(mast) => mast,
        PinCheck::Stale => {
            show_message(
                ctx,
                MessageKind::Warning,
                "The design changed since this Deep Solve ran -- re-run it before pinning.",
            );
            return;
        }
        PinCheck::Missing => {
            show_message(
                ctx,
                MessageKind::Error,
                "No verified mast recorded for this tier any more -- re-run Deep Solve.",
            );
            return;
        }
    };
    let outcome = {
        let mut app = ctx.state.borrow_mut();
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        design_state
            .session
            .pin_tier_mast(tier, mast, coalesce_now())
    };
    match outcome {
        Ok(Some(pinned)) => {
            // The pin is this result's own edit: move the result's generation along with
            // it, so the other rows stay pinnable. Any other edit still makes it stale.
            let generation = ctx.state.borrow().generation();
            RUNTIME.with(|rt| {
                if let (Some(shown), Some(generation)) =
                    (rt.borrow_mut().shown.as_mut(), generation)
                {
                    shown.generation = generation;
                }
            });
            finish(ctx, Dirty::All);
            let replaced = if pinned.replaced_meet.is_some() {
                " It replaced the tier's meet constraint; Undo restores it."
            } else {
                ""
            };
            show_message(
                ctx,
                MessageKind::Success,
                &format!(
                    "Pinned tier #{} to Deep Solve's verified mast.{replaced}",
                    tier + 1
                ),
            );
        }
        Ok(None) => show_message(
            ctx,
            MessageKind::Info,
            &format!(
                "Tier #{} is already pinned at Deep Solve's verified mast.",
                tier + 1
            ),
        ),
        Err(error) => show_message(ctx, MessageKind::Error, &error.to_string()),
    }
}

/// One refresh: whether Deep Solve can run, the hint that says why not, and whether the
/// shown result has gone stale. The Optimize tab's push lives in the inspector poll; this
/// panel keeps its own timer so that file stays untouched.
fn refresh(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DeepSolveModel>();
    let read = {
        let app = ctx.state.borrow();
        app.design.as_ref().map(|design_state| {
            let (available, hint) = availability(
                &design_state.session.design,
                design_state.printed_proportions.as_ref(),
            );
            (available, hint, design_state.session.current_generation())
        })
    };
    let Some((available, hint, generation)) = read else {
        clear(&model);
        return;
    };
    model.set_available(available);
    model.set_hint(hint.into());
    let stale = RUNTIME.with(|rt| {
        rt.borrow()
            .shown
            .as_ref()
            .is_some_and(|shown| shown.generation != generation)
    });
    model.set_result_stale(stale && !model.get_running());
}

/// Blank: no design is loaded.
fn clear(model: &DeepSolveModel<'_>) {
    RUNTIME.with(|rt| rt.borrow_mut().shown = None);
    model.set_available(false);
    model.set_hint(SharedString::new());
    model.set_result_stale(false);
    if !model.get_running() {
        model.set_status(SharedString::new());
        model.set_rows(ModelRc::new(VecModel::default()));
    }
}

/// Registers Deep Solve's callbacks and starts its refresh timer.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<DeepSolveModel>();
    let c = ctx.clone();
    model.on_run(move || run(&c));
    let c = ctx.clone();
    model.on_cancel(move || cancel(&c));
    let c = ctx.clone();
    model.on_pin(move |tier| pin(&c, tier));
    let c = ctx.clone();
    POLL.with(|timer| {
        timer.start(TimerMode::Repeated, POLL_INTERVAL, move || refresh(&c));
    });
    refresh(ctx);
}
