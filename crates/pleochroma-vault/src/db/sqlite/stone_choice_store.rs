//! The cutter's full choice for a stone of a saved rough plan: pose, free pose, lock, Reason.
//!
//! The pose lives in the `stone_pose_choice` table of `zoning_store`. Three NULLABLE columns
//! extend it, added by a guarded `ALTER TABLE` ([`Database::migrate_stone_choice_columns`], called
//! by `migrate_zoning_tables`):
//!
//! | column | content |
//! |---|---|
//! | `free_pose_json` | a free pose (centre, axes, scale) as JSON text the application owns, or NULL |
//! | `locked` | 1 when the stone is locked, else NULL |
//! | `reason` | the cutter's Reason, or NULL |
//!
//! A pose-only row keeps all three NULL, so it is what an older build wrote, and an older build
//! opens the file unchanged: it never reads the columns, and its own INSERT leaves them NULL. A
//! row whose pose is `0` stays when it carries a free pose, a lock or a Reason (`pose = 0` is the
//! canonical pose), and is removed when it carries nothing.

use super::Database;
use crate::model::zoning::StoneChoiceRow;
use anyhow::{Context, Result};
use rusqlite::params;

/// The nullable columns added to `stone_pose_choice`, with their `ALTER TABLE` text.
const CHOICE_COLUMNS: [(&str, &str); 3] = [
    (
        "free_pose_json",
        "ALTER TABLE stone_pose_choice ADD COLUMN free_pose_json TEXT",
    ),
    (
        "locked",
        "ALTER TABLE stone_pose_choice ADD COLUMN locked INTEGER",
    ),
    (
        "reason",
        "ALTER TABLE stone_pose_choice ADD COLUMN reason TEXT",
    ),
];

impl Database {
    /// Adds the nullable choice columns to `stone_pose_choice` when it lacks them.
    ///
    /// Idempotent; called by `migrate_zoning_tables`, after the table exists.
    ///
    /// # Errors
    ///
    /// Returns an error if an `ALTER TABLE` fails.
    pub(in crate::db::sqlite) fn migrate_stone_choice_columns(&self) -> Result<()> {
        for (column, sql) in CHOICE_COLUMNS {
            if !self.zoning_column_exists("stone_pose_choice", column)? {
                self.conn
                    .execute(sql, [])
                    .with_context(|| format!("Failed to add the {column} column"))?;
            }
        }
        Ok(())
    }

    /// Sets the pose of one stone back to the canonical one: the row is removed when it holds
    /// nothing else, and keeps its free pose, lock and Reason otherwise.
    ///
    /// # Errors
    ///
    /// Returns an error if the update or the delete fails.
    pub(in crate::db::sqlite) fn reset_stone_pose(
        &self,
        plan_id: i64,
        layout_index: u32,
        stone_index: u32,
    ) -> Result<()> {
        let (layout, stone) = (i64::from(layout_index), i64::from(stone_index));
        self.conn
            .execute(
                "UPDATE stone_pose_choice SET pose = 0
                 WHERE plan_id = ?1 AND layout_index = ?2 AND stone_index = ?3
                   AND (free_pose_json IS NOT NULL OR locked IS NOT NULL OR reason IS NOT NULL)",
                params![plan_id, layout, stone],
            )
            .context("Failed to reset a stone pose")?;
        self.conn
            .execute(
                "DELETE FROM stone_pose_choice
                 WHERE plan_id = ?1 AND layout_index = ?2 AND stone_index = ?3
                   AND free_pose_json IS NULL AND locked IS NULL AND reason IS NULL",
                params![plan_id, layout, stone],
            )
            .context("Failed to clear a stone pose choice")?;
        Ok(())
    }

    /// Stores the full choice `row` for a stone of plan `plan_id` (inserts or replaces). A row that
    /// says nothing ([`StoneChoiceRow::is_default`]) removes the stone's row instead, so an unset
    /// stone and a default one are the same thing.
    ///
    /// # Errors
    ///
    /// Returns an error if the upsert or the delete fails (a read-only database, say).
    pub fn set_stone_choice(&self, plan_id: i64, row: &StoneChoiceRow) -> Result<()> {
        let (layout, stone) = (i64::from(row.layout_index), i64::from(row.stone_index));
        if row.is_default() {
            self.conn
                .execute(
                    "DELETE FROM stone_pose_choice
                     WHERE plan_id = ?1 AND layout_index = ?2 AND stone_index = ?3",
                    params![plan_id, layout, stone],
                )
                .context("Failed to clear a stone choice")?;
            return Ok(());
        }
        self.conn
            .execute(
                "INSERT INTO stone_pose_choice
                    (plan_id, layout_index, stone_index, pose, free_pose_json, locked, reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(plan_id, layout_index, stone_index) DO UPDATE SET
                    pose = excluded.pose,
                    free_pose_json = excluded.free_pose_json,
                    locked = excluded.locked,
                    reason = excluded.reason",
                params![
                    plan_id,
                    layout,
                    stone,
                    i64::from(row.pose),
                    row.free_pose_json,
                    row.locked.then_some(1_i64),
                    (!row.reason.is_empty()).then_some(row.reason.as_str()),
                ],
            )
            .context("Failed to store a stone choice")?;
        Ok(())
    }

    /// Every stone choice of plan `plan_id`, ordered by layout then stone: the stones with a
    /// non-canonical pose, a free pose, a lock or a Reason. Empty when the table does not exist
    /// (a read-only file this build has not opened yet); a table without the choice columns
    /// answers with its poses alone.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails or a stored value is out of range.
    pub fn stone_choices(&self, plan_id: i64) -> Result<Vec<StoneChoiceRow>> {
        if !self.zoning_table_exists("stone_pose_choice")? {
            return Ok(Vec::new());
        }
        let extended = self.zoning_column_exists("stone_pose_choice", "reason")?;
        let sql = if extended {
            "SELECT layout_index, stone_index, pose, free_pose_json, locked, reason
             FROM stone_pose_choice WHERE plan_id = ?1 ORDER BY layout_index, stone_index"
        } else {
            "SELECT layout_index, stone_index, pose, NULL, NULL, NULL
             FROM stone_pose_choice WHERE plan_id = ?1 ORDER BY layout_index, stone_index"
        };
        let mut stmt = self
            .conn
            .prepare(sql)
            .context("Failed to prepare the stone choice listing")?;
        let rows = stmt
            .query_map(params![plan_id], |row| {
                let index = |column: usize| -> rusqlite::Result<u32> {
                    let value: i64 = row.get(column)?;
                    u32::try_from(value)
                        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, value))
                };
                let pose: i64 = row.get(2)?;
                let locked: Option<i64> = row.get(4)?;
                let reason: Option<String> = row.get(5)?;
                Ok(StoneChoiceRow {
                    layout_index: index(0)?,
                    stone_index: index(1)?,
                    pose: u8::try_from(pose)
                        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, pose))?,
                    free_pose_json: row.get(3)?,
                    locked: locked.is_some_and(|value| value != 0),
                    reason: reason.unwrap_or_default(),
                })
            })
            .context("Failed to run the stone choice listing")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to read the stone choice listing")
    }
}
