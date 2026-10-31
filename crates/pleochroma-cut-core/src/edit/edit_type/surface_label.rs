//! The cutter-facing sentence of an [`super::Edit::SetTierSurface`], for the undo and redo
//! hints and the history list.

use crate::design::TierSurface;

/// "Set surface finish of P1 to Commercial" for a set finish (with the number of facets that
/// differ, see [`TierSurface::text`]) and "Clear surface finish of P1" for a clear -- `label` is
/// the tier as the table shows it.
pub(super) fn describe_surface(label: &str, surface: Option<&TierSurface>) -> String {
    surface.map_or_else(
        || format!("Clear surface finish of {label}"),
        |surface| format!("Set surface finish of {label} to {}", surface.text()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::{SurfaceFinish, SurfaceGrade};

    #[test]
    fn the_sentence_names_the_tier_and_the_finish() {
        assert_eq!(describe_surface("P1", None), "Clear surface finish of P1");
        let commercial = TierSurface::of_tier(SurfaceFinish::of_grade(SurfaceGrade::Commercial));
        assert_eq!(
            describe_surface("P1", Some(&commercial)),
            "Set surface finish of P1 to Commercial"
        );
        let with_facet = commercial
            .with_facet(12.0, Some(SurfaceFinish::of_grade(SurfaceGrade::Prepolish)))
            .expect("states a finish");
        assert_eq!(
            describe_surface("C2", Some(&with_facet)),
            "Set surface finish of C2 to Commercial, 1 facet differs"
        );
    }
}
