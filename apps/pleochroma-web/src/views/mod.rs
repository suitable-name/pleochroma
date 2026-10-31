//! The Solid and Diagram tabs: the desktop's solid viewport in its
//! Solid (view mode 0) and Diagram (view mode 3) modes, drawn on the main thread
//! by `pleochroma_solid::preview` -- the SAME planner/state machine/rasterizer the
//! desktop's two worker threads run, so the same design, pose and size give the
//! same pixels (`pleochroma_solid`'s `preview::tests` pin that against bytes
//! recorded from the desktop).
//!
//! # How a frame comes about
//!
//! [`refresh`] compares what the views last showed (design generation, solve,
//! selection, multi-selection, camera, view size, the view's own options) with
//! the app's state now and does the least work that brings the frame up to date,
//! exactly the desktop's three request kinds:
//! - **replan** (`PreviewPipeline::plan` + `render_planned`, the desktop's PLAN
//!   worker + `RedrawRequest::Planned`) after an edit, a solve, a selection change
//!   or a Preform/Cut/enlarged-panel change, budgeted by
//!   `live_update::DEFAULT_PREVIEW_BUDGET` against a `performance.now()` clock;
//! - **reproject** (`RedrawRequest::Reproject`) for a camera move, a resize or a
//!   tab switch;
//! - **overlay** (`RedrawRequest::UpdateFacetOverlay`) for hover, click and the
//!   multi-selection highlight.
//!
//! A replan that comes back `Stale` (the subgraph re-solve overran the budget)
//! shows the previous planes with the edited tiers outlined as pending, exactly as
//! the desktop does, and replans once more on the next event-loop turn from the
//! late result it chained forward. A design with no usable masts (first load, a
//! tier added or removed) is planned right away when the desktop would solve it
//! synchronously (`solve_policy::should_solve_synchronously`, or every tier
//! pinned); otherwise the views keep the last frame, ask the app for a solve
//! (`app::solve::with_solved`, answered by the solve Worker for larger designs) and
//! replan when it arrives.
//!
//! # When [`refresh`] runs
//!
//! On every input in the views (coalesced to one run per event-loop turn by
//! [`request_refresh`]), and from a 100 ms poll while a Solid/Diagram tab is shown
//! -- which is how an edit, undo, load or an asynchronous solve result reaches the
//! views without any other module having to call in.
//!
//! # Direct manipulation
//!
//! [`manip`] adds the desktop's mouse-driven tools to the Solid view: the angle, depth
//! and index drag handles on the selected facet and the Slice tool. It hooks into
//! [`refresh`] (`manip::frame_landed` after every shown frame, `manip::expire` before
//! each run, and `replan_now` plans the provisional slice's design while a session
//! exists) and into the click handler; see its own module doc comment.
//!
//! # Cutting Mode's step pictures
//!
//! Cutting Mode (`crate::editor::cutting_mode`) shows the stone after each step through the
//! Solid view's own picture (`SolidModel.frame`). [`hold_for_cutting`] takes a note of the view
//! (tab, camera, selection, Cut slider) and switches to the Solid tab with the camera fitted to
//! the rough; [`show_cut_step`] sets the step's `cut_steps` (the planner draws the stone after
//! that many steps in cutting order, concave tiers included, through
//! `pleochroma_solid::preview::view::plan_job_with_cut_steps`) and selects the step's tier row;
//! [`release_for_cutting`] puts everything back.
//!
//! # The Cut slider
//!
//! The toolbar's Cut slider walks the same cutting steps in the same order as Cutting Mode and
//! the desktop: handle `k` is the stone after the first `k` steps in cutting order, so slider
//! position `k` and Cutting Mode step `k` draw one stone. `SolidModel.tier-cutoff` stores the
//! position in the desktop's encoding (`-1` finished, `-2` rough, `n >= 0` through steps `0..=n`);
//! `cut_slider::publish` writes the step count, handle and label from it on every refresh, and
//! `refresh` hands the planner `cut_steps` (never the table-row `tier_cutoff`). The mapping and
//! its tests are `pleochroma_web_core::cut_slider`.
//!
//! # Selection API (for the tier table)
//!
//! - `WebApp::selected_tier: Option<usize>` is the selected tier. A click on a
//!   facet (Solid) or a facet fill (Diagram) sets it; a click that misses clears
//!   it; Escape clears it; Up/Down/PageUp/PageDown step it. Like the desktop, every
//!   such plain selection also empties `EditorSession::multi_selected`.
//! - `EditorSession::multi_selected` is the Ctrl+click group; its tiers' facets
//!   are outlined in both views.
//! - Change either and the views follow within one poll; call
//!   [`request_refresh`] (or [`select_tier`], which also applies the desktop's
//!   multi-selection rule) for an immediate redraw.
//! - The last index-wheel tooth clicked in the Diagram is
//!   `SolidModel.clicked-tooth` (the desktop's `diagram_clicked_tooth`, which its
//!   tier table highlights).

