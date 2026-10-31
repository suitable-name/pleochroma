//! The design library screen without the window: what the page shows and what its buttons do,
//! as plain functions the native tests can run.
//!
//! - [`run_query`]: the search box, the filter combos and chips and the sort order become the
//!   indexes of the matching entries (the vault's `local::search`, the same rules the desktop
//!   library uses); [`filter_options`] lists what the combos offer.
//! - [`row_text`], [`detail_fields`], [`detail_info`]: the words on a row and in the detail pane.
//! - [`apply_detail_edit`]: the detail pane's Save Changes, validated.
//! - [`open_candidate`] and [`export_psdd`]: the file an entry opens from, and its `.psdd`
//!   export with the library's metadata written into the file's `[meta]` table.
//! - [`ImportReport`]: what the Import button tells the user.
//! - [`storage_line`]: the used share of the browser's storage.

use crate::{
    design_meta::iso8601_utc_from_epoch_ms,
    library_measure::design_of_file,
    library_record::{LibraryEntry, StorageEstimate, normalize_tags},
    open_route::design_file_name_for_asc,
    storage_keys::sha256_hex,
};
use pleochroma_cut_core::native::{
    DesignExtras, design_from_str, design_to_file, design_to_string,
};
use pleochroma_formats::native::design::{self as design_file, DesignMetadata};
use pleochroma_vault::{
    local::{
        DesignFileKind, is_native_design_name, native_design_attachment_position,
        primary_design_attachment,
        search::{SearchOptions, SearchRecord, SortOrder, search},
    },
    model::{design_key::normalize_design_uuid, file::AttachedFile, filter::RangeFilter},
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::Write as _,
};

mod report;

pub use report::{ImportReport, Severity};

/// The combo entry that means "no restriction".
pub const ALL: &str = "All";

/// How many rows the list draws at once; the rest wait behind a refined search.
pub const PAGE_SIZE: usize = 120;

/// The size of a cached thumbnail in pixels (the Solid view's 16:10 aspect).
pub const THUMBNAIL_SIZE: (u32, u32) = (192, 120);

// ---------------------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------------------

/// The order of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LibrarySort {
    /// Newest first (the order designs came into the library).
    #[default]
    RecentlyAdded,
    /// The last edited first.
    RecentlyEdited,
    /// Title A to Z.
    Title,
    /// The oldest first.
    OldestFirst,
}

impl LibrarySort {
    /// Every order, in the sort combo's order.
    pub const ALL: [Self; 4] = [
        Self::RecentlyAdded,
        Self::RecentlyEdited,
        Self::Title,
        Self::OldestFirst,
    ];

    /// The label the sort combo shows (Title Case).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::RecentlyAdded => "Recently Added",
            Self::RecentlyEdited => "Recently Edited",
            Self::Title => "Title A-Z",
            Self::OldestFirst => "Oldest First",
        }
    }

    /// The order the combo's `index` names; an index out of range is the default order.
    #[must_use]
    pub fn from_index(index: i32) -> Self {
        usize::try_from(index)
            .ok()
            .and_then(|i| Self::ALL.get(i).copied())
            .unwrap_or_default()
    }

    /// The search engine's order for this one.
    #[must_use]
    pub const fn order(self) -> SortOrder {
        match self {
            Self::RecentlyAdded => SortOrder::Newest,
            Self::RecentlyEdited => SortOrder::RecentlyEdited,
            Self::Title => SortOrder::Title,
            Self::OldestFirst => SortOrder::CatalogueOrder,
        }
    }
}

/// Everything the library's search bar and filters hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryQuery {
    /// The search box (title, designer, design id and tier notes).
    pub text: String,
    /// The shape combo's value; empty or [`ALL`] for any.
    pub shape: String,
    /// The gear combo's value; empty or [`ALL`] for any.
    pub gear: String,
    /// The tag combo's value; empty or [`ALL`] for any.
    pub tag: String,
    /// The "Concave Only" chip.
    pub concave_only: bool,
    /// The "Show Ignored" chip.
    pub show_ignored: bool,
    /// The sort combo.
    pub sort: LibrarySort,
}

/// The entries a query matched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryResult {
    /// Indexes into the entry slice, in display order (capped by the search engine).
    pub indexes: Vec<usize>,
    /// How many entries match in all.
    pub total: usize,
}

