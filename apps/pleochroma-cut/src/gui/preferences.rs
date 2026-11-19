//! App-wide preferences: the Simple/Advanced switch, the UI scale, high contrast and the Tilt
//! Average Range.
//!
//! It also holds the larger drag handles and the tutorial resets, plus the Snap/Slice pills
//! the Solid viewport remembers.
//!
//! The Slint side is `ui/models/preferences.slint` (`PreferencesModel`), the Preferences
//! dialog (`ui/components/preferences_dialog.slint`, Edit > Preferences..., Ctrl+Comma) and
//! the header's "Simple | Advanced" pill. This module is the Rust half:
//!
//! - [`apply_loaded`] seeds the models from the loaded `AppSettings` at start-up;
//! - [`setup_preferences_callbacks`] saves each change through the `SettingsPersister` and
//!   applies it;
//! - high contrast reaches EVERY window: each window's own `Theme` instance (a Slint
//!   global exists once per window) is registered as a sink, so the main window, the
//!   compare window and the rough planner all switch together, however late they open;
//! - the Tilt Average Range reaches every window that words a tilt figure: the range in force
//!   is [`display_tilt_window_deg`], and each window or presenter registers a sink with
//!   [`register_tilt_range_sink`] that is told every change (it sets the `TiltWindows` words
//!   and shows the tilt rows again from the kept profiles, never measuring);
//! - [`saved_ui_scale_factor`] turns the saved UI scale into `SLINT_SCALE_FACTOR`; `main`
//!   sets it before any window exists, so a change needs a restart.

use crate::{
    CompareWindow, CrystalAxisModel, LocateWindow, MainWindow, ManipulateModel, PreferencesModel,
    RigWindow, RoughPlannerWindow, Theme, TiltWindows,
    bridge::render_thread::RenderContext,
    gui::{
        external_links::open_local_path,
        tutorial_events::{raise, raise_weak},
    },
    settings::{
        SettingsPersister,
        model::{
            AppSettings, UI_SCALE_CHOICES, UiMode, clamp_path_cache_disk_limit_gb,
            ui_scale_factor_text,
        },
        store::{default_render_cache_dir, peek_ui_scale_percent},
    },
};
use pleochroma::renderer::path_cache::lru;
use pleochroma_editor::{
    guide::viewing_events as events,
    metric_deltas::{
        DEFAULT_DISPLAY_TILT_WINDOW_DEG, DISPLAY_TILT_WINDOW_CHOICES_DEG,
        display_tilt_window_caption, display_tilt_window_plain,
    },
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::{Cell, RefCell},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
};

/// How much larger the Solid viewport's handles are drawn (and grabbed) with the "Larger
/// handles" preference: markers, strokes and the hit radius all scale by this.
const LARGE_HANDLE_SCALE: f32 = 1.6;

/// `ManipulateModel.handle_scale` for the "Larger handles" preference.
const fn handle_scale(large_handles: bool) -> f32 {
    if large_handles {
        LARGE_HANDLE_SCALE
    } else {
        1.0
    }
}

// --- UI scale -----------------------------------------------------------------------

/// The scale combo's first entry: follow the operating system.
const AUTOMATIC_LABEL: &str = "Automatic";

/// The labels of the scale combo: "Automatic", then one per [`UI_SCALE_CHOICES`] entry
/// ("75 %", ...), in that order.
fn ui_scale_labels() -> Vec<String> {
    std::iter::once(AUTOMATIC_LABEL.to_owned())
        .chain(
            UI_SCALE_CHOICES
                .iter()
                .map(|percent| format!("{percent} %")),
        )
        .collect()
}

/// The percentage combo entry `index` stands for: `0` (Automatic) for the first entry and
/// for anything out of range.
fn ui_scale_percent_for_index(index: i32) -> u16 {
    usize::try_from(index)
        .ok()
        .and_then(|entry| entry.checked_sub(1))
        .and_then(|choice| UI_SCALE_CHOICES.get(choice))
        .copied()
        .unwrap_or(0)
}

/// The combo entry for a stored `percent`: the entry of that percentage, else Automatic.
fn ui_scale_index_for_percent(percent: u16) -> i32 {
    UI_SCALE_CHOICES
        .iter()
        .position(|&choice| choice == percent)
        .and_then(|choice| i32::try_from(choice + 1).ok())
        .unwrap_or(0)
}

/// What `SLINT_SCALE_FACTOR` should be set to for the saved `percent`. `None` leaves it
/// alone: Automatic, a percentage that is not offered, or a variable the user has set
/// themselves (their value wins).
const fn scale_factor_override(percent: u16, user_set_variable: bool) -> Option<&'static str> {
    if user_set_variable {
        None
    } else {
        ui_scale_factor_text(percent)
    }
}

/// The value to put in `SLINT_SCALE_FACTOR` for the saved UI scale, or `None` to leave the
/// operating system's scale in charge.
///
/// Reads the settings file directly (see `settings::store::peek_ui_scale_percent`) because
/// the scale has to be in place before the first window exists, which is earlier than the
/// normal settings load. `main` calls this first thing, before any thread starts.
#[must_use]
pub fn saved_ui_scale_factor() -> Option<&'static str> {
    let user_set = std::env::var_os("SLINT_SCALE_FACTOR").is_some_and(|value| !value.is_empty());
    scale_factor_override(peek_ui_scale_percent(), user_set)
}

// --- High contrast across windows ----------------------------------------------------

/// Applies the high-contrast flag to one window's `Theme`; returns `false` once that
/// window is gone, so the registry can forget it.
type ThemeSink = Box<dyn Fn(bool) -> bool>;

thread_local! {
    /// The current high-contrast state, so a window created later starts in it. UI-thread
    /// only, like every Slint handle.
    static HIGH_CONTRAST: Cell<bool> = const { Cell::new(false) };
    /// One sink per window that shows the app's colours.
    static THEME_SINKS: RefCell<Vec<ThemeSink>> = const { RefCell::new(Vec::new()) };
}

/// The high-contrast state for threads other than the UI thread (the Solid preview's plan
/// worker reads it; `HIGH_CONTRAST` above is UI-thread only).
static HIGH_CONTRAST_SHARED: AtomicBool = AtomicBool::new(false);

/// Whether the high-contrast palette is on right now (UI thread only).
pub(crate) fn high_contrast_enabled() -> bool {
    HIGH_CONTRAST.get()
}

/// Whether the high-contrast palette is on right now, from any thread. The Solid viewport's
/// plan worker draws its facet style in that palette.
pub(crate) fn high_contrast_flag() -> bool {
    HIGH_CONTRAST_SHARED.load(Ordering::Relaxed)
}

