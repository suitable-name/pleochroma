//! The Color Pattern window: creating it, registering its callbacks and opening a session.
//!
//! The window is created on the first open and kept (like the Rough Color wizard's), so the second
//! open is instant. Its two background workers deliver to the UI thread through
//! `upgrade_in_event_loop`, then reach the thread-local host through `on_host`.

use super::{
    actions,
    colors::{SolveCache, SolveWorker, cache_key},
    own::own_color,
    picker,
    preview::{self, OutlineKey, PreviewWorker, Rendered},
    save,
    session::{
        Deps, Host, Session, VIEW_SLICE, VIEW_STONE, clear_deps, on_host, set_host, take_host,
        with_host,
    },
    stone::StonePatternApply,
    store,
    variants::zone_variants,
};
use crate::{
    ColorPatternModel, ColorPatternWindow, MainWindow, Zoning,
    gui::{show_toast, tutorial_events::raise},
};
use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::material::pattern::{ColorPattern, OwnColor, allowed_rank};
use pleochroma_editor::{guide::viewing_events as events, lch_color::reference_path_mm};
use slint::{CloseRequestResponse, ComponentHandle, Timer, TimerMode};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, PoisonError, atomic::Ordering},
    time::Duration,
};
use tracing::warn;

/// How often the open window is checked for a new size.
const TICK: Duration = Duration::from_millis(250);

/// What a session opens with.
pub(super) struct Opening {
    /// The saved material name, or the stone session's target label.
    pub material_name: String,
    pub stone_apply: Option<Rc<StonePatternApply>>,
    pub targets_design: bool,
    /// The pattern to edit.
    pub pattern: ColorPattern,
    /// A sentence about where the pattern came from, if it was not stored as a pattern.
    pub import_note: Option<String>,
    /// The stone width the pattern is resolved at, millimeters.
    pub stone_width_mm: f64,
    /// The material the Current Design preview renders the pattern in.
    pub base_material: GemMaterial,
    /// Whether the material is birefringent, so a zone may have two colors.
    pub two_colors_allowed: bool,
    /// The color the stone has without a pattern, per direction; `None`: worked out from the base
    /// material, with the pattern's base zone as the color of a stone without pleochroism.
    pub own: Option<OwnColor>,
}

/// The solved colors a pattern already holds, so an undo back to them needs no new solve.
fn seed_cache(pattern: &ColorPattern) -> SolveCache {
    let mut cache = SolveCache::new();
    for zone in std::iter::once(&pattern.base).chain(&pattern.zones) {
        for color in zone.color.zone_colors() {
            if color.is_solved() {
                cache.insert(cache_key(color.lch), color.bands.clone());
            }
        }
    }
    cache
}

