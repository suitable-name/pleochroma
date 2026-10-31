//! The guided tutorials (Help > Tutorials..., and the empty state's walkthrough link): the lesson
//! runner. It pushes the running lesson's steps into `ui/models/guide_model.slint`'s
//! `GuideModel`, judges the current step from state, takes the UI events a step may wait for,
//! starts a lesson (arranging the design it begins with) and keeps the reader's place across a
//! reload.
//!
//! # Shared with the desktop
//!
//! The lessons are `pleochroma_editor::guide`'s catalogue (`GuideCatalog`), the same data the
//! desktop's `gui/editor/guide` pushes into its own `GuideModel`. The browser shows each lesson
//! through the web overlay (`GuideCatalog::web_guide`): the browser's own wording, the steps it
//! cannot do left out, its lock groups. Which lessons are offered is the shared offer table
//! (`WebFeatures`, held by `pleochroma_web_core::guide_runner::Offers`), and the browser lists exactly those
//! ([`offered_guides`], `editor::tutorials`). Every step is indexed in the web view of its
//! lesson, never in the catalogue's (an omitted step shifts the later indices).
//!
//! The ten-step worked example ("New Design Walkthrough") is one of those lessons and runs through
//! the same code; `GuideModel.start()` starts it.
//!
//! The Slint side owns navigation (`start-guide` / `next` / `back` / `close`, the short "Done"
//! moment before an automatic advance) and every lock: each control ANDs a
//! `GuideModel.allows-*()` helper into its own `enabled`, driven by the current step's `allow`
//! flags `push_guide` pushes. Rust's jobs are the content, reporting reached goals through
//! `GuideModel.notify`, the events, the launch, and the tab session.
//!
//! # Which action reports what
//!
//! Goals are judged from STATE, never from which button was clicked, so every route to a
//! goal counts (quick add, the inspector form, an inline edit, undo / redo, a drag, the
//! auto-solve), and a rejected form -- which never changes the design -- can never
//! advance. [`check_progress`] is the one evaluation (`judge::goal_is_met` over the web's
//! `GoalContext`); it runs
//!
//! - at the end of every edit ([`super::edit::finish_edit`]: tier added / edited / removed,
//!   Apply Material, Apply Yield Inputs, orbit tools, Optimize / Retarget applied, ...);
//! - after an undo or redo, and after a design is opened or restored;
//! - when a solve finishes (`app::solve::finish`: "Solve and check", whether the button or
//!   the auto-solve ran it);
//! - whenever the tier table sees the design or its solve change (its 100 ms poll: a drag
//!   in the Solid view, anything not listed above);
//! - when a step is entered (`GuideModel.step-entered`), so a goal that is already met
//!   completes without waiting for another edit;
//! - every 300 ms while the current step's goal reads the view tab or the inspector tab, which
//!   change without the design changing (`GuideModel.poll`).
//!
//! A goal that is an EVENT rather than a state ([`raise`]) is reported by the place the action
//! succeeds. The only event raised across the app today is `NEW_DESIGN_CREATED`
//! (`pleochroma_editor::guide::NEW_DESIGN_CREATED`), by the New Design dialog and the gallery;
//! the browser raises `tutorials_opened` itself. Raising the rest, per lesson section, is the
//! events lanes' job: they call [`raise`] with an `EVENTS` name from the place the action
//! succeeds.
//!
//! # The web's stage
//!
//! The desktop's "Show Me Where" sets up the view, panels, inspector tab and camera a step
//! needs. The web has three view tabs and five inspector tabs; `judge` maps the shared stage onto
//! them, the panel offers the button when the window shows something else, and `apply_stage`
//! goes there. The library panel, accordion sections, Parameters dock and camera pose are the
//! desktop's and are ignored.
//!
//! # Starting a lesson
//!
//! [`start_guide`] asks `plan_start` what the lesson begins with. A lesson that needs a design says
//! so in a toast; a lesson that starts from a new design creates it the way the New Design dialog
//! does (the unsaved-changes guard first) and opens when the design has arrived; a lesson on the
//! open design opens at once. A lesson that starts from a library design (`OpenLibrary`) says in
//! a toast that this build cannot open one: no lesson the browser offers starts that way before
//! the library lane lands, and that lane adds the opener.
//!
//! # Next does it for me
//!
//! A step with a recipe (`GuideStep::perform`) gets a Next button that does the step: `perform`
//! maps the recipe onto the page's own controls (the Tier form, the table's Solve and Delete,
//! Apply Material, the view tabs, ...), and a recipe the page cannot do keeps Skip Step. The
//! mapping is `pleochroma_web_core::guide_perform`; [`register_performer`] is how it plugs in.
//!
//! # Tab session
//!
//! Open / lesson / step / collapsed / the dragged panel position are stored as JSON under
//! `localStorage["pleochroma.guide.v2"]` (`pleochroma_web_core::storage_keys::GuidePlace`; the ten
//! step walkthrough's `pleochroma.guide.v1` is read once when v2 is absent, and with
//! `localStorage` blocked the key stays in `sessionStorage`), written a moment after any of them
//! changes and read back by [`restore`]. The finished lessons are `editor::tutorials`'.

