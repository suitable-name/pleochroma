//! The one place the library UI opens its own read-only database connection.
//!
//! Every library read off the shared `Mutex<Database>` goes through [`with_read_db`]. The
//! shared handle is held by writers and long workers, such as a
//! preview batch or a whole-catalogue refresh, so a read queued behind them would freeze
//! the UI thread for that whole hold. WAL lets the read-only connection run alongside them.

use pleochroma_vault::db::sqlite::Database;
use std::sync::{Arc, Mutex, PoisonError};

/// Runs `read` against the catalogue through a read-only connection.
///
/// That connection is to [`crate::gui::db_path`], or the shared handle when no second
/// connection is possible (the `:memory:` fallback library; see
/// [`Database::with_read_connection`]). Unit tests always take the shared handle: their
/// databases are temp files, and `db_path` is the user's real catalogue.
pub(in crate::gui::library) fn with_read_db<T>(
    db: &Arc<Mutex<Database>>,
    read: impl FnOnce(&Database) -> T,
) -> T {
    if cfg!(test) {
        let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
        return read(&guard);
    }
    Database::with_read_connection(&crate::gui::db_path(), db, read)
}
