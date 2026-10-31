//! The bytes-in importer: one picked file's bytes become a parsed, measured
//! [`ImportedAsc`], with no database, no paths and no threads.
//!
//! Desktop, Android and the browser all run this one function. What differs between them
//! stays with the caller: where the bytes come from, the panic guard around the call, the
//! measuring geometry (the two hooks of [`import_bytes`], usually built on
//! [`apply_measured_metadata`](super::apply_measured_metadata)), how a failure reads to the
//! user, and how the row is stored (see [`reconcile_with_existing`](super::reconcile_with_existing)).
//!
//! The function does no panic isolation of its own: measuring reaches into geometry code
//! this crate does not own, so callers wrap the call in the same guard they use elsewhere.

use super::{
    DesignFileKind, ImportedAsc, design_file_to_asc_text, import_asc, import_asc_bytes,
    import_native_design, is_native_design_name,
};
use crate::model::{detail::FacetingDiagramDetail, file::AttachedFile};

/// What kind of design file a picked file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    /// `.asc` cutting instructions (and any other file the caller does not classify).
    Asc,
    /// A self-contained `.psdd` design file.
    Design,
    /// `GemCAD`'s binary `.gem` save file.
    Gem,
    /// Gem Cut Studio's XML `.gcs` file.
    Gcs,
}

impl ImportKind {
    /// The kind `file_name` names by its extension: a design file, `.gem` or `.gcs`;
    /// anything else reads as `.asc` text.
    #[must_use]
    pub fn from_file_name(file_name: &str) -> Self {
        if is_native_design_name(file_name) {
            return Self::Design;
        }
        match DesignFileKind::from_file_name(file_name) {
            Some(DesignFileKind::Gem) => Self::Gem,
            Some(DesignFileKind::Gcs) => Self::Gcs,
            Some(DesignFileKind::Asc) | None => Self::Asc,
        }
    }

    /// The conversion this kind needs to become `.asc`; `None` for a kind read directly.
    const fn foreign(self) -> Option<DesignFileKind> {
        match self {
            Self::Gem => Some(DesignFileKind::Gem),
            Self::Gcs => Some(DesignFileKind::Gcs),
            Self::Asc | Self::Design => None,
        }
    }
}

/// Which step of [`import_bytes`] a failure came from, so a caller can word it its own way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportStage {
    /// A design file did not read.
    Design,
    /// An `.asc` file did not parse.
    Asc,
    /// A `.gem` or `.gcs` did not convert to `.asc` cutting instructions.
    Convert,
    /// The `.asc` text a converted `.gem` or `.gcs` produced did not parse.
    ConvertedSchedule,
}

/// A file [`import_bytes`] could not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFailure {
    /// The step that failed.
    pub stage: ImportStage,
    /// The reader's own message, without the file name.
    pub message: String,
}

impl std::fmt::Display for ImportFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// One file read: the parsed design and what the converter noted about it.
pub struct ImportedBytes {
    /// The entry, detail and extras, ready to store.
    pub parsed: ImportedAsc,
    /// What the `.gem`/`.gcs` reader and converter noted (a preform section that is not
    /// converted, hidden or guide tiers, a missing refractive index, ...). Empty for every
    /// other kind and for a clean file.
    pub warnings: Vec<String>,
}