mod perform;

use crate::{
    AppModel, AppWindow, ConfirmModel, GalleryModel, GuideAllow, GuideModel, GuideStepData,
    InspectorModel,
    app::{
        Ctx, persist,
        push::{MessageKind, push_all, show_message},
        solve::auto_solve,
        state::{DesignSource, DesignState, SolveState, WebApp},
    },
    editor::{tutorials, unsaved::confirm_discard},
};
use pleochroma_cut_core::FreshDesignSpec;
use pleochroma_editor::{
    EditorSession,
    guide::{
        Group, Guide, GuideStep, LaunchStatus, NEW_DESIGN_CREATED, StartPlan, launch_status,
        plan_start,
    },
    templates::template_cards,
};
use pleochroma_web_core::{
    guide::{GUIDE_KEY, GuideProgress},
    guide_event_log::EventLog,
    guide_judge::{self as judge, RENDER_TAB, SOLID_TAB, Shown, UiFacts},
    guide_runner::{AllowFlags, LAUNCH_GRACE_TICKS, Launch, Offers, observe, step_view},
    storage_keys::{GuidePlace, guide_place_key},
};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::{cell::RefCell, time::Duration};

/// Quiet time before a change of the guide's state is written.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(300);

/// How long the guide keeps trying to re-check a step while the app state is being written.
const RECHECK_AFTER: Duration = Duration::from_millis(50);

/// How many 300 ms ticks Next waits for a step it is doing for the learner before it gives up
/// (a solve can take a few seconds).
const PERFORM_PATIENCE_TICKS: u32 = 20;

/// What Next does for the learner on a step with a recipe: runs it through the editor's own
/// paths and returns whether it ran. Registered by the executor lane
/// ([`register_performer`]); the step then completes the usual way once its goal holds.
pub type Performer = fn(&Ctx, &GuideStep) -> bool;

/// What the runner remembers between a lesson's steps.
struct Runtime {
    /// The lessons the browser offers (the catalogue and whether the web library exists, see
    /// [`set_library_available`]).
    offers: Offers,
    /// The running lesson as the web shows it.
    active: Option<Guide>,
    /// The UI events reported since the current step began.
    events: EventLog,
    /// The lesson waiting for its starting design.
    launch: Option<Launch>,
    /// What Next does on a step with a recipe, once the executor is registered.
    performer: Option<Performer>,
    /// Ticks since the press of Next that is doing a step.
    perform_ticks: u32,
}

impl Runtime {
    fn new() -> Self {
        Self {
            offers: Offers::new(),
            active: None,
            events: EventLog::default(),
            launch: None,
            performer: None,
            perform_ticks: 0,
        }
    }
}

thread_local! {
    /// The runner's state. Slint callbacks all run on the UI thread, so this lives in a
    /// `thread_local`. Nothing calls into Slint while it is borrowed (a Slint callback could
    /// re-enter Rust), and nothing hands out the borrow.
    static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::new());

    /// Debounces the storage write (restarting the same timer is what makes it a
    /// debounce).
    static SAVE: Timer = Timer::default();
}

fn with<R>(f: impl FnOnce(&mut Runtime) -> R) -> R {
    RUNTIME.with_borrow_mut(f)
}