mod cut_slider;
mod input;
mod manip;
mod present;
mod refresh;
mod state;

use crate::{AppModel, AppWindow, ManipulateModel, SolidModel, app::Ctx};
use pleochroma::geometry::stone_metrics::{SolidStatus, build_solid_mesh};
use pleochroma_cut_core::Design;
use pleochroma_solid::preview::{DEFAULT_MESH_BOUNDING_RADIUS, camera};
use slint::{ComponentHandle, Timer, TimerMode};
use std::{cell::RefCell, collections::BTreeSet, time::Duration};

pub use refresh::refresh;

/// How often the views check the app state for changes they did not cause.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

thread_local! {
    /// The views' pipeline and last-frame state (single-threaded, like `WebApp`).
    static VIEWS: RefCell<state::ViewsState> = RefCell::new(state::ViewsState::default());
    /// The one-shot timer [`request_refresh`] restarts, so a burst of input events
    /// in one event-loop turn draws once.
    static REFRESH_TIMER: Timer = Timer::default();
    /// The poll timer (kept alive for the page's lifetime).
    static POLL_TIMER: Timer = Timer::default();
}

/// Wires the `SolidModel` callbacks and starts the poll. Call once, after
/// `app::callbacks::wire`.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    input::wire(ui, ctx);
    manip::wire(ui, ctx);
    let poll = ctx.clone();
    POLL_TIMER.with(|timer| {
        timer.start(TimerMode::Repeated, POLL_INTERVAL, move || refresh(&poll));
    });
}

/// Brings the Solid/Diagram view up to date on the next event-loop turn;
/// repeated calls within one turn coalesce into one [`refresh`].
pub fn request_refresh(ctx: &Ctx) {
    let ctx = ctx.clone();
    REFRESH_TIMER.with(|timer| {
        timer.start(TimerMode::SingleShot, Duration::ZERO, move || refresh(&ctx));
    });
}

/// Selects `tier` (or nothing) the way a plain click does on the desktop: sets
/// `WebApp::selected_tier`, empties `EditorSession::multi_selected`, and redraws.
pub fn select_tier(ctx: &Ctx, tier: Option<usize>) {
    {
        let mut app = ctx.state.borrow_mut();
        let tier_count = app
            .design
            .as_ref()
            .map_or(0, |d| d.session.design.tiers.len());
        app.selected_tier = tier.filter(|&t| t < tier_count);
        if let Some(design) = app.design.as_mut() {
            design.session.multi_selected.clear();
        }
    }
    request_refresh(ctx);
}

/// The Solid tab's index in `AppModel.view-tab`.
const SOLID_TAB: i32 = 1;

/// `SolidModel.tier-cutoff` for the whole design.
const CUTOFF_ALL: i32 = -1;

/// `camera::standard_view`'s kind for Fit.
const FIT_VIEW: i32 = 5;

/// What the views showed before Cutting Mode took them, for [`release_for_cutting`].
#[derive(Debug, Clone)]
pub struct HeldView {
    /// `AppModel.view-tab`.
    tab: i32,
    /// The shared camera: yaw, pitch and distance.
    camera: (f32, f32, f32),
    /// The selected tier.
    selected_tier: Option<usize>,
    /// The Ctrl+click multi-selection.
    multi_selected: BTreeSet<usize>,
    /// `SolidModel.tier-cutoff`, the Cut slider.
    tier_cutoff: i32,
}

/// The distance from the centre to the farthest corner of the design's rough, `None` when its
/// planes close no solid. No step draws a stone bigger than this one.
fn rough_radius(design: &Design) -> Option<f64> {
    match build_solid_mesh(&design.preform.planes()) {
        SolidStatus::Closed(mesh) => Some(
            mesh.positions
                .iter()
                .map(|position| position.length())
                .fold(0.0_f64, f64::max),
        ),
        _ => None,
    }
}

