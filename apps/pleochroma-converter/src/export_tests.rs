use super::*;
use crate::database::Source;
use pleochroma_cut_core::native::{DesignMetadata, design_from_str};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::{fs, path::PathBuf};

const BOX_ASC: &[u8] = b"GemCad 5.0\ng 4 0\ny 1 n\nI 1.62\na 90 1 0 1 2 3\na 0 0.6\na -0 0.55\n";

struct Fixture {
    directory: tempfile::TempDir,
    connection: Connection,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let connection = Connection::open(directory.path().join("library.sqlite")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE diagram_entries (id INTEGER PRIMARY KEY, title TEXT, ignored INTEGER);
                 CREATE TABLE diagram_details (id INTEGER PRIMARY KEY, entry_id INTEGER, designer TEXT);
                 CREATE TABLE attached_files (id INTEGER PRIMARY KEY, detail_id INTEGER, name TEXT, url TEXT, content BLOB);",
            )
            .unwrap();
        Self {
            directory,
            connection,
        }
    }

    fn entry(&self, id: i64, title: &str) {
        self.connection
            .execute(
                "INSERT INTO diagram_entries VALUES (?1, ?2, 0)",
                params![id, title],
            )
            .unwrap();
        self.detail(id, id, "Recorded designer");
    }

    fn detail(&self, id: i64, entry: i64, designer: &str) {
        self.connection
            .execute(
                "INSERT INTO diagram_details VALUES (?1, ?2, ?3)",
                params![id, entry, designer],
            )
            .unwrap();
    }

    fn source(&self, id: i64, detail: i64, name: &str, bytes: &[u8]) {
        self.connection
            .execute(
                "INSERT INTO attached_files VALUES (?1, ?2, ?3, '', ?4)",
                params![id, detail, name, bytes],
            )
            .unwrap();
    }

    fn output(&self) -> PathBuf {
        self.directory.path().join("export")
    }

    fn run(&self) -> (bool, Value) {
        let args = Args {
            db: self.directory.path().join("library.sqlite"),
            output: self.output(),
            adopt_meets: false,
        };
        let original = fs::read(&args.db).unwrap();
        let partial = export(&args).unwrap();
        assert_eq!(fs::read(&args.db).unwrap(), original);
        let report =
            serde_json::from_slice(&fs::read(args.output.join("report.json")).unwrap()).unwrap();
        (partial, report)
    }

    fn designs(&self) -> Vec<PathBuf> {
        let mut designs = Vec::new();
        for directory in [self.output(), self.output().join("drafts")] {
            if !directory.exists() {
                continue;
            }
            for entry in fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "psdd")
                {
                    designs.push(path);
                }
            }
        }
        designs.sort();
        designs
    }
}

