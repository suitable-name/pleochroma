//! The Material Reference window: a separate, non-modal window with the table of every
//! material (sortable, filterable, searchable) and, behind a row, the details page of one
//! material (index curves, spectra, colors, variants and gem data).
//!
//! # Where the pieces are
//!
//! - [`session`]: the window's state as plain data and its rules (sort, filter, selection, page,
//!   Copy Table text). No Slint.
//! - [`prefs`]: the sort, filter, search text and dispersion scale remembered in the settings.
//! - [`push`]: copies a session and a `MaterialDetail` into `MaterialReferenceModel`.
//! - [`designs`]: the details page's list "Your designs that work here", from the vault's stored
//!   RI ranges.
//! - [`actions`]: Preview As, Duplicate as Custom and Use for Design, through the app's
//!   existing paths.
//! - This file: creating, showing and focusing the window, and the callbacks between the
//!   window's global and the session. The rows and the details come from
//!   `pleochroma_editor::material_reference`, the one view model the web, Android and CLI
//!   front ends share.
//!
//! The window is created on first use and then only hidden and shown, like the Compare window.
//! Opening it again focuses it and selects the requested material. Everything here runs on the
//! UI thread.

mod actions;
mod designs;
mod prefs;
mod push;
mod session;
#[cfg(test)]
mod tests;

use crate::{
    EditorModel, HelpModel, LibraryModel, MainWindow, MaterialReferenceModel,
    MaterialReferenceWindow, RefRow, RiBatchModel,
    bridge::render_thread::RenderContext,
    gui::{library::clipboard::copy_to_clipboard_with_toast, show_toast},
};
use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::MaterialCatalogue;
use pleochroma_editor::{
    material_lookup::EditorMaterialLookup,
    material_reference::{
        CustomGemData, DetailContext, DispersionScale, MaterialDetail, ReferenceRow,
        material_detail_for_variant, reference_rows, text::copied_text,
    },
};
use prefs::{ViewPrefs, column_from_index, filter_from_index};
use session::{Selection, Session};
use slint::{ComponentHandle, Model, Weak, winit_030::WinitWindowAccessor};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};
use tracing::warn;

/// What the window needs from the main window: a handle to it and the render context that
/// holds the custom materials.
struct Deps {
    /// The main window.
    ui: Weak<MainWindow>,
    /// The render context (custom materials and their specific gravities).
    render_ctx: Arc<Mutex<RenderContext>>,
}

/// The window and what it shows.
struct Host {
    /// The window, created on first use.
    window: MaterialReferenceWindow,
    /// The rows, the view and the selection.
    session: RefCell<Session>,
    /// The details page's data, for the material and variant it was built for.
    detail: RefCell<Option<MaterialDetail>>,
}

thread_local! {
    /// The main window's handles, set once at start-up. UI-thread-only.
    static DEPS: RefCell<Option<Rc<Deps>>> = const { RefCell::new(None) };
    /// The window, once it has been opened. UI-thread-only.
    static HOST: RefCell<Option<Rc<Host>>> = const { RefCell::new(None) };
}

/// The main window's handles, `None` before [`setup`] and after [`release_window_state`].
fn deps() -> Option<Rc<Deps>> {
    DEPS.with(|cell| cell.borrow().clone())
}

/// The window, once it has been created.
fn host() -> Option<Rc<Host>> {
    HOST.with(|cell| cell.borrow().clone())
}

/// Wires `EditorModel.open_material_reference` (the Stone tab's "Material Details..." button, the
/// Help menu and the command palette) to [`open`]. Called once, when the main window is built.
pub(in crate::gui) fn setup(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
    DEPS.with(|cell| {
        *cell.borrow_mut() = Some(Rc::new(Deps {
            ui: ui.as_weak(),
            render_ctx: Arc::clone(render_ctx),
        }));
    });
    ui.global::<EditorModel>()
        .on_open_material_reference(|name| open(name.as_str()));
}

/// Hides the window and drops its state: the main window is closing, so the window must not keep
/// the event loop alive.
pub(in crate::gui) fn close_window() {
    if let Some(host) = host() {
        let _ = host.window.hide();
    }
}

