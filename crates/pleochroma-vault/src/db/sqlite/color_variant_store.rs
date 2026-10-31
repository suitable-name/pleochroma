//! Storage for the color variants a cutter saved for a material.
//!
//! One more side table next to the ones in `pattern_store`, with the same rules: created by
//! `CREATE TABLE IF NOT EXISTS` in the same migrate step, no foreign keys (an older build never
//! reads or deletes it, and this build removes the rows whose material is gone with
//! [`Database::prune_color_variant_orphans`]), and a read-only database of a file this build has
//! not opened answers "nothing stored".
//!
//! | table | key | content |
//! |---|---|---|
//! | `material_color_variants` | `key` (`user.<uuid>`) | the material name, the label, the variant JSON, its format version, two UTC timestamps |
//!
//! The vault never parses the JSON: `pleochroma-cut` owns the format (a tensor of band rows and
//! its unit). A variant of a built-in material is stored too (its `material_name` is the built-in's
//! name), so the orphan prune takes the built-in names from the caller.
//!
//! The table is named `material_color_variants`, not `variants`: the vault already has
//! `diagram_variants`, which are design-history variants.

use super::Database;
use crate::model::zoning::MaterialColorVariantRow;
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params, params_from_iter};

/// The `CREATE TABLE IF NOT EXISTS` text of the color variant table and its index (text columns
/// last, no BLOBs).
const COLOR_VARIANT_TABLES_SQL: &str = "
    CREATE TABLE IF NOT EXISTS material_color_variants (
        key TEXT PRIMARY KEY,
        material_name TEXT NOT NULL COLLATE NOCASE,
        label TEXT NOT NULL,
        version INTEGER NOT NULL,
        variant_json TEXT NOT NULL,
        created_utc TEXT NOT NULL,
        updated_utc TEXT NOT NULL
    );

    CREATE INDEX IF NOT EXISTS material_color_variants_by_material
        ON material_color_variants(material_name);
";

/// The columns every reader selects, in the order [`color_variant_of`] reads them.
const COLUMNS: &str = "key, material_name, label, version, variant_json, created_utc, updated_utc";

/// The SQLite expression for "now" as UTC text (`YYYY-MM-DDTHH:MM:SSZ`).
const NOW_UTC: &str = "strftime('%Y-%m-%dT%H:%M:%SZ', 'now')";

/// Reads the [`COLUMNS`] of one row.
fn color_variant_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<MaterialColorVariantRow> {
    let version: i64 = row.get(3)?;
    Ok(MaterialColorVariantRow {
        key: row.get(0)?,
        material_name: row.get(1)?,
        label: row.get(2)?,
        version: u32::try_from(version)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(3, version))?,
        variant_json: row.get(4)?,
        created_utc: row.get(5)?,
        updated_utc: row.get(6)?,
    })
}

impl Database {
    /// Creates the color variant table and its index when they are missing.
    ///
    /// Naturally idempotent; called by [`Database::new`].
    ///
    /// # Errors
    ///
    /// Returns an error if creating the table or the index fails.
    pub(in crate::db::sqlite) fn migrate_color_variant_tables(&self) -> Result<()> {
        self.conn
            .execute_batch(COLOR_VARIANT_TABLES_SQL)
            .context("Failed to create the color variant table")
    }