/// Tells the runner that the web library exists (or no longer does). The library lessons and
/// the lessons built from a library design are offered from then on. (Deep Solve, concave
/// editing and cutting mode exist in the browser; see `Offers::features`.)
pub fn set_library_available(available: bool) {
    with(|rt| rt.offers.set_library(available));
}

/// The lessons the browser offers, as it shows them, in the learning path's order.
#[must_use]
pub fn offered_guides() -> Vec<Guide> {
    with(|rt| rt.offers.offered())
}

/// Lesson `id` as the browser shows it, or `None` when it is not offered.
#[must_use]
pub fn web_guide(id: &str) -> Option<Guide> {
    with(|rt| rt.offers.web_guide(id))
}

/// Adds a lesson generated at run time (a lesson that rebuilds a library design, say) to the
/// catalogue, so the browser lists it once the library exists and [`start_guide`] can start it.
///
/// # Errors
///
/// A lesson that is not fit to run, or whose id belongs to a built-in lesson, is refused with
/// the reason (`GuideCatalog::add_generated`).
#[expect(
    dead_code,
    reason = "the library UI lane registers its generated lessons"
)]
pub fn register_generated(guide: Guide) -> Result<(), String> {
    with(|rt| rt.offers.register_generated(guide))
}

/// Registers the executor behind Next on a step with a recipe (`perform`, from [`wire`]).
/// Until then every action step keeps its Skip Step button.
pub fn register_performer(ui: &AppWindow, performer: Performer) {
    with(|rt| rt.performer = Some(performer));
    ui.global::<GuideModel>().set_performer_ready(true);
}

/// The lock groups a step leaves open (`pleochroma_web_core::guide_runner::allow_flags`) as the
/// Slint `GuideAllow` struct.
const fn guide_allow(allow: AllowFlags) -> GuideAllow {
    GuideAllow {
        new_design: allow.new_design,
        tier_form: allow.tier_form,
        tier_table: allow.tier_table,
        design_settings: allow.design_settings,
        solve: allow.solve,
        preform_tab: allow.preform_tab,
        advanced: allow.advanced,
        view_tabs: allow.view_tabs,
        file_ops: allow.file_ops,
        history: allow.history,
        concave_tier: allow.concave_tier,
    }
}

/// Step number `index` of its lesson as the Slint `GuideStepData` the panel renders
/// (`pleochroma_web_core::guide_runner::step_view`). `step` is from the web view, already in the
/// browser's words.
fn step_data(index: usize, step: &GuideStep) -> GuideStepData {
    let view = step_view(index, step);
    let actions: Vec<SharedString> = view
        .actions
        .iter()
        .map(|action| SharedString::from(action.as_str()))
        .collect();
    GuideStepData {
        title: view.title.as_str().into(),
        intro: view.intro.as_str().into(),
        actions: ModelRc::new(VecModel::from(actions)),
        check: view.check.as_str().into(),
        why: view.why.as_str().into(),
        waiting: view.waiting.as_str().into(),
        highlight_target: view.highlight_target.as_str().into(),
        completion: view.completion.as_str().into(),
        watches_ui: view.watches_ui,
        performable: view.performable,
        allow: guide_allow(view.allow),
    }
}

/// Pushes `guide`'s steps, id and title into `GuideModel` (without opening the panel).
fn push_guide(ui: &AppWindow, guide: &Guide) {
    let steps: Vec<GuideStepData> = guide
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| step_data(index, step))
        .collect();
    let model = ui.global::<GuideModel>();
    model.set_steps(ModelRc::new(VecModel::from(steps)));
    model.set_guide_id(guide.id.as_str().into());
    model.set_guide_title(guide.title.as_str().into());
}

/// Pushes `guide`, makes it the running lesson and opens its first step. Any lesson that was
/// open is replaced.
fn open_guide(ui: &AppWindow, guide: &Guide) {
    push_guide(ui, guide);
    with(|rt| {
        rt.active = Some(guide.clone());
        rt.events.clear();
        rt.perform_ticks = 0;
    });
    // A first step that locks the Render tab is set up the way it describes, on the Solid tab.
    let first_allows_render = guide
        .steps
        .first()
        .is_none_or(|step| step.allow.contains(&Group::ViewTabs));
    let app_model = ui.global::<AppModel>();
    if !first_allows_render && app_model.get_view_tab() == RENDER_TAB {
        app_model.set_view_tab(SOLID_TAB);
        app_model.invoke_view_tab_changed(SOLID_TAB);
    }
    ui.global::<GuideModel>().invoke_begin();
}

