//! The guide driver: walks an open guide to its end the way a learner would, one step at a time.
//!
//! Per step:
//!
//! * a reading step (`completion == "manual"`) is finished with `GuideModel.next`, the Next
//!   button's callback;
//! * a step the panel offers to do (`offers_perform`) is done with `GuideModel.go_on`, the very
//!   callback the panel's button calls (it sets `perform_busy` and runs `GuideModel.perform`).
//!   The harness then waits in REAL time, with mock time kept in step with it so the recipe's
//!   own tick budget behaves as in the app, until the step's goal is met;
//! * a step without a recipe is completed by what it asks for where the harness can do that
//!   (the palette chord, the tutorial browser), otherwise by raising the event it waits for
//!   through `GuideModel.event` (the callback every widget route ends in; reported once as a
//!   note), otherwise it is a GAP (reported, then skipped so the later steps are still tried).
//!
//! A step that only advances by skipping is always a finding or a GAP, never a pass.

use super::super::main_window;
use crate::support::{
    Ctx,
    app::{DEFAULT_SIZE, pump},
    coverage,
    query::is_clickable_role,
};
use pleochroma_cut::{
    EditorModel, GuideModel, MainWindow, PreferencesModel, TutorialsModel, VariantsModel,
    harness_support,
};
use pleochroma_editor::guide::{
    Goal,
    solving_events::{VARIANT_SAVED, VARIANTS_OPENED},
};
use slint::{ComponentHandle, Model};
use std::{
    cell::Cell,
    time::{Duration, Instant},
};

/// How long one step may take before it is a finding (a solve takes seconds).
const STEP_TIMEOUT: Duration = Duration::from_secs(30);
/// The limit once an earlier step was skipped: the state may not be what the step expects.
const AFTER_SKIP_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to keep waiting after the recipe gave up (a late solve result still completes the
/// step in the app).
const GIVE_UP_GRACE: Duration = Duration::from_secs(5);
/// Time for a freshly entered step to check its goal against the design.
const ENTRY_SETTLE: Duration = Duration::from_millis(120);
/// Time for a chord or a browser opening to reach the guide.
const ROUTE_SETTLE: Duration = Duration::from_millis(1500);
/// The event the palette reports when it opens.
const PALETTE_OPENED: &str = "palette_opened";
/// The event the tutorial browser reports when it opens.
const TUTORIALS_OPENED: &str = "tutorials_opened";
/// The event a written cutting sheet reports.
const SHEET_EXPORTED: &str = "sheet_exported";
/// The event the Interface Mode switch reports.
const INTERFACE_MODE_CHANGED: &str = "interface_mode_changed";
/// Time for a solve and the sheet write to reach the guide.
const SHEET_SETTLE: Duration = Duration::from_secs(30);
/// Time for a variant save or open (a library write on a worker) to land.
const VARIANT_SETTLE: Duration = Duration::from_secs(15);
/// The goal label of the Variants lesson's "Go back to the first variant" step
/// (`pleochroma-editor` `guide/tutorials/solving/compare.rs`): opening the first variant puts the
/// template's crown and pavilion back.
const FIRST_VARIANT_CHECK: &str = "the template's own crown and pavilion";
/// A window tall enough that the Edit view's dock shows the inspector whole (the coverage
/// click-through's `coverage::overlays::TALL`).
const TALL: (u32, u32) = (1366, 3200);
/// The names the Variants lesson asks for, in order of saving.
const VARIANT_NAMES: [&str; 2] = ["Variant 1", "Steeper crown"];

/// What `GuideModel` shows right now.
#[derive(Clone, Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the GuideModel properties it reads; open, done, busy, failed, offers_perform and manual are independent flags"
)]
pub struct Snap {
    pub open: bool,
    pub id: String,
    pub index: i32,
    pub total: i32,
    pub done: bool,
    pub busy: bool,
    pub failed: bool,
    pub offers_perform: bool,
    pub manual: bool,
    pub title: String,
    pub waiting: String,
}

impl Snap {
    /// Whether step `n` (0-based) is the open guide's current step.
    fn at(&self, n: usize) -> bool {
        self.open && usize::try_from(self.index) == Ok(n)
    }
}

