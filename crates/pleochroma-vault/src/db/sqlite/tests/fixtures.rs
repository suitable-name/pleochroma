//! Fixtures shared by more than one topic module here: a fresh temp-database path, and
//! the pre-migration schema seeder several migration tests (in both [`super::migrations`]
//! and [`super::schema_migrations`]) build their "old shape" databases from.

use super::super::{Connection, params};
use std::sync::atomic::{AtomicU32, Ordering};

/// Returns a fresh temp-database path. Tests need a real file (not `:memory:`) to reopen
/// across two `Database::new` calls.
///
/// The counter and process id keep names unique within a run. A file left by an earlier
/// run can reuse a name, so any existing file is removed first.
pub(super) fn temp_db_path(label: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "pleochroma_vault_test_{label}_{n}_{}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

/// One `(title, url, ri, lw, volume, gear, facets_count)` row for
/// [`seed_pre_migration_db`].
pub(super) type SeedRow<'a> = (
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
);

/// Creates the *pre-migration* schema directly (bypassing `Database::new`, which always
/// migrates on open) and inserts one `diagram_entries`/`diagram_details` pair per
/// `SeedRow`, in the old TEXT-column shape. Migration tests use it to match the data of
/// a real `facet_diagrams.sqlite` catalogue.
pub(super) fn seed_pre_migration_db(path: &std::path::Path, rows: &[SeedRow<'_>]) {
    let conn = Connection::open(path).expect("open raw connection for seeding");
    conn.execute_batch(
        "CREATE TABLE diagram_entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                url TEXT NOT NULL UNIQUE,
                design_id TEXT
            );
            CREATE TABLE diagram_details (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entry_id INTEGER NOT NULL UNIQUE,
                page_url TEXT NOT NULL,
                diagram_image_name TEXT,
                diagram_image_data BLOB,
                competition_diagram TEXT,
                lw_ratio TEXT,
                refractive_index TEXT,
                index_gear TEXT,
                volume TEXT,
                facets_count TEXT,
                shape TEXT,
                designer_info TEXT,
                FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
            );
            CREATE TABLE angle_settings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                order_idx INTEGER NOT NULL,
                detail_id INTEGER NOT NULL,
                facet TEXT NOT NULL,
                angle TEXT NOT NULL,
                index_val TEXT NOT NULL,
                notes TEXT NOT NULL,
                FOREIGN KEY (detail_id) REFERENCES diagram_details (id) ON DELETE CASCADE
            );
            CREATE TABLE attached_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                detail_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                url TEXT NOT NULL,
                content BLOB NOT NULL,
                FOREIGN KEY (detail_id) REFERENCES diagram_details (id) ON DELETE CASCADE
            );
            CREATE TABLE custom_gem_materials (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                refractive_index REAL NOT NULL,
                dispersion REAL NOT NULL,
                birefringence REAL NOT NULL,
                absorption_r REAL NOT NULL,
                absorption_g REAL NOT NULL,
                absorption_b REAL NOT NULL
            );",
    )
    .expect("create pre-migration schema");

    for (title, url, ri, lw, volume, gear, facets_count) in rows {
        conn.execute(
            "INSERT INTO diagram_entries (title, url, design_id) VALUES (?1, ?2, NULL)",
            params![title, url],
        )
        .expect("insert entry");
        let entry_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO diagram_details (
                    entry_id, page_url, competition_diagram, lw_ratio, refractive_index,
                    index_gear, volume, facets_count, shape, designer_info
                 ) VALUES (?1, '', NULL, ?2, ?3, ?4, ?5, ?6, NULL, NULL)",
            params![entry_id, lw, ri, gear, volume, facets_count],
        )
        .expect("insert detail");
    }
}