/// Whether the design's current solve is a closed solid: the editor's own "solved"
/// verdict that is not a problem.
fn solved_closed(app: &WebApp) -> bool {
    app.current_solved().is_some() && matches!(app.solve, SolveState::Solved { problem: false, .. })
}

/// What the window and the app state say besides the design, for `step`'s goal.
fn ui_facts(ui: &AppWindow, app: &WebApp, step: &GuideStep) -> UiFacts {
    let closed = solved_closed(app);
    // A goal that compares solved depths is given the depths of the cached solve, and only
    // once the editor reports a closed solid.
    let masts: Option<Vec<f64>> = if closed && step.goal.wants_solved_masts() {
        app.current_solved()
            .map(|tiers| tiers.iter().map(|tier| tier.mast).collect())
    } else {
        None
    };
    UiFacts {
        solved_closed: closed,
        view_tab: ui.global::<AppModel>().get_view_tab(),
        inspector_tab: ui.global::<InspectorModel>().get_tab(),
        masts,
    }
}

/// Reports an event: it counts for the rest of the current step, and the step is re-checked
/// at once. Does nothing while no lesson is open or starting.
///
/// This is the one entry point for the lessons' events (`pleochroma_editor::guide::EVENTS`, and
/// the Deep Solve and cutting events of `solving_events` / `viewing_events`): call it from the
/// place the action succeeds, after the state is updated. Safe to call from anywhere on the UI
/// thread, including while the app state is borrowed: the re-check then waits a moment and tries
/// once more, and every later refresh re-checks anyway. The Slint side raises the same events
/// with `GuideModel.event("name")`.
pub fn raise(ctx: &Ctx, name: &str) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let open = ui.global::<GuideModel>().get_open();
    let launching = with(|rt| rt.launch.is_some());
    if !open && !launching {
        return;
    }
    with(|rt| rt.events.record(name));
    // The design a lesson may have asked for is here: open it before the re-check.
    if name == NEW_DESIGN_CREATED && mark_arrived() {
        evaluate_launch(ctx);
    }
    check_now(ctx);
}

/// Reports `key` as an event: the walkthrough's New Design step (`NEW_DESIGN_CREATED`) is
/// reported from the two places a design is made from a template. The same as [`raise`].
pub fn notify(ctx: &Ctx, key: &str) {
    raise(ctx, key);
}

/// Notes that the design a launch waits for has been installed. Returns whether a launch is in
/// progress.
fn mark_arrived() -> bool {
    with(|rt| {
        rt.launch.as_mut().is_some_and(|launch| {
            launch.design_arrived = true;
            true
        })
    })
}

/// Evaluates the current lesson step's goal against the design and reports it to
/// `GuideModel.notify` when reached. A no-op while the guide is closed, on a manual step
/// (its goal never reads as reached), or while the current step already shows "Done".
/// Safe to call while something else holds the state (it then does nothing: every writer
/// ends in a refresh that calls it again).
pub fn check_progress(ctx: &Ctx) {
    run_check(ctx);
}

/// [`check_progress`]'s work. Returns `false` when the app state was being written and the check
/// could not run.
fn run_check(ctx: &Ctx) -> bool {
    let Some(ui) = ctx.ui.upgrade() else {
        return true;
    };
    let guide = ui.global::<GuideModel>();
    if !guide.get_open() || guide.get_step_done() {
        return true;
    }
    let Ok(index) = usize::try_from(guide.get_step_index()) else {
        return true;
    };
    let Ok(app) = ctx.state.try_borrow() else {
        return false;
    };
    let design = app.design.as_ref().map(|design| &design.session.design);
    let met = with(|rt| {
        let step = rt.active.as_ref()?.steps.get(index)?;
        let facts = ui_facts(&ui, &app, step);
        Some(judge::goal_is_met(
            step,
            design,
            &facts,
            rt.events.as_slice(),
        ))
    });
    drop(app);
    if met == Some(true) {
        guide.invoke_notify(guide.get_current().completion);
    }
    true
}

