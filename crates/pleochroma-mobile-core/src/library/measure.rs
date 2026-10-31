//! Measured metadata for a design about to be stored, and carrying hand-entered metadata
//! over when a file replaces an existing entry.
//!
//! The rule that turns a measured solid into the stored proportions and shape, the designer
//! line and the merge are `pleochroma_vault::local`'s, the same ones the desktop importer
//! runs, so a row written here reads the same as one the desktop writes from the same file.
//! What is mobile's is the geometry: [`SolvedPlanes`] takes the planes from the shared solver
//! instead of the desktop's database-angle reconstruction; the numbers are the same measure
//! of the same solid. The solve-and-measure glue ([`SolvedSolid`]) lives in
//! `pleochroma_web_core::library_measure`, where the browser library uses it too.

use std::sync::atomic::AtomicBool;

use pleochroma_vault::{
    local::{SolidMeasure, SolidSource},
    model::detail::FacetingDiagramDetail,
};
use pleochroma_web_core::library_measure::SolvedSolid;

use super::design_source::design_of;

pub(super) use pleochroma_vault::local::{
    fill_designer_info, merge_reimport_metadata as merge_existing_metadata,
};

/// Fills the proportion fields and, when the file named none, the shape, from the
/// design's own solved planes. A design that does not read, solve or close leaves every
/// field as it was: a number that could not be measured is never invented.
pub(super) fn apply_measured_metadata(detail: &mut FacetingDiagramDetail) {
    pleochroma_vault::local::apply_measured_metadata(detail, &mut SolvedPlanes::default());
}

/// The mobile [`SolidSource`]: the planes of the solved design the attachments describe,
/// measured with `measure_solid`.
#[derive(Default)]
struct SolvedPlanes {
    /// The solid of the last [`SolidSource::measure`], for the girdle count.
    solid: SolvedSolid,
}

impl SolidSource for SolvedPlanes {
    fn measure(&mut self, detail: &FacetingDiagramDetail) -> Option<SolidMeasure> {
        let design = design_of(&detail.attached_files).ok()?;
        self.solid = SolvedSolid::of_design(&design, &AtomicBool::new(false));
        self.solid.measure(detail)
    }

    /// Classified from the SAME planes, so the girdle outline is measured.
    fn girdle_sides(&mut self) -> usize {
        self.solid.girdle_sides()
    }
}
