//! Storage for the Color Patterns of custom materials and the saved pattern presets.
//!
//! Two more side tables next to the ones in `zoning_store`, with the same rules: created by
//! `CREATE TABLE IF NOT EXISTS`, no foreign keys (an older build never reads or deletes them, and
//! this build removes the rows whose material is gone with [`Database::prune_pattern_orphans`]),
//! and a read-only database of a file this build has not opened answers "nothing stored".
//!
//! | table | key | content |
//! |---|---|---|
//! | `material_pattern` | `material_name` | the pattern JSON, its format version |
//! | `color_pattern_presets` | `name` | the pattern JSON, its format version |
//!
//! The vault never parses the JSON: `pleochroma-cut` owns the format. A material with a pattern
//! keeps its `material_zoning` row (the resolved zones the renderer reads) and its base color in
//! the material row's band column, so an older build shows the base color and ignores the rest.

use super::{CustomMaterialParams, Database};
use crate::model::zoning::{ColorPatternPresetRow, MaterialPatternRow, MaterialZoningRow};
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};

/// The `CREATE TABLE IF NOT EXISTS` text of the two pattern side tables (text columns last, no
/// BLOBs).
const PATTERN_TABLES_SQL: &str = "
    CREATE TABLE IF NOT EXISTS material_pattern (
        material_name TEXT PRIMARY KEY COLLATE NOCASE,
        version INTEGER NOT NULL,
        pattern_json TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS color_pattern_presets (
        name TEXT PRIMARY KEY COLLATE NOCASE,
        version INTEGER NOT NULL,
        pattern_json TEXT NOT NULL
    );
";

/// Reads `material_name, version, pattern_json`.
fn material_pattern_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<MaterialPatternRow> {
    let version: i64 = row.get(1)?;
    Ok(MaterialPatternRow {
        material_name: row.get(0)?,
        version: u32::try_from(version)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(1, version))?,
        pattern_json: row.get(2)?,
    })
}

/// Reads `name, version, pattern_json`.
fn preset_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<ColorPatternPresetRow> {
    let version: i64 = row.get(1)?;
    Ok(ColorPatternPresetRow {
        name: row.get(0)?,
        version: u32::try_from(version)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(1, version))?,
        pattern_json: row.get(2)?,
    })
}

/// The upsert behind the pattern writers.
fn upsert_material_pattern(conn: &rusqlite::Connection, row: &MaterialPatternRow) -> Result<()> {
    conn.execute(
        "INSERT INTO material_pattern (material_name, version, pattern_json)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(material_name) DO UPDATE SET
            version = excluded.version,
            pattern_json = excluded.pattern_json",
        params![row.material_name, i64::from(row.version), row.pattern_json],
    )
    .with_context(|| {
        format!(
            "Failed to store the color pattern of '{}'",
            row.material_name
        )
    })?;
    Ok(())
}

impl Database {
    /// Creates the pattern side tables when they are missing.
    ///
    /// Naturally idempotent; called by [`Database::new`].
    ///
    /// # Errors
    ///
    /// Returns an error if creating a table fails.
    pub(in crate::db::sqlite) fn migrate_pattern_tables(&self) -> Result<()> {
        self.conn
            .execute_batch(PATTERN_TABLES_SQL)
            .context("Failed to create the color pattern side tables")
    }