/// Re-checks the current step against the state as it is now: the stage first, then the goal.
fn check_now(ctx: &Ctx) {
    if let Some(ui) = ctx.ui.upgrade() {
        check_stage_alignment(&ui);
    }
    if !run_check(ctx) {
        // Someone is writing the state right now; their refresh ends in a check of its own,
        // and one more try a moment later covers a caller that does not refresh.
        let c = ctx.clone();
        Timer::single_shot(RECHECK_AFTER, move || {
            run_check(&c);
        });
    }
}

/// What the window shows of the current step's stage.
fn shown_of(ui: &AppWindow) -> Shown {
    let inspector = ui.global::<InspectorModel>();
    Shown {
        view_tab: ui.global::<AppModel>().get_view_tab(),
        inspector_tab: inspector.get_tab(),
        inspector_collapsed: inspector.get_collapsed(),
        render_allowed: ui.global::<GuideModel>().invoke_allows_view_tabs(),
    }
}

/// What the current step asks of the window, if a lesson is open.
fn current_wanted(ui: &AppWindow) -> Option<judge::Wanted> {
    let model = ui.global::<GuideModel>();
    if !model.get_open() {
        return None;
    }
    let index = usize::try_from(model.get_step_index()).ok()?;
    with(|rt| rt.active.as_ref().map(|guide| judge::wanted(guide, index)))
}

/// Tells `GuideModel` whether the window shows the tabs the current step needs, and what
/// "Show Me Where" would do.
fn check_stage_alignment(ui: &AppWindow) {
    let Some(wanted) = current_wanted(ui) else {
        return;
    };
    let model = ui.global::<GuideModel>();
    match judge::mismatch(&wanted, &shown_of(ui)) {
        Some(hint) => {
            model.set_stage_hint(hint.into());
            model.set_stage_mismatched(true);
        }
        None => model.set_stage_mismatched(false),
    }
}

/// "Show Me Where": switches to the view tab and the inspector tab the current step needs.
fn apply_stage(ui: &AppWindow) {
    let Some(wanted) = current_wanted(ui) else {
        return;
    };
    let (view, inspector) = judge::moves(&wanted, &shown_of(ui));
    if let Some(tab) = view {
        let app_model = ui.global::<AppModel>();
        app_model.set_view_tab(tab);
        app_model.invoke_view_tab_changed(tab);
    }
    if wanted.inspector_tab.is_some() {
        let model = ui.global::<InspectorModel>();
        if let Some(tab) = inspector {
            model.set_tab(tab);
        }
        model.set_collapsed(false);
    }
    check_stage_alignment(ui);
}

/// A step was entered (start, Next, an automatic advance, Back): forget the events of the step
/// before and re-check the step's goal and stage.
fn step_entered(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<GuideModel>();
    let revisiting = model.get_revisiting();
    if let Ok(index) = usize::try_from(model.get_step_index()) {
        with(|rt| {
            if let Some(active) = &rt.active {
                rt.events.enter(active, index, revisiting);
            }
            rt.perform_ticks = 0;
        });
    }
    check_now(ctx);
}

/// The 300 ms tick while a lesson is open: the stage always, the goal when it reads UI state.
fn poll(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    check_stage_alignment(&ui);
    if ui.global::<GuideModel>().get_current().watches_ui {
        run_check(ctx);
    }
}

/// Next on an unfinished step: runs the step's recipe, then judges the goal as for any other
/// route to it. The overlay layer's timer asks for ticks while the step is not done.
fn perform(ctx: &Ctx, index: i32) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<GuideModel>();
    let found = usize::try_from(index).ok().and_then(|index| {
        with(|rt| {
            let step = rt.active.as_ref()?.steps.get(index)?.clone();
            rt.perform_ticks = 0;
            Some((step, rt.performer?))
        })
    });
    let started = found.is_some_and(|(step, performer)| performer(ctx, &step));
    if !started {
        // Nothing to run: never promise what was not delivered.
        model.set_perform_busy(false);
        model.set_perform_failed(true);
    }
    check_now(ctx);
}

