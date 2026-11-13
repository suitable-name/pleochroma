//! Converts a catalogue row into the Slint-facing `DiagramItem`, runs the local
//! `Database` read behind a search/filter, and applies the result onto `ui`.

use crate::{
    DiagramItem, LibraryModel, MainWindow,
    gui::library::{detail::clear_current_detail_display, read_access::with_read_db},
};
use anyhow::Result;
use pleochroma_net::library::DesignSummary;
use pleochroma_vault::{
    db::sqlite::{Database, DisplayFilters, SortOrder},
    model::{
        entry::DiagramListItem,
        filter::{AttributeRanges, RangeFilter},
    },
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, Mutex},
};
use tracing::warn;

/// Whether `url` is the synthetic `local://` URL of a locally imported design.
///
/// Shared by [`to_diagram_item`] and [`row_to_item`], so the "Mine" badge is decided the same
/// way for local and remote rows.
fn is_local_url(url: &str) -> bool {
    url.starts_with("local://")
}

/// Converts a database id to the `i32` a Slint `DiagramItem.id` holds. Ids are
/// auto-increment row ids far below `i32::MAX`; saturating keeps the conversion total
/// and identical for the row-to-item and the row-matches-item comparison.
pub(super) fn slint_id(id: i64) -> i32 {
    i32::try_from(id).unwrap_or(i32::MAX)
}

/// Converts a result count to the `i32` a Slint count property holds (saturating; the
/// counts are capped by the query and nowhere near `i32::MAX`).
pub(super) fn slint_count(count: usize) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// Converts a remote [`DesignSummary`] to a list item. The ignored flag and `is_local` come
/// from the same fields a local row carries, so a remote row is classified the same way.
pub(super) fn to_diagram_item(item: &DesignSummary) -> DiagramItem {
    DiagramItem {
        id: slint_id(item.entry_id),
        title: item.title.clone().into(),
        shape: item.shape.clone().unwrap_or_default().into(),
        gear: item.index_gear.clone().unwrap_or_default().into(),
        facets: item.facets_count.clone().unwrap_or_default().into(),
        designer: item.designer_info.clone().unwrap_or_default().into(),
        lw_ratio: item.lw_ratio.clone().unwrap_or_default().into(),
        ri: item.refractive_index.clone().unwrap_or_default().into(),
        ignored: item.ignored,
        is_local: is_local_url(&item.url),
        // The planner mark and tags exist only in the local database, and the remote
        // protocol carries neither, so a remote row shows neither.
        planner_excluded: false,
        tags: ModelRc::new(VecModel::from(Vec::<slint::SharedString>::new())),
    }
}

/// One row [`fetch_diagram_list`] returns: the plain catalogue row plus whether it is
/// currently marked ignored.
pub struct DiagramListRow {
    pub item: DiagramListItem,
    /// `true` iff this row is a currently-ignored design being shown because
    /// `RangeFilter::include_ignored` is on. Always `false` while that toggle is off --
    /// an ignored row can't reach this struct in that case, since it's excluded by the
    /// query itself.
    pub ignored: bool,
    /// `true` iff the Rough Planner leaves this design out of its candidate set
    /// (`Database::planner_excluded_among`). Independent of `ignored`: an excluded
    /// design is listed and searchable like any other.
    pub planner_excluded: bool,
    /// This row's tag names. [`fetch_diagram_list_with_options`] fetches them for all rows in
    /// one query (`Database::tags_for_entries`).
    pub tags: Vec<String>,
}

