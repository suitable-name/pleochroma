use super::*;
use pleochroma_cut_core::native::DesignMetadata;

const BOX_ASC: &str = "GemCad 5.0\n\
g 4 0.0\n\
y 1 n\n\
I 1.62\n\
H Recorded design\n\
a 90.000000 1.00000000 0 1 2 3 n G G Set girdle thickness\n\
a 0.000000 0.60000000 n T G Meet G\n\
a -0.000000 0.55000000 n C G Set stone size\n";

fn entry() -> Entry {
    Entry {
        id: 42,
        title: "Library title".into(),
        metadata: DesignMetadata {
            title: "Library title".into(),
            designer: "Recorded designer".into(),
            source_url: "https://example.com/cut".into(),
            notes: "Original catalogue notes.".into(),
            ..DesignMetadata::default()
        },
    }
}

fn source(name: &str, bytes: &[u8]) -> Source {
    Source {
        id: 7,
        name: name.into(),
        url: "https://example.com/cut.asc".into(),
        bytes: bytes.to_vec(),
    }
}

#[test]
fn asc_keeps_original_depths_meet_notes_and_source_bytes() {
    let input = source("../unsafe\\design.ASC", BOX_ASC.as_bytes());
    let converted = convert(&entry(), &input, false).unwrap();
    assert!(!converted.draft);
    assert_eq!(converted.adopted_meets, 0);
    let loaded = design_from_str(&converted.text).unwrap();
    let schedule = asc::parse_asc(BOX_ASC).unwrap();
    verify_pinned_masts(&loaded.design, &schedule).unwrap();
    assert!(loaded.design.tiers[1].imported_meet.is_some());
    assert_eq!(
        loaded.design.tiers[1].original_notes.as_deref(),
        Some("Meet G")
    );
    let mut metadata = loaded.metadata.clone();
    assert!(metadata.notes.starts_with("Original catalogue notes.\n\n"));
    assert!(metadata.notes.contains("not measured rough"));
    metadata.notes = entry().metadata.notes;
    assert_eq!(metadata, entry().metadata);
    assert_eq!(loaded.attachments[0].name, "source-7.asc");
    assert_eq!(loaded.attachments[0].data, input.bytes);
    assert_eq!(loaded.attachments[0].source_url, input.url);
    assert!(loaded.design.girdle_diameter_mm.is_none());
    assert!(loaded.design.material.name.is_none());
    assert_eq!(loaded.design.meta.refractive_index, 1.62);
}

#[test]
fn native_without_adoption_is_copied_exactly_including_comments_and_future_keys() {
    let converted = convert(&entry(), &source("box.asc", BOX_ASC.as_bytes()), false).unwrap();
    let native = format!(
        "# Original comment\nfuture_field = 'keep this'\n{}",
        converted.text
    );
    let copied = convert(&entry(), &source("box.psdd", native.as_bytes()), false).unwrap();
    assert_eq!(copied.text, native);
    assert!(!copied.draft);
}

#[test]
fn native_adoption_preserves_custom_material_history_attachments_and_unknown_fields() {
    let input = "GemCad 5.0\ng 96 0\ny 1 n\nI 1.62\n\
a 45 0.5 0 n C0 G Set stone size\n\
a 45 0.5 24 n C1 G Meet C0, C2, C3\n\
a 45 0.5 48 n C2 G Set stone size\n\
a 45 0.5 72 n C3 G Set stone size\n\
a -45 0.5 0 24 48 72 n P G Set stone size\n";
    let converted = convert(&entry(), &source("octahedron.asc", input.as_bytes()), false).unwrap();
    let mut file = native_format::parse(&converted.text).unwrap();
    file.material.name = Some("Original custom material".into());
    file.material.custom = Some(pleochroma_formats::native::CustomMaterialSnapshot::new(
        1.62,
        0.02,
        0.0,
        Some(3.0),
        "Cubic",
        "Isotropic",
    ));
    file.history = Some(pleochroma_formats::native::HistoryTable::new(
        (0..300)
            .map(|index| format!("Original edit {index}"))
            .collect(),
    ));
    file.unknown
        .insert("future_document_key".into(), "retain".into());
    file.tiers[1]
        .unknown
        .insert("future_tier_key".into(), "retain".into());
    file.tiers[1].frosted = true;
    let text = native_format::to_string(&file).unwrap();
    let converted = convert(&entry(), &source("octahedron.psdd", text.as_bytes()), true).unwrap();
    assert_eq!(converted.adopted_meets, 1, "{:?}", converted.warnings);
    assert!(!converted.draft);
    let output = native_format::parse(&converted.text).unwrap();
    assert_eq!(output.tiers[1].unknown, file.tiers[1].unknown);
    assert!(output.tiers[1].frosted);
    // The entire document must be unchanged apart from the adopted constraint.
    file.tiers[1].constraint = output.tiers[1].constraint.clone();
    file.version = output.version;
    assert_eq!(output, file);
}

#[test]
fn incomplete_schedule_is_kept_as_a_draft_instead_of_closed_by_guessed_rough() {
    let asc = "GemCad 5.0\ng 4 0\ny 1 n\nI 1.62\na 0 0.6 G Set stone size\n";
    let converted = convert(&entry(), &source("incomplete.asc", asc.as_bytes()), true).unwrap();
    assert!(converted.draft);
    assert!(design_from_str(&converted.text).unwrap().draft);
    assert_eq!(converted.adopted_meets, 0);
}

#[test]
fn unsolvable_native_is_marked_as_a_draft_without_discarding_other_fields() {
    let converted = convert(&entry(), &source("box.asc", BOX_ASC.as_bytes()), false).unwrap();
    let mut file = native_format::parse(&converted.text).unwrap();
    for tier in &mut file.tiers {
        tier.constraint = pleochroma_formats::native::NativeMeetConstraint::MeetExisting;
    }
    let input = native_format::to_string(&file).unwrap();
    let converted = convert(&entry(), &source("unsolved.psdd", input.as_bytes()), true).unwrap();
    assert!(converted.draft);
    assert_eq!(converted.adopted_meets, 0);
    let output = native_format::parse(&converted.text).unwrap();
    file.draft = true;
    assert_eq!(output, file);
}

#[test]
fn inferred_rough_encloses_a_cut_larger_than_the_editor_default() {
    let asc = BOX_ASC.replace("1.00000000", "12.00000000");
    let converted = convert(&entry(), &source("large.asc", asc.as_bytes()), false).unwrap();
    assert!(!converted.draft);
    let loaded = design_from_str(&converted.text).unwrap();
    assert!(loaded.design.preform.half_width > 12.0);
    assert!(loaded.design.preform.half_width * loaded.design.preform.length_over_width > 12.0);
}

#[test]
fn empty_and_malformed_attachments_fail_without_a_native_document() {
    assert!(convert(&entry(), &source("empty.asc", &[]), false).is_err());
    assert!(convert(&entry(), &source("bad.psdd", b"not a document"), false).is_err());
}
