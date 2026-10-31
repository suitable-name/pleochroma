//! The design's name as the status strip and the editor's title row show it: the design
//! FILE the design was opened from or saved to (`tutorial-design.psdd`), not the
//! cutting-schedule name (`tutorial-design.asc`) the editor records for it internally.

use super::state::{EditorState, design_label_text};
use crate::gui::editor::native_io;
use std::path::Path;

/// `schedule_name`'s file stem (`foo.asc` -> `foo`); a name with no extension is its own stem.
fn stem_of(name: &str) -> &str {
    Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(name)
}

/// The label for a design recorded as `schedule_name` (`foo.asc`, `None` for a new,
/// never-saved design) whose design file is `file`, if it has one.
///
/// The file's own name, with its real extension, when `file` names this design: its stem
/// equals the schedule name's stem (compared case-insensitively, the rule a quick Save
/// uses). A library selection or a New design does not clear the remembered file, so a
/// file of another stem is never shown. Everything else keeps the schedule name, or
/// "Untitled design" when there is none -- the `.asc` name stays the name of the
/// cutting-schedule export only.
#[must_use]
pub(super) fn design_label_for(schedule_name: Option<&str>, file: Option<&Path>) -> String {
    let file_name = schedule_name.zip(file).and_then(|(schedule, file)| {
        let stem = file.file_stem()?.to_str()?;
        let name = file.file_name()?.to_str()?;
        stem.eq_ignore_ascii_case(stem_of(schedule)).then_some(name)
    });
    file_name.map_or_else(|| design_label_text(schedule_name), str::to_string)
}

/// [`design_label_for`] for the live editor: the design file `native_io` remembers.
#[must_use]
pub(in crate::gui::editor) fn design_label(state: &EditorState) -> String {
    design_label_for(
        state.asc_filename.as_deref(),
        native_io::current_design_file().as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::design_label_for;
    use std::path::Path;

    #[test]
    fn an_opened_design_file_shows_its_own_name_with_its_extension() {
        assert_eq!(
            design_label_for(
                Some("tutorial-design.asc"),
                Some(Path::new("C:/designs/tutorial-design.psdd"))
            ),
            "tutorial-design.psdd"
        );
    }

    #[test]
    fn a_remembered_file_of_another_design_is_never_shown() {
        assert_eq!(
            design_label_for(Some("catalogue.asc"), Some(Path::new("/x/other.psdd"))),
            "catalogue.asc"
        );
    }

    #[test]
    fn a_design_without_a_file_keeps_its_schedule_name_or_untitled() {
        assert_eq!(
            design_label_for(Some("stone.asc"), None),
            "stone.asc",
            "a catalogue or converted design"
        );
        assert_eq!(design_label_for(None, None), "Untitled design");
        assert_eq!(
            design_label_for(None, Some(Path::new("/x/other.psdd"))),
            "Untitled design",
            "a new design is not named after the file the last one came from"
        );
    }
}