fn selected_record(report: &Value, entry: i64) -> &Value {
    let selected: Vec<_> = report["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["entry_id"] == entry && record["output"].is_string())
        .collect();
    assert_eq!(selected.len(), 1, "one selected source per entry");
    selected[0]
}

fn native_source() -> String {
    let entry = Entry {
        id: 1,
        title: "Native title".into(),
        metadata: DesignMetadata {
            title: "Native title".into(),
            ..DesignMetadata::default()
        },
    };
    let source = Source {
        id: 900,
        name: "original.asc".into(),
        url: String::new(),
        bytes: BOX_ASC.to_vec(),
    };
    let converted = convert::convert(&entry, &source, false).unwrap();
    format!(
        "# Preserve native comments and authoring\n{}",
        converted.text
    )
}

#[test]
fn duplicate_sources_export_one_design_and_choose_the_lowest_attachment_id() {
    let fixture = Fixture::new();
    fixture.entry(25, "Shared cut");
    // Deliberately insert in the opposite order from the selection tie-break.
    fixture.source(49, 25, "copy.asc", BOX_ASC);
    fixture.source(48, 25, "original.ASC", BOX_ASC);
    let (partial, report) = fixture.run();
    assert!(!partial);
    assert_eq!(report["schema"], 2);
    assert_eq!(report["summary"]["entries"], 1);
    assert_eq!(report["summary"]["sources"], 2);
    assert_eq!(report["summary"]["converted"], 1);
    assert_eq!(report["summary"]["skipped"], 1);
    assert_eq!(report["records"].as_array().unwrap().len(), 2);
    let selected = selected_record(&report, 25);
    assert_eq!(selected["attachment_id"], 48);
    assert_eq!(selected["output"], "entry-25-shared-cut.psdd");
    assert_eq!(
        fixture.designs(),
        [fixture.output().join("entry-25-shared-cut.psdd")]
    );
    let text = fs::read_to_string(&fixture.designs()[0]).unwrap();
    let loaded = design_from_str(&text).unwrap();
    assert_eq!(loaded.attachments[0].data, BOX_ASC);
    assert!(!loaded.draft);
    let skipped = report["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["attachment_id"] == 49)
        .unwrap();
    assert_eq!(skipped["status"], "skipped");
    assert!(skipped["output"].is_null());
}

#[test]
fn invalid_first_source_falls_back_without_losing_the_failure_diagnostic() {
    let fixture = Fixture::new();
    fixture.entry(1, "Cut");
    fixture.source(10, 1, "broken.asc", b"invalid");
    fixture.source(11, 1, "working.asc", BOX_ASC);
    let (partial, report) = fixture.run();
    assert!(
        partial,
        "failed source attempts remain visible in the exit status"
    );
    assert_eq!(report["summary"]["failed"], 1);
    assert_eq!(report["summary"]["converted"], 1);
    assert_eq!(selected_record(&report, 1)["attachment_id"], 11);
    assert_eq!(fixture.designs().len(), 1);
    let failed = report["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["attachment_id"] == 10)
        .unwrap();
    assert_eq!(failed["status"], "failed");
    assert!(!failed["notes"].as_array().unwrap().is_empty());
    assert!(failed["output"].is_null());
}

#[test]
fn all_invalid_sources_are_reported_without_writing_a_design() {
    let fixture = Fixture::new();
    fixture.entry(1, "Broken");
    fixture.source(1, 1, "first.asc", b"invalid");
    fixture.source(2, 1, "second.asc", b"");
    let (partial, report) = fixture.run();
    assert!(partial);
    assert_eq!(report["summary"]["failed"], 2);
    assert_eq!(report["summary"]["converted"], 0);
    assert!(fixture.designs().is_empty());
    assert!(
        report["records"]
            .as_array()
            .unwrap()
            .iter()
            .all(|record| record["status"] == "failed" && record["output"].is_null())
    );
}

#[test]
fn distinct_entries_with_the_same_title_keep_separate_output_files() {
    let fixture = Fixture::new();
    for id in [1, 2] {
        fixture.entry(id, "Same title");
        fixture.source(id, id, "same.asc", BOX_ASC);
    }
    let (partial, report) = fixture.run();
    assert!(!partial);
    assert_eq!(report["summary"]["converted"], 2);
    assert_eq!(
        selected_record(&report, 1)["output"],
        "entry-1-same-title.psdd"
    );
    assert_eq!(
        selected_record(&report, 2)["output"],
        "entry-2-same-title.psdd"
    );
    assert_eq!(fixture.designs().len(), 2);
}

#[test]
fn complete_asc_is_preferred_to_a_higher_priority_native_draft() {
    let fixture = Fixture::new();
    fixture.entry(1, "Cut");
    let native = pleochroma_formats::native::design::parse(&native_source()).unwrap();
    let draft = pleochroma_formats::native::design::to_string(&native.with_draft(true)).unwrap();
    fixture.source(1, 1, "unfinished.psdd", draft.as_bytes());
    fixture.source(2, 1, "complete.asc", BOX_ASC);
    let (partial, report) = fixture.run();
    assert!(!partial);
    assert_eq!(report["summary"]["converted"], 1);
    assert_eq!(report["summary"]["drafts"], 0);
    assert_eq!(report["summary"]["skipped"], 1);
    assert_eq!(selected_record(&report, 1)["attachment_id"], 2);
    assert_eq!(
        fixture.designs(),
        [fixture.output().join("entry-1-cut.psdd")]
    );
    assert!(!fixture.output().join("drafts").exists());
}

#[test]
fn complete_native_is_preferred_to_asc_and_keeps_its_authored_document() {
    let fixture = Fixture::new();
    fixture.entry(1, "Catalogue title");
    let native = native_source();
    fixture.source(1, 1, "legacy.asc", BOX_ASC);
    fixture.source(2, 1, "authored.PsDd", native.as_bytes());
    let (partial, report) = fixture.run();
    assert!(!partial);
    let selected = selected_record(&report, 1);
    assert_eq!(selected["attachment_id"], 2);
    let output = fixture.output().join(selected["output"].as_str().unwrap());
    assert_eq!(fs::read_to_string(output).unwrap(), native);
    assert_eq!(fixture.designs().len(), 1);
}

#[test]
fn multiple_detail_rows_do_not_duplicate_entries_or_hide_later_sources() {
    let fixture = Fixture::new();
    fixture.entry(1, "Cut");
    fixture.detail(2, 1, "Later metadata");
    fixture.source(10, 1, "broken.asc", b"invalid");
    fixture.source(11, 2, "working.asc", BOX_ASC);
    let (partial, report) = fixture.run();
    assert!(partial);
    assert_eq!(report["summary"]["entries"], 1);
    assert_eq!(report["summary"]["sources"], 2);
    assert_eq!(report["summary"]["converted"], 1);
    let selected = selected_record(&report, 1);
    assert_eq!(selected["attachment_id"], 11);
    let text =
        fs::read_to_string(fixture.output().join(selected["output"].as_str().unwrap())).unwrap();
    assert_eq!(
        design_from_str(&text).unwrap().metadata.designer,
        "Recorded designer"
    );
    assert_eq!(fixture.designs().len(), 1);
}

#[test]
fn missing_sources_are_reported_without_hiding_successful_entries() {
    let fixture = Fixture::new();
    fixture.entry(1, "Available");
    fixture.entry(2, "Unavailable");
    fixture.source(10, 1, "source.asc", BOX_ASC);
    let (partial, report) = fixture.run();
    assert!(partial);
    assert_eq!(report["summary"]["entries"], 2);
    assert_eq!(report["summary"]["missing_sources"], 1);
    assert_eq!(report["summary"]["converted"], 1);
    assert_eq!(fixture.designs().len(), 1);
    let missing = report["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["entry_id"] == 2)
        .unwrap();
    assert_eq!(missing["status"], "missing_source");
    assert!(missing["output"].is_null());
}

#[test]
fn all_draft_sources_produce_only_the_preferred_draft() {
    let fixture = Fixture::new();
    fixture.entry(1, "Incomplete");
    let incomplete = b"GemCad 5.0\ng 4 0\ny 1 n\nI 1.62\na 0 0.6\n";
    fixture.source(2, 1, "second.asc", incomplete);
    fixture.source(1, 1, "first.asc", incomplete);
    let (partial, report) = fixture.run();
    assert!(partial);
    assert_eq!(report["summary"]["drafts"], 1);
    assert_eq!(report["summary"]["skipped"], 1);
    let selected = selected_record(&report, 1);
    assert_eq!(selected["attachment_id"], 1);
    assert_eq!(selected["status"], "draft");
    assert_eq!(selected["output"], "drafts/entry-1-incomplete.psdd");
    assert_eq!(fixture.designs().len(), 1);
}