/// Reads one file's `bytes` as a `kind` design into an [`ImportedAsc`].
///
/// - `Asc`: the vault's [`import_asc_bytes`] (the original bytes are the attachment).
/// - `Design`: [`import_native_design`]; `design_file` is not used.
/// - `Gem` / `Gcs`: converted to `.asc` cutting instructions
///   ([`design_file_to_asc_text`]) and parsed like any `.asc`. The generated text is the
///   entry's design file, renamed to the converted `.asc` name; the ORIGINAL file is attached
///   beside it under `file_name` (a `.gem` also fills `gem_file`), then `design_file`. A
///   `.gcs`'s frosting travels in the extras. The title falls back to the file stem, not the
///   full `file_name`, when the design carries no title header.
///
/// `design_file` is the `<stem>.psdd` design file a caller with
/// a file system found beside the file, as `(name, bytes)`; a caller with only the picked bytes
/// passes `None`.
///
/// `after_parse` runs on the detail right after the file is parsed, before any attachment is
/// added or renamed; `after_attach` runs last, when the attachments are as they will be
/// stored. A caller measures in the one that matches where its geometry reads from (the angle
/// table: either; the stored design file: `after_attach`).
///
/// # Errors
///
/// An [`ImportFailure`] naming the step that failed and the reader's own message.
pub fn import_bytes(
    file_name: &str,
    kind: ImportKind,
    bytes: &[u8],
    design_file: Option<(&str, &[u8])>,
    after_parse: impl FnOnce(&mut FacetingDiagramDetail),
    after_attach: impl FnOnce(&mut FacetingDiagramDetail),
) -> Result<ImportedBytes, ImportFailure> {
    let fail = |stage: ImportStage| move |message: String| ImportFailure { stage, message };
    let Some(foreign) = kind.foreign() else {
        let mut parsed = if kind == ImportKind::Design {
            import_native_design(file_name, bytes).map_err(fail(ImportStage::Design))?
        } else {
            import_asc_bytes(file_name, bytes, design_file).map_err(fail(ImportStage::Asc))?
        };
        after_parse(&mut parsed.detail);
        after_attach(&mut parsed.detail);
        return Ok(ImportedBytes {
            parsed,
            warnings: Vec::new(),
        });
    };

    let converted =
        design_file_to_asc_text(file_name, foreign, bytes).map_err(fail(ImportStage::Convert))?;
    let mut parsed = import_asc(file_name, &converted.asc_text, None)
        .map_err(fail(ImportStage::ConvertedSchedule))?;
    after_parse(&mut parsed.detail);
    // `.asc` has no notation for frosting, so what a `.gcs` states is stored with the row.
    let mut parsed = parsed.with_frosting(&converted.frosting);
    let asc_name = converted.asc_file_name;
    // `import_asc` names its one attachment after `file_name` but holds the generated `.asc`
    // text: rename it, and keep the original file beside it.
    if let Some(generated) = parsed.detail.attached_files.first_mut() {
        generated.name.clone_from(&asc_name);
    }
    parsed.detail.attached_files.push(AttachedFile {
        name: file_name.to_owned(),
        url: String::new(),
        content: bytes.to_vec(),
    });
    if let Some((name, content)) = design_file {
        parsed.detail.attached_files.push(AttachedFile {
            name: name.to_owned(),
            url: String::new(),
            content: content.to_vec(),
        });
    }
    if kind == ImportKind::Gem {
        parsed.detail.gem_file = Some(file_name.to_owned());
    }
    if parsed.entry.title == file_name {
        asc_name
            .trim_end_matches(".asc")
            .clone_into(&mut parsed.entry.title);
    }
    after_attach(&mut parsed.detail);
    Ok(ImportedBytes {
        parsed,
        warnings: converted.warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const SAMPLE_ASC: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 6 y\n\
I 1.72\n\
H Round Trichecker-12\n\
a -41.000000 0.64991234 92 n 1 84 76 68 60\n\
a 41.000000 0.5 92 n T\n";

    #[test]
    fn the_kind_follows_the_extension_case_insensitively_and_defaults_to_asc() {
        assert_eq!(ImportKind::from_file_name("a.asc"), ImportKind::Asc);
        assert_eq!(ImportKind::from_file_name("a.ASC"), ImportKind::Asc);
        assert_eq!(ImportKind::from_file_name("a.psdd"), ImportKind::Design);
        assert_eq!(ImportKind::from_file_name("a.GEM"), ImportKind::Gem);
        assert_eq!(ImportKind::from_file_name("a.gcs"), ImportKind::Gcs);
        assert_eq!(ImportKind::from_file_name("a.txt"), ImportKind::Asc);
        assert_eq!(ImportKind::from_file_name("noextension"), ImportKind::Asc);
    }

    #[test]
    fn an_asc_imports_with_its_bytes_a_beside_design_file_and_both_hooks_in_order() {
        let order = RefCell::new(Vec::new());
        let imported = import_bytes(
            "trichecker.asc",
            ImportKind::Asc,
            SAMPLE_ASC.as_bytes(),
            Some(("trichecker.psdd", b"design".as_slice())),
            |detail| {
                order
                    .borrow_mut()
                    .push(format!("parse:{}", detail.attached_files.len()));
            },
            |detail| {
                order
                    .borrow_mut()
                    .push(format!("attach:{}", detail.attached_files.len()));
            },
        )
        .unwrap_or_else(|failure| panic!("{failure}"));
        assert_eq!(imported.parsed.entry.title, "Round Trichecker-12");
        assert_eq!(imported.parsed.entry.url, "local://trichecker.asc");
        assert!(imported.warnings.is_empty());
        let names: Vec<&str> = imported
            .parsed
            .detail
            .attached_files
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(names, ["trichecker.asc", "trichecker.psdd"]);
        assert_eq!(*order.borrow(), ["parse:2", "attach:2"]);
    }

    #[test]
    fn a_bad_asc_fails_at_the_asc_stage_and_a_bad_design_at_the_design_stage() {
        let asc = import_bytes("bad.asc", ImportKind::Asc, b"nope", None, |_| {}, |_| {})
            .err()
            .expect("not an .asc");
        assert_eq!(asc.stage, ImportStage::Asc);
        assert!(!asc.message.is_empty());
        assert_eq!(asc.to_string(), asc.message);

        let design = import_bytes(
            "bad.psdd",
            ImportKind::Design,
            b"\xFF\xFE",
            None,
            |_| {},
            |_| {},
        )
        .err()
        .expect("not UTF-8");
        assert_eq!(design.stage, ImportStage::Design);
        assert!(design.message.contains("UTF-8"), "{}", design.message);
    }

    #[test]
    fn a_design_file_imports_without_converting_and_ignores_a_beside_file() {
        let text = super::super::oriented_design_text(true);
        let imported = import_bytes(
            "axis.psdd",
            ImportKind::Design,
            text.as_bytes(),
            Some(("other.psdd", b"ignored".as_slice())),
            |_| {},
            |_| {},
        )
        .unwrap_or_else(|failure| panic!("{failure}"));
        assert_eq!(imported.parsed.entry.url, "local://axis.psdd");
        assert!(imported.parsed.extras.preview_axis.is_some());
        assert!(
            imported
                .parsed
                .detail
                .attached_files
                .iter()
                .all(|file| file.name != "other.psdd")
        );
    }

    #[test]
    fn an_unreadable_gem_or_gcs_fails_at_the_convert_stage_before_either_hook_runs() {
        for (name, kind) in [("x.gem", ImportKind::Gem), ("x.gcs", ImportKind::Gcs)] {
            let ran = RefCell::new(false);
            let failure = import_bytes(
                name,
                kind,
                &[1, 2, 3],
                None,
                |_| *ran.borrow_mut() = true,
                |_| *ran.borrow_mut() = true,
            )
            .err()
            .expect("three bytes are no design");
            assert_eq!(failure.stage, ImportStage::Convert, "{name}");
            assert!(
                failure.message.contains("not a readable"),
                "{}",
                failure.message
            );
            assert!(!*ran.borrow());
        }
    }
}