/// Reads `GuideModel`.
pub fn snap(ui: &MainWindow) -> Snap {
    let guide = ui.global::<GuideModel>();
    let current = guide.get_current();
    Snap {
        open: guide.get_open(),
        id: guide.get_guide_id().to_string(),
        index: guide.get_step_index(),
        total: guide.get_total_steps(),
        done: guide.get_step_done(),
        busy: guide.get_perform_busy(),
        failed: guide.get_perform_failed(),
        offers_perform: guide.get_offers_perform(),
        manual: guide.get_current_is_manual(),
        title: current.title.to_string(),
        waiting: current.waiting.to_string(),
    }
}

/// Pumps until `condition` holds or `timeout` of real time has passed. Unlike
/// `app::wait_until`, mock time advances by the real time that passed (not a fixed 10 ms per
/// round), so the guide's timers and the recipe's tick budget run at the speed they do in the
/// app while worker threads solve in real time.
pub fn wait_real(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    let mut last = start;
    loop {
        let now = Instant::now();
        let ms = u64::try_from(now.duration_since(last).as_millis()).unwrap_or(10);
        last = now;
        pump(ms.clamp(5, 100));
        if condition() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// What a run of [`drive_guide`] did.
#[derive(Debug, Default)]
pub struct RunStats {
    /// Steps the guide has.
    pub steps: usize,
    /// Steps finished by their goal (a recipe, or the thing the step asks for).
    pub by_goal: usize,
    /// Reading steps finished with Next.
    pub read: usize,
    /// Steps whose event was raised through `GuideModel.event` instead of its widget.
    pub injected: usize,
    /// Steps the harness could not drive (reported as GAP).
    pub gaps: usize,
    /// Steps passed with Skip (after a finding or a GAP).
    pub skipped: usize,
    /// The walk ended early because the guide was not where it should be.
    pub aborted: bool,
}

/// Walks the open guide from its current step to its end. `goals` are the guide's goals when
/// the guide is a built-in one (they tell the harness what a step without a recipe waits for);
/// `scenario` names the scenario for the findings.
pub fn drive_guide(
    ctx: &mut Ctx<'_>,
    scenario: &str,
    guide_id: &str,
    goals: Option<&[Goal]>,
) -> RunStats {
    let mut stats = RunStats::default();
    let first = snap(ctx.app.ui());
    let total = usize::try_from(first.total).unwrap_or(0);
    stats.steps = total;
    for n in 0..total {
        let before = snap(ctx.app.ui());
        let label = format!("{guide_id} step {}: {}", n + 1, before.title);
        ctx.report.set_context(format!("{scenario}: {label}"));
        if !before.at(n) {
            ctx.report.add_simple(
                "MainWindow",
                "tutorial-state",
                format!(
                    "the guide should be at step {} but shows open={} step_index={}",
                    n + 1,
                    before.open,
                    before.index
                ),
            );
            stats.aborted = true;
            break;
        }
        if n == 0 {
            ctx.sweep_main_at_sizes(&format!("{guide_id} guide panel open"));
        }
        let goal = goals.and_then(|all| all.get(n));
        drive_step(ctx, &label, n, total, &before, goal, &mut stats);
        ctx.check_health(&label);
        if stats.aborted {
            break;
        }
    }
    ctx.report.set_context(scenario.to_owned());
    stats
}

/// Finishes step `n` and checks that the guide moved on.
fn drive_step(
    ctx: &mut Ctx<'_>,
    label: &str,
    n: usize,
    total: usize,
    before: &Snap,
    goal: Option<&Goal>,
    stats: &mut RunStats,
) {
    // Was a fixed wait of `ENTRY_SETTLE`: a reading step and a step the goal already finished
    // (the guide moved on or reports it done) are decided at once, so the wait ends there; any
    // other step still gets the full time to check its goal.
    let _ = wait_real(ENTRY_SETTLE, || {
        let now = snap(ctx.app.ui());
        !now.at(n) || now.manual || now.done
    });
    let now = snap(ctx.app.ui());
    if !now.at(n) {
        // The goal held on entry and the "Done" moment already moved on.
        stats.by_goal += 1;
    } else if now.manual {
        stats.read += 1;
        press_next(ctx);
    } else if now.done {
        // The goal already held when the step was entered.
        stats.by_goal += 1;
        press_next(ctx);
    } else if now.offers_perform {
        ctx.app.ui().global::<GuideModel>().invoke_go_on();
        await_and_advance(ctx, label, n, before, stats);
        // The assertion of its effect is verify_advanced below; recorded here, after the call.
        coverage::record_callback("GuideModel.go_on");
    } else {
        drive_without_recipe(ctx, label, n, goal, before, stats);
    }
    verify_advanced(ctx, label, n, total, stats);
}

/// The Next button.
fn press_next(ctx: &Ctx<'_>) {
    ctx.app.ui().global::<GuideModel>().invoke_next();
    pump(30);
    // No assertion precedes this: verify_advanced checks the step moved on, after the call.
    coverage::record_callback("GuideModel.next");
}

/// Waits for step `n` to be done (the app advances by itself after the "Done" moment, which
/// also counts), then presses Next. A recipe that gave up, a timeout and an unsaved-changes
/// question are findings; the step is then skipped so the later steps are still tried.
fn await_and_advance(
    ctx: &mut Ctx<'_>,
    label: &str,
    n: usize,
    before: &Snap,
    stats: &mut RunStats,
) {
    let timeout = if stats.skipped > 0 {
        AFTER_SKIP_TIMEOUT
    } else {
        STEP_TIMEOUT
    };
    let started = Instant::now();
    let gave_up_at: Cell<Option<Instant>> = Cell::new(None);
    let question = Cell::new(false);
    let ui = ctx.app.ui();
    let _ = wait_real(timeout, || {
        let s = snap(ui);
        if !s.at(n) || s.done {
            return true;
        }
        if ui.global::<EditorModel>().get_unsaved_dialog_open() {
            question.set(true);
            return true;
        }
        if s.failed && !s.busy {
            let since = gave_up_at.get().unwrap_or_else(Instant::now);
            gave_up_at.set(Some(since));
            return since.elapsed() >= GIVE_UP_GRACE;
        }
        false
    });
    let end = snap(ui);
    let after_skip = if stats.skipped > 0 {
        " (an earlier step of this guide was skipped, so the state may not match)"
    } else {
        ""
    };
    let waiting = &before.waiting;
    if question.get() {
        ctx.report.add_simple(
            "MainWindow",
            "tutorial-unsaved-question",
            format!("{label}: the unsaved-changes question appeared; waiting for: {waiting}"),
        );
        ctx.app
            .ui()
            .global::<EditorModel>()
            .invoke_unsaved_dialog_discard();
        skip(ctx, stats);
    } else if !end.at(n) || end.done {
        stats.by_goal += 1;
        if gave_up_at.get().is_some() {
            ctx.report.add_simple(
                "MainWindow",
                "tutorial-perform-gave-up",
                format!(
                    "{label}: the recipe gave up (perform_failed) before its goal was met; \
                     the step was done after {} ms. Waiting for: {waiting}",
                    started.elapsed().as_millis()
                ),
            );
        }
        if end.at(n) {
            press_next(ctx);
        }
    } else {
        let why = if gave_up_at.get().is_some() {
            "the recipe ran, gave up (perform_failed) and the goal was never met"
        } else {
            "the goal was not met in time"
        };
        ctx.report.add_simple(
            "MainWindow",
            "tutorial-step-not-completed",
            format!(
                "{label}: {why} after {} ms. Waiting for: {waiting}{after_skip}",
                started.elapsed().as_millis()
            ),
        );
        skip(ctx, stats);
    }
}

/// "Skip step": Next on an unfinished step.
fn skip(ctx: &Ctx<'_>, stats: &mut RunStats) {
    stats.skipped += 1;
    press_next(ctx);
}

/// A step with no recipe: do what it asks where the harness can, else raise its event, else GAP.
fn drive_without_recipe(
    ctx: &mut Ctx<'_>,
    label: &str,
    n: usize,
    goal: Option<&Goal>,
    before: &Snap,
    stats: &mut RunStats,
) {
    let Some(goal) = goal else {
        gap(
            ctx,
            label,
            "a step without a recipe in a generated lesson (no goal information)",
            stats,
        );
        return;
    };
    if matches!(goal, Goal::YieldApplied) {
        apply_yield_inputs(ctx);
        await_and_advance(ctx, label, n, before, stats);
        return;
    }
    if matches!(goal, Goal::Check { label: check, .. } if *check == FIRST_VARIANT_CHECK) {
        open_first_variant(ctx);
        await_and_advance(ctx, label, n, before, stats);
        return;
    }
    // A goal made of parts the harness can each bring about (a tab plus the event of a button
    // on it): every part in turn, then the usual wait.
    if let Goal::All(parts) = goal
        && !event_only(goal)
        && parts.iter().all(drivable_part)
    {
        for part in parts {
            drive_part(ctx, part, stats);
        }
        await_and_advance(ctx, label, n, before, stats);
        return;
    }
    if !event_only(goal) {
        let why = format!(
            "goal {} has no recipe and cannot be driven",
            goal_kind(goal)
        );
        gap(ctx, label, &why, stats);
        return;
    }
    let events: Vec<String> = goal.events().into_iter().map(str::to_owned).collect();
    for name in &events {
        drive_event(ctx, name, events.len() == 1, stats);
    }
    await_and_advance(ctx, label, n, before, stats);
}

/// The recipe of a `YieldApplied` step (the guide has none of its own: the learner types the
/// stone width into the Preform tab): sets the girdle diameter and presses Apply Yield Inputs
/// through the callback the button calls.
fn apply_yield_inputs(ctx: &Ctx<'_>) {
    const GIRDLE_DIAMETER_MM: &str = "6.5";
    let editor = ctx.app.ui().global::<EditorModel>();
    editor.set_girdle_diameter_mm(GIRDLE_DIAMETER_MM.into());
    editor.invoke_apply_yield_inputs(
        GIRDLE_DIAMETER_MM.into(),
        0,
        editor.get_specific_gravity_override(),
    );
    pump(100);
    if !editor.get_design_girdle_diameter_text().is_empty() {
        coverage::record_callback("EditorModel.apply_yield_inputs");
    }
}

/// Records a GAP and skips the step.
fn gap(ctx: &mut Ctx<'_>, label: &str, why: &str, stats: &mut RunStats) {
    stats.gaps += 1;
    ctx.report.note(format!("GAP: {label} -- {why}"));
    skip(ctx, stats);
}

/// Whether [`drive_part`] can bring this part of an `All` goal about.
const fn drivable_part(goal: &Goal) -> bool {
    matches!(goal, Goal::Event(_) | Goal::InspectorTab(_))
}

/// One part of an `All` goal: an inspector tab is shown the way the app's own "show tab" does
/// it (the Edit view, the tab, the inspector expanded), an event goes through [`drive_event`].
fn drive_part(ctx: &mut Ctx<'_>, part: &Goal, stats: &mut RunStats) {
    match part {
        Goal::InspectorTab(tab) => {
            main_window::select_top_tab_model(ctx.app, 0);
            main_window::select_render_view(ctx.app, 1);
            let editor = ctx.app.ui().global::<EditorModel>();
            editor.set_inspector_tab(*tab);
            editor.set_inspector_collapsed(false);
            pump(100);
        }
        Goal::Event(name) => drive_event(ctx, name, false, stats),
        _ => {}
    }
}

/// Presses the first clickable element labelled `label` whose element type is `kind` in the main
/// window; `false` when there is none.
fn press_label(ctx: &Ctx<'_>, label: &str, kind: &str) -> bool {
    let view = ctx.app.main_view();
    let Some(element) = view.by_label(label).into_iter().find(|e| {
        e.accessible_role().is_some_and(is_clickable_role)
            && e.type_name().is_some_and(|t| t.as_str() == kind)
    }) else {
        return false;
    };
    coverage::record_activation(&element);
    coverage::record_wrappers(&view.root, &element, false);
    element.invoke_accessible_default_action();
    pump(200);
    true
}

/// The History tab's Variants pill, which reports `variants_opened` itself; the event is raised
/// through `GuideModel.event` only when the pill cannot be found or did not report it.
fn open_variants_view(ctx: &mut Ctx<'_>, stats: &mut RunStats) {
    let variants = ctx.app.ui().global::<VariantsModel>();
    if variants.get_view() == 1 {
        // The pill reports only when clicked; leave the view first, as the lesson tells the user.
        variants.set_view(0);
        pump(50);
    }
    // The inspector is the last part of the Edit view's scrolling dock: at the default 1366 x 768
    // it lies below the dock's visible area, and the harness does not enumerate scrolled-out
    // controls (the 2026-10-30 local run found the Edit view but no inspector at all). The window
    // is made tall like the coverage click-through's (`coverage::overlays::TALL`) for the press.
    let before = ctx.app.main_view().size;
    ctx.app.set_size(TALL.0, TALL.1);
    pump(100);
    // The pill of its own component type: other controls are labelled "Variants" too, and the
    // first of them was pressed instead (2026-10-29_170825). The History tab is built when the
    // inspector shows it, so the pill may need a frame or two to appear.
    let mut found = false;
    let _ = wait_real(ROUTE_SETTLE, || {
        found = press_label(ctx, "Variants", "HistoryViewPill");
        found
    });
    let pressed = found && wait_real(ROUTE_SETTLE, || variants.get_view() == 1);
    if !pressed {
        // What the window does offer under that label, so the next run says why.
        let seen: Vec<String> = ctx
            .app
            .main_view()
            .by_label("Variants")
            .into_iter()
            .map(|e| {
                format!(
                    "{}/{:?}",
                    e.type_name()
                        .map_or_else(|| "?".to_owned(), |t| t.to_string()),
                    e.accessible_role()
                )
            })
            .collect();
        let view = ctx.app.main_view();
        let tabs: Vec<String> = view
            .root
            .query_descendants()
            .match_accessible_role(uiprobe::AccessibleRole::Tab)
            .find_all()
            .into_iter()
            .map(|e| {
                e.accessible_label()
                    .map_or_else(String::new, |l| l.to_string())
            })
            .collect();
        let count = |component: &str| {
            view.root
                .query_descendants()
                .match_inherits(component)
                .find_all()
                .len()
        };
        let ui = ctx.app.ui();
        let editor = ui.global::<EditorModel>();
        let parts = format!(
            "active tab {}, render view {}, editor enabled {}, inspector collapsed {}, height {}; \
             EditorView {}, EditorInspector {}, InspectorTabPill {}, HistoryTab {}, \
             HistoryViewPill {}",
            ui.get_active_tab(),
            ui.get_render_view_tab(),
            editor.get_enabled(),
            editor.get_inspector_collapsed(),
            editor.get_inspector_height(),
            count("EditorView"),
            count("EditorInspector"),
            count("InspectorTabPill"),
            count("HistoryTab"),
            count("HistoryViewPill"),
        );
        ctx.report.add_simple(
            "MainWindow",
            "tutorial-event-route",
            format!(
                "the History tab's Variants pill (HistoryViewPill) {} (window {:?}, inspector tab \
                 {}, elements labelled Variants: {seen:?}, tabs on screen: {tabs:?}; {parts})",
                if found {
                    "did not open the Variants view"
                } else {
                    "was not found"
                },
                view.size,
                ctx.app.ui().global::<EditorModel>().get_inspector_tab()
            ),
        );
        inject(ctx, VARIANTS_OPENED, stats);
    }
    // Back to the size the walk had, so the later steps see the window they would have seen.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a window size in whole logical pixels"
    )]
    let (width, height) = (before.0.round() as u32, before.1.round() as u32);
    if width > 0 && height > 0 {
        ctx.app.set_size(width, height);
    } else {
        ctx.app.set_size(DEFAULT_SIZE.0, DEFAULT_SIZE.1);
    }
    pump(100);
}

