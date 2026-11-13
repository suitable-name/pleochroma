//! Reads the library panel's search and filter state off `ui`.
//!
//! It produces the plain values that the fetch and search paths consume.

use crate::{LibraryModel, MainWindow};
use anyhow::Result;
use pleochroma_vault::{
    db::sqlite::{Database, DisplayFilters, SortOrder},
    model::{
        filter::RangeFilter,
        performance::{
            PerformanceAggregate, PerformanceBound, PerformanceFilter, PerformanceMetric,
        },
    },
};
use slint::{ComponentHandle, Model};
use std::sync::{Arc, Mutex};

/// Rounds a facet-count slider bound to whole facets.
#[expect(
    clippy::cast_possible_truncation,
    reason = "slider bounds are facet counts (tens to hundreds), far inside i64; `as` saturates anyway"
)]
const fn round_to_i64(value: f64) -> i64 {
    value.round() as i64
}

/// The active bound for a slider value, or `None` when it sits on its data-bounds edge.
///
/// `None` means "not filtering on this side", so `search_diagrams` skips the predicate. Rows
/// with no value for that attribute stay visible, as with the "All" dropdown entries.
/// Any other value is an active bound.
fn active_bound(value: f32, bound_edge: f32) -> Option<f64> {
    let moved_off_edge = (value - bound_edge).abs() >= 1e-6;
    moved_off_edge.then_some(f64::from(value))
}

/// Builds the `RangeFilter` to hand to `search_diagrams`.
///
/// Reads the four range-filter sliders' current min/max values, the RI centre+
/// tolerance filter, the "show ignored" toggle, and every active tilt-performance
/// filter row off `ui`. See `active_bound` for what makes a min/max bound "active"
/// versus effectively unset.
#[must_use]
pub fn read_range_filter(ui: &MainWindow) -> RangeFilter {
    RangeFilter {
        ri_min: active_bound(
            ui.global::<LibraryModel>().get_ri_filter_min(),
            ui.global::<LibraryModel>().get_ri_bounds_min(),
        ),
        ri_max: active_bound(
            ui.global::<LibraryModel>().get_ri_filter_max(),
            ui.global::<LibraryModel>().get_ri_bounds_max(),
        ),
        lw_min: active_bound(
            ui.global::<LibraryModel>().get_lw_filter_min(),
            ui.global::<LibraryModel>().get_lw_bounds_min(),
        ),
        lw_max: active_bound(
            ui.global::<LibraryModel>().get_lw_filter_max(),
            ui.global::<LibraryModel>().get_lw_bounds_max(),
        ),
        volume_min: active_bound(
            ui.global::<LibraryModel>().get_volume_filter_min(),
            ui.global::<LibraryModel>().get_volume_bounds_min(),
        ),
        volume_max: active_bound(
            ui.global::<LibraryModel>().get_volume_filter_max(),
            ui.global::<LibraryModel>().get_volume_bounds_max(),
        ),
        facets_min: active_bound(
            ui.global::<LibraryModel>().get_facets_filter_min(),
            ui.global::<LibraryModel>().get_facets_bounds_min(),
        )
        .map(round_to_i64),
        facets_max: active_bound(
            ui.global::<LibraryModel>().get_facets_filter_max(),
            ui.global::<LibraryModel>().get_facets_bounds_max(),
        )
        .map(round_to_i64),
        // A separate RI band, `[center - tolerance, center + tolerance]`, composing
        // with `ri_min`/`ri_max` above by intersection -- see
        // `RangeFilter::ri_tolerance`'s doc comment. `None` whenever the panel's
        // toggle is off, same convention as `active_bound`.
        ri_tolerance: ui
            .global::<LibraryModel>()
            .get_ri_tolerance_enabled()
            .then(|| {
                (
                    f64::from(ui.global::<LibraryModel>().get_ri_tolerance_center()),
                    f64::from(ui.global::<LibraryModel>().get_ri_tolerance_value()),
                )
            }),
        performance: read_performance_filters(ui),
        include_ignored: ui.global::<LibraryModel>().get_show_ignored(),
        has_concave: None,
    }
}

/// Maps `LibraryModel.sort_order_index` (the header toolbar's sort selector) onto a
/// [`SortOrder`]. An out-of-range index falls back to [`SortOrder::CatalogueOrder`].
#[must_use]
pub fn read_sort_order(ui: &MainWindow) -> SortOrder {
    match ui.global::<LibraryModel>().get_sort_order_index() {
        1 => SortOrder::Title,
        2 => SortOrder::Newest,
        3 => SortOrder::RecentlyEdited,
        _ => SortOrder::CatalogueOrder,
    }
}

/// Reads the library panel's "My designs" toggle (`filter_panel.slint`) -- `true`
/// restricts the catalogue display to the cutter's own locally-imported designs.
#[must_use]
pub fn read_local_only(ui: &MainWindow) -> bool {
    ui.global::<LibraryModel>().get_local_only_filter()
}