/// Registers `sink` and brings it up to the current state at once. A sink that reports its
/// window gone (`false`) is not kept.
pub(crate) fn register_theme_sink(sink: impl Fn(bool) -> bool + 'static) {
    if sink(high_contrast_enabled()) {
        THEME_SINKS.with_borrow_mut(|sinks| sinks.push(Box::new(sink)));
    }
}

/// Switches the palette in every registered window, dropping the sinks of closed ones.
fn apply_high_contrast(on: bool) {
    HIGH_CONTRAST.set(on);
    HIGH_CONTRAST_SHARED.store(on, Ordering::Relaxed);
    THEME_SINKS.with_borrow_mut(|sinks| sinks.retain(|sink| sink(on)));
}

// --- Tilt average range ----------------------------------------------------------------

/// Told every new Tilt Average Range; answers `false` once its window is gone.
type TiltRangeSink = Box<dyn FnMut(u8) -> bool>;

thread_local! {
    /// One sink per window or presenter that words or shows a tilt figure.
    static TILT_RANGE_SINKS: RefCell<Vec<TiltRangeSink>> = const { RefCell::new(Vec::new()) };
}

/// The Tilt Average Range in force, for threads other than the UI thread too (Baseline's worker
/// and Compare's tilt thread read it when they present).
static TILT_RANGE_SHARED: AtomicU8 = AtomicU8::new(DEFAULT_DISPLAY_TILT_WINDOW_DEG);

/// The range in force, in degrees each way from table up: 15, 20, 30, 45, 60 or 90 (the default,
/// every tilt). Set at start-up and on every change.
#[must_use]
pub fn display_tilt_window_deg() -> u8 {
    TILT_RANGE_SHARED.load(Ordering::Relaxed)
}

/// Registers `sink` and brings it up to the range in force at once. A sink that reports its
/// window gone (`false`) is not kept, then or later.
pub(crate) fn register_tilt_range_sink(mut sink: impl FnMut(u8) -> bool + 'static) {
    if sink(display_tilt_window_deg()) {
        TILT_RANGE_SINKS.with_borrow_mut(|sinks| sinks.push(Box::new(sink)));
    }
}

/// Tells every registered sink `degrees`, dropping the ones whose window is gone. The sinks run
/// with the registry released, so one may register another.
fn notify_tilt_range_sinks(degrees: u8) {
    let mut sinks = TILT_RANGE_SINKS.take();
    sinks.retain_mut(|sink| sink(degrees));
    TILT_RANGE_SINKS.with_borrow_mut(|current| {
        sinks.append(current);
        *current = sinks;
    });
}

/// Makes `degrees` the range in force and tells every sink.
fn apply_tilt_range(degrees: u8) {
    TILT_RANGE_SHARED.store(degrees, Ordering::Relaxed);
    notify_tilt_range_sinks(degrees);
}

/// Sets the two words of a window's `TiltWindows` for `degrees`: "to ±90°" for captions and
/// "to 90°" for sentences. `TiltWindows.scored` is fixed and never set.
pub(crate) fn set_tilt_words(words: &TiltWindows<'_>, degrees: u8) {
    words.set_displayed(display_tilt_window_caption(degrees).into());
    words.set_displayed_plain(display_tilt_window_plain(degrees).into());
}

/// The labels of the Tilt Average Range combo: "±15°" up to "±90° (all tilts)", one per
/// [`DISPLAY_TILT_WINDOW_CHOICES_DEG`] entry, in that order.
fn tilt_range_labels() -> Vec<String> {
    DISPLAY_TILT_WINDOW_CHOICES_DEG
        .iter()
        .map(|&degrees| {
            if degrees == DEFAULT_DISPLAY_TILT_WINDOW_DEG {
                format!("\u{b1}{degrees}\u{b0} (all tilts)")
            } else {
                format!("\u{b1}{degrees}\u{b0}")
            }
        })
        .collect()
}

/// The range combo entry `index` stands for: 90 for anything out of range.
fn tilt_range_for_index(index: i32) -> u8 {
    usize::try_from(index)
        .ok()
        .and_then(|entry| DISPLAY_TILT_WINDOW_CHOICES_DEG.get(entry))
        .copied()
        .unwrap_or(DEFAULT_DISPLAY_TILT_WINDOW_DEG)
}

/// The combo entry for a stored range: the entry of that range, else the one of 90.
fn tilt_range_index_for(degrees: u8) -> i32 {
    DISPLAY_TILT_WINDOW_CHOICES_DEG
        .iter()
        .position(|&choice| choice == degrees)
        .or_else(|| {
            DISPLAY_TILT_WINDOW_CHOICES_DEG
                .iter()
                .position(|&choice| choice == DEFAULT_DISPLAY_TILT_WINDOW_DEG)
        })
        .and_then(|entry| i32::try_from(entry).ok())
        .unwrap_or(0)
}

/// Makes the main window's `TiltWindows` words follow the Tilt Average Range preference.
fn bind_main_window_tilt_words(ui: &MainWindow) {
    let weak = ui.as_weak();
    register_tilt_range_sink(move |degrees| {
        let Some(ui) = weak.upgrade() else {
            return false;
        };
        set_tilt_words(&ui.global::<TiltWindows>(), degrees);
        true
    });
}

/// Makes the main window follow the high-contrast preference.
fn bind_main_window_theme(ui: &MainWindow) {
    let weak = ui.as_weak();
    register_theme_sink(move |on| {
        let Some(ui) = weak.upgrade() else {
            return false;
        };
        ui.global::<Theme>().set_high_contrast(on);
        true
    });
}

/// Makes the compare window follow the high-contrast preference. Called once, when the
/// window is created.
pub(super) fn bind_compare_window_theme(window: &CompareWindow) {
    let weak = window.as_weak();
    register_theme_sink(move |on| {
        let Some(window) = weak.upgrade() else {
            return false;
        };
        window.global::<Theme>().set_high_contrast(on);
        true
    });
}

/// Makes the Material Reference window follow the high-contrast preference. Called once, when
/// the window is created.
pub(super) fn bind_material_reference_window_theme(window: &crate::MaterialReferenceWindow) {
    let weak = window.as_weak();
    register_theme_sink(move |on| {
        let Some(window) = weak.upgrade() else {
            return false;
        };
        window.global::<Theme>().set_high_contrast(on);
        true
    });
}

/// Makes the rough planner window follow the high-contrast preference. Called once, when
/// the window is created.
pub(super) fn bind_planner_window_theme(window: &RoughPlannerWindow) {
    let weak = window.as_weak();
    register_theme_sink(move |on| {
        let Some(window) = weak.upgrade() else {
            return false;
        };
        window.global::<Theme>().set_high_contrast(on);
        true
    });
}