/// Saves the design as a new variant through the Variants form (Save as Variant..., the name,
/// Save), the route that reports `variant_saved`. The name is the next one the lesson asks for.
fn save_variant(ctx: &mut Ctx<'_>, single: bool, stats: &mut RunStats) {
    let variants = ctx.app.ui().global::<VariantsModel>();
    let count = variants.get_rows().row_count();
    let name = VARIANT_NAMES
        .get(count)
        .copied()
        .unwrap_or("Another variant");
    variants.invoke_begin_save(-1);
    pump(50);
    variants.set_form_name(name.into());
    variants.invoke_submit_form();
    // The save raises the event when it lands; whether the guide saw it is the step's own wait.
    let saved = wait_real(VARIANT_SETTLE, || {
        !variants.get_busy()
            && variants
                .get_rows()
                .iter()
                .any(|row| row.name.as_str() == name)
    });
    if saved {
        coverage::record_callback("VariantsModel.begin_save");
        coverage::record_callback("VariantsModel.submit_form");
    } else {
        if single {
            ctx.report.add_simple(
                "MainWindow",
                "tutorial-event-route",
                format!(
                    "saving the variant {name:?} did not report variant_saved (form error: {:?})",
                    variants.get_form_error()
                ),
            );
        }
        variants.invoke_cancel_form();
        pump(30);
        inject(ctx, VARIANT_SAVED, stats);
    }
}