/// The entries of `entries` that match `query`, in the order it asks for.
///
/// Entries are told apart by their store id (`item.id`), which the store makes unique.
#[must_use]
pub fn run_query(entries: &[LibraryEntry], query: &LibraryQuery) -> QueryResult {
    let records: Vec<SearchRecord> = entries.iter().map(LibraryEntry::search_record).collect();
    let range = RangeFilter {
        include_ignored: query.show_ignored,
        has_concave: query.concave_only.then_some(true),
        ..RangeFilter::default()
    };
    let tag = query.tag.trim();
    let options = SearchOptions {
        order: query.sort.order(),
        local_only: false,
        tag_filter: (!tag.is_empty() && tag != ALL).then_some(tag),
        id_filter: None,
    };
    let outcome = search(
        &records,
        &query.text,
        query.shape.trim(),
        query.gear.trim(),
        &range,
        options,
    );
    let position: HashMap<i64, usize> = records
        .iter()
        .enumerate()
        .map(|(index, record)| (record.id, index))
        .collect();
    QueryResult {
        indexes: outcome
            .items
            .iter()
            .filter_map(|record| position.get(&record.id).copied())
            .collect(),
        total: outcome.total,
    }
}

/// What the shape, gear and tag combos offer for the entries held.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterOptions {
    /// Distinct shape names, A to Z ignoring case.
    pub shapes: Vec<String>,
    /// Distinct gears, ascending.
    pub gears: Vec<String>,
    /// Distinct tags, A to Z ignoring case.
    pub tags: Vec<String>,
}

/// The gear a list item's text names (`"96"` and `"96.0"` are 96).
fn gear_of(text: &str) -> Option<i64> {
    let text = text.trim();
    text.parse::<i64>()
        .ok()
        .or_else(|| text.strip_suffix(".0").and_then(|head| head.parse().ok()))
}

/// The values the filter combos offer for `entries`.
#[must_use]
pub fn filter_options(entries: &[LibraryEntry]) -> FilterOptions {
    let mut shapes: BTreeMap<String, String> = BTreeMap::new();
    let mut gears: BTreeSet<i64> = BTreeSet::new();
    let mut tags: BTreeMap<String, String> = BTreeMap::new();
    for entry in entries {
        if let Some(shape) = entry.item.shape.as_deref().map(str::trim)
            && !shape.is_empty()
        {
            shapes
                .entry(shape.to_lowercase())
                .or_insert_with(|| shape.to_owned());
        }
        if let Some(gear) = entry.item.index_gear.as_deref().and_then(gear_of) {
            gears.insert(gear);
        }
        for tag in &entry.tags {
            tags.entry(tag.to_lowercase())
                .or_insert_with(|| tag.clone());
        }
    }
    FilterOptions {
        shapes: shapes.into_values().collect(),
        gears: gears.into_iter().map(|gear| gear.to_string()).collect(),
        tags: tags.into_values().collect(),
    }
}

/// A combo's items: [`ALL`] first, then `values`.
#[must_use]
pub fn combo_items(values: &[String]) -> Vec<String> {
    std::iter::once(ALL.to_owned())
        .chain(values.iter().cloned())
        .collect()
}

/// The combo index showing `selected` among [`combo_items`]; [`ALL`] (0) when it is gone.
#[must_use]
pub fn combo_index(values: &[String], selected: &str) -> usize {
    values
        .iter()
        .position(|value| value == selected)
        .map_or(0, |at| at + 1)
}

/// The value the combo item `index` stands for: empty for [`ALL`] (and an index out of range).
#[must_use]
pub fn combo_value(values: &[String], index: i32) -> String {
    usize::try_from(index)
        .ok()
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| values.get(i))
        .cloned()
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------
// Rows and the detail pane
// ---------------------------------------------------------------------------------------

/// A number as text with `decimals` places; text that is not a number comes back as it was.
#[must_use]
pub fn short_number(text: &str, decimals: usize) -> String {
    let text = text.trim();
    text.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map_or_else(|| text.to_owned(), |value| format!("{value:.decimals$}"))
}

/// The words of one list row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowText {
    /// The title.
    pub title: String,
    /// Shape, gear, refractive index, length/width and facet count, as far as known.
    pub subtitle: String,
    /// The tags, comma separated.
    pub tags: String,
}

