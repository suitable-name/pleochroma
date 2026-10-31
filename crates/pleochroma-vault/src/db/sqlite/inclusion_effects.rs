//! Storage for `diagram_inclusion_effects`: the inclusion and play-of-color choice a library
//! design's own file states, as the text the cached-render fingerprints hash.
//!
//! The staleness scans never open a design file, so what the cached renders were made with is read
//! from here, as the surface finish is read from `diagram_surface_finish`. Written with the rest
//! of a design's row state when the design is imported, saved or mirrored
//! (`crate::local::apply_imported_extras`).
//!
//! A side table cascading off `diagram_entries`: writing it never touches the entry row or its
//! `updated_at`. A design without a row states neither choice, which is also what every design
//! that predates the table reads as.

use super::Database;
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use std::collections::BTreeMap;

/// `diagram_inclusion_effects`' `CREATE TABLE IF NOT EXISTS` text. No BLOB column, so no
/// `__reordered` variant.
const DIAGRAM_INCLUSION_EFFECTS_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS diagram_inclusion_effects (
        entry_id INTEGER PRIMARY KEY,
        effects_text TEXT NOT NULL,
        FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
    );
";

impl Database {
    /// Creates `diagram_inclusion_effects` for a database created before the choices were
    /// tracked. Adds nothing to any existing table; idempotent via `CREATE TABLE IF NOT EXISTS`.
    ///
    /// When the table is created by this call it is filled once from the design files the library
    /// already stores ([`Self::backfill_inclusion_effects`]), so a design saved with silk before
    /// the table existed does not keep a thumbnail rendered without it.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table fails. A design file that cannot be read is skipped.
    pub(in crate::db::sqlite) fn migrate_inclusion_effects_table(&self) -> Result<()> {
        let existed = self.inclusion_effects_table_exists()?;
        self.conn
            .execute_batch(DIAGRAM_INCLUSION_EFFECTS_TABLE_SQL)
            .context("Failed to create diagram_inclusion_effects table")?;
        if !existed {
            self.backfill_inclusion_effects()?;
        }
        Ok(())
    }

