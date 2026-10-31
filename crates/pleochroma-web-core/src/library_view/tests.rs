use super::*;
use crate::library_record::{ImportPlan, plan_import};
use pleochroma_vault::local::{ImportKind, import_bytes};

const SAMPLE_ASC: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 6 y\n\
I 1.72\n\
H Round Trichecker-12\n\
a -41.000000 0.64991234 92 n 1 84 76 68 60\n\
a 41.000000 0.5 92 n T\n";

const NOW: &str = "2026-10-26T10:00:00Z";

fn file(name: &str, bytes: &[u8]) -> AttachedFile {
    AttachedFile {
        name: name.to_owned(),
        url: String::new(),
        content: bytes.to_vec(),
    }
}

/// A stored entry (and its attachments) made the way an import makes one.
fn stored(
    file_name: &str,
    title: &str,
    id: i64,
    shape: &str,
    gear: u32,
) -> (LibraryEntry, Vec<AttachedFile>) {
    let asc = SAMPLE_ASC.replace("Round Trichecker-12", title);
    let imported = import_bytes(
        file_name,
        ImportKind::Asc,
        asc.as_bytes(),
        None,
        |_| {},
        |_| {},
    )
    .unwrap_or_else(|failure| panic!("{failure}"));
    let now_ms = (id as f64).mul_add(10_000.0, 1_700_000_000_000.0);
    let ImportPlan::Store(store) = plan_import(imported.parsed, None, None, now_ms, true) else {
        panic!("a new import is stored");
    };
    let mut entry = store.entry;
    entry.item.id = id;
    entry.detail.shape = Some(shape.to_owned());
    entry.detail.index_gear = Some(gear.to_string());
    entry.refresh_item();
    (entry, store.attachments)
}

fn library() -> Vec<LibraryEntry> {
    let mut entries = vec![
        stored("round.asc", "Round Brilliant", 1, "Round", 96).0,
        stored("oval.asc", "Oval Step", 2, "Oval", 64).0,
        stored("heart.asc", "Heart Fancy", 3, "Heart", 96).0,
        stored("cushion.asc", "Concave Cushion", 4, "Cushion", 96).0,
    ];
    entries[1].tags = vec!["gift".to_owned(), "Step".to_owned()];
    entries[2].item.ignored = true;
    entries[3].detail.concave_tiers = 2;
    entries
}

fn titles(entries: &[LibraryEntry], result: &QueryResult) -> Vec<String> {
    result
        .indexes
        .iter()
        .map(|&i| entries[i].item.title.clone())
        .collect()
}

#[test]
fn an_empty_query_lists_everything_but_ignored_designs_newest_first() {
    let entries = library();
    let result = run_query(&entries, &LibraryQuery::default());
    assert_eq!(result.total, 3);
    assert_eq!(
        titles(&entries, &result),
        ["Concave Cushion", "Oval Step", "Round Brilliant"]
    );
    let shown = run_query(
        &entries,
        &LibraryQuery {
            show_ignored: true,
            ..LibraryQuery::default()
        },
    );
    assert_eq!(shown.total, 4);
}

#[test]
fn the_search_box_matches_titles_regardless_of_case() {
    let entries = library();
    let result = run_query(
        &entries,
        &LibraryQuery {
            text: "  OVAL ".to_owned(),
            ..LibraryQuery::default()
        },
    );
    assert_eq!(titles(&entries, &result), ["Oval Step"]);
}