/// Makes the locate-inclusion window follow the high-contrast preference. Called once, when the
/// window is created.
pub(super) fn bind_locate_window_theme(window: &LocateWindow) {
    let weak = window.as_weak();
    register_theme_sink(move |on| {
        let Some(window) = weak.upgrade() else {
            return false;
        };
        window.global::<Theme>().set_high_contrast(on);
        true
    });
}

/// Makes the Rough colour wizard window follow the high-contrast preference. Called once, when
/// the window is created.
pub(super) fn bind_rough_colour_window_theme(window: &crate::RoughColourWindow) {
    let weak = window.as_weak();
    register_theme_sink(move |on| {
        let Some(window) = weak.upgrade() else {
            return false;
        };
        window.global::<Theme>().set_high_contrast(on);
        true
    });
}

/// Makes the Color Pattern window follow the high-contrast preference. Called once, when the
/// window is created.
pub(super) fn bind_color_pattern_window_theme(window: &crate::ColorPatternWindow) {
    let weak = window.as_weak();
    register_theme_sink(move |on| {
        let Some(window) = weak.upgrade() else {
            return false;
        };
        window.global::<Theme>().set_high_contrast(on);
        true
    });
}

/// Makes the camera-rig window follow the high-contrast preference. Called once, when the
/// window is created.
pub(super) fn bind_rig_window_theme(window: &RigWindow) {
    let weak = window.as_weak();
    register_theme_sink(move |on| {
        let Some(window) = weak.upgrade() else {
            return false;
        };
        window.global::<Theme>().set_high_contrast(on);
        true
    });
}

// --- Start-up and callbacks ----------------------------------------------------------

