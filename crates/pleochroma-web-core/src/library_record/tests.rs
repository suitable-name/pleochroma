use super::*;
use pleochroma_vault::local::ImportedExtras;

const ID_A: &str = "886313e1-3b8a-5372-9b90-0c9aee199e5d";

fn file(name: &str, bytes: &[u8]) -> AttachedFile {
    AttachedFile {
        name: name.to_owned(),
        url: String::new(),
        content: bytes.to_vec(),
    }
}

fn parsed(file_name: &str, bytes: &[u8]) -> ImportedAsc {
    ImportedAsc {
        entry: FacetingDiagramEntry {
            title: "Round".to_owned(),
            url: local_url(file_name),
            design_id: String::new(),
        },
        detail: FacetingDiagramDetail {
            shape: Some("Round".to_owned()),
            lw_ratio: Some("1.0".to_owned()),
            attached_files: vec![file(file_name, bytes)],
            diagram_image_data: Some(vec![1, 2, 3]),
            ..FacetingDiagramDetail::default()
        },
        derived_from_entry_id: None,
        extras: ImportedExtras::default(),
    }
}

fn stored(plan: ImportPlan) -> StoreImport {
    match plan {
        ImportPlan::Store(store) => *store,
        ImportPlan::Identical { .. } => panic!("expected a store plan"),
    }
}

#[test]
fn a_failure_text_is_classified_by_the_exception_name() {
    assert_eq!(
        classify_error_text("QuotaExceededError: The quota has been exceeded."),
        StoreErrorKind::Quota
    );
    for text in [
        "IndexedDB is unavailable",
        "SecurityError: The operation is insecure.",
        "InvalidStateError: A mutation operation was attempted on a database that did not allow mutations.",
        "no window",
    ] {
        assert_eq!(
            classify_error_text(text),
            StoreErrorKind::Unavailable,
            "{text}"
        );
    }
    assert_eq!(
        classify_error_text("ConstraintError: key exists"),
        StoreErrorKind::Other
    );
    assert!(StoreError::from_text("QuotaExceededError: full").is_quota());
    assert!(!StoreError::from_text("DataError: bad key").is_quota());
}

#[test]
fn the_user_text_never_blames_the_open_design() {
    let quota = StoreError::from_text("QuotaExceededError: full").user_text();
    assert!(quota.contains("not affected"), "{quota}");
    let private = StoreError::from_text("IndexedDB is unavailable").user_text();
    assert!(private.contains("not affected"), "{private}");
}

#[test]
fn a_design_id_is_the_meta_id_when_valid_else_the_url_uuid() {
    assert_eq!(
        design_id_for(
            "local://a.asc",
            Some("886313E1-3B8A-5372-9B90-0C9AEE199E5D")
        ),
        ID_A
    );
    let by_url = design_id_for("local://a.asc", None);
    assert_eq!(by_url, catalogue_design_uuid("local://a.asc"));
    assert_eq!(by_url, design_id_for("local://a.asc", Some("not-a-uuid")));
    assert_ne!(by_url, design_id_for("local://b.asc", None));
    assert!(require_design_id("nope").is_err());
    assert_eq!(require_design_id(&format!(" {ID_A} ")).unwrap(), ID_A);
}

