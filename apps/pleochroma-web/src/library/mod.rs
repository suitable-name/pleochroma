//! The browser design library's screen (`ui/library/*.slint`, state in `LibraryModel`).
//!
//! The store is `crate::storage::library` (`IndexedDB`); what the screen shows and what its
//! buttons do is `pleochroma_web_core::library_view`, tested natively. This module is the glue:
//!
//! - [`open_library`] / [`reload`] / [`rebuild`]: read the entries, run the query, push the
//!   combos, the rows and the detail pane. The rows are the first [`PAGE_SIZE`] matches.
//! - Thumbnails are cached PNGs ([`thumbs`]); a row without a decoded one asks for the batch it
//!   is missing once, and draws "No Preview" until it arrives.
//! - [`detail`]: the selected design (edit, open, export, delete).
//! - [`import`]: Import Designs and Save to Library -- parse, measure, store, thumbnail.
//!
//! Nothing here touches the autosave; a failed library call is a notice and the open design is
//! never affected.

mod detail;
mod import;
mod thumbs;

pub use import::add_open_design;

use crate::{
    AppWindow, LibraryModel, LibraryRow,
    app::{
        Ctx,
        push::{MessageKind, show_message},
    },
    storage::library,
};
use pleochroma_editor::guide::viewing_events as events;
use pleochroma_web_core::{
    library_record::{LibraryEntry, StorageEstimate},
    library_view::{
        FilterOptions, LibraryQuery, LibrarySort, PAGE_SIZE, QueryResult, combo_index, combo_items,
        combo_value, filter_options, row_text, run_query, storage_line,
    },
};
use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

/// What the screen knows between calls.
#[derive(Default)]
struct State {
    /// Every readable entry, as last read from the store.
    entries: Vec<LibraryEntry>,
    /// Stored records this build could not read (shown in the storage line).
    unreadable: usize,
    /// What the filter combos offer.
    options: FilterOptions,
    /// The entries the query matched.
    result: QueryResult,
    /// How many of `result.indexes` the list draws.
    shown: usize,
    /// The selected design's id.
    selected: Option<String>,
    /// What the browser said about its storage.
    estimate: Option<StorageEstimate>,
    /// Why the browser cannot keep a library, if it cannot.
    problem: Option<String>,
    /// Thumbnails being fetched right now.
    fetching: HashSet<String>,
    /// The query the list was last run with (what a tutorial's events compare against).
    last_query: LibraryQuery,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    /// Decoded thumbnails by design id; `None` is "looked, there is none".
    static THUMBS: RefCell<HashMap<String, Option<Image>>> = RefCell::new(HashMap::new());
}

/// Runs `f` on the screen's state (never across an `.await`).
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with_borrow_mut(f)
}

/// Forgets the cached picture of `design_id` (its design was replaced or deleted).
fn forget_thumb(design_id: &str) {
    THUMBS.with_borrow_mut(|cache| {
        cache.remove(design_id);
    });
}

/// A model of strings for a combo.
fn string_model(items: Vec<String>) -> ModelRc<SharedString> {
    let items: Vec<SharedString> = items.into_iter().map(SharedString::from).collect();
    Rc::new(VecModel::from(items)).into()
}

/// A combo index from a position, `0` ("All") if it does not fit.
fn index_of(position: usize) -> i32 {
    i32::try_from(position).unwrap_or(0)
}

/// Waits one browser task, so a status line set before it is painted before long work starts.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn next_turn() {
    let promise =
        js_sys::Promise::new(
            &mut |resolve: js_sys::Function, _reject| match web_sys::window() {
                Some(window) => {
                    let _ =
                        window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0);
                }
                None => {
                    let _ = resolve.call0(&JsValue::NULL);
                }
            },
        );
    let _ = JsFuture::from(promise).await;
}

/// Shows (`Some`) or clears (`None`) the busy line under the title.
fn set_busy(ctx: &Ctx, text: Option<&str>) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<LibraryModel>();
    model.set_busy(text.is_some());
    model.set_busy_text(text.unwrap_or_default().into());
}

/// The query the search bar and filters hold now.
fn read_query(model: &LibraryModel<'_>, options: &FilterOptions) -> LibraryQuery {
    LibraryQuery {
        text: model.get_search_text().to_string(),
        shape: combo_value(&options.shapes, model.get_shape_index()),
        gear: combo_value(&options.gears, model.get_gear_index()),
        tag: combo_value(&options.tags, model.get_tag_index()),
        concave_only: model.get_concave_only(),
        show_ignored: model.get_show_ignored(),
        sort: LibrarySort::from_index(model.get_sort_index()),
    }
}

