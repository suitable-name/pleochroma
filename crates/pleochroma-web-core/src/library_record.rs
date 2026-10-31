//! The pure half of the browser design library: records, keys, error classes, the import plan.
//!
//! Nothing here touches the DOM, so it is tested natively: only the vault's `model` and `local`
//! types (`default-features = false`), `serde` and `std`. The page's `storage::library` moves
//! the records in and out of `IndexedDB`.
//!
//! # Identity
//!
//! An entry is keyed by its design id: the file's `[meta] id` when it has one, else the
//! name-based UUID of its url ([`catalogue_design_uuid`]), like the desktop's side tables. The
//! dedupe key is the entry's `url` (`local://<file name>`).
//!
//! [`LibraryEntry`] is the summary record: [`DiagramListItem`] plus the light
//! `FacetingDiagramDetail`, tags and planner mark. Attachment bytes are a separate record
//! ([`StoredFiles`]), never part of a listing.

use pleochroma_vault::{
    local::{
        ExistingRow, ImportedAsc, primary_design_attachment, reconcile_with_existing,
        search::SearchRecord,
    },
    model::{
        cut_progress::CutProgressMark,
        design_key::{catalogue_design_uuid, normalize_design_uuid},
        design_lighting::DesignLighting,
        design_variant::{DesignVariant, NewVariant, VariantSummary},
        detail::FacetingDiagramDetail,
        entry::{DiagramListItem, FacetingDiagramEntry, FullDiagramRecord},
        file::AttachedFile,
    },
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};

/// The version of the JSON shapes in this file.
///
/// A record carries it so a later build can migrate an older shape instead of failing to
/// read it.
pub const RECORD_VERSION: u32 = 1;

/// The largest thumbnail PNG the store accepts, in bytes.
pub const MAX_THUMBNAIL_BYTES: usize = 256 * 1024;

/// The first eight bytes of every PNG file.
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// The share of the quota at which [`StorageEstimate::is_nearly_full`] turns true.
const NEARLY_FULL_FRACTION: f64 = 0.9;

// ---------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------

/// Why a store call failed, in the classes a screen words differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreErrorKind {
    /// `IndexedDB` is missing, blocked or not allowed to write (private browsing in some
    /// browsers, site data blocked, an open that stays blocked).
    Unavailable,
    /// The browser refused the write for lack of space (`QuotaExceededError`).
    Quota,
    /// The caller's input was refused before anything was written (bad id, empty name).
    Invalid,
    /// A stored record could not be read back in the shape this build expects.
    Corrupt,
    /// Any other failure; the message says which.
    Other,
}

/// A failed store call: a class for the screen and the text for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError {
    /// The class.
    pub kind: StoreErrorKind,
    /// The browser's or this module's message.
    pub message: String,
}