/// Opens the oldest saved variant (the list is newest first), which the Variants lesson's
/// "Go back to the first variant" step asks for.
fn open_first_variant(ctx: &mut Ctx<'_>) {
    let variants = ctx.app.ui().global::<VariantsModel>();
    let rows = variants.get_rows();
    let Some(oldest) = rows
        .row_count()
        .checked_sub(1)
        .and_then(|i| rows.row_data(i))
    else {
        ctx.report.add_simple(
            "MainWindow",
            "tutorial-step-state",
            "Go back to the first variant: no variant was saved, so there is none to open",
        );
        return;
    };
    variants.invoke_open_variant(oldest.key.clone());
    let opened = wait_real(VARIANT_SETTLE, || {
        !variants.get_busy()
            && variants
                .get_rows()
                .iter()
                .any(|row| row.key == oldest.key && row.opened_from)
    });
    if opened {
        coverage::record_callback("VariantsModel.open_variant");
    }
}

/// Whether the goal is made of events only.
fn event_only(goal: &Goal) -> bool {
    match goal {
        Goal::Event(_) => true,
        Goal::All(goals) | Goal::Any(goals) => goals.iter().all(event_only),
        _ => false,
    }
}

/// A short name for a goal kind, for the report.
const fn goal_kind(goal: &Goal) -> &'static str {
    match goal {
        Goal::Manual => "Manual",
        Goal::Event(_) => "Event",
        Goal::TierMatches { .. } => "TierMatches",
        Goal::TierExists(_) => "TierExists",
        Goal::TierCountAtLeast(_) => "TierCountAtLeast",
        Goal::Material(_) => "Material",
        Goal::SolvedClosed => "SolvedClosed",
        Goal::YieldApplied => "YieldApplied",
        Goal::ViewMode(_) => "ViewMode",
        Goal::InspectorTab(_) => "InspectorTab",
        Goal::FreshDesign { .. } => "FreshDesign",
        Goal::ConcaveTiersAtLeast(_) => "ConcaveTiersAtLeast",
        Goal::DesignRebuilt { .. } => "DesignRebuilt",
        Goal::Check { label, .. } => label,
        Goal::All(_) => "All",
        Goal::Any(_) => "Any",
    }
}