/// Forgets the window and the main-window handles, so the next main window starts clean.
/// See `gui::window_release`.
pub(in crate::gui) fn release_window_state() {
    let old = HOST.with(|cell| cell.borrow_mut().take());
    if let Some(host) = &old {
        let _ = host.window.hide();
    }
    drop(old);
    let deps = DEPS.with(|cell| cell.borrow_mut().take());
    drop(deps);
    designs::release();
}

pub(in crate::gui) use designs::set_database as set_designs_database;

/// Refills the details page's list "Your designs that work here" after ranges were stored (the
/// Compute RI Ranges batch ended). Does nothing while the window was never opened.
pub(in crate::gui) fn designs_changed() {
    let Some(host) = host() else {
        return;
    };
    let ri = host.session.borrow().selected_ri();
    designs::push(&host.window, ri);
}

/// Whether the window holds anything of a previous main window. UI harness only.
#[cfg(feature = "ui-harness")]
pub(in crate::gui) fn holds_state() -> bool {
    host().is_some() || deps().is_some() || designs::holds_database()
}

/// The window once created (it may be hidden). UI harness only.
#[cfg(feature = "ui-harness")]
pub fn harness_window() -> Option<MaterialReferenceWindow> {
    host().map(|host| host.window.clone_strong())
}

/// The custom materials with their specific gravities and the gem data the user entered for
/// them.
struct CustomMaterials {
    /// The custom materials.
    materials: Vec<GemMaterial>,
    /// Each custom material's specific gravity, by name.
    specific_gravity: Vec<(String, f64)>,
    /// The gem data entered for each custom material, by name.
    gem_data: Vec<(String, CustomGemData)>,
}

/// The custom materials, their specific gravities and the gem data the user entered for them, as
/// the render context holds them now.
fn custom_materials(deps: &Deps) -> CustomMaterials {
    let ctx = RenderContext::lock(&deps.render_ctx);
    CustomMaterials {
        materials: (*ctx.custom_materials).clone(),
        specific_gravity: (*ctx.custom_material_specific_gravity).clone(),
        gem_data: (*ctx.custom_material_gem_data).clone(),
    }
}

/// The table rows of every built-in and custom material.
fn build_rows(deps: &Deps) -> Vec<ReferenceRow> {
    let CustomMaterials {
        materials: custom,
        specific_gravity: custom_sg,
        gem_data: custom_gem,
    } = custom_materials(deps);
    let catalogue = MaterialCatalogue::build_with_sg(&custom, &custom_sg);
    let lookup = EditorMaterialLookup::new(&custom);
    reference_rows(&catalogue, &lookup, &custom_gem)
}

/// The details of `name` showing the color variant `variant_key`, for the open design's stone
/// width (7 mm without one).
fn build_detail(
    deps: &Deps,
    ui: &MainWindow,
    name: &str,
    variant_key: &str,
) -> Option<MaterialDetail> {
    let CustomMaterials {
        materials: custom,
        specific_gravity: custom_sg,
        gem_data: custom_gem,
    } = custom_materials(deps);
    let lookup = EditorMaterialLookup::new(&custom);
    let context = DetailContext {
        stone_width_mm: actions::design_width_mm(ui),
        custom_gem: custom_gem
            .iter()
            .find(|(gem_name, _)| gem_name.eq_ignore_ascii_case(name))
            .map(|(_, data)| data.clone()),
        custom_sg: custom_sg
            .iter()
            .find(|(sg_name, _)| sg_name.eq_ignore_ascii_case(name))
            .map(|(_, sg)| *sg),
        user_variants: crate::gui::optics::color_variant_store::saved_variants(name),
        design_volume: None,
    };
    material_detail_for_variant(name, &lookup, &context, variant_key)
}