impl StoreError {
    /// An error of `kind` with `message`.
    #[must_use]
    pub fn new(kind: StoreErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// An [`StoreErrorKind::Invalid`] error.
    #[must_use]
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StoreErrorKind::Invalid, message)
    }

    /// A [`StoreErrorKind::Corrupt`] error.
    #[must_use]
    pub fn corrupt(message: impl Into<String>) -> Self {
        Self::new(StoreErrorKind::Corrupt, message)
    }

    /// The error a browser failure text stands for, classified by [`classify_error_text`].
    #[must_use]
    pub fn from_text(text: impl Into<String>) -> Self {
        let message = text.into();
        Self {
            kind: classify_error_text(&message),
            message,
        }
    }

    /// Whether the browser refused for lack of space.
    #[must_use]
    pub fn is_quota(&self) -> bool {
        self.kind == StoreErrorKind::Quota
    }

    /// One plain sentence for the user; the message stays for the log.
    #[must_use]
    pub fn user_text(&self) -> String {
        match self.kind {
            StoreErrorKind::Unavailable => {
                "This browser does not let the page keep a library (private browsing or blocked site data). Your open design is not affected.".to_owned()
            }
            StoreErrorKind::Quota => {
                "The browser's storage for this site is full. Delete designs from the library or free space; your open design is not affected.".to_owned()
            }
            StoreErrorKind::Invalid => self.message.clone(),
            StoreErrorKind::Corrupt | StoreErrorKind::Other => {
                format!("The library could not be read or written: {}", self.message)
            }
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<String> for StoreError {
    fn from(text: String) -> Self {
        Self::from_text(text)
    }
}

/// The class of a failure text.
///
/// The browser layer words a `DOMException` as `"<Name>: <message>"`, so the exception name
/// decides: `QuotaExceededError` is [`StoreErrorKind::Quota`]; a missing `IndexedDB`, a
/// `SecurityError`, an `InvalidStateError` (Firefox's private-mode refusal), an
/// `UnknownError`, a `VersionError` or a blocked open is [`StoreErrorKind::Unavailable`].
#[must_use]
pub fn classify_error_text(text: &str) -> StoreErrorKind {
    let lower = text.to_ascii_lowercase();
    if lower.contains("quota") {
        return StoreErrorKind::Quota;
    }
    let unavailable = [
        "indexeddb is unavailable",
        "no window",
        "securityerror",
        "invalidstateerror",
        "unknownerror",
        "versionerror",
        "blocked",
        "notallowederror",
    ];
    if unavailable.iter().any(|needle| lower.contains(needle)) {
        return StoreErrorKind::Unavailable;
    }
    StoreErrorKind::Other
}

// ---------------------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------------------

/// The canonical design id of `text`, or an [`StoreErrorKind::Invalid`] error.
///
/// # Errors
///
/// `text` is not an `8-4-4-4-12` hexadecimal UUID.
pub fn require_design_id(text: &str) -> Result<String, StoreError> {
    normalize_design_uuid(text)
        .ok_or_else(|| StoreError::invalid(format!("'{text}' is not a design id")))
}

/// The design id an entry with `url` gets: the file's own `[meta] id` when it has a valid
/// one, else the deterministic UUID of the url.
#[must_use]
pub fn design_id_for(url: &str, meta_id: Option<&str>) -> String {
    meta_id
        .and_then(normalize_design_uuid)
        .unwrap_or_else(|| catalogue_design_uuid(url))
}

/// The url every file imported under `file_name` is stored under (the desktop rule).
#[must_use]
pub fn local_url(file_name: &str) -> String {
    format!("local://{file_name}")
}

/// What a side-data record holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideKind {
    /// One saved variant (one record per variant).
    Variant,
    /// The steps marked done (one record per design).
    CutProgress,
    /// The design's own lighting choice (one record per design).
    Lighting,
}

impl SideKind {
    /// The kind's name inside a key.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Variant => "variant",
            Self::CutProgress => "cut",
            Self::Lighting => "lighting",
        }
    }
}

/// The key of a side-data record: `<design id>/<kind>[/<sub, 20 digits>]`.
///
/// The padded `sub` keeps variants in id order under `IndexedDB`'s string ordering.
#[must_use]
pub fn side_key(design_id: &str, kind: SideKind, sub: Option<i64>) -> String {
    sub.map_or_else(
        || format!("{design_id}/{}", kind.name()),
        |sub| format!("{design_id}/{}/{sub:020}", kind.name()),
    )
}

/// The key of a variant's thumbnail in the thumbnail store.
#[must_use]
pub fn variant_thumbnail_key(design_id: &str, variant_id: i64) -> String {
    side_key(design_id, SideKind::Variant, Some(variant_id))
}

/// `[lower, upper)` such that every key starting with `prefix` is in the range and nothing
/// else is; `prefix` must end with `/`.
///
/// The upper bound is the prefix with its `/` replaced by the next character, `0`.
#[must_use]
pub fn prefix_bounds(prefix: &str) -> (String, String) {
    let stem = prefix.strip_suffix('/').unwrap_or(prefix);
    (format!("{stem}/"), format!("{stem}0"))
}

/// The bounds of every side-data key (and variant thumbnail key) of `design_id`.
#[must_use]
pub fn design_prefix_bounds(design_id: &str) -> (String, String) {
    prefix_bounds(&format!("{design_id}/"))
}

/// The bounds of every variant key of `design_id`.
#[must_use]
pub fn variant_prefix_bounds(design_id: &str) -> (String, String) {
    prefix_bounds(&format!("{design_id}/{}/", SideKind::Variant.name()))
}

// ---------------------------------------------------------------------------------------
// Entries
// ---------------------------------------------------------------------------------------

