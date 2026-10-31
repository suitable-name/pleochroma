//! Import of the self-contained `.psdd` design file: stand-alone, attached to the `.asc`
//! beside it, picked directly, and its extension handling in the folder scan.

use super::super::{
    pipeline::import_path,
    scan::{collect_import_candidates, find_design_file_beside},
};
use crate::gui::library::local::helpers::test_support::{
    VALID_ASC, open_temp_db, temp_db_path_for_test, temp_dir_for_test,
};
use pleochroma_cut_core::{
    ConstraintTier, Design, PreformSpec, ScheduleMeta,
    native::{DesignExtras, design_to_string},
};
use std::path::Path;

/// The text of a design file for the standard round brilliant.
fn design_text() -> String {
    let design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    design_to_string(&design, None, &DesignExtras::default()).expect("serializes")
}

/// The attachment names and url of every imported row, sorted by url.
fn rows_of(
    db: &std::sync::Arc<std::sync::Mutex<pleochroma_vault::db::sqlite::Database>>,
    ids: &[i64],
) -> Vec<(String, Vec<String>)> {
    let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut rows: Vec<(String, Vec<String>)> = ids
        .iter()
        .map(|id| {
            let full = conn
                .get_diagram_full(*id)
                .expect("query must succeed")
                .expect("row must exist");
            (
                full.url,
                full.attached_files.into_iter().map(|f| f.name).collect(),
            )
        })
        .collect();
    rows.sort();
    rows
}

fn file_names(paths: &[std::path::PathBuf]) -> Vec<String> {
    let mut names: Vec<String> = paths
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    names
}

/// A `.psdd` file with nothing beside it is a design of its own: the folder scan
/// finds it, and it becomes a row holding the file itself and the angle table read
/// from its tiers.
#[test]
fn a_standalone_design_file_is_imported_as_a_design_of_its_own() {
    let dir = temp_dir_for_test("design_solo");
    std::fs::write(dir.join("solo.psdd"), design_text()).expect("write solo.psdd");

    let db_path = temp_db_path_for_test("design_solo");
    let db = open_temp_db(&db_path);
    let outcome = import_path(&db, &dir, false, |_, _| {});
    assert!(
        outcome.summary.contains("Imported 1"),
        "summary was: {}",
        outcome.summary
    );
    assert!(!outcome.had_failures, "summary was: {}", outcome.summary);

    let rows = rows_of(&db, &outcome.imported_ids);
    assert_eq!(
        rows,
        vec![(
            "local://solo.psdd".to_string(),
            vec!["solo.psdd".to_string()]
        )]
    );
    let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let full = conn
        .get_diagram_full(outcome.imported_ids[0])
        .expect("query must succeed")
        .expect("row must exist");
    assert_eq!(
        full.angle_settings.len(),
        ConstraintTier::standard_round_brilliant().len(),
        "the angle table comes from the file's own tiers"
    );
    drop(conn);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&db_path);
}

/// A `.psdd` beside a same-stem `.asc` is attached to it: one row, two attachments, and
/// the design file is not also imported on its own.
#[test]
fn a_design_file_beside_an_asc_is_attached_to_it() {
    let dir = temp_dir_for_test("design_paired");
    std::fs::write(dir.join("paired.asc"), VALID_ASC).expect("write paired.asc");
    std::fs::write(dir.join("paired.psdd"), design_text()).expect("write paired.psdd");

    let candidates = collect_import_candidates(&dir, false).expect("candidates");
    assert_eq!(file_names(&candidates), ["paired.asc"]);

    let db_path = temp_db_path_for_test("design_paired");
    let db = open_temp_db(&db_path);
    let outcome = import_path(&db, &dir, false, |_, _| {});
    assert!(
        outcome.summary.contains("Imported 1"),
        "summary was: {}",
        outcome.summary
    );
    let rows = rows_of(&db, &outcome.imported_ids);
    assert_eq!(
        rows,
        vec![(
            "local://paired.asc".to_string(),
            vec!["paired.asc".to_string(), "paired.psdd".to_string()]
        )]
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&db_path);
}

/// The `.psdd` file beside an `.asc` is found only when it is a design file: one that is
/// not is ignored.
#[test]
fn the_design_file_beside_an_asc_is_found_only_if_it_is_a_design() {
    let dir = temp_dir_for_test("design_preferred");
    let asc = dir.join("both.asc");
    std::fs::write(&asc, VALID_ASC).expect("write both.asc");
    std::fs::write(dir.join("both.psdd"), design_text()).expect("write design");
    assert_eq!(find_design_file_beside(&asc).expect("found").0, "both.psdd");

    std::fs::write(dir.join("both.psdd"), "not a design file").expect("overwrite");
    assert!(
        find_design_file_beside(&asc).is_none(),
        "a .psdd file that is not a design file is ignored"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Extension handling in the scan: `.psdd` is recognised (any case) next to the other
/// design kinds, and picking a file directly accepts a stand-alone design file and
/// redirects to the `.asc` beside an attached one.
#[test]
fn the_scan_recognises_design_files() {
    let dir = temp_dir_for_test("design_scan");
    std::fs::write(dir.join("alone.PSDD"), design_text()).expect("write alone");
    std::fs::write(dir.join("with_asc.asc"), VALID_ASC).expect("write with_asc.asc");
    std::fs::write(dir.join("with_asc.psdd"), design_text()).expect("write with_asc design");
    std::fs::write(dir.join("notes.txt"), "x").expect("write notes");

    let folder = collect_import_candidates(&dir, false).expect("folder candidates");
    assert_eq!(file_names(&folder), ["alone.PSDD", "with_asc.asc"]);

    let alone = collect_import_candidates(&dir.join("alone.PSDD"), false).expect("alone");
    assert_eq!(file_names(&alone), ["alone.PSDD"]);

    let attached = collect_import_candidates(&dir.join("with_asc.psdd"), false).expect("attached");
    assert_eq!(file_names(&attached), ["with_asc.asc"]);

    let empty = temp_dir_for_test("design_scan_empty");
    std::fs::write(Path::new(&empty).join("readme.txt"), "x").expect("write readme");
    let none = collect_import_candidates(&empty, false);
    assert!(none.is_err_and(|message| message.contains(".psdd")));

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&empty);
}
