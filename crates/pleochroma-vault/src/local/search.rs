//! A pure search/filter/sort engine over in-memory library summary records.
//!
//! It has the same semantics as the SQLite search (`db::sqlite::search`, built by its
//! `build_search_predicate`), without `rusqlite`, threads or the filesystem, so it builds for
//! `wasm32-unknown-unknown` and runs over whatever a browser store holds. The pieces:
//!
//! - [`SearchRecord`]: the fields the filters read, with [`SearchRecord::from_parts`] and
//!   `From<&FullDiagramRecord>` to build one from the vault model types.
//! - [`fold`]: the case, curly-quote and dash folding the free-text title and designer match
//!   goes through (a copy of the SQL `fold()` function).
//! - [`search`] and [`matching_ids`]: the filtered, sorted, capped page and the uncapped id
//!   walk.
//!
//! # Semantics shared with the SQL search
//!
//! - Free text is a literal substring match (`%`, `_` and `\` are ordinary characters). The
//!   title, the designer text and the split designer name compare through [`fold`]; the design
//!   id and each tier's notes compare ASCII-case-insensitively only, like a plain SQLite `LIKE`.
//! - The shape filter is an exact, case-sensitive comparison. The gear filter compares numbers.
//! - A range bound on an attribute a record does not have excludes the record.
//! - Ignored designs are excluded unless [`RangeFilter::include_ignored`].
//! - "My designs" is a `local://` url prefix (ASCII-case-insensitive, like the SQL `LIKE`).
//!
//! # Differences from the SQL search
//!
//! - There are no stored tilt curves here (guide decision D16). A non-empty
//!   [`RangeFilter::performance`] therefore matches nothing, and
//!   [`SearchOutcome::excluded_for_missing_curves`] counts the designs that passed every other
//!   filter, exactly what the SQL search reports for a library without curves.
//! - A numeric attribute whose stored text is not a number reads as absent
//!   ([`SearchRecord::from_parts`]); SQLite would keep the text and order it after every number.
//! - Tags are matched by name (ASCII-case-insensitive, the `tags.name` collation), not by id.

use crate::model::{
    angle::AngleSetting,
    detail::FacetingDiagramDetail,
    entry::{FacetingDiagramEntry, FullDiagramRecord},
    facets::parse_facets_count,
    filter::RangeFilter,
};
use std::collections::HashSet;

/// The most records [`search`] returns in [`SearchOutcome::items`].
///
/// The same value as the SQL search's `SEARCH_RESULT_CAP`.
pub const SEARCH_RESULT_CAP: usize = 1000;

/// The url scheme every locally imported design carries (`local://<file name>`).
const LOCAL_URL_PREFIX: &str = "local://";

/// Sort order of a [`search`] result.
///
/// The same four orders as the SQL search's `SortOrder`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    /// Entry id ascending (insertion order).
    #[default]
    CatalogueOrder,
    /// Title A-Z, ASCII-case-insensitive, then id.
    Title,
    /// Most recently created first; a record without `created_at` sorts last.
    Newest,
    /// Most recently edited first; a record without `updated_at` sorts last.
    RecentlyEdited,
}

/// The sort and restriction options of a [`search`] call.
///
/// Mirrors the SQL search's `DisplayFilters`, except that the tag is named, not numbered.
#[derive(Debug, Clone, Copy, Default)]
pub struct SearchOptions<'a> {
    /// Sort order.
    pub order: SortOrder,
    /// Keep only designs whose url starts with `local://` ("My designs").
    pub local_only: bool,
    /// Keep only designs carrying this tag (compared ASCII-case-insensitively); `None` for no
    /// restriction.
    pub tag_filter: Option<&'a str>,
    /// Keep only these entry ids; `None` or empty for no restriction.
    pub id_filter: Option<&'a [i64]>,
}

/// One design as the search reads it: identity, the filterable attributes and its side data.
///
/// Numeric attributes are parsed numbers (the SQL columns are numeric); `None` means the
/// design does not state the attribute. A design with no detail row is a record with every
/// attribute `None` (`SearchRecord::default()` plus an id, title and url).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchRecord {
    /// The entry id (`diagram_entries.id`, or the browser store's key).
    pub id: i64,
    /// The display title.
    pub title: String,
    /// The identity url (`local://<file name>` for a local design).
    pub url: String,
    /// The scraped source's design-id text.
    pub design_id: Option<String>,
    /// The shape name.
    pub shape: Option<String>,
    /// The gear-tooth count.
    pub index_gear: Option<i64>,
    /// The main facet count (the part of `"55+6"` before the girdle count).
    pub facets: Option<i64>,
    /// The refractive index.
    pub refractive_index: Option<f64>,
    /// The length/width ratio.
    pub lw_ratio: Option<f64>,
    /// The volume.
    pub volume: Option<f64>,
    /// The free-text `"Designer; citation"` string.
    pub designer_info: Option<String>,
    /// The designer name alone.
    pub designer: Option<String>,
    /// How many tiers are concave (tool-cut); `0` for a planar design.
    pub concave_tiers: u32,
    /// The owner marked the design ignored.
    pub ignored: bool,
    /// Each tier's free-text notes.
    pub notes: Vec<String>,
    /// The names of the tags the design carries.
    pub tags: Vec<String>,
    /// Creation time (any monotonic unit; seconds in the SQL store), `None` if unknown.
    pub created_at: Option<i64>,
    /// Last-edit time, `None` if unknown.
    pub updated_at: Option<i64>,
}

/// The text fields of a detail row, borrowed, for [`SearchRecord::assemble`].
struct DetailText<'a> {
    shape: Option<&'a str>,
    designer_info: Option<&'a str>,
    designer: Option<&'a str>,
    refractive_index: Option<&'a str>,
    lw_ratio: Option<&'a str>,
    volume: Option<&'a str>,
    index_gear: Option<&'a str>,
    facets_count: Option<&'a str>,
    angle_settings: &'a [AngleSetting],
    concave_tiers: u32,
}

impl SearchRecord {
    /// A record for entry `id` from its `entry` and `detail` rows.
    ///
    /// The numeric text fields are parsed like the SQL columns read them: surrounding
    /// whitespace is ignored, text that is not a number (a European `"1,76"`) is absent, the
    /// facet count is the main part of `"55+6"` ([`parse_facets_count`]), and a gear written
    /// `"96.0"` is 96. `ignored`, tags and timestamps start empty; set them with
    /// [`Self::with_ignored`], [`Self::with_tags`] and [`Self::with_timestamps`].
    #[must_use]
    pub fn from_parts(
        id: i64,
        entry: &FacetingDiagramEntry,
        detail: &FacetingDiagramDetail,
    ) -> Self {
        Self::assemble(
            id,
            &entry.title,
            &entry.url,
            Some(entry.design_id.as_str()),
            &DetailText {
                shape: detail.shape.as_deref(),
                designer_info: detail.designer_info.as_deref(),
                designer: detail.designer.as_deref(),
                refractive_index: detail.refractive_index.as_deref(),
                lw_ratio: detail.lw_ratio.as_deref(),
                volume: detail.volume.as_deref(),
                index_gear: detail.index_gear.as_deref(),
                facets_count: detail.facets_count.as_deref(),
                angle_settings: &detail.angle_settings_table,
                concave_tiers: detail.concave_tiers,
            },
        )
    }