/// Raises event `name` the way the UI does where the harness can drive that route (`single` is
/// whether it is the only event the step waits for, so the route can be judged), else through
/// `GuideModel.event`.
fn drive_event(ctx: &mut Ctx<'_>, name: &str, single: bool, stats: &mut RunStats) {
    match name {
        PALETTE_OPENED => {
            ctx.press("Ctrl+K");
            let done = wait_real(ROUTE_SETTLE, || snap(ctx.app.ui()).done);
            ctx.press("Escape");
            if !done && single {
                ctx.report.add_simple(
                    "MainWindow",
                    "tutorial-event-route",
                    "Ctrl+K did not report palette_opened to the guide",
                );
                inject(ctx, name, stats);
            }
        }
        TUTORIALS_OPENED => {
            ctx.app
                .ui()
                .global::<TutorialsModel>()
                .invoke_open_browser();
            let done = wait_real(ROUTE_SETTLE, || snap(ctx.app.ui()).done);
            if done {
                coverage::record_callback("TutorialsModel.open_browser");
            }
            ctx.app
                .ui()
                .global::<TutorialsModel>()
                .invoke_close_browser();
            pump(50);
            // No assertion precedes this: the browser's closed state is not read back.
            coverage::record_callback("TutorialsModel.close_browser");
            if !done && single {
                ctx.report.add_simple(
                    "MainWindow",
                    "tutorial-event-route",
                    "opening the tutorial browser did not report tutorials_opened",
                );
                inject(ctx, name, stats);
            }
        }
        SHEET_EXPORTED => {
            // The Export menu's cutting sheet: the picker answer first, then the callback the
            // menu item calls; the sheet is written on a worker and reported when it lands.
            let path = ctx.scratch_file("tutorial_cutting_sheet.html");
            harness_support::queue_picker_path(path.clone());
            ctx.app
                .ui()
                .global::<EditorModel>()
                .invoke_export_cutting_sheet();
            let done = wait_real(SHEET_SETTLE, || path.exists() && snap(ctx.app.ui()).done);
            if done {
                // The sheet exists and the guide saw the event.
                coverage::record_callback("EditorModel.export_cutting_sheet");
            } else {
                harness_support::clear_picker_answers();
                if single {
                    ctx.report.add_simple(
                        "MainWindow",
                        "tutorial-event-route",
                        "Export Cutting Sheet did not report sheet_exported to the guide",
                    );
                }
                inject(ctx, name, stats);
            }
        }
        INTERFACE_MODE_CHANGED => {
            // The Preferences switch: flips the mode and fires the callback the switch calls.
            let prefs = ctx.app.ui().global::<PreferencesModel>();
            let simple = !prefs.get_simple_mode();
            prefs.set_simple_mode(simple);
            prefs.invoke_simple_mode_changed(simple);
            let done = wait_real(ROUTE_SETTLE, || snap(ctx.app.ui()).done);
            if done {
                coverage::record_callback("PreferencesModel.simple_mode_changed");
            } else {
                if single {
                    ctx.report.add_simple(
                        "MainWindow",
                        "tutorial-event-route",
                        "switching the interface mode did not report interface_mode_changed",
                    );
                }
                inject(ctx, name, stats);
            }
        }
        VARIANTS_OPENED => open_variants_view(ctx, stats),
        VARIANT_SAVED => save_variant(ctx, single, stats),
        _ => inject(ctx, name, stats),
    }
}