/// One library entry: what a listing, the search and the detail pane read.
///
/// `item.id` is a number from the store's own counter (stable for the life of the entry;
/// `0` before the first write). The web identity is [`Self::design_id`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryEntry {
    /// The record shape version, [`RECORD_VERSION`].
    pub version: u32,
    /// The key of the entry and of all its side data.
    pub design_id: String,
    /// The display row. Its `ignored` field is the ignored mark.
    pub item: DiagramListItem,
    /// The detail row without bytes: ratios, designer, concave counts, the angle table. Its
    /// `attached_files` is empty and its `diagram_image_data` is `None`.
    pub detail: FacetingDiagramDetail,
    /// The flat tags, trimmed, de-duplicated and sorted.
    pub tags: Vec<String>,
    /// The design is excluded from the Rough Planner's candidate set.
    pub planner_excluded: bool,
    /// When the entry was first stored, milliseconds since the Unix epoch.
    pub added_ms: f64,
    /// When the entry was last written, milliseconds since the Unix epoch.
    pub updated_ms: f64,
    /// The name of the attachment that is the design file, if the import had one.
    pub design_file: Option<String>,
}

impl LibraryEntry {
    /// Whether the entry is one of the user's own imports (`local://` url).
    #[must_use]
    pub fn is_local(&self) -> bool {
        self.item.url.starts_with("local://")
    }

    /// The full vault record of this entry with `attachments` attached.
    ///
    /// It is what the vault's re-import rules (`reconcile_with_existing`,
    /// `merge_reimport_metadata`) compare a fresh parse against.
    #[must_use]
    pub fn full_record(&self, attachments: Vec<AttachedFile>) -> FullDiagramRecord {
        let detail = &self.detail;
        FullDiagramRecord {
            entry_id: self.item.id,
            title: self.item.title.clone(),
            url: self.item.url.clone(),
            design_id: self.item.design_id.clone(),
            page_url: detail.page_url.clone(),
            diagram_image_name: detail.diagram_image_name.clone(),
            diagram_image_data: detail.diagram_image_data.clone(),
            competition_diagram: detail.competition_diagram.clone(),
            lw_ratio: detail.lw_ratio.clone(),
            refractive_index: detail.refractive_index.clone(),
            index_gear: detail.index_gear.clone(),
            volume: detail.volume.clone(),
            facets_count: detail.facets_count.clone(),
            shape: detail.shape.clone(),
            designer_info: detail.designer_info.clone(),
            hw_ratio: detail.hw_ratio.clone(),
            tw_ratio: detail.tw_ratio.clone(),
            uw_ratio: detail.uw_ratio.clone(),
            pw_ratio: detail.pw_ratio.clone(),
            cw_ratio: detail.cw_ratio.clone(),
            symmetry_order: detail.symmetry_order.clone(),
            mirror_symmetry: detail.mirror_symmetry,
            designer: detail.designer.clone(),
            source_citation: detail.source_citation.clone(),
            pdf_file: detail.pdf_file.clone(),
            gem_file: detail.gem_file.clone(),
            shape_category: detail.shape_category.clone(),
            angle_settings: detail.angle_settings_table.clone(),
            attached_files: attachments,
            frosting: Vec::new(),
        }
    }

    /// The record the vault's pure search reads (`pleochroma_vault::local::search`).
    ///
    /// Built from the stored entry and detail rows, the ignored mark, the tags (matched by
    /// name) and the timestamps in Unix seconds. `search` borrows a slice of these, so the
    /// page maps its listing once per change, not per keystroke.
    #[must_use]
    pub fn search_record(&self) -> SearchRecord {
        let entry = FacetingDiagramEntry {
            title: self.item.title.clone(),
            url: self.item.url.clone(),
            design_id: self.item.design_id.clone().unwrap_or_default(),
        };
        SearchRecord::from_parts(self.item.id, &entry, &self.detail)
            .with_ignored(self.item.ignored)
            .with_tags(&self.tags)
            .with_timestamps(
                Some(ms_to_unix_seconds(self.added_ms)),
                Some(ms_to_unix_seconds(self.updated_ms)),
            )
    }

    /// Rebuilds [`Self::item`] from the title, url and detail, keeping the id and the
    /// ignored mark. Call it after editing `detail` or the title.
    pub fn refresh_item(&mut self) {
        let keep_id = self.item.id;
        let keep_ignored = self.item.ignored;
        let title = std::mem::take(&mut self.item.title);
        let url = std::mem::take(&mut self.item.url);
        let design_id = self.item.design_id.take();
        self.item = list_item(keep_id, title, url, design_id, &self.detail, keep_ignored);
    }
}

