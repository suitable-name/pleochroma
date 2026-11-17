//! The SQLite-backed [`Database`]: schema creation/migration, and the two ways to open
//! it -- [`Database::new`] (read-write) and [`Database::open_read_only`].
//!
//! # Concurrency model
//!
//! One process opens the file read-write at a time, `apps/pleochroma-cut`. Its mirror-sync
//! writer thread and UI thread share that single [`Connection`], which is `Send` but not
//! `Sync`, so a `Mutex` or channel around it serializes access. Other processes, most notably
//! `apps/pleochroma-worker`'s `serve` with one short-lived [`Database::open_read_only`]
//! connection per accepted connection, read the same file concurrently.
//!
//! [`Database::new`] enables WAL journaling with `synchronous=NORMAL`, so the writer and the
//! readers do not block each other. `NORMAL` survives an application crash; only an OS crash
//! or power loss before the next checkpoint can lose the last commits. That is an accepted
//! trade for a local design library, which is not a system of record. Both connections set a
//! 5 s `busy_timeout` as a second guard around the writer's `BEGIN IMMEDIATE`/`COMMIT`, which
//! WAL narrows but does not remove.
//!
//! [`Database::open_read_only`] first tries a plain read-only open. A reader of a WAL database
//! needs the `-shm` file, so a reader in a directory it cannot write to fails with
//! `CANTOPEN`/`READONLY`. Only then, and only when no `-wal` file holds un-checkpointed frames,
//! does it retry with the `immutable=1` URI option, which skips the WAL machinery. Such a
//! connection never sees later commits, so the retry is refused whenever a `-wal` file has
//! pending frames: a stale read would otherwise go unnoticed.

use anyhow::{Context, Result};
// `Connection` is re-exported so a caller can build a raw pre-migration fixture: it opens its
// own connection and hand-writes the old `CREATE TABLE`/`INSERT` statements that this module's
// migrations rebuild, without a `rusqlite` dependency of its own. A `#[cfg(test)]` fixture would
// not reach a dependent crate, because `--cfg test` is set only while this crate tests itself.
pub use rusqlite::Connection;
use rusqlite::OpenFlags;
use tracing::{debug, info, warn};

mod base_schema;
mod color_variant_store;
mod cut_progress;
mod design_lighting;
mod entries;
mod frosting;
mod inclusion_effects;
mod materials;
mod migrations;
mod mirror_state;
mod pattern_store;
mod planner_exclusions;
mod preview_axis;
mod preview_material;
mod previews;
mod read_access;
mod render_jobs;
mod saved_rough_plans;
mod search;
mod solid_extents;
mod solid_hull;
mod stone_choice_store;
mod surface_finish;
mod tags;
#[cfg(test)]
mod tests;
mod tilt_curves;
mod variants;
mod zoning_store;

pub use materials::CustomMaterialParams;
pub use search::{DisplayFilters, SEARCH_RESULT_CAP, SortOrder};

// Test-only imports, so the `tests` module can `use super::*;`. Production code reaches these
// names through the submodules, so they are unused in non-test builds and gated out of them.
#[cfg(test)]
use crate::model::{
    detail::FacetingDiagramDetail, entry::FacetingDiagramEntry, filter::RangeFilter,
    metadata_update::MetadataUpdate,
};
#[cfg(test)]
use rusqlite::params;
#[cfg(test)]
use search::percentile_of_sorted;

/// The default database file [`Database::new`] opens when given `None`.
///
/// A path resolved relative to the process's current working directory. `pub` so a
/// caller that wants to report or reuse this default (e.g. `pleochroma-worker`'s
/// `serve --db`, which falls back to this exact path) doesn't have to duplicate the
/// literal.
pub const DEFAULT_DB_FILE: &str = "facet_diagrams.sqlite";

/// The `source_id` every row synced before the `source_id` column existed is
/// backfilled with -- see `Database::migrate_source_id_column`.
///
/// Every design predating the `source_id` column has exactly one possible origin, so
/// backfilling them all to this value is a historical fact about that data, not a
/// guess.
///
/// Deliberately a plain string literal: `db` is the lower layer and must not depend on
/// whatever produces any particular `source_id`. `pub` so that a crate which CAN see
/// both this constant and the identifier it has to match is able to assert the two
/// stay equal -- that assertion cannot live here, because from here only one side of
/// it is visible.
pub const LEGACY_SOURCE_ID: &str = "facetdiagrams.org";

/// The canonical shape vocabulary, defined in [`crate::model::DEFAULT_SHAPES`].
///
/// Re-exported so the `db::sqlite::DEFAULT_SHAPES` path resolves.
pub use crate::model::DEFAULT_SHAPES;