/// Seeds the preference models, the Solid viewport's remembered Snap/Slice pills and the
/// main window's palette from the loaded `settings`. Called once from
/// `startup_settings::apply_loaded_ui_mirrors`, before any callback is wired.
pub(super) fn apply_loaded(ui: &MainWindow, settings: &AppSettings) {
    let prefs = ui.global::<PreferencesModel>();
    prefs.set_simple_mode(settings.ui_mode.is_simple());
    prefs.set_ui_scale_percent(i32::from(settings.ui_scale_percent));
    prefs.set_ui_scale_options(ModelRc::new(VecModel::from(
        ui_scale_labels()
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
    prefs.set_ui_scale_index(ui_scale_index_for_percent(settings.ui_scale_percent));
    prefs.set_tilt_range_options(ModelRc::new(VecModel::from(
        tilt_range_labels()
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
    prefs.set_tilt_range_index(tilt_range_index_for(settings.tilt_average_range_deg));
    prefs.set_high_contrast(settings.high_contrast);
    prefs.set_large_handles(settings.large_handles);
    prefs.set_adaptive_viewport(settings.adaptive_viewport);
    seed_path_cache(ui, settings);
    // The welcome tour itself reads this flag; here it only says whether one is due.
    prefs.set_first_run_tour_pending(!settings.first_run_tour_done);

    let manipulate = ui.global::<ManipulateModel>();
    manipulate.set_snap_off(settings.manipulate_snap_off);
    manipulate.set_slice_symmetric(settings.slice_symmetric);
    manipulate.set_handle_scale(handle_scale(settings.large_handles));
    // The Solid viewport's "Crystal Axis" toggle, remembered per user (off on a new install).
    ui.global::<CrystalAxisModel>()
        .set_show_axis(settings.show_crystal_axis);

    HIGH_CONTRAST.set(settings.high_contrast);
    HIGH_CONTRAST_SHARED.store(settings.high_contrast, Ordering::Relaxed);
    bind_main_window_theme(ui);
    TILT_RANGE_SHARED.store(settings.tilt_average_range_deg, Ordering::Relaxed);
    bind_main_window_tilt_words(ui);
}

/// Wires every `PreferencesModel` callback: each change is saved through the debounced
/// `SettingsPersister` and applied at once (all but the UI scale, which needs a restart).
pub(super) fn setup_preferences_callbacks(
    ui: &MainWindow,
    settings_store: &Arc<SettingsPersister>,
) {
    let prefs = ui.global::<PreferencesModel>();

    // Each change below is also reported to an open tutorial: a step may wait for it.
    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    prefs.on_simple_mode_changed(move |simple: bool| {
        store.update(|s| s.settings.ui_mode = UiMode::from_simple(simple));
        raise_weak(&ui_weak, events::INTERFACE_MODE_CHANGED);
    });

    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    prefs.on_ui_scale_changed(move |index: i32| {
        let percent = ui_scale_percent_for_index(index);
        store.update(|s| s.settings.set_ui_scale_percent(i32::from(percent)));
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<PreferencesModel>()
                .set_ui_scale_percent(i32::from(percent));
            raise(&ui, events::UI_SCALE_CHANGED);
        }
    });

    // The Tilt Average Range applies at once: the words and the open tilt rows follow, nothing is
    // measured again.
    let store = Arc::clone(settings_store);
    prefs.on_tilt_range_changed(move |index: i32| {
        let degrees = tilt_range_for_index(index);
        store.update(|s| s.settings.set_tilt_average_range_deg(i32::from(degrees)));
        apply_tilt_range(degrees);
    });

    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    prefs.on_high_contrast_changed(move |on: bool| {
        store.update(|s| s.settings.high_contrast = on);
        apply_high_contrast(on);
        raise_weak(&ui_weak, events::HIGH_CONTRAST_CHANGED);
    });

    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    prefs.on_large_handles_changed(move |on: bool| {
        store.update(|s| s.settings.large_handles = on);
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<ManipulateModel>()
                .set_handle_scale(handle_scale(on));
            raise(&ui, events::LARGE_HANDLES_CHANGED);
        }
    });

    let store = Arc::clone(settings_store);
    prefs.on_reset_tutorials(move || {
        store.update(|s| s.settings.reset_tutorials());
    });

    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    prefs.on_show_tour_again(move || {
        store.update(|s| s.settings.show_tour_again());
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<PreferencesModel>()
                .set_first_run_tour_pending(true);
        }
    });
}

// --- Adaptive sampling in the viewport -------------------------------------------------

/// Saves the "Adaptive Sampling in the Viewport" switch and hands it to the render thread. A
/// real change marks the context dirty, so the picture restarts under the new rule (a switch
/// set to what it already was restarts nothing).
fn apply_adaptive_viewport(render_ctx: &Mutex<RenderContext>, store: &SettingsPersister, on: bool) {
    store.update(|s| s.settings.adaptive_viewport = on);
    let mut ctx = RenderContext::lock(render_ctx);
    if ctx.adaptive_viewport != on {
        ctx.adaptive_viewport = on;
        ctx.dirty = true;
    }
}

/// Wires `PreferencesModel.adaptive_viewport_changed`: it needs the render context, which
/// [`setup_preferences_callbacks`] does not take. Called next to it, once.
pub(super) fn setup_adaptive_viewport_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx = Arc::clone(render_ctx);
    let store = Arc::clone(settings_store);
    ui.global::<PreferencesModel>()
        .on_adaptive_viewport_changed(move |on: bool| {
            apply_adaptive_viewport(&render_ctx, &store, on);
        });
}

// --- The path cache ------------------------------------------------------------------

/// Bytes in a decimal gigabyte, the unit of the disk limit (the cache's own `format_bytes` and
/// `DEFAULT_DISK_LIMIT_BYTES` use decimal gigabytes too).
const BYTES_PER_GB: u64 = 1_000_000_000;

/// The disk limit in bytes for a limit of `gb` gigabytes, limited to what the field offers.
#[must_use]
fn disk_limit_bytes(gb: u32) -> u64 {
    u64::from(clamp_path_cache_disk_limit_gb(gb)) * BYTES_PER_GB
}

/// Deletes every kept path cache under `root` (a folder named by its key) and returns how many
/// there were. Anything else in the folder stays, and a missing folder has nothing to clear.
fn clear_path_caches(root: &Path) -> std::io::Result<usize> {
    let mut cleared = 0;
    for entry in lru::scan(root)? {
        std::fs::remove_dir_all(root.join(&entry.key))?;
        cleared += 1;
    }
    Ok(cleared)
}

/// What "Clear Path Cache" tells the user after clearing `root`.
fn clear_path_cache_status(root: &Path) -> String {
    match clear_path_caches(root) {
        Ok(0) => "There was no path cache to clear.".to_owned(),
        Ok(1) => "Cleared 1 path cache.".to_owned(),
        Ok(count) => format!("Cleared {count} path caches."),
        Err(error) => format!("The path cache could not be cleared: {error}"),
    }
}

/// Saves the disk limit (limited to what the field offers) and returns the value saved.
fn save_path_cache_limit(store: &SettingsPersister, gb: u32) -> u32 {
    let gb = clamp_path_cache_disk_limit_gb(gb);
    store.update(|s| s.settings.path_cache_disk_limit_gb = gb);
    gb
}

/// Deletes the least recently used caches under `root` until the folder fits `gb` gigabytes;
/// returns how many were deleted. A failure is logged, never shown: the limit applies again on
/// the next change.
fn enforce_path_cache_limit(gb: u32, root: &Path) -> usize {
    lru::enforce_limit(root, disk_limit_bytes(gb), None).map_or_else(
        |error| {
            tracing::warn!("The path cache limit could not be applied: {error}");
            0
        },
        |evicted| evicted.len(),
    )
}

/// Saves the "Speed Up Still Views With a Path Cache" switch and hands it to the render thread.
/// A real change marks the context dirty, so the picture restarts under the new rule.
fn apply_path_cache_viewport(
    render_ctx: &Mutex<RenderContext>,
    store: &SettingsPersister,
    on: bool,
) {
    store.update(|s| s.settings.path_cache_viewport = on);
    let mut ctx = RenderContext::lock(render_ctx);
    if ctx.path_cache_viewport != on {
        ctx.path_cache_viewport = on;
        ctx.dirty = true;
    }
}

/// Saves the "Instant Lighting Changes in the Ray-Traced View" switch and hands it to the render
/// thread. A real change marks the context dirty, so the picture restarts under the new rule.
fn apply_instant_lighting(render_ctx: &Mutex<RenderContext>, store: &SettingsPersister, on: bool) {
    store.update(|s| s.settings.instant_lighting = on);
    let mut ctx = RenderContext::lock(render_ctx);
    if ctx.instant_lighting != on {
        ctx.instant_lighting = on;
        ctx.dirty = true;
    }
}

/// What the render thread is told for "Instant Lighting Changes With Beams": the saved choice, but
/// only while "Beam Tracing in Still Views" is on, because the beams it re-lights exist only then.
#[must_use]
#[cfg(feature = "beam")]
pub const fn effective_beam_instant_lighting(beam_viewport: bool, beam_instant: bool) -> bool {
    beam_viewport && beam_instant
}

/// Saves the "Beam Tracing in Still Views" switch and hands it to the render thread, together with
/// the beam lighting switch it gates. A real change marks the context dirty, so the picture
/// restarts under the new rule.
#[cfg(feature = "beam")]
fn apply_beam_viewport(render_ctx: &Mutex<RenderContext>, store: &SettingsPersister, on: bool) {
    store.update(|s| s.settings.beam_viewport = on);
    let instant = store.snapshot().settings.beam_instant_lighting;
    set_beam_context(render_ctx, on, instant);
}

/// Saves the "Instant Lighting Changes With Beams" switch and hands it to the render thread; it
/// takes effect only while "Beam Tracing in Still Views" is on.
#[cfg(feature = "beam")]
fn apply_beam_instant_lighting(
    render_ctx: &Mutex<RenderContext>,
    store: &SettingsPersister,
    on: bool,
) {
    store.update(|s| s.settings.beam_instant_lighting = on);
    let viewport = store.snapshot().settings.beam_viewport;
    set_beam_context(render_ctx, viewport, on);
}

/// Writes the two beam switches into the render context (the second gated by the first) and marks
/// it dirty when either really changed.
#[cfg(feature = "beam")]
fn set_beam_context(render_ctx: &Mutex<RenderContext>, viewport: bool, instant: bool) {
    let instant = effective_beam_instant_lighting(viewport, instant);
    let mut ctx = RenderContext::lock(render_ctx);
    if ctx.beam_viewport != viewport || ctx.beam_instant_lighting != instant {
        ctx.beam_viewport = viewport;
        ctx.beam_instant_lighting = instant;
        ctx.dirty = true;
    }
}

/// Shows the cache folder, the saved disk limit and the instant lighting switch in the Preferences
/// model. Called at start-up.
fn seed_path_cache(ui: &MainWindow, settings: &AppSettings) {
    let prefs = ui.global::<PreferencesModel>();
    prefs.set_path_cache_viewport(settings.path_cache_viewport);
    // The app renders with exit collection, which uses neither the path cache nor beam tracing,
    // so the path cache rows are hidden and the beam switch states why it applies only to the
    // Ray-Traced view while it is the process default. The saved values are kept.
    let branching = pleochroma::optics::raytracer::default_estimator()
        == pleochroma::optics::raytracer::Estimator::Branching;
    prefs.set_path_cache_available(branching);
    prefs.set_beam_available(branching);
    prefs.set_instant_lighting(settings.instant_lighting);
    #[cfg(feature = "beam")]
    prefs.set_beam_viewport(settings.beam_viewport);
    #[cfg(feature = "beam")]
    prefs.set_beam_instant_lighting(settings.beam_instant_lighting);
    prefs.set_path_cache_dir(
        default_render_cache_dir()
            .to_string_lossy()
            .into_owned()
            .into(),
    );
    prefs.set_path_cache_disk_limit_gb(
        i32::try_from(clamp_path_cache_disk_limit_gb(
            settings.path_cache_disk_limit_gb,
        ))
        .unwrap_or(i32::MAX),
    );
}

/// Wires the path cache entries of the Preferences dialog: the viewport switch (it needs the
/// render context), the disk limit, "Open Folder" and "Clear Path Cache". Called next to
/// [`setup_preferences_callbacks`], once.
pub(super) fn setup_path_cache_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let prefs = ui.global::<PreferencesModel>();

    let lighting_ctx = Arc::clone(render_ctx);
    let lighting_store = Arc::clone(settings_store);
    prefs.on_instant_lighting_changed(move |on: bool| {
        apply_instant_lighting(&lighting_ctx, &lighting_store, on);
    });

    // Beam tracing is off in shipped builds: its switches are wired only with the `beam` feature.
    #[cfg(feature = "beam")]
    {
        let beam_ctx = Arc::clone(render_ctx);
        let beam_store = Arc::clone(settings_store);
        prefs.on_beam_viewport_changed(move |on: bool| {
            apply_beam_viewport(&beam_ctx, &beam_store, on);
        });

        let beam_light_ctx = Arc::clone(render_ctx);
        let beam_light_store = Arc::clone(settings_store);
        prefs.on_beam_instant_lighting_changed(move |on: bool| {
            apply_beam_instant_lighting(&beam_light_ctx, &beam_light_store, on);
        });
    }

    let render_ctx = Arc::clone(render_ctx);
    let store = Arc::clone(settings_store);
    prefs.on_path_cache_viewport_changed(move |on: bool| {
        apply_path_cache_viewport(&render_ctx, &store, on);
    });

    let store = Arc::clone(settings_store);
    prefs.on_path_cache_limit_changed(move |gb: i32| {
        let gb = save_path_cache_limit(&store, u32::try_from(gb).unwrap_or(0));
        // Deleting old caches can take a moment: not on the UI thread.
        std::thread::spawn(move || {
            enforce_path_cache_limit(gb, &default_render_cache_dir());
        });
    });

    let ui_weak = ui.as_weak();
    prefs.on_open_path_cache_folder(move || {
        let root = default_render_cache_dir();
        let opened = std::fs::create_dir_all(&root)
            .map_err(|error| format!("Could not create {}: {error}", root.display()))
            .and_then(|()| open_local_path(&root));
        if let (Err(message), Some(ui)) = (opened, ui_weak.upgrade()) {
            ui.global::<PreferencesModel>()
                .set_path_cache_status(message.into());
        }
    });

    let ui_weak = ui.as_weak();
    prefs.on_clear_path_cache(move || {
        let status = clear_path_cache_status(&default_render_cache_dir());
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<PreferencesModel>()
                .set_path_cache_status(status.into());
        }
    });
}

