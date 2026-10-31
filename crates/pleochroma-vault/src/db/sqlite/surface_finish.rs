//! Storage for `diagram_surface_finish` and `diagram_edge_abrasion`: the surface finish a
//! library design's own file states.
//!
//! The finish of a tier (or of single facets of it) other than the ideal polish, and the edge
//! abrasion of the stone, have no notation in `.asc`, so a design imported from a design file
//! keeps them here, keyed by `entry_id` and the tier's position in the design's schedule. The
//! staleness scans never open a design file, so the cached renders' fingerprints and the batch
//! renders read the finish from here, as they read the frosting from `diagram_frosting`. See
//! `crate::model::surface_finish` for the model.
//!
//! Like `diagram_frosting` and `diagram_preview_axes` these are side tables cascading off
//! `diagram_entries`: writing them never touches the entry row or its `updated_at`, and a
//! `diagram_details` re-sync leaves them alone. A design without a row has the ideal polish
//! everywhere, which is also what every design that predates the tables reads as.

use super::Database;
use crate::model::surface_finish::{
    StoredEdgeAbrasion, StoredSurfaceFinish, StoredSurfaceGrade, StoredTierSurface,
};
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use std::collections::BTreeMap;

/// The `slot` of a row that holds the tier's own grade rather than a facet's.
const TIER_SLOT: i64 = -1;