/// Unix seconds of a `Date.now()` value (milliseconds); negative or non-finite reads as `0`.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "a Date.now() value is about 1.8e12, far inside i64, and is floored first"
)]
pub fn ms_to_unix_seconds(ms: f64) -> i64 {
    if ms.is_finite() && ms > 0.0 {
        (ms / 1000.0).floor() as i64
    } else {
        0
    }
}

/// The display row of a detail.
fn list_item(
    id: i64,
    title: String,
    url: String,
    design_id: Option<String>,
    detail: &FacetingDiagramDetail,
    ignored: bool,
) -> DiagramListItem {
    DiagramListItem {
        id,
        title,
        url,
        design_id,
        shape: detail.shape.clone(),
        index_gear: detail.index_gear.clone(),
        facets_count: detail.facets_count.clone(),
        designer_info: detail.designer_info.clone(),
        lw_ratio: detail.lw_ratio.clone(),
        refractive_index: detail.refractive_index.clone(),
        volume: detail.volume.clone(),
        competition_diagram: detail.competition_diagram.clone(),
        ignored,
    }
}

/// Tags trimmed, empty ones dropped, de-duplicated and sorted.
#[must_use]
pub fn normalize_tags<I, S>(tags: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    tags.into_iter()
        .map(|tag| tag.as_ref().trim().to_owned())
        .filter(|tag| !tag.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The attachments of an entry: the bytes beside a summary record.
pub type StoredFiles = Vec<AttachedFile>;

/// The attachment that is the design file (the vault's rule: a native design first, then
/// `.asc`, `.gem`, `.gcs`).
#[must_use]
pub fn design_attachment(files: &[AttachedFile]) -> Option<&AttachedFile> {
    primary_design_attachment(files)
}

/// What importing one parsed file into the library comes to.
#[derive(Debug, Clone)]
pub enum ImportPlan {
    /// The library already holds exactly this file under its url: write nothing.
    Identical {
        /// The existing entry's design id.
        design_id: String,
    },
    /// Write this entry and these attachments.
    Store(Box<StoreImport>),
}

/// The payload of [`ImportPlan::Store`].
#[derive(Debug, Clone)]
pub struct StoreImport {
    /// The entry to write (`item.id` is `0` for a new entry).
    pub entry: LibraryEntry,
    /// The attachments to write beside it.
    pub attachments: StoredFiles,
    /// An entry with this url existed and is being replaced (its id, tags and ignored mark
    /// are kept; hand-entered metadata is merged by the vault rule).
    pub replaced: bool,
}

/// Settles a parsed import against the entry already stored under its url.
///
/// `meta_id` is the design file's `[meta] id` (the web page parses it from the design);
/// a new entry is keyed by it, else by the url's UUID. An existing entry keeps its id so its
/// side data stays attached. `existing` is that entry with its attachments; with
/// `skip_identical`, an entry holding the same design file bytes ends the import.
///
/// The tags and marks of the file are added to an existing entry's (union of tags, ignored
/// and planner marks stay set), never removed.
#[must_use]
pub fn plan_import(
    parsed: ImportedAsc,
    meta_id: Option<&str>,
    existing: Option<(&LibraryEntry, &[AttachedFile])>,
    now_ms: f64,
    skip_identical: bool,
) -> ImportPlan {
    let ImportedAsc {
        entry,
        mut detail,
        extras,
        ..
    } = parsed;
    let record = existing.map(|(old, files)| old.full_record(files.to_vec()));
    let row = reconcile_with_existing(&mut detail, record.as_ref(), skip_identical);
    if let (ExistingRow::Identical, Some((old, _))) = (row, existing) {
        return ImportPlan::Identical {
            design_id: old.design_id.clone(),
        };
    }
    let attachments = std::mem::take(&mut detail.attached_files);
    detail.diagram_image_data = None;
    let design_file = design_attachment(&attachments).map(|file| file.name.clone());

    let (design_id, id, added_ms, tags, ignored, planner_excluded) = match existing {
        Some((old, _)) => (
            old.design_id.clone(),
            old.item.id,
            old.added_ms,
            normalize_tags(old.tags.iter().chain(&extras.tags)),
            old.item.ignored || extras.ignored,
            old.planner_excluded || extras.planner_excluded,
        ),
        None => (
            design_id_for(&entry.url, meta_id),
            0,
            now_ms,
            normalize_tags(&extras.tags),
            extras.ignored,
            extras.planner_excluded,
        ),
    };
    let item_design_id = (!entry.design_id.is_empty()).then(|| entry.design_id.clone());
    let item = list_item(id, entry.title, entry.url, item_design_id, &detail, ignored);
    ImportPlan::Store(Box::new(StoreImport {
        entry: LibraryEntry {
            version: RECORD_VERSION,
            design_id,
            item,
            detail,
            tags,
            planner_excluded,
            added_ms,
            updated_ms: now_ms,
            design_file,
        },
        attachments,
        replaced: row == ExistingRow::Replaced,
    }))
}

/// The entry that already owns the url an import is about to write onto `existing`, when it is
/// a different entry.
///
/// `existing` is the entry the import replaces (found by the file's `[meta]` id), `by_url` the
/// entry the url index returns for the file. Both present with different design ids means the
/// replacement would leave two entries on one url, which the url lookup cannot tell apart; the
/// caller refuses the import. With no `existing`, `by_url` is simply the entry to replace.
#[must_use]
pub fn url_taken_by_other<'a>(
    existing: Option<&LibraryEntry>,
    by_url: Option<&'a LibraryEntry>,
) -> Option<&'a LibraryEntry> {
    let (old, owner) = (existing?, by_url?);
    (old.design_id != owner.design_id).then_some(owner)
}

