//! The surface finish of a library design (`diagram_surface_finish`, `diagram_edge_abrasion`):
//! the round trip, replacing and clearing it, the cascade on delete, the migration that adds
//! the tables to an old database and backfills them from the stored design files, the
//! read-only reader, and that a design without a finish leaves no row.

use super::{super::*, fixtures::temp_db_path};
use crate::{
    local::{LOCAL_SOURCE_ID, apply_imported_extras, import_native_design, surface_design_text},
    model::surface_finish::{
        StoredEdgeAbrasion, StoredSurfaceFinish, StoredSurfaceGrade, StoredTierSurface,
    },
};

fn in_memory_db() -> Database {
    Database::new(Some(":memory:")).expect("create in-memory db")
}

/// Saves one local entry with an (empty) detail row and returns its id.
fn save_entry(db: &Database, title: &str) -> i64 {
    db.save_design(
        &FacetingDiagramEntry {
            title: title.to_string(),
            url: format!("local://{}.asc", title.to_lowercase().replace(' ', "-")),
            design_id: String::new(),
        },
        &FacetingDiagramDetail::default(),
        LOCAL_SOURCE_ID,
    )
    .expect("save design")
}

fn grade(name: &str) -> StoredSurfaceGrade {
    StoredSurfaceGrade {
        surface: Some(name.to_string()),
        ..StoredSurfaceGrade::default()
    }
}

fn sample() -> StoredSurfaceFinish {
    StoredSurfaceFinish {
        tiers: vec![
            StoredTierSurface {
                tier: 1,
                grade: grade("Commercial"),
                facets: vec![(0, StoredSurfaceGrade::default()), (3, grade("Fine"))],
            },
            StoredTierSurface {
                tier: 4,
                grade: StoredSurfaceGrade {
                    surface: Some("Custom".to_string()),
                    rq_nm: Some(3.5),
                    lap_deg: Some(90.0),
                    aniso: Some(2.0),
                    no_sweep: true,
                    ground_alpha: Some(0.25),
                },
                facets: Vec::new(),
            },
            // Only a facet override: the tier's own grade stays ideal.
            StoredTierSurface {
                tier: 6,
                grade: StoredSurfaceGrade::default(),
                facets: vec![(2, grade("Mirror"))],
            },
        ],
        edge: Some(StoredEdgeAbrasion {
            width: 0.02,
            alpha: 0.3,
            halo_rq_nm: 4.0,
        }),
    }
}