/// The `CREATE TABLE IF NOT EXISTS` text of both tables. No BLOB column, so no `__reordered`
/// variant. `slot` is [`TIER_SLOT`] for the tier's own grade, else the facet's slot in the
/// tier's index list.
const SURFACE_FINISH_TABLES_SQL: &str = "
    CREATE TABLE IF NOT EXISTS diagram_surface_finish (
        entry_id INTEGER NOT NULL,
        tier INTEGER NOT NULL,
        slot INTEGER NOT NULL,
        surface TEXT,
        rq_nm REAL,
        lap_deg REAL,
        aniso REAL,
        no_sweep INTEGER NOT NULL DEFAULT 0,
        ground_alpha REAL,
        PRIMARY KEY (entry_id, tier, slot),
        FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS diagram_edge_abrasion (
        entry_id INTEGER PRIMARY KEY,
        width REAL NOT NULL,
        alpha REAL NOT NULL,
        halo_rq_nm REAL NOT NULL,
        FOREIGN KEY (entry_id) REFERENCES diagram_entries (id) ON DELETE CASCADE
    );
";

/// One stored grade row: `(entry_id, tier, slot, grade)`.
type GradeRow = (i64, i64, i64, StoredSurfaceGrade);

impl Database {
    /// Creates `diagram_surface_finish` and `diagram_edge_abrasion` for a database created
    /// before a library design could keep the surface finish its design file states. Adds
    /// nothing to any existing table; naturally idempotent via `CREATE TABLE IF NOT EXISTS`.
    ///
    /// When the tables are created by this call they are filled once from the design files the
    /// library already stores ([`Self::backfill_surface_finish`]): a design saved before the
    /// tables existed with a surface finish would otherwise keep a thumbnail rendered with the
    /// ideal polish until it was saved again.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the tables fails. A design file that cannot be read is
    /// skipped (it states no finish), not an error.
    pub(in crate::db::sqlite) fn migrate_surface_finish_tables(&self) -> Result<()> {
        let existed = self.surface_finish_table_exists()?;
        self.conn
            .execute_batch(SURFACE_FINISH_TABLES_SQL)
            .context("Failed to create the surface finish tables")?;
        if !existed {
            self.backfill_surface_finish()?;
        }
        Ok(())
    }

    /// Records the surface finish of every entry whose stored design file (an attached `.psdd`;
    /// the first one, in attachment order) states one. Entries without a design
    /// file, or whose file states none, get no row.
    fn backfill_surface_finish(&self) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT dd.entry_id, af.name, af.content
                 FROM attached_files af
                 JOIN diagram_details dd ON dd.id = af.detail_id
                 WHERE lower(af.name) LIKE '%.psdd'
                 ORDER BY dd.entry_id, af.id",
            )
            .context("Failed to prepare the surface-finish backfill query")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .context("Failed to run the surface-finish backfill query")?;
        let mut done = std::collections::BTreeSet::new();
        for row in rows {
            let (entry_id, name, content) =
                row.context("Failed to read a design file for the surface-finish backfill")?;
            if !crate::local::is_native_design_name(&name) || !done.insert(entry_id) {
                continue;
            }
            let finish = crate::local::surface_finish_of_bytes(&content);
            if !finish.is_empty() {
                self.set_diagram_surface_finish(entry_id, &finish)?;
            }
        }
        Ok(())
    }

    /// Replaces the surface finish of design `entry_id` with `finish`; an empty finish makes
    /// the design ideal. `finish` is normalised first (one row per tier and slot, ascending), so
    /// the order and repeats of its lists do not matter.
    ///
    /// Deliberately does NOT bump `diagram_entries.updated_at`, for the reason given on
    /// [`Self::set_planner_excluded`]: a caller that replaces the design writes this right
    /// after [`Self::save_design`], which already moved the stamp.
    ///
    /// # Errors
    ///
    /// Returns an error if a write fails, including a non-empty `finish` for an `entry_id` that
    /// matches no `diagram_entries` row (the tables' `FOREIGN KEY`); nothing is changed in that
    /// case.
    pub fn set_diagram_surface_finish(
        &self,
        entry_id: i64,
        finish: &StoredSurfaceFinish,
    ) -> Result<()> {
        let finish = finish.clone().normalised();
        let tx = self
            .conn
            .unchecked_transaction()
            .context("Failed to start the surface finish transaction")?;
        tx.execute(
            "DELETE FROM diagram_surface_finish WHERE entry_id = ?1",
            params![entry_id],
        )
        .with_context(|| {
            format!("Failed to clear the surface finish of diagram entry {entry_id}")
        })?;
        tx.execute(
            "DELETE FROM diagram_edge_abrasion WHERE entry_id = ?1",
            params![entry_id],
        )
        .with_context(|| {
            format!("Failed to clear the edge abrasion of diagram entry {entry_id}")
        })?;
        for tier in &finish.tiers {
            let position = i64::from(tier.tier);
            if !tier.grade.is_ideal() {
                insert_grade(&tx, entry_id, position, TIER_SLOT, &tier.grade)?;
            }
            for (slot, grade) in &tier.facets {
                insert_grade(&tx, entry_id, position, i64::from(*slot), grade)?;
            }
        }
        if let Some(edge) = finish.edge {
            tx.execute(
                "INSERT INTO diagram_edge_abrasion (entry_id, width, alpha, halo_rq_nm)
                 VALUES (?1, ?2, ?3, ?4)",
                params![entry_id, edge.width, edge.alpha, edge.halo_rq_nm],
            )
            .with_context(|| {
                format!("Failed to store the edge abrasion of diagram entry {entry_id}")
            })?;
        }
        tx.commit().with_context(|| {
            format!("Failed to store the surface finish of diagram entry {entry_id}")
        })
    }

    /// The surface finish of design `entry_id`; empty for a design without any (and for an
    /// `entry_id` that matches no design).
    ///
    /// A read-only database of a file this build has not opened yet may lack the tables
    /// ([`Self::open_read_only`] runs no migration): that reads as ideal too.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running an underlying query fails.
    pub fn diagram_surface_finish(&self, entry_id: i64) -> Result<StoredSurfaceFinish> {
        if !self.surface_finish_table_exists()? {
            return Ok(StoredSurfaceFinish::default());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT entry_id, tier, slot, surface, rq_nm, lap_deg, aniso, no_sweep, ground_alpha
                 FROM diagram_surface_finish WHERE entry_id = ?1 ORDER BY tier, slot",
            )
            .context("Failed to prepare the surface finish query")?;
        let rows = stmt
            .query_map(params![entry_id], grade_of_row)
            .with_context(|| format!("Failed to run the surface finish query of entry {entry_id}"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .with_context(|| format!("Failed to read the surface finish of entry {entry_id}"))?;
        let edge = self
            .conn
            .query_row(
                "SELECT width, alpha, halo_rq_nm FROM diagram_edge_abrasion WHERE entry_id = ?1",
                params![entry_id],
                edge_of_row,
            )
            .optional()
            .with_context(|| format!("Failed to read the edge abrasion of entry {entry_id}"))?;
        Ok(
            assemble(rows, edge.map(|edge| (entry_id, edge)).into_iter())
                .remove(&entry_id)
                .unwrap_or_default(),
        )
    }

    /// Every design's surface finish, keyed by entry id.
    ///
    /// One pair of queries instead of one [`Self::diagram_surface_finish`] per design. Designs
    /// without one are absent; the map is empty when the tables do not exist (a read-only
    /// database this build has not opened yet), like the per-design read.
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running an underlying query fails.
    pub fn all_diagram_surface_finish(&self) -> Result<BTreeMap<i64, StoredSurfaceFinish>> {
        if !self.surface_finish_table_exists()? {
            return Ok(BTreeMap::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT entry_id, tier, slot, surface, rq_nm, lap_deg, aniso, no_sweep, ground_alpha
                 FROM diagram_surface_finish ORDER BY entry_id, tier, slot",
            )
            .context("Failed to prepare the all-surface-finish query")?;
        let rows = stmt
            .query_map([], grade_of_row)
            .context("Failed to run the all-surface-finish query")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to read the stored surface finish")?;
        let mut stmt = self
            .conn
            .prepare(
                "SELECT width, alpha, halo_rq_nm, entry_id FROM diagram_edge_abrasion
                 ORDER BY entry_id",
            )
            .context("Failed to prepare the all-edge-abrasion query")?;
        let edges = stmt
            .query_map([], |row| Ok((row.get::<_, i64>(3)?, edge_of_row(row)?)))
            .context("Failed to run the all-edge-abrasion query")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to read the stored edge abrasion")?;
        Ok(assemble(rows, edges.into_iter()))
    }

    /// Whether `diagram_surface_finish` exists in this database (`diagram_edge_abrasion` is
    /// created in the same step).
    fn surface_finish_table_exists(&self) -> Result<bool> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'diagram_surface_finish'",
                [],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to look for the diagram_surface_finish table")?;
        Ok(found.is_some())
    }
}

/// Writes one grade row.
fn insert_grade(
    tx: &rusqlite::Transaction<'_>,
    entry_id: i64,
    tier: i64,
    slot: i64,
    grade: &StoredSurfaceGrade,
) -> Result<()> {
    tx.execute(
        "INSERT INTO diagram_surface_finish
            (entry_id, tier, slot, surface, rq_nm, lap_deg, aniso, no_sweep, ground_alpha)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            entry_id,
            tier,
            slot,
            grade.surface,
            grade.rq_nm,
            grade.lap_deg,
            grade.aniso,
            i64::from(grade.no_sweep),
            grade.ground_alpha
        ],
    )
    .with_context(|| {
        format!("Failed to store the surface finish of tier {tier} of diagram entry {entry_id}")
    })?;
    Ok(())
}