/// The words of `entry`'s row.
#[must_use]
pub fn row_text(entry: &LibraryEntry) -> RowText {
    let item = &entry.item;
    let present = |text: &Option<String>| {
        text.as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let parts: Vec<String> = [
        present(&item.shape),
        present(&item.index_gear).map(|gear| format!("Gear {}", short_number(&gear, 0))),
        present(&item.refractive_index).map(|ri| format!("RI {}", short_number(&ri, 3))),
        present(&item.lw_ratio).map(|lw| format!("L/W {}", short_number(&lw, 2))),
        present(&item.facets_count).map(|facets| format!("{facets} facets")),
        (entry.detail.concave_tiers > 0).then(|| "Concave".to_owned()),
    ]
    .into_iter()
    .flatten()
    .collect();
    RowText {
        title: item.title.clone(),
        subtitle: parts.join(" \u{b7} "),
        tags: entry.tags.join(", "),
    }
}

/// The editable fields of the detail pane, as the text boxes hold them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetailFields {
    /// The title (required).
    pub title: String,
    /// The designer.
    pub designer: String,
    /// The publication citation.
    pub citation: String,
    /// The shape name (free text).
    pub shape: String,
    /// The refractive index.
    pub ri: String,
    /// The length-to-width ratio.
    pub lw: String,
    /// The volume as `Vol/W^3`.
    pub volume: String,
    /// The tags, comma separated.
    pub tags: String,
    /// The design is hidden from the list unless "Show Ignored" is on.
    pub ignored: bool,
}

/// The detail pane's fields for `entry`.
#[must_use]
pub fn detail_fields(entry: &LibraryEntry) -> DetailFields {
    let detail = &entry.detail;
    let text = |value: &Option<String>| value.as_deref().unwrap_or_default().trim().to_owned();
    DetailFields {
        title: entry.item.title.clone(),
        designer: text(&detail.designer),
        citation: text(&detail.source_citation),
        shape: text(&detail.shape),
        ri: text(&detail.refractive_index),
        lw: short_number(&text(&detail.lw_ratio), 3),
        volume: short_number(&text(&detail.volume), 3),
        tags: entry.tags.join(", "),
        ignored: entry.item.ignored,
    }
}

/// `text` as an optional number text: blank is `None`, a number inside `low..=high` stays as
/// typed, anything else is refused with `label`.
fn number_field(text: &str, label: &str, low: f64, high: f64) -> Result<Option<String>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<f64>() {
        Ok(value) if value.is_finite() && (low..=high).contains(&value) => {
            Ok(Some(text.to_owned()))
        }
        _ => Err(format!("{label} must be a number from {low} to {high}.")),
    }
}

/// `value` trimmed; blank is `None`.
fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// The `Designer; Citation` line the scraped catalogue (and the search) reads.
fn designer_line(designer: Option<&str>, citation: Option<&str>) -> Option<String> {
    match (designer, citation) {
        (Some(designer), Some(citation)) => Some(format!("{designer}; {citation}")),
        (Some(only), None) | (None, Some(only)) => Some(only.to_owned()),
        (None, None) => None,
    }
}

/// Applies the detail pane's fields to `entry`.
///
/// Nothing changes when a field is refused. The listing text (`entry.item`) follows the
/// edited detail; the `Designer; Citation` line is rebuilt only when the designer or the
/// citation changed, so a hand-written line survives an edit of another field.
///
/// # Errors
///
/// The title is blank, or a number field holds something that is not a plausible number.
pub fn apply_detail_edit(entry: &mut LibraryEntry, fields: &DetailFields) -> Result<(), String> {
    let title = fields.title.trim();
    if title.is_empty() {
        return Err("A design needs a title.".to_owned());
    }
    let ri = number_field(&fields.ri, "Refractive index", 1.0, 5.0)?;
    let lw = number_field(&fields.lw, "Length/width", 0.01, 100.0)?;
    let volume = number_field(&fields.volume, "Volume", 0.0, 1.0e6)?;
    let designer = optional_text(&fields.designer);
    let citation = optional_text(&fields.citation);

    let detail = &mut entry.detail;
    let line_changed = detail.designer.as_deref().map(str::trim) != designer.as_deref()
        || detail.source_citation.as_deref().map(str::trim) != citation.as_deref();
    if line_changed {
        detail.designer_info = designer_line(designer.as_deref(), citation.as_deref());
    }
    detail.designer = designer;
    detail.source_citation = citation;
    detail.shape = optional_text(&fields.shape);
    detail.refractive_index = ri;
    detail.lw_ratio = lw;
    detail.volume = volume;
    title.clone_into(&mut entry.item.title);
    entry.item.ignored = fields.ignored;
    entry.tags = normalize_tags(fields.tags.split(','));
    entry.refresh_item();
    Ok(())
}