/// [`fetch_diagram_list`]'s full result.
///
/// The rows, the catalogue's total design count (unrelated to how many matched), the
/// real match count for the active search/filters (see
/// [`pleochroma_vault::db::sqlite::Database::count_matching_diagrams`], distinct from
/// both `total` and `rows.len()`, which is capped), and how many otherwise-matching
/// designs a currently-active tilt-performance filter excluded for having no computed
/// curves at all -- see
/// `pleochroma_vault::model::filter::PerformanceSearchResult::excluded_for_missing_curves`'s
/// doc comment for what that count does and doesn't include.
pub struct FetchedDiagramList {
    pub rows: Vec<DiagramListRow>,
    pub total: usize,
    pub matched_count: usize,
    pub excluded_for_missing_curves: usize,
    /// `true` when `total` and/or `matched_count` above is a FALLBACK value because
    /// the real `COUNT(*)` query behind it failed, rather than being swallowed
    /// silently via `unwrap_or(result.items.len())`, which would make a broken count
    /// query look exactly like "every matching design fit on this page" instead of an
    /// error.
    /// [`apply_diagram_list_to_ui`] turns this into a `status_message` note; the rows
    /// that DID come back are still rendered regardless.
    pub count_unavailable: bool,
}

/// The `Database` read half of `super::dispatch::refresh_diagram_list`, with no
/// sort/"My designs" preference.
///
/// [`fetch_diagram_list_with_options`] at [`SortOrder::CatalogueOrder`] and
/// `local_only: false`. Used by the post-write refresh in `gui::library::local::helpers`.
///
/// # Errors
///
/// Returns an error under the same conditions as [`fetch_diagram_list_with_options`].
pub fn fetch_diagram_list(
    db_mutex: &Arc<Mutex<Database>>,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilter,
) -> Result<FetchedDiagramList> {
    fetch_diagram_list_with_options(
        db_mutex,
        search,
        shape_filter,
        gear_filter,
        range,
        DisplaySearchOptions::default(),
    )
}

/// [`fetch_diagram_list_with_options`]'s sort and restriction options. They are bundled to
/// keep that function within clippy's `too_many_arguments` limit.
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplaySearchOptions<'a> {
    pub order: SortOrder,
    pub local_only: bool,
    /// The tag chip filter's NAME -- see `super::filters::read_tag_filter`'s
    /// own doc comment for why this stays a name this far down, resolved to an id
    /// only once the `Database` lock is already held below.
    pub tag_filter: Option<&'a str>,
    /// "Show these N" restriction to an explicit id set -- see
    /// `super::filters::read_id_filter`'s own doc comment.
    pub id_filter: Option<&'a [i64]>,
}

/// [`fetch_diagram_list`] with a caller-chosen sort and restrictions.
///
/// What `super::dispatch::refresh_diagram_list` calls, with the options read off the UI via
/// `super::filters`.
///
/// It takes no `ui`, so it can run on a background thread. Only the cheap
/// [`apply_diagram_list_to_ui`] step needs the UI thread.
///
/// # Errors
///
/// Returns an error if `Database::search_diagrams_display_with_count` fails (a bad
/// filter combination, or the connection itself). `get_total_count`'s own failure is
/// tolerated instead (see [`FetchedDiagramList::count_unavailable`]).
pub fn fetch_diagram_list_with_options(
    db_mutex: &Arc<Mutex<Database>>,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilter,
    options: DisplaySearchOptions<'_>,
) -> Result<FetchedDiagramList> {
    // Holds the lock only for the query: a UI-thread callback may be waiting on it.
    let db = db_mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let result = fetch_diagram_list_from_db(&db, search, shape_filter, gear_filter, range, options);
    drop(db);
    result
}