#[test]
fn side_keys_sort_by_variant_id_and_prefix_ranges_fence_one_design() {
    let one = side_key(ID_A, SideKind::Variant, Some(2));
    let ten = side_key(ID_A, SideKind::Variant, Some(10));
    assert!(one < ten, "zero padding keeps numeric order");
    assert_eq!(
        side_key(ID_A, SideKind::CutProgress, None),
        format!("{ID_A}/cut")
    );
    let (low, high) = design_prefix_bounds(ID_A);
    assert_eq!(
        (low.as_str(), high.as_str()),
        (&*format!("{ID_A}/"), &*format!("{ID_A}0"))
    );
    for key in [
        one.clone(),
        side_key(ID_A, SideKind::Lighting, None),
        side_key(ID_A, SideKind::CutProgress, None),
    ] {
        assert!(key >= low && key < high, "{key}");
    }
    let other = side_key(
        "00000000-0000-4000-8000-000000000000",
        SideKind::Lighting,
        None,
    );
    assert!(other < low || other >= high);
    assert!(
        ID_A < low.as_str(),
        "the bare design id is outside the range"
    );
    let (vlow, vhigh) = variant_prefix_bounds(ID_A);
    assert!(one >= vlow && one < vhigh);
    // The other kinds sort on either side of "variant" ("cut" and "lighting" come first), and
    // none of them is inside the variant range.
    for kind in [SideKind::CutProgress, SideKind::Lighting] {
        let key = side_key(ID_A, kind, None);
        assert!(key < vlow || key >= vhigh, "{key} is outside the variants");
    }
}

#[test]
fn tags_are_trimmed_unique_and_sorted() {
    assert_eq!(
        normalize_tags(["  step ", "", "Brilliant", "step", "alpha"]),
        vec!["Brilliant", "alpha", "step"]
    );
}

#[test]
fn a_new_import_is_keyed_by_the_url_uuid_and_keeps_bytes_out_of_the_summary() {
    let mut p = parsed("round.asc", b"asc text");
    p.extras.tags = vec!["round".to_owned(), "round".to_owned()];
    p.extras.ignored = true;
    let store = stored(plan_import(p, None, None, 1000.0, true));
    let entry = &store.entry;
    assert_eq!(entry.design_id, catalogue_design_uuid("local://round.asc"));
    assert_eq!(entry.item.id, 0);
    assert_eq!(entry.item.url, "local://round.asc");
    assert_eq!(entry.item.shape.as_deref(), Some("Round"));
    assert!(entry.item.ignored);
    assert_eq!(entry.tags, vec!["round"]);
    assert!(entry.detail.attached_files.is_empty());
    assert!(entry.detail.diagram_image_data.is_none());
    assert_eq!(entry.design_file.as_deref(), Some("round.asc"));
    assert_eq!(store.attachments.len(), 1);
    assert!(!store.replaced);
    assert!(entry.is_local());
    assert_eq!((entry.added_ms, entry.updated_ms), (1000.0, 1000.0));
}

#[test]
fn the_meta_id_keys_a_new_import() {
    let store = stored(plan_import(
        parsed("a.asc", b"x"),
        Some(ID_A),
        None,
        5.0,
        true,
    ));
    assert_eq!(store.entry.design_id, ID_A);
}

#[test]
fn the_same_bytes_under_the_same_url_are_identical_and_other_bytes_replace() {
    let first = stored(plan_import(
        parsed("round.asc", b"v1"),
        None,
        None,
        10.0,
        true,
    ));
    let mut old = first.entry.clone();
    old.item.id = 7;
    old.tags = vec!["keep".to_owned()];
    old.detail.designer = Some("Typed By Hand".to_owned());
    let files = first.attachments;

    let same = plan_import(
        parsed("round.asc", b"v1"),
        None,
        Some((&old, &files)),
        20.0,
        true,
    );
    assert!(matches!(same, ImportPlan::Identical { ref design_id } if *design_id == old.design_id));

    let again = stored(plan_import(
        parsed("round.asc", b"v1"),
        None,
        Some((&old, &files)),
        20.0,
        false,
    ));
    assert!(again.replaced, "without skip_identical the row is replaced");

    let mut changed = parsed("round.asc", b"v2");
    changed.extras.tags = vec!["new".to_owned()];
    let replaced = stored(plan_import(changed, None, Some((&old, &files)), 30.0, true));
    let entry = &replaced.entry;
    assert!(replaced.replaced);
    assert_eq!(entry.design_id, old.design_id, "side data stays attached");
    assert_eq!(entry.item.id, 7);
    assert_eq!(entry.added_ms, 10.0);
    assert_eq!(entry.updated_ms, 30.0);
    assert_eq!(entry.tags, vec!["keep", "new"]);
    assert_eq!(entry.detail.designer.as_deref(), Some("Typed By Hand"));
    assert_eq!(replaced.attachments[0].content, b"v2");
}