    /// This record marked ignored (or not).
    #[must_use]
    pub const fn with_ignored(mut self, ignored: bool) -> Self {
        self.ignored = ignored;
        self
    }

    /// This record carrying the tags `tags` (names).
    #[must_use]
    pub fn with_tags<S: AsRef<str>>(mut self, tags: &[S]) -> Self {
        self.tags = tags.iter().map(|t| t.as_ref().to_owned()).collect();
        self
    }

    /// This record with its creation and last-edit times.
    #[must_use]
    pub const fn with_timestamps(
        mut self,
        created_at: Option<i64>,
        updated_at: Option<i64>,
    ) -> Self {
        self.created_at = created_at;
        self.updated_at = updated_at;
        self
    }

    fn assemble(
        id: i64,
        title: &str,
        url: &str,
        design_id: Option<&str>,
        detail: &DetailText<'_>,
    ) -> Self {
        Self {
            id,
            title: title.to_owned(),
            url: url.to_owned(),
            design_id: design_id.filter(|d| !d.is_empty()).map(str::to_owned),
            shape: detail.shape.map(str::to_owned),
            index_gear: detail.index_gear.and_then(parse_integer),
            facets: parse_facets_count(detail.facets_count).0,
            refractive_index: parse_real(detail.refractive_index),
            lw_ratio: parse_real(detail.lw_ratio),
            volume: parse_real(detail.volume),
            designer_info: detail.designer_info.map(str::to_owned),
            designer: detail.designer.map(str::to_owned),
            concave_tiers: detail.concave_tiers,
            ignored: false,
            notes: detail
                .angle_settings
                .iter()
                .map(|a| a.notes.clone())
                .collect(),
            tags: Vec::new(),
            created_at: None,
            updated_at: None,
        }
    }
}

/// A record from a full record read back from the store.
///
/// A full record does not carry its concave tier count, so it is the number of tiers with a
/// tool; ignored, tags and timestamps start empty, like [`SearchRecord::from_parts`].
impl From<&FullDiagramRecord> for SearchRecord {
    fn from(record: &FullDiagramRecord) -> Self {
        let concave = record
            .angle_settings
            .iter()
            .filter(|a| a.tool.is_some())
            .count();
        Self::assemble(
            record.entry_id,
            &record.title,
            &record.url,
            record.design_id.as_deref(),
            &DetailText {
                shape: record.shape.as_deref(),
                designer_info: record.designer_info.as_deref(),
                designer: record.designer.as_deref(),
                refractive_index: record.refractive_index.as_deref(),
                lw_ratio: record.lw_ratio.as_deref(),
                volume: record.volume.as_deref(),
                index_gear: record.index_gear.as_deref(),
                facets_count: record.facets_count.as_deref(),
                angle_settings: &record.angle_settings,
                concave_tiers: u32::try_from(concave).unwrap_or(u32::MAX),
            },
        )
    }
}

/// The result of a [`search`].
#[derive(Debug, Clone)]
pub struct SearchOutcome<'a> {
    /// The matching records in the requested order, at most [`SEARCH_RESULT_CAP`].
    pub items: Vec<&'a SearchRecord>,
    /// How many records match in total, uncapped.
    pub total: usize,
    /// How many records matched every filter except [`RangeFilter::performance`] but were
    /// dropped because a performance filter is active and no curves exist; `0` otherwise.
    pub excluded_for_missing_curves: usize,
}

/// Folds `text` for a typography- and case-insensitive substring comparison.
///
/// It lowercases (Unicode-aware) and maps the curly quotes U+2018/U+2019 to `'`, U+201C/U+201D
/// to `"` and the en and em dashes U+2013/U+2014 to `-`. Not a normalization: the list is the
/// small fixed one of the SQL `fold()` function, and `%`, `_` and `\` pass through unchanged.
#[must_use]
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        let mapped = match ch {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201c}' | '\u{201d}' => '"',
            '\u{2013}' | '\u{2014}' => '-',
            other => other,
        };
        out.extend(mapped.to_lowercase());
    }
    out
}

/// Whether `url` is a locally imported design's (`local://` prefix, ASCII-case-insensitive).
#[must_use]
pub fn is_local_url(url: &str) -> bool {
    url.get(..LOCAL_URL_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(LOCAL_URL_PREFIX))
}

/// The records matching the filters, sorted and capped.
///
/// `query` is trimmed; empty means no text filter. `shape_filter` and `gear_filter` are
/// ignored when empty or `"All"`. See the module docs for the semantics.
#[must_use]
pub fn search<'a>(
    records: &'a [SearchRecord],
    query: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilter,
    options: SearchOptions<'_>,
) -> SearchOutcome<'a> {
    let filter = Filter::new(query, shape_filter, gear_filter, range, options);
    let mut matches: Vec<&SearchRecord> = records.iter().filter(|r| filter.accepts(r)).collect();
    if !range.performance.is_empty() {
        return SearchOutcome {
            items: Vec::new(),
            total: 0,
            excluded_for_missing_curves: matches.len(),
        };
    }
    sort_matches(&mut matches, options.order);
    let total = matches.len();
    matches.truncate(SEARCH_RESULT_CAP);
    SearchOutcome {
        items: matches,
        total,
        excluded_for_missing_curves: 0,
    }
}

/// Every matching entry id in catalogue order, with no [`SEARCH_RESULT_CAP`] truncation.
///
/// The sort in `options` is ignored, like the SQL search's id walk. Empty when a performance
/// filter is active.
#[must_use]
pub fn matching_ids(
    records: &[SearchRecord],
    query: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilter,
    options: SearchOptions<'_>,
) -> Vec<i64> {
    if !range.performance.is_empty() {
        return Vec::new();
    }
    let filter = Filter::new(query, shape_filter, gear_filter, range, options);
    let mut ids: Vec<i64> = records
        .iter()
        .filter(|r| filter.accepts(r))
        .map(|r| r.id)
        .collect();
    ids.sort_unstable();
    ids
}

fn sort_matches(matches: &mut [&SearchRecord], order: SortOrder) {
    match order {
        SortOrder::CatalogueOrder => matches.sort_by_key(|r| r.id),
        // SQLite's NOCASE folds ASCII only and compares bytes, like this.
        SortOrder::Title => matches.sort_by_cached_key(|r| (r.title.to_ascii_lowercase(), r.id)),
        // `None < Some`, so the reversed comparison puts a missing time last, like SQL `DESC`.
        SortOrder::Newest => {
            matches.sort_by(|a, b| {
                b.created_at
                    .cmp(&a.created_at)
                    .then_with(|| b.id.cmp(&a.id))
            });
        }
        SortOrder::RecentlyEdited => {
            matches.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then_with(|| b.id.cmp(&a.id))
            });
        }
    }
}