/// The rows to draw and the ids whose thumbnails were never looked up.
fn build_rows() -> (Vec<LibraryRow>, Vec<String>) {
    STATE.with_borrow(|state| {
        THUMBS.with_borrow(|cache| {
            let mut missing = Vec::new();
            let rows = state
                .result
                .indexes
                .iter()
                .take(state.shown)
                .filter_map(|&at| state.entries.get(at))
                .map(|entry| {
                    let text = row_text(entry);
                    let cached = cache.get(&entry.design_id);
                    if cached.is_none() {
                        missing.push(entry.design_id.clone());
                    }
                    let thumb = cached.cloned().flatten();
                    LibraryRow {
                        id: entry.design_id.as_str().into(),
                        title: text.title.into(),
                        subtitle: text.subtitle.into(),
                        tags: text.tags.into(),
                        has_thumb: thumb.is_some(),
                        thumb: thumb.unwrap_or_default(),
                        ignored: entry.item.ignored,
                        selected: state.selected.as_deref() == Some(entry.design_id.as_str()),
                    }
                })
                .collect::<Vec<_>>();
            (rows, missing)
        })
    })
}

/// Draws the list from the state; asks for the thumbnails it lacks.
fn push_rows(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let (rows, missing) = build_rows();
    ui.global::<LibraryModel>()
        .set_rows(ModelRc::new(VecModel::from(rows)));
    if !missing.is_empty() {
        fetch_thumbs(ctx, missing);
    }
}

/// Reads the cached pictures of `ids` once and redraws.
fn fetch_thumbs(ctx: &Ctx, ids: Vec<String>) {
    let ids: Vec<String> = with_state(|state| {
        ids.into_iter()
            .filter(|id| state.fetching.insert(id.clone()))
            .collect()
    });
    if ids.is_empty() {
        return;
    }
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let found = library::get_thumbnails(&ids).await.unwrap_or_default();
        THUMBS.with_borrow_mut(|cache| {
            for id in &ids {
                cache.insert(id.clone(), None);
            }
            for (id, thumbnail) in found {
                if let Some(image) = thumbs::decode_png(&thumbnail.png) {
                    cache.insert(id, Some(image));
                }
            }
        });
        with_state(|state| {
            for id in &ids {
                state.fetching.remove(id);
            }
        });
        push_rows(&ctx);
        detail::push_thumb(&ctx);
    });
}

/// The storage line, the list line and the "Show More" / empty-list flags.
fn push_status(ui: &AppWindow) {
    let (storage, list, more, is_empty) = STATE.with_borrow(|state| {
        let total = state.result.total;
        let list = if state.entries.is_empty() {
            String::new()
        } else if state.shown < total {
            format!("Showing {} of {total} matching designs", state.shown)
        } else if total == 1 {
            "1 matching design".to_owned()
        } else {
            format!("{total} matching designs")
        };
        (
            storage_line(state.entries.len(), state.unreadable, state.estimate),
            list,
            state.shown < state.result.indexes.len(),
            state.entries.is_empty(),
        )
    });
    let model = ui.global::<LibraryModel>();
    model.set_storage_text(storage.into());
    model.set_list_text(list.into());
    model.set_more(more);
    model.set_is_empty(is_empty);
}

/// Pushes why the library is unavailable, or that it is available.
fn push_availability(ui: &AppWindow) {
    let problem = with_state(|state| state.problem.clone());
    let model = ui.global::<LibraryModel>();
    model.set_available(problem.is_none());
    model.set_unavailable_text(problem.unwrap_or_default().into());
}

/// Redraws everything from the entries held: combos (keeping the values chosen), the query,
/// the status, the rows and the detail pane.
fn rebuild(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<LibraryModel>();
    let previous = with_state(|state| read_query(&model, &state.options));
    let options = with_state(|state| {
        state.options = filter_options(&state.entries);
        let selected_gone = state
            .selected
            .as_ref()
            .is_some_and(|id| !state.entries.iter().any(|e| &e.design_id == id));
        if selected_gone {
            state.selected = None;
        }
        state.result = run_query(&state.entries, &previous);
        state.shown = state.result.indexes.len().min(PAGE_SIZE);
        state.last_query = previous.clone();
        state.options.clone()
    });
    model.set_shape_options(string_model(combo_items(&options.shapes)));
    model.set_shape_index(index_of(combo_index(&options.shapes, &previous.shape)));
    model.set_gear_options(string_model(combo_items(&options.gears)));
    model.set_gear_index(index_of(combo_index(&options.gears, &previous.gear)));
    model.set_tag_options(string_model(combo_items(&options.tags)));
    model.set_tag_index(index_of(combo_index(&options.tags, &previous.tag)));
    push_availability(&ui);
    push_status(&ui);
    push_rows(ctx);
    detail::push_selection(ctx);
}