/// Registers every callback of the window's `ColorPatternModel`.
fn wire(window: &ColorPatternWindow) {
    let model = window.global::<ColorPatternModel>();
    model.on_open_gallery(|| on_host(|host| actions::open_gallery(host)));
    model.on_close_gallery(|| on_host(|host| actions::close_gallery(host)));
    model.on_template_picked(|index| on_host(|host| actions::template_picked(host, index)));
    model.on_typical_picked(|index| on_host(|host| actions::typical_picked(host, index)));
    model.on_preset_picked(|index| on_host(|host| actions::preset_picked(host, index)));
    model.on_preset_delete(|index| on_host(|host| actions::preset_delete(host, index)));
    model.on_save_preset(|| on_host(|host| actions::save_preset(host)));
    model.on_preset_name_committed(|name| {
        on_host(|host| actions::preset_name_committed(host, name.as_str()));
    });
    model.on_preset_name_cancelled(|| on_host(|host| actions::preset_name_cancelled(host)));

    model.on_view_changed(|view| on_host(|host| actions::view_changed(host, view)));
    model.on_swap_views(|| on_host(|host| actions::swap_views(host)));
    model.on_slice_changed(|position| on_host(|host| actions::slice_changed(host, position)));
    model.on_viewport_pointer(|fx, fy, kind| {
        on_host(|host| actions::pointer(host, fx, fy, kind));
    });

    model.on_select_row(|row| on_host(|host| actions::select_row(host, row)));
    model.on_add_zone(|index| on_host(|host| actions::add_zone(host, index)));
    model.on_duplicate_row(|row| on_host(|host| actions::duplicate_row(host, row)));
    model.on_delete_row(|row| on_host(|host| actions::delete_row(host, row)));
    model.on_move_row(|row, delta| on_host(|host| actions::move_row(host, row, delta)));
    model.on_reorder_row(|from, to| on_host(|host| actions::reorder_row(host, from, to)));
    model.on_toggle_hidden(|row| on_host(|host| actions::toggle_hidden(host, row)));
    model.on_rename_row(|row, name| {
        on_host(|host| actions::rename_row(host, row, name.as_str()));
    });
    model.on_shape_changed(|index| on_host(|host| actions::shape_changed(host, index)));
    model.on_param_changed(|row, value| on_host(|host| actions::param_changed(host, row, value)));
    model.on_size_changed(|value| on_host(|host| actions::size_changed(host, value)));

    model.on_variant_picked(|index| on_host(|host| actions::variant_picked(host, index)));
    model.on_convert_legacy(|| on_host(|host| actions::convert_legacy(host)));

    model.on_axis_changed(|index| on_host(|host| actions::axis_changed(host, index)));
    model.on_tilt_turn_changed(|tilt, turn| {
        on_host(|host| actions::tilt_turn_changed(host, tilt, turn));
    });
    model.on_scale_mode_changed(|relative| {
        on_host(|host| actions::scale_mode_changed(host, relative));
    });
    model.on_softness_changed(|value| on_host(|host| actions::softness_changed(host, value)));

    model.on_undo(|| on_host(|host| actions::undo(host)));
    model.on_redo(|| on_host(|host| actions::redo(host)));
    model.on_cancel(|| on_host(|host| actions::cancel(host)));
    model.on_discard_confirmed(|| on_host(|host| actions::discard_confirmed(host)));
    model.on_discard_declined(|| on_host(|host| actions::discard_declined(host)));
    model.on_save(|| on_host(|host| save::store_pattern(host, true)));
    picker::wire(window);

    // The title bar's close button is Cancel: it asks first when there are edits.
    window.window().on_close_requested(|| {
        on_host(|host| actions::cancel(host));
        CloseRequestResponse::KeepWindowShown
    });
}

/// Builds the window, its workers and the host; `None` when the window cannot be created.
fn create(main: &MainWindow, deps: &Deps) -> Option<Rc<Host>> {
    let window = match ColorPatternWindow::new() {
        Ok(window) => window,
        Err(error) => {
            warn!("Could not create the color pattern window: {error}");
            return None;
        }
    };
    crate::gui::preferences::bind_color_pattern_window_theme(&window);
    wire(&window);
    let weak = window.as_weak();
    let solve_weak = weak.clone();
    let solver = SolveWorker::new(move |solved, failed, cancelled| {
        let _ = solve_weak.upgrade_in_event_loop(move |_| {
            if !cancelled.load(Ordering::SeqCst) {
                on_host(|host| actions::solved(host, solved, failed));
            }
        });
    });
    let post_weak = weak.clone();
    let outline_weak = weak;
    let preview = PreviewWorker::new(
        Arc::new(move |ticket: u64, result: Rendered| {
            let _ = post_weak.upgrade_in_event_loop(move |_| {
                on_host(|host| preview::deliver(host, ticket, result));
            });
        }),
        Arc::new(move |key: OutlineKey, outline: Option<Vec<(f64, f64)>>| {
            let _ = outline_weak.upgrade_in_event_loop(move |_| {
                on_host(|host| preview::deliver_outline(host, key, outline));
            });
        }),
    );
    let host = Rc::new(Host {
        window,
        main: main.as_weak(),
        deps: deps.clone(),
        session: RefCell::new(None),
        solver,
        preview,
        picker_timer: Timer::default(),
        tick: Timer::default(),
    });
    set_host(&host);
    // The window has no resize callback of its own: it is watched a few times a second.
    host.tick.start(TimerMode::Repeated, TICK, || {
        on_host(|host| actions::tick(host));
    });
    Some(host)
}