/// The gear filter, resolved once.
#[derive(Debug, Clone, Copy)]
enum GearFilter {
    Any,
    Exactly(i64),
    /// Text that is not a number: no design has such a gear.
    Nothing,
}

/// The free-text needle in the two forms the columns compare in.
struct Needle {
    folded: String,
    ascii_lower: String,
}

/// Everything [`search`] filters on, resolved once per call.
struct Filter<'a> {
    needle: Option<Needle>,
    shape: Option<&'a str>,
    gear: GearFilter,
    range: &'a RangeFilter,
    local_only: bool,
    tag: Option<&'a str>,
    ids: Option<HashSet<i64>>,
}

impl<'a> Filter<'a> {
    fn new(
        query: &str,
        shape_filter: &'a str,
        gear_filter: &str,
        range: &'a RangeFilter,
        options: SearchOptions<'a>,
    ) -> Self {
        let query = query.trim();
        let needle = (!query.is_empty()).then(|| Needle {
            folded: fold(query),
            ascii_lower: query.to_ascii_lowercase(),
        });
        let gear = if gear_filter.is_empty() || gear_filter == "All" {
            GearFilter::Any
        } else {
            parse_integer(gear_filter).map_or(GearFilter::Nothing, GearFilter::Exactly)
        };
        Self {
            needle,
            shape: (!shape_filter.is_empty() && shape_filter != "All").then_some(shape_filter),
            gear,
            range,
            local_only: options.local_only,
            tag: options.tag_filter.map(str::trim),
            ids: options
                .id_filter
                .filter(|ids| !ids.is_empty())
                .map(|ids| ids.iter().copied().collect()),
        }
    }

    /// Whether `record` passes every filter except the performance ones.
    fn accepts(&self, record: &SearchRecord) -> bool {
        self.accepts_text(record)
            && self.accepts_attributes(record)
            && self.accepts_ranges(record)
            && (self.range.include_ignored || !record.ignored)
            && (!self.local_only || is_local_url(&record.url))
            && self.tag.is_none_or(|tag| {
                record
                    .tags
                    .iter()
                    .any(|t| t.trim().eq_ignore_ascii_case(tag))
            })
            && self.ids.as_ref().is_none_or(|ids| ids.contains(&record.id))
    }

    fn accepts_text(&self, record: &SearchRecord) -> bool {
        let Some(needle) = &self.needle else {
            return true;
        };
        let folded_hit = |text: &str| fold(text).contains(&needle.folded);
        let ascii_hit = |text: &str| text.to_ascii_lowercase().contains(&needle.ascii_lower);
        folded_hit(&record.title)
            || record.designer_info.as_deref().is_some_and(folded_hit)
            || record.designer.as_deref().is_some_and(folded_hit)
            || record.design_id.as_deref().is_some_and(ascii_hit)
            || record.notes.iter().any(|n| ascii_hit(n))
    }

    fn accepts_attributes(&self, record: &SearchRecord) -> bool {
        if let Some(shape) = self.shape
            && record.shape.as_deref() != Some(shape)
        {
            return false;
        }
        match self.gear {
            GearFilter::Any => true,
            GearFilter::Exactly(gear) => record.index_gear == Some(gear),
            GearFilter::Nothing => false,
        }
    }

    fn accepts_ranges(&self, record: &SearchRecord) -> bool {
        let range = self.range;
        within(record.refractive_index, range.ri_min, range.ri_max)
            && within(record.lw_ratio, range.lw_min, range.lw_max)
            && within(record.volume, range.volume_min, range.volume_max)
            && within(record.facets, range.facets_min, range.facets_max)
            && range.ri_tolerance.is_none_or(|(center, tolerance)| {
                within(
                    record.refractive_index,
                    Some(center - tolerance),
                    Some(center + tolerance),
                )
            })
            && match range.has_concave {
                Some(true) => record.concave_tiers > 0,
                Some(false) => record.concave_tiers == 0,
                None => true,
            }
    }
}

/// Whether `value` lies in `[min, max]`; an absent value fails any bound that is set.
fn within<T: PartialOrd>(value: Option<T>, min: Option<T>, max: Option<T>) -> bool {
    if min.is_none() && max.is_none() {
        return true;
    }
    let Some(value) = value else {
        return false;
    };
    min.is_none_or(|min| value >= min) && max.is_none_or(|max| value <= max)
}

/// `raw` as a finite number, `None` if blank or not a number.
fn parse_real(raw: Option<&str>) -> Option<f64> {
    let value = raw?.trim().parse::<f64>().ok()?;
    value.is_finite().then_some(value)
}

