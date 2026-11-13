//! Builds a [`Design`] from a bare `.asc` file's own text, with the default preform
//! that bounds it.
//!
//! The desktop's catalogue-record path (a vault record, possibly with a design file
//! attached) stays in the desktop, which calls into this.

use pleochroma_cut_core::{
    Design, PreformSpec,
    native::{AttachmentBlob, DesignMetadata},
};

/// A preform sized to bound a design whose schedule may already close its own shape (a
/// real `.asc`'s tiers, or a placeholder reconstruction).
///
/// It is generous relative to the "masts of order 1" scale of `pleochroma_cut_core::preform`,
/// so a complete design's facets never clip it; only a schedule missing a closing facet
/// would touch its walls.
///
/// `length_over_width` is read from `lw_ratio` (a catalogue's recorded L/W) when it is a
/// positive finite number, so an oval design is not needlessly round; `1.0` otherwise.
#[must_use]
pub fn default_preform_for_schedule(
    schedule: &pleochroma_formats::asc::AscSchedule,
    lw_ratio: Option<&str>,
) -> PreformSpec {
    let length_over_width = lw_ratio
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(1.0);
    PreformSpec::cylinder_for_schedule(schedule, 3.0, length_over_width, 3.0)
}

/// A resolved design plus where its schedule came from.
pub struct LoadedDesign {
    /// The design.
    pub design: Design,
    /// `true` iff no real `.asc` was found and `design`'s schedule came from an
    /// angle-table placeholder reconstruction (every mast `0.0`).
    pub used_placeholder: bool,
    /// The real `.asc`'s own bare file name, `None` on the placeholder path.
    pub asc_filename: Option<String>,
    /// The real `.asc`'s exact original text, `None` on the placeholder path; fed to
    /// `pleochroma_cut_core::catalogue_asc` so a save can leave the `.asc`
    /// half byte-for-byte untouched.
    pub original_asc_text: Option<String>,
    /// The `[meta]` table of a design file the design came from; all fields
    /// unset when the source carried none (a bare `.asc` never does).
    pub metadata: DesignMetadata,
    /// The attachments of a design file the design came from, bytes decoded and
    /// verified, in file order; empty for a bare `.asc`.
    pub attachments: Vec<AttachmentBlob>,
}

/// Builds a [`LoadedDesign`] from a real `.asc` file's own text, always along the
/// "real schedule" outcome (`used_placeholder: false`).
///
/// # Errors
///
/// The parse failure's own message when `text` does not parse as `.asc` cutting
/// instructions at all.
pub fn design_from_asc_text(
    file_name: &str,
    text: &str,
    lw_ratio: Option<&str>,
) -> Result<LoadedDesign, String> {
    let schedule = pleochroma_formats::asc::parse_asc(text).map_err(|e| e.to_string())?;
    let preform = default_preform_for_schedule(&schedule, lw_ratio);
    Ok(LoadedDesign {
        design: Design::from_asc_schedule(preform, &schedule),
        used_placeholder: false,
        asc_filename: Some(file_name.to_string()),
        original_asc_text: Some(text.to_string()),
        metadata: DesignMetadata::default(),
        attachments: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn design_from_asc_text_parses_a_real_schedule() {
        let text = "GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n";
        let loaded = design_from_asc_text("design.asc", text, None).unwrap();
        assert!(!loaded.used_placeholder);
        assert_eq!(loaded.asc_filename.as_deref(), Some("design.asc"));
        assert_eq!(loaded.original_asc_text.as_deref(), Some(text));
        assert_eq!(loaded.design.tiers.len(), 1);
        assert!(loaded.metadata.is_empty());
        assert_eq!(loaded.attachments.len(), 0);
    }

    #[test]
    fn design_from_asc_text_rejects_unparseable_text() {
        assert!(design_from_asc_text("bad.asc", "not a real .asc schedule", None).is_err());
    }
}
