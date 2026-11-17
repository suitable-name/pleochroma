//! The design library as one viewer connection sees it.
//!
//! A read-only [`Database`] that is opened by the connection's first library request, never for
//! render, tilt, final-image, ping or asset traffic.
//!
//! A connection used only to render therefore never pays for opening the library, and never
//! fails because it cannot be opened.

use super::{handle_request, handlers};
use crate::serve::socket::open_library_database;
use pleochroma_net::library::{LibraryRequest, LibraryResponse};
use pleochroma_vault::db::sqlite::Database;
use std::{cell::OnceCell, net::SocketAddr, path::PathBuf};

/// One connection's read-only view of the design library, opened on first use.
///
/// Owned by the connection's own thread: `rusqlite::Connection` is `Send` but not
/// `Sync`, and SQLite's concurrency model is many independent reader connections, so
/// every connection opens its own handle rather than sharing one.
pub struct LibraryHandle {
    /// Where the database opens from; empty for a handle built by [`Self::open`], whose
    /// cell is already filled.
    path: PathBuf,
    /// The connection's peer, named in the log line of a failed open.
    peer: Option<SocketAddr>,
    /// Filled by the first successful open. A failed open leaves it empty, so the next
    /// library request tries again.
    db: OnceCell<Database>,
    /// A coordinator running without a design library (see [`Self::absent`]): nothing is
    /// ever opened and every library request gets the "no library" error.
    absent: bool,
}

impl LibraryHandle {
    /// A handle that opens the read-only database at `path` on its first use. `peer`
    /// only labels the log line of an open that fails.
    #[must_use]
    pub const fn lazy(path: PathBuf, peer: Option<SocketAddr>) -> Self {
        Self {
            path,
            peer,
            db: OnceCell::new(),
            absent: false,
        }
    }

    /// A handle for a coordinator that has no design library.
    ///
    /// Nothing is opened, and every library request is answered with a plain "no library"
    /// [`LibraryResponse::Error`] (code [`super::LIBRARY_ERROR_CODE`]). The connection stays usable
    /// for everything else, rendering included.
    #[must_use]
    pub const fn absent(peer: Option<SocketAddr>) -> Self {
        Self {
            path: PathBuf::new(),
            peer,
            db: OnceCell::new(),
            absent: true,
        }
    }

    /// A handle around a database the caller already opened.
    #[must_use]
    pub fn open(db: Database) -> Self {
        Self {
            path: PathBuf::new(),
            peer: None,
            db: OnceCell::from(db),
            absent: false,
        }
    }

    /// Whether the database has been opened yet.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.db.get().is_some()
    }

    /// The database, opened read-only first if no earlier call has.
    ///
    /// # Errors
    ///
    /// A human-readable message if the database cannot be opened read-only (missing
    /// file, permissions, not a valid SQLite database). Nothing is remembered then, so
    /// a later call opens it again.
    pub fn database(&self) -> Result<&Database, String> {
        if let Some(db) = self.db.get() {
            return Ok(db);
        }
        if self.absent {
            return Err("this coordinator runs without a design library".to_owned());
        }
        let opened = open_library_database(&self.path)?;
        Ok(self.db.get_or_init(|| opened))
    }

    /// Answers one library request, opening the database first if it is not open yet.
    ///
    /// A database that cannot be opened is logged in full and answered with the same
    /// generic [`LibraryResponse::Error`] a failed query gets; the connection stays
    /// usable for every other request. A handle without a library ([`Self::absent`])
    /// answers the plain "no library" error, without a log line per request.
    #[must_use]
    pub fn handle(&self, request: &LibraryRequest) -> LibraryResponse {
        if self.absent {
            return handlers::no_library();
        }
        match self.database() {
            Ok(db) => handle_request(request, db),
            Err(message) => {
                tracing::warn!("connection {:?}: {message}", self.peer);
                handlers::server_error()
            }
        }
    }
}