/// The grade row in the nine columns `entry_id, tier, slot, surface, rq_nm, lap_deg, aniso,
/// no_sweep, ground_alpha`.
fn grade_of_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GradeRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        StoredSurfaceGrade {
            surface: row.get(3)?,
            rq_nm: row.get(4)?,
            lap_deg: row.get(5)?,
            aniso: row.get(6)?,
            no_sweep: row.get::<_, i64>(7)? != 0,
            ground_alpha: row.get(8)?,
        },
    ))
}

/// The edge in the first three columns of `row` (`width`, `alpha`, `halo_rq_nm`).
fn edge_of_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredEdgeAbrasion> {
    Ok(StoredEdgeAbrasion {
        width: row.get(0)?,
        alpha: row.get(1)?,
        halo_rq_nm: row.get(2)?,
    })
}

/// The finishes the grade rows and the edge rows describe, keyed by entry id and normalised. A
/// row with a tier or slot outside the stored ranges is skipped.
fn assemble(
    rows: Vec<GradeRow>,
    edges: impl Iterator<Item = (i64, StoredEdgeAbrasion)>,
) -> BTreeMap<i64, StoredSurfaceFinish> {
    let mut by_entry: BTreeMap<i64, BTreeMap<u32, StoredTierSurface>> = BTreeMap::new();
    for (entry_id, tier, slot, grade) in rows {
        let Ok(tier) = u32::try_from(tier) else {
            continue;
        };
        let entry = by_entry
            .entry(entry_id)
            .or_default()
            .entry(tier)
            .or_insert_with(|| StoredTierSurface {
                tier,
                ..StoredTierSurface::default()
            });
        if slot == TIER_SLOT {
            entry.grade = grade;
        } else if let Ok(slot) = u32::try_from(slot) {
            entry.facets.push((slot, grade));
        }
    }
    let mut finishes: BTreeMap<i64, StoredSurfaceFinish> = by_entry
        .into_iter()
        .map(|(entry_id, tiers)| {
            (
                entry_id,
                StoredSurfaceFinish {
                    tiers: tiers.into_values().collect(),
                    edge: None,
                },
            )
        })
        .collect();
    for (entry_id, edge) in edges {
        finishes.entry(entry_id).or_default().edge = Some(edge);
    }
    finishes
        .into_iter()
        .map(|(entry_id, finish)| (entry_id, finish.normalised()))
        .filter(|(_, finish)| !finish.is_empty())
        .collect()
}