/// Reads the library panel's tag chip filter (`filter_panel.slint`).
/// `LibraryModel.active_tag_filter_name` is `""` when no chip is selected.
///
/// Returns the tag's name, not its id. Resolving the name is a database read, so it is left
/// to [`super::fetch::fetch_diagram_list_with_options`], where the database is open.
#[must_use]
pub fn read_tag_filter(ui: &MainWindow) -> Option<String> {
    let name = ui.global::<LibraryModel>().get_active_tag_filter_name();
    (!name.is_empty()).then(|| name.to_string())
}

/// Reads the "show these N" restriction a batch import leaves behind.
///
/// It is `LibraryModel.recent_import_filter`, and an empty list means no restriction.
///
/// `gui::library::diagram_list::setup_search_and_filter_callbacks` clears it on the first
/// real search or filter change, so it does not outlive the view it was made for.
#[must_use]
pub fn read_id_filter(ui: &MainWindow) -> Option<Vec<i64>> {
    let model = ui.global::<LibraryModel>().get_recent_import_filter();
    if model.row_count() == 0 {
        return None;
    }
    Some(model.iter().map(i64::from).collect())
}

/// Builds the `Vec<PerformanceFilter>` for `read_range_filter`.
///
/// It reads the tilt-performance rows (`LibraryModel.performance_filters`, a
/// `PerformanceFilterRow`) and applies the index-to-enum mapping.
///
/// Each row goes through `PerformanceFilter::new`, the one boundary where a raw tilt radius
/// becomes a `PerformanceFilter`. A rejected row is dropped, never clamped into range, and
/// reported through the status message.
fn read_performance_filters(ui: &MainWindow) -> Vec<PerformanceFilter> {
    let rows = ui.global::<LibraryModel>().get_performance_filters();
    let mut filters = Vec::with_capacity(rows.row_count());
    let mut rejected = 0usize;
    for row in rows.iter() {
        let metric = match row.metric_index {
            0 => PerformanceMetric::Brilliance,
            1 => PerformanceMetric::Extinction,
            _ => PerformanceMetric::Windowing,
        };
        let bound = if row.bound_index == 0 {
            PerformanceBound::AtMost(row.threshold_pct)
        } else {
            PerformanceBound::AtLeast(row.threshold_pct)
        };
        let aggregate = if row.aggregate_index == 0 {
            PerformanceAggregate::Worst
        } else {
            PerformanceAggregate::Mean
        };
        match PerformanceFilter::new(metric, bound, row.tilt_radius_deg, aggregate) {
            Ok(filter) => filters.push(filter),
            Err(_) => rejected += 1,
        }
    }
    if rejected > 0 {
        ui.global::<LibraryModel>().set_status_message(
            format!(
                "{rejected} tilt-performance filter(s) had an invalid tilt radius and were \
                 ignored."
            )
            .into(),
        );
    }
    filters
}

/// The library panel's search and filter state as plain owned values.
///
/// It holds what [`resolve_filtered_entry_ids`] needs, captured on the UI thread so the query
/// can run on a worker.
///
/// Compared with `==` at apply time to tell whether the panel changed while the worker
/// was resolving (a changed snapshot means the resolved ids no longer describe what the
/// panel shows).
#[derive(Debug, Clone, PartialEq)]
pub struct FilterSnapshot {
    /// The search box text, as typed.
    pub search: String,
    /// The shape dropdown's text with the "All Shapes" label mapped to `"All"`.
    pub clean_shape: String,
    /// The gear dropdown's text with the "All Gears" label mapped to `"All"`.
    pub clean_gear: String,
    /// Every range, RI-band and tilt-performance filter.
    pub range: RangeFilter,
    /// The "My designs" toggle.
    pub local_only: bool,
    /// The active tag chip's NAME (resolved to an id by [`resolve_filtered_entry_ids`]).
    pub tag_filter_name: Option<String>,
    /// The "show these N" restriction a batch import leaves behind.
    pub id_filter: Option<Vec<i64>>,
}

impl FilterSnapshot {
    /// Reads the panel's current filter state off `ui`.
    #[must_use]
    pub fn capture(ui: &MainWindow) -> Self {
        let search = ui.global::<LibraryModel>().get_search_text();
        let shape_idx = usize::try_from(ui.global::<LibraryModel>().get_selected_shape_index())
            .unwrap_or(usize::MAX);
        let shape = ui
            .global::<LibraryModel>()
            .get_shape_options()
            .row_data(shape_idx)
            .unwrap_or_default();
        let gear_idx = usize::try_from(ui.global::<LibraryModel>().get_selected_gear_index())
            .unwrap_or(usize::MAX);
        let gear = ui
            .global::<LibraryModel>()
            .get_gear_options()
            .row_data(gear_idx)
            .unwrap_or_default();
        let clean_shape = if shape == "All Shapes" {
            "All".to_string()
        } else {
            shape.to_string()
        };
        let clean_gear = if gear == "All Gears" {
            "All".to_string()
        } else {
            gear.to_string()
        };
        Self {
            search: search.to_string(),
            clean_shape,
            clean_gear,
            range: read_range_filter(ui),
            local_only: read_local_only(ui),
            tag_filter_name: read_tag_filter(ui),
            id_filter: read_id_filter(ui),
        }
    }
}