#[test]
fn shape_gear_tag_and_concave_filters_narrow_the_list() {
    let entries = library();
    let by = |query: LibraryQuery| titles(&entries, &run_query(&entries, &query));
    assert_eq!(
        by(LibraryQuery {
            shape: "Round".to_owned(),
            ..LibraryQuery::default()
        }),
        ["Round Brilliant"]
    );
    assert_eq!(
        by(LibraryQuery {
            gear: "96".to_owned(),
            sort: LibrarySort::Title,
            ..LibraryQuery::default()
        }),
        ["Concave Cushion", "Round Brilliant"]
    );
    assert_eq!(
        by(LibraryQuery {
            tag: "GIFT".to_owned(),
            ..LibraryQuery::default()
        }),
        ["Oval Step"]
    );
    assert_eq!(
        by(LibraryQuery {
            tag: ALL.to_owned(),
            shape: ALL.to_owned(),
            gear: ALL.to_owned(),
            ..LibraryQuery::default()
        })
        .len(),
        3,
        "All means no restriction"
    );
    assert_eq!(
        by(LibraryQuery {
            concave_only: true,
            ..LibraryQuery::default()
        }),
        ["Concave Cushion"]
    );
}

#[test]
fn the_sort_orders_follow_the_combo() {
    let entries = library();
    let by = |sort| {
        titles(
            &entries,
            &run_query(
                &entries,
                &LibraryQuery {
                    sort,
                    ..LibraryQuery::default()
                },
            ),
        )
    };
    assert_eq!(
        by(LibrarySort::Title),
        ["Concave Cushion", "Oval Step", "Round Brilliant"]
    );
    assert_eq!(
        by(LibrarySort::OldestFirst),
        ["Round Brilliant", "Oval Step", "Concave Cushion"]
    );
    assert_eq!(LibrarySort::from_index(2), LibrarySort::Title);
    assert_eq!(LibrarySort::from_index(-1), LibrarySort::RecentlyAdded);
    assert_eq!(LibrarySort::from_index(40), LibrarySort::RecentlyAdded);
    assert_eq!(
        LibrarySort::ALL.map(LibrarySort::label),
        [
            "Recently Added",
            "Recently Edited",
            "Title A-Z",
            "Oldest First"
        ]
    );
}

#[test]
fn the_filter_options_are_distinct_and_sorted() {
    let mut entries = library();
    entries.push(stored("round2.asc", "Another Round", 5, "round", 96).0);
    entries[4].tags = vec!["gift".to_owned()];
    let options = filter_options(&entries);
    assert_eq!(options.shapes, ["Cushion", "Heart", "Oval", "Round"]);
    assert_eq!(options.gears, ["64", "96"]);
    assert_eq!(options.tags, ["gift", "Step"]);
    let items = combo_items(&options.gears);
    assert_eq!(items, ["All", "64", "96"]);
    assert_eq!(combo_index(&options.gears, "96"), 2);
    assert_eq!(combo_index(&options.gears, "12"), 0);
    assert_eq!(combo_value(&options.gears, 0), "");
    assert_eq!(combo_value(&options.gears, 2), "96");
    assert_eq!(combo_value(&options.gears, 9), "");
    assert_eq!(combo_value(&options.gears, -1), "");
}

#[test]
fn a_gear_of_96_point_0_is_96() {
    assert_eq!(gear_of("96"), Some(96));
    assert_eq!(gear_of(" 96.0 "), Some(96));
    assert_eq!(gear_of("ninety"), None);
}

#[test]
fn a_row_names_what_the_file_states() {
    let (mut entry, _) = stored("round.asc", "Round Brilliant", 1, "Round", 96);
    entry.detail.refractive_index = Some("1.54".to_owned());
    entry.detail.lw_ratio = Some("1.0000000001".to_owned());
    entry.detail.facets_count = Some("57".to_owned());
    entry.detail.concave_tiers = 1;
    entry.tags = vec!["a".to_owned(), "b".to_owned()];
    entry.refresh_item();
    let row = row_text(&entry);
    assert_eq!(row.title, "Round Brilliant");
    assert_eq!(
        row.subtitle,
        "Round \u{b7} Gear 96 \u{b7} RI 1.540 \u{b7} L/W 1.00 \u{b7} 57 facets \u{b7} Concave"
    );
    assert_eq!(row.tags, "a, b");
    assert_eq!(short_number("not a number", 2), "not a number");
    assert_eq!(short_number("2", 2), "2.00");
}

