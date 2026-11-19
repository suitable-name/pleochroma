//! The run loop: builds the window and the app state and wires the Slint callbacks to the core.
//!
//! It also pushes the first state to the UI, runs the event loop and finally tells the core
//! the app is stopping.
//!
//! Threading: the `AppState` lives in an `Rc<RefCell<..>>` on the UI thread. Code on
//! other threads (platform callbacks, lifecycle notifications) reaches it only through
//! `slint::invoke_from_event_loop` / `Weak::upgrade_in_event_loop`.
//!
//! The Back key: the `Navigation` global (`ui/navigation.slint`) names what Back would
//! close (dialog, info sheet, a Design screen or sheet, a Library or More page, the
//! Design destination) as a target plus a version, and reports every change to the
//! platform (`Platform::update_back_target`). On Android 13 and later the activity owns
//! the dispatcher callback and answers with `nativeBackCommitted` / `nativeBackProgress`
//! (`platform_android/back.rs`, `crate::navigation::back`); the pinned Slint backend
//! (`vendor/slint-android-activity-1.18.1/PATCH.md`) leaves Back to the app. Older
//! versions keep the legacy key route: `ui/app.slint` handles `Key.Back` (and Escape) in
//! a root `FocusScope`, and a key nothing consumed is rejected so the activity's default
//! Back action runs. At the root Back is left to Android.

mod ui_state;

use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::Arc, time::Duration};

use pleochroma::optics::raytracer::{Estimator, set_default_estimator};
use pleochroma_mobile_core::{
    app_state::{AppState, Lifecycle},
    layout::Destination,
    platform::Platform,
    settings::ThemeChoice,
};
use slint::ComponentHandle;

use crate::{AppWindow, PropsModel, Shell, ViewportModel, commands, push};

/// Options for [`run_with`] that only the desktop preview sets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunOptions {
    /// The window's initial logical size in px (width, height). `None` keeps the
    /// size the platform gives the window (always the case on Android).
    pub initial_size: Option<(f32, f32)>,
    /// Opens this destination instead of the remembered one.
    pub destination: Option<Destination>,
    /// Saves a PNG of the window to this path shortly after start, then quits (for layout
    /// checks without a device).
    pub screenshot: Option<PathBuf>,
    /// Starts on a new design from this template card (0 is the blank design), replacing
    /// whatever the autosave restored.
    pub template: Option<usize>,
    /// Opens this tab of the Design page (0 Tiers, 1 Stone, 2 Light, 3 Checks).
    pub tab: Option<i32>,
    /// Puts the Design page's sheet at this detent (0 peek, 1 half, 2 full).
    pub sheet: Option<i32>,
    /// Puts the Design viewport in this mode (0 Solid, 1 Diagram, 2 Render).
    pub view: Option<i32>,
    /// Preview-only explicit image engine: 0 Path-Traced or 1 Ray-Traced.
    pub engine: Option<u8>,
    /// How long the screenshot run waits before capturing, instead of the default (longer in
    /// Render mode). A large library takes a while to open.
    pub screenshot_delay: Option<Duration>,
    /// Preview-only: starts with this theme instead of the stored one (not saved).
    pub theme: Option<ThemeChoice>,
    /// Preview-only: opens Cutting Mode at its first step once the design is solved.
    pub cutting: bool,
    /// Preview-only: unlocks the Developer page without the About taps.
    pub developer: bool,
    /// Preview-only: opens this More sub-page (6 Developer, 7 log viewer, and the older ones).
    pub more_page: Option<i32>,
    /// Preview-only: writes the fixed sample logs into the logs folder before the first frame.
    pub developer_sample_logs: bool,
    /// Preview-only: freezes the render's preparing state at this `(done, total)` of the
    /// Path-Traced shader build, so its screens can be shot.
    pub hold_preparing: Option<(u32, u32)>,
}

/// How long the screenshot run waits for the first frames before capturing.
#[cfg(feature = "desktop-preview")]
const SCREENSHOT_DELAY: Duration = Duration::from_millis(2500);

/// The longest a Render-mode screenshot waits for the picture to finish. Ray-Traced may first
/// wait for its GPU kernels, which a cold shader cache builds at start-up.
#[cfg(feature = "desktop-preview")]
const RENDER_SCREENSHOT_LIMIT: Duration = Duration::from_secs(180);

/// How often a Render-mode screenshot checks whether the picture is finished.
#[cfg(feature = "desktop-preview")]
const RENDER_POLL: Duration = Duration::from_millis(250);

/// How long a Render-mode screenshot waits after the picture finished, so the finished picture
/// and its status are on screen.
#[cfg(feature = "desktop-preview")]
const RENDER_SETTLE: Duration = Duration::from_millis(600);

/// How often the preview's `--cutting` tries to open Cutting Mode while the start-up solve runs,
/// and how many tries it makes before it gives up with the app's "not solved" toast (about 4 s).
const CUTTING_RETRY: Duration = Duration::from_millis(250);
const CUTTING_ATTEMPTS: u32 = 16;

