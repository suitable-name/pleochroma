//! What a re-import does to a row that already exists: the merge of hand-entered
//! metadata, and the rule that says a file is a duplicate.
//!
//! The rows themselves live in a store the caller owns (SQLite on desktop and Android,
//! `IndexedDB` in the browser); this module only decides, from a fresh parse and the stored
//! record, what the row to write looks like.

use crate::model::{detail::FacetingDiagramDetail, entry::FullDiagramRecord, file::AttachedFile};

use super::{design_attachment_position, native_design_attachment_position};

/// What stands in the store for a freshly parsed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingRow {
    /// No row has this file's url: it is stored as a new entry.
    Absent,
    /// A row has this url and already holds a design file with exactly these bytes:
    /// nothing is written.
    Identical,
    /// A row has this url and different bytes: it is replaced, its hand-entered
    /// metadata carried over by [`merge_reimport_metadata`].
    Replaced,
}

/// The attachment of `files` a stored entry opens from, if there is one.
///
/// A self-contained `.psdd` design file first, else the first
/// `.asc`, else the first `.gem`, else the first `.gcs`. Other attachments (PDFs, images)
/// are never a design.
#[must_use]
pub fn primary_design_attachment(files: &[AttachedFile]) -> Option<&AttachedFile> {
    let names = || files.iter().map(|file| file.name.as_str());
    native_design_attachment_position(names())
        .or_else(|| design_attachment_position(names()).map(|(position, _)| position))
        .and_then(|position| files.get(position))
}

/// Whether the stored entry already holds the design file `detail` would store: the same
/// file name with exactly the same bytes.
#[must_use]
pub fn holds_same_file(detail: &FacetingDiagramDetail, stored: &FullDiagramRecord) -> bool {
    let Some(fresh) = primary_design_attachment(&detail.attached_files) else {
        return false;
    };
    stored
        .attached_files
        .iter()
        .any(|file| file.name == fresh.name && file.content == fresh.content)
}

/// Settles a fresh parse against the record already stored under its url.
///
/// With `skip_identical`, a record that holds the same design file ends the import
/// ([`ExistingRow::Identical`], `fresh` untouched); otherwise, or when the bytes differ,
/// the record's hand-entered metadata is carried into `fresh` and the row counts as
/// [`ExistingRow::Replaced`]. `None` (no row under the url) is [`ExistingRow::Absent`].
pub fn reconcile_with_existing(
    fresh: &mut FacetingDiagramDetail,
    existing: Option<&FullDiagramRecord>,
    skip_identical: bool,
) -> ExistingRow {
    let Some(existing) = existing else {
        return ExistingRow::Absent;
    };
    if skip_identical && holds_same_file(fresh, existing) {
        return ExistingRow::Identical;
    }
    merge_reimport_metadata(fresh, existing);
    ExistingRow::Replaced
}