// --- The Solid viewport's remembered pills -------------------------------------------

/// Saves the Snap pill (`ManipulateModel.snap_off`) through the ambient persister. Called
/// from the pill's `snap_toggled` handler, which has no settings handle of its own.
pub(super) fn persist_snap_off(snap_off: bool) {
    if let Some(store) = SettingsPersister::installed_for_this_thread() {
        store.update(|s| s.settings.manipulate_snap_off = snap_off);
    }
}

/// Saves the Slice tool's Symmetric pill (`ManipulateModel.slice_symmetric`). The Slice
/// mode itself is deliberately never saved: it changes what a left-drag does.
pub(super) fn persist_slice_symmetric(symmetric: bool) {
    if let Some(store) = SettingsPersister::installed_for_this_thread() {
        store.update(|s| s.settings.slice_symmetric = symmetric);
    }
}

/// Saves the toolbar's "Crystal Axis" toggle (`CrystalAxisModel.show_axis`) through the
/// ambient persister. Called from the toggle's `show_axis_toggled` handler only: the
/// session-only switch-on (`gui::render::crystal_axis_ui::show_axis_for_session`) is not saved.
pub(in crate::gui) fn persist_show_crystal_axis(show: bool) {
    if let Some(store) = SettingsPersister::installed_for_this_thread() {
        store.update(|s| s.settings.show_crystal_axis = show);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::model::SettingsFile;
    use std::rc::Rc;

    #[test]
    fn the_scale_combo_starts_with_automatic_and_lists_every_choice_in_order() {
        let labels = ui_scale_labels();
        assert_eq!(labels.len(), UI_SCALE_CHOICES.len() + 1);
        assert_eq!(labels[0], "Automatic");
        assert_eq!(labels[1], "75 %");
        assert_eq!(labels[3], "100 %");
        assert_eq!(labels.last().map(String::as_str), Some("200 %"));
    }

    #[test]
    fn every_combo_entry_maps_to_a_percentage_and_back() {
        assert_eq!(ui_scale_percent_for_index(0), 0);
        for (position, &percent) in UI_SCALE_CHOICES.iter().enumerate() {
            let index = i32::try_from(position + 1).expect("a small index");
            assert_eq!(ui_scale_percent_for_index(index), percent);
            assert_eq!(ui_scale_index_for_percent(percent), index);
        }
        assert_eq!(ui_scale_index_for_percent(0), 0);
    }

    #[test]
    fn an_out_of_range_entry_or_percentage_means_automatic() {
        assert_eq!(ui_scale_percent_for_index(-1), 0);
        assert_eq!(ui_scale_percent_for_index(99), 0);
        assert_eq!(ui_scale_index_for_percent(133), 0);
        assert_eq!(ui_scale_index_for_percent(u16::MAX), 0);
    }

    #[test]
    fn the_environment_override_follows_the_saved_scale() {
        assert_eq!(scale_factor_override(125, false), Some("1.25"));
        assert_eq!(scale_factor_override(75, false), Some("0.75"));
        assert_eq!(scale_factor_override(200, false), Some("2"));
        assert_eq!(scale_factor_override(0, false), None, "Automatic");
        assert_eq!(scale_factor_override(133, false), None, "not offered");
    }

    #[test]
    fn a_scale_the_user_set_in_the_environment_wins_over_the_saved_one() {
        assert_eq!(scale_factor_override(125, true), None);
        assert_eq!(scale_factor_override(0, true), None);
    }

    #[test]
    fn the_range_combo_lists_the_six_choices_in_order_and_ninety_is_last() {
        let labels = tilt_range_labels();
        assert_eq!(
            labels,
            [
                "\u{b1}15\u{b0}",
                "\u{b1}20\u{b0}",
                "\u{b1}30\u{b0}",
                "\u{b1}45\u{b0}",
                "\u{b1}60\u{b0}",
                "\u{b1}90\u{b0} (all tilts)",
            ]
        );
    }

    #[test]
    fn every_range_entry_maps_to_a_range_and_back_and_odd_ones_mean_ninety() {
        for (position, &degrees) in DISPLAY_TILT_WINDOW_CHOICES_DEG.iter().enumerate() {
            let index = i32::try_from(position).expect("a small index");
            assert_eq!(tilt_range_for_index(index), degrees);
            assert_eq!(tilt_range_index_for(degrees), index);
        }
        assert_eq!(tilt_range_index_for(90), 5, "the default is the last entry");
        assert_eq!(tilt_range_for_index(-1), 90);
        assert_eq!(tilt_range_for_index(6), 90);
        assert_eq!(tilt_range_index_for(25), 5);
    }

    #[test]
    fn a_range_sink_is_told_every_change_and_forgotten_once_its_window_is_gone() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let alive = Rc::new(Cell::new(true));
        let (sink_seen, sink_alive) = (Rc::clone(&seen), Rc::clone(&alive));
        register_tilt_range_sink(move |degrees| {
            sink_seen.borrow_mut().push(degrees);
            sink_alive.get()
        });
        assert_eq!(
            seen.borrow().len(),
            1,
            "told the range in force on registering"
        );
        notify_tilt_range_sinks(30);
        notify_tilt_range_sinks(60);
        assert_eq!(seen.borrow()[1..], [30, 60]);
        alive.set(false);
        notify_tilt_range_sinks(45);
        notify_tilt_range_sinks(90);
        assert_eq!(
            seen.borrow().len(),
            4,
            "the closed window's sink answered once, then left"
        );
    }

    #[test]
    fn larger_handles_are_one_point_six_times_bigger() {
        assert!((handle_scale(false) - 1.0).abs() < 1e-6);
        assert!((handle_scale(true) - 1.6).abs() < 1e-6);
    }

    /// A sink that records every state it is given and stays registered while `alive`.
    fn recording_sink(
        seen: &Rc<RefCell<Vec<bool>>>,
        alive: &Rc<Cell<bool>>,
    ) -> impl Fn(bool) -> bool + 'static {
        let seen = Rc::clone(seen);
        let alive = Rc::clone(alive);
        move |on| {
            seen.borrow_mut().push(on);
            alive.get()
        }
    }

    #[test]
    fn a_new_window_starts_in_the_current_palette_and_follows_later_changes() {
        apply_high_contrast(true);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let alive = Rc::new(Cell::new(true));
        register_theme_sink(recording_sink(&seen, &alive));
        assert_eq!(*seen.borrow(), vec![true], "starts in the current state");

        apply_high_contrast(false);
        apply_high_contrast(true);
        assert_eq!(*seen.borrow(), vec![true, false, true]);
        assert!(high_contrast_enabled());
        apply_high_contrast(false);
    }

    #[test]
    fn every_registered_window_switches_together() {
        apply_high_contrast(false);
        let first = Rc::new(RefCell::new(Vec::new()));
        let second = Rc::new(RefCell::new(Vec::new()));
        let alive = Rc::new(Cell::new(true));
        register_theme_sink(recording_sink(&first, &alive));
        register_theme_sink(recording_sink(&second, &alive));
        apply_high_contrast(true);
        assert_eq!(*first.borrow(), vec![false, true]);
        assert_eq!(*second.borrow(), vec![false, true]);
        apply_high_contrast(false);
    }

    #[test]
    fn a_closed_window_is_forgotten_and_never_called_again() {
        apply_high_contrast(false);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let alive = Rc::new(Cell::new(true));
        register_theme_sink(recording_sink(&seen, &alive));
        alive.set(false);
        apply_high_contrast(true); // reported gone: dropped here
        apply_high_contrast(false);
        assert_eq!(*seen.borrow(), vec![false, true]);

        // A sink that is already gone when it registers is never kept at all.
        let gone = Rc::new(RefCell::new(Vec::new()));
        register_theme_sink(recording_sink(&gone, &alive));
        apply_high_contrast(true);
        assert_eq!(*gone.borrow(), vec![false]);
        apply_high_contrast(false);
    }

    /// A persister on a scratch path, installed for this test's thread.
    fn install_scratch_persister(tag: &str) -> (Arc<SettingsPersister>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-cut-preferences-test-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch directory");
        let path = dir.join("settings.toml");
        let persister = Arc::new(SettingsPersister::spawn(
            path.clone(),
            SettingsFile::default(),
        ));
        SettingsPersister::install_for_this_thread(&persister);
        (persister, path)
    }

    #[test]
    fn the_snap_pill_is_saved_through_the_ambient_persister() {
        let (persister, path) = install_scratch_persister("snap");
        assert!(!persister.snapshot().settings.manipulate_snap_off);
        persist_snap_off(true);
        assert!(persister.snapshot().settings.manipulate_snap_off);
        persist_snap_off(false);
        assert!(!persister.snapshot().settings.manipulate_snap_off);

        persist_snap_off(true);
        persister.flush();
        let text = std::fs::read_to_string(&path).expect("flush wrote the file");
        assert!(text.contains("manipulate_snap_off = true"), "{text}");
        let _ = std::fs::remove_dir_all(path.parent().expect("a parent directory"));
    }

    #[test]
    fn the_symmetric_pill_is_saved_and_the_slice_mode_never_is() {
        let (persister, path) = install_scratch_persister("symmetric");
        assert!(persister.snapshot().settings.slice_symmetric);
        persist_slice_symmetric(false);
        assert!(!persister.snapshot().settings.slice_symmetric);

        persister.flush();
        let text = std::fs::read_to_string(&path).expect("flush wrote the file");
        assert!(text.contains("slice_symmetric = false"), "{text}");
        assert!(
            !text.contains("slice_mode"),
            "the Slice tool must not be remembered: {text}"
        );
        let _ = std::fs::remove_dir_all(path.parent().expect("a parent directory"));
    }

    #[test]
    fn the_crystal_axis_toggle_is_off_by_default_and_remembered_once_chosen() {
        let (persister, path) = install_scratch_persister("crystal-axis");
        assert!(
            !persister.snapshot().settings.show_crystal_axis,
            "off by default"
        );
        persist_show_crystal_axis(true);
        assert!(persister.snapshot().settings.show_crystal_axis);

        persister.flush();
        let text = std::fs::read_to_string(&path).expect("flush wrote the file");
        assert!(text.contains("show_crystal_axis = true"), "{text}");
        let _ = std::fs::remove_dir_all(path.parent().expect("a parent directory"));
    }

    #[test]
    fn a_settings_file_without_the_crystal_axis_key_loads_it_off() {
        let file: SettingsFile = toml::from_str("[settings]\nlarge_handles = true\n")
            .expect("an older settings file parses");
        assert!(!file.settings.show_crystal_axis);
        assert!(file.settings.large_handles);
    }

    #[test]
    fn the_adaptive_viewport_switch_is_off_by_default_saved_and_applied() {
        let (persister, path) = install_scratch_persister("adaptive-viewport");
        let ctx = Mutex::new(RenderContext::default());
        assert!(!persister.snapshot().settings.adaptive_viewport);
        assert!(!RenderContext::lock(&ctx).adaptive_viewport);

        RenderContext::lock(&ctx).dirty = false;
        apply_adaptive_viewport(&ctx, &persister, true);
        assert!(persister.snapshot().settings.adaptive_viewport);
        let guard = RenderContext::lock(&ctx);
        let (applied, dirty) = (guard.adaptive_viewport, guard.dirty);
        drop(guard);
        assert!(applied);
        assert!(dirty, "a real change restarts the picture");

        // Setting it to what it already is restarts nothing.
        RenderContext::lock(&ctx).dirty = false;
        apply_adaptive_viewport(&ctx, &persister, true);
        assert!(!RenderContext::lock(&ctx).dirty);

        persister.flush();
        let text = std::fs::read_to_string(&path).expect("flush wrote the file");
        assert!(text.contains("adaptive_viewport = true"), "{text}");

        apply_adaptive_viewport(&ctx, &persister, false);
        assert!(!persister.snapshot().settings.adaptive_viewport);
        assert!(!RenderContext::lock(&ctx).adaptive_viewport);
        let _ = std::fs::remove_dir_all(path.parent().expect("a parent directory"));
    }

    #[test]
    fn a_settings_file_without_the_adaptive_viewport_key_loads_it_off() {
        let file: SettingsFile = toml::from_str("[settings]\nlarge_handles = true\n")
            .expect("an older settings file parses");
        assert!(!file.settings.adaptive_viewport);
        let on: SettingsFile = toml::from_str("[settings]\nadaptive_viewport = true\n")
            .expect("a file with the key parses");
        assert!(on.settings.adaptive_viewport);
    }

    #[test]
    fn the_path_cache_viewport_switch_is_off_by_default_saved_and_applied() {
        let (persister, path) = install_scratch_persister("path-cache-viewport");
        let ctx = Mutex::new(RenderContext::default());
        assert!(!persister.snapshot().settings.path_cache_viewport);
        assert!(!RenderContext::lock(&ctx).path_cache_viewport);

        RenderContext::lock(&ctx).dirty = false;
        apply_path_cache_viewport(&ctx, &persister, true);
        assert!(persister.snapshot().settings.path_cache_viewport);
        let guard = RenderContext::lock(&ctx);
        let (applied, dirty) = (guard.path_cache_viewport, guard.dirty);
        drop(guard);
        assert!(applied);
        assert!(dirty, "a real change restarts the picture");

        // Setting it to what it already is restarts nothing.
        RenderContext::lock(&ctx).dirty = false;
        apply_path_cache_viewport(&ctx, &persister, true);
        assert!(!RenderContext::lock(&ctx).dirty);

        persister.flush();
        let text = std::fs::read_to_string(&path).expect("flush wrote the file");
        assert!(text.contains("path_cache_viewport = true"), "{text}");
        let _ = std::fs::remove_dir_all(path.parent().expect("a parent directory"));
    }

    #[test]
    fn the_instant_lighting_switch_is_off_by_default_saved_and_applied() {
        let (persister, path) = install_scratch_persister("instant-lighting");
        let ctx = Mutex::new(RenderContext::default());
        assert!(!persister.snapshot().settings.instant_lighting);
        assert!(!RenderContext::lock(&ctx).instant_lighting);

        RenderContext::lock(&ctx).dirty = false;
        apply_instant_lighting(&ctx, &persister, true);
        assert!(persister.snapshot().settings.instant_lighting);
        let guard = RenderContext::lock(&ctx);
        let (applied, dirty) = (guard.instant_lighting, guard.dirty);
        drop(guard);
        assert!(applied);
        assert!(dirty, "a real change restarts the picture");

        // Setting it to what it already is restarts nothing.
        RenderContext::lock(&ctx).dirty = false;
        apply_instant_lighting(&ctx, &persister, true);
        assert!(!RenderContext::lock(&ctx).dirty);

        persister.flush();
        let text = std::fs::read_to_string(&path).expect("flush wrote the file");
        assert!(text.contains("instant_lighting = true"), "{text}");
        let _ = std::fs::remove_dir_all(path.parent().expect("a parent directory"));
    }

    #[cfg(all(feature = "beam", beam_tests))]
    #[test]
    fn the_beam_switches_are_off_by_default_saved_and_applied() {
        let (persister, path) = install_scratch_persister("beam-switches");
        let ctx = Mutex::new(RenderContext::default());
        let saved = persister.snapshot().settings;
        assert!(!saved.beam_viewport && !saved.beam_instant_lighting);
        assert!(!RenderContext::lock(&ctx).beam_viewport);
        assert!(!RenderContext::lock(&ctx).beam_instant_lighting);

        RenderContext::lock(&ctx).dirty = false;
        apply_beam_viewport(&ctx, &persister, true);
        assert!(persister.snapshot().settings.beam_viewport);
        let guard = RenderContext::lock(&ctx);
        let (viewport, instant, dirty) = (
            guard.beam_viewport,
            guard.beam_instant_lighting,
            guard.dirty,
        );
        drop(guard);
        assert!(viewport);
        assert!(!instant, "the second switch is still off");
        assert!(dirty, "a real change restarts the picture");

        // The second switch works once the first is on, and is saved.
        apply_beam_instant_lighting(&ctx, &persister, true);
        assert!(persister.snapshot().settings.beam_instant_lighting);
        assert!(RenderContext::lock(&ctx).beam_instant_lighting);

        // Setting it to what it already is restarts nothing.
        RenderContext::lock(&ctx).dirty = false;
        apply_beam_instant_lighting(&ctx, &persister, true);
        assert!(!RenderContext::lock(&ctx).dirty);

        // Turning the first off disables the second in the context, and keeps the saved choice.
        apply_beam_viewport(&ctx, &persister, false);
        assert!(!RenderContext::lock(&ctx).beam_instant_lighting);
        assert!(persister.snapshot().settings.beam_instant_lighting);

        persister.flush();
        let text = std::fs::read_to_string(&path).expect("flush wrote the file");
        assert!(text.contains("beam_instant_lighting = true"), "{text}");
        assert!(text.contains("beam_viewport = false"), "{text}");
        let _ = std::fs::remove_dir_all(path.parent().expect("a parent directory"));
    }

    #[cfg(all(feature = "beam", beam_tests))]
    #[test]
    fn the_second_beam_switch_needs_the_first() {
        assert!(!effective_beam_instant_lighting(false, true));
        assert!(!effective_beam_instant_lighting(true, false));
        assert!(effective_beam_instant_lighting(true, true));
    }

    #[cfg(all(feature = "beam", beam_tests))]
    #[test]
    fn a_settings_file_without_the_beam_keys_loads_them_off() {
        let old: SettingsFile = toml::from_str("[settings]\nlarge_handles = true\n")
            .expect("an older settings file parses");
        assert!(!old.settings.beam_viewport && !old.settings.beam_instant_lighting);
        let on: SettingsFile =
            toml::from_str("[settings]\nbeam_viewport = true\nbeam_instant_lighting = true\n")
                .expect("a file with the keys parses");
        assert!(on.settings.beam_viewport && on.settings.beam_instant_lighting);
    }

    #[test]
    fn a_settings_file_without_the_instant_lighting_key_loads_it_off() {
        let old: SettingsFile = toml::from_str("[settings]\nlarge_handles = true\n")
            .expect("an older settings file parses");
        assert!(!old.settings.instant_lighting);
        assert!(!AppSettings::default().instant_lighting);
        let on: SettingsFile = toml::from_str("[settings]\ninstant_lighting = true\n")
            .expect("a file with the key parses");
        assert!(on.settings.instant_lighting);
    }

    #[test]
    fn the_path_cache_disk_limit_defaults_to_fifty_and_is_saved_within_its_range() {
        // An older settings file has no key: 50 GB. A file with the key keeps it.
        let old: SettingsFile = toml::from_str("[settings]\nlarge_handles = true\n")
            .expect("an older settings file parses");
        assert_eq!(old.settings.path_cache_disk_limit_gb, 50);
        assert_eq!(AppSettings::default().path_cache_disk_limit_gb, 50);
        assert!(!old.settings.path_cache_viewport);
        let kept: SettingsFile = toml::from_str(
            "[settings]\npath_cache_disk_limit_gb = 120\npath_cache_viewport = true\n",
        )
        .expect("a file with the keys parses");
        assert_eq!(kept.settings.path_cache_disk_limit_gb, 120);
        assert!(kept.settings.path_cache_viewport);

        let (persister, path) = install_scratch_persister("path-cache-limit");
        assert_eq!(save_path_cache_limit(&persister, 200), 200);
        assert_eq!(persister.snapshot().settings.path_cache_disk_limit_gb, 200);
        assert_eq!(save_path_cache_limit(&persister, 0), 1, "never below 1 GB");
        assert_eq!(
            save_path_cache_limit(&persister, u32::MAX),
            100_000,
            "never above the field's range"
        );
        persister.flush();
        let text = std::fs::read_to_string(&path).expect("flush wrote the file");
        assert!(text.contains("path_cache_disk_limit_gb = 100000"), "{text}");
        let _ = std::fs::remove_dir_all(path.parent().expect("a parent directory"));
        assert_eq!(disk_limit_bytes(50), 50_000_000_000);
    }

    /// A scratch folder under the system temp folder, removed when dropped.
    struct ScratchRoot(std::path::PathBuf);

    impl ScratchRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "pleochroma-cut-path-cache-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch directory");
            Self(dir)
        }

        /// A folder named like a cache key (32 lowercase hex digits) holding `bytes` bytes.
        fn add_cache(&self, digit: char, bytes: usize) -> String {
            let name = digit.to_string().repeat(32);
            let dir = self.0.join(&name);
            std::fs::create_dir_all(&dir).expect("cache folder");
            std::fs::write(dir.join("chunk.bin"), vec![0_u8; bytes]).expect("cache file");
            name
        }
    }

    impl Drop for ScratchRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn clearing_deletes_only_the_key_named_folders() {
        let root = ScratchRoot::new("clear");
        let first = root.add_cache('a', 10);
        let second = root.add_cache('b', 10);
        std::fs::create_dir_all(root.0.join("notes")).expect("foreign folder");
        std::fs::write(root.0.join("loose.txt"), "keep me").expect("foreign file");

        assert_eq!(clear_path_cache_status(&root.0), "Cleared 2 path caches.");
        assert!(!root.0.join(first).exists());
        assert!(!root.0.join(second).exists());
        assert!(
            root.0.join("notes").is_dir(),
            "a folder that is no cache stays"
        );
        assert!(
            root.0.join("loose.txt").is_file(),
            "a file that is no cache stays"
        );
        assert!(root.0.is_dir(), "the cache folder itself stays");

        assert_eq!(
            clear_path_cache_status(&root.0),
            "There was no path cache to clear."
        );
        let one = root.add_cache('c', 1);
        assert_eq!(clear_path_cache_status(&root.0), "Cleared 1 path cache.");
        assert!(!root.0.join(one).exists());
        // A folder that was never created has nothing to clear.
        assert_eq!(
            clear_path_cache_status(&root.0.join("not-there")),
            "There was no path cache to clear."
        );
    }

    #[test]
    fn the_disk_limit_leaves_caches_under_it_alone_and_a_missing_folder_is_fine() {
        let root = ScratchRoot::new("limit");
        let kept = root.add_cache('d', 10);
        // 1 GB is the lowest limit; a cache of ten bytes stays.
        assert_eq!(enforce_path_cache_limit(1, &root.0), 0);
        assert!(root.0.join(kept).is_dir());
        // A folder that does not exist has nothing to delete either.
        assert_eq!(enforce_path_cache_limit(1, &root.0.join("not-there")), 0);
    }

    #[test]
    fn saving_a_pill_without_a_persister_does_nothing() {
        // No persister is installed on this test's own thread.
        persist_snap_off(true);
        persist_slice_symmetric(false);
        persist_show_crystal_axis(true);
    }
}
