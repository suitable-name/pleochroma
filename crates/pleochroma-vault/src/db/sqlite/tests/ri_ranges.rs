//! The stored RI ranges (`ri_ranges`): the round trip, replacing a row, the "Mine" list, the
//! entries missing a range or holding one of another threshold version, and the cascade on
//! delete.

use super::super::*;
use crate::model::ri_range::StoredRiRange;

fn in_memory_db() -> Database {
    Database::new(Some(":memory:")).expect("create in-memory db")
}

/// Saves one entry at `url` with an (empty) detail row and returns its id.
fn save_entry(db: &Database, title: &str, url: &str) -> i64 {
    db.save_design(
        &FacetingDiagramEntry {
            title: title.to_string(),
            url: url.to_string(),
            design_id: String::new(),
        },
        &FacetingDiagramDetail::default(),
        "local-import",
    )
    .expect("save design")
}

/// Saves a "Mine" entry (a `local://` url) and returns its id.
fn save_mine(db: &Database, title: &str) -> i64 {
    save_entry(
        db,
        title,
        &format!("local://{}.asc", title.to_lowercase().replace(' ', "-")),
    )
}

fn range(entry_id: i64, version: i64, low: Option<f64>, high: Option<f64>) -> StoredRiRange {
    StoredRiRange {
        entry_id,
        planes_hash: 1000 + entry_id,
        threshold_version: version,
        n_low: low,
        n_high: high,
    }
}

#[test]
fn a_design_without_a_range_reads_as_none() {
    let db = in_memory_db();
    let id = save_mine(&db, "Plain");
    assert_eq!(db.ri_range(id).unwrap(), None);
    assert_eq!(db.ri_range(id + 1000).unwrap(), None);
}

#[test]
fn put_then_get_round_trips_bounded_and_open_ranges() {
    let db = in_memory_db();
    let a = save_mine(&db, "Alpha");
    let b = save_mine(&db, "Beta");
    let bounded = range(a, 1, Some(1.55), Some(1.9));
    let low_only = range(b, 1, Some(1.6), None);
    db.put_ri_range(&bounded).unwrap();
    db.put_ri_range(&low_only).unwrap();

    assert_eq!(db.ri_range(a).unwrap(), Some(bounded));
    assert_eq!(db.ri_range(b).unwrap(), Some(low_only));
}

#[test]
fn put_replaces_the_row_of_the_same_design() {
    let db = in_memory_db();
    let id = save_mine(&db, "Alpha");
    db.put_ri_range(&range(id, 1, Some(1.5), Some(1.8)))
        .unwrap();
    let newer = StoredRiRange {
        entry_id: id,
        planes_hash: 99,
        threshold_version: 2,
        n_low: Some(1.52),
        n_high: None,
    };
    db.put_ri_range(&newer).unwrap();

    assert_eq!(db.ri_range(id).unwrap(), Some(newer));
    let rows: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM ri_ranges", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1, "one row per design");
}

#[test]
fn put_for_a_missing_entry_is_refused() {
    let db = in_memory_db();
    assert!(db.put_ri_range(&range(4242, 1, Some(1.5), None)).is_err());
}

#[test]
fn mine_lists_only_local_entries_of_the_version_with_their_names() {
    let db = in_memory_db();
    let beta = save_mine(&db, "beta");
    let alpha = save_mine(&db, "Alpha");
    let catalogue = save_entry(&db, "Catalogue", "https://example.org/catalogue");
    let stale = save_mine(&db, "Stale");
    let hidden = save_mine(&db, "Hidden");
    for (id, version) in [
        (beta, 3),
        (alpha, 3),
        (catalogue, 3),
        (stale, 2),
        (hidden, 3),
    ] {
        db.put_ri_range(&range(id, version, Some(1.5), Some(1.9)))
            .unwrap();
    }
    db.set_diagram_ignored(hidden, true).unwrap();

    let mine = db.mine_ri_ranges(3).unwrap();
    let listed: Vec<(i64, &str)> = mine
        .iter()
        .map(|(id, name, _)| (*id, name.as_str()))
        .collect();
    assert_eq!(
        listed,
        vec![(alpha, "Alpha"), (beta, "beta")],
        "local, current version, not ignored, ordered by name"
    );
    for (id, _, stored) in &mine {
        assert_eq!(stored.entry_id, *id);
        assert_eq!(stored.threshold_version, 3);
        assert_eq!(*stored, db.ri_range(*id).unwrap().unwrap());
    }
    assert!(db.mine_ri_ranges(9).unwrap().is_empty());
}

#[test]
fn entries_missing_a_range_cover_no_row_and_another_version() {
    let db = in_memory_db();
    let current = save_mine(&db, "Current");
    let none = save_mine(&db, "None");
    let stale = save_mine(&db, "Stale");
    let catalogue = save_entry(&db, "Catalogue", "https://example.org/catalogue");
    let hidden = save_mine(&db, "Hidden");
    db.put_ri_range(&range(current, 5, Some(1.5), None))
        .unwrap();
    db.put_ri_range(&range(stale, 4, Some(1.5), None)).unwrap();
    db.set_diagram_ignored(hidden, true).unwrap();

    assert_eq!(
        db.entries_missing_ri_range(5).unwrap(),
        vec![none, stale],
        "no row, or a row of another version; never a catalogue or ignored entry"
    );
    assert!(!db.entries_missing_ri_range(5).unwrap().contains(&catalogue));

    // Recomputing the stale row at the current version removes it from the list.
    db.put_ri_range(&range(stale, 5, Some(1.6), None)).unwrap();
    assert_eq!(db.entries_missing_ri_range(5).unwrap(), vec![none]);
    // A new threshold version makes every row stale again.
    assert_eq!(
        db.entries_missing_ri_range(6).unwrap(),
        vec![current, none, stale]
    );
}

#[test]
fn a_range_goes_with_its_entry() {
    let db = in_memory_db();
    let id = save_mine(&db, "Alpha");
    db.put_ri_range(&range(id, 1, Some(1.5), None)).unwrap();
    db.delete_diagram_entry(id).unwrap();

    assert_eq!(db.ri_range(id).unwrap(), None);
    assert!(db.mine_ri_ranges(1).unwrap().is_empty());
}
