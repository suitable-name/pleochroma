//! Shared fixtures for [`super`]'s topic modules: two small real `.asc`
//! schedules, the [`Design`] builders on top of them and the design file round trip.

use crate::{
    design::Design,
    material::MaterialSelection,
    native::{DesignExtras, LoadedDesign, design_from_str, design_to_string},
    preform::PreformSpec,
};
use pleochroma::geometry::meet_solver::MeetConstraint;

/// A real, small schedule (two anchored tiers, no meet-derived structure needed)
/// reused by several tests below.
pub(super) const SIMPLE_ASC: &str = "GemCad 5.0\n\
     g 4 0.0\n\
     y 1 n\n\
     I 1.62\n\
     a 90.000000 1.00000000 0 1 2 3 G Set girdle thickness\n\
     a 0.000000 0.60000000 G Set stone size\n\
     a -0.000000 0.55000000 G Set stone size\n";

/// Same geometry as [`SIMPLE_ASC`], but each tier's `G` note already has the canonical text
/// that `constraint_notes` writes for a [`MeetConstraint::ScaleReference`] tier ("Set stone
/// size.", full stop included). Notes are regenerated from the constraint, so `SIMPLE_ASC`'s
/// "Set girdle thickness" would never re-export byte-identical. This is the file the
/// untouched-design preservation test needs.
pub(super) const FULLY_CANONICAL_ASC: &str = "GemCad 5.0\n\
     g 4 0.0\n\
     y 1 n\n\
     I 1.62\n\
     a 90.000000 1.00000000 0 1 2 3 G Set stone size.\n\
     a 0.000000 0.60000000 G Set stone size.\n\
     a -0.000000 0.55000000 G Set stone size.\n";

pub(super) fn simple_design() -> Design {
    let schedule = pleochroma_formats::asc::parse_asc(SIMPLE_ASC).expect("fixture must parse");
    let mut design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    design.girdle_diameter_mm = Some(6.5);
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        // `Design::effective_refractive_index` derives RI from a recognized material
        // name when there is no override; pinning this to the fixture's own `I 1.62`
        // keeps the "Diamond" selection (real n_D ~2.417) from changing what
        // `to_asc_schedule` exports for an otherwise-untouched design.
        refractive_index_override: Some(1.62),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
        color_pattern_override: None,
        color_variant_override: None,
    };
    design.tiers[1].constraint = MeetConstraint::MeetExisting;
    design.tiers[1].detached = vec![0.0, 2.0];
    design
}

/// Same base as [`simple_design`], but without its tier-1 `MeetExisting` override. Every tier
/// keeps the `ScaleReference` pin from [`crate::design::Design::from_asc_schedule`]. With the
/// override, `Design::solve` derives tier 1's mast from geometry instead of the recorded `0.6`,
/// so `to_asc_schedule()` would not reproduce the text. Used by the save-preserve and
/// custom-catalogue tests.
pub(super) fn fully_anchored_design() -> Design {
    let schedule =
        pleochroma_formats::asc::parse_asc(FULLY_CANONICAL_ASC).expect("fixture must parse");
    let mut design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    design.girdle_diameter_mm = Some(6.5);
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        // See `simple_design`'s comment on this field.
        refractive_index_override: Some(1.62),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
        color_pattern_override: None,
        color_variant_override: None,
    };
    design
}

/// A design with no scale-reference tier at all -- `Design::solve` cannot produce a
/// mast for it -- built directly (not via `Design::from_asc_schedule`) so its single
/// tier carries a real `imported_meet`/`original_notes`/`detached` for a draft round
/// trip to actually exercise.
pub(super) fn unsolved_design() -> Design {
    let mut design = Design::fresh(PreformSpec::block(2.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(crate::design::ConstraintTier {
        angle_deg: 30.0,
        name: "A".to_string(),
        indices: vec![0.0, 24.0, 48.0, 72.0],
        constraint: MeetConstraint::MeetExisting,
        imported_meet: Some(MeetConstraint::MeetNamed(vec!["B".to_string()])),
        original_notes: Some("Meet B".to_string()),
        detached: vec![24.0],
    });
    design
}

/// The text of `design` as a design file, with no extras.
pub(super) fn design_text(design: &Design) -> String {
    design_to_string(design, None, &DesignExtras::default()).expect("must serialize")
}

/// `design` written as a design file and opened again.
pub(super) fn reopen(design: &Design) -> LoadedDesign {
    design_from_str(&design_text(design)).expect("must open its own output")
}
