//! The surface finish of a remote design: a mirror sync reads it from the design file among the
//! design's attachments, a later revision replaces it, and a design whose file states none (or
//! that has no design file) is mirrored with the ideal polish.

use super::{
    super::pass::run_mirror_sync,
    fixtures::{FakeTransport, design_record_at, design_summary_at, temp_db},
};
use crate::bridge::library::mirror::options::{MirrorOptions, MirrorOutcome};
use pleochroma_formats::native::{
    MaterialTable, NativeMeetConstraint, NativePreformShape, PreformTable, SurfaceColumns,
    TierTable,
    design::{self, DesignFile, ScheduleTable},
};
use pleochroma_net::library::AttachedFileMeta;
use pleochroma_vault::{db::sqlite::Database, model::surface_finish::StoredSurfaceFinish};
use std::sync::{Arc, Mutex, atomic::AtomicBool};

const URL: &str = "https://example.test/finished";

/// The text of a one-tier design file; the tier has a Commercial finish when `finished`.
fn design_text(finished: bool) -> Vec<u8> {
    let mut tier = TierTable::new(
        "C1",
        NativeMeetConstraint::ScaleReference { mast: 0.5 },
        Vec::new(),
    );
    tier.angle_deg = Some(41.0);
    tier.indices = Some(vec![0.0, 24.0, 48.0, 72.0]);
    if finished {
        tier = tier.with_surface(SurfaceColumns {
            surface: Some("Commercial".to_string()),
            ..SurfaceColumns::default()
        });
    }
    let schedule = ScheduleTable {
        gemcad_version: "5.0".to_string(),
        gear_teeth: 96,
        gear_reference_angle: 0.0,
        symmetry_order: 4,
        mirror: true,
        refractive_index: 1.54,
        headers: Vec::new(),
        footnotes: Vec::new(),
        unknown: std::iter::empty().collect(),
    };
    let file = DesignFile::new(
        PreformTable::new(NativePreformShape::Block, 1.0, 1.0, 2.0),
        MaterialTable::new(None, None, None),
        schedule,
        None,
        vec![tier],
    );
    design::to_string(&file).expect("serializes").into_bytes()
}

/// Mirrors the design at `revision`; `file` is the design file attached to it, if any.
fn sync(db: &Arc<Mutex<Database>>, revision: u32, file: Option<Vec<u8>>) {
    let mut record = design_record_at(1, "Finished Brilliant", URL, "v1", revision);
    let mut transport_files = Vec::new();
    if let Some(bytes) = file {
        record.attachments = vec![AttachedFileMeta {
            id: 100,
            name: "finished.psdd".to_string(),
            url: String::new(),
            size: bytes.len() as u64,
        }];
        transport_files.push(bytes);
    }
    let mut transport = FakeTransport::new(
        vec![design_summary_at(
            1,
            "Finished Brilliant",
            URL,
            "v1",
            revision,
        )],
        vec![record],
    );
    for bytes in transport_files {
        transport = transport.with_attachment(100, "finished.psdd", bytes);
    }
    let outcome = run_mirror_sync(
        db,
        &transport,
        "remote-library:w",
        MirrorOptions::default(),
        &AtomicBool::new(false),
        |_| {},
    );
    assert!(
        matches!(outcome, MirrorOutcome::Completed(ref c) if c.failed == 0),
        "{outcome:?}"
    );
}

fn stored(db: &Arc<Mutex<Database>>) -> StoredSurfaceFinish {
    let guard = db.lock().unwrap();
    let id = guard
        .diagram_entry_id_for_url(URL)
        .unwrap()
        .expect("the design was mirrored");
    guard.diagram_surface_finish(id).unwrap()
}

#[test]
fn a_mirror_sync_reads_the_finish_from_the_attached_design_file_and_a_later_revision_replaces_it() {
    let (db, path) = temp_db();
    sync(&db, 0, Some(design_text(true)));
    let finish = stored(&db);
    assert_eq!(finish.tiers.len(), 1);
    assert_eq!(finish.tiers[0].grade.surface.as_deref(), Some("Commercial"));

    // The remote design was re-cut and its file now states no finish: replaced, not kept.
    sync(&db, 1, Some(design_text(false)));
    assert!(stored(&db).is_empty());

    // And a design that has no design file at all is mirrored with the ideal polish.
    sync(&db, 2, Some(design_text(true)));
    assert!(!stored(&db).is_empty());
    sync(&db, 3, None);
    assert!(stored(&db).is_empty());

    std::fs::remove_file(&path).ok();
}
