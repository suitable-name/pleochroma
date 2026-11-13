//! The beam keys of the settings file, which are read and written in every build.
//!
//! So a settings file saved by a build with the `beam` feature loses nothing when a standard
//! build loads and saves it.

use super::SettingsFile;
use crate::settings::store::{SettingsLoadOutcome, load_with_outcome, save};
use std::path::{Path, PathBuf};

/// A settings document with every beam key switched on: the viewport switches in `[settings]`,
/// the still export's two check boxes and the tilt video's one.
const BEAM_DOCUMENT: &str = "[settings]\n\
    beam_viewport = true\n\
    beam_instant_lighting = true\n\
    \n\
    [settings.output.still]\n\
    beam = true\n\
    keep_beams = true\n\
    \n\
    [settings.output.tilt_video]\n\
    beam = true\n";

/// A scratch folder under the OS temp dir, removed when the guard drops.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-cut-settings-beam-keys-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Whether `table` (a parsed settings document) holds `true` under `path` (table names, then the
/// key).
fn holds_true(table: &toml::Table, path: &[&str]) -> bool {
    let Some((key, tables)) = path.split_last() else {
        return false;
    };
    let mut current = table;
    for name in tables {
        let Some(next) = current.get(*name).and_then(toml::Value::as_table) else {
            return false;
        };
        current = next;
    }
    current.get(*key).and_then(toml::Value::as_bool) == Some(true)
}

/// A settings file from a beam build keeps its beam keys.
///
/// It loads with the values intact, saving writes every one of them back under the same names,
/// and loading the saved file gives the same settings.
#[test]
fn a_settings_file_from_a_beam_build_keeps_its_beam_keys_through_load_and_save() {
    let dir = ScratchDir::new();
    let source = dir.path().join("settings.toml");
    std::fs::write(&source, BEAM_DOCUMENT).unwrap();

    let (file, outcome) = load_with_outcome(&source);
    assert_eq!(outcome, SettingsLoadOutcome::Ok, "the file loads");
    assert!(file.settings.beam_viewport);
    assert!(file.settings.beam_instant_lighting);
    assert!(file.settings.output.still.beam);
    assert!(file.settings.output.still.keep_beams);
    assert!(file.settings.output.tilt_video.beam);

    let saved = dir.path().join("saved.toml");
    save(&saved, &file).unwrap();
    let text = std::fs::read_to_string(&saved).unwrap();
    let table: toml::Table = toml::from_str(&text).unwrap();
    for path in [
        &["settings", "beam_viewport"][..],
        &["settings", "beam_instant_lighting"],
        &["settings", "output", "still", "beam"],
        &["settings", "output", "still", "keep_beams"],
        &["settings", "output", "tilt_video", "beam"],
    ] {
        assert!(
            holds_true(&table, path),
            "{path:?} is written back:\n{text}"
        );
    }

    let (again, outcome) = load_with_outcome(&saved);
    assert_eq!(outcome, SettingsLoadOutcome::Ok);
    assert_eq!(again, file, "nothing is lost by a second round trip");
}

/// A settings file without beam keys has every beam switch off.
///
/// So a file from a standard build does not turn beam tracing on in a build that has it.
#[test]
fn a_settings_file_without_beam_keys_loads_with_every_beam_switch_off() {
    let parsed: SettingsFile = toml::from_str("[settings]\nexposure = 1.2\n").unwrap();
    assert!(!parsed.settings.beam_viewport);
    assert!(!parsed.settings.beam_instant_lighting);
    assert!(!parsed.settings.output.still.beam);
    assert!(!parsed.settings.output.still.keep_beams);
    assert!(!parsed.settings.output.tilt_video.beam);
}