fn row_counts(db: &Database) -> (i64, i64) {
    let count = |table: &str| {
        db.conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    (
        count("diagram_surface_finish"),
        count("diagram_edge_abrasion"),
    )
}

#[test]
fn a_design_without_a_finish_reads_as_ideal_and_leaves_no_row() {
    let db = in_memory_db();
    let id = save_entry(&db, "Plain");
    assert!(db.diagram_surface_finish(id).unwrap().is_empty());
    assert!(db.all_diagram_surface_finish().unwrap().is_empty());
    assert_eq!(row_counts(&db), (0, 0));

    // Writing the empty finish stays row-free too.
    db.set_diagram_surface_finish(id, &StoredSurfaceFinish::default())
        .unwrap();
    assert_eq!(row_counts(&db), (0, 0));
}

#[test]
fn the_finish_round_trips_is_replaced_and_is_cleared() {
    let db = in_memory_db();
    let id = save_entry(&db, "Finished");
    db.set_diagram_surface_finish(id, &sample()).unwrap();
    assert_eq!(db.diagram_surface_finish(id).unwrap(), sample());
    // Tier 1: its own grade and two facets; tier 4: its own grade; tier 6: one facet (its own
    // grade is ideal and writes no row); and the one edge row.
    assert_eq!(row_counts(&db), (5, 1));

    // Replacing drops what the new finish leaves out, edge included.
    let smaller = StoredSurfaceFinish {
        tiers: vec![StoredTierSurface {
            tier: 2,
            grade: grade("Prepolish"),
            facets: Vec::new(),
        }],
        edge: None,
    };
    db.set_diagram_surface_finish(id, &smaller).unwrap();
    assert_eq!(db.diagram_surface_finish(id).unwrap(), smaller);
    assert_eq!(row_counts(&db), (1, 0));

    db.set_diagram_surface_finish(id, &StoredSurfaceFinish::default())
        .unwrap();
    assert!(db.diagram_surface_finish(id).unwrap().is_empty());
    assert_eq!(row_counts(&db), (0, 0));
}

#[test]
fn the_order_and_repeats_of_the_lists_do_not_matter() {
    let db = in_memory_db();
    let id = save_entry(&db, "Shuffled");
    let shuffled = StoredSurfaceFinish {
        tiers: vec![
            StoredTierSurface {
                tier: 4,
                grade: grade("Fine"),
                facets: vec![(5, grade("Mirror")), (1, grade("Fine")), (5, grade("Fine"))],
            },
            StoredTierSurface {
                tier: 0,
                grade: grade("Commercial"),
                facets: Vec::new(),
            },
        ],
        edge: None,
    };
    db.set_diagram_surface_finish(id, &shuffled).unwrap();
    let read = db.diagram_surface_finish(id).unwrap();
    let tiers: Vec<u32> = read.tiers.iter().map(|t| t.tier).collect();
    assert_eq!(tiers, [0, 4]);
    assert_eq!(
        read.tiers[1].facets,
        vec![(1, grade("Fine")), (5, grade("Fine"))],
        "a later entry for a slot wins"
    );
}

#[test]
fn the_finish_does_not_touch_the_entry_row_and_other_designs_keep_their_own() {
    let db = in_memory_db();
    let a = save_entry(&db, "A");
    let b = save_entry(&db, "B");
    let stamp = |id: i64| -> Option<i64> {
        db.conn
            .query_row(
                "SELECT updated_at FROM diagram_entries WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap()
    };
    let before = stamp(a);
    db.set_diagram_surface_finish(a, &sample()).unwrap();
    assert_eq!(stamp(a), before, "the stamp does not move");
    assert!(db.diagram_surface_finish(b).unwrap().is_empty());

    db.set_diagram_surface_finish(b, &StoredSurfaceFinish::default())
        .unwrap();
    assert_eq!(db.diagram_surface_finish(a).unwrap(), sample());
}

#[test]
fn all_diagram_surface_finish_equals_the_per_design_reads() {
    let db = in_memory_db();
    let a = save_entry(&db, "A");
    let b = save_entry(&db, "B");
    let edge_only = save_entry(&db, "Edge Only");
    let plain = save_entry(&db, "Plain");
    db.set_diagram_surface_finish(a, &sample()).unwrap();
    db.set_diagram_surface_finish(
        b,
        &StoredSurfaceFinish {
            tiers: vec![StoredTierSurface {
                tier: 0,
                grade: grade("Fine"),
                facets: Vec::new(),
            }],
            edge: None,
        },
    )
    .unwrap();
    db.set_diagram_surface_finish(
        edge_only,
        &StoredSurfaceFinish {
            tiers: Vec::new(),
            edge: Some(StoredEdgeAbrasion {
                width: 0.01,
                alpha: 0.2,
                halo_rq_nm: 0.0,
            }),
        },
    )
    .unwrap();

    let all = db.all_diagram_surface_finish().unwrap();
    assert_eq!(all.len(), 3);
    for id in [a, b, edge_only] {
        assert_eq!(
            all[&id],
            db.diagram_surface_finish(id).unwrap(),
            "entry {id}"
        );
    }
    assert!(!all.contains_key(&plain));
}

#[test]
fn a_stored_finish_for_a_missing_entry_is_refused_and_changes_nothing() {
    let db = in_memory_db();
    let id = save_entry(&db, "Kept");
    db.set_diagram_surface_finish(id, &sample()).unwrap();
    assert!(
        db.set_diagram_surface_finish(id + 1000, &sample()).is_err(),
        "the foreign key refuses an entry that does not exist"
    );
    assert_eq!(db.diagram_surface_finish(id).unwrap(), sample());
    // An empty finish for a missing entry writes nothing, so it is not an error.
    db.set_diagram_surface_finish(id + 1000, &StoredSurfaceFinish::default())
        .unwrap();
}

#[test]
fn deleting_the_entry_removes_its_finish() {
    let db = in_memory_db();
    let id = save_entry(&db, "Gone");
    db.set_diagram_surface_finish(id, &sample()).unwrap();
    db.conn
        .execute("DELETE FROM diagram_entries WHERE id = ?1", params![id])
        .unwrap();
    assert_eq!(row_counts(&db), (0, 0));
    assert!(db.all_diagram_surface_finish().unwrap().is_empty());
}

#[test]
fn a_current_database_that_lacks_the_tables_gets_them_back_on_open() {
    let path = temp_db_path("surface_finish_tables_dropped");
    {
        let db = Database::new(Some(path.to_str().unwrap())).expect("create");
        save_entry(&db, "Old");
        db.conn
            .execute_batch("DROP TABLE diagram_surface_finish; DROP TABLE diagram_edge_abrasion;")
            .expect("drop the tables");
        // The state of a read-only open of an older file: no tables, every design ideal.
        assert!(db.diagram_surface_finish(1).unwrap().is_empty());
        assert!(db.all_diagram_surface_finish().unwrap().is_empty());
    }
    let db = Database::new(Some(path.to_str().unwrap())).expect("open + migrate");
    for (table, column) in [
        ("diagram_surface_finish", "slot"),
        ("diagram_surface_finish", "ground_alpha"),
        ("diagram_edge_abrasion", "halo_rq_nm"),
    ] {
        assert!(
            Database::column_exists(&db.conn, table, column).unwrap(),
            "migration must create {table}.{column}"
        );
    }
    db.set_diagram_surface_finish(1, &sample()).unwrap();
    db.migrate_surface_finish_tables()
        .expect("re-run migration");
    assert_eq!(db.diagram_surface_finish(1).unwrap(), sample());
    drop(db);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

#[test]
fn a_read_only_connection_to_a_database_without_the_tables_reads_as_ideal() {
    let path = temp_db_path("surface_finish_read_only_old");
    let db = Database::new(Some(path.to_str().unwrap())).expect("create");
    let id = save_entry(&db, "Served");
    db.conn
        .execute_batch("DROP TABLE diagram_surface_finish; DROP TABLE diagram_edge_abrasion;")
        .expect("drop the tables");

    let ro = Database::open_read_only(path.to_str().unwrap()).expect("open read-only");
    assert!(ro.diagram_surface_finish(id).unwrap().is_empty());
    assert!(ro.all_diagram_surface_finish().unwrap().is_empty());

    drop(ro);
    drop(db);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

/// A design file's tier finish and edge abrasion are stored with the row on import, a re-import
/// of a file that states none clears them, and a library saved before the tables existed is
/// filled once from its stored design files.
#[test]
fn the_finish_follows_the_design_file_and_old_libraries_are_backfilled() {
    let db = in_memory_db();
    let save = |file_name: &str, finished: bool| {
        let text = surface_design_text(finished);
        let imported = import_native_design(file_name, text.as_bytes()).expect("imports");
        let id = db
            .save_design(&imported.entry, &imported.detail, LOCAL_SOURCE_ID)
            .expect("saves");
        apply_imported_extras(&db, id, &imported.extras).expect("applies");
        (id, imported)
    };
    let (finished, imported) = save("finished.psdd", true);
    let (plain, _) = save("plain.psdd", false);

    let stored = db.diagram_surface_finish(finished).unwrap();
    assert_eq!(stored, imported.extras.surface_finish);
    assert_eq!(stored.tiers.len(), 1);
    assert_eq!(stored.tiers[0].grade.surface.as_deref(), Some("Commercial"));
    assert_eq!(stored.edge.map(|e| e.width), Some(0.02));
    assert!(db.diagram_surface_finish(plain).unwrap().is_empty());

    // The state an older database is in: rows exist, the side tables do not.
    db.conn
        .execute_batch("DROP TABLE diagram_surface_finish; DROP TABLE diagram_edge_abrasion;")
        .expect("drops");
    db.migrate_surface_finish_tables().expect("migrates");
    assert_eq!(db.diagram_surface_finish(finished).unwrap(), stored);
    assert!(db.diagram_surface_finish(plain).unwrap().is_empty());

    // The tables are there now: a later open does not rewrite what a save cleared.
    db.set_diagram_surface_finish(finished, &StoredSurfaceFinish::default())
        .unwrap();
    db.migrate_surface_finish_tables().expect("migrates again");
    assert!(db.diagram_surface_finish(finished).unwrap().is_empty());

    // The same design re-imported from a file that states no finish: the row's is replaced.
    db.set_diagram_surface_finish(finished, &stored).unwrap();
    let again = import_native_design("finished.psdd", surface_design_text(false).as_bytes())
        .expect("imports");
    apply_imported_extras(&db, finished, &again.extras).expect("applies");
    assert!(db.diagram_surface_finish(finished).unwrap().is_empty());
}
