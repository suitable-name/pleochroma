//! Reading a stored design back and measuring it for the browser library.
//!
//! The vault's importer (`pleochroma_vault::local::import_bytes`) shares one rule that turns
//! a measured solid into the stored proportions, volume and shape; what differs per platform
//! is where the solid comes from ([`SolidSource`]). The browser's comes from the shared
//! solver: [`SolvedSolid`] holds the viewport planes of one solve and measures them with the
//! desktop's `measure_solid`, so a row written here reads the same as one the desktop and the
//! Android core write from the same file.
//!
//! The solve itself is the caller's: a small design is solved on the page
//! ([`SolvedSolid::of_design`]), a large one in a Worker whose `SolveOutcome` is handed to
//! [`SolvedSolid::from_outcome`]. A design that does not read, solve or close measures
//! nothing, and the stored fields are then left as the file had them.

use crate::solve::{SolveOutcome, SolveResponse, solve_design, to_solved_tiers};
use glam::DVec3;
use pleochroma::geometry::{
    GpuFacetPlane, girdle, meet_solver::SolvedTier, stone_metrics::measure_solid,
};
use pleochroma_cut_core::{Design, native::design_from_str};
use pleochroma_editor::loading::design_from_asc_text;
use pleochroma_vault::{
    local::{
        DesignFileKind, SolidMeasure, SolidSource, design_attachment_position,
        design_file_to_asc_text, is_native_design_name, native_design_attachment_position,
    },
    model::{detail::FacetingDiagramDetail, file::AttachedFile},
};
use std::sync::atomic::AtomicBool;

/// One solved solid: the figures its proportions are made of and the planes they came from.
#[derive(Debug, Clone, Default)]
pub struct SolvedSolid {
    /// The measured figures; `None` when nothing was solved or the solid does not close.
    measure: Option<SolidMeasure>,
    /// The planes the figures were measured from, kept for the girdle count.
    planes: Vec<GpuFacetPlane>,
    /// The solved masts of the solve (what a thumbnail is drawn from); `None` for a solid
    /// built from planes alone.
    tiers: Option<Vec<SolvedTier>>,
}

impl SolvedSolid {
    /// The solid bounded by `planes` (viewport planes: `n . x + d <= 0` is inside).
    #[must_use]
    pub fn from_planes(planes: Vec<GpuFacetPlane>) -> Self {
        let measure = measure_planes(&planes);
        Self {
            measure,
            planes,
            tiers: None,
        }
    }

    /// The solid of a finished solve; empty when the design did not solve.
    #[must_use]
    pub fn from_outcome(outcome: SolveOutcome) -> Self {
        let Some(solved) = outcome.solved.as_deref() else {
            return Self::default();
        };
        let tiers = to_solved_tiers(solved);
        Self::from_planes(
            outcome
                .planes
                .into_iter()
                .map(GpuFacetPlane::from)
                .collect(),
        )
        .with_tiers(tiers)
    }

    /// This solid carrying the solved masts `tiers` (one per tier of the design).
    #[must_use]
    pub fn with_tiers(mut self, tiers: Vec<SolvedTier>) -> Self {
        self.tiers = Some(tiers);
        self
    }

    /// The solved masts, when the solid came from a solve that produced them.
    #[must_use]
    pub fn tiers(&self) -> Option<&[SolvedTier]> {
        self.tiers.as_deref()
    }

    /// Solves `design` here and now (the page's tiny-design path) and measures it.
    ///
    /// Empty when the solve was cancelled, failed or the design does not close.
    #[must_use]
    pub fn of_design(design: &Design, cancel: &AtomicBool) -> Self {
        match solve_design(design, cancel) {
            SolveResponse::Solved(outcome) => Self::from_outcome(outcome),
            _ => Self::default(),
        }
    }

    /// The measured figures, if the solid closed.
    #[must_use]
    pub const fn measure_ref(&self) -> Option<&SolidMeasure> {
        self.measure.as_ref()
    }

    /// The planes the figures were measured from (empty when nothing was solved).
    #[must_use]
    pub fn planes(&self) -> &[GpuFacetPlane] {
        &self.planes
    }
}

impl SolidSource for SolvedSolid {
    fn measure(&mut self, _detail: &FacetingDiagramDetail) -> Option<SolidMeasure> {
        self.measure
    }

    /// Classified from the SAME planes, so the girdle outline is measured.
    fn girdle_sides(&mut self) -> usize {
        girdle::classify_girdle_plane_indices(&self.planes).len()
    }
}

/// The proportions of the solid `planes` bound, `None` when it is open or degenerate.
fn measure_planes(planes: &[GpuFacetPlane]) -> Option<SolidMeasure> {
    // `measure_solid` wants unit outward normals with `n.x <= m`; the viewport planes satisfy
    // `n.x + d = 0`, so `m = -d`.
    let measurable: Vec<(DVec3, f64)> = planes
        .iter()
        .map(|plane| {
            (
                DVec3::new(
                    f64::from(plane.normal[0]),
                    f64::from(plane.normal[1]),
                    f64::from(plane.normal[2]),
                ),
                -f64::from(plane.d),
            )
        })
        .collect();
    let metrics = measure_solid(&measurable)?;
    Some(SolidMeasure {
        length_axis: metrics.length_axis,
        width_axis: metrics.width_axis,
        total_height: metrics.total_height,
        crown_height: metrics.crown_height,
        pavilion_depth: metrics.pavilion_depth,
        volume: metrics.volume,
    })
}

/// The design the attachments of one entry describe, by the vault's rule: a design file first,
/// else the first `.asc`, `.gem` or `.gcs`.
///
/// # Errors
///
/// The entry holds no design file, or the file does not read (the reader's own message).
pub fn design_of_attachments(files: &[AttachedFile]) -> Result<Design, String> {
    let names = || files.iter().map(|file| file.name.as_str());
    let position = native_design_attachment_position(names())
        .or_else(|| design_attachment_position(names()).map(|(position, _)| position));
    let file = position
        .and_then(|position| files.get(position))
        .ok_or_else(|| "This entry holds no design file.".to_owned())?;
    design_of_file(file)
}

/// The design one stored file describes (a design file, `.asc`, `.gem` or `.gcs`).
///
/// A `.gem` or `.gcs` is converted to `.asc` cutting instructions first; the frosting a
/// `.gcs` states is applied.
///
/// # Errors
///
/// The file is not a design file or does not read.
pub fn design_of_file(file: &AttachedFile) -> Result<Design, String> {
    let name = &file.name;
    if is_native_design_name(name) {
        let text = std::str::from_utf8(&file.content)
            .map_err(|_| format!("\"{name}\" is not a text (UTF-8) design file."))?;
        return design_from_str(text)
            .map(|loaded| loaded.design)
            .map_err(|error| format!("\"{name}\" cannot be read: {error}"));
    }
    let kind = DesignFileKind::from_file_name(name)
        .ok_or_else(|| format!("\"{name}\" is not a design file."))?;
    let converted = design_file_to_asc_text(name, kind, &file.content)?;
    design_from_asc_text(&converted.asc_file_name, &converted.asc_text, None)
        .map(|loaded| {
            let mut design = loaded.design;
            design.apply_imported_frosting(&converted.frosting);
            design
        })
        .map_err(|error| format!("\"{name}\" cannot be read: {error}"))
}

#[cfg(test)]
mod tests;