#[test]
fn editing_the_detail_changes_the_listing_and_keeps_the_id() {
    let (mut entry, _) = stored("round.asc", "Round Brilliant", 7, "Round", 96);
    let mut fields = detail_fields(&entry);
    fields.title = "  New Title ".to_owned();
    fields.designer = "Capps, Jerry".to_owned();
    fields.citation = "Lapidary Journal".to_owned();
    fields.shape = "Oval".to_owned();
    fields.ri = "1.62".to_owned();
    fields.tags = " b, a ,b,".to_owned();
    fields.ignored = true;
    apply_detail_edit(&mut entry, &fields).unwrap();
    assert_eq!(entry.item.title, "New Title");
    assert_eq!(
        entry.detail.designer_info.as_deref(),
        Some("Capps, Jerry; Lapidary Journal")
    );
    assert_eq!(entry.item.shape.as_deref(), Some("Oval"));
    assert_eq!(entry.item.refractive_index.as_deref(), Some("1.62"));
    assert_eq!(entry.tags, ["a", "b"]);
    assert!(entry.item.ignored);
    assert_eq!(entry.item.id, 7);
    assert_eq!(entry.item.url, "local://round.asc");
}

#[test]
fn a_hand_written_designer_line_survives_an_edit_of_another_field() {
    let (mut entry, _) = stored("round.asc", "Round Brilliant", 1, "Round", 96);
    entry.detail.designer = Some("Capps".to_owned());
    entry.detail.designer_info = Some("Hand Written Line".to_owned());
    let mut fields = detail_fields(&entry);
    fields.title = "Renamed".to_owned();
    apply_detail_edit(&mut entry, &fields).unwrap();
    assert_eq!(
        entry.detail.designer_info.as_deref(),
        Some("Hand Written Line")
    );
    fields.designer = String::new();
    apply_detail_edit(&mut entry, &fields).unwrap();
    assert_eq!(entry.detail.designer_info, None, "no designer, no line");
}

#[test]
fn a_refused_edit_changes_nothing() {
    let (mut entry, _) = stored("round.asc", "Round Brilliant", 1, "Round", 96);
    let before = detail_fields(&entry);
    let mut fields = before.clone();
    fields.title = "   ".to_owned();
    assert!(apply_detail_edit(&mut entry, &fields).is_err());
    fields.title = "Fine".to_owned();
    fields.ri = "9".to_owned();
    let refusal = apply_detail_edit(&mut entry, &fields).unwrap_err();
    assert!(refusal.contains("Refractive index"), "{refusal}");
    fields.ri = String::new();
    fields.lw = "wide".to_owned();
    assert!(apply_detail_edit(&mut entry, &fields).is_err());
    fields.lw = String::new();
    fields.volume = "-2".to_owned();
    assert!(apply_detail_edit(&mut entry, &fields).is_err());
    assert_eq!(detail_fields(&entry), before);
}

#[test]
fn the_info_lines_leave_out_what_the_file_does_not_state() {
    let (mut entry, _) = stored("round.asc", "Round Brilliant", 1, "Round", 96);
    entry.detail.symmetry_order = Some("8".to_owned());
    entry.detail.mirror_symmetry = Some(true);
    entry.detail.hw_ratio = Some("0.6234".to_owned());
    entry.detail.cw_ratio = Some("0.3".to_owned());
    let info = detail_info(&entry);
    assert!(info.contains("Gear: 96"), "{info}");
    assert!(info.contains("Symmetry: 8-fold, mirror"), "{info}");
    assert!(
        info.contains("Proportions: H/W 0.62 \u{b7} C/W 0.30"),
        "{info}"
    );
    assert!(info.contains("Design File: round.asc"), "{info}");
    assert!(info.contains("Added: 2023-11-14"), "{info}");
    assert!(!info.contains("Concave"), "{info}");
}