    /// Records the choices of every entry whose stored design file (an attached `.psdd`;
    /// the first one, in attachment order) states any.
    fn backfill_inclusion_effects(&self) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT dd.entry_id, af.name, af.content
                 FROM attached_files af
                 JOIN diagram_details dd ON dd.id = af.detail_id
                 WHERE lower(af.name) LIKE '%.psdd'
                 ORDER BY dd.entry_id, af.id",
            )
            .context("Failed to prepare the inclusion-effects backfill query")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .context("Failed to run the inclusion-effects backfill query")?;
        let mut done = std::collections::BTreeSet::new();
        for row in rows {
            let (entry_id, name, content) =
                row.context("Failed to read a design file for the inclusion-effects backfill")?;
            if !crate::local::is_native_design_name(&name) || !done.insert(entry_id) {
                continue;
            }
            if let Some(text) = crate::local::inclusion_effects_of_bytes(&content) {
                self.set_diagram_inclusion_effects(entry_id, Some(&text))?;
            }
        }
        Ok(())
    }

    /// Records the inclusion and play-of-color choice design `entry_id`'s own file states;
    /// `None` removes it. Replaces whatever the row held.
    ///
    /// # Errors
    ///
    /// Returns an error if the write fails, including a `Some` for an `entry_id` that matches no
    /// `diagram_entries` row (the table's `FOREIGN KEY`).
    pub fn set_diagram_inclusion_effects(&self, entry_id: i64, text: Option<&str>) -> Result<()> {
        match text {
            None => {
                self.conn
                    .execute(
                        "DELETE FROM diagram_inclusion_effects WHERE entry_id = ?1",
                        params![entry_id],
                    )
                    .with_context(|| {
                        format!("Failed to clear the inclusion effects of diagram entry {entry_id}")
                    })?;
            }
            Some(text) => {
                self.conn
                    .execute(
                        "INSERT INTO diagram_inclusion_effects (entry_id, effects_text)
                         VALUES (?1, ?2)
                         ON CONFLICT(entry_id) DO UPDATE SET effects_text = excluded.effects_text",
                        params![entry_id, text],
                    )
                    .with_context(|| {
                        format!("Failed to store the inclusion effects of diagram entry {entry_id}")
                    })?;
            }
        }
        Ok(())
    }

    /// The choice text design `entry_id`'s own file states; `None` for a design that states none
    /// (and for a database that lacks the table, which a read-only open of an older file does).
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying query fails.
    pub fn diagram_inclusion_effects(&self, entry_id: i64) -> Result<Option<String>> {
        if !self.inclusion_effects_table_exists()? {
            return Ok(None);
        }
        self.conn
            .query_row(
                "SELECT effects_text FROM diagram_inclusion_effects WHERE entry_id = ?1",
                params![entry_id],
                |row| row.get(0),
            )
            .optional()
            .with_context(|| format!("Failed to read the inclusion effects of entry {entry_id}"))
    }

    /// Every design's choice text, keyed by entry id: one query instead of one
    /// [`Self::diagram_inclusion_effects`] per design. Designs without one are absent; the map is
    /// empty when the table does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying query fails.
    pub fn all_diagram_inclusion_effects(&self) -> Result<BTreeMap<i64, String>> {
        if !self.inclusion_effects_table_exists()? {
            return Ok(BTreeMap::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT entry_id, effects_text FROM diagram_inclusion_effects ORDER BY entry_id",
            )
            .context("Failed to prepare the all-inclusion-effects query")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .context("Failed to run the all-inclusion-effects query")?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()
            .context("Failed to read the stored inclusion effects")?;
        Ok(rows)
    }

    /// Whether `diagram_inclusion_effects` exists in this database.
    fn inclusion_effects_table_exists(&self) -> Result<bool> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'diagram_inclusion_effects'",
                [],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to look for the diagram_inclusion_effects table")?;
        Ok(found.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::entry::FacetingDiagramEntry;

    fn entry(db: &Database, name: &str) -> i64 {
        db.save_diagram_entry(
            &FacetingDiagramEntry {
                title: name.to_string(),
                url: format!("local://{name}.asc"),
                design_id: String::new(),
            },
            "local-import",
        )
        .expect("the entry saves")
    }

    #[test]
    fn the_text_round_trips_is_replaced_and_is_cleared() {
        let db = Database::new(Some(":memory:")).expect("in-memory database opens");
        let a = entry(&db, "a");
        let b = entry(&db, "b");
        assert_eq!(db.diagram_inclusion_effects(a).unwrap(), None);
        assert!(db.all_diagram_inclusion_effects().unwrap().is_empty());

        db.set_diagram_inclusion_effects(a, Some("silk")).unwrap();
        db.set_diagram_inclusion_effects(b, Some("opal")).unwrap();
        db.set_diagram_inclusion_effects(a, Some("fine silk"))
            .unwrap();
        let all = db.all_diagram_inclusion_effects().unwrap();
        assert_eq!(all.get(&a).map(String::as_str), Some("fine silk"));
        assert_eq!(all.get(&b).map(String::as_str), Some("opal"));

        db.set_diagram_inclusion_effects(a, None).unwrap();
        assert_eq!(db.diagram_inclusion_effects(a).unwrap(), None);
        assert_eq!(db.all_diagram_inclusion_effects().unwrap().len(), 1);
    }

    #[test]
    fn deleting_the_entry_removes_its_text() {
        let db = Database::new(Some(":memory:")).expect("in-memory database opens");
        let id = entry(&db, "gone");
        db.set_diagram_inclusion_effects(id, Some("silk")).unwrap();
        db.conn
            .execute("DELETE FROM diagram_entries WHERE id = ?1", params![id])
            .unwrap();
        assert!(db.all_diagram_inclusion_effects().unwrap().is_empty());
    }
}