// ---------------------------------------------------------------------------------------
// Thumbnails
// ---------------------------------------------------------------------------------------

/// A cached PNG thumbnail with the stamp of what it was rendered from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    /// The PNG file bytes.
    pub png: Vec<u8>,
    /// Text the page chooses (a hash or generation of the design it drew); a thumbnail whose
    /// stamp differs from the design's current one is stale.
    pub stamp: String,
}

/// Refuses a thumbnail that is not a PNG or is too large to be worth the quota.
///
/// # Errors
///
/// `png` is empty, larger than [`MAX_THUMBNAIL_BYTES`], or lacks the PNG signature.
pub fn check_thumbnail(png: &[u8]) -> Result<(), StoreError> {
    if png.is_empty() {
        return Err(StoreError::invalid("the thumbnail is empty"));
    }
    if png.len() > MAX_THUMBNAIL_BYTES {
        return Err(StoreError::invalid(format!(
            "the thumbnail is {} bytes; the limit is {MAX_THUMBNAIL_BYTES}",
            png.len()
        )));
    }
    if !png.starts_with(&PNG_SIGNATURE) {
        return Err(StoreError::invalid("the thumbnail is not a PNG"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Side data
// ---------------------------------------------------------------------------------------

/// A stored variant (the design text is kept here, the thumbnail beside it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantRecord {
    /// The variant's id (from the store's counter).
    pub variant_id: i64,
    /// The cutter's name.
    pub name: String,
    /// The variant this one was made from.
    pub parent_variant_id: Option<i64>,
    /// When it was made, Unix seconds.
    pub created_at: i64,
    /// The note, if any.
    pub note: Option<String>,
    /// The design as `.psdd` text.
    pub design_text: String,
}

impl VariantRecord {
    /// The record of `new` under `variant_id`, with the name and note trimmed.
    ///
    /// # Errors
    ///
    /// The trimmed name is empty, or the design text is.
    pub fn from_new(new: &NewVariant<'_>, variant_id: i64) -> Result<Self, StoreError> {
        let name = new.name.trim();
        if name.is_empty() {
            return Err(StoreError::invalid("a variant needs a name"));
        }
        if new.design_text.trim().is_empty() {
            return Err(StoreError::invalid("a variant needs the design text"));
        }
        Ok(Self {
            variant_id,
            name: name.to_owned(),
            parent_variant_id: new.parent_variant_id,
            created_at: new.created_at,
            note: clean_note(new.note),
            design_text: new.design_text.to_owned(),
        })
    }

    /// The list row of this record for `design_id`.
    #[must_use]
    pub fn summary(&self, design_id: &str) -> VariantSummary {
        VariantSummary {
            variant_id: self.variant_id,
            design_uuid: design_id.to_owned(),
            name: self.name.clone(),
            parent_variant_id: self.parent_variant_id,
            created_at: self.created_at,
            note: self.note.clone(),
        }
    }

    /// The variant with its design text.
    #[must_use]
    pub fn into_variant(self, design_id: &str) -> DesignVariant {
        DesignVariant {
            summary: VariantSummary {
                variant_id: self.variant_id,
                design_uuid: design_id.to_owned(),
                name: self.name,
                parent_variant_id: self.parent_variant_id,
                created_at: self.created_at,
                note: self.note,
            },
            design_text: self.design_text,
        }
    }
}

/// A note trimmed; blank means none.
#[must_use]
pub fn clean_note(note: Option<&str>) -> Option<String> {
    note.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// The variant summaries in id order, a parent that is not in the list read as `None`
/// (the vault's rule for a variant whose parent was deleted).
#[must_use]
pub fn variant_summaries(design_id: &str, mut records: Vec<VariantRecord>) -> Vec<VariantSummary> {
    records.sort_by_key(|record| record.variant_id);
    let known: BTreeSet<i64> = records.iter().map(|record| record.variant_id).collect();
    records
        .iter()
        .map(|record| {
            let mut summary = record.summary(design_id);
            summary.parent_variant_id = summary
                .parent_variant_id
                .filter(|parent| known.contains(parent));
            summary
        })
        .collect()
}

/// One step marked done, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkRecord {
    /// The tier (or concave tier) the step cuts.
    pub step_key: String,
    /// The fingerprint of the step's values when it was marked.
    pub step_signature: String,
    /// When it was marked, Unix seconds.
    pub done_at: i64,
}

impl From<&CutProgressMark> for MarkRecord {
    fn from(mark: &CutProgressMark) -> Self {
        Self {
            step_key: mark.step_key.clone(),
            step_signature: mark.step_signature.clone(),
            done_at: mark.done_at,
        }
    }
}

impl From<MarkRecord> for CutProgressMark {
    fn from(record: MarkRecord) -> Self {
        Self {
            step_key: record.step_key,
            step_signature: record.step_signature,
            done_at: record.done_at,
        }
    }
}

/// Marks a step done: replaces the mark with the same step key, else appends.
///
/// # Errors
///
/// The step key is blank.
pub fn apply_mark(marks: &mut Vec<MarkRecord>, mark: &CutProgressMark) -> Result<(), StoreError> {
    if mark.step_key.trim().is_empty() {
        return Err(StoreError::invalid("a step mark needs a step key"));
    }
    let record = MarkRecord::from(mark);
    match marks.iter_mut().find(|m| m.step_key == record.step_key) {
        Some(existing) => *existing = record,
        None => marks.push(record),
    }
    Ok(())
}

/// Removes the mark of `step_key`; `true` when there was one.
pub fn remove_mark(marks: &mut Vec<MarkRecord>, step_key: &str) -> bool {
    let before = marks.len();
    marks.retain(|m| m.step_key != step_key);
    marks.len() != before
}

/// A design's lighting choice, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LightingRecord {
    /// The preset the settings came from.
    pub preset_name: String,
    /// The settings text the page understands.
    pub settings_json: String,
    /// When it was stored, Unix seconds.
    pub updated_at: i64,
}

impl From<&DesignLighting> for LightingRecord {
    fn from(lighting: &DesignLighting) -> Self {
        Self {
            preset_name: lighting.preset_name.clone(),
            settings_json: lighting.settings_json.clone(),
            updated_at: lighting.updated_at,
        }
    }
}

impl From<LightingRecord> for DesignLighting {
    fn from(record: LightingRecord) -> Self {
        Self {
            preset_name: record.preset_name,
            settings_json: record.settings_json,
            updated_at: record.updated_at,
        }
    }
}

// ---------------------------------------------------------------------------------------
// Storage estimate
// ---------------------------------------------------------------------------------------

/// What `navigator.storage.estimate()` reported.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StorageEstimate {
    /// Bytes this origin uses.
    pub usage_bytes: f64,
    /// Bytes the browser lets this origin use (an estimate; browsers round it).
    pub quota_bytes: f64,
}

impl StorageEstimate {
    /// The used share of the quota, `0.0` while the quota is unknown.
    #[must_use]
    pub fn fraction_used(&self) -> f64 {
        if self.quota_bytes > 0.0 {
            (self.usage_bytes / self.quota_bytes).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Whether the origin uses at least nine tenths of its quota.
    #[must_use]
    pub fn is_nearly_full(&self) -> bool {
        self.fraction_used() >= NEARLY_FULL_FRACTION
    }

    /// The bytes still free.
    #[must_use]
    pub fn bytes_free(&self) -> f64 {
        (self.quota_bytes - self.usage_bytes).max(0.0)
    }
}

#[cfg(test)]
mod tests;
