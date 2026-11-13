//! Walks the remote catalogue's own paginated listing to exhaustion.

use crate::bridge::library::mirror::options::{LibraryTransport, MirrorOutcome};
use pleochroma_net::library::{LibraryRequest, LibraryResponse, RangeFilterWire};

/// Walks [`LibraryRequest::SearchPage`] to exhaustion, concatenating every page's
/// [`DesignSummary`](pleochroma_net::library::DesignSummary) list into one `Vec` -- the
/// WHOLE remote catalogue matching the (currently always empty/unfiltered) query, not
/// just its first page. It uses a keyset cursor: `cursor: None` first, then each reply's
/// `next_cursor`, until it is `None`. Rows added on the remote during the walk are never skipped
/// or repeated.
///
/// Returns `Err(MirrorOutcome::Failed(..))` on the first request that fails or replies with
/// anything other than [`LibraryResponse::SearchResultsPage`]. Nothing is written locally here,
/// so the database is left unchanged.
pub(super) fn enumerate_remote_catalogue(
    transport: &impl LibraryTransport,
) -> Result<Vec<pleochroma_net::library::DesignSummary>, MirrorOutcome> {
    let mut summaries = Vec::new();
    let mut cursor = None;
    loop {
        let request = LibraryRequest::SearchPage {
            query: String::new(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire::default(),
            cursor,
        };
        match transport.request(&request) {
            Ok(LibraryResponse::SearchResultsPage {
                results,
                next_cursor,
                // The unfiltered walk sets no `range.performance`, so the worker sends `0` here.
                excluded_for_missing_curves: _,
            }) => {
                summaries.extend(results);
                match next_cursor {
                    Some(c) => cursor = Some(c),
                    None => return Ok(summaries),
                }
            }
            Ok(other) => {
                return Err(MirrorOutcome::Failed(format!(
                    "remote worker replied to SearchPage with an unexpected message: {other:?}"
                )));
            }
            Err(e) => {
                return Err(MirrorOutcome::Failed(format!(
                    "could not list the remote library: {e}"
                )));
            }
        }
    }
}