/// Runs the app until its window closes.
///
/// `platform` provides files, sharing, printing and the rest (the Android shim, or
/// the desktop preview's stand-in). On Android the caller initializes the Slint
/// backend first (`slint::android::init`).
///
/// # Errors
/// When the window or the event loop cannot be created or run.
pub fn run(platform: &Arc<dyn Platform>) -> Result<(), slint::PlatformError> {
    run_with(platform, &RunOptions::default())
}

/// [`run`] with [`RunOptions`].
///
/// # Errors
/// When the window or the event loop cannot be created or run.
pub fn run_with(
    platform: &Arc<dyn Platform>,
    options: &RunOptions,
) -> Result<(), slint::PlatformError> {
    // The app's fixed estimator; must be set before any frame is prepared.
    set_default_estimator(Estimator::APP_DEFAULT);
    let window = AppWindow::new()?;
    if let Some((width, height)) = options.initial_size {
        window
            .window()
            .set_size(slint::LogicalSize::new(width, height));
    }

    let state = Rc::new(RefCell::new(AppState::new(Arc::clone(platform))));
    if let Some(destination) = options.destination {
        state.borrow_mut().destination = destination;
    }
    if let Some(theme) = options.theme {
        // In memory only: the stored setting is left alone.
        state.borrow_mut().settings.theme = theme;
    }
    ui_state::install(Rc::clone(&state));

    commands::wire(&window, &state);
    commands::register_incoming(&window, platform);
    if let Some(index) = options.template {
        let created =
            pleochroma_mobile_core::document::new_from_template(&mut state.borrow_mut(), index);
        created.map_or_else(
            |message| tracing::warn!(%message, "the requested template was not opened"),
            |()| crate::design::after_edit(&window, &state),
        );
    }
    push::all(&window, &state.borrow());
    apply_developer_options(&window, &state.borrow(), options);
    apply_view_options(&window, options);
    if options.cutting {
        open_cutting_mode_when_solved(&window, &state);
    }

    #[cfg(feature = "desktop-preview")]
    if let Some(path) = options.screenshot.clone() {
        match options.screenshot_delay {
            Some(delay) => schedule_screenshot(&window, path, delay),
            // A render is captured once its picture is finished, not after a fixed time.
            None if options.view == Some(2) => screenshot_when_rendered(&window, &state, path),
            None => schedule_screenshot(&window, path, SCREENSHOT_DELAY),
        }
    }

    tracing::info!("mobile app: event loop starting");
    let result = window.run();

    // Cancel long operations before joining their worker queues.
    state.borrow_mut().operations.cancel();
    crate::remote::shutdown();
    crate::design::shutdown();
    state.borrow_mut().shutdown();
    ui_state::clear();
    tracing::info!("mobile app: event loop ended");
    result
}

/// Opens Cutting Mode at the first step as soon as the design's solve has landed (the preview's
/// `--cutting`): it retries quietly every [`CUTTING_RETRY`], and only the last try shows the
/// app's "not solved" toast.
fn open_cutting_mode_when_solved(window: &AppWindow, state: &Rc<RefCell<AppState>>) {
    try_open_cutting_mode(window.as_weak(), Rc::clone(state), 1);
}

fn try_open_cutting_mode(weak: slint::Weak<AppWindow>, state: Rc<RefCell<AppState>>, attempt: u32) {
    slint::Timer::single_shot(CUTTING_RETRY, move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        if attempt >= CUTTING_ATTEMPTS {
            crate::cut::mode::open(&window, &state, 0);
        } else if !crate::cut::mode::open_quietly(&window, &state, 0) {
            try_open_cutting_mode(weak, state, attempt + 1);
        }
    });
}

/// Applies the preview-only developer options: the unlocked page, the sample logs, the More
/// sub-page and the frozen preparing state. Runs before [`apply_view_options`], so a frozen
/// preparing state is in place before the render view starts a render.
fn apply_developer_options(window: &AppWindow, state: &AppState, options: &RunOptions) {
    if options.developer {
        state.diagnostics.unlock().force();
    }
    if options.developer_sample_logs
        && let Err(error) = state.diagnostics.store().write_sample_logs()
    {
        tracing::warn!(kind = ?error.kind(), "the sample logs could not be written");
    }
    if let Some(page) = options.more_page {
        window.global::<Shell>().set_more_page(page.clamp(0, 7));
    }
    if let Some(counts) = options.hold_preparing {
        pleochroma::diag::set_prepare_override(Some(counts));
    }
}