/// One tick while Next is doing a step: re-checks the goal, and gives up waiting after
/// [`PERFORM_PATIENCE_TICKS`].
fn perform_tick(ctx: &Ctx) {
    check_now(ctx);
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<GuideModel>();
    if !model.get_perform_busy() {
        return;
    }
    if model.get_step_done() {
        model.set_perform_busy(false);
        return;
    }
    let waited = with(|rt| {
        rt.perform_ticks = rt.perform_ticks.saturating_add(1);
        rt.perform_ticks
    });
    if waited >= PERFORM_PATIENCE_TICKS {
        model.set_perform_busy(false);
        model.set_perform_failed(true);
    }
}

/// Starts lesson `id` as the browser shows it, after arranging the state it starts from. A
/// lesson that cannot start says why in a toast and does not open.
pub fn start_guide(ctx: &Ctx, id: &str) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let Some(guide) = web_guide(id) else {
        show_message(ctx, MessageKind::Error, "That tutorial is not available.");
        return;
    };
    let has_design = ctx.state.borrow().design.is_some();
    match plan_start(&guide.starting_state, has_design) {
        StartPlan::Begin => open_guide(&ui, &guide),
        StartPlan::Blocked(reason) => show_message(ctx, MessageKind::Info, reason),
        StartPlan::CreateNew {
            template_index,
            spec,
        } => {
            with(|rt| {
                rt.launch = Some(Launch {
                    guide_id: guide.id.clone(),
                    ticks: 0,
                    design_arrived: false,
                });
            });
            // The lesson opens when the design has arrived: at once, or after the unsaved
            // changes question has been answered.
            let c = ctx.clone();
            confirm_discard(ctx, move || {
                install_lesson_design(&c, *spec, template_index);
            });
            evaluate_launch(ctx);
        }
        StartPlan::OpenLibrary { .. } => show_message(
            ctx,
            MessageKind::Info,
            "That tutorial starts from a library design, and this build cannot open one.",
        ),
    }
}

/// Installs the design a lesson starts from (the New Design dialog's own steps: the design
/// replaces the open one, the dialog closes, the design is solved and saved), then reports
/// `NEW_DESIGN_CREATED`, which opens the lesson that was waiting for it. Runs once nothing
/// unsaved is at stake.
fn install_lesson_design(ctx: &Ctx, spec: FreshDesignSpec, template_index: i32) {
    let name = usize::try_from(template_index)
        .ok()
        .and_then(|index| template_cards().into_iter().nth(index))
        .map_or_else(|| "Empty".to_owned(), |card| card.name);
    let session = EditorSession::from_template(spec, template_index);
    {
        let mut app = ctx.state.borrow_mut();
        app.replace_design(DesignState::new(
            session,
            DesignSource::Template(name.clone()),
        ));
        // A design the learner just created is unsaved until the first save.
        if let Some(design) = app.design.as_mut() {
            design.session.mark_unsaved();
        }
    }
    if let Some(ui) = ctx.ui.upgrade() {
        let gallery = ui.global::<GalleryModel>();
        gallery.set_open(false);
        gallery.set_selected_index(0);
        push_all(&ui, &ctx.state.borrow());
    }
    auto_solve(ctx);
    persist::schedule_save(ctx);
    show_message(ctx, MessageKind::Success, &format!("New design: {name}."));
    raise(ctx, NEW_DESIGN_CREATED);
}

/// Checks the launch in progress (if any): opens the lesson when its design has arrived, keeps
/// waiting while the editor is still deciding, forgets the launch when nothing will deliver
/// the design. Called right after the editor was asked, when the design arrives and on every
/// `GuideModel.launch-tick`.
fn evaluate_launch(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<GuideModel>();
    let Some(launch) = with(|rt| rt.launch.clone()) else {
        model.set_launch_pending(false);
        return;
    };
    let dialog_open = ui.global::<ConfirmModel>().get_open();
    match launch_status(&observe(&launch, dialog_open), LAUNCH_GRACE_TICKS) {
        LaunchStatus::Ready => {
            with(|rt| rt.launch = None);
            model.set_launch_pending(false);
            if let Some(guide) = web_guide(&launch.guide_id) {
                open_guide(&ui, &guide);
            }
        }
        LaunchStatus::Waiting => {
            with(|rt| {
                if let Some(launch) = rt.launch.as_mut() {
                    launch.ticks = if dialog_open {
                        0
                    } else {
                        launch.ticks.saturating_add(1)
                    };
                }
            });
            model.set_launch_pending(true);
        }
        LaunchStatus::Abandoned => {
            with(|rt| rt.launch = None);
            model.set_launch_pending(false);
        }
    }
}