/// Carries the hand-entered fields of the row being replaced into the fresh detail.
///
/// A row is replaced wholesale by the store, so re-importing a file whose name collides
/// with an existing row would, without this merge, silently wipe every hand-entered field
/// the fresh parse does not itself produce: `designer_info`, a manually corrected `shape`,
/// the competition-entry columns, and any proportion the cutter typed in over the
/// measurement.
///
/// # The merge rule
///
/// For every field below: the freshly imported or measured value wins when it is present
/// (`Some`, non-empty): a re-import is the cutter saying "this file is the current
/// version", so a fresh geometry-derived shape or proportion wins over a stale one. Only
/// when the fresh parse left a field blank does the EXISTING row's value survive. Since
/// the importer never populates `designer_info`, `designer`, `source_citation`,
/// `competition_diagram`, `pdf_file`, `gem_file`, `shape_category`,
/// `diagram_image_name`/`diagram_image_data` or `page_url` at all (those are
/// remote-scrape-only or hand-typed fields), this rule always preserves them across a
/// local re-import: a cutter's typed metadata is never silently discarded.
/// `angle_settings_table` and `attached_files` are deliberately NOT touched: those two
/// are the whole point of a re-import and always come from the fresh file.
pub fn merge_reimport_metadata(fresh: &mut FacetingDiagramDetail, existing: &FullDiagramRecord) {
    if fresh.page_url.is_empty() {
        fresh.page_url.clone_from(&existing.page_url);
    }
    fresh.diagram_image_name = fresh
        .diagram_image_name
        .take()
        .or_else(|| existing.diagram_image_name.clone());
    fresh.diagram_image_data = fresh
        .diagram_image_data
        .take()
        .or_else(|| existing.diagram_image_data.clone());
    fresh.competition_diagram = fresh
        .competition_diagram
        .take()
        .or_else(|| existing.competition_diagram.clone());
    fresh.lw_ratio = fresh.lw_ratio.take().or_else(|| existing.lw_ratio.clone());
    fresh.refractive_index = fresh
        .refractive_index
        .take()
        .or_else(|| existing.refractive_index.clone());
    fresh.index_gear = fresh
        .index_gear
        .take()
        .or_else(|| existing.index_gear.clone());
    fresh.volume = fresh.volume.take().or_else(|| existing.volume.clone());
    fresh.facets_count = fresh
        .facets_count
        .take()
        .or_else(|| existing.facets_count.clone());
    fresh.shape = fresh.shape.take().or_else(|| existing.shape.clone());
    fresh.designer_info = fresh
        .designer_info
        .take()
        .or_else(|| existing.designer_info.clone());
    fresh.hw_ratio = fresh.hw_ratio.take().or_else(|| existing.hw_ratio.clone());
    fresh.tw_ratio = fresh.tw_ratio.take().or_else(|| existing.tw_ratio.clone());
    fresh.uw_ratio = fresh.uw_ratio.take().or_else(|| existing.uw_ratio.clone());
    fresh.pw_ratio = fresh.pw_ratio.take().or_else(|| existing.pw_ratio.clone());
    fresh.cw_ratio = fresh.cw_ratio.take().or_else(|| existing.cw_ratio.clone());
    fresh.symmetry_order = fresh
        .symmetry_order
        .take()
        .or_else(|| existing.symmetry_order.clone());
    fresh.mirror_symmetry = fresh.mirror_symmetry.or(existing.mirror_symmetry);
    fresh.designer = fresh.designer.take().or_else(|| existing.designer.clone());
    fresh.source_citation = fresh
        .source_citation
        .take()
        .or_else(|| existing.source_citation.clone());
    fresh.pdf_file = fresh.pdf_file.take().or_else(|| existing.pdf_file.clone());
    fresh.gem_file = fresh.gem_file.take().or_else(|| existing.gem_file.clone());
    fresh.shape_category = fresh
        .shape_category
        .take()
        .or_else(|| existing.shape_category.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, content: &[u8]) -> AttachedFile {
        AttachedFile {
            name: name.to_owned(),
            url: String::new(),
            content: content.to_vec(),
        }
    }

    fn fresh_with(files: Vec<AttachedFile>) -> FacetingDiagramDetail {
        FacetingDiagramDetail {
            attached_files: files,
            ..FacetingDiagramDetail::default()
        }
    }

    /// A stored record holding `files`, with the hand-entered fields a cutter types.
    fn stored_with(files: Vec<AttachedFile>) -> FullDiagramRecord {
        FullDiagramRecord {
            entry_id: 7,
            title: "Old".to_owned(),
            url: "local://a.asc".to_owned(),
            design_id: None,
            page_url: "https://example.org/a".to_owned(),
            diagram_image_name: Some("a.png".to_owned()),
            diagram_image_data: Some(vec![1, 2, 3]),
            competition_diagram: Some("Cup".to_owned()),
            lw_ratio: Some("1.5".to_owned()),
            refractive_index: Some("1.54".to_owned()),
            index_gear: Some("96".to_owned()),
            volume: Some("0.7".to_owned()),
            facets_count: Some("57".to_owned()),
            shape: Some("Oval".to_owned()),
            designer_info: Some("Ada; Journal".to_owned()),
            hw_ratio: None,
            tw_ratio: None,
            uw_ratio: None,
            pw_ratio: None,
            cw_ratio: None,
            symmetry_order: None,
            mirror_symmetry: Some(true),
            designer: Some("Ada".to_owned()),
            source_citation: None,
            pdf_file: Some("a.pdf".to_owned()),
            gem_file: None,
            shape_category: Some("Fancy".to_owned()),
            angle_settings: Vec::new(),
            attached_files: files,
            frosting: Vec::new(),
        }
    }

    #[test]
    fn the_primary_attachment_prefers_a_design_file_then_asc_then_gem_then_gcs() {
        let files = vec![
            file("note.pdf", b"p"),
            file("a.gcs", b"c"),
            file("a.gem", b"g"),
            file("a.asc", b"s"),
            file("a.psdd", b"d"),
        ];
        assert_eq!(primary_design_attachment(&files).unwrap().name, "a.psdd");
        assert_eq!(
            primary_design_attachment(&files[..4]).unwrap().name,
            "a.asc"
        );
        assert_eq!(
            primary_design_attachment(&files[..3]).unwrap().name,
            "a.gem"
        );
        assert_eq!(
            primary_design_attachment(&files[..2]).unwrap().name,
            "a.gcs"
        );
        assert!(primary_design_attachment(&files[..1]).is_none());
        assert!(primary_design_attachment(&[]).is_none());
    }

    #[test]
    fn the_same_name_with_the_same_bytes_is_the_same_file_and_other_bytes_are_not() {
        let stored = stored_with(vec![file("a.asc", b"one"), file("a.psdd", b"two")]);
        assert!(holds_same_file(
            &fresh_with(vec![file("a.asc", b"one")]),
            &stored
        ));
        assert!(!holds_same_file(
            &fresh_with(vec![file("a.asc", b"changed")]),
            &stored
        ));
        assert!(!holds_same_file(
            &fresh_with(vec![file("b.asc", b"one")]),
            &stored
        ));
        assert!(!holds_same_file(&fresh_with(Vec::new()), &stored));
    }

    #[test]
    fn reconcile_reports_absent_identical_and_replaced() {
        let stored = stored_with(vec![file("a.asc", b"one")]);

        let mut fresh = fresh_with(vec![file("a.asc", b"one")]);
        assert_eq!(
            reconcile_with_existing(&mut fresh, None, true),
            ExistingRow::Absent
        );
        assert_eq!(
            reconcile_with_existing(&mut fresh, Some(&stored), true),
            ExistingRow::Identical
        );
        assert_eq!(fresh.shape, None, "an identical file is not merged");

        let mut same_bytes_replace_anyway = fresh_with(vec![file("a.asc", b"one")]);
        assert_eq!(
            reconcile_with_existing(&mut same_bytes_replace_anyway, Some(&stored), false),
            ExistingRow::Replaced
        );

        let mut changed = fresh_with(vec![file("a.asc", b"two")]);
        assert_eq!(
            reconcile_with_existing(&mut changed, Some(&stored), true),
            ExistingRow::Replaced
        );
        assert_eq!(changed.shape.as_deref(), Some("Oval"));
    }

    #[test]
    fn a_fresh_value_wins_and_a_blank_one_keeps_the_stored_hand_entered_value() {
        let stored = stored_with(Vec::new());
        let mut fresh = FacetingDiagramDetail {
            shape: Some("Round".to_owned()),
            lw_ratio: Some("1".to_owned()),
            page_url: String::new(),
            ..FacetingDiagramDetail::default()
        };
        merge_reimport_metadata(&mut fresh, &stored);
        assert_eq!(fresh.shape.as_deref(), Some("Round"), "fresh wins");
        assert_eq!(fresh.lw_ratio.as_deref(), Some("1"), "fresh wins");
        assert_eq!(fresh.volume.as_deref(), Some("0.7"));
        assert_eq!(fresh.designer_info.as_deref(), Some("Ada; Journal"));
        assert_eq!(fresh.designer.as_deref(), Some("Ada"));
        assert_eq!(fresh.competition_diagram.as_deref(), Some("Cup"));
        assert_eq!(fresh.page_url, "https://example.org/a");
        assert_eq!(fresh.diagram_image_name.as_deref(), Some("a.png"));
        assert_eq!(fresh.diagram_image_data.as_deref(), Some(&[1, 2, 3][..]));
        assert_eq!(fresh.mirror_symmetry, Some(true));
        assert_eq!(fresh.shape_category.as_deref(), Some("Fancy"));
        assert_eq!(fresh.pdf_file.as_deref(), Some("a.pdf"));
        assert!(fresh.attached_files.is_empty(), "attachments never carry");
    }
}
