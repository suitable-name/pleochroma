//! What a `.psdd` design file carries besides the design itself, kept for the
//! open design so the next Save writes it back unchanged: the `[meta]` table (with its
//! unknown keys) and the byte-exact `[[attachments]]`.

use super::design_identity::{fresh_design_uuid, resolve_design_uuid, resolve_file_design_uuid};
use pleochroma_cut_core::native::{AttachmentBlob, DesignMetadata, RiRangeChoice};
use std::path::Path;

/// The `[meta]` table and attachments the open design came with.
///
/// Empty for a design that has neither (a new design, a bare `.asc`), apart from the
/// design's UUID, which every open design has (see
/// [`Self::with_design_uuid_assigned`]). A Save starts from these, overlays what the
/// design's library row says (see `native_io::design_meta`), and writes the result.
#[derive(Debug, Clone, Default)]
pub(in crate::gui) struct DesignFileExtras {
    /// The design file's `[meta]` table as loaded, or as last written by a Save.
    ///
    /// The RI range choice (`ri_min`, `ri_max`, `ri_range_source`) is stored here in the file.
    /// While the design is open the choice lives in `Design::ri_range_choice`, which Undo moves
    /// (`gui::editor::ri_range::field`): the open copies it there, and every write takes it back
    /// through `EditorState::file_extras_for_write`. Read it from the design, not from here.
    pub(in crate::gui::editor) metadata: DesignMetadata,
    /// The design file's attachments as loaded, in file order.
    pub(in crate::gui::editor) attachments: Vec<AttachmentBlob>,
    /// The range the file itself states, as `(low, high)`: what "From File" offers.
    ///
    /// Read from the metadata when the extras are built, so it is what the file said when it was
    /// opened (a `.psdd` `[meta]`, or a `.gcs` `ri_min` / `ri_max` carried in by the importer) or
    /// what the last Save wrote. Choosing Off or Estimate afterwards does not lose it.
    pub(in crate::gui::editor) file_ri_range: Option<(f64, Option<f64>)>,
}

impl DesignFileExtras {
    /// The extras a loaded design file (or catalogue attachment) carried.
    pub(in crate::gui::editor) fn new(
        metadata: DesignMetadata,
        attachments: Vec<AttachmentBlob>,
    ) -> Self {
        let file_ri_range = match metadata.ri_range_choice() {
            RiRangeChoice::Designer { low, high } => Some((low, high)),
            RiRangeChoice::Off | RiRangeChoice::Estimate => None,
        };
        Self {
            metadata,
            attachments,
            file_ri_range,
        }
    }

    /// These extras with the design's UUID (`[meta].id`) settled: the one the file
    /// already has, else -- for a design loaded from the local catalogue, whose entry
    /// `url` is `catalogue_url` -- the entry's deterministic UUID, else a fresh random
    /// one. See [`resolve_design_uuid`] for the rules.
    ///
    /// Every place that puts a design into the editor calls this, so the UUID exists
    /// from the moment the design opens, and the next Save or autosave writes it to the
    /// file.
    pub(in crate::gui::editor) fn with_design_uuid_assigned(
        mut self,
        catalogue_url: Option<&str>,
    ) -> Self {
        self.metadata.id = resolve_design_uuid(&self.metadata.id, catalogue_url, fresh_design_uuid);
        self
    }

    /// These extras with the design's UUID settled for a design opened from the file at
    /// `path`: the id the file already has, else the deterministic UUID of the file's
    /// location (see [`resolve_file_design_uuid`]), so a file that has no id yet gets the
    /// same one every time it is opened until a Save writes it in.
    pub(in crate::gui::editor) fn with_design_uuid_assigned_for_file(
        mut self,
        path: &Path,
    ) -> Self {
        self.metadata.id = resolve_file_design_uuid(&self.metadata.id, path, fresh_design_uuid);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_choice(choice: RiRangeChoice) -> DesignMetadata {
        let mut metadata = DesignMetadata::default();
        metadata.set_ri_range_choice(choice);
        metadata
    }

    #[test]
    fn a_range_the_file_states_is_remembered_for_from_file() {
        let extras = DesignFileExtras::new(
            with_choice(RiRangeChoice::Designer {
                low: 1.7,
                high: Some(2.15),
            }),
            Vec::new(),
        );
        assert_eq!(extras.file_ri_range, Some((1.7, Some(2.15))));
        let open_ended = DesignFileExtras::new(
            with_choice(RiRangeChoice::Designer {
                low: 1.63,
                high: None,
            }),
            Vec::new(),
        );
        assert_eq!(open_ended.file_ri_range, Some((1.63, None)));
    }

    #[test]
    fn no_stated_range_means_from_file_is_not_offered() {
        for choice in [RiRangeChoice::Off, RiRangeChoice::Estimate] {
            let extras = DesignFileExtras::new(with_choice(choice), Vec::new());
            assert_eq!(extras.file_ri_range, None, "{choice:?}");
        }
        assert_eq!(DesignFileExtras::default().file_ri_range, None);
    }

    #[test]
    fn changing_the_choice_later_keeps_what_the_file_stated() {
        let mut extras = DesignFileExtras::new(
            with_choice(RiRangeChoice::Designer {
                low: 1.7,
                high: None,
            }),
            Vec::new(),
        );
        extras.metadata.set_ri_range_choice(RiRangeChoice::Off);
        assert_eq!(extras.file_ri_range, Some((1.7, None)));
    }
}