/// The guide's place as the panel holds it now.
fn place_of(guide: &GuideModel<'_>) -> GuidePlace {
    GuidePlace {
        guide: guide.get_guide_id().to_string(),
        open: guide.get_open(),
        step: usize::try_from(guide.get_step_index()).unwrap_or(0),
        collapsed: guide.get_collapsed(),
        float_placed: guide.get_float_placed(),
        float_x: guide.get_float_x(),
        float_y: guide.get_float_y(),
    }
    .sanitized()
}

/// Writes the guide's place to the browser storage after a moment of quiet. Nothing is written
/// before a lesson has been started.
fn schedule_save(ctx: &Ctx) {
    let weak = ctx.ui.clone();
    SAVE.with(|timer| {
        timer.start(TimerMode::SingleShot, SAVE_DEBOUNCE, move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let guide = ui.global::<GuideModel>();
            if guide.get_guide_id().is_empty() {
                return;
            }
            let key = guide_place_key();
            match place_of(&guide).to_json() {
                Ok(json) => persist::write(&key, &json),
                Err(e) => persist::discard(&key, &e),
            }
        });
    });
}

/// The stored place: this build's, else the ten-step walkthrough's from an earlier one. An
/// entry that does not parse is removed.
fn stored_place() -> Option<GuidePlace> {
    let key = guide_place_key();
    if let Some(text) = persist::read(&key) {
        return match GuidePlace::from_json(&text) {
            Ok(place) => Some(place),
            Err(e) => {
                persist::discard(&key, &e);
                None
            }
        };
    }
    let text = persist::read(GUIDE_KEY)?;
    match GuideProgress::from_json(&text) {
        Ok(legacy) => Some(GuidePlace::from_legacy(&legacy)),
        Err(e) => {
            persist::discard(GUIDE_KEY, &e);
            None
        }
    }
}

/// Puts the stored place back (lesson, open, step, collapsed, panel position) and checks the
/// restored step's goal against the restored design. Call once at start-up, after the design has
/// been restored and pushed. A lesson the browser no longer offers stays closed.
pub fn restore(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let Some(place) = stored_place() else {
        return;
    };
    let Some(guide) = web_guide(&place.guide) else {
        return;
    };
    push_guide(&ui, &guide);
    let step = place.step_within(guide.steps.len());
    with(|rt| {
        rt.active = Some(guide);
        rt.events.clear();
    });
    let model = ui.global::<GuideModel>();
    model.set_step_index(i32::try_from(step).unwrap_or(0));
    model.set_collapsed(place.collapsed);
    model.set_float_placed(place.float_placed);
    model.set_float_x(place.float_x);
    model.set_float_y(place.float_y);
    model.set_open(place.open);
    if place.open {
        step_entered(ctx);
    }
}

/// Registers the runner's callbacks and the tutorial browser's. The steps are pushed when a
/// lesson starts ([`start_guide`]) or the stored place is restored ([`restore`]).
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let guide = ui.global::<GuideModel>();

    let c = ctx.clone();
    guide.on_start_guide(move |id| start_guide(&c, id.as_str()));
    let c = ctx.clone();
    guide.on_step_entered(move || step_entered(&c));
    let c = ctx.clone();
    guide.on_event(move |name| raise(&c, name.as_str()));
    let c = ctx.clone();
    guide.on_poll(move || poll(&c));
    let c = ctx.clone();
    guide.on_perform(move |index| perform(&c, index));
    let c = ctx.clone();
    guide.on_perform_tick(move || perform_tick(&c));
    let c = ctx.clone();
    guide.on_launch_tick(move || evaluate_launch(&c));
    guide.on_show_me_where({
        let weak = ctx.ui.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                apply_stage(&ui);
            }
        }
    });
    let c = ctx.clone();
    guide.on_finished(move |id| tutorials::record_finished(&c, id.as_str()));
    let c = ctx.clone();
    guide.on_state_changed(move || schedule_save(&c));

    perform::wire(ui);
    tutorials::wire(ui, ctx);
}
