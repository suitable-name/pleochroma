//! Storage for `diagram_preview_axes`: the crystal axis a library design's own file sets.
//!
//! The staleness scans (`Database::entry_ids_missing_previews` /
//! `entry_ids_missing_tilt_curves`) never open a design file, so the axis the cached renders
//! were made with is read from here, as the frosting is read from `diagram_frosting`. Written
//! with the rest of a design's row state when the design is imported or saved
//! (`crate::local::apply_imported_extras`).
//!
//! A side table cascading off `diagram_entries`, like `diagram_frosting`: writing it never
//! touches the entry row or its `updated_at`. A design without a row has no axis of its own,
//! which is also what every design that predates the table reads as.

use super::Database;
use crate::model::preview::PreviewAxis;
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use std::collections::BTreeMap;

/// `diagram_preview_axes`' `CREATE TABLE IF NOT EXISTS` text. No BLOB column, so no
/// `__reordered` variant.
const DIAGRAM_PREVIEW_AXES_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS diagram_preview_axes (
        entry_id INTEGER PRIMARY KEY,
        tilt_deg REAL NOT NULL,
        azimuth_deg REAL NOT NULL,
        second_deg REAL,
        FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
    );
";

impl Database {
    /// Creates `diagram_preview_axes` for a database created before a design's own crystal
    /// axis was tracked. Adds nothing to any existing table; naturally idempotent via
    /// `CREATE TABLE IF NOT EXISTS`.
    ///
    /// When the table is created by this call, it is filled once from the design files the
    /// library already stores ([`Self::backfill_preview_axes`]): a design saved before the
    /// table existed with its crystal axis on would otherwise keep a thumbnail rendered
    /// without the axis until it was saved again.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table fails. A design file that cannot be read is
    /// skipped (it has no axis to record), not an error.
    pub(in crate::db::sqlite) fn migrate_preview_axis_table(&self) -> Result<()> {
        let existed = self.preview_axis_table_exists()?;
        self.conn
            .execute_batch(DIAGRAM_PREVIEW_AXES_TABLE_SQL)
            .context("Failed to create diagram_preview_axes table")?;
        if !existed {
            self.backfill_preview_axes()?;
        }
        Ok(())
    }

    /// Records the crystal axis of every entry whose stored design file (an attached `.psdd`;
    /// the first one, in attachment order) turns the override on. Entries
    /// without a design file, or whose file sets no axis, get no row.
    fn backfill_preview_axes(&self) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT dd.entry_id, af.name, af.content
                 FROM attached_files af
                 JOIN diagram_details dd ON dd.id = af.detail_id
                 WHERE lower(af.name) LIKE '%.psdd'
                 ORDER BY dd.entry_id, af.id",
            )
            .context("Failed to prepare the preview-axis backfill query")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .context("Failed to run the preview-axis backfill query")?;
        let mut done = std::collections::BTreeSet::new();
        for row in rows {
            let (entry_id, name, content) =
                row.context("Failed to read a design file for the preview-axis backfill")?;
            if !crate::local::is_native_design_name(&name) || !done.insert(entry_id) {
                continue;
            }
            if let Some(axis) = crate::local::preview_axis_of_bytes(&content) {
                self.set_preview_axis(entry_id, Some(axis))?;
            }
        }
        Ok(())
    }

    /// Records the crystal axis design `entry_id`'s own file sets; `None` removes it (the
    /// design draws with its material's own axis). Replaces whatever the row held.
    ///
    /// # Errors
    ///
    /// Returns an error if the write fails, including a `Some` for an `entry_id` that
    /// matches no `diagram_entries` row (the table's `FOREIGN KEY`).
    pub fn set_preview_axis(&self, entry_id: i64, axis: Option<PreviewAxis>) -> Result<()> {
        match axis {
            None => {
                self.conn
                    .execute(
                        "DELETE FROM diagram_preview_axes WHERE entry_id = ?1",
                        params![entry_id],
                    )
                    .with_context(|| {
                        format!("Failed to clear the preview axis of diagram entry {entry_id}")
                    })?;
            }
            Some(axis) => {
                self.conn
                    .execute(
                        "INSERT INTO diagram_preview_axes (entry_id, tilt_deg, azimuth_deg, second_deg)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(entry_id) DO UPDATE SET
                            tilt_deg = excluded.tilt_deg,
                            azimuth_deg = excluded.azimuth_deg,
                            second_deg = excluded.second_deg",
                        params![
                            entry_id,
                            f64::from(axis.tilt_deg),
                            f64::from(axis.azimuth_deg),
                            axis.second_deg.map(f64::from)
                        ],
                    )
                    .with_context(|| {
                        format!("Failed to store the preview axis of diagram entry {entry_id}")
                    })?;
            }
        }
        Ok(())
    }

    /// The crystal axis design `entry_id`'s own file sets; `None` for a design without one
    /// (and for a database that lacks the table, which a read-only open of an older file
    /// does).
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying query fails.
    pub fn preview_axis(&self, entry_id: i64) -> Result<Option<PreviewAxis>> {
        if !self.preview_axis_table_exists()? {
            return Ok(None);
        }
        self.conn
            .query_row(
                "SELECT tilt_deg, azimuth_deg, second_deg FROM diagram_preview_axes
                 WHERE entry_id = ?1",
                params![entry_id],
                axis_of_row,
            )
            .optional()
            .with_context(|| format!("Failed to read the preview axis of entry {entry_id}"))
    }

    /// Every design's own crystal axis, keyed by entry id: one query instead of one
    /// [`Self::preview_axis`] per design. Designs without one are absent; the map is empty
    /// when the table does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying query fails.
    pub fn all_preview_axes(&self) -> Result<BTreeMap<i64, PreviewAxis>> {
        if !self.preview_axis_table_exists()? {
            return Ok(BTreeMap::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT tilt_deg, azimuth_deg, second_deg, entry_id FROM diagram_preview_axes
                 ORDER BY entry_id",
            )
            .context("Failed to prepare the all-preview-axes query")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, i64>(3)?, axis_of_row(row)?)))
            .context("Failed to run the all-preview-axes query")?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()
            .context("Failed to read the stored preview axes")?;
        Ok(rows)
    }

    /// Whether `diagram_preview_axes` exists in this database.
    fn preview_axis_table_exists(&self) -> Result<bool> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'diagram_preview_axes'",
                [],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to look for the diagram_preview_axes table")?;
        Ok(found.is_some())
    }
}