#[test]
fn an_entry_opens_from_a_design_file_then_a_gcs_then_an_asc() {
    let a = file("a.asc", b"x");
    let gcs = file("a.gcs", b"x");
    let gem = file("a.gem", b"x");
    let native = file("b.psdd", b"x");
    let pdf = file("notes.pdf", b"x");
    let pick = |files: &[AttachedFile]| open_candidate(files).map(|f| f.name.clone());
    assert_eq!(
        pick(&[a.clone(), gcs.clone(), native]).as_deref(),
        Some("b.psdd")
    );
    assert_eq!(
        pick(&[a.clone(), gcs, pdf.clone()]).as_deref(),
        Some("a.gcs")
    );
    assert_eq!(pick(&[gem.clone(), a]).as_deref(), Some("a.asc"));
    assert_eq!(pick(&[gem]).as_deref(), Some("a.gem"));
    assert_eq!(pick(&[pdf]), None);
    assert_eq!(pick(&[]), None);
}

#[test]
fn an_asc_entry_exports_as_an_psdd_carrying_the_library_metadata() {
    let (mut entry, files) = stored("trichecker.asc", "Round Brilliant", 1, "Round", 96);
    entry.tags = vec!["gift".to_owned()];
    entry.detail.designer = Some("Capps, Jerry".to_owned());
    entry.item.ignored = true;
    let exported = export_psdd(&entry, &files, NOW).unwrap();
    assert_eq!(exported.file_name, "trichecker.psdd");
    let loaded = design_from_str(&exported.text).unwrap();
    assert!(!loaded.design.tiers.is_empty());
    assert_eq!(loaded.metadata.id, entry.design_id);
    assert_eq!(loaded.metadata.title, "Round Brilliant");
    assert_eq!(loaded.metadata.designer, "Capps, Jerry");
    assert_eq!(loaded.metadata.tags, ["gift"]);
    assert!(loaded.metadata.ignored);
    assert_eq!(loaded.metadata.modified_at, NOW);
    assert!(!loaded.metadata.created_at.is_empty());
}

#[test]
fn an_psdd_entry_exports_with_its_file_kept_and_its_edits_written() {
    let (mut entry, files) = stored("trichecker.asc", "Round Brilliant", 1, "Round", 96);
    entry.tags = vec!["gift".to_owned()];
    let first = export_psdd(&entry, &files, NOW).unwrap();

    // Import the exported file as a design file, edit it in the library, export again.
    let imported = import_bytes(
        &first.file_name,
        ImportKind::Design,
        first.text.as_bytes(),
        None,
        |_| {},
        |_| {},
    )
    .unwrap_or_else(|failure| panic!("{failure}"));
    let meta_id = design_from_str(&first.text).unwrap().metadata.id;
    let planned = plan_import(imported.parsed, Some(meta_id.as_str()), None, 5.0e12, true);
    let ImportPlan::Store(store) = planned else {
        panic!("a new import is stored");
    };
    let mut second = store.entry;
    assert_eq!(second.design_id, entry.design_id);
    assert_eq!(second.tags, ["gift"], "tags travel inside the file");
    second.item.title = "Edited In The Library".to_owned();
    let later = "2026-10-27T08:00:00Z";
    let again = export_psdd(&second, &store.attachments, later).unwrap();
    assert_eq!(again.file_name, "trichecker.psdd");
    let loaded = design_from_str(&again.text).unwrap();
    assert_eq!(loaded.metadata.title, "Edited In The Library");
    assert_eq!(loaded.metadata.tags, ["gift"]);
    assert_eq!(loaded.metadata.modified_at, later);
    let first_meta = design_from_str(&first.text).unwrap().metadata;
    assert_eq!(
        loaded.metadata.created_at, first_meta.created_at,
        "the creation time is kept"
    );
}

#[test]
fn an_entry_without_a_design_file_cannot_be_exported() {
    let (entry, _) = stored("trichecker.asc", "Round Brilliant", 1, "Round", 96);
    let error = export_psdd(&entry, &[file("notes.pdf", b"%PDF")], NOW).unwrap_err();
    assert!(error.contains("no design file"), "{error}");
}