/// Whether `path` names SQLite's special in-memory database, for which
/// `journal_mode=WAL` is meaningless (SQLite always reports `journal_mode` back as
/// `memory` there and refuses to change it). Only the exact literal SQLite itself
/// recognizes -- a URI form like `file::memory:` would need its own `SQLITE_OPEN_URI`
/// handling this crate doesn't otherwise use, and no caller in this workspace passes
/// one (see `Database::new`'s callers).
fn is_memory_db(path: &str) -> bool {
    path == ":memory:"
}

/// Enables WAL journal mode and, only when WAL was actually obtained,
/// `synchronous=NORMAL` on `conn`, opened at `path`. Never fails `Database::new` on its
/// own -- a WAL attempt can fail for reasons outside this database's control (a network
/// filesystem SQLite's WAL doesn't support, a read-only parent directory that can't hold
/// the new `-wal`/`-shm` files), and falling back to SQLite's default rollback-journal
/// mode is a safe, correct degradation, just a slower one under concurrent access. A
/// missing WAL is logged at warn, and `synchronous` is then left at SQLite's default
/// (`FULL`): `NORMAL` is only crash-safe under WAL.
fn enable_wal(conn: &Connection, path: &str) {
    match conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0)) {
        Ok(mode) if mode.eq_ignore_ascii_case("wal") => {
            debug!("{path}: journal_mode=WAL enabled");
        }
        Ok(mode) => {
            warn!(
                "{path}: requested journal_mode=WAL but SQLite reports {mode:?} -- continuing with it \
                 and leaving synchronous at its default (common cause: the database file lives on a \
                 filesystem WAL doesn't support, e.g. a network share)"
            );
            return;
        }
        Err(e) => {
            warn!(
                "{path}: failed to set journal_mode=WAL ({e}); continuing with the existing journal \
                 mode and leaving synchronous at its default"
            );
            return;
        }
    }

    // Safe under WAL specifically (the early returns above guarantee it): a commit is
    // still durable across an application crash (the WAL frame is on disk before
    // `COMMIT` returns), and only an OS crash or power loss between that commit and its
    // later checkpoint could lose it -- an acceptable trade here, not a system of
    // record. Best-effort: leaving `synchronous` at its previous value if this fails
    // costs nothing this database relies on.
    if let Err(e) = conn.pragma_update(None, "synchronous", "NORMAL") {
        debug!(
            "{path}: failed to set synchronous=NORMAL ({e}); continuing with the existing setting"
        );
    }
}

/// Whether `err` is the SQLite result code [`Database::open_read_only`]'s
/// `immutable=1` fallback is actually meant for: `CANTOPEN` or `READONLY` -- see this
/// module's doc comment. Any other failure (a corrupt file, a bad path, a permissions
/// problem on the main file itself rather than its `-shm` sidecar) means the fallback
/// would not help and must not be attempted.
const fn is_cantopen_or_readonly(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::CannotOpen | rusqlite::ffi::ErrorCode::ReadOnly,
                ..
            },
            _,
        )
    )
}

/// Whether `path` has a sibling `-wal` file with un-checkpointed writes in it (a
/// nonexistent or zero-length `-wal` file both count as "none pending"; a missing file
/// can't be read at all so it's treated the same as "no pending writes" rather than an
/// error). See [`Database::open_read_only`]'s doc comment for why this gates its
/// `immutable=1` fallback: that mode never sees a `-wal` file's contents, so opening
/// through it while one has pending frames would silently return a stale database.
fn has_pending_wal_file(path: &str) -> bool {
    std::fs::metadata(format!("{path}-wal")).is_ok_and(|meta| meta.len() > 0)
}

/// Builds a `file:` URI naming `path` with the `immutable=1` query option, for
/// [`Database::open_read_only`]'s fallback -- see that method and this module's doc
/// comment.
///
/// Percent-encodes the characters SQLite's URI filenames treat specially (`?`, `#`,
/// `%`) and normalizes Windows `\` separators to `/`: SQLite's URI parser accepts an
/// absolute path like `file:C:/path/to/db.sqlite` directly, no authority (`//`)
/// component needed.
fn to_sqlite_immutable_uri(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let mut encoded = String::with_capacity(normalized.len());
    for ch in normalized.chars() {
        match ch {
            '%' => encoded.push_str("%25"),
            '?' => encoded.push_str("%3f"),
            '#' => encoded.push_str("%23"),
            other => encoded.push(other),
        }
    }
    format!("file:{encoded}?immutable=1")
}

