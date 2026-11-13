//! The "manufacturability must not regress" gate: the baseline's warning counts and the
//! comparison every candidate passes before it is scored. Re-exported by [`super`].

use crate::manufacturability::ManufacturabilityWarning;

/// Running counts of the two mesh-dependent [`ManufacturabilityWarning`] variants an
/// angle-only edit can actually change. `FractionalIndex`/`OutOfOrderMeet`/
/// `MeetNameNotAscSafe` are excluded: they depend on `indices`/`constraint`, which
/// this module never touches (it only ever rewrites a free tier's `angle_deg`, and
/// an anchored tier's mast), so their counts are invariant across every candidate by
/// construction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::optimize) struct BaselineWarningCounts {
    vanishing: usize,
    undersized: usize,
}

impl BaselineWarningCounts {
    pub(in crate::optimize) fn count(warnings: &[ManufacturabilityWarning]) -> Self {
        let mut counts = Self::default();
        for warning in warnings {
            match warning {
                ManufacturabilityWarning::VanishingFacet { .. } => counts.vanishing += 1,
                ManufacturabilityWarning::UndersizedFacet { .. } => counts.undersized += 1,
                ManufacturabilityWarning::FractionalIndex { .. }
                | ManufacturabilityWarning::OutOfOrderMeet { .. }
                | ManufacturabilityWarning::MeetNameNotAscSafe { .. }
                | ManufacturabilityWarning::ConcaveTiersOmittedFromExport { .. }
                | ManufacturabilityWarning::ToolMissesStone { .. }
                | ManufacturabilityWarning::ToolBreaksThrough { .. }
                | ManufacturabilityWarning::ToolRemovesMeet { .. }
                | ManufacturabilityWarning::ToolsOverlap { .. }
                | ManufacturabilityWarning::ConcaveSliver { .. }
                | ManufacturabilityWarning::ToolRemovesHullVertex { .. }
                | ManufacturabilityWarning::ToolEnclosed { .. } => {}
            }
        }
        counts
    }
}

/// `true` iff `candidate` has strictly more vanishing or undersized facets than `baseline`.
///
/// [`super::evaluate_candidate_controlled`] applies this gate to every candidate. Matching
/// the starting design is not a regression.
pub(in crate::optimize) fn manufacturability_regressed(
    baseline: &BaselineWarningCounts,
    candidate_warnings: &[ManufacturabilityWarning],
) -> bool {
    let candidate = BaselineWarningCounts::count(candidate_warnings);
    candidate.vanishing > baseline.vanishing || candidate.undersized > baseline.undersized
}