/// Opens the editor on `opening`: creates the window on the first call, then shows it.
pub(super) fn open_session(main: &MainWindow, deps: &Deps, opening: Opening) {
    if with_host(|host| {
        host.session
            .try_borrow()
            .map_or(true, |session| session.is_some())
    })
    .unwrap_or(false)
    {
        show_toast(
            main,
            "Finish or cancel the open Color Pattern editor first.",
            "warning",
        );
        return;
    }
    let Some(host) = with_host(Rc::clone).or_else(|| create(main, deps)) else {
        show_toast(
            main,
            "The color pattern window could not be opened.",
            "error",
        );
        return;
    };
    let saved_presets = {
        let guard = deps.db.lock().unwrap_or_else(PoisonError::into_inner);
        store::load_presets(&guard).unwrap_or_else(|error| {
            warn!("Color pattern presets could not be read: {error:#}");
            Vec::new()
        })
    };
    let preview_design = if opening.stone_apply.is_some() && !opening.targets_design {
        None
    } else {
        crate::gui::editor::native_io::live_design()
    };
    let has_design = preview_design.is_some();
    let width = opening.stone_width_mm;
    let variants = zone_variants(&opening.base_material, Some(deps.db.as_ref()));
    let own = opening.own.unwrap_or_else(|| {
        own_color(
            &opening.base_material,
            None,
            &variants,
            opening.pattern.base_lch(),
            width,
        )
    });
    let rank = allowed_rank(&opening.base_material, opening.two_colors_allowed);
    let session = Session {
        material_name: opening.material_name,
        stone_apply: opening.stone_apply,
        targets_design: opening.targets_design,
        preview_design,
        cache: seed_cache(&opening.pattern),
        stone_width_mm: width,
        path_mm: reference_path_mm(Some(width)),
        rank,
        variants,
        // The Stone view when a design is open, else the Slice.
        view: if has_design { VIEW_STONE } else { VIEW_SLICE },
        saved_presets,
        import_note: opening.import_note,
        ..Session::blank(opening.pattern, opening.base_material, own)
    };
    if let Ok(mut slot) = host.session.try_borrow_mut() {
        *slot = Some(session);
    }
    main.global::<Zoning>().set_pattern_editor_open(true);
    if let Err(error) = host.window.show() {
        warn!("Could not show the color pattern window: {error}");
        actions::close(&host);
        return;
    }
    host.window.window().set_minimized(false);
    actions::ensure_solved(&host);
    actions::refresh_all(&host);
    raise(main, events::PATTERN_EDITOR_OPENED);
}

/// Closes the window and lets its session go (the main window is closing, or being rebuilt).
pub fn close_window() {
    on_host(|host| actions::close(host));
}

/// Forgets the host, the window and the remembered handles of a previous main window.
pub fn release_window_state() {
    if let Some(host) = take_host() {
        host.solver.cancel();
        host.preview.next_ticket();
        let _ = host.window.hide();
    }
    clear_deps();
}

/// The open Color Pattern window, if one was created (the UI harness reads it).
#[cfg(feature = "ui-harness")]
pub fn harness_window() -> Option<ColorPatternWindow> {
    with_host(|host| host.window.clone_strong())
}

/// Opens the window on a Watermelon pattern with solved colors, for the UI harness: no worker
/// and no vault row are needed.
#[cfg(feature = "ui-harness")]
pub fn harness_open(main: &MainWindow) {
    use pleochroma_cut_core::material::pattern::{
        PatternAxis, PatternColor, TemplateId, ZoneColor, template,
    };
    let Some(deps) = super::session::deps() else {
        return;
    };
    let mut pattern = template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL);
    pattern.base.color = PatternColor::Single(ZoneColor::solved(
        [55.0, 35.0, 140.0],
        vec![[600.0, 40.0, 0.5]],
    ));
    pattern.zones[0].color = PatternColor::Single(ZoneColor::solved(
        [60.0, 35.0, 0.0],
        vec![[520.0, 40.0, 0.5]],
    ));
    open_session(
        main,
        &deps,
        Opening {
            material_name: "Harness Material".to_owned(),
            stone_apply: None,
            targets_design: false,
            pattern,
            import_note: None,
            stone_width_mm: 8.0,
            // A sapphire, so the window shows its row of typical patterns.
            base_material: GemMaterial::sapphire(),
            two_colors_allowed: true,
            own: None,
        },
    );
}