/// A SQLite-backed connection to the design catalogue.
///
/// See this module's own doc comment for the concurrency model (WAL and `busy_timeout`), and
/// [`Database::new`]/[`Database::open_read_only`] for the two ways to open one.
pub struct Database {
    conn: Connection,
    /// Counts the invalidations of cached solid extents and hulls (see
    /// [`Database::solid_extents_epoch`]); bumped only by `delete_solid_extents`.
    extents_epoch: std::sync::atomic::AtomicU64,
}

impl Drop for Database {
    /// Runs `PRAGMA optimize` on close: SQLite's recommended call when a connection closes.
    /// It is a cheap, sampled statistics refresh (not a full `ANALYZE`), so the next open plans
    /// `search`'s predicates with reasonably fresh statistics.
    ///
    /// Best-effort: a failure here (e.g. this connection was opened read-only, or the
    /// underlying file has since vanished) is logged at debug and never propagated --
    /// `Drop` cannot return a `Result`, and this must never mask whatever real error or
    /// panic the caller may already be unwinding through when this runs.
    fn drop(&mut self) {
        if let Err(e) = self.conn.execute_batch("PRAGMA optimize;") {
            debug!("PRAGMA optimize on close failed (harmless): {e}");
        }
    }
}

impl Database {
    /// Creates a new Database instance, connecting to the SQLite database file.
    /// Creates the file and tables if they don't exist.
    ///
    /// # Arguments
    /// * `db_path` - Optional path to the database file. Defaults to "`facet_diagrams.sqlite`".
    ///
    /// # Errors
    ///
    /// Returns an error if the SQLite file at `db_path` cannot be opened (e.g. bad
    /// path, permissions, or a file that isn't a valid SQLite database), if enabling
    /// the `foreign_keys` pragma or setting the busy timeout fails, or if creating the
    /// schema (tables/indexes) fails.
    pub fn new(db_path: Option<&str>) -> Result<Self> {
        let path = db_path.unwrap_or(DEFAULT_DB_FILE);
        info!("Connecting to database: {}", path);
        let conn = Connection::open(path).context(format!("Failed to open database at {path}"))?;

        // Enforce foreign keys: the `ON DELETE CASCADE` clauses depend on it.
        conn.execute("PRAGMA foreign_keys = ON;", [])
            .context("Failed to enable foreign keys")?;

        // Set before the WAL switch below: `enable_wal` issues statements on this connection
        // that can hit SQLITE_BUSY while another process holds the write lock. Retry a locked
        // statement for up to 5 s instead of failing at once.
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .context("Failed to set busy timeout")?;

        // WAL lets this connection's writes proceed without blocking readers, including
        // readers in other processes, which the rollback journal's every-writer-blocks-
        // every-reader behavior would not allow. `:memory:` has no journal to change, so it
        // is skipped.
        if is_memory_db(path) {
            debug!("journal_mode=WAL skipped: {path} is an in-memory database");
        } else {
            enable_wal(&conn, path);
        }

        let db = Self {
            conn,
            extents_epoch: std::sync::atomic::AtomicU64::new(0),
        };
        db.create_tables_if_not_exist()?;
        db.migrate_numeric_columns()?;
        db.migrate_source_id_column()?;
        db.migrate_proportions_columns()?;
        db.migrate_designer_and_attachment_columns()?;
        db.migrate_drop_unused_designer_index()?;
        db.migrate_crystal_optics_columns()?;
        db.migrate_per_axis_dispersion_column()?;
        db.migrate_custom_material_specific_gravity()?;
        db.migrate_custom_material_color_recipe()?;
        db.migrate_custom_material_dispersion_model()?;
        db.migrate_custom_material_absorption_bands()?;
        db.migrate_custom_material_crystal_axis()?;
        db.migrate_custom_material_inclusions()?;
        db.migrate_custom_material_gem_data()?;
        db.migrate_shape_vocabulary()?;
        db.migrate_ignored_column()?;
        db.migrate_diagram_entries_timestamps()?;
        db.migrate_diagram_entries_provenance()?;
        db.migrate_diagram_previews_table()?;
        db.migrate_diagram_tilt_curves_table()?;
        // After both cache tables exist: it also marks the cached results of designs that
        // carry a design file outdated, in both.
        db.migrate_preview_material_provenance()?;
        db.migrate_diagram_solid_extents_table()?;
        db.migrate_diagram_solid_hull_table()?;
        db.migrate_saved_rough_plans_table()?;
        db.migrate_planner_exclusion_table()?;
        // The frosting of a library design (see `frosting`); it cascades off `diagram_entries`.
        db.migrate_diagram_frosting_table()?;
        // The crystal axis a design's own file sets, for the cached-render fingerprints (see
        // `preview_axis`); it cascades off `diagram_entries` too.
        db.migrate_preview_axis_table()?;
        // The surface finish a design's own file states, for the cached renders and the batch
        // views (see `surface_finish`); it cascades off `diagram_entries` too.
        db.migrate_surface_finish_tables()?;
        // The inclusion and play-of-color choice a design's own file states (see
        // `inclusion_effects`); it cascades off `diagram_entries` too.
        db.migrate_inclusion_effects_table()?;
        db.migrate_design_variants_table()?;
        db.migrate_design_cut_progress_table()?;
        db.migrate_design_lighting_table()?;
        db.migrate_render_jobs_table()?;
        // The rough-colour side tables (see `zoning_store`).
        db.migrate_zoning_tables()?;
        // The Color Pattern side tables (see `pattern_store`).
        db.migrate_pattern_tables()?;
        // The saved color variants of materials (see `color_variant_store`).
        db.migrate_color_variant_tables()?;
        db.migrate_mirror_state_tombstone_column()?;
        db.migrate_prune_tilt_curve_aggregate_columns()?;
        // Before `migrate_blob_columns_last`: that rebuild names diagram_details'
        // columns explicitly (the concave ones included), so they must already
        // exist on an old table when it copies the rows across.
        db.migrate_concave_columns()?;
        // After every diagram_details/diagram_previews/diagram_tilt_curves column-add
        // migration above, so it rebuilds each table's FINAL column set exactly once
        // rather than needing to re-detect a moving target.
        db.migrate_blob_columns_last()?;
        db.migrate_tag_tables()?;
        // After `migrate_tag_tables`: one of its two indexes is on `diagram_tag_links`,
        // which a database old enough to predate that migration does not have yet.
        db.migrate_search_indexes()?;
        search::register_fold_function(&db.conn)
            .context("Failed to register the fold() SQL function")?;
        Ok(db)
    }

