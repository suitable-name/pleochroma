//! Storage for `ri_ranges`, a side table keyed by `entry_id`.
//!
//! The table's SQL is `RI_RANGES_TABLE_SQL` (in `super::migrations`); the row type is
//! `crate::model::ri_range::StoredRiRange`.

use super::Database;
use crate::model::ri_range::StoredRiRange;
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};

/// The `diagram_entries` condition for a "Mine" design: a local import, not hidden.
///
/// "Mine" is the `local://` url prefix, not authorship.
const MINE_CONDITION: &str = "substr(de.url, 1, 8) = 'local://' AND de.ignored = 0";

impl Database {
    /// Stores `range` as the one range of `range.entry_id`, replacing an earlier row.
    ///
    /// # Errors
    ///
    /// Returns an error if the entry does not exist (the foreign key refuses the row), or the
    /// underlying `INSERT ... ON CONFLICT` fails.
    pub fn put_ri_range(&self, range: &StoredRiRange) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO ri_ranges (entry_id, planes_hash, threshold_version, n_low, n_high)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(entry_id) DO UPDATE SET
                     planes_hash = excluded.planes_hash,
                     threshold_version = excluded.threshold_version,
                     n_low = excluded.n_low,
                     n_high = excluded.n_high",
                params![
                    range.entry_id,
                    range.planes_hash,
                    range.threshold_version,
                    range.n_low,
                    range.n_high
                ],
            )
            .with_context(|| {
                format!(
                    "Failed to save the RI range for entry_id: {}",
                    range.entry_id
                )
            })?;
        Ok(())
    }

    /// Loads `entry_id`'s stored RI range, or `None` if none was computed yet.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `SELECT` fails.
    pub fn ri_range(&self, entry_id: i64) -> Result<Option<StoredRiRange>> {
        self.conn
            .query_row(
                "SELECT entry_id, planes_hash, threshold_version, n_low, n_high
                 FROM ri_ranges WHERE entry_id = ?1",
                params![entry_id],
                |row| {
                    Ok(StoredRiRange {
                        entry_id: row.get(0)?,
                        planes_hash: row.get(1)?,
                        threshold_version: row.get(2)?,
                        n_low: row.get(3)?,
                        n_high: row.get(4)?,
                    })
                },
            )
            .optional()
            .with_context(|| format!("Failed to load the RI range for entry_id: {entry_id}"))
    }

    /// The "Mine" entries (`local://`, not ignored) with a row of `threshold_version`, with
    /// their names (the entry titles), ordered by name and then id.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying query fails.
    pub fn mine_ri_ranges(
        &self,
        threshold_version: i64,
    ) -> Result<Vec<(i64, String, StoredRiRange)>> {
        let sql = format!(
            "SELECT de.id, de.title, r.planes_hash, r.threshold_version, r.n_low, r.n_high
             FROM ri_ranges r
             JOIN diagram_entries de ON de.id = r.entry_id
             WHERE r.threshold_version = ?1 AND {MINE_CONDITION}
             ORDER BY de.title COLLATE NOCASE, de.id"
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .context("Failed to prepare the Mine RI-range query")?;
        let rows = stmt
            .query_map(params![threshold_version], |row| {
                let entry_id: i64 = row.get(0)?;
                Ok((
                    entry_id,
                    row.get::<_, String>(1)?,
                    StoredRiRange {
                        entry_id,
                        planes_hash: row.get(2)?,
                        threshold_version: row.get(3)?,
                        n_low: row.get(4)?,
                        n_high: row.get(5)?,
                    },
                ))
            })
            .context("Failed to query the Mine RI ranges")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to read the Mine RI ranges")?;
        Ok(rows)
    }

    /// The "Mine" entries (`local://`, not ignored) with no row, or a row of another
    /// `threshold_version`, in id order.
    ///
    /// A changed `planes_hash` is not found here: the batch compares it when it loads the
    /// entry.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying query fails.
    pub fn entries_missing_ri_range(&self, threshold_version: i64) -> Result<Vec<i64>> {
        let sql = format!(
            "SELECT de.id
             FROM diagram_entries de
             LEFT JOIN ri_ranges r ON r.entry_id = de.id
             WHERE {MINE_CONDITION}
               AND (r.entry_id IS NULL OR r.threshold_version <> ?1)
             ORDER BY de.id"
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .context("Failed to prepare the missing RI-range query")?;
        stmt.query_map(params![threshold_version], |row| row.get(0))
            .context("Failed to query the entries missing an RI range")?
            .collect::<rusqlite::Result<Vec<i64>>>()
            .context("Failed to read the entries missing an RI range")
    }
}
