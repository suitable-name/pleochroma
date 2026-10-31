//! The full stone choice (pose, free pose, lock, Reason) in `stone_pose_choice`: the guarded
//! migration, the round trip, and what a pose reset keeps.

use super::{super::*, fixtures::temp_db_path};
use crate::model::zoning::{StoneChoiceRow, StonePoseChoiceRow};

fn in_memory_db() -> Database {
    Database::new(Some(":memory:")).expect("create in-memory db")
}

fn choice(layout: u32, stone: u32, pose: u8) -> StoneChoiceRow {
    StoneChoiceRow {
        layout_index: layout,
        stone_index: stone,
        pose,
        free_pose_json: None,
        locked: false,
        reason: String::new(),
    }
}

fn columns(db: &Database) -> Vec<String> {
    let mut stmt = db
        .conn
        .prepare("SELECT name FROM pragma_table_info('stone_pose_choice') ORDER BY cid")
        .unwrap();
    stmt.query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

#[test]
fn a_new_database_has_the_nullable_columns() {
    let db = in_memory_db();
    assert_eq!(
        columns(&db),
        [
            "plan_id",
            "layout_index",
            "stone_index",
            "pose",
            "free_pose_json",
            "locked",
            "reason"
        ]
    );
}

#[test]
fn an_older_table_gains_the_columns_and_keeps_its_rows() {
    let path = temp_db_path("stone_choice_migration");
    let path_text = path.to_str().unwrap();
    {
        let db = Database::new(Some(path_text)).expect("create");
        // The table as an older build made it: four columns.
        db.conn
            .execute_batch(
                "DROP TABLE stone_pose_choice;
                 CREATE TABLE stone_pose_choice (
                     plan_id INTEGER NOT NULL,
                     layout_index INTEGER NOT NULL,
                     stone_index INTEGER NOT NULL,
                     pose INTEGER NOT NULL,
                     PRIMARY KEY (plan_id, layout_index, stone_index)
                 );
                 INSERT INTO stone_pose_choice VALUES (4, 0, 2, 3);",
            )
            .unwrap();
        assert_eq!(columns(&db).len(), 4);
    }
    // Opening it twice adds the columns once and keeps the row as it was.
    for _ in 0..2 {
        let db = Database::new(Some(path_text)).expect("reopen");
        assert_eq!(columns(&db).len(), 7);
        assert_eq!(
            db.stone_pose_choices(4).unwrap(),
            [StonePoseChoiceRow {
                layout_index: 0,
                stone_index: 2,
                pose: 3
            }]
        );
        assert_eq!(db.stone_choices(4).unwrap(), [choice(0, 2, 3)]);
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_full_choice_round_trips_and_a_default_one_is_not_stored() {
    let db = in_memory_db();
    let full = StoneChoiceRow {
        free_pose_json: Some("{\"center_mm\":[1,2,3]}".to_string()),
        locked: true,
        reason: "cut for color".to_string(),
        ..choice(1, 4, 2)
    };
    db.set_stone_choice(9, &full).unwrap();
    db.set_stone_choice(9, &choice(0, 0, 1)).unwrap();
    assert_eq!(
        db.stone_choices(9).unwrap(),
        [choice(0, 0, 1), full.clone()]
    );
    // The pose-only reader sees the poses alone.
    assert_eq!(db.stone_pose_choices(9).unwrap().len(), 2);

    // Storing again replaces; a default row removes the stone's row.
    let relocked = StoneChoiceRow {
        locked: false,
        ..full
    };
    db.set_stone_choice(9, &relocked).unwrap();
    assert_eq!(db.stone_choices(9).unwrap()[1], relocked);
    db.set_stone_choice(9, &choice(1, 4, 0)).unwrap();
    assert_eq!(db.stone_choices(9).unwrap(), [choice(0, 0, 1)]);
    assert_eq!(db.stone_choices(10).unwrap(), Vec::new());
}

#[test]
fn resetting_the_pose_keeps_a_lock_and_a_reason_and_drops_an_empty_row() {
    let db = in_memory_db();
    let locked = StoneChoiceRow {
        locked: true,
        reason: "keep this one".to_string(),
        ..choice(0, 1, 3)
    };
    db.set_stone_choice(5, &locked).unwrap();
    db.set_stone_pose_choice(5, 0, 1, 0).unwrap();
    assert_eq!(
        db.stone_choices(5).unwrap(),
        [StoneChoiceRow { pose: 0, ..locked }]
    );
    // A row with a pose-0 and extra data is not a pose choice.
    assert_eq!(db.stone_pose_choices(5).unwrap(), Vec::new());

    // A pose-only row goes away, as it always did.
    db.set_stone_pose_choice(5, 0, 2, 2).unwrap();
    db.set_stone_pose_choice(5, 0, 2, 0).unwrap();
    assert_eq!(db.stone_choices(5).unwrap().len(), 1);

    // Setting a pose on a stone that has a Reason keeps the Reason.
    db.set_stone_pose_choice(5, 0, 1, 2).unwrap();
    let kept = &db.stone_choices(5).unwrap()[0];
    assert_eq!((kept.pose, kept.reason.as_str()), (2, "keep this one"));
}

#[test]
fn deleting_a_plan_clears_its_stone_choices() {
    let db = in_memory_db();
    let full = StoneChoiceRow {
        locked: true,
        ..choice(0, 0, 0)
    };
    db.set_stone_choice(3, &full).unwrap();
    db.set_stone_choice(4, &full).unwrap();
    db.delete_rough_colour(3).unwrap();
    assert_eq!(db.stone_choices(3).unwrap(), Vec::new());
    assert_eq!(db.stone_choices(4).unwrap(), [full]);
}