/// Applies the preview-only options that select what is shown: the engine, the Design tab,
/// the sheet detent and the viewport mode. Runs after the first push, so every surface is wired.
fn apply_view_options(window: &AppWindow, options: &RunOptions) {
    if let Some(engine) = options.engine {
        window
            .global::<crate::RenderModel>()
            .invoke_engine_selected(i32::from(engine));
    }
    if let Some(tab) = options.tab {
        window
            .global::<PropsModel>()
            .set_design_tab(tab.clamp(0, 3));
    }
    if let Some(detent) = options.sheet {
        window
            .global::<Shell>()
            .set_sheet_detent(detent.clamp(0, 2));
    }
    if let Some(view) = options.view {
        window
            .global::<ViewportModel>()
            .invoke_view_mode_chosen(view.clamp(0, 2));
    }
}

/// Captures the window to `path` once the render is finished (or failed), checking every
/// [`RENDER_POLL`]; after [`RENDER_SCREENSHOT_LIMIT`] it captures whatever is on screen.
#[cfg(feature = "desktop-preview")]
fn screenshot_when_rendered(window: &AppWindow, state: &Rc<RefCell<AppState>>, path: PathBuf) {
    let deadline = std::time::Instant::now() + RENDER_SCREENSHOT_LIMIT;
    poll_render(window.as_weak(), Rc::clone(state), path, deadline);
}

/// Whether a Render-mode screenshot may be taken now.
///
/// Only a finished or failed render ends the wait; every other state keeps it going,
/// preparing included.
#[cfg(feature = "desktop-preview")]
const fn render_has_finished(status: &pleochroma_mobile_core::render::RenderStatus) -> bool {
    use pleochroma_mobile_core::render::RenderStatus;

    match status {
        RenderStatus::Done { .. } | RenderStatus::Failed(_) => true,
        RenderStatus::Idle
        | RenderStatus::Tracing { .. }
        | RenderStatus::Paused(_)
        | RenderStatus::Preparing { .. } => false,
    }
}

#[cfg(feature = "desktop-preview")]
fn poll_render(
    weak: slint::Weak<AppWindow>,
    state: Rc<RefCell<AppState>>,
    path: PathBuf,
    deadline: std::time::Instant,
) {
    slint::Timer::single_shot(RENDER_POLL, move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let finished = render_has_finished(&state.borrow().render.status());
        if finished {
            schedule_screenshot(&window, path, RENDER_SETTLE);
        } else if std::time::Instant::now() >= deadline {
            tracing::warn!("the render did not finish in time; the screenshot shows it unfinished");
            schedule_screenshot(&window, path, Duration::ZERO);
        } else {
            poll_render(weak, state, path, deadline);
        }
    });
}

/// Captures the window to `path` as a PNG after `delay`, then quits the event loop.
/// Failures are logged; the loop quits either way.
#[cfg(feature = "desktop-preview")]
fn schedule_screenshot(window: &AppWindow, path: PathBuf, delay: Duration) {
    let weak = window.as_weak();
    slint::Timer::single_shot(delay, move || {
        if let Some(window) = weak.upgrade() {
            match window.window().take_snapshot() {
                Ok(buffer) => {
                    let saved = image::save_buffer(
                        &path,
                        buffer.as_bytes(),
                        buffer.width(),
                        buffer.height(),
                        image::ExtendedColorType::Rgba8,
                    );
                    saved.map_or_else(
                        |error| tracing::warn!(%error, "screenshot could not be saved"),
                        |()| tracing::info!(path = %path.display(), "screenshot saved"),
                    );
                }
                Err(error) => tracing::warn!(%error, "screenshot could not be taken"),
            }
        }
        if let Err(error) = slint::quit_event_loop() {
            tracing::warn!(%error, "the event loop could not be quit after the screenshot");
        }
    });
}

/// Tells the core about an Android lifecycle change (`onPause`, `onResume`, `onStop`).
///
/// Callable from any thread; the event is queued to the UI thread. Does nothing (but
/// logs) before [`run`] started or after it ended.
pub fn notify_lifecycle(event: Lifecycle) {
    if !ui_state::is_running() {
        tracing::debug!(
            ?event,
            "lifecycle event ignored: the run loop is not active"
        );
        return;
    }
    let queued = slint::invoke_from_event_loop(move || {
        if matches!(event, Lifecycle::Paused | Lifecycle::Stopping) {
            crate::remote::interrupt();
        }
        if matches!(event, Lifecycle::Resumed) {
            // The network browse ended when the app stopped: a remote failure still on show
            // may look for the computer again.
            crate::design::render::forget_failure();
        }
        let handled = ui_state::with_state(|state| {
            state.lifecycle(event);
            // Cutting Mode lets the screen sleep while paused and keeps it on again after.
            crate::cut::mode::on_lifecycle(state, event);
        });
        if handled.is_some() {
            ui_state::refresh_editor();
        }
        if handled.is_none() {
            tracing::warn!(
                ?event,
                "lifecycle event dropped: no app state on the UI thread"
            );
        }
    });
    if let Err(error) = queued {
        tracing::warn!(?event, %error, "lifecycle event could not be queued to the UI thread");
    }
}