    /// Whether table `name` exists (a read-only database of an older file may lack the pattern
    /// tables).
    fn pattern_table_exists(&self, name: &str) -> Result<bool> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()
            .with_context(|| format!("Failed to look for table {name}"))?;
        Ok(found.is_some())
    }

    // ---- material_pattern ---------------------------------------------------------------

    /// Stores custom material `material`, its pattern `pattern` and its zones `zoning` in ONE
    /// transaction.
    ///
    /// The material row is rewritten from `material` (the caller puts the base zone's band rows in
    /// `material.absorption_bands_json`, so a reader that knows no zones shows the base color), the
    /// pattern row is upserted, and the `material_zoning` row is upserted from `Some` or deleted
    /// for `None` (a Uniform pattern has no zones).
    ///
    /// # Errors
    ///
    /// Returns an error if the names differ, or if any write or the commit fails; the transaction
    /// is then rolled back and the vault is unchanged.
    pub fn save_material_pattern_with_zoning(
        &self,
        material: &CustomMaterialParams<'_>,
        pattern: &MaterialPatternRow,
        zoning: Option<&MaterialZoningRow>,
    ) -> Result<()> {
        if material.name != pattern.material_name
            || zoning.is_some_and(|row| row.material_name != material.name)
        {
            anyhow::bail!(
                "The pattern or zones of another material cannot be stored with '{}'",
                material.name
            );
        }
        let transaction = self
            .conn
            .unchecked_transaction()
            .context("Failed to start the material pattern write")?;
        self.save_custom_material(material)?;
        upsert_material_pattern(&transaction, pattern)?;
        match zoning {
            Some(row) => self.save_material_zoning(row)?,
            None => {
                transaction
                    .execute(
                        "DELETE FROM material_zoning WHERE material_name = ?1 COLLATE NOCASE",
                        params![pattern.material_name],
                    )
                    .with_context(|| {
                        format!("Failed to clear the zones of '{}'", pattern.material_name)
                    })?;
            }
        }
        transaction
            .commit()
            .context("Failed to commit the material pattern write")
    }

    /// Stores (inserts or replaces) the pattern row alone, leaving the material and its zones
    /// as they are (the dialog write-back).
    ///
    /// # Errors
    ///
    /// Returns an error if the upsert fails.
    pub fn save_material_pattern(&self, row: &MaterialPatternRow) -> Result<()> {
        upsert_material_pattern(&self.conn, row)
    }

    /// The pattern of custom material `material_name` (ASCII case-insensitive), or `None`.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails or a stored version is out of range.
    pub fn load_material_pattern(&self, material_name: &str) -> Result<Option<MaterialPatternRow>> {
        if !self.pattern_table_exists("material_pattern")? {
            return Ok(None);
        }
        self.conn
            .query_row(
                "SELECT material_name, version, pattern_json
                 FROM material_pattern WHERE material_name = ?1 COLLATE NOCASE",
                params![material_name],
                material_pattern_of,
            )
            .optional()
            .with_context(|| format!("Failed to read the color pattern of '{material_name}'"))
    }

    /// Removes the pattern of `material_name` (ASCII case-insensitive); the zones stay.
    /// Returns how many rows were removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the `DELETE` fails.
    pub fn delete_material_pattern(&self, material_name: &str) -> Result<usize> {
        if !self.pattern_table_exists("material_pattern")? {
            return Ok(0);
        }
        self.conn
            .execute(
                "DELETE FROM material_pattern WHERE material_name = ?1 COLLATE NOCASE",
                params![material_name],
            )
            .with_context(|| format!("Failed to delete the color pattern of '{material_name}'"))
    }

    /// Removes the pattern AND the zones of `material_name` in one transaction; the material row
    /// itself stays. Nothing stored is a no-op that succeeds.
    ///
    /// # Errors
    ///
    /// Returns an error if a `DELETE` or the commit fails.
    pub fn delete_material_pattern_and_zoning(&self, material_name: &str) -> Result<()> {
        let transaction = self
            .conn
            .unchecked_transaction()
            .context("Failed to start the pattern and zones delete")?;
        for (table, statement) in [
            (
                "material_pattern",
                "DELETE FROM material_pattern WHERE material_name = ?1 COLLATE NOCASE",
            ),
            (
                "material_zoning",
                "DELETE FROM material_zoning WHERE material_name = ?1 COLLATE NOCASE",
            ),
        ] {
            if !self.pattern_table_exists(table)? {
                continue;
            }
            transaction
                .execute(statement, params![material_name])
                .with_context(|| format!("Failed to delete '{material_name}' from {table}"))?;
        }
        transaction
            .commit()
            .context("Failed to commit the pattern and zones delete")
    }

    // ---- color_pattern_presets ----------------------------------------------------------

    /// Stores (inserts or replaces) the preset `row.name`.
    ///
    /// # Errors
    ///
    /// Returns an error if the upsert fails.
    pub fn save_color_pattern_preset(&self, row: &ColorPatternPresetRow) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO color_pattern_presets (name, version, pattern_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(name) DO UPDATE SET
                    version = excluded.version,
                    pattern_json = excluded.pattern_json",
                params![row.name, i64::from(row.version), row.pattern_json],
            )
            .with_context(|| format!("Failed to store the color pattern preset '{}'", row.name))?;
        Ok(())
    }

    /// Every saved preset, ordered by name (ASCII case-insensitive).
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails or a stored version is out of range.
    pub fn load_color_pattern_presets(&self) -> Result<Vec<ColorPatternPresetRow>> {
        if !self.pattern_table_exists("color_pattern_presets")? {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT name, version, pattern_json FROM color_pattern_presets
                 ORDER BY name COLLATE NOCASE",
            )
            .context("Failed to prepare the color pattern preset listing")?;
        let rows = stmt
            .query_map([], preset_of)
            .context("Failed to run the color pattern preset listing")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to read the color pattern preset listing")
    }

    /// Removes preset `name` (ASCII case-insensitive). Returns how many rows were removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the `DELETE` fails.
    pub fn delete_color_pattern_preset(&self, name: &str) -> Result<usize> {
        if !self.pattern_table_exists("color_pattern_presets")? {
            return Ok(0);
        }
        self.conn
            .execute(
                "DELETE FROM color_pattern_presets WHERE name = ?1 COLLATE NOCASE",
                params![name],
            )
            .with_context(|| format!("Failed to delete the color pattern preset '{name}'"))
    }

    /// Renames preset `from` to `to`. Returns how many rows were renamed (`0` when `from` does
    /// not exist).
    ///
    /// # Errors
    ///
    /// Returns an error if the update fails, which includes `to` being the name of another
    /// preset.
    pub fn rename_color_pattern_preset(&self, from: &str, to: &str) -> Result<usize> {
        if !self.pattern_table_exists("color_pattern_presets")? {
            return Ok(0);
        }
        self.conn
            .execute(
                "UPDATE color_pattern_presets SET name = ?2 WHERE name = ?1 COLLATE NOCASE",
                params![from, to],
            )
            .with_context(|| format!("Failed to rename the color pattern preset '{from}'"))
    }

    // ---- housekeeping -------------------------------------------------------------------

    /// Removes the `material_pattern` rows whose custom material no longer exists, through
    /// `conn` (the transaction of [`Database::prune_zoning_orphans`]). Returns how many rows were
    /// removed; a missing table, or a table with no orphans, removes nothing and writes nothing
    /// (so the prune also succeeds on a read-only connection).
    ///
    /// # Errors
    ///
    /// Returns an error if the orphan count or the `DELETE` fails.
    pub(in crate::db::sqlite) fn prune_pattern_orphans(
        &self,
        conn: &rusqlite::Connection,
    ) -> Result<usize> {
        if !self.pattern_table_exists("material_pattern")? {
            return Ok(0);
        }
        let orphans: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM material_pattern
                 WHERE material_name NOT IN (SELECT name FROM custom_gem_materials)",
                [],
                |row| row.get(0),
            )
            .context("Failed to count the material_pattern orphans")?;
        if orphans == 0 {
            return Ok(0);
        }
        conn.execute(
            "DELETE FROM material_pattern
             WHERE material_name NOT IN (SELECT name FROM custom_gem_materials)",
            [],
        )
        .context("Failed to prune material_pattern")
    }
}