/// The read-only lines under the detail pane's fields: facts the file states and the library's
/// own dates. Missing facts are left out.
#[must_use]
pub fn detail_info(entry: &LibraryEntry) -> String {
    let detail = &entry.detail;
    let mut lines: Vec<String> = Vec::new();
    let mut fact = |label: &str, value: Option<String>| {
        if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
            lines.push(format!("{label}: {}", value.trim()));
        }
    };
    fact("Gear", detail.index_gear.clone());
    fact("Facets", detail.facets_count.clone());
    fact(
        "Symmetry",
        detail.symmetry_order.as_ref().map(|order| {
            if detail.mirror_symmetry == Some(true) {
                format!("{order}-fold, mirror")
            } else {
                format!("{order}-fold")
            }
        }),
    );
    let ratios = [
        ("H/W", &detail.hw_ratio),
        ("T/W", &detail.tw_ratio),
        ("C/W", &detail.cw_ratio),
        ("P/W", &detail.pw_ratio),
    ]
    .into_iter()
    .filter_map(|(label, value)| {
        value
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .map(|text| format!("{label} {}", short_number(text, 2)))
    })
    .collect::<Vec<_>>();
    fact(
        "Proportions",
        (!ratios.is_empty()).then(|| ratios.join(" \u{b7} ")),
    );
    fact(
        "Concave Tiers",
        (detail.concave_tiers > 0).then(|| detail.concave_tiers.to_string()),
    );
    fact("Design File", entry.design_file.clone());
    fact("Added", date_text(entry.added_ms));
    fact("Edited", date_text(entry.updated_ms));
    lines.join("\n")
}

/// The date part (`2026-10-26`) of a `Date.now()` value; `None` for a time before 1971.
fn date_text(ms: f64) -> Option<String> {
    if !ms.is_finite() || ms < 31_536_000_000.0 {
        return None;
    }
    iso8601_utc_from_epoch_ms(ms).get(..10).map(str::to_owned)
}

// ---------------------------------------------------------------------------------------
// Open and export
// ---------------------------------------------------------------------------------------

/// The attachment an entry opens from.
///
/// A native design file first; else a `.gcs` (it states the frosting an `.asc` cannot); else
/// the vault's rule (`.asc`, then `.gem`).
#[must_use]
pub fn open_candidate(files: &[AttachedFile]) -> Option<&AttachedFile> {
    let names = || files.iter().map(|file| file.name.as_str());
    if let Some(position) = native_design_attachment_position(names()) {
        return files.get(position);
    }
    files
        .iter()
        .find(|file| {
            matches!(
                DesignFileKind::from_file_name(&file.name),
                Some(DesignFileKind::Gcs)
            )
        })
        .or_else(|| primary_design_attachment(files))
}

/// A design file ready to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedDesign {
    /// The file name (`<stem>.psdd`).
    pub file_name: String,
    /// The file text.
    pub text: String,
}

/// Writes the library's metadata over `meta`'s fields: identity, title, designer, citation,
/// shape, tags and marks; every other field (and any unknown key) stays as read.
fn merge_entry_meta(meta: &mut DesignMetadata, entry: &LibraryEntry, now_iso: &str) {
    let text = |value: &Option<String>| value.as_deref().unwrap_or_default().trim().to_owned();
    meta.id.clone_from(&entry.design_id);
    meta.title.clone_from(&entry.item.title);
    meta.designer = text(&entry.detail.designer);
    meta.designer_info = text(&entry.detail.designer_info);
    meta.source_citation = text(&entry.detail.source_citation);
    meta.shape = text(&entry.detail.shape);
    meta.tags.clone_from(&entry.tags);
    meta.ignored = entry.item.ignored;
    meta.planner_excluded = entry.planner_excluded;
    if meta.created_at.is_empty() {
        meta.created_at = iso8601_utc_from_epoch_ms(entry.added_ms);
    }
    now_iso.clone_into(&mut meta.modified_at);
}