    /// Whether the color variant table exists (a read-only database of an older file may lack
    /// it).
    fn color_variant_table_exists(&self) -> Result<bool> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'material_color_variants'",
                [],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to look for the color variant table")?;
        Ok(found.is_some())
    }

    // ---- writers ------------------------------------------------------------------------

    /// Stores (inserts or replaces) the variant `row.key`.
    ///
    /// A new row gets both timestamps set to now; replacing keeps `created_utc` and sets
    /// `updated_utc` to now (the timestamps of `row` itself are ignored).
    ///
    /// # Errors
    ///
    /// Returns an error if the upsert fails (a read-only database refuses it).
    pub fn save_color_variant(&self, row: &MaterialColorVariantRow) -> Result<()> {
        self.conn
            .execute(
                &format!(
                    "INSERT INTO material_color_variants
                        (key, material_name, label, version, variant_json, created_utc, updated_utc)
                     VALUES (?1, ?2, ?3, ?4, ?5, {NOW_UTC}, {NOW_UTC})
                     ON CONFLICT(key) DO UPDATE SET
                        material_name = excluded.material_name,
                        label = excluded.label,
                        version = excluded.version,
                        variant_json = excluded.variant_json,
                        updated_utc = excluded.updated_utc"
                ),
                params![
                    row.key,
                    row.material_name,
                    row.label,
                    i64::from(row.version),
                    row.variant_json
                ],
            )
            .with_context(|| format!("Failed to store the color variant '{}'", row.key))?;
        Ok(())
    }

    /// Sets the label of variant `key`. Returns how many rows changed (`0` when the key does
    /// not exist).
    ///
    /// # Errors
    ///
    /// Returns an error if the update fails.
    pub fn rename_color_variant_label(&self, key: &str, label: &str) -> Result<usize> {
        if !self.color_variant_table_exists()? {
            return Ok(0);
        }
        self.conn
            .execute(
                &format!(
                    "UPDATE material_color_variants
                     SET label = ?2, updated_utc = {NOW_UTC} WHERE key = ?1"
                ),
                params![key, label],
            )
            .with_context(|| format!("Failed to rename the color variant '{key}'"))
    }

    /// Moves every variant of material `from` to material `to` (both ASCII case-insensitive).
    /// Returns how many rows moved.
    ///
    /// For a material that is renamed in place; the desktop's "save under a new name" copies
    /// instead and leaves the old name's rows to its delete.
    ///
    /// # Errors
    ///
    /// Returns an error if the update fails.
    pub fn rename_color_variant_material(&self, from: &str, to: &str) -> Result<usize> {
        if !self.color_variant_table_exists()? {
            return Ok(0);
        }
        self.conn
            .execute(
                &format!(
                    "UPDATE material_color_variants
                     SET material_name = ?2, updated_utc = {NOW_UTC}
                     WHERE material_name = ?1 COLLATE NOCASE"
                ),
                params![from, to],
            )
            .with_context(|| format!("Failed to move the color variants of '{from}' to '{to}'"))
    }

    /// Removes variant `key`. Returns how many rows were removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the `DELETE` fails.
    pub fn delete_color_variant(&self, key: &str) -> Result<usize> {
        if !self.color_variant_table_exists()? {
            return Ok(0);
        }
        self.conn
            .execute(
                "DELETE FROM material_color_variants WHERE key = ?1",
                params![key],
            )
            .with_context(|| format!("Failed to delete the color variant '{key}'"))
    }

    /// Removes every variant of `material_name` (ASCII case-insensitive). Returns how many rows
    /// were removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the `DELETE` fails.
    pub fn delete_color_variants_of_material(&self, material_name: &str) -> Result<usize> {
        if !self.color_variant_table_exists()? {
            return Ok(0);
        }
        self.conn
            .execute(
                "DELETE FROM material_color_variants WHERE material_name = ?1 COLLATE NOCASE",
                params![material_name],
            )
            .with_context(|| format!("Failed to delete the color variants of '{material_name}'"))
    }

    // ---- readers ------------------------------------------------------------------------

    /// The variant stored under `key`, or `None`.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails or a stored version is out of range.
    pub fn load_color_variant(&self, key: &str) -> Result<Option<MaterialColorVariantRow>> {
        if !self.color_variant_table_exists()? {
            return Ok(None);
        }
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM material_color_variants WHERE key = ?1"),
                params![key],
                color_variant_of,
            )
            .optional()
            .with_context(|| format!("Failed to read the color variant '{key}'"))
    }

    /// Every variant of `material_name` (ASCII case-insensitive), ordered by label (ASCII
    /// case-insensitive), then key. Empty when nothing is stored.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails or a stored version is out of range.
    pub fn load_color_variants_of_material(
        &self,
        material_name: &str,
    ) -> Result<Vec<MaterialColorVariantRow>> {
        if !self.color_variant_table_exists()? {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {COLUMNS} FROM material_color_variants
                 WHERE material_name = ?1 COLLATE NOCASE
                 ORDER BY label COLLATE NOCASE, key"
            ))
            .context("Failed to prepare the color variant listing")?;
        let rows = stmt
            .query_map(params![material_name], color_variant_of)
            .with_context(|| format!("Failed to list the color variants of '{material_name}'"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .with_context(|| format!("Failed to read the color variants of '{material_name}'"))
    }

    // ---- housekeeping -------------------------------------------------------------------

    /// Removes the variants whose material is neither a custom material nor one of
    /// `built_in_names` (ASCII case-insensitive): the rows a deleted custom material left behind.
    /// Returns how many rows were removed; a missing table, or a table with no orphans, removes
    /// nothing and writes nothing (so the prune also succeeds on a read-only connection).
    ///
    /// The vault does not know the built-in materials, so the caller passes their names. An
    /// empty `built_in_names` treats every variant of a non-custom material as an orphan.
    ///
    /// # Errors
    ///
    /// Returns an error if the orphan count or the `DELETE` fails.
    pub fn prune_color_variant_orphans(&self, built_in_names: &[&str]) -> Result<usize> {
        if !self.color_variant_table_exists()? {
            return Ok(0);
        }
        let placeholders = (0..built_in_names.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let built_in_clause = if built_in_names.is_empty() {
            String::new()
        } else {
            format!("AND material_name NOT IN ({placeholders})")
        };
        let orphans: i64 = self
            .conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM material_color_variants
                     WHERE material_name NOT IN (SELECT name FROM custom_gem_materials)
                     {built_in_clause}"
                ),
                params_from_iter(built_in_names.iter()),
                |row| row.get(0),
            )
            .context("Failed to count the material_color_variants orphans")?;
        if orphans == 0 {
            return Ok(0);
        }
        self.conn
            .execute(
                &format!(
                    "DELETE FROM material_color_variants
                     WHERE material_name NOT IN (SELECT name FROM custom_gem_materials)
                     {built_in_clause}"
                ),
                params_from_iter(built_in_names.iter()),
            )
            .context("Failed to prune material_color_variants")
    }
}