/// The axis in the first three columns of `row` (`tilt`, `azimuth`, `second`).
#[expect(
    clippy::cast_possible_truncation,
    reason = "angles in degrees were stored from f32 values"
)]
fn axis_of_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PreviewAxis> {
    Ok(PreviewAxis {
        tilt_deg: row.get::<_, f64>(0)? as f32,
        azimuth_deg: row.get::<_, f64>(1)? as f32,
        second_deg: row.get::<_, Option<f64>>(2)?.map(|v| v as f32),
    })
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
    fn a_design_without_a_row_has_no_axis_of_its_own() {
        let db = Database::new(Some(":memory:")).expect("in-memory database opens");
        let id = entry(&db, "plain");
        assert_eq!(db.preview_axis(id).unwrap(), None);
        assert!(db.all_preview_axes().unwrap().is_empty());
    }

    #[test]
    fn the_axis_round_trips_is_replaced_and_is_cleared() {
        let db = Database::new(Some(":memory:")).expect("in-memory database opens");
        let a = entry(&db, "a");
        let b = entry(&db, "b");
        let axis = PreviewAxis {
            tilt_deg: 30.0,
            azimuth_deg: 120.5,
            second_deg: Some(20.0),
        };
        db.set_preview_axis(a, Some(axis)).unwrap();
        assert_eq!(db.preview_axis(a).unwrap(), Some(axis));
        assert_eq!(db.preview_axis(b).unwrap(), None);

        let other = PreviewAxis {
            tilt_deg: 45.0,
            azimuth_deg: 0.0,
            second_deg: None,
        };
        db.set_preview_axis(a, Some(other)).unwrap();
        db.set_preview_axis(b, Some(axis)).unwrap();
        let all = db.all_preview_axes().unwrap();
        assert_eq!(all.get(&a), Some(&other));
        assert_eq!(all.get(&b), Some(&axis));

        db.set_preview_axis(a, None).unwrap();
        assert_eq!(db.preview_axis(a).unwrap(), None);
        assert_eq!(db.all_preview_axes().unwrap().len(), 1);
    }

    /// A library saved before the table existed: its stored design text sets an axis, the table
    /// is missing, and the build that creates it records the axis; a design whose text leaves
    /// the override off gets no row, and a second run (table present) changes nothing.
    #[test]
    fn creating_the_table_backfills_the_axis_of_stored_design_files() {
        use crate::local::{LOCAL_SOURCE_ID, import_native_design, oriented_design_text};

        let db = Database::new(Some(":memory:")).expect("in-memory database opens");
        let save = |file_name: &str, enabled: bool| {
            let text = oriented_design_text(enabled);
            let imported = import_native_design(file_name, text.as_bytes()).expect("imports");
            db.save_design(&imported.entry, &imported.detail, LOCAL_SOURCE_ID)
                .expect("saves")
        };
        let oriented = save("oriented.psdd", true);
        let plain = save("plain.psdd", false);
        // The state an older database is in: rows exist, the side table does not.
        db.conn
            .execute_batch("DROP TABLE diagram_preview_axes")
            .expect("drops");
        assert_eq!(db.preview_axis(oriented).unwrap(), None);

        db.migrate_preview_axis_table().expect("migrates");
        assert_eq!(
            db.preview_axis(oriented).unwrap(),
            Some(PreviewAxis {
                tilt_deg: 30.0,
                azimuth_deg: 120.0,
                second_deg: Some(20.0),
            })
        );
        assert_eq!(db.preview_axis(plain).unwrap(), None);

        // The table is there now: a later open does not rewrite what a save cleared.
        db.set_preview_axis(oriented, None).unwrap();
        db.migrate_preview_axis_table().expect("migrates again");
        assert_eq!(db.preview_axis(oriented).unwrap(), None);
    }

    #[test]
    fn deleting_the_entry_removes_its_axis() {
        let db = Database::new(Some(":memory:")).expect("in-memory database opens");
        let id = entry(&db, "gone");
        let axis = PreviewAxis::from_effects(true, 10.0, 20.0, None).expect("override is on");
        db.set_preview_axis(id, Some(axis)).unwrap();
        db.conn
            .execute("DELETE FROM diagram_entries WHERE id = ?1", params![id])
            .unwrap();
        assert!(db.all_preview_axes().unwrap().is_empty());
    }

    #[test]
    fn effects_bring_the_axis_into_range_and_off_means_none() {
        assert_eq!(
            PreviewAxis::from_effects(false, 10.0, 20.0, Some(5.0)),
            None
        );
        let axis = PreviewAxis::from_effects(true, f64::NAN, 720.0, Some(400.0)).expect("on");
        assert_eq!(axis.tilt_deg.to_bits(), 0.0f32.to_bits());
        assert_eq!(axis.azimuth_deg.to_bits(), 360.0f32.to_bits());
        assert_eq!(axis.second_deg, Some(180.0));
    }
}