/// The pure `Database` half of [`fetch_diagram_list_with_options`] -- operates on an
/// already-open connection directly, with no locking of its own, so a caller that
/// holds its OWN dedicated connection (see [`fetch_diagram_list_with_own_connection`])
/// never touches the shared `Mutex<Database>` at all.
fn fetch_diagram_list_from_db(
    db: &Database,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilter,
    options: DisplaySearchOptions<'_>,
) -> Result<FetchedDiagramList> {
    let clean_shape = if shape_filter == "All Shapes" {
        "All"
    } else {
        shape_filter
    };
    let clean_gear = if gear_filter == "All Gears" {
        "All"
    } else {
        gear_filter
    };

    // A tag name that no longer resolves (deleted between the UI reading its
    // chip and this call) is treated as no restriction rather than an error --
    // consistent with `shape_filter`/`gear_filter`'s own "not found = All"
    // tolerance a few lines up.
    let tag_filter_id = options
        .tag_filter
        .and_then(|name| db.tag_id_by_name(name).ok().flatten());
    let filters = DisplayFilters {
        order: options.order,
        local_only: options.local_only,
        tag_filter: tag_filter_id,
        id_filter: options.id_filter,
    };
    // One candidate walk gives both the page and the match count. Decoding the tilt
    // curves twice would cost time (see `Database::search_diagrams_display_with_count`).
    let (result, matched_count) =
        db.search_diagrams_display_with_count(search, clean_shape, clean_gear, range, filters)?;

    // `get_total_count` is a separate, whole-catalogue query (unrelated to
    // the search/filter above) whose own failure is handled explicitly rather than
    // swallowed into `result.items.len()` with nothing on screen saying so -- see
    // `FetchedDiagramList::count_unavailable`'s own doc comment.
    let mut count_unavailable = false;
    let total = db.get_total_count().unwrap_or_else(|e| {
        warn!("get_total_count failed: {e}");
        count_unavailable = true;
        result.items.len()
    });

    // One query for all rows' tags. A failed lookup shows no tags rather than failing the
    // whole fetch. An empty result skips the query.
    let ids: Vec<i64> = result.items.iter().map(|item| item.id).collect();
    let mut tags_by_entry = if ids.is_empty() {
        HashMap::new()
    } else {
        db.tags_for_entries(&ids).unwrap_or_default()
    };
    // The planner-exclusion marks of the same ids, in one query. This runs on the
    // read-only connection too; a failure degrades to "none excluded" like the tags do.
    let planner_excluded_ids = db.planner_excluded_among(&ids).unwrap_or_else(|e| {
        warn!("planner_excluded_among failed: {e}");
        BTreeSet::new()
    });

    // `ignored` is read straight off the row (`DiagramListItem::ignored`).
    let rows: Vec<DiagramListRow> = result
        .items
        .into_iter()
        .map(|item| {
            let ignored = item.ignored;
            let planner_excluded = planner_excluded_ids.contains(&item.id);
            let tags = tags_by_entry.remove(&item.id).unwrap_or_default();
            DiagramListRow {
                item,
                ignored,
                planner_excluded,
                tags,
            }
        })
        .collect();

    Ok(FetchedDiagramList {
        rows,
        total,
        matched_count,
        excluded_for_missing_curves: result.excluded_for_missing_curves,
        count_unavailable,
    })
}

/// Runs [`fetch_diagram_list_from_db`] on a fresh, dedicated read-only connection.
///
/// A slow performance-filtered query holds the shared `db_mutex` for its whole run, which would
/// block UI-thread reads too. WAL lets a second connection read alongside the writer.
///
/// Falls back to the shared handle (see [`with_read_db`]) when no second connection can be
/// opened. That is the case for the `:memory:` fallback database of `gui::run_gui`, which
/// exists only inside the open connection.
pub(super) fn fetch_diagram_list_with_own_connection(
    db_mutex: &Arc<Mutex<Database>>,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilter,
    options: DisplaySearchOptions<'_>,
) -> Result<FetchedDiagramList> {
    with_read_db(db_mutex, |db| {
        fetch_diagram_list_from_db(db, search, shape_filter, gear_filter, range, options)
    })
}

