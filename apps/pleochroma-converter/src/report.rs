//! Machine-readable accounting of every library entry and design attachment.

use serde::Serialize;

#[derive(Default, Serialize)]
pub struct Summary {
    pub entries: usize,
    pub sources: usize,
    pub converted: usize,
    pub drafts: usize,
    pub failed: usize,
    pub skipped: usize,
    pub missing_sources: usize,
    pub adopted_meets: usize,
}

impl Summary {
    pub const fn is_partial(&self) -> bool {
        self.failed != 0 || self.missing_sources != 0 || self.drafts != 0
    }
}

#[derive(Serialize)]
pub struct Report {
    pub schema: u32,
    pub converter_version: &'static str,
    pub database: String,
    pub adopt_source_meets: bool,
    pub summary: Summary,
    pub records: Vec<Record>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Converted,
    Draft,
    Failed,
    Skipped,
    MissingSource,
}

#[derive(Serialize)]
pub struct Record {
    pub entry_id: i64,
    pub title: String,
    pub attachment_id: Option<i64>,
    pub source_name: Option<String>,
    pub status: Status,
    pub output: Option<String>,
    pub adopted_meets: usize,
    pub notes: Vec<String>,
}

impl Record {
    pub fn new(entry: &crate::database::Entry, source: Option<&crate::database::Source>) -> Self {
        Self {
            entry_id: entry.id,
            title: entry.title.clone(),
            attachment_id: source.map(|source| source.id),
            source_name: source.map(|source| source.name.clone()),
            status: Status::Failed,
            output: None,
            adopted_meets: 0,
            notes: Vec::new(),
        }
    }
}
