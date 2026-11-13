//! The remote counterpart of the local search path: one `LibraryRequest::Search`
//! against a worker, plus the local<->wire mappings a remote search request needs.

use super::{
    dispatch::{bump_search_seq, is_current_search},
    fetch::{slint_count, to_diagram_item},
    filters::{read_range_filter, read_sort_order, read_tag_filter},
};
use crate::{
    DiagramItem, LibraryModel, MainWindow, bridge::library::source as library_source,
    settings::WorkerSettings,
};
use pleochroma_net::library::{
    LibraryRequest, LibraryResponse, PerformanceAggregateWire, PerformanceBoundWire,
    PerformanceFilterWire, PerformanceMetricWire, RangeFilterWire, SortOrderWire,
};
use pleochroma_vault::{
    db::sqlite::SortOrder,
    model::{
        filter::RangeFilter,
        performance::{
            PerformanceAggregate, PerformanceBound, PerformanceFilter, PerformanceMetric,
        },
    },
};
use slint::{ComponentHandle, ModelRc, VecModel};
use tracing::error;

/// The remote counterpart of `super::dispatch::refresh_diagram_list`.
///
/// It sends one `LibraryRequest::Search` to `worker`, off the UI thread. The result is capped
/// like the local query. `pub(in crate::gui)` because `gui::library::remote` calls it directly
/// after a source switch, when it already has the `WorkerSettings`.
///
/// The sequence is bumped before the request is built, and the reply is checked with
/// [`is_current_search`] before any model write. Otherwise a slow reply could overwrite a
/// newer list, or the list of a library the cutter has since left. Stale remote entry ids
/// there could be acted on by the local-only actions in `local::organize`.
pub(in crate::gui) fn refresh_diagram_list_remote(
    ui: &MainWindow,
    worker: WorkerSettings,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
) {
    let seq = bump_search_seq();
    let clean_shape = if shape_filter == "All Shapes" {
        "All".to_string()
    } else {
        shape_filter.to_string()
    };
    let clean_gear = if gear_filter == "All Gears" {
        "All".to_string()
    } else {
        gear_filter.to_string()
    };
    let range = to_range_wire(&read_range_filter(ui));
    // The worker applies the sort order and the tag chip (see `LibraryRequest::Search`), so
    // a remote search honours them like the local one.
    let request = LibraryRequest::Search {
        query: search.to_string(),
        shape_filter: clean_shape,
        gear_filter: clean_gear,
        range,
        order: to_sort_order_wire(read_sort_order(ui)),
        tag_filter: read_tag_filter(ui),
    };
    library_source::spawn_library_request(ui.as_weak(), worker, request, move |ui, result| {
        if !is_current_search(seq) {
            // A newer search (local or remote) or a source switch already owns
            // the display -- see this function's own doc comment.
            return;
        }
        match result {
            Ok(LibraryResponse::SearchResults {
                items,
                excluded_for_missing_curves,
            }) => {
                let total = items.len();
                let slint_items: Vec<DiagramItem> = items.iter().map(to_diagram_item).collect();
                ui.global::<LibraryModel>()
                    .set_diagram_list(ModelRc::new(VecModel::from(slint_items)));
                ui.global::<LibraryModel>()
                    .set_total_count(slint_count(total));
                // The reply carries only the capped list, not a match count. The capped
                // `total` is shown as a lower bound. An exact count needs a new wire field
                // in `pleochroma-net`.
                ui.global::<LibraryModel>()
                    .set_matched_count(slint_count(total));
                // `DesignSummary::ignored` and the missing-curves count give a remote list the
                // same styling and notice as a local one. The count is capped, so
                // `cast_signed` cannot wrap.
                ui.global::<LibraryModel>()
                    .set_performance_excluded_count(excluded_for_missing_curves.cast_signed());
            }
            Ok(_) => {
                ui.global::<LibraryModel>()
                    .set_status_message("Unexpected reply searching the remote library.".into());
            }
            Err(e) => {
                error!("Remote search failed: {e}");
                ui.global::<LibraryModel>()
                    .set_status_message(format!("Remote search failed: {e}").into());
            }
        }
    });
}

/// Maps a local [`RangeFilter`] onto the wire [`RangeFilterWire`] of a remote search.
///
/// The worker's `serve::library::from_range_wire` reverses it. This is not `const`, since
/// mapping `performance` needs an allocation.
fn to_range_wire(range: &RangeFilter) -> RangeFilterWire {
    RangeFilterWire {
        ri_min: range.ri_min,
        ri_max: range.ri_max,
        lw_min: range.lw_min,
        lw_max: range.lw_max,
        volume_min: range.volume_min,
        volume_max: range.volume_max,
        facets_min: range.facets_min,
        facets_max: range.facets_max,
        ri_tolerance: range.ri_tolerance,
        include_ignored: range.include_ignored,
        performance: range
            .performance
            .iter()
            .map(to_performance_filter_wire)
            .collect(),
    }
}

/// Maps a local [`SortOrder`] onto the wire [`SortOrderWire`] of a remote search.
///
/// The worker maps it back to a `SortOrder`.
const fn to_sort_order_wire(order: SortOrder) -> SortOrderWire {
    match order {
        SortOrder::CatalogueOrder => SortOrderWire::CatalogueOrder,
        SortOrder::Title => SortOrderWire::Title,
        SortOrder::Newest => SortOrderWire::Newest,
        SortOrder::RecentlyEdited => SortOrderWire::RecentlyEdited,
    }
}

const fn to_performance_filter_wire(filter: &PerformanceFilter) -> PerformanceFilterWire {
    let metric = match filter.metric {
        PerformanceMetric::Brilliance => PerformanceMetricWire::Brilliance,
        PerformanceMetric::Extinction => PerformanceMetricWire::Extinction,
        PerformanceMetric::Windowing => PerformanceMetricWire::Windowing,
    };
    let bound = match filter.bound {
        PerformanceBound::AtMost(t) => PerformanceBoundWire::AtMost(t),
        PerformanceBound::AtLeast(t) => PerformanceBoundWire::AtLeast(t),
    };
    let aggregate = match filter.aggregate {
        PerformanceAggregate::Worst => PerformanceAggregateWire::Worst,
        PerformanceAggregate::Mean => PerformanceAggregateWire::Mean,
    };
    PerformanceFilterWire {
        metric,
        bound,
        tilt_radius_deg: filter.tilt_radius_deg,
        aggregate,
    }
}
