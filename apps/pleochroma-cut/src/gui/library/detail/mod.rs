//! The design detail panel: builds the detail model for a local or remote design, edits
//! metadata, exports attachments and reconstructs the 3D viewport's planes.
//!
//! [`local_load`] and [`remote_load`] build the model from each source. [`planes`] holds
//! the plane and material reconstruction both share, [`shared`] the formatting and angle
//! helpers, and `load_worker` the bounded thread the local loads run on. [`metadata`] and
//! [`attachments`] hold the edit and export callbacks.

mod attachments;
mod load_worker;
mod local_load;
mod metadata;
mod planes;
mod remote_load;
mod row_cursor;
mod shared;
#[cfg(test)]
mod tests;

pub use attachments::{export_diagram_file, export_diagram_file_via_source};
pub use metadata::setup_save_metadata_callback;
pub use planes::reconstruct_planes;
pub use shared::{clear_current_detail_display, setup_filtered_row_count_callback};

// Makes every local detail load that is running or waiting stale; see
// `local_load::invalidate_detail_loads`.
pub(in crate::gui::library) use local_load::invalidate_detail_loads;

use crate::{
    MainWindow,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::solid_preview::preview_state::SolidPreviewState,
};
use pleochroma_vault::db::sqlite::Database;
use std::sync::{Arc, Mutex};

/// Loads a design's detail from the active library: the local database
/// ([`local_load::load_diagram_detail`]) or the remote server
/// ([`remote_load::load_diagram_detail_remote`]).
///
/// Call sites use this rather than either loader. A remote entry id and a local row id are
/// independent id spaces (see `bridge::library::mirror`'s module doc on identity), so this
/// keeps a remote id from being looked up in the local database.
///
/// `preview_state` is only used by the remote branch. The local branch hands its load to
/// the `load_worker` thread and redraws the Live Render solid once its planes land.
pub fn load_diagram_detail_via_source(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
    preview_state: &Arc<SolidPreviewState>,
) {
    let current = source
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match current {
        LibrarySource::Local => local_load::load_diagram_detail(ui, db_mutex, render_ctx, entry_id),
        LibrarySource::Remote(worker) => {
            // A local load still running for the previous selection must not apply once a
            // remote row is selected (the two libraries' entry ids can be equal).
            invalidate_detail_loads();
            // Bumped here, before the request is even built, so a second selection
            // (of the same or a different row, remote or local) made before this
            // fetch's reply lands is what invalidates it -- see
            // `remote_load::load_diagram_detail_remote`'s own doc comment.
            let seq = crate::gui::library::search::bump_search_seq();
            remote_load::load_diagram_detail_remote(
                ui,
                worker,
                render_ctx,
                entry_id,
                preview_state,
                Arc::clone(source),
                seq,
            );
        }
    }
}