/// Opens the window (creating it on first use), or focuses it when it is open, on the table with
/// `requested` selected. An empty name selects the current material: the open design's, else the
/// one the viewport shows.
pub(in crate::gui) fn open(requested: &str) {
    let Some(deps) = deps() else {
        return;
    };
    let Some(ui) = deps.ui.upgrade() else {
        return;
    };
    let Some(host) = ensure_host(&deps, &ui) else {
        return;
    };
    let name = if requested.trim().is_empty() {
        actions::current_material(&ui)
    } else {
        requested.trim().to_owned()
    };
    let view_changed = {
        let mut session = host.session.borrow_mut();
        session.set_rows(build_rows(&deps));
        session.back();
        match session.select(&name) {
            Selection::Selected { view_changed } => view_changed,
            Selection::Unknown => false,
        }
    };
    if view_changed {
        host.session.borrow().prefs().save();
    }
    *host.detail.borrow_mut() = None;
    let session = host.session.borrow();
    let model = host.window.global::<MaterialReferenceModel>();
    push::push_table(&model, &session);
    push::clear_detail(&model);
    drop(session);
    if let Err(error) = host.window.show() {
        warn!("Could not show the Material Reference window: {error}");
        show_toast(
            &ui,
            "Could not open the Material Reference window.",
            "error",
        );
        return;
    }
    host.window.window().set_minimized(false);
    // The platform window is raised on the first show by itself; this is for a window that is
    // already open behind the main one.
    let _ = host
        .window
        .window()
        .with_winit_window(slint::winit_030::winit::window::Window::focus_window);
}

/// The window, created and wired on first use.
fn ensure_host(deps: &Rc<Deps>, ui: &MainWindow) -> Option<Rc<Host>> {
    if let Some(host) = host() {
        return Some(host);
    }
    let window = match MaterialReferenceWindow::new() {
        Ok(window) => window,
        Err(error) => {
            warn!("Could not create the Material Reference window: {error}");
            show_toast(ui, "Could not open the Material Reference window.", "error");
            return None;
        }
    };
    // Starts in the current theme and follows later changes.
    crate::gui::preferences::bind_material_reference_window_theme(&window);
    let session = Session::new(build_rows(deps), ViewPrefs::load());
    let host = Rc::new(Host {
        window,
        session: RefCell::new(session),
        detail: RefCell::new(None),
    });
    register_callbacks(&host);
    HOST.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&host)));
    Some(host)
}

/// Runs `f` with the window and the main window, if both still exist.
fn with_host(f: impl FnOnce(&Host, &MainWindow)) {
    let (Some(host), Some(deps)) = (host(), deps()) else {
        return;
    };
    let Some(ui) = deps.ui.upgrade() else {
        return;
    };
    f(&host, &ui);
}

/// Takes the table's current selection into the session: a click only moves `selected` in the
/// window, so the session learns of it from the row the window highlights.
fn sync_selection(host: &Host) {
    let model = host.window.global::<MaterialReferenceModel>();
    let Ok(index) = usize::try_from(model.get_selected()) else {
        return;
    };
    let row: Option<RefRow> = model.get_rows().row_data(index);
    if let Some(row) = row {
        host.session.borrow_mut().select(row.name.as_str());
    }
}

/// Pushes the table and saves the view.
fn table_changed(host: &Host) {
    let session = host.session.borrow();
    session.prefs().save();
    push::push_table(&host.window.global::<MaterialReferenceModel>(), &session);
}

/// Builds the details of the selected material and pushes them.
fn refresh_detail(host: &Host, ui: &MainWindow) {
    let Some(deps) = deps() else {
        return;
    };
    let (name, variant_key, transmission) = {
        let session = host.session.borrow();
        (
            session.selected().map(str::to_owned),
            session.variant_key().to_owned(),
            session.transmission(),
        )
    };
    let model = host.window.global::<MaterialReferenceModel>();
    let Some(name) = name else {
        push::clear_detail(&model);
        return;
    };
    let detail = build_detail(&deps, ui, &name, &variant_key);
    detail.as_ref().map_or_else(
        || push::clear_detail(&model),
        |detail| push::push_detail(&model, detail, transmission, actions::design_is_open(ui)),
    );
    *host.detail.borrow_mut() = detail;
    designs::push(&host.window, host.session.borrow().selected_ri());
}

