//! The Import notice: what a run of Import Designs did, in one sentence per outcome.

use super::counted;
use std::fmt::Write as _;

/// A design whose stored name is longer than this is shown cut off in a notice.
const NOTICE_NAME_CHARS: usize = 40;

/// How many failed files a notice names before it says "and N more".
const NOTICE_NAMED_FAILURES: usize = 3;

/// How serious a notice is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Everything went as asked.
    Success,
    /// Something was skipped or approximate.
    Warning,
    /// Nothing could be done.
    Error,
}

/// What an Import run did, for the notice that ends it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Designs stored as new entries.
    pub added: usize,
    /// Designs that replaced an earlier copy of the same file (its tags and marks kept).
    pub updated: usize,
    /// Files the library already held byte for byte.
    pub identical: usize,
    /// Files that could not be imported, with the reason.
    pub failed: Vec<(String, String)>,
    /// Stored designs whose proportions could not be measured here (they come from the file).
    pub unmeasured: usize,
    /// The browser ran out of storage and the rest of the files were not tried.
    pub out_of_space: bool,
    /// A sentence per file the converter had a note about.
    pub notes: Vec<String>,
}

/// `name` cut to a notice's width.
fn notice_name(name: &str) -> String {
    if name.chars().count() <= NOTICE_NAME_CHARS {
        return name.to_owned();
    }
    let head: String = name.chars().take(NOTICE_NAME_CHARS).collect();
    format!("{head}...")
}

impl ImportReport {
    /// Designs written to the library.
    #[must_use]
    pub const fn stored(&self) -> usize {
        self.added + self.updated
    }

    /// Whether anything at all happened.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.stored() == 0 && self.identical == 0 && self.failed.is_empty() && !self.out_of_space
    }

    /// The notice's severity.
    #[must_use]
    pub const fn severity(&self) -> Severity {
        if self.stored() == 0
            && self.identical == 0
            && (!self.failed.is_empty() || self.out_of_space)
        {
            Severity::Error
        } else if !self.failed.is_empty() || self.unmeasured > 0 || self.out_of_space {
            Severity::Warning
        } else {
            Severity::Success
        }
    }

    /// The notice text.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut sentences: Vec<String> = Vec::new();
        if self.stored() > 0 {
            let mut text = format!("Imported {}", counted(self.stored(), "design"));
            if self.updated > 0 {
                let _ = write!(text, " ({} replaced an earlier copy)", self.updated);
            }
            text.push('.');
            sentences.push(text);
        }
        if self.identical > 0 {
            sentences.push(format!(
                "{} already in the library.",
                counted(self.identical, "file")
            ));
        }
        if !self.failed.is_empty() {
            let named: Vec<String> = self
                .failed
                .iter()
                .take(NOTICE_NAMED_FAILURES)
                .map(|(name, why)| format!("{} ({why})", notice_name(name)))
                .collect();
            let more = self.failed.len().saturating_sub(NOTICE_NAMED_FAILURES);
            let tail = if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            };
            sentences.push(format!(
                "{} could not be imported: {}{tail}.",
                counted(self.failed.len(), "file"),
                named.join("; ")
            ));
        }
        if self.unmeasured > 0 {
            let subject = if self.unmeasured == 1 {
                "1 design is".to_owned()
            } else {
                format!("{} designs are", self.unmeasured)
            };
            sentences.push(format!(
                "{subject} too large to measure here; the proportions come from the file."
            ));
        }
        if self.out_of_space {
            sentences.push(
                "The browser's storage for this site is full, so the remaining files were not \
                 imported. Delete designs from the library or free space."
                    .to_owned(),
            );
        }
        if sentences.is_empty() {
            return "Nothing to import.".to_owned();
        }
        sentences.extend(self.notes.iter().cloned());
        sentences.join(" ")
    }
}