#[test]
fn the_meta_id_of_a_design_file_keys_its_entry() {
    let (entry, files) = stored("trichecker.asc", "Round Brilliant", 1, "Round", 96);
    let exported = export_psdd(&entry, &files, NOW).unwrap();
    assert_eq!(
        meta_id_of(&exported.file_name, exported.text.as_bytes()).as_deref(),
        Some(entry.design_id.as_str())
    );
    assert_eq!(meta_id_of("trichecker.asc", exported.text.as_bytes()), None);
    assert_eq!(meta_id_of("broken.psdd", b"not a design"), None);
    assert_eq!(meta_id_of("broken.psdd", &[0xff, 0xfe]), None);
}

#[test]
fn a_thumbnail_stamp_follows_the_file_bytes() {
    assert_eq!(thumbnail_stamp(b"abc"), thumbnail_stamp(b"abc"));
    assert_ne!(thumbnail_stamp(b"abc"), thumbnail_stamp(b"abd"));
    assert!(thumbnail_stamp(b"abc").starts_with("t1:"));
}

#[test]
fn the_import_notice_counts_what_happened() {
    let mut report = ImportReport::default();
    assert!(report.is_empty());
    assert_eq!(report.summary(), "Nothing to import.");
    report.added = 2;
    report.updated = 1;
    report.identical = 1;
    assert_eq!(
        report.summary(),
        "Imported 3 designs (1 replaced an earlier copy). 1 file already in the library."
    );
    assert_eq!(report.severity(), Severity::Success);
    report.unmeasured = 2;
    assert_eq!(report.severity(), Severity::Warning);
    assert!(
        report
            .summary()
            .contains("2 designs are too large to measure")
    );
}

#[test]
fn the_import_notice_names_a_few_failures_and_counts_the_rest() {
    let mut report = ImportReport::default();
    for n in 1..=5 {
        report
            .failed
            .push((format!("bad{n}.asc"), "cannot be read".to_owned()));
    }
    assert_eq!(report.severity(), Severity::Error);
    let text = report.summary();
    assert!(text.starts_with("5 files could not be imported: bad1.asc (cannot be read)"));
    assert!(text.contains("and 2 more."), "{text}");
    assert!(!text.contains("bad4.asc"), "{text}");
    report.added = 1;
    assert_eq!(report.severity(), Severity::Warning);
    report.failed.clear();
    report.out_of_space = true;
    assert_eq!(report.severity(), Severity::Warning);
    assert!(report.summary().contains("storage for this site is full"));
    report.added = 0;
    assert_eq!(report.severity(), Severity::Error);
}

#[test]
fn the_storage_line_says_how_much_of_the_quota_is_used() {
    assert_eq!(human_bytes(512.0), "512 B");
    assert_eq!(human_bytes(2048.0), "2 KB");
    assert_eq!(human_bytes(3.5 * 1024.0 * 1024.0), "3.5 MB");
    assert_eq!(human_bytes(2.0 * 1024.0 * 1024.0 * 1024.0), "2.0 GB");
    assert_eq!(human_bytes(f64::NAN), "0 B");
    assert_eq!(storage_line(1, 0, None), "1 design");
    let estimate = StorageEstimate {
        usage_bytes: 1024.0 * 1024.0,
        quota_bytes: 10.0 * 1024.0 * 1024.0,
    };
    assert_eq!(
        storage_line(12, 0, Some(estimate)),
        "12 designs \u{b7} 1.0 MB of 10.0 MB used"
    );
    let full = StorageEstimate {
        usage_bytes: 95.0,
        quota_bytes: 100.0,
    };
    assert!(storage_line(2, 1, Some(full)).contains("nearly full"));
    assert!(storage_line(2, 1, Some(full)).contains("1 stored in a newer format"));
    let unknown = StorageEstimate {
        usage_bytes: 5.0,
        quota_bytes: 0.0,
    };
    assert_eq!(storage_line(2, 0, Some(unknown)), "2 designs");
}