/// Registers the table's callbacks: sort, filter, search, scale, and opening a row.
fn register_table_callbacks(model: &MaterialReferenceModel<'_>) {
    model.on_sort_clicked(|index| {
        with_host(|host, _| {
            sync_selection(host);
            if let Some(column) = column_from_index(index) {
                host.session.borrow_mut().click_sort(column);
                table_changed(host);
            }
        });
    });
    model.on_filter_chosen(|index| {
        with_host(|host, _| {
            sync_selection(host);
            if let Some(filter) = filter_from_index(index) {
                host.session.borrow_mut().set_filter(filter);
                table_changed(host);
            }
        });
    });
    model.on_query_edited(|query| {
        with_host(|host, _| {
            sync_selection(host);
            host.session.borrow_mut().set_query(query.as_str());
            table_changed(host);
        });
    });
    model.on_scale_chosen(|blue_to_green| {
        with_host(|host, _| {
            sync_selection(host);
            host.session.borrow_mut().set_scale(if blue_to_green {
                DispersionScale::BG
            } else {
                DispersionScale::FC
            });
            table_changed(host);
        });
    });
    model.on_row_opened(|index| {
        with_host(|host, ui| {
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            if host.session.borrow_mut().open_row(index).is_none() {
                return;
            }
            refresh_detail(host, ui);
            push::push_table(
                &host.window.global::<MaterialReferenceModel>(),
                &host.session.borrow(),
            );
        });
    });
    model.on_back(|| {
        with_host(|host, _| {
            host.session.borrow_mut().back();
            push::push_table(
                &host.window.global::<MaterialReferenceModel>(),
                &host.session.borrow(),
            );
        });
    });
}

/// Brings the main window forward.
fn show_main_window(ui: &MainWindow) {
    ui.window().set_minimized(false);
    let _ = ui.window().show();
}

/// Opens the design `id` of "Your designs that work here" in the main window's library.
fn open_design(ui: &MainWindow, id: i32) {
    ui.global::<LibraryModel>().invoke_select_diagram(id);
    show_main_window(ui);
}

/// Opens the Compute RI Ranges dialog in the main window.
fn open_ri_batch(ui: &MainWindow) {
    ui.global::<RiBatchModel>().invoke_open_batch();
    show_main_window(ui);
}

/// Registers the details page's callbacks: close, copy, preview, variants and the spectrum mode.
fn register_detail_callbacks(model: &MaterialReferenceModel<'_>) {
    // Escape: the same as the title bar's close button.
    model.on_close(|| {
        with_host(|host, _| {
            let _ = host.window.hide();
        });
    });
    model.on_copy_table(|| {
        with_host(|host, ui| {
            let (text, count) = host.session.borrow().copy_text();
            copy_to_clipboard_with_toast(ui, text, copied_text(count));
        });
    });
    model.on_preview_as(|name| with_host(|_, ui| actions::preview_as(ui, name.as_str())));
    model.on_duplicate_as_custom(|name| {
        with_host(|_, ui| actions::duplicate_as_custom(ui, name.as_str()));
    });
    model.on_variant_selected(|index| {
        with_host(|host, ui| {
            let key = usize::try_from(index).ok().and_then(|index| {
                host.detail
                    .borrow()
                    .as_ref()
                    .and_then(|detail| detail.variants.get(index).map(|v| v.key.clone()))
            });
            let Some(key) = key else {
                return;
            };
            host.session.borrow_mut().set_variant_key(&key);
            refresh_detail(host, ui);
        });
    });
    model.on_use_variant(|index| {
        with_host(|host, ui| {
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            if let Some(detail) = host.detail.borrow().as_ref() {
                actions::use_variant(ui, detail, index);
            }
        });
    });
    model.on_design_clicked(|id| with_host(|_, ui| open_design(ui, id)));
    model.on_compute_ri_ranges(|| with_host(|_, ui| open_ri_batch(ui)));
    model.on_spectrum_mode(|mode| {
        with_host(|host, ui| {
            host.session.borrow_mut().set_transmission(mode == 1);
            refresh_detail(host, ui);
        });
    });
}

/// Registers the window's callbacks, once, when the window is created.
fn register_callbacks(host: &Host) {
    let model = host.window.global::<MaterialReferenceModel>();
    register_table_callbacks(&model);
    register_detail_callbacks(&model);
    // The window's "?" button: its own `HelpModel` instance hands the topic to the main window,
    // which owns the help window.
    host.window.global::<HelpModel>().on_open_topic(|topic| {
        with_host(|_, ui| {
            if let Err(message) = crate::gui::help::open_topic(ui, topic.as_str()) {
                show_toast(ui, &message, "error");
            }
        });
    });
}