    /// Opens `db_path` READ-ONLY at the SQLite connection level (`SQLITE_OPEN_READ_ONLY`,
    /// no `SQLITE_OPEN_CREATE`) -- for a caller that must never write to this database,
    /// ever, not even to create it if missing.
    ///
    /// Unlike [`Self::new`], this does NOT run schema creation or any migration: a
    /// read-only connection cannot perform the `CREATE TABLE`/`ALTER TABLE` statements
    /// those need (and, per this method's own contract, must not even try). It assumes
    /// `db_path` already has the schema [`Self::new`] would have brought it to --
    /// appropriate for pointing this at an existing, already-populated catalogue (e.g.
    /// a long-running server reading the user's own library), never for provisioning a
    /// fresh one.
    ///
    /// # Errors
    ///
    /// Returns an error if `db_path` doesn't exist, isn't a valid SQLite database,
    /// can't be opened read-only (including the `immutable=1` fallback when it's
    /// attempted -- see this method's doc comment above), or if enabling the
    /// `foreign_keys` pragma, setting the busy timeout, or registering the `fold()` SQL
    /// function fails.
    pub fn open_read_only(db_path: &str) -> Result<Self> {
        info!("Connecting to database (read-only): {}", db_path);
        let read_only_flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;

        let conn = match Connection::open_with_flags(db_path, read_only_flags) {
            Ok(conn) => conn,
            Err(primary_err) => {
                // The `immutable=1` fallback is only ever correct for the specific
                // failure this module's doc comment describes (a WAL database's `-shm`
                // file not creatable/writable by this reader) -- any other open
                // failure (corrupt file, wrong path, permissions on the main file
                // itself) means `immutable=1` would fail identically or, worse,
                // "succeed" against something that isn't actually this database.
                if !is_cantopen_or_readonly(&primary_err) {
                    return Err(anyhow::anyhow!(
                        "Failed to open database read-only at {db_path}: {primary_err}"
                    ));
                }
                // `immutable=1` skips the WAL/`-shm` machinery entirely, so it can
                // never see any write still sitting in an un-checkpointed `-wal` file
                // -- reading through it while one exists would silently return a STALE
                // database instead of failing, which is worse than just erroring out
                // here and telling the caller why.
                if has_pending_wal_file(db_path) {
                    return Err(anyhow::anyhow!(
                        "Failed to open database read-only at {db_path}: {primary_err} (the \
                         immutable=1 fallback was not attempted: a `-wal` file with \
                         un-checkpointed writes is present, and immutable=1 would silently read \
                         stale data instead of those writes)"
                    ));
                }
                debug!(
                    "open_read_only: primary open of {db_path} failed ({primary_err}) with \
                     CannotOpen/ReadOnly and no pending -wal file; retrying with the immutable=1 \
                     URI fallback"
                );
                let uri = to_sqlite_immutable_uri(db_path);
                Connection::open_with_flags(&uri, read_only_flags | OpenFlags::SQLITE_OPEN_URI).map_err(
                    |fallback_err| {
                        anyhow::anyhow!(
                            "Failed to open database read-only at {db_path}: {primary_err} (immutable=1 \
                             fallback also failed: {fallback_err})"
                        )
                    },
                )?
            }
        };

        conn.execute("PRAGMA foreign_keys = ON;", [])
            .context("Failed to enable foreign keys")?;

        // Same rationale as `Self::new`: a concurrent mirror-sync writer can hold the
        // lock briefly, so retry internally for up to 5s instead of failing right
        // away. WAL (enabled by the read-write connection in `Self::new`) already lets
        // this reader proceed alongside that writer in the common case; this remains
        // as a second line of defense.
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .context("Failed to set busy timeout")?;

        search::register_fold_function(&conn)
            .context("Failed to register the fold() SQL function")?;

        Ok(Self {
            conn,
            extents_epoch: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Creates every table and index a fresh database needs, in one transaction: the tables
    /// of [`base_schema`] first, then the side tables and indexes shared with `migrations`.
    fn create_tables_if_not_exist(&self) -> Result<()> {
        debug!("Ensuring database tables exist...");
        // `diagram_previews` and `diagram_tilt_curves` come from `migrations`, not a second copy
        // in `base_schema`: a fresh database and an old database's migration must run the same
        // SQL text. The tilt-curve table's 6 generated aggregate columns are not safe to
        // hand-transcribe twice.
        let diagram_tilt_curves_sql = migrations::diagram_tilt_curves_table_sql();
        let shared_sections: [&str; 13] = [
            // Cached preview renders and tilt curves, keyed by entry_id so they survive a
            // diagram_details re-sync; see migrate_diagram_previews_table.
            migrations::DIAGRAM_PREVIEWS_TABLE_SQL,
            &diagram_tilt_curves_sql,
            // Cached finished-solid extents for the Rough Planner; see
            // migrate_diagram_solid_extents_table's own doc comment.
            migrations::DIAGRAM_SOLID_EXTENTS_TABLE_SQL,
            // Cached finished-solid convex hull for the Rough Planner; see
            // migrate_diagram_solid_hull_table's own doc comment.
            migrations::DIAGRAM_SOLID_HULL_TABLE_SQL,
            // Saved rough plans; see migrate_saved_rough_plans_table's own doc comment.
            migrations::SAVED_ROUGH_PLANS_TABLE_SQL,
            // Designs the Rough Planner leaves out of its candidate set; see
            // migrate_planner_exclusion_table's own doc comment.
            migrations::DIAGRAM_PLANNER_EXCLUSIONS_TABLE_SQL,
            // The frosted tiers and facets of a library design; see
            // migrate_diagram_frosting_table's own doc comment.
            migrations::DIAGRAM_FROSTING_TABLE_SQL,
            // Per-design side data keyed by the design's UUID rather than by entry_id:
            // saved variants, cutting progress and the lighting choice; see
            // migrate_design_variants_table's own doc comment.
            migrations::DESIGN_VARIANTS_TABLE_SQL,
            migrations::DESIGN_CUT_PROGRESS_TABLE_SQL,
            migrations::DESIGN_LIGHTING_TABLE_SQL,
            // The desktop render queue; see migrate_render_jobs_table's own doc comment.
            migrations::RENDER_JOBS_TABLE_SQL,
            // Flat tag set plus its many-to-many join table; see migrate_tag_tables's
            // own doc comment for why this is a side table pair, not a column on
            // diagram_entries.
            migrations::TAG_TABLES_SQL,
            // The library search predicate's supporting indexes; see SEARCH_INDEXES_SQL for
            // why each is needed. Last, because one of them is on a table the block above creates.
            migrations::SEARCH_INDEXES_SQL,
        ];
        let mut sql = String::from("BEGIN;\n\n");
        for section in base_schema::BASE_TABLES_SQL
            .into_iter()
            .chain(shared_sections)
        {
            sql.push_str(section);
            sql.push_str("\n\n");
        }
        sql.push_str("COMMIT;");
        self.conn
            .execute_batch(&sql)
            .context("Failed to create database tables")?;
        info!("Database tables checked/created successfully.");
        Ok(())
    }
}
