//! What the Open picker offers, and the file names a design is saved under.
//!
//! Which loader a picked or dropped file goes to is decided by
//! [`pleochroma_editor::files::InputFileKind::classify`] (by content where the name
//! cannot tell: a `.psdd` file is a design file, and a `.toml` is only opened when its
//! header says it is one).

use pleochroma_formats::native::design::{DESIGN_EXTENSION, DESIGN_EXTENSION_DOTTED};

/// The extensions the Open picker offers, the design file first (`.psdd`), then the
/// other formats it reads.
pub const OPEN_EXTENSIONS: [&str; 5] = [DESIGN_EXTENSION, "asc", "gem", "gcs", "hdr"];

/// The `.asc` name a design file called `file_name` is recorded under, so a later
/// save proposes the same stem (`round.psdd` -> `round.asc`).
#[must_use]
pub fn asc_name_for_design_file(file_name: &str) -> String {
    pleochroma_editor::files::converted_asc_file_name(file_name)
}

/// The design file name for a design recorded under `asc_name`: the extension replaced
/// by `.psdd` (`round.asc` -> `round.psdd`), or appended to a name with no
/// extension.
#[must_use]
pub fn design_file_name_for_asc(asc_name: &str) -> String {
    let stem = asc_name
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map_or(asc_name, |(stem, _)| stem);
    format!("{stem}{DESIGN_EXTENSION_DOTTED}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_formats::native::design::DESIGN_VERSION_PLANES;

    #[test]
    fn a_file_from_a_newer_version_is_refused_with_a_clear_message() {
        // One past the newest version this build reads (the one with plane-set preforms).
        let text = format!(
            "format = \"pleochroma-design\"\nversion = {}\n",
            DESIGN_VERSION_PLANES + 1
        );
        assert_eq!(
            pleochroma_editor::files::InputFileKind::classify("future.psdd", text.as_bytes()),
            Some(pleochroma_editor::files::InputFileKind::Design)
        );
        let error = pleochroma_cut_core::native::design_from_str(&text)
            .expect_err("a newer version is refused");
        assert!(error.to_string().contains("newer Pleochroma"), "{error}");
    }

    #[test]
    fn the_picker_lists_the_design_extension_first() {
        assert_eq!(OPEN_EXTENSIONS[0], "psdd");
        assert!(!OPEN_EXTENSIONS.contains(&"toml"));
        assert_eq!(
            pleochroma_editor::files::InputFileKind::from_file_name("a.psdd"),
            Some(pleochroma_editor::files::InputFileKind::Design)
        );
    }

    #[test]
    fn file_names_map_between_the_design_file_and_its_asc_name() {
        assert_eq!(asc_name_for_design_file("round.psdd"), "round.asc");
        assert_eq!(design_file_name_for_asc("round.asc"), "round.psdd");
        assert_eq!(design_file_name_for_asc("round"), "round.psdd");
        assert_eq!(design_file_name_for_asc("a.b.asc"), "a.b.psdd");
    }
}
