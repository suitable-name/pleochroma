//! Read-only catalogue access, including entries without convertible attachments.

use std::{collections::BTreeSet, path::Path, time::Duration};

use anyhow::{Context, Result, ensure};
use pleochroma_cut_core::native::DesignMetadata;
use rusqlite::{Connection, OpenFlags, Row, types::ValueRef};

#[derive(Debug)]
pub struct Entry {
    pub id: i64,
    pub title: String,
    pub metadata: DesignMetadata,
}

#[derive(Debug)]
pub struct Source {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub bytes: Vec<u8>,
}

pub struct Library {
    conn: Connection,
}

impl Library {
    pub fn open(path: &Path) -> Result<Self> {
        ensure!(path.is_file(), "database is not a file: {}", path.display());
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("cannot open database read-only: {}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        // One read transaction makes the listing and all later source reads refer to
        // the same catalogue revision even when the application is open beside us.
        conn.execute_batch("PRAGMA query_only = ON; BEGIN DEFERRED TRANSACTION;")?;
        Ok(Self { conn })
    }

    pub fn entries(&self) -> Result<Vec<Entry>> {
        let entry_columns = self.columns("diagram_entries")?;
        let detail_columns = self.columns("diagram_details")?;
        let entry = |column| optional_column("de", column, &entry_columns);
        let detail = |column| optional_column("dd", column, &detail_columns);
        let columns = [
            entry("url"),
            entry("design_id"),
            entry("ignored"),
            detail("page_url"),
            detail("designer"),
            detail("designer_info"),
            detail("source_citation"),
            detail("shape"),
            detail("shape_category"),
            detail("competition_diagram"),
            detail("pdf_file"),
            detail("gem_file"),
        ]
        .join(", ");
        let sql = format!(
            "SELECT de.id, de.title, {columns} FROM diagram_entries de \
             LEFT JOIN diagram_details dd ON dd.id = \
                 (SELECT MIN(d.id) FROM diagram_details d WHERE d.entry_id = de.id) \
             ORDER BY de.id"
        );
        self.conn
            .prepare(&sql)?
            .query_map([], entry_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("cannot enumerate catalogue entries")
    }

    pub fn sources(&self, entry_id: i64) -> Result<Vec<Source>> {
        self.conn
            .prepare(
                "SELECT af.id, af.name, af.url, af.content \
                 FROM attached_files af \
                 JOIN diagram_details dd ON dd.id = af.detail_id \
                 WHERE dd.entry_id = ?1 AND (lower(af.name) LIKE '%.asc' \
                    OR lower(af.name) LIKE '%.psdd' OR lower(af.name) LIKE '%.gem' \
                    OR lower(af.name) LIKE '%.gcs') ORDER BY af.id",
            )?
            .query_map([entry_id], |row| {
                let bytes = match row.get_ref(3)? {
                    ValueRef::Blob(bytes) | ValueRef::Text(bytes) => bytes.to_vec(),
                    ValueRef::Null => Vec::new(),
                    other => {
                        return Err(rusqlite::Error::InvalidColumnType(
                            3,
                            "content".to_owned(),
                            other.data_type(),
                        ));
                    }
                };
                Ok(Source {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    url: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    bytes,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .with_context(|| format!("cannot read design attachments for entry {entry_id}"))
    }

    fn columns(&self, table: &str) -> Result<BTreeSet<String>> {
        // `table` is supplied only by the fixed calls above, never by file contents.
        let columns = self
            .conn
            .prepare(&format!("PRAGMA table_info({table})"))?
            .query_map([], |row| row.get(1))?
            .collect::<rusqlite::Result<BTreeSet<String>>>()?;
        ensure!(!columns.is_empty(), "database table {table} is missing");
        Ok(columns)
    }
}

fn optional_column(alias: &str, column: &str, available: &BTreeSet<String>) -> String {
    if available.contains(column) {
        // Metadata columns have changed from TEXT to numeric in older catalogues.
        // These fields are descriptive strings or integer category ids, not geometry.
        format!("CAST({alias}.{column} AS TEXT)")
    } else {
        "NULL".to_owned()
    }
}

fn entry_from_row(row: &Row<'_>) -> rusqlite::Result<Entry> {
    let text = |index| {
        row.get::<_, Option<String>>(index)
            .map(Option::unwrap_or_default)
    };
    let title: String = row.get(1)?;
    let source_url = [text(5)?, text(2)?]
        .into_iter()
        .find(|url| {
            let url = url.trim();
            !url.is_empty() && !url.to_ascii_lowercase().starts_with("local://")
        })
        .unwrap_or_default();
    let metadata = DesignMetadata {
        title: title.clone(),
        source_url,
        source_design_id: text(3)?,
        ignored: text(4)?.parse::<i64>().is_ok_and(|value| value != 0),
        designer: text(6)?,
        designer_info: text(7)?,
        source_citation: text(8)?,
        shape: text(9)?,
        shape_category: text(10)?,
        competition: text(11)?,
        pdf_file: text(12)?,
        gem_file: text(13)?,
        ..DesignMetadata::default()
    };
    Ok(Entry {
        id: row.get(0)?,
        title,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Library {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE diagram_entries (id INTEGER PRIMARY KEY, title TEXT, url TEXT, ignored INTEGER); \
             CREATE TABLE diagram_details (id INTEGER PRIMARY KEY, entry_id INTEGER, designer TEXT); \
             CREATE TABLE attached_files (id INTEGER PRIMARY KEY, detail_id INTEGER, name TEXT, url TEXT, content BLOB); \
             INSERT INTO diagram_entries VALUES (9, 'Ignored', 'local://private.asc', 1), (2, 'No source', 'https://example.org/design', 0); \
             INSERT INTO diagram_details VALUES (5, 9, 'Designer');",
        )
        .unwrap();
        Library { conn }
    }

    #[test]
    fn includes_ignored_and_missing_detail_entries_without_inventing_metadata() {
        let entries = library().entries().unwrap();
        assert_eq!(
            entries.iter().map(|entry| entry.id).collect::<Vec<_>>(),
            [2, 9]
        );
        assert_eq!(entries[0].metadata.source_url, "https://example.org/design");
        assert!(entries[0].metadata.designer.is_empty());
        assert!(entries[1].metadata.ignored);
        assert!(entries[1].metadata.source_url.is_empty());
        assert_eq!(entries[1].metadata.designer, "Designer");
    }

    #[test]
    fn reads_all_design_attachments_in_id_order_and_keeps_text_bytes() {
        let library = library();
        library
            .conn
            .execute_batch(
                "INSERT INTO attached_files VALUES \
                 (12, 5, 'copy.ASC', '', 'text'), (7, 5, 'copy.ASC', '', X'0102'), \
                 (13, 5, 'native.PsDd', '', NULL), (14, 5, 'diagram.pdf', '', X'03');",
            )
            .unwrap();
        let sources = library.sources(9).unwrap();
        assert_eq!(
            sources.iter().map(|source| source.id).collect::<Vec<_>>(),
            [7, 12, 13]
        );
        assert_eq!(sources[0].bytes, [1, 2]);
        assert_eq!(sources[1].bytes, b"text");
        assert!(sources[2].bytes.is_empty());
        assert!(library.sources(2).unwrap().is_empty());
    }
}
