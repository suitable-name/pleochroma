//! A custom snapshot's color pattern in the text of a design file.

use super::*;

/// A color pattern is written as the plain string key `pattern`, round trips, is omitted when
/// unset, and an older build (a snapshot struct without the field) keeps the key in its
/// unknown table, so it writes the pattern back unchanged.
/// An older build's view of the custom-material table: no `pattern` field.
#[derive(serde::Deserialize)]
struct OlderSnapshot {
    #[expect(dead_code, reason = "read only to prove the known keys still parse")]
    mean_ri: f64,
    #[serde(flatten, default)]
    unknown: toml::Table,
}

#[test]
fn custom_snapshot_pattern_round_trips_and_an_older_build_keeps_it() {
    let mut native = sample_file();
    let text_of_pattern = "{\"version\":1,\"zones\":[]}".to_string();
    let snapshot = trigonal_snapshot().with_pattern(Some(text_of_pattern.clone()));
    native.material = native.material.with_custom(Some(snapshot.clone()));
    let text = write(&native);
    assert!(text.contains("pattern = "), "{text}");
    let custom = read(&text)
        .material
        .custom
        .expect("custom snapshot loads back");
    assert_eq!(custom, snapshot);
    assert_eq!(custom.pattern.as_deref(), Some(text_of_pattern.as_str()));

    // An older build: the same table read by a struct that has no `pattern` field.
    let document: toml::Table = text.parse().expect("the file is TOML");
    let custom_table = document["material"]["custom"].clone();
    let older: OlderSnapshot = custom_table.try_into().expect("an older build reads it");
    assert_eq!(
        older.unknown.get("pattern").and_then(toml::Value::as_str),
        Some(text_of_pattern.as_str())
    );

    // Without a pattern no key is written.
    native.material = native.material.with_custom(Some(trigonal_snapshot()));
    let text = write(&native);
    assert!(!text.contains("pattern = "), "{text}");
    let custom = read(&text)
        .material
        .custom
        .expect("custom snapshot loads back");
    assert_eq!(custom.pattern, None);
}