/// `raw` as a whole number; `"96"` and `"96.0"` are 96, anything else is `None`.
fn parse_integer(raw: &str) -> Option<i64> {
    let text = raw.trim();
    if let Ok(value) = text.parse::<i64>() {
        return Some(value);
    }
    let (whole, fraction) = text.split_once('.')?;
    if fraction.chars().all(|c| c == '0') {
        whole.parse::<i64>().ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::performance::{
        PerformanceAggregate, PerformanceBound, PerformanceFilter, PerformanceMetric,
    };

    fn record(id: i64, title: &str) -> SearchRecord {
        SearchRecord {
            id,
            title: title.to_owned(),
            url: format!("https://example.test/{id}"),
            ..SearchRecord::default()
        }
    }

    fn ids(outcome: &SearchOutcome<'_>) -> Vec<i64> {
        outcome.items.iter().map(|r| r.id).collect()
    }

    fn run(records: &[SearchRecord], query: &str) -> Vec<i64> {
        ids(&search(
            records,
            query,
            "All",
            "All",
            &RangeFilter::default(),
            SearchOptions::default(),
        ))
    }

    fn with_range(records: &[SearchRecord], range: &RangeFilter) -> Vec<i64> {
        ids(&search(
            records,
            "",
            "All",
            "All",
            range,
            SearchOptions::default(),
        ))
    }

    #[test]
    fn fold_lowercases_unicode_and_maps_curly_punctuation() {
        assert_eq!(fold("TORBJ\u{d6}RN"), "torbj\u{f6}rn");
        assert_eq!(fold("Cam\u{2019}s"), "cam's");
        assert_eq!(fold("\u{201c}Q\u{201d}"), "\"q\"");
        assert_eq!(fold("a\u{2013}b\u{2014}c"), "a-b-c");
        assert_eq!(fold("50%_\\"), "50%_\\");
    }

    #[test]
    fn free_text_matches_title_designer_and_split_designer_folded() {
        let mut a = record(1, "Plain");
        a.designer_info = Some("Torbj\u{f6}rn \u{c5}kesson; JGD".to_owned());
        let mut b = record(2, "Other");
        b.designer = Some("Cam\u{2019}s Designs".to_owned());
        let records = [a, b, record(3, "TORBJ\u{d6}RN Star")];
        assert_eq!(run(&records, "torbj\u{f6}rn"), vec![1, 3]);
        assert_eq!(run(&records, "CAM'S"), vec![2]);
        assert_eq!(run(&records, "  \u{e5}kesson "), vec![1]);
        assert_eq!(run(&records, "nobody"), Vec::<i64>::new());
    }

    #[test]
    fn like_wildcards_are_literal() {
        let records = [
            record(1, "Round_Brilliant"),
            record(2, "Round Brilliant"),
            record(3, "50% Off"),
            record(4, "back\\slash"),
        ];
        assert_eq!(run(&records, "_"), vec![1]);
        assert_eq!(run(&records, "%"), vec![3]);
        assert_eq!(run(&records, "50%"), vec![3]);
        assert_eq!(run(&records, "\\"), vec![4]);
    }

    #[test]
    fn design_id_and_notes_match_ascii_case_insensitively_but_not_folded() {
        let mut a = record(1, "A");
        a.design_id = Some("FD-0042".to_owned());
        let mut b = record(2, "B");
        b.notes = vec![String::new(), "Client HEIRLOOM spec".to_owned()];
        let mut c = record(3, "C");
        c.notes = vec!["Caf\u{c9}".to_owned()];
        let records = [a, b, c];
        assert_eq!(run(&records, "fd-0042"), vec![1]);
        assert_eq!(run(&records, "heirloom"), vec![2]);
        // Notes are not folded: a non-ASCII capital does not match its lowercase.
        assert_eq!(run(&records, "caf\u{e9}"), Vec::<i64>::new());
        assert_eq!(run(&records, "Caf\u{c9}"), vec![3]);
    }

    #[test]
    fn shape_and_gear_filters() {
        let mut a = record(1, "A");
        a.shape = Some("Round".to_owned());
        a.index_gear = Some(96);
        let mut b = record(2, "B");
        b.shape = Some("round".to_owned());
        b.index_gear = Some(64);
        let records = [a, b, record(3, "C")];
        let go = |shape: &str, gear: &str| {
            ids(&search(
                &records,
                "",
                shape,
                gear,
                &RangeFilter::default(),
                SearchOptions::default(),
            ))
        };
        assert_eq!(go("Round", "All"), vec![1]);
        assert_eq!(go("All", "96"), vec![1]);
        assert_eq!(go("", " 64 "), vec![2]);
        assert_eq!(go("", "96.0"), vec![1]);
        assert_eq!(go("", "abc"), Vec::<i64>::new());
        assert_eq!(go("", ""), vec![1, 2, 3]);
    }

    #[test]
    fn numeric_ranges_exclude_records_without_the_attribute() {
        let mut a = record(1, "A");
        a.refractive_index = Some(1.54);
        a.lw_ratio = Some(1.0);
        a.volume = Some(0.4);
        a.facets = Some(57);
        let mut b = record(2, "B");
        b.refractive_index = Some(1.76);
        b.lw_ratio = Some(1.5);
        b.volume = Some(0.9);
        b.facets = Some(80);
        let records = [a, b, record(3, "C")];
        let none = RangeFilter::default();
        assert_eq!(with_range(&records, &none), vec![1, 2, 3]);
        let ri = RangeFilter {
            ri_min: Some(1.54),
            ri_max: Some(1.6),
            ..RangeFilter::default()
        };
        assert_eq!(with_range(&records, &ri), vec![1]);
        let lw = RangeFilter {
            lw_min: Some(1.2),
            ..RangeFilter::default()
        };
        assert_eq!(with_range(&records, &lw), vec![2]);
        let volume = RangeFilter {
            volume_max: Some(0.5),
            ..RangeFilter::default()
        };
        assert_eq!(with_range(&records, &volume), vec![1]);
        let facets = RangeFilter {
            facets_min: Some(60),
            facets_max: Some(80),
            ..RangeFilter::default()
        };
        assert_eq!(with_range(&records, &facets), vec![2]);
        let tolerance = RangeFilter {
            ri_tolerance: Some((1.75, 0.02)),
            ..RangeFilter::default()
        };
        assert_eq!(with_range(&records, &tolerance), vec![2]);
        let both = RangeFilter {
            ri_min: Some(1.7),
            ri_tolerance: Some((1.54, 0.01)),
            ..RangeFilter::default()
        };
        assert_eq!(with_range(&records, &both), Vec::<i64>::new());
    }

    #[test]
    fn concave_filter_counts_a_missing_detail_as_planar() {
        let mut a = record(1, "A");
        a.concave_tiers = 2;
        let records = [a, record(2, "B")];
        let concave = RangeFilter {
            has_concave: Some(true),
            ..RangeFilter::default()
        };
        let planar = RangeFilter {
            has_concave: Some(false),
            ..RangeFilter::default()
        };
        assert_eq!(with_range(&records, &concave), vec![1]);
        assert_eq!(with_range(&records, &planar), vec![2]);
    }

    #[test]
    fn ignored_designs_are_hidden_unless_opted_in() {
        let records = [record(1, "A").with_ignored(true), record(2, "B")];
        assert_eq!(with_range(&records, &RangeFilter::default()), vec![2]);
        let all = RangeFilter {
            include_ignored: true,
            ..RangeFilter::default()
        };
        assert_eq!(with_range(&records, &all), vec![1, 2]);
    }

    #[test]
    fn mine_tag_and_id_restrictions() {
        let mut a = record(1, "A").with_tags(&["Competition"]);
        a.url = "local://a.asc".to_owned();
        let mut b = record(2, "B").with_tags(&["favorite", " competition "]);
        b.url = "LOCAL://b.asc".to_owned();
        let c = record(3, "C");
        let records = [a, b, c];
        let go = |options: SearchOptions<'_>| {
            ids(&search(
                &records,
                "",
                "All",
                "All",
                &RangeFilter::default(),
                options,
            ))
        };
        assert_eq!(
            go(SearchOptions {
                local_only: true,
                ..SearchOptions::default()
            }),
            vec![1, 2]
        );
        assert_eq!(
            go(SearchOptions {
                tag_filter: Some("COMPETITION"),
                ..SearchOptions::default()
            }),
            vec![1, 2]
        );
        assert_eq!(
            go(SearchOptions {
                tag_filter: Some("nothing"),
                ..SearchOptions::default()
            }),
            Vec::<i64>::new()
        );
        assert_eq!(
            go(SearchOptions {
                id_filter: Some(&[3, 1]),
                ..SearchOptions::default()
            }),
            vec![1, 3]
        );
        assert_eq!(
            go(SearchOptions {
                id_filter: Some(&[]),
                ..SearchOptions::default()
            }),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn is_local_url_is_a_case_insensitive_prefix() {
        assert!(is_local_url("local://x.asc"));
        assert!(is_local_url("Local://x.asc"));
        assert!(!is_local_url("https://local://"));
        assert!(!is_local_url("loc"));
        assert!(!is_local_url(""));
    }

    #[test]
    fn sort_orders() {
        let records = [
            record(1, "b").with_timestamps(Some(10), Some(50)),
            record(2, "A").with_timestamps(None, None),
            record(3, "B").with_timestamps(Some(30), Some(50)),
            record(4, "\u{c9}clat").with_timestamps(Some(30), Some(70)),
            record(5, "a").with_timestamps(Some(20), None),
        ];
        let go = |order: SortOrder| {
            ids(&search(
                &records,
                "",
                "All",
                "All",
                &RangeFilter::default(),
                SearchOptions {
                    order,
                    ..SearchOptions::default()
                },
            ))
        };
        assert_eq!(go(SortOrder::CatalogueOrder), vec![1, 2, 3, 4, 5]);
        // NOCASE folds ASCII only, so "Eclat" with an accent sorts after every ASCII title.
        assert_eq!(go(SortOrder::Title), vec![2, 5, 1, 3, 4]);
        assert_eq!(go(SortOrder::Newest), vec![4, 3, 5, 1, 2]);
        assert_eq!(go(SortOrder::RecentlyEdited), vec![4, 3, 1, 5, 2]);
    }

    #[test]
    fn results_are_capped_but_the_total_and_id_walk_are_not() {
        let records: Vec<SearchRecord> = (1..=1203).map(|i| record(i, "Same")).collect();
        let outcome = search(
            &records,
            "",
            "All",
            "All",
            &RangeFilter::default(),
            SearchOptions::default(),
        );
        assert_eq!(outcome.items.len(), SEARCH_RESULT_CAP);
        assert_eq!(outcome.total, 1203);
        assert_eq!(outcome.items[0].id, 1);
        let all = matching_ids(
            &records,
            "",
            "All",
            "All",
            &RangeFilter::default(),
            SearchOptions::default(),
        );
        assert_eq!(all.len(), 1203);
    }

    #[test]
    fn a_performance_filter_matches_nothing_and_counts_the_exclusions() {
        let records = [
            record(1, "Round A"),
            record(2, "Round B"),
            record(3, "Oval"),
        ];
        let filter = PerformanceFilter::new(
            PerformanceMetric::Brilliance,
            PerformanceBound::AtLeast(50.0),
            30.0,
            PerformanceAggregate::Worst,
        )
        .expect("a valid radius");
        let range = RangeFilter {
            performance: vec![filter],
            ..RangeFilter::default()
        };
        let outcome = search(
            &records,
            "round",
            "All",
            "All",
            &range,
            SearchOptions::default(),
        );
        assert!(outcome.items.is_empty());
        assert_eq!(outcome.total, 0);
        assert_eq!(outcome.excluded_for_missing_curves, 2);
        assert!(
            matching_ids(
                &records,
                "round",
                "All",
                "All",
                &range,
                SearchOptions::default()
            )
            .is_empty()
        );
    }

    #[test]
    fn from_parts_parses_the_text_fields_like_the_sql_columns() {
        let entry = FacetingDiagramEntry {
            title: "Heart".to_owned(),
            url: "local://heart.asc".to_owned(),
            design_id: String::new(),
        };
        let detail = FacetingDiagramDetail {
            shape: Some("Heart".to_owned()),
            refractive_index: Some(" 1.76 ".to_owned()),
            lw_ratio: Some("1,2".to_owned()),
            volume: Some(String::new()),
            index_gear: Some("96.0".to_owned()),
            facets_count: Some("55+6".to_owned()),
            angle_settings_table: vec![
                AngleSetting {
                    notes: "first".to_owned(),
                    ..AngleSetting::default()
                },
                AngleSetting {
                    notes: "second".to_owned(),
                    tool: Some("Ball".to_owned()),
                    ..AngleSetting::default()
                },
            ],
            concave_tiers: 1,
            ..FacetingDiagramDetail::default()
        };
        let record = SearchRecord::from_parts(7, &entry, &detail)
            .with_ignored(true)
            .with_tags(&["x"])
            .with_timestamps(Some(1), Some(2));
        assert_eq!(record.id, 7);
        assert_eq!(record.design_id, None);
        assert_eq!(record.refractive_index, Some(1.76));
        assert_eq!(record.lw_ratio, None);
        assert_eq!(record.volume, None);
        assert_eq!(record.index_gear, Some(96));
        assert_eq!(record.facets, Some(55));
        assert_eq!(record.notes, vec!["first", "second"]);
        assert_eq!(record.concave_tiers, 1);
        assert!(record.ignored);
        assert_eq!(record.tags, vec!["x"]);
        assert_eq!((record.created_at, record.updated_at), (Some(1), Some(2)));
    }

    #[test]
    fn a_full_record_converts_with_its_tool_tiers_as_the_concave_count() {
        let full = FullDiagramRecord {
            entry_id: 9,
            title: "Full".to_owned(),
            url: "local://full.asc".to_owned(),
            design_id: Some("FD-1".to_owned()),
            page_url: String::new(),
            diagram_image_name: None,
            diagram_image_data: None,
            competition_diagram: None,
            lw_ratio: Some("1.1".to_owned()),
            refractive_index: Some("1.5".to_owned()),
            index_gear: Some("64".to_owned()),
            volume: None,
            facets_count: Some("40".to_owned()),
            shape: Some("Oval".to_owned()),
            designer_info: Some("Someone".to_owned()),
            hw_ratio: None,
            tw_ratio: None,
            uw_ratio: None,
            pw_ratio: None,
            cw_ratio: None,
            symmetry_order: None,
            mirror_symmetry: None,
            designer: None,
            source_citation: None,
            pdf_file: None,
            gem_file: None,
            shape_category: None,
            angle_settings: vec![
                AngleSetting::default(),
                AngleSetting {
                    tool: Some("Ball".to_owned()),
                    ..AngleSetting::default()
                },
            ],
            attached_files: Vec::new(),
            frosting: Vec::new(),
        };
        let record = SearchRecord::from(&full);
        assert_eq!(record.id, 9);
        assert_eq!(record.design_id.as_deref(), Some("FD-1"));
        assert_eq!(record.facets, Some(40));
        assert_eq!(record.index_gear, Some(64));
        assert_eq!(record.concave_tiers, 1);
        assert_eq!(record.shape.as_deref(), Some("Oval"));
    }
}

/// The same fixtures through the SQLite `Database` search and this engine, comparing ids and
/// order (and the totals and exclusion counts).
#[cfg(all(test, feature = "sqlite"))]
mod parity_tests {
    use super::*;
    use crate::{
        db::sqlite::{Database, DisplayFilters, SEARCH_RESULT_CAP as SQL_CAP},
        local::LOCAL_SOURCE_ID,
        model::performance::{
            PerformanceAggregate, PerformanceBound, PerformanceFilter, PerformanceMetric,
        },
    };
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU32, Ordering},
    };

    /// A fresh temp database path, removed (with its WAL files) on drop.
    struct TempDb(PathBuf);

    impl TempDb {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "pleochroma_vault_search_parity_{n}_{}.sqlite",
                std::process::id()
            ));
            let _ = std::fs::remove_file(&path);
            Self(path)
        }

        fn path(&self) -> &str {
            self.0.to_str().expect("a UTF-8 temp path")
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let mut name = self.0.clone().into_os_string();
                name.push(suffix);
                let _ = std::fs::remove_file(PathBuf::from(name));
            }
        }
    }

    /// One fixture design: the rows saved to the database and the side data around them.
    struct Row {
        title: &'static str,
        url: &'static str,
        design_id: &'static str,
        detail: Option<FacetingDiagramDetail>,
        ignored: bool,
        tags: &'static [&'static str],
        created_at: Option<i64>,
        updated_at: Option<i64>,
    }

    fn detail(
        shape: &str,
        gear: &str,
        ri: &str,
        lw: &str,
        volume: &str,
        facets: &str,
    ) -> FacetingDiagramDetail {
        FacetingDiagramDetail {
            shape: Some(shape.to_owned()),
            index_gear: Some(gear.to_owned()),
            refractive_index: Some(ri.to_owned()),
            lw_ratio: Some(lw.to_owned()),
            volume: Some(volume.to_owned()),
            facets_count: Some(facets.to_owned()),
            ..FacetingDiagramDetail::default()
        }
    }

    /// Two tiers with notes, one holding `%` and one a word to search for.
    fn noted_detail() -> FacetingDiagramDetail {
        FacetingDiagramDetail {
            angle_settings_table: vec![
                AngleSetting {
                    facet: "P1".to_owned(),
                    notes: "Polish at 50% speed".to_owned(),
                    ..AngleSetting::default()
                },
                AngleSetting {
                    facet: "C1".to_owned(),
                    notes: "client heirloom".to_owned(),
                    ..AngleSetting::default()
                },
            ],
            ..detail("Round", "64", "1.62", "1.0", "0.6", "33+1")
        }
    }

    /// Two concave tiers, the tool name of one standing apart from its notes.
    fn concave_detail() -> FacetingDiagramDetail {
        FacetingDiagramDetail {
            concave_tiers: 2,
            concave_facets: 4,
            angle_settings_table: vec![AngleSetting {
                facet: "D1".to_owned(),
                tool: Some("Ball 6mm".to_owned()),
                notes: "dimple".to_owned(),
                ..AngleSetting::default()
            }],
            ..detail("Heart", "96", "1.5", "1.2", "0.8", "40+0")
        }
    }

    fn rows() -> Vec<Row> {
        let mut rows = first_rows();
        rows.extend(more_rows());
        rows
    }

    fn first_rows() -> Vec<Row> {
        let torbjorn = FacetingDiagramDetail {
            designer_info: Some("Torbj\u{f6}rn \u{c5}kesson; JGD 2001".to_owned()),
            designer: Some("\u{c5}kesson, Torbj\u{f6}rn".to_owned()),
            ..detail("Round", "96", "1.54", "1.0", "0.45", "57+8")
        };
        let cams = FacetingDiagramDetail {
            designer_info: Some("Cam\u{2019}s Designs; Facet Journal".to_owned()),
            ..detail("Oval", "96", "1.76", "1.35", "0.5", "49+8")
        };
        let noted = noted_detail();
        let dashes = FacetingDiagramDetail {
            designer: Some("Smith - Jones".to_owned()),
            ..detail("Round", "96", "1.54", "1.05", "0.5", "57+8")
        };
        let concave = concave_detail();
        vec![
            Row {
                title: "Torbj\u{f6}rn Round",
                url: "local://torb.asc",
                design_id: "",
                detail: Some(torbjorn),
                ignored: false,
                tags: &["Favorite"],
                created_at: Some(100),
                updated_at: Some(900),
            },
            Row {
                title: "Cam\u{2019}s Oval",
                url: "https://example.test/cams",
                design_id: "FD-0042",
                detail: Some(cams),
                ignored: false,
                tags: &["Competition"],
                created_at: Some(300),
                updated_at: Some(100),
            },
            Row {
                title: "Round_Brilliant",
                url: "local://rb.asc",
                design_id: "",
                detail: Some(noted),
                ignored: false,
                tags: &["Favorite"],
                created_at: Some(200),
                updated_at: Some(100),
            },
            Row {
                title: "50% Off Design",
                url: "https://example.test/off",
                design_id: "",
                detail: Some(detail("Cushion", "96", "2.42", "1.1", "0.9", "58")),
                ignored: false,
                tags: &["Competition"],
                created_at: None,
                updated_at: Some(500),
            },
            Row {
                title: "Em\u{2014}Dash Design",
                url: "local://dash.asc",
                design_id: "",
                detail: Some(dashes),
                ignored: false,
                tags: &[],
                created_at: Some(300),
                updated_at: None,
            },
            Row {
                title: "Plain",
                url: "local://plain.asc",
                design_id: "",
                detail: None,
                ignored: false,
                tags: &[],
                created_at: Some(50),
                updated_at: Some(50),
            },
            Row {
                title: "Concave Heart",
                url: "local://heart.asc",
                design_id: "",
                detail: Some(concave),
                ignored: false,
                tags: &[],
                created_at: Some(400),
                updated_at: Some(400),
            },
        ]
    }

    fn more_rows() -> Vec<Row> {
        vec![
            Row {
                title: "Hidden Round",
                url: "https://example.test/hidden",
                design_id: "",
                detail: Some(detail("Round", "96", "1.54", "1.0", "0.45", "57+8")),
                ignored: true,
                tags: &["Favorite"],
                created_at: Some(10),
                updated_at: Some(10),
            },
            Row {
                title: "ZEBRA star",
                url: "https://example.test/zebra",
                design_id: "",
                detail: Some(detail("Star", "80", "1.9", "0.8", "0.3", "80+FC")),
                ignored: false,
                tags: &["Competition"],
                created_at: Some(500),
                updated_at: Some(600),
            },
            Row {
                title: "alpha emerald",
                url: "local://emerald.asc",
                design_id: "",
                detail: Some(detail("Emerald", "96", "1.58", "1.5", "0.7", "44+4")),
                ignored: false,
                tags: &["Favorite"],
                created_at: Some(200),
                updated_at: Some(700),
            },
            Row {
                title: "Alpha Emerald",
                url: "https://example.test/emerald",
                design_id: "",
                detail: Some(detail("Emerald", "96", "1.58", "1.5", "0.7", "44+4")),
                ignored: false,
                tags: &[],
                created_at: Some(200),
                updated_at: Some(700),
            },
            Row {
                title: "\u{c9}clat",
                url: "https://example.test/eclat",
                design_id: "",
                detail: Some(detail("Round", "64", "1.48", "1.0", "0.2", "17")),
                ignored: false,
                tags: &[],
                created_at: Some(600),
                updated_at: Some(800),
            },
            Row {
                title: "Upper Url",
                url: "LOCAL://Upper.asc",
                design_id: "",
                detail: Some(detail("Oval", "96", "1.5", "1.4", "0.5", "49+8")),
                ignored: false,
                tags: &[],
                created_at: Some(700),
                updated_at: Some(200),
            },
        ]
    }

    struct Fixture {
        _file: TempDb,
        db: Database,
        records: Vec<SearchRecord>,
        ids: Vec<i64>,
    }

    fn fixture() -> Fixture {
        let file = TempDb::new();
        let db = Database::new(Some(file.path())).expect("create migrated db");
        let mut records = Vec::new();
        let mut ids = Vec::new();
        for row in rows() {
            let entry = FacetingDiagramEntry {
                title: row.title.to_owned(),
                url: row.url.to_owned(),
                design_id: row.design_id.to_owned(),
            };
            let detail = row.detail.clone().unwrap_or_default();
            let id = if row.detail.is_some() {
                db.save_design(&entry, &detail, LOCAL_SOURCE_ID)
                    .expect("save design")
            } else {
                db.save_diagram_entry(&entry, LOCAL_SOURCE_ID)
                    .expect("save entry")
            };
            if row.ignored {
                db.set_diagram_ignored(id, true).expect("ignore");
            }
            for tag in row.tags {
                db.add_tag_to_entry(id, tag).expect("tag");
            }
            records.push(
                SearchRecord::from_parts(id, &entry, &detail)
                    .with_ignored(row.ignored)
                    .with_tags(row.tags)
                    .with_timestamps(row.created_at, row.updated_at),
            );
            ids.push(id);
        }
        // The store stamps both times with "now"; give them the fixture's distinct values
        // (and NULLs) through a second connection so the date sorts have something to order.
        let conn = rusqlite::Connection::open(file.path()).expect("second connection");
        for record in &records {
            conn.execute(
                "UPDATE diagram_entries SET created_at = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![record.created_at, record.updated_at, record.id],
            )
            .expect("set timestamps");
        }
        drop(conn);
        Fixture {
            _file: file,
            db,
            records,
            ids,
        }
    }

    #[derive(Default)]
    struct Case {
        name: &'static str,
        query: &'static str,
        shape: &'static str,
        gear: &'static str,
        range: RangeFilter,
        order: SortOrder,
        local_only: bool,
        tag: Option<&'static str>,
        ids: Option<Vec<i64>>,
    }

    fn named(name: &'static str, query: &'static str) -> Case {
        Case {
            name,
            query,
            ..Case::default()
        }
    }

    /// A default range filter with `update` applied.
    fn ranged(update: impl FnOnce(&mut RangeFilter)) -> RangeFilter {
        let mut range = RangeFilter::default();
        update(&mut range);
        range
    }

    const fn sql_order(order: SortOrder) -> crate::db::sqlite::SortOrder {
        match order {
            SortOrder::CatalogueOrder => crate::db::sqlite::SortOrder::CatalogueOrder,
            SortOrder::Title => crate::db::sqlite::SortOrder::Title,
            SortOrder::Newest => crate::db::sqlite::SortOrder::Newest,
            SortOrder::RecentlyEdited => crate::db::sqlite::SortOrder::RecentlyEdited,
        }
    }

    /// Runs `case` through both and asserts the same ids in the same order, total and
    /// exclusion count; also the uncapped id walk.
    fn assert_parity(fixture: &Fixture, case: &Case) {
        let tag_id = case.tag.map(|name| {
            fixture
                .db
                .tag_id_by_name(name)
                .expect("tag lookup")
                .expect("the fixture carries this tag")
        });
        let (sql, sql_total) = fixture
            .db
            .search_diagrams_display_with_count(
                case.query,
                case.shape,
                case.gear,
                &case.range,
                DisplayFilters {
                    order: sql_order(case.order),
                    local_only: case.local_only,
                    tag_filter: tag_id,
                    id_filter: case.ids.as_deref(),
                },
            )
            .expect("sql search");
        let options = SearchOptions {
            order: case.order,
            local_only: case.local_only,
            tag_filter: case.tag,
            id_filter: case.ids.as_deref(),
        };
        let mine = search(
            &fixture.records,
            case.query,
            case.shape,
            case.gear,
            &case.range,
            options,
        );
        let sql_ids: Vec<i64> = sql.items.iter().map(|i| i.id).collect();
        let my_ids: Vec<i64> = mine.items.iter().map(|r| r.id).collect();
        assert_eq!(my_ids, sql_ids, "case '{}': ids and order", case.name);
        assert_eq!(mine.total, sql_total, "case '{}': total", case.name);
        assert_eq!(
            mine.excluded_for_missing_curves, sql.excluded_for_missing_curves,
            "case '{}': performance exclusions",
            case.name
        );

        let sql_walk = fixture
            .db
            .matching_entry_ids(
                case.query,
                case.shape,
                case.gear,
                &case.range,
                DisplayFilters {
                    order: crate::db::sqlite::SortOrder::CatalogueOrder,
                    local_only: case.local_only,
                    tag_filter: tag_id,
                    id_filter: case.ids.as_deref(),
                },
            )
            .expect("sql id walk");
        let my_walk = matching_ids(
            &fixture.records,
            case.query,
            case.shape,
            case.gear,
            &case.range,
            options,
        );
        assert_eq!(my_walk, sql_walk, "case '{}': id walk", case.name);
    }

    fn text_cases() -> Vec<Case> {
        vec![
            named("empty", ""),
            named("blank", "   "),
            named("ascii title", "round"),
            named("upper title", "ROUND"),
            named("padded", "  round  "),
            named("accented upper", "TORBJ\u{d6}RN"),
            named("accented lower", "torbj\u{f6}rn"),
            named("split designer", "\u{e5}kesson"),
            named("plain apostrophe", "cam's"),
            named("curly apostrophe", "Cam\u{2019}s"),
            named("em dash query", "\u{2014}"),
            named("hyphen query", "-"),
            named("designer dash", "smith \u{2014} jones"),
            named("underscore", "_"),
            named("percent", "%"),
            named("50 percent", "50%"),
            named("backslash", "\\"),
            named("design id", "fd-0042"),
            named("design id upper", "FD-"),
            named("note", "heirloom"),
            named("note upper", "HEIRLOOM"),
            named("note with percent", "50% speed"),
            named("tool text is not notes", "Ball"),
            named("accented title only", "\u{e9}clat"),
            named("no match", "zzzzqq"),
        ]
    }

    #[test]
    fn free_text_matches_the_sql_search() {
        let fixture = fixture();
        for case in text_cases() {
            assert_parity(&fixture, &case);
        }
        // The cases above must actually discriminate: a few have to hit and a few to miss.
        let hit = search(
            &fixture.records,
            "TORBJ\u{d6}RN",
            "All",
            "All",
            &RangeFilter::default(),
            SearchOptions::default(),
        );
        assert_eq!(hit.total, 1);
    }

    #[test]
    fn attribute_and_range_filters_match_the_sql_search() {
        let fixture = fixture();
        let mut cases = vec![
            Case {
                name: "shape Round",
                shape: "Round",
                ..Case::default()
            },
            Case {
                name: "shape lowercase is exact",
                shape: "round",
                ..Case::default()
            },
            Case {
                name: "shape All",
                shape: "All",
                ..Case::default()
            },
            Case {
                name: "gear 96",
                gear: "96",
                ..Case::default()
            },
            Case {
                name: "gear 64",
                gear: "64",
                ..Case::default()
            },
            Case {
                name: "gear text",
                gear: "abc",
                ..Case::default()
            },
            Case {
                name: "shape and gear and text",
                query: "round",
                shape: "Round",
                gear: "96",
                ..Case::default()
            },
        ];
        let ranges: Vec<(&'static str, RangeFilter)> = vec![
            ("ri min", ranged(|r| r.ri_min = Some(1.54))),
            ("ri max", ranged(|r| r.ri_max = Some(1.54))),
            (
                "ri band",
                ranged(|r| (r.ri_min, r.ri_max) = (Some(1.5), Some(1.6))),
            ),
            (
                "lw band",
                ranged(|r| (r.lw_min, r.lw_max) = (Some(1.0), Some(1.2))),
            ),
            (
                "volume band",
                ranged(|r| (r.volume_min, r.volume_max) = (Some(0.45), Some(0.7))),
            ),
            (
                "facets band",
                ranged(|r| (r.facets_min, r.facets_max) = (Some(44), Some(57))),
            ),
            ("facets min only", ranged(|r| r.facets_min = Some(58))),
            (
                "ri tolerance",
                ranged(|r| r.ri_tolerance = Some((1.54, 0.01))),
            ),
            (
                "ri tolerance and min",
                ranged(|r| {
                    r.ri_min = Some(1.55);
                    r.ri_tolerance = Some((1.58, 0.1));
                }),
            ),
            ("concave", ranged(|r| r.has_concave = Some(true))),
            ("planar", ranged(|r| r.has_concave = Some(false))),
            ("include ignored", ranged(|r| r.include_ignored = true)),
            (
                "include ignored and ri",
                ranged(|r| {
                    r.include_ignored = true;
                    r.ri_max = Some(1.54);
                }),
            ),
        ];
        for (name, range) in ranges {
            cases.push(Case {
                name,
                range,
                ..Case::default()
            });
        }
        for case in &cases {
            assert_parity(&fixture, case);
        }
    }

    #[test]
    fn restrictions_and_sort_orders_match_the_sql_search() {
        let fixture = fixture();
        let some_ids: Vec<i64> = fixture.ids.iter().copied().step_by(2).collect();
        let mut cases = vec![
            Case {
                name: "mine",
                local_only: true,
                ..Case::default()
            },
            Case {
                name: "mine and text",
                local_only: true,
                query: "round",
                ..Case::default()
            },
            Case {
                name: "tag lowercase",
                tag: Some("competition"),
                ..Case::default()
            },
            Case {
                name: "tag with ignored",
                tag: Some("Favorite"),
                range: RangeFilter {
                    include_ignored: true,
                    ..RangeFilter::default()
                },
                ..Case::default()
            },
            Case {
                name: "id set",
                ids: Some(some_ids),
                ..Case::default()
            },
            Case {
                name: "empty id set",
                ids: Some(Vec::new()),
                ..Case::default()
            },
        ];
        for (name, order) in [
            ("catalogue", SortOrder::CatalogueOrder),
            ("title", SortOrder::Title),
            ("newest", SortOrder::Newest),
            ("edited", SortOrder::RecentlyEdited),
        ] {
            cases.push(Case {
                name,
                order,
                range: RangeFilter {
                    include_ignored: true,
                    ..RangeFilter::default()
                },
                ..Case::default()
            });
            cases.push(Case {
                name,
                order,
                local_only: true,
                ..Case::default()
            });
            cases.push(Case {
                name,
                order,
                query: "e",
                ..Case::default()
            });
        }
        for case in &cases {
            assert_parity(&fixture, case);
        }
    }

    #[test]
    fn a_performance_filter_matches_the_sql_search_without_curves() {
        let fixture = fixture();
        let filter = PerformanceFilter::new(
            PerformanceMetric::Windowing,
            PerformanceBound::AtMost(20.0),
            45.0,
            PerformanceAggregate::Worst,
        )
        .expect("a valid radius");
        for query in ["", "round"] {
            let case = Case {
                name: "performance",
                query,
                range: RangeFilter {
                    performance: vec![filter],
                    ..RangeFilter::default()
                },
                ..Case::default()
            };
            assert_parity(&fixture, &case);
        }
    }

    #[test]
    fn the_result_cap_matches_the_sql_search() {
        assert_eq!(i64::try_from(SEARCH_RESULT_CAP).ok(), Some(SQL_CAP));

        let db = Database::new(Some(":memory:")).expect("in-memory db");
        let mut records = Vec::new();
        for n in 0..(SEARCH_RESULT_CAP + 12) {
            let entry = FacetingDiagramEntry {
                title: format!("Design {n}"),
                url: format!("local://design-{n}.asc"),
                design_id: String::new(),
            };
            let detail = FacetingDiagramDetail::default();
            let id = db
                .save_design(&entry, &detail, LOCAL_SOURCE_ID)
                .expect("save design");
            records.push(SearchRecord::from_parts(id, &entry, &detail));
        }
        let (sql, sql_total) = db
            .search_diagrams_display_with_count(
                "design",
                "All",
                "All",
                &RangeFilter::default(),
                DisplayFilters::default(),
            )
            .expect("sql search");
        let mine = search(
            &records,
            "design",
            "All",
            "All",
            &RangeFilter::default(),
            SearchOptions::default(),
        );
        assert_eq!(sql.items.len(), SEARCH_RESULT_CAP);
        assert_eq!(mine.items.len(), SEARCH_RESULT_CAP);
        assert_eq!(sql_total, SEARCH_RESULT_CAP + 12);
        assert_eq!(mine.total, sql_total);
        let sql_ids: Vec<i64> = sql.items.iter().map(|i| i.id).collect();
        let my_ids: Vec<i64> = mine.items.iter().map(|r| r.id).collect();
        assert_eq!(my_ids, sql_ids);
    }
}