/// [`fetch_diagram_list_with_own_connection`] plus the attribute ranges, on one connection.
///
/// Used by the post-write refresh (`gui::library::local::helpers`), so that it does not hold
/// the shared `Mutex<Database>`.
pub(in crate::gui) fn fetch_list_and_ranges_with_own_connection(
    db_mutex: &Arc<Mutex<Database>>,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilter,
    options: DisplaySearchOptions<'_>,
) -> (Option<AttributeRanges>, Result<FetchedDiagramList>) {
    with_read_db(db_mutex, |db| {
        let ranges = db.get_attribute_ranges().ok();
        let list =
            fetch_diagram_list_from_db(db, search, shape_filter, gear_filter, range, options);
        (ranges, list)
    })
}

/// The Slint-facing row for one fetched catalogue row.
fn row_to_item(row: DiagramListRow) -> DiagramItem {
    DiagramItem {
        id: slint_id(row.item.id),
        title: row.item.title.into(),
        shape: row.item.shape.unwrap_or_default().into(),
        gear: row.item.index_gear.unwrap_or_default().into(),
        facets: row.item.facets_count.unwrap_or_default().into(),
        designer: row.item.designer_info.unwrap_or_default().into(),
        lw_ratio: row.item.lw_ratio.unwrap_or_default().into(),
        ri: row.item.refractive_index.unwrap_or_default().into(),
        ignored: row.ignored,
        is_local: is_local_url(&row.item.url),
        planner_excluded: row.planner_excluded,
        // This row's tag chips.
        tags: ModelRc::new(VecModel::from(
            row.tags
                .into_iter()
                .map(Into::into)
                .collect::<Vec<SharedString>>(),
        )),
    }
}

/// Whether `model` already shows exactly `rows`: the same designs in the same order,
/// with every displayed field (not only the id) equal -- a rename or a tag change keeps
/// the id but must still replace the list.
fn list_unchanged(model: &ModelRc<DiagramItem>, rows: &[DiagramListRow]) -> bool {
    model.row_count() == rows.len()
        && model
            .iter()
            .zip(rows)
            .all(|(shown, row)| row_matches_item(row, &shown))
}

/// Whether `shown` is what [`row_to_item`] would build from `row`.
fn row_matches_item(row: &DiagramListRow, shown: &DiagramItem) -> bool {
    let item = &row.item;
    shown.id == slint_id(item.id)
        && shown.title.as_str() == item.title
        && text_matches(&shown.shape, item.shape.as_deref())
        && text_matches(&shown.gear, item.index_gear.as_deref())
        && text_matches(&shown.facets, item.facets_count.as_deref())
        && text_matches(&shown.designer, item.designer_info.as_deref())
        && text_matches(&shown.lw_ratio, item.lw_ratio.as_deref())
        && text_matches(&shown.ri, item.refractive_index.as_deref())
        && shown.ignored == row.ignored
        && shown.planner_excluded == row.planner_excluded
        && shown.is_local == is_local_url(&item.url)
        && shown.tags.row_count() == row.tags.len()
        && shown
            .tags
            .iter()
            .zip(&row.tags)
            .all(|(shown_tag, tag)| shown_tag.as_str() == tag.as_str())
}

/// Whether the displayed `shown` text equals `value`, an absent value reading as empty.
fn text_matches(shown: &SharedString, value: Option<&str>) -> bool {
    shown.as_str() == value.unwrap_or_default()
}