/// Every entry id matching `snapshot` in `db`: the real, uncapped "filtered set".
///
/// Used by the batch actions that act on the filtered set. It covers the search text, the
/// shape and gear dropdowns, and every range, tag and local-only filter, via
/// [`Database::matching_entry_ids`].
///
/// Pure over `(db, snapshot)`, so it runs on any thread with any connection to the catalogue.
///
/// # Errors
///
/// Returns whatever [`Database::matching_entry_ids`] returns.
pub fn resolve_filtered_entry_ids(db: &Database, snapshot: &FilterSnapshot) -> Result<Vec<i64>> {
    let tag_filter = snapshot
        .tag_filter_name
        .as_deref()
        .and_then(|name| db.tag_id_by_name(name).ok().flatten());
    let filters = DisplayFilters {
        // `matching_entry_ids` always walks in `SortOrder::CatalogueOrder` internally
        // (see that method's own doc comment) -- this field is otherwise unused by it,
        // kept at the default purely because `DisplayFilters` has no other convenient
        // "don't care" spelling.
        order: SortOrder::CatalogueOrder,
        local_only: snapshot.local_only,
        tag_filter,
        id_filter: snapshot.id_filter.as_deref(),
    };
    db.matching_entry_ids(
        &snapshot.search,
        &snapshot.clean_shape,
        &snapshot.clean_gear,
        &snapshot.range,
        filters,
    )
}

/// Every entry id matching the library panel's current search/filter state.
///
/// Captures a [`FilterSnapshot`] and runs the uncapped query on the calling thread.
///
/// It holds the shared lock for the query. UI-thread callers should prefer the worker-based
/// resolver in `gui::library::diagram_list`. This is local catalogue only: a remote library
/// session has no local `Database`, so a caller under a remote source must refuse the action.
///
/// # Errors
///
/// Returns whatever [`Database::matching_entry_ids`] returns.
pub fn current_filtered_entry_ids(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
) -> Result<Vec<i64>> {
    let snapshot = FilterSnapshot::capture(ui);
    let db = db_mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    resolve_filtered_entry_ids(&db, &snapshot)
}

#[cfg(test)]
mod tests {
    use super::{FilterSnapshot, resolve_filtered_entry_ids};
    use pleochroma_vault::{
        db::sqlite::{Database, DisplayFilters, SortOrder},
        model::{entry::FacetingDiagramEntry, filter::RangeFilter},
    };

    fn snapshot(search: &str) -> FilterSnapshot {
        FilterSnapshot {
            search: search.to_string(),
            clean_shape: "All".to_string(),
            clean_gear: "All".to_string(),
            range: RangeFilter::default(),
            local_only: false,
            tag_filter_name: None,
            id_filter: None,
        }
    }

    fn seeded_db() -> Database {
        let db = Database::new(Some(":memory:")).expect("an in-memory database opens");
        for (title, url) in [
            ("Round Alpha", "local://round-alpha.asc"),
            ("Round Beta", "local://round-beta.asc"),
            ("Oval Gamma", "local://oval-gamma.asc"),
        ] {
            db.save_diagram_entry(
                &FacetingDiagramEntry {
                    title: title.to_string(),
                    url: url.to_string(),
                    design_id: String::new(),
                },
                "local-import",
            )
            .expect("the entry saves");
        }
        db
    }

    #[test]
    fn resolve_matches_the_direct_database_query_in_the_same_order() {
        let db = seeded_db();
        let snap = snapshot("round");
        let resolved = resolve_filtered_entry_ids(&db, &snap).expect("the query runs");
        let direct = db
            .matching_entry_ids(
                "round",
                "All",
                "All",
                &RangeFilter::default(),
                DisplayFilters {
                    order: SortOrder::CatalogueOrder,
                    local_only: false,
                    tag_filter: None,
                    id_filter: None,
                },
            )
            .expect("the direct query runs");
        assert_eq!(resolved.len(), 2, "two of the three designs match 'round'");
        assert_eq!(resolved, direct);
    }

    #[test]
    fn snapshots_of_equal_values_compare_equal_and_a_changed_search_does_not() {
        assert_eq!(snapshot("round"), snapshot("round"));
        assert_ne!(snapshot("round"), snapshot("oval"));
    }
}