/// Search text, a combo or a chip changed: runs the query again.
fn query_changed(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<LibraryModel>();
    let query = with_state(|state| read_query(&model, &state.options));
    let before = with_state(|state| {
        state.result = run_query(&state.entries, &query);
        state.shown = state.result.indexes.len().min(PAGE_SIZE);
        std::mem::replace(&mut state.last_query, query.clone())
    });
    push_status(&ui);
    push_rows(ctx);
    // A tutorial step may wait for a search, a filter or a reset (state, not "something changed").
    for name in pleochroma_web_core::guide_events::library_events(&before, &query) {
        crate::editor::guide::raise(ctx, name);
    }
}

/// The list's "Show More".
fn show_more(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    with_state(|state| {
        state.shown = (state.shown + PAGE_SIZE).min(state.result.indexes.len());
    });
    push_status(&ui);
    push_rows(ctx);
}

/// Selects the design `id` (the empty id clears the selection).
fn select(ctx: &Ctx, id: &str) {
    with_state(|state| {
        state.selected = (!id.is_empty() && state.entries.iter().any(|e| e.design_id == id))
            .then(|| id.to_owned());
    });
    let chosen = with_state(|state| state.selected.is_some());
    detail::push_selection(ctx);
    push_rows(ctx);
    // A tutorial step may wait for a design to be selected (each click, as on the desktop).
    if chosen {
        crate::editor::guide::raise(ctx, events::LIBRARY_DESIGN_SELECTED);
    }
}

/// Reads the store and redraws. A library the browser cannot keep is said so on the screen.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn reload(ctx: &Ctx) {
    let listed = library::list_entries().await;
    let estimate = library::storage_estimate().await.ok();
    match listed {
        Ok(list) => with_state(|state| {
            state.entries = list.entries;
            state.unreadable = list.unreadable;
            state.estimate = estimate;
            state.problem = None;
        }),
        Err(error) => with_state(|state| {
            state.entries.clear();
            state.unreadable = 0;
            state.estimate = None;
            state.problem = Some(error.user_text());
        }),
    }
    rebuild(ctx);
    set_busy(ctx, None);
}

/// Opens the screen and reads the library.
fn open_library(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    ui.global::<LibraryModel>().set_open(true);
    set_busy(ctx, Some("Loading the library..."));
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        reload(&ctx).await;
    });
}

/// Closes the screen.
fn close(ctx: &Ctx) {
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<LibraryModel>().set_open(false);
    }
}

/// Redraws the screen from the store if it is open (a save or import finished elsewhere).
fn refresh_if_open(ctx: &Ctx) {
    let open = ctx
        .ui
        .upgrade()
        .is_some_and(|ui| ui.global::<LibraryModel>().get_open());
    if open {
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            reload(&ctx).await;
        });
    }
}

/// A notice in the toast.
fn notify(ctx: &Ctx, kind: MessageKind, text: &str) {
    show_message(ctx, kind, text);
}

/// Registers the library's callbacks and tells the tutorials whether a library exists.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<LibraryModel>();
    model.set_sort_options(string_model(
        LibrarySort::ALL
            .iter()
            .map(|sort| sort.label().to_owned())
            .collect(),
    ));
    let c = ctx.clone();
    model.on_open_library(move || open_library(&c));
    let c = ctx.clone();
    model.on_close(move || close(&c));
    let c = ctx.clone();
    model.on_import_files(move || {
        let inner = c.clone();
        crate::io::picker::pick_design_files(&c, move |files| import::import_files(&inner, files));
    });
    let c = ctx.clone();
    model.on_save_current(move || crate::io::save::save_to_library(&c));
    let c = ctx.clone();
    model.on_query_changed(move || query_changed(&c));
    let c = ctx.clone();
    model.on_show_more(move || show_more(&c));
    let c = ctx.clone();
    model.on_select(move |id| select(&c, &id));
    let c = ctx.clone();
    model.on_open_selected(move || detail::open_selected(&c));
    let c = ctx.clone();
    model.on_export_selected(move || detail::export_selected(&c));
    let c = ctx.clone();
    model.on_delete_selected(move || detail::delete_selected(&c));
    let c = ctx.clone();
    model.on_apply_detail(move || detail::apply_detail(&c));
    let c = ctx.clone();
    model.on_revert_detail(move || detail::push_selection(&c));

    // Is this browser able to keep a library? The tutorials offer the library lessons only if so.
    let c = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        match library::probe().await {
            Ok(()) => crate::editor::guide::set_library_available(true),
            Err(error) => {
                with_state(|state| state.problem = Some(error.user_text()));
                if let Some(ui) = c.ui.upgrade() {
                    push_availability(&ui);
                }
            }
        }
    });
}