/// Raises `name` through `GuideModel.event`.
fn inject(ctx: &mut Ctx<'_>, name: &str, stats: &mut RunStats) {
    stats.injected += 1;
    ctx.report.note(format!(
        "tutorial event `{name}` is raised through GuideModel.event (the callback its widget \
         route ends in); the widget route itself is not driven"
    ));
    ctx.app
        .ui()
        .global::<GuideModel>()
        .invoke_event(name.into());
    pump(30);
    // No assertion precedes this: the step's advance is asserted by verify_advanced afterwards.
    coverage::record_callback("GuideModel.event");
}

/// After step `n` was finished: the guide is at step `n + 1`, or closed after the last step.
fn verify_advanced(ctx: &mut Ctx<'_>, label: &str, n: usize, total: usize, stats: &mut RunStats) {
    pump(30);
    let s = snap(ctx.app.ui());
    let advanced = if n + 1 < total { s.at(n + 1) } else { !s.open };
    if !advanced {
        ctx.report.add_simple(
            "MainWindow",
            "tutorial-advance",
            format!(
                "{label}: after finishing the step the guide shows open={} step_index={} \
                 (expected {})",
                s.open,
                s.index,
                if n + 1 < total {
                    format!("step {}", n + 2)
                } else {
                    "closed".to_owned()
                }
            ),
        );
        stats.aborted = true;
    }
}