#[test]
fn the_full_record_carries_the_detail_and_the_attachments() {
    let store = stored(plan_import(
        parsed("round.asc", b"x"),
        None,
        None,
        1.0,
        true,
    ));
    let record = store.entry.full_record(store.attachments.clone());
    assert_eq!(record.url, "local://round.asc");
    assert_eq!(record.shape.as_deref(), Some("Round"));
    assert_eq!(record.attached_files.len(), 1);
}

#[test]
fn refreshing_the_item_follows_the_detail_and_keeps_id_and_ignored() {
    let store = stored(plan_import(
        parsed("round.asc", b"x"),
        None,
        None,
        1.0,
        true,
    ));
    let mut entry = store.entry;
    entry.item.id = 4;
    entry.item.ignored = true;
    entry.detail.shape = Some("Oval".to_owned());
    entry.refresh_item();
    assert_eq!(entry.item.shape.as_deref(), Some("Oval"));
    assert_eq!((entry.item.id, entry.item.ignored), (4, true));
    assert_eq!(entry.item.url, "local://round.asc");
    assert_eq!(entry.item.title, "Round");
}

#[test]
fn an_entry_becomes_a_search_record_with_tags_ignored_and_seconds() {
    let mut p = parsed("round.asc", b"x");
    p.detail.refractive_index = Some("1.54".to_owned());
    p.detail.facets_count = Some("57".to_owned());
    p.extras.tags = vec!["Round".to_owned()];
    let mut entry = stored(plan_import(p, None, None, 5000.0, true)).entry;
    entry.item.id = 9;
    entry.item.ignored = true;
    entry.updated_ms = 7500.0;
    let record = entry.search_record();
    assert_eq!(record.id, 9);
    assert_eq!(record.title, "Round");
    assert_eq!(record.url, "local://round.asc");
    assert_eq!(record.tags, vec!["Round"]);
    assert!(record.ignored);
    assert_eq!(record.refractive_index, Some(1.54));
    assert_eq!((record.created_at, record.updated_at), (Some(5), Some(7)));
    assert_eq!(ms_to_unix_seconds(-1.0), 0);
    assert_eq!(ms_to_unix_seconds(f64::NAN), 0);
}

#[test]
fn a_thumbnail_must_be_a_small_png() {
    let mut png = PNG_SIGNATURE.to_vec();
    png.extend_from_slice(&[0; 16]);
    assert!(check_thumbnail(&png).is_ok());
    assert!(check_thumbnail(&[]).is_err());
    assert!(check_thumbnail(b"GIF89a").is_err());
    let mut big = PNG_SIGNATURE.to_vec();
    big.resize(MAX_THUMBNAIL_BYTES + 1, 0);
    assert!(check_thumbnail(&big).is_err());
}

#[test]
fn a_new_variant_is_trimmed_and_refused_without_a_name_or_text() {
    let new = NewVariant {
        name: "  Deep pavilion ",
        parent_variant_id: Some(1),
        note: Some("  "),
        design_text: "[meta]",
        thumbnail_png: None,
        created_at: 99,
    };
    let record = VariantRecord::from_new(&new, 5).unwrap();
    assert_eq!(record.name, "Deep pavilion");
    assert_eq!(record.note, None);
    assert_eq!(record.variant_id, 5);
    let nameless = NewVariant { name: " ", ..new };
    assert!(VariantRecord::from_new(&nameless, 6).is_err());
    let empty = NewVariant {
        design_text: "",
        ..new
    };
    assert!(VariantRecord::from_new(&empty, 6).is_err());
    let variant = record.into_variant(ID_A);
    assert_eq!(variant.summary.design_uuid, ID_A);
    assert_eq!(variant.design_text, "[meta]");
}

