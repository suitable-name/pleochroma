//! [`Tag`]: one row of the catalogue's flat tag set.
//!
//! Flat by design: a tag has no parent or child (no folders) and no metadata beyond its
//! name, while a design can carry any number of tags. The schema, and why the tags are a
//! side table pair rather than a column on `diagram_entries`, are documented on
//! `crate::db::sqlite::Database::migrate_tag_tables`.

use serde::{Deserialize, Serialize};

/// One tag, as stored in the `tags` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tag {
    /// Database identifier.
    pub id: i64,
    /// Display name, unique ignoring ASCII case.
    pub name: String,
}