/// Applies a fetched list to the window, on the UI thread.
///
/// A selected design that the new result no longer contains is deselected, and its detail
/// pane is cleared. The design still exists, so nothing else on screen should show it.
///
/// The list model is replaced only when `fetched` differs from what is shown (see
/// [`list_unchanged`]). The counts and the other properties are always updated.
pub fn apply_diagram_list_to_ui(ui: &MainWindow, fetched: FetchedDiagramList) {
    let selected_id = ui.global::<LibraryModel>().get_selected_entry_id();
    if selected_id >= 0
        && !fetched
            .rows
            .iter()
            .any(|row| row.item.id == i64::from(selected_id))
    {
        clear_current_detail_display(ui);
    }
    let count_unavailable = fetched.count_unavailable;

    // A refresh that returns the very rows already on screen (a filter tick that changed
    // nothing, a keystroke that did not narrow the result) keeps the shown model: a new
    // one would make the list re-create every visible card and reset the keyboard focus
    // for no visible change.
    if !list_unchanged(
        &ui.global::<LibraryModel>().get_diagram_list(),
        &fetched.rows,
    ) {
        let slint_items: Vec<DiagramItem> = fetched.rows.into_iter().map(row_to_item).collect();
        ui.global::<LibraryModel>()
            .set_diagram_list(ModelRc::new(VecModel::from(slint_items)));
    }
    ui.global::<LibraryModel>()
        .set_total_count(slint_count(fetched.total));
    ui.global::<LibraryModel>()
        .set_matched_count(slint_count(fetched.matched_count));
    // Counts designs in one query's result set, which the query caps.
    ui.global::<LibraryModel>()
        .set_performance_excluded_count(slint_count(fetched.excluded_for_missing_curves));

    // Shown in plain text, so a fallback total is not mistaken for the real one. The rows
    // are still rendered.
    if count_unavailable {
        ui.global::<LibraryModel>()
            .set_status_message("Count unavailable.".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, title: &str, tags: &[&str]) -> DiagramListRow {
        DiagramListRow {
            item: DiagramListItem {
                id,
                title: title.to_string(),
                url: format!("local://{id}"),
                design_id: None,
                shape: Some("Round".to_string()),
                index_gear: None,
                facets_count: Some("57+8".to_string()),
                designer_info: None,
                lw_ratio: None,
                refractive_index: None,
                volume: None,
                competition_diagram: None,
                ignored: false,
            },
            ignored: false,
            planner_excluded: false,
            tags: tags.iter().map(|&tag| tag.to_string()).collect(),
        }
    }

    fn shown(rows: Vec<DiagramListRow>) -> ModelRc<DiagramItem> {
        let items: Vec<DiagramItem> = rows.into_iter().map(row_to_item).collect();
        ModelRc::new(VecModel::from(items))
    }

    fn catalogue() -> Vec<DiagramListRow> {
        vec![row(1, "Alpha", &[]), row(2, "Beta", &["keep"])]
    }

    #[test]
    fn the_rows_a_model_was_built_from_are_unchanged() {
        assert!(list_unchanged(&shown(catalogue()), &catalogue()));
    }

    #[test]
    fn a_renamed_design_with_the_same_id_is_a_change() {
        let mut rows = catalogue();
        rows[1].item.title = "Beta II".to_string();
        assert!(!list_unchanged(&shown(catalogue()), &rows));
    }

    #[test]
    fn a_changed_tag_list_is_a_change() {
        let mut rows = catalogue();
        rows[0].tags.push("new".to_string());
        assert!(!list_unchanged(&shown(catalogue()), &rows));
        let mut retagged = catalogue();
        retagged[1].tags = vec!["other".to_string()];
        assert!(!list_unchanged(&shown(catalogue()), &retagged));
    }

    #[test]
    fn a_flipped_ignored_flag_is_a_change() {
        let mut rows = catalogue();
        rows[0].ignored = true;
        assert!(!list_unchanged(&shown(catalogue()), &rows));
    }

    #[test]
    fn a_flipped_planner_flag_is_a_change() {
        let mut rows = catalogue();
        rows[0].planner_excluded = true;
        assert!(!list_unchanged(&shown(catalogue()), &rows));
        // Un-excluding a shown excluded row is a change as well.
        assert!(!list_unchanged(&shown(rows), &catalogue()));
    }

    #[test]
    fn a_different_order_or_length_is_a_change() {
        let mut reordered = catalogue();
        reordered.reverse();
        assert!(!list_unchanged(&shown(catalogue()), &reordered));
        let mut shorter = catalogue();
        shorter.pop();
        assert!(!list_unchanged(&shown(catalogue()), &shorter));
        assert!(!list_unchanged(&shown(shorter), &catalogue()));
    }
}
