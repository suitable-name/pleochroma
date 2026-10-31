//! The upload order of the videos: the tutorial browser's order (the sections of
//! [`GuideCategory::ALL`], the learning path of `CURRICULUM` inside each section), numbered
//! from 1.
//!
//! Every video file starts with its number (`07-solving-trials.mp4`), zero-padded to the width
//! of the largest number, and sits in its section's folder (`05-solving-and-checking/`, see
//! [`crate::section_folders`]), so the folders and the files sorted by name are the upload
//! order. `playlist.md` in the output folder lists number, section, title and file for the
//! whole catalogue.

use crate::section_folders::section_folder;
use pleochroma_editor::guide::{GuideCatalog, GuideCategory, browser_rows};
use std::{
    collections::BTreeSet,
    fmt::Write as _,
    fs, io,
    path::{Path, PathBuf},
};

/// One tutorial in upload order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Numbered {
    /// The position in the upload order, from 1.
    pub number: usize,
    /// The guide's id.
    pub id: String,
    /// The guide's title.
    pub title: String,
    /// The browser section it sits in.
    pub section: GuideCategory,
}

/// Every built-in tutorial in upload order.
#[must_use]
pub fn upload_order() -> Vec<Numbered> {
    let catalog = GuideCatalog::new();
    browser_rows(catalog.all(), &BTreeSet::new(), true, "")
        .into_iter()
        .enumerate()
        .map(|(index, row)| Numbered {
            number: index + 1,
            id: row.id,
            title: row.title,
            section: row.category,
        })
        .collect()
}

/// The digits every number is padded to: those of the largest number, at least two.
fn width(count: usize) -> usize {
    count.to_string().len().max(2)
}

/// The folder of `section` under the output folder.
#[must_use]
pub fn folder_of(section: GuideCategory) -> String {
    let index = GuideCategory::ALL
        .iter()
        .position(|&category| category == section)
        .unwrap_or(GuideCategory::ALL.len());
    section_folder(index, section.label())
}

/// The path of tutorial `id`'s files relative to the output folder, without extension: its
/// section's folder and its numbered stem. An id the browser does not list keeps its bare id at
/// the top of the output folder.
#[must_use]
pub fn relative_stem(id: &str) -> PathBuf {
    let order = upload_order();
    let digits = width(order.len());
    order.iter().find(|entry| entry.id == id).map_or_else(
        || PathBuf::from(id),
        |entry| {
            Path::new(&folder_of(entry.section)).join(format!("{:0digits$}-{id}", entry.number))
        },
    )
}

/// The text of `playlist.md`.
#[must_use]
pub fn playlist_text() -> String {
    let order = upload_order();
    let digits = width(order.len());
    let mut text = String::from(
        "# Tutorial videos in upload order\n\n| # | Section | Title | File |\n|---|---|---|---|\n",
    );
    for entry in &order {
        let _ = writeln!(
            text,
            "| {number:0digits$} | {section} | {title} | {folder}/{number:0digits$}-{id}.mp4 |",
            number = entry.number,
            section = entry.section.label(),
            title = entry.title,
            folder = folder_of(entry.section),
            id = entry.id,
        );
    }
    text
}

/// Writes `playlist.md` into `out_dir` unless it already holds exactly this text (the worker
/// processes of one run all reach here; only a changed catalogue rewrites the file).
///
/// # Errors
///
/// The folder or the file cannot be written.
pub fn write_playlist(out_dir: &Path) -> io::Result<()> {
    let path = out_dir.join("playlist.md");
    let text = playlist_text();
    if fs::read_to_string(&path).is_ok_and(|old| old == text) {
        return Ok(());
    }
    fs::create_dir_all(out_dir)?;
    fs::write(path, text)
}