/// Takes the views for Cutting Mode: the Solid tab (the Diagram and the Render tab draw no
/// step pictures), the Slice tool off, nothing selected, the Cut slider at "all", and the
/// camera on the standard 3/4 pose fitted to the rough, so the stone after every step is whole
/// in the picture. Returns what was there, to give back with [`release_for_cutting`].
///
/// `None` when the page is going away.
pub fn hold_for_cutting(ctx: &Ctx) -> Option<HeldView> {
    let ui = ctx.ui.upgrade()?;
    let manip = ui.global::<ManipulateModel>();
    if manip.get_slice_mode() {
        // The Slice tool plans its own provisional design, which has no steps.
        manip.set_slice_mode(false);
        manip.invoke_slice_toggled();
    }
    let solid = ui.global::<SolidModel>();
    let app_model = ui.global::<AppModel>();
    let held = {
        let mut app = ctx.state.borrow_mut();
        let held = HeldView {
            tab: app_model.get_view_tab(),
            camera: (app.view.yaw, app.view.pitch, app.view.distance),
            selected_tier: app.selected_tier,
            multi_selected: app
                .design
                .as_ref()
                .map(|design| design.session.multi_selected.clone())
                .unwrap_or_default(),
            tier_cutoff: solid.get_tier_cutoff(),
        };
        let radius = app
            .design
            .as_ref()
            .and_then(|design| rough_radius(&design.session.design))
            .unwrap_or(DEFAULT_MESH_BOUNDING_RADIUS);
        let pose = camera::standard_view(FIT_VIEW, camera::RESET_POSE, radius);
        app.view.yaw = pose.yaw;
        app.view.pitch = pose.pitch;
        app.view.distance = pose.distance;
        app.selected_tier = None;
        if let Some(design) = app.design.as_mut() {
            design.session.multi_selected.clear();
        }
        held
    };
    if solid.get_tier_cutoff() != CUTOFF_ALL {
        solid.set_tier_cutoff(CUTOFF_ALL);
    }
    if held.tab != SOLID_TAB {
        app_model.set_view_tab(SOLID_TAB);
        app_model.invoke_view_tab_changed(SOLID_TAB);
    }
    request_refresh(ctx);
    Some(held)
}

/// Shows the stone after a step: `cut_steps` is the number of steps cut (`None` for the
/// finished stone, `Some(0)` for the rough), `tier_row` the tier-table row of the step, which
/// is selected so the picture highlights its facets (a concave row has no facet to highlight
/// here yet and selects nothing).
pub fn show_cut_step(ctx: &Ctx, cut_steps: Option<usize>, tier_row: Option<usize>) {
    VIEWS.with(|cell| cell.borrow_mut().cut_steps = cut_steps);
    {
        let mut app = ctx.state.borrow_mut();
        let tier_count = app
            .design
            .as_ref()
            .map_or(0, |design| design.session.design.tiers.len());
        app.selected_tier = tier_row.filter(|&row| row < tier_count);
    }
    request_refresh(ctx);
}

/// Gives the views back the way [`hold_for_cutting`] found them.
pub fn release_for_cutting(ctx: &Ctx, held: &HeldView) {
    VIEWS.with(|cell| cell.borrow_mut().cut_steps = None);
    {
        let mut app = ctx.state.borrow_mut();
        let tier_count = app
            .design
            .as_ref()
            .map_or(0, |design| design.session.design.tiers.len());
        (app.view.yaw, app.view.pitch, app.view.distance) = held.camera;
        app.selected_tier = held.selected_tier.filter(|&tier| tier < tier_count);
        if let Some(design) = app.design.as_mut() {
            design.session.multi_selected = held
                .multi_selected
                .iter()
                .copied()
                .filter(|&tier| tier < tier_count)
                .collect();
        }
    }
    if let Some(ui) = ctx.ui.upgrade() {
        let solid = ui.global::<SolidModel>();
        if solid.get_tier_cutoff() != held.tier_cutoff {
            solid.set_tier_cutoff(held.tier_cutoff);
        }
        let app_model = ui.global::<AppModel>();
        if app_model.get_view_tab() != held.tab {
            app_model.set_view_tab(held.tab);
            app_model.invoke_view_tab_changed(held.tab);
        }
    }
    request_refresh(ctx);
}