#[test]
fn variant_summaries_are_in_id_order_and_a_missing_parent_reads_as_none() {
    let record = |variant_id, parent| VariantRecord {
        variant_id,
        name: format!("v{variant_id}"),
        parent_variant_id: parent,
        created_at: 0,
        note: None,
        design_text: "x".to_owned(),
    };
    let list = variant_summaries(
        ID_A,
        vec![record(3, Some(2)), record(2, Some(1)), record(1, None)],
    );
    assert_eq!(
        list.iter().map(|v| v.variant_id).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(list[1].parent_variant_id, Some(1));
    assert_eq!(list[2].parent_variant_id, Some(2));
    let orphaned = variant_summaries(ID_A, vec![record(3, Some(2))]);
    assert_eq!(orphaned[0].parent_variant_id, None);
}

#[test]
fn marks_replace_by_step_key_and_remove_by_step_key() {
    let mark = |key: &str, signature: &str| CutProgressMark {
        step_key: key.to_owned(),
        step_signature: signature.to_owned(),
        done_at: 1,
    };
    let mut marks = Vec::new();
    apply_mark(&mut marks, &mark("P1", "a")).unwrap();
    apply_mark(&mut marks, &mark("P2", "b")).unwrap();
    apply_mark(&mut marks, &mark("P1", "c")).unwrap();
    assert_eq!(marks.len(), 2);
    assert_eq!(marks[0].step_signature, "c");
    assert!(apply_mark(&mut marks, &mark(" ", "x")).is_err());
    assert!(remove_mark(&mut marks, "P1"));
    assert!(!remove_mark(&mut marks, "P1"));
    assert_eq!(marks.len(), 1);
}

#[test]
fn records_round_trip_through_json() {
    let store = stored(plan_import(
        parsed("round.asc", b"x"),
        None,
        None,
        1.0,
        true,
    ));
    let json = serde_json::to_string(&store.entry).unwrap();
    let back: LibraryEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(back.design_id, store.entry.design_id);
    assert_eq!(back.item.shape, store.entry.item.shape);
    assert_eq!(back.version, RECORD_VERSION);
    let lighting = LightingRecord {
        preset_name: "Studio".to_owned(),
        settings_json: "{}".to_owned(),
        updated_at: 3,
    };
    let json = serde_json::to_string(&lighting).unwrap();
    assert_eq!(
        serde_json::from_str::<LightingRecord>(&json).unwrap(),
        lighting
    );
    let value: DesignLighting = lighting.into();
    assert_eq!(value.preset_name, "Studio");
}

#[test]
fn the_storage_estimate_reports_the_used_share() {
    let estimate = StorageEstimate {
        usage_bytes: 95.0,
        quota_bytes: 100.0,
    };
    assert!(estimate.is_nearly_full());
    assert_eq!(estimate.bytes_free(), 5.0);
    let unknown = StorageEstimate {
        usage_bytes: 50.0,
        quota_bytes: 0.0,
    };
    assert_eq!(unknown.fraction_used(), 0.0);
    assert!(!unknown.is_nearly_full());
}

#[test]
fn an_import_never_puts_two_entries_on_one_url() {
    // A was found by its `[meta]` id, B owns the url the new file would take.
    let a = stored(plan_import(
        parsed("a.asc", b"1"),
        Some(ID_A),
        None,
        1.0,
        true,
    ))
    .entry;
    let b = stored(plan_import(parsed("b.asc", b"2"), None, None, 1.0, true)).entry;
    assert_ne!(a.design_id, b.design_id);
    let owner = url_taken_by_other(Some(&a), Some(&b)).expect("B is another entry");
    assert_eq!(owner.design_id, b.design_id);
    // The same entry found twice, an import with no id match, and a free url are all fine.
    assert!(url_taken_by_other(Some(&a), Some(&a)).is_none());
    assert!(url_taken_by_other(None, Some(&b)).is_none());
    assert!(url_taken_by_other(Some(&a), None).is_none());
}