/// `entry` as a self-contained `.psdd` design file.
///
/// An entry stored as a design file is read and written again with its attachments, history,
/// custom material and every other `[meta]` field kept; an `.asc`, `.gem` or `.gcs` entry is
/// converted. Either way the library's id, title, designer, citation, shape, tags and marks
/// go into the file's `[meta]` table, so what was edited in the library travels with the file.
///
/// # Errors
///
/// The entry holds no design file, the file does not read, or the writer refuses the result
/// (an attachment over the size cap, a text field too long).
pub fn export_psdd(
    entry: &LibraryEntry,
    files: &[AttachedFile],
    now_iso: &str,
) -> Result<ExportedDesign, String> {
    let file =
        open_candidate(files).ok_or_else(|| "This entry holds no design file.".to_owned())?;
    let written = if is_native_design_name(&file.name) {
        let source = std::str::from_utf8(&file.content)
            .map_err(|_| format!("\"{}\" is not a text (UTF-8) design file.", file.name))?;
        let loaded = design_from_str(source)
            .map_err(|error| format!("\"{}\" cannot be read: {error}", file.name))?;
        let mut meta = loaded.metadata.clone();
        merge_entry_meta(&mut meta, entry, now_iso);
        let file_record = design_to_file(
            &loaded.design,
            loaded.printed_proportions.as_ref(),
            &DesignExtras {
                custom_material: loaded.restorable_custom_material.as_ref(),
                history_entries: &loaded.history_entries,
                metadata: Some(&meta),
                attachments: &loaded.attachments,
            },
        )
        .with_draft(loaded.draft);
        design_file::to_string(&file_record)
    } else {
        let design = design_of_file(file)?;
        let mut meta = DesignMetadata::default();
        merge_entry_meta(&mut meta, entry, now_iso);
        design_to_string(
            &design,
            None,
            &DesignExtras {
                metadata: Some(&meta),
                ..DesignExtras::default()
            },
        )
    };
    let text =
        written.map_err(|error| format!("Cannot write \"{}\": {error}", entry.item.title))?;
    let stem = entry
        .item
        .url
        .strip_prefix("local://")
        .filter(|name| !name.is_empty())
        .unwrap_or(&entry.item.title);
    Ok(ExportedDesign {
        file_name: design_file_name_for_asc(stem),
        text,
    })
}

/// The text a cached thumbnail is stamped with: it names the design file it was drawn from, so
/// a replaced file's picture reads as stale.
#[must_use]
pub fn thumbnail_stamp(design_file_bytes: &[u8]) -> String {
    format!("t1:{}", sha256_hex(design_file_bytes))
}

/// The `[meta] id` of the design file `bytes` (named `file_name`), when it has a valid one.
///
/// The file must be a design file. The id keys a new library entry, so the same design keeps
/// one entry across saves and re-imports.
#[must_use]
pub fn meta_id_of(file_name: &str, bytes: &[u8]) -> Option<String> {
    if !is_native_design_name(file_name) {
        return None;
    }
    let loaded = design_from_str(std::str::from_utf8(bytes).ok()?).ok()?;
    normalize_design_uuid(&loaded.metadata.id)
}

/// `count` and the noun, pluralised the plain way.
fn counted(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

// ---------------------------------------------------------------------------------------
// Storage line
// ---------------------------------------------------------------------------------------

/// Bytes as `3.4 MB`: binary multiples, one decimal from MB up.
#[must_use]
pub fn human_bytes(bytes: f64) -> String {
    const KIB: f64 = 1024.0;
    let bytes = if bytes.is_finite() {
        bytes.max(0.0)
    } else {
        0.0
    };
    if bytes < KIB {
        format!("{bytes:.0} B")
    } else if bytes < KIB * KIB {
        format!("{:.0} KB", bytes / KIB)
    } else if bytes < KIB * KIB * KIB {
        format!("{:.1} MB", bytes / (KIB * KIB))
    } else {
        format!("{:.1} GB", bytes / (KIB * KIB * KIB))
    }
}

/// The one line under the library's title: how many designs, and the share of the browser's
/// storage this site uses when the browser said.
#[must_use]
pub fn storage_line(
    designs: usize,
    unreadable: usize,
    estimate: Option<StorageEstimate>,
) -> String {
    let mut text = counted(designs, "design");
    if unreadable > 0 {
        let _ = write!(text, " ({unreadable} stored in a newer format and hidden)");
    }
    if let Some(estimate) = estimate.filter(|estimate| estimate.quota_bytes > 0.0) {
        let _ = write!(
            text,
            " \u{b7} {} of {} used",
            human_bytes(estimate.usage_bytes),
            human_bytes(estimate.quota_bytes)
        );
        if estimate.is_nearly_full() {
            text.push_str(" \u{b7} storage is nearly full");
        }
    }
    text
}

#[cfg(test)]
mod tests;
